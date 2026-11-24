# PostRust — Current Status Index

**Purpose:** the maintained, always-current index of what the engine actually
is — integrated, tested, measured, and pending. The stage records in
`docs/ENGINE-WORKSPACE-NARRATIVE.md` and `docs/` are **historical acceptance evidence, retained
as history and never rewritten**; this file is the live layer on top of them.
A fresh development or review agent should be able to determine the actual
current status, active authority, source of truth, known limitations, and next
work from THIS file alone.

- **Maintained since:** 2026-11-14 (the post-audit remediation cycle, F-13);
  reconciled for Cycle 1 on 2026-11-15.
- **Reconciled for the PostRust extraction on 2026-11-20:** the engine moved
  from the Sektor7 game repository to this repository
  (https://github.com/Willpatpost/PostRust) with full history; the workspace
  now lives at the repository ROOT (no `engine/` prefix — historical records
  below still use it). This pass also brought the §4 counts and the
  capability matrix current through Cycle 5 + perf-hardening batch 1; the
  extraction baseline is `main` @ `b19d210`, five-job CI run `44827915274`.
- **Update rule:** every change that alters capabilities, test counts,
  performance evidence, or priorities updates this file in the same change.

---

## 1. Historical stage-completion records (where the history lives)

| Record | Location |
|---|---|
| The stage-by-stage workspace narrative (Stages 0–5, the stage banner) | `docs/ENGINE-WORKSPACE-NARRATIVE.md` — historical, retained |
| The architecture decision ledger (T-01…, accepted and deferred) | `docs/DECISIONS.md` |
| Stage 4 physical acceptance (the owner's rig) | `docs/STAGE4-WINDOWS-ACCEPTANCE-RECORD-2026-10-21.md` |
| Stage 5 Batch 1–5 acceptance records | `docs/STAGE5-B1-WIRE-FORMAT.md` … `STAGE5-B5-LIFECYCLE-RECORD.md` |
| Slice 0 integration acceptance | `docs/SLICE0-INTEGRATION-ACCEPTANCE-RECORD-2026-10-26.md` |
| Production-persistence line records (PP-B1…B4) | `docs/PP-B1-SQLITE-PERSISTENCE-RECORD.md` … `PP-B4-ORCHESTRATION-VALIDATION-RECORD.md` |
| Stage 6 evidence schema, burst validation, soak runbooks | `docs/STAGE6-*.md` |
| The game-side vision and design bible | `VISION.md`, `DESIGN-BIBLE.md` in the **Sektor7 game repository** (https://github.com/Willpatpost/Sektor7 — they moved with the game at the extraction); `docs/TECH-ARCHITECTURE.md` here |

These are point-in-time acceptance records. Later work does not edit them to
appear further along; later state lives in this index.

## 2. Current integrated engine capabilities (what `main` actually carries)

Five-crate workspace at the repository root (`crates/`), all cross-platform (Linux, Windows, macOS
ARM64) with a five-job CI:

- **`engine-core`** — the deterministic simulation spine: `(due, seq)` event
  queue, cross-class causal dependencies, per-tick budget, execution ledger
  with first-class lateness, the safe `processed_through` watermark, FNV-1a
  fingerprints, canonical wire primitives + full-fidelity spine round-trip;
  plus the Cycle-1 **player movement/collision prototype module**
  (kinematic mechanics, std-only, behind a narrow `CollisionWorld` query
  contract — §8 scope); plus the perf-hardening **zero-allocation main
  path** (2026-11-20): diagnostics construction is threshold-gated
  (`Diagnostics::would_log` — a suppressed record allocates nothing),
  handler follow-ups collect into a reused scratch, partially-released
  blocked events keep their registrations on the remaining prerequisites,
  and the unmet-dependency paths are allocation-free — gated by the
  `spine_allocations` integration test (a counting global allocator
  pinning BOTH allocation events and bytes for a multi-dependency
  scenario, engine-side attribution only: cold window 18 events /
  6880 bytes on a fresh spine, warm window 9 / 3024 in steady state,
  against 87 / 12990 for the SAME scenario measured on the
  pre-optimization implementation; every fingerprint and scenario pin
  byte-identical, and the measured sustained-load execution cost
  dropped p50 1.24 -> 0.51 ms/window with identical trace digests).
- **`engine-persist`** — the persistence stack: SHA-256 content-addressed
  revision store; `CanonicalMedium` boundary with **two production mediums
  carrying distinct durability contracts** — SQLiteMedium
  `TransactionalWalFull` (WAL + `synchronous=FULL`, both verified by
  querying the actual configuration at open) and LocalFileMedium
  `FullPosixSequence` on supported POSIX / `ProcessCrashOnly` on Windows
  (see §5 and §8); the durable T1 journal (hash-chained transaction
  frames); the `CommitPipeline`
  (single-authority per-key lattice, canonical confirmation); the
  recovery machinery (strict temporal boundaries, authenticated ancestry
  witnesses, recovery-state classification, degradation matrices); the
  epoch-checked record constructors.
- **`engine-renderer`** — the Vulkan backend (ash): swapchain with recreation
  handling, present-mode preference with FIFO fallback, depth-correct scene,
  the dynamic region-scene vertex path (the F-02 caller-held scratch form),
  the retained overlay-vertex buffer (F-05), GPU timestamps, validation-layer
  routing, the debug bitmap overlay, the Cycle-1 **dynamic-geometry
  truncation counters** (F-03 Step 1: per-frame region/overlay
  omitted-vertex counts with supplied/capacity context, reported in every
  run summary — fail-visible over the former silent clamps), and the
  Cycle 6-E **instanced proxy path**: the instance-rate vertex input
  (mesh binding + packed-instance binding), the lazily-created
  per-frame-slot mapped instance buffer and per-kind mesh VBOs
  (zero per-frame allocation — the counters stay constant across
  warmed populated frames), one draw call per populated proxy kind,
  and the per-object reference submission mode kept as the
  equivalence gate's N-draws side; the Cycle 6-E2 **indirect
  submission** on the same path: one `vkCmdDrawIndirect` per
  populated kind with the instance counts read from GPU memory
  (the lazily-created per-frame-slot mapped command buffer, the
  byte-pinned pure command packing, byte-identical output to the
  direct instanced draw — the mechanism the deferred GPU-culling
  package authors counts through). The Era-1 **population v2**: the
  shipped proxy meshes are the v2 silhouettes (`populated_meshes` —
  the eleven-box humanoid, the sedan with libm-free cylinder
  wheels; the instance transform contract unchanged, origin at
  feet/ground-center).
  Cycle 6-F **directional shadow path**: the deterministic light-space
  cascade core, the per-slot shadow maps behind a depth-only pass
  (an external subpass dependency orders the writes before the main
  pass's fragment reads), the shadowed lit pipeline sampling the map
  (Era-1 **PCF soft shadows** — the 3×3 `shadow_filter` tap table
  over the validated map, a compare-time analytical bias shared
  verbatim with the CPU oracle, both sides carrying the same
  out-of-bounds-reads-clear convention), the known-answer map
  readback, and the `--corner --shadows` route — all lazily created
  (the unshadowed path's allocation pins untouched, its renders
  pixel-identical). The Era-1 **sky layer** (`--corner --sky`, the
  sticky `set_sky` setting): the fullscreen-triangle sky pass drawn
  first behind the scene with no depth interaction (`sky.wgsl`
  mirroring `sky.rs`'s equation; the palette and disc direction from
  `atmosphere_at`; the ray reconstructed PER-FRAGMENT from the
  CPU-computed inverse view-projection — the package-6 first-run
  correction: the original per-vertex form affine-interpolated
  denormalized directions and never rendered the disc) — lazily
  created, and a fully-covered frame stays byte-identical with the
  sky on and off. The Era-1 **day/night cycle** (`--corner
  --day-night <seconds>`): the triangle-wave clock
  (`sky::cycle_position`) advances the atmosphere with the demo
  clock — the lit scene's light parameters and the sky layer ride
  the same `atmosphere_at` frame each frame (the free-look path
  uses the wall clock; the scripted route the presented-frame count
  at its reference 60 Hz, deterministic). The Era-1 **post chain**
  (`--corner --post`, the sticky `set_post` setting; package 6, the
  ladder's architectural step): the HDR render-target chain — the
  scene pass into an offscreen R16G16B16A16_SFLOAT target (+ its
  depth attachment, per frame slot), the 1/4-res bloom mip chain
  (the bright-passed 2×2 bilinear-footprint box downsample, then
  the separable [1,4,6,4,1]/16 blur, `bloom.wgsl` mirroring
  `post.rs`'s chain), and the composite pass (post.wgsl's ACES
  tonemap + the bloom add-back at `post.rs::BLOOM_STRENGTH`) into
  the swapchain — textureLoad-exact through the whole chain (the
  lavapipe gate pins presented pixels against post.rs's reference
  at 0/255 measured), lazily created with per-frame-slot
  ping-pong images (the shadow-map discipline) and NO per-frame
  uniforms (the PostParams block is extent-static), composes with
  every corner flag, and the postless path stays byte-identical.
- **`engine-shell`** — the SDL3 application shell (statically-linked vendored
  SDL 3.4.18; real window + offscreen/dummy modes).
- **`engine-slice`** — the integration/demo driver: the stdin CLI, `--window`,
  `--render` (with `--regions` LOD visualization), `--sim` (headless
  deterministic), `--demo-persist` (the integrated authoritative campaign
  behind the render loop, with the soak/evidence machinery), the session
  crossing (founded-and-anchored or recovered, the Space action, the durable
  close), the benchmark harnesses
  (`--benchmark-persistence`, `--benchmark-instrumentation`,
  `--benchmark-entity-scale`, `--benchmark-render`), and the Cycle-1
  additions: **`--benchmark-sustained-load`** (the F-08/F-12 sustained-scale
  demonstration command) with **`--sustained-load-full-matrix`** (the full
  reference matrix; the raw evidence record is archived in the
  coordination ledger — §7) and **`--player` / `--player-headless`** (the
  player prototype's interactive and deterministic paths), and the Era-1
  **`--corner --free-look`** eyeball camera (the Stage-2 6-DoF machinery
  wired to the shell's input intents, clamped to the corner scene's world
  AABB — presentation only: the interactive camera never enters the spine,
  a fingerprint, or any measured claim, and the scripted route's
  measurement series print only with the mode off).

The **integrated campaign demo** (`--demo-persist`) is the vertical slice:
one authoritative, persistently-orchestrated 1000-NPC / 8-region campaign
with LOD, crash recovery, soak evidence, and rendering together.

## 3. Production mechanisms vs demo-only implementations

| Layer | Class |
|---|---|
| `engine-core` spine, `engine-persist` store/journal/pipeline/recovery, `engine-renderer` Vulkan backend, `engine-shell` | **Engine libraries** — production mechanisms (no demo coupling) |
| `engine-slice` scenarios (`stage6-v1`), the CLI modes, the benchmark harnesses, the debug overlay content | **Demo/driver layer** — deliberately not the game |
| `composite-test-fixture` (engine-persist), `crash-test-seams` (engine-slice) | **Test-only features** — compile-time isolated, never in production builds (CI re-proves the feature-disabled production path) |
| The Vulkan renderer's current shape (static scene buffers, host-visible persistently-mapped dynamic buffers, 2 frames in flight, bounded region-vertex capacity with **fail-visible truncation counters** — F-03 Step 1 delivered; the Step-2 capacity/refusal policy deferred) | **Prototype-class** rendering design — correct and validated, not the final architecture (see §8) |
| The Cycle-1 player prototype (`engine-core` player module + the `--player` demo layer) | **Prototype-class** kinematic mechanics — reusable engine surface, reversible, explicitly not a ratified production physics backend |

### November 21 audit and corrective candidate

The audit examined `219f800`. Its fixes landed directly on `main` as
`a4eb372`, spanning 34 files. The change covered journal recovery and
reclamation, scheduler and player input, drawable sizing and geometry
capacity, whole-run timing, the CLI and documentation. Four existing
ignored Vulkan smoke tests were run separately. Three gained assertions.
The old `overlay_slot_regions_are_disjoint` test covered offsets in a
shared mapped buffer. Per-slot overlay buffers retired that layout, and
the replacement test covers on-demand capacity growth.

The review candidate combines `a4eb372` with Batch 3 (`e0b52e8`).
It retains the pipeline cache, hybrid frame pacing, validation gating and
release shader-orientation check. The validation queue updates its pending
flag under the same mutex as its messages. Resize hints trigger a drawable-pixel
extent check. CPU and GPU percentiles both use whole-run `TimingSamples`.
The journal sidecar now allows a short writer handoff and reports typed
contention after two seconds. Tests exercise double-open and release, frame
end arithmetic at the integer boundary, and capture reconciliation after
an operating-system sync error.

## 4. Current automated test coverage

The main and heavy CI jobs check the number of passes, ignored tests and
result lines. The table gives the local Linux and Windows measurements
for the combined candidate.

| CI partition | Linux | Windows |
|---|---:|---:|
| Main shard (`--workspace --exclude engine-slice`) | 1188 | 1137 |
| Heavy shard (`-p engine-slice --features crash-test-seams`) | 582 | 581 |
| **Combined** | **1770** | **1718** |

The macOS job has its own partition. Main has two ignored tests and 33
result lines. Heavy has seventeen ignored tests and 62 result lines. The
table is settled at `main` @ `406aceb` (2026-11-24): it carries the Era-1
ladder's merged endpoint (PRs #13–#19; #20 was docs-only), matching the
workflow's enforced pins at that exact tree — main 1188 Linux / 1137
Windows with 2 ignored / 33 result lines, renderer release 111/2, heavy
582 Linux / 581 Windows with 17 ignored / 62 result lines — each value
confirmed by the green five-job CI runs on that tree, never a guess.
The per-package growth history that produced those endpoint counts
(the 6-E scaffold through the Era-1 ladder, each measured at its own
integrated tree): the Cycle 6-E implementation candidate's Linux
counts were MEASURED
locally with the workflow's own awk (1142 main = the 6-E scaffold's
1138 + 4 platform-uniform engine-renderer tests; heavy passed counts
unchanged, its new `render_instanced` binary contributing +1 ignored
/+1 result line); Windows = the prior 1087/569 + the same platform-
uniform deltas (inference, confirmed by the candidate's CI run — a
mismatch is corrected from actuals, never by weakening a test). The
Cycle 6-E ladder's population package (2026-11-23, riding the 6-E
branch) grew the heavy shard's passed count 570 -> 576 (the
`corner_population` module's six-test placement battery, within the
existing lib result line) and its ignored count 10 -> 11 (the second
`render_instanced` gate — the full 24-NPC + 2-vehicle table's N-vs-1 —
in the existing binary, serialized on the sdl_lock pattern); Linux
576/11/58 MEASURED with the workflow's own awk, Windows 575 = the
prior 569 + the same 6 (platform-uniform inference: the battery's
pins are margin-based comparisons and the byte pin is pure-IEEE
packing — to be confirmed by the candidate's CI run). Main is
untouched (engine-renderer untouched). The
Cycle 6-E2 indirect-draws package (2026-11-23, riding the population
branch) grew the main shard 1142 -> 1146 and the renderer release
battery 65 -> 69 (the `proxy_indirect` command core's four
platform-uniform tests: the packing determinism/wire layout, the
command-bytes FNV cross-platform pin, the empty-kind zero counts,
the slot-region math — pure integer packing, no cfg gates) and the
heavy shard's ignored count 11 -> 12 (the third `render_instanced`
gate — the indirect submission's three-way byte-equality, the
one-time command-buffer allocation pin, an oracle wall through the
indirect submission — in the same binary); Linux 1146/69 and
576/12/58 MEASURED locally with the workflow's own awk, Windows
1095/69 and 575/12/58 = the same platform-uniform deltas (to be
confirmed by the candidate's CI run). The Era-1 population-v2
package (2026-11-23): the v2 silhouettes (`populated_meshes` — the
eleven-box humanoid at 396 vertices, the sedan with cylinder wheels
at 552) become the population path's shipped meshes; the
`proxy_indirect` command pins and both packed-bytes FNV pins
re-measured for the new counts and the three stroller placements
nudged for the true 0.36 m footprint (every count UNCHANGED — the
repins are values inside existing tests, no new test anywhere).
  The Era-1 post package (2026-11-23): the main shard grew 1179 ->
  1187 and the renderer release battery 102 -> 110 (post.rs's chain
  reference - eight platform-uniform tests: the exactly-zero bright
  pass below one, the floor-division mip extent, the bilinear-
  footprint downsample box, the per-channel knee feed, the clamped
  blur taps and uniform-field preservation, the bilinear upsample's
  exact weights and border clamp, the composite equation, and the
  full-chain hand-computed 8x8 scene); the heavy shard's ignored
  count 15 -> 16 and result lines 60 -> 61 (the new
  tests/render_post.rs binary's ONE #[ignore]d gate - the
  known-answer pixels vs post.rs's chain at 0/255 MEASURED on
  lavapipe, the bloom-ring lift, the postless byte-identity, the
  one-time allocation delta, the composition smokes). The Era-1
  gates' FIRST-RUN CORRECTIONS (2026-11-23, the post package's
  session): every Era-1 lavapipe gate (soft shadows, sky,
  population-v2's re-pins) had never executed anywhere - their CI
  steps live in the parked workflow commits and the prior sessions
  measured shard counts only. First execution on the re-provisioned
  pod exposed and fixed: the sky pass's affine-interpolated
  per-vertex rays (the disc never rendered; now reconstructed
  per-fragment, 0/255 against the CPU rays), the sky gate's
  occluded sun/moon poses (the street-level origin aims the sun
  INTO the southwest building's silhouette; the poses moved above
  the masses), atmosphere_at's unnormalized night disc direction
  (0.9055 long, violating the documented unit contract - the moon
  never met the disc threshold), the sky gate's capture-buffer
  allocation pin (+1 memory, the first capture's lazy destination),
  the two render_shadows tests' parallel SDL init race (the
  sdl_lock serialization, render_instanced's pattern),
  population-v2's MISSED re-pins in the integration battery (the
  indirect oracle wall still carried the v1 rectangle 0.24 x
  [0.85, 1.45] - its frame includes the v2 pelvis/torso dead zone;
  the command-word defense-in-depth copy still carried the v1
  108/72 mesh counts), and post.wgsl's binding-1 mip aliasing the
  scene target through the build's atlas-share binding map (moved
  to binding 4, the 6-F shadow map's convention). Every Era-1
  graphical gate now executes and passes on the pod's lavapipe.
  The Era-1 wet-ground reflection package (2026-11-23, the
  ladder's last rung): the planar pass — the corner mesh's
  building masses re-rendered MIRRORED (planar.rs's
  ground_reflection_matrix composed into the view-projection,
  the winding flipped to CLOCKWISE, the ground itself never
  drawn in the mirror — it would occlude its own reflection)
  into per-frame-slot RGBA16F targets riding the post state's
  HDR render pass object (the wet mode carries the post state,
  never necessarily its chain) — and the ground re-draw:
  the ground quad re-rasterized inside whichever scene pass
  runs (EQUAL depth, no write) sampling the reflection at the
  same screen position (the planar property: the mirror fixes
  the ground plane, so the texel IS the reflected scene behind
  the pixel), Schlick-blended on the alpha stage (planar.rs's
  reflection_blend, r0 = 0.02). --corner --wet, composes with
  every corner flag; the mirrored scene is the buildings (the
  sky and the proxies are the documented follow-ups). The gates
  (tests/render_reflection.rs): the known-answer chain — the
  pixel's ray to its ground hit, the reflected ray raycast into
  the boxes, the lit equation at the hit (the mirror
  invariance: the mirrored render's shading at the mirrored
  point IS the real equation at the real point from the real
  eye), the Schlick blend over the dry pixel — MEASURED 1/255
  at grazing (60 pixels, blend factors 0.43-0.46) and overhead
  (3249 pixels, wet ~ dry at ~r0); the dry path byte-identical;
  the one-time allocation delta pinned with its decomposition;
  the composition smokes (wet+post, wet+post+sky,
  wet+shadows, wet+populated) all clean. The heavy shard's
  ignored count 16 -> 17 and result lines 61 -> 62 (the new
  binary's one #[ignore]d gate); the main shard and renderer
  release +1 each (planar.rs's mirrored-view-projection
  contract test). The package's own first-run catches (all
  fixed, the evidence in the commit): the color-write-mask
  default is ZERO (a legal, validation-silent no-op pipeline —
  the re-draw recorded, rasterized, and wrote nothing), the
  ground re-draw initially recorded OUTSIDE the HDR scene pass
  (a lavapipe segfault), and the mirrored sets' per-material
  indexing was off by one (the buildings rendered with the
  ground's asphalt texture).
Cycle 6-F candidate's Linux counts were MEASURED locally with the
workflow's own awk (1146 main = the pre-6-F 1138 + 8 platform-uniform
engine-renderer tests — the shadow core's full battery; heavy passed
counts unchanged, its new `render_shadows` binary contributing +1
ignored / +1 result line); Windows = the prior 1087/569 + the same
platform-uniform deltas (inference, to be confirmed by the candidate's
CI run — a mismatch is corrected from actuals, never by weakening a
test). The COMBINED tree (the full 6-E stack — 6-E + population +
6-E2 — merged with 6-F, 2026-11-23) carries all three additions:
main 1138 + 4 + 4 + 8 = 1154, heavy 576 passed with the three
`render_instanced` gates + the `render_shadows` gate (9 -> 13
ignored) and both new binaries (57 -> 59 result lines); Linux
MEASURED with the workflow's own awk on the merged tree, Windows
1103/575 = the platform-uniform sums (inference, to be confirmed by
the candidate's CI run). The
pre-6-E-implementation workflow pins (1138/1087 main, 570/569 heavy
with 9 ignored / 57 lines) carried the 6-A/6-B/6-E-scaffold history;
the workflow's comment chains remain the per-batch measurement record.
The Era-1 free-look package (2026-11-23) grew the heavy shard's passed
count 576 -> 582 (the `corner_demo` camera battery's six
platform-uniform tests — the route-entry matrix identity, the
world-AABB/eye-band clamp, the Stage-2-speed basis movement, the
runaway-input clamp, the shared look-mapping composition, the pitch
pin — within the existing lib result line; ignored and result lines
unchanged); Linux 582/13/59 MEASURED locally with the workflow's own
awk, Windows 581 = the prior 575 + the same 6 (platform-uniform
inference — pure f32 arithmetic and struct equality, no cfg gates; to
be confirmed by the candidate's CI run). The main-shard table row is
settled above at `main` @ `406aceb` — the scaffold-era flag (the row
carrying pre-scaffold 1154/1103 against the workflow's then-pin
1173/1122, deferred to "the scaffold's own obligation" rather than
folded into that package's diff) is discharged by this file's
2026-11-24 reconciliation; the flag's history lives in git.

**The Era-1 corrective cycle (2026-11-23/24, the overnight autonomous
run; the spec is Astra's 20-finding review,
`docs/ERA1-RENDERING-REVIEW-286d3e3.md`, merged as #20 — findings 1,
10 and 11 pre-verified by the home team):** every one of the 20
findings' corrections AND verification tests is landed across seven
open, five-job CI-green, NEVER-merged PRs (the owner's GO is the only
merge authority; the merge-order arithmetic lives in the workflow's
comment chains and the PR bodies):

- **#21 `corrective/capture-trust`** — findings 1+2: the
  device-to-host visibility barrier and producing-submission/buffer-
  generation binding on both capture paths; the third
  `render_shadows` gate
  (`shadow_readback_belongs_to_the_shadowed_frame_and_its_buffer`)
  exercising interposition and replacement.
  Heavy ignored 17→18 at its tip; no other count moves.
- **#22 `corrective/frame-preparation`** — findings 3–9: pre-acquire
  target preparation, the negotiated depth format (post fallbacks),
  the current camera before either lighting branch, the generation
  cache key, immediate handle registration, the fault-injection seam
  with its count census, and explicitly-enabled synchronization
  validation with a negative control. At its tip: main 1195 Linux /
  1144 Windows, renderer release 118/2, heavy 582/21/62.
- **#23 `corrective/allocations-geometry-gates`** — findings 10–19:
  the host-allocation counter beside the Vulkan one (warmed direct,
  indirect and shadowed frames all pinned at zero), the owned wheel
  winding with the oblique-view gate, the full-vertex FNV digest pins,
  the lit-transform oracle, the shadow-map ground/NaN hardening, the
  reachable-anchor PCF boundaries, the captured-lift bloom
  measurement, the shader guard mirrors, the virtual-eye reflection
  oracle, and `@invariant` position outputs SPIR-V-verified with wet
  coverage bands. At its tip: main 1197/2/33 Linux (1146 Windows),
  renderer release 120/2, heavy 588/17/62 Linux (587 Windows).
- **#24 `corrective/architecture-split`** — finding 20's phased
  `frame_prepare` extraction and corner-scene recorder split (phases
  A/B/C plus the coordinator flag contract at the phase-A seam): a
  pure refactor, no test count moves.
- **#25 `corrective/sky-offaxis-coverage`** — the gate-table's sky
  extension: the off-axis disc edge at 264×200 (aspect 1.32) inside
  the existing sky gate; no count moves.
- **#26 `corrective/wet-depth-coverage`** — finding 19's
  fallback-format half (D24_S8) on the #22+#23 union tree. At its
  tip: main 1208/2/33, renderer release 131/2, heavy 588/24/62.
- **#27 `corrective/post-edge-coverage`** — the cycle-closing audit's
  three unlanded check-halves: findings 16+17's GPU-side
  discriminators (the asymmetric-disc mirror discriminator, the
  1×256/2×256 tiny-extent ambient feed) and finding 4's wet D16
  cell. At its tip (the largest stacked tree): main 1208 Linux /
  1157 Windows (2/33), renderer release 131/2, heavy 588 Linux /
  587 Windows (26 ignored / 62 result lines) — MEASURED with the
  workflow's own awk, Windows confirmed by the exact-count CI gate.
- **#30 `corrective/acquire-out-of-date-coverage`** (the recursive
  phase's next item, 2026-11-24) — finding 6's out-of-date half: the
  acquire-out-of-date recreation path's first-frame verification,
  the one review-proposed check #22 had honestly documented as
  driver-forced and not exercisable from the test API. A one-shot
  diagnostic injection (the `inject_target_fault` seam precedent)
  replaces the first `vkAcquireNextImageKHR` with the injected
  `ERROR_OUT_OF_DATE_KHR` — the driver contract modeled exactly (no
  image, semaphore untouched), the arm's retry acquire structurally
  real — and the new `#[ignore]d` render_reflection gate
  (`first_frame_after_out_of_date_recreation_keeps_every_layer`)
  proves the FIRST presented frame after that recreation carries
  sky/wet/post by the pending-recreate sibling's own differential
  method, plus the one-shot consumption proof. Stacks on #27's tip;
  at its tree: main 1208/2/33 and renderer release 131/2 unchanged,
  heavy 588/27/62 (ignored 26 → 27, the one new gate) — MEASURED
  with the workflow's own awk, the reflection battery 6/6 on the
  pod's lavapipe. At the full union the heavy ignored pin is 28
  (#21's capture gate + 27 here) — MEASURED on the assembled
  eight-PR union by the extended rehearsal (the paragraph below),
  the exact-count gate enforcing it through CI on PR #29.

The full union's counts are MEASURED (the 2026-11-24 union
  rehearsal, branch `corrective/union-rehearsal` — EIGHT `--no-ff`
  merges in the ladder order over `main` @ `406aceb`: #21–#27,
  then #30's acquire-out-of-date gate as the eighth, the end state
  the owner's merge sequence produces — the full local gate battery
  with the workflow's own awk): main **1208/2/33** (Windows 1157),
  renderer release **131/2**, heavy **588/28/62** (Windows 587 —
  the seven-PR union's measured 27 plus #30's one gate, the
  arithmetic #30's body documented, MEASURED on the assembled
  tree). The eighth merge is the one the seven-PR rehearsal could
  not cover (#30 stacks on #27's tip, so its one-shot injection
  consume site — vulkan.rs's first acquire — had never met #24's
  `prepare_after_recreation` restructure of that exact region):
  its ONE conflict was the workflow's heavy-ignored pin block,
  resolved additively, and the linchpin gate re-fired on the merged
  tree with every measured line identical (the injection survived
  the split — re-verified on the pod's lavapipe AND in CI's own
  Reflection-gates step); draft PR #29 (the
  rehearsal's DO-NOT-MERGE verification PR — GitHub's
  `pull_request` CI is the only mechanism that runs the exact-count
  gates on the union tree, the push trigger being branch-filtered)
  carries it through the five-job CI — run `45392386762` green on
  the seven-PR tree, run `45406550126` green on the eight-PR tree
  (the Windows 1157/587/28 exact-count pins' first execution on
  the end state). The rehearsal also caught the union's two
  merge-time obligations — both at the #21×#23 seam, correct on
  every branch's own tree and invisible to GitHub's conflict
  detection: `compare_map`'s 4→7-tuple (four destructure sites in
  #21's capture gate) and `intersection_boxes()`'s Vec→`&'static
  [SceneBox]` slice change (the same four sites' `&boxes` becomes
  a needless borrow under clippy `-D warnings`) — plus #22's
  accidentally-committed `vulkan.rs.backup` (a stale 5083-line
  copy of `vulkan.rs`, removed on that branch in `5a4cbd2` with a
  diff-identity proof of gate inertness). The full record lives in
  PR #28's comment trail; the fixes are carried verbatim on the
  rehearsal branch for the owner's merge sequence. Every rig-dependent clause (the discrete-GPU
capture-visibility behavior, driver-forced fallback paths) is
documented as rig-pending in the PR bodies, awaiting the owner's
RTX 5070 Ti procedure — never presented as measured. The cycle's own
closing re-audit (2026-11-24, independently re-verified against
source) found the recursive queue empty: no unlanded correction, no
unlanded verification test, no NOTE/IMPROVEMENT finding uncovered.
The NEXT cycle's re-check (2026-11-24, the supervisor cycling the
agent as the protocol directs) re-audited that emptiness claim
against the source with a second independent read-only pass and
found exactly one remainder: finding 6's out-of-date half, the
check #22 had documented as not exercisable — landed as #30 above
(the cycle's own 5/3 seam precedent applied to its last documented
limitation). The queue is empty again: every remaining clause is
either landed with a CI-visible gate or documented as rig-pending
in the PR bodies.

(At `a4eb372`, Windows counts were measured before landing; Linux
counts were inferred at landing and subsequently confirmed by GitHub
run `44937616390`.)

(The extraction baseline was measured at `main` @
`b19d210` by five-job CI run `44827915274` (and identical at the Sektor7
pre-extraction mainline `2640b64`, run `44825693818`): the Cycle-3-era
values this table carried before the 2026-11-20 reconciliation (1011/960 +
498/497 at Cycle 3, then 1050/999 + 512/513) were superseded by Cycle 4
(+46 main platform-uniform across its batches), Cycle 5 (+2 main, +12 heavy
+1 ignored), and perf-hardening batch 1 (+2 main, +1 result line — the
spine allocation gate). Every step was measured at its integrated tree;
the history is preserved in the workflow's comment chains and the cycle
commit messages.)

**The Cycle-1 candidate** (was `s7-cycle1-integration`;
**merged to `main` 2026-11-15**, superseded by the Cycle-2 merge below)
added +32 main / +16 heavy platform-uniform tests and three integration
binaries, including the full corrective round (R4/R4b player boundary
and exact-accounting regressions, R2's gradual-growth regression).
Measured at the R4b-merged tree (the workflow's own awk semantics):
main **987 / 1 ignored / 30 result lines**; heavy **483 / 4 ignored / 49
result lines**. Windows expectations 936 / 482 (platform-uniform
inference) are confirmed by the candidate CI run; the workflow pins are
updated in the same change (the F2 maintenance obligation). All
determinism pins below are byte-identical at the candidate, plus the new
pinned player trace `engine-core/player-trace/1` class
(`8513d40792ea88f0`, the libm-free script class — cross-platform by
construction; divergence is an investigation, never a silent re-pin).
The formerly ignored oversize smoke is now ALSO executed by an enforced
Linux CI step (the R1 corrective) with its evidence lines in the log.

**Cycle 2** (built on the Cycle-1 candidate; **merged to `main`
2026-11-16**, superseded by the orientation merge below) adds C2-P1's
+19 main (11 player mechanics: step-up, the exact-slab segment query,
camera obstruction; 6 interaction-framework tests; the
corrective/closeout rounds' progression and interaction-validity
regressions) and +11 heavy (`playable_world`, a new integration
binary, including the corrective rounds and the connected live-frame
route) plus C2-P2's +2 heavy envelope regressions, with a second frozen
scenario trace (`8f6b490648326d26`, the libm-free gatehouse course).
Both Cycle-1 frozen traces and every determinism pin remained
byte-identical through the merge.

**The orientation corrective** (2026-11-17, PR #4 — `main` =
`15b8109e0cca4760c1128b2bc61bf59b3f4fdce5`, post-merge CI
`44014349602` success ×5) fixed the owner-observed upside-down world:
the single Vulkan Y-axis conversion (the naga shader-boundary flip)
with `perspective()` un-negated, both pipelines `front_face
CLOCKWISE` (the winding audit), and the bounded frame-capture readback
with pixel-level orientation regressions (`render_orientation`, an
enforced Linux CI step). It also made the overlay actually render for
the first time (the glyph quads had been back-facing/culled under the
old shared COUNTER_CLOCKWISE state since Stage 2 — measured pixel
evidence; no earlier test ever asserted overlay pixels).

**The post-orientation hardening candidate** (2026-11-17,
`s7-post-orientation-hardening`, awaiting review) fixes the
owner-observed inverted horizontal mouse look (mouse-X negated at the
input mappings across all four camera paths — the playable, player,
and the shared free-flight render/persist cameras; the vertical
mapping byte-identical) and hardens the capture API: an explicit
4-byte RGBA8/BGRA8 capture-format contract with typed refusals before
any GPU copy (an 8-byte fallback surface format can no longer
under-allocate the destination), checked sizing, the fallible capture
preparation moved BEFORE the frame-fence reset and image acquire (a
refused capture can never poison frame synchronization — the frame
renders normally and the refusal is observable), and the bounded
single-flight request lifecycle (a second request while one is
outstanding is refused with a typed error — never overwriting an
unconsumed readback). The final cache-validation corrective on the
same candidate adds the cache-reuse full-extent validation (a
1280×720 destination never serves a 960×960 swapchain, same pixel
count notwithstanding) and the transfer-source-capability check
independent of destination reusability (at preparation AND at the
record-time guard). Adds +4 main (the H3 capture contract units and
the two cache-validation regressions) and +3 heavy (the H2
mouse-look direction regressions across the player, playable, and
free-flight paths — asserting the resulting world-space look
direction through the real mappings and engine math). Measured at
the candidate: main **1011 / 1 / 30**; heavy **498 / 5 / 51 result
lines** (Windows 960 / 497, platform-uniform). Every determinism pin
and both frozen traces byte-identical at the candidate.

- **Determinism pins (never change):** engine `6504123a47cab66f`, state
  `937bd3b6f56f07e0`, save `len=1209 / fnv=c6c1b46c7d21843b`, the Stage 3
  and Stage 4 scenario fingerprints — all double-run and byte-compared in CI.
- **Smokes:** fingerprint/save self-tests (double-run), headless simulation,
  the three presence scenarios, the Vulkan offscreen render smokes with the
  active validation layer incl. swapchain recreation and pipeline
  replacement (Linux/lavapipe — correctness only), the bounded integrated
  soak, and the F-06 release render-path canary (distributions printed, the
  A/B geometry equality asserted, no timing thresholds).
- **Counts describe the current CI configurations** — the specific commands
  in the workflow, not every platform or feature combination. The historical
  count narratives inside `docs/ENGINE-WORKSPACE-NARRATIVE.md` describe
  earlier states and are retained as history.

## 5. Accepted architecture decisions

The ledger is `docs/DECISIONS.md`; the load-bearing accepted decisions:

- Stage-gated minimal scope with per-mechanism tests (the anti-abstraction
  ledger, TECH-ARCHITECTURE §8).
- Deterministic-first simulation (the spine owns logical time; rendering and
  pacing never enter logical state).
- Content-addressed storage with structural domain separation; the
  single-authority commit model (canonical confirmation = verified chain
  inclusion AND per-key validation); no parallel canonical writes.
- The T1 journal and the two-contract recovery (pure-function
  reconciliation; strict temporal boundaries at every begin).
- The two production canonical mediums with their **distinct durability
  contracts** (the provider-selection gate closed through the PP line):
  SQLiteMedium **`TransactionalWalFull`** (WAL + `synchronous=FULL`, both
  verified by querying the actual configuration at open — PP-B1/D-PP-1,
  subject to its documented configuration/storage assumptions);
  LocalFileMedium **`FullPosixSequence`** on supported POSIX (the full
  fsync sequence; the macOS std-semantics note applies) and
  **`ProcessCrashOnly` on Windows** — the process-crash tier, insufficient
  for the ratified destructive-reclamation durability requirement (the
  OC-P2 tier gates refuse the destructive arms on that platform).
- SDL3 vendored-static; the engine never links system SDL.
- Production-persistence decisions: `docs/PRODUCTION-PERSISTENCE-DECISIONS.md`.

## 6. Owner-ratified decisions and deferred design items

The following three behavioral and measurement decisions were
**ratified by the owner on 2026-11-22** (the decision ledger:
`docs/DECISIONS.md`, entries T-16/T-17/T-18). They are accepted
behavior, not open questions — future changes require an explicit
tradeoff analysis and owner approval.

- **Jump input (ratified).** The first executed simulation tick
  consumes a queued press; an airborne player must press again
  after landing. Future gameplay polish may propose configurable,
  bounded jump buffering (with coyote-time) under the ratified
  requirements in the decision entry — none is implemented.
- **Render overflow (ratified).** At the shared scene-vertex cap,
  foreground draws (player-visible geometry, weapon/viewmodel
  presentation) take priority and world geometry truncates first.
  The permanent geometry-capacity policy (complete primitives,
  scalable overflow handling) remains renderer-development backlog
  work — the fixed current cap is a prototype constraint, not
  endorsed architecture.
- **Timing percentiles (ratified).** Live CPU/GPU diagnostics use
  the bounded histogram (bucket-upper-bound percentiles, exact
  maxima, documented ≤ ~1.6% relative error). Engineering
  benchmarks that justify optimization or architecture claims must
  use exact measurements or rigorous A/B methodology — approximate
  histogram percentiles are never presented as exact observations.

The following audit finding remains genuinely open:

- **Scheduler budget (audit F4).** The per-tick limit counts executed
  events. Registering blocked events and waking their dependents still
  costs CPU outside that limit. **CLOSED 2026-11-22 (Cycle 6-C,
  decision T-18): MEASURED NEGLIGIBLE — no bounded budget needed.**
  The per-tick bookkeeping cost is linear at ~1 ns per blocked event
  per tick (under 0.1% of a 60 Hz frame at 10,000 blocked events —
  10x the demonstrated sustained-load scale); each event parks exactly
  once regardless of waiting duration (pinned structurally by
  `tests/spine_bookkeeping.rs`). The finding reopens only if a
  10,000+ blocked-event-per-tick workload is demonstrated.

- **F-03** — the region-render vertex-capacity policy: **Step 1 is
  DELIVERED in Cycle 1** (the fail-visible truncation counters — §2 —
  measured zero-truncation on every ordinary run, and the oversize
  lavapipe smoke proves the exact counting through the real render path);
  The current renderer gives foreground and gameplay HUD vertices priority
  within the existing caps and allocates overlay storage on demand.
  **Step 2** (constants-derived capacity + typed refusal at the
  `render_frame` boundary) remains deferred awaiting the owner's separate
  decision — the technical recommendation with alternatives and tradeoffs
  is delivered with the Cycle-1 P1 package (`GPU-CAPACITY-SYNC-ANALYSIS.md`
  §4 Step 2 + the delivery report).
- **F-04** — GPU asset streaming / upload architecture: the measured-
  hypothesis plan is **DESIGN READY** (the same document §2.2/§3 — gated on
  the rig measurements H1/H4/H5; no refactor justified by current
  evidence).
- **F-08 / F-12** — the sustained-load scale methodology is
  **IMPLEMENTED in Cycle 1** (the `--benchmark-sustained-load`
  demonstration + `--sustained-load-full-matrix` full harness: the six
  workload classes, twelve measurement dimensions, the measured
  composition table, the honest effective matrix — see §7), **corrected
  in the R2/R3 round** (per-logical-tick slope units; recovery horizons
  derived from measured drains with explicit bounded-observation wording
  for non-draining compositions); remaining follow-ups are documented
  limitations (§8: the class-6 director-protocol anchor is
  harness-driven, and the sustained codec's identity-capacity
  tightening), not missing methodology.
- **The F-05 overlay throttle** — deliberately NOT implemented (the measured
  costs are negligible at target frame budgets; revisit only with rig
  evidence).
- Compaction follow-ups and the multiplayer **implementation** —
  `docs/MULTIPLAYER-SPEC.md`'s player-facing multiplayer and governance
  rules are **ratified product requirements**; the in-session
  architecture boundaries are **designed** (unratified) in
  `docs/CYCLE6-6D-SESSION-REPLICATION-BOUNDARIES.md` (Cycle 6-D,
  2026-11-23 — sim placement, input ordering, replication scope,
  divergence detection, session lifecycle, with the owner decision
  list in its §9); what remains open beyond it is the owner's §9
  rulings, the transport/provider selection (T-06 deferral), and all
  implementation (owner-gated batches, its §10).

## 7. Actual performance evidence (and what is NOT evidence)

- **CPU render path (pod, `--benchmark-render`):** the F-02 scratch form
  p95 0.081 → 0.035 ms/frame (−57%), byte-identical geometry; the F-05
  overlay breakdown (glyph vertex rebuild 0.090–0.131 ms dominant; format
  chain and percentiles negligible). Pod CPU evidence only.
- **Entity scale (pod, `--benchmark-entity-scale`):** 1k / 5k / 20k
  entities × three LOD scenarios, bounded event execution per tick with
  double-run determinism. Bounded-cost evidence, **not** sustained-load
  proof (F-12/F-08 open). **Read-through correction (Cycle 1, measured
  2026-11-14):** at the 5k and 20k points the benchmark sustains only
  **1,000 live routine chains** — `npc::routine_payload_parts` rejects
  identities ≥ `NPC_COUNT` (1,000), so entities above the first thousand
  execute their initial event and never chain (measured: ≈154–162
  executions/tick at ALL three populations; pending stays exactly 1,000).
  The historical at-scale numbers are one-1000-chain steady states, not
  scaled-chain evidence; the sustained-load harness below realizes full
  chains at every population with its own codec.
- **Sustained load (pod, Cycle-1; corrected per the R2/R3 corrective):**
  the F-08/F-12 reference matrix — **`--sustained-load-full-matrix`** runs
  it; **`--benchmark-sustained-load`** is the bounded demonstration
  command. The raw measurement record (every cell, double-run, with
  environment parameters and reason-coded N/A lines) is archived at the
  coordination ledger as
  `cycle1/evidence/P2-sustained-load-full-matrix-R2R3.output` — this
  summary qualifies it and replaces no raw result. **Overload onset is a
  function of population × LOD composition, not population alone**
  (measured base demand per logical tick against the 200/tick budget):
  at 1k, no scenario's base saturates (full 154.1, mixed 84.1,
  event-driven 13.7); at 5k, full and mixed saturate the budget while
  event-driven (69.2) stays sustainable; at 20k all three saturate. The
  overloaded cells measure the honest "fast-because-falling-behind"
  (per-logical-tick backlog slopes — the R2 units corrective — e.g.
  +57.7/tick at 5k-full, +199.7/tick at 1k-event-driven-overlaid; the
  gradual-growth regression pins +1/tick so slow unsustainability cannot
  masquerade as sustainable). **Recovery horizons are derived from
  measured drains** (the R3 corrective: the 10× rule over observed drain
  times — 1k drains 100–150 ticks → 1000–1500-tick horizons, one
  resource-capped at 1200, stated); at 5k/20k no complete drain occurs
  within the bounded probe, so those cells are explicit bounded
  observation windows with backlog/age trends reported and **no
  complete-recovery claim**. Mixed-LOD splits lane-vs-lane (near
  overloaded / far designed cadence). Compacted-representation variants
  show the ledger downgrade. Pod CPU evidence only, logical-time
  determinism throughout.
- **Player prototype (pod, Cycle-1 `--player-headless` / offscreen):**
  collide-what-you-see physics verified end-to-end (the scripted walk
  stops against the visible cube at the exact SKIN offset); the
  libm-free trace class is cross-platform bit-stable (pinned
  `8513d40792ea88f0`, reproduced at the integrated three-package tree).
  **No human-played run has occurred** — offscreen runs are correctness
  smokes only.
- **Persistence (pod, the F-07 harness):** ~1295 sequential ops/s, ~9%
  worker-queue overhead, contention p99 128 ms, cold recovery ~34.7 ms,
  ~19× head-of-line amplification — pod measurements of the approved
  topology, not universal guarantees; the actionable interpretation is
  delivered
  (`docs/F07-PERSISTENCE-OPTIMIZATION-ANALYSIS-2026-11-14.md`, §9).
- **Real GPU (the only real-GPU evidence):** the Stage 4 Windows acceptance
  record (2026-10-21, the owner's RTX 5070 Ti rig — e.g. 1744 fps avg
  uncapped/immediate on the Stage 4 scene). **Standing rule: no CI or pod
  number is GPU evidence** (llvipe/lavapipe are correctness smoke only).
  The current-scene hardware baseline procedure is
  `docs/WINDOWS-GPU-PROFILING-KIT.md` (F-09-corrected), awaiting the owner's
  rig.

## 8. Known limitations (the honest list)

- **LocalFileMedium on Windows is `ProcessCrashOnly`** — the documented std
  Windows sequence (file fsync + rename ordering) establishes
  acknowledged-revision persistence across process crash only, NOT OS
  crash or power loss, and it is insufficient for the ratified
  destructive-reclamation durability requirement (the OC-P2 tier gates
  refuse the destructive arms on that platform). Production Windows
  deployments use the SQLite medium (`TransactionalWalFull`).
- The renderer is prototype-class: static scene buffers, host-visible
  persistently-mapped dynamic buffers, 2 frames in flight, and a **bounded
  region-vertex capacity** (F-03 — the truncation is now COUNTED and
  reported, measured zero on every ordinary run, but the capacity limit
  itself stands; the Step-2 refusal policy is deferred by design).
- Player movement uses kinematic box collision. The playable gatehouse and
  courtyard encounter use a constrained third-person camera, and the
  encounter includes rotated cover. Controller state remains transient and
  movement runs beside the spine. The interactive demos provide rebindable
  controls and adjustable mouse sensitivity, and the encounter also provides
  user-controlled HUD scaling. See [Player controls](PLAYER-INPUT.md).
- The sustained-load harness's documented Cycle-1 limitations: class 6's
  RegionDirector-protocol anchor is harness-driven (the director's own
  transition semantics stay covered by the regions/s6 suites, not the
  matrix), and the sustained codec's 16-bit identity field contradicts
  its 2^24 restore bound (latent — no delivered path triggers it; a
  follow-up tightening).
- The entity-scale benchmark's 5k/20k dead-chain read-through (§7): the
  historical at-scale evidence is one-1000-chain steady state.
- **The s6_soak late-window fsync race (flake C, 2026-11-23, macOS run
  45104773498):** `deterministic_late_window_hard_exit_control_recovers`
  assumes the tick-3300 publication's WAL fsync cannot complete within
  the five sim ticks before the controlled hard exit ("the pure-sim
  ticks between always outrun the fsync") — but a 726 ms runner stall
  on the sim path let the fsync land first (the campaign recovered at
  tick 3300, not the asserted 3054). A genuine wall-clock race in the
  test's SETUP, not an engine defect (recovery, conservation, and
  close integrity all held). The principled fix — a test-only
  fsync-timing control (a fault-seam delay in the journal path making
  the in-flight state deterministic) — is an open engine-persist
  package; until then the test may flake rarely on contended runners
  (the third 2026-11-23 runner-timing flake; see
  `docs/AUTONOMOUS-CYCLE-PROTOCOL.md` §3.1 for the rerun-once
  protocol).
- Sustained-load scale is QUALIFIED with composition-dependent
  interpretations (sustainable at 1k across scenarios; at 5k the full and
  mixed compositions genuinely overload the 200/tick budget while
  event-driven stays sustainable; at 20k all compositions overload —
  measured, per-tick units), but production-scale claims remain out; the
  reference matrix is pod CPU evidence with logical-time determinism,
  not a universal throughput guarantee, and the 5k/20k recovery cells
  are bounded observation windows, not complete drains.
- No GPU asset streaming (F-04); dynamic content moves through per-frame
  host-visible copies.
- No networking, replication, procedural generation, asset pipeline, or
  editor systems (§10 — not yet designed, not hidden). Combat, adversary,
  and encounter prototypes landed through Cycles 2–5 (§2, the capability
  matrix) — game-scale gameplay on top of them remains undesigned; the
  physics backend remains the reversible Cycle-1 prototype.
- The engine is a **development foundation + demo**, not a production game
  engine; nothing in the current evidence supports production-readiness
  claims for the complete game.

## 9. Active development priorities

The post-audit remediation cycle (2026-11-14) **is merged** — `main` @
`900b0ea` carries F-09 GPU-kit/CLI correctness, F-11, F-14, F-13, and the
Milestone-2 analyses (`docs/SUSTAINED-LOAD-METHODOLOGY.md`,
`docs/F07-PERSISTENCE-OPTIMIZATION-ANALYSIS-2026-11-14.md`,
`docs/GPU-CAPACITY-SYNC-ANALYSIS.md`,
`docs/F10-FAULT-SEAM-COVERAGE-MATRIX-2026-11-14.md`).

**Cycle 1 (2026-11-14/15, the owner's GO directive preserved verbatim with
SHA-256 at the coordination ledger, §10)** — three packages delivered,
independently cross-reviewed (all ACCEPT), consolidated on
`s7-cycle1-integration`: **P1** renderer overflow visibility (F-03 Step 1,
merged `359fbc6`), **P3** the player movement/collision prototype (merged
`7101433`), **P2** the sustained-load verification harness (F-08/F-12,
merged `0940a62`), the §8 reconciliation (`0eeba4c`), and the **ChatGPT
corrective round** (the HOLD review, hash-verified at the ledger): R1 the
enforced CI execution of the oversize regression (`47c7f8e`), R2/R3 the
per-logical-tick slope units + derived recovery horizons (merged
`2b620eb`), R4 the player-input boundary contracts (merged `ee23927`) —
each corrective independently verified by a non-author reviewer (all
ACCEPT; verdicts archived in `cycle1/reviews/`).

The **corrected Cycle-1 candidate** (full ancestry from `900b0ea`) measured
at its exact tree: fmt/clippy clean, main **984/1/30**, heavy **483/4/49**,
every determinism pin byte-identical, the P3 frozen trace reproducing, and
the cross-package combined-semantics check green (the player proxy through
the truncation counters — 0 omitted). The five-job `workflow_dispatch`
candidate CI (including the enforced R1 execution step) is dispatched at
this exact SHA; per-job conclusions are recorded in the cycle report.

**Cycle 2 (2026-11-15/16, the GO directive hash-verified at the ledger)** —
two implementation packages plus the F-03 Step-2 recommendation
(owner-gated, §6), consolidated on `s7-cycle2-integration` (built on the
Cycle-1 candidate):

- **C2-P1 — the playable-world interaction loop** (the gatehouse
  courtyard): step-up navigation over multi-box geometry, the exact
  ray–AABB `segment_hit` powering both TP-camera obstruction pull-in and
  line-of-sight interaction filtering, the minimal reusable interaction
  framework (engine-core `interaction.rs`; game policy in the demo),
  visible feedback through the existing overlay + dynamic scene, and the
  `--playable` / `--playable-headless` deterministic twin (the frozen
  scenario trace `8f6b490648326d26`; the full event log incl. the
  occluded→clear LOS moment and the TP pull-in sample). All scenario state
  **transient**; controls documented for the owner's hardware procedure
  (the C2-P3 draft in the cycle report).
- **C2-P2 — simulation scalability**: the measured **capacity envelope**
  (budget sweep 200–3200, release profile, real chains, double-run): 1k
  compositions sustainable from budget 200–400; **5k-full from 1600**,
  **5k-mixed from 800**, 5k-event-driven from 200; **20k-full NOT
  sustained within the sweep** (the honest limit at 3200), 20k-mixed from
  3200, 20k-event-driven from 800. Spine isolation **0.278–0.517
  µs/event release** (≈4× the debug profile); work rates 1,044–1,482
  events/ms — **the budget binds, not the CPU**. The envelope
  regressions pin sustainability at the measured budget (5k-mixed
  800/240) and near-lane starvation-freedom under far flood at the
  raised budget. The default-budget change is **owner-gated** (the
  measured recommendation delivered; nothing silently re-defaulted). No
  direct allocation/locality instrument was added — C4's drop rests on
  the isolation's cost-flatness; a bounded counting-allocator remains a
  future-cycle option (with the `unsafe_code=deny` interaction that
  would need surfacing). The near-lane per-event-cost lead (~2×) is a
  documented lead, not a conclusion (work/ms at sustaining budgets is
  similar across compositions: 5k-full@1600 ≈ 1,301 vs 5k-mixed@800 ≈
  1,280 events/ms).

Both packages independently cross-reviewed (ACCEPT; verdicts archived in
`cycle2/reviews/`), the ChatGPT review chain closed (**CYCLE 2
CLOSED-PASS**), and merged to `main` 2026-11-16; the orientation
corrective followed via **PR #4** (2026-11-17, `main` =
`15b8109e0cca4760c1128b2bc61bf59b3f4fdce5`, CI `44014349602` success
×5, the owner's physical right-side-up verification on the RTX 5070
Ti). The post-orientation hardening candidate (§4) is the active
review cycle. Standing owner decisions: the F-03 Step-2 policy
ratification and the simulation default-budget change.

**Cycle 3 (2026-11-17/18, the GO directive hash-verified at the ledger)**
— six parallel implementation seats + three independent reviewer seats
(the owner-authorized parallel expansion), all six packages
review-accepted and integrated on `s7-cycle3-integration` (the
candidate documented in §4's pins): **P1a** the reusable combat
mechanics (`engine-core/combat.rs`: the pure `resolve_attack` with
exact-slab occlusion + the Health/Defeated domain + the 0xC0AB codec);
**P1b** the adversary behavior (`engine-core/adversary.rs`: the
six-state cadence-driven machine over the spine, LOS/hysteresis
perception, the 0xB00B namespace proof); **P2** the event-driven SDL3
audio (`engine-shell/audio.rs`: callback-free cue streams, procedural
placeholders, zero per-frame API, observable silent degradation); **P3**
the scene separation (the Stage-2 test scene now EXPLICIT opt-in —
`DemoScenePreference::Omitted` the default; the renderer imposes no
demo content on game scenes); **P4** the macOS exclusion-test flake fix
(the deterministic complete witness; the failure independently
replicated at P = 1/185.1); **P6** the integrated playable combat
encounter (`engine-slice/combat_demo.rs` + the `combat_world` battery:
the winnable encounter + the reproducible failure scenario, the
per-tick combat transition, B's adversary through the spine, the audio
cues at the observation points, the NEW `engine-core/combat-world/1`
trace domain `d2ef688081dbc659`). All combat/adversary/encounter state
TRANSIENT (round resets, honestly labeled — no checkpoint claims);
provisional values are placeholders. The review chain: every package
independently reviewed (G/H/I), every MAJOR corrective landed and
confirmed (B's snap-branch blocking check; F's per-tick pacing + the
B-seam swap), plus the orchestrator's composition fix (combat_demo's
RenderConfig gained D's demo_scene field). The candidate CI at the
exact SHA is the closing gate; the owner's playtest commands:
`--combat` (the encounter) and `--playable` (the gatehouse, unchanged).

**The Era-1 corrective cycle (2026-11-23/24, the overnight autonomous
run).** The spec is Astra's 20-finding Era-1 rendering review
(`docs/ERA1-RENDERING-REVIEW-286d3e3.md`, merged as #20; findings 1,
10 and 11 pre-verified by the home team). The ladder's four packages
landed first — capture-trust (#21), frame-preparation (#22),
allocations-geometry-gates (#23), architecture-split (#24) — then the
recursive phase per the owner's directive: the sky off-axis extension
(#25), finding 19's fallback-format half (#26), and the closing
check-halves audit (#27). All seven PRs are open and never merged
(the owner's GO is the only merge authority; the count evidence and
merge-order notes are in §4 and the PR bodies). The cycle's closing
re-audit (2026-11-24, independently re-verified against source) found
the recursive queue empty. What awaits the owner: the merge GOs (in
the documented merge order — with the two #21×#23 merge-time fixes
and the measured union counts from the 2026-11-24 union rehearsal,
`corrective/union-rehearsal` / PR #28's comment trail), and the rig
run for the rig-pending clauses (the discrete-GPU
capture-visibility behavior and the driver-forced depth-fallback
paths — the RTX 5070 Ti procedure,
`docs/WINDOWS-GPU-PROFILING-KIT.md`).

## 10. The authoritative coordination ledger

- **Location:** `/home/coder/sektor7-coordination/` on the development pod
  (the live operational ledger since the 2026-11-14 clean-slate restart —
  the legacy `~/.qwen/sektor7-coordination/STATUS.md` location was wiped in
  that day's workspace purge and is obsolete) — **outside the Git
  repository, by charter**. It holds the owner's directives preserved
  verbatim with SHA-256 (`owner-directives/`, verified against
  `HASHES.sha256`), the cycle charters/matrices/challenges/reviews/deliveries
  (`cycle1/`), and the relay log (`sessions/registry.md`).
- **Ownership:** the **GLM orchestrator session** (`coder-1e`) is the sole
  writer, under the standing chain of command — Owner → ChatGPT (independent
  technical lead) → orchestrator → engineering seats. Seats receive
  provenanced relays (owner-directive + hash references, relay IDs
  registered in the ledger). The owner's authorizations are the preserved
  directive files, never paraphrase.
- **Durability:** pod-local operational storage (not committed, not on
  GitHub). It is deliberately independent of the repo's branches and
  worktrees, so branch/worktree cleanup never invalidates references to it.
  After a coordinator-session reset it is rebuildable from the preserved
  directive files plus the repository's own records (commit messages and
  `docs/`); this index carries the technical status, the ledger carries the
  campaign/operational state.

---

## The capability-readiness matrix

Classification per the owner's directive (2026-11-14 §10): **Implemented and
validated** / **partially implemented** / **prototype only** / **designed but
not implemented** / **not yet designed**.

| Capability | Classification | Basis |
|---|---|---|
| Deterministic simulation & scheduling | Implemented and validated (demo scale; sustained-scale measured at the covered populations — §7/§8) | `engine-core` spine; the determinism pins; the CI battery; the entity-scale benchmark; the sustained-load reference matrix |
| Persistence & recovery | Implemented and validated | `engine-persist` store/journal/pipeline; the crash/recovery matrices; real process-kill proofs; cross-platform CI; the per-platform durability tiers (§5/§8) |
| Rendering (Vulkan presentation) | Partially implemented (prototype-class design) | `engine-renderer`; lavapipe + rig validation; F-03 Step 1 delivered (fail-visible truncation counters), Step-2 policy + F-04 open |
| Shader/material architecture | Prototype only | Hand-written build-time naga shaders; no material system |
| GPU resource streaming | Not yet designed | F-04 deferred; host-visible copies only |
| Instanced/indirect draws | **Implemented (6-E + 6-E2)** | The Cycle 6-E implementation landed 2026-11-23 on the scaffold's deterministic core (the libm-free turn-fraction transforms, the byte-pinned packing, the proxy meshes, the `instanced_proxy.wgsl` shader): the instance-rate vertex input, the lazily-created zero-per-frame-allocation instance buffer (allocations constant across warmed populated frames), one draw call per populated kind (the populated corner route: **26 instances — the full 24-NPC + 2-vehicle placement table, the ladder's population package `corner_population`, pure data with a placement-validity battery and its own packed-bytes FNV pin — in 2 calls**), the N-vs-1 pixel-equivalence gate (`tests/render_instanced.rs`, byte-equal frames for BOTH the 13-instance fixture and the shipped 24+2 table, per-pixel 6-B oracle checks at 0/255 max error on lavapipe), the draw-call pins, and the `--corner --populated` demo mode — see `docs/CYCLE6-6E-BRIEF.md`. The Cycle 6-E2 package (2026-11-23, riding the population branch) added the indirect submission: `ProxyDrawMode::Indirect` — one `vkCmdDrawIndirect` per populated kind with the counts read from GPU memory, the pure byte-pinned `proxy_indirect` command core (FNV cross-platform pin, `firstVertex`/`firstInstance` pinned zero — no `drawIndirectFirstInstance` feature), a lazily-created per-frame-slot mapped command buffer (one-time +1 device_memory, pinned; the direct path's counters untouched), byte-equal frames across per-object/direct/indirect submissions (the third `render_instanced` gate, with an oracle wall through the indirect path), and the populated route's default submission now indirect (`--proxy-instanced` selects the direct 6-E shape). GPU-side count AUTHORSHIP (compute/culling writing the counts) remains deferred to the GPU-culling package. The rig measurement commands live in the 6-E/6-E2 PRs (lavapipe is correctness evidence only) |
| Directional shadows | **Implemented (6-F)** | The Cycle 6-F implementation landed 2026-11-23: the deterministic core (the light-space orthographic cascade, the compare-time slope-scaled bias model, the CPU raycaster oracle — pure IEEE, unit-pinned) + the depth-only pass + the shadowed lit pipeline sampling the map (hard shadows, one cascade, the corner scene). The gates: the map's known-answer (every texel vs the raycaster, max |Δdepth| 2.7e-7 on lavapipe) and the pixel equivalence (a shadowed ground patch renders exactly `albedo·ambient`, a lit patch the unshadowed equation, the unshadowed path pixel-identical) — `tests/render_shadows.rs`, executed by the CI's Linux Vulkan step. The `--corner --shadows` demo mode runs the full route. PCF/softening, cascaded splits, and proxy shadows are open |
| NPC/world representation (LOD) | Partially implemented | The region director, four LOD levels, activation/reconstruction — validated at 1000 NPCs / 8 regions; game-scale behaviors undesigned |
| Physics and collision | Prototype only | The Cycle-1 kinematic player prototype (`engine-core` player module: swept-AABB collision, fixed-step movement, FP/TP cameras) — headless/offscreen validated, collide-what-you-see; reversible and explicitly NOT a ratified production physics backend (§8) |
| Combat and gameplay systems | Partially implemented (demo-scale prototype, validated) | The Cycle 2–5 line: `engine-core` combat resolution (`resolve_attack`, pure, allocation-free) + the adversary cadence model + the encounter composition with cover/HUD/cues in `engine-slice` — pinned by the combat_world (21) and encounter_world (30) batteries with byte-identical scenario traces; game-scale gameplay systems (progression, economy, AI beyond the cadence prototype) remain undesigned |
| Networking and replication | In-session boundaries **designed (unratified)** | The player-facing multiplayer and governance rules are **ratified** in `docs/MULTIPLAYER-SPEC.md`; the campaign-level authority/persistence research lives in `docs/MULTIPLAYER-ARCHITECTURE-RESEARCH.md` (unapproved); the **in-session architecture** — sim placement, input ordering, replication scope, divergence detection, session lifecycle — is designed in `docs/CYCLE6-6D-SESSION-REPLICATION-BOUNDARIES.md` (**unratified**; owner decision list §9); transport/provider selection remains undesigned (T-06 deferral) |
| Listen-server authority | Designed (unratified) | The 6-D design's S3 direction — host-authoritative canonical spine with optional non-authoritative client mirror — is a PROPOSAL; no engine system |
| Host handoff | Designed (unratified) | Planned-required per the ratified multiplayer requirements; the 6-D design maps the handoff onto the research §5 lease/epoch machinery over `engine-persist`; no engine system |
| Co-op roster governance | Not yet designed | The player-facing governance rules (roster democracy, unanimity requirements) are **ratified** in `docs/MULTIPLAYER-SPEC.md`; no engine system |
| Procedural world generation | Not yet designed | — |
| Dense city streaming | Not yet designed | F-08 deferred |
| Assets and content pipeline | Not yet designed | — |
| Editor/creation tools | Not yet designed | — |
| Performance instrumentation | Implemented and validated | The benchmark flags; the run summaries; the F-06 canary; the GPU kit; the Cycle-1 sustained-load reference matrix and the renderer truncation counters |
| Production deployment | Not yet designed | The CLI is a development driver, not a product |
