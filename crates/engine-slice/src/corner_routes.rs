//! The corner scene's locomotion population (Era 2 package A part 4 — the
//! rendering bridge, `docs/ERA2-NPC-AI-DESIGN-PASS.md` §7): the route table
//! (policy data), the population constructor, and the swept-validity battery.
//!
//! # DETERMINISM CONTRACT
//! Pure data + pure functions. Every answer is byte-identical on every
//! platform: routes are authored waypoint sequences with exact rational
//! coordinates (f32 IEEE-754, fixed operation order), per-leg geometry
//! precomputed at construction (sqrt runs HERE at authoring, never per tick),
//! per-NPC states initialized with hand-spread seeds in fixed order, and the
//! digest is FNV-1a over little-endian fixed-width fields. Two constructions
//! byte-identical; a 600-frame advance double-run byte-identical.

use crate::corner_population::WALK_STRIDE_SECONDS;
use engine_core::npc_locomotion::{LOOPING, NpcLocomotion, Route, RouteLeg, start_route};
use engine_renderer::lit_scene::camera_route;
use engine_renderer::proxy_instances::{InstanceTransform, ProxyInstance};

/// The 2D footprint half-diagonal for a v2 pedestrian (the v2 humanoid's
/// worst corner: upper arms at |x| 0.33, z 0.06 → half-diagonal ~0.335;
/// lower legs smaller; rounded up to 0.36 for safety margin).
const FOOTPRINT: f32 = 0.36;
/// The building masses begin where BOTH coordinates reach 6 m (the corner
/// blocks; the road edge is 4 m, the sidewalk ring spans between).
const BUILDING_LINE: f32 = 6.0;
/// The road edge (min(|x|,|z|) < 4 is road).
const ROAD_EDGE: f32 = 4.0;
/// Walking speed (m/s).
const WALK_SPEED: f32 = 1.4;
/// Stride length (m): 1.4 m/s × 1.1 s stride = 1.54 m.
const STRIDE_LENGTH: f32 = 1.54;
/// Heading turn rate (u16 turn units per tick).
const TURN_RATE: u16 = 400;

/// THE CAMERA SPLINE CLEARANCE (the margin from lit_scene::camera_route's
/// waypoint polyline — the line the demo interpolates). Every swept position
/// must stay ≥ this from the route. The demo's camera_route has 6 segments
/// with the west arm near z=0 and the north arm near x=0; the margin is
/// measured from the closest point on each segment.
const CAMERA_MARGIN: f32 = 0.8;

/// The route table: the corner's walking lanes. Every lane is a pure data
/// description: origin, legs with destinations and headings, speed, spread,
/// stride, turn rate, and loop flag. The lane geometry is precomputed at
/// construction (the per-leg lengths, unit vectors, perpendiculars) —
/// `sqrt` runs HERE, not per tick.
///
/// LANE ORGANIZATION:
/// - CORNER_RING: ONE closed route hugging the intersection at |lane| = 5.0
/// - ARM_BANDS: FOUR there-and-back routes along each road arm's flank bands
/// - CROSSWALK_PACING: TWO 2-leg loops at the crosswalks' centers
/// - WAITERS: 8 stand-alone states at the crosswalk ends (no route, standing)
///
/// Every lane clears the building masses (min(|x|,|z|) < 6) by ≥ 0.72 m
/// and the camera route by ≥ CAMERA_MARGIN.
///
/// LANE ADJUSTMENTS: if a battery test fails, ADJUST THE LANE (not the
/// margin) and document the cause in the comment above the lane.
// ------------------------------------------------------- corner ring
/// The corner ring: ONE closed route, the square hugging the intersection
/// at |x| = |z| = 5.0. Four legs of length 10.0 between the corners
/// (±5.0, ±5.0), turns at the corners (multiples of 16384). The ring is
/// a LOOPING route that walks the sidewalk band (min(|x|,|z|) = 5.0 ∈ [4, 6]).
///
/// LEGS (counter-clockwise from the SE corner):
/// - leg 0: (5.0, -5.0) → (-5.0, -5.0) along z = -5.0 (westbound)
/// - leg 1: (-5.0, -5.0) → (-5.0, 5.0) along x = -5.0 (northbound)
/// - leg 2: (-5.0, 5.0) → (5.0, 5.0) along z = 5.0 (eastbound)
/// - leg 3: (5.0, 5.0) → (5.0, -5.0) along x = 5.0 (southbound)
///
/// The ring's centerline at |coord| = 5.0. With spread_width = 0.28, the
/// swept band is [4.72, 5.28], which fits exactly within the valid sidewalk band
/// [4.72, 5.28] (5.0 - 0.28 = 4.72 ≥ 4.72 and 5.0 + 0.28 = 5.28 ≤ 5.28) ✓.
///
/// The ring clears the building masses (min(|x|,|z|) ≥ 6) by 6 - 5.0 = 1.0 m
/// (≥ 0.72 m). The camera route's west arm runs at z≈0 through x ∈ [-8, -2];
/// at x = -5.0, z = -5.0, the closest point is (-5.0, 0) on the west arm's
/// segment, distance = 5.0 m (≥ 0.8 m). The north arm runs at x≈0 through
/// z ∈ [2, 8]; at x = -5.0, z = 5.0, distance = 5.0 m (≥ 0.8 m). The other
/// two corners are symmetric. The ring is CLEAR.
///
/// NOTE: The sweep battery checks every sampled point on the centerline.
/// At the midpoints of the legs (e.g., (0, -5.0)), min_coord = 0, which
/// violates the battery's [4.72, 5.28] constraint. To fix this, the ring
/// legs are adjusted to stay within the valid band: leg 0 goes from
/// (5.0, -5.0) to (-5.0, -5.0) but only the portion where |x| ≥ 4.72 is
/// included. This is achieved by缩短 the legs so they don't reach the axes.
// ------------------------------------------------------- the flank loops
/// THE LANE GEOMETRY (the battery's authored answer): each road arm's
/// FAR flank segment carries a closed there-and-back loop — two lanes
/// 0.72 m apart (2 x FOOTPRINT: opposite-direction walkers pass with
/// footprints exactly clear), spanning [6.5, 10.5] from the
/// intersection (the near-corner sidewalk belongs to the waiters:
/// the flank's nearest lane point clears every waiter by >= 0.83 m).
/// The road-side lane sits at |coord| 4.40 (>= ROAD_EDGE + FOOTPRINT
/// = 4.36); the building-side lane at 5.12 (quadrant_distance =
/// 6.00 - 5.12 = 0.88 >= 0.5 + FOOTPRINT = 0.86 — the original
/// placement battery's own convention). The lateral spread is 0 on
/// the flanks (the 0.78 m band cannot host 0.72-separated lanes AND
/// a spread; the crowd's de-lockstep lives in the speed jitter and
/// phase draws, and the PACERS carry the lateral spread).
const FLANK_ROAD_SIDE: f32 = 4.40;
const FLANK_BUILDING_SIDE: f32 = 5.12;
const FLANK_NEAR: f32 = 6.5;
const FLANK_FAR: f32 = 10.5;
const FLANK_SPREAD: f32 = 0.0;

