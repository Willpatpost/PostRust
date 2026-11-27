//! The attach-time scene description (P4 D2 §3.2, stage S2): the
//! immutable, attach-once AUTHORED content one renderer draws —
//! the mesh (vertices + per-material draw ranges), the material
//! table, the procedural albedo set, the shadow cascade's world
//! bounds, and the wet ground's role-tagged ranges. Everything
//! here is DATA the scene author supplies; the renderer's lighting
//! EQUATION ([`crate::lit_scene::lit_surface_color`]), the
//! [`crate::lit_scene::LightParams`]/[`Material`] types, and
//! [`crate::shadows::cascade_for`] stay renderer-owned lighting
//! semantics, not content.
//!
//! THE SEPARATION THIS MODULE MAKES: before S2 the corner's
//! geometry/materials/bounds/ground-roles were renderer-internal
//! constants (`lit_scene::intersection_boxes()`/`materials()`,
//! `lit_mesh::corner_mesh()`, and the draw ranges
//! `vulkan::corner::CornerResources` derived) read implicitly by
//! the render path. From S2 the render path reads ONE attached
//! description — [`crate::Renderer::attach_scene`] hands it over,
//! [`SceneHandle`] is the proof it exists — so authored content
//! stops being a renderer constant and becomes caller data (the
//! `DemoScenePreference::Corner` construction preference keeps
//! working through the migration shim, `corner_content.rs`, which
//! attaches the same corner content through this same API; S6
//! deletes the shim and the preference arm).
//!
//! VALIDATION (the capture-refusal discipline, attach-time form):
//! [`SceneDescription::validate`] checks the description WHOLE,
//! device-free, BEFORE any resource is built — a refusal builds
//! nothing and leaves the renderer untouched. The checks are the
//! structural contracts the pass machinery relies on: one draw
//! range per material in material order, the ranges contiguous
//! and covering the vertex buffer (the shadow depth pass draws
//! `[0, last.end)`), every material's albedo index inside the
//! attached set, every texture at the procedural generator's
//! byte contract, sane bounds, and the wet roles inside the mesh.
//! ONE boundary is a compile-time shape, not author freedom: the
//! pass families' descriptor sets (the shadowed, skinned, and
//! reflection per-material sets) are built for
//! [`PASS_FAMILY_MATERIAL_COUNT`] materials — the corner-era
//! table — so attach refuses any other count (generalizing those
//! families is the migration's S6 step; the second scene of the
//! design's §7 fits the shape).
//!
//! ZERO-COST PLACEMENT: this is ATTACH-TIME data — `Vec`-backed,
//! built once, consumed once (the GPU copies live in the
//! context's corner resources; the renderer retains only the
//! distilled facts — the handle's material count and the
//! bounds). The per-frame submission stays borrow-only (see
//! [`crate::submission`]).

use std::ops::Range;

use crate::lit_scene::Material;
use crate::submission::SceneBounds;
use crate::vulkan::RendererError;

pub use crate::lit_mesh::LitVertex;

/// The material count the renderer's pass families are COMPILED
/// for: the lit/shadowed/skinned/reflection descriptor-set
/// families are built per material × slot against fixed-size
/// arrays (shadow_draw's and skinned_draw's `MATERIAL_COUNT`, the
/// reflection state's six reflected sets), and the depth-only
/// pass draws the whole mesh as one range — all corner-era
/// shapes. [`SceneDescription::validate`] refuses any other table
/// length at attach (fail-visible, never a silent truncation);
/// relaxing this is the migration's S6 generalization step (the
/// design's second scene, §7, authors a 3-material table and
/// fits as-is).
pub(crate) const PASS_FAMILY_MATERIAL_COUNT: usize = 3;

