# P5 — GPU, Animation, Memory, and Environmental Scalability: Design

**Author:** design author, package P5 (glm-5.3-w4a8 subagent)
**Status:** COMPLETE — the design the P5 implementation lanes execute from.
**Read base:** branch `ee/design-p5-resource-residency`, HEAD `9e17516`
("integrate the a04 lane…", the post-S6/A04 tree). All line numbers below
cite this tree. `crates/**` read-only throughout — no builds, no cargo, no
test runs; this file is the only write.
**Scope (charter §7 P5):** A11, A17–A20, A23, B14 — resource residency and
explicit budgets, complete-primitive capacity policy, persistent
instance/mesh/material/pose storage, dirty-range/batched uploads, reusable
staging/retirement, ripple localization, and the reflection/post target
measurement package. NEW-REND-03 (no staging pool) folds into A20,
NEW-REND-08 (static-population caching) into A18, NEW-REND-09 into A19,
NEW-REND-10 (pose-capacity mirroring) into A11/A23 (FINDINGS.md:198–205).
**Constraints honored:** allowed edit paths `docs/engineering-excellence/**`
only; no production code, no Cargo.toml; commit on the lane branch, no
branch/merge/push commands.

> The charter's hardest bars, quoted for every section that follows:
> "Select device memory strategy based on hardware … do not assume
> host-visible is always slower or device-local always wins." /
> "Use indirect GPU authorship/visibility, GPU skin/particle or CPU
> alternatives only when evidence warrants" / "Preserve conservative
> bounds, no false-negative culling, frame-slot ownership, multiple passes,
> visibility stability and graceful capacity saturation." /
> "show costs including small/all-dirty workloads, CPU cycles and GPU
> bandwidth" / "This is groundwork for high fidelity and world streaming,
> not a mandate to build every future graphics feature."
> (DIRECTIVE.md §7 P5 paragraph, DIRECTIVE.md:207–209.)

---

## 0. Source-reading log, coverage honesty, and the measured reality

READ in full or in cited ranges (all line numbers at HEAD `9e17516`):

- `DIRECTIVE.md` §7-P5/§8/§9.3/§9.5/§9.6; `FINDINGS.md` (rows A11, A17,
  A18, A19, A20, A23, B14; NEW-REND-03/08/09/10; the Sol re-audit's
  standing orders; the auditor calibration record).
