//! The pass-resource aggregate (P4 S4 — design §5.1's passes
//! responsibility): `PipelineState` — the render pass, the base
//! scene/overlay/lit/proxy/rain pipelines, the scene buffers, and every
//! lazily-created pass-family state (proxy, shadows, sky, post,
//! reflection, skinned, shadowed proxy) with its generation-lifetime
//! ensure/destroy discipline and the S3 plan-driven ensure dispatcher.
//! The per-family modules live under `vulkan/passes/`; the design is
//! silent on a home for the aggregate itself, so it splits out by
//! change reason (pipeline assembly and replacement change together).
//! Pure move from vulkan.rs (stage S4); no logic changed — only
//! location and visibility.

use std::ffi::CStr;

use ash::vk;

use super::RendererError;
use super::context::VulkanContext;
use super::frame_plan::FramePlan;
use super::resources::{
    create_device_local_buffer_with_data, create_mapped_buffer, create_shader_module,
};
use super::swapchain::SwapchainState;
use super::{post_draw, proxy_draw, reflection_draw, shadow_draw, skinned_draw, sky_draw};
use crate::config::{DemoScenePreference, FRAMES_IN_FLIGHT};
use crate::overlay::OverlayVertex;
use crate::scene::SceneVertex;

/// Shader entry point for all Stage 2 pipelines.
const ENTRY_MAIN: &CStr = c"main";

/// Render pass, pipelines, and the scene/overlay vertex + index buffers.
pub(crate) struct PipelineState {
    pub render_pass: vk::RenderPass,
    pub(super) lit_pipeline: vk::Pipeline,
    pub(super) lit_layout: vk::PipelineLayout,
    pub(super) proxy_pipeline: vk::Pipeline,
    /// The Era-2 package C rain pipeline (the third populated kind):
    /// the unlit blended particle pipeline — ONE object,
    /// render-pass-bound (created in the corner branch of pipeline
    /// assembly beside the proxy pipeline, from particle.spv; the
    /// HDR variant is the post state's lazy `rain_hdr` family). The
    /// same proxy vertex input and corner layout as the proxy
    /// pipeline; the depth WRITE flipped off and the overlay blend
    /// attachment (the translucent-streak contract —
    /// particle.wgsl's header). Destroyed in the proxy pipeline's
    /// release path; re-created by the replacement discipline (a
    /// replacement rebuilds it in its own assembly).
    pub(super) rain_pipeline: vk::Pipeline,
    pub(super) proxy_layout: vk::PipelineLayout,
    pub scene_pipeline: vk::Pipeline,
    pub scene_layout: vk::PipelineLayout,
    pub overlay_pipeline: vk::Pipeline,
    pub overlay_layout: vk::PipelineLayout,
    pub scene_vbo: vk::Buffer,
    pub scene_vbo_memory: vk::DeviceMemory,
    pub scene_ibo: vk::Buffer,
    pub scene_ibo_memory: vk::DeviceMemory,
    pub scene_index_count: u32,
    pub region_vbo: vk::Buffer,
    pub region_vbo_memory: vk::DeviceMemory,
    /// The Cycle 6-E instanced-proxy resources (the two mesh VBOs +
    /// the mapped per-slot instance buffer), created LAZILY at the
    /// first populated corner frame — the region-buffer precedent:
    /// the unpopulated path pays nothing. Dies with this state (the
    /// proxy pipeline reading it does); a replacement re-ensures an
    /// identical set before installation (finding A).
    pub proxy: Option<proxy_draw::ProxyResources>,
    /// The Cycle 6-F shadow state (the per-slot shadow maps, the
    /// depth-only pass, the shadowed lit pipeline, and the readback
    /// path), created LAZILY at the first shadowed corner frame —
    /// the region-buffer precedent: the unshadowed corner path pays
    /// nothing and its allocation pins are untouched. Dies with this
    /// state (the pipelines reading it do); a replacement re-ensures
    /// an identical set before installation (finding A).
    pub shadows: Option<shadow_draw::ShadowResources>,
    /// The Era-1 sky state (the fullscreen-triangle pipeline + the
    /// mapped per-slot SkyParams uniform), created LAZILY at the
    /// first sky frame — the region-buffer precedent: the skyless
    /// path pays nothing and its allocation pins are untouched. Dies
    /// with this state (the pipeline reading it does); a replacement
    /// re-ensures an identical set before installation (finding A).
    pub sky: Option<sky_draw::SkyResources>,
    /// The Era-1 post state (the HDR render-target chain: the HDR
    /// scene pass, the bloom mip chain, and the composite pass),
    /// created LAZILY at the first post frame — the region-buffer
    /// precedent: the postless path pays nothing and its allocation
    /// pins are untouched (a postless frame's bytes are identical
    /// to the pre-package renderer's). Dies with this state; a
    /// replacement re-ensures an identical set before installation
    /// (finding A).
    pub post: Option<post_draw::PostResources>,
    /// The Era-1 wet-ground reflection state (the mirrored-scene
    /// pass + the ground re-draw), created LAZILY at the first wet
    /// frame — the same discipline: the dry path pays nothing and
    /// stays byte-identical. Depends on the POST STATE'S HDR RENDER
    /// PASS OBJECT (the reflection pass is render-pass-compatible
    /// with it — the identical attachment descriptions), so a wet
    /// renderer always carries the post state; the post CHAIN
    /// itself stays optional.
    pub reflection: Option<reflection_draw::ReflectionResources>,
    /// The skinned proxy family's PASS-BOUND pipelines (the
    /// main-pass skinned lit pipeline + the lazy HDR variant),
    /// created LAZILY at the first populated frame — the
    /// region-buffer precedent: the unpopulated path pays nothing.
    /// Dies with this state; a replacement rebuilds the pipelines
    /// before installation (finding A) on the CONTEXT's inherited
    /// retained skinned state (the pass-independent pose storage /
    /// pool / sets / layout — never re-created; see
    /// [`VulkanContext::skinned_retained`]).
    pub skinned: Option<skinned_draw::SkinnedProxyResources>,
    /// The shadowed-populated proxy state (the NPC and vehicle
    /// casting/receiving paths through the shadow family), created
    /// LAZILY at the first SHADOWED POPULATED corner frame — one
    /// level deeper than the skinned state (the indirect-command
    /// precedent): the unshadowed populated path pays nothing for a
    /// mechanism it never draws through. Dies with this state (the
    /// pipelines reading it do); a replacement re-ensures an
    /// identical set before installation (finding A).
    pub shadowed_proxy: Option<skinned_draw::ShadowedProxyResources>,
    /// Per-slot vertex capacity of the dynamic region-scene buffer
    /// (Stage 4 graphical integration — the same disjoint per-slot
    /// pattern as the overlay).
    pub region_capacity: u32,
    /// Byte size of one frame slot's region-scene region.
    pub region_slot_bytes: vk::DeviceSize,
    pub(super) region_mapped: *mut core::ffi::c_void,
}

