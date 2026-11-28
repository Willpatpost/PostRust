//! Frame synchronization and commands (P4 S4 — design §5.1's frame
//! row): `FrameState` — per-slot command buffers, fences, per-image
//! present-wait semaphores, timestamps, overlay slots — the frame
//! outcome, and the full wait -> acquire -> (recreate) -> record ->
//! submit -> present flow (`render_frame`, the coordinator's host
//! half). The S4 split was a pure move from vulkan.rs; A04 (design
//! §3.4 + §5.4) then collapsed the flow's parameter set and named
//! its phases — no behavior change in either step:
//!
//! **The §3.4 collapse (19 → 6):** the four owning states stay four
//! explicit `&mut` parameters (the ownership argument the original
//! parameter-list comment defended — the design agrees with it and
//! disagrees only about the input half); the fifteen input/policy
//! parameters collapsed into TWO values: [`FramePolicy`]
//! (construction-time config, `Copy`) and [`InternalFrame`] (the
//! validated, repacked submission one level down, plus the [`FramePlan`]
//! derived from it — a borrow bundle). The same Options and slices
//! flow through, named by one struct instead of positional
//! parameters; the preparation pair and `recreate_swapchain` take
//! the same two values (the policy one level down each time).
//!
//! **The §5.4 phases:** `render_frame` is now the phase ladder in
//! call order — `validate_proxy_materials` → `prepare_frame` (the
//! S4-extracted preparation pair) → `wait_slot_fence` →
//! `acquire_with_retry` → `guard_image_and_read_timestamps` →
//! `upload_frame_data` → `present_semaphore_for` →
//! `record_command_buffer` → `commit_slot_fence` →
//! `submit_command_buffer` → `present_image`, with phases 5-8
//! grouped in [`acquire_to_submit`] and every error inside them
//! routed through [`dispose_failed_acquisition`] (CA-10's
//! acquisition disposition — the slot fence's reset is DEFERRED to
//! the commit between record and submit, so no post-acquire failure
//! can strand the slot). Each phase is a
//! private function of this module — not a phase trait, not
//! extensible at runtime; the ordering is the code. Where S3/S4 had
//! already landed a piece of a phase (the preparation pair, the
//! per-family record functions, the plan), the phase below is the
//! remaining wiring only — each phase's doc names the overlap.
//!
//! **Zero-cost statement (§3.4):** [`FramePolicy`] and
//! [`InternalFrame`] are `Copy` borrow bundles — no `Vec`, no `Box`,
//! no trait objects; construction is stack assembly the compiler
//! elides. No per-frame heap allocation and no dyn dispatch were
//! introduced anywhere in the flow. The behavioral half of the proof
//! is the batteries' identical `FrameOutcome`s and allocation
//! counters (the A04 gate set); the type-level half is
//! `policy_tests::the_collapsed_frame_parameters_are_copy_stack_data`.

use ash::vk;

use super::RendererError;
use super::WAIT_TIMEOUT_NS;
use super::capture::{CaptureState, ShadowReadbackState};
use super::context::VulkanContext;
use super::frame_plan::FramePlan;
use super::frame_prepare::{FramePreparationResult, prepare_after_recreation, prepare_frame};
use super::pipelines::PipelineState;
use super::resources::create_mapped_buffer;
use super::swapchain::{SwapchainState, mark_suboptimal};
use super::uploads::{LayerPriority, upload_layers};
use super::{corner, post_draw, proxy_draw, scene_record};
use crate::config::{DemoScenePreference, DynamicClamp, FRAMES_IN_FLIGHT, PresentModePreference};
use crate::overlay::OverlayVertex;
use crate::submission::FrameInputs;

/// A single frame slot owns this mapping. The slot fence must have
/// signaled before growing or writing it.
#[derive(Default)]
struct OverlaySlot {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: *mut core::ffi::c_void,
    capacity: u32,
}

impl OverlaySlot {
    fn capacity_for(current: u32, required: usize) -> u32 {
        let limit = crate::overlay::overlay_capacity_vertices() / FRAMES_IN_FLIGHT;
        let required = required.min(limit as usize) as u32;
        if required <= current {
            return current;
        }
        required.next_power_of_two().min(limit)
    }

    fn reserve(&mut self, ctx: &VulkanContext, required: usize) -> Result<(), RendererError> {
        let capacity = Self::capacity_for(self.capacity, required);
        if capacity != self.capacity {
            let bytes = u64::from(capacity) * size_of::<OverlayVertex>() as u64;
            let (buffer, memory, mapped) =
                create_mapped_buffer(ctx, vk::BufferUsageFlags::VERTEX_BUFFER, bytes)?;
            self.destroy(ctx);
            *self = Self {
                buffer,
                memory,
                mapped,
                capacity,
            };
        }
        Ok(())
    }

    fn destroy(&mut self, ctx: &VulkanContext) {
        if self.capacity > 0 {
            unsafe {
                ctx.device.unmap_memory(self.memory);
                ctx.device.destroy_buffer(self.buffer, None);
                ctx.device.free_memory(self.memory, None);
            }
            *self = Self::default();
        }
    }
}

/// Sync objects, command buffers, and the timestamp pool, per frames-in-
/// flight slot, plus per-swapchain-image state.
pub(crate) struct FrameState {
    overlay_slots: [OverlaySlot; FRAMES_IN_FLIGHT as usize],
    /// Created with `RESET_COMMAND_BUFFER` (corrective finding D): the
    /// per-slot command buffers are re-recorded every frame, and the
    /// per-slot fence proves a buffer is no longer pending when its
    /// re-record begins.
    pub command_pool: vk::CommandPool,
    pub command_buffers: Vec<vk::CommandBuffer>,
    pub image_available: Vec<vk::Semaphore>,
    /// Presentation-wait semaphores indexed by **swapchain image**
    /// (corrective finding C): the submit fence proves the graphics work
    /// finished, never that the presentation engine stopped waiting on
    /// the present semaphore — slot-indexed reuse could therefore signal
    /// a semaphore with an outstanding wait. Indexing by image ties reuse
    /// to re-acquisition of that image. Grown on demand; released only
    /// after `device_wait_idle`.
    pub present_semaphores: Vec<vk::Semaphore>,
    pub in_flight: Vec<vk::Fence>,
    pub images_in_flight: Vec<vk::Fence>,
    pub timestamp_pool: vk::QueryPool,
    /// False when the graphics queue family exposes no timestamp bits
    /// (corrective finding E).
    pub timestamps_supported: bool,
    /// Whether this slot's queries have resolved at least once (finding
    /// E): never block-read unwritten queries.
    pub timestamps_written: [bool; FRAMES_IN_FLIGHT as usize],
    /// A present reported suboptimal — recreate before the next frame.
    pub needs_recreate: bool,
    /// The 6-F shadow-map readback handshake (single-flight, like
    /// the color capture; survives pipeline replacements).
    pub(crate) shadow_readback: ShadowReadbackState,
    /// Hint-gated surface probing (perf batch 3): true while the
    /// surface's drawable is believed unavailable (the previous frame
    /// deferred), so minimized/transitional surfaces keep probing
    /// until valid while ordinary frames skip the per-frame driver
    /// round-trip entirely.
    pub surface_probe_pending: bool,
    pub frame_slot: usize,
    pub image_index: u32,
    /// The bounded frame-capture readback (lazily allocated on first
    /// request — the orientation corrective's evidence path).
    pub capture: CaptureState,
}

impl FrameState {
    pub(crate) fn new(
        ctx: &VulkanContext,
        swapchain_image_count: u32,
    ) -> Result<FrameState, RendererError> {
        // Partial-initialization cleanup (corrective finding G).
        let mut state = FrameState {
            overlay_slots: std::array::from_fn(|_| OverlaySlot::default()),
            command_pool: vk::CommandPool::null(),
            command_buffers: Vec::new(),
            image_available: Vec::new(),
            present_semaphores: Vec::new(),
            in_flight: Vec::new(),
            images_in_flight: vec![vk::Fence::null(); swapchain_image_count as usize],
            timestamp_pool: vk::QueryPool::null(),
            timestamps_supported: ctx.timestamp_valid_bits > 0,
            timestamps_written: [false; FRAMES_IN_FLIGHT as usize],
            needs_recreate: false,
            surface_probe_pending: false,
            shadow_readback: ShadowReadbackState::default(),
            frame_slot: 0,
            image_index: 0,
            capture: CaptureState::dead(),
        };
        if let Err(error) = state.build(ctx, swapchain_image_count) {
            state.destroy(ctx);
            return Err(error);
        }
        Ok(state)
    }

    fn build(
        &mut self,
        ctx: &VulkanContext,
        swapchain_image_count: u32,
    ) -> Result<(), RendererError> {
        let pool_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
            .queue_family_index(ctx.graphics_family);
        self.command_pool = unsafe { ctx.device.create_command_pool(&pool_info, None) }
            .map_err(|error| RendererError::vk(error, "create_command_pool"))?;
        let allocate_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(FRAMES_IN_FLIGHT);
        self.command_buffers = unsafe { ctx.device.allocate_command_buffers(&allocate_info) }
            .map_err(|error| RendererError::vk(error, "allocate_command_buffers"))?;

        for _ in 0..FRAMES_IN_FLIGHT {
            let semaphore = unsafe {
                ctx.device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
            }
            .map_err(|error| RendererError::vk(error, "create_semaphore"))?;
            self.image_available.push(semaphore);
            let fence = unsafe {
                ctx.device.create_fence(
                    &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                    None,
                )
            }
            .map_err(|error| RendererError::vk(error, "create_fence"))?;
            self.in_flight.push(fence);
        }
        self.sync_present_semaphores(ctx, swapchain_image_count)?;

        if self.timestamps_supported {
            let query_info = vk::QueryPoolCreateInfo::default()
                .query_type(vk::QueryType::TIMESTAMP)
                .query_count(FRAMES_IN_FLIGHT * 2);
            self.timestamp_pool = unsafe { ctx.device.create_query_pool(&query_info, None) }
                .map_err(|error| RendererError::vk(error, "create_query_pool"))?;
        }
        Ok(())
    }

