//! The destructive reclamation execution under HEAD.lock (split
//! from the local monolith by change reason, P3-L9 pure move): the
//! reclaim-family CanonicalMedium surface, the audit
//! authentication and survivor partition, the OC-C2/V2X-CP2 begin
//! admissions and their locked internals, the quarantine-family
//! probing, the authorized deletion with its completion accounting,
//! and the crash-test process-exit seams. These change when the
//! verify-before-delete reclamation semantics change.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io;
use std::path::PathBuf;
// (RMD-03) The atomic import rides the crash-test-seams feature: the
// only consumers are the gated reclamation process-exit statics.
#[cfg(feature = "crash-test-seams")]
use std::sync::atomic::{AtomicBool, Ordering};

use crate::checkpoint::{CHECKPOINT_METADATA_DOMAIN, CheckpointMetadata};
use crate::fault::FaultPoint;
use crate::hash::{Hash, hash_object};
use crate::medium::{CanonicalMedium, MediumError, StoredObject};
use crate::record::RevisionRecord;

use super::{HEAD_FILE, LocalFileMedium, OBJECTS_DIR, QUARANTINE_DIR, io_err};

/// (V2X-CP2) The reusable lock-held reader adapter— the internal
/// non-locking hash-verified object reads backing both
/// [`crate::reclaim::BeginReads`] kinds (the V1's fn-local pattern
/// lifted to the module level for the V2 admission and
/// verification).
struct LocalLockedReads<'a> {
    medium: &'a LocalFileMedium,
}
impl crate::reclaim::BeginReads for LocalLockedReads<'_> {
    fn read_record_bytes(&mut self, id: &Hash) -> Result<Vec<u8>, MediumError> {
        self.medium.load_object(id)
    }
    fn read_object_bytes(&mut self, hash: &Hash) -> Result<Vec<u8>, MediumError> {
        self.medium.load_object(hash)
    }
}

/// The trusted reclamation internals (K2-C2B P2) — all under the
/// already-held HEAD.lock, all using the INTERNAL hash-verified
/// read primitives (the public reads' own lock/unlock pairs would
/// break this boundary).
impl LocalFileMedium {
    /// The canonical audit authentication under the boundary: the
    /// gap-aware descent from the authoritative HEAD over the
    /// internal reads — the exact audit identity (the hash-verified
    /// load BY identity), the MK derived from the decoded ledger,
    /// the deterministic key, the requested MK's metadata bound by
    /// revision identity, the SK/digest/seed bindings, and the exact
    /// units-vs-index membership. The RECOVERY-SAFE form: the
    /// per-unit manifest reread is NOT here (the main flow applies
    /// it with the resume tolerance — an absent retiring record is
    /// accounted for by the durable audit).
    pub(super) fn authenticate_audit_locked(
        &self,
        audit_record: &Hash,
    ) -> Result<(crate::reclaim::ReclamationLedger, CheckpointMetadata, Hash), MediumError> {
        const MAX_DESCENT: u64 = 1 << 20;
        let mut current = self
            .read_head_file()?
            .ok_or(MediumError::Chain { rule: "unfounded" })?;
        let head = current;
        let mut ledger: Option<crate::reclaim::ReclamationLedger> = None;
        let mut metadata: Option<CheckpointMetadata> = None;
        let mut skip_table: Option<CheckpointMetadata> = None;
        for _ in 0..MAX_DESCENT {
            let bytes = self.load_object(&current)?;
            let record = RevisionRecord::decode(&bytes).map_err(MediumError::Format)?;
            let parent = record.parent;
            if let Some(object) = record.content_refs.first().copied() {
                let object_bytes = self.load_object(&object)?;
                let domain = StoredObject::new(object_bytes.clone())?.domain().to_owned();
                if domain == CHECKPOINT_METADATA_DOMAIN {
                    let decoded =
                        CheckpointMetadata::decode(&object_bytes).map_err(MediumError::Format)?;
                    skip_table = Some(decoded.clone());
                    if let Some(ledger) = ledger.as_ref()
                        && current == ledger.mk
                        && decoded.sk != parent.unwrap_or(Hash::ZERO)
                    {
                        return Err(MediumError::Chain {
                            rule: "reclaim/mk-binding-mismatch",
                        });
                    }
                    if let Some(ledger) = ledger.as_ref()
                        && current == ledger.mk
                    {
                        metadata = Some(decoded);
                    }
                } else if domain == crate::reclaim::RECLAMATION_LEDGER_DOMAIN
                    && current == *audit_record
                {
                    let decoded = crate::reclaim::ReclamationLedger::decode(&object_bytes)
                        .map_err(MediumError::Format)?;
                    let expected = crate::reclaim::reclamation_ledger_idempotency_key(&decoded.mk);
                    if record.idempotency_key != Some(expected) {
                        return Err(MediumError::Chain {
                            rule: "reclaim/audit-key-mismatch",
                        });
                    }
                    ledger = Some(decoded);
                }
            }
            if ledger.is_some() && metadata.is_some() {
                break;
            }
            let Some(parent) = parent else {
                break;
            };
            if self.object_path(&parent).is_file() {
                current = parent;
            } else {
                let Some(table) = skip_table.as_ref() else {
                    return Err(MediumError::MissingObject { hash: parent });
                };
                let target = if table.sk == parent {
                    table
                        .self_skip()
                        .map(|entry| entry.nearest_retained_ancestor)
                } else {
                    table
                        .skip_for(&current)
                        .map(|entry| entry.nearest_retained_ancestor)
                };
                let Some(target) = target else {
                    return Err(MediumError::MissingObject { hash: parent });
                };
                current = target;
            }
        }
        let Some(ledger) = ledger else {
            return Err(MediumError::Chain {
                rule: "reclaim/audit-not-canonical",
            });
        };
        let Some(metadata) = metadata else {
            return Err(MediumError::Chain {
                rule: "reclaim/mk-not-a-checkpoint",
            });
        };
        if ledger.sk != metadata.sk
            || ledger.range_digest != metadata.range_digest
            || ledger.seed != metadata.seed
        {
            return Err(MediumError::Chain {
                rule: "reclaim/binding-mismatch",
            });
        }
        let index: BTreeSet<Hash> = metadata.index.iter().map(|entry| entry.hash).collect();
        let units: BTreeSet<Hash> = ledger.units.iter().map(|unit| unit.record).collect();
        if index.len() != metadata.index.len()
            || units.len() != ledger.units.len()
            || index != units
        {
            return Err(MediumError::Chain {
                rule: "reclaim/units-index-mismatch",
            });
        }
        Ok((ledger, metadata, head))
    }

