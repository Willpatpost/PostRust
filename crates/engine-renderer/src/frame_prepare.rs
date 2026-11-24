//! Frame preparation module — Phase A of architectural split (finding 20).
//!
//! Extracted from `render_frame` in vulkan.rs: the preparation phase
//! handles swapchain state checks, lazy state initialization, and target
//! creation BEFORE acquire. This isolation enables:
//!
//! - Finding 3's correction: fallible target preparation before acquire,
//!   so allocation failures don't strand acquired images or signaled
//!   semaphores.
//! - Finding 6's correction: preparation against the replacement
//!   swapchain generation ensures all variants (sky, HDR, reflection)
//!   exist on the first frame after recreation.
//!
//! The seam: acquire, submit, and present remain in the frame coordinator
//! (`render_frame`). The preparation function only handles pre-acquire
//! state that must succeed before any synchronization object is touched.
//! The coordinator owns the OUTCOME FLAGS: `prepare_frame` /
//! `prepare_after_recreation` report `Deferred`/`Recreated`/`Proceeded`,
//! and `render_frame` sets `FrameOutcome::{deferred, recreated}` from
//! those arms — the public poll-and-retry contract
//! (`FrameResult::deferred`, `FrameResult::recreated`) must survive any
//! future reshaping of this seam.

use crate::config::DemoScenePreference;
use crate::scene::SceneVertex;
use crate::vulkan::proxy_draw;
use crate::vulkan::reflection_draw::WetCamera;
use crate::vulkan::shadow_draw::ShadowFrameParams;
use crate::vulkan::sky_draw::SkyFrameParams;

/// Result of the preparation phase, indicating what action the frame
/// coordinator should take next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FramePreparationResult {
    /// The drawable was unavailable or probing is pending — defer the
    /// frame without touching synchronization state.
    Deferred,
    /// The swapchain was recreated — the caller must re-run preparation
    /// against the replacement generation.
    Recreated,
    /// Preparation succeeded — proceed to acquire.
    Proceeded,
}