    /// Grows the per-image presentation-wait semaphore table (corrective
    /// finding C). Entries are never removed — recreation idles the
    /// device first, so outstanding present waits have drained.
    fn sync_present_semaphores(
        &mut self,
        ctx: &VulkanContext,
        image_count: u32,
    ) -> Result<(), RendererError> {
        let needed = present_semaphore_len_for(self.present_semaphores.len(), image_count as usize);
        while self.present_semaphores.len() < needed {
            let semaphore = unsafe {
                ctx.device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
            }
            .map_err(|error| RendererError::vk(error, "create present semaphore"))?;
            self.present_semaphores.push(semaphore);
        }
        Ok(())
    }

    /// Returns the present-wait semaphore for the acquired image
    /// (corrective finding C), growing the table if the index exceeds it.
    fn present_semaphore_for(
        &mut self,
        ctx: &VulkanContext,
        image_index: u32,
    ) -> Result<vk::Semaphore, RendererError> {
        // CA-10 fault knob: the PRESENT-SEMAPHORE boundary (Phase
        // 6.5, post-acquire) — a plan armed here fires before the
        // creation, with the slot fence still signaled and nothing
        // recorded (the fail-fast position this growth already
        // holds).
        ctx.check_lifecycle_fault(crate::LifecycleFaultSite::FramePresentSemaphore)?;
        self.sync_present_semaphores(ctx, image_index + 1)?;
        Ok(self.present_semaphores[image_index as usize])
    }

    /// Re-syncs the per-image tables for a new swapchain (the image count
    /// may change across recreation).
    pub(crate) fn resize_images(
        &mut self,
        ctx: &VulkanContext,
        swapchain_image_count: u32,
    ) -> Result<(), RendererError> {
        self.images_in_flight = vec![vk::Fence::null(); swapchain_image_count as usize];
        self.sync_present_semaphores(ctx, swapchain_image_count)
    }

    pub(crate) fn destroy(&mut self, ctx: &VulkanContext) {
        unsafe {
            ctx.device.destroy_query_pool(self.timestamp_pool, None);
            for fence in &self.in_flight {
                ctx.device.destroy_fence(*fence, None);
            }
            self.in_flight.clear();
            for semaphore in &self.present_semaphores {
                ctx.device.destroy_semaphore(*semaphore, None);
            }
            self.present_semaphores.clear();
            for semaphore in &self.image_available {
                ctx.device.destroy_semaphore(*semaphore, None);
            }
            self.image_available.clear();
            self.capture.destroy(ctx);
            for overlay in &mut self.overlay_slots {
                overlay.destroy(ctx);
            }
            ctx.device.destroy_command_pool(self.command_pool, None);
        }
    }
}

/// Outcome of one `render_frame` call.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct FrameOutcome {
    pub presented: bool,
    pub recreated: bool,
    pub suboptimal: bool,
    /// The drawable was temporarily unavailable (zero-sized surface
    /// capabilities) — nothing was created, destroyed, or presented.
    pub deferred: bool,
    pub gpu_render_ms: Option<f64>,
    /// F-03 Step 1: the frame's region-scene and overlay capacity
    /// decisions (supplied / per-slot capacity / drawn / omitted). Both
    /// stay default — all zero — on deferred or minimized frames, which
    /// record nothing; the overlay capacity is the current slot's
    /// allocation, while the region capacity
    /// reads 0 until the lazy region buffer allocates. Nonzero omitted
    /// counts are REPORTED upstream, never fatal here.
    pub region: DynamicClamp,
    pub foreground_drawn: u32,
    pub overlay: DynamicClamp,
    /// The proxy draw calls recorded this frame (the 6-E headline:
    /// one per populated kind instanced; one per instance in the
    /// per-object reference). Zero on unpopulated and deferred
    /// frames. The corner mesh's own material draws are NOT counted
    /// here — this counts proxy submission only.
    pub proxy_draw_calls: u32,
    /// vkCmdDrawIndirect issuances THIS frame (the 6-E2 issuance
    /// observation — the delta of the context's monotonic counter
    /// across this frame's recording). One per populated kind in
    /// the indirect mode and ZERO in every other mode; zero on a
    /// deferred frame. Distinguishes the indirect submission from
    /// the direct one, which the draw-call count deliberately
    /// cannot (the release fallback draws the identical geometry
    /// through cmd_draw — this field is what observes the
    /// difference).
    pub indirect_draws_issued: u32,
    /// P5 §1.3 (A11/R1): the frame's instance-arena facts — the
    /// population supplied (npc + vehicle + rain) and the carve's
    /// per-slot capacity AFTER this frame's ensure (0 on unpopulated
    /// and deferred frames, the region discipline). The safe layer
    /// accumulates them into `RenderStats`' capacity/peak-used
    /// fields.
    pub instance_supplied: u32,
    pub instance_capacity: u32,
    /// P5 §1.3: the frame's pose-arena facts — blocks supplied and
    /// the carve's per-slot block capacity after the ensure (0 on
    /// unpopulated and deferred frames).
    pub pose_supplied: u32,
    pub pose_capacity: u32,
    /// P5 §1.6: arena growth events THIS frame (the delta of the
    /// context's monotonic counter — the indirect-issuance
    /// observation's exact discipline). A growth event is a carve
    /// RE-CREATION at a larger size (the initial demand-sized carve
    /// is not an event: nothing existed to replace, nothing could
    /// have referenced it, no stall); each event stalls the frame
    /// once under `device_wait_idle`, bounded by the power-of-two
    /// quantum to at most `log2(cap / 64)` per session. Deferred
    /// frames record nothing at their own return; the counter is
    /// monotonic, so a growth that preceded an acquire-time
    /// deferral (the one deferral arm after the ensure ladder)
    /// lands on the NEXT frame's delta — no event is ever lost,
    /// at most attributed to the following frame.
    pub arena_growth_events: u32,
}

/// Construction-time frame policy (P4 D2 §3.4, stage A04): the
/// `RenderConfig` values the frame flow reads per call, collapsed
/// from three positional parameters into one `Copy` value — the
/// policy half of the 19 → 6 parameter collapse. The four owning
/// states stay four explicit `&mut` parameters through every
/// internal function (the design's §5.2 anti-god-object rule); this
/// struct carries ONLY configuration, never state.
///
/// **The design's `scene_attached` adaptation (documented):** §3.4
/// sketched the third field as `scene_attached: bool`, written when
/// the `Corner` construction preference was the scene-carrying arm.
/// S6 deleted that arm — authored scene content now reaches the
/// renderer only through `attach_scene`, and the attached handle's
/// state IS `ctx.corner` (read directly by the flow; §3.4's own
/// "the attached handle's state (was ctx.corner)" note). The
/// preference that remains is the static Stage-2 demonstration
/// scene's `Included` opt-in (design §3.2 keeps it), and the
/// pipeline-replacement path consumes it
/// (`recreate_swapchain` → `PipelineState::new`) — so the policy
/// carries the preference itself, verbatim, and the attached-scene
/// fact needs no boolean mirror.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FramePolicy {
    /// The present-mode preference a recreation re-resolves against
    /// (`RenderConfig::present_mode`).
    pub present_preference: PresentModePreference,
    /// The verification seam that forces pipeline replacement on
    /// every recreation (`RenderConfig::replace_pipelines`,
    /// `--replace-pipelines`).
    pub replace_pipelines: bool,
    /// The static demonstration scene's opt-in (P3,
    /// `RenderConfig::demo_scene`) — the LIVE preference every
    /// replacement pipeline is built with.
    pub demo_scene: DemoScenePreference,
    /// P5 §1.3 (A11/R1): the instance arena's configured cap
    /// (`RenderConfig::instance_capacity`) — the lazy-ensure
    /// ladder's growth ceiling for the instance carve.
    pub instance_capacity: usize,
    /// P5 §1.3: the pose arena's configured cap
    /// (`RenderConfig::pose_capacity`) — the retained pose storage's
    /// growth ceiling.
    pub pose_capacity: usize,
    /// P5 §1.3: the region arena's configured per-slot vertex budget
    /// (`RenderConfig::region_capacity`) — the region carve's growth
    /// ceiling AND the F-03 clamp's boundary (unchanged semantics:
    /// truncation above it stays fail-visible with the counters).
    pub region_capacity: u32,
}

/// One validated frame, one level down (P4 D2 §3.4, stage A04): the
/// input half of a `render_frame` call as ONE value — the
/// [`FrameInputs`] the safe layer's adapter built from a validated
/// `FrameSubmission` (S1's equivalence target), plus the
/// [`FramePlan`] derived from it ONCE (S3's single derivation, now
/// at this bundle's construction). This is the frame half of the
/// 19 → 6 collapse: the same Options and slices that flowed through
/// fifteen positional parameters, named by one struct.
///
/// **Zero-cost statement (§3.4):** a borrow bundle — `Copy`, no
/// `Vec`, no `Box`, no trait objects; construction is stack assembly
/// the compiler elides. The batteries' identical `FrameOutcome`s and
/// allocation counters are the behavioral half of the proof.
///
/// **The design's field-list adaptation (documented):** §3.4 sketched
/// `{ submission, scene, ripple_debug }` — pre-S1 text, written
/// before the submission contract landed. The landed shape: the
/// SUBMISSION arrives already validated and repacked (the adapter
/// runs in the safe layer, device-free, per the §3.3
/// validate-whole-before-any-frame-state invariant — `FrameInputs`
/// is "the validated submission, one level down"); the SCENE stays
/// `ctx.corner` — a reference carved out of the context cannot
/// coexist with the `&mut ctx` the four-state ownership rule
/// requires, and post-S6 `ctx.corner` IS the attached handle's
/// state; and `ripple_debug` is folded at adaptation time into the
/// wet camera's packed ripple uniform (the retained test-only
/// sticky's only consumer, through `adapt_frame`'s policy
/// parameter).
#[derive(Debug, Clone, Copy)]
pub(crate) struct InternalFrame<'a> {
    /// The validated, repacked input set (the safe layer adapter's
    /// output — `adapt_frame`, or the layers-only literal).
    pub inputs: FrameInputs<'a>,
    /// The frame's explicit pass-uses declaration, derived once at
    /// construction (S3's `FramePlan`, consumed by the preparation
    /// pair and the record cascade).
    pub plan: FramePlan,
}

