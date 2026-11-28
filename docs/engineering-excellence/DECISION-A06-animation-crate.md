# DECISION A06 — The animation core's crate placement

**Status: DECISION RECORD — AWAITING OWNER RATIFICATION. This document
decides nothing by itself.** No implementation lane dispatches, no code
changes, no manifest changes follow from this file alone; every action
listed is gated on the owner's explicit GO (§5). The roadmap's framing —
A06 "needs the decision, not just code" — is exactly what this record
delivers: the evidence, the options, the recommendation, and the
pre-designed migration should the owner override.

- **Finding:** A06 (FINDINGS.md:26 — "animation core is pure but lives
  INSIDE the renderer crate … the crate-coupling issue stands; slice half
  verified: `corner_routes.rs` imports `engine_renderer::animation`
  directly").
- **Charter seed:** DIRECTIVE.md:126 — "Separate reusable animation
  representation/evaluation from GPU submission and demo assets.
  **Minimize extra boundaries/copies; choose crate/module placement based
  on actual dependencies.**"
- **Provenance:** lane `ee/rend-a06-animation-decision`, base `63b01d4`
  (the integrated program HEAD at lane start; the worktree was clean, no
  source file touched — the diff is this document only).
- **Prior dispositions this record must reconcile:** P4's §10 row
  (NO-CRATE-MOVE, deferred with trigger — P4-renderer-architecture.md:1090)
  and the Sol re-audit's outstanding-findings list, which still counts A06
  as "migration incomplete" (FINDINGS.md:70-71). The re-audit postdates the
  P4 design and did not accept "deferred" as closure — because no owner
  had ratified it. This record is that ratification vehicle.

---

## 1. The evidence

### 1.1 The dependency profile — pure of Vulkan, but NOT standalone

`crates/engine-renderer/src/animation.rs` is 2,260 lines (last line 2260),
22 tests (lines 1424-2245), the deterministic skeletal-pose core: the clip
model, the turn-fraction rotation machinery, the joint-hierarchy matrix
composition, the skinning equation, the pose packing, and the walk clip.

**The Vulkan-purity claim is TRUE and verified by direct read.** The
module contains zero references to `ash`, `vk::`, device, swapchain, or
SDL types (grep over the module: the only `ash` substring matches are the
words "hasher"/"hashes" — animation.rs:832-848, :2238). Its complete
code-level import set is exactly two `use` statements plus two
crate-qualified references:

| Import | Site | Nature |
|---|---|---|
| `use crate::proxy_instances::sin_cos_turn;` | animation.rs:60 | **internal renderer edge** — the libm-free turn trigonometry |
| `use std::sync::OnceLock;` | animation.rs:61 | std |
| `bytemuck::Pod, bytemuck::Zeroable` (derives on `SkinnedVertex`) | animation.rs:505 | **external dep** — bytemuck (engine-renderer Cargo.toml:25) |
| `crate::proxy_instances::MAX_INSTANCES` (via `MAX_POSE_BLOCKS`) | animation.rs:250 | **internal renderer edge** — the capacity mirror |
| `crate::populated_meshes::humanoid_mesh()` (inside `skinned_humanoid_mesh`) | animation.rs:517 | **internal renderer edge** — the unskinned mesh source |

**But "pure" does not mean "stands alone," and the extraction analysis
must say so.** Three internal renderer edges plus bytemuck tie the module
to its crate, and one edge runs the OTHER way: `mat3_multiply` is
`pub(crate)` (animation.rs:306) precisely so `shadows.rs`'s populated
oracle composes the same-convention rotations through this ONE
implementation (shadows.rs:326, :521, :1393, :1564-1567). The module
header's "NO Vulkan, NO new dependencies" (animation.rs:1-6) is accurate
in its original context — the module added nothing to the renderer's
manifest (bytemuck predates it: proxy_mesh.rs:18, Cargo.toml:23-25) — but
a reader must not inflate that to "zero-dependency": engine-core is the
crate with that property (measured, FOOTPRINT.md:64), not this module.

The transitive internal closure of the module is small but real:

```
animation.rs
  ├─ proxy_instances::sin_cos_turn        (proxy_instances.rs:112 — std-only, self-contained module)
  ├─ proxy_instances::MAX_INSTANCES       (proxy_instances.rs:33 — 4096, the instance capacity)
  └─ populated_meshes::humanoid_mesh      (populated_meshes.rs:174)
       ├─ proxy_instances::sin_cos_turn   (populated_meshes.rs:15 — the wheel circles)
       └─ proxy_mesh::ProxyVertex         (proxy_mesh.rs:24 — bytemuck, proxy_mesh.rs:18)
```

`sin_cos_turn` is shared machinery with a wide consumer set beyond
animation: proxy_instances' own instance packing (proxy_instances.rs:145),
populated_meshes' wheel generation (populated_meshes.rs:81), and the
slice's test oracles (`engine_renderer::proxy_instances::sin_cos_turn` at
render_instanced.rs:485, :982, :1914, :2522; render_post.rs:3529). The
repo's own discipline pins such conventions to ONE implementation —
mat3_multiply's doc says so explicitly for the rotation convention
(animation.rs:304-306); `sin_cos_turn` is the same class (the
determinism-critical trig — proxy_instances.rs:15-23). **Any extraction
must MOVE this machinery, not duplicate it.**

