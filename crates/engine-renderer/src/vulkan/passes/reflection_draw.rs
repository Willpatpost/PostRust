//! The wet-ground planar-reflection GPU resources (Era-1 package 7
//! — see `docs/CYCLE6-ERA1-BRIEF.md`): the mirrored-scene pass
//! into an offscreen R16G16B16A16_SFLOAT target, and the ground
//! re-draw that samples it Schlick-blended over the already-lit
//! ground.
//!
//! THE PLANAR CONSTRUCTION (the world-mirror form, planar.rs's
//! matrix): the reflection pass renders the MIRRORED world
//! (`ground_reflection_matrix` applied inside the transform —
//! `mirrored_view_projection` = view_proj × M) through the
//! UNCHANGED camera. Every point ON the ground plane lands at
//! exactly its true clip position (the mirror fixes the plane —
//! planar.rs's unit test pins it bit-equal), so the ground's
//! screen footprint samples the mirrored scene behind it at the
//! SAME texel: the ground re-draw reads the reflection target at
//! `frag_coord / extent`, plus the rain ripple's PROJECTED
//! perturbation (Era-2 finding 19's corrective — the shader
//! projects the ground point and its world tangent displacement
//! through the frame block's TRUE view-projection; the mirror
//! fixes the plane, so the true VP and the target's mirrored VP
//! agree bit-exactly on both points — planar.rs's
//! `projected_offset_is_mirror_equivalent_on_the_plane` pins the
//! license). The mirrored buildings live BELOW the plane in this form,
//! so the pass draws ONLY the building ranges (materials 1 and 2)
//! — the ground itself would occlude its own reflection (the
//! mirror surface is the glass, never a rendered object in the
//! mirror).
//!
//! THE WINDING FLIP: the mirror inverts handedness, so the corner
//! mesh's CCW-about-outward-normal faces wind CLOCKWISE in the
//! mirrored render — the reflected lit pipeline sets
//! `front_face CLOCKWISE` (the brief's documented alternative to a
//! viewport Y-flip; the two are equivalent, the pipeline state is
//! the cheaper one). A winding error here empties the mirror
//! (everything culled) — the gate's known-answer pixels catch it.
//!
//! THE BLEND: the ground re-draw runs INSIDE the scene pass (the
//! main pass or the HDR pass — two pipelines, the post package's
//! compatibility discipline), AFTER the mesh draws, with
//! `depth_compare_op EQUAL` and no depth write: the re-draw's
//! quad rasterizes exactly where the ground already drew (the
//! same view-projection, the same vertices — bit-identical depth),
//! and the buildings' nearer depths reject their pixels. The
//! fragment outputs `[reflection.rgb, schlick_blend]` and the
//! fixed-function alpha stage (SRC_ALPHA / ONE_MINUS_SRC_ALPHA)
//! computes `reflection·b + ground·(1−b)` — exactly
//! planar.rs's composite, with the destination being the
//! already-rendered lit ground (the CPU reference's form).
//!
//! ZERO-PER-FRAME-ALLOCATION DISCIPLINE: everything is created
//! ONCE, lazily, at the first wet frame (the region-buffer
//! precedent: the dry path pays nothing and stays byte-identical
//! — the wet re-draw is guarded off it entirely). The extent-bound
//! bundle (the per-slot targets, framebuffers, and sets) rebuilds
//! only when the swapchain's extent or view set changes — only a
//! recreation (already `device_wait_idle`) can cause that; the
//! post package's rebuild-safety proof, verbatim.
//!
//! SYNCHRONIZATION: the per-slot targets are ping-pongs (slot S's
//! reflection overwrites only slot S's images, inside the frame
//! that waited slot S's fence). The reflection pass rides the
//! POST STATE'S OWN HDR RENDER PASS — the attachment descriptions
//! are identical (RGBA16F color, final layout SHADER_READ, + the
//! renderer's depth format), so the passes are compatible and the
//! reflection framebuffers bind it directly (no third render
//! pass, and the HDR lit pipeline family binds the reflection
//! targets without new variants). The reflection pass's writes
//! are ordered before the ground re-draw's fragment reads by an
//! explicit image barrier between the passes (the shadow-map
//! readback's pattern: the pass's final SHADER_READ transition
//! plus a COLOR_ATTACHMENT_OUTPUT → FRAGMENT_SHADER execution
//! dependency).
//!
//! SCOPE (documented, the brief's one-cascade discipline): the
//! mirrored scene is the corner mesh's building masses. The sky
//! and the proxy population are NOT drawn in the mirror (the
//! mirrored sky parameters and the proxy reflections are the
//! documented follow-ups); a wet populated frame renders the
//! population unreflected — a visual simplification, honestly
//! labeled, never a correctness hole.

use super::corner::{CornerResources, FrameParams};
use super::{RendererError, VulkanContext, create_mapped_buffer, find_memory_type};
use crate::config::{FRAMES_IN_FLIGHT, TargetFaultSite, TargetFaultStage};
use crate::lit_scene::LightParams;
use crate::planar::{mirrored_eye, mirrored_view_projection};
use ash::vk;

use std::ops::Range;

/// The reflection target's color format — the HDR pass's own
/// (RGBA16F): the reflection samples carry scene-radiance values
/// and the target is read back through the same SHADER_READ
/// machinery; required, not negotiated (the render-pass
/// compatibility with the post state's HDR pass rests on it).
const REFLECTION_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;

/// The ripple uniform's row count: params0, params1, the A19
/// field-extent bound row, then RIPPLE_POOL_BOUND rings
/// (engine-core's bound — the shader's array size and this layout
/// are ONE contract).
///
/// THE A19 VERSIONED CLAIM (P5 — the reserved-row discipline, the
/// pose_index/alpha precedents): rows[2] is the field-extent bound
/// `(bound_center_x, bound_center_z, bound_radius, 0)` — the
/// conservative bounding circle over the live rings the fragment
/// shader's early-out gate reads. The ring rows shifted
/// `rows[2 + i]` → `rows[3 + i]` and this count went 34 → 35 in
/// the same versioned change; the stride/base arithmetic below
/// derives from this constant and follows; the WGSL struct mirrors
/// it (`ground_reflect.wgsl`'s RippleField). No S5 layout pin
/// covers the ripple block (the pins cover FrameParams/
/// MaterialParams/LightMatrix of the composed variants only), so
/// the contract is the packer↔shader term-for-term mirror plus the
/// identity/ink gates — the planar-side layout test pins the
/// cross-side agreement.
pub(crate) const RIPPLE_UNIFORM_ROWS: usize = 3 + engine_core::ripple::RIPPLE_POOL_BOUND;