    /// The survivor set under the boundary: the exclusion walk over
    /// the audited retiring records (pure `&self` — the authoritative
    /// cursor is never touched).
    pub(super) fn survivor_set_locked(
        &self,
        head: Hash,
        ledger: &crate::reclaim::ReclamationLedger,
    ) -> Result<BTreeSet<Hash>, MediumError> {
        let exclusion: BTreeSet<Hash> = ledger.units.iter().map(|unit| unit.record).collect();
        let (_summary, simulated) = self.cold_walk_excluding(head, &exclusion)?;
        Ok(simulated.reachable)
    }

    /// The OC-C2 trusted begin admission and publication under the
    /// exclusive HEAD.lock (the complete safety-critical sequence
    /// coordinated; no public trait method, no nested lock): the
    /// internal audit authentication (the exact identity, the key,
    /// the MK-bound metadata), the shared first-execution evidence
    /// core over the INTERNAL hash-verified reads (every record
    /// present including SK, the manifest equality, the INDEPENDENT
    /// digest recomputation), the OC-P2 tier gate, and the
    /// publication via append_locked with the truthful outcome
    /// classification. NOT connected to the destructive execution
    /// path (OC-C3).
    pub fn begin_reclamation(
        &mut self,
        audit: &Hash,
        mk: &Hash,
        tick: u64,
    ) -> Result<crate::reclaim::BeginPublicationOutcome, MediumError> {
        self.lock()?;
        let outcome = self.begin_reclamation_locked(audit, mk, tick);
        // (C2-R7) The outcome-preservation discipline (the reviewed
        // append wrapper's rule): an unlock failure must never
        // replace an already established Accepted or Unknown
        // publication disposition with an ordinary definite error —
        // the OS releases the lock at handle close or process death
        // either way.
        let _ = self.unlock();
        outcome
    }

    fn begin_reclamation_locked(
        &mut self,
        audit: &Hash,
        mk: &Hash,
        tick: u64,
    ) -> Result<crate::reclaim::BeginPublicationOutcome, MediumError> {
        // (A) The exact canonical audit identity over the internal
        // reads: the gap-aware descent, the deterministic key, the
        // strict decode, the MK binding.
        let (ledger, metadata, _head) = self.authenticate_audit_locked(audit)?;
        if ledger.mk != *mk {
            return Err(MediumError::Chain {
                rule: "begin-admission/mk-mismatch",
            });
        }
        // (F4) The gap-aware founding descent (the shared
        // reader-parameterized helper over the internal reads — a
        // missing parent is a gap crossed only under an
        // authenticated skip table, never mistaken for the
        // founding).
        let head_for_founding = self
            .read_head_file()?
            .ok_or(MediumError::Chain { rule: "unfounded" })?;
        let founding = crate::reclaim::founding_identity(
            head_for_founding,
            |identity| self.load_object(identity),
            |identity| self.load_object(identity),
            |identity| {
                // (C2-R2) The typed presence: a genuine NotFound is
                // the absence; every other filesystem error refuses —
                // never converted to a missing-parent observation.
                match std::fs::metadata(self.object_path(identity)) {
                    Ok(_) => Ok(true),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                    Err(e) => Err(io_err(
                        "founding presence probe",
                        &self.object_path(identity),
                        e,
                    )),
                }
            },
        )?;
        // (B-D) The shared first-execution evidence core over the
        // internal hash-verified object reads.
        let mut begin = crate::reclaim::verify_first_execution_evidence(
            &ledger,
            &metadata,
            &founding,
            |identity| self.load_object(identity),
            |identity| self.load_object(identity),
        )?;
        begin.source = *audit;
        // (OC-P2) The tier gate: the begin publication requires the
        // durable-authorization tier — the Windows ProcessCrashOnly
        // provider refuses fail-closed (the unrelated non-destructive
        // storage behavior untouched).
        if self.durability_tier() != crate::medium::SyncTier::FullPosixSequence {
            return Err(MediumError::Chain {
                rule: "begin-admission/tier-insufficient",
            });
        }
        // (C2-R6) The authoritative-chain validation — the SAME
        // trusted-cursor discipline the normal append applies under
        // the held lock (O(0) on the verified head, a bounded suffix
        // walk on divergence, the cold walk otherwise): a chain
        // carrying an otherwise authentic audit and retirement
        // evidence but malformed revisions above the audit never
        // receives a trusted begin — the early duplicate outcome is
        // conditional on the chain's validity, and so is the
        // installation (the internal append revalidates O(0) on the
        // now-verified head).
        self.validate_observed(self.read_head_file()?)?;
        // The duplicate resolution FIRST: a same-key begin already
        // installed is discovered and EXACTLY verified — never
        // inferred from key occupancy.
        if let Some((installed_id, installed)) = self.find_installed_begin_locked(mk)? {
            if installed == begin {
                return Ok(crate::reclaim::BeginPublicationOutcome::DuplicateVerified {
                    id: installed_id,
                });
            }
            return Err(MediumError::Chain {
                rule: "begin-admission/duplicate-content-mismatch",
            });
        }
        // (6-D) The publication: the canonical revision against the
        // authoritative HEAD, via the internal append (the lock
        // already held).
        let head = self
            .read_head_file()?
            .ok_or(MediumError::Chain { rule: "unfounded" })?;
        let parent_epoch = self.load_object(&head).map(|b| {
            RevisionRecord::decode(&b)
                .map(|r| r.epoch)
                .map_err(MediumError::Format)
        })??;
        parent_epoch.checked_add(1).ok_or(MediumError::Chain {
            rule: "epoch-regression",
        })?;
        let key = crate::reclaim::reclamation_begin_idempotency_key(&begin.mk);
        let object = StoredObject::new(begin.canonical_bytes())?;
        let record = RevisionRecord::regular(head, parent_epoch, tick)
            .with_idempotency_key(key)
            .with_content_refs(vec![object.hash()]);
        let record_bytes = record.canonical_bytes();
        let record_id = crate::hash::hash_object(&record_bytes);
        let admitted =
            crate::medium::AdmittedAppend::generic(&record, std::slice::from_ref(&object));
        let outcome = self.append_admitted_locked(admitted, Some(head))?;
        match outcome {
            crate::medium::AppendOutcome::Accepted { id, tier } => {
                // (C2-R8) The ACTUAL durability tier of the accepted
                // append — propagated, never synthesized (the OC-P2
                // gate above already established FullPosixSequence;
                // ProcessCrashOnly never reaches this arm).
                Ok(crate::reclaim::BeginPublicationOutcome::Accepted { id, tier })
            }
            crate::medium::AppendOutcome::RejectedStaleParent { .. } => {
                // Under the held lock a stale parent means an
                // unexpected external mutation — the conservative
                // typed refusal (a bounded retry belongs to the
                // caller's next maintenance pass).
                Err(MediumError::Chain {
                    rule: "begin-admission/stale-parent",
                })
            }
            crate::medium::AppendOutcome::Unknown { .. } => {
                match self.reconcile_locked(&record_id)? {
                    crate::medium::Reconciliation::Installed { id } => {
                        // Membership proven; the durability
                        // acknowledgment UNCONFIRMED (the OC-C3
                        // reaffirmation upgrades it).
                        Ok(crate::reclaim::BeginPublicationOutcome::ReconciledInstalled { id })
                    }
                    crate::medium::Reconciliation::NotInstalled { .. } => {
                        Ok(crate::reclaim::BeginPublicationOutcome::ReconciledNotInstalled)
                    }
                    crate::medium::Reconciliation::Unverifiable { .. } => Err(MediumError::Chain {
                        rule: "begin-admission/reconcile-unverifiable",
                    }),
                }
            }
        }
    }

