//! Safe public renderer API (Stage 2). All Vulkan unsafe lives in
//! [`crate::vulkan`]; this module owns the four Vulkan states, exposes the
//! safe surface, and collects instrumentation.

use ash::vk;
use engine_core::timing::TimingSamples;
use std::time::Duration;

use sdl3::video::Window as SdlWindow;

use crate::config::{
    ArenaBudgets, RenderConfig, TargetFaultSite, TargetFaultStage, present_mode_str,
};
use crate::overlay::{AtlasText, OverlayVertex};
use crate::scene::SceneVertex;
use crate::scene_description::{SceneDescription, SceneHandle};
use crate::submission::{
    FrameInputs, FrameSubmission, PopulationGenerations, SubmissionPolicy, adapt_frame,
};
use crate::vulkan::{
    self, CapturedFrame, FrameState, InternalFrame, PipelineState, RendererError, SwapchainState,
    VulkanContext, drain_validation_messages, environment_pending, pick_depth_format,
    pick_surface_format,
};

/// One captured shadow-map readback (the 6-F known-answer path): the
/// depth-only pass's map, one f32 per texel, row-major with row 0
/// first, at the map's square extent. The depths are UNBIASED
/// light-space values in [0, 1] (the compare-time analytical bias is
/// applied at lookup, never written) — exactly the CPU raycaster's
/// `t / (far − near)`.
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowMapReadback {
    /// The map's square texel extent.
    pub extent: u32,
    /// `extent × extent` depths, row-major, row 0 first.
    pub depths: Vec<f32>,
}

/// A batch of triangles with its own transform into clip space. A
/// foreground batch typically supplies camera-local vertices and a projection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneLayer<'a> {
    pub vertices: &'a [SceneVertex],
    pub view_proj: &'a [[f32; 4]; 4],
}

/// One proxy population for a populated corner frame: the NPC,
/// vehicle, and RAIN instance lists (the Era-2 package C third
/// kind), the npc kind's evaluated POSE BLOCKS (the Era-2 skinning
/// input), and the per-kind material indices into the corner
/// scene's material table (which specular/shininess set each kind
/// draws with — CALLER policy, not engine policy; the demo and the
/// equivalence gate document their choices). The renderer packs the
/// lists into its retained instance buffer in the fixed
/// npc-then-vehicle-then-rain order (rain LAST), and the pose
/// blocks into its retained pose byte buffer in POSE-INDEX order.
///
/// THE SHADOW EXCLUSION (the Era-2 package C documented contract):
/// rain casts no shadows — translucent streaks have no silhouette —
/// so a shadowed populated frame submits only the npc and vehicle
/// kinds through the shadow depth pass; the rain kind rides the LIT
/// pass alone (drawn through the unlit blended particle pipeline,
/// which receives no shadows by construction).
#[derive(Debug, Clone, Copy)]
pub struct ProxyPopulation<'a> {
    pub npc: &'a [crate::proxy_instances::ProxyInstance],
    /// The npc kind's skin-matrix blocks, in POSE-INDEX order (block
    /// i serves every npc whose `pose_index` is i — usually one
    /// block per npc; fewer blocks than npcs is the dedup shape).
    /// CALLER policy: the evaluated poses for this frame (the walk
    /// cycle's sampled matrices) or the rest block repeated for a
    /// static population. Validated: every populated npc's
    /// `pose_index` must index into this list (a typed refusal), and
    /// the list is bounded by [`crate::animation::MAX_POSE_BLOCKS`].
    pub npc_poses: &'a [[[[f32; 4]; 4]; crate::animation::HUMANOID_JOINTS]],
    pub npc_material: u32,
    pub vehicle: &'a [crate::proxy_instances::ProxyInstance],
    pub vehicle_material: u32,
    /// The rain kind's instance list (the Era-2 package C — the
    /// translucent streaks, submitted LAST in the fixed kind order;
    /// the instances' `alpha` field carries the per-streak
    /// coverage). Empty on the dry path — byte-identical to the
    /// pre-package frames.
    pub rain: &'a [crate::proxy_instances::ProxyInstance],
    /// The rain kind's material index into the corner table (its
    /// set binds the streak texture's albedo/sampler). An EMPTY rain
    /// kind may carry any index (validated only when populated,
    /// exactly like the other kinds).
    pub rain_material: u32,
    /// P5 §3.3 (A18/R2 — the dirty-generation protocol): the
    /// INSTANCE population's generation — the caller-owned claim
    /// that this submission's npc/vehicle/rain bytes are IDENTICAL
    /// to every prior submission at the same nonzero value. **0 =
    /// always-dirty** (today's behavior: the renderer validates and
    /// repacks every populated frame, and every frame slot is
    /// rewritten — the default every un-opted caller keeps). The
    /// renderer tracks, per retained buffer and per frame slot,
    /// which generation's bytes each holds and skips the
    /// regeneration and the mapped write on a match.
    ///
    /// **The ownership contract** (the mutation invariant): bump
    /// this EXACTLY when any instance input changes — a re-packed
    /// crowd, a rebuilt rain pool, a retinted vehicle — and never
    /// reuse a value after its population changed (the generation
    /// space belongs to THIS population source; two interleaved
    /// opted-in populations colliding at one generation is a caller
    /// defect the debug verify mode catches). A missed bump renders
    /// the prior population in release — the protocol's one real
    /// hazard, made loud by [`crate::proxy_instances::
    /// verify_population_generation`] in debug/test builds. The
    /// indirect command block rides THIS generation (its bytes are a
    /// pure function of the per-kind counts).
    pub instance_generation: u64,
    /// P5 §3.3: the POSE population's generation — the same
    /// caller-owned claim for `npc_poses` (the garage's split: the
    /// static placement never bumps the instance half, the advancing
    /// walk cycle bumps this one every evaluated frame). **0 =
    /// always-dirty.** Bump exactly when the evaluated blocks change
    /// (a pose advance, a stander/walker transition); the
    /// composition-side dedup (the shared rest block) changes the
    /// BLOCK LIST and therefore bumps this generation once at the
    /// adopting population's next change.
    pub pose_generation: u64,
}

/// How proxy instances are submitted (the 6-E equivalence gate's
/// axis): `Instanced` — one draw per populated proxy kind (the
/// production path, the package's headline); `PerInstance` — one
/// draw per INSTANCE through the same shader, bytes, and descriptor
/// sets (the pre-6-E per-object submission shape, kept as the
/// N-draws reference for the pixel-equivalence gate and the
/// before/after draw-call evidence — production paths never set it);
/// `Indirect` — the 6-E2 mechanism: one `vkCmdDrawIndirect` per
/// populated kind with the instance count read from GPU memory at
/// draw time (the packed [`crate::proxy_indirect`] command block),
/// byte-identical output to `Instanced` — the submission stops
/// carrying the count so a later GPU pass can author it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProxyDrawMode {
    #[default]
    Instanced,
    PerInstance,
    Indirect,
}

/// Actual dynamic uploads and draws recorded in this frame.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GeometryStats {
    /// World plus foreground vertices.
    pub scene_drawn: u32,
    /// Portion of `scene_drawn` used by the foreground.
    pub foreground_drawn: u32,
    /// HUD plus diagnostic vertices.
    pub overlay_drawn: u32,
    pub scene_omitted: u32,
    pub overlay_omitted: u32,
    pub dynamic_draw_calls: u32,
    /// Proxy draw calls recorded this frame (the 6-E headline: one
    /// per populated kind in [`ProxyDrawMode::Instanced`] and
    /// [`ProxyDrawMode::Indirect`] alike — the GPU-side count
    /// changes where N lives, never the draw-call count — and one
    /// per instance in [`ProxyDrawMode::PerInstance`]). Counts proxy
    /// submission only — the corner mesh's own material draws are
    /// not included. Zero on unpopulated frames.
    pub proxy_draw_calls: u32,
    /// vkCmdDrawIndirect issuances this frame (the 6-E2 issuance
    /// observation the Era-1 review required of the instancing
    /// gate: the draw-call count cannot distinguish the indirect
    /// submission from the direct one — both are one per populated
    /// kind — and the release fallback silently submits identical
    /// geometry through cmd_draw; this counts the actual
    /// vkCmdDrawIndirect calls at their site). One per populated
    /// kind in [`ProxyDrawMode::Indirect`], zero in every other
    /// mode.
    pub indirect_draws_issued: u32,
    pub uploaded_bytes: u64,
}

/// Per-frame result surfaced to the application.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FrameResult {
    pub presented: bool,
    pub recreated: bool,
    pub suboptimal: bool,
    /// The drawable was temporarily unavailable (zero-sized surface
    /// capabilities — minimized or mid-transition): nothing happened;
    /// poll and try again next frame.
    pub deferred: bool,
    pub gpu_render_ms: Option<f64>,
    pub geometry: GeometryStats,
}

