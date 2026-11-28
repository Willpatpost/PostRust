//! (CA-17, Q08) THE CONFIRMED-IDENTITY RESIDENCY DESIGN RECORD and
//! its bounded first step — the authenticated cold
//! membership/producer retrieval.
//!
//! ## The finding (verified against source before this module
//! existed)
//!
//! The settled-transaction watermark (P3-L6) bounds WORK, not
//! RESIDENCY. Three structures grow monotonically with confirmed
//! history no matter where the watermark sits:
//!
//! * `CommitVerifier::confirmed` — one `BTreeMap` entry per
//!   chain-confirmed identity, for the session's lifetime AND across
//!   sessions (every open's chain walk re-collects every batch entry
//!   the chain still proves). The coverage shed drops only the
//!   `coverage` Vec; the entry itself — the producing transaction,
//!   the effect commitment, the confirming revision, the epoch — is
//!   retained forever ("the dedup membership is NEVER dropped", the
//!   L6 contract).
//! * `CommitVerifier::shed_coverage` — one `HashSet` slot per shed
//!   identity, unbounded in the shed population.
//! * `CommitVerifier::shed_eligible` — bounded (the documented grace
//!   window), the one already-bounded term.
//!
//! The rotation bounds the JOURNAL side (retired transactions
//! re-anchor into a fresh generation) but not this map: the rotation
//! classification itself consults `is_confirmed`, and the confirmed
//! batches must stay chain-reachable because they ARE the dedup
//! authority — delete one and a re-submitted identity would
//! re-confirm (a double install). The map's population is therefore
//! "every confirmed identity whose batch the chain still proves",
//! which is total confirmed history under the current reclaim
//! policy. The session-open walk that rebuilds it is O(chain) — the
//! honest cost the pipeline documents — and the map it builds is the
//! in-session dedup surface every commit consults.
//!
//! ## Why the metadata cannot simply be dropped
//!
//! Three duties ride on the hot map today, and any eviction must
//! preserve them (the audit's preservation list, each with its
//! load-bearing call sites):
//!
//! * **Duplicate detection**: `is_confirmed` (the rotation's
//!   retirement classification, the actionable scan's filter), the
//!   conflicting-reuse refusal in `CommitAuthority::stage`, and the
//!   exact-provenance comparisons in the actionable scan and the
//!   remaining-keys filter (`ConfirmedView::confirmed`).
//! * **Retry identity**: the retained `PendingAttempt`'s re-filter
//!   over the confirmed view — a retry must see which of its keys
//!   another writer confirmed, with the exact provenance.
//! * **Provenance**: `confirmed_evidence` (the recovery composition's
//!   chain-side input) and the receipts' per-key evidence.
//!
//! ## The cold tier that already exists
//!
//! The chain IS the authenticated cold store: hash-verified records
//! (the medium's `read_record`/`read_object` verify content against
//! identity), gap-aware below reclaimed ranges (the checkpoint
//! metadata's skip tables), every batch entry re-derivable exactly
//! (`walk_chain`'s collection). A cold query for one identity is a
//! targeted walk — [`probe_confirmed`], this module's primitive:
//! the same descent, the same verified reads, the same
//! conflicting-reuse refusal, answered WITHOUT the in-memory map.
//! It is O(chain) per query — the honest cold cost the measurement
//! rows below characterize (that cost is the DESIGN INPUT for the
//! bounded-retrieval decision, not a defect of the primitive).
//!
//! ## The options considered
//!
//! * **(a) Evict + per-query chain walks**: correct, zero new
//!   machinery — but the commit path's duplicate check would pay
//!   O(chain) per actionable key, regressing the per-commit cost the
//!   watermark just bounded. Rejected as the standing shape;
//!   acceptable as the rare path.
//! * **(b) A rotation-authored confirmed ledger**: a content object
//!   written at each rotation boundary carrying the
//!   confirmed-identity → (producing transaction, revision, epoch)
//!   table (the coverage detail stays in the batches — it is what
//!   the shed already trades), hash-chained into the chain itself.
//!   The cold lookup then reads ONE object plus its chain
//!   verification — bounded per generation, re-authored at each
//!   rotation. This aligns with the P3 design's precedent that
//!   REJECTED on-medium indexes for the journal read path
//!   ("duplicates rotation's job, adds a trust surface"): the ledger
//!   is authored AT the rotation — the existing compaction point —
//!   not maintained as a second index.
//! * **(c) A lossy membership filter per generation** (Bloom-class):
//!   compact, but eviction decisions need exactness and every false
//!   positive pays the full walk anyway. Rejected.
//!
//! **THE RECOMMENDATION (staged, design-first):**
//!
//! * **Stage 1 (THIS LANE, the bounded first step):** the
//!   cold-retrieval primitive itself — [`probe_confirmed`] — plus
//!   the residency measurement surface
//!   ([`crate::pipeline::RetentionStats::confirmed_residency_bytes`]
//!   and `shed_residency_bytes`) plus the boundary-query proof: an
//!   identity whose coverage the shed dropped answers the COLD path
//!   with its full chain-proven coverage, closing today's honest
//!   in-session refusal gap (`confirmed_coverage` refuses shed
//!   detail; the cold path re-derives it). No eviction, no behavior
//!   change to any existing path — the map stays exactly as today.
//! * **Stage 2 (a future lane, gated on measurement):** the
//!   rotation-authored ledger (option b) and the eviction of
//!   settle-proven identities beyond a documented grace bound —
//!   eligible exactly when the producing transaction is settled
//!   (the shed's eligibility proof) AND subsumed below the anchor
//!   boundary (the rotation's retirement rule) — with the evicted
//!   map provably equal to what a fresh open re-collects (the
//!   equivalence bar this module's tests establish for the probe).
//!   The gate: the measured cold-walk latency vs the ledger-read
//!   cost at the owner-rig scale.
//! * **Stage 3 (coordination):** the reclaim side — what history
//!   the chain retains decides what cold retrieval can prove; the
//!   skip-table discipline already preserves reachability to
//!   retained batches, and the ledger would carry the membership
//!   past batch reclamation.
//!
//! ## The invariants any eviction must preserve (the test bar)
//!
//! Duplicate detection, retry identity, and provenance across BOTH
//! providers (the primitive is generic over
//! [`CanonicalMedium`]; the tests run the local file medium and the
//! SQLite medium); the fresh-open equivalence (the cold answer IS
//! the fresh walk's answer — pinned here); the fail-closed arms
//! (missing history, conflicting reuse — pinned here, error-shape
//! identical to the walk's); wire-invisibility (the residency
//! surfaces are never serialized, never fingerprint inputs — the
//! `RetentionStats` discipline).
//!
//! ## [ASSUMPTION] markers (the owner decisions the design defers)
//!
//! * The stage-2 grace bound for ENTRY eviction (mirroring
//!   [`crate::pipeline::SETTLED_COVERAGE_RETAINED`]'s 1,024 and its
//!   own pending owner-rig data point).
//! * The ledger's per-generation size budget (the number of
//!   confirmed identities a rotation generation may retain before
//!   the ledger itself needs tiering).
//! * The eviction eligibility's subsumption arm (the anchor
//!   boundary's per-region episodes — the rotation's rule, applied
//!   to map residency).

