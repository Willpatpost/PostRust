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
//! `[0, last.end)`), NON-INVERTED — start ≤ end per range, so
//! the lit pass's `end - start` vertex count is overflow-free by
//! construction on every admitted table (an empty range is the
//! legal draws-nothing case); and TRIANGLE-ALIGNED boundaries
//! (CA-15/F15) — every draw and wet-role boundary a multiple of
//! [`VERTICES_PER_TRIANGLE`], so the depth pass's whole-mesh draw
//! and the lit/reflection passes' per-range draws assemble the
//! SAME primitive set (the mesh draws' typed admission is
//! [`SceneMesh::admit_draws`], the retained
//! [`DrawTable`]); every material's albedo index inside the
//! attached set; every texture at the TYPED extent's byte
//! contract ([`TextureExtent`] — CA-14's bounded first step);
//! FINITE bounds and mirror-plane height (the shadow cascade's
//! projection math and the wet reflection's mirrored camera
//! consume them — nonfinite input degenerates both silently, so
//! the admission domain is finite-only); the bounds ordered per
//! axis (NaN refuses); and the wet roles inside the mesh.
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

/// CA-15 (F15): the number of vertices one primitive of the scene
/// mesh's topology consumes. Every pipeline that draws the attached
/// mesh — the lit pass's per-material ranges
/// (`corner.rs`'s `draw`), the shadow depth pass's whole-mesh draw
/// (`shadow_draw.rs`'s `record_depth_pass`), and the reflection's
/// receiver/redraw and reflected ranges (`reflection_draw.rs`) —
/// assembles it as a NON-INDEXED TRIANGLE LIST (the pipelines'
/// `vk::PrimitiveTopology::TRIANGLE_LIST` input-assembly state), so
/// a primitive boundary sits at every vertex index that is a
/// multiple of this constant. A draw/role range boundary that is
/// NOT such a multiple splits a primitive: the whole-buffer draw
/// and the per-range draws then assemble DIFFERENT primitive sets
/// from the same vertices (the audit's disagreement — the depth
/// pass's shadow map covers triangles the lit pass never draws, and
/// vice versa), and a range whose LENGTH is not a multiple assembles
/// an incomplete trailing primitive. The admission
/// ([`SceneMesh::admit_draws`], the [`GroundRoles`] checks in
/// [`SceneDescription::validate`]) rejects both shapes at attach.
pub const VERTICES_PER_TRIANGLE: u32 = 3;

/// CA-15 (F15): whether a vertex index sits ON a primitive boundary
/// of the triangle-list topology (a multiple of
/// [`VERTICES_PER_TRIANGLE`]) — the alignment every admitted draw
/// and wet-role range boundary must carry. Pure.
pub fn triangle_list_boundary(vertex_index: u32) -> bool {
    vertex_index.is_multiple_of(VERTICES_PER_TRIANGLE)
}

/// CA-14 (Q06 — the bounded first step of the scene-contract
/// generalization; see `scene_contract_design` for the full plan):
/// the TYPED texture extent. Before this type the admission checked
/// the albedo set's bytes against a magic arithmetic
/// (`TEXTURE_SIZE * TEXTURE_SIZE * 4`) with the extent implicit in
/// the number, and the upload re-stated it as a bare `Extent2D`
/// built from the generator's constant — the first pass family's
/// authored shape was knowledge duplicated at every consumer. The
/// typed extent names the shape ONCE: the admission matches bytes
/// against it ([`Self::matches`]), and the upload consumes the same
/// value ([`SceneDescription::albedo_extent`]) — the generator's
/// constants and the consumers' expectations are single-sourced
/// through the type.
///
/// The first family's single square RGBA8 extent stays the compiled
/// shape today (the same documented boundary as
/// [`PASS_FAMILY_MATERIAL_COUNT`]); per-material extents and
/// non-RGBA8 formats are the design record's migration plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureExtent {
    /// The extent's width in texels.
    pub width: u32,
    /// The extent's height in texels.
    pub height: u32,
    /// The bytes one texel carries (RGBA8 = 4 — the paired upload
    /// format is `vk::Format::R8G8B8A8_UNORM`).
    pub bytes_per_texel: u32,
}

