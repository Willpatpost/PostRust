//! The instanced-proxy GPU resources (Cycle 6-E — see
//! `docs/CYCLE6-6E-BRIEF.md`): the per-kind proxy mesh vertex
//! buffers (device-local, uploaded once) and the per-frame instance
//! buffer (persistently mapped host-visible memory, one disjoint
//! region per frame slot) — ONE draw call per populated proxy kind.
//! Cycle 6-E2 adds the indirect command buffer (see
//! [`crate::proxy_indirect`] — the deterministic command-byte core):
//! the same one-draw-per-kind submission with the instance count
//! read from GPU memory at draw time. The Era-2 package C adds the
//! THIRD populated kind, Rain (the unlit blended particle path —
//! see [`ProxyKind::Rain`]): the same instance buffer, the same
//! per-kind mesh VBO discipline, one more command in the indirect
//! block.
//!
//! ZERO-PER-FRAME-ALLOCATION DISCIPLINE (the standing gate): every
//! object here is created ONCE, lazily, at the FIRST populated
//! corner frame ([`PipelineState::ensure_proxy_resources`] — the
//! region-buffer precedent: the unpopulated path pays nothing); the
//! per-frame instance bytes are repacked into the renderer's
//! RETAINED Vec ([`crate::proxy_instances::repack_populations`] —
//! the F-05 overlay pattern) and copied into THIS slot's mapped
//! region after the slot's fence wait. The 6-E2 indirect buffer
//! follows the SAME lazy discipline one level deeper: it is created
//! at the first INDIRECT-mode populated frame only, so the direct
//! instanced path's allocation pins (the 6-E gates) pay nothing for
//! the mechanism they never touch. `renderer.allocations()`
//! therefore stays constant across warmed populated frames in every
//! mode.
//!
//! SYNCHRONIZATION: the instance buffer and the indirect command
//! buffer both follow the region/overlay pattern — slot S's region
//! is written only inside the frame flow that first waited slot
//! S's fence (proving the previous submission through S finished),
//! before the commands reading it are recorded and submitted. The
//! host-visible HOST_COHERENT writes are visible to the draws of
//! the submission that follows them (the queue-submit ordering
//! guarantee — the instance buffer's own documented discipline,
//! applied unchanged to the command bytes). The mesh VBOs are
//! written once through the staged-upload helper (fence-waited
//! inside) and only read afterward.
//!
//! THE PER-OBJECT REFERENCE: the `PerInstance` submission mode
//! splits the same population into one draw per INSTANCE
//! (instanceCount = 1, a per-draw byte offset into the same slot
//! region) — the pre-6-E submission shape, existing for the N-vs-1
//! pixel-equivalence gate and the before/after draw-call evidence.
//! Same bytes, same shader, same descriptor sets: only the
//! submission differs.
//!
//! THE INDIRECT SUBMISSION (6-E2): one `vkCmdDrawIndirect` per
//! populated kind, drawCount = 1, reading the kind's 16-byte
//! `VkDrawIndirectCommand` from this slot's block of the indirect
//! buffer (npc at offset 0, vehicle at 16, rain at 32 — the
//! repack's fixed kind order, the rain kind appended LAST by the
//! Era-2 package C). `firstInstance` is pinned zero (the instance
//! range arrives through the instance-rate vertex-binding offset,
//! exactly as the direct instanced draw — no `drawIndirectFirstInstance`
//! feature required), and `firstVertex` is pinned zero (each kind's
//! mesh draws from its own bound VBO at offset 0). The command
//! bytes are the pure [`crate::proxy_indirect`] packing: the pinned
//! mesh vertex counts and this frame's per-kind counts, written
//! after the slot's fence wait, byte-pinned by the core's own tests.
//!
//! THE INSTANCE-RATE VERTEX INPUT (the scaffold's shader header
//! documents the WGSL side): binding 0 is the proxy mesh at VERTEX
//! rate (the lit pipeline's 32-byte pos/normal/uv format, locations
//! 0–2); binding 1 is the packed instance bytes at INSTANCE rate
//! (stride [`INSTANCE_STRIDE`], locations 3–7 as
//! `R32G32B32A32_SFLOAT` — the four column-major matrix columns
//! then the tint vec4). The pipeline is created in the corner branch
//! of pipeline assembly with the SAME descriptor set layout as the
//! lit pipeline (the 6-B foundation; no new bindings).

use super::{
    RendererError, ScopedMappedBuffer, VulkanContext, create_device_local_buffer_with_data,
    create_mapped_buffer,
};
use crate::ProxyDrawMode;
use crate::config::FRAMES_IN_FLIGHT;
use crate::populated_meshes::{rain_streak_mesh, sedan_mesh};
use crate::proxy_indirect::{INDIRECT_COMMAND_BYTES, INDIRECT_SLOT_BYTES, indirect_kind_offset};
use crate::proxy_instances::INSTANCE_STRIDE;
use crate::proxy_mesh::ProxyVertex;
use ash::vk;
use core::ops::Range;

