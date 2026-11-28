//! Stage 5 Batch 2: the content-addressed revision store and local
//! canonical medium (the accepted v1.8 architecture's §5.3 nine-step
//! CAS protocol, §5.3.2 validation cost model, and §5.3.3 v1.4 orphan
//! protocol; the implementation record is
//! `docs/STAGE5-B2-STORAGE-RECORD.md`).
//!
//! **Dependency direction (ratified):** `engine-core → engine-persist
//! → application` — this crate uses the Batch 1 wire primitives and
//! adds the single approved `sha2` dependency; engine-core stays
//! zero-dependency and is never reversed into persistence.
//!
//! **What lives here:** SHA-256 content addressing with structural
//! domain separation; the versioned hash-chained revision record;
//! immutable insert-once content-addressed objects with
//! verify-on-retry and the quarantine-never-delete orphan protocol;
//! the [`CanonicalMedium`] boundary with the three distinct append
//! outcomes — `Accepted` (carrying the platform's honest durability
//! tier), `RejectedStaleParent`, and `Unknown` (reconciled under the
//! lock, never guessed); and [`LocalFileMedium`] — the OS-managed
//! `HEAD.lock`, the trusted-cursor validation (O(1) walk work per
//! straightforward append, cold founding → HEAD walk at session open,
//! suffix walks on divergence, fail-closed on non-descendant
//! histories), and the crash-window fault seam the real-process test
//! matrix drives.
//!
//! **What deliberately does not live here (B3+):** the five-state
//! gameplay CommitPipeline, T1 encounter journals, NpcWorld/
//! RegionDirector codecs, networking, compaction/retention, and any
//! asynchronous storage. SHA-256 here is object integrity, never
//! signer authentication.
//!
//! **Windows durability (the central review risk):** the custom
//! provider is qualified for the process-crash tier only (the
//! documented std file-fsync + rename sequence); acknowledged-revision
//! persistence across OS crash or sudden power loss is **not claimed
//! from std** — the N−1-valid/N−lost scenario governs, and SQLite
//! remains the identified production fallback (an evidence-based
//! comparison is delivered at the Batch 2 review; the lead selects).
//!
//! **The `sqlite` Cargo feature (D-EE-10 item 4, owner APPROVED
//! 2026-11-27):** the SQLite provider is an OPTIONAL dependency
//! behind the `sqlite` feature — **ENABLED BY DEFAULT**, so ordinary
//! consumers retain the full functionality unchanged (the
//! `SqliteMedium` provider, the §6.1 migration, the backup capture).
//! A `--no-default-features` build is **LOCAL-ONLY**: `rusqlite`
//! and the bundled SQLite C compile (the F-02 measured cost) never
//! resolve, and the type surface contracts to the local provider —
//! the [`sqlite`] module (with [`SqliteMedium`],
//! [`SqliteMediumConfig`], and the fault seam), the [`migrate`]
//! module (with `migrate_local_to_sqlite` and its report types),
//! and the [`MediumError::Sqlite`] / [`MediumError::MigrationPending`]
//! variants are ABSENT from such a build (a compile-time fact, not
//! a runtime fallback). Every operation whose REQUIREMENT is the
//! SQLite provider — the store-location selection naming a
//! database, the §6.1 migration, a stronger durability tier that
//! only the transactional provider grants — fails visibly with the
//! typed [`MediumError::SqliteNotBuilt`] refusal naming the missing
//! feature: **NEVER a silent downgrade to the local file medium**
//! (the weaker durability tier can never stand in for the stronger
//! one the caller demanded).