impl TextureExtent {
    /// The procedural albedo contract's extent — the first pass
    /// family's authored shape (256×256 RGBA8), single-sourced from
    /// the generator's own constants: the drift the byte-check used
    /// to leave implicit is now a type-level pin (the generator, the
    /// admission, and the upload read the SAME shape).
    pub const PROC_ALBEDO: Self = Self {
        width: crate::proc_texture::TEXTURE_SIZE,
        height: crate::proc_texture::TEXTURE_SIZE,
        bytes_per_texel: 4,
    };

    /// The extent's byte length — the admission's exact-match bound.
    /// Saturating: an absurd authored extent (whose product would
    /// overflow `u64`) never matches a real byte slice, so it
    /// refuses rather than wrapping.
    pub fn byte_len(self) -> u64 {
        (self.width as u64)
            .saturating_mul(self.height as u64)
            .saturating_mul(self.bytes_per_texel as u64)
    }

    /// Whether a byte slice carries exactly this extent's shape
    /// (`width × height × bytes_per_texel` bytes, row-major).
    pub fn matches(self, bytes: &[u8]) -> bool {
        bytes.len() as u64 == self.byte_len()
    }
}

/// CA-14 + CA-15: the ADMITTED material/draw table — the typed form
/// [`SceneMesh`]'s draw ranges take at admission. Built ONLY by
/// [`SceneMesh::admit_draws`], which performs the whole structural
/// contract (one range per material in material order, first start
/// at 0, every range non-inverted, contiguous, covering the whole
/// vertex buffer, and — CA-15's addition — every boundary
/// TRIANGLE-ALIGNED), so every consumer reads a table whose ranges
/// assemble the SAME primitive set however they are grouped:
///
/// - the lit pass draws each material's range separately
///   (`firstVertex = range.start`, `vertexCount = end − start`);
/// - the shadow depth pass draws the WHOLE buffer as one range
///   (`[0, whole_mesh_vertex_count)`);
/// - the reflection re-draws the receiver and reflected ranges.
///
/// With every boundary a multiple of
/// [`VERTICES_PER_TRIANGLE`], all three groupings partition the same
/// authored triangles — the pre-fix admission checked ordering,
/// coverage and bounds but NOT alignment, so a boundary inside a
/// triangle (a table contiguous, covering, non-inverted, and
/// misaligned) made the depth pass's absolute-triangle assembly
/// disagree with the lit pass's per-range assembly. The typed
/// retention (the upload path keeps the admitted table, not the raw
/// ranges) is the first step of the CA-14 generalization: the
/// pass families' consumption goes through named admission facts
/// ([`Self::ranges`], [`Self::whole_mesh_vertex_count`]) instead of
/// re-deriving them from the raw table at each draw site.
///
/// `Default` is the PARTIAL-CONSTRUCTION placeholder only (an
/// empty table over zero vertices — `CornerResources`'s build
/// initializes the real table through [`SceneMesh::admit_draws`]
/// before any draw can run); it is never drawn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DrawTable {
    /// The per-material draw ranges, in material order (validated:
    /// see [`SceneMesh::admit_draws`]).
    ranges: Vec<Range<u32>>,
    /// The whole-buffer vertex count (the last range's end — the
    /// depth pass's whole-mesh draw count, a validated
    /// triangle-aligned tail).
    vertex_count: u32,
}

impl DrawTable {
    /// The per-material draw ranges in material order — the lit
    /// pass's bind-per-range consumption.
    pub fn ranges(&self) -> &[Range<u32>] {
        &self.ranges
    }

