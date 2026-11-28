//! The reclamation admission and execution over transactions (split
//! from the sqlite monolith by change reason, P3-L9 pure move - the
//! design's sqlite row list has no home for this, the file's
//! dominant cohesive block): the transaction-scoped begin
//! admissions and authority verifications, the audit authentication
//! family, the authorized destructive reclaim, the post-uncertain
//! commit reconciliations, and the transaction-scoped BeginReads
//! adapter. These change when the verify-before-delete reclamation
//! semantics change.

use std::collections::BTreeSet;

use rusqlite::{Connection, params};

use crate::checkpoint::CheckpointMetadata;
use crate::hash::{Hash, hash_object};
use crate::medium::{MediumError, StoredObject, SyncTier};
use crate::record::RevisionRecord;

use super::meta::read_meta_head;
use super::rows::{insert_object_once, load_object_verified, load_record_verified, record_exists};
use super::walks::{validate_observed, walk_chain_excluding};
use super::{
    SqliteFaultPoint, SqliteMedium, fault_check, is_constraint, no_rows_as_none, sqlite_err,
};

/// (OC-C3 CP-1) The transaction-scoped installed-begin authority
/// verification: the recovery-safe audit authentication (no deleted
/// original retirement record required), the founding-derived seed,
/// the shared canonical begin verifier over ONE transaction-scoped
/// reader, and the SQLite durability grade — the read-snapshot
/// visibility proof under the verified WAL/FULL configuration (a
/// begin observed inside a fresh transaction was committed — the
/// exposure follows the synchronized WAL commit — preserving the
/// distinction between canonical membership and a previous
/// publication's acknowledged outcome).
pub(super) fn verify_begin_authority_tx(
    tx: &rusqlite::Transaction<'_>,
    audit: &Hash,
    mk: &Hash,
    tier: SyncTier,
) -> Result<crate::reclaim::BeginAuthority, MediumError> {
    let (ledger, metadata, head) = authenticate_audit_tx(tx, audit)?;
    if ledger.mk != *mk {
        return Err(MediumError::Chain {
            rule: "begin-authority/mk-mismatch",
        });
    }
    // The founding-derived seed (the E1 discipline — the
    // recovery-safe gap-aware descent).
    let founding = crate::reclaim::founding_identity(
        head,
        |identity| load_record_verified(tx, identity),
        |identity| load_object_verified(tx, identity),
        |identity| record_exists(tx, identity),
    )?;
    crate::reclaim::verify_founding_seed(&metadata, &founding)?;
    // The shared canonical begin verifier over ONE
    // transaction-scoped reader.
    struct SqliteBeginReads<'a, 'b> {
        tx: &'b rusqlite::Transaction<'a>,
    }
    impl crate::reclaim::BeginReads for SqliteBeginReads<'_, '_> {
        fn read_record_bytes(&mut self, id: &Hash) -> Result<Vec<u8>, MediumError> {
            load_record_verified(self.tx, id)
        }
        fn read_object_bytes(&mut self, hash: &Hash) -> Result<Vec<u8>, MediumError> {
            load_object_verified(self.tx, hash)
        }
    }
    let mut reads = SqliteBeginReads { tx };
    let Some((id, _begin)) =
        crate::reclaim::verify_installed_begin(&mut reads, head, audit, mk, &ledger)?
    else {
        return Err(MediumError::Chain {
            rule: "begin-authority/absent",
        });
    };
    Ok(crate::reclaim::BeginAuthority { id, tier })
}

