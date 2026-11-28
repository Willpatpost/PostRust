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
//!
//! # The housekeeping cost model (T08 §2 — the audit's demanded split)
//!
//! With execution budget `B`, the admission caps `D`/`F`/`K`
//! ([`HousekeepingAdmission`]), `n` the pending population, and
//! `hk ≤ 4.5·B + 4F` the whole-tick housekeeping-unit bound (see
//! [`housekeeping_cap`]'s ceiling paragraph), the ENGINE-bounded work
//! per tick is the closed form
//! `O(B·(F + K·(D + log n)) + hk·D·log n)`:
//!
//! | Work class | Per-unit engine cost | Per-tick count bound |
//! |---|---|---|
//! | Execution (the drain's execute arm) | ledger push (amortized O(1)), `unfinished` remove O(log n), stats | ≤ B |
//! | — its atomic waiter release | one O(1) `try_unblock` per registered waiter | ≤ B·F (registered ⊆ declared) |
//! | — its follow-up admissions | O(D) validation + O(log n) push (cropped at K) | ≤ B·K |
//! | Park (the drain's park arm) | O(D) unmet scan + ≤ D `waiting_on` inserts (O(log n) each) + O(1) hashed counter insert + O(log n) blocked insert | ≤ Σ allowances ≤ 4.5·B |
//! | Heap churn | O(log n) per push/pop | ≤ (B + parks + unblocks) · O(log n) |
//! | Declared-fan-in upkeep | O(1) hashed entry op per site | ≤ B + parks, allocation-free in reserve |
//! | Stale entries | `NotBlocked` charged (the charge identity); the lifecycle keeps the arm unreachable | 0 in admitted graphs |
//! | Compaction (T07) | per firing: chunk-bounded SHA-256 + head bump; amortized ≤ one move per discarded record | policy-bounded |
//! | Class index (CA-08) | O(log n) membership update per transition | ≤ the counts above |
//!
//! The `log n` term is the pending population's heap/B-tree depth —
//! the engine's per-tick work is population-log, never
//! population-linear (the T09 regional-locality finding is a
//! different defect and stays open).
//!
//! **External handler cost — NOT engine-bounded, stated plainly:**
//! handler bodies (`World::execute` runs arbitrary user code inside
//! `execute_now` — the engine bounds its own bookkeeping around the
//! handler, never the handler); the follow-up count a handler
//! ATTEMPTS beyond K (the engine admits ≤ K per execution and crops
//! the rest fail-visibly — the emission effort is the handler's own
//! cost); and driver-side costs (world fingerprinting, enabled
//! diagnostics, the driver's own scheduling and retention-consumer
//! policy).
//!
//! # T08 measurement obligations (the next rig window)
//!
//! The bounds are landed; these measurements are what actually close
//! B01, and none is claimed done by the observations above:
//!
//! 1. **The 1.5× `[ASSUMPTION]` re-pin** (see [`housekeeping_cap`]):
//!    the sustained matrix plus adversarial graphs must re-measure
//!    the housekeeping-per-execution ratio at production fan-in
//!    mixes and re-pin the multiple under the placeholder protocol.
//! 2. **The default-cap headroom audit**: D=64/F=4096/K=1024 exceed
//!    every workspace fixture's observed maximum, but whether
//!    production worlds fit under them — or prefer tighter caps
//!    with visible refusals — is driver policy the observability
//!    (refusal counts, crop counts, backlog slope) makes
//!    conscious; audit the headroom at the landing measurement.
//! 3. **The composite per-tick actual at production scale**: the
//!    batteries pin actuals at fixture scales (≤ 600-herd, 100k
//!    unrelated); production-scale adversarial composites at the
//!    §2 ceiling are a rig obligation, not a test-bed claim.
//! 4. **The declared-vs-registered conservative gap**: F caps
//!    DECLARED fan-in, not registered — whether the conservative
//!    gap matters in real workloads is measurable
//!    (`dependency_registrations` against the map's counts); a
//!    registered-count cap would reintroduce the park-time
//!    livelock class and is rejected pending evidence it is
//!    needed.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap};
use std::hash::{BuildHasher, Hasher};
use std::num::{NonZeroU32, NonZeroU64};

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
    /// The K admission bound's remaining slots for this execution
    /// (T08 §4.3): initialized to
    /// [`HousekeepingAdmission::max_follow_ups`] at `ExecCtx`
    /// construction (the single construction site lives in
    /// `Spine::execute_now`), decremented per buffered follow-up.
    admissions_remaining: u32,
    /// Follow-up emissions cropped at emission by the K bound —
    /// beyond the cap the emission is not buffered, not scheduled,
    /// and counted; `execute_now` publishes the total into
    /// [`HousekeepingStats::follow_ups_cropped`] after the drain
    /// (never silent: the counter plus an `Error`-level diagnostic
    /// line). Cropping at EMISSION also bounds the scratch buffer's
    /// steady-state element capacity at K.
    crops: u32,
}

impl<'a> ExecCtx<'a> {
    /// The tick at which the current event is actually executing.
    pub fn executed_tick(&self) -> Tick {
        self.executed_tick
    }

    pub fn is_executed(&self, id: EventId) -> bool {
        id.seq < self.next_seq && !self.unfinished.contains(&id.seq)
    }

    /// The K admission gate (T08 §4.3): one slot per buffered
    /// follow-up, the excess cropped at emission — not buffered, not
    /// scheduled, counted — a config outcome deliberately distinct
    /// from the dependency-shape refusals (`invalid_dropped`, a
    /// fingerprinted Stats class): a crop is the admission policy
    /// speaking, fail-visible through the counter and the
    /// `Error`-level diagnostic.
    fn admit(&mut self, spec: EventSpec) {
        if self.admissions_remaining == 0 {
            self.crops += 1;
            return;
        }
        self.admissions_remaining -= 1;
        self.follow_ups.push(spec);
    }

    /// Schedules an execution-relative follow-up: due = executed_tick +
    /// `delay`. Anchors to *actual* execution, not to the event's intended
    /// time — an overdue cause shifts its effects honestly instead of
    /// stacking impossible deadlines (`docs/TECH-ARCHITECTURE.md` §3.2).
    /// Beyond the K admission bound
    /// ([`HousekeepingAdmission::max_follow_ups`]) the emission is
    /// cropped at emission — never silently (see [`Self::admit`]).
    pub fn follow_up(
        &mut self,
        delay: u64,
        class: EventClass,
        payload: u64,
        depends_on: Vec<EventId>,
    ) {
        self.admit(EventSpec {
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
    /// retreat. Beyond the K admission bound the emission is cropped
    /// at emission (see [`Self::admit`]).
    pub fn schedule_at(
        &mut self,
        tick: Tick,
        class: EventClass,
        payload: u64,
        depends_on: Vec<EventId>,
    ) {
        self.admit(EventSpec {
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
    /// Beyond the K admission bound the emission is cropped at
    /// emission (see [`Self::admit`]).
    pub fn follow_up_in_field(
        &mut self,
        field: Field,
        delay: u64,
        class: EventClass,
        payload: u64,
        depends_on: Vec<EventId>,
    ) {
        self.admit(EventSpec {
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
/// stats) — never silently dropped (E-10 discipline). The two admission
/// refusals (T08) are CONFIG outcomes, surfaced typed at the producer so
/// the driver can raise the cap ([`Spine::set_admission_bounds`]) or
/// restructure its graph instead of the engine silently absorbing
/// unbounded housekeeping work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleError {
    /// A dependency references an event that was never scheduled. Because
    /// dependencies must reference already-scheduled or already-executed
    /// events, this rule also makes dependency cycles structurally
    /// impossible (review correction 2).
    UnknownDependency(EventId),
    /// The event's dependency-declaration length exceeds the admission
    /// bound D ([`HousekeepingAdmission::max_dependencies`]) — the raw
    /// `depends_on` length, checked before the identity is allocated so a
    /// refused spec leaves zero trace (no seq consumed, no queue push, no
    /// counter moves). The raw length bounds the unique fan-out (the
    /// registration pass dedups repeated ids later), so one park's
    /// registration work is bounded `O(D·log n)` — a closed form, never a
    /// hope (T08 §3.1).
    DependencyFanOutExceeded {
        /// The raw declaration length the schedule call observed.
        observed: usize,
        /// The configured cap D.
        cap: u32,
    },
    /// Scheduling this event would push a prerequisite's declared-dependent
    /// count above the admission bound F
    /// ([`HousekeepingAdmission::max_declared_dependents`]). The count is
    /// the number of UNFINISHED events declaring the prerequisite —
    /// declarations on executed prerequisites are free (the declarer
    /// executes directly and never parks) — maintained through the
    /// derived, wire-invisible `declared_fan_in` map, so the atomic
    /// waiter release an execution performs is bounded by exactly this
    /// count (registered waiters ⊆ declared dependents, T08 §3.1's
    /// load-bearing proof: the E-10 all-or-nothing release stays atomic,
    /// bounded rather than suspended). Checked before the identity is
    /// allocated; a refused spec leaves zero trace.
    PrerequisiteFanInExceeded {
        /// The prerequisite whose declared-dependent count would exceed
        /// the cap.
        prerequisite: EventId,
        /// The count the schedule call would have produced
        /// (`declared + 1`).
        observed: u32,
        /// The configured cap F.
        cap: u32,
    },
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
/// the L7 lane, extended by T07's two-class eligibility model): the
/// configurable cadence that bounds the ledger's resident size by
/// firing the EXISTING explicit compaction
/// ([`Spine::compact`]) — no second compaction mechanism, no second
/// summary domain. The policy is OPT-IN configuration with no
/// default: a spine constructed without one never auto-compacts,
/// keeping today's exact behavior (the no-silent-default rule — the
/// application opts in explicitly, and opting in now CARRIES the
/// eligibility decision [`Spine::compact`]'s own doc delegates to
/// the driver — the T07 two-class model: a world that is
/// PERMANENTLY consequence-free (no application consumer of
/// execution-history records exists or will ever exist) arms the
/// cadence through [`Spine::set_retention_policy`] with the
/// [`ConsequenceFreedom`] world-class capability, while a
/// consequence-bearing world arms it through
/// [`Spine::arm_retention_frontier`] and the cadence cuts only up
/// to the minimum consumer acknowledgment, never on standing
/// configuration — the engine policy bounds RAM, the driver proves
/// when consequence lanes tolerate a prefix cut, and the proof is
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
    /// but declined to cut for want of firing eligibility — a bound
    /// declared through [`Spine::set_retention_bound`] without an
    /// arming (Class 1 or Class 2), OR a frontier armed with zero
    /// registered consumers (the empty-frontier fail-closed rule:
    /// `min(∅)` is a refusal, never a permission) — the refusal the
    /// lag observer ([`Spine::retention_lag_records`]) makes visible
    /// as the ledger's growth past the bound.
    pub refused_ticks: u64,
    /// Ticks at which the threshold was exceeded with the frontier
    /// (not the arming) the limiter: a registered consumer's
    /// acknowledgment below the policy's cut — the cadence held,
    /// counted, observable (T07's Class 2 backpressure signal; the
    /// arming was present, the consumers had not released).
    pub frontier_hold_ticks: u64,
    /// The most recent threshold-exceeded evaluation's frontier
    /// shortfall: how many records the policy could have cut
    /// (`min(excess, chunk)`) but the consumers had not released —
    /// `min(excess, chunk)` minus the clamped cut actually taken.
    /// Zero when the consumers released everything the policy
    /// wanted; the policy's full want when the frontier holds
    /// everything. Updated only at threshold-exceeded frontier
    /// evaluations (under the threshold the policy wants nothing,
    /// so no frontier shortfall is defined); the value read is the
    /// last such evaluation's.
    pub retention_frontier_lag_records: u64,
}

/// The composition boundary's consequence-freedom capability (T07,
/// CA-07/F07 — Q05's eligibility half, the design's Class 1):
/// constructing this value asserts the WORLD CLASS — no application
/// consumer of execution-history records exists or will ever exist
/// in this composition (no late accounting over the verbatim tail,
/// no audit consumer, no history API use — the world's handlers
/// execute, the spine's own authority structures carry everything
/// the engine needs, and history is diagnostic only). For such a
/// world the eligible prefix is EVERYTHING EXECUTED — the frontier
/// is the whole ledger — and [`RetentionPolicy`]'s `retain` window
/// is a diagnostic-preference window, not a safety window.
///
/// **The honest limit, stated in the type:** Rust cannot prove a
/// negative about future callers; the capability is still an
/// assertion. What the two-class model changes is that the
/// assertion is now SCOPED to a world class with an unbounded
/// frontier — it no longer impersonates evidence about consumer
/// state. The register of evidence (which compositions asserted
/// it) is the commit history and the audit trail, not the type
/// system. Where the driver can discharge the proof structurally it
/// should prefer the frontier class
/// ([`Spine::arm_retention_frontier`]) even for consequence-free
/// worlds; this class exists for the genuinely static case
/// (deterministic simulations, benchmarks, the spine's own test
/// fixtures).
///
/// The capability is REQUIRED at every (re)configuration of the
/// retention policy through [`Spine::set_retention_policy`] — the
/// cadence never fires on standing configuration alone, and a
/// restored spine (whose configuration restore clears, an
/// intentional contract) must re-assert it before the next compact.
/// A driver that wants the RAM bound declared without the world
/// class assertion uses [`Spine::set_retention_bound`]: the
/// checkpoint then REFUSES to cut and the refusal and lag are
/// observable ([`RetentionStats::refused_ticks`],
/// [`Spine::retention_lag_records`]) — never a silent default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsequenceFreedom {
    /// Construction is the assertion; the field keeps the value
    /// opaque (there is nothing to read — the words are the proof).
    _asserted: (),
}

impl ConsequenceFreedom {
    /// Constructs the world-class capability: by constructing this
    /// value the composition boundary asserts its world is
    /// PERMANENTLY consequence-free (see the type doc — the eligible
    /// prefix is unbounded by construction of the application, not
    /// by evidence gathered at runtime).
    pub const fn permanently_free() -> Self {
        ConsequenceFreedom { _asserted: () }
    }

    /// The historical constructor name (CA-07's landing), kept for
    /// one release so every existing call site compiles unchanged —
    /// the token was never an "acknowledgment" of consumer state,
    /// and the rename says so; use
    /// [`ConsequenceFreedom::permanently_free`].
    #[deprecated(
        since = "0.1.0",
        note = "renamed: ConsequenceFreedom::permanently_free — the token asserts the \
                consequence-free WORLD CLASS, not an acknowledgment of consumer state (T07)"
    )]
    pub const fn acknowledged() -> Self {
        ConsequenceFreedom::permanently_free()
    }
}

/// The CA-07 landing's name for [`ConsequenceFreedom`] (deprecated
/// for one release per the T07 design's §6 migration): every
/// external caller and test call site compiles unchanged through
/// the alias while the docs stop calling the token an
/// "acknowledgment."
#[deprecated(
    since = "0.1.0",
    note = "renamed: ConsequenceFreedom — the two-class eligibility model (T07); construct \
            through ConsequenceFreedom::permanently_free, or arm the per-consumer frontier \
            through Spine::arm_retention_frontier"
)]
pub type ConsequenceEligibility = ConsequenceFreedom;

/// A named retention consumer (T07's Class 2 — the consequence-bearing
/// world's eligible-prefix frontier): a small stable identity the
/// driver assigns to each application lane that reads execution
/// history. The id keys the consumer's monotone acknowledgment in
/// [`Spine::arm_retention_frontier`]'s registry; the frontier value
/// the cadence consults is the MINIMUM over every registered
/// consumer's acknowledgment, so a consumer is exactly as binding as
/// its id being registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RetentionConsumerId(&'static str);

impl RetentionConsumerId {
    /// The consumer identity for a stable name (the driver's own
    /// naming; two consumers are distinct iff their names are).
    pub const fn new(name: &'static str) -> Self {
        RetentionConsumerId(name)
    }

    /// The stable name the id was constructed from.
    pub const fn as_str(&self) -> &'static str {
        self.0
    }
}

/// Typed refusals of [`Spine::acknowledge_retention_through`] (the
/// frontier's monotonicity, enforced — never assumed): a consumer's
/// acknowledgment only ever advances, and only consumers the driver
/// registered exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionAckError {
    /// No consumer is registered under this id — the acknowledgment
    /// registry is explicit ([`Spine::register_retention_consumer`]),
    /// and an ack for an unknown name is a driver wiring error, never
    /// a silent registration.
    UnknownConsumer { id: RetentionConsumerId },
    /// The attempted acknowledgment REGRESSES below the consumer's
    /// standing acknowledgment (monotonicity is a rule, not an
    /// assumption: a consumer that re-derives its durable read point
    /// must never move it backwards).
    Regression {
        id: RetentionConsumerId,
        /// The consumer's standing acknowledgment (the high-water the
        /// attempted value would have to reach).
        acknowledged_through: u64,
        /// The regressing value the call attempted.
        attempted_through: u64,
    },
}

/// Typed refusals of [`Spine::register_retention_consumer`] (the
/// register rule, made mechanical — the join-time confrontation the
/// T07 design's §8 honest limit requires): registering onto a
/// compacted spine vouches for the STANDING cut, because the
/// already-summarized prefix is unrecoverable by design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionRegisterError {
    /// The consumer's durable read point (`vouches_through`) sits
    /// below the standing summarized prefix: the records it still
    /// needs were already summarized — unrecoverable, impossible to
    /// serve (they were impossible to serve before the frontier
    /// existed too; the register rule forces that impossibility to
    /// be confronted at JOIN time, never discovered at read time).
    /// The driver must handle this refusal; the engine cannot make
    /// history reconstructible.
    NeedsSummarizedRecords {
        id: RetentionConsumerId,
        /// The standing cut (records already summarized — the minimum
        /// a joining consumer must be able to vouch for).
        summarized_records: u64,
        /// The joining consumer's offered durable read point.
        consumer_read_point: u64,
    },
    /// A consumer is already registered under this id: registration
    /// is a join, and silently overwriting a live consumer's
    /// acknowledgment (or silently ignoring the re-registration) is
    /// exactly the class of unnoticed driver bug the typed-refusal
    /// discipline exists to surface. Advance an existing consumer
    /// through [`Spine::acknowledge_retention_through`].
    AlreadyRegistered {
        id: RetentionConsumerId,
        /// The registered consumer's standing acknowledgment.
        acknowledged_through: u64,
    },
    /// No frontier is armed: the consumer registry belongs to
    /// [`Spine::arm_retention_frontier`]'s mode, and registering
    /// onto a spine armed with the Class-1 capability (or with only
    /// a declared bound, or nothing) is a wiring mistake surfaced
    /// typed — never a silent registry entry the checkpoint would
    /// ignore.
    FrontierNotArmed { id: RetentionConsumerId },
}

/// The retention cadence's eligibility mode (T07's two-class model —
/// private configuration state, the same never-fingerprinted,
/// never-serialized class as the [`RetentionPolicy`] field itself):
/// the permanently-consequence-free capability (Class 1) or the
/// per-consumer eligible-prefix frontier (Class 2). `None` is the
/// refusal arm ([`Spine::set_retention_bound`]). The frontier never
/// rides the wire: a restored spine arrives with no eligibility and
/// no consumer registry, so a restored process can never resume
/// cutting on a vouch a prior process made and this one did not
/// re-make (the CA-07 hazard, closed at the representation).
#[derive(Debug, Clone, PartialEq, Eq)]
enum RetentionEligibility {
    /// Class 1 — the permanently consequence-free world capability:
    /// the eligible prefix is unbounded, the cadence cut consults no
    /// consumer.
    Freedom(ConsequenceFreedom),
    /// Class 2 — the per-consumer monotone eligible-prefix frontier:
    /// each registered consumer's acknowledgment (a global executed
    /// ordinal — record *n* is the *n*-th execution of the spine's
    /// lifetime, the ordinal space `summarized_records` already
    /// counts on the wire), the cadence cut clamped to
    /// `min(excess, chunk, eligible − summarized)`. `BTreeMap` for
    /// deterministic iteration in the observability report.
    Frontier {
        consumers: BTreeMap<RetentionConsumerId, u64>,
    },
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

/// The scheduler-housekeeping admission bounds (T08 — Option A, bound
/// admission before mutations): three caps, every check BEFORE any graph
/// mutation, giving the whole tick a closed-form engine-work ceiling
/// `O(B·(F + K·(D + log n)) + hk·D·log n)` with `hk ≤ 4.5·B + 4F`.
///
/// * **D** ([`Self::max_dependencies`]) — the per-event RAW
///   dependency-declaration length, refused in [`Spine::schedule`] beside
///   the unknown-dependency validation, before the seq is allocated. The
///   raw length bounds the unique fan-out (registration dedups repeated
///   ids), so one park's registration work is `O(D·log n)`.
/// * **F** ([`Self::max_declared_dependents`]) — the per-prerequisite
///   count of UNFINISHED declarers, refused through the derived
///   wire-invisible `declared_fan_in` map. The load-bearing consequence:
///   registered waiters ⊆ declared dependents ≤ F at every stable
///   boundary, so the E-10 all-or-nothing release an execution performs
///   is **bounded, not suspended** (the CA-03 recorded contract,
///   preserved verbatim).
/// * **K** ([`Self::max_follow_ups`]) — the per-execution follow-up
///   admission count, cropped at `ExecCtx` EMISSION (beyond the cap the
///   emission is not buffered, not scheduled, and counted in
///   [`HousekeepingStats::follow_ups_cropped`]) — never silent, and
///   bounding the scratch buffer's steady-state element capacity at K.
///
/// CONFIGURATION, never logical state — the retention-policy tradition:
/// set through the additive [`Spine::set_admission_bounds`] (no
/// constructor churn), never fingerprinted, never serialized. A restored
/// spine resumes at the DEFAULTS and the driver re-configures — the
/// documented retention-policy precedent: a world saved under non-default
/// bounds and restored without re-configuring behaves under the defaults
/// afterward; the divergence is observable in world content (which
/// declarations get admitted), never in spine fingerprints, because
/// config was never fingerprinted.
///
/// Default justification (measured-actual discipline, T08 §4.1): every
/// workspace fixture's observed maximum sits comfortably under each
/// default — the largest declared fan-in is 2048 (the lanes.rs
/// 2100/2050 herd), the largest dependency list 3 raw entries, the
/// largest per-execution follow-up count 3 — so D=64/F=4096/K=1024 leave
/// 2-500× headroom over every battery while keeping the ceiling
/// closed-form. Within-cap graphs behave bit-for-bit as before: the caps
/// are refusals at admission, not deferrals in the drain, so no
/// canonical order, no event id, no held/blocked/unfinished semantic, no
/// wire byte, and no allocation pin moves. The defaults are judgment,
/// not measurement: whether production worlds fit under them — or prefer
/// tighter caps with visible refusals — is driver policy the
/// observability (refusal counts, crop counts, backlog slope) makes
/// conscious.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HousekeepingAdmission {
    /// The per-event dependency-declaration cap D (raw `depends_on`
    /// length).
    pub max_dependencies: NonZeroU32,
    /// The per-prerequisite declared-dependent cap F.
    pub max_declared_dependents: NonZeroU32,
    /// The per-execution follow-up admission cap K.
    pub max_follow_ups: NonZeroU32,
}

impl Default for HousekeepingAdmission {
    /// D = 64, F = 4096, K = 1024 (see the type doc's default
    /// justification).
    fn default() -> Self {
        HousekeepingAdmission {
            max_dependencies: NonZeroU32::new(64).expect("64 is non-zero"),
            max_declared_dependents: NonZeroU32::new(4096).expect("4096 is non-zero"),
            max_follow_ups: NonZeroU32::new(1024).expect("1024 is non-zero"),
        }
    }
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
    /// The near lane's queue length at the tick's most recent
    /// EXHAUSTION BREAK — a housekeeping-allowance break (B01, any
    /// drain phase) or the execution-budget break below — an O(1)
    /// snapshot of the backlog that remained, per-tick reset (zero
    /// on a tick with no break). Together with
    /// [`Self::housekeeping_backlog_far`] this publishes the backlog
    /// SLOPE per tick (T08 §5.4) where only the cumulative
    /// [`HousekeepingStats::exhaustion_backlog_peak`] existed before:
    /// a caller sampling the report series sees the queue drain
    /// without hand-sampling `near_pending_count`. Observational
    /// class — never fingerprinted, never serialized (a restored
    /// spine resumes at defaults); an upper bound on the DUE backlog
    /// (exact when every queued event is due, the adversarial
    /// shape).
    pub housekeeping_backlog_near: u64,
    /// The far lane's queue length at the tick's most recent
    /// exhaustion break — the far-lane half of the slope publication
    /// (see [`Self::housekeeping_backlog_near`]).
    pub housekeeping_backlog_far: u64,
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
///   [`Self::release_decrements`] and [`Self::release_unblocks`];
/// * a release visit that finds the waiter no longer blocked is the
///   defensively-unreachable `NotBlocked` no-op arm — charged as the
///   visit it performed and published as
///   [`Self::release_not_blocked`] (T08 §5.1), so the release
///   accounting is the CLOSED CHARGE IDENTITY
///   `waiter_visits == release_decrements + release_unblocks +
///   release_not_blocked` at every boundary: every visit resolves to
///   exactly one counted outcome, no class uncharged.
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
    /// Release visits that found the waiter no longer blocked — the
    /// defensively-unreachable no-op arm (the lifecycle keeps
    /// registrations inside the blocked set; no battery reaches it
    /// through the public API), CHARGED as the visit it performed
    /// since T08 §5.1 and counted here so the identity
    /// `waiter_visits == release_decrements + release_unblocks +
    /// release_not_blocked` closes mechanically. If a future path
    /// ever makes the arm reachable, its charge is already correct.
    pub release_not_blocked: u64,
    /// The worst per-tick blended housekeeping total across all four
    /// drain phases (the quantity the per-phase allowance bounds —
    /// published per tick, not just at exhaustion).
    pub worst_tick_housekeeping: u64,
    /// The worst per-tick dependency-registration total across all
    /// four drain phases (mirroring
    /// [`Self::worst_tick_housekeeping`]): the park fan-out's
    /// per-tick slope — the per-edge truth the one-unit park charge
    /// deliberately does not scale with (B01's granularity, kept;
    /// T08 §5.2 publishes the slope beside it instead).
    pub worst_tick_dependency_registrations: u64,
    /// The worst lane queue length observed at a housekeeping
    /// exhaustion break (an O(1) upper bound on the due backlog
    /// that remained — exact when every queued event is due, the
    /// adversarial shape; the per-tick series a caller samples
    /// through [`Spine::near_pending_count`] /
    /// [`Spine::far_pending_count`] yields the backlog slope).
    pub exhaustion_backlog_peak: u64,
    /// Handler follow-up emissions cropped at `ExecCtx` emission by the
    /// K admission bound ([`HousekeepingAdmission::max_follow_ups`],
    /// T08 §4.3): beyond the cap the emission is not buffered, not
    /// scheduled, and counted here. A CONFIG outcome, deliberately a
    /// distinct class from fingerprinted `Stats::invalid_dropped` (the
    /// dependency-shape refusals a handler could have fixed): a crop
    /// is the admission policy speaking, never silently — this counter
    /// plus the `Error`-level diagnostic line under the `would_log`
    /// guard make it fail-visible.
    pub follow_ups_cropped: u64,
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

/// One class's membership across the spine's four obligation homes
/// (CA-08/F08 — the regional scheduler-locality index's value type).
/// The queued sets carry the spine's canonical priority key
/// `(due, seq)` — a `BTreeSet` over it iterates in exactly the order
/// a lane's canonical sort produces; the blocked and held sets carry
/// bare seqs — ascending seq is the canonical visitation order the
/// blocked/held extract passes already guarantee (IA-02). The
/// blocked set is deliberately LANE-BLIND: a blocked obligation's
/// field is its future lane, consulted only when it re-enters a
/// queue, so a reclassify of blocked members changes no membership.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ClassMembership {
    near: BTreeSet<(Tick, u64)>,
    far: BTreeSet<(Tick, u64)>,
    blocked: BTreeSet<u64>,
    held: BTreeSet<u64>,
}

/// The derived class-locality index (CA-08): class → that class's
/// membership across the lanes, the blocked map, and the held set.
/// The outer map is keyed-lookup only in production code (never
/// iterated), so its ordering cannot reach an observable — the
/// `witness_index` / `remaining_deps` lookup-only discipline.
type ClassIndex = BTreeMap<EventClass, ClassMembership>;

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
    /// The logical head of the execution ledger (T07 §7's Option 4 —
    /// the tail-shifting drain replacement): `[..head]` is the
    /// physically present but already-summarized DEAD prefix,
    /// `[head..]` the live tail every public view exposes.
    /// INVARIANT: `head > 0` implies `compacted.is_some()` — the
    /// dead prefix exists only after a compaction and a `Full` spine
    /// always has `head == 0`, so the full-form save, `ledger()`,
    /// the full-history fingerprints, and `try_ledger_full` never
    /// observe a dead prefix; restore always rebuilds tight
    /// (`head == 0`). The cut path bumps `head` — zero bytes moved,
    /// zero allocations — and an amortized compact
    /// (`Vec::drain(..head)`, still allocation-free, capacity
    /// retained) fires when `head` reaches the live tail's size (the
    /// classic amortization: each record is physically moved at most
    /// once per discard batch, total moves ≤ total discarded records
    /// over any prefix of the run), with physical residency
    /// documented at worst `2 × live` at the trigger — never data
    /// discarded to hit it. Representation-internal, never
    /// fingerprinted, never serialized: the wire encodes the live
    /// tail only, so a live spine with a dead prefix and its
    /// restored tight image encode IDENTICAL bytes.
    head: usize,
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
    /// The eligibility arming the retention cadence (T07's two-class
    /// model — see [`RetentionEligibility`]): the
    /// permanently-consequence-free world capability (Class 1,
    /// [`ConsequenceFreedom`]) or the per-consumer eligible-prefix
    /// frontier (Class 2, [`Spine::arm_retention_frontier`]). A
    /// policy may be declared WITHOUT it (the refusal arm — the
    /// checkpoint declines to cut, the lag exposed), and the arming
    /// must accompany every (re)configuration that wants firing.
    /// Same configuration class as `retention`: never fingerprinted,
    /// never serialized, cleared at restore — a restored spine
    /// arrives with no eligibility AND no consumer registry, so it
    /// can never resume cutting on a vouch a prior process made.
    retention_eligibility: Option<RetentionEligibility>,
    /// Observational retention counters (the [`RetentionStats`] doc
    /// — the `LaneTotals`/`SuspensionStats` never-fingerprinted,
    /// never-serialized class).
    retention_stats: RetentionStats,
    /// Observational housekeeping-work counters (the
    /// [`HousekeepingStats`] doc — the same never-fingerprinted,
    /// never-serialized class; the CA-03 class-separate publication
    /// of the drain's bookkeeping).
    housekeeping_stats: HousekeepingStats,
    /// The at-break lane queue lengths feeding
    /// [`LaneReport::housekeeping_backlog_near`] /
    /// [`LaneReport::housekeeping_backlog_far`] (T08 §5.4): reset at
    /// the top of every `run_tick`, written at each exhaustion break
    /// the tick passes through (a drain phase's housekeeping break or
    /// the end-of-tick budget break — the most recent break's
    /// snapshot wins), read once at report assembly. Observational
    /// scratch in the never-fingerprinted, never-serialized class;
    /// zero on a tick with no break.
    backlog_at_break_near: u64,
    backlog_at_break_far: u64,
    /// Reusable handler follow-up buffer — taken per execution, drained
    /// into `schedule`, and returned with its capacity retained (the
    /// F-02 caller-held-scratch discipline applied to the spine's
    /// execution loop: collecting handler follow-ups costs no
    /// allocation in steady state). Representation-internal and
    /// fingerprint-invisible — the same class of state as
    /// `max_executed_index`.
    scratch_follow_ups: Vec<EventSpec>,
    /// The derived class-locality index (CA-08/F08 — the regional
    /// scheduler-locality index): `EventClass` → that class's
    /// membership across both lane queues, the blocked map, and the
    /// held set, maintained at every obligation-mutation site once
    /// materialized. `None` — the default — is the UNMATERIALIZED
    /// state: no class-keyed call has ever run, every maintenance
    /// site short-circuits on a `None` check, and the generic
    /// scheduling main path pays nothing (the `spine_allocations`
    /// pins' scenario never materializes — the F09 adoption-boundary
    /// discipline applied to the spine: the index is built only when
    /// a class-keyed transition actually occurs, the first such call
    /// rebuilding it from canonical state in one pass).
    ///
    /// DERIVED, never serialized, never fingerprinted (the
    /// `held_index` / `remaining_deps` / `witness_index` precedent):
    /// the canonical structures (queues, `blocked`, `held`) stay the
    /// single authority; the index is a pure function of them
    /// (rebuild-equals-maintained, pinned by test), a restored spine
    /// starts `None` and reconstructs on its first class-keyed call,
    /// and save bytes, fingerprints, and every statistic are
    /// untouched by its existence. Memory cost, once live: one
    /// `BTreeSet` entry per unfinished obligation (~the price
    /// `witness_index` pays per captured key) — the price of
    /// class-keyed transitions that touch only their own members;
    /// entries vanish when a class's last member leaves.
    class_index: Option<ClassIndex>,
    /// The derived declared-fan-in counts (T08 §4.2): prerequisite
    /// seq → the number of UNFINISHED events whose `depends_on`
    /// declares it. INVARIANT at every stable boundary:
    /// `declared_fan_in[P]` equals that count for every unfinished
    /// P; entries for executed P do not exist (removed at P's
    /// execution); a prerequisite with zero unfinished declarers has
    /// no entry. The F admission check consults it (point lookup
    /// only) and the four maintenance sites keep it:
    /// `schedule`/`schedule_held` (+1 per unique dep whose target is
    /// unfinished — the held arm is REQUIRED by the
    /// registered-⊆-declared proof, since a held declarer is
    /// unfinished and parks on resume), `execute_now` (remove the
    /// executed event's OWN entry, beside the `unfinished.remove`
    /// and before the handler runs so follow-ups declaring their
    /// just-executing producer skip the check), the held
    /// reconstruction's recount, and `restore`'s rebuild pass.
    ///
    /// THE UPKEEP PROOF (why no decrement site exists): a declarer W
    /// of P can leave `unfinished` only by executing, and W executes
    /// only when every dependency is met — P included — so P's entry
    /// was already removed at P's execution; while P's entry exists,
    /// the count of its declarers never changes. Suspension is
    /// invariant by construction (a held declarer stays unfinished;
    /// resume re-parks through the normal path) — fewer maintenance
    /// sites than `waiting_on` itself.
    ///
    /// DERIVED, never serialized, never fingerprinted, rebuilt at
    /// restore through the canonical-spec pass (the
    /// `remaining_deps` precedent — wire-invisible by construction);
    /// never iterated for any decision (point lookups, inserts, and
    /// removes only — so no deterministic-iteration requirement).
    /// REPRESENTATION: a lookup-only `HashMap` over the
    /// `RemainingDepsHasherBuilder`, PRE-RESERVED at construction
    /// (`DECLARED_FAN_IN_RESERVE`) — the allocation-pin discipline:
    /// the reservation moves the structural cost outside every
    /// measured window and the steady-state ops are
    /// allocation-free within reserve.
    declared_fan_in: HashMap<u64, u32, RemainingDepsHasherBuilder>,
    /// The scheduler-housekeeping admission bounds (T08 — see
    /// [`HousekeepingAdmission`]). Configuration, never logical
    /// state: not fingerprinted, not serialized — a restored spine
    /// resumes at the defaults and the driver re-configures through
    /// [`Spine::set_admission_bounds`] (the retention-policy
    /// precedent).
    admission: HousekeepingAdmission,
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

/// Visits each UNIQUE dependency id in `deps` exactly once, in
/// first-occurrence order — the in-place O(len²) dedup scan the
/// admission checks use (T08 §3.1): bounded by the D cap itself
/// (raw length ≤ D), allocation-free by construction, so the
/// checks run inside the allocation-pinned scheduling path without
/// moving a pin. Repeated ids encounter the earlier occurrence's
/// scan and are skipped — the same consecutively-deduplicated
/// declaration order the registration pass and the canonical
/// release order rest on.
fn for_each_unique_dep(deps: &[EventId], visit: impl FnMut(EventId)) {
    let mut visit = visit;
    for (index, dep) in deps.iter().enumerate() {
        if deps[..index].contains(dep) {
            continue;
        }
        visit(*dep);
    }
}

/// Rebuilds the derived declared-fan-in counts from canonical state
/// (T08 §4.2 — the `remaining_deps` rebuild's shape, over EVERY
/// unfinished obligation's spec this time: the count spans queued,
/// blocked, AND held declarers): for each unique dependency whose
/// target is unfinished, one count. The maintained map equals this
/// pure function at every stable boundary (the
/// rebuild-equals-maintained invariant the revalidation helper
/// pins); the held reconstruction's replacement path and `restore`
/// both refresh through this pass.
fn rebuild_declared_fan_in<'a>(
    map: &mut HashMap<u64, u32, RemainingDepsHasherBuilder>,
    events: impl Iterator<Item = &'a Event>,
    unfinished: &BTreeSet<u64>,
    next_seq: u64,
) {
    map.clear();
    for event in events {
        for_each_unique_dep(&event.depends_on, |dep| {
            if !is_executed_seq_with(unfinished, next_seq, dep.seq) {
                *map.entry(dep.seq).or_insert(0) += 1;
            }
        });
    }
}