    /// (OC-C3 CP-1) The lock-held installed-begin authority
    /// verification: the recovery-safe audit authentication (no
    /// deleted original retirement record required), the
    /// founding-derived seed, the shared canonical begin verifier
    /// over the internal lock-held reads, the platform tier gate,
    /// and — only on the eligible tier — the complete reaffirmation
    /// synchronization sequence. The grade is ESTABLISHED, never
    /// assumed; every failure is typed and fail-closed.
    pub(super) fn verify_begin_authority_locked(
        &self,
        audit: &Hash,
        mk: &Hash,
    ) -> Result<crate::reclaim::BeginAuthority, MediumError> {
        let (ledger, metadata, _head) = self.authenticate_audit_locked(audit)?;
        if ledger.mk != *mk {
            return Err(MediumError::Chain {
                rule: "begin-authority/mk-mismatch",
            });
        }
        let head = self
            .read_head_file()?
            .ok_or(MediumError::Chain { rule: "unfounded" })?;
        // The founding-derived seed (the E1 discipline — the
        // recovery-safe gap-aware descent; the founding is
        // permanent, never a deleted range member).
        let founding = crate::reclaim::founding_identity(
            head,
            |identity| self.load_object(identity),
            |identity| self.load_object(identity),
            |identity| {
                // (C2-R2) The typed presence: a genuine NotFound is
                // the absence; every other filesystem error refuses.
                match fs::metadata(self.object_path(identity)) {
                    Ok(_) => Ok(true),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
                    Err(e) => Err(io_err(
                        "founding presence probe",
                        &self.object_path(identity),
                        e,
                    )),
                }
            },
        )?;
        crate::reclaim::verify_founding_seed(&metadata, &founding)?;
        // The shared canonical begin verifier over ONE lock-held
        // reader (the internal non-locking hash-verified reads).
        struct LockedBeginReads<'a> {
            medium: &'a LocalFileMedium,
        }
        impl crate::reclaim::BeginReads for LockedBeginReads<'_> {
            fn read_record_bytes(&mut self, id: &Hash) -> Result<Vec<u8>, MediumError> {
                self.medium.load_object(id)
            }
            fn read_object_bytes(&mut self, hash: &Hash) -> Result<Vec<u8>, MediumError> {
                self.medium.load_object(hash)
            }
        }
        let mut reads = LockedBeginReads { medium: self };
        let Some((id, _begin)) =
            crate::reclaim::verify_installed_begin(&mut reads, head, audit, mk, &ledger)?
        else {
            return Err(MediumError::Chain {
                rule: "begin-authority/absent",
            });
        };
        // (OC-P2) The platform tier gate: ProcessCrashOnly never
        // grants the destructive-authorization grade — an
        // installed, restored, or otherwise discoverable begin
        // cannot override the platform limitation.
        if self.durability_tier() != crate::medium::SyncTier::FullPosixSequence {
            return Err(MediumError::Chain {
                rule: "begin-authority/tier-insufficient",
            });
        }
        // The complete reaffirmation sequence — FullPosixSequence
        // only after every required operation succeeds.
        self.reaffirm_begin_authority_locked(head, id)?;
        Ok(crate::reclaim::BeginAuthority {
            id,
            tier: crate::medium::SyncTier::FullPosixSequence,
        })
    }

    /// (V2X-CP2) The plan-bound V2 begin admission under the held
    /// lock: (A) the ALL-AUDITS-FIRST verification — the COMPLETE
    /// accepted CP1 contract re-derived under THIS lock over the
    /// internal hash-verifying reads (a pre-lock verification never
    /// substitutes); only the exact ExecutionReady positive admits;
    /// (B) the begin constructed from the verified values only —
    /// the ratified V2 source semantics (source = the installed
    /// plan REVISION-RECORD identity; the digest and seed the
    /// checkpoint's own values the plan's binding just verified);
    /// (C) the OC-P2 tier gate (Windows ProcessCrashOnly refuses
    /// fail-closed); (D) the authoritative-chain validation (the
    /// C2-R6 discipline); (E) the duplicate resolution FIRST — the
    /// exact-content check IS the V1/V2 source-form exclusion (an
    /// installed V1-bound begin under the same key differs in
    /// source: the typed conflict, never a reinterpretation);
    /// (F) the canonical CAS publication and the Unknown
    /// reconciliation (the truthful outcome model — never a guess).
    pub(super) fn begin_reclamation_v2_locked(
        &mut self,
        mk: &Hash,
        tick: u64,
    ) -> Result<crate::reclaim::BeginPublicationOutcome, MediumError> {
        // (A) The all-audits-first CP1 contract under this lock.
        let head = self
            .read_head_file()?
            .ok_or(MediumError::Chain { rule: "unfounded" })?;
        let (plan_record, range_digest, seed) = {
            let mut reads = LocalLockedReads { medium: &*self };
            match crate::reclaim::verify_v2_execution_readiness_via(&mut reads, head, mk)? {
                crate::reclaim::V2ExecutionReadiness::ExecutionReady { plan_record, .. } => {
                    // The begin's digest and seed: the checkpoint's
                    // own permanent values (the plan's binding to
                    // them was verified inside the readiness
                    // contract) — re-read under the same lock.
                    let Some((_id, meta_bytes)) =
                        crate::reclaim::find_on_chain_via(&mut reads, head, None, Some(mk))?
                    else {
                        return Err(MediumError::Chain {
                            rule: "v2-begin-admission/mk-not-installed",
                        });
                    };
                    let metadata =
                        CheckpointMetadata::decode(&meta_bytes).map_err(MediumError::Format)?;
                    (plan_record, metadata.range_digest, metadata.seed)
                }
                _ => {
                    return Err(MediumError::Chain {
                        rule: "v2-begin-admission/not-execution-ready",
                    });
                }
            }
        };
        // (B) The begin from the verified values only.
        let begin = crate::reclaim::ReclamationBegin {
            mk: *mk,
            source: plan_record,
            range_digest,
            seed,
        };
        // (C) The tier gate.
        if self.durability_tier() != crate::medium::SyncTier::FullPosixSequence {
            return Err(MediumError::Chain {
                rule: "v2-begin-admission/tier-insufficient",
            });
        }
        // (D) The authoritative-chain validation.
        self.validate_observed(self.read_head_file()?)?;
        // (E) The duplicate resolution.
        if let Some((installed_id, installed)) = self.find_installed_begin_locked(mk)? {
            if installed == begin {
                // (V2X-CP2 F1) A byte-identical installed begin is
                // DuplicateVerified ONLY when the installed begin's
                // own authority holds — the strict temporal position
                // (the plan revision and every plan-listed audit
                // strictly BELOW the begin) and the complete
                // retained membership, re-proved here under the
                // same lock. An adversarially installed
                // byte-identical begin sitting above a
                // later-published audit is a typed temporal refusal,
                // never a trusted duplicate: "DuplicateVerified"
                // asserts verified authority, never merely occupied
                // bytes under the key.
                let dup_head = self
                    .read_head_file()?
                    .ok_or(MediumError::Chain { rule: "unfounded" })?;
                let mut reads = LocalLockedReads { medium: &*self };
                crate::reclaim::verify_v2_begin_authority_evidence_via(&mut reads, dup_head, mk)?;
                return Ok(crate::reclaim::BeginPublicationOutcome::DuplicateVerified {
                    id: installed_id,
                });
            }
            return Err(MediumError::Chain {
                rule: "v2-begin-admission/duplicate-content-mismatch",
            });
        }
        // (F) The publication.
        let head = self
            .read_head_file()?
            .ok_or(MediumError::Chain { rule: "unfounded" })?;
        let parent_bytes = self.load_object(&head)?;
        let parent = RevisionRecord::decode(&parent_bytes).map_err(MediumError::Format)?;
        let parent_epoch = parent.epoch;
        parent_epoch.checked_add(1).ok_or(MediumError::Chain {
            rule: "epoch-regression",
        })?;
        let key = crate::reclaim::reclamation_begin_idempotency_key(&begin.mk);
        let object = StoredObject::new(begin.canonical_bytes())?;
        let record = RevisionRecord::regular(head, parent_epoch, tick)
            .with_idempotency_key(key)
            .with_content_refs(vec![object.hash()]);
        let record_bytes = record.canonical_bytes();
        let record_id = crate::hash::hash_object(&record_bytes);
        let admitted =
            crate::medium::AdmittedAppend::generic(&record, std::slice::from_ref(&object));
        let outcome = self.append_admitted_locked(admitted, Some(head))?;
        match outcome {
            crate::medium::AppendOutcome::Accepted { id, tier } => {
                Ok(crate::reclaim::BeginPublicationOutcome::Accepted { id, tier })
            }
            crate::medium::AppendOutcome::RejectedStaleParent { .. } => Err(MediumError::Chain {
                rule: "v2-begin-admission/stale-parent",
            }),
            crate::medium::AppendOutcome::Unknown { .. } => {
                match self.reconcile_locked(&record_id)? {
                    crate::medium::Reconciliation::Installed { id } => {
                        Ok(crate::reclaim::BeginPublicationOutcome::ReconciledInstalled { id })
                    }
                    crate::medium::Reconciliation::NotInstalled { .. } => {
                        Ok(crate::reclaim::BeginPublicationOutcome::ReconciledNotInstalled)
                    }
                    crate::medium::Reconciliation::Unverifiable { .. } => Err(MediumError::Chain {
                        rule: "v2-begin-admission/reconcile-unverifiable",
                    }),
                }
            }
        }
    }

    /// (V2X-CP2) The retained-evidence V2 begin authority under the
    /// held lock: the recovery-safe verification (the retained
    /// plan/audits/begin + the authenticated skip tables — no
    /// original retirement record required), the OC-P2 tier gate,
    /// and — only on the eligible tier — the complete reaffirmation
    /// synchronization sequence (the HEAD→founding walk covers the
    /// plan, every audit, and the begin: all on the ancestry). The
    /// grade is ESTABLISHED, never assumed.
    pub(super) fn verify_begin_authority_v2_locked(
        &self,
        mk: &Hash,
    ) -> Result<crate::reclaim::BeginAuthority, MediumError> {
        let head = self
            .read_head_file()?
            .ok_or(MediumError::Chain { rule: "unfounded" })?;
        let mut reads = LocalLockedReads { medium: self };
        let evidence =
            crate::reclaim::verify_v2_begin_authority_evidence_via(&mut reads, head, mk)?;
        // (OC-P2) The platform tier gate: ProcessCrashOnly never
        // grants the destructive-authorization grade.
        if self.durability_tier() != crate::medium::SyncTier::FullPosixSequence {
            return Err(MediumError::Chain {
                rule: "v2-begin-authority/tier-insufficient",
            });
        }
        // The complete reaffirmation sequence — FullPosixSequence
        // only after every required operation succeeds.
        self.reaffirm_begin_authority_locked(head, evidence.begin_id)?;
        Ok(crate::reclaim::BeginAuthority {
            id: evidence.begin_id,
            tier: crate::medium::SyncTier::FullPosixSequence,
        })
    }

    /// (V2X-CP3-B-P2A-C5b) The provider-coordinated observation
    /// under the HELD HEAD.lock: the authoritative head read from
    /// the HEAD file INSIDE the lock scope, then the accepted C5a
    /// observation walk over the internal hash-verifying reads —
    /// the head read and the whole walk inside ONE coordination
    /// scope. READ-ONLY: no publication, no mutation, no cursor
    /// change — nothing but the walk's own reads. An unfounded
    /// campaign refuses typed; the promoted claim is the
    /// coordinated-scope observation only (see the
    /// [`crate::range_installation::CoordinatedObservation`]
    /// contract).
    pub(super) fn observe_range_terminal_under_coordination_locked(
        &self,
        expected: &crate::range_completion::RangeCompletionContext,
        limits: &crate::range_installation::RangeInstallationLimits,
    ) -> Result<crate::range_installation::CoordinatedObservation, MediumError> {
        let coordinated_head = self
            .read_head_file()?
            .ok_or(MediumError::Chain { rule: "unfounded" })?;
        let mut reads = LocalLockedReads { medium: self };
        crate::range_installation::observe_coordinated(
            &mut reads,
            &coordinated_head,
            expected,
            limits,
        )
    }

    /// (OC-C3 CP-1) The reaffirmation synchronization sequence under
    /// the held lock: the complete recovery-relevant
    /// authorization-evidence closure — every record on the
    /// gap-aware HEAD-to-founding ancestry and each record's first
    /// content-reference object (the checkpoint/skip metadata),
    /// which includes the begin's own record and payload object —
    /// fsynced; then the covering objects directory; then the HEAD
    /// file; then the root directory. The sequence aborts on the
    /// first failure (the typed I/O refusal — never success): an
    /// interrupted or failed reaffirmation can never return the
    /// authority grade, and (the CP-2 invariant) no deletion ever
    /// precedes a completed sequence.
    fn reaffirm_begin_authority_locked(&self, head: Hash, begin: Hash) -> Result<(), MediumError> {
        const MAX_DESCENT: u64 = 1 << 20;
        // The collecting walk: HEAD to the founding, gap-aware
        // (the same authenticated skip-table discipline), gathering
        // every walked record file and its first content-reference
        // object — exactly the future re-verification set.
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        let mut current = head;
        let mut seen = std::collections::BTreeSet::new();
        let mut seen_begin = false;
        let mut skip_table: Option<CheckpointMetadata> = None;
        for _ in 0..MAX_DESCENT {
            if !seen.insert(current) {
                return Err(MediumError::Chain { rule: "cycle" });
            }
            let bytes = self.load_object(&current)?;
            let record = RevisionRecord::decode(&bytes).map_err(MediumError::Format)?;
            if current == begin {
                seen_begin = true;
            }
            if let Some(object) = record.content_refs.first().copied() {
                let object_bytes = self.load_object(&object)?;
                if StoredObject::new(object_bytes.clone())?.domain()
                    == crate::checkpoint::CHECKPOINT_METADATA_DOMAIN
                {
                    skip_table = Some(
                        CheckpointMetadata::decode(&object_bytes).map_err(MediumError::Format)?,
                    );
                }
                files.push(self.object_path(&object));
            }
            files.push(self.object_path(&current));
            let Some(parent) = record.parent else {
                break;
            };
            if !self.object_path(&parent).is_file() {
                let Some(table) = skip_table.as_ref() else {
                    return Err(MediumError::MissingObject { hash: parent });
                };
                let target = if table.sk == parent {
                    table
                        .self_skip()
                        .map(|entry| entry.nearest_retained_ancestor)
                } else {
                    table
                        .skip_for(&current)
                        .map(|entry| entry.nearest_retained_ancestor)
                };
                let Some(target) = target else {
                    return Err(MediumError::MissingObject { hash: parent });
                };
                current = target;
            } else {
                current = parent;
            }
        }
        if !seen_begin {
            // Defensive: the shared verifier already proved the
            // begin's membership; a walk that fails to reproduce it
            // is an internal inconsistency — refuse.
            return Err(MediumError::Chain {
                rule: "begin-authority/begin-not-on-ancestry",
            });
        }
        // (1) Every collected file — the records and their
        // first-reference objects.
        for path in &files {
            let file = File::open(path).map_err(|e| io_err("reaffirm open", path, e))?;
            file.sync_all()
                .map_err(|e| io_err("reaffirm sync", path, e))?;
        }
        // (OC-C3 CP-1, test-only seam) The interruption window:
        // after the file syncs, before the directory/HEAD/root
        // anchoring — an injected failure must never return
        // success.
        self.fault_pre(FaultPoint::ReaffirmFilesSynced)?;
        // (2) The covering objects directory.
        let objects_dir = self.root.join(OBJECTS_DIR);
        let dir = File::open(&objects_dir)
            .map_err(|e| io_err("reaffirm open objects dir", &objects_dir, e))?;
        dir.sync_all()
            .map_err(|e| io_err("reaffirm sync objects dir", &objects_dir, e))?;
        // (3) The HEAD file.
        let head_path = self.root.join(HEAD_FILE);
        let head_file =
            File::open(&head_path).map_err(|e| io_err("reaffirm open HEAD", &head_path, e))?;
        head_file
            .sync_all()
            .map_err(|e| io_err("reaffirm sync HEAD", &head_path, e))?;
        // (4) The root directory.
        let root =
            File::open(&self.root).map_err(|e| io_err("reaffirm open root", &self.root, e))?;
        root.sync_all()
            .map_err(|e| io_err("reaffirm sync root", &self.root, e))?;
        Ok(())
    }

    /// The begin discovery under the held lock: the descent for the
    /// deterministic key, the strict decode, the binding checks.
    pub(super) fn find_installed_begin_locked(
        &self,
        mk: &Hash,
    ) -> Result<Option<(Hash, crate::reclaim::ReclamationBegin)>, MediumError> {
        const MAX_DESCENT: u64 = 1 << 20;
        let key = crate::reclaim::reclamation_begin_idempotency_key(mk);
        let mut current = self
            .read_head_file()?
            .ok_or(MediumError::Chain { rule: "unfounded" })?;
        let mut skip_table: Option<CheckpointMetadata> = None;
        for _ in 0..MAX_DESCENT {
            let bytes = self.load_object(&current)?;
            let record = RevisionRecord::decode(&bytes).map_err(MediumError::Format)?;
            if record.idempotency_key.as_deref() == Some(key.as_slice()) {
                // (F3) The full revision-shape validation — the
                // head-descent (this walk) proves the canonical
                // membership; the shape checks below complete the
                // authority proof (a keyed record is a candidate,
                // never authority by itself).
                if !matches!(record.kind, crate::record::RecordKind::Regular) {
                    return Err(MediumError::Chain {
                        rule: "begin-admission/installed-shape-mismatch",
                    });
                }
                if record.content_refs.len() != 1 {
                    return Err(MediumError::Chain {
                        rule: "begin-admission/installed-shape-mismatch",
                    });
                }
                let Some(parent) = record.parent else {
                    return Err(MediumError::Chain {
                        rule: "begin-admission/installed-shape-mismatch",
                    });
                };
                // (C2-R3) The parent/epoch linkage — the same
                // coherence the normal chain validator enforces: a
                // present parent must carry epoch+1; an absent parent
                // must be an authenticated skip-table gap.
                if self.object_path(&parent).is_file() {
                    let parent_bytes = self.load_object(&parent)?;
                    let parent_record =
                        RevisionRecord::decode(&parent_bytes).map_err(MediumError::Format)?;
                    // (C2-R7) Checked arithmetic: an epoch at u64::MAX
                    // has no valid successor — saturation must never
                    // make an incoherent linkage appear coherent.
                    if parent_record.epoch.checked_add(1) != Some(record.epoch) {
                        return Err(MediumError::Chain {
                            rule: "begin-admission/installed-linkage-mismatch",
                        });
                    }
                } else {
                    let Some(table) = skip_table.as_ref() else {
                        return Err(MediumError::MissingObject { hash: parent });
                    };
                    let covered = if table.sk == parent {
                        table.self_skip().is_some()
                    } else {
                        table.skip_for(&current).is_some()
                    };
                    if !covered {
                        return Err(MediumError::MissingObject { hash: parent });
                    }
                }
                let Some(object) = record.content_refs.first().copied() else {
                    return Err(MediumError::Chain {
                        rule: "begin-admission/installed-malformed",
                    });
                };
                let object_bytes = self.load_object(&object)?;
                let begin = crate::reclaim::ReclamationBegin::decode(&object_bytes)
                    .map_err(MediumError::Format)?;
                if begin.mk != *mk {
                    return Err(MediumError::Chain {
                        rule: "begin-admission/installed-mk-mismatch",
                    });
                }
                return Ok(Some((current, begin)));
            }
            if let Some(object) = record.content_refs.first().copied() {
                let object_bytes = self.load_object(&object)?;
                if StoredObject::new(object_bytes.clone())?.domain()
                    == crate::checkpoint::CHECKPOINT_METADATA_DOMAIN
                {
                    skip_table = Some(
                        CheckpointMetadata::decode(&object_bytes).map_err(MediumError::Format)?,
                    );
                }
            }
            let Some(parent) = record.parent else {
                return Ok(None);
            };
            if !self.object_path(&parent).is_file() {
                let Some(table) = skip_table.as_ref() else {
                    return Err(MediumError::MissingObject { hash: parent });
                };
                let target = if table.sk == parent {
                    table.self_skip().map(|e| e.nearest_retained_ancestor)
                } else {
                    table
                        .skip_for(&current)
                        .map(|e| e.nearest_retained_ancestor)
                };
                let Some(target) = target else {
                    return Err(MediumError::MissingObject { hash: parent });
                };
                current = target;
            } else {
                current = parent;
            }
        }
        Ok(None)
    }

    /// The internal append under the ALREADY-HELD lock: the same
    /// structural admission the public path applies before its own
    /// lock acquisition, then append_locked directly (no nested
    /// coordination).
    fn append_admitted_locked(
        &mut self,
        admitted: crate::medium::AdmittedAppend,
        expected_parent: Option<Hash>,
    ) -> Result<crate::medium::AppendOutcome, MediumError> {
        let (record, objects, composite) = admitted.parts_for_tests();
        let record = record.clone();
        let objects: Vec<StoredObject> = objects.to_vec();
        let record_bytes = record.canonical_bytes();
        let reparsed = RevisionRecord::decode(&record_bytes).map_err(MediumError::Format)?;
        if reparsed != record {
            return Err(MediumError::Chain {
                rule: "record-noncanonical",
            });
        }
        let supplied: BTreeSet<Hash> = objects.iter().map(StoredObject::hash).collect();
        let declared: BTreeSet<Hash> = record.content_refs.iter().copied().collect();
        if !composite && supplied != declared {
            return Err(MediumError::Chain {
                rule: "content-set-mismatch",
            });
        }
        let record_id = crate::hash::hash_object(&record_bytes);
        self.append_locked(
            &record,
            &objects,
            expected_parent,
            &record_bytes,
            &record_id,
            composite,
        )
    }

    /// The W1 quarantine name-family discovery: ONE bounded read_dir
    /// pass collecting every entry in the identity's family (the
    /// plain name, the .reason sidecar, the suffixed incidents and
    /// their sidecars) — the directory listing is the truth; sparse
    /// suffixes cannot be missed.
    pub(super) fn quarantine_family(&self, identity: &Hash) -> Result<Vec<PathBuf>, MediumError> {
        let dir = self.root.join(QUARANTINE_DIR);
        let plain = identity.hex();
        let prefix = format!("{plain}.");
        let mut family = Vec::new();
        let entries = fs::read_dir(&dir).map_err(|e| io_err("read quarantine dir", &dir, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| io_err("read quarantine entry", &dir, e))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_family = name == plain
                || name == format!("{plain}.reason")
                || (name.starts_with(&prefix) && {
                    let rest = &name[plain.len() + 1..];
                    let digits = rest.strip_suffix(".reason").unwrap_or(rest);
                    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
                });
            if is_family {
                family.push(dir.join(name));
            }
        }
        Ok(family)
    }

    /// The trusted two-pass execution under the boundary. Every
    /// authorization refusal occurs BEFORE any mutation; the passes
    /// are idempotent and restart-safe (the resume from the durable
    /// audit: an absent retiring record's manifest reread is skipped
    /// — the audit accounts for it; an absent SURVIVOR record fails
    /// the walk closed).
    pub(super) fn reclaim_authorized_locked(
        &mut self,
        directive: &crate::reclaim::ReclaimDirective,
    ) -> Result<crate::reclaim::ReclaimOutcome, MediumError> {
        // (1-3) The authentication and the directive cross-check.
        let (ledger, metadata, head) = self.authenticate_audit_locked(&directive.audit_record)?;
        if ledger.units != directive.units {
            return Err(MediumError::Chain {
                rule: "reclaim/directive-units-mismatch",
            });
        }
        // (E2) The newest-first retirement order: the persisted unit
        // sequence must be strictly descending by indexed epoch —
        // the provider never trusts the caller's construction.
        crate::reclaim::verify_newest_first_order(&ledger.units, &metadata)?;
        // (4) The manifest reread with the resume tolerance: a
        // PRESENT retiring record's objects must equal its actual
        // content references; an ABSENT one is the protocol-accounted
        // resume — but ONLY in the deletion-prefix shape. The (E1)
        // recovery boundary: this provider's pass 1 deletes
        // newest-first, so a genuine interruption leaves the absent
        // set as a PREFIX of the unit order (an absent unit is never
        // followed by a present one). Any other absence pattern is
        // evidence of missing storage, never proof of a prior
        // authorized deletion — the fabricated-audit-with-missing-
        // data shape fails closed. The prefix-shaped acceptance is
        // the UNRESOLVED recovery-policy question (Option A/B/C —
        // the owner decision is PENDING; no option is ratified): the
        // documented, narrower trust under which the range digest is
        // NOT proven for absent members.
        let mut present_seen = false;
        for unit in &ledger.units {
            let record_path = self.object_path(&unit.record);
            if record_path.is_file() {
                present_seen = true;
                let bytes = self.load_object(&unit.record)?;
                let record = RevisionRecord::decode(&bytes).map_err(MediumError::Format)?;
                if unit.objects != record.content_refs {
                    return Err(MediumError::Chain {
                        rule: "reclaim/units-objects-mismatch",
                    });
                }
            } else if present_seen {
                return Err(MediumError::Chain {
                    rule: "reclaim/absence-not-deletion-prefix",
                });
            }
        }
        // (OC-C3 CP-2) THE BEGIN-BOUND DESTRUCTIVE AUTHORIZATION —
        // every authorization check precedes the FIRST irreversible
        // mutation (the cursor invalidation, PASS 1, PASS 2). The
        // platform tier FIRST: ProcessCrashOnly never grants the
        // destructive grade (regardless of any begin's existence —
        // an installed, restored, or otherwise discoverable begin
        // cannot override the platform limitation). Then the
        // installed-begin authority through the CP-1 lock-held
        // internal helper (the recovery-safe authentication, the
        // founding seed, the canonical verifier, the reaffirmation
        // — NEVER the public wrapper: no lock reacquisition under
        // the already-held HEAD.lock). The DEFINITE canonical
        // absence is the crossing's fresh-admission trigger; every
        // corrupt, mismatched, orphaned, unverifiable, or
        // insufficient-grade state is its own typed refusal — never
        // absence. A resumed retirement is never forced through the
        // OC-C2 first-execution verifier (the recovery-safe path).
        if self.durability_tier() != crate::medium::SyncTier::FullPosixSequence {
            return Err(MediumError::Chain {
                rule: "execution/begin-tier-insufficient",
            });
        }
        match self.verify_begin_authority_locked(&directive.audit_record, &ledger.mk) {
            Ok(_authority) => {}
            Err(MediumError::Chain {
                rule: "begin-authority/absent",
            }) => {
                return Err(MediumError::Chain {
                    rule: "execution/begin-absent",
                });
            }
            Err(other) => return Err(other),
        }
        // (5) The survivor partition under the same boundary — and
        // the E1 range commitments: the founding-derived seed
        // (ALWAYS verifiable — the founding is permanent) and, on
        // the FIRST execution (every range publication present),
        // the recomputed range digest over the canonical bytes (a
        // fabricated-but-matching stored commitment refuses; the
        // interrupted resume with absent publications trusts the
        // durable audit — the ratified recovery contract).
        let (_summary, simulated) =
            self.cold_walk_excluding(head, &ledger.units.iter().map(|unit| unit.record).collect())?;
        crate::reclaim::verify_founding_seed(&metadata, &simulated.founding)?;
        let all_present = ledger
            .units
            .iter()
            .all(|unit| unit.record == ledger.sk || self.object_path(&unit.record).is_file());
        if all_present {
            let mut digest_members: Vec<(u64, Hash)> = ledger
                .units
                .iter()
                .filter(|unit| unit.record != ledger.sk)
                .filter_map(|unit| {
                    metadata
                        .index
                        .iter()
                        .find(|e| e.hash == unit.record)
                        .map(|e| (e.epoch, unit.record))
                })
                .collect();
            digest_members.sort_unstable_by_key(|(epoch, _)| *epoch);
            let mut range_bytes = Vec::with_capacity(digest_members.len());
            for (_, record) in digest_members {
                range_bytes.push(self.load_object(&record)?);
            }
            let recomputed = crate::checkpoint::chain_range_digest(&metadata.seed, &range_bytes);
            if recomputed != ledger.range_digest {
                return Err(MediumError::Chain {
                    rule: "reclaim/range-digest-mismatch",
                });
            }
        }
        let survivors = simulated.reachable;
        let mut partition = crate::reclaim::partition_reclaim(&ledger.units, &survivors)?;
        let mut removed = 0u64;
        // (E4) The cursor lifecycle: once the mutation phase begins,
        // the pre-retirement cursor is NEVER authoritative — the
        // invalidation precedes pass 1 (an interrupted mutation
        // leaves the safe invalidated state; the next operation
        // rebuilds via the authenticated walk).
        self.cursor = None;
        // (6) PASS 1 — the retiring records, newest-first (the
        // ledger's unit order — the crossing's Phase A collection
        // order; never the index's hash order), idempotent. The
        // genuine crash seams (the real-kill evidence): the
        // mid-record-pass window after each record deletion, and the
        // last-record/before-objects window — the two-pass
        // schedule's interruption boundaries.
        for (index, unit) in ledger.units.iter().enumerate() {
            let record_path = self.object_path(&unit.record);
            if record_path.is_file() {
                fs::remove_file(&record_path)
                    .map_err(|e| io_err("reclaim record", &record_path, e))?;
                removed += 1;
            }
            // The two-pass schedule's crash seams (test-only, the
            // real-kill evidence — compile-time isolated under the
            // crash-test-seams feature per the RMD-03 corrective).
            #[cfg(feature = "crash-test-seams")]
            reclaim_mid_unit_seam();
            if index + 1 == ledger.units.len() {
                #[cfg(feature = "crash-test-seams")]
                reclaim_last_unit_seam();
            }
        }
        // (7) PASS 2 — the deduplicated MANDATORY objects only (the
        // partition's output is already deduplicated); the deferred
        // identities are never touched. The W1 discipline on every
        // form: hash-authenticate before deleting; preserve
        // mismatched or unrelated evidence; the sidecars follow
        // their authenticated evidence (an orphaned sidecar is the
        // residue of THIS identity's cleanup).
        for identity in &partition.mandatory {
            let object_path = self.object_path(identity);
            if object_path.is_file() {
                let bytes = fs::read(&object_path)
                    .map_err(|e| io_err("reclaim object read", &object_path, e))?;
                if hash_object(&bytes) == *identity {
                    fs::remove_file(&object_path)
                        .map_err(|e| io_err("reclaim object", &object_path, e))?;
                    removed += 1;
                }
                // A mismatched file at the audited name is preserved
                // evidence — never removed on a filename match.
            }
            for path in self.quarantine_family(identity)? {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                if name.ends_with(".reason") {
                    // A sidecar: the residue rule — it follows its
                    // evidence; an orphan (the evidence absent or
                    // just removed) is THIS cleanup's residue. The
                    // is-file tolerance: the evidence's own
                    // companion removal may have already removed
                    // this sidecar within the same family pass
                    // (idempotent convergence).
                    let evidence = path.with_file_name(name.trim_end_matches(".reason"));
                    if !evidence.is_file() && path.is_file() {
                        fs::remove_file(&path)
                            .map_err(|e| io_err("reclaim sidecar residue", &path, e))?;
                        removed += 1;
                    }
                } else if path.is_file() {
                    // An evidence file: hash-authenticate BEFORE
                    // deleting; a mismatch is preserved suspicious
                    // evidence.
                    let bytes = fs::read(&path)
                        .map_err(|e| io_err("reclaim quarantined read", &path, e))?;
                    if hash_object(&bytes) == *identity {
                        fs::remove_file(&path)
                            .map_err(|e| io_err("reclaim quarantined", &path, e))?;
                        removed += 1;
                        let sidecar = path.with_file_name(format!("{name}.reason"));
                        if sidecar.is_file() {
                            fs::remove_file(&sidecar)
                                .map_err(|e| io_err("reclaim sidecar", &sidecar, e))?;
                            removed += 1;
                        }
                    }
                }
            }
        }
        partition.removed = removed;
        Ok(partition)
    }
}

