//! Device-lifetime scene resources (P4 S2: generalized from the
//! corner constants to the ATTACHED
//! [`SceneDescription`](crate::scene_description::SceneDescription)
//! — the design's "the existing CornerResources machinery
//! generalized"). Mesh, material blocks and descriptors survive
//! pipeline replacement; only the fenced frame slot is writable.
//! The name keeps its history (the corner was the first — and so
//! far only — attached scene); the CONTENT is caller data now.
use super::descriptors::create_sets_sized;
use super::staging::{FlushDisposition, FlushMode};
use super::{
    RendererError, SamplerPool, Texture, VulkanContext, create_device_local_buffer_staged,
    create_device_local_buffer_with_data, create_mapped_buffer, shader_layout_pins,
};
use crate::config::FRAMES_IN_FLIGHT;
use crate::lit_scene::{LightParams, Material};
use crate::scene_description::{DrawTable, GroundRoles, SceneDescription};
use ash::vk;
use bytemuck::{Pod, Zeroable};
use std::mem::offset_of;
use std::ops::Range;

/// WGSL FrameParams: mat4 followed by three 16-byte lanes. Their spare W
/// components carry eye.xyz without changing the scaffold ABI.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, Pod, Zeroable)]
pub(crate) struct FrameParams {
    view_proj: [[f32; 4]; 4],
    light_dir: [f32; 4],
    light_color: [f32; 4],
    ambient: [f32; 4],
}

// The P4 S5 layout pins (design §6.4): this Rust mirror is checked
// against the WGSL-derived layout constants the build emits from the
// COMPOSED shader sources (see `vulkan::shader_layout_pins`) — a
// drift on either side fails the build. The hand-written 112-byte
// size pin this replaces dated from Cycle 6-B; the values are the
// same, now single-sourced from the shader side.
const _: () = assert!(size_of::<FrameParams>() == shader_layout_pins::FRAME_PARAMS_SIZE);
const _: () = assert!(
    offset_of!(FrameParams, view_proj) == shader_layout_pins::FRAME_PARAMS_VIEW_PROJ_OFFSET
);
const _: () = assert!(
    offset_of!(FrameParams, light_dir) == shader_layout_pins::FRAME_PARAMS_LIGHT_DIR_OFFSET
);
const _: () = assert!(
    offset_of!(FrameParams, light_color) == shader_layout_pins::FRAME_PARAMS_LIGHT_COLOR_OFFSET
);
const _: () =
    assert!(offset_of!(FrameParams, ambient) == shader_layout_pins::FRAME_PARAMS_AMBIENT_OFFSET);
// The material tail this module writes is the WGSL MaterialParams
// block's host form (specular rgb + shininess packed as [f32; 4],
// written at the tail start — the block's only member must sit at
// offset 0 for the write to land on it).
const _: () = assert!(size_of::<[f32; 4]>() == shader_layout_pins::MATERIAL_PARAMS_SIZE);
const _: () = assert!(shader_layout_pins::MATERIAL_PARAMS_SPECULAR_OFFSET == 0);

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
    /// One descriptor set per attached material (the description's
    /// table length — validated at attach; the pass families'
    /// compile-time shape pins it at three today).
    sets: Vec<vk::DescriptorSet>,
    /// The attached albedo set's uploads, in texture-index order.
    textures: Vec<Texture>,
    sampler: SamplerPool,
    uniforms: vk::Buffer,
    uniform_memory: vk::DeviceMemory,
    mapped: *mut std::ffi::c_void,
    stride: u32,
    vertices: vk::Buffer,
    vertex_memory: vk::DeviceMemory,
    /// The attached mesh's per-material draw table (CA-14/CA-15's
    /// typed admission — [`SceneMesh::admit_draws`]'s retained
    /// [`DrawTable`]: one range per material, contiguous, covering,
    /// non-inverted, and TRIANGLE-ALIGNED at every boundary, so the
    /// lit loop's per-range draws and the depth pass's whole-mesh
    /// draw assemble the same primitive set).
    draws: DrawTable,
    /// The attached material table (kept for the downstream
    /// pass-family builders — the shadow/reflection material tails
    /// and the albedo-index mapping — and the table-length checks
    /// that used to read `lit_scene::materials()` directly).
    materials: Vec<Material>,
    /// The attached wet ground roles (the receiver and reflected
    /// ranges the recorder and reflection state consume — the
    /// values `receiver_range()`/`reflected_ranges()` used to
    /// derive from the draw table).
    ground: GroundRoles,
}

