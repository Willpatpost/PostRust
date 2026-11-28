//! (P3-L7, B13) The worker transport's queue-wait vs service-time
//! observability class — the measurement half of the deferred-
//! confirmation question.
//!
//! **Why this exists (the design's contract):** the single FIFO I/O
//! worker is the serialization point every canonical operation rides
//! (`WorkerMedium`'s send-then-`recv` round trips), and the B13
//! question — whether the canonical confirmation should become
//! submit-and-pollable — is decided by MEASUREMENT, not argument: how
//! much of a caller's stall is *queue wait* (the job admitted but not
//! yet dequeued, behind earlier work) versus *service time* (the
//! worker actually executing the job against the provider)? If the
//! transition-frame stall is service-bound (fsync/SQLite-bound), a
//! deferred confirmation moves almost nothing off the caller thread
//! and the blocking path stays; if it is queue-wait-bound (admission
//! behind publications), the deferral reuses the proven publication
//! machinery. This class makes the two quantities separately visible
//! per job kind so the rig's release-mode data point (§4 workload 4,
//! feeding the §9.5/§9.7 gate curves) can decide.
//!
//! **The class discipline (the [`crate::pipeline::RetentionStats`]
//! precedent):** these counters are read-only derived observability —
//! NEVER serialized (no encoder exists or may exist for them), NEVER
//! a fingerprint or digest input, and session-scoped: the cell lives
//! exactly as long as one worker session, and a fresh worker opened
//! after recovery resumes at defaults (zeros — restore never replays
//! timings). Nothing in the persistence formats, the journal frames,
//! the chain bytes, or any test digest reads a value from this
//! module; the zero-perturbation proof is the transport battery's
//! byte-identity test (the worker-mediated store and journal are
//! byte-identical to the direct provider's with the instrumentation
//! live).
//!
//! **The clock and the determinism boundary:** the quantities are
//! durations, and the crate has no deterministic clock — the
//! established timing precedent (`LocalFileMedium`'s
//! `AppendTimings`) already uses `std::time::Instant` (monotonic
//! wall-clock), and so does this class. The boundary this draws:
//! every counter here is OBSERVATIONAL — tests never assert on
//! exact wall-clock-derived values; the discriminating assertions
//! are (a) structural facts (job counts per class — deterministic),
//! (b) arithmetic over CONSTRUCTED `Instant` values (pure
//! arithmetic, no clock read — deterministic), and (c) separation
//! signs under the deterministic fault seams (the park barrier),
//! never absolute magnitudes.
//!
//! **The measured windows:**
//! - *queue wait* — the admit-call instant (taken just before the
//!   bounded `send`) to the worker's dequeue instant. Under a
//!   saturated queue this deliberately includes the bounded-send
//!   admission backpressure, because that is the caller-experienced
//!   wait the deferral question decomposes (the design's §4
//!   "admission-wait"); the label in every diagnostic is
//!   `admission→dequeue`.
//! - *service time* — the dequeue instant to the completion
//!   boundary (the reply deposited, or deliberately dropped by a
//!   fault seam). A job that never completes (a panicked or
//!   hard-exited worker) records NOTHING — an unfinished job has no
//!   service sample; the counters are completed-job statistics and
//!   the recording happens before the reply action, so a caller
//!   that observed the reply always sees the job counted
//!   (read-your-write, no polling in tests).
//!
//! **Bounded memory (the A21 discipline):** fixed per-class
//! counters plus a fixed-capacity recent-sample ring
//! ([`WORKER_STATS_SAMPLE_RING`]) with a lifetime overwrite counter
//! — the ring preserves FIFO order across classes (the interleaving
//! evidence: which job kinds sat in front of which), overwrites
//! oldest-first, and never grows. Per-class means come from the
//! all-time counters; per-class p99s come from the recent ring
//! (labeled as such in the diagnostic).
//!
//! **Placement:** the class lives in engine-persist (the
//! observability-class precedent), while the recording sites live in
//! the slice's worker transport — the only place the admission,
//! dequeue, and completion events exist (the design's §2.3.1 cites
//! `worker.rs`, the engine-slice transport). The class is
//! transport-generic: any single-writer queue discipline can record
//! into it.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The job kinds the transport classifies operations into — the
/// per-kind split the design's measurement names. The mapping from
/// the transport's operation enum to these classes is owned by the
/// transport (it sees its own operation type); the class set is
/// frozen here so the rig's tables stay comparable across runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerOpClass {
    /// `head` round trips.
    Head,
    /// The append family (the revision + content objects, and the
    /// unforgeable admitted-append form) — the provider's
    /// fsync/COMMIT boundary.
    Append,
    /// The read family (record reads, object reads, classification)
    /// — the chain-walk service the confirmation's read-back
    /// performs.
    Read,
    /// Full chain validation walks.
    Validate,
    /// Unknown-outcome reconciliation under the provider's lock.
    Reconcile,
    /// The reclamation/maintenance family (directives, begin/verify
    /// admissions, coordinated observations).
    Reclaim,
    /// Lifecycle checkpoints (the SQLite WAL truncate).
    Checkpoint,
    /// The maintenance FIFO barrier (the no-op settlement probe).
    Barrier,
}