/// Which proxy kind a draw addresses (its mesh and instance range).
/// The Era-2 package C adds the third populated kind: Rain — the
/// translucent streaks, drawn LAST in the fixed kind order (after
/// vehicle, never between), through the unlit blended particle
/// pipeline. Rain NEVER reaches the shadow depth pass (the
/// proxy-cast path submits npc/vehicle only — translucent streaks
/// have no silhouette; see [`ProxyResources::bind_kind_buffers`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProxyKind {
    Npc,
    Vehicle,
    Rain,
}

/// One populated frame's proxy submission request: the safe layer's
/// repacked instance bytes, the repacked POSE bytes (the npc kind's
/// skin-matrix blocks in pose-index order), the per-kind counts,
/// materials, and submission mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ProxyFrameUpload<'a> {
    /// The repacked instance bytes (npc population first, vehicle
    /// after, rain LAST — [`crate::proxy_instances::repack_populations`]'s
    /// fixed order); exactly `(npc_count + vehicle_count +
    /// rain_count) * INSTANCE_STRIDE` bytes (the repack's validated
    /// total, bounded by the CONFIGURED instance cap — the typed
    /// refusal's boundary since P5 §1.3).
    pub bytes: &'a [u8],
    /// The repacked pose bytes: the npc kind's skin-matrix blocks in
    /// POSE-INDEX order (block i serves every npc whose
    /// `pose_index` is i — the safe layer validated `max(npc
    /// pose_index) < blocks.len()`), at most the CONFIGURED pose
    /// cap's blocks × [`crate::animation::POSE_STRIDE`] bytes (the
    /// repack's validated capacity). The vehicle and rain kinds
    /// never read them.
    pub pose_bytes: &'a [u8],
    pub npc_count: u32,
    pub vehicle_count: u32,
    /// The rain kind's instance count (the Era-2 package C third
    /// populated kind — the translucent streaks, appended after
    /// vehicle in every fixed order).
    pub rain_count: u32,
    /// Per-kind material indices into the corner scene's material
    /// table (caller policy). Validated by the safe layer AND again
    /// by the frame flow before any synchronization state is
    /// touched — the rain kind's index included, exactly like the
    /// other kinds.
    pub npc_material: u32,
    pub vehicle_material: u32,
    /// The rain kind's material index (the corner material its set
    /// binds — the streak texture's albedo/sampler; the tint and
    /// alpha come per-instance).
    pub rain_material: u32,
    /// The submission mode (the equivalence gate's axis): direct
    /// instanced, the per-object reference, or the 6-E2 indirect
    /// submission.
    pub mode: ProxyDrawMode,
}

/// The per-kind pipelines one populated recording needs, for ONE
/// render pass (the main pass or the HDR pass — each carries its own
/// variants): the vehicle kind's plain proxy pipeline + layout (the
/// 6-E path, byte-identical), the npc kind's skinned pipeline (the
/// skinned state's variant for this pass), and the rain kind's
/// UNLIT BLENDED particle pipeline (one object, render-pass-bound —
/// the HDR variant is the post state's lazy `rain_hdr` family).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProxyPipelines {
    pub vehicle: vk::Pipeline,
    pub npc: vk::Pipeline,
    pub rain: vk::Pipeline,
}

/// The npc kind's skinned set bind: the skinned pipeline layout, the
/// per-material skinned set, and the pose slot's dynamic offset (the
/// SECOND dynamic offset in the bind; the first is the corner
/// FrameParams offset the caller computes).
#[derive(Debug, Clone, Copy)]
pub(crate) struct SkinnedSetBind {
    pub layout: vk::PipelineLayout,
    pub set: vk::DescriptorSet,
    /// The binding-2 dynamic offset (the FrameParams slot region —
    /// the CORNER uniform's stride for the unshadowed family, the
    /// SHADOW uniform's for the shadowed family; the caller supplies
    /// the right one, NEVER derived here).
    pub frame_offset: u32,
    pub pose_offset: u32,
}

/// A PLAIN kind's set bind (formerly the vehicle kind's own
/// `VehicleSetBind` — the Era-2 package C rain stage generalized the
/// shape: every kind that draws through a plain corner-material set
/// with ONE dynamic offset at binding 2 — the vehicle kind's corner
/// material set for the unshadowed family, its plain-shadowed set
/// for the shadowed family, and the rain kind's corner set in both
/// (rain receives no shadows, so its set is always the corner one)
/// — reuses this struct; the reuse is deliberate, one shape for one
/// binding contract).
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlainSetBind {
    pub layout: vk::PipelineLayout,
    pub set: vk::DescriptorSet,
    pub frame_offset: u32,
}

/// The per-kind instance range within one slot's packed bytes (the
/// fixed npc-then-vehicle-then-rain order — rain LAST; byte offsets
/// are the range times [`INSTANCE_STRIDE`]). Pure — pinned by tests.
pub(crate) fn kind_instance_range(
    npc_count: u32,
    vehicle_count: u32,
    rain_count: u32,
    kind: ProxyKind,
) -> Range<u64> {
    let (start, end) = match kind {
        ProxyKind::Npc => (0, npc_count as u64),
        // Chained index arithmetic (u32 total, bounded by the repack).
        ProxyKind::Vehicle => (npc_count as u64, npc_count as u64 + vehicle_count as u64),
        ProxyKind::Rain => (
            npc_count as u64 + vehicle_count as u64,
            npc_count as u64 + vehicle_count as u64 + rain_count as u64,
        ),
    };
    start..end
}

