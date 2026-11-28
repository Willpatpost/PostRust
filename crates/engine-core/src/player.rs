//! Cycle 1 P3 — the reusable player movement and collision prototype.
//!
//! Kinematic, std-only, deterministic fixed-step mechanics: the first
//! player capability beyond the free-flight debug camera. This is a
//! REVERSIBLE PROTOTYPE and architectural-feedback instrument, not a
//! ratified production physics backend — the collision query surface
//! ([`CollisionWorld`]) is deliberately narrow so an established physics
//! library can slot behind the same contract in a later ratified cycle
//! (Rapier was the evaluated candidate; see the Cycle-1 P3 matrix).
//!
//! Determinism discipline (the spine's rules, mirrored): wall-clock time
//! never enters logical state — [`PlayerDriver`] accrues elapsed time
//! into due ticks exactly like `SimDriver` (bounded catch-up per frame,
//! no-discard accounting: due == executed + outstanding, always);
//! [`step_player`] is a function of state, intent, and the fixed tick
//! `dt` only. Traces are FNV-1a fingerprints over IEEE-754 bit patterns
//! under the NEW domain `engine-core/player-trace/1` — no historical
//! fingerprint domain is touched. Cross-platform bitwise determinism is
//! claimed ONLY for libm-free scripted workloads (yaw and pitch exactly
//! zero through the run, so `sin`/`cos` return their exact values and
//! only correctly-rounded arithmetic remains); look math at nonzero
//! angles is same-binary-same-platform reproducible, never claimed
//! stronger (charter rule: no unproven determinism claims).
//!
//! Engine/game boundary: this module owns MECHANICS — movement
//! integration, collision queries, camera-mode derivation. Game policy
//! (speeds, key bindings, world content, presentation) stays in the
//! application layer, which supplies [`PlayerConfig`] values.
//!
//! Collision model (prototype scope, documented): axis-aligned boxes
//! against a ground plane and optional world bounds; per-axis swept
//! resolution in a fixed Y→X→Z order with a [`SKIN`] contact margin, so
//! resting contact never blocks movement along a perpendicular axis and
//! no speed can tunnel through a wall thinner than one tick's travel.
//! Overlapping (spawned-inside) states depenetrate along the
//! minimal-penetration axis to the NEAR face, never through to the far
//! side. One depenetration pass per step (multi-box pile-ups are out of
//! prototype scope). The third-person camera has no collision of its
//! own (documented limitation).
//!
//! C2-P1 (the playable-world interaction loop): the grounded
//! step-up probe ([`PlayerConfig::step_height`], default 0 =
//! DISABLED, preserving the Cycle-1 clamp behavior bit-exactly — the
//! frozen trace's guarantee); the exact segment intersection query
//! [`CollisionWorld::segment_hit`] (ray–AABB slabs, no sampling
//! aliasing); the bounded TP-camera obstruction pull-in
//! [`camera_pose_obstructed`] (with documented residuals); and the
//! game-layer world swap [`PlayerDriver::set_world`]. The minimal
//! reusable interaction selection lives in `crate::interaction`.
//!
//! Input-validity contracts (the Cycle-1 R4/R4b correctives): tick
//! rates are clamped into [`MIN_TICK_HZ`]..=[`MAX_TICK_HZ`] — the
//! representable fixed-step range at nanosecond logical resolution
//! (`period_ns` ≥ 1 ns and `dt` finite and positive; a rate above one
//! billion would round the nanosecond period to zero — the
//! division-by-zero hazard this policy closes). [`step_player`]
//! refuses a non-finite or non-positive `dt` and any non-finite or
//! semantically invalid [`PlayerConfig`] BEFORE any state mutation,
//! reporting `refused_invalid_inputs`: a valid player state can
//! never be silently turned non-finite by invalid inputs (a
//! non-finite incoming STATE is refused separately,
//! `refused_nonfinite_state`). The R4b semantic minimums
//! (positive `pitch_limit` — a negative one inverts the pitch clamp
//! and panics — and positive `height`/`radius` extents) and exact
//! u64 tick accounting across the whole tick-rate range: a 2³²-tick
//! obligation at the 1 GHz bound accounts exactly, and the driver's
//! total logical advancement is bounded to [`MAX_TOTAL_ADVANCE_NS`]
//! with any clamping REPORTED, never silent.
//!
//! P2 L5 — the static-world broad phase (the scalability objective:
//! engine work proportional to the relevant workload, not the global
//! population). [`CollisionWorld`] carries a persistent uniform grid
//! over BOTH collider lists (keyed by each collider's world AABB —
//! the rotated list's cached one, the axis-aligned list's per-axis
//! sorted interval), activated by a collider-count gate
//! ([`GRID_COLLIDER_GATE`]). Below the gate every query is the exact
//! linear walk, bit-identical, forever (the charter's retained
//! small-workload fast path; every current demo world sits far below
//! it). At or above it, each query gathers candidate collider INDICES
//! from the cells under its conservative extent, RE-SORTS them to
//! (list, index) insertion order, and runs the unchanged narrow
//! phases over that ordered candidate slice. The grid is a LOOKUP
//! STRUCTURE ONLY — never iterated — so the hash order is invisible,
//! and because the candidate set is a conservative superset of the
//! colliders that can affect any accumulator update (every update
//! rule is strict-<, so a skipped collider is provably a no-op)
//! evaluated in the linear walk's own order, EVERY result and EVERY
//! mathematical tie is bit-identical between the two paths.
//! Out-of-domain inputs (non-finite, or beyond [`GRID_CELL_LIMIT`])
//! fail CLOSED onto the exact linear path — behavior never depends
//! on the broad phase being available. Bounded construction (Q01):
//! a collider covering more than [`GRID_OVERSIZED_CELL_LIMIT`] cells
//! is stored on the grid's oversized list — one resident entry,
//! consulted by every query, entering the same candidate re-sort —
//! so no finite in-domain collider can amplify the grid's memory or
//! build time (total resident entries are bounded by
//! (cap + 1) × collider count; the oversized-equivalence battery
//! pins the bit-identity and the insertion-order tie with oversized
//! colliders present).

use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::time::Duration;

use crate::collision::{RotatedCollider, segment_rotated_hit};
use crate::fingerprint::Fingerprinter;

/// Contact separation skin: swept targets clamp this far short of a
/// face, so resting contact (overlap of exactly [`SKIN`]) never counts
/// as a perpendicular-axis block and the nonpenetration invariant has a
/// strict margin to rest on.
pub const SKIN: f32 = 1e-4;

/// The tick-rate policy's lower bound (the sim driver's own
/// `.max(1)` precedent): rates below one tick per second are
/// degenerate and clamp up.
pub const MIN_TICK_HZ: u32 = 1;

/// The tick-rate policy's upper bound: one billion ticks per second
/// is the finest cadence nanosecond logical time can represent
/// (`period_ns` = 1 ns exactly; `dt` = 1 ns⁻¹, finite and positive —
/// 10⁹ is exactly representable in f32). Rates above this would make
/// the nanosecond period round to zero — a division-by-zero hazard
/// at every accumulator site — so they clamp down.
pub const MAX_TICK_HZ: u32 = 1_000_000_000;

/// The driver's total-logical-advancement domain (R4b): u64::MAX
/// nanoseconds (≈584 years at nanosecond resolution — beyond any
/// conceivable session). Within the domain the tick accounting is
/// exact in u64 for every representable input (`due == executed +
/// outstanding`, including a 2³²-tick obligation at the 1 GHz tick
/// bound — u32 arithmetic would wrap it to zero); an advance
/// that would exceed the remaining domain is clamped to the boundary
/// and REPORTED via [`DriveStep::domain_clamped`] — never silently
/// discarded.
pub const MAX_TOTAL_ADVANCE_NS: u128 = u64::MAX as u128;

/// The static broad phase's collider-count gate (P2 L5): worlds whose
/// TOTAL collider count (the axis-aligned list plus the rotated list
/// — the population every linear query walks) is below this take the
/// exact existing linear paths, bit-identical, forever; at or above
/// it the persistent uniform grid serves every query arm its
/// candidates. 128 is the design's initial gate
/// (docs/engineering-excellence/designs/P2-simulation-spatial.md
/// §2.2), a measurement-pinned placeholder, not fashion: the E-8
/// locomotion isolate shows collision queries are already ~½ the
/// near lane's cost at 1000 movers against a tiny world, so the
/// one-time static build amortizes over the per-tick per-mover query
/// load at far smaller collider counts than a single-query world
/// would — while the E-8 census/tick numbers show the linear scan is
/// the cost/benefit optimum at every current demo world (tens of
/// colliders). The grid exists for the scaling slope the flat E-8
/// tick line hides.
pub const GRID_COLLIDER_GATE: usize = 128;

/// The uniform grid's cell edge (m) — the design's "cell size ~ query
/// extent" derivation over the static family's extent census: the
/// per-tick mover queries (the hot family: player and NPC boxes,
/// extents ~0.6–2.4 m including one tick's travel and the [`SKIN`]
/// margin) sit at the upper end, and every other family-S extent
/// (reach 2.5 m, camera pull-in ~4.5 m) is within the ~one decade
/// the design records. A power of two, so the
/// `coordinate / GRID_CELL_SIZE` division is EXACT in f32 (no
/// rounding in the cell mapping — the determinism discipline). The
/// value is a cost knob only: the grid is conservative for ANY cell
/// size, because insertion covers each collider's full world AABB
/// and every query walks the full conservative extent of its own
/// geometry.
pub const GRID_CELL_SIZE: f32 = 2.0;

/// The cell-index domain bound: |cell| < 2²¹ on every axis (the
/// design's §2.2.6 numerical-domain contract, mirroring `perceive`'s
/// fail-closed radius gate). With [`GRID_CELL_SIZE`] this admits
/// world coordinates out to ±4,194,304 m — far beyond any
/// region-local world. A collider whose AABB is non-finite or maps
/// outside this domain poisons the whole broad phase (fail-closed):
/// the world keeps the exact linear paths, so behavior is
/// bit-identical by fallback rather than silently degrading. A QUERY
/// extent outside the domain fails the same way, per query.
pub const GRID_CELL_LIMIT: i32 = 1 << 21;

/// The cell-walk crossover factor (the DDA stand-in): when a query's
/// conservative extent spans more than this many cells PER COLLIDER
/// in the world, the cell walk alone outcosts the linear path's
/// per-collider evaluations (the per-item constants are comparable —
/// one hash lookup vs one slab/SAT test), so the query takes the
/// exact linear path. This is the design's own §2.6 waste trigger
/// number ("> ~4× the ray's true cell crossings"), applied per query
/// against the measured population instead of per world — the
/// documented class it keeps linear is the LONG DIAGONAL RAY over a
/// small world (a 120 m diagonal spans ~3.7 k cells: linear below
/// ~1 k colliders, grid above, crossover decided by this guard, not
/// by a fixed threshold). Bit-identity holds either way (the guard's
/// fallback IS the linear path); the DDA refinement replaces it if
/// measurement ever shows the guard engaging on a workload that
/// matters.
pub const GRID_CELL_WASTE_FACTOR: u64 = 4;

/// The per-collider cell-occupancy cap (Q01 — the bounded-construction
/// contract): a collider whose world AABB covers more than this many
/// grid cells is OVERSIZED. It is stored on the grid's dedicated
/// oversized list as ONE resident entry — never amplified into
/// per-cell entries — and EVERY query consults that list unconditionally,
/// so conservative correctness is preserved exactly (an oversized
/// collider is always a candidate, the same re-sort-to-insertion-order
/// discipline applies, and results stay bit-identical to the linear
/// path; the oversized-equivalence battery pins this, order ties
/// included).
///
/// THE BOUND this cap establishes: total resident grid entries ≤
/// ( [`GRID_OVERSIZED_CELL_LIMIT`] + 1 ) × total collider count —
/// each in-cells collider contributes at most the cap's worth of
/// entries, each oversized collider exactly one. Without it, a single
/// finite, IN-DOMAIN collider (e.g. one spanning ±4,000,000 m on
/// every axis — well inside the [`GRID_CELL_LIMIT`] coordinate
/// domain) covers ~6.4×10¹⁹ cells and the insertion loop amplifies
/// one collider into more cell entries than could ever be allocated:
/// the amplification Q01 names. The cap does NOT replace the
/// fail-closed domain rules: a collider that is non-finite or maps
/// outside the cell domain still poisons the broad phase (the linear
/// arms consult raw NaN faces, so no grid can conservatively cover
/// them); only a collider that is finite, in-domain, AND beyond the
/// cap becomes oversized.
///
/// THE VALUE (a cost knob, like [`GRID_CELL_SIZE`]): the shipped and
/// fixture geometry census tops out at 3×3×3 = 27 covered cells (a
/// 3 m footprint, 4 m tall block straddling cell boundaries at the
/// 2 m cell size); 64 admits more than double that margin, so no
/// real collider changes behavior. Beyond the cap a collider is
/// geographically structural — an arena wall, a terrain slab — the
/// class the oversized list exists for. The trade the knob sets: an
/// oversized collider costs every query one extra candidate
/// evaluation (the list is universally consulted), an in-cells
/// collider costs memory and build work proportional to its coverage.
/// 64 puts that trade where a collider must span roughly 8× the hot
/// query extents (the 0.6–2.4 m mover family) before it becomes
/// universally consulted instead of cell-localized.
pub const GRID_OVERSIZED_CELL_LIMIT: u64 = 64;

/// Movement intent for one tick — the application maps its input
/// snapshots onto this; the engine never sees keys or devices.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MoveIntent {
    /// +1 forward, −1 backward (W/S axes cancel at the shell).
    pub forward: i8,
    /// +1 right, −1 left (D/A axes cancel at the shell).
    pub strafe: i8,
    /// A fresh jump press this tick (edge-triggered; holding is not a
    /// new press).
    pub jump_pressed: bool,
    pub sprint: bool,
    /// Look delta for this tick in radians: (yaw, pitch).
    pub look: (f32, f32),
}

/// Player mechanics parameters — policy values supplied by the
/// application layer (the engine/game boundary).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerConfig {
    /// Horizontal speed cap when walking (m/s).
    pub walk_speed: f32,
    /// Horizontal speed cap when sprinting (m/s).
    pub sprint_speed: f32,
    /// Downward acceleration (m/s²).
    pub gravity: f32,
    /// Jump impulse: the exact upward velocity a grounded jump sets.
    pub jump_speed: f32,
    /// Maximum downward speed (terminal velocity clamp).
    pub terminal_fall: f32,
    /// Player AABB height (m).
    pub height: f32,
    /// Player AABB half-extent in x and z (m).
    pub radius: f32,
    /// First-person eye height above the feet (m).
    pub eye_height: f32,
    /// Third-person camera distance behind the eye (m).
    pub tp_distance: f32,
    /// Third-person camera lift above the eye (m).
    pub tp_lift: f32,
    /// The TP camera's minimum distance from the eye under
    /// obstruction pull-in (m) — the floor below which the camera
    /// never draws in (C2-P1; a documented residual: at the floor the
    /// camera may still sit inside geometry wedged against the eye).
    pub tp_min_distance: f32,
    /// Pitch clamp (radians, symmetric).
    pub pitch_limit: f32,
    /// The grounded step-up height (m): obstacles at or below this
    /// height are climbed without jumping (C2-P1). Zero (the default)
    /// DISABLES the mechanic, preserving the Cycle-1 clamp behavior
    /// bit-exactly — the frozen player trace's guarantee.
    pub step_height: f32,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        PlayerConfig {
            walk_speed: 4.0,
            sprint_speed: 12.0,
            gravity: 25.0,
            jump_speed: 8.0,
            terminal_fall: 50.0,
            height: 1.7,
            radius: 0.35,
            eye_height: 1.6,
            tp_distance: 4.5,
            tp_lift: 1.2,
            tp_min_distance: 1.2,
            pitch_limit: 1.54,
            step_height: 0.0,
        }
    }
}

/// One axis-aligned static collider. `min`/`max` are the box corners.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StaticCollider {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl StaticCollider {
    /// A box from a center-bottom base, a full footprint width, and a
    /// height — the same parameters `scene.rs`'s cube builders use, so
    /// the collidable world can mirror the visible one box for box.
    pub fn from_base(base: [f32; 3], footprint: f32, height: f32) -> StaticCollider {
        let half = footprint / 2.0;
        StaticCollider {
            min: [base[0] - half, base[1], base[2] - half],
            max: [base[0] + half, base[1] + height, base[2] + half],
        }
    }
}

/// The static collision world: axis-aligned boxes, yaw-rotated boxes
/// (Cycle 4 C — see [`crate::collision`]), a ground plane, optional
/// bounds. This is the reusable query contract — a future physics
/// backend implements the same queries.
///
/// The static broad phase (P2 L5): `grid` holds the persistent
/// uniform grid over both collider lists, activated when the total
/// collider count reaches [`GRID_COLLIDER_GATE`]. It is maintained
/// incrementally by the `with_*` builders (the write-once analysis:
/// there is no post-construction collider-mutation API — colliders
/// are only ever pushed, never removed or edited, and the game
/// layer's `set_world` swaps whole worlds — so builder-time
/// maintenance keeps the grid permanently consistent with the lists,
/// with no dirty tracking and no query-time work). See the module
/// docs for the bit-identity argument.
#[derive(Debug, Clone, Default)]
pub struct CollisionWorld {
    colliders: Vec<StaticCollider>,
    rotated: Vec<RotatedCollider>,
    bounds: Option<StaticCollider>,
    ground_y: f32,
    /// The static broad phase. `Off` below the gate (and for the
    /// empty default), `Grid` once activated, `Unusable` when a
    /// collider's AABB is non-finite or outside the
    /// [`GRID_CELL_LIMIT`] domain (permanently — the poisoning
    /// collider never leaves).
    grid: BroadPhase,
}

/// The static broad phase state. The grid is a LOOKUP STRUCTURE ONLY
/// — it is never iterated: every query walks the cells under its own
/// conservative extent in canonical row-major order, collects the
/// candidate entries, and re-sorts them to (list, index) collider
/// insertion order before the unchanged narrow phases run. The
/// re-sorted candidate sequence is then exactly the linear walk's
/// iteration order restricted to a conservative SUPERSET of the
/// colliders that can affect the result — which is why every result
/// and every tie is bit-identical to the linear path (the P2
/// design's determinism trick; the skipped colliders are provable
/// no-ops because every accumulator update rule is strict-<).
#[derive(Debug, Clone, Default)]
enum BroadPhase {
    /// Below [`GRID_COLLIDER_GATE`] (or the empty default): every
    /// query takes the exact existing linear path.
    #[default]
    Off,
    /// The persistent uniform grid over both collider lists.
    Grid(StaticGrid),
    /// Activation was attempted but a collider's AABB is non-finite
    /// or outside the [`GRID_CELL_LIMIT`] domain — permanently linear
    /// (fail-closed; colliders never leave, so the poisoning AABB
    /// never leaves either).
    Unusable,
}

/// A trivially-cheap deterministic hasher for cell keys (FNV-1a over
/// the key bytes). The grid is lookup-only — never iterated — so the
/// hash choice is behavior-invisible by construction; it exists
/// purely to keep per-cell lookups cheap (SipHash's DoS resistance
/// buys nothing over engine-authored integer cell keys).
#[derive(Clone, Default)]
struct CellHasher(u64);

impl CellHasher {
    /// The FNV-1a offset basis.
    const BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    /// The FNV-1a prime.
    const PRIME: u64 = 0x0000_0100_0000_01b3;
}

impl Hasher for CellHasher {
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

/// Builds [`CellHasher`]s (see its docs for why the default hasher is
/// replaced).
#[derive(Clone, Default)]
struct CellHasherBuilder;

impl BuildHasher for CellHasherBuilder {
    type Hasher = CellHasher;