/// Successful Vulkan allocation calls since renderer construction and the
/// resident byte size of the texture allocations (including staging tails).
/// The additional fields count only the Era-1 target builders' Vulkan
/// objects (image, memory, view, framebuffer — post chain, wet chain,
/// shadow chain). Texture/corner/overlay allocations stay outside this
/// scope; the existing fields' semantics are unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocationStats {
    pub device_memory: u64,
    pub texture_memory: u64,
    pub texture_bytes: u64,
    pub descriptor_pools: u64,
    pub descriptor_sets: u64,
    pub samplers: u64,
    /// Monotonic creation count of Era-1 target builder images
    /// (post, wet, shadow).
    pub target_images: u64,
    /// Monotonic creation count of Era-1 target builder image views.
    pub target_image_views: u64,
    /// Monotonic creation count of Era-1 target builder framebuffers.
    pub target_framebuffers: u64,
    /// Monotonic creation count of Era-1 target builder memory allocations.
    pub target_memories: u64,
    /// Monotonic destruction count of Era-1 target builder images.
    pub target_image_frees: u64,
    /// Monotonic destruction count of Era-1 target builder image views.
    pub target_view_frees: u64,
    /// Monotonic destruction count of Era-1 target builder framebuffers.
    pub target_framebuffer_frees: u64,
    /// Monotonic destruction count of Era-1 target builder memory allocations.
    pub target_memory_frees: u64,
    /// P5 R0 (§9.4, the attach/upload census): monotonic count of
    /// one-shot staging submissions through the upload helper
    /// (`with_upload_commands`) — each is a fresh command pool + one
    /// command buffer + a fence, ONE queue submit, ONE blocking
    /// `wait_for_fences`, then destruction of pool and fence. The
    /// per-object machinery the A20/R3 staging pool replaces;
    /// before-R3, every attach-time buffer and texture upload pays
    /// exactly one of these. Read-only surface: recorded at the
    /// helper's site, never changed by any frame-path code.
    pub upload_submits: u64,
    /// P5 R0 (§9.4): accumulated wall-clock nanoseconds blocked
    /// inside those staging submissions' `wait_for_fences` calls —
    /// the "blocking-wait total" row. Attach-time only (the frame
    /// path's fence waits are outside this counter); the CPU/GPU
    /// overlap the deferred-wait option would recover is exactly
    /// this quantity.
    pub upload_blocking_wait_ns: u64,
    /// P5 R0 (§9.4): monotonic count of device-local buffer uploads
    /// staged through a throwaway HOST_VISIBLE pair
    /// (`create_device_local_buffer_with_data`) — each call creates
    /// AND destroys one staging buffer plus one staging memory. The
    /// "staging object counts" row's buffer half (the texture half
    /// is the permanent-tail counter below).
    pub staged_buffer_uploads: u64,
    /// P5 R0 (§9.4): the PERMANENT per-texture staging tails — for
    /// every uploaded texture, the bytes of its single
    /// device-local allocation beyond the image's own memory
    /// requirement (the staging buffer's bytes plus the alignment
    /// padding between the image and staging regions), resident for
    /// the texture's whole lifetime. The dead VRAM A20/R3's pool
    /// retires; `texture_bytes` above INCLUDES these same bytes in
    /// its per-texture totals, so this field is the attribution
    /// slice, not an additional sum.
    pub texture_staging_tail_bytes: u64,
    /// CA-01 (the attach-atomicity census): monotonic creation count
    /// of the corner pipeline group's Vulkan objects (the lit,
    /// proxy, and rain pipelines plus the lit and proxy layouts —
    /// the five handles the group stages before publishing into the
    /// pipeline state). The paired frees counter below counts the
    /// group's scoped-owner releases; a FAILED attach that rolled
    /// back leaves the pair equal (created then released), which is
    /// the no-leak proof the attach battery asserts per injected
    /// fault site.
    pub corner_pipeline_objects: u64,
    /// CA-01: monotonic destruction count of those same corner
    /// pipeline group objects through the group's scoped owner (a
    /// failed or replaced build's partial release). Objects that
    /// COMMIT into the live pipeline state are freed later by the
    /// state's own `destroy` — outside this counter — so the pair
    /// reads equal exactly when every staged object was released.
    pub corner_pipeline_object_frees: u64,
    /// CA-11 (the growth-handle-loss census): monotonic count of
    /// arena-carve replacements STAGED through the scoped mapped-
    /// buffer owner (the region, instance, and pose growth paths'
    /// new buffer+memory pair, created before the old carve dies).
    /// The paired frees counter below counts the owner's releases; a
    /// growth whose fallible idle wait failed leaves the pair equal
    /// (staged then released — the new objects cleaned up, the old
    /// bindings intact), which is the no-leak proof the growth
    /// battery asserts per injected fault site.
    pub arena_replacements: u64,
    /// CA-11: monotonic destruction count of those same staged
    /// replacement pairs through the scoped owner's Drop. A
    /// replacement that COMMIT (retiring the old carve) is freed
    /// later by the owning state's destroy — outside this counter —
    /// so the pair reads equal exactly when every staged replacement
    /// was released.
    pub arena_replacement_frees: u64,
}

/// Long-lived renderer statistics (Stage 2 authorization, item 4).
///
/// These are instrumentation readings of the simple Stage 2 scene — they
/// are not claims about the first game's performance (TECH-ARCHITECTURE
/// §6, stage-table policy).
#[derive(Debug, Clone)]
pub struct RenderStats {
    pub gpu_name: String,
    pub device_type: String,
    pub api_version: String,
    pub present_mode: String,
    pub supported_present_modes: Vec<String>,
    pub surface_format: String,
    pub extent: (u32, u32),
    pub swapchain_images: u32,
    pub frames_presented: u64,
    pub swapchain_recreations: u64,
    pub validation_active: bool,
    /// Synchronization validation requested-and-accepted on the
    /// validation instance (finding 9's explicit enablement: the
    /// VK_EXT_validation_features pNext chained when
    /// `ValidationPreference::On` and the layer accepted it — a
    /// failed attempt falls back with this false alongside
    /// `validation_active`). When true, the `validation_errors`
    /// count includes the layer's synchronization-hazard findings
    /// (SYNC-HAZARD-*), so every gate asserting
    /// `validation_errors == 0` now also proves the render passes'
    /// execution and memory dependencies.
    pub sync_validation: bool,
    pub gpu_timestamps_supported: bool,
    pub validation_errors: u64,
    pub validation_warnings: u64,
    /// Last validation messages (bounded), most recent last.
    pub recent_validation: Vec<String>,
    /// F-03 Step 1 (fail-visible over fail-silent): cumulative
    /// dynamic-geometry vertices omitted because a frame's supply
    /// exceeded the frame slot's buffer capacity, with the peak
    /// supplied and the per-slot capacity for context. Zero in every
    /// ordinary run (the H3 proof); nonzero is REPORTED in the run
    /// summary, never fatal — the capacity/refusal policy is Step 2,
    /// separately authorized. The region capacity reads 0 until the
    /// lazy region buffer allocates (no region frames).
    pub region_truncated_vertices: u64,
    pub region_supplied_peak_vertices: u32,
    pub region_capacity_vertices: u32,
    pub overlay_truncated_vertices: u64,
    pub overlay_supplied_peak_vertices: u32,
    pub overlay_capacity_vertices: u32,
    /// P5 §1.3 (A11/R1): the instance carve's current per-slot
    /// capacity, instances (the max over frames — the carve only
    /// grows; 0 until the first populated frame, the region
    /// discipline). Landing P4 §3.6's named-but-unlanded
    /// observability: the budget and its use are reported, not a
    /// hidden global.
    pub instance_capacity: u32,
    /// P5 §1.3: the peak total population supplied in one frame
    /// (npc + vehicle + rain — the demand the carve sized against).
    pub instance_peak_used: u32,
    /// P5 §1.3: the pose carve's current per-slot capacity, blocks
    /// (0 until the first populated frame).
    pub pose_capacity: u32,
    /// P5 §1.3: the peak pose blocks supplied in one frame.
    pub pose_peak_used: u32,
    /// P5 §1.6: monotonic count of arena GROWTH events — carve
    /// re-creations at a larger size (the initial demand-sized carve
    /// is not an event; each event stalled its frame once under
    /// `device_wait_idle`, bounded by the pow2 quantum to at most
    /// `log2(cap / 64)` per session and carves never shrink).
    pub arena_growth_events: u64,
    /// P5 §3.3 (A18/R2): monotonic count of population mapped-region
    /// writes the dirty-generation protocol SKIPPED — instance,
    /// pose, and indirect blocks whose per-slot stamp already
    /// recorded the submission's generation (the between-tick and
    /// static-placement frames' whole saving, made observable). Zero
    /// for every un-opted (generation-0) caller.
    pub population_writes_skipped: u64,
    gpu_timing: TimingSamples,
    /// The negotiated depth format (corrective finding 4, F):
    /// the renderer's chosen depth format, reported so tests can
    /// verify fallbacks and the DepthFormatPreference knob.
    pub depth_format: String,
}

impl RenderStats {
    /// GPU render time (timestamp-derived), millisecond percentiles.
    pub fn gpu_ms_p50(&self) -> f64 {
        self.gpu_timing.percentile_ms(0.50)
    }

    pub fn gpu_ms_p95(&self) -> f64 {
        self.gpu_timing.percentile_ms(0.95)
    }

    pub fn gpu_ms_p99(&self) -> f64 {
        self.gpu_timing.percentile_ms(0.99)
    }

    pub fn gpu_ms_max(&self) -> f64 {
        self.gpu_timing.max_ms()
    }

