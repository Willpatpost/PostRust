//! The reader-generic evidence discovery and verification (split
//! from the reclaim monolith by change reason, P3-L9 pure move): the
//! BeginReads seam, the chain-plan discovery, the installed-begin
//! and execution-readiness verifiers, the V1 audit authentication,
//! and the completion verification. These change when the
//! verify-before-reclaim evidence rules change.

use super::execution::{founding_of, verify_completion_begin_authority};
/// (OC-C3 CP-1) The reader abstraction for the shared installed-
/// begin verifier: ONE mutable reader object serving both the
/// record and the object reads, so a single coordination-scoped
/// borrow — a provider transaction, a lock-held medium, or a
/// generic trait object — backs the whole walk without conflicting
use super::*;
/// Rust borrows. Every implementation MUST hash-verify: the bytes
/// returned for an identity are the stored bytes AT that identity.
pub trait BeginReads {
    /// Reads and hash-verifies a revision record's canonical bytes.
    fn read_record_bytes(&mut self, id: &Hash) -> Result<Vec<u8>, MediumError>;
    /// Reads and hash-verifies a content object's bytes.
    fn read_object_bytes(&mut self, hash: &Hash) -> Result<Vec<u8>, MediumError>;
}

/// (OC-C3 CP-1) The generic-medium adapter: ONE
/// `&mut dyn CanonicalMedium` backing both read kinds through the
/// trait's hash-verifying reads — the completion-admission path's
/// borrowing model (no double capture; the adapter's `&mut self`
/// methods reborrow the single mutable reference per call).
pub struct MediumReads<'a> {
    medium: &'a mut dyn crate::medium::CanonicalMedium,
}

impl<'a> MediumReads<'a> {
    /// The single-mutable-borrow constructor.
    pub fn new(medium: &'a mut dyn crate::medium::CanonicalMedium) -> Self {
        MediumReads { medium }
    }
}

impl BeginReads for MediumReads<'_> {
    fn read_record_bytes(&mut self, id: &Hash) -> Result<Vec<u8>, MediumError> {
        Ok(self.medium.read_record(id)?.canonical_bytes())
    }
    fn read_object_bytes(&mut self, hash: &Hash) -> Result<Vec<u8>, MediumError> {
        self.medium.read_object(hash)
    }
}

/// (V2X-CP2) Decodes a strictly framed revision record from its
/// hash-verified canonical bytes — the reader-generic walks'
/// equivalent of the medium trait's decoded `read_record`.
pub(crate) fn decode_record(bytes: &[u8]) -> Result<RevisionRecord, MediumError> {
    RevisionRecord::decode(bytes).map_err(MediumError::Format)
}

/// (V2X-CP2) The reader-generic decoded record read: the
/// hash-verified canonical bytes through any [`BeginReads`], then
/// the strict decode.
pub(crate) fn read_record_via<R: BeginReads>(
    reads: &mut R,
    id: &Hash,
) -> Result<RevisionRecord, MediumError> {
    decode_record(&reads.read_record_bytes(id)?)
}

/// (V2X-CP2) The reader-generic founding identity: the accepted
/// closure-parameterized [`founding_identity`] descent over ONE
/// reader (the RefCell reborrow pattern — the publication's
/// precedent), with the typed presence probe expressed as the
/// reader's own MissingObject classification (an absent record is
/// the gap signal; every other error propagates).
pub(crate) fn founding_identity_via<R: BeginReads>(
    reads: &mut R,
    head: Hash,
) -> Result<Hash, MediumError> {
    let cell = std::cell::RefCell::new(reads);
    founding_identity(
        head,
        |id| cell.borrow_mut().read_record_bytes(id),
        |id| cell.borrow_mut().read_object_bytes(id),
        |id| match cell.borrow_mut().read_record_bytes(id) {
            Ok(_) => Ok(true),
            Err(MediumError::MissingObject { .. }) => Ok(false),
            Err(other) => Err(other),
        },
    )
}

