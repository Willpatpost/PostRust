//! The event spine: canonical campaign clock, scheduling with deterministic
//! `(due, seq)` queue priority, cross-class causal dependencies, per-tick
//! budget with the near/far lane policy, the housekeeping allowance that
//! bounds parked-transition and waiter-release work per drain phase (B01),
//! the execution ledger with its opt-in retention policy (A22-core), and
//! the safe processed-through watermark.
//!
//! Stage 3 (`docs/TECH-ARCHITECTURE.md` §2.4, §6): near and far lanes with
//! the three-phase budget policy — the near reservation is a **guaranteed
//! allocation, not a ceiling**: (1) near-field events execute up to the
//! reservation, (2) far-field events up to the nonreserved capacity, and
//! (3) remaining budget redistributes near-first (reviewer clarification
//! 2026-10-20). The legacy constructors keep `near_reserve == budget`,
//! reproducing the Stage 0 single-lane behavior exactly; the Stage 0
//! fingerprint domain ("engine-core/spine/2") is untouched and stays
//! byte-identical, while the new lane-aware domain ("spine-lanes/1")
//! represents the lane classification and reservation as authoritative
//! state. Regions, LOD levels, and persistence still attach above this
//! core in their stages.
//!
//! The execution-ledger retention policy (A22-core, P2 design §6.2): the
//! ledger is diagnostic history — the identity/dedup authority is the
//! unfinished complement (`seq < next_seq && !unfinished`), never the
//! ledger — so an opt-in [`RetentionPolicy`] bounds the ledger's resident
//! size by firing the existing explicit [`Spine::compact`] at the tick
//! boundary, chunk-bounded per invocation. The default is no policy:
//! exactly today's behavior, byte for byte.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap};
use std::hash::{BuildHasher, Hasher};
use std::num::NonZeroU64;

use crate::clock::Tick;
use crate::diag::{Diagnostics, Level};
use crate::event::{
    Event, EventClass, EventId, EventSpec, ExecutionRecord, Field, Schedule, ScheduleKind,
};
use crate::fingerprint::Fingerprinter;
use crate::sha256::{Sha256, digest};
use crate::wire::{
    Decoder, Encoder, MAX_BYTE_LEN, MAX_COLLECTION_COUNT, MAX_TAG_LEN, SAVE_DOMAIN, SAVE_DOMAIN_V2,
    SAVE_FORMAT_VERSION, SAVE_FORMAT_VERSION_V2, SPINE_DOMAIN, SPINE_HISTORY_DOMAIN, SaveError,
    SaveWorld,
};

/// Game-side event handler. Implemented by the application, never by the
/// engine — game policy stays outside reusable engine mechanisms by
/// *placement*, whatever form it takes (code, data, configuration, or a
/// future scripting layer) (`docs/TECH-ARCHITECTURE.md` §1 P5, review
/// correction 4).
pub trait World {
    /// Executes one event. Follow-ups are scheduled through `ctx`; the
    /// handler never reaches into the spine directly.
    fn execute(&mut self, event: &Event, ctx: &mut ExecCtx);

    /// Feeds canonical logical state into `fp`. Order-stable by contract:
    /// nothing derived from unordered iteration may reach a fingerprint.
    fn fingerprint(&self, fp: &mut Fingerprinter);
}

/// Handler context: what an executing event may observe and enqueue.
pub struct ExecCtx<'a> {
    executed_tick: Tick,
    /// The complement-of-unfinished executed-membership view: the
    /// unfinished-id index plus the allocation frontier. Identical
    /// semantics to the replaced per-identity executed set (the E3
    /// selection — see [`Spine::is_executed`]).
    unfinished: &'a BTreeSet<u64>,
    next_seq: u64,
    /// Lane of the currently executing event: handler follow-ups inherit
    /// their cause's lane (Stage 3 decision — chained work stays in its
    /// lane; an explicit override API is deferred until a use case
    /// exists).
    field: Field,
    follow_ups: Vec<EventSpec>,
}

impl<'a> ExecCtx<'a> {
    /// The tick at which the current event is actually executing.
    pub fn executed_tick(&self) -> Tick {
        self.executed_tick
    }

    pub fn is_executed(&self, id: EventId) -> bool {
        id.seq < self.next_seq && !self.unfinished.contains(&id.seq)
    }

    /// Schedules an execution-relative follow-up: due = executed_tick +
    /// `delay`. Anchors to *actual* execution, not to the event's intended
    /// time — an overdue cause shifts its effects honestly instead of
    /// stacking impossible deadlines (`docs/TECH-ARCHITECTURE.md` §3.2).
    pub fn follow_up(
        &mut self,
        delay: u64,
        class: EventClass,
        payload: u64,
        depends_on: Vec<EventId>,
    ) {
        self.follow_ups.push(EventSpec {
            class,
            schedule: Schedule::ExecutionRelative {
                origin: self.executed_tick,
                delay,
            },
            payload,
            field: self.field,
            depends_on,
        });
    }

    /// Schedules at a fixed logical time. `tick` may lie in the past — a
    /// late arrival; the spine records it and the safe watermark can
    /// retreat.
    pub fn schedule_at(
        &mut self,
        tick: Tick,
        class: EventClass,
        payload: u64,
        depends_on: Vec<EventId>,
    ) {
        self.follow_ups.push(EventSpec {
            class,
            schedule: Schedule::AtLogicalTime { due: tick },
            payload,
            field: self.field,
            depends_on,
        });
    }

    /// Schedules an execution-relative follow-up in an **explicit lane**
    /// — the Stage 3 "deferred until a use case exists" override, whose
    /// use case is Stage 4's dynamic near/far classification: the
    /// workload classifies each *new* follow-up under the region's
    /// current policy, while live obligations keep their creation-time
    /// lane (reclassified only through the explicit transition
    /// operation, [`Spine::reclassify_matching`]). Identity,
    /// dependencies, and accounting are identical to [`Self::follow_up`].
    pub fn follow_up_in_field(
        &mut self,
        field: Field,
        delay: u64,
        class: EventClass,
        payload: u64,
        depends_on: Vec<EventId>,
    ) {
        self.follow_ups.push(EventSpec {
            class,
            schedule: Schedule::ExecutionRelative {
                origin: self.executed_tick,
                delay,
            },
            payload,
            field,
            depends_on,
        });
    }

    fn drain(&mut self) -> Vec<EventSpec> {
        std::mem::take(&mut self.follow_ups)
    }
}

/// Rejection reasons for a scheduling request. Rejections are returned to
/// the caller (or, for handler follow-ups, reported through diagnostics and
/// stats) — never silently dropped (E-10 discipline).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleError {
    /// A dependency references an event that was never scheduled. Because
    /// dependencies must reference already-scheduled or already-executed
    /// events, this rule also makes dependency cycles structurally
    /// impossible (review correction 2).
    UnknownDependency(EventId),
}

/// The spine's execution-history representation (PP-B2, the ratified
/// v1.8 §5.6.1 `HistoryKind`). `Full` retains every
/// [`ExecutionRecord`]; `Compacted` replaced the first
/// `summarized_records` records with the authenticated SHA-256 prefix
/// summary — a derivation witness that authenticates *what was
/// discarded*, never a reconstruction of it — and retains the tail
/// verbatim. The Spine's logical state (scheduling, conservation,
/// watermarks, every fingerprint on `Full` instances) is identical
/// under both representations; only per-record history retention
/// differs. Discarded records are never reconstructible: the
/// full-history APIs surface their unavailability loudly, and the
/// safe accessors ([`Spine::ledger_retained`],
/// [`Spine::try_ledger_full`], the `try_*` fingerprint variants)
/// report it without crashing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryKind {
    Full,
    Compacted {
        /// Records below the compaction cut, summarized into the
        /// prefix summary (cumulative across repeated compactions).
        summarized_records: u64,
        /// The retained tail's record count (always the live ledger
        /// length).
        retained_records: u64,
        /// The authenticated SHA-256 prefix-summary chain over the
        /// summarized records' canonical bytes, seeded from the
        /// spine-history domain tag and chained across repeated
        /// compactions.
        prefix_summary: [u8; 32],
    },
}

/// Typed refusals of the explicit compaction operation
/// ([`Spine::compact`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionError {
    /// The requested retained window covers the whole ledger —
    /// nothing would be summarized, and a compaction that summarizes
    /// nothing is not a compaction (the `spine-history/2` form
    /// requires `summarized_records >= 1`, keeping the full and
    /// compacted forms honestly disjoint).
    NothingToSummarize { ledger_len: u64, retain: u64 },
    /// The requested retained window exceeds the ledger length — the
    /// explicit operation takes an exact window, never a clamped one.
    RetainExceedsLedger { ledger_len: u64, retain: u64 },
}

/// The execution-ledger retention policy (A22-core, P2 design §6.2 —
/// the L7 lane): the configurable cadence that bounds the ledger's
/// resident size by firing the EXISTING explicit compaction
/// ([`Spine::compact`]) — no second compaction mechanism, no second
/// summary domain. The policy is OPT-IN configuration with no
/// default: a spine constructed without one never auto-compacts,
/// keeping today's exact behavior (the no-silent-default rule — the
/// application opts in explicitly through
/// [`Spine::set_retention_policy`], and opting in now CARRIES the
/// eligibility decision [`Spine::compact`]'s own doc delegates to
/// the driver: the acknowledgment token [`ConsequenceEligibility`]
/// is constructed only by a composition boundary that has proven
/// every consequence consumer of the ledger prefix can relinquish
/// it — the engine policy bounds RAM, the driver proves when
/// consequence lanes tolerate a prefix cut, and the proof is
/// required at every reconfiguration, never standing configuration
/// alone; a driver that wants the bound declared without the proof
/// uses [`Spine::set_retention_bound`] and the checkpoint's
/// refusal and lag are observable instead of the cut).
///
/// **The evidence/history split (the contract this policy polices):**
/// executed-membership, exactly-once, conservation, and the
/// watermarks hang off `next_seq`, the `unfinished` complement,
/// `waiting_on`, and the incrementally maintained
/// [`Spine::max_executed_due`] index — never off the ledger. The
/// ledger is diagnostic history: audit trail, the late-accounting
/// source, and the compaction digest's input. The policy therefore
/// sheds only history (each discarded record authenticated into the
/// SHA-256 prefix chain), and the authority is structurally
/// untouched by every cut.
///
/// **The cadence (when it fires):** at the tick boundary — the one
/// checkpoint site, at the end of [`Spine::run_tick`] after every
/// drain phase has completed, never mid-drain — when
/// `ledger_retained().len() > retain + chunk`. The `+ chunk` term is
/// the hysteresis band: the policy tolerates the ledger floating up
/// to `retain + chunk` records before firing, so every firing has a
/// full chunk of work and per-record compaction churn never happens.
///
/// **The chunk bound (what one invocation costs):** each firing
/// summarizes `min(excess, chunk)` records — at the threshold gate
/// exactly `chunk` — so one invocation's SHA-256 work is bounded by
/// `chunk` records hashed BY CONSTRUCTION (the design's ≤ 4,096
/// guidance), and a pre-existing backlog (a restored ledger, a long
/// quiet period) drains at one chunk per tick: bounded per tick,
/// observable in [`Spine::retention_stats`], like every other
/// budget.
///
/// **The resident bound (the active-RAM target):** with the
/// checkpoint running every tick, the retained ledger holds at most
/// [`Self::resident_bound_records`] records at every tick boundary
/// PROVIDED `chunk` is at least the sustained per-tick execution
/// count (the per-tick execution budget bounds that): each tick
/// adds at most `budget` records and each firing removes a full
/// chunk. A `chunk` below the sustained rate still bounds every
/// INVOCATION, but the ledger nets upward — the configuration rule
/// is part of the contract, and [`Spine::retention_stats`] makes the
/// misconfiguration observable. Resident bytes ≈ records ×
/// `size_of::<ExecutionRecord>()` (the ~40 B in-memory stride vs
/// the 31 B wire record — the design's §9-measurement placeholder).
///
/// `retain` is the guaranteed verbatim tail window: choose
/// `retain ≥ budget` when late-accounting consumers need the
/// current tick's records verbatim — the current tick's appends
/// always precede the boundary checkpoint, so a `retain ≥ budget`
/// window always contains them.
///
/// Configuration, never logical state: not fingerprinted, not
/// serialized — a restored spine resumes with NO policy until the
/// driver re-configures, while the compaction the policy already
/// performed persists in the `spine-history/2` save form exactly as
/// an explicit compaction would.
///
/// **WHY the runtime policy is not persisted separately from the
/// canonical world bytes (CA-07's documented why-not):** a
/// spine-side persistence channel would have to ride either the
/// canonical save (making save bytes depend on non-logical
/// configuration — the same logical state would encode
/// differently with and without a policy, breaking the
/// byte-identity discipline every digest pin enforces) or a
/// sidecar (which the engine cannot authenticate into the
/// rotation anchor's evidence without a new trust surface — the
/// on-disk-index rejection's reasoning). Decisively: persisting
/// the eligibility ACKNOWLEDGMENT would let a restored process
/// resume cutting history its consequence consumers never
/// re-vouched for — exactly the hazard this policy's
/// acknowledgment requirement exists to close. The driver that
/// wants continuity re-applies its policy at its own
/// configuration boundary after restore (the pinned
/// restore → reconfigure → next-compact equivalence makes that
/// lossless), and the cuts already performed ride the wire as
/// `spine-history/2` state either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    /// The guaranteed verbatim tail window, in records — the policy
    /// never compacts below it.
    retain: NonZeroU64,
    /// The per-invocation compaction chunk: the maximum records one
    /// firing summarizes (the SHA-256 work bound) and the hysteresis
    /// band above `retain` the ledger may float through before a
    /// firing.
    chunk: NonZeroU64,
}

impl RetentionPolicy {
    /// The policy with the given verbatim tail window and
    /// per-invocation chunk (both non-zero by type: a zero retain
    /// window or a zero chunk is not a policy but a degenerate
    /// compaction loop).
    pub const fn new(retain: NonZeroU64, chunk: NonZeroU64) -> Self {
        RetentionPolicy { retain, chunk }
    }

    /// The guaranteed verbatim tail window, in records.
    pub const fn retain(&self) -> NonZeroU64 {
        self.retain
    }

    /// The per-invocation chunk (records summarized per firing, the
    /// SHA-256 work bound).
    pub const fn chunk(&self) -> NonZeroU64 {
        self.chunk
    }

    /// The documented resident bound: the most retained records the
    /// ledger holds at any tick boundary when `chunk` is at least
    /// the sustained per-tick execution count (see the type doc).
    pub const fn resident_bound_records(&self) -> u64 {
        self.retain.get().saturating_add(self.chunk.get())
    }
}

/// Observational retention counters (the A22-core observability
/// class — like [`LaneTotals`] and [`SuspensionStats`]: never
/// fingerprinted, never serialized, never affecting scheduling; a
/// restored spine resumes them at defaults). `summarized_total`
/// counts POLICY summarizations only — a manual [`Spine::compact`]
/// moves `history_kind()`'s `summarized_records` without touching
/// these counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetentionStats {
    /// Policy-driven compaction invocations (at most one per tick —
    /// the cadence's firing count).
    pub compactions: u64,
    /// Cumulative records summarized by policy invocations.
    pub summarized_total: u64,
    /// The most recent invocation's cut (the chunk witness).
    pub last_chunk: u64,
    /// The largest cut any single invocation has performed.
    pub max_chunk: u64,
    /// Ticks at which the checkpoint found the threshold exceeded
    /// but declined to cut for want of consequence eligibility (a
    /// bound declared through [`Spine::set_retention_bound`] without
    /// the acknowledgment [`Spine::set_retention_policy`] requires)
    /// — the refusal the lag observer
    /// ([`Spine::retention_lag_records`]) makes visible as the
    /// ledger's growth past the bound.
    pub refused_ticks: u64,
}

/// The composition boundary's consequence-eligibility acknowledgment
/// (CA-07/F07 — Q05's eligibility half): the token a driver can only
/// construct by explicitly asserting the proof [`Spine::compact`]'s
/// own doc delegates to it. Constructing this value through
/// [`Self::acknowledged`] is the assertion that EVERY consequence
/// consumer of the execution-ledger prefix — every application
/// lane that reads history records (late accounting over the
/// verbatim tail, the history APIs, audit consumers) — can
/// relinquish those records, because the engine cannot know them:
/// the ledger cut is structurally safe for every ENGINE invariant
/// (the authority hangs off `next_seq`, the unfinished complement,
/// `waiting_on`, and the max-due index — never the ledger), so the
/// only eligibility that can be violated is the caller's own.
///
/// The acknowledgment is REQUIRED at every (re)configuration of the
/// retention policy — the cadence never fires on standing
/// configuration alone, and a restored spine (whose configuration
/// restore clears, an intentional contract) must re-prove it before
/// the next compact. A driver that wants the RAM bound declared
/// without the proof uses [`Spine::set_retention_bound`]: the
/// checkpoint then REFUSES to cut and the refusal and lag are
/// observable ([`RetentionStats::refused_ticks`],
/// [`Spine::retention_lag_records`]) — never a silent default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsequenceEligibility {
    /// Construction is the assertion; the field keeps the value
    /// opaque (there is nothing to read — the words are the proof).
    _asserted: (),
}

impl ConsequenceEligibility {
    /// Constructs the acknowledgment. By constructing this value the
    /// composition boundary asserts it has established that every
    /// consequence consumer of the ledger prefix can relinquish it
    /// — the eligibility proof is the CALLER's, made here, made
    /// explicitly, and re-made at every reconfiguration.
    pub const fn acknowledged() -> Self {
        ConsequenceEligibility { _asserted: () }
    }
}

/// Typed refusals of the held-reconstruction replacement set
/// ([`Spine::try_reconstruct_held`]; the direct
/// [`Spine::reconstruct_held`] surfaces the same conflicts loudly).
/// The reconstruction is a REPLACEMENT — the matching chains are
/// removed, the given events installed as held under their own
/// identities — so a refusal is always about the replacement set's
/// identities, and it is always fail-closed and PRE-MUTATION: the
/// spine is left exactly as the call found it. The divergence class
/// a refusal closes (IA-06): an unchecked install of a conflicting
/// identity gives one identity two simultaneous obligations and
/// manufactures a save the restore gates refuse as
/// `Duplicate { section: "spine-save/identities", .. }` — the audit
/// record lives in the commit that landed the validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconstructionError {
    /// The replacement set repeats an identity: the second install
    /// would silently overwrite the first in the held map (keyed by
    /// seq) — a partial install the conservation accounting cannot
    /// represent, since the scheduled statistic counts both inserts.
    DuplicateReplacement { id: EventId },
    /// The identity still belongs to a SURVIVING obligation — one the
    /// removal predicate did not match, in either lane, the blocked
    /// set, or the held set. Installing it would give one identity
    /// two simultaneous obligations; the save's identity-uniqueness
    /// gate would refuse the resulting spine at restore.
    SurvivingIdentity { id: EventId },
}

/// Spine counters. All fields are logical state and fingerprinted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub scheduled: u64,
    pub executed: u64,
    /// Events scheduled with a due already behind the clock.
    pub late_arrivals: u64,
    /// Executions with `executed_tick > intended_due`.
    pub late_executions: u64,
    /// Due events moved to the blocked set (unmet dependencies).
    pub blocked_transitions: u64,
    /// Handler follow-ups rejected at schedule time — reported, never silent.
    pub invalid_dropped: u64,
    /// Ticks that ended with the budget fully consumed and due work
    /// remaining in the queue.
    pub budget_exhausted_ticks: u64,
}

/// Observational per-tick near/far lane report (Stage 3). Not part of the
/// Stage 0 fingerprint schema and not authoritative state — the ledger
/// and queues already represent executions deterministically; this is
/// instrumentation for diagnostics and the overlay, like `Diagnostics`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LaneReport {
    pub near_executed: u32,
    pub far_executed: u32,
    pub near_reserve: u32,
    pub budget: u32,
    /// Due near work remained when the total budget was exhausted.
    pub near_backlogged: bool,
    /// Due far work remained when the total budget was exhausted.
    pub far_backlogged: bool,
    /// A drain phase this tick was stopped by the housekeeping
    /// allowance — not the execution budget — while due work remained
    /// queued in that phase's lane (B01). Recorded at the break: a
    /// later redistribution phase in the same tick may still drain
    /// that work, so this is the bound-fired signal; the end-of-tick
    /// backlog state is `near_backlogged`/`far_backlogged`.
    pub housekeeping_exhausted: bool,
}

/// Cumulative lane execution totals (observational, Stage 3): derivable
/// from the ledger, kept separately so diagnostics never touch
/// fingerprinted state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LaneTotals {
    pub near_executed: u64,
    pub far_executed: u64,
    pub near_backlogged_ticks: u64,
    pub far_backlogged_ticks: u64,
    /// Ticks in which any drain phase was stopped by the housekeeping
    /// allowance with due work remaining in that phase's lane (B01).
    /// Observational like every `LaneTotals` field — never
    /// fingerprinted, never serialized, never affecting scheduling.
    pub housekeeping_exhausted_ticks: u64,
}

/// Observational housekeeping-work counters (CA-03/F03 — Q03/B01's
/// bounded-work contract): the drain's bookkeeping, counted by CLASS
/// instead of the one blended `housekeeping` number. The blended
/// number stays the charge model (the allowance's unit — one per
/// park, one per release attempt, per `housekeeping_cap`'s
/// documented granularity); these counters publish what the blend
/// hides, which the audit found the phase could perform more of
/// than the counter suggested:
///
/// * a blocked move registers one DEPENDENCY EDGE per unique unmet
///   dependency but charges one unit per PARK — the fan-out is
///   [`Self::dependency_registrations`], not `parks`;
/// * a parent's synchronous release visits one WAITER LIST ENTRY
///   per registered waiter but charges per attempt, summed only
///   after the whole loop — the atomic overshoot is
///   [`Self::waiter_visits`] against the allowance, never a
///   surprise in the total;
/// * a release attempt either DECREMENTS (the waiter still has
///   unmet dependencies — partial release) or UNBLOCKS (the last
///   dependency resolved, the waiter re-queued) —
///   [`Self::release_decrements`] and [`Self::release_unblocks`].
///
/// Like every observational class ([`LaneTotals`],
/// [`SuspensionStats`], [`RetentionStats`]): never fingerprinted,
/// never serialized, never affecting scheduling; a restored spine
/// resumes them at defaults. The registration count covers every
/// waiter-edge registration the spine performs — drain parks,
/// restore-time reconstruction, suspension resumption re-parks, and
/// the held-reconstruction rebuild — the same map mutation is the
/// same work wherever it happens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HousekeepingStats {
    /// Blocked moves parked by a drain phase (the pop + unmet scan +
    /// registration class — the unit the allowance charges).
    pub parks: u64,
    /// Dependency edges registered across every registration site
    /// (parks, restore reconstruction, resumption re-parks, held
    /// rebuilds) — the park's real bookkeeping fan-out.
    pub dependency_registrations: u64,
    /// Waiter-list entries visited by synchronous release (the
    /// release loop's true iteration count — the atomic overshoot's
    /// size).
    pub waiter_visits: u64,
    /// Release visits that only decremented the waiter's remaining
    /// count (partial release — the waiter stays blocked).
    pub release_decrements: u64,
    /// Release visits that resolved the waiter's last unmet
    /// dependency and re-queued it.
    pub release_unblocks: u64,
    /// The worst per-tick blended housekeeping total across all four
    /// drain phases (the quantity the per-phase allowance bounds —
    /// published per tick, not just at exhaustion).
    pub worst_tick_housekeeping: u64,
    /// The worst lane queue length observed at a housekeeping
    /// exhaustion break (an O(1) upper bound on the due backlog
    /// that remained — exact when every queued event is due, the
    /// adversarial shape; the per-tick series a caller samples
    /// through [`Spine::near_pending_count`] /
    /// [`Spine::far_pending_count`] yields the backlog slope).
    pub exhaustion_backlog_peak: u64,
}

/// Observational suspension and reclassification counters (Stage 4
/// diagnostics — like `LaneTotals`, never fingerprinted; the held set
/// itself is the authoritative suspended state).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SuspensionStats {
    /// Total obligations moved into the held set.
    pub suspended_total: u64,
    /// Total obligations returned from the held set.
    pub resumed_total: u64,
    /// Total obligations created directly in the held state
    /// ([`Spine::schedule_held`] — new work targeting dormant regions).
    pub scheduled_held_total: u64,
    /// Total queued/blocked obligations moved between lanes
    /// ([`Spine::reclassify_matching`]).
    pub reclassified_total: u64,
}

/// Where an obligation currently lives (Stage 4 suspension).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObligationLocation {
    Queued,
    Blocked,
    Held,
}

/// Read-only view of one pending or held obligation, for suspension and
/// resumption matching. The spine treats `class` and `payload` as
/// opaque — the mapping between event classes and spatial regions is
/// application policy (Stage 4 reviewer clarification B).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObligationView<'a> {
    pub id: EventId,
    pub due: Tick,
    /// The obligation's FULL schedule form: the collected obligation's
    /// exact semantics — logical-time or execution-relative — must
    /// survive the plan's owned form, so a recovered chain resumes
    /// with its original anchoring, never a flattened due.
    pub schedule: Schedule,
    pub class: EventClass,
    pub payload: u64,
    pub field: Field,
    pub depends_on: &'a [EventId],
    pub location: ObligationLocation,
}

/// Queue entry, ordered by `(due, seq)`. Each dequeue selects the minimum
/// key among currently queued, due events; a causality-blocked event leaves
/// the queue and reenters with its original priority once its dependencies
/// are satisfied, and late arrivals can introduce earlier-due events after
/// later ones have executed — so neither dequeue nor execution order is
/// globally monotonic (see [`EventClass`](crate::EventClass)); the ledger
/// records actual execution.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pending {
    due: Tick,
    seq: u64,
    event: Event,
}

impl Pending {
    fn new(event: Event) -> Self {
        let due = event.schedule.due();
        Pending {
            due,
            seq: event.id.seq,
            event,
        }
    }

    fn view(&self, location: ObligationLocation) -> ObligationView<'_> {
        ObligationView {
            id: self.event.id,
            due: self.due,
            schedule: self.event.schedule,
            class: self.event.class,
            payload: self.event.payload,
            field: self.event.field,
            depends_on: &self.event.depends_on,
            location,
        }
    }
}

impl Ord for Pending {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.due, self.seq).cmp(&(other.due, other.seq))
    }
}