/// The packed ripple uniform (the GPU view of engine-core's
/// RippleField): rows[0] = (live count, speed, width,
/// tangent-displacement gain m), rows[1] = (roughen gain, the
/// TEST-ONLY RippleDebugMode lane — renderer.rs's separation knobs,
/// 0.0 in every production frame, 0, 0), rows[2] = the A19
/// field-extent bound (center_x, center_z, radius, 0 — the
/// conservative bounding circle over the live prefix, packed by the
/// same pure packer), rows[3 + i] = (center_x, center_z, age,
/// current amplitude). Copy-sized, zero-allocation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RippleUniformData {
    pub rows: [[f32; 4]; RIPPLE_UNIFORM_ROWS],
}

impl RippleUniformData {
    /// The empty field (count 0 — the shader's loop never runs, the
    /// identity gate's no-op; also the rainless wet path's state).
    pub(crate) fn empty() -> Self {
        Self {
            rows: [[0.0; 4]; RIPPLE_UNIFORM_ROWS],
        }
    }
}

/// The wet frame's camera description (the renderer layer builds
/// it from its own arguments — the true view-projection, eye, and
/// light — before the frame flow packages them into the corner or
/// shadowed FrameParams; the reflection pass's mirrored math
/// consumes it directly). The ripple field rides along as the
/// packed uniform the ground re-draw reads (the per-frame camera
/// description's own lane — `render_frame`'s flat parameter list
/// never changes).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct WetCamera {
    pub view_proj: [[f32; 4]; 4],
    pub eye: [f32; 3],
    pub light: LightParams,
    pub ripple: RippleUniformData,
}

/// The extent-bound half: the per-slot reflection targets and the
/// descriptor sets that bind them (the post targets' pattern —
/// same rebuild proof).
struct ReflectionTargets {
    extent: (u32, u32),
    /// The swapchain generation when this bundle was created — the
    /// view set's identity (recreation produces a new generation;
    /// Vulkan permits handle reuse, so a numerical view handle could
    /// recur after destruction). (corrective finding 7)
    swapchain_generation: u64,
    color_images: Vec<vk::Image>,
    color_memories: Vec<vk::DeviceMemory>,
    color_views: Vec<vk::ImageView>,
    depth_images: Vec<vk::Image>,
    depth_memories: Vec<vk::DeviceMemory>,
    depth_views: Vec<vk::ImageView>,
    framebuffers: Vec<vk::Framebuffer>,
    /// The reflected lit pipeline's per-material × slot sets.
    reflected_pool: vk::DescriptorPool,
    reflected_sets: [vk::DescriptorSet; 6],
    /// The ground re-draw's per-slot sets (the reflection view, the
    /// linear sampler, the corner's own frame buffer).
    ground_pool: vk::DescriptorPool,
    ground_sets: [vk::DescriptorSet; 2],
}

/// The lazily-created reflection state, owned by
/// [`super::PipelineState`] (dies with the pipelines that read it;
/// a replacement re-ensures an identical set before installation —
/// finding A).
pub(crate) struct ReflectionResources {
    /// The mirrored-scene lit pipeline (the HDR lit pipeline's
    /// exact config with FRONT_FACE CLOCKWISE — the winding flip —
    /// bound to the reflection sets' layout).
    lit_reflected: vk::Pipeline,
    reflected_layout: vk::DescriptorSetLayout,
    reflected_pipeline_layout: vk::PipelineLayout,
    /// The ground re-draw inside the MAIN render pass (the dry
    /// renderer's LDR path).
    ground_main: vk::Pipeline,
    /// The ground re-draw inside the HDR scene pass (the post
    /// chain's path — the reflection then rides the tonemap).
    ground_hdr: vk::Pipeline,
    ground_layout: vk::DescriptorSetLayout,
    ground_pipeline_layout: vk::PipelineLayout,
    /// The linear, CLAMP_TO_EDGE sampler for the 1:1 screen-uv
    /// read (CLAMP covers the sub-pixel edges).
    linear_sampler: vk::Sampler,
    /// The mirrored FrameParams uniform: per-slot regions + the
    /// immutable material tail (the shadow state's buffer shape).
    params_buffer: vk::Buffer,
    params_memory: vk::DeviceMemory,
    params_mapped: *mut core::ffi::c_void,
    params_stride: u32,
    tail_stride: u32,
    /// The ripple uniform's per-slot stride (the same align
    /// discipline as params_stride — the ripple regions are dynamic
    /// uniform descriptors).
    ripple_stride: u32,
    /// The ripple regions' base byte offset inside the SAME params
    /// buffer: after the per-slot FrameParams regions and the
    /// immutable material tail (binding 4's descriptor write points
    /// here; the per-frame dynamic offset adds slot × stride).
    ripple_base: u32,
    /// The attached scene's mirror-plane height (P4 S2: the wet
    /// ground role's `receiver_plane_y` — the S1-era read of
    /// `lit_scene::GROUND_Y`, now the description's own authored
    /// plane, carried here at the state's construction).
    ground_plane_y: f32,
    targets: Option<ReflectionTargets>,
}

impl ReflectionResources {
    /// Builds the extent-free half. The HDR render pass comes from
    /// the post state — WET REQUIRES POST'S HDR PASS OBJECT: the
    /// reflection pass is render-pass-compatible with it (the
    /// identical attachment descriptions), and the state is
    /// created at the first wet frame regardless of whether the
    /// post CHAIN itself is on (the pass object exists once the
    /// post state exists; the frame flow ensures the post state
    /// before the reflection state — the wet mode's documented
    /// dependency, the shadow/sky lazy-state orderings' shape).
    pub(super) fn new(
        ctx: &VulkanContext,
        corner: &CornerResources,
        hdr_pass: vk::RenderPass,
        main_render_pass: vk::RenderPass,
    ) -> Result<Self, RendererError> {
        let mut partial = ReflectionResources {
            lit_reflected: vk::Pipeline::null(),
            reflected_layout: vk::DescriptorSetLayout::null(),
            reflected_pipeline_layout: vk::PipelineLayout::null(),
            ground_main: vk::Pipeline::null(),
            ground_hdr: vk::Pipeline::null(),
            ground_layout: vk::DescriptorSetLayout::null(),
            ground_pipeline_layout: vk::PipelineLayout::null(),
            linear_sampler: vk::Sampler::null(),
            params_buffer: vk::Buffer::null(),
            params_memory: vk::DeviceMemory::null(),
            params_mapped: core::ptr::null_mut(),
            params_stride: 0,
            tail_stride: 0,
            ripple_stride: 0,
            ripple_base: 0,
            ground_plane_y: 0.0,
            targets: None,
        };
        if let Err(error) = partial.build(ctx, corner, hdr_pass, main_render_pass) {
            partial.release(ctx);
            return Err(error);
        }
        Ok(partial)
    }

