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
use super::corner;
use super::frame_plan::FramePlan;
use super::resources::{
    ScopedMappedBuffer, create_device_local_buffer_staged, create_device_local_buffer_with_data,
    create_mapped_buffer, create_shader_module,
};
use super::staging::FlushMode;
use super::swapchain::SwapchainState;
use super::{
    FramePolicy, post_draw, proxy_draw, reflection_draw, shadow_draw, skinned_draw, sky_draw,
};
use crate::config::{DemoScenePreference, FRAMES_IN_FLIGHT, arena_capacity_for};
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
    /// THE B14/R5 SHARED DEPTH ATTACHMENTS (P5's aliasing decision —
    /// the depth pair): the per-slot full-extent depth images the
    /// HDR scene pass and the wet reflection pass ALIAS (one image,
    /// both families' framebuffers). Created by whichever
    /// extent-bound family built first (post or wet — the creation
    /// registered to that family's partial for the fault-injection
    /// unwind, then MOVED here at its build's commit); the second
    /// family creates only its own views. Released AFTER both
    /// families' bundles (their framebuffers bind views of these
    /// images — the coordinated stale release in `frame_prepare`
    /// and this struct's `destroy` order below maintain it).
    pub shared_depth: Option<post_draw::SharedDepthTargets>,
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

/// The corner-family pipeline group held by a SCOPED OWNER while
/// unpublished (CA-01 — the attach-atomicity fix): the lit, proxy,
/// and rain pipelines plus the lit and proxy layouts — the five
/// handles [`PipelineState::assemble_corner_pipelines`] creates
/// against the attached scene's descriptor layout.
///
/// BEFORE CA-01 the assembly wrote these handles DIRECTLY into a
/// `&mut PipelineState`: on the attach path that state is LIVE, so
/// a mid-assembly failure (e.g. the rain pipeline's creation)
/// installed a PARTIAL group — `lit_pipeline` non-null with
/// `proxy_pipeline`/`rain_pipeline` null — while the attach's `?`
/// propagated the error and the caller's rollback (the context's
/// staged scene resources) never ran. The renderer was left with
/// installed resources, no published scene handle, a REFUSED retry
/// (the context's one-scene guard), and — even if the retry had
/// been allowed — [`PipelineState::ensure_corner_pipelines`]'s
/// first-lit-non-null readiness heuristic would have skipped the
/// rebuild and the first scene-backed frame would bind a null
/// pipeline (the S6 rig-found access-violation class).
///
/// This guard closes the window: the assembly writes into the
/// GROUP, [`Drop`] releases exactly what exists (every `?` inside
/// the assembly is leak-free and state-neutral by construction),
/// and [`Self::adopt_into`] moves the completed set into the live
/// pipeline state at the commit point — the only place the handles
/// become reachable by frames. The construction path (the
/// swapchain-recreation arm, where the whole `PipelineState` is
/// itself partial and discarded wholesale on failure) adopts
/// through the same commit; its own destroy handles the null
/// handles a failed build leaves behind.
///
/// The group's objects are counted on the context's census pair
/// (`corner_pipeline_objects` / `corner_pipeline_object_frees`) —
/// the target-images/target-frees counters' own discipline — so
/// the attach battery can prove a failed-and-rolled-back attach
/// released every staged object.
struct CornerPipelineGroup {
    lit_layout: vk::PipelineLayout,
    lit_pipeline: vk::Pipeline,
    proxy_layout: vk::PipelineLayout,
    proxy_pipeline: vk::Pipeline,
    rain_pipeline: vk::Pipeline,
    /// The context that created the objects — `Drop` reaches the
    /// device through it. SAFETY: the raw-borrow shape is confined
    /// to the renderer's single owning thread exactly like the
    /// scoped mapped-buffer guard's; the group exists only inside
    /// one assembly call, adopted or dropped before it returns.
    ctx: *const VulkanContext,
}

