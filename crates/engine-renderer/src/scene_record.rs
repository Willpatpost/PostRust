//! Scene recording module — Phase B of architectural split (finding 20).
//!
//! Extracted from `render_frame` in vulkan.rs and `PostResources::record` in
//! post_draw.rs: the unified corner-scene recorder takes the target-compatible
//! pipelines and current camera, handling sky, corner, shadowed/ordinary
//! lighting, proxy, and wet-ground choices in one place.
//!
//! This isolates the scene traversal logic so both the main pass and HDR pass
//! share the same draw sequence, eliminating the duplication that finding 20
//! identified as a maintenance hazard. Findings 3, 5, and 6 are concrete
//! consequences of this split responsibility — this correction unifies the
//! recorder while leaving the preparation and post-processing phases intact.

use crate::vulkan::corner;
use crate::vulkan::shadow_draw;
use crate::vulkan::sky_draw;
use ash::vk;

/// The four target-compatible pipelines the scene recorder binds during
/// the corner frame sequence: sky, lit (ordinary), shadowed, and proxy.
///
/// The caller supplies these as a bundle so the recorder does not need
/// to know which branch (main/HDR) it serves — the pipelines are the
/// only divergence between the two arms.
///
/// # Design choice: Option fields for belt-check style guards
///
/// All pipeline fields are `Option<vk::Pipeline>` to preserve the belt-check
/// style guards of both the main pass and HDR pass:
///
/// - Main pass sky guard: `if let (Some(sky_state), Some(params)) = ...`
///   → `SceneTargetPipelines.sky` is `pipes.sky.map(|s| s.pipeline())`
///
/// - HDR pass sky guard: `if let (Some(sky_state), Some(params), Some(pipeline)) = ...`
///   → `SceneTargetPipelines.sky` is `self.sky_hdr`
///
/// Both arms now have identical guards in the recorder, and un-ensured variants
/// (e.g. `sky_hdr` not yet created) silently skip that draw.
///
/// # Shadowed arm proxy support
///
/// The shadowed arm's proxy draws use the shadowed-populated state's
/// own pipelines and sets (the npc kind's `skinned_shadowed` set with
/// the skinned shadowed pipeline, the vehicle kind's `plain_shadowed`
/// set with the plain shadowed pipeline). The HDR arm now carries the
/// ensured shadowed variants through the family-growth block (the
/// frame flow ensures them lazily at the first populated SHADOWED POST
/// frame). The belt-check below remains for the lazy first-frame window
/// and pipeline-replacement frames (the same guard shape as `sky`).
#[derive(Clone, Copy)]
pub(crate) struct SceneTargetPipelines {
    /// The sky pipeline handle — `Option` for belt-check style guards.
    /// Main pass: `pipes.sky.map(|s| s.pipeline())`; HDR pass: `self.sky_hdr`.
    pub sky: Option<vk::Pipeline>,
    /// The ordinary lit pipeline handle — `Option` for belt-check style.
    /// Main pass: `Some(pipes.lit_pipeline)`; HDR pass: `Some(self.lit_hdr)`.
    pub lit: Option<vk::Pipeline>,
    /// The SHADOWED lit pipeline handle — `Option` for belt-check style.
    /// Main pass: `pipes.shadows.map(|s| s.shadowed(slot).0)`; HDR pass:
    /// `self.shadowed_hdr`.
    pub shadowed: Option<vk::Pipeline>,
    /// The proxy pipeline handle — `Option` for belt-check style.
    /// Main pass: `Some(pipes.proxy_pipeline)`; HDR pass: `Some(self.proxy_hdr)`.
    /// (The VEHICLE kind's pipeline — the npc kind's is `proxy_skinned`.)
    pub proxy: Option<vk::Pipeline>,
    /// The RAIN kind's pipeline handle (the unlit blended particle
    /// variant — the Era-2 package C third populated kind). Main
    /// pass: `Some(pipes.rain_pipeline)`; HDR pass: the post state's
    /// lazily-ensured `rain_hdr` (the belt-check below stays for the
    /// lazy first-frame window, the sky shape). Rain NEVER binds a
    /// shadowed variant: the translucent streaks receive no shadows,
    /// so both the ordinary and the shadowed arm draw them through
    /// this same pipeline with the corner material set.
    pub proxy_rain: Option<vk::Pipeline>,
    /// The SKINNED proxy pipeline handle (the npc kind's — this
    /// render pass's variant). Main pass: `pipes.skinned.map(|s| s.pipeline())`;
    /// HDR pass: the skinned state's `skinned_hdr` (ensured at the
    /// first populated post frame).
    pub proxy_skinned: Option<vk::Pipeline>,
    /// The shadowed-populated npc kind pipeline (the main pass's
    /// skinned shadowed pipeline). HDR passes the ensured variant
    /// (the frame flow's family-growth block; the belt-check below
    /// remains for the lazy first-frame window and pipeline-replacement).
    pub shadowed_proxy_skinned: Option<vk::Pipeline>,
    /// The shadowed-populated vehicle kind pipeline (the main pass's
    /// plain shadowed pipeline). HDR passes the ensured variant.
    pub shadowed_proxy_vehicle: Option<vk::Pipeline>,
}

