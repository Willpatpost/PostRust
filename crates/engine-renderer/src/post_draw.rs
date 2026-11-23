//! The post-processing GPU resources (Era-1 package 6 — see
//! `docs/CYCLE6-ERA1-BRIEF.md`): the HDR render-target chain —
//! the scene pass into an offscreen R16G16B16A16_SFLOAT color
//! target (plus its depth attachment), the bloom chain over that
//! target (the bright-passed 1/4 box downsample, then the
//! separable 5-tap blur, ping-ponged between two 1/4-res mips),
//! and the post pass that composites and tonemaps into the
//! swapchain. THE architectural step of the Era-1 ladder: the
//! frame's presentation path changes from "render into the
//! swapchain" to "render into HDR, resolve into the swapchain".
//!
//! THE CHAIN (one frame, one command buffer, in order):
//!   1. the HDR scene pass — the sky, the corner mesh, the proxy
//!      population, the shadowed lit pipeline: exactly the draws
//!      the main pass records (the SAME descriptor sets, uniforms,
//!      and write_frame discipline — only the pipelines differ,
//!      bound to THIS pass's R16G16B16A16 attachments), ending
//!      with the color attachment transitioned to
//!      SHADER_READ_ONLY_OPTIMAL
//!   2. the bright downsample — bloom.wgsl's `fs_bright`, one
//!      fullscreen triangle into this slot's bloom_a (the 2×2
//!      bilinear footprint, post.rs :: downsample_texel +
//!      bright_pass_rgb)
//!   3. the horizontal blur — `fs_blur_h`, bloom_a → bloom_b
//!   4. the vertical blur — `fs_blur_v`, bloom_b → bloom_a (the
//!      final mip)
//!   5. the post pass — post.wgsl's `fs_main` composite over the
//!      HDR target and the final mip, into the acquired swapchain
//!      image, leaving it in PRESENT_SRC_KHR exactly as the main
//!      pass would (the capture path and the present machinery
//!      are untouched)
//!
//! ZERO-PER-FRAME-ALLOCATION DISCIPLINE: every object here is
//! created ONCE, lazily, at the first post frame
//! ([`PipelineState::ensure_post_resources`] — the region-buffer
//! precedent: the postless path pays nothing and its allocation
//! pins are untouched). The chain carries NO per-frame uniforms at
//! all — the PostParams block (the texel sizes and
//! `post.rs::BLOOM_STRENGTH`) is written once per target extent.
//! The extent-bound bundle (the images, framebuffers, and sets) is
//! rebuilt only when the swapchain's extent or view set changes —
//! which can only happen through a swapchain recreation, whose
//! `device_wait_idle` proves no in-flight submission reads the old
//! targets (the rebuild-safety proof; the shadow maps avoid this
//! entirely by running at a fixed extent, but the HDR target is
//! window-sized by nature).
//!
//! SYNCHRONIZATION: the HDR color, bloom_a, and bloom_b images
//! are per-frame-slot PING-PONGS (the shadow-map discipline): slot
//! S's chain overwrites only slot S's images, inside the frame
//! that first waited slot S's fence, so the previous frame's
//! in-flight reads are complete. Between the passes, each bloom
//! render pass carries external subpass dependencies ordering the
//! PREVIOUS pass's color writes (and its final layout transition)
//! before this pass's fragment-shader reads, and — for the passes
//! that write an image a prior pass READ (the vertical blur writes
//! bloom_a, which the horizontal blur read) — the prior reads
//! before this pass's color writes. The post pass carries the
//! read dependency for the scene and final-mip writes (chained
//! through the bloom dependencies) plus the main pass's
//! acquire-side dependency form for the swapchain image itself.
//! The swapchain image stays under the existing disciplines
//! untouched: the acquire semaphore waits at
//! COLOR_ATTACHMENT_OUTPUT, the per-image fence guards reuse, and
//! the capture path's PRESENT_SRC → TRANSFER_SRC transition runs
//! after the post pass exactly as it ran after the main pass.
//!
//! THE KNOWN-ANSWER PATH: the chain is textureLoad-exact (no
//! filtered fetches anywhere in the bloom passes; the post pass's
//! scene fetch is a 1:1 bilinear at pixel centers — a pure texel
//! read — and its bloom fetch is the bilinear upsample post.rs ::
//! bloom_at_pixel mirrors), so `tests/render_post.rs` compares
//! presented pixels against post.rs's chain over the same rays
//! (the 6-B oracle discipline, with the f16 storage quantization
//! and the hardware bilinear's accumulation order folded into the
//! documented tolerance).

use super::corner::FrameParams;
use super::proxy_draw::ProxyFrameUpload;
use super::shadow_draw::ShadowFrameParams;
use super::sky_draw::SkyFrameParams;
use super::{PipelineState, RendererError, VulkanContext, create_mapped_buffer, find_memory_type};
use crate::config::FRAMES_IN_FLIGHT;
use crate::post::BLOOM_STRENGTH;
use ash::vk;

/// The HDR scene target's color format: 16-bit float per channel —
/// the headroom above 1.0 the tonemap exists to resolve (the sun
/// disc deliberately exceeds 1.0 so it blooms). Required, not
/// negotiated, for this package's scope (core Vulkan; universally
/// supported as a color attachment).
const HDR_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;

/// The post pass's parameter block — post.wgsl's PostParams,
/// byte-identical (a vec4 then a scalar, WGSL uniform layout:
/// offset 0 and 16, size 32; the tail lanes pad).
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct PostParams {
    /// xy = the scene texel size (1/width, 1/height), zw = the
    /// bloom mip's (unused by the shader today — carried for the
    /// documented layout).
    pub texel: [f32; 4],
    pub bloom_strength: f32,
    pub _pad: [f32; 3],
}

const _: () = assert!(size_of::<PostParams>() == 32);

/// One post frame's recording inputs — the same values the
/// main-pass branch of `vulkan::render_frame` holds (the flat
/// render_frame precedent: the frame flow owns them, the chain
/// consumes them; grouping THIS side keeps the record call
/// readable without restructuring the caller).
pub(crate) struct PostFrameInputs<'a> {
    pub slot: usize,
    pub image_index: u32,
    pub extent: vk::Extent2D,
    /// The HDR scene pass's clear color — the SAME value the main
    /// pass would clear with (the corner/shadowed convention).
    pub clear_color: [f32; 4],
    pub corner_params: Option<&'a FrameParams>,
    pub proxy: Option<&'a ProxyFrameUpload<'a>>,
    pub proxy_sets: Option<[vk::DescriptorSet; 2]>,
    pub shadowed: Option<&'a ShadowFrameParams>,
    pub sky: Option<&'a SkyFrameParams>,
    /// The Era-1 wet reflection's ground re-draw lands inside THIS
    /// pass (the HDR one) after the mesh draws — the frame flow's
    /// mirrored-scene pass and barrier already preceded the scene
    /// pass; the belt check keeps a dry frame off it.
    pub wet: bool,
}

