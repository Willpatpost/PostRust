//! `LocalFileMedium` — the §5.3 nine-step conditional-append protocol
//! on a local directory, with the §5.3.2 trusted-cursor validation
//! model and the §5.3.3 v1.4 orphan protocol.
//!
//! **Layout** (the documented campaign directory):
//!
//! ```text
//! <root>/HEAD        the authoritative head pointer (head/1)
//! <root>/HEAD.lock   the persistent OS-managed coordination file —
//!                    created once by the medium, locked per operation,
//!                    NEVER unlinked or recreated during ordinary
//!                    release; process death releases ownership
//! <root>/HEAD.tmp    transient publication scratch (only meaningful
//!                    under the lock; overwritten by each publish)
//! <root>/objects/    immutable content-addressed objects (<hex>)
//! <root>/quarantine/ evidence preserved, never deleted
//! ```
//!
//! **The nine steps** (append): acquire `HEAD.lock` → validate the
//! observed HEAD (trusted cursor: O(1) on the verified head, suffix
//! walk on divergence, cold walk otherwise) → compare with the
//! caller's expected parent → reject stale writers without altering
//! HEAD → write every object the candidate needs (insert-once,
//! verify-on-retry, quarantine-then-rewrite on an unreferenced
//! mismatch, fail-closed on a referenced mismatch) with file fsyncs
//! and the Unix objects-directory fsync → publish HEAD via
//! `HEAD.tmp` + fsync + rename (the protocol's **linearization
//! point**) → post-rename root-directory fsync (Unix; the documented
//! Windows sequence offers no directory flush — the tier carries the
//! limitation) → release the lock after a defined outcome → reconcile
//! uncertainty on demand. **Every I/O failure from the first object
//! write onward is the `Unknown` outcome** — never `Accepted`, never
//! a clean retryable error hiding an uncertain commit.
//!
//! **Validation (§5.3.2):** session open performs the full founding →
//! HEAD walk (every record and every referenced object hash-verified)
//! and an unreferenced-object sweep that quarantines under the lock.
//! Within the session, an append to the already-verified head does
//! O(1) validation work (no records walked — the deterministic
//! counter in [`WalkStats`] is asserted by the tests); external
//! divergence suffix-walks only the new records; integrity failure or
//! a non-descendant observed head fails closed (divergent histories
//! are detected, never merged). The cursor is session-scoped in-memory
//! derived state — **no persistent trusted-cache file**. Its
//! `seen_keys`/`reachable` indexes are O(chain) memory, documented
//! honestly; the O(1) claim is about per-append walk work.
//!
//! **Orphans (§5.3.3 v1.4):** a referenced object failing its hash is
//! fail-closed evidence (never replaced, never deleted); an
//! unreferenced questionable object is quarantined with its bytes and
//! a reason file — never auto-deleted; permanent reclamation is not
//! authorized in this batch. `HEAD.tmp` is protocol scratch, not
//! evidence: the rename's atomicity means HEAD already reflects the
//! outcome, so a stale `HEAD.tmp` is simply overwritten by the next
//! publish.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use engine_core::Decoder;
use engine_core::wire::MAX_BYTE_LEN;

use crate::fault::{FaultControl, FaultPoint};
use crate::hash::{Hash, hash_object};
use crate::medium::{
    AppendOutcome, CanonicalMedium, ChainSummary, MediumError, Reconciliation, StoredObject,
    SyncTier, UnknownReason,
};

mod head_protocol;
mod layout;
mod orphan;
mod reclaim_exec;
mod timings;
mod walks;

use timings::elapsed_nanos;
pub use timings::{AppendTimings, WalkStats};

/// The HEAD pointer's domain (a persisted wire format; visible to
/// in-crate tests that hand-build stores).
pub(crate) const HEAD_DOMAIN: &str = "sektor7/engine-persist/head/1";
pub(crate) const HEAD_VERSION: u32 = 1;

