//! # CYCLE 6-B IMPLEMENTATION BRIEF — Materials & the First Lit Scene
//!
//! **Same-conversation delta:** you (the implementer) built 6-A; the
//! repo, the verification loop, the discipline, and the descriptor/
//! texture foundation are yours already. This brief covers only
//! what's NEW. The full ground rules live in `docs/CYCLE6-6A-BRIEF.md`
//! and your prior handoff — they still bind.
//!
//! ## The goal
//!
//! "One Corner of the City" renders **lit, textured, and measured**:
//! the 40 m intersection with wet-look asphalt, four building masses,
//! the fixed camera route, day and night lighting presets — plus the
//! whole-route frame-time evidence (live `TimingSamples` AND exact
//! capture-based measurement, per decision D3's dual regime). This is
//! the first *judged* visual milestone: from here, renderer
//! investments are measured against image quality and route-wide
//! smoothness, not microbenchmarks.
//!
//! ## What is DONE on this branch (tests pin it — do not redo)
//!
//! - `crates/engine-renderer/src/lit_scene.rs` — **the scene model**:
//!   the four building masses framing the 8 m road cross (validity-
//!   tested: non-degenerate, above ground, inside the world, outside
//!   the road), the three-material table (wet asphalt / concrete /
//!   dark stone), the day and night `LightParams` presets (normalized
//!   directions tested), the six-waypoint fixed camera route
//!   (validity-tested), and — the linchpin — **`lit_surface_color`,
//!   the CPU reference lighting oracle** (Blinn-Phong, even-power-of-
//!   two shininess via repeated squaring so CPU and GPU agree without
//!   `pow`; known-answer tests pin facing-lit, facing-away-ambient-
//!   only, and night-dimmer-than-day).
//! - `crates/engine-renderer/src/proc_texture.rs` — **the deterministic
//!   procedural albedo textures** (3 textures, 256², RGBA8, seeded
//!   value noise, byte-identical across platforms — determinism and
//!   distinctness tested).
//! - `crates/engine-renderer/shaders/lit_scene.wgsl` — **the lit
//!   shaders** (the exact equation, term-for-term, with the repeated-
//!   squaring specular and `countTrailingZeros`).
//!
//! ## YOUR WORK
//!
//! 1. **The lit vertex format + mesh generation:** a `LitVertex`
//!    (pos.xyz, normal, uv — your format choice, documented) and a
//!    mesh builder that turns `intersection_boxes()` + the ground
//!    plane into lit vertices with per-face normals and sensible UVs
//!    (world-space planar mapping is fine — document the choice).
//! 2. **The per-draw material binding:** extend the 6-A descriptor
//!    foundation to per-material texture sets (3 textures, 3 sets —
//!    or one set rebound per draw; your choice, documented; the
//!    no-per-frame-allocation contract from 6-A still binds). The
//!    material + light parameters ride push constants (the shader's
//!    `PushConstants` defines the layout — mind the 128-byte limit).
//!    NOTE the shader's view-direction shortcut: it derives `v` from
//!    `-world_pos`; replace with the true camera position in the
//!    push constants when wiring (an integration decision to document).
//! 3. **The lit pipeline:** a new pipeline alongside the scene and
//!    overlay pipelines (the WGSL has both entry points; depth-tested,
//!    the scene's depth format); the demo path renders the corner
//!    scene through it with the route camera (interpolate the
//!    waypoints at a fixed cadence — the whole-route requirement).
//! 4. **The day/night toggle** (a demo flag; both presets from
//!    `lit_scene.rs`).
//! 5. **The measurement evidence:** a `--corner` demo mode running
//!    the full route; the run summary prints the route's CPU/GPU
//!    frame-time distributions (`TimingSamples` for live + an exact
//!    capture-based p50/p95/p99 via frame-interval recording) and
//!    peak memory. The CLAIM uses the exact numbers (D3).
//! 6. **The lighting-equivalence gate:** a lavapipe capture test
//!    comparing the GPU output against the CPU oracle — render a
//!    small known scene (one box face, known normal/view/light), read
//!    back the pixels, and compare against `lit_surface_color` within
//!    a DOCUMENTED tolerance (shader compilers may reassociate; the
//!    6-A palette-index discipline is the model). Pin what you assert.
//!
//! ## Acceptance criteria (merge-blocking, all six)
//!
//! 1. The corner scene renders lit + textured on **lavapipe (CI)** and
//!    on the **5070 Ti rig** (the owner runs it — provide the command).
//! 2. The lighting-equivalence gate passes with its tolerance
//!    documented and its assertion pinned.
//! 3. The route measurement prints both regimes' numbers; the
//!    exact-measurement path is the claim's source.
//! 4. No per-frame allocation: `renderer.allocations()` constant
//!    across warmed route frames (assert it in the test).
//! 5. Every existing gate holds: fmt, clippy `-D warnings`, exact-
//!    count pins re-measured for new tests (never weakened), the
//!    five-job CI, the lavapipe smokes, fingerprints byte-identical.
//! 6. Determinism: the scene, textures, and route are pure data —
//!    the sim, the spine, and every fingerprint untouched.
//!
//! ## Deferred (do NOT pull in)
//!
//! Instanced/indirect draws (separate package 6-B2), NPCs/vehicles
//! (engine-slice composition later), shadow mapping, wet-surface
//! reflection tricks beyond the specular model, asset loading (the
//! procedural textures ARE the 6-B asset story). One PR, this scope
//! only.
