//! T09 stage-1 allocation census: the with-members queued class
//! transition's allocation cost against a growing unrelated queued
//! population (strict-audit-IV T09; the T09 design §7 stage 1 —
//! "the allocation-harness update measurement").
//!
//! WHAT IS MEASURED: `Spine::suspend_class` on a class that owns
//! exactly 40 queued members (the audit's fixed affected work) while
//! the unrelated foreign queued population grows 1k / 5k / 20k /
//! 100k. The census question: does the O(lane) scan and re-heapify
//! the arm performs over the UNRELATED population allocate at all?
//! The code-level answer (into_vec adopts the heap's buffer in O(1),
//! `Vec::extract_if` compacts in place, `BinaryHeap::from` adopts the
//! Vec — the A08a linear shape's zero-scratch-allocation discipline)
//! says the transition's only allocations are the m held-side
//! structural inserts (`held_index` nodes, the class membership's
//! held set, the held map itself) — the value-carrying relocation the
//! design's tombstone analysis turns on. This binary MEASURES that:
//! the with-members window's allocation events and bytes must be
//! IDENTICAL at every population size — flat in the unrelated
//! population — and the empty-membership early exit (the
//! dormant-region common case) must allocate NOTHING.
//!
//! WHY A SEPARATE BINARY: the counting global allocator is
//! process-wide, so this file is its own integration-test binary
//! carrying EXACTLY ONE test — nothing else competes for the counters
//! (the `spine_allocations.rs` precedent, whose thread-attribution
//! corrective is adopted verbatim: only the measuring thread's
//! allocations count, excluding libtest's main-thread waker activity
//! observed as the 2026-11-23 CI flake).
//!
//! THE PINS ARE NOT TOUCHED: `spine_allocations.rs`'s (18, 6880) /
//! (9, 3024) scenario never makes a class-keyed call, so it stays
//! byte-identical under this addition (zero production change — this
//! lane adds measurements only). The numbers printed here are
//! measured actuals, pod-observational; the ASSERTIONS are the
//! structural invariants only (cross-size equality, double-run
//! equality, the empty arm's zero) — deterministic engine code makes
//! them exact, never threshold-fuzzy.
//!
//! STAGE CONTEXT: this is the stage-1 census, the baseline the
//! stage-2 indexed lane (the T09 design's dual-representation
//! `LaneQueue`) would extend with the positions map's structural
//! growth — measured BESIDE these numbers, never replacing them.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use engine_core::{
    Event, EventClass, EventSpec, ExecCtx, Field, Fingerprinter, Schedule, Spine, Tick, World,
};

/// Process-wide allocation-event and allocation-byte counters.
static ALLOCATION_COUNT: AtomicUsize = AtomicUsize::new(0);
static ALLOCATION_BYTES: AtomicUsize = AtomicUsize::new(0);

// The measuring thread's window flag (the `spine_allocations.rs`
// thread-attribution corrective, adopted verbatim): const-init
// thread-local, allocation-free to read, no destructor — the hook
// attributes counted events to the engine's executing thread and
// excludes foreign-thread runtime activity.
thread_local! {
    static IN_WINDOW: Cell<bool> = const { Cell::new(false) };
}

/// Counts one allocation event of `size` bytes IF the allocating
/// thread is inside a measured window (thread attribution).
fn count_measured(size: usize) {
    IN_WINDOW.with(|measured| {
        if measured.get() {
            ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOCATION_BYTES.fetch_add(size, Ordering::Relaxed);
        }
    });
}

/// Forwarding allocator that only counts allocation events and the
/// requested byte sizes (the `spine_allocations.rs` precedent).
///
/// SAFETY EXCEPTION (the unsafe-with-proofs policy): confined to this
/// test target's allocator implementation below, so the rest of the
/// file keeps the workspace's ordinary `unsafe_code = "deny"`
/// restriction.
struct CountingAllocator;

#[allow(unsafe_code)]
// SAFETY: every method forwards its arguments unchanged to the system
// allocator (which satisfies the global allocator contract), adding
// only relaxed atomic increments that cannot reenter allocation.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_measured(layout.size());
        // SAFETY: forwarded unchanged to the system allocator.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count_measured(layout.size());
        // SAFETY: forwarded unchanged to the system allocator.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count_measured(new_size);
        // SAFETY: forwarded unchanged to the system allocator.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarded unchanged to the system allocator.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// The fixture's target class (a foreign tag like any other; the
