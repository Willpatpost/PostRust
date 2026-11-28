//! F09/Q04 — the indexed-query PRODUCTION ADOPTION BOUNDARY at the
//! combat compositions (audit `runtime.md` F09, verified against
//! source before implementation).
//!
//! # THE FINDING (verified)
//!
//! The engine's indexed query APIs had no production adoption: the
//! live combat paths called the linear resolver directly
//! (`combat_demo.rs`'s fire step; `encounter_frame.rs`'s sight step;
//! the encounter's `try_fire` volley), and the composition layers
//! that own the rosters never ran the admission gates the indexed
//! siblings carry (`CombatIndex::build`'s roster gate;
//! `CensusIndexScratch::build_if_advisable`'s population×queries
//! gate). A microbenchmark alone does not establish frame savings —
//! the audit's demand is the decision AT the composition boundary,
//! with the measured counts on record.
//!
//! # THE ADOPTION MAP (the measured counts, recorded)
//!
//! Every composition boundary this module serves, with the roster
//! size and the query count ONE stable roster revision carries:
//!
//! | Boundary | Roster per revision | Queries per revision | Decision |
//! |---|---|---|---|
//! | `combat_demo` fire (`fire_weapon`) | 1 adversary (the authored single body; the roster is rebuilt every press — the body patrols between presses) | 1 trace (one press edge; the 18-tick cooldown bounds the rate) | **LINEAR** (both halves decline) |
//! | `encounter_demo` fire (`try_fire` volley) | 1–4 adversaries (`adversary_count` 1 scripted / 4 the interactive default; rebuilt every fire) | 1 ray (precision) or 5 rays (the spread volley) | **LINEAR** (the roster half declines today; and the measured crossover says the 5-ray volley sits below the query gate at ANY roster — see the seam record below) |
//! | `encounter_frame` sight (the third-person contact probe) | 1–4 adversaries (rebuilt per changed `SightKey`: tick/weapon/origin/aim) | 1 trace per key change | **LINEAR** (both halves decline) |
//!
//! The rule (the audit's words, implemented): retain the tiny-roster
//! path; build once per stable roster/epoch ONLY when the number of
//! queries pays for the build. [`fire_index_advisable`] is the
//! composition's gate — BOTH halves must clear:
//!
//! * the ROSTER half ([`FIRE_INDEX_ROSTER_GATE`], 2,048) is the
//!   measured-CONFIDENT regime: at the engine's own
//!   [`COMBAT_INDEX_GATE`] edge (a 128-roster) the indexed arm's
//!   per-query advantage on the callers' real query mix is a WASH
//!   (the crossover table's recorded noise-bound row), so the
//!   composition floor sits at the first size where the measurement
//!   is confident — the engine's 128 gate remains the MACHINERY
//!   floor inside `CombatIndex::build` (what builds when built),
//!   layered, not re-litigated;
//! * the QUERY half ([`FIRE_INDEX_QUERIES_GATE`], 32) is the
//!   build-amortization bound measured at THIS seam (the crossover
//!   test below prints the table; the gate is pinned at 2× the
//!   worst confident crossover — the same derivation discipline
//!   that set `CENSUS_INDEX_QUERIES_GATE` = 4 from a measured 2–3).
//!
//! # THE FIRECONTEXT SEAM (the B06 lane's recorded shape)
//!
//! The B06 lane recorded the follow-up seam as "`FireContext` could
//! carry `Option<&CombatIndex>`" (`combat.rs`'s placement decision).
//! This module lands that seam AT THE COMPOSITION — the B06-recorded
//! shape, composition-carried: [`FireRoster`] is one stable roster
//! revision's resolution plan, built by the composition where the
//! roster is built (the `encounter_targets` cadence — roster and
//! index rebuilt together in one scope, so the Q02 borrowed-roster
//! contract holds by construction), carrying the
//! `Option<CombatIndex>` internally and routing
//! [`FireRoster::resolve`] through the index when the gate adopted
//! and the exact linear resolver otherwise.
//!
//! The ENGINE-side half of the seam — `engine_core::weapons::
//! FireContext` gaining the `Option<&CombatIndex>` field so
//! `try_fire`'s per-ray resolves can route through the composition's
//! per-revision handle — is the recorded follow-up (outside this
//! lane's edit scope: engine-core is touched only for the Q09
//! scratch-handle, and that trigger did NOT fire — see below). It is
//! mechanical when needed: the composition already builds the
//! `FireRoster` at the same scope the `FireContext` is built, so the
//! field borrows the handle this module owns. Until then the
//! encounter's volley rides `try_fire`'s linear resolves — honest at
//! every roster that exists today (1–4, below the gate regardless),
//! and honest at scale too: the measured crossover (~16–18 queries
//! at 2,048; ~11–15 at 32,768) says the 5-ray spread volley never
//! pays for a build at ANY roster, so the engine-side extension is
//! not economically prompted until a composition presents
//! gate-many queries against one stable roster (a suppressive sweep
//! or an AI volley burst) — recorded at the call site.
//!
//! # THE Q09 SCRATCH-HANDLE ASSESSMENT (the trigger did not fire)
//!
//! Q09's named trigger for the reusable-storage optimization: "a
//! composition adopts a grid arm in a per-tick hot loop at large
//! populations AND rig telemetry shows allocator pressure." The
//! measured facts at this seam:
//!
//! * NO composition adopts a grid arm today — every boundary in the
//!   adoption map above declines, so every shipped query path is the
//!   linear zero-allocation arm (the audit's retained fast path).
//! * The arms that WOULD run indexed (roster ≥ 2,048 AND queries ≥
//!   the gate) are per-PRESS and per-SIGHT-KEY revisions — user-
//!   triggered edges, not per-tick loops; the per-tick hot family
//!   (the census) already carries its scratch (`CensusIndexScratch`,
//!   Q09's confirmed warm-path zero-alloc).
//! * The grid arm's measured churn when it does run is a handful of
//!   allocation events per query (the log-growth chain of the
//!   candidate gather — measured at this seam: 1 event / 58 B at a
//!   sparse 128-roster ray, 5 events / ~2.4 KB and 6 events /
//!   ~38 KB at the dense 2,048 / 32,768 fixtures, the BYTES scaling
//!   with the candidates gathered — workload, not structure; the
//!   Q09 table's sparser battle-roster rows measured 3 events /
//!   112 B at the shot shape) against per-query index-arm times
//!   that beat the linear walk by ~3–16× in the confident regime —
//!   the churn is noise at every measured workload, a
//!   single-digit-fraction of the winning path's own cost.
//!
//! The allocation test below re-measures the churn AT THIS SEAM and
//! pins the linear zeros; the honest numbers stay on record. No
//! engine-core edit: the `CombatIndexScratch` shape lands WITH an
//! adoption that fires the trigger, exactly as Q04+Q09 planned.
//!
//! # WHAT THE TESTS PROVE (the audit's mandatory comparisons)
//!
//! 1. **Equivalence at the adopted caller** — the seam's own
//!    `resolve` (the exact code `combat_demo`/`encounter_frame` now
//!    call) vs the linear primitive, bit-for-bit through the B06 f32
//!    bit-pattern discipline (`to_bits`, NaN-inclusive), tie
//!    ordering included (an authored equidistant pair), at
//!    increasing sizes and the real callers' request shapes (the
//!    demo's own arena world, weapon ranges, point-blank and
//!    dead-ahead and diagonal traces).
//! 2. **Build amortization** — the crossover measured at the real
//!    caller's shape (the query count that pays for the build),
//!    printed as the pod-observational table the repo's measurement
//!    discipline mandates; the structural half (the build's
//!    allocation cost per revision, the per-query churn, the linear
//!    zeros) is pinned.
//! 3. **The fallback arms** — the dense cluster, the long diagonal
//!    (the B06/Q09 waste row), the poisoned roster, and the
//!    refuse-arms: bit-identity through every arm, with the
//!    per-query allocation count as the arm discriminator (the
//!    waste-guard fallback allocates nothing; the grid arm's churn
//!    is the measured nonzero).

