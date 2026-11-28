//! The commit pipeline (`pipeline.rs`, the fourth of the six Batch 3
//! modules) — the five-state durability model's per-key view over the
//! T1 journal and the canonical medium (§4.2–4.3, §5.5).
//!
//! **The per-key phases** (the GO's four statuses, one lattice):
//! `InMemory` (staged accrual during play — no I/O touches this
//! path) → `JournaledT1` (the boundary capture journaled and fsynced
//! — durable, replayable) → `AppliedToLivePending` (the application
//! reports the effect in the live authoritative state while the
//! canonical commit is pending — never conflated with durability) →
//! `InConfirmedChain` (canonically confirmed). An effect may exist in
//! live state while its canonical commit is pending; that is never
//! presented as durability.
//!
//! **Canonical confirmation (the rule, never relaxed):** a
//! consequence is confirmed iff (1) the revision carrying its
//! `consequence-batch/1` content object is **included in the verified
//! chain** — re-read and hash-verified through the medium, never a
//! bare `Accepted`, never inferred from time, local receipts,
//! counter matches, T1 fsync, or caches; (2) the batch object is read
//! back **by its content identity** and **every `ConsequenceId` is
//! individually validated** — present, with exactly the attempted
//! occurrence coverage and producing transaction, and no unattempted
//! keys; and (3) the chain walk placed each key at the confirming
//! revision. Merely storing a batch object or confirming its
//! aggregate revision is **insufficient** — a mismatch is
//! [`PipelineError::ConfirmationInvalid`], never a false T3.
//!
//! **The canonical batch (requirement A):** the multi-key batch is a
//! versioned `consequence-batch/1` content object referenced via the
//! revision's `content_refs` — carrying the complete validated key
//! set with per-consequence occurrence coverage — never an opaque
//! batch idempotency key. The revision's idempotency key **is** the
//! batch's content hash (chain-unique, enforced by the medium), so a
//! stale-parent rebase or an idempotent retry of the same logical
//! batch can never double-install. Duplicate keys across revisions
//! never duplicate: chain-confirmed keys are never re-batched, and a
//! conflicting identity reuse fails closed.
//!
//! **Stale parents and Unknown outcomes:** the pipeline appends
//! against its own verified head (the trusted-cursor model — the
//! session's view, not a fresh read). A `RejectedStaleParent` means
//! another writer advanced the chain: the pipeline re-walks the
//! observed suffix (determining the now-confirmed keys — a key
//! confirmed by another writer's batch is confirmed, never
//! re-submitted), rebases the remaining keys **with identical
//! per-key identities and coverages** on the observed parent, and
//! retries once; a second rejection retains the attempt for
//! [`Self::retry_pending`]. An `Unknown` outcome retains the exact
//! revision, record bytes, and key set; [`Self::reconcile_pending`]
//! resolves it under the medium's lock — `Installed` still requires
//! the full read-back walk and individual validation before
//! confirmation; `NotInstalled` is retry-safe under the same content
//! identity; `Unverifiable` stays pending, never falsely confirmed.
//!
//! **Honesty:** the confirmation receipt carries the exact revision,
//! the verified chain position (epoch), every individually validated
//! key, the actual provider tier, and the authority mode **from the
//! founding record** (never assumed): standalone campaigns label
//! `T3-local (standalone)` — never bare T3; shared campaigns confirm
//! via the shared medium. Windows carries the process-crash tier,
//! never presented as power-loss durability. The chain walk is
//! O(new revisions) per commit (the session-open walk is O(chain),
//! documented honestly — never per tick); staging performs no I/O;
//! the pipeline never decides gameplay policy.
//!
//! **The settled-transaction watermark and coverage shedding
//! (P3-L6, A22-persist):** the per-commit actionable scan is
//! proportional to the UNSETTLED window, never the total history.
//! The authority tracks per-transaction settledness — a journal
//! transaction is *settled* when it is durably canceled (never
//! actionable again — cancellations are append-only) or when EVERY
//! envelope it captured is chain-confirmed under exact provenance
//! (the same producing transaction, coverage, and effect commitment
//! the scan itself verifies). Settlements are recorded as a monotone
//! prefix mark (the watermark — every transaction below it is
//! settled, never revisited) plus a settled-bitmap for out-of-order
//! settlements above the mark. The ordering proof: the journal is
//! append-only within a generation and a transaction's envelope set
//! is fixed at capture ([`CommitAuthority::stage`] refuses tracked
//! identities), the confirmed map is insert-only (a conflicting
//! claim fails the walk closed), and confirmation is monotone — so
//! the exact-provenance check that established a settlement would
//! succeed identically on every later scan; skipping it is
//! decision-equivalent, and a settled transaction's keys can never
//! become actionable again. **Fail-closed:** the watermark can never
//! advance past an unconfirmed transaction — the mark holds at the
//! earliest unsettled transaction, its keys are re-collected on
//! every commit (never silently dropped), and the held state is
//! reported in [`CommitPipeline::retention_stats`], never silently
//! accepted. A fresh open re-derives the watermark lazily at the
//! first actionable scan (O(journal) once per session — the same
//! honest-cost tradition as the session-open chain walk).
//!
//! **The coverage shed:** the per-key coverage detail
//! (`ConfirmedEntry.coverage`) of a chain-confirmed identity is dead
//! weight once its producing journal transaction is settled — the
//! actionable scan skips settled transactions, and the retained
//! pending attempts only ever carry keys whose producers are
//! UNSETTLED (an attempt is collected from an unsettled transaction,
//! and no further collection runs while the attempt is unresolved),
//! so the exact-provenance comparisons that consult coverage never
//! reach a settled producer's entry; a foreign or duplicate claim on
//! a settled identity fails on the producing-transaction arm before
//! coverage is compared. The shed therefore drops the coverage Vec
//! of settled producers' entries — keeping the identity, the
//! producing transaction, the effect commitment, the confirming
//! revision, and the epoch: **the dedup membership is NEVER dropped**
//! (`is_confirmed`, `confirmed_keys`, the conflicting-reuse refusal,
//! the receipts, and the promotions all survive unchanged). The shed
//! is eligibility-gated (only settled producers — anything unproven
//! is retained, fail-closed) and bounded to
//! [`SETTLED_COVERAGE_RETAINED`] retained entries (the documented
//! grace window for post-confirmation provenance queries). A shed
//! identity's coverage **re-derives from the chain**: a freshly
//! opened pipeline's session walk re-collects every batch entry with
//! full coverage — the recovery composition runs over exactly that
//! fresh open (the crossing's recovery driver opens the pipeline
//! before consuming `confirmed_evidence`), so recovery never depends
//! on shed detail; in-session, [`CommitVerifier::confirmed_coverage`]
//! and [`CommitVerifier::confirmed_evidence`] honestly refuse the
//! shed detail (reported in the retention stats, never silent).
//!
//! **The compaction cadence:** rotation+reclamation is ALREADY the
//! compaction point for the journal-side retention — the rotation's
//! authenticated classification (rotation.rs) retires canceled and
//! confirmed-and-subsumed transactions, re-anchoring only the
//! unsettled survivors into a fresh generation, and the reopened
//! pipeline reseeds its dedup keys and watermark from the smaller
//! journal. The engine's contribution to the cadence is the
//! documented threshold ([`ROTATION_CADENCE_TRANSACTIONS`]) and the
//! [`CommitPipeline::retention_stats`] surface that reports a
//! generation exceeding it: the growth is REPORTED (the application
//! layer's maintenance reads the stats and invokes the rotation),
//! never silently accepted, and never auto-triggered (the rotation
//! stays the explicit maintenance action under the single-writer
//! discipline).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt;

use engine_core::{Decoder, Encoder, SaveError};

use crate::checkpoint::{CHECKPOINT_METADATA_DOMAIN, CheckpointMetadata};
use crate::consequence::ConsequenceId;
use crate::envelope::{ClassifiedSpan, ConsequenceEnvelope, DurabilityClass, SourcePosition};
use crate::hash::Hash;
use crate::journal::{
    CancellationReason, FrontierEntry, JournalError, JournalReceipt, T1Journal, TransactionId,
    TransactionInput, WriteReconciliation,
};
use crate::medium::{
    AppendOutcome, CanonicalMedium, MediumError, ObjectClass, Reconciliation, StoredObject,
    SyncTier, UnknownReason,
};
use crate::record::{AuthorityMode, RecordKind, RevisionRecord};
use crate::recovery::ConfirmedEvidence;

/// The canonical multi-consequence batch's domain (versioned; the
/// content object referenced via the revision's `content_refs`).
/// **v2 (the third review's effect-equivalence corrective):** each
/// entry additionally carries the producing envelope's EFFECT
/// COMMITMENT — the SHA-256 of its canonical bytes — binding the
/// confirmed consequence to its actual authoritative effects. v1
/// carried only (identity, transaction, coverage), which cannot
/// distinguish two valid payloads sharing the identity-level
/// `TransactionId` and coverage (e.g. +2 pending vs +2 captured
/// under the same identity, span, classification, and plan). v1
/// never shipped in an accepted build (the B3 delivery remained on
/// HOLD), so there is no on-disk compatibility surface: the walk
/// reads v2 only, and this constant is the documented migration
/// boundary.
pub const CONSEQUENCE_BATCH_DOMAIN: &str = "sektor7/engine-persist/consequence-batch/2";
pub const CONSEQUENCE_BATCH_SCHEMA: u8 = 2;

/// Documented maximum for one batch's entries (bounded decode — each
/// entry is at least 16 + 40 + 8 bytes, also bounded by the remaining
/// input).
pub const MAX_BATCH_ENTRIES: u64 = 1 << 16;
/// Documented maximum for one entry's coverage spans.
pub const MAX_BATCH_SPANS: u64 = 1 << 16;

/// (P3-L6, A22-persist) The documented settled-coverage retention
/// bound: the number of shed-ELIGIBLE confirmed entries (settled
/// producers' identities) whose coverage detail is retained past
/// eligibility — the grace window for in-session post-confirmation
/// provenance queries. Entries beyond the bound shed their coverage
/// Vec (the identity, producing transaction, effect commitment,
/// revision, and epoch are ALWAYS kept — the dedup membership is
/// never dropped); the shed detail re-derives from the chain at the
/// next session open (the walk's full batch collection). The bound
/// exists so the shed only ever trades detail that has actually
/// accumulated: two orders above the largest observed single-commit
/// key set at the current scale (the E-8 memory-lane flushes are
/// tens of keys against `MAX_BATCH_ENTRIES`'s 65,536 ceiling).
/// **[ASSUMPTION]** pending the owner-rig retention data point at
/// the intended first-game scale (the design supplies the mechanism,
/// not the number — the P3 §6 unresolved-questions discipline).
pub const SETTLED_COVERAGE_RETAINED: usize = 1 << 10;

/// (P3-L6, A22-persist) The documented rotation-cadence threshold:
/// the committed-transaction count at which a journal generation is
/// REPORTED as exceeding the cadence
/// ([`CommitPipeline::retention_stats`]'s
/// `exceeds_rotation_cadence`). The rotation itself is never
/// auto-triggered — it stays the explicit maintenance action under
/// the single-writer discipline (rotation.rs), and this constant is
/// the documented default the application layer's maintenance reads:
/// without maintenance available, the growth is reported in the
/// stats, never silently accepted. The order matches the long-journal
/// fixture scale the P3 design names (~5,000 settled transactions).
/// **[ASSUMPTION]** pending the owner-rig data point at the intended
/// first-game scale (the design supplies the mechanism, not the
/// threshold).
pub const ROTATION_CADENCE_TRANSACTIONS: u64 = 1 << 12;

/// One consequence's entry in the canonical batch — the complete
/// per-consequence evidence: the identity, the producing transaction,
/// the occurrence coverage, and the effect commitment. Confirmation
/// validates each entry individually; the aggregate alone is never
/// sufficient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchEntry {
    pub id: ConsequenceId,
    pub transaction: TransactionId,
    /// The class-homogeneous occurrence coverage this consequence
    /// commits (mirrors the envelope's sub-spans).
    pub coverage: Vec<ClassifiedSpan>,
    /// The producing envelope's effect commitment — the SHA-256 of
    /// its canonical bytes — binding this confirmed consequence to
    /// its actual authoritative effects (the uniform signal lane,
    /// the deltas, the classes). The identity-level `TransactionId`
    /// is deliberately content-blind; THIS is the semantic binding
    /// that distinguishes effect equivalence from identity equality.
    pub effect: Hash,
}

/// The versioned canonical batch content object (requirement A —
/// never an opaque batch key). Entries are strictly ascending by
/// identity, unique.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsequenceBatch {
    pub entries: Vec<BatchEntry>,
}

impl ConsequenceBatch {
    /// Canonical bytes (strict: only ordered, unique, well-formed
    /// batches serialize).
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, SaveError> {
        const SECTION: &str = "consequence-batch";
        if self.entries.len() as u64 > MAX_BATCH_ENTRIES {
            return Err(SaveError::Limit {
                section: SECTION,
                kind: "entry-count",
                value: self.entries.len() as u64,
                max: MAX_BATCH_ENTRIES,
            });
        }
        let mut enc = Encoder::new();
        enc.write_str(CONSEQUENCE_BATCH_DOMAIN);
        enc.write_u8(CONSEQUENCE_BATCH_SCHEMA);
        enc.write_count(self.entries.len());
        let mut prev: Option<&ConsequenceId> = None;
        for entry in &self.entries {
            if let Some(prior) = prev
                && entry.id <= *prior
            {
                return Err(SaveError::Order { section: SECTION });
            }
            prev = Some(&entry.id);
            enc.write_u32(entry.id.region);
            enc.write_u64(entry.id.episode);
            enc.write_u32(entry.id.npc);
            enc.write_bytes(entry.transaction.0.as_bytes());
            enc.write_bytes(entry.effect.as_bytes());
            write_coverage(&mut enc, &entry.coverage)?;
        }
        Ok(enc.into_bytes())
    }

    /// Strict decode (fail-closed: domain, schema, ascending unique
    /// identities, per-span validity and canonical order, bounded
    /// counts, no trailing).
    pub fn decode(bytes: &[u8]) -> Result<Self, SaveError> {
        const SECTION: &str = "consequence-batch";
        let mut dec = Decoder::new(bytes);
        dec.read_domain(CONSEQUENCE_BATCH_DOMAIN)?;
        let schema = dec.read_u8()?;
        if schema != CONSEQUENCE_BATCH_SCHEMA {
            return Err(SaveError::Version {
                observed: u32::from(schema),
                supported: u32::from(CONSEQUENCE_BATCH_SCHEMA),
            });
        }
        let count = dec.read_count(MAX_BATCH_ENTRIES, SECTION, "entry-count")?;
        // Each entry is at least 16 (identity) + 40 (transaction) +
        // 32 (effect commitment) + 8 (coverage count) bytes.
        let remaining = dec.remaining() as u64;
        if count > remaining / 96 {
            return Err(SaveError::Limit {
                section: SECTION,
                kind: "entry-count",
                value: count,
                max: remaining / 96,
            });
        }
        let mut entries: Vec<BatchEntry> = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let id = ConsequenceId {
                region: dec.read_u32()?,
                episode: dec.read_u64()?,
                npc: dec.read_u32()?,
            };
            if let Some(prior) = entries.last()
                && id <= prior.id
            {
                return Err(SaveError::Order { section: SECTION });
            }
            let transaction_bytes = dec.read_bytes(Hash::WIDTH, SECTION)?;
            let transaction = TransactionId(
                Hash::from_bytes(transaction_bytes)
                    .ok_or(SaveError::Integrity("consequence-batch/hash-width"))?,
            );
            let effect_bytes = dec.read_bytes(Hash::WIDTH, SECTION)?;
            let effect = Hash::from_bytes(effect_bytes)
                .ok_or(SaveError::Integrity("consequence-batch/hash-width"))?;
            let coverage = read_coverage(&mut dec)?;
            entries.push(BatchEntry {
                id,
                transaction,
                coverage,
                effect,
            });
        }
        dec.finish()?;
        Ok(ConsequenceBatch { entries })
    }
}

fn class_discriminant(class: DurabilityClass) -> u8 {
    match class {
        DurabilityClass::DurableEligible => 0,
        DurabilityClass::EncounterProvisional => 1,
    }
}

fn read_class(dec: &mut Decoder<'_>) -> Result<DurabilityClass, SaveError> {
    match dec.read_u8()? {
        0 => Ok(DurabilityClass::DurableEligible),
        1 => Ok(DurabilityClass::EncounterProvisional),
        discriminant => Err(SaveError::Enum {
            section: "consequence-batch",
            discriminant: u64::from(discriminant),
        }),
    }
}

/// One coverage list's canonical form: per-span validity, sorted,
/// non-overlapping; same-class touching spans would have composed
/// (non-canonical); **different-class adjacency is the legitimate
/// class-homogeneous partition boundary** — a mixed aggregate's
/// sub-spans meet exactly there (envelope/1's rule, carried into the
/// canonical batch).
fn validate_coverage(coverage: &[ClassifiedSpan]) -> Result<(), SaveError> {
    for classified in coverage {
        if classified.span.lo >= classified.span.hi {
            return Err(SaveError::Integrity(
                "consequence-batch/coverage-span-inverted",
            ));
        }
    }
    for pair in coverage.windows(2) {
        let (prior, next) = (&pair[0], &pair[1]);
        if next.span.lo < prior.span.hi {
            return Err(SaveError::Integrity(
                "consequence-batch/coverage-span-overlap",
            ));
        }
        if next.span.lo == prior.span.hi && prior.class == next.class {
            return Err(SaveError::Integrity(
                "consequence-batch/coverage-span-noncanonical",
            ));
        }
    }
    Ok(())
}

fn write_coverage(enc: &mut Encoder, coverage: &[ClassifiedSpan]) -> Result<(), SaveError> {
    const SECTION: &str = "consequence-batch";
    if coverage.len() as u64 > MAX_BATCH_SPANS {
        return Err(SaveError::Limit {
            section: SECTION,
            kind: "coverage-span-count",
            value: coverage.len() as u64,
            max: MAX_BATCH_SPANS,
        });
    }
    validate_coverage(coverage)?;
    enc.write_count(coverage.len());
    for classified in coverage {
        enc.write_u64(classified.span.lo);
        enc.write_u64(classified.span.hi);
        enc.write_u8(class_discriminant(classified.class));
    }
    Ok(())
}

fn read_coverage(dec: &mut Decoder<'_>) -> Result<Vec<ClassifiedSpan>, SaveError> {
    const SECTION: &str = "consequence-batch";
    let count = dec.read_count(MAX_BATCH_SPANS, SECTION, "coverage-span-count")?;
    let remaining = dec.remaining() as u64;
    if count > remaining / 17 {
        return Err(SaveError::Limit {
            section: SECTION,
            kind: "coverage-span-count",
            value: count,
            max: remaining / 17,
        });
    }
    let mut coverage: Vec<ClassifiedSpan> = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let lo = dec.read_u64()?;
        let hi = dec.read_u64()?;
        let class = read_class(dec)?;
        coverage.push(ClassifiedSpan {
            span: crate::consequence::OccurrenceSpan { lo, hi },
            class,
        });
    }
    validate_coverage(&coverage)?;
    Ok(coverage)
}

/// The per-key phase lattice (the GO's four statuses).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPhase {
    /// Staged in memory only, before the journal write — never
    /// guaranteed across a crash, never claimed to be.
    InMemory,
    /// Journaled and fsynced (T1) — durable and replayable; pending
    /// canonical confirmation.
    JournaledT1,
    /// The application reports the effect in the live authoritative
    /// state while the canonical commit is pending.
    AppliedToLivePending,
    /// Individually validated inside the verified canonical chain.
    InConfirmedChain,
}

/// One key's pipeline view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyStatus {
    pub phase: KeyPhase,
    /// The journal transaction that captured the key.
    pub transaction: Option<TransactionId>,
    /// The chain revision that confirmed it.
    pub revision: Option<Hash>,
}

/// The capture boundary's receipt (returned only after the journal's
/// fsync — requirement C; a reconciled capture recovered a valid
/// committed frame without a fabricated acknowledgment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureReceipt {
    pub journal: JournalReceipt,
    pub reconciled: bool,
}

/// What a canonical commit produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitOutcome {
    /// Every key individually validated inside the verified chain.
    Confirmed(ConfirmationReceipt),
    /// No actionable, unconfirmed journaled keys existed.
    NothingPending,
    /// The append's outcome is uncertain — the exact revision, record
    /// bytes, and key set are retained; reconcile via
    /// [`CommitPipeline::reconcile_pending`].
    Unknown {
        revision: Hash,
        reason: UnknownReason,
    },
    /// The rebase-and-retry was rejected stale again — the attempt is
    /// retained for [`CommitPipeline::retry_pending`].
    StaleParent { observed: Option<Hash> },
}

/// What reconciling a retained unknown attempt established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileOutcome {
    /// Installed **and** fully verified (the read-back walk plus the
    /// individual per-key validation) — confirmed.
    Confirmed(ConfirmationReceipt),
    /// Not installed — the attempt is retained; an idempotent retry
    /// under the same content identity is safe.
    NotInstalled,
    /// Verification could not establish the result — the attempt
    /// stays pending, never falsely confirmed.
    Unverifiable { reason: String },
    /// No retained attempt exists — nothing to reconcile.
    NothingPending,
}

/// The confirmation receipt — the exact revision, the verified chain
/// position, every individually validated key, the actual tier, and
/// the authority mode from the founding record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmationReceipt {
    pub revision: Hash,
    pub epoch: u64,
    pub keys: Vec<ConsequenceId>,
    pub tier: SyncTier,
    pub authority: AuthorityMode,
    pub label: ConfirmationLabel,
}

/// The honest confirmation label (§3.3 — no silent mode
/// presentations).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationLabel {
    /// A standalone campaign's local canonical confirmation —
    /// legitimate, never presented as replication or power-loss
    /// protection.
    T3LocalStandalone,
    /// A shared campaign confirmed through the shared medium.
    SharedCanonical,
}

impl fmt::Display for ConfirmationLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfirmationLabel::T3LocalStandalone => write!(f, "T3-local (standalone)"),
            ConfirmationLabel::SharedCanonical => write!(f, "shared canonical"),
        }
    }
}

/// Typed pipeline failures.
#[derive(Debug)]
pub enum PipelineError {
    Medium(MediumError),
    Journal(JournalError),
    Format(SaveError),
    /// The campaign store is unfounded — the founding record is the
    /// application's setup act, never a per-commit decision.
    UnfoundedMedium,
    /// A tracked identity re-staged or re-journaled with different
    /// content — conflicting reuse fails closed.
    ConflictingReuse {
        id: ConsequenceId,
    },
    /// The journal boundary aborted before the commit record —
    /// nothing durable was written; the staged set is retained and
    /// the retry is safe.
    CaptureAborted,
    /// An unresolved canonical attempt exists — reconcile or retry it
    /// before starting a new one (identity exactness).
    AttemptUnresolved {
        revision: Hash,
    },
    /// An untracked key was reported applied.
    UnknownKey {
        id: ConsequenceId,
    },
    /// An unstaged, unjournaled key was reported applied to live
    /// state — authoritative application before durability violates
    /// durable-before-discard.
    AppliedBeforeJournal {
        id: ConsequenceId,
    },
    /// The confirmed batch failed individual per-key validation —
    /// never a false T3.
    ConfirmationInvalid {
        detail: &'static str,
    },
    /// Conflicting reuse of a `ConsequenceId` across valid canonical
    /// batches: two chain revisions claim the same identity. The
    /// exact-provenance standard rejects the second — fail closed,
    /// never overwriting the first revision's evidence, never
    /// guessing which is authoritative.
    CanonicalIdentityConflict {
        id: ConsequenceId,
        first_revision: Hash,
        second_revision: Hash,
    },
    /// Foreign-confirmed keys span distinct confirming revisions —
    /// one honest receipt cannot report them; fail closed rather
    /// than misreport any key's provenance.
    ForeignProvenanceDivergent {
        id: ConsequenceId,
        first_revision: Hash,
        second_revision: Hash,
    },
    /// The local attempt's provenance conflicts with the canonical
    /// entry claiming the identity — a different producing
    /// transaction or occurrence coverage. Exact-provenance
    /// equivalence is required before a foreign confirmation
    /// satisfies the local attempt; a mismatch fails closed, never
    /// silently absorbed.
    ConfirmedProvenanceMismatch {
        id: ConsequenceId,
    },
}

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PipelineError::Medium(error) => write!(f, "canonical medium failure: {error}"),
            PipelineError::Journal(error) => write!(f, "journal failure: {error}"),
            PipelineError::Format(error) => write!(f, "pipeline format failure: {error}"),
            PipelineError::UnfoundedMedium => {
                write!(
                    f,
                    "the canonical medium is unfounded — found the campaign first"
                )
            }
            PipelineError::ConflictingReuse { id } => {
                write!(f, "conflicting reuse of consequence identity {id}")
            }
            PipelineError::CaptureAborted => write!(
                f,
                "the capture boundary aborted before the commit record — retry-safe"
            ),
            PipelineError::AttemptUnresolved { revision } => write!(
                f,
                "an unresolved canonical attempt ({revision}) exists — reconcile it first"
            ),
            PipelineError::UnknownKey { id } => {
                write!(f, "key {id} is not tracked by the pipeline")
            }
            PipelineError::AppliedBeforeJournal { id } => write!(
                f,
                "key {id} was reported applied before it was journaled — \
                 durable-before-discard violated"
            ),
            PipelineError::ConfirmationInvalid { detail } => write!(
                f,
                "canonical confirmation failed individual validation ({detail}) — \
                 never a false T3"
            ),
            PipelineError::CanonicalIdentityConflict {
                id,
                first_revision,
                second_revision,
            } => write!(
                f,
                "consequence identity {id} is claimed by two valid canonical batches \
                 ({first_revision} and {second_revision}) — conflicting reuse fails \
                 closed"
            ),
            PipelineError::ForeignProvenanceDivergent {
                id,
                first_revision,
                second_revision,
            } => write!(
                f,
                "foreign-confirmed keys span distinct confirming revisions \
                 ({first_revision} and {second_revision}, first at {id}) — one \
                 honest receipt cannot report them"
            ),
            PipelineError::ConfirmedProvenanceMismatch { id } => write!(
                f,
                "the local attempt for consequence identity {id} conflicts with \
                 the canonical entry claiming it — exact provenance is required, \
                 a mismatch fails closed"
            ),
        }
    }
}