impl PartialOrd for Pending {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// The stored compacted-history state (see [`HistoryKind`]): the
/// cumulative summarized-record count and the authenticated prefix
/// summary. The retained-record count is always the live ledger
/// length, so it is derived, never stored.
struct CompactedHistory {
    summarized_records: u64,
    prefix_summary: [u8; 32],
}

/// The event spine.
pub struct Spine<W: World> {
    clock: Tick,
    next_seq: u64,
    near_queue: BinaryHeap<Reverse<Pending>>,
    far_queue: BinaryHeap<Reverse<Pending>>,
    /// Due events waiting on unmet dependencies, keyed by seq.
    blocked: BTreeMap<u64, Event>,
    /// dependency seq -> dependent seqs currently registered on it.
    waiting_on: BTreeMap<u64, Vec<u64>>,
    /// Derived remaining-dependency counts over BLOCKED events only
    /// (B01, P2 design §3.3(2)): seq → the number of UNIQUE unmet
    /// dependencies at park time, decremented once per executed
    /// prerequisite, so the release path's unblock decision is O(1)
    /// instead of a full dependency-list revalidation. Lifecycle
    /// follows `blocked` exactly: park creates the entry through the
    /// same registration pass that dedups repeated dependency ids,
    /// the completing release consumes it, suspension of a blocked
    /// event drops it alongside its `waiting_on` registrations,
    /// resume and the held-reconstruction rebuild recount through the
    /// normal path, and restore rebuilds it from canonical state.
    /// DERIVED — never serialized, never fingerprinted, rebuilt on
    /// load (the `held_index` precedent).
    ///
    /// REPRESENTATION (the one deliberate deviation from the design
    /// sketch's `BTreeMap`): a lookup-only `HashMap` over a cheap
    /// FNV-1a hasher, pre-reserved at construction. A `BTreeMap`
    /// allocates its root node at the FIRST park, which would move
    /// the spine's pinned main-path allocation counts
    /// (tests/spine_allocations.rs — 18/6880 cold, 9/3024 warm, an
    /// investigation-never-silent-repin gate) for every spine that
    /// ever parks; the pre-reserved hash map keeps every
    /// park/release/unblock operation allocation-free in steady
    /// state, the same property the reused handler scratch bought.
    /// The map is never iterated by production code — keyed `get`,
    /// `insert`, `remove`, and a key-only `retain` — so unordered
    /// iteration can never reach an observable (the `fingerprint.rs`
    /// rule; the L5 static grid's lookup-only hash map is the
    /// precedent, hasher included).
    remaining_deps: HashMap<u64, u32, RemainingDepsHasherBuilder>,
    /// The unfinished-id index — every identity currently holding an
    /// obligation (queued, blocked, or held). Under the ratified
    /// dense-allocation invariant (`known == [0, next_seq)` exactly,
    /// allocation is cancellation-free) executed-membership is the
    /// COMPLEMENT: a seq is executed iff `seq < next_seq` and not in
    /// this index. The index is bounded by live work, never by
    /// lifetime executions — the v1.8 §5.6 erratum E3 selection,
    /// which replaces the per-identity `known`/`executed` membership
    /// sets (one entry per ever-allocated id — lifetime-growing)
    /// with a bounds check plus this index. Representation-internal
    /// and fingerprint-invisible: no Stage 0–4 domain hashed either
    /// set, so every historical pin is mechanically preserved.
    unfinished: BTreeSet<u64>,
    ledger: Vec<ExecutionRecord>,
    /// The compacted-history state; `None` is `HistoryKind::Full`.
    /// The retained-record count is always the live ledger length
    /// (execution appends to the tail), so only the cumulative
    /// summarized count and the authenticated prefix summary are
    /// stored.
    compacted: Option<CompactedHistory>,
    /// Incrementally maintained maximum intended-due among executed
    /// events — updated at the single ledger-append site, so
    /// `max_executed_due()` is O(1) instead of a scan over the
    /// ever-growing execution history (the graphical overlay queries it
    /// and `processed_through()` every frame — Stage 4
    /// final-verification finding). Derived state, value-identical to
    /// the ledger scan; the ledger stays canonical and neither is
    /// fingerprinted (the `held_index` precedent).
    max_executed_index: Option<Tick>,
    world: W,
    budget: u32,
    /// Guaranteed near-lane capacity per tick — never a ceiling; leftover
    /// budget redistributes near-first (§2.4 three-phase policy).
    near_reserve: u32,
    stats: Stats,
    diag: Diagnostics,
    last_report: LaneReport,
    lane_totals: LaneTotals,
    /// Stage 4 suspended obligations, keyed by seq (canonical order —
    /// the authoritative held state). Identity-preserving: records keep
    /// their original `(due, seq)`, lane, schedule, payload, and
    /// dependencies; `stats.scheduled` is never decremented (suspension
    /// changes an obligation's location, not the fact it was scheduled).
    held: BTreeMap<u64, Pending>,
    /// Incrementally maintained `(due, seq)` index over the held set —
    /// updated only at suspension/resumption, so the watermark's held
    /// contribution is O(log n) and dormant cost never scales with the
    /// number of held obligations per tick (Stage 4 reviewer
    /// constraint). Derived state; the held map is canonical.
    held_index: BTreeSet<(Tick, u64)>,
    suspension: SuspensionStats,
    /// The configured execution-ledger retention policy (A22-core):
    /// `None` — the default — never auto-compacts (today's exact
    /// behavior, byte for byte); `Some` declares the bound the
    /// checkpoint enforces — and, with the eligibility
    /// acknowledgment present (CA-07), FIRES the chunked compaction
    /// at the tick boundary (see [`RetentionPolicy`]). Configuration,
    /// never logical state: not fingerprinted, not serialized — a
    /// restored spine resumes with no policy until the driver
    /// re-configures.
    retention: Option<RetentionPolicy>,
    /// The consequence-eligibility acknowledgment arming the
    /// retention cadence (CA-07 — see [`ConsequenceEligibility`]):
    /// a policy may be declared WITHOUT it (the refusal arm — the
    /// checkpoint declines to cut, the lag exposed), and the
    /// acknowledgment must accompany every (re)configuration that
    /// wants firing. Same configuration class as `retention`:
    /// never fingerprinted, never serialized, cleared at restore.
    retention_eligibility: Option<ConsequenceEligibility>,
    /// Observational retention counters (the [`RetentionStats`] doc
    /// — the `LaneTotals`/`SuspensionStats` never-fingerprinted,
    /// never-serialized class).
    retention_stats: RetentionStats,
    /// Observational housekeeping-work counters (the
    /// [`HousekeepingStats`] doc — the same never-fingerprinted,
    /// never-serialized class; the CA-03 class-separate publication
    /// of the drain's bookkeeping).
    housekeeping_stats: HousekeepingStats,
    /// Reusable handler follow-up buffer — taken per execution, drained
    /// into `schedule`, and returned with its capacity retained (the
    /// F-02 caller-held-scratch discipline applied to the spine's
    /// execution loop: collecting handler follow-ups costs no
    /// allocation in steady state). Representation-internal and
    /// fingerprint-invisible — the same class of state as
    /// `max_executed_index`.
    scratch_follow_ups: Vec<EventSpec>,
}

/// Registers a freshly blocked event on every unmet dependency and
/// returns the number of registrations performed — exactly the
/// event's count of UNIQUE unmet dependencies: repeated dependency
/// IDs encounter the same last entry (so they register, and count,
/// once), and a waiter's registrations exist only while it is
/// blocked (suspension and removal drop them with the blocked
/// entry), so every distinct unmet dependency registers exactly
/// once. The return value seeds the derived remaining-dependency
/// count (B01): decremented once per executed prerequisite, it
/// reaches zero exactly when the last unmet dependency resolves.
fn register_waiters_for_unmet_into(
    waiting_on: &mut BTreeMap<u64, Vec<u64>>,
    dependent: u64,
    event: &Event,
    is_executed: impl Fn(u64) -> bool,
) -> u32 {
    let mut registered = 0;
    for id in &event.depends_on {
        if is_executed(id.seq) {
            continue;
        }
        let waiters = waiting_on.entry(id.seq).or_default();
        if waiters.last() != Some(&dependent) {
            waiters.push(dependent);
            registered += 1;
        }
    }
    registered
}

/// Executed-membership by seq over split borrows — the free-function
/// form of [`Spine::is_executed_seq`] (the E3 complement
/// representation: an allocated identity is executed iff below the
/// frontier and not unfinished), so the split-borrow bulk-partition
/// passes (the A08a suspension/resume arms) can evaluate dependencies
/// while a map or queue iterator holds another field. The method
/// delegates here — one source of truth for the membership
/// expression, the `register_waiters_for_unmet_into` precedent.
fn is_executed_seq_with(unfinished: &BTreeSet<u64>, next_seq: u64, seq: u64) -> bool {
    seq < next_seq && !unfinished.contains(&seq)
}

/// Whether `event` still has unexecuted dependencies over split
/// borrows — the free-function form of
/// [`Spine::has_unmet_dependencies`], for the same in-place passes.
fn has_unmet_dependencies_with(unfinished: &BTreeSet<u64>, next_seq: u64, event: &Event) -> bool {
    event
        .depends_on
        .iter()
        .any(|id| !is_executed_seq_with(unfinished, next_seq, id.seq))
}

/// The constructor-time reservation for the derived
/// remaining-dependency counts: enough entries that a spine whose
/// steady blocked set fits inside it never allocates on the park or
/// release path (the allocation-pin discipline — the pin's own
/// module doc names index nodes as pinned structural costs; this
/// reservation moves that structural cost to construction, outside
/// every measured window). `[ASSUMPTION]` — a measurement
/// placeholder in the `PERCEPTION_BUDGETS` tradition: the §9
/// sustained matrix and adversarial graphs measure the real steady
/// blocked-set sizes; beyond the reservation the map simply grows
/// (amortized, never on the pinned scenario's path).
const REMAINING_DEPS_RESERVE: usize = 16;

/// A trivially-cheap deterministic hasher for the derived
/// remaining-dependency counts (FNV-1a over the key bytes — the L5
/// static grid's `CellHasher` precedent). The map is lookup-only —
/// never iterated by production code — so the hash choice is
/// behavior-invisible by construction; it exists purely to keep
/// per-key lookups cheap (SipHash's DoS resistance buys nothing over
/// engine-authored integer keys).
#[derive(Clone, Default)]
struct RemainingDepsHasher(u64);

impl RemainingDepsHasher {
    /// The FNV-1a offset basis.
    const BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    /// The FNV-1a prime.
    const PRIME: u64 = 0x0000_0100_0000_01b3;
}

impl Hasher for RemainingDepsHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            // FNV-1a is DEFINED over wrapping arithmetic (the prime
            // multiply intentionally exceeds u64).
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(Self::PRIME);
        }
    }
}

/// Builds [`RemainingDepsHasher`]s (see its docs for why the default
/// hasher is replaced).
#[derive(Clone, Default)]
struct RemainingDepsHasherBuilder;

impl BuildHasher for RemainingDepsHasherBuilder {
    type Hasher = RemainingDepsHasher;

    fn build_hasher(&self) -> RemainingDepsHasher {
        RemainingDepsHasher(RemainingDepsHasher::BASIS)
    }
}

/// The per-drain-phase housekeeping allowance: one and a half times
/// the phase's execution allowance, in u64 (overflow-free for any
/// u32 cap; `housekeeping_cap(0) == 0` keeps zero-cap phases the
/// exact no-ops they are today).
///
/// WHAT IT BOUNDS (B01, P2 design §3.3(1)): the per-tick work that
/// is housekeeping rather than execution — each causality-blocked
/// park (the pop + unmet scan + waiter registration) and each waiter
/// release attempt costs one unit. NOT charged: the heap peek, the
/// pop of an event that executes (that is the execution's own cost,
/// accounted by the execution cap), and handler follow-up
/// scheduling (part of the executed event's consequence — deferring
/// it would reassign `next_seq` and change observable order, so it
/// stays atomic with the execution). One unit per park and one per
/// release attempt — the successful unblock's queue push folded
/// into its attempt — is the granularity the design's own
/// healthy-ratio derivation dictates: charging the push separately
/// would put the healthy dependency-graph ratio at two units per
/// event, above any `budget + budget/2` allowance, while the folded
/// model keeps it at roughly one.
///
/// WHY ONE AND A HALF TIMES (the default's justification,
/// measurement-pinned): the E-8 healthy sustained shape amortizes to
/// roughly one housekeeping unit per event against an execution
/// budget the events saturate (D_base 154 → 200 at budget 200), so
/// `cap + cap/2` gives 50% headroom — a healthy tick never trips the
/// bound, which is precisely the no-behavior-change property the
/// lane's acceptance demands (sustained-matrix cells identical),
/// while the adversarial shapes the lane targets (thundering herds
/// on one prerequisite, mass due-blocked parking storms) exceed any
/// proportional allowance by design and are cut off at it.
/// `[ASSUMPTION]` — a placeholder in the `PERCEPTION_BUDGETS`
/// tradition: the §9.7 sustained matrix and adversarial graphs
/// measure the real housekeeping-per-execution ratio and re-pin this
/// multiple.
///
/// The bound lives INSIDE `drain_due` (not in `run_tick`), so the
/// three-phase near/far reservation arithmetic is untouched: each
/// phase's housekeeping allowance is proportional to its execution
/// allowance, preserving near/far fairness by construction.
fn housekeeping_cap(cap: u32) -> u64 {
    u64::from(cap) + u64::from(cap) / 2
}

/// One waiter-release attempt's class (CA-03 — the release
/// accounting, counted separately from the blended charge): the
/// charge model stays ONE housekeeping unit per `Decrement` and per
/// `Unblocked` (B01's granularity, unchanged — the class split is
/// observability, never a behavior change).
enum ReleaseAttempt {
    /// The waiter is not currently blocked — a defensive no-op
    /// (uncharged, as ever).
    NotBlocked,
    /// The waiter's remaining-count decremented: still blocked
    /// (partial release), or the lifecycle-unreachable
    /// counter-consumed-without-waiter branch (charged as the
    /// attempt it performed).
    Decrement,
    /// The waiter's last unmet dependency resolved: re-queued.
    Unblocked,
}

/// One drain phase's outcome: the executions performed (the phase's
/// consumption of the tick's execution budget), the blended
/// housekeeping total the phase charged (parks + release attempts —
/// published per tick through [`HousekeepingStats`]'s
/// `worst_tick_housekeeping`), and whether the housekeeping
/// allowance — not the execution cap — stopped the drain while due
/// work remained queued in the phase's lane (B01's
/// observable-exhaustion signal, recorded at the break).
struct DrainOutcome {
    executed: u32,
    housekeeping: u64,
    housekeeping_exhausted: bool,
}

impl<W: World> Spine<W> {
    pub fn new(world: W, budget: u32) -> Self {
        Self::with_diagnostics(world, budget, Diagnostics::new(Level::Off))
    }

    pub fn with_diagnostics(world: W, budget: u32, diag: Diagnostics) -> Self {
        Self::with_reservation_and_diagnostics(world, budget, budget, diag)
    }

    /// Stage 3 near/far lanes: `near_reserve` is the guaranteed near-lane
    /// capacity per tick, clamped to the total budget. The legacy
    /// constructors keep `near_reserve == budget`, which reproduces the
    /// Stage 0 single-lane behavior exactly.
    pub fn with_reservation(world: W, budget: u32, near_reserve: u32) -> Self {
        Self::with_reservation_and_diagnostics(
            world,
            budget,
            near_reserve,
            Diagnostics::new(Level::Off),
        )
    }

    pub fn with_reservation_and_diagnostics(
        world: W,
        budget: u32,
        near_reserve: u32,
        diag: Diagnostics,
    ) -> Self {
        let budget = budget.max(1);
        Spine {
            clock: Tick::ZERO,
            next_seq: 0,
            near_queue: BinaryHeap::new(),
            far_queue: BinaryHeap::new(),
            blocked: BTreeMap::new(),
            waiting_on: BTreeMap::new(),
            remaining_deps: HashMap::with_capacity_and_hasher(
                REMAINING_DEPS_RESERVE,
                RemainingDepsHasherBuilder,
            ),
            unfinished: BTreeSet::new(),
            ledger: Vec::new(),
            compacted: None,
            max_executed_index: None,
            world,
            budget,
            near_reserve: near_reserve.min(budget),
            stats: Stats::default(),
            diag,
            last_report: LaneReport::default(),
            lane_totals: LaneTotals::default(),
            held: BTreeMap::new(),
            held_index: BTreeSet::new(),
            suspension: SuspensionStats::default(),
            retention: None,
            retention_eligibility: None,
            retention_stats: RetentionStats::default(),
            housekeeping_stats: HousekeepingStats::default(),
            scratch_follow_ups: Vec::new(),
        }
    }

    // ------------------------------------------------------------- queries

    pub fn clock(&self) -> Tick {
        self.clock
    }

    pub fn budget(&self) -> u32 {
        self.budget
    }

    /// Guaranteed near-lane capacity per tick (Stage 3).
    pub fn near_reserve(&self) -> u32 {
        self.near_reserve
    }

    /// Events waiting in the near lane (due or not yet due).
    pub fn near_pending_count(&self) -> usize {
        self.near_queue.len()
    }

    /// Events waiting in the far lane (due or not yet due).
    pub fn far_pending_count(&self) -> usize {
        self.far_queue.len()
    }

    /// Observational per-tick lane report (Stage 3 diagnostics).
    pub fn lane_report(&self) -> LaneReport {
        self.last_report
    }

    /// Cumulative lane execution totals (observational, Stage 3).
    pub fn lane_totals(&self) -> LaneTotals {
        self.lane_totals
    }

