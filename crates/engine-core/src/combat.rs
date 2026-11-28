//! Cycle 3 P1a — the reusable combat primitives and deterministic
//! resolution contracts.
//!
//! Engine MECHANICS only, mirroring the module precedents (`player.rs`,
//! `interaction.rs`): validated hit resolution (an instant, purely
//! geometric hitscan trace through the existing exact-slab segment
//! query), saturating damage/health transitions whose `Defeated`
//! outcome is the engine's ONLY defeat declaration, the combat event
//! vocabulary the spine can ledger, and their input-validity and
//! determinism rules. GAME POLICY — weapon statistics, rate of fire,
//! adversary temperament, encounter scripting, feedback — lives in
//! the application layer, which supplies every policy value
//! (`AttackProfile`, damage amounts, target slices). The published
//! contract is `cycle3/interfaces/COMBAT-INTERFACE.md` (v1); this
//! module is its compilable form.
//!
//! Determinism discipline (the spine's rules, mirrored): no
//! wall-clock time ever enters logical state — `combat.rs` reads no
//! clock at all; every public function is pure over its explicit
//! inputs (repeated calls with equal inputs give bitwise-equal
//! results; reordered target slices give the same selection). The
//! float discipline is layered honestly: the transition path
//! (`apply_damage`/`apply_heal`) uses add/sub/compare only, and the
//! resolution path additionally mul/div/sqrt — all IEEE
//! exactly-rounded operations — so both are cross-platform
//! BIT-STABLE for identical inputs; the AIM direction is supplied by
//! the caller (typically `player::look_direction`, whose own
//! nonzero-angle discipline is documented there and is NOT claimed
//! here). Combat world state (in tests) fingerprints under the NEW
//! domain `engine-core/combat/1` — no historical fingerprint domain
//! is touched.
//!
//! Input validity (the R4/R4b fail-visible discipline extended): a
//! non-finite or semantically invalid request/profile is REFUSED
//! with a typed reason before any geometry or state mutation; a
//! non-finite or inverted target box is SKIPPED — never an
//! unpredictable comparison and never displacing a valid candidate;
//! a refused health transition leaves the value bit-identical.
//!
//! Proportionality (charter §6, structural): the engine keeps NO
//! registry, NO target list, NO turn scheduler and NO per-frame or
//! per-tick hooks — resolution work happens exactly when a driver
//! resolves an attack, and an idle encounter costs the engine
//! nothing.
//!
//! P2 B06 — the combatant broad phase (the scalability objective,
//! the design's Q-G candidate-discovery half): the linear scan above
//! is the CONTRACT and the retained small-workload fast path;
//! [`CombatIndex`] is its sibling over large rosters — a persistent
//! uniform hash grid over one roster's hitboxes (the StaticGrid
//! pattern, player.rs P2 L5), lookup-only, candidates re-sorted to
//! slice position before the unchanged narrow phase, gated on
//! combatant count ([`COMBAT_INDEX_GATE`]) so every existing combat
//! scenario (1–4 combatants) stays exactly linear. See the
//! [`CombatIndex`] docs for the bit-identity argument, the gate
//! derivation, and the two placement decisions (the wall query
//! already rides the L5 static foundation; the target scan is the
//! D-family half this type adds). The wall-vs-target occlusion and
//! tie semantics are byte-frozen by the tests below and the
//! engine-slice combat/encounter batteries.
//!
//! Q02 — the roster/ownership contract: [`CombatIndex`] BORROWS the
//! roster it was built over (the `CensusIndex` precedent), so it can
//! never serve a different-content roster through a grid built over
//! another — the pre-Q02 length-only guard passed exactly that case
//! and served a wrong answer (the discriminating test reconstructs
//! it). Q01 — the bounded construction: a hitbox covering more than
//! [`GRID_OVERSIZED_CELL_LIMIT`] cells goes to the grid's oversized
//! list (one resident entry, consulted by every query), so no
//! finite in-domain hitbox can amplify the index.

use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};

use crate::event::EventClass;
use crate::player::{
    CollisionWorld, GRID_CELL_LIMIT, GRID_CELL_SIZE, GRID_CELL_WASTE_FACTOR,
    GRID_OVERSIZED_CELL_LIMIT, StaticCollider,
};

/// The combat event payload namespace's type tag (the top 16 bits of
/// a combat event's spine payload). Distinct from every other live
/// namespace: the legacy NPC layout (top bits zero for npc < 2^16),
/// the npc routine chains (`0x5E47`), the sustained-load routine and
/// demand overlays (`0x5F12` / `0x0F12`), the soak bursts (`0x5336`),
/// and the adversary chains (`0xB00B`, SEAT B's reservation — see
/// the contract's §8 tag table).
pub const COMBAT_MAGIC: u64 = 0xC0AB;

/// The RECOMMENDED spine class for combat occurrences
/// (`EventClass::new(0x0C0A)` — above every existing demo/test class
/// id). A recommendation, not a requirement: the payload magic is
/// the interop guarantee; classes organize spine queue policy and
/// remain the driver's choice.
pub const COMBAT_CLASS: EventClass = EventClass::new(0x0C0A);

/// The combat domain's fingerprint tag — namespaced and versioned;
/// no historical domain is touched by this module.
pub const COMBAT_FINGERPRINT_DOMAIN: &str = "engine-core/combat/1";

/// Stable combatant identity. The id SPACE is game-owned; the engine
/// only carries it (parity with `InteractionTarget::id`).
pub type CombatantId = u32;

/// One hittable body, as the engine sees it: an identity plus an
/// axis-aligned extent. [`StaticCollider`] is reused deliberately —
/// the combatant body model is the SAME box model as the collision
/// world, so the exact-slab math applies unchanged, and the player's
/// box is already derivable (`player::player_box`). A box with any
/// non-finite coordinate or an inverted axis (`min > max`) is a
/// validity failure SKIPPED by resolution — never an unpredictable
/// comparison, never displacing a valid candidate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Combatant {
    pub id: CombatantId,
    /// The hittable extent (min/max corners).
    pub hit_box: StaticCollider,
}

impl Combatant {
    /// Whether the extent satisfies the validity contract (all
    /// coordinates finite; `min <= max` on every axis).
    pub fn hit_box_valid(&self) -> bool {
        box_valid(&self.hit_box)
    }
}

/// The geometric weapon policy for ONE trace — game-layer values
/// (PLACEHOLDERS in any default; no balance approval is claimed).
/// Damage is deliberately NOT here: what a hit DOES is game policy,
/// applied through [`Health::apply_damage`] by the driver — keeping
/// resolution purely geometric (the contract's §0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttackProfile {
    /// Effective range (m) — the trace length. The input contract:
    /// finite and nonnegative (`0` = point-blank only: a target
    /// containing the origin still hits at distance 0).
    pub range: f32,
    /// Static-collider expansion for the occlusion query (m). The
    /// input contract: finite and nonnegative. `0.0` (the expected
    /// value) blocks walls exactly at their faces; a positive margin
    /// closes paper-thin gaps. Targets are NEVER expanded by this —
    /// it inflates walls only.
    pub occlusion_margin: f32,
}

impl Default for AttackProfile {
    fn default() -> Self {
        // A neutral placeholder profile (the demo layer supplies the
        // real policy): a 30 m range and exact-face occlusion.
        AttackProfile {
            range: 30.0,
            occlusion_margin: 0.0,
        }
    }
}

/// Which profile field violated its contract (typed refusal detail).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileField {
    /// `range` was non-finite or negative.
    Range,
    /// `occlusion_margin` was non-finite or negative.
    OcclusionMargin,
}

/// Why an [`AttackRequest`] was refused ([`HitResolution::Refused`]).
/// Refusals happen BEFORE any geometry runs — no state is consulted
/// or mutated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackRefusal {
    /// A NaN/inf component in the origin.
    NonFiniteOrigin,
    /// A NaN/inf component in the direction.
    NonFiniteDirection,
    /// The direction has no usable length — the zero vector, or a
    /// length that overflows f32 (finite components whose squares
    /// overflow) — there is no aim to normalize.
    ZeroDirection,
    /// A profile field violated its contract.
    InvalidProfile(ProfileField),
}

/// One aim/fire input — what the driver supplies at the fire moment.
/// The attack is an INSTANT trace (hitscan): no projectile state.
/// The aim direction is ANY nonzero finite vector — the resolver
/// normalizes it. The attacker id is carried for ledger/feedback
/// identity; the geometry never consults it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttackRequest {
    /// Who fired.
    pub attacker: CombatantId,
    /// Trace origin (m) — the player's eye, an adversary's eye,
    /// whatever the driver's policy defines.
    pub origin: [f32; 3],
    /// Aim direction (unnormalized is fine).
    pub direction: [f32; 3],
    /// The geometric profile.
    pub profile: AttackProfile,
}

/// Why a valid trace hit nothing (a VALID resolution outcome — not
/// a refusal).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MissKind {
    /// A static collider occluded the trace before any target.
    /// `distance`/`point` are the wall entry (the same conventions
    /// as [`HitResolution::Hit`]) — usable for impact feedback.
    Occluded { distance: f32, point: [f32; 3] },
    /// The trace ran its full range unobstructed and touched no
    /// target box.
    Clear,
}

/// The resolution of one attack — a pure function of (world,
/// request, targets) with three outcomes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HitResolution {
    /// A combatant box was intersected with a clear line, strictly
    /// nearer than any occluder.
    Hit {
        target: CombatantId,
        /// Distance along the NORMALIZED direction from the origin
        /// to the box entry face (m). Always in `[0, range]`.
        distance: f32,
        /// The impact point `origin + normalized_direction * distance`.
        point: [f32; 3],
    },
    /// No combatant hit.
    Miss { kind: MissKind },
    /// The request violated the input contract — refused BEFORE any
    /// geometry runs.
    Refused(AttackRefusal),
}

fn unit_direction(direction: [f32; 3]) -> Result<[f32; 3], AttackRefusal> {
    if !direction.iter().all(|v| v.is_finite()) {
        return Err(AttackRefusal::NonFiniteDirection);
    }
    let [x, y, z] = direction;
    let length = (x * x + y * y + z * z).sqrt();
    if length == 0.0 || !length.is_finite() {
        return Err(AttackRefusal::ZeroDirection);
    }
    Ok(direction.map(|v| v / length))
}

fn point_at(origin: [f32; 3], unit: [f32; 3], distance: f32) -> [f32; 3] {
    std::array::from_fn(|i| origin[i] + unit[i] * distance)
}

impl HitResolution {
    /// World-space end of a resolved ray. Contacts carry their actual entry
    /// point. A clear ray ends at the original request's range, using the same
    /// normalization as the collision segment. No collision query is performed.
    pub fn endpoint(&self, origin: [f32; 3], direction: [f32; 3], range: f32) -> Option<[f32; 3]> {
        let point = match *self {
            Self::Hit { point, .. }
            | Self::Miss {
                kind: MissKind::Occluded { point, .. },
            } => point,
            Self::Miss {
                kind: MissKind::Clear,
            } if range.is_finite() && range >= 0.0 => {
                point_at(origin, unit_direction(direction).ok()?, range)
            }
            _ => return None,
        };
        point.iter().all(|v| v.is_finite()).then_some(point)
    }
}

/// Whether a box satisfies the extent validity contract: all
/// coordinates finite, `min <= max` on every axis (shared by the
/// target-skip rule and the public primitive's skip rule).
fn box_valid(box_extent: &StaticCollider) -> bool {
    let finite = |v: [f32; 3]| v.iter().all(|x| x.is_finite());
    finite(box_extent.min)
        && finite(box_extent.max)
        && (0..3).all(|a| box_extent.min[a] <= box_extent.max[a])
}

/// The exact-slab segment-vs-box test — the single-box primitive
/// [`CollisionWorld::segment_hit`] applies per collider. Returns the
/// smallest `t` in `[0, 1]` at which the segment `from → to` enters
/// the box, or `None` when the segment misses. Pure arithmetic (the
/// ray–AABB slab method) — no sampling, so no aliasing. `Some(0.0)`
/// when the segment starts inside. A box with any non-finite
/// coordinate or an inverted axis returns `None` (skipped — never an
/// unpredictable comparison).
pub fn segment_box_hit(from: [f32; 3], to: [f32; 3], box_extent: StaticCollider) -> Option<f32> {
    if !box_valid(&box_extent) {
        return None;
    }
    let direction = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let mut t_enter = 0.0f32;
    let mut t_exit = 1.0f32;
    for axis in 0..3 {
        let d = direction[axis];
        let origin = from[axis];
        if d == 0.0 {
            if origin < box_extent.min[axis] || origin > box_extent.max[axis] {
                return None;
            }
        } else {
            let mut t1 = (box_extent.min[axis] - origin) / d;
            let mut t2 = (box_extent.max[axis] - origin) / d;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            t_enter = t_enter.max(t1);
            t_exit = t_exit.min(t2);
            if t_enter > t_exit {
                return None;
            }
        }
    }
    if t_exit >= 0.0 && t_enter <= 1.0 {
        Some(t_enter.max(0.0))
    } else {
        None
    }
}

/// Resolves one attack: a pure, instant, geometric trace from
/// `request.origin` along the normalized `request.direction`, for
/// `request.profile.range` meters, occluded by the static collision
/// world through [`CollisionWorld::segment_hit`] (the existing
/// exact-slab query), intersected against the caller-supplied
/// [`Combatant`] boxes. The selection: nearest hit distance wins,
/// equal distances break to the LOWEST id, slice order never
/// matters; a target is a candidate only STRICTLY nearer than an
/// occluder (the wall wins ties — the collision world is the
/// authoritative geometry). Invalid targets are skipped; an invalid
/// request is refused before any geometry. No allocation beyond the
/// returned value. This is the LINEAR PRIMITIVE — the contract and
/// the retained small-workload fast path (P2 B06); [`CombatIndex`]
/// is the broad-phase sibling over large rosters, with bit-identical
/// results.
pub fn resolve_attack(
    world: &CollisionWorld,
    request: &AttackRequest,
    targets: &[Combatant],
) -> HitResolution {
    resolve_attack_over(world, request, targets, None)
}