    fn build(
        &mut self,
        ctx: &VulkanContext,
        corner: &CornerResources,
        hdr_pass: vk::RenderPass,
        main_render_pass: vk::RenderPass,
    ) -> Result<(), RendererError> {
        let slots = FRAMES_IN_FLIGHT as usize;
        // The attached scene's material table (P4 S2: the S1-era read
        // of `lit_scene::materials()` — the description's own values,
        // pinned equal to the reference table by the shim-equality
        // gate while both exist).
        let material_count = corner.materials().len();

        // --- the mirrored FrameParams uniform (per-slot regions +
        // the material tail — the shadow state's buffer shape).
        // SAFETY: query the same live device that owns the mapped UBO.
        let limits = unsafe {
            ctx.instance
                .get_physical_device_properties(ctx.physical_device)
        }
        .limits;
        let align = limits.min_uniform_buffer_offset_alignment.max(1);
        let params_stride = ((size_of::<FrameParams>() as u64)
            .max(align)
            .next_multiple_of(align)) as u32;
        let tail_stride = (size_of::<[f32; 4]>() as u64).max(align) as u32;
        // The ripple uniform's per-slot regions (Era-2 package C
        // recursive item 1): the same buffer, sized with the same
        // align discipline as params_stride. The regions live AFTER
        // the material tail — NO new Vulkan object rides the ripple
        // (the Era-1 wet allocation delta pin (7,2,8,2) in
        // tests/render_reflection.rs is untouched: one buffer, one
        // mapped memory, the same pools and sets — binding 4 is a
        // write into the EXISTING ground sets).
        let ripple_stride = ((RIPPLE_UNIFORM_ROWS as u64 * 16)
            .max(align)
            .next_multiple_of(align)) as u32;
        let ripple_base = slots as u64 * u64::from(params_stride)
            + material_count as u64 * u64::from(tail_stride);
        let total = u64::from(params_stride) * slots as u64
            + u64::from(tail_stride) * material_count as u64
            + u64::from(ripple_stride) * slots as u64;
        let (params_buffer, params_memory, params_mapped) =
            create_mapped_buffer(ctx, vk::BufferUsageFlags::UNIFORM_BUFFER, total)?;
        self.params_buffer = params_buffer;
        self.params_memory = params_memory;
        self.params_mapped = params_mapped;
        self.params_stride = params_stride;
        self.tail_stride = tail_stride;
        self.ripple_stride = ripple_stride;
        self.ripple_base = ripple_base as u32;
        // The wet ground's mirror plane (the attached description's
        // receiver plane — the value the camera math below reflects
        // across; the corner's is the ground at y = 0).
        self.ground_plane_y = corner.ground_plane_y();
        // The immutable material tail, written once (the attached
        // scene's own values — the description's table, the same
        // values the corner's own uniform tail carries).
        // SAFETY: disjoint aligned tail in coherent memory, written
        // once before any submission.
        unsafe {
            for (index, mat) in corner.materials().iter().enumerate() {
                let offset = (slots as u64 * u64::from(params_stride)
                    + index as u64 * u64::from(tail_stride)) as usize;
                let params = [
                    mat.specular[0],
                    mat.specular[1],
                    mat.specular[2],
                    mat.shininess as f32,
                ];
                self.params_mapped
                    .byte_add(offset)
                    .cast::<[f32; 4]>()
                    .write(params);
            }
        }
        // The ripple span, ZEROED at construction (the belt): mapped
        // memory is not guaranteed zeroed, and the FIRST frames of a
        // rainless wet session read the uniform before any
        // `set_ripple_field` call — write_frame writes the region
        // before every GPU read too (the suspenders; both documented).
        // The span is whole slots × stride in vec4 rows, starting at
        // ripple_base (the stride is a multiple of 16 — the align
        // discipline's power-of-two align and the 544-byte minimum).
        // SAFETY: disjoint aligned span in coherent memory, written
        // once before any submission.
        unsafe {
            let rows = slots as u64 * u64::from(ripple_stride) / 16;
            for row in 0..rows {
                let offset = (ripple_base + row * 16) as usize;
                self.params_mapped
                    .byte_add(offset)
                    .cast::<[f32; 4]>()
                    .write([0.0f32; 4]);
            }
        }

        // --- the linear sampler.
        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);
        // SAFETY: one live device owns this sampler until the
        // state's teardown.
        self.linear_sampler = unsafe { ctx.device.create_sampler(&sampler_info, None) }
            .map_err(|error| RendererError::vk(error, "reflection sampler"))?;
        // SAFETY: the allocation counter is the context's own (the
        // textures module's discipline).
        ctx.sampler_creations.set(ctx.sampler_creations.get() + 1);