    pub fn world(&self) -> &W {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut W {
        &mut self.world
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    pub fn diagnostics(&self) -> &Diagnostics {
        &self.diag
    }

    pub fn diagnostics_mut(&mut self) -> &mut Diagnostics {
        &mut self.diag
    }

    /// The full execution history — **full-history API** (the v1.8
    /// §5.6.1 contract): keeps its exact historical meaning on every
    /// instance and never silently returns a retained tail. On a
    /// compacted instance the full history is not retained, so
    /// calling this is a documented contract violation surfaced
    /// LOUDLY — a panic naming the safe accessors — never a silently
    /// misleading value. Production code uses [`Self::history_kind`],
    /// [`Self::ledger_retained`], or [`Self::try_ledger_full`].
    pub fn ledger(&self) -> &[ExecutionRecord] {
        self.require_full_history("ledger()", "ledger_retained() / try_ledger_full()");
        &self.ledger
    }

    /// The spine's execution-history representation (the capability
    /// check every history-touching call site should consult first).
    pub fn history_kind(&self) -> HistoryKind {
        match &self.compacted {
            None => HistoryKind::Full,
            Some(compacted) => HistoryKind::Compacted {
                summarized_records: compacted.summarized_records,
                retained_records: self.ledger.len() as u64,
                prefix_summary: compacted.prefix_summary,
            },
        }
    }

    /// The retained execution records — the full ledger on a `Full`
    /// instance, the verbatim tail on a `Compacted` one (the
    /// explicitly named safe accessor; identical to [`Self::ledger`]
    /// on `Full`).
    pub fn ledger_retained(&self) -> &[ExecutionRecord] {
        &self.ledger
    }

    /// The safe full-history request: `Some` on `Full`, `None` on
    /// `Compacted` — reports unavailability, never crashes, never
    /// fabricates.
    pub fn try_ledger_full(&self) -> Option<&[ExecutionRecord]> {
        if self.compacted.is_some() {
            None
        } else {
            Some(&self.ledger)
        }
    }

    /// The loud full-history guard (the fail-visible contract — see
    /// [`Self::ledger`]).
    fn require_full_history(&self, api: &'static str, remedy: &'static str) {
        if self.compacted.is_some() {
            panic!(
                "{api} requires the full execution history, which this compacted spine does \
                 not retain (history_kind() is Compacted) — use {remedy}, or the try_* \
                 fingerprint variants for the fingerprint domains; calling a full-history \
                 API on a compacted instance is a documented contract violation surfaced \
                 loudly, never a silently misleading value"
            );
        }
    }

    /// Events waiting in either lane (due or not yet due).
    pub fn pending_count(&self) -> usize {
        self.near_queue.len() + self.far_queue.len()
    }

    /// Due events parked on unmet dependencies.
    pub fn blocked_count(&self) -> usize {
        self.blocked.len()
    }

    /// Suspended (held) obligations — authoritative unfinished work that
    /// cannot execute while suspended (Stage 4).
    pub fn held_count(&self) -> usize {
        self.held.len()
    }

    /// Read-only held-obligation views, in canonical seq order.
    pub fn held_view(&self) -> Vec<ObligationView<'_>> {
        self.held
            .values()
            .map(|pending| pending.view(ObligationLocation::Held))
            .collect()
    }

    /// Observational suspension counters (never fingerprinted).
    pub fn suspension_stats(&self) -> SuspensionStats {
        self.suspension
    }

    /// True while any obligation remains unfinished — runnable (queued
    /// or blocked) **or held**. [`Self::run_to_completion`] stops when
    /// no *runnable* work remains; held obligations are unfinished work
    /// that cannot execute while suspended — no runnable work is not no
    /// unfinished work (Stage 4 reviewer clarification C).
    pub fn has_unfinished_work(&self) -> bool {
        self.has_any_work() || !self.held.is_empty()
    }

    pub fn is_executed(&self, id: EventId) -> bool {
        self.is_executed_seq(id.seq)
    }

    /// Executed-membership by seq — the complement of unfinished
    /// under the dense-allocation invariant: an allocated identity is
    /// exactly one of Executed/Queued/Blocked/Held at every stable
    /// boundary, so `seq < next_seq ∧ seq ∉ unfinished` is executed
    /// (the E3 representation; identical semantics to the replaced
    /// per-identity set). Delegates to the free-function form over
    /// split borrows.
    fn is_executed_seq(&self, seq: u64) -> bool {
        is_executed_seq_with(&self.unfinished, self.next_seq, seq)
    }

    /// Maximum intended-due tick among executed events — a raw progress
    /// measure, **not** a completeness claim (review correction 1).
    /// O(1): read from the incrementally maintained index updated at the
    /// ledger-append site (Stage 4 final-verification finding — the
    /// graphical overlay queries this and `processed_through()` every
    /// frame, and a ledger scan would scale per-frame cost with the
    /// total execution history).
    pub fn max_executed_due(&self) -> Option<Tick> {
        self.max_executed_index
    }

    /// Smallest intended due among pending events — queued, blocked, or
    /// held (suspended obligations are authoritative, so suspension can
    /// never let the safe watermark advance past them; Stage 4 reviewer
    /// clarification B). The held contribution reads the incrementally
    /// maintained `(due, seq)` index — O(log n), never a scan.
    pub fn min_pending_due(&self) -> Option<Tick> {
        let mut min = self
            .near_queue
            .peek()
            .map(|item| item.0.due)
            .into_iter()
            .chain(self.far_queue.peek().map(|item| item.0.due))
            .min();
        for event in self.blocked.values() {
            let due = event.schedule.due();
            min = Some(min.map_or(due, |m| m.min(due)));
        }
        if let Some((due, _)) = self.held_index.first() {
            min = Some(min.map_or(*due, |m| m.min(*due)));
        }
        min
    }

    /// Safe processed-through watermark (review correction 1): the largest
    /// tick `T` such that
    ///
    /// 1. `T <= max_executed_due` — completeness is never claimed beyond
    ///    work that has actually run, because a future late arrival may
    ///    still carry an earlier due; and
    /// 2. no pending event — queued, executing, blocked by unresolved
    ///    causal dependencies, **or held (suspended)** — has an intended
    ///    due `<= T`.
    ///
    /// Returns `None` when no such `T` exists: nothing has executed yet, or
    /// a pending event sits at tick 0 (independent audit 2026-10-19, item 1
    /// — the largest safe `T` would have to be negative, so no
    /// non-negative tick is provably complete).
    ///
    /// Computed live from queue state, so it may **retreat** when late
    /// events are scheduled; consumers must re-read, never cache. This is a
    /// **live progress measure, not irreversible finality** — it makes no
    /// permanence claim; durable finality belongs to the Commit Pipeline's
    /// confirmed lane, a separate subsystem. Stage 0 executes
    /// synchronously, so the "executing" state is observable only between
    /// ticks — where this query is meaningful.
    pub fn processed_through(&self) -> Option<Tick> {
        let max_exec = self.max_executed_due()?;
        match self.min_pending_due() {
            Some(min_pending) => {
                let due = min_pending.as_u64();
                if due == 0 {
                    None
                } else {
                    Some(Tick(max_exec.as_u64().min(due - 1)))
                }
            }
            None => Some(max_exec),
        }
    }

    // ---------------------------------------------------------- scheduling

    /// Schedules an event. Dependency ids must reference already-scheduled
    /// or already-executed events; unknown ids are rejected immediately
    /// (which also makes dependency cycles structurally impossible).
    pub fn schedule(&mut self, spec: EventSpec) -> Result<EventId, ScheduleError> {
        // Dense allocation issues exactly `[0, next_seq)`, so `known`
        // is the frontier bounds check (the E3 representation).
        if let Some(unknown) = spec.depends_on.iter().find(|id| id.seq >= self.next_seq) {
            return Err(ScheduleError::UnknownDependency(*unknown));
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        let id = EventId { seq };
        let event = Event {
            id,
            class: spec.class,
            field: spec.field,
            schedule: spec.schedule,
            payload: spec.payload,
            depends_on: spec.depends_on,
        };
        let due = event.schedule.due();
        if due < self.clock {
            self.stats.late_arrivals += 1;
            if self.diag.would_log(Level::Warn) {
                self.diag.log(
                    Level::Warn,
                    format!(
                        "late arrival {id}: due {due} behind clock {} — completeness claims above {due} are invalidated",
                        self.clock
                    ),
                );
            }
        }
        // The guard keeps a suppressed record from allocating at all
        // (the would_log contract) — this site runs once per scheduled
        // event in the live game.
        if self.diag.would_log(Level::Trace) {
            self.diag.log(
                Level::Trace,
                format!(
                    "scheduled {id} class {} payload {} due {due} deps {}",
                    event.class,
                    event.payload,
                    event.depends_on.len()
                ),
            );
        }
        self.unfinished.insert(seq);
        match event.field {
            Field::Near => self.near_queue.push(Reverse(Pending::new(event))),
            Field::Far => self.far_queue.push(Reverse(Pending::new(event))),
        }
        self.stats.scheduled += 1;
        Ok(id)
    }

    /// Schedules an obligation directly into the held (suspended) state
    /// — for new work targeting dormant regions (Stage 4 Batch 2
    /// priority B): it cannot execute while held, so nothing resumes
    /// ordinary processing unnoticed; it resumes with full identity
    /// when its region reactivates, and it is accounted at creation
    /// (both `scheduled` and the held set count it — the conservation
    /// identity holds unchanged). Dependency validation and late-
    /// arrival accounting are identical to [`Self::schedule`].
    pub fn schedule_held(&mut self, spec: EventSpec) -> Result<EventId, ScheduleError> {
        // The dense-allocation bounds check (the E3 representation).
        if let Some(unknown) = spec.depends_on.iter().find(|id| id.seq >= self.next_seq) {
            return Err(ScheduleError::UnknownDependency(*unknown));
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        let id = EventId { seq };
        let event = Event {
            id,
            class: spec.class,
            field: spec.field,
            schedule: spec.schedule,
            payload: spec.payload,
            depends_on: spec.depends_on,
        };
        let pending = Pending::new(event);
        if pending.due < self.clock {
            self.stats.late_arrivals += 1;
            if self.diag.would_log(Level::Warn) {
                self.diag.log(
                    Level::Warn,
                    format!(
                        "late arrival {id}: due {} behind clock {} — completeness claims above {} are invalidated",
                        pending.due, self.clock, pending.due
                    ),
                );
            }
        }
        self.unfinished.insert(seq);
        self.held_index.insert((pending.due, pending.seq));
        self.held.insert(pending.seq, pending);
        self.stats.scheduled += 1;
        self.suspension.scheduled_held_total += 1;
        Ok(id)
    }

    // ---------------------------------------------------------- suspension

    /// Read-only enumeration of the queued and blocked obligations a
    /// subsequent [`Self::suspend_matching`] with the same predicate
    /// would move — the six-phase protocol's non-destructive
    /// preparation query (the v1.8 proposal §5.5.1: a
    /// `DeactivationPlan` builds from read-only sources only, so
    /// phases 1–2 cost nothing and a pre-durable abort leaves the
    /// pre-transition state intact). No mutation, no accounting
    /// change, no suspension stats; held obligations are excluded —
    /// suspension moves only queued and blocked entries. The result
    /// is canonically ordered by `(due, seq)` — the spine's priority
    /// key — so binary-heap layout never reaches the caller (the same
    /// rebuild-invisibility discipline as the suspension mechanics
    /// themselves).
    pub fn collect_matching(
        &self,
        matches: impl Fn(&ObligationView) -> bool,
    ) -> Vec<ObligationView<'_>> {
        let mut collected = Vec::new();
        for Reverse(pending) in self.near_queue.iter() {
            let view = pending.view(ObligationLocation::Queued);
            if matches(&view) {
                collected.push(view);
            }
        }
        for Reverse(pending) in self.far_queue.iter() {
            let view = pending.view(ObligationLocation::Queued);
            if matches(&view) {
                collected.push(view);
            }
        }
        for event in self.blocked.values() {
            let view = event_view(event, ObligationLocation::Blocked);
            if matches(&view) {
                collected.push(view);
            }
        }
        collected.sort_by_key(|view| (view.due, view.id.seq));
        collected
    }

    /// Suspends matching pending (queued or blocked) obligations into
    /// the held set — **identity-preserving** relocation (Stage 4
    /// reviewer clarification A): each record keeps its original
    /// `EventId`, due, class, schedule, payload, lane, and dependencies;
    /// nothing is re-scheduled, and `stats.scheduled` is never
    /// decremented (clarification B — suspension changes an obligation's
    /// location, not the fact it was scheduled).
    ///
    /// Suspending a *queued* event leaves `waiting_on` registrations
    /// intact, so dependents of the suspended event correctly remain
    /// blocked until it resumes and executes. Suspending a *blocked*
    /// event also removes its `waiting_on` registrations, keeping the
    /// dependency-release path hold-unaware; `resume` re-validates
    /// dependencies through the normal path. Held ids remain valid
    /// dependency targets (`known` is untouched). Already-held matches
    /// are no-ops. The caller's predicate decides what matches — the
    /// spine never assumes event classes mean regions (clarification B).
    /// Suspension happens between ticks; an executing event cannot be
    /// suspended.
    ///
    /// The predicate observes candidates in **canonical `(due, seq)`
    /// order** within each lane (near first, then far), then the
    /// blocked set in ascending-seq order — the canonical-visitation
    /// contract (IA-02): a stateful
    /// `FnMut` closure sees the identical visitation sequence on a
    /// live spine and on a save→restored copy of the same spine, so
    /// its selection can never depend on heap-rebuild layout.
    pub fn suspend_matching(&mut self, mut matches: impl FnMut(&ObligationView) -> bool) -> u32 {
        let mut suspended = 0u32;
        suspended += Self::suspend_from_queue(
            &mut self.near_queue,
            &mut self.held,
            &mut self.held_index,
            &mut matches,
        );
        suspended += Self::suspend_from_queue(
            &mut self.far_queue,
            &mut self.held,
            &mut self.held_index,
            &mut matches,
        );
        // The blocked arm of the same bulk-partition shape (A08a): one
        // allocation-free in-place pass — `extract_if` yields the
        // removed entries by value in canonical seq order, with no
        // collect-seqs-then-remove temp Vec. The split field borrows
        // are what make the in-place pass possible: the dependency
        // check runs through the free-function helpers (never a
        // whole-spine `&self` method call) while the map iterator
        // holds `blocked`, and the borrow checker proves every
        // touched field disjoint. `unfinished` and `next_seq` are
        // read-only here, so each entry is judged in ascending-seq
        // order against the same fixed view, and every per-entry
        // decision — registration cleanup, `held_index`/`held`
        // insertion — matches the two-phase form.
        let unfinished = &self.unfinished;
        let next_seq = self.next_seq;
        let extracted = self.blocked.extract_if(.., |_, event| {
            matches(&event_view(event, ObligationLocation::Blocked))
        });
        for (seq, event) in extracted {
            // Leave the dependency-release path hold-unaware: drop
            // this event's registrations on its unexecuted deps.
            for dep in &event.depends_on {
                if !is_executed_seq_with(unfinished, next_seq, dep.seq)
                    && let Some(waiters) = self.waiting_on.get_mut(&dep.seq)
                {
                    waiters.retain(|&waiter| waiter != seq);
                    if waiters.is_empty() {
                        self.waiting_on.remove(&dep.seq);
                    }
                }
            }
            let pending = Pending::new(event);
            self.held_index.insert((pending.due, pending.seq));
            self.held.insert(pending.seq, pending);
            suspended += 1;
        }
        // The derived remaining-dependency counts follow the blocked
        // map's lifecycle exactly: the suspended entries drop with
        // their `waiting_on` registrations, and the survivors' counts
        // are untouched — suspension executes nothing, so every
        // survivor's unmet-dependency count is exactly what it was.
        // The key-only retain keeps the map's lookup-only discipline
        // (no unordered iteration reaches an observable).
        let blocked = &self.blocked;
        self.remaining_deps
            .retain(|&seq, _| blocked.contains_key(&seq));
        self.suspension.suspended_total += u64::from(suspended);
        suspended
    }

    /// The id-exact batch variant of [`Self::suspend_matching`] (the
    /// v1.8 proposal §5.5.2's staged-install step): suspends exactly
    /// the obligations a previously collected plan names, with all the
    /// identity-preserving mechanics of the predicate form. Ids not
    /// currently queued or blocked (already executed, already held,
    /// or never allocated) are not moved and not counted — the
    /// caller's preparation validated presence against
    /// [`Self::collect_matching`], so a short count is the
    /// staged-commit discipline's defensive-failure signal, never a
    /// silent partial install.
    pub fn suspend_ids(&mut self, ids: &[EventId]) -> u32 {
        let targets: BTreeSet<u64> = ids.iter().map(|id| id.seq).collect();
        self.suspend_matching(|view| targets.contains(&view.id.seq))
    }

    /// The recovery's id-exact held reconstruction (the v1.8 §5.5.1
    /// case 6 load path, never a live-mutation scheduling API):
    /// REMOVE every queued, blocked, AND held obligation matching
    /// `matches` — the stale pre-crash chains, superseded by the
    /// journal's plan data — then INSERT `events` as held under
    /// their OWN identities (the dormant region's chains exactly as
    /// the deactivation captured them). The scheduled statistic
    /// adjusts by the replacement delta so the conservation identity
    /// holds. Returns the inserted count.
    ///
    /// The replacement set is VALIDATED before any mutation (IA-06,
    /// the audit's disposition option (a) — validate rather than
    /// restrict the API to a pre-validated plan, so the recovery
    /// callers keep this shape): every replacement identity must be
    /// unique within the set and free of every SURVIVING obligation
    /// (one the predicate did not match). A conflicting set is a
    /// documented contract violation surfaced LOUDLY here — the
    /// `ledger()`/`try_ledger_full` discipline — because proceeding
    /// would give one identity two simultaneous obligations and
    /// manufacture a save the restore gates must refuse as
    /// `Duplicate`; the typed refusal lives on
    /// [`Self::try_reconstruct_held`] for callers that compose the
    /// conflict into their own fail-closed reporting.
    pub fn reconstruct_held(
        &mut self,
        matches: impl FnMut(&ObligationView) -> bool,
        events: Vec<Event>,
    ) -> u32 {
        self.try_reconstruct_held(matches, events).expect(
            "reconstruct_held requires a replacement set whose identities are unique among \
             themselves and free of every surviving obligation — a conflicting set would give \
             one identity two simultaneous obligations and manufacture a save the restore \
             gates refuse as Duplicate; use try_reconstruct_held for the typed refusal \
             (ReconstructionError); an inconsistent reconstruction input is a documented \
             contract violation surfaced loudly, never a silent duplicate install",
        )
    }

    /// The typed-refusal form of [`Self::reconstruct_held`]: identical
    /// replacement semantics, with an inconsistent replacement set
    /// returned as [`ReconstructionError`] instead of surfaced loudly
    /// — fail-closed and PRE-MUTATION, so a refusal leaves the spine
    /// observationally exact (the lanes re-heap from the decision
    /// snapshot's own multiset; `unfinished`, `blocked`, `held`, and
    /// every statistic are untouched) and the spine still saves and
    /// restores cleanly. The failure class a refusal closes (IA-06,
    /// see [`ReconstructionError`]): an unchecked install of a
    /// SURVIVING identity gives one identity two simultaneous
    /// obligations — a save the restore gates must refuse as
    /// `Duplicate`.
    ///
    /// Replacement identities MAY reuse identities the removal just
    /// freed (the crossing's recovery re-installs the journaled
    /// chains under their own ids), and may carry formerly completed
    /// or freshly journaled identities — the documented recovery
    /// semantics; the gate is exactly the uniqueness the save's
    /// identity checks enforce, so an accepted reconstruction can
    /// never manufacture a spine the restore gates must refuse. The
    /// removal predicate observes candidates in the same order the
    /// unvalidated form always visited: each lane ascending
    /// `(due, seq)`, then the blocked and held sets in ascending-seq
    /// order.
    pub fn try_reconstruct_held(
        &mut self,
        mut matches: impl FnMut(&ObligationView) -> bool,
        events: Vec<Event>,
    ) -> Result<u32, ReconstructionError> {
        // The decision snapshot — ONE predicate pass, in the exact
        // order the unvalidated form visited (each lane ascending
        // (due, seq) — a min-heap over unique keys pops canonically —
        // then the blocked and held maps in ascending-seq order), with
        // every structure intact: the lanes are drained into the
        // snapshot, `unfinished` keeps every identity, and a refusal
        // below re-heaps the lanes from the same multiset (heap
        // layout is rebuild-dependent and invisible — the restore
        // path's own argument).
        let (near_retained, near_dropped) =
            Self::partition_queue(&mut self.near_queue, &mut matches);
        let (far_retained, far_dropped) = Self::partition_queue(&mut self.far_queue, &mut matches);
        let blocked_drop: Vec<u64> = self
            .blocked
            .values()
            .filter(|event| matches(&event_view(event, ObligationLocation::Blocked)))
            .map(|event| event.id.seq)
            .collect();
        let held_drop: Vec<u64> = self
            .held
            .values()
            .filter(|pending| matches(&pending.view(ObligationLocation::Held)))
            .map(|pending| pending.seq)
            .collect();

        // The pre-mutation replacement validation (IA-06): the
        // survivors are exactly the non-matching halves — the
        // obligations still holding their identities after the
        // removal — so the replacement set must be unique within
        // itself and disjoint from them.
        let blocked_drop_set: BTreeSet<u64> = blocked_drop.iter().copied().collect();
        let held_drop_set: BTreeSet<u64> = held_drop.iter().copied().collect();
        let mut surviving: BTreeSet<u64> = BTreeSet::new();
        surviving.extend(
            near_retained
                .iter()
                .chain(far_retained.iter())
                .map(|Reverse(pending)| pending.seq),
        );
        surviving.extend(
            self.blocked
                .keys()
                .copied()
                .filter(|seq| !blocked_drop_set.contains(seq)),
        );
        surviving.extend(
            self.held
                .keys()
                .copied()
                .filter(|seq| !held_drop_set.contains(seq)),
        );
        let mut replacement_ids: BTreeSet<u64> = BTreeSet::new();
        let refusal = events.iter().find_map(|event| {
            if surviving.contains(&event.id.seq) {
                Some(ReconstructionError::SurvivingIdentity { id: event.id })
            } else if !replacement_ids.insert(event.id.seq) {
                Some(ReconstructionError::DuplicateReplacement { id: event.id })
            } else {
                None
            }
        });
        if let Some(error) = refusal {
            // Fail-closed, pre-mutation: restore both lanes from the
            // snapshot's multiset — every canonical observable (save
            // bytes, fingerprints, counts) is unchanged.
            Self::restore_queue(&mut self.near_queue, near_retained, near_dropped);
            Self::restore_queue(&mut self.far_queue, far_retained, far_dropped);
            return Err(error);
        }

        // The removal half — applied only after the validation
        // passed, in the order the unvalidated form applied it (the
        // lanes' unfinished removals first, so the blocked cleanup's
        // executed-membership answers are identical).
        let mut removed = 0u64;
        for Reverse(pending) in near_dropped {
            self.unfinished.remove(&pending.seq);
            removed += 1;
        }
        for Reverse(pending) in far_dropped {
            self.unfinished.remove(&pending.seq);
            removed += 1;
        }
        self.near_queue = BinaryHeap::from(near_retained);
        self.far_queue = BinaryHeap::from(far_retained);
        for seq in blocked_drop {
            if let Some(event) = self.blocked.remove(&seq) {
                // The suspend path's discipline: leave the
                // dependency-release path unaware of the removal.
                for dep in &event.depends_on {
                    if !self.is_executed_seq(dep.seq)
                        && let Some(waiters) = self.waiting_on.get_mut(&dep.seq)
                    {
                        waiters.retain(|&waiter| waiter != seq);
                        if waiters.is_empty() {
                            self.waiting_on.remove(&dep.seq);
                        }
                    }
                }
                self.unfinished.remove(&seq);
                removed += 1;
            }
        }
        for seq in held_drop {
            if let Some(pending) = self.held.remove(&seq) {
                self.held_index.remove(&(pending.due, pending.seq));
                self.unfinished.remove(&seq);
                removed += 1;
            }
        }
        let mut inserted = 0u32;
        for event in events {
            let pending = Pending::new(event);
            self.held_index.insert((pending.due, pending.seq));
            self.unfinished.insert(pending.seq);
            self.held.insert(pending.seq, pending);
            inserted += 1;
        }
        // Recovery can replace formerly completed identities with held ones.
        // Rebuild the derived reverse index so every surviving blocked event
        // is registered on all dependencies that are now unfinished — and
        // rebuild the derived remaining-dependency counts through the same
        // pass (B01): the removals may have resolved or newly unmet a
        // survivor's dependencies, so every count is recomputed against the
        // current unfinished set, exactly what the release path's O(1)
        // check would consult.
        self.waiting_on.clear();
        self.remaining_deps.clear();
        for event in self.blocked.values() {
            let registered =
                register_waiters_for_unmet_into(&mut self.waiting_on, event.id.seq, event, |seq| {
                    seq < self.next_seq && !self.unfinished.contains(&seq)
                });
            self.housekeeping_stats.dependency_registrations += u64::from(registered);
            self.remaining_deps.insert(event.id.seq, registered);
        }
        // The replacement semantics: the conservation identity holds
        // with the scheduled statistic adjusted by the delta.
        self.stats.scheduled = self
            .stats
            .scheduled
            .saturating_sub(removed)
            .checked_add(u64::from(inserted))
            .expect("the scheduled count is bounded");
        Ok(inserted)
    }

    /// Partitions one lane into the reconstruction's decision
    /// snapshot (the validation's read-only source): `into_vec`
    /// consumes the heap's buffer, the sort exits it into canonical
    /// `(due, seq)` order — the order a min-heap over unique keys
    /// pops ascending (the IA-02
    /// canonical-visitation discipline) — and `extract_if` is the
    /// retain-style compaction yielding the matching half by value.
    /// Returns `(retained, dropped)`; NO other structure is touched
    /// (`unfinished` keeps every identity), so the caller applies
    /// the snapshot only after the replacement-set validation passes
    /// and re-heaps both halves on refusal.
    fn partition_queue(
        queue: &mut BinaryHeap<Reverse<Pending>>,
        matches: &mut impl FnMut(&ObligationView) -> bool,
    ) -> (Vec<Reverse<Pending>>, Vec<Reverse<Pending>>) {
        let mut items = std::mem::take(queue).into_vec();
        // Canonical visitation for the public predicate (IA-02 — the
        // suspend_from_queue argument): the buffer enters heap order;
        // the predicate must exit it into `(due, seq)` order before
        // any call.
        items.sort_unstable_by_key(|Reverse(pending)| (pending.due, pending.seq));
        let dropped: Vec<Reverse<Pending>> = items
            .extract_if(.., |Reverse(pending)| {
                matches(&pending.view(ObligationLocation::Queued))
            })
            .collect();
        (items, dropped)
    }

    /// Re-heaps one lane from the snapshot's halves — the refusal
    /// path's exact restore: the multiset (retained + dropped) is the
    /// lane the snapshot drained, so every canonical observable is
    /// unchanged (heap buffer layout is rebuild-dependent and
    /// invisible — the restore path's own argument; save bytes and
    /// the fingerprints all sort canonically).
    fn restore_queue(
        queue: &mut BinaryHeap<Reverse<Pending>>,
        mut retained: Vec<Reverse<Pending>>,
        dropped: Vec<Reverse<Pending>>,
    ) {
        retained.extend(dropped);
        *queue = BinaryHeap::from(retained);
    }

    /// Returns matching held obligations to pending state with their
    /// original `(due, seq)`, lane, schedule, payload, and dependencies
    /// — identity preserved through the entire suspension. An event
    /// suspended while queued simply re-enters its lane with its
    /// original priority; an event suspended while blocked re-validates
    /// its dependencies — queueing if all have executed, re-entering the
    /// blocked set with fresh `waiting_on` registrations otherwise
    /// (execution statistics are untouched either way). Non-held
    /// matches are no-ops.
    ///
    /// Resumed obligations that became overdue during suspension keep
    /// their original due and record lateness upon actual execution —
    /// clock coherence is a property of the reconstructed state, never
    /// an erasure of historical due times (reviewer clarification C).
    pub fn resume_matching(&mut self, mut matches: impl FnMut(&ObligationView) -> bool) -> u32 {
        // The A08a bulk-partition shape: one allocation-free in-place
        // pass over the held map — `extract_if` yields the removed
        // entries by value in canonical seq order, with no
        // collect-seqs-then-remove temp Vec. The split field borrows
        // (the `register_waiters_for_unmet_into` precedent: the
        // free-function dependency helpers plus direct per-field
        // access, never a whole-spine method call) let the pass
        // update `held_index`, route entries to their lanes, and
        // re-register blocked waiters while the map iterator holds
        // `held` — the borrow checker proves every touched field
        // disjoint. `unfinished` is read-only here (resume never
        // mutates it), so entries are judged in the same
        // ascending-seq order against the same fixed view, and
        // revalidation, registration, and lane routing match the
        // two-phase form.
        let unfinished = &self.unfinished;
        let next_seq = self.next_seq;
        let mut resumed = 0u32;
        let extracted = self.held.extract_if(.., |_, pending| {
            matches(&pending.view(ObligationLocation::Held))
        });
        for (seq, pending) in extracted {
            self.held_index.remove(&(pending.due, pending.seq));
            if !has_unmet_dependencies_with(unfinished, next_seq, &pending.event) {
                match pending.event.field {
                    Field::Near => self.near_queue.push(Reverse(pending)),
                    Field::Far => self.far_queue.push(Reverse(pending)),
                }
            } else {
                let remaining = register_waiters_for_unmet_into(
                    &mut self.waiting_on,
                    seq,
                    &pending.event,
                    |dep| dep < next_seq && !unfinished.contains(&dep),
                );
                // Re-parking recounts through the normal path: the
                // count reflects the dependencies unmet NOW (the
                // suspension may have spanned prerequisite
                // executions, and the old count left with the
                // suspension).
                self.housekeeping_stats.dependency_registrations += u64::from(remaining);
                self.remaining_deps.insert(seq, remaining);
                self.blocked.insert(seq, pending.event);
            }
            resumed += 1;
        }
        self.suspension.resumed_total += u64::from(resumed);
        resumed
    }

    /// Bulk-partitions one lane, holding matching entries and
    /// heapifying the rest back into the same buffer (the A08a
    /// linear shape, P2 design §4.2 step 1): `into_vec` consumes the
    /// heap in O(1) reusing its buffer, `extract_if` is the
    /// retain-style compaction that yields the removed entries by
    /// value (no temp Vec — the held inserts receive them directly),
    /// and `BinaryHeap::from` heapifies in O(N) with no allocation
    /// (the buffer transfers). Net O(N log N) — the sort below
    /// dominates — with zero scratch allocations per call.
    ///
    /// The taken buffer is **sorted into canonical `(due, seq)` order
    /// before the partition** (IA-02): the public `FnMut` predicate
    /// must observe candidates in canonical order — never the heap's
    /// rebuild-dependent buffer order, which a save→restore rebuilds
    /// differently from live scheduling (a stateful predicate's
    /// selection would otherwise change across restore). The sort is
    /// a transient reordering of the already-owned buffer —
    /// allocation-free (`sort_unstable_by_key` never allocates,
    /// unlike the stable sort's scratch), deterministic because
    /// `(due, seq)` is unique per lane under dense allocation, and
    /// invisible to every other observable: the retained multiset is
    /// unchanged, so pop order, save bytes, and fingerprints — all
    /// derived from the canonical keys — are identical.
    fn suspend_from_queue(
        queue: &mut BinaryHeap<Reverse<Pending>>,
        held: &mut BTreeMap<u64, Pending>,
        held_index: &mut BTreeSet<(Tick, u64)>,
        matches: &mut impl FnMut(&ObligationView) -> bool,
    ) -> u32 {
        let mut suspended = 0u32;
        let mut items = std::mem::take(queue).into_vec();
        // Canonical visitation for the public predicate (IA-02): the
        // buffer enters heap order; the predicate must exit it into
        // `(due, seq)` order before any call.
        items.sort_unstable_by_key(|Reverse(pending)| (pending.due, pending.seq));
        let extracted = items.extract_if(.., |Reverse(pending)| {
            matches(&pending.view(ObligationLocation::Queued))
        });
        for Reverse(pending) in extracted {
            held_index.insert((pending.due, pending.seq));
            held.insert(pending.seq, pending);
            suspended += 1;
        }
        *queue = BinaryHeap::from(items);
        suspended
    }

    /// Moves matching queued and blocked obligations to the target lane,
    /// **preserving full identity** — `(due, seq)`, schedule, payload,
    /// and dependencies (Stage 4 Batch 2 priority C: dynamic near/far
    /// classification, so a newly player-proximate region is protected
    /// by the near reservation immediately instead of starving behind
    /// far-lane overload for the remainder of a long event-driven
    /// cadence). The execution ledger records the lane at execution
    /// time, as always. Conservation is untouched — obligations stay
    /// queued/blocked; only their lane changes. Matching events already
    /// in the target lane are left as they are (not counted). Held
    /// obligations are **not** reclassified here: `resume` restores
    /// their original lane, and the caller reclassifies after
    /// resumption if the region's policy changed.
    ///
    /// The predicate observes queued candidates in **canonical
    /// `(due, seq)` order** within each source lane (near first, then
    /// far — the target lane is skipped whole by the source == target
    /// early return), then the blocked set in ascending-seq order —
    /// the same canonical-visitation contract as
    /// [`Self::suspend_matching`] (IA-02): identical visitation, and
    /// therefore identical selection, on a live spine and on a
    /// save→restored copy of the same spine.
    pub fn reclassify_matching(
        &mut self,
        mut matches: impl FnMut(&ObligationView) -> bool,
        field: Field,
    ) -> u32 {
        let mut moved: Vec<Pending> = Vec::new();
        Self::reclassify_queue(
            &mut self.near_queue,
            &mut matches,
            field,
            Field::Near,
            &mut moved,
        );
        Self::reclassify_queue(
            &mut self.far_queue,
            &mut matches,
            field,
            Field::Far,
            &mut moved,
        );
        let mut reclassified = moved.len() as u32;
        for mut pending in moved {
            pending.event.field = field;
            match field {
                Field::Near => self.near_queue.push(Reverse(pending)),
                Field::Far => self.far_queue.push(Reverse(pending)),
            }
        }
        for event in self.blocked.values_mut() {
            if event.field != field && matches(&event_view(event, ObligationLocation::Blocked)) {
                event.field = field;
                reclassified += 1;
            }
        }
        self.suspension.reclassified_total += u64::from(reclassified);
        reclassified
    }

    /// Bulk-partitions one lane for reclassification, collecting
    /// matching obligations into `moved` (the caller re-lanes and
    /// re-pushes them) and heapifying the rest back into the same
    /// buffer — the A08a linear shape (see
    /// [`Self::suspend_from_queue`]): the canonical pre-partition
    /// sort plus O(N) partition and heapify, with zero scratch
    /// allocations (the caller's `moved` Vec is the operation's
    /// output, not scratch). The sort gives the public predicate the
    /// canonical `(due, seq)` visitation (IA-02 — see
    /// [`Self::suspend_from_queue`]); every other
    /// observable derives from the canonical keys, so the retained
    /// multiset's rebuild stays invisible.
    fn reclassify_queue(
        queue: &mut BinaryHeap<Reverse<Pending>>,
        matches: &mut impl FnMut(&ObligationView) -> bool,
        target: Field,
        source: Field,
        moved: &mut Vec<Pending>,
    ) {
        if target == source {
            return;
        }
        let mut items = std::mem::take(queue).into_vec();
        // Canonical visitation for the public predicate (IA-02): the
        // buffer enters heap order; the predicate must exit it into
        // `(due, seq)` order before any call.
        items.sort_unstable_by_key(|Reverse(pending)| (pending.due, pending.seq));
        let extracted = items.extract_if(.., |Reverse(pending)| {
            matches(&pending.view(ObligationLocation::Queued))
        });
        for Reverse(pending) in extracted {
            moved.push(pending);
        }
        *queue = BinaryHeap::from(items);
    }

    // -------------------------------------------------------------- ticking

    /// Advances one tick and processes due events under the per-tick
    /// budget. The budget is a *mechanism*; the budget *value* is game
    /// policy (P5).
    ///
    /// §2.4 three-phase policy (reviewer clarification 2026-10-20): the
    /// near reservation is a **guaranteed allocation, not a ceiling** —
    /// (1) near-field events execute up to the reservation, (2) far-field
    /// events up to the nonreserved capacity, and (3) remaining budget
    /// redistributes near-first. With the legacy constructors
    /// (`near_reserve == budget`, all events Near by default) the three
    /// phases collapse to the Stage 0 single-queue behavior exactly.
    pub fn run_tick(&mut self) -> Tick {
        self.clock = self.clock.saturating_add(1);
        let near_guaranteed = self.drain_due(Field::Near, self.near_reserve);
        let far_guaranteed = self.drain_due(Field::Far, self.budget - self.near_reserve);
        let near_extra = self.drain_due(
            Field::Near,
            self.budget - near_guaranteed.executed - far_guaranteed.executed,
        );
        let far_extra = self.drain_due(
            Field::Far,
            self.budget - near_guaranteed.executed - far_guaranteed.executed - near_extra.executed,
        );
        let near_executed = near_guaranteed.executed + near_extra.executed;
        let far_executed = far_guaranteed.executed + far_extra.executed;
        let executed = near_executed + far_executed;
        // CA-03's worst-per-tick publication: the blended housekeeping
        // total across all four drain phases (parks + release
        // attempts), published at its worst — the quantity the
        // per-phase allowance bounds, now observable without
        // tripping the bound. Observational only (the
        // `HousekeepingStats` class).
        let tick_housekeeping = near_guaranteed.housekeeping
            + far_guaranteed.housekeeping
            + near_extra.housekeeping
            + far_extra.housekeeping;
        self.housekeeping_stats.worst_tick_housekeeping = self
            .housekeeping_stats
            .worst_tick_housekeeping
            .max(tick_housekeeping);
        // B01's observable-exhaustion signal, recorded at the break:
        // any phase's drain was stopped by the housekeeping allowance
        // while due work remained queued in that phase's lane. The
        // execution-budget exhaustion below stays its own, untouched
        // class (fingerprinted Stats) — each counter reports its own
        // bound, and a tick may set either, both, or neither.
        let housekeeping_exhausted = near_guaranteed.housekeeping_exhausted
            || far_guaranteed.housekeeping_exhausted
            || near_extra.housekeeping_exhausted
            || far_extra.housekeeping_exhausted;
        if housekeeping_exhausted {
            self.lane_totals.housekeeping_exhausted_ticks += 1;
            if self.diag.would_log(Level::Info) {
                self.diag.log(
                    Level::Info,
                    format!(
                        "housekeeping allowance exhausted at {} with due work remaining",
                        self.clock
                    ),
                );
            }
        }
        let near_backlogged = self.lane_has_due_work(Field::Near);
        let far_backlogged = self.lane_has_due_work(Field::Far);
        if executed >= self.budget && (near_backlogged || far_backlogged) {
            self.stats.budget_exhausted_ticks += 1;
            if self.diag.would_log(Level::Info) {
                self.diag.log(
                    Level::Info,
                    format!("budget exhausted at {} with due work remaining", self.clock),
                );
            }
        }
        self.lane_totals.near_executed += u64::from(near_executed);
        self.lane_totals.far_executed += u64::from(far_executed);
        if near_backlogged {
            self.lane_totals.near_backlogged_ticks += 1;
        }
        if far_backlogged {
            self.lane_totals.far_backlogged_ticks += 1;
        }
        self.last_report = LaneReport {
            near_executed,
            far_executed,
            near_reserve: self.near_reserve,
            budget: self.budget,
            near_backlogged,
            far_backlogged,
            housekeeping_exhausted,
        };
        // The A22-core retention checkpoint: the policy's ONE firing
        // site, at the tick boundary — after every drain phase and the
        // report assembly, so compaction can never interrupt
        // execution, parking, or waiter-release work (never mid-drain
        // by construction; the only other compaction entry point is
        // the public explicit `compact`, which stays the driver's
        // between-ticks tool). With no policy configured the
        // checkpoint is a no-op with zero cost (the allocation-pin
        // discipline).
        self.apply_retention_policy();
        self.clock
    }

    pub fn run_ticks(&mut self, ticks: u64) -> Tick {
        for _ in 0..ticks {
            self.run_tick();
        }
        self.clock
    }

    /// Runs until no *progressible* work remains (the queues are empty —
    /// a blocked obligation whose unmet dependencies all resolve to
    /// held, or to blocked chains beneath held roots, can never execute,
    /// so advancing campaign time would be pointless — Batch 1 review
    /// item C) or `max_ticks` elapse; returns the ticks consumed. Held
    /// obligations are unfinished but not executable, and stopping never
    /// implies all obligations completed — see
    /// [`Self::has_unfinished_work`] (no runnable work ≠ no unfinished
    /// work).
    ///
    /// Termination is guaranteed only for **finite** event graphs: within a
    /// fixed scheduled set, dependency chains are acyclic by construction
    /// and the advancing clock eventually reaches every pending due. A
    /// `World` whose handlers unconditionally spawn follow-ups can keep the
    /// spine busy indefinitely — `max_ticks` is the bound that protects
    /// against that, and remaining work stays observable through
    /// `pending_count`/`blocked_count` rather than being assumed absent.
    pub fn run_to_completion(&mut self, max_ticks: u64) -> u64 {
        let mut used = 0;
        while used < max_ticks && self.has_progressible_work() {
            self.run_tick();
            used += 1;
        }
        used
    }

    // ------------------------------------------------------------ compaction

    /// The explicit, versioned history compaction (PP-B2 — the
    /// ratified v1.8 §5.6 / D-PP-3): summarizes the first
    /// `ledger.len() - retain` execution records into the
    /// authenticated SHA-256 prefix summary and retains the last
    /// `retain` records verbatim. The logical state — scheduling,
    /// conservation, watermarks, every active structure — is
    /// untouched; only per-record history retention changes, and the
    /// spine's subsequent saves take the `spine-history/2` form.
    ///
    /// **THE AUTHENTICATED PREFIX SUMMARY**: a running SHA-256 chain
    /// over the discarded records' canonical wire bytes — the first
    /// compaction seeds from the spine-history domain tag, a repeated
    /// compaction chains from the prior summary, so successive
    /// compactions authenticate end-to-end and two independent
    /// compactions of the same history produce identical summaries.
    /// The summary is a derivation witness: it authenticates *what*
    /// was discarded; it never reconstructs it.
    ///
    /// **THE ELIGIBILITY DISCIPLINE**: this method is the spine-side
    /// mechanism — any prefix cut of a live ledger is structurally
    /// valid (the retained tail satisfies every load gate). The
    /// ratified eligibility conditions (which identities' consequence
    /// lane state may be summarized — the B4 extension) are the
    /// application compaction driver's to verify before calling this;
    /// compaction is an explicit operation, never a silent default.
    /// The CALL is the driver's acknowledgment (each invocation an
    /// explicit, between-ticks decision); the policy cadence — the
    /// automatic path — carries its own acknowledgment requirement
    /// ([`ConsequenceEligibility`], CA-07).
    ///
    /// **THE RETAINED-TAIL MOVE (CA-07's measured cost)**: the prefix
    /// is hashed first (read-only), then removed IN PLACE through
    /// [`Vec::drain`] — the retained tail memmoves down within the
    /// ledger's own buffer, `retained × size_of::<ExecutionRecord>()`
    /// bytes per invocation (the unavoidable cost of Vec prefix
    /// removal), with NO fresh allocation and the high-water capacity
    /// RETAINED (the pre-CA-07 `split_off` shape allocated a fresh
    /// tight vector per compaction and forced a reallocation on the
    /// next push past it — the drain shape keeps the sustained-load
    /// loop allocation-free across every firing; pinned by the
    /// retention tests).
    pub fn compact(&mut self, retain: u64) -> Result<HistoryKind, CompactionError> {
        let ledger_len = self.ledger.len() as u64;
        if retain > ledger_len {
            return Err(CompactionError::RetainExceedsLedger { ledger_len, retain });
        }
        let cut = ledger_len - retain;
        if cut == 0 {
            return Err(CompactionError::NothingToSummarize { ledger_len, retain });
        }
        let mut chain = match &self.compacted {
            Some(prior) => prior.prefix_summary,
            None => digest(SPINE_HISTORY_DOMAIN.as_bytes()),
        };
        for record in &self.ledger[..cut as usize] {
            let mut hasher = Sha256::new();
            hasher.update(&chain);
            hasher.update(&record_wire_bytes(record));
            chain = hasher.finish();
        }
        // The in-place prefix removal: the retained tail moves down
        // within the same buffer (no allocation, capacity retained);
        // the discarded prefix records are dropped as the drain
        // yields them.
        self.ledger.drain(..cut as usize);
        let summarized_records = match &self.compacted {
            Some(prior) => prior.summarized_records + cut,
            None => cut,
        };
        self.compacted = Some(CompactedHistory {
            summarized_records,
            prefix_summary: chain,
        });
        Ok(self.history_kind())
    }

    // ---------------------------------------------------- retention policy

    /// Configures the execution-ledger retention policy (A22-core —
    /// see [`RetentionPolicy`]) WITH the consequence-eligibility
    /// acknowledgment (CA-07): the cadence this arms is the
    /// ACKNOWLEDGED safe cut — the eligibility token is the
    /// composition boundary's explicit proof that every consequence
    /// consumer of the ledger prefix can relinquish it, required at
    /// every (re)configuration (a restored spine re-proves before
    /// its next compact; the cadence never fires on standing
    /// configuration a prior process once vouched for). Setting a
    /// policy never changes logical state — the fingerprint domains
    /// and save bytes are untouched by configuration alone; only a
    /// FIRING changes the history representation, through the same
    /// authenticated cut the explicit [`Spine::compact`] performs.
    pub fn set_retention_policy(
        &mut self,
        policy: RetentionPolicy,
        eligibility: ConsequenceEligibility,
    ) {
        self.retention = Some(policy);
        self.retention_eligibility = Some(eligibility);
    }

    /// Declares the retention bound WITHOUT the eligibility
    /// acknowledgment (CA-07's refusal arm): the checkpoint observes
    /// the threshold but REFUSES to cut — the refusal counted in
    /// [`RetentionStats::refused_ticks`], the ledger's growth past
    /// the bound exposed by [`Self::retention_lag_records`]. This is
    /// the state for a driver that knows its RAM bound but has not
    /// (or no longer) proven consequence eligibility: never a silent
    /// default, never a cut on unproven standing. Arming afterwards
    /// is a fresh [`Self::set_retention_policy`] call — the bound
    /// and the proof together.
    pub fn set_retention_bound(&mut self, policy: RetentionPolicy) {
        self.retention = Some(policy);
        // A bound change re-opens the eligibility question: a prior
        // acknowledgment vouched for cutting under the old bound's
        // cadence, so it does not survive the reconfiguration.
        self.retention_eligibility = None;
    }

    /// Removes the policy — the explicit opt-out. Future ticks never
    /// auto-compact; an already-compacted history STAYS compacted
    /// (compaction is not reversible — the discarded records are never
    /// reconstructible), and the observational counters freeze at
    /// their last values.
    pub fn clear_retention_policy(&mut self) {
        self.retention = None;
        self.retention_eligibility = None;
    }

    /// The configured policy, if any. Never serialized: a restored
    /// spine reports `None` until the driver re-configures.
    pub fn retention_policy(&self) -> Option<RetentionPolicy> {
        self.retention
    }

    /// Whether the configured policy's cadence is ARMED — the
    /// consequence-eligibility acknowledgment is present, so the
    /// checkpoint will fire when the threshold is exceeded. A
    /// declared-but-unarmed policy (see [`Self::set_retention_bound`])
    /// refuses instead, observably.
    pub fn retention_armed(&self) -> bool {
        self.retention_eligibility.is_some()
    }

    /// How far the retained ledger sits ABOVE the configured
    /// policy's resident bound (CA-07's lag observer): zero at or
    /// under the bound; the growth past it otherwise — the
    /// hysteresis band's normal float, a misconfigured chunk below
    /// the sustained rate's drift, or an unarmed policy's refused
    /// accumulation (with [`RetentionStats::refused_ticks`], the
    /// refusal made visible instead of silent). Zero with no policy
    /// configured (no bound is declared, so no lag is defined).
    pub fn retention_lag_records(&self) -> u64 {
        match self.retention {
            Some(policy) => {
                (self.ledger.len() as u64).saturating_sub(policy.resident_bound_records())
            }
            None => 0,
        }
    }

    /// Observational retention counters (never fingerprinted, never
    /// serialized; see [`RetentionStats`]).
    pub fn retention_stats(&self) -> RetentionStats {
        self.retention_stats
    }

    /// Observational housekeeping-work counters (never fingerprinted,
    /// never serialized; see [`HousekeepingStats`] — the CA-03
    /// class-separate publication of the drain's bookkeeping).
    pub fn housekeeping_stats(&self) -> HousekeepingStats {
        self.housekeeping_stats
    }

    /// THE POLICY CHECKPOINT (A22-core): the cadence's one firing
    /// site, called at the END of [`Self::run_tick`] — every drain
    /// phase has completed and the lane report is assembled, so
    /// compaction can never interrupt execution, parking, or waiter
    /// release. Fires when the retained ledger exceeds
    /// `retain + chunk` (the hysteresis band), summarizing
    /// `min(excess, chunk)` records through the existing compaction —
    /// at the gate exactly one chunk — and records the invocation in
    /// the observational counters. A no-op with no policy configured.
    ///
    /// **THE ELIGIBILITY GATE (CA-07):** firing additionally requires
    /// the consequence-eligibility acknowledgment
    /// ([`Self::retention_armed`] — the acknowledged safe cut). A
    /// declared-but-unarmed policy (see [`Self::set_retention_bound`])
    /// REFUSES here when the threshold is exceeded: the refusal
    /// counted in [`RetentionStats::refused_ticks`], the ledger's
    /// growth past the bound exposed by [`Self::retention_lag_records`]
    /// — the cut is never made on unproven standing, and the lag is
    /// never silent.
    ///
    /// The cut is a valid compaction window BY CONSTRUCTION: the gate
    /// guarantees `excess ≥ chunk ≥ 1` and `cut ≥ 1`, so the window
    /// retains ≥ `retain` ≥ 1 records and summarizes ≥ 1 — neither
    /// typed refusal of [`Self::compact`] is reachable; the `expect`
    /// documents the invariant, it never fires.
    fn apply_retention_policy(&mut self) {
        let Some(policy) = self.retention else {
            return;
        };
        let ledger_len = self.ledger.len() as u64;
        let threshold = policy.resident_bound_records();
        if ledger_len <= threshold {
            return;
        }
        if self.retention_eligibility.is_none() {
            self.retention_stats.refused_ticks += 1;
            if self.diag.would_log(Level::Info) {
                self.diag.log(
                    Level::Info,
                    format!(
                        "retention threshold exceeded at {} (retained {ledger_len} > bound \
                         {threshold}) without consequence eligibility — refusing to cut; \
                         lag {} records",
                        self.clock,
                        ledger_len - threshold
                    ),
                );
            }
            return;
        }
        let excess = ledger_len - policy.retain.get();
        let cut = excess.min(policy.chunk.get());
        let retained = ledger_len - cut;
        self.compact(retained).expect(
            "the retention policy's cut is a valid compaction window by construction: the \
             threshold gate guarantees excess ≥ chunk ≥ 1 and cut ≥ 1, so the window retains \
             ≥ retain ≥ 1 records and summarizes ≥ 1 — neither typed refusal is reachable",
        );
        self.retention_stats.compactions += 1;
        self.retention_stats.summarized_total += cut;
        self.retention_stats.last_chunk = cut;
        self.retention_stats.max_chunk = self.retention_stats.max_chunk.max(cut);
        if self.diag.would_log(Level::Info) {
            self.diag.log(
                Level::Info,
                format!(
                    "retention policy compacted {cut} records at {} (retained {retained}, \
                     cumulative summarized {})",
                    self.clock, self.retention_stats.summarized_total
                ),
            );
        }
    }

    // ------------------------------------------------------- fingerprints

    /// Fingerprint of engine-logical state (clock, budget, stats, pending
    /// queue, blocked set, ledger). Excludes diagnostics (observational)
    /// and the world — see [`Self::state_fingerprint`]. **Full-history
    /// API**: hashes the execution ledger, so a compacted instance —
    /// which does not retain it — surfaces the call loudly (see
    /// [`Self::ledger`]); the safe path is [`Self::try_engine_fingerprint`].
    pub fn engine_fingerprint(&self) -> u64 {
        self.require_full_history("engine_fingerprint()", "try_engine_fingerprint()");
        let mut fp = Fingerprinter::new();
        // Schema v2: length-prefixed strings in Fingerprinter::write_str
        // (independent audit 2026-10-19, item 4).
        fp.write_str("engine-core/spine/2");
        fp.write_tick(self.clock);
        fp.write_u64(self.next_seq);
        fp.write_u32(self.budget);
        fp.write_u64(self.stats.scheduled);
        fp.write_u64(self.stats.executed);
        fp.write_u64(self.stats.late_arrivals);
        fp.write_u64(self.stats.late_executions);
        fp.write_u64(self.stats.blocked_transitions);
        fp.write_u64(self.stats.invalid_dropped);
        fp.write_u64(self.stats.budget_exhausted_ticks);

        // Heap iteration order is unspecified — sort canonically first.
        // The two lanes merge into one (due, seq)-sorted view so the
        // Stage 0 schema hashes identical bytes for single-lane states.
        let mut queued: Vec<&Pending> = self
            .near_queue
            .iter()
            .chain(self.far_queue.iter())
            .map(|item| &item.0)
            .collect();
        queued.sort_by_key(|pending| (pending.due, pending.seq));
        fp.write_usize(queued.len());
        for pending in queued {
            fp.write_u64(pending.seq);
            fp.write_u32(u32::from(pending.event.class.0));
            fp.write_u64(pending.event.payload);
            fingerprint_schedule(&mut fp, &pending.event.schedule);
            fingerprint_deps(&mut fp, &pending.event);
        }

        fp.write_usize(self.blocked.len());
        for (seq, event) in &self.blocked {
            fp.write_u64(*seq);
            fp.write_u32(u32::from(event.class.0));
            fp.write_u64(event.payload);
            fingerprint_schedule(&mut fp, &event.schedule);
            fingerprint_deps(&mut fp, event);
        }

        fp.write_usize(self.ledger.len());
        for record in &self.ledger {
            fp.write_u64(record.id.seq);
            fp.write_u32(u32::from(record.class.0));
            fp.write_tick(record.intended_due);
            fp.write_tick(record.executed_tick);
            fp.write_u8(match record.schedule_kind {
                ScheduleKind::AtLogicalTime => 0,
                ScheduleKind::ExecutionRelative => 1,
            });
            fp.write_u8(u8::from(record.late));
        }
        fp.finish()
    }

    /// Fingerprint of engine state plus world state — the full logical
    /// state, in the E-07…E-10 double-run tradition. **Full-history
    /// API** (composes [`Self::engine_fingerprint`]): the safe path on
    /// a compacted instance is [`Self::try_state_fingerprint`].
    pub fn state_fingerprint(&self) -> u64 {
        self.require_full_history("state_fingerprint()", "try_state_fingerprint()");
        let mut fp = Fingerprinter::new();
        fp.write_str("engine-core/state/2");
        fp.write_u64(self.engine_fingerprint());
        self.world.fingerprint(&mut fp);
        fp.finish()
    }

    pub fn engine_fingerprint_hex(&self) -> String {
        format!("{:016x}", self.engine_fingerprint())
    }

    pub fn state_fingerprint_hex(&self) -> String {
        format!("{:016x}", self.state_fingerprint())
    }

    /// Stage 3 lane-aware engine fingerprint (domain
    /// "engine-core/spine-lanes/1"): the authoritative engine state
    /// **including** the near/far lane classification and the reservation.
    /// The Stage 0 domain ("engine-core/spine/2") is deliberately
    /// untouched so its historical fingerprints stay byte-identical;
    /// this domain meaningfully represents the new authoritative state —
    /// different lane configurations or classifications produce
    /// different fingerprints (reviewer clarification 2026-10-20). Like
    /// every fingerprint here, it derives only from logical state:
    /// wall-clock time, rendering cadence, and unordered iteration never
    /// reach it.
    pub fn lane_fingerprint(&self) -> u64 {
        self.require_full_history("lane_fingerprint()", "try_lane_fingerprint()");
        let mut fp = Fingerprinter::new();
        fp.write_str("engine-core/spine-lanes/1");
        fp.write_tick(self.clock);
        fp.write_u64(self.next_seq);
        fp.write_u32(self.budget);
        fp.write_u32(self.near_reserve);
        fp.write_u64(self.stats.scheduled);
        fp.write_u64(self.stats.executed);
        fp.write_u64(self.stats.late_arrivals);
        fp.write_u64(self.stats.late_executions);
        fp.write_u64(self.stats.blocked_transitions);
        fp.write_u64(self.stats.invalid_dropped);
        fp.write_u64(self.stats.budget_exhausted_ticks);

        // Each lane sorts canonically by (due, seq); the section itself
        // encodes the lane, so the classification is authoritative here.
        for (queue, field) in [
            (&self.near_queue, Field::Near),
            (&self.far_queue, Field::Far),
        ] {
            let mut lane: Vec<&Pending> = queue.iter().map(|item| &item.0).collect();
            lane.sort_by_key(|pending| (pending.due, pending.seq));
            fp.write_usize(lane.len());
            for pending in lane {
                fp.write_u64(pending.seq);
                fp.write_u8(field_byte(field));
                fp.write_u32(u32::from(pending.event.class.0));
                fp.write_u64(pending.event.payload);
                fingerprint_schedule(&mut fp, &pending.event.schedule);
                fingerprint_deps(&mut fp, &pending.event);
            }
        }

        fp.write_usize(self.blocked.len());
        for (seq, event) in &self.blocked {
            fp.write_u64(*seq);
            fp.write_u8(field_byte(event.field));
            fp.write_u32(u32::from(event.class.0));
            fp.write_u64(event.payload);
            fingerprint_schedule(&mut fp, &event.schedule);
            fingerprint_deps(&mut fp, event);
        }

        fp.write_usize(self.ledger.len());
        for record in &self.ledger {
            fp.write_u64(record.id.seq);
            fp.write_u8(field_byte(record.field));
            fp.write_u32(u32::from(record.class.0));
            fp.write_tick(record.intended_due);
            fp.write_tick(record.executed_tick);
            fp.write_u8(match record.schedule_kind {
                ScheduleKind::AtLogicalTime => 0,
                ScheduleKind::ExecutionRelative => 1,
            });
            fp.write_u8(u8::from(record.late));
        }
        fp.finish()
    }

    /// Stage 3 lane-aware state fingerprint (domain
    /// "engine-core/state-lanes/1"): the lane fingerprint plus world
    /// state. **Full-history API** (composes
    /// [`Self::lane_fingerprint`]): the safe path on a compacted
    /// instance is [`Self::try_lane_state_fingerprint`].
    pub fn lane_state_fingerprint(&self) -> u64 {
        self.require_full_history("lane_state_fingerprint()", "try_lane_state_fingerprint()");
        let mut fp = Fingerprinter::new();
        fp.write_str("engine-core/state-lanes/1");
        fp.write_u64(self.lane_fingerprint());
        self.world.fingerprint(&mut fp);
        fp.finish()
    }

    pub fn lane_fingerprint_hex(&self) -> String {
        format!("{:016x}", self.lane_fingerprint())
    }

    pub fn lane_state_fingerprint_hex(&self) -> String {
        format!("{:016x}", self.lane_state_fingerprint())
    }

    /// Stage 4 held-state fingerprint (domain "engine-core/spine-held/1"):
    /// the authoritative suspension state, canonically ordered by seq.
    /// The existing domains ("engine-core/spine/2", "spine-lanes/1")
    /// deliberately exclude the held set, so every historical baseline
    /// stays byte-identical when suspension is unused; authoritative
    /// Stage 4 comparisons compose this domain with the lane and world
    /// domains at the application level (the combined fingerprint lives
    /// in `engine-slice`, reviewer clarification C).
    pub fn held_fingerprint(&self) -> u64 {
        let mut fp = Fingerprinter::new();
        fp.write_str("engine-core/spine-held/1");
        fp.write_usize(self.held.len());
        for pending in self.held.values() {
            fp.write_u64(pending.seq);
            fp.write_u8(field_byte(pending.event.field));
            fp.write_u32(u32::from(pending.event.class.0));
            fp.write_u64(pending.event.payload);
            fingerprint_schedule(&mut fp, &pending.event.schedule);
            fingerprint_deps(&mut fp, &pending.event);
        }
        fp.finish()
    }

    pub fn held_fingerprint_hex(&self) -> String {
        format!("{:016x}", self.held_fingerprint())
    }

    // --------------------------------------------- history-safe variants

    /// The safe engine-fingerprint path: `Some` on `Full` (identical
    /// to [`Self::engine_fingerprint`]), `None` on `Compacted` —
    /// reports unavailability, never crashes, never fabricates (the
    /// v1.8 §5.6.1 `try_*` contract).
    pub fn try_engine_fingerprint(&self) -> Option<u64> {
        if self.compacted.is_some() {
            None
        } else {
            Some(self.engine_fingerprint())
        }
    }

    /// The safe lane-fingerprint path (see
    /// [`Self::try_engine_fingerprint`]).
    pub fn try_lane_fingerprint(&self) -> Option<u64> {
        if self.compacted.is_some() {
            None
        } else {
            Some(self.lane_fingerprint())
        }
    }

    /// The safe state-fingerprint path (see
    /// [`Self::try_engine_fingerprint`]).
    pub fn try_state_fingerprint(&self) -> Option<u64> {
        if self.compacted.is_some() {
            None
        } else {
            Some(self.state_fingerprint())
        }
    }

    /// The safe lane-state-fingerprint path (see
    /// [`Self::try_engine_fingerprint`]).
    pub fn try_lane_state_fingerprint(&self) -> Option<u64> {
        if self.compacted.is_some() {
            None
        } else {
            Some(self.lane_state_fingerprint())
        }
    }

    // -------------------------------------------------- compacted domains

    /// The compacted-history fingerprint (domain
    /// "engine-core/spine-compacted/1" — the ratified v1.8 §5.6.1
    /// domain): the domain tag, clock, `next_seq`, budget,
    /// near-reserve, every Stats counter, both lane queues (canonical
    /// `(due, seq)` order, the lane-aware entry encoding), blocked,
    /// held, the prefix-summary bytes, `summarized_records`,
    /// `retained_records`, the retained tail records (the lane-aware
    /// record encoding), and a SHA-256 digest of the unfinished-id
    /// index. **Comparison semantics**: two compacted instances agree
    /// iff the hashed content agrees — the same logical state AND the
    /// same compaction cut; an uncompacted instance and its compacted
    /// image never compare through this domain (their equivalence is
    /// behavioral, established by the twin-continuation protocol).
    /// **Compacted-domain API**: requires the compacted representation
    /// — a `Full` instance surfaces the call loudly (the symmetric
    /// fail-visible discipline).
    pub fn compacted_fingerprint(&self) -> u64 {
        self.require_compacted("compacted_fingerprint()");
        let Some(compacted) = &self.compacted else {
            unreachable!("require_compacted enforced the variant")
        };
        let mut fp = Fingerprinter::new();
        fp.write_str("engine-core/spine-compacted/1");
        fp.write_tick(self.clock);
        fp.write_u64(self.next_seq);
        fp.write_u32(self.budget);
        fp.write_u32(self.near_reserve);
        fp.write_u64(self.stats.scheduled);
        fp.write_u64(self.stats.executed);
        fp.write_u64(self.stats.late_arrivals);
        fp.write_u64(self.stats.late_executions);
        fp.write_u64(self.stats.blocked_transitions);
        fp.write_u64(self.stats.invalid_dropped);
        fp.write_u64(self.stats.budget_exhausted_ticks);
        for (queue, field) in [
            (&self.near_queue, Field::Near),
            (&self.far_queue, Field::Far),
        ] {
            let mut lane: Vec<&Pending> = queue.iter().map(|item| &item.0).collect();
            lane.sort_by_key(|pending| (pending.due, pending.seq));
            fp.write_usize(lane.len());
            for pending in lane {
                fp.write_u64(pending.seq);
                fp.write_u8(field_byte(field));
                fp.write_u32(u32::from(pending.event.class.0));
                fp.write_u64(pending.event.payload);
                fingerprint_schedule(&mut fp, &pending.event.schedule);
                fingerprint_deps(&mut fp, &pending.event);
            }
        }
        fp.write_usize(self.blocked.len());
        for (seq, event) in &self.blocked {
            fp.write_u64(*seq);
            fp.write_u8(field_byte(event.field));
            fp.write_u32(u32::from(event.class.0));
            fp.write_u64(event.payload);
            fingerprint_schedule(&mut fp, &event.schedule);
            fingerprint_deps(&mut fp, event);
        }
        fp.write_usize(self.held.len());
        for pending in self.held.values() {
            fp.write_u64(pending.seq);
            fp.write_u8(field_byte(pending.event.field));
            fp.write_u32(u32::from(pending.event.class.0));
            fp.write_u64(pending.event.payload);
            fingerprint_schedule(&mut fp, &pending.event.schedule);
            fingerprint_deps(&mut fp, &pending.event);
        }
        fp.write_bytes(&compacted.prefix_summary);
        fp.write_u64(compacted.summarized_records);
        fp.write_usize(self.ledger.len());
        for record in &self.ledger {
            fp.write_u64(record.id.seq);
            fp.write_u8(field_byte(record.field));
            fp.write_u32(u32::from(record.class.0));
            fp.write_tick(record.intended_due);
            fp.write_tick(record.executed_tick);
            fp.write_u8(match record.schedule_kind {
                ScheduleKind::AtLogicalTime => 0,
                ScheduleKind::ExecutionRelative => 1,
            });
            fp.write_u8(u8::from(record.late));
        }
        // The unfinished-id digest — SHA-256 over the ascending ids
        // (the complement representation's authoritative membership
        // index, hashed per the v1.8 §5.6.1 domain definition).
        let mut hasher = Sha256::new();
        for seq in &self.unfinished {
            hasher.update(&seq.to_le_bytes());
        }
        fp.write_bytes(&hasher.finish());
        fp.finish()
    }

    /// The compacted-history state fingerprint (domain
    /// "engine-core/state-compacted/1"): the compacted fingerprint
    /// plus world state.
    pub fn compacted_state_fingerprint(&self) -> u64 {
        self.require_compacted("compacted_state_fingerprint()");
        let mut fp = Fingerprinter::new();
        fp.write_str("engine-core/state-compacted/1");
        fp.write_u64(self.compacted_fingerprint());
        self.world.fingerprint(&mut fp);
        fp.finish()
    }

    pub fn compacted_fingerprint_hex(&self) -> String {
        format!("{:016x}", self.compacted_fingerprint())
    }

    pub fn compacted_state_fingerprint_hex(&self) -> String {
        format!("{:016x}", self.compacted_state_fingerprint())
    }

    /// The loud compacted-domain guard (the symmetric discipline — a
    /// `Full` instance has no compaction cut to compare through).
    fn require_compacted(&self, api: &'static str) {
        if self.compacted.is_none() {
            panic!(
                "{api} compares through the compacted-history domains, which require the \
                 spine-history/2 representation (history_kind() is Full) — compact first via \
                 Spine::compact, or compare Full instances through the existing fingerprint \
                 domains; this call is a documented contract violation surfaced loudly, \
                 never a silently misleading value"
            );
        }
    }

    // ------------------------------------------------------------ internals

    /// Drains due events from one lane, executing up to `cap` and
    /// performing at most [`housekeeping_cap`]`(cap)` units of
    /// housekeeping work (B01). Causality-blocked events park without
    /// consuming execution budget (no work was done) and keep their
    /// original `(due, seq)` priority on reentry; each park and each
    /// waiter release attempt costs one housekeeping unit. When the
    /// housekeeping allowance is exhausted with due work remaining,
    /// the drain stops and reports it — the un-drained events STAY
    /// QUEUED with their `(due, seq)` untouched, exactly like
    /// execution-budget exhaustion (late execution on a later tick,
    /// never dropped; each later retry is itself a counted unit, so
    /// the per-tick bound holds by construction). One execution and
    /// its synchronous waiter release (E-10) are atomic and may
    /// overshoot the allowance — the bound gates the next iteration,
    /// never a half-released herd: a waiter whose prerequisite
    /// executed must receive its remaining-count decrement, or the
    /// derived invariant would strand it permanently.
    fn drain_due(&mut self, field: Field, cap: u32) -> DrainOutcome {
        let mut executed = 0;
        let mut housekeeping: u64 = 0;
        let hk_cap = housekeeping_cap(cap);
        while executed < cap && housekeeping < hk_cap {
            let head = match field {
                Field::Near => self.near_queue.peek(),
                Field::Far => self.far_queue.peek(),
            };
            let Some(top) = head else { break };
            if top.0.due > self.clock {
                break;
            }
            let Reverse(pending) = match field {
                Field::Near => self.near_queue.pop(),
                Field::Far => self.far_queue.pop(),
            }
            .expect("queue head was just peeked");
            let event = pending.event;
            if !self.has_unmet_dependencies(&event) {
                // The execution's synchronous waiter release is
                // atomic with it (E-10) — the housekeeping units it
                // consumes are accounted here and may overshoot the
                // allowance; the loop bound above gates the next
                // iteration, never the release itself.
                housekeeping += self.execute_now(event);
                executed += 1;
            } else {
                // Due, but causality-blocked: parked, not dropped. Blocked
                // transitions take bookkeeping time outside the execution
                // budget. The parked event caps the safe watermark
                // until its dependencies resolve.
                self.stats.blocked_transitions += 1;
                self.housekeeping_stats.parks += 1;
                if self.diag.would_log(Level::Trace) {
                    self.diag.log(
                        Level::Trace,
                        format!(
                            "blocked {} at {} ({} unmet dependencies)",
                            event.id,
                            self.clock,
                            self.unmet_dependency_count(&event)
                        ),
                    );
                }
                let remaining = self.register_waiters_for_unmet(event.id.seq, &event);
                self.remaining_deps.insert(event.id.seq, remaining);
                self.blocked.insert(event.id.seq, event);
                // One blocked park (the pop + unmet scan +
                // registration): one housekeeping unit (B01). The
                // registration fan-out the park performs beyond the
                // one unit is published separately (CA-03 —
                // `HousekeepingStats::dependency_registrations`,
                // counted at the registration site).
                housekeeping += 1;
            }
        }
        // The backlog witness at the exhaustion break (CA-03): the
        // lane's queue length — an O(1) upper bound on the due work
        // that remained when the allowance stopped the drain (exact
        // when every queued event is due, the adversarial shape).
        if executed < cap && housekeeping >= hk_cap && self.lane_has_due_work(field) {
            let queued = match field {
                Field::Near => self.near_queue.len(),
                Field::Far => self.far_queue.len(),
            } as u64;
            self.housekeeping_stats.exhaustion_backlog_peak =
                self.housekeeping_stats.exhaustion_backlog_peak.max(queued);
        }
        DrainOutcome {
            executed,
            housekeeping,
            // The exhaustion signal, recorded at the break: the
            // allowance (not the execution cap — `executed < cap`
            // discriminates) stopped the drain while due work
            // remained queued in this phase's lane. A zero-cap phase
            // can never report exhaustion (the loop never runs).
            housekeeping_exhausted: executed < cap
                && housekeeping >= hk_cap
                && self.lane_has_due_work(field),
        }
    }

    fn lane_has_due_work(&self, field: Field) -> bool {
        match field {
            Field::Near => self
                .near_queue
                .peek()
                .is_some_and(|item| item.0.due <= self.clock),
            Field::Far => self
                .far_queue
                .peek()
                .is_some_and(|item| item.0.due <= self.clock),
        }
    }

    fn execute_now(&mut self, event: Event) -> u64 {
        let id = event.id;
        let executed_tick = self.clock;
        let due = event.schedule.due();
        let record = ExecutionRecord {
            id: event.id,
            class: event.class,
            field: event.field,
            intended_due: due,
            executed_tick,
            schedule_kind: event.schedule.kind(),
            late: executed_tick > due,
        };
        // Record execution *before* the handler runs, so follow-ups may
        // declare dependencies on the event currently executing.
        self.ledger.push(record);
        // The derived max-intended-due index (O(1) per execution — the
        // overlay queries it every frame; a ledger scan would scale
        // per-frame cost with total execution history).
        self.max_executed_index = Some(match self.max_executed_index {
            Some(max) if max >= due => max,
            _ => due,
        });
        // The event leaves the unfinished index BEFORE the handler
        // runs, so follow-ups may declare dependencies on the event
        // currently executing (the complement representation's
        // equivalent of the executed-set insert — same sequencing,
        // same semantics).
        self.unfinished.remove(&event.id.seq);
        self.stats.executed += 1;
        if executed_tick > due {
            self.stats.late_executions += 1;
            if self.diag.would_log(Level::Debug) {
                self.diag.log(
                    Level::Debug,
                    format!(
                        "late execution {id}: due {due} executed {executed_tick} ({} ticks late)",
                        executed_tick.as_u64() - due.as_u64()
                    ),
                );
            }
        } else if self.diag.would_log(Level::Trace) {
            // The guard keeps a suppressed record from allocating at all
            // (the would_log contract) — this site runs once per executed
            // event in the live game.
            self.diag
                .log(Level::Trace, format!("executed {id} at {executed_tick}"));
        }

        // The reusable handler follow-up buffer (the scratch field's
        // take/return protocol): seeded from `scratch_follow_ups` and
        // returned below with its capacity retained, so a handler
        // emitting one follow-up per wake (the adversary workload's
        // shape) costs no allocation in steady state.
        let mut follow_ups = {
            let unfinished = &self.unfinished;
            let next_seq = self.next_seq;
            let mut ctx = ExecCtx {
                executed_tick,
                unfinished,
                next_seq,
                field: event.field,
                follow_ups: std::mem::take(&mut self.scratch_follow_ups),
            };
            self.world.execute(&event, &mut ctx);
            ctx.drain()
        };
        for spec in follow_ups.drain(..) {
            if let Err(error) = self.schedule(spec) {
                // Handler follow-ups cannot return errors to their
                // producer; rejection is reported through diagnostics and
                // stats — never silently dropped (E-10 discipline).
                self.stats.invalid_dropped += 1;
                if self.diag.would_log(Level::Error) {
                    self.diag.log(
                        Level::Error,
                        format!("dropped invalid follow-up from {id}: {error:?}"),
                    );
                }
            }
        }
        self.scratch_follow_ups = follow_ups;

        // Release dependents that were waiting on this event (E-10:
        // the synchronous waiter release). The whole loop is atomic
        // with this execution — every waiter registered on the event
        // must receive its remaining-count decrement NOW, or the
        // derived invariant would strand it — so the housekeeping
        // units it consumes (one per release attempt, B01) are
        // returned for accounting and may overshoot the drain's
        // allowance; the drain loop's bound gates the NEXT iteration.
        //
        // THE ATOMIC RELEASE/ORDER CONTRACT (CA-03's decided
        // contract, recorded BEFORE any resumable cursor exists): a
        // release is ALL-OR-NOTHING per execution — the loop runs to
        // exhaustion within this call, visiting the waiter list in
        // REGISTRATION order (the `waiting_on` vector's push order,
        // itself the blocked event's `depends_on` declaration order,
        // consecutively deduplicated — deterministic), and no
        // observable state exists between its iterations. A future
        // resumable drain cursor must therefore suspend ONLY at
        // execution boundaries — never inside a release loop — and
        // the class-separate counters
        // (`HousekeepingStats::waiter_visits` against
        // `release_decrements` + `release_unblocks`) are exactly the
        // surface that proves no partial release survives any
        // interruption: visits must equal decrements + unblocks +
        // not-blocked no-ops at every boundary.
        let mut released: u64 = 0;
        if let Some(waiters) = self.waiting_on.remove(&event.id.seq) {
            self.housekeeping_stats.waiter_visits += waiters.len() as u64;
            for waiter in waiters {
                match self.try_unblock(waiter) {
                    ReleaseAttempt::Decrement => {
                        self.housekeeping_stats.release_decrements += 1;
                        released += 1;
                    }
                    ReleaseAttempt::Unblocked => {
                        self.housekeeping_stats.release_unblocks += 1;
                        released += 1;
                    }
                    ReleaseAttempt::NotBlocked => {}
                }
            }
        }
        released
    }

    /// Whether `event` still has unexecuted dependencies — the hot-path
    /// emptiness question the drain and unblock paths ask. No
    /// allocation: both callers need only this boolean (plus, for
    /// suppressed diagnostics, a count) — never a collected Vec.
    /// Delegates to the free-function form over
    /// split borrows (the A08a in-place passes share it).
    fn has_unmet_dependencies(&self, event: &Event) -> bool {
        has_unmet_dependencies_with(&self.unfinished, self.next_seq, event)
    }

    /// The number of `event`'s unexecuted dependencies — only for the
    /// diagnostics at the blocked/unblocked sites, under the
    /// `would_log` guard.
    fn unmet_dependency_count(&self, event: &Event) -> usize {
        event
            .depends_on
            .iter()
            .filter(|id| !self.is_executed_seq(id.seq))
            .count()
    }

    /// Registers `dependent` on every currently-unexecuted dependency
    /// of a newly blocked `event` (deduplicated) — the fused unmet filter + waiter
    /// registration the drain, resume, and restore paths
    /// share. Allocation-free. Delegates to the
    /// free-function form over split borrows, and returns the
    /// registration count that seeds the derived remaining-dependency
    /// counter (B01 — see [`register_waiters_for_unmet_into`]). The
    /// registration count is also published observationally (CA-03:
    /// [`HousekeepingStats::dependency_registrations`] — the park's
    /// real bookkeeping fan-out, which the one-unit park charge
    /// does not scale with).
    fn register_waiters_for_unmet(&mut self, dependent: u64, event: &Event) -> u32 {
        let unfinished = &self.unfinished;
        let next_seq = self.next_seq;
        let registered =
            register_waiters_for_unmet_into(&mut self.waiting_on, dependent, event, |seq| {
                seq < next_seq && !unfinished.contains(&seq)
            });
        self.housekeeping_stats.dependency_registrations += u64::from(registered);
        registered
    }

    /// Attempts to release one waiter of a just-executed event,
    /// returning the attempt's class (CA-03: the release accounting
    /// counted separately — the charge model stays one housekeeping
    /// unit per Decrement/Unblocked attempt, B01's granularity with
    /// the successful unblock's queue push folded into its attempt,
    /// the granularity the design's healthy-ratio derivation
    /// dictates). O(1) through the derived remaining-dependency
    /// count: the count of UNIQUE unmet dependencies at park time,
    /// decremented once per executed prerequisite, is zero exactly
    /// when a full-list revalidation of the unmet dependencies would
    /// pass — equivalent by the registration dedup (repeated
    /// dependency ids register, and count, once) and by the
    /// lifecycle rule that a waiter's registrations exist only while
    /// it is blocked and the prerequisite unexecuted.
    fn try_unblock(&mut self, seq: u64) -> ReleaseAttempt {
        let Some(remaining) = self.remaining_deps.get_mut(&seq) else {
            // Not currently blocked — a defensive no-op (the
            // lifecycle keeps waiter lists inside the blocked set).
            return ReleaseAttempt::NotBlocked;
        };
        if *remaining > 1 {
            *remaining -= 1;
            return ReleaseAttempt::Decrement;
        }
        // The completing prerequisite resolves the waiter's last
        // unmet dependency: consume the counter, then unblock. Only
        // the completed prerequisite's waiter list was drained; the
        // zero count proves every other prerequisite's list was
        // consumed by its own execution.
        self.remaining_deps.remove(&seq);
        let Some(event) = self.blocked.remove(&seq) else {
            // The derived counter resolved without a blocked entry —
            // the lifecycle invariant says unreachable; charged as an
            // attempt exactly as before (the counter WAS consumed).
            return ReleaseAttempt::Decrement;
        };
        let due = event.schedule.due();
        if self.diag.would_log(Level::Trace) {
            self.diag
                .log(Level::Trace, format!("unblocked {} (due {due})", event.id));
        }
        match event.field {
            Field::Near => self.near_queue.push(Reverse(Pending::new(event))),
            Field::Far => self.far_queue.push(Reverse(Pending::new(event))),
        }
        ReleaseAttempt::Unblocked
    }

    fn has_any_work(&self) -> bool {
        !self.near_queue.is_empty() || !self.far_queue.is_empty() || !self.blocked.is_empty()
    }

    /// True while work exists that can eventually execute: queued events
    /// — the advancing clock reaches their dues, and executing them
    /// releases blocked dependents. A blocked-only graph whose unmet
    /// dependencies resolve to held obligations can make no progress
    /// (no execution can ever occur), so `run_to_completion` stops
    /// rather than burning its tick budget on motionless time (Batch 1
    /// review item C).
    fn has_progressible_work(&self) -> bool {
        !self.near_queue.is_empty() || !self.far_queue.is_empty()
    }
}

fn event_view(event: &Event, location: ObligationLocation) -> ObligationView<'_> {
    ObligationView {
        id: event.id,
        due: event.schedule.due(),
        schedule: event.schedule,
        class: event.class,
        payload: event.payload,
        field: event.field,
        depends_on: &event.depends_on,
        location,
    }
}

fn field_byte(field: Field) -> u8 {
    match field {
        Field::Near => 0,
        Field::Far => 1,
    }
}

fn fingerprint_schedule(fp: &mut Fingerprinter, schedule: &Schedule) {
    match schedule {
        Schedule::AtLogicalTime { due } => {
            fp.write_u8(0);
            fp.write_tick(*due);
        }
        Schedule::ExecutionRelative { origin, delay } => {
            fp.write_u8(1);
            fp.write_tick(*origin);
            fp.write_u64(*delay);
        }
    }
}

fn fingerprint_deps(fp: &mut Fingerprinter, event: &Event) {
    fp.write_usize(event.depends_on.len());
    for dep in &event.depends_on {
        fp.write_u64(dep.seq);
    }
}

// ------------------------------------------------------------ save/load
//
// Stage 5 Batch 1 (`docs/STAGE5-PERSISTENCE-ARCHITECTURE-PROPOSAL.md`
// §5.1/§5.2; the frozen layout lives in `docs/STAGE5-B1-WIRE-FORMAT.md`):
// the full-fidelity reference encoding of authoritative spine state,
// fail-closed load-time integrity gates, and derived-state
// reconstruction.
//
// Authoritative on the wire: clock, `next_seq`, budget and reservation,
// every Stats counter, both lane queues (canonical `(due, seq)` order —
// heap layout is rebuild-dependent and invisible), the blocked set (seq
// order), the held set (seq order), and the full execution ledger
// (execution order). Derived — `known` (dense allocation), `executed`
// (the complement of unfinished), `waiting_on`, `held_index`,
// `max_executed_index`, `remaining_deps` (B01's derived
// remaining-dependency counts) — is rebuilt from those canonical sources
// through the same code paths the live spine uses. Observational —
// `LaneReport`, `LaneTotals`, `SuspensionStats`, `RetentionStats`,
// `Diagnostics` — is not logical state: a restored spine resumes them at
// defaults, they never affect scheduling, accounting, or fingerprints,
// and the boundary is documented in the wire spec. The retention POLICY
// (A22-core) is driver configuration in the same never-serialized class:
// a restored spine resumes with no policy until re-configured, while
// compaction the policy already performed persists in the
// `spine-history/2` save form exactly as an explicit compaction would.

/// Minimum encoded size of one obligation entry (wire-format spec
/// §maxima): seq 8 + class 4 + field 1 + schedule kind 1 + due 8 +
/// payload 8 + dependency count 8.
const ENTRY_MIN_BYTES: u64 = 38;

/// Encoded size of one execution-record entry: seq 8 + class 4 + field
/// 1 + intended due 8 + executed tick 8 + schedule kind 1 + late flag
/// 1.
const RECORD_BYTES: u64 = 31;

/// Encoded size of one dependency identity (a bare seq).
const DEP_BYTES: u64 = 8;

impl<W: SaveWorld> Spine<W> {
    /// Encodes the complete save — header, spine section, world
    /// section — into `enc`, so a save can nest inside larger canonical
    /// streams (the revision objects of later batches). The outer
    /// domain and version select the representation: `sektor7/save/1`
    /// (the full-fidelity reference) for a `Full` history,
    /// `sektor7/save/2` (`spine-history/2`) for a compacted one — the
    /// forms reject one another at load, never silently reinterpreted.
    pub fn save(&self, enc: &mut Encoder) {
        if let Some(compacted) = &self.compacted {
            enc.write_str(SAVE_DOMAIN_V2);
            enc.write_u32(SAVE_FORMAT_VERSION_V2);
            self.save_spine_history_section(enc, compacted);
        } else {
            enc.write_str(SAVE_DOMAIN);
            enc.write_u32(SAVE_FORMAT_VERSION);
            self.save_spine_section(enc);
        }
        let mut world_enc = Encoder::new();
        self.world.save(&mut world_enc);
        enc.write_str(W::SAVE_DOMAIN);
        enc.write_bytes(&world_enc.into_bytes());
    }