    pub fn gpu_sample_count(&self) -> usize {
        self.gpu_timing.sample_count()
    }
}

/// The Stage 2 Vulkan renderer. Rendering cadence is caller-driven — the
/// renderer imposes no frame-rate ceiling, and rendering is deliberately
/// independent of the future fixed-rate campaign simulation (Stage 3,
/// gated). Drop performs the ordered teardown documented in
/// [`crate::vulkan`].
pub struct Renderer {
    ctx: VulkanContext,
    swapchain: SwapchainState,
    pipes: PipelineState,
    frames: FrameState,
    config: RenderConfig,
    stats: RenderStats,
    /// The attached scene's CPU-side facts (P4 S2, completed at S6):
    /// the handle — the material-count bound the population
    /// validation reads. `None` until [`Self::attach_scene`]; the
    /// former `Corner` construction preference (the S2 migration's
    /// auto-attach adapter) is DELETED, so every consumer — the
    /// corner demo included — attaches its own
    /// [`SceneDescription`] through the public API. The description
    /// itself is consumed at attach (the GPU half lives in the
    /// context's scene resources; the shadow cascade's bounds travel
    /// per frame in the submission's [`crate::SceneFrame`], the
    /// authored value the caller holds with the description).
    scene: Option<AttachedScene>,
    overlay_enabled: bool,
    overlay_color: [f32; 4],
    /// The F-05 retained overlay-vertex buffer: rebuilt in place every
    /// frame (the capacity from the first frames is reused — no
    /// per-frame allocation or growth; the vertex content is
    /// identical to the allocating form).
    overlay_vertices: Vec<OverlayVertex>,
    atlas_text: AtlasText,
    /// The 6-E retained instance-byte buffer (the same F-05
    /// discipline, applied through
    /// [`crate::proxy_instances::repack_populations`]): repacked in
    /// place every populated frame; the capacity from the first
    /// populated frames is reused — no per-frame allocation or
    /// growth.
    instance_bytes: Vec<u8>,
    /// The retained pose byte buffer (the F-05 discipline applied to
    /// the pose blocks — repacked in place every populated frame,
    /// its capacity from the first frames reused forever).
    pose_bytes: Vec<u8>,
    /// P5 §3.3 (A18/R2): the retained repack buffers' generation
    /// records — which generation's bytes `instance_bytes` and
    /// `pose_bytes` each hold (0 = unknown: the initial state, and
    /// what every generation-0 submission resets to, so an opted-in
    /// generation can never reuse against a buffer holding some
    /// other population's bytes). The CPU half of the
    /// dirty-generation protocol; the per-frame-slot write half
    /// lives in the frame state's stamps.
    population_generations: PopulationGenerations,
    proxy_draw_mode: ProxyDrawMode,
    /// The TEST-ONLY ripple debug mode (finding 15b's separation
    /// fixtures — see [`RippleDebugMode`]). The one retained
    /// renderer-level policy the submission adapter still reads per
    /// frame (with the proxy mode); `Off` by default and in every
    /// production frame.
    ripple_debug: RippleDebugMode,
    /// CA-12 (F12): the three arena budgets VALIDATED at
    /// construction — the shared representable capacity (`u32`, the
    /// pose arena device-checked against the binding-5 storage
    /// range/alignment). The ONE trio every later boundary reads:
    /// the frame policy's carve caps AND the submission policy's
    /// repack caps (widened to `usize` only at the public repack
    /// calls, losslessly), so the CPU typed-refusal boundary and
    /// the device carve ceiling are the same number by construction.
    arena_budgets: ArenaBudgets,
    /// CA-13 (F13/A14): the device-loss latch. `None` until the
    /// first [`crate::RendererError::DeviceLost`] leaves a
    /// device-touching entry; from then on every public
    /// frame/attach/capture entry refuses with the typed variant
    /// WITHOUT touching the dead device (repeated submissions on a
    /// lost device are prevented, not retried).
    device_lost: vulkan::DeviceLossLatch,
}

/// The Era-1 sky layer's frame description (the per-frame
/// [`crate::SceneFrame`] field the former sticky `set_sky` setting
/// fed): the palette and the celestial disc's direction. The
/// renderer derives the pass's remaining input (the inverse
/// view-projection) per frame from the camera matrix it is already
/// given — the caller supplies nothing per frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyFrame {
    /// The gradient + disc palette (see [`crate::sky::SkyPalette`];
    /// the day/night presets and `atmosphere_at` provide them).
    pub palette: crate::sky::SkyPalette,
    /// The direction TOWARD the celestial body (the sun by day, the
    /// moon by night; the presets' light directions). The renderer
    /// normalizes it (the CPU core's contract).
    pub disc_dir: [f32; 3],
}

/// The ripple tangent-displacement gain (the finding-19 semantics:
/// METERS of ground-plane push per unit of triangle-bump slope —
/// the shader projects the push into screen UV, so this constant
/// converts the unitless slope `amplitude / width` into world
/// meters, never directly into screen units).
///
/// THE RETUNE DERIVATION (the projection changed the gain's units
/// from raw screen-uv to world meters, so the visible magnitude was
/// re-derived to keep the audit's measured reference — a unit-
/// amplitude fresh band (slope a₀/w = 1/0.35 ≈ 2.857) pushing
/// ~5.85 px horizontally at 1280×720, the pre-corrective tree's
/// 0.0016/0.35 ≈ 0.00457 normalized ≈ 5.85 px): at the demo/gate
/// vantage (the eye [0, 1.7, 15] on the north road, the ground
/// rings 3–13 m out), the projection carries, per world meter of
/// ground-plane push, `½·focal/extent·extent_px = 565 px/m ÷
/// view_distance` — square-pixel equal in both axes (640·focal/
/// aspect == 360·focal at 1280×720). At the representative 8.2 m
/// ring (the gate's [0, 7] ground point) that is 69 px/m, so a
/// push of 2.857·g meters spans 197·g px; g = 0.03 gives 5.9 px —
/// the old visible magnitude, now camera-correct (the nearer
/// forced rings foreshorten to ~15 px at 3.2 m, the farther ones
/// compress to ~4 px at 11.7 m — the perspective the raw add
/// never carried). PROVISIONAL pending the lavapipe ink
/// measurement, as the pre-corrective value was.
pub const RIPPLE_UV_GAIN: f32 = 0.03;
/// The ripple roughen gain: the bump coverage's dimming of the
/// Schlick blend (0.5 = the peak coverage halves the mirror).
pub const RIPPLE_ROUGHEN_GAIN: f32 = 0.5;

/// TEST-ONLY ripple debug mode (the Era-2 audit finding 15b
/// separation fixtures): selects which perturbation term the wet
/// ground re-draw applies, so the ink gate can isolate the UV
/// displacement and the blend dimming into separate fixtures.
/// `Off` is the shipped path — the production packing writes lane
/// 0.0 into the uniform's rows[1].y and every demo frame runs it;
/// the two suppressors ride the SAME spare lane the shader reads
/// (`ground_reflect.wgsl`'s params1.y — the packing's layout
/// contract, documented test-only). The CPU oracle
/// (`planar::ripple_perturbation`) mirrors the SHIPPED mode-0 path
/// term-for-term; the suppressors remove whole terms (never
/// re-shape them), so a suppressed fixture's surviving term is
/// still the oracle's own arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RippleDebugMode {
    /// The shipped path: both the UV displacement and the blend
    /// roughening active (uniform lane 0.0).
    #[default]
    Off,
    /// The UV/displacement term suppressed (uniform lane 1.0):
    /// blend only — the roughening dims the mirror, the sample
    /// position never moves.
    BlendOnly,
    /// The blend/roughen term suppressed (uniform lane 2.0):
    /// displacement only — the sample position moves, the mirror
    /// never dims.
    DisplacementOnly,
}

impl RippleDebugMode {
    /// The uniform lane value (ground_reflect.wgsl's params1.y).
    fn uniform_lane(self) -> f32 {
        match self {
            Self::Off => 0.0,
            Self::BlendOnly => 1.0,
            Self::DisplacementOnly => 2.0,
        }
    }
}

/// The attached scene's retained fact (the renderer's `scene`
/// field's payload — see its doc): plain `Copy` data distilled from
/// the consumed [`SceneDescription`] at attach. The handle's
/// material count is the population validation bound (the S1-era
/// `lit_scene::materials().len()` read). The shadow cascade's
/// bounds are NOT retained here: the submission's
/// [`crate::SceneFrame`] carries them per frame — authored content
/// the caller holds alongside the description it attached.
#[derive(Debug, Clone, Copy)]
struct AttachedScene {
    handle: SceneHandle,
}

/// The attach step behind the public API
/// ([`Renderer::attach_scene`]): validate the description WHOLE,
/// device-free (a refusal builds nothing and leaves the context
/// untouched), then upload through the context's scene-resource
/// machinery (partial-failure-safe — on error the context is
/// unchanged), then distill the retained facts. The INSTALL the
/// machinery performs is STAGING: the caller completes the
/// transaction by building the corner pipeline group and publishes
/// the handle only on full success, rolling the install back
/// through [`VulkanContext::release_staged_scene_resources`]
/// otherwise (CA-01 — see the public method's doc). ONE scene per
/// renderer lifetime (the context's attach refuses a second; the
/// device-lifetime pass families bind the attached buffers).
fn attach_description(
    ctx: &mut VulkanContext,
    description: SceneDescription,
) -> Result<AttachedScene, RendererError> {
    description.validate()?;
    let material_count = description.materials.len() as u32;
    ctx.attach_scene_resources(&description)?;
    Ok(AttachedScene {
        handle: SceneHandle { material_count },
    })
}

