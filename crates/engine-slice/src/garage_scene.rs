//! THE P4 SECOND SCENE (the design's §7 — the charter's
//! demonstration requirement): "The Garage" — a small workshop
//! interior authored COMPLETELY on the slice side of the boundary
//! and rendered ENTIRELY through the public API
//! ([`engine_renderer::Renderer::attach_scene`] +
//! [`engine_renderer::Renderer::submit_frame`]). NO corner types,
//! NO corner content, NO renderer internals: the scene's mesh,
//! materials, bounds, wet roles, light, sky, population, and
//! camera are all authored here, and the render_garage battery
//! proves the same public surface the corner route uses serves
//! authored content it has never seen.
//!
//! WHAT THE SCENE IS: a 20 m × 20 m slab — an oiled concrete
//! floor (material 0, the wet receiver), a U of painted masonry
//! walls open on the +Z side (material 1), and a workbench +
//! storage-crate cluster (material 2) under one warm work-lamp
//! directional light. A dusk sky shows through the open door and
//! above the walls. THE MATERIAL COUNT IS EXACTLY THREE — the
//! pass families' compiled shape
//! ([`engine_renderer::PASS_FAMILY_MATERIAL_COUNT`]); the second
//! scene deliberately fits the compiled shape rather than paying
//! for the fixed-array generalization (the boundary is
//! documented at the constant, fail-visible at attach).
//!
//! THE POPULATION: six mechanics — two standing at the workbench,
//! four mid-room walkers whose pose blocks advance with the demo
//! clock (the SAME
//! [`engine_renderer::ProxyPopulation`] + pose-block contract the
//! corner crowd uses, evaluated through the same pure
//! `evaluate_population_poses` machinery) — plus one parked van.
//! The walkers animate IN PLACE: the placement is static (the
//! garage has no locomotion spine by design — the scene proves
//! the RENDER contract, not crowd simulation; the corner demo
//! remains the locomotion consumer).
//!
//! THE WET ROLES: the floor quad is the receiver (an oiled slab
//! reads as wet); the walls and the crate cluster reflect; the
//! mirror plane is the floor at y = 0 — the wet path renders the
//! mirrored masses Schlick-blended over the floor exactly as the
//! corner's does, over THIS scene's own role tagging.

use engine_renderer::lit_scene::{LightParams, Material, RouteWaypoint, SceneBox};
use engine_renderer::proxy_instances::{InstanceTransform, ProxyInstance};
use engine_renderer::shadows::ShadowBias;
use engine_renderer::sky::atmosphere_at;
use engine_renderer::{
    GroundRoles, PASS_FAMILY_MATERIAL_COUNT, SceneBounds, SceneDescription, SceneMesh, SkyFrame,
};

/// The slab's half extent: the floor quad and the wall U span
/// ±[`GARAGE_EXTENT`] horizontally (20 m × 20 m).
pub const GARAGE_EXTENT: f32 = 10.0;
/// The floor's height (and the wet mirror's plane).
const GROUND_Y: f32 = 0.0;
/// The wall U's height (the masonry rises to 4 m; the dusk sky
/// shows above it and through the open +Z side).
const WALL_HEIGHT: f32 = 4.0;

/// The garage's mass table (the authored box table the shadow-map
/// known-answer gate runs the CPU raycaster over — the render_shadows
/// discipline, parameterized by THIS scene's own content): the U of
/// walls (material 1 — back wall closing the −Z side, left and right
/// walls flanking ±X, all spanning the full slab so the shadow
/// cascade's box AABB covers the floor quad exactly) and the
/// furniture (material 2 — a workbench against the back wall and a
/// two-crate storage cluster in the back-left corner, behind the
/// scripted orbit's door-side sweep).
const MASSES: [SceneBox; 6] = [
    // Back wall: the −Z side closed, spanning the slab's full width.
    SceneBox {
        min: [-GARAGE_EXTENT, GROUND_Y, -GARAGE_EXTENT],
        max: [GARAGE_EXTENT, WALL_HEIGHT, -GARAGE_EXTENT + 1.0],
        material: 1,
    },
    // Left wall: the −X side, spanning the full depth.
    SceneBox {
        min: [-GARAGE_EXTENT, GROUND_Y, -GARAGE_EXTENT],
        max: [-GARAGE_EXTENT + 1.0, WALL_HEIGHT, GARAGE_EXTENT],
        material: 1,
    },
    // Right wall: the +X side, spanning the full depth.
    SceneBox {
        min: [GARAGE_EXTENT - 1.0, GROUND_Y, -GARAGE_EXTENT],
        max: [GARAGE_EXTENT, WALL_HEIGHT, GARAGE_EXTENT],
        material: 1,
    },
    // Workbench: against the back wall's right half, workshop height.
    SceneBox {
        min: [2.0, GROUND_Y, -8.0],
        max: [8.0, 1.0, -6.0],
        material: 2,
    },
    // Storage crates: a two-box cluster in the back-left corner
    // (beside the bench, behind the scripted orbit's door-side
    // sweep — the camera's quadrant stays clear by authoring).
    SceneBox {
        min: [-7.0, GROUND_Y, -3.0],
        max: [-4.0, 1.5, -1.0],
        material: 2,
    },
    SceneBox {
        min: [-6.0, GROUND_Y, -6.5],
        max: [-3.0, 1.8, -4.0],
        material: 2,
    },
];

