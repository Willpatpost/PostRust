//! The NPC locomotion core (Era 2 — the living world's legs, per
//! `docs/ERA2-NPC-AI-DESIGN-PASS.md` §4.2's Full-tier row and §4.3):
//! routes as pure data, the per-NPC reconstructible locomotion state
//! (the position bridge), the identity-ordered near-lane batch step,
//! the speed-band clip selection, the committed-cell quantizer, and
//! the locomotion-stream digest — ALL pure, fully tested, no I/O, no
//! clocks, no Vulkan, no new dependencies.
//!
//! # DETERMINISM CONTRACT
//! Every answer this module produces is byte-identical on every
//! platform: the per-tick arithmetic is fixed-order IEEE f32
//! (mul/add/sub/div only — every one correctly rounded), the heading
//! math is INTEGER u16 turn units with `rem_euclid` (no floats, no
//! transcendental anywhere at tick time), leg lengths/units/perps are
//! precomputed at route construction (the only `sqrt` in the module —
//! IEEE-correctly-rounded, and it runs at authoring, never per tick),
//! per-leg heading TURNS are authored data (the placement table's
//! pattern extended to paths: no runtime atan2 exists to vary), and
//! the digest is FNV-1a over little-endian fixed-width fields. A
//! fixed route table, fixed seeds, and a fixed tick delta produce the
//! same locomotion stream forever — pinned by digest.
//!
//! # THE FINITE-ARITHMETIC CONTRACT (every loop terminates)
//! No input admitted by `Route::new` can stall the tick. The stride
//! phase's wrap into [0, 1) is ONE `floor` and ONE subtraction —
//! never a repeated `-= 1.0`, which does not terminate once the
//! phase reaches 2^24 (the f32 spacing there is 2.0 or coarser, so
//! subtracting 1.0 rounds back to the same float; a valid authored
//! stride of 2^-30 against a distance of 1.0 puts the phase at 2^30).
//! The reduction is exact everywhere exactness is possible and
//! byte-identical to the repeated subtraction it replaces (the
//! arithmetic proof lives at step 9 below); a non-finite phase
//! (distance/stride overflowing f32 on valid authored inputs) is
//! clamped to 0.0 and counted fail-visibly. The crossing loop is
//! bounded by [`MAX_CROSSINGS_PER_TICK`], and distance past the
//! bound is DROPPED with the state settled at the last admitted
//! crossing — never carried for extrapolation off the leg.
//!
//! # THE ENGINE/POLICY BOUNDARY (the design's rule, §4.1/§4.3)
//! This module ships the locomotion MACHINERY: the route/leg data
//! model, the batch step, the turn blend, the lateral spread, the
//! collision composition, the clip band, the cell quantizer, and the
//! digest pinning. The route TABLES — which waypoints exist, what
//! speeds and widths they carry, what their policy tags mean, the
//! walk-speed threshold — are policy data authored above the engine.
//! No cyberpunk-specific behavior lives here.
//!
//! # THE BATCH LANE (§4.3 — movement is NOT a per-NPC event storm)
//! `advance_batch` is the Full tier's per-tick locomotion integration
//! (§4.2's Full row: "every near tick, inside the guaranteed near
//! reservation"): the CALLER invokes it once per fixed tick over the
//! present (Full-tier) population in identity order. It allocates
//! nothing, touches no spine state, and skips absent NPCs
//! byte-identically — the far tiers' trajectories are commitments
//! (the calendar's), never physics. The engine/primitive boundary:
//! the spine fires the commitment wakes (npc_execution), the
//! selector picks the activity, POLICY maps the activity to a route
//! and destination (`start_route`/`set_destination`), and THIS module
//! moves the NPC every tick until the destination arrives.
//!
//! # THE POSITION BRIDGE (§4.3's committed-location rule)
//! The locomotion state carries CONTINUOUS f32 positions in fixed
//! operation order (the near tier's reconstructible precision), while
//! the COMMITTED location stays coarse: `leg` names the last waypoint
//! departed (the caller mirrors it into `NpcRoutine::route_progress`
//! at arrival), and `committed_cell` quantizes the continuous
//! position onto the (u8, u8) region grid — the render proxies'
//! existing cell→world mapping's inverse, the far-tier fallback.
//! Far/dormant state never pays continuous precision.
//!
//! # STATE INVARIANTS (every legs[leg] read is guarded)
//! `leg` is the leg in progress == the last waypoint departed; when
//! `speed == 0.0` the NPC stands AT waypoint `leg` (the same index
//! names the next leg to depart — legs[leg] departs waypoint leg).
//! `leg == leg_count` occurs only standing at a non-looping route's
//! end waypoint (set there by arrival; every `legs[leg]` access in
//! the batch is guarded by `speed > 0.0`, which arrival zeroes, and
//! `set_destination` from that state either stands (dest == leg) or
//! refuses (dest < leg is backward on a non-loop)). On a looping
//! route `leg` is always < leg_count (the wrap re-zeros it; loop
//! destinations must name a waypoint < leg_count because waypoint
//! leg_count IS waypoint 0 — the closure).

use crate::npc_behavior::{NpcId, NpcRng};
use crate::player::CollisionWorld;

/// The no-destination sentinel: walk the route forever (the sidewalk
/// stroll). Any other value is a waypoint index to stop at. Refused
/// on non-looping routes at `start_route`/`set_destination` (an open
/// route with no destination would run off its last waypoint — an
/// authoring contradiction, not a runtime case).
pub const LOOPING: u32 = u32::MAX;
/// The maximum waypoint crossings one batch tick may consume (the
/// pathological guard: a huge dt against tiny legs). Distance past
/// this bound's crossings is dropped, deterministically — the state
/// SETTLES at the last admitted crossing (the NPC stands at the
/// waypoint it reached: `leg_distance` 0 on the leg it entered, an
/// on-route position), and the next tick resumes the walk. The
/// `guard_settles` counter in [`BatchSummary`] reports the
/// settlement fail-visibly; ordinary authored trajectories never
/// reach the bound.
pub const MAX_CROSSINGS_PER_TICK: u32 = 16;
/// The lateral-fade window: the fraction of each leg's start/end over
/// which the lateral offset eases to zero — the no-pop-at-corners
/// rule (an offset that held through a corner would teleport the NPC
/// sideways when the perpendicular flips). Fixed at 0.15.
pub const LATERAL_FADE_WINDOW: f32 = 0.15;

/// One authored leg: the destination waypoint plus the heading turn
/// held while traveling it. Turns are AUTHORED u16 turn units
/// (1/65536 of a full rotation — the `proxy_instances` scale), the
/// placement table's hand-authored-turn pattern extended to paths:
/// deriving a heading from a direction vector would need atan2, and
/// no runtime transcendental may touch a byte-packed path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteLeg {
    /// The leg's destination, world-space (x, z).
    pub to: [f32; 2],
    /// The heading held while traveling this leg (u16 turn units).
    pub turn: u16,
}

/// Per-leg geometry precomputed at construction (one allocation at
/// authoring; the batch step allocates nothing and does only
/// mul/add/sub/div against these fields).
#[derive(Debug, Clone, Copy)]
struct RouteLegGeom {
    /// Leg i's start (origin for i == 0, legs[i-1].to otherwise).
    from: [f32; 2],
    /// The leg's length: `(dx·dx + dz·dz).sqrt()` — computed HERE,
    /// at authoring; IEEE-correctly-rounded, so byte-identical on
    /// every platform, and never on the per-tick path.
    length: f32,
    /// `1.0 / length` (precomputed; the per-tick path multiplies).
    inv_length: f32,
    /// Unit direction: `[(to−from)·inv_length]` per component.
    unit: [f32; 2],
    /// Right-hand perpendicular `[unit[1], −unit[0]]` — the pinned
    /// convention the lateral offset applies along.
    perp: [f32; 2],
}

/// A route: pure data (§4.3 — "waypoint sequences with authored
/// widths and hand-spread variability seeds"). Waypoint 0 is
/// `origin`; leg i travels FROM waypoint i TO waypoint i+1
/// (`legs[i].to`). A looping route must close exactly
/// (`legs.last().to == origin`, componentwise) — authored corner
/// routes make this natural; the engine refuses an unclosed loop
/// rather than inventing a closing leg.
#[derive(Debug, Clone)]
pub struct Route {
    origin: [f32; 2],
    legs: Vec<RouteLeg>,
    per_leg: Vec<RouteLegGeom>,
    speed: f32,
    spread_width: f32,
    stride_length: f32,
    turn_rate: u16,
    loops: bool,
    policy_tag: u32,
}

impl Route {
    /// Fail-closed construction (`None`, never a panic), exactly
    /// when: `legs` is empty; any authored f32 (origin, leg `to`,
    /// speed, spread_width, stride_length) is not finite;
    /// `speed <= 0.0`; `stride_length <= 0.0`; `spread_width < 0.0`;
    /// any leg has zero length (`to == from` — a zero-length leg
    /// would divide by zero and mean nothing); or `loops` is set and
    /// the route does not close exactly on its origin.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        origin: [f32; 2],
        legs: Vec<RouteLeg>,
        speed: f32,
        spread_width: f32,
        stride_length: f32,
        turn_rate: u16,
        loops: bool,
        policy_tag: u32,
    ) -> Option<Route> {
        if legs.is_empty() {
            return None;
        }
        if !origin[0].is_finite()
            || !origin[1].is_finite()
            || !legs
                .iter()
                .all(|l| l.to[0].is_finite() && l.to[1].is_finite())
            || !speed.is_finite()
            || !spread_width.is_finite()
            || !stride_length.is_finite()
        {
            return None;
        }
        if speed <= 0.0 || stride_length <= 0.0 || spread_width < 0.0 {
            return None;
        }
        // The loop closure: exact componentwise equality — an
        // authored loop closes on its origin or it is not a loop.
        if loops && (legs[legs.len() - 1].to != origin) {
            return None;
        }

        // Per-leg geometry, chained from the origin. The zero-length
        // refusal lives HERE (against each leg's own `from`), where
        // the length is actually computed — never against the route
        // origin (a closed loop's last leg legitimately returns to
        // it).
        let mut per_leg = Vec::with_capacity(legs.len());
        let mut from = origin;
        for leg in &legs {
            let dx = leg.to[0] - from[0];
            let dz = leg.to[1] - from[1];
            let length = (dx * dx + dz * dz).sqrt();
            if length == 0.0 || !length.is_finite() {
                return None;
            }
            let inv_length = 1.0 / length;
            let unit = [dx * inv_length, dz * inv_length];
            let perp = [unit[1], -unit[0]];
            per_leg.push(RouteLegGeom {
                from,
                length,
                inv_length,
                unit,
                perp,
            });
            from = leg.to;
        }

        Some(Route {
            origin,
            legs,
            per_leg,
            speed,
            spread_width,
            stride_length,
            turn_rate,
            loops,
            policy_tag,
        })
    }

    /// The number of legs (== the number of waypoints minus one).
    pub fn leg_count(&self) -> usize {
        self.legs.len()
    }

    /// Leg i's start waypoint (the origin for i == 0).
    pub fn leg_from(&self, i: usize) -> [f32; 2] {
        self.per_leg[i].from
    }

    /// Leg i's destination waypoint.
    pub fn leg_to(&self, i: usize) -> [f32; 2] {
        self.legs[i].to
    }

    /// Leg i's authored length (the precomputed geometry).
    pub fn leg_length(&self, i: usize) -> f32 {
        self.per_leg[i].length
    }

    /// Leg i's authored heading turn.
    pub fn leg_turn(&self, i: usize) -> u16 {
        self.legs[i].turn
    }

    /// The route's canonical speed (units/s).
    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// The lateral spread width (units; offsets reach ± this).
    pub fn spread_width(&self) -> f32 {
        self.spread_width
    }

    /// The distance per full stride cycle (the phase denominator).
    pub fn stride_length(&self) -> f32 {
        self.stride_length
    }

    /// The heading blend rate (u16 turn units per tick).
    pub fn turn_rate(&self) -> u16 {
        self.turn_rate
    }

    /// Whether arrival at the last waypoint wraps to leg 0.
    pub fn loops(&self) -> bool {
        self.loops
    }

    /// The policy tag (crossing semantics etc.) — engine-opaque.
    pub fn policy_tag(&self) -> u32 {
        self.policy_tag
    }
}