impl PipelineState {
    /// `depth_format` comes from the renderer's single pick (corrective
    /// finding F) so the render pass matches the swapchain's depth
    /// attachments exactly. `demo_scene` is the P3 preference: the
    /// static Stage-2 demonstration scene is uploaded ONLY on the
    /// explicit opt-in — an omitted renderer owns no scene buffers and
    /// the record step's guarded draw skips the static scene.
    pub(crate) fn new(
        ctx: &VulkanContext,
        color_format: vk::Format,
        depth_format: vk::Format,
        demo_scene: DemoScenePreference,
    ) -> Result<PipelineState, RendererError> {
        // Partial-initialization cleanup (corrective finding G): handles
        // start dead; on failure `destroy` releases exactly what exists.
        let mut state = PipelineState {
            render_pass: vk::RenderPass::null(),
            lit_pipeline: vk::Pipeline::null(),
            lit_layout: vk::PipelineLayout::null(),
            proxy_pipeline: vk::Pipeline::null(),
            rain_pipeline: vk::Pipeline::null(),
            proxy_layout: vk::PipelineLayout::null(),
            scene_pipeline: vk::Pipeline::null(),
            scene_layout: vk::PipelineLayout::null(),
            overlay_pipeline: vk::Pipeline::null(),
            overlay_layout: vk::PipelineLayout::null(),
            scene_vbo: vk::Buffer::null(),
            scene_vbo_memory: vk::DeviceMemory::null(),
            scene_ibo: vk::Buffer::null(),
            scene_ibo_memory: vk::DeviceMemory::null(),
            scene_index_count: 0,
            region_vbo: vk::Buffer::null(),
            region_vbo_memory: vk::DeviceMemory::null(),
            region_capacity: 0,
            region_slot_bytes: 0,
            region_mapped: core::ptr::null_mut(),
            proxy: None,
            shadows: None,
            sky: None,
            post: None,
            reflection: None,
            skinned: None,
            shadowed_proxy: None,
        };
        if let Err(error) = state.build(ctx, color_format, depth_format, demo_scene) {
            state.destroy(ctx);
            return Err(error);
        }
        Ok(state)
    }

    fn build(
        &mut self,
        ctx: &VulkanContext,
        color_format: vk::Format,
        depth_format: vk::Format,
        demo_scene: DemoScenePreference,
    ) -> Result<(), RendererError> {
        Self::create_pipelines(ctx, color_format, depth_format, self)?;

        // Scene buffers: upload once through a staging buffer into
        // device-local memory — ONLY on the explicit opt-in (Cycle 3
        // P3): the reusable renderer must not impose the Stage-2
        // demonstration content on a game's scene. `Omitted` leaves
        // the handles null and `scene_index_count` 0, so the record
        // step's guarded draw skips the static scene entirely and the
        // renderer pays no scene-buffer allocation at all.
        if demo_scene == DemoScenePreference::Included {
            let (vertices, indices) = crate::scene::build_scene();
            self.scene_index_count = indices.len() as u32;
            let (scene_vbo, scene_vbo_memory) = create_device_local_buffer_with_data(
                ctx,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                bytemuck::cast_slice(&vertices),
            )?;
            self.scene_vbo = scene_vbo;
            self.scene_vbo_memory = scene_vbo_memory;
            let (scene_ibo, scene_ibo_memory) = create_device_local_buffer_with_data(
                ctx,
                vk::BufferUsageFlags::INDEX_BUFFER,
                bytemuck::cast_slice(&indices),
            )?;
            self.scene_ibo = scene_ibo;
            self.scene_ibo_memory = scene_ibo_memory;
        }

        // Dynamic region-scene buffer (Stage 4 graphical integration):
        // NOT created here — it is allocated lazily on first use
        // (`ensure_region_capacity`) so the default renderer path
        // (no dynamic region geometry) pays no allocation, mapping, or
        // memory cost (Stage 4 final-verification finding). The pattern
        // itself is the overlay's: persistently mapped, disjoint
        // per-slot regions, fenced-slot writes.
        Ok(())
    }

    /// Allocates the dynamic region-scene buffer on first use (the
    /// overlay's mapped-buffer pattern): persistently mapped host
    /// visible memory with one disjoint region per frame slot, sized by
    /// `region_scene_capacity_vertices()` divided by
    /// `FRAMES_IN_FLIGHT`. Called from `render_frame` OUTSIDE command
    /// recording whenever a frame carries region geometry; a no-op once
    /// allocated. Partial-state safe: `destroy` handles the null
    /// handles this leaves on the default path.
    pub(crate) fn ensure_region_capacity(
        &mut self,
        ctx: &VulkanContext,
    ) -> Result<(), RendererError> {
        if self.region_vbo != vk::Buffer::null() {
            return Ok(());
        }
        self.region_capacity = crate::scene::region_scene_capacity_vertices() / FRAMES_IN_FLIGHT;
        self.region_slot_bytes = self.region_capacity as vk::DeviceSize
            * std::mem::size_of::<SceneVertex>() as vk::DeviceSize;
        let region_size = self.region_slot_bytes * FRAMES_IN_FLIGHT as vk::DeviceSize;
        let (region_vbo, region_vbo_memory, region_mapped) =
            create_mapped_buffer(ctx, vk::BufferUsageFlags::VERTEX_BUFFER, region_size)?;
        self.region_vbo = region_vbo;
        self.region_vbo_memory = region_vbo_memory;
        self.region_mapped = region_mapped;
        Ok(())
    }