// -- the class-locality index's maintenance helpers (CA-08) ----------
//
// Free functions over the index alone (the
// `register_waiters_for_unmet_into` split-borrow precedent): every
// obligation-mutation site calls one of these behind a
// `if let Some(index) = &mut self.class_index` gate, so an
// unmaterialized spine — the generic scheduling main path — executes
// one `None` check and nothing else. Each helper is O(log) in the
// class's own membership; none allocates beyond the B-tree node a
// genuinely new member needs (the `witness_index` price).

/// Whether a class's membership has emptied in every home — the
/// signal to drop the outer entry, keeping the index bounded by
/// classes with LIVE members, never by classes ever seen.
fn class_membership_is_empty(membership: &ClassMembership) -> bool {
    membership.near.is_empty()
        && membership.far.is_empty()
        && membership.blocked.is_empty()
        && membership.held.is_empty()
}

/// Records one queued member under its lane's canonical
/// `(due, seq)` key.
fn class_track_queued(
    index: &mut ClassIndex,
    class: EventClass,
    field: Field,
    due: Tick,
    seq: u64,
) {
    let membership = index.entry(class).or_default();
    match field {
        Field::Near => {
            membership.near.insert((due, seq));
        }
        Field::Far => {
            membership.far.insert((due, seq));
        }
    }
}

/// Drops one queued member; the outer entry leaves with the class's
/// last member anywhere.
fn class_drop_queued(index: &mut ClassIndex, class: EventClass, field: Field, due: Tick, seq: u64) {
    let Some(membership) = index.get_mut(&class) else {
        return;
    };
    match field {
        Field::Near => {
            membership.near.remove(&(due, seq));
        }
        Field::Far => {
            membership.far.remove(&(due, seq));
        }
    }
    if class_membership_is_empty(membership) {
        index.remove(&class);
    }
}

/// Records one blocked member (ascending-seq canonical order comes
/// from the set itself).
fn class_track_blocked(index: &mut ClassIndex, class: EventClass, seq: u64) {
    index.entry(class).or_default().blocked.insert(seq);
}

/// Drops one blocked member (the queued-field==lane invariant makes a
/// blocked obligation's field irrelevant here — membership is
/// lane-blind until the release path re-queues it).
fn class_drop_blocked(index: &mut ClassIndex, class: EventClass, seq: u64) {
    let Some(membership) = index.get_mut(&class) else {
        return;
    };
    membership.blocked.remove(&seq);
    if class_membership_is_empty(membership) {
        index.remove(&class);
    }
}

/// Records one held member.
fn class_track_held(index: &mut ClassIndex, class: EventClass, seq: u64) {
    index.entry(class).or_default().held.insert(seq);
}

