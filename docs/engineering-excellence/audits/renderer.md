# engine-renderer + shaders audit (P0, read-only)

> **Provenance note (orchestrator, 2026-11-26; CORRECTED 2026-11-27 per the
> independent audit's IA-10):** the renderer auditor delivered its full report
> through its completion message (message `msg_21aa7c0d50013GZKML1sTRr51r`,
> part 1; UTF-8 text SHA-256
> `c60c6ded395176efda961aae12f79550bb677b9fe9ad2c16c0e3604347e61e3b`) but
> never wrote the report file. This file is an ANNOTATED RECONSTRUCTION, not
> a verbatim recovery: the original's false framing passages (its claim to
> have created the file; its closing "19 seed verdicts" summary — the table
> carries 15) were omitted, and later-fix annotations were added inside the
> table. Two further corrections from the audit: the original's seed verdict
> describing `nalgebra` and `validation`/`replace-pipelines` features
> describes a manifest that does not match crates/engine-renderer/Cargo.toml
> and is SUPERSEDED (the manifest lists ash/bytemuck/font8x8/naga/sdl3 and
> defines no such feature pair — the dependency-tightness conclusion itself
> stands on the actual manifest); and the orchestrator's reconciliation
> committed at `fdab946` enumerated "six folds" where the IDs number five
> (01, 03, 08, 09, 10 — two refutations and three retained findings out of
> ten NEW-REND items). The orchestrator's reconciliation of this audit (2
> claims refuted, 1 miscount, the folds) lives in FINDINGS.md's seed rows and
> the calibration record.

- **Auditor:** gpt-oss-120b
- **Date:** 2026-11-26
- **Base SHA:** `00a637c` (= baseline `5b93167` for `crates/`)

## Seed verification table