/// The per-NPC reconstructible locomotion state (§4.3's position
/// bridge). Copy, like `NpcRoutine`; captured before discard on
/// deactivation, derived on activation, NEVER persisted as durable
/// truth (the routine lane's discipline).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NpcLocomotion {
    /// Continuous world-space position (x, z), fixed operation order.
    pub position: [f32; 2],
    /// Heading in u16 turn units (the sin_cos_turn scale).
    pub facing: u16,
    /// The route table index this state walks.
    pub route: u32,
    /// The leg in progress == the last waypoint departed. When
    /// `speed == 0.0` the NPC stands AT waypoint `leg` (the module
    /// invariants: the same index names the next leg to depart).
    pub leg: u32,
    /// Distance traveled along the current leg (units; in
    /// [0, leg_length) while walking, 0.0 when standing).
    pub leg_distance: f32,
    /// The destination waypoint index, or LOOPING.
    pub destination: u32,
    /// The current speed (units/s). Only `start_route`/
    /// `set_destination` raise it; a destination arrival drops it to
    /// 0.0; a blocked tick deliberately leaves it (the pinned-blocked
    /// shape resumes the walk the instant the path clears).
    pub speed: f32,
    /// Per-NPC speed scale in [0.9, 1.1) — the hand-spread draw (the
    /// crowd does not march in lockstep), fixed at `start_route`.
    pub speed_scale: f32,
    /// Per-NPC lateral offset fraction in [-1, 1] (times the route's
    /// spread_width, along each leg's perpendicular — the crowd does
    /// not file in lockstep), fixed at `start_route`.
    pub lateral: f32,
    /// The walk-cycle stride phase in [0, 1) — the (clip, phase)
    /// instance contract's phase half, advanced by DISTANCE (feet
    /// never slide: it freezes when blocked or standing).
    pub stride_phase: f32,
}

/// Derives the state at activation/authoring: stands the NPC at
/// `at_waypoint` on `route_index`, facing the departing leg's
/// authored turn, with the three hand-spread draws — speed_scale
/// (`0.9 + 0.2·d1`), lateral (`2.0·d2 − 1.0`), stride_phase (`d3`) —
/// taken in THIS fixed order from the appearance seed's OWN stream
/// (`NpcRng::for_npc(NpcId(appearance_seed))`, npc_behavior's
/// SplitMix64 family). The seed's own stream, never the NPC's
/// selector stream: locomotion reconstruction must not depend on
/// selector replay order (§8.3's reconstructibility rule).
///
/// Fail-closed `None`: `at_waypoint > leg_count`; `at_waypoint ==
/// leg_count && !loops` (standing at an open route's end has no
/// departing leg); `destination > leg_count`; `!loops &&
/// destination < at_waypoint` (unreachable backward); `destination
/// == LOOPING && !loops` (an open route with no destination runs off
/// its end); `destination == leg_count && loops` (on a loop, waypoint
/// leg_count IS waypoint 0 — the closure — and naming it is a
/// confusion the engine refuses; author 0). VALID and notable:
/// `destination == at_waypoint` (the NPC simply stands — speed 0.0).
#[allow(clippy::too_many_arguments)]
pub fn start_route(
    route_index: u32,
    route: &Route,
    appearance_seed: u32,
    at_waypoint: u32,
    destination: u32,
) -> Option<NpcLocomotion> {
    let leg_count = route.leg_count() as u32;
    if at_waypoint > leg_count {
        return None;
    }
    if at_waypoint == leg_count && !route.loops() {
        return None;
    }
    // The LOOPING sentinel is exempt from the range check (it is not
    // a waypoint index) and is only valid on a looping route.
    if destination != LOOPING && destination > leg_count {
        return None;
    }
    if destination == LOOPING && !route.loops() {
        return None;
    }
    if destination == leg_count && route.loops() {
        return None;
    }
    if !route.loops() && destination < at_waypoint {
        return None;
    }

    // The departing leg: at_waypoint names it directly; the loop's
    // end waypoint wraps to leg 0 (the closure makes waypoint
    // leg_count and waypoint 0 the same place).
    let departing_leg = if at_waypoint == leg_count {
        0
    } else {
        at_waypoint
    };

    // The standing position: waypoint w's authored coordinates —
    // the origin for w == 0, legs[w-1].to otherwise (and the origin
    // again at the loop closure). Authored BYTES, no arithmetic.
    let position = if at_waypoint == 0 || at_waypoint == leg_count {
        route.origin
    } else {
        route.leg_to((at_waypoint - 1) as usize)
    };

    // The three hand-spread draws, fixed order, from the seed's own
    // stream (the reconstruction proof re-derives them exactly so).
    let mut rng = NpcRng::for_npc(NpcId(appearance_seed));
    let speed_scale = 0.9 + 0.2 * rng.next_unit();
    let lateral = 2.0 * rng.next_unit() - 1.0;
    let stride_phase = rng.next_unit();

    Some(NpcLocomotion {
        position,
        facing: route.leg_turn(departing_leg as usize),
        route: route_index,
        leg: departing_leg,
        leg_distance: 0.0,
        destination,
        speed: if destination == at_waypoint {
            0.0
        } else {
            route.speed()
        },
        speed_scale,
        lateral,
        stride_phase,
    })
}

/// Retargets a walking or standing NPC to a new destination on its
/// CURRENT route (the policy seam a commitment's activity drives).
/// False (fail-closed, the state byte-identical — the refusals
/// mirror `start_route`'s): `destination > leg_count`; `!loops &&
/// destination < leg` (unreachable backward); `destination == LOOPING
/// && !loops`; `destination == leg_count && loops`. True otherwise:
/// `destination == leg` stands the NPC in place (speed 0.0 — the
/// already-there case); any other destination (re)starts the walk at
/// the route's canonical speed, continuing from the current
/// leg_distance — no redraw, no phase reset.
pub fn set_destination(route: &Route, state: &mut NpcLocomotion, destination: u32) -> bool {
    let leg_count = route.leg_count() as u32;
    // The LOOPING sentinel is exempt from the range check (it is not
    // a waypoint index); the range and authoring clauses follow.
    if destination != LOOPING && destination > leg_count {
        return false;
    }
    if destination == LOOPING && !route.loops() {
        return false;
    }
    if destination == leg_count && route.loops() {
        return false;
    }
    if !route.loops() && destination < state.leg {
        return false;
    }

    state.destination = destination;
    state.speed = if destination == state.leg {
        0.0
    } else {
        route.speed()
    };
    true
}

/// The speed band → clip selection (§4.3: "walk-vs-idle clip
/// selection is derived from the locomotion state — speed band →
/// clip id"). Stand strictly below the threshold, Walk at or above
/// it. The threshold is POLICY (the game's walk-speed constant).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocomotionClip {
    /// No locomotion — the idle baseline.
    Stand,
    /// Striding — the walk cycle's (clip, phase) contract.
    Walk,
}

/// Selects the clip for a speed under a policy threshold.
pub fn clip_for(speed: f32, walk_threshold: f32) -> LocomotionClip {
    if speed < walk_threshold {
        LocomotionClip::Stand
    } else {
        LocomotionClip::Walk
    }
}

/// Continuous world (x, z) → the coarse committed cell (§4.3's
/// committed-location bridge): the (u8, u8) region-grid cell the
/// render proxies' cell→world mapping inverts. Fixed operation
/// order per axis: subtract the grid origin, divide by the spacing,
/// floor, cast to i32 (Rust's saturating float→int cast —
/// deterministic), clamp to [0, side−1], cast to u8. The grid's
/// semantics (origin, spacing, side) are the caller's — the region
/// plate geometry; `spacing > 0.0` is the caller's contract.
pub fn committed_cell(
    position: [f32; 2],
    grid_origin: [f32; 2],
    spacing: f32,
    side: u8,
) -> (u8, u8) {
    let clamp_axis = |value: f32, origin: f32| {
        let cell = ((value - origin) / spacing).floor() as i32;
        cell.clamp(0, i32::from(side) - 1) as u8
    };
    (
        clamp_axis(position[0], grid_origin[0]),
        clamp_axis(position[1], grid_origin[1]),
    )
}

/// One NPC's per-tick batch outcome (diagnostics, never authority —
/// the state itself is the truth).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StepOutcome {
    /// The DESTINATION waypoint reached this tick, if any (a
    /// crossing that continues walking is not an arrival — the
    /// caller turns a destination arrival into its next commitment,
    /// §4.2's commitment points).
    pub arrived: Option<u32>,
    /// A collision depenetration engaged this tick (the
    /// pinned-blocked shape: the position is idempotent across
    /// blocked ticks, the along-leg distance reverts, the stride
    /// phase freezes — no foot slide).
    pub blocked: bool,
}

/// The batch summary: counters over the tick's present NPCs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BatchSummary {
    /// NPCs that attempted movement this tick (present, valid route,
    /// speed > 0) — blocked ticks still count (the attempt is the
    /// work); standing and absent NPCs do not.
    pub stepped: u32,
    /// Destination arrivals this tick.
    pub arrivals: u32,
    /// Collision-blocked ticks this tick.
    pub blocked: u32,
    /// Crossing-guard settlements this tick: a pathological tick
    /// whose distance crossed more than [`MAX_CROSSINGS_PER_TICK`]
    /// legs had its excess distance dropped and its state settled
    /// at the last admitted crossing. Fail-visible diagnostics —
    /// ordinary authored trajectories never fire it.
    pub guard_settles: u32,
    /// Stride-phase clamps this tick: the distance/stride increment
    /// overflowed f32 (valid authored inputs can do it — a giant
    /// speed over a tiny stride), so the phase was clamped to 0.0.
    /// Fail-visible diagnostics — ordinary authored trajectories
    /// never fire it.
    pub phase_clamps: u32,
}

