//! The skinned-proxy GPU resources (Era-2 stage 2c — the animation +
//! skinning + proxy shadows package): the per-frame-slot mapped POSE
//! STORAGE buffer (the per-instance skin matrices the
//! `instanced_proxy_skinned` vertex shader reads through binding 5)
//! and the skinned lit pipeline (the NPC kind's path — the vehicle
//! kind stays on the plain proxy pipeline, byte-identical).
//!
//! ZERO-PER-FRAME-ALLOCATION DISCIPLINE (the standing gate): every
//! object here is created ONCE, lazily, at the FIRST populated
//! corner frame ([`PipelineState::ensure_skinned_resources`] — the
//! region-buffer precedent: the unpopulated path pays nothing, and
//! the unpopulated allocation pins are untouched). The per-frame
//! pose bytes are the safe layer's repacked blocks
//! ([`crate::animation::repack_poses`] into the renderer's RETAINED
//! Vec — the instance-bytes pattern) copied into THIS slot's mapped
//! region after the slot's fence wait. `renderer.allocations()`
//! therefore stays constant across warmed populated frames.
//!
//! SYNCHRONIZATION: the pose buffer follows the instance buffer's
//! discipline exactly — slot S's region is written only inside the
//! frame that first waited slot S's fence (the previous submission
//! through S, whose vertex shaders read this region, has completed),
//! before the commands reading it are recorded and submitted. The
//! host-visible HOST_COHERENT write is visible to the draws of the
//! submission that follows it (the queue-submit ordering guarantee).
//!
//! THE DESCRIPTOR SETS: one set per corner material (3), each
//! binding the corner's OWN resources — the material's albedo view
//! and immutable nearest sampler at binding 0, the corner uniform
//! buffer's per-slot FrameParams region at binding 2 (the DYNAMIC
//! offset selects the slot — the same `frame_offset` the corner mesh
//! draws bind), the immutable material tail at binding 3 (a STATIC
//! offset), and the pose buffer at binding 5 (STORAGE_BUFFER_DYNAMIC
//! — the dynamic offset selects this slot's region, so the shader's
//! `poses[i]` indexes relative to the slot base). Two dynamic
//! offsets per set bind: [frame_offset, pose_offset] in binding
//! order. The sets are render-pass-AGNOSTIC; only the pipelines are
//! pass-specific (the main-pass pipeline here, the HDR variant
//! lazily at the first populated POST frame — the
//! `ensure_shadowed_hdr` precedent).
//!
//! THE ALIGNMENT PROOF (minStorageBufferOffsetAlignment): every
//! slot's dynamic offset is `slot * 2,883,584`, and 2,883,584 =
//! 4096 × 704 with 704 = 44 × 16 — a multiple of 16, hence of every
//! power-of-two alignment ≥ 1 the Vulkan spec permits for storage
//! buffers. The constructor still ASSERTS the multiple against the
//! live device's limit and fails typed otherwise (the proof covers
//! the spec's minimums; a driver reporting something exotic gets the
//! refusal, not the undefined behavior).
//!
//! THE POSE BLOCK CONTRACT: the caller's blocks are in POSE-INDEX
//! order (block i serves every npc whose `pose_index` is i — the
//! safe layer validates `max(npc pose_index) < blocks.len()` before
//! any frame state is touched); at most [`MAX_POSE_BLOCKS`] blocks
//! (the typed refusal, never truncation).

use super::corner::CornerResources;
use super::{
    RendererError, VulkanContext, create_mapped_buffer, create_shader_module,
    descriptors::create_sets,
};
use crate::animation::{MAX_POSE_BLOCKS, POSE_STRIDE};
use crate::config::FRAMES_IN_FLIGHT;
use crate::proxy_instances::INSTANCE_STRIDE;
use crate::proxy_mesh::ProxyVertex;
use ash::vk;

/// The pose storage buffer's slot stride: one 704-byte block per
/// possible pose index (MAX_POSE_BLOCKS), one region per frame slot.
const SLOT_STRIDE: u64 = MAX_POSE_BLOCKS as u64 * POSE_STRIDE as u64;

/// The corner material table's length (the skinned sets' fixed
/// count — one set per material, `corner.rs`'s own three).
const MATERIAL_COUNT: usize = 3;

