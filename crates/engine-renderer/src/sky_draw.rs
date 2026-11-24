//! The sky-pass GPU resources (Era 1 package 3 — see
//! `docs/CYCLE6-ERA1-BRIEF.md`): the fullscreen-triangle pipeline
//! that draws `sky.rs`'s equation (`shaders/sky.wgsl`, the scaffold's
//! term-for-term mirror) BEHIND the scene — first in the main render
//! pass, with NO depth test and NO depth write, so the scene's own
//! LESS-tested fragments draw over it exactly as they drew over the
//! clear color (the only visual change is the background itself).
//!
//! ZERO-PER-FRAME-ALLOCATION DISCIPLINE: every object here is created
//! ONCE, lazily, at the first sky frame ([`PipelineState::
//! ensure_sky_resources`] — the region-buffer precedent: the skyless
//! corner path pays nothing and its allocation pins are untouched).
//! The per-frame SkyParams (the inverse view-projection, the
//! palette, and the disc direction) are written into ONE
//! persistently mapped uniform with a disjoint region per frame
//! slot, after the slot's fence wait — the corner/shadow
//! discipline.
//!
//! THE INVERSE: the vertex shader reconstructs the view ray from
//! `sky::mat4_inverse(view_proj)` (the CPU-computed inverse the
//! renderer uploads — the deterministic core's own tested function;
//! the screen-center ray is definitionally the camera's forward axis,
//! which is what the known-answer gate pins against, independent of
//! the reconstruction).

use super::{RendererError, VulkanContext, create_mapped_buffer, descriptors::create_sets};
use crate::config::FRAMES_IN_FLIGHT;
use crate::sky::SkyPalette;
use ash::vk;

/// The sky pass's per-frame parameters — `shaders/sky.wgsl`'s
/// SkyParams block, byte-identical (mat4 + four vec4s; the alpha
/// lanes carry nothing and are written 0).
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct SkyFrameParams {
    /// The inverse of the corner frame's view-projection (the ray
    /// reconstruction's source; CPU-computed via `sky::mat4_inverse`).
    pub view_proj_inv: [[f32; 4]; 4],
    pub horizon: [f32; 4],
    pub zenith: [f32; 4],
    pub disc_color: [f32; 4],
    /// xyz = the unit direction toward the celestial body.
    pub disc_dir: [f32; 4],
}

const _: () = assert!(size_of::<SkyFrameParams>() == 128);

impl SkyFrameParams {
    /// Builds the block from the palette and the disc direction (the
    /// renderer's own assembly: the direction normalized — the CPU
    /// core's contract — and the alpha lanes zeroed).
    pub(crate) fn new(
        view_proj_inv: [[f32; 4]; 4],
        palette: SkyPalette,
        disc_dir: [f32; 3],
    ) -> Self {
        let length =
            (disc_dir[0] * disc_dir[0] + disc_dir[1] * disc_dir[1] + disc_dir[2] * disc_dir[2])
                .sqrt();
        let unit = if length == 0.0 {
            [0.0, 1.0, 0.0]
        } else {
            [
                disc_dir[0] / length,
                disc_dir[1] / length,
                disc_dir[2] / length,
            ]
        };
        let vec4 = |rgb: [f32; 3]| [rgb[0], rgb[1], rgb[2], 0.0];
        SkyFrameParams {
            view_proj_inv,
            horizon: vec4(palette.horizon),
            zenith: vec4(palette.zenith),
            disc_color: vec4(palette.disc),
            disc_dir: [unit[0], unit[1], unit[2], 0.0],
        }
    }
}

/// The lazily-created sky state, owned by [`super::PipelineState`]
/// (dies with the pipelines that read it; a replacement re-ensures an
/// identical set before installation — finding A).
pub(crate) struct SkyResources {
    /// The mapped per-slot SkyParams uniform (one dynamic set; the
    /// dynamic offset selects the slot's region).
    frame_buffer: vk::Buffer,
    frame_memory: vk::DeviceMemory,
    frame_mapped: *mut core::ffi::c_void,
    frame_stride: u32,
    sky_pipeline: vk::Pipeline,
    sky_layout: vk::PipelineLayout,
    sky_pool: vk::DescriptorPool,
    sky_set_layout: vk::DescriptorSetLayout,
    sky_set: vk::DescriptorSet,
}

