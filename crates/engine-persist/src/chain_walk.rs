//! (P3-L9b) The shared local/sqlite walk core — the A13 Option 2
//! walk half (the design §2.4.2): the pure, reader-generic
//! trusted-cursor validation machinery BOTH providers call under
//! their own coordination (the local HEAD.lock, the sqlite
//! transaction), expressed over a reader seam of exactly the walks'
//! three loading operations — the V2X-CP2 `BeginReads` narrowness,
//! never the rejected `ChainWalkProvider` generalization (the
//! providers' distinctions are the contract: the file loads vs the
//! SQL rows, the lock vs the transaction, the insert-once vs the
//! BEGIN IMMEDIATE ladder).
//!
//! **The frozen refusal rules — ONE place (the win):** every
//! `MediumError::Chain { rule }` the walks can produce is defined
//! here and nowhere else: `divergence`, `epoch-regression`,
//! `duplicate-idempotency-key`, `depends-on-unresolvable`, `cycle`,
//! `kind-incoherent`, `founding-missing`,
//! `checkpoint-index-incoherent`, and
//! `composite-closure-verification` (the composite suffix closure's
//! rule — unified on the string both providers' cold walks and the
//! reclaim execution walk already used; local's pre-unification
//! suffix form carried `composite-suffix-closure-verification`, the
//! one deviation from the identical-rules contract, an artifact of
//! the uncoordinated R10/R15 correctives — retired here, proven by
//! the differential battery). The uncovered-gap refusal
//! (`MissingObject` for the absent parent) and the typed format /
//! referenced-corruption propagations are likewise defined here.
//!
//! **What is shared (this module):** the cold founding → HEAD walk
//! with the PP-B3 gap-aware skip descent and the K2-C2B
//! virtual-exclusion variant; the bounded suffix walk with the
//! verified composite closure (the R15 discipline — the closure is
//! verified DURING the walk, exactly once, never re-extracted); the
//! suffix absorption; the observed-HEAD validation dispatch (the
//! §5.3.2 arms); the O(1) cursor advances; and the §5.3.2 ten-field
//! trusted cursor itself (the field sets were identical across the
//! providers — the same names, types, and order — so the core owns
//! the one definition and each provider aliases it).
//!
//! **What stays provider-side (the L9 record's four differences,
//! each with its disposition):**
//!
//! 1. **The loaders** — the [`ChainReads`] seam: local reads via the
//!    lock-held hash-verified object-file loads (records and content
//!    objects are the same content-addressed files, and the
//!    parent-present probe is the object path's `is_file`); sqlite
//!    reads via the transaction-scoped SQL rows (the `records` table
//!    for record bytes, the `objects` table for content bytes, and
//!    the `record_exists` probe for the gap detection). Every
//!    implementation MUST hash-verify (the `BeginReads` contract).
//! 2. **The free fns vs methods and the stats discipline** — the
//!    providers keep their own `validate_observed` wrappers (local's
//!    method on `LocalFileMedium`, sqlite's free fn over
//!    `(&Connection, &mut Option<SqliteCursor>, &mut WalkStats)`)
//!    and map the [`ValidationOutcome`] to their own counters:
//!    local counts cold walks into `cold_walk_records`, sqlite
//!    counts the post-invalidation cold walks into
//!    `last_append_walked` (each pinned by its own provider's
//!    tests; the counters are deliberately NOT part of the
//!    differential battery's frozen observables).
//! 3. **The suffix closure timing** — UNIFIED on sqlite's R15 form
//!    (the [`VerifiedSuffixEntry`] carrying the walk-verified
//!    closure): local's pre-unification two-pass form (the closure
//!    recomputed in `absorb_suffix` after the walk) is retired. The
//!    disposition is documented in the differential battery's corner
//!    test: in the closure-failure corner local previously let the
//!    trusted cursor SURVIVE the refusal while sqlite discarded it
//!    (the sound-invalidation arm); the unified core takes the R15
//!    discipline on both — the failure routes into the invalidation
//!    arm, the cursor is discarded, and the cold walk re-derives the
//!    refusal (and local gains R15's no-duplicate-reads property:
//!    the closure's hash-verifying loads happen exactly once).
//! 4. **The refusal rules** — centralized HERE (see the frozen table
//!    above); the one pre-existing deviation (local's suffix
//!    composite-closure string) is resolved toward the
//!    four-of-five-walk-sites majority and the contract's intent,
//!    with the battery proving every other output byte-identical.