/// Device-lifetime skinned proxy resources. Owned by
/// [`super::PipelineState`] (dies with the pipelines that read it; a
/// pipeline replacement re-ensures an identical set before
/// installation — the finding-A discipline, `vulkan.rs`'s
/// replacement path).
pub(crate) struct SkinnedProxyResources {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: *mut core::ffi::c_void,
    slot_stride: u64,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    sets: [vk::DescriptorSet; MATERIAL_COUNT],
    /// The main-pass skinned lit pipeline (vs_main/fs_main of
    /// instanced_proxy_skinned.spv).
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    /// The HDR-pass variant (created lazily at the first populated
    /// POST frame — the ensure_shadowed_hdr precedent; reuses THIS
    /// state's pipeline layout and sets, which are pass-agnostic).
    skinned_hdr: Option<vk::Pipeline>,
}

impl SkinnedProxyResources {
    /// Builds the whole skinned state against the MAIN render pass
    /// and the corner's resources. Partial-construction safe:
    /// `release` frees exactly what exists (null handles denote
    /// partial construction — the corrective-finding-G discipline).
    pub(super) fn new(
        ctx: &VulkanContext,
        main_render_pass: vk::RenderPass,
        corner: &CornerResources,
    ) -> Result<Self, RendererError> {
        // The alignment proof's live check (the header's arithmetic
        // covers the spec minimums; the device gets the final word).
        // SAFETY: querying the live physical device this context was
        // built on (the corner's own build does the same).
        let limits = unsafe {
            ctx.instance
                .get_physical_device_properties(ctx.physical_device)
        }
        .limits;
        let min_alignment = limits.min_storage_buffer_offset_alignment.max(1);
        if !SLOT_STRIDE.is_multiple_of(min_alignment) {
            return Err(RendererError::Vulkan(format!(
                "the pose buffer's slot stride {SLOT_STRIDE} is not a multiple of the \
                 device's minStorageBufferOffsetAlignment {min_alignment}"
            )));
        }

        let mut partial = SkinnedProxyResources {
            buffer: vk::Buffer::null(),
            memory: vk::DeviceMemory::null(),
            mapped: core::ptr::null_mut(),
            slot_stride: SLOT_STRIDE,
            set_layout: vk::DescriptorSetLayout::null(),
            pool: vk::DescriptorPool::null(),
            sets: [vk::DescriptorSet::null(); MATERIAL_COUNT],
            pipeline: vk::Pipeline::null(),
            pipeline_layout: vk::PipelineLayout::null(),
            skinned_hdr: None,
        };
        if let Err(error) = partial.build(ctx, main_render_pass, corner) {
            partial.release(ctx);
            return Err(error);
        }
        Ok(partial)
    }