/// THE BATCH STEP (§4.3's data-oriented lane): advances every PRESENT
/// NPC in identity (index) order. Allocates NOTHING. `states` and
/// `present` are parallel slices indexed by population index; a
/// false mask entry leaves that state byte-identical (the far tiers'
/// trajectories are commitments, not physics). A state whose route
/// index is foreign to `routes` is skipped fail-visibly (counters
/// unchanged, never a panic — the caller authored a bad table).
///
/// `collision` is the near-tier kinematic query — the player's
/// `CollisionWorld` contract (NPCs are kinematic movers against the
/// same world the player collides with, near tier only, §4.3).
/// `npc_half` is the NPC box's horizontal half-extent; `npc_height`
/// its full height; the box sits on the world's ground plane.
///
/// # THE PER-NPC FIXED ORDER (pinned line by line)
/// 1. `distance = speed · speed_scale · dt` (left-to-right);
/// 2. the blocked-revert anchor: the (leg, leg_distance) PAIR — a
///    crossing may advance the leg within the tick, and the honest
///    revert must restore both or the pre-step distance lands on the
///    WRONG leg's unit vector;
/// 3. `leg_distance += distance`;
/// 4. the crossing loop (≤ MAX_CROSSINGS_PER_TICK): while
///    `leg_distance >= leg_length`, subtract the length and advance
///    `leg` (wrapping on loops; a destination arrival snaps position
///    to the AUTHORED waypoint bytes, zeroes leg_distance, drops
///    speed to 0, and reports `arrived`). If the bound exhausted
///    with distance still at or beyond the leg's end, the excess is
///    dropped and the state SETTLES at the last admitted crossing
///    (leg_distance 0 — the waypoint reached, on-route; counted in
///    `guard_settles`);
/// 5. `position = leg_from + leg_distance · unit` (distance times
///    the unit vector — exact for binary fixtures);
/// 6. the lateral offset: `lateral · spread_width · fade(t)` along
///    the leg's perpendicular, `fade` the smoothstep window over
///    `min(t, 1−t)/LATERAL_FADE_WINDOW`;
/// 7. collision: `depenetration` over the NPC box at the candidate;
///    a push moves x/z only (ground-constrained kinematics), reverts
///    the (leg, leg_distance) pair to the pre-step values (honest
///    no-progress), and freezes the phase;
/// 8. the facing blend toward the leg's authored turn (integer u16,
///    shortest way, `turn_rate` per tick) — standing NPCs never
///    turn, blocked NPCs still do;
/// 9. the stride phase: `+= distance / stride_length`, wrapped into
///    [0, 1) by ONE floor and ONE subtraction (the finite-
///    arithmetic contract in the module header) — frozen when
///    blocked or standing (no foot slide).
#[allow(clippy::too_many_arguments)]
pub fn advance_batch(
    routes: &[Route],
    states: &mut [NpcLocomotion],
    present: &[bool],
    dt: f32,
    collision: Option<&CollisionWorld>,
    npc_half: f32,
    npc_height: f32,
) -> BatchSummary {
    debug_assert_eq!(
        states.len(),
        present.len(),
        "states and present are parallel population slices"
    );

    let mut summary = BatchSummary::default();

    for idx in 0..states.len() {
        if !present[idx] {
            continue;
        }
        let state = &mut states[idx];
        let route = match routes.get(state.route as usize) {
            Some(route) => route,
            // A foreign route index is the caller's authoring error:
            // skipped fail-visibly, never a panic.
            None => continue,
        };

        // Standing NPCs are stable: no movement, no turn, no phase.
        if state.speed <= 0.0 {
            continue;
        }
        summary.stepped += 1;

        // 1. The per-tick distance, fixed order.
        let distance = state.speed * state.speed_scale * dt;

        // 2. The blocked-revert anchor: the (leg, distance) PAIR. A
        //    crossing may advance `leg` within this tick; the honest
        //    revert must restore BOTH, or the pre-step distance
        //    lands on the WRONG leg's unit vector — a walker
        //    teleported off its route by an obstruction immediately
        //    past a corner.
        let before_leg = state.leg;
        let before = state.leg_distance;

        // 3. Advance along the leg.
        state.leg_distance += distance;

        // 4. The crossing loop. `arrived` fires ONLY on a destination
        // arrival (a stop); a crossing that continues walking is not
        // an arrival.
        let mut arrived: Option<u32> = None;
        for _ in 0..MAX_CROSSINGS_PER_TICK {
            let geom = route.per_leg[state.leg as usize];
            if state.leg_distance < geom.length {
                break;
            }
            state.leg_distance -= geom.length;
            let reached = state.leg + 1;

            if reached == route.leg_count() as u32 {
                // The route's end waypoint.
                if state.destination == reached {
                    // The end-waypoint arrival (an open route walked
                    // to its last waypoint): leg becomes leg_count —
                    // the standing-at-end state the module invariants
                    // document; every legs[leg] read stays guarded.
                    state.leg = reached;
                    state.leg_distance = 0.0;
                    state.position = route.leg_to(route.leg_count() - 1);
                    state.speed = 0.0;
                    arrived = Some(reached);
                    break;
                }
                if route.loops() {
                    // The wrap: waypoint leg_count IS waypoint 0.
                    state.leg = 0;
                } else {
                    // LOOPING on an open route is refused at every
                    // entry point; reaching here means corrupted
                    // state — stop, fail-visible, never a panic.
                    state.leg = reached;
                    state.leg_distance = 0.0;
                    state.position = route.leg_to(route.leg_count() - 1);
                    state.speed = 0.0;
                    state.destination = reached;
                    arrived = Some(reached);
                    break;
                }
            } else {
                state.leg = reached;
            }

            if state.destination != LOOPING && state.leg == state.destination {
                // A mid-route destination arrival: snap to the
                // AUTHORED waypoint bytes (waypoint 0 is the origin),
                // zero the along-leg distance, stand.
                state.leg_distance = 0.0;
                state.position = if state.leg == 0 {
                    route.origin
                } else {
                    route.leg_to((state.leg - 1) as usize)
                };
                state.speed = 0.0;
                arrived = Some(state.destination);
                break;
            }
        }

        // 4b. THE CROSSING-GUARD SETTLEMENT. If the
        // MAX_CROSSINGS_PER_TICK bound exhausted with distance still
        // at or beyond the current leg's end, the excess is DROPPED
        // and the state settles at the last admitted crossing: the
        // NPC stands at the waypoint it reached (leg_distance 0 on
        // the leg it entered — an on-route position), and the next
        // tick resumes the walk. The pre-corrective defect kept the
        // unconsumed distance in the state, so step 5 extrapolated
        // `leg_from + leg_distance · unit` past the leg's end by
        // that whole distance — a walker off its route with the
        // `leg_distance < leg_length` invariant broken. Ordinary
        // trajectories never fire this (the loop breaks with
        // `leg_distance < length`); the guard's counter reports it
        // fail-visibly. The `speed > 0` guard is the same one step 5
        // carries: every arrival path (and the corrupted-state
        // clause above) has already zeroed the speed and may have
        // parked `leg` at leg_count, where per_leg has no entry.
        if state.speed > 0.0 {
            let geom = route.per_leg[state.leg as usize];
            if state.leg_distance >= geom.length {
                state.leg_distance = 0.0;
                summary.guard_settles += 1;
            }
        }

        // 5-9. Position, lateral, collision, facing, phase — all
        // skipped when the arrival snap already stood the NPC.
        let mut blocked = false;
        if state.speed > 0.0 {
            let geom = route.per_leg[state.leg as usize];

            // 5. Distance along the unit vector (NOT t·unit — the
            // fraction times the unit would shrink the step by the
            // leg's length; the distance itself is the multiplier).
            let base = [
                geom.from[0] + state.leg_distance * geom.unit[0],
                geom.from[1] + state.leg_distance * geom.unit[1],
            ];

            // 6. The lateral spread: the offset eases to zero over
            // each leg's start/end window (the no-pop-at-corners
            // rule) — smoothstep, pure f32, fixed order.
            let t = state.leg_distance * geom.inv_length;
            let nearest_edge = if t < 1.0 - t { t } else { 1.0 - t };
            let edge = (nearest_edge / LATERAL_FADE_WINDOW).clamp(0.0, 1.0);
            let fade = edge * edge * (3.0 - 2.0 * edge);
            let offset = state.lateral * route.spread_width() * fade;
            let candidate = [
                base[0] + offset * geom.perp[0],
                base[1] + offset * geom.perp[1],
            ];

            // 7. The near-tier kinematic query (the player's
            // contract). The push's y is DISCARDED — the NPC is
            // ground-constrained; only x/z move it.
            let final_position = match collision {
                Some(world) => {
                    let ground_y = world.ground_y();
                    let min = [candidate[0] - npc_half, ground_y, candidate[1] - npc_half];
                    let max = [
                        candidate[0] + npc_half,
                        ground_y + npc_height,
                        candidate[1] + npc_half,
                    ];
                    match world.depenetration(min, max) {
                        Some(push) => {
                            blocked = true;
                            // The honest no-progress revert: the
                            // (leg, distance) PAIR — the pre-tick leg
                            // AND the pre-tick distance — so the next
                            // blocked tick recomputes the identical
                            // crossing, candidate, and push
                            // (pinned-blocked idempotence), and the
                            // state never interprets the pre-step
                            // distance on a leg it does not belong
                            // to (a corner crossing must not strand
                            // the old distance on the new leg).
                            state.leg = before_leg;
                            state.leg_distance = before;
                            [candidate[0] + push[0], candidate[1] + push[2]]
                        }
                        None => candidate,
                    }
                }
                None => candidate,
            };
            state.position = final_position;

            // 8. The facing blend — integer u16 units, the shortest
            // way around the circle (rem_euclid: Rust's % truncates
            // toward zero and would misdirect negative deltas).
            // Standing NPCs never reach here (speed > 0 guard);
            // blocked NPCs deliberately still turn (a walker pinned
            // against a wall keeps facing its heading).
            let target = route.leg_turn(state.leg as usize);
            let delta =
                ((i32::from(target) - i32::from(state.facing) + 32768).rem_euclid(65536)) - 32768;
            let rate = i32::from(route.turn_rate());
            let step = delta.clamp(-rate, rate);
            state.facing = ((i32::from(state.facing) + step).rem_euclid(65536)) as u16;

            // 9. The stride phase from DISTANCE, frozen when blocked
            // (the no-foot-slide rule: feet do not cycle against a
            // wall). THE FINITE-ARITHMETIC CONTRACT: the wrap into
            // [0, 1) is ONE floor and ONE subtraction — never a
            // repeated `-= 1.0`, which does not terminate once the
            // phase reaches 2^24 (the f32 spacing at 2^24 is 2.0, so
            // subtracting 1.0 rounds back to the same float; a valid
            // authored stride of 2^-30 against a distance of 1.0
            // puts the phase at 2^30, and the tick never returned).
            // The reduction is EXACT everywhere exactness is
            // possible, and byte-identical to the repeated
            // subtraction it replaces: for a phase in [1, 2^24)
            // every integer up to the phase and every multiple of
            // the phase's own ulp in [0, 1) are representable, so
            // both `floor` and the subtraction are exact — the
            // repeated subtraction was the same exact difference
            // taken one unit at a time. At 2^24 and beyond a finite
            // f32 is itself an integer, so `phase − floor(phase)` is
            // exactly 0.0 — the value the unbounded loop would have
            // reached had the arithmetic been unbounded. A
            // NON-FINITE phase (distance/stride overflowing f32 —
            // valid authored inputs can do it: a giant speed over a
            // tiny stride) is clamped to 0.0 and counted in
            // `phase_clamps`, fail-visible.
            if !blocked {
                state.stride_phase += distance / route.stride_length();
                if state.stride_phase.is_finite() {
                    if state.stride_phase >= 1.0 {
                        state.stride_phase -= state.stride_phase.floor();
                    }
                } else {
                    state.stride_phase = 0.0;
                    summary.phase_clamps += 1;
                }
            }
        }

        if arrived.is_some() {
            summary.arrivals += 1;
        }
        if blocked {
            summary.blocked += 1;
        }
    }

    summary
}

/// One recorded sample in the locomotion stream (the digest's unit —
/// the caller records per tick per present NPC, in identity order).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocomotionSample {
    pub npc: u32,
    /// The continuous position, hashed by its f32 bit patterns.
    pub position: [f32; 2],
    pub facing: u16,
    pub stride_phase: f32,
    pub leg: u32,
}