    /// Allocates the Cycle 6-E proxy resources on first populated
    /// corner frame (the same lazy discipline as the region buffer:
    /// the unpopulated corner path pays nothing). A no-op once
    /// allocated. Partial-state safe: `destroy` handles the `None`
    /// this leaves on the unpopulated path.
    pub(crate) fn ensure_proxy_resources(
        &mut self,
        ctx: &VulkanContext,
    ) -> Result<(), RendererError> {
        if self.proxy.is_some() {
            return Ok(());
        }
        self.proxy = Some(proxy_draw::ProxyResources::new(ctx)?);
        Ok(())
    }

    /// Ensures the skinned family on the first populated frame (the
    /// same lazy discipline: the unpopulated path pays nothing — its
    /// allocation pins are untouched). TWO halves since the Era-2
    /// ownership transfer: the PASS-INDEPENDENT resources (pose
    /// storage, pool, sets, layout) are ensured ON THE CONTEXT —
    /// once ever, inherited by every pipeline generation — and only
    /// the PASS-BOUND pipelines (this state's main-pass skinned
    /// pipeline, rendering inside THIS state's render pass) are
    /// built here, once per state. A pipeline replacement therefore
    /// re-runs this ensure against the inherited retained state and
    /// rebuilds ONLY the pipelines (the flat replacement tuple).
    pub(crate) fn ensure_skinned_resources(
        &mut self,
        ctx: &mut VulkanContext,
    ) -> Result<(), RendererError> {
        ctx.ensure_skinned_retained()?;
        if self.skinned.is_none() {
            let retained = ctx.skinned_retained.as_ref().expect(
                "the retained ensure above makes a missing retained \
                 state unreachable",
            );
            self.skinned = Some(skinned_draw::SkinnedProxyResources::new(
                ctx,
                retained,
                self.render_pass,
            )?);
        }
        Ok(())
    }

    /// Allocates the Cycle 6-F shadow state on the first shadowed
    /// corner frame (the same lazy discipline: the unshadowed corner
    /// path pays nothing — its allocation pins are untouched). The
    /// shadowed lit pipeline renders inside THIS state's render pass,
    /// so the ensure carries it; a no-op once allocated.
    pub(crate) fn ensure_shadow_resources(
        &mut self,
        ctx: &VulkanContext,
    ) -> Result<(), RendererError> {
        if self.shadows.is_some() {
            return Ok(());
        }
        let corner = ctx
            .corner
            .as_ref()
            .ok_or_else(|| RendererError::Vulkan("shadows require an attached scene".into()))?;
        self.shadows = Some(shadow_draw::ShadowResources::new(
            ctx,
            self.render_pass,
            corner,
        )?);
        Ok(())
    }

    /// Allocates the shadowed-populated proxy state on the first
    /// shadowed populated corner frame (one level deeper than the
    /// skinned state — the indirect-command precedent: the
    /// unshadowed populated path pays nothing for a mechanism it
    /// never draws through). The state needs shadows, the context's
    /// RETAINED skinned state (the pose buffer its sets bind), AND
    /// a corner — a no-op once allocated.
    pub(crate) fn ensure_shadowed_proxy_resources(
        &mut self,
        ctx: &VulkanContext,
    ) -> Result<(), RendererError> {
        if self.shadowed_proxy.is_some() {
            return Ok(());
        }
        let corner = ctx.corner.as_ref().ok_or_else(|| {
            RendererError::Vulkan("shadowed proxy requires an attached scene".into())
        })?;
        let shadows = self.shadows.as_ref().ok_or_else(|| {
            RendererError::Vulkan(
                "shadowed proxy needs shadows (ensure_shadow_resources first)".into(),
            )
        })?;
        // The pose buffer comes from the CONTEXT's retained skinned
        // state — the frame flow's ensure order (skinned before the
        // shadowed-populated gate) makes its absence unreachable on
        // a shadowed populated frame.
        let retained = ctx.skinned_retained.as_ref().expect(
            "the skinned ensure discipline makes a missing retained \
             state unreachable on a shadowed populated frame",
        );
        self.shadowed_proxy = Some(skinned_draw::ShadowedProxyResources::new(
            ctx,
            shadows,
            retained,
            corner,
            self.render_pass,
        )?);
        Ok(())
    }

    /// Allocates the Era-1 sky state on the first sky frame (the
    /// same lazy discipline: the skyless path pays nothing — its
    /// allocation pins are untouched). The sky pipeline renders
    /// inside THIS state's render pass, so the ensure carries it; a
    /// no-op once allocated.
    pub(crate) fn ensure_sky_resources(
        &mut self,
        ctx: &VulkanContext,
    ) -> Result<(), RendererError> {
        if self.sky.is_some() {
            return Ok(());
        }
        self.sky = Some(sky_draw::SkyResources::new(ctx, self.render_pass)?);
        Ok(())
    }

