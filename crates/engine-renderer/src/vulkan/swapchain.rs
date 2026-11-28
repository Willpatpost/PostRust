//! Swapchain and presentation (P4 S4 — design §5.1's swapchain row):
//! the swapchain with its per-image depth attachments and framebuffers,
//! extent clamping and surface-availability probing, the monotonic
//! generation counter, recreation (format-gated pipeline replacement
//! included), and the suboptimal/out-of-date recreation handshake
//! helpers. Pure move from vulkan.rs (stage S4); no logic changed —
//! only location and visibility.

use ash::vk;

use super::RendererError;
use super::context::{VulkanContext, pick_surface_format};
use super::frame::{FramePolicy, FrameState};
use super::pipelines::PipelineState;
use super::resources::find_memory_type;
use crate::config::{FRAMES_IN_FLIGHT, PresentModePreference, resolve_present_mode};

/// Swapchain + attachments + framebuffers. Recreated on resize,
/// out-of-date, and suboptimal-present.
pub(crate) struct SwapchainState {
    pub swapchain: vk::SwapchainKHR,
    /// Last drawable size requested by the application. A surface can pin
    /// or clamp its actual extent to a different size.
    pub(super) requested_size: (u32, u32),
    /// The raw swapchain images (framebuffer-owner side of the views —
    /// kept for the bounded frame-capture readback, which copies the
    /// presented image; the swapchain itself owns their lifetime).
    pub images: Vec<vk::Image>,
    pub views: Vec<vk::ImageView>,
    pub framebuffers: Vec<vk::Framebuffer>,
    /// One depth attachment per swapchain image (corrective finding A):
    /// two in-flight submissions render into different images, so they
    /// never share depth state.
    pub depth_images: Vec<vk::Image>,
    pub depth_memories: Vec<vk::DeviceMemory>,
    pub depth_views: Vec<vk::ImageView>,
    pub extent: vk::Extent2D,
    pub format: vk::Format,
    pub present_mode: vk::PresentModeKHR,
    pub image_count: u32,
    pub depth_format: vk::Format,
    /// Whether the surface's swapchain images support TRANSFER_SRC
    /// usage (the frame-capture readback path; requested at creation
    /// whenever supported).
    pub transfer_src_supported: bool,
    /// Monotonic generation for cache invalidation (corrective finding 7).
    /// Vulkan permits handle reuse, so a numerical view handle could
    /// recur after destruction. The generation increases forever so
    /// retired generations' keys can never recur.
    pub generation: u64,
}