    /// The complete save as standalone canonical bytes.
    pub fn save_bytes(&self) -> Vec<u8> {
        let mut enc = Encoder::new();
        self.save(&mut enc);
        enc.into_bytes()
    }

    /// Restores a spine from complete save bytes (fail-closed).
    ///
    /// The outer domain tag selects the representation —
    /// `sektor7/save/1` (full fidelity) or `sektor7/save/2`
    /// (`spine-history/2`) — and the forms reject one another by
    /// domain and version, never silently reinterpreted.
    ///
    /// Every §5.2 integrity gate is enforced with a typed
    /// [`SaveError`]: domain/schema match; the dense-allocation identity
    /// (`scheduled == next_seq`); conservation (`scheduled == executed +
    /// queued + blocked + held`, and its structural form `executed ==
    /// next_seq - unfinished`); the ledger-count identity; identity
    /// uniqueness and frontier bounds; clock coherence; dependency
    /// resolvability against `[0, next_seq)`; coherent blocked state.
    /// The `spine-history/2` form adds the ratified compacted gates:
    /// the count identity (`summarized + retained == executed`); a
    /// non-empty summary; the retained tail's disjointness, clock
    /// coherence, and order; the late-count bound; and the carried
    /// `max_executed_index`'s coherence (present, at-or-above the
    /// tail's maximum, at-or-below the clock). Corrupted input is
    /// rejected, never repaired — no best-effort reconstruction of
    /// unverified history.
    ///
    /// The clock is initialized from the verified save — the one
    /// sanctioned deviation from tick-zero construction, meaningful only
    /// after every gate above has passed (an incoherent save is rejected
    /// rather than time-traveled).
    pub fn restore(bytes: &[u8], diag: Diagnostics) -> Result<Spine<W>, SaveError> {
        let mut dec = Decoder::new(bytes);
        // The representation dispatch: the outer domain tag selects
        // full fidelity or the compacted form; each form's version and
        // spine-section domain are then checked exactly.
        let compacted_form = {
            let observed = dec.read_str(MAX_TAG_LEN, "domain-tag")?;
            match observed {
                SAVE_DOMAIN => false,
                SAVE_DOMAIN_V2 => true,
                other => {
                    return Err(SaveError::Domain {
                        expected: SAVE_DOMAIN,
                        observed: other.to_owned(),
                    });
                }
            }
        };
        let supported = if compacted_form {
            SAVE_FORMAT_VERSION_V2
        } else {
            SAVE_FORMAT_VERSION
        };
        let version = dec.read_u32()?;
        if version != supported {
            return Err(SaveError::Version {
                observed: version,
                supported,
            });
        }
        if compacted_form {
            dec.read_domain(SPINE_HISTORY_DOMAIN)?;
        } else {
            dec.read_domain(SPINE_DOMAIN)?;
        }
        let clock = dec.read_tick()?;
        let next_seq = dec.read_u64()?;
        let budget = dec.read_u32()?;
        let near_reserve = dec.read_u32()?;
        let stats = Stats {
            scheduled: dec.read_u64()?,
            executed: dec.read_u64()?,
            late_arrivals: dec.read_u64()?,
            late_executions: dec.read_u64()?,
            blocked_transitions: dec.read_u64()?,
            invalid_dropped: dec.read_u64()?,
            budget_exhausted_ticks: dec.read_u64()?,
        };

        // Configuration bounds. The live constructors normalize
        // (`budget >= 1`, `near_reserve <= budget`); a save holding
        // unnormalized values is corrupt, not quietly re-normalized.
        if budget == 0 {
            return Err(SaveError::Integrity("spine-save/budget-zero"));
        }
        if near_reserve > budget {
            return Err(SaveError::Integrity("spine-save/reserve-exceeds-budget"));
        }

        let near = read_lane(&mut dec, "spine-save/near", Field::Near)?;
        let far = read_lane(&mut dec, "spine-save/far", Field::Far)?;
        let blocked = read_seq_section(&mut dec, "spine-save/blocked")?;
        let held = read_seq_section(&mut dec, "spine-save/held")?;
        // The history payload: the full ledger, or the compacted
        // form's fields — the carried derived max, the authenticated
        // prefix summary, the cumulative summarized count, and the
        // retained tail in the full form's per-record encoding.
        let payload = if compacted_form {
            let max_executed_index = match dec.read_u8()? {
                0 => None,
                1 => Some(dec.read_tick()?),
                discriminant => {
                    return Err(SaveError::Enum {
                        section: "spine-history/max-present",
                        discriminant: u64::from(discriminant),
                    });
                }
            };
            let summary_bytes = dec.read_bytes(32, "spine-history/summary")?;
            if summary_bytes.len() != 32 {
                return Err(SaveError::Integrity("spine-history/summary-length"));
            }
            let mut prefix_summary = [0u8; 32];
            prefix_summary.copy_from_slice(summary_bytes);
            let summarized_records = dec.read_u64()?;
            let tail = read_ledger_section(&mut dec, "spine-history/tail")?;
            HistoryPayload::Compacted {
                max_executed_index,
                prefix_summary,
                summarized_records,
                tail,
            }
        } else {
            HistoryPayload::Full {
                ledger: read_ledger_section(&mut dec, "spine-save/ledger")?,
            }
        };

        // ---- integrity gates -----------------------------------------

        // Identity uniqueness across every unfinished structure, frontier
        // bounds, and dependency resolvability: dense allocation issues
        // exactly `[0, next_seq)`, so every identity lives below the
        // frontier, appears at most once anywhere, and every dependency
        // references the same range (the live `known` gate).
        let mut unfinished = BTreeSet::new();
        for event in near.iter().chain(far.iter()).chain(&blocked).chain(&held) {
            let seq = event.id.seq;
            if seq >= next_seq {
                return Err(SaveError::Integrity("spine-save/frontier"));
            }
            if !unfinished.insert(seq) {
                return Err(SaveError::Duplicate {
                    section: "spine-save/identities",
                    seq,
                });
            }
            for dep in &event.depends_on {
                if dep.seq >= next_seq {
                    return Err(SaveError::Integrity("spine-save/dependency-frontier"));
                }
            }
        }

        // Dense allocation: `scheduled` counts every issued identity.
        if stats.scheduled != next_seq {
            return Err(SaveError::Integrity("spine-save/dense-allocation"));
        }

        // Structural conservation: allocation is dense and
        // cancellation-free, so `executed == next_seq - unfinished`
        // exactly — the derivation the complement rebuild below rests
        // on, and the no-cancellation invariant in one check.
        let unfinished_count =
            u64::try_from(unfinished.len()).expect("counts are bounded by the input length");
        let executed_derived =
            next_seq
                .checked_sub(unfinished_count)
                .ok_or(SaveError::Overflow {
                    section: "spine-save/conservation",
                })?;
        if stats.executed != executed_derived {
            return Err(SaveError::Integrity("spine-save/structural-conservation"));
        }

        // The §5.2 conservation identity in its stated form, each
        // section counted once: `scheduled == executed + queued +
        // blocked + held`. (Algebraically implied by dense allocation
        // plus the structural gate above — kept as the named gate the
        // spec states, with checked arithmetic guarding the sums.)
        let queued =
            u64::try_from(near.len() + far.len()).expect("counts are bounded by the input length");
        let blocked_count =
            u64::try_from(blocked.len()).expect("counts are bounded by the input length");
        let held_count = u64::try_from(held.len()).expect("counts are bounded by the input length");
        let conserved = stats
            .executed
            .checked_add(queued)
            .and_then(|sum| sum.checked_add(blocked_count))
            .and_then(|sum| sum.checked_add(held_count))
            .ok_or(SaveError::Overflow {
                section: "spine-save/conservation",
            })?;
        if stats.scheduled != conserved {
            return Err(SaveError::Integrity("spine-save/conservation"));
        }

        // The history payload's own gates. The per-record checks are
        // shared between the forms — frontier bounds, disjointness
        // from unfinished work, per-record clock coherence with the
        // encoded lateness flag, and chronological order — applied to
        // whichever records the form retains.
        let (ledger, compacted, max_carried) = match payload {
            HistoryPayload::Full { ledger } => {
                if stats.executed != ledger.len() as u64 {
                    return Err(SaveError::Integrity("spine-save/ledger-count"));
                }
                check_record_section(&ledger, &unfinished, next_seq, clock)?;
                // The authoritative ledger reconciles the statistic
                // exactly: the live spine increments
                // `late_executions` precisely when it appends a
                // record carrying the lateness flag, so the two
                // counts can never legitimately disagree.
                if stats.late_executions != count_late(&ledger) {
                    return Err(SaveError::Integrity("spine-save/late-execution-count"));
                }
                (ledger, None, None)
            }
            HistoryPayload::Compacted {
                max_executed_index,
                prefix_summary,
                summarized_records,
                tail,
            } => {
                // An explicit compaction summarizes at least one
                // record — the full and compacted forms stay honestly
                // disjoint.
                if summarized_records == 0 {
                    return Err(SaveError::Integrity("spine-history/empty-summary"));
                }
                // The compacted count identity.
                let retained =
                    u64::try_from(tail.len()).expect("counts are bounded by the input length");
                let counted =
                    summarized_records
                        .checked_add(retained)
                        .ok_or(SaveError::Overflow {
                            section: "spine-history/count-identity",
                        })?;
                if counted != stats.executed {
                    return Err(SaveError::Integrity("spine-history/count-identity"));
                }
                check_record_section(&tail, &unfinished, next_seq, clock)?;
                // The late-count bound (equality is the full form's
                // gate — the summarized records may include late
                // ones, so the retained tail's late count is a lower
                // bound on the statistic).
                if stats.late_executions < count_late(&tail) {
                    return Err(SaveError::Integrity("spine-history/late-count-bound"));
                }
                // The carried max: required (a compacted history
                // executed at least one record), at-or-above the
                // retained tail's maximum intended due, at-or-below
                // the clock.
                let Some(max) = max_executed_index else {
                    return Err(SaveError::Integrity("spine-history/max-required"));
                };
                if let Some(tail_max) = tail.iter().map(|record| record.intended_due).max()
                    && tail_max > max
                {
                    return Err(SaveError::Integrity("spine-history/max-below-tail"));
                }
                if max > clock {
                    return Err(SaveError::Integrity("spine-history/max-clock-coherence"));
                }
                (
                    tail,
                    Some(CompactedHistory {
                        summarized_records,
                        prefix_summary,
                    }),
                    max_executed_index,
                )
            }
        };

        // Counter bounds, each structurally implied by live behavior:
        // late executions are counted per executed event, late arrivals
        // per scheduled event, blocked transitions at most once per
        // obligation, and a tick is budget-exhausted at most once.
        if stats.late_executions > stats.executed
            || stats.late_arrivals > stats.scheduled
            || stats.blocked_transitions > stats.scheduled
            || stats.budget_exhausted_ticks > clock.as_u64()
        {
            return Err(SaveError::Integrity("spine-save/counter-bounds"));
        }

        // ---- world section -------------------------------------------

        dec.read_domain(W::SAVE_DOMAIN)?;
        let world_bytes = dec.read_bytes(MAX_BYTE_LEN, "spine-save/world")?;
        let mut world_dec = Decoder::new(world_bytes);
        let world = W::restore(&mut world_dec)?;
        world_dec.finish()?;
        dec.finish()?;

        // ---- derived-state reconstruction -----------------------------

        // `known` is the dense allocation range (a bounds check);
        // `executed` is the complement of unfinished — both exact by
        // the gates above, so the unfinished index the gates built IS
        // the live membership structure (the E3 representation: no
        // per-identity sets are materialized, so a restored spine's
        // memory scales with live work, never with lifetime
        // executions).

        let mut spine = Spine {
            clock,
            next_seq,
            near_queue: BinaryHeap::new(),
            far_queue: BinaryHeap::new(),
            blocked: BTreeMap::new(),
            waiting_on: BTreeMap::new(),
            remaining_deps: HashMap::with_capacity_and_hasher(
                REMAINING_DEPS_RESERVE,
                RemainingDepsHasherBuilder,
            ),
            unfinished,
            ledger,
            compacted,
            max_executed_index: None,
            world,
            budget,
            near_reserve,
            stats,
            diag,
            last_report: LaneReport::default(),
            lane_totals: LaneTotals::default(),
            held: BTreeMap::new(),
            held_index: BTreeSet::new(),
            suspension: SuspensionStats::default(),
            retention: None,
            retention_eligibility: None,
            retention_stats: RetentionStats::default(),
            housekeeping_stats: HousekeepingStats::default(),
            scratch_follow_ups: Vec::new(),
        };

        // Queues: heap layout is rebuild-order-dependent and invisible —
        // every observable behavior (pop order, fingerprints) derives
        // from the canonical `(due, seq)` keys (the
        // `suspend_from_queue` argument).
        for event in near {
            spine.near_queue.push(Reverse(Pending::new(event)));
        }
        for event in far {
            spine.far_queue.push(Reverse(Pending::new(event)));
        }

        // Blocked obligations and their waiter registrations, through
        // the live code paths (`has_unmet_dependencies` against the
        // rebuilt executed set, `register_waiters_for_unmet` with its
        // dedup) — identical structures by construction, including
        // the derived remaining-dependency counts (B01): each entry
        // is the registration pass's unique-unmet count, the same
        // integer the live park path would have computed, so the
        // restored spine's release path decrements from the identical
        // starting point. A blocked
        // obligation without unmet dependencies is not a live state
        // (the release path would have returned it to its queue):
        // rejected, not repaired. Waiter-vec order may differ from the
        // original registration history; it is behaviorally invisible
        // (queues are `(due, seq)`-keyed and no fingerprint hashes
        // `waiting_on`) — proven so by the continuation-invisibility
        // tests.
        for event in blocked {
            let seq = event.id.seq;
            if !spine.has_unmet_dependencies(&event) {
                return Err(SaveError::Integrity(
                    "spine-save/blocked-without-unmet-deps",
                ));
            }
            let remaining = spine.register_waiters_for_unmet(seq, &event);
            spine.remaining_deps.insert(seq, remaining);
            spine.blocked.insert(seq, event);
        }

        // Held obligations and the incrementally maintained
        // `(due, seq)` index.
        for event in held {
            let pending = Pending::new(event);
            spine.held_index.insert((pending.due, pending.seq));
            spine.held.insert(pending.seq, pending);
        }

        // The derived max-intended-due index: recomputed from the
        // canonical ledger on the full path (value-identical to the
        // incrementally maintained index by construction); carried
        // from the compacted save on the spine-history/2 path (the
        // pre-cut records are summarized — the writer-computed value,
        // gated above against the retained tail and the clock).
        spine.max_executed_index = match max_carried {
            Some(carried) => Some(carried),
            None => spine.ledger.iter().map(|record| record.intended_due).max(),
        };

        Ok(spine)
    }