    /// Allocates the Era-1 post state on the first post frame (the
    /// same lazy discipline: the postless path pays nothing — its
    /// allocation pins are untouched, and a postless frame's bytes
    /// are identical to the pre-package renderer's). The state's
    /// extent-bound targets are NOT built here (they need the
    /// live swapchain's extent and views — the frame flow ensures
    /// them after the acquire); a no-op once allocated.
    pub(crate) fn ensure_post_resources(
        &mut self,
        ctx: &VulkanContext,
        swapchain_format: vk::Format,
        depth_format: vk::Format,
    ) -> Result<(), RendererError> {
        if self.post.is_some() {
            return Ok(());
        }
        if ctx.corner.is_none() {
            return Err(RendererError::Vulkan(
                "the post chain requires an attached scene".into(),
            ));
        }
        self.post = Some(post_draw::PostResources::new(
            ctx,
            swapchain_format,
            depth_format,
            self.lit_layout,
            self.proxy_layout,
        )?);
        Ok(())
    }

    /// Allocates the Era-1 wet-ground reflection state on the first
    /// wet frame (the same lazy discipline: the dry path pays
    /// nothing and stays byte-identical). The reflection pass rides
    /// the POST STATE'S HDR RENDER PASS — a wet renderer always
    /// carries the post state (the pass object), whether or not
    /// the post chain itself runs.
    pub(crate) fn ensure_reflection_resources(
        &mut self,
        ctx: &VulkanContext,
    ) -> Result<(), RendererError> {
        if self.reflection.is_some() {
            return Ok(());
        }
        let corner = ctx.corner.as_ref().ok_or_else(|| {
            RendererError::Vulkan("the wet reflection requires an attached scene".into())
        })?;
        let post = self.post.as_ref().ok_or_else(|| {
            RendererError::Vulkan(
                "the wet reflection requires the post state's HDR render pass".into(),
            )
        })?;
        self.reflection = Some(reflection_draw::ReflectionResources::new(
            ctx,
            corner,
            post.hdr_render_pass(),
            self.render_pass,
        )?);
        Ok(())
    }

    /// Ensures every state the CURRENT frame's active layers need,
    /// against the live swapchain generation — idempotent (each
    /// ensure_* is an existence check), called BEFORE the recreation
    /// decision (so the replacement knows which lazy states to
    /// replicate — the finding-A discipline) and AGAIN after any
    /// recreation (so the generation that draws carries every active
    /// variant — review finding 6: the replacement replicates only
    /// proxy/indirect/shadow/post, never sky, reflection, or the HDR
    /// variants).
    ///
    /// THE S3 CONSUMER (P4 D2 §4.2): the uses this dispatcher derives
    /// arrive as the frame's EXPLICIT declaration — the
    /// [`FramePlan`] built once per frame at the top of
    /// `render_frame` — instead of being re-derived from the
    /// positional `Option` parameters at each of the THREE call
    /// sites (`prepare_frame`'s pre-recreation ensure, the
    /// `Recreated` arm's re-run, and `prepare_after_recreation`). The
    /// boolean ladder below is unchanged condition-for-condition
    /// (each `uses` bit IS the former `Option` check — the
    /// equivalence is pinned by `frame_plan`'s unit test); only the
    /// derivation moved. The layered family-growth conditions
    /// (`shadowed && proxy`, `post && proxy`, …) are now written
    /// once against the plan's bits.
    pub(super) fn ensure_frame_targets_and_resources(
        &mut self,
        ctx: &mut VulkanContext,
        swapchain: &SwapchainState,
        slot: usize,
        plan: &FramePlan,
    ) -> Result<(), RendererError> {
        let uses = &plan.uses;
        // Lazy region-scene buffer (Stage 4 final-verification finding):
        // allocated on first use only, outside command recording — the
        // default renderer path (no dynamic region geometry) pays no
        // allocation, mapping, or memory cost.
        if uses.region_layers {
            self.ensure_region_capacity(ctx)?;
        }

        // The 6-E lazy proxy resources (the same discipline): created at
        // the first populated frame only, outside command recording —
        // the unpopulated corner path pays nothing. Structural invariant:
        // a proxy upload implies corner resources (the safe layer's
        // scene-preference gate), hence `ctx.corner` in the pattern.
        // The 6-E2 indirect command state is one level lazier still:
        // created at the first INDIRECT-mode populated frame only, so
        // the direct instanced path's allocation pins (the 6-E gates)
        // pay nothing for the mechanism they never touch.
        if uses.populated && ctx.corner.is_some() {
            self.ensure_proxy_resources(ctx)?;
            // The Era-2 companion: the npc kind ALWAYS skins (the
            // pose storage + the skinned pipeline exist from the
            // first populated frame; the vehicle kind's plain path
            // never reads them). One level deeper: the HDR variant
            // waits for a populated POST frame (the family-growth
            // block below).
            self.ensure_skinned_resources(ctx)?;
            if uses.indirect
                && let Some(proxy_state) = self.proxy.as_mut()
            {
                proxy_state.ensure_indirect(ctx)?;
            }
        }

        // The 6-F lazy shadow state (the same discipline): created at
        // the first shadowed frame only, outside command recording — the
        // unshadowed corner path pays nothing. Structural invariant: a
        // shadowed frame implies corner resources (the safe layer's
        // scene-preference gate), which the ensure itself re-checks.
        if uses.shadow_depth {
            self.ensure_shadow_resources(ctx)?;
        }
        // The shadowed-populated proxy state: one level deeper than the
        // skinned state (the indirect-command precedent). The frame
        // flow's ensure discipline guarantees shadows AND skinned exist
        // before the shadowed populated path is reached (shadows and
        // skinned are ensured above, so this is safe).
        if uses.shadow_depth && uses.populated && self.skinned.is_some() {
            self.ensure_shadowed_proxy_resources(ctx)?;
        }

        // The Era-1 lazy sky state (the same discipline): created at the
        // first sky frame only, outside command recording — the skyless
        // path pays nothing, and a pipeline replacement re-ensures an
        // identical set at the next sky frame (the 6-F finding-A shape).
        if uses.sky {
            self.ensure_sky_resources(ctx)?;
        }

        // The Era-1 lazy post state (the same discipline): created at
        // the first post frame only, outside command recording — the
        // postless path pays nothing and stays byte-identical. The
        // family pipelines that depend on LAZY states (the shadowed
        // and sky HDR variants) are ensured in the same order the
        // frame flow ensures their states (shadows, then sky, then
        // post — so the layouts exist by the time the variant
        // pipelines bind them).
        if uses.post {
            self.ensure_post_resources(ctx, swapchain.format, swapchain.depth_format)?;
            if uses.shadow_depth
                && let (Some(post_state), Some(shadow_state)) = (&mut self.post, &self.shadows)
            {
                let layout = shadow_state.shadowed(slot).1;
                post_state.ensure_shadowed_hdr(ctx, layout)?;
            }
            // The Era-2 companion: a populated POST frame needs the
            // skinned state's HDR pipeline variant (the post chain's
            // scene pass renders the proxies into the HDR target —
            // the same family-growth shape as the shadowed HDR
            // variant above; the skinned state itself was ensured
            // with the populated frame above).
            if uses.populated
                && let (Some(post_state), Some(skinned_state)) = (&mut self.post, &mut self.skinned)
            {
                let retained = ctx.skinned_retained.as_ref().expect(
                    "the skinned ensure discipline makes a populated frame's \
                     missing retained state unreachable",
                );
                skinned_state.ensure_hdr(ctx, retained, post_state.hdr_render_pass())?;
            }
            // The Era-2 package C companion: a populated POST frame
            // needs the rain kind's HDR pipeline variant (the HDR
            // scene pass draws rain through the unlit blended
            // pipeline against the HDR attachments — the same
            // family-growth shape as the skinned HDR variant above;
            // the rain kind itself needs no state beyond the proxy
            // family, so this ensure is only the pass-bound
            // pipeline). Per-frame idempotent, so the
            // pipeline-replacement path re-ensures against the new
            // pass exactly like the skinned HDR variant (the
            // finding-A discipline: the replacement replicates the
            // base states, the variants re-ensure lazily).
            if uses.populated
                && let Some(post_state) = &mut self.post
            {
                let proxy_layout = self.proxy_layout;
                post_state.ensure_rain_hdr(ctx, proxy_layout)?;
            }
            // The Era-2 companion's shadowed half: a populated SHADOWED
            // POST frame needs the shadowed-populated state's HDR pipeline
            // variants (the HDR scene pass's shadowed arm draws the proxies
            // through them — the same family-growth shape as the skinned
            // ensure above; the shadowed-proxy state itself was ensured with
            // the shadowed populated frame, before this block). Per-frame
            // idempotent, so the pipeline-replacement path re-ensures against
            // the new pass exactly like the skinned HDR variant (the finding-A
            // discipline: the replacement replicates the base states, the
            // variants re-ensure lazily).
            if uses.populated
                && uses.shadow_depth
                && let (Some(post_state), Some(shadowed_proxy_state)) =
                    (&mut self.post, &mut self.shadowed_proxy)
            {
                let retained = ctx.skinned_retained.as_ref().expect(
                    "the skinned ensure discipline makes a populated frame's \
                     missing retained state unreachable",
                );
                shadowed_proxy_state.ensure_hdr(ctx, retained, post_state.hdr_render_pass())?;
            }
            if uses.sky
                && let (Some(post_state), Some(sky_state)) = (&mut self.post, &self.sky)
            {
                let layout = sky_state.set_and_layout().1;
                post_state.ensure_sky_hdr(ctx, layout)?;
            }
        }

        // The Era-1 lazy wet-reflection state (the same discipline): a
        // wet renderer always carries the post state (the reflection
        // pass rides its HDR render pass object) — ensured ABOVE the
        // post chain's own ensure so the pass exists; the chain itself
        // stays optional.
        if uses.wet {
            self.ensure_post_resources(ctx, swapchain.format, swapchain.depth_format)?;
            self.ensure_reflection_resources(ctx)?;
        }

        Ok(())
    }