/// One static lit mesh: world-space triangle-list vertices plus
/// ONE draw range per material, in MATERIAL ORDER — range `i`
/// draws material `i`'s triangles through material `i`'s
/// descriptor set (the lit pipelines' bind-per-range shape). The
/// ranges are contiguous and cover the whole vertex buffer
/// (validated: the shadow depth pass draws `[0, last.end)` as the
/// whole scene; an author cannot leave a gap the depth pass
/// would miss). An empty range is legal (that material draws
/// nothing); the builder over an authored box table
/// ([`crate::lit_mesh::mesh_from_boxes`]) emits the ground quad
/// into material 0's range first — the wet receiver role's
/// position in the corner authoring.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneMesh {
    /// The triangle-list vertices (three per primitive, the lit
    /// pipelines' 32-byte input — see [`LitVertex`]).
    pub vertices: Vec<LitVertex>,
    /// One draw range per material, in material order: contiguous,
    /// starting at 0, ending at `vertices.len()`.
    pub draws: Vec<Range<u32>>,
}

/// The wet-ground reflection's role-tagged mesh ranges — the
/// generalization of the corner's `receiver_range()`/
/// `reflected_ranges()` coupling (P4 §3.2): which mesh range the
/// wet re-draw re-rasterizes (the RECEIVER — the ground quad,
/// drawn EQUAL-depth with the mirrored scene sampled at the same
/// screen position), the receiver's world-plane height (the
/// planar mirror's plane — the wet camera reflects across it),
/// and which ranges the mirror reflects toward the receiver (the
/// building masses, in MATERIAL ORDER: position `i` is material
/// `1 + i`, the reflection state's per-material set layout's
/// contract — its reflected sets are indexed
/// `material · slots + slot`).
///
/// The recorder stays a primitive over these ranges exactly as it
/// was over the corner's (finding 20, phase C): the placement
/// logic (the finding-14 ordered blend between the opaque kinds
/// and the translucent rain) is unchanged — only the range SOURCE
/// moved, from the corner's own draw table to the attached
/// description.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroundRoles {
    /// The receiver's draw range — non-empty (validated): the
    /// ground quad the wet re-draw re-rasterizes.
    pub receiver: Range<u32>,
    /// The receiver's world-plane height: the horizontal plane the
    /// wet camera mirrors across (`planar.rs`'s mirror functions
    /// consume it — the corner's value is the ground at y = 0).
    pub receiver_plane_y: f32,
    /// The reflected masses' draw ranges, in material order
    /// (position `i` ↔ material `1 + i`). May be empty (a range
    /// with `end == start` reflects nothing — a legal no-op draw);
    /// the corner's two building masses fill both.
    pub reflected: [Range<u32>; 2],
}

/// Immutable, attach-once scene content (P4 §3.2): everything the
/// renderer's lit/shadowed/wet/populated passes need that is
/// AUTHORED rather than per-frame. Built by ANY consumer (the
/// migration shim's corner builder, a future game's scene
/// author) and handed to [`crate::Renderer::attach_scene`], which
/// validates it whole ([`Self::validate`]) and uploads it once
/// through the corner resources machinery. The per-frame half —
/// cameras, lights, populations, effects — is the submission
/// contract's ([`crate::submission::SceneFrame`]); the shadow
/// cascade's BOUNDS live here at attach while the per-frame bias
/// and the cascade computation stay submission/renderer side.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneDescription {
    /// The static lit mesh (vertices + per-material draw ranges).
    pub mesh: SceneMesh,
    /// The material table, index-stable: the mesh's draw ranges
    /// and the population's per-kind material indices select from
    /// it. Exactly [`PASS_FAMILY_MATERIAL_COUNT`] entries
    /// (validated — the pass families' compiled shape).
    pub materials: Vec<Material>,
    /// The procedural albedo set (see
    /// [`crate::proc_texture::ProcTextureSet`]); each material's
    /// `texture` field indexes it.
    pub textures: crate::proc_texture::ProcTextureSet,
    /// The scene's world AABB — the shadow cascade's projection
    /// domain (the corner route's value folds
    /// `lit_scene::intersection_boxes()` through
    /// `shadows::scene_bounds`; now authored with the scene).
    pub bounds: SceneBounds,
    /// The wet ground's role-tagged ranges (see [`GroundRoles`]).
    pub ground: GroundRoles,
}