impl<'a> InternalFrame<'a> {
    /// Builds the flow's frame bundle from the adapter's output,
    /// deriving the [`FramePlan`] ONCE — the S3 single derivation,
    /// performed at exactly the same point relative to the flow as
    /// when it sat inline at the top of `render_frame`'s body
    /// (before any synchronization state is touched, on the same
    /// flat facts every route hands the frame flow).
    pub(crate) fn new(inputs: FrameInputs<'a>) -> Self {
        // THE FRAME PLAN (P4 D2 §4.2, stage S3): the explicit pass-uses
        // declaration, derived ONCE here from the input set — the same
        // flat facts every route hands the frame flow. The THREE
        // preparation sites below (the pre-recreation ensure, the
        // Recreated arm's re-run, and the out-of-date path's
        // re-preparation) and the record cascade's pass-variant decisions
        // (the post/main branch, the wet ground's placement, the overlay
        // upload gate) consume it instead of re-deriving the uses from
        // the positional Options — the preparation triplication the
        // design's §1.4 finding named. The equivalence to the implicit
        // derivation this flow previously performed at each site is
        // pinned by `frame_plan`'s unit test (the S1/S2 pattern).
        let foreground_vertices = inputs.foreground.map_or(&[][..], |layer| layer.vertices);
        let plan = FramePlan::derive(
            inputs.region_vertices,
            foreground_vertices,
            inputs.hud,
            inputs.debug_overlay,
            inputs.proxy.as_ref(),
            inputs.shadowed.as_ref(),
            inputs.sky.as_ref(),
            inputs.post,
            inputs.wet.as_ref(),
        );
        Self { inputs, plan }
    }
}

/// The acquire phase's result (§5.4): an acquired image, or the
/// deferral signal (every deferred arm inside `acquire_with_retry`
/// already latched what it owes — the recreation intent in
/// `frames.needs_recreate` — exactly as the inline arms did).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Acquisition {
    Acquired {
        image_index: u32,
        suboptimal: bool,
        /// The out-of-date arm performed the inline recreation (the
        /// public `recreated` contract: callers learn the generation
        /// changed).
        recreated: bool,
    },
    Deferred,
}

/// The full frame flow: wait → acquire → (recreate if needed) → record →
/// submit → present. Called by the safe [`crate::Renderer`] wrapper.
// Private frame orchestration (P4 D2 §3.4, stage A04): the four owning
// states stay four explicit `&mut` parameters — the ownership argument
// this list always defended — and the fifteen input/policy parameters
// collapsed into the `Copy` policy and the borrow-only frame bundle
// (see [`FramePolicy`] and [`InternalFrame`]). The body below is the
// §5.4 phase ladder in call order; each phase function carries its own
// verbatim-moved comments and safety arguments.
pub(crate) fn render_frame(
    ctx: &mut VulkanContext,
    swapchain: &mut SwapchainState,
    pipes: &mut PipelineState,
    frames: &mut FrameState,
    policy: FramePolicy,
    frame: &InternalFrame<'_>,
) -> Result<FrameOutcome, RendererError> {
    let mut outcome = FrameOutcome::default();
    let slot = frames.frame_slot;
    // The 6-E2 issuance observation's frame baseline: the recording
    // below (this frame's command buffer) increments the context's
    // monotonic vkCmdDrawIndirect counter at each call site; the
    // delta lands on the outcome after the submission. Deferred and
    // refused frames record nothing, so their delta is zero.
    let indirect_issued_before = ctx.indirect_draws_issued.get();
    // P5 §1.6's arena-growth baseline, the same discipline: the
    // lazy-ensure ladder (prepare, below) may re-create a carve at a
    // larger size under `device_wait_idle`; the monotonic counter's
    // delta is this frame's growth events. Deferred frames record
    // nothing at their own return; a growth that preceded an
    // acquire-time deferral lands on the next frame's delta (the
    // counter is monotonic — no event is lost).
    let arena_growth_before = ctx.arena_growth_events.get();

    // Phase 1 — validate_inputs: the proxy material validation,
    // before ANY synchronization state is touched.
    let proxy_sets = validate_proxy_materials(ctx, frame.inputs.proxy.as_ref())?;

    // Phase 2 — prepare: swapchain checks, lazy state, and target
    // creation before acquire (finding 20, Phase A; findings 3-8).
    // The coordinator owns the outcome flags: the preparation pair
    // reports Deferred/Recreated/Proceeded and this match sets
    // FrameOutcome::{deferred, recreated} from those arms.
    match prepare_frame(ctx, swapchain, pipes, frames, policy, frame)? {
        FramePreparationResult::Deferred => {
            outcome.deferred = true;
            return Ok(outcome);
        }
        FramePreparationResult::Recreated => {
            outcome.recreated = true;
            // Re-run preparation against the new generation.
            match prepare_frame(ctx, swapchain, pipes, frames, policy, frame)? {
                FramePreparationResult::Deferred => {
                    outcome.deferred = true;
                    return Ok(outcome);
                }
                FramePreparationResult::Recreated => {
                    // This should never happen - recreate once per frame.
                    // If it does, we'll retry in the acquire loop.
                    frames.needs_recreate = true;
                    outcome.deferred = true;
                    return Ok(outcome);
                }
                FramePreparationResult::Proceeded => {}
            }
        }
        FramePreparationResult::Proceeded => {}
    }

    // Phase 3 — wait_slot_fence (plus this slot's overlay growth,
    // which the design's §1.2 inventory grouped with the wait).
    wait_slot_fence(ctx, frames, frame, slot)?;

    // Phase 4 — acquire_with_retry: the out-of-date inline recreation
    // and the retry acquire live inside.
    let acquisition = acquire_with_retry(ctx, swapchain, pipes, frames, policy, frame, slot)?;
    let (image_index, suboptimal) = match acquisition {
        Acquisition::Acquired {
            image_index,
            suboptimal,
            recreated,
        } => {
            outcome.recreated |= recreated;
            (image_index, suboptimal)
        }
        Acquisition::Deferred => {
            outcome.deferred = true;
            return Ok(outcome);
        }
    };
    outcome.suboptimal = suboptimal;
    frames.image_index = image_index;

    // Phases 5-8 — `acquire_to_submit`: everything between the
    // acquisition and the submission. CA-10 (the stranded slot): the
    // slot fence's RESET and the per-image ownership PUBLISH are
    // DEFERRED to the commit step between record and submit (see
    // [`commit_slot_fence`]), so every fallible step below runs with
    // the fence still carrying its previous signal — a failure at
    // ANY of them cannot strand the slot. Any error inside gets the
    // acquisition's disposition (the release in
    // [`dispose_failed_acquisition`]) instead of propagating raw.
    let submitted = acquire_to_submit(
        ctx,
        swapchain,
        pipes,
        frames,
        frame,
        proxy_sets,
        &mut outcome,
        slot,
        image_index,
    );
    let (gpu_render_ms, _world_drawn, present_semaphore) = match submitted {
        Ok(triple) => triple,
        Err(error) => {
            return Err(dispose_failed_acquisition(
                ctx,
                swapchain,
                frames,
                slot,
                image_index,
                error,
            ));
        }
    };

    // Phase 9 — present.
    present_image(
        ctx,
        swapchain,
        frames,
        image_index,
        present_semaphore,
        &mut outcome,
    )?;

    outcome.gpu_render_ms = gpu_render_ms;
    outcome.indirect_draws_issued =
        (ctx.indirect_draws_issued.get() - indirect_issued_before) as u32;
    outcome.arena_growth_events = (ctx.arena_growth_events.get() - arena_growth_before) as u32;
    frames.frame_slot = (slot + 1) % FRAMES_IN_FLIGHT as usize;
    Ok(outcome)
}

/// Phases 5-8 (§5.4, CA-10's restructure): the per-image guard wait
/// and timestamp read, the frame's uploads, the present-wait
/// semaphore, the command-buffer recording, the slot-fence COMMIT
/// (the deferred reset + per-image ownership publish), and the
/// submission. The slot fence stays SIGNALLED through everything
/// before the commit — the property that makes every error below
/// strand-free — and the commit is the last fallible step before
/// the submit (the reset itself is the one CA-10 boundary whose
/// failure leaves the fence state indeterminate; its disposition is
/// the fence-state discriminator in [`dispose_failed_acquisition`]).
#[allow(clippy::too_many_arguments)]
fn acquire_to_submit(
    ctx: &VulkanContext,
    swapchain: &SwapchainState,
    pipes: &PipelineState,
    frames: &mut FrameState,
    frame: &InternalFrame<'_>,
    proxy_sets: Option<[vk::DescriptorSet; 3]>,
    outcome: &mut FrameOutcome,
    slot: usize,
    image_index: u32,
) -> Result<(Option<f64>, u32, vk::Semaphore), RendererError> {
    // Phase 5 — guard_and_timestamps: the per-image guard wait (the
    // LAST wait that can reference this slot's previous signal —
    // the reset is still deferred) and the previous use's timestamp
    // read (Phase 3's slot-fence wait proved them resolved).
    let gpu_render_ms = guard_image_and_read_timestamps(ctx, frames, slot, image_index)?;

    // Phase 6 — upload_frame_data: the mapped-slot writes this
    // slot's fence wait now owns (infallible today — the Result is
    // the boundary contract; the CA-10 fault knob sits at its top).
    let world_drawn = upload_frame_data(ctx, pipes, frames, frame, outcome, slot)?;

    // The present-wait semaphore is owned by the acquired image
    // (corrective finding C). Grown BEFORE recording begins (the
    // fail-fast discipline: a creation failure must not leave a
    // recorded-but-unsubmitted command buffer against a reset fence
    // and an acquired image) — and since CA-10, BEFORE the fence
    // commit as well, so the failure stays strand-free by
    // construction.
    let present_semaphore = frames.present_semaphore_for(ctx, image_index)?;

    // Phase 7 — record: the pass cascade into this slot's command
    // buffer.
    record_command_buffer(
        ctx,
        swapchain,
        pipes,
        frames,
        frame,
        proxy_sets,
        slot,
        image_index,
        world_drawn,
        outcome,
    )?;

    // Phase 7.5 — the slot-fence COMMIT (CA-10): the reset and the
    // per-image ownership publish, deferred until every fallible
    // step before the submission has succeeded.
    commit_slot_fence(ctx, frames, slot)?;

    // Phase 8 — submit.
    submit_command_buffer(ctx, frames, present_semaphore, slot)?;

    Ok((gpu_render_ms, world_drawn, present_semaphore))
}