pub mod ancestry_accounting;
pub mod ancestry_history;
pub mod ancestry_witness;
pub mod batch_completion;
mod chain_walk;
pub mod checkpoint;
pub mod composite_append;
pub mod composite_manifest;
pub mod composite_reader;
pub mod consequence;
pub mod dark_reachability;
pub mod encounter;
pub mod envelope;
pub mod fault;
pub mod hash;
pub mod journal;
pub mod local;
pub mod medium;
// (D-EE-10 item 4) The §6.1 legacy→SQLite migration: its target
// side constructs the SQLite provider, so the whole module —
// `migrate_local_to_sqlite`, `copy_legacy_chain`,
// `cutover_seal_activate`, `archive_legacy`, and the report types —
// exists only under the `sqlite` feature (DEFAULT ON; a
// --no-default-features build is local-only and the migration is a
// compile-time absence, never a runtime fallback).
#[cfg(feature = "sqlite")]
pub mod migrate;
pub mod pipeline;
pub mod range_completion;
pub mod range_installation;
pub mod reclaim;
pub mod record;
pub mod recovery;
pub mod recovery_classification;
pub mod recovery_discovery;
pub mod residency;
pub mod rotation;
pub mod session_anchor;
// D-EE-10 item 4: the sqlite provider and the §6.1 migration (its
// target side IS a SqliteMedium) exist only under the `sqlite`
// feature — DEFAULT ON; a --no-default-features build is local-only
// (the absent-type surface is documented in the crate header, and
// every sqlite-requiring operation fails visibly with the typed
// MediumError::SqliteNotBuilt refusal, never a silent downgrade).
#[cfg(feature = "sqlite")]
pub mod sqlite;
pub mod worker_stats;

#[cfg(test)]
mod tests;
// The sqlite-touching test batteries (D-EE-10 item 4): the sqlite
// parity battery, the migration battery, and the local-vs-sqlite
// walk differential all require the provider; the crate-root test
// mod `tests` (the local medium's own battery) runs in BOTH
// configurations.
#[cfg(all(test, feature = "sqlite"))]
mod migrate_tests;
#[cfg(all(test, feature = "sqlite"))]
mod sqlite_tests;
#[cfg(all(test, feature = "sqlite"))]
mod walk_differential;

