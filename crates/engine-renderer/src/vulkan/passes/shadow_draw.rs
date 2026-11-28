//! The directional-shadow GPU resources (Cycle 6-F — see
//! `docs/AUTONOMOUS-CYCLE-PROTOCOL.md` §2 item 2): the per-slot
//! shadow maps, the depth-only pass that fills them, the shadowed
//! lit pipeline that samples them, and the readback path the
//! known-answer gate captures through. ONE CASCADE, the corner
//! scene, no framework.
//!
//! ZERO-PER-FRAME-ALLOCATION DISCIPLINE: every object here is
//! created ONCE, lazily, at the first shadowed corner frame
//! ([`PipelineState::ensure_shadow_resources`] — the region-buffer
//! precedent: the unshadowed corner path pays nothing and its
//! existing allocation pins are untouched). The per-frame extended
//! FrameParams (view/light matrices + the shadow control) are
//! written into ONE persistently mapped uniform with a disjoint
//! region per frame slot, after the slot's fence wait — the
//! corner/region/overlay discipline.
//!
//! SYNCHRONIZATION: the shadow map is a per-frame-slot PING-PONG —
//! slot S's depth pass overwrites slot S's image only inside the
//! frame that first waited slot S's fence, so the previous frame's
//! in-flight fragment reads of that image are complete. Within a
//! frame, the depth pass's final subpass dependency
//! (`LATE_FRAGMENT_TESTS`/`DEPTH_STENCIL_ATTACHMENT_WRITE` →
//! `FRAGMENT_SHADER`/`SHADER_READ`) orders the map's writes before
//! the main pass's texel fetches, and the pass's final layout is
//! `SHADER_READ_ONLY_OPTIMAL` (the transition happens inside the
//! pass). The readback copy transitions `SHADER_READ` →
//! `TRANSFER_SRC` inside the same command buffer, after the main
//! pass, so the submission fence proves the copy complete before
//! `take` reads it (the color-capture lifecycle, applied to depth).
//! The transfer-to-host visibility is established via a buffer
//! memory barrier after the copy (the fence proves completion, the
//! barrier makes the write visible to the host reader).
//!
//! THE KNOWN-ANSWER PATH: the depth-only pass writes UNBIASED
//! light-space depths (the compare-time analytical bias lives in the
//! shadowed shader and the CPU oracle), so every texel is exactly
//! the CPU raycaster's `t / (far − near)` — the capture gate pins
//! the whole map against `shadows::texel_ray_origin` +
//! `shadow_ray_hit`.

use super::corner::CornerResources;
use super::shader_layout_pins;
use super::{RendererError, VulkanContext, create_mapped_buffer, descriptors::create_sets};
use crate::config::{FRAMES_IN_FLIGHT, TargetFaultSite, TargetFaultStage};
use crate::shadows::SHADOW_MAP_EXTENT;
use ash::vk;
use std::mem::offset_of;

// Sibling modules (same `vulkan` parent)
use super::proxy_draw;
use super::skinned_draw;

/// The pass families' material-count shape — the attach-time
/// contract's own constant (P4 S2: the lit/shadowed/skinned/
/// reflection descriptor sets are compiled for the corner-era
/// table; `SceneDescription::validate` refuses any other count at
/// attach, so this belt check compares the attached table against
/// the same single source). The shadowed sets' fixed count: one
/// set per material × frame slot.
const MATERIAL_COUNT: usize = crate::scene_description::PASS_FAMILY_MATERIAL_COUNT;

/// The shadow map's depth format: D32_SFLOAT — one f32 per texel,
/// exactly what the known-answer readback and `textureLoad`'s
/// `texture_depth_2d` consume (no UNORM quantization between the
/// rasterized depth and the compared value). Required, not
/// negotiated, for this package's one-cascade scope.
const SHADOW_FORMAT: vk::Format = vk::Format::D32_SFLOAT;

/// The extended FrameParams block the shadowed lit shader reads (the
/// unshadowed shader reads only the 112-byte prefix; the light
/// matrix repeats at the region's aligned tail for the depth pass's
/// own set — see `light_offset`).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct ShadowFrameParams {
    pub view_proj: [[f32; 4]; 4],
    pub light_dir: [f32; 4],
    pub light_color: [f32; 4],
    pub ambient: [f32; 4],
    pub light_view_proj: [[f32; 4]; 4],
    /// x = map extent (texels), y = bias constant, z = bias slope
    /// scale, w = 0.
    pub shadow_control: [f32; 4],
}

// The P4 S5 layout pins (design §6.4): this Rust mirror is checked
// against the WGSL-derived layout constants the build emits from the
// COMPOSED shadowed variants (see `vulkan::shader_layout_pins`) — a
// drift on either side fails the build. The base members assert
// against the PLAIN block's pins because the shadowed block's base
// is build-checked to pin-identically to it (the unshadowed shader
// reads the same 112-byte prefix — the prefix invariant). The
// hand-written 192-byte size pin this replaces dated from 6-F; the
// values are the same, now single-sourced from the shader side.
const _: () =
    assert!(size_of::<ShadowFrameParams>() == shader_layout_pins::SHADOW_FRAME_PARAMS_SIZE);