/// (OC-C3 CP-1) The shared, recovery-safe canonical installed-begin
/// verifier — ONE implementation of the discovery discipline,
/// callable under provider coordination (the provider adapters over
/// a transaction or a held lock) or through the generic medium
/// abstraction. The walk proves, from the supplied HEAD down the
/// canonical ancestry (crossing legitimate reclaimed gaps ONLY
/// under authenticated checkpoint skip tables):
///
/// - the deterministic MK-bound begin key at the discovered record
///   (key-index occupancy alone is never authority — the record is
///   found BY the ancestry walk, never the reverse);
/// - the regular-revision shape (exactly one content reference, a
///   parent);
/// - the CHECKED parent/epoch linkage (a present parent carries
///   epoch+1 — `u64::MAX` has no coherent successor; an absent
///   parent only under authenticated skip coverage);
/// - the strict 206-byte begin decode of the single content object;
/// - the four payload bindings against the independently
///   authenticated retained evidence — the exact MK, the exact
///   source-audit identity, the ledger's range digest, and the
///   ledger's founding seed;
/// - the canonical position: the source audit appears on the walked
///   ancestry BELOW the begin (the admission appends above the
///   audit; a begin below its claimed audit, or a walk reaching the
///   founding without it, is noncanonical and refuses).
///
/// Every missing, corrupt, orphaned, mismatched, or unverifiable
/// input fails closed with a typed error. Recovery-safe: no deleted
/// original retirement record is ever required (the binding inputs
/// are the retained audit, checkpoint metadata, and the begin's own
/// payload). Returns `Ok(None)` when the walk reaches the founding
/// without any same-key begin — a DEFINITE canonical absence,
/// distinct from every failure.
pub fn verify_installed_begin<R: BeginReads>(
    reads: &mut R,
    head: Hash,
    audit: &Hash,
    mk: &Hash,
    ledger: &ReclamationLedger,
) -> Result<Option<(Hash, ReclamationBegin)>, MediumError> {
    const MAX_DESCENT: u64 = 1 << 20;
    let key = reclamation_begin_idempotency_key(mk);
    let mut current = head;
    let mut seen = std::collections::BTreeSet::new();
    let mut skip_table: Option<crate::checkpoint::CheckpointMetadata> = None;
    let mut begin_found: Option<(Hash, ReclamationBegin)> = None;
    for _ in 0..MAX_DESCENT {
        if !seen.insert(current) {
            return Err(MediumError::Chain { rule: "cycle" });
        }
        let bytes = reads.read_record_bytes(&current)?;
        let record = RevisionRecord::decode(&bytes).map_err(MediumError::Format)?;
        if let Some(object) = record.content_refs.first().copied() {
            let object_bytes = reads.read_object_bytes(&object)?;
            if StoredObject::new(object_bytes.clone())?.domain()
                == crate::checkpoint::CHECKPOINT_METADATA_DOMAIN
            {
                skip_table = Some(
                    crate::checkpoint::CheckpointMetadata::decode(&object_bytes)
                        .map_err(MediumError::Format)?,
                );
            }
        }
        if begin_found.is_none() && record.idempotency_key.as_deref() == Some(key.as_slice()) {
            let Some(parent) = record.parent else {
                return Err(MediumError::Chain {
                    rule: "begin-authority/installed-shape-mismatch",
                });
            };
            if !matches!(record.kind, crate::record::RecordKind::Regular)
                || record.content_refs.len() != 1
            {
                return Err(MediumError::Chain {
                    rule: "begin-authority/installed-shape-mismatch",
                });
            }
            // The checked parent/epoch linkage (the C2-R7
            // arithmetic): a present parent carries epoch+1; an
            // absent parent only under authenticated skip coverage.
            match reads.read_record_bytes(&parent) {
                Ok(parent_bytes) => {
                    let parent_record =
                        RevisionRecord::decode(&parent_bytes).map_err(MediumError::Format)?;
                    if parent_record.epoch.checked_add(1) != Some(record.epoch) {
                        return Err(MediumError::Chain {
                            rule: "begin-authority/installed-linkage-mismatch",
                        });
                    }
                }
                Err(MediumError::MissingObject { hash: parent }) => {
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
                Err(other) => return Err(other),
            }
            let object = record.content_refs[0];
            let object_bytes = reads.read_object_bytes(&object)?;
            let begin = ReclamationBegin::decode(&object_bytes).map_err(MediumError::Format)?;
            if begin.mk != *mk
                || begin.source != *audit
                || begin.range_digest != ledger.range_digest
                || begin.seed != ledger.seed
            {
                return Err(MediumError::Chain {
                    rule: "begin-authority/binding-mismatch",
                });
            }
            begin_found = Some((current, begin));
        }
        if begin_found.is_some() && current == *audit {
            // The source audit is on the walked ancestry below the
            // begin — the canonical position holds.
            return Ok(begin_found);
        }
        let Some(parent) = record.parent else {
            if begin_found.is_some() {
                // The begin exists but its claimed source audit is
                // NOT below it on the canonical ancestry.
                return Err(MediumError::Chain {
                    rule: "begin-authority/audit-not-below-begin",
                });
            }
            return Ok(None);
        };
        match reads.read_record_bytes(&parent) {
            Ok(_) => current = parent,
            Err(MediumError::MissingObject { hash: parent }) => {
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
            Err(other) => return Err(other),
        }
    }
    Err(MediumError::Chain {
        rule: "begin-authority/descent-bound",
    })
}

/// (V2X-CP2) The shared, recovery-safe canonical installed V2
/// plan-bound begin verifier — ONE implementation of the discovery
/// discipline, callable under provider coordination or through the
/// generic medium abstraction (the [`verify_installed_begin`]
/// discipline generalized to the V2 source form). The walk proves,
/// from the supplied HEAD down the canonical ancestry (crossing
/// legitimate reclaimed gaps ONLY under authenticated checkpoint
/// skip tables — RECOVERY-SAFE: no original retiring record is ever
/// required; the retained plan/audit/begin evidence and the
/// authenticated skip tables are the authorities):
///
/// - the deterministic MK-bound begin key at the discovered record
///   (key-index occupancy alone is never authority — the record is
///   found BY the ancestry walk, never the reverse);
/// - the regular-revision shape (exactly one content reference, a
///   parent) and the CHECKED parent/epoch linkage (a present parent
///   carries epoch+1; an absent parent only under authenticated
///   skip coverage);
/// - the strict 206-byte begin decode of the single content object;
/// - THE FOUR-FIELD V2 BINDING against the independently
///   authenticated retained evidence: the exact MK, the source ==
///   the installed PLAN REVISION-RECORD identity (never the plan
///   object's content hash, never an individual audit — the
///   ratified V2 source semantics; a V1-bound begin fails this
///   binding by construction), the plan's range digest, and the
///   plan's founding seed;
/// - THE CANONICAL TEMPORAL POSITION (the strict boundary): the
///   begin's discovered location is a STRICT temporal boundary —
///   the plan revision and every plan-defined audit key count
///   toward certification ONLY when encountered strictly BELOW
///   that boundary during the HEAD-to-founding descent; a sighting
///   ABOVE the begin is recorded as the boundary violation and
///   NEVER reused as below-begin proof (the walk descends, so
///   sightings preceding the begin's encounter are temporally
///   AFTER it — above it on the ancestry);
/// - each below-begin audit's object strictly decoded with the
///   field agreement and the ordered unit sequence against the
///   plan's own batch records (the immutable partition), its
///   encoded bytes within the plan's admitted ledger byte limit,
///   and its `reclaimed_keys` count recomputed from the retained
///   checkpoint index's key-presence information (the plan-key
///   record's identity exactly the claimed plan record);
/// - every failure typed and fail-closed; `Ok(None)` only when no
///   begin exists under the key at all (the definitive absence).
pub fn verify_installed_begin_v2<R: BeginReads>(
    reads: &mut R,
    head: Hash,
    mk: &Hash,
    plan_record: &Hash,
    plan: &ReclaimPlan,
    metadata: &crate::checkpoint::CheckpointMetadata,
) -> Result<Option<V2InstalledBegin>, MediumError> {
    const MAX_DESCENT: u64 = 1 << 20;
    let begin_key = reclamation_begin_idempotency_key(mk);
    let plan_key = reclamation_plan_idempotency_key(mk);
    let mut expected_audit_keys: std::collections::BTreeMap<Vec<u8>, u64> =
        std::collections::BTreeMap::new();
    for batch in &plan.batches {
        expected_audit_keys.insert(
            reclamation_ledger_v2_idempotency_key(mk, batch.index),
            batch.index,
        );
    }
    let mut current = head;
    let mut seen = std::collections::BTreeSet::new();
    let mut skip_table: Option<crate::checkpoint::CheckpointMetadata> = None;
    let mut installed: Option<(Hash, ReclamationBegin)> = None;
    let mut found_plan: Option<Hash> = None;
    let mut found_audits: std::collections::BTreeMap<u64, Hash> = std::collections::BTreeMap::new();
    // (V2X-CP2 F1) The strict-boundary state: targets collected
    // ONLY below the begin; a required target sighted ABOVE the
    // begin (before its encounter in the descending walk) is the
    // recorded boundary violation — never below-begin proof.
    let mut target_above_begin = false;
    for _ in 0..MAX_DESCENT {
        if !seen.insert(current) {
            return Err(MediumError::Chain { rule: "cycle" });
        }
        let record = read_record_via(reads, &current)?;
        let mut first_object: Option<Vec<u8>> = None;
        if let Some(object) = record.content_refs.first().copied() {
            let bytes = reads.read_object_bytes(&object)?;
            if StoredObject::new(bytes.clone())?.domain()
                == crate::checkpoint::CHECKPOINT_METADATA_DOMAIN
            {
                skip_table = Some(
                    crate::checkpoint::CheckpointMetadata::decode(&bytes)
                        .map_err(MediumError::Format)?,
                );
            }
            first_object = Some(bytes);
        }
        // The begin discovery (only ABOVE the targets — once the
        // begin is found, later same-key records are impossible by
        // chain-key uniqueness; the first encounter is the one).
        if installed.is_none() && record.idempotency_key.as_deref() == Some(begin_key.as_slice()) {
            let Some(parent) = record.parent else {
                return Err(MediumError::Chain {
                    rule: "begin-authority-v2/installed-shape-mismatch",
                });
            };
            if !matches!(record.kind, crate::record::RecordKind::Regular)
                || record.content_refs.len() != 1
            {
                return Err(MediumError::Chain {
                    rule: "begin-authority-v2/installed-shape-mismatch",
                });
            }
            match reads.read_record_bytes(&parent) {
                Ok(parent_bytes) => {
                    let parent_record = decode_record(&parent_bytes)?;
                    if parent_record.epoch.checked_add(1) != Some(record.epoch) {
                        return Err(MediumError::Chain {
                            rule: "begin-authority-v2/installed-linkage-mismatch",
                        });
                    }
                }
                Err(MediumError::MissingObject { hash: parent }) => {
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
                Err(other) => return Err(other),
            }
            let object = record.content_refs[0];
            let object_bytes = reads.read_object_bytes(&object)?;
            let begin = ReclamationBegin::decode(&object_bytes).map_err(MediumError::Format)?;
            if begin.mk != *mk
                || begin.source != *plan_record
                || begin.range_digest != plan.range_digest
                || begin.seed != plan.seed
            {
                return Err(MediumError::Chain {
                    rule: "begin-authority-v2/binding-mismatch",
                });
            }
            installed = Some((current, begin));
        }
        // The target collection (the plan revision and every audit
        // key) — STRICTLY BELOW the begin only (V2X-CP2 F1): a
        // sighting while the begin is not yet encountered is ABOVE
        // the begin on the ancestry (the walk descends from HEAD),
        // recorded as the boundary violation and never counted.
        if record.idempotency_key.as_deref() == Some(plan_key.as_slice()) {
            if installed.is_some() {
                if found_plan.is_some() && found_plan != Some(current) {
                    return Err(MediumError::Chain { rule: "cycle" });
                }
                found_plan = Some(current);
            } else {
                target_above_begin = true;
            }
        }
        if let Some(key) = record.idempotency_key.clone()
            && let Some(batch_index) = expected_audit_keys.get(&key).copied()
        {
            if !matches!(record.kind, crate::record::RecordKind::Regular)
                || record.content_refs.len() != 1
            {
                return Err(MediumError::Chain {
                    rule: "begin-authority-v2/audit-shape-mismatch",
                });
            }
            let object_hash = record.content_refs[0];
            let bytes = first_object
                .clone()
                .expect("the single content ref was read above");
            let ledger = ReclamationLedgerV2::decode(&bytes).map_err(MediumError::Format)?;
            let batch = &plan.batches[batch_index as usize];
            if ledger.mk != *mk
                || ledger.sk != plan.sk
                || ledger.batch != batch.index
                || ledger.range_digest != plan.range_digest
                || ledger.seed != plan.seed
            {
                return Err(MediumError::Chain {
                    rule: "begin-authority-v2/audit-binding-mismatch",
                });
            }
            let unit_records: Vec<Hash> = ledger.units.iter().map(|unit| unit.record).collect();
            if unit_records != batch.records {
                return Err(MediumError::Chain {
                    rule: "begin-authority-v2/audit-order-mismatch",
                });
            }
            // (V2X-CP2 F2) The audit's encoded byte limit against
            // the plan's admitted bound — the same immutable
            // invariant the CP1 reader enforces on the audit set.
            if bytes.len() as u64 > plan.ledger_byte_limit {
                return Err(MediumError::Chain {
                    rule: "begin-authority-v2/audit-byte-limit",
                });
            }
            // (V2X-CP2 F2) The retained key-count recomputation:
            // the audit's `reclaimed_keys` count recomputed from
            // the retained checkpoint index's key-presence
            // information (the index is permanent evidence — the
            // check needs no original retiring record). Every unit
            // is an indexed member (the membership proof precedes
            // this walk); a lookup miss is fail-closed.
            let mut recomputed_keys = 0u64;
            for unit in &ledger.units {
                match metadata
                    .index
                    .binary_search_by(|entry| entry.hash.cmp(&unit.record))
                {
                    Ok(position) => {
                        if metadata.index[position].key.is_some() {
                            recomputed_keys =
                                recomputed_keys.checked_add(1).ok_or(MediumError::Chain {
                                    rule: "begin-authority-v2/audit-key-count",
                                })?;
                        }
                    }
                    Err(_) => {
                        return Err(MediumError::Chain {
                            rule: "begin-authority-v2/audit-member-not-indexed",
                        });
                    }
                }
            }
            if recomputed_keys != ledger.reclaimed_keys {
                return Err(MediumError::Chain {
                    rule: "begin-authority-v2/audit-key-count",
                });
            }
            if installed.is_some() {
                found_audits.insert(batch_index, current);
            } else {
                target_above_begin = true;
            }
            let _ = object_hash;
        }
        // The completion check: the begin found AND every target
        // collected BELOW it (the collections are below-gated —
        // only sightings strictly after the begin's encounter in
        // the descending walk count, which are exactly the records
        // strictly below the begin on the linear ancestry).
        if let Some((begin_id, begin)) = installed.as_ref()
            && found_plan == Some(*plan_record)
            && found_audits.len() == plan.batches.len()
        {
            let mut audit_records = Vec::with_capacity(plan.batches.len());
            for batch in &plan.batches {
                audit_records.push(
                    found_audits
                        .get(&batch.index)
                        .copied()
                        .expect("the completion check established every batch"),
                );
            }
            return Ok(Some(V2InstalledBegin {
                id: *begin_id,
                begin: begin.clone(),
                audit_records,
            }));
        }
        let Some(parent) = record.parent else {
            // The founding: no begin under the key at all — the
            // definitive absence; a begin found with a required
            // target NOT strictly below it never certifies — the
            // typed temporal refusal distinguishes the sighting
            // ABOVE the begin (the strict-boundary violation — a
            // required audit or the plan revision published after
            // the begin) from the never-encountered-below absence.
            if installed.is_some() {
                return Err(MediumError::Chain {
                    rule: if target_above_begin {
                        "begin-authority-v2/target-above-begin"
                    } else {
                        "begin-authority-v2/target-not-below-begin"
                    },
                });
            }
            return Ok(None);
        };
        match reads.read_record_bytes(&parent) {
            Ok(_) => current = parent,
            Err(MediumError::MissingObject { .. }) => {
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
            Err(other) => return Err(other),
        }
    }
    Err(MediumError::Chain {
        rule: "begin-authority-v2/descent-bound",
    })
}

/// (V2X-CP2) The RETAINED-EVIDENCE V2 begin authority verification —
/// the recovery-safe form (NO original retirement record is
/// required: the digest fold and the closure equality belong to the
/// ADMISSION's first-execution witness, not to the post-publication
/// authority; a future authorized CP3 execution may legitimately
/// delete the retiring records). Over ANY [`BeginReads`]:
/// (1) the MK checkpoint authentication (the canonical membership,
/// the permanent metadata, the SK binding, the non-empty index);
/// (2) the installed plan discovery (the deterministic key), the
/// plan's binding to the checkpoint's facts, the ledger-byte
/// ceiling, the plan revision's installed shape and exact key, and
/// THE COMPLETE RETAINED MEMBERSHIP PROOF (the flattened plan
/// batches equal the checkpoint index's retirement set in the
/// ratified newest-first epoch order, position by position);
/// (3) the founding-derived seed (the founding is permanent — the
/// recovery-safe descent);
/// (4) the W2 mixed-state refusal (a v1 audit under the MK's v1
/// ledger key coexisting with the plan refuses — one
/// interpretation per MK, ever);
/// (5) [`verify_installed_begin_v2`] — the begin's canonical
/// membership, the four-field plan-record binding, the per-audit
/// immutable invariants (the byte limit, the retained key-count
/// recomputation), and the STRICT temporal position of the plan
/// and every audit below the begin.
/// Every failure is typed and fail-closed; the definitive absence
/// of a begin under the key is the typed
/// `v2-begin-authority/absent` refusal.
pub(crate) fn verify_v2_begin_authority_evidence_via<R: BeginReads>(
    reads: &mut R,
    head: Hash,
    mk: &Hash,
) -> Result<V2BeginAuthorityEvidence, MediumError> {
    // (1) The MK checkpoint authentication.
    let Some((_mk_id, meta_bytes)) = find_on_chain_via(reads, head, None, Some(mk))? else {
        return Err(MediumError::Chain {
            rule: "v2-begin-authority/mk-not-installed",
        });
    };
    let metadata =
        crate::checkpoint::CheckpointMetadata::decode(&meta_bytes).map_err(MediumError::Format)?;
    let mk_record = read_record_via(reads, mk)?;
    if Some(metadata.sk) != mk_record.parent {
        return Err(MediumError::Chain {
            rule: "v2-begin-authority/metadata-binding",
        });
    }
    if metadata.index.is_empty() {
        return Err(MediumError::Chain {
            rule: "v2-begin-authority/empty-index",
        });
    }
    // (2) The plan discovery, binding, and ceiling (the retained
    // form — no original-evidence phase).
    let Some((plan_record, plan)) = find_installed_plan_via(reads, head, mk)? else {
        return Err(MediumError::Chain {
            rule: "v2-begin-authority/plan-absent",
        });
    };
    if plan.mk != *mk
        || plan.sk != metadata.sk
        || plan.range_digest != metadata.range_digest
        || plan.seed != metadata.seed
    {
        return Err(MediumError::Chain {
            rule: "v2-begin-authority/plan-binding",
        });
    }
    if plan.ledger_byte_limit > MAX_RECLAMATION_LEDGER_BYTES {
        return Err(MediumError::Chain {
            rule: "v2-begin-authority/ledger-byte-limit-exceeds-ceiling",
        });
    }
    // (V2X-CP2 F2) THE INSTALLED PLAN REVISION'S SHAPE AND EXACT
    // DETERMINISTIC KEY: the discovery matched the key, and the
    // decoded content is bound — the revision ITSELF must carry
    // the publication's shape (a regular revision with exactly one
    // content reference — the already-decoded plan object), or the
    // record under the key is not the installed plan form.
    {
        let plan_key = reclamation_plan_idempotency_key(mk);
        let plan_revision = read_record_via(reads, &plan_record)?;
        if !matches!(plan_revision.kind, crate::record::RecordKind::Regular)
            || plan_revision.content_refs.len() != 1
            || plan_revision.idempotency_key.as_deref() != Some(plan_key.as_slice())
        {
            return Err(MediumError::Chain {
                rule: "v2-begin-authority/plan-shape-mismatch",
            });
        }
    }
    // (V2X-CP2 F2) THE COMPLETE RETAINED MEMBERSHIP PROOF — the
    // recovery-safe equivalent of the CP1 reader's ordered
    // partition agreement, from permanent evidence only: the
    // checkpoint's retained index independently re-derived into
    // its deterministic newest-first retirement sequence (from
    // the indexed epochs — the index is stored hash-sorted, so the
    // retirement order is DERIVED here, never trusted) and
    // compared position by position with the flattened plan batch
    // membership: no omission, no injection, no duplicate, no
    // reordering (a set-equal but reordered partition refuses —
    // the deterministic order is the identity). A forged,
    // correctly keyed plan that is otherwise self-consistent with
    // its own audit set therefore cannot appear complete.
    {
        let index_records: Vec<Hash> = {
            let mut newest_first: Vec<&crate::checkpoint::ReclaimedIndexEntry> =
                metadata.index.iter().collect();
            // Newest-first by epoch; the hash is the deterministic
            // tiebreak (genuine epochs are unique — an adversarial
            // duplicate-epoch index still derives one fixed order).
            newest_first
                .sort_unstable_by(|a, b| b.epoch.cmp(&a.epoch).then_with(|| a.hash.cmp(&b.hash)));
            newest_first.iter().map(|entry| entry.hash).collect()
        };
        let plan_records: Vec<Hash> = plan
            .batches
            .iter()
            .flat_map(|batch| batch.records.iter().copied())
            .collect();
        if plan_records != index_records {
            return Err(MediumError::Chain {
                rule: "v2-begin-authority/plan-index-partition",
            });
        }
    }
    // (3) The founding-derived seed (the recovery-safe descent —
    // the founding is permanent).
    let founding = founding_identity_via(reads, head)?;
    if crate::checkpoint::compute_digest_seed(&founding) != metadata.seed {
        return Err(MediumError::Chain {
            rule: "v2-begin-authority/founding-seed",
        });
    }
    // (4) The W2 mixed-state refusal.
    if find_on_chain_via(
        reads,
        head,
        Some(&reclamation_ledger_idempotency_key(mk)),
        None,
    )?
    .is_some()
    {
        return Err(MediumError::Chain {
            rule: "v2-begin-authority/v1-audit-mixed",
        });
    }
    // (5) The canonical installed begin with the strict temporal
    // proof (the metadata supplies the retained key-presence
    // evidence the per-audit key-count recomputation needs).
    let Some(installed) =
        verify_installed_begin_v2(reads, head, mk, &plan_record, &plan, &metadata)?
    else {
        return Err(MediumError::Chain {
            rule: "v2-begin-authority/absent",
        });
    };
    Ok(V2BeginAuthorityEvidence {
        begin_id: installed.id,
        plan_record,
        plan,
        audit_records: installed.audit_records,
    })
}

/// The gap-aware founding descent (the OC-C2-F4 corrective): the
/// walk from the authoritative HEAD to the ACTUAL founding revision,
/// crossing legitimate reclaimed gaps ONLY under authenticated
/// checkpoint skip tables — a missing parent is evidence of a gap,
/// never proof of the founding. Reader-parameterized so each
/// provider runs it under its already-held coordination (the
/// internal hash-verifying reads; no public trait method, no nested
/// lock or transaction).
pub fn founding_identity(
    head: Hash,
    read_record_bytes: impl Fn(&Hash) -> Result<Vec<u8>, MediumError>,
    read_object_bytes: impl Fn(&Hash) -> Result<Vec<u8>, MediumError>,
    record_present: impl Fn(&Hash) -> Result<bool, MediumError>,
) -> Result<Hash, MediumError> {
    const MAX_DESCENT: u64 = 1 << 20;
    let mut current = head;
    let mut seen = std::collections::BTreeSet::new();
    let mut skip_table: Option<crate::checkpoint::CheckpointMetadata> = None;
    for _ in 0..MAX_DESCENT {
        if !seen.insert(current) {
            return Err(MediumError::Chain { rule: "cycle" });
        }
        let bytes = read_record_bytes(&current)?;
        let record = RevisionRecord::decode(&bytes).map_err(MediumError::Format)?;
        if let Some(object) = record.content_refs.first().copied() {
            let object_bytes = read_object_bytes(&object)?;
            if StoredObject::new(object_bytes.clone())?.domain()
                == crate::checkpoint::CHECKPOINT_METADATA_DOMAIN
            {
                skip_table = Some(
                    crate::checkpoint::CheckpointMetadata::decode(&object_bytes)
                        .map_err(MediumError::Format)?,
                );
            }
        }
        let Some(parent) = record.parent else {
            // The verified parentless record IS the founding.
            return Ok(current);
        };
        if record_present(&parent)? {
            current = parent;
            continue;
        }
        // The gap: legitimate ONLY under the most recent
        // authenticated skip table (the self-skip/skip-for
        // distinction); an uncovered gap refuses fail-closed.
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
    Err(MediumError::Chain {
        rule: "begin-admission/founding-descent-bound",
    })
}

/// The shared first-execution evidence core (OC-C2-B): given the
/// already-authenticated ledger and metadata (the provider's own
/// internal, coordination-respecting authentication) and a
/// hash-verifying record-bytes reader, verify the complete original
/// retirement evidence and construct the begin from the verified
/// values. The reader is provider-injected so the verification runs
/// under the coordination the provider already holds — no public
/// trait method, no nested lock, no transaction escape.
pub fn verify_first_execution_evidence(
    ledger: &ReclamationLedger,
    metadata: &crate::checkpoint::CheckpointMetadata,
    founding: &Hash,
    read_record_bytes: impl Fn(&Hash) -> Result<Vec<u8>, MediumError>,
    read_object_bytes: impl Fn(&Hash) -> Result<Vec<u8>, MediumError>,
) -> Result<ReclamationBegin, MediumError> {
    // (B) The checkpoint bindings: the ledger's SK, digest, and seed
    // agree with the MK's permanent metadata.
    if ledger.sk != metadata.sk
        || ledger.range_digest != metadata.range_digest
        || ledger.seed != metadata.seed
    {
        return Err(MediumError::Chain {
            rule: "begin-admission/binding-mismatch",
        });
    }
    // The units exactly match the retirement index (the record
    // identities, as a set — no omission, no extraneous entry).
    let index: std::collections::BTreeSet<Hash> =
        metadata.index.iter().map(|entry| entry.hash).collect();
    let units: std::collections::BTreeSet<Hash> =
        ledger.units.iter().map(|unit| unit.record).collect();
    if index.len() != metadata.index.len() || units.len() != ledger.units.len() || index != units {
        return Err(MediumError::Chain {
            rule: "begin-admission/units-index-mismatch",
        });
    }
    // The founding-derived seed (always computable — the founding is
    // permanent).
    verify_founding_seed(metadata, founding)?;
    // (C) The complete original retirement evidence: EVERY unit's
    // record present (SK included — a missing original record
    // refuses; the deletion-prefix absence is NEVER first-execution
    // authority) and its manifest exactly equal to its actual
    // content references.
    for unit in &ledger.units {
        let bytes = read_record_bytes(&unit.record)?;
        let record = RevisionRecord::decode(&bytes).map_err(MediumError::Format)?;
        if unit.objects != record.content_refs {
            return Err(MediumError::Chain {
                rule: "begin-admission/units-objects-mismatch",
            });
        }
        // (F1) The referenced CONTENT evidence: every original
        // content object present and hash-verified (the reader is
        // the provider's hash-verifying read), with the transitive
        // composite closure verified under the established rules —
        // a matching ledger list alone is never proof the content
        // exists intact.
        for identity in &record.content_refs {
            let object_bytes = read_object_bytes(identity)?;
            if StoredObject::new(object_bytes.clone())?.domain()
                == crate::composite_manifest::SESSION_SNAPSHOT_V2_DOMAIN
            {
                let manifest = crate::composite_manifest::decode(&object_bytes)
                    .map_err(MediumError::Format)?;
                for entry in &manifest.entries {
                    let chunk_bytes = read_object_bytes(&entry.identity)?;
                    crate::composite_append::verify_chunk_dependency(
                        &chunk_bytes,
                        &entry.identity,
                        &manifest,
                    )
                    .map_err(|_error| MediumError::Chain {
                        rule: "begin-admission/content-closure-verification",
                    })?;
                }
            }
        }
    }
    // The valid newest-first retirement order.
    verify_newest_first_order(&ledger.units, metadata)?;
    // (D) The INDEPENDENT range-digest recomputation over the
    // actual, hash-verified original canonical record bytes, in the
    // established order, with the founding-derived seed — a
    // fabricated ledger whose digest merely agrees with the metadata
    // refuses here (the original history itself must prove the
    // commitment).
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
        range_bytes.push(read_record_bytes(&record)?);
    }
    let recomputed = crate::checkpoint::chain_range_digest(&metadata.seed, &range_bytes);
    if recomputed != ledger.range_digest {
        return Err(MediumError::Chain {
            rule: "begin-admission/range-digest-mismatch",
        });
    }
    // (E) The construction from the verified values only.
    Ok(ReclamationBegin {
        mk: ledger.mk,
        source: Hash::ZERO, // filled by the caller with the authenticated audit identity
        range_digest: recomputed,
        seed: metadata.seed,
    })
}

/// (V2-W F4) The stable journal-root identity's deterministic
/// derivation: the hash over the canonical encoding of the founding
/// identity and the authoritative location — stable across
/// rotations, reproducible at every recovery, and independent of
/// the filesystem's inode or path spelling beyond the location's
/// own normalized form.
pub fn journal_root_id(founding: &Hash, location: &str) -> Hash {
    let mut enc = Encoder::new();
    enc.write_str("engine-persist/journal-root-id/1");
    enc.write_bytes(founding.as_bytes());
    enc.write_str(location);
    let bytes = enc.into_bytes();
    crate::hash::hash_object(&bytes)
}

/// (V2-W F4) The installed binding discovery: the record carrying
/// THIS founding's deterministic key, with its strictly decoded
/// binding.
pub fn find_installed_journal_root_binding(
    medium: &mut dyn CanonicalMedium,
    founding: &Hash,
) -> Result<Option<(Hash, JournalRootBinding)>, MediumError> {
    match find_on_chain(
        medium,
        Some(&journal_root_binding_idempotency_key(founding)),
        None,
    )? {
        None => Ok(None),
        Some((id, bytes)) => Ok(Some((
            id,
            JournalRootBinding::decode(&bytes).map_err(MediumError::Format)?,
        ))),
    }
}

/// (V2-W, W2-B) The journal-evidence collector — the writer-only
/// preparation bridge over the SAME trusted discipline the
/// production crossing uses: the active journal must scan green
/// before any stale generation is removable (the survival set
/// stands), the stale names derive from the rotation's generation
/// evidence, and the ownership stays the journal root's. The plan
/// writer binds the collected obligations to the immutable plan at
/// admission; a retry or recovery discovers and verifies the
/// installed plan — never regenerates different obligations and
/// accepts them as equivalent.
pub fn collect_reclaimable_journal_files(
    journal_root: &std::path::Path,
) -> Result<Vec<String>, crate::journal::JournalError> {
    let active = crate::rotation::active_journal_path(journal_root)?;
    if active == journal_root {
        return Ok(Vec::new());
    }
    let active_name = active
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned);
    crate::journal::T1Journal::scan_file(&active)?;
    let names = crate::rotation::read_journal_generations(journal_root)?;
    let mut stale = Vec::new();
    for name in names {
        if active_name.as_deref() == Some(name.as_str()) {
            continue;
        }
        stale.push(name);
    }
    stale.sort();
    Ok(stale)
}

/// (V2-W F4) The bound journal-obligation collection: the SAME
/// trusted rotation-lineage discipline as the legacy collector,
/// with EVERY identity verified — the active journal scanned
/// through the bound form (the founding hash and root ID checked
/// against the installed canonical binding; a legacy, foreign, or
/// corrupt header refuses), and EVERY stale generation named by the
/// ownership record verified present with the SAME bound identities
/// (a missing generation is un-verifiable retirement evidence —
/// fail closed; a mismatched generation refuses). The caller holds
/// the root's exclusive sidecar lock across this collection and the
/// canonical publication (the serialization boundary).
pub fn collect_bound_reclaimable_journal_files(
    journal_root: &std::path::Path,
    founding: &Hash,
    root_id: &Hash,
) -> Result<Vec<String>, MediumError> {
    let refused = |rule: &'static str| MediumError::Chain { rule };
    let active = crate::rotation::active_journal_path(journal_root)
        .map_err(|_error| refused("journal-root/journal-evidence"))?;
    if active == journal_root {
        // The unrotated root IS the active journal: the bound scan
        // verifies its identity (an absent file is MISSING evidence
        // once the binding exists — never the empty fallback).
        crate::journal::scan_bound_file(journal_root, founding, root_id)
            .map_err(|_error| refused("journal-root/active-identity"))?;
        return Ok(Vec::new());
    }
    let active_name = active
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned);
    crate::journal::scan_bound_file(&active, founding, root_id)
        .map_err(|_error| refused("journal-root/active-identity"))?;
    let names = crate::rotation::read_journal_generations(journal_root)
        .map_err(|_error| refused("journal-root/journal-evidence"))?;
    let mut stale = Vec::new();
    let bound_header_len = crate::journal::bound_header_bytes(founding, root_id).len();
    for name in names {
        if active_name.as_deref() == Some(name.as_str()) {
            continue;
        }
        let path = journal_root.with_file_name(&name);
        // Read only the bound header prefix; the full journal is not needed.
        let bytes = crate::journal::read_prefix(&path, bound_header_len)
            .map_err(|_error| refused("journal-root/generation-missing"))?;
        match crate::journal::read_bound_header(&bytes, founding, root_id) {
            Ok(crate::journal::BoundHeaderState::Matching { .. }) => {}
            _ => return Err(refused("journal-root/generation-identity-mismatch")),
        }
        stale.push(name);
    }
    stale.sort();
    Ok(stale)
}

/// (V2-W) The on-chain discovery: the gap-aware descent from the
/// authoritative HEAD, matching either an exact idempotency key or
/// an exact record identity, returning the matched record's id and
/// its FIRST content object's bytes. The skip-table discipline is
/// the established one (a missing parent crosses only under an
/// authenticated checkpoint skip table); a record bypassed by a
/// skip jump is never falsely reported absent on production-shaped
/// tables (the true nearest-retained-ancestor targets). Shared by
/// the W2 exclusion checks, the writer duplicate resolutions, and
/// the audit writer's plan discovery.
pub(super) fn find_on_chain(
    medium: &mut dyn CanonicalMedium,
    key: Option<&[u8]>,
    id: Option<&Hash>,
) -> Result<Option<(Hash, Vec<u8>)>, MediumError> {
    let head = medium
        .head()?
        .ok_or(MediumError::Chain { rule: "unfounded" })?;
    find_on_chain_via(&mut MediumReads::new(medium), head, key, id)
}

/// (V2X-CP2) The reader-generic keyed/id gap-aware descent — the
/// accepted [`find_on_chain`] walk over ANY [`BeginReads`] (the
/// provider adapters under their own coordination; the public
/// medium-based form above remains the thin wrapper). The
/// discipline is unchanged: the hash-verifying reads, the strict
/// decode, the authenticated skip-table tracking and gap crossing,
/// the cycle guard, the bounded descent, the founding termination.
pub(crate) fn find_on_chain_via<R: BeginReads>(
    reads: &mut R,
    head: Hash,
    key: Option<&[u8]>,
    id: Option<&Hash>,
) -> Result<Option<(Hash, Vec<u8>)>, MediumError> {
    const MAX_DESCENT: u64 = 1 << 20;
    let mut current = head;
    let mut seen = std::collections::BTreeSet::new();
    let mut skip_table: Option<crate::checkpoint::CheckpointMetadata> = None;
    for _ in 0..MAX_DESCENT {
        if !seen.insert(current) {
            return Err(MediumError::Chain { rule: "cycle" });
        }
        let record = read_record_via(reads, &current)?;
        let mut first_object: Option<Vec<u8>> = None;
        if let Some(object) = record.content_refs.first().copied() {
            let bytes = reads.read_object_bytes(&object)?;
            if StoredObject::new(bytes.clone())?.domain()
                == crate::checkpoint::CHECKPOINT_METADATA_DOMAIN
            {
                skip_table = Some(
                    crate::checkpoint::CheckpointMetadata::decode(&bytes)
                        .map_err(MediumError::Format)?,
                );
            }
            first_object = Some(bytes);
        }
        if (id.is_some() && id == Some(&current))
            || (key.is_some() && record.idempotency_key.as_deref() == key)
        {
            return Ok(first_object.map(|bytes| (current, bytes)));
        }
        let Some(parent) = record.parent else {
            return Ok(None);
        };
        match reads.read_record_bytes(&parent) {
            Ok(_) => current = parent,
            Err(MediumError::MissingObject { .. }) => {
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
            Err(other) => return Err(other),
        }
    }
    Err(MediumError::Chain {
        rule: "reclamation/descent-bound",
    })
}

/// (V2-W, V2W-F2) The authoritative founding identity over the
/// generic medium: the gap-aware descent from the HEAD to the
/// chain's root record (the parentless founding), crossing
/// legitimate gaps only under authenticated checkpoint skip tables
/// — the same discipline the accepted `founding_identity` helper
/// enforces under the providers' own coordination, shaped here for
/// the provider-neutral writer boundary.
pub fn founding_identity_medium(medium: &mut dyn CanonicalMedium) -> Result<Hash, MediumError> {
    const MAX_DESCENT: u64 = 1 << 20;
    let head = medium
        .head()?
        .ok_or(MediumError::Chain { rule: "unfounded" })?;
    let mut current = head;
    let mut seen = std::collections::BTreeSet::new();
    let mut skip_table: Option<crate::checkpoint::CheckpointMetadata> = None;
    for _ in 0..MAX_DESCENT {
        if !seen.insert(current) {
            return Err(MediumError::Chain { rule: "cycle" });
        }
        let record = medium.read_record(&current)?;
        if let Some(object) = record.content_refs.first().copied() {
            let bytes = medium.read_object(&object)?;
            if StoredObject::new(bytes.clone())?.domain()
                == crate::checkpoint::CHECKPOINT_METADATA_DOMAIN
            {
                skip_table = Some(
                    crate::checkpoint::CheckpointMetadata::decode(&bytes)
                        .map_err(MediumError::Format)?,
                );
            }
        }
        let Some(parent) = record.parent else {
            return Ok(current);
        };
        match medium.read_record(&parent) {
            Ok(_) => current = parent,
            Err(MediumError::MissingObject { .. }) => {
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
            Err(other) => return Err(other),
        }
    }
    Err(MediumError::Chain {
        rule: "reclamation/descent-bound",
    })
}

/// (V2-W) The installed-plan discovery: the record carrying THIS
/// MK's deterministic plan key, with its strictly decoded plan.
pub fn find_installed_plan(
    medium: &mut dyn CanonicalMedium,
    mk: &Hash,
) -> Result<Option<(Hash, ReclaimPlan)>, MediumError> {
    match find_on_chain(medium, Some(&reclamation_plan_idempotency_key(mk)), None)? {
        None => Ok(None),
        Some((id, bytes)) => Ok(Some((
            id,
            ReclaimPlan::decode(&bytes).map_err(MediumError::Format)?,
        ))),
    }
}

/// (V2X-CP2) The reader-generic installed-plan discovery: the
/// deterministic MK-bound key lookup and the strict decode over
/// ANY [`BeginReads`] — the provider-coordinated equivalent of
/// [`find_installed_plan`].
pub(crate) fn find_installed_plan_via<R: BeginReads>(
    reads: &mut R,
    head: Hash,
    mk: &Hash,
) -> Result<Option<(Hash, ReclaimPlan)>, MediumError> {
    match find_on_chain_via(
        reads,
        head,
        Some(&reclamation_plan_idempotency_key(mk)),
        None,
    )? {
        None => Ok(None),
        Some((id, bytes)) => Ok(Some((
            id,
            ReclaimPlan::decode(&bytes).map_err(MediumError::Format)?,
        ))),
    }
}

/// (V2-W) The installed v2 batch-audit discovery: the record
/// carrying THIS (MK, batch) deterministic key, with its strictly
/// decoded ledger.
pub fn find_installed_v2_audit(
    medium: &mut dyn CanonicalMedium,
    mk: &Hash,
    batch: u64,
) -> Result<Option<(Hash, ReclamationLedgerV2)>, MediumError> {
    match find_on_chain(
        medium,
        Some(&reclamation_ledger_v2_idempotency_key(mk, batch)),
        None,
    )? {
        None => Ok(None),
        Some((id, bytes)) => Ok(Some((
            id,
            ReclamationLedgerV2::decode(&bytes).map_err(MediumError::Format)?,
        ))),
    }
}

/// The discovery's result (V2X-CP1 F1, the bounded-memory contract):
/// each expected key that was found, mapped to its authenticated
/// record identity and single content object's HASH — locator
/// evidence only, never the payload bytes. The per-audit
/// verification RELOADS and hash-verifies each audit's record and
/// object when it processes it (a previously discovered key is not
/// itself permission to trust later bytes), so the reader never
/// retains more than one audit's payload at a time.
pub(crate) type DiscoveredKeys = std::collections::BTreeMap<Vec<u8>, (Hash, Hash)>;

/// (V2X-CP1) The single-descent multi-key discovery: ONE
/// authenticated, gap-aware, cycle-guarded walk from the canonical
/// HEAD, checking each encountered record's idempotency key against
/// an expected set and collecting every match's record identity and
/// single content object bytes — O(chain) regardless of the
/// key-set's size, replacing the O(keys × chain) composition of
/// per-key lookups in the reader's normal verification path. The
/// walk preserves the accepted discovery discipline verbatim (the
/// [`find_on_chain`] skeleton): the provider's hash-verifying reads,
/// the strict record decode, the authenticated checkpoint
/// skip-table tracking and gap crossing (an uncovered gap is the
/// typed absence, never silently crossed), the cycle guard, the
/// bounded descent, and the founding termination. The early exit
/// fires once every expected key has been found. A matched record
/// carries the installed shape the plan/audit publications build
/// (a regular revision, exactly one content reference) — anything
/// else refuses typed. THE RETENTION IS BOUNDED (V2X-CP1 F1): only
/// the matched records' identities and their single content
/// objects' HASHES are retained — the first object's bytes are read
/// for the authenticated skip-table detection and then dropped;
/// the payload is re-read and hash-verified per audit at
/// verification time. This is NOT a cache and NOT a trusted
/// provider index: it is the canonical walk with a set-membership
/// predicate — the same trust basis as every accepted discovery —
/// and its equivalence with the per-key lookups is pinned by the
/// CP1 regressions.
/// (V2X-CP2) The medium-based convenience wrapper — the production
/// paths use the reader-generic form under their own coordination;
/// this one remains for the in-crate regression suites.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn discover_keyed_set(
    medium: &mut dyn CanonicalMedium,
    expected: &std::collections::BTreeSet<Vec<u8>>,
) -> Result<DiscoveredKeys, MediumError> {
    let head = medium
        .head()?
        .ok_or(MediumError::Chain { rule: "unfounded" })?;
    discover_keyed_set_via(&mut MediumReads::new(medium), head, expected)
}

/// (V2X-CP2) The reader-generic single-descent multi-key discovery
/// — the accepted [`discover_keyed_set`] walk over ANY [`BeginReads`]
/// (the provider adapters under their own coordination). The
/// discipline and the bounded locator-only retention are unchanged.
pub(crate) fn discover_keyed_set_via<R: BeginReads>(
    reads: &mut R,
    head: Hash,
    expected: &std::collections::BTreeSet<Vec<u8>>,
) -> Result<DiscoveredKeys, MediumError> {
    const MAX_DESCENT: u64 = 1 << 20;
    let mut found: DiscoveredKeys = std::collections::BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut skip_table: Option<crate::checkpoint::CheckpointMetadata> = None;
    let mut current = head;
    for _ in 0..MAX_DESCENT {
        if !seen.insert(current) {
            return Err(MediumError::Chain { rule: "cycle" });
        }
        let record = read_record_via(reads, &current)?;
        if let Some(object) = record.content_refs.first().copied() {
            // The first object's bytes are read for the authenticated
            // skip-table detection and then DROPPED (the bounded
            // contract: only the hashes are retained).
            let bytes = reads.read_object_bytes(&object)?;
            if StoredObject::new(bytes.clone())?.domain()
                == crate::checkpoint::CHECKPOINT_METADATA_DOMAIN
            {
                skip_table = Some(
                    crate::checkpoint::CheckpointMetadata::decode(&bytes)
                        .map_err(MediumError::Format)?,
                );
            }
        }
        if let Some(key) = record.idempotency_key.clone()
            && expected.contains(&key)
            && !found.contains_key(&key)
        {
            if !matches!(record.kind, crate::record::RecordKind::Regular)
                || record.content_refs.len() != 1
            {
                return Err(MediumError::Chain {
                    rule: "v2-discovery/installed-shape-mismatch",
                });
            }
            let object_hash = record.content_refs[0];
            found.insert(key, (current, object_hash));
            if found.len() == expected.len() {
                return Ok(found);
            }
        }
        let Some(parent) = record.parent else {
            return Ok(found);
        };
        match reads.read_record_bytes(&parent) {
            Ok(_) => current = parent,
            Err(MediumError::MissingObject { .. }) => {
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
            Err(other) => return Err(other),
        }
    }
    Err(MediumError::Chain {
        rule: "v2-discovery/descent-bound",
    })
}

pub fn verify_v2_execution_readiness(
    medium: &mut dyn CanonicalMedium,
    mk: &Hash,
) -> Result<V2ExecutionReadiness, MediumError> {
    let head = medium
        .head()?
        .ok_or(MediumError::Chain { rule: "unfounded" })?;
    verify_v2_execution_readiness_via(&mut MediumReads::new(medium), head, mk)
}

/// (V2X-CP2) The reader-generic trusted all-audits reader — the
/// accepted CP1 contract over ANY [`BeginReads`] (the provider
/// adapters under their already-held coordination boundary: the
/// LocalFile lock-held internal reads, the SQLite transaction, the
/// public medium). Every accepted semantic is unchanged (the
/// parity gate: the thirteen CP1 reader regressions run through
/// the public wrapper into THIS core).
pub fn verify_v2_execution_readiness_via<R: BeginReads>(
    reads: &mut R,
    head: Hash,
    mk: &Hash,
) -> Result<V2ExecutionReadiness, MediumError> {
    // (1) THE CHECKPOINT AUTHENTICATION — the same facts the trusted
    // plan publication established: the MK's canonical chain
    // membership (the descent match), its permanent metadata, the
    // SK binding, and the non-empty retirement index. A bogus MK is
    // a typed refusal, never an ambiguity.
    let Some((_mk_id, meta_bytes)) = find_on_chain_via(reads, head, None, Some(mk))? else {
        return Err(MediumError::Chain {
            rule: "v2-readiness/mk-not-installed",
        });
    };
    let metadata =
        crate::checkpoint::CheckpointMetadata::decode(&meta_bytes).map_err(MediumError::Format)?;
    let mk_record = read_record_via(reads, mk)?;
    if Some(metadata.sk) != mk_record.parent {
        return Err(MediumError::Chain {
            rule: "v2-readiness/metadata-binding",
        });
    }
    if metadata.index.is_empty() {
        return Err(MediumError::Chain {
            rule: "v2-readiness/empty-index",
        });
    }
    // (2) THE PLAN DISCOVERY (the deterministic MK-bound key): a
    // genuine checkpoint with no installed plan is the legitimate
    // pre-publication state — classified, never certified.
    let Some((plan_record, plan)) = find_installed_plan_via(reads, head, mk)? else {
        return Ok(V2ExecutionReadiness::PublicationIncomplete {
            missing_batches: Vec::new(),
        });
    };
    // (3) THE PLAN'S BINDING to THIS checkpoint's authenticated
    // facts (before any range-crossing work — a read-free field
    // comparison): the MK, the SK, the pre-verified range digest,
    // and the founding-derived seed — the plan carries the
    // checkpoint's own values or refuses.
    if plan.mk != *mk
        || plan.sk != metadata.sk
        || plan.range_digest != metadata.range_digest
        || plan.seed != metadata.seed
    {
        return Err(MediumError::Chain {
            rule: "v2-readiness/plan-binding",
        });
    }
    // (V2X-CP1 F3) THE LEDGER-BYTE CEILING — the accepted plan
    // publisher enforces the canonical upper bound
    // (MAX_RECLAMATION_LEDGER_BYTES); the reader independently
    // rejects a decoded installed plan whose recorded limit exceeds
    // it (the plan codec itself refuses only the zero form, so an
    // adversarially installed over-ceiling plan must be caught
    // HERE, before anything downstream trusts the recorded bound).
    // The constant is unchanged and its numerical value is NOT
    // ratified by this check.
    if plan.ledger_byte_limit > MAX_RECLAMATION_LEDGER_BYTES {
        return Err(MediumError::Chain {
            rule: "v2-readiness/ledger-byte-limit-exceeds-ceiling",
        });
    }
    // (4) THE ALL-AUDITS DISCOVERY AND COMPLETENESS — BEFORE any
    // range-crossing descent or evidence verification (V2X-CP1 F2:
    // the readiness classifications are truthful — the
    // incomplete-publication classification is decided on the
    // audit-set completeness ALONE, and the range-not-intact
    // classification is reachable ONLY with the complete audit set
    // installed; the combined incomplete-and-degraded state is
    // never represented as though audit completeness were proven).
    // The discovery is the single-descent multi-key walk (O(chain)
    // once, not per batch), retaining ONLY the bounded locator
    // evidence (the record identities and the content-object
    // hashes — F1).
    let plan_key = reclamation_plan_idempotency_key(mk);
    let mut expected_keys: std::collections::BTreeSet<Vec<u8>> =
        std::collections::BTreeSet::from([plan_key.clone()]);
    for batch in &plan.batches {
        expected_keys.insert(reclamation_ledger_v2_idempotency_key(mk, batch.index));
    }
    // The discovery's descent crosses the retirement range when a
    // key remains unfound above it: if the crossing hits an absent
    // PLAN MEMBER (a retiring record this plan itself lists), the
    // audit-set completeness CANNOT be established (the walk cannot
    // prove the unfound key's absence below the degraded range) —
    // the truthful answer asserts NEITHER the completeness NOR the
    // readiness: a typed refusal, never a false PublicationIncomplete
    // (which would claim the missing set is known) and never a
    // RangeNotIntact (which claims the complete audit set).
    let discovered = match discover_keyed_set_via(reads, head, &expected_keys) {
        Ok(discovered) => discovered,
        Err(MediumError::MissingObject { hash })
            if plan
                .batches
                .iter()
                .any(|batch| batch.records.contains(&hash)) =>
        {
            return Err(MediumError::Chain {
                rule: "v2-readiness/range-degraded-before-completeness",
            });
        }
        Err(other) => return Err(other),
    };
    let mut missing_batches: Vec<u64> = Vec::new();
    for batch in &plan.batches {
        if !discovered.contains_key(&reclamation_ledger_v2_idempotency_key(mk, batch.index)) {
            missing_batches.push(batch.index);
        }
    }
    if !missing_batches.is_empty() {
        return Ok(V2ExecutionReadiness::PublicationIncomplete { missing_batches });
    }
    // (5) THE BOUNDED FIRST-EXECUTION EVIDENCE — the SAME accepted
    // trusted-begin contract the publication proved (V2W-F2),
    // re-proved HERE against the present state in a BOUNDED form
    // (V2X-CP1 F1: no full-range unit-manifest retention, no
    // aggregate payload retention — one member's evidence and one
    // 32-byte digest accumulator at a time), and only with the
    // complete audit set installed (F2). Every check of the
    // accepted contract is preserved: the founding identity, every
    // indexed record present and hash-valid, every content object
    // present and hash-verified (the composite closure
    // envelope-verified), the retirement membership, the
    // founding-derived seed, and the INDEPENDENT range-digest
    // recomputation through the exact accepted algorithm (the
    // shared chain_range_digest_step — byte-exact with the slice
    // fold by construction, the parity pinned by the regression).
    // An absent member anywhere in this phase is the
    // RANGE-NOT-INTACT classification (Option C: before a begin, an
    // absence is not recoverable); corruption propagates typed.
    //
    // (5a) THE INDEX CONSTRUCTION AND THE INCREMENTAL DIGEST FOLD —
    // every retirement-index member read through the hash-verifying
    // read, in the digest's oldest-first epoch order, the identity
    // sets (the member records and their content objects) collected
    // as the absence guards for the range-crossing descents below
    // (the record codec canonically ORDERS the content references,
    // so a range record's FIRST reference may be any of its objects
    // — the descents' first-object peeks hit them), and the digest
    // chained one member at a time (the SK excluded, exactly as the
    // accepted verifier's digest-membership rule).
    let mut indexed_members: Vec<&crate::checkpoint::ReclaimedIndexEntry> =
        metadata.index.iter().collect();
    indexed_members.sort_unstable_by_key(|entry| entry.epoch);
    let mut index_objects: std::collections::BTreeSet<Hash> = std::collections::BTreeSet::new();
    let mut digest_chain = metadata.seed;
    for entry in &indexed_members {
        let record = match read_record_via(reads, &entry.hash) {
            Ok(record) => record,
            Err(MediumError::MissingObject { hash }) => {
                return Ok(V2ExecutionReadiness::RangeNotIntact { absent: vec![hash] });
            }
            Err(other) => return Err(other),
        };
        index_objects.extend(record.content_refs.iter().copied());
        if entry.hash != metadata.sk {
            digest_chain = crate::checkpoint::chain_range_digest_step(
                &digest_chain,
                &record.canonical_bytes(),
            );
        }
    }
    let range_member = |hash: &Hash| {
        metadata.index.iter().any(|entry| entry.hash == *hash) || index_objects.contains(hash)
    };
    // (5b) THE AUTHORITATIVE FOUNDING — the gap-aware descent to the
    // parentless root, crossing the retirement range under the
    // authenticated skip tables; an absent RANGE MEMBER at the
    // crossing is the range-not-intact classification, and the
    // founding-derived seed is verified against the metadata (the
    // accepted rule: the seed's authority is the founding).
    let founding = match founding_identity_via(reads, head) {
        Ok(founding) => founding,
        Err(MediumError::MissingObject { hash }) if range_member(&hash) => {
            return Ok(V2ExecutionReadiness::RangeNotIntact { absent: vec![hash] });
        }
        Err(other) => return Err(other),
    };
    if crate::checkpoint::compute_digest_seed(&founding) != metadata.seed {
        return Err(MediumError::Chain {
            rule: "v2-readiness/founding-seed",
        });
    }
    // (5c) THE PER-MEMBER CONTENT EVIDENCE — for every indexed
    // member, the complete hash-verified content reads with the
    // transitive composite closure and dependency-envelope
    // verification (the SAME collect_unit_objects machinery the
    // publication and the accepted verifier use), the returned
    // manifest DISCARDED immediately (the bounded contract: the
    // per-unit closure equality against the installed audits is
    // established per audit in the streaming phase below, never by
    // retaining the whole range's manifests).
    for entry in &indexed_members {
        match collect_unit_objects_via(reads, &entry.hash) {
            Ok(_objects) => {}
            Err(MediumError::MissingObject { hash }) => {
                return Ok(V2ExecutionReadiness::RangeNotIntact { absent: vec![hash] });
            }
            Err(other) => return Err(other),
        }
    }
    // (5d) THE DIGEST COMPARISON — UNCONDITIONAL (V2X-CP1 F4): the
    // incrementally recomputed range digest equals the checkpoint's
    // stored commitment regardless of the member count — an empty
    // digest-member sequence's digest is the seed itself (exactly
    // the accepted chain_range_digest contract for an empty range),
    // and an incorrect stored commitment refuses even then; the
    // fabricated-checkpoint counterexample refuses here.
    if digest_chain != metadata.range_digest {
        return Err(MediumError::Chain {
            rule: "v2-readiness/range-digest-mismatch",
        });
    }
    // (6) THE MIXED V1/V2 REFUSAL — the reader-side mirror of the
    // accepted W2 bidirectional exclusion: one interpretation per
    // MK, ever. A v1 audit coexisting with a plan (constructible
    // only by bypassing the publication gates) refuses typed. The
    // key lookup's full-chain descent crosses the retirement range
    // (reading each crossed record's canonically-first reference):
    // an absent RANGE MEMBER at those reads is the range-not-intact
    // classification; anything else propagates.
    let v1_mixed = match find_on_chain_via(
        reads,
        head,
        Some(&reclamation_ledger_idempotency_key(mk)),
        None,
    ) {
        Ok(found) => found,
        Err(MediumError::MissingObject { hash }) if range_member(&hash) => {
            return Ok(V2ExecutionReadiness::RangeNotIntact { absent: vec![hash] });
        }
        Err(other) => return Err(other),
    };
    if v1_mixed.is_some() {
        return Err(MediumError::Chain {
            rule: "v2-readiness/v1-audit-mixed",
        });
    }
    // (7) THE ORDERED PARTITION AGREEMENT — the plan's complete
    // membership, flattened in the plan's deterministic batch order,
    // equals the checkpoint index's retiring set in the ratified
    // newest-first epoch order, position by position: no omission,
    // no injection, no duplicate, no reordering (a set-equal but
    // reordered partition refuses — the deterministic order is the
    // identity).
    let index_records: Vec<Hash> = {
        let mut newest_first: Vec<&crate::checkpoint::ReclaimedIndexEntry> =
            metadata.index.iter().collect();
        newest_first.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.epoch));
        newest_first.iter().map(|entry| entry.hash).collect()
    };
    let plan_records: Vec<Hash> = plan
        .batches
        .iter()
        .flat_map(|batch| batch.records.iter().copied())
        .collect();
    if plan_records != index_records {
        return Err(MediumError::Chain {
            rule: "v2-readiness/plan-index-partition",
        });
    }
    // (8) THE PER-AUDIT STREAMING VERIFICATION — one audit loaded,
    // decoded, and fully verified at a time, its payload and decoded
    // closure released before the next loads (V2X-CP1 F1: at most
    // ONE audit's encoded payload and decoded closure resident):
    //   (i) THE RELOAD — the discovered locator is identity evidence
    //       only; the audit's RECORD is re-read hash-verified NOW
    //       (establishing the exact canonical identity, the revision
    //       shape, and the deterministic key) and its OBJECT is
    //       re-read hash-verified NOW (the content-addressed hash IS
    //       the content integrity) — a previously discovered key is
    //       not itself permission to trust later bytes;
    //  (ii) the strict decode (the schema bounds, the sharing
    //       rules, the zero-identity and count-coherence refusals —
    //       the owner-ratified codec);
    // (iii) the field agreement with the plan and the checkpoint
    //       metadata (the MK, SK, batch index, range digest, seed);
    //  (iv) the ORDERED unit sequence equals the plan's batch
    //       records exactly (position by position — not merely set
    //       equality);
    //   (v) the re-encoded ledger satisfies the plan's recorded
    //       byte limit;
    //  (vi) THE ORIGINAL EVIDENCE (per unit): the retiring record
    //       read through the hash-verifying read and the closure
    //       RECOMPUTED (the direct references plus the verified
    //       composite closure) exactly equal to the audit's ordered
    //       unit representation. An absent record or object is
    //       collected into the range-not-intact classification
    //       (Option C strict); corruption propagates typed.
    let mut absent: Vec<Hash> = Vec::new();
    let mut audit_records: Vec<Hash> = Vec::with_capacity(plan.batches.len());
    for batch in &plan.batches {
        let key = reclamation_ledger_v2_idempotency_key(mk, batch.index);
        let (audit_record, audit_object) = discovered
            .get(&key)
            .expect("the completeness check above established the discovery");
        let record = read_record_via(reads, audit_record)?;
        if !matches!(record.kind, crate::record::RecordKind::Regular)
            || record.content_refs.len() != 1
            || record.idempotency_key.as_deref() != Some(key.as_slice())
            || record.content_refs[0] != *audit_object
        {
            return Err(MediumError::Chain {
                rule: "v2-readiness/audit-shape-mismatch",
            });
        }
        let audit_bytes = reads.read_object_bytes(audit_object)?;
        let ledger = ReclamationLedgerV2::decode(&audit_bytes).map_err(MediumError::Format)?;
        if ledger.mk != *mk
            || ledger.sk != plan.sk
            || ledger.batch != batch.index
            || ledger.range_digest != plan.range_digest
            || ledger.seed != plan.seed
        {
            return Err(MediumError::Chain {
                rule: "v2-readiness/audit-binding",
            });
        }
        let unit_records: Vec<Hash> = ledger.units.iter().map(|unit| unit.record).collect();
        if unit_records != batch.records {
            return Err(MediumError::Chain {
                rule: "v2-readiness/audit-order",
            });
        }
        if ledger.canonical_bytes().len() as u64 > plan.ledger_byte_limit {
            return Err(MediumError::Chain {
                rule: "v2-readiness/audit-byte-limit",
            });
        }
        for unit in &ledger.units {
            match reads
                .read_record_bytes(&unit.record)
                .map(|bytes| decode_record(&bytes))
            {
                Ok(_record) => match collect_unit_objects_via(reads, &unit.record) {
                    Ok(objects) => {
                        if objects != unit.objects {
                            return Err(MediumError::Chain {
                                rule: "v2-readiness/unit-closure-mismatch",
                            });
                        }
                    }
                    Err(MediumError::MissingObject { hash }) => absent.push(hash),
                    Err(other) => return Err(other),
                },
                Err(MediumError::MissingObject { hash }) => absent.push(hash),
                Err(other) => return Err(other),
            }
        }
        audit_records.push(*audit_record);
        // The decoded closure and the payload are released here —
        // the streaming bound (one audit resident at most).
        drop(ledger);
        drop(audit_bytes);
    }
    if !absent.is_empty() {
        return Ok(V2ExecutionReadiness::RangeNotIntact { absent });
    }
    let plan_object = discovered
        .get(&plan_key)
        .map(|(_record, object)| *object)
        .expect("the plan key was in the discovery's expected set");
    Ok(V2ExecutionReadiness::ExecutionReady {
        plan_record,
        plan_object,
        audit_records,
    })
}

/// (V2-W, V2W-F3) The unit's closure-expanded object manifest: the
/// record's direct content references plus the verified composite
/// closure (the first reference's /2 root decoded, every chunk
/// dependency envelope-verified — the same machinery the provider
/// walks use), collected while the first-execution evidence remains
/// intact. EVERY direct content reference is explicitly read
/// through the provider's hash-verifying read at this boundary (the
/// proof never depends on incidental full-chain validation or a
/// provider cursor-cache state), and a structurally invalid
/// DUPLICATE reference within one unit REFUSES typed (the ratified
/// representation is a set — the v2 ledger codec's own rule) rather
/// than being silently normalized; a repeated identity ACROSS units
/// remains legitimate sharing.
pub(crate) fn collect_unit_objects(
    medium: &mut dyn CanonicalMedium,
    record_id: &Hash,
) -> Result<Vec<Hash>, MediumError> {
    collect_unit_objects_via(&mut MediumReads::new(medium), record_id)
}

/// (V2X-CP2) The reader-generic closure-expanded manifest
/// collection — the accepted [`collect_unit_objects`] over ANY
/// [`BeginReads`]. Every direct reference is read through the
/// reader's hash-verifying read; the composite /2 root's manifest
/// is decoded and every chunk envelope verified — the same
/// machinery, no medium lock required.
pub(crate) fn collect_unit_objects_via<R: BeginReads>(
    reads: &mut R,
    record_id: &Hash,
) -> Result<Vec<Hash>, MediumError> {
    let record = read_record_via(reads, record_id)?;
    let mut objects: Vec<Hash> = Vec::new();
    let mut seen: HashSet<Hash> = HashSet::new();
    for (position, reference) in record.content_refs.iter().enumerate() {
        // (V2W-F3) The explicit hash-verified read of EVERY direct
        // reference — a corrupt or missing second (or later)
        // reference refuses through the typed integrity or
        // missing-object classification here, before any authority
        // installs.
        let bytes = reads.read_object_bytes(reference)?;
        if !seen.insert(*reference) {
            return Err(MediumError::Chain {
                rule: "reclamation-ledger-v2/duplicate-unit-object",
            });
        }
        objects.push(*reference);
        if position == 0
            && peek_domain(&bytes)? == crate::composite_manifest::SESSION_SNAPSHOT_V2_DOMAIN
        {
            let manifest =
                crate::composite_manifest::decode(&bytes).map_err(MediumError::Format)?;
            for entry in &manifest.entries {
                let chunk_bytes = reads.read_object_bytes(&entry.identity)?;
                crate::composite_append::verify_chunk_dependency(
                    &chunk_bytes,
                    &entry.identity,
                    &manifest,
                )
                .map_err(|_error| MediumError::Chain {
                    rule: "reclamation-plan/composite-closure-verification",
                })?;
                if seen.insert(entry.identity) {
                    objects.push(entry.identity);
                }
            }
        }
    }
    Ok(objects)
}

/// (F4-R3) The capability issuance: the founding discovery, the
/// installed-binding requirement (an unbound or legacy campaign
/// refuses — v2 trusted publication requires the bound lifecycle),
/// and the location check (a foreign root, a competing second
/// root, or a relocated root refuses — the root uniqueness and the
/// bypass prevention). NO lock and NO collection here — the
/// issuance is the authentication, the publication route owns the
/// serialization boundary.
pub fn verify_bound_journal_authority(
    medium: &mut dyn CanonicalMedium,
    journal_root: &std::path::Path,
) -> Result<BoundJournalAuthority, MediumError> {
    let founding = founding_identity_medium(medium)?;
    let Some((_binding_id, binding)) = find_installed_journal_root_binding(medium, &founding)?
    else {
        return Err(MediumError::Chain {
            rule: "journal-root/not-bound",
        });
    };
    if binding.location != JournalRootBinding::normalize_location(journal_root) {
        return Err(MediumError::Chain {
            rule: "journal-root/location-mismatch",
        });
    }
    Ok(BoundJournalAuthority {
        founding,
        root_id: binding.root_id,
    })
}

/// The canonical v1 audit authority verification (P2 Increment A,
/// corrected per A1-A4): a gap-aware authoritative descent proving
/// (A1) the EXACT supplied audit revision identity is encountered on
/// the chain — its computed identity, its first content object's
/// strict v1-ledger decode, its MK, and its deterministic MK-bound
/// key all checked AT that record (an unrelated or fabricated
/// source refuses even when a valid audit for the MK exists);
/// (A2) the requested MK is bound BY REVISION IDENTITY — the record
/// whose identity equals the MK, whose first reference strictly
/// decodes as checkpoint metadata whose SK is that record's parent
/// (a newer checkpoint's metadata never substitutes); (A3) the
/// descent crosses legitimately reclaimed gaps ONLY under the most
/// recent authenticated checkpoint's skip table (the established
/// discovery discipline — uncovered gaps refuse fail-closed; the
/// descent bound is a documented safety valve, not a schema limit);
/// and (A4) the pre-deletion manifest authority — every unit's
/// record is hash-verified by identity and its object list equals
/// the record's actual content references (the v1 collection
/// contract; omitted, extraneous, or mismatched deletion identities
/// refuse). Post-interruption resume verification — where retiring
/// records may already be deleted — is the manifest-driven
/// recovery's own path, never this pre-deletion boundary.
pub fn verify_canonical_v1_audit(
    medium: &mut dyn CanonicalMedium,
    audit_record: &Hash,
    mk: &Hash,
) -> Result<(ReclamationLedger, crate::checkpoint::CheckpointMetadata), MediumError> {
    // The safety-valve descent bound: each iteration consumes one
    // present record or one authenticated skip jump; a campaign
    // exceeding this refuses typed (the documented operational
    // limit — the same policy class as the survivor-walk bound,
    // pending the owner's numerical ratification).
    const MAX_DESCENT: u64 = 1 << 20;
    let mut current = medium
        .head()?
        .ok_or(MediumError::Chain { rule: "unfounded" })?;
    let mut ledger: Option<ReclamationLedger> = None;
    let mut metadata: Option<crate::checkpoint::CheckpointMetadata> = None;
    // (A3) The traversal skip table: the most recent checkpoint
    // metadata seen on the descent (ANY checkpoint — the gap-cover
    // authority), distinct from the requested MK's own metadata.
    let mut skip_table: Option<crate::checkpoint::CheckpointMetadata> = None;
    for _ in 0..MAX_DESCENT {
        let record = medium.read_record(&current)?;
        let parent = record.parent;
        if let Some(object) = record.content_refs.first().copied() {
            let bytes = medium.read_object(&object)?;
            let domain = StoredObject::new(bytes.clone())?.domain().to_owned();
            if domain == crate::checkpoint::CHECKPOINT_METADATA_DOMAIN {
                let decoded = crate::checkpoint::CheckpointMetadata::decode(&bytes)
                    .map_err(MediumError::Format)?;
                skip_table = Some(decoded.clone());
                // (A2) The requested MK, bound by revision identity:
                // this record IS the MK, its metadata's SK is its
                // own parent.
                if current == *mk {
                    if decoded.sk != parent.unwrap_or(Hash::ZERO) {
                        return Err(MediumError::Chain {
                            rule: "reclamation-complete/mk-binding-mismatch",
                        });
                    }
                    metadata = Some(decoded);
                }
            } else if domain == RECLAMATION_LEDGER_DOMAIN && current == *audit_record {
                // (A1) The EXACT requested audit identity.
                let decoded = ReclamationLedger::decode(&bytes).map_err(MediumError::Format)?;
                if decoded.mk != *mk {
                    return Err(MediumError::Chain {
                        rule: "reclamation-complete/mk-mismatch",
                    });
                }
                let expected = reclamation_ledger_idempotency_key(mk);
                if record.idempotency_key != Some(expected) {
                    return Err(MediumError::Chain {
                        rule: "reclamation-complete/audit-key-mismatch",
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
        // (A3) The gap-aware advance: a present parent follows
        // normally; an absent parent crosses ONLY under the most
        // recent authenticated skip table (the owner/self-skip
        // distinction preserved); an uncovered gap refuses.
        match medium.read_record(&parent) {
            Ok(_) => current = parent,
            Err(MediumError::MissingObject { .. }) => {
                let Some(table) = skip_table.as_ref() else {
                    return Err(MediumError::MissingObject { hash: parent });
                };
                let target = if table.sk == parent {
                    // The skip owner's own parent (the SK) is
                    // absent: the self-skip names the owner's
                    // nearest retained ancestor.
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
            Err(other) => return Err(other),
        }
    }
    let Some(ledger) = ledger else {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/audit-not-canonical",
        });
    };
    let Some(metadata) = metadata else {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/mk-not-a-checkpoint",
        });
    };
    // The bindings: the ledger's SK, digest, and seed agree with the
    // requested MK's permanent metadata.
    if ledger.sk != metadata.sk
        || ledger.range_digest != metadata.range_digest
        || ledger.seed != metadata.seed
    {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/binding-mismatch",
        });
    }
    // The units exactly match the retirement index (the record
    // identities, as a set — no omission, no extraneous entry).
    let index: std::collections::BTreeSet<Hash> =
        metadata.index.iter().map(|entry| entry.hash).collect();
    let units: std::collections::BTreeSet<Hash> =
        ledger.units.iter().map(|unit| unit.record).collect();
    if index.len() != metadata.index.len() || units.len() != ledger.units.len() {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/index-incoherent",
        });
    }
    if index != units {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/units-index-mismatch",
        });
    }
    // (A4) The pre-deletion manifest authority: every unit's record
    // is hash-verified by identity and its object list equals the
    // record's actual content references (the v1 collection
    // contract — a deletion manifest that omits or adds objects
    // refuses).
    for unit in &ledger.units {
        // (B1) The provider's RECORD-specific hash-verified read —
        // the cross-provider interface (SQLite stores revision
        // records in their own table; read_object is the
        // content-object namespace and would refuse a valid SQLite
        // audit whose records are absent from the objects table).
        // Both providers' read_record implementations load and
        // hash-verify the canonical record bytes.
        let record = medium.read_record(&unit.record)?;
        if unit.objects != record.content_refs {
            return Err(MediumError::Chain {
                rule: "reclamation-complete/units-objects-mismatch",
            });
        }
    }
    Ok((ledger, metadata))
}