impl SwapchainState {
    /// `depth_format` is chosen once by the owning renderer and shared
    /// with the pipeline state, so the render pass and the depth
    /// attachments cannot drift apart (corrective finding F).
    /// `old_swapchain` is set during recreation: creating the replacement
    /// while the current swapchain still owns the native window is the
    /// documented `VkSwapchainCreateInfoKHR::oldSwapchain` flow — without
    /// it, native-window platforms (Win32) fail with
    /// ERROR_NATIVE_WINDOW_IN_USE_KHR, found on the lead's rig when
    /// maximizing (corrective follow-up, 2026-10-20). Null for a fresh
    /// renderer.
    // Swapchain creation inputs plus the recreation linkage; bundling
    // would hide which object pairs with which lifetime.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        ctx: &VulkanContext,
        window_size: (u32, u32),
        preference: PresentModePreference,
        format: vk::Format,
        color_space: vk::ColorSpaceKHR,
        render_pass: vk::RenderPass,
        depth_format: vk::Format,
        old_swapchain: vk::SwapchainKHR,
    ) -> Result<SwapchainState, RendererError> {
        // Partial-initialization cleanup (corrective finding G): build
        // into a state whose handles start dead; on failure `destroy`
        // releases exactly what was created (null/empty entries are
        // no-ops per the Vulkan spec).
        let mut state = SwapchainState {
            swapchain: vk::SwapchainKHR::null(),
            requested_size: window_size,
            images: Vec::new(),
            views: Vec::new(),
            framebuffers: Vec::new(),
            depth_images: Vec::new(),
            depth_memories: Vec::new(),
            depth_views: Vec::new(),
            extent: vk::Extent2D::default(),
            format,
            present_mode: vk::PresentModeKHR::FIFO,
            image_count: 0,
            depth_format,
            transfer_src_supported: false,
            generation: next_swapchain_generation(),
        };
        if let Err(error) = state.build(
            ctx,
            window_size,
            preference,
            format,
            color_space,
            render_pass,
            old_swapchain,
        ) {
            state.destroy(ctx);
            return Err(error);
        }
        Ok(state)
    }

    // Same rationale as `new`: the creation inputs plus the recreation
    // linkage, spread for lifetime clarity.
    #[allow(clippy::too_many_arguments)]
    fn build(
        &mut self,
        ctx: &VulkanContext,
        window_size: (u32, u32),
        preference: PresentModePreference,
        format: vk::Format,
        color_space: vk::ColorSpaceKHR,
        render_pass: vk::RenderPass,
        old_swapchain: vk::SwapchainKHR,
    ) -> Result<(), RendererError> {
        let surface = ctx.surface;
        let capabilities = unsafe {
            ctx.surface_ext
                .get_physical_device_surface_capabilities(ctx.physical_device, surface)
        }
        .map_err(|error| RendererError::vk(error, "surface capabilities"))?;

        let extent = clamp_extent(window_size, &capabilities);
        if extent.width == 0 || extent.height == 0 {
            return Err(RendererError::Vulkan(
                "zero window extent (minimized?)".into(),
            ));
        }
        self.extent = extent;
        self.present_mode = resolve_present_mode(preference, &ctx.supported_present_modes);

        let desired_images = (capabilities.min_image_count + 1).max(FRAMES_IN_FLIGHT);
        let max_images = if capabilities.max_image_count == 0 {
            desired_images
        } else {
            capabilities.max_image_count
        };
        let image_count = desired_images.min(max_images);

        // Frame-capture readback (the orientation corrective): the
        // bounded capture path copies the presented image to the host,
        // which requires TRANSFER_SRC usage on the swapchain images —
        // requested whenever the surface supports it (the spec only
        // guarantees COLOR_ATTACHMENT; universal in practice).
        self.transfer_src_supported = capabilities
            .supported_usage_flags
            .contains(vk::ImageUsageFlags::TRANSFER_SRC);
        let mut image_usage = vk::ImageUsageFlags::COLOR_ATTACHMENT;
        if self.transfer_src_supported {
            image_usage |= vk::ImageUsageFlags::TRANSFER_SRC;
        }

        let create_info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(image_count)
            .image_format(format)
            .image_color_space(color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(image_usage)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(capabilities.current_transform)
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(self.present_mode)
            .old_swapchain(old_swapchain)
            .clipped(true);
        self.swapchain = unsafe { ctx.swapchain_ext.create_swapchain(&create_info, None) }
            .map_err(|error| RendererError::vk(error, "create_swapchain"))?;
        let images = unsafe { ctx.swapchain_ext.get_swapchain_images(self.swapchain) }
            .map_err(|error| RendererError::vk(error, "get_swapchain_images"))?;
        self.image_count = images.len() as u32;
        // Kept for the capture readback (CopyImageToBuffer needs the
        // raw images; the views are the rendering side).
        self.images = images;

        let subresource = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            level_count: 1,
            layer_count: 1,
            ..Default::default()
        };
        for image in &self.images {
            let view_info = vk::ImageViewCreateInfo::default()
                .image(*image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
                .subresource_range(subresource);
            let view = unsafe { ctx.device.create_image_view(&view_info, None) }
                .map_err(|error| RendererError::vk(error, "create_image_view"))?;
            self.views.push(view);
        }

        // One depth attachment per swapchain image (corrective finding
        // A): in-flight submissions into different images never share
        // depth state.
        for _ in &self.images {
            let depth_info = vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .extent(vk::Extent3D {
                    width: extent.width,
                    height: extent.height,
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(1)
                .format(self.depth_format)
                .tiling(vk::ImageTiling::OPTIMAL)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
                .samples(vk::SampleCountFlags::TYPE_1)
                .sharing_mode(vk::SharingMode::EXCLUSIVE);
            let depth_image = unsafe { ctx.device.create_image(&depth_info, None) }
                .map_err(|error| RendererError::vk(error, "create depth image"))?;
            self.depth_images.push(depth_image);
            let memory_requirements =
                unsafe { ctx.device.get_image_memory_requirements(depth_image) };
            let depth_memory_index = find_memory_type(
                ctx,
                memory_requirements.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .ok_or_else(|| RendererError::NoDevice("no DEVICE_LOCAL memory type".into()))?;
            let allocate_info = vk::MemoryAllocateInfo::default()
                .allocation_size(memory_requirements.size)
                .memory_type_index(depth_memory_index);
            let depth_memory = unsafe { ctx.allocate_memory(&allocate_info) }
                .map_err(|error| RendererError::vk(error, "allocate depth memory"))?;
            self.depth_memories.push(depth_memory);
            unsafe { ctx.device.bind_image_memory(depth_image, depth_memory, 0) }
                .map_err(|error| RendererError::vk(error, "bind depth memory"))?;
            let depth_view_info = vk::ImageViewCreateInfo::default()
                .image(depth_image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(self.depth_format)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::DEPTH,
                    level_count: 1,
                    layer_count: 1,
                    ..Default::default()
                });
            let depth_view = unsafe { ctx.device.create_image_view(&depth_view_info, None) }
                .map_err(|error| RendererError::vk(error, "create depth view"))?;
            self.depth_views.push(depth_view);
        }

        for index in 0..self.views.len() {
            let attachments = [self.views[index], self.depth_views[index]];
            let framebuffer_info = vk::FramebufferCreateInfo::default()
                .render_pass(render_pass)
                .attachments(&attachments)
                .width(extent.width)
                .height(extent.height)
                .layers(1);
            let framebuffer = unsafe { ctx.device.create_framebuffer(&framebuffer_info, None) }
                .map_err(|error| RendererError::vk(error, "create_framebuffer"))?;
            self.framebuffers.push(framebuffer);
        }

        Ok(())
    }

    pub(crate) fn destroy(&mut self, ctx: &VulkanContext) {
        unsafe {
            for framebuffer in &self.framebuffers {
                ctx.device.destroy_framebuffer(*framebuffer, None);
            }
            self.framebuffers.clear();
            for view in &self.depth_views {
                ctx.device.destroy_image_view(*view, None);
            }
            self.depth_views.clear();
            for image in &self.depth_images {
                ctx.device.destroy_image(*image, None);
            }
            self.depth_images.clear();
            for memory in &self.depth_memories {
                ctx.device.free_memory(*memory, None);
            }
            self.depth_memories.clear();
            for view in &self.views {
                ctx.device.destroy_image_view(*view, None);
            }
            self.views.clear();
            // The raw swapchain images are owned by the swapchain
            // itself (destroyed below) — only the handles are dropped.
            self.images.clear();
            if self.swapchain != vk::SwapchainKHR::null() {
                ctx.swapchain_ext.destroy_swapchain(self.swapchain, None);
                self.swapchain = vk::SwapchainKHR::null();
            }
        }
    }
}

fn clamp_extent(
    window_size: (u32, u32),
    capabilities: &vk::SurfaceCapabilitiesKHR,
) -> vk::Extent2D {
    let current = capabilities.current_extent;
    // Zero-sized inputs are deliberately NOT forced to 1: a zero extent
    // means the drawable is temporarily unavailable, which render_frame
    // defers before any swapchain is created — the 1×1 fabrication was
    // the Win32 minimize/transition defect (VUID
    // VkSwapchainCreateInfoKHR-pNext-07781, found on the rig 2026-10-20).
    // max == 0 reads as unbounded, per the swapchain extension's
    // convention for extent limits.
    let clamp =
        |value: u32, min: u32, max: u32| value.clamp(min, if max == 0 { u32::MAX } else { max });
    let (width, height) = if current.width == u32::MAX && current.height == u32::MAX {
        window_size
    } else {
        (current.width, current.height)
    };
    vk::Extent2D {
        width: clamp(
            width,
            capabilities.min_image_extent.width,
            capabilities.max_image_extent.width,
        ),
        height: clamp(
            height,
            capabilities.min_image_extent.height,
            capabilities.max_image_extent.height,
        ),
    }
}

/// True when the surface's drawable is temporarily unavailable — a
/// zero-sized current extent (Win32 reports (0,0) for minimized and
/// mid-transition surfaces; with min/max also 0 there is provably no
/// valid extent). Pure policy over the capabilities.
pub(crate) fn surface_extent_unavailable(capabilities: &vk::SurfaceCapabilitiesKHR) -> bool {
    capabilities.current_extent.width == 0 || capabilities.current_extent.height == 0
}

/// Live availability probe used once per frame before any recreation
/// trigger: a deferred frame creates nothing, destroys nothing, and
/// errors on nothing.
pub(super) fn drawable_extent(
    ctx: &VulkanContext,
    window_size: (u32, u32),
) -> Result<Option<vk::Extent2D>, RendererError> {
    let capabilities = unsafe {
        ctx.surface_ext
            .get_physical_device_surface_capabilities(ctx.physical_device, ctx.surface)
    }
    .map_err(|error| RendererError::vk(error, "surface capabilities"))?;
    Ok((!surface_extent_unavailable(&capabilities))
        .then(|| clamp_extent(window_size, &capabilities)))
}

/// The hint-gated probe decision (pure over the frame-state flags —
/// extracted for the state-transition regression, the batch-3 review's
/// corrective item 3). THE LIFECYCLE CONTRACT (finding 1): a
/// `ProbeDeferred` outcome defers the frame WITHOUT touching
/// `needs_recreate` — a pending recreation's intent survives the
/// deferral and is consumed only by the recreate block on a frame
/// where the probe reports the drawable available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SurfaceProbeDecision {
    /// No evidence of trouble — no driver round-trip this frame.
    Skip,
    /// A probe was warranted and the drawable is available: clear the
    /// deferral hint; a pending recreation proceeds to its own
    /// consume-and-recreate block.
    ProbeAvailable,
    /// A probe was warranted and the drawable is unavailable: defer
    /// the frame, latch the deferral hint, and PRESERVE any pending
    /// recreation intent.
    ProbeDeferred,
}

/// The pure decision table: `surface_available` is the probed answer,
/// consulted only when the hints warrant a probe.
pub(crate) fn surface_probe_decision(
    surface_probe_pending: bool,
    needs_recreate: bool,
    surface_available: bool,
) -> SurfaceProbeDecision {
    if !surface_probe_pending && !needs_recreate {
        SurfaceProbeDecision::Skip
    } else if surface_available {
        SurfaceProbeDecision::ProbeAvailable
    } else {
        SurfaceProbeDecision::ProbeDeferred
    }
}

/// Monotonic swapchain generation counter for cache invalidation.
/// Vulkan permits handle reuse, so a numerical view handle could recur
/// after destruction. The generation is host-side bookkeeping only,
/// never packed into any deterministic byte path; it increases forever
/// so retired generations' keys can never recur. (corrective finding 7)
pub(crate) static SWAPCHAIN_GENERATION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);