/// The draw-call count a population records under a submission mode
/// (the pinned-integer headline): one per POPULATED kind instanced
/// and indirect alike (the GPU-side count changes where N lives,
/// never how many draws submit) — the rain kind joins every arm,
/// exactly like the other kinds; one per INSTANCE in the per-object
/// reference. Pure — pinned by tests; the recorded draw's own count
/// must equal it.
pub(crate) fn proxy_draw_call_count(
    mode: ProxyDrawMode,
    npc_count: u32,
    vehicle_count: u32,
    rain_count: u32,
) -> u32 {
    match mode {
        ProxyDrawMode::PerInstance => npc_count + vehicle_count + rain_count,
        ProxyDrawMode::Instanced | ProxyDrawMode::Indirect => {
            u32::from(npc_count > 0) + u32::from(vehicle_count > 0) + u32::from(rain_count > 0)
        }
    }
}

/// The 6-E2 indirect command state: one persistently mapped
/// host-visible buffer holding one 48-byte command block per frame
/// slot (the npc command at offset 0, the vehicle command at 16,
/// the rain command at 32 — the repack's fixed kind order, the rain
/// kind appended LAST by the Era-2 package C). Created LAZILY at
/// the first INDIRECT-mode populated frame, inside
/// [`ProxyResources::ensure_indirect`] (the instance buffer's own
/// discipline one level deeper: the direct instanced path's
/// allocation pins pay nothing for a mechanism it never touches);
/// re-ensured identically on pipeline replacement (the finding-A
/// discipline — the replacement's draws read this buffer).
pub(crate) struct IndirectResources {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: *mut core::ffi::c_void,
    /// The NPC mesh's vertex count (396 — the Era-1 v2 humanoid).
    npc_vertices: u32,
    /// The vehicle mesh's vertex count (552 — the Era-1 v2 sedan).
    vehicle_vertices: u32,
    /// The rain streak mesh's vertex count (6 — one quad).
    rain_vertices: u32,
}

impl IndirectResources {
    /// The indirect command buffer handle (Era-2 stage 4b: the
    /// shadowed depth pass needs access to this buffer).
    pub(super) fn buffer(&self) -> vk::Buffer {
        self.buffer
    }

    fn new(
        ctx: &VulkanContext,
        npc_vertices: u32,
        vehicle_vertices: u32,
        rain_vertices: u32,
    ) -> Result<Self, RendererError> {
        let (buffer, memory, mapped) = create_mapped_buffer(
            ctx,
            // The draw reads the commands from GPU memory — the
            // INDIRECT_BUFFER usage the validation layer demands
            // (VUID-vkCmdDrawIndirect-buffer-02709).
            vk::BufferUsageFlags::INDIRECT_BUFFER,
            // One disjoint 48-byte region per frame slot — the
            // fence-before-write discipline needs no smaller carve.
            INDIRECT_SLOT_BYTES as u64 * FRAMES_IN_FLIGHT as u64,
        )?;
        Ok(IndirectResources {
            buffer,
            memory,
            mapped,
            npc_vertices,
            vehicle_vertices,
            rain_vertices,
        })
    }

    /// Copies this frame's three command bytes into `slot`'s mapped
    /// region — the pure [`crate::proxy_indirect`] packing of the
    /// pinned mesh vertex counts and this frame's per-kind counts.
    ///
    /// # Safety
    /// The caller must own `slot` — its fence wait already passed
    /// (the same contract as [`ProxyResources::write_slot`], which
    /// the frame flow satisfies immediately before this call).
    pub(super) unsafe fn write_slot(
        &self,
        slot: usize,
        npc_count: u32,
        vehicle_count: u32,
        rain_count: u32,
    ) {
        let block = crate::proxy_indirect::pack_indirect_block(
            self.npc_vertices,
            npc_count,
            self.vehicle_vertices,
            vehicle_count,
            self.rain_vertices,
            rain_count,
        );
        // SAFETY: the documented caller contract — this slot's
        // fence wait passed, so the previous submission through this
        // slot (whose draws read this region) has completed.
        unsafe {
            let dest = self
                .mapped
                .byte_add(slot * crate::proxy_indirect::INDIRECT_SLOT_BYTES)
                .cast::<u8>();
            core::ptr::copy_nonoverlapping(
                block.as_ptr(),
                dest,
                crate::proxy_indirect::INDIRECT_SLOT_BYTES,
            );
        }
    }

    fn release(&mut self, ctx: &VulkanContext) {
        // SAFETY: owned by this state; the owning PipelineState is
        // destroyed only while the device is idle, after the draws
        // reading the buffer.
        unsafe {
            if !self.mapped.is_null() {
                ctx.device.unmap_memory(self.memory);
                self.mapped = core::ptr::null_mut();
            }
            ctx.device.destroy_buffer(self.buffer, None);
            ctx.device.free_memory(self.memory, None);
            self.buffer = vk::Buffer::null();
            self.memory = vk::DeviceMemory::null();
        }
    }
}