/// foreign population uses classes 0–5).
const TARGET_CLASS: EventClass = EventClass::new(7);

/// The absent class the "materialize without moving anything" call
/// names (no fixture schedules class `u16::MAX`).
const ABSENT_CLASS: EventClass = EventClass::new(u16::MAX);

/// The fixed affected work at every size — the audit's battery fixes
/// m = 40 (the CA-08 fixture used the same).
const OWN: usize = 40;

/// The audit's exact slope sizes: 1k / 5k / 20k / 100k.
const SIZES: [usize; 4] = [1_000, 5_000, 20_000, 100_000];

/// A minimal no-op world: the census never executes an event (no tick
/// ever runs — the transition measured is the BETWEEN-TICKS class-keyed
/// move, exactly where the region director runs it), and the
/// fingerprint is a constant tag.
struct CensusWorld;

impl World for CensusWorld {
    fn execute(&mut self, _event: &Event, _ctx: &mut ExecCtx) {}

    fn fingerprint(&self, fp: &mut Fingerprinter) {
        fp.write_str("engine-core/spine/t09-queued-census/1");
    }
}

/// A logical-time spec with an explicit class, lane, and payload —
/// the suspension battery's fixture shape.
fn spec(class: u16, field: Field, due: u64, payload: u64) -> EventSpec {
    EventSpec::new(
        EventClass::new(class),
        Schedule::AtLogicalTime { due: Tick(due) },
        payload,
    )
    .in_field(field)
}

/// Builds the T09 stage-1 queued fixture (the spine.rs battery's
/// both-lanes variant — the CA-08 alternation): the target owns
/// EXACTLY `OWN` queued obligations split across both lanes, the
/// unrelated population grows with `total`, every due far-future, no
/// tick ever driven. The index is materialized WITHOUT moving the
/// target (the absent-class suspend) so no measured window contains
/// the one-time adoption build.
fn build_fixture(total: usize) -> Spine<CensusWorld> {
    let mut spine = Spine::new(CensusWorld, 100_000);
    for i in 0..OWN {
        spine
            .schedule(spec(
                TARGET_CLASS.0,
                if i.is_multiple_of(2) {
                    Field::Near
                } else {
                    Field::Far
                },
                500 + u64::try_from(i).expect("due fits u64") * 3,
                u64::try_from(i).expect("payload fits u64"),
            ))
            .expect("fixture scheduling is valid");
    }
    for i in 0..total - OWN {
        spine
            .schedule(spec(
                u16::try_from(i % 6).expect("class fits u16"),
                if i.is_multiple_of(2) {
                    Field::Near
                } else {
                    Field::Far
                },
                500 + u64::try_from(i % 97).expect("due fits u64") * 3,
                100_000 + u64::try_from(i).expect("payload fits u64"),
            ))
            .expect("fixture scheduling is valid");
    }
    spine.suspend_class(ABSENT_CLASS);
    spine
}

/// Opens one measured window on THIS thread, runs `f`, and returns
/// `(allocation events, allocation bytes, f's result)` — the
/// thread-attribution discipline (see the module docs).
fn measured_window<R>(f: impl FnOnce() -> R) -> (usize, usize, R) {
    ALLOCATION_COUNT.store(0, Ordering::Relaxed);
    ALLOCATION_BYTES.store(0, Ordering::Relaxed);
    let result = IN_WINDOW.with(|window| {
        window.set(true);
        let result = f();
        window.set(false);
        result
    });
    (
        ALLOCATION_COUNT.load(Ordering::Relaxed),
        ALLOCATION_BYTES.load(Ordering::Relaxed),
        result,
    )
}

