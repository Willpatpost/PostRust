//! The skinned-proxy GPU resources (Era-2 stage 2c — the animation +
//! skinning + proxy shadows package): the per-frame-slot mapped POSE
//! STORAGE buffer (the per-instance skin matrices the
//! `instanced_proxy_skinned` vertex shader reads through binding 5)
//! and the skinned lit pipeline (the NPC kind's path — the vehicle
//! kind stays on the plain proxy pipeline, byte-identical).
//!
//! ZERO-PER-FRAME-ALLOCATION DISCIPLINE (the standing gate): every
//! object here is created ONCE, lazily, outside command recording —
//! the pass-independent half ([`SkinnedRetained`]) at the FIRST
//! populated corner frame EVER, on the renderer's Vulkan context
//! ([`crate::vulkan::VulkanContext::ensure_skinned_retained`] — the
//! region-buffer precedent: the unpopulated path pays nothing, and
//! the unpopulated allocation pins are untouched); the pass-bound
//! pipelines ([`SkinnedProxyResources`]) once per pipeline state.
//! The per-frame pose bytes are the safe layer's repacked blocks
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
//! THE OWNERSHIP SPLIT (the Era-2 follow-up, PR #33's recorded
//! renderer-architecture repair): precisely BECAUSE everything above
//! is pass-agnostic and binds only context-owned corner resources
//! plus the family's own pose buffer, the retained half — the pose
//! storage, the set layout, the pool, the sets, and the pipeline
//! layout — lives on the renderer's [`VulkanContext`] at DEVICE
//! lifetime (the `pipeline_cache` precedent), created once at the
//! first populated frame and INHERITED by every pipeline generation:
//! a forced pipeline replacement (`--replace-pipelines`, the
//! surface-format-change lifecycle) rebuilds only the render-pass-
//! bound pipelines and never re-creates the pool/sets/pose-buffer
//! (the flat replacement tuple the lifecycle gates pin). The
//! shadowed-populated family is deliberately NOT transferred: its
//! sets bind the re-created shadow state's per-slot map views, so it
//! keeps the finding-A per-generation re-ensure.
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

/// The proxy vertex form a pipeline reads. The two kinds' mesh
/// bindings differ: the npc kind's 64-byte
/// [`crate::animation::SkinnedVertex`] (locations 0-2 plus the
/// skinning tail 8-9 and the instance params' pose index at 10), the
/// vehicle kind's 32-byte [`ProxyVertex`] (locations 0-2 and the
/// instance columns 3-7 — the 6-E plain proxy pipeline's own input).
/// A pipeline assembled for the wrong form reads the wrong stride
/// against the kind's VBO — the vehicle stream is 32-byte, so a
/// skinned-stride pipeline fetches past its end (the first 4b draft's
/// plain pipelines carried the skinned input; the vertex-form
/// parameter makes that mistake unexpressible).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ProxyVertexForm {
    /// The npc kind: [`crate::animation::SkinnedVertex`], locations
    /// 0-2 + 8-9 at binding 0, 3-7 + 10 at binding 1.
    Skinned,
    /// The vehicle kind: [`ProxyVertex`], locations 0-2 at binding 0,
    /// 3-7 at binding 1.
    Plain,
}

/// The device-lifetime, pass-independent skinned resources (the
/// Era-2 ownership transfer): the mapped pose storage, the skinned
/// set layout/pool/sets, and the skinned pipeline layout — every
/// object that binds no render pass. Owned by
/// [`super::VulkanContext`] (the `pipeline_cache` precedent:
/// created once at the first populated frame, INHERITED by every
/// pipeline generation, released only at context teardown — AFTER
/// the owning Renderer destroyed every PipelineState, so every
/// pipeline that read these objects is already gone). A pipeline
/// replacement therefore rebuilds only the pass-bound pipelines
/// ([`SkinnedProxyResources`]) and never re-creates this state.
pub(crate) struct SkinnedRetained {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: *mut core::ffi::c_void,
    slot_stride: u64,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    sets: [vk::DescriptorSet; MATERIAL_COUNT],
    /// The skinned pipeline layout (pass-agnostic — it wraps only
    /// the set layout above, so it transfers with the retained
    /// state; the pass-bound pipelines are built on it).
    pipeline_layout: vk::PipelineLayout,
}

impl SkinnedRetained {
    /// Builds the whole retained state against the corner's
    /// resources. Partial-construction safe: `release` frees
    /// exactly what exists (null handles denote partial
    /// construction — the corrective-finding-G discipline).
    pub(super) fn new(
        ctx: &VulkanContext,
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

        let mut partial = SkinnedRetained {
            buffer: vk::Buffer::null(),
            memory: vk::DeviceMemory::null(),
            mapped: core::ptr::null_mut(),
            slot_stride: SLOT_STRIDE,
            set_layout: vk::DescriptorSetLayout::null(),
            pool: vk::DescriptorPool::null(),
            sets: [vk::DescriptorSet::null(); MATERIAL_COUNT],
            pipeline_layout: vk::PipelineLayout::null(),
        };
        if let Err(error) = partial.build(ctx, corner) {
            partial.release(ctx);
            return Err(error);
        }
        Ok(partial)
    }