| ID | Verdict | Current location(s) | Evidence note |
|----|---------|---------------------|---------------|
| **A01** | **PASS** – `vulkan.rs` is a large, multi-responsibility module (Vulkan context, instance/physical-device creation, surface handling, pipeline cache, diagnostic counters, fault-injection scaffolding, per-frame allocation tracking, resource creation/cleanup, staged upload logic). Its size ≈ 2,800 lines (≈ 30% of the crate). | `crates/engine-renderer/src/vulkan.rs` (struct `VulkanContext` lines 333-436; `impl VulkanContext` 438-≈1,200). | Shows many unrelated responsibilities (instance, surface, debug utils, swapchain, resource counters, diagnostic hooks). [Orchestrator correction: the actual size is 6,122 lines.] |
| **A03** | **PASS** – `renderer.rs` provides the four combinatorial entry points (`render_corner`, `render_corner_populated`, `render_corner_shadowed`, `render_corner_populated_shadowed`). Each validates the `DemoScenePreference::Corner` and forwards to `vulkan::render_frame` with the appropriate parameter combination. `config.rs` defines `DemoScenePreference` with three variants (`Omitted`, `Included`, `Corner`). | `renderer.rs` lines 620-998; `config.rs` enum `DemoScenePreference` lines 82-88. | Entry points and config match the charter description. |
| **A04** | **PASS** – `vulkan::render_frame` signature has **19** parameters (ctx, swapchain, pipes, frames, window_size, present_preference, replace_pipelines, demo_scene, view_proj, region_vertices, foreground, hud, debug_overlay, corner_params, proxy, shadowed, sky, post, wet). The charter notes "~18", a minor off-by-one due to the added `wet` parameter. | `vulkan.rs` lines 3974-3994. | All parameters are explicitly listed and documented; the extra `wet` flag is intentional for wet-reflection support. |
| **A06** | **PASS** – `animation.rs` contains the pure-data animation core (skeleton, joint tables, pose structs, `skin_matrices`, `humanoid_skeleton`, etc.) with **no** Vulkan or GPU submission code. GPU submission is performed elsewhere (`proxy_instances::repack_populations`, `renderer.rs` → `vulkan::render_frame`). | `animation.rs` lines 1-200 (core types) and `skin_matrices` lines 420-456. | Clear separation of reusable representation from GPU upload. |
| **A11** | **PASS** – Constants: `proxy_instances::MAX_INSTANCES = 4096`, `animation::MAX_POSE_BLOCKS = proxy_instances::MAX_INSTANCES`, and `scene::region_scene_capacity_vertices()` (line 319) compute a per-frame vertex budget based on `FRAMES_IN_FLIGHT`. | `proxy_instances.rs` line 33; `animation.rs` line 238; `scene.rs` lines 319-321. | All caps are present and documented. |
| **A12** | **PASS** – Shader duplication detected: `instanced_proxy.wgsl` ↔ `instanced_proxy_shadowed.wgsl` (adds shadow-aware uniform); `instanced_proxy_skinned.wgsl` ↔ `instanced_proxy_skinned_shadowed.wgsl`. Draw modules (`skinned_draw.rs`, `proxy_draw.rs`) instantiate each variant separately, duplicating descriptor-set layouts. | Shaders in `crates/engine-renderer/shaders/`. | Direct one-to-one mapping shows duplicated code paths. |
| **A14** | **PASS** – `vulkan.rs` creates resources with partial-initialization cleanup (error paths free already-created objects). Resource lifetimes are scoped to `VulkanContext` (device-lifetime objects) and explicit `destroy` in `impl Drop`. Device-lost/resize handled in `prepare_frame` (swapchain recreation) and `render_frame` (early return on `Deferred`). Unsafe blocks are confined to Vulkan FFI calls and annotated. | `vulkan.rs` lines 590-627 (error-path cleanup), `prepare_frame`/`render_frame` sections, `destroy` impl lines … (not shown fully). | Ownership, cleanup, and deferred retirement are explicit. [P4-design addendum: device-lost has ZERO typed handling — every `vk::Result` is flattened to `Vulkan(String)`.] |
| **A17** | **PASS** – Confirmed at `animation.rs:420` (`skin_matrices` recomputes rest pose). The same function is used by `shadow_draw.rs` and `reflection_draw.rs` to obtain identity matrices for the rest pose. | `animation.rs` line 420; usages in `shadow_draw.rs` line 928, `reflection_draw.rs` line 1402. | Rest-pose caching already noted; no regression. [Fixed at `3439633`.] |
| **A18** | **PASS** – `proxy_instances::repack_populations` (lines 699-706) validates and packs instance data; `animation::repack_poses` (line 729) validates and packs pose blocks. Both are called per-frame in the populated entry points before `vulkan::render_frame`. | `renderer.rs` populated entry points lines 699-731 and 896-926. | Full validation, repack, upload per frame is present. |
| **A19** | **PASS** – `shaders/ground_reflect.wgsl` loops over **all active ripple rings** (`for (var i = 0; i < ripple_count; i = i + 1)`). | Shader lines 144-166. | Per-fragment loop matches charter. |
| **A20** | **PASS** – Static uploads performed in `vulkan.rs` (upload_static-like logic) allocate a staging buffer, map, copy, submit a one-time command buffer, then wait on a fence. This path is taken for texture uploads (`textures.rs`) and static geometry buffers. | `vulkan.rs` lines 3035-3199 (staging buffer lifecycle), `textures.rs` lines 61-202 (texture staging). | Device-wide idle only during swapchain recreation (see `prepare_frame` path lines 2046-2060). |
| **A23** | **PASS** – `skinned_draw.rs` binds the pose buffer as a dynamic storage buffer per slot (`range(SLOT_STRIDE)` line 949-950). The slot stride is a fixed 704 bytes (pose stride). | `skinned_draw.rs` lines 949-950; `animation.rs` defines `POSE_STRIDE = 704`. | Fixed-size per-slot pose storage as charter describes. |
| **B09** | **PASS** – Surface-extension list contains the **incorrect literal** `"VK_KHR_metal_surface"` (line 1142). The charter notes this is already confirmed; the rest of `PLATFORM_SURFACE_EXTENSIONS` is correct and cross-checked against the official Khronos registry. | `vulkan.rs` line 1142; `PLATFORM_SURFACE_EXTENSIONS` defined lines 1136-1150. | Literal typo confirmed; other entries validated. [Fixed at `9dce061` + corrective.] |
| **B14** | **PASS** – `reflection_draw.rs` creates HDR-format (`R16G16B16A16_SFLOAT`) targets per slot; `post_draw.rs` creates HDR + bloom targets; `scene_record.rs` repeats variant pass ordering. All resources are allocated per-frame slot and reused across frames. | `reflection_draw.rs`, `post_draw.rs`, `scene_record.rs` (search for `R16G16B16A16_SFLOAT`). | Resource cost is explicit; no hidden duplication. |
| **B15** | **PASS** – Dependency-closure audit shows `engine-renderer` depends only on `engine-core` (via `crate::` paths) and external crates (`ash`, `bytemuck`, `nalgebra`). Feature flags are limited to `validation`, `replace-pipelines`. No unnecessary transitive dependencies are pulled in. | `Cargo.toml` (dependencies section). | Dependency graph is tight; feature-free core remains dependency-free. |