    fn save_spine_section(&self, enc: &mut Encoder) {
        enc.write_str(SPINE_DOMAIN);
        enc.write_tick(self.clock);
        enc.write_u64(self.next_seq);
        enc.write_u32(self.budget);
        enc.write_u32(self.near_reserve);
        enc.write_u64(self.stats.scheduled);
        enc.write_u64(self.stats.executed);
        enc.write_u64(self.stats.late_arrivals);
        enc.write_u64(self.stats.late_executions);
        enc.write_u64(self.stats.blocked_transitions);
        enc.write_u64(self.stats.invalid_dropped);
        enc.write_u64(self.stats.budget_exhausted_ticks);
        for queue in [&self.near_queue, &self.far_queue] {
            // Canonical `(due, seq)` order — heap layout is
            // rebuild-dependent and never observable.
            let mut lane: Vec<&Pending> = queue.iter().map(|item| &item.0).collect();
            lane.sort_by_key(|pending| (pending.due, pending.seq));
            enc.write_count(lane.len());
            for pending in lane {
                write_event_entry(enc, &pending.event);
            }
        }
        enc.write_count(self.blocked.len());
        for event in self.blocked.values() {
            write_event_entry(enc, event);
        }
        enc.write_count(self.held.len());
        for pending in self.held.values() {
            write_event_entry(enc, &pending.event);
        }
        enc.write_count(self.ledger.len());
        for record in &self.ledger {
            write_record_entry(enc, record);
        }
    }