// ------------------------------------------ the trusted completion (P2-F4)
//
// The verified completion receipt: the marker installed ONLY
// through [`publish_reclamation_complete`] — the in-crate admission
// (the generic append refuses the completion domain; the
// `AdmittedAppend` receipt is `pub(crate)`) — whose verification
// establishes the ACTUAL retirement state, never the caller's
// claim. The trusted consumer re-establishes the structural
// authority for readers; the admission-time state verification is
// the gate a marker-shaped object cannot bypass.

/// The recovery-safe audit authentication: the same descent,
/// bindings, and units-vs-index membership as
/// [`verify_canonical_v1_audit`] — but WITHOUT the pre-deletion
/// requirement that every unit's record is present. The completed
/// retirement's records are absent BY CONSTRUCTION; the absent set
/// must hold the (E1) deletion-prefix shape (an absent unit after a
/// present one is missing storage, never proof of a prior
/// authorized deletion), and the range digest is recomputed
/// whenever every digest member is present. A present unit's
/// manifest is still verified against its actual content
/// references.
pub fn verify_installed_v1_audit(
    medium: &mut dyn CanonicalMedium,
    audit_record: &Hash,
    mk: &Hash,
) -> Result<(ReclamationLedger, crate::checkpoint::CheckpointMetadata), MediumError> {
    const MAX_DESCENT: u64 = 1 << 20;
    let mut current = medium
        .head()?
        .ok_or(MediumError::Chain { rule: "unfounded" })?;
    let mut ledger: Option<ReclamationLedger> = None;
    let mut metadata: Option<crate::checkpoint::CheckpointMetadata> = None;
    let mut skip_table: Option<crate::checkpoint::CheckpointMetadata> = None;
    for _ in 0..MAX_DESCENT {
        let record = medium.read_record(&current)?;
        let parent = record.parent;
        if let Some(object) = record.content_refs.first().copied() {
            let bytes = medium.read_object(&object)?;
            let domain = StoredObject::new(bytes.clone())?.domain().to_owned();
            if domain == crate::checkpoint::CHECKPOINT_METADATA_DOMAIN {
                let decoded = crate::checkpoint::CheckpointMetadata::decode(&bytes)
                    .map_err(MediumError::Format)?;
                skip_table = Some(decoded.clone());
                if current == *mk {
                    if decoded.sk != parent.unwrap_or(Hash::ZERO) {
                        return Err(MediumError::Chain {
                            rule: "reclamation-complete/mk-binding-mismatch",
                        });
                    }
                    metadata = Some(decoded);
                }
            } else if domain == RECLAMATION_LEDGER_DOMAIN && current == *audit_record {
                let decoded = ReclamationLedger::decode(&bytes).map_err(MediumError::Format)?;
                if decoded.mk != *mk {
                    return Err(MediumError::Chain {
                        rule: "reclamation-complete/mk-mismatch",
                    });
                }
                let expected = reclamation_ledger_idempotency_key(mk);
                if record.idempotency_key != Some(expected) {
                    return Err(MediumError::Chain {
                        rule: "reclamation-complete/audit-key-mismatch",
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
        match medium.read_record(&parent) {
            Ok(_) => current = parent,
            Err(MediumError::MissingObject { .. }) => {
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
            Err(other) => return Err(other),
        }
    }
    let Some(ledger) = ledger else {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/audit-not-canonical",
        });
    };
    let Some(metadata) = metadata else {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/mk-not-a-checkpoint",
        });
    };
    if ledger.sk != metadata.sk
        || ledger.range_digest != metadata.range_digest
        || ledger.seed != metadata.seed
    {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/binding-mismatch",
        });
    }
    let index: std::collections::BTreeSet<Hash> =
        metadata.index.iter().map(|entry| entry.hash).collect();
    let units: std::collections::BTreeSet<Hash> =
        ledger.units.iter().map(|unit| unit.record).collect();
    if index.len() != metadata.index.len() || units.len() != ledger.units.len() {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/index-incoherent",
        });
    }
    if index != units {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/units-index-mismatch",
        });
    }
    // (E2) The newest-first order — the prefix property below
    // depends on it.
    verify_newest_first_order(&ledger.units, &metadata)?;
    // The founding-derived seed: ALWAYS verifiable (the founding is
    // permanent — the descent crosses the reclaimed gaps under the
    // authenticated skip tables).
    verify_founding_seed(&metadata, &founding_of(medium)?)?;
    // The (E1) recovery boundary + the digest recomputation: the
    // present units verify their manifests; the absent set must be
    // a deletion prefix; every digest member present triggers the
    // recomputation.
    let mut present_seen = false;
    let mut all_present = true;
    for unit in &ledger.units {
        match medium.read_record(&unit.record) {
            Ok(record) => {
                present_seen = true;
                if unit.objects != record.content_refs {
                    return Err(MediumError::Chain {
                        rule: "reclamation-complete/units-objects-mismatch",
                    });
                }
            }
            Err(MediumError::MissingObject { .. }) => {
                if present_seen {
                    return Err(MediumError::Chain {
                        rule: "reclaim/absence-not-deletion-prefix",
                    });
                }
                if unit.record != ledger.sk {
                    all_present = false;
                }
            }
            Err(other) => return Err(other),
        }
    }
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
            range_bytes.push(medium.read_record(&record)?.canonical_bytes());
        }
        let recomputed = crate::checkpoint::chain_range_digest(&metadata.seed, &range_bytes);
        if recomputed != ledger.range_digest {
            return Err(MediumError::Chain {
                rule: "reclaim/range-digest-mismatch",
            });
        }
    }
    Ok((ledger, metadata))
}