/// Phase 1 — `validate_inputs` (§5.4): the proxy materials are
/// validated before ANY synchronization state is touched (the
/// capture-refusal discipline: a refusal here can never leave a fence
/// reset-but-unsignaled or an acquired image's semaphore unconsumed).
/// A populated kind whose material index falls outside the attached
/// scene's table is a typed error; an EMPTY kind may carry any index
/// (it draws nothing). The Era-2 package C rain kind joins the
/// validation exactly like the other kinds. Extracted verbatim from
/// `render_frame`'s body at A04; `ctx` is an immutable reborrow — the
/// validation only reads the attached scene's material table.
fn validate_proxy_materials(
    ctx: &VulkanContext,
    proxy: Option<&proxy_draw::ProxyFrameUpload<'_>>,
) -> Result<Option<[vk::DescriptorSet; 3]>, RendererError> {
    let proxy_sets = match (ctx.corner.as_ref(), proxy) {
        (Some(corner), Some(upload)) => {
            let npc_set = corner.material_set(upload.npc_material);
            let vehicle_set = corner.material_set(upload.vehicle_material);
            let rain_set = corner.material_set(upload.rain_material);
            if (upload.npc_count > 0 && npc_set.is_none())
                || (upload.vehicle_count > 0 && vehicle_set.is_none())
                || (upload.rain_count > 0 && rain_set.is_none())
            {
                return Err(RendererError::Vulkan(format!(
                    "proxy material index outside the scene material table \
                     (npc {}, vehicle {}, rain {})",
                    upload.npc_material, upload.vehicle_material, upload.rain_material
                )));
            }
            Some([
                npc_set.unwrap_or(vk::DescriptorSet::null()),
                vehicle_set.unwrap_or(vk::DescriptorSet::null()),
                rain_set.unwrap_or(vk::DescriptorSet::null()),
            ])
        }
        // A proxy upload without scene resources is structurally
        // unreachable (the safe layer requires an attached scene —
        // attach_scene is the only path, and it sets the context's
        // scene resources) — treated as the
        // unpopulated frame rather than an error.
        (None, Some(_)) | (None, None) | (Some(_), None) => None,
    };
    Ok(proxy_sets)
}

/// Phase 3 — `wait_slot_fence` (§5.4, the slot-fence wait plus the
/// overlay slot growth the design's §1.2 inventory grouped with it
/// at the wait's line range): waits for this slot's previous
/// submission, then grows only the fenced slot's overlay buffer —
/// before acquire, so a failure leaves the image-available semaphore
/// and fence reusable.
fn wait_slot_fence(
    ctx: &VulkanContext,
    frames: &mut FrameState,
    frame: &InternalFrame<'_>,
    slot: usize,
) -> Result<(), RendererError> {
    // Wait for this slot's previous submission. The fence is reset only
    // AFTER the per-image guard below: under present backpressure the
    // driver can hand a slot the image that slot last rendered, and
    // resetting first would leave that guard waiting on a fence only
    // THIS frame's submission will signal — the 240-pacing-against-FIFO
    // wedge (rig finding, 2026-10-20: "wait image fence timed out").
    unsafe {
        ctx.device
            .wait_for_fences(&[frames.in_flight[slot]], true, WAIT_TIMEOUT_NS)
            .map_err(|error| RendererError::vk(error, "wait_for_fences (slot fence timed out — a submission never completed; presenting faster than the present mode drains?)"))?;
    }

    // Allocation can fail. Grow only the fenced slot, before acquire, so
    // a failure leaves the image-available semaphore and fence reusable.
    frames.overlay_slots[slot].reserve(
        ctx,
        frame
            .inputs
            .hud
            .len()
            .saturating_add(frame.inputs.debug_overlay.len()),
    )?;
    Ok(())
}

/// Phase 4 — `acquire_with_retry` (§5.4): the first acquire, the
/// injected-out-of-date simulation, the out-of-date inline recreation
/// (through `prepare_after_recreation`), and the retry acquire.
/// OUT_OF_DATE retries once. The fence stays signaled until an image
/// has actually been acquired, including when both attempts defer.
fn acquire_with_retry(
    ctx: &mut VulkanContext,
    swapchain: &mut SwapchainState,
    pipes: &mut PipelineState,
    frames: &mut FrameState,
    policy: FramePolicy,
    frame: &InternalFrame<'_>,
    slot: usize,
) -> Result<Acquisition, RendererError> {
    // If the acquire-out-of-date plan is armed, inject the error at the
    // FIRST acquire site only (the retry acquire inside the out-of-date
    // arm is NEVER injected — the plan is consumed at the first site
    // only, so the retry remains structurally real). The injected result
    // models the driver contract exactly (ERROR_OUT_OF_DATE_KHR means no
    // image was acquired and the semaphore is untouched — precisely the
    // state the out-of-date arm below already handles; skipping the real
    // call is therefore the faithful simulation, not an approximation).
    let mut acquire_result = if ctx.take_acquire_out_of_date() {
        Err(vk::Result::ERROR_OUT_OF_DATE_KHR)
    } else {
        unsafe {
            ctx.swapchain_ext.acquire_next_image(
                swapchain.swapchain,
                100_000_000,
                frames.image_available[slot],
                vk::Fence::null(),
            )
        }
    };
    let mut recreated = false;
    if let Err(vk::Result::ERROR_OUT_OF_DATE_KHR) = acquire_result {
        // The out-of-date inline recreation uses prepare_after_recreation
        // to handle probe, recreate, state-ensure, target-ensure, capture-ensure,
        // and retry acquire (finding 20, Phase A; findings 3-8).
        match prepare_after_recreation(ctx, swapchain, pipes, frames, policy, frame)? {
            FramePreparationResult::Deferred => {
                return Ok(Acquisition::Deferred);
            }
            FramePreparationResult::Recreated => {
                // This should never happen - prepare_after_recreation already
                // retried acquire. If it returns Recreated, something went wrong.
                frames.needs_recreate = true;
                return Ok(Acquisition::Deferred);
            }
            FramePreparationResult::Proceeded => {
                // The inline recreation happened inside the helper — the
                // original arm reported it on the outcome (the public
                // recreated contract: callers learn the generation changed).
                recreated = true;
                // Retry acquire after preparation.
                acquire_result = unsafe {
                    ctx.swapchain_ext.acquire_next_image(
                        swapchain.swapchain,
                        100_000_000,
                        frames.image_available[slot],
                        vk::Fence::null(),
                    )
                };
            }
        }
    }
    match acquire_result {
        Ok((image_index, suboptimal)) => Ok(Acquisition::Acquired {
            image_index,
            suboptimal,
            recreated,
        }),
        Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
            frames.needs_recreate = true;
            Ok(Acquisition::Deferred)
        }
        Err(vk::Result::TIMEOUT) => Err(RendererError::Vulkan(
            "acquire_next_image timed out: no swapchain image became available in 100 ms \
             (present mode not draining — vsync-locked FIFO paced beyond the display rate?)"
                .into(),
        )),
        Err(error) => Err(RendererError::vk(error, "acquire_next_image")),
    }
}

/// Phase 5 — `guard_image_and_read_timestamps` (§5.4; the per-image
/// guard wait, CA-10's restructure of the old
/// `reset_and_timestamps`): guards against acquiring an image
/// another in-flight submission still reads, then reads the previous
/// use's timestamps. THE WAIT DISCIPLINE IS UNCHANGED from the
/// pre-CA-10 code — this is still "the last wait that can reference
/// this slot's previous signal" — but the RESET that used to follow
/// it here is DEFERRED to [`commit_slot_fence`] between record and
/// submit: every fallible step between this guard and the commit
/// (the uploads, the present semaphore, the recording) now runs with
/// the slot fence still carrying its previous signal, so a failure
/// at any of them leaves the fence waitable and the slot reusable
/// (the stranded-slot fix — see [`dispose_failed_acquisition`]).
fn guard_image_and_read_timestamps(
    ctx: &VulkanContext,
    frames: &mut FrameState,
    slot: usize,
    image_index: u32,
) -> Result<Option<f64>, RendererError> {
    if let Some(image_fence) = mut_at(&mut frames.images_in_flight, image_index as usize) {
        if *image_fence != vk::Fence::null() {
            unsafe {
                ctx.device
                    .wait_for_fences(&[*image_fence], true, WAIT_TIMEOUT_NS)
                    .map_err(|error| {
                        RendererError::vk(
                            error,
                            "wait image fence (timed out — a submission never completed)",
                        )
                    })?;
            }
        }
        // The per-image ownership STAGING (CA-10): the slot fence is
        // recorded as the image's owner BEFORE the fence is reset —
        // publishing here is safe because the slot fence is still
        // SIGNALED (a future guard waiting on it passes immediately)
        // and the PUBLISHING slot is the one this frame is about to
        // reset-and-submit; if anything below fails before the
        // commit, `dispose_failed_acquisition` restores the previous
        // owner or nulls the entry (the frame that would have
        // signaled it never ran).
        *image_fence = frames.in_flight[slot];
    }

    // Timestamps from this slot's previous use are resolved now —
    // Phase 3's slot-fence wait proved the submission complete (the
    // fence is still signaled; the read does not consume it). The
    // first use of a slot has no samples yet, and unsupported
    // timestamps are never queried (corrective finding E).
    let gpu_render_ms =
        if timestamp_sample_ready(frames.timestamps_supported, frames.timestamps_written[slot]) {
            read_timestamps(ctx, frames, slot)
        } else {
            None
        };
    Ok(gpu_render_ms)
}