/// The shared resolution core — the linear primitive and the
/// indexed sibling run the SAME code (the single-source-of-truth
/// rule the static broad phase established): the validation, the
/// wall occlusion query (through the UNCHANGED
/// [`CollisionWorld::segment_hit`] — its own P2 L5 static broad
/// phase serves it, independent of the combatant index), then the
/// target scan — over the full slice, or over the index's
/// conservative candidates re-sorted to slice order. The only
/// possible difference between the two paths is WHICH combatants
/// the scan visits, in what order. INVARIANT (Q02's ownership
/// contract): when `index` is `Some`, `targets` IS the index's
/// borrowed `population` — the only call site passes exactly that,
/// so the candidates (gathered from the grid built over that same
/// slice) index into the slice they were gathered from.
fn resolve_attack_over(
    world: &CollisionWorld,
    request: &AttackRequest,
    targets: &[Combatant],
    index: Option<&CombatIndex>,
) -> HitResolution {
    if !request.origin.iter().all(|v| v.is_finite()) {
        return HitResolution::Refused(AttackRefusal::NonFiniteOrigin);
    }
    let look = match unit_direction(request.direction) {
        Ok(look) => look,
        Err(reason) => return HitResolution::Refused(reason),
    };
    if !request.profile.range.is_finite() || request.profile.range < 0.0 {
        return HitResolution::Refused(AttackRefusal::InvalidProfile(ProfileField::Range));
    }
    if !request.profile.occlusion_margin.is_finite() || request.profile.occlusion_margin < 0.0 {
        return HitResolution::Refused(AttackRefusal::InvalidProfile(
            ProfileField::OcclusionMargin,
        ));
    }
    let to = point_at(request.origin, look, request.profile.range);
    // Occlusion first (the existing public exact-slab query): t in
    // [0, 1] of the segment maps to meters along the trace.
    let wall_t = world.segment_hit(request.origin, to, request.profile.occlusion_margin);
    let wall_distance = wall_t.map(|t| t * request.profile.range);
    let mut best: Option<(CombatantId, f32)> = None;
    if let Some(candidates) = index.and_then(|index| index.candidates(request.origin, to)) {
        for position in candidates {
            consider_combatant(
                &targets[position as usize],
                request,
                to,
                wall_distance,
                &mut best,
            );
        }
    } else {
        for target in targets {
            consider_combatant(target, request, to, wall_distance, &mut best);
        }
    }
    match best {
        Some((target, distance)) => HitResolution::Hit {
            target,
            distance,
            point: point_at(request.origin, look, distance),
        },
        None => match wall_distance {
            Some(distance) => HitResolution::Miss {
                kind: MissKind::Occluded {
                    distance,
                    point: point_at(request.origin, look, distance),
                },
            },
            None => HitResolution::Miss {
                kind: MissKind::Clear,
            },
        },
    }
}

/// The per-target narrow phase and accumulator step — the linear
/// scan's own body, shared verbatim by the grid candidate walk:
/// the exact-slab hit test, the wall's strictly-nearer rule, and
/// the (distance, id) selection. A combatant skipped by either path
/// (invalid box, or outside the broad phase's conservative
/// candidate set) offers no pair to the accumulator at all, so it
/// cannot change the result.
fn consider_combatant(
    target: &Combatant,
    request: &AttackRequest,
    to: [f32; 3],
    wall_distance: Option<f32>,
    best: &mut Option<(CombatantId, f32)>,
) {
    // Invalid targets are skipped, never an unpredictable
    // comparison and never displacing a valid candidate.
    let Some(t) = segment_box_hit(request.origin, to, target.hit_box) else {
        return;
    };
    let distance = t * request.profile.range;
    // The wall wins ties: a target must be STRICTLY nearer than
    // an occluder.
    if wall_distance.is_some_and(|wall| distance >= wall) {
        return;
    }
    let better = match *best {
        None => true,
        Some((best_id, best_distance)) => {
            distance < best_distance || (distance == best_distance && target.id < best_id)
        }
    };
    if better {
        *best = Some((target.id, distance));
    }
}

// ==================================================== the P2 B06 broad phase

/// The combatant broad phase's population gate (P2 B06): rosters
/// whose combatant count is below this build the INERT linear index
/// (the exact existing scan, bit-identical, forever — the charter's
/// retained small-workload fast path); at or above it
/// [`CombatIndex::build`] activates the uniform grid and every
/// [`CombatIndex::resolve_attack`] query gathers its candidates from
/// it. 128 mirrors the static family's gate
/// ([`GRID_COLLIDER_GATE`], the design's initial-gate number —
/// docs/engineering-excellence/designs/P2-simulation-spatial.md
/// §2.2), a measurement-pinned placeholder, not fashion. The
/// design's D-family census gate in the same section (population
/// ≥ 512 AND ≥ 4 queries per tick) assumes a PER-TICK rebuild; this
/// index is persistent per ROSTER (built once per
/// [`CombatIndex::build`], amortized over every trace the
/// composition runs against that roster — a spread volley resolves
/// up to [`crate::weapons::MAX_SPREAD_RAYS`] rays over one build),
/// so the query-count clause is the composition's adoption choice
/// and the engine-side gate is the population clause alone. Every
/// existing scenario sits two orders of magnitude below it — the
/// combat demo's single adversary, the encounter demo's 1–4
/// adversaries, the adversary attack's single player target — so
/// every combat world today is exactly the linear path.
pub const COMBAT_INDEX_GATE: usize = 128;

/// A trivially-cheap deterministic hasher for cell keys (FNV-1a
/// over the key bytes) — the local twin of `player.rs`'s private
/// `CellHasher`. The twin exists because the shared cell machinery
/// in `player.rs` is PRIVATE to that module and this lane's edit
/// scope is `combat.rs`/`interaction.rs` only (the lane boundary;
/// promoting the helpers to a shared pub surface is an integrator
/// decision, documented in the lane handoff). The choice is
/// behavior-invisible either way: the grid is lookup-only and never
/// iterated, so the hash order never reaches a result.
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

/// Builds [`CellHasher`]s (see its docs for why the default hasher
/// is replaced).
#[derive(Clone, Default)]
struct CellHasherBuilder;

impl BuildHasher for CellHasherBuilder {
    type Hasher = CellHasher;

    fn build_hasher(&self) -> CellHasher {
        CellHasher(CellHasher::BASIS)
    }
}