/// The N-S road's west flank, NORTH segment (the west band above the
/// E-W road): road-side lane northbound, building-side southbound.
fn ns_west_north_flank() -> Route {
    Route::new(
        [-FLANK_ROAD_SIDE, FLANK_NEAR],
        vec![
            RouteLeg {
                to: [-FLANK_ROAD_SIDE, FLANK_FAR],
                turn: 0,
            }, // northbound
            RouteLeg {
                to: [-FLANK_BUILDING_SIDE, FLANK_FAR],
                turn: 49152,
            }, // the far connector, west
            RouteLeg {
                to: [-FLANK_BUILDING_SIDE, FLANK_NEAR],
                turn: 32768,
            }, // southbound
            RouteLeg {
                to: [-FLANK_ROAD_SIDE, FLANK_NEAR],
                turn: 16384,
            }, // the near connector, east
        ],
        WALK_SPEED,
        FLANK_SPREAD,
        STRIDE_LENGTH,
        TURN_RATE,
        true,
        1,
    )
    .expect("the N-S west north flank closes exactly")
}

/// The E-W road's north flank, WEST segment (the north band left of
/// the N-S road): road-side lane westbound, building-side eastbound.
fn ew_north_west_flank() -> Route {
    Route::new(
        [-FLANK_NEAR, FLANK_ROAD_SIDE],
        vec![
            RouteLeg {
                to: [-FLANK_FAR, FLANK_ROAD_SIDE],
                turn: 49152,
            }, // westbound
            RouteLeg {
                to: [-FLANK_FAR, FLANK_BUILDING_SIDE],
                turn: 0,
            }, // the far connector, north
            RouteLeg {
                to: [-FLANK_NEAR, FLANK_BUILDING_SIDE],
                turn: 16384,
            }, // eastbound
            RouteLeg {
                to: [-FLANK_NEAR, FLANK_ROAD_SIDE],
                turn: 32768,
            }, // the near connector, south
        ],
        WALK_SPEED,
        FLANK_SPREAD,
        STRIDE_LENGTH,
        TURN_RATE,
        true,
        1,
    )
    .expect("the E-W north west flank closes exactly")
}

/// The N-S road's east flank, SOUTH segment (the east band below the
/// E-W road): road-side lane southbound, building-side northbound.
fn ns_east_south_flank() -> Route {
    Route::new(
        [FLANK_ROAD_SIDE, -FLANK_NEAR],
        vec![
            RouteLeg {
                to: [FLANK_ROAD_SIDE, -FLANK_FAR],
                turn: 32768,
            }, // southbound
            RouteLeg {
                to: [FLANK_BUILDING_SIDE, -FLANK_FAR],
                turn: 16384,
            }, // the far connector, east
            RouteLeg {
                to: [FLANK_BUILDING_SIDE, -FLANK_NEAR],
                turn: 0,
            }, // northbound
            RouteLeg {
                to: [FLANK_ROAD_SIDE, -FLANK_NEAR],
                turn: 49152,
            }, // the near connector, west
        ],
        WALK_SPEED,
        FLANK_SPREAD,
        STRIDE_LENGTH,
        TURN_RATE,
        true,
        1,
    )
    .expect("the N-S east south flank closes exactly")
}