    fn build(
        &mut self,
        ctx: &VulkanContext,
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
            // The pose binding's range is the CONCRETE per-slot
            // region (SLOT_STRIDE), NEVER VK_WHOLE_SIZE: Vulkan
            // forbids a nonzero dynamic offset on a WHOLE_SIZE
            // descriptor (VUID-vkCmdBindDescriptorSets-
            // pDescriptorSets-06715) — slot 0's zero offset passes
            // validation while slot 1's violates it, exactly the
            // half-green trap the first CI run of the gates caught
            // (the violation sat silently through the earlier
            // stages because those gates never asserted
            // validation_errors). With the concrete range the
            // effective view is [pose_offset, pose_offset +
            // SLOT_STRIDE) — exactly this slot's region, and tighter
            // than WHOLE_SIZE (the shader cannot index past its
            // slot's bound).
            let poses = [vk::DescriptorBufferInfo::default()
                .buffer(self.buffer)
                .range(SLOT_STRIDE)];
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

        // The skinned pipeline layout (pass-agnostic — it wraps only
        // the set layout above, so the retained state owns it; every
        // per-generation pipeline — main-pass and HDR — is built on
        // it, with the SKINNED vertex input and this set family).
        // SAFETY: the layout is owned by this state immediately; the
        // create-info slices live through the call.
        self.pipeline_layout = unsafe {
            ctx.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&[self.set_layout]),
                None,
            )
        }
        .map_err(|e| RendererError::vk(e, "skinned proxy layout"))?;
        Ok(())
    }

    /// The skinned lit pipeline against `render_pass` — the plain
    /// proxy pipeline's states with the SKINNED vertex input (the
    /// one pipeline builder the per-generation family's main-pass
    /// and HDR variants call, on THIS retained state's layout; the
    /// states exist exactly once).
    pub(super) fn build_pipeline(
        &self,
        ctx: &VulkanContext,
        render_pass: vk::RenderPass,
        what: &str,
    ) -> Result<vk::Pipeline, RendererError> {
        let module = create_shader_module(
            ctx,
            include_bytes!(concat!(env!("OUT_DIR"), "/instanced_proxy_skinned.spv")),
        )?;
        let result = self.pipeline_from_module(
            ctx,
            module,
            self.pipeline_layout,
            render_pass,
            ProxyVertexForm::Skinned,
            what,
        );
        // SAFETY: consumed by the pipeline above (or the build failed
        // with none alive); destroyed exactly once.
        unsafe { ctx.device.destroy_shader_module(module, None) };
        result
    }

    /// The pipeline assembly proper (the module is the caller's to
    /// destroy — this shape lets a failed build release exactly the
    /// module it created, the assemble_pipelines discipline). The
    /// pipeline is created on `layout` — the CALLER's set layout for
    /// the family this pipeline belongs to (the unshadowed skinned
    /// path passes its own; the shadowed-populated builders pass
    /// their per-family layouts; the plain vehicle depth cast passes
    /// the shadow state's own 6-F depth layout). `form` selects the
    /// vertex input — see [`ProxyVertexForm`]. The first device
    /// execution (CI run 45485839516) is on record for why both are
    /// parameters and never defaults: a pipeline silently built on
    /// the wrong layout or the wrong vertex form passes every
    /// CPU-side gate and fails only where the validation layer can
    /// see it.
    fn pipeline_from_module(
        &self,
        ctx: &VulkanContext,
        module: vk::ShaderModule,
        layout: vk::PipelineLayout,
        render_pass: vk::RenderPass,
        form: ProxyVertexForm,
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
        // Binding 0: the proxy mesh at VERTEX rate — the form's own
        // stride (the 64-byte SkinnedVertex for the npc kind, the
        // 32-byte ProxyVertex for the vehicle kind). Binding 1: the
        // packed instance bytes at INSTANCE rate (stride
        // INSTANCE_STRIDE; the skinned form's location 10 reads the
        // params vec4's pose index at byte offset 80, the plain form
        // stops at the tint's 64).
        let mesh_binding = vk::VertexInputBindingDescription {
            binding: 0,
            stride: match form {
                ProxyVertexForm::Skinned => std::mem::size_of::<crate::animation::SkinnedVertex>(),
                ProxyVertexForm::Plain => std::mem::size_of::<ProxyVertex>(),
            } as u32,
            input_rate: vk::VertexInputRate::VERTEX,
        };
        let instance_binding = vk::VertexInputBindingDescription {
            binding: 1,
            stride: INSTANCE_STRIDE as u32,
            input_rate: vk::VertexInputRate::INSTANCE,
        };
        let bindings = [mesh_binding, instance_binding];
        let vec4 = vk::Format::R32G32B32A32_SFLOAT;
        // The shared prefix: the mesh's pos/normal/uv at locations
        // 0-2 and the instance columns at 3-7 — every form's input.
        // The skinned form extends it with the skinning tail
        // (joints R32G32B32A32_UINT at byte 32, weights at 48 — the
        // SkinnedVertex bytes past the ProxyVertex prefix) and the
        // params vec4's pose index at location 10 (byte 80 of the
        // instance stride); the plain form's slice STOPS at the
        // prefix — its 32-byte mesh stride cannot carry the tail
        // (offsets 32/48 would read the next vertex, past the buffer
        // at the last one).
        let all_attributes = [
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
            vk::VertexInputAttributeDescription {
                location: 10,
                binding: 1,
                format: vec4,
                offset: 80,
            },
        ];
        let attributes = &all_attributes[..match form {
            ProxyVertexForm::Skinned => 11,
            ProxyVertexForm::Plain => 8,
        }];
        let inputs = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(attributes);
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
            .layout(layout)
            .render_pass(render_pass)
            .subpass(0);
        // SAFETY: the module/layout/render pass are live (the caller
        // owns the module; the layout belongs to the family's state
        // and outlives this pipeline; the pass is the ensure's); the
        // returned pipeline is owned by the caller's state.
        unsafe {
            ctx.device
                .create_graphics_pipelines(ctx.pipeline_cache, &[info], None)
        }
        .map_err(|(_, error)| RendererError::vk(error, what))
        .map(|mut pipelines| pipelines.remove(0))
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

    /// The skinned pipeline layout (the recording binds it with the
    /// skinned sets).
    pub(super) fn pipeline_layout(&self) -> vk::PipelineLayout {
        self.pipeline_layout
    }

    /// The per-material skinned set (the npc kind's material's set).
    pub(super) fn set(&self, material: u32) -> Option<vk::DescriptorSet> {
        self.sets.get(material as usize).copied()
    }

    /// The per-slot region size (bytes).
    #[allow(dead_code)]
    pub(super) fn slot_bytes(&self) -> u64 {
        self.slot_stride
    }

    /// The pose storage buffer handle (the shadowed-populated
    /// state's set writes bind it at binding 5).
    pub(super) fn pose_buffer(&self) -> vk::Buffer {
        self.buffer
    }

    /// The pose dynamic offset for `slot`'s region (the second
    /// dynamic offset in the skinned set binds).
    pub(super) fn pose_offset(&self, slot: usize) -> u32 {
        (slot as u64 * self.slot_stride) as u32
    }

    /// Builds the skinned depth pipeline (the NPC cast through the
    /// shadow state's depth pass) on `layout` — the skinned depth
    /// family's OWN layout (its set carries the light matrix at
    /// binding 0 and the pose region at binding 5; a pipeline built
    /// on any other layout mismatches the shader's declarations at
    /// the first device draw).
    pub(super) fn build_skinned_depth_pipeline(
        &self,
        ctx: &VulkanContext,
        spv: &[u8],
        depth_render_pass: vk::RenderPass,
        layout: vk::PipelineLayout,
        what: &str,
    ) -> Result<vk::Pipeline, RendererError> {
        let module = create_shader_module(ctx, spv)?;
        let result = self.pipeline_from_module(
            ctx,
            module,
            layout,
            depth_render_pass,
            ProxyVertexForm::Skinned,
            what,
        );
        unsafe { ctx.device.destroy_shader_module(module, None) };
        result
    }

    /// Builds the plain depth pipeline (the vehicle cast through the
    /// shadow state's depth pass) on `layout` — the SHADOW state's
    /// own 6-F depth layout (its set carries the light matrix; the
    /// recording binds that very set) — with the PLAIN vertex form
    /// (the vehicle mesh's 32-byte stream).
    pub(super) fn build_plain_depth_pipeline(
        &self,
        ctx: &VulkanContext,
        spv: &[u8],
        depth_render_pass: vk::RenderPass,
        layout: vk::PipelineLayout,
        what: &str,
    ) -> Result<vk::Pipeline, RendererError> {
        let module = create_shader_module(ctx, spv)?;
        let result = self.pipeline_from_module(
            ctx,
            module,
            layout,
            depth_render_pass,
            ProxyVertexForm::Plain,
            what,
        );
        unsafe { ctx.device.destroy_shader_module(module, None) };
        result
    }

    /// Builds the skinned shadowed lit pipeline (the NPC receiving
    /// shadows through the main pass) on `layout` — the skinned
    /// shadowed family's own layout (bindings 0/2/3/4/5: the albedo,
    /// the extended shadow FrameParams, the material tail, the slot's
    /// map view, the pose region).
    pub(super) fn build_skinned_lit_pipeline(
        &self,
        ctx: &VulkanContext,
        spv: &[u8],
        render_pass: vk::RenderPass,
        layout: vk::PipelineLayout,
        what: &str,
    ) -> Result<vk::Pipeline, RendererError> {
        let module = create_shader_module(ctx, spv)?;
        let result = self.pipeline_from_module(
            ctx,
            module,
            layout,
            render_pass,
            ProxyVertexForm::Skinned,
            what,
        );
        unsafe { ctx.device.destroy_shader_module(module, None) };
        result
    }

    /// Builds the plain shadowed lit pipeline (the vehicle receiving
    /// shadows through the main pass) on `layout` — the plain
    /// shadowed family's own layout (bindings 0/2/3/4, no pose) —
    /// with the PLAIN vertex form.
    pub(super) fn build_plain_lit_pipeline(
        &self,
        ctx: &VulkanContext,
        spv: &[u8],
        render_pass: vk::RenderPass,
        layout: vk::PipelineLayout,
        what: &str,
    ) -> Result<vk::Pipeline, RendererError> {
        let module = create_shader_module(ctx, spv)?;
        let result = self.pipeline_from_module(
            ctx,
            module,
            layout,
            render_pass,
            ProxyVertexForm::Plain,
            what,
        );
        unsafe { ctx.device.destroy_shader_module(module, None) };
        result
    }

    /// Releases every handle exactly once; valid for both the
    /// fully-built and the partial state (null handles denote
    /// partial construction).
    pub(super) fn release(&mut self, ctx: &VulkanContext) {
        // SAFETY: owned by the VulkanContext, which releases this
        // state only at its own teardown — AFTER the owning Renderer
        // destroyed every PipelineState (Renderer::drop: frames →
        // swapchain → pipes.destroy → ctx.destroy; the replacement
        // path destroys an outgoing state only after a device wait)
        // — so every pipeline that read these objects is already
        // destroyed, and no in-flight submission references them.
        unsafe {
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
            self.pipeline_layout = vk::PipelineLayout::null();
            self.pool = vk::DescriptorPool::null();
            self.set_layout = vk::DescriptorSetLayout::null();
            self.buffer = vk::Buffer::null();
            self.memory = vk::DeviceMemory::null();
        }
    }
}