/// The cell index of one coordinate: floor(coord /
/// [`GRID_CELL_SIZE`]) — the same power-of-two divisor as the static
/// grid, so the division is exact in f32 for every finite input
/// (no rounding in the cell mapping — the determinism discipline).
/// `None` when the coordinate is non-finite or the cell lies
/// outside the [`GRID_CELL_LIMIT`] domain — the caller's
/// fail-closed signal (the design's §2.2.6 numerical-domain
/// contract; with the shared cell size this admits world
/// coordinates out to ±4,194,304 m).
fn combat_cell_of(coordinate: f32) -> Option<i32> {
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
/// [lo, hi]: floor(lo/s)..=floor(hi/s). Inclusive on both ends — a
/// hitbox whose face sits exactly ON a cell boundary is found by a
/// query that merely touches it (the closed-interval overlap the
/// slab entry point guarantees). `None` when non-finite or out of
/// domain.
fn combat_cell_span(lo: f32, hi: f32) -> Option<(i32, i32)> {
    let lo_cell = combat_cell_of(lo)?;
    let hi_cell = combat_cell_of(hi)?;
    Some((lo_cell, hi_cell))
}

/// The per-axis cell spans covering one trace's conservative extent
/// — the segment's raw AABB (targets are NEVER expanded by any
/// margin; only the wall occlusion query expands its colliders, and
/// that is the static world's own business). `None` (fail-closed
/// onto the exact linear path) when either endpoint is non-finite —
/// checked BEFORE the per-axis min/max, which swallow NaN — or any
/// span lies outside the cell domain, or the walked-cell count
/// overflows (checked multiply).
fn combat_query_spans(from: [f32; 3], to: [f32; 3]) -> Option<([(i32, i32); 3], u64)> {
    let mut spans = [(0i32, 0i32); 3];
    let mut walked: u64 = 1;
    for axis in 0..3 {
        if !from[axis].is_finite() || !to[axis].is_finite() {
            return None;
        }
        let lo = from[axis].min(to[axis]);
        let hi = to[axis].max(from[axis]);
        let (lo_cell, hi_cell) = combat_cell_span(lo, hi)?;
        spans[axis] = (lo_cell, hi_cell);
        let count = (hi_cell as i64 - lo_cell as i64 + 1).max(0) as u64;
        walked = walked.checked_mul(count)?;
    }
    Some((spans, walked))
}

/// The uniform grid over one roster's valid hitboxes: OCCUPIED cells
/// only (a hash grid — no dense world-space allocation, the galaxy
/// rule), each carrying the SLICE POSITIONS of the combatants
/// inserted into it, in insertion order (push, never sorted — the
/// candidate re-sort relies only on the positions, but keeping the
/// per-cell lists in insertion order makes the construction itself
/// canonical). Lookup-only — never iterated. Plus the OVERSIZED list
/// (Q01's bounded construction, the StaticGrid twin): hitboxes whose
/// covered-cell count exceeds [`GRID_OVERSIZED_CELL_LIMIT`] — one
/// resident entry each, consulted by every query.
#[derive(Debug, Clone, Default)]
struct CombatantGrid {
    /// Cell key = (floor(x/s), floor(y/s), floor(z/s)) with
    /// s = [`GRID_CELL_SIZE`]; value = slice positions.
    cells: HashMap<[i32; 3], Vec<u32>, CellHasherBuilder>,
    /// VALID hitboxes covering more than [`GRID_OVERSIZED_CELL_LIMIT`]
    /// cells (or whose coverage overflows the checked product): ONE
    /// entry each, universally consulted. A finite in-domain hitbox
    /// can never amplify the index's memory or build time — the
    /// resident-entry bound (cap + 1) per combatant.
    oversized: Vec<u32>,
}

impl CombatantGrid {
    /// Inserts one VALID combatant's entry into every cell its
    /// hitbox covers — unless the coverage exceeds
    /// [`GRID_OVERSIZED_CELL_LIMIT`], in which case the position goes
    /// to the OVERSIZED list as one entry (Q01's bounded
    /// construction, identical to the static grid's mechanism). The
    /// caller has already validated the box
    /// ([`Combatant::hit_box_valid`] — finite, `min <= max` on every
    /// axis), so no per-axis sort is needed here (unlike the static
    /// grid, whose linear arms also consult inverted collider
    /// literals): the combat contract's own skip rule refuses
    /// inverted boxes outright. Returns `false` when the box lies
    /// outside the [`GRID_CELL_LIMIT`] domain — the caller poisons
    /// the index (fail-closed), because an out-of-domain VALID box
    /// is hit-able by the linear path and therefore cannot simply
    /// be skipped; the domain rule wins over the oversized list
    /// exactly as it does in the static grid.
    fn insert(&mut self, position: u32, min: [f32; 3], max: [f32; 3]) -> bool {
        let mut spans = [(0i32, 0i32); 3];
        for axis in 0..3 {
            let Some(span) = combat_cell_span(min[axis], max[axis]) else {
                return false;
            };
            spans[axis] = span;
        }
        // The covered-cell count, checked (a whole-domain hitbox's
        // product exceeds u64 — itself proof it is over the cap).
        let mut covered: u64 = 1;
        for span in &spans {
            let per_axis = (span.1 - span.0 + 1) as u64;
            match covered.checked_mul(per_axis) {
                Some(product) => covered = product,
                None => {
                    self.oversized.push(position);
                    return true;
                }
            }
        }
        if covered > GRID_OVERSIZED_CELL_LIMIT {
            self.oversized.push(position);
            return true;
        }
        for cx in spans[0].0..=spans[0].1 {
            for cy in spans[1].0..=spans[1].1 {
                for cz in spans[2].0..=spans[2].1 {
                    self.cells.entry([cx, cy, cz]).or_default().push(position);
                }
            }
        }
        true
    }

    /// The conservative candidate set for precomputed query cell
    /// spans: walk the covered cells row-major (the canonical order
    /// — any fixed order would do, since the re-sort restores the
    /// contract order), collect the positions PLUS the entire
    /// oversized list (Q01: every query consults it — conservative
    /// correctness with oversized hitboxes present), re-sort to slice
    /// position (the linear scan's own iteration order), dedup.
    /// Duplicates (a hitbox covering several walked cells) collapse
    /// here; they are provable no-ops anyway (the accumulator
    /// accepts only a strictly smaller (distance, id) pair, so a
    /// re-evaluated combatant never re-displaces), but removing them
    /// keeps the candidate walk honest. The oversized entries enter
    /// the SAME re-sort, so the candidate sequence is exactly the
    /// linear scan's iteration order over a conservative superset —
    /// bit-identity holds, ties included.
    fn candidates(&self, spans: [(i32, i32); 3]) -> Vec<u32> {
        let mut positions: Vec<u32> = Vec::new();
        for cx in spans[0].0..=spans[0].1 {
            for cy in spans[1].0..=spans[1].1 {
                for cz in spans[2].0..=spans[2].1 {
                    if let Some(cell) = self.cells.get(&[cx, cy, cz]) {
                        positions.extend_from_slice(cell);
                    }
                }
            }
        }
        positions.extend_from_slice(&self.oversized);
        positions.sort_unstable();
        positions.dedup();
        positions
    }
}

/// The full-grid build over one roster (the gate-crossing path and
/// the test forcing path share it). INVALID hitboxes are SKIPPED,
/// never poisoned — the combat contract's own skip rule
/// ([`segment_box_hit`] returns `None` for a non-finite or inverted
/// box, so such a target is a provable no-op in the linear scan;
/// this differs DELIBERATELY from the static grid, whose linear
/// arms consult raw NaN faces and must fail closed instead). `None`
/// only when a VALID box lies outside the cell domain — fail-closed,
/// because such a box IS hit-able by the linear path.
fn build_combatant_grid(targets: &[Combatant]) -> Option<CombatantGrid> {
    let mut grid = CombatantGrid::default();
    for (position, target) in targets.iter().enumerate() {
        if !target.hit_box_valid() {
            continue;
        }
        if !grid.insert(position as u32, target.hit_box.min, target.hit_box.max) {
            return None;
        }
    }
    Some(grid)
}

/// The combatant broad phase state (the [`CombatIndex`] interior).
#[derive(Debug, Clone, Default)]
enum CombatPhase {
    /// Below [`COMBAT_INDEX_GATE`] (or the empty default): every
    /// resolve takes the exact linear scan.
    #[default]
    Linear,
    /// The uniform grid over the roster's valid hitboxes.
    Grid(CombatantGrid),
    /// A VALID hitbox lay outside the [`GRID_CELL_LIMIT`] domain —
    /// permanently linear for this roster (fail-closed; the roster
    /// is fixed at build, so the poisoning box never leaves it).
    Unusable,
}

/// The per-combat population index (P2 B06 — the design's Q-G
/// candidate-discovery half): a persistent uniform hash grid over
/// ONE combatant roster's hitboxes, built by [`CombatIndex::build`],
/// answering queries through [`CombatIndex::resolve_attack`] with
/// results BIT-IDENTICAL to [`resolve_attack`] on the same roster —
/// the StaticGrid determinism trick (player.rs, P2 L5): the grid is
/// a LOOKUP STRUCTURE ONLY (never iterated; the hash order is
/// invisible), every query walks the cells under its conservative
/// extent in canonical row-major order, and the candidates are
/// RE-SORTED to slice position — the linear scan's own iteration
/// order — before the unchanged narrow phase ([`segment_box_hit`])
/// and the shared accumulator ([`consider_combatant`]) run over
/// them.
///
/// THE BIT-IDENTITY ARGUMENT (stronger than the static family's):
///
/// 1. **The candidate set is a conservative superset of every
///    combatant that can affect the result.** A combatant changes
///    `best` only when [`segment_box_hit`] returns `Some(t)` — the
///    segment enters its box. The entry point then lies in BOTH the
///    closed box and the closed segment (a convex combination of the
///    endpoints), hence in the segment's AABB on every axis; the
///    inclusive cell spans therefore guarantee the box's cell
///    coverage shares the walked cell containing the entry point —
///    the box IS a candidate. Skipped combatants offer no
///    (distance, id) pair at all.
/// 2. **The selection is order-independent by its own rule.** The
///    accumulator keeps the lexicographic minimum of (distance,
///    id) — an associative, commutative, duplicate-idempotent
///    reduction — so the final pair (and the Hit it produces:
///    `point` is a function of the same `distance`) is a pure
///    function of the candidate multiset. The re-sort to slice
///    order is kept anyway: the landed foundation's uniform
///    discipline, and the guard against any future order-sensitive
///    accumulator evolution.
/// 3. **Every other input is shared code.** Validation, the wall
///    occlusion query, and the final reduction run in
///    [`resolve_attack_over`] — one function, both paths.
///
/// THE PLACEMENT DECISIONS (the design's family analysis,
/// §1.11/§2.2):
///
/// * **The WALL segment query (occlusion) needs NO new machinery.**
///   It already rides [`CollisionWorld::segment_hit`] — the S-ray
///   family query whose OWN static broad phase landed in P2 L5
///   (gated at [`GRID_COLLIDER_GATE`] with the segment-AABB
///   conservative extent and the [`GRID_CELL_WASTE_FACTOR`] guard).
///   The wall population (the static collider list — built once,
///   never mutates) and the target population (the combatant roster
///   — rebuilt per roster revision) are different populations with
///   different lifecycles; one index would couple them for no
///   query's benefit (the design §2.3's rejection of the universal
///   index). Today's combat arenas sit far below the static gate —
///   the combat demo's courtyard is 10 solid boxes (12 literals
///   minus the two visual markers), the encounter arena 7 boxes + 2
///   rotated prisms — so the wall query is the exact linear path
///   today, and a large combat world inherits the L5 grid with zero
///   B06 work.
/// * **The TARGET scan is the D-family half this type adds** — as a
///   sibling primitive beside the unchanged linear
///   [`resolve_attack`], the design's L6 shape ("the composition
///   chooses per workload; both paths remain permanent"). The
///   composition owns the roster and rebuilds the index when the
///   roster changes (the encounter's `encounter_targets` cadence);
///   adoption in `engine-slice`/`weapons` is a follow-up seam
///   (`FireContext` could carry `Option<&CombatIndex>`), outside
///   this lane's edit scope.
///
/// **Fail-closed domains:** a VALID hitbox outside the cell domain
/// poisons the index for its roster ([`CombatPhase::Unusable`]); a
/// non-finite query extent (e.g. a `to` that overflowed at huge
/// range), an out-of-domain extent, or the
/// [`GRID_CELL_WASTE_FACTOR`] crossover guard each fall back to the
/// exact linear path PER QUERY. Behavior never depends on the broad
/// phase being available.
///
/// **THE ROSTER/OWNERSHIP CONTRACT (Q02):** the index BORROWS the
/// roster it was built over — `population: &'p [Combatant]` — so it
/// cannot outlive it and cannot serve any other slice. This replaces
/// the pre-Q02 shape (a stored roster LENGTH, checked per query),
/// whose guard missed exactly the dangerous case: a SAME-LENGTH
/// different-content roster passed the length check and the grid
/// then served candidates gathered from the BUILT roster's
/// hitboxes against the PASSED roster's boxes — a wrong answer
/// (demonstrated, not hypothetical: the Q02 discriminating test
/// reconstructs the old call and shows it missing a target the
/// linear scan over the passed roster hits). A pointer check would
/// not close it either (a public mutable roster can change contents
/// without changing its address — the same reasoning that keeps
/// `interaction.rs`'s per-call census linear); a per-query content
/// fingerprint would cost O(n) per query, defeating the index. The
/// borrow is the only sound-by-construction shape, and it is this
/// engine's own established pattern: the census index handle
/// (`npc_perception::CensusIndexScratch::build`) returns a handle
/// borrowing BOTH the caller's scratch and the population slice for
/// exactly this reason ("staleness is impossible by construction —
/// drop the handle, mutate the population, rebuild"). The composition
/// contract that follows: REBUILD the index whenever the roster
/// mutates (the encounter's `encounter_targets` cadence — roster
/// and index are rebuilt together, in one scope, so the borrow is
/// natural); a stale index over a mutated roster is a borrow-check
/// error, never a silent wrong answer. No production call site
/// exists yet (the `FireContext` adoption seam is Q04's), so the
/// contract changes cost nothing to adopt.
#[derive(Debug, Clone, Default)]
pub struct CombatIndex<'p> {
    /// The roster this index was built over and serves — BORROWED
    /// (Q02's ownership contract: the handle cannot outlive the
    /// roster, and every resolve reads THIS slice, so a
    /// different-content roster can never be served through a grid
    /// built over another; mutation requires dropping the handle
    /// first — the borrow checker enforces the rebuild cadence).
    population: &'p [Combatant],
    phase: CombatPhase,
}

impl<'p> CombatIndex<'p> {
    /// Builds the index over one roster, borrowing it. Below
    /// [`COMBAT_INDEX_GATE`] the result is the inert linear index
    /// (the retained small-workload fast path — every current
    /// scenario); at or above it the grid is built, unless a VALID
    /// hitbox lies outside the cell domain (then permanently linear
    /// for this roster, fail-closed). INVALID hitboxes never poison
    /// — they are skipped, exactly as the linear scan skips them;
    /// oversized VALID hitboxes (coverage over
    /// [`GRID_OVERSIZED_CELL_LIMIT`]) go to the grid's oversized
    /// list — one resident entry, universally consulted (Q01's
    /// bounded construction).
    pub fn build(targets: &'p [Combatant]) -> CombatIndex<'p> {
        let mut index = CombatIndex {
            population: targets,
            phase: CombatPhase::Linear,
        };
        if targets.len() >= COMBAT_INDEX_GATE {
            index.phase = match build_combatant_grid(targets) {
                Some(grid) => CombatPhase::Grid(grid),
                None => CombatPhase::Unusable,
            };
        }
        index
    }

    /// Resolves one attack over the roster this index was built
    /// from (the borrowed `population` — there is no `targets`
    /// parameter to get wrong, Q02's point) — the IDENTICAL contract
    /// to [`resolve_attack`] (validation, occlusion, selection,
    /// ties, refusals): the only difference is the candidate source,
    /// and the result is bit-identical by construction (see the
    /// type's docs). Allocates the candidate list per query (the
    /// linear primitive never does; the composition weighs that
    /// against its roster size — see the Q09 allocation census,
    /// `tests/spatial_query_allocations.rs`).
    pub fn resolve_attack(&self, world: &CollisionWorld, request: &AttackRequest) -> HitResolution {
        resolve_attack_over(world, request, self.population, Some(self))
    }

    /// The broad-phase candidates for one trace, or `None` to take
    /// the exact linear path: below gate / poisoned / non-finite or
    /// out-of-domain extent / waste guard. The population is the
    /// borrowed roster by construction — the pre-Q02 length-mismatch
    /// guard is deleted (unrepresentable under the borrow).
    fn candidates(&self, from: [f32; 3], to: [f32; 3]) -> Option<Vec<u32>> {
        let CombatPhase::Grid(grid) = &self.phase else {
            return None;
        };
        let (spans, walked) = combat_query_spans(from, to)?;
        // The crossover guard (the DDA stand-in, the static family's
        // own GRID_CELL_WASTE_FACTOR): more cells walked than 4 per
        // combatant means the walk alone outcosts the linear
        // per-target evaluations — the exact linear path. The
        // documented class it keeps linear is the LONG DIAGONAL ray
        // over a small roster (a 170 m diagonal spans ~3.7 k cells:
        // linear below ~1 k combatants, grid above).
        if walked > GRID_CELL_WASTE_FACTOR * self.population.len() as u64 {
            return None;
        }
        Some(grid.candidates(spans))
    }
}

/// Why a health operation was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthError {
    /// The amount was NaN or infinite.
    NonFiniteAmount,
    /// The amount was negative (use [`Health::apply_heal`]).
    NegativeAmount,
    /// The target maximum was invalid at construction
    /// (`health_full`).
    InvalidMaximum,
    /// The `Health` value itself violates its invariants (only
    /// possible through external mutation) — the operation refuses
    /// rather than silently repairing it.
    InvalidState,
}

/// The outcome of one damage application.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DamageOutcome {
    /// `amount == 0.0` — a VALID graze: the hit landed, feedback
    /// fires, health is UNCHANGED (a defeated value stays defeated).
    Graze,
    /// Damage applied; `remaining > 0`. `dealt == amount`.
    Wounded { remaining: f32, dealt: f32 },
    /// THE DEFEATED DECLARATION — the engine's only defeat
    /// authority: the damage application that touched or crossed
    /// zero (`current <= amount`). `current` is now EXACTLY `0.0`
    /// (saturating, never negative).
    Defeated { dealt: f32 },
}

/// The outcome of one heal application.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HealOutcome {
    /// Healed, still below maximum. `gained` is the applied amount.
    Restored { current: f32, gained: f32 },
    /// Saturated at maximum. `gained` is the APPLIED amount (the
    /// clamped remainder is not silently claimed as healed).
    Capped { current: f32, gained: f32 },
}

/// A combatant's health state — the authoritative transition domain.
/// INVARIANTS (enforced at construction and before every
/// transition): `current` and `maximum` finite; `maximum >= 0`;
/// `0 <= current <= maximum`. A value that violates them (only
/// possible through external mutation — the fields are pub for
/// fingerprinting/inspection) refuses every transition with
/// [`HealthError::InvalidState`], never a silent repair.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Health {
    pub current: f32,
    pub maximum: f32,
}

/// Constructs full health (`current == maximum`). `maximum` must be
/// finite and nonnegative — `maximum == 0.0` is a valid zero-health
/// combatant (born defeated; useful for pre-defeated encounter
/// states). This is ALSO the transient encounter reset's
/// reconstruction step (the contract's §9 — no persistence claims).
pub fn health_full(maximum: f32) -> Result<Health, HealthError> {
    if !maximum.is_finite() || maximum < 0.0 {
        return Err(HealthError::InvalidMaximum);
    }
    Ok(Health {
        current: maximum,
        maximum,
    })
}

impl Health {
    fn state_valid(&self) -> bool {
        self.current.is_finite()
            && self.maximum.is_finite()
            && self.maximum >= 0.0
            && self.current >= 0.0
            && self.current <= self.maximum
    }

    /// The defeated read: exactly `current == 0.0` under the
    /// invariants. A convenience read, not a second authority —
    /// after any successful `apply_damage` it agrees exactly with
    /// the `Defeated` declaration.
    pub fn is_defeated(&self) -> bool {
        self.current == 0.0
    }

