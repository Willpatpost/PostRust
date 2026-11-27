# P4 — Renderer Architecture and Separation (DESIGN)

**Status:** COMPLETE (design-p4 subagent, glm-5.3-w4a8).
**Base for all source citations:** `refactor/engineering-excellence` @
`7961cb290cc5222db88d222333fc4de8f4183925` (working-tree HEAD observed at
dispatch; `crates/**` read-only throughout — no cargo, no commits, no pushes).
**Head moved to `a972f3e` mid-session** (the orchestrator's p2-l1 closure
commit, which also committed this file's early skeleton): `git diff
--name-only 7961cb2 a972f3e` touches only the CI workflow,
`engine-core/src/interaction.rs` (B06), FINDINGS.md and this file — zero
engine-renderer/engine-slice/shader changes, so every source citation below
is exact at both SHAs.
**Charter:** `DIRECTIVE.md` §7 P4 paragraph (every sentence treated as a
requirement; see §13 for the sentence-by-sentence trace) + §4 engineering
standards. Finding rows: A01 (renderer half), A03, A04, A06, A11, A12, A14,
A15, B14, B15 in `FINDINGS.md`.

---

## 0. Source-reading log (coverage + honesty)

READ in full or in cited ranges (all line numbers refer to the SHA above):

- `DIRECTIVE.md` §0/§4/§7-P4/§9; `FINDINGS.md` (all cited rows + the
  calibration record and standing decisions D-EE-06/08/09);
  `audits/slice.md` §2 (module classification map); `DECISIONS.md`;
  `EVIDENCE.md` E-6/E-7; `AGENTS.md` (registry).
- `renderer.rs` (1,465 lines — full structural read: the `Renderer` struct,
  both entry families, sticky setters, capture surface, `Drop`, ripple
  packing).
- `vulkan.rs` (6,158 lines — targeted: context/teardown 240–450, 1000–1110;
  pipeline state + ensure dispatcher 1887–2503; the whole `render_frame`
  3930–4949; symbol greps for the rest).
- `scene_record.rs` (585 lines, full), `frame_prepare.rs` (364, full),
  `config.rs` (78–197), `scene.rs` (300–348), `lit_scene.rs` (1–80),
  `animation.rs` (1–60), `skinned_draw.rs` (744–873), `lib.rs` (full),
  `build.rs` (full), `Cargo.toml` (deps), `shaders/instanced_proxy{,_shadowed}.wgsl`
  (full/partial) + line counts and `git diff --no-index --stat` for all five
  variant pairs.
- Slice side: `corner_demo.rs` render-dispatch sites (1008–1021, 1322–1335),
  `encounter_frame.rs` `draw` (195–234), greps for every
  `render_corner*`/`render_layers`/`render_frame` consumer.
- Tests: `render_scene_separation.rs` (header D1–D4), `render_instanced.rs`
  (gate/allocation-assertion greps), `render_weather.rs`/`render_post.rs`
  (pin-value greps), the test-file inventory.

**COVERAGE GAP (recorded, not silently skipped):** `audits/renderer.md` is
referenced by `FINDINGS.md` (rows A03/A04/A12 cite it; the calibration record
describes its "structure map" and "shader duplication matrix") but the file
does NOT exist at HEAD — `git ls-files docs/engineering-excellence/audits/`
shows only core/persist/shell/slice, and `git log --all -- <path>` finds no
commit that ever touched it. The renderer audit's facts were therefore
re-derived from source in this session; every load-bearing claim below
carries its own direct citation instead of leaning on the missing artifact.
(Recommendation for the orchestrator: re-materialize or de-reference
`audits/renderer.md` in FINDINGS/AGENTS so the register doesn't cite a
phantom file.)

NOT examined (and why it doesn't block this design): full bodies of
`post_draw.rs`, `shadows.rs`, `reflection_draw.rs`, `sky_draw.rs`,
`shadow_draw.rs`, `corner.rs`, `textures.rs`, `populated_meshes.rs`,
`planar.rs`, `proxy_mesh.rs`/`lit_mesh.rs`, `overlay.rs`, `glyph_atlas.rs`,
`proc_texture.rs` (their structures, lifetimes and record-entry signatures
were read via greps and the cited call sites; P4's seams touch their edges,
not their interiors); the engine-slice demo modules beyond the dispatch and
pose-evaluation sites; full test bodies. No builds or test runs (read-only
mandate). §12 lists the consequences.

---

## 1. Evidence base — the architecture as it stands

### 1.1 The two entry families and the combinatorial seam (A03)

`Renderer` (renderer.rs:270–331) exposes TWO frame-submission families:

1. **The layers family** — `render_frame` (renderer.rs:537–552) delegates to
   `render_layers` (renderer.rs:562–618): `view_proj`, `world`/`foreground`
   scene vertices, `hud` overlay vertices, `debug_text`, `window_size`. This
   is the scene-independent path the non-corner demos already use
   (encounter_frame.rs:208–219; persist_demo.rs:3057; playable_demo.rs:1218;
   player_demo.rs:457; render_demo.rs:524; combat_demo.rs:1971). It carries
   NO lighting/material/population capability at all.
2. **The corner family** — four entry points, one per feature combination:
   `render_corner` (renderer.rs:621), `render_corner_populated` (:672),
   `render_corner_shadowed` (:780), `render_corner_populated_shadowed`
   (:868). Each re-implements, in near-identical text: the
   `DemoScenePreference::Corner` gate (e.g. :628–632, :680–684, :788–792,
   :877–881), the material-index validation against the **corner material
   table** (:685–698 verbatim at :882–895), `repack_populations` +
   pose-index validation + `repack_poses` (:699–730 verbatim at :896–927),
   the `ShadowFrameParams` construction from `LightParams`+`ShadowBias`+
   `lit_scene::intersection_boxes` bounds (:793–819 verbatim at :928–954),
   and the ~19-argument `vulkan::render_frame` call (:636–656, :745–765,
   :826–846, :972–992). The consumer dispatches over the same four
   combinations (corner_demo.rs:1008–1021 scripted, :1322–1335 free-look).

`config.rs:82–88` defines `DemoScenePreference { Omitted, Included, Corner }`
— construction-time, carried across every pipeline replacement
(renderer.rs:437–447, vulkan.rs:1995). The corner preference makes
`VulkanContext` upload `CornerResources` at construction (vulkan.rs:787–789)
— mesh, material table, per-slot uniform regions — so "Corner" is both a
feature flag and an authored-content import baked into the renderer.

Sky/post/wet/ripple/proxy-mode escaped the explosion by becoming **sticky
per-renderer settings** (`set_sky` :1016, `set_post` :1035, `set_wet`
:1058, `set_ripple_field` :1076, `set_ripple_debug_mode` :1086,
`set_proxy_draw_mode` :999) with lazily-created either-way state
(renderer.rs:296–330 documents the pattern). The population×shadow pair did
not get that treatment — it went combinatorial instead. The next feature
combination (e.g. populated × shadowed × a new kind) has no place to land
without another entry point: exactly A03's prediction.

### 1.2 The 19-parameter internal frame function (A04)

`vulkan::render_frame` (vulkan.rs:3983–4003) takes 19 parameters: the four
owning states (`ctx`, `swapchain`, `pipes`, `frames`), `window_size`,
`present_preference`, `replace_pipelines`, `demo_scene`, and the input
surface as one Option per layer family (`view_proj`, `region_vertices`,
`foreground`, `hud`, `debug_overlay`, `corner_params`, `proxy`, `shadowed`,
`sky`, `post: bool`, `wet`). Its ~910-line body (3983–4893) interleaves:
proxy-material validation (4021–4047), preparation via `prepare_frame`
(4052–4110, re-run on recreation), slot-fence wait + overlay slot growth
(4118–4126), acquire + out-of-date inline recreation (4128–4232), fence
reset + timestamp read (4234–4251), clear-color selection (4257–4263),
layer/overlay/proxy/pose uploads (4277–4349), viewport/depth-interval setup
(4351–4373), the pass recording cascade (shadow depth 4407–4421 → wet
mirror 4432–4452 → post/HDR-or-main 4463–4674 → shadow readback 4689–4697
→ timestamps 4698–4705 → capture readback 4706–4829), submit (4830–4852),
present (4854–4886). The code itself defends the flat parameter list
(vulkan.rs:3973–3975: "bundling them would hide the ownership story for no
gain") — a correct instinct about the **four owning states** that this
design preserves (§3.4); the defect is the other 15 parameters: the
per-layer Options are the same combinatorial surface as the entry points,
threaded through three more functions (`prepare_frame`,
`prepare_after_recreation` — 16 parameters each, frame_prepare.rs:79–96,
254–271 — and `PostFrameInputs`, post_draw.rs:317/943 carry the same
bundle again).

### 1.3 The pass inventory and ordering (B14's half)

One corner frame can run up to seven passes in a fixed order:
shadow depth (`shadow_state.record_depth_pass`, vulkan.rs:4411) → wet
mirrored-scene (`record_reflection_pass` + `record_read_barrier`,
vulkan.rs:4441–4451) → the scene pass (either the main render pass or the
post chain's HDR scene pass, vulkan.rs:4463–4482) → bloom mips + composite
(inside `post_state.record`) → overlay (4640–4661) → readbacks
(shadow 4689–4697, capture 4719–4829). Inside the scene pass,
`scene_record::record_corner_scene` (scene_record.rs:182–197) sequences:
sky (199–218) → corner FrameParams write (232–236) → the lighting branch.

**The variant-ordering duplication (B14's named half):** the shadowed arm
(scene_record.rs:245–424) and the ordinary arm (:426–590) each carry their
own `record_ground` closure (:265–285 vs :447–467 — textually identical
finding-14 ordered-blend logic) and their own per-kind proxy bind setup
(:319–417 vs :501–584 — the same SkinnedSetBind/PlainSetBind/
ProxyPipelines construction differing only in WHERE the npc kind's bind
comes from: the shadowed-proxy state's extended-FrameParams stride vs the
corner's plain stride). The HDR arm inside `post_draw` passes the same
sequence its own pipeline bundle (SceneTargetPipelines' doc,
scene_record.rs:53–122, documents the main/HDR dual guards). Consolidating
semantic ordering without erasing specialized pipelines is charter text
(DIRECTIVE §6 B14).

**The preparation triplication:** the post/wet extent-target ensure block
(frame_prepare.rs:212–232) exists three times — in `prepare_frame`, in its
`Recreated` arm re-run path (via the double `ensure_frame_targets_and_resources`
calls :137–178), and in `prepare_after_recreation` (:335–361, verbatim
copies plus the capture re-ensure).

### 1.4 The lazy-ensure dispatcher (the implicit "explicit uses")

`PipelineState::ensure_frame_targets_and_resources` (vulkan.rs:2303–2463)
is the de-facto pass-requirements resolver: each `Option`/flag in the frame
input drives an idempotent `ensure_*` (region capacity :2320, proxy :2333,
skinned :2341, indirect :2342, shadows :2354, shadowed-proxy :2362, sky
:2370, post :2382 with its four family-growth HDR variants :2384–2449,
wet :2457). The presence-driven discipline is sound (lazy, either-way,
flat allocation counters across replacement); what it lacks is a **typed
declaration of uses** — the requirements are re-derived implicitly from
parameter Options at four call sites, and the layered family-growth
conditions (`shadowed && proxy && skinned.is_some()`, `post && proxy`) are
inline boolean ladders that must stay in lockstep with the recording
cascade's guards (the belt-checks in scene_record.rs:290–307 exist precisely
because they can drift).

### 1.5 Resource lifetimes and the SkinnedRetained precedent (A14)

Three lifetime tiers already exist, each with a documented discipline:

1. **Device lifetime** — `pipeline_cache` (vulkan.rs:357–363) and
   `skinned_retained` (:365–380): created once, inherited by every pipeline
   generation, released at context teardown in manifest order
   (`ensure_skinned_retained` :1015–1025; `VulkanContext::destroy`
   :1029–1061 — retained first, then corner, then descriptors/samplers/
   texture, messenger, cache, device, surface, instance).
2. **Generation (swapchain/pipeline-replacement) lifetime** —
   `PipelineState` (vulkan.rs:1887): pass-bound pipelines and lazy family
   states, replaced on format change or the `replace_pipelines` test seam
   (NEW-REND-02's refuted "every recreation" concern — replacement is
   format-gated, vulkan.rs:4959 per FINDINGS), rebuilt through
   `create_pipelines` with partial-failure ownership (2469–2485).
3. **Frame-slot lifetime** — `FrameState` (vulkan.rs:3711): per-slot
   command buffers, fences, image guards, overlay slots; writes gated by
   the slot-fence wait with the 240-pacing wedge fix documented
   (vulkan.rs:4112–4121) and the acquire-image guard ordering
   (:4218–4240).

Construction and swapchain recreation have cross-stage cleanup ladders
(`Renderer::new` renderer.rs:419–474; `recreate_swapchain` vulkan.rs:4939+)
and a probe/defer discipline for unavailable surfaces
(frame_prepare.rs:99–130, 272–294). `Renderer::drop` orders teardown
(renderer.rs:1330–1339). **The gap:** device-lost has NO distinct handling —
a grep for `DEVICE_LOST` across `crates/engine-renderer/src` returns zero
matches; `RendererError::vk` (vulkan.rs:272–274) flattens every `vk::Result`
into `RendererError::Vulkan(String)`, so a lost device is indistinguishable
from a transient error and there is no documented recovery boundary. A14's
"device-lost/resize lifecycle" is half-addressed (resize: excellent;
device-lost: absent). Disposition in §5.6.

### 1.6 Capacity constants (A11 — P4 half)

`MAX_INSTANCES = 4096` (proxy_instances.rs:33), `MAX_POSE_BLOCKS =
MAX_INSTANCES` (animation.rs:245), and `region_scene_capacity_vertices() =
((8 + 1000) * 36 + 512) * FRAMES_IN_FLIGHT` (scene.rs:319–321) are
demo-workload-derived globals: the region budget literally hard-codes "all
eight regions Full plus 1000 NPC proxy boxes" (its own comment) — the A05
population policy leaking into a renderer buffer size. The typed-refusal
discipline around them is excellent (proxy_instances.rs:258–276,
animation.rs:682–695, the F-03 truncation counters in
`RenderStats`/`FrameOutcome`, vulkan.rs:3943–3951) and must be preserved
verbatim. P4's half is the **contract seam** (capacity becomes a named,
per-renderer budget with the same typed refusal); the chunked/arena
residency mechanics stay P5 (charter places A11 in both; P5 owns "scalable
chunks/arena/residency", P4 owns "explicit resource budgets" visibility).

### 1.7 The shader duplication matrix (A12)

Five 1:1 variant pairs, measured with `git diff --no-index --stat` at the
cited SHA:

| Pair (plain → variant) | Plain lines | Variant lines | Diff |
|---|---|---|---|
| `instanced_proxy.wgsl` → `instanced_proxy_shadowed.wgsl` | 119 | 195 | +112/−33 |
| `instanced_proxy_skinned.wgsl` → `instanced_proxy_skinned_shadowed.wgsl` | 164 | 248 | +106/−19 |
| `lit_scene.wgsl` → `lit_scene_shadowed.wgsl` | — | — | +128/−29 |
| `shadow_depth.wgsl` → `shadow_depth_instanced.wgsl` | — | — | +20/−8 |
| `shadow_depth_instanced.wgsl` → `shadow_depth_skinned.wgsl` | — | — | +36/−8 |

The duplicated semantics: the lighting equation + `square_pow`/
`trailing_zeros` helpers + the base `FrameParams` struct
(instanced_proxy.wgsl:34–48, 84–127 vs the shadowed copies), and — across
all three `*_shadowed` receivers — the `tap_offset`/`shadow_factor` PCF
block (instanced_proxy_shadowed.wgsl:112–178, self-described as
"term-for-term" the CPU oracle `shadow_filter.rs::pcf_at_point`). The
shadowed variant extends `FrameParams` with `light_view_proj` +
`shadow_control` and adds the depth-map binding at 4; the skinned pair
adds the pose storage at binding 5 (build.rs:132–146 documents the binding
map). Host-side mirrors: `corner::FrameParams` (112 B plain block) vs
`shadow_draw::ShadowFrameParams` (192 B extended — the sizes named in
scene_record.rs:315–318's discarded-bind bug comment), each hand-kept in
step with its WGSL struct. Build path: `build.rs` compiles each `.wgsl`
through the **pinned naga =29.0.4** (Cargo.toml:35) with `wgsl-in`/
`spv-out`; naga 29's WGSL front end has **no include/preprocessor
facility**, so source sharing must be assembled in `build.rs` or not at
all. Rust-side draw modules mirror the variant split per family
(`SkinnedProxyResources`/`ShadowedProxyResources` — skinned_draw.rs:744,
:830 — each carrying its own layout/pool/sets; the shadowed state alone
holds six per-variant descriptor pools/layouts, skinned_draw.rs:830–860).

### 1.8 Animation placement (A06)

`animation.rs` is pure: "ALL pure, fully tested, NO Vulkan, NO new
dependencies" (animation.rs:1–6), its only import is
`crate::proxy_instances::sin_cos_turn` (:60), itself a pure fixed-order
Taylor helper. The crate-coupling finding stands: slice's pose-evaluation
policy imports `engine_renderer::animation` directly
(corner_routes.rs:618, corner_population.rs:242/283/566/608). Disposition
in §10 (A06 row).

### 1.9 The gate batteries (the equivalence bars this design must name)

- **Lavapipe offscreen batteries** (`#[ignore]`d, requiring
  `SDL_VIDEODRIVER=offscreen` + a software ICD): `render_corner`,
  `render_instanced` (the 6-E/6-E2 suite), `render_shadows`,
  `render_sky`, `render_post`, `render_reflection`, `render_weather`,
  `render_scene_separation`, `render_orientation`, `render_overflow`,
  `render_encounter` (test inventory, engine-slice/tests/).