use std::collections::{BTreeMap, BTreeSet};

use crate::checkpoint::{
    CHECKPOINT_METADATA_DOMAIN, CheckpointMetadata, ReclaimedIndexEntry, peek_object_domain,
};
use crate::composite_manifest;
use crate::hash::Hash;
use crate::medium::{ChainSummary, MediumError};
use crate::record::{AuthorityMode, RecordKind, RevisionRecord};

// ----------------------------------------------------------- the reader seam

/// (P3-L9b) The walks' exact loading surface — the V2X-CP2
/// `BeginReads` narrowness (three operations, nothing else): the
/// hash-verified record bytes, the hash-verified content-object
/// bytes, and the record-presence probe the compacted walk's gap
/// detection consumes. ONE reader object backs a whole walk (a
/// provider transaction, a lock-held medium) so a single
/// coordination-scoped borrow suffices. Every implementation MUST
/// hash-verify: the bytes returned for an identity are the stored
/// bytes AT that identity (a mismatch is the referenced-corruption
/// refusal, never silently passed through).
pub(crate) trait ChainReads {
    /// Reads and hash-verifies a revision record's canonical bytes.
    fn read_record_bytes(&mut self, id: &Hash) -> Result<Vec<u8>, MediumError>;
    /// Reads and hash-verifies a content object's bytes.
    fn read_object_bytes(&mut self, hash: &Hash) -> Result<Vec<u8>, MediumError>;
    /// Whether a record exists at all (the compacted walk's gap
    /// detection — an absent parent is a gap, legitimate only under
    /// the covering skip table).
    fn record_present(&mut self, id: &Hash) -> Result<bool, MediumError>;
}

// ---------------------------------------------------------------- the cursor

/// The §5.3.2 session-scoped trusted cursor — in-memory derived
/// state rebuilt by the cold walk; never persisted. The ten-field
/// form both providers carried identically (the same names, types,
/// and order — the differential battery's cursor fingerprint relies
/// on exactly that).
#[derive(Debug)]
pub(crate) struct WalkCursor {
    pub(crate) head: Hash,
    pub(crate) epoch: u64,
    /// Idempotency keys seen across the chain (session index for O(1)
    /// duplicate checks at append; O(chain) memory, documented).
    pub(crate) seen_keys: BTreeSet<Vec<u8>>,
    /// Every record id and content hash reachable from HEAD.
    pub(crate) reachable: BTreeSet<Hash>,
    /// The chain's revision ids alone (dependency resolution requires
    /// a *prior revision*, never a content hash that happens to be
    /// reachable).
    pub(crate) record_ids: BTreeSet<Hash>,
    /// The reclaim-indexed revisions (identity → epoch) from every
    /// on-chain MK's index whose records are genuinely absent — the
    /// historical membership evidence (PP-B3's condition 2: a
    /// reclaimed revision's key-dedup and depends_on resolution
    /// survive its deletion). The observational counts (the MK list,
    /// the reclaimed totals) live on the [`ChainSummary`] the walk
    /// returns, never on the cursor.
    pub(crate) reclaimed: BTreeMap<Hash, u64>,
    pub(crate) founding: Hash,
    pub(crate) authority: AuthorityMode,
    pub(crate) campaign_id: String,
    pub(crate) content_objects: u64,
}