- Renderer: `proxy_instances.rs` (full — the capacity constant, the
  packing, the repack), `animation.rs` (full production body — POSE_STRIDE,
  MAX_POSE_BLOCKS, repack_poses, the landed A17 cache/bind), `scene.rs`
  (full — the region capacity closed form), `config.rs` (full —
  FRAMES_IN_FLIGHT, DynamicClamp/F-03), `renderer.rs` (ProxyPopulation,
  ProxyDrawMode, GeometryStats, AllocationStats, RenderStats, the retained
  buffers, pack_ripple_field, RIPPLE gains), `submission.rs` (the
  submission contract + adapt_frame's per-frame repack sequence),
  `frame_plan.rs` (the PassUses/FramePlan surface), `vulkan/frame.rs` (the
  phase ladder + upload_frame_data), `vulkan/resources.rs` (full — the
  staging path), `vulkan/uploads.rs` (full — upload_layers),
  `vulkan/pipelines.rs` (the region carve), `textures.rs` (full — the
  texture staging + permanent tails), `vulkan/passes/skinned_draw.rs`
  (SkinnedRetained, SLOT_STRIDE, write_slot, pose_offset),
  `vulkan/passes/proxy_draw.rs` (ProxyResources, the instance carve,
  indirect write_slot), `vulkan/passes/reflection_draw.rs` (full — the
  ripple uniform, the per-slot full-extent targets),
  `vulkan/passes/post_draw.rs` (the target half — HDR + bloom images),
  `shadows.rs` (SHADOW_MAP_EXTENT), `shaders/ground_reflect.wgsl` (full —
  the ring loop), `shaders/core/pose_storage.wgsl`, `build.rs` (the layout
  pins), `scene_description.rs` (PASS_FAMILY_MATERIAL_COUNT),
  `overlay.rs` (overlay_capacity_vertices), `proc_texture.rs` (texture
  scale).
- Slice: `corner_population.rs` (the 24-NPC table, evaluate poses),
  `corner_routes.rs` (corner_instances_into,
  evaluate_population_poses_locomotion), `corner_demo.rs` (the render-then-
  step crowd, population()), `garage_demo.rs` (the static placement +
  per-frame poses), `garage_scene.rs` (the scene + population scale),
  `corner_weather.rs` (the rain layer's instance pool).
- Core: `engine-core/src/ripple.rs` (RIPPLE_POOL_BOUND, the measured
  steady state).
- Evidence: `EVIDENCE.md` E-7 (the allocation tuple + ink pins), E-8 (the
  baseline battery), E-9 (the owner's maximal-composition walk);
  `docs/CURRENT-STATUS.md` §7 (the CPU render-path medians).
- The precedent designs: `P2-simulation-spatial.md`, `P4-renderer-
  architecture.md` (format, lane/classification discipline, gate
  vocabulary — this document follows both).

NOT examined (and why it does not block this design): the interiors of
`shadow_draw.rs`'s target builder beyond its extent constant (its census
row is named for the measurement plan, its aliasing candidacy is derived
from the pass order already documented in frame.rs's cascade);
`sky_draw.rs`, `capture.rs`, `swapchain.rs`, `context.rs` interiors (no P5
finding touches them); `populated_meshes.rs`/`proxy_mesh.rs` mesh interiors
(mesh sizes consumed via the cited vertex counts); engine-persist/engine-
shell beyond the register's recorded dispositions (A23's shell half is
MEASURED no-change per FINDINGS.md:41; NEW-SHELL-3 is the owner's design
review, §11). §12 records the consequences.

### 0.1 The measured reality this design must respect (the workload census)

Every design decision below is grounded in these measured/derived counts
(the B06/L5/L6 discipline — real counts before mechanisms):

| Quantity | Value | Source |
|---|---|---|
| Corner crowd (npc) | **24** instances, fixed table | corner_population.rs:2–4, :69–74 |
| Corner vehicles | **2** | corner_population.rs:2–4 |
| Garage population | **6 mechanics + 1 van**, placement static | garage_scene.rs:25–34, :238–277 |
| Rain kind pool | Vec capacity **1024** streaks | corner_weather.rs:301 |
| Maximal-composition route (owner-walked) | **26 instances in 2 indirect draw calls**, 2,537 frames, validation 0/0 | EVIDENCE.md:193–206 (E-9) |
| Pose blocks in use | 24 (corner) / 6 (garage) of **4096** carved | corner_population.rs:2–4; skinned_draw.rs:87–89 |
| Sim-side world | NPC_COUNT **1000**, 8 regions (the region carve's derivation) | engine-slice npc.rs:32; scene.rs:313–318 |
| Mapped dynamic carves (total) | instance 786,432 B + pose 5,767,168 B + region 1,766,400 B = **8,320,000 B** | proxy_draw.rs:407,:439–446; skinned_draw.rs:89,:193–199; pipelines.rs:242–247 × SceneVertex 24 B (scene.rs:11–20), FRAMES_IN_FLIGHT 2 (config.rs:178–180) |
| Reflection targets | per slot: full-extent RGBA16F color + depth; **2 slots** | reflection_draw.rs:1161–1280 |
| Post targets | per slot: full-extent HDR RGBA16F + depth + **2 bloom mips at 1/4 extent**; 2 slots | post_draw.rs:2–6, :1648–1650 |
| Shadow map | 1024² | shadows.rs:46 |
| Ripple field | live count ≤ **32**; measured steady state **~3.7 live rings** (8.5× headroom) | engine-core ripple.rs:56–61, :1143, :1259 |
| Textures | 3 procedural 256×256 RGBA8 + atlas per scene | proc_texture.rs:14–16; corner_scene.rs:135–137 |
| CPU render path | p95 **0.035 ms/frame** (pod, F-02 scratch form; 0.081 before) | CURRENT-STATUS.md:1647–1650 |
| E-7 rig allocation tuple | (19/4/**1589248**/3/7/2) — device_memory/texture_memory/texture_bytes/pools/sets/samplers | EVIDENCE.md:133–148 |
| GPU timing instrument | per-slot timestamp query pool → `gpu_ms_p50/p95/p99/max` | frame.rs:130–136; renderer.rs:244–264 |

Two headline asymmetries drive the whole package:

1. **The capacity constants are 170–680× above the measured demand.**
   MAX_INSTANCES = 4096 (proxy_instances.rs:29–33) against 26 measured
   instances (E-9); MAX_POSE_BLOCKS = 4096 (animation.rs:249–250) against
   24/6 blocks in use — the pose storage reserves 2,883,584 B per slot
   (skinned_draw.rs:87–89) of which the corner uses 16,896 B (0.59%) and
   the garage 4,224 B (0.15%). The region carve hard-codes the
   1000-NPC/8-region demo arithmetic as literals (scene.rs:331–333
   `((8 + 1000) * 36 + 512) * FRAMES_IN_FLIGHT`). This is exactly the
   re-audit's "largest outstanding objective": engine work and resident
   memory proportional to relevant workload rather than
   demonstration-sized fixed capacities (FINDINGS.md:72–74).
2. **The per-frame CPU cost is already small at the measured scale** — the
   whole render path is 0.035 ms/frame p95 (CURRENT-STATUS.md:1647–1650)
   — so the repack/upload findings (A18/A20) are **scaling-shape
   findings, not present-day hotspots**, and every mechanism below earns
   itself against the scaling slope and the charter's stated future
   workloads (streaming, densely authored cities), with no-change
   boundaries stated where the evidence says the current mechanism is the
   right cost/benefit (§10).

---

## 1. A11 — Capacity constants → explicit budgets + demand-sized arenas

### 1.1 The evidence (cited)

- `MAX_INSTANCES: usize = 4096` — "The sustained-load matrix's
  demonstrated entity range is 1,000–5,000; 4,096 is the initial per-draw
  bound (one power-of-two above the demonstrated midpoint). Revisitable
  only with rig evidence, never silently." (proxy_instances.rs:29–33).
- `MAX_POSE_BLOCKS = MAX_INSTANCES` (animation.rs:249–250); `POSE_STRIDE
  = HUMANOID_JOINTS * 64 = 704` (animation.rs:63–64, :72–78);
  `INSTANCE_STRIDE = 96` (proxy_instances.rs:35–41).
- The GPU carves, all sized by the constants:
  - instance buffer: `(MAX_INSTANCES * INSTANCE_STRIDE)` per slot ×
    FRAMES_IN_FLIGHT = 393,216 × 2 = **786,432 B** mapped
    (proxy_draw.rs:407, :439–446 — "One disjoint region per frame slot,
    each holding a full bounded population");
  - pose storage: `SLOT_STRIDE = MAX_POSE_BLOCKS * POSE_STRIDE` =
    2,883,584 B per slot, `slot_stride * FRAMES_IN_FLIGHT` =
    **5,767,168 B** mapped HOST_VISIBLE (skinned_draw.rs:87–89, :193–199)
    — device-lifetime `SkinnedRetained`, created at the first populated
    frame, inherited by every pipeline generation, released only at
    teardown (skinned_draw.rs:120–130);
  - region scene buffer: `region_scene_capacity_vertices() /
    FRAMES_IN_FLIGHT` = 36,800 verts per slot (pipelines.rs:235–250),
    where the capacity is the literal demo arithmetic
    `((8 + 1000) * 36 + 512) * FRAMES_IN_FLIGHT` with its own
    closed-form documentation (scene.rs:313–333).
- The typed-refusal discipline around the constants is excellent and must
  be preserved verbatim: the capacity refusal fires BEFORE any byte
  exists (proxy_instances.rs:275–281; animation.rs:694–699), the repack is
  atomic on refusal (proxy_instances.rs:254–256, test :663–693), and the
  F-03 region clamp is fail-visible with counters (config.rs:245–284;
  renderer.rs:223–236).
- **The P4 §3.6 observability half never landed:** RenderStats carries
  region/overlay capacity fields (renderer.rs:231–236) but NO
  `instance_capacity`/`pose_capacity` — the budget is still a hidden
  global, exactly what P4's design said it would stop being
  (P4-renderer-architecture.md §3.6; verified absent at
  renderer.rs:189–242).

### 1.2 Options

**(a) Status quo with a documented boundary.** Zero risk; 8.32 MB of
mapped carves is real but small against 16 GB VRAM; the constants are
documented and revisitable-with-rig-evidence. Cost: the re-audit's named
objective stays open (FINDINGS.md:72–74) — proportional residency is a
9.3/9.6 rubric bar ("Rendering has scalable instance/resource budget
policy", DIRECTIVE.md:255–257; "Large retained buffers must justify
capacity and replication", DIRECTIVE.md:267–269), and a 6-NPC garage scene
reserving 5.5 MiB of pose storage is not a defensible "justified
capacity".

**(b) Configured budgets only.** `RenderConfig` grows
`instance_capacity`/`pose_capacity`/`region_capacity` fields (defaults =
today's constants). Cheap, but the caller must guess the workload up
front; a mis-sized guess re-creates the problem one world over.

**(c) Demand-sized arenas — the overlay precedent generalized.** The
renderer ALREADY has the exact mechanism for one of the three carves: the
per-slot overlay buffer grows on demand, power-of-two, bounded by
`overlay_capacity_vertices()` (frame.rs:63–107; overlay.rs:94–98 —
"Each frame slot grows independently on demand up to its share of this
budget"), grown only on the fenced slot before acquire (frame.rs:667–676).
The instance/pose/region carves predate that discipline and were sized by
constants instead.

**(d) Grow-on-overflow with the refusal retained (c + hard cap).** First
populated frame sizes the carve at observed demand (rounded up to a
power of two, floor 64); a frame whose supply exceeds the current carve
triggers one growth event (double, up to the configured cap); above the
cap the typed refusal fires exactly as today.

### 1.3 The recommendation

**(d) — demand-sized arenas under a configured hard cap, plus the missing
observability fields.** The reasoning:

1. **It is the repo's own proven pattern, not a new mechanism** — the
   overlay's `capacity_for`/`reserve` (frame.rs:71–95) is the shape; P5
   generalizes it to the two population carves and the region carve,
   each through its OWN ownership-safe growth path (below).
2. **Behavior at the boundary is already correct and stays so:** the
   typed refusal above the cap is byte-identical (same
   `InstancePackError::Capacity`/`PosePackError::Capacity` payloads —
   proxy_instances.rs:93–100; animation.rs:242–247); the F-03 region
   clamp/counter machinery is untouched (config.rs:245–284). "Graceful
   capacity saturation" (charter) is preserved because the CAP, not the
   current carve, is the refusal boundary.
3. **The per-carve growth paths respect the three ownership tiers** (the
   P4 §4.5 taxonomy):
   - region carve (`PipelineState`-owned, mapped, vertex-buffer bound
     with per-draw offsets — no descriptors): grow exactly like the
     overlay slot — new mapped buffer, destroy the old under the slot
     fence discipline (frame.rs:643–678 precedent), or at the
     device-idle recreation window; the F-03 counters keep reporting
     any truncation that occurred before the growth lands (growth is
     accommodation, never masking — the counter is NOT cleared).
   - instance carve (`PipelineState`-owned, mapped, vertex-buffer bound
     — proxy_draw.rs:648 uses raw `cmd_bind_vertex_buffers` offsets, no
     descriptor range to re-write): same fenced/idle growth as region.
   - pose carve (`SkinnedRetained`, device-lifetime, bound as
     STORAGE_BUFFER_DYNAMIC with the concrete per-slot range in the
     sets — skinned_draw.rs:252–266): growth = re-ensure the retained
     state at the larger SLOT_STRIDE under `device_wait_idle` (the
     reflection/post targets' rebuild-safety proof shape — "only a
     recreation — already device-idle — can change either",
     reflection_draw.rs:50–57). Power-of-two growth amortizes this to
     O(log cap) events per session (≤ 6 for 64→4096); the alignment
     check at SkinnedRetained::new (skinned_draw.rs:154–169) re-runs
     per growth.
4. **The cap becomes configuration, not a hidden global:**
   `RenderConfig` grows the three budget fields (defaults = today's
   constants, so every existing caller's behavior at the cap is
   unchanged), and `RenderStats` gains
   `instance_capacity/pose_capacity/region_capacity` plus
   peak-used-per-carve fields — landing P4 §3.6's named-but-unlanded
   observability (renderer.rs:189–242 verified absent). The region
   capacity STOPS being the literal `8 + 1000` demo arithmetic
   (scene.rs:331–333) and becomes the configured budget with the
   closed-form retained as the DEFAULT's derivation comment — closing
   the "A05 population policy leaking into a renderer buffer size"
   coupling P4 named (P4 §1.6) and coordinating with P2-L8's
   WorldConfig (FINDINGS.md:23–24, IN-PROGRESS — no engine-core
   constant welds the simulation shape to the renderer's after this).
5. **Why not (c) pure-demand with no cap:** the charter's "complete-
   primitive capacity policy" and the repo's D2 no-partial-overflow
   discipline want a typed refusal somewhere; an unbounded arena would
   convert today's fail-visible refusal into silent unbounded growth —
   a regression against "graceful capacity saturation."

### 1.4 Wire/behavior compatibility

Byte-identical: the packed instance bytes (FNV pin
`9913137345556047595`, proxy_instances.rs:610), the pose bytes, the
draw streams, the typed-refusal error shapes and payloads, the F-03
counter semantics. What moves, CONSCIOUSLY (D-EE-06 pin corrections,
batched and documented): the allocation-constancy tuples — the carves
now allocate at demand scale, so the batteries must WARM the maximal
population before baselining (the a6e5adc corrective's established
discipline: "warming the capture path before the baseline … the
render_post discipline", git log a6e5adc; FINDINGS.md's S6 record).
RenderStats grows fields (additive; no fingerprint hashes RenderStats —
verified: the campaign digests hash behavior streams, P2 §3.4's
precedent).

### 1.5 Mandatory tests (the L5/L6/B06 pattern)

- **Equivalence battery:** the full lavapipe set (render_corner,
  render_instanced, render_shadows, render_sky, render_post,
  render_reflection, render_weather, render_scene_separation,
  render_orientation, render_overflow, render_encounter, render_garage)
  re-run digit-for-digit; allocation-constancy legs re-based AFTER a
  maximal-population warm frame (the tuple must then hold steady).
- **Scaling evidence:** resident mapped bytes per carve at {garage (6),
  corner (24), 1k, 4k} before/after — the §9.5 census row; the pose
  carve must show 5,767,168 B → demand-sized (≤ 2× the used bytes
  rounded to the power-of-two quantum).
- **Gate boundary:** the typed refusal at the configured cap (existing
  capacity-refusal tests re-run green — proxy_instances.rs:419–483,
  animation's mirror); a NEW growth-path test: supply exceeding the
  carve but under the cap grows once and renders identically; supply
  above the cap refuses with the exact existing error; the region F-03
  counter still reports truncation that preceded a growth event.

### 1.6 Risks

The growth paths touch sync-sensitive ownership (the pose carve's
device-idle re-ensure is the heaviest); a growth event mid-session
stalls a frame (bounded, rare, counted — a `arena_growth_events` stat).
The batteries' pin movement is review noise (batched per D-EE-06). The
configured-cap defaults are `[ASSUMPTION]` until §9 measures the real
demand distribution (§11).

---

## 2. A23 — Pose storage (the renderer half)

### 2.1 The evidence

The shell half is MEASURED no-change (FINDINGS.md:41: device layer
≈434 KiB, cue surface ≈326 KiB, construction-time, "no blanket
replacement warranted"; the open question NEW-SHELL-3 is the owner's
design review — out of P5's renderer scope, §11). The renderer half:

- One mapped STORAGE_BUFFER of `SLOT_STRIDE × FRAMES_IN_FLIGHT` =
  5,767,168 B (skinned_draw.rs:87–89, :193–199), bound per draw as
  binding 5 `STORAGE_BUFFER_DYNAMIC` with the concrete per-slot range
  written into the retained sets (skinned_draw.rs:252–266) and the
  per-slot dynamic offset `slot × SLOT_STRIDE`
  (skinned_draw.rs:586–589; the shadowed family binds the same buffer
  with the same offsets — skinned_draw.rs:1218, :1393–1411).
- `write_slot` copies the frame's whole pose-byte slice into the slot
  region after the fence wait (skinned_draw.rs:538–561) — the slice is
  the safe layer's `repack_poses` output (submission.rs:354–355), i.e.
  `blocks.len() × 704` B for the frame's supplied blocks.
- The pose-index contract makes the storage an INDEXED array: npc i
  reads `pose_index * POSE_STRIDE` relative to the slot base
  (skinned_draw.rs:36–39; proxy_instances.rs:79–88) — several instances
  MAY share one block ("the INDEX form is what makes pose-block DEDUP a
  later non-breaking change", proxy_instances.rs:84–88).

### 2.2 Options

**(a) No-change** — 5.5 MiB mapped, one buffer, no churn. **(b) Demand-
sized SLOT_STRIDE** (A11's arena applied to this carve — §1.3(3)). **(c)
Compact pose layout** (quaternion+translation joints instead of mat4:
704 B → ~11×(4+3)×4 = 308 B) — a versioned WGSL/Rust layout change to
`pose_storage.wgsl`/`pack_pose_bytes` with real bandwidth upside at
4,096 blocks and zero upside at 24. **(d) Pose dedup at the composition**
(the shared-animation-palette question — §3.4 covers it with A18).

### 2.3 The recommendation

**(b)** — A23's renderer half IS A11's pose carve; the demand-sized
SLOT_STRIDE (first populated frame sizes at demand; growth under
device-idle; cap = the configured `pose_capacity`) is the whole
mechanism, landed by R1 (§8). **(c) compact layouts: NO-CHANGE with a
recorded trigger** — the charter's own "Consider compact transform/pose
layouts" is conditional, and the measured bandwidth at 24–26 blocks is
~17 KB/frame against a 0.035 ms p95 CPU path with the GPU-side read
being 11 mat4 fetches per skinned vertex regardless of layout
compactness in the vertex shader's matrix loads; the trigger is the §9
matrix showing pose-upload bytes dominating the frame's CPU cost at a
population ≥ ~1k WITH the dedup lane (§3.4) already landed (dedup first
— it reduces block count, which reduces bytes without any layout
change). **(d)** is A18's lane.

### 2.4 Wire/behavior compatibility, tests, risks

Subsumed by §1.4–1.6 (same carve, same gates). One A23-specific
equivalence leg: the shadowed-populated family binds the SAME retained
buffer (skinned_draw.rs:869, :1053–1054, :1393–1411) — the growth path
must rebuild BOTH families' sets coherently (the re-ensure does; the
shadow battery's known-answer readback is the proof gate).

---

## 3. A18 — Per-frame repack → the dirty-generation protocol

### 3.1 The evidence (cited)

- `adapt_frame` runs `repack_populations` then `repack_poses` on EVERY
  populated submission (submission.rs:311–369; the repack calls at
  :326–332 and :354–355) — validate-all, clear, reserve, regenerate ALL
  bytes (proxy_instances.rs:287–292; animation.rs:700–709).
- `upload_frame_data` then copies the whole instance-byte slice, the
  indirect block, and the whole pose-byte slice into the slot's mapped
  regions (frame.rs:880–909 — `proxy_state.write_slot(slot,
  upload.bytes)` at :881, `retained.write_slot(slot, upload.pose_bytes)`
  at :901).
- The per-instance cost is real arithmetic: two fixed-order Taylor
  series per instance (`sin_cos_turn`, proxy_instances.rs:112–134) plus
  96 B of `to_bits` packing (proxy_instances.rs:189–208); the pose side
  is `sample_pose` + `compose_joint_matrices` (11 chain mat4
  multiplies) + `skin_matrices_bound` (11 more) per block
  (animation.rs:384–404, :414–425, :473–501) — the A17 lane already
  removed the per-NPC rest-chain recomposition (FINDINGS.md:35; the
  bind is constructed once per batch at both production sites —
  corner_routes.rs:623–627, corner_population.rs:243–250).
- The compositions' actual dirtiness:
  - **Corner (render-then-step):** the crowd re-packs per SIM TICK, not
    per frame — `step_composed` re-packs instances and poses only when
    the fixed-timestep accumulator fires a tick
    (corner_demo.rs:320–329, :1132's "THE CROWD'S TICK
    (render-then-step): the submission above"); at 144 Hz rendering vs
    the 60 Hz sim clock, the majority of frames submit a
    byte-identical population.
  - **Garage:** "the placement is static, the pose blocks re-evaluated
    per presented frame" (garage_demo.rs:103–109, :132–138) — the
    INSTANCE bytes are frame-invariant (static table, static tints,
    static pose_index); only the walker pose blocks advance.
  - Standing NPCs' pose blocks are the CACHED rest block every frame
    (corner_routes.rs:633–635, corner_population.rs:269 — `rest_pose_block()`
    from the OnceLock, animation.rs:712–725).
- NEW-REND-08's auditor note (the folded row): "the indirect command
  buffer is rebuilt each frame even when the population is unchanged…
  Cache … per-population hash; rebuild only when instance counts
  change" (audits/renderer.md:107) — the same observation at the
  indirect block (frame.rs:882–893 writes the 48-B block per frame;
  proxy_indirect.rs:59).
- Measured cost today: inside the 0.035 ms/frame p95 render path at 26
  instances (~19 KB of regenerated bytes) — noise (CURRENT-
  STATUS.md:1647–1650). The finding is the O(N) slope: at the 4,096 cap
  the repack is ~4,096 Taylor-pair evaluations + ~393 KB instance
  packing + up to ~2.88 MB pose packing per frame.

### 3.2 Options

**(a) No-change with boundary.** Defensible at demo scale (the measured
numbers above); leaves the render-then-step between-tick frames paying
a full repack for identical bytes, and leaves NEW-REND-08's cached-
indirect observation unanswered.

**(b) Renderer-derived dirtiness (hash or compare).** Hash the input
population per frame: O(N) hashing ≈ the packing cost itself — buys
nothing. Compare against a retained shadow copy: O(N) 40-B compares
vs ~100 flops + 96-B pack per instance — maybe 2–4× cheaper on the
instance side, ~10× on the pose side (704-B memcmp vs ~500 flops), but
still O(N) and doubles the retained CPU memory. Honest but weak.

**(c) Caller-owned generation + per-slot stamps.** The submission
carries a `population_generation: u64` (0 = "always dirty", today's
behavior for every caller that doesn't opt in). The renderer tracks,
per frame slot, the last generation written into that slot's instance/
pose/indirect regions; `upload_frame_data` skips the write when the
submission's generation equals the slot's stamp. The composition bumps
the generation exactly when its population inputs changed (the corner:
on every crowd tick — the same place `corner_instances_into` runs,
corner_demo.rs:320–329; the garage: never for instances, on the pose
advance for poses — see (e)).

**(d) Dirty-RANGE writes.** Per-instance dirty bits → `write_slot`
copies only the changed contiguous ranges. First-order cost is the
REPACK arithmetic, not the mapped memcpy — ranges are second-order;
also the fixed npc→vehicle→rain order (proxy_instances.rs:246–261)
means movers and standers interleave, fragmenting ranges.

**(e) The pose dedup / shared palette.** The pose-index contract
already permits several instances to share one block
(proxy_instances.rs:84–88). Today the corner assigns sequential
`pose_index: i` (corner_routes.rs:588) and submits one block per NPC —
standers each carry a COPY of the identical rest block. The dedup:
the composition submits `[rest] ∪ {walker blocks}` and points every
stander at block 0 — fewer blocks to evaluate, pack, and upload;
rendered output identical (same skin matrices).

### 3.3 The recommendation

**(c) as the protocol, (e) in the same lane, (a)'s boundary stated for
(d).** The reasoning:

1. **The correctness argument is structural, not statistical:** the
   slot regions hold whole frames' bytes; skipping a write is valid
   exactly when the slot already holds byte-identical bytes — which the
   per-slot stamp PROVES by construction (the stamp is written only
   with the bytes). The ping-pong hazard (slot A holds frame N−2, not
   N−1) is exactly why the stamps are PER-SLOT, not global: with
   FRAMES_IN_FLIGHT = 2 (config.rs:178–180), a stable population pays
   one write per slot per generation and then zero — the between-tick
   frames (the render-then-step shape, corner_demo.rs:1132) become
   free.
2. **The stale-render hazard (a missed bump) is the real risk** — a
   caller that forgets to bump renders the prior population. Mitigations
   the design mandates: (i) generation 0 = always-dirty is the DEFAULT,
   so un-opted callers cannot go stale; (ii) a debug-build verification
   mode (behind the existing test-seam discipline — cfg(test)/debug
   only, never a production frame's cost) that re-packs and byte-
   compares when a skip fires, failing loudly on divergence; (iii) the
   adopting compositions are the two demo routes whose bump sites are
   single, visible lines (corner_demo.rs:320–329; garage_demo.rs:138).
3. **The indirect block rides the same generation** (NEW-REND-08's
   cached-indirect note): its 48 bytes are pure functions of the
   per-kind counts (proxy_indirect.rs:106–107) — the same stamp skips
   the write when neither counts nor bytes changed.
4. **(e) is the charter's "shared animation palettes" landed with zero
   renderer change** — it is composition-side, available today by the
   pose-index contract's own design (proxy_instances.rs:84–88), and it
   composes with (c): the corner's standers stop re-evaluating and
   re-uploading identical rest blocks entirely (their block IS the
   shared one).
5. **(d) dirty-ranges: DEFERRED with a measured trigger** — the §9.3
   matrix (population × dirty fraction × repack/upload CPU) decides;
   ranges land only if the measured per-frame cost at a target-scale
   population with a small dirty fraction still exceeds the frame
   budget share after (c)+(e). The charter's "dirty-range/batched
   uploads" is satisfied by the generation protocol's whole-set skip
   plus the R3 staging batch; per-instance ranges are the third-order
   refinement.

### 3.4 Wire/behavior compatibility

The GPU-visible bytes of every frame are byte-identical when the
protocol is honored (a skip writes nothing; a write writes exactly the
repack's bytes — the FNV pin, the pose pins, and every pixel battery
hold). `FrameSubmission`/`ProxyPopulation` grow a field (additive,
versioned public-API change, default preserving today's behavior —
generation 0). The corner table's packed-bytes pin does NOT move (the
pin is a fixed fixture list, proxy_instances.rs:528–595 — the dedup
changes the DEMO's submitted pose_index values, which no fixture pins;
the render gates are pixel-level and hold because the skin matrices
applied are identical). The digest pins on the crowd's simulation
state (rain/ripple fold states, corner_demo.rs:384–393) are
simulation-side and untouched.

### 3.5 Mandatory tests

- **Equivalence battery:** a fuzzed (generation, population) sequence —
   same-generation populations byte-identical, different-generation
   populations written — asserting the per-slot stamps produce exactly
   the bytes the always-write path produces (an oracle harness in the
   upload-policy unit class of uploads.rs's policy_tests); the debug
   verify-mode ON in every test build; the full lavapipe set re-run
   digit-for-digit with the corner + garage routes opted in; the
   render-then-step between-tick frames exercised (a >60 Hz present
   mode or the scripted accumulator shape).
- **Scaling evidence:** repack+upload CPU µs/frame at {24, 1k, 4k} ×
   dirty fraction {0% (static), ~40% (corner standers), 100%} — before
   vs after (c)+(e); the between-tick frame cost must drop to ~the
   submission-plumbing floor at 0%.
- **Gate boundary:** the stale-generation hazard — a deliberate missed
   bump in a test MUST be caught by the debug verify mode (red-then-
   green witness, the B04 pattern); generation 0 callers behave
   byte-identically to today (a route that never bumps, compared
   frame-by-frame).

### 3.6 Risks

The protocol is a correctness-critical surface in the frame path —
class it ORCHESTRATOR per the calibration record's standing rule
(timing/determinism semantics; FINDINGS.md:262–267) even under the
glm-5.3 design-as-spec precedent (FINDINGS.md:291–303). The dedup
changes the demo's submitted bytes (not its pixels) — the equivalence
argument must be written into the lane brief explicitly (same skin
matrices per npc, proven by the skinned known-answer and the pixel
gates).

---

## 4. A20 — Reusable staging, batched uploads, and the texture tails

### 4.1 The evidence (cited)

- **Every static upload creates and destroys its own one-shot
  machinery:** `with_upload_commands` builds a fresh command pool, one
  command buffer, a fence, submits, blocks on
  `wait_for_fences(…, u64::MAX)`, then destroys pool and fence
  (resources.rs:53–104 — the module doc names itself "A20's P5 seam,
  moved not redesigned", resources.rs:1–11).
- **Every device-local buffer upload allocates its own staging pair:**
  `create_device_local_buffer_with_data` creates a staging buffer +
  staging memory per call, maps, copies, unmaps, uploads, destroys
  (resources.rs:106–216, the staging half at :146–193).
- **The callers, all init/attach-time:** the Included demo scene's VBO
  and IBO (pipelines.rs:201–208), the attached scene's mesh
  (corner.rs:133), the three proxy mesh VBOs (proxy_draw.rs:424–437),
  and every texture (textures.rs:154 rides the same helper) — plus the
  atlas. Each is a separate blocking submit.
- **The texture staging tails are PERMANENT:** each `Texture`'s single
  device-local allocation includes the staging buffer's bytes forever
  — "The image and the temporary transfer buffer occupy disjoint pages
  in one device-local allocation. Its full size (including alignment
  and the staging tail) is reported at runtime" (textures.rs:18–20;
  the sizing at :98–112; the reported `texture_bytes` is the E-7
  tuple's 1,589,248 B — EVIDENCE.md:133–148). The tail exists to keep
  one allocation per texture (a deliberate, documented choice) — but
  it is dead VRAM after the one copy.
- **The memory-strategy discipline already present (charter's
  "select device memory strategy based on hardware"):** textures.rs
  prefers a mappable DEVICE_LOCAL type and falls back to
  `cmd_update_buffer` staging in command storage for unmappable VRAM,
  word-aligned and 64-KiB-chunked (textures.rs:113–126, :154–170) —
  the integrated-vs-discrete question is already handled at THIS site;
  the buffer path always uses HOST_VISIBLE|HOST_COHERENT staging into
  DEVICE_LOCAL (resources.rs:132–137, :153–159) — correct on both
  device classes.

### 4.2 Options

**(a) No-change with boundary:** ~10 blocking submits at attach, a few
ms total; the tails are ~0.5–1 MB. Honest at demo scale; leaves the
charter's "reusable staging/retirement", "CPU/GPU overlap", and the
§8.2 "large resource loads/streaming" measurement row without a
mechanism to measure.

**(b) A staging pool with a batch API.** One device-lifetime
`StagingPool` (the `skinned_retained` ownership precedent —
vulkan.rs's retained states, skinned_draw.rs:120–130): a persistently
mapped HOST_VISIBLE|HOST_COHERENT buffer grown power-of-two on demand
(the overlay precedent, frame.rs:71–95), a RESET command pool with
one-time-submit buffers, and a bump allocator. `begin_batch` → N×
(stage bytes → record the copy) → ONE submit → ONE fence → wait or
defer → reset. Per-object machinery: zero.

**(c) Async uploads (deferred wait).** The batch's fence is waited at
FIRST USE (the first frame that draws the uploaded objects) instead of
at upload time — the attach path builds the next objects while the GPU
copies. Requires: the pool's regions stay bound until the batch fence
signals (an in-flight-batch list; reset only past the signaled
watermark), and the first-frame path waits (the first draw IS the
first use — the existing wait discipline covers it).

**(d) Suballocated device-local memory (a general allocator).** The
charter's "per-object allocation vs suballocation" sentence. Every
buffer/image today owns one `vkDeviceMemory` (resources.rs:106–216;
textures.rs:127–132; the target builders). At ~20 allocations
(E-9's device_memory count) fragmentation is zero and the
`maxMemoryAllocationCount` limit (typically 4096) is nowhere near —
suballocation buys nothing now and adds a retirement bookkeeping
surface. NO-CHANGE with the boundary stated and the trigger recorded
(a streaming workload approaching the allocation limit or measured
 fragmentation).

### 4.3 The recommendation

**(b) + (c): the staging pool with batched, deferrable uploads; (d)
NO-CHANGE with trigger; the texture tails retired by (b).** Reasoning:

1. **The pool is the seam every P5 future needs** — "groundwork for
   high fidelity and world streaming" is the charter's own framing
   (DIRECTIVE.md:209): a streaming loader stages through exactly this
   API, and the §8.2 "large resource loads/streaming" row finally has
   a mechanism to measure. The current per-object path cannot express
   a batch.
2. **The tails retire for free:** `Texture::upload` stages in the pool
   (no per-texture staging buffer, no permanent tail — the allocation
   shrinks to the image alone). The one-allocation-per-texture choice
   (textures.rs:18–20) was made to avoid a second allocation per
   texture; the pool is ONE shared allocation for ALL staging —
   strictly better on both counts. The `cmd_update_buffer` fallback
   path for unmappable VRAM is RETAINED verbatim (the hardware
   strategy discipline, textures.rs:154–170) — the pool's mapped
   staging is the primary path on devices that have any HOST_VISIBLE
   type (all of them, per Vulkan's minimum guarantees).
3. **The deferred wait is the charter's "CPU/GPU overlap" made small
   and honest:** attach-time only today, no frame-path change; the
   in-flight bookkeeping is a Vec of (fence, bump-range) checked at
   batch end. Sync validation stays ON in the test gates (the E-9
   0-validation-errors bar).
4. **Failure/retirement discipline preserved:** the pool's growth is
   power-of-two with the old buffer retired at idle (the overlay
   destroy-then-replace shape, frame.rs:80–106); a failed batch
   leaves every live object intact (the corrective-finding-G
   discipline the current helpers already follow,
   resources.rs:112–119, :196–215).

### 4.4 Wire/behavior compatibility

The device-local CONTENTS are byte-identical (the same bytes, the same
copy commands — an oracle test can prove the recorded command
sequences equivalent). No frame-path bytes change. What moves,
consciously: the allocation counters — `texture_bytes` shrinks (the
tails retire; E-7's 1,589,248 → the image-only sum, a documented pin
correction per D-EE-06 with the rig re-measure), `device_memory`
consolidates (the staging pairs vanish into one pool allocation). The
fault-injection cells (TargetFaultSite/Stage, resources.rs:270–297)
are untouched — they target the image builders, not the staging path.

### 4.5 Mandatory tests

- **Equivalence battery:** the full lavapipe set through the pooled
  path (every attach-time upload now pooled) — pixel/capture gates
  digit-for-digit; a batch-vs-per-object oracle: identical bytes
  staged through both paths into a readback-visible destination (the
  capture machinery or a staging->host readback assert); the
  unmappable-VRAM fallback path re-proven (the textures.rs
  cmd_update_buffer arm, its existing tests).
- **Scaling evidence:** the attach census — staging object counts,
  blocking-wait total, submit counts, resident staging bytes — before
  (per-object) vs after (batched): objects → 1 pool, submits → 1/batch,
  waits → 1/batch (or 1 deferred), tails → 0.
- **Gate boundary:** in-flight safety — a batch whose fence has NOT
  signaled must never have its regions reset (a fault-injected
  never-signaling fence in a test; red-then-green); the deferred wait
  must complete before first use (a first-frame draw immediately after
  a deferred batch, sync validation ON, zero errors).

### 4.6 Risks

The pool is a lifetime/sync surface (the in-flight reset discipline);
it is classed orchestrator for the core, lane-able for the caller
migration tail (§8). The E-7 tuple's texture_bytes pin moves — batch
the correction with R1's (one conscious re-pin run on the rig).

---

## 5. A19 — The ripple ring loop, localized by the field's active extent

### 5.1 The evidence (cited)

- The per-fragment loop over ALL live rings:
  ground_reflect.wgsl:144–166 — `for (var i = 0; i < ripple_count;
  i = i + 1)` with a sqrt + band gate per ring, running for EVERY
  fragment of the ground re-draw (the receiver quad's screen
  footprint, reflection_draw.rs:997–1048).
- The bound and the measured reality: `RIPPLE_POOL_BOUND = 32`
  (engine-core ripple.rs:61), authored from the demo study — "the
  steady state is ~3.7 live rings, so the 32 bound carries 8.5x
  headroom" (ripple.rs:49, :56–58; the measured 3.8-live confirmation
  at :1259; "the demo campaign never fills the ripple pool" :1143).
  The loop runs the LIVE count (the uniform's params0.x,
  ground_reflect.wgsl:128), not 32 — so the measured loop is ~3.7
  iterations.
- The identity contract that makes localization safe: "Pixels inside a
  live field but OUTSIDE every ring's band gate keep the same identity
  value-exactly" — uv_offset exactly (0,0), roughen exactly 0
  (ground_reflect.wgsl:58–67); the empty field is a bit-identical no-op
  (:59–62). The CPU oracle (`planar::ripple_perturbation`) mirrors the
  shipped mode-0 path term-for-term (renderer.rs:365–377).
- The uniform's layout: rows[0]/rows[1] params + 32 ring rows
  (reflection_draw.rs:98–122; the packer renderer.rs:1069–1094 —
  rows[1] = (roughen gain, TEST-ONLY debug lane, **0, 0**) — two
  reserved lanes; the pack is a pure read after the tick, the
  finding-18 discipline, corner_weather.rs:315–324).
- The wet pass's cost sits inside the GPU timestamp instrument already
  (frame.rs:130–136 → renderer.rs:244–264) and inside the rig's
  render_weather/render_reflection batteries (E-7; E-10's
  post-resize discipline).

### 5.2 Options

**(a) No-change with boundary.** ~3.7 live rings × the ground
footprint × ~15 flops — against a 5070 Ti this is nothing; the
saturation case (32 rings, full-screen ground at 4K) is ~32 × 8.3 M
px × 15 flops ≈ 4 G flop ≈ well under 1 ms. The honest statement: the
loop is not a measured hotspot at any shipped workload.

**(b) CPU-side receiver-rect culling in the packer.** The ground
re-draw rasterizes ONLY the receiver range (reflection_draw.rs:997–1048);
a ring whose annulus [radius−width, radius+width] around its center
misses the receiver rect contributes exactly zero fragments — dropping
it from the packed prefix is output-bit-identical (the identity note
covers every in-rect fragment: it is outside the dropped ring's band).
Cost: the packer needs the receiver rect (plumbing the attached
scene's GroundRoles receiver range into the wet camera pack —
scene_description.rs carries it; the pack is device-free pure today,
renderer.rs:1069–1094).

**(c) Shader-side field-extent early-out.** The uniform gains ONE row
(the versioned-claim discipline — the params vec4s' reserved lanes are
claimed only by a versioned change, the pose_index/alpha precedents,
proxy_instances.rs:35–41, :79–88): rows[2] = (bound_center_x,
bound_center_z, bound_radius, 0) — the conservative bounding circle
over all live rings (max_i(|center_i| + radius_i + width), centered at
the rings' bounding-box center). The fragment: if outside the bound,
take the identity path directly (the SAME values the loop produces —
bit-identical by the identity contract); else run the loop verbatim.

**(d) Tile/emitter-localized processing** (the charter's "Localize
ripple/particle processing by emitter/visible region/tile"): compute
passes / tiled ripple accumulation. The engine has NO compute passes;
this is a new subsystem. Defer with a recorded trigger.

### 5.3 The recommendation

**(c) the field-extent early-out as A19's named localization, (b)
folded into the same lane where the plumbing is cheap; (a)'s boundary
stated; (d) deferred with trigger.** Reasoning:

1. **(c) is bit-identical by the shader's own documented contract**
   (ground_reflect.wgsl:58–67): the early-out reproduces exactly the
   identity values an out-of-band fragment already gets — the ink
   gates (E-7's ripple sweep 22/255, the term separations
   96548/67535/7625 + 27149/27149/2538) must reproduce
   digit-for-digit, and they will because no in-band fragment's
   arithmetic changes at all.
2. **The cost/benefit is honest:** ~30 lines of shader + packer + one
   uniform row, buying the loop's elimination for the (typical)
   majority of ground fragments far from the ~3.7-ring field. It is
   the mechanism the finding names ("the loop bounded by the field's
   active extent") at the smallest possible surface — no tiling, no
   compute, no per-ring datastructure.
3. **(b) is the same win on the CPU side** (fewer rings packed at all)
   and exact by the same argument; it rides the same lane because the
   packer changes once. If the GroundRoles plumbing turns out to
   complicate the device-free pack contract (the pack is called from
   adapt_frame before any device state, submission.rs:425–430), (b)
   may be dropped from the lane without affecting (c) — recorded as
   the lane's optional tail.
4. **The versioned layout claim is the repo's established pattern**
   (the alpha slot's Era-2 claim, the pose-index claim — both
   documented at their sites with the pins re-run green): rows[1].z/w
   stay reserved; the NEW row is rows[2] and the ring rows shift to
   rows[3+i]. RIPPLE_UNIFORM_ROWS 34 → 35 (reflection_draw.rs:101);
   the stride/base arithmetic (reflection_draw.rs:281–296) follows;
   the WGSL struct mirrors (ground_reflect.wgsl:68–74). No S5 layout
   pin covers the ripple block (the pins cover FrameParams/
   MaterialParams/LightMatrix — build.rs:340–346), so the contract is
   the packer↔shader term-for-term mirror plus the identity/ink gates
   — both re-run.
5. **Why not a per-ring squared-distance restructure** (avoiding the
   sqrt before the band test): a pure arithmetic micro-opt on a
   non-hotspot with REAL bit-identity risk (the `abs(dist − radius)
   < width` decision at the band edge, radius < width edge cases) —
   rejected; the early-out achieves the class of win without touching
   the in-band arithmetic.

### 5.4 Wire/behavior compatibility

Rendered output bit-identical (the identity contract; the ink gates
digit-for-digit). The ripple uniform's internal layout changes (the
versioned row claim) — no pin hashes the uniform bytes (the
rain/ripple fold-state pins hash the SIMULATION state,
corner_demo.rs:384–393; the E-7 pins are pixels/allocations). The
empty-field and debug-lane semantics are untouched
(ground_reflect.wgsl:59–62, :129–138).

### 5.5 Mandatory tests

- **Equivalence battery:** render_weather + render_reflection re-run
  on the rig digit-for-digit (the identity gates, the sweep, the term
  separations — E-7's values); a NEW unit battery: the early-out path
  vs the unmodified loop on synthetic fields — for randomized ring
  sets, fragments inside and outside the bound produce identical
  fragment outputs (a CPU-side harness over the same arithmetic, the
  planar oracle's domain); the empty field's no-op pin re-run.
- **Scaling evidence:** the GPU timestamp rows (gpu_ms_p50/p99,
  renderer.rs:244–264) on the wet route at {empty, 3.7-ring steady,
  32-ring saturation} × {720p, 1440p} before/after — the saturation
  row is the gate the finding actually names.
- **Gate boundary:** the bound's conservativeness property test — for
  randomized fields, the bound NEVER excludes a fragment any ring's
  band contains (the no-false-negative-culling bar, DIRECTIVE.md:209);
  the versioned-row claim's cross-side agreement (the packer's rows
  and the shader's struct — one layout test).

### 5.6 Risks

A shader semantics change — class ORCHESTRATOR (the visual-equivalence
class; S5's precedent). The versioned row shifts every ring row
(packer + shader + the two stride derivations) — a one-shot
coordinated change, gated by the ink pins. The (b) tail's plumbing
risk is contained by its optional status.

---

## 6. B14 — The reflection/post target package: measure first, alias only on evidence

### 6.1 The evidence (cited)

- **The reflection targets are per-slot, FULL swapchain extent, RGBA16F
  color + the renderer's depth format** — the `create_image` closure
  sizes every image at `extent.width × extent.height`
  (reflection_draw.rs:1173–1240; the per-slot loop :1242–1265; the
  framebuffers :1266–1280); REFLECTION_FORMAT = R16G16B16A16_SFLOAT
  "required, not negotiated" by the render-pass compatibility with the
  post state's HDR pass (reflection_draw.rs:91–96, :59–72).
- **The post targets are per-slot HDR color + depth at full extent plus
  two bloom mips at the 1/4 extent** (post_draw.rs:2–6 the chain;
  :1648–1650 "the HDR color target + its depth attachment at the scene
  extent, the two bloom mips at the 1/4 extent"; HDR_FORMAT RGBA16F
  :90–95).
- **The ground re-draw samples the reflection target 1:1 at screen
  UV** — "the target is the frame's extent" (ground_reflect.wgsl:109–114;
  the planar construction, reflection_draw.rs:7–27) — a smaller
  reflection target is NOT a drop-in: it needs a UV remap AND changes
  every sampled texel (a fidelity change, §6.3).
- **The lifetimes within one frame are strictly ordered passes** (the
  record cascade, frame.rs:913–926): shadow depth → wet mirrored-scene
  (+barrier, reflection_draw.rs:932–982) → the scene pass (main or HDR)
  → bloom mips → composite → readbacks. The reflection color image's
  last read is the ground re-draw INSIDE the scene pass
  (reflection_draw.rs:984–1048); the bloom images' first write is
  AFTER the scene pass — **disjoint lifetimes within the frame.**
- **The shadow map is 1024²** (shadows.rs:46).
- **The counters that make the census cheap already exist:**
  target_images/views/framebuffers/memories + frees
  (renderer.rs:170–186), texture_memory/texture_bytes (:156–167), the
  allocation tuple (E-7), and the per-slot GPU timestamps
  (frame.rs:130–136).
- The arithmetic at the gate extent 1280×720 (921,600 px; RGBA16F 8
  B/px; depth 4 B/px): reflection ≈ 2×(7,372,800 + 3,686,400) =
  **22,118,400 B**; post ≈ 2×(7,372,800 + 3,686,400 + 2×460,800) =
  **23,961,600 B**; both scale linearly with extent (4K ≈ 9× the
  720p numbers). P4's design explicitly deferred this to P5:
  "B14's aliasing question is explicitly deferred to measurement (P5)"
  (P4-renderer-architecture.md §4.1); FINDINGS.md:57: "P5 measurement
  package."

### 6.2 Options

**(a) The census only, then no-change with boundary.** Measure the
target residency per feature × extent (the §9 row), record the
boundary, change nothing.

**(b) The census + gated aliasing.** If the census shows targets are a
material share of the renderer's device memory at the owner's real
usage, alias the disjoint-lifetime pairs: reflection color ↔
bloom_a/b (both RGBA16F; lifetimes disjoint within the frame; the wet
barrier discipline already proves the transition pattern,
reflection_draw.rs:932–982), and reflection depth ↔ HDR depth (both
written-only within their own passes). Mechanism:
VK_IMAGE_CREATE_ALIAS_BIT + one shared device memory + layout
transitions at the lifetime boundaries — barriers hand-proven under
sync validation, exactly like the existing wet read barrier.

**(c) Half-resolution reflection.** Render the mirror at 1/2 extent,
bilinearly upsample in the ground re-draw. Halves the reflection
target bytes and the mirrored-scene fill cost — but CHANGES EVERY
SAMPLED TEXEL (a fidelity change). The charter's "Do not reduce
fidelity or content to speed up" (P4's non-goal precedent) makes this
the OWNER's lever only, never a P5 default.

**(d) A render graph / automatic aliasing.** Rejected by P4 §4.1 with
the justification that stands (pass set ≤ 7, fixed documented order,
hand-proven barriers); P5 does not reopen it.

### 6.3 The recommendation

**(a)+(b): the measurement package IS the deliverable; aliasing is
gated on it. (c) recorded as the owner's fidelity lever, default
NO.** Reasoning:

1. **The charter's order is explicit:** "Select … based on hardware" /
  "only when evidence warrants" / the §8.2 rendering row demands
  "VRAM/native allocations … churn and resource retirement" measured
  BEFORE optimization claims (DIRECTIVE.md:207–209, :232). P4's
  coordinator was built so this measurement is cheap (the counters +
  the FramePlan's explicit uses, frame_plan.rs:86–134, let the census
  attribute residency per feature family precisely).
2. **The aliasing engineering is real but bounded** — the lifetimes
  are already proven disjoint by the cascade's own documentation
  (frame.rs:913–926); the wet barrier is the pattern for the
  transition (reflection_draw.rs:932–982); the fault-injection cells
  and the allocation-constancy batteries are the teeth. But on a
  16 GB discrete card at 720–1440p, ~46 MB of targets is not a
  problem — the census decides whether the integrated/shared-memory
  case or a 4K window makes it one. [ASSUMPTION] §11 names the
  threshold.
3. **The census also closes the 9.6 rubric row** ("Large retained
  buffers must justify capacity and replication") for the target
  families: the per-slot replication is justified by the
  ping-pong-fence ownership proof (reflection_draw.rs:59–64 — "slot
  S's reflection overwrites only slot S's images, inside the frame
  that waited slot S's fence"); the census DOCUMENTS that
  justification with numbers.

### 6.4 Wire/behavior compatibility (if aliasing lands)

The image CONTENTS and the pass order are unchanged — aliasing moves
WHERE the bytes live, never what they are. The batteries' captures
must be byte-identical (the S1g byte-equality gate shape,
E-10's re-confirmation pattern). The allocation tuple moves
consciously (target_memories drops; one shared allocation replaces
pairs — a D-EE-06 batched pin correction with the rig re-run).

### 6.5 Mandatory tests

- **The census itself (the scaling evidence):** per feature family
  {wet, post, wet+post, shadows} × extent {1280×720, 1920×1080,
  2560×1440, 3840×2160}: the target counters, computed resident
  bytes, and the GPU pass timestamps — release mode, the owner's rig
  (the standing lavapipe-correctness-only rule, EVIDENCE.md:237).
- **If aliasing lands (equivalence battery):** the full wet+post
  battery set digit-for-digit through aliased targets; sync
  validation ON with zero errors (the E-9 bar); the fault-injection
  matrix re-run (the Wet/Post image/memory/bind stages now create
  aliased objects — the cells must still fire);
  allocation-constancy re-based.
- **Gate boundary:** the lifetime-disjointness proof made executable —
  a sync-validation run at every feature combination (the existing
  validation_errors==0 gates are exactly this), plus a targeted test
  that a ground re-draw NEVER reads a bloom-aliased image (the
  barrier's scope asserted by the validation layer).

### 6.6 Risks

Aliasing is the highest-sensitivity change in the package (memory
lifetime + barriers); it is gated BEHIND the census and classed
orchestrator. The census itself is zero-risk (read-only counters).
The (c) lever is explicitly not-P5's-to-pull.

---

## 7. A17 — the landed verification (NO-CHANGE with evidence)

The charter lists A17 in P5; the register records it
FIXED-AWAITING-REVIEW integrated `3439633` (FINDINGS.md:35): the
`rest_pose_block` OnceLock cache (animation.rs:63–70, :712–725) and
`SkinBind`/`skin_matrices_bound` with the bind constructed ONCE PER
BATCH at both production sites (corner_routes.rs:623–627,
corner_population.rs:243–250; the delegation record at
FINDINGS.md:291–303). Verified present at HEAD. **P5's disposition:
NO-CHANGE** — the remaining A17-adjacent cost is the per-walker
`sample_pose` + chain composition, which is A18's generation protocol's
to skip (§3.3(1)), not A17's to restructure. The A17 row closes when
the register's re-audit confirms (the P7 flow); no P5 lane touches it.

---

## 8. The lane table

Dependency order; classification per the calibration record's standing
rules (FINDINGS.md:262–267: timing/determinism semantics work is
orchestrator-implemented or design-first; FINDINGS.md:291–303: glm-5.3
implementation lanes under design-as-spec are 4/4 clean — the
design-document-IS-the-spec pattern this document serves). Every lane's
acceptance includes the full lavapipe battery set + fmt/clippy + the
workspace gates per D-EE-03, plus its own discriminating tests (§1–6).
No two concurrent lanes share a file (the P2 conflict-free ownership
rule).

| Lane | Content | Class | Depends on | Pins that must hold | Discriminating tests |
|---|---|---|---|---|---|
| **R0 — the census harness** | The §9 measurement rows as harness/battery code: the VRAM census (counters + computed bytes per feature × extent), the repack matrix (population × dirty fraction), the attach census, the ripple timestamp rows. Additive harness only. | LANE-ABLE (measurement owner; harness code, the B10 evidence-quality precedent) | none | none (read-only instrumentation; no behavior change) | the census rows themselves, run on the rig, archived per E-8 discipline |
| **R1 — A11/A23 arenas + budgets** | Demand-sized instance/pose/region carves (the overlay precedent, §1.3); RenderConfig budget fields (defaults = today's constants); RenderStats capacity/peak fields (P4 §3.6's unlanded half); the region literal → configured budget. | **ORCHESTRATOR** (allocation-constancy pins + the SkinnedRetained device-idle rebuild — the A24 equivalence-sensitive class) | R0 (the demand numbers feed the growth quantum decision; the before-census is the baseline) | every pixel/capture battery digit-for-digit; the FNV/pose byte pins; the capacity-refusal payloads | growth-path test (under-cap growth renders identically; at-cap refusal exact); F-03 counter survives growth; constancy legs re-based after maximal warm |
| **R2 — A18 dirty generation + pose dedup** | The `population_generation` submission field (0 = always-dirty default); per-slot stamps in upload_frame_data; the indirect block on the same stamp; the debug verify mode; composition adoption (corner bump-on-tick, garage static instances); the stander→rest-block dedup. | **ORCHESTRATOR** (frame-path semantics; the standing rule) | R1 (the carve/write surface settles first — both touch proxy_draw/skinned_draw write paths) | every battery digit-for-digit with the routes opted in; the simulation-side digest pins (untouched by design) | the (generation, population) fuzz oracle; the missed-bump red-then-green witness; generation-0 byte-identity; between-tick frames measured |
| **R3 — A20 staging pool** | The StagingPool (mapped, power-of-two, in-flight batch tracking); the batch API; all attach-time callers migrated; the texture tails retired; the deferred-wait option. Core pool + sync semantics orchestrator; the caller migration tail lane-able. | **ORCHESTRATOR core / LANE-ABLE tail** | R0 (the attach census); parallel-safe with R1/R2 (disjoint files: resources.rs/textures.rs/corner.rs/pipelines.rs upload sites vs R1's carve owners — sequence the pipelines.rs hunk against R1's) | every battery through the pooled path; the E-7 ink values; the fault cells still firing | batch-vs-per-object oracle; never-signaled-fence reset refusal; deferred-wait-first-use sync validation; the tail retirement measured |
| **R4 — A19 ring localization** | The versioned ripple row (RIPPLE_UNIFORM_ROWS 34→35 both sides); the shader early-out; the packer bound; the optional receiver-rect culling tail. | **ORCHESTRATOR** (visual-equivalence class, the S5 precedent) | none (independent of R1–R3; touches reflection_draw.rs + ground_reflect.wgsl + renderer.rs's packer — renderer.rs also touched by R1's stats fields: sequence or partition hunks) | render_weather + render_reflection digit-for-digit (the identity gates, sweep, term separations); the empty-field no-op | the early-out-vs-loop equivalence harness; the bound's no-false-negative property; the saturation timestamp row |
| **R5 — B14 census + the aliasing decision** | The census analysis over R0's rows; the aliasing DECISION RECORD (gated on the census + the owner's threshold ruling); if gated in: the ALIAS_BIT implementation for the disjoint pairs. Census/analysis lane-able; aliasing implementation orchestrator. | LANE-ABLE (census) / **ORCHESTRATOR** (aliasing, if authorized) | R0 (the census rows); the owner's threshold ruling (§11) | if aliasing: the full wet+post battery digit-for-digit; sync validation 0; the fault matrix | the census table; the lifetime-disjointness executable proof; the tuple's conscious re-pin |
| **M-P5 — the rig runs** | The §9 plan's serial rig executions per lane (before/after), the release-mode curves, the E-8-format evidence records. | measurement owner | runs alongside every lane | E-8/E-9 cells re-run identical for no-behavior lanes | the archived rows themselves |

**Sequencing rationale:** R0 first — the B06/L5/L6 discipline (real
counts before mechanisms) and P2's own §0 precedent; every later lane's
[ASSUMPTION] defaults are pinned by it. R1 before R2 because the
generation protocol layers per-slot stamps onto the same write paths
R1 reshapes (upload_frame_data's carve owners — frame.rs:880–909);
landing R2 first would build on a moving surface. R3 is
file-disjoint from R1/R2 except the pipelines.rs upload hunk — cheap
to sequence. R4 is independent (shader-side) and may run any time
after S5's composed tree (landed); its only contention is renderer.rs
hunks with R1 — partition or sequence. R5's decision consumes R0 and
the owner's ruling; its implementation, if authorized, is last among
the mechanism lanes (highest sync sensitivity, gated payoff).

**Expected count movements (stated for the pin workflow, per D-EE-06):**
R1: renderer release pin +N tests (growth/capacity/stats), the
constancy legs re-based (no count change, value moves documented);
R2: +N (fuzz oracle, missed-bump witness, generation-0 identity);
R3: +N (pool oracle, reset refusal, deferred wait); R4: +N (early-out
equivalence, bound property, layout agreement) — the weather/reflection
pins' VALUES unchanged; R5: census harness +N, and if aliasing lands,
the tuple's conscious re-pin. All pin corrections batched at each
lane's gate close, never silent.

---

## 9. The measurement plan (charter §8.2, the E-8 extension)

**Baseline anchors (already measured):** the CPU render path p95 0.035
ms/frame pod (CURRENT-STATUS.md:1647–1650); E-9's maximal-composition
walk (2,537 frames, 26 instances, validation 0/0, the allocation
tuple); E-7's rig ink pins + tuple; the ripple steady state ~3.7 live
(E-8 log via ripple.rs:49's citation of the study). These are the
BEFORE numbers; every lane re-runs the relevant rows serially on the
rig, release mode, matched conditions, raw samples archived with
commands and exit codes (the E-8 discipline, EVIDENCE.md:152–156).

1. **The VRAM census (B14/R0/R5):** per feature family {dry, +shadows,
   +post, +wet, +wet+post, the maximal composition} × extent
   {1280×720, 1920×1080, 2560×1440, 3840×2160}: the target counters
   (renderer.rs:170–186), texture_memory/bytes, the computed carves
   (§0.1's arithmetic), and the full AllocationStats tuple. One table,
   the owner's rig, release. This is the row that gates §6.
2. **The repack matrix (A18/R2):** populations {6, 24, 1k, 4k} × dirty
   fraction {0%, the corner's stander mix, 100%} × {instance repack,
   pose evaluation, slot write} — CPU µs/frame, before/after R2; plus
   the render-then-step frame pattern at present rates ABOVE the sim
   clock (the between-tick unchanged-population frames — the protocol's
   actual win case, measured).
3. **The arena residency (A11/A23/R1):** resident mapped bytes per
   carve at {garage, corner, 1k, 4k} before/after — 8,320,000 B →
   demand-sized; the growth-event count over a session (the
   amortization claim).
4. **The attach/upload census (A20/R3):** staging object counts,
   submits, blocking-wait total, resident tail bytes — before
   (per-object, ~10 submits, permanent tails) vs after (1 pool,
   1/batch, tails 0); the deferred-wait overlap measured as
   attach-wall-time.
5. **The ripple rows (A19/R4):** gpu_ms_p50/p99 (renderer.rs:244–264)
   on the wet route at {empty, 3.7-ring steady, 32-ring saturation} ×
   {720p, 1440p} before/after — the saturation row is the finding's
   named worst case; the identity gates re-run in the same sessions.
6. **The combined-scene curves (the charter's "measure composition
   overhead, not sum of microbenchmarks"):** the maximal composition
   route (E-9's flag set) at each census extent — total frame
   p50/p95/p99/max, GPU render ms, presented-frame count, validation
   0/0 — re-run after EVERY lane (the no-regression bar; a small-
   workload regression cannot hide behind a large-workload win,
   DIRECTIVE.md:239–241).
7. **The gate-boundary rows:** the typed-refusal behavior at the
   configured caps (capacity refusals re-run); the F-03 region
   counters on a deliberately over-supplied frame; the saturation
   frames' observable behavior (refusal/counter, never a hang or a
   partial buffer).

**Reporting discipline:** exact commands, exit codes, tree SHAs,
artifact paths in EVIDENCE.md; slopes with sample counts and noise
floor; no speedup claims below measured noise; lavapipe for logic
only, the rig for every number above (the standing rule).

---

## 10. No-change recommendations (evidence per charter §2)

1. **A17 — already landed** (§7): the cache + bind are present and
   verified at HEAD; nothing further is warranted.
2. **The per-object device-memory allocations — keep** (§4.2(d)): ~20
   allocations against a typical 4,096 limit, zero measured
   fragmentation; suballocation adds a retirement surface for no
   measured win. Trigger recorded: a streaming workload approaching
   the allocation limit.
3. **The compact pose/transform layouts — keep the mat4 form**
   (§2.2(c)): the measured bandwidth at 24–26 blocks is trivial; the
   trigger is the §9.2 matrix showing pose bytes dominating at ≥1k
   population AFTER the dedup lands.
4. **Dirty-RANGE (per-instance) uploads — defer** (§3.2(d)): the
   generation protocol + dedup remove the whole-set cost first; ranges
   are the third-order refinement, gated on the §9.2 matrix.
5. **GPU-driven visibility / compute ripple tiling — defer with
   triggers** (§5.2(d), §3 of the charter's "only when evidence
   warrants"): no measured workload needs GPU-authored counts (26
   instances, E-9) or tiled cosmetics; the indirect mode's seam
   (ProxyDrawMode::Indirect, renderer.rs:89–107) already exists for
   the future consumer. Triggers: a population where the CPU
   submission cost is measured dominant AND culling is authoritatively
   conservative (no false negatives — the charter bar).
6. **The half-resolution reflection — NOT P5's to change** (§6.2(c)):
   a fidelity change; the owner's lever, recorded.

---

## 11. Open questions for the owner (none blocking design)

1. **[ASSUMPTION] The B14 aliasing threshold:** proposed — design the
   aliasing only if the census shows targets ≥ ~10% of the renderer's
   total device-memory residency at the owner's typical extent, OR the
   integrated/shared-memory deployment is in scope (the AMD iGPU on
   the rig is the available test case, EVIDENCE.md:16). Owner rules;
   R5 consumes.
2. **[ASSUMPTION] The arena defaults:** initial carve = max(64,
   first-demand) rounded to the power-of-two quantum; growth =
   doubling; caps = today's constants (4096/4096/36,800-per-slot
   derived). R0's demand distribution should confirm the quantum —
   owner sign-off on the final numbers.
3. **[ASSUMPTION] The A18 adoption scope:** the corner + garage routes
   opt in (the two visible bump sites); every other caller stays
   generation-0 (today's behavior). Confirm the scripted census routes
   may also opt in (their pixel pins are unchanged by design — §3.4 —
   but the owner may prefer the scripted routes maximally
   conservative).
4. **The staging pool's deferred wait:** proposed deferred-to-first-
   use with the first-frame wait mandatory; owner may prefer
   inline-wait-only for the first cut (simpler, still batches).
5. **The region growth policy:** grow on F-03 truncation up to the
   configured cap (counter always reported), or configured-only with
   no growth? Proposed: grow (the counter is the observable either
   way).
6. **NEW-SHELL-3 (the two audio surfaces):** the register's open
   design question (FINDINGS.md:221) is the owner's review — P5 states
   it is OUT of this package's renderer scope (the A23 shell half is
   measured no-change, FINDINGS.md:41).
7. **The half-resolution reflection lever** (§10.6): owner's call,
   with the fidelity statement, if the census shows the reflection
   family dominant.

---

## 12. Honest coverage limits of this design's evidence

- Read fully or in cited ranges: everything in §0's log; the citations
  above are the load-bearing set. NOT verified line-by-line:
  `shadow_draw.rs`'s target builder interior (its extent constant and
  set shapes are cited; its census row is R0's to measure),
  `sky_draw.rs`, `capture.rs`, `swapchain.rs`, `context.rs` interiors,
  `populated_meshes.rs`/`proxy_mesh.rs` mesh bodies (vertex counts
  consumed via the cited proxy_draw fields), the persist/shell crates
  beyond the register's dispositions, and the E-8 log's raw artifact
  (cleaned per EVIDENCE.md:4–10 — the summarized numbers stand as the
  durable record; the re-verification re-runs the commands).
- The VRAM arithmetic in §0.1/§6.1 is computed from source extents
  and formats, not device-queried allocation sizes (Vulkan
  `requirements.size` ≥ the texel sums by alignment) — the census's
  counters are the authoritative numbers; the arithmetic is the
  prediction the census checks.
- No measurements were taken in this session (read-only mandate): the
  demand/growth amortization, the repack slopes, and the ripple
  saturation costs are structural claims GATED by §9's rows, not
  asserted as proven — the charter's evidence rules carry the burden
  (DIRECTIVE.md:221, :245).
- The pose-dedup equivalence argument (§3.4) rests on the pose-index
  contract's own design (proxy_instances.rs:79–88) and the skinned
  known-answer gates; the lane must write the explicit per-npc
  skin-matrix-identity proof into its brief (the audits'
  citation-error record is the cautionary precedent — every number
  above was re-derived from source, including the brief's own
  FINDINGS line references, which were re-checked against
  FINDINGS.md:31–57 at HEAD).
- Line numbers are exact at `9e17516`; any rebase shifts them (the
  program's standing caveat — symbols are stable, numbers are
  SHA-scoped).

---

## 13. Charter §7-P5 sentence-by-sentence compliance trace

| Charter sentence (§7 P5, verbatim order) | Design section |
|---|---|
| "Build on P4 interfaces: resource residency and explicit budgets" | §1 (budgets + arenas on the FramePlan/RenderConfig surfaces) |
| "complete-primitive capacity policy" | §1.3(3)–(4) (the cap + the typed refusal preserved) |
| "persistent instance/mesh/material/pose storage" | §1/§2 (the carves' persistence kept; only their SIZE policy changes) |
| "dirty-range/batched uploads" | §3 (the generation protocol; ranges deferred with trigger), §4 (the staging batches) |
| "reusable staging/retirement" | §4 (the pool + the in-flight retirement discipline) |
| "visibility and LOD-driven work, CPU/GPU overlap" | §4.3(3) (deferred wait); visibility deferred with trigger (§10.5) |
| "Select device memory strategy based on hardware … do not assume host-visible is always slower or device-local always wins" | §4.1 (the existing textures.rs preference preserved verbatim; the pool keeps HOST_VISIBLE staging + DEVICE_LOCAL destination — correct on both classes) |
| "Evaluate per-object allocation vs suballocation with Vulkan limits and fragmentation" | §4.2(d) (evaluated: no-change with the boundary + trigger) |
| "Use indirect GPU authorship/visibility, GPU skin/particle or CPU alternatives only when evidence warrants" | §10.5 (no measured warrant; the Indirect seam exists for the future) |
| "separate cosmetic from authoritative effects and respect determinism" | §3.4/§5.4 (no simulation state touched; the cosmetic ripple/rain effects byte/pixel-identical) |
| "Preserve conservative bounds, no false-negative culling, frame-slot ownership, multiple passes, visibility stability and graceful capacity saturation" | §1.3(2) (the cap + F-03), §5.5 (the bound's no-false-negative property test), §6.1 (slot ping-pong ownership preserved), §1.5 (the saturation gate rows) |
| "Consider compact transform/pose layouts, shared animation palettes and unchanged updates" | §2.2(c) (compact: no-change with trigger), §3.3(4) (the shared palette = the dedup) |
| "show costs including small/all-dirty workloads, CPU cycles and GPU bandwidth" | §9.2 (the dirty-fraction axis), §9.1/§9.5 (CPU cycles + GPU timestamps + VRAM) |
| "Localize ripple/particle processing by emitter/visible region/tile while maintaining correct natural impacts and projected distortion" | §5 (the field-extent localization, bit-identical; tiling deferred) |
| "Measure combined scene rather than isolated improvements only" | §9.6 (the maximal-composition curves after every lane) |
| "This is groundwork for high fidelity and world streaming, not a mandate to build every future graphics feature" | §10 (the no-change list + triggers), §4.3(1) (the pool as the streaming seam) |

---

*— design-p5 complete at `9e17516`; the handoff summary (commit SHA,
lane table, per-finding recommendations, measurement headline, the
[ASSUMPTION] list, risks) is in the dispatching session. Lanes R0–R5 +
M-P5 above are the independently reviewable delivery units.*