- **Allocation-constancy counters**: `renderer.allocations()` warm-equality
  asserts throughout render_instanced.rs (:677–771, :1107–1609,
  :3063–3222, :3586–3683) — the lazily-created-either-way discipline's
  proof.
- **Rig-verified pixel pins** (E-7, EVIDENCE.md:109–121, RTX 5070 Ti,
  tree `1884582`): render_weather's 11 gates — rain ink 17041 differing
  bytes; broadside ink 3482 px / 2162 upper-third; allocation tuple
  (19/4/1589248/3/7/2) over 12 captures; ripple sweep 22/255; term
  separation 96548/67535/7625 and 27149/27149/2538; double-run
  byte-equality.
- **Known-answer readbacks**: `ShadowMapReadback` (renderer.rs:19–31,
  request/take :1132–1157) and the frame capture single-flight handshake
  (:1300–1319).
- **Separation gates**: render_scene_separation.rs D1–D4 (the opt-in
  boundary pixel-proven both directions, header :1–42).
- **SPIR-V orientation pins**: `vulkan.rs` mod `orientation` (:5732+) and
  `position_invariance` (:6026+) re-parse the emitted words through naga
  spv-in.
- **D-EE-08**: the lavapipe pod is currently un-rebuilt; the first P4
  stage gate that needs it triggers the rebuild (the recipe is on record).

