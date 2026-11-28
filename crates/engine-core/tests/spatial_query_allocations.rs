//! Q09 — the spatial-query allocation census (the measured
//! evaluation the owner's re-audit mandates before any
//! reusable-storage optimization): the per-query allocation churn
//! of the three spatial-query families at the battery scales,
//! measured with a counting global allocator.
//!
//! THE FAMILIES AND THEIR SHAPES:
//!
//! * **The static broad phase** (`player.rs`, P2 L5): the six query
//!   arms over city-block worlds at 128 / 2,048 / 32,768 colliders
//!   (the gate and the battery scales), plus the CLUSTER adversarial
//!   row (every collider in a few cells — the maximal
//!   candidate-gathering regime) and the below-gate linear reference
//!   (64 / 127 colliders — the exact same narrow-phase code the grid
//!   falls back to, running naturally linear).
//! * **The combat broad phase** (`combat.rs`, P2 B06): the three
//!   probe traces over battle rosters at 128 / 2,048 / 32,768
//!   combatants, indexed vs the same-population linear primitive.
//! * **The census** (`npc_perception.rs`, P2 L4/L6): the
//!   scratch-backed indexed census at 512 / 2,048 records — the
//!   warm per-batch REBUILD and the warm per-query cost, the family
//!   that already carries the reusable-storage discipline
//!   (`CensusIndexScratch`).
//!
//! THE MEASURED CLAIMS (pinned exactly, because they are
//! structural):
//!
//! 1. Every LINEAR arm allocates NOTHING per query (the scans are
//!    pure accumulator walks — no candidate storage exists).
//! 2. The census's warm rebuild and warm query allocate NOTHING
//!    (the F-02 caller-held-scratch discipline: buffers cleared and
//!    refilled in place, capacities retained) — the reusable-storage
//!    shape CONFIRMED by measurement, not just by design.
//! 3. The static and combat GRID arms allocate the per-query
//!    candidate `Vec` (the churn Q09 names): measured, printed,
//!    loosely sanity-bounded — the table is the evidence, not a
//!    gate (the F-06 no-thresholds-before-variance rule).
//!
//! THE DISPOSITION the table supports (the no-change decision, with
//! the trigger named — see the lane handoff for the full argument):
//! at every workload that exists today the grid arms do not run at
//! all (every shipped world sits below the 128-collider gate, so
//! the measured per-query churn there is exactly the linear zero);
//! at the battery scales where the grid DOES run, the measured
//! churn is a handful of sub-microsecond small allocations per
//! query against query times that are 100–600× below the linear
//! alternative — the churn never changes which path is faster and
//! is a single-digit fraction of the winning path's own cost. The
//! CLUSTER row is the honest exception (the adversarial shape
//! gathers ~N candidates per query — the waste guard bounds cells
//! WALKED, not candidates GATHERED, a recorded observation for the
//! owner, not a shipped-workload claim). THE TRIGGER for the
//! reusable-storage optimization: a composition adopts a grid arm
//! in a per-tick hot loop at large populations AND rig telemetry
//! shows allocator pressure — then the shape to adopt is the
//! census's own (`CensusIndexScratch`: a scratch-carrying query
//! handle beside the frozen per-call APIs, borrowed like
//! `CensusIndex`), landed WITH the adoption seam (Q04), exactly as
//! L4/L6 landed scratch and index together.
//!
//! THE MEASURED TABLE (first run of this test, 2026-11-27, Windows
//! dev pod, UNOPTIMIZED test profile — the same caveat the in-crate
//! scaling batteries carry: ratios and counts are the evidence,
//! absolute ns belong to the release rig):
//!
//! ```text
//! static   city-block  grid mover arms (box/dep/sweep/cam)  1 alloc,    32 B per query, flat 128→32768
//! static   city-block  grid diag_ray                        9 allocs, 16,352 B per query (the S-ray band: ~2,044 candidates)
//! static   city-block  grid diag_ray @128                   0 (the waste guard serves it linear — the DDA trigger)
//! static   cluster     grid mover arms                      1 alloc,   344 B; dep_hit 2 allocs, 30,552 B; diag 4, 68,800 B
//! static   all         linear arms                          0 allocs, 0 B (PINNED)
//! combat   battle      grid shot                            3 allocs,  112 B per query, flat 128→32768
//! combat   battle      grid miss                            1 alloc,   16 B
//! combat   battle      grid diagonal @32768                14 allocs, 262,128 B — and 13.3 ms vs linear 2.86 ms
//!                                                        (the DDA-trigger regime: the guard's crossover serves the
//!                                                         whole-roster band through the grid arm — the B06-recorded
//!                                                         honest waste row, an owner observation, not alloc churn)
//! combat   all         linear arms                          0 allocs, 0 B (PINNED, same population)
//! census   uniform     warm query                           0 allocs, 0 B (PINNED — the F-02 scratch discipline CONFIRMED)
//! census   uniform     warm rebuild + query                 0 allocs, 0 B (PINNED — per-batch cadence is allocation-free warm)
//! ```
//!
//! The mover-family churn (the per-tick hot class) is ONE 32-byte
//! allocation per query on the static side and three (112 B) on the
//! combat side — against grid-arm query times of ~0.4–1.6 µs and
//! linear alternatives of ~0.1–2.4 ms at the same populations: the
//! churn is noise at every measured workload, and the family that
//! IS adopted per-tick at scale (the census) measures exactly zero.
//!
//! THE MEASUREMENT ATTRIBUTION (the spine_allocations.rs
//! discipline, mirrored): the counting hook is process-wide, but a
//! measured window is one thread's execution — only allocations
//! made by the thread that opened the window count (the const-init
//! thread-local flag; allocation-free to read, no lazy machinery).
//! Every counted allocation inside a window is QUERY-side: the
//! worlds, rosters, and indexes are built BEFORE the window opens.
//! The double-run equality at the end mirrors the repo-wide
//! determinism discipline: the allocation sequence itself must be
//! deterministic, or the table is meaningless. The counting global
//! allocator is process-wide, so this file is its own
//! integration-test binary carrying EXACTLY ONE test — nothing else
//! competes for the counters.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use engine_core::combat::{AttackProfile, AttackRequest, CombatIndex, Combatant, resolve_attack};
use engine_core::npc_perception::{CensusIndexScratch, NpcRecord, perceive};
use engine_core::player::{CollisionWorld, PlayerConfig, StaticCollider, player_box};