use engine_core::combat::{AttackRequest, CombatIndex, Combatant, HitResolution, resolve_attack};
use engine_core::player::CollisionWorld;

/// The composition's ROSTER gate half: the roster size at which the
/// indexed arm's per-query advantage on the callers' REAL query mix
/// is measured CONFIDENT (the crossover measurement below: at the
/// engine's own [`COMBAT_INDEX_GATE`] edge — a 128-roster — the
/// advantage is a WASH on this pod, linear ~0.016 ms vs indexed
/// ~0.016 ms per query, the crossover noise-bound at ~hundreds to
/// ~thousands of queries across runs; at 2,048 the crossover is a
/// stable ~16–18; at 32,768 ~11–15). The composition boundary
/// therefore adopts only in the confident regime — roster ≥ 2,048 —
/// while the engine's own 128 gate remains the MACHINERY floor
/// inside `CombatIndex::build` (what builds when built, not when to
/// build): the layered decision the B06 design itself recorded
/// ("the composition chooses per workload; both paths remain
/// permanent"). Raising the composition floor above the engine's is
/// the measured no-payback answer at the edge, not a re-litigation
/// of the engine gate.
pub const FIRE_INDEX_ROSTER_GATE: usize = 2048;

/// The composition's query-count gate half (F09's
/// build-amortization bound): the number of resolve queries one
/// STABLE roster revision must carry before building the
/// [`CombatIndex`] for it pays. Pinned from the crossover
/// measurement at this seam (the test below prints the table; first
/// run 2026-11-28, Windows dev pod, UNOPTIMIZED test profile,
/// median statistics — ratios are the evidence): over the callers'
/// REAL query mix (the two weapons' profile ranges, 30 m / 12 m —
/// every live ray is short), the confident crossovers are ~16–18
/// queries at 2,048 and ~11–15 at 32,768 — the gate is pinned at 32
/// with the census lanes' derivation margin (measured 2–3 → gate 4
/// there; measured ~16–18 → gate 32 here, 2× headroom over the
/// worst confident cell). A revision below 32 queries stays on the
/// exact linear resolver at every adoptable roster size — the
/// audit's retained small-workload fast path, now a measured
/// decision rather than an assertion: no composition in this engine
/// presents that many queries against one stable roster (the press
/// trace is 1, the sight probe 1, the spread volley 5 — all recorded
/// in `combat_adoption`'s table). The build's own
/// ~O(occupied-cells) allocation churn is the cost the gate
/// amortizes (the Q09 census: `CombatIndex` carries no reusable
/// scratch — the shape that lands when a real adoption fires the
/// Q09 trigger).
pub const FIRE_INDEX_QUERIES_GATE: u64 = 32;

/// The composition's per-revision adoption gate (both halves, the
/// `census_index_advisable` shape): the roster reaches
/// [`FIRE_INDEX_ROSTER_GATE`] (the measured-confident regime — see
/// that constant's wash-row record) AND the revision's query count
/// reaches [`FIRE_INDEX_QUERIES_GATE`]. Below either, the exact
/// linear resolver is the fast path and
/// [`FireRoster::for_revision`] declines the build.
#[must_use = "a gate that is never checked adopts nothing"]
pub fn fire_index_advisable(roster_len: usize, queries_per_revision: u64) -> bool {
    roster_len >= FIRE_INDEX_ROSTER_GATE && queries_per_revision >= FIRE_INDEX_QUERIES_GATE
}

/// One stable roster revision's fire-resolution plan — the
/// composition-side adoption carrier (the B06-recorded
/// `FireContext` seam shape, composition-carried; see the module
/// docs for the engine-side follow-up).
///
/// Built by the composition where the roster is built (one scope —
/// the Q02 borrowed-roster contract: the handle cannot outlive the
/// roster and cannot serve any other slice, enforced by the
/// borrow). [`resolve`](Self::resolve) routes through the built
/// [`CombatIndex`] when the gate adopted — bit-identical to the
/// linear resolver by the B06 equivalence argument (the candidates
/// are re-sorted to slice order before the UNCHANGED narrow phase
/// and selection) — and through the exact linear
/// [`resolve_attack`] otherwise. A roster with a VALID hitbox
/// outside the cell domain poisons the index
/// (`CombatPhase::Unusable`) and every resolve falls back to the
/// exact linear path PER QUERY — behavior never depends on the
/// broad phase being available.
#[derive(Debug, Clone)]
pub struct FireRoster<'a> {
    /// The revision's roster (the linear arm's population and the
    /// index's borrowed population are the SAME slice).
    targets: &'a [Combatant],
    /// The built index, present only when
    /// [`fire_index_advisable`] cleared both halves for this
    /// revision. `None` below the gates (the exact linear call);
    /// `Some(inert)` cannot occur — the composition's roster gate
    /// (2,048) sits far above the engine's own build gate (128), so
    /// a built index at or above the composition gate actually
    /// carries the grid (or the fail-closed `Unusable` phase for a
    /// poisoned roster).
    index: Option<CombatIndex<'a>>,
}

impl<'a> FireRoster<'a> {
    /// The adoption decision for ONE stable roster revision: the
    /// composition measures the revision's query count (the real
    /// number — one trace for a press or a sight key, the ray count
    /// for a volley) and the gate builds or declines. The build is
    /// per revision: the roster mutates (bodies move, defeats
    /// remove entries), so the handle is rebuilt with it — never
    /// stored across revisions.
    #[must_use]
    pub fn for_revision(targets: &'a [Combatant], queries_per_revision: u64) -> Self {
        let index = fire_index_advisable(targets.len(), queries_per_revision)
            .then(|| CombatIndex::build(targets));
        FireRoster { targets, index }
    }

    /// Whether THIS revision adopted the index (observability for
    /// tests and diagnostics — never a behavior input).
    #[must_use]
    pub fn indexed(&self) -> bool {
        self.index.is_some()
    }

