//! The corner scene's deterministic population (the Cycle 6-E
//! ladder's package 3 — `docs/AUTONOMOUS-CYCLE-PROTOCOL.md` §2): the
//! FULL 24-NPC + 2-vehicle placement table, drawn through the
//! instanced proxy path (`--corner --populated`).
//!
//! PURE DATA, FIXED TABLE (not seeded — the corner scene's own
//! discipline, `lit_scene::intersection_boxes`' "no seeding
//! needed"): hand-authored placements in the intersection's
//! road-cross frame, byte-stable, documented per entry. The ENTRY
//! ORDER is part of the pin: npc entries pack before vehicle entries
//! (`repack_populations`' fixed order), and the packed bytes carry
//! their own FNV-1a pin below — any hand edit (even one tint
//! channel) is a conscious repin, never silent drift.
//!
//! THE WORLD FRAME (the corner scene's): +X east, +Z north; the 8 m
//! road cross through the origin (min(|x|, |z|) < 4 is road); the
//! 2 m sidewalk ring around it (min(|x|, |z|) in [4, 6]); the four
//! building masses beyond (min(|x|, |z|) >= 6, out to
//! [`SCENE_EXTENT`]). Two crosswalks sit just outside the
//! intersection box: the WEST crosswalk crosses the west road arm at
//! x in [-6, -4] (walked north-south), the NORTH crosswalk crosses
//! the north road arm at z in [4, 6] (walked east-west).
//!
//! THE FACING CONVENTION (derived, not labeled by intuition): the
//! proxy meshes' local forward is local +Z — the axis the 6-E
//! oracle walls face (`render_instanced`'s single-instance checks).
//! The instance matrix's third column is (sin, 0, cos), so:
//!
//! ```text
//!   turn     0 -> +Z (north)    16384 -> +X (east)
//!   turn 32768 -> -Z (south)    49152 -> -X (west)
//! ```
//!
//! Every entry's comment states its facing FROM THIS TABLE. (The
//! 12-NPC demo table this replaces labeled its facings
//! inconsistently: under no single mesh-forward choice did all its
//! comments hold against the pinned turn convention. This table
//! derives each facing instead.)
//!
//! THE PLACEMENT RULES (pinned by the tests below):
//! - pedestrians stand on the sidewalk ring or mid-crosswalk, feet
//!   at y = 0.02 (the 6-E seam convention: on the ground, off its
//!   exact plane), uniform scale 1.0 — a placement table, not a
//!   height sampler (scale variety is the gate fixture's job);
//! - the pedestrian footprint is a circle of radius 0.36 m (the v2
//!   humanoid's worst corner, the upper arms at |x| 0.33 x z 0.06);
//!   non-crossing pedestrians keep it fully off the road
//!   (min(|x|,|z|) >= 4.36);
//! - vehicles rest in their right-hand-traffic lanes — westbound
//!   keeps the north half (z in (0, 4)), northbound the east half
//!   (x in (0, 4)) — clear of the intersection box, at y = 0;
//! - EVERY placement clears the camera route (the corner route's
//!   own waypoint polyline) by at least 1.0 m, footprint included;
//! - pedestrians never overlap: >= 0.7 m center-to-center (at the
//!   table's cardinal facings the worst mesh contact is the
//!   arm-to-arm reach, 0.66 m across — the 0.72 m arbitrary-
//!   orientation bounding circle is NOT reachable at cardinal
//!   turns; the 0.7 m rule carries 0.04 m of true margin); vehicles
//!   clear pedestrians, each other, and the building masses by at
//!   least 0.5 m footprint-inclusive, and everything stays inside
//!   the scene extent.
//!
//! The table uses only cardinal facings; the turn-fraction math's
//! non-cardinal coverage belongs to the gate fixture (and the
//! scaffold's known-answer tests), not to this table.

use engine_renderer::proxy_instances::{InstanceTransform, ProxyInstance};

