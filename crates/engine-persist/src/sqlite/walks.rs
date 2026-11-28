//! The trusted-cursor validation walks (split from the sqlite
//! monolith by change reason, P3-L9 pure move): the SqliteCursor,
//! the cold chain walk with the PP-B3 gap-aware skip descent, the
//! observed-HEAD validation dispatch, the bounded suffix walk with
//! its verified composite closure, and the cursor advances. These
//! change when the chain-validation rules change; the provider's
//! own walks stay provider-local by design - the local/sqlite walk
//! unification is deliberately NOT this lane (the orchestrator's
//! follow-up; the observed duplication is recorded in the lane
//! report).

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::Connection;

use crate::checkpoint::{
    CHECKPOINT_METADATA_DOMAIN, CheckpointMetadata, ReclaimedIndexEntry, peek_object_domain,
};
use crate::composite_manifest;
use crate::hash::Hash;
use crate::local::WalkStats;
use crate::medium::{ChainSummary, MediumError};
use crate::record::{AuthorityMode, RecordKind, RevisionRecord};

use super::rows::{load_referenced_object, load_referenced_record, record_exists};

// ------------------------------------------------------------- the cursor

/// The session-scoped trusted cursor (the §5.3.2 model — identical
/// semantics to the file medium's).
pub(super) struct SqliteCursor {
    pub(super) head: Hash,
    pub(super) epoch: u64,
    pub(super) seen_keys: BTreeSet<Vec<u8>>,
    pub(super) reachable: BTreeSet<Hash>,
    pub(super) record_ids: BTreeSet<Hash>,
    /// The reclaim-indexed revisions (identity → epoch) from every
    /// on-chain MK's index whose records are genuinely absent — the
    /// historical membership evidence (PP-B3's condition 2). The
    /// observational counts (the MK list, the reclaimed totals) live
    /// on the [`ChainSummary`] the walk returns, never on the cursor.
    pub(super) reclaimed: BTreeMap<Hash, u64>,
    pub(super) founding: Hash,
    pub(super) authority: AuthorityMode,
    pub(super) campaign_id: String,
    pub(super) content_objects: u64,
}

/// A completed cold walk: the summary plus the cursor it establishes.
pub(super) struct WalkedChain {
    pub(super) summary: ChainSummary,
    pub(super) cursor: SqliteCursor,
}

// --------------------------------------------------------------- the medium
/// MK's skip table covers the gap — a skip for exactly the record
/// whose parent is absent (or that MK's own self entry), jumping to
/// the nearest retained ancestor under the gap-aware
/// strictly-earlier epoch linkage. An uncovered absence is hard
/// corruption — fail closed, never assumed reclamation.
pub(super) fn walk_chain(conn: &Connection, head: Hash) -> Result<WalkedChain, MediumError> {
    walk_chain_excluding(conn, head, &BTreeSet::new())
}