impl std::error::Error for PipelineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PipelineError::Medium(error) => Some(error),
            PipelineError::Journal(error) => Some(error),
            PipelineError::Format(error) => Some(error),
            _ => None,
        }
    }
}

impl From<MediumError> for PipelineError {
    fn from(error: MediumError) -> Self {
        PipelineError::Medium(error)
    }
}

impl From<JournalError> for PipelineError {
    fn from(error: JournalError) -> Self {
        PipelineError::Journal(error)
    }
}

/// One chain-confirmed key's evidence (the suppression authority):
/// the exact coverage, the producing transaction, the effect
/// commitment binding the confirmed consequence to its actual
/// authoritative effects, and the confirming revision with its epoch
/// — the full provenance the receipt reports and the conflict checks
/// compare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedEntry {
    pub coverage: Vec<ClassifiedSpan>,
    pub transaction: TransactionId,
    pub effect: Hash,
    pub revision: Hash,
    pub epoch: u64,
}

/// The read-only confirmed-evidence view the actionable-key
/// derivation, the remaining-keys filter, and the key-phase promotion
/// run over. The verifier's truth map implements it (the synchronous
/// facade passes the verifier itself); a future actor composition
/// passes a receipt-fed mirror instead — over-inclusion is always
/// corrected by the verifier's own filter, never trusted from the
/// view.
pub trait ConfirmedView {
    /// The chain-confirmed evidence for one identity, if any.
    fn confirmed(&self, id: &ConsequenceId) -> Option<&ConfirmedEntry>;
}

/// One attempted key's exact-provenance evidence: the occurrence
/// coverage, the producing transaction, and the envelope's effect
/// commitment (identity-level equality is content-blind; the
/// commitment binds the actual authoritative effects — the tuple the
/// foreign-confirmation and confirmed-recovery equivalence compares).
pub type AttemptedKey = (Vec<ClassifiedSpan>, TransactionId, Hash);

/// One immutable commit request: the actionable keys' frozen evidence
/// plus the journal-derived position ticks — everything the
/// submission derives from the journal, so the verifier itself is
/// journal-blind (the authority half remains the journal's sole
/// owner).
#[derive(Clone)]
pub struct CommitRequest {
    /// The actionable, unconfirmed journaled keys: identity →
    /// (coverage, producing transaction, effect commitment).
    pub keys: BTreeMap<ConsequenceId, AttemptedKey>,
    /// The producing transactions' journal position ticks (a
    /// transaction's position is fixed at capture, so the frozen
    /// snapshot equals any later live lookup).
    pub ticks: BTreeMap<TransactionId, u64>,
}

/// A retained canonical attempt — the exact revision, the batch's
/// content identity, and the per-key identities (the batch bytes
/// re-derive deterministically from the keys; never rebuilt from
/// guesswork).
#[derive(Clone)]
struct PendingAttempt {
    record: RevisionRecord,
    batch_hash: Hash,
    /// The attempted per-key evidence: identity → (coverage,
    /// producing transaction, effect commitment).
    keys: BTreeMap<ConsequenceId, AttemptedKey>,
    /// The producing transactions' journal position ticks, frozen at
    /// submission (the retry's re-derivation uses the retained
    /// snapshot — identical to a live lookup by position
    /// immutability).
    ticks: BTreeMap<TransactionId, u64>,
}

/// The authority-owned half of the commit pipeline: the T1 journal,
/// the staged in-memory accrual, and the per-key phase tracking.
/// Journal-only — no medium I/O lives here. This is the half a future
/// verification actor leaves on the authority thread.
pub struct CommitAuthority {
    journal: T1Journal,
    /// The staged in-memory accrual — no I/O touches this path; a
    /// crash loses it honestly (P0-5).
    staged: Vec<ConsequenceEnvelope>,
    keys: BTreeMap<ConsequenceId, KeyStatus>,
    /// (P3-L6) THE SETTLED-TRANSACTION WATERMARK — the monotone prefix
    /// mark: every `journal.transactions()[..watermark]` is settled
    /// (durably canceled, or every envelope chain-confirmed under the
    /// exact provenance a scan verified) and is never revisited by
    /// the actionable scan. The mark only ever advances through a
    /// settled transaction — an unconfirmed transaction HOLDS it
    /// (fail-closed: the unsettled window keeps paying for it, and
    /// its keys keep being collected, never silently dropped).
    settled_watermark: usize,
    /// (P3-L6) The settled-bitmap: transactions settled OUT of order
    /// (at or above the mark — e.g. a canceled transaction behind an
    /// unconfirmed hole). The mark absorbs an entry (removing it)
    /// exactly when it advances past it.
    settled_above: HashSet<TransactionId>,
    /// (P3-L6) The envelope identities of the transactions the
    /// actionable scans settled since the last drain — the coverage
    /// shed's eligibility input, consumed by the facade's
    /// post-commit composition. A direct half-composition that never
    /// drains forfeits the shed for those settlements (coverage stays
    /// retained — never a correctness effect).
    newly_settled_keys: Vec<ConsequenceId>,
    /// (P3-L6) Cumulative transaction visits by the actionable scans
    /// (the observational scaling surface — see
    /// [`CommitPipeline::retention_stats`]).
    scan_transactions: u64,
    /// (P3-L6) Cumulative envelope examinations by the actionable
    /// scans — the provenance-check work the watermark bounds.
    scan_envelopes: u64,
}

/// The medium-owned half of the commit pipeline: the canonical
/// medium, the chain-derived confirmed key set (the session's
/// verified view), the trusted cursor, and the retained unresolved
/// canonical attempts. No journal access — everything journal-derived
/// arrives inside the immutable request. This is the half a future
/// verification actor owns on its own thread.
pub struct CommitVerifier<M: CanonicalMedium> {
    medium: M,
    /// The chain-derived confirmed key set (the session's verified
    /// view — rebuilt at open, extended by the walks).
    confirmed: BTreeMap<ConsequenceId, ConfirmedEntry>,
    /// (P3-L6) The shed-eligible queue: confirmed identities whose
    /// producing journal transactions settled (the actionable scan's
    /// settlement proof), in settlement order, bounded to
    /// [`SETTLED_COVERAGE_RETAINED`] retained entries — beyond the
    /// bound the oldest entry's coverage detail sheds.
    shed_eligible: VecDeque<ConsequenceId>,
    /// (P3-L6) The shed discriminator: identities whose coverage
    /// detail was shed. An EMPTY `ConfirmedEntry.coverage` is
    /// legitimate for span-less movements — this set (never the Vec's
    /// emptiness alone) is what distinguishes a shed span-bearing
    /// entry, so the movement arm can never be confused with the shed
    /// state.
    shed_coverage: HashSet<ConsequenceId>,
    /// (P3-L6) Cumulative shed count (the observability surface).
    shed_coverage_total: u64,
    /// The last revision the walk processed (the append basis — the
    /// trusted-cursor model; `None` on an unfounded medium).
    verified_head: Option<Hash>,
    verified_epoch: u64,
    /// The authority mode and campaign id from the founding record.
    founding: Option<(AuthorityMode, String)>,
    /// The retained unresolved canonical attempt, if any.
    pending: Option<PendingAttempt>,
    tier: SyncTier,
    /// Completed walks (incremented only when `walk_chain` runs to
    /// its end). The facade promotes tracked keys exactly when this
    /// advances during an operation — today's walk-closing promotion
    /// loop ran precisely per completed walk.
    walk_rounds: u64,
}

/// The synchronous composition — behaviorally identical to the
/// accepted single-struct pipeline. Every published operation
/// delegates to the half that owns the state; the commit-family
/// operations compose: the authority derives the immutable request
/// (journal + the verifier's confirmed truth), the verifier processes
/// it (append → confirm → walk — strict full-object verification
/// throughout), and the authority promotes tracked keys from the
/// verifier's post-operation truth exactly when a walk completed. A
/// future verification actor replaces this composition's middle step
/// with a queue and a receipt; the halves themselves are unchanged.
pub struct CommitPipeline<M: CanonicalMedium> {
    authority: CommitAuthority,
    verifier: CommitVerifier<M>,
}

/// (P3-L6, A22-persist) The retention/compaction observability
/// surface — the settled-transaction watermark's position, the
/// unsettled population the per-commit actionable scan pays for, the
/// coverage-shed state, and the rotation-cadence reading. Read-only
/// derived state (never serialized, never a fingerprint input); the
/// application layer's maintenance reads it to decide the rotation
/// cadence — a generation exceeding the documented threshold is
/// REPORTED here, never silently accepted, and the rotation itself
/// stays the explicit maintenance action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionStats {
    /// The generation's committed transactions (the journal total).
    pub journal_transactions: u64,
    /// The settled-transaction watermark: every transaction below the
    /// mark is settled (canceled, or every envelope chain-confirmed
    /// under exact provenance) — the prefix the actionable scan never
    /// revisits.
    pub settled_watermark: u64,
    /// Settled transactions at or above the mark (out-of-order
    /// settlements the mark has not yet absorbed).
    pub settled_above_watermark: u64,
    /// The unsettled population the per-commit scan examines envelopes
    /// for (journal total − settled) — the window the watermark bounds
    /// the per-commit cost to.
    pub unsettled_transactions: u64,
    /// The dedup-membership size (`keys`) — the identity authority,
    /// NEVER dropped; bounded generationally by the rotation's
    /// retirement (the reopened pipeline reseeds from the smaller
    /// journal).
    pub tracked_keys: u64,
    /// The chain-confirmed membership size (`confirmed`).
    pub confirmed_keys: u64,
    /// Confirmed entries whose coverage detail was shed past the
    /// documented bound (cumulative since open).
    pub shed_coverage_entries: u64,
    /// Shed-eligible entries whose coverage is retained within the
    /// documented bound (the grace window).
    pub retained_settled_coverage: u64,
    /// Cumulative transaction visits by the actionable scans — with a
    /// watermark HELD by an unconfirmed transaction this grows with
    /// the window above the mark (the bitmap-skips are cheap but not
    /// free); the honest cost the hold reports.
    pub actionable_scan_transactions: u64,
    /// Cumulative envelope examinations by the actionable scans — the
    /// provenance-check work: flat iff the per-commit cost is
    /// proportional to the unsettled window (the observational
    /// scaling the watermark exists for).
    pub actionable_scan_envelopes: u64,
    /// Whether the generation exceeds the documented rotation-cadence
    /// threshold ([`ROTATION_CADENCE_TRANSACTIONS`]) — the reported
    /// growth the maintenance layer acts on.
    pub exceeds_rotation_cadence: bool,
    /// (CA-17) The confirmed map's ESTIMATED resident bytes — the
    /// per-identity metadata the dedup authority retains (the fixed
    /// `ConfirmedEntry` fields, the retained coverage spans, and the
    /// map's own per-entry overhead), computed by the documented
    /// formula in [`CommitVerifier::confirmed_residency_bytes`]. An
    /// estimate (the B15 computed-carve discipline), structurally
    /// testable: at fixed live obligations it grows LINEARLY in the
    /// confirmed history — the finding's measured shape.
    pub confirmed_residency_bytes: u64,
    /// (CA-17) The shed discriminator set's ESTIMATED resident bytes
    /// — one entry per identity whose coverage detail was shed, never
    /// bounded by anything today (the watermark bounds WORK, not
    /// residency).
    pub shed_residency_bytes: u64,
    /// (R07) The authority's tracked-key map's ESTIMATED resident
    /// bytes — the per-identity phase bookkeeping: per entry, the
    /// fixed `KeyStatus` fields, the `ConsequenceId` key, and the
    /// map's amortized per-entry node share (the CA-17 formula
    /// discipline). Linear in the generation's journaled identities
    /// at fixed live obligations; bounded generationally by the
    /// rotation's retirement (the reopened authority reseeds from the
    /// smaller journal).
    pub tracked_residency_bytes: u64,
    /// (R07) The staged accrual's ESTIMATED resident bytes — per
    /// staged envelope, the fixed struct plus the sub-span and
    /// dependency heap. Boundary-scoped (drained by the capture; a
    /// crash loses it honestly — P0-5): bounded by one capture
    /// boundary's batch, never by history.
    pub staged_residency_bytes: u64,
    /// (R07) The retained unresolved canonical attempt's ESTIMATED
    /// resident bytes — the pending slot's single retained copy (the
    /// revision record with its idempotency-key/content-ref/
    /// dependency heap, the batch content identity, the per-key
    /// evidence with the coverage heap, and the frozen ticks).
    /// Attempt-scoped: ZERO whenever no attempt is retained (the
    /// P3-L3 single-copy shape — at most one live attempt exists).
    pub pending_residency_bytes: u64,
    /// (R07) The journal's decoded committed history's ESTIMATED
    /// resident bytes — the in-memory T1 state the session holds:
    /// per committed transaction, the fixed skeleton, the COMPLETE
    /// envelopes (fixed struct + sub-span and dependency heap), the
    /// plan, the frontier, and the dependency heap; per cancellation,
    /// the record and its detail; plus the committed-index,
    /// canceled-id, and session-acknowledgment lookups (the formula
    /// lives at [`T1Journal::decoded_residency_bytes`]). Linear in
    /// the generation's committed transactions; bounded
    /// generationally by the rotation.
    pub journal_residency_bytes: u64,
}

impl RetentionStats {
    /// The settled population — the watermark plus the out-of-order
    /// bitmap.
    pub fn settled_transactions(&self) -> u64 {
        self.settled_watermark + self.settled_above_watermark
    }
}

impl CommitAuthority {
    /// Opens the authority half: the journal's committed transactions
    /// seed the per-key tracking (the T1 state is durable across
    /// sessions).
    pub fn open(journal: T1Journal) -> Self {
        let mut authority = CommitAuthority {
            journal,
            staged: Vec::new(),
            keys: BTreeMap::new(),
            // (P3-L6) The watermark starts at zero: settlement is
            // derived state, re-proven by the first actionable scan
            // of the session (O(journal) once — the same
            // honest-cost tradition as the verifier's session-open
            // chain walk; sessions that never commit never pay it).
            settled_watermark: 0,
            settled_above: HashSet::new(),
            newly_settled_keys: Vec::new(),
            scan_transactions: 0,
            scan_envelopes: 0,
        };
        for transaction in authority.journal.transactions() {
            for envelope in &transaction.envelopes {
                authority.keys.entry(envelope.id).or_insert(KeyStatus {
                    phase: KeyPhase::JournaledT1,
                    transaction: Some(transaction.id),
                    revision: None,
                });
            }
        }
        authority
    }

    /// The authority's journal (read-only access — the rotation's
    /// classification and the diagnostics read the durable T1
    /// state; the instance and its sidecar lock stay owned here).
    pub fn journal(&self) -> &T1Journal {
        &self.journal
    }

    /// Stages one consequential envelope — the in-memory accrual
    /// (never guaranteed across a crash). A tracked identity staged
    /// again with different content is a conflicting reuse and fails
    /// closed; a byte-identical restage is a no-op.
    pub fn stage(&mut self, envelope: ConsequenceEnvelope) -> Result<(), PipelineError> {
        envelope.canonical_bytes().map_err(PipelineError::Format)?;
        if self.keys.contains_key(&envelope.id) {
            if self.staged.iter().any(|staged| staged == &envelope) {
                return Ok(());
            }
            return Err(PipelineError::ConflictingReuse { id: envelope.id });
        }
        self.keys.insert(
            envelope.id,
            KeyStatus {
                phase: KeyPhase::InMemory,
                transaction: None,
                revision: None,
            },
        );
        self.staged.push(envelope);
        Ok(())
    }

    /// Discards staged (in-memory, uncaptured) envelopes for the given
    /// identities — the failed-boundary path: effects staged but never
    /// captured are in-memory pending only; discarding them never
    /// touches journaled or confirmed keys (P0-5 — in-memory state is
    /// honestly lost). A retried boundary re-stages identical bytes.
    pub fn discard_staged(&mut self, ids: &[ConsequenceId]) {
        self.staged.retain(|envelope| !ids.contains(&envelope.id));
        for id in ids {
            if let Some(status) = self.keys.get(id)
                && status.phase == KeyPhase::InMemory
            {
                self.keys.remove(id);
            }
        }
    }

    /// The capture boundary: journals the staged consequences as one
    /// T1 transaction (the receipt returns only after the journal's
    /// fsync). An ambiguous commit outcome is reconciled immediately
    /// — reread and verified, never guessed; the recovery of a valid
    /// committed frame is **not** a fabricated acknowledgment. An
    /// aborted capture is retry-safe with the staged set retained.
    pub fn capture(
        &mut self,
        position: SourcePosition,
        classification: DurabilityClass,
        dependencies: Vec<TransactionId>,
        frontier: Vec<FrontierEntry>,
        plan: Vec<u8>,
    ) -> Result<CaptureReceipt, PipelineError> {
        let input = TransactionInput {
            envelopes: self.staged.clone(),
            position,
            classification,
            dependencies,
            frontier,
            plan,
        };
        let receipt = match self.journal.append_transaction(&input) {
            Ok(receipt) => CaptureReceipt {
                journal: receipt,
                reconciled: false,
            },
            Err(JournalError::AmbiguousCommit { .. }) => {
                let reconciled = match self.journal.reconcile_write(&input)? {
                    WriteReconciliation::Committed { receipt } => receipt,
                    WriteReconciliation::NotCommitted => return Err(PipelineError::CaptureAborted),
                    // A different consequential payload is committed
                    // under this identity — the attempt is NOT durable
                    // and can never be: fail closed, never retry as
                    // though it were the same attempt.
                    WriteReconciliation::ConflictingPayload { id } => {
                        return Err(PipelineError::Journal(JournalError::ConflictingPayload {
                            id,
                        }));
                    }
                };
                CaptureReceipt {
                    journal: reconciled,
                    reconciled: true,
                }
            }
            Err(JournalError::AbortedBeforeCommit { .. }) => {
                return Err(PipelineError::CaptureAborted);
            }
            Err(other) => return Err(PipelineError::Journal(other)),
        };
        for envelope in self.staged.drain(..) {
            self.keys.insert(
                envelope.id,
                KeyStatus {
                    phase: KeyPhase::JournaledT1,
                    transaction: Some(receipt.journal.id),
                    revision: None,
                },
            );
        }
        Ok(receipt)
    }

    /// Appends a validated, durable, reason-coded cancellation for a
    /// journaled transaction (the authority proxies the journal —
    /// supersession is the explicit durable record, never a higher
    /// sequence).
    pub fn cancel_transaction(
        &mut self,
        target: TransactionId,
        reason: CancellationReason,
        detail: &[u8],
    ) -> Result<u64, PipelineError> {
        let sequence = self.journal.append_cancellation(target, reason, detail)?;
        Ok(sequence)
    }

    /// The application reports effects applied to the live
    /// authoritative state (the pending-vs-confirmed distinction —
    /// never conflated with durability). A key must be journaled
    /// first (durable-before-discard).
    pub fn mark_applied(&mut self, keys: &[ConsequenceId]) -> Result<(), PipelineError> {
        for id in keys {
            let Some(status) = self.keys.get(id) else {
                return Err(PipelineError::UnknownKey { id: *id });
            };
            match status.phase {
                KeyPhase::InMemory => {
                    return Err(PipelineError::AppliedBeforeJournal { id: *id });
                }
                KeyPhase::JournaledT1 => {}
                KeyPhase::AppliedToLivePending | KeyPhase::InConfirmedChain => {}
            }
        }
        for id in keys {
            if let Some(status) = self.keys.get_mut(id)
                && status.phase == KeyPhase::JournaledT1
            {
                status.phase = KeyPhase::AppliedToLivePending;
            }
        }
        Ok(())
    }

    /// One key's pipeline view.
    pub fn status(&self, id: ConsequenceId) -> Option<KeyStatus> {
        self.keys.get(&id).cloned()
    }

    /// Collects the actionable, unconfirmed journaled keys — the
    /// batch candidates — over a confirmed VIEW (the verifier's truth
    /// through the synchronous facade; a receipt-fed mirror in a
    /// future actor composition — the verifier's own remaining-keys
    /// filter corrects any over-inclusion, never trusting the view).
    /// A key claimed by two journal transactions is a conflicting
    /// reuse and fails closed.
    ///
    /// (P3-L6) THE SETTLED-TRANSACTION WATERMARK: the scan starts at
    /// the watermark (every transaction below the mark is settled and
    /// never revisited) and skips the settled-bitmap's out-of-order
    /// entries — the per-commit cost is proportional to the UNSETTLED
    /// window, not the total history. A transaction settles during
    /// the scan when it is durably canceled or when EVERY envelope is
    /// confirmed under exact provenance (the check this very scan
    /// performs — the settlement proof is per-transaction and
    /// independent, so settlements recorded before a later
    /// transaction's failure stand). Settlements accumulate into the
    /// shed-eligibility drain the facade's post-commit composition
    /// consumes; canceled settlements mark the watermark but feed no
    /// shed eligibility (a canceled transaction's keys are never
    /// confirmed — the batch excludes them). The mark advances only
    /// through settled transactions — an unconfirmed transaction
    /// HOLDS it, fail-closed.
    pub fn collect_actionable(
        &mut self,
        confirmed: &dyn ConfirmedView,
    ) -> Result<BTreeMap<ConsequenceId, AttemptedKey>, PipelineError> {
        let mut keys: BTreeMap<ConsequenceId, AttemptedKey> = BTreeMap::new();
        // The journal borrow is field-disjoint from every watermark
        // mutation below (NLL tracks the paths; no whole-self method
        // call happens while the slice is live).
        let transactions = self.journal.transactions();
        for transaction in transactions.iter().skip(self.settled_watermark) {
            self.scan_transactions += 1;
            // The settled-bitmap skip (out-of-order settlements the
            // mark has not absorbed).
            if self.settled_above.contains(&transaction.id) {
                continue;
            }
            if !self.journal.is_actionable(transaction.id) {
                // Durably canceled — permanently un-actionable
                // (cancellations are append-only): settle it for the
                // scan, no shed eligibility (its keys are never
                // confirmed).
                self.settled_above.insert(transaction.id);
                continue;
            }
            let mut settled = true;
            for envelope in &transaction.envelopes {
                self.scan_envelopes += 1;
                let effect = envelope
                    .effect_commitment()
                    .map_err(PipelineError::Format)?;
                if let Some(entry) = confirmed.confirmed(&envelope.id) {
                    // Exact provenance: the journal envelope must
                    // match the canonical entry — the same producing
                    // transaction, coverage, and EFFECT COMMITMENT
                    // (identity-level equality is content-blind; the
                    // effect commitment binds the actual authoritative
                    // effects); a mismatch is a conflicting claim,
                    // never silently skipped. A matching entry is one
                    // envelope's settlement proof — the coverage is
                    // guaranteed present here: a shed entry's producer
                    // is settled (this transaction would have been
                    // skipped), and a foreign entry naming a DIFFERENT
                    // producing transaction fails on the transaction
                    // arm before coverage is compared.
                    if entry.transaction == transaction.id
                        && entry.coverage == envelope.sub_spans
                        && entry.effect == effect
                    {
                        continue;
                    }
                    return Err(PipelineError::ConfirmedProvenanceMismatch { id: envelope.id });
                }
                settled = false;
                if keys.contains_key(&envelope.id) {
                    return Err(PipelineError::ConflictingReuse { id: envelope.id });
                }
                keys.insert(
                    envelope.id,
                    (envelope.sub_spans.clone(), transaction.id, effect),
                );
            }
            if settled {
                // THE SETTLEMENT: every envelope chain-confirmed under
                // exact provenance — the transaction's keys can never
                // become actionable again (confirmation is monotone
                // and the journal is append-only), so the scan will
                // never need to re-verify them. The identities feed
                // the shed-eligibility drain.
                self.settled_above.insert(transaction.id);
                self.newly_settled_keys
                    .extend(transaction.envelopes.iter().map(|envelope| envelope.id));
            }
        }
        // The mark advances through the settled prefix only — an
        // unconfirmed transaction holds it (fail-closed), and the
        // bitmap keeps the out-of-order settlements above it.
        while self.settled_watermark < transactions.len()
            && self
                .settled_above
                .contains(&transactions[self.settled_watermark].id)
        {
            self.settled_above
                .remove(&transactions[self.settled_watermark].id);
            self.settled_watermark += 1;
        }
        Ok(keys)
    }