/// The on-disk layout (documented in the module docs; visible to
/// in-crate tests).
pub(crate) const OBJECTS_DIR: &str = "objects";
pub(crate) const QUARANTINE_DIR: &str = "quarantine";
pub(crate) const HEAD_FILE: &str = "HEAD";
pub(crate) const HEAD_LOCK: &str = "HEAD.lock";
pub(crate) const HEAD_TMP: &str = "HEAD.tmp";

/// The §6.1 offline migration's seal marker (PP-B1): its presence
/// means the directory was migrated to a SQLite campaign database
/// and is archived evidence — new binaries refuse to open it as a
/// source (binaries predating the tombstone cannot be made to honor
/// it; the ratified documented operational limitation).
pub(crate) const MIGRATED_MARKER: &str = "MIGRATED";

/// Documented maximum for a stored object's size (matches the Batch 1
/// byte-length maximum; enforced by stat before any read allocates).
pub(crate) const MAX_OBJECT_BYTES: u64 = MAX_BYTE_LEN;

/// The ephemeral trusted cursor (§5.3.2) — session-scoped derived
/// state rebuilt by the cold walk; never persisted to a file.
/// `Debug` is derived for the P3-L9b differential battery's cursor
/// fingerprint (the field names, types, and order are the §5.3.2
/// contract, identical across providers — so the derived output is
/// the byte-identical comparison form).
#[derive(Debug)]
struct Cursor {
    head: Hash,
    epoch: u64,
    /// Idempotency keys seen across the chain (session index for O(1)
    /// duplicate checks at append; O(chain) memory, documented).
    seen_keys: BTreeSet<Vec<u8>>,
    /// Every record id and content hash reachable from HEAD.
    reachable: BTreeSet<Hash>,
    /// The chain's revision ids alone (dependency resolution requires
    /// a *prior revision*, never a content hash that happens to be
    /// reachable).
    record_ids: BTreeSet<Hash>,
    /// The reclaim-indexed revisions (identity → epoch) from every
    /// on-chain MK's index whose records are genuinely absent — the
    /// historical membership evidence (PP-B3's condition 2: a
    /// reclaimed revision's key-dedup and depends_on resolution
    /// survive its deletion). The observational counts (the MK list,
    /// the reclaimed totals) live on the [`ChainSummary`] the walk
    /// returns, never on the cursor.
    reclaimed: BTreeMap<Hash, u64>,
    founding: Hash,
    authority: AuthorityMode,
    campaign_id: String,
    content_objects: u64,
}

/// The local file-backed canonical medium.
pub struct LocalFileMedium {
    root: PathBuf,
    lock_file: File,
    cursor: Option<Cursor>,
    fault: Option<FaultControl>,
    timings: AppendTimings,
    walks: WalkStats,
    platform_tier: SyncTier,
}

impl LocalFileMedium {
    /// A pre-window fault check — an injected failure is a clean,
    /// retryable error (nothing has been written).
    fn fault_pre(&self, point: FaultPoint) -> Result<(), MediumError> {
        match &self.fault {
            Some(control) => control.check(point),
            None => Ok(()),
        }
    }

    /// An in-window fault check — an injected failure becomes the
    /// `Unknown` outcome for the phase's reason.
    fn fault_window(&self, point: FaultPoint, reason: UnknownReason) -> Result<(), UnknownReason> {
        match &self.fault {
            Some(control) => control.check(point).map_err(|_| reason),
            None => Ok(()),
        }
    }
}

impl CanonicalMedium for LocalFileMedium {
    fn begin_reclamation_v2(
        &mut self,
        mk: &Hash,
        tick: u64,
    ) -> Result<crate::reclaim::BeginPublicationOutcome, MediumError> {
        // (V2X-CP2) The plan-bound V2 begin admission under the
        // exclusive HEAD.lock (the C2-R7 outcome-preservation
        // discipline on unlock) — the ALL-AUDITS-FIRST CP1 contract
        // re-derived UNDER the lock through the internal reads.
        self.lock()?;
        let outcome = self.begin_reclamation_v2_locked(mk, tick);
        let _ = self.unlock();
        outcome
    }