/// Device-lifetime proxy resources: the three mesh VBOs (npc,
/// vehicle, rain — the Era-2 package C third kind) and the mapped
/// instance buffer. Created lazily at the first populated corner
/// frame; owned by [`super::PipelineState`] (rebuilt identically on
/// pipeline replacement — the finding-A re-ensure discipline, since
/// the proxy pipelines that read these buffers die with that state).
pub(crate) struct ProxyResources {
    npc_vbo: vk::Buffer,
    npc_memory: vk::DeviceMemory,
    /// The NPC mesh's vertex count (396 — the skinned humanoid's
    /// own pin; the same count the plain v2 mesh carried, so the
    /// 6-E2 indirect command pins hold unchanged).
    npc_vertices: u32,
    vehicle_vbo: vk::Buffer,
    vehicle_memory: vk::DeviceMemory,
    vehicle_vertices: u32,
    /// The rain streak mesh's VBO and vertex count (6 — one quad;
    /// the Era-2 package C third kind's own mesh, uploaded through
    /// the same staged helper at construction).
    rain_vbo: vk::Buffer,
    rain_memory: vk::DeviceMemory,
    rain_vertices: u32,
    instances: vk::Buffer,
    instances_memory: vk::DeviceMemory,
    instances_mapped: *mut core::ffi::c_void,
    /// The instance carve's per-slot capacity, INSTANCES (P5 §1.3,
    /// A11/R1 — demand-sized under the configured cap, was the
    /// MAX_INSTANCES constant). One frame slot's byte region is
    /// `capacity * INSTANCE_STRIDE`.
    capacity: u32,
    /// One frame slot's region of the instance buffer, bytes
    /// (`capacity * INSTANCE_STRIDE` — the repack's validated bound
    /// against the configured cap, one region per frames-in-flight
    /// slot).
    slot_bytes: u64,
    /// The 6-E2 indirect command state — lazily created at the
    /// first INDIRECT-mode populated frame (see [`IndirectResources`]).
    pub(super) indirect: Option<IndirectResources>,
}

impl ProxyResources {
    /// `capacity` is the instance carve's per-slot size in instances
    /// (P5 §1.3: the demand quantum's choice at the first populated
    /// frame — `max(64, first-demand)` rounded to the power of two,
    /// clamped to the configured cap; the overlay slot's
    /// `capacity_for` arithmetic). The zero-per-frame-allocation
    /// discipline is unchanged: the carve is created once and grows
    /// only when a later frame's population exceeds it (still under
    /// the cap).
    pub(super) fn new(ctx: &VulkanContext, capacity: u32) -> Result<Self, RendererError> {
        // The Era-2 skinning claim: the npc kind's mesh is the
        // SKINNED humanoid (the same 396 vertices — pos/normal/uv
        // byte-identical to humanoid_mesh, per animation.rs's
        // derivation pin — plus the rigid joints/weights tail the
        // skinned pipeline's locations 8/9 read). The vehicle kind's
        // mesh stays the plain sedan (32-byte ProxyVertex, the plain
        // pipeline's own stride — byte-identical path). The rain
        // kind's mesh is the streak quad (6 vertices, the same
        // 32-byte ProxyVertex form — the unlit blended pipeline's
        // own input).
        let npc = crate::animation::skinned_humanoid_mesh();
        let vehicle = sedan_mesh();
        let rain = rain_streak_mesh();
        // Partial-initialization cleanup (corrective finding G):
        // handles start null; a failure releases exactly what exists.
        let mut partial = ProxyResources {
            npc_vbo: vk::Buffer::null(),
            npc_memory: vk::DeviceMemory::null(),
            npc_vertices: npc.len() as u32,
            vehicle_vbo: vk::Buffer::null(),
            vehicle_memory: vk::DeviceMemory::null(),
            vehicle_vertices: vehicle.len() as u32,
            rain_vbo: vk::Buffer::null(),
            rain_memory: vk::DeviceMemory::null(),
            rain_vertices: rain.len() as u32,
            instances: vk::Buffer::null(),
            instances_memory: vk::DeviceMemory::null(),
            instances_mapped: core::ptr::null_mut(),
            capacity,
            slot_bytes: (capacity as usize * INSTANCE_STRIDE) as u64,
            indirect: None,
        };
        if let Err(error) = partial.build(ctx, &npc, &vehicle, &rain) {
            partial.release(ctx);
            return Err(error);
        }
        Ok(partial)
    }

    fn build(
        &mut self,
        ctx: &VulkanContext,
        npc: &[crate::animation::SkinnedVertex],
        vehicle: &[ProxyVertex],
        rain: &[ProxyVertex],
    ) -> Result<(), RendererError> {
        (self.npc_vbo, self.npc_memory) = create_device_local_buffer_with_data(
            ctx,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            bytemuck::cast_slice(npc),
        )?;
        (self.vehicle_vbo, self.vehicle_memory) = create_device_local_buffer_with_data(
            ctx,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            bytemuck::cast_slice(vehicle),
        )?;
        (self.rain_vbo, self.rain_memory) = create_device_local_buffer_with_data(
            ctx,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            bytemuck::cast_slice(rain),
        )?;
        (self.instances, self.instances_memory, self.instances_mapped) = create_mapped_buffer(
            ctx,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            // One disjoint region per frame slot, each holding a full
            // bounded population — the fence-before-write discipline
            // needs no smaller carve.
            self.slot_bytes * FRAMES_IN_FLIGHT as u64,
        )?;
        Ok(())
    }