impl WorkerOpClass {
    /// The frozen class order (diagnostics and ring iteration).
    pub const ALL: [WorkerOpClass; WORKER_OP_CLASS_COUNT] = [
        WorkerOpClass::Head,
        WorkerOpClass::Append,
        WorkerOpClass::Read,
        WorkerOpClass::Validate,
        WorkerOpClass::Reconcile,
        WorkerOpClass::Reclaim,
        WorkerOpClass::Checkpoint,
        WorkerOpClass::Barrier,
    ];

    /// The class's stable index (the counter array's position).
    pub fn index(self) -> usize {
        self as usize
    }
}

impl fmt::Display for WorkerOpClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            WorkerOpClass::Head => "head",
            WorkerOpClass::Append => "append",
            WorkerOpClass::Read => "read",
            WorkerOpClass::Validate => "validate",
            WorkerOpClass::Reconcile => "reconcile",
            WorkerOpClass::Reclaim => "reclaim",
            WorkerOpClass::Checkpoint => "checkpoint",
            WorkerOpClass::Barrier => "barrier",
        };
        f.write_str(label)
    }
}

/// The number of [`WorkerOpClass`] variants (the counter array's
/// fixed size).
pub const WORKER_OP_CLASS_COUNT: usize = 8;

/// The bounded recent-sample ring's capacity (the A21 discipline:
/// fixed size, oldest-first overwrite, a lifetime counter for the
/// overwritten). Two orders above the largest per-commit job count
/// at the current scale (a memory-lane confirmation issues tens of
/// jobs); sized so a sustained-load run retains a meaningful recent
/// tail for the per-class p99 without unbounded growth.
pub const WORKER_STATS_SAMPLE_RING: usize = 256;

/// One completed job's recorded split (a ring sample).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerSample {
    /// The job's class.
    pub class: WorkerOpClass,
    /// The admission→dequeue window (nanoseconds; includes the
    /// bounded-send backpressure under saturation — see the module
    /// doc).
    pub queue_wait_nanos: u64,
    /// The dequeue→completion window (nanoseconds).
    pub service_nanos: u64,
}

/// The per-class cumulative counters (completed jobs only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorkerClassStats {
    /// Completed jobs of this class.
    pub jobs: u64,
    /// Cumulative admission→dequeue nanoseconds.
    pub queue_wait_nanos: u64,
    /// Cumulative dequeue→completion nanoseconds.
    pub service_nanos: u64,
}

impl WorkerClassStats {
    /// The all-time mean queue wait (zero for zero jobs — never a
    /// division by zero, never a float).
    pub fn mean_queue_wait_nanos(&self) -> u64 {
        self.queue_wait_nanos / self.jobs.max(1)
    }

    /// The all-time mean service time.
    pub fn mean_service_nanos(&self) -> u64 {
        self.service_nanos / self.jobs.max(1)
    }
}