impl Renderer {
    pub fn new(window: &SdlWindow, config: RenderConfig) -> Result<Renderer, RendererError> {
        let atlas_text = AtlasText::default();
        let (mut ctx, validation_active, sync_validation) =
            VulkanContext::new(window, atlas_text.atlas(), &config)?;
        // CA-12 (F12): the shared representable-capacity validation,
        // at construction, against the LIVE device — the audit's
        // "validate a shared representable capacity at construction;
        // include device limits, per-slot sizes and dynamic-offset
        // alignment": the three configured `usize` budgets narrow
        // ONCE here (a budget outside the representable `u32`
        // domain refuses, named — the pre-fix `cap as u32` sites at
        // the carves silently wrapped it to a different number), and
        // the pose budget checks the binding-5 storage range and
        // offset alignment its cap-clamped carve would bind. The
        // ladder discipline: this is the context's own stage — a
        // refusal destroys exactly what exists so far.
        let (max_storage_buffer_range, min_storage_buffer_offset_alignment) =
            ctx.pose_binding_limits();
        let arena_budgets = match ArenaBudgets::validate(
            &config,
            max_storage_buffer_range,
            min_storage_buffer_offset_alignment,
        ) {
            Ok(budgets) => budgets,
            Err(error) => {
                ctx.destroy();
                return Err(error);
            }
        };
        // P4 S6: the `Corner` construction preference is deleted —
        // authored scene content reaches the renderer ONLY through
        // [`Self::attach_scene`] (the demo and every test attach
        // their own description). Construction builds no scene
        // resources; the ladder below is the plain
        // context→format→pipelines→swapchain→frames climb.
        // Cross-stage cleanup (corrective finding G): each constructor
        // cleans up its own partials; this ladder releases completed
        // stages when a later stage fails.
        let (format, color_space) = match pick_surface_format(&ctx) {
            Ok(picked) => picked,
            Err(error) => {
                ctx.destroy();
                return Err(error);
            }
        };
        // The depth format is chosen once and shared by the render pass
        // and every depth attachment (corrective finding F).
        let depth_format =
            pick_depth_format(&ctx.instance, ctx.physical_device, config.depth_format);
        // The demo-scene preference is fixed at construction (P3): the
        // static Stage-2 demonstration scene uploads only on the
        // explicit opt-in, and every later pipeline replacement is
        // built with this same live preference.
        let mut pipes = match PipelineState::new(&ctx, format, depth_format, config.demo_scene) {
            Ok(pipes) => pipes,
            Err(error) => {
                ctx.destroy();
                return Err(error);
            }
        };
        let (width, height) = window.size_in_pixels();
        let mut swapchain = match SwapchainState::new(
            &ctx,
            (width, height),
            config.present_mode,
            format,
            color_space,
            pipes.render_pass,
            depth_format,
            vk::SwapchainKHR::null(),
        ) {
            Ok(swapchain) => swapchain,
            Err(error) => {
                pipes.destroy(&ctx);
                ctx.destroy();
                return Err(error);
            }
        };
        let frames = match FrameState::new(&ctx, swapchain.image_count) {
            Ok(frames) => frames,
            Err(error) => {
                swapchain.destroy(&ctx);
                pipes.destroy(&ctx);
                ctx.destroy();
                return Err(error);
            }
        };
        let stats = RenderStats {
            gpu_name: ctx.gpu_name.clone(),
            device_type: ctx.device_type.clone(),
            api_version: ctx.api_version.clone(),
            present_mode: present_mode_str(swapchain.present_mode).to_string(),
            supported_present_modes: ctx
                .supported_present_modes
                .iter()
                .map(|mode| present_mode_str(*mode).to_string())
                .collect(),
            surface_format: format!("{:?}", swapchain.format),
            extent: (swapchain.extent.width, swapchain.extent.height),
            swapchain_images: swapchain.image_count,
            frames_presented: 0,
            swapchain_recreations: 0,
            validation_active,
            sync_validation,
            gpu_timestamps_supported: frames.timestamps_supported,
            validation_errors: 0,
            validation_warnings: 0,
            recent_validation: Vec::new(),
            region_truncated_vertices: 0,
            region_supplied_peak_vertices: 0,
            region_capacity_vertices: 0,
            overlay_truncated_vertices: 0,
            overlay_supplied_peak_vertices: 0,
            overlay_capacity_vertices: 0,
            instance_capacity: 0,
            instance_peak_used: 0,
            pose_capacity: 0,
            pose_peak_used: 0,
            arena_growth_events: 0,
            population_writes_skipped: 0,
            gpu_timing: TimingSamples::default(),
            depth_format: format!("{:?}", depth_format),
        };
        Ok(Renderer {
            ctx,
            swapchain,
            pipes,
            frames,
            config,
            stats,
            scene: None,
            overlay_enabled: true,
            overlay_color: [0.72, 1.0, 0.78, 0.85],
            overlay_vertices: Vec::new(),
            atlas_text,
            instance_bytes: Vec::new(),
            pose_bytes: Vec::new(),
            population_generations: PopulationGenerations::default(),
            proxy_draw_mode: ProxyDrawMode::default(),
            ripple_debug: RippleDebugMode::Off,
            arena_budgets,
            device_lost: vulkan::DeviceLossLatch::default(),
        })
    }

    /// Renders one frame. `window_size` is the current window size in
    /// pixels — the application owns window state (the Stage 1 shell), and
    /// the renderer never queries SDL after construction. A minimized
    /// window (zero extent) presents nothing. `region_scene` is dynamic
    /// per-frame geometry (Stage 4 graphical integration: region area
    /// plates + NPC render proxies) drawn through the scene pipeline from
    /// a per-frame-slot persistently mapped buffer — the overlay's
    /// dynamic-update pattern; an empty slice draws nothing (the Stage
    /// 2/3 default path). `overlay_text` renders the debug overlay when
    /// overlays are enabled.
    pub fn render_frame(
        &mut self,
        view_proj: &[[f32; 4]; 4],
        region_scene: &[SceneVertex],
        overlay_text: Option<&str>,
        window_size: (u32, u32),
    ) -> Result<FrameResult, RendererError> {
        self.render_layers(
            view_proj,
            region_scene,
            None,
            &[],
            overlay_text,
            window_size,
        )
    }

    /// Draws the world, an optional foreground with independent depth,
    /// and one screen-space batch. World and foreground share the same
    /// scene slot, in that order. HUD and diagnostics share the overlay
    /// slot in that order. At capacity, foreground takes precedence over
    /// world geometry and HUD takes precedence over diagnostics. Omitted
    /// vertex counts are returned in `FrameResult::geometry`.
    /// All supplied batches are triangle lists (three
    /// vertices per primitive). The diagnostics toggle controls only the text.
    pub fn render_layers(
        &mut self,
        view_proj: &[[f32; 4]; 4],
        world: &[SceneVertex],
        foreground: Option<SceneLayer<'_>>,
        hud: &[OverlayVertex],
        debug_text: Option<&str>,
        window_size: (u32, u32),
    ) -> Result<FrameResult, RendererError> {
        // CA-13 (F13/A14): the device-loss entry gate — a latched
        // loss refuses without touching the dead device
        // (`render_frame` delegates here, so this gate covers both
        // entries).
        if let Some(refusal) = self.device_lost.refusal() {
            return Err(refusal);
        }
        // The F-05 overlay-vertex reuse (the audit's measured
        // candidate): the glyph vertices are rebuilt into the
        // renderer's own retained buffer — the capacity from the
        // first frames is reused every frame (no per-frame
        // allocation or growth; the vertex content is identical).
        if self.overlay_enabled {
            if let Some(text) = debug_text {
                self.atlas_text.build_overlay_vertices_into(
                    &mut self.overlay_vertices,
                    text,
                    window_size,
                    self.overlay_color,
                );
            } else {
                self.overlay_vertices.clear();
            }
        } else {
            self.overlay_vertices.clear();
        }
        // The frame flow — the layers-only input set as ONE bundle
        // (A04, design §3.4): the same fifteen positional parameters
        // this call once threaded, with every scene-half input off
        // (the sky layer is a scene-backed-family feature — the
        // submission's `scene.sky` field; the general region path
        // stays skyless. The post chain likewise (the region path
        // stays postless — the layers-only shape's documented
        // scope), and the wet reflection with it).
        let frame = InternalFrame::new(FrameInputs {
            window_size,
            view_proj: *view_proj,
            region_vertices: world,
            foreground,
            hud,
            debug_overlay: &self.overlay_vertices,
            corner_params: None,
            proxy: None,
            shadowed: None,
            sky: None,
            post: false,
            wet: None,
        });
        let policy = self.frame_policy();
        let outcome = match vulkan::render_frame(
            &mut self.ctx,
            &mut self.swapchain,
            &mut self.pipes,
            &mut self.frames,
            policy,
            &frame,
        ) {
            Ok(outcome) => outcome,
            // CA-13 (F13/A14): the error passes the latch on its way
            // out — a device loss (or a terminal slot failure whose
            // SOURCE is one) latches for every later entry.
            Err(error) => {
                self.device_lost.observe(&error);
                return Err(error);
            }
        };
        Ok(self.finish_frame(outcome))
    }