    /// The instance carve's per-slot capacity (instances) — the
    /// observability read (the frame flow records it into
    /// `FrameOutcome`) and the growth target's current value.
    pub(super) fn capacity(&self) -> u32 {
        self.capacity
    }

    /// P5 §1.3 (A11/R1): grows the instance carve to `target`
    /// instances per slot when `target` exceeds the current capacity
    /// — the overlay slot's `reserve` shape on the SHARED per-slot
    /// buffer. A no-op at or below the current capacity (the steady
    /// state; also the pipeline-replacement replication, whose
    /// target IS the outgoing capacity).
    ///
    /// # The growth path's sync discipline (the design's §1.3
    /// ownership tier: PipelineState-owned, mapped, vertex-buffer
    /// bound with per-draw offsets — no descriptors)
    /// The instance buffer is ONE buffer object shared by every
    /// frame slot (disjoint regions, shared handles), so destroying
    /// the old buffer requires that no PENDING submission reads it —
    /// not just this slot's fence but every slot's. The conservative
    /// and already-precedented point is `device_wait_idle` (the
    /// design's "device-idle recreation window" arm; the
    /// reflection/post targets' rebuild-safety shape): bounded by
    /// the power-of-two quantum to at most `log2(cap / 64)` events
    /// per session, counted on the context's monotonic
    /// arena-growth counter. No content copy: every frame that
    /// DRAWS from a slot's region first WROTE it (the
    /// fence-before-write discipline), so the first draw after the
    /// growth is preceded by that slot's write at the new offsets —
    /// the overlay growth's own argument, verbatim.
    pub(super) fn ensure_capacity(
        &mut self,
        ctx: &VulkanContext,
        target: u32,
    ) -> Result<(), RendererError> {
        if target <= self.capacity {
            return Ok(());
        }
        let slot_bytes = u64::from(target) * INSTANCE_STRIDE as u64;
        // CA-11 (the growth-handle-loss fix): the replacement pair
        // lives in a SCOPED OWNER from creation to commit — a
        // failure at the fallible idle wait below drops the guard,
        // which destroys the NEW buffer and memory (the old
        // bindings intact), instead of dropping the bare handles on
        // the floor as the pre-fix tuple did. The staging/release
        // events land on the census pair
        // (`arena_replacements`/`arena_replacement_frees`).
        let replacement = ScopedMappedBuffer::create(
            ctx,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            // One disjoint region per frame slot, each holding a
            // full bounded population — the fence-before-write
            // discipline needs no smaller carve.
            slot_bytes * FRAMES_IN_FLIGHT as u64,
        )?;
        // The replacement point: the device is drained FIRST, the
        // new buffer exists BEFORE the old one dies (a failure above
        // leaves the prior state fully intact — the finding-G
        // discipline), and the event is counted. The stall is the
        // renderer's own ordered-teardown helper (device_wait_idle,
        // typed). The CA-11 fault knob sits immediately before the
        // wait: a plan armed here fires with the replacement
        // allocated and the old carve still live — the exact leak
        // window the guard closes.
        ctx.check_lifecycle_fault(crate::LifecycleFaultSite::InstanceGrowthIdle)?;
        super::device_idle(ctx)?;
        let (instances, instances_memory, instances_mapped) = replacement.commit();
        unsafe {
            if !self.instances_mapped.is_null() {
                ctx.device.unmap_memory(self.instances_memory);
            }
            ctx.device.destroy_buffer(self.instances, None);
            ctx.device.free_memory(self.instances_memory, None);
        }
        self.instances = instances;
        self.instances_memory = instances_memory;
        self.instances_mapped = instances_mapped;
        self.capacity = target;
        self.slot_bytes = slot_bytes;
        ctx.record_arena_growth();
        Ok(())
    }