/// The extent-bound half of the post state: the per-slot HDR and
/// bloom images, every framebuffer (including the post pass's own
/// per-swapchain-image set), and the per-slot descriptor sets that
/// bind the images. Rebuilt whenever the swapchain's extent or
/// view set changes (the rebuild-safety proof is the module
/// header's — only a recreation can change either, and a
/// recreation idles the device first).
struct PostTargets {
    extent: (u32, u32),
    /// The first swapchain view the post framebuffers were built
    /// over — the view set's identity (recreation rebuilds every
    /// view, so one handle comparison catches a changed set).
    swapchain_view0: vk::ImageView,
    hdr_images: Vec<vk::Image>,
    hdr_memories: Vec<vk::DeviceMemory>,
    hdr_views: Vec<vk::ImageView>,
    hdr_depth_images: Vec<vk::Image>,
    hdr_depth_memories: Vec<vk::DeviceMemory>,
    hdr_depth_views: Vec<vk::ImageView>,
    bloom_a_images: Vec<vk::Image>,
    bloom_a_memories: Vec<vk::DeviceMemory>,
    bloom_a_views: Vec<vk::ImageView>,
    bloom_b_images: Vec<vk::Image>,
    bloom_b_memories: Vec<vk::DeviceMemory>,
    bloom_b_views: Vec<vk::ImageView>,
    /// Per frame slot (the HDR scene pass's target).
    hdr_framebuffers: Vec<vk::Framebuffer>,
    /// Per frame slot (the bright and vertical-blur passes write
    /// bloom_a; the horizontal blur writes bloom_b).
    bloom_a_framebuffers: Vec<vk::Framebuffer>,
    bloom_b_framebuffers: Vec<vk::Framebuffer>,
    /// Per swapchain image (the post pass's target).
    post_framebuffers: Vec<vk::Framebuffer>,
    /// The bloom chain's per-slot sets: bright (reads the HDR
    /// view), blur_h (reads bloom_a), blur_v (reads bloom_b) —
    /// one pool, the shared single-binding layout.
    bloom_pool: vk::DescriptorPool,
    bright_sets: [vk::DescriptorSet; 2],
    blur_h_sets: [vk::DescriptorSet; 2],
    blur_v_sets: [vk::DescriptorSet; 2],
    /// The post pass's per-slot sets (the HDR view, the final mip
    /// view, the linear sampler, the PostParams uniform).
    post_pool: vk::DescriptorPool,
    post_sets: [vk::DescriptorSet; 2],
}

/// The lazily-created post state, owned by [`super::PipelineState`]
/// (dies with the pipelines that read it; a replacement re-ensures
/// an identical set before installation — finding A).
pub(crate) struct PostResources {
    /// The HDR scene pass (RGBA16F color + depth; color's final
    /// layout SHADER_READ_ONLY_OPTIMAL — the bloom and post passes
    /// sample it).
    hdr_pass: vk::RenderPass,
    /// The post pass (the swapchain's color format, no depth; the
    /// composite + tonemap, leaving the image in PRESENT_SRC_KHR).
    post_pass: vk::RenderPass,
    /// The bloom passes (RGBA16F, no depth, load DONT_CARE — the
    /// fullscreen triangle overwrites every texel; final layout
    /// SHADER_READ_ONLY_OPTIMAL).
    bloom_pass: vk::RenderPass,
    /// The renderer's depth format (the HDR depth attachments
    /// share it — the corrective-F discipline).
    depth_format: vk::Format,
    /// The corner lit pipeline bound to the HDR pass (the same
    /// layout, shader, and state as the main one — the render pass
    /// differs, which is the compatibility boundary).
    lit_hdr: vk::Pipeline,
    /// The instanced-proxy pipeline bound to the HDR pass.
    proxy_hdr: vk::Pipeline,
    /// The shadowed lit pipeline bound to the HDR pass — created
    /// at the first SHADOWED post frame (the shadow state's own
    /// lazy existence precedes it; the frame flow ensures that
    /// order).
    shadowed_hdr: Option<vk::Pipeline>,
    /// The sky pipeline bound to the HDR pass — created at the
    /// first SKY post frame (the same ordering argument).
    sky_hdr: Option<vk::Pipeline>,
    bright_pipeline: vk::Pipeline,
    blur_h_pipeline: vk::Pipeline,
    blur_v_pipeline: vk::Pipeline,
    post_pipeline: vk::Pipeline,
    bloom_layout: vk::DescriptorSetLayout,
    post_layout: vk::DescriptorSetLayout,
    /// The pipeline layouts wrapping the descriptor layouts (the
    /// pipelines bind these; the sets bind the descriptor layouts).
    bloom_pipeline_layout: vk::PipelineLayout,
    post_pipeline_layout: vk::PipelineLayout,
    /// The linear, CLAMP_TO_EDGE sampler the post pass's bilinear
    /// bloom upsample reads through.
    sampler: vk::Sampler,
    params_buffer: vk::Buffer,
    params_memory: vk::DeviceMemory,
    params_mapped: *mut core::ffi::c_void,
    targets: Option<PostTargets>,
}

/// Creates one descriptor set layout (the extent-free half of the
/// `descriptors::create_sets` pattern — the post state's layouts
/// survive target rebuilds, so they are created once here).
fn create_layout(
    ctx: &VulkanContext,
    bindings: &[vk::DescriptorSetLayoutBinding<'_>],
) -> Result<vk::DescriptorSetLayout, RendererError> {
    // SAFETY: fresh object; the caller owns it from creation.
    unsafe {
        ctx.device.create_descriptor_set_layout(
            &vk::DescriptorSetLayoutCreateInfo::default().bindings(bindings),
            None,
        )
    }
    .map_err(|error| RendererError::vk(error, "post descriptor set layout"))
}

/// Allocates a pool and `N` sets FROM a given layout (the post
/// targets' sets are extent-bound; the layout is not — this is
/// `descriptors::create_sets` split at its seam, with the same
/// allocation counters).
fn create_pool_sets<const N: usize>(
    ctx: &VulkanContext,
    layout: vk::DescriptorSetLayout,
    sizes: &[vk::DescriptorPoolSize],
    capacity: u32,
) -> Result<(vk::DescriptorPool, [vk::DescriptorSet; N]), RendererError> {
    // SAFETY: fresh pool; the sets allocated from it reference the
    // caller's live layout; on allocation failure the pool is
    // released before the error escapes.
    unsafe {
        let pool = ctx
            .device
            .create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(capacity)
                    .pool_sizes(sizes),
                None,
            )
            .map_err(|error| RendererError::vk(error, "post descriptor pool"))?;
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
                Err(RendererError::vk(error, "post descriptor sets"))
            }
        }
    }
}