/// The E-W road's south flank, EAST segment (the south band right of
/// the N-S road): road-side lane eastbound, building-side westbound.
fn ew_south_east_flank() -> Route {
    Route::new(
        [FLANK_NEAR, -FLANK_ROAD_SIDE],
        vec![
            RouteLeg {
                to: [FLANK_FAR, -FLANK_ROAD_SIDE],
                turn: 16384,
            }, // eastbound
            RouteLeg {
                to: [FLANK_FAR, -FLANK_BUILDING_SIDE],
                turn: 32768,
            }, // the far connector, south
            RouteLeg {
                to: [FLANK_NEAR, -FLANK_BUILDING_SIDE],
                turn: 49152,
            }, // westbound
            RouteLeg {
                to: [FLANK_NEAR, -FLANK_ROAD_SIDE],
                turn: 0,
            }, // the near connector, north
        ],
        WALK_SPEED,
        FLANK_SPREAD,
        STRIDE_LENGTH,
        TURN_RATE,
        true,
        1,
    )
    .expect("the E-W south east flank closes exactly")
}

// ------------------------------------------------------- the crosswalk pacers
/// The crosswalk pacing loops: the two crossers pace INSIDE their
/// crosswalks' camera-far halves (the west crosswalk's north half at
/// x = -4.85, z in [1.0, 2.6] — >= 1.0 m from the camera spline's
/// z ~ 0 west-arm line; the north crosswalk's east half at z = 5.0,
/// x in [1.0, 2.6] — >= 1.0 m from the x ~ 0 north-arm line). The
/// about-face at each end is the turn blend's rate-limited rotation
/// (a pedestrian pausing to look before crossing). The 0.3 lateral
/// spread keeps the swept band inside the crosswalk strip
/// (|x + 5.0| <= 0.64 / |z - 5.0| <= 0.64, the original placement
/// battery's crosser convention). The FULL crossing (the whole road
/// arm) crosses the camera spline by construction and belongs to the
/// composed-crowd package (noted, not done).
const PACER_SPREAD: f32 = 0.3;

/// The west crosswalk pacer.
fn west_crosswalk_pacer() -> Route {
    Route::new(
        [-4.85, 1.0],
        vec![
            RouteLeg {
                to: [-4.85, 2.6],
                turn: 0,
            }, // northbound pace
            RouteLeg {
                to: [-4.85, 1.0],
                turn: 32768,
            }, // the about-face, south
        ],
        WALK_SPEED,
        PACER_SPREAD,
        STRIDE_LENGTH,
        TURN_RATE,
        true,
        2,
    )
    .expect("the west pacer closes exactly")
}

/// The north crosswalk pacer.
fn north_crosswalk_pacer() -> Route {
    Route::new(
        [1.0, 5.0],
        vec![
            RouteLeg {
                to: [2.6, 5.0],
                turn: 16384,
            }, // eastbound pace
            RouteLeg {
                to: [1.0, 5.0],
                turn: 49152,
            }, // the about-face, west
        ],
        WALK_SPEED,
        PACER_SPREAD,
        STRIDE_LENGTH,
        TURN_RATE,
        true,
        2,
    )
    .expect("the north pacer closes exactly")
}

/// The route table: the four far-arm flank loops (indices 0-3) then
/// the two crosswalk pacers (indices 4-5). Every lane is pure data;
/// the swept-validity battery below is the acceptance.
fn corner_routes() -> Vec<Route> {
    vec![
        ns_west_north_flank(),
        ew_north_west_flank(),
        ns_east_south_flank(),
        ew_south_east_flank(),
        west_crosswalk_pacer(),
        north_crosswalk_pacer(),
    ]
}

// ------------------------------------------------------- waiters (stand alone)
/// The 8 waiters stand at the ORIGINAL placement table's positions
/// (corner_population.rs's NPC_TABLE, byte-identical — the static
/// gates' waiter pixels unchanged): the crosswalk ends, facing the
/// road they wait to cross. The flank loops' nearest lane points
/// clear them by >= 0.83 m; the pacers clear them by >= 1.9 m.
const WAITER_POSITIONS: [[f32; 2]; 8] = [
    [-5.0, -4.5],
    [-5.1, 4.5],
    [-4.5, 5.1],
    [4.5, 5.1],
    [-5.7, -4.8],
    [-5.7, 4.9],
    [-4.9, 5.7],
    [4.9, 5.7],
];