    fn build_hasher(&self) -> CellHasher {
        CellHasher(CellHasher::BASIS)
    }
}

/// The uniform grid itself: OCCUPIED cells only (a hash grid — no
/// dense world-space allocation, the galaxy rule), each carrying the
/// collider entries inserted into it, in insertion order (push, never
/// sorted — the candidate re-sort relies only on the indices, but
/// keeping the per-cell lists in insertion order makes the
/// construction itself canonical), plus the OVERSIZED list (Q01's
/// bounded construction: colliders covering more than
/// [`GRID_OVERSIZED_CELL_LIMIT`] cells, one resident entry each,
/// consulted by every query).
#[derive(Debug, Clone, Default)]
struct StaticGrid {
    /// Cell key = (floor(x/s), floor(y/s), floor(z/s)) with
    /// s = [`GRID_CELL_SIZE`]; value = (is_rotated, index into that
    /// list).
    cells: HashMap<[i32; 3], Vec<(bool, u32)>, CellHasherBuilder>,
    /// Colliders whose covered-cell count exceeds
    /// [`GRID_OVERSIZED_CELL_LIMIT`] (or overflows the checked
    /// product — a whole-domain collider): ONE entry each, in
    /// insertion order, universally consulted. Never deduplicated
    /// against the cells (an oversized collider is in NO cell), so
    /// the candidate dedup sees each oversized entry exactly once.
    oversized: Vec<(bool, u32)>,
}

/// The cell index of one coordinate: floor(coord /
/// [`GRID_CELL_SIZE`]). The division is exact (power-of-two divisor),
/// so the floor is exact for every finite input. `None` when the
/// coordinate is non-finite or the cell lies outside the
/// [`GRID_CELL_LIMIT`] domain — the caller's fail-closed signal.
fn grid_cell_of(coordinate: f32) -> Option<i32> {
    if !coordinate.is_finite() {
        return None;
    }
    let cell = (coordinate / GRID_CELL_SIZE).floor();
    // The f32 compare is exact (integer-valued cells, |cell| < 2^24
    // in-range, and the limit is exactly representable), so the i32
    // conversion below never saturates.
    if cell.abs() >= GRID_CELL_LIMIT as f32 {
        return None;
    }
    Some(cell as i32)
}

/// The inclusive cell span covering the (already sorted) interval
/// [lo, hi]: floor(lo/s)..=floor(hi/s). Inclusive on both ends, so a
/// collider whose face sits exactly ON a cell boundary is found by a
/// query that merely touches it — the boundary-tight class the
/// linear arms' `<=`/`>=` face conditions admit. `None` when
/// non-finite or out of domain.
fn grid_cell_span(lo: f32, hi: f32) -> Option<(i32, i32)> {
    let lo_cell = grid_cell_of(lo)?;
    let hi_cell = grid_cell_of(hi)?;
    Some((lo_cell, hi_cell))
}

/// The per-axis cell spans covering a (sorted-per-axis) query extent,
/// together with the total walked-cell count (checked multiply — an
/// absurd span fails closed onto the linear path). Finiteness is
/// validated on the RAW extent values BEFORE the per-axis sort —
/// `f32::min`/`f32::max` swallow NaN, which would silently shrink a
/// non-finite query onto the finite corner's cells while the linear
/// arms still consult the raw NaN coordinates.
fn grid_query_spans(min: [f32; 3], max: [f32; 3]) -> Option<([(i32, i32); 3], u64)> {
    let mut spans = [(0i32, 0i32); 3];
    let mut walked: u64 = 1;
    for axis in 0..3 {
        if !min[axis].is_finite() || !max[axis].is_finite() {
            return None;
        }
        let lo = min[axis].min(max[axis]);
        let hi = max[axis].max(min[axis]);
        let (lo_cell, hi_cell) = grid_cell_span(lo, hi)?;
        spans[axis] = (lo_cell, hi_cell);
        let count = (hi_cell as i64 - lo_cell as i64 + 1).max(0) as u64;
        walked = walked.checked_mul(count)?;
    }
    Some((spans, walked))
}

/// The full-grid build over both lists (the gate-crossing path and
/// the test forcing path share it). `None` when any collider's AABB
/// is non-finite or outside the cell domain (fail-closed — the world
/// keeps the exact linear paths).
fn build_static_grid(
    colliders: &[StaticCollider],
    rotated: &[RotatedCollider],
) -> Option<StaticGrid> {
    let mut grid = StaticGrid::default();
    for (index, col) in colliders.iter().enumerate() {
        if !grid.insert(false, index as u32, col.min, col.max) {
            return None;
        }
    }
    for (index, col) in rotated.iter().enumerate() {
        let aabb = col.world_aabb();
        if !grid.insert(true, index as u32, aabb.min, aabb.max) {
            return None;
        }
    }
    Some(grid)
}

impl StaticGrid {
    /// Inserts one collider's entry into every cell its world AABB
    /// covers — unless the coverage exceeds
    /// [`GRID_OVERSIZED_CELL_LIMIT`], in which case the collider goes
    /// to the OVERSIZED list as one entry (Q01's bounded
    /// construction: no finite in-domain collider can amplify the
    /// grid, because its resident contribution is capped at either
    /// its coverage or exactly one entry). The AABB is sorted PER
    /// AXIS first: the linear narrow phases consult INVERTED
    /// `StaticCollider` literals too (an inverted interval can still
    /// overlap a straddling query box), so the grid must cover the
    /// sorted span — the conservative enclosure of every face value
    /// any linear arm can read. Finiteness is validated on the RAW
    /// corners BEFORE the sort — `f32::min`/`f32::max` swallow NaN,
    /// which would silently map a non-finite collider onto the other
    /// corner's cells while the linear arms still consult its raw NaN
    /// faces. Returns `false` when the AABB is non-finite or outside
    /// the cell domain (the caller poisons the broad phase,
    /// fail-closed) — the domain rules win over the oversized list,
    /// because no cell span can conservatively cover a raw NaN face
    /// or an out-of-domain coordinate the linear arms still read.
    fn insert(&mut self, rotated: bool, index: u32, min: [f32; 3], max: [f32; 3]) -> bool {
        let mut spans = [(0i32, 0i32); 3];
        for axis in 0..3 {
            if !min[axis].is_finite() || !max[axis].is_finite() {
                return false;
            }
            let lo = min[axis].min(max[axis]);
            let hi = max[axis].max(min[axis]);
            let Some(span) = grid_cell_span(lo, hi) else {
                return false;
            };
            spans[axis] = span;
        }
        // The covered-cell count, checked (a whole-domain collider's
        // product exceeds u64 — itself proof it is over the cap).
        // Over-limit or overflow: ONE oversized entry, never the
        // per-cell amplification.
        let mut covered: u64 = 1;
        for span in &spans {
            let per_axis = (span.1 - span.0 + 1) as u64;
            match covered.checked_mul(per_axis) {
                Some(product) => covered = product,
                None => {
                    self.oversized.push((rotated, index));
                    return true;
                }
            }
        }
        if covered > GRID_OVERSIZED_CELL_LIMIT {
            self.oversized.push((rotated, index));
            return true;
        }
        let entry = (rotated, index);
        for cx in spans[0].0..=spans[0].1 {
            for cy in spans[1].0..=spans[1].1 {
                for cz in spans[2].0..=spans[2].1 {
                    self.cells.entry([cx, cy, cz]).or_default().push(entry);
                }
            }
        }
        true
    }

    /// The conservative candidate set for precomputed query cell
    /// spans: walk the covered cells row-major (the canonical order —
    /// the design's rule; any fixed order would do, since the re-sort
    /// restores the contract order), collect the entries PLUS the
    /// entire oversized list (Q01: every query consults it — an
    /// oversized collider can overlap any extent, so conservative
    /// correctness requires it as a candidate always), re-sort to
    /// (list, index) insertion order, dedup. Duplicates (a collider
    /// covering several walked cells) collapse here; they are
    /// provably no-ops anyway (every accumulator update is
    /// strict-<, so a re-evaluated collider never re-clamps), but
    /// removing them keeps the candidate walk honest. The oversized
    /// entries enter the SAME re-sort — they are (list, index) pairs
    /// like any other — so the candidate sequence stays exactly the
    /// linear walk's iteration order over a conservative superset:
    /// bit-identity holds with oversized colliders present, ties
    /// included (the oversized-equivalence battery pins the order tie
    /// explicitly).
    fn candidates(&self, spans: [(i32, i32); 3]) -> Vec<(bool, u32)> {
        let mut entries: Vec<(bool, u32)> = Vec::new();
        for cx in spans[0].0..=spans[0].1 {
            for cy in spans[1].0..=spans[1].1 {
                for cz in spans[2].0..=spans[2].1 {
                    if let Some(cell) = self.cells.get(&[cx, cy, cz]) {
                        entries.extend_from_slice(cell);
                    }
                }
            }
        }
        entries.extend_from_slice(&self.oversized);
        // bool orders false (the axis-aligned list) before true (the
        // rotated list): (list, index) == the linear walk's order.
        entries.sort_unstable();
        entries.dedup();
        entries
    }
}

impl CollisionWorld {
    /// A world whose floor sits at `ground_y`.
    pub fn new(ground_y: f32) -> CollisionWorld {
        CollisionWorld {
            colliders: Vec::new(),
            rotated: Vec::new(),
            bounds: None,
            ground_y,
            grid: BroadPhase::Off,
        }
    }

    /// Adds one box (see [`StaticCollider::from_base`]).
    pub fn with_box(mut self, base: [f32; 3], footprint: f32, height: f32) -> CollisionWorld {
        self.colliders
            .push(StaticCollider::from_base(base, footprint, height));
        self.grid_after_push(false);
        self
    }

    /// Adds movement bounds: the player box stays inside this region,
    /// clamped at the faces with outward velocity zeroed.
    pub fn with_bounds(mut self, min: [f32; 3], max: [f32; 3]) -> CollisionWorld {
        self.bounds = Some(StaticCollider { min, max });
        self
    }

    /// Adds a collider in general (rectangular) form — walls and other
    /// non-square geometry the square-footprint builder cannot express.
    pub fn with_collider(mut self, collider: StaticCollider) -> CollisionWorld {
        self.colliders.push(collider);
        self.grid_after_push(false);
        self
    }

    /// Adds one yaw-rotated box collider (Cycle 4 C — the
    /// [`crate::collision`] representation). Every geometry query is
    /// rotated-aware: the axis-aligned list is consulted first (its
    /// behavior byte-identical to the pre-rotation engine), then the
    /// rotated list through the exact narrow phase. A `None` (a
    /// collider refused as degenerate/non-finite by
    /// [`RotatedCollider::new`]'s validation) is skipped: world
    /// literals should assert construction at their own site so a
    /// refused cover piece is never silently missing.
    pub fn with_rotated_collider(mut self, collider: Option<RotatedCollider>) -> CollisionWorld {
        if let Some(collider) = collider {
            self.rotated.push(collider);
            self.grid_after_push(true);
        }
        self
    }

    /// The broad-phase maintenance hook every collider push runs (the
    /// write-once analysis: colliders are added only through the
    /// consuming builders — there is no post-construction mutation
    /// API, and the game layer's world swap replaces whole worlds —
    /// so builder-time maintenance keeps the grid permanently
    /// consistent with the lists, with no dirty tracking and no
    /// query-time work). Below the gate this is a count check only;
    /// AT the gate it builds the full grid once; past the gate each
    /// push inserts just its own collider's covered cells.
    fn grid_after_push(&mut self, pushed_rotated: bool) {
        let total = self.colliders.len() + self.rotated.len();
        // The pushed collider's entry data, read before any grid
        // borrow (the push already happened).
        let pushed = if pushed_rotated {
            let index = self.rotated.len() - 1;
            let aabb = self.rotated[index].world_aabb();
            (true, index as u32, aabb.min, aabb.max)
        } else {
            let index = self.colliders.len() - 1;
            let col = self.colliders[index];
            (false, index as u32, col.min, col.max)
        };
        // mem::take moves the state out (Off/Unusable are free to
        // move; the grid is one HashMap move) so the arms can assign
        // the successor state without fighting the match borrow.
        match std::mem::take(&mut self.grid) {
            BroadPhase::Grid(mut grid) => {
                let consistent = grid.insert(pushed.0, pushed.1, pushed.2, pushed.3);
                self.grid = if consistent {
                    BroadPhase::Grid(grid)
                } else {
                    // Poisoned by the pushed collider: permanently
                    // linear (fail-closed).
                    BroadPhase::Unusable
                };
            }
            BroadPhase::Off if total >= GRID_COLLIDER_GATE => {
                // The gate crossing: one full build over both lists.
                self.grid = match build_static_grid(&self.colliders, &self.rotated) {
                    Some(grid) => BroadPhase::Grid(grid),
                    None => BroadPhase::Unusable,
                };
            }
            state => self.grid = state,
        }
    }

    /// The broad-phase candidates for a raw box query extent (no
    /// expansion — the box-query arms' participation conditions all
    /// imply strict overlap of the sorted intervals, which implies
    /// shared cells). `None` for below-gate/poisoned worlds, for
    /// out-of-domain extents, and when the [`GRID_CELL_WASTE_FACTOR`]
    /// crossover guard fires (the cell walk would outcost the linear
    /// narrow phases) — the exact linear path in every `None` case.
    fn grid_candidates(
        &self,
        extent_min: [f32; 3],
        extent_max: [f32; 3],
    ) -> Option<Vec<(bool, u32)>> {
        let BroadPhase::Grid(grid) = &self.grid else {
            return None;
        };
        let (spans, walked) = grid_query_spans(extent_min, extent_max)?;
        // The crossover guard (the DDA stand-in, documented on
        // GRID_CELL_WASTE_FACTOR): more cells walked than 4 per
        // collider means the walk alone outcosts the linear
        // per-collider evaluations — take the exact linear path.
        let population = (self.colliders.len() + self.rotated.len()) as u64;
        if walked > GRID_CELL_WASTE_FACTOR * population {
            return None;
        }
        Some(grid.candidates(spans))
    }

    /// The broad-phase candidates for one axis sweep: the SWEPT box
    /// (the union of the box and its displaced self, per-axis sorted)
    /// expanded by [`SKIN`] on the moving axis. The SKIN expansion is
    /// required and sufficient: the axis-aligned arm's clamp faces
    /// can sit up to SKIN past the swept extent (`face < allowed`
    /// with `allowed ≤ delta` bounds the face within
    /// SKIN of the sweep's leading edge — the tiny back-push class),
    /// and the rotated arm's own pretest (the raw swept box,
    /// touch-inclusive) is covered by the larger extent.
    /// Over-expansion is harmless — conservativeness only ever adds
    /// candidates the narrow phases then reject exactly. Non-finite
    /// deltas take the exact linear path (their own NaN handling), as
    /// does ANY non-finite player coordinate: a NaN on the moving
    /// axis makes the rotated arm's SAT arithmetic emit the ±SKIN
    /// resting-contact face from every pretest-passing collider (NaN
    /// comparisons never gate) — a clamp pattern no finite cell span
    /// can cover, so the raw check runs BEFORE the per-axis min/max
    /// (which swallow NaN).
    fn grid_candidates_for_sweep(
        &self,
        player_min: [f32; 3],
        player_max: [f32; 3],
        axis: usize,
        delta: f32,
    ) -> Option<Vec<(bool, u32)>> {
        if !delta.is_finite() {
            return None;
        }
        if (0..3).any(|a| !player_min[a].is_finite() || !player_max[a].is_finite()) {
            return None;
        }
        let mut extent_min = player_min;
        let mut extent_max = player_max;
        let lo = player_min[axis].min(player_max[axis]);
        let hi = player_max[axis].max(player_min[axis]);
        extent_min[axis] = lo.min(lo + delta) - SKIN;
        extent_max[axis] = hi.max(hi + delta) + SKIN;
        self.grid_candidates(extent_min, extent_max)
    }

    /// The broad-phase candidates for a segment query: the segment's
    /// raw AABB expanded by 2·`margin` on every axis. The expansion
    /// must cover the narrow phases' own collider expansion — the
    /// axis-aligned arm expands each box by exactly `margin` per
    /// world axis, and the rotated arm's LOCAL expansion projects to
    /// a horizontal world reach of `margin·(|cos yaw| + |sin yaw|)`
    /// ≤ `margin·√2` (the C4-R1 enclosure discipline); 2·margin (an
    /// exact power-of-two scale) bounds both. Negative or non-finite
    /// margins take the exact linear path — the validated broad-phase
    /// domain — as does any extent that lands outside the cell
    /// domain (e.g. a far endpoint).
    fn grid_candidates_for_segment(
        &self,
        from: [f32; 3],
        to: [f32; 3],
        margin: f32,
    ) -> Option<Vec<(bool, u32)>> {
        if !(margin.is_finite() && margin >= 0.0) {
            return None;
        }
        // Raw-endpoint finiteness BEFORE the per-axis min/max (which
        // swallow NaN — grid_query_spans's own raw check would then
        // never see it): a NaN endpoint makes the linear slab IGNORE
        // that axis (an infinite-slab hit pattern), which no finite
        // cell span can cover — the exact linear path serves it.
        if (0..3).any(|a| !from[a].is_finite() || !to[a].is_finite()) {
            return None;
        }
        let expand = 2.0 * margin;
        let mut extent_min = [0.0f32; 3];
        let mut extent_max = [0.0f32; 3];
        for axis in 0..3 {
            extent_min[axis] = from[axis].min(to[axis]) - expand;
            extent_max[axis] = from[axis].max(to[axis]) + expand;
        }
        self.grid_candidates(extent_min, extent_max)
    }

    /// The rotated colliders (empty in every pre-Cycle-4 world — the
    /// byte-identity guarantee: the empty list takes the exact
    /// existing query paths).
    pub fn rotated(&self) -> &[RotatedCollider] {
        &self.rotated
    }

    pub fn colliders(&self) -> &[StaticCollider] {
        &self.colliders
    }

    pub fn bounds(&self) -> Option<&StaticCollider> {
        self.bounds.as_ref()
    }

    pub fn ground_y(&self) -> f32 {
        self.ground_y
    }

    /// Whether the given box strictly overlaps any collider (the
    /// nonpenetration check; contact at the [`SKIN`] margin is clear).
    /// The axis-aligned colliders are tested first, then the rotated
    /// colliders through the exact 2D separating-axis narrow phase.
    /// At or above [`GRID_COLLIDER_GATE`] the broad phase supplies the
    /// candidates (a conservative superset re-sorted to insertion
    /// order — bit-identical results; see the module docs).
    pub fn box_overlaps(&self, min: [f32; 3], max: [f32; 3]) -> bool {
        if let Some(candidates) = self.grid_candidates(min, max) {
            for (rotated, index) in candidates {
                let overlaps = if rotated {
                    self.rotated[index as usize].overlaps_box(min, max)
                } else {
                    let col = &self.colliders[index as usize];
                    (0..3).all(|a| min[a] < col.max[a] && max[a] > col.min[a])
                };
                if overlaps {
                    return true;
                }
            }
            false
        } else {
            self.colliders
                .iter()
                .any(|col| (0..3).all(|a| min[a] < col.max[a] && max[a] > col.min[a]))
                || self.rotated.iter().any(|col| col.overlaps_box(min, max))
        }
    }

    /// Exact segment-vs-world intersection (C2-P1, challenge F2's
    /// slab form): the smallest `t` in `[0, 1]` at which the segment
    /// `from → to` enters any collider, each expanded by `margin`.
    /// Pure arithmetic per collider (the ray–AABB slab method) — no
    /// sampling, so no aliasing: a wall thinner than any sample step
    /// still registers exactly. `Some(0.0)` when the segment starts
    /// inside a collider; `None` when the segment misses every
    /// collider. The ground plane and bounds are NOT consulted (this
    /// is a line-of-sight/pull-in query over the box world). At or
    /// above [`GRID_COLLIDER_GATE`] the broad phase supplies the
    /// candidates (bit-identical results; see the module docs) — for
    /// segments the conservative extent is the segment's AABB
    /// expanded by 2·margin (the C4-R1 enclosure bound), so a LONG
    /// DIAGONAL ray spans many cells; the [`GRID_CELL_WASTE_FACTOR`]
    /// crossover guard keeps such rays on the exact linear path until
    /// the population makes the walk pay (the DDA stand-in — see the
    /// const's docs).
    pub fn segment_hit(&self, from: [f32; 3], to: [f32; 3], margin: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
        if let Some(candidates) = self.grid_candidates_for_segment(from, to, margin) {
            for (rotated, index) in candidates {
                let hit = if rotated {
                    segment_rotated_hit(from, to, &self.rotated[index as usize], margin)
                        .map(|(t, _)| t)
                } else {
                    segment_aabb_t(from, to, margin, &self.colliders[index as usize])
                };
                if let Some(t) = hit
                    && best.is_none_or(|best_t| t < best_t)
                {
                    best = Some(t);
                }
            }
        } else {
            for col in &self.colliders {
                if let Some(t) = segment_aabb_t(from, to, margin, col)
                    && best.is_none_or(|best_t| t < best_t)
                {
                    best = Some(t);
                }
            }
            for col in &self.rotated {
                let Some((t, _)) = segment_rotated_hit(from, to, col, margin) else {
                    continue;
                };
                if best.is_none_or(|best_t| t < best_t) {
                    best = Some(t);
                }
            }
        }
        best
    }

    /// Swept movement along one axis: the largest displacement not
    /// exceeding `delta` that keeps the player box out of every
    /// collider (and above the ground plane for downward moves), plus
    /// whether anything clamped it. Perpendicular axes use a [`SKIN`]
    /// overlap margin, so resting contact never blocks movement along
    /// the moving axis. The clamp targets sit [`SKIN`] short of each
    /// face, which is what makes tunneling impossible at any speed:
    /// the box stops before crossing, it never passes through. At or
    /// above [`GRID_COLLIDER_GATE`] the broad phase supplies the
    /// candidates (bit-identical results; see the module docs).
    pub fn sweep_axis(
        &self,
        player_min: [f32; 3],
        player_max: [f32; 3],
        axis: usize,
        delta: f32,
    ) -> (f32, bool) {
        if delta == 0.0 {
            return (0.0, false);
        }
        let mut allowed = delta;
        let mut hit = false;
        if axis == 1 && delta < 0.0 {
            let floor = self.ground_y - player_min[1];
            if floor > allowed {
                allowed = floor;
                hit = true;
            }
        }
        if let Some(candidates) =
            self.grid_candidates_for_sweep(player_min, player_max, axis, delta)
        {
            for (rotated, index) in candidates {
                let face = if rotated {
                    self.rotated[index as usize].sweep_axis_box(player_min, player_max, axis, delta)
                } else {
                    sweep_aabb_face(
                        player_min,
                        player_max,
                        axis,
                        delta,
                        &self.colliders[index as usize],
                    )
                };
                if let Some(face) = face {
                    apply_sweep_face(&mut allowed, &mut hit, face, delta);
                }
            }
        } else {
            for col in &self.colliders {
                if let Some(face) = sweep_aabb_face(player_min, player_max, axis, delta, col) {
                    apply_sweep_face(&mut allowed, &mut hit, face, delta);
                }
            }
            for col in &self.rotated {
                if let Some(face) = col.sweep_axis_box(player_min, player_max, axis, delta) {
                    apply_sweep_face(&mut allowed, &mut hit, face, delta);
                }
            }
        }
        (allowed, hit)
    }

    /// Minimal-axis depenetration: if the box overlaps a collider, the
    /// smallest push that clears it — to the NEAR face (never through
    /// to the far side), [`SKIN`] short of it. Deterministic: candidate
    /// magnitudes compare with pure +/− arithmetic (cross-platform
    /// stable) and the first strictly-smaller candidate wins, so
    /// mathematically equal pushes resolve by iteration order —
    /// identically on every run and platform. Rotated colliders add
    /// their face-normal candidates after the axis-aligned ones (a
    /// mathematical tie keeps the axis-aligned push — the stable
    /// first-wins order). At or above [`GRID_COLLIDER_GATE`] the
    /// broad phase supplies the candidates, re-sorted to this exact
    /// iteration order (bit-identical results, ties included; see the
    /// module docs).
    pub fn depenetration(&self, player_min: [f32; 3], player_max: [f32; 3]) -> Option<[f32; 3]> {
        let mut best: Option<([f32; 3], f32)> = None;
        if let Some(candidates) = self.grid_candidates(player_min, player_max) {
            for (rotated, index) in candidates {
                let candidate = if rotated {
                    self.rotated[index as usize].push_out_box(player_min, player_max, self.ground_y)
                } else {
                    depenetration_aabb_candidate(
                        player_min,
                        player_max,
                        self.ground_y,
                        &self.colliders[index as usize],
                    )
                };
                if let Some(candidate) = candidate {
                    keep_depenetration(&mut best, candidate);
                }
            }
        } else {
            for col in &self.colliders {
                if let Some(candidate) =
                    depenetration_aabb_candidate(player_min, player_max, self.ground_y, col)
                {
                    keep_depenetration(&mut best, candidate);
                }
            }
            for col in &self.rotated {
                if let Some(candidate) = col.push_out_box(player_min, player_max, self.ground_y) {
                    keep_depenetration(&mut best, candidate);
                }
            }
        }
        best.map(|(vector, _)| vector)
    }