/// The 24 pedestrian placements, in fixed order: the crosswalk
/// waiters (8), the crossers (2), then the sidewalk strollers (14).
/// Tints are a hand-authored street palette (deliberately not a
/// gradient or a seeded generator — pure data).
#[rustfmt::skip]
const NPC_TABLE: [ProxyInstance; 24] = [
    // -- Crosswalk waiters (8): two per end of the west and north
    //    crosswalks — the two crosswalks the camera route passes.
    // West crosswalk, south end (the south band at the crosswalk
    // line): facing north (turn 0), waiting to cross the west arm.
    ProxyInstance { transform: InstanceTransform { pos: [-5.0, 0.02, -4.5], turn: 0, scale: 1.0 }, tint: [0.85, 0.70, 0.55] , pose_index: 0 },
    ProxyInstance { transform: InstanceTransform { pos: [-5.7, 0.02, -4.8], turn: 0, scale: 1.0 }, tint: [0.20, 0.45, 0.75] , pose_index: 1 },
    // West crosswalk, north end (the north band at the crosswalk
    // line): facing south (turn 32768).
    ProxyInstance { transform: InstanceTransform { pos: [-5.1, 0.02, 4.5], turn: 32768, scale: 1.0 }, tint: [0.90, 0.35, 0.20] , pose_index: 2 },
    ProxyInstance { transform: InstanceTransform { pos: [-5.7, 0.02, 4.9], turn: 32768, scale: 1.0 }, tint: [0.35, 0.75, 0.30] , pose_index: 3 },
    // North crosswalk, west end (the west band at the crosswalk
    // line): facing east (turn 16384), waiting to cross the north
    // arm. The lead stands tight to the road edge, off the corner
    // the west crosswalk's north-end pair occupies.
    ProxyInstance { transform: InstanceTransform { pos: [-4.5, 0.02, 5.1], turn: 16384, scale: 1.0 }, tint: [0.75, 0.30, 0.60] , pose_index: 4 },
    ProxyInstance { transform: InstanceTransform { pos: [-4.9, 0.02, 5.7], turn: 16384, scale: 1.0 }, tint: [0.95, 0.80, 0.40] , pose_index: 5 },
    // North crosswalk, east end (the east band at the crosswalk
    // line): facing west (turn 49152).
    ProxyInstance { transform: InstanceTransform { pos: [4.5, 0.02, 5.1], turn: 49152, scale: 1.0 }, tint: [0.15, 0.55, 0.85] , pose_index: 6 },
    ProxyInstance { transform: InstanceTransform { pos: [4.9, 0.02, 5.7], turn: 49152, scale: 1.0 }, tint: [0.60, 0.60, 0.65] , pose_index: 7 },
    // -- Crossers (2): mid-crosswalk, the only pedestrians in the
    //    road. The west crosswalk is crossed northbound (turn 0),
    //    the north crosswalk eastbound (turn 16384); both hold
    //    >= 1.0 m of footprint-inclusive clearance from the camera
    //    route, which runs z~0 through the west arm and x~0 through
    //    the north arm.
    ProxyInstance { transform: InstanceTransform { pos: [-5.0, 0.02, 1.5], turn: 0, scale: 1.0 }, tint: [0.45, 0.80, 0.75] , pose_index: 8 },
    ProxyInstance { transform: InstanceTransform { pos: [1.5, 0.02, 5.0], turn: 16384, scale: 1.0 }, tint: [0.70, 0.20, 0.20] , pose_index: 9 },
    // -- Sidewalk strollers (14): walking along the bands (parallel
    //    to the road they border), both directions represented. The
    //    far south-east strip is deliberately empty — the route's
    //    only southeast look (waypoint 3) frames the east band's
    //    south segment and the SE mass base, not that strip.
    // West band, north of the E-W road: walking north (turn 0) and
    // south (turn 32768). The v2 footprint (0.36 m) moved the
    // band's building-clear line outward — the southbound stroller
    // stands at x -4.8 (the corner clearance 1.2 m).
    ProxyInstance { transform: InstanceTransform { pos: [-5.0, 0.02, 7.5], turn: 0, scale: 1.0 }, tint: [0.55, 0.40, 0.30] , pose_index: 10 },
    ProxyInstance { transform: InstanceTransform { pos: [-4.8, 0.02, 10.0], turn: 32768, scale: 1.0 }, tint: [0.80, 0.50, 0.30] , pose_index: 11 },
    // West band, south of the E-W road.
    ProxyInstance { transform: InstanceTransform { pos: [-5.1, 0.02, -7.5], turn: 32768, scale: 1.0 }, tint: [0.30, 0.30, 0.35] , pose_index: 12 },
    ProxyInstance { transform: InstanceTransform { pos: [-4.9, 0.02, -10.0], turn: 0, scale: 1.0 }, tint: [0.90, 0.75, 0.60] , pose_index: 13 },
    // East band, north of the E-W road. The southbound stroller
    // stands at x 4.8 for the v2 footprint's corner clearance.
    ProxyInstance { transform: InstanceTransform { pos: [5.0, 0.02, 8.0], turn: 0, scale: 1.0 }, tint: [0.25, 0.60, 0.40] , pose_index: 14 },
    ProxyInstance { transform: InstanceTransform { pos: [4.8, 0.02, 10.5], turn: 32768, scale: 1.0 }, tint: [0.85, 0.60, 0.70] , pose_index: 15 },
    // East band, south of the E-W road.
    ProxyInstance { transform: InstanceTransform { pos: [5.1, 0.02, -7.0], turn: 0, scale: 1.0 }, tint: [0.40, 0.40, 0.55] , pose_index: 16 },
    ProxyInstance { transform: InstanceTransform { pos: [4.9, 0.02, -9.5], turn: 32768, scale: 1.0 }, tint: [0.95, 0.55, 0.30] , pose_index: 17 },
    // North band, west of the N-S road: walking east (turn 16384)
    // and west (turn 49152). The westbound stroller stands at
    // z 4.8 for the v2 footprint's corner clearance.
    ProxyInstance { transform: InstanceTransform { pos: [-7.5, 0.02, 5.0], turn: 16384, scale: 1.0 }, tint: [0.65, 0.75, 0.30] , pose_index: 18 },
    ProxyInstance { transform: InstanceTransform { pos: [-10.0, 0.02, 4.8], turn: 49152, scale: 1.0 }, tint: [0.50, 0.65, 0.80] , pose_index: 19 },
    // North band, east of the N-S road.
    ProxyInstance { transform: InstanceTransform { pos: [7.5, 0.02, 5.0], turn: 49152, scale: 1.0 }, tint: [0.80, 0.35, 0.45] , pose_index: 20 },
    ProxyInstance { transform: InstanceTransform { pos: [10.0, 0.02, 4.9], turn: 16384, scale: 1.0 }, tint: [0.30, 0.70, 0.55] , pose_index: 21 },
    // South band, west of the N-S road.
    ProxyInstance { transform: InstanceTransform { pos: [-7.0, 0.02, -5.0], turn: 16384, scale: 1.0 }, tint: [0.70, 0.65, 0.50] , pose_index: 22 },
    ProxyInstance { transform: InstanceTransform { pos: [-9.5, 0.02, -5.1], turn: 49152, scale: 1.0 }, tint: [0.55, 0.30, 0.65] , pose_index: 23 },
];