    /// The compacted spine section (`spine-history/2`): the
    /// authoritative active state in the full section's exact encoding
    /// (clock through held), then the compacted-history fields — the
    /// CARRIED derived max (the pre-cut records are summarized, so the
    /// load cannot derive it from the tail alone), the authenticated
    /// prefix summary, the cumulative summarized count, and the
    /// retained tail in the full form's per-record encoding.
    fn save_spine_history_section(&self, enc: &mut Encoder, compacted: &CompactedHistory) {
        enc.write_str(SPINE_HISTORY_DOMAIN);
        enc.write_tick(self.clock);
        enc.write_u64(self.next_seq);
        enc.write_u32(self.budget);
        enc.write_u32(self.near_reserve);
        enc.write_u64(self.stats.scheduled);
        enc.write_u64(self.stats.executed);
        enc.write_u64(self.stats.late_arrivals);
        enc.write_u64(self.stats.late_executions);
        enc.write_u64(self.stats.blocked_transitions);
        enc.write_u64(self.stats.invalid_dropped);
        enc.write_u64(self.stats.budget_exhausted_ticks);
        for queue in [&self.near_queue, &self.far_queue] {
            // Canonical `(due, seq)` order — heap layout is
            // rebuild-dependent and never observable.
            let mut lane: Vec<&Pending> = queue.iter().map(|item| &item.0).collect();
            lane.sort_by_key(|pending| (pending.due, pending.seq));
            enc.write_count(lane.len());
            for pending in lane {
                write_event_entry(enc, &pending.event);
            }
        }
        enc.write_count(self.blocked.len());
        for event in self.blocked.values() {
            write_event_entry(enc, event);
        }
        enc.write_count(self.held.len());
        for pending in self.held.values() {
            write_event_entry(enc, &pending.event);
        }
        enc.write_u8(u8::from(self.max_executed_index.is_some()));
        if let Some(max) = self.max_executed_index {
            enc.write_tick(max);
        }
        enc.write_bytes(&compacted.prefix_summary);
        enc.write_u64(compacted.summarized_records);
        enc.write_count(self.ledger.len());
        for record in &self.ledger {
            write_record_entry(enc, record);
        }
    }
}

fn write_event_entry(enc: &mut Encoder, event: &Event) {
    enc.write_u64(event.id.seq);
    enc.write_u32(u32::from(event.class.0));
    enc.write_u8(field_byte(event.field));
    match event.schedule {
        Schedule::AtLogicalTime { due } => {
            enc.write_u8(0);
            enc.write_tick(due);
        }
        Schedule::ExecutionRelative { origin, delay } => {
            enc.write_u8(1);
            enc.write_tick(origin);
            enc.write_u64(delay);
        }
    }
    enc.write_u64(event.payload);
    enc.write_count(event.depends_on.len());
    for dep in &event.depends_on {
        enc.write_u64(dep.seq);
    }
}

fn write_record_entry(enc: &mut Encoder, record: &ExecutionRecord) {
    enc.write_u64(record.id.seq);
    enc.write_u32(u32::from(record.class.0));
    enc.write_u8(field_byte(record.field));
    enc.write_tick(record.intended_due);
    enc.write_tick(record.executed_tick);
    enc.write_u8(match record.schedule_kind {
        ScheduleKind::AtLogicalTime => 0,
        ScheduleKind::ExecutionRelative => 1,
    });
    enc.write_u8(u8::from(record.late));
}

fn read_event_entry(dec: &mut Decoder, section: &'static str) -> Result<Event, SaveError> {
    let seq = dec.read_u64()?;
    let wide_class = dec.read_u32()?;
    let class = u16::try_from(wide_class).map_err(|_| SaveError::Limit {
        section,
        kind: "class",
        value: u64::from(wide_class),
        max: u64::from(u16::MAX),
    })?;
    let field = read_field(dec, section)?;
    let schedule = read_schedule(dec, section)?;
    let payload = dec.read_u64()?;
    let dep_count = dec.read_count(MAX_COLLECTION_COUNT, section, "dependency-count")?;
    if dep_count > dec.remaining() as u64 / DEP_BYTES {
        return Err(SaveError::Limit {
            section,
            kind: "dependency-count",
            value: dep_count,
            max: dec.remaining() as u64 / DEP_BYTES,
        });
    }
    let mut depends_on = Vec::with_capacity(dep_count as usize);
    for _ in 0..dep_count {
        depends_on.push(EventId {
            seq: dec.read_u64()?,
        });
    }
    Ok(Event {
        id: EventId { seq },
        class: EventClass::new(class),
        schedule,
        payload,
        field,
        depends_on,
    })
}

fn read_field(dec: &mut Decoder, section: &'static str) -> Result<Field, SaveError> {
    match dec.read_u8()? {
        0 => Ok(Field::Near),
        1 => Ok(Field::Far),
        discriminant => Err(SaveError::Enum {
            section,
            discriminant: u64::from(discriminant),
        }),
    }
}

fn read_schedule(dec: &mut Decoder, section: &'static str) -> Result<Schedule, SaveError> {
    match dec.read_u8()? {
        0 => Ok(Schedule::AtLogicalTime {
            due: dec.read_tick()?,
        }),
        1 => Ok(Schedule::ExecutionRelative {
            origin: dec.read_tick()?,
            delay: dec.read_u64()?,
        }),
        discriminant => Err(SaveError::Enum {
            section,
            discriminant: u64::from(discriminant),
        }),
    }
}

fn read_schedule_kind(dec: &mut Decoder, section: &'static str) -> Result<ScheduleKind, SaveError> {
    match dec.read_u8()? {
        0 => Ok(ScheduleKind::AtLogicalTime),
        1 => Ok(ScheduleKind::ExecutionRelative),
        discriminant => Err(SaveError::Enum {
            section,
            discriminant: u64::from(discriminant),
        }),
    }
}

fn read_late(dec: &mut Decoder, section: &'static str) -> Result<bool, SaveError> {
    match dec.read_u8()? {
        0 => Ok(false),
        1 => Ok(true),
        discriminant => Err(SaveError::Enum {
            section,
            discriminant: u64::from(discriminant),
        }),
    }
}

/// Reads a lane section: entries in canonical `(due, seq)` order with
/// unique identities and the lane's own field on every entry — the
/// live spine routes each obligation to the queue matching its field,
/// so a serialized disagreement is not a live state and is rejected,
/// never silently re-laned. Duplicates or disorder are integrity
/// failures, never repaired.
fn read_lane(
    dec: &mut Decoder,
    section: &'static str,
    lane: Field,
) -> Result<Vec<Event>, SaveError> {
    let count = dec.read_count(MAX_COLLECTION_COUNT, section, "entry-count")?;
    if count > dec.remaining() as u64 / ENTRY_MIN_BYTES {
        return Err(SaveError::Limit {
            section,
            kind: "entry-count",
            value: count,
            max: dec.remaining() as u64 / ENTRY_MIN_BYTES,
        });
    }
    let mut events = Vec::with_capacity(count as usize);
    let mut seen = BTreeSet::new();
    let mut prev: Option<(u64, u64)> = None;
    for _ in 0..count {
        let event = read_event_entry(dec, section)?;
        if event.field != lane {
            return Err(SaveError::Integrity("spine-save/lane-field-coherence"));
        }
        let key = (event.schedule.due().as_u64(), event.id.seq);
        if let Some(prior) = prev
            && key <= prior
        {
            return Err(SaveError::Order { section });
        }
        if !seen.insert(event.id.seq) {
            return Err(SaveError::Duplicate {
                section,
                seq: event.id.seq,
            });
        }
        prev = Some(key);
        events.push(event);
    }
    Ok(events)
}

/// Reads a seq-ordered section (blocked or held): strictly ascending
/// identities — the BTreeMap canonical order.
fn read_seq_section(dec: &mut Decoder, section: &'static str) -> Result<Vec<Event>, SaveError> {
    let count = dec.read_count(MAX_COLLECTION_COUNT, section, "entry-count")?;
    if count > dec.remaining() as u64 / ENTRY_MIN_BYTES {
        return Err(SaveError::Limit {
            section,
            kind: "entry-count",
            value: count,
            max: dec.remaining() as u64 / ENTRY_MIN_BYTES,
        });
    }
    let mut events = Vec::with_capacity(count as usize);
    let mut prev_seq: Option<u64> = None;
    for _ in 0..count {
        let event = read_event_entry(dec, section)?;
        if let Some(prior) = prev_seq {
            if event.id.seq == prior {
                return Err(SaveError::Duplicate {
                    section,
                    seq: prior,
                });
            }
            if event.id.seq < prior {
                return Err(SaveError::Order { section });
            }
        }
        prev_seq = Some(event.id.seq);
        events.push(event);
    }
    Ok(events)
}

/// Reads a ledger-form section (the full ledger, or the compacted
/// form's retained tail): the authoritative execution order (append
/// order), with unique identities.
fn read_ledger_section(
    dec: &mut Decoder,
    section: &'static str,
) -> Result<Vec<ExecutionRecord>, SaveError> {
    let count = dec.read_count(MAX_COLLECTION_COUNT, section, "entry-count")?;
    if count > dec.remaining() as u64 / RECORD_BYTES {
        return Err(SaveError::Limit {
            section,
            kind: "entry-count",
            value: count,
            max: dec.remaining() as u64 / RECORD_BYTES,
        });
    }
    let mut records = Vec::with_capacity(count as usize);
    let mut seen = BTreeSet::new();
    for _ in 0..count {
        let seq = dec.read_u64()?;
        let wide_class = dec.read_u32()?;
        let class = u16::try_from(wide_class).map_err(|_| SaveError::Limit {
            section,
            kind: "class",
            value: u64::from(wide_class),
            max: u64::from(u16::MAX),
        })?;
        let field = read_field(dec, section)?;
        let intended_due = dec.read_tick()?;
        let executed_tick = dec.read_tick()?;
        let schedule_kind = read_schedule_kind(dec, section)?;
        let late = read_late(dec, section)?;
        if !seen.insert(seq) {
            return Err(SaveError::Duplicate { section, seq });
        }
        records.push(ExecutionRecord {
            id: EventId { seq },
            class: EventClass::new(class),
            field,
            intended_due,
            executed_tick,
            schedule_kind,
            late,
        });
    }
    Ok(records)
}

/// The history payload [`Spine::restore`] read: the full ledger, or
/// the compacted form's fields.
enum HistoryPayload {
    Full {
        ledger: Vec<ExecutionRecord>,
    },
    Compacted {
        max_executed_index: Option<Tick>,
        prefix_summary: [u8; 32],
        summarized_records: u64,
        tail: Vec<ExecutionRecord>,
    },
}

/// The per-record ledger checks shared by both history forms:
/// frontier bounds, disjointness from unfinished work, per-record
/// clock coherence with the encoded lateness flag, and chronological
/// order (live execution appends within nondecreasing campaign ticks
/// — one clock advance per tick, every execution of that tick at it —
/// so a regressing `executed_tick` is not a live state).
fn check_record_section(
    records: &[ExecutionRecord],
    unfinished: &BTreeSet<u64>,
    next_seq: u64,
    clock: Tick,
) -> Result<(), SaveError> {
    let mut prev_executed_tick: Option<u64> = None;
    for record in records {
        let seq = record.id.seq;
        if seq >= next_seq {
            return Err(SaveError::Integrity("spine-save/frontier"));
        }
        if unfinished.contains(&seq) {
            return Err(SaveError::Integrity(
                "spine-save/ledger-overlaps-unfinished",
            ));
        }
        if record.intended_due > record.executed_tick
            || record.executed_tick > clock
            || record.late != (record.executed_tick > record.intended_due)
        {
            return Err(SaveError::Integrity("spine-save/clock-coherence"));
        }
        if let Some(prior) = prev_executed_tick
            && record.executed_tick.as_u64() < prior
        {
            return Err(SaveError::Integrity("spine-save/ledger-order"));
        }
        prev_executed_tick = Some(record.executed_tick.as_u64());
    }
    Ok(())
}

fn count_late(records: &[ExecutionRecord]) -> u64 {
    u64::try_from(records.iter().filter(|record| record.late).count())
        .expect("counts are bounded by the input length")
}

/// The fixed-width canonical record bytes — the exact 31 bytes
/// [`write_record_entry`] emits (pinned equal by test) — hashed into
/// the authenticated prefix-summary chain. Fixed-width by
/// construction: seq 8 + class 4 + field 1 + intended due 8 +
/// executed tick 8 + schedule kind 1 + late flag 1.
fn record_wire_bytes(record: &ExecutionRecord) -> [u8; 31] {
    let mut bytes = [0u8; 31];
    bytes[0..8].copy_from_slice(&record.id.seq.to_le_bytes());
    bytes[8..12].copy_from_slice(&u32::from(record.class.0).to_le_bytes());
    bytes[12] = field_byte(record.field);
    bytes[13..21].copy_from_slice(&record.intended_due.as_u64().to_le_bytes());
    bytes[21..29].copy_from_slice(&record.executed_tick.as_u64().to_le_bytes());
    bytes[29] = match record.schedule_kind {
        ScheduleKind::AtLogicalTime => 0,
        ScheduleKind::ExecutionRelative => 1,
    };
    bytes[30] = u8::from(record.late);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The prefix-summary chain's per-record bytes are the save
    /// encoding's per-record bytes — pinned exactly, so the summary
    /// hashes what the wire format specifies.
    #[test]
    fn record_wire_bytes_matches_the_save_encoding() {
        let record = ExecutionRecord {
            id: EventId { seq: 42 },
            class: EventClass::new(7),
            field: Field::Far,
            intended_due: Tick(5),
            executed_tick: Tick(9),
            schedule_kind: ScheduleKind::ExecutionRelative,
            late: true,
        };
        let mut enc = Encoder::new();
        write_record_entry(&mut enc, &record);
        assert_eq!(enc.into_bytes(), record_wire_bytes(&record));
    }

    // ------------------------------------------ A08a bulk-partition tests

    /// A minimal deterministic world: the bulk-partition tests never
    /// execute an event, so the handler is empty and the fingerprint
    /// is a constant tag (the module's world never contributes state
    /// differences).
    struct NoopWorld;

    impl World for NoopWorld {
        fn execute(&mut self, _event: &Event, _ctx: &mut ExecCtx) {}
        fn fingerprint(&self, fp: &mut Fingerprinter) {
            fp.write_str("engine-core/spine/bulk-partition-tests/1");
        }
    }

    /// A logical-time spec with an explicit class, lane, and payload —
    /// the suspension battery's fixture shape, local to this module.
    fn spec(class: u16, field: Field, due: u64, payload: u64) -> EventSpec {
        EventSpec::new(
            EventClass::new(class),
            Schedule::AtLogicalTime { due: Tick(due) },
            payload,
        )
        .in_field(field)
    }

    /// The canonical `(due, seq)` enumeration of every queued and
    /// blocked obligation — exactly the order `collect_matching`
    /// emits (sorted by the spine's priority key) and the order the
    /// save encoder and lane fingerprints read.
    fn canonical_keys<W: World>(spine: &Spine<W>) -> Vec<(u64, u64)> {
        spine
            .collect_matching(|_| true)
            .iter()
            .map(|view| (view.due.as_u64(), view.id.seq))
            .collect()
    }

    /// The A08a partition rebuilds each lane's heap from its own
    /// buffer (`into_vec` + heapify), so the proof these tests pin is
    /// that the CANONICAL `(due, seq)` enumeration — which save bytes,
    /// fingerprints, and `collect_matching` all derive from — is
    /// exactly the pre-partition set minus the moved entries, in the
    /// same order, for every one of the three operations, and that a
    /// full suspend/resume cycle restores it element-for-element.
    #[test]
    fn bulk_partition_preserves_the_canonical_due_seq_order() {
        let mut spine = Spine::new(NoopWorld, 100);
        for i in 0..40u64 {
            let class = u16::try_from(i % 4).expect("class fits u16");
            let field = if i % 3 == 0 { Field::Far } else { Field::Near };
            // Interleaved dues across classes and lanes, so no
            // partition shortcut (class or lane) coincides with due
            // order.
            let due = 100 + (i % 7) * 10;
            spine
                .schedule(spec(class, field, due, i))
                .expect("schedule");
        }
        let before = canonical_keys(&spine);
        assert_eq!(before.len(), 40);
        assert!(
            before.windows(2).all(|pair| pair[0] < pair[1]),
            "collect_matching emits strictly ascending (due, seq)"
        );

        // Suspend one region-owned class: the queued enumeration must
        // be the original minus exactly the held identities, order
        // untouched.
        let region = EventClass::new(2);
        assert_eq!(spine.suspend_matching(|view| view.class == region), 10);
        let held_ids: BTreeSet<u64> = spine.held_view().iter().map(|view| view.id.seq).collect();
        let expected: Vec<(u64, u64)> = before
            .iter()
            .copied()
            .filter(|&(_, seq)| !held_ids.contains(&seq))
            .collect();
        assert_eq!(canonical_keys(&spine), expected);
        assert_eq!(spine.pending_count(), 30, "held work left the lanes");

        // Resume restores the original canonical enumeration exactly.
        assert_eq!(spine.resume_matching(|view| view.class == region), 10);
        assert_eq!(canonical_keys(&spine), before);
        assert_eq!(spine.held_count(), 0);

        // Reclassification moves obligations between lanes without
        // touching the canonical order (only the lane column changes).
        // Matching events ALREADY in the target lane are left as they
        // are and not counted (the documented contract), so the moved
        // count is the class's cross-lane population only: 7 of the
        // 10 class-1 obligations sit in Near against a Far target.
        assert_eq!(
            spine.reclassify_matching(|view| view.class == EventClass::new(1), Field::Far),
            7
        );
        assert_eq!(canonical_keys(&spine), before);
        assert!(
            spine
                .collect_matching(|view| view.class == EventClass::new(1))
                .iter()
                .all(|view| view.field == Field::Far),
            "the reclassified region sits in the target lane"
        );
        assert_eq!(
            spine.pending_count(),
            40,
            "reclassification moves, never drops"
        );
    }

    /// The partition walks each lane's internal buffer, not pop
    /// order — sound only because the outcome is a function of the
    /// logical multiset alone. Two spines hold the SAME obligations
    /// with different internal layouts (one has been through
    /// parity-suspend/resume rebuild churn, which reheapifies both
    /// lanes); the same class-based suspension must then produce
    /// identical canonical state on both. This is the property that
    /// keeps the buffer-order predicate calls (every production
    /// predicate is a pure function of the view) observationally
    /// identical to the old pop-order walk.
    #[test]
    fn bulk_partition_outcomes_are_heap_layout_independent() {
        let build = || {
            let mut spine = Spine::new(NoopWorld, 100);
            for i in 0..50u64 {
                let class = u16::try_from(i % 5).expect("class fits u16");
                let field = if i % 2 == 0 { Field::Far } else { Field::Near };
                spine
                    .schedule(spec(class, field, 100 + (i % 9) * 7, i))
                    .expect("schedule");
            }
            spine
        };
        let mut churned = build();
        assert_eq!(churned.suspend_matching(|view| view.payload % 2 == 0), 25);
        assert_eq!(churned.resume_matching(|view| view.payload % 2 == 0), 25);
        let fresh = build();
        // Same logical state after the churn: the no-op cycle is
        // fingerprint-invisible (also pinned by the suspension
        // battery, here the precondition for the layout test).
        assert_eq!(
            churned.engine_fingerprint(),
            fresh.engine_fingerprint(),
            "the churn cycle is a state no-op"
        );

        let region = EventClass::new(3);
        let mut partitioned = churned;
        let mut control = fresh;
        assert_eq!(
            partitioned.suspend_matching(|view| view.class == region),
            control.suspend_matching(|view| view.class == region),
            "the same predicate moves the same count under either layout"
        );
        assert_eq!(partitioned.held_fingerprint(), control.held_fingerprint());
        assert_eq!(
            partitioned.lane_state_fingerprint(),
            control.lane_state_fingerprint()
        );
        assert_eq!(
            partitioned.engine_fingerprint(),
            control.engine_fingerprint()
        );
        assert_eq!(canonical_keys(&partitioned), canonical_keys(&control));
    }

    /// The partition's degenerate shapes: empty lanes (a zero-length
    /// `extract_if` over an empty buffer) and single-entry lanes
    /// round-trip through suspend/resume/reclassify without loss —
    /// the conservation identity holds at every step.
    #[test]
    fn bulk_partition_handles_empty_and_single_entry_lanes() {
        let mut spine = Spine::new(NoopWorld, 10);
        assert_eq!(spine.suspend_matching(|_| true), 0);
        assert_eq!(spine.resume_matching(|_| true), 0);
        assert_eq!(spine.reclassify_matching(|_| true, Field::Far), 0);
        assert_eq!(spine.pending_count(), 0);

        spine
            .schedule(spec(0, Field::Near, 5, 1))
            .expect("schedule");
        assert_eq!(spine.suspend_matching(|_| true), 1);
        assert_eq!(spine.pending_count(), 0, "the lone obligation is held");
        assert_eq!(spine.resume_matching(|_| true), 1);
        assert_eq!(spine.pending_count(), 1);
        assert_eq!(spine.reclassify_matching(|_| true, Field::Far), 1);
        spine.run_ticks(10);
        assert_eq!(
            spine.stats().executed,
            1,
            "the obligation survived every partition"
        );
        assert_eq!(spine.pending_count(), 0);
    }

    // ----------------------------------- IA-02 canonical-visitation tests

    /// A save-round-trip-capable no-op world: the IA-02 tests compare a
    /// LIVE spine against a save→restored copy of the same spine, so the
    /// world section must round-trip (an empty encoding — this world
    /// never contributes state differences).
    struct SaveableNoopWorld;

    impl World for SaveableNoopWorld {
        fn execute(&mut self, _event: &Event, _ctx: &mut ExecCtx) {}
        fn fingerprint(&self, fp: &mut Fingerprinter) {
            fp.write_str("engine-core/spine/ia02-tests/1");
        }
    }

    impl SaveWorld for SaveableNoopWorld {
        const SAVE_DOMAIN: &'static str = "engine-core/spine/ia02-world/1";

        fn save(&self, _enc: &mut Encoder) {}

        fn restore(_dec: &mut Decoder) -> Result<Self, SaveError> {
            Ok(SaveableNoopWorld)
        }
    }

    /// The IA-02 three-event fixture: near-lane obligations due
    /// `[1, 3, 2]` in that insertion order. The live heap's internal
    /// buffer then reads `[1, 3, 2]`, while the save encoder writes
    /// lanes in canonical `(due, seq)` order — so a save→restored copy
    /// of the SAME logical spine rebuilds its buffer as `[1, 2, 3]`.
    /// Those two buffers are exactly the states a public `FnMut`
    /// predicate must never be able to tell apart: visitation is the
    /// canonical `(due, seq)` order on both (the pre-bulk-partition
    /// pop loop's order).
    fn ia02_fixture() -> Spine<SaveableNoopWorld> {
        let mut spine = Spine::new(SaveableNoopWorld, 100);
        for due in [1u64, 3, 2] {
            spine
                .schedule(spec(0, Field::Near, due, due))
                .expect("schedule");
        }
        spine
    }

    /// IA-02's first mandatory witness (the audit's three-event
    /// fixture): `suspend_matching`'s predicate must observe queued
    /// candidates in the canonical `(due, seq)` order the pop loop
    /// visited — never the heap's rebuild-dependent buffer order. A
    /// stateful closure accepting its second call visits `[1, 2, 3]`
    /// and holds the due-2 obligation, on a live spine AND on a
    /// save→restored copy of the same spine: restore rebuilds the
    /// lane buffer in a different order than live scheduling did,
    /// and an unchanged public call must select identically on both
    /// (the independent audit observed the selections — and the
    /// resulting saves — diverging on the unsorted bulk path).
    #[test]
    fn suspension_visits_canonically_on_live_and_restored_spines() {
        let mut live = ia02_fixture();
        let mut visited: Vec<u64> = Vec::new();
        let mut calls = 0u32;
        assert_eq!(
            live.suspend_matching(|view| {
                visited.push(view.due.as_u64());
                calls += 1;
                calls == 2
            }),
            1,
            "the stateful predicate accepts exactly its second call"
        );
        assert_eq!(
            visited,
            vec![1, 2, 3],
            "queued candidates are visited in canonical (due, seq) order"
        );
        assert_eq!(
            live.held_view()
                .iter()
                .map(|view| view.payload)
                .collect::<Vec<_>>(),
            vec![2],
            "the second canonical visit is the due-2 obligation"
        );

        // The decisive save→restore half: the same spine saved before
        // suspension, restored from those bytes, identical closure.
        let pristine = ia02_fixture();
        let bytes = pristine.save_bytes();
        let mut restored: Spine<SaveableNoopWorld> =
            Spine::restore(&bytes, Diagnostics::new(Level::Off)).expect("restore");
        assert_eq!(
            restored.save_bytes(),
            bytes,
            "precondition: the two spines initially serialize identically"
        );
        let mut restored_visited: Vec<u64> = Vec::new();
        let mut restored_calls = 0u32;
        assert_eq!(
            restored.suspend_matching(|view| {
                restored_visited.push(view.due.as_u64());
                restored_calls += 1;
                restored_calls == 2
            }),
            1,
            "the stateful predicate accepts exactly its second call"
        );
        assert_eq!(
            restored_visited,
            vec![1, 2, 3],
            "the restored spine's visitation is canonical too"
        );
        assert_eq!(
            restored
                .held_view()
                .iter()
                .map(|view| view.payload)
                .collect::<Vec<_>>(),
            vec![2],
            "the restored spine selects identically"
        );
        assert_eq!(
            live.save_bytes(),
            restored.save_bytes(),
            "identical selections leave the two spines byte-identical"
        );
    }

    /// IA-02's second mandatory witness: `reclassify_matching`'s
    /// traversal carries the same canonical-visitation contract (its
    /// near-lane arm runs the same bulk partition `suspend_matching`
    /// does; the far-lane arm early-returns on target == source and
    /// the blocked arm is empty in this fixture, so the predicate
    /// observes exactly the near-lane walk). The identical stateful
    /// closure moves the due-2 obligation to the far lane on a live
    /// spine AND on a save→restored copy of the same spine.
    #[test]
    fn reclassification_visits_canonically_on_live_and_restored_spines() {
        let mut live = ia02_fixture();
        let mut visited: Vec<u64> = Vec::new();
        let mut calls = 0u32;
        assert_eq!(
            live.reclassify_matching(
                |view| {
                    visited.push(view.due.as_u64());
                    calls += 1;
                    calls == 2
                },
                Field::Far
            ),
            1,
            "the stateful predicate accepts exactly its second call"
        );
        assert_eq!(
            visited,
            vec![1, 2, 3],
            "queued candidates are visited in canonical (due, seq) order"
        );
        assert_eq!(
            live.collect_matching(|view| view.field == Field::Far)
                .iter()
                .map(|view| view.payload)
                .collect::<Vec<_>>(),
            vec![2],
            "the second canonical visit moves the due-2 obligation"
        );
        assert_eq!(live.near_pending_count(), 2);
        assert_eq!(live.far_pending_count(), 1);
        assert_eq!(
            live.pending_count(),
            3,
            "reclassification moves, never drops"
        );

        // The decisive save→restore half, identical in shape to the
        // suspension witness.
        let pristine = ia02_fixture();
        let bytes = pristine.save_bytes();
        let mut restored: Spine<SaveableNoopWorld> =
            Spine::restore(&bytes, Diagnostics::new(Level::Off)).expect("restore");
        assert_eq!(
            restored.save_bytes(),
            bytes,
            "precondition: the two spines initially serialize identically"
        );
        let mut restored_visited: Vec<u64> = Vec::new();
        let mut restored_calls = 0u32;
        assert_eq!(
            restored.reclassify_matching(
                |view| {
                    restored_visited.push(view.due.as_u64());
                    restored_calls += 1;
                    restored_calls == 2
                },
                Field::Far
            ),
            1,
            "the stateful predicate accepts exactly its second call"
        );
        assert_eq!(
            restored_visited,
            vec![1, 2, 3],
            "the restored spine's visitation is canonical too"
        );
        assert_eq!(
            restored
                .collect_matching(|view| view.field == Field::Far)
                .iter()
                .map(|view| view.payload)
                .collect::<Vec<_>>(),
            vec![2],
            "the restored spine selects identically"
        );
        assert_eq!(
            live.save_bytes(),
            restored.save_bytes(),
            "identical selections leave the two spines byte-identical"
        );
    }

    // ----------------------------------- IA-06 replacement-validation tests

    /// IA-06's mandatory witness, the audit's probe verbatim: schedule
    /// seq 0, attempt the held reconstruction with a replacement event
    /// carrying the SAME seq under a predicate that matches nothing
    /// (so the queued seq-0 obligation SURVIVES the removal), and
    /// assert the TYPED refusal — never the duplicate insert. The
    /// pre-fix form reported one insertion and the save then failed
    /// restore with `Duplicate { section: "spine-save/identities",
    /// seq: 0 }` (reproduced red-on-old before this fix landed); the
    /// refusal is pre-mutation, so the spine is exactly the spine
    /// that scheduled seq 0 and the save restores cleanly.
    #[test]
    fn reconstruct_held_refuses_a_surviving_identity_before_mutation() {
        let mut spine = Spine::new(SaveableNoopWorld, 100);
        spine
            .schedule(spec(0, Field::Near, 5, 50))
            .expect("schedule");
        let bytes_before = spine.save_bytes();
        let replacement = Event {
            id: EventId { seq: 0 },
            class: EventClass::new(0),
            schedule: Schedule::AtLogicalTime { due: Tick(9) },
            payload: 900,
            field: Field::Near,
            depends_on: Vec::new(),
        };
        assert_eq!(
            spine.try_reconstruct_held(|_| false, vec![replacement]),
            Err(ReconstructionError::SurvivingIdentity {
                id: EventId { seq: 0 }
            }),
            "the identity still belongs to the surviving queued obligation"
        );
        // Never the duplicate insert: the refusal left the spine
        // observationally exact — the obligation still queued (not
        // held), the accounting untouched, the save bytes identical.
        assert_eq!(spine.near_pending_count(), 1);
        assert_eq!(spine.held_count(), 0);
        assert_eq!(spine.stats().scheduled, 1);
        assert_eq!(spine.stats().executed, 0);
        assert_eq!(
            spine.save_bytes(),
            bytes_before,
            "the refusal is observationally a no-op"
        );
        // The save restores cleanly — the pre-fix form's save failed
        // with Duplicate { section: "spine-save/identities", seq: 0 }.
        let bytes = spine.save_bytes();
        let restored: Spine<SaveableNoopWorld> =
            Spine::restore(&bytes, Diagnostics::new(Level::Off)).expect("clean restore");
        assert_eq!(restored.save_bytes(), bytes);
    }

    /// The validation's other half — replacement identities against
    /// ONE ANOTHER: the pre-fix insertion loop keyed the held map by
    /// seq, so a repeated identity silently OVERWROTE its earlier
    /// install while the scheduled statistic counted both — a
    /// partial install the conservation identity cannot represent.
    /// The typed refusal fires before any mutation.
    #[test]
    fn reconstruct_held_refuses_a_repeated_replacement_identity() {
        let mut spine = Spine::new(SaveableNoopWorld, 100);
        spine
            .schedule(spec(0, Field::Near, 5, 50))
            .expect("schedule");
        let bytes_before = spine.save_bytes();
        let replacement = |seq: u64| Event {
            id: EventId { seq },
            class: EventClass::new(3),
            schedule: Schedule::AtLogicalTime { due: Tick(9) },
            payload: 900,
            field: Field::Near,
            depends_on: Vec::new(),
        };
        assert_eq!(
            spine.try_reconstruct_held(|_| false, vec![replacement(9001), replacement(9001)]),
            Err(ReconstructionError::DuplicateReplacement {
                id: EventId { seq: 9001 }
            }),
            "the replacement set repeats an identity"
        );
        assert_eq!(spine.held_count(), 0, "never a partial install");
        assert_eq!(
            spine.near_pending_count(),
            1,
            "the queued obligation is untouched"
        );
        assert_eq!(spine.stats().scheduled, 1);
        assert_eq!(
            spine.save_bytes(),
            bytes_before,
            "the refusal is observationally a no-op"
        );
        let bytes = spine.save_bytes();
        let restored: Spine<SaveableNoopWorld> =
            Spine::restore(&bytes, Diagnostics::new(Level::Off)).expect("clean restore");
        assert_eq!(restored.save_bytes(), bytes);
    }

    /// The direct form surfaces the conflict loudly — the
    /// `ledger()`/`try_ledger_full` discipline: an inconsistent
    /// replacement set is a documented contract violation, never a
    /// silent duplicate install. (IA-06's fixture: seq 0 scheduled,
    /// a replacement carrying the same seq, predicate matching
    /// nothing.)
    #[test]
    #[should_panic(expected = "reconstruct_held requires a replacement set whose identities")]
    fn reconstruct_held_surfaces_a_conflicting_replacement_loudly() {
        let mut spine = Spine::new(SaveableNoopWorld, 100);
        spine
            .schedule(spec(0, Field::Near, 5, 50))
            .expect("schedule");
        let replacement = Event {
            id: EventId { seq: 0 },
            class: EventClass::new(0),
            schedule: Schedule::AtLogicalTime { due: Tick(9) },
            payload: 900,
            field: Field::Near,
            depends_on: Vec::new(),
        };
        spine.reconstruct_held(|_| false, vec![replacement]);
    }

    /// The legitimate replacement the crossing's recovery performs
    /// (B4's id-exact install): the matching chains removed, the
    /// plan's obligation installed as held under the JUST-FREED
    /// identity — the validation must not refuse this, and the
    /// resulting spine must save and restore cleanly (the
    /// conservation identity holds with the delta:
    /// scheduled 2 - 1 + 1 == next_seq, executed unchanged).
    #[test]
    fn a_validated_replacement_installs_and_saves_cleanly() {
        let mut spine = Spine::new(SaveableNoopWorld, 100);
        spine
            .schedule(spec(0, Field::Near, 5, 500))
            .expect("schedule"); // seq 0, class 0 — replaced
        spine
            .schedule(spec(1, Field::Far, 6, 501))
            .expect("schedule"); // seq 1, class 1 — survives
        let replacement = Event {
            id: EventId { seq: 0 }, // the identity the removal frees
            class: EventClass::new(0),
            schedule: Schedule::AtLogicalTime { due: Tick(9) },
            payload: 900,
            field: Field::Near,
            depends_on: Vec::new(),
        };
        assert_eq!(
            spine
                .try_reconstruct_held(|view| view.class == EventClass::new(0), vec![replacement])
                .expect("the legitimate replacement installs"),
            1
        );
        assert_eq!(spine.held_count(), 1);
        assert_eq!(
            spine.far_pending_count(),
            1,
            "the class-1 obligation survives the replacement"
        );
        assert_eq!(
            spine
                .held_view()
                .iter()
                .map(|view| view.id.seq)
                .collect::<Vec<_>>(),
            vec![0],
            "the replacement is held under its own identity"
        );
        let bytes = spine.save_bytes();
        let mut restored: Spine<SaveableNoopWorld> =
            Spine::restore(&bytes, Diagnostics::new(Level::Off))
                .expect("the replaced spine saves and restores cleanly");
        assert_eq!(restored.save_bytes(), bytes);
        // The reconstructed obligation resumes under its identity and
        // executes.
        assert_eq!(restored.resume_matching(|_| true), 1);
        restored.run_to_completion(60);
        assert!(
            restored.ledger().iter().any(|record| record.id.seq == 0),
            "the reconstructed obligation executes under its own identity"
        );
    }

    // ------------------------------------- B01 accounting tests (P2 L3)

    /// A world whose handler records executed payloads — the
    /// accounting fixtures' execution-order witness (the NoopWorld
    /// shape cannot observe order).
    struct RecordingWorld {
        executed: Vec<u64>,
    }

    impl World for RecordingWorld {
        fn execute(&mut self, event: &Event, _ctx: &mut ExecCtx) {
            self.executed.push(event.payload);
        }
        fn fingerprint(&self, fp: &mut Fingerprinter) {
            fp.write_str("engine-core/spine/b01-tests/1");
            for payload in &self.executed {
                fp.write_u64(*payload);
            }
        }
    }

    /// A logical-time spec with dependencies — the accounting
    /// fixtures' fixture shape.
    fn dep_spec(class: u16, field: Field, due: u64, payload: u64, deps: Vec<EventId>) -> EventSpec {
        EventSpec::new(
            EventClass::new(class),
            Schedule::AtLogicalTime { due: Tick(due) },
            payload,
        )
        .in_field(field)
        .with_dependencies(deps)
    }

    /// The number of UNIQUE unmet dependencies of `event` — the
    /// full-list revalidation's answer, computed independently of the
    /// spine's derived counter (the equivalence checker's oracle).
    fn unique_unmet_deps<W: World>(spine: &Spine<W>, event: &Event) -> u32 {
        let mut seen: BTreeSet<u64> = BTreeSet::new();
        let mut count = 0;
        for dep in &event.depends_on {
            if !spine.is_executed_seq(dep.seq) && seen.insert(dep.seq) {
                count += 1;
            }
        }
        count
    }

    /// THE EQUIVALENCE ORACLE: the derived remaining-dependency counts
    /// equal the full-list revalidation's answer for every blocked
    /// event, and the counter map's lifecycle is exactly the blocked
    /// map's — the property that makes `try_unblock`'s O(1) check
    /// interchangeable with the O(deps) scan it replaced, at every
    /// stable point the fixtures below pause at.
    fn remaining_counts_match_full_revalidation<W: World>(spine: &Spine<W>) {
        assert_eq!(
            spine.remaining_deps.len(),
            spine.blocked.len(),
            "the derived counts live exactly on the blocked set"
        );
        for (seq, event) in &spine.blocked {
            let count = spine
                .remaining_deps
                .get(seq)
                .unwrap_or_else(|| panic!("blocked event {seq} has no derived remaining count"));
            assert_eq!(
                *count,
                unique_unmet_deps(spine, event),
                "the derived remaining count for {seq} must equal the full-list revalidation"
            );
        }
    }

    /// THE ADVERSARIAL FAN-IN PARKING STORM (B01's mass due-blocked
    /// moves): 600 due events blocked on one far-future prerequisite
    /// at budget 100 (housekeeping allowance 150 per phase). The bound
    /// rate-limits parking to the allowance — 150 parks per drain
    /// phase, 300 per tick across the guaranteed and redistribution
    /// phases, never 600 in one drain as today — with the un-parked
    /// events staying QUEUED under their original `(due, seq)` (never
    /// dropped, each later retry a counted unit), the exhaustion
    /// observable in the non-fingerprinted lane report and totals
    /// while the execution budget never binds (zero executions, zero
    /// `budget_exhausted_ticks` — the clean discrimination between
    /// the two exhaustion classes), and every event still parking
    /// exactly once across the deferral, with no deadlock.
    #[test]
    fn adversarial_parking_storm_is_housekeeping_bounded() {
        let mut spine = Spine::new(
            RecordingWorld {
                executed: Vec::new(),
            },
            100,
        );
        let prereq = spine.schedule(spec(0, Field::Near, 100, 9999)).unwrap();
        for i in 0..600u64 {
            spine
                .schedule(dep_spec(1, Field::Near, 1, i, vec![prereq]))
                .unwrap();
        }
        // Tick 1: exactly the two phases' allowances park (150 + 150);
        // the other 300 stay queued due — today's unbounded drain
        // parks all 600 in the first phase.
        spine.run_tick();
        assert_eq!(spine.blocked_count(), 150 + 150);
        assert_eq!(
            spine.pending_count(),
            301,
            "the 300 un-drained waiters stay queued (plus the not-yet-due prerequisite), never dropped"
        );
        assert_eq!(spine.stats().executed, 0);
        assert_eq!(
            spine.stats().budget_exhausted_ticks,
            0,
            "the execution budget never bound — the housekeeping class discriminates"
        );
        assert!(spine.lane_report().housekeeping_exhausted);
        assert_eq!(spine.lane_totals().housekeeping_exhausted_ticks, 1);
        remaining_counts_match_full_revalidation(&spine);
        // Tick 2: the parking completes (the final phase parks into an
        // emptying queue); the (due, seq) order was untouched, so the
        // parking proceeded in ascending seq.
        spine.run_tick();
        assert_eq!(
            spine.blocked_count(),
            600,
            "the deferral parks everyone eventually"
        );
        assert_eq!(
            spine.pending_count(),
            1,
            "only the not-yet-due prerequisite remains queued"
        );
        assert_eq!(
            spine.stats().blocked_transitions,
            600,
            "each event parked exactly once — retries park, never re-park"
        );
        assert_eq!(spine.stats().executed, 0);
        remaining_counts_match_full_revalidation(&spine);
        // Ticks 3-9: the blocked set is passive — no due queued work,
        // no housekeeping, no exhaustion signal, per-tick report reset.
        spine.run_ticks(7);
        assert_eq!(
            spine.lane_totals().housekeeping_exhausted_ticks,
            2,
            "ticks 1-2 only"
        );
        assert!(!spine.lane_report().housekeeping_exhausted);
        // The release at the prerequisite's due: one atomic storm,
        // then the deferred executions under the execution cap alone
        // (popping an executable event is the execution's own cost,
        // not housekeeping) — progress to completion, no deadlock.
        spine.run_to_completion(120);
        assert_eq!(spine.stats().executed, 601);
        let mut expected = vec![9999u64];
        expected.extend(0..600);
        assert_eq!(
            spine.world().executed,
            expected,
            "canonical (due, seq) order preserved through the deferral"
        );
        assert_eq!(spine.lane_totals().housekeeping_exhausted_ticks, 3);
    }

    /// THE ADVERSARIAL FAN-OUT RELEASE STORM (B01's thundering herd on
    /// one prerequisite): 600 parked waiters released by a single
    /// execution against a 150-unit allowance. The release itself
    /// completes ATOMICALLY (E-10 synchrony — every registered waiter
    /// receives its remaining-count decrement; a half-released herd
    /// would strand the derived invariant), the drain then stops on
    /// the exhausted allowance with the unblocked waiters still queued
    /// under their original `(due, seq)`, and the deferred waiters
    /// execute over subsequent ticks in canonical order. On a pure
    /// release storm the execution budget co-fires (a 600-storm at
    /// budget 100 fills the cap through redistribution even as each
    /// phase's drain stops) — the clean hk-vs-budget discrimination is
    /// the parking storm's zero-execution shape above.
    #[test]
    fn adversarial_release_storm_is_atomic_bounded_and_deferred() {
        let mut spine = Spine::new(
            RecordingWorld {
                executed: Vec::new(),
            },
            100,
        );
        let prereq = spine.schedule(spec(0, Field::Near, 10, 9999)).unwrap();
        for i in 0..600u64 {
            spine
                .schedule(dep_spec(1, Field::Near, 1, i, vec![prereq]))
                .unwrap();
        }
        // Ticks 1-2: the parking storm rate-limits (300 + 300); by
        // tick 2 all 600 are parked.
        spine.run_ticks(2);
        assert_eq!(spine.blocked_count(), 600);
        assert_eq!(spine.lane_totals().housekeeping_exhausted_ticks, 2);
        remaining_counts_match_full_revalidation(&spine);
        // Ticks 3-9: quiet (the blocked set is passive).
        spine.run_ticks(7);
        // Tick 10: the prerequisite pops (the only due queued work)
        // and executes; its atomic release touches all 600 waiters —
        // and executes; its atomic release touches all 600 waiters —
        // 600 housekeeping units against the 150 allowance — so the
        // guaranteed phase stops before popping a single unblocked
        // waiter; the redistribution phase (a fresh allowance) then
        // drains 99 under the execution cap. Nothing was dropped and
        // nobody stayed blocked.
        spine.run_tick();
        assert_eq!(spine.stats().executed, 1 + 99);
        assert_eq!(
            spine.blocked_count(),
            0,
            "the storm unblocked completely — the release is atomic"
        );
        assert_eq!(
            spine.pending_count(),
            501,
            "the unblocked waiters the allowance deferred stay queued (600 minus the 99 drained)"
        );
        assert!(spine.lane_report().housekeeping_exhausted);
        assert_eq!(spine.lane_totals().housekeeping_exhausted_ticks, 3);
        remaining_counts_match_full_revalidation(&spine);
        // Ticks 11-16: the deferred waiters execute under the
        // execution cap alone, in ascending (due, seq) order (the
        // last tick drains the final one).
        spine.run_ticks(6);
        assert_eq!(spine.stats().executed, 601);
        let mut expected = vec![9999u64];
        expected.extend(0..600);
        assert_eq!(spine.world().executed, expected);
        assert!(
            !spine.lane_report().housekeeping_exhausted,
            "clean ticks are not exhausted"
        );
    }

    /// THE HEALTHY RATIO NEVER TRIPS: the E-8 sustained shape — a
    /// dependency graph whose housekeeping stays near one unit per
    /// event against an execution budget the events saturate — drains
    /// exactly as before (the design's acceptance: same executions,
    /// same dues). 1000 waiters on one prerequisite at budget 1100
    /// (allowance 1650): all 1000 park in one tick, and the release
    /// plus all 1000 executions complete within the SAME tick — the
    /// pre-B01 behavior, bit for bit.
    #[test]
    fn healthy_ratio_never_trips_the_allowance() {
        let mut spine = Spine::new(
            RecordingWorld {
                executed: Vec::new(),
            },
            1100,
        );
        let prereq = spine.schedule(spec(0, Field::Near, 2, 9999)).unwrap();
        for i in 0..1000u64 {
            spine
                .schedule(dep_spec(1, Field::Near, 1, i, vec![prereq]))
                .unwrap();
        }
        spine.run_tick();
        assert_eq!(
            spine.blocked_count(),
            1000,
            "a healthy tick parks everything — no rate limit"
        );
        assert!(!spine.lane_report().housekeeping_exhausted);
        assert_eq!(spine.stats().executed, 0);
        spine.run_tick();
        assert_eq!(
            spine.stats().executed,
            1001,
            "the release and all executions complete in one tick, as before"
        );
        assert_eq!(spine.blocked_count(), 0);
        assert_eq!(spine.stats().budget_exhausted_ticks, 0);
        assert_eq!(spine.lane_totals().housekeeping_exhausted_ticks, 0);
        remaining_counts_match_full_revalidation(&spine);
    }

    /// The derived counter's edge cases (the design's list): repeated
    /// dependency ids count ONCE (the registration dedup — a naive
    /// per-list count would strand the waiter at a value its single
    /// prerequisite execution could never reach); a HELD dependency is
    /// unfinished, not executed, so it keeps the count above zero
    /// (held is not execution — the complement rule); and a dependency
    /// that executed while the waiter was still QUEUED is simply
    /// absent from the count (registration, and counting, happen only
    /// at park).
    #[test]
    fn duplicate_and_held_dependencies_count_correctly() {
        let mut spine = Spine::new(NoopWorld, 100);
        let dup_a = spine.schedule(spec(0, Field::Near, 5, 0)).unwrap();
        let dup_b = spine.schedule(spec(0, Field::Near, 6, 0)).unwrap();
        let held = spine.schedule(spec(0, Field::Near, 50, 0)).unwrap();
        let late = spine.schedule(spec(0, Field::Near, 20, 0)).unwrap();
        // Deps [X, X, Y]: the duplicate counts once — 2, not 3.
        let dup_waiter = spine
            .schedule(dep_spec(1, Field::Near, 1, 0, vec![dup_a, dup_a, dup_b]))
            .unwrap();
        // Deps [X, H] with H suspended before the waiter parks: held
        // is unmet — 2.
        let held_waiter = spine
            .schedule(dep_spec(2, Field::Near, 1, 0, vec![dup_a, held]))
            .unwrap();
        // Deps [Y, L] with Y executing at tick 6 while this waiter
        // (due 7) is still queued: only L counts at park — 1.
        let queued_waiter = spine
            .schedule(dep_spec(3, Field::Near, 7, 0, vec![dup_b, late]))
            .unwrap();
        assert_eq!(spine.suspend_matching(|view| view.id == held), 1);

        // Tick 1: the two due-1 waiters park.
        spine.run_tick();
        assert_eq!(spine.blocked_count(), 2);
        assert_eq!(spine.remaining_deps.get(&dup_waiter.seq), Some(&2));
        assert_eq!(spine.remaining_deps.get(&held_waiter.seq), Some(&2));
        remaining_counts_match_full_revalidation(&spine);

        // Tick 5: X executes — ONE decrement per waiter (X's waiter
        // list holds each waiter once, the dedup), never two for the
        // duplicated id.
        spine.run_ticks(4);
        assert_eq!(spine.remaining_deps.get(&dup_waiter.seq), Some(&1));
        assert_eq!(
            spine.remaining_deps.get(&held_waiter.seq),
            Some(&1),
            "held is not executed — the waiter stays blocked at a positive count"
        );
        assert_eq!(spine.blocked_count(), 2);
        remaining_counts_match_full_revalidation(&spine);

        // Tick 6: Y executes — the duplicate-dep waiter reaches zero
        // and unblocks (a naive count of 3 would have stranded it at
        // 1 forever: the deadlock the dedup prevents), then executes
        // in the same drain. The queued waiter is not yet due.
        spine.run_tick();
        assert_eq!(spine.remaining_deps.get(&dup_waiter.seq), None);
        assert_eq!(spine.blocked_count(), 1);
        assert_eq!(
            spine.stats().executed,
            3,
            "X, Y, and the unblocked duplicate-dep waiter"
        );

        // Tick 7: the queued waiter parks counting ONLY the still
        // unmet L (Y executed while the waiter was queued — absent
        // from the count).
        spine.run_tick();
        assert_eq!(
            spine.remaining_deps.get(&queued_waiter.seq),
            Some(&1),
            "a dependency executed while the waiter was queued is absent from the count"
        );
        remaining_counts_match_full_revalidation(&spine);

        // Tick 20: L executes — the queued waiter releases. Then
        // resume H and let it execute at its due — the held-dep waiter
        // releases through the O(1) path.
        spine.run_ticks(13);
        assert_eq!(spine.remaining_deps.get(&queued_waiter.seq), None);
        assert_eq!(spine.resume_matching(|view| view.id == held), 1);
        spine.run_ticks(30);
        assert_eq!(spine.remaining_deps.get(&held_waiter.seq), None);
        assert_eq!(spine.blocked_count(), 0);
        assert_eq!(spine.stats().executed, 7);
        assert!(spine.is_executed(held_waiter));
    }

    /// THE ONE EXISTING-BATTERY DIVERGENCE, CHARACTERIZED: the
    /// lanes.rs `shared_prerequisites_release_fanout_once_in_order`
    /// fixture's shape — 2048 waiters with duplicated prerequisite ids
    /// at budget 127 / reserve 63 — is a 2048:127 housekeeping-to-
    /// budget ratio, adversarial by the design's own definition and
    /// exactly the shape B01 exists to bound. Under the allowance the
    /// parking is rate-limited (567 of 2048 parked within two ticks —
    /// the pre-B01 battery pinned 2048 here, the one conscious test
    /// update the lane's landing requires), while the fixture's INTENT
    /// assertions all hold: the prerequisites execute at their dues,
    /// the release fires once with duplicates deduplicated, the
    /// deferred waiters execute in canonical `(due, seq)` order to
    /// completion, and suspend/resume mid-storm survives.
    #[test]
    fn release_fanout_stays_once_and_in_order_under_the_allowance() {
        let mut spine = Spine::with_reservation(
            RecordingWorld {
                executed: Vec::new(),
            },
            127,
            63,
        );
        let first = spine.schedule(spec(0, Field::Far, 2, 0)).unwrap();
        let last = spine.schedule(spec(0, Field::Far, 4, 1)).unwrap();
        for payload in 2..2050u64 {
            spine
                .schedule(dep_spec(
                    1,
                    Field::Near,
                    1,
                    payload,
                    vec![first, last, first],
                ))
                .unwrap();
        }
        spine.run_ticks(2);
        // THE RATE LIMIT (the diverged assertion): not all 2048 parked
        // within two ticks. Conservation is exact — nothing dropped
        // (2050 scheduled minus the one executed prerequisite):
        assert_eq!(spine.blocked_count() + spine.pending_count(), 2049);
        assert!(spine.blocked_count() < 2048);
        assert!(spine.lane_totals().housekeeping_exhausted_ticks >= 1);
        // The two ticks' execution semantics are unchanged:
        assert_eq!(
            spine.world().executed,
            vec![0],
            "the first prerequisite executed at its due, tick 2"
        );
        remaining_counts_match_full_revalidation(&spine);
        // The fixture's own churn: suspend and resume one waiter
        // mid-storm (the blocked arm — the derived counter drops with
        // the blocked entry and recounts at resume).
        assert_eq!(spine.suspend_matching(|v| v.payload == 27), 1);
        assert_eq!(spine.resume_matching(|v| v.payload == 27), 1);
        remaining_counts_match_full_revalidation(&spine);
        // The intent: every obligation executes exactly once, the
        // waiters in canonical order — `[first, last, first]` released
        // once, duplicates deduplicated.
        spine.run_to_completion(40);
        assert_eq!(
            spine.world().executed,
            (0..2050u64).collect::<Vec<_>>(),
            "release fan-out once, in (due, seq) order, to completion"
        );
        assert_eq!(spine.blocked_count(), 0);
        assert_eq!(spine.pending_count(), 0);
    }

    /// THE SAVE/RESTORE EQUIVALENCE: a spine with a PARTIALLY released
    /// blocked event (its derived count mid-decrement) saves, restores,
    /// and re-saves byte-identically (the counter is derived, never
    /// serialized — the wire format untouched by construction), the
    /// restored spine rebuilds the SAME integer, and the two
    /// continuations agree on every fingerprint and save byte.
    #[test]
    fn save_restore_rebuilds_remaining_counts_byte_identically() {
        let mut spine = Spine::new(SaveableNoopWorld, 100);
        let dep_early = spine.schedule(spec(0, Field::Near, 3, 0)).unwrap();
        let dep_late = spine.schedule(spec(0, Field::Near, 5, 0)).unwrap();
        let waiter = spine
            .schedule(dep_spec(1, Field::Near, 1, 0, vec![dep_early, dep_late]))
            .unwrap();
        // Tick 1: the waiter parks on both (count 2). Tick 3: the
        // early dependency executes — a PARTIAL release, count 1.
        spine.run_ticks(3);
        assert_eq!(spine.remaining_deps.get(&waiter.seq), Some(&1));
        assert_eq!(spine.blocked_count(), 1);
        remaining_counts_match_full_revalidation(&spine);
        let bytes = spine.save_bytes();
        let mut restored: Spine<SaveableNoopWorld> =
            Spine::restore(&bytes, Diagnostics::new(Level::Off)).expect("restore");
        assert_eq!(
            restored.save_bytes(),
            bytes,
            "the derived counter never reaches the wire"
        );
        assert_eq!(
            restored.remaining_deps.get(&waiter.seq),
            Some(&1),
            "the same integer rebuilt from canonical state"
        );
        remaining_counts_match_full_revalidation(&restored);
        // The continuations agree: the late dependency releases the
        // waiter on both, and the final states are indistinguishable.
        spine.run_ticks(10);
        restored.run_ticks(10);
        assert_eq!(spine.stats().executed, 3);
        assert_eq!(restored.stats().executed, 3);
        assert_eq!(spine.engine_fingerprint(), restored.engine_fingerprint());
        assert_eq!(spine.state_fingerprint(), restored.state_fingerprint());
        assert_eq!(spine.save_bytes(), restored.save_bytes());
    }

    /// THE LIFECYCLE EQUIVALENCE UNDER CHURN: across a scenario that
    /// parks, partially releases, suspends a blocked event (dropping
    /// its counter with its registrations), resumes it (re-parking
    /// with a RECOUNTED counter), and replaces an unrelated queued
    /// obligation through the held reconstruction (whose full rebuild
    /// recomputes every surviving count), the derived counts equal the
    /// full-list revalidation after every step — the O(1) check is
    /// interchangeable with the O(deps) scan it replaced — and the
    /// completing release drives the waiter home through the O(1)
    /// path.
    #[test]
    fn derived_counts_survive_suspend_resume_and_reconstruction() {
        let mut spine = Spine::new(SaveableNoopWorld, 100);
        let a = spine.schedule(spec(0, Field::Near, 30, 0)).unwrap();
        let b = spine.schedule(spec(0, Field::Near, 10, 0)).unwrap();
        let w = spine
            .schedule(dep_spec(1, Field::Near, 1, 0, vec![a, b]))
            .unwrap();
        spine.run_tick();
        assert_eq!(spine.remaining_deps.get(&w.seq), Some(&2));
        remaining_counts_match_full_revalidation(&spine);
        // Tick 10: b executes — a partial release (count 1).
        spine.run_ticks(9);
        assert_eq!(spine.remaining_deps.get(&w.seq), Some(&1));
        // Suspend the BLOCKED waiter: its counter dies with the
        // blocked entry; resume re-parks it with a fresh count against
        // the CURRENT unfinished set (only a remains unmet).
        assert_eq!(spine.suspend_matching(|view| view.id == w), 1);
        assert_eq!(
            spine.remaining_deps.get(&w.seq),
            None,
            "the counter died with the blocked entry"
        );
        assert_eq!(spine.blocked_count(), 0);
        assert_eq!(spine.resume_matching(|view| view.id == w), 1);
        assert_eq!(
            spine.remaining_deps.get(&w.seq),
            Some(&1),
            "re-park recounts: only a remains unmet"
        );
        remaining_counts_match_full_revalidation(&spine);
        // The held reconstruction's full rebuild: removing an
        // unrelated queued obligation (reinstalled as held under its
        // own identity) re-registers every surviving blocked event and
        // recomputes every derived count against the current
        // unfinished set — w's count is value-identical.
        let unrelated = spine.schedule(spec(2, Field::Near, 40, 0)).unwrap();
        let replacement = Event {
            id: unrelated,
            class: EventClass::new(2),
            schedule: Schedule::AtLogicalTime { due: Tick(40) },
            payload: 7,
            field: Field::Near,
            depends_on: Vec::new(),
        };
        assert_eq!(
            spine.try_reconstruct_held(|view| view.class == EventClass::new(2), vec![replacement]),
            Ok(1)
        );
        assert_eq!(spine.remaining_deps.get(&w.seq), Some(&1));
        remaining_counts_match_full_revalidation(&spine);
        // Tick 30: a executes — the completing release zeroes the
        // count, unblocks, and the waiter executes through the O(1)
        // path. The held replacement never runs.
        spine.run_ticks(20);
        assert_eq!(spine.remaining_deps.get(&w.seq), None);
        assert_eq!(spine.blocked_count(), 0);
        assert!(spine.is_executed(w));
        assert_eq!(
            spine.stats().executed,
            3,
            "b, a, and w — the replacement stays held"
        );
    }

    /// THE OBSERVABILITY SURFACE: the housekeeping-exhaustion signal
    /// lives in the non-fingerprinted observational class only —
    /// `LaneReport` (per-tick, reset every tick) and `LaneTotals`
    /// (cumulative) — never in fingerprinted Stats and never on the
    /// wire (restore resumes both at defaults). A zero-cap phase (the
    /// far guaranteed phase under a full near reservation) can never
    /// report exhaustion: its drain never runs, exactly as today.
    #[test]
    fn housekeeping_exhaustion_is_observational_only() {
        // Defaults on a fresh spine.
        let mut spine = Spine::with_reservation(NoopWorld, 10, 10);
        assert!(!spine.lane_report().housekeeping_exhausted);
        assert_eq!(spine.lane_totals().housekeeping_exhausted_ticks, 0);
        // Far work due under a full near reservation: the far
        // guaranteed phase has cap 0 (allowance 0 — the drain never
        // runs, the cap-0 no-op preserved), and the redistribution
        // phase that drains it is healthy — backlog is reported as
        // backlog, never as housekeeping exhaustion.
        spine.schedule(spec(0, Field::Far, 1, 0)).unwrap();
        spine.run_tick();
        assert_eq!(
            spine.stats().executed,
            1,
            "the redistribution phase drains the far work"
        );
        assert!(!spine.lane_report().housekeeping_exhausted);
        assert_eq!(spine.lane_totals().housekeeping_exhausted_ticks, 0);

        // A parking storm at budget 2 (allowance 3 per phase): three
        // parks trip the guaranteed phase, the fourth parks in the
        // redistribution phase — the tick is flagged, the next
        // (quiet) tick is not, and the cumulative total persists.
        let mut storm = Spine::new(SaveableNoopWorld, 2);
        let prereq = storm.schedule(spec(0, Field::Near, 50, 0)).unwrap();
        for _ in 0..4u64 {
            storm
                .schedule(dep_spec(1, Field::Near, 1, 0, vec![prereq]))
                .unwrap();
        }
        storm.run_tick();
        assert_eq!(
            storm.blocked_count(),
            4,
            "3 + 1 parks across the two phases"
        );
        assert!(storm.lane_report().housekeeping_exhausted);
        assert_eq!(storm.lane_totals().housekeeping_exhausted_ticks, 1);
        assert_eq!(storm.stats().budget_exhausted_ticks, 0);
        storm.run_tick();
        assert!(
            !storm.lane_report().housekeeping_exhausted,
            "the per-tick flag resets"
        );
        assert_eq!(storm.lane_totals().housekeeping_exhausted_ticks, 1);
        // The observational class never reaches a fingerprint or the
        // wire: identical logical state under different exhaustion
        // history fingerprints identically (the digest pins and the
        // determinism battery are the end-to-end witnesses).
        let bytes = storm.save_bytes();
        let restored: Spine<SaveableNoopWorld> =
            Spine::restore(&bytes, Diagnostics::new(Level::Off)).expect("restore");
        assert_eq!(
            restored.lane_totals().housekeeping_exhausted_ticks,
            0,
            "restore resumes the observational class at defaults"
        );
        assert_eq!(restored.engine_fingerprint(), storm.engine_fingerprint());
    }

    // ----------------------- CA-03 housekeeping class-counter tests (F03)

    /// THE CLASS-SEPARATE PUBLICATION (CA-03): the drain's bookkeeping,
    /// counted by class instead of the one blended number — a park's
    /// dependency-registration fan-out, a parent release's waiter-list
    /// visits, and the partial-release decrements against full
    /// unblocks. The scenario pins every counter exactly: two
    /// single-dependency near waiters, one single-dependency far
    /// waiter, and one two-dependency waiter (the partial-release
    /// arm) on two prerequisites across both lanes.
    #[test]
    fn housekeeping_counters_separate_parks_registrations_and_releases() {
        let mut spine = Spine::new(
            RecordingWorld {
                executed: Vec::new(),
            },
            100,
        );
        let p = spine
            .schedule(spec(0, Field::Near, 3, 0))
            .expect("schedule");
        let p2 = spine.schedule(spec(0, Field::Far, 4, 1)).expect("schedule");
        spine
            .schedule(dep_spec(0, Field::Near, 1, 10, vec![p]))
            .expect("schedule");
        spine
            .schedule(dep_spec(0, Field::Near, 1, 11, vec![p]))
            .expect("schedule");
        spine
            .schedule(dep_spec(0, Field::Far, 1, 12, vec![p]))
            .expect("schedule");
        spine
            .schedule(dep_spec(0, Field::Near, 1, 13, vec![p, p2]))
            .expect("schedule");
        // Five ticks: the far-lane prerequisite executes in the
        // tick-4 redistribution phase, so the waiter it completes
        // runs at tick 5 (the three-phase policy's documented shape
        // under a full near reservation).
        spine.run_ticks(5);
        assert_eq!(spine.stats().executed, 6, "every event runs");
        let counters = spine.housekeeping_stats();
        // Four blocked moves parked across the tick-1 drains (three
        // near, one far).
        assert_eq!(counters.parks, 4, "each blocked move parks once");
        // Five dependency edges: four waiters × one edge, plus the
        // two-dependency waiter's second edge — the fan-out the one
        // blended park charge never scaled with.
        assert_eq!(counters.dependency_registrations, 5);
        // Five release visits: four on the first prerequisite's
        // execution (three unblocks, one decrement for the
        // two-dependency waiter), one on the second's (the completing
        // unblock).
        assert_eq!(counters.waiter_visits, 5);
        assert_eq!(counters.release_decrements, 1, "the partial release");
        assert_eq!(counters.release_unblocks, 4);
        // The no-partial-release invariant (the resumable-cursor
        // contract's proof surface): every visit resolves to exactly
        // one counted outcome.
        assert_eq!(
            counters.waiter_visits,
            counters.release_decrements + counters.release_unblocks,
            "no release visit is lost mid-loop (the atomic-release contract)"
        );
        // Worst per-tick blended total: tick 1 parks four (4 units);
        // tick 3 executes the prerequisite whose atomic release
        // visits four waiters (4 units); tick 4's release-and-execute
        // is 2 — the worst is 4.
        assert_eq!(counters.worst_tick_housekeeping, 4);
        assert_eq!(
            counters.exhaustion_backlog_peak, 0,
            "no exhaustion in the healthy shape"
        );
    }

    /// THE PHASE-BOUNDARY PIN (CA-03's save/restore mandate): blocked
    /// work in flight, saved mid-cadence at a drain-phase boundary,
    /// restored, and continued — the continued run's canonical saves
    /// are byte-identical to the uninterrupted control's, and the
    /// RELEASE-side counters (visits, decrements, unblocks) reproduce
    /// exactly on the restored spine while the park-side work is
    /// reconstructed as restore-time registrations (the same map
    /// mutation, the same counter, a different site — the documented
    /// accounting).
    #[test]
    fn blocked_work_survives_phase_boundary_save_restore_with_identical_counters() {
        fn build() -> Spine<SaveableNoopWorld> {
            let mut spine = Spine::new(SaveableNoopWorld, 100);
            let p = spine
                .schedule(spec(0, Field::Near, 3, 0))
                .expect("schedule");
            let p2 = spine.schedule(spec(0, Field::Far, 4, 1)).expect("schedule");
            spine
                .schedule(dep_spec(0, Field::Near, 1, 10, vec![p]))
                .expect("schedule");
            spine
                .schedule(dep_spec(0, Field::Near, 1, 11, vec![p]))
                .expect("schedule");
            spine
                .schedule(dep_spec(0, Field::Far, 1, 12, vec![p]))
                .expect("schedule");
            spine
                .schedule(dep_spec(0, Field::Near, 1, 13, vec![p, p2]))
                .expect("schedule");
            spine
        }
        // The uninterrupted control.
        let mut control = build();
        control.run_ticks(5);
        // The split: one tick (all four waiters parked mid-drain),
        // save at the phase boundary, restore, continue.
        let mut split = build();
        split.run_tick();
        assert_eq!(split.blocked_count(), 4, "waiters parked at the boundary");
        let bytes = split.save_bytes();
        let mut restored: Spine<SaveableNoopWorld> =
            Spine::restore(&bytes, Diagnostics::new(Level::Off)).expect("restore");
        assert_eq!(restored.save_bytes(), bytes, "the round trip is stable");
        // …and the restore-time reconstruction registers the same
        // five edges (counted at the registration site) while every
        // park-side and release-side counter starts at zero — the
        // counters never ride the wire, only the rebuild's own work
        // is counted as it happens.
        let reconstructed = restored.housekeeping_stats();
        assert_eq!(reconstructed.parks, 0);
        assert_eq!(reconstructed.dependency_registrations, 5);
        assert_eq!(reconstructed.waiter_visits, 0);
        assert_eq!(reconstructed.release_decrements, 0);
        assert_eq!(reconstructed.release_unblocks, 0);
        assert_eq!(reconstructed.worst_tick_housekeeping, 0);
        assert_eq!(reconstructed.exhaustion_backlog_peak, 0);
        assert_eq!(restored.blocked_count(), 4, "the blocked set survived");
        // …and the restore-time reconstruction registers the same
        // five edges (counted at the registration site).
        restored.run_ticks(4);
        assert_eq!(
            restored.save_bytes(),
            control.save_bytes(),
            "the interrupted run is byte-identical to the control"
        );
        let restored_counters = restored.housekeeping_stats();
        let control_counters = control.housekeeping_stats();
        // The release-side work reproduces exactly — the same waiter
        // lists, the same decrements and unblocks, after the rebuild.
        assert_eq!(
            restored_counters.waiter_visits,
            control_counters.waiter_visits
        );
        assert_eq!(
            restored_counters.release_decrements,
            control_counters.release_decrements
        );
        assert_eq!(
            restored_counters.release_unblocks,
            control_counters.release_unblocks
        );
        // The park-side work was reconstructed, not re-parked: the
        // restored spine registered the edges at restore time and
        // parked nothing.
        assert_eq!(restored_counters.parks, 0);
        assert_eq!(
            restored_counters.dependency_registrations, control_counters.dependency_registrations,
            "the reconstruction registers the same edges"
        );
    }

    /// THE BOUNDED-STORM WITNESS (CA-03's worst-work and backlog
    /// publication): ninety single-dependency waiters against a
    /// far-due prerequisite at budget 10 — each drain phase's
    /// allowance parks fifteen (hk cap 15), the two near phases park
    /// thirty per tick, the exhaustion breaks publish the shrinking
    /// queue (the queued series 61 → 31 → 1 across the three ticks it
    /// takes to park the herd — the backlog slope made observable),
    /// and the prerequisite's eventual execution performs the
    /// documented ATOMIC OVERSHOOT: ninety release visits in one tick
    /// against the fifteen-unit phase allowance, published as the
    /// worst per-tick blended total (the E-10 contract — the release
    /// is all-or-nothing, the bound gates the NEXT iteration).
    #[test]
    fn housekeeping_publishes_worst_tick_work_and_the_exhaustion_backlog() {
        let mut spine = Spine::new(
            RecordingWorld {
                executed: Vec::new(),
            },
            10,
        );
        let p = spine
            .schedule(spec(0, Field::Near, 100, 0))
            .expect("schedule");
        for payload in 1..=90u64 {
            spine
                .schedule(dep_spec(0, Field::Near, 1, payload, vec![p]))
                .expect("schedule");
        }
        spine.run_tick();
        let tick1 = spine.housekeeping_stats();
        assert_eq!(tick1.parks, 30, "the two near phases park fifteen each");
        assert_eq!(tick1.dependency_registrations, 30);
        // The queue-length bound includes the not-yet-due prerequisite
        // (91 queued events, 15 parked): 76 — the documented O(1)
        // upper bound on the due backlog, exact up to the one
        // far-due event.
        assert_eq!(
            tick1.exhaustion_backlog_peak, 76,
            "the backlog at the first break"
        );
        assert_eq!(spine.lane_totals().housekeeping_exhausted_ticks, 1);
        assert_eq!(
            spine.near_pending_count(),
            61,
            "the backlog slope's first sample"
        );
        spine.run_tick();
        let tick2 = spine.housekeeping_stats();
        assert_eq!(tick2.parks, 60);
        assert_eq!(
            spine.near_pending_count(),
            31,
            "the slope: 31 queued after two ticks"
        );
        spine.run_tick();
        let tick3 = spine.housekeeping_stats();
        assert_eq!(tick3.parks, 90, "the whole herd parked");
        assert_eq!(
            tick3.exhaustion_backlog_peak, 76,
            "the peak stays the first break's"
        );
        assert_eq!(
            spine.near_pending_count(),
            1,
            "only the far-due prerequisite remains"
        );
        assert_eq!(spine.lane_totals().housekeeping_exhausted_ticks, 3);
        // The prerequisite executes at tick 100: the whole herd
        // releases atomically (ninety visits — the overshoot), then
        // the budget admits ten per tick.
        spine.run_to_completion(200);
        assert_eq!(spine.stats().executed, 91, "the storm drains completely");
        let final_counters = spine.housekeeping_stats();
        assert_eq!(final_counters.parks, 90);
        assert_eq!(final_counters.dependency_registrations, 90);
        assert_eq!(final_counters.waiter_visits, 90);
        assert_eq!(final_counters.release_decrements, 0);
        assert_eq!(final_counters.release_unblocks, 90);
        assert_eq!(
            final_counters.worst_tick_housekeeping, 90,
            "the atomic release overshoot published as the worst tick"
        );
        assert_eq!(
            final_counters.exhaustion_backlog_peak, 90,
            "the released herd's re-queue is the largest exhaustion backlog"
        );
    }

    // -------------------------------- A22-core retention policy tests (P2 L7)

    /// A retention policy from plain non-zero integers (the fixtures'
    /// shorthand — production callers construct the `NonZeroU64`s at
    /// their configuration boundary).
    fn policy(retain: u64, chunk: u64) -> RetentionPolicy {
        RetentionPolicy::new(
            NonZeroU64::new(retain).expect("retain is non-zero"),
            NonZeroU64::new(chunk).expect("chunk is non-zero"),
        )
    }

    /// The consequence-eligibility acknowledgment (the fixtures'
    /// shorthand — the test driver asserting the consequence
    /// consumers it controls can relinquish the prefix; production
    /// callers construct it at their own composition boundary, CA-07).
    fn eligibility() -> ConsequenceEligibility {
        ConsequenceEligibility::acknowledged()
    }

    /// THE OPT-OUT CONTRACT: no policy — the default — never
    /// auto-compacts, and the policy's PRESENCE alone (configured but
    /// never firing) changes no observable: fingerprints and save
    /// bytes stay byte-identical to a never-configured spine. A
    /// policy cleared after a firing freezes the cadence (future
    /// ticks never fire) while the already-cut history stays
    /// compacted, and a restored spine resumes with no policy and
    /// default counters — configuration is never wire state.
    #[test]
    fn retention_policy_is_opt_in_and_its_presence_changes_nothing() {
        // The default: real work, never compacted, zero counters.
        let build = || {
            let mut spine = Spine::new(SaveableNoopWorld, 50);
            for t in 1..=6u64 {
                for i in 0..10u64 {
                    spine
                        .schedule(spec(0, Field::Near, t, t * 100 + i))
                        .expect("schedule");
                }
            }
            spine
        };
        let mut plain = build();
        plain.run_ticks(6);
        assert_eq!(plain.history_kind(), HistoryKind::Full);
        assert_eq!(plain.retention_policy(), None);
        assert_eq!(plain.retention_stats(), RetentionStats::default());

        // The same scenario with a policy whose threshold the ledger
        // never reaches: identical fingerprints, identical saves —
        // configuration alone is not logical state.
        let mut configured = build();
        let fingerprint_before = configured.engine_fingerprint();
        configured.set_retention_policy(policy(10_000, 4_096), eligibility());
        assert_eq!(
            configured.engine_fingerprint(),
            fingerprint_before,
            "setting a policy touches no fingerprinted state"
        );
        configured.run_ticks(6);
        assert_eq!(configured.history_kind(), HistoryKind::Full);
        assert_eq!(configured.retention_stats(), RetentionStats::default());
        assert_eq!(configured.engine_fingerprint(), plain.engine_fingerprint());
        assert_eq!(configured.save_bytes(), plain.save_bytes());

        // Cleared after firing: the cadence freezes, the cut persists,
        // and the restored spine — carrying the compacted history —
        // resumes with no policy and default counters. Six ticks of
        // 10 executions against retain 10 / chunk 40 (threshold 50)
        // fire exactly once, at tick 6 (60 > 50, cutting 40).
        let mut frozen = build();
        frozen.set_retention_policy(policy(10, 40), eligibility());
        frozen.run_ticks(6);
        let fired = frozen.retention_stats();
        assert_eq!(fired.compactions, 1);
        assert_eq!(fired.summarized_total, 40);
        assert_eq!(fired.last_chunk, 40);
        assert_eq!(fired.max_chunk, 40);
        frozen.clear_retention_policy();
        let bytes = frozen.save_bytes();
        let mut restored: Spine<SaveableNoopWorld> =
            Spine::restore(&bytes, Diagnostics::new(Level::Off)).expect("restore");
        assert_eq!(restored.retention_policy(), None);
        assert_eq!(restored.retention_stats(), RetentionStats::default());
        // More work after the clear and the restore: neither spine
        // fires again — the restored spine carries no policy, and the
        // cleared one keeps its word. Both stay compacted (history
        // already cut); the counters freeze at the firing values.
        for t in 7..=9u64 {
            for target in [&mut frozen, &mut restored] {
                for i in 0..10u64 {
                    target
                        .schedule(spec(0, Field::Near, t, t * 100 + i))
                        .expect("schedule");
                }
            }
        }
        frozen.run_ticks(3);
        restored.run_ticks(3);
        assert_eq!(frozen.retention_stats(), fired);
        assert_eq!(restored.retention_stats(), RetentionStats::default());
        assert!(matches!(
            frozen.history_kind(),
            HistoryKind::Compacted { .. }
        ));
        assert!(matches!(
            restored.history_kind(),
            HistoryKind::Compacted { .. }
        ));
    }

    /// THE CADENCE, EXACT: 100 executions per tick against
    /// retain 10 / chunk 300 (threshold 310). The ledger sawtooths
    /// 100 → 200 → 300, fires at tick 4 (400 > 310, cutting exactly
    /// one chunk of 300), and repeats — firings at ticks 4, 7, 10 —
    /// with the retained ledger never above the hysteresis band's
    /// top, the count identity (`summarized + retained == executed`)
    /// holding at every boundary, and the no-policy twin — the same
    /// schedule, the same ticks — indistinguishable in every
    /// authoritative observable at every tick (the drain semantics
    /// are policy-invisible: the checkpoint runs at the tick
    /// boundary, after every drain phase, never mid-tick), while its
    /// FULL ledger provides the verbatim suffix the retained tail
    /// must equal.
    #[test]
    fn retention_policy_fires_at_the_documented_cadence() {
        let build = || {
            let mut spine = Spine::new(SaveableNoopWorld, 100);
            for t in 1..=10u64 {
                for i in 0..100u64 {
                    spine
                        .schedule(spec(0, Field::Near, t, t * 100 + i))
                        .expect("schedule");
                }
            }
            spine
        };
        let mut spine = build();
        spine.set_retention_policy(policy(10, 300), eligibility());
        let mut twin = build();

        // The exact per-tick shape: (retained length, cumulative
        // firings). Ticks 1-3 grow inside the band; tick 4 crosses
        // the threshold and fires; the sawtooth repeats every 3 ticks.
        let expected: [(u64, u64); 10] = [
            (100, 0),
            (200, 0),
            (300, 0),
            (100, 1),
            (200, 1),
            (300, 1),
            (100, 2),
            (200, 2),
            (300, 2),
            (100, 3),
        ];
        for (index, &(len, firings)) in expected.iter().enumerate() {
            let tick = index + 1;
            spine.run_tick();
            twin.run_tick();
            assert_eq!(
                spine.ledger_retained().len() as u64,
                len,
                "tick {tick}: the retained length"
            );
            let stats = spine.retention_stats();
            assert_eq!(stats.compactions, firings, "tick {tick}: firing count");
            if firings > 0 {
                assert_eq!(stats.last_chunk, 300, "tick {tick}: the cut is one chunk");
                assert_eq!(stats.max_chunk, 300, "tick {tick}: the largest cut");
                assert_eq!(stats.summarized_total, firings * 300);
                // The compacted count identity at every boundary.
                let HistoryKind::Compacted {
                    summarized_records,
                    retained_records,
                    ..
                } = spine.history_kind()
                else {
                    panic!("tick {tick}: a firing has compacted the spine");
                };
                assert_eq!(
                    summarized_records + retained_records,
                    spine.stats().executed,
                    "tick {tick}: count identity"
                );
                assert_eq!(retained_records, len);
                // The retained tail is the twin's verbatim suffix.
                let twin_ledger = twin.ledger();
                assert_eq!(
                    spine.ledger_retained(),
                    &twin_ledger[twin_ledger.len() - len as usize..],
                    "tick {tick}: the tail is the control's verbatim suffix"
                );
            } else {
                assert_eq!(spine.history_kind(), HistoryKind::Full);
                assert_eq!(stats, RetentionStats::default());
            }
            // The authority and the drain semantics: identical to the
            // no-policy twin — stats, queues, watermarks, the report.
            assert_eq!(spine.stats(), twin.stats(), "tick {tick}: stats");
            assert_eq!(spine.pending_count(), twin.pending_count());
            assert_eq!(spine.blocked_count(), twin.blocked_count());
            assert_eq!(spine.held_count(), twin.held_count());
            assert_eq!(
                spine.max_executed_due(),
                twin.max_executed_due(),
                "tick {tick}: the watermark's max input"
            );
            assert_eq!(
                spine.processed_through(),
                twin.processed_through(),
                "tick {tick}: the safe watermark"
            );
            assert_eq!(
                spine.lane_report(),
                twin.lane_report(),
                "tick {tick}: report"
            );
            // Conservation on both.
            for s in [&spine, &twin] {
                let stats = s.stats();
                assert_eq!(
                    stats.scheduled,
                    stats.executed
                        + s.pending_count() as u64
                        + s.blocked_count() as u64
                        + s.held_count() as u64,
                    "tick {tick}: conservation"
                );
            }
        }
        // The final shape: 1000 executed, 900 summarized, 100 retained.
        assert_eq!(spine.stats().executed, 1000);
        let HistoryKind::Compacted {
            summarized_records, ..
        } = spine.history_kind()
        else {
            panic!("the cadence compacted the spine");
        };
        assert_eq!(summarized_records, 900);
        assert_eq!(spine.ledger_retained().len(), 100);
    }

    /// THE AUTHORITY SURVIVES EVERY CUT: after the cadence has
    /// summarized 600 of 700 executed records (seven ticks), executed
    /// membership still answers for SUMMARIZED identities — the
    /// unfinished complement, never the ledger, is the authority —
    /// matching the twin's full-ledger ground truth for every
    /// allocated identity; a follow-up may depend on a summarized
    /// (already-executed) identity exactly as on the twin; unknown
    /// identities are still rejected; and identical continuations
    /// agree on stats, watermark, and the verbatim suffix at the cut.
    #[test]
    fn policy_compaction_preserves_the_identity_authority() {
        let build = || {
            let mut spine = Spine::new(SaveableNoopWorld, 100);
            for t in 1..=10u64 {
                for i in 0..100u64 {
                    spine
                        .schedule(spec(0, Field::Near, t, t * 100 + i))
                        .expect("schedule");
                }
            }
            spine
        };
        let mut spine = build();
        spine.set_retention_policy(policy(10, 300), eligibility());
        let mut twin = build();
        spine.run_ticks(7);
        twin.run_ticks(7);
        // 700 executed, 600 summarized (firings at ticks 4 and 7):
        // the complement answers for all 1000 allocated identities.
        let twin_executed: BTreeSet<u64> =
            twin.ledger().iter().map(|record| record.id.seq).collect();
        assert_eq!(twin_executed.len(), 700);
        let HistoryKind::Compacted {
            summarized_records, ..
        } = spine.history_kind()
        else {
            panic!("two firings have compacted the spine");
        };
        assert_eq!(summarized_records, 600);
        for seq in 0..1000u64 {
            assert_eq!(
                spine.is_executed(EventId { seq }),
                twin_executed.contains(&seq),
                "membership mismatch at #{seq} — summarized identities answer through the complement"
            );
        }
        // A follow-up on a SUMMARIZED identity: accepted on both
        // spines identically (the dependency gates consult
        // executed-membership, never the ledger); the unknown
        // identity is rejected on both.
        let summarized = EventId { seq: 5 };
        assert!(spine.is_executed(summarized));
        assert!(
            spine
                .schedule(dep_spec(9, Field::Near, 12, 7, vec![summarized]))
                .is_ok()
        );
        assert!(
            twin.schedule(dep_spec(9, Field::Near, 12, 7, vec![summarized]))
                .is_ok()
        );
        let unknown = EventId { seq: 5000 };
        for target in [&mut spine, &mut twin] {
            let bad = dep_spec(9, Field::Near, 12, 8, vec![unknown]);
            assert!(matches!(
                target.schedule(bad),
                Err(ScheduleError::UnknownDependency(id)) if id == unknown
            ));
        }
        // Identical continuations: the follow-up executes at its due
        // on both, the stats and watermarks agree, and the compacted
        // history is the twin's verbatim suffix at the policy's cut
        // (firings at ticks 4, 7, 10 → 900 summarized, 101 retained).
        spine.run_ticks(8);
        twin.run_ticks(8);
        assert_eq!(spine.stats(), twin.stats());
        assert_eq!(spine.processed_through(), twin.processed_through());
        assert_eq!(spine.retention_stats().compactions, 3);
        let spine_cut = match spine.history_kind() {
            HistoryKind::Compacted {
                summarized_records, ..
            } => summarized_records as usize,
            HistoryKind::Full => unreachable!("the cadence compacted the spine"),
        };
        assert_eq!(spine_cut, 900);
        assert_eq!(
            spine.ledger_retained(),
            &twin.ledger()[spine_cut..],
            "the compacted history is the twin's verbatim suffix at the cut"
        );
    }

    /// THE POLICY CHANGES WHEN COMPACTION HAPPENS, NEVER WHAT: three
    /// policy firings (3 × 300 records) and one explicit
    /// [`Spine::compact`] to the same total window produce IDENTICAL
    /// save bytes — the prefix chain is invocation-boundary-
    /// independent — and a mid-cadence save→restore with the policy
    /// re-configured reproduces the uninterrupted cadence byte for
    /// byte (the wire carries the cut; the configuration rides along
    /// out of band, and the restored counters count only the new
    /// session's firings).
    #[test]
    fn policy_cadence_equals_the_equivalent_explicit_compaction() {
        let build = || {
            let mut spine = Spine::new(SaveableNoopWorld, 100);
            for t in 1..=10u64 {
                for i in 0..100u64 {
                    spine
                        .schedule(spec(0, Field::Near, t, t * 100 + i))
                        .expect("schedule");
                }
            }
            spine
        };
        // The auto cadence: 10 ticks, firings at 4/7/10 (900
        // summarized, 100 retained).
        let mut auto = build();
        auto.set_retention_policy(policy(10, 300), eligibility());
        auto.run_ticks(10);
        assert_eq!(auto.retention_stats().compactions, 3);
        // The explicit equivalent: the same ticks, one manual cut to
        // the same window.
        let mut manual = build();
        manual.run_ticks(10);
        manual.compact(100).expect("the equivalent window compacts");
        assert_eq!(
            auto.save_bytes(),
            manual.save_bytes(),
            "3 × 300 policy firings and one explicit 900-record cut agree byte for byte"
        );

        // Mid-cadence save→restore→reconfigure→continue: identical
        // bytes to the never-interrupted cadence. The save at tick 5
        // carries one firing (300 summarized, 200 retained); the
        // restored spine fires at ticks 7 and 10 — the cadence
        // resumes exactly where the wire left it.
        let mut interrupted = build();
        interrupted.set_retention_policy(policy(10, 300), eligibility());
        interrupted.run_ticks(5);
        let bytes = interrupted.save_bytes();
        let mut restored: Spine<SaveableNoopWorld> =
            Spine::restore(&bytes, Diagnostics::new(Level::Off)).expect("restore");
        assert_eq!(
            restored.save_bytes(),
            bytes,
            "the round trip is byte-identical"
        );
        assert_eq!(
            restored.retention_policy(),
            None,
            "configuration never rides the wire"
        );
        restored.set_retention_policy(policy(10, 300), eligibility());
        restored.run_ticks(5);
        assert_eq!(restored.retention_stats().compactions, 2);
        assert_eq!(
            restored.save_bytes(),
            auto.save_bytes(),
            "the interrupted-and-resumed cadence is byte-identical to the uninterrupted one"
        );
    }

    /// THE BACKLOG DRAINS CHUNK-BOUNDED: a pre-existing 1000-record
    /// ledger under a retain-10/chunk-100 policy compacts at most 100
    /// records per tick — one bounded invocation per boundary, never
    /// a monolithic cut — converging to the band in 9 ticks and
    /// stopping (the design's "a 100k-record cut would be ~ms scale —
    /// chunk caps it" made mechanical).
    #[test]
    fn policy_backlog_drains_one_chunk_per_tick() {
        let mut spine = Spine::new(SaveableNoopWorld, 100);
        for t in 1..=10u64 {
            for i in 0..100u64 {
                spine
                    .schedule(spec(0, Field::Near, t, t * 100 + i))
                    .expect("schedule");
            }
        }
        spine.run_ticks(10);
        assert_eq!(spine.ledger_retained().len(), 1000);
        spine.set_retention_policy(policy(10, 100), eligibility());
        let mut len = 1000u64;
        for tick in 0..9u64 {
            spine.run_tick(); // no due work remains: only the checkpoint acts
            len -= 100;
            let stats = spine.retention_stats();
            assert_eq!(stats.compactions, tick + 1, "one invocation per tick");
            assert_eq!(
                stats.last_chunk, 100,
                "each invocation cuts exactly one chunk"
            );
            assert_eq!(spine.ledger_retained().len() as u64, len);
        }
        // Inside the band (100 ≤ 110): the cadence stops.
        spine.run_tick();
        assert_eq!(spine.retention_stats().compactions, 9);
        assert_eq!(spine.ledger_retained().len(), 100);
        // The count identity survived the drain.
        let HistoryKind::Compacted {
            summarized_records, ..
        } = spine.history_kind()
        else {
            panic!("the backlog drain compacted the spine");
        };
        assert_eq!(
            summarized_records + 100,
            spine.stats().executed,
            "900 summarized + 100 retained == 1000 executed"
        );
    }

    /// The sustained-load world (the E-8 sustained shape): a stateless
    /// self-feeding handler — every execution schedules exactly one
    /// follow-up due the next tick — so a seed batch of `budget` due
    /// events saturates the execution budget every tick indefinitely.
    /// Saveable with an empty encoding (the world never contributes
    /// state differences); the logical witness is the spine's own
    /// counters.
    struct SustainedWorld;

    impl World for SustainedWorld {
        fn execute(&mut self, _event: &Event, ctx: &mut ExecCtx) {
            ctx.follow_up(1, SUSTAINED_CLASS, 7, Vec::new());
        }
        fn fingerprint(&self, fp: &mut Fingerprinter) {
            fp.write_str("engine-core/spine/a22-sustained/1");
        }
    }

    impl SaveWorld for SustainedWorld {
        const SAVE_DOMAIN: &'static str = "engine-core/spine/a22-sustained-world/1";

        fn save(&self, _enc: &mut Encoder) {}

        fn restore(_dec: &mut Decoder) -> Result<Self, SaveError> {
            Ok(SustainedWorld)
        }
    }

    /// The sustained class — the seeds and the self-spawned follow-ups
    /// share it (`spec` takes the raw u16).
    const SUSTAINED_CLASS: EventClass = EventClass::new(5);

    /// BOUNDED ACTIVE RAM UNDER SUSTAINED LOAD (the re-audit's bar —
    /// engine work and resident memory proportional to the relevant
    /// workload, not historical state): 500 ticks of budget-saturated
    /// self-feeding execution — 50,000 executions against a
    /// retain 128 / chunk 300 policy (bound 428; 128 ≥ budget so the
    /// current tick's records always stay verbatim) — keep the
    /// retained ledger at or under the bound at EVERY tick boundary,
    /// conservation exact at every boundary, the count identity exact
    /// at every firing, while `stats.executed` grows 100 per tick
    /// without bound. The end-state save round-trips byte-identically
    /// in the `spine-history/2` form, the restored spine resumes with
    /// no policy (configuration never rides the wire), and the whole
    /// 500-tick sustained run is double-run deterministic: two
    /// independent runs produce identical save bytes, compacted
    /// fingerprints, and counters (the E-8 double-run-identical
    /// discipline).
    #[test]
    fn sustained_load_keeps_the_resident_ledger_at_the_bound() {
        const BUDGET: u64 = 100;
        const TICKS: u64 = 500;
        let policy_config = policy(128, 300);
        let bound = policy_config.resident_bound_records();

        let run = || {
            let mut spine = Spine::new(SustainedWorld, BUDGET as u32);
            for i in 0..BUDGET {
                spine
                    .schedule(spec(5, Field::Near, 1, i))
                    .expect("schedule");
            }
            spine.set_retention_policy(policy_config, eligibility());
            for tick in 1..=TICKS {
                spine.run_tick();
                let len = spine.ledger_retained().len() as u64;
                assert!(
                    len <= bound,
                    "tick {tick}: the retained ledger ({len}) is at or under the bound ({bound})"
                );
                // Conservation at every boundary: each execution
                // spawns exactly one successor.
                let stats = spine.stats();
                assert_eq!(
                    stats.scheduled,
                    stats.executed
                        + spine.pending_count() as u64
                        + spine.blocked_count() as u64
                        + spine.held_count() as u64,
                    "tick {tick}: conservation"
                );
                assert_eq!(stats.executed, tick * BUDGET, "tick {tick}: saturation");
                // The count identity at every firing.
                if let HistoryKind::Compacted {
                    summarized_records,
                    retained_records,
                    ..
                } = spine.history_kind()
                {
                    assert_eq!(
                        summarized_records + retained_records,
                        stats.executed,
                        "tick {tick}: count identity"
                    );
                }
                // With retain ≥ budget, the current tick's records are
                // always verbatim in the tail: the checkpoint runs at
                // the boundary, AFTER the tick's appends (sampled
                // every 50 ticks to keep the witness cheap).
                if tick % 50 == 0 {
                    let retained = spine.ledger_retained();
                    let current = &retained[retained.len() - BUDGET as usize..];
                    assert!(
                        current
                            .iter()
                            .all(|record| record.executed_tick.as_u64() == tick),
                        "tick {tick}: the tick's own records survive verbatim"
                    );
                }
            }
            spine
        };
        let spine = run();
        // The exact end shape: firings at ticks 5, 8, …, 500 (every
        // 3 ticks once the sawtooth settles) — 166 firings.
        assert_eq!(spine.stats().executed, TICKS * BUDGET);
        assert_eq!(spine.retention_stats().compactions, 166);
        assert_eq!(spine.retention_stats().summarized_total, 49_800);
        assert_eq!(spine.retention_stats().last_chunk, 300);
        assert_eq!(spine.retention_stats().max_chunk, 300);
        let HistoryKind::Compacted {
            summarized_records,
            retained_records,
            ..
        } = spine.history_kind()
        else {
            panic!("the sustained load compacted the spine");
        };
        assert_eq!(summarized_records, 49_800);
        assert_eq!(retained_records, 200);
        assert_eq!(spine.ledger_retained().len(), 200);
        // The round trip: byte-identical, no policy, the same state.
        let bytes = spine.save_bytes();
        let restored: Spine<SustainedWorld> =
            Spine::restore(&bytes, Diagnostics::new(Level::Off)).expect("restore");
        assert_eq!(restored.save_bytes(), bytes);
        assert_eq!(restored.retention_policy(), None);
        assert_eq!(restored.retention_stats(), RetentionStats::default());
        assert_eq!(
            restored.compacted_fingerprint(),
            spine.compacted_fingerprint()
        );
        // DOUBLE-RUN DETERMINISM: the whole sustained run reproduces
        // byte for byte (the replay re-validates every per-tick
        // invariant above on the way).
        let replay = run();
        assert_eq!(replay.save_bytes(), bytes);
        assert_eq!(
            replay.compacted_fingerprint(),
            spine.compacted_fingerprint()
        );
        assert_eq!(replay.retention_stats(), spine.retention_stats());
    }

    // ----------------------------- CA-07 eligibility/lag/tail-move tests (F07)

    /// THE REFUSAL ARM (CA-07): a retention bound declared WITHOUT the
    /// consequence-eligibility acknowledgment never cuts — the
    /// checkpoint refuses at every threshold crossing (counted), the
    /// ledger's growth past the bound is exposed as lag, and arming
    /// afterwards (the acknowledgment constructed at the composition
    /// boundary) resumes the cadence and converges to the bound. The
    /// sustained shape at ten executions per tick against
    /// retain 4 / chunk 30 (bound 34): ticks 4–6 cross the threshold
    /// and refuse, arming at tick 7 fires chunk-bounded cuts until the
    /// ledger sits inside the band.
    #[test]
    fn unacknowledged_retention_bounds_refuse_and_expose_the_lag() {
        let mut spine = Spine::new(SustainedWorld, 10);
        for i in 0..10u64 {
            spine
                .schedule(spec(5, Field::Near, 1, i))
                .expect("schedule");
        }
        spine.set_retention_bound(policy(4, 30));
        assert_eq!(spine.retention_policy(), Some(policy(4, 30)));
        assert!(
            !spine.retention_armed(),
            "a bound alone never arms the cadence"
        );
        spine.run_ticks(6);
        // 60 records against the 34 bound: three threshold crossings,
        // three refusals, zero cuts, the lag exposed.
        assert_eq!(spine.stats().executed, 60);
        assert_eq!(spine.retention_stats().refused_ticks, 3);
        assert_eq!(spine.retention_stats().compactions, 0);
        assert_eq!(spine.ledger_retained().len(), 60);
        assert_eq!(spine.retention_lag_records(), 60 - 34);
        assert!(matches!(spine.history_kind(), HistoryKind::Full));
        // Arming: the acknowledgment accompanies a fresh
        // configuration (the re-proof), and the cadence fires.
        spine.set_retention_policy(policy(4, 30), eligibility());
        assert!(spine.retention_armed());
        spine.run_tick();
        assert_eq!(spine.retention_stats().compactions, 1);
        assert_eq!(
            spine.retention_stats().refused_ticks,
            3,
            "armed: no more refusals"
        );
        // 70 records, one chunk of 30 cut: 40 retained, 6 above the
        // bound — the hysteresis band converging.
        assert_eq!(spine.ledger_retained().len(), 40);
        assert_eq!(spine.retention_lag_records(), 6);
        spine.run_ticks(3);
        // Tick 8 fires again (50 → 20); ticks 9–10 float inside the
        // band then fire once more (40 → 10): three armed firings
        // total, the ledger at or under the bound.
        assert_eq!(spine.retention_stats().compactions, 3);
        assert!(spine.ledger_retained().len() <= 34);
        assert_eq!(spine.retention_lag_records(), 0);
        assert!(matches!(
            spine.history_kind(),
            HistoryKind::Compacted { .. }
        ));
    }

    /// THE RESTORE→RECONFIGURE→NEXT-COMPACT PIN with UNRESOLVED
    /// CONSEQUENCES (CA-07's mandated control): a spine whose blocked
    /// waiter depends on a HELD prerequisite (an obligation that can
    /// never complete while the hold stands — the unresolved
    /// consequence) compacts on schedule around it; a mid-cadence
    /// save→restore→re-arm→continue reproduces the uninterrupted
    /// control's canonical saves BYTE FOR BYTE, and the re-arm is
    /// required (the restored spine arrives unarmed — configuration
    /// never rides the wire, and the acknowledgment never survives a
    /// process boundary it was not re-proved at).
    #[test]
    fn restore_reconfigure_next_compact_matches_the_control_with_unresolved_consequences() {
        fn build() -> Spine<SaveableNoopWorld> {
            let mut spine = Spine::new(SaveableNoopWorld, 100);
            // The unresolved consequence: a waiter on a held
            // prerequisite — blocked forever, serialized with every
            // save, present at every compaction boundary.
            let held = spine
                .schedule_held(spec(0, Field::Near, 1, 999))
                .expect("schedule held");
            spine
                .schedule(dep_spec(0, Field::Near, 1, 1000, vec![held]))
                .expect("schedule");
            for t in 1..=6u64 {
                for i in 0..100u64 {
                    spine
                        .schedule(spec(0, Field::Near, t, t * 100 + i))
                        .expect("schedule");
                }
            }
            spine
        }
        // The uninterrupted control: six ticks of 100 executions
        // against retain 10 / chunk 300 (bound 310) — one firing at
        // tick 4 (400 > 310, cutting 300), the blocked waiter
        // unresolved at every boundary.
        let mut control = build();
        control.set_retention_policy(policy(10, 300), eligibility());
        control.run_ticks(6);
        assert_eq!(control.retention_stats().compactions, 1);
        assert_eq!(
            control.blocked_count(),
            1,
            "the consequence stays unresolved"
        );
        assert_eq!(control.held_count(), 1);
        // The split: three ticks (no firing yet), save at the
        // boundary, restore, re-arm, continue.
        let mut split = build();
        split.set_retention_policy(policy(10, 300), eligibility());
        split.run_ticks(3);
        assert_eq!(split.retention_stats().compactions, 0);
        assert_eq!(split.blocked_count(), 1);
        let bytes = split.save_bytes();
        let mut restored: Spine<SaveableNoopWorld> =
            Spine::restore(&bytes, Diagnostics::new(Level::Off)).expect("restore");
        assert_eq!(restored.save_bytes(), bytes, "the round trip is stable");
        assert_eq!(
            restored.retention_policy(),
            None,
            "the policy never rides the wire"
        );
        assert!(
            !restored.retention_armed(),
            "the acknowledgment never rides the wire"
        );
        assert_eq!(
            restored.blocked_count(),
            1,
            "the unresolved consequence survived"
        );
        assert_eq!(restored.retention_stats(), RetentionStats::default());
        // A bound without the proof would refuse; the re-armed policy
        // fires the SAME cut the control fired.
        restored.set_retention_bound(policy(10, 300));
        restored.run_ticks(1);
        assert_eq!(restored.retention_stats().refused_ticks, 1);
        assert_eq!(restored.retention_stats().compactions, 0);
        restored.set_retention_policy(policy(10, 300), eligibility());
        restored.run_ticks(2);
        assert_eq!(restored.retention_stats().compactions, 1);
        assert_eq!(
            restored.save_bytes(),
            control.save_bytes(),
            "restore → reconfigure → next compact is byte-identical to the control"
        );
        assert_eq!(
            restored.compacted_fingerprint(),
            control.compacted_fingerprint()
        );
    }

    /// THE RETAINED-TAIL MOVE, MEASURED (CA-07's mandated measurement):
    /// at constant chunk size, one compaction's cost is exactly
    /// `chunk` SHA-256 record hashes plus `retained` in-place record
    /// moves (the [`Vec::drain`] prefix removal — the pre-CA-07
    /// `split_off` shape additionally allocated a fresh tight vector
    /// per firing and forced a reallocation on the next push past
    /// it). The operation counts are exact and deterministic (the
    /// IA-07 discipline: costs are operation counts until a
    /// benchmark times them — wall-clock is deliberately not pinned
    /// here); the capacity witness below is the allocation proof:
    /// the in-place drain retains the high-water capacity across
    /// every firing, so the sustained loop fires with ZERO ledger
    /// allocations, while the bytes moved grow with the retained
    /// history at constant chunk.
    #[test]
    fn policy_compaction_moves_the_tail_in_place_at_constant_chunk() {
        const CHUNK: u64 = 300;
        // Two retained-history sizes at the same chunk: the moves per
        // firing are the retained-record counts after each cut.
        for (retain, ticks) in [(128u64, 12u64), (300, 12u64)] {
            let mut spine = Spine::new(SustainedWorld, 100);
            for i in 0..100u64 {
                spine
                    .schedule(spec(5, Field::Near, 1, i))
                    .expect("schedule");
            }
            spine.set_retention_policy(policy(retain, CHUNK), eligibility());
            let mut moves: Vec<u64> = Vec::new();
            let mut capacity_high_water: usize = 0;
            for tick in 1..=ticks {
                spine.run_tick();
                // The capacity witness: never shrinks, never regrows
                // after the first firing (drain keeps the buffer;
                // split_off would have re-allocated it tight).
                let capacity = spine.ledger.capacity();
                assert!(
                    capacity >= capacity_high_water,
                    "tick {tick}: the ledger buffer never shrinks (retain {retain})"
                );
                capacity_high_water = capacity_high_water.max(capacity);
                let stats = spine.retention_stats();
                if stats.compactions > moves.len() as u64 {
                    // The firing's move count: the retained tail's
                    // length after the cut (exactly the records
                    // drain moved down within the buffer).
                    moves.push(spine.ledger_retained().len() as u64);
                }
            }
            let stats = spine.retention_stats();
            assert_eq!(stats.last_chunk, CHUNK, "the chunk is constant");
            assert_eq!(stats.max_chunk, CHUNK);
            // The published cost table: per firing, `CHUNK` hashes +
            // `retained` moves — the moves grow with the retained
            // history at constant chunk (128-retain moves 200 per
            // firing; 300-retain moves 400 per firing).
            assert!(
                moves.iter().all(|&retained_after| retained_after >= retain),
                "every firing retains at least the verbatim window"
            );
            assert_eq!(
                moves.len() as u64,
                stats.compactions,
                "one move-count sample per firing (retain {retain})"
            );
            // The capacity discriminator: after the first firing the
            // buffer is strictly larger than the retained tail (the
            // high-water survives the cuts — split_off's fresh tight
            // vector would have made them equal).
            assert!(
                spine.ledger.capacity() > spine.ledger_retained().len(),
                "the in-place drain retains the high-water capacity (retain {retain})"
            );
            if retain == 300 {
                // Bound 600: firings at ticks 7 and 10, each cutting
                // one 300 chunk from 700 down to 400 retained.
                assert_eq!(moves, vec![400, 400]);
            } else {
                // Bound 428: firings at ticks 5, 8, and 11, each
                // cutting one 300 chunk from 500 down to 200 retained.
                assert_eq!(moves, vec![200, 200, 200]);
            }
        }
    }
}