### 1.10 What is already excellent and must be preserved

(charter §5: "Preserve existing good mechanisms" — the enumerated list,
verified present: instanced/indirect submission renderer.rs:87–105;
mapped per-frame GPU storage vulkan.rs:4277–4349; lazily-created resources
§1.4; statically specialized shaders §1.7 — one .spv per variant, zero
runtime branching; explicit synchronization §1.5; retained CPU repack
buffers renderer.rs:279–295; fail-visible capacity §1.6; validated-before-
frame-state discipline renderer.rs:668–730 + vulkan.rs:4013–4047; the
capture-refusal never-poisons-sync rule frame_prepare.rs:182–195.)

---

## 2. Goals and non-goals

### 2.1 Goals (each traces to charter §7-P4 text; full trace in §13)

G1 A small, typed, scene-independent **frame submission contract**
(cameras, visible instances/resources, material/light/effect/pass
requirements, output) that replaces the four `render_corner*` entry points
and the 15-parameter input half of `vulkan::render_frame`.
G2 A thin **pass coordinator** with explicit uses and lifetimes — no
needless allocation, no runtime dispatch — consolidating the
`scene_record` variant ordering and the preparation triplication.
G3 A **Vulkan responsibility split** into context/capabilities, resource
allocation/retirement, swapchain/presentation, frame
synchronization/commands, uploads, pass resources, capture — building on
`SkinnedRetained`/`pipeline_cache`/`frame_prepare`/`scene_record`
precedents, NOT a generic context object.
G4 A **shader source of truth** for the lit/shadow variant pairs via
build-time composition (no runtime branch penalty) + host/shader layout
invariants with a checked single source.
G5 A **staged migration DAG** — compatibility adapters first, old-path
removal last, every stage independently gated by the byte/pixel-equality
bars of §1.9.
G6 **A second non-corner scene consumer** using the same public API with
zero renderer-internals edits (the charter's demonstration requirement).
G7 Demo scene authoring stays in engine-slice; world-facing extraction
separated from backend data; frame lifetime separated from resource
lifetime; capture/diagnostics separated from production submission.

### 2.2 Non-goals (charter-mandated and scope-discipline)

- **No fidelity reduction**: every current effect (sky, PCF shadows, HDR
  bloom/tonemap, wet planar reflection + ripple displacement, rain
  blending, skinning) survives with identical output. No effect is dropped
  "to speed up" anything (charter P4 last sentences; §0 ambition clause).
- **No generic framework**: no render graph, no barrier solver, no
  trait-object/`dyn Pass` abstraction, no ECS-ification of populations, no
  plugin registry, no second graphics backend (lib.rs:30–32 already
  excludes it). The coordinator is data + direct calls (§4).
- **Preserved direct Vulkan control**: ash stays; all unsafe stays inside
  the private `vulkan` tree (lib.rs:22–30 unsafe containment policy);
  no wrapper framework between the engine and Vulkan.
- **Preserved static shader specialization**: variant count does not grow
  or shrink; specialization stays at build time (§6).
- **No new dependencies**: no shaderc/glslang/shader-preprocessor crate;
  naga =29.0.4 stays the only shader tool (B15: the dep closure is already
  tight — ash/bytemuck/nalgebra/engine-core).
- **No P5 work**: residency budgets, dirty-range uploads, staging pools
  (A20), pose-capacity mechanics (A23), reflection-target VRAM measurement
  (B14's other half), GPU-driven visibility — all stay P5. P4 only
  reserves clean seams for them (§5.2, §10).
- **No device-lost hot recovery**: P4 adds the typed error +
  documented boundary (§5.6); building automatic device re-initialization
  is out of scope and would be speculative.
- **No public-API stability commitment**: the contract is versioned
  within the program; the stability policy is P6's deliverable.
- **No animation crate extraction yet** (A06 — §10 disposition).

---

## 3. D1 — The scene-independent submission contract

### 3.1 The shape

One public frame entry point carrying a typed submission; construction
policy stays construction policy; per-frame inputs become data:

```rust
/// Everything one frame needs, scene-independent. Validated whole,
/// BEFORE any frame state is touched (the capture-refusal discipline).
pub struct FrameSubmission<'a> {
    pub camera: FrameCamera,              // view_proj + eye (the one main camera)
    pub output: OutputTarget,             // window pixel size (app-owned state)
    pub layers: DynamicLayers<'a>,        // world/foreground/hud/debug-text batches
    pub scene: Option<SceneFrame<'a>>,    // this frame's retained-scene requirements
}

pub struct FrameCamera { pub view_proj: [[f32; 4]; 4], pub eye: [f32; 3] }

pub struct DynamicLayers<'a> {
    pub world: &'a [SceneVertex],                 // the render_layers path, unchanged
    pub foreground: Option<SceneLayer<'a>>,        // independent projection (kept)
    pub hud: &'a [OverlayVertex],
    pub debug_text: Option<&str>,
}

/// What the attached scene contributes THIS FRAME — the pass-requirements
/// declaration. Each field names a pass and its data; None = the pass
/// does not run. No method-per-combination can exist.
pub struct SceneFrame<'a> {
    pub light: LightParams,                       // shared by lit/shadowed/wet
    pub shadows: Option<ShadowFrame>,             // bias + scene bounds (cascade computed in-renderer)
    pub population: Option<ProxyPopulation<'a>>,  // the existing typed struct, verbatim
    pub sky: Option<SkyFrame>,                    // per-frame (was sticky)
    pub post: bool,                               // per-frame (was sticky)
    pub wet: Option<WetFrame<'a>>,                // ripple field (was sticky-packed)
}

pub struct ShadowFrame { pub bias: ShadowBias, pub bounds: SceneBounds }
pub struct WetFrame<'a> { pub ripple: Option<&'a RippleField> }  // camera/light derived from the frame
```

`Renderer::submit_frame(&mut self, submission: FrameSubmission<'_>) ->
Result<FrameResult, RendererError>` — the single production entry point.
The layers family folds in as `scene: None` + populated `layers`
(encounter_frame's shape, §1.1); the corner family folds in as
`Some(SceneFrame)` with the population/shadow fields set. **Camera(s)
plural** is satisfied by the main camera + the foreground layer's own
projection — the existing two-camera shape (renderer.rs:554–561), no
speculative multi-viewport.

### 3.2 The scene handle (feature selection from authored geometry)

The deepest separation. Today "Corner" means: `CornerResources` uploaded at
construction (vulkan.rs:787–789), the material table
(`lit_scene::materials()` — renderer-authored content, renderer.rs:685),
shadow scene bounds from `lit_scene::intersection_boxes()` (:793–794), the
wet ground receiver range (`corner.receiver_range()`,
scene_record.rs:281) and reflected ranges (`corner.reflected_ranges()`,
vulkan.rs:4449) — authored geometry baked into renderer decisions.

The contract replaces it with an **attach-time scene description**:

```rust
/// Immutable, attach-once scene content. Built by ANY consumer (the
/// corner demo, the second scene, a future game). Upload happens once,
/// through the existing CornerResources machinery generalized.
pub struct SceneDescription {
    pub mesh: SceneMesh,                // vertices + indices + per-material draw ranges
    pub materials: Vec<MaterialParams>, // the material table (specular/shininess per set)
    pub textures: ProcTextureSet,       // procedural albedo set (proc_texture.rs types)
    pub bounds: SceneBounds,            // world AABB for the shadow cascade
    pub ground: GroundRoles,            // receiver_range + reflected_ranges (wet feature data)
}

impl Renderer {
    pub fn attach_scene(&mut self, scene: SceneDescription)
        -> Result<SceneHandle, RendererError>;   // idempotent-lazy under the hood
}
```

- Material validation reads the **attached scene's** table length —
  `render_corner_populated`'s corner-table check (renderer.rs:685–698)
  becomes `population material >= scene.materials.len()` against the
  handle. Same typed refusal, same error text shape, no hardcoded table.
- The shadow cascade keeps `shadows::cascade_for` (deterministic core,
  renderer-side) but consumes `SceneFrame::shadows.bounds` supplied by the
  scene author — the corner adapter passes its intersection-boxes bounds,
  now authored in slice.
- The wet reflection's ground re-draw consumes the scene's
  `GroundRoles` ranges instead of `corner.receiver_range()` — the wet
  feature becomes data-driven over role-tagged mesh ranges instead of
  corner-coupled (this is the full "feature selection from authored
  corner geometry" separation; scene_record.rs:257–294's placement logic is
  unchanged, only its range source moves).
- `DemoScenePreference::Corner`'s construction-time upload becomes the
  adapter: `Renderer::new` keeps accepting the preference, and the
  preference constructs the corner `SceneDescription` internally (content
  still in renderer during migration, moved out at S2/S6 — §8).
- `DemoScenePreference::Included` (the static Stage-2 scene,
  vulkan.rs:2051–2068) survives unchanged: it is a debug content opt-in
  orthogonal to the attach API (it draws through the scene pipeline, not
  the lit family). Its disposition at S6 is documented removal candidate
  only if the second consumer subsumes it — default: keep (non-goal to
  remove functionality).

### 3.3 Validation ordering — the contract invariant

The submission is validated **whole, before any frame state is touched**,
preserving exactly the current discipline and order
(renderer.rs:668–730; vulkan.rs:4013–4047 "a refusal here can never leave a
fence reset-but-unsignaled or an acquired image's semaphore unconsumed"):

1. scene-preference/attach coherence (a `SceneFrame` without an attached
   scene is a typed refusal — the `render_corner` preference gate's
   generalization, renderer.rs:628–632);
2. material indices vs the attached table (empty-kind-any-index rule kept,
   vulkan.rs:4026–4029);
3. `repack_populations` + pose-index-vs-blocks + `repack_poses`
   (renderer.rs:699–730, the ProxyPack/PosePack typed refusals);
4. only then `vulkan::render_frame`.

The contract document states this as THE invariant: *a refused submission
renders nothing, touches no synchronization state, and leaves every sticky
setting and capture request intact.*

### 3.4 The internal collapse (A04) — ownership story preserved

`vulkan::render_frame`'s four owning states stay four explicit `&mut`
parameters (honoring vulkan.rs:3973–3975's correct ownership argument —
this design disagrees only about the input half). The 15 input/policy
parameters collapse to two:

```rust
pub(crate) struct FramePolicy {            // construction-time config, copied per call
    present_preference: PresentModePreference,
    replace_pipelines: bool,
    scene_attached: bool,                  // replaces demo_scene in the internal flow
}

pub(crate) struct InternalFrame<'a> {      // the validated submission, one level down
    submission: &'a FrameSubmission<'a>,   // verbatim
    scene: Option<&'a SceneResources>,     // the attached handle's state (was ctx.corner)
    ripple_debug: RippleDebugMode,         // the one retained test-only sticky
}

pub(crate) fn render_frame(
    ctx: &mut VulkanContext, swapchain: &mut SwapchainState,
    pipes: &mut PipelineState, frames: &mut FrameState,
    policy: FramePolicy, frame: &InternalFrame<'_>,
) -> Result<FrameOutcome, RendererError>;   // 19 → 6 parameters
```

`prepare_frame`/`prepare_after_recreation` take `(states…, policy, frame)`
similarly (16 → 6). No behavior change: the same Options flow through, now
named by one struct instead of positional parameters. The sticky sky/post/
wet/ripple state moves OUT of `Renderer` fields into the submission (the
renderer retains only `ripple_debug` — test-only — and `proxy_draw_mode`,
which is submission policy and moves into `ProxyPopulation`'s mode field
via the existing `ProxyFrameUpload.mode` plumbing, vulkan.rs:732–742).

**Zero-cost statement (charter §4 "determine which"):** `FrameSubmission`
is a borrow-only struct (no Vec, no Box — `ProxyPopulation<'a>` and
`SceneLayer<'a>` are already `Copy` borrow bundles, renderer.rs:36–39,
59–85); `FramePolicy` is `Copy`; construction is stack assembly the
compiler can and does elide entirely — the adapters (§3.5) prove it by
producing identical `FrameOutcome`s and identical allocation counters on
the existing batteries. No dyn dispatch, no per-frame heap allocation
introduced anywhere in this design.

### 3.5 The compatibility adapters (temporary by charter)

During migration each `render_corner*` method becomes a ~15-line adapter:
gate check → build `FrameSubmission` from the same arguments + the
renderer's sticky fields → `submit_frame`. The demo's `set_sky`-once-then-
render loop (corner_demo.rs) keeps working because the adapter copies the
sticky field into the submission per frame. Byte-identity gate: the
lavapipe batteries + E-7 rig pins reproduce digit-for-digit through the
adapters (the frame content is bit-identical by construction — same
repack, same uniforms, same command stream). Removal at S6 (§8).

### 3.6 Capacity and refusal semantics (A11 P4 half)

The submission carries populations bounded by the existing typed refusals;
P4 adds only **naming and observability**: `RenderStats` gains
`instance_capacity` / `pose_capacity` fields (reading the existing
constants) so the budget is a reported contract, not a hidden global; the
budget's VALUE stays fixed at 4096 (changing it is P5's chunked-residency
work — non-goal here). `region_scene_capacity_vertices`' demo arithmetic
(scene.rs:313–321) gets a doc block naming it the P5 replacement seam (the
NEW-REND-05 margin note rides along).

---

## 4. D2 — The pass coordinator

### 4.1 Why a thin coordinator, not a graph (the justified negative)

A render graph would buy: automatic barrier derivation, pass culling,
resource aliasing. The codebase has none of those problems: the pass set
is ≤7, the order is fixed by documented blend/dependency semantics
(scene_record.rs:19–38 finding-14; vulkan.rs:4402–4431 the shadow→scene
subpass dependency comment; the wet barrier 4443–4452), barriers are
hand-proven under sync-validation (renderer.rs:204–215 — every
`validation_errors == 0` gate proves execution/memory dependencies), and
B14's aliasing question is explicitly deferred to measurement (P5). A
graph would add: an allocation per frame (or a pool), dynamic pass
dispatch, a barrier solver to test, and a second way to express ordering
that can drift from the documented one. **Rejected** — the charter's
"thin pass coordinator (or justified graph) … without needless
allocations or runtime dispatch" selects the coordinator, and the
justification is the above.

### 4.2 The FramePlan

Built once per frame from the validated submission, on the stack, `Copy`:

```rust
pub(crate) struct FramePlan {
    pub uses: PassUses,        // one bool/Option per pass — THE explicit-uses declaration
    pub slot: usize,           // frame slot (owned by FrameState)
    pub lit_variant: LitVariant,   // Ordinary | Shadowed — selects the bind family
    pub ground: GroundRedrawPass,  // None | Main | Hdr (the finding-14 enum, verbatim)
}
```

`PassUses` is the typed mirror of today's implicit Option-ladder
(§1.4): `{shadow_depth, wet_mirror, scene_pass: Main|Hdr, bloom, overlay,
shadow_readback, capture}` with their data references. It is constructed
in one place (`FramePlan::from(&submission, &sticky, &frames)`) and
consumed by:

1. **`ensure_for(&plan)`** — replaces
   `ensure_frame_targets_and_resources`'s boolean ladders
   (vulkan.rs:2303–2463) with a single match on `uses`: the same
   idempotent ensures, the same lazy/either-way/flat-counter discipline,
   the same family-growth conditions — now written once against the plan
   instead of re-derived from positional Options at three call sites. The
   belt-checks in scene_record stay (they guard the lazy first-frame
   window and replacement frames — scene_record.rs:60–83 — and the plan
   does not remove that window).
2. **The recording cascade** — the existing fixed order
   (§1.3) driven by `uses` fields instead of parameter Options: each pass
   still records through its own statically-typed module function
   (`record_depth_pass`, `record_reflection_pass`, `post_state.record`,
   `record_corner_scene`, overlay block) — **no trait objects, no Box, no
   dyn**. The coordinator is a match over a Copy struct; dispatch is
   static.

### 4.3 The scene_record consolidation (B14's half, concrete)

Extract the two arms' duplicated sequencing into one function
parameterized by the ONLY real variation — where the npc kind's bind comes
from:

```rust
/// The per-kind bind family for one lighting variant. The ordinary and
/// shadowed arms construct one of these; the sequencing below is shared.
pub(crate) enum LitBind {
    Ordinary { skinned: SkinnedSetBind, vehicle: PlainSetBind, rain: PlainSetBind },
    Shadowed { skinned: SkinnedSetBind, vehicle: PlainSetBind, rain: PlainSetBind },
}
```

- `record_proxy_sequence(ctx, cmd, slot, upload, bind, pipelines, ground)`:
  opaque kinds → `record_ground` → rain kind (scene_record.rs:394–417's
  ordered sequence, written once). The two `record_ground` closures
  (:265–285, :447–467) become one; the bind construction
  (:319–341 vs :501–523) stays per-variant (it is the genuine difference —
  the shadowed family's stride/extended-FrameParams discipline,
  scene_record.rs:308–318's discarded-bind warning).
- The sky block, corner FrameParams write, and the lighting-variant mesh
  draw (:199–256) are shared as today.
- Net: ~120 duplicated lines per arm collapse to one sequencing function
  + two bind constructors; the specialized pipelines and per-variant
  descriptor sets are untouched (charter: "without erasing specialized
  GPU pipelines").

### 4.4 The preparation consolidation

`ensure_extent_targets(ctx, pipes, swapchain, uses)` extracted from the
three verbatim/near-verbatim sites (frame_prepare.rs:212–232, the
Recreated arm, :335–361) — one target-ensure path called by
`prepare_frame` and `prepare_after_recreation`; the capture ensure
(:192–195, :358–361) folds into it. The probe/defer/recreate state machine
(frame_prepare.rs:99–130, 272–294) is NOT touched — it is correct,
rig-proven (the 1×1 fabrication corrective, the fence-preservation
review finding), and out of the duplication's scope.

### 4.5 Frame lifetime vs resource lifetime (the separation made explicit)

The coordinator names the three tiers as its ownership taxonomy (§1.5's
existing de-facto tiers, now documented as THE model):
device-lifetime (cache, retained pose/skinned foundation, attached scene
resources), generation-lifetime (pass-bound pipelines + family states),
frame-slot (command buffers, fences, mapped slot regions). Every ensure in
§4.2(1) is annotated with which tier it creates; every record pass in
(2) with which tiers it reads. This is documentation + type organization,
not new machinery — the tiers already exist and work
(`SkinnedRetained`'s doc, vulkan.rs:365–380, is the precedent text).

---

## 5. D3 — The Vulkan responsibility split

### 5.1 The module map

`vulkan.rs` (6,158 lines) becomes a module TREE under `src/vulkan/`,
continuing the proven `frame_prepare`/`scene_record` extraction pattern
(finding 20 Phases A/B — both landed cleanly with gates green). `vulkan.rs`
shrinks to the mod file + the shared low-level helpers. Target layout
(each module's current-code anchor):

| Module (new) | Responsibility (charter's list) | Content moved (current anchor) |
|---|---|---|
| `vulkan/context.rs` | context / capabilities | `VulkanContext` struct + construction + teardown manifest (vulkan.rs:333–436, 439–1061); extension plan (1064–1130); device/format/present selection (`pick_surface_format` :1388, `pick_depth_format` :1819, `resolve_*` config.rs:184–227) |
| `vulkan/resources.rs` | resource allocation / retirement | allocation counters + target-counter cells (:384–435, :1000–1007); `create_device_local_buffer_with_data`, `create_mapped_buffer`, memory-type lookup (the static-upload staging path :3035–3199 — A20's seam, moved not redesigned); the fault-injection cells (:404–419) |
| `vulkan/swapchain.rs` | swapchain / presentation | `SwapchainState` (:1351–1695); `recreate_swapchain` (:4939+); `drawable_extent`/`surface_probe_decision`/`consume_recreate`/`mark_suboptimal` (frame_prepare.rs consumers) |
| `vulkan/frame.rs` | frame synchronization / commands | `FrameState` (:3711–3930); the wait→acquire→record→submit→present flow (render_frame's body :4112–4893) — the coordinator's host half |
| `vulkan/uploads.rs` | uploads | `upload_layers` + the mapped-slot write discipline (:4277–4349); overlay slot reserve (:4124–4126); proxy/pose `write_slot` call sites (proxy_draw.rs/skinned_draw.rs keep their own buffers — uploads.rs owns the discipline documentation + the layer clamp, config.rs:253–271) |
| `vulkan/passes/*` | pass resources (per family) | `corner.rs` (→ generalizes to scene resources per §3.2), `shadow_draw.rs`, `sky_draw.rs`, `post_draw.rs`, `reflection_draw.rs`, `skinned_draw.rs`, `proxy_draw.rs`, `scene_record.rs`, `sky.rs`/`post.rs`/`shadows.rs`/`shadow_filter.rs` pure cores stay at `src/` |
| `vulkan/capture.rs` | capture / diagnostics | the capture + shadow-readback handshake (`frames.capture`, `frames.shadow_readback` state + `request/take` functions, renderer.rs:1300–1319, 1132–1157; vulkan.rs:4706–4829 record-side); validation drain plumbing (:167–172) |

### 5.2 What does NOT happen (the anti-god-object rules)

- `VulkanContext` **does not grow**: it keeps exactly its current fields
  (device/instance/queues/capabilities/counters + the two retained
  pass-independent states). It gains no pass state, no swapchain state, no
  frame state — the four-state ownership (ctx/swapchain/pipes/frames)
  stays four separate `&mut` parameters through every internal function
  (§3.4). The split moves code OUT of the monolith into sibling modules,
  not INTO a central object.
- No `RendererContext`/`Engine` aggregate, no service locator, no trait
  unification of the pass-resource states: each family keeps its own
  struct with its own ensure/release (their shared discipline —
  idempotent ensure, partial-failure-safe release, release-exactly-once —
  is documented ONCE in `vulkan/passes/mod.rs` as the pattern contract
  each module cites, mirroring how `SkinnedRetained` documents its own
  lifetime, vulkan.rs:365–380).
- The unsafe containment policy is unchanged: everything stays inside the
  private `vulkan` tree (lib.rs:22–30); module reshuffling does not move
  unsafe outside it.
- P5 seams reserved, not implemented: `resources.rs` documents (doc-block
  only) where the staging pool / dirty-range upload / residency budget
  will attach — no speculative code.

### 5.3 Mechanical rules for the split lanes (D-EE-09 discipline)

Pure-move commits only: each lane moves named symbol ranges to named
files, `use` fixes, zero logic edits, zero comment rewrites. Gate per
lane: fmt + clippy + the crate's unit tests (`cargo test -p engine-renderer`,
the 172/173-pin battery of E-6) + one lavapipe battery re-run at batch
close. Any behavior drift = lane rejected (the A24 calibration lesson).

### 5.4 The frame flow's phases made explicit (A04's "explicit phases")

`vulkan/frame.rs` names the phases as functions in call order (each is
today's comment-delineated block, extracted verbatim):
`validate_inputs` (4021–4047) → `prepare` (§4.4) → `wait_slot_fence`
(4118–4126) → `acquire_with_retry` (4128–4232) → `reset_and_timestamps`
(4234–4251) → `upload_frame_data` (§5.1 uploads) → `record` (the cascade,
§4.2(2)) → `submit` (4830–4852) → `present` (4854–4886). The phases are
private functions of the frame module — not a phase trait, not extensible
at runtime; the ordering is the code.

### 5.5 Capture/diagnostics separation (charter: "capture/diagnostics from
normal production submission")

The submission contract carries ZERO diagnostic state (verified shape:
`FrameSubmission` has no capture/fault/debug fields — §3.1). Capture
stays request-side (`request_frame_capture`/`take`/
`request_shadow_map_capture`/`take` — the single-flight handshake,
renderer.rs:1124–1157, 1292–1319); fault injection stays
`inject_target_fault`/`inject_acquire_out_of_date` (:1249–1278);
`ripple_debug` remains the single retained test-only sticky, documented
test-only as today (renderer.rs:326–330). `vulkan/capture.rs` owns the
handshake state machine so the production frame path (`frame.rs`) records
the readback passes only when the plan's `uses.capture/shadow_readback`
say so — the current guard shape (vulkan.rs:4689–4697, 4719–4829),
relocated not redesigned.

### 5.6 Device-lost disposition (A14's open half — honest gap handling)

Add `RendererError::DeviceLost(String)` as a typed arm (detected at the
`vk()` helper for `ERROR_DEVICE_LOST`/`ERROR_DEVICE_LOST_KHR` and at
`wait_for_fences` timeout differentiation — currently flattened,
vulkan.rs:272–274, 4118–4121). Policy: **fail-visible, no auto-recovery**
— the error surfaces; the application may drop the renderer and construct
a fresh one (the construction ladder already supports clean teardown,
renderer.rs:419–474). Document the boundary in the contract: which states
die with the device (everything device-lifetime), what the application
must re-supply (scene descriptions, sticky-equivalent submission fields).
Full hot-swap/in-place recovery is a non-goal (§2.2). This closes A14's
"device-lost" sentence at the contract level without inventing untested
recovery machinery.

---

## 6. D4 — The shader source of truth (A12)

### 6.1 The constraint that decides the mechanism

naga =29.0.4's `wgsl-in` has no include/preprocessor facility (verified:
the WGSL path is `parse_str` on one source string, build.rs:108–156; the
pinned-version rationale, Cargo.toml:31–35; upgrading naga to gain
includes is a toolchain change with its own risk profile — non-goal).
Therefore source sharing happens **in build.rs at text level**, or via
post-parse verification only.

### 6.2 Options considered

- **(a) Build-time composition** — shared core fragments +
  per-variant shells, assembled by build.rs into the same six `.spv`
  outputs (names unchanged so pipeline assembly is untouched).
- **(b) Verification-only** — keep the separate files; a test parses all
  variants through naga and asserts the shared semantic blocks are
  structurally identical (equation, helpers, struct layouts).
- **(c) naga module-level composition** — parse core once, splice at the
  IR level. Fragile against naga's API surface, high effort, no precedent
  in-tree. **Rejected.**

**(a) is the design, with (b) as its teeth.** (b) alone leaves the
duplication (A12 unfixed — the charter requires a shared source of truth,
not a drift detector); (a) alone could silently diverge from the host
mirrors. Together: one source text per shared semantic + generated
variants + a layout pin that cross-checks host and shader.

### 6.3 The composition scheme

```
shaders/
  core/
    lighting.wgsl        // FrameParams base struct, square_pow, trailing_zeros,
                         // lit_color(...) — the oracle-mirrored equation
    pcf.wgsl             // tap_offset + shadow_factor — the three shadowed
                         // receivers' shared block (oracle: shadow_filter.rs)
    skinning.wgsl        // the skin equation + pose-storage declarations
                         // (binding 5), shared by the two skinned pairs
  variants/
    lit_scene.wgsl            // shell: bindings + vs_main/fs_main calling lit_color
    lit_scene_shadowed.wgsl   // shell: + map binding 4, extends FrameParams,
                              //   fs_main multiplies by pcf's shadow_factor
    instanced_proxy*.wgsl     // four shells (instancing input §, tint composition)
    shadow_depth*.wgsl        // three shells (depth-only; instanced/skinned input)
```

`build.rs::compose(shell, cores)` concatenates at explicit
`// @core:<name>` anchor comments (string splice, no templating language,
no new deps), then runs the existing parse→validate→emit path
(build.rs:115–148) per composed source, emitting the SAME out_names the
pipeline assembly includes today. The binding-map stays (132–146).
Rationale for anchors over a macro language: the composed text is
inspectable (emit it to OUT_DIR for debugging — a `SHADER_DEBUG=1`
env dump), greppable, and diffable; the splice is 30 lines of std Rust.

**No runtime branch penalty, no variant-count change**: the emitted
module set is exactly today's ten WGSL targets (build.rs:55–102); each
pipeline still binds one specialized .spv; specialization remains static
(charter requirement, and lib.rs's "statically specialized shaders"
preserve-list entry).

### 6.4 Host/shader layout invariants — the single checked source

Today the FrameParams struct exists four-plus times: WGSL in each variant
(instanced_proxy.wgsl:34–39 and its shadowed extension) and Rust in
`corner.rs`/`shadow_draw.rs` — hand-kept in step (the 112/192-byte sizes
are only documented in a bug-comment, scene_record.rs:315–318). The
design makes the WGSL struct the canonical text and pins the Rust side to
it mechanically:

- A build script (or unit test with naga as dev-dep — the spv-in dev
  dependency precedent exists, Cargo.toml:39–40) parses each composed
  variant, resolves each struct's member offsets/sizes through naga's
  layout, and emits `layout_pins.rs` containing `const` assertions.
- The Rust mirrors (`corner::FrameParams`, `ShadowFrameParams`,
  `RippleUniformData`, `MaterialParams`) are checked against those pins:
  `size_of`/`offset_of!` equality with the WGSL-derived constants (Rust
  ≥1.77 has `offset_of!`; the repo's MSRV supports it — toolchain pinned
  per workspace).
- Any mismatch is a compile-time break on the offending side — the
  "clear source of truth" the charter asks for, at zero runtime cost.

### 6.5 Equivalence gates for shader work (S5)

SPIR-V byte-equality is **deliberately NOT the bar**: composed sources
re-emit modules whose instruction streams may differ in ordering while
being semantically identical; byte-pinning would freeze naga's codegen
and block toolchain movement (the orientation pin itself was rewritten
once to be representation-robust — vulkan.rs:5753–5763 documents exactly
this lesson). The bar is:
1. naga validation passes for every composed variant (build fails
   otherwise — same as today);
2. the lavapipe batteries reproduce their pinned ink/census values
   digit-for-digit (render_instanced/render_shadows/render_post/
   render_reflection/render_weather — §1.9);
3. the `ShadowMapReadback` known-answer depths unchanged (the 6-F path,
   renderer.rs:19–31);
4. the orientation + position-invariance pins green (they operate on
   emitted words' dataflow, robust to reordering — vulkan.rs:5732+);
5. the layout pins of §6.4 green;
6. rig re-verification of E-7's render_weather values on the owner's
   5070 Ti at stage close (the A24 discipline: lavapipe proves logic,
   the rig proves the physical GPU).

---

## 7. D6 — The second non-corner scene consumer (the charter's proof)

**Requirement (charter §7-P4, verbatim):** "Demonstrate a second
non-corner scene using same public renderer API without editing renderer
internals."

**The scene:** `engine-slice`'s `field_scene.rs` (name TBD) — an authored
dusk meadow: a rolling ground plate + scattered rock masses (its own
`SceneMesh` with per-material ranges), 2–3 own materials over
`proc_texture` albedos, one directional dusk light, sky on, post chain on,
a small skinned population (10–40 walkers through the SAME
`ProxyPopulation` + pose-block contract), shadows on, no wet reflection
(wet's ground-role generalization is exercised by a unit-level
`SceneDescription` fixture instead — its proof is the range-plumbing, not
a full second wet demo). NO intersection geometry, NO corner route, NO
corner material table.

**The proof (three legs):**
1. **The diff is the proof**: the lane's diff touches
   `engine-slice/**` + tests ONLY — `git diff --stat` shows zero
   `engine-renderer` hunks. This is checkable mechanically at review.
2. **The gate is the teeth**: the demo pins its own capture census
   (pixel-band counts at fixed camera presets, the ink-discipline
   pattern), its allocation-constancy tuple (the render_instanced
   pattern, §1.9), and a double-run byte-equality assert (the A24
   pattern) — a fresh battery `render_field_scene.rs` alongside the
   existing ones.
3. **The modularity rubric cites it**: charter §9.2's "second non-corner
   renderer/application scenario without changing engine internals" is
   evidenced by legs 1+2.

This consumer is also the **regression guard for the contract**: any
future renderer-internal change that re-couples the renderer to corner
content breaks the field scene's build (it has no corner types) — the
separation becomes executable, not aspirational.

---

## 8. D5 — The staged migration DAG

Notation per stage: content → gates (the equivalence bar, named) →
removal condition. Stages are independently reviewable commits/series;
adapters are temporary BY CHARTER ("Migrate demo through compatibility
adapters only temporarily, then remove duplicated old paths when safe").

```
S1 contract+adapters ──► S2 scene extraction ──► S3 pass coordinator ──► S4 vulkan split ──► S6 removal+second scene
        │                                                                        ▲
        └────────────────────────────► S5 shader composition (independent) ──────┘
                                     (may land any time after S1; needs no contract)
```

Serialization rationale: S1 fixes the public shape everything else
adapts to; S2 changes what the submission carries (scene handle) so it
precedes S3's plan (the plan reads the handle); S3 settles the frame flow
before S4 moves it (moving unsettled code = churn + review noise);
S5 is textually independent (shaders/build.rs only) but is sequenced
after S1 so its battery re-run covers the adapter path too; S6 is last
by definition (it deletes the adapters S1–S5 are verified through).

### S1 — The contract skeleton + adapters (orchestrator)

- Add `FrameSubmission`/`submit_frame` (§3.1, §3.3, §3.4); the four
  `render_corner*` methods + `render_layers`/`render_frame` become
  adapters (§3.5); the sticky sky/post/wet/ripple fields keep feeding the
  adapters; `vulkan::render_frame` internal signature collapses to the
  6-parameter form (pure plumbing — the Options thread through the
  struct).
- **Gates:** the full lavapipe battery set re-run through the adapters
  (render_corner, render_instanced, render_shadows, render_sky,
  render_post, render_reflection, render_scene_separation,
  render_orientation, render_overflow, render_encounter) — pinned census
  values digit-for-digit, allocation-constancy tuples unchanged
  (render_instanced.rs:677–771 pattern); render_weather rig re-run
  (E-7 values, the owner's GPU) at batch close; D-EE-08 pod rebuild
  triggered by this stage's first battery need; fmt/clippy/full workspace
  gates per D-EE-03. **Zero public API removal** — the old entry points
  still exist.
- Removal condition: none (S1 is additive).

### S2 — Scene extraction (seam: orchestrator; content move: lane-able)

- Add `SceneDescription`/`attach_scene`/`SceneHandle` (§3.2); the corner
  content — `lit_scene`'s geometry/materials/presets/route data,
  `intersection_boxes`, the shadow-bounds input — moves to
  `engine-slice`'s `corner_scene.rs` (demo policy per the slice
  classification map, audits/slice.md §2); the renderer keeps the
  lighting EQUATION + `LightParams`/`Material` types + `cascade_for`
  (lighting semantics, not content). The `Corner` preference becomes an
  adapter constructing the corner `SceneDescription`.

  **The dependency-direction trap (alternative considered and rejected):**
  the adapter cannot call into slice for the content — renderer must not
  depend on slice (the dep DAG is slice→renderer), and inverting via a
  generic `Fn() -> SceneDescription` registered on `Renderer::new` was
  considered and rejected (a constructor closure is a service-locator
  smell; it would also leave the content permanently straddling the
  boundary). The adapter shape is instead:
  `RenderConfig::demo_scene: Corner` keeps uploading the corner content
  during S2 **from a renderer-side `corner_content.rs` module explicitly
  marked as the migration's removable shim** (content duplicated
  temporarily — the charter's "compatibility adapters … only temporarily"
  shape), while slice's `corner_demo` concurrently migrates to
  `attach_scene` with its own copy of the same content. S6 deletes the
  shim + the preference arm. The temporary-duplication cost is bounded
  and deleted, and the gate below makes drift impossible while it exists.

  **Gate addition:** a build-time assert that the shim's content and
  slice's corner builder produce identical `SceneDescription` bytes
  while both exist (a unit test comparing the two constructors —
  drift-proof during the temporary window).
- **Gates:** S1's batteries + render_scene_separation D1–D4 (the opt-in
  boundary) + the corner batteries' pixel pins (the corner content now
  flows through the attach API — ink values must not move) + the
  shim-equality test.
- Removal condition: corner_demo + all corner gates green on the
  attach path.

### S3 — The pass coordinator (orchestrator)

- `FramePlan` + `ensure_for` (§4.2); the scene_record consolidation
  (§4.3); the preparation consolidation (§4.4); the sticky sky/post/wet/
  ripple state migrates into the submission (adapters now build them
  per-frame from their retained fields — the demo's set-once pattern
  still works; the field is read by the adapter).
- **Gates:** the full battery set again (this stage touches the record
  cascade — the finding-14 ordered blend is re-proven by the weather
  term-separation pins 96548/67535/7625 + 27149/27149/2538, E-7);
  allocation-constancy (the plan must not allocate: the tuples are the
  proof); the shadow readback known-answers (the cascade still runs
  `cascade_for` on scene bounds).
- Removal condition: the two scene_record arms' duplication and the
  prepare triplication are gone (diff-verified) with all gates green.

### S4 — The Vulkan split (lane-able, mechanical, D-EE-09)

- The module moves of §5.1, pure-move commits, one module per lane
  batch; behavior unchanged by construction (moves only).
- **Gates:** `cargo test -p engine-renderer` (the 173-pin release
  battery, E-6) per move + fmt/clippy + one full lavapipe battery at
  batch close + the workspace gates.
- Removal condition: `vulkan.rs` reduced to mod + shared helpers
  (target: every module < ~1,200 lines; A01's renderer half closed with
  the "split by change reason" discipline — each module owns one charter
  responsibility).

### S5 — The shader composition (orchestrator; mechanical tail lane-able)

- §6.3's `core/` + `variants/` + build.rs composer + §6.4's layout pins;
  the ten .wgsl roots become the generated set; the old duplicated files
  deleted in the same series (no dual source).
- **Gates:** §6.5's six-bar equivalence set (validation, lavapipe
  batteries' pixel pins, shadow known-answers, orientation/
  position-invariance pins, layout pins, rig re-run).
- Removal condition: `git grep shadow_factor` hits exactly one file
  (`core/pcf.wgsl`); the variant shells contain no duplicated equation
  text.

### S6 — Old-path removal + the second scene (removal: orchestrator; scene authoring: lane-able)

- Delete the four `render_corner*` adapters, `render_layers`'s legacy
  signature (fold to `submit_frame` with `layers`), the sticky setters
  (`set_sky`/`set_post`/`set_wet`/`set_ripple_field` — the demo migrates
  to per-frame submission fields), the `corner_content.rs` shim, and
  `DemoScenePreference::Corner` (`Omitted`/`Included` disposition per
  §3.2 — `Included` kept). Slice's corner_demo/population/routes migrate
  their call sites (8 sites, corner_demo.rs:1008–1021, 1322–1335 +
  the sticky-setter call sites). The field scene consumer lands
  (§7).
- **Gates:** the full battery set on the FINAL API (no adapter anywhere);
  the new `render_field_scene.rs` battery green with its own pins; the
  diff-is-the-proof check (§7 leg 1); render_weather rig re-run (the
  free-look accumulator + sticky-migration is exactly the A24 risk class
  — the rig gate is mandatory, not optional); CI pin corrections batched
  per D-EE-06 if any test count moves (test removals/relocations
  documented).
- Removal condition: `git grep render_corner` returns zero production
  hits; the modularity rubric's second-scenario evidence recorded.

### Per-stage risk classification (D6, per the calibration record's standing rules)

| Stage | Classification | Why |
|---|---|---|
| S1 | **ORCHESTRATOR** | Semantics: validation ordering, byte-identity of frames through adapters — the A24 class (frame-semantics lanes failed 2/2 rounds; standing decision routes them to orchestrator). |
| S2 seam (SceneDescription/attach API, shim design) | **ORCHESTRATOR** | Lifetime/ownership + the dep-DAG inversion trap (§S2 correction). |
| S2 content move (lit_scene→slice, corner_scene.rs) | **LANE-ABLE** | Mechanical relocation under a tight brief; the shim-equality test + pixel pins are the teeth. |
| S3 | **ORCHESTRATOR** | The ensure discipline + ordered-blend semantics; every gate it touches is equivalence-sensitive. |
| S4 | **LANE-ABLE** (D-EE-09: pre-created branches, pure-move briefs, re-measured counts) | Mechanical extraction; the failed-lane pattern here was scope/branch discipline, solved by D-EE-09. |
| S5 composer + pins | **ORCHESTRATOR** | Visual-equivalence semantics; small, equivalence-sensitive surface. |
| S5 mechanical tail (variant shell rebases once composer is proven) | **LANE-ABLE** | Repetitive text surgery with the §6.5 gate set as teeth. |
| S6 removal decisions | **ORCHESTRATOR** | Public API deletion + demo migration semantics. |
| S6 field-scene authoring | **LANE-ABLE** | Demo content + a new gate battery per brief; the diff-is-the-proof check makes scope overrun visible. |

---

## 9. What each charter finding gets (P4-scope dispositions)

| Finding | P4 disposition (this design) |
|---|---|
| A01 (rend half: vulkan.rs 6,122; combinatorial entries) | §5.1 split by responsibility (the charter's "split by change reason"); §3.1 removes the entry-point god-shape; S4 delivers it. NOT closed by S4 alone — the follow-through is P6's module-coherence pass. |
| A03 (4 entry points + DemoScenePreference) | §3.1/§3.2/§8-S1/S6: one typed contract + scene attach; preference dies at S6. CLOSED at S6. |
| A04 (19-param render_frame) | §3.4: 6-parameter internal form, ownership story preserved; §5.4 named phases. CLOSED at S1. |
| A06 (animation in renderer crate) | **NO-CRATE-MOVE (deferred with trigger):** animation.rs is pure and its only consumer outside the renderer is pose-evaluation policy that feeds the renderer's own population contract — extracting a crate now adds a workspace member for one consumer (charter: avoid speculative abstraction; B15: dep-closure discipline). The design documents `animation` as the renderer's pure pose-evaluation surface (its module doc already says it, animation.rs:1–6) and records the revisit trigger: a second NON-RENDERING consumer of pose evaluation. Slice's import stays legitimate: it builds `ProxyPopulation` inputs. |
| A11 (capacity constants) | §3.6: named + observable budgets, typed refusals preserved; mechanics (chunks/arena/dirty ranges) stay P5. PARTIAL-BY-DESIGN (P4's half). |
| A12 (shader variant pairs + draw-module duplication) | §6: build-time composition + layout pins; the Rust-side per-variant sets stay (they encode genuine binding differences — §4.3 keeps only true variation). CLOSED at S5 for shaders; the descriptor-set mirror duplication is bounded by the same variant count, documented in the passes pattern contract. |
| A14 (lifetimes, partial-failure, device-lost/resize) | §1.5/§4.5/§5.6: three-tier taxonomy documented, teardown manifest cited, device-lost typed + boundary documented, resize discipline untouched (already excellent). CLOSED at S3/S4 + §5.6. |
| A15 (rend half — demo policy vs mechanism) | §3.2/§7/§8-S2/S6: corner content to slice, second consumer proves the boundary. The slice-internal extraction (math.rs/frame_timing.rs etc.) is P6's per the classification map; P4 consumes the map's split for the corner content only. |
| B14 (rend half: variant ordering; reflection targets) | §4.3 consolidates the ordering (the charter's named half); the full-size-target VRAM measurement stays P5 (charter puts B14's measurement in P5). HALF CLOSED at S3. |
| B15 (rend half: dep closure) | §2.2 non-goal: no new deps; the second consumer doubles as the minimal-renderer-consumer evidence for P6's footprint work. |

---

## 10. Open questions (for the orchestrator / owner, none blocking design)

1. **The second scene's CLI surface** — a new flag in main.rs's cascade
   (NEW-SLICE-05 already flags the cascade's split for P6): land the field
   scene as `--scene-field` (one more early-return arm, consistent with
   the current style) and let P6's cli.rs split absorb it? Recommend yes
   (P6-owned refactor later).
2. **`DemoScenePreference::Included`'s long-term home** — kept (§3.2) as
   the debug static-scene opt-in; should it eventually become a
   SceneDescription too (uploaded through attach)? Deferred to P6's API
   pass — no P4 behavior change.
3. **`ProxyDrawMode`'s placement** — moves from a renderer sticky into
   the submission's population policy (§3.4); confirm the equivalence
   gates' mode-switching patterns (render_instanced's mode batteries)
   still read naturally. Design says yes (the mode is per-population
   submission data, matching `ProxyFrameUpload.mode`'s existing plumbing).
4. **A06 revisit trigger** (§9) — record in FINDINGS when the P2 world
   representation work creates a second pose consumer.
5. **Device-lost error arm naming/semantics** — `DeviceLost(String)` vs a
   structured payload; decide at S1 implementation (trivial, but it's a
   public error-contract addition — P6's error-semantics audit should see
   it).
6. **The phantom `audits/renderer.md`** (§0) — orchestrator to
   re-materialize or de-reference.
7. **B14's P5 measurement dependency** — S3's coordinator must not be
   blocked on reflection-target profiling; confirmed independent (the
   ordering consolidation is semantic, the targets' sizes are a P5
   resource question).

---

## 11. Honest coverage limits of this design's evidence

- The renderer audit artifact was absent (§0); all renderer facts were
  re-derived from source this session, but the audit's own coverage
  (which files it read deeply) is unknown — my coverage stands in for it
  and is narrower than a full area audit on: `post_draw.rs`,
  `reflection_draw.rs`, `sky_draw.rs`, `shadow_draw.rs`, `corner.rs`,
  `textures.rs`, `planar.rs`, `populated_meshes.rs` interiors. The design
  touches those modules' edges (call signatures, lifetimes, target
  ensures) — all cited from the call sites in `vulkan.rs`/
  `frame_prepare.rs`/`scene_record.rs`, which I read; their INTERIORS
  were not re-verified line-by-line.
- The slice side was consumed via the audit's classification map + the
  dispatch/pose sites; corner_demo's loop internals beyond the dispatch
  (weather integration, routes) were not re-read (A24's record covers the
  timing semantics this design's S6 sticky-migration touches — the rig
  gate is named for exactly that reason).
- No measurements were taken (read-only): the zero-cost claims in §3.4
  are structural (borrow-only structs, no dyn, no heap) and are gated —
  not asserted as proven — by the allocation-constancy batteries named
  per stage. The "no regression" burden rests on those gates, per the
  charter's evidence rules.
- The shader composition (§6) was designed against naga 29.0.4's API
  surface as used in build.rs (parse_str/validate/write_vec +
  binding_map); the layout-pin emission path (naga's type-layout
  resolution) was not prototyped — S5's first implementation step is a
  spike proving the pin emission, before any variant moves.
- Line numbers are exact at `7961cb2`; any rebase shifts them (the
  charter's standard caveat — symbols are stable, numbers are
  SHA-scoped).

---

## 12. Charter §7-P4 sentence-by-sentence compliance trace

| Charter sentence (§7 P4, verbatim-order) | Design section |
|---|---|
| "Define a small scene-independent renderer submission contract with explicit frame inputs, camera(s), visible instances/resources, material/light/effect/pass requirements and output." | §3.1–§3.2 |
| "Separate world-facing scene extraction from backend data" | §3.2 (scene handle), §5.1 (pass modules) |
| "feature selection from authored corner geometry" | §3.2 (GroundRoles/bounds/material table from the scene) |
| "frame lifetime from resource lifetime" | §4.5 (three-tier taxonomy) |
| "capture/diagnostics from normal production submission" | §5.5 |
| "Preserve current specialized shaders and direct Vulkan control." | §2.2, §6.3 (variant set unchanged), §5.2 (ash, unsafe containment) |
| "Factor shared shader semantics at build time if useful; maintain host/shader layout invariants with a clear source of truth." | §6.2–§6.4 |
| "Build a thin pass coordinator (or justified graph) with explicit resource uses and lifetime, without needless allocations or runtime dispatch." | §4.1–§4.2 (+ the graph rejection justification) |
| "Split Vulkan responsibilities (context/device capabilities; resource allocation/retirement; swapchain/presentation; frame synchronization/commands; uploads; pass resources; capture)." | §5.1 (all seven named) |
| "Avoid merely moving central complexity into a generic context object." | §5.2 (anti-god-object rules, four-state ownership preserved) |
| "Establish resource handles/ownership and failure cleanup." | §3.2 (SceneHandle), §1.5/§5.2 (partial-failure discipline preserved), §5.6 |
| "Migrate demo through compatibility adapters only temporarily, then remove duplicated old paths when safe." | §3.5, §8 (S1 adapters → S6 removal, per-stage safety gates) |
| "Demonstrate a second non-corner scene using same public renderer API without editing renderer internals." | §7 (three proof legs) |
| "Maintain all current effects, visual appearance and data flow." | §2.2 non-goal, §8's per-stage pixel-pin gates |
| "Do not reduce fidelity or content to speed up." | §2.2, §6.5 (no quality-axis gate weakening) |
| "Generalization must pay its way on small scenes too." | §3.4 zero-cost statement + the allocation-constancy gates run on the SMALL scenes (the corner batteries are small-scene workloads) |

---

## 13. Implementation-order summary (for the lane planner)

S1 (orchestrator) → {S2 seam (orchestrator) → S2 content (lane)} → S3
(orchestrator) → S4 (lanes, D-EE-09) with S5 (orchestrator + mechanical
tail) parallelizable after S1 → S6 (orchestrator removal + lane scene
authoring). Every stage's gate set is named in §8; the equivalence bar
throughout is the existing pinned batteries (lavapipe logic gates +
allocation-constancy + pixel/ink pins + rig re-verification), never a
weakened or new-easier threshold (D-EE-06 applies to any pin movement).

*— design-p4, complete at `7961cb2`; handoff summary in the dispatching
session.*