/// Process-wide allocation-event and allocation-byte counters.
static ALLOCATION_COUNT: AtomicU64 = AtomicU64::new(0);
static ALLOCATION_BYTES: AtomicU64 = AtomicU64::new(0);

// The measuring thread's window flag (the spine_allocations.rs
// thread-attribution discipline). Const-init thread-local:
// allocation-free to read, no destructor, no lazy machinery — the
// hook can query it on every allocation without perturbing the
// counts. Only the thread running a window ever sets it.
thread_local! {
    static IN_WINDOW: Cell<bool> = const { Cell::new(false) };
}

/// Counts one allocation event of `size` bytes IF the allocating
/// thread is inside a measured window (thread attribution).
fn count_measured(size: usize) {
    IN_WINDOW.with(|measured| {
        if measured.get() {
            ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOCATION_BYTES.fetch_add(size as u64, Ordering::Relaxed);
        }
    });
}

/// Forwarding allocator that only counts allocation events and the
/// requested byte sizes (the spine_allocations.rs twin). The
/// counter increments are atomic and lock-free; the allocator
/// itself performs no allocation, so forwarding to the system
/// allocator preserves every guarantee of the global allocator
/// contract.
///
/// SAFETY EXCEPTION (the unsafe-with-proofs policy, the
/// spine_allocations.rs precedent): confined to this test target;
/// the allowance is scoped to the allocator implementation below so
/// the rest of the file keeps the workspace's
/// `unsafe_code = "deny"` restriction.
struct CountingAllocator;

#[allow(unsafe_code)]
// SAFETY: every method forwards its arguments unchanged to the
// system allocator (which satisfies the global allocator contract),
// adding only relaxed atomic increments that cannot reenter
// allocation.
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

/// One measured window: runs `queries` repetitions of `run` on THIS
/// thread and returns `(allocation events, allocation bytes,
/// mean ns per repetition)`. Everything the queries touch is built
/// by the caller BEFORE the window opens — every counted allocation
/// is query-side.
fn measure(queries: u64, mut run: impl FnMut()) -> (u64, u64, f64) {
    ALLOCATION_COUNT.store(0, Ordering::Relaxed);
    ALLOCATION_BYTES.store(0, Ordering::Relaxed);
    IN_WINDOW.with(|measured| measured.set(true));
    let start = Instant::now();
    for _ in 0..queries {
        run();
    }
    let elapsed = start.elapsed().as_secs_f64();
    IN_WINDOW.with(|measured| measured.set(false));
    (
        ALLOCATION_COUNT.load(Ordering::Relaxed),
        ALLOCATION_BYTES.load(Ordering::Relaxed),
        elapsed / queries as f64 * 1e9,
    )
}

