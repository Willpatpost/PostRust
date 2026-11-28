//! Static, device-local textures. Creation and the complete transfer happen
//! before the first frame. Each image owns exactly one allocation.

use super::{RendererError, VulkanContext, find_memory_type, with_upload_commands};
use ash::vk;

#[derive(Default)]
pub struct Texture {
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
    pub view: vk::ImageView,
    pub format: vk::Format,
    pub extent: vk::Extent2D,
    pub(crate) allocation_bytes: u64,
}

impl Texture {
    /// Uploads tightly packed R8 or RGBA8 UNORM texels. The image and the temporary transfer
    /// buffer occupy disjoint pages in one device-local allocation. Its full
    /// size (including alignment and the staging tail) is reported at runtime.
    pub(crate) fn upload(
        ctx: &VulkanContext,
        format: vk::Format,
        extent: vk::Extent2D,
        texels: &[u8],
    ) -> Result<Self, RendererError> {
        let bytes_per_pixel = match format {
            vk::Format::R8_UNORM => 1,
            vk::Format::R8G8B8A8_UNORM => 4,
            _ => return Err(RendererError::Vulkan("unsupported texture format".into())),
        };
        let pixels = u64::from(extent.width) * u64::from(extent.height);
        if texels.is_empty() || pixels.checked_mul(bytes_per_pixel) != Some(texels.len() as u64) {
            return Err(RendererError::Vulkan(
                "expected tightly packed, nonempty texture".into(),
            ));
        }
        let usage = vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED;
        // SAFETY: the physical device is live; query the exact tiling and usage.
        let limits = unsafe {
            ctx.instance.get_physical_device_image_format_properties(
                ctx.physical_device,
                format,
                vk::ImageType::TYPE_2D,
                vk::ImageTiling::OPTIMAL,
                usage,
                vk::ImageCreateFlags::empty(),
            )
        }
        .map_err(|e| RendererError::vk(e, "texture image format/usage"))?;
        if extent.width > limits.max_extent.width || extent.height > limits.max_extent.height {
            return Err(RendererError::Vulkan(
                "texture exceeds device format extent".into(),
            ));
        }
        let mut texture = Self {
            format,
            extent,
            ..Default::default()
        };
        let mut staging = vk::Buffer::null();
        let size = texels.len() as u64;
        let image_extent = vk::Extent3D {
            width: extent.width,
            height: extent.height,
            depth: 1,
        };
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        // SAFETY: handles are owned immediately, released on every exit below.
        // The temporary buffer outlives the copy fence; only the image/view and
        // their single backing allocation survive initialization.
        let result = (|| unsafe {
            let info = vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(format)
                .extent(image_extent)
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(usage)
                .sharing_mode(vk::SharingMode::EXCLUSIVE);
            texture.image = ctx
                .device
                .create_image(&info, None)
                .map_err(|e| RendererError::vk(e, "create texture image"))?;
            let buffer_info = vk::BufferCreateInfo::default()
                .size(size.next_multiple_of(4))
                .usage(vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST)
                .sharing_mode(vk::SharingMode::EXCLUSIVE);
            staging = ctx
                .device
                .create_buffer(&buffer_info, None)
                .map_err(|e| RendererError::vk(e, "create texture staging buffer"))?;
            let image_req = ctx.device.get_image_memory_requirements(texture.image);
            let stage_req = ctx.device.get_buffer_memory_requirements(staging);
            let granularity = ctx
                .instance
                .get_physical_device_properties(ctx.physical_device)
                .limits
                .buffer_image_granularity;
            let overflow = || RendererError::Vulkan("texture allocation size overflow".into());
            let stage_offset = image_req
                .size
                .checked_next_multiple_of(stage_req.alignment.max(granularity))
                .ok_or_else(overflow)?;
            texture.allocation_bytes = stage_offset
                .checked_add(stage_req.size)
                .ok_or_else(overflow)?;
            let compatible = image_req.memory_type_bits & stage_req.memory_type_bits;
            let local = vk::MemoryPropertyFlags::DEVICE_LOCAL;
            let mapped_type = find_memory_type(
                ctx,
                compatible,
                local
                    | vk::MemoryPropertyFlags::HOST_VISIBLE
                    | vk::MemoryPropertyFlags::HOST_COHERENT,
            );
            let memory_type = mapped_type
                .or_else(|| find_memory_type(ctx, compatible, local))
                .ok_or_else(|| {
                    RendererError::NoDevice("no compatible DEVICE_LOCAL texture memory".into())
                })?;
            let alloc = vk::MemoryAllocateInfo::default()
                .memory_type_index(memory_type)
                .allocation_size(texture.allocation_bytes);
            texture.memory = ctx
                .allocate_memory(&alloc)
                .map_err(|e| RendererError::vk(e, "allocate texture image and staging memory"))?;
            ctx.texture_allocations
                .set(ctx.texture_allocations.get() + 1);
            // P5 R0 (§9.4, the attach/upload census): the PERMANENT
            // staging tail — this texture's single device-local
            // allocation carries the staging buffer's bytes (plus
            // the alignment padding before it) forever, dead VRAM
            // after the one copy. Recorded for the census row; the
            // subtraction is total-ordered by construction
            // (`allocation_bytes` = the image requirement rounded up
            // to the staging alignment, plus the staging size).
            ctx.record_texture_staging_tail(
                texture.allocation_bytes.saturating_sub(image_req.size),
            );
            ctx.device
                .bind_image_memory(texture.image, texture.memory, 0)
                .map_err(|e| RendererError::vk(e, "bind texture image"))?;
            ctx.device
                .bind_buffer_memory(staging, texture.memory, stage_offset)
                .map_err(|e| RendererError::vk(e, "bind texture staging buffer"))?;
            if mapped_type.is_some() {
                let dst = ctx
                    .device
                    .map_memory(
                        texture.memory,
                        stage_offset,
                        size,
                        vk::MemoryMapFlags::empty(),
                    )
                    .map_err(|e| RendererError::vk(e, "map texture staging buffer"))?;
                std::ptr::copy_nonoverlapping(texels.as_ptr(), dst.cast(), texels.len());
                ctx.device.unmap_memory(texture.memory);
            }
            with_upload_commands(ctx, |cmd| {
                if mapped_type.is_none() {
                    // On devices with unmappable VRAM, UpdateBuffer stages host
                    // bytes in command storage. Vulkan requires word alignment
                    // and at most 64 KiB per update.
                    let whole = texels.len() / 4 * 4;
                    for (i, chunk) in texels[..whole].chunks(65536).enumerate() {
                        ctx.device
                            .cmd_update_buffer(cmd, staging, (i * 65536) as u64, chunk);
                    }
                    if whole < texels.len() {
                        let mut last = [0; 4];
                        last[..texels.len() - whole].copy_from_slice(&texels[whole..]);
                        ctx.device
                            .cmd_update_buffer(cmd, staging, whole as u64, &last);
                    }
                }
                let barrier = vk::ImageMemoryBarrier::default()
                    .image(texture.image)
                    .subresource_range(range)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
                // Queue submission makes coherent host writes visible. The
                // transfer dependency covers the UpdateBuffer path as well.
                let staging_ready = vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
                ctx.device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[staging_ready],
                    &[],
                    &[barrier
                        .old_layout(vk::ImageLayout::UNDEFINED)
                        .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                        .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)],
                );
                let copy = vk::BufferImageCopy::default()
                    .image_extent(image_extent)
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    );
                ctx.device.cmd_copy_buffer_to_image(
                    cmd,
                    staging,
                    texture.image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[copy],
                );
                ctx.device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::FRAGMENT_SHADER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[barrier
                        .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                        .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                        .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                        .dst_access_mask(vk::AccessFlags::SHADER_READ)],
                );
            })?;
            let view_info = vk::ImageViewCreateInfo::default()
                .image(texture.image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
                .subresource_range(range);
            texture.view = ctx
                .device
                .create_image_view(&view_info, None)
                .map_err(|e| RendererError::vk(e, "create texture view"))?;
            Ok(())
        })();
        // SAFETY: upload is complete, failed before submission, or device lost.
        unsafe { ctx.device.destroy_buffer(staging, None) };
        if let Err(e) = result {
            texture.destroy(ctx);
            return Err(e);
        }
        Ok(texture)
    }

    pub(crate) fn destroy(&mut self, ctx: &VulkanContext) {
        // SAFETY: idle or partial init. Vulkan accepts null handles in these
        // destructors. Reset ownership before releasing the backing memory.
        let texture = std::mem::take(self);
        unsafe {
            ctx.device.destroy_image_view(texture.view, None);
            ctx.device.destroy_image(texture.image, None);
            ctx.device.free_memory(texture.memory, None);
        }
    }
}