/// The 2 vehicle placements, in fixed order (westbound then
/// northbound — the demo's `proxy population:` line prints in this
/// order). Both are stopped sedans in their right-hand-traffic
/// lanes, clear of the intersection box; the westbound one is
/// ONCOMING (approaching from the east, nose west — visible to the
/// entering camera), the northbound one DEPARTED (north of the north
/// crosswalk, nose north — the east crosser passes behind it).
#[rustfmt::skip]
const VEHICLE_TABLE: [ProxyInstance; 2] = [
    // Westbound, the north lane (z = +2) of the E-W road's east arm:
    // nose west (turn 49152), stopped short east of the box.
    ProxyInstance { transform: InstanceTransform { pos: [9.0, 0.0, 2.0], turn: 49152, scale: 1.0 }, tint: [0.75, 0.18, 0.15] , pose_index: 0 },
    // Northbound, the east lane (x = +2) of the N-S road's north
    // arm: nose north (turn 0), departed clear of the crosswalk.
    ProxyInstance { transform: InstanceTransform { pos: [2.0, 0.0, 9.0], turn: 0, scale: 1.0 }, tint: [0.15, 0.25, 0.70] , pose_index: 0 },
];

/// The corner population: 24 NPCs then 2 vehicles, in the fixed
/// buffer order (the repack's npc-then-vehicle order — the table's
/// entry order is part of the byte pin). Built ONCE per run (the
/// demo holds it for the whole route); the per-frame
/// zero-allocation discipline is the renderer's retained repack, not
/// this table's construction.
pub fn corner_population() -> (Vec<ProxyInstance>, Vec<ProxyInstance>) {
    (NPC_TABLE.to_vec(), VEHICLE_TABLE.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_renderer::lit_scene::{SCENE_EXTENT, camera_route};
    use engine_renderer::proxy_instances::{INSTANCE_STRIDE, repack_populations};

    /// The pedestrian footprint radius: the v2 humanoid's worst
    /// corner (the upper arms at |x| 0.33, z 0.06 → half-diagonal
    /// ~0.335; the lower legs' forward feet smaller; rounded up).
    const FOOTPRINT: f32 = 0.36;
    /// The v2 sedan's half extents (width x, length z at turn 0):
    /// the wheels' outer edge 0.975 and the body's nose/tail 2.20.
    const VEHICLE_HALF_WIDTH: f32 = 0.98;
    const VEHICLE_HALF_LENGTH: f32 = 2.20;
    /// The building masses begin where BOTH coordinates reach 6 m
    /// (the corner blocks; the road edge is 4 m, the sidewalk ring
    /// spans between).
    const BUILDING_LINE: f32 = 6.0;
    const ROAD_EDGE: f32 = 4.0;

    /// The counts, heights, scales, and cardinal turns are pinned:
    /// the ladder's headline is 24 NPCs + 2 vehicles, the seam
    /// convention puts feet at 0.02 and vehicles at 0.0, and this is
    /// a placement table (uniform scale, cardinal facings only).
    #[test]
    fn counts_heights_scales_and_turns_are_pinned() {
        let (npc, vehicle) = corner_population();
        assert_eq!(npc.len(), 24, "the ladder's 24-NPC pin");
        assert_eq!(vehicle.len(), 2, "the ladder's 2-vehicle pin");
        for instance in &npc {
            assert_eq!(instance.transform.pos[1], 0.02, "feet off the ground plane");
            assert_eq!(instance.transform.scale, 1.0, "uniform adult scale");
            assert_eq!(
                instance.transform.turn % 16384,
                0,
                "cardinal facings only (documented table choice)"
            );
        }
        for instance in &vehicle {
            assert_eq!(
                instance.transform.pos[1], 0.0,
                "vehicles on the ground plane"
            );
            assert_eq!(instance.transform.scale, 1.0);
            assert_eq!(instance.transform.turn % 16384, 0, "axis-aligned lanes");
            assert_eq!(instance.pose_index, 0, "vehicles never read the pose slot");
        }
        // THE POSE-INDEX PIN: every npc's block index is its list
        // position — the identity mapping the renderer's pose repack
        // and the demo's walk evaluation both rely on (a later
        // dedup remaps these values consciously, never silently).
        for (index, instance) in npc.iter().enumerate() {
            assert_eq!(
                instance.pose_index, index as u32,
                "npc {index}'s pose block is its list position"
            );
        }
    }

    /// Every pedestrian stands fully on the sidewalk ring (footprint
    /// off the road, clear of the building masses, inside the
    /// extent) — except the crossers, who stand mid-crosswalk: on
    /// the road, inside their crosswalk band with footprint margin.
    #[test]
    fn pedestrians_stand_on_the_sidewalk_ring_or_a_crosswalk() {
        let (npc, _) = corner_population();
        for (index, instance) in npc.iter().enumerate() {
            let [x, _, z] = instance.transform.pos;
            let (ax, az) = (x.abs(), z.abs());
            let off_road = ax.min(az);
            if off_road >= ROAD_EDGE {
                // Sidewalk pedestrian: the footprint stays off the
                // road and clear of every building quadrant by at
                // least 0.5 m edge-to-edge.
                assert!(
                    off_road >= ROAD_EDGE + FOOTPRINT,
                    "npc {index} at ({x}, {z}): the footprint is on the road"
                );
                assert!(
                    quadrant_distance(x, z) >= 0.5 + FOOTPRINT,
                    "npc {index} at ({x}, {z}): closer than 0.5 m to a building"
                );
            } else {
                // Crosser: ON the road, inside one of the two
                // crosswalk bands with footprint margin.
                let west_band = (x + 5.0).abs() <= 1.0 - FOOTPRINT && az <= ROAD_EDGE - FOOTPRINT;
                let north_band = (z - 5.0).abs() <= 1.0 - FOOTPRINT && ax <= ROAD_EDGE - FOOTPRINT;
                assert!(
                    west_band || north_band,
                    "npc {index} at ({x}, {z}): in the road but outside both crosswalk bands"
                );
            }
            assert!(
                ax <= SCENE_EXTENT - FOOTPRINT && az <= SCENE_EXTENT - FOOTPRINT,
                "npc {index} at ({x}, {z}): outside the scene extent"
            );
        }
    }

    /// The vehicles rest in their right-hand-traffic lanes, inside
    /// their road arms, clear of the intersection box, the building
    /// masses, the pedestrians, and each other (all footprint-
    /// inclusive >= 0.5 m).
    #[test]
    fn vehicles_rest_in_their_lanes_clear_of_everything() {
        let (npc, vehicle) = corner_population();
        let [westbound, northbound] = &vehicle[..] else {
            panic!("the two vehicles are pinned by order");
        };
        // Westbound: the E-W road's NORTH half (z in (0, 4)), in the
        // east arm, stopped short of the box (the whole box east of
        // x = 4).
        let (wb_x, wb_z) = xz(westbound);
        let (wb_hx, wb_hz) = vehicle_half_extents(westbound);
        assert!(wb_z > 0.0 && wb_z + wb_hz < ROAD_EDGE, "the north lane");
        assert!(wb_x - wb_hx > ROAD_EDGE, "clear of the intersection box");
        // Northbound: the N-S road's EAST half (x in (0, 4)), in the
        // north arm, departed north of the box.
        let (nb_x, nb_z) = xz(northbound);
        let (nb_hx, nb_hz) = vehicle_half_extents(northbound);
        assert!(nb_x > 0.0 && nb_x + nb_hx < ROAD_EDGE, "the east lane");
        assert!(nb_z - nb_hz > ROAD_EDGE, "clear of the intersection box");

        for instance in &vehicle {
            let (cx, cz) = xz(instance);
            let (hx, hz) = vehicle_half_extents(instance);
            // Clear of every building quadrant (axis-aligned box vs
            // the quadrant regions, exact via the reflected corner
            // gaps).
            for (sx, sz) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
                let gx = (BUILDING_LINE - (sx * cx + hx)).max(0.0);
                let gz = (BUILDING_LINE - (sz * cz + hz)).max(0.0);
                let distance = (gx * gx + gz * gz).sqrt();
                assert!(
                    distance >= 0.5,
                    "vehicle at ({cx}, {cz}): {distance} m from a building mass"
                );
            }
            // Clear of every pedestrian (box vs footprint circle).
            for (index, person) in npc.iter().enumerate() {
                let (px, pz) = xz(person);
                let distance = point_box_distance(px, pz, cx, cz, hx, hz) - FOOTPRINT;
                assert!(
                    distance >= 0.5,
                    "npc {index} at ({px}, {pz}): {distance} m from the vehicle at ({cx}, {cz})"
                );
            }
        }
        // Clear of each other (axis-aligned boxes).
        let gap_x =
            (westbound.transform.pos[0] - northbound.transform.pos[0]).abs() - (wb_hx + nb_hx);
        let gap_z =
            (westbound.transform.pos[2] - northbound.transform.pos[2]).abs() - (wb_hz + nb_hz);
        let gap = (gap_x.max(0.0) * gap_x.max(0.0) + gap_z.max(0.0) * gap_z.max(0.0)).sqrt();
        assert!(gap >= 0.5, "the vehicles sit {gap} m apart");
    }

    /// Every placement clears the camera route (the corner route's
    /// waypoint polyline — the line the demo interpolates along) by
    /// at least 1.0 m, footprint included: pedestrians as their
    /// footprint circles, vehicles as their (axis-aligned, cardinal
    /// turn) road boxes. The prior 12-NPC table's tightest approach
    /// was 0.26 m (a deliberate near-pass); this table is strictly
    /// more conservative.
    #[test]
    fn population_clears_the_camera_route() {
        let (npc, vehicle) = corner_population();
        let route = route_segments();
        for (index, instance) in npc.iter().enumerate() {
            let (px, pz) = xz(instance);
            let mut best = f32::INFINITY;
            for &(a, b) in &route {
                best = best.min(point_segment_distance(px, pz, a, b));
            }
            let clearance = best - FOOTPRINT;
            assert!(
                clearance >= 1.0,
                "npc {index} at ({px}, {pz}): {clearance} m of route clearance"
            );
        }
        for instance in &vehicle {
            let (cx, cz) = xz(instance);
            let (hx, hz) = vehicle_half_extents(instance);
            let edges = box_edges(cx, cz, hx, hz);
            let mut best = f32::INFINITY;
            for &(a, b) in &route {
                for edge in edges {
                    best = best.min(segment_segment_distance(a, b, edge[0], edge[1]));
                }
            }
            assert!(
                best >= 1.0,
                "vehicle at ({cx}, {cz}): {best} m of route clearance"
            );
        }
    }

    /// Pedestrians never overlap: >= 0.7 m center-to-center. At the
    /// table's cardinal facings the worst true mesh contact is the
    /// arm-to-arm reach (|x| 0.33 a side — 0.66 m across, the 0.7 m
    /// rule's 0.04 m of margin); the 0.72 m arbitrary-orientation
    /// bounding circle is not reachable at cardinal turns.
    #[test]
    fn pedestrians_never_overlap() {
        let (npc, _) = corner_population();
        for (i, a) in npc.iter().enumerate() {
            for b in &npc[i + 1..] {
                let (ax, az) = xz(a);
                let (bx, bz) = xz(b);
                let dx = ax - bx;
                let dz = az - bz;
                let distance = (dx * dx + dz * dz).sqrt();
                assert!(
                    distance >= 0.7,
                    "npcs at ({ax}, {az}) and ({bx}, {bz}): {distance} m apart"
                );
            }
        }
    }

    /// THE PLACEMENT-TABLE BYTE PIN: the full population packs to a
    /// byte sequence whose FNV-1a hash is this pinned u64 (measured
    /// actual; the cross-platform proof is the scaffold's
    /// `packed_bytes_fnv_pin` discipline — the packing is pure IEEE
    /// arithmetic over `to_bits`, so every platform's CI asserts the
    /// same value). The repack itself validates every instance
    /// before a byte exists, so this pin also proves the whole table
    /// passes the packer's contract.
    #[test]
    fn population_packed_bytes_fnv_pin() {
        let (npc, vehicle) = corner_population();
        let mut buffer = Vec::new();
        repack_populations(&mut buffer, &npc, &vehicle).unwrap();
        assert_eq!(buffer.len(), 26 * INSTANCE_STRIDE);
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for &byte in &buffer {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        assert_eq!(
            hash, PINNED_FNV,
            "the placement table's packed-bytes hash moved: a hand edit — a \
             conscious repin in the PR that made it, never silent drift"
        );
    }

    /// The measured-actual FNV-1a of the pinned placement table
    /// above (the placeholder-pin discipline: the first run measures,
    /// the pin follows, and any later movement is an investigation).
    /// History: repinned in the Era-1 population-v2 package (the
    /// three strollers nudged outward for the v2 footprint, from
    /// 9_133_392_768_426_866_649 to 4_298_571_200_219_075_995);
    /// repinned again in the Era-2 skinning package — the params
    /// vec4's x now carries each npc's sequential pose block index
    /// (the versioned claim; the pin fired on the change exactly as
    /// designed and the cause is the claim, not an accident).
    const PINNED_FNV: u64 = 16_877_662_039_878_331_722;

    // ------------------------------------------------------- helpers
    // (pure f32; the pins carry margins, so platform ulp differences
    // cannot flip them — only the byte pin requires bit-stability,
    // and the packing provides it.)

    fn xz(instance: &ProxyInstance) -> (f32, f32) {
        (instance.transform.pos[0], instance.transform.pos[2])
    }

    /// The distance from (x, z) to the nearest building-quadrant
    /// region {|u| >= 6 AND |v| >= 6} (a point; the sign reflection
    /// folds all four quadrants into the northeast one).
    fn quadrant_distance(x: f32, z: f32) -> f32 {
        let mut best = f32::INFINITY;
        for (sx, sz) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
            let gx = (BUILDING_LINE - sx * x).max(0.0);
            let gz = (BUILDING_LINE - sz * z).max(0.0);
            best = best.min((gx * gx + gz * gz).sqrt());
        }
        best
    }

    /// The sedan's axis-aligned half extents at its turn (cardinal
    /// turns only — pinned above; a quarter turn swaps width and
    /// length).
    fn vehicle_half_extents(instance: &ProxyInstance) -> (f32, f32) {
        match instance.transform.turn {
            0 | 32768 => (VEHICLE_HALF_WIDTH, VEHICLE_HALF_LENGTH),
            _ => (VEHICLE_HALF_LENGTH, VEHICLE_HALF_WIDTH),
        }
    }

    /// The camera route as 2D segments (the demo interpolates exactly
    /// along these lines at 120 steps per segment).
    fn route_segments() -> Vec<([f32; 2], [f32; 2])> {
        let route = camera_route();
        route
            .windows(2)
            .map(|pair| {
                (
                    [pair[0].position[0], pair[0].position[2]],
                    [pair[1].position[0], pair[1].position[2]],
                )
            })
            .collect()
    }

    /// The exact point-to-segment distance in 2D.
    fn point_segment_distance(px: f32, pz: f32, a: [f32; 2], b: [f32; 2]) -> f32 {
        let ab = [b[0] - a[0], b[1] - a[1]];
        let ap = [px - a[0], pz - a[1]];
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

    /// Whether two 2D segments cross (the orientation test).
    fn segments_intersect(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> bool {
        let cross = |o: [f32; 2], p: [f32; 2], q: [f32; 2]| {
            (p[0] - o[0]) * (q[1] - o[1]) - (p[1] - o[1]) * (q[0] - o[0])
        };
        let d1 = cross(c, d, a);
        let d2 = cross(c, d, b);
        let d3 = cross(a, b, c);
        let d4 = cross(a, b, d);
        ((d1 > 0.0) != (d2 > 0.0) || d1 == 0.0 || d2 == 0.0)
            && ((d3 > 0.0) != (d4 > 0.0) || d3 == 0.0 || d4 == 0.0)
    }

    /// The exact distance between two 2D segments: 0 when they cross,
    /// otherwise the closest endpoint-to-segment distance (exact for
    /// disjoint segments — for parallel segments the perpendicular
    /// foot of an endpoint is interior, and for crossing lines the
    /// closest pair faces an endpoint).
    fn segment_segment_distance(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> f32 {
        if segments_intersect(a, b, c, d) {
            return 0.0;
        }
        [
            point_segment_distance(a[0], a[1], c, d),
            point_segment_distance(b[0], b[1], c, d),
            point_segment_distance(c[0], c[1], a, b),
            point_segment_distance(d[0], d[1], a, b),
        ]
        .into_iter()
        .fold(f32::INFINITY, f32::min)
    }

    /// The distance from a 2D point to an axis-aligned box.
    fn point_box_distance(px: f32, pz: f32, cx: f32, cz: f32, hx: f32, hz: f32) -> f32 {
        let dx = (px - cx).abs() - hx;
        let dz = (pz - cz).abs() - hz;
        (dx.max(0.0) * dx.max(0.0) + dz.max(0.0) * dz.max(0.0)).sqrt()
    }

    /// The box's four edges as segment endpoint pairs.
    fn box_edges(cx: f32, cz: f32, hx: f32, hz: f32) -> [[[f32; 2]; 2]; 4] {
        let corners = [
            [cx - hx, cz - hz],
            [cx + hx, cz - hz],
            [cx + hx, cz + hz],
            [cx - hx, cz + hz],
        ];
        [
            [corners[0], corners[1]],
            [corners[1], corners[2]],
            [corners[2], corners[3]],
            [corners[3], corners[0]],
        ]
    }
}