    /// The whole-buffer vertex count — the shadow depth pass's
    /// whole-mesh draw (`[0, vertex_count)`), the typed admission
    /// fact the pass reads instead of re-deriving it from the raw
    /// table.
    pub fn whole_mesh_vertex_count(&self) -> u32 {
        self.vertex_count
    }
}

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
    /// non-inverted (start ≤ end — an empty range draws nothing) and
    /// every boundary triangle-aligned (a multiple of
    /// [`VERTICES_PER_TRIANGLE`]). Validated whole by
    /// [`Self::admit_draws`] — the typed constructor
    /// [`SceneDescription::validate`] and the upload path both
    /// consume.
    pub draws: Vec<Range<u32>>,
}

impl SceneMesh {
    /// CA-14 + CA-15: admits the draw table WHOLE — the typed
    /// constructor that performs every structural contract the pass
    /// machinery relies on, returning the retained
    /// [`DrawTable`]. A refusal builds nothing (the
    /// capture-refusal discipline, attach-time form). The checks,
    /// in order:
    ///
    /// 1. **One range per material** (`draws.len() ==
    ///    material_count`) — the lit pass binds one descriptor set
    ///    per range.
    /// 2. **First start at 0** — the shadow depth pass draws
    ///    `[0, last.end)` as the whole scene; an author cannot
    ///    leave a gap the depth pass would miss.
    /// 3. **Every range NON-INVERTED** (start ≤ end) — the R01
    ///    hole: the lit pass computes `end - start` as the draw's
    ///    vertex count, a u32 underflow on an inverted table. An
    ///    empty `[n, n)` is legal (a vertexCount-0 `cmd_draw` is a
    ///    Vulkan no-op).
    /// 4. **Contiguous** — range `i`'s end is range `i+1`'s start.
    /// 5. **Covering** — the last range's end is the vertex count.
    /// 6. **Every boundary TRIANGLE-ALIGNED** (CA-15/F15) — every
    ///    range's start AND the vertex count are multiples of
    ///    [`VERTICES_PER_TRIANGLE`]. Together with contiguity and
    ///    coverage this makes EVERY boundary (interior ends
    ///    included, and every range's length) aligned, so the depth
    ///    pass's whole-buffer draw and the lit pass's per-range
    ///    draws assemble the SAME primitive set; before this check
    ///    a boundary inside a triangle passed every legacy check
    ///    and split a primitive between materials.
    pub fn admit_draws(&self, material_count: usize) -> Result<DrawTable, RendererError> {
        if self.draws.len() != material_count {
            return Err(RendererError::Vulkan(format!(
                "the scene mesh has {} draw ranges but the material table has \
                 {} entries (one range per material, in material order)",
                self.draws.len(),
                material_count
            )));
        }
        if self.draws[0].start != 0 {
            return Err(RendererError::Vulkan(format!(
                "the scene mesh's first draw range starts at {} (the ranges \
                 must start at vertex 0 — the shadow depth pass draws the \
                 whole buffer)",
                self.draws[0].start
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
        for (index, range) in self.draws.iter().enumerate() {
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
        for (index, pair) in self.draws.windows(2).enumerate() {
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
        let vertex_count = self.vertices.len() as u32;
        if self
            .draws
            .last()
            .is_none_or(|range| range.end != vertex_count)
        {
            return Err(RendererError::Vulkan(format!(
                "the scene mesh's last draw range ends at {} but the mesh has \
                 {} vertices (the ranges must cover the whole buffer)",
                self.draws.last().map_or(0, |range| range.end),
                vertex_count
            )));
        }
        // ---- CA-15 (F15): the triangle-list boundary contract. The
        // checks above prove every boundary sits in
        // [0, vertex_count]; THIS check proves each sits ON a
        // primitive boundary. The failure shape it refuses: a table
        // contiguous, covering, and non-inverted whose boundary
        // falls INSIDE a triangle — the depth pass's whole-buffer
        // draw assembles primitives at absolute indices
        // 0, 3, 6, … while the lit pass's material-`i` draw
        // assembles at `range.start, range.start + 3, …`, so a
        // misaligned boundary makes the two passes assemble
        // DIFFERENT primitive sets from the same vertices (the
        // shadow map covers triangles the lit pass never draws,
        // and vice versa), and a range whose length is not a
        // multiple of 3 assembles an incomplete trailing
        // primitive. Checking every range's start (which, through
        // contiguity, is every interior end) plus the vertex count
        // (the tail) makes every boundary and every range length
        // aligned. ----
        for (index, range) in self.draws.iter().enumerate() {
            if !triangle_list_boundary(range.start) {
                return Err(RendererError::Vulkan(format!(
                    "the scene mesh's draw range {index} starts at vertex {} — \
                     inside a triangle (triangle-list boundaries are multiples \
                     of {}; the depth pass's whole-mesh draw and the lit pass's \
                     per-range draws would assemble different primitive sets)",
                    range.start, VERTICES_PER_TRIANGLE
                )));
            }
        }
        if !triangle_list_boundary(vertex_count) {
            return Err(RendererError::Vulkan(format!(
                "the scene mesh has {vertex_count} vertices — not a multiple of \
                 {} (the tail splits a triangle: the depth pass's whole-mesh \
                 draw would assemble a primitive set the per-range draws \
                 cannot)",
                VERTICES_PER_TRIANGLE
            )));
        }
        Ok(DrawTable {
            ranges: self.draws.clone(),
            vertex_count,
        })
    }
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
        // ---- the mesh's draw table: the TYPED admission (CA-14's
        // bounded first step — the structural checks moved INTO
        // [`SceneMesh::admit_draws`]'s typed constructor, with
        // CA-15's triangle-alignment contracts added); the admitted
        // table is what the upload path retains. ----
        if mesh.vertices.is_empty() {
            return Err(RendererError::Vulkan("the scene mesh is empty".into()));
        }
        let draws = mesh.admit_draws(materials.len())?;
        let vertex_count = draws.whole_mesh_vertex_count();
        // ---- the albedo set: every material's index inside it, every
        // texture at the TYPED extent's byte contract (CA-14's
        // bounded first step — the extent is named and typed, the
        // same value the upload consumes through
        // [`Self::albedo_extent`]). ----
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
        let extent = self.albedo_extent();
        for (index, texture) in textures.textures.iter().enumerate() {
            if !extent.matches(texture) {
                return Err(RendererError::Vulkan(format!(
                    "scene texture {index} is {} bytes; the procedural albedo \
                     contract is the typed extent {}×{}×{} B/texel ({} bytes)",
                    texture.len(),
                    extent.width,
                    extent.height,
                    extent.bytes_per_texel,
                    extent.byte_len()
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
        // but never inverted or outside the mesh; and — CA-15 (F15)
        // — every role boundary TRIANGLE-ALIGNED: the wet re-draw
        // re-rasterizes the receiver with `firstVertex = start` and
        // the mirror pass draws each reflected range the same way,
        // so a role boundary inside a triangle makes the re-draw
        // assemble primitives the lit pass never drew (a receiver
        // [1, 7) over a ground quad authored at [0, 6) rasterizes
        // the slivers (1,2,3)(4,5,6) — geometry no pass authored),
        // and a role LENGTH that is not a multiple of 3 assembles an
        // incomplete trailing primitive. The policy is uniform,
        // empty ranges included: a role range is a position IN the
        // mesh's primitive structure, and an author pointing inside
        // a primitive is an authoring error refused fail-visibly
        // (the builder emits only aligned boundaries — every quad is
        // six vertices). The mirror plane FINITE (the re-audit's
        // R02 class on the reflection's own authored float):
        // `planar.rs` mirrors the wet camera across it
        // (`2 · plane_y − eye`, the mirrored view-projection), and a
        // NaN or ±inf plane propagates straight into the mirrored
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
        // CA-15: the receiver's two boundaries (start for the
        // re-draw's firstVertex, length for its vertexCount).
        if !triangle_list_boundary(ground.receiver.start) {
            return Err(RendererError::Vulkan(format!(
                "the wet ground's receiver range starts at vertex {} — inside \
                 a triangle (triangle-list boundaries are multiples of {}; the \
                 wet re-draw would rasterize primitives the lit pass never \
                 drew)",
                ground.receiver.start, VERTICES_PER_TRIANGLE
            )));
        }
        if !triangle_list_boundary(ground.receiver.end - ground.receiver.start) {
            return Err(RendererError::Vulkan(format!(
                "the wet ground's receiver range [{}, {}) is {} vertices long — \
                 not a multiple of {} (the wet re-draw would assemble an \
                 incomplete trailing triangle)",
                ground.receiver.start,
                ground.receiver.end,
                ground.receiver.end - ground.receiver.start,
                VERTICES_PER_TRIANGLE
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
            // CA-15: the reflected range's boundaries, the receiver
            // check's own contract (an empty range must still sit ON
            // a boundary — the uniform role policy above).
            if !triangle_list_boundary(range.start) {
                return Err(RendererError::Vulkan(format!(
                    "the wet ground's reflected range {index} starts at vertex \
                     {} — inside a triangle (triangle-list boundaries are \
                     multiples of {}; the mirror pass would draw primitives \
                     the lit pass never drew)",
                    range.start, VERTICES_PER_TRIANGLE
                )));
            }
            if !triangle_list_boundary(range.end - range.start) {
                return Err(RendererError::Vulkan(format!(
                    "the wet ground's reflected range {index} [{}, {}) is {} \
                     vertices long — not a multiple of {} (the mirror pass \
                     would assemble an incomplete trailing triangle)",
                    range.start,
                    range.end,
                    range.end - range.start,
                    VERTICES_PER_TRIANGLE
                )));
            }
        }
        Ok(())
    }

    /// CA-14 (Q06): the albedo set's TYPED extent — the shape every
    /// texture was validated against and the upload's `Extent2D`
    /// source. The first pass family's single square RGBA8 shape
    /// today ([`TextureExtent::PROC_ALBEDO`]); per-set extents are
    /// the design record's migration plan (`scene_contract_design`).
    pub fn albedo_extent(&self) -> TextureExtent {
        TextureExtent::PROC_ALBEDO
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

    /// THE CA-15 WITNESS (F15 — the triangle-list boundary split,
    /// verified against the pre-fix admission at the lane's base):
    /// a table CONTIGUOUS, COVERING, and NON-INVERTED — passing
    /// every legacy check — whose boundary falls INSIDE a triangle.
    /// The baseline's ground quad is [0, 6); a boundary at 5 splits
    /// its second triangle (verts 3, 4, 5): the depth pass's
    /// whole-buffer draw assembles triangles at absolute indices
    /// 0, 3, 6, … while material 0's lit draw assembles at 0, 3 and
    /// material 1's starts at 5 — different primitive sets from the
    /// same vertices (the shadow map covers triangles the lit pass
    /// never draws, and vice versa). The triangle-aligned admission
    /// refuses it, next to the VALID three-vertex boundary that
    /// admits (the split sits one vertex from a legal boundary).
    #[test]
    fn misaligned_draw_boundaries_are_refused() {
        // A boundary inside the ground quad's second triangle.
        let mut scene = valid_scene();
        scene.mesh.draws[0] = 0..5;
        scene.mesh.draws[1] = 5..scene.mesh.draws[1].end;
        let error = scene
            .validate()
            .expect_err("a mid-triangle draw boundary must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("draw range 1") && text.contains("inside a triangle"),
            "the refusal names the range and the split: {text}"
        );
        // The same split one material later (a boundary inside a
        // box's wall quad): [0, 6), [6, 37), [37, 66).
        let mut scene = valid_scene();
        let end = scene.mesh.draws[1].end;
        scene.mesh.draws[1] = scene.mesh.draws[1].start..end + 1;
        scene.mesh.draws[2] = end + 1..scene.mesh.draws[2].end;
        let error = scene
            .validate()
            .expect_err("a mid-mesh misaligned boundary must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("draw range 2") && text.contains("inside a triangle"),
            "the refusal names the range and the split: {text}"
        );
        // THE TAIL: a vertex count that is not a multiple of 3 —
        // contiguous and covering (the last range ends exactly at
        // the buffer) but the whole-mesh draw would assemble a
        // primitive set the per-range draws cannot.
        let mut scene = valid_scene();
        scene.mesh.vertices.push(scene.mesh.vertices[0]);
        let last = scene.mesh.draws.last_mut().expect("three ranges");
        last.end += 1;
        let error = scene
            .validate()
            .expect_err("a misaligned vertex count must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("not a multiple of 3"),
            "the refusal names the tail split: {text}"
        );
        // The VALID three-vertex boundary one vertex away admits —
        // the baseline itself (the ink gates' scenes: the corner and
        // garage tables are exactly this shape, so their captures
        // are unchanged by the admission).
        valid_scene()
            .validate()
            .expect("the aligned baseline still admits");
    }

    /// CA-15 on the WET ROLES: a receiver or reflected range that
    /// starts inside a triangle (the re-draw would rasterize
    /// primitives no pass authored) or carries a length that is not
    /// a multiple of 3 (an incomplete trailing primitive) refuses —
    /// and an EMPTY reflected range must still sit ON a boundary
    /// (the uniform role policy: a role range is a position in the
    /// mesh's primitive structure).
    #[test]
    fn misaligned_wet_role_boundaries_are_refused() {
        // A receiver starting inside the ground quad's first
        // triangle: [1, 7) — start misaligned, length aligned.
        let mut scene = valid_scene();
        scene.ground.receiver = 1..7;
        let error = scene
            .validate()
            .expect_err("a misaligned receiver start must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("receiver range") && text.contains("inside a triangle"),
            "the refusal names the receiver split: {text}"
        );
        // A receiver whose LENGTH splits a triangle: [0, 5) — start
        // aligned, length misaligned.
        let mut scene = valid_scene();
        scene.ground.receiver = 0..5;
        let error = scene
            .validate()
            .expect_err("a misaligned receiver length must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("receiver range")
                && text.contains("not a multiple of 3")
                && text.contains("incomplete trailing triangle"),
            "the refusal names the incomplete primitive: {text}"
        );
        // A reflected range with both splits: [7, 12).
        let mut scene = valid_scene();
        scene.ground.reflected[0] = 7..12;
        let error = scene
            .validate()
            .expect_err("a misaligned reflected range must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("reflected range 0") && text.contains("inside a triangle"),
            "the refusal names the reflected split: {text}"
        );
        // A reflected range whose length splits a triangle: [6, 11).
        let mut scene = valid_scene();
        scene.ground.reflected[0] = 6..11;
        let error = scene
            .validate()
            .expect_err("a misaligned reflected length must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("reflected range 0") && text.contains("not a multiple of 3"),
            "the refusal names the reflected length: {text}"
        );
        // An EMPTY reflected range at a misaligned position refuses
        // under the uniform policy (it draws nothing, but the author
        // is pointing inside a primitive — fail-visible, never a
        // silent oddity).
        let mut scene = valid_scene();
        scene.ground.reflected[1] = 4..4;
        let error = scene
            .validate()
            .expect_err("an empty-but-misaligned reflected range must refuse");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert!(
            text.contains("reflected range 1") && text.contains("inside a triangle"),
            "the refusal names the empty-but-misaligned range: {text}"
        );
        // The VALID shapes next to every split above admit: the
        // baseline's roles (the builder's own boundaries).
        valid_scene()
            .validate()
            .expect("the aligned roles still admit");
    }

    /// CA-14 + CA-15's valid-case pin (the ink-gate coherence): the
    /// shared builder's output is triangle-aligned at EVERY
    /// boundary (each quad is exactly six vertices — two complete
    /// triangles), and the typed admission reproduces the raw
    /// table's facts. Every live scene (the corner, the garage, the
    /// fixtures) builds through this builder, so the alignment
    /// admission changes none of them — the rig's capture gates
    /// (the ink gates) keep their exact color/depth agreement.
    #[test]
    fn the_builder_output_is_triangle_aligned_and_the_admission_reproduces_it() {
        let scene = valid_scene();
        let table = scene
            .mesh
            .admit_draws(PASS_FAMILY_MATERIAL_COUNT)
            .expect("the builder's table admits");
        // Every boundary (each start, each end) is a multiple of 3.
        for range in table.ranges() {
            assert!(
                triangle_list_boundary(range.start),
                "start {} is a primitive boundary",
                range.start
            );
            assert!(
                triangle_list_boundary(range.end),
                "end {} is a primitive boundary",
                range.end
            );
            assert!(triangle_list_boundary(range.end - range.start));
        }
        // The typed table reproduces the raw table's facts.
        assert_eq!(table.ranges(), scene.mesh.draws.as_slice());
        assert_eq!(
            table.whole_mesh_vertex_count(),
            scene.mesh.vertices.len() as u32
        );
        // The builder's own quads: six vertices each, so every
        // material's range is a multiple of 6 (two triangles per
        // quad — the alignment is structural, not incidental).
        for range in table.ranges() {
            assert_eq!(
                (range.end - range.start) % 6,
                0,
                "the box-authored range [{}, {}) is quad-multiples",
                range.start,
                range.end
            );
        }
    }

    /// CA-14 (Q06): the typed texture extent pins the procedural
    /// contract — the extent's facts are the generator's own
    /// constants (single-sourced), the byte length is the exact
    /// match bound, and the admission's texture check reads the
    /// typed shape (the refusal names the extent, not a magic
    /// byte count).
    #[test]
    fn the_typed_texture_extent_pins_the_procedural_contract() {
        let extent = TextureExtent::PROC_ALBEDO;
        assert_eq!(extent.width, crate::proc_texture::TEXTURE_SIZE);
        assert_eq!(extent.height, crate::proc_texture::TEXTURE_SIZE);
        assert_eq!(extent.bytes_per_texel, 4);
        assert_eq!(
            extent.byte_len(),
            u64::from(crate::proc_texture::TEXTURE_SIZE * crate::proc_texture::TEXTURE_SIZE * 4)
        );
        assert_eq!(extent.byte_len(), 262_144, "256×256×4");
        // The generator's own output matches the typed extent (the
        // single-sourcing pin: generator, admission, and upload read
        // one shape).
        assert!(extent.matches(&crate::proc_texture::procedural_texture(0)));
        assert!(!extent.matches(&[]));
        assert!(!extent.matches(&vec![0_u8; 262_143]));
        assert!(!extent.matches(&vec![0_u8; 262_145]));
        // An absurd authored extent saturates instead of wrapping —
        // and therefore never matches a real byte slice.
        let absurd = TextureExtent {
            width: u32::MAX,
            height: u32::MAX,
            bytes_per_texel: u32::MAX,
        };
        assert_eq!(absurd.byte_len(), u64::MAX);
        assert!(!absurd.matches(&vec![0_u8; 1_000]));
        // The description's accessor hands the same typed shape to
        // the upload path.
        assert_eq!(valid_scene().albedo_extent(), TextureExtent::PROC_ALBEDO);
    }
}