/// The per-render-pass skinned pipelines (the Era-2 ownership
/// transfer's other half): the main-pass skinned lit pipeline and
/// its lazily-created HDR variant — every object here IS
/// render-pass-bound. Owned by [`super::PipelineState`] (dies with
/// the state's pipelines; a replacement rebuilds both against the
/// new render pass BEFORE installation — the finding-A discipline),
/// built on the context's RETAINED layout
/// ([`SkinnedRetained::pipeline_layout`]) and recording through the
/// retained sets and pose storage, which the replacement INHERITS
/// (never re-creates — the flat replacement tuple).
pub(crate) struct SkinnedProxyResources {
    /// The main-pass skinned lit pipeline (vs_main/fs_main of
    /// instanced_proxy_skinned.spv).
    pipeline: vk::Pipeline,
    /// The HDR-pass variant (created lazily at the first populated
    /// POST frame — the ensure_shadowed_hdr precedent; built on the
    /// same retained layout, which is pass-agnostic).
    skinned_hdr: Option<vk::Pipeline>,
}

impl SkinnedProxyResources {
    /// Builds the per-generation pipelines on the RETAINED state's
    /// layout against this state's render pass. A failed build owns
    /// nothing (the builder releases its own shader module on both
    /// paths), so no partial-state dance is needed.
    pub(super) fn new(
        ctx: &VulkanContext,
        retained: &SkinnedRetained,
        main_render_pass: vk::RenderPass,
    ) -> Result<Self, RendererError> {
        // The skinned lit pipeline: the SAME rasterizer (the proxy
        // meshes wind CCW about their outward normals), depth
        // contract, and blend as the plain proxy pipeline — the
        // vertex input and the LAYOUT differ (the SkinnedVertex
        // format with joints/weights at locations 8/9, the instance
        // params vec4's pose index at location 10; the skinned set
        // layout with the pose storage).
        Ok(SkinnedProxyResources {
            pipeline: retained.build_pipeline(ctx, main_render_pass, "skinned proxy pipeline")?,
            skinned_hdr: None,
        })
    }