/// The garage's material table — THREE entries (the pass
/// families' compiled shape; the assert below pins it):
/// 0 — the oiled concrete floor (the wet receiver: broad dim
///     specular that the Schlick-blended reflection reads as a
///     polished slab),
/// 1 — the painted masonry walls,
/// 2 — the furniture (workbench timber / crate steel).
/// Every shininess is a power of two (the repeated-squaring
/// contract the CPU oracle and the shader share).
const MATERIALS: [Material; 3] = [
    Material {
        texture: 0,
        specular: [0.5, 0.55, 0.6],
        shininess: 64,
    },
    Material {
        texture: 1,
        specular: [0.15, 0.15, 0.18],
        shininess: 8,
    },
    Material {
        texture: 2,
        specular: [0.3, 0.28, 0.2],
        shininess: 32,
    },
];

// The authored table carries the pass families' compiled shape.
const _: () = assert!(MATERIALS.len() == PASS_FAMILY_MATERIAL_COUNT);

/// The garage's work lamp: one warm directional light high toward
/// the back-left of the slab — the shadows it casts fall toward
/// the open door, in view of the scripted camera. AUTHORED
/// content (not an `atmosphere_at` preset): the second scene's
/// lighting is its own.
pub fn garage_light() -> LightParams {
    LightParams {
        dir_toward_light: [-0.4, 0.85, -0.35],
        light_color: [1.0, 0.88, 0.7],
        ambient: [0.14, 0.14, 0.18],
    }
}

/// The dusk sky over the open door: the atmosphere's own late-day
/// palette (the `atmosphere_at` gradient at t = 0.85) with the
/// sun disc low toward the west — visible above the walls and
/// through the opening. The light stays the authored work lamp
/// (the sky's atmosphere and the lamp are deliberately
/// independent: an interior scene's lamps need not track the sky).
pub fn garage_sky() -> SkyFrame {
    SkyFrame {
        palette: atmosphere_at(0.85).sky,
        disc_dir: [-0.8, 0.25, -0.3],
    }
}

/// The garage's shadow-bias policy (documented per the API
/// contract, the corner demo's own discipline): the interior's
/// axis-aligned walls under the work lamp stay acne-free at the
/// 1024² cascade with the corner route's measured values — the
/// scene's faces are no steeper than the corner's.
pub fn garage_bias() -> ShadowBias {
    ShadowBias {
        constant: 0.0015,
        slope_scale: 0.002,
    }
}

/// The shadow cascade's world AABB over the garage's mass table —
/// the value the garage's shadowed submissions declare (the box
/// table spans the slab exactly, so the cascade covers the floor
/// quad the shadows fall on).
pub fn garage_bounds() -> SceneBounds {
    let (min, max) = engine_renderer::shadows::scene_bounds(&MASSES);
    SceneBounds { min, max }
}

pub fn garage_masses() -> &'static [SceneBox] {
    &MASSES
}

