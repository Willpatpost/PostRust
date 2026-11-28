//! THE CA-14 DESIGN RECORD (F14/Q06/R06 — the scene contract's
//! generalization), in the P5-design format. This module is the
//! DESIGN artifact; the bounded first step it recommends is
//! IMPLEMENTED in [`crate::scene_description`]
//! ([`TextureExtent`], [`crate::scene_description::DrawTable`],
//! [`crate::scene_description::SceneMesh::admit_draws`]) and
//! consumed in `vulkan/passes/corner.rs` (the retained typed table,
//! the typed extent upload) and `vulkan/passes/shadow_draw.rs`
//! (the typed whole-mesh count). The split is documented honestly
//! below: what landed, what is deferred, and why.
//!
//! ## Evidence
//!
//! The audit (F14, Q06/R06) against `95408c9`, verified on this
//! tree (the line numbers have drifted with the R1 budget work,
//! the substance unchanged):
//!
//! - `scene_description.rs` — [`crate::scene_description::
//!   PASS_FAMILY_MATERIAL_COUNT`] = 3: the material table's length
//!   is the pass families' COMPILED shape (shadow_draw's and
//!   skinned_draw's `MATERIAL_COUNT` fixed arrays, the reflection
//!   state's six reflected sets — `material·slots + slot`), so
//!   attach refuses any other count. The corner authored exactly
//!   three materials; the second scene (engine-slice's garage)
//!   authors three to FIT the compiled shape — the S6 disposition
//!   documented at the constant.
//! - `GroundRoles::reflected: [Range<u32>; 2]` — the reflection's
//!   range shape is a fixed two-entry array (the mirror reflects
//!   exactly two building masses, the corner's authored content).
//! - The albedo contract is one square RGBA8 shape
//!   (`TEXTURE_SIZE²·4` bytes per texture — 256×256×4), checked as
//!   a magic byte arithmetic and re-stated as a bare `Extent2D` at
//!   the upload.
//!
//! The consequence the audit names: the PUBLIC scene contract
//! encodes the FIRST pass family's assets. A scene whose authored
//! shape differs — four materials, one reflected mass, non-square
//! or per-material texture extents — cannot be attached at all
//! (fail-visible, the S6 refusal), and the renderer's resource
//! topology cannot be negotiated per pass. The garage proves
//! reuse INSIDE the family's shape, not generalization beyond it.
//!
//! ## Options
//!
//! 1. **Full generalization in one lane** — dynamic material
//!    counts through every per-material family (the shadowed and
//!    skinned set arrays, the reflection's reflected-set layout,
//!    the uniform tail's material slots), per-pass resource
//!    negotiation at attach (each pass family declares the table
//!    facts it needs — counts, extents, range shapes — and the
//!    attach builds exactly those), a second resource topology
//!    with different counts and dimensions, switch/release tests,
//!    and matched captures. REJECTED for this lane: it rebuilds
//!    the descriptor-set families of four pass modules, their
//!    pipeline layouts, and the reflection set indexing — each
//!    with device-battery evidence to re-pin (the rig ink gates)
//!    — well beyond one lane's verified-change budget, and the
//!    audit's own severity is Medium with NO live consumer outside
//!    the compiled shape (both live scenes fit it).
//! 2. **Bounded first step + this design record** (the audit's
//!    proposal split at its natural seam) — land the TYPED
//!    contract surface the generalization builds on: the typed
//!    texture extent (the shape named once, admission and upload
//!    single-sourced through it) and the typed material/draw table
//!    (the admission's whole validation moved into
//!    `SceneMesh::admit_draws`, the retained `DrawTable` the pass
//!    machinery consumes through named facts). The compiled-family
//!    boundaries (the material-count equality, the two-entry
//!    reflected array) stay the documented refusals they are
//!    today. ACCEPTED — implemented in this lane.
//! 3. **Nothing until a real consumer needs it** — the S6
//!    disposition extended. REJECTED: the audit asks for the
//!    typed surface now because every month the contract stays
//!    untyped, another consumer hard-codes the first family's
//!    shape at its side of the boundary (the cost compounds).
//!
//! ## Recommendation (what landed, and the migration plan)
//!
//! LANDED (this lane, `engine-renderer` only):
//!
//! - `TextureExtent` — the typed extent, `PROC_ALBEDO`
//!   single-sourced from the generator's constants;
//!   `SceneDescription::albedo_extent()` is the upload's source
//!   (corner.rs's build consumes it instead of the raw constant).
//! - `DrawTable` + `SceneMesh::admit_draws` — the whole draw-table
//!   validation as a typed constructor (one-per-material,
//!   first-start-0, non-inverted, contiguous, covering, and — with
//!   CA-15 — triangle-aligned boundaries); the retained table
//!   gives the consuming passes named facts
//!   (`whole_mesh_vertex_count` for the depth pass, `ranges()` for
//!   the lit loop) instead of raw-table re-derivations.
//! - The admission-side tests: the typed extent's pins, the
//!   aligned-builder pin (the ink-gate coherence), and CA-15's
//!   misalignment witnesses.
//!
//! DEFERRED (the migration plan, one verified step per lane):
//!
//! 1. **The material table's dynamic count** — the shadowed and
//!    skinned per-material set families move from fixed arrays to
//!    `Vec`-backed tables built at attach against the admitted
//!    count (the corner's `sets: Vec<vk::DescriptorSet>` — already
//!    dynamic — is the pattern; shadow_draw's `MATERIAL_COUNT`
//!    arrays and skinned_draw's `sets:
//!    [vk::DescriptorSet; MATERIAL_COUNT]` follow). The
//!    uniform-tail's material slots size from the count. The
//!    admission's equality check becomes an upper bound against
//!    the NEGOTIATED count. Evidence: the attach battery plus the
//!    rig ink gates (byte-identical captures for the existing
//!    three-material scenes), a four-material fixture refusing
//!    before the step and admitting after.
//! 2. **Per-pass resource negotiation at attach** — each pass
//!    family declares the table facts it consumes (a small
//!    `PassNeeds` record: material count, reflected-role count,
//!    texture extent); the attach validates the description
//!    against the DECLARED needs and builds exactly those
//!    resources. The `GroundRoles::reflected` fixed array becomes
//!    a negotiated count (the reflection set layout's
//!    `material·slots + slot` indexing generalizes with it).
//! 3. **The second resource topology** — a scene fixture with
//!    different counts and dimensions (e.g. five materials, one
//!    reflected mass, 128×128 albedo), the invalid role/range
//!    cases (a role naming a material the topology does not
//!    carry, a range outside the negotiated count), switch/release
//!    tests (attach → detach-by-drop → attach on fresh renderers),
//!    and matched captures against the CPU oracle — the audit's
//!    acceptance tests, each pinned device-side.
//! 4. **Texture extents per set/material** —
//!    `SceneDescription::albedo_extent()` generalizes from the
//!    single `PROC_ALBEDO` constant to the attached set's own
//!    typed extents; the upload loop and the admission follow the
//!    same type (no new arithmetic sites).
//!
//! The approved full-resolution reflection default (R06's
//! "preserve the approved full-resolution reflection default") is
//! untouched by every step — the reflection TARGETS' extents are
//! pass-owned resources, not scene-contract data.
//!
//! ## Acceptance tests for the deferred steps
//!
//! Each migration step lands with: the refusal-then-admission
//! fixture pair at the admission boundary (device-free), the rig
//! ink gates re-pinned byte-identical for every existing scene
//! (the corner, the garage — the captures prove no existing
//! contract moved), and the new-topology fixture's matched
//! captures (the CPU oracle's own discipline).
