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
//!
//! **Stage S3 (P4 D2 §4.3 — B14's named half):** the two lighting
//! arms' repeated variant pass ordering is consolidated SEMANTICALLY
//! without erasing the specialized GPU pipelines: the wet receiver
//! re-draw (`record_wet_receiver`) and the ordered per-kind proxy
//! sequence (`record_proxy_sequence`) are written ONCE, while each
//! arm keeps its own bind construction — the genuine variation
//! ([`LitBind`]: which descriptor family and dynamic stride each
//! kind's set comes from, the corner's plain 112-byte FrameParams
//! block vs the shadow uniform's extended 192-byte one). The
//! per-variant pipelines, sets, and strides are untouched; only the
//! sequencing they were duplicated around is shared.

use crate::vulkan::corner;
use crate::vulkan::shadow_draw;
use crate::vulkan::sky_draw;
use ash::vk;

/// The wet-ground receiver redraw's placement inside the scene
/// pass (the audit's finding 14 corrective): the recorder slots the
/// ground re-draw BETWEEN the scene's opaque geometry and the
/// depth-tested translucent rain, so the reflection blends over the
/// opaque ground and the rain blends over the reflection — the
/// ordered blend. The pre-corrective order drew the rain first and
/// the ground re-draw (EQUAL depth) after, blending the reflection
/// OVER the already-blended rain — at high reflectivity it erased a
/// foreground streak. The pass variant selects the ground pipeline
/// matching the recording render pass (the main pass's or the HDR
/// scene pass's own object); `None` is the dry frame (no redraw).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GroundRedrawPass {
    /// The dry frame — no ground re-draw.
    None,
    /// The wet frame inside the MAIN render pass.
    Main,
    /// The wet frame inside the HDR scene pass.
    Hdr,
}

impl GroundRedrawPass {
    /// Whether this frame records the ground re-draw at all.
    fn wet(self) -> bool {
        !matches!(self, GroundRedrawPass::None)
    }

    /// The reflection pipeline's own pass flag (the Main/Hdr
    /// discriminant `record_ground_redraw` consumes).
    fn in_hdr(self) -> bool {
        matches!(self, GroundRedrawPass::Hdr)
    }
}

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

/// The per-kind proxy bind family for one lighting variant (P4 D2
/// §4.3, stage S3 — B14's named half): the three per-kind binds the
/// shared proxy sequence ([`record_proxy_sequence`]) consumes. The
/// ordinary and shadowed arms each construct one — the construction
/// is the arms' ONLY genuine divergence (which descriptor family and
/// dynamic stride each kind's set comes from: the skinned state's
/// per-material sets at the CORNER uniform's plain 112-byte
/// FrameParams stride, vs the shadowed-populated state's sets at the
/// SHADOW uniform's extended 192-byte stride) — while the ordered
/// draw sequence is written ONCE. The specialized GPU pipelines are
/// untouched (the charter's "without erasing specialized GPU
/// pipelines"): each variant still binds its own compiled objects
/// and per-material sets — the discriminant is the type-level proof
/// that two bind families exist, exactly the variation the
/// consolidation must preserve.
#[derive(Clone, Copy)]
pub(crate) enum LitBind {
    /// The ordinary arm's family: the skinned state's retained
    /// per-material sets and the corner sets, all offset at the
    /// corner uniform's plain stride.
    Ordinary {
        skinned: crate::vulkan::proxy_draw::SkinnedSetBind,
        vehicle: crate::vulkan::proxy_draw::PlainSetBind,
        rain: crate::vulkan::proxy_draw::PlainSetBind,
    },
    /// The shadowed arm's family: the shadowed-populated state's own
    /// sets at the shadow uniform's extended stride (the npc kind's
    /// `skinned_shadowed_bind`, the vehicle kind's
    /// `plain_shadowed_bind` — never the unshadowed sets: their
    /// binding 2 points at the corner's plain block, which the
    /// 192-byte shadowed shader would read past).
    Shadowed {
        skinned: crate::vulkan::proxy_draw::SkinnedSetBind,
        vehicle: crate::vulkan::proxy_draw::PlainSetBind,
        rain: crate::vulkan::proxy_draw::PlainSetBind,
    },
}