/// Assembles the garage's [`SceneDescription`] — the content the
/// garage demo and the render_garage battery attach through
/// [`engine_renderer::Renderer::attach_scene`]. Pure and
/// deterministic: the same bytes on every call (pinned by the
/// module's determinism test).
pub fn garage_scene_description() -> SceneDescription {
    let (vertices, draws) = engine_renderer::lit_mesh::mesh_from_boxes(
        &MASSES,
        GARAGE_EXTENT,
        GROUND_Y,
        MATERIALS.len(),
    );
    SceneDescription {
        mesh: SceneMesh {
            vertices,
            draws: draws.clone(),
        },
        materials: MATERIALS.to_vec(),
        textures: engine_renderer::proc_texture::ProcTextureSet {
            textures: (0..engine_renderer::proc_texture::TEXTURE_COUNT)
                .map(engine_renderer::proc_texture::procedural_texture)
                .collect(),
        },
        bounds: garage_bounds(),
        // The wet roles over the built mesh — the garage's own
        // tagging: the floor quad (material 0's range) receives;
        // the walls (material 1) and the furniture (material 2)
        // reflect; the mirror is the floor's plane.
        ground: GroundRoles {
            receiver: draws[0].clone(),
            receiver_plane_y: GROUND_Y,
            reflected: [draws[1].clone(), draws[2].clone()],
        },
    }
}

/// The demo's proxy materials (the caller policy
/// [`engine_renderer::ProxyPopulation`] carries): the mechanics
/// with the painted-masonry set (matte overalls), the van with
/// the furniture set (paint sheen). Documented choice, not engine
/// policy.
pub const GARAGE_NPC_MATERIAL: u32 = 1;
pub const GARAGE_VEHICLE_MATERIAL: u32 = 2;

/// The six mechanics' placements, in fixed order (the two
/// workbench standers, then the four mid-room walkers). Tints are
/// a hand-authored workshop palette (deliberately not a gradient
/// — pure data, the corner table's own discipline).
#[rustfmt::skip]
const NPC_TABLE: [ProxyInstance; 6] = [
    // Workbench standers: at the bench's front face (z = -6),
    // facing −Z into the bench (turn 0 — zero yaw looks down −Z).
    ProxyInstance { transform: InstanceTransform { pos: [3.0, 0.02, -4.8], turn: 0,     scale: 1.0 }, tint: [0.25, 0.30, 0.45], pose_index: 0, alpha: 1.0 },
    ProxyInstance { transform: InstanceTransform { pos: [5.5, 0.02, -4.6], turn: 0,     scale: 1.0 }, tint: [0.50, 0.40, 0.30], pose_index: 1, alpha: 1.0 },
    // Mid-room walkers: two crossing the open floor east/west
    // (turn 16384 faces +X, 49152 faces −X), two walking −Z into
    // the workshop from the door side.
    ProxyInstance { transform: InstanceTransform { pos: [-3.0, 0.02,  3.0], turn: 16384, scale: 1.0 }, tint: [0.60, 0.55, 0.35], pose_index: 2, alpha: 1.0 },
    ProxyInstance { transform: InstanceTransform { pos: [ 3.5, 0.02,  4.5], turn: 49152, scale: 1.0 }, tint: [0.35, 0.50, 0.55], pose_index: 3, alpha: 1.0 },
    ProxyInstance { transform: InstanceTransform { pos: [-5.0, 0.02,  6.0], turn: 0,     scale: 1.0 }, tint: [0.70, 0.35, 0.30], pose_index: 4, alpha: 1.0 },
    ProxyInstance { transform: InstanceTransform { pos: [ 5.0, 0.02,  6.5], turn: 0,     scale: 1.0 }, tint: [0.45, 0.45, 0.50], pose_index: 5, alpha: 1.0 },
];

/// The six mechanics' walk-cycle dispositions (the placement
/// table's phase column — the corner population's own `Phase`
/// vocabulary): the standers stand (the rest pose), the walkers
/// walk at spread phase offsets so the frozen-mid-stride crowd
/// never marches in lockstep.
#[rustfmt::skip]
pub fn garage_phases() -> Vec<crate::corner_population::Phase> {
    use crate::corner_population::Phase;
    vec![
        Phase::Stand, Phase::Stand,
        Phase::Walk(0), Phase::Walk(16384),
        Phase::Walk(32768), Phase::Walk(49152),
    ]
}

/// The garage's parked van: nose-in against the right wall, in
/// the walkers' clear lane — the population's vehicle kind.
#[rustfmt::skip]
const VEHICLE_TABLE: [ProxyInstance; 1] = [
    ProxyInstance { transform: InstanceTransform { pos: [6.0, 0.0, -3.0], turn: 16384, scale: 1.0 }, tint: [0.55, 0.60, 0.62], pose_index: 0, alpha: 1.0 },
];