/// Prepares the frame against the active swapchain generation and shared
/// attachment ownership code (findings 3-8).
///
/// This is Phase A of finding 20: extract preparation against the active
/// swapchain generation and the shared attachment ownership code touched
/// by findings 3-8. Leave acquire, submit and present in the frame
/// coordinator.
///
/// The function handles:
/// - Minimized/zero-capability surface deferral
/// - Surface probing and recreation decisions
/// - Lazy state initialization (`pipes.ensure_frame_targets_and_resources`)
/// - Swapchain recreation and state re-initialization
/// - Pre-acquire target creation (post, reflection)
/// - Capture resource preparation
///
/// The `FrameOutcome` is updated with `deferred` and `recreated` flags
/// exactly as they were set in the original `render_frame` body.
///
/// # Return
///
/// - `FramePreparationResult::Deferred` if the frame should be skipped
///   (minimized, probing, or recreate pending)
/// - `FramePreparationResult::Recreated` if swapchain was rebuilt and
///   the caller must run the same preparation again
/// - `FramePreparationResult::Proceeded` if preparation succeeded and
///   acquire can proceed
///
/// # Safety
///
/// All Vulkan operations happen while the context and states are valid.
/// The caller must ensure no synchronization state (fences, semaphores)
/// is modified until this function returns.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_frame(
    ctx: &mut crate::vulkan::VulkanContext,
    swapchain: &mut crate::vulkan::SwapchainState,
    pipes: &mut crate::vulkan::PipelineState,
    frames: &mut crate::vulkan::FrameState,
    window_size: (u32, u32),
    present_preference: crate::config::PresentModePreference,
    replace_pipelines: bool,
    demo_scene: DemoScenePreference,
    proxy: Option<&proxy_draw::ProxyFrameUpload<'_>>,
    shadowed: Option<&ShadowFrameParams>,
    sky: Option<&SkyFrameParams>,
    post: bool,
    wet: Option<&WetCamera>,
    region_vertices: &[SceneVertex],
    foreground_vertices: &[SceneVertex],
    capture_pending: bool,
) -> Result<FramePreparationResult, crate::vulkan::RendererError> {
    let slot = frames.frame_slot;

    // Minimized windows report a zero extent — present nothing.
    if window_size.0 == 0 || window_size.1 == 0 {
        frames.surface_probe_pending = true;
        return Ok(FramePreparationResult::Deferred);
    }

    // A minimized or transitioning surface can report zero-sized
    // capabilities while the application-side window size stays stale
    // and nonzero (Win32: currentExtent and min/max all 0) — the
    // drawable is temporarily unavailable. Defer the whole frame BEFORE
    // any recreation trigger: no swapchain is created, destroyed, or
    // retried and no error is raised; rendering resumes when the
    // surface reports a valid extent again (correction of the 1×1
    // fabrication defect, VUID-VkSwapchainCreateInfoKHR-pNext-07781,
    // found on the rig 2026-10-20).
    let size_changed = swapchain.requested_size != window_size;
    if size_changed || frames.surface_probe_pending || frames.needs_recreate {
        let extent = crate::vulkan::drawable_extent(ctx, window_size)?;
        if crate::vulkan::surface_probe_decision(
            frames.surface_probe_pending || size_changed,
            frames.needs_recreate,
            extent.is_some(),
        ) == crate::vulkan::SurfaceProbeDecision::ProbeDeferred
        {
            frames.surface_probe_pending = true;
            return Ok(FramePreparationResult::Deferred);
        }
        frames.surface_probe_pending = false;
        // A changed application request triggers one recreation even on a
        // surface with a fixed extent. The new swapchain records that request.
        frames.needs_recreate |= size_changed || extent != Some(swapchain.extent);
    }

    // Ensure all lazy frame state before any acquire or recreation
    // decision — the pre-acquire preparation discipline (finding 3's
    // stranded-frame corrective, and finding 6's replacement-variant
    // completeness). Each ensure_* is idempotent (an existence check),
    // so calling it twice (before and after recreation) is safe.
    pipes.ensure_frame_targets_and_resources(
        ctx,
        swapchain,
        slot,
        proxy,
        shadowed,
        sky,
        post,
        wet,
        region_vertices,
        foreground_vertices,
    )?;

    // A previous present reported suboptimal — recreate before drawing
    // (verification requirement: suboptimal must eventually trigger
    // recreation).
    if crate::vulkan::consume_recreate(&mut frames.needs_recreate) {
        crate::vulkan::recreate_swapchain(
            ctx,
            swapchain,
            pipes,
            frames,
            window_size,
            present_preference,
            replace_pipelines,
            demo_scene,
        )?;
        // The generation that draws carries every active variant —
        // calling the helper here ensures sky, reflection, and HDR
        // variant pipelines exist on the new generation (finding 6).
        pipes.ensure_frame_targets_and_resources(
            ctx,
            swapchain,
            slot,
            proxy,
            shadowed,
            sky,
            post,
            wet,
            region_vertices,
            foreground_vertices,
        )?;
        return Ok(FramePreparationResult::Recreated);
    }

    // The capture readback's destination buffer, prepared BEFORE any
    // acquire or fence reset (the post-orientation hardening's
    // frame-sync rule): a refusal here touches no synchronization
    // state, so it can never leave a fence reset-but-unsignaled or an
    // acquired image's semaphore unconsumed. A refused capture
    // (unsupported format, no TRANSFER_SRC, sizing overflow) is NOT a
    // frame error — the request is dropped, recorded as the last
    // refusal, and the frame renders normally; a later request can
    // try again. Deferred frames keep the request alive for the next
    // frame that records.
    if capture_pending && let Err(error) = frames.capture.ensure(ctx, swapchain) {
        frames.capture.pending = false;
        frames.capture.last_refusal = Some(error);
    }

    // The Era-1 post chain's and the wet reflection's extent-bound
    // targets: prepared HERE — BEFORE the slot-fence wait, the fence
    // reset, and the acquire (finding 3's stranded-frame corrective,
    // the capture-ensure discipline above): the swapchain's extent
    // and views are equally live at this point (only a recreation
    // can change them, and every recreation site precedes this
    // block), so a target-creation failure returns with every sync
    // object untouched — no reset-but-unsignaled fence, no signaled
    // acquire semaphore left unconsumed. A rebuild releases the old
    // bundle only when the extent or view set changed, which only a
    // recreation (already device-idle) can cause — the rebuild-safety
    // proof in post_draw's module header. The swapchain generation
    // replaces the view handle for cache invalidation (corrective
    // finding 7): Vulkan permits handle reuse, so a numerical view
    // handle could recur after destruction.
    if post && let Some(post_state) = &mut pipes.post {
        post_state.ensure_targets(
            ctx,
            swapchain.extent,
            &swapchain.views,
            swapchain.generation,
        )?;
    }
    if wet.is_some()
        && let (Some(post_state), Some(reflection_state), Some(corner)) =
            (&pipes.post, &mut pipes.reflection, ctx.corner.as_ref())
    {
        reflection_state.ensure_targets(
            ctx,
            swapchain.extent,
            corner,
            post_state.hdr_render_pass(),
            swapchain.generation,
            swapchain.depth_format,
        )?;
    }

    Ok(FramePreparationResult::Proceeded)
}