/// One immutable nearest sampler. A solid white border lets solid rectangles
/// use the same shader, VBO and draw order as glyphs.
#[derive(Default)]
pub struct SamplerPool {
    nearest: vk::Sampler,
}
impl SamplerPool {
    pub(crate) fn new(ctx: &VulkanContext) -> Result<Self, RendererError> {
        Self::with_address_mode(ctx, vk::SamplerAddressMode::CLAMP_TO_BORDER)
    }
    pub(super) fn with_address_mode(
        ctx: &VulkanContext,
        address: vk::SamplerAddressMode,
    ) -> Result<Self, RendererError> {
        let info = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::NEAREST)
            .min_filter(vk::Filter::NEAREST)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(address)
            .address_mode_v(address)
            .address_mode_w(address)
            .border_color(vk::BorderColor::FLOAT_OPAQUE_WHITE);
        // SAFETY: one live device owns this sampler until descriptor teardown.
        let nearest = unsafe { ctx.device.create_sampler(&info, None) }
            .map_err(|e| RendererError::vk(e, "create texture sampler"))?;
        ctx.sampler_creations.set(ctx.sampler_creations.get() + 1);
        Ok(Self { nearest })
    }
    pub(crate) fn nearest(&self) -> vk::Sampler {
        self.nearest
    }
    pub(crate) fn destroy(&mut self, ctx: &VulkanContext) {
        // SAFETY: no descriptors are in flight, null is a valid no-op.
        unsafe {
            ctx.device
                .destroy_sampler(std::mem::take(&mut self.nearest), None)
        };
    }
}
