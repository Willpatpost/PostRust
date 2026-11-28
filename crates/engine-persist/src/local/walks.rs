//! The trusted-cursor validation walks (split from the local
//! monolith by change reason, P3-L9 pure move): the observed-HEAD
//! validation dispatch, the bounded suffix walk, the suffix
//! absorption, the cold founding-walk with the PP-B3 gap-aware skip
//! descent and the K2-C2B virtual-exclusion variant, and the O(1)
//! cursor advances. These change when the chain-validation rules
//! change; the provider's own walks stay provider-local by design -
//! the local/sqlite walk unification is deliberately NOT this lane
//! (the orchestrator's follow-up; the observed duplication is
//! recorded in the lane report).

use std::collections::{BTreeMap, BTreeSet};

use crate::checkpoint::{
    CHECKPOINT_METADATA_DOMAIN, CheckpointMetadata, ReclaimedIndexEntry, peek_object_domain,
};
use crate::composite_manifest;
use crate::hash::Hash;
use crate::medium::{ChainSummary, MediumError};
use crate::record::{RecordKind, RevisionRecord};

use super::{Cursor, LocalFileMedium};

impl LocalFileMedium {
    // -------------------------------------------------------- validation

    /// Validates the observed HEAD against the trusted cursor: O(0)
    /// record walks on the verified head, a bounded suffix walk on
    /// external divergence, a cold walk with no cursor, and fail-closed
    /// divergence when HEAD vanished or is not a descendant.
    pub(super) fn validate_observed(&mut self, observed: Option<Hash>) -> Result<(), MediumError> {
        let trusted = self
            .cursor
            .as_ref()
            .map(|cursor| (cursor.head, cursor.epoch));
        match (observed, trusted) {
            (None, None) => Ok(()),
            (Some(id), Some((cursor_head, _))) if id == cursor_head => {
                self.walks.last_append_walked = 0;
                Ok(())
            }
            (Some(id), Some((trusted_head, trusted_epoch))) => {
                match self.walk_suffix(id, trusted_head, trusted_epoch) {
                    Ok(suffix) => {
                        self.walks.last_append_walked = suffix.len() as u64;
                        self.absorb_suffix(suffix, id)?;
                        Ok(())
                    }
                    Err(MediumError::Chain { rule: "divergence" }) => {
                        // The observed chain does not contain our
                        // verified history: a divergent rewrite —
                        // detected, never merged, fail closed.
                        Err(MediumError::Chain { rule: "divergence" })
                    }
                    Err(_) => {
                        // Sound invalidation: discard the cursor; the
                        // full walk rebuilds it or fails closed.
                        self.cursor = None;
                        self.cold_walk_from(id).map(|_| ())
                    }
                }
            }
            (Some(id), None) => self.cold_walk_from(id).map(|_| ()),
            (None, Some(_)) => Err(MediumError::Chain { rule: "divergence" }),
        }
    }