impl SkyResources {
    /// Builds the whole sky state against the MAIN render pass (the
    /// sky draws first inside it). Partial-construction safe:
    /// `release` frees exactly what exists.
    pub(super) fn new(
        ctx: &VulkanContext,
        main_render_pass: vk::RenderPass,
    ) -> Result<Self, RendererError> {
        let mut partial = SkyResources {
            frame_buffer: vk::Buffer::null(),
            frame_memory: vk::DeviceMemory::null(),
            frame_mapped: core::ptr::null_mut(),
            frame_stride: 0,
            sky_pipeline: vk::Pipeline::null(),
            sky_layout: vk::PipelineLayout::null(),
            sky_pool: vk::DescriptorPool::null(),
            sky_set_layout: vk::DescriptorSetLayout::null(),
            sky_set: vk::DescriptorSet::null(),
        };
        if let Err(error) = partial.build(ctx, main_render_pass) {
            partial.release(ctx);
            return Err(error);
        }
        Ok(partial)
    }

    fn build(
        &mut self,
        ctx: &VulkanContext,
        main_render_pass: vk::RenderPass,
    ) -> Result<(), RendererError> {
        let slots = FRAMES_IN_FLIGHT as usize;
        // --- the mapped per-slot SkyParams uniform (binding 2, the
        // scaffold shader's own layout choice; one dynamic set).
        // SAFETY: the same live device that owns the mapped UBO.
        let limits = unsafe {
            ctx.instance
                .get_physical_device_properties(ctx.physical_device)
        }
        .limits;
        let align = limits.min_uniform_buffer_offset_alignment.max(1);
        let block = size_of::<SkyFrameParams>() as u64;
        let frame_stride = block.max(align).next_multiple_of(align) as u32;
        let total = u64::from(frame_stride) * slots as u64;
        let (frame_buffer, frame_memory, frame_mapped) =
            create_mapped_buffer(ctx, vk::BufferUsageFlags::UNIFORM_BUFFER, total)?;
        self.frame_buffer = frame_buffer;
        self.frame_memory = frame_memory;
        self.frame_mapped = frame_mapped;
        self.frame_stride = frame_stride;
        let dynamic = vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC;
        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(2)
            .descriptor_type(dynamic)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)];
        let sizes = [vk::DescriptorPoolSize {
            ty: dynamic,
            descriptor_count: 1,
        }];
        let (sky_pool, sky_set_layout, [sky_set]) = create_sets(ctx, &bindings, &sizes, 1)?;
        self.sky_pool = sky_pool;
        self.sky_set_layout = sky_set_layout;
        self.sky_set = sky_set;
        let buffer = [vk::DescriptorBufferInfo::default()
            .buffer(frame_buffer)
            .range(block)];
        let writes = [vk::WriteDescriptorSet::default()
            .dst_set(sky_set)
            .dst_binding(2)
            .descriptor_type(dynamic)
            .buffer_info(&buffer)];
        // SAFETY: fresh set from a live pool; the buffer outlives the
        // pool (released after it below).
        unsafe { ctx.device.update_descriptor_sets(&writes, &[]) };

        // --- the fullscreen-triangle pipeline (no vertex input; NO
        // depth test and NO depth write — drawn FIRST, the scene's
        // own depth-tested fragments then draw over it exactly as
        // they drew over the clear).
        let module =
            super::create_shader_module(ctx, include_bytes!(concat!(env!("OUT_DIR"), "/sky.spv")))?;
        let sky_result: Result<vk::Pipeline, RendererError> = (|| {
            // SAFETY: the module is destroyed exactly once below.
            let stages = [
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::VERTEX)
                    .module(module)
                    .name(c"vs_main"),
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::FRAGMENT)
                    .module(module)
                    .name(c"fs_main"),
            ];
            // No vertex buffers: the triangle is vertex_index-driven.
            let inputs = vk::PipelineVertexInputStateCreateInfo::default();
            let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
                .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
            let viewport_state = vk::PipelineViewportStateCreateInfo::default()
                .viewport_count(1)
                .scissor_count(1);
            let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
                .polygon_mode(vk::PolygonMode::FILL)
                // The fullscreen triangle needs no culling.
                .cull_mode(vk::CullModeFlags::NONE)
                .line_width(1.0);
            let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
                .rasterization_samples(vk::SampleCountFlags::TYPE_1);
            // THE SKY'S DEPTH CONTRACT: no test, no write — the sky
            // never touches the depth attachment, so the scene's
            // LESS-tested draws see the cleared 1.0 exactly as they
            // did before the sky existed (the unchanged-path proof).
            let depth = vk::PipelineDepthStencilStateCreateInfo::default()
                .depth_test_enable(false)
                .depth_write_enable(false);
            let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::RGBA)];
            let blend =
                vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
            let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
            let dynamic_state =
                vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
            let layouts = [sky_set_layout];
            // SAFETY: the set layout is owned above; the partial state
            // owns the pipeline layout immediately.
            let layout = unsafe {
                ctx.device.create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts),
                    None,
                )
            }
            .map_err(|e| RendererError::vk(e, "sky layout"))?;
            self.sky_layout = layout;
            let info = vk::GraphicsPipelineCreateInfo::default()
                .stages(&stages)
                .vertex_input_state(&inputs)
                .input_assembly_state(&assembly)
                .viewport_state(&viewport_state)
                .rasterization_state(&rasterizer)
                .multisample_state(&multisampling)
                .depth_stencil_state(&depth)
                .color_blend_state(&blend)
                .dynamic_state(&dynamic_state)
                .layout(layout)
                .render_pass(main_render_pass)
                .subpass(0);
            // SAFETY: the caller's main render pass owns the
            // compatibility; the partial state owns the pipeline.
            unsafe {
                ctx.device
                    .create_graphics_pipelines(ctx.pipeline_cache, &[info], None)
            }
            .map_err(|(_, e)| RendererError::vk(e, "sky pipeline"))
            .map(|pipelines| pipelines[0])
        })();
        // SAFETY: consumed by the pipelines above (or failed with
        // none alive); destroyed exactly once.
        unsafe { ctx.device.destroy_shader_module(module, None) };
        self.sky_pipeline = sky_result?;
        Ok(())
    }

    /// Writes this slot's SkyParams region.
    ///
    /// # Safety
    /// `slot` must be owned (its fence wait passed).
    pub(super) unsafe fn write_frame(&self, slot: usize, params: &SkyFrameParams) {
        let base = slot as u32 * self.frame_stride;
        // SAFETY: the documented caller contract — this slot's region
        // is disjoint from every other live reader.
        unsafe {
            self.frame_mapped
                .byte_add(base as usize)
                .cast::<SkyFrameParams>()
                .write(*params);
        }
    }

    /// The sky draw's descriptor set and pipeline layout (the
    /// Era-1 post chain's HDR sky pass binds ITS OWN pipeline
    /// against the same set — the state, uniform, and layout are
    /// shared, only the render pass differs).
    pub(super) fn set_and_layout(&self) -> (vk::DescriptorSet, vk::PipelineLayout) {
        (self.sky_set, self.sky_layout)
    }

    /// The sky params' per-slot dynamic-uniform stride (the HDR sky
    /// pass binds the same per-slot offsets the main one does).
    pub(super) fn frame_stride(&self) -> u32 {
        self.frame_stride
    }

    /// Records the sky draw — FIRST inside the main render pass
    /// (before every scene draw), one fullscreen triangle, no depth
    /// interaction.
    ///
    /// # Safety
    /// `cmd` must be recording inside the main render pass, before
    /// the scene draws; this slot's uniform region was written after
    /// the slot's fence wait.
    pub(super) unsafe fn record(&self, ctx: &VulkanContext, cmd: vk::CommandBuffer, slot: usize) {
        // SAFETY: the pipeline and set are the sky state's own; this
        // slot's region holds this frame's bytes (written above,
        // after the fence wait).
        unsafe {
            ctx.device
                .cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.sky_pipeline);
            ctx.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.sky_layout,
                0,
                &[self.sky_set],
                &[slot as u32 * self.frame_stride],
            );
            ctx.device.cmd_draw(cmd, 3, 1, 0, 0);
        }
    }

    pub(super) fn destroy(mut self, ctx: &VulkanContext) {
        self.release(ctx);
    }

    /// Releases every handle exactly once; valid for both the
    /// fully-built and the partial state.
    fn release(&mut self, ctx: &VulkanContext) {
        // SAFETY: owned by this state; destroyed only while the
        // device is idle (replacement waits; teardown waits), after
        // the pipeline that references these objects.
        unsafe {
            if !self.frame_mapped.is_null() {
                ctx.device.unmap_memory(self.frame_memory);
                self.frame_mapped = core::ptr::null_mut();
            }
            ctx.device.destroy_pipeline(self.sky_pipeline, None);
            ctx.device.destroy_pipeline_layout(self.sky_layout, None);
            ctx.device.destroy_descriptor_pool(self.sky_pool, None);
            ctx.device
                .destroy_descriptor_set_layout(self.sky_set_layout, None);
            ctx.device.destroy_buffer(self.frame_buffer, None);
            ctx.device.free_memory(self.frame_memory, None);
        }
    }
}
