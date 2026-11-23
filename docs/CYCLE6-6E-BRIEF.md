//! # CYCLE 6-E IMPLEMENTATION BRIEF — Instanced Draws
//!
//! **Audience:** the implementing agent (autonomous or attended).
//! The ground rules live in `docs/AUTONOMOUS-CYCLE-PROTOCOL.md` (if
//! running unattended) and the repo's standing discipline; this
//! brief covers WHAT to build and the merge-blocking acceptance
//! criteria.
//!
//! ## The goal
//!
//! **One draw call for N proxies** instead of N vertex rebuilds —
//! closing the O(entities) CPU ceiling from the original renderer
//! audit. The corner scene's population (24 NPCs, 2 vehicles) and
//! every population-scale scene after it rests on this.
//!
//! ## What is DONE on this branch (tests pin it — do not redo)
//!
//! - `crates/engine-renderer/src/proxy_instances.rs` — **the
//!   deterministic instance core**: the `InstanceTransform` model
//!   (position + yaw as a `u16` turn fraction + uniform scale — the
//!   turn fraction exists because `f32::sin/cos` bits vary across
//!   libm platforms; `sin_cos_turn` is a fixed-order Taylor over
//!   pure IEEE arithmetic, ≤ 2e-6 error, bit-identical everywhere),
//!   `instance_columns` (the column-major T·R·S matrix, the pinned
//!   quarter-turn convention: turn 16384 maps +X→−Z), the 96-byte
//!   `INSTANCE_STRIDE` packing (`f32::to_bits`, little-endian, fixed
//!   field order: 4 matrix columns, tint rgb + alpha 1.0, 16 reserved
//!   zero bytes), `MAX_INSTANCES = 4096`, the typed
//!   `InstancePackError` refusals (capacity and per-field validation
//!   — no partial buffers, ever), and the **cross-platform FNV-1a
//!   byte pin** (the fingerprint-pin discipline applied to the
//!   instance buffer: CI on all three platforms asserts the same
//!   packed-bytes hash).
//! - `crates/engine-renderer/src/proxy_mesh.rs` — **the shared proxy
//!   geometry**: `npc_proxy_mesh()` (3-box humanoid, 108 vertices,
//!   origin at the feet) and `vehicle_proxy_mesh()` (2-box sedan,
//!   72 vertices, origin at ground center), the lit pipeline's
//!   32-byte vertex format, per-face unit normals, planar UVs,
//!   counts and byte-determinism pinned by tests.
//! - `crates/engine-renderer/shaders/instanced_proxy.wgsl` — **the
//!   instanced shaders**: locations 0–2 per-vertex (the mesh),
//!   locations 3–7 PER-INSTANCE (the four matrix columns + tint) —
//!   the Vulkan side marks the instance binding with
//!   `VK_VERTEX_INPUT_RATE_INSTANCE`; WGSL sees ordinary inputs. The
//!   binding set is IDENTICAL to `lit_scene.wgsl` (0/1 albedo +
//!   sampler, 2 FrameParams with the eye in the w components, 3
//!   MaterialParams) — **the 6-B descriptor foundation is reused
//!   as-is; this shader adds no bindings.** The fragment stage is
//!   `lit_scene.fs_main` with one change: the per-instance tint
//!   multiplies the sampled albedo BEFORE the equation.
//!
//! ## YOUR WORK
//!
//! 1. **The instance-rate vertex input state** (ash side): binding 0
//!    = the proxy mesh (vertex rate), binding 1 = the packed
//!    instance bytes (instance rate, stride `INSTANCE_STRIDE`),
//!    locations 3–7 as `R32G32B32A32_SFLOAT`. Document the choice
//!    where the scaffold's shader header does.
//! 2. **The instance buffer management**: the per-frame instance
//!    bytes must reach the GPU with **zero per-frame allocation**
//!    (the standing gate): a persistent staging buffer + copy, or a
//!    host-visible double/ping-pong buffer — your choice, documented;
//!    `renderer.allocations()` must stay constant across warmed
//!    frames (assert it in the test).
//! 3. **The instanced draw**: one `vkCmdDraw` (or indexed draw) with
//!    `instanceCount = N` per proxy kind, integrated where the
//!    corner demo builds its scene — a `--corner --populated` demo
//!    mode placing a deterministic NPC/vehicle population (seeded or
//!    fixed-table — pure data, document the choice) around the
//!    intersection.
//! 4. **THE EQUIVALENCE GATE** (the linchpin): a lavapipe capture
//!    test proving the instanced draw is **pixel-identical** to the
//!    same population drawn through the existing per-object path
//!    (the `render_glyph_atlas` / 6-B pixel-equivalence pattern):
//!    same meshes, same transforms, same tints — N draws vs 1
//!    instanced draw, byte-equal captured frames. Pin what you
//!    assert.
//! 5. **The measurement evidence** (D3 dual regime): draw-call count
//!    before/after (pinned integers — the claim's headline), CPU
//!    p50/p95/p99 before/after via `TimingSamples` + exact capture,
//!    on the rig for the CLAIM (the owner runs it — provide the
//!    command) and lavapipe for the correctness smoke.
//!
//! ## Acceptance criteria (merge-blocking, all five)
//!
//! 1. The equivalence gate passes: N-draws vs 1-instanced-draw
//!    pixel-identical, assertion pinned, on lavapipe CI.
//! 2. Draw-call count for the populated corner scene drops from N
//!    proxy draws to ≤ 2 (one per proxy kind) — pinned in a test.
//! 3. Zero per-frame allocation: `renderer.allocations()` constant
//!    across warmed populated-route frames (asserted).
//! 4. Every existing gate holds: fmt, clippy `-D warnings`, exact
//!    pins re-measured for new tests (never weakened), the five-job
//!    CI, fingerprints byte-identical, engine-core untouched.
//! 5. The rig measurement commands provided; the CLAIM cites exact
//!    numbers (the owner's run is authoritative).
//!
//! ## Deferred (do NOT pull in)
//!
//! Indirect draws (`vkCmdDrawIndirect`, GPU-side counts — 6-E2,
//! a separate package after this lands), GPU culling, animation
//! (skeletal or vertex morphing — the current proxies are static
//! tints/transforms), crowd LOD beyond the existing region director.
//! One PR, this scope only.
