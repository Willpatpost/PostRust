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
//! stopped being a renderer constant and became caller data. S6
//! completed the migration: the `Corner` construction preference
//! and its renderer-side migration shim (`corner_content.rs`) are
//! DELETED, and every description consumers attach is built on
//! their side of the boundary (engine-slice's `corner_scene`
//! holds the corner's; the garage scene holds the second
//! consumer's).
//!
//! VALIDATION (the capture-refusal discipline, attach-time form):
//! [`SceneDescription::validate`] checks the description WHOLE,
//! device-free, BEFORE any resource is built — a refusal builds
//! nothing and leaves the renderer untouched. The checks are the
//! structural contracts the pass machinery relies on: one draw
//! range per material in material order, the ranges contiguous,
//! covering the vertex buffer (the shadow depth pass draws
//! `[0, last.end)`), and NON-INVERTED — start ≤ end per range, so
//! the lit pass's `end - start` vertex count is overflow-free by
//! construction on every admitted table (an empty range is the
//! legal draws-nothing case); every material's albedo index
//! inside the attached set; every texture at the procedural
//! generator's byte contract; FINITE bounds and mirror-plane
//! height (the shadow cascade's projection math and the wet
//! reflection's mirrored camera consume them — nonfinite input
//! degenerates both silently, so the admission domain is
//! finite-only); the bounds ordered per axis (NaN refuses); and
//! the wet roles inside the mesh.
//! ONE boundary is a compile-time shape, not author freedom: the
//! pass families' descriptor sets (the shadowed, skinned, and
//! reflection per-material sets) are built for
//! [`PASS_FAMILY_MATERIAL_COUNT`] materials — the corner-era
//! table — so attach refuses any other count. THE S6 DISPOSITION
//! of that boundary: the second scene (engine-slice's garage)
//! authors a THREE-material table and fits the compiled shape
//! as-is, so generalizing the fixed arrays is NOT paid for by
//! this stage — the boundary is documented here as the named
//! follow-up (relaxing it means rebuilding the shadowed/skinned/
//! reflection set families against dynamic counts; the design's
//! §7 second scene deliberately stayed inside the compiled
//! shape, and the boundary refuses fail-visibly rather than
//! truncating silently).
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
/// length at attach (fail-visible, never a silent truncation).
/// THE S6 BOUNDARY, made explicit: both live scenes (the corner
/// and the second consumer's garage) author THREE-material tables
/// and fit the compiled shape, so the generalization the S2
/// design sketched for this step is NOT exercised — relaxing the
/// count means rebuilding the per-material descriptor-set
/// families against dynamic sizes, deferred until a real scene
/// needs it (the refusal is the named, fail-visible boundary).
/// Public since S6 (the description authors' named boundary): the
/// compiled family shape is part of the attach contract's
/// documented surface, asserted at build sites on the consumer's
/// side of the boundary.
pub const PASS_FAMILY_MATERIAL_COUNT: usize = 3;