/// The trusted reclamation internals (K2-C2B P2) — the transaction-
/// scoped authentication and execution (the same logical authority
/// contract as the local provider's locked form).
/// The OC-C2 trusted begin admission and publication under ONE
/// `BEGIN IMMEDIATE` transaction (the authoritative verification and
/// the publication coordinated; the transaction-scoped hash-verifying
/// reads; no public trait method, no nested transaction): the audit
/// authentication, the shared first-execution evidence core, the
/// duplicate resolution, and the insertion + the HEAD advance +
/// the COMMIT — all atomic (a rolled-back admission leaves nothing).
/// NOT combined with destructive deletion (OC-C3). The tier is the
/// provider's TransactionalWalFull.
/// The transaction-scoped begin discovery (the OC-C2-F3 corrective):
/// the gap-aware descent from the authoritative HEAD for the
/// deterministic key — proving the canonical membership by
/// construction — with the full revision-shape validation and the
/// strict decode. An unreferenced same-key row is never found (and
/// never authoritative).
pub(super) fn find_installed_begin_tx(
    tx: &rusqlite::Transaction<'_>,
    mk: &Hash,
    expected: &crate::reclaim::ReclamationBegin,
) -> Result<Option<(Hash, crate::reclaim::ReclamationBegin)>, MediumError> {
    const MAX_DESCENT: u64 = 1 << 20;
    let key = crate::reclaim::reclamation_begin_idempotency_key(mk);
    let mut current = read_meta_head(tx)?.ok_or(MediumError::Chain { rule: "unfounded" })?;
    let mut skip_table: Option<CheckpointMetadata> = None;
    for _ in 0..MAX_DESCENT {
        let bytes = load_record_verified(tx, &current)?;
        let record = RevisionRecord::decode(&bytes).map_err(MediumError::Format)?;
        if record.idempotency_key.as_deref() == Some(key.as_slice()) {
            let Some(parent) = record.parent else {
                return Err(MediumError::Chain {
                    rule: "begin-admission/installed-shape-mismatch",
                });
            };
            if !matches!(record.kind, crate::record::RecordKind::Regular)
                || record.content_refs.len() != 1
            {
                return Err(MediumError::Chain {
                    rule: "begin-admission/installed-shape-mismatch",
                });
            }
            // (C2-R3) The parent/epoch linkage: a present parent must
            // carry epoch+1; an absent parent must be an
            // authenticated skip-table gap.
            if record_exists(tx, &parent)? {
                let parent_bytes = load_record_verified(tx, &parent)?;
                let parent_record =
                    RevisionRecord::decode(&parent_bytes).map_err(MediumError::Format)?;
                // (C2-R7) Checked arithmetic: an epoch at u64::MAX has
                // no valid successor — saturation must never make an
                // incoherent linkage appear coherent.
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
            let object = record.content_refs[0];
            let object_bytes = load_object_verified(tx, &object)?;
            let installed = crate::reclaim::ReclamationBegin::decode(&object_bytes)
                .map_err(MediumError::Format)?;
            if installed.mk != *mk {
                return Err(MediumError::Chain {
                    rule: "begin-admission/installed-mk-mismatch",
                });
            }
            if installed != *expected {
                return Err(MediumError::Chain {
                    rule: "begin-admission/duplicate-content-mismatch",
                });
            }
            return Ok(Some((current, installed)));
        }
        if let Some(object) = record.content_refs.first().copied() {
            let object_bytes = load_object_verified(tx, &object)?;
            if StoredObject::new(object_bytes.clone())?.domain()
                == crate::checkpoint::CHECKPOINT_METADATA_DOMAIN
            {
                skip_table =
                    Some(CheckpointMetadata::decode(&object_bytes).map_err(MediumError::Format)?);
            }
        }
        let Some(parent) = record.parent else {
            return Ok(None);
        };
        if !record_exists(tx, &parent)? {
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

/// The admission's transaction body. Returns the publication outcome
/// AND the exact attempted candidate revision identity (Some only
/// when an installation was attempted) — the (C2-R5) identity the
/// uncertain-commit reconciliation must be bound to.
fn begin_reclamation_tx(
    tx: &rusqlite::Transaction<'_>,
    audit: &Hash,
    mk: &Hash,
    tick: u64,
) -> Result<(crate::reclaim::BeginPublicationOutcome, Option<Hash>), MediumError> {
    // (A) The exact canonical audit identity over the
    // transaction-scoped reads.
    let (ledger, metadata, _head) = authenticate_audit_tx(tx, audit)?;
    if ledger.mk != *mk {
        return Err(MediumError::Chain {
            rule: "begin-admission/mk-mismatch",
        });
    }
    // (F4) The gap-aware founding descent (the shared
    // reader-parameterized helper over the transaction-scoped
    // hash-verifying reads).
    let founding = crate::reclaim::founding_identity(
        read_meta_head(tx)?.ok_or(MediumError::Chain { rule: "unfounded" })?,
        |identity| load_record_verified(tx, identity),
        |identity| load_object_verified(tx, identity),
        |identity| record_exists(tx, identity),
    )?;
    // (B-D) The shared first-execution evidence core over the
    // transaction-scoped hash-verified record reads.
    let mut begin = crate::reclaim::verify_first_execution_evidence(
        &ledger,
        &metadata,
        &founding,
        |identity| load_record_verified(tx, identity),
        |identity| load_object_verified(tx, identity),
    )?;
    begin.source = *audit;
    // (F3) The duplicate resolution: the gap-aware HEAD-ANCESTRY
    // discovery (the idempotency index is a discovery aid, never
    // authority — an unreferenced same-key row is not a canonical
    // begin), the full revision-shape validation, and the exact
    // content verification.
    let key = crate::reclaim::reclamation_begin_idempotency_key(&begin.mk);
    if let Some((installed_id, _installed)) = find_installed_begin_tx(tx, mk, &begin)? {
        return Ok((
            crate::reclaim::BeginPublicationOutcome::DuplicateVerified { id: installed_id },
            None,
        ));
    }
    // The publication: the object row + the record row + the HEAD
    // advance — one atomic unit with the commit.
    let head = read_meta_head(tx)?.ok_or(MediumError::Chain { rule: "unfounded" })?;
    let parent_bytes = load_record_verified(tx, &head)?;
    let parent = RevisionRecord::decode(&parent_bytes).map_err(MediumError::Format)?;
    let parent_epoch = parent.epoch;
    parent_epoch.checked_add(1).ok_or(MediumError::Chain {
        rule: "epoch-regression",
    })?;
    let object = StoredObject::new(begin.canonical_bytes())?;
    let record = RevisionRecord::regular(head, parent_epoch, tick)
        .with_idempotency_key(key.clone())
        .with_content_refs(vec![object.hash()]);
    let record_bytes = record.canonical_bytes();
    let record_id = crate::hash::hash_object(&record_bytes);
    // (F2) The insert-once, verify-on-retry discipline (the
    // established helper — a preexisting row under the expected
    // identity is hash-verified before reuse; a corrupt row refuses
    // typed and is never overwritten).
    insert_object_once(tx, &None, &object)?;
    tx.execute(
        "INSERT INTO records (hash, bytes, epoch, parent, idem, kind, tick) \
         VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
        params![
            record_id.as_bytes(),
            record_bytes,
            record.epoch as i64,
            head.as_bytes(),
            key,
            record.tick as i64,
        ],
    )
    .map_err(|e| {
        // The same coherence guarantee as the normal admitted
        // publication: the chain-wide idempotency-key uniqueness is
        // the typed duplicate rule (an orphan squatting the key
        // refuses fail-closed; nothing installed).
        if is_constraint(&e) {
            MediumError::Chain {
                rule: "duplicate-idempotency-key",
            }
        } else {
            sqlite_err("begin record insert", e)
        }
    })?;
    tx.execute(
        "UPDATE meta SET value = ?1 WHERE key = 'head'",
        params![record_id.as_bytes()],
    )
    .map_err(|e| sqlite_err("begin head advance", e))?;
    Ok((
        crate::reclaim::BeginPublicationOutcome::Accepted {
            id: record_id,
            // (C2-R8) The truthful transactional tier: this value
            // surfaces ONLY after the wrapper's successful COMMIT
            // (the uncertain-commit paths divert to the read-only
            // reconciliation before returning it), under the
            // already-verified WAL/FULL configuration.
            tier: SyncTier::TransactionalWalFull,
        },
        Some(record_id),
    ))
}

/// (V2X-CP2) The transaction-scoped reader adapter — ONE
/// transaction backing both [`crate::reclaim::BeginReads`] kinds
/// through the established hash-verifying loads.
struct SqliteTxReads<'a, 'b> {
    tx: &'b rusqlite::Transaction<'a>,
}
impl crate::reclaim::BeginReads for SqliteTxReads<'_, '_> {
    fn read_record_bytes(&mut self, id: &Hash) -> Result<Vec<u8>, MediumError> {
        load_record_verified(self.tx, id)
    }
    fn read_object_bytes(&mut self, hash: &Hash) -> Result<Vec<u8>, MediumError> {
        load_object_verified(self.tx, hash)
    }
}

/// (V2X-CP2) The plan-bound V2 begin admission under ONE
/// `BEGIN IMMEDIATE` transaction: the ALL-AUDITS-FIRST CP1 contract
/// re-derived INSIDE the transaction over the transaction-scoped
/// hash-verifying reads (a pre-transaction verification never
/// substitutes); only the exact ExecutionReady positive admits; the
/// begin from the verified values only (the ratified plan-record
/// source semantics); the duplicate resolution's exact-content check
/// (the V1/V2 source-form exclusion); the atomic publication (the
/// object row + the record row + the HEAD advance + the COMMIT — a
/// rolled-back admission leaves nothing). The tier is the provider's
/// TransactionalWalFull, surfaced only after the wrapper's
/// successful COMMIT.
fn begin_reclamation_v2_tx(
    tx: &rusqlite::Transaction<'_>,
    mk: &Hash,
    tick: u64,
) -> Result<(crate::reclaim::BeginPublicationOutcome, Option<Hash>), MediumError> {
    // (A) The all-audits-first CP1 contract inside the transaction.
    let head = read_meta_head(tx)?.ok_or(MediumError::Chain { rule: "unfounded" })?;
    let mut reads = SqliteTxReads { tx };
    let (plan_record, range_digest, seed) =
        match crate::reclaim::verify_v2_execution_readiness_via(&mut reads, head, mk)? {
            crate::reclaim::V2ExecutionReadiness::ExecutionReady { plan_record, .. } => {
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
        };
    // (B) The begin from the verified values only.
    let begin = crate::reclaim::ReclamationBegin {
        mk: *mk,
        source: plan_record,
        range_digest,
        seed,
    };
    // (C) The duplicate resolution (the exact-content check IS the
    // V1/V2 source-form exclusion). A byte-identical installed
    // begin is DuplicateVerified ONLY when the installed begin's
    // own authority holds (V2X-CP2 F1): the strict temporal
    // position and the complete retained membership, re-proved
    // inside this same transaction — an adversarially installed
    // byte-identical begin above a later-published audit is a
    // typed temporal refusal, never a trusted duplicate.
    let key = crate::reclaim::reclamation_begin_idempotency_key(&begin.mk);
    if let Some((installed_id, _installed)) = find_installed_begin_tx(tx, mk, &begin)? {
        let mut reads = SqliteTxReads { tx };
        let dup_head = read_meta_head(tx)?.ok_or(MediumError::Chain { rule: "unfounded" })?;
        crate::reclaim::verify_v2_begin_authority_evidence_via(&mut reads, dup_head, mk)?;
        return Ok((
            crate::reclaim::BeginPublicationOutcome::DuplicateVerified { id: installed_id },
            None,
        ));
    }
    // (D) The atomic publication.
    let head = read_meta_head(tx)?.ok_or(MediumError::Chain { rule: "unfounded" })?;
    let parent_bytes = load_record_verified(tx, &head)?;
    let parent = RevisionRecord::decode(&parent_bytes).map_err(MediumError::Format)?;
    let parent_epoch = parent.epoch;
    parent_epoch.checked_add(1).ok_or(MediumError::Chain {
        rule: "epoch-regression",
    })?;
    let object = StoredObject::new(begin.canonical_bytes())?;
    let record = RevisionRecord::regular(head, parent_epoch, tick)
        .with_idempotency_key(key.clone())
        .with_content_refs(vec![object.hash()]);
    let record_bytes = record.canonical_bytes();
    let record_id = crate::hash::hash_object(&record_bytes);
    insert_object_once(tx, &None, &object)?;
    tx.execute(
        "INSERT INTO records (hash, bytes, epoch, parent, idem, kind, tick) \
         VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
        params![
            record_id.as_bytes(),
            record_bytes,
            record.epoch as i64,
            head.as_bytes(),
            key,
            record.tick as i64,
        ],
    )
    .map_err(|e| {
        if is_constraint(&e) {
            MediumError::Chain {
                rule: "duplicate-idempotency-key",
            }
        } else {
            sqlite_err("v2 begin record insert", e)
        }
    })?;
    tx.execute(
        "UPDATE meta SET value = ?1 WHERE key = 'head'",
        params![record_id.as_bytes()],
    )
    .map_err(|e| sqlite_err("v2 begin head advance", e))?;
    Ok((
        crate::reclaim::BeginPublicationOutcome::Accepted {
            id: record_id,
            tier: SyncTier::TransactionalWalFull,
        },
        Some(record_id),
    ))
}

/// (V2X-CP3-B-P2A-C5b) The transaction-scoped coordinated
/// observation: the authoritative head read from the meta table
/// INSIDE the caller's transaction, then the accepted C5a
/// observation walk over the transaction-scoped hash-verifying
/// reads — the head read and the whole walk inside ONE snapshot.
/// READ-ONLY. An unfounded campaign refuses typed.
pub(super) fn observe_range_terminal_tx(
    tx: &rusqlite::Transaction<'_>,
    expected: &crate::range_completion::RangeCompletionContext,
    limits: &crate::range_installation::RangeInstallationLimits,
) -> Result<crate::range_installation::CoordinatedObservation, MediumError> {
    let coordinated_head = read_meta_head(tx)?.ok_or(MediumError::Chain { rule: "unfounded" })?;
    let mut reads = SqliteTxReads { tx };
    crate::range_installation::observe_coordinated(&mut reads, &coordinated_head, expected, limits)
}

/// (V2X-CP2) The transaction-scoped retained-evidence V2 begin
/// authority: the recovery-safe verification (no original retiring
/// record required) over ONE transaction-scoped reader, and the
/// SQLite durability grade — the read-snapshot visibility proof
/// under the verified WAL/FULL configuration.
pub(super) fn verify_begin_authority_v2_tx(
    tx: &rusqlite::Transaction<'_>,
    mk: &Hash,
    tier: SyncTier,
) -> Result<crate::reclaim::BeginAuthority, MediumError> {
    let head = read_meta_head(tx)?.ok_or(MediumError::Chain { rule: "unfounded" })?;
    let mut reads = SqliteTxReads { tx };
    let evidence = crate::reclaim::verify_v2_begin_authority_evidence_via(&mut reads, head, mk)?;
    Ok(crate::reclaim::BeginAuthority {
        id: evidence.begin_id,
        tier,
    })
}

pub(super) fn reclaim_authorized_tx(
    tx: &rusqlite::Transaction<'_>,
    directive: &crate::reclaim::ReclaimDirective,
) -> Result<crate::reclaim::ReclaimOutcome, MediumError> {
    let (ledger, metadata, head) = authenticate_audit_tx(tx, &directive.audit_record)?;
    if ledger.units != directive.units {
        return Err(MediumError::Chain {
            rule: "reclaim/directive-units-mismatch",
        });
    }
    // (E2) The newest-first retirement order.
    crate::reclaim::verify_newest_first_order(&ledger.units, &metadata)?;
    // The manifest reread with the resume tolerance (the records-
    // table read; an absent retiring record is the protocol-
    // accounted resume — but ONLY in the deletion-prefix shape: the
    // (E1) recovery boundary. This provider's pass 1 deletes
    // newest-first under one transaction, so a GENUINE process
    // interruption rolls back to the all-present state — a
    // prefix-shaped absence here is always external degradation,
    // currently accepted under the SAME unresolved recovery-policy
    // question as the file provider (the owner decision PENDING —
    // Options A/B/C, none ratified; no durable begin-record exists
    // on this provider, whose atomic boundary never produces the
    // partial shape itself). A non-prefix pattern is evidence of
    // missing storage, never proof of a prior authorized deletion —
    // the fabricated-audit-with-missing-data shape fails closed).
    let mut present_seen = false;
    for unit in &ledger.units {
        if record_exists(tx, &unit.record)? {
            present_seen = true;
            let bytes = load_record_verified(tx, &unit.record)?;
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
    // inside the SAME provider transaction, before any deletion:
    // the accepted CP-1 transaction-scoped verification helper (the
    // recovery-safe authentication, the founding seed, the canonical
    // verifier, the TransactionalWalFull grade from the read
    // snapshot's visibility proof — never a separate unlocked
    // advisory result). The DEFINITE canonical absence is the
    // crossing's fresh-admission trigger; every corrupt,
    // mismatched, orphaned, or unverifiable state is its own typed
    // refusal — never absence.
    match verify_begin_authority_tx(
        tx,
        &directive.audit_record,
        &ledger.mk,
        SyncTier::TransactionalWalFull,
    ) {
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
    // The survivor partition under the same transaction — and the
    // E1 range commitments: the founding-derived seed (always
    // verifiable) and, on the FIRST execution (every range
    // publication present), the recomputed range digest.
    let exclusion: BTreeSet<Hash> = ledger.units.iter().map(|unit| unit.record).collect();
    let walked = walk_chain_excluding(tx, head, &exclusion)?;
    crate::reclaim::verify_founding_seed(&metadata, &walked.cursor.founding)?;
    let mut all_present = true;
    for unit in &ledger.units {
        if unit.record != ledger.sk && !record_exists(tx, &unit.record)? {
            all_present = false;
            break;
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
            range_bytes.push(load_record_verified(tx, &record)?);
        }
        let recomputed = crate::checkpoint::chain_range_digest(&metadata.seed, &range_bytes);
        if recomputed != ledger.range_digest {
            return Err(MediumError::Chain {
                rule: "reclaim/range-digest-mismatch",
            });
        }
    }
    let survivors = walked.cursor.reachable;
    let mut partition = crate::reclaim::partition_reclaim(&ledger.units, &survivors)?;
    let mut removed = 0u64;
    // PASS 1 — the retiring records, newest-first (the ledger's unit
    // order), idempotent.
    for unit in &ledger.units {
        let count = tx
            .execute(
                "DELETE FROM records WHERE hash = ?1",
                params![unit.record.as_bytes()],
            )
            .map_err(|e| sqlite_err("delete record", e))?;
        removed += count as u64;
    }
    // PASS 2 — the deduplicated MANDATORY objects only (the deferred
    // identities never touched). (E3) The content-integrity
    // verification: each PRESENT row's stored bytes are read and
    // hash-verified INSIDE the transaction before deletion — a
    // corrupted row under an audited hash is PRESERVED evidence
    // (the typed integrity refusal rolls the whole transaction
    // back); an absent row is the resume (already reclaimed).
    for identity in &partition.mandatory {
        let bytes: Option<Vec<u8>> = tx
            .query_row(
                "SELECT bytes FROM objects WHERE hash = ?1",
                params![identity.as_bytes()],
                |row| row.get(0),
            )
            .map(Some)
            .or_else(no_rows_as_none)
            .map_err(|e| sqlite_err("probe object before delete", e))?;
        let Some(bytes) = bytes else {
            continue;
        };
        if hash_object(&bytes) != *identity {
            return Err(MediumError::ReferencedObjectCorrupt { hash: *identity });
        }
        let count = tx
            .execute(
                "DELETE FROM objects WHERE hash = ?1",
                params![identity.as_bytes()],
            )
            .map_err(|e| sqlite_err("delete object", e))?;
        removed += count as u64;
    }
    partition.removed = removed;
    Ok(partition)
}

/// The transaction-scoped canonical audit authentication (the same
/// discipline as the local provider's locked form: the exact audit
/// identity via the hash-verified load, the MK from the decoded
/// ledger, the deterministic key, the requested MK's metadata bound
/// by revision identity, the bindings, and the units-vs-index
/// membership — the recovery-safe form without the per-unit
/// reread).
fn authenticate_audit_tx(
    tx: &rusqlite::Transaction<'_>,
    audit_record: &Hash,
) -> Result<
    (
        crate::reclaim::ReclamationLedger,
        crate::checkpoint::CheckpointMetadata,
        Hash,
    ),
    MediumError,
> {
    authenticate_audit_on(tx, audit_record)
}

#[allow(dead_code)]
fn authenticate_audit_tx_unused(
    tx: &rusqlite::Transaction<'_>,
    audit_record: &Hash,
) -> Result<
    (
        crate::reclaim::ReclamationLedger,
        crate::checkpoint::CheckpointMetadata,
        Hash,
    ),
    MediumError,
> {
    const MAX_DESCENT: u64 = 1 << 20;
    let mut current = read_meta_head(tx)?.ok_or(MediumError::Chain { rule: "unfounded" })?;
    let head = current;
    let mut ledger: Option<crate::reclaim::ReclamationLedger> = None;
    let mut metadata: Option<crate::checkpoint::CheckpointMetadata> = None;
    let mut skip_table: Option<crate::checkpoint::CheckpointMetadata> = None;
    for _ in 0..MAX_DESCENT {
        let bytes = load_record_verified(tx, &current)?;
        let record = RevisionRecord::decode(&bytes).map_err(MediumError::Format)?;
        let parent = record.parent;
        if let Some(object) = record.content_refs.first().copied() {
            let object_bytes = load_object_verified(tx, &object)?;
            let domain = StoredObject::new(object_bytes.clone())?.domain().to_owned();
            if domain == crate::checkpoint::CHECKPOINT_METADATA_DOMAIN {
                let decoded = crate::checkpoint::CheckpointMetadata::decode(&object_bytes)
                    .map_err(MediumError::Format)?;
                skip_table = Some(decoded.clone());
                if let Some(ledger) = ledger.as_ref()
                    && current == ledger.mk
                {
                    if decoded.sk != parent.unwrap_or(Hash::ZERO) {
                        return Err(MediumError::Chain {
                            rule: "reclaim/mk-binding-mismatch",
                        });
                    }
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
        if record_exists(tx, &parent)? {
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
    if index.len() != metadata.index.len() || units.len() != ledger.units.len() || index != units {
        return Err(MediumError::Chain {
            rule: "reclaim/units-index-mismatch",
        });
    }
    Ok((ledger, metadata, head))
}

/// The Mandatory-only probe over the plain connection (the advisory
/// read; the same authentication and partition).
pub(super) fn reclaim_pending_authorized(
    conn: &Connection,
    directive: &crate::reclaim::ReclaimDirective,
) -> Result<bool, MediumError> {
    let (ledger, _metadata, head) = authenticate_audit_conn(conn, &directive.audit_record)?;
    if ledger.units != directive.units {
        return Err(MediumError::Chain {
            rule: "reclaim/directive-units-mismatch",
        });
    }
    let exclusion: BTreeSet<Hash> = ledger.units.iter().map(|unit| unit.record).collect();
    let walked = walk_chain_excluding(conn, head, &exclusion)?;
    let survivors = walked.cursor.reachable;
    let partition = crate::reclaim::partition_reclaim(&ledger.units, &survivors)?;
    for identity in &partition.mandatory {
        let present: i64 = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM objects WHERE hash = ?1 UNION ALL                  SELECT 1 FROM records WHERE hash = ?1 LIMIT 1)",
                params![identity.as_bytes()],
                |row| row.get(0),
            )
            .map_err(|e| sqlite_err("probe mandatory identity", e))?;
        if present != 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The plain-connection variant of the audit authentication (the
/// probe path — the same checks over the unlocked reads).
fn authenticate_audit_conn(
    conn: &Connection,
    audit_record: &Hash,
) -> Result<
    (
        crate::reclaim::ReclamationLedger,
        crate::checkpoint::CheckpointMetadata,
        Hash,
    ),
    MediumError,
> {
    // The identical discipline over the plain connection: reuse the
    // transaction form through the connection's own implicit read
    // consistency (SQLite's connection-scoped snapshot).
    authenticate_audit_on(conn, audit_record)
}

/// The shared descent over ANY connection or transaction handle.
fn authenticate_audit_on(
    conn: &Connection,
    audit_record: &Hash,
) -> Result<
    (
        crate::reclaim::ReclamationLedger,
        crate::checkpoint::CheckpointMetadata,
        Hash,
    ),
    MediumError,
> {
    const MAX_DESCENT: u64 = 1 << 20;
    let mut current = read_meta_head(conn)?.ok_or(MediumError::Chain { rule: "unfounded" })?;
    let head = current;
    let mut ledger: Option<crate::reclaim::ReclamationLedger> = None;
    let mut metadata: Option<crate::checkpoint::CheckpointMetadata> = None;
    let mut skip_table: Option<crate::checkpoint::CheckpointMetadata> = None;
    for _ in 0..MAX_DESCENT {
        let bytes = load_record_verified(conn, &current)?;
        let record = RevisionRecord::decode(&bytes).map_err(MediumError::Format)?;
        let parent = record.parent;
        if let Some(object) = record.content_refs.first().copied() {
            let object_bytes = load_object_verified(conn, &object)?;
            let domain = StoredObject::new(object_bytes.clone())?.domain().to_owned();
            if domain == crate::checkpoint::CHECKPOINT_METADATA_DOMAIN {
                let decoded = crate::checkpoint::CheckpointMetadata::decode(&object_bytes)
                    .map_err(MediumError::Format)?;
                skip_table = Some(decoded.clone());
                if let Some(ledger) = ledger.as_ref()
                    && current == ledger.mk
                {
                    if decoded.sk != parent.unwrap_or(Hash::ZERO) {
                        return Err(MediumError::Chain {
                            rule: "reclaim/mk-binding-mismatch",
                        });
                    }
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
        if record_exists(conn, &parent)? {
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
    if index.len() != metadata.index.len() || units.len() != ledger.units.len() || index != units {
        return Err(MediumError::Chain {
            rule: "reclaim/units-index-mismatch",
        });
    }
    Ok((ledger, metadata, head))
}

// ------------------------------------------------------------ the inserts
// ------------------------------------------------------------ the inserts

/// The OC-C2 trusted begin admission (the transactional twin — see
/// the free `begin_reclamation_tx`): ONE BEGIN IMMEDIATE covering
/// the authentication, the evidence core, the duplicate resolution,
/// and the insertion + the HEAD advance; the uncertain-commit
/// discipline (never a false confirmation).
impl SqliteMedium {
    pub fn begin_reclamation(
        &mut self,
        audit: &Hash,
        mk: &Hash,
        tick: u64,
    ) -> Result<crate::reclaim::BeginPublicationOutcome, MediumError> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| sqlite_err("begin admission transaction", e))?;
        // (C2-R6) The authoritative-chain validation — the SAME
        // transaction-scoped trusted-cursor discipline the normal
        // publication applies (O(0) on the verified head, a bounded
        // suffix walk on divergence, the cold walk otherwise): a
        // chain carrying an otherwise authentic audit and retirement
        // evidence but malformed revisions above the audit never
        // receives a trusted begin — no duplicate outcome, no
        // installation.
        let observed = read_meta_head(&tx)?;
        if let Err(error) = validate_observed(&tx, &mut self.cursor, &mut self.walks, observed) {
            let _ = tx.rollback();
            return Err(error);
        }
        let (outcome, candidate) = begin_reclamation_tx(&tx, audit, mk, tick)?;
        // (C2-R1) The fault-wired commit boundary (the established
        // SqliteFaultPoint infrastructure — the same points the
        // normal append path uses): an injected CommitFailure
        // simulates the pre-commit rollback (a DEFINITE absence —
        // distinguished from a genuinely ambiguous real commit); an
        // injected AfterCommit simulates the
        // installed-but-acknowledgment-lost window; a real COMMIT
        // error is the genuine uncertainty. All three reconcile
        // through the READ-ONLY observation — never a second
        // publication inside the reconciliation.
        let uncertain =
            if let Err(_injected) = fault_check(&self.fault, SqliteFaultPoint::CommitFailure) {
                let _ = tx.rollback();
                true
            } else if tx.commit().is_err() {
                true
            } else {
                fault_check(&self.fault, SqliteFaultPoint::AfterCommit).is_err()
            };
        if uncertain {
            // (C2-R5) The reconciliation is bound to the EXACT
            // attempted candidate revision identity.
            return reconcile_begin_publication(self, audit, mk, candidate);
        }
        Ok(outcome)
    }

    /// (V2X-CP2) The plan-bound V2 begin admission — the trusted
    /// transactional wrapper: ONE `BEGIN IMMEDIATE` covering the
    /// all-audits-first verification and the publication (the C2-R6
    /// authoritative-chain validation first, the accepted V1
    /// wrapper's discipline); the honest COMMIT uncertainty (the
    /// injected CommitFailure = the definite pre-commit rollback;
    /// the injected AfterCommit = the installed-but-acknowledgment-
    /// lost window; a real COMMIT error = the genuine uncertainty)
    /// reconciled through the READ-ONLY V2 observation — never a
    /// second publication inside the reconciliation.
    pub fn begin_reclamation_v2(
        &mut self,
        mk: &Hash,
        tick: u64,
    ) -> Result<crate::reclaim::BeginPublicationOutcome, MediumError> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| sqlite_err("v2 begin admission transaction", e))?;
        let observed = read_meta_head(&tx)?;
        if let Err(error) = validate_observed(&tx, &mut self.cursor, &mut self.walks, observed) {
            let _ = tx.rollback();
            return Err(error);
        }
        let (outcome, candidate) = begin_reclamation_v2_tx(&tx, mk, tick)?;
        let uncertain =
            if let Err(_injected) = fault_check(&self.fault, SqliteFaultPoint::CommitFailure) {
                let _ = tx.rollback();
                true
            } else if tx.commit().is_err() {
                true
            } else {
                fault_check(&self.fault, SqliteFaultPoint::AfterCommit).is_err()
            };
        if uncertain {
            return reconcile_begin_publication_v2(self, mk, candidate);
        }
        Ok(outcome)
    }
}

/// (V2X-CP2) The post-uncertain-commit V2 reconciliation: a
/// READ-ONLY fresh observation (never publishes, never inserts,
/// never constructs a replacement candidate) — the authoritative
/// chain validated under the same trusted-cursor discipline, the
/// expected V2 begin re-derived from the RETAINED evidence (the
/// checkpoint metadata and the installed plan — the read-only
/// forms), then the gap-aware HEAD-ancestry discovery bound to the
/// EXACT attempted candidate identity: the candidate itself
/// canonically present is ReconciledInstalled (the membership
/// proof, never Accepted, never a durability claim); a DIFFERENT
/// same-key revision is DuplicateVerified ONLY under its OWN
/// complete retained-authority proof (V2X-CP2 F3: the strict
/// temporal position, the complete retained membership, and the
/// source binding, re-proved over the same read-only transaction
/// with the verifier's begin identity exactly the observed
/// revision — the payload match alone is never trusted completion;
/// a failure is the appropriate typed refusal or the unverifiable
/// classification); a definite absence of any same-key begin is
/// ReconciledNotInstalled; any discovery failure is the typed
/// unverifiable uncertainty.
pub(super) fn reconcile_begin_publication_v2(
    medium: &mut SqliteMedium,
    mk: &Hash,
    candidate: Option<Hash>,
) -> Result<crate::reclaim::BeginPublicationOutcome, MediumError> {
    let tx = medium
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| MediumError::Chain {
            rule: "v2-begin-admission/reconcile-unverifiable",
        })?;
    // (READ-ONLY) The authoritative-chain validation.
    let observed = read_meta_head(&tx)?;
    if observed.is_none()
        || validate_observed(&tx, &mut medium.cursor, &mut medium.walks, observed).is_err()
    {
        return Err(MediumError::Chain {
            rule: "v2-begin-admission/reconcile-unverifiable",
        });
    }
    // (READ-ONLY) The expected begin re-derivation from the retained
    // evidence: the checkpoint metadata and the installed plan.
    let head = match observed {
        Some(head) => head,
        None => {
            return Err(MediumError::Chain {
                rule: "v2-begin-admission/reconcile-unverifiable",
            });
        }
    };
    let mut reads = SqliteTxReads { tx: &tx };
    let expected = match crate::reclaim::find_on_chain_via(&mut reads, head, None, Some(mk)) {
        Ok(Some((_id, meta_bytes))) => match CheckpointMetadata::decode(&meta_bytes) {
            Ok(_metadata) => match crate::reclaim::find_installed_plan_via(&mut reads, head, mk) {
                Ok(Some((plan_record, plan))) => Some(crate::reclaim::ReclamationBegin {
                    mk: *mk,
                    source: plan_record,
                    range_digest: plan.range_digest,
                    seed: plan.seed,
                }),
                _ => None,
            },
            Err(_) => None,
        },
        _ => None,
    };
    let Some(expected) = expected else {
        return Err(MediumError::Chain {
            rule: "v2-begin-admission/reconcile-unverifiable",
        });
    };
    match find_installed_begin_tx(&tx, mk, &expected) {
        Ok(Some((id, _installed))) => {
            if candidate == Some(id) {
                Ok(crate::reclaim::BeginPublicationOutcome::ReconciledInstalled { id })
            } else {
                // (V2X-CP2 F3) A DIFFERENT same-key revision is
                // DuplicateVerified ONLY when its OWN retained
                // authority holds: the complete strict temporal,
                // retained-membership, and source-binding proof over
                // the SAME read-only transaction snapshot (no nested
                // transaction — the reader borrows this transaction),
                // with the verifier's begin identity exactly the
                // observed same-key revision. The payload match
                // alone (the expected-form equality above) is never
                // trusted completion: a verification failure is the
                // appropriate typed refusal (or the unverifiable
                // classification when the observed revision is not
                // the proven authority), never DuplicateVerified.
                let mut authority_reads = SqliteTxReads { tx: &tx };
                match crate::reclaim::verify_v2_begin_authority_evidence_via(
                    &mut authority_reads,
                    head,
                    mk,
                ) {
                    Ok(evidence) if evidence.begin_id == id => {
                        Ok(crate::reclaim::BeginPublicationOutcome::DuplicateVerified { id })
                    }
                    Ok(_) => Err(MediumError::Chain {
                        rule: "v2-begin-admission/reconcile-unverifiable",
                    }),
                    Err(error) => Err(error),
                }
            }
        }
        Ok(None) => Ok(crate::reclaim::BeginPublicationOutcome::ReconciledNotInstalled),
        Err(_) => Err(MediumError::Chain {
            rule: "v2-begin-admission/reconcile-unverifiable",
        }),
    }
}

/// The post-uncertain-commit reconciliation (the C2-R1/R5
/// corrective): a READ-ONLY fresh observation — it never publishes,
/// never inserts, never constructs a replacement candidate. It
/// validates the authoritative chain under the same trusted-cursor
/// discipline (a concurrent writer may have changed the state
/// between the attempt and this observation), re-derives the
/// expected begin from the (read-only) evidence verification, then
/// the gap-aware HEAD-ancestry discovery — all bound to the EXACT
/// attempted candidate identity: the candidate itself canonically
/// present is ReconciledInstalled (the membership proof, never
/// Accepted, never a durability claim); a definite absence of any
/// same-key begin is ReconciledNotInstalled (an EXPLICIT later retry
/// may revalidate and construct a new candidate — a separate
/// operation, never this one); a DIFFERENT same-key revision is
/// separately discovered, verified duplicate evidence
/// (DuplicateVerified) — never misclassified as the installation of
/// the attempted candidate; any failure is the typed unverifiable
/// uncertainty.
pub(super) fn reconcile_begin_publication(
    medium: &mut SqliteMedium,
    audit: &Hash,
    mk: &Hash,
    candidate: Option<Hash>,
) -> Result<crate::reclaim::BeginPublicationOutcome, MediumError> {
    let tx = medium
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| MediumError::Chain {
            rule: "begin-admission/reconcile-unverifiable",
        })?;
    // (C2-R6, READ-ONLY) The authoritative-chain validation — the
    // same trusted-cursor discipline, inside the observation's own
    // snapshot.
    let observed = read_meta_head(&tx)?;
    if validate_observed(&tx, &mut medium.cursor, &mut medium.walks, observed).is_err() {
        return Err(MediumError::Chain {
            rule: "begin-admission/reconcile-unverifiable",
        });
    }
    // (READ-ONLY) The expected begin re-derivation — the
    // authentication, the founding, the evidence core: all reads.
    let (ledger, metadata, _head) = match authenticate_audit_tx(&tx, audit) {
        Ok(triple) => triple,
        Err(_) => {
            return Err(MediumError::Chain {
                rule: "begin-admission/reconcile-unverifiable",
            });
        }
    };
    if ledger.mk != *mk {
        return Err(MediumError::Chain {
            rule: "begin-admission/reconcile-unverifiable",
        });
    }
    let founding = match crate::reclaim::founding_identity(
        read_meta_head(&tx)?.ok_or(MediumError::Chain { rule: "unfounded" })?,
        |identity| load_record_verified(&tx, identity),
        |identity| load_object_verified(&tx, identity),
        |identity| record_exists(&tx, identity),
    ) {
        Ok(founding) => founding,
        Err(_) => {
            return Err(MediumError::Chain {
                rule: "begin-admission/reconcile-unverifiable",
            });
        }
    };
    let mut expected = match crate::reclaim::verify_first_execution_evidence(
        &ledger,
        &metadata,
        &founding,
        |identity| load_record_verified(&tx, identity),
        |identity| load_object_verified(&tx, identity),
    ) {
        Ok(begin) => begin,
        Err(_) => {
            return Err(MediumError::Chain {
                rule: "begin-admission/reconcile-unverifiable",
            });
        }
    };
    expected.source = *audit;
    // (READ-ONLY, C2-R5) The discovery bound to the exact attempted
    // candidate: the candidate itself present → the membership proof;
    // a DIFFERENT same-key revision → the separately discovered,
    // verified duplicate evidence (never ReconciledInstalled for the
    // original attempt); a definite absence → ReconciledNotInstalled.
    match find_installed_begin_tx(&tx, mk, &expected) {
        Ok(Some((id, _installed))) => {
            if candidate == Some(id) {
                Ok(crate::reclaim::BeginPublicationOutcome::ReconciledInstalled { id })
            } else {
                Ok(crate::reclaim::BeginPublicationOutcome::DuplicateVerified { id })
            }
        }
        Ok(None) => Ok(crate::reclaim::BeginPublicationOutcome::ReconciledNotInstalled),
        Err(_) => Err(MediumError::Chain {
            rule: "begin-admission/reconcile-unverifiable",
        }),
    }
}