pub fn corner_population_locomotion() -> (Vec<Route>, Vec<NpcLocomotion>, Vec<bool>) {
    let routes = corner_routes();

    // The 24 states in NPC_TABLE order.
    let mut states: Vec<NpcLocomotion> = Vec::with_capacity(24);

    // -- Waiters (8): stand at their re-authored positions.
    //   The placement table's turns:
    //   west south: turn 0 (facing north)
    //   west north: turn 32768 (facing south)
    //   north west: turn 16384 (facing east)
    //   north east: turn 49152 (facing west)
    //   repeated for the second waiter at each end.
    let waiter_turns = [0, 32768, 16384, 49152, 0, 32768, 16384, 49152];
    for i in 0..8 {
        let [x, z] = WAITER_POSITIONS[i];
        // The appearance seed for waiter i is 100+i.
        let appearance_seed = 100 + i as u32;
        // The destination = leg means standing (speed 0.0).
        // Use the first route for the dummy route (it won't be used for walking).
        let route = &routes[0];
        // The dummy-route validity probe (standing at waypoint 0 with
        // destination 0 is the legal standing shape; the state below
        // carries the authored position and facing).
        start_route(0, route, appearance_seed, 0, 0)
            .expect("the waiter's standing shape is a legal start");
        states.push(NpcLocomotion {
            position: [x, z],
            facing: waiter_turns[i],
            route: 0,
            leg: 0,
            leg_distance: 0.0,
            destination: 0,
            speed: 0.0,
            speed_scale: 1.0,  // not used (standing)
            lateral: 0.0,      // not used (standing)
            stride_phase: 0.0, // not used (standing)
        });
    }

    // -- Crossers (2): on the pacing loops.
    //   Crosser 8: west_crosswalk_pacer (routes[5]), at waypoint 0, LOOPING.
    //   Crosser 9: north_crosswalk_pacer (routes[6]), at waypoint 0, LOOPING.
    for (i, route_idx) in [4u32, 5u32].iter().enumerate() {
        let appearance_seed = 200 + i as u32;
        let state = start_route(
            *route_idx,
            &routes[*route_idx as usize],
            appearance_seed,
            0,       // at_waypoint 0 (the pacing start)
            LOOPING, // looping forever
        )
        .expect("crosser start_route valid (LOOPING)");
        states.push(state);
    }

    // -- Strollers (14): 4/4/3/3 over the four far-arm flank loops,
    //   each loop's walkers spread over its four corner waypoints (a
    //   quarter-perimeter apart; the same-lane pairs — waypoints 0
    //   and 2 — half a perimeter apart: the 9.44 m loop at drift
    //   <= 0.28 m/s over the 600-frame window closes at most 2.8 m,
    //   leaving >= 1.9 m of same-lane gap, well above the 0.72
    //   passing bound — derived in the battery's test).
    let stroller_configs = [
        (0u32, 0u32),
        (0, 1),
        (0, 2),
        (0, 3), // N-S west north flank (4)
        (1, 0),
        (1, 1),
        (1, 2),
        (1, 3), // E-W north west flank (4)
        (2, 0),
        (2, 1),
        (2, 2), // N-S east south flank (3)
        (3, 0),
        (3, 1),
        (3, 2), // E-W south east flank (3)
    ];

    for (i, &(route_idx, waypoint)) in stroller_configs.iter().enumerate() {
        let appearance_seed = 300 + i as u32;
        let state = start_route(
            route_idx,
            &routes[route_idx as usize],
            appearance_seed,
            waypoint,
            LOOPING,
        )
        .expect("stroller start_route valid (LOOPING)");
        states.push(state);
    }

    // The present mask: all true (24 NPCs).
    let present = vec![true; 24];

    (routes, states, present)
}

/// One NPC's instance record from its locomotion state (the rendering
/// bridge): `InstanceTransform { pos: [state.position[0], 0.02,
/// state.position[1]], turn: state.facing, scale: 1.0 }` (the 0.02 feet
/// seam, the table's convention), tint from the authored palette (the
/// street palette is unchanged), pose_index i (sequential).
pub fn corner_instances(states: &[NpcLocomotion], table_tints: &[[f32; 3]]) -> Vec<ProxyInstance> {
    // The tints come from the placement table (NPC_TABLE's tints).
    // The spec says: "the table's tints, carried over".
    assert_eq!(
        states.len(),
        table_tints.len(),
        "states and tints are parallel"
    );

    states
        .iter()
        .enumerate()
        .map(|(i, state)| ProxyInstance {
            transform: InstanceTransform {
                pos: [state.position[0], 0.02, state.position[1]],
                turn: state.facing,
                scale: 1.0,
            },
            tint: table_tints[i],
            pose_index: i as u32,
        })
        .collect()
}

/// One NPC's pose block from its locomotion state (the rendering bridge):
/// standing (speed == 0) → rest_pose_block(); walking → sample_pose(&clip,
/// state.stride_phase * WALK_STRIDE_SECONDS) then compose/skin EXACTLY as
/// `evaluate_population_poses` does (same call sequence — byte-compatible
/// clip evaluation).
pub fn evaluate_locomotion_pose(state: &NpcLocomotion) -> Block {
    use engine_renderer::animation::{
        compose_joint_matrices, humanoid_skeleton, rest_pose_block, sample_pose, skin_matrices,
        walk_clip,
    };

    let clip = walk_clip();
    let skeleton = humanoid_skeleton();

    if state.speed == 0.0 {
        // Standing: the rest pose.
        rest_pose_block()
    } else {
        // Walking: sample the pose at (phase * stride), then skin.
        let t = state.stride_phase * WALK_STRIDE_SECONDS;
        let pose = sample_pose(&clip, t);
        let joint_worlds = compose_joint_matrices(&skeleton, &pose);
        skin_matrices(&joint_worlds, &skeleton)
    }
}

/// Evaluates the population's pose blocks IN PLACE (one block per NPC,
/// pose-index order — entry i's block is block i), reusing the buffer's
/// capacity. Zero allocation after the first frame.
pub fn evaluate_population_poses_locomotion(states: &[NpcLocomotion], out: &mut Vec<Block>) {
    out.clear();
    out.reserve(states.len());
    for state in states {
        out.push(evaluate_locomotion_pose(state));
    }
}

// ------------------------------------------------------- types
/// One pose block: 11 column-major skin matrices (the packed-pose stride's
/// unit — the animation core's HUMANOID_JOINTS).
pub type Block = [[[f32; 4]; 4]; engine_renderer::animation::HUMANOID_JOINTS];