### 1.2 The public surface inventory

23 items are re-exported at the renderer crate root (lib.rs:67-72):
`BOX_JOINT, Clip, HUMANOID_BOXES, HUMANOID_JOINTS, Joint, JointPose,
Keyframe, MAX_POSE_BLOCKS, POSE_STRIDE, PosePackError, SkinnedVertex,
compose_joint_matrices, humanoid_skeleton, pack_pose_bytes,
pose_sequence_digest, posed_box_bounds, repack_poses, rest_pose_block,
sample_pose, skin_matrices, skin_vertex, skinned_humanoid_mesh,
walk_clip`. Two public items are module-path-only (NOT in the root
re-export): **`SkinBind` and `skin_matrices_bound`** (animation.rs:434,
:473) — the A17 additions chose the module path alone, so the surface
already has two import paths with slightly different coverage (a minor
hygiene point §3 acts on). One item is `pub(crate)` only:
`mat3_multiply` (animation.rs:306). Private: `rotation_matrix_for_axis`,
`compose_transform_matrix`, `compose_local_matrix`,
`matrix_multiply_4x4`, the FNV constants (animation.rs:822-823), the
`REST_POSE_BLOCK_CACHE` OnceLock (animation.rs:70).

Functional classes within the surface:

- **Pose evaluation (pure math):** `sample_pose`, `compose_joint_matrices`,
  `SkinBind`/`skin_matrices_bound`/`skin_matrices`, `rest_pose_block`
  (OnceLock-cached, animation.rs:715-725), `walk_clip`, `humanoid_skeleton`,
  the skeleton/clip data types.