/// Test-only (the genuine mid-unit reclamation crash seam, the
/// review corrective F4's real-kill evidence): when armed, the
/// reclamation delete hard-exits AFTER a unit's record object is
/// removed and BEFORE its referenced content objects — the exact
/// orphaning window the audited manifest closes on the retry (exit
/// 10, distinct from the after-SK seam's 8). Engine code never arms
/// this; the static is process-local, so only the child that armed
/// it dies. COMPILE-TIME ISOLATED (the RMD-03 corrective, the T2-B
/// pattern): the seam exists ONLY under engine-persist's
/// crash-test-seams feature (forwarded from engine-slice's, which
/// the heavy shard's battery enables) — the production build
/// carries neither the arm nor the process-exit behavior (the CI's
/// feature-disabled artifact check enforces the absence).
#[cfg(feature = "crash-test-seams")]
#[doc(hidden)]
pub static ARM_RECLAIM_MID_UNIT: AtomicBool = AtomicBool::new(false);

/// The mid-unit seam's exit point (see [`ARM_RECLAIM_MID_UNIT`]).
#[cfg(feature = "crash-test-seams")]
fn reclaim_mid_unit_seam() {
    if ARM_RECLAIM_MID_UNIT.load(Ordering::Relaxed) {
        std::process::exit(10);
    }
}