        // --- the reflected lit pipeline's descriptor layout: the
        // corner's own bindings (0 = the combined albedo sampler, 2
        // = the dynamic FrameParams, 3 = the material) with binding
        // 2 pointing at THIS state's mirrored-params buffer.
        let combined = vk::DescriptorType::COMBINED_IMAGE_SAMPLER;
        let dynamic = vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC;
        let uniform = vk::DescriptorType::UNIFORM_BUFFER;
        let frag = vk::ShaderStageFlags::FRAGMENT;
        let both = frag | vk::ShaderStageFlags::VERTEX;
        let samplers = [corner.repeat_sampler()];
        let reflected_bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(combined)
                .descriptor_count(1)
                .stage_flags(frag)
                .immutable_samplers(&samplers),
            vk::DescriptorSetLayoutBinding::default()
                .binding(2)
                .descriptor_type(dynamic)
                .descriptor_count(1)
                .stage_flags(both),
            vk::DescriptorSetLayoutBinding::default()
                .binding(3)
                .descriptor_type(uniform)
                .descriptor_count(1)
                .stage_flags(frag),
        ];
        // SAFETY: fresh object; the partial state owns it.
        self.reflected_layout = {
            // SAFETY: as above.
            unsafe {
                ctx.device.create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&reflected_bindings),
                    None,
                )
            }
            .map_err(|error| RendererError::vk(error, "reflection set layout"))?
        };
        let layouts = [self.reflected_layout];
        // SAFETY: the descriptor layout is owned above; the partial
        // state owns the pipeline layout immediately.
        self.reflected_pipeline_layout = unsafe {
            ctx.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts),
                None,
            )
        }
        .map_err(|error| RendererError::vk(error, "reflection pipeline layout"))?;

        // --- the ground re-draw's layout: 0 = the reflection view,
        // 2 = the sampler, 3 = the corner's own frame buffer
        // (ground_reflect.wgsl's documented map — binding 1 is the
        // atlas share, unusable), 4 = the ripple field (this state's
        // own params buffer, the free slot the shader documents).
        let sampled = vk::DescriptorType::SAMPLED_IMAGE;
        let ground_bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(sampled)
                .descriptor_count(1)
                .stage_flags(frag),
            vk::DescriptorSetLayoutBinding::default()
                .binding(2)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .descriptor_count(1)
                .stage_flags(frag),
            vk::DescriptorSetLayoutBinding::default()
                .binding(3)
                .descriptor_type(dynamic)
                .descriptor_count(1)
                .stage_flags(both),
            // The ripple field (Era-2 package C recursive item 1):
            // UNIFORM_BUFFER_DYNAMIC like binding 3 (the per-slot
            // regions in this state's own params buffer), FRAGMENT
            // only — the perturbation never touches the vertex
            // stage.
            vk::DescriptorSetLayoutBinding::default()
                .binding(4)
                .descriptor_type(dynamic)
                .descriptor_count(1)
                .stage_flags(frag),
        ];
        // SAFETY: fresh object; the partial state owns it.
        self.ground_layout = {
            // SAFETY: as above.
            unsafe {
                ctx.device.create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&ground_bindings),
                    None,
                )
            }
            .map_err(|error| RendererError::vk(error, "ground set layout"))?
        };
        let layouts = [self.ground_layout];
        // SAFETY: as above.
        self.ground_pipeline_layout = unsafe {
            ctx.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts),
                None,
            )
        }
        .map_err(|error| RendererError::vk(error, "ground pipeline layout"))?;

        // --- the pipelines.
        let lit_module = super::create_shader_module(
            ctx,
            include_bytes!(concat!(env!("OUT_DIR"), "/lit_scene.spv")),
        )?;
        let lit_result = Self::lit_pipeline(
            ctx,
            lit_module,
            self.reflected_pipeline_layout,
            hdr_pass,
            "reflection lit pipeline",
            vk::FrontFace::CLOCKWISE,
        );
        // SAFETY: consumed by the pipeline above (or failed with
        // none alive); destroyed exactly once.
        unsafe { ctx.device.destroy_shader_module(lit_module, None) };
        self.lit_reflected = lit_result?;

        let ground_module = super::create_shader_module(
            ctx,
            include_bytes!(concat!(env!("OUT_DIR"), "/ground_reflect.spv")),
        )?;
        let ground_result = Self::ground_pipeline(
            ctx,
            ground_module,
            self.ground_pipeline_layout,
            hdr_pass,
            "reflection ground pipeline (HDR)",
        );
        // SAFETY: as above.
        unsafe { ctx.device.destroy_shader_module(ground_module, None) };
        self.ground_hdr = ground_result?;
        let ground_module = super::create_shader_module(
            ctx,
            include_bytes!(concat!(env!("OUT_DIR"), "/ground_reflect.spv")),
        )?;
        let ground_result = Self::ground_pipeline(
            ctx,
            ground_module,
            self.ground_pipeline_layout,
            main_render_pass,
            "reflection ground pipeline (main)",
        );
        // SAFETY: as above.
        unsafe { ctx.device.destroy_shader_module(ground_module, None) };
        self.ground_main = ground_result?;
        Ok(())
    }

    /// The lit pipeline against the reflection pass — the HDR lit
    /// pipeline's exact config (the lit vertex input, the CCW
    /// convention's rasterizer, the LESS depth) with the FRONT FACE
    /// FLIPPED (the mirror inverts handedness; the winding flip is
    /// the documented alternative to a viewport Y-flip).
    fn lit_pipeline(
        ctx: &VulkanContext,
        module: vk::ShaderModule,
        layout: vk::PipelineLayout,
        render_pass: vk::RenderPass,
        what: &str,
        front_face: vk::FrontFace,
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
        use crate::lit_mesh::LitVertex;
        let bindings = [vk::VertexInputBindingDescription {
            binding: 0,
            stride: size_of::<LitVertex>() as u32,
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
                offset: std::mem::offset_of!(LitVertex, uv) as u32,
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
            .front_face(front_face)
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
            .render_pass(render_pass)
            .subpass(0);
        // SAFETY: the module/layout/render pass are the caller's
        // live objects; the pipeline is returned to the state that
        // owns it.
        unsafe {
            ctx.device
                .create_graphics_pipelines(ctx.pipeline_cache, &[info], None)
        }
        .map_err(|(_, error)| RendererError::vk(error, what))
        .map(|pipelines| pipelines[0])
    }

    /// The ground re-draw pipeline: the lit vertex input, the
    /// ground_reflect fragment, NO culling (the re-draw's quad is
    /// the ground's own winding), depth COMPARE EQUAL with no
    /// write (the re-draw lands exactly on the ground's own depth;
    /// the buildings' nearer depths reject their pixels), and the
    /// fixed-function SRC_ALPHA / ONE_MINUS_SRC_ALPHA blend — the
    /// CPU composite `reflection·b + ground·(1−b)`.
    fn ground_pipeline(
        ctx: &VulkanContext,
        module: vk::ShaderModule,
        layout: vk::PipelineLayout,
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
        use crate::lit_mesh::LitVertex;
        let bindings = [vk::VertexInputBindingDescription {
            binding: 0,
            stride: size_of::<LitVertex>() as u32,
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
                offset: std::mem::offset_of!(LitVertex, uv) as u32,
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
            .cull_mode(vk::CullModeFlags::NONE)
            .line_width(1.0);
        let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(false)
            .depth_compare_op(vk::CompareOp::EQUAL);
        // THE COLOR WRITE MASK is not optional: the state's default
        // is ZERO (no components written — a legal, validation-
        // silent no-op pipeline; found on the first execution: the
        // re-draw recorded, rasterized, and wrote nothing). Every
        // pipeline in this module sets it explicitly; this one must
        // too.
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD)];
        let blend =
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
            .color_blend_state(&blend)
            .dynamic_state(&dynamic_state)
            .layout(layout)
            .render_pass(render_pass)
            .subpass(0);
        // SAFETY: as the lit pipeline above.
        unsafe {
            ctx.device
                .create_graphics_pipelines(ctx.pipeline_cache, &[info], None)
        }
        .map_err(|(_, error)| RendererError::vk(error, what))
        .map(|pipelines| pipelines[0])
    }

    /// Builds (or rebuilds) the extent-bound target bundle — a
    /// no-op when the extent and view set match. The rebuild-safety
    /// proof is the post package's (only a recreation — already
    /// device-idle — can change either). The cache key is the extent
    /// plus the swapchain GENERATION (corrective finding 7): a
    /// retired chain's numerical view handle could recur after
    /// destruction (Vulkan permits handle reuse), but a generation
    /// never repeats, and a larger image count on the new chain
    /// forces a rebuild instead of indexing past the bundle's
    /// framebuffers.
    pub(super) fn ensure_targets(
        &mut self,
        ctx: &VulkanContext,
        extent: vk::Extent2D,
        corner: &CornerResources,
        hdr_pass: vk::RenderPass,
        swapchain_generation: u64,
        depth_format: vk::Format,
    ) -> Result<(), RendererError> {
        if let Some(targets) = &self.targets
            && targets.extent == (extent.width, extent.height)
            && targets.swapchain_generation == swapchain_generation
        {
            return Ok(());
        }
        let mut old = self.targets.take();
        if let Some(old) = &mut old {
            old.release(ctx);
        }
        self.targets = Some(ReflectionTargets::build(
            ctx,
            extent,
            corner,
            hdr_pass,
            swapchain_generation,
            self.reflected_layout,
            self.ground_layout,
            self.linear_sampler,
            self.params_buffer,
            self.params_stride,
            self.tail_stride,
            self.ripple_base,
            self.ripple_stride,
            depth_format,
        )?);
        Ok(())
    }

    /// Writes this slot's MIRRORED FrameParams region (the
    /// reflection pass's own camera math: the mirrored
    /// view-projection and the mirrored eye, planar.rs's functions;
    /// the light unchanged — a directional light's reflection is
    /// the lit mirrored world) AND this slot's ripple region (the
    /// packed uniform from the camera description — the ground
    /// re-draw's binding 4 reads it; written every frame, before
    /// any GPU read, the construction-time zeroing's suspenders).
    ///
    /// # Safety
    /// `slot` must be owned (its fence wait passed).
    pub(super) unsafe fn write_frame(&self, slot: usize, camera: &WetCamera) {
        let mirrored = FrameParams::new(
            mirrored_view_projection(&camera.view_proj, self.ground_plane_y),
            mirrored_eye(camera.eye, self.ground_plane_y),
            camera.light,
        );
        let base = slot as u32 * self.params_stride;
        // SAFETY: the documented caller contract — this slot's
        // region is disjoint from every other live reader.
        unsafe {
            self.params_mapped
                .byte_add(base as usize)
                .cast::<FrameParams>()
                .write(mirrored);
        }
        // The ripple half: this slot's region in the same mapped
        // buffer, after the material tail (ripple_base), at the
        // ripple stride — the fixed-size Copy write, zero
        // allocation. The same slot-ownership proof as the
        // mirrored-params write above (the fence wait owns the
        // slot; the regions are disjoint per slot and from the
        // FrameParams regions and the material tail).
        let ripple_offset = self.ripple_base + slot as u32 * self.ripple_stride;
        // SAFETY: as above — this slot's ripple region, disjoint,
        // written after the fence wait.
        unsafe {
            self.params_mapped
                .byte_add(ripple_offset as usize)
                .cast::<[[f32; 4]; RIPPLE_UNIFORM_ROWS]>()
                .write(camera.ripple.rows);
        }
    }

    /// Records the mirrored-scene pass (BEFORE the scene pass): the
    /// HDR render pass over this slot's reflection target, drawing
    /// ONLY the building ranges (materials 1 and 2 — the mirrored
    /// buildings live below the plane; the ground would occlude
    /// its own reflection), through the winding-flipped lit
    /// pipeline with this slot's mirrored FrameParams. The render
    /// pass is the POST STATE'S HDR PASS (the frame flow passes it
    /// — the attachment descriptions are identical, so the
    /// reflection framebuffers bind it; the wet mode's documented
    /// dependency on the post state's existence, never its chain).
    ///
    /// # Safety
    /// `cmd` must be recording, before the scene pass; `slot`'s
    /// mirrored params were written after the slot's fence wait;
    /// the targets match the live extent and were built against
    /// this same pass object.
    // The recording plumbing's flat parameter list (the render_frame
    // precedent: the context, target, pass object, slot, extent, and
    // clear color are exactly what the record step holds).
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn record_reflection_pass(
        &self,
        ctx: &VulkanContext,
        corner: &CornerResources,
        cmd: vk::CommandBuffer,
        hdr_pass: vk::RenderPass,
        slot: usize,
        extent: vk::Extent2D,
        clear_color: [f32; 4],
        reflected: &[Range<u32>; 2],
    ) {
        let targets = self
            .targets
            .as_ref()
            .expect("the frame flow ensures the targets before recording");
        let clear = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: clear_color,
                },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        ];
        let scissor = vk::Rect2D {
            extent,
            offset: vk::Offset2D { x: 0, y: 0 },
        };
        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: extent.width as f32,
            height: extent.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let slots = FRAMES_IN_FLIGHT as usize;
        let (vertices, _) = corner.mesh();
        // SAFETY: this slot's reflection framebuffer and mirrored
        // params region (written above, after the fence wait); the
        // corner mesh's building ranges; the sets are this state's
        // own per-material × slot sets.
        unsafe {
            ctx.device.cmd_begin_render_pass(
                cmd,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(hdr_pass)
                    .framebuffer(targets.framebuffers[slot])
                    .render_area(scissor)
                    .clear_values(&clear),
                vk::SubpassContents::INLINE,
            );
            ctx.device.cmd_set_viewport(cmd, 0, &[viewport]);
            ctx.device.cmd_set_scissor(cmd, 0, &[scissor]);
            ctx.device
                .cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.lit_reflected);
            ctx.device
                .cmd_bind_vertex_buffers(cmd, 0, &[vertices], &[0]);
            // The building ranges only (materials 1 and 2; material
            // 0 is the ground — the mirror's glass, never drawn).
            // The descriptor set indexing is MATERIAL * slots + slot:
            // position 0 in `reflected` → material 1, position 1 → material 2.
            // This matches the current behavior: sets[1*slots+slot] and sets[2*slots+slot].
            for (position, range) in reflected.iter().enumerate() {
                let material = 1 + position as u32;
                // The sets are laid out per MATERIAL (0-based:
                // material·slots + slot) — the building material N
                // binds its OWN set, not (N-1)'s (the first version
                // handed the buildings the ground's asphalt texture —
                // the mirror rendered dark asphalt walls).
                let set = targets.reflected_sets[material as usize * slots + slot];
                ctx.device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.reflected_pipeline_layout,
                    0,
                    &[set],
                    &[slot as u32 * self.params_stride],
                );
                // CA-15 (F15): the range is a validated
                // triangle-list boundary pair (the admission's
                // GroundRoles check) — this draw assembles exactly
                // the primitives the lit pass drew over the same
                // vertices.
                ctx.device
                    .cmd_draw(cmd, range.end - range.start, 1, range.start, 0);
            }
            ctx.device.cmd_end_render_pass(cmd);
        }
    }

    /// The barrier ordering the reflection pass's writes before the
    /// ground re-draw's fragment reads (the shadow-map readback's
    /// pattern — the pass's final SHADER_READ transition plus the
    /// execution dependency). The mirrored-scene pass's own outgoing
    /// dependency (finding 9) provides the producer-side scope for
    /// the final transition, making the barrier belt-and-suspenders.
    ///
    /// # Safety
    /// `cmd` must be recording, between the reflection pass and the
    /// scene pass; `slot` is this frame's slot.
    pub(super) unsafe fn record_read_barrier(
        &self,
        ctx: &VulkanContext,
        cmd: vk::CommandBuffer,
        slot: usize,
    ) {
        let targets = self
            .targets
            .as_ref()
            .expect("the frame flow ensures the targets before recording");
        let to_read = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(targets.color_images[slot])
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                level_count: 1,
                layer_count: 1,
                ..Default::default()
            })
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
        // SAFETY: this frame's reflection pass was recorded above
        // in the same command buffer; the barrier orders its writes
        // (already transitioned to SHADER_READ by the pass) before
        // the ground re-draw's fragment reads inside the scene pass
        // that follows.
        unsafe {
            ctx.device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_read],
            );
        }
    }

    /// Records the ground re-draw — INSIDE the scene pass (the main
    /// pass or the HDR pass, selected by `in_hdr`), after the mesh
    /// draws: the ground quad re-rasterized (EQUAL depth, no
    /// write) with the reflection sampled at the same screen
    /// position and the Schlick blend on the alpha stage. Uses the
    /// CORNER's own FrameParams regions (the TRUE view-projection
    /// and eye — the ground draws at its true position).
    ///
    /// # Safety
    /// `cmd` must be recording INSIDE the scene pass, after the
    /// corner mesh draws; `slot`'s corner FrameParams region holds
    /// this frame's true values (the frame flow's write); the
    /// reflection barrier was recorded before the scene pass.
    pub(super) unsafe fn record_ground_redraw(
        &self,
        ctx: &VulkanContext,
        corner: &CornerResources,
        cmd: vk::CommandBuffer,
        slot: usize,
        in_hdr: bool,
        receiver: Range<u32>,
    ) {
        let targets = self
            .targets
            .as_ref()
            .expect("the frame flow ensures the targets before recording");
        let pipeline = if in_hdr {
            self.ground_hdr
        } else {
            self.ground_main
        };
        let (vertices, _) = corner.mesh();
        // SAFETY: the pipeline matches the recording render pass
        // (the frame flow selects it); the set binds this slot's
        // reflection view and the corner's own frame region (the
        // TRUE camera — the ground's own draw); the EQUAL depth
        // lands exactly on the ground's own depth (the same
        // view-projection, the same vertices). The TWO dynamic
        // offsets apply to the set's dynamic bindings in ascending
        // binding order — binding 3 (the corner's frame buffer,
        // base offset 0, so slot x frame_stride) then binding 4
        // (this state's ripple region, base offset ripple_base
        // already in the descriptor write, so slot x ripple_stride
        // — the effective offset is ripple_base + slot x stride,
        // inside the buffer's sized span).
        unsafe {
            ctx.device
                .cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
            ctx.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.ground_pipeline_layout,
                0,
                &[targets.ground_sets[slot]],
                &[
                    slot as u32 * corner.frame_stride(),
                    slot as u32 * self.ripple_stride,
                ],
            );
            ctx.device
                .cmd_bind_vertex_buffers(cmd, 0, &[vertices], &[0]);
            // CA-15 (F15): the receiver is a validated
            // triangle-list boundary pair (the admission's
            // GroundRoles check) — the EQUAL-depth re-draw
            // rasterizes exactly the primitives the lit pass drew
            // over the same vertices, so the depth equality the
            // receiver relies on holds.
            ctx.device
                .cmd_draw(cmd, receiver.end - receiver.start, 1, receiver.start, 0);
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
        // the pipelines that reference these objects.
        unsafe {
            if let Some(mut targets) = self.targets.take() {
                targets.release(ctx);
            }
            if !self.params_mapped.is_null() {
                ctx.device.unmap_memory(self.params_memory);
                self.params_mapped = core::ptr::null_mut();
            }
            ctx.device.destroy_buffer(self.params_buffer, None);
            ctx.device.free_memory(self.params_memory, None);
            ctx.device.destroy_sampler(self.linear_sampler, None);
            ctx.device.destroy_pipeline(self.ground_main, None);
            ctx.device.destroy_pipeline(self.ground_hdr, None);
            ctx.device.destroy_pipeline(self.lit_reflected, None);
            ctx.device
                .destroy_pipeline_layout(self.ground_pipeline_layout, None);
            ctx.device
                .destroy_descriptor_set_layout(self.ground_layout, None);
            ctx.device
                .destroy_pipeline_layout(self.reflected_pipeline_layout, None);
            ctx.device
                .destroy_descriptor_set_layout(self.reflected_layout, None);
        }
    }
}

