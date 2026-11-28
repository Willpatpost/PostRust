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
//! byte-identical; a 663-frame advance (the demo's full window) double-run
//! byte-identical.
//!
//! # THE ROUTE TABLE
//! - THE FLANK LOOPS (EIGHT): both sidewalk bands of each road arm's FAR
//!   segment — closed there-and-backs, two lanes exactly 0.75 m apart.
//! - THE CROSSWALK CROSSERS (TWO): the full crossings of both crosswalks
//!   on the shared clock — the crosser completes its original crossing
//!   (away from the camera's line) while the camera passes the crosswalk
//!   behind it, then crosses the full road arm; the crossing turn-arounds
//!   at ±3.7 clear the standing waiters by the 0.8 walker-waiter margin.
//! - THE WAITERS (8): no route — standing states at the original placement
//!   table's entries, byte-identical.
//!
//! THE CORNER RING IS RETIRED: a closed circuit around the intersection
//! must cross roads, the scene's only crosswalks are the camera's own
//! (west and north), and a camera-safe ring is not a ring — the honest
//! camera-safe crowd is the flanks + crossers + the waiters' standing.

use crate::corner_population::{Block, WALK_STRIDE_SECONDS};
use engine_core::npc_locomotion::{LOOPING, NpcLocomotion, Route, RouteLeg, start_route};
use engine_renderer::lit_scene::camera_route;
use engine_renderer::proxy_instances::{InstanceTransform, ProxyInstance};

/// The 2D footprint half-diagonal for a v2 pedestrian (the v2 humanoid's
/// worst corner: upper arms at |x| 0.33, z 0.06 → half-diagonal ~0.335;
/// lower legs smaller; rounded up to 0.36 for safety margin).
const FOOTPRINT: f32 = 0.36;
/// The building masses begin where BOTH coordinates reach 6 m (the corner
/// blocks; the road edge is 4 m, the sidewalk spans between).
const BUILDING_LINE: f32 = 6.0;
/// The road edge (min(|x|,|z|) < 4 is road).
const ROAD_EDGE: f32 = 4.0;
/// Walking speed (m/s).
const WALK_SPEED: f32 = 1.4;
/// Stride length (m): 1.4 m/s × 1.1 s stride = 1.54 m.
const STRIDE_LENGTH: f32 = 1.54;
/// Heading turn rate (u16 turn units per tick).
const TURN_RATE: u16 = 400;

// ------------------------------------------------------- the flank lanes
/// 35/8 — DYADIC, exact in f32. The road-side lane's |coord|: it clears
/// ROAD_EDGE + FOOTPRINT = 4.36 by 0.015 (measured margin 0.0149998665).
const FLANK_ROAD_SIDE: f32 = 4.375;
/// 41/8 — DYADIC, exact in f32. The building-side lane's |coord|: its
/// quadrant distance 6.0 − 5.125 = 0.875 clears 0.5 + FOOTPRINT = 0.86
/// by 0.015 (measured margin 0.014999986).
const FLANK_BUILDING_SIDE: f32 = 5.125;
/// The near end of the flank span — the near-corner sidewalk (min(|x|,|z|)
/// ∈ [4, 6] beside the intersection box) belongs to the waiters.
const FLANK_NEAR: f32 = 6.5;
/// The far end of the flank span.
const FLANK_FAR: f32 = 10.5;
/// The flanks carry NO lateral spread: the 0.78 m sidewalk band
/// ([4.36, 5.14]) hosts the 0.75 lane pair with 0.015 of margin at BOTH
/// edges — nothing remains for a spread. The de-lockstep lives in the
/// speed and phase draws; the pacers carry the lateral spread.
const FLANK_SPREAD: f32 = 0.0;
/// The crossers' lateral spread: the swept band ±0.3 stays inside the
/// crosswalk strips' ±0.64 footprint-clear band (the original crosser
/// convention).
const CROSSER_SPREAD: f32 = 0.3;

// THE LANE-SEPARATION DERIVATION: the two flank lanes are EXACTLY 0.75
// apart (5.125 − 4.375 — both dyadic, so the subtraction is exact in f32).
// 0.75 is the battery's different-NPC walker margin MET BY CONSTRUCTION:
// when two walkers pass on opposite lanes the computed distance is
// sqrt(0.75² + dz²) ≥ 0.75 exactly (IEEE sqrt is correctly rounded and
// monotone; the lane coordinates stay exact because a cardinal leg's unit
// vector has a zero component and the flank spread is 0). The measured
// walker-involved minimum over the demo's full 663-frame window is EXACTLY
// 0.75 — the passing separation itself, not luck.

fn ns_west_north_flank() -> Route {
    Route::new(
        [-FLANK_ROAD_SIDE, FLANK_NEAR],
        vec![
            RouteLeg {
                to: [-FLANK_ROAD_SIDE, FLANK_FAR],
                turn: 0,
            }, // northbound, road-side
            RouteLeg {
                to: [-FLANK_BUILDING_SIDE, FLANK_FAR],
                turn: 49152,
            }, // the far connector, west
            RouteLeg {
                to: [-FLANK_BUILDING_SIDE, FLANK_NEAR],
                turn: 32768,
            }, // southbound, building-side
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

/// The N-S road's west flank, SOUTH segment (road-side lane southbound,
/// building-side northbound).
fn ns_west_south_flank() -> Route {
    Route::new(
        [-FLANK_ROAD_SIDE, -FLANK_NEAR],
        vec![
            RouteLeg {
                to: [-FLANK_ROAD_SIDE, -FLANK_FAR],
                turn: 32768,
            }, // southbound
            RouteLeg {
                to: [-FLANK_BUILDING_SIDE, -FLANK_FAR],
                turn: 16384,
            }, // the far connector, east
            RouteLeg {
                to: [-FLANK_BUILDING_SIDE, -FLANK_NEAR],
                turn: 0,
            }, // northbound
            RouteLeg {
                to: [-FLANK_ROAD_SIDE, -FLANK_NEAR],
                turn: 49152,
            }, // the near connector, west
        ],
        WALK_SPEED,
        FLANK_SPREAD,
        STRIDE_LENGTH,
        TURN_RATE,
        true,
        2,
    )
    .expect("the N-S west south flank closes exactly")
}

/// The N-S road's east flank, NORTH segment (road-side lane northbound,
/// building-side southbound).
fn ns_east_north_flank() -> Route {
    Route::new(
        [FLANK_ROAD_SIDE, FLANK_NEAR],
        vec![
            RouteLeg {
                to: [FLANK_ROAD_SIDE, FLANK_FAR],
                turn: 0,
            }, // northbound, road-side
            RouteLeg {
                to: [FLANK_BUILDING_SIDE, FLANK_FAR],
                turn: 16384,
            }, // the far connector, east
            RouteLeg {
                to: [FLANK_BUILDING_SIDE, FLANK_NEAR],
                turn: 32768,
            }, // southbound, building-side
            RouteLeg {
                to: [FLANK_ROAD_SIDE, FLANK_NEAR],
                turn: 49152,
            }, // the near connector, west
        ],
        WALK_SPEED,
        FLANK_SPREAD,
        STRIDE_LENGTH,
        TURN_RATE,
        true,
        3,
    )
    .expect("the N-S east north flank closes exactly")
}

/// The N-S road's east flank, SOUTH segment (road-side lane southbound,
/// building-side northbound).
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
                turn: 49152,
            }, // the far connector, west
            RouteLeg {
                to: [FLANK_BUILDING_SIDE, -FLANK_NEAR],
                turn: 0,
            }, // northbound
            RouteLeg {
                to: [FLANK_ROAD_SIDE, -FLANK_NEAR],
                turn: 16384,
            }, // the near connector, east
        ],
        WALK_SPEED,
        FLANK_SPREAD,
        STRIDE_LENGTH,
        TURN_RATE,
        true,
        4,
    )
    .expect("the N-S east south flank closes exactly")
}