use crate::checkpoint::{CHECKPOINT_METADATA_DOMAIN, CheckpointMetadata};
use crate::consequence::ConsequenceId;
use crate::hash::Hash;
use crate::medium::{CanonicalMedium, MediumError};
use crate::pipeline::{
    BatchEntry, CONSEQUENCE_BATCH_DOMAIN, ConfirmedEntry, ConsequenceBatch, PipelineError,
};
use crate::record::RevisionRecord;

/// The authenticated cold membership/producer retrieval — the
/// chain-side answer for ONE identity, derived without the in-memory
/// confirmed map: the same gap-aware head-down hash-verified descent
/// [`crate::pipeline::CommitVerifier::walk_chain`] runs, targeted at
/// one `ConsequenceId`.
///
/// * **Authenticated by construction**: every record is read through
///   the medium's hash-verifying `read_record`; every consequence
///   batch through the verified `read_object` + strict decode — the
///   probe trusts nothing the walk itself would not trust.
/// * **The same fail-closed arms as the walk**: a missing parent
///   outside a skip table's sanctioned gaps refuses
///   `MissingObject`; a second chain revision claiming the identity
///   refuses `CanonicalIdentityConflict` with the SAME
///   first-is-older/second-is-newer shape the walk's insert check
///   produces (the probe never guesses which claim is
///   authoritative).
/// * **The fresh-open answer**: the entry returned is exactly the
///   entry a freshly opened pipeline's session walk would collect
///   for this identity — the eviction-boundary query surface.
/// * **O(chain) per query** — every batch object on the chain is
///   verified and decoded (the conflict arm requires the full
///   descent). This is the honest cold cost the design record's
///   stage-2 gate measures; the working set stays bounded (one batch
///   object at a time, never the chain materialized).
pub(crate) fn probe_confirmed<M: CanonicalMedium>(
    medium: &mut M,
    id: ConsequenceId,
) -> Result<Option<ConfirmedEntry>, PipelineError> {
    let Some(head) = medium.head()? else {
        // An unfounded medium confirms nothing — the walk's own
        // graceful shape.
        return Ok(None);
    };
    // The descent is head-down (newest first), so the FIRST claim
    // met is the NEWEST. The conflict arm reports the walk's shape —
    // first_revision the OLDER claim, second_revision the NEWER —
    // and errors eagerly at the second claim (the walk's insert
    // check fires at the first duplicate; a third can never matter).
    let mut newest_claim: Option<(Hash, u64, BatchEntry)> = None;
    let mut cursor = head;
    // The compacted-mode skip table (PP-B3): an absent parent is a
    // gap legitimate only under the most recent on-chain MK's skip
    // table — the walk's rule, mirrored exactly (owner/self-skip
    // selection included).
    let mut skip_table: Option<(Hash, CheckpointMetadata)> = None;
    loop {
        let record: RevisionRecord = medium.read_record(&cursor)?;
        let parent = record.parent;
        let current = cursor;
        // The checkpoint-metadata detection runs on the record's
        // FIRST content object (the walk's placement).
        if let Some(object) = record.content_refs.first() {
            let class = medium.classify_object(object)?;
            if class.domain.as_deref() == Some(CHECKPOINT_METADATA_DOMAIN) {
                let bytes = medium.read_object(object)?;
                let metadata = CheckpointMetadata::decode(&bytes).map_err(PipelineError::Format)?;
                skip_table = Some((current, metadata));
            }
        }
        // The batch collection runs over ALL content refs — the
        // store is domain-agnostic; only consequence batches carry
        // keys.
        for content in &record.content_refs {
            let class = medium.classify_object(content)?;
            if class.domain.as_deref() != Some(CONSEQUENCE_BATCH_DOMAIN) {
                continue;
            }
            let bytes = medium.read_object(content)?;
            let batch = ConsequenceBatch::decode(&bytes).map_err(PipelineError::Format)?;
            for entry in batch.entries {
                if entry.id != id {
                    continue;
                }
                if let Some((newer_revision, _, _)) = &newest_claim {
                    return Err(PipelineError::CanonicalIdentityConflict {
                        id,
                        // The OLDER claim (this one, met second in
                        // the descent) is the walk's
                        // `first_revision`.
                        first_revision: current,
                        // The NEWER claim (met first) is the walk's
                        // `second_revision`.
                        second_revision: *newer_revision,
                    });
                }
                newest_claim = Some((current, record.epoch, entry));
            }
        }
        match parent {
            Some(parent) => match medium.read_record(&parent) {
                Ok(_) => cursor = parent,
                Err(MediumError::MissingObject { .. }) => {
                    let Some((owner, table)) = skip_table.as_ref() else {
                        return Err(PipelineError::Medium(MediumError::MissingObject {
                            hash: parent,
                        }));
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
                        return Err(PipelineError::Medium(MediumError::MissingObject {
                            hash: parent,
                        }));
                    };
                    cursor = target;
                }
                Err(other) => return Err(other.into()),
            },
            None => break,
        }
    }
    Ok(newest_claim.map(|(revision, epoch, entry)| ConfirmedEntry {
        coverage: entry.coverage,
        transaction: entry.transaction,
        effect: entry.effect,
        revision,
        epoch,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consequence::{AwardCheckpoint, Coverage, OccurrenceSpan};
    use crate::envelope::{
        ClassifiedSpan, ConsequenceEnvelope, CounterValues, DurabilityClass, SignalLane,
        SourceBaseline, SourcePosition,
    };
    use crate::journal::{FrontierEntry, T1Journal, TransactionId};
    use crate::local::{LocalFileMedium, OBJECTS_DIR};
    use crate::medium::{AppendOutcome, CanonicalMedium, StoredObject};
    use crate::pipeline::{
        CommitOutcome, CommitPipeline, PipelineError, SETTLED_COVERAGE_RETAINED,
    };
    use crate::record::{AuthorityMode, RevisionRecord};
    use crate::sqlite::SqliteMedium;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let seq = SEQ.fetch_add(1, Ordering::SeqCst);
            let path =
                std::env::temp_dir().join(format!("s7-ca17-{}-{label}-{seq}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create test root");
            TestDir(path)
        }

        fn store(&self) -> PathBuf {
            self.0.join("store")
        }

        fn db(&self) -> PathBuf {
            self.0.join("store.db")
        }

        fn journal(&self) -> PathBuf {
            self.0.join("journal.t1")
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// One staged envelope for the bulk fixtures — the pipeline
    /// tests' shape, parameterized for unique identities.
    fn bulk_envelope(episode: u64, npc: u64) -> ConsequenceEnvelope {
        ConsequenceEnvelope::capture(
            ConsequenceId::new(1, episode, npc as u32),
            vec![ClassifiedSpan {
                span: OccurrenceSpan::new(2 * (npc - 1), 2 * npc).unwrap(),
                class: DurabilityClass::DurableEligible,
            }],
            SignalLane::Captured,
            CounterValues::default(),
            SourceBaseline {
                counters: CounterValues::default(),
                award: AwardCheckpoint {
                    durable: Coverage::new(),
                    provisional: Coverage::new(),
                },
            },
            SourcePosition {
                head: None,
                tick: 100 + episode,
            },
        )
        .unwrap()
    }

    /// The capture boundary for a bulk batch (the pipeline tests'
    /// boundary shape).
    fn bulk_boundary(
        episode: u64,
    ) -> (
        SourcePosition,
        DurabilityClass,
        Vec<TransactionId>,
        Vec<FrontierEntry>,
        Vec<u8>,
    ) {
        (
            SourcePosition {
                head: None,
                tick: 100 + episode,
            },
            DurabilityClass::DurableEligible,
            Vec::new(),
            vec![FrontierEntry { region: 1, episode }],
            format!("plan-{episode}").into_bytes(),
        )
    }

    /// Commits one transaction of `per_batch` identities at `episode`
    /// — the growing-history primitive both providers drive.
    fn commit_batch<M: CanonicalMedium>(
        pipeline: &mut CommitPipeline<M>,
        episode: u64,
        per_batch: u64,
    ) {
        for npc in 1..=per_batch {
            pipeline.stage(bulk_envelope(episode, npc)).unwrap();
        }
        let (position, classification, dependencies, frontier, plan) = bulk_boundary(episode);
        pipeline
            .capture(position, classification, dependencies, frontier, plan)
            .unwrap();
        let CommitOutcome::Confirmed(receipt) = pipeline.commit_canonical().unwrap() else {
            panic!("the bulk batch {episode} must confirm");
        };
        assert_eq!(receipt.keys.len(), per_batch as usize);
    }

    /// Stages-but-does-not-commit one live unsettled batch (the fixed
    /// live obligation: journaled, never confirmed this session).
    fn stage_live_unsettled<M: CanonicalMedium>(pipeline: &mut CommitPipeline<M>, episode: u64) {
        pipeline.stage(bulk_envelope(episode, 1)).unwrap();
        let (position, classification, dependencies, frontier, plan) = bulk_boundary(episode);
        pipeline
            .capture(position, classification, dependencies, frontier, plan)
            .unwrap();
    }

    fn local_pipeline(dir: &TestDir) -> CommitPipeline<LocalFileMedium> {
        let mut medium = LocalFileMedium::open(dir.store()).unwrap();
        if medium.head().unwrap().is_none() {
            let founding = RevisionRecord::founding(AuthorityMode::Standalone, "ca17-campaign", 0);
            match medium.append(&founding, &[], None).unwrap() {
                AppendOutcome::Accepted { .. } => {}
                other => panic!("the founding must accept: {other:?}"),
            }
        }
        drop(medium);
        let journal = T1Journal::open(dir.journal()).unwrap();
        CommitPipeline::open(LocalFileMedium::open(dir.store()).unwrap(), journal).unwrap()
    }

    fn sqlite_pipeline(dir: &TestDir) -> CommitPipeline<SqliteMedium> {
        let mut medium = SqliteMedium::open(dir.db()).unwrap();
        if medium.head().unwrap().is_none() {
            let founding = RevisionRecord::founding(AuthorityMode::Standalone, "ca17-campaign", 0);
            match medium.append(&founding, &[], None).unwrap() {
                AppendOutcome::Accepted { .. } => {}
                other => panic!("the founding must accept: {other:?}"),
            }
        }
        drop(medium);
        let journal = T1Journal::open(dir.journal()).unwrap();
        CommitPipeline::open(SqliteMedium::open(dir.db()).unwrap(), journal).unwrap()
    }

    /// The equivalence assertion: the cold probe's answer equals the
    /// hot map's answer (and therefore the fresh walk's — the hot map
    /// IS the fresh walk's collection within a session that has
    /// confirmed nothing foreign).
    fn assert_cold_equals_hot<M: CanonicalMedium>(
        pipeline: &mut CommitPipeline<M>,
        id: ConsequenceId,
    ) {
        let cold = pipeline.cold_confirmed(id).unwrap();
        let hot = pipeline
            .confirmed_evidence()
            .into_iter()
            .find(|(evidence_id, _)| *evidence_id == id)
            .map(|(_, evidence)| (evidence.coverage, evidence.transaction, evidence.effect));
        match (cold, hot) {
            (None, None) => {}
            (Some(cold), Some((coverage, transaction, effect))) => {
                assert_eq!(cold.coverage, coverage, "the coverage agrees for {id:?}");
                assert_eq!(
                    cold.transaction, transaction,
                    "the producer agrees for {id:?}"
                );
                assert_eq!(
                    cold.effect, effect,
                    "the effect commitment agrees for {id:?}"
                );
            }
            (cold, hot) => panic!("the cold/hot disagreement for {id:?}: {cold:?} vs {hot:?}"),
        }
    }

    /// (CA-17, test 1) THE EQUIVALENCE BATTERY, BOTH PROVIDERS: over
    /// a growing history, the cold probe answers exactly what the hot
    /// map holds — coverage, producing transaction, effect
    /// commitment — and an absent identity answers None on both
    /// paths. The local file medium and the SQLite medium run the
    /// same battery (the "both providers" preservation duty).
    #[test]
    fn cold_probe_agrees_with_the_hot_map_on_both_providers() {
        let dir = TestDir::new("equivalence-local");
        let mut pipeline = local_pipeline(&dir);
        for episode in 1..=3u64 {
            commit_batch(&mut pipeline, episode, 40);
        }
        for episode in 1..=3u64 {
            for npc in [1u64, 20, 40] {
                assert_cold_equals_hot(&mut pipeline, ConsequenceId::new(1, episode, npc as u32));
            }
        }
        assert_cold_equals_hot(&mut pipeline, ConsequenceId::new(1, 99, 1));
        drop(pipeline);

        let dir = TestDir::new("equivalence-sqlite");
        let mut pipeline = sqlite_pipeline(&dir);
        for episode in 1..=3u64 {
            commit_batch(&mut pipeline, episode, 40);
        }
        for episode in 1..=3u64 {
            for npc in [1u64, 20, 40] {
                assert_cold_equals_hot(&mut pipeline, ConsequenceId::new(1, episode, npc as u32));
            }
        }
        assert_cold_equals_hot(&mut pipeline, ConsequenceId::new(1, 99, 1));
    }

    /// (CA-17, test 2) PARTIAL SETTLEMENT: a history with confirmed
    /// transactions AND a live unsettled one — the probe answers the
    /// confirmed identities and refuses the journaled-only identity
    /// (None, matching the hot map: the journal is not the chain).
    #[test]
    fn cold_probe_over_partial_settlement() {
        let dir = TestDir::new("partial-settlement");
        let mut pipeline = local_pipeline(&dir);
        commit_batch(&mut pipeline, 1, 30);
        commit_batch(&mut pipeline, 2, 30);
        stage_live_unsettled(&mut pipeline, 3);
        let confirmed = ConsequenceId::new(1, 1, 1);
        let unsettled = ConsequenceId::new(1, 3, 1);
        assert!(pipeline.cold_confirmed(confirmed).unwrap().is_some());
        assert!(pipeline.cold_confirmed(unsettled).unwrap().is_none());
        assert!(pipeline.is_confirmed(confirmed));
        assert!(!pipeline.is_confirmed(unsettled));
        assert_cold_equals_hot(&mut pipeline, confirmed);
        assert_cold_equals_hot(&mut pipeline, unsettled);
    }

    /// (CA-17, test 3) CRASH/REOPEN: the probe's answer survives the
    /// session boundary — the reopened pipeline (a fresh walk, the
    /// crash-recovery shape) probes identically to the pre-drop
    /// session, and the hot map agrees on both sides.
    #[test]
    fn cold_probe_survives_crash_and_reopen() {
        let dir = TestDir::new("crash-reopen");
        let mut pipeline = local_pipeline(&dir);
        commit_batch(&mut pipeline, 1, 25);
        commit_batch(&mut pipeline, 2, 25);
        let before = pipeline
            .cold_confirmed(ConsequenceId::new(1, 2, 7))
            .unwrap();
        drop(pipeline);
        let mut reopened = local_pipeline(&dir);
        let after = reopened
            .cold_confirmed(ConsequenceId::new(1, 2, 7))
            .unwrap();
        assert_eq!(
            before, after,
            "the cold answer is session-independent (the chain is the authority)"
        );
        assert_cold_equals_hot(&mut reopened, ConsequenceId::new(1, 2, 7));
    }

    /// (CA-17, test 4) MISSING HISTORY, FAIL-CLOSED: an identity the
    /// chain never confirmed answers None; an unfounded medium
    /// answers None gracefully; a chain whose founding record was
    /// destroyed refuses typed — the walk's own fail-closed arm,
    /// never a silent None over corrupt history.
    #[test]
    fn cold_probe_on_missing_history_fails_closed() {
        let dir = TestDir::new("missing-history");
        let mut pipeline = local_pipeline(&dir);
        commit_batch(&mut pipeline, 1, 20);
        assert!(
            pipeline
                .cold_confirmed(ConsequenceId::new(1, 50, 1))
                .unwrap()
                .is_none()
        );
        drop(pipeline);
        // An unfounded medium confirms nothing.
        let fresh = TestDir::new("unfounded");
        let mut unfounded = local_pipeline(&fresh);
        assert!(
            unfounded
                .cold_confirmed(ConsequenceId::new(1, 1, 1))
                .unwrap()
                .is_none()
        );
        drop(unfounded);
        // A destroyed founding record: the descent cannot reach the
        // root and refuses (never a silent None over a broken
        // chain).
        let broken = TestDir::new("broken-chain");
        let mut broken_pipeline = local_pipeline(&broken);
        commit_batch(&mut broken_pipeline, 1, 20);
        drop(broken_pipeline);
        {
            let mut medium = LocalFileMedium::open(broken.store()).unwrap();
            // Walk to the founding record and destroy its object
            // file (the corruption class the walks fail closed on).
            let mut cursor = medium.head().unwrap().unwrap();
            loop {
                let record = medium.read_record(&cursor).unwrap();
                match record.parent {
                    Some(parent) => cursor = parent,
                    None => break,
                }
            }
            let founding_object = broken.store().join(OBJECTS_DIR).join(cursor.hex());
            std::fs::remove_file(&founding_object).unwrap();
            // (A pipeline OPEN over this medium refuses at the walk
            // for the same reason — the corruption class fails every
            // descent. The probe is driven on the medium handle
            // directly: the shape a cold-tier consumer holds.)
            assert!(
                matches!(
                    probe_confirmed(&mut medium, ConsequenceId::new(1, 1, 1)),
                    Err(PipelineError::Medium(MediumError::MissingObject { .. }))
                        | Err(PipelineError::Medium(
                            MediumError::ReferencedObjectCorrupt { .. }
                        ))
                ),
                "the destroyed founding refuses typed, never a silent None"
            );
        }
    }

    /// (CA-17, test 5) THE QUERY ACROSS THE EVICTION BOUNDARY — the
    /// payoff: past the coverage shed's documented bound, an
    /// in-session `confirmed_coverage` query REFUSES the shed detail
    /// (today's honest contract — it re-derives only at the next
    /// open), while the COLD path answers it NOW with the full
    /// chain-proven coverage. The L6 shed fixture's shape (1,400
    /// settled identities against the 1,024 bound).
    #[test]
    fn cold_probe_answers_across_the_shed_eviction_boundary() {
        let dir = TestDir::new("shed-boundary");
        let mut pipeline = local_pipeline(&dir);
        const PER_TRANSACTION: u64 = 140;
        const TRANSACTIONS: u64 = 10;
        const TOTAL: u64 = PER_TRANSACTION * TRANSACTIONS;
        for episode in 1..=TRANSACTIONS {
            commit_batch(&mut pipeline, episode, PER_TRANSACTION);
        }
        // The last transaction settles at the next scan.
        assert_eq!(
            pipeline.commit_canonical().unwrap(),
            CommitOutcome::NothingPending
        );
        let stats = pipeline.retention_stats();
        assert_eq!(stats.confirmed_keys, TOTAL);
        assert_eq!(
            stats.shed_coverage_entries,
            TOTAL - SETTLED_COVERAGE_RETAINED as u64,
            "the L6 bound drove the shed (376 identities)"
        );
        // A SHED identity (transaction 1's batch — the FIFO's front):
        // in-session coverage refuses, the cold path re-derives the
        // full chain-proven entry.
        let shed_id = ConsequenceId::new(1, 1, 1);
        assert!(pipeline.is_confirmed(shed_id), "the membership never drops");
        assert_eq!(
            pipeline.confirmed_coverage(shed_id),
            None,
            "the shed detail refuses in-session (the L6 contract)"
        );
        let cold = pipeline
            .cold_confirmed(shed_id)
            .expect("the cold probe walks clean")
            .expect("the shed identity is chain-confirmed");
        assert_eq!(
            cold.coverage,
            vec![ClassifiedSpan {
                span: OccurrenceSpan::new(0, 2).unwrap(),
                class: DurabilityClass::DurableEligible,
            }],
            "the cold path returns the FULL coverage across the boundary"
        );
        // The measurement surface reports both growth terms.
        let stats = pipeline.retention_stats();
        assert!(stats.confirmed_residency_bytes > 0);
        assert!(stats.shed_residency_bytes > 0);
        // A RETAINED identity (the newest settlement): hot and cold
        // agree with full coverage.
        let retained_id = ConsequenceId::new(1, TRANSACTIONS, 1);
        assert!(pipeline.confirmed_coverage(retained_id).is_some());
        assert_cold_equals_hot(&mut pipeline, retained_id);
    }

    /// (CA-17, test 6) THE CONFLICT ARM, ERROR-SHAPE EQUIVALENT: a
    /// chain carrying two revisions claiming one identity —
    /// hand-appended past the pipeline (the medium cannot refuse it:
    /// the record idempotency key differs) — refuses the cold probe
    /// with EXACTLY the error a fresh pipeline open's walk produces
    /// (the same variant, the same older-is-first/ newer-is-second
    /// revision pair).
    #[test]
    fn cold_probe_refuses_the_conflicting_reuse_exactly_as_the_walk() {
        let dir = TestDir::new("conflicting-reuse");
        let mut pipeline = local_pipeline(&dir);
        commit_batch(&mut pipeline, 1, 20);
        drop(pipeline);
        // The foreign conflicting revision: a hand-built batch
        // claiming an already-confirmed identity, appended directly
        // to the medium under a fresh idempotency key.
        let target = ConsequenceId::new(1, 1, 1);
        let foreign = ConsequenceBatch {
            entries: vec![BatchEntry {
                id: target,
                transaction: TransactionId(crate::hash::hash_object(b"foreign-producer")),
                coverage: vec![ClassifiedSpan {
                    span: OccurrenceSpan::new(0, 2).unwrap(),
                    class: DurabilityClass::DurableEligible,
                }],
                effect: crate::hash::hash_object(b"foreign-effect"),
            }],
        };
        let object = StoredObject::new(foreign.canonical_bytes().unwrap()).unwrap();
        let mut medium = LocalFileMedium::open(dir.store()).unwrap();
        let parent = medium.head().unwrap().unwrap();
        let parent_epoch = medium.read_record(&parent).unwrap().epoch;
        let record = RevisionRecord::regular(parent, parent_epoch, 999)
            .with_idempotency_key(b"ca17-foreign-conflict".to_vec())
            .with_content_refs(vec![object.hash()]);
        let foreign_revision = match medium
            .append(&record, std::slice::from_ref(&object), Some(parent))
            .unwrap()
        {
            AppendOutcome::Accepted { id, .. } => id,
            other => panic!("the foreign revision must accept: {other:?}"),
        };
        // (A pipeline OPEN over this medium refuses at the walk — the
        // conflict arm below pins that; the probe is driven on the
        // medium handle directly.)
        let conflict =
            probe_confirmed(&mut medium, target).expect_err("the conflicting chain refuses");
        let PipelineError::CanonicalIdentityConflict {
            first_revision,
            second_revision,
            ..
        } = conflict
        else {
            panic!("the conflict must be CanonicalIdentityConflict, got {conflict:?}")
        };
        // The OLDER (original, legitimate) claim is first: the same
        // revision that confirms the target's sibling identity.
        let older = probe_confirmed(&mut medium, ConsequenceId::new(1, 1, 2))
            .unwrap()
            .map(|entry| entry.revision);
        assert_eq!(Some(first_revision), older);
        // The hand-appended NEWER claim is second — the walk's
        // ordering.
        assert_eq!(second_revision, foreign_revision);
        drop(medium);
        // The fresh open's walk refuses the SAME chain the same way
        // (the equivalence of the fail-closed arms) — reopening over
        // the corrupted chain cannot even construct.
        let journal = T1Journal::open(dir.journal()).unwrap();
        let walk = CommitPipeline::open(LocalFileMedium::open(dir.store()).unwrap(), journal);
        match walk {
            Err(PipelineError::CanonicalIdentityConflict { .. }) => {}
            _ => panic!("the fresh walk must refuse the conflicting chain"),
        }
    }

    /// (CA-17, test 7) THE MEASUREMENT ROW (pod-observational): the
    /// resident-bytes estimate at FIXED live obligations (eight
    /// staged-unsettled envelopes live at every scale) over growing
    /// confirmed histories — linear in the history (the finding's
    /// measured shape), with the cold-lookup and hot-lookup latencies
    /// printed as the design record's stage-2 gate input.
    #[test]
    fn residency_grows_linearly_with_history_at_fixed_live_obligations() {
        let mut rows: Vec<(u64, u64, u64)> = Vec::new();
        for history in [512u64, 2048, 8192] {
            let dir = TestDir::new(format!("residency-{history}").as_str());
            let mut pipeline = local_pipeline(&dir);
            const PER_BATCH: u64 = 256;
            let batches = history / PER_BATCH;
            for episode in 1..=batches {
                commit_batch(&mut pipeline, episode, PER_BATCH);
            }
            // The fixed live obligation: eight staged, journaled,
            // never-confirmed envelopes held at every scale.
            for live in 0..8u64 {
                pipeline.stage(bulk_envelope(10_000 + live, 1)).unwrap();
            }
            let (position, classification, dependencies, frontier, plan) = bulk_boundary(10_000);
            pipeline
                .capture(position, classification, dependencies, frontier, plan)
                .unwrap();
            let stats = pipeline.retention_stats();
            assert_eq!(stats.confirmed_keys, history);
            rows.push((
                history,
                stats.confirmed_residency_bytes,
                stats.unsettled_transactions,
            ));
            // The latency pair (printed, never pinned — the
            // pod-observational discipline): one cold probe against
            // one thousand hot lookups at this scale.
            let target = ConsequenceId::new(1, batches, 128);
            let cold_start = Instant::now();
            let cold_answer = pipeline.cold_confirmed(target).unwrap();
            let cold_elapsed = cold_start.elapsed();
            assert!(cold_answer.is_some());
            let hot_start = Instant::now();
            let mut hits = 0;
            for _ in 0..1_000 {
                if pipeline.is_confirmed(target) {
                    hits += 1;
                }
            }
            let hot_elapsed = hot_start.elapsed();
            assert_eq!(hits, 1_000);
            println!(
                "ca17 measurement row: history {history} confirmed identities, 8 live \
                 unsettled — residency {} B (~{} B/identity), cold lookup {:?}, 1000 hot \
                 lookups {:?}",
                stats.confirmed_residency_bytes,
                stats.confirmed_residency_bytes / history,
                cold_elapsed,
                hot_elapsed
            );
        }
        // THE FIXED LIVE OBLIGATION: the unsettled window (the staged
        // live batch plus at most the last committed transaction
        // awaiting its next scan) is IDENTICAL at every scale — the
        // growth measured below is the history term alone.
        assert_eq!(
            rows[0].2, rows[1].2,
            "the live window is fixed across scales"
        );
        assert_eq!(
            rows[1].2, rows[2].2,
            "the live window is fixed across scales"
        );
        // THE STRUCTURAL CLAIM: linear growth in history at fixed
        // live obligations (the 4x history steps must scale the
        // residency within the estimate's documented variance — no
        // superlinear term, no fixed term dominating).
        let (_, b1, _) = rows[0];
        let (h2, b2, _) = rows[1];
        let (_, b3, _) = rows[2];
        let ratio12 = b2 as f64 / b1 as f64;
        let ratio23 = b3 as f64 / b2 as f64;
        // The history steps are 4x by construction (512 → 2048 →
        // 8192).
        let history_ratio = h2 as f64 / rows[0].0 as f64;
        assert!(
            ratio12 > history_ratio * 0.8 && ratio12 < history_ratio * 1.4,
            "the 512→2048 residency ratio {ratio12:.2} must track the history ratio \
             {history_ratio:.2} (linear)"
        );
        assert!(
            ratio23 > history_ratio * 0.8 && ratio23 < history_ratio * 1.4,
            "the 2048→8192 residency ratio {ratio23:.2} must track the history ratio \
             {history_ratio:.2} (linear)"
        );
    }
}
