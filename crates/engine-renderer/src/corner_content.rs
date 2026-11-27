//! THE P4 S2 MIGRATION SHIM — **TEMPORARY, DELETED AT S6** (the
//! design's §8-S2 "renderer-side `corner_content.rs` module
//! explicitly marked as the migration's removable shim").
//!
//! The corner's authored scene content, held HERE so the render
//! path reads it through the attach-time
//! [`SceneDescription`](crate::scene_description::SceneDescription)
//! handle instead of renderer-internal constants — while the CPU
//! reference model (the shadow raycaster's box tables, the
//! capture-gate oracles, the slice demos) keeps reading
//! `lit_scene`/`lit_mesh`/`proc_texture` unchanged. The content
//! therefore exists TWICE inside this crate during the migration
//! window (this shim's tables and the reference modules' own),
//! exactly the design's "content duplicated temporarily …
//! bounded and deleted" shape: S6 moves the content to the slice
//! side (its own `SceneDescription` builder), deletes this shim,
//! and the drift gate below keeps the two copies provably
//! identical for as long as both exist.
//!
//! WHAT LIVES HERE (authored content, not mechanism): the four
//! building masses (the box table `lit_scene::intersection_boxes`
//! also holds), the three-entry material table
//! (`lit_scene::materials`'s values), the ground plate's extent
//! and height (`lit_scene`'s `SCENE_EXTENT`/`GROUND_Y`), the
//! albedo set's texture indices, and the wet ground's role
//! tagging (receiver = material 0's ground-quad range at the
//! mirror plane y = 0; reflected = materials 1 and 2's mass
//! ranges). WHAT DOES NOT: the mesh builder
//! (`lit_mesh::mesh_from_boxes` — the shared quad/wall emission
//! both this shim and the drift gate's reference call), the
//! texture generator (`proc_texture::procedural_texture`), and
//! the bounds walk (`shadows::scene_bounds`) — pure deterministic
//! mechanism, one copy, parameterized by the content.
//!
//! THE CONSUMER: `Renderer::new`'s `DemoScenePreference::Corner`
//! arm attaches this description at construction (the preference
//! adapter the design names — the construction-time upload the
//! preference used to perform inside `VulkanContext::new`, now
//! through the same `attach_scene` path every future consumer
//! uses). The demo (engine-slice) does NOT migrate in S2 — its
//! `render_corner*` routes keep working through this adapter; S6
//! moves the demo to `attach_scene` with its own builder and
//! deletes this module.

use crate::lit_scene::{Material, SceneBox};
use crate::proc_texture::ProcTextureSet;
use crate::scene_description::{GroundRoles, SceneDescription, SceneMesh};
use crate::submission::SceneBounds;

/// The intersection's world extent — the shim's copy of
/// `lit_scene::SCENE_EXTENT` (40 m × 40 m, the "One Corner of the
/// City" definition). The drift gate pins the copies equal.
const SCENE_EXTENT: f32 = 40.0;
/// The ground plate's height — the shim's copy of
/// `lit_scene::GROUND_Y` (and the wet mirror's plane).
const GROUND_Y: f32 = 0.0;

/// The static intersection: the four building masses framing the
/// corner (two tall across the streets, two mid on the corner),
/// leaving the 8 m road cross open — the shim's copy of
/// `lit_scene::intersection_boxes()`'s table (same values, same
/// order; the drift gate pins them equal while both exist).
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

/// The corner's material table — the shim's copy of
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

// The shim's tables carry the pass families' compiled shape
// (three materials — see PASS_FAMILY_MATERIAL_COUNT).
const _: () = assert!(MATERIALS.len() == crate::scene_description::PASS_FAMILY_MATERIAL_COUNT);

/// Assembles the corner's [`SceneDescription`] — the exact content
/// the `Corner` construction preference attaches (and the exact
/// content the pre-S2 render path built implicitly from the
/// reference tables; the drift gate below proves those two
/// sentences cannot drift apart while both constructors exist).
/// Pure and deterministic: the same bytes on every call, every
/// platform.
pub(crate) fn corner_scene_description() -> SceneDescription {
    let (vertices, draws) =
        crate::lit_mesh::mesh_from_boxes(&MASSES, SCENE_EXTENT, GROUND_Y, MATERIALS.len());
    SceneDescription {
        mesh: SceneMesh {
            vertices,
            draws: draws.clone(),
        },
        materials: MATERIALS.to_vec(),
        textures: ProcTextureSet {
            textures: (0..crate::proc_texture::TEXTURE_COUNT)
                .map(crate::proc_texture::procedural_texture)
                .collect(),
        },
        bounds: corner_bounds(),
        // The wet roles over the built mesh — the corner authoring's
        // own tagging (the values `CornerResources::receiver_range()`/
        // `reflected_ranges()` used to derive from the draw table):
        // the ground quad (material 0's range) receives; the two
        // building masses (materials 1 and 2) reflect; the mirror is
        // the ground plate's plane.
        ground: GroundRoles {
            receiver: draws[0].clone(),
            receiver_plane_y: GROUND_Y,
            reflected: [draws[1].clone(), draws[2].clone()],
        },
    }
}

