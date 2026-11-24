//! Device-lifetime corner resources. Mesh, material blocks and descriptors
//! survive pipeline replacement. Only the fenced frame slot is writable.
use super::{
    RendererError, SamplerPool, Texture, VulkanContext, create_device_local_buffer_with_data,
    create_mapped_buffer, descriptors::create_sets,
};
use crate::config::FRAMES_IN_FLIGHT;
use crate::lit_mesh::corner_mesh;
use crate::lit_scene::{LightParams, materials};
use crate::proc_texture::{TEXTURE_SIZE, procedural_texture};
use ash::vk;
use bytemuck::{Pod, Zeroable};
use std::ops::Range;

/// WGSL FrameParams: mat4 followed by three 16-byte lanes. Their spare W
/// components carry eye.xyz without changing the scaffold ABI.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct FrameParams {
    view_proj: [[f32; 4]; 4],
    light_dir: [f32; 4],
    light_color: [f32; 4],
    ambient: [f32; 4],
}

const _: () = assert!(size_of::<FrameParams>() == 112);

impl FrameParams {
    pub(super) fn clear_color(&self) -> [f32; 4] {
        let [r, g, b, _] = self.ambient;
        [r, g, b, 1.]
    }

    pub(crate) fn new(view_proj: [[f32; 4]; 4], eye: [f32; 3], light: LightParams) -> Self {
        let lane = |[x, y, z]: [f32; 3], w| [x, y, z, w];
        Self {
            view_proj,
            light_dir: lane(light.dir_toward_light, eye[0]),
            light_color: lane(light.light_color, eye[1]),
            ambient: lane(light.ambient, eye[2]),
        }
    }
}

#[derive(Default)]
pub(super) struct CornerResources {
    pub layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    sets: [vk::DescriptorSet; 3],
    textures: [Texture; 3],
    sampler: SamplerPool,
    uniforms: vk::Buffer,
    uniform_memory: vk::DeviceMemory,
    mapped: *mut std::ffi::c_void,
    stride: u32,
    vertices: vk::Buffer,
    vertex_memory: vk::DeviceMemory,
    draws: [Range<u32>; 3],
}

impl CornerResources {
    pub fn new(ctx: &VulkanContext) -> Result<Self, RendererError> {
        let mut state = Self::default();
        if let Err(e) = state.build(ctx) {
            state.destroy(ctx);
            return Err(e);
        }
        Ok(state)
    }