impl ReflectionTargets {
    #[allow(clippy::too_many_arguments)]
    fn build(
        ctx: &VulkanContext,
        extent: vk::Extent2D,
        corner: &CornerResources,
        hdr_pass: vk::RenderPass,
        swapchain_generation: u64,
        reflected_layout: vk::DescriptorSetLayout,
        ground_layout: vk::DescriptorSetLayout,
        linear_sampler: vk::Sampler,
        params_buffer: vk::Buffer,
        params_stride: u32,
        tail_stride: u32,
        ripple_base: u32,
        ripple_stride: u32,
        depth_format: vk::Format,
    ) -> Result<ReflectionTargets, RendererError> {
        let mut partial = ReflectionTargets {
            extent: (extent.width, extent.height),
            swapchain_generation,
            color_images: Vec::new(),
            color_memories: Vec::new(),
            color_views: Vec::new(),
            depth_images: Vec::new(),
            depth_memories: Vec::new(),
            depth_views: Vec::new(),
            framebuffers: Vec::new(),
            reflected_pool: vk::DescriptorPool::null(),
            reflected_sets: [vk::DescriptorSet::null(); 6],
            ground_pool: vk::DescriptorPool::null(),
            ground_sets: [vk::DescriptorSet::null(); 2],
        };
        if let Err(error) = partial.build_targets(
            ctx,
            extent,
            corner,
            hdr_pass,
            reflected_layout,
            ground_layout,
            linear_sampler,
            params_buffer,
            params_stride,
            tail_stride,
            ripple_base,
            ripple_stride,
            swapchain_generation,
            depth_format,
        ) {
            partial.release(ctx);
            return Err(error);
        }
        Ok(partial)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_targets(
        &mut self,
        ctx: &VulkanContext,
        extent: vk::Extent2D,
        corner: &CornerResources,
        hdr_pass: vk::RenderPass,
        reflected_layout: vk::DescriptorSetLayout,
        ground_layout: vk::DescriptorSetLayout,
        linear_sampler: vk::Sampler,
        params_buffer: vk::Buffer,
        params_stride: u32,
        tail_stride: u32,
        ripple_base: u32,
        ripple_stride: u32,
        _swapchain_generation: u64,
        depth_format: vk::Format,
    ) -> Result<(), RendererError> {
        let slots = FRAMES_IN_FLIGHT as usize;

        // --- the per-slot reflection targets (RGBA16F color + the
        // renderer's depth format — the HDR pass's attachments).
        //
        // The create_image closure takes an explicit aspect mask so it
        // can handle depth formats correctly (corrective finding 4).
        // It pushes each handle immediately after successful creation
        // to the partial bundle's vectors (corrective finding 8:
        // immediate registration to avoid leaks on partial failure).
        //
        // The diagnostic fault-injection seam (corrective findings 3+8)
        // checks before each fallible step. On the first matching fault,
        // returns RendererError::InjectedFault instead of proceeding.
        let create_image = |format: vk::Format,
                            usage: vk::ImageUsageFlags,
                            aspect: vk::ImageAspectFlags,
                            images: &mut Vec<vk::Image>,
                            memories: &mut Vec<vk::DeviceMemory>,
                            views: &mut Vec<vk::ImageView>,
                            what: &str|
         -> Result<(), RendererError> {
            let image_info = vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(format)
                .extent(vk::Extent3D {
                    width: extent.width,
                    height: extent.height,
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(usage)
                .sharing_mode(vk::SharingMode::EXCLUSIVE);
            // SAFETY: fresh object; push it to the partial bundle's
            // image vector immediately so release() can free it on error.
            // Fault check before image creation (site: Wet, stage: Image)
            ctx.check_target_fault(TargetFaultSite::Wet, TargetFaultStage::Image)?;
            let image = unsafe { ctx.create_target_image(&image_info, what) }?;
            images.push(image);
            let requirements = unsafe { ctx.device.get_image_memory_requirements(image) };
            // Fault check before memory type lookup (site: Wet, stage: MemoryLookup)
            ctx.check_target_fault(TargetFaultSite::Wet, TargetFaultStage::MemoryLookup)?;
            let memory_index = find_memory_type(
                ctx,
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .ok_or_else(|| RendererError::NoDevice("no DEVICE_LOCAL memory".into()))?;
            let memory_info = vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(memory_index);
            // SAFETY: push the memory immediately after allocation
            // (it's bound to the image we just pushed).
            // Fault check before memory allocation (site: Wet, stage: Memory)
            ctx.check_target_fault(TargetFaultSite::Wet, TargetFaultStage::Memory)?;
            let memory = unsafe { ctx.allocate_target_memory(&memory_info, what) }?;
            memories.push(memory);
            // Fault check before memory binding (site: Wet, stage: Bind)
            ctx.check_target_fault(TargetFaultSite::Wet, TargetFaultStage::Bind)?;
            unsafe { ctx.device.bind_image_memory(image, memory, 0) }
                .map_err(|error| RendererError::vk(error, what))?;
            let subresource = vk::ImageSubresourceRange {
                aspect_mask: aspect,
                level_count: 1,
                layer_count: 1,
                ..Default::default()
            };
            let view_info = vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
                .subresource_range(subresource);
            // SAFETY: push the view immediately after creation.
            // Fault check before view creation (site: Wet, stage: View)
            ctx.check_target_fault(TargetFaultSite::Wet, TargetFaultStage::View)?;
            let view = unsafe { ctx.create_target_image_view(&view_info, what) }?;
            views.push(view);
            Ok(())
        };
        let color_usage = vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED;
        for _slot in 0..slots {
            // Color images: aspect is COLOR.
            create_image(
                REFLECTION_FORMAT,
                color_usage,
                vk::ImageAspectFlags::COLOR,
                &mut self.color_images,
                &mut self.color_memories,
                &mut self.color_views,
                "reflection color image",
            )?;
            // The HDR depth attachment's format is the renderer's
            // negotiated depth format (corrective finding 4), passed
            // as `depth_format`.
            create_image(
                depth_format,
                vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
                super::depth_aspect(depth_format),
                &mut self.depth_images,
                &mut self.depth_memories,
                &mut self.depth_views,
                "reflection depth image",
            )?;
        }
        for slot in 0..slots {
            let attachments = [self.color_views[slot], self.depth_views[slot]];
            let info = vk::FramebufferCreateInfo::default()
                .render_pass(hdr_pass)
                .attachments(&attachments)
                .width(extent.width)
                .height(extent.height)
                .layers(1);
            // Fault check before framebuffer creation (site: Wet, stage: Framebuffer)
            ctx.check_target_fault(TargetFaultSite::Wet, TargetFaultStage::Framebuffer)?;
            // SAFETY: fresh object; the partial state owns it.
            let framebuffer =
                unsafe { ctx.create_target_framebuffer(&info, "reflection framebuffer") }?;
            self.framebuffers.push(framebuffer);
        }

        // --- the reflected lit pipeline's sets: material × slot
        // (binding 0 = the corner's own albedo + repeat sampler,
        // binding 2 = this state's mirrored-params regions, binding
        // 3 = the material tail — the shadow state's set shape).
        let combined = vk::DescriptorType::COMBINED_IMAGE_SAMPLER;
        let dynamic = vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC;
        let uniform = vk::DescriptorType::UNIFORM_BUFFER;
        let reflected_sizes = [combined, dynamic, uniform].map(|ty| vk::DescriptorPoolSize {
            ty,
            descriptor_count: 6,
        });
        // SAFETY: fresh pool; the sets reference the corner's
        // textures and this state's buffer, which share or outlive
        // this bundle's lifetime.
        let (reflected_pool, reflected_sets) =
            alloc_sets::<6>(ctx, reflected_layout, &reflected_sizes, 6)?;
        self.reflected_pool = reflected_pool;
        let material_count = corner.materials().len();
        for material in 0..material_count {
            let view = corner
                .albedo_view(material as u32)
                .ok_or_else(|| RendererError::Vulkan("corner material out of table".into()))?;
            let tail_offset =
                slots as u64 * u64::from(params_stride) + material as u64 * u64::from(tail_stride);
            for slot in 0..slots {
                let set = reflected_sets[material * slots + slot];
                let image = [vk::DescriptorImageInfo::default()
                    .image_view(view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                let buffer = [vk::DescriptorBufferInfo::default().buffer(params_buffer)];
                let frame = [buffer[0].range(size_of::<FrameParams>() as u64)];
                let material_params = [buffer[0].offset(tail_offset).range(16)];
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
                ];
                // SAFETY: fresh set from a live pool; every
                // referenced object shares or outlives this bundle.
                unsafe { ctx.device.update_descriptor_sets(&writes, &[]) };
                self.reflected_sets[material * slots + slot] = set;
            }
        }
        // --- the ground re-draw's per-slot sets: the reflection
        // view, the linear sampler, THE CORNER's own frame buffer
        // (the TRUE camera — the ground's own draw reads it), and
        // THIS state's ripple regions at binding 4 (the same buffer
        // the mirrored FrameParams live in — no new Vulkan object).
        let sampled = vk::DescriptorType::SAMPLED_IMAGE;
        let ground_sizes = [
            vk::DescriptorPoolSize {
                ty: sampled,
                descriptor_count: 2,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLER,
                descriptor_count: 2,
            },
            vk::DescriptorPoolSize {
                ty: dynamic,
                // Two dynamic bindings per set now (3 = the corner
                // frame, 4 = the ripple field) x 2 slots.
                descriptor_count: 4,
            },
        ];
        // SAFETY: as above.
        let (ground_pool, ground_sets) = alloc_sets::<2>(ctx, ground_layout, &ground_sizes, 2)?;
        self.ground_pool = ground_pool;
        for (slot, &color_view) in self.color_views.iter().enumerate() {
            let image = [vk::DescriptorImageInfo::default()
                .image_view(color_view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let sampler_info = [vk::DescriptorImageInfo::default().sampler(linear_sampler)];
            let buffer = [vk::DescriptorBufferInfo::default()
                .buffer(corner.frame_buffer())
                .range(size_of::<FrameParams>() as u64)];
            // The ripple region's base write: binding 4 points at
            // THIS state's params buffer at ripple_base (the
            // per-frame dynamic offset adds slot x ripple_stride —
            // binding 3's base offset is 0, this one's is
            // ripple_base, both in their own writes). The stride
            // contract the record-time dynamic offset and the
            // construction-time zeroing both rely on: at least the
            // packed uniform's size, a multiple of 16 (the align
            // discipline's power-of-two align over the 560-byte
            // block — 35 rows, the A19 bound row included).
            debug_assert!(ripple_stride >= (RIPPLE_UNIFORM_ROWS * 16) as u32);
            debug_assert!(ripple_stride.is_multiple_of(16));
            let ripple = [vk::DescriptorBufferInfo::default()
                .buffer(params_buffer)
                .offset(u64::from(ripple_base))
                .range((RIPPLE_UNIFORM_ROWS * 16) as u64)];
            let writes = [
                vk::WriteDescriptorSet::default()
                    .dst_set(ground_sets[slot])
                    .dst_binding(0)
                    .descriptor_type(sampled)
                    .image_info(&image),
                vk::WriteDescriptorSet::default()
                    .dst_set(ground_sets[slot])
                    .dst_binding(2)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .image_info(&sampler_info),
                vk::WriteDescriptorSet::default()
                    .dst_set(ground_sets[slot])
                    .dst_binding(3)
                    .descriptor_type(dynamic)
                    .buffer_info(&buffer),
                vk::WriteDescriptorSet::default()
                    .dst_set(ground_sets[slot])
                    .dst_binding(4)
                    .descriptor_type(dynamic)
                    .buffer_info(&ripple),
            ];
            // SAFETY: as above.
            unsafe { ctx.device.update_descriptor_sets(&writes, &[]) };
            self.ground_sets[slot] = ground_sets[slot];
        }
        Ok(())
    }

    /// Releases every handle exactly once (the bundle's own).
    fn release(&mut self, ctx: &VulkanContext) {
        // SAFETY: owned by this bundle; destroyed only while the
        // device is idle (the rebuild proof), after the pipelines.
        unsafe {
            for framebuffer in self.framebuffers.drain(..) {
                ctx.destroy_target_framebuffer(framebuffer);
            }
            ctx.device.destroy_descriptor_pool(self.ground_pool, None);
            ctx.device
                .destroy_descriptor_pool(self.reflected_pool, None);
            // Independent drains, not zips — the partial-construction
            // case leaves the vectors unbalanced (the finding-8 fault
            // points); a zip would destroy neither side.
            for memory in self.depth_memories.drain(..) {
                ctx.free_target_memory(memory);
            }
            for image in self.depth_images.drain(..) {
                ctx.destroy_target_image(image);
            }
            for view in self.depth_views.drain(..) {
                ctx.destroy_target_image_view(view);
            }
            for memory in self.color_memories.drain(..) {
                ctx.free_target_memory(memory);
            }
            for image in self.color_images.drain(..) {
                ctx.destroy_target_image(image);
            }
            for view in self.color_views.drain(..) {
                ctx.destroy_target_image_view(view);
            }
        }
    }
}

/// Allocates a pool and `N` sets from a given layout (the post
/// package's `create_pool_sets` shape, with the same allocation
/// counters).
fn alloc_sets<const N: usize>(
    ctx: &VulkanContext,
    layout: vk::DescriptorSetLayout,
    sizes: &[vk::DescriptorPoolSize],
    capacity: u32,
) -> Result<(vk::DescriptorPool, [vk::DescriptorSet; N]), RendererError> {
    // SAFETY: fresh pool; the sets reference the caller's live
    // layout; on allocation failure the pool is released before
    // the error escapes.
    unsafe {
        let pool = ctx
            .device
            .create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(capacity)
                    .pool_sizes(sizes),
                None,
            )
            .map_err(|error| RendererError::vk(error, "reflection descriptor pool"))?;
        ctx.descriptor_pools.set(ctx.descriptor_pools.get() + 1);
        match ctx.device.allocate_descriptor_sets(
            &vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(pool)
                .set_layouts(&[layout; N]),
        ) {
            Ok(sets) => {
                ctx.descriptor_sets
                    .set(ctx.descriptor_sets.get() + N as u64);
                Ok((pool, std::array::from_fn(|index| sets[index])))
            }
            Err(error) => {
                ctx.device.destroy_descriptor_pool(pool, None);
                Err(RendererError::vk(error, "reflection descriptor sets"))
            }
        }
    }
}