impl PostResources {
    /// Builds the extent-free half of the post state: the three
    /// render passes, the scene pipelines (the corner lit and
    /// proxy pipelines always — their layouts exist from
    /// construction; the shadowed and sky variants lazily), the
    /// bloom and post pipelines, the descriptor layouts, the
    /// sampler, and the PostParams uniform. Partial-construction
    /// safe: `release` frees exactly what exists.
    pub(super) fn new(
        ctx: &VulkanContext,
        swapchain_format: vk::Format,
        depth_format: vk::Format,
        lit_layout: vk::PipelineLayout,
        proxy_layout: vk::PipelineLayout,
    ) -> Result<Self, RendererError> {
        let mut partial = PostResources {
            hdr_pass: vk::RenderPass::null(),
            post_pass: vk::RenderPass::null(),
            bloom_pass: vk::RenderPass::null(),
            depth_format,
            lit_hdr: vk::Pipeline::null(),
            proxy_hdr: vk::Pipeline::null(),
            shadowed_hdr: None,
            sky_hdr: None,
            bright_pipeline: vk::Pipeline::null(),
            blur_h_pipeline: vk::Pipeline::null(),
            blur_v_pipeline: vk::Pipeline::null(),
            post_pipeline: vk::Pipeline::null(),
            bloom_layout: vk::DescriptorSetLayout::null(),
            post_layout: vk::DescriptorSetLayout::null(),
            bloom_pipeline_layout: vk::PipelineLayout::null(),
            post_pipeline_layout: vk::PipelineLayout::null(),
            sampler: vk::Sampler::null(),
            params_buffer: vk::Buffer::null(),
            params_memory: vk::DeviceMemory::null(),
            params_mapped: core::ptr::null_mut(),
            targets: None,
        };
        if let Err(error) = partial.build(ctx, swapchain_format, lit_layout, proxy_layout) {
            partial.release(ctx);
            return Err(error);
        }
        Ok(partial)
    }