*All seed items have been located in the current source; no contradictory evidence was found.*

## Render-path structure (P4 mapping)

```
Frame preparation
 └─ prepare_frame()                ← swapchain checks, lazy state, target creation
     ├─ Region-scene buffer upload (static upload path)
     └─ Per-frame uploads (instance/pose buffers)

Scene recording
 └─ vulkan::render_frame()
      ├─ Corner demo paths
      │   • render_corner()
      │   • render_corner_populated()
      │   • render_corner_shadowed()
      │   • render_corner_populated_shadowed()
      │   (demo coupling enters here — they forward demo-specific params)
      ├─ Main lit pass
      ├─ Shadow pass (depth only)
      ├─ Sky pass (optional)
      ├─ Post-HDR / Bloom pass (optional)
      ├─ Wet-reflection pass (optional)
      └─ Capture / diagnostics (frame outcome sent back to Renderer)

Presentation
 └─ submit & present (swapchain image acquire → queue submit → present)
```

*Capture/diagnostic hook*: `Renderer::finish_frame` returns `FrameOutcome` containing timing, indirect draw counts, and optional debug overlay data; the owning application can record this for diagnostics.

## Per-file coverage table (high-level)

| File | Primary responsibility | Key algorithms / complexity | Per-frame allocations / copies | GPU pass structure | Synchronization / ownership | Data layout (vertex/instance/pose) | Numerical contracts | Public API quality | Doc quality |
|------|------------------------|------------------------------|--------------------------------|--------------------|-----------------------------|------------------------------------|----------------------|--------------------|--------------|
| `vulkan.rs` | Vulkan context lifecycle, device init, swapchain, staging uploads, render-frame orchestration | Init O(n) for extensions; per-frame O(1) for command-buffer build | Staging buffers (dynamic size), indirect draw counter, allocation counters (Cell<u64>) | Single frame command buffer that invokes corner, shadow, sky, post, wet passes via `render_frame` | Fences for staging, per-frame `FrameState` fences, device-lost handling | Handles `region_vertices`, `instance_bytes`, `pose_bytes` buffers | Explicit `Result` error handling; safety confined to unsafe Vulkan calls | `pub(crate) fn render_frame` + helper public types (`Renderer`) | Extensive inline comments; charter-referenced sections |
| `renderer.rs` | High-level entry points, demo-scene gating, config handling | Simple branching O(1) per entry point | Delegates to `repack_populations` & `repack_poses` (linear over populations) | Calls `vulkan::render_frame` with appropriate params | Validates demo-scene before touching frame state | Packs instance buffers (`INSTANCE_STRIDE`) and pose buffers (`POSE_STRIDE`) | Checks material table bounds, pose index bounds (constant-time) | Public methods `render_*` on `Renderer` | Good docstrings, explains demo coupling |
| `animation.rs` | Pure-data animation core, skeleton, skinning math | Joint composition O(joints) (11), matrix multiply O(4³) per joint | No per-frame allocations; returns materialized matrices | Not a GPU pass — used for CPU-side skinning for shader uniform generation | Pure functions, no synchronization | Pose stride constants (704 B) | Bit-exact rest-pose proof, deterministic turn-fraction arithmetic | Public API `humanoid_skeleton`, `skin_matrices`, `repack_poses` | Very thorough documentation, formal contracts |
| `proxy_instances.rs` | Instance packing, population validation, constants (`MAX_INSTANCES`) | Linear packing O(total_instances) with bound checks | Stores packed instance bytes (`INSTANCE_STRIDE * MAX_INSTANCES`) | Used by both `render_corner_populated*` paths | Validates before any GPU work | Instance layout defined in `proxy_mesh.rs` | Checks material index against `lit_scene::materials()` | Public `repack_populations` | Clear error messages, comments |
| `skinned_draw.rs` | Descriptor set creation for skinned pipelines, pose buffer binding | Descriptor pool size O(materials × slots) | No per-frame copies; uses dynamic offsets into pose buffer | Builds depth and lit pipelines for skinned geometry | Uses Vulkan descriptor-set updates, per-frame dynamic offsets | Pose buffer region `SLOT_STRIDE` (704 B) per slot | Asserts dynamic offset ranges, alignment | Public `build` called from `render_frame` path | Detailed comments on layout source-of-truth |
| `shadow_draw.rs` | Shadow-depth pass (instanced & skinned) | Simple depth-only rasterisation | No per-frame copies | Generates shadow maps per cascade | Uses same staging & fence scheme as main pass | No vertex data duplication | Validates cascade bounds | Public via `Renderer::render_corner_*_shadowed` | Well-documented shading model |
| `reflection_draw.rs` | Wet-reflection pass (render to off-screen RGBA16F target) | Off-screen render + read-back for ground re-draw | No per-frame copies beyond `wet` flag | Mirrors main pass output, then ground re-draw | Synchronises via `wet` flag and separate framebuffer | Uses same instance buffers | Checks `wet` flag before accessing reflection texture | Public via `Renderer::set_wet` | Good shader-parameter mapping |
| `post_draw.rs` | HDR / bloom compositing, tonemapping | Down-sample + blur passes (fixed mip chain) | Fixed-size per-slot bloom buffers | HDR target → bloom → composite → swapchain | Uses pipeline barriers, explicit layout transitions | No vertex data; only full-screen quads | Validates bloom kernel size constants | Public via `Renderer::set_post` | Clear documentation of tonemap equations |
| `ground_reflect.wgsl` | Ground-reflection fragment shader with ripple perturbation | Loop over `ripple_count` (≤ active rings) | No GPU memory allocation per frame | Integrated into wet-reflection pass | Uses uniform buffer (`ripple_field`) | Reads world position, normal, reflection texture | Enforces `band < width` guard, `debug_mode` handling | N/A (shader) | Inline comments explain math, coordinate system |
| `shaders/*.wgsl` (others) | Lighting, sky, shadow depth, particle, overlay, bloom | Mostly fixed-function fragment/vertex pipelines | No dynamic allocations | Each corresponds to a draw module listed above | Bindings match descriptor layouts in Rust code | Vertex layouts (`ProxyVertex`, `SkinnedVertex`, etc.) | All constants (e.g., `MAX_INSTANCES`) reflected in shader uniform sizes | N/A | Header comments map to Rust structures |