/// Records the wet-ground receiver re-draw — finding 14's
/// ordered-blend slot, ONE implementation for every recording arm
/// (P4 D2 §4.3, stage S3; before it the shadowed and ordinary arms
/// each carried a textually identical `record_ground` closure, and
/// the proxy-less paths called their own copy). The reflection
/// blends over the opaque ground and the rain blends over the
/// reflection — never the reflection over the rain (the
/// pre-corrective order drew the rain first and the ground re-draw
/// (EQUAL depth) after, blending the reflection OVER the
/// already-blended rain — at high reflectivity it erased a
/// foreground streak). A no-op on the dry frame
/// (`GroundRedrawPass::None`) and on a frame whose reflection state
/// or scene attachment is absent (the belt-check shape both arms'
/// closures held, preserved verbatim).
///
/// # Safety
///
/// - `cmd` must be recording inside the scene pass (the main or HDR
///   one), past the opaque draws this placement follows
/// - this slot's corner FrameParams region holds this frame's true
///   camera (the frame flow's write)
/// - the reflection barrier preceded the scene pass (the frame
///   flow's recording order)
unsafe fn record_wet_receiver(
    ctx: &crate::vulkan::VulkanContext,
    pipes: &crate::vulkan::PipelineState,
    cmd: vk::CommandBuffer,
    slot: usize,
    ground: GroundRedrawPass,
) {
    if ground.wet()
        && let (Some(reflection_state), Some(corner)) = (&pipes.reflection, &ctx.corner)
    {
        // SAFETY: this function's documented contract — the same
        // argument the two arms' identical closures carried.
        unsafe {
            reflection_state.record_ground_redraw(
                ctx,
                corner,
                cmd,
                slot,
                ground.in_hdr(),
                corner.receiver_range(),
            );
        }
    }
}

