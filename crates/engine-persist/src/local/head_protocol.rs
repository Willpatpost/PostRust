//! The nine-step append protocol and the HEAD publication (split
//! from the local monolith by change reason, P3-L9 pure move): the
//! uncertain-window object-write ladder, the insert-once
//! verify-on-retry discipline, the HEAD.tmp + fsync + rename
//! linearization point, the post-rename root-directory sync, and the
//! on-demand reconciliation of an uncertain append. These change
//! when the append protocol's crash-window classification changes.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::time::Instant;

use engine_core::Encoder;

use crate::composite_manifest;
use crate::fault::FaultPoint;
use crate::hash::{Hash, hash_object};
use crate::medium::{AppendOutcome, MediumError, Reconciliation, StoredObject, UnknownReason};
use crate::record::{RecordKind, RevisionRecord};

#[cfg(unix)]
use super::OBJECTS_DIR;
use super::{HEAD_DOMAIN, HEAD_FILE, HEAD_TMP, HEAD_VERSION, LocalFileMedium, elapsed_nanos};

/// How an in-window object-write failure classifies.
enum ObjectWriteFailure {
    Unknown(UnknownReason),
    FailClosed(MediumError),
}

impl LocalFileMedium {
    // ------------------------------------------------------- append steps

    /// Steps 2–4 plus the record-level coherence checks, then steps
    /// 5–7 with every in-window failure classified as `Unknown`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn append_locked(
        &mut self,
        record: &RevisionRecord,
        objects: &[StoredObject],
        expected_parent: Option<Hash>,
        record_bytes: &[u8],
        record_id: &Hash,
        composite: bool,
    ) -> Result<AppendOutcome, MediumError> {
        self.fault_pre(FaultPoint::AfterLock)?;

        // Step 2: validate the observed HEAD.
        let validation_started = Instant::now();
        let observed = self.read_head_file()?;
        self.validate_observed(observed)?;
        self.timings.validation_nanos = elapsed_nanos(validation_started);
        self.fault_pre(FaultPoint::AfterValidation)?;

        // Steps 3–4: compare and reject stale writers.
        if observed != expected_parent {
            return Ok(AppendOutcome::RejectedStaleParent { observed });
        }
        match (observed, &record.kind) {
            (None, RecordKind::Founding { .. }) => {
                if record.epoch != 0 || record.parent.is_some() {
                    return Err(MediumError::Chain {
                        rule: "kind-incoherent",
                    });
                }
                if !record.depends_on.is_empty() {
                    return Err(MediumError::Chain {
                        rule: "depends-on-unresolvable",
                    });
                }
            }
            (Some(head), RecordKind::Regular) => {
                let cursor = self
                    .cursor
                    .as_ref()
                    .expect("validation established the cursor");
                // Checked arithmetic: an epoch at u64::MAX has no
                // valid successor and must fail closed, never wrap.
                if cursor.epoch.checked_add(1) != Some(record.epoch) {
                    return Err(MediumError::Chain {
                        rule: "epoch-regression",
                    });
                }
                if record.parent != Some(head) {
                    return Err(MediumError::Chain {
                        rule: "parent-mismatch",
                    });
                }
            }
            // Unreachable after the equality comparison above; kept as
            // a defensive stale classification.
            (observed, _) => return Ok(AppendOutcome::RejectedStaleParent { observed }),
        }
        if let Some(key) = &record.idempotency_key
            && let Some(cursor) = &self.cursor
            && cursor.seen_keys.contains(key)
        {
            return Err(MediumError::Chain {
                rule: "duplicate-idempotency-key",
            });
        }
        if let Some(cursor) = &self.cursor {
            // A dependency must name a prior revision of the
            // authoritative chain — the record-id index, never the
            // mixed reachable set (a content hash is not a revision)
            // — or a reclaim-indexed revision with its indexed epoch
            // strictly below the candidate's (PP-B3's condition 2).
            for dependency in &record.depends_on {
                let resolves = cursor.record_ids.contains(dependency)
                    || cursor
                        .reclaimed
                        .get(dependency)
                        .is_some_and(|&dep_epoch| dep_epoch < record.epoch);
                if !resolves {
                    return Err(MediumError::Chain {
                        rule: "depends-on-unresolvable",
                    });
                }
            }
        }
        self.fault_pre(FaultPoint::AfterComparison)?;

        // The K2-C2A PUBLICATION FENCE: revalidate every REUSED
        // composite dependency under HEAD.lock, immediately before
        // the first mutating write — the admission's caller-side
        // reads are advisory; a dependency that vanished (a sweep's
        // quarantine move, a committed reclamation delete) or
        // changed between the admission and this publication
        // REFUSES typed, with nothing mutated. The loader is the
        // lock-held internal hash-verifying read the walks use —
        // no public storage operation, no nested coordination.
        if composite {
            let supplied: BTreeSet<Hash> = objects.iter().map(StoredObject::hash).collect();
            crate::composite_append::fence_reused_dependencies(
                objects[0].bytes(),
                &supplied,
                |identity| self.load_referenced(identity),
            )?;
        }

        // Step 5: write every object the candidate needs (the
        // uncertain window opens).
        let objects_started = Instant::now();
        if let Err(failure) = self.write_all_objects(objects, record_bytes, record_id) {
            return match failure {
                ObjectWriteFailure::Unknown(reason) => Ok(AppendOutcome::Unknown { reason }),
                ObjectWriteFailure::FailClosed(error) => Err(error),
            };
        }

        // Step 6: publish HEAD (the linearization point is the rename
        // inside).
        if let Err(reason) = self.publish_head(record_id) {
            return Ok(AppendOutcome::Unknown { reason });
        }

        // Step 7: post-rename metadata synchronization.
        if let Err(reason) = self.fault_window(
            FaultPoint::AfterHeadRename,
            UnknownReason::PostPublishSyncFailed,
        ) {
            return Ok(AppendOutcome::Unknown { reason });
        }
        if let Err(reason) = self.sync_root_dir() {
            return Ok(AppendOutcome::Unknown { reason });
        }
        if let Err(reason) = self.fault_window(
            FaultPoint::AfterRootDirSync,
            UnknownReason::PostPublishSyncFailed,
        ) {
            return Ok(AppendOutcome::Unknown { reason });
        }
        self.timings.object_write_sync_nanos = elapsed_nanos(objects_started);

        // The trusted cursor advances to the accepted revision.
        // The K2-C1 corrective: a composite append advances the
        // cursor with the VERIFIED closure (the manifest's
        // identities — the admission already verified every chunk;
        // the manifest re-decode here is the same bounded object).
        if composite {
            let manifest =
                composite_manifest::decode(objects[0].bytes()).map_err(MediumError::Format)?;
            self.advance_cursor_with_closure(
                record,
                record_id,
                Some(&manifest.closure_identities()),
            );
        } else {
            self.advance_cursor(record, record_id);
        }
        Ok(AppendOutcome::Accepted {
            id: *record_id,
            tier: self.platform_tier,
        })
    }

    /// Step 5: insert-once writes for every object (contents first,
    /// the record last), then file syncs for the whole set, then the
    /// Unix objects-directory sync.
    fn write_all_objects(
        &mut self,
        objects: &[StoredObject],
        record_bytes: &[u8],
        record_id: &Hash,
    ) -> Result<(), ObjectWriteFailure> {
        // Phase A — write bytes (no syncs yet): the crash matrix's
        // "during creation" and "after creation, before
        // synchronization" boundaries.
        let mut written_first = false;
        for object in objects {
            self.insert_object(object.bytes(), &object.hash())?;
            if !written_first {
                written_first = true;
                if let Err(reason) =
                    self.fault_window(FaultPoint::MidObjectWrite, UnknownReason::ObjectWriteFailed)
                {
                    return Err(ObjectWriteFailure::Unknown(reason));
                }
            }
        }
        self.insert_object(record_bytes, record_id)?;
        if let Err(reason) =
            self.fault_window(FaultPoint::ObjectsWritten, UnknownReason::ObjectWriteFailed)
        {
            return Err(ObjectWriteFailure::Unknown(reason));
        }

        // Phase B — sync every object in the set (including
        // idempotently reused ones: a prior interrupted writer may
        // have left them unsynced). The handles open with WRITE
        // access: Windows `FlushFileBuffers` requires it (a read-only
        // handle fails with ERROR_ACCESS_DENIED), while POSIX fsync
        // accepts any descriptor — write access is the portable
        // intersection.
        let mut hashes: Vec<Hash> = objects.iter().map(StoredObject::hash).collect();
        hashes.push(*record_id);
        for hash in &hashes {
            let path = self.object_path(hash);
            let file = OpenOptions::new()
                .write(true)
                .open(&path)
                .map_err(|_| ObjectWriteFailure::Unknown(UnknownReason::ObjectSyncFailed))?;
            file.sync_all()
                .map_err(|_| ObjectWriteFailure::Unknown(UnknownReason::ObjectSyncFailed))?;
        }
        if let Err(reason) =
            self.fault_window(FaultPoint::ObjectsSynced, UnknownReason::ObjectSyncFailed)
        {
            return Err(ObjectWriteFailure::Unknown(reason));
        }

        // Phase C — the objects-directory sync (Unix only; §5.3.3: std
        // exposes no Windows directory flush, and the tier carries the
        // limitation).
        #[cfg(unix)]
        {
            let directory = File::open(self.root.join(OBJECTS_DIR))
                .map_err(|_| ObjectWriteFailure::Unknown(UnknownReason::ObjectSyncFailed))?;
            directory
                .sync_all()
                .map_err(|_| ObjectWriteFailure::Unknown(UnknownReason::ObjectSyncFailed))?;
        }
        if let Err(reason) = self.fault_window(
            FaultPoint::AfterObjectsDirSync,
            UnknownReason::ObjectSyncFailed,
        ) {
            return Err(ObjectWriteFailure::Unknown(reason));
        }
        Ok(())
    }

    /// Insert-once with verify-on-retry: an existing path is accepted
    /// only if its bytes hash to the expected identity; a mismatch is
    /// a corruption condition — fail-closed when the identity is
    /// chain-referenced, quarantine-then-rewrite when it is not (the
    /// §5.3.3 evidence reconciliation, never a silent overwrite).
    fn insert_object(&self, bytes: &[u8], hash: &Hash) -> Result<(), ObjectWriteFailure> {
        let path = self.object_path(hash);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(bytes)
                    .map_err(|_| ObjectWriteFailure::Unknown(UnknownReason::ObjectWriteFailed))?;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let existing = fs::read(&path)
                    .map_err(|_| ObjectWriteFailure::Unknown(UnknownReason::ObjectWriteFailed))?;
                if hash_object(&existing) != *hash {
                    let referenced = self
                        .cursor
                        .as_ref()
                        .is_some_and(|cursor| cursor.reachable.contains(hash));
                    if referenced {
                        return Err(ObjectWriteFailure::FailClosed(
                            MediumError::ReferencedObjectCorrupt { hash: *hash },
                        ));
                    }
                    self.quarantine_object(hash, "insert-once hash mismatch (unreferenced)")
                        .map_err(ObjectWriteFailure::FailClosed)?;
                    let mut file = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                        .map_err(|_| {
                            ObjectWriteFailure::Unknown(UnknownReason::ObjectWriteFailed)
                        })?;
                    file.write_all(bytes).map_err(|_| {
                        ObjectWriteFailure::Unknown(UnknownReason::ObjectWriteFailed)
                    })?;
                }
            }
            Err(_) => {
                return Err(ObjectWriteFailure::Unknown(
                    UnknownReason::ObjectWriteFailed,
                ));
            }
        }
        Ok(())
    }

    /// Step 6: write `HEAD.tmp`, fsync it, rename onto `HEAD`. The
    /// rename is the protocol's linearization point.
    fn publish_head(&self, id: &Hash) -> Result<(), UnknownReason> {
        let tmp = self.root.join(HEAD_TMP);
        let bytes = {
            let mut enc = Encoder::new();
            enc.write_str(HEAD_DOMAIN);
            enc.write_u32(HEAD_VERSION);
            enc.write_bytes(id.as_bytes());
            enc.into_bytes()
        };
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp)
            .map_err(|_| UnknownReason::HeadPublishFailed)?;
        file.write_all(&bytes)
            .map_err(|_| UnknownReason::HeadPublishFailed)?;
        self.fault_window(FaultPoint::HeadTmpWritten, UnknownReason::HeadPublishFailed)?;
        file.sync_all()
            .map_err(|_| UnknownReason::HeadPublishFailed)?;
        self.fault_window(FaultPoint::HeadTmpSynced, UnknownReason::HeadPublishFailed)?;
        drop(file);
        fs::rename(&tmp, self.root.join(HEAD_FILE))
            .map_err(|_| UnknownReason::HeadPublishFailed)?;
        Ok(())
    }

    /// Step 7 (Unix): the post-rename root-directory fsync.
    fn sync_root_dir(&self) -> Result<(), UnknownReason> {
        #[cfg(unix)]
        {
            let directory =
                File::open(&self.root).map_err(|_| UnknownReason::PostPublishSyncFailed)?;
            directory
                .sync_all()
                .map_err(|_| UnknownReason::PostPublishSyncFailed)?;
        }
        #[cfg(not(unix))]
        {
            // The documented std Windows sequence offers no
            // pre-rename parent-directory flush (§5.3.3); the
            // SyncTier carries the limitation honestly.
        }
        Ok(())
    }

    // ------------------------------------------------------ reconciliation

    pub(super) fn reconcile_locked(
        &mut self,
        candidate: &Hash,
    ) -> Result<Reconciliation, MediumError> {
        if let Err(error) = self.fault_pre(FaultPoint::ReconcileLocked) {
            return Ok(Reconciliation::Unverifiable {
                reason: error.to_string(),
            });
        }
        let observed = self.read_head_file()?;
        if let Err(error) = self.fault_pre(FaultPoint::ReconcileHeadRead) {
            return Ok(Reconciliation::Unverifiable {
                reason: error.to_string(),
            });
        }
        let mut walked = 0u64;
        let result = match observed {
            Some(head) => {
                // Full verification before any classification: the
                // cold walk establishes a fully valid authoritative
                // chain — the candidate's own revision, its content,
                // its dependencies, and every descendant are
                // hash-verified and coherence-checked. `Installed` is
                // only ever returned when the candidate's presence in
                // that verified chain is established (HEAD itself, or
                // a member of the walked revision-id set); a matching
                // parent hash alone is never proof. Corruption
                // anywhere fails closed (a typed error), never a
                // false confirmation.
                self.cursor = None;
                let summary = self.cold_walk_from(head)?;
                walked = summary.records;
                let installed = head == *candidate
                    || self
                        .cursor
                        .as_ref()
                        .is_some_and(|cursor| cursor.record_ids.contains(candidate));
                if installed {
                    Reconciliation::Installed { id: *candidate }
                } else {
                    // Never installed in a verified chain: the
                    // candidate object, if present, is unreferenced
                    // evidence of the ambiguous attempt — quarantine
                    // it (never delete), then report retry-safe
                    // absence.
                    if self.object_path(candidate).exists() {
                        self.quarantine_object(
                            candidate,
                            "reconcile: candidate not installed (unreferenced)",
                        )?;
                    }
                    Reconciliation::NotInstalled { head: Some(head) }
                }
            }
            None => {
                // No authoritative history exists (never founded, or
                // HEAD vanished): nothing can be verified installed —
                // the candidate object, if present, is unreferenced
                // evidence; quarantine it and report retry-safe
                // absence.
                if self.object_path(candidate).exists() {
                    self.quarantine_object(
                        candidate,
                        "reconcile: candidate not installed (unreferenced)",
                    )?;
                }
                Reconciliation::NotInstalled { head: None }
            }
        };
        self.walks.last_reconcile_walked = walked;
        Ok(result)
    }
}