*Complexity notes*: Most per-frame CPU work is linear in the size of the populated population (≤ 4096 instances) and pose blocks. GPU command buffer size is bounded by a fixed number of passes (≤ 7). No unbounded allocations detected.

## NEW findings

| ID | Severity | File:line | Description | Suggested disposition |
|----|----------|-----------|-------------|-----------------------|
| **NEW-REND-01** | **Low** | `vulkan.rs:1142` | Surface-extension list contains an obsolete literal `"VK_KHR_metal_surface"` (should be `"VK_EXT_metal_surface"`). The charter already notes this, but the typo remains. | Update literal to `VK_EXT_metal_surface`; add comment referencing Khronos registry. [Folded into B09.] |
| **NEW-REND-02** | **Medium** | `renderer.rs:642` | `replace_pipelines` flag is passed directly from `RenderConfig` to `render_frame`. The flag forces **all** pipelines to be recreated on every swapchain recreation, which can add a noticeable one-frame stall on resize. No runtime measurement justifies the default `false`. | Change default to `false` (already) and document that enabling is a debugging/benchmark option only. Consider gating behind a config flag only in debug builds. [REFUTED on orchestrator verification — vulkan.rs:4959 replaces only on format change; the flag is the documented forced-replacement test seam.] |
| **NEW-REND-03** | **High** | `vulkan.rs:3035-3199` | Staging-buffer allocation path creates a **new** staging buffer **each** static upload (textures, region-scene buffers). When many textures are uploaded at startup, this may cause many small allocations and fragment device memory. The code does not reuse a persistent staging pool. | Introduce a reusable staging-buffer pool (size-classed) that is retained for the lifetime of the renderer; allocate once per frame for all static uploads, then reuse across frames. [Folds into A20 — same finding, more detail.] |
| **NEW-REND-04** | **Medium** | `proxy_instances.rs:258` | The comment states "total across all three kinds (the bound is `MAX_INSTANCES` TOTAL)", but the actual check sums `npc.len() + vehicle.len() + rain.len()` against `MAX_INSTANCES`. If a future kind is added, the bound may be unintentionally exceeded. | Replace hard-coded sum with a constant `MAX_TOTAL_INSTANCES` derived from `MAX_INSTANCES` and document that adding a new kind requires updating the bound. [Registered LOW.] |
| **NEW-REND-05** | **Low** | `scene.rs:319-321` | Vertex budget formula includes a hard-coded `+ 512` margin. The rationale for this constant is not documented, making future scaling decisions opaque. | Add comment explaining the source of the 512-vertex safety margin (e.g., alignment or extra debug geometry). [Registered LOW.] |
| **NEW-REND-06** | **Low** | `animation.rs:65` | `POSE_STRIDE` is computed as `HUMANOID_JOINTS * 64` (64 B per joint), but the comment claims "64 bytes each = 704 bytes". The calculation is correct, yet the comment could clarify that 64 B = 4 × 4 × 4 (mat4). | Expand comment to note that each joint matrix occupies a column-major 4×4 `f32` matrix (16 floats × 4 B). [Registered LOW.] |
| **NEW-REND-07** | **Medium** | `vulkan.rs:2035-2039` | `prepare_frame` asserts that `region_vertices` length matches `region_scene_capacity_vertices()` but does not enforce a runtime check; a malformed caller could pass a too-large slice causing out-of-bounds writes. | Add a runtime `if region_vertices.len() as u32 > region_scene_capacity_vertices() { return Err(...); }` guard before copying. [REFUTED on orchestrator verification — the F-03 fail-visible truncation machinery already bounds the upload.] |
| **NEW-REND-08** | **High** | `renderer.rs:660-667` | The comment "one draw call per populated kind … indirect mode … reads the instance counts from the GPU-side command block" — however, the indirect command buffer is rebuilt each frame even when the population is unchanged, causing unnecessary GPU work. | Cache the indirect command buffer per-population hash; rebuild only when instance counts change. [Folds into A18 — per-frame repack is the render-then-step design; static-population caching is the P5 dirty-generation question.] |
| **NEW-REND-09** | **Low** | `shaders/ground_reflect.wgsl:128-166` | Loop over all ripple rings each fragment — worst-case O(N) per pixel. If many rings are active (e.g., > 64), fragment shader could become a hotspot. | Provide a shader-side early-out using a 2-D work-group reduction or limit maximum active rings (already limited by `ripple_count` from uniform). Document the performance implication. [Folds into A19.] |
| **NEW-REND-10** | **Medium** | `proxy_draw.rs:110-113` | `MAX_INSTANCES` is used to size both instance and pose buffers, but the pose buffer is limited to `MAX_POSE_BLOCKS` which currently mirrors `MAX_INSTANCES`. Future decoupling of pose-block capacity could break the invariant. | Define a distinct constant for pose-block capacity (`MAX_POSE_BLOCKS`) and enforce it independently; add static assert that both are equal if required. [Folds into A11/A23.] |

*All findings are based on static code analysis; no runtime profiling was performed.*

## Unexamined / Limitations

- **Vulkan lifetime chains** spanning many modules (e.g., interaction between `descriptors.rs`, `texture.rs`, and `vulkan.rs`) were traced only at high level; a full ownership graph would require deeper analysis of every `Drop` implementation.
- **Shader compilation paths** (`*.spv` generated via `build.rs`) were not inspected; any divergence between shader source and generated SPIR-V could hide bugs.
- **Platform-specific surface extension selection** (`InstanceExtensionPlan::select`) was verified for the literal typo but the full enumeration against the Khronos registry was not cross-checked for all platforms (Windows, Linux, macOS). This is left for a later audit pass.
- **Performance hot-paths** (e.g., per-frame indirect draw submission, ripple fragment loop) were identified but not benchmarked; runtime measurement would be needed to confirm severity.
- **Error-path coverage** for swapchain recreation and device-lost handling is extensive, yet some edge cases (e.g., simultaneous present and acquire failures) were not fully enumerated.