impl SceneDescription {
    /// Validates the description WHOLE, device-free, before any
    /// resource is built — the attach-time form of the
    /// capture-refusal discipline: a refusal builds nothing and
    /// leaves the renderer (and its context) untouched. Every
    /// check names the structural contract the pass machinery
    /// relies on; the error texts are the typed refusals callers
    /// match on.
    pub fn validate(&self) -> Result<(), RendererError> {
        let SceneDescription {
            mesh,
            materials,
            textures,
            bounds,
            ground,
        } = self;
        // ---- the material table's shape ----
        if materials.is_empty() {
            return Err(RendererError::Vulkan(
                "the scene material table is empty".into(),
            ));
        }
        if materials.len() != PASS_FAMILY_MATERIAL_COUNT {
            return Err(RendererError::Vulkan(format!(
                "the scene material table has {} entries; the pass families' \
                 descriptor sets are built for {} (the corner-era compile-time \
                 shape — generalizing them is the migration's S6 step)",
                materials.len(),
                PASS_FAMILY_MATERIAL_COUNT
            )));
        }
        // ---- the mesh's draw ranges: one per material, contiguous,
        // covering the buffer (the depth pass draws [0, last.end)). ----
        if mesh.vertices.is_empty() {
            return Err(RendererError::Vulkan("the scene mesh is empty".into()));
        }
        if mesh.draws.len() != materials.len() {
            return Err(RendererError::Vulkan(format!(
                "the scene mesh has {} draw ranges but the material table has \
                 {} entries (one range per material, in material order)",
                mesh.draws.len(),
                materials.len()
            )));
        }
        if mesh.draws[0].start != 0 {
            return Err(RendererError::Vulkan(format!(
                "the scene mesh's first draw range starts at {} (the ranges \
                 must start at vertex 0 — the shadow depth pass draws the \
                 whole buffer)",
                mesh.draws[0].start
            )));
        }
        for (index, pair) in mesh.draws.windows(2).enumerate() {
            if pair[0].end != pair[1].start {
                return Err(RendererError::Vulkan(format!(
                    "the scene mesh's draw ranges must be contiguous (range \
                     {index} ends at {} but range {} starts at {})",
                    pair[0].end,
                    index + 1,
                    pair[1].start
                )));
            }
        }
        let vertex_count = mesh.vertices.len() as u32;
        if mesh
            .draws
            .last()
            .is_none_or(|range| range.end != vertex_count)
        {
            return Err(RendererError::Vulkan(format!(
                "the scene mesh's last draw range ends at {} but the mesh has \
                 {} vertices (the ranges must cover the whole buffer)",
                mesh.draws.last().map_or(0, |range| range.end),
                vertex_count
            )));
        }
        // ---- the albedo set: every material's index inside it, every
        // texture at the generator's byte contract. ----
        for (index, material) in materials.iter().enumerate() {
            if material.texture as usize >= textures.textures.len() {
                return Err(RendererError::Vulkan(format!(
                    "scene material {index}'s albedo texture index {} falls \
                     outside the attached set ({} textures)",
                    material.texture,
                    textures.textures.len()
                )));
            }
        }
        let texture_bytes =
            (crate::proc_texture::TEXTURE_SIZE * crate::proc_texture::TEXTURE_SIZE * 4) as usize;
        for (index, texture) in textures.textures.iter().enumerate() {
            if texture.len() != texture_bytes {
                return Err(RendererError::Vulkan(format!(
                    "scene texture {index} is {} bytes; the procedural albedo \
                     contract is TEXTURE_SIZE²·4 ({texture_bytes})",
                    texture.len()
                )));
            }
        }
        // ---- the shadow bounds: a sane world AABB. NaN refuses —
        // `partial_cmp` returns `None` for NaN operands, so only a
        // true `min ≤ max` ordering passes. ----
        for axis in 0..3 {
            if !matches!(
                bounds.min[axis].partial_cmp(&bounds.max[axis]),
                Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
            ) {
                return Err(RendererError::Vulkan(format!(
                    "the scene bounds are degenerate on axis {axis} (min {:?} \
                     vs max {:?})",
                    bounds.min, bounds.max
                )));
            }
        }
        // ---- the wet ground roles: inside the mesh; the receiver
        // non-empty (an empty receiver would silently disable the wet
        // re-draw); reflected ranges may be EMPTY (a legal no-op draw)
        // but never inverted or outside the mesh. ----
        if ground.receiver.start >= ground.receiver.end {
            return Err(RendererError::Vulkan(format!(
                "the wet ground's receiver range [{}, {}) is empty",
                ground.receiver.start, ground.receiver.end
            )));
        }
        if ground.receiver.end > vertex_count {
            return Err(RendererError::Vulkan(format!(
                "the wet ground's receiver range [{}, {}) falls outside the \
                 mesh ({vertex_count} vertices)",
                ground.receiver.start, ground.receiver.end
            )));
        }
        for (index, range) in ground.reflected.iter().enumerate() {
            if range.start > range.end {
                return Err(RendererError::Vulkan(format!(
                    "the wet ground's reflected range {index} [{}, {}) is \
                     inverted",
                    range.start, range.end
                )));
            }
            if range.end > vertex_count {
                return Err(RendererError::Vulkan(format!(
                    "the wet ground's reflected range {index} [{}, {}) falls \
                     outside the mesh ({vertex_count} vertices)",
                    range.start, range.end
                )));
            }
        }
        Ok(())
    }
}