/// Records the unified corner scene sequence: sky (if present), corner
/// FrameParams write, shadowed/ordinary lighting pass, proxy draw (if
/// present), returning the proxy draw call count when the corner family
/// recorded, `None` when neither corner nor shadowed were active.
///
/// This is the shared draw sequence extracted from `render_frame`'s main
/// pass (the `else` branch of `if let (Some(post_state), true) = (&pipes.post, post)`)
/// and `PostResources::record`'s HDR scene pass (step "1. the HDR scene pass").
///
/// # Arguments
///
/// - `ctx`: the Vulkan context
/// - `pipes`: the pipeline state (used for corner resources and proxy layout)
/// - `cmd`: the recording command buffer
/// - `slot`: the frame slot index
/// - `targets`: the target-compatible pipelines (all `Option` for belt-check guards)
/// - `corner_params`: the corner FrameParams for this frame (may be `None`)
/// - `shadowed`: the shadowed parameters (may be `None` — indicates shadowed mode)
/// - `sky`: the sky FrameParams (may be `None` — sky layer may be inactive)
/// - `proxy`: the proxy upload data (may be `None` — proxy may be inactive)
/// - `proxy_sets`: the proxy descriptor sets (may be `None` if proxy is `None`)
///
/// # Return
///
/// - `Some(proxy_calls)` when the corner family recorded (either shadowed
///   or ordinary arm with proxy present)
/// - `None` when neither corner nor shadowed were active
///
/// # Safety
///
/// - `cmd` must be recording inside the render pass where the scene draws
///   occur (the main pass or HDR scene pass)
/// - `slot` must be this frame's frame slot index
/// - All present pipeline handles in `targets` must be valid and compatible with
///   the current render pass
/// - The corner FrameParams uniform region at `slot` has been written and
///   is disjoint from other slots (guaranteed by the fence-wait before this
///   function is called)
/// - If proxy is present, `proxy_sets` must have been validated before
///   recording and the instance region at `slot` holds this frame's bytes
///
/// # SAFETY ARGUMENTS (preserved from original arms)
///
/// - **Per-slot fence-wait ownership**: each call happens after the slot's
///   fence wait, so the uniform regions (corner FrameParams, proxy instances)
///   hold this frame's bytes and are disjoint from other slots.
/// - **Coherent-region disjointness**: the FrameParams and instance regions
///   do not overlap other slots or the immutable material tail.
/// - **Set-validation**: proxy descriptor sets are validated before recording.
/// - **Instance-region ownership**: the proxy state's instance buffer writes
///   happen immediately before recording inside the same frame flow that
///   first waited the slot's fence.
#[allow(unsafe_op_in_unsafe_fn)]
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn record_corner_scene(
    ctx: &crate::vulkan::VulkanContext,
    pipes: &crate::vulkan::PipelineState,
    cmd: vk::CommandBuffer,
    slot: usize,
    targets: SceneTargetPipelines,
    corner_params: Option<&corner::FrameParams>,
    shadowed: Option<&shadow_draw::ShadowFrameParams>,
    sky: Option<&sky_draw::SkyFrameParams>,
    proxy: Option<&crate::vulkan::proxy_draw::ProxyFrameUpload<'_>>,
    proxy_sets: Option<[vk::DescriptorSet; 3]>,
) -> Option<u32> {
    // 1. sky draw — the main pass's `if let (Some(sky_state), Some(params))`
    //    and HDR's `if let (Some(sky_state), Some(params), Some(pipeline))`.
    //    The caller supplies the target-compatible pipeline (main:
    //    `sky_state.pipeline()`, HDR: the post state's `sky_hdr`).
    if let (Some(sky_state), Some(params), Some(pipeline)) = (&pipes.sky, sky, targets.sky) {
        // SAFETY: this slot's fence wait passed above (the sky uniform
        // region is disjoint per slot); the recording command buffer
        // is inside the render pass, before the scene draws.
        unsafe {
            sky_state.write_frame(slot, params);
            let (set, layout) = sky_state.set_and_layout();
            ctx.device
                .cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
            ctx.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                layout,
                0,
                &[set],
                &[slot as u32 * sky_state.frame_stride()],
            );
            ctx.device.cmd_draw(cmd, 3, 1, 0, 0);
        }
    }

    // 2. the corner FrameParams write — the main pass's and HDR's
    //    identical `if let (Some(resources), Some(params)) = (&ctx.corner,
    //    corner_params)`. The corner buffer must be written before the scene
    //    pass for both the shadowed and ordinary paths (finding 5: the wet
    //    ground re-draw reads this camera block). The corner RESOURCES'
    //    presence gates the corner family — the params may be absent on a
    //    shadowed frame (the shadowed arm supplies its own extended
    //    FrameParams through the shadow state's region, written before the
    //    depth pass), so the write is skipped but the shadowed draw still
    //    runs, exactly as the original arms' guard shapes.
    let corner_resources = ctx.corner.as_ref()?;

    if let Some(params) = corner_params {
        // SAFETY: this slot's fence wait passed above; the coherent frame
        // regions do not overlap other slots or the immutable material tail.
        unsafe { corner_resources.write_frame(slot, params) };
    }

    // 3. shadowed vs ordinary lighting branch — the main pass's and HDR's
    //    `if shadowed.is_some() && let (...)` / `else if corner+params` arms.
    //    The target-compatible pipeline comes from `targets.shadowed` (the
    //    main arm passes the shadow state's own main pipeline handle, the
    //    HDR arm its lazily-ensured `shadowed_hdr`); the tuple supplies the
    //    layout, per-material sets, and frame offset — the state's own
    //    region for the extended FrameParams the depth pass wrote.
    if let (Some(_shadowed_params), Some(shadow_state), Some(shadowed_pipeline)) =
        (shadowed, &pipes.shadows, targets.shadowed)
    {
        let (_, layout, sets, frame_offset) = shadow_state.shadowed(slot);
        // SAFETY: this slot's fence wait passed above (the extended
        // FrameParams region is disjoint per slot); the pipeline matches
        // the render pass the caller is recording inside.
        unsafe {
            ctx.device
                .cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, shadowed_pipeline);
            corner_resources.draw(ctx, cmd, layout, &sets, frame_offset);
        }
        // The shadowed arm draws proxies when the HDR variants are ensured
        // (the frame flow's family-growth block). The belt-check guards the
        // lazy first-frame window and pipeline-replacement frames (the same
        // shape as `sky` — see the struct doc).
        if targets.shadowed_proxy_skinned.is_none() || targets.shadowed_proxy_vehicle.is_none() {
            return Some(0);
        }
        let shadowed_proxy = pipes.shadowed_proxy.as_ref().expect(
            "the shadowed-proxy ensure discipline makes a populated shadowed frame's \
             missing shadowed-proxy state unreachable",
        );
        let retained = ctx.skinned_retained.as_ref().expect(
            "the skinned ensure discipline makes a populated frame's \
             missing retained state unreachable",
        );
        let proxy_state = pipes.proxy.as_ref().expect(
            "the proxy ensure discipline makes a populated shadowed frame's \
             missing proxy state unreachable",
        );
        let calls = if let Some(upload) = proxy {
            // The per-kind proxy draws through the SHADOWED family's
            // own pipelines and sets — the npc kind's skinned-shadowed
            // set (binding 2 = the shadow state's EXTENDED FrameParams
            // at the shadow uniform's stride, binding 4 = this slot's
            // map view, binding 5 = the pose slot) and the vehicle
            // kind's plain-shadowed set. NEVER the unshadowed sets:
            // their binding 2 points at the corner's 112-byte PLAIN
            // block, which the 192-byte shadowed shader would read
            // past (the first wiring draft made exactly that mistake
            // — the discarded-bind bug this repair exists to fix).
            let (skinned_layout, skinned_set, [skinned_frame, skinned_pose]) = shadowed_proxy
                .skinned_shadowed_bind(
                    upload.npc_material,
                    slot,
                    shadow_state.frame_stride(),
                    retained.pose_offset(slot),
                );
            let skinned_bind = crate::vulkan::proxy_draw::SkinnedSetBind {
                layout: skinned_layout,
                set: skinned_set,
                frame_offset: skinned_frame,
                pose_offset: skinned_pose,
            };
            let (vehicle_layout, vehicle_set, vehicle_frame) = shadowed_proxy.plain_shadowed_bind(
                upload.vehicle_material,
                slot,
                shadow_state.frame_stride(),
            );
            let vehicle_bind = crate::vulkan::proxy_draw::PlainSetBind {
                layout: vehicle_layout,
                set: vehicle_set,
                frame_offset: vehicle_frame,
            };
            // The rain kind's bind: the CORNER material set, never a
            // shadowed variant — rain receives no shadows (the
            // translucent streaks' unlit blended pipeline reads only
            // view_proj from the corner FrameParams). The corner
            // region holds this frame's camera (the safe layer
            // always supplies corner_params on a populated frame —
            // both render_corner_populated paths build it), so the
            // corner dynamic stride is the right offset here even on
            // the shadowed arm. The frame flow validated the set
            // alongside the other kinds'; the null fallback is the
            // never-bound belt-check shape (an empty rain kind draws
            // nothing).
            let rain_set = proxy_sets.map(|sets| sets[2]).unwrap_or_else(|| {
                debug_assert!(
                    upload.rain_count == 0,
                    "a populated frame carries the validated corner sets"
                );
                vk::DescriptorSet::null()
            });
            let rain_bind = crate::vulkan::proxy_draw::PlainSetBind {
                layout: pipes.proxy_layout,
                set: rain_set,
                frame_offset: slot as u32 * corner_resources.dynamic_stride(),
            };
            // The rain pipeline for THIS pass (the unlit blended
            // particle object — never a shadowed variant). The
            // fallback mirrors the ordinary arm's skinned fallback:
            // the main-pass object when the HDR variant is not yet
            // ensured, never bound because a populated rain kind
            // implies the ensure ran (the debug_assert fires first
            // in debug builds).
            let rain_pipeline = targets.proxy_rain.unwrap_or_else(|| {
                debug_assert!(
                    upload.rain_count == 0,
                    "a populated rain kind needs this pass's rain pipeline"
                );
                pipes.rain_pipeline
            });
            let pipelines = crate::vulkan::proxy_draw::ProxyPipelines {
                vehicle: targets.shadowed_proxy_vehicle.unwrap(),
                npc: targets.shadowed_proxy_skinned.unwrap(),
                rain: rain_pipeline,
            };
            // SAFETY: the per-kind pipelines are bound with their own
            // layouts inside their render pass (the draw does the
            // binding); the sets are the shadowed-populated state's
            // own (material-validated at the safe layer), EXCEPT the
            // rain kind's — the corner set the frame flow validated
            // (rain receives no shadows, so it never binds the
            // shadowed family); this slot's instance AND pose
            // regions hold this frame's bytes (written above, after
            // the fence wait).
            proxy_state.draw(
                ctx,
                cmd,
                slot,
                upload,
                &pipelines,
                &skinned_bind,
                &vehicle_bind,
                &rain_bind,
            )
        } else {
            0
        };
        return Some(calls);
    }
    if let (Some(_params), Some(lit_pipeline)) = (corner_params, targets.lit) {
        // Ordinary branch: bind the lit pipeline, draw the corner mesh.
        // SAFETY: this slot's fence wait passed above; the coherent frame
        // regions do not overlap other slots or the immutable material tail.
        // The corner buffer was written above (the shadowed path now supplies
        // the camera, so the ordinary branch no longer writes).
        unsafe {
            ctx.device
                .cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, lit_pipeline);
        }
        corner_resources.draw(
            ctx,
            cmd,
            pipes.lit_layout,
            corner_resources.sets(),
            slot as u32 * corner_resources.frame_stride(),
        );

        // 4. proxy draw — the main pass's and HDR's `if let (Some(proxy_state), Some(upload), Some(sets))`
        //    arm. The proxy draws after the static corner mesh (the depth-ordering
        //    note of the main branch, unchanged). The ordinary arm recorded
        //    regardless of the proxy layer's presence (a proxy-less corner
        //    frame still drew the mesh — the original assigned the call
        //    count only in the proxy branch, leaving it at zero).
        let mut calls = 0;
        if let (Some(proxy_state), Some(upload), Some(sets), Some(proxy_pipeline)) =
            (&pipes.proxy, proxy, proxy_sets, targets.proxy)
        {
            // The Era-2 per-kind split: the npc kind's skinned
            // pipeline for THIS pass (the skinned state exists — the
            // frame flow's ensure ran before the fence wait), and its
            // per-material skinned set + the pose slot's dynamic
            // offset. The material index was validated against the
            // corner table (the same 3 materials the skinned sets
            // mirror), so a populated npc kind's set is Some.
            let skinned_state = pipes.skinned.as_ref().expect(
                "the skinned ensure discipline makes a populated frame's \
                 missing skinned pipelines unreachable",
            );
            let retained = ctx.skinned_retained.as_ref().expect(
                "the skinned ensure discipline makes a populated frame's \
                 missing retained state unreachable",
            );
            let skinned_pipeline = targets.proxy_skinned.unwrap_or_else(|| {
                debug_assert!(
                    upload.npc_count == 0,
                    "a populated npc kind needs this pass's skinned pipeline"
                );
                skinned_state.pipeline()
            });
            let skinned_bind = crate::vulkan::proxy_draw::SkinnedSetBind {
                layout: retained.pipeline_layout(),
                set: retained
                    .set(upload.npc_material)
                    .unwrap_or(vk::DescriptorSet::null()),
                frame_offset: slot as u32 * corner_resources.dynamic_stride(),
                pose_offset: retained.pose_offset(slot),
            };
            let vehicle_bind = crate::vulkan::proxy_draw::PlainSetBind {
                layout: pipes.proxy_layout,
                set: sets[1],
                frame_offset: slot as u32 * corner_resources.dynamic_stride(),
            };
            // The rain kind's bind (the Era-2 package C third kind):
            // the rain material's corner set through the SAME
            // PlainSetBind shape as the vehicle kind — one dynamic
            // offset at binding 2 (the corner FrameParams region the
            // unlit particle shader's view_proj reads).
            let rain_bind = crate::vulkan::proxy_draw::PlainSetBind {
                layout: pipes.proxy_layout,
                set: sets[2],
                frame_offset: slot as u32 * corner_resources.dynamic_stride(),
            };
            // The rain pipeline for THIS pass — the main pass's own
            // object, or the HDR variant ensured at the first
            // populated post frame. The fallback mirrors the skinned
            // fallback above: the main-pass object when the HDR
            // variant is not yet ensured, never bound because a
            // populated rain kind implies the ensure ran.
            let rain_pipeline = targets.proxy_rain.unwrap_or_else(|| {
                debug_assert!(
                    upload.rain_count == 0,
                    "a populated rain kind needs this pass's rain pipeline"
                );
                pipes.rain_pipeline
            });
            let pipelines = crate::vulkan::proxy_draw::ProxyPipelines {
                vehicle: proxy_pipeline,
                npc: skinned_pipeline,
                rain: rain_pipeline,
            };
            // SAFETY: the per-kind pipelines are bound with their own
            // layouts inside their render pass (the draw does the
            // binding); `sets` were validated before recording; this
            // slot's instance AND pose regions hold this frame's
            // bytes (written above, after the fence wait).
            let recorded = proxy_state.draw(
                ctx,
                cmd,
                slot,
                upload,
                &pipelines,
                &skinned_bind,
                &vehicle_bind,
                &rain_bind,
            );
            // The recorded count and the pure submission model must
            // agree (the unit test pins the model; this co-pins the
            // loop against it in debug builds).
            debug_assert_eq!(
                recorded,
                crate::vulkan::proxy_draw::proxy_draw_call_count(
                    upload.mode,
                    upload.npc_count,
                    upload.vehicle_count,
                    upload.rain_count
                )
            );
            calls = recorded;
        }
        return Some(calls);
    }

    // Corner resources exist but neither the shadowed nor the ordinary
    // arm could run (no corner params, no shadowed params, or the
    // target's variant pipeline not yet ensured — the belt-check shape).
    None
}