    /// Creates the HDR-pass skinned pipeline variant if absent (the
    /// lazy discipline one level deeper: the first populated POST
    /// frame pays the one-time pipeline; postless populated frames
    /// never do). A failure leaves the prior state fully intact.
    pub(super) fn ensure_hdr(
        &mut self,
        ctx: &VulkanContext,
        retained: &SkinnedRetained,
        hdr_render_pass: vk::RenderPass,
    ) -> Result<(), RendererError> {
        if self.skinned_hdr.is_some() {
            return Ok(());
        }
        let pipeline =
            retained.build_pipeline(ctx, hdr_render_pass, "skinned proxy hdr pipeline")?;
        self.skinned_hdr = Some(pipeline);
        Ok(())
    }

    /// The main-pass skinned pipeline handle.
    pub(super) fn pipeline(&self) -> vk::Pipeline {
        self.pipeline
    }

    /// The HDR-pass skinned pipeline handle, when ensured.
    pub(super) fn skinned_hdr(&self) -> Option<vk::Pipeline> {
        self.skinned_hdr
    }

    /// Releases every handle exactly once; valid for both the
    /// fully-built and the partial state (null/absent handles denote
    /// partial construction).
    pub(super) fn release(&mut self, ctx: &VulkanContext) {
        // SAFETY: owned by this state; the owning PipelineState is
        // destroyed only while the device is idle (replacement waits;
        // teardown waits). The RETAINED layout/sets/pose-buffer these
        // pipelines read are the context's and outlive this state.
        unsafe {
            if let Some(hdr) = self.skinned_hdr.take() {
                ctx.device.destroy_pipeline(hdr, None);
            }
            if self.pipeline != vk::Pipeline::null() {
                ctx.device.destroy_pipeline(self.pipeline, None);
            }
            self.pipeline = vk::Pipeline::null();
        }
    }
}

/// The shadowed-populated proxy state (Era-2 stage 4b — the NPC and vehicle
/// casting/receiving paths through the shadow family), created LAZILY at the
/// first SHADOWED POPULATED corner frame — one level deeper than the skinned
/// state (the indirect-command precedent).
pub(crate) struct ShadowedProxyResources {
    /// The npc cast pipeline (the shadow state's depth pass).
    skinned_depth_pipeline: vk::Pipeline,
    skinned_depth_layout: vk::PipelineLayout,
    skinned_depth_set: vk::DescriptorSet,
    skinned_depth_pool: vk::DescriptorPool,
    skinned_depth_set_layout: vk::DescriptorSetLayout,
    /// The vehicle cast pipeline (reuses the shadow state's own
    /// depth set at record time — no descriptors here).
    plain_depth_pipeline: vk::Pipeline,
    /// The npc receiving pipeline (the main render pass).
    skinned_shadowed_pipeline: vk::Pipeline,
    skinned_shadowed_layout: vk::PipelineLayout,
    skinned_shadowed_sets: [vk::DescriptorSet; MATERIAL_COUNT * 2],
    skinned_shadowed_pool: vk::DescriptorPool,
    skinned_shadowed_set_layout: vk::DescriptorSetLayout,
    /// The vehicle receiving pipeline (the main render pass).
    plain_shadowed_pipeline: vk::Pipeline,
    plain_shadowed_layout: vk::PipelineLayout,
    plain_shadowed_sets: [vk::DescriptorSet; MATERIAL_COUNT * 2],
    plain_shadowed_pool: vk::DescriptorPool,
    plain_shadowed_set_layout: vk::DescriptorSetLayout,
    /// The npc receiving pipeline's HDR variant (the post chain's HDR
    /// scene pass — the same shader module, layout, and sets as the
    /// main-pass pipeline, built against the post state's HDR render
    /// pass; lazily created at the first populated SHADOWED POST frame,
    /// the `SkinnedProxyResources::ensure_hdr` precedent).
    skinned_shadowed_hdr: Option<vk::Pipeline>,
    /// The vehicle receiving pipeline's HDR variant (the same shape).
    plain_shadowed_hdr: Option<vk::Pipeline>,
}

impl ShadowedProxyResources {
    /// Builds the shadowed-populated proxy state against the shadow
    /// state's depth pass, its extended uniform, its per-slot map
    /// views, and the context's RETAINED skinned pose buffer.
    /// Partial-construction safe (the release discipline).
    pub(super) fn new(
        ctx: &VulkanContext,
        shadows: &super::shadow_draw::ShadowResources,
        retained: &SkinnedRetained,
        corner: &CornerResources,
        main_render_pass: vk::RenderPass,
    ) -> Result<Self, RendererError> {
        let _slots = FRAMES_IN_FLIGHT as usize;
        let mut partial = ShadowedProxyResources {
            skinned_depth_pipeline: vk::Pipeline::null(),
            skinned_depth_layout: vk::PipelineLayout::null(),
            skinned_depth_set: vk::DescriptorSet::null(),
            skinned_depth_pool: vk::DescriptorPool::null(),
            skinned_depth_set_layout: vk::DescriptorSetLayout::null(),
            plain_depth_pipeline: vk::Pipeline::null(),
            skinned_shadowed_pipeline: vk::Pipeline::null(),
            skinned_shadowed_layout: vk::PipelineLayout::null(),
            skinned_shadowed_sets: [vk::DescriptorSet::null(); MATERIAL_COUNT * 2],
            skinned_shadowed_pool: vk::DescriptorPool::null(),
            skinned_shadowed_set_layout: vk::DescriptorSetLayout::null(),
            plain_shadowed_pipeline: vk::Pipeline::null(),
            plain_shadowed_layout: vk::PipelineLayout::null(),
            plain_shadowed_sets: [vk::DescriptorSet::null(); MATERIAL_COUNT * 2],
            plain_shadowed_pool: vk::DescriptorPool::null(),
            plain_shadowed_set_layout: vk::DescriptorSetLayout::null(),
            skinned_shadowed_hdr: None,
            plain_shadowed_hdr: None,
        };
        if let Err(error) = partial.build(ctx, shadows, retained, corner, main_render_pass) {
            partial.release(ctx);
            return Err(error);
        }
        Ok(partial)
    }