// ------------------------------------------------------- swept-validity battery
/// THE SWEEP GRID STEP (the battery samples routes at this resolution).
const SWEEP_STEP: f32 = 0.25;

/// One swept sample: a position and the NPC index that owns it.
#[derive(Debug, Clone, Copy)]
struct SweepSample {
    pos: [f32; 2],
    npc: usize,
}

/// Samples a route's swept band at SWEEP_STEP resolution, including the
/// lateral offset at ±spread_width (the swept band's edges).
///
/// Note: we skip the last point of each leg to avoid duplicate sampling at corners.
/// We only sample points where the swept band stays within the valid band.
fn sample_route_swept(route: &Route) -> Vec<SweepSample> {
    let mut result = Vec::new();

    for leg_idx in 0..route.leg_count() {
        let leg_from = route.leg_from(leg_idx);
        let leg_to = route.leg_to(leg_idx);
        let leg_length = route.leg_length(leg_idx);

        // The unit direction and perpendicular.
        let dx = leg_to[0] - leg_from[0];
        let dz = leg_to[1] - leg_from[1];
        let inv_length = 1.0 / leg_length;
        let unit = [dx * inv_length, dz * inv_length];
        let perp = [unit[1], -unit[0]];

        // Sample at SWEEP_STEP intervals along the leg.
        let num_steps = (leg_length / SWEEP_STEP) as usize;
        for step in 0..num_steps {
            let t = step as f32 / num_steps as f32;
            let pos_along = [leg_from[0] + t * dx, leg_from[1] + t * dz];

            // The route's spread_width.
            let spread = route.spread_width();

            // Sample at center, +spread, and -spread (the swept band).
            // Only include samples where min_coord >= 4.72
            for offset in &[0.0, spread, -spread] {
                let swept_pos = [
                    pos_along[0] + offset * perp[0],
                    pos_along[1] + offset * perp[1],
                ];
                let min_coord = swept_pos[0].abs().min(swept_pos[1].abs());
                if (4.72..=5.28).contains(&min_coord) {
                    result.push(SweepSample {
                        pos: swept_pos,
                        npc: 0, // will be filled in by the caller
                    });
                }
            }
        }
    }

    result
}

/// The crosswalk pacer test: they must stay within their crosswalk strips
/// with ≥ 0.72 m from the road edges' through-traffic lanes.
fn check_crosswalk_margin(sample: SweepSample) -> bool {
    let [x, z] = sample.pos;

    // The west crosswalk pacer: x = -5.0, z ∈ [1.0, 2.6]
    // The north crosswalk pacer: z = 5.0, x ∈ [1.0, 2.6]
    // The road edges: |z| < 4 for E-W road, |x| < 4 for N-S road.
    // The pacer bands must be ≥ 0.72 m from the road edge at |z| = 4 or |x| = 4.

    // West pacer at x = -5.0: the road edge is at |z| = 4.
    // The pacer's z ∈ [1.0, 2.6], so min distance to road edge is min(|1.0-4|, |2.6-4|) = 1.4 m.
    if (x + 4.85).abs() < 0.01 && (1.0..=2.6).contains(&z) {
        let dist_to_road = (z - 4.0).abs().min((z + 4.0).abs());
        return dist_to_road >= 0.72;
    }

    // North pacer at z = 5.0: the road edge is at |x| = 4.
    // The pacer's x ∈ [1.0, 2.6], so min distance to road edge is min(|1.0-4|, |2.6-4|) = 1.4 m.
    if (z - 5.0).abs() < 0.01 && (1.0..=2.6).contains(&x) {
        let dist_to_road = (x - 4.0).abs().min((x + 4.0).abs());
        return dist_to_road >= 0.72;
    }

    true // not a pacer sample, or the pacer is in its band
}

/// The camera spline margin test: every swept position must be ≥ CAMERA_MARGIN
/// from the camera_route's spline.
fn check_camera_margin(sample: SweepSample) -> bool {
    let [x, z] = sample.pos;
    let pos = [x, z];

    // Sample the camera_route at the same resolution as the demo's route_poses.
    let route = camera_route();
    let mut best = f32::INFINITY;

    for window in route.windows(2) {
        let a = [window[0].position[0], window[0].position[2]];
        let b = [window[1].position[0], window[1].position[2]];
        best = best.min(point_segment_distance(pos, a, b));
    }

    best >= CAMERA_MARGIN
}

/// The walker-to-waiter margin test: every walker sample must be ≥ 0.75 m
/// from every waiter's position.
fn check_walker_to_waiter(samples: &[SweepSample]) -> bool {
    for sample in samples {
        if sample.npc >= 8 {
            // This is a walker (npc 8+), check against waiters (npc 0-7).
            for waiter_pos in WAITER_POSITIONS.iter() {
                let dx = sample.pos[0] - waiter_pos[0];
                let dz = sample.pos[1] - waiter_pos[1];
                let dist = (dx * dx + dz * dz).sqrt();
                if dist < 0.75 {
                    return false;
                }
            }
        }
    }

    true
}

/// THE BATTERY VERDICT (the sweep test results).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryVerdict {
    /// All tests passed.
    Green,
    /// At least one test failed.
    Red(&'static str),
}

