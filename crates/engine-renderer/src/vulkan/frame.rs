//! Frame synchronization and commands (P4 S4 — design §5.1's frame
//! row): `FrameState` — per-slot command buffers, fences, per-image
//! present-wait semaphores, timestamps, overlay slots — the frame
//! outcome, and the full wait -> acquire -> (recreate) -> record ->
//! submit -> present flow (`render_frame`, the coordinator's host
//! half). Pure move from vulkan.rs (stage S4); no logic changed —
//! only location and visibility.

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
use super::{corner, post_draw, proxy_draw, reflection_draw, scene_record, shadow_draw, sky_draw};
use crate::SceneLayer;
use crate::config::{DemoScenePreference, DynamicClamp, FRAMES_IN_FLIGHT, PresentModePreference};
use crate::overlay::OverlayVertex;
use crate::scene::SceneVertex;

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
}

/// The full frame flow: wait → acquire → (recreate if needed) → record →
/// submit → present. Called by the safe [`crate::Renderer`] wrapper.
// Private frame orchestration: the four owning states plus the per-frame
// inputs are exactly the parameters the record/submit/present sequence
// needs; bundling them would hide the ownership story for no gain.
// The Era-1 sky pass rides the same flat list (the one-Option-per-layer
// shape; the sky is a layer like the shadowed one, drawn first when
// present). The Era-1 post chain rides it as ONE bool (the chain has no
// per-frame data at all — its only uniform is extent-static), and the
// wet reflection as ONE Option (the camera description its mirrored
// math consumes; None keeps the dry path).
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_frame(
    ctx: &mut VulkanContext,
    swapchain: &mut SwapchainState,
    pipes: &mut PipelineState,
    frames: &mut FrameState,
    window_size: (u32, u32),
    present_preference: PresentModePreference,
    replace_pipelines: bool,
    demo_scene: DemoScenePreference,
    view_proj: &[[f32; 4]; 4],
    region_vertices: &[SceneVertex],
    foreground: Option<SceneLayer<'_>>,
    hud: &[OverlayVertex],
    debug_overlay: &[OverlayVertex],
    corner_params: Option<&corner::FrameParams>,
    proxy: Option<&proxy_draw::ProxyFrameUpload<'_>>,
    shadowed: Option<&shadow_draw::ShadowFrameParams>,
    sky: Option<&sky_draw::SkyFrameParams>,
    post: bool,
    wet: Option<&reflection_draw::WetCamera>,
) -> Result<FrameOutcome, RendererError> {
    let mut outcome = FrameOutcome::default();
    let slot = frames.frame_slot;
    // The 6-E2 issuance observation's frame baseline: the recording
    // below (this frame's command buffer) increments the context's
    // monotonic vkCmdDrawIndirect counter at each call site; the
    // delta lands on the outcome after the submission. Deferred and
    // refused frames record nothing, so their delta is zero.
    let indirect_issued_before = ctx.indirect_draws_issued.get();

    // The proxy materials are validated before ANY synchronization
    // state is touched (the capture-refusal discipline: a refusal
    // here can never leave a fence reset-but-unsignaled or an
    // acquired image's semaphore unconsumed). A populated kind whose
    // material index falls outside the corner table is a typed error;
    // an EMPTY kind may carry any index (it draws nothing). The
    // Era-2 package C rain kind joins the validation exactly like
    // the other kinds.
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
        // the Corner preference auto-attaches at construction, and
        // attach_scene is the public path) — treated as the
        // unpopulated frame rather than an error.
        (None, Some(_)) | (None, None) | (Some(_), None) => None,
    };

    // Frame preparation: swapchain checks, lazy state, and target creation
    // before acquire (finding 20, Phase A; findings 3-8).
    let foreground_vertices = foreground.map_or(&[][..], |layer| layer.vertices);
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
    let plan = FramePlan::derive(
        region_vertices,
        foreground_vertices,
        hud,
        debug_overlay,
        proxy,
        shadowed,
        sky,
        post,
        wet,
    );
    match prepare_frame(
        ctx,
        swapchain,
        pipes,
        frames,
        window_size,
        present_preference,
        replace_pipelines,
        demo_scene,
        &plan,
        frames.capture.pending,
    )? {
        FramePreparationResult::Deferred => {
            outcome.deferred = true;
            return Ok(outcome);
        }
        FramePreparationResult::Recreated => {
            outcome.recreated = true;
            // Re-run preparation against the new generation.
            match prepare_frame(
                ctx,
                swapchain,
                pipes,
                frames,
                window_size,
                present_preference,
                replace_pipelines,
                demo_scene,
                &plan,
                frames.capture.pending,
            )? {
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
    frames.overlay_slots[slot].reserve(ctx, hud.len().saturating_add(debug_overlay.len()))?;

    // OUT_OF_DATE retries once. The fence stays signaled until an image
    // has actually been acquired, including when both attempts defer.
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
    if let Err(vk::Result::ERROR_OUT_OF_DATE_KHR) = acquire_result {
        // The out-of-date inline recreation uses prepare_after_recreation
        // to handle probe, recreate, state-ensure, target-ensure, capture-ensure,
        // and retry acquire (finding 20, Phase A; findings 3-8).
        match prepare_after_recreation(
            ctx,
            swapchain,
            pipes,
            frames,
            window_size,
            present_preference,
            replace_pipelines,
            demo_scene,
            &plan,
            frames.capture.pending,
        )? {
            FramePreparationResult::Deferred => {
                outcome.deferred = true;
                return Ok(outcome);
            }
            FramePreparationResult::Recreated => {
                // This should never happen - prepare_after_recreation already
                // retried acquire. If it returns Recreated, something went wrong.
                frames.needs_recreate = true;
                outcome.deferred = true;
                return Ok(outcome);
            }
            FramePreparationResult::Proceeded => {
                // The inline recreation happened inside the helper — the
                // original arm reported it on the outcome (the public
                // recreated contract: callers learn the generation changed).
                outcome.recreated = true;
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
    let (image_index, suboptimal) = match acquire_result {
        Ok((index, suboptimal)) => (index, suboptimal),
        Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
            frames.needs_recreate = true;
            outcome.deferred = true;
            return Ok(outcome);
        }
        Err(vk::Result::TIMEOUT) => {
            return Err(RendererError::Vulkan(
                "acquire_next_image timed out: no swapchain image became available in 100 ms \
                 (present mode not draining — vsync-locked FIFO paced beyond the display rate?)"
                    .into(),
            ));
        }
        Err(error) => return Err(RendererError::vk(error, "acquire_next_image")),
    };
    outcome.suboptimal = suboptimal;
    frames.image_index = image_index;
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
        *image_fence = frames.in_flight[slot];
    }

    // Safe to reset only now: the image guard above is the last wait
    // that can reference this slot's previous signal.
    unsafe {
        ctx.device
            .reset_fences(&[frames.in_flight[slot]])
            .map_err(|error| RendererError::vk(error, "reset_fences"))?;
    }

    // Timestamps from this slot's previous use are resolved now (the
    // fence above proves it). The first use of a slot has no samples
    // yet, and unsupported timestamps are never queried (corrective
    // finding E).
    let gpu_render_ms =
        if timestamp_sample_ready(frames.timestamps_supported, frames.timestamps_written[slot]) {
            read_timestamps(ctx, frames, slot)
        } else {
            None
        };

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
    if plan.uses.overlay {
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
            unsafe { retained.write_slot(slot, upload.pose_bytes) };
        } else {
            debug_assert!(
                false,
                "the skinned ensure discipline makes a missing pose buffer \
                 unreachable on a populated frame"
            );
        }
    }

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

    // The present-wait semaphore is owned by the acquired image
    // (corrective finding C).
    let present_semaphore = frames.present_semaphore_for(ctx, image_index)?;

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

        // Submit: wait for the acquire semaphore, signal the image's
        // present-wait semaphore (corrective finding C) and the slot
        // fence.
        let wait_stage = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let wait_semaphores = [frames.image_available[slot]];
        let signal_semaphores = [present_semaphore];
        let commands = [command];
        let submit_info = vk::SubmitInfo::default()
            .wait_semaphores(&wait_semaphores)
            .wait_dst_stage_mask(&wait_stage)
            .command_buffers(&commands)
            .signal_semaphores(&signal_semaphores);
        ctx.device
            .queue_submit(ctx.graphics_queue, &[submit_info], frames.in_flight[slot])
            .map_err(|error| RendererError::vk(error, "queue_submit"))?;
        if frames.timestamps_supported {
            frames.timestamps_written[slot] = true;
        }
    }

    // Present.
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

    outcome.gpu_render_ms = gpu_render_ms;
    outcome.indirect_draws_issued =
        (ctx.indirect_draws_issued.get() - indirect_issued_before) as u32;
    frames.frame_slot = (slot + 1) % FRAMES_IN_FLIGHT as usize;
    Ok(outcome)
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