/// The proof an attached scene exists (P4 §3.2): returned by
/// [`crate::Renderer::attach_scene`] and carried by the renderer
/// for its lifetime — ONE scene per renderer (the device-lifetime
/// pass families — the retained skinned state, the shadow and
/// reflection per-material sets — bind the attached mesh/material
/// buffers; a second attachment would orphan them, so attach
/// refuses while one lives, and a fresh renderer is the supported
/// path for different content).
///
/// Plain `Copy` data, not an index: the handle carries the facts
/// callers need to author against the attachment — the material
/// table's count (the population material-index validation bound;
/// the corner's value is 3) — so a population can be validated
/// before it is ever submitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneHandle {
    /// The attached table's material count: every
    /// [`crate::renderer::ProxyPopulation`] material index must be
    /// strictly below it (the submission's typed refusal).
    pub material_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The corner description (the migration shim's own content —
    /// the one description the render path attaches today), as the
    /// validation fixtures' valid baseline: the shim-equality test
    /// in `corner_content` proves it equals the reference tables;
    /// here it is the KNOWN-GOOD input each refusal mutates.
    fn corner() -> SceneDescription {
        crate::corner_content::corner_scene_description()
    }

    /// The valid baseline passes: the corner description is
    /// coherent (the attach path accepts exactly this).
    #[test]
    fn the_corner_description_validates() {
        corner().validate().expect("the corner content is valid");
    }

    /// The pass-family count boundary: a table whose length is not
    /// the compiled family shape is refused AT ATTACH (fail-visible
    /// before any resource is built — the shadowed/skinned/reflection
    /// sets' fixed arrays would otherwise be the failure point, or
    /// worse, a silent truncation).
    #[test]
    fn a_table_outside_the_pass_family_shape_is_refused() {
        let mut scene = corner();
        // Four materials: one MORE than the families are built for.
        scene.materials.push(scene.materials[0]);
        scene.mesh.draws.push(scene.mesh.draws[2].clone());
        let error = scene.validate().expect_err("four materials must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("the pass families' descriptor sets are built for 3"),
            "the refusal names the compiled shape: {text}"
        );
        // Two materials: one FEWER refuses the same way (the check is
        // equality, not an upper bound).
        let mut scene = corner();
        scene.materials.pop();
        scene.mesh.draws.pop();
        assert!(scene.validate().is_err(), "two materials must refuse");
        // The empty table has its own refusal.
        let mut scene = corner();
        scene.materials.clear();
        scene.mesh.draws.clear();
        let error = scene.validate().expect_err("an empty table must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert_eq!(text, "the scene material table is empty");
    }

    /// The mesh's range contract: one range per material, starting
    /// at 0, contiguous, covering the buffer — the shadow depth
    /// pass draws `[0, last.end)` as the whole scene, so a gap or a
    /// short tail would silently drop geometry from the shadow map.
    #[test]
    fn mesh_range_contract_violations_are_refused() {
        // A range-count mismatch.
        let mut scene = corner();
        scene.mesh.draws.push(scene.mesh.draws[2].clone());
        let error = scene.validate().expect_err("a spare range must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("one range per material"),
            "the refusal names the per-material contract: {text}"
        );
        // A gap between ranges.
        let mut scene = corner();
        let end = scene.mesh.draws[0].end;
        scene.mesh.draws[1].start = end + 6;
        let error = scene.validate().expect_err("a gap must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("contiguous"),
            "the refusal names contiguity: {text}"
        );
        // A tail that does not reach the buffer's end.
        let mut scene = corner();
        scene.mesh.vertices.push(scene.mesh.vertices[0]);
        let error = scene.validate().expect_err("a short tail must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("cover the whole buffer"),
            "the refusal names coverage: {text}"
        );
        // A first range that does not start at 0.
        let mut scene = corner();
        scene.mesh.draws[0].start = 3;
        assert!(scene.validate().is_err(), "a nonzero start must refuse");
        // An empty mesh.
        let mut scene = corner();
        scene.mesh.vertices.clear();
        let error = scene.validate().expect_err("an empty mesh must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert_eq!(text, "the scene mesh is empty");
    }

    /// The albedo set's contract: a material's texture index must
    /// select a supplied texture, and every texture must carry the
    /// generator's byte length (the upload reads TEXTURE_SIZE²
    /// RGBA8 with no runtime shape check).
    #[test]
    fn albedo_set_contract_violations_are_refused() {
        let mut scene = corner();
        scene.materials[1].texture = 9;
        let error = scene
            .validate()
            .expect_err("an out-of-set index must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("falls outside the attached set"),
            "the refusal names the set boundary: {text}"
        );
        let mut scene = corner();
        scene.textures.textures[2] = Vec::new();
        let error = scene
            .validate()
            .expect_err("a malformed texture must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("the procedural albedo contract"),
            "the refusal names the byte contract: {text}"
        );
    }

    /// The bounds and wet-role contracts: a degenerate (or NaN)
    /// AABB refuses; a receiver or reflected range outside the mesh
    /// refuses; an empty receiver refuses (it would silently
    /// disable the wet re-draw).
    #[test]
    fn bounds_and_ground_role_violations_are_refused() {
        let mut scene = corner();
        scene.bounds.min[1] = scene.bounds.max[1] + 1.0;
        let error = scene.validate().expect_err("an inverted AABB must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("degenerate on axis 1"),
            "the refusal names the axis: {text}"
        );
        let mut scene = corner();
        scene.bounds.min[0] = f32::NAN;
        assert!(scene.validate().is_err(), "a NaN bound must refuse");
        let mut scene = corner();
        scene.ground.receiver.end = u32::MAX;
        let error = scene
            .validate()
            .expect_err("an out-of-mesh receiver must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("receiver range") && text.contains("falls outside the mesh"),
            "the refusal names the receiver and the mesh: {text}"
        );
        let mut scene = corner();
        scene.ground.receiver.start = scene.ground.receiver.end;
        let error = scene.validate().expect_err("an empty receiver must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("receiver range") && text.contains("is empty"),
            "the refusal names the empty receiver: {text}"
        );
        // An EMPTY reflected range is legal (a no-op draw — the mirror
        // reflects fewer masses); only out-of-mesh ranges refuse.
        let mut scene = corner();
        scene.ground.reflected[1] = 0..0;
        scene
            .validate()
            .expect("an empty reflected range is a legal no-op");
        let mut scene = corner();
        scene.ground.reflected[0].end = u32::MAX;
        assert!(
            scene.validate().is_err(),
            "an out-of-mesh reflected range must refuse"
        );
    }
}