    /// Clamps the feet-center position so the player box stays inside
    /// the bounds; returns which axes clamped (outward velocity is the
    /// caller's to zero — the boundary behavior).
    pub fn clamp_to_bounds(&self, position: &mut [f32; 3], config: &PlayerConfig) -> [bool; 3] {
        let Some(bounds) = &self.bounds else {
            return [false; 3];
        };
        let mut clamped = [false; 3];
        for axis in [0, 2] {
            let lo = bounds.min[axis] + config.radius;
            let hi = bounds.max[axis] - config.radius;
            if position[axis] < lo {
                position[axis] = lo;
                clamped[axis] = true;
            }
            if position[axis] > hi {
                position[axis] = hi;
                clamped[axis] = true;
            }
        }
        if position[1] < bounds.min[1] {
            position[1] = bounds.min[1];
            clamped[1] = true;
        }
        if position[1] > bounds.max[1] - config.height {
            position[1] = bounds.max[1] - config.height;
            clamped[1] = true;
        }
        clamped
    }
}

/// The exact ray–AABB slab entry time of ONE axis-aligned collider —
/// the per-collider half of [`CollisionWorld::segment_hit`]'s linear
/// walk, shared verbatim by the grid path (single source of truth:
/// both paths run this exact arithmetic, so the only possible
/// difference between them is WHICH colliders run it, in what order —
/// and the broad phase's candidate order is the linear order over a
/// conservative superset). Returns the smallest `t` in `[0, 1]` at
/// which the segment enters the collider expanded by `margin`, or
/// `None` on a miss; `Some(0.0)` for a segment starting inside.
fn segment_aabb_t(from: [f32; 3], to: [f32; 3], margin: f32, col: &StaticCollider) -> Option<f32> {
    let direction = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let mut t_enter = 0.0f32;
    let mut t_exit = 1.0f32;
    let mut missed = false;
    for axis in 0..3 {
        let lo = col.min[axis] - margin;
        let hi = col.max[axis] + margin;
        let d = direction[axis];
        let origin = from[axis];
        if d == 0.0 {
            if origin < lo || origin > hi {
                missed = true;
                break;
            }
        } else {
            let mut t1 = (lo - origin) / d;
            let mut t2 = (hi - origin) / d;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            t_enter = t_enter.max(t1);
            t_exit = t_exit.min(t2);
            if t_enter > t_exit {
                missed = true;
                break;
            }
        }
    }
    if !missed && t_exit >= 0.0 && t_enter <= 1.0 {
        Some(t_enter.max(0.0))
    } else {
        None
    }
}

/// The per-collider half of [`CollisionWorld::sweep_axis`]'s
/// axis-aligned walk (shared verbatim by the grid path): the contact
/// face this collider would clamp the move to, or `None` when it
/// cannot clamp (the perpendicular [`SKIN`] gate, or a face on the
/// wrong side / beyond the move — the caller's sign comparison then
/// never sees it, exactly like the linear walk's arm conditions).
fn sweep_aabb_face(
    player_min: [f32; 3],
    player_max: [f32; 3],
    axis: usize,
    delta: f32,
    col: &StaticCollider,
) -> Option<f32> {
    for o in 0..3 {
        if o == axis {
            continue;
        }
        if player_min[o] >= col.max[o] - SKIN || player_max[o] <= col.min[o] + SKIN {
            return None;
        }
    }
    if delta > 0.0 && player_max[axis] <= col.min[axis] {
        Some(col.min[axis] - SKIN - player_max[axis])
    } else if delta < 0.0 && player_min[axis] >= col.max[axis] {
        Some(col.max[axis] + SKIN - player_min[axis])
    } else {
        None
    }
}

/// The sweep accumulator update — the linear walk's own rule, shared
/// by the grid path: strictly-better faces only, so mathematical ties
/// keep the earlier collider's clamp (the determinism rule; the
/// strict-< is also what makes duplicate candidates provable no-ops).
fn apply_sweep_face(allowed: &mut f32, hit: &mut bool, face: f32, delta: f32) {
    if delta > 0.0 {
        if face < *allowed {
            *allowed = face;
            *hit = true;
        }
    } else if face > *allowed {
        *allowed = face;
        *hit = true;
    }
}

/// The per-collider half of [`CollisionWorld::depenetration`]'s
/// axis-aligned walk (shared verbatim by the grid path): this
/// collider's own minimal push over its three axes, under the
/// strictly-smaller rule. Reducing per collider FIRST is
/// order-isomorphic to the flat (collider, axis) candidate sequence —
/// the strict-< rule composes associatively along a fixed order, so
/// every tie resolves to the same push on both paths (pinned by the
/// equivalence battery's tie fixtures).
fn depenetration_aabb_candidate(
    player_min: [f32; 3],
    player_max: [f32; 3],
    ground_y: f32,
    col: &StaticCollider,
) -> Option<([f32; 3], f32)> {
    let overlapping = (0..3).all(|a| player_min[a] < col.max[a] && player_max[a] > col.min[a]);
    if !overlapping {
        return None;
    }
    let mut best: Option<([f32; 3], f32)> = None;
    for axis in 0..3 {
        // Exiting through the min face moves the player's MAX face to
        // it; through the max face, the player's MIN — the pushed-out
        // box ends fully clear of the collider, never merely
        // re-aligned with the near face.
        let push_neg = col.min[axis] - SKIN - player_max[axis];
        let push_pos = col.max[axis] + SKIN - player_min[axis];
        // The ground plane is solid: a downward push that would cross
        // it is not an escape (the ground clamp would push the player
        // straight back into the box), so the negative candidate is
        // invalid on the vertical axis.
        let neg_valid = !(axis == 1 && player_min[1] + push_neg < ground_y);
        let (push, magnitude) = if neg_valid && push_neg.abs() <= push_pos {
            (push_neg, push_neg.abs())
        } else {
            (push_pos, push_pos.abs())
        };
        let mut vector = [0.0f32; 3];
        vector[axis] = push;
        keep_depenetration(&mut best, (vector, magnitude));
    }
    best
}

/// The depenetration accumulator update — the linear walk's own rule,
/// shared by the grid path: strictly-smaller magnitudes win, so
/// mathematical ties keep the earlier candidate (deterministic on
/// every run and platform).
fn keep_depenetration(best: &mut Option<([f32; 3], f32)>, candidate: ([f32; 3], f32)) {
    if best.is_none_or(|(_, best_magnitude)| candidate.1 < best_magnitude) {
        *best = Some(candidate);
    }
}

/// The player's authoritative kinematic state. Independent of camera
/// placement: camera poses are DERIVED from this (see [`camera_pose`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerState {
    /// Feet-center position.
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub camera_mode: CameraMode,
}

impl PlayerState {
    pub fn spawn(position: [f32; 3]) -> PlayerState {
        PlayerState {
            position,
            velocity: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            camera_mode: CameraMode::FirstPerson,
        }
    }

    /// Toggles the camera mode — single-call semantics (one toggle per
    /// key press edge; the application's job). Player state is
    /// otherwise untouched: switching cameras never teleports or resets
    /// the player (the charter invariant, proven by test).
    pub fn toggle_camera_mode(&mut self) {
        self.camera_mode = match self.camera_mode {
            CameraMode::FirstPerson => CameraMode::ThirdPerson,
            CameraMode::ThirdPerson => CameraMode::FirstPerson,
        };
    }
}

/// Camera mode: which derived view [`camera_pose`] produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraMode {
    FirstPerson,
    ThirdPerson,
}

/// A derived camera pose — pure function of player state and config.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraPose {
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    /// Always zero in this prototype (the free-flight roll input stays
    /// with the debug camera; the player camera has no roll).
    pub roll: f32,
}

/// The look direction for yaw/pitch (Ry·Rx applied to −Z, matching the
/// application camera's convention: identity at zero angles).
pub fn look_direction(yaw: f32, pitch: f32) -> [f32; 3] {
    let (sin_yaw, cos_yaw) = (yaw.sin(), yaw.cos());
    let (sin_pitch, cos_pitch) = (pitch.sin(), pitch.cos());
    [-cos_pitch * sin_yaw, sin_pitch, -cos_pitch * cos_yaw]
}

/// The camera pose derived from the player state: first person puts
/// the eye at the player's eye height; third person pulls it back
/// along the look direction with a lift. The third-person camera has
/// NO collision handling (documented prototype limitation).
pub fn camera_pose(state: &PlayerState, config: &PlayerConfig) -> CameraPose {
    let eye = [
        state.position[0],
        state.position[1] + config.eye_height,
        state.position[2],
    ];
    match state.camera_mode {
        CameraMode::FirstPerson => CameraPose {
            position: eye,
            yaw: state.yaw,
            pitch: state.pitch,
            roll: 0.0,
        },
        CameraMode::ThirdPerson => {
            let forward = look_direction(state.yaw, state.pitch);
            CameraPose {
                position: [
                    eye[0] - forward[0] * config.tp_distance,
                    eye[1] - forward[1] * config.tp_distance + config.tp_lift,
                    eye[2] - forward[2] * config.tp_distance,
                ],
                yaw: state.yaw,
                pitch: state.pitch,
                roll: 0.0,
            }
        }
    }
}

/// The obstruction query's camera clearance margin (m): the
/// [`camera_pose_obstructed`] pull-in treats geometry as expanded by
/// this much, so the drawn-in camera does not kiss the wall it stopped
/// at.
pub const CAMERA_MARGIN: f32 = 0.15;

/// The pull-in backoff (m): the camera sits this far short of the
/// first face crossing.
pub const CAMERA_BACKOFF: f32 = 0.05;

/// The camera pose with third-person obstruction handling (C2-P1):
/// the desired pose from [`camera_pose`], pulled in along the
/// eye→camera segment to the first collider crossing — the EXACT
/// slab intersection ([`CollisionWorld::segment_hit`], challenge F2:
/// no sample aliasing; a wall thinner than any march step still pulls
/// in) — never closer to the eye than `tp_min_distance`. First person
/// returns the desired pose as-is. Known residual (documented): when
/// the eye itself is under geometry, the hit lands at t = 0 and the
/// camera sits at the min-distance floor — possibly inside that
/// geometry; corner peeks can still clip briefly. This is a bounded
/// pull-in, not full camera collision.
pub fn camera_pose_obstructed(
    state: &PlayerState,
    config: &PlayerConfig,
    world: &CollisionWorld,
) -> CameraPose {
    constrain_camera_pose(camera_pose(state, config), state, config, world)
}

/// Apply the player's camera collision policy to a caller's desired world pose.
/// Shoulder offsets and other view translations must precede this query so
/// the complete eye-to-camera segment is tested against the same geometry.
pub fn constrain_camera_pose(
    mut pose: CameraPose,
    state: &PlayerState,
    config: &PlayerConfig,
    world: &CollisionWorld,
) -> CameraPose {
    if state.camera_mode != CameraMode::ThirdPerson {
        return pose;
    }
    let eye = [
        state.position[0],
        state.position[1] + config.eye_height,
        state.position[2],
    ];
    let segment = [
        pose.position[0] - eye[0],
        pose.position[1] - eye[1],
        pose.position[2] - eye[2],
    ];
    let full_length =
        (segment[0] * segment[0] + segment[1] * segment[1] + segment[2] * segment[2]).sqrt();
    if full_length <= 0.0 || !full_length.is_finite() {
        return pose;
    }
    if let Some(t) = world.segment_hit(eye, pose.position, CAMERA_MARGIN) {
        let hit_length = t * full_length;
        let floor = config.tp_min_distance.min(full_length);
        let allowed = (hit_length - CAMERA_BACKOFF).max(floor).min(full_length);
        let scale = allowed / full_length;
        pose.position = [
            eye[0] + segment[0] * scale,
            eye[1] + segment[1] * scale,
            eye[2] + segment[2] * scale,
        ];
    }
    pose
}

/// Observable per-step outcome — the fail-visible surface for input
/// sanitization (F5): non-finite look deltas are zeroed AND counted;
/// invalid `dt`/config inputs and non-finite states are refused AND
/// flagged. Never silent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StepOutcome {
    pub jumped: bool,
    /// Ground contact regained this step (was airborne).
    pub landed: bool,
    pub hit_x: bool,
    pub hit_z: bool,
    /// Upward motion clamped by a collider underside.
    pub hit_ceiling: bool,
    /// Downward motion clamped by ground or a box top.
    pub hit_ground: bool,
    /// Non-finite look-delta components zeroed this step.
    pub sanitized_look: u32,
    /// The step was refused BEFORE any state mutation: the time step
    /// was non-finite or non-positive, or a config field was
    /// non-finite (the R4 input-validity contract — a valid state can
    /// never be silently turned non-finite by invalid inputs).
    pub refused_invalid_inputs: bool,
    /// The step was refused: the incoming state was non-finite and is
    /// left untouched for the caller to handle.
    pub refused_nonfinite_state: bool,
}

/// The player's AABB (feet-center position, config extents).
pub fn player_box(position: [f32; 3], config: &PlayerConfig) -> ([f32; 3], [f32; 3]) {
    let r = config.radius;
    (
        [position[0] - r, position[1], position[2] - r],
        [
            position[0] + r,
            position[1] + config.height,
            position[2] + r,
        ],
    )
}

fn state_is_finite(state: &PlayerState) -> bool {
    state
        .position
        .iter()
        .chain(state.velocity.iter())
        .all(|v| v.is_finite())
        && state.yaw.is_finite()
        && state.pitch.is_finite()
}

fn finite_or_zero(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

/// The config side of the R4/R4b input-validity contract: every
/// parameter finite, plus the semantic minimums the kinematic
/// implementation depends on — a POSITIVE `pitch_limit` (a negative
/// one inverts the pitch clamp's bounds, which panics; zero locks
/// pitch and is refused by the same positive-only policy) and
/// POSITIVE `height`/`radius` extents (nonpositive values invert or
/// degenerate the player box).
fn config_is_valid(config: &PlayerConfig) -> bool {
    config.walk_speed.is_finite()
        && config.sprint_speed.is_finite()
        && config.gravity.is_finite()
        && config.jump_speed.is_finite()
        && config.terminal_fall.is_finite()
        && config.height.is_finite()
        && config.radius.is_finite()
        && config.eye_height.is_finite()
        && config.tp_distance.is_finite()
        && config.tp_lift.is_finite()
        && config.pitch_limit.is_finite()
        && config.pitch_limit > 0.0
        && config.height > 0.0
        && config.radius > 0.0
        && config.tp_min_distance.is_finite()
        && config.tp_min_distance >= 0.0
        && config.step_height.is_finite()
        && config.step_height >= 0.0
}

/// The horizontal sweep's context for one step-up probe: the axis,
/// the intended displacement, the clamped result it produced, and the
/// grounded state snapshot from before the tick's vertical phase.
struct StepProbe {
    axis: usize,
    want: f32,
    clamped: f32,
    was_on_ground: bool,
}

/// The C2-P1 grounded step-up probe for one horizontal axis: when the
/// GROUNDED player's move is clamped and `step_height` > 0, retry the
/// same move one step-height higher — if the raised box is clear at
/// the current position (no low ceiling) and the raised move is
/// unblocked, take it and snap down onto the step (up to
/// step_height + [`SKIN`]). One probe per axis per tick (a multi-step
/// staircase climbs one box per contact — smooth at 60 Hz). Returns
/// `true` when the climb was taken; `false` leaves the clamped
/// behavior EXACTLY as Cycle 1 (and with `step_height` 0.0 the probe
/// never runs — the disabled path is bit-identical).
fn try_step_up(
    state: &mut PlayerState,
    config: &PlayerConfig,
    world: &CollisionWorld,
    probe: StepProbe,
    outcome: &mut StepOutcome,
) -> bool {
    let StepProbe {
        axis,
        want,
        clamped,
        was_on_ground,
    } = probe;
    if !was_on_ground || config.step_height <= 0.0 || want == 0.0 {
        return false;
    }
    let (min, max) = player_box(state.position, config);
    let mut raised_min = min;
    let mut raised_max = max;
    raised_min[1] += config.step_height;
    raised_max[1] += config.step_height;
    if world.box_overlaps(raised_min, raised_max) {
        return false; // a low ceiling over the current position
    }
    let (raised_move, raised_hit) = world.sweep_axis(raised_min, raised_max, axis, want);
    if raised_hit || raised_move.abs() <= clamped.abs() {
        return false; // the obstacle stands taller than the step
    }
    state.position[axis] += raised_move;
    state.position[1] += config.step_height;
    let (min, max) = player_box(state.position, config);
    let (down, down_hit) = world.sweep_axis(min, max, 1, -(config.step_height + SKIN));
    state.position[1] += down;
    if down_hit {
        state.on_ground = true;
        state.velocity[1] = 0.0;
        outcome.hit_ground = true;
    }
    true
}

/// One fixed-dt movement step. Order: refuse invalid inputs (a
/// non-finite or non-positive `dt`, or a config that is non-finite
/// or semantically invalid — nothing mutated, the outcome reports
/// it); refuse a non-finite
/// state; sanitize and apply look; set horizontal velocity from the
/// intent in the yaw frame (diagonal normalized to the speed cap);
/// gravity (terminal-clamped), then a grounded jump's exact impulse;
/// depenetrate; swept per-axis moves Y→X→Z with velocity zeroed on the
/// clamped axis; bounds clamp. `on_ground` is true exactly when the
/// downward move clamped this step — resting contact re-clamps every
/// tick, so the resting state is a stable fixed point.
pub fn step_player(
    state: &mut PlayerState,
    config: &PlayerConfig,
    world: &CollisionWorld,
    intent: &MoveIntent,
    dt: f32,
) -> StepOutcome {
    let mut outcome = StepOutcome::default();

    if !dt.is_finite() || dt <= 0.0 || !config_is_valid(config) {
        outcome.refused_invalid_inputs = true;
        return outcome;
    }

    if !state_is_finite(state) {
        outcome.refused_nonfinite_state = true;
        return outcome;
    }

    if !intent.look.0.is_finite() {
        outcome.sanitized_look += 1;
    }
    if !intent.look.1.is_finite() {
        outcome.sanitized_look += 1;
    }
    let look = (finite_or_zero(intent.look.0), finite_or_zero(intent.look.1));
    state.yaw += look.0;
    state.pitch = (state.pitch + look.1).clamp(-config.pitch_limit, config.pitch_limit);

    let forward_input = f32::from(intent.forward);
    let strafe_input = f32::from(intent.strafe);
    let speed = if intent.sprint {
        config.sprint_speed
    } else {
        config.walk_speed
    };
    let (sin_yaw, cos_yaw) = (state.yaw.sin(), state.yaw.cos());
    let mut dir_x = -sin_yaw * forward_input + cos_yaw * strafe_input;
    let mut dir_z = -cos_yaw * forward_input - sin_yaw * strafe_input;
    let magnitude_sq = dir_x * dir_x + dir_z * dir_z;
    if magnitude_sq > 1.0 {
        let scale = speed / magnitude_sq.sqrt();
        dir_x *= scale;
        dir_z *= scale;
    } else {
        dir_x *= speed;
        dir_z *= speed;
    }
    state.velocity[0] = dir_x;
    state.velocity[2] = dir_z;

    state.velocity[1] = (state.velocity[1] - config.gravity * dt).max(-config.terminal_fall);
    if intent.jump_pressed && state.on_ground {
        state.velocity[1] = config.jump_speed;
        outcome.jumped = true;
    }

    let (min, max) = player_box(state.position, config);
    if let Some(push) = world.depenetration(min, max) {
        for (position, component) in state.position.iter_mut().zip(push) {
            *position += component;
        }
    }

    let was_on_ground = state.on_ground;
    state.on_ground = false;

    let (min, max) = player_box(state.position, config);
    let (dy, hit_y) = world.sweep_axis(min, max, 1, state.velocity[1] * dt);
    state.position[1] += dy;
    if hit_y {
        if state.velocity[1] < 0.0 {
            state.on_ground = true;
            outcome.hit_ground = true;
            if !was_on_ground {
                outcome.landed = true;
            }
        } else {
            outcome.hit_ceiling = true;
        }
        state.velocity[1] = 0.0;
    }

    let (min, max) = player_box(state.position, config);
    let want_x = state.velocity[0] * dt;
    let (dx, hit_x) = world.sweep_axis(min, max, 0, want_x);
    let climbed_x = hit_x
        && try_step_up(
            state,
            config,
            world,
            StepProbe {
                axis: 0,
                want: want_x,
                clamped: dx,
                was_on_ground,
            },
            &mut outcome,
        );
    if !climbed_x {
        state.position[0] += dx;
        if hit_x {
            outcome.hit_x = true;
            state.velocity[0] = 0.0;
        }
    }

    let (min, max) = player_box(state.position, config);
    let want_z = state.velocity[2] * dt;
    let (dz, hit_z) = world.sweep_axis(min, max, 2, want_z);
    let climbed_z = hit_z
        && try_step_up(
            state,
            config,
            world,
            StepProbe {
                axis: 2,
                want: want_z,
                clamped: dz,
                was_on_ground,
            },
            &mut outcome,
        );
    if !climbed_z {
        state.position[2] += dz;
        if hit_z {
            outcome.hit_z = true;
            state.velocity[2] = 0.0;
        }
    }

    let clamped = world.clamp_to_bounds(&mut state.position, config);
    if clamped[0] {
        outcome.hit_x = true;
        state.velocity[0] = 0.0;
    }
    if clamped[1] {
        if state.velocity[1] < 0.0 {
            state.on_ground = true;
            outcome.hit_ground = true;
            if !was_on_ground {
                outcome.landed = true;
            }
        }
        state.velocity[1] = 0.0;
    }
    if clamped[2] {
        outcome.hit_z = true;
        state.velocity[2] = 0.0;
    }

    outcome
}

/// One recorded tick of the deterministic trace.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceEntry {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub on_ground: bool,
    pub camera_mode: CameraMode,
}

/// One `advance` call's outcome (the no-discard accounting view).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriveStep {
    pub executed_ticks: u32,
    /// Due ticks retained (not yet executed) — a reported obligation,
    /// never silently discarded; drains when catch-up capacity
    /// allows. u64 (R4b): exact across the whole tick-rate range —
    /// a 2³²-tick obligation at the 1 GHz tick bound accounts
    /// exactly (u32 accounting would wrap it to zero).
    pub outstanding_ticks: u64,
    /// The advance was clamped to the remaining
    /// [`MAX_TOTAL_ADVANCE_NS`] domain — reported, never silent.
    pub domain_clamped: bool,
}

/// Fixed-step movement driver — the SimDriver discipline applied to
/// the player: elapsed monotonic time decides WHEN ticks are due;
/// logical state ever sees only the fixed tick. `advance` is the
/// live-input path. Held movement applies to every tick in the frame.
/// Mouse displacement and jump presses accumulate until the next tick
/// and are consumed once. `run_headless`
/// is the scripted deterministic path: the intent is a function of the
/// logical tick index, so trace history is cadence-independent.
pub struct PlayerDriver {
    state: PlayerState,
    config: PlayerConfig,
    world: CollisionWorld,
    dt: f32,
    period_ns: u128,
    max_catchup_per_frame: u32,
    accumulator_ns: u128,
    pending_look: (f32, f32),
    pending_jump: bool,
    total_advanced_ns: u128,
    headless_ticks: u64,
    executed_ticks: u64,
    /// Steps refused by the input-validity contracts (R4b): counted
    /// and reported, never hidden as movement work. The tick itself
    /// is still consumed — the accounting invariant is time-based.
    refused_steps: u64,
    record_trace: bool,
    trace: Vec<TraceEntry>,
}