/// One static lit mesh: world-space triangle-list vertices plus
/// ONE draw range per material, in MATERIAL ORDER — range `i`
/// draws material `i`'s triangles through material `i`'s
/// descriptor set (the lit pipelines' bind-per-range shape). The
/// ranges are contiguous and cover the whole vertex buffer
/// (validated: the shadow depth pass draws `[0, last.end)` as the
/// whole scene; an author cannot leave a gap the depth pass
/// would miss), and every range is NON-INVERTED — start ≤ end
/// (validated: the lit pass computes `end - start` as the draw's
/// vertex count, and the admission's chained contracts make that
/// subtraction overflow-free on every admitted table). An empty
/// range is legal (that material draws nothing — a vertexCount-0
/// `cmd_draw` is a Vulkan no-op); the builder over an authored
/// box table ([`crate::lit_mesh::mesh_from_boxes`]) emits the
/// ground quad into material 0's range first — the wet receiver
/// role's position in the corner authoring, and an empty
/// `[n, n)` for any material the box table leaves unpopulated.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneMesh {
    /// The triangle-list vertices (three per primitive, the lit
    /// pipelines' 32-byte input — see [`LitVertex`]).
    pub vertices: Vec<LitVertex>,
    /// One draw range per material, in material order: starting at
    /// 0, contiguous, ending at `vertices.len()`, every range
    /// non-inverted (start ≤ end — an empty range draws nothing).
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
    /// FINITE (validated, the R02 domain): the mirrored camera is
    /// `2 · plane_y − eye` arithmetic, so a NaN or ±inf plane
    /// breaks the reflection silently rather than refusing.
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
    /// FINITE on every endpoint and ordered min ≤ max per axis
    /// (validated, the R02 domain — `shadows::cascade_for` does
    /// span/center arithmetic over these six floats, and
    /// nonfinite endpoints silently degenerate the light matrix).
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
        // ---- the mesh's draw ranges: one per material,
        // non-inverted, contiguous, covering the buffer (the depth
        // pass draws [0, last.end); the lit pass's per-range
        // `end - start` is overflow-free on every admitted
        // table). ----
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
        // ---- the re-audit's R01 hole, closed: every range
        // NON-INVERTED. Before this check the four structural
        // contracts (start at 0, contiguous, covering) all held for
        // an inverted table like [[0,5],[5,3],[3,N]] — contiguity
        // compares end to the NEXT start, so it holds on both
        // sides of an inverted range — and the lit pass's draw
        // loop (corner.rs) then computed `end - start` as the
        // draw's vertex count, a u32 UNDERFLOW (a debug panic; a
        // four-billion-vertex release draw).
        //
        // THE POLICY — start ≤ end, NOT start < end: an empty
        // range [n, n) is a LEGAL draws-nothing table entry.
        // `lit_mesh::mesh_from_boxes` emits exactly that for a
        // material with no boxes in the authored table, and the
        // wet roles already document the same legality for an
        // empty `reflected` range. Downstream, an empty range is
        // consumed SAFELY with no guard needed: `cmd_draw` with
        // vertexCount 0 is a Vulkan no-op (corner.rs's lit loop),
        // and the depth pass's whole-mesh draw reads only the
        // LAST range's end (unchanged by interior empties).
        //
        // THE OVERFLOW-FREE PROOF this check completes: with
        // start ≤ end per range, the four contracts chain into
        //   0 = draws[0].start ≤ draws[0].end = draws[1].start ≤
        //   … ≤ last.end = vertices.len(),
        // so EVERY endpoint of EVERY range sits in
        // [0, vertex_count] — each `end - start` is in
        // [0, vertex_count] and each `start` (the draw's
        // firstVertex) is inside the buffer. The subtraction the
        // lit and reflection recorders perform is overflow-free
        // on EVERY admitted description, by construction. ----
        for (index, range) in mesh.draws.iter().enumerate() {
            if range.start > range.end {
                return Err(RendererError::Vulkan(format!(
                    "the scene mesh's draw range {index} [{}, {}) is inverted \
                     (every range must satisfy start ≤ end — the lit pass \
                     computes end - start as the draw's vertex count, which \
                     would underflow)",
                    range.start, range.end
                )));
            }
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
        // ---- the shadow bounds: a sane, FINITE world AABB. The
        // ordering check keeps its NaN refusal (`partial_cmp`
        // returns `None` for NaN operands, so only a true
        // `min ≤ max` passes). The finite check closes the
        // re-audit's R02 hole: `partial_cmp` ORDERS infinities
        // ([-inf, 0] and [-inf, +inf] compare `Less`, [inf, inf]
        // compares `Equal`), so nonfinite endpoints passed the
        // ordering — but the bounds' consumers do ARITHMETIC, not
        // comparisons: `shadows::cascade_for`'s 8-corner walk
        // turns ±inf endpoints into infinite spans (the scale
        // `2/span` collapses to 0) and NaN centers
        // ((-inf + inf) · 0.5), silently degenerating the light
        // matrix the depth pass and the shadowed compare both
        // read. FINITE-ONLY is the coherent numerical domain for
        // authored scene floats the pass math consumes — the same
        // discipline the wet mirror's plane height gets below. ----
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
            if !bounds.min[axis].is_finite() || !bounds.max[axis].is_finite() {
                return Err(RendererError::Vulkan(format!(
                    "the scene bounds are not finite on axis {axis} (min {} vs \
                     max {} — the shadow cascade's projection math consumes \
                     the AABB and degenerates on nonfinite input)",
                    bounds.min[axis], bounds.max[axis]
                )));
            }
        }
        // ---- the wet ground roles: inside the mesh; the receiver
        // non-empty (an empty receiver would silently disable the wet
        // re-draw); reflected ranges may be EMPTY (a legal no-op draw)
        // but never inverted or outside the mesh. The mirror plane
        // FINITE (the re-audit's R02 class on the reflection's own
        // authored float): `planar.rs` mirrors the wet camera across
        // it (`2 · plane_y − eye`, the mirrored view-projection), and
        // a NaN or ±inf plane propagates straight into the mirrored
        // camera — a silently broken reflection, not a refusal. ----
        if !ground.receiver_plane_y.is_finite() {
            return Err(RendererError::Vulkan(format!(
                "the wet ground's mirror-plane height {} is not finite (the \
                 reflection camera mirrors across it)",
                ground.receiver_plane_y
            )));
        }
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

    /// The validation fixtures' valid baseline: a synthetic
    /// three-material description in the corner's own authoring
    /// SHAPE (a ground quad + one mass per remaining material),
    /// built through the same public builder every consumer's
    /// description uses (`lit_mesh::mesh_from_boxes`). The
    /// corner's CONTENT moved to engine-slice at S6
    /// (`corner_scene`, carrying the drift gate that pins it to
    /// the `lit_scene` reference tables); this renderer-local
    /// fixture keeps the ADMISSION contract's tests device-free
    /// and content-independent — what each refusal below mutates
    /// is the table's STRUCTURE, never which scene built it.
    fn valid_scene() -> SceneDescription {
        let boxes = [
            crate::lit_scene::SceneBox {
                min: [6.0, 0.0, 6.0],
                max: [24.0, 18.0, 24.0],
                material: 1,
            },
            crate::lit_scene::SceneBox {
                min: [-24.0, 0.0, -24.0],
                max: [-6.0, 9.0, -6.0],
                material: 2,
            },
        ];
        let (vertices, draws) =
            crate::lit_mesh::mesh_from_boxes(&boxes, 24.0, 0.0, PASS_FAMILY_MATERIAL_COUNT);
        let (min, max) = crate::shadows::scene_bounds(&boxes);
        SceneDescription {
            mesh: SceneMesh {
                vertices,
                draws: draws.clone(),
            },
            materials: crate::lit_scene::materials().to_vec(),
            textures: crate::proc_texture::ProcTextureSet {
                textures: (0..crate::proc_texture::TEXTURE_COUNT)
                    .map(crate::proc_texture::procedural_texture)
                    .collect(),
            },
            bounds: SceneBounds { min, max },
            ground: GroundRoles {
                receiver: draws[0].clone(),
                receiver_plane_y: 0.0,
                reflected: [draws[1].clone(), draws[2].clone()],
            },
        }
    }

    /// The valid baseline passes: the synthetic description is
    /// coherent (the attach path accepts exactly this shape).
    #[test]
    fn the_valid_baseline_description_validates() {
        valid_scene()
            .validate()
            .expect("the baseline content is valid");
    }

    /// The pass-family count boundary: a table whose length is not
    /// the compiled family shape is refused AT ATTACH (fail-visible
    /// before any resource is built — the shadowed/skinned/reflection
    /// sets' fixed arrays would otherwise be the failure point, or
    /// worse, a silent truncation).
    #[test]
    fn a_table_outside_the_pass_family_shape_is_refused() {
        let mut scene = valid_scene();
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
        let mut scene = valid_scene();
        scene.materials.pop();
        scene.mesh.draws.pop();
        assert!(scene.validate().is_err(), "two materials must refuse");
        // The empty table has its own refusal.
        let mut scene = valid_scene();
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
        let mut scene = valid_scene();
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
        let mut scene = valid_scene();
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
        let mut scene = valid_scene();
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
        let mut scene = valid_scene();
        scene.mesh.draws[0].start = 3;
        assert!(scene.validate().is_err(), "a nonzero start must refuse");
        // An empty mesh.
        let mut scene = valid_scene();
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
        let mut scene = valid_scene();
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
        let mut scene = valid_scene();
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
        let mut scene = valid_scene();
        scene.bounds.min[1] = scene.bounds.max[1] + 1.0;
        let error = scene.validate().expect_err("an inverted AABB must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("degenerate on axis 1"),
            "the refusal names the axis: {text}"
        );
        let mut scene = valid_scene();
        scene.bounds.min[0] = f32::NAN;
        assert!(scene.validate().is_err(), "a NaN bound must refuse");
        let mut scene = valid_scene();
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
        let mut scene = valid_scene();
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
        let mut scene = valid_scene();
        scene.ground.reflected[1] = 0..0;
        scene
            .validate()
            .expect("an empty reflected range is a legal no-op");
        let mut scene = valid_scene();
        scene.ground.reflected[0].end = u32::MAX;
        assert!(
            scene.validate().is_err(),
            "an out-of-mesh reflected range must refuse"
        );
    }

    /// THE R01 WITNESS (the re-audit's confirmed pass-today shape,
    /// verified RED at the lane's base `600bff2`: the table
    /// [[0,5],[5,3],[3,N]] over the fixture's own vertex count
    /// passed validate — contiguity compares end to the NEXT
    /// start, so it holds on both sides of an inverted range, and
    /// the coverage check only reads the LAST end). The lit pass's
    /// draw loop would then compute `3 - 5` as a u32 vertex count:
    /// a debug panic, a four-billion-vertex release draw. The
    /// non-inversion admission refuses it.
    // The witness IS the inverted range the clippy lint exists to
    // catch in production code — here it is the INPUT the refusal
    // is proven on, so the lint is allowed for the literal alone.
    #[allow(clippy::reversed_empty_ranges)]
    #[test]
    fn inverted_draw_ranges_are_refused() {
        let mut scene = valid_scene();
        let vertex_count = scene.mesh.vertices.len() as u32;
        scene.mesh.draws[0] = 0..5;
        scene.mesh.draws[1] = 5..3;
        scene.mesh.draws[2] = 3..vertex_count;
        let error = scene.validate().expect_err("an inverted range must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("draw range 1") && text.contains("inverted"),
            "the refusal names the inverted range: {text}"
        );
        // The same inversion in the LAST range: the tail's start
        // exceeds its end while every legacy check still holds —
        // contiguity at the shared boundary (draws[1]'s end ==
        // the tail's fabricated start), coverage at the end value
        // itself (the vertex count).
        let mut scene = valid_scene();
        let vertex_count = scene.mesh.vertices.len() as u32;
        scene.mesh.draws[1] = scene.mesh.draws[0].end..vertex_count + 1;
        scene.mesh.draws[2] = vertex_count + 1..vertex_count;
        let error = scene.validate().expect_err("the inverted tail must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("draw range 2") && text.contains("inverted"),
            "the refusal names the inverted tail: {text}"
        );
    }

    /// THE EMPTY-RANGE POLICY PIN (the R01 policy decision,
    /// investigated and documented): admission requires
    /// start ≤ end, NOT start < end — an empty material draw is
    /// legal. The builder itself emits one
    /// (`mesh_from_boxes` over a table that leaves a material
    /// unpopulated emits `[n, n)` for it), and the downstream
    /// consumption is safe with NO guard: a vertexCount-0
    /// `cmd_draw` is a Vulkan no-op, and the depth pass's
    /// whole-mesh draw reads only the last range's end. This
    /// description — the baseline's masses minus material 1's — is
    /// the builder-faithful shape, empty middle range and empty
    /// reflected role both, and it admits.
    #[test]
    fn an_empty_material_draw_is_legal() {
        // A table with material-2 boxes ONLY: material 1's range
        // comes out empty from the real builder.
        let boxes = [crate::lit_scene::SceneBox {
            min: [6.0, 0.0, -40.0],
            max: [40.0, 9.0, -6.0],
            material: 2,
        }];
        let (vertices, draws) =
            crate::lit_mesh::mesh_from_boxes(&boxes, 40.0, 0.0, PASS_FAMILY_MATERIAL_COUNT);
        assert_eq!(
            draws[1],
            draws[0].end..draws[0].end,
            "the builder emits the empty range for the unpopulated material"
        );
        let mut scene = valid_scene();
        scene.mesh = SceneMesh {
            vertices,
            draws: draws.clone(),
        };
        scene.ground = GroundRoles {
            receiver: draws[0].clone(),
            receiver_plane_y: 0.0,
            // The empty reflected role: the mirror reflects the
            // material-1 mass that no longer exists — the
            // documented legal no-op.
            reflected: [draws[1].clone(), draws[2].clone()],
        };
        scene
            .validate()
            .expect("an empty material draw is a legal no-op draw");
    }

    /// THE R02 WITNESSES (the re-audit's confirmed pass-today
    /// shapes, verified RED at the lane's base `600bff2`:
    /// `partial_cmp` ORDERS infinities, so all of these passed
    /// min ≤ max — [-inf, 0] and [-inf, +inf] compare `Less`,
    /// [inf, inf] compares `Equal`). The bounds' consumer does
    /// arithmetic, not comparisons — `cascade_for`'s 8-corner
    /// walk over ±inf endpoints yields infinite spans (scale
    /// `2/span` = 0) and NaN centers — so the admission domain is
    /// finite-only.
    #[test]
    fn nonfinite_scene_bounds_are_refused() {
        // [-inf, 0] on axis 0.
        let mut scene = valid_scene();
        scene.bounds.min[0] = f32::NEG_INFINITY;
        scene.bounds.max[0] = 0.0;
        let error = scene.validate().expect_err("[-inf, 0] bounds must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("not finite on axis 0"),
            "the refusal names the axis and the domain: {text}"
        );
        // [0, +inf] on axis 0.
        let mut scene = valid_scene();
        scene.bounds.min[0] = 0.0;
        scene.bounds.max[0] = f32::INFINITY;
        let error = scene.validate().expect_err("[0, +inf] bounds must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("not finite on axis 0"),
            "the refusal names the axis and the domain: {text}"
        );
        // [-inf, +inf] on axis 0.
        let mut scene = valid_scene();
        scene.bounds.min[0] = f32::NEG_INFINITY;
        scene.bounds.max[0] = f32::INFINITY;
        let error = scene
            .validate()
            .expect_err("[-inf, +inf] bounds must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("not finite on axis 0"),
            "the refusal names the axis and the domain: {text}"
        );
        // [inf, inf] — the Equal shape that ALSO ordered before
        // the finite check (partial_cmp returns Some(Equal)).
        let mut scene = valid_scene();
        scene.bounds.min[1] = f32::INFINITY;
        scene.bounds.max[1] = f32::INFINITY;
        let error = scene.validate().expect_err("[inf, inf] bounds must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("not finite on axis 1"),
            "the refusal names the axis and the domain: {text}"
        );
    }

    /// The R02 finite domain on the reflection's own authored
    /// float (verified RED at the lane's base: a NaN mirror plane
    /// passed validate): the wet camera mirrors across the plane
    /// (`2 · plane_y − eye` in `planar.rs`), so a nonfinite plane
    /// propagates into the mirrored view-projection instead of
    /// refusing.
    #[test]
    fn a_nonfinite_mirror_plane_is_refused() {
        for plane in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut scene = valid_scene();
            scene.ground.receiver_plane_y = plane;
            let error = scene
                .validate()
                .expect_err("a nonfinite mirror plane must refuse");
            let RendererError::Vulkan(text) = &error else {
                panic!("the refusal is a Vulkan error: {error:?}");
            };
            assert!(
                text.contains("mirror-plane height"),
                "the refusal names the plane: {text}"
            );
        }
    }
}