    fn verify_begin_authority_v2(
        &mut self,
        mk: &Hash,
    ) -> Result<crate::reclaim::BeginAuthority, MediumError> {
        // (V2X-CP2) The retained-evidence V2 begin authority under
        // the exclusive HEAD.lock (the C2-R7 outcome-preservation
        // discipline on unlock).
        self.lock()?;
        let result = self.verify_begin_authority_v2_locked(mk);
        let _ = self.unlock();
        result
    }

    fn observe_range_terminal_under_coordination(
        &mut self,
        expected: &crate::range_completion::RangeCompletionContext,
        limits: &crate::range_installation::RangeInstallationLimits,
    ) -> Result<crate::range_installation::CoordinatedObservation, MediumError> {
        // (V2X-CP3-B-P2A-C5b) The provider-coordinated observation
        // under the exclusive HEAD.lock: the head read AND the whole
        // walk inside one lock scope (the capture-then-unlock
        // discipline — an error path never leaks the OS lock).
        self.lock()?;
        let result = self.observe_range_terminal_under_coordination_locked(expected, limits);
        let _ = self.unlock();
        result
    }

    fn head(&mut self) -> Result<Option<Hash>, MediumError> {
        self.lock()?;
        // Capture-then-unlock (the reviewed open() discipline): no
        // early return between lock and unlock — an error path must
        // never leak the OS lock while the medium stays alive.
        let outcome = self
            .read_head_file()
            .and_then(|observed| self.validate_observed(observed).map(|()| observed));
        let _ = self.unlock();
        outcome
    }

    fn append_admitted(
        &mut self,
        admitted: crate::medium::AdmittedAppend,
        expected_parent: Option<Hash>,
    ) -> Result<AppendOutcome, MediumError> {
        let encode_started = Instant::now();
        let (record, objects, composite) = admitted.parts();
        // The set rule BY KIND: the generic form keeps the exact
        // supplied==referenced equality (unchanged); the composite
        // form carries the ROOT-ONLY record with the
        // admission-verified extras (the root plus the new chunks —
        // the record's single reference IS the root, and every
        // extra was verified against the root manifest by the
        // in-crate admission that built the receipt).
        let record = record.clone();
        let objects: Vec<StoredObject> = objects.to_vec();
        if composite
            && (record.content_refs.len() != 1
                || objects.is_empty()
                || objects[0].hash() != record.content_refs[0])
        {
            return Err(MediumError::Chain {
                rule: "composite-receipt-shape",
            });
        }
        // Structural validation (pre-window; a failure is a clean
        // error — nothing has been written): the record must be
        // canonically representable and reference exactly the supplied
        // object set.
        let record_bytes = record.canonical_bytes();
        let reparsed = RevisionRecord::decode(&record_bytes).map_err(MediumError::Format)?;
        if reparsed != record {
            return Err(MediumError::Chain {
                rule: "record-noncanonical",
            });
        }
        let record_id = hash_object(&record_bytes);
        let supplied: BTreeSet<Hash> = objects.iter().map(StoredObject::hash).collect();
        let declared: BTreeSet<Hash> = record.content_refs.iter().copied().collect();
        if !composite && supplied != declared {
            return Err(MediumError::Chain {
                rule: "content-set-mismatch",
            });
        }
        if composite && !supplied.is_superset(&declared) {
            return Err(MediumError::Chain {
                rule: "composite-content-missing",
            });
        }
        self.timings.encode_nanos = elapsed_nanos(encode_started);

        // Step 1: acquire the OS-managed coordination.
        self.fault_pre(FaultPoint::BeforeLock)?;
        self.lock()?;
        let outcome = self.append_locked(
            &record,
            &objects,
            expected_parent,
            &record_bytes,
            &record_id,
            composite,
        );
        // Step 8: release after the defined outcome. An unlock failure
        // does not alter a reached outcome — the OS releases ownership
        // at handle close or process death either way.
        let _ = self.unlock();
        outcome
    }