/// Re-runs preparation after a swapchain recreation (out-of-date arm).
///
/// This is the reusable helper for the out-of-date inline recreation
/// path. It:
/// 1. Re-creates the swapchain
/// 2. Re-ensures all lazy frame state
/// 3. Re-runs post/target and capture preparation
///
/// The caller must have already detected OUT_OF_DATE and handled the
/// probe decision.
///
/// # Return
///
/// - `FramePreparationResult::Recreated` if swapchain was rebuilt
/// - `FramePreparationResult::Deferred` if probe decision is deferred
/// - `FramePreparationResult::Proceeded` if preparation succeeded
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_after_recreation(
    ctx: &mut crate::vulkan::VulkanContext,
    swapchain: &mut crate::vulkan::SwapchainState,
    pipes: &mut crate::vulkan::PipelineState,
    frames: &mut crate::vulkan::FrameState,
    window_size: (u32, u32),
    present_preference: crate::config::PresentModePreference,
    replace_pipelines: bool,
    demo_scene: DemoScenePreference,
    proxy: Option<&proxy_draw::ProxyFrameUpload<'_>>,
    shadowed: Option<&ShadowFrameParams>,
    sky: Option<&SkyFrameParams>,
    post: bool,
    wet: Option<&WetCamera>,
    region_vertices: &[SceneVertex],
    foreground_vertices: &[SceneVertex],
    capture_pending: bool,
) -> Result<FramePreparationResult, crate::vulkan::RendererError> {
    // The out-of-date inline recreation ALSO probes first (the
    // batch-3 review's directive: every recreation path checks
    // surface availability before recreating) — a mid-transition
    // surface must not be recreated into. Unavailable: PRESERVE
    // the already-signaled slot fence (vkWaitForFences does not
    // consume a signaled fence — it stays signaled until the next
    // submission's owner resets it, so the next frame's wait
    // passes and every images_in_flight reference stays valid;
    // destroying and recreating the fence here would leave
    // per-image references to a dead handle — the review's fence
    // finding), latch the deferral hint, PRESERVE the recreation
    // intent, and defer. No image was acquired, so no semaphore
    // waits.
    if crate::vulkan::surface_probe_decision(
        false,
        true,
        crate::vulkan::drawable_extent(ctx, window_size)?.is_some(),
    ) == crate::vulkan::SurfaceProbeDecision::ProbeDeferred
    {
        crate::vulkan::mark_suboptimal(&mut frames.needs_recreate);
        frames.surface_probe_pending = true;
        return Ok(FramePreparationResult::Deferred);
    }

    crate::vulkan::recreate_swapchain(
        ctx,
        swapchain,
        pipes,
        frames,
        window_size,
        present_preference,
        replace_pipelines,
        demo_scene,
    )?;

    // The generation that draws carries every active variant —
    // re-running the state-ensure helper here creates sky,
    // reflection, and the HDR variant pipelines on the new
    // generation (finding 6: the replacement replicates only
    // proxy/indirect/shadow/post; without this call the first
    // frame after THIS recreation silently skips sky and wet
    // passes).
    let slot = frames.frame_slot;
    pipes.ensure_frame_targets_and_resources(
        ctx,
        swapchain,
        slot,
        proxy,
        shadowed,
        sky,
        post,
        wet,
        region_vertices,
        foreground_vertices,
    )?;

    // The targets were ensured pre-acquire against the OLD
    // generation — repeat preparation against the replacement
    // before retrying the out-of-date acquire (finding 3's
    // stranded-frame corrective; the same frame-sync discipline,
    // still before any acquire of the new chain). The swapchain
    // generation replaces the view handle for cache invalidation
    // (corrective finding 7).
    if post && let Some(post_state) = &mut pipes.post {
        post_state.ensure_targets(
            ctx,
            swapchain.extent,
            &swapchain.views,
            swapchain.generation,
        )?;
    }
    if wet.is_some()
        && let (Some(post_state), Some(reflection_state), Some(corner)) =
            (&pipes.post, &mut pipes.reflection, ctx.corner.as_ref())
    {
        reflection_state.ensure_targets(
            ctx,
            swapchain.extent,
            corner,
            post_state.hdr_render_pass(),
            swapchain.generation,
            swapchain.depth_format,
        )?;
    }
    // The capture ensure is repeated here too (the same frame-sync
    // discipline: a failure here cannot strand a frame).
    if capture_pending && let Err(error) = frames.capture.ensure(ctx, swapchain) {
        frames.capture.pending = false;
        frames.capture.last_refusal = Some(error);
    }

    Ok(FramePreparationResult::Proceeded)
}