pub use ancestry_accounting::{
    AccountingCounts, AccountingLimitations, AccountingLimits, Classification,
    CollisionClassification, DeeperHistoryScope, DeferredIdentity, LiveCoordination,
    MAX_ACCOUNTING_CLASSIFIED_LIST, MAX_ACCOUNTING_COLLISION_LIST, MAX_ACCOUNTING_PROTECTOR_LIST,
    MAX_ACCOUNTING_RUN_BUFFER_ENTRIES, MAX_ACCOUNTING_RUN_COUNT,
    MAX_ACCOUNTING_SCRATCH_BYTE_CEILING, MAX_ACCOUNTING_SHARED_LIST, MandatoryIdentity,
    NamespaceCollision, PriorBatchCompletionTrust, PriorBatchPremise, ProtectorKind, ProtectorRef,
    RetirementAccounting, ScratchReport, SharedReference, StorageRole, SurvivorRootSummary,
    account_batch_retirement,
};
pub use ancestry_history::{
    BatchPosition, HistoricalStep, HistoryWalkSummary, walk_authenticated_history,
    walk_authenticated_history_bounded,
};
pub use ancestry_witness::{
    AncestryWitnessManifest, AncestryWitnessShard, CompleteWitnessPublication,
    InstalledWitnessVerification, MAX_WITNESS_MEMBER_PREIMAGE_BYTES, RECLAMATION_WITNESS_FAMILY,
    RECLAMATION_WITNESS_KEY_DOMAIN, RECLAMATION_WITNESS_MANIFEST_DOMAIN,
    RECLAMATION_WITNESS_MANIFEST_VERSION, RECLAMATION_WITNESS_SHARD_DOMAIN,
    RECLAMATION_WITNESS_SHARD_KEY_DOMAIN, RECLAMATION_WITNESS_SHARD_VERSION,
    WITNESS_ENTRY_FRAMING_BYTES, WITNESS_SHARD_HEADER_BYTES, WITNESS_SHARD_PAYLOAD_CEILING,
    WitnessEntry, WitnessPublicationOutcome, authenticate_against_index, canonical_member_order,
    construct_witness, enforce_ancestry_witness_family_gate, find_installed_witness_manifest,
    find_installed_witness_shard, is_reclamation_ancestry_witness_family, publish_ancestry_witness,
    reclamation_witness_manifest_idempotency_key, reclamation_witness_shard_idempotency_key,
    validate_index_shape, validate_parent_edges_standalone, verify_exact_coverage,
    verify_installed_ancestry_witness, verify_witness_standalone, witness_coverage_digest,
};
pub use batch_completion::{
    BATCH_CLASSIFICATION_COMMITMENT_MAGIC, BATCH_CLASSIFICATION_COMMITMENT_VERSION,
    BatchClassificationCommitment, BatchCompletionContext, BatchCompletionMarkerV2,
    CLASSIFICATION_ENTRY_BYTES, ClassificationCommitmentBuilder, ClassificationCounters,
    ClassificationEntry, EntryClassification, MAX_V2_BATCH_CLASSIFICATION_ENTRIES,
    RECLAMATION_COMPLETE_FAMILY, RECLAMATION_COMPLETE_V2_DOMAIN,
    RECLAMATION_COMPLETE_V2_KEY_DOMAIN, RECLAMATION_COMPLETE_V2_VERSION, StructuralBatchCandidate,
    commit_classification, is_reclamation_complete_family, read_structural_batch_candidate,
    reclamation_complete_v2_idempotency_key,
};
pub use checkpoint::{
    CHECKPOINT_METADATA_DOMAIN, CHECKPOINT_METADATA_VERSION, CHECKPOINT_MK_KEY_DOMAIN,
    CheckpointMetadata, ReclaimedIndexEntry, SkipEntry, chain_range_digest,
    checkpoint_mk_idempotency_key, compute_digest_seed, compute_range_digest,
    publish_checkpoint_mk,
};
pub use composite_append::{CompositeAdmissionError, CompositeAppend};
pub use composite_reader::{
    CompositeReadError, ObjectSource, ObjectSourceError, ReadLimits, VerificationMemo,
    VerifiedChunk, VerifiedComposite, read_composite_closure, read_composite_closure_with_memo,
};
pub use consequence::{
    AwardCheckpoint, AwardState, ConsequenceId, Coverage, OccurrenceSpan, PackedKeyError, SpanError,
};
pub use dark_reachability::{
    LiveSet, ReachError, ReachabilityLimits, RetentionReason, RetirementCandidate,
    RetirementEligibility, live_set,
};
pub use encounter::{EncounterError, EncounterJournal};
pub use envelope::{
    ClassifiedSpan, ConsequenceEnvelope, CounterDelta, CounterValues, DurabilityClass,
    PartialApplication, SignalLane, SourceBaseline, SourcePosition,
};
pub use fault::{FaultAction, FaultControl, FaultPoint};
pub use hash::Hash;
pub use journal::{
    CancellationReason, CommitStage, CommittedTransaction, FrontierEntry, JournalError,
    JournalFaultControl, JournalFaultPoint, JournalReceipt, JournalScan, RecordedCancellation,
    T1Journal, TailKind, TailState, TransactionDurability, TransactionId, TransactionInput,
    WriteReconciliation,
};
pub use local::{AppendTimings, LocalFileMedium, WalkStats};
pub use medium::{
    AppendOutcome, CanonicalMedium, ChainSummary, MediumError, ObjectClass, Reconciliation,
    StoredObject, SyncTier, UnknownReason,
};
#[cfg(feature = "sqlite")]
pub use migrate::{
    CopiedChain, MigrationError, MigrationReport, archive_legacy, copy_legacy_chain,
    cutover_seal_activate, migrate_local_to_sqlite,
};
pub use pipeline::{
    AttemptedKey, BatchEntry, CONSEQUENCE_BATCH_DOMAIN, CaptureReceipt, CommitAuthority,
    CommitOutcome, CommitPipeline, CommitRequest, CommitVerifier, ConfirmationLabel,
    ConfirmationReceipt, ConfirmedEntry, ConfirmedView, ConsequenceBatch, KeyPhase, KeyStatus,
    PipelineError, ReconcileOutcome,
};
pub use range_completion::{
    MAX_RANGE_AGGREGATE_PARTITION, RANGE_COVERAGE_COMMITMENT_MAGIC,
    RANGE_COVERAGE_COMMITMENT_VERSION, RANGE_JOURNAL_OBLIGATION_COMMITMENT_MAGIC,
    RANGE_JOURNAL_OBLIGATION_COMMITMENT_VERSION, RECLAMATION_COMPLETE_V3_DOMAIN,
    RECLAMATION_COMPLETE_V3_KEY_DOMAIN, RECLAMATION_COMPLETE_V3_VERSION, RangeAggregateCounters,
    RangeCompletionContext, RangeCompletionMarkerV3, RangeCoverageCommitment,
    RangeCoverageCommitmentBuilder, RangeCoverageObservation, StructuralRangeCandidate,
    commit_journal_obligations, commit_range_coverage, read_structural_range_candidate,
    reclamation_complete_v3_range_idempotency_key,
};
pub use range_installation::{
    CoordinatedObservation, CoordinatedUnresolvedObligations, DiscoveredObservationError,
    DiscoveredRangeContext, DiscoveredRangeObservation, MAX_OBSERVATION_RECORDS,
    NotEstablished as RangeInstallNotEstablished, NotObservedTerminus, RangeInstallationLimits,
    RangeInstallationObservation, UnresolvedObligations,
    observe_discovered_range_terminal_under_coordination,
    verify_range_terminal_on_observed_ancestry,
};
pub use reclaim::{
    CompletionKind, PlanBatch, RECLAMATION_COMPLETE_DOMAIN, RECLAMATION_COMPLETE_KEY_DOMAIN,
    RECLAMATION_COMPLETE_VERSION, RECLAMATION_LEDGER_DOMAIN, RECLAMATION_LEDGER_KEY_DOMAIN,
    RECLAMATION_LEDGER_V2_DOMAIN, RECLAMATION_LEDGER_V2_KEY_DOMAIN, RECLAMATION_LEDGER_V2_VERSION,
    RECLAMATION_LEDGER_VERSION, RECLAMATION_PLAN_DOMAIN, RECLAMATION_PLAN_KEY_DOMAIN,
    RECLAMATION_PLAN_VERSION, ReclaimDirective, ReclaimOutcome, ReclaimPlan, ReclaimUnit,
    ReclamationComplete, ReclamationLedger, ReclamationLedgerV2, V2ExecutionReadiness,
    is_reclamation_ledger_family, publish_reclamation_complete, publish_reclamation_ledger,
    reclamation_complete_idempotency_key, reclamation_ledger_idempotency_key,
    reclamation_ledger_v2_idempotency_key, reclamation_plan_idempotency_key,
    verify_v2_execution_readiness,
};
pub use record::{AuthorityMode, RECORD_DOMAIN, RECORD_VERSION, RecordKind, RevisionRecord};
pub use recovery::{
    AppliedEffect, ConfirmedEvidence, RecoveredParticipant, RecoveryAnchor, RecoveryError,
    RecoveryOutcome, RecoveryReport, TransitionClassification, reconcile, reconcile_from_parts,
    reconstruct_episode_frontier,
};
pub use recovery_classification::{
    ClassificationError, EvidenceDegradation, NotEstablished, ObservedBegin, ObservedPlan,
    ObservedV1Begin, OlderRangeStatus, RecoveryClassification, RecoveryState, UnresolvedAuthority,
    WitnessDegradation, WitnessEvidenceObservation, classify_recovery_evidence,
};
pub use recovery_discovery::{
    CompletionCandidateObservation, CompletionStructural, CoveringCheckpoint, CoveringObservation,
    DiscoveredFamily, DiscoveredPayload, DiscoveredRecord, DiscoveryBoundary, DiscoveryError,
    DiscoveryLimits, MAX_DISCOVERY_RECORDS, RecoveryEvidenceDiscovery, discover_recovery_evidence,
};
pub use rotation::{
    GENERATIONS_IO_FAULT, JOURNAL_ACTIVE_SUFFIX, JournalRotation, POINTER_DIR_SYNC_FAULT,
    PointerDurability, SessionRotationError, active_journal_path, read_journal_generations,
    reestablish_pointer_durability, retire_journal_generations, rotate_session_journal,
};
#[cfg(feature = "sqlite")]
pub use sqlite::{
    SqliteFaultAction, SqliteFaultControl, SqliteFaultPoint, SqliteMedium, SqliteMediumConfig,
};
pub use worker_stats::{
    WORKER_OP_CLASS_COUNT, WORKER_STATS_SAMPLE_RING, WorkerClassStats, WorkerOpClass, WorkerSample,
    WorkerStats, WorkerStatsCell,
};