impl PlayerDriver {
    pub fn new(
        state: PlayerState,
        config: PlayerConfig,
        world: CollisionWorld,
        tick_hz: u32,
    ) -> PlayerDriver {
        Self::with_catchup(state, config, world, tick_hz, 8)
    }

    /// Constructs the driver under the documented tick-rate policy
    /// (R4): the rate is clamped into
    /// [`MIN_TICK_HZ`]..=[`MAX_TICK_HZ`] — the representable
    /// fixed-step range at nanosecond logical resolution. This
    /// guarantees `period_ns` ≥ 1 (never zero — the >1 GHz
    /// division-by-zero hazard is structurally closed) and `dt`
    /// finite, positive, ≥ 1 ns⁻¹. Valid rates (the demo's 60, the
    /// tests' 100) are unaffected.
    pub fn with_catchup(
        state: PlayerState,
        config: PlayerConfig,
        world: CollisionWorld,
        tick_hz: u32,
        max_catchup_per_frame: u32,
    ) -> PlayerDriver {
        let rate = tick_hz.clamp(MIN_TICK_HZ, MAX_TICK_HZ);
        PlayerDriver {
            state,
            config,
            world,
            dt: 1.0 / (rate as f32),
            period_ns: 1_000_000_000u128 / u128::from(rate),
            max_catchup_per_frame: max_catchup_per_frame.max(1),
            accumulator_ns: 0,
            pending_look: (0.0, 0.0),
            pending_jump: false,
            total_advanced_ns: 0,
            headless_ticks: 0,
            executed_ticks: 0,
            refused_steps: 0,
            record_trace: true,
            trace: Vec::new(),
        }
    }

    pub fn state(&self) -> &PlayerState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut PlayerState {
        &mut self.state
    }

    pub fn config(&self) -> &PlayerConfig {
        &self.config
    }

    pub fn world(&self) -> &CollisionWorld {
        &self.world
    }

    pub fn trace(&self) -> &[TraceEntry] {
        &self.trace
    }

    pub fn set_trace_recording(&mut self, on: bool) {
        self.record_trace = on;
    }

    /// Swaps the collision world (C2-P1): an explicit GAME-LAYER act —
    /// the demo owns world content, and opening a gate or spawning
    /// geometry replaces the whole static box set at a logical moment.
    /// Player state, config, the input contracts, and the tick
    /// accounting are untouched: the swap never teleports or resets
    /// the player (regression-pinned in the playable-world battery).
    pub fn set_world(&mut self, world: CollisionWorld) {
        self.world = world;
    }

    /// Discards unconsumed mouse and jump input when the window loses focus.
    pub fn clear_pending_input(&mut self) {
        self.pending_look = (0.0, 0.0);
        self.pending_jump = false;
    }

    /// Advances by elapsed monotonic time (frame-sampled live
    /// intent). R4b: the due-tick arithmetic is u128-wide with u64
    /// public accounting — exact across the whole tick-rate range
    /// (the 2³²-tick counterexample at the 1 GHz bound included;
    /// u32 narrowing would wrap it to zero) — and an advance
    /// beyond the remaining [`MAX_TOTAL_ADVANCE_NS`] domain is
    /// clamped to the boundary and REPORTED via `domain_clamped`.
    pub fn advance(&mut self, elapsed: Duration, intent: MoveIntent) -> DriveStep {
        self.advance_observed(elapsed, intent, |_state, _index| {})
    }

    /// Advances by elapsed monotonic time — IDENTICAL accounting to
    /// [`Self::advance`] (the accumulator's bounded catch-up, the
    /// domain clamp, the no-discard drain, the per-step input
    /// validity inside [`Self::tick`]) — with `after_step` invoked
    /// after EACH executed movement step, receiving the POST-STEP
    /// player state and the 1-based step count within this call.
    /// The fixed-step composition seam (the C3-R1 corrective):
    /// gameplay logic that must observe each logical tick's own
    /// resulting state interleaves HERE, between the driver's
    /// steps — never against the frame's final state. The observer
    /// receives a read-only state snapshot and cannot reach the
    /// driver, so the movement accounting is structurally
    /// untouched (a no-op observer makes this exactly `advance`).
    pub fn advance_observed(
        &mut self,
        elapsed: Duration,
        intent: MoveIntent,
        mut after_step: impl FnMut(&PlayerState, u64),
    ) -> DriveStep {
        let requested = elapsed.as_nanos();
        let remaining = MAX_TOTAL_ADVANCE_NS - self.total_advanced_ns;
        let granted = requested.min(remaining);
        let domain_clamped = granted < requested;
        self.total_advanced_ns += granted;
        self.accumulator_ns += granted;
        let due = self.accumulator_ns / self.period_ns;
        let executed = due.min(u128::from(self.max_catchup_per_frame));
        self.pending_look.0 += intent.look.0;
        self.pending_look.1 += intent.look.1;
        self.pending_jump |= intent.jump_pressed;
        for index in 0..executed {
            let tick_intent = MoveIntent {
                look: std::mem::take(&mut self.pending_look),
                jump_pressed: std::mem::take(&mut self.pending_jump),
                ..intent
            };
            self.tick(tick_intent);
            after_step(&self.state, index as u64 + 1);
        }
        self.accumulator_ns -= executed * self.period_ns;
        DriveStep {
            executed_ticks: executed as u32,
            outstanding_ticks: (self.accumulator_ns / self.period_ns) as u64,
            domain_clamped,
        }
    }

    /// Runs exactly `ticks` fixed ticks with scripted intents — the
    /// headless deterministic path (`intent_at` receives the global
    /// headless tick index).
    pub fn run_headless(&mut self, ticks: u64, intent_at: impl Fn(u64) -> MoveIntent) {
        for _ in 0..ticks {
            let intent = intent_at(self.headless_ticks);
            self.headless_ticks += 1;
            self.tick(intent);
        }
    }

    fn tick(&mut self, intent: MoveIntent) {
        let outcome = step_player(&mut self.state, &self.config, &self.world, &intent, self.dt);
        self.executed_ticks += 1;
        if outcome.refused_invalid_inputs || outcome.refused_nonfinite_state {
            self.refused_steps += 1;
        }
        if self.record_trace {
            self.trace.push(TraceEntry {
                position: self.state.position,
                velocity: self.state.velocity,
                on_ground: self.state.on_ground,
                camera_mode: self.state.camera_mode,
            });
        }
    }

    /// Ticks due from every source: elapsed-time advancement (executed
    /// or outstanding) plus headless execution. Saturating only beyond
    /// the documented [`MAX_TOTAL_ADVANCE_NS`] domain (u64::MAX ticks
    /// — never wraps).
    pub fn ticks_due_total(&self) -> u64 {
        ((self.total_advanced_ns / self.period_ns) as u64).saturating_add(self.headless_ticks)
    }

    pub fn executed_ticks_total(&self) -> u64 {
        self.executed_ticks
    }

    /// Due-but-unexecuted ticks right now — u64 (R4b): exact across
    /// the whole tick-rate range, never a wrapped u32.
    pub fn outstanding_ticks(&self) -> u64 {
        (self.accumulator_ns / self.period_ns) as u64
    }

    /// Steps refused by the input-validity contracts (R4b): explicit,
    /// bounded, never hidden as movement work.
    pub fn refused_steps(&self) -> u64 {
        self.refused_steps
    }

    /// FNV-1a over the recorded trace (bit patterns, domain
    /// `engine-core/player-trace/1`). Empty when trace recording is
    /// off.
    pub fn trace_fingerprint_hex(&self) -> String {
        let mut fp = Fingerprinter::new();
        fp.write_str("engine-core/player-trace/1");
        fp.write_usize(self.trace.len());
        for entry in &self.trace {
            for value in entry.position {
                fp.write_u32(value.to_bits());
            }
            for value in entry.velocity {
                fp.write_u32(value.to_bits());
            }
            fp.write_u8(u8::from(entry.on_ground));
            fp.write_u8(match entry.camera_mode {
                CameraMode::FirstPerson => 1,
                CameraMode::ThirdPerson => 2,
            });
        }
        fp.finish_hex()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    fn bits(values: [f32; 3]) -> [u32; 3] {
        values.map(f32::to_bits)
    }

    fn idle() -> MoveIntent {
        MoveIntent::default()
    }

    /// Runs idle steps until grounded (bounded — a fall that never
    /// lands must fail the test, not hang it).
    fn settle(state: &mut PlayerState, config: &PlayerConfig, world: &CollisionWorld) {
        for _ in 0..600 {
            step_player(state, config, world, &idle(), DT);
            if state.on_ground {
                return;
            }
        }
        panic!("the player never landed within 600 ticks");
    }

    #[test]
    fn axes_map_to_world_velocity() {
        let config = PlayerConfig::default();
        let world = CollisionWorld::new(0.0);

        let mut state = PlayerState::spawn([0.0, 5.0, 0.0]);
        step_player(
            &mut state,
            &config,
            &world,
            &MoveIntent {
                forward: 1,
                ..idle()
            },
            DT,
        );
        assert_eq!(state.velocity[0], 0.0);
        assert_eq!(state.velocity[2], -config.walk_speed, "W moves -Z at yaw 0");

        let mut state = PlayerState::spawn([0.0, 5.0, 0.0]);
        step_player(
            &mut state,
            &config,
            &world,
            &MoveIntent {
                strafe: 1,
                ..idle()
            },
            DT,
        );
        assert_eq!(state.velocity[0], config.walk_speed, "D moves +X at yaw 0");
        assert_eq!(state.velocity[2], 0.0);

        let mut state = PlayerState::spawn([0.0, 5.0, 0.0]);
        step_player(&mut state, &config, &world, &idle(), DT);
        assert_eq!(
            state.velocity[0], 0.0,
            "W and S cancel at the shell; zero intent means zero motion"
        );
        assert_eq!(state.velocity[2], 0.0);

        // Nonzero yaw: same-binary reproducibility (the trig path is
        // deliberately unpinned — see test 16 and the F1 amendment).
        let mut first = PlayerState::spawn([0.0, 5.0, 0.0]);
        first.yaw = 0.5;
        let mut second = first;
        let intent = MoveIntent {
            forward: 1,
            strafe: 1,
            ..idle()
        };
        step_player(&mut first, &config, &world, &intent, DT);
        step_player(&mut second, &config, &world, &intent, DT);
        assert_eq!(bits(first.velocity), bits(second.velocity));
    }

    #[test]
    fn horizontal_speed_capped() {
        let config = PlayerConfig::default();
        let world = CollisionWorld::new(0.0);
        let diagonal = MoveIntent {
            forward: 1,
            strafe: 1,
            ..idle()
        };

        let mut state = PlayerState::spawn([0.0, 50.0, 0.0]);
        step_player(&mut state, &config, &world, &diagonal, DT);
        let magnitude =
            (state.velocity[0] * state.velocity[0] + state.velocity[2] * state.velocity[2]).sqrt();
        assert!(
            (magnitude - config.walk_speed).abs() < 1e-5,
            "diagonal input is normalized to the cap: {magnitude}"
        );

        let mut state = PlayerState::spawn([0.0, 50.0, 0.0]);
        step_player(
            &mut state,
            &config,
            &world,
            &MoveIntent {
                forward: 1,
                sprint: true,
                ..idle()
            },
            DT,
        );
        assert_eq!(state.velocity[2], -config.sprint_speed);
    }

    #[test]
    fn gravity_bounded_by_terminal_velocity() {
        let config = PlayerConfig::default();
        let world = CollisionWorld::new(0.0);
        let mut state = PlayerState::spawn([0.0, 1000.0, 0.0]);
        for _ in 0..300 {
            step_player(&mut state, &config, &world, &idle(), DT);
        }
        assert_eq!(
            state.velocity[1], -config.terminal_fall,
            "free fall clamps at the terminal velocity"
        );
        assert!(!state.on_ground);
        assert!(state.position[1] > 0.0, "still airborne: no floor reached");
    }

    #[test]
    fn jump_requires_ground_and_fresh_press() {
        let config = PlayerConfig::default();
        let world = CollisionWorld::new(0.0);
        let mut airborne_press = PlayerState::spawn([0.0, 10.0, 0.0]);
        step_player(&mut airborne_press, &config, &world, &idle(), DT);
        let falling = airborne_press.velocity[1];
        step_player(
            &mut airborne_press,
            &config,
            &world,
            &MoveIntent {
                jump_pressed: true,
                ..idle()
            },
            DT,
        );
        assert!(
            airborne_press.velocity[1] < 0.0 && airborne_press.velocity[1] < falling,
            "an airborne jump press grants no impulse"
        );

        let mut state = PlayerState::spawn([0.0, 0.0, 0.0]);
        settle(&mut state, &config, &world);
        let mut outcome = StepOutcome::default();
        for _ in 0..3 {
            outcome = step_player(
                &mut state,
                &config,
                &world,
                &MoveIntent {
                    jump_pressed: true,
                    ..idle()
                },
                DT,
            );
        }
        assert!(
            !outcome.jumped,
            "a held key is not a fresh press: only the first tick jumps"
        );
    }

    #[test]
    fn jump_settles_stably() {
        let config = PlayerConfig::default();
        let world = CollisionWorld::new(0.0);
        let mut state = PlayerState::spawn([0.0, 0.0, 0.0]);
        settle(&mut state, &config, &world);

        let outcome = step_player(
            &mut state,
            &config,
            &world,
            &MoveIntent {
                jump_pressed: true,
                ..idle()
            },
            DT,
        );
        assert!(outcome.jumped);
        assert_eq!(state.velocity[1], config.jump_speed, "the exact impulse");

        for _ in 0..200 {
            step_player(&mut state, &config, &world, &idle(), DT);
            if state.on_ground {
                break;
            }
        }
        assert!(state.on_ground, "the jump lands");
        assert_eq!(state.velocity[1], 0.0);

        let resting = bits(state.position);
        let resting_velocity = bits(state.velocity);
        for _ in 0..50 {
            step_player(&mut state, &config, &world, &idle(), DT);
        }
        assert_eq!(
            bits(state.position),
            resting,
            "the resting state is a fixed point — no drift"
        );
        assert_eq!(bits(state.velocity), resting_velocity);
    }

    #[test]
    fn ground_contact_box_top_and_plane() {
        let config = PlayerConfig::default();
        let world = CollisionWorld::new(0.0).with_box([3.0, 0.0, 0.0], 2.0, 2.0);
        let mut state = PlayerState::spawn([3.0, 5.0, 0.0]);
        settle(&mut state, &config, &world);
        assert!(state.on_ground);
        assert_eq!(
            state.position[1],
            2.0 + SKIN,
            "box-top contact rests a skin above the face"
        );

        // Walk off the box (resting contact must not block the walk).
        let walk = MoveIntent {
            strafe: 1,
            ..idle()
        };
        let mut left_ground = false;
        for _ in 0..80 {
            step_player(&mut state, &config, &world, &walk, DT);
            if !state.on_ground {
                left_ground = true;
                break;
            }
        }
        assert!(left_ground, "walking past the edge starts a fall");
        settle(&mut state, &config, &world);
        assert_eq!(state.position[1], 0.0, "the ground plane holds at y=0");
    }

    #[test]
    fn nonpenetration_holds_after_resolution() {
        let config = PlayerConfig::default();
        let world = CollisionWorld::new(0.0).with_box([0.0, 0.0, -5.0], 2.0, 3.0);
        let mut state = PlayerState::spawn([0.0, 0.0, 0.0]);
        settle(&mut state, &config, &world);

        let walk = MoveIntent {
            forward: 1,
            ..idle()
        };
        for _ in 0..200 {
            step_player(&mut state, &config, &world, &walk, DT);
        }
        let (min, max) = player_box(state.position, &config);
        assert!(
            !world.box_overlaps(min, max),
            "no interpenetration after clamping"
        );
        assert_eq!(
            state.position[2],
            -4.0 + SKIN + config.radius,
            "the wall stops the player a skin short of its near face"
        );

        // Adversarial: spawned INSIDE a tall box, off-center so the
        // minimal push is unambiguously +X (the downward candidate
        // would cross the solid ground plane and is invalid; a centered
        // spawn makes x and z mathematically tie, resolving by ulp —
        // correct but not a stable expectation to pin).
        let world = CollisionWorld::new(0.0).with_box([0.0, 0.0, -5.0], 2.0, 5.0);
        let mut inside = PlayerState::spawn([0.5, 2.0, -5.0]);
        step_player(&mut inside, &config, &world, &idle(), DT);
        let (min, max) = player_box(inside.position, &config);
        assert!(!world.box_overlaps(min, max));
        assert!(
            inside.position[0] > 1.0 && inside.position[0] < 1.5,
            "pushed out to the near (+X) face, minimally: {}",
            inside.position[0]
        );
        assert!(inside.position[1] < 2.0, "the escape does not hover");
        assert!(!inside.on_ground, "still airborne after the push-out");
    }

    #[test]
    fn no_tunneling_at_max_speed() {
        let config = PlayerConfig::default();
        // A wall thinner than one sprint tick's travel (0.2 m).
        let world = CollisionWorld::new(0.0).with_box([0.0, 0.0, -5.0], 0.2, 3.0);
        let mut state = PlayerState::spawn([0.0, 0.0, 0.0]);
        settle(&mut state, &config, &world);

        let sprint = MoveIntent {
            forward: 1,
            sprint: true,
            ..idle()
        };
        for _ in 0..100 {
            step_player(&mut state, &config, &world, &sprint, DT);
        }
        assert_eq!(
            state.position[2],
            -4.9 + SKIN + config.radius,
            "the swept clamp stops the player at the near face — a wall \
             thinner than one tick's travel cannot be tunneled"
        );
        let (min, max) = player_box(state.position, &config);
        assert!(!world.box_overlaps(min, max));
    }

    #[test]
    fn wall_collision_zeroes_axis_velocity() {
        let config = PlayerConfig::default();
        // A wall long enough that the slide never passes its end.
        let world = CollisionWorld::new(0.0).with_collider(StaticCollider {
            min: [-20.0, 0.0, -6.0],
            max: [20.0, 3.0, -4.0],
        });
        let mut state = PlayerState::spawn([0.0, 0.0, 0.0]);
        settle(&mut state, &config, &world);

        let diagonal = MoveIntent {
            forward: 1,
            strafe: 1,
            ..idle()
        };
        for _ in 0..200 {
            step_player(&mut state, &config, &world, &diagonal, DT);
        }
        assert_eq!(state.velocity[2], 0.0, "the blocked axis zeroes");
        assert!(
            state.velocity[0] > 0.0,
            "the unblocked axis keeps sliding along the wall"
        );
        assert!(state.position[0] > 1.0, "the slide actually moved");
    }

    #[test]
    fn ceiling_blocks_ascent() {
        let config = PlayerConfig::default();
        // An overhang the jump reaches (apex head height 2.98 > 2.5).
        let world = CollisionWorld::new(0.0).with_box([0.0, 2.5, 0.0], 2.0, 2.0);
        let mut state = PlayerState::spawn([0.0, 0.0, 0.0]);
        settle(&mut state, &config, &world);

        let mut hit_ceiling = false;
        let outcome = step_player(
            &mut state,
            &config,
            &world,
            &MoveIntent {
                jump_pressed: true,
                ..idle()
            },
            DT,
        );
        assert!(outcome.jumped);
        for _ in 0..200 {
            let outcome = step_player(&mut state, &config, &world, &idle(), DT);
            hit_ceiling |= outcome.hit_ceiling;
            if state.on_ground {
                break;
            }
        }
        assert!(hit_ceiling, "the overhang clamps the ascent");
        assert!(state.on_ground, "gravity resumes after the ceiling hit");
        assert_eq!(state.velocity[1], 0.0, "the ceiling zeroed the ascent");
    }

    #[test]
    fn corner_collision_stable() {
        let config = PlayerConfig::default();
        // An inside corner of long walls: one spanning z blocks +X,
        // one spanning x blocks +Z — the diagonal cannot slip past
        // either end.
        let world = CollisionWorld::new(0.0)
            .with_collider(StaticCollider {
                min: [2.0, 0.0, -6.0],
                max: [4.0, 3.0, 6.0],
            })
            .with_collider(StaticCollider {
                min: [-6.0, 0.0, 2.0],
                max: [6.0, 3.0, 4.0],
            });
        let mut state = PlayerState::spawn([0.0, 0.0, 0.0]);
        settle(&mut state, &config, &world);

        // backward (+Z) plus strafe (+X): diagonally into the corner.
        let into_corner = MoveIntent {
            forward: -1,
            strafe: 1,
            ..idle()
        };
        for _ in 0..100 {
            step_player(&mut state, &config, &world, &into_corner, DT);
        }
        assert!(
            state.position[0] > 1.5 && state.position[0] < 2.0,
            "clamped at the x wall's near face: {}",
            state.position[0]
        );
        assert!(
            state.position[2] > 1.5 && state.position[2] < 2.0,
            "clamped at the z wall's near face: {}",
            state.position[2]
        );
        let corner = bits(state.position);
        for _ in 0..20 {
            step_player(&mut state, &config, &world, &into_corner, DT);
        }
        assert_eq!(
            bits(state.position),
            corner,
            "multi-axis contact is a stable fixed point — no jitter"
        );
        let (min, max) = player_box(state.position, &config);
        assert!(!world.box_overlaps(min, max));
    }

    #[test]
    fn world_bounds_clamp() {
        let config = PlayerConfig::default();
        let world = CollisionWorld::new(0.0).with_bounds([-10.0, 0.0, -10.0], [10.0, 50.0, 10.0]);
        let mut state = PlayerState::spawn([0.0, 0.0, 0.0]);
        settle(&mut state, &config, &world);

        let sprint_diagonal = MoveIntent {
            forward: -1,
            strafe: 1,
            sprint: true,
            ..idle()
        };
        for _ in 0..300 {
            step_player(&mut state, &config, &world, &sprint_diagonal, DT);
        }
        assert_eq!(state.position[0], 10.0 - config.radius);
        assert_eq!(state.position[2], 10.0 - config.radius);
        assert_eq!(state.velocity[0], 0.0, "outward velocity zeroed at bounds");
        assert_eq!(state.velocity[2], 0.0);
        assert!(state.on_ground);
    }

    #[test]
    fn nonfinite_intent_sanitized() {
        let config = PlayerConfig::default();
        let world = CollisionWorld::new(0.0);
        let mut state = PlayerState::spawn([0.0, 0.0, 0.0]);
        settle(&mut state, &config, &world);
        let before = (state.yaw.to_bits(), state.pitch.to_bits());

        let outcome = step_player(
            &mut state,
            &config,
            &world,
            &MoveIntent {
                look: (f32::NAN, f32::INFINITY),
                ..idle()
            },
            DT,
        );
        assert_eq!(outcome.sanitized_look, 2, "the F5 observable surface");
        assert!(!outcome.refused_nonfinite_state);
        assert_eq!(
            (state.yaw.to_bits(), state.pitch.to_bits()),
            before,
            "non-finite look deltas are dropped, not applied"
        );

        let mut corrupt = PlayerState::spawn([f32::NAN, 0.0, 0.0]);
        let position_before = bits(corrupt.position);
        let velocity_before = bits(corrupt.velocity);
        let outcome = step_player(&mut corrupt, &config, &world, &idle(), DT);
        assert!(
            outcome.refused_nonfinite_state,
            "a non-finite state is refused, visibly"
        );
        assert_eq!(bits(corrupt.position), position_before);
        assert_eq!(bits(corrupt.velocity), velocity_before);
    }

    #[test]
    fn live_input_consumes_displacement_and_jump_once() {
        let mut state = PlayerState::spawn([0.0; 3]);
        state.on_ground = true;
        let mut driver = PlayerDriver::new(
            state,
            PlayerConfig::default(),
            CollisionWorld::new(0.0),
            100,
        );
        let press = MoveIntent {
            look: (0.125, 0.0625),
            jump_pressed: true,
            ..idle()
        };
        assert_eq!(
            driver
                .advance(Duration::from_millis(3), press)
                .executed_ticks,
            0
        );
        driver.advance(
            Duration::from_millis(2),
            MoveIntent {
                look: (0.125, 0.0),
                ..idle()
            },
        );
        let mut observed = Vec::new();
        let step = driver.advance_observed(Duration::from_millis(25), idle(), |state, _| {
            observed.push((state.yaw, state.pitch, state.velocity[1]));
        });
        assert_eq!(step.executed_ticks, 3);
        assert!(
            observed
                .iter()
                .all(|&(yaw, pitch, _)| (yaw, pitch) == (0.25, 0.0625))
        );
        assert!(observed[0].2 > 0.0, "jump captured before the first tick");
        assert!(observed[1].2 < observed[0].2);

        driver.advance(Duration::ZERO, press);
        driver.clear_pending_input();
        driver.state_mut().on_ground = true;
        driver.state_mut().velocity = [0.0; 3];
        driver.advance(Duration::from_millis(10), idle());
        assert_eq!(driver.state().yaw, 0.25);
        assert!(driver.state().velocity[1] <= 0.0);
    }

    #[test]
    fn fixed_step_batch_equivalence() {
        // 100 Hz: the period divides a second exactly, so 64 ticks are
        // 640 ms — the batch, the drip, and the headless run cover the
        // same due ticks (within the catch-up bound; the over-bound
        // case belongs to no_discard_accounting below — F3).
        let make = || {
            PlayerDriver::with_catchup(
                PlayerState::spawn([0.0, 0.0, 0.0]),
                PlayerConfig::default(),
                CollisionWorld::new(0.0).with_box([0.0, 0.0, -2.0], 1.0, 1.0),
                100,
                64,
            )
        };
        let intent = MoveIntent {
            forward: 1,
            ..idle()
        };

        let mut batched = make();
        batched.advance(Duration::from_nanos(640_000_000), intent);

        let mut dripped = make();
        for _ in 0..64 {
            dripped.advance(Duration::from_nanos(10_000_000), intent);
        }

        let mut headless = make();
        headless.run_headless(64, |_| intent);

        assert_eq!(batched.ticks_due_total(), 64);
        assert_eq!(batched.executed_ticks_total(), 64);
        assert_eq!(dripped.executed_ticks_total(), 64);
        assert_eq!(headless.executed_ticks_total(), 64);
        assert_eq!(
            bits(batched.state().position),
            bits(dripped.state().position),
            "batching does not change logical state"
        );
        assert_eq!(
            bits(batched.state().position),
            bits(headless.state().position),
            "the headless path is the same fixed step"
        );
        assert_eq!(
            batched.trace_fingerprint_hex(),
            dripped.trace_fingerprint_hex()
        );
        assert_eq!(
            batched.trace_fingerprint_hex(),
            headless.trace_fingerprint_hex()
        );
    }

    #[test]
    fn no_discard_accounting() {
        let mut driver = PlayerDriver::with_catchup(
            PlayerState::spawn([0.0, 0.0, 0.0]),
            PlayerConfig::default(),
            CollisionWorld::new(0.0),
            100,
            8,
        );
        let step = driver.advance(Duration::from_nanos(2_000_000_000), idle());
        assert_eq!(step.executed_ticks, 8, "the catch-up bound holds");
        assert_eq!(step.outstanding_ticks, 192, "the remainder is retained");

        // The accounting invariant at every observation boundary.
        for _ in 0..25 {
            driver.advance(Duration::from_nanos(100_000_000), idle());
            assert_eq!(
                driver.ticks_due_total(),
                driver.executed_ticks_total() + driver.outstanding_ticks(),
                "due == executed + outstanding, never silently discarded"
            );
        }
        let mut drained = 0;
        while driver.outstanding_ticks() > 0 {
            // Zero elapsed time: the retained obligation drains purely
            // from catch-up capacity — no new time accrues (the
            // no-discard discipline's drain semantics).
            driver.advance(Duration::ZERO, idle());
            drained += 1;
            assert!(drained < 10_000, "the drain must terminate");
        }
        assert_eq!(
            driver.ticks_due_total(),
            driver.executed_ticks_total(),
            "every due tick eventually executes — conservation"
        );
    }

    #[test]
    fn deterministic_trace_reproducible() {
        // Same-binary-same-platform reproducibility, WITH turns and
        // jumps (the trig path — deliberately not pinned cross-platform).
        let make = || {
            PlayerDriver::new(
                PlayerState::spawn([0.0, 3.0, 0.0]),
                PlayerConfig::default(),
                CollisionWorld::new(0.0)
                    .with_box([1.0, 0.0, -4.0], 2.0, 2.0)
                    .with_box([-2.0, 0.0, -8.0], 1.0, 1.0),
                60,
            )
        };
        let pattern = |tick: u64| MoveIntent {
            forward: if tick.is_multiple_of(3) { 1 } else { 0 },
            strafe: if tick.is_multiple_of(5) { 1 } else { 0 },
            jump_pressed: tick == 10 || tick == 100,
            sprint: tick.is_multiple_of(2),
            look: if tick.is_multiple_of(7) {
                (0.01, -0.003)
            } else {
                (0.0, 0.0)
            },
        };
        let mut first = make();
        first.run_headless(300, pattern);
        let mut second = make();
        second.run_headless(300, pattern);
        assert_eq!(
            first.trace_fingerprint_hex(),
            second.trace_fingerprint_hex(),
            "identical scripted runs produce identical traces"
        );
        assert_eq!(bits(first.state().position), bits(second.state().position));
    }

    #[test]
    fn idle_player_settles_and_stays() {
        let mut driver = PlayerDriver::new(
            PlayerState::spawn([0.0, 3.0, 0.0]),
            PlayerConfig::default(),
            CollisionWorld::new(0.0),
            60,
        );
        driver.run_headless(200, |_| idle());
        assert!(driver.state().on_ground);
        assert_eq!(driver.state().position[1], 0.0);

        let resting = bits(driver.state().position);
        let resting_velocity = bits(driver.state().velocity);
        driver.run_headless(50, |_| idle());
        assert_eq!(bits(driver.state().position), resting, "no drift at rest");
        assert_eq!(bits(driver.state().velocity), resting_velocity);
    }

    #[test]
    fn camera_pose_first_person_from_state() {
        let config = PlayerConfig::default();
        let mut state = PlayerState::spawn([1.0, 2.0, 3.0]);
        state.yaw = 0.3;
        state.pitch = -0.2;
        let pose = camera_pose(&state, &config);
        assert_eq!(pose.position, [1.0, 2.0 + config.eye_height, 3.0]);
        assert_eq!(pose.yaw, 0.3);
        assert_eq!(pose.pitch, -0.2);
        assert_eq!(pose.roll, 0.0);
        assert_eq!(
            bits(camera_pose(&state, &config).position),
            bits(pose.position),
            "the pose is a pure function of state"
        );
    }

    #[test]
    fn camera_pose_third_person_offset() {
        let config = PlayerConfig::default();
        let mut state = PlayerState::spawn([1.0, 2.0, 3.0]);
        state.camera_mode = CameraMode::ThirdPerson;

        // Zero angles: exact arithmetic (the libm-free path — F1).
        let pose = camera_pose(&state, &config);
        assert_eq!(
            pose.position,
            [
                1.0,
                2.0 + config.eye_height + config.tp_lift,
                3.0 + config.tp_distance
            ]
        );

        // Nonzero angles: the documented offset formula.
        state.yaw = 0.7;
        state.pitch = -0.4;
        let pose = camera_pose(&state, &config);
        let forward = look_direction(state.yaw, state.pitch);
        let eye = [1.0, 2.0 + config.eye_height, 3.0];
        let expected = [
            eye[0] - forward[0] * config.tp_distance,
            eye[1] - forward[1] * config.tp_distance + config.tp_lift,
            eye[2] - forward[2] * config.tp_distance,
        ];
        assert_eq!(bits(pose.position), bits(expected));
        assert_eq!(pose.yaw, 0.7);
        assert_eq!(pose.pitch, -0.4);
    }

    #[test]
    fn camera_toggle_preserves_player_state_bitwise() {
        let config = PlayerConfig::default();
        let mut state = PlayerState::spawn([1.0, 2.0, 3.0]);
        state.velocity = [0.5, -0.25, -1.0];
        state.yaw = 0.7;
        state.pitch = -0.3;
        state.on_ground = true;
        let position = bits(state.position);
        let velocity = bits(state.velocity);
        let (yaw, pitch) = (state.yaw.to_bits(), state.pitch.to_bits());

        for toggles in 1..=5 {
            state.toggle_camera_mode();
            assert_eq!(
                state.camera_mode,
                if toggles % 2 == 1 {
                    CameraMode::ThirdPerson
                } else {
                    CameraMode::FirstPerson
                }
            );
            assert_eq!(bits(state.position), position, "no teleport (charter)");
            assert_eq!(bits(state.velocity), velocity, "no reset");
            assert_eq!(state.yaw.to_bits(), yaw);
            assert_eq!(state.pitch.to_bits(), pitch);
            assert!(state.on_ground);
        }

        // Only the DERIVED pose differs between modes (state is
        // ThirdPerson after the odd toggle count).
        let mut fp = state;
        fp.camera_mode = CameraMode::FirstPerson;
        assert_ne!(
            bits(camera_pose(&state, &config).position),
            bits(camera_pose(&fp, &config).position)
        );
    }

    #[test]
    fn tick_rate_policy_clamps_into_the_representable_range() {
        // The R4 boundaries: rates at, below, and above the policy
        // window — including the >1e9 zone that previously produced
        // period_ns = 0 (the division-by-zero hazard).
        for rate in [
            0u32,
            1,
            2,
            60,
            999_999_999,
            1_000_000_000,
            1_000_000_001,
            u32::MAX,
        ] {
            let driver = PlayerDriver::with_catchup(
                PlayerState::spawn([0.0, 0.0, 0.0]),
                PlayerConfig::default(),
                CollisionWorld::new(0.0),
                rate,
                8,
            );
            assert!(
                driver.period_ns >= 1,
                "rate {rate}: period_ns is nonzero under the policy"
            );
            assert!(
                driver.dt.is_finite() && driver.dt > 0.0,
                "rate {rate}: dt is finite and positive"
            );
        }
        // Boundary exactness: 1 Hz is a one-second period; 1 GHz is
        // exactly one nanosecond; above the bound clamps DOWN to it.
        let one_hz = PlayerDriver::with_catchup(
            PlayerState::spawn([0.0, 0.0, 0.0]),
            PlayerConfig::default(),
            CollisionWorld::new(0.0),
            1,
            8,
        );
        assert_eq!(one_hz.period_ns, 1_000_000_000);
        let one_ghz = PlayerDriver::with_catchup(
            PlayerState::spawn([0.0, 0.0, 0.0]),
            PlayerConfig::default(),
            CollisionWorld::new(0.0),
            1_000_000_000,
            8,
        );
        assert_eq!(one_ghz.period_ns, 1);
        let over = PlayerDriver::with_catchup(
            PlayerState::spawn([0.0, 0.0, 0.0]),
            PlayerConfig::default(),
            CollisionWorld::new(0.0),
            1_000_000_001,
            8,
        );
        assert_eq!(over.period_ns, 1, "clamped down to the 1 GHz bound");
        assert_eq!(over.dt, one_ghz.dt, "the clamp preserves dt exactness");
    }

    #[test]
    fn period_zero_is_impossible_and_tiny_advances_execute() {
        // At the 1 GHz bound, nanosecond-scale advances flow through
        // every accumulator division site without a zero period —
        // real ticks execute, bounded by the catch-up limit, and the
        // no-discard accounting holds at the extreme rate.
        let mut driver = PlayerDriver::with_catchup(
            PlayerState::spawn([0.0, 0.0, 0.0]),
            PlayerConfig::default(),
            CollisionWorld::new(0.0),
            1_000_000_000,
            8,
        );
        assert_eq!(driver.period_ns, 1);
        let step = driver.advance(Duration::from_nanos(8), idle());
        assert_eq!(step.executed_ticks, 8);
        assert_eq!(step.outstanding_ticks, 0);
        let step = driver.advance(Duration::from_nanos(3), idle());
        assert_eq!(step.executed_ticks, 3);
        assert_eq!(
            driver.ticks_due_total(),
            driver.executed_ticks_total() + driver.outstanding_ticks(),
            "the accounting invariant holds at the policy bound"
        );
    }

    #[test]
    fn nonfinite_timestep_and_config_refused() {
        let config = PlayerConfig::default();
        let world = CollisionWorld::new(0.0);
        // The R4 time-step contract: finite and positive, or refused
        // BEFORE any state mutation.
        for dt in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -1.0] {
            let mut state = PlayerState::spawn([0.0, 0.0, 0.0]);
            let position = bits(state.position);
            let velocity = bits(state.velocity);
            let outcome = step_player(&mut state, &config, &world, &idle(), dt);
            assert!(outcome.refused_invalid_inputs, "dt {dt} must be refused");
            assert!(!outcome.jumped && !outcome.landed && !outcome.hit_ground);
            assert_eq!(
                bits(state.position),
                position,
                "refusal leaves the state untouched"
            );
            assert_eq!(bits(state.velocity), velocity);
        }
        // The config side: any non-finite field refuses the step the
        // same way — a valid state can never be silently turned
        // non-finite by invalid inputs.
        for config in [
            PlayerConfig {
                gravity: f32::NAN,
                ..PlayerConfig::default()
            },
            PlayerConfig {
                walk_speed: f32::INFINITY,
                ..PlayerConfig::default()
            },
            PlayerConfig {
                radius: f32::NAN,
                ..PlayerConfig::default()
            },
        ] {
            let mut state = PlayerState::spawn([0.0, 0.0, 0.0]);
            let position = bits(state.position);
            let outcome = step_player(&mut state, &config, &world, &idle(), DT);
            assert!(
                outcome.refused_invalid_inputs,
                "config {config:?} must be refused"
            );
            assert_eq!(bits(state.position), position);
        }
    }