    /// Resolves one attack over this revision's roster — the
    /// IDENTICAL contract to [`resolve_attack`] (validation,
    /// occlusion, selection, ties, refusals); the only difference
    /// is the candidate source when the index was adopted.
    pub fn resolve(&self, world: &CollisionWorld, request: &AttackRequest) -> HitResolution {
        match &self.index {
            Some(index) => index.resolve_attack(world, request),
            None => resolve_attack(world, request, self.targets),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::RotatedCollider;
    use engine_core::combat::{AttackProfile, AttackRefusal, COMBAT_INDEX_GATE, MissKind};
    use engine_core::look_direction;
    use engine_core::player::{GRID_CELL_SIZE, StaticCollider};

    use crate::combat_demo::{ADVERSARY_SPAWN, adversary_body, arena_world};

    /// Forces the adoption ON regardless of the composition gate
    /// (the battery's control — B06's own `force_index_on`
    /// discipline, one level up): the SAME roster through the seam's
    /// index arm must produce bit-identical results even where the
    /// composition boundary declines. The ENGINE's own 128 roster
    /// gate still applies inside `CombatIndex::build` — below it the
    /// forced index is inert and the arm is the exact linear path
    /// (the engine's retained floor, unchanged).
    fn forced_adoption<'a>(targets: &'a [Combatant]) -> FireRoster<'a> {
        FireRoster {
            targets,
            index: Some(CombatIndex::build(targets)),
        }
    }

    // ------------------------------------------------- the fixture twins
    // (the B06 battery's own discipline, mirrored at the composition
    // seam: a deterministic xorshift64* stream, the three roster
    // shapes, the corrupted-entry class, and the f32 bit-pattern
    // comparator — engine-core's test fixtures are private, so the
    // seam carries its own twins building the SAME shapes.)

    /// A deterministic xorshift64* stream for fixture generation
    /// (std-only, seeded per fixture, so every run of the battery
    /// builds the IDENTICAL population).
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

    /// The fixture roster shapes (the B06 battery's set): Spread — a
    /// jittered ±60 m lattice with the cell-boundary and
    /// sparse-outlier classes; Cluster — the whole roster inside a
    /// few cells (the maximal per-cell candidate regime); Ring — an
    /// equidistant boundary-saturated ring (the tie classes).
    #[derive(Clone, Copy, Debug)]
    enum Shape {
        Spread,
        Cluster,
        Ring,
    }

    /// The deterministic fixture roster: human-scale bodies (the
    /// `adversary_body` extent class), distinct ids, and — when
    /// `corrupt` — every 9th entry an INVALID hitbox (alternating
    /// NaN / inverted), the skip rule's class.
    fn fixture_roster(seed: u64, count: usize, shape: Shape, corrupt: bool) -> Vec<Combatant> {
        let mut rng = FixtureRng(seed);
        let mut roster = Vec::with_capacity(count);
        for index in 0..count {
            let base = match shape {
                Shape::Spread => {
                    let lattice = 3.0;
                    let x = (rng.range(-60.0, 60.0) / lattice).round() * lattice;
                    let z = (rng.range(-60.0, 60.0) / lattice).round() * lattice;
                    let y = if index.is_multiple_of(4) {
                        0.0
                    } else {
                        rng.range(0.0, 3.0)
                    };
                    if index % 13 == 5 {
                        [x * 8.0, y, z * 8.0]
                    } else if index.is_multiple_of(7) {
                        // EXACTLY on cell boundaries (the floor-edge
                        // class).
                        [
                            (x / GRID_CELL_SIZE).round() * GRID_CELL_SIZE,
                            y,
                            (z / GRID_CELL_SIZE).round() * GRID_CELL_SIZE,
                        ]
                    } else {
                        [x, y, z]
                    }
                }
                Shape::Cluster => [
                    rng.range(-3.0, 3.0),
                    rng.range(0.0, 2.5),
                    rng.range(-3.0, 3.0),
                ],
                Shape::Ring => {
                    let angle = index as f32 * 0.6180339; // golden-angle spacing
                    let radius = 24.0;
                    [
                        radius * angle.cos(),
                        rng.range(0.0, 2.0),
                        radius * angle.sin(),
                    ]
                }
            };
            let (min, max) = adversary_body(base);
            let hit_box = if corrupt && index % 9 == 4 {
                if index % 18 == 4 {
                    StaticCollider {
                        min: [f32::NAN, 0.0, 0.0],
                        max: [1.0, 1.0, 1.0],
                    }
                } else {
                    StaticCollider {
                        min: [1.0, 0.0, 0.0],
                        max: [-1.0, 1.0, 1.0], // inverted on x
                    }
                }
            } else {
                StaticCollider { min, max }
            };
            roster.push(Combatant {
                id: index as u32 + 1,
                hit_box,
            });
        }
        roster
    }

    /// Bit-level equality — the battery's claim IS bit-identity, so
    /// even NaN results compare by their bit patterns (the B06
    /// discipline).
    fn same_f32(a: f32, b: f32) -> bool {
        a.to_bits() == b.to_bits()
    }