/// The completion postconditions over the live medium: every
/// retiring record absent, the partition recomputed from the
/// surviving chain, every mandatory identity absent, and every
/// deferred identity present and hash-verified. Returns the
/// verified (mandatory count, deferred set) the marker must carry.
pub fn verify_completion_state(
    medium: &mut dyn CanonicalMedium,
    ledger: &ReclamationLedger,
) -> Result<(u64, std::collections::BTreeSet<Hash>), MediumError> {
    for unit in &ledger.units {
        match medium.read_record(&unit.record) {
            Err(MediumError::MissingObject { .. }) => {}
            Ok(_) => {
                return Err(MediumError::Chain {
                    rule: "reclamation-complete/record-still-present",
                });
            }
            Err(other) => return Err(other),
        }
    }
    let exclusion: std::collections::BTreeSet<Hash> =
        ledger.units.iter().map(|unit| unit.record).collect();
    let survivors = survivor_objects(medium, &exclusion)?;
    let partition = partition_reclaim(&ledger.units, &survivors)?;
    for identity in &partition.mandatory {
        match medium.read_object(identity) {
            Err(MediumError::MissingObject { .. }) => {}
            Ok(_) => {
                return Err(MediumError::Chain {
                    rule: "reclamation-complete/mandatory-residue",
                });
            }
            Err(other) => return Err(other),
        }
    }
    for identity in &partition.deferred {
        // A deferred identity is a required surviving object: its
        // absence is a postcondition violation (the shared evidence
        // the marker certifies as preserved).
        medium.read_object(identity)?;
    }
    Ok((
        partition.mandatory.len() as u64,
        partition.deferred.iter().copied().collect(),
    ))
}