/// The E-W road's north flank, WEST segment (road-side lane westbound,
/// building-side eastbound).
fn ew_north_west_flank() -> Route {
    Route::new(
        [-FLANK_NEAR, FLANK_ROAD_SIDE],
        vec![
            RouteLeg {
                to: [-FLANK_FAR, FLANK_ROAD_SIDE],
                turn: 49152,
            }, // westbound, road-side
            RouteLeg {
                to: [-FLANK_FAR, FLANK_BUILDING_SIDE],
                turn: 0,
            }, // the far connector, north
            RouteLeg {
                to: [-FLANK_NEAR, FLANK_BUILDING_SIDE],
                turn: 16384,
            }, // eastbound, building-side
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
        5,
    )
    .expect("the E-W north west flank closes exactly")
}

/// The E-W road's north flank, EAST segment (road-side lane eastbound,
/// building-side westbound).
fn ew_north_east_flank() -> Route {
    Route::new(
        [FLANK_NEAR, FLANK_ROAD_SIDE],
        vec![
            RouteLeg {
                to: [FLANK_FAR, FLANK_ROAD_SIDE],
                turn: 16384,
            }, // eastbound
            RouteLeg {
                to: [FLANK_FAR, FLANK_BUILDING_SIDE],
                turn: 0,
            }, // the far connector, north
            RouteLeg {
                to: [FLANK_NEAR, FLANK_BUILDING_SIDE],
                turn: 49152,
            }, // westbound
            RouteLeg {
                to: [FLANK_NEAR, FLANK_ROAD_SIDE],
                turn: 32768,
            }, // the near connector, south
        ],
        WALK_SPEED,
        FLANK_SPREAD,
        STRIDE_LENGTH,
        TURN_RATE,
        true,
        6,
    )
    .expect("the E-W north east flank closes exactly")
}

/// The E-W road's south flank, WEST segment (road-side lane westbound,
/// building-side eastbound).
fn ew_south_west_flank() -> Route {
    Route::new(
        [-FLANK_NEAR, -FLANK_ROAD_SIDE],
        vec![
            RouteLeg {
                to: [-FLANK_FAR, -FLANK_ROAD_SIDE],
                turn: 49152,
            }, // westbound
            RouteLeg {
                to: [-FLANK_FAR, -FLANK_BUILDING_SIDE],
                turn: 32768,
            }, // the far connector, south
            RouteLeg {
                to: [-FLANK_NEAR, -FLANK_BUILDING_SIDE],
                turn: 16384,
            }, // eastbound
            RouteLeg {
                to: [-FLANK_NEAR, -FLANK_ROAD_SIDE],
                turn: 0,
            }, // the near connector, north
        ],
        WALK_SPEED,
        FLANK_SPREAD,
        STRIDE_LENGTH,
        TURN_RATE,
        true,
        7,
    )
    .expect("the E-W south west flank closes exactly")
}

/// The E-W road's south flank, EAST segment (road-side lane eastbound,
/// building-side westbound).
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
        8,
    )
    .expect("the E-W south east flank closes exactly")
}

// ------------------------------------------------------- the crosswalk pacers (2)
/// THE CROSSWALK CROSSERS (TWO): full crossings of both crosswalks on the shared clock — the
/// crosser completes its original crossing (away from the camera's line) while the camera passes
/// the crosswalk behind it, then crosses the full road arm; the crossing turn-arounds at ±3.7 are
/// the furthest points that clear the standing waiters by the walker-waiter margin (4.5 − 3.7;
/// f32 measures 0.79999995 — 3.7 is not dyadic) — the whole road arm curb-to-curb minus the
/// waiter standoff (the waiters stand ON the crosswalk lines at the curbs).
/// The west crosswalk crosser.
fn west_crosswalk_crosser() -> Route {
    Route::new(
        [-5.0, 1.5],
        vec![
            RouteLeg {
                to: [-5.0, 3.7],
                turn: 0,
            }, // northbound: completing the original crossing (away from the camera's west-arm line z=0)
            RouteLeg {
                to: [-5.0, -3.7],
                turn: 32768,
            }, // the about-face at the north turn-around; the FULL southbound crossing
            RouteLeg {
                to: [-5.0, 1.5],
                turn: 0,
            }, // the full northbound crossing back to the origin (the loop closes; no turn at the origin — the heading is north on both sides)
        ],
        WALK_SPEED,
        CROSSER_SPREAD,
        STRIDE_LENGTH,
        TURN_RATE,
        true,
        9,
    )
    .expect("the west crosser closes exactly")
}

/// The north crosswalk crosser.
fn north_crosswalk_crosser() -> Route {
    Route::new(
        [1.5, 5.0],
        vec![
            RouteLeg {
                to: [3.7, 5.0],
                turn: 16384,
            }, // eastbound: completing the original crossing (away from the camera's north-arm line x=0)
            RouteLeg {
                to: [-3.7, 5.0],
                turn: 49152,
            }, // the about-face; the FULL westbound crossing
            RouteLeg {
                to: [1.5, 5.0],
                turn: 16384,
            }, // the full eastbound crossing back to the origin (closes)
        ],
        WALK_SPEED,
        CROSSER_SPREAD,
        STRIDE_LENGTH,
        TURN_RATE,
        true,
        10,
    )
    .expect("the north crosser closes exactly")
}

// ------------------------------------------------------- route table (10 routes total)
/// THE ROUTE TABLE:
/// Routes 0-7: the 8 flank loops (N-S west north, N-S west south,
/// N-S east north, N-S east south, E-W north west, E-W north east,
/// E-W south west, E-W south east)
/// Routes 8-9: the 2 crosswalk crossers (west, north)
pub fn corner_routes() -> Vec<Route> {
    vec![
        ns_west_north_flank(),
        ns_west_south_flank(),
        ns_east_north_flank(),
        ns_east_south_flank(),
        ew_north_west_flank(),
        ew_north_east_flank(),
        ew_south_west_flank(),
        ew_south_east_flank(),
        west_crosswalk_crosser(),
        north_crosswalk_crosser(),
    ]
}

// ------------------------------------------------------- waiters (8)
/// The 8 waiters stand at the ORIGINAL placement table's entries
/// (`corner_population::NPC_TABLE`'s waiter rows, byte-identical in
/// position, turn, and order — the frame-0 waiter pixels are therefore
/// unchanged): the crosswalk ends, facing the road they wait to cross.
/// The swept bands' nearest approach to any waiter is 0.8003907 m (the
/// battery's measured minimum) — the E-W south-band loops' near
/// connectors passing the north crosswalk's east-end pair.
const WAITER_POSITIONS: [[f32; 2]; 8] = [
    [-5.0, -4.5],
    [-5.7, -4.8],
    [-5.1, 4.5],
    [-5.7, 4.9],
    [-4.5, 5.1],
    [-4.9, 5.7],
    [4.5, 5.1],
    [4.9, 5.7],
];

const WAITER_TURNS: [u16; 8] = [0, 0, 32768, 32768, 16384, 16384, 49152, 49152];