/// The garage population: 6 NPCs then 1 vehicle, in the fixed
/// buffer order (the repack's npc-then-vehicle order — the entry
/// order is part of the byte pin, the corner table's discipline).
/// Since P5 §3.2(e) the returned npc instances carry the
/// STANDER-DEDUP pose indices (see
/// [`crate::corner_population::stander_dedup_block_indices`]): the
/// two workbench standers share palette block 0, the four walkers
/// carry 1..4 — the authored sequential values remapped mechanically
/// at construction (the corner table's own versioned claim; the
/// pixels are identical by the skin-matrix identity proof).
pub fn garage_population() -> (Vec<ProxyInstance>, Vec<ProxyInstance>) {
    let mut npc = NPC_TABLE.to_vec();
    let mapping = crate::corner_population::stander_dedup_block_indices(
        &garage_phases()
            .iter()
            .map(|phase| matches!(phase, crate::corner_population::Phase::Stand))
            .collect::<Vec<_>>(),
    );
    for (instance, &block) in npc.iter_mut().zip(mapping.iter()) {
        instance.pose_index = block;
    }
    (npc, VEHICLE_TABLE.to_vec())
}

// ---- the scripted camera ----

/// The garage route's orbit: the camera sweeps the door-side
/// (open +Z) quadrant at eye height, facing the workshop — the
/// workbench, the crate cluster, the population, and the
/// lamp-cast shadows stay in frame while the walls never pass
/// between the eye and the room. θ from −60° to +60° over
/// [`GARAGE_ROUTE_FRAMES`] frames, then the exit summary (the
/// corner route's own fixed-cadence shape at garage scale).
pub const GARAGE_ROUTE_FRAMES: u64 = 240;
/// The orbit's radius and the look-at target (the room's center,
/// workbench height).
const ORBIT_RADIUS: f32 = 7.5;
const ORBIT_LOOK_AT: [f32; 3] = [0.0, 1.2, -2.0];