    /// Attaches one scene's authored content (P4 S2 — the design's
    /// §3.2 attach-time surface): the mesh, the material table,
    /// the albedo set, the shadow bounds, and the wet ground's
    /// roles, all as caller data. The description is validated
    /// WHOLE before anything is built (a refusal — an incoherent
    /// mesh/table/roles shape, or a material count outside the
    /// pass families' compiled shape — builds nothing and leaves
    /// the renderer exactly as it was), then uploaded once through
    /// the scene-resource machinery, and the returned
    /// [`SceneHandle`] is the proof the content is live: every
    /// scene-backed frame ([`Self::submit_frame`]'s `scene` half)
    /// draws it.
    ///
    /// ALL-OR-NOTHING (CA-01, the attach-atomicity contract): the
    /// resource upload and the dependent corner-pipeline group are
    /// one STAGED transaction — a failure at ANY creation step
    /// rolls the whole attach back (the staged resources released,
    /// no partial pipeline group, no handle published), so the
    /// renderer is exactly as it was and a RETRY of the same
    /// description is as clean as a first attach.
    ///
    /// ONE scene per renderer lifetime: the device-lifetime pass
    /// families (the retained skinned state, the shadow/reflection
    /// per-material sets) bind the attached mesh/material buffers,
    /// so a second attachment is a typed refusal — construct a
    /// fresh renderer for different content. Since S6 this is the
    /// ONLY way authored scene content reaches the renderer (the
    /// `Corner` construction preference and its migration shim are
    /// deleted); the corner demo and every corner gate attach
    /// their own description through this call, and the second
    /// scene (engine-slice's garage) proves the boundary.
    pub fn attach_scene(&mut self, scene: SceneDescription) -> Result<SceneHandle, RendererError> {
        // CA-13 (F13/A14): the device-loss entry gate — attach is a
        // device-touching entry (uploads, pipeline creation); a
        // latched loss refuses without touching the dead device.
        if let Some(refusal) = self.device_lost.refusal() {
            return Err(refusal);
        }
        // CA-01 (attach atomicity): the attach is a STAGED
        // transaction. The resources install FIRST (the pipeline
        // group binds their descriptor layout), but until the group
        // commits they are STAGING — and if the group's assembly
        // fails, the guard below releases them, leaving the
        // renderer exactly as it was: no installed resources (the
        // context's one-scene budget unconsumed), no partial
        // pipeline group (the assembly stages in a scoped owner and
        // adopts only on full success), no published handle — and
        // the RETRY of the same description is as clean as a first
        // attach. Before this guard, a pipeline failure left the
        // resources installed behind a `None` scene: the retry was
        // refused ("a scene is already attached") and the renderer
        // was unusable for scene-backed frames — the stranded
        // no-scene state the audit's CA-01 named.
        let attached = match attach_description(&mut self.ctx, scene) {
            Ok(attached) => attached,
            // CA-13: the loss latches on its way out (the uploads
            // inside the attach are device work).
            Err(error) => {
                self.device_lost.observe(&error);
                return Err(error);
            }
        };
        // The corner-family pipelines (P4 S6): the scene attaches
        // AFTER construction, so the lit/proxy/rain objects — which
        // bind the ATTACHED scene's descriptor layout — are assembled
        // here (idempotent; the swapchain-recreation path builds them
        // at pipeline-state creation because the scene predates the
        // new state there). Before this ensure, a scene attached
        // after construction left those handles null and the first
        // scene-backed frame bound a null pipeline — found as an
        // access violation on the rig GPU, invisible to every
        // pre-S6 path (the Corner preference attached before
        // construction; the Omitted path never drew the scene
        // family).
        if let Err(error) = self.pipes.ensure_corner_pipelines(&self.ctx) {
            // The rollback guard: the staged resources die here, the
            // pipeline state was never written (the group staging),
            // and the error propagates — the retry re-runs the whole
            // attach from a clean context.
            self.ctx.release_staged_scene_resources();
            // CA-13: the pipeline creation is device work — the loss
            // latches here too.
            self.device_lost.observe(&error);
            return Err(error);
        }
        // Full success — publish the retained facts and the handle
        // together (the one place the attach becomes visible to
        // frames).
        let handle = attached.handle;
        self.scene = Some(attached);
        Ok(handle)
    }

    /// The currently attached scene's handle, if any (the
    /// material-count bound and the attach proof — see
    /// [`SceneHandle`]).
    pub fn scene(&self) -> Option<SceneHandle> {
        self.scene.map(|scene| scene.handle)
    }

    /// Renders one frame from a typed, scene-independent submission
    /// (the P4 D1 contract's entry point — since S6 THE production
    /// entry point for scene-backed frames). The submission is
    /// validated WHOLE before any frame state is touched — a refusal
    /// renders nothing and leaves every capture request intact —
    /// then mapped onto the frame flow's inputs through
    /// [`crate::submission`]'s adapter, which produces the same
    /// parameter set the former per-feature entry points constructed
    /// inline (that equivalence was pinned at the type level by the
    /// submission module's unit test and on-device by the S1 gate's
    /// byte-equality; both pins survive the entries' S6 deletion —
    /// the expectations are hand-built in the tests).
    ///
    /// `scene: None` is the [`Self::render_layers`] shape (dynamic
    /// layers only); `scene: Some(_)` is the scene-backed family's —
    /// the light, the optional shadows/population, and the per-frame
    /// sky/post/wet state (the former sticky setters' fields, per
    /// frame since S3; the stickies themselves are deleted at S6 —
    /// the submission is the one declaration of the frame's passes).
    /// The debug overlay builds exactly like `render_layers`'s path
    /// (the F-05 retained rebuild) whenever `layers.debug_text` is
    /// set. The proxy submission mode and the TEST-ONLY ripple debug
    /// lane stay renderer-level settings, read per frame.
    pub fn submit_frame(
        &mut self,
        submission: FrameSubmission<'_>,
    ) -> Result<FrameResult, RendererError> {
        // CA-13 (F13/A14): the device-loss entry gate — a latched
        // loss refuses without touching the dead device (repeated
        // submissions on a lost device are prevented, not retried).
        if let Some(refusal) = self.device_lost.refusal() {
            return Err(refusal);
        }
        // Scene/attach coherence (the design's §3.3 order step 1):
        // a SceneFrame requires an ATTACHED scene — attach_scene is
        // the only way to bring one (the Corner preference's
        // construction adapter is deleted). The layers-only shape
        // (`scene: None`) needs none. The attached table's length
        // rides down to the adapter for the population validation
        // (0 on the layers-only shape — unread there).
        let material_table = match (&submission.scene, self.scene) {
            (None, _) => 0,
            (Some(_), Some(scene)) => scene.handle.material_count,
            (Some(_), None) => {
                return Err(RendererError::Vulkan(
                    "submit_frame with a scene requires an attached scene \
                     (attach the SceneDescription first)"
                        .into(),
                ));
            }
        };
        // The F-05 overlay-vertex rebuild (render_layers' own
        // discipline, verbatim): the glyph vertices are built into
        // the retained buffer — the capacity from the first frames is
        // reused every frame (no per-frame allocation or growth; the
        // vertex content is identical); no debug text clears it.
        if self.overlay_enabled {
            if let Some(text) = submission.layers.debug_text {
                self.atlas_text.build_overlay_vertices_into(
                    &mut self.overlay_vertices,
                    text,
                    submission.output.window_size,
                    self.overlay_color,
                );
            } else {
                self.overlay_vertices.clear();
            }
        } else {
            self.overlay_vertices.clear();
        }
        // Validate + repack + build the input set (the adapter —
        // device-free, whole-submission validation before the frame
        // flow). The frame policy is collected BEFORE the adapter
        // runs (it reads `self.config` while the adapter's output
        // holds the repack buffers' borrows — disjoint fields, but
        // collected once up front either way: it is `Copy`
        // construction-time config, never state).
        let policy = SubmissionPolicy {
            proxy_draw_mode: self.proxy_draw_mode,
            ripple_debug: self.ripple_debug,
            // CA-12 (F12): the CONSTRUCTION-VALIDATED budgets — the
            // same `u32` numbers the device carve clamps against
            // (widened losslessly to the public repack's `usize`
            // boundary), so the CPU typed refusal and the carve
            // ceiling are one number.
            instance_capacity: self.arena_budgets.instance as usize,
            pose_capacity: self.arena_budgets.pose as usize,
        };
        let frame_policy = self.frame_policy();
        let inputs = adapt_frame(
            policy,
            &submission,
            material_table,
            &self.overlay_vertices,
            &mut self.instance_bytes,
            &mut self.pose_bytes,
            &mut self.population_generations,
        )?;
        // The frame flow — the validated input set as ONE bundle
        // (A04, design §3.4): the same parameter set the former
        // entries handed `vulkan::render_frame` through fifteen
        // positional parameters, with the frame plan derived once
        // at the bundle's construction.
        let frame = InternalFrame::new(inputs);
        let outcome = match vulkan::render_frame(
            &mut self.ctx,
            &mut self.swapchain,
            &mut self.pipes,
            &mut self.frames,
            frame_policy,
            &frame,
        ) {
            Ok(outcome) => outcome,
            // CA-13 (F13/A14): the error passes the latch on its way
            // out — a device loss (or a terminal slot failure whose
            // SOURCE is one) latches for every later entry.
            Err(error) => {
                self.device_lost.observe(&error);
                return Err(error);
            }
        };
        Ok(self.finish_frame(outcome))
    }