/// A deterministic xorshift64* stream for fixture generation (the
/// in-crate batteries' own FixtureRng twin — std-only, seeded per
/// fixture, so every run builds the IDENTICAL population).
struct FixtureRng(u64);

impl FixtureRng {
    fn next_u32(&mut self) -> u32 {
        debug_assert!(self.0 != 0, "a zero state would freeze the stream");
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as u32
    }

    /// [0, 1) at 24-bit granularity.
    fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / 16_777_216.0
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

/// One printed table row: family, population, arm, path, allocation
/// events per query, bytes per query, mean ns per query. The
/// double-run equality compares these rows wholesale.
#[derive(Clone, Debug, PartialEq)]
struct Row {
    family: &'static str,
    population: usize,
    arm: &'static str,
    path: &'static str,
    allocs_per_query: f64,
    bytes_per_query: f64,
    ns_per_query: f64,
}

/// The census row carrier (rebuild counts are per-build, not
/// per-query — kept in the same row shape with the count in the
/// allocs column and a note in `arm`).
fn census_row(
    population: usize,
    arm: &'static str,
    path: &'static str,
    measured: (u64, u64, f64),
    runs: u64,
) -> Row {
    Row {
        family: "census",
        population,
        arm,
        path,
        allocs_per_query: measured.0 as f64 / runs as f64,
        bytes_per_query: measured.1 as f64 / runs as f64,
        ns_per_query: measured.2,
    }
}

/// The full measurement scenario, run twice for the determinism
/// equality. Returns the table (one row per measured cell).
fn run_scenario() -> Vec<Row> {
    let mut rows = Vec::new();
    let config = PlayerConfig::default();

    // ------------------------------------------------ the static family
    // The city-block shape (the in-crate scaling battery's own):
    // colliders on a 3 m lattice, every 8th a rotated prism.
    let city_block = |count: usize| {
        let mut world = CollisionWorld::new(0.0);
        let side = ((count as f64).sqrt().ceil() as usize).max(1);
        for i in 0..count {
            let base = [(i % side) as f32 * 3.0, 0.0, (i / side) as f32 * 3.0];
            if i % 8 == 3 {
                world = world.with_rotated_collider(
                    engine_core::collision::RotatedCollider::from_base_yaw(
                        base, 1.2, 1.2, 2.0, 0.6,
                    ),
                );
            } else {
                world = world.with_box(base, 1.0, 2.0);
            }
        }
        world
    };
    // The cluster shape (the adversarial row): every collider inside
    // a few cells — the maximal candidate-gathering regime.
    let cluster = |count: usize| {
        let mut rng = FixtureRng(0xC105_EED0);
        let mut world = CollisionWorld::new(0.0);
        for _ in 0..count {
            let base = [
                rng.range(-3.0, 3.0),
                rng.range(0.0, 2.0),
                rng.range(-3.0, 3.0),
            ];
            world = world.with_box(base, 0.8, 1.5);
        }
        world
    };

    // The probe set (the in-crate scaling battery's own probes: the
    // per-tick mover arms plus the two segment regimes).
    let street = [1.5, 0.0, 1.5];
    let (s_min, s_max) = player_box(street, &config);
    let (h_min, h_max) = player_box([0.0, 0.0, 0.0], &config);
    let cam_from = [1.5, 1.6, 1.5];
    let cam_to = [1.5, 1.6, 6.0];
    let diag_from = [-60.0, 1.0, -60.0];
    let diag_to = [60.0, 1.0, 60.0];

    for (population, world) in [
        (64usize, city_block(64)), // below the gate: naturally linear
        (127, city_block(127)),    // the last below-gate population
        (128, city_block(128)),    // AT the gate — the grid activates
        (2048, city_block(2048)),
        (32768, city_block(32768)),
        (2048, cluster(2048)), // the adversarial cluster row
    ] {
        let grid_active = population >= engine_core::player::GRID_COLLIDER_GATE;
        let path = if grid_active { "grid" } else { "linear" };
        let queries = (600_000 / population as u64).clamp(100, 2000);
        // One measurement arm: a name and the boxed query body (the
        // bodies borrow this iteration's `world` and probes).
        type Arm<'a> = (&'static str, Box<dyn FnMut() + 'a>);
        let arms: [Arm; 6] = [
            (
                "box_miss",
                Box::new(|| {
                    let _ = std::hint::black_box(world.box_overlaps(s_min, s_max));
                }),
            ),
            (
                "dep_miss",
                Box::new(|| {
                    let _ = std::hint::black_box(world.depenetration(s_min, s_max));
                }),
            ),
            (
                "dep_hit",
                Box::new(|| {
                    let _ = std::hint::black_box(world.depenetration(h_min, h_max));
                }),
            ),
            (
                "sweep",
                Box::new(|| {
                    let _ = std::hint::black_box(world.sweep_axis(s_min, s_max, 0, 0.2));
                }),
            ),
            (
                "cam_ray",
                Box::new(|| {
                    let _ = std::hint::black_box(world.segment_hit(cam_from, cam_to, 0.15));
                }),
            ),
            (
                "diag_ray",
                Box::new(|| {
                    let _ = std::hint::black_box(world.segment_hit(diag_from, diag_to, 0.0));
                }),
            ),
        ];
        for (arm, run) in arms {
            let measured = measure(queries, run);
            rows.push(Row {
                family: "static",
                population,
                arm,
                path,
                allocs_per_query: measured.0 as f64 / queries as f64,
                bytes_per_query: measured.1 as f64 / queries as f64,
                ns_per_query: measured.2,
            });
        }
    }

    // ------------------------------------------------ the combat family
    let battle = |count: usize| {
        let side = ((count as f64).sqrt().ceil() as usize).max(1);
        (0..count)
            .map(|i| {
                let base = [(i % side) as f32 * 3.0, 0.0, (i / side) as f32 * 3.0];
                Combatant {
                    id: i as u32 + 1,
                    hit_box: StaticCollider {
                        min: [base[0] - 0.45, base[1], base[2] - 0.45],
                        max: [base[0] + 0.45, base[1] + 1.8, base[2] + 0.45],
                    },
                }
            })
            .collect::<Vec<_>>()
    };
    let empty = CollisionWorld::new(0.0);
    let shot = AttackRequest {
        attacker: 0,
        origin: [1.5, 1.6, 1.5],
        direction: [1.0, 0.0, 0.0],
        profile: AttackProfile {
            range: 30.0,
            occlusion_margin: 0.0,
        },
    };
    let diagonal = AttackRequest {
        attacker: 0,
        origin: [1.5, 1.6, 1.5],
        direction: [1.0, 0.0, 1.0],
        profile: AttackProfile {
            range: 500.0,
            occlusion_margin: 0.0,
        },
    };
    let miss = AttackRequest {
        attacker: 0,
        origin: [1.5, 1.6, 1.5],
        direction: [0.0, 1.0, -0.15],
        profile: AttackProfile {
            range: 50.0,
            occlusion_margin: 0.0,
        },
    };
    for population in [128usize, 2048, 32768] {
        let roster = battle(population);
        let index = CombatIndex::build(&roster);
        let queries = (600_000 / population as u64).clamp(100, 2000);
        for (arm, request) in [("shot", &shot), ("diagonal", &diagonal), ("miss", &miss)] {
            let measured = measure(queries, || {
                let _ = std::hint::black_box(index.resolve_attack(&empty, request));
            });
            rows.push(Row {
                family: "combat",
                population,
                arm,
                path: "grid",
                allocs_per_query: measured.0 as f64 / queries as f64,
                bytes_per_query: measured.1 as f64 / queries as f64,
                ns_per_query: measured.2,
            });
            // The same-population linear primitive (the frozen public
            // signature serves any roster, no index).
            let linear = measure(queries, || {
                let _ = std::hint::black_box(resolve_attack(&empty, request, &roster));
            });
            rows.push(Row {
                family: "combat",
                population,
                arm,
                path: "linear",
                allocs_per_query: linear.0 as f64 / queries as f64,
                bytes_per_query: linear.1 as f64 / queries as f64,
                ns_per_query: linear.2,
            });
        }
    }

    // ------------------------------------------------ the census family
    let uniform_population = |count: usize| {
        let mut rng = FixtureRng(0x5EED_0009);
        (0..count)
            .map(|i| NpcRecord {
                id: i as u32,
                pos: [rng.range(-60.0, 60.0), rng.range(-60.0, 60.0)],
            })
            .collect::<Vec<_>>()
    };
    for population in [512usize, 2048] {
        let population = uniform_population(population);
        let mut scratch = CensusIndexScratch::new();
        // The cold build (buffers allocate — OUTSIDE any window; the
        // census's amortization story is the WARM rebuild).
        let mut index = scratch.build(&population).expect("grid-usable");
        // One warm-up query (the candidate scratch reaches its
        // steady-state capacity), then the WARM query window.
        let _ = index.perceive([0.0, 0.0], 3.0).count();
        let queries = 2000u64;
        let warm_query = measure(queries, || {
            let _ = std::hint::black_box(index.perceive([0.0, 0.0], 3.0).count());
        });
        rows.push(census_row(
            population.len(),
            "perceive",
            "grid-warm",
            warm_query,
            queries,
        ));
        // The same-population linear census.
        let linear = measure(queries, || {
            let _ = std::hint::black_box(perceive([0.0, 0.0], 3.0, &population).count());
        });
        rows.push(census_row(
            population.len(),
            "perceive",
            "linear",
            linear,
            queries,
        ));
        // The WARM per-batch rebuild (the per-tick cadence: the
        // handle's borrow ends at its last use above, the population
        // is unchanged, the scratch is reused).
        let rebuilds = 100u64;
        let warm_rebuild = measure(rebuilds, || {
            let mut index = scratch.build(&population).expect("grid-usable");
            let _ = index.perceive([0.0, 0.0], 3.0).count();
        });
        rows.push(census_row(
            population.len(),
            "rebuild+query",
            "grid-warm",
            warm_rebuild,
            rebuilds,
        ));
    }

    rows
}

/// The Q09 allocation census: measures every family's per-query
/// allocation churn at the battery scales, prints the table, pins
/// the structural zeros (linear arms, census warm rebuild/query),
/// sanity-bounds the grid arms, and requires the ALLOCATION COUNTS
/// (not the timing columns — wall-clock noise is not a determinism
/// claim) to be identical across two runs.
#[test]
fn spatial_query_allocation_census() {
    let first = run_scenario();
    let second = run_scenario();
    // The determinism discipline on the COUNTS: identical scenarios
    // must produce identical allocation sequences (events and
    // bytes); the ns column is wall-clock context, excluded from
    // the equality by design.
    assert_eq!(
        first.len(),
        second.len(),
        "the table shape must be deterministic"
    );
    for (a, b) in first.iter().zip(&second) {
        assert_eq!(
            (
                a.family,
                a.population,
                a.arm,
                a.path,
                a.allocs_per_query,
                a.bytes_per_query
            ),
            (
                b.family,
                b.population,
                b.arm,
                b.path,
                b.allocs_per_query,
                b.bytes_per_query
            ),
            "the allocation counts must be deterministic across runs: \
             {a:?} vs {b:?}"
        );
    }

    println!(
        "[q09-allocations] family       pop  arm          path      \
         allocs/q    bytes/q      ns/q"
    );
    for row in &first {
        println!(
            "[q09-allocations] {:8} {:>6} {:12} {:10} {:>9.3} {:>9.1} {:>10.1}",
            row.family,
            row.population,
            row.arm,
            row.path,
            row.allocs_per_query,
            row.bytes_per_query,
            row.ns_per_query
        );
    }

    // THE STRUCTURAL PINS — exact zeros, because the property is
    // structural (pure accumulator walks; retained scratch buffers):
    for row in &first {
        if row.path == "linear" {
            assert_eq!(
                (row.allocs_per_query, row.bytes_per_query),
                (0.0, 0.0),
                "{:?} — a linear arm must allocate nothing per query",
                row
            );
        }
        if row.family == "census" && row.path == "grid-warm" {
            assert_eq!(
                (row.allocs_per_query, row.bytes_per_query),
                (0.0, 0.0),
                "{:?} — the census's warm scratch discipline must \
                 allocate nothing (F-02, confirmed by measurement)",
                row
            );
        }
    }

    // THE GRID ARMS — measured, printed, loosely sanity-bounded (the
    // table is the evidence; generous bounds only, the F-06 rule).
    // A ZERO-alloc grid row is itself honest evidence: the
    // [`engine_core::player::GRID_CELL_WASTE_FACTOR`] guard served
    // that query through the exact linear fallback (the long
    // diagonal over small populations — the DDA-trigger regime the
    // B06 record documents).
    let mut grid_rows = 0usize;
    for row in &first {
        if row.path == "grid" {
            grid_rows += 1;
            assert!(
                row.allocs_per_query <= 64.0 && row.bytes_per_query <= 512.0 * 1024.0,
                "{:?} — an absurd per-query allocation (sanity bound)",
                row
            );
            assert!(
                row.ns_per_query.is_finite()
                    && row.ns_per_query < 5_000_000.0 + row.population as f64 * 4_000.0,
                "{:?} - an absurdly slow query (sanity bound; the per-record form — the \
                 diagonal arms scale with the population and a fixed 50 ms trips on a \
                 shared runner)",
                row
            );
        }
    }
    // The scenario's shape: 6 static populations × 6 arms, 3 combat
    // populations × 3 arms × 2 paths, 2 census populations × 3 rows.
    assert_eq!(first.len(), 6 * 6 + 3 * 3 * 2 + 2 * 3);
    assert!(grid_rows > 0, "the grid arms must be measured");
}