/// The snapshot a worker session's observers read: per-class
/// cumulative counters, the bounded recent-sample ring (FIFO order,
/// oldest first), and the ring's lifetime overwrite count. Derived
/// read-only state — never serialized, never a fingerprint input;
/// a fresh worker (post-recovery) snapshots all defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerStats {
    /// The per-class cumulative counters, indexed by
    /// [`WorkerOpClass::index`] (iterate
    /// [`WorkerOpClass::ALL`]).
    pub classes: [WorkerClassStats; WORKER_OP_CLASS_COUNT],
    /// The bounded recent-sample ring, oldest first (at most
    /// [`WORKER_STATS_SAMPLE_RING`] entries).
    pub recent: Vec<WorkerSample>,
    /// Lifetime count of ring samples overwritten past the bound
    /// (the A21 `dropped_total` analog — the honest accounting of
    /// what the ring did not retain).
    pub ring_overwritten: u64,
}

impl WorkerStats {
    /// The empty snapshot (a fresh worker's reading).
    pub fn new() -> Self {
        Self {
            classes: std::array::from_fn(|_| WorkerClassStats::default()),
            recent: Vec::new(),
            ring_overwritten: 0,
        }
    }

    /// One class's cumulative counters.
    pub fn class(&self, class: WorkerOpClass) -> &WorkerClassStats {
        &self.classes[class.index()]
    }

    /// The across-classes totals.
    pub fn totals(&self) -> WorkerClassStats {
        let mut totals = WorkerClassStats::default();
        for stats in &self.classes {
            totals.jobs += stats.jobs;
            totals.queue_wait_nanos += stats.queue_wait_nanos;
            totals.service_nanos += stats.service_nanos;
        }
        totals
    }
}

impl Default for WorkerStats {
    fn default() -> Self {
        Self::new()
    }
}

/// The diagnostic form (the rig measurement's data surface): one
/// stable, greppable line per class plus the totals row. Means are
/// all-time (the cumulative counters); p99s are computed from the
/// recent ring only (nearest-rank over the class's retained
/// samples — zero when the ring holds none). Durations print as
/// integer microseconds. The format is frozen: the release rig's
/// §4-workload-4 capture greps `worker-transport/` lines, and any
/// change to it is a rig-facing change recorded per program
/// discipline.
impl fmt::Display for WorkerStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for class in WorkerOpClass::ALL {
            let stats = self.class(class);
            writeln!(
                f,
                "worker-transport/{class} jobs={} wait_total_us={} wait_mean_us={} \
                 wait_p99_us={} service_total_us={} service_mean_us={} service_p99_us={}",
                stats.jobs,
                stats.queue_wait_nanos / 1_000,
                stats.mean_queue_wait_nanos() / 1_000,
                self.wait_p99(class) / 1_000,
                stats.service_nanos / 1_000,
                stats.mean_service_nanos() / 1_000,
                self.service_p99(class) / 1_000,
            )?;
        }
        let totals = self.totals();
        writeln!(
            f,
            "worker-transport/total jobs={} wait_total_us={} wait_mean_us={} \
             service_total_us={} service_mean_us={} ring_overwritten={}",
            totals.jobs,
            totals.queue_wait_nanos / 1_000,
            totals.mean_queue_wait_nanos() / 1_000,
            totals.service_nanos / 1_000,
            totals.mean_service_nanos() / 1_000,
            self.ring_overwritten,
        )
    }
}

impl WorkerStats {
    /// The nearest-rank p99 of one class's RECENT ring samples'
    /// queue waits (zero when the ring holds none for the class).
    fn wait_p99(&self, class: WorkerOpClass) -> u64 {
        self.ring_percentile_of(class, 99, true).unwrap_or(0)
    }

    /// The nearest-rank p99 of one class's RECENT ring samples'
    /// service times (zero when the ring holds none for the class).
    fn service_p99(&self, class: WorkerOpClass) -> u64 {
        self.ring_percentile_of(class, 99, false).unwrap_or(0)
    }