/// FNV-1a 64 (the repo's constants, inline like `execution_digest`)
/// over, per sample, in THIS fixed order: npc (u32 LE), position x/z
/// (f32 `to_bits` LE), facing (u16 LE), stride_phase (f32 `to_bits`
/// LE), leg (u32 LE).
///
/// THE CROSS-PLATFORM BYTE PIN: a fixed campaign's locomotion stream
/// hashes to one u64 everywhere; a divergence is an investigation,
/// never a repin.
pub fn locomotion_digest(samples: &[LocomotionSample]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut fold = |bytes: &[u8]| {
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for sample in samples {
        fold(&sample.npc.to_le_bytes());
        fold(&sample.position[0].to_bits().to_le_bytes());
        fold(&sample.position[1].to_bits().to_le_bytes());
        fold(&sample.facing.to_le_bytes());
        fold(&sample.stride_phase.to_bits().to_le_bytes());
        fold(&sample.leg.to_le_bytes());
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Tick;
    use crate::event::{Event, EventClass, EventSpec, Field, Schedule};
    use crate::npc_behavior::{Activity, ActivityKind, MAX_NEEDS, NpcRoutine, Rule};
    use crate::npc_execution::{
        StreamEntry, evaluate_wake, execution_digest, wake_payload, wake_payload_parts,
    };
    use crate::npc_schedule::{ActivityDescriptor, Cadence, Commitment, NpcSchedule, Target};
    use crate::spine::{ExecCtx, Spine, World};

    const CLASS_WAKE: EventClass = EventClass::new(0xC4);

    /// The shared square loop: origin (0,0), legs 8/6/8/6 around the
    /// quadrant (perimeter 28), speed 8, stride 2, rate 100, width
    /// 0.5, closed exactly. dt 0.125 → 1.0 unit/tick at scale 1.0:
    /// crossings at ticks 8, 14, 22, 28 (the wrap) — every fixture
    /// below is binary-exact.
    fn square_loop() -> Route {
        Route::new(
            [0.0, 0.0],
            vec![
                RouteLeg {
                    to: [8.0, 0.0],
                    turn: 0,
                },
                RouteLeg {
                    to: [8.0, 6.0],
                    turn: 16384,
                },
                RouteLeg {
                    to: [0.0, 6.0],
                    turn: 32768,
                },
                RouteLeg {
                    to: [0.0, 0.0],
                    turn: 49152,
                },
            ],
            8.0,
            0.5,
            2.0,
            100,
            true,
            0,
        )
        .expect("the square loop closes exactly")
    }

    /// The tests' single-NPC driver. `&mut [state]` would build a
    /// temporary array COPY and mutate that — `slice::from_mut`
    /// advances the real state. npc_half/height fixed at 0.25/1.0
    /// (every single-NPC fixture's shape).
    fn step1(
        route: &Route,
        state: &mut NpcLocomotion,
        dt: f32,
        world: Option<&CollisionWorld>,
    ) -> BatchSummary {
        advance_batch(
            std::slice::from_ref(route),
            std::slice::from_mut(state),
            &[true],
            dt,
            world,
            0.25,
            1.0,
        )
    }

    /// Test 1: construction geometry, exact (§8.3's fixed-order
    /// arithmetic — every value here is binary-exact).
    #[test]
    fn route_construction_geometry_exact() {
        let route = Route::new(
            [0.0, 0.0],
            vec![
                RouteLeg {
                    to: [8.0, 0.0],
                    turn: 0,
                },
                RouteLeg {
                    to: [8.0, 6.0],
                    turn: 16384,
                },
            ],
            2.0,
            0.0,
            1.0,
            0,
            false,
            0,
        )
        .expect("an open two-leg route is valid");

        // Leg 0: (0,0)→(8,0) — length 8 (sqrt(64) exact), unit (1,0)
        // exact, perp (0,−1) exact, inv 0.125 exact.
        assert_eq!(route.leg_from(0), [0.0, 0.0]);
        assert_eq!(route.leg_to(0), [8.0, 0.0]);
        assert_eq!(route.leg_length(0), 8.0);
        assert_eq!(route.leg_turn(0), 0);

        // Leg 1: (8,0)→(8,6) — chained from leg 0's destination.
        assert_eq!(route.leg_from(1), [8.0, 0.0]);
        assert_eq!(route.leg_to(1), [8.0, 6.0]);
        assert_eq!(route.leg_length(1), 6.0);
        assert_eq!(route.leg_turn(1), 16384);

        // The precomputed geometry: exact where f32 is exact, and
        // the round-trip identity where it is not.
        assert_eq!(route.per_leg[0].inv_length, 0.125, "1/8 is exact");
        assert_eq!(route.per_leg[0].unit, [1.0, 0.0]);
        assert_eq!(route.per_leg[0].perp, [0.0, -1.0]);
        // 1/6 is not exact in f32, but (f32(1/6))·6 rounds to 1.0
        // (error 2.98e-8 < half the spacing at 1.0) — the unit
        // vector is exact by the same product.
        assert_eq!(route.per_leg[1].inv_length * 6.0, 1.0, "1/6 round-trips");
        assert_eq!(route.per_leg[1].unit, [0.0, 1.0]);
        assert_eq!(route.per_leg[1].perp, [1.0, 0.0]);
    }

    /// Test 2: construction refusals — every fail-closed clause.
    #[test]
    fn route_construction_refuses() {
        let leg = RouteLeg {
            to: [1.0, 0.0],
            turn: 0,
        };
        let ok = |legs: Vec<RouteLeg>, loops: bool| {
            Route::new([0.0, 0.0], legs, 1.0, 0.0, 1.0, 0, loops, 0)
        };

        assert!(ok(vec![], false).is_none(), "empty legs");
        assert!(
            Route::new([f32::NAN, 0.0], vec![leg], 1.0, 0.0, 1.0, 0, false, 0).is_none(),
            "NaN origin"
        );
        assert!(
            Route::new([0.0, f32::INFINITY], vec![leg], 1.0, 0.0, 1.0, 0, false, 0).is_none(),
            "infinite origin"
        );
        assert!(
            Route::new(
                [0.0, 0.0],
                vec![RouteLeg {
                    to: [f32::NAN, 0.0],
                    turn: 0
                }],
                1.0,
                0.0,
                1.0,
                0,
                false,
                0
            )
            .is_none(),
            "NaN leg destination"
        );
        assert!(
            Route::new([0.0, 0.0], vec![leg], 0.0, 0.0, 1.0, 0, false, 0).is_none(),
            "zero speed"
        );
        assert!(
            Route::new([0.0, 0.0], vec![leg], -1.0, 0.0, 1.0, 0, false, 0).is_none(),
            "negative speed"
        );
        assert!(
            Route::new([0.0, 0.0], vec![leg], 1.0, 0.0, 0.0, 0, false, 0).is_none(),
            "zero stride length"
        );
        assert!(
            Route::new([0.0, 0.0], vec![leg], 1.0, 0.0, -1.0, 0, false, 0).is_none(),
            "negative stride length"
        );
        assert!(
            Route::new([0.0, 0.0], vec![leg], 1.0, -0.1, 1.0, 0, false, 0).is_none(),
            "negative spread width"
        );
        assert!(
            Route::new(
                [0.0, 0.0],
                vec![RouteLeg {
                    to: [0.0, 0.0],
                    turn: 0
                }],
                1.0,
                0.0,
                1.0,
                0,
                false,
                0
            )
            .is_none(),
            "zero-length leg (to == from)"
        );
        // An OPEN route that does not return to its origin is VALID
        // (the crosswalk shape) — only loops must close.
        assert!(ok(vec![leg], false).is_some(), "an open route is valid");
        // A loop that does not close exactly is refused.
        assert!(
            ok(
                vec![
                    RouteLeg {
                        to: [1.0, 0.0],
                        turn: 0
                    },
                    RouteLeg {
                        to: [1.0, 1.0],
                        turn: 16384
                    },
                    RouteLeg {
                        to: [0.0, 1.0],
                        turn: 32768
                    },
                ],
                true
            )
            .is_none(),
            "an unclosed loop is refused"
        );
        // The closed square loop constructs.
        assert!(square_loop().leg_count() == 4);
    }

    /// Test 3: start_route's known state — the reconstruction proof
    /// (the three draws re-derived from the seed's own stream).
    #[test]
    fn start_route_known_state() {
        let route = square_loop();
        let state = start_route(0, &route, 7, 0, LOOPING).expect("a loop stroll is valid");

        assert_eq!(state.position, [0.0, 0.0], "waypoint 0 is the origin");
        assert_eq!(state.facing, 0, "the departing leg's authored turn");
        assert_eq!(state.route, 0);
        assert_eq!(state.leg, 0);
        assert_eq!(state.leg_distance, 0.0);
        assert_eq!(state.destination, LOOPING);
        assert_eq!(state.speed, 8.0, "walking at the route's canonical speed");

        // The three draws, re-derived in the same fixed order from
        // the appearance seed's OWN stream — the activation-time
        // reconstruction is byte-identical by construction.
        let mut rng = NpcRng::for_npc(NpcId(7));
        let d1 = rng.next_unit();
        let d2 = rng.next_unit();
        let d3 = rng.next_unit();
        assert_eq!(
            state.speed_scale,
            0.9 + 0.2 * d1,
            "speed_scale = 0.9 + 0.2·d1"
        );
        assert_eq!(state.lateral, 2.0 * d2 - 1.0, "lateral = 2·d2 − 1");
        assert_eq!(state.stride_phase, d3, "stride_phase = d3");

        // The loop-end start wraps to leg 0 (the closure).
        let state = start_route(0, &route, 7, 4, LOOPING).expect("the loop end wraps");
        assert_eq!(state.position, [0.0, 0.0], "waypoint 4 is waypoint 0");
        assert_eq!(state.leg, 0, "the departing leg wraps to 0");
    }

    /// Test 4: start_route's refusals — every fail-closed clause.
    #[test]
    fn start_route_refusals() {
        let route = square_loop();
        let leg_count = route.leg_count() as u32;

        assert!(
            start_route(0, &route, 0, leg_count + 1, 0).is_none(),
            "at > leg_count"
        );
        // An OPEN route's end waypoint has no departing leg.
        let open = Route::new(
            [0.0, 0.0],
            vec![
                RouteLeg {
                    to: [8.0, 0.0],
                    turn: 0,
                },
                RouteLeg {
                    to: [8.0, 6.0],
                    turn: 16384,
                },
            ],
            8.0,
            0.0,
            2.0,
            0,
            false,
            0,
        )
        .expect("the open route is valid");
        assert!(
            start_route(0, &open, 0, 2, 2).is_none(),
            "at the open end without a loop"
        );
        assert!(
            start_route(0, &open, 0, 0, 3).is_none(),
            "destination > leg_count"
        );
        assert!(
            start_route(0, &open, 0, 1, 0).is_none(),
            "backward destination on an open route"
        );
        assert!(
            start_route(0, &open, 0, 0, LOOPING).is_none(),
            "LOOPING on an open route"
        );
        assert!(
            start_route(0, &route, 0, 0, leg_count).is_none(),
            "destination == leg_count on a loop (waypoint leg_count IS waypoint 0)"
        );
        // The standing start is valid and stands.
        let state = start_route(0, &route, 7, 2, 2).expect("destination == at stands");
        assert_eq!(state.speed, 0.0);
        assert_eq!(state.position, [8.0, 6.0], "standing at waypoint 2");
    }

    /// Test 5: the advance known answer — every intermediate exact.
    /// Fixture: a closed 2-leg loop (16 out, 16 back), speed 2,
    /// dt 0.125 → distance 0.25/tick (all binary-exact).
    #[test]
    fn advance_exact_known_answer() {
        let route = Route::new(
            [0.0, 0.0],
            vec![
                RouteLeg {
                    to: [8.0, 0.0],
                    turn: 0,
                },
                RouteLeg {
                    to: [0.0, 0.0],
                    turn: 32768,
                },
            ],
            2.0,
            0.0,
            1.0,
            0,
            true,
            0,
        )
        .expect("the out-and-back loop closes");
        let mut states = vec![start_route(0, &route, 0, 0, LOOPING).expect("valid stroll")];
        // The arithmetic isolation: the draws' effects are pinned by
        // test 3; THIS test pins the step math at scale exactly 1.0
        // and a zero phase/lateral baseline.
        states[0].speed_scale = 1.0;
        states[0].stride_phase = 0.0;
        states[0].lateral = 0.0;
        let dt = 0.125;

        // Tick 1: leg_distance 0.25; position = from + 0.25·(1,0).
        advance_batch(
            std::slice::from_ref(&route),
            &mut states,
            &[true],
            dt,
            None,
            0.25,
            1.0,
        );
        assert_eq!(states[0].leg_distance, 0.25);
        assert_eq!(states[0].position, [0.25, 0.0], "one exact step east");
        // The phase advances by distance/stride = 0.25/1.0 — exact.
        assert_eq!(states[0].stride_phase, 0.25);

        // 31 more ticks: leg_distance 8.0 == the leg length — the
        // crossing fires (>= is the pin), remainder 0.0, leg 1, and
        // the position is leg 1's from exactly: (8, 0).
        for _ in 0..31 {
            advance_batch(
                std::slice::from_ref(&route),
                &mut states,
                &[true],
                dt,
                None,
                0.25,
                1.0,
            );
        }
        assert_eq!(states[0].leg, 1, "crossed into the return leg");
        assert_eq!(states[0].leg_distance, 0.0, "the remainder is exact");
        assert_eq!(states[0].position, [8.0, 0.0], "the waypoint, byte-exact");
        // 32 ticks × 0.25 = 8.0 strides — the phase wrapped to 0.0.
        assert_eq!(states[0].stride_phase, 0.0);
    }

    /// Test 6: the crossing carries its remainder — two fixtures,
    /// both binary-exact.
    #[test]
    fn leg_crossing_carries_remainder_exact() {
        // Fixture A: a square of side 2 (four legs, closed), speed 2,
        // dt 0.5 → 1.0/tick: crossings every second tick, remainder 0.
        let square2 = Route::new(
            [0.0, 0.0],
            vec![
                RouteLeg {
                    to: [2.0, 0.0],
                    turn: 0,
                },
                RouteLeg {
                    to: [2.0, 2.0],
                    turn: 16384,
                },
                RouteLeg {
                    to: [0.0, 2.0],
                    turn: 32768,
                },
                RouteLeg {
                    to: [0.0, 0.0],
                    turn: 49152,
                },
            ],
            2.0,
            0.0,
            4.0,
            0,
            true,
            0,
        )
        .expect("the side-2 square closes");
        let mut states = vec![start_route(0, &square2, 0, 0, LOOPING).expect("valid stroll")];
        states[0].speed_scale = 1.0; // the arithmetic isolation (test 3 pins the draw)
        let dt = 0.5;

        advance_batch(
            std::slice::from_ref(&square2),
            &mut states,
            &[true],
            dt,
            None,
            0.25,
            1.0,
        );
        assert_eq!((states[0].leg, states[0].leg_distance), (0, 1.0));

        // Tick 2: 2.0 >= 2.0 → crossing, remainder 0.0, leg 1.
        advance_batch(
            std::slice::from_ref(&square2),
            &mut states,
            &[true],
            dt,
            None,
            0.25,
            1.0,
        );
        assert_eq!((states[0].leg, states[0].leg_distance), (1, 0.0));
        assert_eq!(states[0].position, [2.0, 0.0]);

        // Ticks 3-8: crossings at 4, 6, and the WRAP at 8 — back on
        // leg 0 at the origin exactly.
        for _ in 0..6 {
            advance_batch(
                std::slice::from_ref(&square2),
                &mut states,
                &[true],
                dt,
                None,
                0.25,
                1.0,
            );
        }
        assert_eq!((states[0].leg, states[0].leg_distance), (0, 0.0));
        assert_eq!(states[0].position, [0.0, 0.0], "the wrap, byte-exact");

        // Fixture B: legs of length 1.5 (exact in f32), open, walked
        // to the end — the remainder 2.0 − 1.5 = 0.5 is carried
        // EXACTLY into leg 1.
        let odd = Route::new(
            [0.0, 0.0],
            vec![
                RouteLeg {
                    to: [1.5, 0.0],
                    turn: 0,
                },
                RouteLeg {
                    to: [3.0, 0.0],
                    turn: 0,
                },
            ],
            2.0,
            0.0,
            1.0,
            0,
            false,
            0,
        )
        .expect("the open odd route is valid");
        let mut states = vec![start_route(0, &odd, 0, 0, 2).expect("walk to the end")];
        states[0].speed_scale = 1.0; // the arithmetic isolation (test 3 pins the draw)

        advance_batch(
            std::slice::from_ref(&odd),
            &mut states,
            &[true],
            dt,
            None,
            0.25,
            1.0,
        );
        assert_eq!(states[0].leg_distance, 1.0, "1.0 < 1.5: no crossing");
        assert_eq!(states[0].position, [1.0, 0.0]);

        // Tick 2: 2.0 >= 1.5 → crossing; remainder 0.5 carried.
        advance_batch(
            std::slice::from_ref(&odd),
            &mut states,
            &[true],
            dt,
            None,
            0.25,
            1.0,
        );
        assert_eq!((states[0].leg, states[0].leg_distance), (1, 0.5));
        assert_eq!(
            states[0].position,
            [2.0, 0.0],
            "1.5 + 0.5·(1,0) — the carried remainder, exact"
        );

        // Tick 3: 1.5 >= 1.5 → the end-waypoint arrival.
        let summary = advance_batch(
            std::slice::from_ref(&odd),
            &mut states,
            &[true],
            dt,
            None,
            0.25,
            1.0,
        );
        assert_eq!(summary.arrivals, 1);
        assert_eq!(states[0].position, [3.0, 0.0], "the authored end bytes");
        assert_eq!(states[0].speed, 0.0);
    }

    /// Test 7: a destination arrival snaps to the authored waypoint
    /// bytes and stands — 14 ticks, not 8 (leg 0 is 8 units, leg 1
    /// is 6; at 1.0/tick the walk is 8 + 6).
    #[test]
    fn destination_arrival_snaps_and_stands() {
        let route = Route::new(
            [0.0, 0.0],
            vec![
                RouteLeg {
                    to: [8.0, 0.0],
                    turn: 0,
                },
                RouteLeg {
                    to: [8.0, 6.0],
                    turn: 16384,
                },
            ],
            8.0,
            0.0,
            2.0,
            100,
            false,
            0,
        )
        .expect("the open walk is valid");
        let mut states = vec![start_route(0, &route, 0, 0, 2).expect("walk to waypoint 2")];
        let dt = 0.125;

        let mut total_arrivals = 0u32;
        for _ in 0..14 {
            total_arrivals += advance_batch(
                std::slice::from_ref(&route),
                &mut states,
                &[true],
                dt,
                None,
                0.25,
                1.0,
            )
            .arrivals;
        }
        assert_eq!(total_arrivals, 1, "exactly one arrival");
        assert_eq!(
            states[0].position,
            [8.0, 6.0],
            "the authored bytes, no arithmetic"
        );
        assert_eq!(states[0].speed, 0.0);
        assert_eq!(states[0].leg, 2, "standing AT waypoint 2");
        assert_eq!(states[0].leg_distance, 0.0);

        // Standing is stable: the whole state byte-frozen.
        let frozen = states[0];
        let summary = advance_batch(
            std::slice::from_ref(&route),
            &mut states,
            &[true],
            dt,
            None,
            0.25,
            1.0,
        );
        assert_eq!(states[0], frozen, "nothing moves after arrival");
        assert_eq!(summary.stepped, 0, "standing NPCs are not stepped");
        assert_eq!(summary.arrivals, 0);
    }

    /// Test 8: the loop wrap walks forever — the full cycle pinned,
    /// 200 ticks (7 cycles + 4 units), no arrival ever.
    #[test]
    fn loop_wrap_continues_forever() {
        let route = square_loop();
        let mut states = vec![start_route(0, &route, 0, 0, LOOPING).expect("the stroll")];
        // The arithmetic isolation (test 3 pins the draws).
        states[0].speed_scale = 1.0;
        states[0].stride_phase = 0.0;
        states[0].lateral = 0.0;
        let dt = 0.125;

        let mut arrivals = 0u32;
        // The cycle: crossings at 8 (→(8,0)), 14 (→(8,6)), 22
        // (→(0,6)), 28 (the wrap →(0,0), leg 0).
        for _ in 0..8 {
            arrivals += advance_batch(
                std::slice::from_ref(&route),
                &mut states,
                &[true],
                dt,
                None,
                0.25,
                1.0,
            )
            .arrivals;
        }
        assert_eq!((states[0].leg, states[0].leg_distance), (1, 0.0));
        assert_eq!(states[0].position, [8.0, 0.0]);
        for _ in 0..6 {
            arrivals += advance_batch(
                std::slice::from_ref(&route),
                &mut states,
                &[true],
                dt,
                None,
                0.25,
                1.0,
            )
            .arrivals;
        }
        assert_eq!((states[0].leg, states[0].leg_distance), (2, 0.0));
        assert_eq!(states[0].position, [8.0, 6.0]);
        for _ in 0..8 {
            arrivals += advance_batch(
                std::slice::from_ref(&route),
                &mut states,
                &[true],
                dt,
                None,
                0.25,
                1.0,
            )
            .arrivals;
        }
        assert_eq!((states[0].leg, states[0].leg_distance), (3, 0.0));
        assert_eq!(states[0].position, [0.0, 6.0]);
        for _ in 0..6 {
            arrivals += advance_batch(
                std::slice::from_ref(&route),
                &mut states,
                &[true],
                dt,
                None,
                0.25,
                1.0,
            )
            .arrivals;
        }
        assert_eq!((states[0].leg, states[0].leg_distance), (0, 0.0));
        assert_eq!(
            states[0].position,
            [0.0, 0.0],
            "the wrap lands on the origin"
        );

        // 172 more ticks = 6 full cycles (168) + 4: leg 0, 4 units in.
        for _ in 0..172 {
            arrivals += advance_batch(
                std::slice::from_ref(&route),
                &mut states,
                &[true],
                dt,
                None,
                0.25,
                1.0,
            )
            .arrivals;
        }
        assert_eq!(states[0].speed, 8.0, "still walking");
        assert_eq!((states[0].leg, states[0].leg_distance), (0, 4.0));
        assert_eq!(states[0].position, [4.0, 0.0]);
        assert_eq!(arrivals, 0, "a stroll never arrives");
        // 200 ticks × (1.0/2.0 strides) = 100 strides — phase 0.0.
        assert_eq!(states[0].stride_phase, 0.0);
    }

    /// Test 9: set_destination's semantics — every branch.
    #[test]
    fn set_destination_semantics() {
        let route = square_loop();
        let leg_count = route.leg_count() as u32;

        // Forward retarget: continues from the current leg_distance.
        let mut state = start_route(0, &route, 7, 0, 2).expect("walking");
        step1(&route, &mut state, 0.125, None);
        let advanced = state;
        assert!(set_destination(&route, &mut state, 1));
        assert_eq!(state.leg_distance, advanced.leg_distance, "no reset");
        assert_eq!(state.speed, 8.0);

        // Retarget to the standing waypoint: stands immediately.
        let standing = state.leg;
        assert!(set_destination(&route, &mut state, standing));
        assert_eq!(state.speed, 0.0);

        // Backward on a loop: allowed (forward the wrapping way).
        let mut state = start_route(0, &route, 7, 2, 2).expect("standing at 2");
        assert!(
            set_destination(&route, &mut state, 0),
            "loops wrap backward targets"
        );
        assert_eq!(state.speed, 8.0);

        // Backward on an open route: refused, state byte-identical.
        let open = Route::new(
            [0.0, 0.0],
            vec![
                RouteLeg {
                    to: [8.0, 0.0],
                    turn: 0,
                },
                RouteLeg {
                    to: [8.0, 6.0],
                    turn: 16384,
                },
            ],
            8.0,
            0.0,
            2.0,
            0,
            false,
            0,
        )
        .expect("the open route");
        let mut state = start_route(0, &open, 7, 1, 2).expect("on leg 1");
        let before = state;
        assert!(!set_destination(&open, &mut state, 0), "backward refused");
        assert_eq!(state, before, "the refusal leaves the state untouched");

        // The authoring contradictions, refused identically.
        assert!(
            !set_destination(&open, &mut state, LOOPING),
            "LOOPING on open"
        );
        assert!(
            !set_destination(&route, &mut state, leg_count),
            "leg_count on a loop"
        );
        assert!(
            !set_destination(&route, &mut state, leg_count + 1),
            "> leg_count"
        );

        // Standing at an open route's end: retargeting the end
        // stands (already there); anything backward is refused.
        let mut state = start_route(0, &open, 7, 0, 2).expect("walking to the end");
        for _ in 0..14 {
            step1(&open, &mut state, 0.125, None);
        }
        assert_eq!(state.leg, 2, "standing at the open end");
        assert!(
            set_destination(&open, &mut state, 2),
            "the end re-target stands"
        );
        assert_eq!(state.speed, 0.0);
        assert!(
            !set_destination(&open, &mut state, 1),
            "backward from the end refused"
        );
    }

    /// Test 10: the turn blend — shortest way, integer units, rate
    /// clamping, the wrap case, and the zero-rate freeze. Fixtures
    /// use a 64-unit leg so no arrival freezes the blend mid-proof.
    #[test]
    fn turn_blend_shortest_way() {
        let long_leg = |turn: u16, rate: u16| {
            Route::new(
                [0.0, 0.0],
                vec![RouteLeg {
                    to: [64.0, 0.0],
                    turn,
                }],
                1.0,
                0.0,
                1.0,
                rate,
                false,
                0,
            )
            .expect("the long walk")
        };
        let walking = |facing: u16, route_index: u32| NpcLocomotion {
            position: [0.0, 0.0],
            facing,
            route: route_index,
            leg: 0,
            leg_distance: 0.0,
            destination: 1,
            speed: 1.0,
            speed_scale: 1.0,
            lateral: 0.0,
            stride_phase: 0.0,
        };

        // facing 0 → target 16384 at rate 400: after k ticks facing
        // = min(400k, 16384) — k = 41 clamps exactly onto the target.
        let route = long_leg(16384, 400);
        let mut state = walking(0, 0);
        for k in 1..=41u16 {
            step1(&route, &mut state, 0.125, None);
            assert_eq!(state.facing, (400 * k).min(16384), "tick {k}");
        }
        assert_eq!(state.facing, 16384, "clamped onto the target");

        // The wrap: facing 60000 → target 1000 at rate 1000. The
        // shortest way: delta = ((1000 − 60000 + 32768) rem 65536)
        // − 32768 = 39304 − 32768 = +6536 (clockwise). k = 6:
        // 60000 + 6000 = 66000 rem 65536 = 464; k = 7: the delta
        // clamps to 6536 → 60000 + 6536 = 66536 rem 65536 = 1000.
        let route = long_leg(1000, 1000);
        let mut state = walking(60000, 0);
        for _ in 0..6 {
            step1(&route, &mut state, 0.125, None);
        }
        assert_eq!(state.facing, 464, "six rate steps, wrapped");
        step1(&route, &mut state, 0.125, None);
        assert_eq!(
            state.facing, 1000,
            "clamped through the wrap onto the target"
        );

        // The zero rate freezes facing (documented: authors who want
        // snaps author the start facing, which start_route does).
        let route = long_leg(32768, 0);
        let mut state = walking(0, 0);
        step1(&route, &mut state, 0.125, None);
        assert_eq!(state.facing, 0, "rate 0 freezes");
    }

    /// Test 11: the lateral fade — the smoothstep window's exact
    /// values at edge-exact ticks, and the no-pop-at-corners proof.
    /// Fixture: a 16-unit leg, speed 2, dt 0.125 → 0.25/tick; t =
    /// k/64 (exact); edge = min(t, 1−t)/0.15 = 5k/48 (exact for the
    /// chosen k); lateral 1.0 × width 0.5 along perp (0,−1).
    #[test]
    fn lateral_fade_no_corner_pop() {
        let route = Route::new(
            [0.0, 0.0],
            vec![
                RouteLeg {
                    to: [16.0, 0.0],
                    turn: 0,
                },
                RouteLeg {
                    to: [0.0, 0.0],
                    turn: 32768,
                },
            ],
            2.0,
            0.5,
            4.0,
            0,
            true,
            0,
        )
        .expect("the out-and-back loop");
        let mut state = NpcLocomotion {
            position: [0.0, 0.0],
            facing: 0,
            route: 0,
            leg: 0,
            leg_distance: 0.0,
            destination: LOOPING,
            speed: 2.0,
            speed_scale: 1.0,
            lateral: 1.0,
            stride_phase: 0.0,
        };

        // k = 3: t = 3/64, edge = 15/48 = 0.3125, fade = 0.3125² ·
        // (3 − 0.625) = 0.09765625 · 2.375 = 475/2048; offset =
        // 0.5 · 475/2048 = 475/4096 = 0.115966796875 (all binary).
        for _ in 0..3 {
            step1(&route, &mut state, 0.125, None);
        }
        assert_eq!(state.position, [0.75, -(475.0 / 4096.0)], "k=3 ease-in");

        // k = 6: edge = 30/48 = 0.625, fade = 0.625² · 1.75 =
        // 175/512; offset = 0.5 · 175/512 = 0.341796875.
        for _ in 0..3 {
            step1(&route, &mut state, 0.125, None);
        }
        assert_eq!(state.position, [1.5, -(175.0 / 512.0)], "k=6 mid-window");

        // k = 16: t = 0.25 → edge 1 (clamped), fade 1: the full
        // offset, mid-leg.
        for _ in 0..10 {
            step1(&route, &mut state, 0.125, None);
        }
        assert_eq!(state.position, [4.0, -0.5], "k=16 the full spread");

        // k = 58 and k = 61: the ease-out mirrors the ease-in
        // (t ↔ 1−t): k=61 → 1 − 61/64 = 3/64 — the k=3
        // values; k=58 → 1 − 58/64 = 6/64 — the k=6 values.
        for _ in 0..42 {
            step1(&route, &mut state, 0.125, None);
        }
        assert_eq!(state.position, [14.5, -(175.0 / 512.0)], "k=58 mirrors k=6");
        for _ in 0..3 {
            step1(&route, &mut state, 0.125, None);
        }
        assert_eq!(
            state.position,
            [15.25, -(475.0 / 4096.0)],
            "k=61 mirrors k=3"
        );

        // k = 64: the crossing — remainder 0, t = 0 on the return
        // leg, fade 0: the position is the waypoint EXACTLY (z = 0,
        // no pop) — the corner-continuity proof. (61 + 3 = 64.)
        for _ in 0..3 {
            step1(&route, &mut state, 0.125, None);
        }
        assert_eq!(state.leg, 1);
        assert_eq!(
            state.position,
            [16.0, 0.0],
            "the corner lands on the waypoint"
        );
    }

    /// Test 12: collision blocks pin and freeze — the player's
    /// depenetration contract, the honest revert, the frozen phase,
    /// the still-turning facing, idempotence, and the counterfactual.
    /// Fixture: a 10-unit walk, a 2-footprint box at x ∈ [3, 5], NPC
    /// half 0.25, speed 2, dt 0.125 → 0.25/tick: overlap first at
    /// k = 12 (candidate x = 3.0, max face 3.25 > 3).
    #[test]
    fn collision_blocks_pin_and_freeze() {
        let route = Route::new(
            [0.0, 0.0],
            vec![RouteLeg {
                to: [10.0, 0.0],
                turn: 16384,
            }],
            2.0,
            0.0,
            1.0,
            100,
            false,
            0,
        )
        .expect("the walk");
        let world = CollisionWorld::new(0.0).with_box([4.0, 0.0, 0.0], 2.0, 1.0);
        let mut state = NpcLocomotion {
            position: [0.0, 0.0],
            facing: 0,
            route: 0,
            leg: 0,
            leg_distance: 0.0,
            destination: 1,
            speed: 2.0,
            speed_scale: 1.0,
            lateral: 0.0,
            stride_phase: 0.0,
        };

        // 11 unblocked ticks: leg_distance 2.75, phase 0.75 (11 ×
        // 0.25 mod 1), facing 1100 (rate 100 toward 16384).
        for _ in 0..11 {
            step1(&route, &mut state, 0.125, Some(&world));
        }
        assert_eq!(state.leg_distance, 2.75);
        assert_eq!(
            state.position,
            [2.75, 0.0],
            "clear of the box (max face at 3.0)"
        );
        assert_eq!(state.stride_phase, 0.75);

        // Tick 12: candidate x = 3.0 overlaps (3.25 > 3) — the
        // minimum-magnitude push exits the WEST face: position.x +
        // 0.25 lands at 3 − SKIN, so position.x < 2.75; the
        // along-leg distance reverts to 2.75; the phase freezes.
        let summary = step1(&route, &mut state, 0.125, Some(&world));
        assert_eq!(summary.blocked, 1);
        assert!(
            state.position[0] < 2.75,
            "pushed fully clear of the west face"
        );
        assert!(
            state.position[0] > 2.7,
            "the push magnitude is |3 − SKIN − 3.25| ≈ 0.25"
        );
        assert_eq!(state.leg_distance, 2.75, "the honest no-progress revert");
        assert_eq!(state.stride_phase, 0.75, "no foot slide");
        assert_eq!(state.facing, 1200, "facing still blends while blocked");
        let pinned = state;

        // The idempotence: the next blocked tick recomputes the same
        // candidate (same reverted leg_distance) and the same push.
        for _ in 0..5 {
            step1(&route, &mut state, 0.125, Some(&world));
        }
        assert_eq!(
            state.position, pinned.position,
            "pinned-blocked idempotence"
        );
        assert_eq!(state.leg_distance, 2.75);
        assert_eq!(state.stride_phase, 0.75, "still frozen");
        assert_eq!(state.facing, 1700, "facing advanced 5 more rate steps");
        assert_eq!(
            state.speed, 2.0,
            "blocked keeps the speed — the walk resumes"
        );

        // The counterfactual: the same fixture without the world
        // walks through — tick 12's position is the candidate, 3.0.
        let mut free = NpcLocomotion {
            position: [0.0, 0.0],
            facing: 0,
            route: 0,
            leg: 0,
            leg_distance: 0.0,
            destination: 1,
            speed: 2.0,
            speed_scale: 1.0,
            lateral: 0.0,
            stride_phase: 0.0,
        };
        for _ in 0..12 {
            step1(&route, &mut free, 0.125, None);
        }
        assert_eq!(
            free.position,
            [3.0, 0.0],
            "the counterfactual walks through"
        );
    }

    /// Test 13: committed_cell's known answers — the coarse bridge.
    #[test]
    fn committed_cell_known_answers() {
        // A unit grid at the origin, 16 cells per side.
        assert_eq!(committed_cell([0.9, 3.2], [0.0, 0.0], 1.0, 16), (0, 3));
        assert_eq!(
            committed_cell([-5.0, 8.0], [0.0, 0.0], 1.0, 16),
            (0, 8),
            "clamp low"
        );
        assert_eq!(
            committed_cell([100.0, 3.0], [0.0, 0.0], 1.0, 16),
            (15, 3),
            "clamp high"
        );
        // The region-plate inverse (region_render's mapping: world =
        // center + cell − 7.5): the grid's min corner at −7.5 puts
        // the world origin at cell (7, 7) — floor(7.5) = 7.
        assert_eq!(
            committed_cell([0.0, 0.0], [-7.5, -7.5], 1.0, 16),
            (7, 7),
            "the plate inverse"
        );
    }

    /// Test 14: the clip bands.
    #[test]
    fn clip_for_bands() {
        assert_eq!(clip_for(0.0, 1.0), LocomotionClip::Stand);
        assert_eq!(clip_for(0.99, 1.0), LocomotionClip::Stand);
        assert_eq!(
            clip_for(1.0, 1.0),
            LocomotionClip::Walk,
            "at the threshold walks"
        );
        assert_eq!(clip_for(1.01, 1.0), LocomotionClip::Walk);
    }

    /// Test 15: the digest's purity and sensitivity.
    #[test]
    fn digest_purity_and_sensitivity() {
        let samples = [
            LocomotionSample {
                npc: 1,
                position: [1.0, 2.0],
                facing: 100,
                stride_phase: 0.5,
                leg: 0,
            },
            LocomotionSample {
                npc: 2,
                position: [3.0, 4.0],
                facing: 200,
                stride_phase: 0.75,
                leg: 1,
            },
        ];

        assert_eq!(
            locomotion_digest(&samples),
            locomotion_digest(&samples),
            "pure"
        );
        assert_eq!(
            locomotion_digest(&[]),
            0xcbf2_9ce4_8422_2325,
            "empty = the basis"
        );

        // One position LSB flip.
        let mut flipped = samples;
        flipped[0].position[0] = f32::from_bits(flipped[0].position[0].to_bits() ^ 1);
        assert_ne!(locomotion_digest(&flipped), locomotion_digest(&samples));

        // One facing unit, one leg unit, one npc — each differs.
        let mut bumped = samples;
        bumped[0].facing += 1;
        assert_ne!(locomotion_digest(&bumped), locomotion_digest(&samples));
        let mut bumped = samples;
        bumped[0].leg += 1;
        assert_ne!(locomotion_digest(&bumped), locomotion_digest(&samples));
        let mut bumped = samples;
        bumped[0].npc += 1;
        assert_ne!(locomotion_digest(&bumped), locomotion_digest(&samples));
    }

    /// Test 16: the batch's skip semantics — absent, foreign, and
    /// standing NPCs leave their states byte-identical.
    #[test]
    fn advance_batch_skips_absent_and_foreign() {
        let route = Route::new(
            [0.0, 0.0],
            vec![RouteLeg {
                to: [64.0, 0.0],
                turn: 0,
            }],
            2.0,
            0.0,
            1.0,
            0,
            false,
            0,
        )
        .expect("the walk");
        let mut states = vec![
            start_route(0, &route, 1, 0, 1).expect("the mover"),
            start_route(0, &route, 2, 0, 1).expect("the absent one"),
            start_route(0, &route, 3, 0, 0).expect("the stander"),
        ];
        // The arithmetic isolation (test 3 pins the draw).
        states[0].speed_scale = 1.0;
        // A foreign route index, out of the table's range.
        states[2].route = 7;
        states[2].speed = 2.0;
        let before: Vec<_> = states.clone();

        let summary = advance_batch(
            std::slice::from_ref(&route),
            &mut states,
            &[true, false, true],
            0.125,
            None,
            0.25,
            1.0,
        );
        assert_eq!(summary.stepped, 1, "only the present valid mover steps");
        assert_eq!(states[1], before[1], "absent: byte-identical");
        assert_eq!(states[2], before[2], "foreign route: byte-identical");
        assert_eq!(states[0].leg_distance, 0.25, "the mover advanced");
    }

    /// The composition world (test 17/18's harness): a real spine
    /// driving ONE commitment-walked NPC (NPC 1) over the locomotion
    /// batch, with a stroller (NPC 0), a waiter (NPC 2), and an
    /// absent far-tier NPC (NPC 3). POLICY lives here — the
    /// commitment's kind tag interpreted as a MoveTo, the route
    /// table, the present mask — the engine machinery stays pure.
    struct LocoWorld {
        /// The wake-driven NPC's selector-side state.
        routine: NpcRoutine,
        rng: crate::npc_behavior::NpcRng,
        last_decay_tick: Tick,
        schedule: NpcSchedule,
        rules: Vec<Rule>,
        rates: [f32; MAX_NEEDS],
        /// The composed activity stream (the spine-driven record).
        stream: Vec<StreamEntry>,
        wake_executions: u64,
        arrivals: u64,
        stepped: u64,
    }

    impl LocoWorld {
        fn new() -> Self {
            // NPC 1's commitment: recurring every 20 ticks from 10,
            // near lane, kind tag 1 = policy's "walk route 1 to
            // waypoint 1" (the interpretation below).
            let schedule = NpcSchedule {
                entries: vec![Commitment {
                    start: Tick(10),
                    cadence: Cadence::every(20).expect("period 20 is nonzero"),
                    activity: ActivityDescriptor {
                        kind: 1,
                        field: Field::Near,
                    },
                    target: Target {
                        npc: 1,
                        anchor: (0, 0),
                    },
                }],
            };
            // The rule table: one idle baseline (a due commitment
            // always wins the selector — Fulfill is schedule
            // authority, npc_behavior's contract).
            let rules = vec![Rule {
                activity: ActivityKind::Idle,
                need: None,
                base_priority: 0,
                applies_below: 0.0,
                witness: None,
            }];
            LocoWorld {
                routine: NpcRoutine {
                    current: Activity {
                        kind: ActivityKind::Idle,
                        rule: u32::MAX,
                    },
                    needs: [1.0; MAX_NEEDS],
                    route_progress: 0,
                },
                rng: crate::npc_behavior::NpcRng::for_npc(NpcId(1)),
                last_decay_tick: Tick(0),
                schedule,
                rules,
                rates: [0.001; MAX_NEEDS],
                stream: Vec::new(),
                wake_executions: 0,
                arrivals: 0,
                stepped: 0,
            }
        }
    }

    impl World for LocoWorld {
        fn execute(&mut self, event: &Event, ctx: &mut ExecCtx) {
            let Some((npc, commitment)) = wake_payload_parts(event.payload) else {
                return;
            };
            if event.class != CLASS_WAKE || npc != 1 {
                return;
            }
            // Copy-in/copy-out (Copy types) so the borrows never alias.
            let mut rng = self.rng;
            let routine = self.routine;
            let Some(outcome) = evaluate_wake(
                &self.schedule,
                commitment,
                ctx.executed_tick(),
                self.last_decay_tick,
                &routine,
                &self.rules,
                &mut rng,
                &self.rates,
            ) else {
                return;
            };
            self.routine = outcome.routine_after;
            self.rng = rng;
            self.last_decay_tick = ctx.executed_tick();
            self.stream.push(StreamEntry {
                tick: ctx.executed_tick(),
                npc,
                activity: outcome.activity,
            });
            self.wake_executions += 1;
            // The honest re-arm: the entry's next occurrence strictly
            // after the ACTUAL execution tick.
            if let Some(next) = outcome.next_wake {
                let payload = wake_payload(1, 0).expect("the authored index packs");
                ctx.schedule_at(next, CLASS_WAKE, payload, Vec::new());
            }
        }

        /// The canonical logical record: the routine lane, the
        /// commitment cursor, and the execution counters —
        /// order-stable (fixed field order, no unordered iteration),
        /// the BehaviorWorld pattern.
        fn fingerprint(&self, fp: &mut crate::fingerprint::Fingerprinter) {
            fp.write_str("engine-core/npc-locomotion-world/1");
            fp.write_u32(self.routine.current.rule);
            fp.write_u32(match self.routine.current.kind {
                ActivityKind::Idle => 0,
                ActivityKind::MoveTo { .. } => 1,
                ActivityKind::Fulfill { .. } => 2,
                ActivityKind::React { .. } => 3,
            });
            fp.write_u32(self.routine.route_progress);
            for need in self.routine.needs {
                fp.write_u64(u64::from(need.to_bits()));
            }
            fp.write_u64(self.wake_executions);
            fp.write_u64(self.arrivals);
            fp.write_u64(self.stepped);
        }
    }

    /// The campaign driver: builds the routes, states, and spine;
    /// arms NPC 1's first wake; runs `ticks` fixed ticks, each one
    /// spine-first then batch (the Full-tier order: the wake may
    /// retarget mid-tick, the batch moves after); records the
    /// locomotion samples in identity order.
    fn run_campaign(ticks: u64) -> (Vec<LocomotionSample>, WorldSnapshot, Vec<NpcLocomotion>) {
        let loop_route = square_loop();
        let walk_route = Route::new(
            [0.0, 0.0],
            vec![RouteLeg {
                to: [12.0, 0.0],
                turn: 0,
            }],
            4.0,
            0.25,
            2.0,
            100,
            false,
            0,
        )
        .expect("the crosswalk-shaped walk");
        let routes = vec![loop_route.clone(), walk_route.clone()];

        // NPC 0 strolls the loop; NPC 1 starts standing on the loop
        // (the wake retargets it onto route 1); NPC 2 stands (the
        // waiter); NPC 3 is absent (the far tier).
        let mut states = vec![
            start_route(0, &loop_route, 11, 0, LOOPING).expect("the stroll"),
            start_route(0, &loop_route, 21, 0, 0).expect("standing until the wake"),
            start_route(0, &loop_route, 31, 2, 2).expect("the waiter stands at 2"),
            start_route(0, &loop_route, 41, 0, LOOPING).expect("the absent one"),
        ];
        let present = vec![true, true, true, false];
        let before: Vec<_> = states.clone();

        let mut spine = Spine::with_reservation(LocoWorld::new(), 32, 16);
        spine
            .schedule(EventSpec {
                class: CLASS_WAKE,
                schedule: Schedule::AtLogicalTime { due: Tick(10) },
                payload: wake_payload(1, 0).expect("the authored index packs"),
                field: Field::Near,
                depends_on: Vec::new(),
            })
            .expect("the first wake arms");

        let mut samples = Vec::new();
        for _ in 0..ticks {
            spine.run_tick();
            // POLICY (the game-side seam this test stands in for):
            // a Fulfill whose commitment carries kind tag 1 walks
            // route 1 to waypoint 1 — a NEW route start_route's (the
            // seed's own stream redraws identically), the CURRENT
            // route set_destination's (no redraw, no phase reset).
            let world = spine.world_mut();
            if world.wake_executions > 0
                && matches!(
                    world.routine.current.kind,
                    ActivityKind::Fulfill { commitment: 0 }
                )
            {
                let state = &mut states[1];
                if state.route == 1 {
                    set_destination(&walk_route, state, 1);
                } else {
                    *state = start_route(1, &walk_route, 21, 0, 1).expect("the walk retargets");
                }
            }
            let summary = advance_batch(&routes, &mut states, &present, 0.125, None, 0.25, 1.0);
            let world = spine.world_mut();
            world.stepped += u64::from(summary.stepped);
            world.arrivals += u64::from(summary.arrivals);
            // The arrival mirror: the committed location goes coarse
            // (NpcRoutine.route_progress ← the standing waypoint).
            if states[1].speed == 0.0 && world.routine.route_progress != states[1].leg {
                world.routine.route_progress = states[1].leg;
            }
            for (i, state) in states.iter().enumerate() {
                if present[i] {
                    samples.push(LocomotionSample {
                        npc: i as u32,
                        position: state.position,
                        facing: state.facing,
                        stride_phase: state.stride_phase,
                        leg: state.leg,
                    });
                }
            }
        }
        // The absent NPC never moved.
        assert_eq!(states[3], before[3], "the far tier is byte-frozen");
        // The world's terminal record, read out through the spine.
        let world = spine.world();
        let snapshot = WorldSnapshot {
            stream: world.stream.clone(),
            wake_executions: world.wake_executions,
            arrivals: world.arrivals,
            stepped: world.stepped,
            routine: world.routine,
        };
        (samples, snapshot, states)
    }

    /// The campaign world's terminal record (run_campaign's readout).
    struct WorldSnapshot {
        stream: Vec<StreamEntry>,
        wake_executions: u64,
        arrivals: u64,
        stepped: u64,
        routine: NpcRoutine,
    }

    /// Test 17: the composition gate — a seeded campaign over a REAL
    /// spine, the commitment-driven MoveTo, the batch, and both
    /// digests byte-identical across two independent constructions.
    #[test]
    fn seeded_campaign_double_run_byte_identical() {
        let (samples_a, world_a, states_a) = run_campaign(200);
        let (samples_b, world_b, states_b) = run_campaign(200);

        assert_eq!(
            locomotion_digest(&samples_a),
            locomotion_digest(&samples_b),
            "the locomotion stream is byte-identical"
        );
        assert_eq!(
            execution_digest(&world_a.stream),
            execution_digest(&world_b.stream),
            "the activity stream is byte-identical"
        );
        assert_eq!(states_a, states_b, "the terminal states are byte-identical");
        assert_eq!(world_a.routine, world_b.routine);

        // Non-vacuous: the wake path, the batch, and the arrival all
        // executed (10 wakes at ticks 10..190; the walk arrives once
        // — later wakes re-target the standing waypoint).
        assert_eq!(
            world_a.wake_executions, 10,
            "one wake per 20-tick occurrence"
        );
        // The stroller walks every tick (200); NPC 1's walk adds
        // 22-27 more (12 units at 0.45-0.55/tick) — any draw's scale
        // keeps the sum above 220.
        assert!(
            world_a.stepped > 220,
            "both movers contributed (stepped: {})",
            world_a.stepped
        );
        assert_eq!(world_a.arrivals, 1, "exactly one destination arrival");
        assert_eq!(world_a.stream.len(), 10);
    }

    /// Test 18: the digest pin — the measured actual, never silently
    /// re-pinned (a divergence is an investigation).
    #[test]
    fn seeded_campaign_locomotion_digest_pin() {
        let (samples, _world, _states) = run_campaign(200);
        let digest = locomotion_digest(&samples);
        // MEASURED on this exact tree (the campaign of test 17, 200
        // ticks, 600 samples): the pin — never silently re-pinned.
        assert_eq!(digest, 852983358677453085, "the locomotion digest pin");
    }

    /// Test 19 (the audit's finding 4 deciding test, part 1): a
    /// VALID tiny stride cannot stall the tick. `Route::new` admits
    /// any positive finite stride; a stride of 2^-30 against a
    /// distance of 1.0 puts the phase at 2^30, where the f32 spacing
    /// is 128 — the pre-corrective repeated `-= 1.0` rounded back to
    /// the same float and the tick NEVER RETURNED. The bounded
    /// reduction (one floor, one subtraction) completes and leaves
    /// the phase in [0, 1).
    #[test]
    fn tiny_stride_tick_completes_phase_bounded() {
        // Exactly 2^-30 (exponent 127−30, unit mantissa).
        let tiny_stride = f32::from_bits(0x3080_0000);
        assert_eq!(tiny_stride, 1.0 / (1024.0 * 1024.0 * 1024.0));
        let route = Route::new(
            [0.0, 0.0],
            vec![RouteLeg {
                to: [64.0, 0.0],
                turn: 0,
            }],
            8.0,
            0.0,
            tiny_stride,
            0,
            false,
            0,
        )
        .expect("a tiny stride is valid authored data");
        let mut state = start_route(0, &route, 1, 0, 1).expect("the walk");
        state.speed_scale = 1.0; // the arithmetic isolation (test 3 pins the draw)
        state.stride_phase = 0.0;
        state.lateral = 0.0;

        // BOUNDED COMPLETION: ten ticks at 1.0 unit each — every
        // tick's phase increment is 1.0/2^-30 = 2^30, a float whose
        // exact fractional part is 0. The loop below returns (the
        // pre-corrective code hung on the FIRST tick's wrap).
        for _ in 0..10 {
            step1(&route, &mut state, 0.125, None);
            assert!(
                (0.0..1.0).contains(&state.stride_phase),
                "the phase is bounded in [0, 1): {}",
                state.stride_phase
            );
        }
        // 2^30's exact fractional part is 0.0 — the reduction's
        // documented answer, and the value the unbounded repeated
        // subtraction would have reached.
        assert_eq!(state.stride_phase, 0.0);
        // The walk itself advanced normally: ten units, on-route.
        assert_eq!(state.leg_distance, 10.0);
        assert_eq!(state.position, [10.0, 0.0]);
        assert_eq!(state.speed, 8.0, "the walk is unaffected by the wrap");

        // The clamp: an increment that overflows f32 (distance 1.0
        // over the smallest subnormal stride, 2^-149 — the exact
        // quotient 2^149 exceeds f32::MAX) is non-finite — clamped
        // to 0.0 and counted fail-visibly.
        let overflow_route = Route::new(
            [0.0, 0.0],
            vec![RouteLeg {
                to: [64.0, 0.0],
                turn: 0,
            }],
            8.0,
            0.0,
            f32::from_bits(1),
            0,
            false,
            0,
        )
        .expect("the smallest subnormal stride is valid authored data");
        let mut state = start_route(0, &overflow_route, 1, 0, 1).expect("the walk");
        state.speed_scale = 1.0;
        state.stride_phase = 0.0;
        let summary = step1(&overflow_route, &mut state, 0.125, None);
        assert_eq!(summary.phase_clamps, 1, "the overflow clamped, counted");
        assert_eq!(state.stride_phase, 0.0, "the clamp's documented value");
        assert_eq!(state.leg_distance, 1.0, "the walk itself is unaffected");

        // THE EQUIVALENCE PIN (byte-identity for ordinary phases):
        // for increments whose phase stays below 2^24, the reduction
        // equals the naive repeated subtraction EXACTLY — the same
        // bytes the pre-corrective wrap produced.
        for stride in [0.5f32, 1.0, 2.0, 7.0] {
            let route = Route::new(
                [0.0, 0.0],
                vec![RouteLeg {
                    to: [64.0, 0.0],
                    turn: 0,
                }],
                8.0,
                0.0,
                stride,
                0,
                false,
                0,
            )
            .expect("the walk");
            let mut state = start_route(0, &route, 1, 0, 1).expect("the walk");
            state.speed_scale = 1.0;
            state.lateral = 0.0;
            state.stride_phase = 0.3;
            // The naive wrap, computed beside the engine (it
            // terminates for these small phases by the same
            // exactness argument the reduction's proof carries).
            let mut naive = 0.3f32;
            for _ in 0..17 {
                step1(&route, &mut state, 0.125, None);
                naive += 1.0 / stride;
                while naive >= 1.0 {
                    naive -= 1.0;
                }
                assert_eq!(
                    state.stride_phase, naive,
                    "stride {stride}: the reduction equals the repeated subtraction"
                );
            }
        }
    }

    /// Test 20 (the audit's finding 4 deciding test, part 2): a tick
    /// crossing 17 unit legs SETTLES at the last admitted crossing,
    /// never extrapolating off the route. MAX_CROSSINGS_PER_TICK is
    /// 16; the 17-crossing tick's excess distance is DROPPED — the
    /// NPC stands at the waypoint the 16th crossing reached, on an
    /// on-route position, and the next tick resumes the walk. The
    /// pre-corrective defect kept the unconsumed distance in the
    /// state and let step 5 extrapolate `leg_from +
    /// leg_distance · unit` past the leg's end by that whole
    /// distance.
    #[test]
    fn crossing_guard_settles_at_last_admitted_crossing() {
        // Fixture A (the audit's literal shape): a 2×2 square LOOP of
        // eight unit legs (perimeter 8), walked at 17.5 units/tick —
        // 17 crossings needed, 16 admitted, the 17th's 1.5 units
        // dropped (17.5 − 16 admitted).
        let square = Route::new(
            [0.0, 0.0],
            vec![
                RouteLeg {
                    to: [1.0, 0.0],
                    turn: 0,
                },
                RouteLeg {
                    to: [2.0, 0.0],
                    turn: 0,
                },
                RouteLeg {
                    to: [2.0, 1.0],
                    turn: 16384,
                },
                RouteLeg {
                    to: [2.0, 2.0],
                    turn: 16384,
                },
                RouteLeg {
                    to: [1.0, 2.0],
                    turn: 32768,
                },
                RouteLeg {
                    to: [0.0, 2.0],
                    turn: 32768,
                },
                RouteLeg {
                    to: [0.0, 1.0],
                    turn: 49152,
                },
                RouteLeg {
                    to: [0.0, 0.0],
                    turn: 49152,
                },
            ],
            140.0,
            0.0,
            1.0,
            0,
            true,
            0,
        )
        .expect("the unit-leg square loop closes exactly");
        let mut state = start_route(0, &square, 1, 0, LOOPING).expect("the stroll");
        state.speed_scale = 1.0; // 140 · 1.0 · 0.125 = 17.5 units/tick, exact
        state.stride_phase = 0.0;
        state.lateral = 0.0;

        let summary = step1(&square, &mut state, 0.125, None);
        // The guard fired, fail-visibly.
        assert_eq!(
            summary.guard_settles, 1,
            "the 17th crossing's excess dropped"
        );
        assert_eq!(summary.arrivals, 0, "a settlement is not an arrival");
        // 16 admitted crossings from leg 0 wrap twice: leg 16 mod 8
        // = 0, the last admitted crossing DEPARTED waypoint 0 — the
        // NPC stands there, on-route, with the state invariant held.
        assert_eq!(state.leg, 0, "the last admitted crossing's leg");
        assert_eq!(state.leg_distance, 0.0, "settled AT the crossing");
        assert_eq!(state.position, [0.0, 0.0], "the waypoint, byte-exact");
        assert_eq!(state.speed, 140.0, "the walk resumes next tick");
        // The phase advanced by the tick's whole distance (17.5
        // strides) and wrapped into [0, 1).
        assert_eq!(state.stride_phase, 0.5);
        assert!((0.0..1.0).contains(&state.stride_phase));
        // The pre-corrective defect's answer, for the record: the
        // position would have extrapolated to (0,0) + 1.5·(1,0) =
        // (1.5, 0) with leg_distance 1.5 ≥ leg_length on leg 0 —
        // off the settled state's invariant, the excess carried.

        // The second tick settles identically (the walk resumes and
        // re-crosses 17 legs).
        let summary = step1(&square, &mut state, 0.125, None);
        assert_eq!(summary.guard_settles, 1);
        assert_eq!((state.leg, state.leg_distance), (0, 0.0));

        // Fixture B (the visibly off-route case): an OPEN staircase
        // of 17 unit legs turning every leg, walked to its end
        // (destination 17) at 17.5 units/tick. Tick 1 admits 16
        // crossings → leg 16 with 1.5 units unconsumed → the guard
        // settles at waypoint 16 = (8, 8). The pre-corrective defect
        // extrapolated to (8,8) + 1.5·(1,0) = (9.5, 8) — half a unit
        // PAST the route's end waypoint (9, 8), off the route.
        let staircase: Vec<RouteLeg> = (0..17u16)
            .map(|i| RouteLeg {
                to: if i % 2 == 0 {
                    [f32::from(i / 2 + 1), f32::from(i / 2)]
                } else {
                    [f32::from(i.div_ceil(2)), f32::from(i.div_ceil(2))]
                },
                turn: if i % 2 == 0 { 0 } else { 16384 },
            })
            .collect();
        let route = Route::new([0.0, 0.0], staircase, 140.0, 0.0, 1.0, 0, false, 0)
            .expect("the 17-leg staircase is valid");
        assert_eq!(route.leg_count(), 17);
        assert_eq!(route.leg_from(16), [8.0, 8.0], "waypoint 16");
        let mut state = start_route(0, &route, 1, 0, 17).expect("walk to the end");
        state.speed_scale = 1.0;
        state.stride_phase = 0.0;
        state.lateral = 0.0;

        let summary = step1(&route, &mut state, 0.125, None);
        assert_eq!(summary.guard_settles, 1);
        assert_eq!(
            summary.arrivals, 0,
            "the end is one crossing past the bound"
        );
        assert_eq!(state.leg, 16, "the last admitted crossing's leg");
        assert_eq!(state.leg_distance, 0.0, "settled AT the crossing");
        assert_eq!(
            state.position,
            [8.0, 8.0],
            "waypoint 16 — on-route, byte-exact"
        );
        assert_eq!(state.speed, 140.0, "the walk resumes next tick");
        assert_eq!(state.stride_phase, 0.5, "17.5 strides wrapped");

        // Tick 2: the resumed walk crosses the final leg and
        // ARRIVES at the authored end waypoint — the settlement
        // deferred, not lost.
        let summary = step1(&route, &mut state, 0.125, None);
        assert_eq!(summary.guard_settles, 0, "17.5 from leg 16 crosses once");
        assert_eq!(summary.arrivals, 1, "the end-waypoint arrival");
        assert_eq!(state.position, [9.0, 8.0], "the authored end bytes");
        assert_eq!(state.speed, 0.0);
        assert_eq!(state.leg, 17, "standing AT the end waypoint");
    }

    /// Test 21 (the audit's finding 4 deciding test, part 3): an
    /// obstruction immediately past a corner rolls the state back as
    /// a (leg, distance) PAIR. The pre-corrective defect restored
    /// only the pre-step distance while KEEPING the crossed leg —
    /// the next position evaluated the old distance on the NEW
    /// leg's unit vector, teleporting the walker off its route (and
    /// past the obstruction) by the whole pre-step distance.
    #[test]
    fn collision_rollback_restores_leg_and_distance_pair() {
        let route = Route::new(
            [0.0, 0.0],
            vec![
                RouteLeg {
                    to: [8.0, 0.0],
                    turn: 0,
                },
                RouteLeg {
                    to: [8.0, 8.0],
                    turn: 16384,
                },
            ],
            2.0,
            0.0,
            1.0,
            100,
            false,
            0,
        )
        .expect("the corner walk");
        // The obstruction: a 1×1-footprint box standing immediately
        // past the corner (x ∈ [7.5, 8.5], z ∈ [0, 1]) — the NPC box
        // (half 0.25) at the corner waypoint (8, 0) overlaps it.
        let world = CollisionWorld::new(0.0).with_box([8.0, 0.0, 0.5], 1.0, 2.0);
        let mut state = NpcLocomotion {
            position: [7.75, 0.0],
            facing: 0,
            route: 0,
            leg: 0,
            leg_distance: 7.75,
            destination: 2,
            speed: 2.0,
            speed_scale: 1.0,
            lateral: 0.0,
            stride_phase: 0.5,
        };

        // The tick crosses the corner (8.0 >= 8.0 → leg 1, remainder
        // 0) and the candidate — the corner waypoint itself — sits in
        // the obstruction: blocked.
        let summary = step1(&route, &mut state, 0.125, Some(&world));
        assert_eq!(summary.blocked, 1);
        // THE PAIR: leg 0 AND distance 7.75 both restored. The
        // pre-corrective state kept leg 1 with distance 7.75 — the
        // next tick would have evaluated (8, 7.75), 7.75 units up
        // the return leg, deep past the obstruction and off the
        // walk.
        assert_eq!(state.leg, 0, "the pre-crossing leg restored");
        assert_eq!(state.leg_distance, 7.75, "the pre-step distance restored");
        // The depenetrated candidate: pushed south out of the box
        // (the minimal-axis push, |3−SKIN−0.25| ≈ 0.25), x still at
        // the corner.
        assert_eq!(state.position[0], 8.0);
        assert!(
            state.position[1] < -0.24 && state.position[1] > -0.26,
            "pushed clear of the obstruction: {}",
            state.position[1]
        );
        // The phase frozen in [0, 1).
        assert_eq!(state.stride_phase, 0.5, "no foot slide");
        assert!((0.0..1.0).contains(&state.stride_phase));

        // Idempotence: repeated blocked ticks recompute the
        // identical crossing, candidate, and push — the
        // pinned-blocked shape held AT the corner.
        let pinned = state;
        for _ in 0..5 {
            step1(&route, &mut state, 0.125, Some(&world));
        }
        assert_eq!(state.position, pinned.position, "pinned at the corner");
        assert_eq!(state.leg, 0, "the pair holds");
        assert_eq!(state.leg_distance, 7.75);
        assert_eq!(state.stride_phase, 0.5, "still frozen");
        assert_eq!(state.speed, 2.0, "the walk resumes when the path clears");

        // The counterfactual: the same tick without the obstruction
        // completes the crossing on-route.
        let mut free = NpcLocomotion {
            position: [7.75, 0.0],
            facing: 0,
            route: 0,
            leg: 0,
            leg_distance: 7.75,
            destination: 2,
            speed: 2.0,
            speed_scale: 1.0,
            lateral: 0.0,
            stride_phase: 0.5,
        };
        step1(&route, &mut free, 0.125, None);
        assert_eq!(free.leg, 1, "the crossing completes");
        assert_eq!(free.leg_distance, 0.0);
        assert_eq!(free.position, [8.0, 0.0], "the corner waypoint, on-route");
        assert_eq!(free.stride_phase, 0.75, "0.5 + 0.25 — the stride advanced");
    }
}