    /// (P3-L6) Drains the envelope identities of the transactions the
    /// actionable scans settled since the last drain — the coverage
    /// shed's eligibility input. The facade's post-commit composition
    /// drains per commit; a direct half-composition that never drains
    /// forfeits the shed for those settlements (the coverage stays
    /// retained — never a correctness effect).
    fn take_newly_settled_keys(&mut self) -> Vec<ConsequenceId> {
        std::mem::take(&mut self.newly_settled_keys)
    }

    /// The producing transactions' journal position ticks for one
    /// collected key set — everything the submission derives from the
    /// journal, frozen into the immutable request (a transaction's
    /// position is fixed at capture, so this snapshot equals any
    /// later live lookup).
    pub fn transaction_ticks(
        &self,
        keys: &BTreeMap<ConsequenceId, AttemptedKey>,
    ) -> BTreeMap<TransactionId, u64> {
        let mut ticks: BTreeMap<TransactionId, u64> = BTreeMap::new();
        for (_, transaction, _) in keys.values() {
            // (P3-L3) The committed-index lookup replaces the per-key
            // linear scan over every committed transaction
            // (O(keys × transactions) → O(keys · log transactions));
            // an identity outside the committed set still freezes
            // nothing — the same absence the linear find produced.
            if let Some(position) = self.journal.transaction_position(*transaction) {
                ticks.insert(*transaction, position.tick);
            }
        }
        ticks
    }

    /// (R07) The tracked-key map's estimated resident bytes — the
    /// CA-17 formula discipline: per entry, the `KeyStatus` fixed
    /// fields, the `ConsequenceId` key, and the map's amortized
    /// per-entry node share. An ESTIMATE (std's exact node occupancy
    /// is its own business); the structural claim — linear in the
    /// generation's journaled identities at fixed live obligations —
    /// is pinned by the residency module's R07 census test.
    pub(crate) fn tracked_residency_bytes(&self) -> u64 {
        const BTREE_ENTRY_OVERHEAD: u64 = 48;
        (std::mem::size_of::<ConsequenceId>() as u64
            + std::mem::size_of::<KeyStatus>() as u64
            + BTREE_ENTRY_OVERHEAD)
            * self.keys.len() as u64
    }

    /// (R07) The staged accrual's estimated resident bytes — per
    /// staged envelope, the fixed struct plus the sub-span and
    /// dependency heap (the same formula discipline). The
    /// boundary-scoped surface: a crash loses it honestly (P0-5), so
    /// it is bounded by one capture boundary's batch, never by
    /// history.
    pub(crate) fn staged_residency_bytes(&self) -> u64 {
        let fixed = std::mem::size_of::<ConsequenceEnvelope>() as u64;
        let span = std::mem::size_of::<ClassifiedSpan>() as u64;
        let id = std::mem::size_of::<ConsequenceId>() as u64;
        self.staged
            .iter()
            .map(|envelope| {
                fixed
                    + envelope.sub_spans.capacity() as u64 * span
                    + envelope.dependencies.capacity() as u64 * id
            })
            .sum()
    }

    /// Promotes every tracked key the view holds confirmed evidence
    /// for — today's walk-closing promotion loop, run by the facade
    /// exactly when a walk completed (idempotent; the entry's
    /// confirming revision is the validated one).
    pub fn promote_from(&mut self, confirmed: &dyn ConfirmedView) {
        for (id, status) in &mut self.keys {
            if let Some(entry) = confirmed.confirmed(id) {
                status.phase = KeyPhase::InConfirmedChain;
                status.revision = Some(entry.revision);
            }
        }
    }
}

impl<M: CanonicalMedium> ConfirmedView for CommitVerifier<M> {
    fn confirmed(&self, id: &ConsequenceId) -> Option<&ConfirmedEntry> {
        self.confirmed.get(id)
    }
}

/// The attempted keys still unconfirmed after a re-filter (the rebase
/// set — identical identities and coverages) over a confirmed VIEW. A
/// foreign confirmation satisfies the local attempt only under EXACT
/// provenance equivalence: the canonical entry must carry the
/// attempt's own producing transaction, coverage, and EFFECT
/// COMMITMENT — a mismatched transaction, coverage, or
/// consequential payload is a conflicting claim, never silently
/// absorbed.
fn remaining_keys(
    attempted: BTreeMap<ConsequenceId, AttemptedKey>,
    confirmed: &dyn ConfirmedView,
) -> Result<BTreeMap<ConsequenceId, AttemptedKey>, PipelineError> {
    let mut remaining = BTreeMap::new();
    for (id, (coverage, transaction, effect)) in attempted {
        if let Some(entry) = confirmed.confirmed(&id) {
            if entry.transaction == transaction
                && entry.coverage == coverage
                && entry.effect == effect
            {
                // Confirmed by another writer's batch carrying
                // exactly this attempt — never re-submitted; the
                // per-key status promotes.
                continue;
            }
            return Err(PipelineError::ConfirmedProvenanceMismatch { id });
        }
        remaining.insert(id, (coverage, transaction, effect));
    }
    Ok(remaining)
}

impl<M: CanonicalMedium> CommitVerifier<M> {
    /// (F4-R11-B) The verifier's canonical medium — crate-private
    /// (the session-bound rotation's binding probe).
    pub(crate) fn medium_mut(&mut self) -> &mut M {
        &mut self.medium
    }
    /// Opens the verifier: the session-open chain walk (O(chain),
    /// documented honestly — never per tick) collects every
    /// chain-confirmed key from the `consequence-batch/1` objects and
    /// establishes the founding authority.
    pub fn open(medium: M) -> Result<Self, PipelineError> {
        let tier = medium.durability_tier();
        let mut verifier = CommitVerifier {
            medium,
            confirmed: BTreeMap::new(),
            shed_eligible: VecDeque::new(),
            shed_coverage: HashSet::new(),
            shed_coverage_total: 0,
            verified_head: None,
            verified_epoch: 0,
            founding: None,
            pending: None,
            tier,
            walk_rounds: 0,
        };
        verifier.walk_chain()?;
        Ok(verifier)
    }

    /// The retained unresolved canonical attempt's revision, if any.
    pub fn pending_revision(&self) -> Option<Hash> {
        self.pending.as_ref().map(|attempt| attempt.record.id())
    }

    /// The trusted cursor (the last walk-processed revision — the
    /// append basis; `None` on an unfounded medium).
    pub fn verified_head(&self) -> Option<Hash> {
        self.verified_head
    }

    /// Completed walks — the facade's promotion trigger (today's
    /// walk-closing promotion loop ran precisely per completed walk).
    pub fn completed_walks(&self) -> u64 {
        self.walk_rounds
    }

    /// Processes one immutable commit request to a terminal outcome:
    /// the remaining-keys filter (exact provenance against THIS
    /// truth — a view that over-included is corrected here, never
    /// trusted), then the submit/rebase machinery.
    pub fn submit(&mut self, request: CommitRequest) -> Result<CommitOutcome, PipelineError> {
        if let Some(pending) = &self.pending {
            return Err(PipelineError::AttemptUnresolved {
                revision: pending.record.id(),
            });
        }
        let Some(parent) = self.verified_head else {
            return Err(PipelineError::UnfoundedMedium);
        };
        let attempted: Vec<ConsequenceId> = request.keys.keys().copied().collect();
        let keys = remaining_keys(request.keys, self)?;
        if keys.is_empty() {
            // Every requested key is already chain-confirmed under
            // exact provenance (a lagged view's over-inclusion,
            // corrected here — unreachable through the synchronous
            // facade, whose collection runs over this same truth and
            // reports NothingPending first).
            let receipt = self.foreign_receipt(attempted)?;
            return Ok(CommitOutcome::Confirmed(receipt));
        }
        self.submit_keys(keys, parent, &request.ticks)
    }

    /// Reconciles a retained unknown attempt under the medium's lock:
    /// `Installed` still requires the full read-back walk and the
    /// individual per-key validation before confirmation; otherwise
    /// the attempt is retained (retry-safe or unverifiable — never
    /// falsely confirmed).
    ///
    /// PP-B4 checkpoint 2's exception safety: the verification runs on
    /// a CLONE — the slot keeps the candidate until confirmation
    /// succeeds, so a readback failure (worker death included) retains
    /// it, never a discarded unresolved obligation.
    pub fn reconcile(&mut self) -> Result<ReconcileOutcome, PipelineError> {
        let Some(attempt) = self.pending.clone() else {
            return Ok(ReconcileOutcome::NothingPending);
        };
        let candidate = attempt.record.id();
        match self.medium.reconcile_unknown(&candidate)? {
            Reconciliation::Installed { .. } => {
                let CommitOutcome::Confirmed(receipt) = self.confirm(&attempt)? else {
                    unreachable!("confirm returns Confirmed or errors");
                };
                self.pending = None;
                Ok(ReconcileOutcome::Confirmed(receipt))
            }
            Reconciliation::NotInstalled { .. } => Ok(ReconcileOutcome::NotInstalled),
            Reconciliation::Unverifiable { reason } => {
                Ok(ReconcileOutcome::Unverifiable { reason })
            }
        }
    }

    /// Retries a retained attempt (after `NotInstalled`, or a stale
    /// parent): re-walks the chain first (determining keys another
    /// writer may have confirmed — never re-submitted), then
    /// re-submits the remaining per-key identities — the same record
    /// bytes when the parent still matches, the rebase (identical
    /// identities and coverages, new parent) when it does not.
    ///
    /// PP-B4 checkpoint 2's exception safety: the ORIGINAL candidate
    /// stays in the slot through every pre-submission fallible step
    /// (the walk, the remaining-keys derivation, the foreign receipt —
    /// the slot clears only after that receipt verifies); the
    /// re-submission then owns the transition — a definite pre-commit
    /// failure of candidate B leaves the original A (its
    /// `NotInstalled` disposition stands; a later retry re-derives),
    /// while any possibly-installed B REPLACES A (append_for's
    /// write-at-uncertainty-or-acceptance) and is never overwritten
    /// back.
    pub fn retry(&mut self) -> Result<CommitOutcome, PipelineError> {
        let Some(attempt) = self.pending.clone() else {
            return Ok(CommitOutcome::NothingPending);
        };
        self.walk_chain()?;
        let attempted: Vec<ConsequenceId> = attempt.keys.keys().copied().collect();
        let remaining = remaining_keys(attempt.keys, self)?;
        if remaining.is_empty() {
            // Every attempted key is now chain-confirmed (another
            // writer's batch) — confirmed without our revision. The
            // slot clears ONLY after the receipt's verification.
            let receipt = self.foreign_receipt(attempted)?;
            self.pending = None;
            return Ok(CommitOutcome::Confirmed(receipt));
        }
        let parent = self.verified_head.ok_or(PipelineError::UnfoundedMedium)?;
        self.submit_keys(remaining, parent, &attempt.ticks)
    }

    /// Whether the identity is individually validated inside the
    /// verified canonical chain (the suppression authority).
    pub fn is_confirmed(&self, id: ConsequenceId) -> bool {
        self.confirmed.contains_key(&id)
    }

    /// One confirmed key's exact occurrence coverage (the suppression
    /// authority's span-exact form).
    ///
    /// (P3-L6) THE SHED CONTRACT: a settled producer's identity whose
    /// coverage detail was shed past the documented bound REFUSES
    /// (`None`) in-session — the membership ([`Self::is_confirmed`])
    /// survives, and the detail RE-DERIVES from the chain at the next
    /// session open (the walk's full batch collection — the recovery
    /// composition's fresh-open path); identities within the retained
    /// bound always answer with the full coverage.
    pub fn confirmed_coverage(&self, id: ConsequenceId) -> Option<&[ClassifiedSpan]> {
        if self.shed_coverage.contains(&id) {
            return None;
        }
        self.confirmed
            .get(&id)
            .map(|entry| entry.coverage.as_slice())
    }

    /// One confirmed key's producing transaction.
    pub fn confirmed_transaction(&self, id: ConsequenceId) -> Option<TransactionId> {
        self.confirmed.get(&id).map(|entry| entry.transaction)
    }

    /// The confirmed evidence export (the recovery composition's
    /// chain-side input).
    ///
    /// (P3-L6) THE SHED CONTRACT: identities whose coverage detail was
    /// shed past the documented bound are OMITTED — an export entry
    /// must carry the complete provenance or none (an empty-coverage
    /// export would misreport a span-bearing entry as a movement).
    /// The recovery composition runs over a FRESHLY OPENED pipeline
    /// by construction (the crossing's recovery driver opens the
    /// pipeline before consuming this export; its session walk
    /// re-derives every entry with full coverage), so the recovery's
    /// accountability never depends on shed detail; the shed
    /// population is reported in [`CommitPipeline::retention_stats`],
    /// never silent.
    pub fn confirmed_evidence(&self) -> BTreeMap<ConsequenceId, ConfirmedEvidence> {
        self.confirmed
            .iter()
            .filter(|(id, _)| !self.shed_coverage.contains(id))
            .map(|(id, entry)| {
                (
                    *id,
                    ConfirmedEvidence {
                        coverage: entry.coverage.clone(),
                        transaction: entry.transaction,
                        effect: entry.effect,
                    },
                )
            })
            .collect()
    }

    /// Every chain-confirmed identity.
    pub fn confirmed_keys(&self) -> Vec<ConsequenceId> {
        self.confirmed.keys().copied().collect()
    }

    /// (CA-17) The authenticated COLD membership/producer retrieval —
    /// the chain-side answer for one identity, derived WITHOUT the
    /// in-memory confirmed map: the same gap-aware head-down
    /// hash-verified walk [`CommitVerifier::walk_chain`] runs,
    /// targeted at one `ConsequenceId`, with the same
    /// conflicting-reuse refusal (two chain revisions claiming the
    /// identity fail closed, never guessed). Returns the entry a
    /// FRESHLY OPENED pipeline's session walk would collect — the
    /// eviction-boundary query surface: an identity whose coverage
    /// detail was shed (or, under the residency design's future
    /// eviction, whose entry left the hot map entirely) answers here
    /// with its FULL chain-proven evidence, authenticated by the
    /// medium's hash-verified reads. O(chain) per query — the honest
    /// cold cost the residency design's measurement rows
    /// characterize (see the `residency` module's design record).
    pub fn cold_confirmed(
        &mut self,
        id: ConsequenceId,
    ) -> Result<Option<ConfirmedEntry>, PipelineError> {
        crate::residency::probe_confirmed(&mut self.medium, id)
    }

    /// (CA-17) The confirmed map's estimated resident bytes — the
    /// documented formula: per entry, the fixed `ConfirmedEntry`
    /// fields (`size_of::<ConfirmedEntry>()`), the retained coverage
    /// spans (`capacity × size_of::<ClassifiedSpan>()`), the
    /// `ConsequenceId` key, and the map's amortized per-entry node
    /// share. An ESTIMATE (the map's exact node occupancy is std's
    /// own business); the structural claim it exists to measure —
    /// linear growth in confirmed history at fixed live obligations
    /// — is pinned by the residency module's tests.
    pub(crate) fn confirmed_residency_bytes(&self) -> u64 {
        const BTREE_ENTRY_OVERHEAD: u64 = 48;
        let fixed = std::mem::size_of::<ConfirmedEntry>() as u64
            + std::mem::size_of::<ConsequenceId>() as u64
            + BTREE_ENTRY_OVERHEAD;
        let span = std::mem::size_of::<ClassifiedSpan>() as u64;
        self.confirmed
            .values()
            .map(|entry| fixed + entry.coverage.capacity() as u64 * span)
            .sum()
    }

    /// (CA-17) The shed discriminator set's estimated resident bytes
    /// — one slot per shed identity (the key plus the set's amortized
    /// slot overhead): unbounded in the shed population today, the
    /// finding's second growth term.
    pub(crate) fn shed_residency_bytes(&self) -> u64 {
        const HASHSET_ENTRY_OVERHEAD: u64 = 32;
        (std::mem::size_of::<ConsequenceId>() as u64 + HASHSET_ENTRY_OVERHEAD)
            * self.shed_coverage.len() as u64
    }

    /// (R07) The retained unresolved canonical attempt's estimated
    /// resident bytes — the pending slot's single retained copy (the
    /// P3-L3 write-at-uncertainty-or-acceptance shape): the revision
    /// record (with its idempotency-key, content-ref, and dependency
    /// heap), the batch content identity, the per-key evidence (the
    /// identity, the tuple's fixed fields, the map's node share, and
    /// the coverage heap), and the frozen per-transaction ticks.
    /// Attempt-scoped by construction: ZERO whenever no attempt is
    /// retained, and at most ONE live attempt exists.
    pub(crate) fn pending_residency_bytes(&self) -> u64 {
        const BTREE_ENTRY_OVERHEAD: u64 = 48;
        let Some(attempt) = &self.pending else {
            return 0;
        };
        let mut total = std::mem::size_of::<RevisionRecord>() as u64
            + Hash::WIDTH
            + attempt
                .record
                .idempotency_key
                .as_ref()
                .map_or(0, |key| key.capacity() as u64)
            + attempt.record.content_refs.capacity() as u64 * Hash::WIDTH
            + attempt.record.depends_on.capacity() as u64 * Hash::WIDTH;
        let span = std::mem::size_of::<ClassifiedSpan>() as u64;
        for (coverage, _, _) in attempt.keys.values() {
            total += std::mem::size_of::<ConsequenceId>() as u64
                + std::mem::size_of::<AttemptedKey>() as u64
                + BTREE_ENTRY_OVERHEAD
                + coverage.capacity() as u64 * span;
        }
        total += (std::mem::size_of::<TransactionId>() as u64
            + std::mem::size_of::<u64>() as u64
            + BTREE_ENTRY_OVERHEAD)
            * attempt.ticks.len() as u64;
        total
    }

    /// What `Accepted` establishes on this medium.
    pub fn sync_tier(&self) -> SyncTier {
        self.tier
    }

    /// The founding authority mode and campaign id.
    pub fn authority(&self) -> Option<(AuthorityMode, &str)> {
        self.founding
            .as_ref()
            .map(|(mode, campaign)| (*mode, campaign.as_str()))
    }

    // ---------------------------------------------------------- internals

    /// Builds the batch for `keys`, submits it against `parent`, and
    /// handles the stale-rebase once. The idempotency key is the
    /// batch's content hash — the rebase preserves per-key
    /// identities, and the medium's chain-wide key uniqueness makes a
    /// double-install unrepresentable. (P3-L3) `keys` MOVES into the
    /// retained attempt — the single key-set copy on this path — and
    /// the stale-parent re-derivation reads it back from the pending
    /// slot, exactly as `retry` does.
    fn submit_keys(
        &mut self,
        keys: BTreeMap<ConsequenceId, AttemptedKey>,
        parent: Hash,
        ticks: &BTreeMap<TransactionId, u64>,
    ) -> Result<CommitOutcome, PipelineError> {
        let (revision, outcome) = self.append_for(keys, parent, ticks)?;
        match outcome {
            AppendOutcome::Accepted { id, tier: _ } => {
                if id != revision {
                    return Err(PipelineError::ConfirmationInvalid {
                        detail: "accepted-revision-identity-mismatch",
                    });
                }
                self.confirm_retained()
            }
            AppendOutcome::RejectedStaleParent { observed } => {
                // Revalidate: the observed chain was validated by the
                // medium before the rejection; the suffix walk
                // determines the now-confirmed keys.
                self.walk_chain()?;
                // (P3-L3) The re-derivation reads the RETAINED
                // candidate — `retry`'s demonstrated shape: a clone of
                // the slot's key set feeds the fallible re-derivation,
                // so the original candidate stays in the slot through
                // every pre-rebase step (the second append_for
                // replaces it only on its own
                // write-at-uncertainty-or-acceptance).
                let retained = self
                    .pending
                    .as_ref()
                    .expect("append_for retains the candidate it just built")
                    .keys
                    .clone();
                let attempted: Vec<ConsequenceId> = retained.keys().copied().collect();
                let remaining = remaining_keys(retained, self)?;
                if remaining.is_empty() {
                    // Every attempted key is now chain-confirmed (another
                    // writer's batch) — confirmed without our revision. The
                    // exact foreign provenance verifies FIRST; the pending
                    // slot clears only after the receipt succeeds (C2-F1: a
                    // verified foreign confirmation must clear the retained
                    // candidate — never leave the caller's next
                    // commit_canonical refusing AttemptUnresolved; a
                    // fallible verification failure preserves it).
                    let receipt = self.foreign_receipt(attempted)?;
                    self.pending = None;
                    return Ok(CommitOutcome::Confirmed(receipt));
                }
                let observed_parent = observed
                    .or(self.verified_head)
                    .ok_or(PipelineError::UnfoundedMedium)?;
                let (revision, outcome) = self.append_for(remaining, observed_parent, ticks)?;
                match outcome {
                    AppendOutcome::Accepted { id, tier: _ } => {
                        if id != revision {
                            return Err(PipelineError::ConfirmationInvalid {
                                detail: "accepted-revision-identity-mismatch",
                            });
                        }
                        self.confirm_retained()
                    }
                    // The rebase was rejected again — append_for already
                    // retains the last built attempt for retry_pending.
                    AppendOutcome::RejectedStaleParent { observed } => {
                        Ok(CommitOutcome::StaleParent { observed })
                    }
                    AppendOutcome::Unknown { reason } => {
                        Ok(CommitOutcome::Unknown { revision, reason })
                    }
                }
            }
            AppendOutcome::Unknown { reason } => Ok(CommitOutcome::Unknown { revision, reason }),
        }
    }

    /// The confirmation procedure's exception-safe entry (PP-B4
    /// checkpoint 2): `confirm` runs on the slot's SINGLE retained
    /// copy — taken out for the verification and restored on every
    /// failure arm, so the pending slot keeps the candidate until
    /// verified confirmation (append_for's
    /// write-at-uncertainty-or-acceptance wrote it; the success path
    /// below is the one place the slot clears). A worker-transport
    /// failure during the readback returns `CommitOutcome::Unknown`
    /// with `WorkerReplyLost` — the append was accepted (its
    /// disposition stands) while the verification could not complete:
    /// never a definite failure, never a false confirmation. Any
    /// other readback error propagates with the candidate restored to
    /// the slot for reconciliation. (The verification window holds
    /// the taken copy; every error exit below restores it, and the
    /// only panic site inside `confirm` is the merge loop's
    /// provably-unreachable exhaustion guard — equal key counts on
    /// both sides of a sorted merge.)
    fn confirm_retained(&mut self) -> Result<CommitOutcome, PipelineError> {
        let attempt = self
            .pending
            .take()
            .expect("append_for retains the candidate before the confirmation");
        let revision = attempt.record.id();
        match self.confirm(&attempt) {
            Ok(outcome) => {
                // Verified confirmation: the single copy is consumed
                // — the slot stays cleared.
                Ok(outcome)
            }
            Err(PipelineError::Medium(MediumError::WorkerUnavailable { .. })) => {
                self.pending = Some(attempt);
                Ok(CommitOutcome::Unknown {
                    revision,
                    reason: UnknownReason::WorkerReplyLost,
                })
            }
            Err(error) => {
                self.pending = Some(attempt);
                Err(error)
            }
        }
    }