impl CornerPipelineGroup {
    fn new(ctx: &VulkanContext) -> Self {
        Self {
            lit_layout: vk::PipelineLayout::null(),
            lit_pipeline: vk::Pipeline::null(),
            proxy_layout: vk::PipelineLayout::null(),
            proxy_pipeline: vk::Pipeline::null(),
            rain_pipeline: vk::Pipeline::null(),
            ctx,
        }
    }

    /// Moves the completed set into the live pipeline state — the
    /// commit point. Disarms the guard (nulls every field) so its
    /// `Drop` cannot double-release what the state now owns; the
    /// state's own `destroy` releases the adopted handles at end of
    /// life (outside the census pair, exactly like committed target
    /// objects).
    fn adopt_into(mut self, state: &mut PipelineState) {
        state.lit_layout = self.lit_layout;
        state.lit_pipeline = self.lit_pipeline;
        state.proxy_layout = self.proxy_layout;
        state.proxy_pipeline = self.proxy_pipeline;
        state.rain_pipeline = self.rain_pipeline;
        self.lit_layout = vk::PipelineLayout::null();
        self.lit_pipeline = vk::Pipeline::null();
        self.proxy_layout = vk::PipelineLayout::null();
        self.proxy_pipeline = vk::Pipeline::null();
        self.rain_pipeline = vk::Pipeline::null();
    }
}