    #[test]
    fn valid_inputs_produce_no_refusals() {
        // The validity contract never fires on the valid-input path:
        // the frozen trace and the whole battery pin the behavior.
        let config = PlayerConfig::default();
        let world = CollisionWorld::new(0.0);
        let mut state = PlayerState::spawn([0.0, 5.0, 0.0]);
        let outcome = step_player(
            &mut state,
            &config,
            &world,
            &MoveIntent {
                forward: 1,
                ..idle()
            },
            DT,
        );
        assert!(!outcome.refused_invalid_inputs);
        assert!(!outcome.refused_nonfinite_state);
        assert_eq!(state.velocity[2], -config.walk_speed);
    }

    #[test]
    fn wide_tick_accounting_survives_the_two_pow32_counterexample() {
        // R4b: at the 1 GHz policy bound, a 2^32-nanosecond advance
        // creates 2^32 due ticks — the old `as u32` narrowing wrapped
        // that count to ZERO, silently discarding the whole
        // obligation. The wide accounting keeps it exact, executing
        // only the catch-up-bounded handful of ticks (never
        // billions).
        let mut driver = PlayerDriver::with_catchup(
            PlayerState::spawn([0.0, 0.0, 0.0]),
            PlayerConfig::default(),
            CollisionWorld::new(0.0),
            1_000_000_000,
            8,
        );
        let step = driver.advance(Duration::from_nanos(4_294_967_296), idle());
        assert_eq!(step.executed_ticks, 8, "only the catch-up bound executes");
        assert_eq!(
            step.outstanding_ticks,
            4_294_967_296 - 8,
            "the 2^32 due ticks account exactly in u64 — no wrap to zero"
        );
        assert_eq!(driver.ticks_due_total(), 4_294_967_296);
        assert_eq!(
            driver.ticks_due_total(),
            driver.executed_ticks_total() + driver.outstanding_ticks(),
            "due == executed + outstanding at the counterexample"
        );

        // The documented total-advancement domain: an advance beyond
        // the remaining u64::MAX-nanosecond domain is clamped to the
        // boundary AND REPORTED (never silent), the accounting
        // identity stays exact at the clamp, and the run stays fast.
        let step = driver.advance(Duration::from_secs(u64::MAX), idle());
        assert!(step.domain_clamped, "the domain clamp is reported");
        assert_eq!(step.executed_ticks, 8, "still catch-up bounded");
        assert_eq!(driver.ticks_due_total(), u64::MAX);
        assert_eq!(
            driver.ticks_due_total(),
            driver.executed_ticks_total() + driver.outstanding_ticks(),
            "the identity holds exactly at the clamped domain boundary"
        );
        // Past the boundary there is no remaining domain: granted
        // time is zero, the clamp reports again, and the retained
        // backlog still drains — the accounting stands still rather
        // than corrupting.
        let step = driver.advance(Duration::from_secs(1), idle());
        assert!(step.domain_clamped);
        assert_eq!(step.executed_ticks, 8, "the retained backlog still drains");
        assert_eq!(
            driver.ticks_due_total(),
            driver.executed_ticks_total() + driver.outstanding_ticks()
        );
    }

    #[test]
    fn invalid_finite_configs_refused_before_mutation() {
        // R4b: finite is not sufficient — a negative pitch limit made
        // the subsequent clamp PANIC (min > max), and nonpositive
        // extents invert or degenerate the player box. The semantic
        // validity contract refuses all of them BEFORE any state
        // mutation.
        let world = CollisionWorld::new(0.0);
        for config in [
            PlayerConfig {
                pitch_limit: -1.0,
                ..PlayerConfig::default()
            },
            PlayerConfig {
                pitch_limit: 0.0,
                ..PlayerConfig::default()
            },
            PlayerConfig {
                height: 0.0,
                ..PlayerConfig::default()
            },
            PlayerConfig {
                height: -1.0,
                ..PlayerConfig::default()
            },
            PlayerConfig {
                radius: -0.5,
                ..PlayerConfig::default()
            },
        ] {
            let mut state = PlayerState::spawn([0.0, 0.0, 0.0]);
            let position = bits(state.position);
            let velocity = bits(state.velocity);
            let outcome = step_player(&mut state, &config, &world, &idle(), DT);
            assert!(
                outcome.refused_invalid_inputs,
                "config {config:?} must be refused"
            );
            assert_eq!(
                bits(state.position),
                position,
                "refusal leaves the state untouched"
            );
            assert_eq!(bits(state.velocity), velocity);
        }
        // Look input cannot smuggle an invalid config past the gate:
        // the refusal precedes the look application too.
        let mut state = PlayerState::spawn([0.0, 0.0, 0.0]);
        let config = PlayerConfig {
            pitch_limit: -1.0,
            ..PlayerConfig::default()
        };
        let outcome = step_player(
            &mut state,
            &config,
            &world,
            &MoveIntent {
                look: (0.1, 0.1),
                ..idle()
            },
            DT,
        );
        assert!(outcome.refused_invalid_inputs);
        assert_eq!(state.yaw.to_bits(), 0.0f32.to_bits());
        assert_eq!(state.pitch.to_bits(), 0.0f32.to_bits());
    }

    #[test]
    fn driver_reports_refused_steps_explicitly() {
        // R4b: the driver never hides refused steps as movement work.
        // An invalid config refuses EVERY step; the refusal count is
        // explicit and bounded; the accounting still consumes its
        // ticks (the invariant is time-based); the player state stays
        // frozen at spawn, traced honestly.
        let mut driver = PlayerDriver::new(
            PlayerState::spawn([0.0, 5.0, 0.0]),
            PlayerConfig {
                pitch_limit: -1.0,
                ..PlayerConfig::default()
            },
            CollisionWorld::new(0.0),
            60,
        );
        driver.run_headless(10, |_| MoveIntent {
            forward: 1,
            ..idle()
        });
        assert_eq!(driver.refused_steps(), 10);
        assert_eq!(
            driver.executed_ticks_total(),
            10,
            "ticks consumed (time-based accounting)"
        );
        assert_eq!(
            driver.state().position,
            [0.0, 5.0, 0.0],
            "no movement work happened"
        );
        assert_eq!(driver.state().velocity, [0.0, 0.0, 0.0]);
        assert_eq!(driver.trace().len(), 10, "the frozen state is traced");
    }

    fn dist3(a: [f32; 3], b: [f32; 3]) -> f32 {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    }

    #[test]
    fn step_up_climbs_small_boxes() {
        let config = PlayerConfig {
            step_height: 0.55,
            ..PlayerConfig::default()
        };
        let world = CollisionWorld::new(0.0).with_box([0.0, 0.0, -5.0], 2.0, 0.5);
        let mut state = PlayerState::spawn([0.0, 0.0, 0.0]);
        settle(&mut state, &config, &world);

        let walk = MoveIntent {
            forward: 1,
            ..idle()
        };
        let mut climbed = false;
        for _ in 0..100 {
            step_player(&mut state, &config, &world, &walk, DT);
            if state.position[1] > 0.4 {
                climbed = true;
                break;
            }
        }
        assert!(climbed, "the 0.5 box is stepped onto without jumping");
        assert_eq!(state.position[1], 0.5 + SKIN, "snapped onto the step top");
        assert!(state.on_ground);
        assert_eq!(
            state.velocity[2], -config.walk_speed,
            "the climb preserves forward motion (no axis zeroing)"
        );
        let (min, max) = player_box(state.position, &config);
        assert!(!world.box_overlaps(min, max));
        for _ in 0..20 {
            step_player(&mut state, &config, &world, &walk, DT);
        }
        assert!(state.on_ground, "still supported on the box top");
        assert!(
            state.position[2] < -4.0,
            "progressed across the top: {}",
            state.position[2]
        );
        assert_eq!(state.position[1], 0.5 + SKIN, "no height drift on top");
    }