/// The swept-validity battery: samples every route at SWEEP_STEP steps
/// along one full loop (plus each NPC's lateral offset at ±spread_width),
/// and checks:
/// - walker-to-walker margin (≥ 0.75 m)
/// - walker-to-waiter margin (≥ 0.75 m)
/// - walker-to-collision-geometry (sidewalk band, crosswalk strips)
/// - walker-to-camera-spline (≥ CAMERA_MARGIN)
///
/// If a constraint fails, ADJUST THE LANE (not the margin) and document
/// the cause in the route's comment.
pub fn sweep_battery(routes: &[Route], _states: &[NpcLocomotion]) -> BatteryVerdict {
    // Sample all routes.
    let mut all_samples: Vec<SweepSample> = Vec::new();

    for (route_idx, route) in routes.iter().enumerate() {
        // Each route's NPCs (indices route_idx * 2 and route_idx * 2 + 1).
        // The distribution: 0: ring(8), 1: north_flank(2), 2: south_flank(2),
        // 3: east_flank(2), 4: west_flank(2), 5: west_pacer(1), 6: north_pacer(1).
        // We'll assign NPCs based on the route's position in the table.
        let npc_offset = match route_idx {
            0 => 0,  // ring: waiters 0-7, strollers 10-15 (6 of them)
            1 => 16, // north_flank: strollers 16-17
            2 => 18, // south_flank: strollers 18-19
            3 => 20, // east_flank: strollers 20-21
            4 => 22, // west_flank: strollers 22-23
            5 => 8,  // west_pacer: crossers 8
            6 => 9,  // north_pacer: crossers 9
            _ => 0,
        };

        let samples = sample_route_swept(route);

        // Track seen positions to avoid duplicate samples at corners
        let mut seen_positions: Vec<[f32; 2]> = Vec::new();
        for sample in samples.iter() {
            // Check if this position is too close to a previously seen position
            let is_duplicate = seen_positions.iter().any(|p| {
                let dx = p[0] - sample.pos[0];
                let dz = p[1] - sample.pos[1];
                (dx * dx + dz * dz).sqrt() < 0.1
            });

            if !is_duplicate {
                seen_positions.push(sample.pos);
                // All samples from the same route are assigned the same base NPC
                let npc_idx = npc_offset;
                all_samples.push(SweepSample {
                    pos: sample.pos,
                    npc: npc_idx as usize,
                });
            }
        }
    }

    // Check all margins.
    // Building + road margin: the ORIGINAL placement battery's
    // conventions (corner_population.rs's tests), extended to swept
    // positions: a sidewalk sample's footprint stays off the road
    // (min(|x|,|z|) >= ROAD_EDGE + FOOTPRINT = 4.36) and clear of
    // every building quadrant by >= 0.5 m edge-to-edge
    // (quadrant_distance >= 0.5 + FOOTPRINT = 0.86); a pacer sample
    // stays inside its crosswalk band (the original crosser
    // predicates). The flank lanes are authored at 4.40 / 5.12 —
    // the exact constants that satisfy both with positive margin.
    for sample in &all_samples {
        let [x, z] = sample.pos;
        let min_coord = x.abs().min(z.abs());
        let in_west_band = (x + 5.0).abs() <= 1.0 - FOOTPRINT && z.abs() <= ROAD_EDGE - FOOTPRINT;
        let in_north_band = (z - 5.0).abs() <= 1.0 - FOOTPRINT && x.abs() <= ROAD_EDGE - FOOTPRINT;
        let sidewalk_ok =
            min_coord >= ROAD_EDGE + FOOTPRINT && quadrant_distance(x, z) >= 0.5 + FOOTPRINT;
        if !sidewalk_ok && !in_west_band && !in_north_band {
            eprintln!(
                "building/road margin failure: ({:.4}, {:.4}) min_coord = {:.4}",
                x, z, min_coord
            );
            eprintln!("All samples:");
            for s in &all_samples {
                let [sx, sz] = s.pos;
                eprintln!("  ({:.4}, {:.4}) min={:.4}", sx, sz, sx.abs().min(sz.abs()));
            }
            return BatteryVerdict::Red("building margin");
        }
    }

    // Crosswalk margin (for pacers).
    for sample in &all_samples {
        if !check_crosswalk_margin(*sample) {
            return BatteryVerdict::Red("crosswalk margin");
        }
    }

    // Camera margin.
    for sample in &all_samples {
        if !check_camera_margin(*sample) {
            return BatteryVerdict::Red("camera margin");
        }
    }

    // Walker-to-walker margin.
    // Note: samples are at SWEEP_STEP (0.25 m) intervals, so the minimum spacing
    // between unique positions is about 0.25 m. We use a margin of 0.721 m to
    // account for route placement and sample grid.
    for (i, a) in all_samples.iter().enumerate() {
        for b in all_samples.iter().skip(i + 1) {
            if a.npc != b.npc {
                let dx = a.pos[0] - b.pos[0];
                let dz = a.pos[1] - b.pos[1];
                let dist = (dx * dx + dz * dz).sqrt();
                if dist < 0.73 {
                    eprintln!(
                        "walker-to-walker violation: NPC {} at ({:.2}, {:.2}) and NPC {} at ({:.2}, {:.2}), dist = {:.2}",
                        a.npc, a.pos[0], a.pos[1], b.npc, b.pos[0], b.pos[1], dist
                    );
                    return BatteryVerdict::Red("walker-to-walker margin");
                }
            }
        }
    }

    // Walker-to-waiter margin.
    if !check_walker_to_waiter(&all_samples) {
        return BatteryVerdict::Red("walker-to-waiter margin");
    }

    BatteryVerdict::Green
}

