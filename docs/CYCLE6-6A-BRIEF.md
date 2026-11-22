//! # CYCLE 6-A IMPLEMENTATION BRIEF — Rendering & Texture Foundations
//!
//! **The owner's division of labor:** this scaffold (module structure,
//! contracts, the deterministic glyph-atlas implementation with its
//! tests, and the shaders) is complete and verified; **the
//! implementer fills in the three stubbed Vulkan bodies** named
//! below, wires the integration, and lands the acceptance evidence.
//!
//! **The goal:** the first sampled-texture render path — the
//! descriptor/texture/material plane the engine currently lacks —
//! proven on the highest-value first customer: replacing the
//! per-pixel overlay glyph rasterization (the largest measured
//! per-frame CPU cost in the renderer: ~90,000 vertices / ~2.2 MB
//! rebuilt per frame; the atlas form is ~4,500 vertices, a ~20x
//! reduction — `render_benchmark` section D is the before evidence).
//!
//! ## What is DONE (do not redo; the tests pin it)
//!
//! - `crates/engine-renderer/src/glyph_atlas.rs` — the deterministic
//!   atlas: the 128x128 R8 bitmap from the pinned font8x8 legacy
//!   font, the UV table, and `push_glyph_quads` (the legacy text
//!   metrics reproduced exactly: 9.0-px advance, 10.0-px line height,
//!   the 6,000-glyph cap, non-finite refusal). Three tests pin the
//!   layout, the UV partition, and the metrics.
//! - `shaders/overlay_atlas.vert` / `shaders/overlay_atlas.frag` —
//!   the atlas overlay shaders (UV in, sampled luminance x tint
//!   out). Build.rs picks them up automatically (it compiles every
//!   shader in `shaders/`).
//! - The contracts in `textures.rs` and `descriptors.rs` (below).
//!
//! ## The three stubbed bodies (the implementation work)
//!
//! | # | Location | What |
//! |---|---|---|
//! | 1 | `textures.rs :: Texture::upload` + `SamplerPool::new` | The image creation, one-shot staging upload, SHADER_READ_ONLY transition, view creation, and the nearest sampler. Follow the file's contract docs. |
//! | 2 | `descriptors.rs :: DescriptorFoundation::new` | The pool + single-binding layout + the immutable combined-image-sampler set. |
//! | 3 | The `vulkan.rs` integration | Wire the atlas into the overlay pipeline: the overlay pipeline layout gains the descriptor set layout; the overlay vertex format gains UV (see the format note below); the recorded command buffer binds the set before the overlay draw; the overlay text path switches from `push_text_px` to `push_glyph_quads` + the atlas quads. |
//!
//! ## The vertex-format note
//!
//! The existing `OverlayVertex` (pos + color) must keep serving the
//! rectangle overlays. The atlas text path needs pos + uv + color.
//! Either extend `OverlayVertex` with a UV (rectangles use
//! `[0,0,1,1]`) or introduce a second vertex buffer + binding —
//! the implementer chooses, documents the choice in the PR, and
//! keeps the F-03 capacity/truncation counters coherent for BOTH
//! overlay sub-paths.
//!
//! ## Acceptance criteria (the PR cannot merge without all)
//!
//! 1. **Pixel equivalence:** the offscreen capture comparison
//!    (`render_scene_separation`'s capture discipline) proves the
//!    atlas-rendered overlay text is visually equivalent to the
//!    legacy per-pixel output (same glyphs, same positions, same
//!    colors; per-texel exact match is NOT required — sampled
//!    nearest-filter of the same bitmap is, and the test should pin
//!    what it actually asserts).
//! 2. **The measured win:** `render_benchmark` gains an atlas-form
//!    section D variant; the report shows the ~20x vertex-count
//!    reduction and the CPU-time delta on the rig (live diagnostics
//!    may use `TimingSamples`; the CLAIM uses exact measurement —
//!    decision D3's dual regime).
//! 3. **No per-frame descriptor/texture/sampler work:** a code-level
//!    assertion (grep or test) that the frame loop touches no
//!    vkAllocateDescriptorSets/vkAllocateMemory/vkCreateSampler.
//! 4. **The allocation count reported:** the run summary prints the
//!    texture/pool allocation count (the audit's P1-4 discipline:
//!    documented, never silent).
//! 5. **Every existing gate holds:** fmt, clippy `-D warnings`, the
//!    exact-count CI pins (re-measured for any new tests — never
//!    weakened), the five-job matrix, the lavapipe offscreen smokes,
//!    and the fingerprint pins byte-identical (the overlay is
//!    observational — the sim is untouched).
//! 6. **The stub markers removed:** every `STUB` comment and the
//!    `#[allow(dead_code)]` allowances on the now-wired types are
//!    deleted in the implementation PR.
//!
//! ## The discipline (binding — the a4eb372 lesson)
//!
//! - One PR, the scope ABOVE ONLY. No mixed-scope riders.
//! - Fail-first where a defect class exists; measured pins; never
//!   weaken a test.
//! - `unsafe` stays confined to the private vulkan.rs pattern behind
//!   the invariants-manifest discipline; new unsafe in textures.rs
//!   or descriptors.rs follows the same commented-contract form.
//! - Determinism untouched: the sim, the spine, the fingerprints.
//! - Direct questions to the orchestrator (GLM); the owner's
//!   sign-off gates the merge.