    /// Builds the render pass + pipelines for a (possibly new) swapchain
    /// format into `out`. Handles are assigned as they are created
    /// (corrective finding G): a mid-build failure leaves them owned by
    /// the caller's partial state, whose `destroy` releases them.
    fn create_pipelines(
        ctx: &VulkanContext,
        color_format: vk::Format,
        depth_format: vk::Format,
        out: &mut PipelineState,
    ) -> Result<(), RendererError> {
        // Shader modules are consumed by the pipelines; tracked so they
        // are destroyed exactly once whether the build succeeds or fails.
        let mut modules: Vec<vk::ShaderModule> = Vec::new();
        let result = Self::assemble_pipelines(ctx, color_format, depth_format, out, &mut modules);
        unsafe {
            for module in modules.drain(..) {
                ctx.device.destroy_shader_module(module, None);
            }
        }
        result
    }

    fn assemble_pipelines(
        ctx: &VulkanContext,
        color_format: vk::Format,
        depth_format: vk::Format,
        out: &mut PipelineState,
        modules: &mut Vec<vk::ShaderModule>,
    ) -> Result<(), RendererError> {
        let color_attachment = vk::AttachmentDescription::default()
            .format(color_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);
        // The depth attachment's stencil ops are DONT_CARE for the
        // same reason post_draw.rs and shadow_draw.rs state
        // (corrective finding 4): no pipeline in this renderer
        // performs stencil operations — the attachment is depth
        // testing only. Explicit rather than defaulted: ash's
        // AttachmentDescription::default() zero-fills, and
        // stencil_load_op = 0 is LOAD, which the validation layer
        // rejects for a stencil-capable format (the D24 fallback)
        // with an UNDEFINED initialLayout — VUID-VkAttachmentDescription-
        // format-06700 (the forced-D24 post-lifecycle gate caught
        // exactly this: the pass was silent on D32 only because
        // D32_SFLOAT has no stencil aspect).
        let depth_attachment = vk::AttachmentDescription::default()
            .format(depth_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let attachments = [color_attachment, depth_attachment];
        let color_reference = vk::AttachmentReference {
            attachment: 0,
            layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        };
        let depth_reference = vk::AttachmentReference {
            attachment: 1,
            layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
        };
        let color_references = [color_reference];
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_references)
            .depth_stencil_attachment(&depth_reference);
        let dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .src_access_mask(vk::AccessFlags::empty())
            .dst_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            );
        let subpasses = [subpass];
        let dependencies = [dependency];
        let render_pass_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        out.render_pass = unsafe { ctx.device.create_render_pass(&render_pass_info, None) }
            .map_err(|error| RendererError::vk(error, "create_render_pass"))?;