    fn read_record(&mut self, id: &Hash) -> Result<RevisionRecord, MediumError> {
        self.lock()?;
        let decoded = self
            .load_object(id)
            .and_then(|bytes| RevisionRecord::decode(&bytes).map_err(MediumError::Format));
        let _ = self.unlock();
        decoded
    }

    fn read_object(&mut self, hash: &Hash) -> Result<Vec<u8>, MediumError> {
        self.lock()?;
        let bytes = self.load_object(hash);
        let _ = self.unlock();
        bytes
    }

    fn validate(&mut self) -> Result<ChainSummary, MediumError> {
        self.lock()?;
        // Capture-then-unlock (the reviewed open() discipline): no
        // early return between lock and unlock — an error path must
        // never leak the OS lock while the medium stays alive.
        let result = match self.read_head_file() {
            Ok(Some(id)) => {
                self.cursor = None;
                self.cold_walk_from(id)
            }
            Ok(None) => Err(MediumError::Chain { rule: "unfounded" }),
            Err(error) => Err(error),
        };
        let _ = self.unlock();
        result
    }

    fn reconcile_unknown(&mut self, candidate: &Hash) -> Result<Reconciliation, MediumError> {
        let started = Instant::now();
        self.lock()?;
        let result = self.reconcile_locked(candidate);
        let _ = self.unlock();
        self.timings.reconcile_nanos = elapsed_nanos(started);
        result
    }

    fn durability_tier(&self) -> SyncTier {
        self.platform_tier
    }

    fn reclaim(
        &mut self,
        directive: &crate::reclaim::ReclaimDirective,
    ) -> Result<crate::reclaim::ReclaimOutcome, MediumError> {
        // THE TRUSTED BOUNDARY (K2-C2B P2): the exclusive HEAD.lock
        // covers the authentication, the partition, and the
        // deletion — no authoritative state can change between the
        // decision and its destructive consequences. The
        // capture-then-unlock discipline: no early return leaks the
        // lock.
        self.lock()?;
        let outcome = self.reclaim_authorized_locked(directive);
        let _ = self.unlock();
        outcome
    }

    fn begin_reclamation(
        &mut self,
        audit: &Hash,
        mk: &Hash,
        tick: u64,
    ) -> Result<crate::reclaim::BeginPublicationOutcome, MediumError> {
        // (OC-C3 CP-1) The routing delegation to the accepted OC-C2
        // admission — UNCHANGED semantics, no rewiring of any
        // destructive or completion boundary.
        LocalFileMedium::begin_reclamation(self, audit, mk, tick)
    }

    fn verify_begin_authority(
        &mut self,
        audit: &Hash,
        mk: &Hash,
    ) -> Result<crate::reclaim::BeginAuthority, MediumError> {
        // (OC-C3 CP-1) The installed-begin durability authority
        // under the exclusive HEAD.lock (the C2-R7
        // outcome-preservation discipline on unlock).
        self.lock()?;
        let result = self.verify_begin_authority_locked(audit, mk);
        let _ = self.unlock();
        result
    }