/// Records the shared per-kind proxy sequence — finding 14's
/// ordered blend written ONCE for both lighting variants (P4 D2
/// §4.3, stage S3): the OPAQUE kinds (npc, vehicle), the wet
/// receiver BETWEEN them and the translucent rain kind, then the
/// rain kind. The per-kind pipelines and binds arrive fully
/// constructed ([`ProxyPipelines`] + [`LitBind`]) — the arms'
/// genuine variation — so nothing here knows the lighting variant;
/// the calls are the same ones both arms recorded before the
/// consolidation, in the same order with the same arguments.
///
/// # Safety
///
/// - `cmd` must be recording inside the scene pass, past the mesh
///   draw (the arms' own contracts)
/// - the per-kind pipelines are compatible with the render pass
///   being recorded (each arm selected this pass's variants)
/// - the sets are material-validated (the frame flow's validation,
///   before any synchronization state was touched)
/// - this slot's instance AND pose regions hold this frame's bytes
///   (written after the fence wait, above)
#[allow(clippy::too_many_arguments)]
unsafe fn record_proxy_sequence(
    ctx: &crate::vulkan::VulkanContext,
    pipes: &crate::vulkan::PipelineState,
    cmd: vk::CommandBuffer,
    slot: usize,
    upload: &crate::vulkan::proxy_draw::ProxyFrameUpload<'_>,
    proxy_state: &crate::vulkan::proxy_draw::ProxyResources,
    pipelines: &crate::vulkan::proxy_draw::ProxyPipelines,
    bind: LitBind,
    ground: GroundRedrawPass,
) -> u32 {
    let (skinned, vehicle, rain) = match bind {
        LitBind::Ordinary {
            skinned,
            vehicle,
            rain,
        }
        | LitBind::Shadowed {
            skinned,
            vehicle,
            rain,
        } => (skinned, vehicle, rain),
    };
    // SAFETY: this function's documented contract (the arms' own
    // arguments, unchanged).
    let opaque_calls = unsafe {
        proxy_state.draw_opaque_kinds(ctx, cmd, slot, upload, pipelines, &skinned, &vehicle, &rain)
    };
    // THE WET RECEIVER, between the opaque kinds and the translucent
    // rain kind (finding 14's ordered blend).
    unsafe { record_wet_receiver(ctx, pipes, cmd, slot, ground) };
    let rain_calls = unsafe {
        proxy_state.draw_rain_kind(ctx, cmd, slot, upload, pipelines, &skinned, &vehicle, &rain)
    };
    opaque_calls + rain_calls
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
/// - `ground`: the wet-ground receiver placement (finding 14 — the
///   redraw records between the opaque kinds and the translucent
///   rain kind, or right after the mesh on a proxy-less frame;
///   `GroundRedrawPass::None` is the dry frame)
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
    ground: GroundRedrawPass,
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
        // THE WET RECEIVER PLACEMENT (finding 14): the ground
        // re-draw slots between the scene's OPAQUE draws and the
        // depth-tested TRANSLUCENT rain kind — the reflection blends
        // over the opaque ground, the rain over the reflection (the
        // ordered blend), never the reflection over the rain. The
        // pre-corrective order drew the ground re-draw after the
        // rain: at high reflectivity the EQUAL-depth blend erased a
        // foreground streak. A no-op on the dry frame. S3: the
        // re-draw itself is `record_wet_receiver` — ONE
        // implementation this arm and the ordinary arm share (their
        // two closures were textually identical).
        // The shadowed arm draws proxies when the HDR variants are ensured
        // (the frame flow's family-growth block). The belt-check guards the
        // lazy first-frame window and pipeline-replacement frames (the same
        // shape as `sky` — see the struct doc).
        if targets.shadowed_proxy_skinned.is_none() || targets.shadowed_proxy_vehicle.is_none() {
            // The proxy draws are skipped, so the receiver lands right
            // after the mesh — the last opaque draw this arm made.
            // SAFETY: `record_wet_receiver`'s contract — inside this
            // arm's render pass, past the mesh draw, this slot's
            // corner region holding this frame's camera.
            unsafe { record_wet_receiver(ctx, pipes, cmd, slot, ground) };
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
            // The per-kind bind construction — THE SHADOWED FAMILY
            // (the arm's only genuine variation, P4 D2 §4.3): the
            // npc kind's skinned-shadowed set (binding 2 = the
            // shadow state's EXTENDED FrameParams at the shadow
            // uniform's stride, binding 4 = this slot's map view,
            // binding 5 = the pose slot) and the vehicle kind's
            // plain-shadowed set. NEVER the unshadowed sets: their
            // binding 2 points at the corner's 112-byte PLAIN block,
            // which the 192-byte shadowed shader would read past
            // (the first wiring draft made exactly that mistake —
            // the discarded-bind bug this repair exists to fix).
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
            // The shared ordered sequence (S3): the opaque kinds, the
            // wet receiver between them and the rain kind, the rain
            // kind — the same calls this arm recorded inline before
            // the consolidation.
            // SAFETY: the per-kind pipelines are bound with their own
            // layouts inside their render pass (the draw does the
            // binding); the sets are the shadowed-populated state's
            // own (material-validated at the safe layer), EXCEPT the
            // rain kind's — the corner set the frame flow validated
            // (rain receives no shadows, so it never binds the
            // shadowed family); this slot's instance AND pose
            // regions hold this frame's bytes (written above, after
            // the fence wait).
            unsafe {
                record_proxy_sequence(
                    ctx,
                    pipes,
                    cmd,
                    slot,
                    upload,
                    proxy_state,
                    &pipelines,
                    LitBind::Shadowed {
                        skinned: skinned_bind,
                        vehicle: vehicle_bind,
                        rain: rain_bind,
                    },
                    ground,
                )
            }
        } else {
            // No proxy draws: the receiver lands right after the
            // mesh — the last opaque draw this arm made.
            // SAFETY: `record_wet_receiver`'s contract (inside this
            // arm's render pass, past the mesh draw).
            unsafe { record_wet_receiver(ctx, pipes, cmd, slot, ground) };
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

        // THE WET RECEIVER PLACEMENT (finding 14 — the shadowed arm's
        // own comment documents the ordered blend; the re-draw itself
        // is the SHARED `record_wet_receiver`, S3: this arm's closure
        // was the shadowed arm's textual twin). A no-op on the dry
        // frame.

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
            // The shared ordered sequence (S3): the opaque kinds, the
            // wet receiver between them and the rain kind, the rain
            // kind — the same calls this arm recorded inline before
            // the consolidation.
            // SAFETY: the per-kind pipelines are bound with their own
            // layouts inside their render pass (the draw does the
            // binding); `sets` were validated before recording; this
            // slot's instance AND pose regions hold this frame's
            // bytes (written above, after the fence wait).
            let recorded = unsafe {
                record_proxy_sequence(
                    ctx,
                    pipes,
                    cmd,
                    slot,
                    upload,
                    proxy_state,
                    &pipelines,
                    LitBind::Ordinary {
                        skinned: skinned_bind,
                        vehicle: vehicle_bind,
                        rain: rain_bind,
                    },
                    ground,
                )
            };
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
        } else {
            // No proxy draws: the receiver lands right after the
            // mesh — the last opaque draw this arm made.
            // SAFETY: `record_wet_receiver`'s contract (inside this
            // arm's render pass, past the mesh draw).
            unsafe { record_wet_receiver(ctx, pipes, cmd, slot, ground) };
        }
        return Some(calls);
    }

    // Corner resources exist but neither the shadowed nor the ordinary
    // arm could run (no corner params, no shadowed params, or the
    // target's variant pipeline not yet ensured — the belt-check shape).
    None
}