    /// Builds the batch content object and the revision record for
    /// `keys` on `parent`, and appends. The tick derives
    /// deterministically from the batched transactions' journaled
    /// positions (retry stability — carried frozen in the request and
    /// the retained attempt). (P3-L3) `keys` moves into the attempt —
    /// the single key-set copy — and the attempt MOVES into the
    /// pending slot (no clone anywhere on this path); the caller
    /// receives only the revision identity and the append outcome.
    ///
    /// THE CANDIDATE-STATE RULE (PP-B4 checkpoint 2, the preflight's
    /// four-state lifecycle): a DEFINITE pre-commit failure (the
    /// medium's own contract — and the worker proxy's discipline:
    /// uncertain dispositions arrive as `Unknown` outcomes, never
    /// errors) returns WITHOUT writing the pending slot, preserving
    /// the accepted direct-retry behavior; every outcome that leaves
    /// the candidate possibly installed (`Accepted`, `Unknown`, or a
    /// stale-parent rebase target) writes the slot BEFORE the caller
    /// takes any further fallible step, so the reconciliation
    /// identity can never be lost to a readback failure. The slot is
    /// cleared only by verified confirmation.
    fn append_for(
        &mut self,
        keys: BTreeMap<ConsequenceId, AttemptedKey>,
        parent: Hash,
        ticks: &BTreeMap<TransactionId, u64>,
    ) -> Result<(Hash, AppendOutcome), PipelineError> {
        let mut entries: Vec<BatchEntry> = Vec::with_capacity(keys.len());
        let mut tick: u64 = 0;
        for (id, (coverage, transaction, effect)) in &keys {
            entries.push(BatchEntry {
                id: *id,
                transaction: *transaction,
                coverage: coverage.clone(),
                effect: *effect,
            });
            if let Some(position) = ticks.get(transaction) {
                tick = tick.max(*position);
            }
        }
        let batch = ConsequenceBatch { entries };
        let batch_bytes = batch.canonical_bytes().map_err(PipelineError::Format)?;
        let batch_object = StoredObject::new(batch_bytes)?;
        let batch_hash = batch_object.hash();
        // The idempotency key IS the batch's content identity —
        // chain-unique, so the same logical batch can never
        // double-install however often it is retried or rebased.
        // (F-14) The construction-boundary guard: the verified epoch
        // is walk-derived from provider records — an epoch at
        // u64::MAX has no representable successor, and the typed
        // refusal precedes the constructor (the medium's append-side
        // checked linkage is the second layer).
        self.verified_epoch
            .checked_add(1)
            .ok_or(PipelineError::Medium(MediumError::Chain {
                rule: "epoch-regression",
            }))?;
        let record = RevisionRecord::regular(parent, self.verified_epoch, tick)
            .with_idempotency_key(batch_hash.as_bytes().to_vec())
            .with_content_refs(vec![batch_hash]);
        let outcome =
            self.medium
                .append(&record, std::slice::from_ref(&batch_object), Some(parent))?;
        let revision = record.id();
        // Write-at-uncertainty-or-acceptance (P3-L3's single
        // retention copy): the attempt — keys moved in, ticks frozen —
        // MOVES into the pending slot, the one copy the confirmation
        // borrows; only its revision identity returns to the caller.
        // The retained candidate precedes every further fallible step
        // (the readback included); cleared only by verified
        // confirmation.
        self.pending = Some(PendingAttempt {
            record,
            batch_hash,
            keys,
            ticks: ticks.clone(),
        });
        Ok((revision, outcome))
    }

    /// The confirmation procedure: the verified read-back walk (a
    /// bare Accepted is never confirmation), the batch read back by
    /// its content identity, and the **individual validation** of
    /// every attempted key — present, with exactly the attempted
    /// coverage and producing transaction, placed by the walk at the
    /// confirming revision, with no unattempted keys. (P3-L3) The
    /// procedure runs on a BORROW: the commit path lends the pending
    /// slot's single retained copy (via [`Self::confirm_retained`],
    /// which restores it on every failure); `reconcile` lends its
    /// retention clone.
    fn confirm(&mut self, attempt: &PendingAttempt) -> Result<CommitOutcome, PipelineError> {
        self.walk_chain()?;
        let bytes = self.medium.read_object(&attempt.batch_hash)?;
        let batch = ConsequenceBatch::decode(&bytes).map_err(PipelineError::Format)?;
        if batch.entries.len() != attempt.keys.len() {
            return Err(PipelineError::ConfirmationInvalid {
                detail: "batch-key-set-mismatch",
            });
        }
        let revision = attempt.record.id();
        // Both attempt.keys (BTreeMap) and batch.entries (Vec) are sorted
        // by ConsequenceId. Use a single ordered merge/zip for O(N) matching
        // instead of O(N²) per-key linear search. This preserves the exact
        // per-key check order: key-missing → payload-mismatch →
        // key-not-chain-included → key-confirmed-by-different-revision.
        let mut batch_iter = batch.entries.iter().peekable();
        for (id, (coverage, transaction, effect)) in &attempt.keys {
            // Check if there are any batch entries with smaller ids (unattempted keys)
            if let Some(entry) = batch_iter.peek()
                && entry.id < *id
            {
                // Batch entry has smaller id - this is an unattempted key
                return Err(PipelineError::ConfirmationInvalid {
                    detail: "key-missing-from-confirmed-batch",
                });
            }
            // Now batch_iter.peek() is either None or has entry.id >= id
            let entry = batch_iter.next().expect("batch entries exhausted early");
            if entry.id != *id {
                // We skipped an entry or hit a larger one - key is missing
                return Err(PipelineError::ConfirmationInvalid {
                    detail: "key-missing-from-confirmed-batch",
                });
            }
            // ids match - perform validation checks in exact order
            if entry.coverage != *coverage
                || entry.transaction != *transaction
                || entry.effect != *effect
            {
                return Err(PipelineError::ConfirmationInvalid {
                    detail: "payload-mismatch-in-confirmed-batch",
                });
            }
            let Some(confirmed) = self.confirmed.get(id) else {
                return Err(PipelineError::ConfirmationInvalid {
                    detail: "key-not-chain-included",
                });
            };
            if confirmed.revision != revision {
                return Err(PipelineError::ConfirmationInvalid {
                    detail: "key-confirmed-by-different-revision",
                });
            }
        }
        // The attempted keys' phase promotion (today's in-confirm
        // loop) is the facade's post-operation promote_from — the
        // walk completed, so it runs; the entry's confirming revision
        // is the validated one.
        let (authority, _) = self
            .founding
            .as_ref()
            .ok_or(PipelineError::UnfoundedMedium)?;
        let label = match authority {
            AuthorityMode::Standalone => ConfirmationLabel::T3LocalStandalone,
            AuthorityMode::Shared => ConfirmationLabel::SharedCanonical,
        };
        Ok(CommitOutcome::Confirmed(ConfirmationReceipt {
            revision,
            epoch: attempt.record.epoch,
            keys: attempt.keys.keys().copied().collect(),
            tier: self.tier,
            authority: *authority,
            label,
        }))
    }

    /// A confirmation receipt for keys another writer's revision
    /// confirmed. Per-key exact provenance: the receipt reports the
    /// revision that ACTUALLY confirmed the keys — never the current
    /// (possibly unrelated, later) HEAD. Keys confirmed by distinct
    /// revisions cannot collapse into one honest receipt: fail
    /// closed rather than misreport any key's provenance.
    fn foreign_receipt(
        &self,
        keys: Vec<ConsequenceId>,
    ) -> Result<ConfirmationReceipt, PipelineError> {
        let (authority, _) = self
            .founding
            .as_ref()
            .ok_or(PipelineError::UnfoundedMedium)?;
        let mut confirming: Option<(Hash, u64)> = None;
        for key in &keys {
            let entry = self
                .confirmed
                .get(key)
                .ok_or(PipelineError::UnknownKey { id: *key })?;
            match confirming {
                None => confirming = Some((entry.revision, entry.epoch)),
                Some((revision, _)) if revision == entry.revision => {}
                Some((revision, _)) => {
                    return Err(PipelineError::ForeignProvenanceDivergent {
                        id: *key,
                        first_revision: revision,
                        second_revision: entry.revision,
                    });
                }
            }
        }
        let (revision, epoch) = confirming.ok_or(PipelineError::UnfoundedMedium)?;
        Ok(ConfirmationReceipt {
            revision,
            epoch,
            keys,
            tier: self.tier,
            authority: *authority,
            label: match authority {
                AuthorityMode::Standalone => ConfirmationLabel::T3LocalStandalone,
                AuthorityMode::Shared => ConfirmationLabel::SharedCanonical,
            },
        })
    }