/// The keyed completion-marker discovery: the gap-aware descent
/// from the authoritative HEAD, returning the first record carrying
/// THIS mk's deterministic completion key with its strictly decoded
/// marker (the mk binding checked; everything else verified by
/// [`verify_reclamation_complete`]).
pub fn find_installed_completion(
    medium: &mut dyn CanonicalMedium,
    mk: &Hash,
) -> Result<Option<(Hash, ReclamationComplete)>, MediumError> {
    const MAX_DESCENT: u64 = 1 << 20;
    let key = reclamation_complete_idempotency_key(mk, Some(0));
    let mut current = medium
        .head()?
        .ok_or(MediumError::Chain { rule: "unfounded" })?;
    let mut skip_table: Option<crate::checkpoint::CheckpointMetadata> = None;
    for _ in 0..MAX_DESCENT {
        let record = medium.read_record(&current)?;
        if record.idempotency_key.as_deref() == Some(key.as_slice()) {
            let Some(object) = record.content_refs.first().copied() else {
                return Err(MediumError::Chain {
                    rule: "reclamation-complete/marker-malformed",
                });
            };
            let bytes = medium.read_object(&object)?;
            let marker = ReclamationComplete::decode(&bytes).map_err(MediumError::Format)?;
            if marker.mk != *mk {
                return Err(MediumError::Chain {
                    rule: "reclamation-complete/marker-mk-mismatch",
                });
            }
            return Ok(Some((current, marker)));
        }
        if let Some(object) = record.content_refs.first().copied() {
            let bytes = medium.read_object(&object)?;
            if StoredObject::new(bytes.clone())?.domain()
                == crate::checkpoint::CHECKPOINT_METADATA_DOMAIN
            {
                skip_table = Some(
                    crate::checkpoint::CheckpointMetadata::decode(&bytes)
                        .map_err(MediumError::Format)?,
                );
            }
        }
        let Some(parent) = record.parent else {
            return Ok(None);
        };
        match medium.read_record(&parent) {
            Ok(_) => current = parent,
            Err(MediumError::MissingObject { .. }) => {
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
            Err(other) => return Err(other),
        }
    }
    Ok(None)
}

/// The trusted completion consumer: the read-side authority for
/// maintenance and discovery decisions. (P2-R2) The verification is
/// INDEPENDENT of the writer's honesty: (1) CANONICAL MEMBERSHIP —
/// the marker revision must be ENCOUNTERED on the authoritative
/// HEAD ancestry (a hash-valid, keyed, correctly-sourced marker
/// that is not installed on the chain is not terminal evidence);
/// the same descent observes whether any LATER reclamation evidence
/// (an audit or a completion marker) stands above this marker —
/// the only mechanism that can shrink the survivor closure; (2) the
/// structural authority — the record's deterministic key, the
/// strict decode, the v1 boundary gates; (3) the recovery-safe
/// source-audit authentication; (4) the SEMANTIC RECHECK against
/// the live state — the retiring records absent (stable under every
/// legal concurrent operation) and, when this marker is the NEWEST
/// reclamation evidence, the FULL obligation equality (the
/// recomputed partition, the mandatory-count, the deferred set,
/// every mandatory identity absent, every deferred identity
/// present); when later COMPLETED retirements stand above (their
/// markers installed — the only mechanism that can have deleted
/// objects this batch deferred), each divergent claim is accounted
/// per-object against the later completed retirement's own audited
/// manifest: a later audit that never executed deletes nothing and
/// relaxes nothing. The precise provable claims are documented on
/// the semantic recheck below.
pub fn verify_reclamation_complete(
    medium: &mut dyn CanonicalMedium,
    marker_record: &Hash,
    mk: &Hash,
) -> Result<ReclamationComplete, MediumError> {
    // (1) The membership descent: walk the authoritative ancestry
    // until the marker record is encountered; collect the LATER
    // COMPLETION MARKERS above it (a completed retirement's
    // certificate — never a mere audit, which may never execute).
    const MAX_DESCENT: u64 = 1 << 20;
    let mut current = medium
        .head()?
        .ok_or(MediumError::Chain { rule: "unfounded" })?;
    let mut skip_table: Option<crate::checkpoint::CheckpointMetadata> = None;
    let mut later_markers: Vec<(Hash, ReclamationComplete)> = Vec::new();
    let mut installed = false;
    for _ in 0..MAX_DESCENT {
        if current == *marker_record {
            installed = true;
            break;
        }
        let record = medium.read_record(&current)?;
        if let Some(object) = record.content_refs.first().copied() {
            let bytes = medium.read_object(&object)?;
            let domain = StoredObject::new(bytes.clone())?.domain().to_owned();
            if domain == RECLAMATION_COMPLETE_DOMAIN {
                // A completion marker ABOVE the requested one: a
                // candidate completed later retirement. It is NOT
                // trusted yet — the accounting below verifies each
                // one (the key, the gates, the source audit, the
                // stable claims, and its own obligation equality)
                // BEFORE its manifest becomes attribution evidence.
                let decoded = ReclamationComplete::decode(&bytes).map_err(MediumError::Format)?;
                let expected =
                    reclamation_complete_idempotency_key(&decoded.mk, Some(decoded.batch));
                if record.idempotency_key != Some(expected) {
                    return Err(MediumError::Chain {
                        rule: "reclamation-complete/marker-key-mismatch",
                    });
                }
                later_markers.push((current, decoded));
            } else if domain == crate::checkpoint::CHECKPOINT_METADATA_DOMAIN {
                skip_table = Some(
                    crate::checkpoint::CheckpointMetadata::decode(&bytes)
                        .map_err(MediumError::Format)?,
                );
            }
        }
        let Some(parent) = record.parent else {
            break;
        };
        match medium.read_record(&parent) {
            Ok(_) => current = parent,
            Err(MediumError::MissingObject { .. }) => {
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
            Err(other) => return Err(other),
        }
    }
    if !installed {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/marker-not-installed",
        });
    }
    // (2) The structural authority.
    let record = medium.read_record(marker_record)?;
    let Some(object) = record.content_refs.first().copied() else {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/marker-malformed",
        });
    };
    let bytes = medium.read_object(&object)?;
    let marker = ReclamationComplete::decode(&bytes).map_err(MediumError::Format)?;
    if marker.mk != *mk {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/marker-mk-mismatch",
        });
    }
    if marker.kind == CompletionKind::Range {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/range-unauthorized",
        });
    }
    if marker.batch != 0 {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/batch-nonzero",
        });
    }
    let expected = reclamation_complete_idempotency_key(mk, Some(marker.batch));
    if record.idempotency_key != Some(expected) {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/marker-key-mismatch",
        });
    }
    // (3) The source audit's authority in the recovery-safe form:
    // the completed retirement's records are absent by construction.
    let (ledger, _metadata) = verify_installed_v1_audit(medium, &marker.source, &marker.mk)?;
    // (OC-C4) THE REQUESTED MARKER'S BEGIN AUTHORITY: the provider-
    // issued, MK/source-bound, durably-acknowledged ReclamationBegin
    // is required before any obligation accounting — a beginless,
    // corrupt, mismatched, or insufficient-grade begin refuses the
    // trusted certification (the provider's own typed rules
    // propagate: begin-authority/absent, binding-mismatch,
    // mk-mismatch, tier-insufficient). All existing semantic
    // postcondition checks below are preserved unchanged.
    verify_completion_begin_authority(medium, &marker.source, &marker.mk)?;
    // (4) The semantic recheck. The retiring records absent (the
    // records-table read — the cross-provider interface) is the
    // stable live claim; the full obligation equality applies when
    // this marker is the newest reclamation evidence; the superset
    // deletion-side check applies when later evidence stands above.
    for unit in &ledger.units {
        match medium.read_record(&unit.record) {
            Err(MediumError::MissingObject { .. }) => {}
            Ok(_) => {
                return Err(MediumError::Chain {
                    rule: "reclamation-complete/record-still-present",
                });
            }
            Err(other) => return Err(other),
        }
    }
    let exclusion: std::collections::BTreeSet<Hash> =
        ledger.units.iter().map(|unit| unit.record).collect();
    let survivors = survivor_objects(medium, &exclusion)?;
    let partition = partition_reclaim(&ledger.units, &survivors)?;
    for identity in &partition.mandatory {
        match medium.read_object(identity) {
            Err(MediumError::MissingObject { .. }) => {}
            Ok(_) => {
                return Err(MediumError::Chain {
                    rule: "reclamation-complete/mandatory-residue",
                });
            }
            Err(other) => return Err(other),
        }
    }
    // (P2-R2-H1) THE AUTHENTICATED CASCADE: each later marker is
    // verified — newest first, its own stable claims (the key and
    // gates checked at collection; the source audit; every retiring
    // record absent; every mandatory identity absent) plus its own
    // obligation equality against the coverage of the ALREADY-
    // VERIFIED newer markers — BEFORE its source-ledger manifest
    // becomes attribution evidence. A fabricated or malformed later
    // marker therefore refuses the whole verification (fail closed)
    // rather than relaxing an earlier marker's certification. No
    // circularity: each marker's accounting uses only the markers
    // ABOVE it.
    let mut later_covered: std::collections::BTreeSet<Hash> = std::collections::BTreeSet::new();
    for (_record_id, later_marker) in &later_markers {
        if later_marker.kind == CompletionKind::Range || later_marker.batch != 0 {
            return Err(MediumError::Chain {
                rule: "reclamation-complete/later-marker-unauthorized",
            });
        }
        let (later_ledger, _later_metadata) =
            verify_installed_v1_audit(medium, &later_marker.source, &later_marker.mk)?;
        // (OC-C4) THE LATER MARKER'S OWN BEGIN AUTHORITY: a beginless
        // later marker never relaxes an earlier certification — its
        // canonical membership, valid encoding, authenticated audit,
        // or apparent completion state never substitute for trusted
        // begin provenance. The provider-issued verification with the
        // eligible durability grade, before its manifest may
        // contribute any historical-attribution coverage.
        verify_completion_begin_authority(medium, &later_marker.source, &later_marker.mk)?;
        for unit in &later_ledger.units {
            match medium.read_record(&unit.record) {
                Err(MediumError::MissingObject { .. }) => {}
                Ok(_) => {
                    return Err(MediumError::Chain {
                        rule: "reclamation-complete/later-record-still-present",
                    });
                }
                Err(other) => return Err(other),
            }
        }
        let later_exclusion: std::collections::BTreeSet<Hash> =
            later_ledger.units.iter().map(|unit| unit.record).collect();
        let later_survivors = survivor_objects(medium, &later_exclusion)?;
        let later_partition = partition_reclaim(&later_ledger.units, &later_survivors)?;
        for identity in &later_partition.mandatory {
            match medium.read_object(identity) {
                Err(MediumError::MissingObject { .. }) => {}
                Ok(_) => {
                    return Err(MediumError::Chain {
                        rule: "reclamation-complete/later-mandatory-residue",
                    });
                }
                Err(other) => return Err(other),
            }
        }
        // The later marker's own obligation equality, against the
        // coverage accumulated from the newer verified markers.
        let mut later_accounted: std::collections::BTreeSet<Hash> =
            std::collections::BTreeSet::new();
        for unit in &later_ledger.units {
            for identity in unit.objects.iter().copied() {
                // The defensive set-membership invariant: only an
                // identity the recomputation actually classified
                // MANDATORY may be accounted (an identity outside the
                // reclassified set is attribution-incoherent).
                if later_covered.contains(&identity)
                    && later_partition.mandatory.contains(&identity)
                {
                    match medium.read_object(&identity) {
                        Err(MediumError::MissingObject { .. }) => {
                            later_accounted.insert(identity);
                        }
                        Ok(_) => {}
                        Err(other) => return Err(other),
                    }
                }
            }
        }
        let later_reconstructed_mandatory = later_partition
            .mandatory
            .len()
            .checked_sub(later_accounted.len())
            .ok_or(MediumError::Chain {
                rule: "reclamation-complete/attribution-incoherent",
            })?;
        if later_marker.mandatory_count != later_reconstructed_mandatory as u64
            || later_marker
                .deferred
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<Hash>>()
                != later_partition
                    .deferred
                    .iter()
                    .copied()
                    .chain(later_accounted.iter().copied())
                    .collect::<std::collections::BTreeSet<Hash>>()
        {
            return Err(MediumError::Chain {
                rule: "reclamation-complete/later-marker-incoherent",
            });
        }
        for identity in &later_partition.deferred {
            medium.read_object(identity)?;
        }
        // Verified: its manifest joins the attribution coverage.
        for unit in &later_ledger.units {
            later_covered.insert(unit.record);
            for object in &unit.objects {
                later_covered.insert(*object);
            }
        }
    }
    // (P2-R2-H2) THE TYPED ABSENCE + the correct namespace: the
    // accounted set contains only OBJECT identities (the content
    // namespace; the retiring records are covered by the stable
    // record-absent check and remain in the mandatory count), each
    // absence established ONLY by the authenticated MissingObject
    // result — corruption and I/O failures propagate, never
    // masquerading as deletion — and the arithmetic is CHECKED (an
    // accounted identity outside the recomputed mandatory set is an
    // inconsistency, never silently saturated).
    let mut accounted: std::collections::BTreeSet<Hash> = std::collections::BTreeSet::new();
    for unit in &ledger.units {
        for identity in unit.objects.iter().copied() {
            if later_covered.contains(&identity)
                && !accounted.contains(&identity)
                && partition.mandatory.contains(&identity)
            {
                match medium.read_object(&identity) {
                    Err(MediumError::MissingObject { .. }) => {
                        accounted.insert(identity);
                    }
                    Ok(_) => {}
                    Err(other) => return Err(other),
                }
            }
        }
    }
    let reconstructed_deferred: std::collections::BTreeSet<Hash> = partition
        .deferred
        .iter()
        .copied()
        .chain(accounted.iter().copied())
        .collect();
    let reconstructed_mandatory = partition
        .mandatory
        .len()
        .checked_sub(accounted.len())
        .ok_or(MediumError::Chain {
            rule: "reclamation-complete/attribution-incoherent",
        })?;
    if marker.mandatory_count != reconstructed_mandatory as u64 {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/mandatory-count-mismatch",
        });
    }
    if marker
        .deferred
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<Hash>>()
        != reconstructed_deferred
    {
        return Err(MediumError::Chain {
            rule: "reclamation-complete/deferred-mismatch",
        });
    }
    // The still-deferred identities remain present and hash-verified;
    // the accounted ones are absent BY the later verified
    // retirement's certified deletion.
    for identity in &partition.deferred {
        medium.read_object(identity)?;
    }
    Ok(marker)
}