    fn build(
        &mut self,
        ctx: &VulkanContext,
        main_render_pass: vk::RenderPass,
        corner: &CornerResources,
    ) -> Result<(), RendererError> {
        // The mapped pose storage: one disjoint region per frame
        // slot, each holding a full bounded pose population (the
        // instance buffer's carve).
        (self.buffer, self.memory, self.mapped) = create_mapped_buffer(
            ctx,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            self.slot_stride * FRAMES_IN_FLIGHT as u64,
        )?;

        // The skinned set layout: the corner's own binding set (0
        // albedo + immutable sampler, 2 FrameParams dynamic, 3
        // material static) plus binding 5 — the pose storage, read
        // in the VERTEX stage (the Era-2 skinning convention; the
        // build's binding map passes 5 through unchanged).
        let frag = vk::ShaderStageFlags::FRAGMENT;
        let vert = vk::ShaderStageFlags::VERTEX;
        let combined = vk::DescriptorType::COMBINED_IMAGE_SAMPLER;
        let uniform_dynamic = vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC;
        let uniform = vk::DescriptorType::UNIFORM_BUFFER;
        let storage_dynamic = vk::DescriptorType::STORAGE_BUFFER_DYNAMIC;
        let samplers = [corner.nearest_sampler()];
        let binding = |index, ty, stages| {
            vk::DescriptorSetLayoutBinding::default()
                .binding(index)
                .descriptor_type(ty)
                .descriptor_count(1)
                .stage_flags(stages)
        };
        let bindings = [
            binding(0, combined, frag).immutable_samplers(&samplers),
            binding(2, uniform_dynamic, frag | vert),
            binding(3, uniform, frag),
            binding(5, storage_dynamic, vert),
        ];
        let sizes = [combined, uniform_dynamic, uniform, storage_dynamic].map(|ty| {
            vk::DescriptorPoolSize {
                ty,
                descriptor_count: MATERIAL_COUNT as u32,
            }
        });
        let (pool, set_layout, sets) = create_sets(ctx, &bindings, &sizes, MATERIAL_COUNT as u32)?;

        // The set writes: every binding points at the corner's OWN
        // resources (the texture views, the uniform buffer's
        // FrameParams regions and immutable material tail) plus this
        // state's pose buffer — written ONCE here, before any
        // submission (the corner set writes' discipline).
        let frame_range = CornerResources::frame_params_size();
        for (material, set) in sets.iter().enumerate() {
            let image = [vk::DescriptorImageInfo::default()
                .image_view(corner.texture_view(material))
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let frame = [vk::DescriptorBufferInfo::default()
                .buffer(corner.uniform_buffer())
                .range(frame_range)];
            let material_info = [vk::DescriptorBufferInfo::default()
                .buffer(corner.uniform_buffer())
                .offset(corner.material_offset(material))
                .range(std::mem::size_of::<[f32; 4]>() as u64)];
            let poses = [vk::DescriptorBufferInfo::default()
                .buffer(self.buffer)
                .range(vk::WHOLE_SIZE)];
            let write = vk::WriteDescriptorSet::default().dst_set(*set);
            // SAFETY: fresh sets; the corner resources and the pose
            // buffer outlive the pool (owned by this state / the
            // context); all offsets/ranges are the corner's own
            // arithmetic.
            unsafe {
                ctx.device.update_descriptor_sets(
                    &[
                        write
                            .dst_binding(0)
                            .descriptor_type(combined)
                            .image_info(&image),
                        write
                            .dst_binding(2)
                            .descriptor_type(uniform_dynamic)
                            .buffer_info(&frame),
                        write
                            .dst_binding(3)
                            .descriptor_type(uniform)
                            .buffer_info(&material_info),
                        write
                            .dst_binding(5)
                            .descriptor_type(storage_dynamic)
                            .buffer_info(&poses),
                    ],
                    &[],
                );
            }
        }
        self.pool = pool;
        self.set_layout = set_layout;
        self.sets = sets;

        // The skinned lit pipeline: the SAME render pass, rasterizer
        // (the proxy meshes wind CCW about their outward normals),
        // depth contract, and blend as the plain proxy pipeline —
        // the vertex input and the LAYOUT differ (the SkinnedVertex
        // format with joints/weights at locations 8/9, the instance
        // params vec4's pose index at location 10; the skinned set
        // layout with the pose storage).
        // SAFETY: the layout is owned by this state immediately; the
        // create-info slices live through the call.
        self.pipeline_layout = unsafe {
            ctx.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&[self.set_layout]),
                None,
            )
        }
        .map_err(|e| RendererError::vk(e, "skinned proxy layout"))?;
        self.pipeline = self.build_pipeline(ctx, main_render_pass, "skinned proxy pipeline")?;
        Ok(())
    }

    /// The skinned lit pipeline against `render_pass` — the plain
    /// proxy pipeline's states with the SKINNED vertex input (the
    /// one pipeline builder both the main-pass and HDR variants
    /// call; the states exist exactly once).
    fn build_pipeline(
        &self,
        ctx: &VulkanContext,
        render_pass: vk::RenderPass,
        what: &str,
    ) -> Result<vk::Pipeline, RendererError> {
        let module = create_shader_module(
            ctx,
            include_bytes!(concat!(env!("OUT_DIR"), "/instanced_proxy_skinned.spv")),
        )?;
        let result = self.pipeline_from_module(ctx, module, render_pass, what);
        // SAFETY: consumed by the pipeline above (or the build failed
        // with none alive); destroyed exactly once.
        unsafe { ctx.device.destroy_shader_module(module, None) };
        result
    }

    /// The pipeline assembly proper (the module is the caller's to
    /// destroy — this shape lets a failed build release exactly the
    /// module it created, the assemble_pipelines discipline).
    fn pipeline_from_module(
        &self,
        ctx: &VulkanContext,
        module: vk::ShaderModule,
        render_pass: vk::RenderPass,
        what: &str,
    ) -> Result<vk::Pipeline, RendererError> {
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
        // Binding 0: the skinned mesh at VERTEX rate — the 64-byte
        // SkinnedVertex (pos/normal/uv at the ProxyVertex offsets,
        // joints R32G32B32A32_UINT at 32, weights at 48). Binding 1:
        // the packed instance bytes at INSTANCE rate — the plain
        // pipeline's locations 3-7 PLUS location 10 (the params
        // vec4's x, the pose block index) at byte offset 80.
        let mesh_binding = vk::VertexInputBindingDescription {
            binding: 0,
            stride: std::mem::size_of::<crate::animation::SkinnedVertex>() as u32,
            input_rate: vk::VertexInputRate::VERTEX,
        };
        let instance_binding = vk::VertexInputBindingDescription {
            binding: 1,
            stride: INSTANCE_STRIDE as u32,
            input_rate: vk::VertexInputRate::INSTANCE,
        };
        let bindings = [mesh_binding, instance_binding];
        let vec4 = vk::Format::R32G32B32A32_SFLOAT;
        let attributes = [
            vk::VertexInputAttributeDescription {
                location: 0,
                binding: 0,
                format: vk::Format::R32G32B32_SFLOAT,
                offset: std::mem::offset_of!(ProxyVertex, pos) as u32,
            },
            vk::VertexInputAttributeDescription {
                location: 1,
                binding: 0,
                format: vk::Format::R32G32B32_SFLOAT,
                offset: std::mem::offset_of!(ProxyVertex, normal) as u32,
            },
            vk::VertexInputAttributeDescription {
                location: 2,
                binding: 0,
                format: vk::Format::R32G32_SFLOAT,
                offset: std::mem::offset_of!(ProxyVertex, uv) as u32,
            },
            // The skinning inputs (the SkinnedVertex tail).
            vk::VertexInputAttributeDescription {
                location: 8,
                binding: 0,
                format: vk::Format::R32G32B32A32_UINT,
                offset: 32,
            },
            vk::VertexInputAttributeDescription {
                location: 9,
                binding: 0,
                format: vec4,
                offset: 48,
            },
            // The instance-rate locations (the plain pipeline's own
            // 3-7, plus the params vec4's pose index at 10).
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
            vk::VertexInputAttributeDescription {
                location: 10,
                binding: 1,
                format: vec4,
                offset: 80,
            },
        ];
        let inputs = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attributes);
        // The plain proxy pipeline's own states: CCW winding, FILL,
        // back-cull, LESS depth with write, one sample, RGBA write,
        // dynamic viewport/scissor.
        let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::BACK)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(true)
            .depth_compare_op(vk::CompareOp::LESS);
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let color_blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state =
            vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&inputs)
            .input_assembly_state(&assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterizer)
            .multisample_state(&multisampling)
            .depth_stencil_state(&depth)
            .color_blend_state(&color_blend)
            .dynamic_state(&dynamic_state)
            .layout(self.pipeline_layout)
            .render_pass(render_pass)
            .subpass(0);
        // SAFETY: the module/layout/render pass are live (the caller
        // owns the module; this state owns the layout; the pass is
        // the ensure's); the returned pipeline is owned by this
        // state.
        unsafe {
            ctx.device
                .create_graphics_pipelines(ctx.pipeline_cache, &[info], None)
        }
        .map_err(|(_, error)| RendererError::vk(error, what))
        .map(|mut pipelines| pipelines.remove(0))
    }

    /// Creates the HDR-pass skinned pipeline variant if absent (the
    /// lazy discipline one level deeper: the first populated POST
    /// frame pays the one-time pipeline; postless populated frames
    /// never do). A failure leaves the prior state fully intact.
    pub(super) fn ensure_hdr(
        &mut self,
        ctx: &VulkanContext,
        hdr_render_pass: vk::RenderPass,
    ) -> Result<(), RendererError> {
        if self.skinned_hdr.is_some() {
            return Ok(());
        }
        let pipeline = self.build_pipeline(ctx, hdr_render_pass, "skinned proxy hdr pipeline")?;
        self.skinned_hdr = Some(pipeline);
        Ok(())
    }

    /// Copies this frame's pose bytes into `slot`'s mapped region.
    ///
    /// # Safety
    /// The caller must own `slot` — its fence wait already passed
    /// (the same contract as the instance buffer's
    /// [`super::proxy_draw::ProxyResources::write_slot`], which the
    /// frame flow satisfies immediately before this call) — and
    /// `bytes` must be at most `slot_stride` long (the safe layer's
    /// validated bound: the repack's capacity refusal caps the
    /// blocks at MAX_POSE_BLOCKS).
    pub(super) unsafe fn write_slot(&self, slot: usize, bytes: &[u8]) {
        debug_assert!(bytes.len() <= self.slot_stride as usize);
        // SAFETY: the documented caller contract — this slot's fence
        // wait passed, so the previous submission through this slot
        // (whose vertex shaders read this region) has completed; the
        // length is within the region's bound.
        unsafe {
            let dest = self
                .mapped
                .byte_add(slot * self.slot_stride as usize)
                .cast::<u8>();
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), dest, bytes.len());
        }
    }

    /// The main-pass skinned pipeline handle.
    pub(super) fn pipeline(&self) -> vk::Pipeline {
        self.pipeline
    }

    /// The HDR-pass skinned pipeline handle, when ensured.
    pub(super) fn skinned_hdr(&self) -> Option<vk::Pipeline> {
        self.skinned_hdr
    }

    /// The skinned pipeline layout (the recording binds it with the
    /// skinned sets).
    pub(super) fn pipeline_layout(&self) -> vk::PipelineLayout {
        self.pipeline_layout
    }

    /// The per-material skinned set (the npc kind's material's set).
    pub(super) fn set(&self, material: u32) -> Option<vk::DescriptorSet> {
        self.sets.get(material as usize).copied()
    }

    /// The pose dynamic offset for `slot`'s region (the second
    /// dynamic offset in the skinned set binds).
    pub(super) fn pose_offset(&self, slot: usize) -> u32 {
        (slot as u64 * self.slot_stride) as u32
    }

    /// Releases every handle exactly once; valid for both the
    /// fully-built and the partial state (null handles denote
    /// partial construction).
    pub(super) fn release(&mut self, ctx: &VulkanContext) {
        // SAFETY: owned by this state; the owning PipelineState is
        // destroyed only while the device is idle (replacement waits;
        // teardown waits), after the pipelines reading these objects.
        unsafe {
            if let Some(hdr) = self.skinned_hdr.take() {
                ctx.device.destroy_pipeline(hdr, None);
            }
            if self.pipeline != vk::Pipeline::null() {
                ctx.device.destroy_pipeline(self.pipeline, None);
            }
            if self.pipeline_layout != vk::PipelineLayout::null() {
                ctx.device
                    .destroy_pipeline_layout(self.pipeline_layout, None);
            }
            if self.pool != vk::DescriptorPool::null() {
                ctx.device.destroy_descriptor_pool(self.pool, None);
            }
            if self.set_layout != vk::DescriptorSetLayout::null() {
                ctx.device
                    .destroy_descriptor_set_layout(self.set_layout, None);
            }
            if !self.mapped.is_null() {
                ctx.device.unmap_memory(self.memory);
                self.mapped = core::ptr::null_mut();
            }
            if self.buffer != vk::Buffer::null() {
                ctx.device.destroy_buffer(self.buffer, None);
            }
            if self.memory != vk::DeviceMemory::null() {
                ctx.device.free_memory(self.memory, None);
            }
            self.pipeline = vk::Pipeline::null();
            self.pipeline_layout = vk::PipelineLayout::null();
            self.pool = vk::DescriptorPool::null();
            self.set_layout = vk::DescriptorSetLayout::null();
            self.buffer = vk::Buffer::null();
            self.memory = vk::DeviceMemory::null();
        }
    }
}