    fn reclaim_pending(
        &mut self,
        directive: &crate::reclaim::ReclaimDirective,
    ) -> Result<bool, MediumError> {
        // The Mandatory-only probe: the same authentication and
        // partition (lock-free — the advisory read), the mandatory
        // identities across their authenticated storage forms.
        let (ledger, _metadata, head) = self.authenticate_audit_locked(&directive.audit_record)?;
        if ledger.units != directive.units {
            return Err(MediumError::Chain {
                rule: "reclaim/directive-units-mismatch",
            });
        }
        let survivors = self.survivor_set_locked(head, &ledger)?;
        let partition = crate::reclaim::partition_reclaim(&ledger.units, &survivors)?;
        for identity in &partition.mandatory {
            // (E5) The normal-path probe matches the deletion
            // discipline: hash-verify the bytes — a mismatched file
            // at an audited name is PRESERVED evidence, never
            // authenticated unfinished retirement (never pending).
            // The read itself is CLASSIFIED, never swallowed: a
            // genuinely absent path is the absence; a failed read
            // (permissions, transient I/O, an inaccessible path) is
            // a typed refusal — an unverifiable observation never
            // silently contributes to a completed result.
            match fs::read(self.object_path(identity)) {
                Ok(bytes) => {
                    if hash_object(&bytes) == *identity {
                        return Ok(true);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(io_err("probe object", &self.object_path(identity), e));
                }
            }
            for path in self.quarantine_family(identity)? {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                if name.ends_with(".reason") {
                    continue;
                }
                if path.is_file() {
                    let bytes = fs::read(&path)
                        .map_err(|e| io_err("probe quarantined evidence", &path, e))?;
                    if hash_object(&bytes) == *identity {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }
}

/// Reads and decodes the authoritative HEAD file at a store root —
/// the raw, cursor-free read (the offline migration's cutover
/// recheck under its own held lock, PP-B1 §6.1).
pub(crate) fn read_head_at(root: &Path) -> Result<Option<Hash>, MediumError> {
    let path = root.join(HEAD_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_err("read HEAD", &path, e)),
    };
    let corrupt = |error: engine_core::SaveError| MediumError::CorruptHead {
        detail: error.to_string(),
    };
    let mut dec = Decoder::new(&bytes);
    dec.read_domain(HEAD_DOMAIN).map_err(corrupt)?;
    let version = dec.read_u32().map_err(corrupt)?;
    if version != HEAD_VERSION {
        return Err(MediumError::CorruptHead {
            detail: format!("unsupported HEAD version {version}"),
        });
    }
    let hash_bytes = dec.read_bytes(Hash::WIDTH, "head").map_err(corrupt)?;
    let id = Hash::from_bytes(hash_bytes).ok_or_else(|| MediumError::CorruptHead {
        detail: "hash width".into(),
    })?;
    dec.finish().map_err(corrupt)?;
    Ok(Some(id))
}

fn io_err(op: &'static str, path: &Path, source: io::Error) -> MediumError {
    MediumError::Io {
        op,
        path: path.display().to_string(),
        source,
    }
}

#[cfg(test)]
mod c5b_local_tests;
#[cfg(test)]
mod c5d_local_tests;
#[cfg(test)]
mod exclusion_tests;

/// (P3-L9b) The walk differential battery's observation seams —
/// test-only, `pub(crate)` so the crate-root battery
/// (`walk_differential.rs`) can observe this provider's CURRENT walk
/// forms without widening any production surface: the pure
/// virtual-exclusion cold walk (its summary plus the established
/// cursor's Debug fingerprint) and the live trusted cursor's
/// fingerprint. The fingerprints are comparable across providers
/// because the cursor field sets are the identical §5.3.2 contract.
#[cfg(test)]
impl LocalFileMedium {
    pub(crate) fn walk_differential_cold(
        &self,
        head: Hash,
        exclude: &BTreeSet<Hash>,
    ) -> Result<(ChainSummary, String), MediumError> {
        self.cold_walk_excluding(head, exclude)
            .map(|(summary, cursor)| (summary, format!("{cursor:?}")))
    }

    pub(crate) fn walk_differential_cursor(&self) -> Option<String> {
        self.cursor.as_ref().map(|cursor| format!("{cursor:?}"))
    }
}
use crate::record::{AuthorityMode, RevisionRecord};