    fn same_point(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b.iter()).all(|(a, b)| same_f32(*a, *b))
    }

    fn same_resolution(a: HitResolution, b: HitResolution) -> bool {
        match (a, b) {
            (HitResolution::Refused(a), HitResolution::Refused(b)) => a == b,
            (
                HitResolution::Hit {
                    target: ta,
                    distance: da,
                    point: pa,
                },
                HitResolution::Hit {
                    target: tb,
                    distance: db,
                    point: pb,
                },
            ) => ta == tb && same_f32(da, db) && same_point(pa, pb),
            (
                HitResolution::Miss {
                    kind:
                        MissKind::Occluded {
                            distance: da,
                            point: pa,
                        },
                },
                HitResolution::Miss {
                    kind:
                        MissKind::Occluded {
                            distance: db,
                            point: pb,
                        },
                },
            ) => same_f32(da, db) && same_point(pa, pb),
            (
                HitResolution::Miss {
                    kind: MissKind::Clear,
                },
                HitResolution::Miss {
                    kind: MissKind::Clear,
                },
            ) => true,
            _ => false,
        }
    }

    /// The REAL callers' wall worlds: the combat demo's own arena
    /// (the exact `fire_world` the demo resolves through — perimeter
    /// walls, full-height pillars, below-eye low walls), the empty
    /// world, and a rotated-prism world (the encounter arena's
    /// cover class).
    fn caller_worlds() -> [(CollisionWorld, &'static str); 3] {
        let rotated = CollisionWorld::new(0.0).with_rotated_collider(Some(
            RotatedCollider::new([0.0, 1.5, -30.0], [4.0, 1.5, 0.5], 0.7)
                .expect("the fixture prism is finite and positive"),
        ));
        [
            (arena_world(), "demo-arena"),
            (CollisionWorld::new(0.0), "empty"),
            (rotated, "rotated-prism"),
        ]
    }

    /// The REAL callers' request shapes: the fire trace (the demo's
    /// eye at the player spawn, look directions at the demo's yaw/
    /// pitch classes, the 30 m weapon range), the sight probe (the
    /// long 120 m range the third-person contact query carries),
    /// point-blank origins inside roster bodies, and the diagonal
    /// class that trips the waste guard.
    fn caller_requests(roster: &[Combatant], seed: u64) -> Vec<AttackRequest> {
        let mut rng = FixtureRng(seed ^ 0x9E37_79B9_7F4A_7C15);
        let mut requests = Vec::with_capacity(36);
        for query in 0..36 {
            let origin = if query % 12 == 0 {
                // Point-blank: the origin sits inside a roster body
                // (t = 0) — or inside NaN for a corrupted one.
                let body = roster[(query * 7) % roster.len()].hit_box;
                [
                    (body.min[0] + body.max[0]) * 0.5,
                    (body.min[1] + body.max[1]) * 0.5,
                    (body.min[2] + body.max[2]) * 0.5,
                ]
            } else if query % 12 == 1 {
                // The demo's own eye: the player spawn at eye height.
                [ADVERSARY_SPAWN[0], 1.5, 6.0]
            } else {
                [
                    rng.range(-50.0, 50.0),
                    rng.range(0.5, 2.2),
                    rng.range(-50.0, 50.0),
                ]
            };
            let direction = match query % 4 {
                0 => look_direction(0.0, 0.0), // dead ahead (the d == 0 slab branches)
                1 => look_direction(0.25, -0.1), // the demo's diagonal aim class
                2 => [rng.range(-1.0, 1.0), rng.range(-0.4, 0.4), -1.0],
                _ => [rng.range(-1.0, 1.0), 0.0, rng.range(-1.0, 1.0)],
            };
            let range = [30.0f32, 120.0, 170.0][query % 3];
            let margin = [0.0f32, 0.5][query % 2];
            requests.push(AttackRequest {
                attacker: 0,
                origin,
                direction,
                profile: AttackProfile {
                    range,
                    occlusion_margin: margin,
                },
            });
        }
        requests
    }

    // ------------------------------------------- the allocation counters
    // (the HostAllocScope/spatial_query_allocations discipline,
    // mirrored for the lib test binary: a forwarding global
    // allocator that counts events and bytes on the CURRENT thread
    // while a window is open. `#[cfg(test)]`-scoped, so the shipped
    // library and the integration batteries keep the default
    // allocator; the thread attribution keeps the counts true amid
    // the binary's other tests; the double-run equality test proves
    // the sequence deterministic.)

    // This thread's window state (const-init thread-locals:
    // allocation-free to read, no destructors, no lazy machinery).
    thread_local! {
        static IN_WINDOW: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        static WINDOW_EVENTS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
        static WINDOW_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }

    fn count_measured(size: usize) {
        IN_WINDOW.with(|measured| {
            if measured.get() {
                WINDOW_EVENTS.with(|events| events.set(events.get() + 1));
                WINDOW_BYTES.with(|bytes| bytes.set(bytes.get() + size as u64));
            }
        });
    }

    /// Forwarding allocator that only counts allocation events and
    /// the requested byte sizes on the measuring thread (the
    /// `tests/common/mod.rs` + `spatial_query_allocations.rs`
    /// twins).
    ///
    /// SAFETY EXCEPTION (the unsafe-with-proofs policy, the
    /// established test-allocator precedent): the allowance is
    /// scoped to this impl so the rest of the module keeps the
    /// workspace's `unsafe_code = "deny"` restriction.
    #[allow(unsafe_code)]
    // SAFETY: every method forwards its arguments unchanged to the
    // system allocator (which satisfies the global allocator
    // contract), adding only thread-local Cell increments that
    // cannot reenter allocation.
    unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
            count_measured(layout.size());
            // SAFETY: forwarded unchanged to the system allocator.
            unsafe { std::alloc::System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
            count_measured(layout.size());
            // SAFETY: forwarded unchanged to the system allocator.
            unsafe { std::alloc::System.alloc_zeroed(layout) }
        }

        unsafe fn realloc(
            &self,
            ptr: *mut u8,
            layout: std::alloc::Layout,
            new_size: usize,
        ) -> *mut u8 {
            count_measured(new_size);
            // SAFETY: forwarded unchanged to the system allocator.
            unsafe { std::alloc::System.realloc(ptr, layout, new_size) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
            // SAFETY: forwarded unchanged to the system allocator.
            unsafe { std::alloc::System.dealloc(ptr, layout) }
        }
    }

    struct CountingAllocator;

    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;

    /// One measured window: runs `reps` repetitions of `run` on THIS
    /// thread and returns `(allocation events, allocation bytes,
    /// total ms)`. Everything the repetitions touch is built by the
    /// caller BEFORE the window opens — every counted allocation is
    /// repetition-side. The closure's return value is discarded
    /// (the resolve arms return their `HitResolution`; only the
    /// window's counts and timing are observed).
    fn measure<T>(reps: u32, mut run: impl FnMut() -> T) -> (u64, u64, f64) {
        WINDOW_EVENTS.with(|events| events.set(0));
        WINDOW_BYTES.with(|bytes| bytes.set(0));
        IN_WINDOW.with(|measured| measured.set(true));
        let start = std::time::Instant::now();
        for _ in 0..reps {
            run();
        }
        let elapsed = start.elapsed().as_secs_f64();
        IN_WINDOW.with(|measured| measured.set(false));
        (
            WINDOW_EVENTS.with(|events| events.get()),
            WINDOW_BYTES.with(|bytes| bytes.get()),
            elapsed * 1e3,
        )
    }

    /// The REAL callers' query mix for the crossover measurement:
    /// the two weapons' actual profile ranges — the precision 30 m
    /// and the spread 12 m (both demos' placeholder profiles; the
    /// sight probe uses the SAME weapon ranges) — over the callers'
    /// aim classes (point-blank, the demo eye, dead-ahead, diagonal,
    /// lateral). Every live ray is SHORT; the long-diagonal class
    /// that trips the waste guard is a fallback arm, not this mix
    /// (it stays in the equivalence battery where both regimes are
    /// the claim).
    fn caller_mix(roster: &[Combatant], seed: u64) -> Vec<AttackRequest> {
        let mut rng = FixtureRng(seed ^ 0x9E37_79B9_7F4A_7C15);
        let mut requests = Vec::with_capacity(8);
        for query in 0..8 {
            let origin = if query == 0 {
                // Point-blank: the origin sits inside a roster body
                // (t = 0).
                let body = roster[(query * 7) % roster.len()].hit_box;
                [
                    (body.min[0] + body.max[0]) * 0.5,
                    (body.min[1] + body.max[1]) * 0.5,
                    (body.min[2] + body.max[2]) * 0.5,
                ]
            } else if query == 1 {
                // The demo's own eye: the player spawn at eye height.
                [ADVERSARY_SPAWN[0], 1.5, 6.0]
            } else {
                [
                    rng.range(-50.0, 50.0),
                    rng.range(0.5, 2.2),
                    rng.range(-50.0, 50.0),
                ]
            };
            let direction = match query % 4 {
                0 => look_direction(0.0, 0.0), // dead ahead (the d == 0 slab branches)
                1 => look_direction(0.25, -0.1), // the demo's diagonal aim class
                2 => [rng.range(-1.0, 1.0), rng.range(-0.4, 0.4), -1.0],
                _ => [rng.range(-1.0, 1.0), 0.0, rng.range(-1.0, 1.0)],
            };
            let range = [30.0f32, 12.0][query % 2];
            requests.push(AttackRequest {
                attacker: 0,
                origin,
                direction,
                profile: AttackProfile {
                    range,
                    occlusion_margin: 0.0,
                },
            });
        }
        requests
    }

    // ============================================= THE MANDATORY TESTS

    /// (1) THE EQUIVALENCE AT THE ADOPTED CALLER — the seam's own
    /// `resolve` (the exact code the live fire/sight paths now call)
    /// vs the exact linear primitive, bit-for-bit through the f32
    /// bit-pattern discipline, at increasing sizes (the live 1/4
    /// rosters; the gate 128; 2,048; 32,768), the three roster
    /// shapes, the corrupted-entry class, and the real callers'
    /// request shapes through the real callers' wall worlds. The
    /// below-gate rows pin the seam's DECLINE (the audit's retained
    /// tiny-roster path — the exact linear call, provably the same
    /// resolution); the at/above-gate rows exercise the adopted
    /// index arm (the grid genuinely built).
    #[test]
    fn fire_roster_equivalence_at_the_caller() {
        // The live callers' rosters first: the combat demo's single
        // authored body at its spawn, and the encounter's
        // four-adversary wave (the `adversary_body` extent class at
        // the encounter's authored spread).
        let demo_body = adversary_body(ADVERSARY_SPAWN);
        let live_single = vec![Combatant {
            id: 10,
            hit_box: StaticCollider {
                min: demo_body.0,
                max: demo_body.1,
            },
        }];
        let live_quad: Vec<Combatant> = (0..4)
            .map(|index| {
                let (min, max) = adversary_body([
                    -4.0 + 2.6 * index as f32,
                    0.0,
                    -7.0 + 1.2 * (index % 2) as f32,
                ]);
                Combatant {
                    id: 10 + index,
                    hit_box: StaticCollider { min, max },
                }
            })
            .collect();
        for (roster, label) in [(&live_single, "live-1"), (&live_quad, "live-4")] {
            let plan = FireRoster::for_revision(roster, 5);
            assert!(
                !plan.indexed(),
                "{label}: the tiny roster declines the build (the retained linear path)"
            );
            for (world, world_label) in caller_worlds() {
                for request in caller_requests(roster, 0xF190_0001) {
                    let through_seam = plan.resolve(&world, &request);
                    let linear = resolve_attack(&world, &request, roster);
                    assert!(
                        same_resolution(through_seam, linear),
                        "{label} {world_label}: the declined seam must be the exact linear call"
                    );
                }
            }
        }

        // The adopted arm: at/above the composition gate the seam
        // carries a REAL grid; every resolution must be bit-identical
        // to the linear authority on the IDENTICAL roster — ties
        // included (the (distance, id) selection), occlusion included
        // (three wall worlds), point-blank included, and the
        // long-range queries that trip the waste guard (both regimes
        // are the claim: the guard's fallback IS the linear path).
        // The 128 rows pin the composition boundary's DECLINE at the
        // engine gate's own edge (the measured wash row) and then
        // force the index arm on anyway — B06's `force_index_on`
        // discipline, one level up — so the arm's bit-identity is
        // proven at the engine gate's edge too, where the ENGINE's
        // build genuinely builds the grid.
        let rows = [
            (128usize, Shape::Spread, false),
            (128, Shape::Cluster, false),
            (2048, Shape::Spread, false),
            (2048, Shape::Ring, false),
            (2048, Shape::Cluster, true), // corrupted entries: skipped, never poisoned
            (32_768, Shape::Spread, false),
        ];
        for (count, shape, corrupt) in rows {
            for seed in [0xF190_0001u64, 0xF190_0010] {
                let roster = fixture_roster(seed, count, shape, corrupt);
                let gate_plan = FireRoster::for_revision(&roster, FIRE_INDEX_QUERIES_GATE);
                let plan = if count >= FIRE_INDEX_ROSTER_GATE {
                    assert!(
                        gate_plan.indexed(),
                        "{count}/{shape:?}: the composition gate adopts \
                         (roster {count} >= {FIRE_INDEX_ROSTER_GATE}, queries >= \
                         {FIRE_INDEX_QUERIES_GATE})"
                    );
                    gate_plan
                } else {
                    assert!(
                        !gate_plan.indexed(),
                        "{count}/{shape:?}: the composition boundary declines at the \
                         engine gate's own edge (the measured wash row — the engine's \
                         {COMBAT_INDEX_GATE} build gate still builds the grid when built)"
                    );
                    forced_adoption(&roster)
                };
                let label = format!("{count}/{shape:?} corrupt={corrupt} @ {seed}");
                for (world, world_label) in caller_worlds() {
                    for (index, request) in caller_requests(&roster, seed).into_iter().enumerate() {
                        let through_seam = plan.resolve(&world, &request);
                        let linear = resolve_attack(&world, &request, &roster);
                        assert!(
                            same_resolution(through_seam, linear),
                            "{label} {world_label} q{index}: seam {through_seam:?} vs linear {linear:?}"
                        );
                    }
                }

                // Slice order: the (distance, id) selection is
                // order-independent for distinct ids — pin it through
                // the seam too (the candidate re-sort must not leak
                // hash order into the result).
                let mut reversed = roster.clone();
                reversed.reverse();
                let reversed_plan = if count >= FIRE_INDEX_ROSTER_GATE {
                    let plan = FireRoster::for_revision(&reversed, FIRE_INDEX_QUERIES_GATE);
                    assert!(plan.indexed(), "{label}: the reversed roster adopts too");
                    plan
                } else {
                    forced_adoption(&reversed)
                };
                let world = CollisionWorld::new(0.0);
                for request in caller_requests(&reversed, seed ^ 0xD1CE_B00B_5EED_1A5E) {
                    let through_seam = reversed_plan.resolve(&world, &request);
                    let linear = resolve_attack(&world, &request, &reversed);
                    assert!(
                        same_resolution(through_seam, linear),
                        "{label} reversed: the seam must be slice-order-independent"
                    );
                }
            }
        }
    }

    /// The tie-ordering half of the equivalence, on AUTHORED ties
    /// (not just fixture luck): two bodies at the SAME distance on
    /// symmetric offsets straddling the trace (the lowest id must
    /// win on both arms), below and above the gate — the index arm's
    /// candidate re-sort must not leak cell order into the
    /// selection.
    #[test]
    fn fire_roster_tie_ordering_matches_linear() {
        // Two symmetric bodies equidistant from a dead-ahead trace:
        // ids 7 (x +0.2) and 3 (x −0.2), both at z −10 — the body
        // half-extent (0.45) covers the trace's x = 0 on both
        // sides, so both are hit at the SAME z entry: the lowest id
        // (3) wins.
        let (min_a, max_a) = adversary_body([0.2, 0.0, -10.0]);
        let (min_b, max_b) = adversary_body([-0.2, 0.0, -10.0]);
        let tied = vec![
            Combatant {
                id: 7,
                hit_box: StaticCollider {
                    min: min_a,
                    max: max_a,
                },
            },
            Combatant {
                id: 3,
                hit_box: StaticCollider {
                    min: min_b,
                    max: max_b,
                },
            },
        ];
        // ...grown with the same symmetric pair pattern so the INDEX
        // arm serves the tie (the grid must resolve it exactly as the
        // linear scan does); the grown ids sit behind the front pair
        // and never displace it. Two sizes: past the ENGINE's build
        // gate (136 — the forced-adoption control, the grid genuinely
        // built by the engine gate) and past the COMPOSITION gate
        // (2,056 — the real adoption through `for_revision`).
        let mut grown = Vec::with_capacity(FIRE_INDEX_ROSTER_GATE + 8);
        for index in 0..(FIRE_INDEX_ROSTER_GATE + 8) {
            let side = if index % 2 == 0 { 0.2 } else { -0.2 };
            let (min, max) = adversary_body([side, 0.0, -10.0 - (index / 64) as f32]);
            grown.push(Combatant {
                id: if index < 2 {
                    [7u32, 3][index]
                } else {
                    20 + index as u32
                },
                hit_box: StaticCollider { min, max },
            });
        }
        let engine_edge: Vec<Combatant> =
            grown.iter().take(COMBAT_INDEX_GATE + 8).cloned().collect();
        let world = CollisionWorld::new(0.0);
        let request = AttackRequest {
            attacker: 0,
            origin: [0.0, 1.5, 0.0],
            direction: look_direction(0.0, 0.0),
            profile: AttackProfile {
                range: 30.0,
                occlusion_margin: 0.0,
            },
        };
        // The tiny tied roster declines the boundary (the retained
        // linear path); the engine-edge roster forces the index arm
        // on (the control — the engine's build gate builds the grid
        // there); the composition-gate roster REALLY adopts.
        let declined = FireRoster::for_revision(&tied, FIRE_INDEX_QUERIES_GATE);
        assert!(
            !declined.indexed(),
            "the two-body roster declines (the retained linear path)"
        );
        let forced = forced_adoption(&engine_edge);
        assert!(
            forced.indexed(),
            "the engine-edge control carries the grid (the engine's build gate)"
        );
        let adopted = FireRoster::for_revision(&grown, FIRE_INDEX_QUERIES_GATE);
        assert!(
            adopted.indexed(),
            "the composition-gate roster really adopted (the index arm serves the tie)"
        );
        for (roster, plan, label) in [
            (&tied[..], &declined, "tied (declines)"),
            (&engine_edge[..], &forced, "engine-gate edge (forced)"),
            (&grown[..], &adopted, "composition gate (adopts)"),
        ] {
            let linear = resolve_attack(&world, &request, roster);
            let through_seam = plan.resolve(&world, &request);
            assert!(
                same_resolution(through_seam, linear),
                "{label} (roster of {}): the tie must resolve identically \
                 (linear {linear:?} vs seam {through_seam:?})",
                roster.len()
            );
            match linear {
                HitResolution::Hit { target, .. } => {
                    // The authored tie: both arms pick the lowest id
                    // among the equidistant front pair (3 < 7).
                    assert_eq!(
                        target, 3,
                        "{label}: the equidistant pair's lowest id wins on both arms"
                    );
                }
                other => panic!("the dead-ahead trace must hit the pair: {other:?}"),
            }
        }
    }

    /// (2) THE BUILD AMORTIZATION — the crossover measured at the
    /// real caller's shape, and the structural half pinned. The
    /// per-query costs are measured over the caller's OWN query mix
    /// ([`caller_mix`]: the two weapons' real profile ranges — every
    /// live ray is short — over the aim classes). The timings use
    /// MEDIAN statistics (the `census` harness's own
    /// [`crate::census::timing_stats`] — debug-profile noise is
    /// large; medians are the evidence, printed pod-observationally
    /// per the repo's never-a-CI-threshold discipline). The pinned
    /// facts are structural: the linear arm allocates NOTHING per
    /// query, the declined seam allocates NOTHING (no build below
    /// the gates), the adopted build allocates its grid (the
    /// amortized cost, a deterministic function of the roster), and
    /// the per-query churn through the adopted arm stays the
    /// log-growth chain shape (a handful of events; the BYTES scale
    /// with the candidates gathered — workload, not structure — and
    /// are printed).
    #[test]
    fn fire_index_crossover_at_the_caller_shape() {
        use crate::census::timing_stats;
        println!(
            "F09 build-amortization crossover at the fire seam \
             (pod-observational; UNOPTIMIZED test profile — medians are the evidence)"
        );
        for count in [128usize, 2048, 32_768] {
            let roster = fixture_roster(0xF190_C0DE, count, Shape::Spread, false);
            let world = CollisionWorld::new(0.0);
            // The caller's real query mix (built BEFORE any window).
            let mix = caller_mix(&roster, 0xF190_C0DE);
            // The measured arm: the machinery question (what the
            // grid costs and saves at this roster), so the 128 row —
            // the engine gate's own edge, where the COMPOSITION
            // boundary declines (the wash row) — is measured through
            // the forced-adoption control, honestly labeled; the
            // confident sizes measure the real adoption.
            let boundary_plan = FireRoster::for_revision(&roster, FIRE_INDEX_QUERIES_GATE);
            let (plan, arm_label) = if count >= FIRE_INDEX_ROSTER_GATE {
                assert!(
                    boundary_plan.indexed(),
                    "{count}: the composition gate adopts for the measurement"
                );
                (boundary_plan, "adopted")
            } else {
                assert!(
                    !boundary_plan.indexed(),
                    "{count}: the composition boundary declines at the engine gate's edge \
                     (the wash row — the machinery is measured through the forced control)"
                );
                (forced_adoption(&roster), "forced (machinery)")
            };
            // The per-revision build cost — the MACHINERY cost
            // (`CombatIndex::build` over the roster, what any
            // adoption pays per revision): the allocation shape
            // through the counting window (16 identical builds
            // allocate identically, so the per-build counts divide
            // exactly), the timing as a 32-sample MEDIAN (separate
            // loop — the window's atomics-free path and the median
            // loop measure the same work).
            let (build_events, build_bytes, _) = measure(16, || CombatIndex::build(&roster));
            let per_build_events = build_events / 16;
            let per_build_bytes = build_bytes / 16;
            assert!(
                per_build_events > 0 && per_build_bytes > 0,
                "{count}: the build allocates its grid (the amortized cost)"
            );
            let mut build_samples = Vec::with_capacity(32);
            for _ in 0..32 {
                let started = std::time::Instant::now();
                let _ = CombatIndex::build(&roster);
                build_samples.push(started.elapsed());
            }
            let build_stats = timing_stats(&mut build_samples);
            // The adopted arm's per-query cost vs the linear arm's,
            // over the same mix: 24 passes, each one resolve of the
            // whole 8-request mix, reduced to the per-pass MEDIAN
            // (the mix is the caller's real per-revision shape; a
            // volley-sized burst, not one ray).
            let mut linear_samples = Vec::with_capacity(24);
            let mut indexed_samples = Vec::with_capacity(24);
            for _ in 0..24 {
                let started = std::time::Instant::now();
                for request in &mix {
                    resolve_attack(&world, request, &roster);
                }
                linear_samples.push(started.elapsed());
                let started = std::time::Instant::now();
                for request in &mix {
                    plan.resolve(&world, request);
                }
                indexed_samples.push(started.elapsed());
            }
            // The allocation comparison over the same measured
            // passes (one window per arm; the counts are per-pass
            // over the 8-request mix).
            let (linear_events, _, _) = measure(24, || {
                for request in &mix {
                    resolve_attack(&world, request, &roster);
                }
            });
            let (indexed_events, indexed_bytes, _) = measure(24, || {
                for request in &mix {
                    plan.resolve(&world, request);
                }
            });
            assert_eq!(
                linear_events, 0,
                "{count}: the linear arm allocates NOTHING per query (the pinned zero)"
            );
            let passes = 24 * mix.len() as u64;
            let linear_per_query =
                timing_stats(&mut linear_samples).median_us / 1000.0 / mix.len() as f64;
            let indexed_per_query =
                timing_stats(&mut indexed_samples).median_us / 1000.0 / mix.len() as f64;
            let build_per_revision = build_stats.median_us / 1000.0;
            let savings = (linear_per_query - indexed_per_query).max(1e-9);
            let crossover = (build_per_revision / savings).ceil();
            let per_query_events = indexed_events / passes;
            let per_query_bytes = indexed_bytes / passes;
            assert!(
                per_query_events <= 32,
                "{count}: the grid arm's per-query churn stays the log-growth chain \
                 shape (measured {per_query_events} events / {per_query_bytes} B)"
            );
            println!(
                "  roster {count:6} [{arm_label}]: build {build_per_revision:8.3} ms/revision \
                 (median; {per_build_events} allocs, {per_build_bytes} B) | linear \
                 {linear_per_query:8.3} ms/query | indexed {indexed_per_query:8.3} ms/query \
                 | crossover ~{crossover:7.1} queries (query gate {FIRE_INDEX_QUERIES_GATE})"
            );
            println!(
                "    per-query churn through the index arm: {per_query_events} allocs, \
                 {per_query_bytes} B (linear: 0, 0 — PINNED; the bytes scale with the \
                 candidates gathered — workload, not structure)"
            );

            // THE GATE'S DERIVATION CHECK (printed, not asserted —
            // the timing discipline): at every ADOPTING size the
            // query gate must sit at or above the measured
            // crossover; at the declined engine-edge row the roster
            // half already keeps the boundary off regardless (the
            // wash row's crossover is noise-bound by construction).
            if count >= FIRE_INDEX_ROSTER_GATE {
                let covered = crossover.is_finite()
                    && crossover >= 0.0
                    && crossover <= FIRE_INDEX_QUERIES_GATE as f64;
                println!(
                    "    query gate {FIRE_INDEX_QUERIES_GATE} covers the measured crossover \
                     {crossover:.1}: {}",
                    if covered {
                        "yes"
                    } else {
                        "NO — re-derive the gate"
                    }
                );
            } else {
                println!(
                    "    the composition boundary declines this size on the ROSTER half \
                     (the wash row — no query count pays at the engine gate's edge)"
                );
            }
        }

        // The gate's truth table (the decision itself, pinned):
        // every live boundary declines — no composition in this
        // engine presents gate-many queries against one stable
        // roster revision (the press trace is 1, the sight probe 1,
        // the spread volley 5, all measured) — and the adoption
        // region is exactly the both-halves-clear quadrant.
        assert!(!fire_index_advisable(1, 1), "the combat demo fire (1, 1)");
        assert!(!fire_index_advisable(4, 1), "the encounter sight (4, 1)");
        assert!(!fire_index_advisable(4, 5), "the encounter volley (4, 5)");
        assert!(
            !fire_index_advisable(32_768, 5),
            "the spread volley's 5 rays stay linear at ANY roster (measured: the \
             confident crossovers are ~16–18 at 2,048 and ~11–15 at 32,768)"
        );
        assert!(
            !fire_index_advisable(FIRE_INDEX_ROSTER_GATE - 1, u64::MAX),
            "below the composition roster gate no query count adopts"
        );
        assert!(
            !fire_index_advisable(COMBAT_INDEX_GATE, u64::MAX),
            "the engine gate's own edge never adopts at the composition (the measured \
             wash row — the composition floor sits at the first confident size)"
        );
        assert!(
            !fire_index_advisable(FIRE_INDEX_ROSTER_GATE, FIRE_INDEX_QUERIES_GATE - 1),
            "at the composition roster gate the query half still decides"
        );
        assert!(
            fire_index_advisable(FIRE_INDEX_ROSTER_GATE, FIRE_INDEX_QUERIES_GATE),
            "the both-halves quadrant adopts"
        );
        assert!(fire_index_advisable(32_768, FIRE_INDEX_QUERIES_GATE));
    }

    /// (3) THE FALLBACK ARMS — the dense cluster, the long diagonal
    /// (the B06/Q09 waste row), the poisoned roster, and the
    /// refuse-arms: bit-identity through every arm, with the
    /// per-query allocation count as the ARM DISCRIMINATOR (the
    /// waste-guard fallback and the poisoned roster allocate
    /// nothing per query; the grid arm's churn is the measured
    /// nonzero — the counts prove WHICH arm served).
    #[test]
    fn fire_roster_fallback_arms() {
        let world = CollisionWorld::new(0.0);

        // THE LONG DIAGONAL (the waste-guard row): a 170 m diagonal
        // from (−80, +80) spans ~60×60 = 3,600 cells at the 2 m cell
        // size. At roster 128 the guard (4 cells per combatant =
        // 512) fires — the query is served by the EXACT linear path
        // INSIDE the adopted index, allocating nothing; at 32,768
        // (4 × 32,768 = 131,072 ≥ 3,600) the grid arm serves — the
        // B06-recorded honest waste row (the whole-roster band
        // through the grid: slower than linear there, an owner
        // observation, never a wrong answer).
        let long_shot = AttackRequest {
            attacker: 0,
            origin: [-80.0, 1.5, 80.0],
            direction: [1.0, 0.0, -1.0],
            profile: AttackProfile {
                range: 170.0,
                occlusion_margin: 0.0,
            },
        };
        for (count, expect_grid_arm) in [(128usize, false), (32_768, true)] {
            let roster = fixture_roster(0xFA11_BA0D, count, Shape::Spread, false);
            // The 128 row is the engine gate's edge — the composition
            // boundary declines it (the wash row), so the guard
            // discrimination is measured through the forced-adoption
            // control (the engine's build gate builds the grid
            // there); the 32,768 row is the real adoption.
            let plan = if count >= FIRE_INDEX_ROSTER_GATE {
                let plan = FireRoster::for_revision(&roster, FIRE_INDEX_QUERIES_GATE);
                assert!(plan.indexed(), "{count}: the composition gate adopts");
                plan
            } else {
                assert!(
                    !FireRoster::for_revision(&roster, FIRE_INDEX_QUERIES_GATE).indexed(),
                    "{count}: the composition boundary declines the engine gate's edge"
                );
                forced_adoption(&roster)
            };
            let (events, bytes, _) = measure(32, || plan.resolve(&world, &long_shot));
            let per_query_events = events / 32;
            let per_query_bytes = bytes / 32;
            if expect_grid_arm {
                assert!(
                    per_query_events > 0,
                    "{count} long diagonal: the grid arm serves (the measured nonzero churn)"
                );
            } else {
                assert_eq!(
                    per_query_events, 0,
                    "{count} long diagonal: the waste guard serves the exact linear path \
                     (the pinned zero — the arm discriminator)"
                );
            }
            let through_seam = plan.resolve(&world, &long_shot);
            let linear = resolve_attack(&world, &long_shot, &roster);
            assert!(
                same_resolution(through_seam, linear),
                "{count} long diagonal: the fallback arm is bit-identical"
            );
            println!(
                "  long diagonal @ {count:6}: grid arm {} ({} allocs, {} B per query), \
                 result bit-identical",
                if expect_grid_arm {
                    "SERVED"
                } else {
                    "declined (guard)"
                },
                per_query_events,
                per_query_bytes
            );
        }

        // THE DENSE CLUSTER (the whole roster inside a few cells):
        // the guard bounds cells WALKED, not candidates GATHERED —
        // the grid arm serves with a maximal candidate set; the
        // result must still be bit-identical (the Q09 cluster
        // observation, re-proven at the seam).
        let cluster = fixture_roster(0x0D3E_0001, 2048, Shape::Cluster, false);
        let cluster_plan = FireRoster::for_revision(&cluster, FIRE_INDEX_QUERIES_GATE);
        assert!(cluster_plan.indexed(), "the cluster roster adopts");
        for request in caller_requests(&cluster, 0x0D3E_0001) {
            let through_seam = cluster_plan.resolve(&world, &request);
            let linear = resolve_attack(&world, &request, &cluster);
            assert!(
                same_resolution(through_seam, linear),
                "the dense cluster is bit-identical through the grid arm"
            );
        }

        // THE POISONED ROSTER (a VALID hitbox outside the cell
        // domain): the build refuses the grid for the whole roster
        // (fail-closed `Unusable`) and EVERY resolve takes the exact
        // linear path — bit-identical, and allocation-free per query
        // (the discriminator: no candidate storage exists). The
        // probe request is built BEFORE the window so only the
        // resolve is counted.
        let mut poisoned = fixture_roster(0x0F05_0002, 2048, Shape::Spread, false);
        let (min, max) = adversary_body([5_000_000.0, 0.0, 0.0]);
        poisoned[7].hit_box = StaticCollider { min, max };
        let poisoned_plan = FireRoster::for_revision(&poisoned, FIRE_INDEX_QUERIES_GATE);
        assert!(
            poisoned_plan.indexed(),
            "the gate adopted (the build then refused — fail-closed)"
        );
        let poisoned_probes = caller_requests(&poisoned, 0x0F05_0002);
        let (events, _, _) = measure(32, || poisoned_plan.resolve(&world, &poisoned_probes[5]));
        assert_eq!(
            events, 0,
            "the poisoned roster serves every query through the exact linear path"
        );
        for (index, request) in poisoned_probes.into_iter().enumerate().take(24) {
            let through_seam = poisoned_plan.resolve(&world, &request);
            let linear = resolve_attack(&world, &request, &poisoned);
            assert!(
                same_resolution(through_seam, linear),
                "poisoned q{index}: the fail-closed build is bit-identical"
            );
        }

        // THE REFUSE-ARMS (the request contract, shared code on both
        // paths): non-finite direction, zero direction, non-finite
        // origin — identical typed refusals, validation order
        // included.
        for direction in [
            [f32::NAN, 0.0, 1.0],
            [0.0, 0.0, 0.0],
            [f32::INFINITY, 0.0, 1.0],
        ] {
            let request = AttackRequest {
                attacker: 0,
                origin: [f32::NAN, 1.5, 0.0],
                direction,
                profile: AttackProfile {
                    range: 30.0,
                    occlusion_margin: 0.0,
                },
            };
            let roster = fixture_roster(0xEF11_0001, 128, Shape::Spread, false);
            // The engine gate's edge, forced on (the control) so the
            // INDEX arm's refusal path is covered too — the refusal
            // happens before any geometry, on both arms identically.
            let plan = forced_adoption(&roster);
            let through_seam = plan.resolve(&world, &request);
            let linear = resolve_attack(&world, &request, &roster);
            assert!(
                matches!(
                    (&through_seam, &linear),
                    (HitResolution::Refused(_), HitResolution::Refused(_))
                ) && same_resolution(through_seam, linear),
                "the refuse-arms are identical typed refusals ({through_seam:?})"
            );
            assert!(
                matches!(
                    through_seam,
                    HitResolution::Refused(AttackRefusal::NonFiniteOrigin)
                ),
                "the validation order refuses the origin first: {through_seam:?}"
            );
        }
    }

    /// (1b) THE ALLOCATION COMPARISON AT THE ADOPTED CALLER — the
    /// double-run equality (the repo-wide determinism discipline
    /// applied to the allocation sequence itself: the table is
    /// meaningless if the sequence is not deterministic). Two full
    /// runs of the same workload must produce the IDENTICAL
    /// (events, bytes) pairs on every arm.
    #[test]
    fn fire_roster_allocation_sequence_is_deterministic() {
        let run_once = || {
            let roster = fixture_roster(0xA110_0003, 2048, Shape::Spread, false);
            let world = CollisionWorld::new(0.0);
            let requests = caller_requests(&roster, 0xA110_0003);
            let plan = FireRoster::for_revision(&roster, FIRE_INDEX_QUERIES_GATE);
            let declined = FireRoster::for_revision(&roster, 1);
            let (seam_events, seam_bytes, _) = measure(64, || plan.resolve(&world, &requests[3]));
            let (linear_events, linear_bytes, _) =
                measure(64, || resolve_attack(&world, &requests[3], &roster));
            let (declined_events, declined_bytes, _) =
                measure(64, || declined.resolve(&world, &requests[3]));
            (
                seam_events,
                seam_bytes,
                linear_events,
                linear_bytes,
                declined_events,
                declined_bytes,
            )
        };
        let first = run_once();
        let second = run_once();
        assert_eq!(
            first, second,
            "the allocation sequence is deterministic across runs (every arm)"
        );
        assert_eq!(
            first.2, 0,
            "the linear arm per-query events: the pinned zero"
        );
        assert_eq!(
            first.3, 0,
            "the linear arm per-query bytes: the pinned zero"
        );
        assert_eq!(
            first.4, 0,
            "the declined seam per-query events: no build below the gates"
        );
        assert_eq!(
            first.5, 0,
            "the declined seam per-query bytes: no build below the gates"
        );
        assert!(
            first.0 > 0 && first.1 > 0,
            "the adopted arm's per-query churn is the measured nonzero (Q09's row)"
        );
        println!(
            "  allocation comparison @2048: adopted arm {} events / {} B per 64 queries; \
             linear 0/0; declined 0/0 (PINNED, double-run identical)",
            first.0, first.1
        );
    }
}