// ------------------------------------------------------- population constructor
/// The corner's locomotion population: the routes, the 24 states in
/// NPC_TABLE order, and the present mask (all true — the vehicles are a
/// separate, unchanged table).
///
/// - WAITERS 0-7: standing states authored directly at
///   [`WAITER_POSITIONS`]/[`WAITER_TURNS`]. A waiter is world-placed
///   policy, not route-walking: `advance_batch` skips speed ≤ 0 states
///   before reading their route fields, so the inert (route 0, leg 0,
///   destination 0) shape carries no semantics — the authored position
///   and facing are the state's whole truth.
/// - CROSSERS 8-9: `start_route` at the pacing loops' starts (the
///   original crossers' exact positions and facings), seeds 200+i,
///   LOOPING.
/// - STROLLERS 10-23: the plan below, seeds 300+i, LOOPING.
///
/// THE STROLLER DISTRIBUTION AND ITS DRIFT BOUND: same-loop pairs start
/// at OPPOSITE-lane waypoints — half the 9.5 m perimeter (4.75 m) apart.
/// Per-NPC speed_scale ∈ [0.9, 1.1) (the seed's own stream) bounds the
/// worst-case relative drift at 0.2 × 1.4 = 0.28 m/s; over the demo's
/// full 663-frame window (11.05 s) that closes ≤ 3.09 m, leaving
/// ≥ 1.66 m of along-loop gap — the pair never reaches the same point,
/// and the closest physical approach is the opposite-lane passing
/// separation, EXACTLY 0.75 m (measured at frame 512: the E-W
/// north-east loop's pair, draws 0.9052/1.0163 — the largest relative
/// drift in the table). This repairs the WIP's quarter-perimeter
/// distribution, whose connector-adjacent pairs started 0.75 m apart
/// with 2.8 m of closable drift — a collision hole the WIP's battery
/// never measured.
pub fn corner_population_locomotion() -> (Vec<Route>, Vec<NpcLocomotion>, Vec<bool>) {
    let routes = corner_routes();

    // The 24 states in NPC_TABLE order.
    let mut states: Vec<NpcLocomotion> = Vec::with_capacity(24);

    // -- Waiters (8): standing states at the authored positions.
    for i in 0..8 {
        let [x, z] = WAITER_POSITIONS[i];
        states.push(NpcLocomotion {
            position: [x, z],
            facing: WAITER_TURNS[i],
            route: 0,
            leg: 0,
            leg_distance: 0.0,
            destination: 0,
            speed: 0.0,
            speed_scale: 1.0,  // inert (standing)
            lateral: 0.0,      // inert (standing)
            stride_phase: 0.0, // inert (standing)
        });
    }

    // -- Crossers (2): on the pacing loops.
    for (i, route_idx) in [8u32, 9u32].iter().enumerate() {
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

    // -- Strollers (14): the plan (route index, start waypoint) — six
    //   loops carry TWO strollers at opposite-lane waypoints (0 and 2,
    //   half a perimeter apart; the drift bound above), the two
    //   camera-far south-band loops carry ONE.
    let stroller_configs = [
        (0u32, 0u32),
        (0, 2),
        (1, 0),
        (1, 2),
        (2, 0),
        (2, 2),
        (3, 0),
        (3, 2),
        (4, 0),
        (4, 2),
        (5, 0),
        (5, 2),
        (6, 0),
        (7, 0),
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
/// street palette is unchanged), and the STANDER-DEDUP pose index
/// (P5 §3.2(e)): standers (`speed == 0`) share palette block 0 (the
/// cached rest), walker j carries `1 + j` — the same
/// [`crate::corner_population::stander_dedup_block_indices`] rule
/// `evaluate_population_poses_locomotion`'s palette emits, so the
/// instance list and the block list agree by construction.
pub fn corner_instances_into(
    states: &[NpcLocomotion],
    table_tints: &[[f32; 3]],
    out: &mut [ProxyInstance],
) {
    assert_eq!(
        states.len(),
        table_tints.len(),
        "states and tints are parallel"
    );
    assert_eq!(
        states.len(),
        out.len(),
        "states and output instances are parallel"
    );
    // The stander-dedup mapping, computed ZERO-ALLOC (the R2-corrective
    // finding: the Vec-based helper form allocated exactly 2 heap objects
    // per warmed frame — measured and pinned in the rig batteries' own
    // scopes, then re-based to zero by this fix as the documented repin).
    // The rule EXACTLY as stander_dedup_block_indices: a stander maps to
    // the shared rest block 0; with standers present, walker j (in table
    // order) maps to 1 + j; with NO standers, walker j maps to j (the
    // identity mapping — byte-identical for all-walking populations).
    let any_stander = states.iter().any(|state| state.speed == 0.0);
    let mut walker_ordinal = 0u32;
    for (i, state) in states.iter().enumerate() {
        let block = if state.speed == 0.0 {
            0
        } else if any_stander {
            walker_ordinal += 1;
            walker_ordinal
        } else {
            let index = walker_ordinal;
            walker_ordinal += 1;
            index
        };
        out[i] = ProxyInstance {
            transform: InstanceTransform {
                pos: [state.position[0], 0.02, state.position[1]],
                turn: state.facing,
                scale: 1.0,
            },
            tint: table_tints[i],
            alpha: 1.0,
            pose_index: block,
        };
    }
}

/// The allocating convenience form (tests): builds the Vec then calls the into form.
pub fn corner_instances(states: &[NpcLocomotion], table_tints: &[[f32; 3]]) -> Vec<ProxyInstance> {
    let mut out = Vec::with_capacity(states.len());
    out.resize(
        states.len(),
        ProxyInstance {
            transform: InstanceTransform {
                pos: [0.0, 0.0, 0.0],
                turn: 0,
                scale: 0.0,
            },
            tint: [0.0, 0.0, 0.0],
            alpha: 1.0,
            pose_index: 0,
        },
    );
    corner_instances_into(states, table_tints, &mut out);
    out
}

/// Evaluates the population's pose PALETTE IN PLACE (P5 §3.2(e),
/// the shared animation palette — was one block per NPC): the
/// emitted blocks are `[rest] ∪ {walker blocks}` in entry order —
/// block 0 the shared cached rest (IFF at least one npc stands),
/// walker j's own block at `1 + j`, matching the pose indices
/// [`corner_instances_into`] writes (the same
/// [`crate::corner_population::stander_dedup_block_indices`] rule
/// over `speed == 0`). Reuses the buffer's capacity: zero
/// allocation after the first frame. The per-npc skin matrices are
/// IDENTICAL to the pre-dedup evaluation (the identity proof's test
/// below).
pub fn evaluate_population_poses_locomotion(states: &[NpcLocomotion], out: &mut Vec<Block>) {
    use engine_animation::{
        SkinBind, compose_joint_matrices, humanoid_skeleton, rest_pose_block, sample_pose,
        skin_matrices_bound, walk_clip,
    };

    let clip = walk_clip();
    let skeleton = humanoid_skeleton();
    // The bind is a pure function of the immutable skeleton: pay the
    // rest-chain composition ONCE per batch, not per walking NPC.
    let bind = SkinBind::from_skeleton(&skeleton);

    out.clear();
    out.reserve(states.len());
    let any_stander = states.iter().any(|state| state.speed == 0.0);
    // Block 0: the ONE shared rest block (the standers' own bytes,
    // bit-exact by the rest-identity proof — deduplicated).
    if any_stander {
        out.push(rest_pose_block());
    }
    for state in states {
        if state.speed != 0.0 {
            // Walking: sample the pose at (phase * stride), then skin.
            let t = state.stride_phase * WALK_STRIDE_SECONDS;
            let pose = sample_pose(&clip, t);
            let joint_worlds = compose_joint_matrices(&skeleton, &pose);
            out.push(skin_matrices_bound(&joint_worlds, &bind));
        }
    }
}

// ------------------------------------------------------- swept-validity battery
/// One sampled swept position, tagged with the route it belongs to
/// (the swept LANE COMPLEX is the unit: same-route different-NPC
/// coexistence is the temporal campaign's job, not the sweep's).
struct SweepSample {
    route: usize,
    pos: [f32; 2],
}

/// Samples every route's legs at 0.25-unit steps INCLUDING both
/// endpoints, at the centerline and at +-spread_width when the
/// route carries a spread (flanks: spread 0 -> centerline only).
fn sample_route_swept(routes: &[Route]) -> Vec<SweepSample> {
    let mut result = Vec::new();

    for (route_idx, route) in routes.iter().enumerate() {
        // The swept band's offsets, computed once per route (no
        // per-step allocation — the battery runs on every push).
        let spread = route.spread_width();
        let offsets: [f32; 3] = if spread == 0.0 {
            [0.0, 0.0, 0.0]
        } else {
            [0.0, spread, -spread]
        };
        let offset_count = if spread == 0.0 { 1 } else { 3 };
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

            // Sample at 0.25-unit intervals along the leg.
            let num_steps = (leg_length / 0.25) as usize;
            for step in 0..=num_steps {
                let t = step as f32 / num_steps as f32;
                let pos_along = [leg_from[0] + t * dx, leg_from[1] + t * dz];

                for &offset in &offsets[..offset_count] {
                    let swept_pos = [
                        pos_along[0] + offset * perp[0],
                        pos_along[1] + offset * perp[1],
                    ];
                    result.push(SweepSample {
                        route: route_idx,
                        pos: swept_pos,
                    });
                }
            }
        }
    }

    result
}

/// The sweep's measured minimums (the evidence the tests pin).
#[derive(Debug)]
pub struct SweepSummary {
    pub samples: usize,
    /// Different routes' swept bands, center-to-center. Margin >= 0.75.
    pub cross_route_min: f32,
    /// Any swept sample vs any waiter position. Margin >= 0.75.
    pub waiter_min: f32,
    /// Any swept sample vs the camera polyline. Margin >= 0.8.
    /// The check is scoped to FLANK routes only (sample.route < 8);
    /// the crossers cross the spline BY DESIGN (part 4's documented deferral;
    /// the TEMPORAL gate below owns their camera safety).
    pub camera_min: f32,
    /// min(|x|,|z|) - (ROAD_EDGE + FOOTPRINT) over sidewalk samples.
    pub road_margin: f32,
    /// quadrant_distance - (0.5 + FOOTPRINT) over sidewalk samples.
    pub building_margin: f32,
    /// Any swept sample vs the VEHICLE_TABLE boxes. Margin >= 1.0.
    pub sedan_min: f32,
    /// Crosser samples outside their crosswalk bands (0 expected).
    pub crosser_band_violations: u32,
}

/// The sedans' axis-aligned half extents at their turns.
const SEDAN_HALF_WIDTH: f32 = 0.98;
const SEDAN_HALF_LENGTH: f32 = 2.20;

/// The sedan boxes with their half extents (from corner_population's VEHICLE_TABLE).
/// Westbound at (9, 2) turn 49152 → half extents (2.20, 0.98).
/// Northbound at (2, 9) turn 0 → (0.98, 2.20).
fn sedan_box(turn: u16, pos: [f32; 2]) -> ([f32; 2], [f32; 2]) {
    // At turn 0 or 32768: half_width in x, half_length in z
    // At turn 16384 or 49152: half_length in x, half_width in z
    if turn == 0 || turn == 32768 {
        ([pos[0], pos[1]], [SEDAN_HALF_WIDTH, SEDAN_HALF_LENGTH])
    } else {
        ([pos[0], pos[1]], [SEDAN_HALF_LENGTH, SEDAN_HALF_WIDTH])
    }
}

fn point_box_distance(px: f32, pz: f32, cx: f32, cz: f32, hx: f32, hz: f32) -> f32 {
    let dx = (px - cx).abs() - hx;
    let dz = (pz - cz).abs() - hz;
    (dx.max(0.0) * dx.max(0.0) + dz.max(0.0) * dz.max(0.0)).sqrt()
}

/// The swept-validity battery: samples every route's swept band (see
/// [`sample_route_swept`]) and measures every margin. PURE measurement —
/// the caller (the lib battery) asserts the gates; a violation shows up
/// as a margin at or below its bound (a lane creeping into the road
/// produces a NEGATIVE `road_margin`, not a skipped sample — every
/// flank sample is measured unconditionally).
pub fn sweep_battery(routes: &[Route]) -> SweepSummary {
    let samples = sample_route_swept(routes);
    let sample_count = samples.len();

    // --- cross_route_min: pairwise over samples with DIFFERENT route tags
    let mut cross_route_min = f32::INFINITY;
    for (i, a) in samples.iter().enumerate() {
        for b in samples.iter().skip(i + 1) {
            if a.route != b.route {
                let dx = a.pos[0] - b.pos[0];
                let dz = a.pos[1] - b.pos[1];
                let dist = (dx * dx + dz * dz).sqrt();
                cross_route_min = cross_route_min.min(dist);
            }
        }
    }

    // --- waiter_min: every sample vs every waiter position
    let mut waiter_min = f32::INFINITY;
    for sample in &samples {
        for waiter_pos in WAITER_POSITIONS.iter() {
            let dx = sample.pos[0] - waiter_pos[0];
            let dz = sample.pos[1] - waiter_pos[1];
            let dist = (dx * dx + dz * dz).sqrt();
            waiter_min = waiter_min.min(dist);
        }
    }

    // --- camera_min: every sample vs the camera_route's segments
    // The check is scoped to FLANK routes only (sample.route < 8);
    // the crossers cross the spline BY DESIGN (part 4's documented deferral;
    // the TEMPORAL gate below owns their camera safety).
    let mut camera_min = f32::INFINITY;
    let camera_route = camera_route();
    for window in camera_route.windows(2) {
        let a = [window[0].position[0], window[0].position[2]];
        let b = [window[1].position[0], window[1].position[2]];
        for sample in &samples {
            if sample.route < 8 {
                let dist = point_segment_distance(sample.pos, a, b);
                camera_min = camera_min.min(dist);
            }
        }
    }

    // --- road_margin and building_margin: EVERY flank sample (routes
    // 0-7), measured UNCONDITIONALLY — a lane that creeps into the road
    // or a building mass shows up as a negative margin, never as a
    // skipped sample (the WIP's filter excluded exactly the violations
    // it existed to detect; that class of weakened gate is barred).
    let mut road_margin = f32::INFINITY;
    let mut building_margin = f32::INFINITY;
    for sample in &samples {
        if sample.route < 8 {
            let [x, z] = sample.pos;
            let min_coord = x.abs().min(z.abs());
            road_margin = road_margin.min(min_coord - (ROAD_EDGE + FOOTPRINT));
            building_margin = building_margin.min(quadrant_distance(x, z) - (0.5 + FOOTPRINT));
        }
    }

    // --- sedan_min: every sample vs the two vehicle boxes
    let mut sedan_min = f32::INFINITY;
    // Westbound at (9, 2) turn 49152
    let (wb_pos, wb_ext) = sedan_box(49152, [9.0, 2.0]);
    // Northbound at (2, 9) turn 0
    let (nb_pos, nb_ext) = sedan_box(0, [2.0, 9.0]);

    for sample in &samples {
        let d1 = point_box_distance(
            sample.pos[0],
            sample.pos[1],
            wb_pos[0],
            wb_pos[1],
            wb_ext[0],
            wb_ext[1],
        );
        let d2 = point_box_distance(
            sample.pos[0],
            sample.pos[1],
            nb_pos[0],
            nb_pos[1],
            nb_ext[0],
            nb_ext[1],
        );
        sedan_min = sedan_min.min(d1.min(d2));
    }

    // --- crosser_band_violations: samples from routes 8-9 must be in their crosswalk bands
    let mut crosser_band_violations = 0u32;
    for sample in &samples {
        if sample.route == 8 {
            // West crosser: |x + 5.0| <= 0.64 AND z ∈ [-3.7, 3.7]
            let [x, z] = sample.pos;
            let in_band = (x + 5.0).abs() <= 0.64 && (-3.7..=3.7).contains(&z);
            if !in_band {
                crosser_band_violations += 1;
            }
        } else if sample.route == 9 {
            // North crosser: |z - 5.0| <= 0.64 AND x ∈ [-3.7, 3.7]
            let [x, z] = sample.pos;
            let in_band = (z - 5.0).abs() <= 0.64 && (-3.7..=3.7).contains(&x);
            if !in_band {
                crosser_band_violations += 1;
            }
        }
    }

    SweepSummary {
        samples: sample_count,
        cross_route_min,
        waiter_min,
        camera_min,
        road_margin,
        building_margin,
        sedan_min,
        crosser_band_violations,
    }
}

// ------------------------------------------------------- helpers
/// The distance from (x, z) to the nearest building-quadrant region
/// {|u| >= 6 AND |v| >= 6}.
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

    /// THE ROUTE CONSTRUCTION: every route constructs successfully.
    #[test]
    fn routes_construct_and_close() {
        let routes = corner_routes();
        assert_eq!(routes.len(), 10, "the route table has 10 routes");

        // Routes 0-7: flanks have 4 legs
        for (i, route) in routes.iter().enumerate().take(8) {
            assert_eq!(route.leg_count(), 4, "flank route {i} has 4 legs");
            assert!(route.speed() > 0.0, "route {i} has positive speed");
            assert!(
                route.spread_width() >= 0.0,
                "route {i} has non-negative spread"
            );
            assert!(route.stride_length() > 0.0, "route {i} has positive stride");
            assert!(route.loops(), "route {i} loops");
        }

        // Routes 8-9: crossers have 3 legs
        for (i, route) in routes.iter().enumerate().skip(8) {
            assert_eq!(route.leg_count(), 3, "crosser route {i} has 3 legs");
        }

        // The policy tags: table index + 1 (the routes' diagnostic
        // identity — the digest never reads them, the authoring pins them).
        for (i, route) in routes.iter().enumerate() {
            assert_eq!(route.policy_tag(), i as u32 + 1, "route {i}'s tag");
        }

        // Spot-pin route 0's leg coordinates and turns
        let route0 = &routes[0];
        assert_eq!(route0.leg_from(0), [-FLANK_ROAD_SIDE, FLANK_NEAR]);
        assert_eq!(route0.leg_to(0), [-FLANK_ROAD_SIDE, FLANK_FAR]);
        assert_eq!(route0.leg_from(1), [-FLANK_ROAD_SIDE, FLANK_FAR]);
        assert_eq!(route0.leg_to(1), [-FLANK_BUILDING_SIDE, FLANK_FAR]);
        assert_eq!(route0.leg_from(2), [-FLANK_BUILDING_SIDE, FLANK_FAR]);
        assert_eq!(route0.leg_to(2), [-FLANK_BUILDING_SIDE, FLANK_NEAR]);
        assert_eq!(route0.leg_from(3), [-FLANK_BUILDING_SIDE, FLANK_NEAR]);
        assert_eq!(route0.leg_to(3), [-FLANK_ROAD_SIDE, FLANK_NEAR]);

        // Spot-pin route 8's leg coordinates and turns (west crosser)
        let route8 = &routes[8];
        assert_eq!(route8.leg_from(0), [-5.0, 1.5]);
        assert_eq!(route8.leg_to(0), [-5.0, 3.7]);
        assert_eq!(route8.leg_from(1), [-5.0, 3.7]);
        assert_eq!(route8.leg_to(1), [-5.0, -3.7]);
        assert_eq!(route8.leg_from(2), [-5.0, -3.7]);
        assert_eq!(route8.leg_to(2), [-5.0, 1.5]);
        // Spot-pin route 9's leg coordinates and turns (north crosser)
        let route9 = &routes[9];
        assert_eq!(route9.leg_from(0), [1.5, 5.0]);
        assert_eq!(route9.leg_to(0), [3.7, 5.0]);
        assert_eq!(route9.leg_from(1), [3.7, 5.0]);
        assert_eq!(route9.leg_to(1), [-3.7, 5.0]);
        assert_eq!(route9.leg_from(2), [-3.7, 5.0]);
        assert_eq!(route9.leg_to(2), [1.5, 5.0]);
    }

    /// THE POPULATION CONSTRUCTOR: builds the routes, 24 states, and mask.
    #[test]
    fn population_construction_is_pinned() {
        let (routes, states, present) = corner_population_locomotion();

        assert_eq!(routes.len(), 10, "the route table has 10 routes");
        assert_eq!(states.len(), 24, "the population has 24 NPCs");
        assert_eq!(present.len(), 24, "the present mask has 24 entries");
        assert!(present.iter().all(|p| *p), "all NPCs are present");

        // Waiters 0-7: standing states, byte-identical to the ORIGINAL
        // placement table's waiter rows (position, turn, order) — the
        // frame-0 waiter pixels are unchanged (the pixel-preservation
        // claim, asserted against corner_population's own table).
        let (shipped, _) = crate::corner_population::corner_population();
        for i in 0..8 {
            assert_eq!(states[i].speed, 0.0, "waiter {i} stands (speed 0.0)");
            assert_eq!(
                states[i].position, WAITER_POSITIONS[i],
                "waiter {i} position"
            );
            assert_eq!(states[i].facing, WAITER_TURNS[i], "waiter {i} facing");
            assert_eq!(
                states[i].position,
                [shipped[i].transform.pos[0], shipped[i].transform.pos[2]],
                "waiter {i} stands at the shipped table's position"
            );
            assert_eq!(
                states[i].facing, shipped[i].transform.turn,
                "waiter {i} faces the shipped table's turn"
            );
        }

        // Crossers 8-9: at the crossing starts — the ORIGINAL crossers'
        // exact positions and facings — walking, LOOPING (frame 0 is
        // byte-identical to the pacers' own: same origin, same first-leg
        // heading, same seed draws).
        assert_eq!(states[8].position, [-5.0, 1.5]);
        assert_eq!(states[8].facing, 0);
        assert_eq!(states[8].route, 8);
        assert!(states[8].speed > 0.0);
        assert_eq!(states[8].destination, LOOPING);

        assert_eq!(states[9].position, [1.5, 5.0]);
        assert_eq!(states[9].facing, 16384);
        assert_eq!(states[9].route, 9);
        assert!(states[9].speed > 0.0);
        assert_eq!(states[9].destination, LOOPING);

        // Strollers 10-23: the plan's (route, waypoint) pairs — the
        // waypoint is the state's departing leg (start_route's own
        // contract), so opposite-lane pairs show legs 0 and 2.
        let expected_plan = [
            (0u32, 0u32),
            (0, 2),
            (1, 0),
            (1, 2),
            (2, 0),
            (2, 2),
            (3, 0),
            (3, 2),
            (4, 0),
            (4, 2),
            (5, 0),
            (5, 2),
            (6, 0),
            (7, 0),
        ];
        for (i, &(expected_route, expected_waypoint)) in expected_plan.iter().enumerate() {
            let state = &states[10 + i];
            assert_eq!(state.route, expected_route, "stroller {} route", 10 + i);
            assert_eq!(
                state.leg,
                expected_waypoint,
                "stroller {} starts at waypoint {}",
                10 + i,
                expected_waypoint
            );
            assert!(state.speed > 0.0, "stroller {} walks", 10 + i);
            assert_eq!(state.destination, LOOPING, "stroller {} loops", 10 + i);
        }
    }

    /// THE CONSTRUCTION DETERMINISM: two calls byte-identical, every
    /// field (the draws included — the seeds' own streams).
    #[test]
    fn construction_determinism_double_run() {
        let (routes_a, states_a, present_a) = corner_population_locomotion();
        let (routes_b, states_b, present_b) = corner_population_locomotion();

        assert_eq!(routes_a.len(), routes_b.len(), "routes count");
        for (i, (a, b)) in routes_a.iter().zip(routes_b.iter()).enumerate() {
            assert_eq!(a.leg_count(), b.leg_count(), "route {i} legs");
            assert_eq!(a.speed(), b.speed(), "route {i} speed");
            assert_eq!(a.spread_width(), b.spread_width(), "route {i} spread");
            assert_eq!(a.stride_length(), b.stride_length(), "route {i} stride");
            assert_eq!(a.turn_rate(), b.turn_rate(), "route {i} turn rate");
            assert_eq!(a.loops(), b.loops(), "route {i} loops");
            assert_eq!(a.policy_tag(), b.policy_tag(), "route {i} tag");
            for leg in 0..a.leg_count() {
                assert_eq!(a.leg_from(leg), b.leg_from(leg), "route {i} leg {leg} from");
                assert_eq!(a.leg_to(leg), b.leg_to(leg), "route {i} leg {leg} to");
                assert_eq!(a.leg_turn(leg), b.leg_turn(leg), "route {i} leg {leg} turn");
                assert_eq!(
                    a.leg_length(leg),
                    b.leg_length(leg),
                    "route {i} leg {leg} length"
                );
            }
        }

        assert_eq!(states_a.len(), states_b.len(), "states count");
        for (i, (a, b)) in states_a.iter().zip(states_b.iter()).enumerate() {
            assert_eq!(a.position, b.position, "npc {i} position");
            assert_eq!(a.facing, b.facing, "npc {i} facing");
            assert_eq!(a.route, b.route, "npc {i} route");
            assert_eq!(a.leg, b.leg, "npc {i} leg");
            assert_eq!(a.leg_distance, b.leg_distance, "npc {i} leg_distance");
            assert_eq!(a.destination, b.destination, "npc {i} destination");
            assert_eq!(a.speed, b.speed, "npc {i} speed");
            assert_eq!(a.speed_scale, b.speed_scale, "npc {i} speed_scale");
            assert_eq!(a.lateral, b.lateral, "npc {i} lateral");
            assert_eq!(a.stride_phase, b.stride_phase, "npc {i} stride_phase");
        }

        assert_eq!(present_a, present_b, "present mask");
    }

    /// THE SWEEP BATTERY GREEN: all constraints pass with measured margins
    /// (the MEASURED column is this tree's own run; movement beyond
    /// rounding is an investigation, never a silent repin).
    #[test]
    fn sweep_battery_green() {
        let (routes, _, _) = corner_population_locomotion();
        let summary = sweep_battery(&routes);

        println!("SweepSummary: {summary:?}");

        assert!(
            summary.cross_route_min >= 0.75,
            "cross_route_min >= 0.75, measured 1.3768167: {summary:?}"
        );
        assert!(
            summary.waiter_min >= 0.75,
            "waiter_min >= 0.75, measured 0.79999995 (the crossers' turn-around standoff: \
             4.5 - 3.7 in f32 — 3.7 is not dyadic): {summary:?}"
        );
        assert!(
            summary.camera_min >= 0.8,
            "camera_min >= 0.8, measured 4.375: {summary:?}"
        );
        assert!(
            summary.road_margin > 0.0,
            "road_margin > 0.0, measured 0.0149998665: {summary:?}"
        );
        assert!(
            summary.building_margin > 0.0,
            "building_margin > 0.0, measured 0.014999986: {summary:?}"
        );
        assert!(
            summary.sedan_min >= 1.0,
            "sedan_min >= 1.0, measured 1.395: {summary:?}"
        );
        assert_eq!(
            summary.crosser_band_violations, 0,
            "the crossers stay inside their crosswalk bands: {summary:?}"
        );
        assert!(summary.samples > 0, "the sweep sampled the routes");
    }

    /// THE TEMPORAL CAMPAIGN (the spec's missing pin, at the DEMO's full
    /// window): a double-constructed population advanced 663 frames at
    /// dt = 1/60 (the demo's render-then-step discipline — presented
    /// frame k shows the crowd after k ticks, and the loop's 663rd step
    /// follows the last render) is byte-identical run-to-run; the
    /// locomotion stream's FNV-1a digest pins at sampled frames; and the
    /// population NEVER collides:
    /// - walker-involved pairs (either NPC speed > 0) >= 0.75 — the
    ///   battery's different-NPC margin. MEASURED minimum EXACTLY 0.75
    ///   (frame 512, the E-W north-east loop's pair — the lane design's
    ///   passing separation, met by construction, not luck).
    /// - waiter-waiter pairs >= 0.7 — the ORIGINAL placement table's own
    ///   cardinal-facing rule (they stand at cardinal turns forever, so
    ///   the 0.72 arbitrary-orientation circle is unreachable). MEASURED
    ///   0.7211102 (waiters 2 and 3, the original table's own spacing).
    #[test]
    fn locomotion_campaign_is_deterministic_and_clear() {
        use engine_core::npc_locomotion::{LocomotionSample, advance_batch, locomotion_digest};

        let dt = 1.0 / 60.0;
        let (routes_a, mut states_a, present_a) = corner_population_locomotion();
        let (routes_b, mut states_b, present_b) = corner_population_locomotion();

        let mut walker_min = f32::INFINITY;
        let mut waiter_min = f32::INFINITY;
        // Frame k = the state after k advances (the demo's render-then-step:
        // frame 0 is the construction; the 663rd step follows the last
        // render). The checks read the frame's state BEFORE stepping it.
        for k in 0..=662u64 {
            // THE DOUBLE-RUN: both populations hold identical states at
            // every rendered frame.
            for i in 0..24 {
                assert_eq!(
                    (
                        states_a[i].position,
                        states_a[i].facing,
                        states_a[i].leg,
                        states_a[i].stride_phase
                    ),
                    (
                        states_b[i].position,
                        states_b[i].facing,
                        states_b[i].leg,
                        states_b[i].stride_phase
                    ),
                    "the double-run diverged at frame {k}, npc {i}"
                );
            }

            // THE DIGEST PINS (frame k = the state after k advances).
            if matches!(k, 0 | 33 | 66 | 300 | 600 | 662) {
                let samples: Vec<LocomotionSample> = states_a
                    .iter()
                    .enumerate()
                    .map(|(i, s)| LocomotionSample {
                        npc: i as u32,
                        position: s.position,
                        facing: s.facing,
                        stride_phase: s.stride_phase,
                        leg: s.leg,
                    })
                    .collect();
                let digest = locomotion_digest(&samples);
                // The MEASURED pins (this tree's own run; movement is an
                // investigation, never a silent repin). Frame 66, 300, 600, 662 re-measure
                // after route structure change (3 legs crossers).
                let pinned = match k {
                    0 => 5_555_165_838_415_353_039,
                    33 => 15_857_914_442_065_508_438,
                    66 => 6_639_413_311_845_848_126,
                    300 => 6_989_226_880_933_403_699,
                    600 => 15_161_854_520_410_245_683,
                    _ => 6_317_174_068_473_851_239,
                };
                assert_eq!(digest, pinned, "the locomotion digest moved at frame {k}");
            }

            // THE PAIRWISE CLEARANCE (every frame, all 276 pairs).
            for i in 0..24 {
                for j in (i + 1)..24 {
                    let dx = states_a[i].position[0] - states_a[j].position[0];
                    let dz = states_a[i].position[1] - states_a[j].position[1];
                    let distance = (dx * dx + dz * dz).sqrt();
                    if states_a[i].speed > 0.0 || states_a[j].speed > 0.0 {
                        walker_min = walker_min.min(distance);
                        assert!(
                            distance >= 0.75,
                            "walkers {i} and {j} closed to {distance} at frame {k}"
                        );
                    } else {
                        waiter_min = waiter_min.min(distance);
                        assert!(
                            distance >= 0.7,
                            "waiters {i} and {j} closed to {distance} at frame {k}"
                        );
                    }
                }
            }

            advance_batch(&routes_a, &mut states_a, &present_a, dt, None, 0.3, 1.75);
            advance_batch(&routes_b, &mut states_b, &present_b, dt, None, 0.3, 1.75);
        }
        println!(
            "locomotion campaign: 663 frames double-run byte-identical; walker-involved \
             minimum {walker_min}, waiter-waiter minimum {waiter_min}"
        );
    }

    /// THE INSTANCES CONSTRUCTOR: produces valid proxy instances.
    #[test]
    fn instances_and_into_form() {
        let (_, states, _) = corner_population_locomotion();
        let tints = crate::corner_population::corner_npc_tints();
        let instances_allocating = corner_instances(&states, &tints);

        // Test the allocating form's fields
        assert_eq!(instances_allocating.len(), 24, "one instance per NPC");
        let mapping = crate::corner_population::stander_dedup_block_indices(
            &states
                .iter()
                .map(|state| state.speed == 0.0)
                .collect::<Vec<_>>(),
        );
        for (i, instance) in instances_allocating.iter().enumerate() {
            let state = &states[i];
            assert_eq!(
                instance.transform.pos[0], state.position[0],
                "instance {i} x"
            );
            assert_eq!(
                instance.transform.pos[1], 0.02,
                "instance {i} y is the feet seam"
            );
            assert_eq!(
                instance.transform.pos[2], state.position[1],
                "instance {i} z"
            );
            assert_eq!(instance.transform.turn, state.facing, "instance {i} turn");
            assert_eq!(instance.transform.scale, 1.0, "instance {i} scale");
            // P5 §3.2(e): the stander-dedup mapping (was the
            // identity `i` — the versioned claim's history).
            assert_eq!(
                instance.pose_index, mapping[i],
                "instance {i} pose_index follows the dedup mapping"
            );
            assert_eq!(instance.tint, tints[i], "instance {i} tint");
        }

        // Test the into form: pre-build, mutate, outputs equal
        let mut instances_into = Vec::with_capacity(24);
        instances_into.resize(
            24,
            ProxyInstance {
                transform: InstanceTransform {
                    pos: [0.0, 0.0, 0.0],
                    turn: 0,
                    scale: 0.0,
                },
                tint: [0.0, 0.0, 0.0],
                alpha: 1.0,
                pose_index: 0,
            },
        );

        corner_instances_into(&states, &tints, &mut instances_into);

        assert_eq!(
            instances_into, instances_allocating,
            "into form equals allocating"
        );
        assert_eq!(instances_into.len(), 24, "into form length");
        assert_eq!(
            instances_into.capacity(),
            24,
            "into form capacity unchanged"
        );

        // Call twice to verify no realloc
        let capacity = instances_into.capacity();
        corner_instances_into(&states, &tints, &mut instances_into);
        assert_eq!(
            instances_into.capacity(),
            capacity,
            "no realloc on second call"
        );
    }

    /// THE POSE EVALUATION LOCOMOTION: matches the phase-column path.
    #[test]
    fn pose_evaluation_matches_the_phase_column_path() {
        use engine_animation::{
            compose_joint_matrices, rest_pose_block, sample_pose, skin_matrices, walk_clip,
        };

        let (_, mut states, _) = corner_population_locomotion();
        let mut blocks = Vec::new();

        // Standing waiter → bit-exact rest_pose_block()
        evaluate_population_poses_locomotion(&[states[0]], &mut blocks);
        let rest = rest_pose_block();
        assert_eq!(blocks[0], rest, "standing NPC evaluates to rest pose");

        // Forged phase tests
        states[8].stride_phase = 0.0;
        evaluate_population_poses_locomotion(&[states[8]], &mut blocks);
        let t = 0.0 * WALK_STRIDE_SECONDS;
        let clip = walk_clip();
        let skeleton = engine_animation::humanoid_skeleton();
        let pose = sample_pose(&clip, t);
        let joint_worlds = compose_joint_matrices(&skeleton, &pose);
        let expected = skin_matrices(&joint_worlds, &skeleton);
        assert_eq!(blocks[0], expected, "walking NPC at phase 0");

        states[8].stride_phase = 0.5;
        evaluate_population_poses_locomotion(&[states[8]], &mut blocks);
        let t = 0.5 * WALK_STRIDE_SECONDS;
        let pose = sample_pose(&clip, t);
        let joint_worlds = compose_joint_matrices(&skeleton, &pose);
        let expected = skin_matrices(&joint_worlds, &skeleton);
        assert_eq!(blocks[0], expected, "walking NPC at phase 0.5");

        // The population function's blocks for the REAL construction are byte-identical across two calls
        let (_, states2, _) = corner_population_locomotion();
        let mut blocks1 = Vec::new();
        let mut blocks2 = Vec::new();
        evaluate_population_poses_locomotion(&states2, &mut blocks1);
        evaluate_population_poses_locomotion(&states2, &mut blocks2);
        assert_eq!(blocks1, blocks2, "population pose blocks byte-identical");
    }

    /// THE POSE BYTES FNV PINS: the packed pose bytes' FNV-1a at frames 0, 33, 300.
    #[test]
    fn pose_bytes_fnv_pins() {
        use engine_animation::pack_pose_bytes;

        let (_, states, _) = corner_population_locomotion();
        let mut blocks = Vec::new();

        let mut digest = |k: u64| -> u64 {
            // Advance k frames
            let mut states_copy = states.clone();
            let (routes, _, present) = corner_population_locomotion();
            for _ in 0..k {
                engine_core::npc_locomotion::advance_batch(
                    &routes,
                    &mut states_copy,
                    &present,
                    1.0 / 60.0,
                    None,
                    0.3,
                    1.75,
                );
            }

            evaluate_population_poses_locomotion(&states_copy, &mut blocks);
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for block in &blocks {
                let bytes = pack_pose_bytes(block);
                for &byte in bytes.iter() {
                    hash ^= u64::from(byte);
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
            hash
        };

        let frame0 = digest(0);
        let frame33 = digest(33);
        let frame300 = digest(300);

        println!("POSE_FNV_0={frame0}");
        println!("POSE_FNV_33={frame33}");
        println!("POSE_FNV_300={frame300}");

        // The measured-actual FNV-1a digests (the placeholder-pin
        // discipline: measured at this tree). History: REPINNED in
        // P5 R2 (A18, the §3.2(e) stander dedup) — the palette is
        // `[rest] ∪ {walker blocks}` where the pre-dedup digest
        // hashed one block per npc (the standers' redundant rest
        // copies removed); the pin fired on the change exactly as
        // designed, and the per-npc skin matrices are proven
        // identical by `locomotion_dedup_preserves_every_npcs_skin_
        /// matrices`.
        const PINNED_0: u64 = 17_667_891_731_665_696_896; // R2 MEASURED (the dedup palette), frozen.
        const PINNED_33: u64 = 6_063_901_512_547_130_183; // R2 MEASURED (the dedup palette), frozen.
        const PINNED_300: u64 = 17_999_475_867_621_276_545; // R2 MEASURED (the dedup palette), frozen.

        assert_eq!(frame0, PINNED_0, "frame 0's pose-bytes digest moved");
        assert_eq!(frame33, PINNED_33, "frame 33's pose-bytes digest moved");
        assert_eq!(frame300, PINNED_300, "frame 300's pose-bytes digest moved");
    }

    /// P5 §3.2(e) — THE LOCOMOTION SKIN-MATRIX IDENTITY PROOF (the
    /// dedup's pixel-compatibility witness, the phase-column twin of
    /// corner_population's): for EVERY npc over an advancing
    /// sequence, the palette block the instance list's dedup
    /// `pose_index` selects is BIT-IDENTICAL to the block the
    /// pre-dedup per-npc evaluation produced for that npc — same
    /// skin matrices per npc ⟹ same pixels, so the ink gates hold
    /// digit for digit through the dedup.
    #[test]
    fn locomotion_dedup_preserves_every_npcs_skin_matrices() {
        use engine_animation::{
            SkinBind, compose_joint_matrices, humanoid_skeleton, rest_pose_block, sample_pose,
            skin_matrices_bound, walk_clip,
        };

        let (routes, mut states, present) = corner_population_locomotion();
        let clip = walk_clip();
        let skeleton = humanoid_skeleton();
        let bind = SkinBind::from_skeleton(&skeleton);
        let tints = crate::corner_population::corner_npc_tints();
        let mut palette = Vec::new();
        let mut instances = Vec::new();
        instances.resize(
            24,
            ProxyInstance {
                transform: InstanceTransform {
                    pos: [0.0, 0.0, 0.0],
                    turn: 0,
                    scale: 0.0,
                },
                tint: [0.0, 0.0, 0.0],
                alpha: 1.0,
                pose_index: 0,
            },
        );
        for step in 0..128_u64 {
            evaluate_population_poses_locomotion(&states, &mut palette);
            corner_instances_into(&states, &tints, &mut instances);
            for (index, state) in states.iter().enumerate() {
                // The pre-dedup per-npc evaluation (the reference
                // oracle): the stander's rest copy, the walker's own
                // sampled block at its CURRENT phase.
                let own = if state.speed == 0.0 {
                    rest_pose_block()
                } else {
                    let t = state.stride_phase * WALK_STRIDE_SECONDS;
                    let pose = sample_pose(&clip, t);
                    let joint_worlds = compose_joint_matrices(&skeleton, &pose);
                    skin_matrices_bound(&joint_worlds, &bind)
                };
                let block_index = instances[index].pose_index as usize;
                assert!(
                    block_index < palette.len(),
                    "step {step}, npc {index}: the index selects a supplied block"
                );
                assert_eq!(
                    palette[block_index], own,
                    "step {step}, npc {index}: the palette block is the npc's own \
                     skin matrices (the dedup preserves the pixels)"
                );
            }
            engine_core::npc_locomotion::advance_batch(
                &routes,
                &mut states,
                &present,
                1.0 / 60.0,
                None,
                0.3,
                1.75,
            );
        }
    }

    // Import helpers from engine_core for advance_batch (used in pose_bytes_fnv_pins test)

    /// THE FULL CROSSINGS CLEAR THE CAMERA ON THE SHARED CLOCK (the
    /// composed-crowd package's headline): the camera route and the crowd
    /// advance on the SAME deterministic clock, and at EVERY frame the
    /// whole population clears the camera's SAME-FRAME position by at
    /// least 0.8 (the placement battery's own camera convention, now
    /// temporal: a full crossing crosses the camera's road line BY
    /// DESIGN — the gate proves it crosses BEHIND the camera).
    ///
    /// THE DEMO'S OWN PAIRING (verified in corner_demo's route loop):
    /// the crowd steps from the timing warmup's first loop iteration
    /// while the camera's pose advances only inside the measured window
    /// (WARMUP = 60), so the owner sees crowd step k with camera pose
    /// `clamp(k - 60, 0, 600)` — the crowd runs 60 ticks (1 s) ahead of
    /// the pose index; the 663 presented frames are 60 warmup, 601
    /// route, and 2 drain. The gate models exactly that sequence,
    /// extended to
    /// 1280 steps to cover both crossers' second-loop line crossings
    /// against the parked camera (crosser 8's loop period ~648 frames
    /// at its 1.3704 m/s draw, crosser 9's ~671 at 1.3231). The
    /// pairwise discipline is NOT claimed beyond the demo's 663
    /// presented frames (the same-loop stroller drift bound is a
    /// 663-frame design bound — the campaign test's own scope).
    ///
    /// THE CAMERA'S SCHEDULE (SCENE_EXTENT 40: the first segment runs
    /// x -38 -> -4 over poses 0-120, so the camera passes the west
    /// crosswalk line x = -5 at pose ~116.5; the north arm runs z 2 ->
    /// 20 over poses 360-480, passing the north crosswalk line z = 5 at
    /// pose 380; beyond pose 600 the camera is parked at [0, 1.7, 38]):
    /// crosser 8 crosses z = 0 at steps 259/583/907/1231 (poses
    /// 199/523/847/1171) — the first 82 poses after the camera passed
    /// the crosswalk, with the camera already at the intersection
    /// approach; crosser 9 crosses x = 0 at steps 268/604/939/1275
    /// (poses 208/544/879/1215) — the first while the camera is still
    /// in the west arm's east end, the second 164 poses after the
    /// camera passed the north crosswalk, the rest against the parked
    /// camera. The MEASURED same-frame minimum: 1.8529123 at step 178
    /// (crosser 8 southbound mid-crosswalk at z ~ 1.83, the camera at
    /// x = -4.57 exiting the west arm) — 2.3x the 0.8 gate.
    #[test]
    fn full_crossings_clear_the_camera_on_the_shared_clock() {
        use engine_core::npc_locomotion::advance_batch;

        // Materialize the route poses once (601 entries).
        let poses: Vec<_> = crate::corner_demo::route_poses().collect();

        let dt = 1.0 / 60.0;
        let (routes_a, mut states_a, present_a) = corner_population_locomotion();
        let (routes_b, mut states_b, present_b) = corner_population_locomotion();

        // The window covers the demo's full 663-frame sequence plus both
        // crossers' second-loop line crossings with the parked camera
        // (crosser 8's period ~648 frames, crosser 9's ~671; 1280 covers both).
        let mut global_min = f32::INFINITY;
        let mut global_min_frame = 0u64;
        let mut crosser8_crossings = Vec::new();
        let mut crosser9_crossings = Vec::new();
        let mut prev_z8 = states_a[8].position[1];
        let mut prev_x9 = states_a[9].position[0];

        for k in 0..=1280u64 {
            // THE DOUBLE-RUN: both populations hold identical states.
            for npc_idx in [8usize, 9] {
                assert_eq!(
                    (
                        states_a[npc_idx].position,
                        states_a[npc_idx].facing,
                        states_a[npc_idx].leg,
                        states_a[npc_idx].stride_phase
                    ),
                    (
                        states_b[npc_idx].position,
                        states_b[npc_idx].facing,
                        states_b[npc_idx].leg,
                        states_b[npc_idx].stride_phase
                    ),
                    "the double-run diverged at frame {k}, npc {npc_idx}"
                );
            }

            // THE CAMERA MINIMUM: compute the camera's 2D position at frame k
            // using the demo's pairing: pose index = clamp(k - 60, 0, 600).
            let pose_idx = ((k as i64 - 60).max(0) as u64).min(600);
            let pose = &poses[pose_idx as usize];
            let camera_pos = [pose.position[0], pose.position[2]];

            let mut min_dist = f32::INFINITY;
            for state in states_a.iter().take(24) {
                let npc_pos = state.position;
                let dx = npc_pos[0] - camera_pos[0];
                let dz = npc_pos[1] - camera_pos[1];
                let dist = (dx * dx + dz * dz).sqrt();
                min_dist = min_dist.min(dist);
            }
            if min_dist < global_min {
                global_min = min_dist;
                global_min_frame = k;
            }
            assert!(
                min_dist >= 0.8,
                "crowd and camera too close at frame {k}: {min_dist} m"
            );

            // Crosser 8's z=0 crossings (northbound positive, southbound negative)
            let curr_z8 = states_a[8].position[1];
            if (prev_z8 <= 0.0 && curr_z8 > 0.0) || (prev_z8 >= 0.0 && curr_z8 < 0.0) {
                crosser8_crossings.push(k);
            }
            prev_z8 = curr_z8;

            // Crosser 9's x=0 crossings (eastbound positive, westbound negative)
            let curr_x9 = states_a[9].position[0];
            if (prev_x9 <= 0.0 && curr_x9 > 0.0) || (prev_x9 >= 0.0 && curr_x9 < 0.0) {
                crosser9_crossings.push(k);
            }
            prev_x9 = curr_x9;

            advance_batch(&routes_a, &mut states_a, &present_a, dt, None, 0.3, 1.75);
            advance_batch(&routes_b, &mut states_b, &present_b, dt, None, 0.3, 1.75);
        }

        println!(
            "full crossings camera minimum: {global_min} at frame {global_min_frame}; \
             crosser 8 z=0 crossings: {:?}; crosser 9 x=0 crossings: {:?}",
            crosser8_crossings, crosser9_crossings
        );

        // THE MEASURED PINS (this tree's own run; movement is an
        // investigation, never a silent repin).
        assert_eq!(
            global_min, 1.8529123,
            "the same-frame camera minimum moved (was 1.8529123 at step 178)"
        );
        assert_eq!(
            global_min_frame, 178,
            "the same-frame camera minimum's frame moved"
        );
        assert_eq!(
            crosser8_crossings,
            [259, 583, 907, 1231],
            "crosser 8's z=0 crossing steps moved"
        );
        assert_eq!(
            crosser9_crossings,
            [268, 604, 939, 1275],
            "crosser 9's x=0 crossing steps moved"
        );

        // THE SEMANTIC TIMING BOUNDS (pose-index terms — the camera's
        // own schedule): crosser 8's first crossing lands at pose
        // 259 - 60 = 199, which must sit past the camera's west-arm
        // exit (pose 120; the camera passed the crosswalk line x = -5
        // at pose ~116.5) — the crossing happens BEHIND the camera.
        let first8_pose = crosser8_crossings[0] - 60;
        assert!(
            first8_pose > 130,
            "crosser 8's first z=0 crossing lands at pose {first8_pose}, past the \
             camera's west-arm exit (120) — the design's behind-the-camera bound"
        );
        // None of crosser 9's crossings may land inside the camera's
        // +-1.5 m window around its own north-crosswalk pass (the
        // camera crosses z = 5 at pose 380 at 9 m/s, so z in [3.5, 6.5]
        // spans poses [370, 390]); the measured crossings land at poses
        // 208/544 and, past the route's end, against the parked camera.
        for &step in &crosser9_crossings {
            let pose = step.saturating_sub(60).min(600);
            assert!(
                !(370..=390).contains(&pose),
                "crosser 9's x=0 crossing lands at pose {pose}, inside the camera's \
                 north-crosswalk window [370, 390]"
            );
        }
    }
}