/// Drops one held member.
fn class_drop_held(index: &mut ClassIndex, class: EventClass, seq: u64) {
    let Some(membership) = index.get_mut(&class) else {
        return;
    };
    membership.held.remove(&seq);
    if class_membership_is_empty(membership) {
        index.remove(&class);
    }
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

/// The constructor-time reservation for the derived declared-fan-in
/// counts (T08 §3.1.1 — the `REMAINING_DEPS_RESERVE` precedent):
/// enough entries that the `spine_allocations` pin scenario — whose
/// window opens INSIDE `run_wave` after spine construction and
/// touches exactly two distinct unfinished prerequisites (the
/// two-dependency blocked event's early and late dependencies) —
/// never allocates on the admission-check or upkeep path. Beyond the
/// reservation the map grows amortized, never on the pinned
/// scenario's path; the steady-state entry ops are allocation-free
/// within reserve, the `RemainingDepsHasherBuilder` reuse.
const DECLARED_FAN_IN_RESERVE: usize = 16;

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
///
/// THE WHOLE-TICK CEILING (T08 §5.3, the analysis the audit
/// demanded): `run_tick` runs four drain phases — near guaranteed
/// (`near_reserve`), far guaranteed (`budget − near_reserve`), and
/// the two redistribution phases against the remaining budget — and
/// each computes this allowance fresh, so the whole-tick sum of
/// phase caps is at most `3·budget` (the guaranteed phases sum to
/// `budget`, the two extras to at most `2·budget` more) and the
/// blended whole-tick housekeeping is bounded by `4.5·budget + 4F`:
/// the per-phase caps contribute `1.5 × 3·budget`, and the `+4F` is
/// the per-phase TERMINAL OVERSHOOT — one execution's atomic waiter
/// release may push a phase past its cap by at most F units (the
/// admission bound on the prerequisite's declared dependents; the
/// bound gates the next iteration, never the release itself — the
/// E-10 all-or-nothing contract). `worst_tick_housekeeping`
/// therefore measures a quantity that is actually bounded, and —
/// with the T08 admission caps — every charged unit bounds
/// closed-form engine work (a park bounds `O(D·log n)`
/// registration work, a release visit is one O(1) `try_unblock`, a
/// follow-up admission is `O(D)` validation).
///
/// VERSION NOTE (the conscious, recorded strengthening): the
/// allowance's NUMBER and GRANULARITY are unchanged — one unit per
/// park, one per release attempt, the 1.5× multiple, per-phase
/// allowances — what changed (T08) is the unit's MEANING: before the
/// admission caps a unit's underlying work was unbounded in the
/// graph (a park could register unbounded edges; a release could
/// walk an unbounded waiter list), so the allowance bounded only a
/// count, never the work. The caps bound admission before any
/// mutation, making each unit's work closed-form without moving a
/// single counter at the defaults. The 1.5× multiple itself remains
/// `[ASSUMPTION]` (see above).
fn housekeeping_cap(cap: u32) -> u64 {
    u64::from(cap) + u64::from(cap) / 2
}

/// One waiter-release attempt's class (CA-03 — the release
/// accounting, counted separately from the blended charge): the
/// charge model stays ONE housekeeping unit per `Decrement` and per
/// `Unblocked` (B01's granularity, unchanged — the class split is
/// observability, never a behavior change).
enum ReleaseAttempt {
    /// The waiter is not currently blocked — a defensive no-op,
    /// charged as the visit it performed (T08 §5.1: every visit
    /// class charges exactly one unit, keeping
    /// `waiter_visits == decrements + unblocks + not_blocked` the
    /// closed charge identity).
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
            head: 0,
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
            backlog_at_break_near: 0,
            backlog_at_break_far: 0,
            scratch_follow_ups: Vec::new(),
            class_index: None,
            declared_fan_in: HashMap::with_capacity_and_hasher(
                DECLARED_FAN_IN_RESERVE,
                RemainingDepsHasherBuilder,
            ),
            admission: HousekeepingAdmission::default(),
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
                retained_records: (self.ledger.len() - self.head) as u64,
                prefix_summary: compacted.prefix_summary,
            },
        }
    }

    /// The retained execution records — the full ledger on a `Full`
    /// instance, the verbatim tail on a `Compacted` one (the
    /// explicitly named safe accessor; identical to [`Self::ledger`]
    /// on `Full`). The tail is the live view `&[head..]` — a dead
    /// prefix (T07 §7) is pure physical slack, never logical, never
    /// observable through this or any other public accessor.
    pub fn ledger_retained(&self) -> &[ExecutionRecord] {
        &self.ledger[self.head..]
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

    /// The F admission check's finder (T08 §3.1): the first unique
    /// dependency whose prerequisite's declared-dependent count would
    /// cross the cap — `None` when every declaration is within bounds
    /// (including declarations on executed prerequisites, which have
    /// no map entry and are free by construction). Read-only: the
    /// check runs before any mutation, so a refused spec leaves zero
    /// trace.
    fn over_cap_prerequisite(&self, deps: &[EventId]) -> Option<(EventId, u32)> {
        if deps.is_empty() {
            return None;
        }
        let max_fan_in = self.admission.max_declared_dependents.get();
        let mut over: Option<(EventId, u32)> = None;
        for_each_unique_dep(deps, |dep| {
            if over.is_some() {
                return;
            }
            if let Some(&declared) = self.declared_fan_in.get(&dep.seq) {
                let observed = declared.saturating_add(1);
                if observed > max_fan_in {
                    over = Some((dep, observed));
                }
            }
        });
        over
    }

    /// Schedules an event. Dependency ids must reference already-scheduled
    /// or already-executed events; unknown ids are rejected immediately
    /// (which also makes dependency cycles structurally impossible).
    ///
    /// **The admission contract (T08, versioned):** a declaration may
    /// also be refused by the housekeeping admission bounds — the
    /// per-event raw dependency length beyond D
    /// ([`HousekeepingAdmission::max_dependencies`], typed
    /// [`ScheduleError::DependencyFanOutExceeded`]), or a
    /// prerequisite's unfinished declared-dependent count beyond F
    /// ([`HousekeepingAdmission::max_declared_dependents`], typed
    /// [`ScheduleError::PrerequisiteFanInExceeded`]). Both refusals
    /// sit BESIDE the unknown-dependency validation — before the seq
    /// is allocated — so a refused spec consumes nothing (no
    /// identity minted, no queue push, no counter moves; the dense
    /// `[0, next_seq)` frontier is untouched). Over-cap graphs move
    /// from "silently admitted, unbounded engine housekeeping work"
    /// to "typed refusal, counted": configure through
    /// [`Self::set_admission_bounds`] (defaults D=64/F=4096/K=1024
    /// leave 2-500× headroom over every workspace fixture's observed
    /// maximum — within-cap graphs behave bit-for-bit as before).
    pub fn schedule(&mut self, spec: EventSpec) -> Result<EventId, ScheduleError> {
        // Dense allocation issues exactly `[0, next_seq)`, so `known`
        // is the frontier bounds check (the E3 representation).
        if let Some(unknown) = spec.depends_on.iter().find(|id| id.seq >= self.next_seq) {
            return Err(ScheduleError::UnknownDependency(*unknown));
        }
        // The D admission bound (T08 §3.1): the RAW declaration
        // length, O(1) to check, bounding the unique registration
        // fan-out (raw ≥ unique) — one park's registration work is
        // O(D·log n), a closed form. Before any mutation.
        if spec.depends_on.len() > self.admission.max_dependencies.get() as usize {
            return Err(ScheduleError::DependencyFanOutExceeded {
                observed: spec.depends_on.len(),
                cap: self.admission.max_dependencies.get(),
            });
        }
        // The F admission bound (T08 §3.1), per UNIQUE dependency,
        // before any mutation: the derived map holds an entry exactly
        // while the prerequisite is unfinished, so declarations on
        // executed prerequisites skip the check naturally (the
        // follow-up-on-producer shape is free); the saturating add
        // refuses rather than wrapping on a legacy over-F restore.
        if let Some((prerequisite, observed)) = self.over_cap_prerequisite(&spec.depends_on) {
            return Err(ScheduleError::PrerequisiteFanInExceeded {
                prerequisite,
                observed,
                cap: self.admission.max_declared_dependents.get(),
            });
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
        let pending = Pending::new(event);
        // The declared-fan-in upkeep (T08 §4.2, site 1): +1 per UNIQUE
        // dependency whose target is unfinished — the entry created at
        // the first declaration. The declarer's declarations are fixed
        // here and the count never changes while the prerequisite
        // stays unfinished (the upkeep proof: a declarer can only
        // finish after its prerequisite, whose own execution already
        // removed the entry). Before the pending moves into its lane.
        if !pending.event.depends_on.is_empty() {
            let unfinished = &self.unfinished;
            let next_seq = self.next_seq;
            for_each_unique_dep(&pending.event.depends_on, |dep| {
                if !is_executed_seq_with(unfinished, next_seq, dep.seq) {
                    *self.declared_fan_in.entry(dep.seq).or_insert(0) += 1;
                }
            });
        }
        // The class-locality index follows the insert (CA-08): one
        // O(log) set insert per scheduled event on a spine whose
        // index is live — the whole steady-state price of every
        // class-keyed transition touching only its own members. A
        // `None` index (never materialized) checks and stops.
        if let Some(index) = self.class_index.as_mut() {
            class_track_queued(
                index,
                pending.event.class,
                pending.event.field,
                pending.due,
                pending.seq,
            );
        }
        match pending.event.field {
            Field::Near => self.near_queue.push(Reverse(pending)),
            Field::Far => self.far_queue.push(Reverse(pending)),
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
        // The admission bounds, identical to [`Self::schedule`]'s and
        // REQUIRED here by the registered-⊆-declared proof (T08): a
        // held declarer is unfinished and parks on resume, so an
        // admission path that skipped held declarations would
        // undercount the declared fan-in and unbound the release the
        // F cap exists to bound.
        if spec.depends_on.len() > self.admission.max_dependencies.get() as usize {
            return Err(ScheduleError::DependencyFanOutExceeded {
                observed: spec.depends_on.len(),
                cap: self.admission.max_dependencies.get(),
            });
        }
        if let Some((prerequisite, observed)) = self.over_cap_prerequisite(&spec.depends_on) {
            return Err(ScheduleError::PrerequisiteFanInExceeded {
                prerequisite,
                observed,
                cap: self.admission.max_declared_dependents.get(),
            });
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
        // The class-locality index follows the insert (CA-08) — same
        // shape as the queued insert above.
        if let Some(index) = self.class_index.as_mut() {
            class_track_held(index, pending.event.class, seq);
        }
        // The declared-fan-in upkeep (site 1's held arm — a held
        // declarer counts exactly like a queued one; suspension
        // invariance by construction), before the pending moves into
        // the held map.
        if !pending.event.depends_on.is_empty() {
            let unfinished = &self.unfinished;
            let next_seq = self.next_seq;
            for_each_unique_dep(&pending.event.depends_on, |dep| {
                if !is_executed_seq_with(unfinished, next_seq, dep.seq) {
                    *self.declared_fan_in.entry(dep.seq).or_insert(0) += 1;
                }
            });
        }
        self.held.insert(pending.seq, pending);
        self.stats.scheduled += 1;
        self.suspension.scheduled_held_total += 1;
        Ok(id)
    }

    /// Configures the scheduler-housekeeping admission bounds (T08 —
    /// see [`HousekeepingAdmission`]). Configuration, never logical
    /// state: setting the bounds changes no fingerprint, no save
    /// byte, and no statistic — only which FUTURE declarations are
    /// admitted. Tighter caps surface over-cap graphs as typed
    /// [`ScheduleError`] refusals at the producer and over-K
    /// follow-up emissions as counted crops
    /// ([`HousekeepingStats::follow_ups_cropped`]); the defaults
    /// (D=64/F=4096/K=1024) leave headroom over every workspace
    /// fixture's observed maximum. A restored spine resumes at the
    /// defaults regardless of what the saving process had configured
    /// (the retention-policy precedent: the driver re-configures at
    /// its own configuration boundary after restore).
    pub fn set_admission_bounds(&mut self, bounds: HousekeepingAdmission) {
        self.admission = bounds;
    }

    /// The configured admission bounds (the defaults until
    /// [`Self::set_admission_bounds`] runs).
    pub fn admission_bounds(&self) -> HousekeepingAdmission {
        self.admission
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
            &mut self.class_index,
            &mut matches,
        );
        suspended += Self::suspend_from_queue(
            &mut self.far_queue,
            &mut self.held,
            &mut self.held_index,
            &mut self.class_index,
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
            // The class-locality index follows the move (CA-08):
            // blocked membership leaves, held membership arrives.
            if let Some(index) = self.class_index.as_mut() {
                class_drop_blocked(index, event.class, seq);
                class_track_held(index, event.class, seq);
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
            if let Some(index) = self.class_index.as_mut() {
                class_drop_queued(
                    index,
                    pending.event.class,
                    Field::Near,
                    pending.due,
                    pending.seq,
                );
            }
            self.unfinished.remove(&pending.seq);
            removed += 1;
        }
        for Reverse(pending) in far_dropped {
            if let Some(index) = self.class_index.as_mut() {
                class_drop_queued(
                    index,
                    pending.event.class,
                    Field::Far,
                    pending.due,
                    pending.seq,
                );
            }
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
                if let Some(index) = self.class_index.as_mut() {
                    class_drop_blocked(index, event.class, seq);
                }
                self.unfinished.remove(&seq);
                removed += 1;
            }
        }
        for seq in held_drop {
            if let Some(pending) = self.held.remove(&seq) {
                if let Some(index) = self.class_index.as_mut() {
                    class_drop_held(index, pending.event.class, seq);
                }
                self.held_index.remove(&(pending.due, pending.seq));
                self.unfinished.remove(&seq);
                removed += 1;
            }
        }
        let mut inserted = 0u32;
        for event in events {
            let pending = Pending::new(event);
            if let Some(index) = self.class_index.as_mut() {
                class_track_held(index, pending.event.class, pending.seq);
            }
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
        // The declared-fan-in recount (T08 §4.2, site 3 — the
        // replacement path): the removals and installs change which
        // unfinished events declare which prerequisites, so every
        // count is recomputed against the current canonical specs —
        // queued, blocked, AND held declarers all count (the count
        // spans every unfinished obligation, where `waiting_on`'s
        // rebuild spans only the blocked set). The
        // rebuild-equals-maintained invariant the revalidation
        // helper pins.
        let unfinished = &self.unfinished;
        let next_seq = self.next_seq;
        rebuild_declared_fan_in(
            &mut self.declared_fan_in,
            self.near_queue
                .iter()
                .map(|Reverse(pending)| &pending.event)
                .chain(self.far_queue.iter().map(|Reverse(pending)| &pending.event))
                .chain(self.blocked.values())
                .chain(self.held.values().map(|pending| &pending.event)),
            unfinished,
            next_seq,
        );
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
            // The class-locality index follows the move (CA-08):
            // held membership leaves; the routing arm decides which
            // set receives it.
            let event_class = pending.event.class;
            let event_field = pending.event.field;
            let event_due = pending.due;
            if let Some(index) = self.class_index.as_mut() {
                class_drop_held(index, event_class, seq);
            }
            if !has_unmet_dependencies_with(unfinished, next_seq, &pending.event) {
                match event_field {
                    Field::Near => self.near_queue.push(Reverse(pending)),
                    Field::Far => self.far_queue.push(Reverse(pending)),
                }
                if let Some(index) = self.class_index.as_mut() {
                    class_track_queued(index, event_class, event_field, event_due, seq);
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
                if let Some(index) = self.class_index.as_mut() {
                    class_track_blocked(index, event_class, seq);
                }
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
        class_index: &mut Option<ClassIndex>,
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
            // The class-locality index follows the move (CA-08): the
            // pending's own field names its lane (the
            // queued-field==lane invariant — every push site pushes
            // by the event's field), so the queued membership leaves
            // the right set wherever this bulk pass runs.
            if let Some(index) = class_index {
                class_drop_queued(
                    index,
                    pending.event.class,
                    pending.event.field,
                    pending.due,
                    pending.seq,
                );
                class_track_held(index, pending.event.class, pending.seq);
            }
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
            let source_field = pending.event.field;
            let (due, seq, class) = (pending.due, pending.seq, pending.event.class);
            pending.event.field = field;
            match field {
                Field::Near => self.near_queue.push(Reverse(pending)),
                Field::Far => self.far_queue.push(Reverse(pending)),
            }
            // The class-locality index follows the lane change
            // (CA-08): queued membership moves between the lane sets
            // under the same canonical key. The blocked arm below
            // changes only `field` — the blocked set is lane-blind,
            // so its membership is already correct.
            if let Some(index) = self.class_index.as_mut() {
                class_drop_queued(index, class, source_field, due, seq);
                class_track_queued(index, class, field, due, seq);
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

    // --------------------------- CA-08: the class-keyed indexed forms

    /// Materializes the class-locality index from canonical state —
    /// the F09 adoption-boundary discipline applied to the spine: the
    /// build is paid once, by the FIRST class-keyed call, and every
    /// mutation site maintains the index from then on. Idempotent;
    /// the one-pass rebuild is the same shape the restore path's
    /// first class-keyed call performs (wire-invisibility).
    fn materialize_class_index(&mut self) {
        if self.class_index.is_some() {
            return;
        }
        let mut index: ClassIndex = BTreeMap::new();
        for Reverse(pending) in self.near_queue.iter() {
            class_track_queued(
                &mut index,
                pending.event.class,
                Field::Near,
                pending.due,
                pending.seq,
            );
        }
        for Reverse(pending) in self.far_queue.iter() {
            class_track_queued(
                &mut index,
                pending.event.class,
                Field::Far,
                pending.due,
                pending.seq,
            );
        }
        for (seq, event) in &self.blocked {
            class_track_blocked(&mut index, event.class, *seq);
        }
        for (seq, pending) in &self.held {
            class_track_held(&mut index, pending.event.class, *seq);
        }
        self.class_index = Some(index);
    }

    /// Suspends every queued and blocked obligation of `class` into
    /// the held set — the INDEXED form of
    /// [`Self::suspend_matching`] with the predicate
    /// `|view| view.class == class` (CA-08/F08: the known
    /// region/identity transitions). Answers are IDENTICAL to the
    /// predicate form's: the same entries move with full identity,
    /// the same count returns, the same statistics move — pinned by
    /// the equivalence battery across fixture shapes, save/restore
    /// boundaries included.
    ///
    /// WHY NO SORT (the audit's own distinction — "ordering is
    /// required for the public stateful predicate"): this path runs
    /// no public predicate. The selection is the membership itself —
    /// exactly the predicate form's extension — and every downstream
    /// observable (the moved multiset, the held keys, save bytes,
    /// fingerprints, the returned count) derives from canonical
    /// keys, never from visitation order. The lane partition is the
    /// A08a bulk shape minus the canonical sort: `into_vec`,
    /// membership-keyed `extract_if`, `BinaryHeap::from` — zero
    /// scratch allocation. A class with no queued members in a lane
    /// costs ONE set lookup (the early exit — the dormant-region
    /// common case); the buffer partition that remains when members
    /// exist is the `BinaryHeap`'s own bound (no keyed removal
    /// exists for a heap; the O(N log N) sort and the per-entry
    /// predicate calls are what this path removes).
    pub fn suspend_class(&mut self, class: EventClass) -> u32 {
        self.materialize_class_index();
        // The member sets move OUT with the entries they name
        // (`mem::take` — allocation-free, and the post-transition
        // membership is exactly the emptied set, the held arm
        // re-filling `held` below).
        let mut near_members = BTreeSet::new();
        let mut far_members = BTreeSet::new();
        let mut blocked_members = BTreeSet::new();
        if let Some(membership) = self.class_index.as_mut().and_then(|i| i.get_mut(&class)) {
            near_members = std::mem::take(&mut membership.near);
            far_members = std::mem::take(&mut membership.far);
            blocked_members = std::mem::take(&mut membership.blocked);
        }
        let mut suspended = 0u32;
        suspended += Self::suspend_class_queue(
            &mut self.near_queue,
            &mut self.held,
            &mut self.held_index,
            &mut self.class_index,
            class,
            &mut near_members,
        );
        suspended += Self::suspend_class_queue(
            &mut self.far_queue,
            &mut self.held,
            &mut self.held_index,
            &mut self.class_index,
            class,
            &mut far_members,
        );
        // The blocked arm — the same mechanics as the predicate
        // form's, visited in the same canonical ascending-seq order
        // (the member set iterates it).
        let unfinished = &self.unfinished;
        let next_seq = self.next_seq;
        for seq in blocked_members {
            if let Some(event) = self.blocked.remove(&seq) {
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
                if let Some(index) = self.class_index.as_mut() {
                    class_track_held(index, event.class, seq);
                }
                let pending = Pending::new(event);
                self.held_index.insert((pending.due, pending.seq));
                self.held.insert(pending.seq, pending);
                suspended += 1;
            }
        }
        // The derived remaining-dependency counts follow the blocked
        // map's lifecycle exactly, as in the predicate form.
        let blocked = &self.blocked;
        self.remaining_deps
            .retain(|&seq, _| blocked.contains_key(&seq));
        self.suspension.suspended_total += u64::from(suspended);
        suspended
    }

    /// Bulk-partitions one lane by class membership — the indexed
    /// suspend's queue arm (see [`Self::suspend_class`]). The
    /// member set is already TAKEN from the index (the entries it
    /// names are leaving), so this pass performs no index removal —
    /// only the held-side insertions.
    fn suspend_class_queue(
        queue: &mut BinaryHeap<Reverse<Pending>>,
        held: &mut BTreeMap<u64, Pending>,
        held_index: &mut BTreeSet<(Tick, u64)>,
        class_index: &mut Option<ClassIndex>,
        class: EventClass,
        members: &mut BTreeSet<(Tick, u64)>,
    ) -> u32 {
        if members.is_empty() {
            // The early exit: a class with no queued members in this
            // lane never drains, sorts, or re-heaps it.
            return 0;
        }
        let mut suspended = 0u32;
        let mut items = std::mem::take(queue).into_vec();
        let extracted = items.extract_if(.., |Reverse(pending)| {
            members.contains(&(pending.due, pending.seq))
        });
        for Reverse(pending) in extracted {
            held_index.insert((pending.due, pending.seq));
            if let Some(index) = class_index {
                class_track_held(index, class, pending.seq);
            }
            held.insert(pending.seq, pending);
            suspended += 1;
        }
        *queue = BinaryHeap::from(items);
        suspended
    }

    /// Returns matching held obligations of `class` to pending state
    /// — the INDEXED form of [`Self::resume_matching`] with the
    /// predicate `|view| view.class == class` (CA-08). Identity,
    /// routing, re-parking, statistics, and the returned count are
    /// IDENTICAL to the predicate form's; the visited order is the
    /// same canonical ascending-seq order (the member set iterates
    /// it), and the held map's keyed removal makes the walk FLAT in
    /// the unrelated held population — O(log + own) per member, no
    /// scan of foreign entries.
    pub fn resume_class(&mut self, class: EventClass) -> u32 {
        self.materialize_class_index();
        let mut held_members = BTreeSet::new();
        if let Some(membership) = self.class_index.as_mut().and_then(|i| i.get_mut(&class)) {
            held_members = std::mem::take(&mut membership.held);
        }
        let unfinished = &self.unfinished;
        let next_seq = self.next_seq;
        let mut resumed = 0u32;
        for seq in held_members {
            let Some(pending) = self.held.remove(&seq) else {
                continue;
            };
            self.held_index.remove(&(pending.due, pending.seq));
            let event_class = pending.event.class;
            let event_field = pending.event.field;
            let event_due = pending.due;
            if let Some(index) = self.class_index.as_mut() {
                class_drop_held(index, event_class, seq);
            }
            if !has_unmet_dependencies_with(unfinished, next_seq, &pending.event) {
                match event_field {
                    Field::Near => self.near_queue.push(Reverse(pending)),
                    Field::Far => self.far_queue.push(Reverse(pending)),
                }
                if let Some(index) = self.class_index.as_mut() {
                    class_track_queued(index, event_class, event_field, event_due, seq);
                }
            } else {
                let remaining = register_waiters_for_unmet_into(
                    &mut self.waiting_on,
                    seq,
                    &pending.event,
                    |dep| dep < next_seq && !unfinished.contains(&dep),
                );
                self.housekeeping_stats.dependency_registrations += u64::from(remaining);
                self.remaining_deps.insert(seq, remaining);
                self.blocked.insert(seq, pending.event);
                if let Some(index) = self.class_index.as_mut() {
                    class_track_blocked(index, event_class, seq);
                }
            }
            resumed += 1;
        }
        self.suspension.resumed_total += u64::from(resumed);
        resumed
    }

    /// Moves every queued and blocked obligation of `class` to the
    /// target lane — the INDEXED form of [`Self::reclassify_matching`]
    /// with the predicate `|view| view.class == class` (CA-08).
    /// Identity, lane routing, the blocked field change, and the
    /// returned count are IDENTICAL to the predicate form's. The
    /// source-lane partition is the same membership-keyed bulk shape
    /// as [`Self::suspend_class`] (no canonical sort — no public
    /// predicate runs; the same-lane skip and the empty-membership
    /// early exit included).
    pub fn reclassify_class(&mut self, class: EventClass, field: Field) -> u32 {
        self.materialize_class_index();
        let mut moved: Vec<Pending> = Vec::new();
        for source in [Field::Near, Field::Far] {
            if source == field {
                continue;
            }
            let mut members = BTreeSet::new();
            if let Some(membership) = self.class_index.as_mut().and_then(|i| i.get_mut(&class)) {
                members = std::mem::take(match source {
                    Field::Near => &mut membership.near,
                    Field::Far => &mut membership.far,
                });
            }
            if members.is_empty() {
                continue;
            }
            let queue = match source {
                Field::Near => &mut self.near_queue,
                Field::Far => &mut self.far_queue,
            };
            let mut items = std::mem::take(queue).into_vec();
            let extracted = items.extract_if(.., |Reverse(p)| members.contains(&(p.due, p.seq)));
            for Reverse(pending) in extracted {
                moved.push(pending);
            }
            *queue = BinaryHeap::from(items);
        }
        let mut reclassified = moved.len() as u32;
        for mut pending in moved {
            let source_field = pending.event.field;
            let (due, seq, event_class) = (pending.due, pending.seq, pending.event.class);
            pending.event.field = field;
            match field {
                Field::Near => self.near_queue.push(Reverse(pending)),
                Field::Far => self.far_queue.push(Reverse(pending)),
            }
            if let Some(index) = self.class_index.as_mut() {
                class_drop_queued(index, event_class, source_field, due, seq);
                class_track_queued(index, event_class, field, due, seq);
            }
        }
        // The blocked arm: the class's blocked members, ascending —
        // the same selection and field change as the predicate form.
        // The member set is taken and RESTORED (blocked membership
        // is unchanged by a lane change), so the pass borrows nothing
        // from the index while mutating `blocked`.
        let mut blocked_members = BTreeSet::new();
        if let Some(membership) = self.class_index.as_mut().and_then(|i| i.get_mut(&class)) {
            blocked_members = std::mem::take(&mut membership.blocked);
        }
        for seq in blocked_members.iter() {
            if let Some(event) = self.blocked.get_mut(seq)
                && event.field != field
            {
                event.field = field;
                reclassified += 1;
            }
        }
        if let Some(membership) = self.class_index.as_mut().and_then(|i| i.get_mut(&class)) {
            membership.blocked = blocked_members;
        }
        self.suspension.reclassified_total += u64::from(reclassified);
        reclassified
    }

    /// Read-only held-obligation views of ONE class, in canonical seq
    /// order — the indexed form of `held_view()` plus a class filter
    /// (CA-08; the region director's deactivation gathering). FLAT in
    /// the unrelated held population once the index is live: the
    /// member seqs resolve through the held map's keyed lookup,
    /// O(log + own) per member. Before materialization the scan form
    /// runs — identical output by the same equivalence battery, so a
    /// cold caller pays the scan exactly once (the deactivation
    /// cadence suspends first, which materializes).
    pub fn held_class_view(&self, class: EventClass) -> Vec<ObligationView<'_>> {
        if let Some(membership) = self
            .class_index
            .as_ref()
            .and_then(|index| index.get(&class))
        {
            return membership
                .held
                .iter()
                .filter_map(|seq| self.held.get(seq))
                .map(|pending| pending.view(ObligationLocation::Held))
                .collect();
        }
        self.held_view()
            .into_iter()
            .filter(|view| view.class == class)
            .collect()
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
        // The per-tick report scratch resets (T08 §5.4): a tick with
        // no exhaustion break reports zero backlog samples.
        self.backlog_at_break_near = 0;
        self.backlog_at_break_far = 0;
        // The per-tick registration delta's base (T08 §5.2): every
        // registration the four drain phases below perform lands in
        // `dependency_registrations`, so the tick's own total is the
        // delta across them — the worst published beside
        // `worst_tick_housekeeping`.
        let registrations_before = self.housekeeping_stats.dependency_registrations;
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
        // The registration slope's per-tick worst (T08 §5.2),
        // mirroring the blended worst above: the edge fan-out the
        // one-unit park charge deliberately does not scale with,
        // published per tick so the slope is observable without
        // tripping anything.
        let tick_registrations =
            self.housekeeping_stats.dependency_registrations - registrations_before;
        self.housekeeping_stats.worst_tick_dependency_registrations = self
            .housekeeping_stats
            .worst_tick_dependency_registrations
            .max(tick_registrations);
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
            // The budget break's snapshot (T08 §5.4): the execution
            // budget is an exhaustion break exactly like the
            // housekeeping allowance's — the most recent break's
            // queue lengths are what the tick's report carries, so a
            // budget-bound tick (the fixed-budget drain shape)
            // publishes its backlog slope through the same fields.
            self.backlog_at_break_near = self.near_queue.len() as u64;
            self.backlog_at_break_far = self.far_queue.len() as u64;
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
            housekeeping_backlog_near: self.backlog_at_break_near,
            housekeeping_backlog_far: self.backlog_at_break_far,
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
    /// automatic path — carries its own eligibility requirement (the
    /// T07 two-class model: [`ConsequenceFreedom`] or the frontier,
    /// CA-07).
    ///
    /// **THE LOGICAL HEAD (T07 §7's Option 4 — the drain
    /// replacement)**: the prefix is hashed first (read-only, the
    /// same chain bytes as ever), then the cut becomes a `head` bump
    /// — the summarized prefix stays physically present as the DEAD
    /// prefix `[..head]` while every public view exposes the live
    /// tail `[head..]`, so the cut itself moves ZERO bytes and
    /// allocates nothing. An amortized compact
    /// (`Vec::drain(..head)` — still allocation-free, the high-water
    /// capacity RETAINED, as with the pre-T07 drain shape) fires
    /// when `head` reaches the live tail's size: the classic
    /// amortization bounding total record moves by total discarded
    /// records over any prefix of the run (each record is physically
    /// moved at most once per discard batch), at the documented
    /// worst-case physical residency of `2 × live` — never data
    /// discarded to hit it. Wire identity: the `spine-history/2`
    /// section encodes the live tail only, so a live spine with a
    /// dead prefix and its restored tight image encode IDENTICAL
    /// bytes (pinned by the dead-prefix round-trip test).
    pub fn compact(&mut self, retain: u64) -> Result<HistoryKind, CompactionError> {
        let ledger_len = (self.ledger.len() - self.head) as u64;
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
        for record in &self.ledger[self.head..self.head + cut as usize] {
            let mut hasher = Sha256::new();
            hasher.update(&chain);
            hasher.update(&record_wire_bytes(record));
            chain = hasher.finish();
        }
        // The logical-head bump: the summarized prefix becomes dead
        // physical slack — zero bytes moved, zero allocations, the
        // high-water capacity retained.
        self.head += cut as usize;
        // The amortized compact (the classic front-vs-back
        // amortization): when the dead prefix reaches the live
        // tail's size, one drain bounds the total moves by the total
        // discards — each surviving record is physically moved at
        // most once per discard batch, and the buffer never carries
        // more than `2 × live` records at the trigger.
        if self.head >= self.ledger.len() - self.head {
            self.ledger.drain(..self.head);
            self.head = 0;
        }
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
    /// see [`RetentionPolicy`]) with the PERMANENTLY
    /// CONSEQUENCE-FREE world-class capability (T07's Class 1 —
    /// CA-07): the cadence this arms is the consequence-free cut —
    /// the capability is the composition boundary's explicit
    /// assertion that NO application consumer of execution-history
    /// records exists or will ever exist in this composition (the
    /// eligible prefix unbounded by construction of the
    /// application), required at every (re)configuration (a restored
    /// spine re-asserts before its next compact; the cadence never
    /// fires on standing configuration a prior process once vouched
    /// for). Consequence-bearing worlds — any composition whose
    /// lanes read history — arm through
    /// [`Self::arm_retention_frontier`] instead, and the cadence
    /// cuts only up to the minimum consumer acknowledgment. Setting
    /// a policy never changes logical state — the fingerprint
    /// domains and save bytes are untouched by configuration alone;
    /// only a FIRING changes the history representation, through
    /// the same authenticated cut the explicit [`Spine::compact`]
    /// performs.
    pub fn set_retention_policy(
        &mut self,
        policy: RetentionPolicy,
        eligibility: ConsequenceFreedom,
    ) {
        self.retention = Some(policy);
        self.retention_eligibility = Some(RetentionEligibility::Freedom(eligibility));
    }

    /// Arms the consequence-bearing retention frontier (T07's
    /// Class 2): the cadence fires only up to the MINIMUM consumer
    /// acknowledgment, never on standing configuration. The
    /// frontier is IN-MEMORY per-consumer monotone state — named
    /// consumers acknowledge the prefix they can relinquish through
    /// [`Self::register_retention_consumer`] and
    /// [`Self::acknowledge_retention_through`], and the checkpoint's
    /// cut is clamped to `min(excess, chunk, eligible −
    /// summarized)` where `eligible` is the minimum acknowledgment.
    ///
    /// **THE EMPTY-FRONTIER RULE (fail-closed):** arming with ZERO
    /// registered consumers NEVER cuts — `min(∅)` is undefined and
    /// the design defines it as refusal, never as permission. A
    /// driver that arms the frontier and forgets to register its
    /// consumers sees [`RetentionStats::frontier_hold_ticks`] grow,
    /// not cuts; this is deliberate, and the register calls belong
    /// at the same configuration boundary as the arming. Arming is
    /// fresh: any prior eligibility (Class 1 capability or an
    /// earlier frontier's consumer registry) is replaced, and the
    /// new registry starts empty.
    ///
    /// The frontier never rides the wire (the CA-07 why-not,
    /// extended from the boolean token to every consumer ack): on
    /// restore every consumer re-registers and re-establishes its
    /// acknowledgment FROM ITS OWN DURABLE STATE — a restored
    /// process can never resume cutting on a vouch a prior process
    /// made and this one did not re-make. Setting the frontier
    /// never changes logical state; only a FIRING does, through the
    /// same authenticated cut the explicit [`Spine::compact`]
    /// performs.
    pub fn arm_retention_frontier(&mut self, policy: RetentionPolicy) {
        self.retention = Some(policy);
        self.retention_eligibility = Some(RetentionEligibility::Frontier {
            consumers: BTreeMap::new(),
        });
    }

    /// Registers a retention consumer onto the frontier (T07's
    /// Class 2 — the register rule): the consumer joins with
    /// `vouches_through`, its own durable read point in the GLOBAL
    /// EXECUTED ORDINAL space (record *n* is the *n*-th execution of
    /// the spine's lifetime — the same count space
    /// `summarized_records` already rides on the wire, so
    /// acknowledgments survive restore as ordinals even though the
    /// registry itself does not). The register rule: a joining
    /// consumer vouches for the STANDING cut — `vouches_through`
    /// must be at or above the current `summarized_records`, because
    /// the already-summarized prefix is unrecoverable and a consumer
    /// that still needs those records cannot be served (a typed
    /// [`RetentionRegisterError::NeedsSummarizedRecords`] refusal —
    /// the join-time impossibility confronted, never discovered at
    /// read time; the driver must handle it, the engine cannot make
    /// history reconstructible). A fresh consumer on an uncut spine
    /// registers at zero.
    ///
    /// The initial acknowledgment is the offered vouch (the §2.2
    /// sketch's "the current summarized prefix" is the register
    /// rule's FLOOR, not its ceiling: a consumer whose durable state
    /// already absorbed past the standing cut joins at its true
    /// point). An over-ack is the caller's trust boundary — the
    /// engine cannot verify a consumer read what it vouches (the
    /// design's honest limit); an under-ack merely holds the
    /// frontier.
    pub fn register_retention_consumer(
        &mut self,
        id: RetentionConsumerId,
        vouches_through: u64,
    ) -> Result<(), RetentionRegisterError> {
        let summarized = self.standing_summarized_records();
        if vouches_through < summarized {
            return Err(RetentionRegisterError::NeedsSummarizedRecords {
                id,
                summarized_records: summarized,
                consumer_read_point: vouches_through,
            });
        }
        let Some(RetentionEligibility::Frontier { consumers }) = &mut self.retention_eligibility
        else {
            // The registry belongs to the frontier mode: registering
            // onto a spine armed with the Class-1 capability (or
            // with only a declared bound) is a wiring mistake
            // surfaced typed — never a silent registry entry the
            // checkpoint would ignore.
            return Err(RetentionRegisterError::FrontierNotArmed { id });
        };
        if let Some(&acknowledged_through) = consumers.get(&id) {
            return Err(RetentionRegisterError::AlreadyRegistered {
                id,
                acknowledged_through,
            });
        }
        consumers.insert(id, vouches_through);
        Ok(())
    }

    /// Advances a registered consumer's acknowledgment through
    /// global executed ordinal `through` (the first `through`
    /// execution records of the lifetime are releasable by this
    /// consumer's vouch). MONOTONE and fail-closed: a regressing
    /// value is a typed [`RetentionAckError::Regression`] refusal
    /// (monotonicity is enforced, never assumed), an unknown
    /// consumer a typed [`RetentionAckError::UnknownConsumer`]
    /// refusal (the registry is explicit). A consumer acknowledging
    /// only what it has DURABLY processed holds the frontier through
    /// every unknown/indeterminate commit — the engine never
    /// resolves an unknown into an ack (the persist side's
    /// settled-transaction watermark rule).
    ///
    /// After an explicit [`Spine::compact`] cut PAST a consumer's
    /// acknowledgment (the manual operator authority — the frontier
    /// polices only the automatic path), the consumer's vouch sits
    /// below the standing cut: the frontier accounting stays
    /// coherent fail-closed (the clamp saturates at zero) until the
    /// consumer's acknowledgment catches up past the new
    /// `summarized_records`; an ack still below the standing cut is
    /// accepted (progress toward catching up) as long as it does
    /// not regress below the consumer's own prior value.
    pub fn acknowledge_retention_through(
        &mut self,
        id: RetentionConsumerId,
        through: u64,
    ) -> Result<(), RetentionAckError> {
        let Some(RetentionEligibility::Frontier { consumers }) = &mut self.retention_eligibility
        else {
            return Err(RetentionAckError::UnknownConsumer { id });
        };
        let Some(acknowledged) = consumers.get_mut(&id) else {
            return Err(RetentionAckError::UnknownConsumer { id });
        };
        if through < *acknowledged {
            return Err(RetentionAckError::Regression {
                id,
                acknowledged_through: *acknowledged,
                attempted_through: through,
            });
        }
        *acknowledged = through;
        Ok(())
    }

    /// The per-consumer frontier lag (T07's observability surface):
    /// the consumer's distance behind the LEADING registered
    /// acknowledgment — which consumer is the gate, by name.
    /// `None` when the id is not registered (or no frontier is
    /// armed); zero when this consumer IS the leader. The lagging
    /// consumer's owner sees the ledger's growth as its own
    /// conscious decision ([`RetentionStats::refused_ticks`] and
    /// [`RetentionStats::frontier_hold_ticks`] count the held
    /// ticks; this accessor names the holder).
    pub fn retention_consumer_lag(&self, id: RetentionConsumerId) -> Option<u64> {
        let RetentionEligibility::Frontier { consumers } = self.retention_eligibility.as_ref()?
        else {
            return None;
        };
        let leading = consumers.values().copied().max()?;
        let own = *consumers.get(&id)?;
        Some(leading - own)
    }

    /// Converts an execution identity to its GLOBAL EXECUTED
    /// ORDINAL (the acknowledgment space): `Some(n)` where the
    /// record is the *n*-th execution of the spine's lifetime and
    /// sits in the RETAINED tail; `None` when the identity is not
    /// in the retained tail (not yet executed, or already
    /// summarized — the summarized prefix cannot be mapped back,
    /// by design). A driver-cadence convenience: O(live) scan of
    /// the verbatim tail, never in the tick loop; consumers that
    /// count what they read never need it.
    pub fn retention_ordinal_of(&self, id: EventId) -> Option<u64> {
        let summarized = self.standing_summarized_records();
        let ordinal = self.ledger[self.head..]
            .iter()
            .position(|record| record.id.seq == id.seq)? as u64;
        Some(summarized + ordinal + 1)
    }

    /// The standing summarized prefix (the register rule's floor):
    /// zero on a `Full` spine, the compacted history's cumulative
    /// `summarized_records` otherwise.
    fn standing_summarized_records(&self) -> u64 {
        self.compacted
            .as_ref()
            .map(|compacted| compacted.summarized_records)
            .unwrap_or(0)
    }

    /// Declares the retention bound WITHOUT any eligibility
    /// (CA-07's refusal arm): the checkpoint observes the threshold
    /// but REFUSES to cut — the refusal counted in
    /// [`RetentionStats::refused_ticks`], the ledger's growth past
    /// the bound exposed by [`Self::retention_lag_records`]. This is
    /// the state for a driver that knows its RAM bound but has not
    /// (or no longer) proven eligibility: never a silent default,
    /// never a cut on unproven standing. Arming afterwards is a
    /// fresh [`Self::set_retention_policy`] (Class 1) or
    /// [`Self::arm_retention_frontier`] (Class 2) call — the bound
    /// and the proof together.
    pub fn set_retention_bound(&mut self, policy: RetentionPolicy) {
        self.retention = Some(policy);
        // A bound change re-opens the eligibility question: a prior
        // arming vouched for cutting under the old bound's cadence,
        // so it does not survive the reconfiguration — the Class-1
        // capability is dropped AND the frontier's consumer registry
        // with it (a fresh arming re-registers; the register rule
        // re-derives every vouch from the consumers' own durable
        // state).
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

    /// Whether the configured policy's cadence is ARMED — armed with
    /// SOME eligibility (T07's two-class model): the Class-1
    /// [`ConsequenceFreedom`] capability, OR a frontier with at
    /// least one registered consumer (an armed-but-unregistered
    /// frontier reports `false` — the empty-frontier fail-closed
    /// rule; its threshold crossings are counted as refusals, never
    /// cut). A declared-but-unarmed policy (see
    /// [`Self::set_retention_bound`]) refuses instead, observably.
    pub fn retention_armed(&self) -> bool {
        match &self.retention_eligibility {
            Some(RetentionEligibility::Freedom(_)) => true,
            Some(RetentionEligibility::Frontier { consumers }) => !consumers.is_empty(),
            None => false,
        }
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
            Some(policy) => ((self.ledger.len() - self.head) as u64)
                .saturating_sub(policy.resident_bound_records()),
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
    /// **THE ELIGIBILITY GATE (CA-07, T07's two-class model):**
    /// firing additionally requires an ARMED eligibility. The
    /// Class-1 capability ([`Self::set_retention_policy`]) leaves the
    /// cut at `min(excess, chunk)` — the world class asserts the
    /// eligible prefix is unbounded. The frontier
    /// ([`Self::arm_retention_frontier`]) CLAMPS the cut to
    /// `min(excess, chunk, eligible − summarized)` where `eligible`
    /// is the minimum registered consumer acknowledgment: a fully
    /// held frontier (or an armed-but-empty one — the empty-frontier
    /// fail-closed rule) refuses exactly like an unarmed policy —
    /// counted in [`RetentionStats::refused_ticks`] AND
    /// [`RetentionStats::frontier_hold_ticks`], the shortfall
    /// exposed in [`RetentionStats::retention_frontier_lag_records`]
    /// and per consumer in [`Self::retention_consumer_lag`] — never
    /// a silent skip, never a discard to keep RAM under a target.
    /// A declared-but-unarmed policy (see [`Self::set_retention_bound`])
    /// REFUSES here when the threshold is exceeded: the cut is never
    /// made on unproven standing, and the lag is never silent.
    ///
    /// The cut is a valid compaction window BY CONSTRUCTION: the gate
    /// guarantees `excess ≥ chunk ≥ 1`, the frontier clamp only ever
    /// lowers the cut, and the firing additionally requires
    /// `cut ≥ 1`, so the window retains ≥ `retain` ≥ 1 records and
    /// summarizes ≥ 1 — neither typed refusal of [`Self::compact`]
    /// is reachable; the `expect` documents the invariant, it never
    /// fires.
    fn apply_retention_policy(&mut self) {
        let Some(policy) = self.retention else {
            return;
        };
        let ledger_len = (self.ledger.len() - self.head) as u64;
        let threshold = policy.resident_bound_records();
        if ledger_len <= threshold {
            return;
        }
        // The cut the POLICY would take at this tick, absent any
        // consumer frontier (the excess above the verbatim window,
        // chunk-bounded) — the frontier can only veto or clamp it,
        // never widen it.
        let policy_cut = (ledger_len - policy.retain.get()).min(policy.chunk.get());
        match &self.retention_eligibility {
            None => {
                self.retention_stats.refused_ticks += 1;
                if self.diag.would_log(Level::Info) {
                    self.diag.log(
                        Level::Info,
                        format!(
                            "retention threshold exceeded at {} (retained {ledger_len} > bound \
                             {threshold}) without eligibility — refusing to cut; lag {} records",
                            self.clock,
                            ledger_len - threshold
                        ),
                    );
                }
            }
            Some(RetentionEligibility::Freedom(_)) => {
                self.fire_retention(ledger_len, policy_cut);
            }
            Some(RetentionEligibility::Frontier { consumers }) => {
                // The frontier value: the minimum registered
                // acknowledgment, in global executed ordinals; the
                // releasable pool is what sits above the standing
                // summarized prefix. An empty registry is the
                // empty-frontier fail-closed rule (min(∅) is a
                // refusal, never a permission).
                let summarized = self.standing_summarized_records();
                let eligible = consumers.values().copied().min();
                let releasable = eligible.map_or(0, |ack| ack.saturating_sub(summarized));
                // The frontier lag: how many records the policy
                // could have cut but the consumers have not
                // released (zero when the frontier released the
                // policy's full want; the policy's full want when
                // the frontier holds everything).
                self.retention_stats.retention_frontier_lag_records =
                    policy_cut.saturating_sub(releasable);
                let cut = policy_cut.min(releasable);
                if cut == 0 {
                    self.retention_stats.refused_ticks += 1;
                    self.retention_stats.frontier_hold_ticks += 1;
                    if self.diag.would_log(Level::Info) {
                        let held_by = match eligible {
                            None => "no consumer registered".to_string(),
                            Some(min_ack) => format!(
                                "{} consumer(s) holding at eligible ordinal {min_ack}",
                                consumers.len()
                            ),
                        };
                        self.diag.log(
                            Level::Info,
                            format!(
                                "retention threshold exceeded at {} (retained {ledger_len} > \
                                 bound {threshold}) with the frontier held ({held_by}; \
                                 summarized {summarized}, releasable {releasable}, policy want \
                                 {policy_cut}) — refusing to cut; frontier lag {} records",
                                self.clock, self.retention_stats.retention_frontier_lag_records
                            ),
                        );
                    }
                } else {
                    self.fire_retention(ledger_len, cut);
                }
            }
        }
    }

    /// Fires one policy compaction of `cut` records (the invocation
    /// the eligibility gate admitted — Class 1 unclamped, Class 2
    /// clamped to the frontier): the shared firing tail of
    /// [`Self::apply_retention_policy`]'s two armed arms. The cut is
    /// a valid compaction window by the caller's construction
    /// (`1 ≤ cut ≤ excess = ledger_len − retain`, so the window
    /// retains ≥ `retain` ≥ 1 records and summarizes ≥ 1).
    fn fire_retention(&mut self, ledger_len: u64, cut: u64) {
        let retained = ledger_len - cut;
        self.compact(retained).expect(
            "the retention policy's cut is a valid compaction window by construction: the \
             threshold gate guarantees excess ≥ chunk ≥ 1, the frontier clamp only ever \
             lowers it, and the firing requires cut ≥ 1, so the window retains ≥ retain ≥ 1 \
             records and summarizes ≥ 1 — neither typed refusal is reachable",
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
        // The compacted domains hash the LIVE view (T07 §7): a dead
        // prefix is physical layout, never logical content — the
        // fingerprint is identical under both storage shapes.
        fp.write_usize(self.ledger.len() - self.head);
        for record in &self.ledger[self.head..] {
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
            // The class-locality index follows the pop (CA-08): the
            // entry leaves this lane whichever arm takes it —
            // execution (no membership anywhere) or a park (the
            // blocked set re-registers it below).
            if let Some(index) = self.class_index.as_mut() {
                class_drop_queued(index, pending.event.class, field, pending.due, pending.seq);
            }
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
                let (parked_class, parked_seq) = (event.class, event.id.seq);
                self.blocked.insert(event.id.seq, event);
                // The class-locality index follows the park (CA-08).
                if let Some(index) = self.class_index.as_mut() {
                    class_track_blocked(index, parked_class, parked_seq);
                }
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
            // The per-tick report snapshot at the break (T08 §5.4):
            // both lanes' queue lengths, the backlog slope's sample
            // for this tick — the most recent break's snapshot wins
            // (the budget break at the tick's end may overwrite it).
            self.backlog_at_break_near = self.near_queue.len() as u64;
            self.backlog_at_break_far = self.far_queue.len() as u64;
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
        // The declared-fan-in entry dies with the unfinished
        // membership (T08 §4.2, site 2 — the upkeep proof: a
        // declarer of this event can only finish after it, so no
        // decrement site exists; the executed event's OWN entry as a
        // prerequisite key is the only one to remove). Follow-ups
        // declaring on this just-executing producer therefore skip
        // the F check — the declaration is free, exactly the
        // follow-up-on-producer shape the design names.
        self.declared_fan_in.remove(&event.id.seq);
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
        let (mut follow_ups, crops) = {
            let unfinished = &self.unfinished;
            let next_seq = self.next_seq;
            let mut ctx = ExecCtx {
                executed_tick,
                unfinished,
                next_seq,
                field: event.field,
                follow_ups: std::mem::take(&mut self.scratch_follow_ups),
                // The K admission bound's per-execution budget (T08
                // §4.3): the slots this execution may admit through
                // `follow_up`/`schedule_at`/`follow_up_in_field`,
                // cropped (counted, never silent) beyond it.
                admissions_remaining: self.admission.max_follow_ups.get(),
                crops: 0,
            };
            self.world.execute(&event, &mut ctx);
            (ctx.drain(), ctx.crops)
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
        // The K crop's publication (T08 §4.3): a config outcome in
        // the observational class, fail-visible — never silent.
        if crops > 0 {
            self.housekeeping_stats.follow_ups_cropped += u64::from(crops);
            if self.diag.would_log(Level::Error) {
                self.diag.log(
                    Level::Error,
                    format!(
                        "cropped {crops} follow-up(s) beyond the admission cap from {id} — \
                         raise the cap through set_admission_bounds or restructure the handler"
                    ),
                );
            }
        }

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
                    ReleaseAttempt::NotBlocked => {
                        // Charged as the visit it performed (T08
                        // §5.1): the arm is defensively unreachable
                        // through the public API (the lifecycle
                        // keeps registrations inside the blocked
                        // set), but an uncharged visit class is
                        // exactly the "observations do not bound
                        // work" shape — the charge identity
                        // `waiter_visits == decrements + unblocks +
                        // not_blocked` closes only with this arm
                        // counted.
                        self.housekeeping_stats.release_not_blocked += 1;
                        released += 1;
                    }
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
        // The class-locality index follows the release (CA-08):
        // blocked membership leaves, queued membership arrives under
        // the event's own field (the queued-field==lane invariant).
        if let Some(index) = self.class_index.as_mut() {
            class_drop_blocked(index, event.class, seq);
            class_track_queued(index, event.class, event.field, due, seq);
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
// remaining-dependency counts), `class_index` (CA-08's class-locality
// membership, rebuilt on the first class-keyed call) — is rebuilt from
// those canonical sources through the same code paths the live spine
// uses. Observational — `LaneReport`, `LaneTotals`, `SuspensionStats`,
// `RetentionStats`, `Diagnostics` — is not logical state: a restored
// spine resumes them at defaults, they never affect scheduling,
// accounting, or fingerprints, and the boundary is documented in the
// wire spec. The retention POLICY
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
            // The logical head always rebuilds tight (T07 §7): the
            // wire encodes the live tail only, so a restored spine
            // carries no dead prefix.
            head: 0,
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
            backlog_at_break_near: 0,
            backlog_at_break_far: 0,
            scratch_follow_ups: Vec::new(),
            // Wire-invisible (CA-08): the class-locality index never
            // reaches the save — a restored spine reconstructs it on
            // its first class-keyed call, from these canonical parts.
            class_index: None,
            // The derived declared-fan-in counts rebuild below, from
            // the canonical specs of every unfinished obligation (the
            // `remaining_deps` rebuild's precedent) — wire-invisible
            // by construction, and pre-reserved so the rebuild itself
            // allocates no more than the entry count needs.
            declared_fan_in: HashMap::with_capacity_and_hasher(
                DECLARED_FAN_IN_RESERVE,
                RemainingDepsHasherBuilder,
            ),
            // The admission bounds reset to the defaults (the
            // retention-policy precedent: configuration never rides
            // the wire; the driver re-configures after restore).
            admission: HousekeepingAdmission::default(),
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

        // The derived declared-fan-in counts (T08 §4.2, site 4):
        // rebuilt from the canonical specs of every unfinished
        // obligation — queued, blocked, and held — the same pure
        // function the live admission path maintains (the
        // `remaining_deps` precedent). Restore is fail-closed on
        // corruption, never on config: the rebuild is faithful to
        // canonical state and does NOT enforce the caps (a pre-cap
        // save with a >F waiter list is admitted as-is; its legacy
        // atomic release performs once, bounded by the saved graph's
        // actual fan-in — the documented restore contract).
        let unfinished = &spine.unfinished;
        let next_seq = spine.next_seq;
        rebuild_declared_fan_in(
            &mut spine.declared_fan_in,
            spine
                .near_queue
                .iter()
                .map(|Reverse(pending)| &pending.event)
                .chain(
                    spine
                        .far_queue
                        .iter()
                        .map(|Reverse(pending)| &pending.event),
                )
                .chain(spine.blocked.values())
                .chain(spine.held.values().map(|pending| &pending.event)),
            unfinished,
            next_seq,
        );

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
        // The v2 section encodes the LIVE tail only (T07 §7): the
        // dead prefix is pure physical slack, never serialized — a
        // live spine with a dead prefix and its restored tight image
        // encode identical bytes.
        enc.write_count(self.ledger.len() - self.head);
        for record in &self.ledger[self.head..] {
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
        // one counted outcome — the CLOSED CHARGE IDENTITY since the
        // `NotBlocked` arm was charged (T08 §5.1): all three release
        // classes count, so the loop's true iteration count can
        // never hide behind an uncharged class.
        assert_eq!(
            counters.release_not_blocked, 0,
            "the no-op arm is defensively unreachable through the public API"
        );
        assert_eq!(
            counters.waiter_visits,
            counters.release_decrements + counters.release_unblocks + counters.release_not_blocked,
            "no release visit is lost mid-loop (the atomic-release charge identity)"
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
        assert_eq!(
            tick1.worst_tick_dependency_registrations, 30,
            "the registration slope's per-tick worst (T08 §5.2) — thirty per tick here"
        );
        // The per-tick backlog samples (T08 §5.4): the report fields
        // carry the slope the hand-sampled near_pending_count
        // assertions below already pin — 61 queued at the tick's last
        // break.
        assert_eq!(spine.lane_report().housekeeping_backlog_near, 61);
        assert_eq!(spine.lane_report().housekeeping_backlog_far, 0);
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
            tick2.worst_tick_dependency_registrations, 30,
            "the per-tick worst, not the cumulative — each tick registers thirty"
        );
        assert_eq!(spine.lane_report().housekeeping_backlog_near, 31);
        assert_eq!(
            spine.near_pending_count(),
            31,
            "the slope: 31 queued after two ticks"
        );
        spine.run_tick();
        let tick3 = spine.housekeeping_stats();
        assert_eq!(tick3.parks, 90, "the whole herd parked");
        assert_eq!(tick3.worst_tick_dependency_registrations, 30);
        // Tick 3's LAST break is the guaranteed phase's (16 queued
        // at that break); the redistribution phase parks the rest
        // without breaking — no due work remains beyond the
        // far-due prerequisite — so the snapshot keeps the
        // mid-tick value, the documented O(1) upper bound.
        assert_eq!(spine.lane_report().housekeeping_backlog_near, 16);
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
        spine.run_ticks(96);
        spine.run_tick();
        // The release tick's report: BOTH exhaustion classes fired
        // (the housekeeping break mid-drain at 90 re-queued, the
        // budget break at the tick's end after the redistribution
        // phase drained nine more) — the most recent break's
        // snapshot wins, so the fields carry the end-of-tick
        // remaining backlog, the slope's sample for this tick.
        let release_report = spine.lane_report();
        assert_eq!(release_report.housekeeping_backlog_near, 81);
        assert_eq!(release_report.housekeeping_backlog_far, 0);
        assert!(release_report.near_backlogged);
        assert_eq!(
            spine.stats().executed,
            10,
            "the prerequisite plus the nine the redistribution phase admitted within the budget"
        );
        spine.run_to_completion(200);
        assert_eq!(spine.stats().executed, 91, "the storm drains completely");
        let final_counters = spine.housekeeping_stats();
        assert_eq!(final_counters.parks, 90);
        assert_eq!(final_counters.dependency_registrations, 90);
        assert_eq!(
            final_counters.worst_tick_dependency_registrations, 30,
            "the worst per-tick registration slope across the whole run"
        );
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

    // ---------------------------- T08 admission-bound tests (Option A)
    //
    // Every battery names its oracle and asserts EXACT accepted
    // execution sequences or EXACT typed outcomes — integers, never
    // wall-clock (the E-8 discipline; §7.12): every decision the
    // caps make is a pure function of canonical state + config.

    /// The admission-bounds fixture shorthand (production callers
    /// construct the `NonZeroU32`s at their configuration boundary).
    fn admission(d: u64, f: u64, k: u64) -> HousekeepingAdmission {
        HousekeepingAdmission {
            max_dependencies: NonZeroU32::new(u32::try_from(d).expect("d fits u32"))
                .expect("d is non-zero"),
            max_declared_dependents: NonZeroU32::new(u32::try_from(f).expect("f fits u32"))
                .expect("f is non-zero"),
            max_follow_ups: NonZeroU32::new(u32::try_from(k).expect("k fits u32"))
                .expect("k is non-zero"),
        }
    }

    /// THE MAP-INVARIANT ORACLE (T08 §4.2, the
    /// `remaining_counts_match_full_revalidation` discipline): the
    /// derived `declared_fan_in` map equals the from-scratch
    /// re-derivation — for every unfinished prerequisite, the number
    /// of unfinished events (queued, blocked, OR held) whose
    /// `depends_on` declares it, unique ids counted once — computed
    /// here through an independent BTreeMap accumulator, never the
    /// production rebuild path.
    fn declared_fan_in_matches_revalidation<W: World>(spine: &Spine<W>) {
        let mut expected: BTreeMap<u64, u32> = BTreeMap::new();
        let mut count_declarations = |event: &Event| {
            let mut seen: BTreeSet<u64> = BTreeSet::new();
            for dep in &event.depends_on {
                if !spine.is_executed_seq(dep.seq) && seen.insert(dep.seq) {
                    *expected.entry(dep.seq).or_insert(0) += 1;
                }
            }
        };
        for Reverse(pending) in spine.near_queue.iter() {
            count_declarations(&pending.event);
        }
        for Reverse(pending) in spine.far_queue.iter() {
            count_declarations(&pending.event);
        }
        for event in spine.blocked.values() {
            count_declarations(event);
        }
        for pending in spine.held.values() {
            count_declarations(&pending.event);
        }
        assert_eq!(
            spine.declared_fan_in.len(),
            expected.len(),
            "the derived declared-fan-in map lives on exactly the \
             declared unfinished prerequisites"
        );
        for (prerequisite, &declared) in &expected {
            assert_eq!(
                spine.declared_fan_in.get(prerequisite),
                Some(&declared),
                "declared_fan_in[{prerequisite}] must equal the full \
                 re-derivation ({declared})"
            );
        }
    }

    /// The release charge identity (T08 §5.1), asserted at battery
    /// boundaries: every waiter visit resolves to exactly one
    /// counted outcome.
    fn assert_release_charge_identity<W: World>(spine: &Spine<W>) {
        let counters = spine.housekeeping_stats();
        assert_eq!(
            counters.waiter_visits,
            counters.release_decrements + counters.release_unblocks + counters.release_not_blocked,
            "the release charge identity at this boundary"
        );
    }

    /// The before-mutations proof's observable snapshot: the dense
    /// frontier, both lanes, the blocked and held sets, every
    /// fingerprinted Stats counter, and every observational
    /// HousekeepingStats counter (all `Copy + PartialEq`).
    fn admission_state<W: World>(
        spine: &Spine<W>,
    ) -> (u64, usize, usize, usize, usize, Stats, HousekeepingStats) {
        (
            spine.next_seq,
            spine.near_queue.len(),
            spine.far_queue.len(),
            spine.blocked.len(),
            spine.held.len(),
            spine.stats(),
            spine.housekeeping_stats(),
        )
    }

    /// §7.1 — THE FAN-OUT REFUSAL (D): a declaration longer than the
    /// cap is refused typed BEFORE the identity is allocated (the
    /// refused spec leaves zero trace — no seq consumed, no queue
    /// push, no `unfinished` insert, no counter moves, no map
    /// mutation), a declaration at exactly the cap is admitted, and
    /// DUPLICATED ids are admitted at the raw-length check (three
    /// raw entries, two unique — the registration dedup counts each
    /// prerequisite once).
    #[test]
    fn dependency_declarations_are_capped_before_seq_allocation() {
        let mut spine = Spine::new(NoopWorld, 100);
        spine.set_admission_bounds(admission(2, 4096, 1024));
        let a = spine.schedule(spec(0, Field::Near, 50, 0)).unwrap();
        let b = spine.schedule(spec(0, Field::Near, 50, 0)).unwrap();
        let c = spine.schedule(spec(0, Field::Near, 50, 0)).unwrap();
        // Three raw dependencies at D=2: the typed refusal.
        let state_before = admission_state(&spine);
        let map_before = spine.declared_fan_in.clone();
        assert_eq!(
            spine.schedule(dep_spec(1, Field::Near, 1, 0, vec![a, b, c])),
            Err(ScheduleError::DependencyFanOutExceeded {
                observed: 3,
                cap: 2
            })
        );
        assert_eq!(
            admission_state(&spine),
            state_before,
            "the refused spec leaves zero trace"
        );
        assert_eq!(spine.declared_fan_in, map_before);
        declared_fan_in_matches_revalidation(&spine);
        // At exactly D: admitted — the dense frontier untouched by
        // the refusal (seq 3, not 4).
        let admitted = spine
            .schedule(dep_spec(1, Field::Near, 1, 0, vec![a, b]))
            .unwrap();
        assert_eq!(admitted.seq, 3);
        // Duplicates — the lanes.rs fixture's `[first, last, first]`
        // shape: three raw entries, two unique. The RAW-length check
        // is the mechanism (§3.1: raw ≥ unique, O(1)), so at D=2 the
        // three raw entries are refused exactly like the first case;
        // at D=3 the same spec is admitted and each unique
        // prerequisite counts ONCE in the map (the registration
        // dedup — the design §7.1's duplicate example read "admitted
        // at D=2", which contradicts the raw check its own
        // parenthetical names; the mechanism wins and the example's
        // cap is corrected here, recorded in the lane notes).
        assert_eq!(
            spine.schedule(dep_spec(1, Field::Near, 1, 0, vec![a, a, b])),
            Err(ScheduleError::DependencyFanOutExceeded {
                observed: 3,
                cap: 2
            }),
            "the raw length is what D checks — duplicates do not shrink it"
        );
        spine.set_admission_bounds(admission(3, 4096, 1024));
        let duplicated = spine
            .schedule(dep_spec(1, Field::Near, 1, 0, vec![a, a, b]))
            .unwrap();
        assert_eq!(duplicated.seq, 4);
        assert_eq!(spine.declared_fan_in.get(&a.seq), Some(&2));
        assert_eq!(spine.declared_fan_in.get(&b.seq), Some(&2));
        assert!(!spine.declared_fan_in.contains_key(&c.seq));
        declared_fan_in_matches_revalidation(&spine);
        assert_release_charge_identity(&spine);
        // Configuration alone is fingerprint-invisible (the
        // retention-policy class).
        let fingerprint_before = spine.engine_fingerprint();
        spine.set_admission_bounds(admission(5, 8, 3));
        assert_eq!(spine.engine_fingerprint(), fingerprint_before);
        assert_eq!(spine.admission_bounds(), admission(5, 8, 3));
    }

    /// §7.2 part 1 — THE FAN-IN REFUSAL (F): the (F+1)-th declarer on
    /// one prerequisite is refused typed before mutation, the first F
    /// admitted, the before-mutations proof as in §7.1, and the cap
    /// is PER-PREREQUISITE (a fresh prerequisite admits at the same
    /// configured bound).
    #[test]
    fn prerequisite_fan_in_is_capped_before_seq_allocation() {
        let mut spine = Spine::new(NoopWorld, 100);
        spine.set_admission_bounds(admission(64, 4, 1024));
        let prereq = spine.schedule(spec(0, Field::Near, 50, 9999)).unwrap();
        for i in 0..4u64 {
            assert!(
                spine
                    .schedule(dep_spec(1, Field::Near, 1, i, vec![prereq]))
                    .is_ok()
            );
        }
        assert_eq!(spine.declared_fan_in.get(&prereq.seq), Some(&4));
        // The fifth declarer: the typed refusal, pre-mutation.
        let state_before = admission_state(&spine);
        let map_before = spine.declared_fan_in.clone();
        assert_eq!(
            spine.schedule(dep_spec(1, Field::Near, 1, 4, vec![prereq])),
            Err(ScheduleError::PrerequisiteFanInExceeded {
                prerequisite: prereq,
                observed: 5,
                cap: 4
            })
        );
        assert_eq!(
            admission_state(&spine),
            state_before,
            "the refused spec leaves zero trace"
        );
        assert_eq!(spine.declared_fan_in, map_before);
        declared_fan_in_matches_revalidation(&spine);
        assert_release_charge_identity(&spine);
        // The cap is per-prerequisite: a fresh one admits.
        let other = spine.schedule(spec(0, Field::Near, 50, 0)).unwrap();
        assert!(
            spine
                .schedule(dep_spec(2, Field::Near, 1, 5, vec![other]))
                .is_ok()
        );
        assert_eq!(spine.declared_fan_in.get(&other.seq), Some(&1));
        declared_fan_in_matches_revalidation(&spine);
    }

    /// §7.2 part 2 — THE BOUND PROVEN: with F configured small (8)
    /// and a herd admitted up to it, the prerequisite's execution
    /// visits EXACTLY F waiters — the admission bound, not the
    /// allowance, is the atomic release's ceiling, and no overshoot
    /// beyond F appears anywhere in the per-tick worst (the audit's
    /// per-tick maximum work, pinned as exact integers, never
    /// wall-clock).
    #[test]
    fn the_fan_in_cap_bounds_the_atomic_release_exactly() {
        let mut spine = Spine::new(
            RecordingWorld {
                executed: Vec::new(),
            },
            10,
        );
        spine.set_admission_bounds(admission(64, 8, 1024));
        let prereq = spine.schedule(spec(0, Field::Near, 10, 9999)).unwrap();
        for i in 0..8u64 {
            assert!(
                spine
                    .schedule(dep_spec(1, Field::Near, 1, i, vec![prereq]))
                    .is_ok()
            );
        }
        assert!(matches!(
            spine.schedule(dep_spec(1, Field::Near, 1, 8, vec![prereq])),
            Err(ScheduleError::PrerequisiteFanInExceeded { .. })
        ));
        // All eight park inside tick 1's allowance (8 < 15).
        spine.run_ticks(9);
        assert_eq!(spine.blocked_count(), 8);
        declared_fan_in_matches_revalidation(&spine);
        assert_release_charge_identity(&spine);
        // Tick 10: the prerequisite executes; its atomic release
        // visits exactly F waiters — the E-10 release stays
        // all-or-nothing, bounded by admission rather than suspended.
        spine.run_tick();
        let counters = spine.housekeeping_stats();
        assert_eq!(counters.waiter_visits, 8, "waiter_visits == F exactly");
        assert_eq!(counters.release_unblocks, 8);
        assert_eq!(counters.release_decrements, 0);
        assert_eq!(counters.release_not_blocked, 0);
        assert_release_charge_identity(&spine);
        // The per-tick worst: tick 1's eight parks and tick 10's
        // eight release visits — no overshoot beyond F anywhere.
        assert_eq!(counters.worst_tick_housekeeping, 8);
        // The whole herd executes in the release tick (budget 10 ≥
        // 1 + 8) in canonical (due, seq) order.
        assert_eq!(spine.stats().executed, 9);
        let mut expected = vec![9999u64];
        expected.extend(0..8);
        assert_eq!(spine.world().executed, expected);
        assert_eq!(spine.blocked_count(), 0);
        assert!(!spine.declared_fan_in.contains_key(&prereq.seq));
        declared_fan_in_matches_revalidation(&spine);
    }

    /// §7.3 — DECLARATIONS ON EXECUTED PREREQUISITES ARE FREE: at
    /// F=1, declarers on an already-executed event are admitted
    /// without refusal, execute directly (never park), and leave the
    /// map empty — including the follow-up-on-just-executing-producer
    /// shape, whose admission the `execute_now` ordering (the entry
    /// dies beside the `unfinished.remove`, before the handler runs)
    /// makes free by construction.
    #[test]
    fn declarations_on_executed_prerequisites_are_free() {
        // The plain shape: schedule-side declarers on an executed event.
        let mut spine = Spine::new(
            RecordingWorld {
                executed: Vec::new(),
            },
            100,
        );
        spine.set_admission_bounds(admission(64, 1, 1024));
        let producer = spine.schedule(spec(0, Field::Near, 1, 9999)).unwrap();
        spine.run_tick();
        assert!(spine.is_executed(producer));
        for i in 0..10u64 {
            assert!(
                spine
                    .schedule(dep_spec(1, Field::Near, 2, i, vec![producer]))
                    .is_ok(),
                "declarations on an executed prerequisite are free"
            );
        }
        assert!(spine.declared_fan_in.is_empty());
        declared_fan_in_matches_revalidation(&spine);
        spine.run_tick();
        assert_eq!(
            spine.stats().executed,
            11,
            "every declarer executed directly — none parked"
        );
        assert_eq!(spine.blocked_count(), 0);
        assert_eq!(spine.housekeeping_stats().parks, 0);

        // The follow-up-on-producer shape: a handler whose follow-up
        // depends on the event currently executing (the :3997-3999
        // shape) — the executing event's entry is already gone when
        // the emission's declaration is checked, and the chain of
        // such follow-ups never parks.
        struct ProducerChainWorld;
        impl World for ProducerChainWorld {
            fn execute(&mut self, event: &Event, ctx: &mut ExecCtx) {
                if event.payload > 0 {
                    ctx.follow_up(1, EventClass::new(2), event.payload - 1, vec![event.id]);
                }
            }
            fn fingerprint(&self, fp: &mut Fingerprinter) {
                fp.write_str("engine-core/spine/t08-producer-chain/1");
            }
        }
        let mut chained = Spine::new(ProducerChainWorld, 100);
        chained.set_admission_bounds(admission(64, 1, 1024));
        chained
            .schedule(spec(3, Field::Near, 1, 3))
            .expect("the chain root schedules");
        chained.run_ticks(5);
        assert_eq!(
            chained.stats().executed,
            4,
            "the root plus three chained follow-ups"
        );
        assert_eq!(chained.blocked_count(), 0);
        assert_eq!(chained.housekeeping_stats().parks, 0);
        assert!(chained.declared_fan_in.is_empty());
        declared_fan_in_matches_revalidation(&chained);
        assert_release_charge_identity(&chained);
    }

    /// §7.4 — HELD AND CROSS-LANE COUNTING: declarers in BOTH lanes
    /// count on the same prerequisite (the caps are lane-blind), a
    /// held declarer counts (a prerequisite's entry survives
    /// suspension of its declarers — suspension invariance), a
    /// declarer resumed from held re-parks through the normal path
    /// with the count unchanged throughout, and the prerequisite's
    /// execution clears the entry (later declarations free).
    #[test]
    fn held_and_cross_lane_declarers_count_with_suspension_invariance() {
        let mut spine = Spine::new(NoopWorld, 100);
        spine.set_admission_bounds(admission(64, 3, 1024));
        let prereq = spine.schedule(spec(0, Field::Near, 50, 0)).unwrap();
        let near_declarer = spine
            .schedule(dep_spec(1, Field::Near, 1, 0, vec![prereq]))
            .unwrap();
        // The far-lane declarer (the cross-lane half of the count).
        spine
            .schedule(dep_spec(1, Field::Far, 1, 1, vec![prereq]))
            .unwrap();
        let held_declarer = spine
            .schedule_held(dep_spec(2, Field::Near, 200, 2, vec![prereq]))
            .unwrap();
        assert_eq!(spine.declared_fan_in.get(&prereq.seq), Some(&3));
        declared_fan_in_matches_revalidation(&spine);
        // Suspending the queued near declarer keeps its count — a
        // held declarer is unfinished, and declarations are counted,
        // not registrations (the suspension invariant).
        assert_eq!(spine.suspend_matching(|view| view.id == near_declarer), 1);
        assert_eq!(
            spine.declared_fan_in.get(&prereq.seq),
            Some(&3),
            "suspension does not drop declarations"
        );
        declared_fan_in_matches_revalidation(&spine);
        // Resume re-parks through the normal path; the count is
        // unchanged throughout.
        assert_eq!(spine.resume_matching(|view| view.id == near_declarer), 1);
        assert_eq!(spine.declared_fan_in.get(&prereq.seq), Some(&3));
        // The held declarer resumes the same way (its due is far;
        // it queues, still unfinished, still counted).
        assert_eq!(spine.resume_matching(|view| view.id == held_declarer), 1);
        assert_eq!(spine.declared_fan_in.get(&prereq.seq), Some(&3));
        declared_fan_in_matches_revalidation(&spine);
        // Tick 1: the far declarer parks; the two resumed declarers
        // re-parked through the resume path (their prerequisite is
        // unmet) — cross-lane declarations on one prerequisite, all
        // three counted throughout.
        spine.run_tick();
        assert_eq!(spine.blocked_count(), 3);
        assert_eq!(spine.declared_fan_in.get(&prereq.seq), Some(&3));
        declared_fan_in_matches_revalidation(&spine);
        // The cap binds at three: the fourth declarer is refused.
        assert!(matches!(
            spine.schedule(dep_spec(3, Field::Near, 1, 3, vec![prereq])),
            Err(ScheduleError::PrerequisiteFanInExceeded { .. })
        ));
        // The prerequisite's execution clears the entry: all three
        // declarers release (the two due-1 ones execute; the due-200
        // one re-queues for its own far due), and later declarations
        // on the prerequisite are free.
        spine.run_ticks(60);
        assert_eq!(
            spine.stats().executed,
            3,
            "the prerequisite and the two due-1 declarers (the due-200 one waits)"
        );
        assert!(!spine.declared_fan_in.contains_key(&prereq.seq));
        declared_fan_in_matches_revalidation(&spine);
        assert!(
            spine
                .schedule(dep_spec(3, Field::Near, 300, 4, vec![prereq]))
                .is_ok(),
            "declarations on the executed prerequisite are free again"
        );
        assert!(spine.declared_fan_in.is_empty());
    }

    /// §7.6 — THE `reconstruct_held` TRANSFER: the replacement
    /// identity's declarations are recounted through the
    /// replacement path's full rebuild — the removed chains'
    /// declarations leave with them, the replacements' specs count
    /// as installed — and the revalidation helper stays green across
    /// the reconstruction (rebuild-equals-maintained).
    #[test]
    fn reconstruction_transfers_declarations_through_the_recount() {
        let mut spine = Spine::new(SaveableNoopWorld, 100);
        spine.set_admission_bounds(admission(64, 8, 1024));
        let prereq = spine.schedule(spec(0, Field::Near, 50, 0)).unwrap();
        let other = spine.schedule(spec(0, Field::Near, 60, 0)).unwrap();
        // A queued class-2 victim declaring `prereq`, replaced by a
        // held event whose spec declares `other`.
        let victim = spine
            .schedule(dep_spec(2, Field::Near, 5, 0, vec![prereq]))
            .unwrap();
        assert_eq!(spine.declared_fan_in.get(&prereq.seq), Some(&1));
        declared_fan_in_matches_revalidation(&spine);
        let replacement = Event {
            id: victim,
            class: EventClass::new(2),
            schedule: Schedule::AtLogicalTime { due: Tick(7) },
            payload: 9,
            field: Field::Near,
            depends_on: vec![other],
        };
        assert_eq!(
            spine.try_reconstruct_held(|view| view.class == EventClass::new(2), vec![replacement]),
            Ok(1)
        );
        // The victim's declaration of `prereq` left with it; the
        // replacement's declaration of `other` arrived.
        assert!(!spine.declared_fan_in.contains_key(&prereq.seq));
        assert_eq!(spine.declared_fan_in.get(&other.seq), Some(&1));
        declared_fan_in_matches_revalidation(&spine);
        assert_release_charge_identity(&spine);
        // The recount's admission consequence: with F=1 a further
        // declarer on `other` (one counted) is refused, while a
        // declarer on `prereq` (zero counted) is free again.
        spine.set_admission_bounds(admission(64, 1, 1024));
        assert!(matches!(
            spine.schedule(dep_spec(3, Field::Near, 1, 0, vec![other])),
            Err(ScheduleError::PrerequisiteFanInExceeded { .. })
        ));
        assert!(
            spine
                .schedule(dep_spec(3, Field::Near, 1, 1, vec![prereq]))
                .is_ok()
        );
        declared_fan_in_matches_revalidation(&spine);
    }

    /// §7.7 — SAVE/RESTORE: a mid-herd save restores byte-identically,
    /// the rebuilt `declared_fan_in` map equals the maintained map
    /// (revalidation), the restored spine — config at DEFAULTS —
    /// continues the herd to completion with the canonical order
    /// preserved, and the config-reset behavior is documented live:
    /// a declaration the saving spine's bounds would refuse is
    /// admitted after restore (world-content divergence observable),
    /// while configuration alone never moves a fingerprint.
    #[test]
    fn save_restore_rebuilds_declared_fan_in_and_resumes_at_default_bounds() {
        fn build() -> Spine<SaveableNoopWorld> {
            let mut spine = Spine::new(SaveableNoopWorld, 100);
            spine.set_admission_bounds(admission(64, 8, 1024));
            let prereq = spine
                .schedule(spec(0, Field::Near, 10, 0))
                .expect("the prerequisite schedules");
            for i in 0..5u64 {
                spine
                    .schedule(dep_spec(1, Field::Near, 1, i, vec![prereq]))
                    .expect("the herd schedules");
            }
            spine
        }
        // The mid-herd boundary: five declarers parked on the
        // far-due prerequisite.
        let mut spine = build();
        spine.run_tick();
        assert_eq!(spine.blocked_count(), 5);
        declared_fan_in_matches_revalidation(&spine);
        let maintained = spine.declared_fan_in.clone();
        let bytes = spine.save_bytes();
        let mut restored: Spine<SaveableNoopWorld> =
            Spine::restore(&bytes, Diagnostics::new(Level::Off)).expect("restore");
        assert_eq!(restored.save_bytes(), bytes, "byte-identical round trip");
        assert_eq!(
            restored.declared_fan_in, maintained,
            "the rebuilt map equals the maintained map"
        );
        declared_fan_in_matches_revalidation(&restored);
        assert_eq!(
            restored.admission_bounds(),
            HousekeepingAdmission::default(),
            "configuration never rides the wire — the restored spine resumes at the defaults"
        );
        // The continuation: the restored spine drains the herd with
        // the canonical order preserved, byte-identical to the
        // uninterrupted control (the :6224/:6490 disciplines).
        let mut control = build();
        control.run_tick();
        spine.run_ticks(12);
        restored.run_ticks(12);
        control.run_ticks(12);
        assert_eq!(spine.stats().executed, 6);
        assert_eq!(restored.stats().executed, 6);
        assert_eq!(restored.save_bytes(), spine.save_bytes());
        assert_eq!(restored.engine_fingerprint(), spine.engine_fingerprint());
        assert_eq!(restored.engine_fingerprint(), control.engine_fingerprint());
        declared_fan_in_matches_revalidation(&restored);
        // The config-reset behavior, live: from the SAME restored
        // bytes, the spine re-configured to the saving process's F=5
        // refuses the sixth declarer (five already counted) while
        // the defaults-admitted twin takes it — the divergence is
        // observable in world content, never in spine fingerprints
        // of identical logical state.
        let mut reconfigured: Spine<SaveableNoopWorld> =
            Spine::restore(&bytes, Diagnostics::new(Level::Off)).expect("restore");
        reconfigured.set_admission_bounds(admission(64, 5, 1024));
        let mut defaulted: Spine<SaveableNoopWorld> =
            Spine::restore(&bytes, Diagnostics::new(Level::Off)).expect("restore");
        // Config alone never moves a fingerprint: the two restored
        // twins — one re-configured, one at the defaults — hash
        // identically over their IDENTICAL logical state.
        assert_eq!(
            reconfigured.engine_fingerprint(),
            defaulted.engine_fingerprint(),
            "identical restored state under different configs fingerprints identically"
        );
        let prereq = EventId { seq: 0 };
        assert!(matches!(
            reconfigured.schedule(dep_spec(2, Field::Near, 1, 5, vec![prereq])),
            Err(ScheduleError::PrerequisiteFanInExceeded { .. })
        ));
        assert!(
            defaulted
                .schedule(dep_spec(2, Field::Near, 1, 5, vec![prereq]))
                .is_ok(),
            "the restored spine sits at the defaults — the sixth declarer is admitted"
        );
        // The divergence is observable in world content — the
        // admitted sixth declarer is real state the refused twin
        // never gained (which is exactly why it shows in the
        // fingerprint: the states genuinely differ, the CONFIG never
        // did).
        assert_eq!(reconfigured.stats().scheduled, 6);
        assert_eq!(defaulted.stats().scheduled, 7);
    }

    /// §7.8 — THE FOLLOW-UP CROP (K): a handler emitting K+1 follow-ups
    /// gets exactly K admitted (the world's stream reflects exactly K
    /// children), `follow_ups_cropped` counts the excess,
    /// `invalid_dropped` stays zero (the classes discriminate — a
    /// crop is a config outcome, not a dependency-shape refusal), the
    /// scratch buffer's steady-state element capacity stays ≤ K, and
    /// a handler emitting within the cap crops nothing.
    #[test]
    fn follow_ups_beyond_the_admission_cap_are_cropped_counted_and_fail_visible() {
        /// The emitter's payload IS its emission count (6 → six
        /// attempts at K=4: four admitted, two cropped; 3 → three
        /// attempts: within the cap, zero crops).
        struct FanoutWorld {
            executions: Vec<u64>,
        }
        impl World for FanoutWorld {
            fn execute(&mut self, event: &Event, ctx: &mut ExecCtx) {
                self.executions.push(event.payload);
                if event.class == EventClass::new(3) {
                    for i in 0..event.payload {
                        ctx.follow_up(1, EventClass::new(2), 100 + i, Vec::new());
                    }
                }
            }
            fn fingerprint(&self, fp: &mut Fingerprinter) {
                fp.write_str("engine-core/spine/t08-fanout/1");
                fp.write_usize(self.executions.len());
            }
        }
        let mut spine = Spine::new(
            FanoutWorld {
                executions: Vec::new(),
            },
            100,
        );
        spine.set_admission_bounds(admission(64, 4096, 4));
        // Two six-attempt emitters at K=4: eight children admitted,
        // four cropped.
        for _ in 0..2u64 {
            spine
                .schedule(spec(3, Field::Near, 1, 6))
                .expect("the emitter schedules");
        }
        spine.run_ticks(4);
        assert_eq!(
            spine.stats().executed,
            10,
            "two emitters plus eight admitted children"
        );
        let counters = spine.housekeeping_stats();
        assert_eq!(
            counters.follow_ups_cropped, 4,
            "six attempted minus four admitted, twice"
        );
        assert_eq!(
            spine.stats().invalid_dropped,
            0,
            "the crop is a config outcome, never a dependency-shape refusal"
        );
        assert_eq!(spine.pending_count(), 0);
        // The world's stream reflects exactly the K admitted children
        // per emitter, in emission order.
        let children: Vec<u64> = spine
            .world()
            .executions
            .iter()
            .copied()
            .filter(|&payload| payload >= 100)
            .collect();
        assert_eq!(
            children,
            vec![100, 101, 102, 103, 100, 101, 102, 103],
            "exactly K children per emitter, the excess never scheduled"
        );
        // The scratch buffer's steady-state element capacity is
        // bounded by K (the crop happens at emission, before the
        // buffer ever holds the excess).
        assert!(
            spine.scratch_follow_ups.capacity() <= 4,
            "the scratch capacity {} stays at or under K",
            spine.scratch_follow_ups.capacity()
        );
        // A handler emitting WITHIN the cap crops nothing: a
        // three-attempt emitter at the same K=4 (due the next tick —
        // the clock sits at 4 after the first phase).
        spine
            .schedule(spec(3, Field::Near, 5, 3))
            .expect("a within-cap emitter schedules");
        spine.run_ticks(3);
        assert_eq!(
            spine.housekeeping_stats().follow_ups_cropped,
            4,
            "the within-cap emission cropped nothing"
        );
        assert_eq!(spine.stats().executed, 14);
        assert_eq!(spine.stats().invalid_dropped, 0);
        assert_release_charge_identity(&spine);
    }

    /// §7.9 — THE STORMS RE-PINNED AT A CONFIGURED FAN-IN: the
    /// 600-storm's shape at F=256 admits the first 256 declarers and
    /// refuses the remaining 344 typed AT ADMISSION (admission order
    /// = schedule order), the release order and execution order of
    /// the ADMITTED graph are the canonical integers (the refused
    /// never exist), and at the DEFAULTS the same 600-declarer shape
    /// admits all 600 (within-cap graphs unchanged — the compat bar).
    #[test]
    fn storms_re_pinned_at_a_configured_fan_in() {
        let mut spine = Spine::new(
            RecordingWorld {
                executed: Vec::new(),
            },
            100,
        );
        spine.set_admission_bounds(admission(64, 256, 1024));
        let prereq = spine.schedule(spec(0, Field::Near, 10, 9999)).unwrap();
        let mut refused = 0u64;
        let mut admitted = 0u64;
        for i in 0..600u64 {
            match spine.schedule(dep_spec(1, Field::Near, 1, i, vec![prereq])) {
                Ok(_) => admitted += 1,
                Err(ScheduleError::PrerequisiteFanInExceeded { .. }) => refused += 1,
                other => panic!("unexpected schedule outcome: {other:?}"),
            }
        }
        assert_eq!(
            admitted, 256,
            "the admission-order refusal point: exactly F"
        );
        assert_eq!(refused, 344);
        assert_eq!(
            spine.stats().scheduled,
            257,
            "the refused declarers never exist"
        );
        declared_fan_in_matches_revalidation(&spine);
        // The 256 admitted park inside one tick's allowances (256 ≤
        // 150 + 106 of the redistribution phase's 150).
        spine.run_ticks(9);
        assert_eq!(spine.blocked_count(), 256);
        declared_fan_in_matches_revalidation(&spine);
        // Tick 10: the prerequisite executes; the atomic release
        // visits exactly 256 — bounded by F, overshooting the
        // phase's 150-unit allowance exactly as the E-10 contract
        // records (the bound gates the next iteration).
        spine.run_tick();
        let counters = spine.housekeeping_stats();
        assert_eq!(counters.waiter_visits, 256);
        assert_eq!(counters.release_unblocks, 256);
        assert_eq!(counters.release_decrements, 0);
        assert_release_charge_identity(&spine);
        // The deferred waiters execute over subsequent ticks in
        // canonical (due, seq) order — the exact accepted sequence.
        spine.run_to_completion(100);
        let mut expected = vec![9999u64];
        expected.extend(0..256);
        assert_eq!(spine.world().executed, expected);
        assert_eq!(spine.stats().executed, 257);
        declared_fan_in_matches_revalidation(&spine);

        // At the DEFAULTS the same shape admits all 600 — within-cap
        // graphs unchanged (compat bar 0's live re-run).
        let mut wide = Spine::new(
            RecordingWorld {
                executed: Vec::new(),
            },
            100,
        );
        let wide_prereq = wide.schedule(spec(0, Field::Near, 10, 9999)).unwrap();
        for i in 0..600u64 {
            assert!(
                wide.schedule(dep_spec(1, Field::Near, 1, i, vec![wide_prereq]))
                    .is_ok()
            );
        }
        assert_eq!(wide.stats().scheduled, 601);
        declared_fan_in_matches_revalidation(&wide);
    }

    /// §7.9 part 2 — BLOCKED-ONLY GRAPHS AND CYCLES: cycles remain
    /// structurally impossible (the unknown-dependency gate — forward
    /// references refused typed, unchanged by the caps), and a
    /// blocked-only graph under a held root with over-cap declarers
    /// behaves exactly as before: the refused declarers never exist,
    /// the admitted graph parks on the held root, and
    /// `run_to_completion` stops on the no-progressible-work rule
    /// without burning its tick budget on motionless time.
    #[test]
    fn over_cap_declarers_never_exist_and_cycles_stay_structurally_impossible() {
        let mut spine = Spine::new(NoopWorld, 100);
        // The cycle gate: a forward reference is an unknown
        // dependency, refused typed (unchanged).
        let ghost = EventId { seq: 99 };
        assert!(
            matches!(spine.schedule(dep_spec(1, Field::Near, 1, 0, vec![ghost])), Err(
                ScheduleError::UnknownDependency(id)
            ) if id == ghost)
        );
        // The blocked-only graph: a held root and six would-be
        // declarers at F=4.
        spine.set_admission_bounds(admission(64, 4, 1024));
        let root = spine.schedule_held(spec(0, Field::Near, 1000, 0)).unwrap();
        let mut refused = 0u64;
        for i in 0..6u64 {
            match spine.schedule(dep_spec(1, Field::Near, 1, i, vec![root])) {
                Ok(_) => {}
                Err(ScheduleError::PrerequisiteFanInExceeded { .. }) => refused += 1,
                other => panic!("unexpected schedule outcome: {other:?}"),
            }
        }
        assert_eq!(refused, 2);
        assert_eq!(
            spine.stats().scheduled,
            5,
            "one held root plus the four admitted declarers"
        );
        assert_eq!(spine.declared_fan_in.get(&root.seq), Some(&4));
        declared_fan_in_matches_revalidation(&spine);
        // The admitted declarers park on the held root (blocked-only:
        // no queued work remains after the parks).
        spine.run_tick();
        assert_eq!(spine.blocked_count(), 4);
        assert_eq!(spine.pending_count(), 0);
        declared_fan_in_matches_revalidation(&spine);
        assert_release_charge_identity(&spine);
        // run_to_completion stops on the no-progressible-work rule —
        // exactly the pre-cap behavior (motionless time is not
        // burned; the graph stays unfinished, observably). The
        // manual tick above already parked everyone and emptied the
        // queues, so zero further ticks run.
        assert_eq!(
            spine.run_to_completion(50),
            0,
            "the parks emptied the queues — no motionless time is burned"
        );
        assert_eq!(spine.stats().executed, 0);
        assert_eq!(spine.blocked_count(), 4);
        assert!(spine.has_unfinished_work());
    }

    /// §7.10 — 100k UNRELATED OBLIGATIONS AT FIXED BUDGET: 100,000
    /// independent events, zero dependencies, at budget 200 — the
    /// audit's fixed-budget case, proving the caps cost nothing when
    /// unused: zero parks, zero registrations, zero refusals, zero
    /// crops, the per-tick worst housekeeping at zero (far under the
    /// 4.5·budget = 900 ceiling), and the backlog slope monotone
    /// decreasing through the per-tick `LaneReport` fields (no
    /// hand-sampling).
    #[test]
    fn one_hundred_thousand_unrelated_obligations_drain_within_the_closed_form() {
        let mut spine = Spine::new(
            RecordingWorld {
                executed: Vec::new(),
            },
            200,
        );
        for i in 0..100_000u64 {
            spine
                .schedule(spec(0, Field::Near, 1, i))
                .expect("the unrelated population schedules");
        }
        assert_eq!(spine.stats().scheduled, 100_000);
        // The integer ceiling (§5.3): Σ phase caps ≤ 3·budget, each
        // phase's allowance 1.5× — 4.5·budget blended — with no
        // terminal overshoot to add (nothing ever releases: F is
        // never in play in this fixture).
        let ceiling = 3u64 * 200 + 3 * (200 / 2);
        assert_eq!(ceiling, 900, "4.5·budget at B=200");
        let mut ticks = 0u64;
        let mut previous_backlog = u64::MAX;
        while spine.has_unfinished_work() {
            spine.run_tick();
            ticks += 1;
            // The published per-tick worst never crosses the
            // whole-tick ceiling (§5.3: Σ phase caps ≤ 3·budget →
            // 4.5·budget blended; no overshoot — nothing to release).
            assert!(
                spine.housekeeping_stats().worst_tick_housekeeping <= ceiling,
                "tick {ticks}: the closed-form ceiling holds"
            );
            let report = spine.lane_report();
            assert!(
                report.housekeeping_backlog_near <= previous_backlog,
                "tick {ticks}: the slope is monotone decreasing ({} after {})",
                report.housekeeping_backlog_near,
                previous_backlog
            );
            previous_backlog = report.housekeeping_backlog_near;
            if ticks == 1 {
                assert_eq!(report.housekeeping_backlog_near, 99_800);
            }
        }
        assert!(ticks >= 500, "drains over at least 500 ticks ({ticks})");
        assert_eq!(ticks, 500);
        assert_eq!(spine.stats().executed, 100_000);
        assert_eq!(spine.world().executed.len(), 100_000);
        let counters = spine.housekeeping_stats();
        assert_eq!(counters.parks, 0, "unrelated ⇒ no caps touched");
        assert_eq!(counters.dependency_registrations, 0);
        assert_eq!(counters.worst_tick_dependency_registrations, 0);
        assert_eq!(counters.follow_ups_cropped, 0);
        assert_eq!(counters.worst_tick_housekeeping, 0);
        assert_eq!(counters.waiter_visits, 0);
        assert_eq!(spine.stats().budget_exhausted_ticks, 499);
        // The canonical execution order survived the whole drain.
        assert_eq!(spine.world().executed, (0..100_000u64).collect::<Vec<_>>());
    }

    /// §7.11 — THE COMPOSITE PER-TICK MAXIMUM: a storm (a parking
    /// herd on one prerequisite), a release overshoot, and a
    /// follow-up emitter in ONE world at small configured caps
    /// (B=10, D=4, F=8, K=4): after every tick the published tick
    /// housekeeping sits at or under the §2 ceiling evaluated at
    /// (B, D, F, K) — `4.5·B + 4F` in blended units — the closed form
    /// pinned as an integer bound on a real adversarial composition,
    /// and the run completes with conservation exact.
    #[test]
    fn the_composite_per_tick_maximum_respects_the_closed_form_ceiling() {
        struct CompositeWorld;
        impl World for CompositeWorld {
            fn execute(&mut self, event: &Event, ctx: &mut ExecCtx) {
                if event.class == EventClass::new(3) {
                    for i in 0..6u64 {
                        ctx.follow_up(1, EventClass::new(2), 100 + i, Vec::new());
                    }
                }
            }
            fn fingerprint(&self, fp: &mut Fingerprinter) {
                fp.write_str("engine-core/spine/t08-composite/1");
            }
        }
        const BUDGET: u64 = 10;
        const FAN_IN: u64 = 8;
        let mut spine = Spine::new(CompositeWorld, BUDGET as u32);
        spine.set_admission_bounds(admission(4, FAN_IN, 4));
        // The herd: one far-due prerequisite, eight declarers (F=8),
        // the ninth refused.
        let prereq = spine.schedule(spec(0, Field::Near, 30, 9999)).unwrap();
        for i in 0..8u64 {
            assert!(
                spine
                    .schedule(dep_spec(1, Field::Near, 1, i, vec![prereq]))
                    .is_ok()
            );
        }
        assert!(matches!(
            spine.schedule(dep_spec(1, Field::Near, 1, 8, vec![prereq])),
            Err(ScheduleError::PrerequisiteFanInExceeded { .. })
        ));
        // The follow-up emitters: two six-attempt emitters at K=4.
        for _ in 0..2u64 {
            spine
                .schedule(spec(3, Field::Near, 1, 6))
                .expect("the emitter schedules");
        }
        assert_eq!(
            spine.stats().scheduled,
            11,
            "1 + 8 + 2 — the refused never exist"
        );
        // The integer ceiling (§5.3): Σ phase caps ≤ 3·B, each
        // phase's allowance 1.5×, plus the per-phase terminal
        // overshoot 4F — the closed form evaluated at the caps.
        let ceiling = 3 * BUDGET + 3 * (BUDGET / 2) + 4 * FAN_IN;
        assert_eq!(ceiling, 77);
        let mut ticks = 0u64;
        while spine.has_unfinished_work() {
            spine.run_tick();
            ticks += 1;
            assert!(
                spine.housekeeping_stats().worst_tick_housekeeping <= ceiling,
                "tick {ticks}: the published worst {} sits within the closed-form \
                 ceiling {ceiling}",
                spine.housekeeping_stats().worst_tick_housekeeping
            );
            declared_fan_in_matches_revalidation(&spine);
            assert_release_charge_identity(&spine);
            // Conservation at every boundary (a refused or cropped
            // event simply never enters `scheduled`).
            let stats = spine.stats();
            assert_eq!(
                stats.scheduled,
                stats.executed
                    + spine.pending_count() as u64
                    + spine.blocked_count() as u64
                    + spine.held_count() as u64,
                "tick {ticks}: conservation"
            );
        }
        assert_eq!(ticks, 30, "the prerequisite's due bounds the run");
        assert_eq!(
            spine.stats().executed,
            19,
            "prereq + 8 declarers + 2 emitters + 8 children"
        );
        assert_eq!(spine.housekeeping_stats().follow_ups_cropped, 4);
        assert_eq!(spine.stats().invalid_dropped, 0);
        assert_eq!(spine.housekeeping_stats().worst_tick_housekeeping, 8);
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

    /// The consequence-freedom world-class capability (the fixtures'
    /// shorthand — the test driver asserting its world is
    /// permanently consequence-free: `NoopWorld`, `SaveableNoopWorld`,
    /// and `SustainedWorld` never read execution history, which is
    /// exactly the Class-1 contract; production callers construct it
    /// at their own composition boundary, T07/CA-07).
    fn eligibility() -> ConsequenceFreedom {
        ConsequenceFreedom::permanently_free()
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

    /// THE LOGICAL HEAD, MEASURED (T07 §7 — the CA-07 :6920 witness
    /// INVERTED by the approved design's §5.9: the drain's per-firing
    /// `retained`-record tail move becomes ZERO — the cut is a `head`
    /// bump, the summarized prefix dead physical slack): per firing
    /// that does not trigger the amortized compact, every surviving
    /// record keeps its EXACT physical index (witnessed below; the
    /// red-first witness `red_witness_drain_moves_surviving_records_
    /// at_every_firing` failed against the drain with
    /// `left: seq 200, right: seq 100` at physical index 100 before
    /// this landed), and the amortized compact — one
    /// `Vec::drain(..head)` when the dead prefix reaches the live
    /// tail's size — bounds TOTAL moves by TOTAL discarded records
    /// over any prefix of the run (each record physically moved at
    /// most once per discard batch, amortized ≤ one move per
    /// discarded record, versus the drain's one move per RETAINED
    /// record per firing). The capacity witness stays the allocation
    /// proof (the IA-07 discipline — operation counts, never
    /// wall-clock): the buffer never shrinks and never regrows after
    /// the first firing (the head bump and the amortized drain both
    /// retain the high-water capacity, so the sustained loop fires
    /// with ZERO ledger allocations), and after any firing the
    /// capacity strictly exceeds the live tail.
    #[test]
    fn policy_compaction_bumps_a_logical_head_with_amortized_moves() {
        // Three regimes: the two CA-07 measured windows at chunk 300
        // (retain 128 fires at ticks 5/8/11 from 500 down to 200
        // live; retain 300 fires at ticks 7/10 from 700 down to 400
        // live), and the wide-window regime at chunk 100 (retain 400
        // fires at ticks 6-12 with the amortized compact co-firing
        // at tick 10 — six pure head bumps around it).
        for (retain, chunk, ticks) in [(128u64, 300u64, 12u64), (300, 300, 12), (400, 100, 12)] {
            let mut spine = Spine::new(SustainedWorld, 100);
            for i in 0..100u64 {
                spine
                    .schedule(spec(5, Field::Near, 1, i))
                    .expect("schedule");
            }
            spine.set_retention_policy(policy(retain, chunk), eligibility());
            let mut capacity_high_water: usize = 0;
            let mut total_moves: u64 = 0;
            let mut total_discarded: u64 = 0;
            let mut discarded_since_compact: u64 = 0;
            let mut firings: u64 = 0;
            let mut pure_bumps: u64 = 0;
            for tick in 1..=ticks {
                // The pre-firing physical snapshot: the survivors'
                // exact positions (the zero-move oracle).
                let pre_physical: Vec<EventId> =
                    spine.ledger.iter().map(|record| record.id).collect();
                let pre_head = spine.head;
                let pre_len = spine.ledger.len();
                spine.run_tick();
                // The capacity witness: never shrinks, never regrows
                // after the first firing (the head bump and the
                // amortized drain both keep the buffer).
                let capacity = spine.ledger.capacity();
                assert!(
                    capacity >= capacity_high_water,
                    "tick {tick}: the ledger buffer never shrinks (retain {retain})"
                );
                capacity_high_water = capacity_high_water.max(capacity);
                let stats = spine.retention_stats();
                if stats.compactions > firings {
                    firings = stats.compactions;
                    let cut = stats.last_chunk;
                    assert_eq!(cut, chunk, "tick {tick}: the chunk is constant");
                    total_discarded += cut;
                    discarded_since_compact += cut;
                    let live = spine.ledger.len() - spine.head;
                    if spine.head == 0 && spine.ledger.len() < pre_len {
                        // The amortized compact co-fired this tick:
                        // the drain moved exactly the live tail down
                        // (live records moved), bounded by the
                        // discards since the last compact.
                        assert!(
                            discarded_since_compact >= live as u64,
                            "tick {tick}: the compact's {live} moves ≤ the \
                             {discarded_since_compact} discards since the last compact \
                             (retain {retain})"
                        );
                        total_moves += live as u64;
                        discarded_since_compact = 0;
                    } else {
                        // The pure head bump: zero bytes moved — the
                        // head advanced by exactly the cut, no record
                        // was removed (the tick's arrivals appended
                        // beyond the pre-firing physical length), and
                        // EVERY pre-firing record — dead prefix
                        // included — keeps its exact physical index.
                        assert_eq!(
                            spine.head,
                            pre_head + cut as usize,
                            "tick {tick}: the cut is a head bump (retain {retain})"
                        );
                        assert!(
                            spine.ledger.len() >= pre_len,
                            "tick {tick}: nothing was removed (retain {retain})"
                        );
                        for (offset, id) in pre_physical.iter().enumerate() {
                            assert_eq!(
                                spine.ledger[offset].id, *id,
                                "tick {tick}: physical index {offset} stable across the \
                                 firing (retain {retain})"
                            );
                        }
                        pure_bumps += 1;
                    }
                    // The amortized bound, cumulative at every firing.
                    assert!(
                        total_moves <= total_discarded,
                        "tick {tick}: total moves {total_moves} ≤ total discards \
                         {total_discarded} (retain {retain})"
                    );
                    // The capacity discriminator: after any firing
                    // the buffer strictly exceeds the live tail (the
                    // high-water survives the cuts — a fresh tight
                    // vector per firing would have made them equal).
                    assert!(
                        spine.ledger.capacity() > spine.ledger_retained().len(),
                        "the high-water capacity survives every firing (retain {retain})"
                    );
                }
            }
            let stats = spine.retention_stats();
            assert_eq!(stats.last_chunk, chunk, "the chunk is constant");
            assert_eq!(stats.max_chunk, chunk);
            assert_eq!(
                stats.summarized_total, total_discarded,
                "the discard accounting matches the cadence (retain {retain})"
            );
            // The published per-regime shape: retain 128 co-fires the
            // amortized compact at every firing (chunk 300 > the
            // 200-record live window — moves 200 per firing, still ≤
            // the 300 discarded); retain 300's first firing is a pure
            // bump and its second co-fires the compact (400 moves ≤
            // 600 discards); the wide window bumps six times around
            // its one compact (500 moves ≤ 700 discards).
            if retain == 128 {
                assert_eq!(firings, 3);
                assert_eq!(pure_bumps, 0);
                assert_eq!(total_moves, 600);
            } else if retain == 300 {
                assert_eq!(firings, 2);
                assert_eq!(pure_bumps, 1);
                assert_eq!(total_moves, 400);
            } else {
                assert_eq!(firings, 7);
                assert_eq!(pure_bumps, 6);
                assert_eq!(total_moves, 500);
            }
            assert!(
                total_moves <= total_discarded,
                "the amortized bound over the whole run (retain {retain})"
            );
        }
    }

    /// THE DEAD-PREFIX ROUND TRIP (T07 §5.9): a live spine carrying a
    /// dead prefix (`head > 0`, physical slack present — only ever
    /// after a compaction, per the head invariant) and its restored
    /// tight image encode IDENTICAL bytes (the `spine-history/2`
    /// section encodes the live tail only; the dead prefix is pure
    /// physical slack, never logical, never serialized), the restore
    /// rebuilds TIGHT (`head == 0`, the physical length equal to the
    /// live length), and every public observable — history kind,
    /// compacted fingerprint, stats — agrees between the two storage
    /// shapes of the same logical state.
    #[test]
    fn dead_prefix_round_trips_byte_identical_and_restores_tight() {
        let mut spine = Spine::new(SustainedWorld, 100);
        for i in 0..100u64 {
            spine
                .schedule(spec(5, Field::Near, 1, i))
                .expect("schedule");
        }
        spine.set_retention_policy(policy(400, 100), eligibility());
        spine.run_ticks(8);
        // Firings at ticks 6, 7, and 8: head 300, live 500, physical
        // 800 — the dead prefix present and the invariant holding.
        assert_eq!(spine.retention_stats().compactions, 3);
        assert_eq!(spine.head, 300);
        assert_eq!(spine.ledger_retained().len(), 500);
        assert_eq!(
            spine.ledger.len(),
            spine.ledger_retained().len() + spine.head,
            "the physical buffer is the live tail plus the dead prefix"
        );
        assert!(
            spine.compacted.is_some(),
            "the head invariant: head > 0 implies compacted.is_some()"
        );
        let bytes = spine.save_bytes();
        let restored: Spine<SustainedWorld> =
            Spine::restore(&bytes, Diagnostics::new(Level::Off)).expect("restore");
        assert_eq!(
            restored.save_bytes(),
            bytes,
            "the dead prefix never reaches the wire — the round trip is byte-identical"
        );
        assert_eq!(restored.head, 0, "the restore rebuilds tight");
        assert_eq!(restored.ledger.len(), restored.ledger_retained().len());
        assert_eq!(restored.ledger.len(), spine.ledger_retained().len());
        assert_eq!(restored.history_kind(), spine.history_kind());
        assert_eq!(
            restored.compacted_fingerprint(),
            spine.compacted_fingerprint(),
            "the compacted domain hashes the live view — identical under both shapes"
        );
        assert_eq!(restored.stats(), spine.stats());
        assert_eq!(restored.retention_policy(), None);
    }

    /// THE IN-MEMORY EXECUTION-RECORD STRIDE, ASSERTED (T07 §8's
    /// mandated measurement — the design's ~40 B estimate corrected
    /// by the measured actual, exactly as the obligation requires):
    /// the default representation reorders fields to minimize
    /// padding, giving 8 (seq) + 8 (intended due) + 8 (executed
    /// tick) + 2 (class) + 1 (field) + 1 (schedule kind) + 1 (late)
    /// and 1 trailing pad, totaling 32. This is the unit the
    /// retention cost reasoning prices — the pre-T07 drain's
    /// per-firing bytes moved (`retained × 32`), the resident-bytes
    /// estimate (`records × 32` against the ~31 B wire record), and
    /// the amortized compact's move bound.
    #[test]
    fn execution_record_stride_is_thirty_two_bytes() {
        assert_eq!(
            std::mem::size_of::<ExecutionRecord>(),
            32,
            "the documented in-memory stride — a move here is a cost-model change, \
             never a silent one"
        );
    }

    // ------------------------------- T07 retention-frontier tests (Class 2)

    /// A consumer identity for the frontier fixtures (the driver's
    /// own naming — stable, small, opaque to the engine).
    fn consumer(name: &'static str) -> RetentionConsumerId {
        RetentionConsumerId::new(name)
    }

    /// THE MOVING CUT, HELD (T07's central case — the audit's first,
    /// and the RED-FIRST witness this battery grew out of: written
    /// first against the OLD api as
    /// `red_witness_standing_token_cuts_past_a_would_be_consumers_need`,
    /// run RED at the pre-fix tree — the standing-token cadence cut
    /// exactly the 30 records a would-be consumer still needed,
    /// `left: 0, right: 30` — then replaced by this frontier form):
    /// the threshold is exceeded, a registered consumer's
    /// acknowledgment sits below the policy's cut, and the checkpoint
    /// REFUSES — counted in `refused_ticks` AND
    /// `frontier_hold_ticks`, both lag metrics advanced, no cut,
    /// `history_kind()` stays `Full`, the ledger grows past the bound
    /// with conservation exact at every boundary (the no-discard
    /// rule made mechanical: a held frontier costs one threshold
    /// comparison, never a record). The consumer then reads its
    /// history intact; when it acknowledges 15 ordinals, the next
    /// firing cuts EXACTLY `min(excess, chunk, eligible −
    /// summarized)` = 15 — the clamp, not the policy's 30.
    #[test]
    fn retention_frontier_holds_the_cut_below_a_registered_consumers_need() {
        let mut spine = Spine::new(SustainedWorld, 10);
        for i in 0..10u64 {
            spine
                .schedule(spec(5, Field::Near, 1, i))
                .expect("schedule");
        }
        spine.arm_retention_frontier(policy(4, 30));
        let accounting = consumer("accounting");
        spine
            .register_retention_consumer(accounting, 0)
            .expect("a fresh consumer on an uncut spine registers at zero");
        assert!(spine.retention_armed());
        // Six ticks with the consumer holding at ack 0: ticks 4–6
        // cross the bound (40/50/60 > 34) and every one refuses.
        for tick in 1..=6u64 {
            spine.run_tick();
            let stats = spine.retention_stats();
            assert_eq!(stats.compactions, 0, "tick {tick}: no cut while held");
            let expected_holds = tick.saturating_sub(3);
            assert_eq!(stats.frontier_hold_ticks, expected_holds);
            assert_eq!(stats.refused_ticks, expected_holds);
            // Conservation and the no-discard growth: every record
            // the world executed is still in the verbatim tail.
            assert_eq!(spine.stats().executed, tick * 10);
            assert_eq!(spine.ledger_retained().len() as u64, tick * 10);
            assert_eq!(
                spine.retention_lag_records(),
                (tick * 10).saturating_sub(34),
                "tick {tick}: the growth past the bound is exposed"
            );
            let stats = spine.stats();
            assert_eq!(
                stats.scheduled,
                stats.executed
                    + spine.pending_count() as u64
                    + spine.blocked_count() as u64
                    + spine.held_count() as u64,
                "tick {tick}: conservation"
            );
        }
        // The would-be consumer's window — the first 30 records (the
        // executions of ticks 1–3) — is INTACT (the red witness's
        // assertion, now held by the frontier).
        let retained = spine.ledger_retained();
        assert_eq!(
            retained
                .iter()
                .filter(|record| record.executed_tick.as_u64() <= 3)
                .count(),
            30,
            "the consumer's un-released records survive the cadence"
        );
        assert_eq!(spine.retention_stats().retention_frontier_lag_records, 30);
        assert!(matches!(spine.history_kind(), HistoryKind::Full));
        // The consumer absorbs 15 ordinals (records 1–15); the next
        // crossing cuts exactly the released prefix — the clamp.
        spine
            .acknowledge_retention_through(accounting, 15)
            .expect("advancing ack");
        spine.run_tick();
        let stats = spine.retention_stats();
        assert_eq!(stats.compactions, 1);
        assert_eq!(stats.last_chunk, 15, "the cut is min(excess, chunk, 15)");
        assert_eq!(stats.summarized_total, 15);
        assert_eq!(spine.ledger_retained().len(), 70 - 15);
        assert_eq!(stats.retention_frontier_lag_records, 30 - 15);
        assert_eq!(
            stats.frontier_hold_ticks, 3,
            "the firing was clamped, not held"
        );
        // The count identity at the boundary.
        let HistoryKind::Compacted {
            summarized_records,
            retained_records,
            ..
        } = spine.history_kind()
        else {
            panic!("the clamped firing compacted the spine");
        };
        assert_eq!(
            summarized_records + retained_records,
            spine.stats().executed
        );
        // Records 16–30 (the rest of the consumer's window) are
        // still verbatim in the tail.
        let retained = spine.ledger_retained();
        assert_eq!(
            retained
                .iter()
                .filter(|record| record.executed_tick.as_u64() <= 3)
                .count(),
            15,
            "only the acknowledged 15 were cut; the rest of the window survives"
        );
    }

    /// THE CLAMPED CADENCE IS THE EXPLICIT SEQUENCE (the :6487
    /// discipline extended to the frontier mode): a consumer
    /// acknowledging past the threshold in steps makes each firing
    /// cut exactly `min(excess, chunk, eligible − summarized)`, and
    /// the resulting save bytes are BYTE-IDENTICAL both to one
    /// explicit compaction of the same total window and to the
    /// interleaved explicit compaction sequence with the same cuts
    /// at the same tick boundaries — the frontier only ever vetoes
    /// or clamps a firing, so the wire stays a function of the cut
    /// sequence alone (the count identity holds at every boundary).
    #[test]
    fn retention_frontier_advances_in_clamped_steps_matching_the_explicit_sequence() {
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
        // The frontier cadence: the consumer acknowledges BETWEEN
        // ticks (its durable cadence — the value is visible to the
        // next checkpoint), and the firings cut [150, 150, 300, 100]
        // at ticks 4, 5, 7, 10 (700 summarized, 300 retained).
        let mut frontier = build();
        frontier.arm_retention_frontier(policy(10, 300));
        let accounting = consumer("accounting");
        frontier
            .register_retention_consumer(accounting, 0)
            .expect("register");
        let schedule: [(u64, u64); 4] = [(4, 150), (5, 300), (7, 700), (10, 700)];
        let mut acked = 0u64;
        let mut cuts: Vec<(u64, u64)> = Vec::new(); // (tick, cut)
        for tick in 1..=10u64 {
            if let Some(&(_, ack_to)) = schedule.iter().find(|&&(at, _)| at == tick) {
                frontier
                    .acknowledge_retention_through(accounting, ack_to)
                    .expect("the step schedule is monotone");
                acked = ack_to;
            }
            // The oracle inputs, captured before the checkpoint runs.
            let pre_summarized = match frontier.history_kind() {
                HistoryKind::Compacted {
                    summarized_records, ..
                } => summarized_records,
                HistoryKind::Full => 0,
            };
            frontier.run_tick();
            let stats = frontier.retention_stats();
            if stats.compactions > cuts.len() as u64 {
                // The at-checkpoint length: the post-firing retained
                // tail plus the cut (the tick's own arrivals
                // included).
                let at_len = frontier.ledger_retained().len() as u64 + stats.last_chunk;
                let expected = (at_len - 10)
                    .min(300)
                    .min(acked.saturating_sub(pre_summarized));
                assert_eq!(
                    stats.last_chunk, expected,
                    "tick {tick}: the cut is min(excess, chunk, eligible − summarized)"
                );
                cuts.push((tick, stats.last_chunk));
                let HistoryKind::Compacted {
                    summarized_records,
                    retained_records,
                    ..
                } = frontier.history_kind()
                else {
                    panic!("tick {tick}: a firing compacted the spine");
                };
                assert_eq!(
                    summarized_records + retained_records,
                    frontier.stats().executed,
                    "tick {tick}: count identity"
                );
            }
        }
        assert_eq!(
            cuts,
            vec![(4, 150), (5, 150), (7, 300), (10, 100)],
            "the clamped cut sequence"
        );
        assert_eq!(frontier.ledger_retained().len(), 300);
        // The explicit one-cut equivalent: the same ticks, one manual
        // cut of the same total window (the prefix chain is
        // invocation-boundary-independent — the :6487 pin).
        let mut manual = build();
        manual.run_ticks(10);
        manual.compact(300).expect("the equivalent window compacts");
        assert_eq!(
            frontier.save_bytes(),
            manual.save_bytes(),
            "the clamped cadence and one explicit 700-record cut agree byte for byte"
        );
        // The interleaved explicit equivalent: the same cuts at the
        // same tick boundaries, through the driver's own tool.
        let mut stepped = build();
        let mut total_cut = 0u64;
        for tick in 1..=10u64 {
            stepped.run_tick();
            let firings_through: u64 = cuts
                .iter()
                .filter(|&&(at, _)| at <= tick)
                .map(|&(_, cut)| cut)
                .sum();
            if firings_through > total_cut {
                total_cut = firings_through;
                let executed = stepped.stats().executed;
                stepped
                    .compact(executed - total_cut)
                    .expect("the step window compacts");
            }
        }
        assert_eq!(
            frontier.save_bytes(),
            stepped.save_bytes(),
            "the clamped cadence and the interleaved explicit sequence agree byte for byte"
        );
    }

    /// Every refusal is TYPED and every hold is COUNTED (the §5.3
    /// matrix): an armed-but-empty frontier never cuts (the
    /// empty-frontier fail-closed rule — `min(∅)` is a refusal,
    /// never a permission, and `retention_armed` reports the truth);
    /// an acknowledgment for an unknown consumer is a typed
    /// `UnknownConsumer`; a regressing acknowledgment is a typed
    /// `Regression` (monotonicity enforced, never assumed); a
    /// registration whose vouch sits below the standing cut is the
    /// typed join-time impossibility (`NeedsSummarizedRecords` — the
    /// register rule); a duplicate registration and a registration
    /// without a frontier armed are typed wiring refusals.
    #[test]
    fn retention_frontier_refusals_are_typed() {
        let mut spine = Spine::new(SustainedWorld, 10);
        for i in 0..10u64 {
            spine
                .schedule(spec(5, Field::Near, 1, i))
                .expect("schedule");
        }
        // Registration without a frontier armed: a wiring mistake,
        // surfaced typed — never a silent registry entry.
        let audit = consumer("audit");
        assert!(matches!(
            spine.register_retention_consumer(audit, 0),
            Err(RetentionRegisterError::FrontierNotArmed { id }) if id == audit
        ));
        spine.arm_retention_frontier(policy(4, 30));
        assert!(
            !spine.retention_armed(),
            "an empty frontier is not an armed cadence"
        );
        spine.run_ticks(4);
        // 40 records against the 34 bound: the empty frontier
        // refuses exactly like an unarmed policy — counted, exposed,
        // never a permission.
        let stats = spine.retention_stats();
        assert_eq!(stats.compactions, 0);
        assert_eq!(stats.frontier_hold_ticks, 1);
        assert_eq!(stats.refused_ticks, 1);
        assert_eq!(stats.retention_frontier_lag_records, 30);
        assert_eq!(spine.ledger_retained().len(), 40);
        // An acknowledgment for a consumer that was never registered.
        let unknown = consumer("never-registered");
        assert!(matches!(
            spine.acknowledge_retention_through(unknown, 10),
            Err(RetentionAckError::UnknownConsumer { id }) if id == unknown
        ));
        // Registration and the monotone rule.
        spine
            .register_retention_consumer(audit, 20)
            .expect("register at a durable read point");
        spine
            .acknowledge_retention_through(audit, 35)
            .expect("advancing");
        assert!(matches!(
            spine.acknowledge_retention_through(audit, 34),
            Err(RetentionAckError::Regression {
                id,
                acknowledged_through: 35,
                attempted_through: 34,
            }) if id == audit
        ));
        assert!(matches!(
            spine.register_retention_consumer(audit, 40),
            Err(RetentionRegisterError::AlreadyRegistered {
                id,
                acknowledged_through: 35,
            }) if id == audit
        ));
        // The register rule: a consumer joining a COMPACTED spine
        // vouches the standing cut or is refused typed.
        let mut compacted = Spine::new(SaveableNoopWorld, 100);
        for t in 1..=4u64 {
            for i in 0..100u64 {
                compacted
                    .schedule(spec(0, Field::Near, t, t * 100 + i))
                    .expect("schedule");
            }
        }
        compacted.run_ticks(4);
        compacted.compact(100).expect("an explicit 300-record cut");
        compacted.arm_retention_frontier(policy(10, 300));
        let late = consumer("late-joiner");
        assert!(matches!(
            compacted.register_retention_consumer(late, 299),
            Err(RetentionRegisterError::NeedsSummarizedRecords {
                id,
                summarized_records: 300,
                consumer_read_point: 299,
            }) if id == late
        ));
        compacted
            .register_retention_consumer(late, 300)
            .expect("vouching the standing cut registers");
        assert_eq!(
            compacted.retention_consumer_lag(late),
            Some(0),
            "the sole consumer is the leader"
        );
    }

    /// THE RESTORE→RE-REGISTER→CONTINUE PIN (the :6824 control shape
    /// extended to the frontier): a mid-cadence save→restore arrives
    /// with the frontier EMPTY and the policy cleared (no
    /// acknowledgment, no consumer registry, no counter ever rides
    /// the wire — a restored process can never resume cutting on a
    /// vouch a prior process made); re-arming and re-registering
    /// with the consumer's durably-derived ack reproduces the
    /// uninterrupted control's save BYTE FOR BYTE under the same
    /// acknowledgment schedule, and the restored process that fails
    /// to re-register REFUSES instead of cutting.
    #[test]
    fn retention_frontier_restore_requires_fresh_registration() {
        fn build() -> Spine<SustainedWorld> {
            let mut spine = Spine::new(SustainedWorld, 10);
            for i in 0..10u64 {
                spine
                    .schedule(spec(5, Field::Near, 1, i))
                    .expect("schedule");
            }
            spine
        }
        let accounting = consumer("accounting");
        // The acknowledgment schedule: the consumer absorbs each
        // tick's ten records after the tick (its durable read point
        // is the executed count).
        let drive = |spine: &mut Spine<SustainedWorld>, ticks: u64, start_tick: u64| {
            for t in start_tick..start_tick + ticks {
                spine.run_tick();
                spine
                    .acknowledge_retention_through(accounting, t * 10)
                    .expect("the schedule is monotone");
            }
        };
        // The uninterrupted control: firings at ticks 4 and 7 (each
        // cutting 30 — the consumer releases each tick's records, so
        // the clamp never binds), 90 executed, 60 summarized, 30
        // retained at the end.
        let mut control = build();
        control.arm_retention_frontier(policy(4, 30));
        control
            .register_retention_consumer(accounting, 0)
            .expect("register");
        drive(&mut control, 9, 1);
        assert_eq!(control.retention_stats().compactions, 2);
        assert_eq!(control.retention_stats().summarized_total, 60);
        assert_eq!(control.ledger_retained().len(), 30);
        // The split: five ticks (one firing in), save, restore.
        let mut split = build();
        split.arm_retention_frontier(policy(4, 30));
        split
            .register_retention_consumer(accounting, 0)
            .expect("register");
        drive(&mut split, 5, 1);
        assert_eq!(split.retention_stats().compactions, 1);
        let bytes = split.save_bytes();
        // The pin: restore → re-arm → re-register at the DURABLE read
        // point (50 — what the consumer had absorbed when the save
        // happened) → continue the same schedule → byte-identical.
        let mut restored: Spine<SustainedWorld> =
            Spine::restore(&bytes, Diagnostics::new(Level::Off)).expect("restore");
        assert_eq!(restored.save_bytes(), bytes, "the round trip is stable");
        assert_eq!(restored.retention_policy(), None);
        assert!(!restored.retention_armed());
        assert_eq!(restored.retention_stats(), RetentionStats::default());
        restored.arm_retention_frontier(policy(4, 30));
        restored
            .register_retention_consumer(accounting, 50)
            .expect("the durable point vouches the standing 30-record cut");
        drive(&mut restored, 4, 6);
        assert_eq!(restored.retention_stats().compactions, 1);
        assert_eq!(restored.retention_stats().summarized_total, 30);
        assert_eq!(
            restored.save_bytes(),
            control.save_bytes(),
            "restore → re-register → continue is byte-identical to the control"
        );
        // The failure to re-register: the restored process re-arms
        // but never registers — it REFUSES at every crossing, never
        // cutting on the prior process's vouch.
        let mut unregistered: Spine<SustainedWorld> =
            Spine::restore(&bytes, Diagnostics::new(Level::Off)).expect("restore");
        unregistered.arm_retention_frontier(policy(4, 30));
        unregistered.run_ticks(4);
        let stats = unregistered.retention_stats();
        assert_eq!(stats.compactions, 0);
        assert_eq!(
            stats.frontier_hold_ticks, 3,
            "ticks 7, 8, and 9 crossed and held"
        );
        assert!(
            matches!(unregistered.history_kind(), HistoryKind::Compacted { .. }),
            "the wire-carried cut persists; the cadence simply never adds to it"
        );
        assert_eq!(unregistered.ledger_retained().len(), 20 + 40);
    }

    /// THE TWIN DISCIPLINE THROUGH THE FRONTIER (the :6267 discipline
    /// extended): a frontier-armed spine whose consumer releases
    /// everything each tick (the clamp never binds) is
    /// indistinguishable from the no-policy twin in every
    /// authoritative observable at EVERY tick — stats, queues,
    /// watermarks, the report — while the twin's FULL ledger is the
    /// verbatim-suffix oracle at every cut: the frontier cadence
    /// reproduces the classic sawtooth exactly.
    #[test]
    fn retention_frontier_cadence_matches_the_no_policy_twin_at_every_tick() {
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
        spine.arm_retention_frontier(policy(10, 300));
        let accounting = consumer("accounting");
        spine
            .register_retention_consumer(accounting, 0)
            .expect("register");
        let mut twin = build();
        // The sawtooth: (retained length, cumulative firings) — the
        // :6267 table, reproduced through the frontier.
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
            // The consumer absorbs the tick's records after the tick
            // (its durable cadence) — the next checkpoint sees them.
            spine
                .acknowledge_retention_through(accounting, tick as u64 * 100)
                .expect("monotone");
            assert_eq!(spine.ledger_retained().len() as u64, len);
            assert_eq!(spine.retention_stats().compactions, firings);
            assert_eq!(spine.stats(), twin.stats(), "tick {tick}: stats");
            assert_eq!(spine.pending_count(), twin.pending_count());
            assert_eq!(spine.blocked_count(), twin.blocked_count());
            assert_eq!(spine.held_count(), twin.held_count());
            assert_eq!(spine.max_executed_due(), twin.max_executed_due());
            assert_eq!(spine.processed_through(), twin.processed_through());
            assert_eq!(spine.lane_report(), twin.lane_report());
            if firings > 0 {
                assert_eq!(spine.retention_stats().last_chunk, 300);
                let HistoryKind::Compacted {
                    summarized_records,
                    retained_records,
                    ..
                } = spine.history_kind()
                else {
                    panic!("tick {tick}: a firing compacted the spine");
                };
                assert_eq!(
                    summarized_records + retained_records,
                    spine.stats().executed,
                    "tick {tick}: count identity"
                );
                let twin_ledger = twin.ledger();
                assert_eq!(
                    spine.ledger_retained(),
                    &twin_ledger[twin_ledger.len() - len as usize..],
                    "tick {tick}: the tail is the control's verbatim suffix"
                );
            }
        }
        assert_eq!(spine.retention_stats().frontier_hold_ticks, 0);
        assert_eq!(spine.retention_stats().retention_frontier_lag_records, 0);
    }

    /// THE RATE REGIMES (the §5.6 matrix): the consumer's
    /// acknowledgment rate against the arrival rate governs the
    /// ledger — a consumer slower than the arrivals nets the ledger
    /// UPWARD (observable in the frontier lag, never resolved by
    /// discarding a record); a consumer at the arrival rate under a
    /// chunk that covers it holds the bound; a chunk below the
    /// arrival rate nets upward through the frontier exactly as the
    /// documented chunk misconfiguration rule does without it.
    #[test]
    fn retention_frontier_consumer_rate_governs_the_ledger_bound() {
        // SLOWER THAN THE ARRIVALS (5 acked per tick against 10
        // executed): clamped firings, the ledger nets upward ~5 per
        // tick, the frontier lag exposes the shortfall, nothing is
        // ever discarded.
        let mut slow = Spine::new(SustainedWorld, 10);
        for i in 0..10u64 {
            slow.schedule(spec(5, Field::Near, 1, i)).expect("schedule");
        }
        slow.arm_retention_frontier(policy(4, 30));
        let accounting = consumer("accounting");
        slow.register_retention_consumer(accounting, 0)
            .expect("register");
        let mut acked = 0u64;
        let mut last_len = 0u64;
        for tick in 1..=8u64 {
            slow.run_tick();
            acked += 5;
            slow.acknowledge_retention_through(accounting, acked)
                .expect("monotone");
            let len = slow.ledger_retained().len() as u64;
            if tick >= 5 {
                assert!(
                    len > last_len,
                    "tick {tick}: the consumer-limited ledger nets upward"
                );
            }
            last_len = len;
            let stats = slow.stats();
            assert_eq!(
                stats.scheduled,
                stats.executed
                    + slow.pending_count() as u64
                    + slow.blocked_count() as u64
                    + slow.held_count() as u64,
                "tick {tick}: conservation — a held frontier never discards"
            );
            if let HistoryKind::Compacted {
                summarized_records,
                retained_records,
                ..
            } = slow.history_kind()
            {
                assert_eq!(
                    summarized_records + retained_records,
                    stats.executed,
                    "tick {tick}: count identity"
                );
            }
        }
        assert_eq!(slow.stats().executed, 80);
        assert_eq!(slow.ledger_retained().len(), 45);
        assert_eq!(
            slow.retention_stats().retention_frontier_lag_records,
            25,
            "the shortfall is exposed: the policy wants 30, the consumer releases 5"
        );
        // AT THE ARRIVAL RATE (10 per tick, chunk 30 covers it): the
        // bound holds at every tick boundary.
        let mut steady = Spine::new(SustainedWorld, 10);
        for i in 0..10u64 {
            steady
                .schedule(spec(5, Field::Near, 1, i))
                .expect("schedule");
        }
        steady.arm_retention_frontier(policy(4, 30));
        steady
            .register_retention_consumer(accounting, 0)
            .expect("register");
        for tick in 1..=9u64 {
            steady.run_tick();
            steady
                .acknowledge_retention_through(accounting, tick * 10)
                .expect("monotone");
            assert!(
                steady.ledger_retained().len() as u64 <= 34,
                "tick {tick}: the bound holds"
            );
        }
        // CHUNK BELOW THE ARRIVAL RATE (chunk 5 against 10 per
        // tick): the ledger nets upward through the frontier exactly
        // as the documented misconfiguration rule does — the
        // consumer is not the limiter, and its lag is not blamed.
        let mut choked = Spine::new(SustainedWorld, 10);
        for i in 0..10u64 {
            choked
                .schedule(spec(5, Field::Near, 1, i))
                .expect("schedule");
        }
        choked.arm_retention_frontier(policy(4, 5));
        choked
            .register_retention_consumer(accounting, 0)
            .expect("register");
        for tick in 1..=6u64 {
            choked.run_tick();
            choked
                .acknowledge_retention_through(accounting, tick * 10)
                .expect("monotone");
        }
        assert_eq!(choked.stats().executed, 60);
        assert!(
            choked.ledger_retained().len() as u64 > 34,
            "the chunk-limited ledger nets upward (the configuration rule)"
        );
        assert_eq!(
            choked.retention_stats().retention_frontier_lag_records,
            0,
            "the consumer released everything the policy wanted — the chunk is the limiter"
        );
    }

    /// THE MULTI-CONSUMER MIN (the §5.6 closing case): with two
    /// registered consumers the LAGGING one gates the cut — the
    /// frontier is the minimum acknowledgment — and
    /// `retention_consumer_lag` NAMES the gate (the leader reports
    /// zero, the laggard reports its distance behind).
    #[test]
    fn retention_frontier_min_over_consumers_names_the_gate() {
        let mut spine = Spine::new(SustainedWorld, 10);
        for i in 0..10u64 {
            spine
                .schedule(spec(5, Field::Near, 1, i))
                .expect("schedule");
        }
        spine.arm_retention_frontier(policy(4, 30));
        let audit = consumer("audit");
        let accounting = consumer("accounting");
        spine
            .register_retention_consumer(audit, 0)
            .expect("register");
        spine
            .register_retention_consumer(accounting, 0)
            .expect("register");
        // The leader absorbs everything; the auditor lags at 5/tick.
        for tick in 1..=6u64 {
            spine.run_tick();
            spine
                .acknowledge_retention_through(accounting, tick * 10)
                .expect("monotone");
            spine
                .acknowledge_retention_through(audit, tick * 5)
                .expect("monotone");
        }
        // 60 executed; the auditor's acknowledgment gated every
        // clamped firing (cuts 15 + 5 + 5 = 25 — exactly the
        // auditor's released prefix at each crossing).
        let HistoryKind::Compacted {
            summarized_records, ..
        } = spine.history_kind()
        else {
            panic!("the clamped cadence compacted the spine");
        };
        assert_eq!(summarized_records, 25);
        assert_eq!(
            spine.retention_stats().summarized_total,
            25,
            "the lagging consumer's ack is exactly what was cut"
        );
        assert_eq!(spine.ledger_retained().len(), 35);
        // The named gate: the leader (accounting, acked 60) reports
        // zero; the auditor (acked 30) reports its 30-ordinal
        // distance behind the leader.
        assert_eq!(spine.retention_consumer_lag(audit), Some(30));
        assert_eq!(
            spine.retention_consumer_lag(accounting),
            Some(0),
            "the leader reports zero — the auditor is the named gate"
        );
        // The tick-6 crossing: the policy wanted 30, the auditor had
        // released 5 above the standing prefix — lag 25.
        assert_eq!(spine.retention_stats().retention_frontier_lag_records, 25);
    }

    /// LATE JOIN (the §5.7 register rule, live): a consumer
    /// registering after a firing vouches the standing cut (its
    /// durable read point is at or above the summarized prefix) and
    /// holds the NEXT firing at its own pace — the join is honest
    /// about what is already unrecoverable, and the fresh consumer's
    /// acknowledgment is exactly as binding as the veteran's.
    /// Also pins `retention_ordinal_of` (the driver-cadence
    /// convenience): a retained identity's global ordinal is
    /// `summarized + tail position + 1`, and a summarized or
    /// unexecuted identity reports `None` (the prefix cannot be
    /// mapped back, by design).
    #[test]
    fn retention_frontier_late_join_vouches_the_standing_cut() {
        let mut spine = Spine::new(SustainedWorld, 10);
        for i in 0..10u64 {
            spine
                .schedule(spec(5, Field::Near, 1, i))
                .expect("schedule");
        }
        spine.arm_retention_frontier(policy(4, 30));
        let veteran = consumer("veteran");
        spine
            .register_retention_consumer(veteran, 0)
            .expect("register");
        for tick in 1..=4u64 {
            spine.run_tick();
            spine
                .acknowledge_retention_through(veteran, tick * 10)
                .expect("monotone");
        }
        // The tick-4 firing cut 30 (summarized 30, retained 10).
        assert_eq!(spine.retention_stats().summarized_total, 30);
        assert_eq!(spine.ledger_retained().len(), 10);
        // The late joiner vouches the standing cut and registers at
        // its own durable point above it.
        let late = consumer("late-joiner");
        spine
            .register_retention_consumer(late, 30)
            .expect("vouching the standing cut registers");
        // Ticks 5–7 with the veteran releasing and the joiner
        // holding: the tick-7 crossing (40 > 34) refuses — the
        // joiner's ack is exactly as binding.
        for tick in 5..=7u64 {
            spine.run_tick();
            spine
                .acknowledge_retention_through(veteran, tick * 10)
                .expect("monotone");
        }
        let stats = spine.retention_stats();
        assert_eq!(
            stats.compactions, 1,
            "the tick-7 crossing held at the joiner"
        );
        assert_eq!(stats.frontier_hold_ticks, 1);
        assert_eq!(spine.retention_consumer_lag(late), Some(70 - 30));
        assert_eq!(spine.retention_consumer_lag(veteran), Some(0));
        // The joiner releases: the next crossing fires, clamped from
        // the new `summarized_records`.
        spine
            .acknowledge_retention_through(late, 50)
            .expect("monotone");
        spine.run_tick();
        let stats = spine.retention_stats();
        assert_eq!(stats.compactions, 2);
        assert_eq!(
            stats.last_chunk, 20,
            "the clamp is min(excess, chunk, 50 − 30)"
        );
        assert_eq!(spine.ledger_retained().len(), 80 - 30 - 20);
        // The ordinal convenience: the first retained record is
        // ordinal 51 (summarized 50 + position 0 + 1); the
        // summarized identity #1 and an unexecuted identity report
        // None.
        assert_eq!(
            spine.retention_ordinal_of(spine.ledger_retained()[0].id),
            Some(51)
        );
        assert_eq!(spine.retention_ordinal_of(EventId { seq: 0 }), None);
        assert_eq!(spine.retention_ordinal_of(EventId { seq: 100_000 }), None);
    }

    /// CLASS-1 COMPAT (the §5.8 bar): every historical
    /// `ConsequenceEligibility` call site compiles unchanged against
    /// the deprecated alias, and a permanently-free-armed cadence —
    /// through either constructor — reproduces the classic sawtooth
    /// (the :6267 shape) with the frontier counters untouched: the
    /// world-class capability leaves the cut at `min(excess,
    /// chunk)`, unclamped, exactly the pre-T07 firing math.
    #[test]
    #[allow(deprecated)]
    fn permanently_free_eligibility_and_the_alias_reproduce_the_classic_sawtooth() {
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
        // The deprecated alias path (the CA-07 call sites, unchanged).
        let mut aliased = build();
        aliased.set_retention_policy(policy(10, 300), ConsequenceEligibility::acknowledged());
        assert!(aliased.retention_armed());
        // The renamed Class-1 path.
        let mut renamed = build();
        renamed.set_retention_policy(policy(10, 300), ConsequenceFreedom::permanently_free());
        assert!(renamed.retention_armed());
        for spine in [&mut aliased, &mut renamed] {
            spine.run_ticks(10);
        }
        for (name, spine) in [("alias", &aliased), ("renamed", &renamed)] {
            let stats = spine.retention_stats();
            assert_eq!(stats.compactions, 3, "{name}: firings at ticks 4/7/10");
            assert_eq!(stats.summarized_total, 900);
            assert_eq!(stats.last_chunk, 300);
            assert_eq!(stats.frontier_hold_ticks, 0, "{name}: no frontier is armed");
            assert_eq!(
                stats.retention_frontier_lag_records, 0,
                "{name}: no frontier is armed"
            );
            assert_eq!(spine.ledger_retained().len(), 100);
        }
        assert_eq!(
            aliased.save_bytes(),
            renamed.save_bytes(),
            "the two Class-1 constructors agree byte for byte"
        );
    }

    /// MANUAL AUTHORITY (the §5.10 bar, the audit's explicit clause):
    /// an explicit [`Spine::compact`] between frontier-held firings
    /// SUCCEEDS — the frontier polices only the automatic path, the
    /// operator who explicitly cuts takes the responsibility
    /// explicitly — and the frontier accounting stays COHERENT
    /// fail-closed around it: a manual cut past a consumer's ack
    /// leaves the clamp saturating at zero (the frontier holds until
    /// the consumer's acknowledgment catches up past the new
    /// `summarized_records`), and once it catches up the next firing
    /// is measured from the new standing prefix.
    #[test]
    fn explicit_compact_between_frontier_holdings_keeps_the_frontier_coherent() {
        let mut spine = Spine::new(SaveableNoopWorld, 100);
        for t in 1..=10u64 {
            for i in 0..100u64 {
                spine
                    .schedule(spec(0, Field::Near, t, t * 100 + i))
                    .expect("schedule");
            }
        }
        spine.run_ticks(6);
        assert_eq!(spine.ledger_retained().len(), 600);
        spine.arm_retention_frontier(policy(10, 300));
        let accounting = consumer("accounting");
        spine
            .register_retention_consumer(accounting, 0)
            .expect("register");
        // The held crossing: the policy wants 300, the consumer has
        // released nothing — the frontier refuses.
        spine.run_tick();
        let stats = spine.retention_stats();
        assert_eq!(stats.compactions, 0);
        assert_eq!(stats.frontier_hold_ticks, 1);
        assert_eq!(stats.retention_frontier_lag_records, 300);
        // The manual authority: the operator cuts 300 anyway —
        // PAST the consumer's 0-ordinal ack. The explicit call
        // succeeds (manual responsibility, never frontier-policed).
        spine
            .compact(300)
            .expect("the manual cut is the operator's authority");
        let HistoryKind::Compacted {
            summarized_records, ..
        } = spine.history_kind()
        else {
            panic!("the manual cut compacted the spine");
        };
        assert_eq!(summarized_records, 400);
        // Coherent fail-closed accounting: the consumer's ack (0)
        // now sits BELOW the standing cut, the clamp saturates at
        // zero, and the frontier holds until the ack catches up —
        // the ack may advance to a point still below the standing
        // cut (progress toward catching up, monotone vs its own
        // prior value only).
        spine.run_tick();
        assert_eq!(
            spine.retention_stats().compactions,
            0,
            "the clamp saturates: releasable is ack 0 − summarized 400"
        );
        spine
            .acknowledge_retention_through(accounting, 250)
            .expect("monotone vs the consumer's own ack, though below the standing cut");
        spine.run_tick();
        assert_eq!(
            spine.retention_stats().compactions,
            0,
            "still saturating: releasable is ack 250 − summarized 400"
        );
        // Catch-up: the ack passes the standing cut and the next
        // crossing fires, measured from the NEW summarized_records.
        spine
            .acknowledge_retention_through(accounting, 500)
            .expect("monotone");
        spine.run_tick();
        let stats = spine.retention_stats();
        assert_eq!(stats.compactions, 1);
        assert_eq!(
            stats.last_chunk, 100,
            "the clamp is min(excess, chunk, 500 − 400)"
        );
        assert_eq!(spine.ledger_retained().len(), 1000 - 400 - 100);
        // The count identity survived the interleave.
        let HistoryKind::Compacted {
            summarized_records,
            retained_records,
            ..
        } = spine.history_kind()
        else {
            panic!("the frontier firing compacted the spine");
        };
        assert_eq!(
            summarized_records + retained_records,
            spine.stats().executed
        );
    }

    // -------------------------------------- CA-08 class-index tests (F08)

    /// The fixture's target class — a foreign tag like any other
    /// (the spine treats classes as opaque; the tests only need one
    /// stable key the foreign population never uses).
    const TARGET_CLASS: EventClass = EventClass::new(7);

    /// The absent class every "materialize without moving anything"
    /// call names (no fixture schedules class `u16::MAX`).
    const ABSENT_CLASS: EventClass = EventClass::new(u16::MAX);

    /// The equivalence battery's shared dependency anchors, so the
    /// park/release shapes can reference them by identity.
    struct ClassFixtureDeps {
        /// Due at tick 2 — resolves inside the fixture's two driven
        /// ticks (the partial-release and full-release groups key on
        /// it).
        resolves: EventId,
        /// Due far in the future — never resolves inside any fixture
        /// phase (the still-blocked groups key on it).
        never: EventId,
        /// Due at tick 2 in the near lane (the second resolver).
        resolves_near: EventId,
    }

    /// Builds one CA-08 fixture twin. Deterministic seq allocation:
    /// the three dependency anchors first, then the shape's entries —
    /// identical on every call, so the three twins of one shape start
    /// byte-identical. Shapes (the CA-03 lane's suspension templates):
    ///
    /// * 0 — the balanced mix: foreign queued in both lanes with
    ///   interleaved dues, target queued in both lanes, target and
    ///   foreign held, target blocked on the never-resolving anchor.
    /// * 1 — high fan-in: sixteen target entries all blocked on the
    ///   SAME never-resolving anchor (one waiter list, sixteen
    ///   registrations), plus foreign queued.
    /// * 2 — partial release: target groups with `[resolves, never]`
    ///   (parks, then partially RELEASED through the release path —
    ///   remaining 1, still blocked), `[resolves, resolves_near]`
    ///   (parks, then fully released and re-queued), and
    ///   `[never, resolves_near]` (parks, partially released).
    /// * 3 — the empty class: the target has no member anywhere.
    /// * 4 — all-held: the target lives only in the held set (the
    ///   double-deactivation shape).
    ///
    /// Two ticks are driven so the parks and the partial/full
    /// releases happen BEFORE the twins diverge in driving style.
    fn build_class_shape(shape: usize) -> (Spine<SaveableNoopWorld>, ClassFixtureDeps) {
        let mut spine = Spine::new(SaveableNoopWorld, 200);
        let deps = ClassFixtureDeps {
            resolves: spine
                .schedule(spec(3, Field::Far, 2, 0))
                .expect("anchor scheduling is valid"),
            never: spine
                .schedule(spec(4, Field::Far, 10_001, 0))
                .expect("anchor scheduling is valid"),
            resolves_near: spine
                .schedule(spec(5, Field::Near, 2, 0))
                .expect("anchor scheduling is valid"),
        };
        let target_at = |due: u64, payload: u64, field: Field, depends: Vec<EventId>| {
            EventSpec::new(
                TARGET_CLASS,
                Schedule::AtLogicalTime { due: Tick(due) },
                payload,
            )
            .in_field(field)
            .with_dependencies(depends)
        };
        match shape {
            0 => {
                for i in 0..56u64 {
                    let class = u16::try_from(i % 7).expect("class fits u16");
                    let field = if i.is_multiple_of(2) {
                        Field::Near
                    } else {
                        Field::Far
                    };
                    spine
                        .schedule(spec(class, field, 100 + (i % 7) * 10, i))
                        .expect("fixture scheduling is valid");
                }
                for i in 0..10u64 {
                    let field = if i.is_multiple_of(2) {
                        Field::Near
                    } else {
                        Field::Far
                    };
                    spine
                        .schedule(spec(7, field, 100 + (i % 5) * 20, 1_000 + i))
                        .expect("fixture scheduling is valid");
                }
                for i in 0..4u64 {
                    spine
                        .schedule_held(spec(7, Field::Near, 200 + i * 10, 2_000 + i))
                        .expect("held scheduling is valid");
                }
                for i in 0..8u64 {
                    spine
                        .schedule_held(spec(
                            u16::try_from(i % 6).expect("class fits u16"),
                            Field::Far,
                            210 + i * 10,
                            3_000 + i,
                        ))
                        .expect("held scheduling is valid");
                }
                for i in 0..3u64 {
                    spine
                        .schedule(target_at(
                            1,
                            4_000 + i,
                            Field::Near,
                            vec![deps.resolves, deps.never],
                        ))
                        .expect("fixture scheduling is valid");
                }
            }
            1 => {
                for i in 0..16u64 {
                    spine
                        .schedule(target_at(
                            1,
                            5_000 + i,
                            if i.is_multiple_of(2) {
                                Field::Near
                            } else {
                                Field::Far
                            },
                            vec![deps.never, deps.never],
                        ))
                        .expect("fixture scheduling is valid");
                }
                for i in 0..24u64 {
                    spine
                        .schedule(spec(
                            u16::try_from(i % 6).expect("class fits u16"),
                            if i.is_multiple_of(3) {
                                Field::Far
                            } else {
                                Field::Near
                            },
                            100 + (i % 5) * 10,
                            6_000 + i,
                        ))
                        .expect("fixture scheduling is valid");
                }
            }
            2 => {
                for i in 0..4u64 {
                    spine
                        .schedule(target_at(
                            1,
                            7_000 + i,
                            Field::Near,
                            vec![deps.resolves, deps.never],
                        ))
                        .expect("fixture scheduling is valid");
                }
                for i in 0..4u64 {
                    spine
                        .schedule(target_at(
                            1,
                            7_100 + i,
                            Field::Far,
                            vec![deps.resolves, deps.resolves_near],
                        ))
                        .expect("fixture scheduling is valid");
                }
                for i in 0..4u64 {
                    spine
                        .schedule(target_at(
                            1,
                            7_200 + i,
                            Field::Near,
                            vec![deps.never, deps.resolves_near],
                        ))
                        .expect("fixture scheduling is valid");
                }
                for i in 0..20u64 {
                    spine
                        .schedule(spec(
                            u16::try_from(i % 6).expect("class fits u16"),
                            if i.is_multiple_of(4) {
                                Field::Far
                            } else {
                                Field::Near
                            },
                            100 + (i % 6) * 10,
                            7_300 + i,
                        ))
                        .expect("fixture scheduling is valid");
                }
            }
            3 => {
                for i in 0..40u64 {
                    spine
                        .schedule(spec(
                            u16::try_from(i % 6).expect("class fits u16"),
                            if i.is_multiple_of(2) {
                                Field::Near
                            } else {
                                Field::Far
                            },
                            100 + (i % 8) * 10,
                            8_000 + i,
                        ))
                        .expect("fixture scheduling is valid");
                }
            }
            4 => {
                for i in 0..8u64 {
                    spine
                        .schedule_held(spec(7, Field::Near, 150 + i * 10, 9_000 + i))
                        .expect("held scheduling is valid");
                }
                for i in 0..30u64 {
                    spine
                        .schedule(spec(
                            u16::try_from(i % 6).expect("class fits u16"),
                            if i.is_multiple_of(2) {
                                Field::Near
                            } else {
                                Field::Far
                            },
                            100 + (i % 7) * 10,
                            9_100 + i,
                        ))
                        .expect("fixture scheduling is valid");
                }
            }
            other => panic!("unknown CA-08 fixture shape {other}"),
        }
        spine.run_ticks(2);
        (spine, deps)
    }

    /// The from-scratch recomputation of the class index from
    /// canonical state — the test-local mirror of
    /// `materialize_class_index` (the `rebuilt_witness_index`
    /// precedent): the maintained index must equal it at every
    /// compared boundary, on the indexed-driven lineage AND on the
    /// generic-driven lineage with the index live.
    fn rebuilt_class_index(spine: &Spine<SaveableNoopWorld>) -> ClassIndex {
        let mut index: ClassIndex = BTreeMap::new();
        for Reverse(pending) in spine.near_queue.iter() {
            class_track_queued(
                &mut index,
                pending.event.class,
                Field::Near,
                pending.due,
                pending.seq,
            );
        }
        for Reverse(pending) in spine.far_queue.iter() {
            class_track_queued(
                &mut index,
                pending.event.class,
                Field::Far,
                pending.due,
                pending.seq,
            );
        }
        for (seq, event) in &spine.blocked {
            class_track_blocked(&mut index, event.class, *seq);
        }
        for (seq, pending) in &spine.held {
            class_track_held(&mut index, pending.event.class, *seq);
        }
        index
    }

    /// The full observable comparison across the three twins: the
    /// canonical save bytes (which cover clock, frontier, every Stats
    /// counter, both lanes, the blocked and held sets, the ledger,
    /// and the world), the lane/held/engine fingerprint domains, the
    /// never-fingerprinted observational counters (suspension and
    /// housekeeping — the CA-03 semantics must move identically),
    /// and the coarse counts.
    fn assert_class_twin_step(
        generic: &Spine<SaveableNoopWorld>,
        generic_live: &Spine<SaveableNoopWorld>,
        indexed: &Spine<SaveableNoopWorld>,
        shape: usize,
        boundary: &str,
    ) {
        let bytes = generic.save_bytes();
        assert_eq!(
            generic_live.save_bytes(),
            bytes,
            "shape {shape} {boundary}: the generic path with a live index is byte-identical"
        );
        assert_eq!(
            indexed.save_bytes(),
            bytes,
            "shape {shape} {boundary}: the indexed path is byte-identical"
        );
        for (label, spine) in [("generic-live", generic_live), ("indexed", indexed)] {
            assert_eq!(
                spine.lane_fingerprint(),
                generic.lane_fingerprint(),
                "shape {shape} {boundary}: {label} lane fingerprint"
            );
            assert_eq!(
                spine.held_fingerprint(),
                generic.held_fingerprint(),
                "shape {shape} {boundary}: {label} held fingerprint"
            );
            assert_eq!(
                spine.engine_fingerprint(),
                generic.engine_fingerprint(),
                "shape {shape} {boundary}: {label} engine fingerprint"
            );
            assert_eq!(
                spine.suspension_stats(),
                generic.suspension_stats(),
                "shape {shape} {boundary}: {label} suspension stats"
            );
            assert_eq!(
                spine.housekeeping_stats(),
                generic.housekeeping_stats(),
                "shape {shape} {boundary}: {label} housekeeping stats"
            );
            assert_eq!(
                (
                    spine.pending_count(),
                    spine.blocked_count(),
                    spine.held_count(),
                    spine.clock()
                ),
                (
                    generic.pending_count(),
                    generic.blocked_count(),
                    generic.held_count(),
                    generic.clock()
                ),
                "shape {shape} {boundary}: {label} coarse counts"
            );
        }
    }

    /// The generic suspension predicate's visitation record:
    /// `(phase, due, seq, class)` with phase 0 = queued-near,
    /// 1 = queued-far, 2 = blocked, 3 = held.
    type ClassVisit = (u8, u64, u64, u16);

    fn visit_phase(view: &ObligationView) -> u8 {
        match (view.location, view.field) {
            (ObligationLocation::Queued, Field::Near) => 0,
            (ObligationLocation::Queued, Field::Far) => 1,
            (ObligationLocation::Blocked, _) => 2,
            (ObligationLocation::Held, _) => 3,
        }
    }

    /// IA-02's canonicity discipline, restated for the battery: the
    /// generic predicate's visits arrive near-first then far, each
    /// lane ascending `(due, seq)`, then the blocked set ascending
    /// seq — the index's existence (and liveness) must never perturb
    /// this order.
    fn assert_canonical_class_visitation(visits: &[ClassVisit], context: &str) {
        let mut phase = 0u8;
        let mut last: Option<(u64, u64)> = None;
        for &(visited_phase, due, seq, _) in visits {
            assert!(
                visited_phase >= phase,
                "{context}: phase order near→far→blocked/held violated at ({visited_phase}, {due}, {seq})"
            );
            if visited_phase != phase {
                phase = visited_phase;
                last = None;
            }
            if let Some((last_due, last_seq)) = last {
                let ascending = if phase <= 1 {
                    (due, seq) > (last_due, last_seq)
                } else {
                    seq > last_seq
                };
                assert!(
                    ascending,
                    "{context}: phase {phase} visitation not canonical at ({due}, {seq}) after ({last_due}, {last_seq})"
                );
            }
            last = Some((due, seq));
        }
    }

    /// THE EQUIVALENCE PROOF (the lane's first mandatory test): one
    /// logical transition sequence — suspend, save/restore boundary,
    /// resume, reclassify, re-suspend, execution ticks — applied to
    /// three byte-identical twins of every fixture shape: the
    /// generic predicate APIs (index never materialized), the SAME
    /// generic APIs with the index live (the coexistence proof — the
    /// general path's own maintenance sites keep it exact), and the
    /// indexed APIs. Every observable — canonical save bytes, all
    /// three fingerprint domains, suspension AND housekeeping
    /// counters, counts — is identical at every boundary; the
    /// generic predicate's visitation is canonical and identical
    /// whether or not the index is live; and both live lineages'
    /// maintained indexes equal the from-scratch rebuild (the
    /// rebuild-equals-maintained invariant).
    #[test]
    fn class_indexed_transitions_match_the_predicate_forms_bit_for_bit() {
        for shape in 0..5usize {
            let (mut generic, _) = build_class_shape(shape);
            let (mut generic_live, _) = build_class_shape(shape);
            let (mut indexed, _) = build_class_shape(shape);
            assert!(
                generic.class_index.is_none(),
                "shape {shape}: the plain twin never materializes"
            );
            // Materialize the second generic twin WITHOUT moving
            // anything (an absent class: build + early exit).
            generic_live.suspend_class(ABSENT_CLASS);
            assert!(generic_live.class_index.is_some());

            // Phase 1 — suspend, with the visitation recorded on both
            // generic twins.
            let mut visits: Vec<ClassVisit> = Vec::new();
            let mut visits_live: Vec<ClassVisit> = Vec::new();
            let g_count = generic.suspend_matching(|view| {
                visits.push((
                    visit_phase(view),
                    view.due.as_u64(),
                    view.id.seq,
                    view.class.0,
                ));
                view.class == TARGET_CLASS
            });
            let g2_count = generic_live.suspend_matching(|view| {
                visits_live.push((
                    visit_phase(view),
                    view.due.as_u64(),
                    view.id.seq,
                    view.class.0,
                ));
                view.class == TARGET_CLASS
            });
            let i_count = indexed.suspend_class(TARGET_CLASS);
            assert_eq!(
                g2_count, g_count,
                "shape {shape}: the live-index generic twin moves the same count"
            );
            assert_eq!(
                i_count, g_count,
                "shape {shape}: the indexed suspend returns the predicate form's count"
            );
            assert_eq!(
                visits, visits_live,
                "shape {shape}: the index's liveness never perturbs the generic visitation"
            );
            assert_canonical_class_visitation(&visits, &format!("shape {shape} suspend"));
            assert_class_twin_step(&generic, &generic_live, &indexed, shape, "after suspend");
            assert_eq!(
                generic_live.class_index,
                Some(rebuilt_class_index(&generic_live)),
                "shape {shape}: the generic path's maintenance keeps the index exact"
            );
            assert_eq!(
                indexed.class_index,
                Some(rebuilt_class_index(&indexed)),
                "shape {shape}: the indexed path's maintenance keeps the index exact"
            );

            // Phase 2 — the save/restore boundary: identical bytes
            // across the wire, the restored indexed twin starting
            // unmaterialized (wire-invisibility), then the same
            // resume + reclassify on all three.
            let bytes = generic.save_bytes();
            let mut generic_r =
                Spine::restore(&bytes, Diagnostics::new(Level::Off)).expect("restore");
            let mut generic_live_r =
                Spine::restore(&bytes, Diagnostics::new(Level::Off)).expect("restore");
            let mut indexed_r =
                Spine::restore(&bytes, Diagnostics::new(Level::Off)).expect("restore");
            assert_eq!(generic_r.save_bytes(), bytes);
            assert_eq!(indexed_r.save_bytes(), bytes);
            assert!(
                indexed_r.class_index.is_none(),
                "shape {shape}: the index never reaches the wire"
            );
            // Re-materialize the live-generic twin through the same
            // absent-class call, so its post-restore generic driving
            // again exercises the maintenance sites.
            generic_live_r.suspend_class(ABSENT_CLASS);

            let mut resume_visits: Vec<ClassVisit> = Vec::new();
            let g_resume = generic_r.resume_matching(|view| {
                resume_visits.push((
                    visit_phase(view),
                    view.due.as_u64(),
                    view.id.seq,
                    view.class.0,
                ));
                view.class == TARGET_CLASS
            });
            let g2_resume = generic_live_r.resume_matching(|view| view.class == TARGET_CLASS);
            let i_resume = indexed_r.resume_class(TARGET_CLASS);
            assert_eq!(g2_resume, g_resume);
            assert_eq!(i_resume, g_resume, "shape {shape}: indexed resume count");
            assert_canonical_class_visitation(&resume_visits, &format!("shape {shape} resume"));
            assert_class_twin_step(
                &generic_r,
                &generic_live_r,
                &indexed_r,
                shape,
                "after restore+resume",
            );

            let g_reclass =
                generic_r.reclassify_matching(|view| view.class == TARGET_CLASS, Field::Near);
            let g2_reclass =
                generic_live_r.reclassify_matching(|view| view.class == TARGET_CLASS, Field::Near);
            let i_reclass = indexed_r.reclassify_class(TARGET_CLASS, Field::Near);
            assert_eq!(
                g2_reclass, g_reclass,
                "shape {shape}: the live-index reclassify count"
            );
            assert_eq!(
                g_reclass, i_reclass,
                "shape {shape}: indexed reclassify count"
            );
            assert_class_twin_step(
                &generic_r,
                &generic_live_r,
                &indexed_r,
                shape,
                "after reclassify",
            );
            assert_eq!(
                generic_live_r.class_index,
                Some(rebuilt_class_index(&generic_live_r)),
                "shape {shape}: reclassify maintenance exact (generic lineage)"
            );
            assert_eq!(
                indexed_r.class_index,
                Some(rebuilt_class_index(&indexed_r)),
                "shape {shape}: reclassify maintenance exact (indexed lineage)"
            );

            // Phase 3 — the full cycle closed (re-suspend) and two
            // execution ticks: the schedule/pop/park/release hooks
            // run while the indexes are live on both lineages.
            let g_again = generic_r.suspend_matching(|view| view.class == TARGET_CLASS);
            let g2_again = generic_live_r.suspend_matching(|view| view.class == TARGET_CLASS);
            let i_again = indexed_r.suspend_class(TARGET_CLASS);
            assert_eq!(
                g2_again, g_again,
                "shape {shape}: the live-index re-suspend count"
            );
            assert_eq!(
                i_again, g_again,
                "shape {shape}: the cycle re-suspend count"
            );
            generic_r.run_ticks(2);
            generic_live_r.run_ticks(2);
            indexed_r.run_ticks(2);
            assert_class_twin_step(
                &generic_r,
                &generic_live_r,
                &indexed_r,
                shape,
                "after ticks",
            );
            assert_eq!(
                generic_live_r.class_index,
                Some(rebuilt_class_index(&generic_live_r)),
                "shape {shape}: tick-path maintenance exact (generic lineage)"
            );
            assert_eq!(
                indexed_r.class_index,
                Some(rebuilt_class_index(&indexed_r)),
                "shape {shape}: tick-path maintenance exact (indexed lineage)"
            );
        }

        // The reclassify maintenance under the OTHER direction (the
        // loop above drives Near only): fresh twins of the balanced
        // shape, both with the index live, the far lane as the
        // target on both forms — identical count, identical state,
        // both lineages' indexes exact.
        let (mut far_generic, _) = build_class_shape(0);
        let (mut far_indexed, _) = build_class_shape(0);
        far_generic.suspend_class(ABSENT_CLASS);
        far_indexed.suspend_class(ABSENT_CLASS);
        far_generic.suspend_matching(|view| view.class == TARGET_CLASS);
        far_indexed.suspend_class(TARGET_CLASS);
        far_generic.resume_matching(|view| view.class == TARGET_CLASS);
        far_indexed.resume_class(TARGET_CLASS);
        let g_far = far_generic.reclassify_matching(|view| view.class == TARGET_CLASS, Field::Far);
        let i_far = far_indexed.reclassify_class(TARGET_CLASS, Field::Far);
        assert_eq!(i_far, g_far, "the far-direction reclassify count matches");
        assert_eq!(
            far_indexed.save_bytes(),
            far_generic.save_bytes(),
            "the far-direction reclassify state matches"
        );
        assert_eq!(
            far_generic.class_index,
            Some(rebuilt_class_index(&far_generic)),
            "the generic far-direction maintenance keeps the index exact"
        );
        assert_eq!(
            far_indexed.class_index,
            Some(rebuilt_class_index(&far_indexed)),
            "the indexed far-direction maintenance keeps the index exact"
        );
    }

    /// THE WIRE-INVISIBILITY PROOF (the derived-class discipline):
    /// the index never reaches the save (a restored spine starts
    /// `None`), the first class-keyed call rebuilds the IDENTICAL
    /// index the live spine maintained (rebuild-equals-maintained),
    /// the from-scratch recomputation agrees with both, and the
    /// indexed and scan read forms emit the identical views.
    #[test]
    fn the_class_index_is_wire_invisible_and_rebuilds_identically() {
        let (mut live, _) = build_class_shape(0);
        live.suspend_class(TARGET_CLASS);
        live.run_ticks(1);
        live.resume_class(TARGET_CLASS);
        live.suspend_class(TARGET_CLASS);
        let maintained = live.class_index.clone().expect("the index is live");
        assert_eq!(maintained, rebuilt_class_index(&live));

        let bytes = live.save_bytes();
        let mut restored: Spine<SaveableNoopWorld> =
            Spine::restore(&bytes, Diagnostics::new(Level::Off))
                .expect("the fixture spine restores");
        assert!(
            restored.class_index.is_none(),
            "the class index never reaches the wire"
        );
        assert_eq!(restored.save_bytes(), bytes);
        // The read form falls back to the scan before materialization
        // — identical output, so the comparison is meaningful.
        let live_held: Vec<(u64, u16)> = live
            .held_class_view(TARGET_CLASS)
            .iter()
            .map(|view| (view.id.seq, view.class.0))
            .collect();
        let restored_held: Vec<(u64, u16)> = restored
            .held_class_view(TARGET_CLASS)
            .iter()
            .map(|view| (view.id.seq, view.class.0))
            .collect();
        assert_eq!(restored_held, live_held, "the scan fallback is identical");
        // The first class-keyed mutating call rebuilds the exact map.
        restored.suspend_class(ABSENT_CLASS);
        assert_eq!(
            restored.class_index,
            Some(maintained),
            "rebuild-equals-maintained across the wire"
        );
    }

    /// Median-of-`reps` nanoseconds for `f` (the robust statistic
    /// under the parallel test battery's noise — the witness-index
    /// lane's helper shape).
    fn class_median_ns(reps: usize, mut f: impl FnMut()) -> u64 {
        let mut samples = Vec::with_capacity(reps);
        for _ in 0..reps {
            let started = std::time::Instant::now();
            f();
            samples.push(started.elapsed().as_nanos() as u64);
        }
        samples.sort_unstable();
        samples[reps / 2]
    }

    /// The scaling fixture: the target owns EXACTLY `own` obligations
    /// (40 at every size — the fixed affected work), the unrelated
    /// population grows with `total` — foreign classes queued in both
    /// lanes AND foreign entries seeded directly into the held set
    /// (the scan form's cost is the WHOLE held map, so the unrelated
    /// held population must grow too). The target is suspended, so
    /// its members are the held set the queries walk.
    fn build_class_scaling_fixture(total: usize, own: usize) -> Spine<SaveableNoopWorld> {
        let mut spine = Spine::new(SaveableNoopWorld, 100_000);
        let held_foreign = (total / 2).saturating_sub(own);
        let queued_foreign = total - own - held_foreign;
        for i in 0..own {
            spine
                .schedule(spec(
                    7,
                    if i.is_multiple_of(2) {
                        Field::Near
                    } else {
                        Field::Far
                    },
                    500 + u64::try_from(i).expect("due fits u64") * 3,
                    i as u64,
                ))
                .expect("fixture scheduling is valid");
        }
        for i in 0..queued_foreign {
            spine
                .schedule(spec(
                    u16::try_from(i % 6).expect("class fits u16"),
                    if i.is_multiple_of(2) {
                        Field::Near
                    } else {
                        Field::Far
                    },
                    500 + u64::try_from(i % 97).expect("due fits u64") * 3,
                    100_000 + i as u64,
                ))
                .expect("fixture scheduling is valid");
        }
        for i in 0..held_foreign {
            spine
                .schedule_held(spec(
                    u16::try_from(i % 6).expect("class fits u16"),
                    if i.is_multiple_of(2) {
                        Field::Near
                    } else {
                        Field::Far
                    },
                    900 + u64::try_from(i % 89).expect("due fits u64") * 3,
                    200_000 + i as u64,
                ))
                .expect("held scheduling is valid");
        }
        spine.suspend_class(TARGET_CLASS);
        spine
    }

    /// THE SCALING EVIDENCE (the lane's second mandatory test): the
    /// affected work fixed (the target owns 40 obligations at every
    /// size) while the unrelated population grows 100×. The held-side
    /// indexed queries — `held_class_view` and `resume_class` — are
    /// FLAT in the unrelated population (the member set resolves
    /// through the held map's keyed lookup; the scan forms walk the
    /// whole map). The queue-side indexed suspend's EMPTY-membership
    /// arm — the dormant-region common case — is flat through the
    /// early exit while the generic empty-predicate suspend still
    /// sorts both lanes. The queue-side partition WITH members is
    /// measured and reported honestly: the `BinaryHeap` admits no
    /// keyed removal, so the buffer partition remains O(lane) — the
    /// canonical sort and the per-entry predicate calls are what the
    /// indexed path removes (pod-observational numbers in the
    /// unoptimized test profile; the assertions are the robust
    /// invariants only, the witness-index lane's discipline).
    #[test]
    fn class_indexed_queries_are_flat_in_unrelated_population() {
        let sizes = [1_000usize, 10_000, 100_000];
        const OWN: usize = 40;
        let mut held_scan = [0u64; 3];
        let mut held_indexed = [0u64; 3];
        let mut resume_scan = [0u64; 3];
        let mut resume_indexed = [0u64; 3];
        let mut empty_scan = [0u64; 3];
        let mut empty_indexed = [0u64; 3];
        let mut suspend_scan = [0u64; 3];
        let mut suspend_indexed = [0u64; 3];
        for (i, &total) in sizes.iter().enumerate() {
            let mut scan_spine = build_class_scaling_fixture(total, OWN);
            let mut indexed_spine = build_class_scaling_fixture(total, OWN);

            held_scan[i] = class_median_ns(9, || {
                let _ = scan_spine
                    .held_view()
                    .into_iter()
                    .filter(|view| view.class == TARGET_CLASS)
                    .count();
            });
            held_indexed[i] = class_median_ns(60, || {
                let _ = indexed_spine.held_class_view(TARGET_CLASS).len();
            });

            // The transitions are state-mutating: the cycle
            // held→queued→held repeats, each leg timed, so every
            // repetition measures the identical state and BOTH timed
            // legs move real members (the fixture enters held; the
            // resume leg queues the target's forty; the suspend leg
            // partitions them back out of the lanes).
            let mut resume_samples = Vec::new();
            let mut resume_indexed_samples = Vec::new();
            let mut suspend_samples = Vec::new();
            let mut suspend_indexed_samples = Vec::new();
            for _ in 0..9 {
                let started = std::time::Instant::now();
                scan_spine.resume_matching(|view| view.class == TARGET_CLASS);
                resume_samples.push(started.elapsed().as_nanos() as u64);
                let started = std::time::Instant::now();
                indexed_spine.resume_class(TARGET_CLASS);
                resume_indexed_samples.push(started.elapsed().as_nanos() as u64);
                let started = std::time::Instant::now();
                scan_spine.suspend_matching(|view| view.class == TARGET_CLASS);
                suspend_samples.push(started.elapsed().as_nanos() as u64);
                let started = std::time::Instant::now();
                indexed_spine.suspend_class(TARGET_CLASS);
                suspend_indexed_samples.push(started.elapsed().as_nanos() as u64);
            }
            let median = |samples: &mut Vec<u64>| {
                samples.sort_unstable();
                samples[samples.len() / 2]
            };
            resume_scan[i] = median(&mut resume_samples);
            resume_indexed[i] = median(&mut resume_indexed_samples);
            suspend_scan[i] = median(&mut suspend_samples);
            suspend_indexed[i] = median(&mut suspend_indexed_samples);

            // The empty-class arms: state-neutral on both forms (the
            // generic still drains, sorts, and re-heaps both lanes).
            empty_scan[i] = class_median_ns(9, || {
                scan_spine.suspend_matching(|view| view.class == ABSENT_CLASS);
            });
            empty_indexed[i] = class_median_ns(60, || {
                indexed_spine.suspend_class(ABSENT_CLASS);
            });

            println!(
                "ca08 scaling (population {total:6}, target owns {OWN}): held_view scan \
                 {h_scan:9} ns vs indexed {h_idx:8} ns; resume scan {r_scan:9} ns vs indexed \
                 {r_idx:8} ns; empty-class suspend scan {e_scan:9} ns vs indexed {e_idx:7} ns; \
                 with-members suspend scan {s_scan:9} ns vs indexed {s_idx:8} ns \
                 (pod-observational, test profile)",
                h_scan = held_scan[i],
                h_idx = held_indexed[i],
                r_scan = resume_scan[i],
                r_idx = resume_indexed[i],
                e_scan = empty_scan[i],
                e_idx = empty_indexed[i],
                s_scan = suspend_scan[i],
                s_idx = suspend_indexed[i],
            );
        }

        // The scan forms grow linearly with the unrelated population
        // (the 100× range must show at least 15× — expected ~50-100×;
        // the with-members suspend's scan includes the O(N log N)
        // sort, at least 10×).
        for (label, series, factor) in [
            ("held_view", &held_scan, 15u64),
            ("resume", &resume_scan, 15),
            ("empty-class suspend", &empty_scan, 10),
            ("with-members suspend", &suspend_scan, 10),
        ] {
            assert!(
                series[2] >= factor * series[0].max(1),
                "{label}: the scan form grows with the unrelated population (measured {} → {} \
                 ns across 1k → 100k)",
                series[0],
                series[2]
            );
        }
        // The indexed walks are flat-to-logarithmic (the keyed
        // lookups' depth grows with log of the population; a 100×
        // range must stay within 3× plus the constant headroom).
        for (label, series, headroom) in [
            ("held_class_view", &held_indexed, 20_000u64),
            ("resume_class", &resume_indexed, 40_000),
            ("suspend_class (empty)", &empty_indexed, 20_000),
        ] {
            assert!(
                series[2] <= 3 * series[0] + headroom,
                "{label}: the indexed query is flat in the unrelated population (measured {} → \
                 {} ns across 1k → 100k)",
                series[0],
                series[2]
            );
        }
        // The with-members queue-side partition — the honest residual
        // (O(lane buffer), no keyed heap removal): measured, reported,
        // and pinned only to never regress against the scan form at
        // the largest population.
        assert!(
            suspend_indexed[2] <= suspend_scan[2],
            "the indexed with-members suspend never regresses against the predicate form \
             (measured {} vs {} ns at 100k)",
            suspend_indexed[2],
            suspend_scan[2]
        );
    }

    /// THE UPDATE COST (the lane's third mandatory test): the index
    /// maintenance overhead on the generic main path — a
    /// schedule/execute/park workload with the index live vs never
    /// materialized. The live form pays one O(log) set insert per
    /// schedule, one removal per execution, one move per park —
    /// bounded by a small multiple of the plain path's per-op cost,
    /// growing sub-linearly (log) in the population. The lazy gate
    /// is pinned beside it: a spine whose driver never makes a
    /// class-keyed call stays unmaterialized through the whole
    /// workload — the property that keeps the `spine_allocations`
    /// pins' scenario byte-identical.
    fn run_class_workload(total: usize, materialize: bool) -> Spine<SaveableNoopWorld> {
        let mut spine = Spine::new(SaveableNoopWorld, 10_000);
        if materialize {
            spine.suspend_class(ABSENT_CLASS);
        }
        // The never-resolving anchor: every tenth entry parks on it.
        let anchor = spine
            .schedule(spec(0, Field::Far, 100_000, 0))
            .expect("anchor scheduling is valid");
        for i in 0..total {
            let spec = if i.is_multiple_of(10) {
                EventSpec::new(
                    EventClass::new(u16::try_from(i % 8).expect("class fits u16")),
                    Schedule::AtLogicalTime {
                        due: Tick(1 + u64::try_from(i % 50).expect("due fits u64")),
                    },
                    i as u64,
                )
                .in_field(if i.is_multiple_of(3) {
                    Field::Far
                } else {
                    Field::Near
                })
                .with_dependencies(vec![anchor])
            } else {
                spec(
                    u16::try_from(i % 8).expect("class fits u16"),
                    if i.is_multiple_of(3) {
                        Field::Far
                    } else {
                        Field::Near
                    },
                    1 + u64::try_from(i % 50).expect("due fits u64"),
                    i as u64,
                )
            };
            spine.schedule(spec).expect("workload scheduling is valid");
        }
        spine.run_ticks(60);
        spine
    }

    #[test]
    fn class_index_maintenance_overhead_is_bounded_and_the_index_stays_lazy() {
        for &total in &[2_000usize, 20_000] {
            let plain = class_median_ns(5, || {
                let _ = run_class_workload(total, false);
            });
            let live = class_median_ns(5, || {
                let _ = run_class_workload(total, true);
            });
            println!(
                "ca08 update cost (population {total:6}): schedule+execute workload plain \
                 {plain:11} ns vs index-live {live:11} ns ({:.2}×, pod-observational, test \
                 profile)",
                live as f64 / plain.max(1) as f64
            );
            assert!(
                live <= 6 * plain + 20_000_000,
                "population {total}: the index maintenance overhead is bounded (measured \
                 {live} ns against {plain} ns plain)"
            );
        }
        // The lazy gate — the allocation-pin scenario's guarantee,
        // restated as its own pin: no class-keyed call, no index.
        let plain_spine = run_class_workload(20_000, false);
        assert!(
            plain_spine.class_index.is_none(),
            "a spine whose driver never makes a class-keyed call never materializes the index"
        );
        // And the live workload's maintained index is exact through
        // the whole generic tick path (the coexistence proof at
        // scale).
        let live_spine = run_class_workload(20_000, true);
        assert_eq!(
            live_spine.class_index,
            Some(rebuilt_class_index(&live_spine)),
            "the generic tick path's maintenance keeps the index exact at scale"
        );
    }

    // --------------------- T09 stage-1 census batteries (the audit slope)

    /// The audit's exact slope sizes (strict-audit-IV T09): the
    /// unrelated queued population grows 1k / 5k / 20k / 100k against
    /// one active class — CA-08 measured 1k/10k/100k only; 5k and
    /// 20k are this battery's additions (the T09 design §7 stage 1).
    const T09_SLOPE_SIZES: [usize; 4] = [1_000, 5_000, 20_000, 100_000];

    /// The fixed affected work at every size — the audit's battery
    /// fixes m = 40 (the CA-08 fixture used the same).
    const T09_OWN: usize = 40;

    /// Builds the T09 stage-1 queued fixture: the target class owns
    /// EXACTLY `own` queued obligations (the fixed affected work at
    /// every size) while the unrelated foreign population grows with
    /// `total`, ALL of it queued — no held foreign seed (the held
    /// side is CA-08's measured flat ground; the queued arm is this
    /// census's subject). `one_lane` concentrates the entire
    /// population in the near lane (the worst per-lane shape — the
    /// whole unrelated population sits in the one lane the
    /// transition scans); otherwise the CA-08 alternation splits both
    /// lanes. Every due is far-future and NO tick ever runs: the
    /// battery measures the BETWEEN-TICKS transition exactly where
    /// the region director runs it (engine-slice `region.rs`'s
    /// `apply` — outside `run_tick`, never housekeeping-charged). The
    /// index is materialized WITHOUT moving the target (an
    /// absent-class `suspend_class`), so no measured window contains
    /// the one-time adoption build (CA-08's already-measured price).
    fn build_t09_queued_fixture(
        total: usize,
        own: usize,
        one_lane: bool,
    ) -> Spine<SaveableNoopWorld> {
        let mut spine = Spine::new(SaveableNoopWorld, 100_000);
        let field_of = |index: usize| {
            if one_lane || index.is_multiple_of(2) {
                Field::Near
            } else {
                Field::Far
            }
        };
        for i in 0..own {
            spine
                .schedule(spec(
                    TARGET_CLASS.0,
                    field_of(i),
                    500 + u64::try_from(i).expect("due fits u64") * 3,
                    i as u64,
                ))
                .expect("fixture scheduling is valid");
        }
        for i in 0..total - own {
            spine
                .schedule(spec(
                    u16::try_from(i % 6).expect("class fits u16"),
                    field_of(i),
                    500 + u64::try_from(i % 97).expect("due fits u64") * 3,
                    100_000 + u64::try_from(i).expect("payload fits u64"),
                ))
                .expect("fixture scheduling is valid");
        }
        spine.suspend_class(ABSENT_CLASS);
        spine
    }

    /// An `Ord`-instrumented `(due, seq)` mirror key for the census:
    /// `Pending`'s `Ord` is EXACTLY `(due, seq)` (spine.rs's own
    /// `impl Ord for Pending`), so a `BinaryHeap` over
    /// `Reverse<T09HeapKey>` holding the same keys in the same buffer
    /// order performs the IDENTICAL comparison sequence as the
    /// production `BinaryHeap<Reverse<Pending>>` heapify — the
    /// counter reads the production rebuild's exact compare count
    /// without touching production code (the stage-1
    /// zero-production-change discipline; the T09 design's stage-2
    /// inline probe replaces this mirror when those paths are edited
    /// anyway).
    struct T09HeapKey {
        due: u64,
        seq: u64,
    }

    /// The heapify compare counter (test-local; single-threaded use
    /// inside one test — reset before each measured heapify).
    static T09_HEAP_COMPARES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    impl T09HeapKey {
        /// Mirrors one retained buffer (same keys, same order) into
        /// the counting form.
        fn mirror_of(pendings: &[Reverse<Pending>]) -> Vec<Reverse<T09HeapKey>> {
            pendings
                .iter()
                .map(|Reverse(pending)| {
                    Reverse(T09HeapKey {
                        due: pending.due.0,
                        seq: pending.seq,
                    })
                })
                .collect()
        }
    }

    impl PartialEq for T09HeapKey {
        fn eq(&self, other: &Self) -> bool {
            self.due == other.due && self.seq == other.seq
        }
    }

    impl Eq for T09HeapKey {}

    impl PartialOrd for T09HeapKey {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }

    impl Ord for T09HeapKey {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            T09_HEAP_COMPARES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            (self.due, self.seq).cmp(&(other.due, other.seq))
        }
    }

    /// One census measurement of the with-members queue arm's lane
    /// transform — the step-for-step mirror of `suspend_class_queue`
    /// (into_vec, membership-keyed `extract_if`, `BinaryHeap::from`)
    /// run over a CLONE of a real fixture lane with the counting
    /// closure the production `members.contains` arm carries.
    /// Returns `(entries_touched, membership_hits, extracted,
    /// retained, heapify_compares)`.
    fn t09_census_one_lane(
        queue: &BinaryHeap<Reverse<Pending>>,
        members: &BTreeSet<(Tick, u64)>,
    ) -> (u64, u64, usize, usize, u64) {
        let mut items = queue.clone().into_vec();
        let mut entries_touched = 0u64;
        let mut membership_hits = 0u64;
        let extracted: Vec<Reverse<Pending>> = items
            .extract_if(.., |Reverse(pending)| {
                entries_touched += 1;
                let hit = members.contains(&(pending.due, pending.seq));
                membership_hits += u64::from(hit);
                hit
            })
            .collect();
        // The extraction post-condition: every extracted entry IS a
        // member, and only members were extracted (the equivalence
        // discipline's local mirror).
        for Reverse(pending) in &extracted {
            assert!(
                members.contains(&(pending.due, pending.seq)),
                "only membership-keyed entries leave the lane"
            );
        }
        let retained = items.len();
        // The re-heapify's compare count, read through the
        // counting-key mirror over the SAME retained buffer (see
        // `T09HeapKey` — the comparison sequence is identical).
        T09_HEAP_COMPARES.store(0, std::sync::atomic::Ordering::Relaxed);
        let rebuilt: BinaryHeap<Reverse<T09HeapKey>> =
            BinaryHeap::from(T09HeapKey::mirror_of(&items));
        let heapify_compares = T09_HEAP_COMPARES.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            rebuilt.len(),
            retained,
            "the mirror heap holds the retained multiset"
        );
        (
            entries_touched,
            membership_hits,
            extracted.len(),
            retained,
            heapify_compares,
        )
    }

    /// THE T09 STAGE-1 CENSUS (zero production change): what the
    /// with-members queued class transition actually touches today.
    /// The audit's finding, made executable: for a nonempty class the
    /// arm converts the ENTIRE lane heap into a Vec, `extract_if`
    /// scans ALL entries with a per-entry `BTreeSet` membership
    /// lookup, and `BinaryHeap::from` re-heapifies the whole
    /// remainder. The measured counters at the audit's four sizes:
    /// **entries touched = the whole lane population N** (every entry
    /// visits the membership predicate — N + m today, exactly m
    /// post-stage-2, the design's headline assertion), the heapify
    /// rebuild ≈ 1–2 compares per retained entry (std's bottom-up
    /// heapify — O(N), pinned below 4×N so an N·log N regression can
    /// never pass silently), and zero allocation (measured in the
    /// `spine_queued_transition_allocations` binary — `into_vec`
    /// reuses the buffer, `extract_if` is in-place, `from` adopts the
    /// Vec). Pod-observational numbers in the unoptimized test
    /// profile; the assertions are the structural invariants only
    /// (the CA-08 discipline).
    #[test]
    fn t09_queued_census_touches_every_lane_entry_today() {
        for (variant, one_lane) in [("one-lane", true), ("both-lanes", false)] {
            let mut first_compares = 0u64;
            let mut last_compares = 0u64;
            for &total in T09_SLOPE_SIZES.iter() {
                let spine = build_t09_queued_fixture(total, T09_OWN, one_lane);
                let membership = spine
                    .class_index
                    .as_ref()
                    .and_then(|index| index.get(&TARGET_CLASS))
                    .expect("the fixture materialized the class index");
                let near_members = membership.near.clone();
                let far_members = membership.far.clone();
                assert_eq!(
                    near_members.len() + far_members.len(),
                    T09_OWN,
                    "the target's queued membership is the fixed affected work"
                );
                let mut touched_total = 0u64;
                let mut hits_total = 0u64;
                let mut compares_total = 0u64;
                for (label, queue, members) in [
                    ("near", &spine.near_queue, &near_members),
                    ("far", &spine.far_queue, &far_members),
                ] {
                    if members.is_empty() {
                        // The empty-membership early exit's census row:
                        // the real arm returns BEFORE `into_vec` (the
                        // dormant-region common case — CA-08 measured
                        // it ~200 ns), so nothing is touched at all.
                        // (In these fixtures an empty member set
                        // always implies the empty lane — the one-lane
                        // variant's far side.)
                        assert!(
                            queue.is_empty(),
                            "the fixture's empty member set implies the empty lane"
                        );
                        continue;
                    }
                    let (touched, hits, extracted, retained, compares) =
                        t09_census_one_lane(queue, members);
                    assert_eq!(
                        touched,
                        queue.len() as u64,
                        "{variant} {label} (population {total}): the with-members arm \
                         touches EVERY lane entry — the census finding"
                    );
                    assert_eq!(hits, members.len() as u64);
                    assert_eq!(extracted, members.len());
                    assert_eq!(retained, queue.len() - members.len());
                    assert!(
                        compares <= 4 * retained as u64 + 64,
                        "{variant} {label} (population {total}): the re-heapify is \
                         O(N) ({compares} compares over {retained} retained)"
                    );
                    touched_total += touched;
                    hits_total += hits;
                    compares_total += compares;
                }
                assert_eq!(
                    touched_total, total as u64,
                    "{variant} (population {total}): the with-members transition \
                     touches every queued entry in the lanes it scans — N + m today"
                );
                assert_eq!(hits_total, T09_OWN as u64);
                if total == T09_SLOPE_SIZES[0] {
                    first_compares = compares_total;
                }
                if total == T09_SLOPE_SIZES[3] {
                    last_compares = compares_total;
                }
                println!(
                    "t09 census ({variant}, population {total:6}, target owns {T09_OWN}): \
                     entries touched {touched_total:6} (every lane entry), membership hits \
                     {hits_total}, heapify compares {compares_total:7} (~{:.2} per retained \
                     entry) — pod-observational, test profile",
                    compares_total as f64 / total.saturating_sub(T09_OWN).max(1) as f64
                );
            }
            assert!(
                last_compares >= 10 * first_compares.max(1),
                "{variant}: the heap-rebuild work grows with the unrelated population \
                 (measured {first_compares} → {last_compares} compares across 1k → 100k)"
            );
        }
    }

    /// THE T09 STAGE-1 SLOPE BATTERY (the audit's exact test): one
    /// active class fixed at m = 40 while the unrelated queued
    /// population grows 1k / 5k / 20k / 100k, both the one-lane and
    /// both-lanes variants, the generic predicate form
    /// (`suspend_matching`) beside the indexed form
    /// (`suspend_class`). The honest recording: CA-08 removed the
    /// O(N log N) canonical sort and the per-entry predicate calls —
    /// the measured 3.8× — but the O(lane) `extract_if` scan and the
    /// whole-remainder re-heapify remain (`BinaryHeap` admits no
    /// keyed removal), so the with-members indexed arm STILL grows
    /// linearly with the unrelated population today. This battery
    /// pins that residual as the stage-1 baseline the stage-2 indexed
    /// lane would replace (flat ≤3× + headroom, the design's §6
    /// headline assertion); until then the arm keeps CA-08's honest
    /// never-regress pin, here extended to the audit's four sizes and
    /// both lane variants. Pod-observational numbers; robust-invariant
    /// assertions only (the medians of state-mutating cycle legs, the
    /// CA-08 held→queued→held discipline — every timed leg moves the
    /// same real members).
    #[test]
    fn t09_queued_slope_at_the_audit_sizes_records_the_linear_residual() {
        for (variant, one_lane) in [("one-lane", true), ("both-lanes", false)] {
            let mut scan = [0u64; 4];
            let mut indexed = [0u64; 4];
            let mut scan_max = [0u64; 4];
            let mut indexed_max = [0u64; 4];
            for (i, &total) in T09_SLOPE_SIZES.iter().enumerate() {
                let mut scan_spine = build_t09_queued_fixture(total, T09_OWN, one_lane);
                let mut indexed_spine = build_t09_queued_fixture(total, T09_OWN, one_lane);
                let mut scan_samples = Vec::with_capacity(9);
                let mut indexed_samples = Vec::with_capacity(9);
                for _ in 0..9 {
                    let started = std::time::Instant::now();
                    let moved = scan_spine.suspend_matching(|view| view.class == TARGET_CLASS);
                    scan_samples.push(started.elapsed().as_nanos() as u64);
                    assert_eq!(
                        moved, T09_OWN as u32,
                        "the scan cycle moves the whole target membership"
                    );
                    let resumed = scan_spine.resume_matching(|view| view.class == TARGET_CLASS);
                    assert_eq!(resumed, T09_OWN as u32);

                    let started = std::time::Instant::now();
                    let moved = indexed_spine.suspend_class(TARGET_CLASS);
                    indexed_samples.push(started.elapsed().as_nanos() as u64);
                    assert_eq!(
                        moved, T09_OWN as u32,
                        "the indexed cycle moves the whole target membership"
                    );
                    let resumed = indexed_spine.resume_class(TARGET_CLASS);
                    assert_eq!(resumed, T09_OWN as u32);
                }
                let median = |samples: &mut Vec<u64>| {
                    samples.sort_unstable();
                    samples[samples.len() / 2]
                };
                scan[i] = median(&mut scan_samples);
                indexed[i] = median(&mut indexed_samples);
                // After the median sort the last sample is the max —
                // the p99-of-9 proxy, reported but never asserted.
                scan_max[i] = *scan_samples.last().expect("nine samples");
                indexed_max[i] = *indexed_samples.last().expect("nine samples");
                println!(
                    "t09 slope ({variant}, population {total:6}, target owns {T09_OWN}): \
                     with-members suspend scan {s:9} ns (max-of-9 {sm:9}) vs indexed \
                     {n:8} ns (max-of-9 {nm:8}) — pod-observational, test profile",
                    s = scan[i],
                    sm = scan_max[i],
                    n = indexed[i],
                    nm = indexed_max[i],
                );
            }
            // The scan forms grow linearly with the unrelated
            // population (CA-08's factor-10 discipline over the
            // 100× range; the 20k midpoint's 20× range gets 4×).
            // The 20k threshold recalibrated from CI actuals
            // (2026-11-29, runs 46155517625/46154762231): shared
            // runners read a noisy-high 1k baseline (2.14 ms vs the
            // pod's 0.94), compressing the measured ratio to 6.4×
            // against the original 8× — 4× retains ~1.5× headroom
            // under that noise while still discriminating a
            // logarithmic form (ln 20 ≈ 3.0×) and a constant form
            // (1×); the 100k row's 10× discriminates ln 100 ≈ 4.6×.
            // The deterministic teeth (entries touched = N exactly,
            // asserted above) carry the census; these rows are the
            // observational confirmation.
            assert!(
                scan[3] >= 10 * scan[0].max(1),
                "{variant}: the scan form grows with the unrelated population (measured \
                 {} → {} ns across 1k → 100k)",
                scan[0],
                scan[3]
            );
            assert!(
                scan[2] >= 4 * scan[0].max(1),
                "{variant}: the scan form grows into the 20k midpoint (measured {} → {} ns \
                 across 1k → 20k)",
                scan[0],
                scan[2]
            );
            // THE HONEST RESIDUAL: the with-members indexed arm grows
            // linearly TOO — structurally certain (the O(lane) scan
            // and re-heapify touch every entry; only the sort and the
            // predicate calls were removed). Recorded as the stage-1
            // baseline, never hidden.
            assert!(
                indexed[3] >= 10 * indexed[0].max(1),
                "{variant}: the with-members indexed residual grows with the unrelated \
                 population (measured {} → {} ns across 1k → 100k) — the O(lane) scan and \
                 re-heapify remain",
                indexed[0],
                indexed[3]
            );
            assert!(
                indexed[2] >= 4 * indexed[0].max(1),
                "{variant}: the indexed residual grows into the 20k midpoint (measured \
                 {} → {} ns across 1k → 20k)",
                indexed[0],
                indexed[2]
            );
            // CA-08's residual pin (:11191–11197 shape), extended to
            // the audit's sizes and both lane variants: removing the
            // canonical sort and the per-entry predicate calls must
            // never regress against the form that still pays them.
            assert!(
                indexed[3] <= scan[3],
                "{variant}: the indexed with-members suspend never regresses against the \
                 predicate form (measured {} vs {} ns at 100k)",
                indexed[3],
                scan[3]
            );
        }
    }
}