    #[test]
    fn step_up_refuses_tall_obstacles() {
        let config = PlayerConfig {
            step_height: 0.55,
            ..PlayerConfig::default()
        };
        let world = CollisionWorld::new(0.0).with_box([0.0, 0.0, -5.0], 2.0, 1.5);
        let mut state = PlayerState::spawn([0.0, 0.0, 0.0]);
        settle(&mut state, &config, &world);

        let walk = MoveIntent {
            forward: 1,
            ..idle()
        };
        for _ in 0..100 {
            step_player(&mut state, &config, &world, &walk, DT);
        }
        assert_eq!(
            state.position[2],
            -4.0 + SKIN + config.radius,
            "clamped at the face"
        );
        assert_eq!(
            state.position[1], 0.0,
            "no climb: the obstacle stands taller than the step"
        );
        assert_eq!(state.velocity[2], 0.0);
    }

    #[test]
    fn step_up_requires_ground() {
        let config = PlayerConfig {
            step_height: 0.55,
            ..PlayerConfig::default()
        };
        let world = CollisionWorld::new(0.0).with_box([0.0, 0.0, -5.0], 2.0, 0.5);
        // Airborne at the box's face height: the probe is
        // grounded-only, so the move stays clamped and gravity
        // applies.
        let mut state = PlayerState::spawn([0.0, 0.2, -3.63]);
        step_player(
            &mut state,
            &config,
            &world,
            &MoveIntent {
                forward: 1,
                ..idle()
            },
            DT,
        );
        assert!(state.position[1] < 0.2, "airborne: gravity, no step gain");
        assert!(!state.on_ground);
        assert_eq!(state.velocity[2], 0.0, "the box still blocks horizontally");
    }

    #[test]
    fn step_up_disabled_preserves_cycle1_behavior() {
        // step_height 0.0 (the default): the LOW box stays a wall —
        // the Cycle-1 clamp behavior, bit-for-bit.
        let config = PlayerConfig::default();
        assert_eq!(config.step_height, 0.0);
        let world = CollisionWorld::new(0.0).with_box([0.0, 0.0, -5.0], 2.0, 0.5);
        let mut state = PlayerState::spawn([0.0, 0.0, 0.0]);
        settle(&mut state, &config, &world);

        let walk = MoveIntent {
            forward: 1,
            ..idle()
        };
        for _ in 0..100 {
            step_player(&mut state, &config, &world, &walk, DT);
        }
        assert_eq!(
            state.position[2],
            -4.0 + SKIN + config.radius,
            "the Cycle-1 clamp"
        );
        assert_eq!(state.position[1], 0.0, "the mechanic is disabled");
        assert_eq!(state.velocity[2], 0.0);
    }

    #[test]
    fn step_up_diagonal_low_corner() {
        // Challenge F6: two adjacent LOW boxes met diagonally — the
        // step-up may fire on either axis in one tick; the ascent is
        // clean, supported, and drift-free across the seam.
        let config = PlayerConfig {
            step_height: 0.55,
            ..PlayerConfig::default()
        };
        // Centered on the 45° approach path (x = -z from the spawn),
        // so the diagonal meets the z-face squarely within the pair's
        // x-span and crosses the seam on top.
        let world = CollisionWorld::new(0.0)
            .with_box([5.0, 0.0, -5.0], 2.0, 0.5)
            .with_box([3.0, 0.0, -5.0], 2.0, 0.5);
        let mut state = PlayerState::spawn([0.0, 0.0, 0.0]);
        settle(&mut state, &config, &world);

        let diagonal = MoveIntent {
            forward: 1,
            strafe: 1,
            ..idle()
        };
        let mut climbed = false;
        for _ in 0..150 {
            step_player(&mut state, &config, &world, &diagonal, DT);
            if state.position[1] > 0.4 {
                climbed = true;
                break;
            }
        }
        assert!(climbed, "the diagonal low corner is climbed");
        assert_eq!(state.position[1], 0.5 + SKIN);
        for _ in 0..20 {
            step_player(&mut state, &config, &world, &diagonal, DT);
        }
        assert!(state.on_ground, "supported across the seam");
        assert_eq!(state.position[1], 0.5 + SKIN, "no height jitter");
        let (min, max) = player_box(state.position, &config);
        assert!(!world.box_overlaps(min, max));
        assert!(
            state.position[2] < -4.0,
            "progressed across the tops: {}",
            state.position[2]
        );
    }

    #[test]
    fn segment_hit_returns_the_exact_first_face() {
        let world = CollisionWorld::new(0.0).with_box([0.0, 0.0, 0.0], 2.0, 2.0);
        // Straight through along -Z: the near face z = 1 over the
        // 10 m segment → t = 0.4 exactly.
        assert_eq!(
            world.segment_hit([0.0, 1.0, 5.0], [0.0, 1.0, -5.0], 0.0),
            Some(0.4)
        );
        // The nearer of two boxes wins.
        let world = CollisionWorld::new(0.0)
            .with_box([0.0, 0.0, -10.0], 2.0, 2.0)
            .with_box([0.0, 0.0, -20.0], 2.0, 2.0);
        assert_eq!(
            world.segment_hit([0.0, 1.0, 0.0], [0.0, 1.0, -30.0], 0.0),
            Some(0.3),
            "the near box's face z = -9 over 30 m"
        );
        // The margin expands the faces.
        let t = world
            .segment_hit([0.0, 1.0, 0.0], [0.0, 1.0, -30.0], 0.5)
            .expect("still hit");
        assert!((t - 8.5 / 30.0).abs() < 1e-6, "expanded near face z = -8.5");
    }

    #[test]
    fn segment_hit_registers_thin_walls_without_aliasing() {
        // Challenge F2's adversarial: a wall thinner than any
        // plausible march step — the exact slab form cannot step
        // over it.
        let world = CollisionWorld::new(0.0).with_collider(StaticCollider {
            min: [-10.0, 0.0, -0.1],
            max: [10.0, 3.0, 0.1],
        });
        assert_eq!(
            world.segment_hit([0.0, 1.5, 5.0], [0.0, 1.5, -5.0], 0.0),
            Some(0.49),
            "the 0.2 m wall registers: entry z = 0.1 → t = 0.49"
        );
        // Clear lines miss entirely.
        assert_eq!(
            world.segment_hit([0.0, 4.5, 5.0], [0.0, 4.5, -5.0], 0.0),
            None,
            "above the wall"
        );
        assert_eq!(
            world.segment_hit([12.0, 1.5, 5.0], [12.0, 1.5, -5.0], 0.0),
            None,
            "beside the wall"
        );
    }

    #[test]
    fn segment_hit_from_inside_clamps_to_zero() {
        let world = CollisionWorld::new(0.0).with_box([0.0, 0.0, 0.0], 2.0, 2.0);
        assert_eq!(
            world.segment_hit([0.0, 1.0, 0.0], [0.0, 1.0, -5.0], 0.0),
            Some(0.0),
            "a segment starting inside reports the origin"
        );
        assert_eq!(
            world.segment_hit([0.0, 1.0, 5.0], [0.0, 1.0, -5.0], 0.0),
            Some(0.4)
        );
    }

    #[test]
    fn camera_obstruction_pulls_in() {
        let config = PlayerConfig::default();
        // A wall BEHIND the player (yaw 0 looks -Z; the TP camera
        // sits at +Z behind the eye), far enough that the pull-in
        // lands beyond the min-distance floor.
        let world = CollisionWorld::new(0.0).with_box([0.0, 0.0, 5.0], 4.0, 4.0);
        let mut state = PlayerState::spawn([0.0, 0.0, 0.0]);
        state.camera_mode = CameraMode::ThirdPerson;
        settle(&mut state, &config, &world);

        let desired = camera_pose(&state, &config);
        let pose = camera_pose_obstructed(&state, &config, &world);
        assert!(
            pose.position[2] < desired.position[2],
            "pulled in toward the eye: {} vs {}",
            pose.position[2],
            desired.position[2]
        );
        assert!(
            pose.position[2] > 2.0 && pose.position[2] < 4.5,
            "between the wall and the desired distance: {}",
            pose.position[2]
        );
        let eye = [0.0, config.eye_height, 0.0];
        let distance = dist3(pose.position, eye);
        assert!(
            distance >= config.tp_min_distance - 0.01,
            "never inside the floor: {distance}"
        );
        assert!(distance < config.tp_distance, "strictly pulled in");
    }

    #[test]
    fn camera_unobstructed_matches_desired() {
        let config = PlayerConfig::default();
        let world = CollisionWorld::new(0.0);
        let mut state = PlayerState::spawn([0.0, 0.0, 0.0]);
        state.camera_mode = CameraMode::ThirdPerson;
        state.yaw = 0.7;
        let pose = camera_pose_obstructed(&state, &config, &world);
        let desired = camera_pose(&state, &config);
        assert_eq!(
            bits(pose.position),
            bits(desired.position),
            "a clear line leaves the desired pose untouched"
        );
        // First person passes through as-is.
        state.camera_mode = CameraMode::FirstPerson;
        assert_eq!(
            bits(camera_pose_obstructed(&state, &config, &world).position),
            bits(camera_pose(&state, &config).position)
        );
    }

    #[test]
    fn camera_min_distance_floor() {
        let config = PlayerConfig::default();
        // The eye walled in behind: the hit lands at the origin and
        // the camera sits AT the floor — the documented residual
        // (possibly inside the wedged geometry).
        let world = CollisionWorld::new(0.0).with_collider(StaticCollider {
            min: [-10.0, 0.0, 0.5],
            max: [10.0, 6.0, 10.0],
        });
        let mut state = PlayerState::spawn([0.0, 0.0, 0.0]);
        state.camera_mode = CameraMode::ThirdPerson;
        settle(&mut state, &config, &world);

        let pose = camera_pose_obstructed(&state, &config, &world);
        let eye = [0.0, config.eye_height, 0.0];
        let distance = dist3(pose.position, eye);
        assert!(
            (distance - config.tp_min_distance).abs() < 0.01,
            "the floor binds: {distance}"
        );
    }

    /// Cycle-4 C §8, the movement discriminator: walking into a 45°
    /// cover at z = 2 (the mid-face band) stops the player at the
    /// TRUE rotated face — SKIN-clear of the actual shape, roughly
    /// two meters PAST where the world-AABB stand-in would stop it.
    /// The stand-in control arm (the same walk against
    /// `world_aabb()`) is the in-vivo form of the GO-forbidden
    /// "rotated visual over an axis-aligned hitbox" defect: under the
    /// stand-in the player stops at the band edge and the gap is
    /// visible. (At z = 0 the approach contacts the AABB-band edge —
    /// the same clamp under both representations — so z = 2 is the
    /// discriminating line.)
    #[test]
    fn step_player_stops_at_the_true_rotated_face_not_the_aabb_gap() {
        let config = PlayerConfig::default();
        let rotated = RotatedCollider::new(
            [0.0, 1.0, 0.0],
            [2.0, 1.0, 2.0],
            std::f32::consts::FRAC_PI_4,
        )
        .unwrap();
        let world = CollisionWorld::new(0.0).with_rotated_collider(Some(rotated));
        let mut state = PlayerState::spawn([-6.0, 0.0, 2.0]);
        state.yaw = -std::f32::consts::FRAC_PI_2; // facing world +X
        settle(&mut state, &config, &world);
        let start_x = state.position[0];
        for _ in 0..120 {
            step_player(
                &mut state,
                &config,
                &world,
                &MoveIntent {
                    forward: 1,
                    ..idle()
                },
                DT,
            );
        }
        let (min, max) = player_box(state.position, &config);
        assert!(
            !rotated.overlaps_box(min, max),
            "SKIN-clear of the true shape"
        );
        assert!(
            max[0] > -2.0,
            "the player pressed past the stand-in gap: max.x = {}",
            max[0]
        );
        assert!(
            state.position[0] > start_x + 2.0,
            "the walk actually happened"
        );
        // The control arm: the same walk against the AABB stand-in.
        let stand_in = CollisionWorld::new(0.0).with_collider(rotated.world_aabb());
        let mut control = PlayerState::spawn([-6.0, 0.0, 2.0]);
        control.yaw = -std::f32::consts::FRAC_PI_2;
        settle(&mut control, &config, &stand_in);
        for _ in 0..120 {
            step_player(
                &mut control,
                &config,
                &stand_in,
                &MoveIntent {
                    forward: 1,
                    ..idle()
                },
                DT,
            );
        }
        let (_, control_max) = player_box(control.position, &config);
        assert!(
            control_max[0] < max[0] - 1.0,
            "the stand-in stops more than a meter earlier: {} vs {}",
            control_max[0],
            max[0]
        );
    }

    /// Cycle-4 C §8, no-tunneling: one meter of movement per tick
    /// (sprint 60 m/s) against a 45° wall whose extent along the
    /// approach axis is ~0.28 m — far beyond any sampled collision's
    /// ability to catch. The swept per-axis TOI stops the player
    /// SKIN-short of the TRUE face every tick; the box never passes
    /// through, at any speed.
    #[test]
    fn step_player_never_tunnels_through_a_thin_rotated_wall_at_speed() {
        let config = PlayerConfig {
            sprint_speed: 60.0,
            ..Default::default()
        };
        let wall = RotatedCollider::from_base_yaw(
            [0.0, 0.0, 0.0],
            0.4,
            6.0,
            3.0,
            std::f32::consts::FRAC_PI_4,
        )
        .unwrap();
        let world = CollisionWorld::new(0.0).with_rotated_collider(Some(wall));
        let mut state = PlayerState::spawn([-5.0, 0.0, 0.0]);
        state.yaw = -std::f32::consts::FRAC_PI_2;
        settle(&mut state, &config, &world);
        for tick in 0..40 {
            step_player(
                &mut state,
                &config,
                &world,
                &MoveIntent {
                    forward: 1,
                    sprint: true,
                    ..idle()
                },
                DT,
            );
            let (min, max) = player_box(state.position, &config);
            assert!(
                !wall.overlaps_box(min, max),
                "tick {tick}: never inside the wall at {:?}",
                state.position
            );
        }
        assert!(
            state.position[0] + config.radius < -0.2,
            "the player rests against the true face, left of the wall: {:?}",
            state.position
        );
        assert!(
            state.position[0] > -1.5,
            "and not flung back by a conservative gap: {:?}",
            state.position
        );
    }

    /// Cycle-4 C §8, step-up over a rotated top: approaching the 0.5 m
    /// 45° slab at z = 0.7 (the mid-face, where the local-X axis
    /// governs contact) climbs exactly like the axis-aligned case —
    /// the raised probe clears (tops are horizontal), the raised sweep
    /// is unclamped above the slab, and the snap-down lands SKIN above
    /// the true top. DOCUMENTED LIMITATION (the corner reaches): near
    /// the diagonal face's ends the contact is governed by the
    /// world-X band axis and the one-probe climb cannot land on the
    /// top (the footprint has not reached the SAT overlap region) —
    /// the player slides in axis-decomposed probes instead; the
    /// nonpenetration invariant holds everywhere (pinned by the
    /// chaotic-walk test below).
    #[test]
    fn step_player_climbs_onto_a_rotated_top_at_the_mid_face() {
        let config = PlayerConfig {
            step_height: 0.55,
            ..Default::default()
        };
        let slab = RotatedCollider::from_base_yaw(
            [1.5, 0.0, 0.0],
            2.0,
            2.0,
            0.5,
            std::f32::consts::FRAC_PI_4,
        )
        .unwrap();
        let world = CollisionWorld::new(0.0).with_rotated_collider(Some(slab));
        let mut state = PlayerState::spawn([-1.5, 0.0, 0.7]);
        state.yaw = -std::f32::consts::FRAC_PI_2;
        settle(&mut state, &config, &world);
        let mut climbed = false;
        for _ in 0..40 {
            step_player(
                &mut state,
                &config,
                &world,
                &MoveIntent {
                    forward: 1,
                    ..idle()
                },
                DT,
            );
            let (min, max) = player_box(state.position, &config);
            assert!(!slab.overlaps_box(min, max), "never inside the slab");
            if state.on_ground && state.position[1] > 0.4 {
                climbed = true;
            }
        }
        assert!(climbed, "the player stood on the rotated top");
        assert!(state.on_ground);
        assert!(
            (state.position[1] - (0.5 + SKIN)).abs() < 1e-2,
            "standing SKIN above the top: {}",
            state.position[1]
        );
        assert!(
            state.position[0] > 0.3,
            "moved onto the slab: {:?}",
            state.position
        );
    }

    /// Cycle-4 C §8, sliding: pressed against the 45° face, a
    /// diagonal intent exactly parallel to the face slides the player
    /// along it — the X arm re-clamps against the receding face every
    /// tick while the Z arm advances freely (the documented
    /// axis-decomposed staircase: sub-0.05 m steps at the demo speed,
    /// never through the face, the local separation held within a
    /// tick's worth of the skin).
    #[test]
    fn step_player_slides_along_a_rotated_face_in_axis_decomposed_steps() {
        let config = PlayerConfig::default();
        let rotated = RotatedCollider::new(
            [0.0, 1.0, 0.0],
            [2.0, 1.0, 2.0],
            std::f32::consts::FRAC_PI_4,
        )
        .unwrap();
        let world = CollisionWorld::new(0.0).with_rotated_collider(Some(rotated));
        let mut state = PlayerState::spawn([-3.0, 0.0, 1.0]);
        state.yaw = -std::f32::consts::FRAC_PI_2; // face +X: press into the face
        settle(&mut state, &config, &world);
        for _ in 0..40 {
            step_player(
                &mut state,
                &config,
                &world,
                &MoveIntent {
                    forward: 1,
                    ..idle()
                },
                DT,
            );
        }
        let (min, max) = player_box(state.position, &config);
        assert!(!rotated.overlaps_box(min, max), "pressed, SKIN-clear");
        let (pressed_x, pressed_z) = (state.position[0], state.position[2]);
        // The slide: yaw −3π/4 makes the intent direction (+X, +Z) —
        // exactly parallel to the upper-left face (whose direction is
        // (1, 1)/√2).
        state.yaw = -3.0 * std::f32::consts::FRAC_PI_4;
        for _ in 0..30 {
            step_player(
                &mut state,
                &config,
                &world,
                &MoveIntent {
                    forward: 1,
                    ..idle()
                },
                DT,
            );
            let (min, max) = player_box(state.position, &config);
            assert!(
                !rotated.overlaps_box(min, max),
                "never through the face while sliding"
            );
        }
        assert!(
            state.position[2] > pressed_z + 1.0,
            "progressed along the face: z {pressed_z} → {}",
            state.position[2]
        );
        assert!(
            state.position[0] > pressed_x + 0.8,
            "the X arm tracked the receding face: x {pressed_x} → {}",
            state.position[0]
        );
        // Still pressed: the local-X separation sits within a tick's
        // worth of the skin (never negative beyond it).
        let local = rotated.to_local([state.position[0], 1.0, state.position[2]]);
        let (c, s) = rotated.frame();
        let separation = local[0].abs() - (2.0 + config.radius * (c.abs() + s.abs()));
        assert!(
            separation > -2.0 * SKIN && separation < 0.05,
            "pressed against the face while sliding: {separation}"
        );
    }

    /// Cycle-4 C §8, the LOS pairs at the world level: the direct ray
    /// through the rotated cover is blocked at the TRUE edge (the
    /// analytic entry t), the same-origin ray at a steeper angle is
    /// clear (it passes the diamond's corner — the blocked-vs-clear
    /// pair differing only by angle), the world-AABB stand-in
    /// over-blocks the parallel-offset ray the true shape clears (the
    /// LOS anti-hitbox discriminator), and a mixed world consults
    /// both collider kinds with the nearest hit winning.
    #[test]
    fn los_blocked_and_clear_pairs_through_one_rotated_cover() {
        let rotated = RotatedCollider::new(
            [0.0, 1.0, 0.0],
            [2.0, 1.0, 2.0],
            std::f32::consts::FRAC_PI_4,
        )
        .unwrap();
        let world = CollisionWorld::new(0.0).with_rotated_collider(Some(rotated));
        // The blocked ray: along +X at eye height through z = 2 — the
        // true diamond's upper-left edge crosses z = 2 at x = −0.83.
        let t = world
            .segment_hit([-10.0, 1.0, 2.0], [10.0, 1.0, 2.0], 0.0)
            .expect("the cover blocks the direct line");
        assert!((t - 0.4586).abs() < 2e-3, "enters at the true edge: {t}");
        // The clear pair from the SAME origin, differing only in
        // angle: aimed to pass above the diamond's upper corner.
        assert_eq!(
            world.segment_hit([-10.0, 1.0, 2.0], [10.0, 1.0, 5.0], 0.0),
            None
        );
        // The LOS anti-hitbox discriminator: a ray parallel to the
        // face, offset beyond it — the true shape clears it, the
        // world-AABB stand-in blocks it.
        assert_eq!(
            world.segment_hit([-3.0, 1.0, 0.2], [2.0, 1.0, 5.2], 0.0),
            None
        );
        let stand_in = CollisionWorld::new(0.0).with_collider(rotated.world_aabb());
        assert!(
            stand_in
                .segment_hit([-3.0, 1.0, 0.2], [2.0, 1.0, 5.2], 0.0)
                .is_some(),
            "the stand-in over-blocks the clear line"
        );
        // The mixed world: both collider kinds consulted; the rotated
        // cover wins the minimum over the axis-aligned block behind
        // it. (This ray spans −10 → 20, so the cover's entry — the
        // same edge point the direct ray hits at t ≈ 0.4586 over its
        // 20-unit span — lands at t ≈ 0.3057 over this 30-unit span;
        // the axis-aligned block behind it would hit at t ≈ 0.617.)
        let mixed = CollisionWorld::new(0.0)
            .with_box([9.0, 0.0, 0.0], 1.0, 3.0)
            .with_rotated_collider(Some(rotated));
        let t_mixed = mixed
            .segment_hit([-10.0, 1.0, 2.0], [20.0, 1.0, 2.0], 0.0)
            .expect("the mixed world hits");
        assert!(
            (t_mixed - 0.3057).abs() < 2e-3,
            "the rotated cover wins the min-t: {t_mixed}"
        );
    }