    fn build(
        &mut self,
        ctx: &VulkanContext,
        shadows: &super::shadow_draw::ShadowResources,
        retained: &SkinnedRetained,
        corner: &CornerResources,
        main_render_pass: vk::RenderPass,
    ) -> Result<(), RendererError> {
        let frag = vk::ShaderStageFlags::FRAGMENT;
        let vert = vk::ShaderStageFlags::VERTEX;
        let both = vert | frag;
        let combined = vk::DescriptorType::COMBINED_IMAGE_SAMPLER;
        let uniform_dynamic = vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC;
        let uniform = vk::DescriptorType::UNIFORM_BUFFER;
        let sampled = vk::DescriptorType::SAMPLED_IMAGE;
        let storage_dynamic = vk::DescriptorType::STORAGE_BUFFER_DYNAMIC;

        // --- The skinned DEPTH set (the npc cast): binding 0 = the
        // light matrix (the shadow uniform's light-matrix region at
        // the static light_offset, the DYNAMIC offset selecting the
        // slot — the corner depth set's own scheme), binding 5 = the
        // pose buffer (the per-slot region range — NEVER
        // VK_WHOLE_SIZE with a dynamic offset, VUID-06715). The
        // shader's OWN declarations (shadow_depth_skinned.wgsl) are
        // the layout's source of truth.
        let binding = |index: u32, ty: vk::DescriptorType, stages: vk::ShaderStageFlags| {
            vk::DescriptorSetLayoutBinding::default()
                .binding(index)
                .descriptor_type(ty)
                .descriptor_count(1)
                .stage_flags(stages)
        };
        let depth_bindings = [
            binding(0, uniform_dynamic, vert),
            binding(5, storage_dynamic, vert),
        ];
        let depth_sizes = [uniform_dynamic, storage_dynamic].map(|ty| vk::DescriptorPoolSize {
            ty,
            descriptor_count: 1,
        });
        let (pool, layout, [set]) =
            super::descriptors::create_sets::<1>(ctx, &depth_bindings, &depth_sizes, 1)?;
        let light = [vk::DescriptorBufferInfo::default()
            .buffer(shadows.frame_buffer())
            .offset(u64::from(shadows.light_offset()))
            .range(64)];
        let poses = [vk::DescriptorBufferInfo::default()
            .buffer(retained.pose_buffer())
            .range(SLOT_STRIDE)];
        // SAFETY: a fresh set; the shadow uniform and pose buffer
        // outlive this pool (their states own them).
        unsafe {
            ctx.device.update_descriptor_sets(
                &[
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(0)
                        .descriptor_type(uniform_dynamic)
                        .buffer_info(&light),
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(5)
                        .descriptor_type(storage_dynamic)
                        .buffer_info(&poses),
                ],
                &[],
            );
        }
        self.skinned_depth_pool = pool;
        self.skinned_depth_set_layout = layout;
        self.skinned_depth_set = set;
        self.skinned_depth_layout = unsafe {
            ctx.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&[layout]),
                None,
            )
        }
        .map_err(|e| RendererError::vk(e, "skinned shadow depth layout"))?;
        self.skinned_depth_pipeline = retained.build_skinned_depth_pipeline(
            ctx,
            include_bytes!(concat!(env!("OUT_DIR"), "/shadow_depth_skinned.spv")),
            shadows.depth_render_pass(),
            self.skinned_depth_layout,
            "skinned shadow depth pipeline",
        )?;
        // The vehicle cast: the PLAIN vertex input against the shadow
        // state's depth pass, laid out for the shadow state's OWN
        // depth set/layout (the recording binds them).
        self.plain_depth_pipeline = retained.build_plain_depth_pipeline(
            ctx,
            include_bytes!(concat!(env!("OUT_DIR"), "/shadow_depth_instanced.spv")),
            shadows.depth_render_pass(),
            shadows.depth_layout(),
            "plain shadow depth pipeline",
        )?;

        // --- The SHADOWED skinned lit sets (the npc kind receiving,
        // per material × slot — the map view differs per slot):
        // binding 0 = the corner albedo view + immutable sampler,
        // binding 2 = the shadow uniform's EXTENDED FrameParams
        // (the 192-byte block, dynamic — the shadow stride's slot
        // offset at bind time), binding 3 = the corner material
        // tail, binding 4 = this slot's shadow map view, binding 5 =
        // the pose buffer's per-slot region. The shaders' OWN
        // declarations (instanced_proxy_skinned_shadowed.wgsl) are
        // the layout's source of truth.
        let samplers = [corner.nearest_sampler()];
        let skinned_bindings = [
            binding(0, combined, frag).immutable_samplers(&samplers),
            binding(2, uniform_dynamic, both),
            binding(3, uniform, frag),
            binding(4, sampled, frag),
            binding(5, storage_dynamic, vert),
        ];
        let set_count = (MATERIAL_COUNT * 2) as u32;
        let slots = FRAMES_IN_FLIGHT as usize;
        let skinned_sizes =
            [combined, uniform_dynamic, uniform, sampled, storage_dynamic].map(|ty| {
                vk::DescriptorPoolSize {
                    ty,
                    descriptor_count: set_count,
                }
            });
        let (skinned_pool, skinned_layout, skinned_sets) =
            super::descriptors::create_sets::<{ MATERIAL_COUNT * 2 }>(
                ctx,
                &skinned_bindings,
                &skinned_sizes,
                set_count,
            )?;
        for material in 0..MATERIAL_COUNT {
            let view = corner.texture_view(material);
            let material_info = [vk::DescriptorBufferInfo::default()
                .buffer(corner.uniform_buffer())
                .offset(corner.material_offset(material))
                .range(std::mem::size_of::<[f32; 4]>() as u64)];
            for slot in 0..slots {
                let set = skinned_sets[material * slots + slot];
                let image = [vk::DescriptorImageInfo::default()
                    .image_view(view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                let map = [vk::DescriptorImageInfo::default()
                    .image_view(shadows.map_view(slot))
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                let frame = [vk::DescriptorBufferInfo::default()
                    .buffer(shadows.frame_buffer())
                    .range(super::shadow_draw::SHADOW_FRAME_PARAMS_SIZE)];
                let poses = [vk::DescriptorBufferInfo::default()
                    .buffer(retained.pose_buffer())
                    .range(SLOT_STRIDE)];
                let write = vk::WriteDescriptorSet::default().dst_set(set);
                // SAFETY: fresh sets; every resource outlives the
                // pool (the states own them).
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
                                .dst_binding(4)
                                .descriptor_type(sampled)
                                .image_info(&map),
                            write
                                .dst_binding(5)
                                .descriptor_type(storage_dynamic)
                                .buffer_info(&poses),
                        ],
                        &[],
                    );
                }
            }
        }
        self.skinned_shadowed_pool = skinned_pool;
        self.skinned_shadowed_set_layout = skinned_layout;
        self.skinned_shadowed_sets = skinned_sets;
        self.skinned_shadowed_layout = unsafe {
            ctx.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&[skinned_layout]),
                None,
            )
        }
        .map_err(|e| RendererError::vk(e, "skinned shadowed layout"))?;
        self.skinned_shadowed_pipeline = retained.build_skinned_lit_pipeline(
            ctx,
            include_bytes!(concat!(
                env!("OUT_DIR"),
                "/instanced_proxy_skinned_shadowed.spv"
            )),
            main_render_pass,
            self.skinned_shadowed_layout,
            "skinned shadowed proxy pipeline",
        )?;

        // --- The SHADOWED plain lit sets (the vehicle kind
        // receiving): the same shape without the pose binding — the
        // PLAIN vehicle shader (instanced_proxy_shadowed.spv,
        // locations 0-7) is the pipeline's own input.
        let plain_bindings = [
            binding(0, combined, frag).immutable_samplers(&samplers),
            binding(2, uniform_dynamic, both),
            binding(3, uniform, frag),
            binding(4, sampled, frag),
        ];
        let plain_sizes =
            [combined, uniform_dynamic, uniform, sampled].map(|ty| vk::DescriptorPoolSize {
                ty,
                descriptor_count: set_count,
            });
        let (plain_pool, plain_layout, plain_sets) =
            super::descriptors::create_sets::<{ MATERIAL_COUNT * 2 }>(
                ctx,
                &plain_bindings,
                &plain_sizes,
                set_count,
            )?;
        for material in 0..MATERIAL_COUNT {
            let view = corner.texture_view(material);
            let material_info = [vk::DescriptorBufferInfo::default()
                .buffer(corner.uniform_buffer())
                .offset(corner.material_offset(material))
                .range(std::mem::size_of::<[f32; 4]>() as u64)];
            for slot in 0..slots {
                let set = plain_sets[material * slots + slot];
                let image = [vk::DescriptorImageInfo::default()
                    .image_view(view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                let map = [vk::DescriptorImageInfo::default()
                    .image_view(shadows.map_view(slot))
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                let frame = [vk::DescriptorBufferInfo::default()
                    .buffer(shadows.frame_buffer())
                    .range(super::shadow_draw::SHADOW_FRAME_PARAMS_SIZE)];
                let write = vk::WriteDescriptorSet::default().dst_set(set);
                // SAFETY: fresh sets; every resource outlives the
                // pool.
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
                                .dst_binding(4)
                                .descriptor_type(sampled)
                                .image_info(&map),
                        ],
                        &[],
                    );
                }
            }
        }
        self.plain_shadowed_pool = plain_pool;
        self.plain_shadowed_set_layout = plain_layout;
        self.plain_shadowed_sets = plain_sets;
        self.plain_shadowed_layout = unsafe {
            ctx.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&[plain_layout]),
                None,
            )
        }
        .map_err(|e| RendererError::vk(e, "plain shadowed proxy layout"))?;
        // The VEHICLE kind's shader — instanced_proxy_shadowed.spv
        // (the PLAIN vertex input), never the skinned variant: the
        // skinned module's locations 8-10 do not exist in the
        // 32-byte ProxyVertex stream (the first 4b draft built this
        // pipeline from the skinned shader — caught in this repair).
        self.plain_shadowed_pipeline = retained.build_plain_lit_pipeline(
            ctx,
            include_bytes!(concat!(env!("OUT_DIR"), "/instanced_proxy_shadowed.spv")),
            main_render_pass,
            self.plain_shadowed_layout,
            "plain shadowed proxy pipeline",
        )?;

        Ok(())
    }

    /// The npc kind's skinned shadowed pipeline layout and set for
    /// `material` and `slot`, plus the dynamic offsets ([frame,
    /// pose]). The caller must validate `material < MATERIAL_COUNT`.
    /// Note: the pose offset comes from the external `skinned` state
    /// passed to `record_depth_casts`, not stored in this structure.
    pub(super) fn skinned_shadowed_bind(
        &self,
        material: u32,
        slot: usize,
        shadows_frame_stride: u32,
        skinned_pose_offset: u32,
    ) -> (vk::PipelineLayout, vk::DescriptorSet, [u32; 2]) {
        let set = self.skinned_shadowed_sets[material as usize * 2 + slot];
        (
            self.skinned_shadowed_layout,
            set,
            [slot as u32 * shadows_frame_stride, skinned_pose_offset],
        )
    }

    /// The vehicle kind's plain shadowed pipeline layout and set for
    /// `material` and `slot`, plus the dynamic offset ([frame]).
    pub(super) fn plain_shadowed_bind(
        &self,
        material: u32,
        slot: usize,
        shadows_frame_stride: u32,
    ) -> (vk::PipelineLayout, vk::DescriptorSet, u32) {
        let set = self.plain_shadowed_sets[material as usize * 2 + slot];
        (
            self.plain_shadowed_layout,
            set,
            slot as u32 * shadows_frame_stride,
        )
    }

    /// The main-pass skinned shadowed pipeline handle.
    pub(super) fn skinned_shadowed_pipeline(&self) -> vk::Pipeline {
        self.skinned_shadowed_pipeline
    }

    /// The main-pass plain shadowed pipeline handle.
    pub(super) fn plain_shadowed_pipeline(&self) -> vk::Pipeline {
        self.plain_shadowed_pipeline
    }

    /// Creates the HDR-pass shadowed proxy pipeline variants if absent
    /// (the lazy discipline one level deeper: the first populated
    /// SHADOWED POST frame pays the two one-time pipelines; postless
    /// and unshadowed-populated frames never do). The SAME shader
    /// modules, pipeline layouts, and descriptor sets serve both passes
    /// — only the render pass (and its color attachment format) differs,
    /// so nothing else is created. A failure between the two builds
    /// leaves the built one owned by this state (released at teardown
    /// exactly once — the partial-construction discipline); the method
    /// is idempotent and re-ensures only what is missing.
    pub(super) fn ensure_hdr(
        &mut self,
        ctx: &VulkanContext,
        retained: &SkinnedRetained,
        hdr_render_pass: vk::RenderPass,
    ) -> Result<(), RendererError> {
        if self.skinned_shadowed_hdr.is_none() {
            self.skinned_shadowed_hdr = Some(retained.build_skinned_lit_pipeline(
                ctx,
                include_bytes!(concat!(
                    env!("OUT_DIR"),
                    "/instanced_proxy_skinned_shadowed.spv"
                )),
                hdr_render_pass,
                self.skinned_shadowed_layout,
                "skinned shadowed proxy hdr pipeline",
            )?);
        }
        if self.plain_shadowed_hdr.is_none() {
            self.plain_shadowed_hdr = Some(retained.build_plain_lit_pipeline(
                ctx,
                include_bytes!(concat!(env!("OUT_DIR"), "/instanced_proxy_shadowed.spv")),
                hdr_render_pass,
                self.plain_shadowed_layout,
                "plain shadowed proxy hdr pipeline",
            )?);
        }
        Ok(())
    }

    /// The npc kind's HDR-pass shadowed pipeline handle, when ensured.
    pub(super) fn skinned_shadowed_hdr(&self) -> Option<vk::Pipeline> {
        self.skinned_shadowed_hdr
    }

    /// The vehicle kind's HDR-pass shadowed pipeline handle, when ensured.
    pub(super) fn plain_shadowed_hdr(&self) -> Option<vk::Pipeline> {
        self.plain_shadowed_hdr
    }

    /// Releases every handle exactly once; valid for both the
    /// fully-built and the partial state (null handles denote
    /// partial construction).
    pub(super) fn release(&mut self, ctx: &VulkanContext) {
        // SAFETY: owned by this state; the owning PipelineState is
        // destroyed only while the device is idle (replacement waits;
        // teardown waits), after the pipelines reading the
        // shadow state's depth pass and the skinned state's pose
        // buffer.
        unsafe {
            for pipeline in [
                self.skinned_depth_pipeline,
                self.plain_depth_pipeline,
                self.skinned_shadowed_pipeline,
                self.plain_shadowed_pipeline,
                self.skinned_shadowed_hdr.unwrap_or(vk::Pipeline::null()),
                self.plain_shadowed_hdr.unwrap_or(vk::Pipeline::null()),
            ] {
                if pipeline != vk::Pipeline::null() {
                    ctx.device.destroy_pipeline(pipeline, None);
                }
            }
            for layout in [
                self.skinned_depth_layout,
                self.skinned_shadowed_layout,
                self.plain_shadowed_layout,
            ] {
                if layout != vk::PipelineLayout::null() {
                    ctx.device.destroy_pipeline_layout(layout, None);
                }
            }
            for pool in [
                self.skinned_depth_pool,
                self.skinned_shadowed_pool,
                self.plain_shadowed_pool,
            ] {
                if pool != vk::DescriptorPool::null() {
                    ctx.device.destroy_descriptor_pool(pool, None);
                }
            }
            for layout in [
                self.skinned_depth_set_layout,
                self.skinned_shadowed_set_layout,
                self.plain_shadowed_set_layout,
            ] {
                if layout != vk::DescriptorSetLayout::null() {
                    ctx.device.destroy_descriptor_set_layout(layout, None);
                }
            }
            self.skinned_depth_pipeline = vk::Pipeline::null();
            self.plain_depth_pipeline = vk::Pipeline::null();
            self.skinned_shadowed_pipeline = vk::Pipeline::null();
            self.plain_shadowed_pipeline = vk::Pipeline::null();
            self.skinned_shadowed_hdr = None;
            self.plain_shadowed_hdr = None;
            self.skinned_depth_layout = vk::PipelineLayout::null();
            self.skinned_shadowed_layout = vk::PipelineLayout::null();
            self.plain_shadowed_layout = vk::PipelineLayout::null();
            self.skinned_depth_pool = vk::DescriptorPool::null();
            self.skinned_shadowed_pool = vk::DescriptorPool::null();
            self.plain_shadowed_pool = vk::DescriptorPool::null();
            self.skinned_depth_set_layout = vk::DescriptorSetLayout::null();
            self.skinned_shadowed_set_layout = vk::DescriptorSetLayout::null();
            self.plain_shadowed_set_layout = vk::DescriptorSetLayout::null();
        }
    }

    /// Records the proxy casts into this slot's shadow map, INSIDE
    /// the depth pass's render pass (the caller has begun it and
    /// drawn the corner mesh; this binds each populated kind's depth
    /// pipeline and draws the same instance ranges the lit pass will
    /// draw). Returns the recorded draw-call count.
    ///
    /// # Safety
    /// `cmd` is recording INSIDE the depth pass; the slot's instance
    /// and pose regions hold this frame's bytes (written after the
    /// fence wait, before the pass began).
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn record_depth_casts(
        &self,
        ctx: &VulkanContext,
        cmd: vk::CommandBuffer,
        _corner: &CornerResources,
        shadows: &super::shadow_draw::ShadowResources,
        retained: &SkinnedRetained,
        slot: usize,
        upload: &super::proxy_draw::ProxyFrameUpload<'_>,
        proxy: &super::proxy_draw::ProxyResources,
    ) -> u32 {
        let frame_offset = slot as u32 * shadows.frame_stride();
        let pose_offset = retained.pose_offset(slot);
        let mut calls = 0;

        // NPC kind (the skinned depth pipeline)
        if upload.npc_count > 0 {
            unsafe {
                ctx.device.cmd_bind_pipeline(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.skinned_depth_pipeline,
                );
                ctx.device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.skinned_depth_layout,
                    0,
                    &[self.skinned_depth_set],
                    &[frame_offset, pose_offset],
                );
            }
            proxy.bind_kind_buffers(ctx, cmd, super::proxy_draw::ProxyKind::Npc, slot, upload);
            match upload.mode {
                crate::ProxyDrawMode::PerInstance => {
                    let range = super::proxy_draw::kind_instance_range(
                        upload.npc_count,
                        upload.vehicle_count,
                        super::proxy_draw::ProxyKind::Npc,
                    );
                    let base =
                        proxy.slot_bytes() * slot as u64 + range.start * INSTANCE_STRIDE as u64;
                    for index in 0..upload.npc_count as u64 {
                        unsafe {
                            ctx.device.cmd_bind_vertex_buffers(
                                cmd,
                                1,
                                &[proxy.instances()],
                                &[base + index * INSTANCE_STRIDE as u64],
                            );
                        }
                        unsafe {
                            ctx.device.cmd_draw(cmd, proxy.npc_vertices(), 1, 0, 0);
                        }
                        calls += 1;
                    }
                }
                crate::ProxyDrawMode::Instanced => {
                    unsafe {
                        ctx.device
                            .cmd_draw(cmd, proxy.npc_vertices(), upload.npc_count, 0, 0);
                    }
                    calls += 1;
                }
                crate::ProxyDrawMode::Indirect => {
                    let indirect = proxy.indirect.as_ref().expect(
                        "the indirect ensure discipline makes a missing \
                         command buffer unreachable",
                    );
                    let offset = (slot * crate::proxy_indirect::INDIRECT_SLOT_BYTES) as u64;
                    unsafe {
                        ctx.device.cmd_draw_indirect(
                            cmd,
                            indirect.buffer(),
                            offset,
                            1,
                            crate::proxy_indirect::INDIRECT_COMMAND_BYTES as u32,
                        );
                    }
                    ctx.indirect_draws_issued
                        .set(ctx.indirect_draws_issued.get() + 1);
                    calls += 1;
                }
            }
        }

        // Vehicle kind (the plain depth pipeline)
        if upload.vehicle_count > 0 {
            unsafe {
                ctx.device.cmd_bind_pipeline(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.plain_depth_pipeline,
                );
                ctx.device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    shadows.depth_layout(),
                    0,
                    &[shadows.depth_set()],
                    &[frame_offset],
                );
            }
            proxy.bind_kind_buffers(
                ctx,
                cmd,
                super::proxy_draw::ProxyKind::Vehicle,
                slot,
                upload,
            );
            match upload.mode {
                crate::ProxyDrawMode::PerInstance => {
                    let range = super::proxy_draw::kind_instance_range(
                        upload.npc_count,
                        upload.vehicle_count,
                        super::proxy_draw::ProxyKind::Vehicle,
                    );
                    let base =
                        proxy.slot_bytes() * slot as u64 + range.start * INSTANCE_STRIDE as u64;
                    for index in 0..upload.vehicle_count as u64 {
                        unsafe {
                            ctx.device.cmd_bind_vertex_buffers(
                                cmd,
                                1,
                                &[proxy.instances()],
                                &[base + index * INSTANCE_STRIDE as u64],
                            );
                        }
                        unsafe {
                            ctx.device.cmd_draw(cmd, proxy.vehicle_vertices(), 1, 0, 0);
                        }
                        calls += 1;
                    }
                }
                crate::ProxyDrawMode::Instanced => {
                    unsafe {
                        ctx.device.cmd_draw(
                            cmd,
                            proxy.vehicle_vertices(),
                            upload.vehicle_count,
                            0,
                            0,
                        );
                    }
                    calls += 1;
                }
                crate::ProxyDrawMode::Indirect => {
                    let indirect = proxy.indirect.as_ref().expect(
                        "the indirect ensure discipline makes a missing \
                         command buffer unreachable",
                    );
                    let offset = (slot * crate::proxy_indirect::INDIRECT_SLOT_BYTES
                        + crate::proxy_indirect::INDIRECT_COMMAND_BYTES)
                        as u64;
                    unsafe {
                        ctx.device.cmd_draw_indirect(
                            cmd,
                            indirect.buffer(),
                            offset,
                            1,
                            crate::proxy_indirect::INDIRECT_COMMAND_BYTES as u32,
                        );
                    }
                    ctx.indirect_draws_issued
                        .set(ctx.indirect_draws_issued.get() + 1);
                    calls += 1;
                }
            }
        }

        calls
    }
}