    /// Sets the proxy submission mode (the equivalence gate's axis —
    /// see [`ProxyDrawMode`]; the default is the production
    /// `Instanced` mode).
    pub fn set_proxy_draw_mode(&mut self, mode: ProxyDrawMode) {
        self.proxy_draw_mode = mode;
    }

    /// The current proxy submission mode.
    pub fn proxy_draw_mode(&self) -> ProxyDrawMode {
        self.proxy_draw_mode
    }

    /// Sets the TEST-ONLY ripple debug mode for subsequent wet
    /// frames (finding 15b's separation fixtures — see
    /// [`RippleDebugMode`]). The mode rides the packed ripple
    /// uniform's rows[1].y spare lane; it NEVER appears in a
    /// production frame (the default is [`RippleDebugMode::Off`],
    /// the shipped path). The one retained renderer-level policy
    /// the submission adapter reads per frame (with the proxy
    /// mode): applies at the next wet submission's packing.
    pub fn set_ripple_debug_mode(&mut self, mode: RippleDebugMode) {
        self.ripple_debug = mode;
    }

    /// Requests the NEXT shadowed frame's shadow-map readback (the
    /// 6-F known-answer path — the depth-only pass's map, one f32
    /// per texel, row-major). Single-flight like the color capture.
    /// Ensures the shadow state if no shadowed frame has run yet
    /// (the lazy discipline's request-side completion — the request
    /// precedes the frame it wants recorded). A plain frame does NOT
    /// consume the request — it survives until the next SHADOWED
    /// frame runs the depth pass.
    pub fn request_shadow_map_capture(&mut self) -> Result<(), RendererError> {
        // CA-13 (F13/A14): the device-loss entry gate — the shadow
        // state's lazy ensure is device work.
        if let Some(refusal) = self.device_lost.refusal() {
            return Err(refusal);
        }
        if self.pipes.shadows.is_none() {
            if self.scene.is_none() {
                return Err(RendererError::Vulkan(
                    "shadow capture requires an attached scene \
                     (attach the SceneDescription first)"
                        .into(),
                ));
            }
            if let Err(error) = self.pipes.ensure_shadow_resources(&self.ctx) {
                self.device_lost.observe(&error);
                return Err(error);
            }
        }
        vulkan::request_shadow_readback(&mut self.frames)
    }

    /// Takes the completed shadow-map readback, if one is ready (the
    /// recording slot's fence is waited — the color capture's take
    /// discipline). The depths are the UNBIASED light-space depths
    /// the depth-only pass rasterized. Propagates fence-wait errors
    /// (timeout/device lost) so the caller may retry; returns
    /// `CaptureDropped` if the shadow state was replaced after the
    /// readback recorded (the recording buffer was destroyed with it).
    pub fn take_shadow_map_capture(&mut self) -> Result<Option<ShadowMapReadback>, RendererError> {
        // CA-13 (F13/A14): the device-loss entry gate — the readback
        // fence wait is device work.
        if let Some(refusal) = self.device_lost.refusal() {
            return Err(refusal);
        }
        let depths =
            match vulkan::take_shadow_readback(&self.ctx, &mut self.pipes, &mut self.frames) {
                Ok(depths) => depths,
                Err(error) => {
                    self.device_lost.observe(&error);
                    return Err(error);
                }
            };
        Ok(depths.map(|depths| ShadowMapReadback {
            extent: crate::shadows::SHADOW_MAP_EXTENT,
            depths,
        }))
    }

    /// The frame flow's construction-time policy (P4 D2 §3.4, stage
    /// A04): the `RenderConfig` values `vulkan::render_frame` reads
    /// per call, collected as one `Copy` value. The four owning
    /// states stay separate `&mut` parameters (the design's §5.2
    /// rule); this carries ONLY configuration.
    fn frame_policy(&self) -> vulkan::FramePolicy {
        vulkan::FramePolicy {
            present_preference: self.config.present_mode,
            replace_pipelines: self.config.replace_pipelines,
            demo_scene: self.config.demo_scene,
            // CA-12 (F12): the CONSTRUCTION-VALIDATED budgets — the
            // `u32` domain the carves and the repack share (the
            // pre-fix `usize` fields re-narrowed at the carve sites,
            // the audit's two-domains defect).
            instance_capacity: self.arena_budgets.instance,
            pose_capacity: self.arena_budgets.pose,
            region_capacity: self.arena_budgets.region,
        }
    }

    fn finish_frame(&mut self, outcome: vulkan::FrameOutcome) -> FrameResult {
        // F-03 Step 1: accumulate the frame's capacity decisions —
        // cumulative omitted vertices, peak supplied, and the per-slot
        // capacities (region stays 0 until its lazy buffer allocates).
        // Deferred/minimized frames carry all-zero decisions, so they
        // contribute nothing. Nonzero truncation is REPORTED in the run
        // summary, never fatal.
        self.stats.region_truncated_vertices += u64::from(outcome.region.truncated);
        self.stats.region_supplied_peak_vertices = self
            .stats
            .region_supplied_peak_vertices
            .max(outcome.region.supplied);
        self.stats.region_capacity_vertices = self
            .stats
            .region_capacity_vertices
            .max(outcome.region.capacity);
        self.stats.overlay_truncated_vertices += u64::from(outcome.overlay.truncated);
        self.stats.overlay_supplied_peak_vertices = self
            .stats
            .overlay_supplied_peak_vertices
            .max(outcome.overlay.supplied);
        self.stats.overlay_capacity_vertices = self
            .stats
            .overlay_capacity_vertices
            .max(outcome.overlay.capacity);
        // P5 §1.3 (A11/R1): the arena facts, the same accumulate
        // discipline (capacities are maxima — the carves only grow;
        // peaks are maxima; growth events accumulate). Deferred and
        // unpopulated frames carry zeros, contributing nothing.
        self.stats.instance_capacity = self.stats.instance_capacity.max(outcome.instance_capacity);
        self.stats.instance_peak_used =
            self.stats.instance_peak_used.max(outcome.instance_supplied);
        self.stats.pose_capacity = self.stats.pose_capacity.max(outcome.pose_capacity);
        self.stats.pose_peak_used = self.stats.pose_peak_used.max(outcome.pose_supplied);
        self.stats.arena_growth_events += u64::from(outcome.arena_growth_events);
        // P5 §3.3 (A18/R2): the protocol's observable saving —
        // gated-out mapped writes accumulate like the growth events
        // (deferred and unpopulated frames carry zeros).
        self.stats.population_writes_skipped += u64::from(outcome.population_writes_skipped);
        if outcome.presented {
            self.stats.frames_presented += 1;
        }
        if outcome.recreated {
            self.stats.swapchain_recreations += 1;
            self.stats.extent = (self.swapchain.extent.width, self.swapchain.extent.height);
            self.stats.swapchain_images = self.swapchain.image_count;
        }
        if let Some(duration) = outcome
            .gpu_render_ms
            .and_then(|ms| Duration::try_from_secs_f64(ms / 1000.0).ok())
        {
            self.stats.gpu_timing.record(duration);
        }
        // The validation drain is gated (perf batch 3, corrected per
        // the review's P2): an inactive validation layer takes no
        // per-frame mutex round-trip — EXCEPT while environment
        // diagnostics (loader limitations recorded through the queue)
        // are pending, which a cheap atomic check catches; the hint
        // clears after that drain, so the ordinary inactive frame
        // stays round-trip-free without ever suppressing the
        // environment warnings.
        if self.stats.validation_active || environment_pending() {
            for (class, message) in drain_validation_messages() {
                match class {
                    2 => {
                        self.stats.validation_errors += 1;
                        self.push_validation_message(message);
                    }
                    1 => {
                        self.stats.validation_warnings += 1;
                        self.push_validation_message(message);
                    }
                    _ => {}
                }
            }
        }
        FrameResult {
            presented: outcome.presented,
            recreated: outcome.recreated,
            suboptimal: outcome.suboptimal,
            deferred: outcome.deferred,
            gpu_render_ms: outcome.gpu_render_ms,
            geometry: GeometryStats {
                scene_drawn: outcome.region.drawn,
                foreground_drawn: outcome.foreground_drawn,
                overlay_drawn: outcome.overlay.drawn,
                scene_omitted: outcome.region.truncated,
                overlay_omitted: outcome.overlay.truncated,
                dynamic_draw_calls: u32::from(outcome.region.drawn > outcome.foreground_drawn)
                    + u32::from(outcome.foreground_drawn > 0)
                    + u32::from(outcome.overlay.drawn > 0),
                proxy_draw_calls: outcome.proxy_draw_calls,
                indirect_draws_issued: outcome.indirect_draws_issued,
                uploaded_bytes: u64::from(outcome.region.drawn) * size_of::<SceneVertex>() as u64
                    + u64::from(outcome.overlay.drawn) * size_of::<OverlayVertex>() as u64,
            },
        }
    }

