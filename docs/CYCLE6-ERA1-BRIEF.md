//! # CYCLE ERA-1 IMPLEMENTATION BRIEF — The Reference Scene, Finished
//!
//! **Audience:** the overnight autonomous agents. The law:
//! `docs/AUTONOMOUS-CYCLE-PROTOCOL.md` (as amended — see its §5 for
//! the diminishing-returns stop rule). This brief covers the seven
//! packages that finish Era 1, in order. The fidelity target is
//! OWNER-RATIFIED: **stylized-coherent** — real silhouettes, strong
//! lighting, good skies, tasteful post; NOT realism (that begins
//! with Era 3's asset pipeline). Work the ladder top to bottom;
//! one branch + one PR each; push and open the draft PR as soon as
//! the package compiles and its gates pass locally (push-as-you-go
//! — no handoff kits; the owner merges in the morning).
//!
//! ## What is DONE on this branch (tests pin it — do not redo)
//!
//! - `sky.rs` — the sky-gradient + sun-disc color function (libm-
//!   free: smoothstep arithmetic, fixed-literal disc constants),
//!   the day/night sky palettes, and `atmosphere_at(t)` — the full
//!   cycle with BIT-EQUAL endpoints to the 6-B presets.
//! - `shadow_filter.rs` — the PCF contract: the 3×3 row-major tap
//!   table and the CPU-side `pcf_factor` (both sides share the
//!   table, the compare, the bias, and the out-of-bounds-reads-
//!   clear convention).
//! - `post.rs` — the ACES tonemap (Narkowicz, with the documented
//!   negative-input guard), the 5-tap separable bloom kernel
//!   (weights sum exactly 1.0), and the smooth-knee bright pass.
//! - `planar.rs` — the ground-mirror matrix (reflects about
//!   y = GROUND_Y; self-inverse; fixes the plane) and Schlick's
//!   reflection blend (r0 = 0.02, pow5 by repeated multiplication).
//! - `populated_meshes.rs` — the v2 silhouettes: the eleven-box
//!   humanoid (396 vertices) and the sedan with four cylinder
//!   wheels (552 vertices) — the wheels' ring points from the
//!   libm-free `sin_cos_turn`, per-vertex radial side normals,
//!   flat cap fans.
//! - `shaders/sky.wgsl` + `shaders/post.wgsl` — the sky and post
//!   equations mirrored term-for-term (fullscreen-triangle forms;
//!   the binding layouts are yours to finalize and document —
//!   build.rs's binding map covers 0–3).
//!
//! ## THE LADDER (in order)
//!
//! 1. **Free-look** (`--free-look`): WASD + mouse camera in the
//!    corner demo. PRESENTATION-ONLY: the camera never touches the
//!    spine, fingerprints, or any measured claim; the demo prints
//!    its usual series only in the scripted mode. Reuse the Stage-2
//!    6-DoF camera machinery in engine-shell/engine-slice; wire the
//!    input intents to position/yaw/pitch; clamp to the world AABB.
//!    A small package — do it first so every later package can be
//!    eyeballed in it.
//! 2. **Soft shadows**: the PCF gate — the shadowed shader filters
//!    the validated map with `shadow_filter.rs`'s table; the CPU
//!    oracle computes the SAME filtered factor over the SAME map
//!    bytes; the gate compares per-pixel within a documented
//!    tolerance (the map itself is already proven to 2.68e-7).
//!    Visual acceptance: penumbrae soften, no new acne, no
//!    peter-panning (the 6-F bias discipline holds).
//! 3. **The sky + sun disc**: render `sky.wgsl` behind the scene
//!    (depth = far, drawn first); drive it from `atmosphere_at`'s
//!    palettes. The gate: known-answer pixel checks at the
//!    gradient extremes and the disc center vs `sky_color` (the
//!    6-B oracle discipline — the CPU function is the reference).
//! 4. **Day/night cycle**: `--day-night` (a cycle speed; the
//!    atmosphere advances with the demo clock; free-look scrubbing
//!    optional). The gate: endpoints EXACTLY the 6-B presets
//!    (already pinned in sky.rs); the lit scene's parameters ride
//!    the same `atmosphere_at` frame.
//! 5. **Population v2**: swap `proxy_mesh` for `populated_meshes`
//!    in the population path (the instance transform contract is
//!    unchanged — origin at feet/ground-center). The gates: the
//!    existing equivalence gates re-pinned to the new mesh counts;
//!    the visual acceptance is the owner's.
//! 6. **Post-processing**: the render-target chain (HDR scene
//!    target → downsampled bloom mip(s) → the post pass →
//!    swapchain). THE architectural step — document the chain. The
//!    gates: the tonemap/bloom equations vs `post.rs` (known-answer
//!    pixel checks: an HDR value above 1 tonemaps to the CPU-
//!    computed output); the sun disc deliberately exceeds 1.0 so it
//!    blooms (pinned by a pixel check).
//! 7. **Wet-ground reflection**: the planar pass — re-render the
//!    scene mirrored (`planar.rs`'s matrix; document the winding
//!    flip) into an offscreen target; the ground shader blends it
//!    by `reflection_blend`. The gate: known-answer blend checks at
//!    grazing vs overhead view angles (the CPU function is the
//!    reference); visual acceptance is the owner's.
//!
//! ## Acceptance criteria (merge-blocking, every package)
//!
//! 1. The package's gates pass on lavapipe CI (the existing
//!    enforced steps extended to the new ones) with tolerances
//!    documented and assertions pinned.
//! 2. `renderer.allocations()` constant across warmed frames
//!    (asserted — the standing zero-alloc gate).
//! 3. Every existing gate holds; pins re-measured for new tests,
//!    never weakened; the five-job CI green.
//! 4. The rig commands provided in the PR body (the owner's run is
//!    the CLAIM); lavapipe numbers labeled as correctness evidence.
//! 5. Determinism untouched: engine-core out of scope; fingerprints
//!    byte-identical; no new libm on any byte-packed path.
//!
//! ## The stop rule (protocol §5)
//!
//! When the ladder is complete and every PR is green, create
//! ~/OVERNIGHT-DONE and STOP. Do not re-measure completed gates;
//! do not polish documentation beyond the PR bodies' needs; do not
//! invent scope. A finished, boring night is a perfect night.