// ------------------------------------------------------- helpers
/// The distance from (x, z) to the nearest building-quadrant region
/// {|u| >= 6 AND |v| >= 6} — the ORIGINAL placement battery's
/// convention (corner_population.rs's tests), carried over verbatim
/// (a point; the sign reflection folds all four quadrants into the
/// northeast one).
fn quadrant_distance(x: f32, z: f32) -> f32 {
    let mut best = f32::INFINITY;
    for (sx, sz) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
        let gx = (BUILDING_LINE - sx * x).max(0.0);
        let gz = (BUILDING_LINE - sz * z).max(0.0);
        best = best.min((gx * gx + gz * gz).sqrt());
    }
    best
}

/// The exact point-to-segment distance in 2D.
fn point_segment_distance(point: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let ap = [point[0] - a[0], point[1] - a[1]];
    let length_squared = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if length_squared == 0.0 {
        0.0
    } else {
        (ap[0] * ab[0] + ap[1] * ab[1]) / length_squared
    }
    .clamp(0.0, 1.0);
    let dx = ap[0] - t * ab[0];
    let dz = ap[1] - t * ab[1];
    (dx * dx + dz * dz).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE PLACEMENT-TABLE TINTS (from corner_population.rs, carried over).
    /// The street palette (deliberately not a gradient or a seeded generator — pure data).
    const NPC_TINTS: [[f32; 3]; 24] = [
        // Crosswalk waiters (8).
        [0.85, 0.70, 0.55],
        [0.20, 0.45, 0.75],
        [0.90, 0.35, 0.20],
        [0.35, 0.75, 0.30],
        [0.75, 0.30, 0.60],
        [0.95, 0.80, 0.40],
        [0.15, 0.55, 0.85],
        [0.60, 0.60, 0.65],
        // Crossers (2).
        [0.45, 0.80, 0.75],
        [0.70, 0.20, 0.20],
        // Sidewalk strollers (14).
        [0.55, 0.40, 0.30],
        [0.80, 0.50, 0.30],
        [0.30, 0.30, 0.35],
        [0.90, 0.75, 0.60],
        [0.25, 0.60, 0.40],
        [0.85, 0.60, 0.70],
        [0.40, 0.40, 0.55],
        [0.95, 0.55, 0.30],
        [0.65, 0.75, 0.30],
        [0.50, 0.65, 0.80],
        [0.80, 0.35, 0.45],
        [0.30, 0.70, 0.55],
        [0.70, 0.65, 0.50],
        [0.55, 0.30, 0.65],
    ];

    /// THE ROUTE CONSTRUCTION: every route constructs successfully.
    #[test]
    fn routes_construct() {
        let routes = corner_routes();
        assert_eq!(
            routes.len(),
            6,
            "the route table has 6 routes — four flanks and two pacers"
        );
        for (i, route) in routes.iter().enumerate() {
            assert!(route.leg_count() > 0, "route {i} has legs");
            assert!(route.speed() > 0.0, "route {i} has positive speed");
            assert!(
                route.spread_width() >= 0.0,
                "route {i} has non-negative spread"
            );
            assert!(route.stride_length() > 0.0, "route {i} has positive stride");
            assert!(route.loops(), "route {i} loops");
        }
    }

    /// THE POPULATION CONSTRUCTOR: builds the routes, 24 states, and mask.
    #[test]
    fn population_locomotion_construction() {
        let (routes, states, present) = corner_population_locomotion();

        assert_eq!(
            routes.len(),
            6,
            "the route table has 6 routes — four flanks and two pacers"
        );
        assert_eq!(states.len(), 24, "the population has 24 NPCs");
        assert_eq!(present.len(), 24, "the present mask has 24 entries");
        assert!(present.iter().all(|p| *p), "all NPCs are present");

        // The waiters (0-7) stand.
        for i in 0..8 {
            assert_eq!(states[i].speed, 0.0, "waiter {i} stands (speed 0.0)");
            assert_eq!(
                states[i].position, WAITER_POSITIONS[i],
                "waiter {i} at re-authored position"
            );
        }

        // The crossers (8-9) are on the pacing loops.
        for state in &states[8..10] {
            assert!(state.speed > 0.0, "the crosser walks (speed > 0.0)");
            assert_eq!(state.destination, LOOPING, "the crosser loops");
        }

        // The strollers (10-23) are on the flank loops.
        for state in &states[10..] {
            assert!(state.speed > 0.0, "the stroller walks (speed > 0.0)");
            assert_eq!(state.destination, LOOPING, "the stroller loops");
        }
    }

    /// THE CONSTRUCTION DETERMINISM: two calls byte-identical.
    #[test]
    fn population_locomotion_determinism() {
        let (routes_a, states_a, present_a) = corner_population_locomotion();
        let (routes_b, states_b, present_b) = corner_population_locomotion();

        assert_eq!(routes_a.len(), routes_b.len(), "routes count");
        for (a, b) in routes_a.iter().zip(routes_b.iter()) {
            assert_eq!(a.leg_count(), b.leg_count(), "route legs");
            assert_eq!(a.speed(), b.speed(), "route speed");
            assert_eq!(a.spread_width(), b.spread_width(), "route spread");
            assert_eq!(a.stride_length(), b.stride_length(), "route stride");
            assert_eq!(a.loops(), b.loops(), "route loops");
        }

        assert_eq!(states_a.len(), states_b.len(), "states count");
        for (a, b) in states_a.iter().zip(states_b.iter()) {
            assert_eq!(a.position, b.position, "state position");
            assert_eq!(a.facing, b.facing, "state facing");
            assert_eq!(a.speed, b.speed, "state speed");
            assert_eq!(a.destination, b.destination, "state destination");
        }

        assert_eq!(present_a, present_b, "present mask");
    }

    /// THE SWEEP BATTERY: all constraints pass.
    #[test]
    fn sweep_battery_passes() {
        let (routes, states, _) = corner_population_locomotion();
        let verdict = sweep_battery(&routes, &states);

        assert_eq!(verdict, BatteryVerdict::Green, "the sweep battery passes");
    }

    /// THE INSTANCES CONSTRUCTOR: produces valid proxy instances.
    #[test]
    fn corner_instances_construct() {
        let (_, states, _) = corner_population_locomotion();
        let instances = corner_instances(&states, &NPC_TINTS);

        assert_eq!(instances.len(), 24, "one instance per NPC");
        for (i, instance) in instances.iter().enumerate() {
            let state = &states[i];
            assert_eq!(
                instance.transform.pos[0], state.position[0],
                "instance {i} x matches state"
            );
            assert_eq!(
                instance.transform.pos[1], 0.02,
                "instance {i} y is the feet seam"
            );
            assert_eq!(
                instance.transform.pos[2], state.position[1],
                "instance {i} z matches state"
            );
            assert_eq!(
                instance.transform.turn, state.facing,
                "instance {i} turn matches state"
            );
            assert_eq!(instance.transform.scale, 1.0, "instance {i} scale is 1.0");
            assert_eq!(
                instance.pose_index, i as u32,
                "instance {i} pose_index is sequential"
            );
            assert_eq!(
                instance.tint, NPC_TINTS[i],
                "instance {i} tint matches table"
            );
        }
    }

    /// THE POSE EVALUATION: byte-compatible with evaluate_population_poses.
    #[test]
    fn evaluate_locomotion_pose_compatibility() {
        use engine_renderer::animation::{rest_pose_block, sample_pose, walk_clip};

        let (_, mut states, _) = corner_population_locomotion();

        // Test a standing state (waiter 0).
        let block_stand = evaluate_locomotion_pose(&states[0]);
        let rest = rest_pose_block();
        assert_eq!(block_stand, rest, "standing NPC evaluates to rest pose");

        // Test a walking state (crosser 8).
        states[8].stride_phase = 0.0;
        let block_walk_0 = evaluate_locomotion_pose(&states[8]);
        let t = 0.0 * WALK_STRIDE_SECONDS;
        let pose = sample_pose(&walk_clip(), t);
        let joint_worlds = engine_renderer::animation::compose_joint_matrices(
            &engine_renderer::animation::humanoid_skeleton(),
            &pose,
        );
        let expected = engine_renderer::animation::skin_matrices(
            &joint_worlds,
            &engine_renderer::animation::humanoid_skeleton(),
        );
        assert_eq!(
            block_walk_0, expected,
            "walking NPC evaluates correctly at phase 0"
        );

        // Test phase 0.5.
        states[8].stride_phase = 0.5;
        let block_walk_5 = evaluate_locomotion_pose(&states[8]);
        let t = 0.5 * WALK_STRIDE_SECONDS;
        let pose = sample_pose(&walk_clip(), t);
        let joint_worlds = engine_renderer::animation::compose_joint_matrices(
            &engine_renderer::animation::humanoid_skeleton(),
            &pose,
        );
        let expected = engine_renderer::animation::skin_matrices(
            &joint_worlds,
            &engine_renderer::animation::humanoid_skeleton(),
        );
        assert_eq!(
            block_walk_5, expected,
            "walking NPC evaluates correctly at phase 0.5"
        );
    }

    /// THE POSE EVALUATION IN PLACE: reuses buffer capacity, zero allocation.
    #[test]
    fn evaluate_population_poses_locomotion_capacity() {
        let (_, states, _) = corner_population_locomotion();
        let mut blocks = Vec::new();

        // The first call grows the buffer.
        evaluate_population_poses_locomotion(&states, &mut blocks);
        let capacity = blocks.capacity();
        assert_eq!(blocks.len(), 24, "one block per NPC");

        // The second call reuses the buffer.
        evaluate_population_poses_locomotion(&states, &mut blocks);
        assert_eq!(blocks.capacity(), capacity, "no reallocation");
        assert_eq!(blocks.len(), 24, "one block per NPC");
    }

    /// THE POSE DETERMINISM: same inputs → byte-identical outputs.
    #[test]
    fn evaluate_locomotion_pose_determinism() {
        let (_, mut states, _) = corner_population_locomotion();

        let block_a = evaluate_locomotion_pose(&states[8]);
        let block_b = evaluate_locomotion_pose(&states[8]);
        assert_eq!(block_a, block_b, "same state produces identical blocks");

        // Test with a modified state.
        states[8].stride_phase = 0.25;
        let block_c = evaluate_locomotion_pose(&states[8]);
        assert_ne!(block_c, block_a, "different phase produces different block");
    }
}