    /// Applies damage: the saturating transition whose `Defeated`
    /// outcome is the ONLY defeat declaration in the engine — the
    /// predicate is exactly `current <= amount` (the application
    /// that touches or crosses zero). Zero damage is a VALID graze
    /// (health unchanged; a defeated value stays defeated). Refused
    /// inputs ([`HealthError`]) leave the value BIT-IDENTICAL —
    /// refused before any mutation. Plain f32 subtract/compare only
    /// (the contract's §6 float discipline: cross-platform
    /// bit-stable).
    pub fn apply_damage(&mut self, amount: f32) -> Result<DamageOutcome, HealthError> {
        if !amount.is_finite() {
            return Err(HealthError::NonFiniteAmount);
        }
        if amount < 0.0 {
            return Err(HealthError::NegativeAmount);
        }
        if !self.state_valid() {
            return Err(HealthError::InvalidState);
        }
        if amount == 0.0 {
            return Ok(DamageOutcome::Graze);
        }
        if self.current <= amount {
            self.current = 0.0;
            return Ok(DamageOutcome::Defeated { dealt: amount });
        }
        self.current -= amount;
        Ok(DamageOutcome::Wounded {
            remaining: self.current,
            dealt: amount,
        })
    }

    /// Applies healing: saturates at `maximum` (a revive is a policy
    /// decision the driver owns; the mechanism is uniform). Same
    /// refusal discipline — refused inputs leave the value
    /// bit-identical.
    pub fn apply_heal(&mut self, amount: f32) -> Result<HealOutcome, HealthError> {
        if !amount.is_finite() {
            return Err(HealthError::NonFiniteAmount);
        }
        if amount < 0.0 {
            return Err(HealthError::NegativeAmount);
        }
        if !self.state_valid() {
            return Err(HealthError::InvalidState);
        }
        let headroom = self.maximum - self.current;
        if amount <= headroom {
            self.current += amount;
            Ok(HealOutcome::Restored {
                current: self.current,
                gained: amount,
            })
        } else {
            let gained = headroom.max(0.0);
            self.current = self.maximum;
            Ok(HealOutcome::Capped {
                current: self.maximum,
                gained,
            })
        }
    }
}

/// What kind of combat occurrence a payload records. Stable
/// discriminants — additive-only in future contract versions, never
/// renumbered (the contract's §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombatEventKind {
    /// A shot was fired (the attacker's id).
    Fire = 1,
    /// A trace hit something (a wall or a body — the subject's id).
    Impact = 2,
    /// Damage was applied (the subject's id).
    Damage = 3,
    /// The DEFEATED declaration landed (the subject's id).
    Defeat = 4,
    /// Healing was applied (the subject's id).
    Heal = 5,
    /// A transient encounter reset occurred.
    Reset = 6,
}

impl CombatEventKind {
    /// Decodes the discriminant; `None` for values outside the
    /// ratified vocabulary (a future kind is FOREIGN until this
    /// contract's version says otherwise — never a coerced decode).
    fn from_discriminant(discriminant: u64) -> Option<CombatEventKind> {
        match discriminant {
            1 => Some(CombatEventKind::Fire),
            2 => Some(CombatEventKind::Impact),
            3 => Some(CombatEventKind::Damage),
            4 => Some(CombatEventKind::Defeat),
            5 => Some(CombatEventKind::Heal),
            6 => Some(CombatEventKind::Reset),
            _ => None,
        }
    }
}

/// The combatant-id bound for LEDGER payloads: ids at or above 2^24
/// refuse at encode (never a truncated payload). The runtime
/// [`CombatantId`] stays u32; the ledger field is 24 bits.
pub const COMBAT_PAYLOAD_ID_BOUND: u64 = 1 << 24;

/// Encodes a combat event payload: `COMBAT_MAGIC(16) | kind(8) |
/// combatant(24) | detail(16)`. Refuses (`None`) a combatant id ≥
/// 2^24 — never a truncated payload (the detail field is u16 by
/// type, so it cannot overflow). The payload is a CLASSIFICATION
/// token, not a serializer: quantitative state lives in the owning
/// world's state and logs.
pub fn combat_payload(kind: CombatEventKind, combatant: CombatantId, detail: u16) -> Option<u64> {
    if u64::from(combatant) >= COMBAT_PAYLOAD_ID_BOUND {
        return None;
    }
    Some(
        (COMBAT_MAGIC << 48)
            | (u64::from(kind as u8) << 40)
            | (u64::from(combatant) << 16)
            | u64::from(detail),
    )
}