    /// Creates the 6-E2 indirect command state if absent (the lazy
    /// discipline: the first INDIRECT-mode populated frame pays the
    /// one-time allocation; direct instanced frames never do). A
    /// failure leaves the prior state fully intact.
    pub(super) fn ensure_indirect(&mut self, ctx: &VulkanContext) -> Result<(), RendererError> {
        if self.indirect.is_some() {
            return Ok(());
        }
        match IndirectResources::new(
            ctx,
            self.npc_vertices,
            self.vehicle_vertices,
            self.rain_vertices,
        ) {
            Ok(state) => {
                self.indirect = Some(state);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// Copies this frame's instance bytes into `slot`'s mapped
    /// region.
    ///
    /// # Safety
    /// The caller must own `slot` — its fence wait already passed —
    /// and `bytes` must be at most `slot_bytes` long (the repack's
    /// validated bound; the frame flow's structural invariants).
    pub(super) unsafe fn write_slot(&self, slot: usize, bytes: &[u8]) {
        debug_assert!(bytes.len() <= self.slot_bytes as usize);
        // SAFETY: the documented caller contract — this slot's fence
        // wait passed (the previous reader finished) and the length is
        // within the region's bound.
        unsafe {
            let dest = self
                .instances_mapped
                .byte_add(slot * self.slot_bytes as usize)
                .cast::<u8>();
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), dest, bytes.len());
        }
    }

    /// Records the OPAQUE proxy kinds for one populated frame —
    /// npc then vehicle, in the FIXED kind order (the repack's byte
    /// order and the indirect block's command order both pin it).
    /// THE FINDING-14 DRAW-ORDER SPLIT: the translucent rain kind
    /// is recorded separately ([`Self::draw_rain_kind`]) so the
    /// scene recorder can slot the wet-ground receiver BETWEEN the
    /// opaque kinds and the depth-tested translucent rain — the
    /// reflection blends over the opaque ground, the rain over the
    /// reflection (the ordered blend), never the reflection over
    /// the rain. The draw modes and per-kind submission logic are
    /// [`Self::draw_kinds`]'s own, unchanged; the two split entry
    /// points together record exactly what the former combined
    /// `draw` recorded (same commands, same order per kind, same
    /// counts — only the rain kind's position relative to the
    /// ground receiver changes).
    ///
    /// # Safety
    /// [`Self::draw_kinds`]'s contract (this is its `[Npc, Vehicle]`
    /// slice).
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn draw_opaque_kinds(
        &self,
        ctx: &VulkanContext,
        cmd: vk::CommandBuffer,
        slot: usize,
        upload: &ProxyFrameUpload<'_>,
        pipelines: &ProxyPipelines,
        skinned: &SkinnedSetBind,
        vehicle: &PlainSetBind,
        rain: &PlainSetBind,
    ) -> u32 {
        // SAFETY: this entry point is draw_kinds' [Npc, Vehicle]
        // slice under this function's own documented contract.
        unsafe {
            self.draw_kinds(
                ctx,
                cmd,
                slot,
                upload,
                pipelines,
                skinned,
                vehicle,
                rain,
                &[ProxyKind::Npc, ProxyKind::Vehicle],
            )
        }
    }

    /// Records the TRANSLUCENT rain kind for one populated frame
    /// (the finding-14 split's second half — see
    /// [`Self::draw_opaque_kinds`]). The rain kind binds the RAIN
    /// pipeline (this pass's unlit blended particle variant) with
    /// the rain material's corner set and the single FrameParams
    /// offset (the [`PlainSetBind`] shape, shared with the vehicle
    /// kind), then submits per the upload's mode exactly like the
    /// opaque kinds.
    ///
    /// # Safety
    /// [`Self::draw_kinds`]'s contract (this is its `[Rain]`
    /// slice).
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn draw_rain_kind(
        &self,
        ctx: &VulkanContext,
        cmd: vk::CommandBuffer,
        slot: usize,
        upload: &ProxyFrameUpload<'_>,
        pipelines: &ProxyPipelines,
        skinned: &SkinnedSetBind,
        vehicle: &PlainSetBind,
        rain: &PlainSetBind,
    ) -> u32 {
        // SAFETY: this entry point is draw_kinds' [Rain] slice
        // under this function's own documented contract.
        unsafe {
            self.draw_kinds(
                ctx,
                cmd,
                slot,
                upload,
                pipelines,
                skinned,
                vehicle,
                rain,
                &[ProxyKind::Rain],
            )
        }
    }

    /// The per-kind recording core both split entry points share:
    /// each kind binds its pipeline and set (the npc kind's SKINNED
    /// pipeline with the skinned set and its two dynamic offsets —
    /// the corner FrameParams slot offset and the pose slot offset;
    /// the vehicle kind's PLAIN proxy pipeline with the corner
    /// material set and the single FrameParams offset; the rain
    /// kind's RAIN pipeline with the rain material's corner set and
    /// the same single FrameParams offset), then its mesh at
    /// binding 0 and its instance range at binding 1, and submits
    /// per the upload's mode: one direct `vkCmdDraw` per kind, one
    /// draw per instance in the reference mode, or — 6-E2 — one
    /// `vkCmdDrawIndirect` per kind reading this slot's command
    /// block. Returns the recorded draw-call count.
    ///
    /// # Safety
    /// `cmd` must be recording inside a render pass compatible with
    /// EVERY `pipelines`' creation passes; `pipelines.npc` must be
    /// the skinned state's pipeline for THAT pass with `skinned` its
    /// own set/offsets; `pipelines.rain` must be the unlit blended
    /// particle pipeline for THAT pass (the rain arm's blend and
    /// depth-write-off states are the pipeline's own, never the
    /// caller's); `skinned`, `vehicle`, and `rain` must be the
    /// per-kind set binds the frame flow validated for every
    /// POPULATED kind (a null set for an empty kind is never bound);
    /// `slot`'s regions must hold this frame's `upload.bytes`, its
    /// pose bytes, and, in the indirect mode, its command block (the
    /// frame flow writes all after the slot's fence wait).
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_kinds(
        &self,
        ctx: &VulkanContext,
        cmd: vk::CommandBuffer,
        slot: usize,
        upload: &ProxyFrameUpload<'_>,
        pipelines: &ProxyPipelines,
        skinned: &SkinnedSetBind,
        vehicle: &PlainSetBind,
        rain: &PlainSetBind,
        kinds: &[ProxyKind],
    ) -> u32 {
        let slot_start = self.slot_bytes * slot as u64;
        let mut calls = 0;
        // The fixed kind order within the requested slice (npc,
        // vehicle, rain — the rain kind appends AFTER vehicle, never
        // between; the repack's byte order and the indirect block's
        // command order both pin it).
        for &kind in kinds {
            let (vbo, vertex_count, count, kind_index) = match kind {
                ProxyKind::Npc => (self.npc_vbo, self.npc_vertices, upload.npc_count, 0),
                ProxyKind::Vehicle => (
                    self.vehicle_vbo,
                    self.vehicle_vertices,
                    upload.vehicle_count,
                    1,
                ),
                ProxyKind::Rain => (self.rain_vbo, self.rain_vertices, upload.rain_count, 2),
            };
            if count == 0 {
                continue;
            }
            let range = kind_instance_range(
                upload.npc_count,
                upload.vehicle_count,
                upload.rain_count,
                kind,
            );
            let base = slot_start + range.start * INSTANCE_STRIDE as u64;
            // SAFETY: the render pass, pipelines, layouts, and this
            // slot's written instance and pose regions are live; the
            // sets were validated before recording (null only for
            // empty kinds, skipped above); all counts and offsets are
            // the repack's bounded values. The rain arm's set is the
            // corner material set (never a shadowed variant — rain
            // receives no shadows), bound with the rain pipeline's
            // own layout.
            unsafe {
                match kind {
                    ProxyKind::Npc => {
                        // The skinned path: the skinned pipeline +
                        // set, TWO dynamic offsets (the FrameParams
                        // slot region and the pose slot region, in
                        // binding order 2 then 5).
                        ctx.device.cmd_bind_pipeline(
                            cmd,
                            vk::PipelineBindPoint::GRAPHICS,
                            pipelines.npc,
                        );
                        ctx.device.cmd_bind_descriptor_sets(
                            cmd,
                            vk::PipelineBindPoint::GRAPHICS,
                            skinned.layout,
                            0,
                            &[skinned.set],
                            &[skinned.frame_offset, skinned.pose_offset],
                        );
                    }
                    ProxyKind::Vehicle => {
                        // The plain path — today's binding, unchanged.
                        ctx.device.cmd_bind_pipeline(
                            cmd,
                            vk::PipelineBindPoint::GRAPHICS,
                            pipelines.vehicle,
                        );
                        ctx.device.cmd_bind_descriptor_sets(
                            cmd,
                            vk::PipelineBindPoint::GRAPHICS,
                            vehicle.layout,
                            0,
                            &[vehicle.set],
                            &[vehicle.frame_offset],
                        );
                    }
                    ProxyKind::Rain => {
                        // The Era-2 package C rain path: the unlit
                        // blended particle pipeline with the rain
                        // material's corner set (the PlainSetBind
                        // shape — one dynamic offset at binding 2,
                        // the corner FrameParams region the unlit
                        // shader's view_proj reads). The pipeline's
                        // own states carry the blend and the
                        // depth-test-on/write-off contract; the
                        // submission logic below is the per-kind
                        // discipline shared with the other kinds.
                        ctx.device.cmd_bind_pipeline(
                            cmd,
                            vk::PipelineBindPoint::GRAPHICS,
                            pipelines.rain,
                        );
                        ctx.device.cmd_bind_descriptor_sets(
                            cmd,
                            vk::PipelineBindPoint::GRAPHICS,
                            rain.layout,
                            0,
                            &[rain.set],
                            &[rain.frame_offset],
                        );
                    }
                }
                ctx.device.cmd_bind_vertex_buffers(cmd, 0, &[vbo], &[0]);
                match upload.mode {
                    ProxyDrawMode::PerInstance => {
                        // The pre-6-E per-object shape: one draw per
                        // instance, each binding its own instance row of
                        // the same slot region.
                        for index in 0..count as u64 {
                            ctx.device.cmd_bind_vertex_buffers(
                                cmd,
                                1,
                                &[self.instances],
                                &[base + index * INSTANCE_STRIDE as u64],
                            );
                            ctx.device.cmd_draw(cmd, vertex_count, 1, 0, 0);
                            calls += 1;
                        }
                    }
                    ProxyDrawMode::Instanced => {
                        ctx.device
                            .cmd_bind_vertex_buffers(cmd, 1, &[self.instances], &[base]);
                        ctx.device.cmd_draw(cmd, vertex_count, count, 0, 0);
                        calls += 1;
                    }
                    ProxyDrawMode::Indirect => {
                        // The 6-E2 submission: the same bound state as
                        // the direct instanced draw, with the counts
                        // read from this slot's command block at draw
                        // time. The frame flow's ensure discipline
                        // guarantees the indirect state exists whenever
                        // this arm runs; the fallback keeps a release
                        // build safe (never silently wrong: the direct
                        // draw submits the identical geometry) if that
                        // structural invariant were ever broken — the
                        // debug_assert fires first in debug builds.
                        let indirect = self.indirect.as_ref();
                        debug_assert!(
                            indirect.is_some(),
                            "the indirect ensure discipline makes a missing \
                             command buffer unreachable"
                        );
                        match indirect {
                            Some(indirect) => {
                                ctx.device.cmd_bind_vertex_buffers(
                                    cmd,
                                    1,
                                    &[self.instances],
                                    &[base],
                                );
                                let offset = (slot * INDIRECT_SLOT_BYTES
                                    + indirect_kind_offset(kind_index))
                                    as u64;
                                // drawCount = 1, stride = one command:
                                // no multiDrawIndirect feature, no
                                // cross-kind state; firstInstance is
                                // pinned zero in the packed bytes (the
                                // portability contract). The issuance
                                // is counted AT the call site (the
                                // 6-E2 observation the render_instanced
                                // gate asserts — see the field's doc).
                                ctx.device.cmd_draw_indirect(
                                    cmd,
                                    indirect.buffer,
                                    offset,
                                    1,
                                    INDIRECT_COMMAND_BYTES as u32,
                                );
                                ctx.indirect_draws_issued
                                    .set(ctx.indirect_draws_issued.get() + 1);
                                calls += 1;
                            }
                            None => {
                                ctx.device.cmd_bind_vertex_buffers(
                                    cmd,
                                    1,
                                    &[self.instances],
                                    &[base],
                                );
                                ctx.device.cmd_draw(cmd, vertex_count, count, 0, 0);
                                calls += 1;
                            }
                        }
                    }
                }
            }
        }
        calls
    }

    /// Binds the per-kind VBO for `kind` at binding 0, and the
    /// instance range for `slot` at binding 1. Used by the shadowed
    /// depth pass (the proxy-cast path — the npc kind skins, the
    /// vehicle kind is plain). The instance range starts at `slot`'s
    /// region plus the kind's offset within that region (the fixed
    /// npc-then-vehicle-then-rain order). The RAIN arm exists for
    /// the exhaustive match only: the shadow depth pass never
    /// requests it (rain casts no shadows — translucent streaks
    /// have no silhouette; the shadowed populated path carries rain
    /// through the LIT pass only, a documented exclusion at the
    /// CALLER, `skinned_draw::record_depth_casts`).
    pub(super) fn bind_kind_buffers(
        &self,
        ctx: &VulkanContext,
        cmd: vk::CommandBuffer,
        kind: ProxyKind,
        slot: usize,
        upload: &ProxyFrameUpload<'_>,
    ) {
        let (vbo, _count) = match kind {
            ProxyKind::Npc => (self.npc_vbo, upload.npc_count),
            ProxyKind::Vehicle => (self.vehicle_vbo, upload.vehicle_count),
            ProxyKind::Rain => (self.rain_vbo, upload.rain_count),
        };
        let range = kind_instance_range(
            upload.npc_count,
            upload.vehicle_count,
            upload.rain_count,
            kind,
        );
        let base = self.slot_bytes * slot as u64 + range.start * INSTANCE_STRIDE as u64;
        unsafe {
            ctx.device.cmd_bind_vertex_buffers(cmd, 0, &[vbo], &[0]);
            ctx.device
                .cmd_bind_vertex_buffers(cmd, 1, &[self.instances], &[base]);
        }
    }

    pub(super) fn destroy(mut self, ctx: &VulkanContext) {
        self.release(ctx);
    }

    /// The per-slot region size (bytes).
    pub(super) fn slot_bytes(&self) -> u64 {
        self.slot_bytes
    }

    /// The NPC mesh's vertex count.
    pub(super) fn npc_vertices(&self) -> u32 {
        self.npc_vertices
    }

    /// The vehicle mesh's vertex count.
    pub(super) fn vehicle_vertices(&self) -> u32 {
        self.vehicle_vertices
    }

    /// The mapped instance buffer handle.
    pub(super) fn instances(&self) -> vk::Buffer {
        self.instances
    }

    /// Releases every handle exactly once; valid for both the
    /// fully-built and the partial state (null handles denote
    /// partial construction — corrective finding G).
    fn release(&mut self, ctx: &VulkanContext) {
        // SAFETY: owned by this state; the owning PipelineState is
        // destroyed only while the device is idle (replacement waits;
        // teardown waits), after the pipelines reading these buffers.
        unsafe {
            if let Some(indirect) = self.indirect.as_mut() {
                indirect.release(ctx);
            }
            self.indirect = None;
            if !self.instances_mapped.is_null() {
                ctx.device.unmap_memory(self.instances_memory);
                self.instances_mapped = core::ptr::null_mut();
            }
            ctx.device.destroy_buffer(self.instances, None);
            ctx.device.free_memory(self.instances_memory, None);
            ctx.device.destroy_buffer(self.vehicle_vbo, None);
            ctx.device.free_memory(self.vehicle_memory, None);
            ctx.device.destroy_buffer(self.rain_vbo, None);
            ctx.device.free_memory(self.rain_memory, None);
            ctx.device.destroy_buffer(self.npc_vbo, None);
            ctx.device.free_memory(self.npc_memory, None);
            self.instances = vk::Buffer::null();
            self.instances_memory = vk::DeviceMemory::null();
            self.vehicle_vbo = vk::Buffer::null();
            self.vehicle_memory = vk::DeviceMemory::null();
            self.rain_vbo = vk::Buffer::null();
            self.rain_memory = vk::DeviceMemory::null();
            self.npc_vbo = vk::Buffer::null();
            self.npc_memory = vk::DeviceMemory::null();
        }
    }
}