/// THE ALLOCATION CENSUS (the T09 stage-1 gate): the with-members
/// queued class transition allocates only for its OWN members — the
/// m held-side structural inserts — and NEVER for the unrelated
/// population it scans. The O(lane) scan and re-heapify are
/// allocation-free (into_vec adopts the buffer, extract_if compacts
/// in place, from adopts the Vec), so the window's cost is a BOUNDED
/// O(m) at every population size. The exact event/byte count varies
/// by a few nodes with the B-tree split pattern, which follows the
/// extraction order — the lane buffer's order, which legitimately
/// differs per population and per cycle (heap layout is
/// rebuild-dependent and invisible); the census therefore pins the
/// O(m) BOUND (an N-proportional allocation at 100k would exceed it
/// by orders of magnitude) rather than a false exact equality, and
/// proves determinism the honest way: a second FRESH fixture at the
/// same population must reproduce the first-transition window
/// exactly (identical construction, identical sequence — the
/// repo-wide double-run discipline). The empty-membership early exit
/// is pinned to zero.
#[test]
fn with_members_queued_transition_allocations_are_flat_in_unrelated_population() {
    // The O(m) bound: at most 8 allocation events and 1 KiB per
    // member (the three held-side structures' amortized node costs —
    // the measured band is 15–18 events / ~7–8.5 KiB for m = 40,
    // varying only with the B-tree split order) plus nothing that
    // grows with the unrelated population.
    let events_bound = 8 * OWN;
    let bytes_bound = 1_024 * OWN;
    let mut first_at_20k: Option<(usize, usize)> = None;
    for &total in SIZES.iter() {
        let mut spine = build_fixture(total);
        // Window 1 — the census target: the with-members suspend on a
        // fresh spine (every held-side structure at its empty root).
        let (events, bytes, moved) = measured_window(|| spine.suspend_class(TARGET_CLASS));
        assert_eq!(
            moved, OWN as u32,
            "the transition moves the whole membership"
        );
        assert!(
            events <= events_bound && bytes <= bytes_bound,
            "the with-members transition allocates only for its own members (measured \
             {events} events / {bytes} bytes at population {total} against the O(m) bound \
             {events_bound} / {bytes_bound})"
        );
        // Restore the queued state outside every window (the cycle
        // leg the spine.rs slope battery times — allocation-free
        // ground for the resume arm, CA-08's measured flat side).
        let resumed = spine.resume_class(TARGET_CLASS);
        assert_eq!(resumed, OWN as u32);
        // Window 2 — the steady-state repeat: the same transition on
        // the cycled spine (the extraction order changed with the
        // re-push, so the split pattern may differ — reported, never
        // conflated with the first).
        let (steady_events, steady_bytes, moved_again) =
            measured_window(|| spine.suspend_class(TARGET_CLASS));
        assert_eq!(moved_again, OWN as u32);
        assert!(
            steady_events <= events_bound && steady_bytes <= bytes_bound,
            "the steady-state with-members transition allocates only for its own members \
             (measured {steady_events} events / {steady_bytes} bytes at population {total})"
        );
        // Window 3 — the empty-membership arm: the dormant-region
        // early exit returns before any buffer work, so it must
        // allocate NOTHING (the CA-08 ~200 ns flat measurement's
        // allocation-side witness).
        let (empty_events, empty_bytes, moved_empty) =
            measured_window(|| spine.suspend_class(ABSENT_CLASS));
        assert_eq!(moved_empty, 0);
        assert_eq!(
            (empty_events, empty_bytes),
            (0, 0),
            "the empty-membership early exit allocates nothing (population {total})"
        );
        println!(
            "t09 allocation census (population {total:6}, target owns {OWN}): with-members \
             suspend {events} events / {bytes} bytes first, {steady_events} / {steady_bytes} \
             steady-state (the O(m) band — the split order varies with the extraction order); \
             empty-membership arm 0 / 0 — measured actuals, pod-observational"
        );
        if total == SIZES[2] {
            first_at_20k = Some((events, bytes));
        }
    }
    // THE DOUBLE-RUN DETERMINISM PROOF (the repo-wide discipline: the
    // allocation sequence itself must be deterministic, or every
    // number above is meaningless): a second fresh fixture at the 20k
    // population must reproduce the first-transition window exactly.
    let mut twin = build_fixture(SIZES[2]);
    let (twin_events, twin_bytes, twin_moved) =
        measured_window(|| twin.suspend_class(TARGET_CLASS));
    assert_eq!(
        twin_moved, OWN as u32,
        "the twin transition moves the whole membership"
    );
    assert_eq!(
        (twin_events, twin_bytes),
        first_at_20k.expect("the 20k population was measured"),
        "the fresh-fixture first transition is deterministic (double-run at {})",
        SIZES[2]
    );
}