/// The scripted camera's waypoint for presented frame `k`
/// (deterministic: frame k always shows the same vantage — the
/// corner route's own contract, at garage scale; `k` past the
/// route's end holds the final vantage, so bounded and unbounded
/// runs share the sweep).
pub fn garage_camera(k: u64) -> RouteWaypoint {
    let span = 120_f32.to_radians(); // −60°..+60°
    let t = (k.min(GARAGE_ROUTE_FRAMES) as f32 / GARAGE_ROUTE_FRAMES as f32) * span - span / 2.0;
    RouteWaypoint {
        position: [ORBIT_RADIUS * t.sin(), 1.7, ORBIT_RADIUS * t.cos()],
        look_at: ORBIT_LOOK_AT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The description validates — the attach path accepts exactly
    /// this (the admission contract's whole point: an authored
    /// scene a renderer has never seen, coherent on the first
    /// attach).
    #[test]
    fn the_garage_description_validates() {
        garage_scene_description()
            .validate()
            .expect("the garage content is valid");
    }

    /// The builder's assembly is pure: two calls produce identical
    /// descriptions (the attach path's determinism — the same GPU
    /// objects, the same frames, on every construction).
    #[test]
    fn the_garage_description_is_deterministic() {
        assert_eq!(
            garage_scene_description(),
            garage_scene_description(),
            "two assemblies are byte-identical"
        );
    }

    /// THE SECOND-SCENE SEPARATION WITNESS (the design's §7, leg
    /// 3's unit half): the garage's description is NOT the
    /// corner's — different vertices, different draw table,
    /// different materials, different bounds — while BOTH
    /// validate against the SAME admission contract and BOTH fit
    /// the pass families' compiled three-material shape. The
    /// boundary is real content diversity, not a re-skin.
    #[test]
    fn the_garage_is_a_genuinely_different_scene_from_the_corner() {
        let garage = garage_scene_description();
        let corner = crate::corner_scene::corner_scene_description();
        assert_ne!(
            garage.mesh.vertices, corner.mesh.vertices,
            "the garage's mesh is its own"
        );
        assert_ne!(
            garage.mesh.draws, corner.mesh.draws,
            "the garage's draw table is its own"
        );
        assert_ne!(garage.materials, corner.materials, "own material table");
        assert_ne!(garage.bounds, corner.bounds, "own shadow bounds");
        assert_ne!(
            garage.ground.reflected, corner.ground.reflected,
            "own wet reflected roles"
        );
        // Both tables fit the compiled family shape — the shared
        // contract both scenes author against.
        assert_eq!(garage.materials.len(), PASS_FAMILY_MATERIAL_COUNT);
        assert_eq!(corner.materials.len(), PASS_FAMILY_MATERIAL_COUNT);
        // The garage's bounds sit INSIDE the corner's world (a
        // 20 m slab inside the 40 m intersection) — the cascade
        // math consumes a tighter box.
        for axis in 0..3 {
            assert!(
                garage.bounds.min[axis] >= corner.bounds.min[axis]
                    && garage.bounds.max[axis] <= corner.bounds.max[axis],
                "axis {axis}: the garage is the smaller world"
            );
        }
    }

    /// The population's pose contract (the
    /// [`engine_renderer::ProxyPopulation`] validation the
    /// submission performs): every npc's pose index selects a
    /// supplied block under the garage's own phase column — the
    /// STANDER-DEDUP palette (P5 §3.2(e)): the two standers share
    /// block 0, the four walkers carry 1..4, so the palette is 5
    /// blocks for 6 npcs — and the pose evaluation is deterministic
    /// in the frame counter (the same pure machinery the corner
    /// crowd uses, over the garage's authored table).
    #[test]
    fn the_garage_population_satisfies_the_pose_contract() {
        let (npcs, vehicles) = garage_population();
        let phases = garage_phases();
        assert_eq!(npcs.len(), phases.len(), "one phase per npc entry");
        let mut poses = Vec::new();
        crate::corner_population::evaluate_population_poses(0, &phases, &mut poses);
        // The dedup palette: [shared rest] ∪ {4 walker blocks}.
        assert_eq!(
            poses.len(),
            5,
            "the palette: one shared rest + four walkers"
        );
        for (npc, block_count) in npcs.iter().zip(std::iter::repeat(poses.len())) {
            assert!(
                (npc.pose_index as usize) < block_count,
                "pose index {} selects a supplied block",
                npc.pose_index
            );
        }
        // The dedup mapping's own pin: standers at 0, walker j at 1+j.
        let expected = [0_u32, 0, 1, 2, 3, 4];
        for (npc, &block) in npcs.iter().zip(expected.iter()) {
            assert_eq!(npc.pose_index, block, "the garage table's dedup mapping");
        }
        // Determinism: frame k always produces byte-identical
        // blocks (the FNV-stable machinery the corner pins).
        let mut a = Vec::new();
        let mut b = Vec::new();
        crate::corner_population::evaluate_population_poses(37, &phases, &mut a);
        crate::corner_population::evaluate_population_poses(37, &phases, &mut b);
        assert_eq!(a, b, "frame 37's blocks are byte-identical");
        // The frame advances the walkers' poses (the shared rest
        // block is frame-invariant).
        let mut later = Vec::new();
        crate::corner_population::evaluate_population_poses(38, &phases, &mut later);
        assert_ne!(a[2], later[2], "walker 1's pose advances with the clock");
        assert_eq!(a[0], later[0], "the shared rest block is frame-invariant");
        // The vehicle kind is populated (the van rides the same
        // submission).
        assert_eq!(vehicles.len(), 1, "the parked van");
    }

    /// The scripted camera: every route frame's eye stays inside
    /// the slab's door quadrant (never inside a wall box), above
    /// the floor at eye height, with the look-at target in front
    /// (the corner route's own in-frame property, at garage
    /// scale).
    #[test]
    fn the_garage_camera_stays_in_the_open_quadrant() {
        for k in 0..=GARAGE_ROUTE_FRAMES {
            let camera = garage_camera(k);
            let [x, y, z] = camera.position;
            assert!((y - 1.7).abs() < 1e-6, "eye height");
            assert!(
                x.abs() <= ORBIT_RADIUS + 1e-6 && z >= 0.0,
                "the door quadrant"
            );
            for (index, b) in MASSES.iter().enumerate() {
                let inside = (x > b.min[0] && x < b.max[0])
                    && (y > b.min[1] && y < b.max[1])
                    && (z > b.min[2] && z < b.max[2]);
                assert!(!inside, "frame {k}: the eye is inside mass {index}");
            }
            assert_ne!(camera.position, camera.look_at, "the target is distinct");
        }
    }
}