    /// The nearest-rank percentile of one class's recent ring
    /// samples (the queue-wait side when `queue_wait` is true, the
    /// service side otherwise). Nearest-rank keeps the arithmetic
    /// integral — no floats in a frozen diagnostic.
    fn ring_percentile_of(
        &self,
        class: WorkerOpClass,
        percent: u64,
        queue_wait: bool,
    ) -> Option<u64> {
        let mut values: Vec<u64> = self
            .recent
            .iter()
            .filter(|sample| sample.class == class)
            .map(|sample| {
                if queue_wait {
                    sample.queue_wait_nanos
                } else {
                    sample.service_nanos
                }
            })
            .collect();
        values.sort_unstable();
        let n = values.len() as u64;
        if n == 0 {
            return None;
        }
        let ordinal = (n * percent).div_ceil(100).clamp(1, n);
        values.get(ordinal as usize - 1).copied()
    }
}

/// The shared stats cell: one per worker session, cloned into every
/// admission point (the handle, the source, the proxies, each job).
/// Concurrency is single-writer/many-reader in practice — the ONE
/// worker thread is the only recorder (once per completed job,
/// before the reply action), observers snapshot rarely — so a mutex
/// is the whole story: the recorder's critical section is three
/// integer adds and a bounded ring push, orders of magnitude below
/// any job's provider I/O.
#[derive(Clone, Default)]
pub struct WorkerStatsCell {
    state: Arc<Mutex<WorkerStatsState>>,
}

#[derive(Default)]
struct WorkerStatsState {
    classes: [WorkerClassStats; WORKER_OP_CLASS_COUNT],
    ring: std::collections::VecDeque<WorkerSample>,
    overwritten: u64,
}

impl WorkerStatsState {
    fn new() -> Self {
        Self {
            classes: std::array::from_fn(|_| WorkerClassStats::default()),
            ring: std::collections::VecDeque::new(),
            overwritten: 0,
        }
    }
}

impl WorkerStatsCell {
    /// A fresh cell (all counters at defaults — the state a
    /// post-recovery worker resumes at).
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(WorkerStatsState::new())),
        }
    }

    /// Records one COMPLETED job's split: the queue wait is
    /// `dequeued_at − admitted_at` (saturating — monotonic clocks
    /// can be read cross-thread, never assumed strictly ordered
    /// past zero), the service time is `dequeued_at → now` (the
    /// call sits immediately before the job's reply action, so the
    /// window is the honest dequeue→completion). Called by the
    /// worker thread only, once per job, on every completion
    /// boundary (reply sent or deliberately dropped); the panic and
    /// hard-exit paths record nothing (an unfinished job has no
    /// sample — see the module doc).
    pub fn record(&self, class: WorkerOpClass, admitted_at: Instant, dequeued_at: Instant) {
        let queue_wait = elapsed_nanos_between(admitted_at, dequeued_at);
        let service = elapsed_nanos_since(dequeued_at);
        let mut state = self
            .state
            .lock()
            .expect("the worker stats cell lock (single writer, rare readers)");
        let stats = &mut state.classes[class.index()];
        stats.jobs += 1;
        stats.queue_wait_nanos = stats.queue_wait_nanos.saturating_add(queue_wait);
        stats.service_nanos = stats.service_nanos.saturating_add(service);
        state.ring.push_back(WorkerSample {
            class,
            queue_wait_nanos: queue_wait,
            service_nanos: service,
        });
        while state.ring.len() > WORKER_STATS_SAMPLE_RING {
            state.ring.pop_front();
            state.overwritten += 1;
        }
    }

    /// The observer's read: a consistent copy of the counters, the
    /// bounded ring (oldest first), and the overwrite count.
    pub fn snapshot(&self) -> WorkerStats {
        let state = self
            .state
            .lock()
            .expect("the worker stats cell lock (single writer, rare readers)");
        WorkerStats {
            classes: state.classes,
            recent: state.ring.iter().copied().collect(),
            ring_overwritten: state.overwritten,
        }
    }
}

/// The `local.rs` timing precedent's nanosecond conversion
/// (saturating — a duration can exceed u64 only past ~584 years).
fn elapsed_nanos_between(earlier: Instant, later: Instant) -> u64 {
    u64::try_from(later.saturating_duration_since(earlier).as_nanos()).unwrap_or(u64::MAX)
}