impl CornerResources {
    /// Builds the scene resources from a VALIDATED description
    /// (the caller — the context's attach step — validates whole
    /// first; a refusal here never happens for a validated
    /// description, and a construction failure releases exactly
    /// what exists). Partial-failure safe, as ever.
    pub fn new(ctx: &VulkanContext, description: &SceneDescription) -> Result<Self, RendererError> {
        let mut state = Self::default();
        if let Err(e) = state.build(ctx, description) {
            state.destroy(ctx);
            return Err(e);
        }
        Ok(state)
    }

    fn build(
        &mut self,
        ctx: &VulkanContext,
        description: &SceneDescription,
    ) -> Result<(), RendererError> {
        let materials = &description.materials;
        self.materials = materials.clone();
        // CA-14/CA-15: the TYPED admission — the same whole
        // validation `SceneDescription::validate` performed at
        // attach (idempotent, pure, attach-time-cheap), producing
        // the retained DrawTable. The description is pre-validated
        // by the attach path, so this cannot refuse; running it
        // here keeps the RETAINED table typed by construction.
        self.draws = description.mesh.admit_draws(materials.len())?;
        self.ground = description.ground.clone();
        // P5 R3 (A20, design §4.3): the attach's whole staging phase —
        // the mesh VBO and EVERY albedo texture — rides ONE pool
        // batch (one submit, one fence). The flush is DEFERRED and
        // sits at the very END of this build: every fallible step
        // above (and every CA-01 knob) fires with the batch still
        // OPEN — an abort reclaims the staged regions with nothing
        // submitted, so the partial-failure destroy below never races
        // a pending copy — while the submission itself overlaps the
        // pipeline-group build that follows this build in
        // `Renderer::attach_scene` (the meaningful independent work:
        // shader compilation), with the wait landing at the attach's
        // drain, before any first use. `None` (the pool unavailable
        // on this device) keeps the retained per-object path.
        // T16 §5.1: the typed begin — `Ok(None)` is the per-object
        // fallback (the sticky unsupported-memory arm, or a TRANSIENT
        // pressure failure that retries the backing next call); a
        // device error at the pool propagates (never masked).
        let mut staging_batch = ctx.begin_staging_batch()?;
        let mesh_bytes = bytemuck::cast_slice(&description.mesh.vertices);
        (self.vertices, self.vertex_memory) = match &mut staging_batch {
            Some(batch) => create_device_local_buffer_staged(
                batch,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                mesh_bytes,
            ),
            None => create_device_local_buffer_with_data(
                ctx,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                mesh_bytes,
            ),
        }?;
        // CA-01 fault knob: the vertex pair exists — a plan armed
        // here fires with it created, exercising this build's own
        // partial-failure release below.
        ctx.check_lifecycle_fault(crate::LifecycleFaultSite::SceneVertices)?;
        // CA-14 (Q06): the TYPED extent — the upload consumes the
        // same shape the admission matched the bytes against (the
        // description's `albedo_extent`, single-sourced from the
        // generator's constants through `TextureExtent`).
        let extent = description.albedo_extent();
        for bytes in &description.textures.textures {
            let texture = match &mut staging_batch {
                Some(batch) => Texture::upload_in(
                    batch,
                    ctx,
                    vk::Format::R8G8B8A8_UNORM,
                    vk::Extent2D {
                        width: extent.width,
                        height: extent.height,
                    },
                    bytes,
                ),
                None => Texture::upload(
                    ctx,
                    vk::Format::R8G8B8A8_UNORM,
                    vk::Extent2D {
                        width: extent.width,
                        height: extent.height,
                    },
                    bytes,
                ),
            }?;
            self.textures.push(texture);
            // CA-01 fault knob: the just-pushed texture exists (the
            // ordinal selects which of the table's textures).
            ctx.check_lifecycle_fault(crate::LifecycleFaultSite::SceneTexture)?;
        }
        self.sampler = SamplerPool::with_address_mode(ctx, vk::SamplerAddressMode::REPEAT)?;
        // CA-01 fault knob: the sampler pool exists.
        ctx.check_lifecycle_fault(crate::LifecycleFaultSite::SceneSampler)?;

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
        (self.uniforms, self.uniform_memory, self.mapped) = create_mapped_buffer(
            ctx,
            vk::BufferUsageFlags::UNIFORM_BUFFER,
            stride * (u64::from(FRAMES_IN_FLIGHT) + materials.len() as u64),
        )?;
        // CA-01 fault knob: the mapped uniform pair exists.
        ctx.check_lifecycle_fault(crate::LifecycleFaultSite::SceneUniforms)?;
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
        let (pool, layout, sets) = create_sets_sized(ctx, &bindings, &sizes, count)?;
        // The registration MUST precede the fault knob: the check
        // fires with the set group created, and the rollback
        // (this build's own partial-failure destroy) can only
        // release what `self` owns — the battery's validator
        // caught exactly this ordering (the set group leaked when
        // the knob sat between the creation and the assignment).
        self.pool = pool;
        self.layout = layout;
        self.sets = sets;
        // CA-01 fault knob: the descriptor set group exists.
        ctx.check_lifecycle_fault(crate::LifecycleFaultSite::SceneDescriptors)?;
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
        // THE BATCH'S ONE SUBMIT — deferred, and the build's LAST
        // fallible-adjacent statement by design (see the batch's
        // opening comment): everything above ran with the batch open
        // (an abort reclaims the regions, nothing submitted), and
        // from here the submission overlaps the caller's
        // pipeline-group build until the attach's drain waits it.
        // T16/T03: the flush's TYPED disposition — the Deferred
        // success carries the batch's [`UploadTicket`] (the identity
        // the attach transaction tracks to readiness), and a
        // `NotSubmitted` failure (nothing pending — the submit
        // failed) propagates as before.
        if let Some(batch) = staging_batch.as_mut() {
            match batch.flush(FlushMode::Deferred) {
                FlushDisposition::Completed | FlushDisposition::Deferred { .. } => {}
                FlushDisposition::NotSubmitted(error) => return Err(error),
                // Structurally impossible in Deferred mode (no wait
                // runs); defended anyway — propagate, never swallow.
                FlushDisposition::SubmittedUnwaited { error, .. } => return Err(error),
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

    /// The attached material table (the downstream pass-family
    /// builders' source — the shadow/reflection material tails and
    /// the table-length checks that read `lit_scene::materials()`
    /// before S2).
    pub(crate) fn materials(&self) -> &[Material] {
        &self.materials
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
        sets: &[vk::DescriptorSet],
        frame_offset: u32,
    ) {
        // SAFETY: the bound vertex buffer is the attached mesh's own;
        // all draws refer to complete triangle ranges in it (the
        // typed admission's contracts: one range per material,
        // contiguous, covering, and TRIANGLE-ALIGNED at every
        // boundary — CA-15). The
        // sets' frame regions were written after the slot's fence
        // wait (the caller's write, the coherent-buffer discipline).
        unsafe {
            ctx.device
                .cmd_bind_vertex_buffers(cmd, 0, &[self.vertices], &[0]);
            for (set, range) in sets.iter().zip(self.draws.ranges()) {
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

    /// The scene's own per-material descriptor sets (the unshadowed
    /// path's `draw` binding).
    pub(crate) fn sets(&self) -> &[vk::DescriptorSet] {
        &self.sets
    }

    /// The skinned proxy path's descriptor companions (the Era-2
    /// sibling sets bind the scene's OWN uniform/texture/sampler
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
    /// (binding 3's static offset — the same arithmetic the scene's
    /// own set writes use).
    pub(crate) fn material_offset(&self, material: usize) -> u64 {
        (u64::from(FRAMES_IN_FLIGHT) + material as u64) * u64::from(self.stride)
    }
    /// Material `i`'s albedo texture view (mapped through the
    /// attached table — the material's OWN texture index, which the
    /// corner authoring happens to make equal to the material
    /// index).
    pub(crate) fn texture_view(&self, material: usize) -> vk::ImageView {
        self.textures[self.materials[material].texture as usize].view
    }
    /// The nearest-addressing sampler (binding 0's immutable
    /// sampler — the scene sets' own choice).
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

    /// The scene's own FrameParams uniform buffer (the Era-1 wet
    /// reflection's ground re-draw binds THE TRUE camera's regions
    /// — the same buffer the scene mesh draws read — at its own
    /// binding 3).
    pub(crate) fn frame_buffer(&self) -> vk::Buffer {
        self.uniforms
    }

    /// The attached mesh's vertex buffer and its material-grouped
    /// draw ranges (the typed admission's per-material view — the
    /// shadow state's depth-only pass and the reflection's redraws
    /// draw through them).
    pub(crate) fn mesh(&self) -> (vk::Buffer, &[Range<u32>]) {
        (self.vertices, self.draws.ranges())
    }

    /// CA-15 (F15): the whole-mesh vertex count as a TYPED
    /// admission fact — the shadow depth pass's whole-buffer draw
    /// (`[0, count)`) reads this instead of re-deriving it from the
    /// raw table. The admission guarantees the count is a
    /// triangle-list boundary (a multiple of
    /// [`crate::scene_description::VERTICES_PER_TRIANGLE`]), so the
    /// single whole-mesh draw assembles exactly the primitive set
    /// the lit pass's per-material ranges assemble.
    pub(crate) fn whole_mesh_vertex_count(&self) -> u32 {
        self.draws.whole_mesh_vertex_count()
    }

    /// The ground quad's draw range — the planar mirror's RECEIVER:
    /// the range the wet re-draw re-rasterizes (EQUAL depth) with
    /// the mirrored scene sampled at the same screen position. The
    /// ATTACHED scene's own planar intent, stated where the content
    /// is authored (finding 20, phase C: the recorder assumed range
    /// zero was the reflector; the caller supplies the range and
    /// the recorder is a primitive over it — since S2 the range
    /// lives in the description's [`GroundRoles`], not the draw
    /// table's implicit slot zero).
    pub(crate) fn receiver_range(&self) -> Range<u32> {
        self.ground.receiver.clone()
    }

    /// The reflected building masses' draw ranges, in MATERIAL
    /// ORDER: position i is material 1+i (the recorder's reflected
    /// descriptor sets are laid out per material — set index
    /// material·slots + slot — so the order is the contract between
    /// this accessor and the reflection state's own set layout).
    /// The masses the planar mirror reflects toward the receiver —
    /// the ATTACHED scene's planar intent (finding 20, phase C;
    /// since S2 the ranges live in the description's
    /// [`GroundRoles`]).
    pub(crate) fn reflected_ranges(&self) -> [Range<u32>; 2] {
        self.ground.reflected.clone()
    }

    /// The wet ground's mirror-plane height (the receiver's world
    /// plane — the reflection camera's mirrored math reflects
    /// across it; the corner authoring's ground plate at y = 0).
    pub(crate) fn ground_plane_y(&self) -> f32 {
        self.ground.receiver_plane_y
    }

    /// The albedo texture view for a material index (the shadowed
    /// sets bind the same textures the scene's own sets do — mapped
    /// through the attached table).
    pub(crate) fn albedo_view(&self, material: u32) -> Option<vk::ImageView> {
        let mat = self.materials.get(material as usize)?;
        Some(self.textures[mat.texture as usize].view)
    }

    /// The scene's REPEAT-addressed sampler (the shadowed sets'
    /// immutable sampler — identical sampling to the unshadowed path).
    pub(crate) fn repeat_sampler(&self) -> vk::Sampler {
        self.sampler.nearest()
    }

    pub fn destroy(mut self, ctx: &VulkanContext) {
        // SAFETY: init failure or idle device with referencing pipelines gone.
        // Pool owns all the sets. Null handles denote partial construction.
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