/// The shadow cascade's world AABB over the shim's mass table —
/// the value the shadowed routes used to compute inline from
/// `lit_scene::intersection_boxes()` (the drift gate pins the two
/// equal).
fn corner_bounds() -> SceneBounds {
    let (min, max) = crate::shadows::scene_bounds(&MASSES);
    SceneBounds { min, max }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE S2 SHIM-EQUALITY GATE (the design's §8-S2 drift-proof):
    /// while the corner content exists twice — this shim's tables
    /// (the attach path's source) and `lit_scene`/`lit_mesh`/
    /// `proc_texture`'s reference tables (the CPU oracle's source:
    /// the shadow raycaster's boxes, the capture gates' material
    /// pins) — this test proves the two constructors produce
    /// IDENTICAL [`SceneDescription`] content, field for field. The
    /// reference side below is built from the SAME expressions the
    /// pre-S2 render path read inline (`corner_mesh()`'s output,
    /// `materials()`, `procedural_texture`, and the
    /// `scene_bounds(intersection_boxes())` the shadowed entries
    /// computed), so if either copy drifts, this fails — and with
    /// it the byte-identity argument for every existing route (the
    /// GPU resources are built from the description; equal
    /// descriptions build equal resources and equal frames).
    #[test]
    fn the_shim_description_is_the_lit_scene_reference_content() {
        let shim = corner_scene_description();
        // The mesh: the shared builder over lit_scene's box table —
        // the exact call `lit_mesh::corner_mesh()` made (its extent,
        // ground height, and material count were lit_scene's own
        // constants).
        let (vertices, draws) = crate::lit_mesh::mesh_from_boxes(
            crate::lit_scene::intersection_boxes(),
            crate::lit_scene::SCENE_EXTENT,
            crate::lit_scene::GROUND_Y,
            crate::lit_scene::materials().len(),
        );
        assert_eq!(shim.mesh.vertices, vertices, "the mesh vertices");
        assert_eq!(shim.mesh.draws, draws, "the per-material draw ranges");
        // The material table (renderer.rs's validation bound and the
        // per-material descriptor tails' source).
        assert_eq!(
            shim.materials,
            crate::lit_scene::materials().to_vec(),
            "the material table"
        );
        // The albedo set (corner.rs's texture uploads' source).
        let textures: Vec<Vec<u8>> = (0..crate::proc_texture::TEXTURE_COUNT)
            .map(crate::proc_texture::procedural_texture)
            .collect();
        assert_eq!(shim.textures.textures, textures, "the albedo set");
        // The shadow bounds (the shadowed routes' cascade input).
        let (min, max) = crate::shadows::scene_bounds(crate::lit_scene::intersection_boxes());
        assert_eq!(
            shim.bounds,
            SceneBounds { min, max },
            "the shadow-cascade bounds"
        );
        // The wet ground roles (the values CornerResources's
        // receiver_range()/reflected_ranges() derived from the draw
        // table, and the mirror plane reflection_draw read as
        // lit_scene::GROUND_Y).
        assert_eq!(
            shim.ground.receiver, draws[0],
            "the wet receiver (the old receiver_range())"
        );
        assert_eq!(shim.ground.receiver_plane_y, crate::lit_scene::GROUND_Y);
        assert_eq!(
            shim.ground.reflected,
            [draws[1].clone(), draws[2].clone()],
            "the reflected masses (the old reflected_ranges())"
        );
        // The description is coherent — the attach path accepts it.
        shim.validate().expect("the corner description validates");
    }

    /// The shim's assembly is pure: two calls produce identical
    /// descriptions (the attach path's determinism — the same GPU
    /// objects, the same frames, on every construction).
    #[test]
    fn the_shim_description_is_deterministic() {
        assert_eq!(
            corner_scene_description(),
            corner_scene_description(),
            "two assemblies are byte-identical"
        );
    }
}