const _: () = assert!(
    offset_of!(ShadowFrameParams, view_proj) == shader_layout_pins::FRAME_PARAMS_VIEW_PROJ_OFFSET
);
const _: () = assert!(
    offset_of!(ShadowFrameParams, light_dir) == shader_layout_pins::FRAME_PARAMS_LIGHT_DIR_OFFSET
);
const _: () = assert!(
    offset_of!(ShadowFrameParams, light_color)
        == shader_layout_pins::FRAME_PARAMS_LIGHT_COLOR_OFFSET
);
const _: () = assert!(
    offset_of!(ShadowFrameParams, ambient) == shader_layout_pins::FRAME_PARAMS_AMBIENT_OFFSET
);
const _: () = assert!(
    offset_of!(ShadowFrameParams, light_view_proj)
        == shader_layout_pins::SHADOW_FRAME_PARAMS_LIGHT_VIEW_PROJ_OFFSET
);
const _: () = assert!(
    offset_of!(ShadowFrameParams, shadow_control)
        == shader_layout_pins::SHADOW_FRAME_PARAMS_SHADOW_CONTROL_OFFSET
);
// The depth-only pass's LightMatrix host form: the light matrix this
// module re-writes at `light_offset` for the depth set's binding 0 —
// pinned to the WGSL LightMatrix block the depth variants declare
// (the block's only member must sit at offset 0 for the matrix write
// to land on it).
const _: () = assert!(size_of::<[[f32; 4]; 4]>() == shader_layout_pins::LIGHT_MATRIX_SIZE);
const _: () = assert!(shader_layout_pins::LIGHT_MATRIX_LIGHT_VIEW_PROJ_OFFSET == 0);

/// The extended FrameParams block's byte size (the shadowed proxy
/// sets' binding-2 range — pub for the sibling state's set writes).
#[allow(dead_code)]
pub(super) const SHADOW_FRAME_PARAMS_SIZE: u64 = size_of::<ShadowFrameParams>() as u64;

impl ShadowFrameParams {
    /// The clear color (the ambient sky tone, the corner's own
    /// convention — the shadowed frame clears identically).
    pub(crate) fn clear_color(&self) -> [f32; 4] {
        let [r, g, b, _] = self.ambient;
        [r, g, b, 1.]
    }
}

/// The lazily-created shadow state, owned by [`super::PipelineState`]
/// (dies with the pipelines that read it; a replacement re-ensures an
/// identical set before installation — finding A).
pub(crate) struct ShadowResources {
    extent: u32,
    /// Per-slot shadow maps + their depth-pass framebuffers.
    images: Vec<vk::Image>,
    memories: Vec<vk::DeviceMemory>,
    views: Vec<vk::ImageView>,
    framebuffers: Vec<vk::Framebuffer>,
    /// The depth-only pass and pipeline (draws the corner mesh with
    /// the light matrix; no color attachments).
    depth_pass: vk::RenderPass,
    depth_pipeline: vk::Pipeline,
    depth_layout: vk::PipelineLayout,
    depth_pool: vk::DescriptorPool,
    depth_set_layout: vk::DescriptorSetLayout,
    depth_set: vk::DescriptorSet,
    /// The shadowed lit pipeline (runs in the MAIN render pass).
    shadowed_pipeline: vk::Pipeline,
    shadowed_layout: vk::PipelineLayout,
    shadowed_pool: vk::DescriptorPool,
    shadowed_set_layout: vk::DescriptorSetLayout,
    /// Per material × slot (binding 4 — the map view — differs per
    /// slot; the FrameParams dynamic offset selects the region).
    shadowed_sets: [vk::DescriptorSet; MATERIAL_COUNT * 2],
    /// The mapped extended-FrameParams uniform: per-slot regions
    /// (each holding the block plus the light matrix at the aligned
    /// `light_offset`) and the immutable material tail.
    frame_buffer: vk::Buffer,
    frame_memory: vk::DeviceMemory,
    frame_mapped: *mut core::ffi::c_void,
    frame_stride: u32,
    light_offset: u32,
    /// The known-answer readback: one tightly packed f32 per texel,
    /// copied inside the recording frame's command buffer. The
    /// request/ready handshake lives in `FrameState` (it must
    /// survive pipeline replacements, like the color capture's);
    /// this buffer is the shadow state's own.
    readback: vk::Buffer,
    readback_memory: vk::DeviceMemory,
    readback_mapped: *mut core::ffi::c_void,
}