    /// Reads cumulative allocation counters on demand.
    pub fn allocations(&self) -> AllocationStats {
        self.ctx.allocations()
    }

    /// Arms a one-shot target-construction fault (the corrective
    /// finding 3+8 verification knob): the ordinal-th matching
    /// fallible step in the named builder returns
    /// [`RendererError::InjectedFault`]. The ordinal is 1-based and
    /// decrements per MATCHING check (across all target builders of
    /// the same site). The plan is consumed only when the ordinal
    /// reaches 0. Diagnostic-only, test-only: never produced by real
    /// device calls. The `&mut self` is intentional: the one-shot
    /// intent is encoded in the type (a caller must mutably borrow to
    /// arm it).
    pub fn inject_target_fault(
        &mut self,
        site: TargetFaultSite,
        stage: TargetFaultStage,
        ordinal: u32,
    ) {
        self.ctx.set_target_fault(site, stage, ordinal);
    }

    /// Arms a one-shot LIFECYCLE fault (the CA-01/CA-10/CA-11
    /// verification knobs — the target-fault seam's own discipline
    /// on the lifecycle site enum): the ordinal-th matching
    /// fallible step named by [`site`](crate::LifecycleFaultSite)
    /// returns [`RendererError::InjectedFault`] instead of its real
    /// device call — a scene-attach creation step, a post-acquire
    /// frame boundary, or an arena growth's idle window. The
    /// ordinal is 1-based and decrements per MATCHING check across
    /// the renderer's lifetime; the plan is consumed when the
    /// ordinal reaches 0. Diagnostic-only, test-only: never
    /// produced by real device calls. The `&mut self` is
    /// intentional (the one-shot intent encoded in the type, the
    /// target seam's own rule).
    pub fn inject_lifecycle_fault(&mut self, site: crate::LifecycleFaultSite, ordinal: u32) {
        self.ctx.set_lifecycle_fault(site, ordinal);
    }

    /// Arms a one-shot acquire-out-of-date injection (finding 6's
    /// verification knob — the driver-forced
    /// `VK_ERROR_OUT_OF_DATE_KHR` acquire path, unreachable from the
    /// test API otherwise). Diagnostic-only, test-only: the plan is
    /// consumed at the first acquire site of the next frame so the
    /// retry acquire inside the out-of-date arm is structurally
    /// real. The `&mut self` is intentional: the one-shot intent is
    /// encoded in the type (a caller must mutably borrow to arm it).
    pub fn inject_acquire_out_of_date(&mut self) {
        self.ctx.set_acquire_out_of_date();
    }

    /// Arms a one-shot DEVICE-LOSS injection (CA-13's verification
    /// knob — F13/A14): the next frame's FIRST acquire site returns
    /// `VK_ERROR_DEVICE_LOST` instead of the real call, so the typed
    /// classification ([`RendererError::DeviceLost`], the operation
    /// context riding the variant) and the entry-gate latch are
    /// exercisable on a live device — real device removal stays a
    /// separate validation question (the audit's own boundary).
    /// Diagnostic-only, test-only: never produced by a real device
    /// call. The `&mut self` is intentional (the one-shot intent
    /// encoded in the type — the seam's own rule).
    pub fn inject_device_loss(&mut self) {
        self.ctx.set_acquire_device_lost();
    }

    pub fn stats(&self) -> &RenderStats {
        &self.stats
    }

    pub fn set_overlay_enabled(&mut self, enabled: bool) {
        self.overlay_enabled = enabled;
    }

    pub fn overlay_enabled(&self) -> bool {
        self.overlay_enabled
    }

    /// Requests that the NEXT rendered frame be copied back to the
    /// host (the bounded frame-capture readback). ONE capture
    /// outstanding at a time: refused with a typed error while an
    /// earlier request is pending or ready-but-unconsumed (the
    /// single-flight lifecycle policy — a second copy could otherwise
    /// overwrite the earlier readback's destination). A capture
    /// REFUSAL never poisons frame synchronization: the frame renders
    /// normally and a later request can try again.
    pub fn request_frame_capture(&mut self) -> Result<(), RendererError> {
        // CA-13 (F13/A14): the device-loss entry gate — a capture on
        // a latched-dead renderer can never complete; refuse without
        // touching the device.
        if let Some(refusal) = self.device_lost.refusal() {
            return Err(refusal);
        }
        vulkan::request_capture(&mut self.frames)
    }

    /// Takes the completed frame capture, if one is ready: waits the
    /// recording frame's submission fence and returns the presented
    /// image's texels (top row first, the swapchain's channel order).
    /// `Ok(None)` when no capture completed. Read promptly — a later
    /// capture overwrites the shared destination buffer.
    pub fn take_frame_capture(&mut self) -> Result<Option<CapturedFrame>, RendererError> {
        // CA-13 (F13/A14): the device-loss entry gate — the fence
        // wait is device work on a dead device.
        if let Some(refusal) = self.device_lost.refusal() {
            return Err(refusal);
        }
        match vulkan::take_capture(&self.ctx, &mut self.frames) {
            Ok(capture) => Ok(capture),
            Err(error) => {
                self.device_lost.observe(&error);
                Err(error)
            }
        }
    }

    /// The most recent capture refusal or drop, if any (`None` in the
    /// ordinary path) — observable for diagnosis. A refused or
    /// dropped capture is never a frame error: the frame rendered
    /// normally regardless.
    pub fn last_capture_refusal(&self) -> Option<&RendererError> {
        vulkan::last_capture_refusal(&self.frames)
    }