/// Test-only (the genuine last-unit reclamation crash seam, the
/// review corrective R1's real-kill evidence): when armed, the
/// reclamation delete hard-exits AFTER the LAST unit's record object
/// is removed and BEFORE its referenced content objects — every
/// indexed record is then absent while the manifest is NOT complete,
/// the exact misclassification window the whole-manifest completion
/// probe closes (exit 11, distinct from the mid-unit seam's 10 and
/// the after-audit seam's 9). Engine code never arms this; the
/// static is process-local, so only the child that armed it dies.
/// COMPILE-TIME ISOLATED (the RMD-03 corrective, the T2-B pattern):
/// the seam exists ONLY under engine-persist's crash-test-seams
/// feature (forwarded from engine-slice's, which the heavy shard's
/// battery enables) — the production build carries neither the arm
/// nor the process-exit behavior (the CI's feature-disabled
/// artifact check enforces the absence).
#[cfg(feature = "crash-test-seams")]
#[doc(hidden)]
pub static ARM_RECLAIM_LAST_UNIT: AtomicBool = AtomicBool::new(false);

/// The last-unit seam's exit point (see [`ARM_RECLAIM_LAST_UNIT`]).
#[cfg(feature = "crash-test-seams")]
fn reclaim_last_unit_seam() {
    if ARM_RECLAIM_LAST_UNIT.load(Ordering::Relaxed) {
        std::process::exit(11);
    }
}