impl Drop for CornerPipelineGroup {
    fn drop(&mut self) {
        let ctx = unsafe { &*self.ctx };
        // Vulkan's per-object destroy calls accept null handles
        // silently, so the unconditional releases are safe for a
        // partially-built group; the census pair below counts only
        // the non-null releases (one per object the group still
        // owns — the destruction half of the no-leak proof).
        unsafe {
            ctx.device.destroy_pipeline(self.lit_pipeline, None);
            ctx.device.destroy_pipeline_layout(self.lit_layout, None);
            ctx.device.destroy_pipeline(self.proxy_pipeline, None);
            ctx.device.destroy_pipeline(self.rain_pipeline, None);
            ctx.device.destroy_pipeline_layout(self.proxy_layout, None);
        }
        if self.lit_pipeline != vk::Pipeline::null() {
            ctx.record_corner_pipeline_object_free();
        }
        if self.lit_layout != vk::PipelineLayout::null() {
            ctx.record_corner_pipeline_object_free();
        }
        if self.proxy_pipeline != vk::Pipeline::null() {
            ctx.record_corner_pipeline_object_free();
        }
        if self.rain_pipeline != vk::Pipeline::null() {
            ctx.record_corner_pipeline_object_free();
        }
        if self.proxy_layout != vk::PipelineLayout::null() {
            ctx.record_corner_pipeline_object_free();
        }
    }
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
            shared_depth: None,
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
            // P5 R3 (A20): the Included scene's VBO and IBO ride ONE
            // pool batch, flushed INLINE — this build's remaining
            // steps (the frames' sync objects) are µs-scale, so no
            // meaningful independent work overlaps the copy; the
            // inline wait is the honest first-milestone shape here.
            // The pool-unavailable arm keeps the per-object helper.
            let mut staging_batch = ctx.begin_staging_batch();
            let vertex_bytes = bytemuck::cast_slice(&vertices);
            let (scene_vbo, scene_vbo_memory) = match &mut staging_batch {
                Some(batch) => create_device_local_buffer_staged(
                    batch,
                    vk::BufferUsageFlags::VERTEX_BUFFER,
                    vertex_bytes,
                ),
                None => create_device_local_buffer_with_data(
                    ctx,
                    vk::BufferUsageFlags::VERTEX_BUFFER,
                    vertex_bytes,
                ),
            }?;
            self.scene_vbo = scene_vbo;
            self.scene_vbo_memory = scene_vbo_memory;
            let index_bytes = bytemuck::cast_slice(&indices);
            let (scene_ibo, scene_ibo_memory) = match &mut staging_batch {
                Some(batch) => create_device_local_buffer_staged(
                    batch,
                    vk::BufferUsageFlags::INDEX_BUFFER,
                    index_bytes,
                ),
                None => create_device_local_buffer_with_data(
                    ctx,
                    vk::BufferUsageFlags::INDEX_BUFFER,
                    index_bytes,
                ),
            }?;
            self.scene_ibo = scene_ibo;
            self.scene_ibo_memory = scene_ibo_memory;
            if let Some(batch) = staging_batch.as_mut() {
                batch.flush(FlushMode::InlineWait)?;
            }
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
    /// visible memory with one disjoint region per frame slot.
    /// Called from `render_frame` OUTSIDE command recording whenever
    /// a frame carries region geometry; a no-op once allocated.
    /// Partial-state safe: `destroy` handles the null handles this
    /// leaves on the default path.
    ///
    /// Since P5 §1.3 (A11/R1) the carve is DEMAND-SIZED under the
    /// configured per-slot budget (`cap` — `RenderConfig`'s
    /// `region_capacity`, default the closed form's 36,800; the
    /// literal `8 + 1000` demo arithmetic no longer sizes this
    /// buffer — the A05-coupling closure). `required` is this
    /// frame's world+foreground vertex supply (the frame plan's
    /// `region_demand_vertices`); the first region frame carves
    /// `max(64, first-demand)` vertices (pow2, clamped to the cap),
    /// and a later frame whose supply exceeds the carve (still
    /// under the cap) grows it once. The F-03 clamp/truncation
    /// machinery is UNTOUCHED: the cap is still the boundary, the
    /// counters still count every truncation (growth never masks —
    /// the counter is not cleared), and a supply at or above the cap
    /// carves exactly the cap (byte-identical with the pre-P5
    /// full-carve behavior the overflow gates pin).
    pub(crate) fn ensure_region_capacity(
        &mut self,
        ctx: &VulkanContext,
        cap: u32,
        required: u32,
    ) -> Result<(), RendererError> {
        let target = arena_capacity_for(self.region_capacity, required, cap);
        if self.region_vbo != vk::Buffer::null() {
            if target <= self.region_capacity {
                return Ok(());
            }
            // GROWTH — the overlay slot's reserve shape on the
            // SHARED per-slot buffer: the region VBO is one buffer
            // object every slot's draws read (disjoint regions,
            // shared handles), so the old buffer's destruction
            // requires no PENDING submission anywhere, not just this
            // slot's fence. `device_wait_idle` is the conservative,
            // already-precedented point (the design's "device-idle
            // recreation window" arm); bounded by the pow2 quantum,
            // counted on the context's arena counter. No content
            // copy: every frame that DRAWS from a slot's region
            // first WROTE it (the fence-before-write discipline), so
            // the first draw after the growth is preceded by that
            // slot's write at the new offsets — the overlay
            // growth's own argument.
            let region_capacity = target;
            let region_slot_bytes = region_capacity as vk::DeviceSize
                * std::mem::size_of::<SceneVertex>() as vk::DeviceSize;
            let region_size = region_slot_bytes * FRAMES_IN_FLIGHT as vk::DeviceSize;
            // CA-11 (the growth-handle-loss fix): the replacement
            // pair lives in a SCOPED OWNER from creation to commit —
            // a failure at the fallible idle wait below drops the
            // guard, which destroys the NEW buffer and memory (the
            // old bindings intact), instead of dropping the bare
            // handles on the floor as the pre-fix tuple did. The
            // staging/release events land on the census pair
            // (`arena_replacements`/`arena_replacement_frees`).
            let replacement =
                ScopedMappedBuffer::create(ctx, vk::BufferUsageFlags::VERTEX_BUFFER, region_size)?;
            // The replacement point: the new buffer exists before
            // the old one dies (a failure above leaves the prior
            // state fully intact), and the event is counted. The
            // stall is the renderer's own ordered-teardown helper
            // (device_wait_idle, typed). The CA-11 fault knob sits
            // immediately before the wait: a plan armed here fires
            // with the replacement allocated and the old carve still
            // live — the exact leak window the guard closes.
            ctx.check_lifecycle_fault(crate::LifecycleFaultSite::RegionGrowthIdle)?;
            super::device_idle(ctx)?;
            let (region_vbo, region_vbo_memory, region_mapped) = replacement.commit();
            unsafe {
                if !self.region_mapped.is_null() {
                    ctx.device.unmap_memory(self.region_vbo_memory);
                }
                ctx.device.destroy_buffer(self.region_vbo, None);
                ctx.device.free_memory(self.region_vbo_memory, None);
            }
            self.region_vbo = region_vbo;
            self.region_vbo_memory = region_vbo_memory;
            self.region_mapped = region_mapped;
            self.region_capacity = region_capacity;
            self.region_slot_bytes = region_slot_bytes;
            ctx.record_arena_growth();
            return Ok(());
        }
        if target == 0 {
            // No demand and nothing allocated (the replacement path's
            // unallocated shape, or a zero-budget cap): nothing to
            // carve.
            return Ok(());
        }
        self.region_capacity = target;
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
    ///
    /// Since P5 §1.3 (A11/R1) the instance carve is DEMAND-SIZED
    /// under the configured cap (`cap` — `RenderConfig`'s
    /// `instance_capacity`, default MAX_INSTANCES): the first
    /// populated frame carves `max(64, first-demand)` instances
    /// (pow2, clamped to the cap) — the repack's typed refusal at
    /// the CAP is unchanged, and the carve grows toward it on
    /// demand.
    pub(crate) fn ensure_proxy_resources(
        &mut self,
        ctx: &VulkanContext,
        cap: u32,
        required: u32,
    ) -> Result<(), RendererError> {
        let existing = self.proxy.as_ref().map_or(0, |state| state.capacity());
        // The creation floor is 1 instance (96 B): a degenerate
        // zero-cap config still refuses every non-empty population
        // at the CAP (the typed boundary) while an EMPTY population
        // keeps a live buffer for the instance-rate vertex binding —
        // a zero-sized VkBuffer is spec-invalid, never created.
        // (CA-12: `cap` is the construction-VALIDATED u32 — the same
        // number the CPU repack refuses above, no re-narrowing.)
        let target = arena_capacity_for(existing, required.max(1), cap).max(1);
        match &mut self.proxy {
            Some(state) => state.ensure_capacity(ctx, target)?,
            None => {
                self.proxy = Some(proxy_draw::ProxyResources::new(ctx, target)?);
            }
        }
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
    ///
    /// Since P5 §1.3 (A11/R1) the pose carve is DEMAND-SIZED under
    /// the configured cap through the CONTEXT's ensure (the
    /// device-lifetime tier), and — because this state's
    /// SHADOWED-POPULATED family keeps its own binding-5 sets
    /// against the retained pose buffer — a growth leaves them
    /// stale until the re-ensure below rewrites them (the
    /// staleness token is the buffer handle; the shadowed family's
    /// own ladder step re-checks it too).
    pub(crate) fn ensure_skinned_resources(
        &mut self,
        ctx: &mut VulkanContext,
        pose_cap: u32,
        required_blocks: u32,
    ) -> Result<(), RendererError> {
        ctx.ensure_skinned_retained(required_blocks, pose_cap)?;
        if let Some(shadowed) = &mut self.shadowed_proxy {
            let retained = ctx.skinned_retained.as_ref().expect(
                "the retained ensure above makes a missing retained \
                 state unreachable",
            );
            shadowed.ensure_pose_binding(ctx, retained)?;
        }
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
    ///
    /// Since P5 §1.3 (A11/R1) the existing-state arm re-ensures the
    /// family's binding-5 sets against the retained pose buffer's
    /// CURRENT handle: a pose-carve growth re-created that buffer,
    /// and this family's sets (per-generation, unlike the retained's
    /// own) would otherwise point at a destroyed buffer.
    pub(crate) fn ensure_shadowed_proxy_resources(
        &mut self,
        ctx: &VulkanContext,
    ) -> Result<(), RendererError> {
        if let Some(shadowed) = &mut self.shadowed_proxy {
            // The frame flow's ensure order (skinned before the
            // shadowed-populated gate) makes the retained state's
            // existence unreachable to miss here.
            let retained = ctx.skinned_retained.as_ref().expect(
                "the skinned ensure discipline makes a missing retained \
                 state unreachable on a shadowed populated frame",
            );
            return shadowed.ensure_pose_binding(ctx, retained);
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
        policy: FramePolicy,
    ) -> Result<(), RendererError> {
        let uses = &plan.uses;
        // Lazy region-scene buffer (Stage 4 final-verification finding):
        // allocated on first use only, outside command recording — the
        // default renderer path (no dynamic region geometry) pays no
        // allocation, mapping, or memory cost. P5 §1.3: the carve is
        // sized by THIS frame's demand (the plan's count) under the
        // configured per-slot budget (the policy's cap).
        if uses.region_layers {
            self.ensure_region_capacity(ctx, policy.region_capacity, plan.region_demand_vertices)?;
        }

        // The 6-E lazy proxy resources (the same discipline): created at
        // the first populated frame only, outside command recording —
        // the unpopulated corner path pays nothing. Structural invariant:
        // a proxy upload implies corner resources (the safe layer's
        // scene-preference gate), hence `ctx.corner` in the pattern.
        // P5 §1.3: the instance and pose carves size by this frame's
        // demand under the configured caps. The 6-E2 indirect command
        // state is one level lazier still: created at the first
        // INDIRECT-mode populated frame only, so the direct instanced
        // path's allocation pins (the 6-E gates) pay nothing for the
        // mechanism they never touch.
        if uses.populated && ctx.corner.is_some() {
            self.ensure_proxy_resources(ctx, policy.instance_capacity, plan.instance_demand)?;
            // The Era-2 companion: the npc kind ALWAYS skins (the
            // pose storage + the skinned pipeline exist from the
            // first populated frame; the vehicle kind's plain path
            // never reads them). One level deeper: the HDR variant
            // waits for a populated POST frame (the family-growth
            // block below).
            self.ensure_skinned_resources(ctx, policy.pose_capacity, plan.pose_demand_blocks)?;
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
            // CA-01: the corner group STAGES here too — the
            // construction path's whole `PipelineState` is partial
            // and discarded wholesale on failure, but the group's
            // own scoped release keeps the partial state's destroy
            // from depending on handles this branch never installed.
            let group = Self::assemble_corner_pipelines(ctx, corner, out.render_pass)?;
            group.adopt_into(out);
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

    /// Assembles the scene-backed (corner-family) pipelines — the lit
    /// mesh, the Cycle 6-E instanced proxy, and the Era-2 package C
    /// rain object — against the ATTACHED scene's descriptor layout
    /// and the given render pass. THE P4 S6 EXTRACTION: this body was
    /// `assemble_pipelines`' corner branch, runnable only when the
    /// scene predates the pipeline state (the deleted `Corner`
    /// construction preference's guarantee). With the public
    /// [`crate::Renderer::attach_scene`] attaching AFTER
    /// construction, the same assembly runs at attach time through
    /// [`Self::ensure_corner_pipelines`] — one body, two call sites
    /// (the swapchain-recreation path still builds at pipeline-state
    /// creation, where the scene already exists). The config structs
    /// below are the branch's own, rebuilt verbatim — plain
    /// deterministic data, no ctx-derived handles.
    ///
    /// CA-01 (attach atomicity): the assembly STAGES its five
    /// handles into a returned [`CornerPipelineGroup`] — a scoped
    /// owner whose `Drop` releases exactly what exists — so a
    /// failure at ANY creation step is state-neutral and leak-free
    /// for the caller's live pipeline state. The two call sites
    /// `adopt_into` the completed group only on full success: the
    /// attach path's rollback (see
    /// [`crate::Renderer::attach_scene`]) can therefore restore the
    /// renderer to exactly its pre-attach state, and the readiness
    /// heuristic in [`Self::ensure_corner_pipelines`] is sound (a
    /// non-null `lit_pipeline` implies the WHOLE group committed).
    /// The three shader modules are consumed by pipeline creation and
    /// released here (the caller's `modules` list is uninvolved —
    /// module objects may be destroyed once the pipelines that
    /// captured them exist).
    fn assemble_corner_pipelines(
        ctx: &VulkanContext,
        corner: &corner::CornerResources,
        render_pass: vk::RenderPass,
    ) -> Result<CornerPipelineGroup, RendererError> {
        // The branch's shared config, verbatim (the scene pipeline's
        // own assembly above defines each value's contract).
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
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let scene_rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
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
        let scene_input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let stage = |flags, module, name| {
            vk::PipelineShaderStageCreateInfo::default()
                .stage(flags)
                .module(module)
                .name(name)
        };

        // The lit mesh pipeline (lit_scene.spv — the shader pair the
        // S5 composition serves): the scene binding at the LIT
        // 32-byte stride, the normal/uv attributes at the LitVertex
        // offsets, CCW front face (the lit meshes wind about their
        // outward normals).
        let mut modules: Vec<vk::ShaderModule> = Vec::new();
        let mut group = CornerPipelineGroup::new(ctx);
        let result = Self::assemble_corner_pipeline_objects(
            ctx,
            corner,
            render_pass,
            &mut group,
            &scene_binding,
            &scene_attributes,
            &viewport_state,
            &scene_rasterizer,
            &multisampling,
            &color_blend,
            &scene_depth,
            &scene_input_assembly,
            &dynamic_state,
            stage,
            &mut modules,
        );
        // SAFETY: the modules were consumed by the pipeline creations
        // above (or the build failed — the pipelines that captured a
        // module exist only after its creation call succeeded).
        unsafe {
            for module in modules.drain(..) {
                ctx.device.destroy_shader_module(module, None);
            }
        }
        result.map(|()| group)
    }

    /// The corner branch's pipeline-creation body (module lifetime
    /// owned by the caller for exactly-once release). See
    /// [`Self::assemble_corner_pipelines`] for the extraction's
    /// contract — and for the CA-01 group-staging discipline this
    /// body's `group` parameter carries (the writes land in the
    /// scoped owner, never directly in a live pipeline state).
    #[allow(clippy::too_many_arguments)]
    fn assemble_corner_pipeline_objects(
        ctx: &VulkanContext,
        corner: &corner::CornerResources,
        render_pass: vk::RenderPass,
        group: &mut CornerPipelineGroup,
        scene_binding: &vk::VertexInputBindingDescription,
        scene_attributes: &[vk::VertexInputAttributeDescription; 2],
        viewport_state: &vk::PipelineViewportStateCreateInfo,
        scene_rasterizer: &vk::PipelineRasterizationStateCreateInfo,
        multisampling: &vk::PipelineMultisampleStateCreateInfo,
        color_blend: &vk::PipelineColorBlendStateCreateInfo,
        scene_depth: &vk::PipelineDepthStencilStateCreateInfo,
        scene_input_assembly: &vk::PipelineInputAssemblyStateCreateInfo,
        dynamic_state: &vk::PipelineDynamicStateCreateInfo,
        stage: impl Fn(
            vk::ShaderStageFlags,
            vk::ShaderModule,
            &'static std::ffi::CStr,
        ) -> vk::PipelineShaderStageCreateInfo,
        modules: &mut Vec<vk::ShaderModule>,
    ) -> Result<(), RendererError> {
        use crate::lit_mesh::LitVertex;
        let module = create_shader_module(
            ctx,
            include_bytes!(concat!(env!("OUT_DIR"), "/lit_scene.spv")),
        )?;
        modules.push(module);
        // CA-01 fault knob: the step succeeded — a plan armed here
        // fires with the module created, exercising the caller's
        // module-drain release.
        ctx.check_lifecycle_fault(crate::LifecycleFaultSite::LitModule)?;
        let layouts = [corner.layout];
        // SAFETY: corner owns its descriptor layout across pipeline replacements;
        // the group's scoped owner owns the pipeline layout immediately.
        group.lit_layout = unsafe {
            ctx.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts),
                None,
            )
        }
        .map_err(|e| RendererError::vk(e, "lit layout"))?;
        ctx.record_corner_pipeline_object();
        ctx.check_lifecycle_fault(crate::LifecycleFaultSite::LitLayout)?;
        let stages = [
            stage(vk::ShaderStageFlags::VERTEX, module, c"vs_main"),
            stage(vk::ShaderStageFlags::FRAGMENT, module, c"fs_main"),
        ];
        let bindings = [vk::VertexInputBindingDescription {
            stride: size_of::<LitVertex>() as u32,
            ..*scene_binding
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
        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&inputs)
            .input_assembly_state(scene_input_assembly)
            .viewport_state(viewport_state)
            .rasterization_state(&raster)
            .multisample_state(multisampling)
            .depth_stencil_state(scene_depth)
            .color_blend_state(color_blend)
            .dynamic_state(dynamic_state)
            .layout(group.lit_layout)
            .render_pass(render_pass)
            .subpass(0);
        // SAFETY: same render pass, depth and viewport contract as the world pipeline.
        group.lit_pipeline = unsafe {
            ctx.device
                .create_graphics_pipelines(ctx.pipeline_cache, &[info], None)
        }
        .map_err(|(_, e)| RendererError::vk(e, "lit pipeline"))?[0];
        ctx.record_corner_pipeline_object();
        ctx.check_lifecycle_fault(crate::LifecycleFaultSite::LitPipeline)?;

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
        // CA-01 fault knob: the proxy module exists — the drain below
        // releases it when a later step's plan fires.
        ctx.check_lifecycle_fault(crate::LifecycleFaultSite::ProxyModule)?;
        // SAFETY: corner owns its descriptor layout across pipeline
        // replacements; the group's scoped owner owns the pipeline
        // layout immediately.
        group.proxy_layout = unsafe {
            ctx.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts),
                None,
            )
        }
        .map_err(|e| RendererError::vk(e, "proxy layout"))?;
        ctx.record_corner_pipeline_object();
        ctx.check_lifecycle_fault(crate::LifecycleFaultSite::ProxyLayout)?;
        let proxy_stages = [
            stage(vk::ShaderStageFlags::VERTEX, module, c"vs_main"),
            stage(vk::ShaderStageFlags::FRAGMENT, module, c"fs_main"),
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
            .input_assembly_state(scene_input_assembly)
            .viewport_state(viewport_state)
            .rasterization_state(&raster)
            .multisample_state(multisampling)
            .depth_stencil_state(scene_depth)
            .color_blend_state(color_blend)
            .dynamic_state(dynamic_state)
            .layout(group.proxy_layout)
            .render_pass(render_pass)
            .subpass(0);
        // SAFETY: same render pass, depth and viewport contract as the lit pipeline.
        group.proxy_pipeline = unsafe {
            ctx.device
                .create_graphics_pipelines(ctx.pipeline_cache, &[proxy_info], None)
        }
        .map_err(|(_, e)| RendererError::vk(e, "proxy pipeline"))?[0];
        ctx.record_corner_pipeline_object();
        ctx.check_lifecycle_fault(crate::LifecycleFaultSite::ProxyPipeline)?;

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
        // alpha ONE/ZERO, ADD). The layout is the SAME corner
        // proxy layout, so the rain material's corner sets bind
        // unchanged.
        let rain_module = create_shader_module(
            ctx,
            include_bytes!(concat!(env!("OUT_DIR"), "/particle.spv")),
        )?;
        modules.push(rain_module);
        // CA-01 fault knob: the rain module exists — the drain below
        // releases it when the rain pipeline's plan fires.
        ctx.check_lifecycle_fault(crate::LifecycleFaultSite::RainModule)?;
        let rain_stages = [
            stage(vk::ShaderStageFlags::VERTEX, rain_module, c"vs_main"),
            stage(vk::ShaderStageFlags::FRAGMENT, rain_module, c"fs_main"),
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
        let rain_blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&rain_blend_attachments);
        let rain_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&rain_stages)
            .vertex_input_state(&proxy_inputs)
            .input_assembly_state(scene_input_assembly)
            .viewport_state(viewport_state)
            .rasterization_state(&raster)
            .multisample_state(multisampling)
            .depth_stencil_state(&rain_depth)
            .color_blend_state(&rain_blend)
            .dynamic_state(dynamic_state)
            .layout(group.proxy_layout)
            .render_pass(render_pass)
            .subpass(0);
        // SAFETY: same render pass and viewport contract as the
        // proxy pipeline; the module/layout are the live objects
        // above.
        group.rain_pipeline = unsafe {
            ctx.device
                .create_graphics_pipelines(ctx.pipeline_cache, &[rain_info], None)
        }
        .map_err(|(_, e)| RendererError::vk(e, "rain pipeline"))?[0];
        ctx.record_corner_pipeline_object();
        ctx.check_lifecycle_fault(crate::LifecycleFaultSite::RainPipeline)?;
        Ok(())
    }

    /// The attach-time corner-pipeline ensure (P4 S6): with the scene
    /// attached AFTER construction (the public
    /// [`crate::Renderer::attach_scene`] — the only path since the
    /// `Corner` construction preference's deletion), the corner-family
    /// pipelines are assembled HERE, against the live render pass and
    /// the attached scene's descriptor layout — exactly the objects
    /// [`Self::assemble_pipelines`] builds when the scene predates the
    /// pipeline state (the swapchain-recreation path). IDEMPOTENT: a
    /// state that already carries the objects (a prior ensure, or a
    /// construction-time build on the recreation path) returns
    /// unchanged; a state with no attached scene returns unchanged
    /// (the pre-S6 Omitted shape — no scene, no corner pipelines).
    ///
    /// CA-01 (attach atomicity): the readiness test below —
    /// `lit_pipeline` non-null — is SOUND only because the assembly
    /// stages its whole group in a [`CornerPipelineGroup`] and adopts
    /// it in ONE commit: a non-null `lit_pipeline` therefore implies
    /// the lit/proxy/rain pipelines AND both layouts exist. Before
    /// the group staging, a mid-assembly failure could leave
    /// `lit_pipeline` set with `proxy_pipeline`/`rain_pipeline`
    /// null, and this test would have skipped the rebuild — the
    /// null-pipeline bind the S6 rig found as an access violation.
    /// The attach caller's rollback guard releases the staged scene
    /// resources when THIS fails, so the failed attach leaves the
    /// renderer exactly as before and the retry rebuilds everything.
    pub(crate) fn ensure_corner_pipelines(
        &mut self,
        ctx: &VulkanContext,
    ) -> Result<(), RendererError> {
        if self.lit_pipeline != vk::Pipeline::null() {
            return Ok(());
        }
        let Some(corner) = ctx.corner.as_ref() else {
            return Ok(());
        };
        let render_pass = self.render_pass;
        let group = Self::assemble_corner_pipelines(ctx, corner, render_pass)?;
        group.adopt_into(self);
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
            // THE B14/R5 RELEASE ORDER (load-bearing): the shared
            // depth images die only AFTER both families' bundles —
            // their framebuffers bound views of these images, and
            // the object-lifetime rule requires every view (and the
            // framebuffers referencing it) destroyed before the
            // image it views.
            if let Some(mut shared_depth) = self.shared_depth.take() {
                shared_depth.release(ctx);
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