/// `Instant::elapsed` in nanoseconds (the same saturating
/// conversion).
fn elapsed_nanos_since(from: Instant) -> u64 {
    u64::try_from(from.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A fresh cell snapshots all defaults — the restore-resumes-at-
    /// defaults discipline (a post-recovery worker's counters start
    /// at zero; timings are never replayed across sessions), and the
    /// fresh diagnostic is the stable format the rig greps.
    #[test]
    fn a_fresh_cell_snapshots_defaults_and_prints_the_frozen_diagnostic() {
        let cell = WorkerStatsCell::new();
        let stats = cell.snapshot();
        assert_eq!(stats.totals().jobs, 0);
        assert_eq!(stats.recent.len(), 0);
        assert_eq!(stats.ring_overwritten, 0);
        for class in WorkerOpClass::ALL {
            assert_eq!(stats.class(class).jobs, 0);
            assert_eq!(stats.class(class).queue_wait_nanos, 0);
            assert_eq!(stats.class(class).service_nanos, 0);
        }
        // The frozen diagnostic: every class row at zero, the totals
        // row, and the overwrite count. The exact string is pinned
        // (a rig-facing format change must be deliberate).
        let expected = "\
worker-transport/head jobs=0 wait_total_us=0 wait_mean_us=0 wait_p99_us=0 service_total_us=0 service_mean_us=0 service_p99_us=0
worker-transport/append jobs=0 wait_total_us=0 wait_mean_us=0 wait_p99_us=0 service_total_us=0 service_mean_us=0 service_p99_us=0
worker-transport/read jobs=0 wait_total_us=0 wait_mean_us=0 wait_p99_us=0 service_total_us=0 service_mean_us=0 service_p99_us=0
worker-transport/validate jobs=0 wait_total_us=0 wait_mean_us=0 wait_p99_us=0 service_total_us=0 service_mean_us=0 service_p99_us=0
worker-transport/reconcile jobs=0 wait_total_us=0 wait_mean_us=0 wait_p99_us=0 service_total_us=0 service_mean_us=0 service_p99_us=0
worker-transport/reclaim jobs=0 wait_total_us=0 wait_mean_us=0 wait_p99_us=0 service_total_us=0 service_mean_us=0 service_p99_us=0
worker-transport/checkpoint jobs=0 wait_total_us=0 wait_mean_us=0 wait_p99_us=0 service_total_us=0 service_mean_us=0 service_p99_us=0
worker-transport/barrier jobs=0 wait_total_us=0 wait_mean_us=0 wait_p99_us=0 service_total_us=0 service_mean_us=0 service_p99_us=0
worker-transport/total jobs=0 wait_total_us=0 wait_mean_us=0 service_total_us=0 service_mean_us=0 ring_overwritten=0
";
        assert_eq!(format!("{}", cell.snapshot()), expected);
    }

    /// Recording separates the two windows: constructed `Instant`
    /// arithmetic is pure (no clock read between construction and
    /// subtraction), so the queue wait is EXACTLY the constructed
    /// span; the service side is a real elapsed over a constructed
    /// PAST instant, which the monotonic clock bounds below by the
    /// constructed gap — the determinism boundary in action:
    /// exactness and lower bounds only where no clock read can
    /// shrink them, never upper-bound assertions.
    #[test]
    fn recording_separates_queue_wait_from_service_time() {
        let cell = WorkerStatsCell::new();
        let now = Instant::now();
        let admitted = now - Duration::from_millis(10);
        let dequeued = now - Duration::from_millis(5);
        cell.record(WorkerOpClass::Append, admitted, dequeued);
        let stats = cell.snapshot();
        let append = stats.class(WorkerOpClass::Append);
        assert_eq!(append.jobs, 1);
        // The queue wait is pure arithmetic: exactly the constructed
        // admission→dequeue span.
        assert_eq!(append.queue_wait_nanos, 5_000_000);
        // The service time is the real elapsed since the constructed
        // dequeue instant — at least the constructed 5 ms gap
        // (monotonic), with no upper bound asserted.
        assert!(append.service_nanos >= 5_000_000);
        assert_eq!(stats.recent.len(), 1);
        assert_eq!(stats.recent[0].class, WorkerOpClass::Append);
        assert_eq!(stats.recent[0].queue_wait_nanos, 5_000_000);
        assert_eq!(stats.recent[0].service_nanos, append.service_nanos);
        // Every other class stays untouched.
        for class in WorkerOpClass::ALL {
            if class != WorkerOpClass::Append {
                assert_eq!(stats.class(class).jobs, 0);
            }
        }
    }

    /// The ring is bounded and honest about overwrites: 300 samples
    /// against a 256-slot ring retain the NEWEST 256 (identifiable
    /// by their constructed waits) and count 44 overwritten.
    #[test]
    fn the_ring_is_bounded_counts_overwrites_and_keeps_the_newest() {
        let cell = WorkerStatsCell::new();
        let base = Instant::now();
        for i in 0..300u64 {
            let admitted = base;
            let dequeued = base + Duration::from_nanos(i);
            cell.record(WorkerOpClass::Read, admitted, dequeued);
        }
        let stats = cell.snapshot();
        assert_eq!(stats.ring_overwritten, 44);
        assert_eq!(stats.recent.len(), WORKER_STATS_SAMPLE_RING);
        // The oldest retained sample is #44 (0-based), the newest is
        // #299 — constructed arithmetic makes both exact.
        assert_eq!(stats.recent[0].queue_wait_nanos, 44);
        assert_eq!(
            stats.recent[WORKER_STATS_SAMPLE_RING - 1].queue_wait_nanos,
            299
        );
        // The cumulative counters are NOT truncated by the ring.
        let read = stats.class(WorkerOpClass::Read);
        assert_eq!(read.jobs, 300);
        let total_wait: u64 = (0..300).sum();
        assert_eq!(read.queue_wait_nanos, total_wait);
        assert_eq!(read.mean_queue_wait_nanos(), total_wait / 300);
    }

    /// The totals aggregate across classes and the diagnostic's
    /// nearest-rank p99 reads the ring (100 constructed samples with
    /// waits 1..=100 ms → mean 50.5 ms, p99 99 ms, printed in
    /// microseconds).
    #[test]
    fn totals_aggregate_and_the_diagnostic_percentile_is_nearest_rank() {
        let cell = WorkerStatsCell::new();
        let base = Instant::now();
        for i in 1..=100u64 {
            let dequeued = base + Duration::from_millis(i);
            cell.record(WorkerOpClass::Validate, base, dequeued);
        }
        cell.record(WorkerOpClass::Barrier, base, base);
        let stats = cell.snapshot();
        assert_eq!(stats.totals().jobs, 101);
        let validate = stats.class(WorkerOpClass::Validate);
        assert_eq!(validate.jobs, 100);
        assert_eq!(validate.mean_queue_wait_nanos(), 50_500_000);
        // Nearest rank over 100 samples at p99 → the 99th value.
        assert_eq!(stats.wait_p99(WorkerOpClass::Validate), 99_000_000);
        // The barrier has ONE sample: every percentile is that
        // sample; a class with no samples has none.
        assert_eq!(stats.wait_p99(WorkerOpClass::Barrier), 0);
        assert_eq!(
            stats.ring_percentile_of(WorkerOpClass::Head, 99, true),
            None
        );
        let printed = format!("{stats}");
        assert!(printed.contains(
            "worker-transport/validate jobs=100 wait_total_us=5050000 wait_mean_us=50500 \
             wait_p99_us=99000"
        ));
        assert!(printed.contains("worker-transport/total jobs=101"));
    }

    /// Clones share one cell (the admission points, the handle, and
    /// the worker thread all record into the same counters).
    #[test]
    fn clones_share_the_one_cell() {
        let cell = WorkerStatsCell::new();
        let shared = cell.clone();
        let base = Instant::now();
        cell.record(WorkerOpClass::Head, base, base);
        let stats = shared.snapshot();
        assert_eq!(stats.class(WorkerOpClass::Head).jobs, 1);
        assert_eq!(cell.snapshot().class(WorkerOpClass::Head).jobs, 1);
    }
}