impl ShadowResources {
    /// Builds the whole shadow state against the MAIN render pass
    /// (the shadowed lit pipeline renders inside it) and the corner's
    /// textures/sampler/mesh. Partial-construction safe: `release`
    /// frees exactly what exists.
    pub(super) fn new(
        ctx: &VulkanContext,
        main_render_pass: vk::RenderPass,
        corner: &CornerResources,
    ) -> Result<Self, RendererError> {
        let mut partial = ShadowResources {
            extent: SHADOW_MAP_EXTENT,
            images: Vec::new(),
            memories: Vec::new(),
            views: Vec::new(),
            framebuffers: Vec::new(),
            depth_pass: vk::RenderPass::null(),
            depth_pipeline: vk::Pipeline::null(),
            depth_layout: vk::PipelineLayout::null(),
            depth_pool: vk::DescriptorPool::null(),
            depth_set_layout: vk::DescriptorSetLayout::null(),
            depth_set: vk::DescriptorSet::null(),
            shadowed_pipeline: vk::Pipeline::null(),
            shadowed_layout: vk::PipelineLayout::null(),
            shadowed_pool: vk::DescriptorPool::null(),
            shadowed_set_layout: vk::DescriptorSetLayout::null(),
            shadowed_sets: [vk::DescriptorSet::null(); MATERIAL_COUNT * 2],
            frame_buffer: vk::Buffer::null(),
            frame_memory: vk::DeviceMemory::null(),
            frame_mapped: core::ptr::null_mut(),
            frame_stride: 0,
            light_offset: 0,
            readback: vk::Buffer::null(),
            readback_memory: vk::DeviceMemory::null(),
            readback_mapped: core::ptr::null_mut(),
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
        let slots = FRAMES_IN_FLIGHT as usize;
        // The attached scene's material table (P4 S2: the S1-era read
        // of `lit_scene::materials()` — the description's own values,
        // pinned equal to the reference table by the shim-equality
        // gate while both exist).
        let material_count = corner.materials().len();
        if material_count != MATERIAL_COUNT {
            return Err(RendererError::Vulkan(format!(
                "the scene material table changed ({material_count} entries, \
                 the shadowed sets are built for {MATERIAL_COUNT})"
            )));
        }

        // --- the per-slot shadow maps (image + memory + view) and
        // the depth-only render pass + framebuffers.
        let depth_attachment = vk::AttachmentDescription::default()
            .format(SHADOW_FORMAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        let depth_reference = vk::AttachmentReference {
            attachment: 0,
            layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
        };
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .depth_stencil_attachment(&depth_reference);
        // The ordering the whole package rests on: the map's depth
        // writes (the pass's LATE_FRAGMENT_TESTS, including its
        // final-layout transition) happen-before the main pass's
        // fragment-shader texel fetches.
        let dependency = vk::SubpassDependency::default()
            .src_subpass(0)
            .dst_subpass(vk::SUBPASS_EXTERNAL)
            .src_stage_mask(vk::PipelineStageFlags::LATE_FRAGMENT_TESTS)
            .src_access_mask(vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags::SHADER_READ);
        let dependencies = [dependency];
        let attachments = [depth_attachment];
        let subpasses = [subpass];
        let render_pass_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        // SAFETY: fresh objects; the partial state owns them from
        // creation and `release` frees them exactly once.
        self.depth_pass = unsafe { ctx.device.create_render_pass(&render_pass_info, None) }
            .map_err(|error| RendererError::vk(error, "shadow render pass"))?;

        let extent = vk::Extent3D {
            width: self.extent,
            height: self.extent,
            depth: 1,
        };
        for _slot in 0..slots {
            let image_info = vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(SHADOW_FORMAT)
                .extent(extent)
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                // The depth pass writes it; the main pass samples it;
                // the known-answer readback copies it out.
                .usage(
                    vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
                        | vk::ImageUsageFlags::SAMPLED
                        | vk::ImageUsageFlags::TRANSFER_SRC,
                )
                .sharing_mode(vk::SharingMode::EXCLUSIVE);
            // Fault check before image creation (site: Shadow, stage: Image)
            ctx.check_target_fault(TargetFaultSite::Shadow, TargetFaultStage::Image)?;
            let image = unsafe { ctx.create_target_image(&image_info, "shadow image") }?;
            // Immediate registration (corrective finding 8): the
            // handle is owned from creation, so any later failure in
            // this slot's construction unwinds through `release` —
            // the post/reflection target builders' pattern.
            self.images.push(image);
            let requirements = unsafe { ctx.device.get_image_memory_requirements(image) };
            // Fault check before memory type lookup (site: Shadow, stage: MemoryLookup)
            ctx.check_target_fault(TargetFaultSite::Shadow, TargetFaultStage::MemoryLookup)?;
            let memory_index = super::find_memory_type(
                ctx,
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .ok_or_else(|| RendererError::NoDevice("no DEVICE_LOCAL memory".into()))?;
            let memory_info = vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(memory_index);
            // SAFETY: the requirement query is for this live image;
            // the partial state owns both handles (the image is
            // registered above; the memory registers on the next
            // line).
            // Fault check before memory allocation (site: Shadow, stage: Memory)
            ctx.check_target_fault(TargetFaultSite::Shadow, TargetFaultStage::Memory)?;
            let memory =
                unsafe { ctx.allocate_target_memory(&memory_info, "shadow image memory") }?;
            self.memories.push(memory);
            // Fault check before memory binding (site: Shadow, stage: Bind)
            ctx.check_target_fault(TargetFaultSite::Shadow, TargetFaultStage::Bind)?;
            unsafe { ctx.device.bind_image_memory(image, memory, 0) }
                .map_err(|error| RendererError::vk(error, "bind shadow image"))?;
            let subresource = vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::DEPTH,
                level_count: 1,
                layer_count: 1,
                ..Default::default()
            };
            let view_info = vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(SHADOW_FORMAT)
                .subresource_range(subresource);
            // Fault check before view creation (site: Shadow, stage: View)
            ctx.check_target_fault(TargetFaultSite::Shadow, TargetFaultStage::View)?;
            let view = unsafe { ctx.create_target_image_view(&view_info, "shadow image view") }?;
            self.views.push(view);
            let framebuffer_attachments = [view];
            let framebuffer_info = vk::FramebufferCreateInfo::default()
                .render_pass(self.depth_pass)
                .attachments(&framebuffer_attachments)
                .width(self.extent)
                .height(self.extent)
                .layers(1);
            // Fault check before framebuffer creation (site: Shadow, stage: Framebuffer)
            ctx.check_target_fault(TargetFaultSite::Shadow, TargetFaultStage::Framebuffer)?;
            let framebuffer =
                unsafe { ctx.create_target_framebuffer(&framebuffer_info, "shadow framebuffer") }?;
            self.framebuffers.push(framebuffer);
        }

        // --- the mapped extended-FrameParams uniform: per-slot
        // regions (block + the light matrix at the aligned tail
        // offset) + the immutable material tail.
        // SAFETY: the same live device that owns the mapped UBO.
        let limits = unsafe {
            ctx.instance
                .get_physical_device_properties(ctx.physical_device)
        }
        .limits;
        let align = limits.min_uniform_buffer_offset_alignment.max(1);
        let light_offset = (size_of::<ShadowFrameParams>() as u64).next_multiple_of(align) as u32;
        let frame_stride = ((light_offset as u64) + 64).next_multiple_of(align) as u32;
        let tail_stride = (size_of::<[f32; 4]>() as u64).max(align);
        let total =
            u64::from(frame_stride) * slots as u64 + tail_stride * corner.materials().len() as u64;
        let (frame_buffer, frame_memory, frame_mapped) =
            create_mapped_buffer(ctx, vk::BufferUsageFlags::UNIFORM_BUFFER, total)?;
        self.frame_buffer = frame_buffer;
        self.frame_memory = frame_memory;
        self.frame_mapped = frame_mapped;
        self.frame_stride = frame_stride;
        self.light_offset = light_offset;
        // The immutable material tail, written once (the attached
        // scene's own values — the description's table, the same
        // values the corner's own uniform tail carries).
        // SAFETY: disjoint aligned tail in coherent memory, written
        // once before any submission.
        unsafe {
            for (index, mat) in corner.materials().iter().enumerate() {
                let offset =
                    (slots as u64 * frame_stride as u64 + index as u64 * tail_stride) as usize;
                let params = [
                    mat.specular[0],
                    mat.specular[1],
                    mat.specular[2],
                    mat.shininess as f32,
                ];
                self.frame_mapped
                    .byte_add(offset)
                    .cast::<[f32; 4]>()
                    .write(params);
            }
        }

        // --- the shadowed lit pipeline's descriptor sets: per
        // material × slot (binding 4 differs per slot).
        let frag = vk::ShaderStageFlags::FRAGMENT;
        let both = frag | vk::ShaderStageFlags::VERTEX;
        let samplers = [corner.repeat_sampler()];
        let binding = |index: u32, ty: vk::DescriptorType, stages: vk::ShaderStageFlags| {
            vk::DescriptorSetLayoutBinding::default()
                .binding(index)
                .descriptor_type(ty)
                .descriptor_count(1)
                .stage_flags(stages)
        };
        let combined = vk::DescriptorType::COMBINED_IMAGE_SAMPLER;
        let dynamic = vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC;
        let uniform = vk::DescriptorType::UNIFORM_BUFFER;
        let sampled = vk::DescriptorType::SAMPLED_IMAGE;
        let shadowed_bindings = [
            binding(0, combined, frag).immutable_samplers(&samplers),
            binding(2, dynamic, both),
            binding(3, uniform, frag),
            binding(4, sampled, frag),
        ];
        let material_count = MATERIAL_COUNT as u32;
        let set_count = material_count * slots as u32;
        let shadowed_sizes =
            [combined, dynamic, uniform, sampled].map(|ty| vk::DescriptorPoolSize {
                ty,
                descriptor_count: set_count,
            });
        let (shadowed_pool, shadowed_set_layout, shadowed_sets) =
            create_sets::<{ MATERIAL_COUNT * 2 }>(
                ctx,
                &shadowed_bindings,
                &shadowed_sizes,
                set_count,
            )?;
        self.shadowed_pool = shadowed_pool;
        self.shadowed_set_layout = shadowed_set_layout;
        for material in 0..material_count as usize {
            let view = corner
                .albedo_view(material as u32)
                .ok_or_else(|| RendererError::Vulkan("corner material out of table".into()))?;
            let tail_offset =
                slots as u64 * u64::from(frame_stride) + material as u64 * tail_stride;
            for slot in 0..slots {
                let set = shadowed_sets[material * slots + slot];
                let image = [vk::DescriptorImageInfo::default()
                    .image_view(view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                let buffer = vk::DescriptorBufferInfo::default().buffer(frame_buffer);
                let frame = [buffer.range(size_of::<ShadowFrameParams>() as u64)];
                let material_params = [buffer.offset(tail_offset).range(16)];
                let map = [vk::DescriptorImageInfo::default()
                    .image_view(self.views[slot])
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                let writes = [
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(0)
                        .descriptor_type(combined)
                        .image_info(&image),
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(2)
                        .descriptor_type(dynamic)
                        .buffer_info(&frame),
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(3)
                        .descriptor_type(uniform)
                        .buffer_info(&material_params),
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(4)
                        .descriptor_type(sampled)
                        .image_info(&map),
                ];
                // SAFETY: fresh sets from a live pool; every
                // referenced object (views, buffer, sampler)
                // outlives the pool.
                unsafe { ctx.device.update_descriptor_sets(&writes, &[]) };
            }
        }
        self.shadowed_sets = shadowed_sets;
        // --- the depth pass's light-matrix set (binding 0: the
        // light matrix at `light_offset`, selected per slot by the
        // dynamic offset).
        let depth_bindings = [binding(0, dynamic, vk::ShaderStageFlags::VERTEX)];
        let depth_sizes = [vk::DescriptorPoolSize {
            ty: dynamic,
            descriptor_count: 1,
        }];
        let (depth_pool, depth_set_layout, [depth_set]) =
            create_sets(ctx, &depth_bindings, &depth_sizes, 1)?;
        self.depth_pool = depth_pool;
        self.depth_set_layout = depth_set_layout;
        self.depth_set = depth_set;
        let light = [vk::DescriptorBufferInfo::default()
            .buffer(frame_buffer)
            .offset(light_offset as u64)
            .range(64)];
        let light_write = [vk::WriteDescriptorSet::default()
            .dst_set(depth_set)
            .dst_binding(0)
            .descriptor_type(dynamic)
            .buffer_info(&light)];
        // SAFETY: fresh set; the buffer outlives the pool.
        unsafe { ctx.device.update_descriptor_sets(&light_write, &[]) };

        // --- the depth-only pipeline (the corner mesh's vertex
        // format, no color attachments, depth write on).
        let module = super::create_shader_module(
            ctx,
            include_bytes!(concat!(env!("OUT_DIR"), "/shadow_depth.spv")),
        )?;
        let depth_result: Result<vk::Pipeline, RendererError> = (|| {
            // SAFETY: the module is destroyed exactly once below.
            let stages = [vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(module)
                .name(c"vs_main")];
            let bindings = [vk::VertexInputBindingDescription {
                binding: 0,
                stride: size_of::<crate::lit_mesh::LitVertex>() as u32,
                input_rate: vk::VertexInputRate::VERTEX,
            }];
            let attributes = [
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
                vk::VertexInputAttributeDescription {
                    binding: 0,
                    location: 2,
                    format: vk::Format::R32G32_SFLOAT,
                    offset: 24,
                },
            ];
            let inputs = vk::PipelineVertexInputStateCreateInfo::default()
                .vertex_binding_descriptions(&bindings)
                .vertex_attribute_descriptions(&attributes);
            let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
                .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
            let viewport_state = vk::PipelineViewportStateCreateInfo::default()
                .viewport_count(1)
                .scissor_count(1);
            // The corner mesh winds CCW about its outward normals;
            // from the light's viewpoint those are the lit faces.
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
            let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
            let dynamic_state =
                vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
            let layouts = [depth_set_layout];
            // SAFETY: the set layout is owned above; the partial
            // state owns the pipeline layout immediately.
            let layout = unsafe {
                ctx.device.create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts),
                    None,
                )
            }
            .map_err(|e| RendererError::vk(e, "shadow depth layout"))?;
            self.depth_layout = layout;
            let info = vk::GraphicsPipelineCreateInfo::default()
                .stages(&stages)
                .vertex_input_state(&inputs)
                .input_assembly_state(&assembly)
                .viewport_state(&viewport_state)
                .rasterization_state(&rasterizer)
                .multisample_state(&multisampling)
                .depth_stencil_state(&depth)
                .dynamic_state(&dynamic_state)
                .layout(layout)
                .render_pass(self.depth_pass)
                .subpass(0);
            // SAFETY: the depth pass above owns the compatibility.
            unsafe {
                ctx.device
                    .create_graphics_pipelines(ctx.pipeline_cache, &[info], None)
            }
            .map_err(|(_, e)| RendererError::vk(e, "shadow depth pipeline"))
            .map(|pipelines| pipelines[0])
        })();
        // SAFETY: consumed by the pipelines above (or failed with
        // none alive); destroyed exactly once.
        unsafe { ctx.device.destroy_shader_module(module, None) };
        self.depth_pipeline = depth_result?;

        // --- the shadowed lit pipeline (lit_scene's vertex input,
        // rasterizer, depth, and blend; the shadowed shader's
        // bindings).
        let module = super::create_shader_module(
            ctx,
            include_bytes!(concat!(env!("OUT_DIR"), "/lit_scene_shadowed.spv")),
        )?;
        let shadowed_result: Result<vk::Pipeline, RendererError> = (|| {
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
            let bindings = [vk::VertexInputBindingDescription {
                binding: 0,
                stride: size_of::<crate::lit_mesh::LitVertex>() as u32,
                input_rate: vk::VertexInputRate::VERTEX,
            }];
            let attributes = [
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
                vk::VertexInputAttributeDescription {
                    binding: 0,
                    location: 2,
                    format: vk::Format::R32G32_SFLOAT,
                    offset: 24,
                },
            ];
            let inputs = vk::PipelineVertexInputStateCreateInfo::default()
                .vertex_binding_descriptions(&bindings)
                .vertex_attribute_descriptions(&attributes);
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
            let blend =
                vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
            let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
            let dynamic_state =
                vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
            let layouts = [shadowed_set_layout];
            // SAFETY: the set layout is owned above; the partial
            // state owns the pipeline layout immediately.
            let layout = unsafe {
                ctx.device.create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts),
                    None,
                )
            }
            .map_err(|e| RendererError::vk(e, "shadowed layout"))?;
            self.shadowed_layout = layout;
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
            .map_err(|(_, e)| RendererError::vk(e, "shadowed pipeline"))
            .map(|pipelines| pipelines[0])
        })();
        // SAFETY: consumed by the pipelines above (or failed with
        // none alive); destroyed exactly once.
        unsafe { ctx.device.destroy_shader_module(module, None) };
        self.shadowed_pipeline = shadowed_result?;

        // --- the known-answer readback buffer (one f32 per texel).
        let readback_bytes =
            u64::from(self.extent) * u64::from(self.extent) * size_of::<f32>() as u64;
        let (readback, readback_memory, readback_mapped) =
            create_mapped_buffer(ctx, vk::BufferUsageFlags::TRANSFER_DST, readback_bytes)?;
        self.readback = readback;
        self.readback_memory = readback_memory;
        self.readback_mapped = readback_mapped;
        Ok(())
    }

    /// Writes this slot's extended FrameParams region (the block at
    /// the region start, the light matrix again at `light_offset`
    /// for the depth pass's set).
    ///
    /// # Safety
    /// `slot` must be owned (its fence wait passed).
    pub(super) unsafe fn write_frame(&self, slot: usize, params: &ShadowFrameParams) {
        let base = slot as u32 * self.frame_stride;
        // SAFETY: the documented caller contract — this slot's
        // region is disjoint from every other live reader.
        unsafe {
            self.frame_mapped
                .byte_add(base as usize)
                .cast::<ShadowFrameParams>()
                .write(*params);
            self.frame_mapped
                .byte_add((base + self.light_offset) as usize)
                .cast::<[[f32; 4]; 4]>()
                .write(params.light_view_proj);
        }
    }

    /// Records the depth-only shadow pass for this slot: clear to
    /// 1.0, draw the whole corner mesh once through the light
    /// matrix, then the populated proxy casts (npc skins, vehicle
    /// plain) through the shadow map. Must run BEFORE the main pass
    /// in the same command buffer (the pass's final subpass
    /// dependency orders the map's writes before the main pass's
    /// fragment reads).
    ///
    /// # Safety
    /// `cmd` must be recording (pre-main-pass); `slot`'s map is
    /// written here and read by the main pass that follows.
    /// `shadowed_proxy`, `proxy`, and `retained` must be the frame
    /// flow's ensure-discipline states (shadows, proxy, the
    /// context's retained skinned state).
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn record_depth_pass(
        &self,
        ctx: &VulkanContext,
        cmd: vk::CommandBuffer,
        corner: &CornerResources,
        slot: usize,
        shadowed_proxy: Option<&skinned_draw::ShadowedProxyResources>,
        proxy: Option<&proxy_draw::ProxyResources>,
        retained: Option<&skinned_draw::SkinnedRetained>,
        upload: Option<&proxy_draw::ProxyFrameUpload<'_>>,
    ) {
        // The whole attached mesh, one draw: the count is the TYPED
        // admission's whole-mesh fact (CA-14/CA-15 — the retained
        // DrawTable's `whole_mesh_vertex_count`, a validated
        // triangle-list boundary, not a re-derivation from the raw
        // table): the ranges are contiguous, covering, and
        // triangle-aligned, so `[0, count)` assembles exactly the
        // primitive set the lit pass's per-material ranges
        // assemble.
        let (vertices, _) = corner.mesh();
        let total = corner.whole_mesh_vertex_count();
        let clear = [vk::ClearValue {
            depth_stencil: vk::ClearDepthStencilValue {
                depth: 1.0,
                stencil: 0,
            },
        }];
        let render_area = vk::Rect2D {
            extent: vk::Extent2D {
                width: self.extent,
                height: self.extent,
            },
            offset: vk::Offset2D { x: 0, y: 0 },
        };
        // SAFETY: the framebuffer and set are this slot's; the map
        // region was written after the slot's fence wait; the mesh
        // ranges are the uploaded corner mesh's own.
        unsafe {
            ctx.device.cmd_begin_render_pass(
                cmd,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.depth_pass)
                    .framebuffer(self.framebuffers[slot])
                    .render_area(render_area)
                    .clear_values(&clear),
                vk::SubpassContents::INLINE,
            );
            ctx.device
                .cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.depth_pipeline);
            ctx.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.depth_layout,
                0,
                &[self.depth_set],
                &[slot as u32 * self.frame_stride],
            );
            let viewport = vk::Viewport {
                x: 0.0,
                y: 0.0,
                width: self.extent as f32,
                height: self.extent as f32,
                min_depth: 0.0,
                max_depth: 1.0,
            };
            ctx.device.cmd_set_viewport(cmd, 0, &[viewport]);
            ctx.device.cmd_set_scissor(cmd, 0, &[render_area]);
            ctx.device
                .cmd_bind_vertex_buffers(cmd, 0, &[vertices], &[0]);
            ctx.device.cmd_draw(cmd, total, 1, 0, 0);