/// Phase 7.5 — `commit_slot_fence` (CA-10's deferred reset): resets
/// this slot's fence at the LAST fallible boundary before the
/// submit, completing the per-image ownership publication the guard
/// phase staged (the slot fence it recorded there becomes the image's
/// real owner the moment this frame's submission signals it). The
/// reset must precede the submission (a submit signals the fence; it
/// must be unsignaled), and every wait that can reference the slot's
/// PREVIOUS signal (Phase 3's slot wait, the per-image guard) has
/// completed by construction — the pre-CA-10 proof, preserved with
/// the reset simply moved later in the same frame.
fn commit_slot_fence(
    ctx: &VulkanContext,
    frames: &mut FrameState,
    slot: usize,
) -> Result<(), RendererError> {
    // Safe to reset only now: every wait that can reference this
    // slot's previous signal has run (Phase 3's wait; the image
    // guard in Phase 5 — the last one).
    unsafe {
        ctx.device
            .reset_fences(&[frames.in_flight[slot]])
            .map_err(|error| RendererError::vk(error, "reset_fences"))?;
    }
    Ok(())
}

/// The acquisition's disposition (CA-10): EVERY error after a
/// successful acquire gets an explicit release instead of
/// propagating raw. The pre-CA-10 code reset the slot fence before
/// the fallible upload/semaphore/record/submit chain, so any error
/// in that chain left the fence reset-but-never-signaled — the next
/// frame on the slot blocked the full 2 s
/// [`WAIT_TIMEOUT_NS`][super::WAIT_TIMEOUT_NS] and failed with the
/// slot-fence timeout, and the acquired image's
/// `image_available` signal stayed pending for the slot's next
/// acquire (signaling an already-signaled binary semaphore). This
/// release defines the disposition for each piece:
///
/// **The acquired image + the acquire semaphore** — RETURNED
/// together: the WSI-canonical release presents the image WAITING
/// `image_available[slot]`, so the presentation is ordered against
/// the acquire's implicit layout transition (the sync-validation
/// WRITE_AFTER_READ the rig caught when an earlier draft presented
/// with no waits) and the wait consumes the signal at the
/// presentation — one operation returning the image to the pool and
/// retiring the semaphore's signal. The content is whatever the
/// image last held (the failed frame drew nothing into it — a
/// visible one-frame staleness on an already-reported error path).
/// An out-of-date refusal is the normal path's own deferred-
/// recreation arm (latch the intent, probe next frame); because a
/// REJECTED present may not have consumed the wait, the semaphore
/// is then REPLACED (below) rather than trusted. A HARD present
/// failure holds the image (the pool shrinks by one, fail-visible
/// as the typed acquire timeout once failures exhaust it) and the
/// semaphore is replaced the same way.
///
/// **The slot fence** — healed when the fence-state discriminator
/// (`get_fence_status`) finds it unsignaled: the fence is SIGNALED
/// for every failure before the commit (never reset this frame),
/// and NOT_READY after a committed reset whose submission failed or
/// never ran (including a reset that itself failed indeterminately
/// — the status reports the actual state). The heal is ONE empty
/// submission (no command buffers, no waits) carrying the fence —
/// the canonical signal-a-fence idiom — and a status error (device
/// lost) takes the heal arm as the conservative direction.
///
/// **The drain proof** — after the release operations, ONE
/// `queue_wait_idle` proves the presentation's semaphore wait (and
/// the heal submission) COMPLETED before this function returns:
/// the next frame's acquire requires the semaphore with NO pending
/// operations, and a queue-asynchronous consume would race the
/// reuse (the rig's second catch — the pending-operations
/// validation error). The idle is bounded by the presentation
/// engine's drain (a FIFO present processes within a vblank or
/// two) — tens of milliseconds against the strand's 2 seconds.
///
/// **The per-image ownership** — nulled when the discriminator
/// found the fence reset (the publishing frame never submitted, so
/// no work references the image; the image's previous real owner
/// completed — the guard waited on it — so the null costs a future
/// acquirer nothing but a skipped wait). Left UNTOUCHED when the
/// fence is still signaled (the previous owner's entry may still be
/// legitimately waited on; the guard's own wait failure returned
/// before the publication).
///
/// **The recorded command buffer** — reset (best-effort): a
/// mid-recording failure leaves the buffer in the recording state,
/// which the next frame's `begin_command_buffer` would refuse; the
/// pool's `RESET_COMMAND_BUFFER` flag makes the reset legal at any
/// time after the slot's previous submission completed (Phase 3's
/// wait proved it). A failed reset surfaces at the next frame's
/// begin as a typed error — fail-visible, never a silent strand.
///
/// **Escalation** — if the heal submission, the drain, or the
/// rejected-present semaphore replacement fails, the slot's
/// synchronization state is unrecoverable: the ORIGINAL error is
/// wrapped in [`RendererError::FrameSlotUnrecoverable`], the typed
/// terminal refusal — the caller must treat this renderer as
/// failed and construct a fresh one. Never a silent strand.
fn dispose_failed_acquisition(
    ctx: &VulkanContext,
    swapchain: &SwapchainState,
    frames: &mut FrameState,
    slot: usize,
    image_index: u32,
    error: RendererError,
) -> RendererError {
    // Best-effort command-buffer reset (a recording-state buffer
    // would refuse the next frame's begin; a clean buffer resets
    // harmlessly — see the doc above).
    unsafe {
        let _ = ctx.device.reset_command_buffer(
            frames.command_buffers[slot],
            vk::CommandBufferResetFlags::empty(),
        );
    }
    // The fence-state discriminator (see the doc above).
    let fence_signaled =
        unsafe { ctx.device.get_fence_status(frames.in_flight[slot]) }.unwrap_or(false);
    let heal_fence = !fence_signaled;
    if heal_fence {
        // The ownership published at the guard must not survive a
        // frame that never submitted: null it.
        if let Some(image_fence) = mut_at(&mut frames.images_in_flight, image_index as usize) {
            *image_fence = vk::Fence::null();
        }
    }
    // The image return + the acquire semaphore's consume: the
    // WSI-canonical release present (see the doc above).
    let present_wait_semaphores = [frames.image_available[slot]];
    let present_swapchains = [swapchain.swapchain];
    let present_image_indices = [image_index];
    let present_info = vk::PresentInfoKHR::default()
        .wait_semaphores(&present_wait_semaphores)
        .swapchains(&present_swapchains)
        .image_indices(&present_image_indices);
    let semaphore_trusted = match unsafe {
        ctx.swapchain_ext
            .queue_present(ctx.graphics_queue, &present_info)
    } {
        Ok(suboptimal) => {
            if suboptimal {
                mark_suboptimal(&mut frames.needs_recreate);
            }
            // A processed present consumed the wait: the semaphore
            // is trusted idle once the drain below completes.
            true
        }
        Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
            // The normal present path's own deferred arm: the
            // presentation is gone, the recreation intent and the
            // deferral hint latch. A rejected present may not have
            // consumed the wait — the semaphore is replaced below.
            mark_suboptimal(&mut frames.needs_recreate);
            frames.surface_probe_pending = true;
            false
        }
        Err(_present_error) => {
            // The image stays held (pool shrinkage, fail-visible as
            // the typed acquire timeout once failures exhaust it);
            // the semaphore may hold its signal — replaced below.
            false
        }
    };
    // The fence heal (see the doc above): one empty submission
    // carrying the slot fence, only when the discriminator found it
    // unsignaled.
    if heal_fence {
        let fence = frames.in_flight[slot];
        // SAFETY: an empty-batch submission with the fence — the
        // signal-a-fence idiom; no command buffers, no waits.
        if let Err(release_error) =
            unsafe { ctx.device.queue_submit(ctx.graphics_queue, &[], fence) }
                .map_err(|error| RendererError::vk(error, "release submit (fence heal)"))
        {
            return RendererError::FrameSlotUnrecoverable {
                source: Box::new(error),
                release: Box::new(release_error),
            };
        }
    }
    // The drain proof (see the doc above): the presentation's
    // semaphore wait and the heal submission are complete when this
    // returns — the slot's next acquire finds no pending operation.
    if let Err(release_error) = super::device_idle(ctx) {
        return RendererError::FrameSlotUnrecoverable {
            source: Box::new(error),
            release: Box::new(release_error),
        };
    }
    if !semaphore_trusted {
        // The release present was rejected or failed: the acquire
        // semaphore's signal may still be pending, and a pending
        // signal poisons the slot's next acquire (signaling an
        // already-signaled binary semaphore). REPLACE it — the
        // drain above proved no queued operation refers to it, so
        // the destroy is legal whatever its signal state, and the
        // fresh semaphore gives the slot a clean acquire.
        let stale = frames.image_available[slot];
        if stale != vk::Semaphore::null() {
            unsafe {
                ctx.device.destroy_semaphore(stale, None);
            }
        }
        let fresh = match unsafe {
            ctx.device
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
        }
        .map_err(|error| RendererError::vk(error, "release (acquire semaphore replacement)"))
        {
            Ok(fresh) => fresh,
            Err(release_error) => {
                return RendererError::FrameSlotUnrecoverable {
                    source: Box::new(error),
                    release: Box::new(release_error),
                };
            }
        };
        frames.image_available[slot] = fresh;
    }
    error
}

