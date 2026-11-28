//! The corner's authored scene content (P4 S6 — the design's §8-S2
//! "S6 moves the content to the slice side (its own
//! `SceneDescription` builder)"): the four building masses, the
//! three-entry material table, the ground plate, the albedo set,
//! the shadow bounds, and the wet ground's roles, assembled into
//! the attach-time [`SceneDescription`] the corner demo and every
//! corner gate attach through
//! [`engine_renderer::Renderer::attach_scene`].
//!
//! THE MIGRATION'S LAST MOVE: from S2 to S6 this content existed
//! TWICE — renderer-side (the `corner_content.rs` migration shim
//! the deleted `Corner` construction preference attached) and
//! reference-side (`engine_renderer::lit_scene`'s tables, the CPU
//! oracle's own source: the shadow raycaster's boxes, the capture
//! gates' material pins). S6 deleted the shim and the preference;
//! THIS module is now the one builder, and the drift gate below
//! still pins it to `lit_scene`'s reference tables — the CPU
//! oracle and the attached GPU content cannot silently diverge.
//!
//! WHAT LIVES HERE (authored content, not mechanism): the four
//! building masses, the three-entry material table, the ground
//! plate's extent and height, the albedo set's texture indices,
//! and the wet ground's role tagging. WHAT DOES NOT: the mesh
//! builder (`engine_renderer::lit_mesh::mesh_from_boxes` — the
//! shared quad/wall emission), the texture generator
//! (`proc_texture::procedural_texture`), and the bounds walk
//! (`shadows::scene_bounds`) — pure deterministic mechanism, one
//! copy, parameterized by the content.

use engine_renderer::lit_scene::{Material, SceneBox};
use engine_renderer::proc_texture::ProcTextureSet;
use engine_renderer::{
    GroundRoles, PASS_FAMILY_MATERIAL_COUNT, Renderer, RendererError, SceneBounds,
    SceneDescription, SceneMesh,
};

/// The intersection's world extent — the corner authoring's copy of
/// `lit_scene::SCENE_EXTENT` (40 m × 40 m, the "One Corner of the
/// City" definition). The drift gate pins the copies equal.
const SCENE_EXTENT: f32 = 40.0;
/// The ground plate's height — the corner authoring's copy of
/// `lit_scene::GROUND_Y` (and the wet mirror's plane).
const GROUND_Y: f32 = 0.0;

/// The static intersection: the four building masses framing the
/// corner (two tall across the streets, two mid on the corner),
/// leaving the 8 m road cross open — the corner authoring's copy
/// of `lit_scene::intersection_boxes()`'s table (same values, same
/// order; the drift gate pins them equal).
const MASSES: [SceneBox; 4] = [
    // Northeast mass (tall): x in [half+sidewalk, e], z in [half+sidewalk, e].
    SceneBox {
        min: [6.0, GROUND_Y, 6.0],
        max: [SCENE_EXTENT, 18.0, SCENE_EXTENT],
        material: 1,
    },
    // Southwest mass (tall): x in [-e, -(half+sidewalk)], z in [-e, -(half+sidewalk)].
    SceneBox {
        min: [-SCENE_EXTENT, GROUND_Y, -SCENE_EXTENT],
        max: [-6.0, 22.0, -6.0],
        material: 1,
    },
    // Southeast mass (mid, dark base): x in [half+sidewalk, e], z in [-e, -(half+sidewalk)].
    SceneBox {
        min: [6.0, GROUND_Y, -SCENE_EXTENT],
        max: [SCENE_EXTENT, 9.0, -6.0],
        material: 2,
    },
    // Northwest mass (mid): x in [-e, -(half+sidewalk)], z in [half+sidewalk, e].
    SceneBox {
        min: [-SCENE_EXTENT, GROUND_Y, 6.0],
        max: [-6.0, 12.0, SCENE_EXTENT],
        material: 2,
    },
];

/// The corner's material table — the authoring's copy of
/// `lit_scene::materials()`'s values (wet asphalt ground, concrete
/// facade, dark stone base; index-stable, the draw ranges and the
/// population's per-kind indices select from it).
const MATERIALS: [Material; 3] = [
    // 0: wet asphalt ground (the wet receiver's material).
    Material {
        texture: 0,
        specular: [0.9, 0.9, 1.0],
        shininess: 64,
    },
    // 1: building facade (concrete).
    Material {
        texture: 1,
        specular: [0.2, 0.2, 0.22],
        shininess: 8,
    },
    // 2: building base (dark stone).
    Material {
        texture: 2,
        specular: [0.25, 0.25, 0.25],
        shininess: 16,
    },
];

// The authoring's table carries the pass families' compiled shape
// (three materials — see PASS_FAMILY_MATERIAL_COUNT).
const _: () = assert!(MATERIALS.len() == PASS_FAMILY_MATERIAL_COUNT);

/// The shadow cascade's world AABB over the corner's mass table —
/// the value the corner route's shadowed frames declare in the
/// submission's [`engine_renderer::SceneFrame`] (the S1-era
/// shadowed entries computed it inline from
/// `lit_scene::intersection_boxes()`; the drift gate pins the two
/// equal). Plain `Copy` data the caller echoes per shadowed frame.
pub fn corner_bounds() -> SceneBounds {
    let (min, max) = engine_renderer::shadows::scene_bounds(&MASSES);
    SceneBounds { min, max }
}