pub(crate) fn next_swapchain_generation() -> u64 {
    SWAPCHAIN_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Swapchain recreation (corrective findings F + G): idle the device
/// (draining outstanding presents), then build replacement resources.
/// Passing oldSwapchain retires it even when vkCreateSwapchainKHR fails.
/// A failed replacement releases the retired handle and retries with a
/// fresh swapchain before the next image acquisition.
/// When the surface format changed, the replacement pipelines are built
/// first — framebuffers are always created against the render pass that
/// will be current after the swap, never a stale one.
/// A04 (P4 D2 §3.4): the three trailing configuration parameters
/// collapsed into the `Copy` [`FramePolicy`] — the same value
/// `render_frame` and the preparation pair carry (6 parameters).
pub(super) fn recreate_swapchain(
    ctx: &mut VulkanContext,
    swapchain: &mut SwapchainState,
    pipes: &mut PipelineState,
    frames: &mut FrameState,
    window_size: (u32, u32),
    policy: FramePolicy,
) -> Result<(), RendererError> {
    // Keep the request pending across failures, including an error before
    // the replacement reaches vkCreateSwapchainKHR.
    frames.needs_recreate = true;
    // P5 §3.3 (A18/R2) — RESOURCE-REPLACEMENT INVALIDATION, the
    // recreation arm: the replacement pipeline state re-creates the
    // instance carve and the 6-E2 indirect command buffer with NO
    // content copy (the finding-A replication discipline — every
    // frame that draws from a slot's region first wrote it), so no
    // population stamp can vouch for any slot's content on the new
    // generation. Clearing forces one rewrite per slot after the
    // recreation — the stamp protocol's correctness rule, never a
    // stale skip against uninitialized mapped memory.
    frames.population_stamps.clear();
    unsafe { ctx.device.device_wait_idle() }
        .map_err(|error| RendererError::vk(error, "device_wait_idle"))?;
    let depth_format = swapchain.depth_format;
    let (new_format, new_color_space) = pick_surface_format(ctx)?;
    // Build the replacement pipelines first when the format changed
    // (finding F) — or when the verification flag forces the
    // replacement path (`--replace-pipelines`, exercising the
    // format-change lifecycle without a real format change; the CI
    // resize smoke cannot otherwise reach it because resizing keeps
    // the surface format). On failure the old state is untouched and
    // the partial replacement cleans itself up (finding G). The
    // replacement is built with the LIVE demo-scene preference (P3):
    // a recreated pipeline renders exactly what its predecessor did —
    // an omitted renderer must not gain demo content by recreating,
    // and an opted-in demonstration must not lose it.
    let replacement_pipes = if policy.replace_pipelines || new_format != swapchain.format {
        let mut replacement = PipelineState::new(ctx, new_format, depth_format, policy.demo_scene)?;
        // Stage 4 final-review finding A: a replacement pipeline must
        // be fully prepared for dynamic region drawing BEFORE it
        // becomes active. If the outgoing state owns a lazily
        // allocated region-scene buffer, the replacement allocates its
        // own before installation — a failure destroys the replacement
        // and leaves the old state valid (the finding-G discipline),
        // so no half-installed pipeline state and, afterward, no null
        // mapped-buffer writes or null region-VBO draws. The default
        // `--render` path never has a region buffer, so it never pays
        // this allocation.
        if pipes.region_vbo != vk::Buffer::null()
            && let Err(error) = replacement.ensure_region_capacity(
                ctx,
                policy.region_capacity,
                // Replicate the OUTGOING carve exactly (the demand
                // quantum is idempotent at required == current): the
                // replacement's region buffer is the same size the
                // outgoing state carved — the finding-A replication,
                // with P5 §1.3's demand-sized carve carried across
                // the replacement (the tuple pins hold: same count,
                // same size).
                pipes.region_capacity,
            )
        {
            replacement.destroy(ctx);
            return Err(error);
        }
        // The 6-E companion: a populated renderer's proxy resources
        // must likewise exist on the replacement BEFORE installation
        // (the same finding-A discipline — the replacement pipeline
        // reads the same lazily-created mesh VBOs and instance
        // buffer, so a half-installed state must never draw through
        // null handles). The 6-E2 indirect command state re-ensures
        // the same way when the outgoing state owned one: the
        // replacement's indirect draws read the same lazily-created
        // command buffer.
        if pipes.proxy.is_some() {
            if let Err(error) = replacement.ensure_proxy_resources(
                ctx,
                policy.instance_capacity,
                // Replicate the OUTGOING instance carve exactly (the
                // P5 §1.3 demand-sized carve carried across the
                // replacement — same count, same size; the tuple
                // pins hold).
                pipes.proxy.as_ref().map_or(0, |state| state.capacity()),
            ) {
                replacement.destroy(ctx);
                return Err(error);
            }
            if pipes
                .proxy
                .as_ref()
                .is_some_and(|state| state.indirect.is_some())
                && let Some(state) = replacement.proxy.as_mut()
                && let Err(error) = state.ensure_indirect(ctx)
            {
                replacement.destroy(ctx);
                return Err(error);
            }
        }
        // The 6-F companion: a shadowed renderer's shadow state must
        // likewise exist on the replacement BEFORE installation (the
        // same finding-A discipline — the replacement's shadowed
        // pipeline renders inside the replacement's own render pass,
        // against the replacement's own maps).
        if pipes.shadows.is_some()
            && let Err(error) = replacement.ensure_shadow_resources(ctx)
        {
            replacement.destroy(ctx);
            return Err(error);
        }
        // The Era-2 companion: a populated renderer's SKINNED
        // pipelines must likewise exist on the replacement BEFORE
        // installation (the same finding-A discipline — the
        // replacement's skinned pipeline renders inside the
        // replacement's own render pass; a half-installed state must
        // never draw through null handles). The ownership transfer
        // halves the cost: the PASS-INDEPENDENT resources (pose
        // storage, pool, sets, pipeline layout) live on the context
        // and are INHERITED — this ensure is a no-op for them (their
        // counters stay flat across the replacement, the lifecycle
        // gates' pin) — so only the replacement's main-pass pipeline
        // rebuilds here.
        if pipes.skinned.is_some()
            && let Err(error) = replacement.ensure_skinned_resources(ctx, policy.pose_capacity, 0)
        {
            replacement.destroy(ctx);
            return Err(error);
        }
        // The shadowed-populated companion: the replacement's
        // shadowed-proxy state must exist before installation (the
        // same finding-A discipline — the replacement's shadowed
        // pipelines render inside the replacement's own render pass,
        // and its draws read the same lazily-created state). The
        // frame flow's ensure discipline guarantees shadows AND
        // skinned exist on the replacement (they're ensured above),
        // so this can safely access them.
        if pipes.shadowed_proxy.is_some()
            && let Err(error) = replacement.ensure_shadowed_proxy_resources(ctx)
        {
            replacement.destroy(ctx);
            return Err(error);
        }
        // The Era-1 post companion: a post renderer's post state
        // must likewise exist on the replacement BEFORE
        // installation (the same finding-A discipline — the
        // replacement's post pass targets the NEW swapchain
        // format, so the state is rebuilt against it here; the
        // extent-bound targets build at the replacement's first
        // frame, against the live views). The shadowed/sky HDR
        // VARIANTS are NOT built here: they bind the shadow/sky
        // states' OWN pipeline layouts, and the replacement's
        // shadow/sky states may not exist yet — the frame flow's
        // family-growth block creates them against the LIVE
        // states' layouts at the first frame that needs them.
        if pipes.post.is_some()
            && let Err(error) = replacement.ensure_post_resources(ctx, new_format, depth_format)
        {
            replacement.destroy(ctx);
            return Err(error);
        }
        Some(replacement)
    } else {
        None
    };
    let render_pass_for_new = replacement_pipes
        .as_ref()
        .map_or(pipes.render_pass, |replacement| replacement.render_pass);
    match SwapchainState::new(
        ctx,
        window_size,
        policy.present_preference,
        new_format,
        new_color_space,
        render_pass_for_new,
        depth_format,
        // After a failed replacement, this is null and the native window
        // is free. Otherwise Vulkan requires the current live handle.
        swapchain.swapchain,
    ) {
        Ok(mut new_state) => {
            // The old handle is already retired.
            if let Err(error) = frames.resize_images(ctx, new_state.image_count) {
                new_state.destroy(ctx);
                swapchain.destroy(ctx);
                if let Some(mut replacement) = replacement_pipes {
                    replacement.destroy(ctx);
                }
                return Err(error);
            }
            // Install, then release the old state — framebuffers before
            // the render pass they reference.
            let mut old_swapchain = std::mem::replace(swapchain, new_state);
            let old_pipes =
                replacement_pipes.map(|replacement| std::mem::replace(pipes, replacement));
            old_swapchain.destroy(ctx);
            if let Some(mut old_pipes) = old_pipes {
                old_pipes.destroy(ctx);
            }
            frames.needs_recreate = false;
            Ok(())
        }
        Err(error) => {
            // The constructor cleaned its partial replacement. Discard
            // the old chain even if the failure preceded retirement.
            swapchain.destroy(ctx);
            if let Some(mut replacement) = replacement_pipes {
                replacement.destroy(ctx);
            }
            Err(error)
        }
    }
}

/// Records that a present reported suboptimal — the next frame recreates
/// before drawing.
pub(crate) fn mark_suboptimal(needs_recreate: &mut bool) {
    *needs_recreate = true;
}

/// Consumes a pending recreation request; true when one was scheduled.
pub(crate) fn consume_recreate(needs_recreate: &mut bool) -> bool {
    let pending = *needs_recreate;
    *needs_recreate = false;
    pending
}

#[cfg(test)]
mod surface_probe_tests {
    use super::{SurfaceProbeDecision, surface_probe_decision};

    /// The hint-gated probe state table (the batch-3 review's
    /// corrective item 3) - the lifecycle transitions the render loop
    /// implements over this pure decision:
    /// - an ordinary frame (no hints) NEVER probes - no driver
    ///   round-trip;
    /// - a pending recreation on an available surface proceeds (the
    ///   recreation is consumed by its own block - the decision never
    ///   touches the flag);
    /// - a pending recreation on an UNAVAILABLE surface defers AND
    ///   PRESERVES the intent (the review's finding 1: the flag
    ///   survives the deferral; the recreation happens on recovery);
    /// - a prior deferral re-probes until available, then clears the
    ///   hint (recovery);
    /// - the stale-nonzero-window mid-transition case reaches the same
    ///   table through the pending-recreation hint the out-of-date
    ///   acquire path sets.
    ///
    /// The corrective-round extension: EVERY recreation entry point
    /// now routes through this table — the proactive-resize path
    /// (extent mismatch) and the out-of-date PRESENT path both latch
    /// the recreation intent plus the deferral hint and defer, so
    /// their recreation happens only on a frame where the probe
    /// reports the drawable available (ProbeAvailable); and NO
    /// deferral path destroys the slot fence — vkWaitForFences does
    /// not consume a signaled fence, so every images_in_flight
    /// reference stays valid by construction across acquisition
    /// failure, deferral, and recovery (the review's fence finding).
    #[test]
    fn probe_decision_table_preserves_recreation_intent_through_deferral() {
        // Ordinary frame: no evidence of trouble, no probe - the
        // availability answer is not even consulted.
        assert_eq!(
            surface_probe_decision(false, false, true),
            SurfaceProbeDecision::Skip
        );
        assert_eq!(
            surface_probe_decision(false, false, false),
            SurfaceProbeDecision::Skip
        );
        // Pending recreation, drawable available: proceed - the
        // consume-and-recreate block runs this frame.
        assert_eq!(
            surface_probe_decision(false, true, true),
            SurfaceProbeDecision::ProbeAvailable
        );
        // Pending recreation, drawable unavailable: DEFER - and the
        // decision's contract keeps needs_recreate untouched, so the
        // intent survives to the recovery frame (finding 1).
        assert_eq!(
            surface_probe_decision(false, true, false),
            SurfaceProbeDecision::ProbeDeferred
        );
        assert_eq!(
            surface_probe_decision(true, true, false),
            SurfaceProbeDecision::ProbeDeferred
        );
        // Recovery: the deferred hint re-probes; available clears it.
        assert_eq!(
            surface_probe_decision(true, false, true),
            SurfaceProbeDecision::ProbeAvailable
        );
        assert_eq!(
            surface_probe_decision(true, true, true),
            SurfaceProbeDecision::ProbeAvailable
        );
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    #[test]
    fn suboptimal_present_schedules_recreation_until_consumed() {
        let mut needs_recreate = false;
        mark_suboptimal(&mut needs_recreate);
        assert!(needs_recreate);
        assert!(consume_recreate(&mut needs_recreate));
        assert!(!needs_recreate);
        assert!(!consume_recreate(&mut needs_recreate));
    }

    fn capabilities(current: [u32; 2], min: [u32; 2], max: [u32; 2]) -> vk::SurfaceCapabilitiesKHR {
        vk::SurfaceCapabilitiesKHR {
            current_extent: vk::Extent2D {
                width: current[0],
                height: current[1],
            },
            min_image_extent: vk::Extent2D {
                width: min[0],
                height: min[1],
            },
            max_image_extent: vk::Extent2D {
                width: max[0],
                height: max[1],
            },
            ..Default::default()
        }
    }

    #[test]
    fn zero_sized_surface_capabilities_defer_rendering() {
        // The Win32 minimize/transition signal: current, min, and max
        // all report (0,0) — no valid extent exists.
        assert!(surface_extent_unavailable(&capabilities(
            [0, 0],
            [0, 0],
            [0, 0]
        )));
        assert!(surface_extent_unavailable(&capabilities(
            [0, 720],
            [0, 0],
            [0, 0]
        )));
    }

    #[test]
    fn valid_and_unspecified_surfaces_remain_renderable() {
        assert!(!surface_extent_unavailable(&capabilities(
            [1280, 720],
            [1, 1],
            [2560, 1440]
        )));
        // Unspecified extent (headless/software surfaces) stays
        // renderable — the application drives the size.
        assert!(!surface_extent_unavailable(&capabilities(
            [u32::MAX, u32::MAX],
            [1, 1],
            [2560, 1440]
        )));
    }

    #[test]
    fn extent_selection_never_fabricates_nonzero_from_zero() {
        // The rig defect: clamping must not invent a 1×1 swapchain from
        // zero-sized capabilities. The frame path defers before any
        // swapchain ever sees this result.
        let extent = clamp_extent((1280, 720), &capabilities([0, 0], [0, 0], [0, 0]));
        assert_eq!((extent.width, extent.height), (0, 0));
    }

    #[test]
    fn extent_selection_respects_pinned_and_unspecified_surfaces() {
        let pinned = clamp_extent(
            (1280, 720),
            &capabilities([1920, 1080], [1, 1], [3840, 2160]),
        );
        assert_eq!((pinned.width, pinned.height), (1920, 1080));
        let driven = clamp_extent(
            (1280, 720),
            &capabilities([u32::MAX, u32::MAX], [1, 1], [3840, 2160]),
        );
        assert_eq!((driven.width, driven.height), (1280, 720));
    }
}