/// Phase 6 — `upload_frame_data` (§5.1's uploads row, §5.4's phase):
/// the mapped per-slot writes this slot's fence wait now owns — the
/// region/foreground layers, the overlay vertices, and the proxy
/// instance/pose/indirect blocks. Returns the world batch's drawn
/// count (the record phase's foreground first-instance offset).
fn upload_frame_data(
    ctx: &VulkanContext,
    pipes: &PipelineState,
    frames: &mut FrameState,
    frame: &InternalFrame<'_>,
    outcome: &mut FrameOutcome,
    slot: usize,
) -> Result<u32, RendererError> {
    // CA-10 fault knob: the UPLOAD boundary (Phase 6, post-acquire).
    // The mapped writes below are infallible today — the Result this
    // function returns is the boundary contract, and the knob keeps
    // the boundary's disposition testable (a plan armed here fires
    // after the acquire, before any write, with the slot fence still
    // signaled — the release must leave the slot reusable).
    ctx.check_lifecycle_fault(crate::LifecycleFaultSite::FrameUpload)?;
    let inputs = &frame.inputs;
    let region_vertices = inputs.region_vertices;
    let foreground = inputs.foreground;
    let foreground_vertices = foreground.map_or(&[][..], |layer| layer.vertices);
    let hud = inputs.hud;
    let debug_overlay = inputs.debug_overlay;
    let proxy = inputs.proxy.as_ref();
    // Both scene layers fit in the existing mapped slot. Copy directly from
    // their retained application buffers without concatenating CPU geometry.
    let mut world_drawn = 0;
    if !region_vertices.is_empty() || !foreground_vertices.is_empty() {
        (outcome.region, world_drawn) = unsafe {
            upload_layers(
                pipes.region_mapped,
                slot,
                pipes.region_capacity,
                region_vertices,
                foreground_vertices,
                LayerPriority::Second,
            )
        };
        outcome.foreground_drawn = outcome.region.drawn - world_drawn;
    }
    let overlay = &frames.overlay_slots[slot];
    outcome.overlay.capacity = overlay.capacity;
    // The overlay upload's gate is the plan's overlay bit (the
    // frame's explicit declaration — the same emptiness check the
    // plan derived once at the top).
    if frame.plan.uses.overlay {
        (outcome.overlay, _) = unsafe {
            upload_layers(
                overlay.mapped,
                0,
                overlay.capacity,
                hud,
                debug_overlay,
                LayerPriority::First,
            )
        };
    }

    // The 6-E instance upload: this slot's region of the mapped
    // instance buffer, written after the slot's fence wait (the
    // region/overlay discipline — the previous submission through
    // this slot, which read this region, has completed). The bytes
    // are the safe layer's repacked population (≤ the region's
    // bound by the repack's validated capacity). The 6-E2 indirect
    // command block follows the identical discipline in the same
    // place: this slot's 32 command bytes (the pure proxy_indirect
    // packing of the pinned mesh vertex counts and this frame's
    // per-kind counts), written only in the indirect mode (the
    // state exists — the frame flow's ensure ran before the fence
    // wait, outside command recording).
    if let (Some(proxy_state), Some(upload)) = (&pipes.proxy, proxy) {
        // P5 §1.3's observability half: the frame's arena facts,
        // recorded at the write site (the capacities are the carve's
        // AFTER this frame's ensure — the growth, if any, already
        // landed in prepare; the region discipline of reading them
        // per frame, never guessing).
        outcome.instance_supplied = upload
            .npc_count
            .saturating_add(upload.vehicle_count)
            .saturating_add(upload.rain_count);
        outcome.instance_capacity = proxy_state.capacity();
        unsafe { proxy_state.write_slot(slot, upload.bytes) };
        if let Some(indirect) = proxy_state.indirect.as_ref()
            && matches!(upload.mode, crate::ProxyDrawMode::Indirect)
        {
            unsafe {
                indirect.write_slot(
                    slot,
                    upload.npc_count,
                    upload.vehicle_count,
                    upload.rain_count,
                )
            };
        }
        // The Era-2 pose bytes: this slot's region of the mapped pose
        // buffer, written under the SAME fence discipline as the
        // instance bytes above (the context's RETAINED skinned state
        // exists whenever a populated frame runs — the ensure is
        // unconditional on proxy presence; the debug_assert co-pins
        // the structural invariant the way the indirect path does).
        if let Some(retained) = ctx.skinned_retained.as_ref() {
            outcome.pose_supplied = (upload.pose_bytes.len() / crate::animation::POSE_STRIDE)
                .min(u32::MAX as usize) as u32;
            outcome.pose_capacity = retained.blocks_capacity();
            unsafe { retained.write_slot(slot, upload.pose_bytes) };
        } else {
            debug_assert!(
                false,
                "the skinned ensure discipline makes a missing pose buffer \
                 unreachable on a populated frame"
            );
        }
    }
    Ok(world_drawn)
}