    /// The verified chain walk: re-reads every new record
    /// (hash-verified by the medium) from the head down to the last
    /// processed revision, collects every `consequence-batch/1`
    /// entry per key (the suppression authority), and promotes
    /// tracked keys. Non-batch content objects are skipped — the
    /// store is domain-agnostic; only consequence batches carry
    /// keys. O(new revisions) within a session; O(chain) at open —
    /// never per tick.
    fn walk_chain(&mut self) -> Result<(), PipelineError> {
        let Some(head) = self.medium.head()? else {
            return Ok(());
        };
        let mut chain: Vec<(Hash, RevisionRecord)> = Vec::new();
        let mut cursor = head;
        // THE COMPACTED MODE (PP-B3): an absent parent is a gap —
        // legitimate only under the most recent on-chain MK's skip
        // table (recognized by the record's first content object
        // carrying the checkpoint-metadata domain); anything else is
        // hard corruption, fail closed. The gap-aware descent lets
        // the confirmed-evidence collection reach the batches BELOW
        // reclaimed ranges through the permanent skips.
        let mut skip_table: Option<(Hash, CheckpointMetadata)> = None;
        // THE S6-C4 READ-PATH CORRECTIVE (the within-walk verified
        // classification memo): each referenced object is examined —
        // and its FULL content hash verified through the medium's
        // verified-classification operation — AT MOST ONCE per walk;
        // the same walk's later examination of the same object reuses
        // the classification. The memo is walk-local: EVERY walk
        // re-verifies every object it examines (a corruption
        // introduced between two walks is detected by the second
        // walk — the accepted timing guarantee preserved); objects
        // whose content the walk CONSUMES (consequence batches,
        // checkpoint metadata) always receive the full verified read.
        let mut classified: HashMap<Hash, ObjectClass> = HashMap::new();
        loop {
            if self.verified_head == Some(cursor) {
                break;
            }
            let record = self.medium.read_record(&cursor)?;
            let parent = record.parent;
            let current = cursor;
            if let Some(object) = record.content_refs.first() {
                let class = self.classify_once(object, &mut classified)?;
                if class.domain.as_deref() == Some(CHECKPOINT_METADATA_DOMAIN) {
                    let bytes = self.read_verified(object)?;
                    let metadata =
                        CheckpointMetadata::decode(&bytes).map_err(PipelineError::Format)?;
                    skip_table = Some((current, metadata));
                }
            }
            chain.push((cursor, record));
            match parent {
                Some(parent) => match self.medium.read_record(&parent) {
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
        for (id, record) in chain.into_iter().rev() {
            if let RecordKind::Founding {
                authority,
                campaign_id,
            } = &record.kind
            {
                self.founding = Some((*authority, campaign_id.clone()));
            }
            for content in &record.content_refs {
                let class = self.classify_once(content, &mut classified)?;
                if class.domain.as_deref() == Some(CONSEQUENCE_BATCH_DOMAIN) {
                    let bytes = self.read_verified(content)?;
                    let batch = ConsequenceBatch::decode(&bytes).map_err(PipelineError::Format)?;
                    for entry in batch.entries {
                        if let Some(existing) = self.confirmed.get(&entry.id) {
                            // Conflicting reuse of a ConsequenceId
                            // across valid canonical batches: two
                            // chain revisions claim the same identity.
                            // The exact-provenance standard rejects
                            // the second — never overwriting the first
                            // revision's evidence, never guessing
                            // which is authoritative.
                            return Err(PipelineError::CanonicalIdentityConflict {
                                id: entry.id,
                                first_revision: existing.revision,
                                second_revision: id,
                            });
                        }
                        self.confirmed.insert(
                            entry.id,
                            ConfirmedEntry {
                                coverage: entry.coverage,
                                transaction: entry.transaction,
                                effect: entry.effect,
                                revision: id,
                                epoch: record.epoch,
                            },
                        );
                    }
                }
            }
            self.verified_head = Some(id);
            self.verified_epoch = record.epoch;
        }
        // A completed walk (today's walk-closing promotion trigger —
        // the facade promotes tracked keys exactly when this
        // advances).
        self.walk_rounds += 1;
        Ok(())
    }

    /// One walk's verified classification of a referenced object,
    /// memoized within that walk (the S6-C4 read-path corrective):
    /// the FIRST examination performs the medium's verified
    /// classification — the full object bytes hash-checked against
    /// the identity, exactly as `read_object` does — and later
    /// examinations of the same object within the SAME walk reuse
    /// the result. The memo is walk-local by construction: the next
    /// walk starts empty, so every object it examines is re-verified
    /// (the corruption-between-walks timing guarantee preserved).
    fn classify_once(
        &mut self,
        hash: &Hash,
        memo: &mut HashMap<Hash, ObjectClass>,
    ) -> Result<ObjectClass, PipelineError> {
        if let Some(class) = memo.get(hash) {
            return Ok(class.clone());
        }
        let class = self.medium.classify_object(hash)?;
        memo.insert(*hash, class.clone());
        Ok(class)
    }

    /// The full verified read for an object whose content this walk
    /// CONSUMES (a consequence batch or checkpoint metadata) — never
    /// memoized, never bypassed: the complete bytes are read back and
    /// hash-verified, and the existing semantic validation runs over
    /// them exactly as before.
    fn read_verified(&mut self, hash: &Hash) -> Result<Vec<u8>, PipelineError> {
        Ok(self.medium.read_object(hash)?)
    }

    /// (P3-L6) THE SETTLED-COVERAGE SHED: records the drained
    /// settlement identities as shed-ELIGIBLE (their producing
    /// journal transactions settled — the actionable scan's
    /// per-transaction proof), then enforces the documented retention
    /// bound: beyond [`SETTLED_COVERAGE_RETAINED`] retained entries,
    /// the oldest eligible entry's coverage Vec is dropped (the
    /// identity, producing transaction, effect commitment, revision,
    /// and epoch are ALWAYS kept — the dedup membership is never
    /// dropped). Fail-closed by construction: ONLY settled producers'
    /// identities ever enter the queue (an entry whose producer is
    /// unproven — an unsettled or partially-confirmed transaction —
    /// is never eligible and keeps its full coverage for the
    /// exact-provenance comparisons that consult it). The shed entry
    /// is marked in the discriminator set so a legitimately
    /// empty-coverage movement can never be confused with it. The
    /// detail re-derives from the chain at the next session open.
    fn shed_settled_coverage(&mut self, eligible: Vec<ConsequenceId>) {
        for id in eligible {
            self.shed_eligible.push_back(id);
        }
        while self.shed_eligible.len() > SETTLED_COVERAGE_RETAINED {
            let id = self
                .shed_eligible
                .pop_front()
                .expect("the bound check preceded the pop");
            // A settled transaction's every envelope was confirmed
            // under exact provenance (the settlement proof), and the
            // confirmed map is insert-only — the entry exists; the
            // guard is defensive shape, never a reachable skip.
            if let Some(entry) = self.confirmed.get_mut(&id) {
                entry.coverage = Vec::new();
                self.shed_coverage.insert(id);
                self.shed_coverage_total += 1;
            }
        }
    }
}

impl<M: CanonicalMedium> CommitPipeline<M> {
    /// Opens the pipeline: the journal's committed transactions seed
    /// the per-key tracking (the T1 state is durable across sessions),
    /// then the session-open chain walk (O(chain), documented honestly
    /// — never per tick) collects every chain-confirmed key from the
    /// `consequence-batch/1` objects, promotes them, and establishes
    /// the founding authority.
    pub fn open(medium: M, journal: T1Journal) -> Result<Self, PipelineError> {
        let mut authority = CommitAuthority::open(journal);
        let verifier = CommitVerifier::open(medium)?;
        authority.promote_from(&verifier);
        Ok(CommitPipeline {
            authority,
            verifier,
        })
    }

    /// The pipeline's journal (read-only access — the rotation's
    /// classification and the diagnostics read the durable T1
    /// state; the instance and its sidecar lock stay owned here).
    pub fn journal(&self) -> &T1Journal {
        self.authority.journal()
    }

    /// (F4-R11-B) The pipeline's canonical medium — crate-private:
    /// the session-bound rotation entry probes the campaign's
    /// installed binding through the pipeline's own medium (the
    /// bound identities are never caller-supplied).
    pub(crate) fn medium_mut(&mut self) -> &mut M {
        self.verifier.medium_mut()
    }

    /// Stages one consequential envelope — the in-memory accrual
    /// (never guaranteed across a crash). A tracked identity staged
    /// again with different content is a conflicting reuse and fails
    /// closed; a byte-identical restage is a no-op.
    pub fn stage(&mut self, envelope: ConsequenceEnvelope) -> Result<(), PipelineError> {
        self.authority.stage(envelope)
    }

    /// Discards staged (in-memory, uncaptured) envelopes for the given
    /// identities — the failed-boundary path: effects staged but never
    /// captured are in-memory pending only; discarding them never
    /// touches journaled or confirmed keys (P0-5 — in-memory state is
    /// honestly lost). A retried boundary re-stages identical bytes.
    pub fn discard_staged(&mut self, ids: &[ConsequenceId]) {
        self.authority.discard_staged(ids)
    }

    /// The capture boundary: journals the staged consequences as one
    /// T1 transaction (the receipt returns only after the journal's
    /// fsync). An ambiguous commit outcome is reconciled immediately
    /// — reread and verified, never guessed; the recovery of a valid
    /// committed frame is **not** a fabricated acknowledgment. An
    /// aborted capture is retry-safe with the staged set retained.
    pub fn capture(
        &mut self,
        position: SourcePosition,
        classification: DurabilityClass,
        dependencies: Vec<TransactionId>,
        frontier: Vec<FrontierEntry>,
        plan: Vec<u8>,
    ) -> Result<CaptureReceipt, PipelineError> {
        self.authority
            .capture(position, classification, dependencies, frontier, plan)
    }

    /// Appends a validated, durable, reason-coded cancellation for a
    /// journaled transaction (the pipeline proxies the journal —
    /// supersession is the explicit durable record, never a higher
    /// sequence).
    pub fn cancel_transaction(
        &mut self,
        target: TransactionId,
        reason: CancellationReason,
        detail: &[u8],
    ) -> Result<u64, PipelineError> {
        self.authority.cancel_transaction(target, reason, detail)
    }

    /// The application reports effects applied to the live
    /// authoritative state (the pending-vs-confirmed distinction —
    /// never conflated with durability). A key must be journaled
    /// first (durable-before-discard).
    pub fn mark_applied(&mut self, keys: &[ConsequenceId]) -> Result<(), PipelineError> {
        self.authority.mark_applied(keys)
    }

    /// Prepares the canonical batch from every actionable,
    /// unconfirmed journaled key and submits it against the verified
    /// head. Canceled transactions are excluded (superseded — never
    /// canonically committed); chain-confirmed keys are never
    /// re-batched.
    pub fn commit_canonical(&mut self) -> Result<CommitOutcome, PipelineError> {
        // Today's walk-closing promotion: the authority promotes
        // tracked keys exactly when a walk completed during the
        // operation (the truth only advances inside completed walks,
        // so an unchanged count means no promotion is due — today's
        // error paths before any walk promoted nothing).
        let before = self.verifier.completed_walks();
        let result = self.commit_canonical_inner();
        if self.verifier.completed_walks() != before {
            self.authority.promote_from(&self.verifier);
        }
        // (P3-L6) THE SETTLED-COVERAGE SHED composition: the
        // actionable scan's settlements (recorded even when the
        // operation later failed — each settlement proof is
        // per-transaction and independent) drain into the verifier's
        // eligibility queue, which enforces the documented retention
        // bound. The shed never touches the dedup membership; a
        // failed operation with no settlements drains nothing.
        let settled_keys = self.authority.take_newly_settled_keys();
        if !settled_keys.is_empty() {
            self.verifier.shed_settled_coverage(settled_keys);
        }
        result
    }

    fn commit_canonical_inner(&mut self) -> Result<CommitOutcome, PipelineError> {
        if let Some(revision) = self.verifier.pending_revision() {
            return Err(PipelineError::AttemptUnresolved { revision });
        }
        if self.verifier.verified_head().is_none() {
            return Err(PipelineError::UnfoundedMedium);
        }
        let keys = self.authority.collect_actionable(&self.verifier)?;
        if keys.is_empty() {
            return Ok(CommitOutcome::NothingPending);
        }
        let ticks = self.authority.transaction_ticks(&keys);
        self.verifier.submit(CommitRequest { keys, ticks })
    }

    /// Reconciles a retained unknown attempt under the medium's lock:
    /// `Installed` still requires the full read-back walk and the
    /// individual per-key validation before confirmation; otherwise
    /// the attempt is retained (retry-safe or unverifiable — never
    /// falsely confirmed).
    ///
    /// PP-B4 checkpoint 2's exception safety: the verification runs on
    /// a CLONE — the slot keeps the candidate until confirmation
    /// succeeds, so a readback failure (worker death included) retains
    /// it, never a discarded unresolved obligation.
    pub fn reconcile_pending(&mut self) -> Result<ReconcileOutcome, PipelineError> {
        let before = self.verifier.completed_walks();
        let result = self.verifier.reconcile();
        if self.verifier.completed_walks() != before {
            self.authority.promote_from(&self.verifier);
        }
        result
    }

    /// Retries a retained attempt (after `NotInstalled`, or a stale
    /// parent): re-walks the chain first (determining keys another
    /// writer may have confirmed — never re-submitted), then
    /// re-submits the remaining per-key identities — the same record
    /// bytes when the parent still matches, the rebase (identical
    /// identities and coverages, new parent) when it does not.
    ///
    /// PP-B4 checkpoint 2's exception safety: the ORIGINAL candidate
    /// stays in the slot through every pre-submission fallible step
    /// (the walk, the remaining-keys derivation, the foreign receipt —
    /// the slot clears only after that receipt verifies); the
    /// re-submission then owns the transition — a definite pre-commit
    /// failure of candidate B leaves the original A (its
    /// `NotInstalled` disposition stands; a later retry re-derives),
    /// while any possibly-installed B REPLACES A (append_for's
    /// write-at-uncertainty-or-acceptance) and is never overwritten
    /// back.
    pub fn retry_pending(&mut self) -> Result<CommitOutcome, PipelineError> {
        let before = self.verifier.completed_walks();
        let result = self.verifier.retry();
        if self.verifier.completed_walks() != before {
            self.authority.promote_from(&self.verifier);
        }
        result
    }

    /// One key's pipeline view.
    pub fn status(&self, id: ConsequenceId) -> Option<KeyStatus> {
        self.authority.status(id)
    }

    /// Whether the identity is individually validated inside the
    /// verified canonical chain (the suppression authority).
    pub fn is_confirmed(&self, id: ConsequenceId) -> bool {
        self.verifier.is_confirmed(id)
    }

    /// The chain-confirmed occurrence coverage for one identity (the
    /// per-key evidence — never a batch aggregate alone).
    pub fn confirmed_coverage(&self, id: ConsequenceId) -> Option<&[ClassifiedSpan]> {
        self.verifier.confirmed_coverage(id)
    }

    /// The producing transaction for one chain-confirmed identity
    /// (the per-key provenance — the transaction whose captured
    /// effects the confirming batch installed).
    pub fn confirmed_transaction(&self, id: ConsequenceId) -> Option<TransactionId> {
        self.verifier.confirmed_transaction(id)
    }

    /// The chain-confirmed canonical evidence per identity — the
    /// recovery interface's provenance input (the coverage, producing
    /// transaction, and effect commitment; a bare identity set cannot
    /// validate journal envelopes against the canonical entries).
    pub fn confirmed_evidence(&self) -> BTreeMap<ConsequenceId, ConfirmedEvidence> {
        self.verifier.confirmed_evidence()
    }

    /// The confirmed key set (the pipeline's session view).
    pub fn confirmed_keys(&self) -> Vec<ConsequenceId> {
        self.verifier.confirmed_keys()
    }

    /// (CA-17) The authenticated cold membership/producer retrieval
    /// (see [`CommitVerifier::cold_confirmed`]) — the pipeline-level
    /// surface: the chain-side answer for one identity, full
    /// provenance included, without the in-memory map. O(chain) per
    /// query (the honest cold cost the residency design measures).
    pub fn cold_confirmed(
        &mut self,
        id: ConsequenceId,
    ) -> Result<Option<ConfirmedEntry>, PipelineError> {
        self.verifier.cold_confirmed(id)
    }

    /// (P3-L6) The retention/compaction observability surface — the
    /// settled-transaction watermark, the unsettled window the
    /// per-commit actionable scan pays for, the coverage-shed state,
    /// and the rotation-cadence reading (see [`RetentionStats`]).
    /// Read-only derived state: the growth a generation past the
    /// documented cadence threshold is REPORTED here for the
    /// maintenance layer's rotation decision, never silently
    /// accepted, never auto-triggered.
    pub fn retention_stats(&self) -> RetentionStats {
        let journal_transactions = self.authority.journal.transactions().len() as u64;
        let settled_above_watermark = self.authority.settled_above.len() as u64;
        let settled = self.authority.settled_watermark as u64 + settled_above_watermark;
        RetentionStats {
            journal_transactions,
            settled_watermark: self.authority.settled_watermark as u64,
            settled_above_watermark,
            unsettled_transactions: journal_transactions.saturating_sub(settled),
            tracked_keys: self.authority.keys.len() as u64,
            confirmed_keys: self.verifier.confirmed.len() as u64,
            shed_coverage_entries: self.verifier.shed_coverage_total,
            retained_settled_coverage: self.verifier.shed_eligible.len() as u64,
            actionable_scan_transactions: self.authority.scan_transactions,
            actionable_scan_envelopes: self.authority.scan_envelopes,
            exceeds_rotation_cadence: journal_transactions >= ROTATION_CADENCE_TRANSACTIONS,
            confirmed_residency_bytes: self.verifier.confirmed_residency_bytes(),
            shed_residency_bytes: self.verifier.shed_residency_bytes(),
            tracked_residency_bytes: self.authority.tracked_residency_bytes(),
            staged_residency_bytes: self.authority.staged_residency_bytes(),
            pending_residency_bytes: self.verifier.pending_residency_bytes(),
            journal_residency_bytes: self.authority.journal().decoded_residency_bytes(),
        }
    }

    /// The actual durability tier of the underlying medium (the
    /// honest platform boundary — Windows carries the process-crash
    /// tier, never presented as power-loss durability).
    pub fn sync_tier(&self) -> SyncTier {
        self.verifier.sync_tier()
    }

    /// The authority mode from the founding record (never assumed).
    pub fn authority(&self) -> Option<(AuthorityMode, &str)> {
        self.verifier.authority()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consequence::{AwardCheckpoint, Coverage, OccurrenceSpan};
    use crate::envelope::{CounterValues, SignalLane, SourceBaseline};
    use crate::fault::{FaultAction, FaultControl, FaultPoint};
    use crate::local::LocalFileMedium;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let seq = SEQ.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir()
                .join(format!("s7-p3-unit-{}-{label}-{seq}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create test root");
            TestDir(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn store(&self) -> PathBuf {
            self.0.join("store")
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Idempotently founds the campaign store (the founding is the
    /// application's setup act, never a per-commit decision).
    fn ensure_founded(dir: &TestDir) {
        let mut medium = LocalFileMedium::open(dir.store()).unwrap();
        if medium.head().unwrap().is_some() {
            return;
        }
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "pipeline-campaign", 0);
        match medium.append(&founding, &[], None).unwrap() {
            AppendOutcome::Accepted { .. } => {}
            other => panic!("the founding must accept: {other:?}"),
        }
    }

    fn clean_medium(dir: &TestDir) -> LocalFileMedium {
        ensure_founded(dir);
        LocalFileMedium::open(dir.store()).unwrap()
    }

    struct Boundary {
        position: SourcePosition,
        classification: DurabilityClass,
        dependencies: Vec<TransactionId>,
        frontier: Vec<FrontierEntry>,
        plan: Vec<u8>,
    }

    fn boundary(index: u64) -> Boundary {
        Boundary {
            position: SourcePosition {
                head: None,
                tick: 100 + index,
            },
            classification: DurabilityClass::DurableEligible,
            dependencies: Vec::new(),
            frontier: vec![FrontierEntry {
                region: 1,
                episode: index,
            }],
            plan: format!("plan-{index}").into_bytes(),
        }
    }

    fn fresh_pipeline(dir: &TestDir) -> CommitPipeline<LocalFileMedium> {
        let journal = T1Journal::open(dir.path().join("journal.t1")).unwrap();
        CommitPipeline::open(clean_medium(dir), journal).unwrap()
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn capture_reconciles_after_operating_system_sync_error() {
        let dir = TestDir::new("os-sync-error");
        let path = dir.path().join("journal.t1");
        let mut journal = T1Journal::create_bound_generation(
            &path,
            &crate::hash::hash_object(b"founding"),
            &crate::hash::hash_object(b"root"),
        )
        .unwrap();
        // Windows rejects FlushFileBuffers on a read-only handle. Unix rejects
        // fsync on a socket. Both exercise File::sync_all's actual error path.
        #[cfg(windows)]
        let handle = fs::File::open(&path).unwrap();
        #[cfg(unix)]
        let handle = {
            let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
            fs::File::from(std::os::fd::OwnedFd::from(socket))
        };
        assert!(handle.sync_all().is_err());
        let barriers = journal.sync_count();
        journal.use_handle_for_next_sync(handle);
        let mut pipeline = CommitPipeline::open(clean_medium(&dir), journal).unwrap();
        pipeline.stage(staged_envelope(1)).unwrap();
        let b = boundary(1);
        let receipt = pipeline
            .capture(
                b.position,
                b.classification,
                b.dependencies,
                b.frontier,
                b.plan,
            )
            .unwrap();
        assert!(receipt.reconciled);
        assert_eq!(pipeline.journal().sync_count(), barriers + 1);
        assert!(pipeline.journal().is_committed(receipt.journal.id));
        capture_index(&mut pipeline, 2);
    }

    fn capture_index(pipeline: &mut CommitPipeline<LocalFileMedium>, index: u64) -> TransactionId {
        pipeline.stage(staged_envelope(index)).unwrap();
        let boundary = boundary(index);
        let receipt = pipeline
            .capture(
                boundary.position,
                boundary.classification,
                boundary.dependencies,
                boundary.frontier,
                boundary.plan,
            )
            .unwrap();
        assert!(!receipt.reconciled);
        receipt.journal.id
    }

    fn staged_envelope(index: u64) -> ConsequenceEnvelope {
        ConsequenceEnvelope::capture(
            ConsequenceId::new(1, index, 1),
            vec![ClassifiedSpan {
                span: OccurrenceSpan::new(2 * (index - 1), 2 * index).unwrap(),
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
                tick: 100 + index,
            },
        )
        .unwrap()
    }

    // ------------------------------------------------------------ the flow

    #[test]
    fn stage_capture_commit_confirms_end_to_end() {
        let dir = TestDir::new("end-to-end");
        let mut pipeline = fresh_pipeline(&dir);
        for index in 1..=3u64 {
            pipeline.stage(staged_envelope(index)).unwrap();
            assert_eq!(
                pipeline
                    .status(ConsequenceId::new(1, index, 1))
                    .unwrap()
                    .phase,
                KeyPhase::InMemory
            );
        }
        let boundary = boundary(1);
        let receipt = pipeline
            .capture(
                boundary.position,
                boundary.classification,
                boundary.dependencies,
                boundary.frontier,
                boundary.plan,
            )
            .unwrap();
        for index in 1..=3u64 {
            assert_eq!(
                pipeline
                    .status(ConsequenceId::new(1, index, 1))
                    .unwrap()
                    .phase,
                KeyPhase::JournaledT1
            );
            assert_eq!(
                pipeline
                    .status(ConsequenceId::new(1, index, 1))
                    .unwrap()
                    .transaction,
                Some(receipt.journal.id)
            );
        }

        let CommitOutcome::Confirmed(confirmation) = pipeline.commit_canonical().unwrap() else {
            panic!("expected confirmation");
        };
        assert_eq!(confirmation.keys.len(), 3);
        assert_eq!(confirmation.epoch, 1, "founding epoch 0 + one revision");
        assert_eq!(confirmation.authority, AuthorityMode::Standalone);
        assert_eq!(confirmation.label, ConfirmationLabel::T3LocalStandalone);
        assert_eq!(confirmation.label.to_string(), "T3-local (standalone)");
        for index in 1..=3u64 {
            let id = ConsequenceId::new(1, index, 1);
            assert_eq!(
                pipeline.status(id).unwrap().phase,
                KeyPhase::InConfirmedChain
            );
            assert!(pipeline.is_confirmed(id));
            // The per-key coverage evidence — individually tracked,
            // never a batch aggregate alone.
            assert_eq!(
                pipeline.confirmed_coverage(id).unwrap(),
                staged_envelope(index).sub_spans.as_slice()
            );
        }
        // The chain holds founding + one revision; re-committing is a
        // no-op.
        assert_eq!(
            pipeline.commit_canonical().unwrap(),
            CommitOutcome::NothingPending
        );
        let mut check = LocalFileMedium::open(dir.store()).unwrap();
        assert_eq!(check.validate().unwrap().records, 2);
    }

    #[test]
    fn t1_pending_until_genuine_t3() {
        let dir = TestDir::new("t1-pending");
        let mut pipeline = fresh_pipeline(&dir);
        capture_index(&mut pipeline, 1);
        // Journaled (T1) — durable and replayable, but NOT confirmed:
        // the chain holds only the founding revision.
        assert_eq!(
            pipeline.status(ConsequenceId::new(1, 1, 1)).unwrap().phase,
            KeyPhase::JournaledT1
        );
        assert!(!pipeline.is_confirmed(ConsequenceId::new(1, 1, 1)));
        let mut check = LocalFileMedium::open(dir.store()).unwrap();
        assert_eq!(check.validate().unwrap().records, 1);
        // The confirmation flips the phase only through the verified
        // chain inclusion.
        let CommitOutcome::Confirmed(_) = pipeline.commit_canonical().unwrap() else {
            panic!("expected confirmation");
        };
        assert_eq!(
            pipeline.status(ConsequenceId::new(1, 1, 1)).unwrap().phase,
            KeyPhase::InConfirmedChain
        );
    }

    #[test]
    fn confirmation_rejects_mismatched_attempts() {
        let dir = TestDir::new("confirm-invalid");
        let mut pipeline = fresh_pipeline(&dir);
        let transaction = capture_index(&mut pipeline, 1);
        let CommitOutcome::Confirmed(confirmation) = pipeline.commit_canonical().unwrap() else {
            panic!("expected confirmation");
        };
        // The real batch object is content-addressed — rebuilding the
        // same entries reproduces the exact on-chain bytes (and
        // therefore the same hash), so a doctored attempt against it
        // reaches the individual validation rather than a missing
        // object.
        let real = ConsequenceBatch {
            entries: vec![BatchEntry {
                id: ConsequenceId::new(1, 1, 1),
                transaction,
                coverage: staged_envelope(1).sub_spans.clone(),
                effect: staged_envelope(1).effect_commitment().unwrap(),
            }],
        };
        let object = StoredObject::new(real.canonical_bytes().unwrap()).unwrap();
        // (a) The coverage lies about what was attempted — the
        // individual validation must catch it and never report a
        // false T3.
        let mut keys = BTreeMap::new();
        keys.insert(
            ConsequenceId::new(1, 1, 1),
            (
                vec![ClassifiedSpan {
                    span: OccurrenceSpan::new(99, 100).unwrap(),
                    class: DurabilityClass::DurableEligible,
                }],
                transaction,
                staged_envelope(1).effect_commitment().unwrap(),
            ),
        );
        let attempt = PendingAttempt {
            record: RevisionRecord::regular(confirmation.revision, confirmation.epoch, 101),
            batch_hash: object.hash(),
            keys,
            ticks: BTreeMap::new(),
        };
        assert!(matches!(
            pipeline.verifier.confirm(&attempt),
            Err(PipelineError::ConfirmationInvalid {
                detail: "payload-mismatch-in-confirmed-batch"
            })
        ));
        // (b) A key the confirmed batch never carried.
        let mut keys = BTreeMap::new();
        keys.insert(
            ConsequenceId::new(1, 1, 1),
            (
                staged_envelope(1).sub_spans.clone(),
                transaction,
                staged_envelope(1).effect_commitment().unwrap(),
            ),
        );
        keys.insert(
            ConsequenceId::new(1, 5, 1),
            (
                staged_envelope(5).sub_spans.clone(),
                transaction,
                staged_envelope(5).effect_commitment().unwrap(),
            ),
        );
        let batch_hash = object.hash();
        let attempt = PendingAttempt {
            record: RevisionRecord::regular(confirmation.revision, confirmation.epoch, 101),
            batch_hash,
            keys,
            ticks: BTreeMap::new(),
        };
        assert!(matches!(
            pipeline.verifier.confirm(&attempt),
            Err(PipelineError::ConfirmationInvalid {
                detail: "batch-key-set-mismatch"
            })
        ));
    }

    #[test]
    fn unknown_installed_reconciles_to_confirmed() {
        let dir = TestDir::new("unknown-installed");
        // The store is founded through a clean instance; the
        // pipeline's medium carries the fault — the rename happens,
        // the acknowledgment is lost.
        ensure_founded(&dir);
        let journal = T1Journal::open(dir.path().join("journal.t1")).unwrap();
        let medium = LocalFileMedium::open_with(
            dir.store(),
            Some(FaultControl::new().arm(FaultPoint::AfterHeadRename, FaultAction::InjectError)),
        )
        .unwrap();
        let mut pipeline = CommitPipeline::open(medium, journal).unwrap();
        capture_index(&mut pipeline, 1);
        let CommitOutcome::Unknown { revision, .. } = pipeline.commit_canonical().unwrap() else {
            panic!("expected unknown");
        };
        assert_eq!(
            pipeline.status(ConsequenceId::new(1, 1, 1)).unwrap().phase,
            KeyPhase::JournaledT1,
            "an unknown outcome is never a confirmation"
        );
        // A new attempt is refused until the unknown is resolved
        // (identity exactness).
        assert!(matches!(
            pipeline.commit_canonical(),
            Err(PipelineError::AttemptUnresolved { revision: pending }) if pending == revision
        ));
        let ReconcileOutcome::Confirmed(confirmation) = pipeline.reconcile_pending().unwrap()
        else {
            panic!("expected installed confirmation");
        };
        assert_eq!(confirmation.revision, revision);
        assert_eq!(
            pipeline.status(ConsequenceId::new(1, 1, 1)).unwrap().phase,
            KeyPhase::InConfirmedChain
        );
        let mut check = LocalFileMedium::open(dir.store()).unwrap();
        assert_eq!(check.validate().unwrap().records, 2);
    }

    #[test]
    fn unknown_absent_retries_idempotently() {
        let dir = TestDir::new("unknown-absent");
        // Founded through a clean instance; the pipeline's medium
        // fails before the rename — Unknown, not installed.
        ensure_founded(&dir);
        let journal = T1Journal::open(dir.path().join("journal.t1")).unwrap();
        let medium = LocalFileMedium::open_with(
            dir.store(),
            Some(FaultControl::new().arm(FaultPoint::HeadTmpWritten, FaultAction::InjectError)),
        )
        .unwrap();
        let mut pipeline = CommitPipeline::open(medium, journal).unwrap();
        capture_index(&mut pipeline, 1);
        let CommitOutcome::Unknown { .. } = pipeline.commit_canonical().unwrap() else {
            panic!("expected unknown");
        };
        assert_eq!(
            pipeline.reconcile_pending().unwrap(),
            ReconcileOutcome::NotInstalled
        );
        // The retry on the still-faulted medium is ambiguous again —
        // retained, never guessed.
        let CommitOutcome::Unknown { .. } = pipeline.retry_pending().unwrap() else {
            panic!("expected the retry to stay ambiguous on the faulted medium");
        };
        assert_eq!(
            pipeline.reconcile_pending().unwrap(),
            ReconcileOutcome::NotInstalled
        );
        // Recovery through a clean instance: the same content
        // identity (the same batch bytes, hash, and idempotency key)
        // installs exactly once — no duplicate revision, no duplicate
        // effect.
        drop(pipeline);
        let mut recovered = fresh_pipeline(&dir);
        let CommitOutcome::Confirmed(_) = recovered.commit_canonical().unwrap() else {
            panic!("expected confirmation");
        };
        assert_eq!(
            recovered.status(ConsequenceId::new(1, 1, 1)).unwrap().phase,
            KeyPhase::InConfirmedChain
        );
        let mut check = LocalFileMedium::open(dir.store()).unwrap();
        assert_eq!(check.validate().unwrap().records, 2, "one revision only");
    }

    /// (P3-L3) The readback-losing medium (the pending-retention test
    /// seam): after the first ACCEPTED append, the NEXT `read_object`
    /// fails ONCE with the worker-death class — the reply the
    /// transport loses after append admission, surfacing exactly
    /// inside the confirmation procedure's readback walk.
    /// Classification stays provider-side (delegated, never routed
    /// through the failing read) so the failure lands on the batch
    /// readback the confirmation performs.
    struct ReadbackLosingMedium<M: CanonicalMedium> {
        inner: M,
        lose_next_readback: bool,
    }

    impl<M: CanonicalMedium> ReadbackLosingMedium<M> {
        fn over(inner: M) -> Self {
            ReadbackLosingMedium {
                inner,
                lose_next_readback: false,
            }
        }
    }

    impl<M: CanonicalMedium> CanonicalMedium for ReadbackLosingMedium<M> {
        fn head(&mut self) -> Result<Option<Hash>, MediumError> {
            self.inner.head()
        }
        fn append_admitted(
            &mut self,
            admitted: crate::medium::AdmittedAppend,
            expected_parent: Option<Hash>,
        ) -> Result<AppendOutcome, MediumError> {
            let outcome = self.inner.append_admitted(admitted, expected_parent)?;
            if matches!(outcome, AppendOutcome::Accepted { .. }) {
                self.lose_next_readback = true;
            }
            Ok(outcome)
        }
        fn read_record(&mut self, id: &Hash) -> Result<RevisionRecord, MediumError> {
            self.inner.read_record(id)
        }
        fn read_object(&mut self, hash: &Hash) -> Result<Vec<u8>, MediumError> {
            if self.lose_next_readback {
                self.lose_next_readback = false;
                return Err(MediumError::WorkerUnavailable {
                    detail: "readback lost after append admission".to_owned(),
                });
            }
            self.inner.read_object(hash)
        }
        fn classify_object(&mut self, hash: &Hash) -> Result<ObjectClass, MediumError> {
            self.inner.classify_object(hash)
        }
        fn validate(&mut self) -> Result<crate::medium::ChainSummary, MediumError> {
            self.inner.validate()
        }
        fn reconcile_unknown(&mut self, candidate: &Hash) -> Result<Reconciliation, MediumError> {
            self.inner.reconcile_unknown(candidate)
        }
        fn durability_tier(&self) -> SyncTier {
            self.inner.durability_tier()
        }
        fn reclaim(
            &mut self,
            directive: &crate::reclaim::ReclaimDirective,
        ) -> Result<crate::reclaim::ReclaimOutcome, MediumError> {
            self.inner.reclaim(directive)
        }
        fn reclaim_pending(
            &mut self,
            directive: &crate::reclaim::ReclaimDirective,
        ) -> Result<bool, MediumError> {
            self.inner.reclaim_pending(directive)
        }
        fn begin_reclamation(
            &mut self,
            audit: &Hash,
            mk: &Hash,
            tick: u64,
        ) -> Result<crate::reclaim::BeginPublicationOutcome, MediumError> {
            self.inner.begin_reclamation(audit, mk, tick)
        }
        fn verify_begin_authority(
            &mut self,
            audit: &Hash,
            mk: &Hash,
        ) -> Result<crate::reclaim::BeginAuthority, MediumError> {
            self.inner.verify_begin_authority(audit, mk)
        }
        fn begin_reclamation_v2(
            &mut self,
            mk: &Hash,
            tick: u64,
        ) -> Result<crate::reclaim::BeginPublicationOutcome, MediumError> {
            self.inner.begin_reclamation_v2(mk, tick)
        }
        fn verify_begin_authority_v2(
            &mut self,
            mk: &Hash,
        ) -> Result<crate::reclaim::BeginAuthority, MediumError> {
            self.inner.verify_begin_authority_v2(mk)
        }
        fn observe_range_terminal_under_coordination(
            &mut self,
            expected: &crate::range_completion::RangeCompletionContext,
            limits: &crate::range_installation::RangeInstallationLimits,
        ) -> Result<crate::range_installation::CoordinatedObservation, MediumError> {
            self.inner
                .observe_range_terminal_under_coordination(expected, limits)
        }
    }

    /// (P3-L3) The single retained copy survives a failed readback:
    /// the confirmation takes the candidate OUT of the pending slot
    /// for the verification and restores it on every failure — a
    /// worker-death readback after an ACCEPTED append still maps to
    /// `Unknown{WorkerReplyLost}` with the attempt retained (never a
    /// discarded unresolved obligation), the slot still refuses a new
    /// submission, and the retry confirms the installed attempt
    /// through the recovered readback.
    #[test]
    fn readback_failure_retains_the_single_pending_copy() {
        let dir = TestDir::new("readback-retention");
        ensure_founded(&dir);
        let journal = T1Journal::open(dir.path().join("journal.t1")).unwrap();
        let medium = ReadbackLosingMedium::over(LocalFileMedium::open(dir.store()).unwrap());
        let mut pipeline = CommitPipeline::open(medium, journal).unwrap();
        capture_index_on(&mut pipeline, 1);
        // The append is ACCEPTED; the confirmation's readback loses
        // the reply — the exact window the retained copy exists for.
        let revision = match pipeline.commit_canonical().unwrap() {
            CommitOutcome::Unknown {
                revision,
                reason: UnknownReason::WorkerReplyLost,
            } => revision,
            other => panic!("expected the worker-reply-lost Unknown, observed {other:?}"),
        };
        // The failed verification RESTORED the single retained copy —
        // the unresolved obligation is still observable and still
        // refuses a new submission.
        assert_eq!(pipeline.verifier.pending_revision(), Some(revision));
        assert!(matches!(
            pipeline.commit_canonical(),
            Err(PipelineError::AttemptUnresolved { revision: pending }) if pending == revision
        ));
        // The append WAS installed: the retry's walk confirms it
        // through the recovered readback, and the slot clears only on
        // that verified confirmation.
        let CommitOutcome::Confirmed(receipt) = pipeline.retry_pending().unwrap() else {
            panic!("expected the retry to confirm the installed attempt");
        };
        assert_eq!(receipt.keys, vec![ConsequenceId::new(1, 1, 1)]);
        assert_eq!(pipeline.verifier.pending_revision(), None);
        assert!(pipeline.is_confirmed(ConsequenceId::new(1, 1, 1)));
        assert_eq!(
            pipeline.status(ConsequenceId::new(1, 1, 1)).unwrap().phase,
            KeyPhase::InConfirmedChain
        );
        let mut check = LocalFileMedium::open(dir.store()).unwrap();
        assert_eq!(check.validate().unwrap().records, 2, "one revision only");
    }

    #[test]
    fn stale_parent_rebases_preserving_identity() {
        let dir = TestDir::new("stale-rebase");
        // Two pipelines over one store, separate journals. B opens
        // before A commits, so B appends against the founding — the
        // stale-parent path.
        let journal_a = T1Journal::open(dir.path().join("journal-a.t1")).unwrap();
        let mut a = CommitPipeline::open(clean_medium(&dir), journal_a).unwrap();
        let journal_b = T1Journal::open(dir.path().join("journal-b.t1")).unwrap();
        let mut b = CommitPipeline::open(clean_medium(&dir), journal_b).unwrap();

        capture_index(&mut a, 1);
        let CommitOutcome::Confirmed(confirmation_a) = a.commit_canonical().unwrap() else {
            panic!("expected A confirmed");
        };

        // B stages its own identity (a different episode) and commits
        // against its verified head (the founding) — stale.
        pipeline_stage_b(&mut b, 2);
        let CommitOutcome::Confirmed(confirmation_b) = b.commit_canonical().unwrap() else {
            panic!("expected B confirmed through the rebase");
        };
        assert_ne!(confirmation_a.revision, confirmation_b.revision);
        // The per-key identity and coverage survived the rebase
        // unchanged.
        let id = ConsequenceId::new(1, 2, 1);
        assert_eq!(b.status(id).unwrap().phase, KeyPhase::InConfirmedChain);
        assert_eq!(
            b.confirmed_coverage(id).unwrap(),
            staged_envelope(2).sub_spans.as_slice()
        );
        // B's re-walk also collected A's keys (the chain's committed
        // set is global).
        assert!(b.is_confirmed(ConsequenceId::new(1, 1, 1)));
        let mut check = LocalFileMedium::open(dir.store()).unwrap();
        assert_eq!(check.validate().unwrap().records, 3, "founding + A + B");
    }

    fn pipeline_stage_b(pipeline: &mut CommitPipeline<LocalFileMedium>, index: u64) {
        pipeline.stage(staged_envelope(index)).unwrap();
        let boundary = boundary(index);
        pipeline
            .capture(
                boundary.position,
                boundary.classification,
                boundary.dependencies,
                boundary.frontier,
                boundary.plan,
            )
            .unwrap();
    }

    #[test]
    fn foreign_confirmation_never_duplicates() {
        let dir = TestDir::new("foreign-confirmed");
        // A commits key k1; B — with its own journal holding the SAME
        // deterministic identity — commits after A: the re-walk finds
        // k1 already chain-confirmed, so B confirms without
        // re-submitting (no duplicate revision, no duplicate effect).
        let journal_a = T1Journal::open(dir.path().join("journal-a.t1")).unwrap();
        let mut a = CommitPipeline::open(clean_medium(&dir), journal_a).unwrap();
        let journal_b = T1Journal::open(dir.path().join("journal-b.t1")).unwrap();
        let mut b = CommitPipeline::open(clean_medium(&dir), journal_b).unwrap();

        capture_index(&mut a, 1);
        let CommitOutcome::Confirmed(_) = a.commit_canonical().unwrap() else {
            panic!("expected A confirmed");
        };
        pipeline_stage_b(&mut b, 1);
        let CommitOutcome::Confirmed(receipt) = b.commit_canonical().unwrap() else {
            panic!("expected the foreign confirmation");
        };
        assert_eq!(receipt.revision, a_head(&dir), "the confirming chain tip");
        assert_eq!(
            b.status(ConsequenceId::new(1, 1, 1)).unwrap().phase,
            KeyPhase::InConfirmedChain
        );
        let mut check = LocalFileMedium::open(dir.store()).unwrap();
        assert_eq!(
            check.validate().unwrap().records,
            2,
            "no duplicate revision"
        );
    }

    fn a_head(dir: &TestDir) -> Hash {
        let mut medium = LocalFileMedium::open(dir.store()).unwrap();
        medium.head().unwrap().expect("founded")
    }

    #[test]
    fn duplicate_identity_reuse_fails_closed() {
        let dir = TestDir::new("duplicate-reuse");
        let mut pipeline = fresh_pipeline(&dir);
        capture_index(&mut pipeline, 1);
        // A different envelope under the same identity — conflicting
        // reuse.
        let mut conflicting = staged_envelope(2);
        conflicting.id = ConsequenceId::new(1, 1, 1);
        assert!(matches!(
            pipeline.stage(conflicting),
            Err(PipelineError::ConflictingReuse { .. })
        ));
        // A byte-identical restage is a no-op.
        let staged = pipeline.authority.staged.len();
        pipeline.stage(staged_envelope(2)).unwrap();
        assert_eq!(pipeline.authority.staged.len(), staged + 1);
        let before = pipeline.authority.staged.len();
        pipeline.stage(staged_envelope(2)).unwrap();
        assert_eq!(
            pipeline.authority.staged.len(),
            before,
            "the identical restage is a no-op"
        );

        // Two journal transactions claiming one identity (built
        // directly through the journal — the pipeline is dropped
        // first: the journal's flock is held for the instance
        // lifetime) fail the batch collection.
        drop(pipeline);
        let mut raw = T1Journal::open(dir.path().join("journal.t1")).unwrap();
        let boundary = boundary(9);
        let input = TransactionInput {
            envelopes: vec![staged_envelope(1)],
            position: boundary.position,
            classification: boundary.classification,
            dependencies: boundary.dependencies,
            frontier: boundary.frontier,
            plan: b"plan-other".to_vec(),
        };
        raw.append_transaction(&input).unwrap();
        drop(raw);
        let mut pipeline = fresh_pipeline(&dir);
        assert!(matches!(
            pipeline.commit_canonical(),
            Err(PipelineError::ConflictingReuse { .. })
        ));
    }

    #[test]
    fn canceled_transactions_are_excluded_from_the_batch() {
        let dir = TestDir::new("canceled-excluded");
        let mut pipeline = fresh_pipeline(&dir);
        let first = capture_index(&mut pipeline, 1);
        let second = capture_index(&mut pipeline, 2);
        pipeline
            .cancel_transaction(first, CancellationReason::ApplicationPolicy, b"policy")
            .unwrap();
        let CommitOutcome::Confirmed(confirmation) = pipeline.commit_canonical().unwrap() else {
            panic!("expected confirmation");
        };
        assert_eq!(confirmation.keys, vec![ConsequenceId::new(1, 2, 1)]);
        assert_eq!(
            pipeline.status(ConsequenceId::new(1, 1, 1)).unwrap().phase,
            KeyPhase::JournaledT1,
            "the canceled transaction's key stays T1-pending, never submitted"
        );
        assert!(pipeline.is_confirmed(ConsequenceId::new(1, 2, 1)));
        assert!(!pipeline.is_confirmed(ConsequenceId::new(1, 1, 1)));
        let _ = second;
    }

    #[test]
    fn mark_applied_tracks_pending_vs_confirmed() {
        let dir = TestDir::new("applied-pending");
        let mut pipeline = fresh_pipeline(&dir);
        // Applying an unstaged key is refused (durable-before-discard).
        pipeline.stage(staged_envelope(1)).unwrap();
        assert!(matches!(
            pipeline.mark_applied(&[ConsequenceId::new(1, 1, 1)]),
            Err(PipelineError::AppliedBeforeJournal { .. })
        ));
        assert!(matches!(
            pipeline.mark_applied(&[ConsequenceId::new(9, 9, 9)]),
            Err(PipelineError::UnknownKey { .. })
        ));
        let boundary = boundary(1);
        pipeline
            .capture(
                boundary.position,
                boundary.classification,
                boundary.dependencies,
                boundary.frontier,
                boundary.plan,
            )
            .unwrap();
        // The effect exists in live state while the canonical commit
        // is pending — never conflated with durability.
        pipeline
            .mark_applied(&[ConsequenceId::new(1, 1, 1)])
            .unwrap();
        assert_eq!(
            pipeline.status(ConsequenceId::new(1, 1, 1)).unwrap().phase,
            KeyPhase::AppliedToLivePending
        );
        assert!(!pipeline.is_confirmed(ConsequenceId::new(1, 1, 1)));
        let CommitOutcome::Confirmed(_) = pipeline.commit_canonical().unwrap() else {
            panic!("expected confirmation");
        };
        assert_eq!(
            pipeline.status(ConsequenceId::new(1, 1, 1)).unwrap().phase,
            KeyPhase::InConfirmedChain
        );
    }

    #[test]
    fn reopen_derives_statuses_from_chain_and_journal() {
        let dir = TestDir::new("reopen");
        let mut pipeline = fresh_pipeline(&dir);
        capture_index(&mut pipeline, 1);
        capture_index(&mut pipeline, 2);
        let CommitOutcome::Confirmed(_) = pipeline.commit_canonical().unwrap() else {
            panic!("expected confirmation");
        };
        capture_index(&mut pipeline, 3);
        // Staged-but-uncaptured keys are honestly lost across
        // sessions (P0-5).
        pipeline.stage(staged_envelope(4)).unwrap();
        drop(pipeline);

        let reopened = fresh_pipeline(&dir);
        assert_eq!(
            reopened.status(ConsequenceId::new(1, 1, 1)).unwrap().phase,
            KeyPhase::InConfirmedChain
        );
        assert_eq!(
            reopened.status(ConsequenceId::new(1, 2, 1)).unwrap().phase,
            KeyPhase::InConfirmedChain
        );
        assert_eq!(
            reopened.status(ConsequenceId::new(1, 3, 1)).unwrap().phase,
            KeyPhase::JournaledT1
        );
        assert!(reopened.status(ConsequenceId::new(1, 4, 1)).is_none());
        assert!(reopened.is_confirmed(ConsequenceId::new(1, 1, 1)));
    }

    #[test]
    fn medium_failure_leaves_commits_pending() {
        let dir = TestDir::new("medium-failure");
        ensure_founded(&dir);
        let journal = T1Journal::open(dir.path().join("journal.t1")).unwrap();
        let medium = LocalFileMedium::open_with(
            dir.store(),
            Some(FaultControl::new().arm(FaultPoint::BeforeLock, FaultAction::InjectError)),
        )
        .unwrap();
        let mut pipeline = CommitPipeline::open(medium, journal).unwrap();
        capture_index(&mut pipeline, 1);
        // A pre-window medium failure is a clean error — the commit
        // stays pending and the gates hold.
        assert!(matches!(
            pipeline.commit_canonical(),
            Err(PipelineError::Medium(_))
        ));
        assert_eq!(
            pipeline.status(ConsequenceId::new(1, 1, 1)).unwrap().phase,
            KeyPhase::JournaledT1
        );
        // Recovery through a clean medium instance confirms.
        drop(pipeline);
        let mut recovered = fresh_pipeline(&dir);
        let CommitOutcome::Confirmed(_) = recovered.commit_canonical().unwrap() else {
            panic!("expected confirmation");
        };
    }

    #[test]
    fn conflicting_canonical_identities_across_batches_rejected() {
        // The review's finding: two valid canonical batches claiming
        // the SAME ConsequenceId — the exact-provenance standard
        // rejects the second, never overwriting the first revision's
        // evidence. The normal pipeline flow cannot produce the
        // conflict (the stale-path re-walk filters confirmed keys
        // before any re-submission), so the second batch is a
        // hand-crafted valid revision — exactly the defensive scope
        // of the walk's contract: ANY valid chain.
        let dir = TestDir::new("canonical-conflict");
        ensure_founded(&dir);
        let identity = ConsequenceId::new(1, 1, 1);
        // The first canonical batch, through the pipeline.
        let journal = T1Journal::open(dir.path().join("journal-a.t1")).unwrap();
        let mut writer =
            CommitPipeline::open(LocalFileMedium::open(dir.store()).unwrap(), journal).unwrap();
        let transaction = capture_index(&mut writer, 1);
        let CommitOutcome::Confirmed(first) = writer.commit_canonical().unwrap() else {
            panic!("expected the first confirmation");
        };
        assert_eq!(first.keys, vec![identity]);
        let confirming = first.revision;
        let head_epoch = first.epoch;
        drop(writer);
        // The conflicting second batch: a valid revision on top of
        // the confirming one, carrying the same identity under a
        // different coverage (a different content hash — the medium
        // accepts it; only the pipeline's provenance standard
        // rejects the claim).
        let batch = ConsequenceBatch {
            entries: vec![BatchEntry {
                id: identity,
                transaction,
                coverage: vec![ClassifiedSpan {
                    span: OccurrenceSpan::new(2, 4).unwrap(),
                    class: DurabilityClass::DurableEligible,
                }],
                effect: staged_envelope(1).effect_commitment().unwrap(),
            }],
        };
        let batch_bytes = batch.canonical_bytes().unwrap();
        let batch_object = StoredObject::new(batch_bytes).unwrap();
        let batch_hash = batch_object.hash();
        let record = RevisionRecord::regular(confirming, head_epoch, 0)
            .with_idempotency_key(batch_hash.as_bytes().to_vec())
            .with_content_refs(vec![batch_hash]);
        let mut medium = LocalFileMedium::open(dir.store()).unwrap();
        match medium
            .append(
                &record,
                std::slice::from_ref(&batch_object),
                Some(confirming),
            )
            .unwrap()
        {
            AppendOutcome::Accepted { .. } => {}
            other => panic!("the conflicting revision must append: {other:?}"),
        }
        drop(medium);
        // A fresh pipeline over the conflicted chain fails closed at
        // its session-open walk — the second batch's claim on the
        // identity is rejected, the first's evidence preserved.
        let journal = T1Journal::open(dir.path().join("journal-b.t1")).unwrap();
        let error = match CommitPipeline::open(LocalFileMedium::open(dir.store()).unwrap(), journal)
        {
            Ok(_) => panic!("the conflicted chain must fail the open"),
            Err(error) => error,
        };
        assert!(
            matches!(
                &error,
                PipelineError::CanonicalIdentityConflict {
                    id,
                    first_revision,
                    second_revision: _,
                } if *id == identity && *first_revision == confirming
            ),
            "the conflicting canonical identity fails closed: {error}"
        );
    }

    #[test]
    fn foreign_receipt_reports_the_confirming_revision_not_the_head() {
        // The review's finding: the foreign receipt must report each
        // key's ACTUAL confirming revision and epoch — never the
        // current (unrelated, later) HEAD. Writer B confirms the
        // identity at revision R1; an unrelated revision R2 lands on
        // top; writer A's stale-path re-walk then returns the foreign
        // confirmation — which must name R1, not R2.
        let dir = TestDir::new("foreign-receipt");
        ensure_founded(&dir);
        let identity = ConsequenceId::new(1, 1, 1);
        // Writer A journals the identity and does NOT commit.
        let journal_a = T1Journal::open(dir.path().join("journal-a.t1")).unwrap();
        let mut writer_a =
            CommitPipeline::open(LocalFileMedium::open(dir.store()).unwrap(), journal_a).unwrap();
        capture_index(&mut writer_a, 1);
        // Writer B (a separate journal) confirms the identity at R1.
        let journal_b = T1Journal::open(dir.path().join("journal-b.t1")).unwrap();
        let mut writer_b =
            CommitPipeline::open(LocalFileMedium::open(dir.store()).unwrap(), journal_b).unwrap();
        capture_index(&mut writer_b, 1);
        let CommitOutcome::Confirmed(foreign) = writer_b.commit_canonical().unwrap() else {
            panic!("expected writer B's confirmation");
        };
        assert_eq!(foreign.keys, vec![identity]);
        let confirming = foreign.revision;
        let confirming_epoch = foreign.epoch;
        // An unrelated revision lands on top of the confirming one.
        capture_index(&mut writer_b, 2);
        let CommitOutcome::Confirmed(unrelated) = writer_b.commit_canonical().unwrap() else {
            panic!("expected the unrelated confirmation");
        };
        assert_ne!(unrelated.revision, confirming);
        drop(writer_b);
        // Writer A's commit: the parent is stale (the head moved past
        // the confirming revision); the suffix re-walk finds the
        // identity confirmed by R1 — the foreign receipt names R1
        // and its epoch, never the unrelated HEAD.
        let CommitOutcome::Confirmed(receipt) = writer_a.commit_canonical().unwrap() else {
            panic!("expected the foreign confirmation");
        };
        assert_eq!(receipt.keys, vec![identity]);
        assert_eq!(receipt.revision, confirming);
        assert_eq!(receipt.epoch, confirming_epoch);
        assert_ne!(receipt.revision, unrelated.revision);
    }

    #[test]
    fn foreign_confirmation_with_conflicting_coverage_fails_closed() {
        // The re-review's finding: remaining_keys previously skipped
        // an attempted identity merely because it existed in the
        // confirmed map. Competing writers claiming the same identity
        // under DIFFERENT coverage: the foreign confirmation does not
        // satisfy the local attempt — exact-provenance equivalence
        // fails closed, never silently absorbed.
        let dir = TestDir::new("foreign-coverage-conflict");
        ensure_founded(&dir);
        let identity = ConsequenceId::new(1, 1, 1);
        // Writer A journals the identity with coverage (0, 2].
        let journal_a = T1Journal::open(dir.path().join("journal-a.t1")).unwrap();
        let mut writer_a =
            CommitPipeline::open(LocalFileMedium::open(dir.store()).unwrap(), journal_a).unwrap();
        writer_a.stage(staged_envelope(1)).unwrap();
        let boundary_a = boundary(1);
        writer_a
            .capture(
                boundary_a.position,
                boundary_a.classification,
                boundary_a.dependencies,
                boundary_a.frontier,
                boundary_a.plan,
            )
            .unwrap();
        // Writer B (a separate journal) claims the same identity under
        // DIFFERENT coverage (2, 4] and confirms it.
        let journal_b = T1Journal::open(dir.path().join("journal-b.t1")).unwrap();
        let mut writer_b =
            CommitPipeline::open(LocalFileMedium::open(dir.store()).unwrap(), journal_b).unwrap();
        writer_b
            .stage(
                ConsequenceEnvelope::capture(
                    identity,
                    vec![ClassifiedSpan {
                        span: OccurrenceSpan::new(2, 4).unwrap(),
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
                        tick: 110,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        let boundary_b = boundary(1);
        writer_b
            .capture(
                boundary_b.position,
                boundary_b.classification,
                boundary_b.dependencies,
                boundary_b.frontier,
                boundary_b.plan,
            )
            .unwrap();
        let CommitOutcome::Confirmed(_) = writer_b.commit_canonical().unwrap() else {
            panic!("expected writer B's confirmation");
        };
        drop(writer_b);
        // Writer A's commit: the stale-path re-walk finds the identity
        // confirmed under a DIFFERENT coverage — the local attempt
        // conflicts with the canonical entry: fail closed.
        let error = writer_a.commit_canonical().unwrap_err();
        assert!(
            matches!(
                &error,
                PipelineError::ConfirmedProvenanceMismatch { id } if *id == identity
            ),
            "the conflicting coverage fails closed: {error}"
        );
    }

    #[test]
    fn foreign_confirmation_with_conflicting_transaction_fails_closed() {
        // The same identity and coverage, but a DIFFERENT producing
        // transaction (a different plan): the canonical entry is not
        // the local attempt — exact provenance fails closed, never
        // silently absorbed.
        let dir = TestDir::new("foreign-transaction-conflict");
        ensure_founded(&dir);
        let identity = ConsequenceId::new(1, 1, 1);
        // Writer A journals the identity under plan-1.
        let journal_a = T1Journal::open(dir.path().join("journal-a.t1")).unwrap();
        let mut writer_a =
            CommitPipeline::open(LocalFileMedium::open(dir.store()).unwrap(), journal_a).unwrap();
        writer_a.stage(staged_envelope(1)).unwrap();
        let boundary_a = boundary(1);
        writer_a
            .capture(
                boundary_a.position,
                boundary_a.classification,
                boundary_a.dependencies,
                boundary_a.frontier,
                boundary_a.plan,
            )
            .unwrap();
        // Writer B claims the same identity + the same coverage under
        // a different plan (a different producing transaction) and
        // confirms it.
        let journal_b = T1Journal::open(dir.path().join("journal-b.t1")).unwrap();
        let mut writer_b =
            CommitPipeline::open(LocalFileMedium::open(dir.store()).unwrap(), journal_b).unwrap();
        writer_b.stage(staged_envelope(1)).unwrap();
        let boundary_b = boundary(2);
        writer_b
            .capture(
                boundary_b.position,
                boundary_b.classification,
                boundary_b.dependencies,
                boundary_b.frontier,
                boundary_b.plan,
            )
            .unwrap();
        let CommitOutcome::Confirmed(_) = writer_b.commit_canonical().unwrap() else {
            panic!("expected writer B's confirmation");
        };
        drop(writer_b);
        // Writer A's commit: the producing transaction differs — the
        // local attempt conflicts with the canonical entry: fail
        // closed.
        let error = writer_a.commit_canonical().unwrap_err();
        assert!(
            matches!(
                &error,
                PipelineError::ConfirmedProvenanceMismatch { id } if *id == identity
            ),
            "the conflicting transaction fails closed: {error}"
        );
    }

    #[test]
    fn foreign_confirmation_with_matching_ids_but_conflicting_effects_fails_closed() {
        // The third re-review's finding: `TransactionId` + occurrence
        // coverage is insufficient to prove effect equivalence — the
        // same identity, span, classification, and plan (the same
        // content-blind TransactionId inputs) admits two valid
        // uniform lanes (+2 pending vs +2 captured). A foreign
        // confirmation carrying the same transaction and coverage
        // but a DIFFERENT consequential effect does not satisfy the
        // local attempt: exact effect provenance fails closed.
        let dir = TestDir::new("foreign-effect-conflict");
        ensure_founded(&dir);
        let identity = ConsequenceId::new(1, 1, 1);
        // Writer A journals the identity with the pending lane —
        // byte-identical to staged_envelope(1) but for the lane.
        let journal_a = T1Journal::open(dir.path().join("journal-a.t1")).unwrap();
        let mut writer_a =
            CommitPipeline::open(LocalFileMedium::open(dir.store()).unwrap(), journal_a).unwrap();
        writer_a
            .stage(
                ConsequenceEnvelope::capture(
                    identity,
                    vec![ClassifiedSpan {
                        span: OccurrenceSpan::new(0, 2).unwrap(),
                        class: DurabilityClass::DurableEligible,
                    }],
                    SignalLane::Pending,
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
                        tick: 101,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        let boundary_a = boundary(1);
        writer_a
            .capture(
                boundary_a.position,
                boundary_a.classification,
                boundary_a.dependencies,
                boundary_a.frontier,
                boundary_a.plan,
            )
            .unwrap();
        // Writer B claims the same identity, span, class, and plan —
        // the same TransactionId and coverage — under the captured
        // lane, and confirms it.
        let journal_b = T1Journal::open(dir.path().join("journal-b.t1")).unwrap();
        let mut writer_b =
            CommitPipeline::open(LocalFileMedium::open(dir.store()).unwrap(), journal_b).unwrap();
        writer_b.stage(staged_envelope(1)).unwrap();
        let boundary_b = boundary(1);
        writer_b
            .capture(
                boundary_b.position,
                boundary_b.classification,
                boundary_b.dependencies,
                boundary_b.frontier,
                boundary_b.plan,
            )
            .unwrap();
        let CommitOutcome::Confirmed(_) = writer_b.commit_canonical().unwrap() else {
            panic!("expected writer B's confirmation");
        };
        drop(writer_b);
        // Writer A's commit: the stale-path re-walk finds the
        // identity confirmed under the same transaction and coverage
        // but a DIFFERENT effect commitment — the local attempt
        // conflicts with the canonical entry: fail closed.
        let error = writer_a.commit_canonical().unwrap_err();
        assert!(
            matches!(
                &error,
                PipelineError::ConfirmedProvenanceMismatch { id } if *id == identity
            ),
            "the conflicting effect fails closed: {error}"
        );
    }

    #[test]
    fn nothing_pending_and_unfounded() {
        let dir = TestDir::new("nothing-pending");
        let mut pipeline = fresh_pipeline(&dir);
        assert_eq!(
            pipeline.commit_canonical().unwrap(),
            CommitOutcome::NothingPending
        );
        capture_index(&mut pipeline, 1);
        let CommitOutcome::Confirmed(_) = pipeline.commit_canonical().unwrap() else {
            panic!("expected confirmation");
        };
        assert_eq!(
            pipeline.commit_canonical().unwrap(),
            CommitOutcome::NothingPending
        );

        // An unfounded medium never reaches submission.
        let dir2 = TestDir::new("unfounded");
        let journal = T1Journal::open(dir2.path().join("journal.t1")).unwrap();
        let mut unfounded =
            CommitPipeline::open(LocalFileMedium::open(dir2.store()).unwrap(), journal).unwrap();
        capture_index(&mut unfounded, 1);
        assert!(matches!(
            unfounded.commit_canonical(),
            Err(PipelineError::UnfoundedMedium)
        ));
    }

    #[test]
    fn empty_boundary_capture_is_journal_only() {
        let dir = TestDir::new("empty-boundary");
        let mut pipeline = fresh_pipeline(&dir);
        let boundary = boundary(7);
        let receipt = pipeline
            .capture(
                boundary.position,
                boundary.classification,
                boundary.dependencies,
                boundary.frontier,
                boundary.plan,
            )
            .unwrap();
        assert_eq!(receipt.journal.record_count, 0, "a frontier-only boundary");
        assert_eq!(
            pipeline.commit_canonical().unwrap(),
            CommitOutcome::NothingPending
        );
    }

    // ------------------------------------------------------- batch codec

    #[test]
    fn batch_codec_round_trips_and_rejects_corruption() {
        let batch = ConsequenceBatch {
            entries: vec![
                BatchEntry {
                    id: ConsequenceId::new(1, 2, 3),
                    transaction: TransactionId(Hash::from_bytes(&[7u8; 32]).unwrap()),
                    coverage: vec![
                        ClassifiedSpan {
                            span: OccurrenceSpan::new(0, 2).unwrap(),
                            class: DurabilityClass::DurableEligible,
                        },
                        ClassifiedSpan {
                            span: OccurrenceSpan::new(5, 7).unwrap(),
                            class: DurabilityClass::EncounterProvisional,
                        },
                    ],
                    effect: Hash::from_bytes(&[11u8; 32]).unwrap(),
                },
                BatchEntry {
                    id: ConsequenceId::new(1, 2, 4),
                    transaction: TransactionId(Hash::from_bytes(&[9u8; 32]).unwrap()),
                    coverage: Vec::new(),
                    effect: Hash::from_bytes(&[13u8; 32]).unwrap(),
                },
            ],
        };
        let bytes = batch.canonical_bytes().unwrap();
        assert_eq!(ConsequenceBatch::decode(&bytes).unwrap(), batch);
        // The class-homogeneous partition boundary: cross-class
        // adjacent sub-spans are legitimate (a mixed aggregate — the
        // encounter envelope's shape); same-class adjacent spans
        // would have composed.
        let partitioned = ConsequenceBatch {
            entries: vec![BatchEntry {
                id: ConsequenceId::new(1, 2, 3),
                transaction: TransactionId(Hash::from_bytes(&[7u8; 32]).unwrap()),
                coverage: vec![
                    ClassifiedSpan {
                        span: OccurrenceSpan::new(0, 2).unwrap(),
                        class: DurabilityClass::DurableEligible,
                    },
                    ClassifiedSpan {
                        span: OccurrenceSpan::new(2, 4).unwrap(),
                        class: DurabilityClass::EncounterProvisional,
                    },
                ],
                effect: Hash::from_bytes(&[21u8; 32]).unwrap(),
            }],
        };
        let bytes = partitioned.canonical_bytes().unwrap();
        assert_eq!(ConsequenceBatch::decode(&bytes).unwrap(), partitioned);
        let mut composed = partitioned;
        composed.entries[0].coverage[1].class = DurabilityClass::DurableEligible;
        assert!(composed.canonical_bytes().is_err());
        // Truncation at every boundary, trailing bytes, a wrong
        // schema, and descending entries all fail closed.
        for cut in 0..bytes.len() {
            assert!(ConsequenceBatch::decode(&bytes[..cut]).is_err());
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(ConsequenceBatch::decode(&trailing).is_err());
        let mut wrong_schema = bytes.clone();
        wrong_schema[8 + CONSEQUENCE_BATCH_DOMAIN.len()] = 9;
        assert!(ConsequenceBatch::decode(&wrong_schema).is_err());
        let mut descending = batch.clone();
        descending.entries.reverse();
        assert!(descending.canonical_bytes().is_err());
    }

    // ---------------------------------------- the S6-C4 read-path corrective

    /// A counting medium wrapper (the S6-C4 test seam): every
    /// `read_object` and `classify_object` is counted per content
    /// hash, distinguishing the CALLER-VISIBLE byte transfer (the
    /// bytes read_object returns) from the implied internal read
    /// volume (the classified lengths — each classification performs
    /// the full verified read provider-side). A reduction in the
    /// transfer is never misreported as a reduction in the physical
    /// reads.
    struct CountingMedium<M: CanonicalMedium> {
        inner: M,
        read_calls: std::collections::HashMap<Hash, u32>,
        read_bytes: u64,
        classify_calls: std::collections::HashMap<Hash, u32>,
        classify_volume: u64,
    }

    impl<M: CanonicalMedium> CountingMedium<M> {
        fn wrap(inner: M) -> Self {
            CountingMedium {
                inner,
                read_calls: std::collections::HashMap::new(),
                read_bytes: 0,
                classify_calls: std::collections::HashMap::new(),
                classify_volume: 0,
            }
        }
    }

    impl<M: CanonicalMedium> CanonicalMedium for CountingMedium<M> {
        fn head(&mut self) -> Result<Option<Hash>, MediumError> {
            self.inner.head()
        }
        fn append_admitted(
            &mut self,
            admitted: crate::medium::AdmittedAppend,
            expected_parent: Option<Hash>,
        ) -> Result<AppendOutcome, MediumError> {
            self.inner.append_admitted(admitted, expected_parent)
        }
        fn read_record(&mut self, id: &Hash) -> Result<RevisionRecord, MediumError> {
            self.inner.read_record(id)
        }
        fn read_object(&mut self, hash: &Hash) -> Result<Vec<u8>, MediumError> {
            let bytes = self.inner.read_object(hash)?;
            *self.read_calls.entry(*hash).or_insert(0) += 1;
            self.read_bytes += bytes.len() as u64;
            Ok(bytes)
        }
        fn classify_object(&mut self, hash: &Hash) -> Result<ObjectClass, MediumError> {
            let class = self.inner.classify_object(hash)?;
            *self.classify_calls.entry(*hash).or_insert(0) += 1;
            self.classify_volume += class.length;
            Ok(class)
        }
        fn validate(&mut self) -> Result<crate::medium::ChainSummary, MediumError> {
            self.inner.validate()
        }
        fn reconcile_unknown(&mut self, candidate: &Hash) -> Result<Reconciliation, MediumError> {
            self.inner.reconcile_unknown(candidate)
        }
        fn durability_tier(&self) -> SyncTier {
            self.inner.durability_tier()
        }
        fn reclaim(
            &mut self,
            directive: &crate::reclaim::ReclaimDirective,
        ) -> Result<crate::reclaim::ReclaimOutcome, MediumError> {
            self.inner.reclaim(directive)
        }
        fn reclaim_pending(
            &mut self,
            directive: &crate::reclaim::ReclaimDirective,
        ) -> Result<bool, MediumError> {
            self.inner.reclaim_pending(directive)
        }
        fn begin_reclamation(
            &mut self,
            audit: &Hash,
            mk: &Hash,
            tick: u64,
        ) -> Result<crate::reclaim::BeginPublicationOutcome, MediumError> {
            // (OC-C3 CP-1) The counting wrapper's plain delegation
            // (no read-accounting on the begin path).
            self.inner.begin_reclamation(audit, mk, tick)
        }

        fn verify_begin_authority(
            &mut self,
            audit: &Hash,
            mk: &Hash,
        ) -> Result<crate::reclaim::BeginAuthority, MediumError> {
            // (OC-C3 CP-1) The counting wrapper's plain delegation.
            self.inner.verify_begin_authority(audit, mk)
        }

        fn begin_reclamation_v2(
            &mut self,
            mk: &Hash,
            tick: u64,
        ) -> Result<crate::reclaim::BeginPublicationOutcome, MediumError> {
            // (V2X-CP2) The counting wrapper's plain delegation (no
            // read-accounting on the begin path).
            self.inner.begin_reclamation_v2(mk, tick)
        }

        fn verify_begin_authority_v2(
            &mut self,
            mk: &Hash,
        ) -> Result<crate::reclaim::BeginAuthority, MediumError> {
            // (V2X-CP2) The counting wrapper's plain delegation.
            self.inner.verify_begin_authority_v2(mk)
        }

        fn observe_range_terminal_under_coordination(
            &mut self,
            expected: &crate::range_completion::RangeCompletionContext,
            limits: &crate::range_installation::RangeInstallationLimits,
        ) -> Result<crate::range_installation::CoordinatedObservation, MediumError> {
            // (V2X-CP3-B-P2A-C5b) The counting wrapper's plain
            // delegation (read-only; no read-accounting on the
            // observation path).
            self.inner
                .observe_range_terminal_under_coordination(expected, limits)
        }
    }

    /// One publication revision's content object: a well-formed
    /// domain-tagged NON-batch payload (the session-snapshot shape —
    /// a domain string distinct from every batch or
    /// checkpoint-metadata domain, followed by filler).
    fn publication_object(index: u64, size: usize) -> StoredObject {
        let mut enc = Encoder::new();
        enc.write_str(&format!("s6c4-publication-clone-{index}"));
        enc.write_bytes(&vec![index as u8; size]);
        StoredObject::new(enc.into_bytes()).unwrap()
    }

    fn append_publication<M: CanonicalMedium>(
        medium: &mut M,
        parent: Option<Hash>,
        index: u64,
        epoch: u64,
        size: usize,
    ) -> Hash {
        let object = publication_object(index, size);
        let parent = parent.expect("the chain is founded before the cluster");
        let record = RevisionRecord::regular(parent, epoch, 100 + index)
            .with_content_refs(vec![object.hash()]);
        match medium.append(&record, &[object], Some(parent)).unwrap() {
            AppendOutcome::Accepted { id, .. } => id,
            other => panic!("the publication must accept: {other:?}"),
        }
    }

    /// A publication cluster over a founded medium: `count` snapshot
    /// revisions appended to the chain (the deferred-walk shape the
    /// hardware finding reproduced), each with a well-formed
    /// domain-tagged non-batch content object of `size` bytes.
    fn append_publications<M: CanonicalMedium>(medium: &mut M, count: u64, size: usize) {
        let mut parent = medium.head().unwrap();
        for index in 1..=count {
            // The parent's epoch: the founding is epoch 0, each
            // publication's own epoch is parent + 1.
            parent = Some(append_publication(medium, parent, index, index - 1, size));
        }
    }

    /// The staged capture + boundary fixture over ANY medium (the
    /// `capture_index` helper's medium-generic form).
    fn capture_index_on<M: CanonicalMedium>(
        pipeline: &mut CommitPipeline<M>,
        index: u64,
    ) -> TransactionId {
        pipeline.stage(staged_envelope(index)).unwrap();
        let boundary = boundary(index);
        let receipt = pipeline
            .capture(
                boundary.position,
                boundary.classification,
                boundary.dependencies,
                boundary.frontier,
                boundary.plan,
            )
            .unwrap();
        assert!(!receipt.reconciled);
        receipt.journal.id
    }

    /// The stored file path of a content object in the local medium
    /// (the `objects/<hex>` layout).
    fn object_file(dir: &TestDir, object: &StoredObject) -> std::path::PathBuf {
        dir.store().join("objects").join(object.hash().hex())
    }

    /// The S6-C4 corrective's read-amplification gate: after a
    /// publication cluster, the confirmation's chain walk verifies
    /// every referenced object (the classification counts) but
    /// transfers only the CONSUMED content (the batch) — the
    /// caller-visible read volume excludes the snapshot bodies, and
    /// no object is classified more than once within the walk.
    #[test]
    fn verified_classification_verifies_each_object_once_per_walk() {
        let dir = TestDir::new("s6c4-amplification");
        let mut medium = CountingMedium::wrap(clean_medium(&dir));
        // The publication cluster: three large snapshot revisions on
        // the chain (the deferred-walk shape the hardware finding
        // reproduced).
        append_publications(&mut medium, 3, 24 * 1024);
        // The pending consequence + the confirmation.
        let journal = T1Journal::open(dir.path().join("journal.t1")).unwrap();
        let mut pipeline = CommitPipeline::open(medium, journal).unwrap();
        capture_index_on(&mut pipeline, 1);
        let outcome = pipeline.commit_canonical().unwrap();
        let CommitOutcome::Confirmed(receipt) = outcome else {
            panic!("the commit must confirm: {outcome:?}")
        };
        assert_eq!(receipt.keys.len(), 1);
        // Every referenced object was verified EXACTLY once per
        // examination walk (the cold open walk and the commit walk
        // each classify each object they examine at most once).
        assert!(
            pipeline
                .verifier
                .medium
                .classify_calls
                .values()
                .all(|count| *count == 1),
            "no object classified more than once per walk: {:?}",
            pipeline.verifier.medium.classify_calls
        );
        // The classification covered the three snapshots (the
        // internal verified reads) — the implied internal volume is
        // honest, NOT zero.
        assert!(pipeline.verifier.medium.classify_volume >= 3 * 24 * 1024);
        // The caller-visible transfer excludes the snapshot bodies:
        // only the consequence batch (small) was read back in full.
        assert!(
            pipeline.verifier.medium.read_bytes < 8 * 1024,
            "the snapshot bodies must not transfer to the caller: {} bytes",
            pipeline.verifier.medium.read_bytes
        );
        assert_eq!(pipeline.verifier.medium.read_calls.len(), 1);
    }

    /// The deferred-walk shape done properly (the hardware finding's
    /// exact structure): the pipeline opens over the founded chain,
    /// THEN a concurrent writer appends publication revisions, and
    /// the next confirmation's incremental walk examines them.
    fn open_then_publish(
        dir: &TestDir,
        publications: u64,
        size: usize,
    ) -> CommitPipeline<LocalFileMedium> {
        let journal = T1Journal::open(dir.path().join("journal.t1")).unwrap();
        let pipeline = CommitPipeline::open(clean_medium(dir), journal).unwrap();
        let mut writer = clean_medium(dir);
        append_publications(&mut writer, publications, size);
        pipeline
    }

    /// The strict integrity guarantee: a corrupted snapshot object —
    /// corrupted BEYOND any domain prefix, mid-body — is detected AT
    /// the walk that examines it (the classification's full-object
    /// hash check, never a prefix-only read).
    #[test]
    fn corrupted_snapshot_detected_at_the_first_examination() {
        let dir = TestDir::new("s6c4-corrupt-first");
        let mut pipeline = open_then_publish(&dir, 2, 16 * 1024);
        // Corrupt the newest snapshot's stored bytes on disk,
        // mid-body (the domain prefix stays intact).
        let target = publication_object(2, 16 * 1024);
        let path = object_file(&dir, &target);
        let mut bytes = fs::read(&path).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] = bytes[mid].wrapping_add(1);
        fs::write(&path, bytes).unwrap();
        capture_index_on(&mut pipeline, 1);
        let error = pipeline.commit_canonical().unwrap_err();
        assert!(
            error.to_string().contains("hash") || error.to_string().contains("Hash"),
            "the corrupted object is detected at the walk: {error}"
        );
    }

    /// The corruption-between-walks guarantee: an object referenced
    /// by a revision that a LATER walk examines for the first time,
    /// corrupted after the earlier walk, is detected by that later
    /// walk (the memo is walk-local — every walk re-verifies what it
    /// examines; no session-wide cache exists).
    #[test]
    fn corruption_between_walks_detected_by_the_second_walk() {
        let dir = TestDir::new("s6c4-corrupt-second");
        let mut pipeline = open_then_publish(&dir, 1, 16 * 1024);
        capture_index_on(&mut pipeline, 1);
        let outcome = pipeline.commit_canonical().unwrap();
        assert!(matches!(outcome, CommitOutcome::Confirmed { .. }));
        // A second publication appended after the first walk — its
        // object corrupted before the second walk examines it. (The
        // parent is the first confirmation: epoch 2 of the chain.)
        let mut writer = clean_medium(&dir);
        let head = writer.head().unwrap();
        append_publication(&mut writer, head, 2, 2, 16 * 1024);
        let target = publication_object(2, 16 * 1024);
        let path = object_file(&dir, &target);
        let mut bytes = fs::read(&path).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] = bytes[mid].wrapping_add(1);
        fs::write(&path, bytes).unwrap();
        capture_index_on(&mut pipeline, 2);
        let error = pipeline.commit_canonical().unwrap_err();
        assert!(
            error.to_string().contains("hash") || error.to_string().contains("Hash"),
            "the second walk detects the intervening corruption: {error}"
        );
    }

    /// A missing referenced object fails the walk closed (the
    /// classification's existence check — the existing
    /// `MissingObject`, through the new path).
    #[test]
    fn missing_referenced_object_fails_the_walk() {
        let dir = TestDir::new("s6c4-missing");
        let mut pipeline = open_then_publish(&dir, 2, 16 * 1024);
        let target = publication_object(1, 16 * 1024);
        fs::remove_file(object_file(&dir, &target)).unwrap();
        capture_index_on(&mut pipeline, 1);
        let error = pipeline.commit_canonical().unwrap_err();
        assert!(
            error.to_string().contains("missing") || error.to_string().contains("Missing"),
            "the missing object fails the walk: {error}"
        );
    }

    // ------------------------------------------------ the 1-A split composition
    //
    // The five focused proofs that the authority/verifier composition
    // produces the same decisions and outputs as the accepted
    // single-struct pipeline — each drives the HALVES explicitly
    // (the future actor's exact protocol: derive the immutable
    // request on the authority, process it in the verifier, promote
    // from the verifier's post-operation truth), then checks the
    // facade over the same state agrees.

    /// A captured transaction subsequently canceled before any
    /// canonical submission is never collected, never submitted, and
    /// stays T1-pending — through the explicit composition and the
    /// facade alike.
    #[test]
    fn split_composition_captured_then_canceled_never_submitted() {
        let dir = TestDir::new("split-canceled");
        let journal = T1Journal::open(dir.path().join("journal.t1")).unwrap();
        let mut authority = CommitAuthority::open(journal);
        let verifier = CommitVerifier::open(clean_medium(&dir)).unwrap();
        authority.promote_from(&verifier);
        authority.stage(staged_envelope(1)).unwrap();
        let boundary = boundary(1);
        let receipt = authority
            .capture(
                boundary.position,
                boundary.classification,
                boundary.dependencies,
                boundary.frontier,
                boundary.plan,
            )
            .unwrap();
        authority
            .cancel_transaction(
                receipt.journal.id,
                CancellationReason::ApplicationPolicy,
                b"policy",
            )
            .unwrap();
        // The authority's actionable derivation excludes the canceled
        // transaction: nothing to submit.
        let keys = authority.collect_actionable(&verifier).unwrap();
        assert!(keys.is_empty(), "the canceled capture is not actionable");
        // The facade over the SAME halves agrees — and the canceled
        // key's phase never advances past T1.
        let mut pipeline = CommitPipeline {
            authority,
            verifier,
        };
        assert!(matches!(
            pipeline.commit_canonical().unwrap(),
            CommitOutcome::NothingPending
        ));
        assert_eq!(
            pipeline.status(ConsequenceId::new(1, 1, 1)).unwrap().phase,
            KeyPhase::JournaledT1,
            "the canceled transaction's key stays T1-pending, never submitted"
        );
    }

    /// Multiple actionable transactions compose into one batch
    /// through the split; two journal transactions claiming ONE
    /// identity fail the collection closed — the halves and the
    /// facade produce the same decisions.
    #[test]
    fn split_composition_overlapping_and_conflicting_transactions() {
        let dir = TestDir::new("split-overlap");
        let mut pipeline = fresh_pipeline(&dir);
        // Two actionable transactions (adjacent coverages, distinct
        // identities): the explicit derivation collects BOTH and
        // freezes BOTH producing transactions' ticks into the
        // request.
        capture_index(&mut pipeline, 1);
        capture_index(&mut pipeline, 2);
        let keys = pipeline
            .authority
            .collect_actionable(&pipeline.verifier)
            .unwrap();
        assert_eq!(keys.len(), 2);
        let ticks = pipeline.authority.transaction_ticks(&keys);
        assert_eq!(
            ticks.len(),
            2,
            "each producing transaction's tick is frozen"
        );
        let CommitOutcome::Confirmed(receipt) = pipeline.commit_canonical().unwrap() else {
            panic!("expected confirmation");
        };
        assert_eq!(
            receipt.keys,
            vec![ConsequenceId::new(1, 1, 1), ConsequenceId::new(1, 2, 1)]
        );
        assert!(pipeline.is_confirmed(ConsequenceId::new(1, 1, 1)));
        assert!(pipeline.is_confirmed(ConsequenceId::new(1, 2, 1)));

        // Conflicting identities: a second journal transaction
        // claiming an already-captured (unconfirmed) identity — built
        // directly through the journal (the pipeline is dropped
        // first: the journal's flock is held for the instance
        // lifetime). Both the authority's derivation and the facade
        // fail closed.
        capture_index(&mut pipeline, 3);
        drop(pipeline);
        let mut raw = T1Journal::open(dir.path().join("journal.t1")).unwrap();
        let boundary = boundary(9);
        let input = TransactionInput {
            envelopes: vec![staged_envelope(3)],
            position: boundary.position,
            classification: boundary.classification,
            dependencies: boundary.dependencies,
            frontier: boundary.frontier,
            plan: b"plan-other".to_vec(),
        };
        raw.append_transaction(&input).unwrap();
        drop(raw);
        let mut pipeline = fresh_pipeline(&dir);
        assert!(matches!(
            pipeline.authority.collect_actionable(&pipeline.verifier),
            Err(PipelineError::ConflictingReuse { .. })
        ));
        assert!(matches!(
            pipeline.commit_canonical(),
            Err(PipelineError::ConflictingReuse { .. })
        ));
    }

    /// A retained unknown attempt survives the failed verification
    /// (the lost acknowledgment) in the VERIFIER half — visible
    /// through the halves, refusing new submissions — and the retry
    /// re-derives the record from the FROZEN request ticks (the
    /// verifier is journal-blind; the retained snapshot must carry
    /// everything).
    #[test]
    fn split_composition_unknown_attempt_survives_and_retries_from_frozen_ticks() {
        let dir = TestDir::new("split-unknown-retry");
        // Founded through a clean instance; the pipeline's medium
        // fails BEFORE the rename — Unknown, nothing installed.
        ensure_founded(&dir);
        let journal = T1Journal::open(dir.path().join("journal.t1")).unwrap();
        let medium = LocalFileMedium::open_with(
            dir.store(),
            Some(FaultControl::new().arm(FaultPoint::HeadTmpWritten, FaultAction::InjectError)),
        )
        .unwrap();
        let mut pipeline = CommitPipeline::open(medium, journal).unwrap();
        capture_index(&mut pipeline, 1);
        let revision = match pipeline.commit_canonical().unwrap() {
            CommitOutcome::Unknown { revision, .. } => revision,
            other => panic!("expected Unknown, observed {other:?}"),
        };
        // The attempt is RETAINED in the verifier half — never
        // dropped by the failed verification.
        assert_eq!(pipeline.verifier.pending_revision(), Some(revision));
        // A new submission is refused while the attempt is
        // unresolved (identity exactness) — through the facade AND
        // the verifier's own submit.
        assert!(matches!(
            pipeline.commit_canonical(),
            Err(PipelineError::AttemptUnresolved { revision: pending }) if pending == revision
        ));
        assert!(matches!(
            pipeline.verifier.submit(CommitRequest {
                keys: BTreeMap::new(),
                ticks: BTreeMap::new(),
            }),
            Err(PipelineError::AttemptUnresolved { revision: pending }) if pending == revision
        ));
        // The verified absence, then the retry FROM THE RETAINED
        // ATTEMPT: on the still-faulted medium the retry stays
        // ambiguous — and the re-derived record is IDENTITY-STABLE
        // (the same parent, epoch, and FROZEN request ticks re-derive
        // the identical record bytes — the verifier is journal-blind;
        // the retained snapshot carries everything).
        assert_eq!(
            pipeline.reconcile_pending().unwrap(),
            ReconcileOutcome::NotInstalled
        );
        match pipeline.retry_pending().unwrap() {
            CommitOutcome::Unknown {
                revision: retried, ..
            } => {
                assert_eq!(
                    retried, revision,
                    "the retry re-derives the identical record from the frozen ticks"
                );
            }
            other => {
                panic!("expected the retry to stay ambiguous on the faulted medium: {other:?}")
            }
        }
        // The attempt SURVIVED the second failed verification too —
        // never dropped, never confirmed.
        assert_eq!(pipeline.verifier.pending_revision(), Some(revision));
        assert_eq!(
            pipeline.reconcile_pending().unwrap(),
            ReconcileOutcome::NotInstalled
        );
        // Recovery through a clean instance (the crash-recovery
        // shape): the same content identity installs exactly once —
        // the same journal evidence, one revision.
        drop(pipeline);
        let mut recovered = fresh_pipeline(&dir);
        let CommitOutcome::Confirmed(confirmation) = recovered.commit_canonical().unwrap() else {
            panic!("expected confirmation");
        };
        assert_eq!(confirmation.keys, vec![ConsequenceId::new(1, 1, 1)]);
        assert_eq!(
            recovered.status(ConsequenceId::new(1, 1, 1)).unwrap().phase,
            KeyPhase::InConfirmedChain
        );
        let mut check = LocalFileMedium::open(dir.store()).unwrap();
        assert_eq!(check.validate().unwrap().records, 2, "one revision only");
    }

    /// No confirmation exists before the full individual-key
    /// verification: a lying attempt (a coverage the confirmed batch
    /// never carried) fails the per-key validation through the
    /// verifier half, and the authority's tracked phase never
    /// advances — never a false T3.
    #[test]
    fn split_composition_no_receipt_before_individual_key_verification() {
        let dir = TestDir::new("split-no-false-receipt");
        let mut pipeline = fresh_pipeline(&dir);
        let transaction = capture_index(&mut pipeline, 1);
        let CommitOutcome::Confirmed(confirmation) = pipeline.commit_canonical().unwrap() else {
            panic!("expected confirmation");
        };
        // A second captured key stays pending while the lying attempt
        // is rejected — the honest key is never conflated.
        let pending = capture_index(&mut pipeline, 2);
        let _ = pending;
        // The lying attempt: the identity the confirmed batch DID
        // carry, but with a coverage it never did.
        let real = ConsequenceBatch {
            entries: vec![BatchEntry {
                id: ConsequenceId::new(1, 1, 1),
                transaction,
                coverage: vec![ClassifiedSpan {
                    span: OccurrenceSpan::new(0, 2).unwrap(),
                    class: DurabilityClass::DurableEligible,
                }],
                effect: staged_envelope(1).effect_commitment().unwrap(),
            }],
        };
        let object = StoredObject::new(real.canonical_bytes().unwrap()).unwrap();
        let mut keys = BTreeMap::new();
        keys.insert(
            ConsequenceId::new(1, 1, 1),
            (
                vec![ClassifiedSpan {
                    span: OccurrenceSpan::new(99, 100).unwrap(),
                    class: DurabilityClass::DurableEligible,
                }],
                transaction,
                staged_envelope(1).effect_commitment().unwrap(),
            ),
        );
        let attempt = PendingAttempt {
            record: RevisionRecord::regular(confirmation.revision, confirmation.epoch, 101),
            batch_hash: object.hash(),
            keys,
            ticks: BTreeMap::new(),
        };
        // The individual validation fails closed through the verifier
        // half — no Confirmed receipt, no promotion on the authority
        // side.
        assert!(matches!(
            pipeline.verifier.confirm(&attempt),
            Err(PipelineError::ConfirmationInvalid {
                detail: "payload-mismatch-in-confirmed-batch"
            })
        ));
        assert_eq!(
            pipeline.status(ConsequenceId::new(1, 2, 1)).unwrap().phase,
            KeyPhase::JournaledT1,
            "the honestly captured key is never promoted by a rejected attempt"
        );
        assert!(!pipeline.is_confirmed(ConsequenceId::new(1, 2, 1)));
    }

    /// The explicit composition protocol end-to-end: the authority
    /// derives the immutable request, the verifier processes it to a
    /// verified Confirmed, and the authority's promote_from advances
    /// every tracked key to InConfirmedChain at exactly the
    /// confirming revision — the facade's semantics, driven by hand.
    #[test]
    fn split_composition_promotes_confirmed_keys_after_verification() {
        let dir = TestDir::new("split-promotion");
        let journal = T1Journal::open(dir.path().join("journal.t1")).unwrap();
        let mut authority = CommitAuthority::open(journal);
        let mut verifier = CommitVerifier::open(clean_medium(&dir)).unwrap();
        authority.promote_from(&verifier);
        // Two captured keys, one reported applied to live state (the
        // pending-vs-confirmed distinction survives the split).
        authority.stage(staged_envelope(1)).unwrap();
        let first = boundary(1);
        authority
            .capture(
                first.position,
                first.classification,
                first.dependencies,
                first.frontier,
                first.plan,
            )
            .unwrap();
        authority.stage(staged_envelope(2)).unwrap();
        let second = boundary(2);
        authority
            .capture(
                second.position,
                second.classification,
                second.dependencies,
                second.frontier,
                second.plan,
            )
            .unwrap();
        authority
            .mark_applied(&[ConsequenceId::new(1, 1, 1)])
            .unwrap();
        assert_eq!(
            authority.status(ConsequenceId::new(1, 1, 1)).unwrap().phase,
            KeyPhase::AppliedToLivePending
        );
        // The composition: derive → submit → promote.
        let keys = authority.collect_actionable(&verifier).unwrap();
        assert_eq!(keys.len(), 2);
        let ticks = authority.transaction_ticks(&keys);
        let CommitOutcome::Confirmed(receipt) =
            verifier.submit(CommitRequest { keys, ticks }).unwrap()
        else {
            panic!("expected confirmation");
        };
        assert_eq!(
            receipt.keys,
            vec![ConsequenceId::new(1, 1, 1), ConsequenceId::new(1, 2, 1)]
        );
        assert_eq!(
            verifier.completed_walks(),
            2,
            "the open walk plus the confirmation walk"
        );
        authority.promote_from(&verifier);
        // Both keys promoted at exactly the confirming revision.
        for id in [ConsequenceId::new(1, 1, 1), ConsequenceId::new(1, 2, 1)] {
            let status = authority.status(id).unwrap();
            assert_eq!(status.phase, KeyPhase::InConfirmedChain);
            assert_eq!(status.revision, Some(receipt.revision));
            assert!(verifier.is_confirmed(id));
        }
        // A SECOND promotion pass is idempotent (the facade runs it
        // after every operation).
        authority.promote_from(&verifier);
        assert_eq!(
            authority.status(ConsequenceId::new(1, 1, 1)).unwrap().phase,
            KeyPhase::InConfirmedChain
        );
    }

    // (F-14) The construction-boundary regression at the commit
    // pipeline's canonical append: the non-enforcing provider model
    // (a crafted well-formed head record at epoch u64::MAX — see
    // crate::medium::epoch_audit_media) sets the pipeline's
    // walk-derived verified epoch to the saturation, and the commit
    // refuses TYPED at the construction boundary — never the
    // constructor's debug panic or the release wrap — with nothing
    // published to the real store beneath.
    #[test]
    fn the_commit_refuses_a_saturated_verified_epoch_typed() {
        let dir = TestDir::new("f14-pipeline");
        ensure_founded(&dir);
        let medium = LocalFileMedium::open(dir.store()).unwrap();
        let wrapped = crate::medium::epoch_audit_media::MaxEpochHeadMedium::over(medium);
        let journal = T1Journal::open(dir.path().join("journal-f14.t1")).unwrap();
        let mut pipeline = CommitPipeline::open(wrapped, journal).unwrap();
        capture_index_on(&mut pipeline, 1);
        let error = pipeline.commit_canonical().unwrap_err();
        assert!(
            matches!(
                error,
                PipelineError::Medium(MediumError::Chain {
                    rule: "epoch-regression"
                })
            ),
            "the saturated verified epoch refuses the commit typed: {error:?}"
        );
        // Nothing was published: the real store beneath the wrapper
        // still holds only the founding.
        let mut check = LocalFileMedium::open(dir.store()).unwrap();
        assert_eq!(check.validate().unwrap().records, 1, "the founding only");
    }

    // ------------------------------------------ P3-L6: the retention lane

    /// (P3-L6) The multi-envelope capture fixture: one staged
    /// transaction carrying `count` envelopes — distinct identities
    /// (the npc field varies), per-identity occurrence coverage, all
    /// under the one capture boundary. The shed fixture's settlement
    /// granularity: one transaction's confirmation settles `count`
    /// identities at once (a transaction may carry up to
    /// `MAX_JOURNAL_ENVELOPES` envelopes).
    fn staged_envelopes(index: u64, count: u64) -> Vec<ConsequenceEnvelope> {
        (1..=count)
            .map(|npc| {
                ConsequenceEnvelope::capture(
                    ConsequenceId::new(1, index, npc as u32),
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
                        tick: 100 + index,
                    },
                )
                .unwrap()
            })
            .collect()
    }

    /// The staged fixture's expected per-identity coverage (the
    /// span-exact form the re-derivation must reproduce).
    fn expected_span(npc: u64) -> Vec<ClassifiedSpan> {
        vec![ClassifiedSpan {
            span: OccurrenceSpan::new(2 * (npc - 1), 2 * npc).unwrap(),
            class: DurabilityClass::DurableEligible,
        }]
    }

    /// The multi-envelope capture: stages `count` envelopes under one
    /// boundary and journals them as one transaction.
    fn capture_many(
        pipeline: &mut CommitPipeline<LocalFileMedium>,
        index: u64,
        count: u64,
    ) -> TransactionId {
        for envelope in staged_envelopes(index, count) {
            pipeline.stage(envelope).unwrap();
        }
        let boundary = boundary(index);
        let receipt = pipeline
            .capture(
                boundary.position,
                boundary.classification,
                boundary.dependencies,
                boundary.frontier,
                boundary.plan,
            )
            .unwrap();
        receipt.journal.id
    }

    /// (P3-L6, test 1) THE WATERMARK'S OBSERVATIONAL SCALING: as the
    /// settled history grows, the per-commit actionable-scan cost
    /// stays FLAT — proportional to the unsettled window (the new
    /// transaction's own envelope plus the one settlement proof for
    /// the transaction the previous commit confirmed), never the
    /// total history behind the mark. The pre-watermark scan examined
    /// EVERY journal envelope on EVERY commit (iteration i paid i —
    /// the quadratic total the watermark removes). The foreign
    /// settlement is asserted at the same scale: another writer's
    /// byte-identical capture confirms this pipeline's next
    /// transaction, and the scan after it settles the LOCAL
    /// transaction from the foreign evidence — the existing
    /// foreign-receipt semantics, now also the watermark's advance.
    #[test]
    fn settled_watermark_keeps_the_per_commit_scan_flat_as_history_grows() {
        let dir = TestDir::new("l6-watermark-flat");
        let mut pipeline = fresh_pipeline(&dir);
        const HISTORY: u64 = 120;
        let mut max_scan_delta = 0u64;
        for index in 1..=HISTORY {
            capture_index(&mut pipeline, index);
            let before = pipeline.retention_stats().actionable_scan_envelopes;
            let CommitOutcome::Confirmed(receipt) = pipeline.commit_canonical().unwrap() else {
                panic!("iteration {index} must confirm");
            };
            assert_eq!(receipt.keys, vec![ConsequenceId::new(1, index, 1)]);
            let delta = pipeline.retention_stats().actionable_scan_envelopes - before;
            max_scan_delta = max_scan_delta.max(delta);
        }
        assert!(
            max_scan_delta <= 2,
            "the per-commit scan cost must stay flat against the settled \
             history's growth: {max_scan_delta}"
        );
        // The trailing confirmation settles at the next scan (the
        // settlement proof runs in the collect that FOLLOWS the
        // confirming commit): one more commit — nothing pending —
        // advances the mark over the full history.
        assert_eq!(
            pipeline.commit_canonical().unwrap(),
            CommitOutcome::NothingPending
        );
        let stats = pipeline.retention_stats();
        // LINEAR cumulative work, not quadratic: the whole history's
        // scan work is a small multiple of the transaction count
        // (the pre-watermark total was HISTORY*(HISTORY+1)/2 = 7,260).
        assert!(
            stats.actionable_scan_envelopes <= 4 * HISTORY,
            "the cumulative scan work must be linear in the history: {}",
            stats.actionable_scan_envelopes
        );
        assert_eq!(stats.journal_transactions, HISTORY);
        assert_eq!(stats.settled_watermark, HISTORY);
        assert_eq!(stats.settled_transactions(), HISTORY);
        assert_eq!(stats.unsettled_transactions, 0);
        assert_eq!(stats.tracked_keys, HISTORY, "the dedup membership");
        assert!(!stats.exceeds_rotation_cadence);

        // THE FOREIGN SETTLEMENT: writer B (its own journal over the
        // same store) captures the SAME deterministic identity
        // byte-identically (the same content-derived TransactionId)
        // and confirms it; this pipeline's commit then confirms
        // through the foreign receipt — no new revision of its own —
        // and the scan after it settles the local transaction.
        capture_index(&mut pipeline, HISTORY + 1);
        let journal_b = T1Journal::open(dir.path().join("journal-b.t1")).unwrap();
        let mut writer_b = CommitPipeline::open(clean_medium(&dir), journal_b).unwrap();
        pipeline_stage_b(&mut writer_b, HISTORY + 1);
        let CommitOutcome::Confirmed(foreign) = writer_b.commit_canonical().unwrap() else {
            panic!("writer B must confirm");
        };
        assert_eq!(foreign.keys, vec![ConsequenceId::new(1, HISTORY + 1, 1)]);
        drop(writer_b);
        let before_records = {
            let mut check = LocalFileMedium::open(dir.store()).unwrap();
            check.validate().unwrap().records
        };
        let CommitOutcome::Confirmed(receipt) = pipeline.commit_canonical().unwrap() else {
            panic!("the foreign confirmation must satisfy the local attempt");
        };
        assert_eq!(receipt.revision, foreign.revision);
        assert_eq!(receipt.keys, vec![ConsequenceId::new(1, HISTORY + 1, 1)]);
        let mut check = LocalFileMedium::open(dir.store()).unwrap();
        assert_eq!(
            check.validate().unwrap().records,
            before_records,
            "the foreign confirmation installs no new revision"
        );
        assert_eq!(
            pipeline.commit_canonical().unwrap(),
            CommitOutcome::NothingPending
        );
        let stats = pipeline.retention_stats();
        assert_eq!(
            stats.settled_watermark,
            HISTORY + 1,
            "the foreign confirmation settles the local transaction"
        );
        assert_eq!(stats.unsettled_transactions, 0);
    }

    /// (P3-L6, test 2) THE FAIL-CLOSED HOLD: an unconfirmed
    /// transaction holds the watermark — the mark cannot advance past
    /// it, its key is re-collected on EVERY commit attempt (never
    /// silently dropped), and the per-commit scan pays only the
    /// unsettled window (the settled history behind the mark is never
    /// revisited — the pre-watermark scan re-examined all 41
    /// envelopes on every attempt). The hold is never a deadlock:
    /// through a clean medium the held transaction confirms and the
    /// mark advances.
    #[test]
    fn unconfirmed_transaction_holds_the_watermark_fail_closed() {
        let dir = TestDir::new("l6-watermark-held");
        // The settled history on a clean medium first; the fault is
        // armed on the held pipeline's own instance (the
        // medium-failure pattern: the fault hits the append, never
        // the journal-side collect).
        let mut pipeline = fresh_pipeline(&dir);
        const HISTORY: u64 = 40;
        for index in 1..=HISTORY {
            capture_index(&mut pipeline, index);
            let CommitOutcome::Confirmed(_) = pipeline.commit_canonical().unwrap() else {
                panic!("iteration {index} must confirm");
            };
        }
        drop(pipeline);
        let journal = T1Journal::open(dir.path().join("journal.t1")).unwrap();
        let medium = LocalFileMedium::open_with(
            dir.store(),
            Some(FaultControl::new().arm(FaultPoint::BeforeLock, FaultAction::InjectError)),
        )
        .unwrap();
        let mut held = CommitPipeline::open(medium, journal).unwrap();
        // The unconfirmed hole: captured after the settled history,
        // never confirmed (every canonical append faults — a
        // definite pre-commit failure that retains no pending
        // attempt, so every attempt re-collects).
        capture_index(&mut held, HISTORY + 1);
        let mut max_delta = 0u64;
        for attempt in 1..=8u64 {
            let before = held.retention_stats().actionable_scan_envelopes;
            assert!(
                matches!(held.commit_canonical(), Err(PipelineError::Medium(_))),
                "attempt {attempt} must fail on the faulted medium"
            );
            let delta = held.retention_stats().actionable_scan_envelopes - before;
            if attempt == 1 {
                // The session's first scan re-derives the watermark
                // (O(journal) once — the honest open tradition): the
                // 40 settlement proofs plus the held transaction's
                // envelope.
                assert_eq!(delta, HISTORY + 1);
            } else {
                max_delta = max_delta.max(delta);
            }
        }
        assert_eq!(
            max_delta, 1,
            "the held window's per-commit cost is the unconfirmed \
             transaction's envelope alone — never the settled history"
        );
        let stats = held.retention_stats();
        assert_eq!(stats.journal_transactions, HISTORY + 1);
        assert_eq!(
            stats.settled_watermark, HISTORY,
            "the mark is HELD at the unconfirmed transaction"
        );
        assert_eq!(stats.settled_transactions(), HISTORY);
        assert_eq!(stats.unsettled_transactions, 1);
        assert_eq!(
            stats.actionable_scan_transactions,
            HISTORY + 1 + 7,
            "the first scan's 41 transaction visits plus one per held attempt"
        );
        // The held key keeps being collected — never silently
        // dropped, never promoted.
        assert_eq!(
            held.status(ConsequenceId::new(1, HISTORY + 1, 1))
                .unwrap()
                .phase,
            KeyPhase::JournaledT1
        );
        // The hold is not a deadlock: through a clean medium the held
        // transaction confirms and the mark advances past it.
        drop(held);
        let mut recovered = fresh_pipeline(&dir);
        let CommitOutcome::Confirmed(receipt) = recovered.commit_canonical().unwrap() else {
            panic!("the held transaction confirms through the clean medium");
        };
        assert_eq!(receipt.keys, vec![ConsequenceId::new(1, HISTORY + 1, 1)]);
        assert_eq!(
            recovered.commit_canonical().unwrap(),
            CommitOutcome::NothingPending
        );
        let stats = recovered.retention_stats();
        assert_eq!(stats.settled_watermark, HISTORY + 1);
        assert_eq!(stats.unsettled_transactions, 0);
    }

    /// (P3-L6, test 3) THE COVERAGE SHED: past the documented
    /// retention bound, settled producers' entries shed their
    /// coverage detail — the dedup membership NEVER drops
    /// (`is_confirmed`, the producing transaction, the
    /// conflicting-reuse refusal all survive), in-session provenance
    /// queries REFUSE the shed detail, and a fresh open RE-DERIVES
    /// every entry's byte-exact coverage from the chain (the recovery
    /// composition's path). 1,400 settled identities against the
    /// 1,024 bound: 376 shed, the grace window retains the rest.
    #[test]
    fn settled_coverage_sheds_past_the_documented_bound_keeping_membership() {
        let dir = TestDir::new("l6-coverage-shed");
        let mut pipeline = fresh_pipeline(&dir);
        const PER_TRANSACTION: u64 = 140;
        const TRANSACTIONS: u64 = 10;
        const TOTAL: u64 = PER_TRANSACTION * TRANSACTIONS;
        for index in 1..=TRANSACTIONS {
            capture_many(&mut pipeline, index, PER_TRANSACTION);
            let CommitOutcome::Confirmed(receipt) = pipeline.commit_canonical().unwrap() else {
                panic!("transaction {index} must confirm");
            };
            assert_eq!(receipt.keys.len() as u64, PER_TRANSACTION);
        }
        // The last transaction settles at the next scan.
        assert_eq!(
            pipeline.commit_canonical().unwrap(),
            CommitOutcome::NothingPending
        );
        let stats = pipeline.retention_stats();
        assert_eq!(stats.confirmed_keys, TOTAL);
        assert_eq!(stats.settled_transactions(), TRANSACTIONS);
        assert_eq!(stats.settled_watermark, TRANSACTIONS);
        // THE BOUND: 1,400 eligible − 1,024 retained = 376 shed (the
        // oldest settlements first — the FIFO's front).
        assert_eq!(
            stats.shed_coverage_entries,
            TOTAL - SETTLED_COVERAGE_RETAINED as u64
        );
        assert_eq!(
            stats.retained_settled_coverage,
            SETTLED_COVERAGE_RETAINED as u64
        );
        // The shed identities: transaction 1's settlement batch (the
        // FIFO's front) — membership retained, detail refused.
        let shed_id = ConsequenceId::new(1, 1, 1);
        assert!(pipeline.is_confirmed(shed_id), "the membership never drops");
        assert_eq!(
            pipeline.confirmed_transaction(shed_id),
            pipeline
                .journal()
                .transactions()
                .first()
                .map(|transaction| transaction.id),
            "the producing transaction is kept"
        );
        assert_eq!(
            pipeline.confirmed_coverage(shed_id),
            None,
            "the shed detail refuses in-session (it re-derives at open)"
        );
        // A conflicting re-stage of a shed identity still fails
        // closed — the identity authority (keys) is never dropped.
        let mut conflicting = staged_envelopes(99, 1).pop().unwrap();
        conflicting.id = shed_id;
        assert!(matches!(
            pipeline.stage(conflicting),
            Err(PipelineError::ConflictingReuse { .. })
        ));
        // A retained identity (the newest settlement) answers with
        // the full coverage — the grace window.
        let retained_id = ConsequenceId::new(1, TRANSACTIONS, 1);
        assert!(pipeline.is_confirmed(retained_id));
        assert_eq!(
            pipeline.confirmed_coverage(retained_id).unwrap(),
            expected_span(1).as_slice()
        );
        // The export omits the shed detail, keeps the retained —
        // reported in the stats, never silent.
        let evidence = pipeline.confirmed_evidence();
        assert_eq!(evidence.len(), SETTLED_COVERAGE_RETAINED);
        assert!(!evidence.contains_key(&shed_id));
        assert!(evidence.contains_key(&retained_id));
        // THE RE-DERIVATION: a fresh open re-collects every batch
        // entry with full coverage — byte-exact against the staged
        // fixture, for shed and retained identities alike (the
        // recovery composition's exact path).
        drop(pipeline);
        let fresh = fresh_pipeline(&dir);
        for (index, npc) in [
            (1u64, 1u64),
            (1, PER_TRANSACTION),
            (5, 64),
            (TRANSACTIONS, 1),
        ] {
            let id = ConsequenceId::new(1, index, npc as u32);
            assert!(fresh.is_confirmed(id));
            assert_eq!(
                fresh.confirmed_coverage(id).unwrap(),
                expected_span(npc).as_slice(),
                "the open walk re-derives the coverage exactly as before"
            );
        }
        assert_eq!(fresh.confirmed_evidence().len() as u64, TOTAL);
        assert_eq!(fresh.retention_stats().shed_coverage_entries, 0);
    }

    /// (P3-L6, test 4) THE SHED REFUSAL (fail-closed): an identity
    /// whose producing transaction is only PARTIALLY confirmed — one
    /// envelope chain-confirmed by a foreign batch carrying the exact
    /// provenance, the other still unconfirmed — is NEVER
    /// shed-eligible: the entry keeps its full coverage, and the
    /// exact-provenance comparisons that consult it (the stale-rebase
    /// remaining-keys filter) keep succeeding. A shed keyed on
    /// confirmation alone would empty the coverage and turn this
    /// commit into a false ConfirmedProvenanceMismatch.
    #[test]
    fn partially_settled_producer_retains_coverage_for_exact_provenance() {
        let dir = TestDir::new("l6-shed-refusal");
        ensure_founded(&dir);
        let id_a = ConsequenceId::new(1, 1, 1);
        let id_b = ConsequenceId::new(1, 1, 2);
        // Writer A captures ONE transaction carrying both envelopes —
        // the partially-settlable producer.
        let journal_a = T1Journal::open(dir.path().join("journal-a.t1")).unwrap();
        let mut writer_a =
            CommitPipeline::open(LocalFileMedium::open(dir.store()).unwrap(), journal_a).unwrap();
        let transaction = capture_many(&mut writer_a, 1, 2);
        // THE PARTIAL FOREIGN BATCH: a valid canonical revision on
        // the founding carrying ONLY id_a, under the journal
        // envelope's exact provenance (the same producing
        // transaction, coverage, and effect commitment) — a foreign
        // writer's legitimate partial confirmation (a batch need not
        // carry every key of a transaction; the medium does not know
        // journals — the hand-crafted-valid-revision pattern the
        // canonical-conflict battery uses).
        let envelope_a = &staged_envelopes(1, 2)[0];
        let batch = ConsequenceBatch {
            entries: vec![BatchEntry {
                id: id_a,
                transaction,
                coverage: envelope_a.sub_spans.clone(),
                effect: envelope_a.effect_commitment().unwrap(),
            }],
        };
        let batch_object = StoredObject::new(batch.canonical_bytes().unwrap()).unwrap();
        let batch_hash = batch_object.hash();
        let mut medium = LocalFileMedium::open(dir.store()).unwrap();
        let founding_head = medium.head().unwrap().expect("founded");
        let record = RevisionRecord::regular(founding_head, 0, 100)
            .with_idempotency_key(batch_hash.as_bytes().to_vec())
            .with_content_refs(vec![batch_hash]);
        match medium
            .append(
                &record,
                std::slice::from_ref(&batch_object),
                Some(founding_head),
            )
            .unwrap()
        {
            AppendOutcome::Accepted { .. } => {}
            other => panic!("the partial foreign revision must append: {other:?}"),
        }
        drop(medium);
        // A's commit: the actionable scan sees both keys unconfirmed
        // (A's verified head predates the foreign revision); the
        // append meets the advanced head — the stale re-walk learns
        // the partial foreign confirmation — and the rebase's
        // remaining-keys filter checks id_a's entry against the
        // local attempt's EXACT provenance, coverage included: the
        // comparison the shed must have refused to empty (id_a's
        // producer is unsettled — id_b is still unconfirmed).
        let CommitOutcome::Confirmed(receipt) = writer_a.commit_canonical().unwrap() else {
            panic!("the partial foreign confirmation must satisfy id_a exactly");
        };
        assert_eq!(
            receipt.keys,
            vec![id_b],
            "only the unconfirmed key is submitted"
        );
        // The refusal's evidence: BOTH entries carry their full
        // coverage — nothing was eligible (the producer T1 is
        // unsettled at the settlement check).
        assert!(writer_a.is_confirmed(id_a));
        assert!(writer_a.is_confirmed(id_b));
        assert_eq!(
            writer_a.confirmed_coverage(id_a).unwrap(),
            expected_span(1).as_slice(),
            "the partially-settled producer's entry retains its coverage"
        );
        assert_eq!(
            writer_a.confirmed_coverage(id_b).unwrap(),
            expected_span(2).as_slice()
        );
        let stats = writer_a.retention_stats();
        assert_eq!(stats.shed_coverage_entries, 0);
        assert_eq!(stats.retained_settled_coverage, 0);
        assert_eq!(stats.unsettled_transactions, 1);
        // The settlement completes at the next scan (both envelopes
        // now confirmed under exact provenance) — the identities
        // become eligible and are RETAINED within the documented
        // bound: the coverage still answers in-session.
        assert_eq!(
            writer_a.commit_canonical().unwrap(),
            CommitOutcome::NothingPending
        );
        let stats = writer_a.retention_stats();
        assert_eq!(stats.settled_watermark, 1);
        assert_eq!(stats.settled_transactions(), 1);
        assert_eq!(stats.retained_settled_coverage, 2);
        assert_eq!(stats.shed_coverage_entries, 0);
        assert_eq!(
            writer_a.confirmed_coverage(id_a).unwrap(),
            expected_span(1).as_slice(),
            "within the bound the settled detail is retained"
        );
        // The store: founding + the partial foreign revision + A's
        // id_b revision — the foreign partial confirmation installed
        // exactly once, A's rebase exactly once.
        let mut check = LocalFileMedium::open(dir.store()).unwrap();
        assert_eq!(check.validate().unwrap().records, 3);
    }
}