/// Assembles the corner's [`SceneDescription`] — the content the
/// corner demo and every corner gate attach through
/// [`Renderer::attach_scene`]. Pure and deterministic: the same
/// bytes on every call, every platform (pinned by the module's
/// determinism test).
pub fn corner_scene_description() -> SceneDescription {
    let (vertices, draws) = engine_renderer::lit_mesh::mesh_from_boxes(
        &MASSES,
        SCENE_EXTENT,
        GROUND_Y,
        MATERIALS.len(),
    );
    SceneDescription {
        mesh: SceneMesh {
            vertices,
            draws: draws.clone(),
        },
        materials: MATERIALS.to_vec(),
        textures: ProcTextureSet {
            textures: (0..engine_renderer::proc_texture::TEXTURE_COUNT)
                .map(engine_renderer::proc_texture::procedural_texture)
                .collect(),
        },
        bounds: corner_bounds(),
        // The wet roles over the built mesh — the corner authoring's
        // own tagging (the values the pre-S2 `CornerResources`::
        // receiver_range()/reflected_ranges() derived from the draw
        // table): the ground quad (material 0's range) receives; the
        // two building masses (materials 1 and 2) reflect; the
        // mirror is the ground plate's plane.
        ground: GroundRoles {
            receiver: draws[0].clone(),
            receiver_plane_y: GROUND_Y,
            reflected: [draws[1].clone(), draws[2].clone()],
        },
    }
}

/// Attaches the corner description to a renderer — the demo and the
/// corner gates' shared construction step (what the deleted `Corner`
/// construction preference used to do inside `Renderer::new`). The
/// attach validates whole first (a refusal — impossible for this
/// table, pinned below — builds nothing and leaves the renderer
/// untouched).
pub fn attach_corner_scene(
    renderer: &mut Renderer,
) -> Result<engine_renderer::SceneHandle, RendererError> {
    renderer.attach_scene(corner_scene_description())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE CORNER CONTENT EQUALITY GATE (the S2 shim-equality gate's
    /// successor, moved to the slice side at S6 when the shim was
    /// deleted): this builder's description and `lit_scene`'s
    /// REFERENCE tables (the CPU oracle's own source — the shadow
    /// raycaster's boxes, the capture gates' material pins) must
    /// produce IDENTICAL content, field for field. The reference
    /// side below is built from the SAME expressions the pre-S2
    /// render path read inline (`mesh_from_boxes`'s output over
    /// `intersection_boxes()`, `materials()`, `procedural_texture`,
    /// and the `scene_bounds(intersection_boxes())` the shadowed
    /// entries computed), so if either copy drifts, this fails — and
    /// with it the byte-identity argument for every corner route
    /// (the GPU resources are built from the description; equal
    /// descriptions build equal resources and equal frames).
    #[test]
    fn the_corner_description_is_the_lit_scene_reference_content() {
        let scene = corner_scene_description();
        // The mesh: the shared builder over lit_scene's box table —
        // the exact call the pre-S2 render path made (its extent,
        // ground height, and material count were lit_scene's own
        // constants).
        let (vertices, draws) = engine_renderer::lit_mesh::mesh_from_boxes(
            engine_renderer::lit_scene::intersection_boxes(),
            engine_renderer::lit_scene::SCENE_EXTENT,
            engine_renderer::lit_scene::GROUND_Y,
            engine_renderer::lit_scene::materials().len(),
        );
        assert_eq!(scene.mesh.vertices, vertices, "the mesh vertices");
        assert_eq!(scene.mesh.draws, draws, "the per-material draw ranges");
        // The material table (the population validation's bound and
        // the per-material descriptor tails' source).
        assert_eq!(
            scene.materials,
            engine_renderer::lit_scene::materials().to_vec(),
            "the material table"
        );
        // The albedo set (the texture uploads' source).
        let textures: Vec<Vec<u8>> = (0..engine_renderer::proc_texture::TEXTURE_COUNT)
            .map(engine_renderer::proc_texture::procedural_texture)
            .collect();
        assert_eq!(scene.textures.textures, textures, "the albedo set");
        // The shadow bounds (the shadowed frames' cascade input).
        let (min, max) = engine_renderer::shadows::scene_bounds(
            engine_renderer::lit_scene::intersection_boxes(),
        );
        assert_eq!(
            scene.bounds,
            SceneBounds { min, max },
            "the shadow-cascade bounds"
        );
        // The wet ground roles (the values the pre-S2
        // receiver_range()/reflected_ranges() derived from the draw
        // table, and the mirror plane the reflection read as
        // lit_scene::GROUND_Y).
        assert_eq!(
            scene.ground.receiver, draws[0],
            "the wet receiver (the old receiver_range())"
        );
        assert_eq!(
            scene.ground.receiver_plane_y,
            engine_renderer::lit_scene::GROUND_Y
        );
        assert_eq!(
            scene.ground.reflected,
            [draws[1].clone(), draws[2].clone()],
            "the reflected masses (the old reflected_ranges())"
        );
        // The description is coherent — the attach path accepts it.
        scene.validate().expect("the corner description validates");
    }

    /// The builder's assembly is pure: two calls produce identical
    /// descriptions (the attach path's determinism — the same GPU
    /// objects, the same frames, on every construction).
    #[test]
    fn the_corner_description_is_deterministic() {
        assert_eq!(
            corner_scene_description(),
            corner_scene_description(),
            "two assemblies are byte-identical"
        );
    }
}