    fn build(
        &mut self,
        ctx: &VulkanContext,
        swapchain_format: vk::Format,
        lit_layout: vk::PipelineLayout,
        proxy_layout: vk::PipelineLayout,
    ) -> Result<(), RendererError> {
        // --- the HDR scene render pass: RGBA16F color (final
        // layout SHADER_READ — the bloom/post passes sample it) +
        // the renderer's own depth format. The external dependency
        // is the main pass's own form (the color/depth write
        // availability barrier — consistent with the pass it
        // replaces; the acquire's COLOR_ATTACHMENT_OUTPUT wait
        // covers it either way).
        let color_attachment = vk::AttachmentDescription::default()
            .format(HDR_FORMAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        let depth_attachment = vk::AttachmentDescription::default()
            .format(self.depth_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
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
        // SAFETY: fresh object; the partial state owns it from
        // creation and `release` frees it exactly once.
        self.hdr_pass = unsafe { ctx.device.create_render_pass(&render_pass_info, None) }
            .map_err(|error| RendererError::vk(error, "post HDR render pass"))?;

        // --- the bloom render pass: one RGBA16F attachment (load
        // DONT_CARE — the fullscreen triangle overwrites every
        // texel; final layout SHADER_READ). TWO external
        // dependencies: the prior pass's color writes (and its
        // final layout transition) happen-before this pass's
        // fragment reads, and the prior READS happen-before this
        // pass's color writes (the vertical blur writes the image
        // the horizontal blur just read).
        let bloom_attachment = vk::AttachmentDescription::default()
            .format(HDR_FORMAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
            .store_op(vk::AttachmentStoreOp::STORE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        let bloom_attachments = [bloom_attachment];
        let bloom_color_reference = vk::AttachmentReference {
            attachment: 0,
            layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        };
        let bloom_color_references = [bloom_color_reference];
        let bloom_subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&bloom_color_references);
        let read_dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags::SHADER_READ);
        let write_dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
            .src_access_mask(vk::AccessFlags::SHADER_READ)
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);
        let bloom_subpasses = [bloom_subpass];
        let bloom_dependencies = [read_dependency, write_dependency];
        let bloom_pass_info = vk::RenderPassCreateInfo::default()
            .attachments(&bloom_attachments)
            .subpasses(&bloom_subpasses)
            .dependencies(&bloom_dependencies);
        // SAFETY: as above.
        self.bloom_pass = unsafe { ctx.device.create_render_pass(&bloom_pass_info, None) }
            .map_err(|error| RendererError::vk(error, "post bloom render pass"))?;

        // --- the post render pass: the swapchain's color format,
        // no depth, load DONT_CARE (the fullscreen triangle
        // overwrites), final layout PRESENT_SRC_KHR — the image
        // leaves the frame exactly as the main pass leaves it (the
        // capture path's PRESENT_SRC → TRANSFER_SRC transition and
        // the present machinery are untouched). The dependencies:
        // the composite's fragment reads of the HDR target and the
        // final mip (ordered behind every prior pass's writes by
        // the chained bloom dependencies), plus the main pass's
        // acquire-side form for the swapchain image itself.
        let post_attachment = vk::AttachmentDescription::default()
            .format(swapchain_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
            .store_op(vk::AttachmentStoreOp::STORE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);
        let post_attachments = [post_attachment];
        let post_color_reference = vk::AttachmentReference {
            attachment: 0,
            layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        };
        let post_color_references = [post_color_reference];
        let post_subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&post_color_references);
        let post_read_dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags::SHADER_READ);
        let post_acquire_dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags::empty())
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);
        let post_subpasses = [post_subpass];
        let post_dependencies = [post_read_dependency, post_acquire_dependency];
        let post_pass_info = vk::RenderPassCreateInfo::default()
            .attachments(&post_attachments)
            .subpasses(&post_subpasses)
            .dependencies(&post_dependencies);
        // SAFETY: as above.
        self.post_pass = unsafe { ctx.device.create_render_pass(&post_pass_info, None) }
            .map_err(|error| RendererError::vk(error, "post render pass"))?;

        // --- the PostParams uniform (written once per extent at
        // target build; NO per-frame writes — the chain's only
        // uniform is extent-static).
        let (params_buffer, params_memory, params_mapped) = create_mapped_buffer(
            ctx,
            vk::BufferUsageFlags::UNIFORM_BUFFER,
            size_of::<PostParams>() as u64,
        )?;
        self.params_buffer = params_buffer;
        self.params_memory = params_memory;
        self.params_mapped = params_mapped;

        // --- the linear sampler (the post pass's bilinear bloom
        // upsample; CLAMP_TO_EDGE so the sub-divisible extent tail
        // and the borders read the edge texels).
        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);
        // SAFETY: one live device owns this sampler until the
        // state's teardown.
        self.sampler = unsafe { ctx.device.create_sampler(&sampler_info, None) }
            .map_err(|error| RendererError::vk(error, "post sampler"))?;
        // SAFETY: the allocation counter is the context's own
        // (every sampler creation increments it — the textures
        // module's discipline).
        ctx.sampler_creations.set(ctx.sampler_creations.get() + 1);

        // --- the descriptor layouts. The bloom chain's: one
        // sampled image (the source of whichever pass binds it).
        // The post pass's: post.wgsl's finalized map — 0 the HDR
        // scene, 1 the final bloom mip, 2 the sampler, 3 the
        // PostParams uniform.
        let sampled = vk::DescriptorType::SAMPLED_IMAGE;
        let frag = vk::ShaderStageFlags::FRAGMENT;
        let bloom_bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(sampled)
            .descriptor_count(1)
            .stage_flags(frag)];
        // SAFETY: fresh object; the partial state owns it.
        self.bloom_layout = create_layout(ctx, &bloom_bindings)?;
        let post_bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(sampled)
                .descriptor_count(1)
                .stage_flags(frag),
            // Binding 4 — NOT 1 — for the mip: post.wgsl's header
            // documents the build's binding-map remap (WGSL binding
            // 1 compiles onto descriptor 0, the 6-A atlas share —
            // a binding-1 mip would alias the scene target).
            vk::DescriptorSetLayoutBinding::default()
                .binding(4)
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
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(frag),
        ];
        // SAFETY: as above.
        self.post_layout = create_layout(ctx, &post_bindings)?;
        // The pipeline layouts wrapping them (the bloom and post
        // pipelines bind these).
        let layouts = [self.bloom_layout];
        // SAFETY: the descriptor layout is owned above; the partial
        // state owns the pipeline layout immediately.
        self.bloom_pipeline_layout = unsafe {
            ctx.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts),
                None,
            )
        }
        .map_err(|error| RendererError::vk(error, "post bloom pipeline layout"))?;
        let layouts = [self.post_layout];
        // SAFETY: as above.
        self.post_pipeline_layout = unsafe {
            ctx.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts),
                None,
            )
        }
        .map_err(|error| RendererError::vk(error, "post pipeline layout"))?;

        // --- the pipelines. The scene variants mirror the main
        // pass's exactly (the same shader modules, vertex inputs,
        // rasterizers, depth, and blend — only the bound render
        // pass and layout owner differ, which is the whole
        // compatibility boundary).
        let lit_module = super::create_shader_module(
            ctx,
            include_bytes!(concat!(env!("OUT_DIR"), "/lit_scene.spv")),
        )?;
        let lit_result = Self::scene_pipeline(
            ctx,
            lit_module,
            lit_layout,
            self.hdr_pass,
            "post lit hdr pipeline",
            false,
        );
        // SAFETY: consumed by the pipeline above (or failed with
        // none alive); destroyed exactly once.
        unsafe { ctx.device.destroy_shader_module(lit_module, None) };
        self.lit_hdr = lit_result?;

        let proxy_module = super::create_shader_module(
            ctx,
            include_bytes!(concat!(env!("OUT_DIR"), "/instanced_proxy.spv")),
        )?;
        let proxy_result = Self::scene_pipeline(
            ctx,
            proxy_module,
            proxy_layout,
            self.hdr_pass,
            "post proxy hdr pipeline",
            true,
        );
        // SAFETY: as above.
        unsafe { ctx.device.destroy_shader_module(proxy_module, None) };
        self.proxy_hdr = proxy_result?;

        // --- the bloom and post pipelines (the fullscreen-
        // triangle form — no vertex input, no depth, no culling,
        // dynamic viewport/scissor; the sky pipeline's own shape).
        let bloom_module = super::create_shader_module(
            ctx,
            include_bytes!(concat!(env!("OUT_DIR"), "/bloom.spv")),
        )?;
        let bloom_result: Result<(), RendererError> = (|| {
            for (entry, which) in [(c"fs_bright", 0usize), (c"fs_blur_h", 1), (c"fs_blur_v", 2)] {
                let pipeline = Self::fullscreen_pipeline(
                    ctx,
                    bloom_module,
                    entry,
                    self.bloom_pipeline_layout,
                    self.bloom_pass,
                    "post bloom pipeline",
                )?;
                match which {
                    0 => self.bright_pipeline = pipeline,
                    1 => self.blur_h_pipeline = pipeline,
                    _ => self.blur_v_pipeline = pipeline,
                }
            }
            Ok(())
        })();
        // SAFETY: as above.
        unsafe { ctx.device.destroy_shader_module(bloom_module, None) };
        bloom_result?;

        let post_module = super::create_shader_module(
            ctx,
            include_bytes!(concat!(env!("OUT_DIR"), "/post.spv")),
        )?;
        let post_result = Self::fullscreen_pipeline(
            ctx,
            post_module,
            c"fs_main",
            self.post_pipeline_layout,
            self.post_pass,
            "post composite pipeline",
        );
        // SAFETY: as above.
        unsafe { ctx.device.destroy_shader_module(post_module, None) };
        self.post_pipeline = post_result?;

        Ok(())
    }

    /// The corner lit / proxy scene pipeline against the HDR pass —
    /// the exact config `PipelineState::assemble_pipelines`
    /// builds (the same vertex input, the lit CCW front face, the
    /// LESS depth, the plain color write), parameterized by the
    /// shader module and layout. `proxy` selects the two-binding
    /// instance-rate vertex input.
    fn scene_pipeline(
        ctx: &VulkanContext,
        module: vk::ShaderModule,
        layout: vk::PipelineLayout,
        render_pass: vk::RenderPass,
        what: &str,
        proxy: bool,
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
        use crate::proxy_mesh::ProxyVertex;
        let mesh_binding = vk::VertexInputBindingDescription {
            binding: 0,
            stride: if proxy {
                size_of::<ProxyVertex>() as u32
            } else {
                size_of::<LitVertex>() as u32
            },
            input_rate: vk::VertexInputRate::VERTEX,
        };
        let base_attributes = [
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
                offset: if proxy {
                    std::mem::offset_of!(ProxyVertex, uv) as u32
                } else {
                    std::mem::offset_of!(LitVertex, uv) as u32
                },
            },
        ];
        // The 6-E instance-rate binding for the proxy variant: the
        // packed transform + tint columns (the
        // proxy_instances::pack layout).
        let instance_binding = vk::VertexInputBindingDescription {
            binding: 1,
            stride: crate::proxy_instances::INSTANCE_STRIDE as u32,
            input_rate: vk::VertexInputRate::INSTANCE,
        };
        let vec4 = vk::Format::R32G32B32A32_SFLOAT;
        let instance_attributes =
            [3, 4, 5, 6, 7].map(|location| vk::VertexInputAttributeDescription {
                binding: 1,
                location,
                format: vec4,
                offset: (location - 3) * 16,
            });
        let (bindings, attributes): (&[_], &[_]) = if proxy {
            (
                &[mesh_binding, instance_binding],
                &[
                    base_attributes[0],
                    base_attributes[1],
                    base_attributes[2],
                    instance_attributes[0],
                    instance_attributes[1],
                    instance_attributes[2],
                    instance_attributes[3],
                    instance_attributes[4],
                ],
            )
        } else {
            (&[mesh_binding], &base_attributes)
        };
        let inputs = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(bindings)
            .vertex_attribute_descriptions(attributes);
        let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        // The corner and proxy meshes wind CCW about their outward
        // normals (the lit convention).
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

    /// The shadowed lit pipeline against the HDR pass (created at
    /// the first shadowed post frame; the shadow state — ensured
    /// before the post ensure by the frame flow — owns the layout).
    pub(super) fn ensure_shadowed_hdr(
        &mut self,
        ctx: &VulkanContext,
        shadowed_layout: vk::PipelineLayout,
    ) -> Result<(), RendererError> {
        if self.shadowed_hdr.is_some() {
            return Ok(());
        }
        let module = super::create_shader_module(
            ctx,
            include_bytes!(concat!(env!("OUT_DIR"), "/lit_scene_shadowed.spv")),
        )?;
        let result = Self::scene_pipeline(
            ctx,
            module,
            shadowed_layout,
            self.hdr_pass,
            "post shadowed hdr pipeline",
            false,
        );
        // SAFETY: consumed by the pipeline above (or failed with
        // none alive); destroyed exactly once.
        unsafe { ctx.device.destroy_shader_module(module, None) };
        self.shadowed_hdr = Some(result?);
        Ok(())
    }

    /// The sky pipeline against the HDR pass (created at the first
    /// sky post frame; the sky state owns the layout).
    pub(super) fn ensure_sky_hdr(
        &mut self,
        ctx: &VulkanContext,
        sky_layout: vk::PipelineLayout,
    ) -> Result<(), RendererError> {
        if self.sky_hdr.is_some() {
            return Ok(());
        }
        let module =
            super::create_shader_module(ctx, include_bytes!(concat!(env!("OUT_DIR"), "/sky.spv")))?;
        // The sky pipeline's own shape: no vertex input, NO depth
        // test and NO write, no culling (the fullscreen triangle).
        let result = Self::fullscreen_pipeline(
            ctx,
            module,
            c"fs_main",
            sky_layout,
            self.hdr_pass,
            "post sky hdr pipeline",
        );
        // SAFETY: consumed by the pipeline above (or failed with
        // none alive); destroyed exactly once.
        unsafe { ctx.device.destroy_shader_module(module, None) };
        self.sky_hdr = Some(result?);
        Ok(())
    }

    /// A fullscreen-triangle pipeline (the bloom and post passes'
    /// shape — no vertex input, no depth, no culling, dynamic
    /// viewport/scissor).
    fn fullscreen_pipeline(
        ctx: &VulkanContext,
        module: vk::ShaderModule,
        fragment_entry: &core::ffi::CStr,
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
                .name(fragment_entry),
        ];
        let inputs = vk::PipelineVertexInputStateCreateInfo::default();
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
            .depth_test_enable(false)
            .depth_write_enable(false);
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

    /// The HDR scene render pass object (the wet reflection's pass
    /// rides it — the attachment descriptions are identical, so the
    /// reflection framebuffers bind this pass directly).
    pub(super) fn hdr_render_pass(&self) -> vk::RenderPass {
        self.hdr_pass
    }

    /// Builds (or rebuilds) the extent-bound target bundle when the
    /// swapchain's extent or view set changed — a no-op when they
    /// match. SAFETY ARGUMENT (the rebuild proof): the extent and
    /// the swapchain views change ONLY through a swapchain
    /// recreation, and every recreation path in this renderer
    /// begins with `device_wait_idle` — so at the moment an
    /// existing bundle is released, no in-flight submission can
    /// still be reading or writing its images. The FIRST build has
    /// nothing in flight by definition (the state itself was just
    /// created). Called outside command recording.
    pub(super) fn ensure_targets(
        &mut self,
        ctx: &VulkanContext,
        extent: vk::Extent2D,
        swapchain_views: &[vk::ImageView],
    ) -> Result<(), RendererError> {
        let first_view = swapchain_views.first().copied().ok_or_else(|| {
            RendererError::Vulkan("the post chain requires swapchain views".into())
        })?;
        if let Some(targets) = &self.targets
            && targets.extent == (extent.width, extent.height)
            && targets.swapchain_view0 == first_view
        {
            return Ok(());
        }
        let mut old = self.targets.take();
        if let Some(old) = &mut old {
            old.release(ctx);
        }
        self.targets = Some(PostTargets::build(
            ctx,
            extent,
            first_view,
            swapchain_views,
            self.hdr_pass,
            self.bloom_pass,
            self.post_pass,
            self.bloom_layout,
            self.post_layout,
            self.sampler,
            self.depth_format,
            self.params_buffer,
            self.params_mapped,
        )?);
        Ok(())
    }

    /// Records the whole post chain for this frame — the five
    /// passes of the module header, replacing the main render pass
    /// (the postless frames never enter here; their path is
    /// byte-identical to the pre-package renderer). Returns the
    /// proxy draw calls recorded (the main-pass branch's own
    /// contract).
    ///
    /// # Safety
    /// `cmd` must be recording (past the shadow depth pass, before
    /// the capture/timestamp tail); `slot`'s fence wait passed (the
    /// sky/corner uniform regions this writes are disjoint per
    /// slot); the targets match the CURRENT swapchain (the frame
    /// flow ensured them after the acquire); `image_index` is the
    /// acquired image.
    pub(super) unsafe fn record(
        &self,
        ctx: &VulkanContext,
        pipes: &PipelineState,
        cmd: vk::CommandBuffer,
        inputs: &PostFrameInputs<'_>,
    ) -> u32 {
        let targets = self
            .targets
            .as_ref()
            .expect("the frame flow ensures the targets before recording");
        let slot = inputs.slot;
        let clear = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: inputs.clear_color,
                },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        ];
        let full_viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: inputs.extent.width as f32,
            height: inputs.extent.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let full_scissor = vk::Rect2D {
            extent: inputs.extent,
            offset: vk::Offset2D { x: 0, y: 0 },
        };
        let bloom_extent = crate::post::bloom_extent((inputs.extent.width, inputs.extent.height));
        let bloom_viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: bloom_extent.0 as f32,
            height: bloom_extent.1 as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let bloom_scissor = vk::Rect2D {
            extent: vk::Extent2D {
                width: bloom_extent.0,
                height: bloom_extent.1,
            },
            offset: vk::Offset2D { x: 0, y: 0 },
        };
        let mut proxy_calls = 0;

        // 1. the HDR scene pass — the main pass's draw sequence,
        //    verbatim, through the HDR pipelines.
        // SAFETY: this slot's framebuffer and the per-slot images;
        // the draw states are the same disciplines the main pass
        // records (each write_frame happens after the slot's fence
        // wait — the frame flow's own proof).
        unsafe {
            ctx.device.cmd_begin_render_pass(
                cmd,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.hdr_pass)
                    .framebuffer(targets.hdr_framebuffers[slot])
                    .render_area(full_scissor)
                    .clear_values(&clear),
                vk::SubpassContents::INLINE,
            );
            ctx.device.cmd_set_viewport(cmd, 0, &[full_viewport]);
            ctx.device.cmd_set_scissor(cmd, 0, &[full_scissor]);
            if let (Some(sky_state), Some(params), Some(pipeline)) =
                (&pipes.sky, inputs.sky, self.sky_hdr)
            {
                // SAFETY: this slot's fence wait passed above (the
                // sky uniform region is disjoint per slot).
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
            if inputs.shadowed.is_some()
                && let (Some(resources), Some(shadow_state), Some(pipeline)) =
                    (&ctx.corner, &pipes.shadows, self.shadowed_hdr)
            {
                // The shadowed corner draws: the shadow state's own
                // per-material sets and frame offset, the HDR
                // pipeline (the main branch's shape — the extended
                // FrameParams were written at the depth pass above).
                let (_, layout, sets, frame_offset) = shadow_state.shadowed(slot);
                ctx.device
                    .cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
                resources.draw(ctx, cmd, layout, &sets, frame_offset);
            } else if let (Some(resources), Some(params)) = (&ctx.corner, inputs.corner_params) {
                ctx.device
                    .cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.lit_hdr);
                // SAFETY: this slot's fence wait passed above; the
                // coherent frame regions do not overlap other slots
                // or the immutable material tail.
                resources.write_frame(slot, params);
                resources.draw(
                    ctx,
                    cmd,
                    pipes.lit_layout,
                    resources.sets(),
                    slot as u32 * resources.frame_stride(),
                );
                // The 6-E instanced-proxy draws, after the static
                // corner mesh (the depth-ordering note of the main
                // branch, unchanged).
                if let (Some(proxy_state), Some(upload), Some(sets)) =
                    (&pipes.proxy, inputs.proxy, inputs.proxy_sets)
                {
                    ctx.device.cmd_bind_pipeline(
                        cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        self.proxy_hdr,
                    );
                    // SAFETY: the proxy pipeline is bound with its
                    // own layout inside its render pass; `sets` were
                    // validated before recording; this slot's
                    // instance region holds this frame's bytes
                    // (written above, after the fence wait).
                    let recorded = proxy_state.draw(
                        ctx,
                        cmd,
                        pipes.proxy_layout,
                        resources,
                        slot,
                        upload,
                        sets,
                    );
                    debug_assert_eq!(
                        recorded,
                        super::proxy_draw::proxy_draw_call_count(
                            upload.mode,
                            upload.npc_count,
                            upload.vehicle_count
                        )
                    );
                    proxy_calls = recorded;
                }
            }
            // The Era-1 wet reflection's ground re-draw (package 7):
            // inside the HDR scene pass, after the mesh draws — the
            // ground quad re-rasterized (EQUAL depth) with the
            // reflection sampled at the same screen position and the
            // Schlick blend on the alpha stage. The mirrored-scene
            // pass and its barrier preceded this pass (the frame
            // flow's recording order).
            // SAFETY: the command buffer is inside the HDR scene
            // pass, past the mesh draws; this slot's corner
            // FrameParams region holds the true camera (the frame
            // flow's write).
            if inputs.wet
                && let (Some(reflection_state), Some(corner)) = (&pipes.reflection, &ctx.corner)
            {
                reflection_state.record_ground_redraw(ctx, corner, cmd, slot, true);
            }
            ctx.device.cmd_end_render_pass(cmd);
        }

        // 2–4. the bloom chain: bright (hdr → bloom_a), blur H
        //      (bloom_a → bloom_b), blur V (bloom_b → bloom_a, the
        //      final mip). One render pass, three framebuffers.
        // SAFETY: the per-slot targets and sets; the passes'
        // external dependencies order the writes and reads (the
        // module header's synchronization notes).
        unsafe {
            let passes = [
                (
                    self.bright_pipeline,
                    targets.bright_sets[slot],
                    targets.bloom_a_framebuffers[slot],
                ),
                (
                    self.blur_h_pipeline,
                    targets.blur_h_sets[slot],
                    targets.bloom_b_framebuffers[slot],
                ),
                (
                    self.blur_v_pipeline,
                    targets.blur_v_sets[slot],
                    targets.bloom_a_framebuffers[slot],
                ),
            ];
            for (pipeline, set, framebuffer) in passes {
                ctx.device.cmd_begin_render_pass(
                    cmd,
                    &vk::RenderPassBeginInfo::default()
                        .render_pass(self.bloom_pass)
                        .framebuffer(framebuffer)
                        .render_area(bloom_scissor)
                        .clear_values(&[]),
                    vk::SubpassContents::INLINE,
                );
                ctx.device.cmd_set_viewport(cmd, 0, &[bloom_viewport]);
                ctx.device.cmd_set_scissor(cmd, 0, &[bloom_scissor]);
                ctx.device
                    .cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
                ctx.device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.bloom_pipeline_layout,
                    0,
                    &[set],
                    &[],
                );
                ctx.device.cmd_draw(cmd, 3, 1, 0, 0);
                ctx.device.cmd_end_render_pass(cmd);
            }
        }

        // 5. the post pass — the composite + tonemap into the
        //    acquired swapchain image, leaving it in PRESENT_SRC.
        // SAFETY: the framebuffer is this frame's acquired image's;
        // the set binds this slot's HDR and final-mip views; the
        // pass's dependencies order the scene and blur writes
        // before the composite's reads, and the acquire semaphore
        // (waited at COLOR_ATTACHMENT_OUTPUT by the submission)
        // covers the swapchain image.
        unsafe {
            ctx.device.cmd_begin_render_pass(
                cmd,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.post_pass)
                    .framebuffer(targets.post_framebuffers[inputs.image_index as usize])
                    .render_area(full_scissor)
                    .clear_values(&[]),
                vk::SubpassContents::INLINE,
            );
            ctx.device.cmd_set_viewport(cmd, 0, &[full_viewport]);
            ctx.device.cmd_set_scissor(cmd, 0, &[full_scissor]);
            ctx.device
                .cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.post_pipeline);
            ctx.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.post_pipeline_layout,
                0,
                &[targets.post_sets[slot]],
                &[],
            );
            ctx.device.cmd_draw(cmd, 3, 1, 0, 0);
            ctx.device.cmd_end_render_pass(cmd);
        }
        proxy_calls
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
            ctx.device.destroy_sampler(self.sampler, None);
            ctx.device.destroy_pipeline(self.post_pipeline, None);
            ctx.device.destroy_pipeline(self.blur_v_pipeline, None);
            ctx.device.destroy_pipeline(self.blur_h_pipeline, None);
            ctx.device.destroy_pipeline(self.bright_pipeline, None);
            if let Some(pipeline) = self.sky_hdr.take() {
                ctx.device.destroy_pipeline(pipeline, None);
            }
            if let Some(pipeline) = self.shadowed_hdr.take() {
                ctx.device.destroy_pipeline(pipeline, None);
            }
            ctx.device.destroy_pipeline(self.proxy_hdr, None);
            ctx.device.destroy_pipeline(self.lit_hdr, None);
            // Framebuffers before the render passes they reference.
            ctx.device.destroy_render_pass(self.post_pass, None);
            ctx.device.destroy_render_pass(self.bloom_pass, None);
            ctx.device.destroy_render_pass(self.hdr_pass, None);
            ctx.device
                .destroy_descriptor_set_layout(self.post_layout, None);
            ctx.device
                .destroy_descriptor_set_layout(self.bloom_layout, None);
            // The pipeline layouts die with the pipelines they
            // feed (after them — the vkDestroyDevice object-tracking
            // check that caught this order the first execution).
            ctx.device
                .destroy_pipeline_layout(self.bloom_pipeline_layout, None);
            ctx.device
                .destroy_pipeline_layout(self.post_pipeline_layout, None);
        }
    }
}