    fn build(&mut self, ctx: &VulkanContext) -> Result<(), RendererError> {
        let (vertices, draws) = corner_mesh();
        self.draws = draws;
        (self.vertices, self.vertex_memory) = create_device_local_buffer_with_data(
            ctx,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            bytemuck::cast_slice(&vertices),
        )?;
        for (i, texture) in self.textures.iter_mut().enumerate() {
            *texture = Texture::upload(
                ctx,
                vk::Format::R8G8B8A8_UNORM,
                vk::Extent2D {
                    width: TEXTURE_SIZE,
                    height: TEXTURE_SIZE,
                },
                &procedural_texture(i as u32),
            )?;
        }
        self.sampler = SamplerPool::with_address_mode(ctx, vk::SamplerAddressMode::REPEAT)?;

        // SAFETY: query the same live device that owns the mapped UBO.
        let limits = unsafe {
            ctx.instance
                .get_physical_device_properties(ctx.physical_device)
        }
        .limits;
        let stride = (size_of::<FrameParams>() as u64)
            .next_multiple_of(limits.min_uniform_buffer_offset_alignment.max(1));
        self.stride = u32::try_from(stride)
            .map_err(|_| RendererError::Vulkan("dynamic UBO offset exceeds u32".into()))?;
        let materials = materials();
        (self.uniforms, self.uniform_memory, self.mapped) = create_mapped_buffer(
            ctx,
            vk::BufferUsageFlags::UNIFORM_BUFFER,
            stride * (u64::from(FRAMES_IN_FLIGHT) + materials.len() as u64),
        )?;
        let combined = vk::DescriptorType::COMBINED_IMAGE_SAMPLER;
        let dynamic = vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC;
        let uniform = vk::DescriptorType::UNIFORM_BUFFER;
        let frag = vk::ShaderStageFlags::FRAGMENT;
        let binding = |index, ty, stages| {
            vk::DescriptorSetLayoutBinding::default()
                .binding(index)
                .descriptor_type(ty)
                .descriptor_count(1)
                .stage_flags(stages)
        };
        let samplers = [self.sampler.nearest()];
        let bindings = [
            binding(0, combined, frag).immutable_samplers(&samplers),
            binding(2, dynamic, frag | vk::ShaderStageFlags::VERTEX),
            binding(3, uniform, frag),
        ];
        let count = materials.len() as u32;
        let sizes = [combined, dynamic, uniform].map(|ty| vk::DescriptorPoolSize {
            ty,
            descriptor_count: count,
        });
        (self.pool, self.layout, self.sets) = create_sets(ctx, &bindings, &sizes, count)?;
        for (i, mat) in materials.iter().enumerate() {
            let offset = (u64::from(FRAMES_IN_FLIGHT) + i as u64) * stride;
            let [r, g, b] = mat.specular;
            let params = [r, g, b, mat.shininess as f32];
            let image = [vk::DescriptorImageInfo::default()
                .image_view(self.textures[mat.texture as usize].view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let buffer = vk::DescriptorBufferInfo::default().buffer(self.uniforms);
            let frame = [buffer.range(size_of::<FrameParams>() as u64)];
            let material = [buffer.offset(offset).range(size_of_val(&params) as u64)];
            let write = vk::WriteDescriptorSet::default().dst_set(self.sets[i]);
            // SAFETY: disjoint aligned material tail in coherent memory, written
            // once before submission. All three descriptors stay valid until idle teardown.
            unsafe {
                self.mapped
                    .byte_add(offset as usize)
                    .cast::<[f32; 4]>()
                    .write(params);
                ctx.device.update_descriptor_sets(
                    &[
                        write
                            .dst_binding(0)
                            .descriptor_type(combined)
                            .image_info(&image),
                        write
                            .dst_binding(2)
                            .descriptor_type(dynamic)
                            .buffer_info(&frame),
                        write
                            .dst_binding(3)
                            .descriptor_type(uniform)
                            .buffer_info(&material),
                    ],
                    &[],
                );
            }
        }
        Ok(())
    }

    pub fn texture_bytes(&self) -> u64 {
        self.textures.iter().map(|t| t.allocation_bytes).sum()
    }

    /// The FrameParams dynamic-uniform offset stride (one per frame
    /// slot) — the proxy draws' 6-E companion path binds the same
    /// per-slot offsets the corner mesh draws use.
    pub(crate) fn dynamic_stride(&self) -> u32 {
        self.stride
    }

    /// The material set for a material table index, or `None` outside
    /// the table (the proxy frame flow validates before recording).
    pub(crate) fn material_set(&self, material: u32) -> Option<vk::DescriptorSet> {
        self.sets.get(material as usize).copied()
    }

    /// Called inside the lit render pass only after this slot's fence
    /// wait (the caller writes the slot's FrameParams first — the
    /// 6-F split, so the shadowed path can write the EXTENDED block
    /// into the shadow state's own uniform instead). The 6-F
    /// parameterized form: `sets` are the per-material descriptor
    /// sets to bind (the corner's own for the unshadowed path; the
    /// shadow state's per-slot sets for the shadowed path) and
    /// `frame_offset` the FrameParams dynamic offset matching those
    /// sets' uniform buffer.
    pub fn draw(
        &self,
        ctx: &VulkanContext,
        cmd: vk::CommandBuffer,
        layout: vk::PipelineLayout,
        sets: &[vk::DescriptorSet; 3],
        frame_offset: u32,
    ) {
        // SAFETY: the bound vertex buffer is the corner mesh's own;
        // all draws refer to complete triangle ranges in it. The
        // sets' frame regions were written after the slot's fence
        // wait (the caller's write, the coherent-buffer discipline).
        unsafe {
            ctx.device
                .cmd_bind_vertex_buffers(cmd, 0, &[self.vertices], &[0]);
            for (set, range) in sets.iter().zip(&self.draws) {
                ctx.device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    layout,
                    0,
                    &[*set],
                    &[frame_offset],
                );
                ctx.device
                    .cmd_draw(cmd, range.end - range.start, 1, range.start, 0);
            }
        }
    }

    /// The corner's own per-material descriptor sets (the unshadowed
    /// path's `draw` binding).
    pub(crate) fn sets(&self) -> &[vk::DescriptorSet; 3] {
        &self.sets
    }

    /// The skinned proxy path's descriptor companions (the Era-2
    /// sibling sets bind the corner's OWN uniform/texture/sampler
    /// resources — the accessors skinned_draw's set writes need).
    /// The uniform buffer handle (FrameParams regions + the material
    /// tail).
    pub(crate) fn uniform_buffer(&self) -> vk::Buffer {
        self.uniforms
    }
    /// The plain FrameParams block's byte size (binding 2's range).
    pub(crate) fn frame_params_size() -> u64 {
        std::mem::size_of::<FrameParams>() as u64
    }
    /// Material `i`'s immutable tail offset in the uniform buffer
    /// (binding 3's static offset — the same arithmetic the corner's
    /// own set writes use).
    pub(crate) fn material_offset(&self, material: usize) -> u64 {
        (u64::from(FRAMES_IN_FLIGHT) + material as u64) * u64::from(self.stride)
    }
    /// Material `i`'s albedo texture view.
    pub(crate) fn texture_view(&self, material: usize) -> vk::ImageView {
        self.textures[material].view
    }
    /// The nearest-addressing sampler (binding 0's immutable
    /// sampler — the corner sets' own choice).
    pub(crate) fn nearest_sampler(&self) -> vk::Sampler {
        self.sampler.nearest()
    }

    /// The FrameParams dynamic-uniform stride (the unshadowed path's
    /// per-slot offsets).
    pub(crate) fn frame_stride(&self) -> u32 {
        self.stride
    }

    /// Writes one slot's FrameParams region (the write half of `draw`,
    /// split for the 6-F shadowed path, which writes the EXTENDED
    /// block into the shadow state's own uniform instead).
    ///
    /// # Safety
    /// `slot` must be owned (its fence wait passed).
    pub(crate) unsafe fn write_frame(&self, slot: usize, params: &FrameParams) {
        let offset = slot as u32 * self.stride;
        // SAFETY: the coherent frame regions do not overlap other
        // slots or the immutable material tail.
        unsafe {
            self.mapped
                .byte_add(offset as usize)
                .cast::<FrameParams>()
                .write(*params);
        }
    }

    /// The corner's own FrameParams uniform buffer (the Era-1 wet
    /// reflection's ground re-draw binds THE TRUE camera's regions
    /// — the same buffer the corner mesh draws read — at its own
    /// binding 3).
    pub(crate) fn frame_buffer(&self) -> vk::Buffer {
        self.uniforms
    }

    /// The corner mesh's vertex buffer and its material-grouped draw
    /// ranges (the shadow state's depth-only pass draws the whole
    /// mesh through them).
    pub(crate) fn mesh(&self) -> (vk::Buffer, [Range<u32>; 3]) {
        (
            self.vertices,
            [
                Range {
                    start: self.draws[0].start,
                    end: self.draws[0].end,
                },
                Range {
                    start: self.draws[1].start,
                    end: self.draws[1].end,
                },
                Range {
                    start: self.draws[2].start,
                    end: self.draws[2].end,
                },
            ],
        )
    }

    /// The ground quad's draw range — the planar mirror's RECEIVER:
    /// the quad the wet re-draw re-rasterizes (EQUAL depth) with the
    /// mirrored scene sampled at the same screen position. This is
    /// the authored corner scene's OWN planar intent, stated where
    /// the scene is authored (finding 20, phase C: the recorder
    /// assumed range zero was the reflector; now the corner caller
    /// supplies the range and the recorder is a primitive over it).
    pub(crate) fn receiver_range(&self) -> Range<u32> {
        Range {
            start: self.draws[0].start,
            end: self.draws[0].end,
        }
    }

    /// The reflected building masses' draw ranges, in MATERIAL
    /// ORDER: position i is material 1+i (the recorder's reflected
    /// descriptor sets are laid out per material — set index
    /// material·slots + slot — so the order is the contract between
    /// this accessor and the reflection state's own set layout).
    /// The masses the planar mirror reflects toward the receiver —
    /// the authored corner scene's planar intent, stated where the
    /// scene is authored (finding 20, phase C: the recorder assumed
    /// materials 1.. were the reflected buildings and skipped the
    /// ground; now the corner caller supplies the ranges and the
    /// recorder iterates exactly what it is given).
    pub(crate) fn reflected_ranges(&self) -> [Range<u32>; 2] {
        [
            Range {
                start: self.draws[1].start,
                end: self.draws[1].end,
            },
            Range {
                start: self.draws[2].start,
                end: self.draws[2].end,
            },
        ]
    }

    /// The albedo texture view for a material index (the shadowed
    /// sets bind the same textures the corner's own sets do).
    pub(crate) fn albedo_view(&self, material: u32) -> Option<vk::ImageView> {
        let table = materials();
        let mat = table.get(material as usize)?;
        Some(self.textures[mat.texture as usize].view)
    }

    /// The corner's REPEAT-addressed sampler (the shadowed sets'
    /// immutable sampler — identical sampling to the unshadowed path).
    pub(crate) fn repeat_sampler(&self) -> vk::Sampler {
        self.sampler.nearest()
    }

    pub fn destroy(mut self, ctx: &VulkanContext) {
        // SAFETY: init failure or idle device with referencing pipelines gone.
        // Pool owns all three sets. Null handles denote partial construction.
        unsafe {
            ctx.device.destroy_descriptor_pool(self.pool, None);
            ctx.device.destroy_descriptor_set_layout(self.layout, None);
            if !self.mapped.is_null() {
                ctx.device.unmap_memory(self.uniform_memory);
            }
            ctx.device.destroy_buffer(self.uniforms, None);
            ctx.device.free_memory(self.uniform_memory, None);
            ctx.device.destroy_buffer(self.vertices, None);
            ctx.device.free_memory(self.vertex_memory, None);
        }
        self.sampler.destroy(ctx);
        for texture in &mut self.textures {
            texture.destroy(ctx);
        }
    }
}