/// A completed cold walk: the summary plus the cursor it establishes.
pub(crate) struct WalkedChain {
    pub(crate) summary: ChainSummary,
    pub(crate) cursor: WalkCursor,
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
pub(crate) struct VerifiedSuffixEntry {
    pub(crate) id: Hash,
    pub(crate) record: RevisionRecord,
    pub(crate) composite_closure: Option<Vec<Hash>>,
}

/// What the observed-HEAD validation dispatch did — the providers
/// map this to their own diagnostics counters (the stats discipline
/// is provider-side by the L9 record's difference (b)).
pub(crate) enum ValidationOutcome {
    /// (None, None): nothing observed, nothing trusted.
    NoObservedHead,
    /// The observed head equals the trusted cursor's head — the O(0)
    /// arm (no records walked).
    VerifiedHead,
    /// A verified suffix was absorbed into the trusted cursor (the
    /// bounded walk over the external divergence).
    SuffixAbsorbed {
        /// The suffix's record count.
        walked: u64,
    },
    /// Sound invalidation: the cursor was discarded and the cold walk
    /// rebuilt it from the observed head.
    RebuiltFromInvalidation {
        /// The rebuilt chain's record count.
        records: u64,
    },
    /// No trusted cursor stood: the cold walk established it from the
    /// observed head.
    FreshColdEstablished {
        /// The established chain's record count.
        records: u64,
    },
}

// --------------------------------------------------------------- the walks

/// The cold founding → HEAD walk: every record hash-verified,
/// epoch-linked with checked arithmetic, key-unique; every referenced
/// object loaded and hash-verified; every depends_on reference
/// resolving to a strictly earlier revision of the same authoritative
/// history (retained, or reclaim-indexed with its indexed epoch).
/// Rebuilds the cursor and returns the summary.
///
/// **The compacted mode (PP-B3):** an absent parent is a
/// legitimately-reclaimed revision only when the most recent on-chain
/// MK's skip table covers the gap — a skip for exactly the record
/// whose parent is absent (or that MK's own self entry), jumping to
/// the nearest retained ancestor under the gap-aware strictly-earlier
/// epoch linkage. An uncovered absence is hard corruption — fail
/// closed, never assumed reclamation.
///
/// **The authenticated virtual-exclusion survivor walk (K2-C2B P2):**
/// a nonempty `exclude` treats the AUDITED retiring-record set as
/// virtually absent — a physically present record in the exclusion
/// routes through the SAME authenticated skip jump an absent one
/// takes, and the MK-index staging automatically carries the excluded
/// records into the reclaimed-membership universe (preserving
/// depends_on resolution and key history exactly as physical
/// reclamation does). The walk is a pure computation over the reader:
/// no provider state is touched.
pub(crate) fn walk_chain_excluding<R: ChainReads>(
    reads: &mut R,
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
    // (id, epoch, depends_on) per record in walk order (newest first)
    // — dependency ancestry is validated once the chain is complete.
    let mut walked: Vec<(Hash, u64, Vec<Hash>)> = Vec::new();
    // The staged per-MK index entries — merged into the universe only
    // after the walk completes (pre-reclamation the range records are
    // still present and retained; only genuinely absent entries are
    // reclaimed members).
    let mut staged_indexes: Vec<ReclaimedIndexEntry> = Vec::new();
    let mut checkpoints: Vec<Hash> = Vec::new();
    // The most recent MK's skip context — the gap cover for everything
    // below it — and the pending-gap flag the linkage check consumes
    // at the jump target.
    let mut skip_table: Option<(Hash, CheckpointMetadata)> = None;
    let mut gap_pending = false;
    let (founding_id, authority, campaign_id) = loop {
        if !seen_ids.insert(current) {
            return Err(MediumError::Chain { rule: "cycle" });
        }
        let bytes = reads.read_record_bytes(&current)?;
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
            let bytes = reads.read_object_bytes(reference)?;
            if position == 0 {
                first_object = Some(bytes);
            }
            if reachable.insert(*reference) {
                content_objects += 1;
            }
        }
        // The MK recognition: a record whose first content object
        // carries the checkpoint-metadata domain — its index joins the
        // membership universe (staged for the post-walk merge) and its
        // skip table becomes the gap cover for everything below it.
        if let Some(bytes) = first_object {
            let domain = peek_object_domain(&bytes).map_err(MediumError::Format)?;
            // The VERIFIED composite-closure reachability (the K2-C1
            // corrective): a /2 root's chunks are transitively
            // referenced through its manifest — the walk proves EVERY
            // dependency (the hash-verified read, the full envelope
            // verification against the manifest entry) before it joins
            // the reachable set. A missing, corrupt, or malformed
            // chunk fails the walk closed; nothing is silently
            // ignored.
            if domain == composite_manifest::SESSION_SNAPSHOT_V2_DOMAIN {
                let manifest = composite_manifest::decode(&bytes).map_err(MediumError::Format)?;
                for entry in &manifest.entries {
                    let chunk_bytes = reads.read_object_bytes(&entry.identity)?;
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
                // The compacted walk: an absent parent is a gap —
                // legitimate only under the covering skip table;
                // anything else is hard corruption (fail closed, never
                // assumed reclamation). The K2-C2B P2 exclusion: an
                // AUDITED retiring record routes through the same
                // authenticated jump (the directive's chain-verified
                // membership is the only virtual-absence authority).
                if reads.record_present(&parent)? && !exclude.contains(&parent) {
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
    // The membership universe merge: only genuinely-absent entries are
    // reclaimed members — pre-reclamation the range records are still
    // present, retained, and already accounted (their index entries
    // are the evidence for the FUTURE reclamation, never
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
    // depend on revisions at positions > i (strictly earlier), or on a
    // reclaim-indexed revision with its indexed epoch strictly below
    // the referencing record's. A dependency naming a nonexistent
    // revision, a content hash, itself, or any later revision is
    // unresolvable — hash consistency alone never makes a chain
    // semantically valid.
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
        cursor: WalkCursor {
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

/// The cold walk over the complete chain (no virtual exclusion).
pub(crate) fn walk_chain<R: ChainReads>(
    reads: &mut R,
    head: Hash,
) -> Result<WalkedChain, MediumError> {
    walk_chain_excluding(reads, head, &BTreeSet::new())
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
pub(crate) fn walk_suffix<R: ChainReads>(
    reads: &mut R,
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
            // The trusted-cursor boundary: the oldest suffix revision
            // must sit exactly one epoch above the trusted head
            // (checked arithmetic — no epoch above u64::MAX can
            // satisfy it, so the boundary can never wrap).
            if let Some(boundary) = suffix.last()
                && trusted_epoch.checked_add(1) != Some(boundary.record.epoch)
            {
                return Err(MediumError::Chain {
                    rule: "epoch-regression",
                });
            }
            // Dependency ancestry, deferred until the suffix is
            // complete: newest-first, a record at index i may depend
            // only on trusted revisions (all older) or on suffix
            // revisions at indices > i (strictly older) — a dependency
            // naming a newer suffix record, itself, or anything
            // outside the authoritative history is unresolvable.
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
        let bytes = reads.read_record_bytes(&current)?;
        let record = RevisionRecord::decode(&bytes).map_err(MediumError::Format)?;
        // Checked linkage: an epoch of u64::MAX has no successor — it
        // must fail closed, never wrap.
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
        // R15: every content reference loads hash-verified; the FIRST
        // reference's domain determines the entry's composite closure
        // — the cold walk's exact discipline — and the established
        // verifier runs HERE, during the walk: a /2 root's full
        // closure is verified exactly once (no re-extraction, no
        // duplicate hash-verifying reads), a malformed root domain
        // refuses (the peek propagates), and a missing,
        // hash-mismatched, or malformed chunk refuses — a failure is
        // never converted into an absent closure.
        let mut first_object: Option<Vec<u8>> = None;
        for (position, reference) in record.content_refs.iter().enumerate() {
            let bytes = reads.read_object_bytes(reference)?;
            if position == 0 {
                first_object = Some(bytes);
            }
        }
        let composite_closure = match first_object {
            Some(bytes) => verified_composite_closure(reads, &bytes)?,
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

/// The VERIFIED composite closure over the reader (the R12
/// corrective): for a /2 root, strictly decode the manifest and
/// verify every declared chunk (the hash-verified load + the full
/// envelope check) — returning the closure identities for the trusted
/// reachability set. Every failure propagates typed; `Ok(None)` is
/// ONLY the positively established non-composite case (the peeked
/// domain is not the /2 root) — never a suppressed verification
/// failure.
pub(crate) fn verified_composite_closure<R: ChainReads>(
    reads: &mut R,
    root_bytes: &[u8],
) -> Result<Option<Vec<Hash>>, MediumError> {
    let domain = peek_object_domain(root_bytes).map_err(MediumError::Format)?;
    if domain != composite_manifest::SESSION_SNAPSHOT_V2_DOMAIN {
        return Ok(None);
    }
    let manifest = composite_manifest::decode(root_bytes).map_err(MediumError::Format)?;
    let mut identities = Vec::with_capacity(manifest.entries.len());
    for entry in &manifest.entries {
        let chunk_bytes = reads.read_object_bytes(&entry.identity)?;
        crate::composite_append::verify_chunk_dependency(&chunk_bytes, &entry.identity, &manifest)
            .map_err(|_error| MediumError::Chain {
                rule: "composite-closure-verification",
            })?;
        identities.push(entry.identity);
    }
    Ok(Some(identities))
}

/// Merges a verified suffix (newest-first) into the cursor and points
/// it at the new head — oldest-first absorption so the final epoch is
/// the newest verified one. R15: the entries carry their
/// walk-verified composite closures (verified during the walk, never
/// re-extracted here) — the cursor absorbs only verified entries,
/// after the whole suffix passed every check.
pub(crate) fn absorb_suffix(
    cursor: &mut Option<WalkCursor>,
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

/// Validates the observed head against the trusted cursor: O(0)
/// record walks on the verified head, a bounded suffix walk on
/// external divergence, a cold walk with no cursor, and fail-closed
/// divergence when the head vanished or is not a descendant. The
/// outcome carries what happened; the provider maps it to its own
/// diagnostics counters.
pub(crate) fn validate_observed<R: ChainReads>(
    reads: &mut R,
    cursor: &mut Option<WalkCursor>,
    observed: Option<Hash>,
) -> Result<ValidationOutcome, MediumError> {
    let trusted = cursor.as_ref().map(|cursor| (cursor.head, cursor.epoch));
    match (observed, trusted) {
        (None, None) => Ok(ValidationOutcome::NoObservedHead),
        (Some(id), Some((cursor_head, _))) if id == cursor_head => {
            Ok(ValidationOutcome::VerifiedHead)
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
                reads,
                id,
                trusted_head,
                trusted_epoch,
                trusted_keys,
                trusted_record_ids,
                reclaimed,
            ) {
                Ok(suffix) => {
                    let walked = suffix.len() as u64;
                    absorb_suffix(cursor, suffix, id);
                    Ok(ValidationOutcome::SuffixAbsorbed { walked })
                }
                Err(MediumError::Chain { rule: "divergence" }) => {
                    // The observed chain does not contain our verified
                    // history: a divergent rewrite — detected, never
                    // merged, fail closed.
                    Err(MediumError::Chain { rule: "divergence" })
                }
                Err(_) => {
                    // Sound invalidation: discard the cursor; the full
                    // walk rebuilds it or fails closed.
                    *cursor = None;
                    let walked = walk_chain(reads, id)?;
                    *cursor = Some(walked.cursor);
                    Ok(ValidationOutcome::RebuiltFromInvalidation {
                        records: walked.summary.records,
                    })
                }
            }
        }
        (Some(id), None) => {
            let walked = walk_chain(reads, id)?;
            *cursor = Some(walked.cursor);
            Ok(ValidationOutcome::FreshColdEstablished {
                records: walked.summary.records,
            })
        }
        (None, Some(_)) => Err(MediumError::Chain { rule: "divergence" }),
    }
}

/// Advances the cursor over an accepted append — O(1) work (plus the
/// verified composite closure when the record is a /2 root — the R12
/// corrective keeps the hot cursor on the same definition of
/// reachability as the cold walk).
pub(crate) fn advance_cursor(
    cursor: &mut Option<WalkCursor>,
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
                // A regular append always has a cursor (validated).
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
            *cursor = Some(WalkCursor {
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