/// The authenticated virtual-exclusion survivor walk (K2-C2B P2):
/// the cold walk with the AUDITED retiring-record set treated as
/// virtually absent — a physically present record in the exclusion
/// routes through the SAME authenticated skip jump an absent one
/// takes, and the MK-index staging automatically carries the
/// excluded records into the reclaimed-membership universe
/// (preserving depends_on resolution and key history exactly as
/// physical reclamation does). Every walked record and object
/// remains hash-verified; an unrelated missing parent still fails
/// closed.
pub(super) fn walk_chain_excluding(
    conn: &Connection,
    head: Hash,
    exclude: &BTreeSet<Hash>,
) -> Result<WalkedChain, MediumError> {
    let mut seen_ids = BTreeSet::new();
    let mut seen_keys = BTreeSet::new();
    let mut reachable = BTreeSet::new();
    let mut records = 0u64;
    let mut content_objects = 0u64;
    let mut current = head;
    let mut newer_epoch: Option<u64> = None;
    let mut head_epoch = 0u64;
    // (id, epoch, depends_on) per record in walk order (newest
    // first) — dependency ancestry is validated once the chain is
    // complete.
    let mut walked: Vec<(Hash, u64, Vec<Hash>)> = Vec::new();
    // The staged per-MK index entries — merged into the universe
    // only after the walk completes (pre-reclamation the range
    // records are still present and retained; only genuinely absent
    // entries are reclaimed members).
    let mut staged_indexes: Vec<ReclaimedIndexEntry> = Vec::new();
    let mut checkpoints: Vec<Hash> = Vec::new();
    // The most recent MK's skip context — the gap cover for
    // everything below it — and the pending-gap flag the linkage
    // check consumes at the jump target.
    let mut skip_table: Option<(Hash, CheckpointMetadata)> = None;
    let mut gap_pending = false;
    let (founding_id, authority, campaign_id) = loop {
        if !seen_ids.insert(current) {
            return Err(MediumError::Chain { rule: "cycle" });
        }
        let bytes = load_referenced_record(conn, &current)?;
        let record = RevisionRecord::decode(&bytes).map_err(MediumError::Format)?;
        if records == 0 {
            head_epoch = record.epoch;
        }
        // Checked linkage: an epoch of u64::MAX has no successor — it
        // must fail closed, never wrap. Across a skip-table jump the
        // linkage is gap-aware instead: the target must sit STRICTLY
        // below the pre-gap record (any gap size ≥ 1 — the reclaimed
        // span was removed whole).
        if let Some(epoch) = newer_epoch {
            if gap_pending {
                if record.epoch >= epoch {
                    return Err(MediumError::Chain {
                        rule: "epoch-regression",
                    });
                }
                gap_pending = false;
            } else if record.epoch.checked_add(1) != Some(epoch) {
                return Err(MediumError::Chain {
                    rule: "epoch-regression",
                });
            }
        }
        if let Some(key) = &record.idempotency_key
            && !seen_keys.insert(key.clone())
        {
            return Err(MediumError::Chain {
                rule: "duplicate-idempotency-key",
            });
        }
        let mut first_object: Option<Vec<u8>> = None;
        for (position, reference) in record.content_refs.iter().enumerate() {
            let bytes = load_referenced_object(conn, reference)?;
            if position == 0 {
                first_object = Some(bytes);
            }
            if reachable.insert(*reference) {
                content_objects += 1;
            }
        }
        // The MK recognition: a record whose first content object
        // carries the checkpoint-metadata domain — its index joins
        // the membership universe (staged for the post-walk merge)
        // and its skip table becomes the gap cover for everything
        // below it.
        if let Some(bytes) = first_object {
            let domain = peek_object_domain(&bytes).map_err(MediumError::Format)?;
            // The R9 corrective: the VERIFIED composite-closure
            // reachability (the same discipline as the file medium's
            // cold walk): a /2 root's chunks are transitively
            // referenced through its manifest — every dependency is
            // loaded hash-verified and fully envelope-checked before
            // it joins the reachable set. A missing, corrupt, or
            // malformed chunk fails the walk closed.
            if domain == composite_manifest::SESSION_SNAPSHOT_V2_DOMAIN {
                let manifest = composite_manifest::decode(&bytes).map_err(MediumError::Format)?;
                for entry in &manifest.entries {
                    let chunk_bytes = load_referenced_object(conn, &entry.identity)?;
                    crate::composite_append::verify_chunk_dependency(
                        &chunk_bytes,
                        &entry.identity,
                        &manifest,
                    )
                    .map_err(|_error| MediumError::Chain {
                        rule: "composite-closure-verification",
                    })?;
                    if reachable.insert(entry.identity) {
                        content_objects += 1;
                    }
                }
            }
            if domain == CHECKPOINT_METADATA_DOMAIN {
                let metadata = CheckpointMetadata::decode(&bytes).map_err(MediumError::Format)?;
                staged_indexes.extend(metadata.index.iter().cloned());
                checkpoints.push(current);
                skip_table = Some((current, metadata));
            }
        }
        reachable.insert(current);
        walked.push((current, record.epoch, record.depends_on.clone()));
        records += 1;
        newer_epoch = Some(record.epoch);
        match record.parent {
            Some(parent) => {
                // The compacted walk: an absent parent row is a gap —
                // legitimate only under the covering skip table;
                // anything else is hard corruption (fail closed,
                // never assumed reclamation). The K2-C2B P2
                // exclusion: an AUDITED retiring record routes
                // through the same authenticated jump (the
                // directive's chain-verified membership is the only
                // virtual-absence authority).
                if record_exists(conn, &parent)? && !exclude.contains(&parent) {
                    current = parent;
                } else {
                    let Some((owner, table)) = skip_table.as_ref() else {
                        return Err(MediumError::MissingObject { hash: parent });
                    };
                    let target = if current == *owner {
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
                    gap_pending = true;
                    current = target;
                }
            }
            None => match record.kind {
                RecordKind::Founding {
                    authority,
                    campaign_id,
                } => {
                    if record.epoch != 0 {
                        return Err(MediumError::Chain {
                            rule: "kind-incoherent",
                        });
                    }
                    break (current, authority, campaign_id);
                }
                RecordKind::Regular => {
                    return Err(MediumError::Chain {
                        rule: "founding-missing",
                    });
                }
            },
        }
    };
    // The membership universe merge: only genuinely-absent entries
    // are reclaimed members — pre-reclamation the range records are
    // still present, retained, and already accounted (their index
    // entries are the evidence for the FUTURE reclamation, never
    // double-counted).
    let mut reclaimed: BTreeMap<Hash, u64> = BTreeMap::new();
    let mut reclaimed_records = 0u64;
    let mut reclaimed_keys = 0u64;
    for entry in staged_indexes {
        if seen_ids.contains(&entry.hash) {
            continue;
        }
        if reclaimed.insert(entry.hash, entry.epoch).is_some() {
            return Err(MediumError::Chain {
                rule: "checkpoint-index-incoherent",
            });
        }
        if let Some(key) = &entry.key
            && !seen_keys.insert(key.clone())
        {
            return Err(MediumError::Chain {
                rule: "duplicate-idempotency-key",
            });
        }
        if entry.key.is_some() {
            reclaimed_keys += 1;
        }
        reclaimed_records += 1;
    }
    // Dependency ancestry, deferred until the chain is complete: the
    // walk runs newest-first, so a record at position i can only
    // depend on revisions at positions > i (strictly earlier), or on
    // a reclaim-indexed revision with its indexed epoch strictly
    // below the referencing record's.
    let mut positions: BTreeMap<Hash, usize> = BTreeMap::new();
    for (index, (id, _, _)) in walked.iter().enumerate() {
        positions.insert(*id, index);
    }
    for (index, (_, record_epoch, dependencies)) in walked.iter().enumerate() {
        for dependency in dependencies {
            let resolves = positions
                .get(dependency)
                .is_some_and(|position| *position > index)
                || reclaimed
                    .get(dependency)
                    .is_some_and(|&dep_epoch| dep_epoch < *record_epoch);
            if !resolves {
                return Err(MediumError::Chain {
                    rule: "depends-on-unresolvable",
                });
            }
        }
    }
    Ok(WalkedChain {
        summary: ChainSummary {
            head,
            height: head_epoch,
            founding: founding_id,
            authority,
            campaign_id: campaign_id.clone(),
            records,
            content_objects,
            checkpoints,
            reclaimed_records,
            reclaimed_keys,
        },
        cursor: SqliteCursor {
            head,
            epoch: head_epoch,
            seen_keys,
            reachable,
            record_ids: seen_ids,
            reclaimed,
            founding: founding_id,
            authority,
            campaign_id,
            content_objects,
        },
    })
}

/// Validates the observed head against the trusted cursor: O(0)
/// record walks on the verified head, a bounded suffix walk on
/// external divergence, a cold walk with no cursor, and fail-closed
/// divergence when the head vanished or is not a descendant.
pub(super) fn validate_observed(
    conn: &Connection,
    cursor: &mut Option<SqliteCursor>,
    walks: &mut WalkStats,
    observed: Option<Hash>,
) -> Result<(), MediumError> {
    let trusted = cursor.as_ref().map(|cursor| (cursor.head, cursor.epoch));
    match (observed, trusted) {
        (None, None) => Ok(()),
        (Some(id), Some((cursor_head, _))) if id == cursor_head => {
            walks.last_append_walked = 0;
            Ok(())
        }
        (Some(id), Some((trusted_head, trusted_epoch))) => {
            let (trusted_keys, trusted_record_ids, reclaimed) = cursor
                .as_ref()
                .map(|cursor| {
                    (
                        cursor.seen_keys.clone(),
                        cursor.record_ids.clone(),
                        cursor.reclaimed.clone(),
                    )
                })
                .unwrap_or_default();
            match walk_suffix(
                conn,
                id,
                trusted_head,
                trusted_epoch,
                trusted_keys,
                trusted_record_ids,
                reclaimed,
            ) {
                Ok(suffix) => {
                    walks.last_append_walked = suffix.len() as u64;
                    // R15: the entries already carry their
                    // walk-verified composite closures — no
                    // re-extraction (no duplicate hash-verifying
                    // reads) and no verification failure converted
                    // into an absent closure: absorption consumes
                    // only verified entries.
                    absorb_suffix(cursor, suffix, id);
                    Ok(())
                }
                Err(MediumError::Chain { rule: "divergence" }) => {
                    Err(MediumError::Chain { rule: "divergence" })
                }
                Err(_) => {
                    // Sound invalidation: discard the cursor; the full
                    // walk rebuilds it or fails closed.
                    *cursor = None;
                    match walk_chain(conn, id) {
                        Ok(walked) => {
                            walks.last_append_walked = walked.summary.records;
                            *cursor = Some(walked.cursor);
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                }
            }
        }
        (Some(id), None) => {
            let walked = walk_chain(conn, id)?;
            walks.last_append_walked = walked.summary.records;
            *cursor = Some(walked.cursor);
            Ok(())
        }
        (None, Some(_)) => Err(MediumError::Chain { rule: "divergence" }),
    }
}

/// The VERIFIED composite closure over the SQLite connection (the
/// R12 corrective): for a /2 root, strictly decode the manifest and
/// verify every declared chunk (the hash-verified load + the full
/// envelope check) — returning the closure identities for the
/// trusted reachability set. Every failure propagates typed;
/// `Ok(None)` is ONLY the positively established non-composite case
/// (the peeked domain is not the /2 root) — never a suppressed
/// verification failure.
fn verified_composite_closure(
    conn: &Connection,
    root_bytes: &[u8],
) -> Result<Option<Vec<Hash>>, MediumError> {
    let domain = peek_object_domain(root_bytes).map_err(MediumError::Format)?;
    if domain != composite_manifest::SESSION_SNAPSHOT_V2_DOMAIN {
        return Ok(None);
    }
    let manifest = composite_manifest::decode(root_bytes).map_err(MediumError::Format)?;
    let mut identities = Vec::with_capacity(manifest.entries.len());
    for entry in &manifest.entries {
        let chunk_bytes = load_referenced_object(conn, &entry.identity)?;
        crate::composite_append::verify_chunk_dependency(&chunk_bytes, &entry.identity, &manifest)
            .map_err(|_error| MediumError::Chain {
                rule: "composite-closure-verification",
            })?;
        identities.push(entry.identity);
    }
    Ok(Some(identities))
}

/// One suffix revision with its walk-verified composite closure (the
/// R15 corrective): the closure was verified DURING the suffix walk
/// by the established closure verifier over the record's FIRST
/// content reference — exactly the cold walk's discipline — so
/// `None` is positively established non-composite content (the
/// peeked domain is not the /2 root), NEVER a suppressed
/// verification failure: every required failure propagated typed
/// before this entry could exist. The entries are produced only by
/// [`walk_suffix`] and consumed only by [`absorb_suffix`] — no other
/// path extracts a closure.
struct VerifiedSuffixEntry {
    id: Hash,
    record: RevisionRecord,
    composite_closure: Option<Vec<Hash>>,
}

/// Walks `target` down its parents until `until` (exclusive),
/// verifying each record and its content references — the same
/// invariants the cold walk enforces, including the trusted-cursor
/// boundary epoch and dependency ancestry over the trusted record
/// set, the strictly-older suffix positions, and the reclaim-indexed
/// revisions' indexed epochs. A suffix may pass a foreign MK — its
/// record and metadata object validate like any other, but its index
/// is not absorbed into this handle's cursor (the universe is rebuilt
/// by the next cold walk; the single-writer discipline makes a
/// mid-handle foreign MK one this process published through another,
/// already-closed handle). R15: each returned entry carries its
/// walk-verified composite closure (the established verifier runs
/// during the walk — see [`VerifiedSuffixEntry`]).
fn walk_suffix(
    conn: &Connection,
    target: Hash,
    until: Hash,
    trusted_epoch: u64,
    trusted_keys: BTreeSet<Vec<u8>>,
    trusted_record_ids: BTreeSet<Hash>,
    reclaimed: BTreeMap<Hash, u64>,
) -> Result<Vec<VerifiedSuffixEntry>, MediumError> {
    let mut suffix: Vec<VerifiedSuffixEntry> = Vec::new();
    let mut current = target;
    let mut newer_epoch: Option<u64> = None;
    let mut visited = BTreeSet::new();
    let mut suffix_keys = BTreeSet::new();
    loop {
        if current == until {
            if let Some(boundary) = suffix.last()
                && trusted_epoch.checked_add(1) != Some(boundary.record.epoch)
            {
                return Err(MediumError::Chain {
                    rule: "epoch-regression",
                });
            }
            let mut positions: BTreeMap<Hash, usize> = BTreeMap::new();
            for (index, entry) in suffix.iter().enumerate() {
                positions.insert(entry.id, index);
            }
            for (index, entry) in suffix.iter().enumerate() {
                for dependency in &entry.record.depends_on {
                    let resolves = trusted_record_ids.contains(dependency)
                        || positions
                            .get(dependency)
                            .is_some_and(|position| *position > index)
                        || reclaimed
                            .get(dependency)
                            .is_some_and(|&dep_epoch| dep_epoch < entry.record.epoch);
                    if !resolves {
                        return Err(MediumError::Chain {
                            rule: "depends-on-unresolvable",
                        });
                    }
                }
            }
            return Ok(suffix);
        }
        if !visited.insert(current) {
            return Err(MediumError::Chain { rule: "cycle" });
        }
        let bytes = load_referenced_record(conn, &current)?;
        let record = RevisionRecord::decode(&bytes).map_err(MediumError::Format)?;
        if let Some(epoch) = newer_epoch
            && record.epoch.checked_add(1) != Some(epoch)
        {
            return Err(MediumError::Chain {
                rule: "epoch-regression",
            });
        }
        if let Some(key) = &record.idempotency_key
            && (trusted_keys.contains(key) || !suffix_keys.insert(key.clone()))
        {
            return Err(MediumError::Chain {
                rule: "duplicate-idempotency-key",
            });
        }
        // R15: every content reference loads hash-verified; the
        // FIRST reference's domain determines the entry's composite
        // closure — the cold walk's exact discipline — and the
        // established verifier runs HERE, during the walk: a /2
        // root's full closure is verified exactly once (no
        // re-extraction, no duplicate hash-verifying reads), a
        // malformed root domain refuses (the peek propagates), and a
        // missing, hash-mismatched, or malformed chunk refuses — a
        // failure is never converted into an absent closure.
        let mut first_object: Option<Vec<u8>> = None;
        for (position, reference) in record.content_refs.iter().enumerate() {
            let bytes = load_referenced_object(conn, reference)?;
            if position == 0 {
                first_object = Some(bytes);
            }
        }
        let composite_closure = match first_object {
            Some(bytes) => verified_composite_closure(conn, &bytes)?,
            None => None,
        };
        newer_epoch = Some(record.epoch);
        let parent = record.parent;
        suffix.push(VerifiedSuffixEntry {
            id: current,
            record,
            composite_closure,
        });
        match parent {
            Some(parent) => current = parent,
            None => return Err(MediumError::Chain { rule: "divergence" }),
        }
    }
}

/// Merges a verified suffix (newest-first) into the cursor and points
/// it at the new head — oldest-first absorption so the final epoch is
/// the newest verified one. R15: the entries carry their
/// walk-verified composite closures (verified during the walk, never
/// re-extracted here) — the cursor absorbs only verified entries,
/// after the whole suffix passed every check.
fn absorb_suffix(
    cursor: &mut Option<SqliteCursor>,
    suffix: Vec<VerifiedSuffixEntry>,
    new_head: Hash,
) {
    let Some(cursor) = cursor.as_mut() else {
        return;
    };
    for entry in suffix.into_iter().rev() {
        let VerifiedSuffixEntry {
            id,
            record,
            composite_closure,
        } = entry;
        if let Some(key) = &record.idempotency_key {
            cursor.seen_keys.insert(key.clone());
        }
        cursor.reachable.insert(id);
        cursor.record_ids.insert(id);
        for reference in &record.content_refs {
            if cursor.reachable.insert(*reference) {
                cursor.content_objects += 1;
            }
        }
        // The walk-verified composite closure joins the trusted
        // reachability set — the same identities the cold walk
        // collects over the same chain (`None` is positively
        // established non-composite content).
        if let Some(closure) = composite_closure {
            for identity in closure {
                if cursor.reachable.insert(identity) {
                    cursor.content_objects += 1;
                }
            }
        }
        cursor.epoch = record.epoch;
    }
    cursor.head = new_head;
}

/// Advances the cursor over an accepted append — O(1) work (plus
/// the verified composite closure when the record is a /2 root —
/// the R12 corrective keeps the hot cursor on the same definition
/// of reachability as the cold walk).
pub(super) fn advance_cursor(
    cursor: &mut Option<SqliteCursor>,
    record: &RevisionRecord,
    id: &Hash,
    composite_closure: Option<&[Hash]>,
) {
    let refs = &record.content_refs;
    match cursor.as_mut() {
        None => {
            let RecordKind::Founding {
                authority,
                campaign_id,
            } = &record.kind
            else {
                return;
            };
            let mut seen_keys = BTreeSet::new();
            if let Some(key) = &record.idempotency_key {
                seen_keys.insert(key.clone());
            }
            let mut reachable = BTreeSet::new();
            reachable.insert(*id);
            let mut record_ids = BTreeSet::new();
            record_ids.insert(*id);
            let mut content_objects = 0u64;
            for reference in refs {
                if reachable.insert(*reference) {
                    content_objects += 1;
                }
            }
            if let Some(closure) = composite_closure {
                for identity in closure {
                    if reachable.insert(*identity) {
                        content_objects += 1;
                    }
                }
            }
            *cursor = Some(SqliteCursor {
                head: *id,
                epoch: record.epoch,
                seen_keys,
                reachable,
                record_ids,
                reclaimed: BTreeMap::new(),
                founding: *id,
                authority: *authority,
                campaign_id: campaign_id.clone(),
                content_objects,
            });
        }
        Some(cursor) => {
            cursor.head = *id;
            cursor.epoch = record.epoch;
            if let Some(key) = &record.idempotency_key {
                cursor.seen_keys.insert(key.clone());
            }
            cursor.reachable.insert(*id);
            cursor.record_ids.insert(*id);
            for reference in refs {
                if cursor.reachable.insert(*reference) {
                    cursor.content_objects += 1;
                }
            }
            if let Some(closure) = composite_closure {
                for identity in closure {
                    if cursor.reachable.insert(*identity) {
                        cursor.content_objects += 1;
                    }
                }
            }
        }
    }
}

// --------------------------------------------------------- the meta rows