impl PostTargets {
    /// Builds the whole extent-bound bundle. Partial-construction
    /// safe: `release` frees exactly what exists.
    #[allow(clippy::too_many_arguments)]
    fn build(
        ctx: &VulkanContext,
        extent: vk::Extent2D,
        first_view: vk::ImageView,
        swapchain_views: &[vk::ImageView],
        hdr_pass: vk::RenderPass,
        bloom_pass: vk::RenderPass,
        post_pass: vk::RenderPass,
        bloom_layout: vk::DescriptorSetLayout,
        post_layout: vk::DescriptorSetLayout,
        sampler: vk::Sampler,
        depth_format: vk::Format,
        params_buffer: vk::Buffer,
        params_mapped: *mut core::ffi::c_void,
    ) -> Result<PostTargets, RendererError> {
        let mut partial = PostTargets {
            extent: (extent.width, extent.height),
            swapchain_view0: first_view,
            hdr_images: Vec::new(),
            hdr_memories: Vec::new(),
            hdr_views: Vec::new(),
            hdr_depth_images: Vec::new(),
            hdr_depth_memories: Vec::new(),
            hdr_depth_views: Vec::new(),
            bloom_a_images: Vec::new(),
            bloom_a_memories: Vec::new(),
            bloom_a_views: Vec::new(),
            bloom_b_images: Vec::new(),
            bloom_b_memories: Vec::new(),
            bloom_b_views: Vec::new(),
            hdr_framebuffers: Vec::new(),
            bloom_a_framebuffers: Vec::new(),
            bloom_b_framebuffers: Vec::new(),
            post_framebuffers: Vec::new(),
            bloom_pool: vk::DescriptorPool::null(),
            bright_sets: [vk::DescriptorSet::null(); 2],
            blur_h_sets: [vk::DescriptorSet::null(); 2],
            blur_v_sets: [vk::DescriptorSet::null(); 2],
            post_pool: vk::DescriptorPool::null(),
            post_sets: [vk::DescriptorSet::null(); 2],
        };
        if let Err(error) = partial.build_targets(
            ctx,
            extent,
            swapchain_views,
            hdr_pass,
            bloom_pass,
            post_pass,
            bloom_layout,
            post_layout,
            sampler,
            depth_format,
            params_buffer,
            params_mapped,
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
        swapchain_views: &[vk::ImageView],
        hdr_pass: vk::RenderPass,
        bloom_pass: vk::RenderPass,
        post_pass: vk::RenderPass,
        bloom_layout: vk::DescriptorSetLayout,
        post_layout: vk::DescriptorSetLayout,
        sampler: vk::Sampler,
        depth_format: vk::Format,
        params_buffer: vk::Buffer,
        params_mapped: *mut core::ffi::c_void,
    ) -> Result<(), RendererError> {
        let slots = FRAMES_IN_FLIGHT as usize;
        let scene_extent = (extent.width, extent.height);
        let bloom_extent = crate::post::bloom_extent(scene_extent);

        // The PostParams block, written once for this extent (no
        // per-frame uniforms anywhere in the chain).
        // SAFETY: the coherent mapping is written before any
        // submission that reads it (the sets below bind it).
        unsafe {
            params_mapped.cast::<PostParams>().write(PostParams {
                texel: [
                    1.0 / scene_extent.0 as f32,
                    1.0 / scene_extent.1 as f32,
                    1.0 / bloom_extent.0 as f32,
                    1.0 / bloom_extent.1 as f32,
                ],
                bloom_strength: BLOOM_STRENGTH,
                _pad: [0.0; 3],
            });
        }

        // --- the per-slot images: the HDR color target + its depth
        // attachment at the scene extent, the two bloom mips at
        // the 1/4 extent (the shadow-map creation pattern).
        let create_image =
            |format: vk::Format,
             image_extent: (u32, u32),
             usage: vk::ImageUsageFlags,
             what: &str|
             -> Result<(vk::Image, vk::DeviceMemory, vk::ImageView), RendererError> {
                let image_info = vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(format)
                    .extent(vk::Extent3D {
                        width: image_extent.0,
                        height: image_extent.1,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(usage)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE);
                // SAFETY: fresh object; the partial state owns it from
                // creation.
                let image = unsafe { ctx.device.create_image(&image_info, None) }
                    .map_err(|error| RendererError::vk(error, what))?;
                let requirements = unsafe { ctx.device.get_image_memory_requirements(image) };
                let memory_index = find_memory_type(
                    ctx,
                    requirements.memory_type_bits,
                    vk::MemoryPropertyFlags::DEVICE_LOCAL,
                )
                .ok_or_else(|| RendererError::NoDevice("no DEVICE_LOCAL memory".into()))?;
                let memory_info = vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(memory_index);
                // SAFETY: the requirement query is for this live image;
                // the partial state owns both handles.
                let memory = unsafe { ctx.allocate_memory(&memory_info) }
                    .map_err(|error| RendererError::vk(error, what))?;
                // SAFETY: as above.
                unsafe { ctx.device.bind_image_memory(image, memory, 0) }
                    .map_err(|error| RendererError::vk(error, what))?;
                let subresource = vk::ImageSubresourceRange {
                    aspect_mask: if format == vk::Format::D32_SFLOAT {
                        vk::ImageAspectFlags::DEPTH
                    } else {
                        vk::ImageAspectFlags::COLOR
                    },
                    level_count: 1,
                    layer_count: 1,
                    ..Default::default()
                };
                let view_info = vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format)
                    .subresource_range(subresource);
                // SAFETY: as above.
                let view = unsafe { ctx.device.create_image_view(&view_info, None) }
                    .map_err(|error| RendererError::vk(error, what))?;
                Ok((image, memory, view))
            };
        let color_usage = vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED;
        for _slot in 0..slots {
            let (image, memory, view) =
                create_image(HDR_FORMAT, scene_extent, color_usage, "post HDR image")?;
            self.hdr_images.push(image);
            self.hdr_memories.push(memory);
            self.hdr_views.push(view);
            let (image, memory, view) =
                create_image(HDR_FORMAT, bloom_extent, color_usage, "post bloom_a image")?;
            self.bloom_a_images.push(image);
            self.bloom_a_memories.push(memory);
            self.bloom_a_views.push(view);
            let (image, memory, view) =
                create_image(HDR_FORMAT, bloom_extent, color_usage, "post bloom_b image")?;
            self.bloom_b_images.push(image);
            self.bloom_b_memories.push(memory);
            self.bloom_b_views.push(view);
        }
        // The depth attachments: the renderer's own depth format,
        // DEPTH_STENCIL_ATTACHMENT usage only (nothing reads it
        // after the pass).
        for _slot in 0..slots {
            let (image, memory, view) = create_image(
                depth_format,
                scene_extent,
                vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
                "post HDR depth image",
            )?;
            self.hdr_depth_images.push(image);
            self.hdr_depth_memories.push(memory);
            self.hdr_depth_views.push(view);
        }
        // The HDR scene framebuffers: color + depth per slot.
        for slot in 0..slots {
            let attachments = [self.hdr_views[slot], self.hdr_depth_views[slot]];
            let info = vk::FramebufferCreateInfo::default()
                .render_pass(hdr_pass)
                .attachments(&attachments)
                .width(extent.width)
                .height(extent.height)
                .layers(1);
            // SAFETY: fresh object; the partial state owns it.
            let framebuffer = unsafe { ctx.device.create_framebuffer(&info, None) }
                .map_err(|error| RendererError::vk(error, "post HDR framebuffer"))?;
            self.hdr_framebuffers.push(framebuffer);
        }
        // The bloom framebuffers: one per mip target per slot
        // (both mip targets share the bloom render pass —
        // identical attachment descriptions).
        let create_framebuffer = |pass: vk::RenderPass,
                                  view: vk::ImageView,
                                  fb_extent: (u32, u32),
                                  what: &str|
         -> Result<vk::Framebuffer, RendererError> {
            let attachments = [view];
            let info = vk::FramebufferCreateInfo::default()
                .render_pass(pass)
                .attachments(&attachments)
                .width(fb_extent.0)
                .height(fb_extent.1)
                .layers(1);
            // SAFETY: fresh object; the partial state owns it.
            unsafe { ctx.device.create_framebuffer(&info, None) }
                .map_err(|error| RendererError::vk(error, what))
        };
        for slot in 0..slots {
            let framebuffer = create_framebuffer(
                bloom_pass,
                self.bloom_a_views[slot],
                bloom_extent,
                "post bloom_a framebuffer",
            )?;
            self.bloom_a_framebuffers.push(framebuffer);
            let framebuffer = create_framebuffer(
                bloom_pass,
                self.bloom_b_views[slot],
                bloom_extent,
                "post bloom_b framebuffer",
            )?;
            self.bloom_b_framebuffers.push(framebuffer);
        }
        // The post framebuffers: one per swapchain image.
        for view in swapchain_views {
            let framebuffer =
                create_framebuffer(post_pass, *view, scene_extent, "post swapchain framebuffer")?;
            self.post_framebuffers.push(framebuffer);
        }

        // --- the per-slot descriptor sets: the bloom chain's
        // (bright reads the HDR view, blur_h reads bloom_a, blur_v
        // reads bloom_b) and the post pass's (the HDR view, the
        // final mip view, the sampler, the uniform).
        let sampled = vk::DescriptorType::SAMPLED_IMAGE;
        let bloom_sizes = [vk::DescriptorPoolSize {
            ty: sampled,
            descriptor_count: 6,
        }];
        // SAFETY: fresh pool; the sets reference this bundle's own
        // views and the caller's live layout.
        let (bloom_pool, bloom_sets) =
            create_pool_sets::<6>(ctx, bloom_layout, &bloom_sizes, slots as u32 * 3)?;
        self.bloom_pool = bloom_pool;
        for slot in 0..slots {
            let sources = [
                (bloom_sets[slot], self.hdr_views[slot]),
                (bloom_sets[slots + slot], self.bloom_a_views[slot]),
                (bloom_sets[2 * slots + slot], self.bloom_b_views[slot]),
            ];
            let images: Vec<_> = sources
                .iter()
                .map(|&(_, view)| {
                    vk::DescriptorImageInfo::default()
                        .image_view(view)
                        .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                })
                .collect();
            let writes: Vec<_> = sources
                .iter()
                .zip(&images)
                .map(|(&(set, _), image)| {
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(0)
                        .descriptor_type(sampled)
                        .image_info(std::slice::from_ref(image))
                })
                .collect();
            // SAFETY: fresh sets from a live pool; the views and
            // the pool share this bundle's lifetime.
            unsafe { ctx.device.update_descriptor_sets(&writes, &[]) };
            self.bright_sets[slot] = bloom_sets[slot];
            self.blur_h_sets[slot] = bloom_sets[slots + slot];
            self.blur_v_sets[slot] = bloom_sets[2 * slots + slot];
        }
        let post_sizes = [
            vk::DescriptorPoolSize {
                ty: sampled,
                descriptor_count: 4,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLER,
                descriptor_count: 2,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_BUFFER,
                descriptor_count: 2,
            },
        ];
        // SAFETY: as above.
        let (post_pool, post_sets) =
            create_pool_sets::<2>(ctx, post_layout, &post_sizes, slots as u32)?;
        self.post_pool = post_pool;
        for (slot, &set) in post_sets.iter().enumerate() {
            let images = [
                vk::DescriptorImageInfo::default()
                    .image_view(self.hdr_views[slot])
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
                vk::DescriptorImageInfo::default()
                    .image_view(self.bloom_a_views[slot])
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
                vk::DescriptorImageInfo::default().sampler(sampler),
            ];
            let buffer = [vk::DescriptorBufferInfo::default()
                .buffer(params_buffer)
                .range(size_of::<PostParams>() as u64)];
            let writes = [
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(sampled)
                    .image_info(std::slice::from_ref(&images[0])),
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(4)
                    .descriptor_type(sampled)
                    .image_info(std::slice::from_ref(&images[1])),
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(2)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .image_info(std::slice::from_ref(&images[2])),
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(3)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(&buffer),
            ];
            // SAFETY: fresh set from a live pool; every referenced
            // object shares or outlives this bundle's lifetime.
            unsafe { ctx.device.update_descriptor_sets(&writes, &[]) };
            self.post_sets[slot] = set;
        }
        Ok(())
    }

    /// Releases every handle exactly once (the bundle's own — the
    /// render passes, layouts, sampler, and uniform belong to the
    /// parent state).
    fn release(&mut self, ctx: &VulkanContext) {
        // SAFETY: owned by this bundle; destroyed only while the
        // device is idle (the rebuild proof — a swapchain
        // recreation preceded every release of a live bundle), and
        // the parent state's pipelines are destroyed after this.
        unsafe {
            // Framebuffers before the render passes they reference
            // (the passes themselves are the parent's).
            for framebuffer in self.post_framebuffers.drain(..) {
                ctx.device.destroy_framebuffer(framebuffer, None);
            }
            for framebuffer in self.bloom_b_framebuffers.drain(..) {
                ctx.device.destroy_framebuffer(framebuffer, None);
            }
            for framebuffer in self.bloom_a_framebuffers.drain(..) {
                ctx.device.destroy_framebuffer(framebuffer, None);
            }
            for framebuffer in self.hdr_framebuffers.drain(..) {
                ctx.device.destroy_framebuffer(framebuffer, None);
            }
            ctx.device.destroy_descriptor_pool(self.post_pool, None);
            ctx.device.destroy_descriptor_pool(self.bloom_pool, None);
            for (image, memory) in self
                .bloom_b_images
                .drain(..)
                .zip(self.bloom_b_memories.drain(..))
            {
                ctx.device.destroy_image(image, None);
                ctx.device.free_memory(memory, None);
            }
            for view in self.bloom_b_views.drain(..) {
                ctx.device.destroy_image_view(view, None);
            }
            for (image, memory) in self
                .bloom_a_images
                .drain(..)
                .zip(self.bloom_a_memories.drain(..))
            {
                ctx.device.destroy_image(image, None);
                ctx.device.free_memory(memory, None);
            }
            for view in self.bloom_a_views.drain(..) {
                ctx.device.destroy_image_view(view, None);
            }
            for (image, memory) in self
                .hdr_depth_images
                .drain(..)
                .zip(self.hdr_depth_memories.drain(..))
            {
                ctx.device.destroy_image(image, None);
                ctx.device.free_memory(memory, None);
            }
            for view in self.hdr_depth_views.drain(..) {
                ctx.device.destroy_image_view(view, None);
            }
            for (image, memory) in self.hdr_images.drain(..).zip(self.hdr_memories.drain(..)) {
                ctx.device.destroy_image(image, None);
                ctx.device.free_memory(memory, None);
            }
            for view in self.hdr_views.drain(..) {
                ctx.device.destroy_image_view(view, None);
            }
        }
    }
}