/// Phase 7 — `record` (§4.2(2), the cascade): the pass recording
/// into this slot's command buffer, in the fixed order the module
/// manifest documents — shadow depth → wet mirrored-scene (+barrier)
/// → the scene pass (the post chain's HDR passes or the main pass
/// with the corner scene recorder / region layers) → the shadow
/// readback → timestamps → the capture readback. The per-family
/// record functions (`record_depth_pass`, `record_reflection_pass`,
/// `post_state.record`, `record_corner_scene`) were already
/// extracted (finding 20 Phase B / S2-S4); this phase is the WIRING
/// the design's §5.4 named — the ladder that sequences them and owns
/// the pass-variant branch (the plan's `post` bit) and the overlay
/// draw. `ctx`, `pipes`, and `swapchain` are immutable here: the
/// cascade records through the states; only the frame state's
/// readback handshake bits mutate.
#[allow(clippy::too_many_arguments)]
fn record_command_buffer(
    ctx: &VulkanContext,
    swapchain: &SwapchainState,
    pipes: &PipelineState,
    frames: &mut FrameState,
    frame: &InternalFrame<'_>,
    proxy_sets: Option<[vk::DescriptorSet; 3]>,
    slot: usize,
    image_index: u32,
    world_drawn: u32,
    outcome: &mut FrameOutcome,
) -> Result<(), RendererError> {
    let frame_inputs = &frame.inputs;
    let plan = &frame.plan;
    // CA-10 fault knob: the RECORD boundary (Phase 7, post-acquire,
    // after the semaphore growth) — a plan armed here fires with
    // nothing recorded and the slot fence still signaled; the
    // release's command-buffer reset covers the mid-recording shape
    // (a failure later in this cascade) identically.
    ctx.check_lifecycle_fault(crate::LifecycleFaultSite::FrameRecord)?;
    let corner_params = frame_inputs.corner_params.as_ref();
    let proxy = frame_inputs.proxy.as_ref();
    let shadowed = frame_inputs.shadowed.as_ref();
    let sky = frame_inputs.sky.as_ref();
    let wet = frame_inputs.wet.as_ref();
    let view_proj = &frame_inputs.view_proj;
    let foreground = frame_inputs.foreground;

    // Record.
    let command = frames.command_buffers[slot];
    let framebuffer = swapchain.framebuffers[image_index as usize];
    let extent = swapchain.extent;
    // The clear color (hoisted: the Era-1 post chain's HDR scene
    // pass clears the SAME color the main pass would — the value
    // is identical, only the attachment's format differs).
    let clear_color = corner_params.map_or_else(
        || shadowed.map_or([0.02, 0.02, 0.04, 1.0], |params| params.clear_color()),
        corner::FrameParams::clear_color,
    );
    let clear_values = [
        vk::ClearValue {
            color: vk::ClearColorValue {
                float32: clear_color,
            },
        },
        vk::ClearValue {
            depth_stencil: vk::ClearDepthStencilValue {
                depth: 1.0,
                stencil: 0,
            },
        },
    ];

    // Disjoint window-depth intervals allow self-occlusion in both layers
    // using one depth attachment, without a clear, barrier or extra pass.
    // The gap at the boundary also accommodates fixed-point depth formats.
    // Even D16 gives a viewmodel 4096 depth levels. Leave one full
    // D16 quantum between intervals, including both clip-plane endpoints.
    const FOREGROUND_MAX_DEPTH: f32 = 1.0 / 16.0;
    let world_min_depth = if outcome.foreground_drawn > 0 {
        FOREGROUND_MAX_DEPTH + 1.0 / 65535.0
    } else {
        0.0
    };
    let viewport = vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: extent.width as f32,
        height: extent.height as f32,
        min_depth: world_min_depth,
        max_depth: 1.0,
    };
    let scissor = vk::Rect2D {
        extent,
        offset: vk::Offset2D { x: 0, y: 0 },
    };

    // The targets were ensured pre-acquire — finding 3's stranded-frame
    // corrective. This comment marks the original call sites (removed).

    unsafe {
        ctx.device
            .begin_command_buffer(
                command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(|error| RendererError::vk(error, "begin_command_buffer"))?;

        if frames.timestamps_supported {
            ctx.device
                .cmd_reset_query_pool(command, frames.timestamp_pool, (slot as u32) * 2, 2);
            ctx.device.cmd_write_timestamp(
                command,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                frames.timestamp_pool,
                (slot as u32) * 2,
            );
        }

        // The 6-F shadow depth pass — recorded BEFORE the main render
        // pass (the pass's final subpass dependency orders the map's
        // writes before the main pass's fragment reads). The extended
        // FrameParams + light matrix are written into this slot's
        // mapped region first (the fence wait above owns the slot).
        if let (Some(corner), Some(shadow_state), Some(params)) =
            (&ctx.corner, &pipes.shadows, shadowed)
        {
            shadow_state.write_frame(slot, params);
            shadow_state.record_depth_pass(
                ctx,
                command,
                corner,
                slot,
                pipes.shadowed_proxy.as_ref(),
                pipes.proxy.as_ref(),
                ctx.skinned_retained.as_ref(),
                proxy,
            );
        }

        // The Era-1 wet reflection's mirrored-scene pass (package 7)
        // — recorded BEFORE the scene pass: the mirrored world
        // rendered into this slot's offscreen target (the world-
        // mirror construction, planar.rs's matrix), then the
        // barrier ordering its writes before the ground re-draw's
        // fragment reads inside the scene pass that follows. The
        // ground re-draw itself lands INSIDE whichever scene pass
        // runs (the post branch's or the main one's), after the
        // mesh draws.
        if let (Some(corner), Some(reflection_state), Some(post_state), Some(camera)) =
            (&ctx.corner, &pipes.reflection, &pipes.post, wet)
        {
            // SAFETY: this slot's fence wait passed above (the
            // mirrored params region is disjoint per slot); the
            // command buffer is recording, before the scene pass;
            // the targets were ensured against the live extent and
            // this same HDR pass object after the acquire.
            reflection_state.write_frame(slot, camera);
            reflection_state.record_reflection_pass(
                ctx,
                corner,
                command,
                post_state.hdr_render_pass(),
                slot,
                extent,
                clear_color,
                &corner.reflected_ranges(),
            );
            reflection_state.record_read_barrier(ctx, command, slot);
        }

        // The Era-1 post chain (package 6): a post frame REPLACES
        // the main render pass with the HDR chain — the scene pass
        // into the offscreen R16G16B16A16 target, the bloom mips,
        // and the composite + tonemap into the acquired swapchain
        // image (post_draw's module header documents the whole
        // sequence and its synchronization). A postless frame takes
        // the main pass below, byte-identical to the pre-package
        // renderer (the post state may exist — the belt check
        // keeps a postless frame off the chain regardless). The
        // branch is the plan's post bit — the scene-pass variant
        // declaration (S3).
        if plan.uses.post
            && let Some(post_state) = &pipes.post
        {
            let inputs = post_draw::PostFrameInputs {
                slot,
                image_index,
                extent,
                clear_color,
                corner_params,
                proxy,
                proxy_sets,
                shadowed,
                sky,
                // The wet receiver's placement, derived once in the
                // plan (the HDR variant here — the post chain's scene
                // pass is the one a wet post frame records through).
                ground: plan.ground,
            };
            // SAFETY: the command buffer is recording past the
            // shadow depth pass; this slot's fence wait passed (the
            // uniform writes inside are disjoint per slot); the
            // targets were ensured against the live swapchain
            // after the acquire; `image_index` is the acquired
            // image.
            outcome.proxy_draw_calls = post_state.record(ctx, pipes, command, &inputs);
        } else {
            ctx.device.cmd_begin_render_pass(
                command,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(pipes.render_pass)
                    .framebuffer(framebuffer)
                    .render_area(scissor)
                    .clear_values(&clear_values),
                vk::SubpassContents::INLINE,
            );
            ctx.device.cmd_set_viewport(command, 0, &[viewport]);
            ctx.device.cmd_set_scissor(command, 0, &[scissor]);
            // The unified corner-scene recorder (finding 20, Phase B):
            // extracts the shared sky/corner/shadowed-ordinary/proxy sequence
            // from the main pass and HDR pass. Returns Some(proxy_calls) when
            // corner-family (shadowed or ordinary) was recorded, None when
            // neither occurred (the else-branch body runs below).
            let targets = scene_record::SceneTargetPipelines {
                sky: pipes.sky.as_ref().map(|s| s.pipeline()),
                lit: Some(pipes.lit_pipeline),
                shadowed: pipes.shadows.as_ref().map(|s| s.shadowed(slot).0),
                proxy: Some(pipes.proxy_pipeline),
                // The rain kind's main-pass pipeline (the unlit
                // blended particle object — the third populated
                // kind).
                proxy_rain: Some(pipes.rain_pipeline),
                proxy_skinned: pipes.skinned.as_ref().map(|s| s.pipeline()),
                shadowed_proxy_skinned: pipes
                    .shadowed_proxy
                    .as_ref()
                    .map(|s| s.skinned_shadowed_pipeline()),
                shadowed_proxy_vehicle: pipes
                    .shadowed_proxy
                    .as_ref()
                    .map(|s| s.plain_shadowed_pipeline()),
            };
            // The wet receiver's placement (finding 14): the ground
            // re-draw moved INSIDE the scene recorder, between the
            // opaque kinds and the translucent rain kind — the
            // ordered blend (the reflection over the opaque ground,
            // the rain over the reflection). The recorder's own
            // comments document the defect this corrects. The
            // placement is the plan's ground field — derived once
            // (the Main variant on a wet postless frame; the HDR
            // variant routes through the post chain's own recorder).
            let ground = plan.ground;
            match scene_record::record_corner_scene(
                ctx,
                pipes,
                command,
                slot,
                targets,
                corner_params,
                shadowed,
                sky,
                proxy,
                proxy_sets,
                ground,
            ) {
                Some(calls) => {
                    outcome.proxy_draw_calls = calls;
                }
                None => {
                    // The Stage-2 scene_pipeline/region/foreground else-branch body
                    // (the non-corner path: static scene + dynamic region/foreground).
                    ctx.device.cmd_bind_pipeline(
                        command,
                        vk::PipelineBindPoint::GRAPHICS,
                        pipes.scene_pipeline,
                    );
                    ctx.device.cmd_push_constants(
                        command,
                        pipes.scene_layout,
                        vk::ShaderStageFlags::VERTEX,
                        0,
                        bytemuck::bytes_of(view_proj),
                    );
                    // The static demonstration scene draws only when the explicit
                    // opt-in uploaded it (`scene_index_count > 0`, P3) — an
                    // omitted renderer binds nothing and draws nothing here; the
                    // scene pipeline below still draws the dynamic region
                    // geometry through the same bound state.
                    if pipes.scene_index_count > 0 {
                        ctx.device
                            .cmd_bind_vertex_buffers(command, 0, &[pipes.scene_vbo], &[0]);
                        ctx.device.cmd_bind_index_buffer(
                            command,
                            pipes.scene_ibo,
                            0,
                            vk::IndexType::UINT32,
                        );
                        ctx.device
                            .cmd_draw_indexed(command, pipes.scene_index_count, 1, 0, 0, 0);
                    }

                    if outcome.region.drawn > 0 {
                        let region_offset = (slot as vk::DeviceSize) * pipes.region_slot_bytes;
                        ctx.device.cmd_bind_vertex_buffers(
                            command,
                            0,
                            &[pipes.region_vbo],
                            &[region_offset],
                        );
                        if world_drawn > 0 {
                            ctx.device.cmd_draw(command, world_drawn, 1, 0, 0);
                        }
                        if let Some(layer) = foreground.filter(|_| outcome.foreground_drawn > 0) {
                            let near_viewport = vk::Viewport {
                                min_depth: 0.0,
                                max_depth: FOREGROUND_MAX_DEPTH,
                                ..viewport
                            };
                            ctx.device.cmd_set_viewport(command, 0, &[near_viewport]);
                            ctx.device.cmd_push_constants(
                                command,
                                pipes.scene_layout,
                                vk::ShaderStageFlags::VERTEX,
                                0,
                                bytemuck::bytes_of(layer.view_proj),
                            );
                            ctx.device.cmd_draw(
                                command,
                                outcome.foreground_drawn,
                                1,
                                world_drawn,
                                0,
                            );
                        }
                    }

                    // The Era-1 wet reflection's ground re-draw for
                    // the NON-CORNER path (the recorder never ran —
                    // the corner family's redraw lives inside it
                    // now, finding 14; this else-branch keeps the
                    // pre-corrective position, after the draws,
                    // byte-identical for the static-scene frames).
                    // The gate is the plan's wet bit (the frame's
                    // explicit declaration). SAFETY: the command
                    // buffer is inside the main render pass, past
                    // the scene draws; this slot's corner
                    // FrameParams region holds the true camera (the
                    // frame flow's write).
                    if plan.uses.wet
                        && let (Some(reflection_state), Some(corner)) =
                            (&pipes.reflection, &ctx.corner)
                    {
                        reflection_state.record_ground_redraw(
                            ctx,
                            corner,
                            command,
                            slot,
                            false,
                            corner.receiver_range(),
                        );
                    }
                }
            }

            if outcome.overlay.drawn > 0 {
                ctx.device.cmd_bind_pipeline(
                    command,
                    vk::PipelineBindPoint::GRAPHICS,
                    pipes.overlay_pipeline,
                );
                ctx.device.cmd_bind_vertex_buffers(
                    command,
                    0,
                    &[frames.overlay_slots[slot].buffer],
                    &[0],
                );
                ctx.device.cmd_bind_descriptor_sets(
                    command,
                    vk::PipelineBindPoint::GRAPHICS,
                    pipes.overlay_layout,
                    0,
                    &[ctx.descriptors.set],
                    &[],
                );
                ctx.device.cmd_draw(command, outcome.overlay.drawn, 1, 0, 0);
            }

            // The Era-1 wet reflection's ground re-draw inside the
            // MAIN pass moved INTO the scene recorder (finding 14:
            // between the opaque kinds and the translucent rain
            // kind — the ordered blend; see the recorder's comments
            // and the GroundRedrawPass::Main argument above). The
            // non-corner else-branch keeps its own call above. The
            // mirrored-scene pass and its barrier preceded this pass
            // (the frame flow's recording order, above the post
            // branch).

            ctx.device.cmd_end_render_pass(command);
        }
        // The 6-F known-answer readback: after the main pass the
        // shadow map sits in SHADER_READ — transition to TRANSFER_SRC
        // and copy the recording slot's map into the mapped buffer
        // (the submission fence proves the copy complete before
        // `take_shadow_map_capture` reads it). The handshake lives in
        // the frame state (replacement-safe). The request is consumed
        // ONLY when the frame actually ran the depth pass — the SAME
        // triple the depth-pass block above requires (corner scene,
        // shadow state, shadowed params) — so a plain frame leaves
        // `pending` untouched and the next shadowed frame records it
        // (the capture survives deferred frames until a frame
        // actually produces the map; the old-layout declaration
        // SHADER_READ below is then always the depth pass's final
        // layout, never a stale one).
        if frames.shadow_readback.pending
            && let (Some(_), Some(shadow_state), Some(_)) = (&ctx.corner, &pipes.shadows, shadowed)
        {
            shadow_state.record_readback(ctx, command, slot);
            frames.shadow_readback.pending = false;
            frames.shadow_readback.ready = true;
            frames.shadow_readback.slot = slot;
            frames.shadow_readback.buffer = shadow_state.readback_buffer();
        }
        if frames.timestamps_supported {
            ctx.device.cmd_write_timestamp(
                command,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                frames.timestamp_pool,
                (slot as u32) * 2 + 1,
            );
        }
        // The bounded frame-capture readback (the orientation
        // corrective's evidence path): after the render pass the image
        // sits in PRESENT_SRC — transition to TRANSFER_SRC, copy the
        // whole image into the mapped buffer, and transition back, all
        // inside this frame's own command buffer so the submission
        // fence proves the copy complete before `take_capture` reads.
        // The record-time guard (the post-orientation hardening + the
        // cache-validation corrective): the acquire path may have
        // recreated the swapchain AFTER the pre-acquire preparation —
        // a destination mismatched in extent, format, OR transfer-
        // source capability is never copied into; the capture is
        // DROPPED (a typed, observable drop) and the frame renders
        // normally.
        if frames.capture.pending {
            if frames.capture.matches(
                extent.width,
                extent.height,
                swapchain.format,
                swapchain.transfer_src_supported,
            ) {
                let image = swapchain.images[image_index as usize];
                let subresource = vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    level_count: 1,
                    layer_count: 1,
                    ..Default::default()
                };
                let to_transfer = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                    .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .image(image)
                    .subresource_range(subresource)
                    .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
                ctx.device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[to_transfer],
                );
                let copy = vk::BufferImageCopy::default()
                    .buffer_offset(0)
                    .buffer_row_length(0)
                    .buffer_image_height(0)
                    .image_subresource(vk::ImageSubresourceLayers {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        mip_level: 0,
                        base_array_layer: 0,
                        layer_count: 1,
                    })
                    .image_extent(vk::Extent3D {
                        width: extent.width,
                        height: extent.height,
                        depth: 1,
                    });
                ctx.device.cmd_copy_image_to_buffer(
                    command,
                    image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    frames.capture.buffer,
                    &[copy],
                );
                // The transfer write must be visible to the host reader.
                // The buffer is HOST_VISIBLE|HOST_COHERENT (create_mapped_buffer),
                // so no explicit invalidate is needed — the barrier establishes
                // the availability/visibility dependency.
                let buffer_barrier = vk::BufferMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::HOST_READ)
                    .buffer(frames.capture.buffer)
                    .offset(0)
                    .size(frames.capture.byte_len as vk::DeviceSize)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
                ctx.device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::HOST,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[buffer_barrier],
                    &[],
                );
                let to_present = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                    .image(image)
                    .subresource_range(subresource)
                    .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                    .dst_access_mask(vk::AccessFlags::empty())
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
                ctx.device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[to_present],
                );
                frames.capture.pending = false;
                frames.capture.ready = true;
                frames.capture.slot = slot;
            } else {
                // The prepared destination does not match the image
                // being recorded (the swapchain changed mid-frame —
                // its extent, format, or transfer-source capability):
                // drop the request — never copy into a mismatched or
                // unsupported destination — and record the typed,
                // observable drop.
                frames.capture.pending = false;
                frames.capture.last_refusal = Some(RendererError::CaptureDropped(format!(
                    "the swapchain changed between the capture preparation and the \
                     recording (extent {}×{}, format {:?}, transfer-src {})",
                    extent.width, extent.height, swapchain.format, swapchain.transfer_src_supported
                )));
            }
        }
        ctx.device
            .end_command_buffer(command)
            .map_err(|error| RendererError::vk(error, "end_command_buffer"))?;
    }
    Ok(())
}

