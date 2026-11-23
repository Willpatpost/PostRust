//! Renderer-lifetime descriptors. Each material set is written once and
//! survives swapchain changes and pipeline replacement.
use super::{RendererError, VulkanContext};
use ash::vk;

/// One live atlas set and one spare set's worth of pool capacity.
const POOL_SETS: u32 = 2;

#[derive(Default)]
pub struct DescriptorFoundation {
    pub pool: vk::DescriptorPool,
    pub layout: vk::DescriptorSetLayout,
    pub set: vk::DescriptorSet,
}

impl DescriptorFoundation {
    pub(crate) fn new(
        ctx: &VulkanContext,
        atlas_view: vk::ImageView,
        sampler: vk::Sampler,
    ) -> Result<Self, RendererError> {
        let ty = vk::DescriptorType::COMBINED_IMAGE_SAMPLER;
        let samplers = [sampler];
        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(ty)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .immutable_samplers(&samplers)];
        let sizes = [vk::DescriptorPoolSize {
            ty,
            descriptor_count: POOL_SETS,
        }];
        let (pool, layout, [set]) = create_sets(ctx, &bindings, &sizes, POOL_SETS)?;
        let image = [vk::DescriptorImageInfo::default()
            .image_view(atlas_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(ty)
            .image_info(&image);
        // SAFETY: fresh set; its immutable sampler and image outlive the pool.
        unsafe { ctx.device.update_descriptor_sets(&[write], &[]) };
        Ok(Self { pool, layout, set })
    }
    pub(crate) fn destroy(&mut self, ctx: &VulkanContext) {
        // SAFETY: pipeline layouts and all submitted users are gone. The pool
        // owns the set; null handles from partial/repeated teardown are valid.
        unsafe {
            ctx.device
                .destroy_descriptor_set_layout(std::mem::take(&mut self.layout), None);
            ctx.device
                .destroy_descriptor_pool(std::mem::take(&mut self.pool), None);
        }
        self.set = vk::DescriptorSet::null();
    }
}

/// Creates one immutable layout and allocates all its sets at initialization.
pub(super) fn create_sets<const N: usize>(
    ctx: &VulkanContext,
    bindings: &[vk::DescriptorSetLayoutBinding<'_>],
    sizes: &[vk::DescriptorPoolSize],
    capacity: u32,
) -> Result<
    (
        vk::DescriptorPool,
        vk::DescriptorSetLayout,
        [vk::DescriptorSet; N],
    ),
    RendererError,
> {
    // SAFETY: all create-info slices live through their calls. On failure
    // release the partial pool/layout; on success the caller owns both.
    unsafe {
        let layout = ctx
            .device
            .create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(bindings),
                None,
            )
            .map_err(|e| RendererError::vk(e, "create descriptor set layout"))?;
        let pool = match ctx.device.create_descriptor_pool(
            &vk::DescriptorPoolCreateInfo::default()
                .max_sets(capacity)
                .pool_sizes(sizes),
            None,
        ) {
            Ok(pool) => pool,
            Err(e) => {
                ctx.device.destroy_descriptor_set_layout(layout, None);
                return Err(RendererError::vk(e, "create descriptor pool"));
            }
        };
        ctx.descriptor_pools.set(ctx.descriptor_pools.get() + 1);
        match ctx.device.allocate_descriptor_sets(
            &vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(pool)
                .set_layouts(&[layout; N]),
        ) {
            Ok(sets) => {
                ctx.descriptor_sets
                    .set(ctx.descriptor_sets.get() + N as u64);
                Ok((pool, layout, std::array::from_fn(|i| sets[i])))
            }
            Err(e) => {
                ctx.device.destroy_descriptor_pool(pool, None);
                ctx.device.destroy_descriptor_set_layout(layout, None);
                Err(RendererError::vk(e, "allocate descriptor sets"))
            }
        }
    }
}