    fn push_validation_message(&mut self, message: String) {
        self.stats.recent_validation.push(message);
        let len = self.stats.recent_validation.len();
        if len > 16 {
            self.stats.recent_validation.drain(0..len - 16);
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        vulkan::device_idle(&self.ctx).ok();
        self.frames.destroy(&self.ctx);
        // Framebuffers before the render pass they reference.
        self.swapchain.destroy(&self.ctx);
        self.pipes.destroy(&self.ctx);
        self.ctx.destroy();
    }
}

/// The ripple field's uniform packing (the pure core behind the
/// submission adapter's wet path — device-free, the unit test's
/// target; the former `set_ripple_field` sticky was this function's
/// one-line delegation, and the per-frame `WetFrame` packs it on
/// every wet submission now). Fixed order, one pass, zero
/// allocation: rows[0] =
/// (active count — exact for ≤ 32, the pool's bound, policy speed,
/// policy width, [`RIPPLE_UV_GAIN`]); rows[1] =
/// ([`RIPPLE_ROUGHEN_GAIN`], the [`RippleDebugMode`] uniform lane,
/// 0, 0); rows[2] = the A19 field-extent bound
/// `(center_x, center_z, radius, 0)` — the conservative bounding
/// circle over the LIVE prefix, [`crate::planar::ripple_field_bound`]'s
/// one shared implementation (the shader's early-out gate; the
/// empty prefix packs zeros, moot under the count gate);
/// rows[3 + i] = (center_x, center_z, age, amplitude *
/// (1.0 - age / lifetime) — the fade at read time, the core's
/// pinned order). `None` packs the EMPTY uniform (all zeros — the
/// shader's identity no-op, the rainless wet path's byte-identity;
/// the debug lane reads 0 there too, but the count gate makes the
/// question moot).
///
/// THE (b) TAIL — CPU-SIDE RECEIVER-RECT RING CULLING — DEFERRED,
/// NOT FOLDED (the design §5.3(3)'s optional-tail provision): the
/// cull's plumbing is not cheap where it matters — the packer↔
/// oracle↔shader term-for-term mirror would ALL need the receiver
/// rect (the oracle's gate-side row builders construct rows
/// independently of this packer, so the mirror contract widens
/// with every consumer), and the cull drops NOTHING on every
/// shipped workload: the corner and garage receivers are the full
/// ~80 m ground plate, and every demo, gate, and census ring lands
/// on it. TRIGGER to fold it: a scene whose receiver is a strict
/// subset of the ripple spawn area (the small-puddle shape) — then
/// [`crate::scene_description::GroundRoles`]'s receiver range
/// becomes the world rect at attach (pure description data), one
/// more packer parameter, with the oracle/helper mirrors moved in
/// the same versioned change.
pub(crate) fn pack_ripple_field(
    field: Option<&engine_core::ripple::RippleField>,
    debug: RippleDebugMode,
) -> vulkan::reflection_draw::RippleUniformData {
    let Some(field) = field else {
        return vulkan::reflection_draw::RippleUniformData::empty();
    };
    let policy = field.policy();
    let mut rows = [[0.0f32; 4]; vulkan::reflection_draw::RIPPLE_UNIFORM_ROWS];
    rows[0] = [
        field.active_count() as f32,
        policy.speed,
        policy.width,
        RIPPLE_UV_GAIN,
    ];
    rows[1] = [RIPPLE_ROUGHEN_GAIN, debug.uniform_lane(), 0.0, 0.0];
    // The A19 bound row (the versioned rows[2] claim): the
    // conservative circle over the LIVE prefix only — the shader's
    // loop reads rings[0..count], so the bound covers exactly the
    // rings the loop visits (dead pool slots never widen it).
    let live = &field.rings()[..field.active_count()];
    let (bound_center, bound_radius) =
        crate::planar::ripple_field_bound(live, policy.speed, policy.width);
    rows[2] = [bound_center[0], bound_center[1], bound_radius, 0.0];
    for (i, ring) in field.rings().iter().enumerate() {
        rows[3 + i] = [
            ring.center[0],
            ring.center[1],
            ring.age,
            ring.amplitude * (1.0 - ring.age / policy.lifetime),
        ];
    }
    vulkan::reflection_draw::RippleUniformData { rows }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test-authored ripple policy (the demo's
    /// corner_weather::RIPPLE_POLICY numbers, verbatim): lifetime
    /// 2.5 s, expansion 0.8 m/s, band half-width 0.35 m, peak
    /// amplitude 1.0, one ring per landing drop.
    fn ripple_policy() -> engine_core::ripple::RipplePolicy {
        engine_core::ripple::RipplePolicy {
            lifetime: 2.5,
            speed: 0.8,
            width: 0.35,
            amplitude: 1.0,
            decimation: 1,
        }
    }

    /// THE PACKING KNOWN ANSWERS (the pure core behind the wet
    /// submission path — device-free; the per-frame `WetFrame`
    /// packing is this function's one-line call site, so the None
    /// branch below IS the `WetFrame { ripple: None }` branch): a
    /// RippleField driven
    /// by hand, every expected value written as the SAME f32
    /// expression the packing performs. The layout contract is the
    /// shader's (ground_reflect.wgsl's RippleField block,
    /// term-for-term): rows[0] = (count, speed, width,
    /// tangent-displacement gain), rows[1] = (roughen gain, the
    /// RippleDebugMode lane, 0, 0), rows[2 + i] = (center_x,
    /// center_z, age, faded amplitude). The debug-lane coverage
    /// (finding 15b's test-only separation knobs) pins each mode's
    /// lane value beside the default's 0.0.
    #[test]
    fn ripple_uniform_packing_known_answers() {
        let dt = 1.0f32 / 60.0f32;
        let mut field =
            engine_core::ripple::RippleField::new(ripple_policy()).expect("the policy validates");
        // One observed kill death at a known position, then one
        // tick: the ring's age is exactly `0.0 + dt`.
        assert_eq!(field.observe_death([-3.5, 1.7, 8.25]), None);
        let report = field
            .tick(dt)
            .expect("the fixture dt is finite and nonnegative");
        assert_eq!(report.active, 1);
        let packed = pack_ripple_field(Some(&field), RippleDebugMode::Off);
        assert_eq!(
            packed.rows[0],
            [1.0, 0.8, 0.35, RIPPLE_UV_GAIN],
            "rows[0]: the live count and the policy's speed/width over the uv gain"
        );
        assert_eq!(
            packed.rows[1],
            [RIPPLE_ROUGHEN_GAIN, 0.0, 0.0, 0.0],
            "rows[1]: the roughen gain over the DEFAULT (Off) debug lane 0.0 — the \
             shipped path every production frame packs"
        );
        // The test-only debug lane (finding 15b's separation
        // fixtures): each mode's lane value, the packing's own
        // mapping — the shader's params1.y reads exactly these.
        assert_eq!(
            pack_ripple_field(Some(&field), RippleDebugMode::BlendOnly).rows[1],
            [RIPPLE_ROUGHEN_GAIN, 1.0, 0.0, 0.0],
            "BlendOnly packs lane 1.0 (the UV term suppressed)"
        );
        assert_eq!(
            pack_ripple_field(Some(&field), RippleDebugMode::DisplacementOnly).rows[1],
            [RIPPLE_ROUGHEN_GAIN, 2.0, 0.0, 0.0],
            "DisplacementOnly packs lane 2.0 (the blend term suppressed)"
        );
        let age = 0.0f32 + dt;
        // THE A19 BOUND ROW (the versioned rows[2] claim): the single
        // live ring's own outer reach around its center (the bbox
        // center of one ring IS its center), padded by the ULP travel
        // budget — the same f32 expressions the packing performs.
        let bound_radius =
            (0.0f32 + 0.8f32 * age + 0.35f32) * crate::planar::RIPPLE_BOUND_ULP_TRAVEL;
        assert_eq!(
            packed.rows[2],
            [-3.5, 8.25, bound_radius, 0.0],
            "rows[2]: the field-extent bound — the live prefix's bbox center and \
             the padded conservative radius"
        );
        assert_eq!(
            packed.rows[3],
            [-3.5, 8.25, age, 1.0f32 * (1.0f32 - age / 2.5f32)],
            "rows[3]: the landing column, the accumulated age, the faded amplitude"
        );
        // The unused ring rows stay zero (the shader reads only
        // rings[0..count] — the tail is never observed, but the
        // packing's full-row initialization keeps it exact zeros).
        for row in &packed.rows[4..] {
            assert_eq!(*row, [0.0; 4], "the unused ring rows are zero");
        }

        // More deaths + ticks: the count row and the per-ring ages.
        // Ring 0 has aged 3 ticks; rings 1 and 2 (observed after
        // tick 1) have aged 2 — each age is its OWN f32
        // accumulation, replicated here in the tick order, never
        // n*dt directly.
        assert_eq!(field.observe_death([1.0, 1.7, -2.0]), None);
        assert_eq!(field.observe_death([2.5, 1.7, 4.0]), None);
        let mut age_first = 0.0f32;
        let mut age_rest = 0.0f32;
        age_first += dt; // tick 1 (ring 0 alive, rings 1/2 not yet born)
        age_first += dt; // tick 2 (all three alive)
        age_rest += dt;
        age_first += dt; // tick 3
        age_rest += dt;
        let _report = field
            .tick(dt)
            .expect("the fixture dt is finite and nonnegative"); // tick 2
        let report = field
            .tick(dt)
            .expect("the fixture dt is finite and nonnegative"); // tick 3
        assert_eq!(report.active, 3);
        let packed = pack_ripple_field(Some(&field), RippleDebugMode::Off);
        assert_eq!(packed.rows[0][0], 3.0, "three live rings");
        assert_eq!(packed.rows[0][1], 0.8);
        assert_eq!(packed.rows[0][2], 0.35);
        assert_eq!(packed.rows[0][3], RIPPLE_UV_GAIN);
        // The three-ring bound: the centers' bbox center, the max
        // over the live rings of the padded per-ring reach — the
        // same f32 expressions the packing performs (the offsets in
        // the shader's own sqrt-of-dot form).
        let bc = [(-3.5f32 + 2.5f32) * 0.5, (-2.0f32 + 8.25f32) * 0.5];
        let reach = |center: [f32; 2], ring_age: f32| {
            let dx = center[0] - bc[0];
            let dz = center[1] - bc[1];
            (dx * dx + dz * dz).sqrt() + 0.8f32 * ring_age + 0.35f32
        };
        let bound_radius = 0.0f32
            .max(reach([-3.5, 8.25], age_first))
            .max(reach([1.0, -2.0], age_rest))
            .max(reach([2.5, 4.0], age_rest))
            * crate::planar::RIPPLE_BOUND_ULP_TRAVEL;
        assert_eq!(
            packed.rows[2],
            [bc[0], bc[1], bound_radius, 0.0],
            "rows[2]: the three-ring field-extent bound (the bbox center, the \
             padded max reach)"
        );
        assert_eq!(
            packed.rows[3],
            [
                -3.5,
                8.25,
                age_first,
                1.0f32 * (1.0f32 - age_first / 2.5f32)
            ],
            "ring 0: three accumulated ticks"
        );
        assert_eq!(
            packed.rows[4],
            [1.0, -2.0, age_rest, 1.0f32 * (1.0f32 - age_rest / 2.5f32)],
            "ring 1: two accumulated ticks"
        );
        assert_eq!(
            packed.rows[5],
            [2.5, 4.0, age_rest, 1.0f32 * (1.0f32 - age_rest / 2.5f32)],
            "ring 2: two accumulated ticks"
        );
        for row in &packed.rows[6..] {
            assert_eq!(*row, [0.0; 4]);
        }
        // Double-run determinism (the rig gates' byte-equality's CPU
        // half): the packing is a pure function of the field and the
        // debug mode — packing the same field again yields the
        // identical rows, the bound row included.
        assert_eq!(
            pack_ripple_field(Some(&field), RippleDebugMode::Off),
            packed,
            "packing the same field twice is byte-identical"
        );

        // The rainless wet path — the exact branch it runs — packs
        // the EMPTY uniform (all zeros; the shader's count gate
        // makes it the bit-identity no-op).
        let cleared = pack_ripple_field(None, RippleDebugMode::DisplacementOnly);
        assert_eq!(
            cleared,
            vulkan::reflection_draw::RippleUniformData::empty(),
            "None clears to the empty uniform — the debug mode cannot leak into it \
             (all zeros, lane included)"
        );
        for row in &cleared.rows {
            assert_eq!(*row, [0.0; 4], "the empty rows are all zeros");
        }
    }
}