    /// Cycle-4 C §8, the movement invariant: a long chaotic scripted
    /// walk (sprints, jumps, direction changes, a rotating yaw)
    /// through a mixed world (an axis-aligned box + three rotated
    /// colliders at different yaws, including a thin wall and a low
    /// slab) never ends a tick inside any collider — the swept
    /// SKIN-short clamping and the depenetration hold the
    /// nonpenetration invariant everywhere, whatever the corner and
    /// step-up probe behavior does along the way.
    #[test]
    fn a_chaotic_walk_never_penetrates_the_rotated_world() {
        let config = PlayerConfig {
            sprint_speed: 9.0,
            step_height: 0.45,
            ..Default::default()
        };
        let big = RotatedCollider::new(
            [0.0, 1.0, 0.0],
            [2.0, 1.0, 2.0],
            std::f32::consts::FRAC_PI_4,
        )
        .unwrap();
        let thin = RotatedCollider::from_base_yaw([3.0, 0.0, -2.0], 0.4, 5.0, 2.0, 0.6).unwrap();
        let slab = RotatedCollider::from_base_yaw(
            [-2.0, 0.0, 3.0],
            2.5,
            2.5,
            0.5,
            -std::f32::consts::FRAC_PI_4,
        )
        .unwrap();
        let world = CollisionWorld::new(0.0)
            .with_box([0.0, 0.0, -5.0], 1.0, 2.0)
            .with_rotated_collider(Some(big))
            .with_rotated_collider(Some(thin))
            .with_rotated_collider(Some(slab));
        let mut state = PlayerState::spawn([-6.0, 0.0, -6.0]);
        let script: [(i8, i8, bool, bool); 8] = [
            (1, 0, false, false),
            (1, 1, true, false),
            (0, 1, false, true),
            (1, 0, true, true),
            (-1, 1, false, false),
            (1, -1, true, false),
            (0, -1, false, true),
            (1, 0, false, false),
        ];
        for round in 0..16 {
            state.yaw += 0.37; // a non-cardinal, ever-changing heading
            for (forward, strafe, sprint, jump) in script {
                let intent = MoveIntent {
                    forward,
                    strafe,
                    sprint,
                    jump_pressed: jump,
                    ..idle()
                };
                for _ in 0..4 {
                    step_player(&mut state, &config, &world, &intent, DT);
                    let (min, max) = player_box(state.position, &config);
                    assert!(
                        !world.box_overlaps(min, max),
                        "round {round}: never inside any collider at {:?}",
                        state.position
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // P2 L5 — the static broad phase battery (the design's L5
    // acceptance row: exhaustive small-world equivalence, ties
    // explicit, the gate boundary, the fail-closed domains, and the
    // scaling evidence).
    // -----------------------------------------------------------------

    /// A deterministic xorshift64* stream for fixture generation
    /// (std-only, seeded per world, so every run of the battery
    /// builds the IDENTICAL population — the no-RNG discipline
    /// applied to tests).
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

    /// One fixture placement: a jittered lattice over ±60 m (dense
    /// enough for real candidate sets), carrying the adversarial
    /// placement classes the broad phase must survive — every 7th
    /// base lands EXACTLY on cell boundaries (multiples of
    /// [`GRID_CELL_SIZE`], the floor-edge class), and every 13th
    /// stretches ~8× farther out (sparse cells, mostly-empty walks).
    fn fixture_base(rng: &mut FixtureRng, index: usize) -> [f32; 3] {
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
            [
                (x / GRID_CELL_SIZE).round() * GRID_CELL_SIZE,
                y,
                (z / GRID_CELL_SIZE).round() * GRID_CELL_SIZE,
            ]
        } else {
            [x, y, z]
        }
    }

    /// A deterministic mixed fixture world: `aabb_count` axis-aligned
    /// boxes plus `rotated_count` yaw-rotated prisms, with inverted
    /// literals (every 11th — the linear narrow phases consult
    /// inverted intervals too, so the grid must cover the sorted
    /// span), yaw-0 rotated colliders (every 5th — exact sin/cos, the
    /// exact-tie-capable class), and arbitrary yaws elsewhere.
    fn fixture_world(seed: u64, aabb_count: usize, rotated_count: usize) -> CollisionWorld {
        let mut rng = FixtureRng(seed);
        let mut world = CollisionWorld::new(0.0);
        for index in 0..aabb_count {
            let base = fixture_base(&mut rng, index);
            let footprint = rng.range(0.4, 3.0);
            let height = rng.range(0.3, 4.0);
            if index % 11 == 3 {
                let min = [
                    base[0] - footprint / 2.0,
                    base[1],
                    base[2] - footprint / 2.0,
                ];
                let max = [
                    base[0] + footprint / 2.0,
                    base[1] + height,
                    base[2] + footprint / 2.0,
                ];
                world = world.with_collider(StaticCollider { min: max, max: min });
            } else {
                world = world.with_box(base, footprint, height);
            }
        }
        for index in 0..rotated_count {
            let base = fixture_base(&mut rng, index + aabb_count);
            let footprint = rng.range(0.4, 2.5);
            let height = rng.range(0.3, 3.0);
            let yaw = if index.is_multiple_of(5) {
                0.0
            } else {
                rng.range(-3.1, 3.1)
            };
            world = world.with_rotated_collider(RotatedCollider::from_base_yaw(
                base,
                footprint,
                rng.range(0.4, 2.5),
                height,
                yaw,
            ));
        }
        world
    }

    /// Forces the broad phase on (building the grid from the current
    /// lists, regardless of the gate) or off — the battery's control:
    /// the SAME collider set through both paths must produce
    /// bit-identical results. Returns whether the grid is active
    /// after the call (a poisoned world cannot be forced on).
    fn force_broad_phase(world: &mut CollisionWorld, on: bool) -> bool {
        if on {
            match build_static_grid(&world.colliders, &world.rotated) {
                Some(grid) => {
                    world.grid = BroadPhase::Grid(grid);
                    true
                }
                None => {
                    world.grid = BroadPhase::Unusable;
                    false
                }
            }
        } else {
            world.grid = BroadPhase::Off;
            false
        }
    }

    /// Bit-level equality — the battery's claim IS bit-identity, so
    /// even NaN results compare by their bit patterns.
    fn same_f32(a: f32, b: f32) -> bool {
        a.to_bits() == b.to_bits()
    }

    fn same_option_f32(a: Option<f32>, b: Option<f32>) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => same_f32(a, b),
            (None, None) => true,
            _ => false,
        }
    }

    fn same_option_vec3(a: Option<[f32; 3]>, b: Option<[f32; 3]>) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => a.iter().zip(b.iter()).all(|(a, b)| same_f32(*a, *b)),
            (None, None) => true,
            _ => false,
        }
    }

    fn same_sweep(a: (f32, bool), b: (f32, bool)) -> bool {
        same_f32(a.0, b.0) && a.1 == b.1
    }

    /// The L5 equivalence battery (the design's L5 acceptance row):
    /// for fixture populations BELOW and ABOVE the gate, every query
    /// arm through the grid path vs the exact linear path on the
    /// IDENTICAL collider set — bit-for-bit, ties included. The
    /// below-gate arm force-activates the grid (the gate otherwise
    /// keeps those worlds linear); the above-gate arm force-disables
    /// it on the control clone. Query placements mix pseudo-random
    /// street points, collider-interior spawns (real depenetration
    /// pushes), face-adjacent sweeps, and both segment regimes of the
    /// [`GRID_CELL_WASTE_FACTOR`] guard (the 1000+100 population
    /// lifts the crossover so the long diagonal runs the GRID arm).
    #[test]
    fn broad_phase_equivalence_battery() {
        let config = PlayerConfig::default();
        for (aabb_count, rotated_count) in [(40usize, 17usize), (96, 31), (150, 40), (1000, 100)] {
            for seed in [0x5EED_0001u64, 0x5EED_0010, 0x5EED_0110] {
                let world = fixture_world(seed, aabb_count, rotated_count);
                let mut linear = world.clone();
                force_broad_phase(&mut linear, false);
                let mut grid = world.clone();
                let grid_active = force_broad_phase(&mut grid, true);
                assert!(
                    grid_active,
                    "seed {seed}: the fixture must be grid-usable \
                     (every AABB finite and in-domain)"
                );
                let label = format!("{aabb_count}+{rotated_count} @ {seed}");

                // Pseudo-random street placements, player-sized boxes.
                let mut rng = FixtureRng(seed ^ 0x9E37_79B9_7F4A_7C15);
                for query in 0..48 {
                    let base = fixture_base(&mut rng, query);
                    let (min, max) = player_box(base, &config);
                    assert_eq!(
                        grid.box_overlaps(min, max),
                        linear.box_overlaps(min, max),
                        "box_overlaps {label} q{query}"
                    );
                    assert!(
                        same_option_vec3(
                            grid.depenetration(min, max),
                            linear.depenetration(min, max)
                        ),
                        "depenetration {label} q{query}"
                    );
                    for axis in 0..3 {
                        for delta in [0.2, 1.7, -0.3, -9.0, 55.0, -55.0] {
                            assert!(
                                same_sweep(
                                    grid.sweep_axis(min, max, axis, delta),
                                    linear.sweep_axis(min, max, axis, delta)
                                ),
                                "sweep_axis {label} q{query} a{axis} d{delta}"
                            );
                        }
                    }
                    // Segments from the query base: a short
                    // camera-like ray and a long cross-world diagonal
                    // (which guard regime depends on the population),
                    // each at three margins.
                    for (to, kind) in [
                        ([base[0] + 4.5, base[1] + 0.3, base[2] - 1.2], "short"),
                        (
                            [base[0] + 120.0, base[1] + 0.5, base[2] - 120.0],
                            "diagonal",
                        ),
                    ] {
                        for margin in [0.0, CAMERA_MARGIN, 0.75] {
                            assert!(
                                same_option_f32(
                                    grid.segment_hit(base, to, margin),
                                    linear.segment_hit(base, to, margin)
                                ),
                                "segment_hit {kind} {label} q{query} m{margin}"
                            );
                        }
                    }
                }

                // Collider-interior spawns: overlapping boxes, real
                // depenetration pushes, and face-adjacent sweeps.
                for (index, col) in world.colliders().iter().enumerate().step_by(3) {
                    let center = [
                        (col.min[0] + col.max[0]) / 2.0,
                        (col.min[1] + col.max[1]) / 2.0,
                        (col.min[2] + col.max[2]) / 2.0,
                    ];
                    let (min, max) = player_box(center, &config);
                    assert!(
                        same_option_vec3(
                            grid.depenetration(min, max),
                            linear.depenetration(min, max)
                        ),
                        "interior depenetration {label} c{index}"
                    );
                    assert_eq!(
                        grid.box_overlaps(min, max),
                        linear.box_overlaps(min, max),
                        "interior box_overlaps {label} c{index}"
                    );
                    let mut face = col.max;
                    face[0] += SKIN * 4.0; // just past the +X face
                    let (min, max) = player_box(face, &config);
                    for delta in [0.5, -0.5] {
                        assert!(
                            same_sweep(
                                grid.sweep_axis(min, max, 0, delta),
                                linear.sweep_axis(min, max, 0, delta)
                            ),
                            "face sweep {label} c{index} d{delta}"
                        );
                    }
                }
                for (index, col) in world.rotated().iter().enumerate().step_by(2) {
                    // CA-05 consumer adaptation: the geometry read goes
                    // through the accessor (the field is private since
                    // the stale-cached-bounds fix).
                    let (min, max) = player_box(col.center(), &config);
                    assert!(
                        same_option_vec3(
                            grid.depenetration(min, max),
                            linear.depenetration(min, max)
                        ),
                        "rotated-interior depenetration {label} r{index}"
                    );
                }
            }
        }
    }

    /// The exact mathematical-tie classes (the design's L5
    /// discriminating tests): equal-magnitude depenetration pushes
    /// from DIFFERENT colliders — an observable tie, because the
    /// winning push DIRECTION differs — plus the rotated-list
    /// symmetric tie at yaw 0 (exact sin/cos, so the magnitudes are
    /// bit-equal) and the equal-face sweep tie. The first candidate
    /// in insertion order must win on BOTH paths.
    #[test]
    fn broad_phase_mathematical_ties_resolve_identically() {
        // Two walls flanking a centered player box: wall X (x ∈
        // [−2, −0.1], spanning z) yields a +X push of −0.1 + SKIN +
        // 0.35; wall Z (z ∈ [−2, −0.1], spanning x) yields a +Z push
        // of the SAME expression over the SAME values — bit-equal
        // magnitudes. Whichever wall was inserted FIRST wins.
        let tie_world = |z_first: bool| {
            let wall_x = StaticCollider {
                min: [-2.0, 0.0, -2.0],
                max: [-0.1, 3.0, 2.0],
            };
            let wall_z = StaticCollider {
                min: [-2.0, 0.0, -2.0],
                max: [2.0, 3.0, -0.1],
            };
            let (first, second) = if z_first {
                (wall_z, wall_x)
            } else {
                (wall_x, wall_z)
            };
            let mut world = CollisionWorld::new(0.0).with_collider(first);
            // Padding to the gate (distant colliders, never
            // interacting): the GRID arm runs.
            for i in 0..GRID_COLLIDER_GATE - 2 {
                world = world.with_box([400.0 + i as f32 * 4.0, 0.0, 400.0], 0.5, 0.5);
            }
            world.with_collider(second)
        };
        let (min, max) = ([-0.35, 0.0, -0.35], [0.35, 1.7, 0.35]);
        for (z_first, expected_axis, label) in [(false, 0usize, "X-first"), (true, 2, "Z-first")] {
            let world = tie_world(z_first);
            assert!(
                matches!(world.grid, BroadPhase::Grid(_)),
                "{label}: the grid arm is active"
            );
            let mut linear = world.clone();
            force_broad_phase(&mut linear, false);
            let grid_push = world.depenetration(min, max).expect("the overlap resolves");
            let linear_push = linear
                .depenetration(min, max)
                .expect("the overlap resolves");
            assert!(
                same_option_vec3(Some(grid_push), Some(linear_push)),
                "{label}: the tie resolves identically"
            );
            let other = if expected_axis == 0 { 2 } else { 0 };
            assert!(
                linear_push[expected_axis] > 0.0
                    && linear_push[other] == 0.0
                    && linear_push[1] == 0.0,
                "{label}: the FIRST wall's push wins the tie: {linear_push:?}"
            );
        }

        // The rotated-list symmetric tie at yaw 0: two identical
        // prisms flanking the player at ±1.05 — bit-equal local-X
        // depths (0.35 + 0.75 − 1.05 + SKIN), opposite directions.
        // The LOWER rotated index wins (the AABB padding has no
        // overlap here, so the rotated arm owns the tie).
        let flank = |center_x: f32| {
            RotatedCollider::new([center_x, 1.5, 0.0], [0.75, 1.5, 2.0], 0.0).unwrap()
        };
        let mut rotated_world = CollisionWorld::new(0.0)
            .with_rotated_collider(Some(flank(-1.05)))
            .with_rotated_collider(Some(flank(1.05)));
        for i in 0..GRID_COLLIDER_GATE - 2 {
            rotated_world = rotated_world.with_box([400.0 + i as f32 * 4.0, 0.0, 400.0], 0.5, 0.5);
        }
        assert!(matches!(rotated_world.grid, BroadPhase::Grid(_)));
        let mut linear = rotated_world.clone();
        force_broad_phase(&mut linear, false);
        let grid_push = rotated_world
            .depenetration(min, max)
            .expect("the flanks overlap");
        let linear_push = linear.depenetration(min, max).expect("the flanks overlap");
        assert!(same_option_vec3(Some(grid_push), Some(linear_push)));
        assert!(
            linear_push[0] > 0.0 && linear_push[1] == 0.0 && linear_push[2] == 0.0,
            "the first rotated flank wins the tie: {linear_push:?}"
        );

        // The equal-face sweep tie (value-identical by nature — both
        // walls clamp to the same allowed — but pinned): two walls at
        // the same x-plane in different z bands, both straddling the
        // player's z extent.
        let mut sweep_world = CollisionWorld::new(0.0).with_collider(StaticCollider {
            min: [3.0, 0.0, -2.0],
            max: [5.0, 3.0, 0.1],
        });
        for i in 0..GRID_COLLIDER_GATE - 2 {
            sweep_world = sweep_world.with_box([400.0 + i as f32 * 4.0, 0.0, 400.0], 0.5, 0.5);
        }
        sweep_world = sweep_world.with_collider(StaticCollider {
            min: [3.0, 0.0, -0.1],
            max: [5.0, 3.0, 2.0],
        });
        let mut linear = sweep_world.clone();
        force_broad_phase(&mut linear, false);
        let (grid_allowed, grid_hit) = sweep_world.sweep_axis(min, max, 0, 10.0);
        let (linear_allowed, linear_hit) = linear.sweep_axis(min, max, 0, 10.0);
        assert!(same_sweep(
            (grid_allowed, grid_hit),
            (linear_allowed, linear_hit)
        ));
        assert!(grid_hit && same_f32(grid_allowed, 3.0 - SKIN - 0.35));
    }

    /// The gate boundary (the L5 acceptance row): at 127 total
    /// colliders the world is below the gate (the exact linear path);
    /// the 128th push ACTIVATES the grid; the 129th keeps it
    /// (incremental insertion). A refused rotated collider never
    /// counts toward the gate. And the switch is behavior-invisible:
    /// the AT-gate world answers bit-identically to its forced-linear
    /// clone — the design's "at the threshold, the switch is correct"
    /// in both directions.
    #[test]
    fn broad_phase_gate_boundary() {
        let config = PlayerConfig::default();
        let mut below = CollisionWorld::new(0.0);
        for i in 0..64 {
            below = below.with_box(
                [
                    (i % 8) as f32 * 3.0 - 10.0,
                    0.0,
                    (i / 8) as f32 * 3.0 - 10.0,
                ],
                1.0,
                2.0,
            );
        }
        for i in 0..63 {
            below = below.with_rotated_collider(RotatedCollider::from_base_yaw(
                [(i % 7) as f32 * 3.0 - 9.0, 0.0, (i / 7) as f32 * 3.0 - 9.0],
                0.8,
                0.8,
                1.5,
                0.3,
            ));
        }
        assert_eq!(below.colliders().len() + below.rotated().len(), 127);
        assert!(matches!(below.grid, BroadPhase::Off), "127: below the gate");

        let at = below.clone().with_box([0.0, 0.0, 0.0], 1.0, 1.0);
        assert_eq!(
            at.colliders().len() + at.rotated().len(),
            GRID_COLLIDER_GATE
        );
        assert!(
            matches!(at.grid, BroadPhase::Grid(_)),
            "the 128th push activates the grid"
        );

        let above = at.clone().with_box([6.0, 0.0, 6.0], 1.0, 1.0);
        assert!(
            matches!(above.grid, BroadPhase::Grid(_)),
            "the 129th push keeps it (incremental insertion)"
        );

        // A refused rotated collider never enters the list, so it
        // never counts toward the gate.
        let mut refused = below.clone();
        refused = refused.with_rotated_collider(None);
        assert!(matches!(refused.grid, BroadPhase::Off));

        // The switch is behavior-invisible at the exact boundary.
        let mut linear = at.clone();
        force_broad_phase(&mut linear, false);
        let mut rng = FixtureRng(0xBEEF_CAFE);
        for query in 0..32 {
            let base = fixture_base(&mut rng, query);
            let (min, max) = player_box(base, &config);
            assert_eq!(
                at.box_overlaps(min, max),
                linear.box_overlaps(min, max),
                "q{query}"
            );
            assert!(same_option_vec3(
                at.depenetration(min, max),
                linear.depenetration(min, max)
            ));
            for axis in 0..3 {
                assert!(
                    same_sweep(
                        at.sweep_axis(min, max, axis, 1.3),
                        linear.sweep_axis(min, max, axis, 1.3)
                    ),
                    "q{query} a{axis}"
                );
            }
            let to = [base[0] + 20.0, base[1], base[2] - 15.0];
            assert!(same_option_f32(
                at.segment_hit(base, to, 0.1),
                linear.segment_hit(base, to, 0.1)
            ));
        }
    }

    /// The fail-closed domains (the design's §2.2.6 numerical-domain
    /// contract): a non-finite or out-of-domain collider AABB poisons
    /// the broad phase permanently (the exact linear paths); an
    /// out-of-domain QUERY extent, a negative margin, a non-finite
    /// sweep delta, and the waste-guard regime each fall back per
    /// query — behavior is bit-identical by fallback in every case.
    #[test]
    fn broad_phase_failclosed_domains() {
        let config = PlayerConfig::default();
        let healthy = fixture_world(0xFACE_1000, 150, 40);
        assert!(matches!(healthy.grid, BroadPhase::Grid(_)));

        // A non-finite collider poisons a previously-active grid...
        let poisoned = healthy.clone().with_collider(StaticCollider {
            min: [f32::NAN, 0.0, 0.0],
            max: [1.0, 1.0, 1.0],
        });
        assert!(matches!(poisoned.grid, BroadPhase::Unusable));
        // ...and it stays poisoned: further healthy pushes cannot
        // resurrect it (colliders never leave).
        let mut more = poisoned.clone();
        for i in 0..10 {
            more = more.with_box([50.0 + i as f32, 0.0, 50.0], 1.0, 1.0);
        }
        assert!(matches!(more.grid, BroadPhase::Unusable));

        // An out-of-domain coordinate (|cell| ≥ GRID_CELL_LIMIT)
        // does the same.
        let domain_edge = GRID_CELL_LIMIT as f32 * GRID_CELL_SIZE;
        let huge = healthy.clone().with_collider(StaticCollider {
            min: [domain_edge, 0.0, 0.0],
            max: [domain_edge + 1.0, 1.0, 1.0],
        });
        assert!(matches!(huge.grid, BroadPhase::Unusable));

        // The poisoned world still answers bit-identically (fallback).
        let mut linear = poisoned.clone();
        force_broad_phase(&mut linear, false);
        let (min, max) = player_box([0.0, 0.0, 0.0], &config);
        assert!(same_option_vec3(
            poisoned.depenetration(min, max),
            linear.depenetration(min, max)
        ));

        // An out-of-domain QUERY on a healthy grid falls back per
        // query: a far-out segment endpoint, and a far-spanning
        // segment that still ends inside the world.
        let mut linear = healthy.clone();
        force_broad_phase(&mut linear, false);
        let far = [domain_edge, 1.0, 0.0];
        assert!(same_option_f32(
            healthy.segment_hit(far, [far[0] + 5.0, 1.0, 0.0], 0.0),
            linear.segment_hit(far, [far[0] + 5.0, 1.0, 0.0], 0.0)
        ));
        assert!(same_option_f32(
            healthy.segment_hit(far, [0.0, 1.0, 0.0], 0.1),
            linear.segment_hit(far, [0.0, 1.0, 0.0], 0.1)
        ));

        // A negative margin takes the linear path (the validated
        // broad-phase domain) — the shrunk-interval geometry the raw
        // slab arithmetic produces, identical either way.
        assert!(same_option_f32(
            healthy.segment_hit([-30.0, 1.0, 0.0], [30.0, 1.0, 0.0], -0.5),
            linear.segment_hit([-30.0, 1.0, 0.0], [30.0, 1.0, 0.0], -0.5)
        ));

        // A non-finite sweep delta: the linear path's own NaN
        // handling, identical through the fallback.
        assert!(same_sweep(
            healthy.sweep_axis(min, max, 0, f32::NAN),
            linear.sweep_axis(min, max, 0, f32::NAN)
        ));

        // Non-finite QUERY coordinates (the raw-before-sort trap: the
        // per-axis min/max swallow NaN, so the fallback must engage
        // at the source — a NaN endpoint makes the linear slab treat
        // that axis as an infinite slab, a NaN query box compares
        // false everywhere). Every arm, identical through the
        // fallback.
        let nan_min = [f32::NAN, 0.0, 0.0];
        let nan_max = [2.0, 2.0, 2.0];
        assert_eq!(
            healthy.box_overlaps(nan_min, nan_max),
            linear.box_overlaps(nan_min, nan_max)
        );
        assert!(same_option_vec3(
            healthy.depenetration(nan_min, nan_max),
            linear.depenetration(nan_min, nan_max)
        ));
        for axis in 0..3 {
            assert!(
                same_sweep(
                    healthy.sweep_axis(nan_min, nan_max, axis, 1.0),
                    linear.sweep_axis(nan_min, nan_max, axis, 1.0)
                ),
                "NaN-box sweep a{axis}"
            );
            assert!(
                same_sweep(
                    healthy.sweep_axis(min, max, axis, 1.0),
                    linear.sweep_axis(min, max, axis, 1.0),
                ),
                "sanity: the finite probe stays identical a{axis}"
            );
        }
        assert!(same_option_f32(
            healthy.segment_hit([f32::NAN, 1.0, 0.0], [10.0, 1.0, 0.0], 0.1),
            linear.segment_hit([f32::NAN, 1.0, 0.0], [10.0, 1.0, 0.0], 0.1)
        ));

        // The waste-guard regime: a query box spanning the whole
        // fixture (far more cells than GRID_CELL_WASTE_FACTOR per
        // collider) takes the linear path — identical results.
        let wide = ([-500.0, -5.0, -500.0], [500.0, 10.0, 500.0]);
        assert_eq!(
            healthy.box_overlaps(wide.0, wide.1),
            linear.box_overlaps(wide.0, wide.1)
        );
        assert!(same_option_vec3(
            healthy.depenetration(wide.0, wide.1),
            linear.depenetration(wide.0, wide.1)
        ));
    }