            // The proxy casts (npc skins, vehicle plain) if the
            // shadowed-populated path is active.
            if let (
                Some(shadowed_proxy),
                Some(proxy_state),
                Some(retained_state),
                Some(upload_state),
            ) = (shadowed_proxy, proxy, retained, upload)
            {
                shadowed_proxy.record_depth_casts(
                    ctx,
                    cmd,
                    corner,
                    self,
                    retained_state,
                    slot,
                    upload_state,
                    proxy_state,
                );
            }

            ctx.device.cmd_end_render_pass(cmd);
        }
    }

    /// The shadowed lit pipeline + the per-material sets for this
    /// slot (the main pass's corner draws bind these instead of the
    /// corner's own).
    /// The shadowed-proxy state's companions (the Era-2 stage-4b
    /// accessors — the populated shadowed path binds the shadow
    /// state's own resources through its sibling state in
    /// skinned_draw.rs).
    /// This slot's shadow map view (binding 4's image).
    pub(super) fn map_view(&self, slot: usize) -> vk::ImageView {
        self.views[slot]
    }
    /// The extended-FrameParams uniform buffer (the shadowed sets'
    /// binding 2) and its per-slot dynamic stride.
    pub(super) fn frame_buffer(&self) -> vk::Buffer {
        self.frame_buffer
    }
    pub(super) fn frame_stride(&self) -> u32 {
        self.frame_stride
    }
    /// The light matrix's aligned offset inside each slot's region
    /// (the depth sets' binding-0 static offset).
    #[allow(dead_code)]
    pub(super) fn light_offset(&self) -> u32 {
        self.light_offset
    }
    /// The depth-only render pass (the proxy-cast pipelines' pass).
    pub(super) fn depth_render_pass(&self) -> vk::RenderPass {
        self.depth_pass
    }
    /// The depth pass's own pipeline layout (the vehicle cast
    /// pipeline binds the shadow state's own depth set).
    pub(super) fn depth_layout(&self) -> vk::PipelineLayout {
        self.depth_layout
    }
    /// The depth pass's own descriptor set (the vehicle cast binds
    /// this set with the frame offset dynamic offset).
    pub(super) fn depth_set(&self) -> vk::DescriptorSet {
        self.depth_set
    }

    pub(super) fn shadowed(
        &self,
        slot: usize,
    ) -> (
        vk::Pipeline,
        vk::PipelineLayout,
        [vk::DescriptorSet; 3],
        u32,
    ) {
        let slots = FRAMES_IN_FLIGHT as usize;
        let sets = [
            self.shadowed_sets[slot],
            self.shadowed_sets[slots + slot],
            self.shadowed_sets[2 * slots + slot],
        ];
        (
            self.shadowed_pipeline,
            self.shadowed_layout,
            sets,
            slot as u32 * self.frame_stride,
        )
    }

    /// Records the map-to-buffer copy for this slot (AFTER the main
    /// render pass: the map sits in SHADER_READ; transition to
    /// TRANSFER_SRC, copy, and leave the transition to the next
    /// frame's depth pass, whose initial layout is UNDEFINED). The
    /// request/ready handshake is the caller's (FrameState — it
    /// survives pipeline replacements).
    ///
    /// The buffer memory barrier establishes a transfer-to-host
    /// visibility dependency (the fence proves completion, the
    /// barrier makes the copy's writes visible to the host reader).
    ///
    /// # Safety
    /// `cmd` must be recording, past the main render pass; `slot` is
    /// this frame's slot.
    pub(super) unsafe fn record_readback(
        &self,
        ctx: &VulkanContext,
        cmd: vk::CommandBuffer,
        slot: usize,
    ) {
        let subresource = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::DEPTH,
            level_count: 1,
            layer_count: 1,
            ..Default::default()
        };
        let to_transfer = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .image(self.images[slot])
            .subresource_range(subresource)
            .src_access_mask(vk::AccessFlags::SHADER_READ)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
        // SAFETY: this frame's main-pass fragment reads of the map
        // are recorded above in the same command buffer; the pipeline
        // barrier orders them before the copy.
        unsafe {
            ctx.device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_transfer],
            );
            let copy = vk::BufferImageCopy::default()
                .buffer_offset(0)
                .buffer_row_length(self.extent)
                .buffer_image_height(self.extent)
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::DEPTH,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_extent(vk::Extent3D {
                    width: self.extent,
                    height: self.extent,
                    depth: 1,
                });
            ctx.device.cmd_copy_image_to_buffer(
                cmd,
                self.images[slot],
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.readback,
                &[copy],
            );
            // The transfer write must be visible to the host reader.
            // The buffer is HOST_VISIBLE|HOST_COHERENT (create_mapped_buffer),
            // so no explicit invalidate is needed — the barrier establishes
            // the availability/visibility dependency.
            let buffer_barrier = vk::BufferMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::HOST_READ)
                .buffer(self.readback)
                .offset(0)
                .size(self.extent as vk::DeviceSize * self.extent as vk::DeviceSize * 4)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
            ctx.device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[],
                &[buffer_barrier],
                &[],
            );
        }
    }

    /// The readback destination's identity — the frame-state handshake
    /// pins the producing generation.
    pub(super) fn readback_buffer(&self) -> vk::Buffer {
        self.readback
    }

    /// Reads the readback buffer's depths (row-major, row 0 first).
    /// The caller owns the handshake: only call after the recording
    /// slot's fence proved the copy complete.
    ///
    /// # Safety
    /// The passed fence must be the recording frame's own.
    pub(super) unsafe fn readback_depths(
        &self,
        ctx: &VulkanContext,
        slot_fence: vk::Fence,
    ) -> Result<Vec<f32>, RendererError> {
        // SAFETY: the recording frame's submission fence — proven
        // signaled before the mapped bytes are read.
        unsafe {
            ctx.device
                .wait_for_fences(&[slot_fence], true, super::WAIT_TIMEOUT_NS)
                .map_err(|error| RendererError::vk(error, "wait_for_fences (shadow readback)"))?;
        }
        let texels = self.extent as usize * self.extent as usize;
        let mut depths = vec![0f32; texels];
        // SAFETY: the fence above proves the copy completed into the
        // coherent mapping.
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.readback_mapped.cast::<f32>(),
                depths.as_mut_ptr(),
                texels,
            );
        }
        Ok(depths)
    }

    pub(super) fn destroy(mut self, ctx: &VulkanContext) {
        self.release(ctx);
    }

    /// Releases every handle exactly once; valid for both the
    /// fully-built and the partial state.
    fn release(&mut self, ctx: &VulkanContext) {
        // SAFETY: owned by this state; destroyed only while the
        // device is idle (replacement waits; teardown waits), after
        // the pipelines that reference these objects.
        unsafe {
            if !self.frame_mapped.is_null() {
                ctx.device.unmap_memory(self.frame_memory);
                self.frame_mapped = core::ptr::null_mut();
            }
            if !self.readback_mapped.is_null() {
                ctx.device.unmap_memory(self.readback_memory);
                self.readback_mapped = core::ptr::null_mut();
            }
            ctx.device.destroy_buffer(self.readback, None);
            ctx.device.free_memory(self.readback_memory, None);
            ctx.device.destroy_pipeline(self.shadowed_pipeline, None);
            ctx.device
                .destroy_pipeline_layout(self.shadowed_layout, None);
            ctx.device.destroy_descriptor_pool(self.shadowed_pool, None);
            ctx.device
                .destroy_descriptor_set_layout(self.shadowed_set_layout, None);
            ctx.device.destroy_pipeline(self.depth_pipeline, None);
            ctx.device.destroy_pipeline_layout(self.depth_layout, None);
            ctx.device.destroy_descriptor_pool(self.depth_pool, None);
            ctx.device
                .destroy_descriptor_set_layout(self.depth_set_layout, None);
            for framebuffer in self.framebuffers.drain(..) {
                ctx.destroy_target_framebuffer(framebuffer);
            }
            for view in self.views.drain(..) {
                ctx.destroy_target_image_view(view);
            }
            // Independent drains, not zipped — a partial slot (the
            // finding-8 fault points between image and memory
            // registration) leaves the vectors unbalanced, and a zip
            // would then destroy NEITHER side (the leak the fault gate
            // caught on the post chain: VUID-vkDestroyDevice-device-05137).
            for memory in self.memories.drain(..) {
                ctx.free_target_memory(memory);
            }
            for image in self.images.drain(..) {
                ctx.destroy_target_image(image);
            }
            // Framebuffers before the render pass they reference.
            ctx.device.destroy_render_pass(self.depth_pass, None);
            ctx.device.destroy_buffer(self.frame_buffer, None);
            ctx.device.free_memory(self.frame_memory, None);
        }
    }
}
