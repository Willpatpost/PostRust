//! Renderer-lifetime atlas descriptors. The single set is written once and
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
        let mut state = Self::default();
        let ty = vk::DescriptorType::COMBINED_IMAGE_SAMPLER;
        let samplers = [sampler];
        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(ty)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .immutable_samplers(&samplers)];
        // SAFETY: the view and immutable sampler outlive the layout/pool.
        // Store each handle as soon as ownership transfers to the partial state.
        let result = (|| unsafe {
            state.layout = ctx
                .device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
                .map_err(|e| RendererError::vk(e, "create atlas set layout"))?;
            let sizes = [vk::DescriptorPoolSize {
                ty,
                descriptor_count: POOL_SETS,
            }];
            state.pool = ctx
                .device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(POOL_SETS)
                        .pool_sizes(&sizes),
                    None,
                )
                .map_err(|e| RendererError::vk(e, "create atlas descriptor pool"))?;
            ctx.descriptor_pools.set(ctx.descriptor_pools.get() + 1);
            state.set = ctx
                .device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(state.pool)
                        .set_layouts(&[state.layout]),
                )
                .map_err(|e| RendererError::vk(e, "allocate atlas descriptor set"))?[0];
            ctx.descriptor_sets.set(ctx.descriptor_sets.get() + 1);
            let image = [vk::DescriptorImageInfo::default()
                .image_view(atlas_view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let write = vk::WriteDescriptorSet::default()
                .dst_set(state.set)
                .dst_binding(0)
                .descriptor_type(ty)
                .image_info(&image);
            ctx.device.update_descriptor_sets(&[write], &[]);
            Ok(())
        })();
        if let Err(e) = result {
            state.destroy(ctx);
            return Err(e);
        }
        Ok(state)
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