    /// Q01's amplification case, the bound proof: a collider spanning
    /// (nearly) the whole cell domain — every coordinate finite and
    /// IN domain, coverage ~(4×10⁶)³ = 6.4×10¹⁹ cells, beyond u64 and
    /// beyond any allocation — produces ONE oversized entry, not the
    /// per-cell amplification the unbounded insertion loop would
    /// attempt (effectively a build-time hang/OOM for ONE collider).
    /// The resident-entry bound holds at the population: the cell map
    /// is IDENTICAL to the same world without the huge collider (it
    /// contributed zero cell entries), and total entries stay within
    /// ( [`GRID_OVERSIZED_CELL_LIMIT`] + 1 ) per collider. The huge
    /// collider is NOT a poison (in-domain and finite — conservative
    /// correctness is preserved through the universally-consulted
    /// oversized list) while an out-of-domain collider still poisons
    /// (the fail-closed domain contract, unchanged), and queries over
    /// the huge world answer bit-identically to the linear oracle.
    #[test]
    fn broad_phase_oversized_collider_is_bounded_not_amplified() {
        let config = PlayerConfig::default();
        // 128 small boxes (the gate): the control grid.
        let mut control = CollisionWorld::new(0.0);
        for i in 0..GRID_COLLIDER_GATE {
            control = control.with_box(
                [(i % 16) as f32 * 3.0, 0.0, (i / 16) as f32 * 3.0],
                1.0,
                2.0,
            );
        }
        assert!(matches!(control.grid, BroadPhase::Grid(_)));
        let BroadPhase::Grid(control_grid) = &control.grid else {
            unreachable!("checked above");
        };
        let control_entries: usize = control_grid.cells.values().map(Vec::len).sum();

        // The whole-domain collider: ±4,000,000 m per axis — every
        // cell |2×10⁶| < GRID_CELL_LIMIT, so it is IN domain and
        // finite; its coverage (4,000,001)³ ≈ 6.4×10¹⁹ cells exceeds
        // u64 itself, which is over the cap by any measure. Both
        // routes to the oversized list are exercised: this one
        // (checked-multiply overflow) and, below, a ~10⁹-cell
        // coverage that fits u64 but exceeds the cap.
        let world = control.clone().with_collider(StaticCollider {
            min: [-4_000_000.0; 3],
            max: [4_000_000.0; 3],
        });
        let BroadPhase::Grid(grid) = &world.grid else {
            panic!(
                "an in-domain finite oversized collider keeps the grid \
                 ACTIVE (oversized, never a poison)"
            );
        };
        assert_eq!(
            grid.oversized,
            vec![(false, GRID_COLLIDER_GATE as u32)],
            "the whole-domain collider is ONE oversized entry"
        );
        assert_eq!(
            grid.cells, control_grid.cells,
            "the whole-domain collider contributed ZERO cell entries — \
             the cell map is identical to the control world's"
        );
        let entries: usize = grid.cells.values().map(Vec::len).sum();
        let total = world.colliders().len() + world.rotated().len();
        assert_eq!(entries, control_entries);
        assert!(
            entries + grid.oversized.len() <= (GRID_OVERSIZED_CELL_LIMIT as usize + 1) * total,
            "the resident-entry bound: (cap + 1) per collider — got \
             {entries} cell entries + {} oversized over {total} colliders",
            grid.oversized.len()
        );

        // The count-overflow-free route: a ±1000 m collider covers
        // (1001)³ ≈ 10⁹ cells — representable in u64, far over the
        // cap — same ONE oversized entry, same untouched cell map.
        let moderate = control.clone().with_collider(StaticCollider {
            min: [-1000.0; 3],
            max: [1000.0; 3],
        });
        let BroadPhase::Grid(moderate_grid) = &moderate.grid else {
            panic!("the in-domain oversized collider keeps the grid active");
        };
        assert_eq!(
            moderate_grid.oversized,
            vec![(false, GRID_COLLIDER_GATE as u32)]
        );
        assert_eq!(moderate_grid.cells, control_grid.cells);

        // The fail-closed domain contract is untouched: an
        // out-of-domain collider still poisons (permanently linear),
        // exactly as before the oversized mechanism existed.
        let domain_edge = GRID_CELL_LIMIT as f32 * GRID_CELL_SIZE;
        let poisoned = control.clone().with_collider(StaticCollider {
            min: [domain_edge, 0.0, 0.0],
            max: [domain_edge + 1.0, 1.0, 1.0],
        });
        assert!(
            matches!(poisoned.grid, BroadPhase::Unusable),
            "out-of-domain still poisons — the oversized list never \
             weakens the fail-closed domain rules"
        );

        // Equivalence with the whole-domain collider present: every
        // query box overlaps it, depenetration resolves inside it,
        // segments start inside it — bit-identical to the linear
        // oracle through the universally-consulted oversized list.
        let mut linear = world.clone();
        force_broad_phase(&mut linear, false);
        let (min, max) = player_box([0.0, 5.0, 0.0], &config);
        assert!(world.box_overlaps(min, max), "inside the huge collider");
        assert_eq!(world.box_overlaps(min, max), linear.box_overlaps(min, max));
        assert!(same_option_vec3(
            world.depenetration(min, max),
            linear.depenetration(min, max)
        ));
        assert!(same_sweep(
            world.sweep_axis(min, max, 1, -4.0),
            linear.sweep_axis(min, max, 1, -4.0)
        ));
        assert!(same_option_f32(
            world.segment_hit([0.0, 5.0, 0.0], [0.0, -5.0, 0.0], 0.0),
            linear.segment_hit([0.0, 5.0, 0.0], [0.0, -5.0, 0.0], 0.0)
        ));
    }

    /// Q01's equivalence case: queries over worlds WITH oversized
    /// colliders present are bit-identical to the linear oracle — the
    /// oversized entries enter the same re-sort-to-insertion-order
    /// candidate discipline. The DISCRIMINATING ORDER TIE: an
    /// oversized slab (insertion index 0) and a local box (index 1)
    /// offer bit-equal depenetration magnitudes with DIFFERENT
    /// directions (+Y vs +X) — the first-in-insertion-order candidate
    /// must win on BOTH paths, so an implementation that appended the
    /// oversized entries after the sort (or skipped the re-sort for
    /// them) would flip the tie and fail this test. Also pins the
    /// sweep clamp and segment hit ON the oversized slab (the
    /// universally-consulted candidate), plus a pseudo-random query
    /// battery over the mixed world.
    #[test]
    fn broad_phase_oversized_queries_bit_identical_with_order_tie() {
        let config = PlayerConfig::default();
        // The oversized slab first (insertion index 0): y ∈ [0, 2],
        // x/z ±4,000,000 m — coverage ~3.2×10¹³ cells, in-domain,
        // over the cap.
        let slab = StaticCollider {
            min: [-4_000_000.0, 0.0, -4_000_000.0],
            max: [4_000_000.0, 2.0, 4_000_000.0],
        };
        // The local tie box (index 1): its +X push is the SAME
        // expression as the slab's +Y push (col.max + SKIN −
        // player_min, both 2.0 + SKIN − 1.0) — bit-equal magnitudes,
        // different directions.
        let tie_box = StaticCollider {
            min: [-1.0, 0.0, -2.0],
            max: [2.0, 2.0, 2.0],
        };
        let mut world = CollisionWorld::new(0.0)
            .with_collider(slab)
            .with_collider(tie_box);
        // Padding to the gate (distant, never interacting).
        for i in 0..GRID_COLLIDER_GATE - 2 {
            world = world.with_box([400.0 + i as f32 * 4.0, 0.0, 400.0], 0.5, 0.5);
        }
        // A second, ROTATED oversized collider (the rotated-list
        // arm): a 60 m × 60 m prism whose world AABB spans ~3.5 k
        // cells.
        let rotated_big = RotatedCollider::new([50.0, 1.0, 0.0], [30.0, 1.0, 30.0], 0.6).unwrap();
        world = world.with_rotated_collider(Some(rotated_big));
        let BroadPhase::Grid(grid) = &world.grid else {
            panic!("the mixed oversized world keeps the grid active");
        };
        assert_eq!(
            grid.oversized,
            vec![(false, 0), (true, 0)],
            "the slab (axis-aligned index 0) and the big prism (rotated \
             index 0) are the two oversized entries, in push order"
        );
        let mut linear = world.clone();
        force_broad_phase(&mut linear, false);

        // THE ORDER TIE: the player straddles both the slab and the
        // box at pos [1.35, 1.0, 0.0] (player_min.x == player_min.y ==
        // 1.0 exactly). The slab's +Y push and the box's +X push are
        // bit-equal (2.0 + SKIN − 1.0); the slab was inserted FIRST,
        // so its push must win on both paths.
        let (min, max) = player_box([1.35, 1.0, 0.0], &config);
        let grid_push = world
            .depenetration(min, max)
            .expect("the straddle resolves");
        let linear_push = linear
            .depenetration(min, max)
            .expect("the straddle resolves");
        assert!(
            same_option_vec3(Some(grid_push), Some(linear_push)),
            "the oversized-present depenetration is bit-identical"
        );
        assert!(
            linear_push[1] > 0.0 && linear_push[0] == 0.0 && linear_push[2] == 0.0,
            "the FIRST-in-insertion-order oversized slab wins the \
             magnitude tie (its +Y push): {linear_push:?}"
        );

        // The sweep clamp ON the oversized slab: a player above it
        // sweeps down and must stop SKIN short of its top face —
        // only the universally-consulted oversized list can supply
        // that candidate (the slab is in NO cell).
        let (above_min, above_max) = player_box([1.35, 5.0, 0.0], &config);
        let (grid_allowed, grid_hit) = world.sweep_axis(above_min, above_max, 1, -4.0);
        let (linear_allowed, linear_hit) = linear.sweep_axis(above_min, above_max, 1, -4.0);
        assert!(same_sweep(
            (grid_allowed, grid_hit),
            (linear_allowed, linear_hit)
        ));
        assert!(
            grid_hit && grid_allowed > -3.0 && grid_allowed < -2.99,
            "the oversized slab clamps the downward sweep at its top \
             face (2 + SKIN − 5): {grid_allowed}"
        );

        // The segment through the oversized slab (and the tie box,
        // whose top shares the y = 2 plane — the minimum wins
        // identically either way).
        let segment = world.segment_hit([1.35, 5.0, 0.0], [1.35, -5.0, 0.0], 0.0);
        assert!(same_option_f32(
            segment,
            linear.segment_hit([1.35, 5.0, 0.0], [1.35, -5.0, 0.0], 0.0)
        ));
        let t = segment.expect("the segment crosses the slab");
        assert!((t - 0.3).abs() < 1e-6, "enters at the y = 2 top: {t}");

        // Above the slab: clear of everything (the slab's y ≤ 2).
        assert_eq!(
            world.box_overlaps(above_min, above_max),
            linear.box_overlaps(above_min, above_max)
        );
        assert!(!world.box_overlaps(above_min, above_max));
        // Inside the slab band: overlapping.
        let (in_min, in_max) = player_box([1.35, 1.0, 0.0], &config);
        assert!(world.box_overlaps(in_min, in_max));

        // The pseudo-random battery over the mixed world: street
        // placements, interior spawns, sweeps, and segments (short
        // and long — the long diagonal rides the waste guard's
        // regime decision, both arms bit-identical).
        let mut rng = FixtureRng(0x0D06_5EED);
        for query in 0..24 {
            let base = fixture_base(&mut rng, query);
            let (q_min, q_max) = player_box(base, &config);
            assert_eq!(
                world.box_overlaps(q_min, q_max),
                linear.box_overlaps(q_min, q_max),
                "q{query}"
            );
            assert!(same_option_vec3(
                world.depenetration(q_min, q_max),
                linear.depenetration(q_min, q_max)
            ));
            for axis in 0..3 {
                assert!(
                    same_sweep(
                        world.sweep_axis(q_min, q_max, axis, 1.1),
                        linear.sweep_axis(q_min, q_max, axis, 1.1)
                    ),
                    "q{query} a{axis}"
                );
            }
            let to = [base[0] + 20.0, base[1] + 0.4, base[2] - 15.0];
            assert!(same_option_f32(
                world.segment_hit(base, to, 0.1),
                linear.segment_hit(base, to, 0.1)
            ));
            let far_to = [base[0] + 120.0, base[1] + 0.5, base[2] - 120.0];
            assert!(same_option_f32(
                world.segment_hit(base, far_to, CAMERA_MARGIN),
                linear.segment_hit(base, far_to, CAMERA_MARGIN)
            ));
        }
    }

    /// The L5 scaling evidence — the design's §9.4 measurement shape,
    /// observational: city-block populations at multiple collider
    /// counts, each query arm through the grid path vs the
    /// forced-linear clone, with least-squares slopes (ns per 1k
    /// colliders), the one-time build cost, and the grid's resident
    /// shape (cells / entries). Printed, not gated: generous sanity
    /// bounds only (the F-06 no-thresholds-before-variance rule, and
    /// the collision.rs Q512 characterization precedent). CAVEAT,
    /// stated honestly: this runs under the UNOPTIMIZED test profile
    /// — the slopes and the grid/linear RATIO are the evidence here;
    /// absolute constants belong to the release-mode rig matrix
    /// (§9, the measurement owner). Every probe is also bit-compared
    /// against the linear path inline (the battery's claim, measured
    /// on the very populations being timed).
    #[test]
    fn broad_phase_scaling_evidence() {
        let config = PlayerConfig::default();
        // The city-block shape: colliders on a 3 m lattice (1 m
        // footprints — 2 m streets), every 8th a yaw-0.6 rotated
        // prism, roughly square extents.
        let city_block = |count: usize| {
            let mut world = CollisionWorld::new(0.0);
            let side = ((count as f64).sqrt().ceil() as usize).max(1);
            for i in 0..count {
                let base = [(i % side) as f32 * 3.0, 0.0, (i / side) as f32 * 3.0];
                if i % 8 == 3 {
                    world = world.with_rotated_collider(RotatedCollider::from_base_yaw(
                        base, 1.2, 1.2, 2.0, 0.6,
                    ));
                } else {
                    world = world.with_box(base, 1.0, 2.0);
                }
            }
            world
        };

        fn mean_ns(queries: usize, mut run: impl FnMut()) -> f64 {
            let start = std::time::Instant::now();
            for _ in 0..queries {
                run();
            }
            start.elapsed().as_secs_f64() / queries as f64 * 1e9
        }

        // Least-squares slope, ns per 1k colliders, over the measured
        // (population, ns) pairs.
        let slope = |points: &[(usize, f64)]| -> f64 {
            let n = points.len() as f64;
            let sum_x: f64 = points.iter().map(|(x, _)| *x as f64).sum();
            let sum_y: f64 = points.iter().map(|(_, y)| *y).sum();
            let sum_xy: f64 = points.iter().map(|(x, y)| *x as f64 * y).sum();
            let sum_xx: f64 = points.iter().map(|(x, _)| (*x as f64) * (*x as f64)).sum();
            (n * sum_xy - sum_x * sum_y) / (n * sum_xx - sum_x * sum_x) * 1000.0
        };

        // The probes: a street-center box (the per-tick miss case),
        // an in-collider box (the depenetration hit case), a per-tick
        // sweep, a camera-length segment along a street, and the long
        // diagonal — the honest waste row (which regime the
        // GRID_CELL_WASTE_FACTOR guard picks depends on the
        // population: linear at 128/512, grid from 2048 up).
        let street = [1.5, 0.0, 1.5];
        let (s_min, s_max) = player_box(street, &config);
        let (h_min, h_max) = player_box([0.0, 0.0, 0.0], &config);
        let cam_from = [1.5, 1.6, 1.5];
        let cam_to = [1.5, 1.6, 6.0];
        let diag_from = [-60.0, 1.0, -60.0];
        let diag_to = [60.0, 1.0, 60.0];

        let sizes = [128usize, 512, 2048, 8192, 32768];
        // Per-arm (population, ns) series: the six grid arms plus the
        // linear reference arms measured alongside (box_miss stands
        // for the mover arms, diag_ray for the segment arms).
        let mut grid_series: Vec<Vec<(usize, f64)>> = vec![Vec::new(); 6];
        let mut linear_box_series: Vec<(usize, f64)> = Vec::new();
        let mut linear_diag_series: Vec<(usize, f64)> = Vec::new();
        for size in sizes {
            let queries = ((600_000 / size as u64).clamp(100, 5000)) as usize;
            let build_start = std::time::Instant::now();
            let world = city_block(size);
            let build_ms = build_start.elapsed().as_secs_f64() * 1e3;
            let mut linear = world.clone();
            force_broad_phase(&mut linear, false);
            assert!(
                matches!(world.grid, BroadPhase::Grid(_)),
                "the city world at {size} activates the grid"
            );

            // Inline bit-identity on the probe set (the battery's
            // claim, restated on the timed populations).
            assert!(same_option_vec3(
                world.depenetration(s_min, s_max),
                linear.depenetration(s_min, s_max)
            ));
            assert!(same_sweep(
                world.sweep_axis(s_min, s_max, 0, 0.2),
                linear.sweep_axis(s_min, s_max, 0, 0.2)
            ));
            assert!(same_option_f32(
                world.segment_hit(diag_from, diag_to, 0.0),
                linear.segment_hit(diag_from, diag_to, 0.0)
            ));

            let t = |world: &CollisionWorld| -> [f64; 6] {
                [
                    mean_ns(queries, || {
                        world.box_overlaps(s_min, s_max);
                    }),
                    mean_ns(queries, || {
                        world.depenetration(s_min, s_max);
                    }),
                    mean_ns(queries, || {
                        world.depenetration(h_min, h_max);
                    }),
                    mean_ns(queries, || {
                        world.sweep_axis(s_min, s_max, 0, 0.2);
                    }),
                    mean_ns(queries, || {
                        world.segment_hit(cam_from, cam_to, CAMERA_MARGIN);
                    }),
                    mean_ns(queries, || {
                        world.segment_hit(diag_from, diag_to, 0.0);
                    }),
                ]
            };
            let grid_times = t(&world);
            let linear_times = t(&linear);
            if let BroadPhase::Grid(grid) = &world.grid {
                let entries: usize = grid.cells.values().map(Vec::len).sum();
                println!(
                    "[broad-phase-scaling] pop {size:6} · build {build_ms:8.3} ms · \
                     cells {} · entries {entries} · oversized {} · queries {queries}",
                    grid.cells.len(),
                    grid.oversized.len()
                );
            }
            println!(
                "[broad-phase-scaling] pop {size:6} · ns/query grid vs linear — \
                 box_miss {:9.1}/{:9.1} · dep_miss {:9.1}/{:9.1} · dep_hit {:9.1}/{:9.1} · \
                 sweep {:9.1}/{:9.1} · cam_ray {:9.1}/{:9.1} · diag_ray {:9.1}/{:9.1}",
                grid_times[0],
                linear_times[0],
                grid_times[1],
                linear_times[1],
                grid_times[2],
                linear_times[2],
                grid_times[3],
                linear_times[3],
                grid_times[4],
                linear_times[4],
                grid_times[5],
                linear_times[5]
            );
            for (arm, value) in grid_times.iter().enumerate() {
                grid_series[arm].push((size, *value));
            }
            linear_box_series.push((size, linear_times[0]));
            linear_diag_series.push((size, linear_times[5]));
            // Sanity bounds only (characterization, not a gate). The
            // per-record form (the 877590f/3e0c729 precedent): the
            // diagonal grid arm walks cells proportionally to the
            // population (the same class the CI runner fired on the
            // combat twin — 51.3 ms at 32,768 vs the fixed 50 ms);
            // corrected here in the same pass so the slower-runner
            // arithmetic cannot fire this file next.
            for ns in grid_times.iter().chain(linear_times.iter()) {
                assert!(
                    ns.is_finite() && *ns < 5_000_000.0 + size as f64 * 4_000.0,
                    "pop {size}: a query arm is absurdly slow: {ns} ns"
                );
            }
        }

        // The fitted slopes (ns per 1k colliders), grid vs linear per
        // arm — the delivery report's numbers.
        let arms = [
            "box_miss", "dep_miss", "dep_hit", "sweep", "cam_ray", "diag_ray",
        ];
        for (arm, series) in arms.iter().zip(&grid_series) {
            let grid_slope = slope(series);
            let (linear_slope, note) = if *arm == "box_miss" {
                (slope(&linear_box_series), "")
            } else if *arm == "diag_ray" {
                (
                    slope(&linear_diag_series),
                    " (the guard's mixed regime: linear below ~1k pop, grid above)",
                )
            } else {
                (
                    slope(&linear_box_series),
                    " (linear reference: box_miss arm)",
                )
            };
            println!(
                "[broad-phase-scaling] slope {arm:9}: grid {grid_slope:9.1} vs linear \
                 {linear_slope:9.1} ns per 1k colliders{note}"
            );
        }
    }
}