/// Phase 8 — `submit` (§5.4): queue the recorded buffer — waiting
/// the acquire semaphore, signaling the image's present-wait
/// semaphore (corrective finding C) and the slot fence.
fn submit_command_buffer(
    ctx: &VulkanContext,
    frames: &mut FrameState,
    present_semaphore: vk::Semaphore,
    slot: usize,
) -> Result<(), RendererError> {
    // CA-10 fault knob: the SUBMIT boundary (Phase 8) — the one
    // post-acquire failure that lands AFTER the fence commit (the
    // reset ran, the ownership published): a plan armed here fires
    // with the fence reset-but-unsignaled, exercising the release's
    // heal arm (the discriminator finds NOT_READY; the empty
    // release submission re-signals the fence while consuming the
    // acquire semaphore).
    ctx.check_lifecycle_fault(crate::LifecycleFaultSite::FrameSubmit)?;
    let command = frames.command_buffers[slot];
    let wait_stage = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
    let wait_semaphores = [frames.image_available[slot]];
    let signal_semaphores = [present_semaphore];
    let commands = [command];
    let submit_info = vk::SubmitInfo::default()
        .wait_semaphores(&wait_semaphores)
        .wait_dst_stage_mask(&wait_stage)
        .command_buffers(&commands)
        .signal_semaphores(&signal_semaphores);
    unsafe {
        ctx.device
            .queue_submit(ctx.graphics_queue, &[submit_info], frames.in_flight[slot])
            .map_err(|error| RendererError::vk(error, "queue_submit"))?;
    }
    if frames.timestamps_supported {
        frames.timestamps_written[slot] = true;
    }
    Ok(())
}

/// Phase 9 — `present` (§5.4): hand the acquired image to the
/// presentation engine, waiting the image's present semaphore. An
/// out-of-date present is DEFERRED, not inline (the batch-3 review's
/// lifecycle finding: every recreation entry point honors surface
/// availability) — the presentation is gone; the recreation intent
/// and the deferral hint latch, and this frame's submission fence
/// still signals normally.
fn present_image(
    ctx: &VulkanContext,
    swapchain: &SwapchainState,
    frames: &mut FrameState,
    image_index: u32,
    present_semaphore: vk::Semaphore,
    outcome: &mut FrameOutcome,
) -> Result<(), RendererError> {
    let present_wait_semaphores = [present_semaphore];
    let present_swapchains = [swapchain.swapchain];
    let present_image_indices = [image_index];
    let present_info = vk::PresentInfoKHR::default()
        .wait_semaphores(&present_wait_semaphores)
        .swapchains(&present_swapchains)
        .image_indices(&present_image_indices);
    match unsafe {
        ctx.swapchain_ext
            .queue_present(ctx.graphics_queue, &present_info)
    } {
        Ok(present_suboptimal) => {
            outcome.presented = true;
            if present_suboptimal || outcome.suboptimal {
                outcome.suboptimal = true;
                mark_suboptimal(&mut frames.needs_recreate);
            }
        }
        Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
            // DEFERRED, not inline (the batch-3 review's lifecycle
            // finding: every recreation entry point honors surface
            // availability): the presentation is gone; latch the
            // recreation intent and the deferral hint — the next
            // frame's probe-gated consume-and-recreate block rebuilds
            // once the drawable is available again. This frame's
            // submission completed and its fence will signal normally;
            // no synchronization state is touched here.
            mark_suboptimal(&mut frames.needs_recreate);
            frames.surface_probe_pending = true;
        }
        Err(error) => return Err(RendererError::vk(error, "queue_present")),
    }
    Ok(())
}

fn mut_at<T>(slice: &mut [T], index: usize) -> Option<&mut T> {
    slice.get_mut(index)
}

fn read_timestamps(ctx: &VulkanContext, frames: &FrameState, slot: usize) -> Option<f64> {
    let mut stamps = [0u64; 2];
    let result = unsafe {
        ctx.device.get_query_pool_results(
            frames.timestamp_pool,
            (slot as u32) * 2,
            &mut stamps,
            vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
        )
    };
    if result.is_err() {
        return None;
    }
    // Timestamp counters may be narrower than 64 bits (corrective
    // finding E): mask the difference to the valid width so a wrapped
    // counter still yields the correct interval.
    let bits = ctx.timestamp_valid_bits.min(64);
    let mask = if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    let delta = stamps[1].wrapping_sub(stamps[0]) & mask;
    Some(delta as f64 * ctx.timestamp_period_ns / 1_000_000.0)
}

/// Timestamp read policy (corrective finding E): query results are read
/// only when the queue family supports timestamps AND the slot's queries
/// have resolved at least once — never block-read unwritten queries.
pub(crate) fn timestamp_sample_ready(supported: bool, written: bool) -> bool {
    supported && written
}

/// Presentation-wait semaphore table growth (corrective finding C): the
/// table only ever grows to cover the required length — entries are never
/// invalidated, because recreation idles the device first.
pub(crate) fn present_semaphore_len_for(current_len: usize, required: usize) -> usize {
    current_len.max(required)
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    /// THE A04 ZERO-COST STATEMENT (design §3.4, the type-level
    /// half): the collapsed parameter set is plain stack data —
    /// [`FramePolicy`] and [`InternalFrame`] are `Copy` borrow
    /// bundles (no `Vec`, no `Box`, no trait objects), so the 19→6
    /// collapse introduced no per-frame heap allocation and no
    /// clone. The behavioral half of the proof is the batteries'
    /// identical `FrameOutcome`s and allocation counters (S1's
    /// byte-equality gates and the allocation-constancy tuples,
    /// which run unchanged against the collapsed flow).
    #[test]
    fn the_collapsed_frame_parameters_are_copy_stack_data() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<FramePolicy>();
        assert_copy::<InternalFrame<'static>>();
        assert_copy::<FramePlan>();
        assert_copy::<FrameOutcome>();
    }

    #[test]
    fn timestamp_gate_requires_support_and_a_resolved_sample() {
        for supported in [false, true] {
            for written in [false, true] {
                assert_eq!(
                    timestamp_sample_ready(supported, written),
                    supported && written,
                    "supported={supported} written={written}"
                );
            }
        }
    }

    #[test]
    fn present_semaphore_table_only_grows() {
        let mut len = 2;
        for acquired in [0u32, 1, 5, 3] {
            len = present_semaphore_len_for(len, acquired as usize + 1);
            assert!(len > acquired as usize);
        }
        assert!(len >= 6);
        assert_eq!(present_semaphore_len_for(6, 4), 6);
    }

    #[test]
    fn overlay_slots_grow_on_demand_to_the_existing_vertex_budget() {
        let limit = crate::overlay::overlay_capacity_vertices() / FRAMES_IN_FLIGHT;
        assert_eq!(OverlaySlot::capacity_for(0, 0), 0);
        assert_eq!(OverlaySlot::capacity_for(0, 37), 64);
        assert_eq!(OverlaySlot::capacity_for(64, 0), 64);
        assert_eq!(OverlaySlot::capacity_for(64, 64), 64);
        assert_eq!(OverlaySlot::capacity_for(64, usize::MAX), limit);
        assert_eq!(OverlaySlot::capacity_for(limit, usize::MAX), limit);
    }
}