- **GPU-format packing (the renderer's layout contract):** `POSE_STRIDE`
  = 704 B "the exact `mat4x4<f32>` column-major layout the shaders read,
  so pose blocks upload with no conversion" (animation.rs:72-78);
  `pack_pose_bytes`, `repack_poses` (typed capacity refusal,
  animation.rs:690-710), `MAX_POSE_BLOCKS` = `MAX_INSTANCES` = 4096
  (animation.rs:250), `PosePackError`.
- **Mesh/vertex surface:** `SkinnedVertex` (bytemuck Pod, 64 B,
  animation.rs:504-512), `skinned_humanoid_mesh` (derives from the
  renderer's unskinned mesh, animation.rs:516-535), `HUMANOID_BOXES` /
  `BOX_JOINT` (constants "copied EXACTLY from humanoid_mesh's box_faces
  calls", animation.rs:171-174).
- **CPU-reference/oracle functions (no production consumer outside
  tests):** `skin_vertex` ("the CPU reference the WGSL will mirror",
  animation.rs:38-41, :540 — production use is zero; the only external
  caller is render_instanced.rs:2478), `posed_box_bounds`
  (animation.rs:618 — used only by the module's own tests, :2019-2055),
  `pose_sequence_digest` (animation.rs:827 — the cross-platform FNV pin).

### 1.3 The consumer map (who calls what, how often)

**engine-renderer internal — 7 files, ~21 sites, the upload/oracle side:**

| File | Sites | Symbols | Frequency |
|---|---|---|---|
| renderer.rs | :71-72 | `HUMANOID_JOINTS`, `MAX_POSE_BLOCKS` (doc) — the `ProxyPopulation.npc_poses` field type | per submission (type level) |
| submission.rs | :354 | `repack_poses` → `RendererError::PosePack` | **every submitted frame with a population** (A18's finding; FINDINGS.md:36) |
| vulkan.rs | :232 | `PosePackError` (the error enum arm) | error path |
| vulkan/passes/skinned_draw.rs | :17, :81, :101, :112, :390 | `repack_poses`, `MAX_POSE_BLOCKS`, `POSE_STRIDE`, `SkinnedVertex` | layout constants + the pose carve (SLOT_STRIDE) |
| vulkan/passes/proxy_draw.rs | :116, :389, :420 | `POSE_STRIDE`, `skinned_humanoid_mesh`, `SkinnedVertex` | mesh upload at attach; stride doc |
| shadows.rs | :326, :521, :1393, :1564-1567 | `HUMANOID_BOXES`, `HUMANOID_JOINTS`, `POSE_STRIDE`, **`mat3_multiply` (pub(crate))**, `BOX_JOINT`, `rest_pose_block`, `skin_matrices`, `sample_pose`, `walk_clip`, `compose_joint_matrices`, `humanoid_skeleton` | the shadow oracle's pose machinery |
| lib.rs | :43, :67-72 | `pub mod animation` + the root re-export block | surface |

**engine-slice — 2 production files + 4 test files, 27 sites, the
evaluation-policy side (the A06 crate-coupling itself):**

| File | Sites | Symbols | Frequency |
|---|---|---|---|
| corner_population.rs | :242-244, :283, :566, :608 | `SkinBind, compose_joint_matrices, humanoid_skeleton, rest_pose_block, sample_pose, skin_matrices_bound, walk_clip`, `HUMANOID_JOINTS` (the `Block` type) | `evaluate_population_poses` (:241-279): **per presented frame** — `SkinBind::from_skeleton` once per batch (:250), then per walking NPC `sample_pose` + `compose_joint_matrices` + `skin_matrices_bound` (:272-274); standers hit the OnceLock `rest_pose_block` (:269). :566/:608 are test-side FNV pins. |
| corner_routes.rs | :618-620, :1319, :1336, :1362 | same set + `skin_matrices`, `pack_pose_bytes` | `evaluate_population_poses_locomotion` (:617-645): the same per-frame shape driven by core's locomotion state. :1319-1357, :1362 are the equivalence/FNV tests. |
| tests/render_instanced.rs | 14 sites (:203-:3487) | `rest_pose_block`, `pack_pose_bytes`, `HUMANOID_BOXES`, `BOX_JOINT`, `skin_vertex`, `SkinnedVertex`, `HUMANOID_JOINTS` | the pose batteries + oracles |
| tests/render_shadows.rs | :992 | `rest_pose_block` | battery |
| tests/render_reflection.rs | :915 | `rest_pose_block` | battery |
| tests/render_post.rs | :618, :3032, :3702 | `rest_pose_block` | battery |

The garage route reaches the same machinery through
`corner_population::evaluate_population_poses` (garage_scene.rs:391-411;
garage_demo.rs:11-12, :105 — "pose blocks re-evaluated per presented
frame"), not its own import.

**engine-core — ZERO code dependencies; two convention mentions only:**
`npc_locomotion.rs:309-310` documents `NpcLocomotion.facing` as "u16 turn
units (the sin_cos_turn scale)" — core speaks the turn-fraction SCALE,
not the function; `audio_mix.rs:47-48` cites `animation::walk_clip` as
the offline-sampling PRECEDENT in a doc comment. engine-shell and
engine-persist: zero.

**Measured scale context (why the frequency column is not a hot-spot
argument either way):** pose blocks in use are 24 (corner) / 6 (garage)
of 4096 carved (P5-resource-residency.md:102); the CPU render path p95
is 0.035 ms/frame (P5:110). The pose evaluation is deterministic-first,
cheap-at-current-scale code.

### 1.4 The placement context — the data flow already spans three crates

The walk cycle's owner chain today:

1. **engine-core** `npc_locomotion` — the sim side: `NpcLocomotion`
   carries `stride_phase`/`facing` (u16 turn scale), `advance_batch`
   advances it (npc_locomotion.rs:306-329; corner_demo.rs:300,
   corner_behavior.rs:413).
2. **engine-slice** — the policy: `evaluate_population_poses[_locomotion]`
   maps locomotion state → pose blocks by importing
   `engine_renderer::animation` (corner_population.rs:242,
   corner_routes.rs:618).
3. **engine-renderer** — the consumer: the blocks flow back in through
   `ProxyPopulation.npc_poses` (renderer.rs:71-72), are repacked
   (submission.rs:354) and uploaded to the skinned-draw carve
   (skinned_draw.rs).

So the animation core sits in the MIDDLE of a core→slice→renderer data
flow, owned by the renderer, reached through by the slice. The phase is
a simulation concern; the pose FORMAT is a GPU layout concern
(animation.rs:72-78); the evaluation is application policy. This is the
structural fact both the finding and the options must respect.

### 1.5 The measured footprints (B15, FOOTPRINT.md)

| Crate | Prod unique / external | Minimal-consumer cold build / closure |
|---|---|---|
| engine-core | 1 / **0** (all edges) | 2.9 s / 2 crates |
| engine-renderer | 16/15 normal; **53/52** +build; 57/56 +dev | **41.9 s / 54 crates** |
| engine-slice | 36/35; 70/69 (subsumes all four) | 52.6 s / 71 crates |

- **F-03 (FOOTPRINT.md:395-402):** the renderer has NO types-only
  surface — a consumer of even plain `FrameCamera`/`SceneBounds` compiles
  the full SDL-static + ash + naga stack. The animation module is the
  one unambiguously pure island inside that closure; extracting it would
  give a pose-evaluation consumer ~the bytemuck chain only (≈6-7 crates,
  near-core class) instead of 54.
- **But the closure arithmetic cuts the other way for every consumer
  that EXISTS:** engine-slice already subsumes the renderer (70 unique —
  FOOTPRINT.md:87, Appendix A:463-484), so moving animation changes NO
  existing consumer's closure by a single crate. The benefit accrues
  only to a consumer that does not exist today (§3, the trigger).
- The renderer's dependency direction today: renderer → engine-core
  (Cargo.toml:19). Slice → renderer is load-bearing regardless of A06
  (`FrameSubmission`, `ProxyPopulation`, `ProxyInstance` — the slice
  builds renderer inputs by design, FINDINGS A15's wire discipline).

### 1.6 What the coming lanes touch (the P5 overlap)

The P5 design reads `animation.rs` as a "full production body" file
(P5:42-43) and its R1/R2 lanes edit exactly the surface an extraction
would move:

- **R1 (A11/A23 arenas, P5:1022):** `MAX_POSE_BLOCKS`/`MAX_INSTANCES`
  const mirror (animation.rs:249-250, cited P5:118, :146-147) →
  configured `pose_capacity` budget fields (P5:191, :252) — R1
  DISSOLVES the capacity coupling point itself.
- **R2 (A18 dirty generation + pose dedup, P5:1023):** `repack_poses`'s
  per-frame call (animation.rs:690-710; submission.rs:354-355, cited
  P5:331, :382) → the generation protocol; the stander→rest-block dedup
  composes the A17 cache (P5:409-410, :995).
- The landed A17 items live here too: the `REST_POSE_BLOCK_CACHE`
  OnceLock (animation.rs:63-70, :712-725) and
  `SkinBind`/`skin_matrices_bound` (animation.rs:434-483) — P5:393, :995.

Both lanes are ORCHESTRATOR-class (frame-path semantics, the standing
rule — P5:1022-1023; FINDINGS.md:265-268 records why such work is not
delegated). **Extracting the crate before or during R1/R2 stacks pure
move-churn on equivalence-sensitive edits of the same lines.**

---

## 2. The options

### Option 1 — Extract a new `engine-animation` crate

**The shape.** New workspace member owning: animation.rs (2,260 lines,
22 tests), `sin_cos_turn` (moved from proxy_instances — it cannot stay:
the renderer will depend on engine-animation for `SkinnedVertex`, so
animation cannot depend back on the renderer; a cycle is illegal and a
duplicate of the determinism-critical trig violates the repo's
"convention exists exactly once" discipline, animation.rs:304-306).
proxy_instances/populated_meshes then import
`engine_animation::sin_cos_turn` (proxy_instances re-exports it to hold
its slice-test consumers, render_instanced.rs:485 etc.). Two coupled
decisions ride along: (a) `MAX_POSE_BLOCKS` — parameterize
`repack_poses`'s capacity (P5 R1's budget shape does this anyway) or
move the const and let the renderer pass it; (b)
`skinned_humanoid_mesh` — move to populated_meshes (mesh derivation
consuming `engine_animation::SkinnedVertex`) or drag the mesh library
too. `mat3_multiply` becomes pub (API growth) so shadows.rs keeps its
single-implementation oracle. Renderer gains `engine-animation` as a
normal dep (its closure unchanged — bytemuck already present). Slice
adds a direct `engine-animation` dep and rewires 27 sites.

**Benefits.** The layering matches the data flow (§1.4): core →
animation → renderer, with slice importing the middle directly. The
F-03 benefit materializes for any pose-only consumer (≈6-7 crates vs
54; a new minimal-consumer row for FOOTPRINT). The 9/10 bar's "explicit
separation of … animation, rendering" (DIRECTIVE.md:253) is satisfied
at crate level. The modularity score's A06 item closes unambiguously
(the re-audit counts it "migration incomplete" today, FINDINGS.md:70).

**Costs.** A pure-move migration touching ~50 sites across 9+ files and
2 manifests on a tree with P5 R1/R2 in flight over the same lines
(§1.6) — or a wait for the P6 window. The `sin_cos_turn` move touches
the instance-packing determinism path (byte-identity must be re-proven
via the instance FNV pin and known-answer tests, proxy_instances.rs:302-
330). The extraction is only honest AFTER R1 lands (R1 dissolves the
capacity mirror; extracting first means moving a coupling R1 is about
to delete). And the charter's own caution — "Minimize extra
boundaries/copies" (DIRECTIVE.md:126), "avoid speculative abstraction"
(P4:1090) — applies to a workspace member created for exactly one
consumer.

### Option 2 — Keep in engine-renderer; document the placement; pin the import discipline; register the trigger

**The shape.** Ratify P4's §10 disposition (P4:1090) as the OWNER
decision it never formally had: (a) `animation` is documented as the
renderer's pure pose-evaluation surface (its module header already
states the purity contract, animation.rs:1-14; this record adds the
dependency inventory §1.1 so "pure" is not misread as "standalone");
(b) the canonical import path for evaluation consumers is the module
path `engine_renderer::animation::` — which the slice already uses at
every site (§1.3; no site imports via the crate root); (c) the revisit
trigger is registered with sharpened arms (§3); (d) the extraction is
PRE-DESIGNED (§4) so the trigger response is dispatch, not design.

**Benefits.** Zero churn on the P5 in-flight lanes. No new workspace
member for one consumer. The slice→renderer edge the animation import
rides is load-bearing anyway: the pose evaluation's OUTPUT is the
renderer's own contract type (`ProxyPopulation.npc_poses`,
renderer.rs:71-72; the `Block` type is defined against
`animation::HUMANOID_JOINTS`, corner_population.rs:283) — slice builds
renderer inputs by the program's own wire discipline (FINDINGS A15).
The purity contract, the once-only conventions, and the byte pins stay
under the batteries that already prove them (the 22 module tests + the
five green rig batteries + the FNV pins).

**Costs (stated honestly).** The F-03 adoption friction persists: any
hypothetical pose-only consumer still compiles SDL+ash+naga (41.9 s).
The slice's evaluation policy — walk cadence, phase-to-pose mapping —
compiles against the rendering crate: a conceptual smudge between
"policy" and "mechanism home," mitigated but not removed by the module
path. The modularity score's A06 item closes by DECISION + trigger
rather than by graph change — acceptable only because the roadmap
asked for the decision; if the owner weights the 9/10 bar's crate-level
separation over the churn cost, Option 1 in the P6 window is the
override.

### Option 3 — Move the module into engine-core

**Refuted by the measured facts, three independent ways:**

1. **The zero-dep closure breaks.** `SkinnedVertex` carries bytemuck
   derives (animation.rs:505); engine-core is measured zero-dependency
   at every edge set (FOOTPRINT.md:64) — the architecture's lightweight
   anchor. The repo's own precedent when core needed SHA-256 was to
   hand-roll it rather than add a dep (F-08, FOOTPRINT.md:418-426);
   adding bytemuck to core for a presentation-adjacent type inverts
   that discipline. Dropping the derives instead would push Pod-casting
   to the renderer boundary — a duplicated layout contract for the 64-B
   vertex (the exact class the layout pins exist to prevent).
2. **The layering inverts.** `POSE_STRIDE` is "the exact mat4x4<f32>
   column-major layout the shaders read" (animation.rs:72-78);
   `HUMANOID_BOXES` is "copied EXACTLY from humanoid_mesh's box_faces
   calls" (animation.rs:171-174). Core's charter is the
   backend-agnostic simulation kernel (engine-core Cargo.toml:5) —
   having it own the renderer's GPU upload format and the renderer mesh
   library's constants couples the simulation authority to a shader
   layout. The 9/10 bar separates "simulation authority … animation,
   rendering" as DISTINCT concerns (DIRECTIVE.md:253); folding animation
   into core merges two of them.
3. **The dependency direction is already wrong for the mesh edge.**
   `skinned_humanoid_mesh` needs the renderer's `populated_meshes`
   (animation.rs:517) → `proxy_mesh::ProxyVertex`; core cannot depend on
   the renderer (that would cycle renderer→core→renderer).

The one honest partial point FOR core: `sin_cos_turn` alone is pure
std-only math, and core already documents the turn SCALE
(npc_locomotion.rs:309) — IF extraction ever happens, core is an
arguable alternative home for the trig helper. That is a sub-decision
of Option 1's dispatch, not a reason to move the module (§4 notes it).

---

## 3. THE RECOMMENDATION — Option 2, with the trigger registered and the extraction pre-designed

**Ratify the P4 disposition: NO CRATE MOVE NOW.** `animation` remains
the renderer's pure pose-evaluation module; the module path is the
canonical import surface; the extraction is dispatched only when a
trigger arm fires or the owner overrides — in the post-P5 window, along
the §4 plan.

**The reasoning, grounded in the measured facts:**

The module is Vulkan-pure but not standalone — three internal renderer
edges (`sin_cos_turn`, `MAX_INSTANCES`, `humanoid_mesh`), one external
dep (bytemuck), and one `pub(crate)` reverse edge from the shadow
oracle (§1.1) — so extraction is a real migration of shared machinery,
not a file move. Its only non-renderer consumer is the slice's
pose-evaluation policy, whose output IS the renderer's submission-
contract type (§1.4): the import rides a slice→renderer edge that must
exist regardless of where animation lives, so the crate split removes
no edge and adds a node. The B15 closures show the split changes zero
existing consumers' closures — slice already subsumes the renderer
(§1.5) — while the benefit, a ~6-crate pose-only consumer against
today's 54, accrues only to a consumer that does not exist; the
charter's A06 text itself commands placement "based on actual
dependencies" and "minimize extra boundaries" (DIRECTIVE.md:126), and
the actual dependency today is one consumer building renderer inputs.
Meanwhile the P5 R1/R2 lanes — ORCHESTRATOR-class, equivalence-sensitive
— are about to reshape the exact capacity/refusal/repack surface the
move would touch (§1.6), and R1 dissolves the `MAX_POSE_BLOCKS` mirror
that would otherwise have to be re-homed; sequencing any extraction
after them is strictly cheaper and safer. The modularity lens that
keeps A06 on the outstanding list is answered not by churn but by this
decision: a ratified placement, a pinned import discipline, a
registered trigger with a pre-designed migration, and the 9/10 bar's
separation satisfied at the level the measured dependencies actually
support today — an acyclic graph, a documented pure boundary, minimal
surfaces — with crate-level separation as the recorded next step the
moment the second consumer is real rather than hypothetical.

**The trigger (register in FINDINGS when ratified) — any arm fires the
§4 dispatch in the next non-P5 window:**

- **T1 (P4's original, sharpened):** a second NON-RENDERING consumer of
  pose evaluation appears — e.g. the P2 world-representation work
  consuming posed bounds (`posed_box_bounds` is exactly the
  sim-side shape, animation.rs:618) or a headless/battery consumer of
  `sample_pose`/`SkinBind` outside engine-slice's renderer-feeding
  policy (P4:1116-1117 anticipated this arm).
- **T2:** the owner decides the F-03 types-only-surface question in
  favor of a renderer surface split (FOOTPRINT.md:448-453) — animation
  is the first and cheapest slice of that split, and composes with it.
- **T3:** P5 R1+R2 land, opening the P6 module-coherence window where
  A01's follow-through and A15's dispatch extraction already live
  (DIRECTIVE.md:213) — the natural moment to re-ask this question at
  near-zero marginal churn cost, with the capacity mirror already
  dissolved.

**Concomitant hygiene (P6-grade, additive-safe, no lane needed now):**
record that `SkinBind`/`skin_matrices_bound` are module-path-only while
the other 23 items have the root re-export (lib.rs:67-72) — when P6
reviews the root re-export block, either add the two or prune the block
to the module path; ONE canonical path is the discipline (§5 point 3).

---

## 4. The contingent migration plan (dispatched ONLY on owner GO or a fired trigger)

Staged per the S1-S6 discipline (additive first, move, delete — the A03
precedent). Sequencing bound: **not before P5 R1/R2 land** (§1.6).

| Stage | Action | Gate (all must pass) |
|---|---|---|
| **E-S1 additive** | Create `crates/engine-animation` (workspace member; deps: bytemuck only). MOVE animation.rs's body there (git-mv semantics); `sin_cos_turn` moves with it (its known-answer tests too, proxy_instances.rs:302-330); proxy_instances/populated_meshes import `engine_animation::sin_cos_turn`, proxy_instances RE-EXPORTS it (holds render_instanced.rs:485/render_post.rs:3529). `skinned_humanoid_mesh` moves to populated_meshes (mesh derivation; `SkinnedVertex` stays in engine-animation). `mat3_multiply` becomes pub in engine-animation. engine-renderer/src/animation.rs becomes a shim: `pub use engine_animation::*;` (+ the `pub(crate)` re-export shadows.rs needs). Renderer Cargo.toml gains engine-animation. **Zero consumer edits in this stage** — all 27 slice sites and all `crate::animation` refs ride the shim. | `cargo check --workspace --all-targets`; `cargo fmt --all -- --check`; the full existing suite green with NO test edits (the shim proves path-stability); renderer batteries compile unmodified |
| **E-S2 move consumers** | Slice's 2 production + 4 test files: `engine_renderer::animation::` → `engine_animation::` (27 sites, mechanical); slice Cargo.toml adds engine-animation. | slice batteries green; the pose FNV pins unchanged (corner_routes.rs pose_bytes pins; render_instanced pose batteries); allocation-constancy counters unchanged |
| **E-S3 delete the shim** | Delete engine-renderer/src/animation.rs. lib.rs:67-72 root re-export: EITHER re-point at `engine_animation` (root path stays stable — recommended, API-stability) OR delete it (a public-API change requiring the P6 API policy's sign-off). | workspace green; every lavapipe battery digit-for-digit; the five rig batteries digit-for-digit on the rig |
| **E-S4 re-measure** | Extend the B15 battery: engine-animation minimal consumer (expected ≈ bytemuck chain, 6-7 crates, near-core class); FOOTPRINT.md gains the row; the F-03 lever's animation slice is realized and measured. | the consumer runs (not just compiles); closure recorded with `--edges` explicit (F-07) |

**Expected count movements:** renderer's pin 191 → 169-class (the 22
module tests move with the module; A12 recorded renderer 191,
FINDINGS.md:32); engine-animation +22; slice test counts unchanged (its
pose tests stay put, re-imported). **The equivalence bars that prove the
pure move:** the pose digest pin `2_694_074_108_253_436_588`
(animation.rs:2253) reproduces bit-for-bit; the instance FNV pin
(proxy_instances.rs:610, per P5:271) reproduces after the `sin_cos_turn`
move; the rest-identity bit-exactness pin (animation.rs:1866-1886); the
shadow oracle's composed-rotation results (shadows.rs:1564-1567);
render_instanced/render_shadows/render_post/render_reflection
digit-for-digit. **Risk bounds:** the only semantic hazards are (a) the
`sin_cos_turn` re-home (bounded by the known-answer + instance-pin
batteries) and (b) the `skinned_humanoid_mesh` relocation (bounded by
the 396-vertex bit-identity test, animation.rs:1987-2011); both are
named discriminating tests a lane brief must carry (the
verify-everything discipline, FINDINGS.md:244-257). Sub-decision for the
dispatching brief: `sin_cos_turn`'s home is engine-animation (default)
vs engine-core (§2 Option 3's partial point) — default stays
engine-animation to keep the move single-crate.

---

## 5. Owner-ratification marker

**THIS DOCUMENT DECIDES NOTHING BY ITSELF.** The recommendation (§3) and
the contingent plan (§4) take effect only on the owner's explicit GO.
No implementation lane dispatches from this record alone; the
constraints of this lane were docs-only (no code, no Cargo.toml —
verified: the worktree diff is this file only).

**The decision points put to the owner:**

1. **THE DECISION:** Ratify Option 2 — animation stays in engine-renderer
   as the documented pure pose-evaluation surface, module-path import
   discipline, trigger T1/T2/T3 registered — OR override to Option 1
   (extract per §4) with a named window (the plan's bound: post-P5 R1/R2).
2. **If ratified:** authorize the register update — FINDINGS A06 row
   moves to a DECIDED/VERIFIED-CLOSED-class state carrying this
   document's reference and the trigger text (the re-audit's
   "migration incomplete" item is then discharged by decision record,
   the roadmap's own remedy).
3. **The root-re-export question** (lib.rs:67-72 vs the module path,
   with `SkinBind`/`skin_matrices_bound` module-only): fold into P6's
   API-stability pass — one canonical path; additive-safe either way.
4. **If a trigger fires or the override is chosen:** confirm the §4
   dispatch shape (E-S1..E-S4, the sequencing bound, the discriminating
   tests) — the lane brief should be written against §4 as its spec.