/// Decodes a combat event payload: `Some((kind, combatant, detail))`
/// ONLY when the top 16 bits are [`COMBAT_MAGIC`] AND the kind
/// discriminant is inside the ratified vocabulary. Foreign payloads
/// — the legacy NPC layout, routine (`0x5E47`), sustained
/// (`0x5F12`/`0x0F12`), burst (`0x5336`), adversary (`0xB00B`) —
/// never decode as combat.
pub fn combat_payload_parts(payload: u64) -> Option<(CombatEventKind, CombatantId, u16)> {
    if payload >> 48 != COMBAT_MAGIC {
        return None;
    }
    let kind = CombatEventKind::from_discriminant((payload >> 40) & 0xFF)?;
    let combatant = ((payload >> 16) & 0x00FF_FFFF) as u32;
    let detail = (payload & 0xFFFF) as u16;
    Some((kind, combatant, detail))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() <= 1e-4
    }

    fn box_at(id: CombatantId, min: [f32; 3], max: [f32; 3]) -> Combatant {
        Combatant {
            id,
            hit_box: StaticCollider { min, max },
        }
    }

    fn request(origin: [f32; 3], direction: [f32; 3], range: f32) -> AttackRequest {
        AttackRequest {
            attacker: 0,
            origin,
            direction,
            profile: AttackProfile {
                range,
                occlusion_margin: 0.0,
            },
        }
    }

    #[test]
    fn resolves_nearest_target_with_a_clear_line() {
        let world = CollisionWorld::new(0.0);
        let targets = [
            box_at(1, [-0.5, 0.0, -5.0], [0.5, 1.8, -4.0]),
            box_at(2, [-0.5, 0.0, -9.0], [0.5, 1.8, -8.0]),
        ];
        let hit = resolve_attack(
            &world,
            &request([0.0, 1.6, 0.0], [0.0, 0.0, -2.0], 30.0),
            &targets,
        );
        match hit {
            HitResolution::Hit {
                target,
                distance,
                point,
            } => {
                assert_eq!(target, 1, "the nearer target wins");
                assert!(approx(distance, 4.0), "entry face at z=-4: {distance}");
                assert!(
                    approx(point[2], -4.0),
                    "the impact point is on the entry face"
                );
                assert!(approx(point[1], 1.6), "the trace stays at eye height");
            }
            other => panic!("expected a hit, got {other:?}"),
        }
    }

    #[test]
    fn unnormalized_directions_are_normalized_by_the_resolver() {
        let world = CollisionWorld::new(0.0);
        let targets = [box_at(7, [-0.5, 0.0, -5.0], [0.5, 1.8, -4.0])];
        let a = resolve_attack(
            &world,
            &request([0.0, 1.6, 0.0], [0.0, 0.0, -1.0], 30.0),
            &targets,
        );
        let b = resolve_attack(
            &world,
            &request([0.0, 1.6, 0.0], [0.0, 0.0, -7.5], 30.0),
            &targets,
        );
        assert_eq!(a, b, "aim magnitude never changes the resolution");
    }

    #[test]
    fn point_blank_target_containing_the_origin_hits_at_zero() {
        let world = CollisionWorld::new(0.0);
        let targets = [box_at(3, [-1.0, 0.0, -1.0], [1.0, 2.0, 1.0])];
        match resolve_attack(
            &world,
            &request([0.0, 1.6, 0.0], [0.0, 0.0, -1.0], 30.0),
            &targets,
        ) {
            HitResolution::Hit {
                target, distance, ..
            } => {
                assert_eq!(target, 3);
                assert_eq!(
                    distance, 0.0,
                    "a target containing the origin is point-blank"
                );
            }
            other => panic!("expected a point-blank hit, got {other:?}"),
        }
    }

    #[test]
    fn thin_wall_occludes_exactly_no_sampling_aliasing() {
        // A 0.05 m wall between the eye and the target: the exact
        // slab form blocks it (B2 — under any sampling march with a
        // step larger than the wall, this test fails).
        let world = CollisionWorld::new(0.0).with_collider(StaticCollider {
            min: [-3.0, 0.0, -2.05],
            max: [3.0, 3.0, -2.0],
        });
        let targets = [box_at(1, [-0.5, 0.0, -5.0], [0.5, 1.8, -4.0])];
        match resolve_attack(
            &world,
            &request([0.0, 1.6, 0.0], [0.0, 0.0, -1.0], 30.0),
            &targets,
        ) {
            HitResolution::Miss {
                kind: MissKind::Occluded { distance, point },
            } => {
                assert!(approx(distance, 2.0), "the wall's near face: {distance}");
                assert!(approx(point[2], -2.0));
            }
            other => panic!("the thin wall must occlude, got {other:?}"),
        }
    }

    #[test]
    fn the_wall_wins_distance_ties() {
        // A wall and a target whose entry faces sit at EXACTLY the
        // same distance (the same f32 arithmetic produces both —
        // both boxes cross the ray, entering at the same z plane):
        // the target must be strictly nearer — the wall blocks.
        let world = CollisionWorld::new(0.0).with_collider(StaticCollider {
            min: [-1.0, 0.0, -2.0],
            max: [1.0, 3.0, -1.0],
        });
        let targets = [box_at(5, [-0.5, 0.0, -2.0], [0.5, 1.8, -1.0])];
        let hit = resolve_attack(
            &world,
            &request([0.0, 1.6, 0.0], [0.0, 0.0, -1.0], 10.0),
            &targets,
        );
        match hit {
            HitResolution::Miss {
                kind: MissKind::Occluded { distance, .. },
            } => {
                assert!(approx(distance, 1.0), "the tied wall entry: {distance}");
            }
            other => panic!("an equal-distance target never beats the wall: {other:?}"),
        }
    }

    #[test]
    fn range_boundary_hits_exactly_at_range_and_misses_beyond() {
        let world = CollisionWorld::new(0.0);
        // Entry face exactly at z = -4 with range 4: t == 1.0 — a
        // hit (t_enter <= 1 is the documented boundary).
        let at_range = [box_at(1, [-0.5, 0.0, -5.0], [0.5, 1.8, -4.0])];
        match resolve_attack(
            &world,
            &request([0.0, 1.6, 0.0], [0.0, 0.0, -1.0], 4.0),
            &at_range,
        ) {
            HitResolution::Hit {
                target, distance, ..
            } => {
                assert_eq!(target, 1);
                assert_eq!(distance, 4.0, "the exactly-at-range boundary hits");
            }
            other => panic!("the at-range boundary must hit, got {other:?}"),
        }
        // Entry face beyond the range: no hit, no occluder — Clear.
        let beyond = [box_at(1, [-0.5, 0.0, -6.0], [0.5, 1.8, -4.1])];
        assert_eq!(
            resolve_attack(
                &world,
                &request([0.0, 1.6, 0.0], [0.0, 0.0, -1.0], 4.0),
                &beyond
            ),
            HitResolution::Miss {
                kind: MissKind::Clear
            }
        );
        // An empty target set with a clear trace is also Clear.
        assert_eq!(
            resolve_attack(
                &world,
                &request([0.0, 1.6, 0.0], [0.0, 0.0, -1.0], 4.0),
                &[]
            ),
            HitResolution::Miss {
                kind: MissKind::Clear
            }
        );
    }

    #[test]
    fn equal_distances_break_to_the_lowest_id_order_independently() {
        let world = CollisionWorld::new(0.0);
        // Two overlapping-in-x boxes, both crossing the ray, both
        // entering at the same z plane: exactly equal distances.
        let a = box_at(7, [-1.0, 0.0, -4.0], [0.2, 1.8, -3.0]);
        let b = box_at(3, [-0.2, 0.0, -4.0], [1.0, 1.8, -3.0]);
        let forward = request([0.0, 1.6, 0.0], [0.0, 0.0, -1.0], 30.0);
        let first = resolve_attack(&world, &forward, &[a, b]);
        let second = resolve_attack(&world, &forward, &[b, a]);
        assert_eq!(first, second, "slice order never changes the selection");
        match first {
            HitResolution::Hit {
                target, distance, ..
            } => {
                assert_eq!(target, 3, "ties break to the lower id");
                assert!(approx(distance, 3.0), "both enter at z=-3: {distance}");
            }
            other => panic!("expected a hit, got {other:?}"),
        }
    }

    #[test]
    fn resolution_is_pure_and_bitwise_repeatable() {
        let world = CollisionWorld::new(0.0).with_collider(StaticCollider {
            min: [-3.0, 0.0, -2.05],
            max: [3.0, 3.0, -2.0],
        });
        let targets = [
            box_at(1, [-0.5, 0.0, -5.0], [0.5, 1.8, -4.0]),
            box_at(2, [-0.5, 0.0, -9.0], [0.5, 1.8, -8.0]),
        ];
        let req = request([0.4, 1.1, 0.3], [0.1, -0.05, -1.0], 27.5);
        let a = resolve_attack(&world, &req, &targets);
        let b = resolve_attack(&world, &req, &targets);
        assert_eq!(a, b, "equal inputs give bitwise-equal results");
    }

    #[test]
    fn invalid_targets_are_skipped_never_displacing_a_valid_candidate() {
        let world = CollisionWorld::new(0.0);
        let valid = box_at(1, [-0.5, 0.0, -5.0], [0.5, 1.8, -4.0]);
        let nan_box = box_at(9, [f32::NAN, 0.0, -3.0], [0.5, 1.8, -2.0]);
        let inverted = box_at(
            8,
            [0.0, 0.0, -3.0],
            [-0.5, 1.8, -2.0], // min > max on x: inverted
        );
        let targets = [nan_box, inverted, valid];
        match resolve_attack(
            &world,
            &request([0.0, 1.6, 0.0], [0.0, 0.0, -1.0], 30.0),
            &targets,
        ) {
            HitResolution::Hit { target, .. } => assert_eq!(
                target, 1,
                "invalid targets never displace the valid candidate"
            ),
            other => panic!("expected the valid candidate, got {other:?}"),
        }
        assert!(!nan_box.hit_box_valid());
        assert!(!inverted.hit_box_valid());
        assert!(valid.hit_box_valid());
    }

    #[test]
    fn request_refusals_are_typed_and_exhaustive() {
        let world = CollisionWorld::new(0.0);
        let targets = [box_at(1, [-0.5, 0.0, -5.0], [0.5, 1.8, -4.0])];
        let cases = [
            (
                request([f32::NAN, 1.6, 0.0], [0.0, 0.0, -1.0], 30.0),
                AttackRefusal::NonFiniteOrigin,
            ),
            (
                request([0.0, f32::INFINITY, 0.0], [0.0, 0.0, -1.0], 30.0),
                AttackRefusal::NonFiniteOrigin,
            ),
            (
                request([0.0, 1.6, 0.0], [0.0, f32::NAN, -1.0], 30.0),
                AttackRefusal::NonFiniteDirection,
            ),
            (
                request([0.0, 1.6, 0.0], [0.0, 0.0, 0.0], 30.0),
                AttackRefusal::ZeroDirection,
            ),
            // Finite components whose squares overflow: no usable
            // length — refused, never a mis-normalized trace.
            (
                request([0.0, 1.6, 0.0], [f32::MAX, f32::MAX, 0.0], 30.0),
                AttackRefusal::ZeroDirection,
            ),
            (
                request([0.0, 1.6, 0.0], [0.0, 0.0, -1.0], -1.0),
                AttackRefusal::InvalidProfile(ProfileField::Range),
            ),
            (
                request([0.0, 1.6, 0.0], [0.0, 0.0, -1.0], f32::NAN),
                AttackRefusal::InvalidProfile(ProfileField::Range),
            ),
            (
                AttackRequest {
                    attacker: 0,
                    origin: [0.0, 1.6, 0.0],
                    direction: [0.0, 0.0, -1.0],
                    profile: AttackProfile {
                        range: 30.0,
                        occlusion_margin: f32::NAN,
                    },
                },
                AttackRefusal::InvalidProfile(ProfileField::OcclusionMargin),
            ),
            (
                AttackRequest {
                    attacker: 0,
                    origin: [0.0, 1.6, 0.0],
                    direction: [0.0, 0.0, -1.0],
                    profile: AttackProfile {
                        range: 30.0,
                        occlusion_margin: -0.1,
                    },
                },
                AttackRefusal::InvalidProfile(ProfileField::OcclusionMargin),
            ),
        ];
        for (req, refusal) in cases {
            assert_eq!(
                resolve_attack(&world, &req, &targets),
                HitResolution::Refused(refusal),
                "the request {req:?} must refuse as {refusal:?}"
            );
        }
        // Zero range and zero margin are VALID boundaries, not
        // refusals (a point-blank profile).
        assert!(matches!(
            resolve_attack(
                &world,
                &AttackRequest {
                    attacker: 0,
                    origin: [0.0, 1.6, 0.0],
                    direction: [0.0, 0.0, -1.0],
                    profile: AttackProfile {
                        range: 0.0,
                        occlusion_margin: 0.0,
                    }
                },
                &[box_at(1, [-1.0, 0.0, -1.0], [1.0, 2.0, 1.0])]
            ),
            HitResolution::Hit { target: 1, .. }
        ));
    }

    #[test]
    fn segment_box_hit_matches_the_world_query_on_identical_boxes() {
        // The parity claim (B2's evidence): the public single-box
        // primitive performs EXACTLY the world query's per-collider
        // arithmetic — identical results for every segment shape.
        let box_extent = StaticCollider {
            min: [-1.0, 0.0, -6.0],
            max: [1.0, 2.0, -4.0],
        };
        let world = CollisionWorld::new(0.0).with_collider(box_extent);
        let segments = [
            ([0.0f32, 1.6, 0.0], [0.0, 1.6, -10.0]), // straight through
            ([0.0, 1.6, 0.0], [0.0, 1.6, -5.0]),     // ends inside the box
            ([0.0, 1.6, 0.0], [0.0, 1.6, -3.0]),     // ends before the box
            ([0.5, 1.6, -5.0], [0.5, 1.6, -4.5]),    // starts inside
            ([3.0, 1.6, 0.0], [3.0, 1.6, -10.0]),    // passes beside
            ([0.0, 5.0, 0.0], [0.0, 0.0, -10.0]),    // diagonal, enters the top
            ([2.0, 1.0, -5.0], [-2.0, 1.0, -5.0]),   // perpendicular cross
        ];
        for (from, to) in segments {
            assert_eq!(
                segment_box_hit(from, to, box_extent),
                world.segment_hit(from, to, 0.0),
                "parity breaks on the segment {from:?} → {to:?}"
            );
        }
    }

    #[test]
    fn damage_transitions_hit_the_documented_boundaries() {
        let mut health = health_full(100.0).unwrap();
        assert_eq!(health.current, 100.0);

        match health.apply_damage(30.0).unwrap() {
            DamageOutcome::Wounded { remaining, dealt } => {
                assert_eq!(remaining, 70.0, "plain f32 subtraction, exactly");
                assert_eq!(dealt, 30.0);
            }
            other => panic!("expected Wounded, got {other:?}"),
        }

        // THE boundary: current == amount → Defeated at EXACTLY 0.0.
        match health.apply_damage(70.0).unwrap() {
            DamageOutcome::Defeated { dealt } => {
                assert_eq!(dealt, 70.0);
                assert_eq!(health.current, 0.0, "saturating, never negative");
                assert!(health.is_defeated());
            }
            other => panic!("the exact boundary must declare defeat, got {other:?}"),
        }

        // Crossing zero: current < amount → Defeated at 0.0.
        let mut small = health_full(10.0).unwrap();
        assert!(matches!(
            small.apply_damage(25.0).unwrap(),
            DamageOutcome::Defeated { dealt: 25.0 }
        ));
        assert_eq!(small.current, 0.0);

        // Just short of the boundary stays Wounded with a positive
        // remainder.
        let mut edge = health_full(30.0).unwrap();
        match edge.apply_damage(29.5).unwrap() {
            DamageOutcome::Wounded { remaining, .. } => {
                assert!(remaining > 0.0, "the remainder is strictly positive");
            }
            other => panic!("just-short must stay Wounded, got {other:?}"),
        }

        // Defeat declared from an already-defeated value (0 <= any
        // positive amount).
        let mut defeated = health_full(5.0).unwrap();
        defeated.apply_damage(5.0).unwrap();
        assert!(matches!(
            defeated.apply_damage(1.0).unwrap(),
            DamageOutcome::Defeated { .. }
        ));
    }

    #[test]
    fn zero_damage_is_a_valid_graze_not_a_refusal() {
        let mut health = health_full(100.0).unwrap();
        assert_eq!(health.apply_damage(0.0).unwrap(), DamageOutcome::Graze);
        assert_eq!(health.current, 100.0, "a graze changes nothing");

        // A graze on a defeated value: stays defeated.
        let mut defeated = health_full(5.0).unwrap();
        defeated.apply_damage(5.0).unwrap();
        assert_eq!(defeated.apply_damage(0.0).unwrap(), DamageOutcome::Graze);
        assert!(defeated.is_defeated());
    }

    #[test]
    fn health_refusals_leave_the_value_bit_identical() {
        let mut health = health_full(100.0).unwrap();
        health.apply_damage(30.0).unwrap();
        let before = health;
        assert_eq!(
            health.apply_damage(f32::NAN),
            Err(HealthError::NonFiniteAmount)
        );
        assert_eq!(
            health.apply_damage(f32::INFINITY),
            Err(HealthError::NonFiniteAmount)
        );
        assert_eq!(health.apply_damage(-1.0), Err(HealthError::NegativeAmount));
        assert_eq!(health, before, "refusals never mutate");

        // An externally-invalid state refuses every transition.
        let mut broken = Health {
            current: -1.0,
            maximum: 100.0,
        };
        assert_eq!(broken.apply_damage(5.0), Err(HealthError::InvalidState));
        let mut nonfinite = Health {
            current: f32::NAN,
            maximum: 100.0,
        };
        assert_eq!(nonfinite.apply_heal(5.0), Err(HealthError::InvalidState));

        // Construction refusals are typed.
        assert_eq!(health_full(-5.0), Err(HealthError::InvalidMaximum));
        assert_eq!(health_full(f32::NAN), Err(HealthError::InvalidMaximum));
        assert_eq!(health_full(f32::INFINITY), Err(HealthError::InvalidMaximum));
    }

    #[test]
    fn zero_maximum_is_a_valid_born_defeated_combatant() {
        let health = health_full(0.0).unwrap();
        assert_eq!(health.current, 0.0);
        assert_eq!(health.maximum, 0.0);
        assert!(health.is_defeated(), "born defeated is a valid state");
        // Damage on it: 0 <= any positive amount → Defeated again.
        let mut health = health;
        assert!(matches!(
            health.apply_damage(1.0).unwrap(),
            DamageOutcome::Defeated { .. }
        ));
    }

    #[test]
    fn healing_saturates_at_maximum_with_applied_amounts() {
        let mut health = health_full(100.0).unwrap();
        health.apply_damage(70.0).unwrap();
        match health.apply_heal(50.0).unwrap() {
            HealOutcome::Restored { current, gained } => {
                assert_eq!(current, 80.0);
                assert_eq!(gained, 50.0);
            }
            other => panic!("expected Restored, got {other:?}"),
        }
        match health.apply_heal(50.0).unwrap() {
            HealOutcome::Capped { current, gained } => {
                assert_eq!(current, 100.0);
                assert_eq!(gained, 20.0, "gained is the APPLIED amount only");
            }
            other => panic!("expected Capped, got {other:?}"),
        }
        // A revive from defeated is uniform mechanics (the policy
        // decision is the driver's).
        let mut defeated = health_full(40.0).unwrap();
        defeated.apply_damage(40.0).unwrap();
        assert!(matches!(
            defeated.apply_heal(15.0).unwrap(),
            HealOutcome::Restored {
                current: 15.0,
                gained: 15.0
            }
        ));
        // A zero heal is a valid no-op.
        assert!(matches!(
            defeated.apply_heal(0.0).unwrap(),
            HealOutcome::Restored { gained: 0.0, .. }
        ));
    }

    #[test]
    fn transition_sequences_are_bitwise_repeatable() {
        // The §6 float-discipline claim, exercised: two separately
        // constructed sequences give BITWISE-equal values (the
        // transition path is add/sub/compare only).
        let run = || {
            let mut health = health_full(100.0).unwrap();
            for amount in [30.0f32, 12.5, 7.25, 50.0] {
                let _ = health.apply_damage(amount);
            }
            let _ = health.apply_heal(33.75);
            (health.current.to_bits(), health.maximum.to_bits())
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn codec_round_trips_every_kind() {
        let kinds = [
            CombatEventKind::Fire,
            CombatEventKind::Impact,
            CombatEventKind::Damage,
            CombatEventKind::Defeat,
            CombatEventKind::Heal,
            CombatEventKind::Reset,
        ];
        for kind in kinds {
            for (combatant, detail) in [(0u32, 0u16), (1, 0xFFFF), (0x00FF_FFFF, 1), (1234, 5678)] {
                let payload = combat_payload(kind, combatant, detail)
                    .expect("in-range ids and u16 details always encode");
                assert_eq!(
                    combat_payload_parts(payload),
                    Some((kind, combatant, detail)),
                    "lossless round trip for {kind:?}"
                );
            }
        }
    }

    #[test]
    fn codec_refuses_out_of_bound_ids_never_truncating() {
        // The largest in-range id (2^24 − 1) round trips — the bound
        // itself is the refusal boundary.
        let top = combat_payload(CombatEventKind::Fire, 0x00FF_FFFF, 0).map(combat_payload_parts);
        assert_eq!(
            top,
            Some(Some((CombatEventKind::Fire, 0x00FF_FFFF, 0))),
            "the largest in-range id encodes and decodes losslessly"
        );
        assert_eq!(
            combat_payload(CombatEventKind::Fire, 0x100_0000, 0),
            None,
            "an id at the 2^24 bound refuses"
        );
        assert_eq!(
            combat_payload(CombatEventKind::Damage, u32::MAX, 9),
            None,
            "a u32-max id refuses"
        );
    }

    #[test]
    fn foreign_payloads_never_decode_as_combat() {
        // Every live namespace, plus the legacy layout and an
        // unknown-kind combat-magic payload: all FOREIGN.
        let foreign = [
            0u64,                                  // legacy npc (npc 0, activity 0)
            (5u64 << 16) | 3,                      // legacy npc (npc 5, activity 3)
            (0x5E47u64 << 48) | (7u64 << 32) | 99, // npc routine magic
            (0x5F12u64 << 48) | 1,                 // sustained routine magic
            (0x0F12u64 << 48) | 1,                 // sustained demand magic
            (0x5336u64 << 48) | 1,                 // soak burst magic
            (0xB00Bu64 << 48) | (3u64 << 32) | 7,  // SEAT B's adversary namespace
            u64::MAX,
        ];
        for payload in foreign {
            assert_eq!(
                combat_payload_parts(payload),
                None,
                "the foreign payload {payload:#x} never decodes as combat"
            );
        }
        // A combat-magic payload with an UNRATIFIED kind discriminant
        // is foreign too (a coerced decode would fabricate meaning).
        let unknown_kind = (COMBAT_MAGIC << 48) | (7u64 << 40) | (1u64 << 16) | 1;
        assert_eq!(combat_payload_parts(unknown_kind), None);
        let zero_kind = (COMBAT_MAGIC << 48) | (1u64 << 16) | 1;
        assert_eq!(
            combat_payload_parts(zero_kind),
            None,
            "kind 0 is not ratified"
        );
    }

    #[test]
    fn combat_fingerprint_domain_pins_a_fixed_scenario() {
        // The NEW domain's guard value (`engine-core/combat/1`):
        // measured once, pinned here — any change to this digest is
        // an investigation, never a silent repin.
        let mut fp = crate::fingerprint::Fingerprinter::new();
        fp.write_str(COMBAT_FINGERPRINT_DOMAIN);
        // A fixed health sequence: 100 → −30 (Wounded 70) → −70
        // (Defeated 0) → heal 25 (Restored 25).
        let mut health = health_full(100.0).unwrap();
        for amount in [30.0f32, 70.0] {
            let outcome = health.apply_damage(amount).unwrap();
            fp.write_u32(health.current.to_bits());
            fp.write_u8(match outcome {
                DamageOutcome::Graze => 0,
                DamageOutcome::Wounded { .. } => 1,
                DamageOutcome::Defeated { .. } => 2,
            });
        }
        let healed = health.apply_heal(25.0).unwrap();
        fp.write_u32(health.current.to_bits());
        fp.write_u8(match healed {
            HealOutcome::Restored { .. } => 1,
            HealOutcome::Capped { .. } => 2,
        });
        // A fixed resolution: the thin-wall occlusion scenario.
        let world = CollisionWorld::new(0.0).with_collider(StaticCollider {
            min: [-3.0, 0.0, -2.05],
            max: [3.0, 3.0, -2.0],
        });
        let targets = [box_at(1, [-0.5, 0.0, -5.0], [0.5, 1.8, -4.0])];
        let resolution = resolve_attack(
            &world,
            &request([0.0, 1.6, 0.0], [0.0, 0.0, -1.0], 30.0),
            &targets,
        );
        match resolution {
            HitResolution::Miss {
                kind: MissKind::Occluded { distance, .. },
            } => {
                fp.write_u8(3);
                fp.write_u32(distance.to_bits());
            }
            _ => panic!("the fixed scenario must occlude"),
        }
        // A fixed payload encode.
        fp.write_u64(
            combat_payload(CombatEventKind::Defeat, 42, 7).expect("the fixed encode is in range"),
        );
        // Measured at the first run of this test (2026-11-18) and
        // pinned: any change to this digest is an investigation,
        // never a silent repin.
        assert_eq!(fp.finish_hex(), "8a3978034487412e");
    }

    // =================================================================
    // The P2 B06 broad-phase battery — the StaticGrid test template
    // (player.rs's L5 battery: force_broad_phase /
    // broad_phase_equivalence_battery / broad_phase_gate_boundary /
    // broad_phase_scaling_evidence) applied to the combatant scan.
    // =================================================================

    use crate::collision::RotatedCollider;

    /// A deterministic xorshift64* stream for fixture generation
    /// (the player.rs test module's own FixtureRng twin — std-only,
    /// seeded per fixture, so every run of the battery builds the
    /// IDENTICAL population; the no-RNG discipline applied to
    /// tests).
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

    /// The fixture roster shapes (the design's §9.1 census
    /// distributions, applied to combatants): Spread — a jittered
    /// ±60 m lattice with every 7th base EXACTLY on cell boundaries
    /// (the floor-edge class) and every 13th ~8× farther out (sparse
    /// cells, mostly-empty walks); Cluster — the whole roster inside
    /// a few cells (the all-in-one-cell class: maximal per-cell
    /// lists, the grid arm doing MORE work than linear — the
    /// equivalence claim must hold there too); Ring — a
    /// boundary-saturated ring (equidistant candidate classes).
    #[derive(Clone, Copy, Debug)]
    enum Shape {
        Spread,
        Cluster,
        Ring,
    }

    /// The deterministic fixture roster: human-scale bodies (the
    /// `adversary_body` extent class), distinct ids (the slice
    /// position plus one), and — when `corrupt` — every 9th entry
    /// an INVALID hitbox (alternating NaN / inverted), the skip
    /// rule's class.
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
            let half = rng.range(0.25, 0.6);
            let height = rng.range(1.2, 2.0);
            let valid = StaticCollider {
                min: [base[0] - half, base[1], base[2] - half],
                max: [base[0] + half, base[1] + height, base[2] + half],
            };
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
                valid
            };
            roster.push(Combatant {
                id: index as u32 + 1,
                hit_box,
            });
        }
        roster
    }

    /// Forces the grid on regardless of the gate (the battery's
    /// control: the SAME roster through both paths must produce
    /// bit-identical results). Returns whether the grid is active
    /// after the call (an out-of-domain VALID box cannot be forced
    /// on — the fail-closed domain).
    fn force_index_on(targets: &[Combatant]) -> (CombatIndex<'_>, bool) {
        let mut index = CombatIndex {
            population: targets,
            phase: CombatPhase::Linear,
        };
        match build_combatant_grid(targets) {
            Some(grid) => {
                index.phase = CombatPhase::Grid(grid);
                (index, true)
            }
            None => {
                index.phase = CombatPhase::Unusable;
                (index, false)
            }
        }
    }

    /// Bit-level equality — the battery's claim IS bit-identity, so
    /// even NaN results compare by their bit patterns.
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

    /// The B06 equivalence battery (the design's L5 acceptance-row
    /// template): for fixture rosters BELOW and ABOVE the gate, in
    /// three distribution shapes (plus the corrupted-entry class),
    /// every resolution through the grid path vs the exact linear
    /// primitive on the IDENTICAL roster — bit-for-bit, ties
    /// included, occlusion included (two wall worlds), point-blank
    /// origins included, and the long-range queries that trip the
    /// [`GRID_CELL_WASTE_FACTOR`] guard (which regime runs depends
    /// on the population — both are the battery's claim, since the
    /// guard's fallback IS the linear path). The below-gate rows
    /// force-activate the grid (the gate otherwise keeps those
    /// rosters linear); the at/above-gate rows use the real
    /// [`CombatIndex::build`].
    #[test]
    fn combat_index_equivalence_battery() {
        let rows = [
            (40usize, Shape::Spread, false),
            (96, Shape::Cluster, false),
            (127, Shape::Spread, false), // just below the gate, grid forced on
            (128, Shape::Spread, false), // AT the gate — build activates
            (150, Shape::Ring, false),
            (150, Shape::Spread, true), // corrupted entries: skipped, never poisoned
            (1000, Shape::Cluster, false),
        ];
        for (count, shape, corrupt) in rows {
            for seed in [0xC0AB_0001u64, 0xC0AB_0010, 0xC0AB_0110] {
                let roster = fixture_roster(seed, count, shape, corrupt);
                let (index, grid_active) = if count >= COMBAT_INDEX_GATE {
                    let index = CombatIndex::build(&roster);
                    let active = matches!(index.phase, CombatPhase::Grid(_));
                    (index, active)
                } else {
                    force_index_on(&roster)
                };
                assert!(
                    grid_active,
                    "seed {seed}: the fixture must be grid-usable \
                     (every valid hitbox finite and in-domain)"
                );
                let label = format!("{count}/{shape:?} corrupt={corrupt} @ {seed}");

                // The wall worlds the traces run through (occlusion
                // is half the equivalence claim): empty, and a
                // walled field with a full-height wall, a thin
                // wall, and a rotated prism.
                let empty = CollisionWorld::new(0.0);
                let walled = CollisionWorld::new(0.0)
                    .with_collider(StaticCollider {
                        min: [-40.0, 0.0, -20.05],
                        max: [40.0, 4.0, -20.0],
                    })
                    .with_collider(StaticCollider {
                        min: [10.0, 0.0, -60.0],
                        max: [10.05, 4.0, 20.0],
                    })
                    .with_rotated_collider(Some(
                        RotatedCollider::new([0.0, 1.5, -30.0], [4.0, 1.5, 0.5], 0.7)
                            .expect("the fixture prism is finite and positive"),
                    ));
                for (world, world_label) in [(&empty, "empty"), (&walled, "walled")] {
                    let mut rng = FixtureRng(seed ^ 0x9E37_79B9_7F4A_7C15);
                    for query in 0..48 {
                        // Point-blank every 12th query: the origin
                        // sits inside a roster body (t = 0) — or, for
                        // a corrupted body, inside NaN (both paths
                        // must refuse identically).
                        let origin = if query % 12 == 0 {
                            let body = roster[(query * 7) % roster.len()].hit_box;
                            [
                                (body.min[0] + body.max[0]) * 0.5,
                                (body.min[1] + body.max[1]) * 0.5,
                                (body.min[2] + body.max[2]) * 0.5,
                            ]
                        } else {
                            [
                                rng.range(-50.0, 50.0),
                                rng.range(0.5, 2.2),
                                rng.range(-50.0, 50.0),
                            ]
                        };
                        let direction = match query % 4 {
                            0 => [0.0, 0.0, -1.0],  // dead ahead (the d == 0 slab branches)
                            1 => [1.0, 0.25, -1.0], // diagonal
                            2 => [rng.range(-1.0, 1.0), rng.range(-0.4, 0.4), -1.0],
                            _ => [rng.range(-1.0, 1.0), 0.0, rng.range(-1.0, 1.0)],
                        };
                        let range = [8.0f32, 30.0, 120.0][query % 3];
                        let margin = [0.0f32, 0.5][query % 2];
                        let request = AttackRequest {
                            attacker: 0,
                            origin,
                            direction,
                            profile: AttackProfile {
                                range,
                                occlusion_margin: margin,
                            },
                        };
                        let grid_result = index.resolve_attack(world, &request);
                        let linear_result = resolve_attack(world, &request, &roster);
                        assert!(
                            same_resolution(grid_result, linear_result),
                            "{label} {world_label} q{query}: \
                             grid {grid_result:?} vs linear {linear_result:?}"
                        );
                    }
                }

                // Slice order: the (distance, id) selection is
                // order-independent for distinct ids — pin it
                // through the grid arm too (the candidate re-sort
                // must not leak hash order into the result).
                let mut reversed = roster.clone();
                reversed.reverse();
                let (rev_index, rev_active) = if count >= COMBAT_INDEX_GATE {
                    let index = CombatIndex::build(&reversed);
                    let active = matches!(index.phase, CombatPhase::Grid(_));
                    (index, active)
                } else {
                    force_index_on(&reversed)
                };
                assert!(
                    rev_active,
                    "seed {seed}: the reversed fixture must be grid-usable"
                );
                let world = CollisionWorld::new(0.0);
                let mut rng = FixtureRng(seed ^ 0xD1CE_B00B_5EED_1A5E);
                for query in 0..16 {
                    let origin = [
                        rng.range(-50.0, 50.0),
                        rng.range(0.5, 2.2),
                        rng.range(-50.0, 50.0),
                    ];
                    let direction = [
                        rng.range(-1.0, 1.0),
                        rng.range(-0.4, 0.4),
                        rng.range(-1.0, 1.0),
                    ];
                    let request = AttackRequest {
                        attacker: 0,
                        origin,
                        direction,
                        profile: AttackProfile {
                            range: 30.0,
                            occlusion_margin: 0.0,
                        },
                    };
                    assert!(
                        same_resolution(
                            index.resolve_attack(&world, &request),
                            rev_index.resolve_attack(&world, &request)
                        ),
                        "{label} perm q{query}: slice order leaks into the grid path"
                    );
                }
            }
        }
    }

    /// The exact tie classes (the design's L5 discriminating-test
    /// template): the equal-distance lowest-id tie, the
    /// duplicate-(id, distance) tie, the wall-wins-tie, the
    /// exactly-at-range boundary, and the point-blank hit — each
    /// grid-vs-linear bit-identical AND pinned to the expected
    /// selection, plus the corrupted-entry class that must NOT
    /// poison (the deliberate difference from the static grid).
    #[test]
    fn combat_index_ties_resolve_identically() {
        let world = CollisionWorld::new(0.0);

        // (A) The equal-distance lowest-id tie through the grid arm:
        // two boxes entering at the same z plane, ids 7 and 3 — both
        // paths select 3, in EITHER slice order.
        let a = box_at(7, [-1.0, 0.0, -4.0], [0.2, 1.8, -3.0]);
        let b = box_at(3, [-0.2, 0.0, -4.0], [1.0, 1.8, -3.0]);
        let forward = request([0.0, 1.6, 0.0], [0.0, 0.0, -1.0], 30.0);
        for roster in [&[a, b], &[b, a]] {
            let (index, active) = force_index_on(roster);
            assert!(active);
            let grid_result = index.resolve_attack(&world, &forward);
            let linear_result = resolve_attack(&world, &forward, roster);
            assert!(
                same_resolution(grid_result, linear_result),
                "the tie resolves identically"
            );
            match grid_result {
                HitResolution::Hit {
                    target, distance, ..
                } => {
                    assert_eq!(target, 3, "the lowest id wins the tie through the grid");
                    assert!(approx(distance, 3.0), "both enter at z=-3: {distance}");
                }
                other => panic!("expected the tie hit, got {other:?}"),
            }
        }

        // (B) Duplicate (id, distance) pairs: the selection is the
        // lexicographic min of the candidate (distance, id)
        // multiset, so duplicate entries at identical pairs are
        // UNOBSERVABLE in the resolution (same target, distance, and
        // point whichever entry wins) — pinned identical on both
        // paths, with and without the duplicate.
        let duplicate = box_at(5, [-0.5, 0.0, -4.0], [0.5, 1.8, -3.0]);
        let pairs: [&[Combatant]; 2] = [&[duplicate, duplicate], &[duplicate]];
        for roster in pairs {
            let (index, active) = force_index_on(roster);
            assert!(active);
            let grid_result = index.resolve_attack(&world, &forward);
            let linear_result = resolve_attack(&world, &forward, roster);
            assert!(
                same_resolution(grid_result, linear_result),
                "the duplicate pair resolves identically"
            );
            assert!(matches!(
                grid_result,
                HitResolution::Hit {
                    target: 5,
                    distance: d,
                    ..
                } if d == 3.0
            ));
        }

        // (C) The wall wins distance ties through the grid arm: a
        // wall and a target whose entry faces sit at EXACTLY the
        // same distance — the target must be STRICTLY nearer; it is
        // not, so Occluded.
        let tie_world = CollisionWorld::new(0.0).with_collider(StaticCollider {
            min: [-1.0, 0.0, -2.0],
            max: [1.0, 3.0, -1.0],
        });
        let tied = box_at(5, [-0.5, 0.0, -2.0], [0.5, 1.8, -1.0]);
        let tied_roster = [tied];
        let (index, active) = force_index_on(&tied_roster);
        assert!(active);
        let tie_request = request([0.0, 1.6, 0.0], [0.0, 0.0, -1.0], 10.0);
        let grid_result = index.resolve_attack(&tie_world, &tie_request);
        let linear_result = resolve_attack(&tie_world, &tie_request, &tied_roster);
        assert!(same_resolution(grid_result, linear_result));
        assert!(
            matches!(
                grid_result,
                HitResolution::Miss {
                    kind: MissKind::Occluded { .. }
                }
            ),
            "an equal-distance target never beats the wall through the grid"
        );

        // (D) The exactly-at-range boundary (t == 1.0) and the
        // point-blank origin-inside hit (t == 0) through the grid.
        let at_range = [box_at(1, [-0.5, 0.0, -5.0], [0.5, 1.8, -4.0])];
        let boundary_request = request([0.0, 1.6, 0.0], [0.0, 0.0, -1.0], 4.0);
        let (index, active) = force_index_on(&at_range);
        assert!(active);
        let grid_result = index.resolve_attack(&world, &boundary_request);
        assert!(same_resolution(
            grid_result,
            resolve_attack(&world, &boundary_request, &at_range)
        ));
        assert!(matches!(
            grid_result,
            HitResolution::Hit {
                target: 1,
                distance: 4.0,
                ..
            }
        ));
        let inside = [box_at(3, [-1.0, 0.0, -1.0], [1.0, 2.0, 1.0])];
        let point_blank = request([0.0, 1.6, 0.0], [0.0, 0.0, -1.0], 30.0);
        let (index, active) = force_index_on(&inside);
        assert!(active);
        let grid_result = index.resolve_attack(&world, &point_blank);
        assert!(same_resolution(
            grid_result,
            resolve_attack(&world, &point_blank, &inside)
        ));
        assert!(matches!(
            grid_result,
            HitResolution::Hit {
                target: 3,
                distance: 0.0,
                ..
            }
        ));

        // (E) Corrupted entries never poison the combat index (the
        // combat skip rule — the DELIBERATE difference from the
        // static grid's fail-closed poisoning, whose linear arms
        // consult raw NaN faces): a NaN and an inverted box beside
        // the valid candidate, the grid STAYS active, and the valid
        // candidate still wins on both paths.
        let valid = box_at(1, [-0.5, 0.0, -5.0], [0.5, 1.8, -4.0]);
        let nan_box = box_at(9, [f32::NAN, 0.0, -3.0], [0.5, 1.8, -2.0]);
        let inverted = box_at(8, [0.0, 0.0, -3.0], [-0.5, 1.8, -2.0]);
        let roster = [nan_box, inverted, valid];
        let (index, active) = force_index_on(&roster);
        assert!(active, "invalid hitboxes never poison the combat index");
        let skip_request = request([0.0, 1.6, 0.0], [0.0, 0.0, -1.0], 30.0);
        let grid_result = index.resolve_attack(&world, &skip_request);
        assert!(same_resolution(
            grid_result,
            resolve_attack(&world, &skip_request, &roster)
        ));
        assert!(matches!(grid_result, HitResolution::Hit { target: 1, .. }));
    }

    /// The gate boundary (the L5 acceptance-row template): at 127
    /// combatants the index is the inert linear fast path (the
    /// default is structural, not caller discipline); the 128th
    /// combatant ACTIVATES the grid; the switch is
    /// behavior-invisible at the exact boundary; a different-length
    /// roster is simply a different index under Q02's borrowing
    /// contract (the old structural-mismatch guard, superseded); the
    /// out-of-domain VALID box poisons (permanently linear for the
    /// roster) while a trace that genuinely REACHES the far box
    /// still hits it (the fallback's honesty); an out-of-domain
    /// QUERY extent and the waste-guard regime fall back per query
    /// on a healthy grid.
    #[test]
    fn combat_index_gate_boundary() {
        let world = CollisionWorld::new(0.0).with_collider(StaticCollider {
            min: [-40.0, 0.0, -20.05],
            max: [40.0, 4.0, -20.0],
        });

        // 127: below the gate — the inert linear index.
        let below = fixture_roster(0x0B06_0001, 127, Shape::Spread, false);
        let below_index = CombatIndex::build(&below);
        assert!(
            matches!(below_index.phase, CombatPhase::Linear),
            "127: below the gate"
        );
        assert_eq!(below_index.population.len(), 127);

        // The 128th combatant crosses the gate: the grid activates.
        let at_roster = fixture_roster(0x0B06_0001, 128, Shape::Spread, false);
        let at_index = CombatIndex::build(&at_roster);
        assert!(
            matches!(at_index.phase, CombatPhase::Grid(_)),
            "the 128th combatant activates the grid"
        );

        // The switch is behavior-invisible at the exact boundary (in
        // both directions from it).
        let mut rng = FixtureRng(0x0B06_CAFE);
        for query in 0..32 {
            let origin = [
                rng.range(-50.0, 50.0),
                rng.range(0.5, 2.2),
                rng.range(-50.0, 50.0),
            ];
            let direction = [rng.range(-1.0, 1.0), rng.range(-0.3, 0.3), -1.0];
            let request = AttackRequest {
                attacker: 0,
                origin,
                direction,
                profile: AttackProfile {
                    range: 30.0,
                    occlusion_margin: 0.1,
                },
            };
            assert!(
                same_resolution(
                    at_index.resolve_attack(&world, &request),
                    resolve_attack(&world, &request, &at_roster)
                ),
                "q{query}: the gate switch is behavior-invisible"
            );
            assert!(
                same_resolution(
                    below_index.resolve_attack(&world, &request),
                    resolve_attack(&world, &request, &below)
                ),
                "q{query}: the below-gate index is the linear path"
            );
        }

        // Q02's ownership contract at the boundary: the index answers
        // over ITS OWN borrowed roster — the old API's separate
        // `targets` parameter (whose length-only guard passed a
        // same-length different-content roster and served a WRONG
        // answer) no longer exists. The discriminating witness lives
        // in `combat_index_roster_contract_refuses_wrong_roster`
        // below; here the boundary re-proves that an index built
        // over a DIFFERENT-length roster is a different index that
        // answers over that roster — the length-mismatch fallback of
        // the old guard is now simply the borrow.
        let grown = fixture_roster(0x0B06_0002, 140, Shape::Spread, false);
        let grown_index = CombatIndex::build(&grown);
        assert!(matches!(grown_index.phase, CombatPhase::Grid(_)));
        let mismatch_request = request([0.0, 1.6, 0.0], [0.0, 0.0, -1.0], 30.0);
        assert!(
            same_resolution(
                grown_index.resolve_attack(&world, &mismatch_request),
                resolve_attack(&world, &mismatch_request, &grown)
            ),
            "the grown roster's own index answers it — a different \
             slice is a different index, never a fallback"
        );

        // The fail-closed domain: a VALID hitbox outside the cell
        // domain poisons the roster's index (permanently linear for
        // it), and the poisoned resolve stays bit-identical —
        // including a trace that actually REACHES the far box (the
        // linear path can hit what the grid cannot index).
        let domain_edge = GRID_CELL_LIMIT as f32 * GRID_CELL_SIZE;
        let mut far_roster = fixture_roster(0x0B06_0003, COMBAT_INDEX_GATE, Shape::Spread, false);
        far_roster[0] = Combatant {
            id: 9_999,
            hit_box: StaticCollider {
                min: [domain_edge, 0.0, -1.0],
                max: [domain_edge + 2.0, 2.0, 1.0],
            },
        };
        let far_index = CombatIndex::build(&far_roster);
        assert!(
            matches!(far_index.phase, CombatPhase::Unusable),
            "an out-of-domain valid box poisons"
        );
        let near_request = request([0.0, 1.6, 0.0], [0.0, 0.0, -1.0], 30.0);
        assert!(same_resolution(
            far_index.resolve_attack(&world, &near_request),
            resolve_attack(&world, &near_request, &far_roster)
        ));
        let reaching = AttackRequest {
            attacker: 0,
            origin: [0.0, 1.0, 0.0],
            direction: [1.0, 0.0, 0.0],
            profile: AttackProfile {
                range: 5_000_000.0,
                occlusion_margin: 0.0,
            },
        };
        assert!(same_resolution(
            far_index.resolve_attack(&world, &reaching),
            resolve_attack(&world, &reaching, &far_roster)
        ));
        assert!(
            matches!(
                resolve_attack(&world, &reaching, &far_roster),
                HitResolution::Hit { target: 9_999, .. }
            ),
            "the far box is genuinely hit-able (the fallback's honesty)"
        );

        // An out-of-domain QUERY extent on a HEALTHY grid (a trace
        // to x = +5,000,000 m) falls back per query — bit-identical.
        assert!(same_resolution(
            at_index.resolve_attack(&world, &reaching),
            resolve_attack(&world, &reaching, &at_roster)
        ));

        // The waste-guard regime: a long diagonal over the at-gate
        // roster spans ~3.7 k cells — more than
        // GRID_CELL_WASTE_FACTOR per combatant — so the grid arm
        // itself takes the linear path (identical by fallback).
        let diagonal = AttackRequest {
            attacker: 0,
            origin: [-60.0, 1.0, -60.0],
            direction: [1.0, 0.0, 1.0],
            profile: AttackProfile {
                range: 170.0,
                occlusion_margin: 0.0,
            },
        };
        assert!(same_resolution(
            at_index.resolve_attack(&world, &diagonal),
            resolve_attack(&world, &diagonal, &at_roster)
        ));
    }

    /// Q02's DISCRIMINATING TEST — the case the old length-only
    /// guard could not see: a SAME-LENGTH different-CONTENT roster.
    /// The pre-Q02 API accepted `index.resolve_attack(world,
    /// request, targets)` with `targets` a separate slice; when that
    /// slice had the built roster's length but different hitboxes,
    /// the grid served candidates gathered from the BUILT roster's
    /// cells against the PASSED roster's boxes — a wrong answer. The
    /// reconstruction below emulates exactly that old call with the
    /// current internals (the grid over roster A, the narrow phase
    /// over roster B) and pins that it MISSES a target the linear
    /// scan over B hits; then pins the contract that replaced it:
    /// the index answers over ITS OWN borrowed roster (bit-identical
    /// to the linear scan over that roster), a mutated roster gets a
    /// REBUILT index (the documented cadence), and the wrong cross
    /// call is no longer expressible on the public API.
    #[test]
    fn combat_index_roster_contract_refuses_wrong_roster() {
        let world = CollisionWorld::new(0.0);
        // Roster A: 128 combatants (the gate), all far off the -Z
        // firing line — combatant 5 sits at x ≈ 900 (cell 450), far
        // outside the trace's cell span.
        let mut roster_a = Vec::with_capacity(COMBAT_INDEX_GATE);
        for i in 0..COMBAT_INDEX_GATE as u32 {
            let base = if i == 5 {
                [900.0, 0.0, 900.0]
            } else {
                [
                    10.0 + (i % 16) as f32 * 3.0,
                    0.0,
                    10.0 + (i / 16) as f32 * 3.0,
                ]
            };
            roster_a.push(box_at(
                i + 1,
                [base[0] - 0.45, base[1], base[2] - 0.45],
                [base[0] + 0.45, base[1] + 1.8, base[2] + 0.45],
            ));
        }
        let index_a = CombatIndex::build(&roster_a);
        assert!(matches!(index_a.phase, CombatPhase::Grid(_)));

        // Roster B: SAME LENGTH, one different content — combatant 5
        // moved onto the firing line (the box the trace now hits).
        let mut roster_b = roster_a.clone();
        roster_b[5] = box_at(6, [-0.5, 0.0, -5.0], [0.5, 1.8, -4.0]);
        assert_eq!(
            roster_a.len(),
            roster_b.len(),
            "same length by construction"
        );

        let request = AttackRequest {
            attacker: 0,
            origin: [0.0, 1.6, 0.0],
            direction: [0.0, 0.0, -1.0],
            profile: AttackProfile {
                range: 30.0,
                occlusion_margin: 0.0,
            },
        };

        // The linear authorities: A has nobody on the line (Miss),
        // B has combatant 5's box at entry z = −4 (Hit, distance 4).
        let linear_a = resolve_attack(&world, &request, &roster_a);
        let linear_b = resolve_attack(&world, &request, &roster_b);
        assert!(matches!(linear_a, HitResolution::Miss { .. }));
        assert!(
            matches!(
                linear_b,
                HitResolution::Hit {
                    target: 6,
                    distance,
                    ..
                } if same_f32(distance, 4.0)
            ),
            "the moved box is on the line: {linear_b:?}"
        );

        // THE OLD API'S WRONG ANSWER, reconstructed: candidates from
        // A's grid (position 5's A-cell at x ≈ 900 is NOT in the
        // trace's cell span, so it is not a candidate), narrow phase
        // over B's boxes — the moved target is invisible to the
        // walk, exactly what `index_a.resolve_attack(world, &req,
        // &roster_b)` computed before Q02. It is a THIRD answer
        // that matches NEITHER authority: a Miss over a roster the
        // linear scan Hits.
        let to = point_at(
            request.origin,
            unit_direction(request.direction).unwrap(),
            30.0,
        );
        let mut best: Option<(CombatantId, f32)> = None;
        let old_style_candidates = index_a.candidates(request.origin, to).expect(
            "the grid arm serves this trace (the waste guard does not \
             fire at this population and extent)",
        );
        assert!(
            !old_style_candidates.contains(&5),
            "the moved combatant's OLD cell is outside the trace's span — \
             the setup's premise"
        );
        for position in old_style_candidates {
            consider_combatant(&roster_b[position as usize], &request, to, None, &mut best);
        }
        let old_style = match best {
            Some((target, distance)) => HitResolution::Hit {
                target,
                distance,
                point: point_at(
                    request.origin,
                    unit_direction(request.direction).unwrap(),
                    distance,
                ),
            },
            None => HitResolution::Miss {
                kind: MissKind::Clear,
            },
        };
        assert!(
            !same_resolution(old_style, linear_b),
            "the pre-Q02 cross-roster call served a WRONG answer: \
             {old_style:?} where the linear authority over the passed \
             roster says {linear_b:?}"
        );
        assert!(matches!(old_style, HitResolution::Miss { .. }));

        // THE NEW CONTRACT: the index answers over its OWN roster —
        // bit-identical to that roster's linear authority — and the
        // cross-roster call is not expressible (there is no targets
        // parameter). The mutation cadence: rebuild for roster B.
        assert!(same_resolution(
            index_a.resolve_attack(&world, &request),
            linear_a
        ));
        let index_b = CombatIndex::build(&roster_b);
        assert!(matches!(index_b.phase, CombatPhase::Grid(_)));
        assert!(same_resolution(
            index_b.resolve_attack(&world, &request),
            linear_b
        ));
        // ...and the rebuilt index's answer is the HIT, not the old
        // wrong Miss: the moved box's NEW cell is in the span.
        assert!(matches!(
            index_b.resolve_attack(&world, &request),
            HitResolution::Hit { target: 6, .. }
        ));
    }

    /// Q01's combat twin — the bounded-construction proof for the
    /// combatant grid: a VALID hitbox spanning (nearly) the whole
    /// cell domain produces ONE oversized entry (never the ~6.4×10¹⁹
    /// per-cell amplification the unbounded insertion loop would
    /// attempt), the index stays ACTIVE (an in-domain oversized
    /// hitbox is not a poison — conservative correctness preserved
    /// through the universally-consulted oversized list), and the
    /// resolve over it is bit-identical to the linear authority,
    /// including a trace that genuinely hits the huge box.
    #[test]
    fn combat_index_oversized_hitbox_is_bounded_not_amplified() {
        let world = CollisionWorld::new(0.0);
        // The control roster at the gate: human-scale bodies.
        let mut control = fixture_roster(0x0D06_0001, COMBAT_INDEX_GATE, Shape::Spread, false);
        let control_index = CombatIndex::build(&control);
        assert!(matches!(control_index.phase, CombatPhase::Grid(_)));
        let CombatPhase::Grid(control_grid) = &control_index.phase else {
            unreachable!("checked above");
        };

        // The whole-domain hitbox appended (same length + 1):
        // coverage (4,000,001)³ ≈ 6.4×10¹⁹ cells — beyond u64, over
        // the cap by any measure, every coordinate finite and IN
        // domain.
        control.push(Combatant {
            id: 9_001,
            hit_box: StaticCollider {
                min: [-4_000_000.0; 3],
                max: [4_000_000.0; 3],
            },
        });
        let huge_index = CombatIndex::build(&control);
        assert!(
            matches!(huge_index.phase, CombatPhase::Grid(_)),
            "an in-domain oversized hitbox keeps the index ACTIVE — \
             oversized, never a poison"
        );
        let CombatPhase::Grid(huge_grid) = &huge_index.phase else {
            unreachable!("checked above");
        };
        assert_eq!(
            huge_grid.oversized,
            vec![COMBAT_INDEX_GATE as u32],
            "the whole-domain hitbox is ONE oversized entry (its slice \
             position)"
        );
        assert_eq!(
            huge_grid.cells, control_grid.cells,
            "the whole-domain hitbox contributed ZERO cell entries"
        );
        let total = control.len();
        let entries: usize = huge_grid.cells.values().map(Vec::len).sum();
        assert!(
            entries + huge_grid.oversized.len() <= (GRID_OVERSIZED_CELL_LIMIT as usize + 1) * total,
            "the resident-entry bound: (cap + 1) per combatant"
        );

        // Equivalence with the huge hitbox present: every trace hits
        // it (every box contains every origin) — the oversized
        // candidate supplies the Hit, bit-identical to the linear
        // authority over the same roster.
        let inside = AttackRequest {
            attacker: 0,
            origin: [0.0, 1.6, 0.0],
            direction: [0.0, 0.0, -1.0],
            profile: AttackProfile {
                range: 30.0,
                occlusion_margin: 0.0,
            },
        };
        let grid_result = huge_index.resolve_attack(&world, &inside);
        assert!(same_resolution(
            grid_result,
            resolve_attack(&world, &inside, &control)
        ));
        assert!(
            matches!(
                grid_result,
                HitResolution::Hit {
                    target: 9_001,
                    distance: 0.0,
                    ..
                }
            ),
            "the trace starts inside the huge box: entry t = 0"
        );
        // A trace into empty space far away still hits the huge box
        // (it covers everything) — the oversized list is consulted
        // by EVERY query.
        let far = AttackRequest {
            attacker: 0,
            origin: [1_000_000.0, 1.6, 1_000_000.0],
            direction: [0.0, 0.0, -1.0],
            profile: AttackProfile {
                range: 30.0,
                occlusion_margin: 0.0,
            },
        };
        assert!(same_resolution(
            huge_index.resolve_attack(&world, &far),
            resolve_attack(&world, &far, &control)
        ));
    }

    /// The B06 scaling evidence — the design's §9.2 measurement
    /// shape, observational: battle populations at multiple sizes,
    /// each probe through the grid path vs the linear primitive,
    /// with least-squares slopes (ns per 1k combatants), the
    /// one-time build cost, and the grid's resident shape (cells /
    /// entries). Printed, not gated: generous sanity bounds only
    /// (the F-06 no-thresholds-before-variance rule, and the
    /// collision.rs Q512 characterization precedent). CAVEAT,
    /// stated honestly: this runs under the UNOPTIMIZED test
    /// profile — the slopes and the grid/linear RATIO are the
    /// evidence here; absolute constants belong to the
    /// release-mode rig matrix (§9, the measurement owner). Every
    /// probe is also bit-compared against the linear path inline
    /// (the battery's claim, measured on the very populations being
    /// timed).
    #[test]
    fn combat_index_scaling_evidence() {
        // The battle shape: human-scale bodies (the adversary_body
        // extent class) on a 3 m lattice, roughly square extents.
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

        fn mean_ns(queries: usize, mut run: impl FnMut()) -> f64 {
            let start = std::time::Instant::now();
            for _ in 0..queries {
                run();
            }
            start.elapsed().as_secs_f64() / queries as f64 * 1e9
        }

        // Least-squares slope, ns per 1k combatants, over the
        // measured (population, ns) pairs.
        let slope = |points: &[(usize, f64)]| -> f64 {
            let n = points.len() as f64;
            let sum_x: f64 = points.iter().map(|(x, _)| *x as f64).sum();
            let sum_y: f64 = points.iter().map(|(_, y)| *y).sum();
            let sum_xy: f64 = points.iter().map(|(x, y)| *x as f64 * y).sum();
            let sum_xx: f64 = points.iter().map(|(x, _)| (*x as f64) * (*x as f64)).sum();
            (n * sum_xy - sum_x * sum_y) / (n * sum_xx - sum_x * sum_x) * 1000.0
        };

        // The probes: a down-street shot (the typical fire), the
        // long cross-field diagonal (the honest waste row — which
        // regime the GRID_CELL_WASTE_FACTOR guard picks depends on
        // the population), and an upward miss.
        let world = CollisionWorld::new(0.0);
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

        let sizes = [128usize, 512, 2048, 8192, 32768];
        let mut grid_series: Vec<Vec<(usize, f64)>> = vec![Vec::new(); 3];
        let mut linear_series: Vec<Vec<(usize, f64)>> = vec![Vec::new(); 3];
        for size in sizes {
            let queries = ((600_000 / size as u64).clamp(100, 5000)) as usize;
            let build_start = std::time::Instant::now();
            let roster = battle(size);
            let roster_build_ms = build_start.elapsed().as_secs_f64() * 1e3;
            let grid_start = std::time::Instant::now();
            let index = CombatIndex::build(&roster);
            let index_build_ms = grid_start.elapsed().as_secs_f64() * 1e3;
            assert!(
                matches!(index.phase, CombatPhase::Grid(_)),
                "the battle at {size} activates the grid"
            );

            // Inline bit-identity on the probe set (the battery's
            // claim, restated on the timed populations).
            for request in [&shot, &diagonal, &miss] {
                assert!(
                    same_resolution(
                        index.resolve_attack(&world, request),
                        resolve_attack(&world, request, &roster)
                    ),
                    "pop {size}: the timed probe is bit-identical"
                );
            }

            let t = |resolve: &dyn Fn(&AttackRequest) -> HitResolution| -> [f64; 3] {
                [
                    mean_ns(queries, || {
                        resolve(&shot);
                    }),
                    mean_ns(queries, || {
                        resolve(&diagonal);
                    }),
                    mean_ns(queries, || {
                        resolve(&miss);
                    }),
                ]
            };
            let grid_times = t(&|request| index.resolve_attack(&world, request));
            let linear_times = t(&|request| resolve_attack(&world, request, &roster));
            if let CombatPhase::Grid(grid) = &index.phase {
                let entries: usize = grid.cells.values().map(Vec::len).sum();
                println!(
                    "[combat-index-scaling] pop {size:6} · roster build {roster_build_ms:8.3} ms \
                     · index build {index_build_ms:8.3} ms · cells {} · entries {entries} · \
                     oversized {} · queries {queries}",
                    grid.cells.len(),
                    grid.oversized.len()
                );
            }
            println!(
                "[combat-index-scaling] pop {size:6} · ns/query grid vs linear — \
                 shot {:9.1}/{:9.1} · diagonal {:9.1}/{:9.1} · miss {:9.1}/{:9.1}",
                grid_times[0],
                linear_times[0],
                grid_times[1],
                linear_times[1],
                grid_times[2],
                linear_times[2]
            );
            for (arm, value) in grid_times.iter().enumerate() {
                grid_series[arm].push((size, *value));
            }
            for (arm, value) in linear_times.iter().enumerate() {
                linear_series[arm].push((size, *value));
            }
            // Sanity bounds only (characterization, not a gate). The
            // per-record form (the 877590f/3e0c729 precedent): the
            // diagonal grid arm walks cells proportionally to the
            // population, so a fixed bound trips on its own scaling
            // under a slow/shared runner (the CI measured 51.3 ms
            // at 32,768 against the old fixed 50 ms — the local rig
            // reads ~13.7 ms). The 4 µs/record form (a2e6e86, ~2.6x
            // headroom over its era's actuals) has since been
            // exceeded TWICE at the 8,192 arm by shared-runner
            // variance: 39.5 ms (run 46139366087) and 38.2 ms
            // (run 46142180934) against its 37.8 ms bound — the
            // second-occurrence protocol firing the measured-
            // actuals correction. 5 ms + 6 µs/record gives ~1.37x
            // headroom over the worst accumulated actual while a
            // quadratic blowup (the diagonal reading quadrupling
            // per population doubling: ~150 ms at 8,192, seconds at
            // 32k) still fails by 3x and more.
            for ns in grid_times.iter().chain(linear_times.iter()) {
                assert!(
                    ns.is_finite() && *ns < 5_000_000.0 + size as f64 * 6_000.0,
                    "pop {size}: a probe is absurdly slow: {ns} ns"
                );
            }
        }

        // The fitted slopes (ns per 1k combatants), grid vs linear
        // per probe — the delivery report's numbers.
        let arms = ["shot", "diagonal", "miss"];
        for (arm, grid_points) in arms.iter().zip(&grid_series) {
            let linear_points = &linear_series[arms.iter().position(|a| a == arm).unwrap()];
            println!(
                "[combat-index-scaling] slope {arm:8}: grid {:9.1} vs linear {:9.1} \
                 ns per 1k combatants",
                slope(grid_points),
                slope(linear_points)
            );
        }
    }
}