        let scene_vert = include_bytes!(concat!(env!("OUT_DIR"), "/scene.vert.spv"));
        let scene_frag = include_bytes!(concat!(env!("OUT_DIR"), "/scene.frag.spv"));
        let atlas_spv = include_bytes!(concat!(env!("OUT_DIR"), "/overlay_atlas.spv"));

        let scene_vert_module = create_shader_module(ctx, scene_vert)?;
        modules.push(scene_vert_module);
        let scene_frag_module = create_shader_module(ctx, scene_frag)?;
        modules.push(scene_frag_module);
        let overlay_vert_module = create_shader_module(ctx, atlas_spv)?;
        modules.push(overlay_vert_module);
        let overlay_frag_module = create_shader_module(ctx, atlas_spv)?;
        modules.push(overlay_frag_module);

        let push_range = vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(64);
        let push_ranges = [push_range];
        let scene_layout_info =
            vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&push_ranges);
        out.scene_layout = unsafe { ctx.device.create_pipeline_layout(&scene_layout_info, None) }
            .map_err(|error| RendererError::vk(error, "scene layout"))?;
        let layouts = [ctx.descriptors.layout];
        let overlay_layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts);
        out.overlay_layout = unsafe {
            ctx.device
                .create_pipeline_layout(&overlay_layout_info, None)
        }
        .map_err(|error| RendererError::vk(error, "overlay layout"))?;

        let scene_binding = vk::VertexInputBindingDescription {
            binding: 0,
            stride: std::mem::size_of::<SceneVertex>() as u32,
            input_rate: vk::VertexInputRate::VERTEX,
        };
        let scene_attributes = [
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 0,
                format: vk::Format::R32G32B32_SFLOAT,
                offset: 0,
            },
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 1,
                format: vk::Format::R32G32B32_SFLOAT,
                offset: 12,
            },
        ];
        let overlay_binding = vk::VertexInputBindingDescription {
            binding: 0,
            stride: std::mem::size_of::<OverlayVertex>() as u32,
            input_rate: vk::VertexInputRate::VERTEX,
        };
        let overlay_attributes = [
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 0,
                format: vk::Format::R32G32_SFLOAT,
                offset: 0,
            },
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 1,
                format: vk::Format::R32G32B32A32_SFLOAT,
                offset: 8,
            },
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 2,
                format: vk::Format::R32G32_SFLOAT,
                offset: std::mem::offset_of!(OverlayVertex, uv) as u32,
            },
        ];

        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        // The orientation corrective (A5 — the winding audit): with
        // `perspective()` no longer negating Y, exactly ONE Y flip
        // remains on every geometry chain — the shader-boundary
        // conversion — so every triangle's screen-space winding is
        // REVERSED relative to the old double-flip chain, and BOTH
        // pipelines treat CLOCKWISE framebuffer winding as
        // front-facing. The overlay's glyph quads face the SAME
        // direction as the scene geometry (both authored for the
        // y-up, pre-flip convention): under the old shared
        // COUNTER_CLOCKWISE state they were BACK-facing and CULLED —
        // measured on the software ICD (0 glyph pixels with
        // COUNTER_CLOCKWISE, correct top-left glyphs with the facing
        // corrected) — no earlier test ever asserted overlay pixels,
        // so the glyphs had never actually rendered. Both pipelines
        // still cull BACK faces.
        let scene_rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::BACK)
            .front_face(vk::FrontFace::CLOCKWISE)
            .line_width(1.0);
        let overlay_rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::BACK)
            .front_face(vk::FrontFace::CLOCKWISE)
            .line_width(1.0);
        let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state =
            vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let color_blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA);
        let color_blend_attachments = [color_blend_attachment];
        let color_blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&color_blend_attachments);

        let scene_depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(true)
            .depth_compare_op(vk::CompareOp::LESS);
        let scene_bindings = [scene_binding];
        let scene_vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&scene_bindings)
            .vertex_attribute_descriptions(&scene_attributes);
        let scene_input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let scene_stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(scene_vert_module)
                .name(ENTRY_MAIN),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(scene_frag_module)
                .name(ENTRY_MAIN),
        ];
        let scene_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&scene_stages)
            .vertex_input_state(&scene_vertex_input)
            .input_assembly_state(&scene_input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&scene_rasterizer)
            .multisample_state(&multisampling)
            .depth_stencil_state(&scene_depth)
            .color_blend_state(&color_blend)
            .dynamic_state(&dynamic_state)
            .layout(out.scene_layout)
            .render_pass(out.render_pass)
            .subpass(0);
        out.scene_pipeline = unsafe {
            ctx.device
                .create_graphics_pipelines(ctx.pipeline_cache, &[scene_info], None)
        }
        .map_err(|(_, error)| RendererError::vk(error, "scene pipeline"))?[0];

        if let Some(corner) = &ctx.corner {
            use crate::lit_mesh::LitVertex;
            let module = create_shader_module(
                ctx,
                include_bytes!(concat!(env!("OUT_DIR"), "/lit_scene.spv")),
            )?;
            modules.push(module);
            let layouts = [corner.layout];
            // SAFETY: corner owns its descriptor layout across pipeline replacements;
            // the partial PipelineState owns the pipeline layout immediately.
            out.lit_layout = unsafe {
                ctx.device.create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts),
                    None,
                )
            }
            .map_err(|e| RendererError::vk(e, "lit layout"))?;
            let stages = [
                scene_stages[0].module(module).name(c"vs_main"),
                scene_stages[1].module(module).name(c"fs_main"),
            ];
            let bindings = [vk::VertexInputBindingDescription {
                stride: size_of::<LitVertex>() as u32,
                ..scene_binding
            }];
            let attributes = [
                scene_attributes[0],
                vk::VertexInputAttributeDescription {
                    offset: std::mem::offset_of!(LitVertex, normal) as u32,
                    ..scene_attributes[1]
                },
                vk::VertexInputAttributeDescription {
                    location: 2,
                    binding: 0,
                    format: vk::Format::R32G32_SFLOAT,
                    offset: std::mem::offset_of!(LitVertex, uv) as u32,
                },
            ];
            let inputs = vk::PipelineVertexInputStateCreateInfo::default()
                .vertex_binding_descriptions(&bindings)
                .vertex_attribute_descriptions(&attributes);
            // The corner mesh winds CCW about its outward normals.
            let raster = scene_rasterizer.front_face(vk::FrontFace::COUNTER_CLOCKWISE);
            let info = scene_info
                .stages(&stages)
                .vertex_input_state(&inputs)
                .rasterization_state(&raster)
                .layout(out.lit_layout);
            // SAFETY: same render pass, depth and viewport contract as the world pipeline.
            out.lit_pipeline = unsafe {
                ctx.device
                    .create_graphics_pipelines(ctx.pipeline_cache, &[info], None)
            }
            .map_err(|(_, e)| RendererError::vk(e, "lit pipeline"))?[0];

            // The Cycle 6-E instanced-proxy pipeline (see
            // docs/CYCLE6-6E-BRIEF.md): the SAME render pass,
            // descriptor layout, rasterizer (the proxy meshes wind
            // CCW about their outward normals — the lit convention),
            // and depth contract as the lit pipeline. The vertex
            // input carries TWO bindings: binding 0 the proxy mesh at
            // VERTEX rate (the lit 32-byte pos/normal/uv format,
            // locations 0-2), binding 1 the packed instance bytes at
            // INSTANCE rate (stride INSTANCE_STRIDE, locations 3-7
            // as R32G32B32A32_SFLOAT: the four column-major matrix
            // columns then the tint vec4 — the
            // proxy_instances::pack layout, byte-pinned by tests).
            let module = create_shader_module(
                ctx,
                include_bytes!(concat!(env!("OUT_DIR"), "/instanced_proxy.spv")),
            )?;
            modules.push(module);
            // SAFETY: corner owns its descriptor layout across pipeline
            // replacements; the partial PipelineState owns the pipeline
            // layout immediately.
            out.proxy_layout = unsafe {
                ctx.device.create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts),
                    None,
                )
            }
            .map_err(|e| RendererError::vk(e, "proxy layout"))?;
            let proxy_stages = [
                scene_stages[0].module(module).name(c"vs_main"),
                scene_stages[1].module(module).name(c"fs_main"),
            ];
            let mesh_binding = vk::VertexInputBindingDescription {
                binding: 0,
                stride: std::mem::size_of::<crate::proxy_mesh::ProxyVertex>() as u32,
                input_rate: vk::VertexInputRate::VERTEX,
            };
            let instance_binding = vk::VertexInputBindingDescription {
                binding: 1,
                stride: crate::proxy_instances::INSTANCE_STRIDE as u32,
                input_rate: vk::VertexInputRate::INSTANCE,
            };
            let proxy_bindings = [mesh_binding, instance_binding];
            let vec4 = vk::Format::R32G32B32A32_SFLOAT;
            let proxy_attributes = [
                scene_attributes[0],
                scene_attributes[1],
                vk::VertexInputAttributeDescription {
                    location: 2,
                    binding: 0,
                    format: vk::Format::R32G32_SFLOAT,
                    offset: std::mem::offset_of!(crate::proxy_mesh::ProxyVertex, uv) as u32,
                },
                vk::VertexInputAttributeDescription {
                    location: 3,
                    binding: 1,
                    format: vec4,
                    offset: 0,
                },
                vk::VertexInputAttributeDescription {
                    location: 4,
                    binding: 1,
                    format: vec4,
                    offset: 16,
                },
                vk::VertexInputAttributeDescription {
                    location: 5,
                    binding: 1,
                    format: vec4,
                    offset: 32,
                },
                vk::VertexInputAttributeDescription {
                    location: 6,
                    binding: 1,
                    format: vec4,
                    offset: 48,
                },
                vk::VertexInputAttributeDescription {
                    location: 7,
                    binding: 1,
                    format: vec4,
                    offset: 64,
                },
            ];
            let proxy_inputs = vk::PipelineVertexInputStateCreateInfo::default()
                .vertex_binding_descriptions(&proxy_bindings)
                .vertex_attribute_descriptions(&proxy_attributes);
            let proxy_info = vk::GraphicsPipelineCreateInfo::default()
                .stages(&proxy_stages)
                .vertex_input_state(&proxy_inputs)
                .input_assembly_state(&scene_input_assembly)
                .viewport_state(&viewport_state)
                .rasterization_state(&raster)
                .multisample_state(&multisampling)
                .depth_stencil_state(&scene_depth)
                .color_blend_state(&color_blend)
                .dynamic_state(&dynamic_state)
                .layout(out.proxy_layout)
                .render_pass(out.render_pass)
                .subpass(0);
            // SAFETY: same render pass, depth and viewport contract as the lit pipeline.
            out.proxy_pipeline = unsafe {
                ctx.device
                    .create_graphics_pipelines(ctx.pipeline_cache, &[proxy_info], None)
            }
            .map_err(|(_, e)| RendererError::vk(e, "proxy pipeline"))?[0];

            // The Era-2 package C rain pipeline (the THIRD populated
            // kind — the mission's "particle shader variant ... or
            // unlit tinted quads" option, the documented choice in
            // particle.wgsl's header): the SAME vertex input state
            // (the two proxy bindings, locations 0-7), raster,
            // input assembly, viewport, and multisample as the proxy
            // pipeline above — no new pipeline CLASS, one more
            // object through the identical assembly. The DEPTH
            // contract copies `scene_depth` and flips the write flag
            // (a dedicated `rain_depth` value: TEST on, WRITE off —
            // rain is occluded by geometry, never occludes). The
            // COLOR BLEND is the OVERLAY precedent's attachment
            // state verbatim (SRC_ALPHA/ONE_MINUS_SRC_ALPHA, ADD;
            // alpha ONE/ZERO, ADD — defined below it in this
            // assembly; restated here because the corner branch
            // precedes it). The layout is the SAME corner proxy
            // layout, so the rain material's corner sets bind
            // unchanged.
            let rain_module = create_shader_module(
                ctx,
                include_bytes!(concat!(env!("OUT_DIR"), "/particle.spv")),
            )?;
            modules.push(rain_module);
            let rain_stages = [
                scene_stages[0].module(rain_module).name(c"vs_main"),
                scene_stages[1].module(rain_module).name(c"fs_main"),
            ];
            let rain_depth = scene_depth.depth_write_enable(false);
            let rain_blend_attachment = vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::RGBA)
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
                .alpha_blend_op(vk::BlendOp::ADD);
            let rain_blend_attachments = [rain_blend_attachment];
            let rain_blend = vk::PipelineColorBlendStateCreateInfo::default()
                .attachments(&rain_blend_attachments);
            let rain_info = vk::GraphicsPipelineCreateInfo::default()
                .stages(&rain_stages)
                .vertex_input_state(&proxy_inputs)
                .input_assembly_state(&scene_input_assembly)
                .viewport_state(&viewport_state)
                .rasterization_state(&raster)
                .multisample_state(&multisampling)
                .depth_stencil_state(&rain_depth)
                .color_blend_state(&rain_blend)
                .dynamic_state(&dynamic_state)
                .layout(out.proxy_layout)
                .render_pass(out.render_pass)
                .subpass(0);
            // SAFETY: same render pass and viewport contract as the
            // proxy pipeline; the module/layout are the live objects
            // above.
            out.rain_pipeline = unsafe {
                ctx.device
                    .create_graphics_pipelines(ctx.pipeline_cache, &[rain_info], None)
            }
            .map_err(|(_, e)| RendererError::vk(e, "rain pipeline"))?[0];
        }

        let overlay_blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD);
        let overlay_blend_attachments = [overlay_blend_attachment];
        let overlay_blend = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(&overlay_blend_attachments);
        let overlay_no_depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(false)
            .depth_write_enable(false);
        let overlay_bindings = [overlay_binding];
        let overlay_vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&overlay_bindings)
            .vertex_attribute_descriptions(&overlay_attributes);
        let overlay_input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let overlay_stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(overlay_vert_module)
                .name(c"vs_main"),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(overlay_frag_module)
                .name(c"fs_main"),
        ];
        let overlay_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&overlay_stages)
            .vertex_input_state(&overlay_vertex_input)
            .input_assembly_state(&overlay_input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&overlay_rasterizer)
            .multisample_state(&multisampling)
            .depth_stencil_state(&overlay_no_depth)
            .color_blend_state(&overlay_blend)
            .dynamic_state(&dynamic_state)
            .layout(out.overlay_layout)
            .render_pass(out.render_pass)
            .subpass(0);
        out.overlay_pipeline = unsafe {
            ctx.device
                .create_graphics_pipelines(ctx.pipeline_cache, &[overlay_info], None)
        }
        .map_err(|(_, error)| RendererError::vk(error, "overlay pipeline"))?[0];

        Ok(())
    }

    pub(crate) fn destroy(&mut self, ctx: &VulkanContext) {
        unsafe {
            if let Some(proxy) = self.proxy.take() {
                proxy.destroy(ctx);
            }
            if let Some(shadows) = self.shadows.take() {
                shadows.destroy(ctx);
            }
            if let Some(sky) = self.sky.take() {
                sky.destroy(ctx);
            }
            if let Some(post) = self.post.take() {
                post.destroy(ctx);
            }
            if let Some(reflection) = self.reflection.take() {
                reflection.destroy(ctx);
            }
            if let Some(mut skinned) = self.skinned.take() {
                skinned.release(ctx);
            }
            if let Some(mut shadowed_proxy) = self.shadowed_proxy.take() {
                shadowed_proxy.release(ctx);
            }
            if !self.region_mapped.is_null() {
                ctx.device.unmap_memory(self.region_vbo_memory);
                self.region_mapped = core::ptr::null_mut();
            }
            ctx.device.destroy_buffer(self.region_vbo, None);
            ctx.device.free_memory(self.region_vbo_memory, None);
            ctx.device.destroy_buffer(self.scene_ibo, None);
            ctx.device.free_memory(self.scene_ibo_memory, None);
            ctx.device.destroy_buffer(self.scene_vbo, None);
            ctx.device.free_memory(self.scene_vbo_memory, None);
            ctx.device.destroy_pipeline(self.lit_pipeline, None);
            ctx.device.destroy_pipeline_layout(self.lit_layout, None);
            ctx.device.destroy_pipeline(self.proxy_pipeline, None);
            ctx.device.destroy_pipeline(self.rain_pipeline, None);
            ctx.device.destroy_pipeline_layout(self.proxy_layout, None);
            ctx.device.destroy_pipeline(self.overlay_pipeline, None);
            ctx.device
                .destroy_pipeline_layout(self.overlay_layout, None);
            ctx.device.destroy_pipeline(self.scene_pipeline, None);
            ctx.device.destroy_pipeline_layout(self.scene_layout, None);
            ctx.device.destroy_render_pass(self.render_pass, None);
        }
    }
}