    /// Walks `target` down its parents until `until` (exclusive),
    /// verifying each record and its content references. Returns the
    /// suffix newest-first; reaching the founding record without
    /// encountering `until` is divergence. Bounded: a corrupt external
    /// chain whose parents cycle without passing through `until` fails
    /// closed (the no-hang obligation). The suffix must satisfy the
    /// same invariants the cold walk enforces: idempotency-key
    /// uniqueness, checked epoch linkage **including the trusted-cursor
    /// boundary** (the oldest suffix revision sits exactly one epoch
    /// above the trusted head), and dependency ancestry (every
    /// depends_on reference names a trusted revision, a strictly
    /// older suffix revision, or a reclaim-indexed revision with its
    /// indexed epoch strictly earlier). A suffix may pass a foreign
    /// MK — its record and metadata object validate like any other,
    /// but its index is not absorbed into this handle's cursor: the
    /// membership universe is rebuilt by the cold walk at the next
    /// open (under the single-writer discipline a mid-handle foreign
    /// MK is one this process published through another,
    /// already-closed handle).
    fn walk_suffix(
        &self,
        target: Hash,
        until: Hash,
        trusted_epoch: u64,
    ) -> Result<Vec<(Hash, RevisionRecord)>, MediumError> {
        let mut suffix: Vec<(Hash, RevisionRecord)> = Vec::new();
        let mut current = target;
        let mut newer_epoch: Option<u64> = None;
        let mut visited = BTreeSet::new();
        let mut suffix_keys = BTreeSet::new();
        let (trusted_keys, trusted_record_ids, reclaimed) = self
            .cursor
            .as_ref()
            .map(|cursor| {
                (
                    cursor.seen_keys.clone(),
                    cursor.record_ids.clone(),
                    cursor.reclaimed.clone(),
                )
            })
            .unwrap_or_default();
        loop {
            if current == until {
                // The trusted-cursor boundary: the oldest suffix
                // revision must sit exactly one epoch above the
                // trusted head (checked arithmetic — no epoch above
                // u64::MAX can satisfy it, so the boundary can never
                // wrap).
                if let Some((_, boundary)) = suffix.last()
                    && trusted_epoch.checked_add(1) != Some(boundary.epoch)
                {
                    return Err(MediumError::Chain {
                        rule: "epoch-regression",
                    });
                }
                // Dependency ancestry, deferred until the suffix is
                // complete: newest-first, a record at index i may
                // depend only on trusted revisions (all older) or on
                // suffix revisions at indices > i (strictly older) —
                // a dependency naming a newer suffix record, itself,
                // or anything outside the authoritative history is
                // unresolvable.
                let mut positions: BTreeMap<Hash, usize> = BTreeMap::new();
                for (index, (id, _)) in suffix.iter().enumerate() {
                    positions.insert(*id, index);
                }
                for (index, (_, record)) in suffix.iter().enumerate() {
                    for dependency in &record.depends_on {
                        let resolves = trusted_record_ids.contains(dependency)
                            || positions
                                .get(dependency)
                                .is_some_and(|position| *position > index)
                            || reclaimed
                                .get(dependency)
                                .is_some_and(|&dep_epoch| dep_epoch < record.epoch);
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
            let bytes = self.load_referenced(&current)?;
            let record = RevisionRecord::decode(&bytes).map_err(MediumError::Format)?;
            // Checked linkage: an epoch of u64::MAX has no successor —
            // it must fail closed, never wrap.
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
            for reference in &record.content_refs {
                self.load_referenced(reference)?;
            }
            newer_epoch = Some(record.epoch);
            match record.parent {
                Some(parent) => {
                    suffix.push((current, record));
                    current = parent;
                }
                None => return Err(MediumError::Chain { rule: "divergence" }),
            }
        }
    }

    /// Merges a verified suffix (newest-first) into the cursor and
    /// points it at the new head. Absorption runs oldest-first so the
    /// final epoch written is the newest verified epoch — for a
    /// multi-revision suffix the cursor's head, epoch, reachable set,
    /// seen-key index, and record-id index stay mutually consistent.
    fn absorb_suffix(
        &mut self,
        suffix: Vec<(Hash, RevisionRecord)>,
        new_head: Hash,
    ) -> Result<(), MediumError> {
        // The R10 corrective: precompute each suffix record's
        // VERIFIED composite closure (if its first reference is a
        // /2 root) BEFORE the cursor borrow — every failure
        // PROPAGATES (never silently treated as an absent closure):
        // the hash-verified load, the domain peek, the strict
        // manifest decode, and EVERY dependency's full envelope
        // verification (the same verify_chunk_dependency the cold
        // walk uses). A missing, corrupt, or malformed transitive
        // chunk refuses the suffix absorption — the trusted cursor
        // does NOT advance past unverifiable composite state.
        let mut closures: Vec<(Hash, RevisionRecord, Option<Vec<Hash>>)> = Vec::new();
        for (id, record) in suffix.into_iter() {
            let mut closure = None;
            if let Some(root) = record.content_refs.first() {
                let bytes = self.load_referenced(root)?;
                // R13: the domain peek PROPAGATES its failure — a
                // malformed leading prefix is a typed fail-closed
                // refusal, never silently treated as an unrelated
                // object. The recognized-but-unsupported future
                // family member (e.g. /3) follows the forward-
                // compatibility semantics: it passes the peek (a
                // valid domain string), does not match /2, and is
                // carried as a non-composite reference — the
                // recovery-level fail-closed (R11's regression)
                // handles it.
                let domain = peek_object_domain(&bytes).map_err(MediumError::Format)?;
                if domain == composite_manifest::SESSION_SNAPSHOT_V2_DOMAIN {
                    let manifest =
                        composite_manifest::decode(&bytes).map_err(MediumError::Format)?;
                    let mut identities = Vec::with_capacity(manifest.entries.len());
                    for entry in &manifest.entries {
                        let chunk_bytes = self.load_referenced(&entry.identity)?;
                        crate::composite_append::verify_chunk_dependency(
                            &chunk_bytes,
                            &entry.identity,
                            &manifest,
                        )
                        .map_err(|_error| MediumError::Chain {
                            rule: "composite-suffix-closure-verification",
                        })?;
                        identities.push(entry.identity);
                    }
                    closure = Some(identities);
                }
            }
            closures.push((id, record, closure));
        }
        let Some(cursor) = self.cursor.as_mut() else {
            return Ok(());
        };
        for (id, record, closure) in closures.into_iter().rev() {
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
            if let Some(closure) = closure {
                for identity in closure {
                    if cursor.reachable.insert(identity) {
                        cursor.content_objects += 1;
                    }
                }
            }
            cursor.epoch = record.epoch;
        }
        cursor.head = new_head;
        Ok(())
    }

    /// The cold founding → HEAD walk: every record hash-verified,
    /// epoch-linked with checked arithmetic, key-unique; every
    /// referenced object loaded and hash-verified; every depends_on
    /// reference resolving to a strictly earlier revision of the same
    /// authoritative history (retained, or reclaim-indexed with its
    /// indexed epoch). Rebuilds the cursor and returns the summary.
    ///
    /// **The compacted mode (PP-B3):** an absent parent is a
    /// legitimately-reclaimed revision only when the most recent
    /// on-chain MK's skip table covers the gap — a skip for exactly
    /// the record whose parent is absent (or that MK's own self
    /// entry), jumping to the nearest retained ancestor under the
    /// gap-aware strictly-earlier epoch linkage. An uncovered absence
    /// is hard corruption — fail closed, never assumed reclamation.
    pub(super) fn cold_walk_from(&mut self, head: Hash) -> Result<ChainSummary, MediumError> {
        let (summary, cursor) = self.cold_walk_excluding(head, &BTreeSet::new())?;
        self.walks.cold_walk_records = summary.records;
        self.cursor = Some(cursor);
        Ok(summary)
    }

    /// The authenticated virtual-exclusion survivor walk (K2-C2B
    /// P2): the cold walk with the AUDITED retiring-record set
    /// treated as virtually absent — a physically present record in
    /// the exclusion routes through the SAME authenticated skip
    /// jump an absent one takes (the covering MK's table, the
    /// gap-aware epoch linkage), and the MK-index staging
    /// automatically carries the excluded records into the
    /// reclaimed-membership universe (preserving depends_on
    /// resolution and key history exactly as physical reclamation
    /// does). **P2-F1: the simulated post-retirement view is
    /// returned as DATA — the authoritative trusted cursor, the
    /// walk counters, and every other provider state are untouched
    /// (the walk is a pure `&self` computation), so an unsuccessful
    /// walk, an authorization refusal, or a resource limit leaves
    /// the provider's ordinary state exactly as it was, and no
    /// append, validation, or sweep ever observes the simulated
    /// view.** The caller decides whether and when to establish a
    /// cursor consistent with the post-deletion physical state.
    pub(super) fn cold_walk_excluding(
        &self,
        head: Hash,
        exclude: &BTreeSet<Hash>,
    ) -> Result<(ChainSummary, Cursor), MediumError> {
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
        // records are still present and retained; only genuinely
        // absent entries are reclaimed members).
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
            let bytes = self.load_referenced(&current)?;
            let record = RevisionRecord::decode(&bytes).map_err(MediumError::Format)?;
            if records == 0 {
                head_epoch = record.epoch;
            }
            // Checked linkage: an epoch of u64::MAX has no successor —
            // it must fail closed, never wrap. Across a skip-table
            // jump the linkage is gap-aware instead: the target must
            // sit STRICTLY below the pre-gap record (any gap size ≥ 1
            // — the reclaimed span was removed whole).
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
                let bytes = self.load_referenced(reference)?;
                if position == 0 {
                    first_object = Some(bytes);
                }
                if reachable.insert(*reference) {
                    content_objects += 1;
                }
            }
            // The MK recognition: a record whose first content object
            // carries the checkpoint-metadata domain — its index
            // joins the membership universe (staged for the post-walk
            // merge) and its skip table becomes the gap cover for
            // everything below it.
            if let Some(bytes) = first_object {
                let domain = peek_object_domain(&bytes).map_err(MediumError::Format)?;
                // The K2-C1 corrective's VERIFIED composite-closure
                // reachability: a /2 root's chunks are transitively
                // referenced through its manifest — the walk proves
                // EVERY dependency (the hash-verified read, the full
                // envelope verification against the manifest entry)
                // before it joins the reachable set. A missing,
                // corrupt, or malformed chunk fails the walk closed;
                // nothing is silently ignored.
                if domain == composite_manifest::SESSION_SNAPSHOT_V2_DOMAIN {
                    let manifest =
                        composite_manifest::decode(&bytes).map_err(MediumError::Format)?;
                    for entry in &manifest.entries {
                        let chunk_bytes = self.load_referenced(&entry.identity)?;
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
                    let metadata =
                        CheckpointMetadata::decode(&bytes).map_err(MediumError::Format)?;
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
                    // anything else is hard corruption (fail closed,
                    // never assumed reclamation). The K2-C2B P2
                    // exclusion: an AUDITED retiring record routes
                    // through the same authenticated jump (the
                    // directive's chain-verified membership is the
                    // only virtual-absence authority).
                    if self.object_path(&parent).is_file() && !exclude.contains(&parent) {
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
        // The membership universe merge: only genuinely-absent
        // entries are reclaimed members — pre-reclamation the range
        // records are still present, retained, and already accounted
        // (their index entries are the evidence for the FUTURE
        // reclamation, never double-counted).
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
        // Dependency ancestry, deferred until the chain is complete:
        // the walk runs newest-first, so a record at position i can
        // only depend on revisions at positions > i (strictly
        // earlier), or on a reclaim-indexed revision with its indexed
        // epoch strictly below the referencing record's. A dependency
        // naming a nonexistent revision, a content hash, itself, or
        // any later revision is unresolvable — hash consistency
        // alone never makes a chain semantically valid.
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
        let cursor = Cursor {
            head,
            epoch: head_epoch,
            seen_keys,
            reachable,
            record_ids: seen_ids,
            reclaimed,
            founding: founding_id,
            authority,
            campaign_id: campaign_id.clone(),
            content_objects,
        };
        Ok((
            ChainSummary {
                head,
                height: head_epoch,
                founding: founding_id,
                authority,
                campaign_id,
                records,
                content_objects,
                checkpoints,
                reclaimed_records,
                reclaimed_keys,
            },
            cursor,
        ))
    }

    /// Advances the cursor over an accepted append (O(1) work).
    pub(super) fn advance_cursor(&mut self, record: &RevisionRecord, id: &Hash) {
        self.advance_cursor_with_closure(record, id, None);
    }

    /// The cursor advance with the optional VERIFIED composite
    /// closure (the K2-C1 corrective): a composite append's chunks
    /// join the reachable set through the admission-verified
    /// manifest, keeping the hot cursor and the cold walk on the
    /// SAME definition of reachability.
    pub(super) fn advance_cursor_with_closure(
        &mut self,
        record: &RevisionRecord,
        id: &Hash,
        composite_closure: Option<&[Hash]>,
    ) {
        let refs = &record.content_refs;
        match self.cursor.as_mut() {
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
                self.cursor = Some(Cursor {
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
}
