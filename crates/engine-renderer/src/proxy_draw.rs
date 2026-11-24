//! The instanced-proxy GPU resources (Cycle 6-E — see
//! `docs/CYCLE6-6E-BRIEF.md`): the per-kind proxy mesh vertex
//! buffers (device-local, uploaded once) and the per-frame instance
//! buffer (persistently mapped host-visible memory, one disjoint
//! region per frame slot) — ONE draw call per populated proxy kind.
//! Cycle 6-E2 adds the indirect command buffer (see
//! [`crate::proxy_indirect`] — the deterministic command-byte core):
//! the same one-draw-per-kind submission with the instance count
//! read from GPU memory at draw time.
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
//! buffer (npc at offset 0, vehicle at 16 — the repack's fixed kind
//! order). `firstInstance` is pinned zero (the instance range
//! arrives through the instance-rate vertex-binding offset, exactly
//! as the direct instanced draw — no `drawIndirectFirstInstance`
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

use super::corner::CornerResources;
use super::{
    RendererError, VulkanContext, create_device_local_buffer_with_data, create_mapped_buffer,
};
use crate::ProxyDrawMode;
use crate::config::FRAMES_IN_FLIGHT;
use crate::populated_meshes::{humanoid_mesh, sedan_mesh};
use crate::proxy_indirect::{
    INDIRECT_COMMAND_BYTES, INDIRECT_SLOT_BYTES, indirect_kind_offset, indirect_slot_commands,
};
use crate::proxy_instances::{INSTANCE_STRIDE, MAX_INSTANCES};
use crate::proxy_mesh::ProxyVertex;
use ash::vk;
use core::ops::Range;

/// Which proxy kind a draw addresses (its mesh and instance range).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProxyKind {
    Npc,
    Vehicle,
}

/// One populated frame's proxy submission request: the safe layer's
/// repacked instance bytes plus the per-kind counts, materials, and
/// submission mode.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProxyFrameUpload<'a> {
    /// The repacked instance bytes (npc population first, vehicle
    /// after — [`crate::proxy_instances::repack_populations`]'s
    /// fixed order); exactly `(npc_count + vehicle_count) *
    /// INSTANCE_STRIDE` bytes (the repack's validated total, at most
    /// [`MAX_INSTANCES`] instances).
    pub bytes: &'a [u8],
    pub npc_count: u32,
    pub vehicle_count: u32,
    /// Per-kind material indices into the corner scene's material
    /// table (caller policy). Validated by the safe layer AND again
    /// by the frame flow before any synchronization state is touched.
    pub npc_material: u32,
    pub vehicle_material: u32,
    /// The submission mode (the equivalence gate's axis): direct
    /// instanced, the per-object reference, or the 6-E2 indirect
    /// submission.
    pub mode: ProxyDrawMode,
}

/// The per-kind instance range within one slot's packed bytes (the
/// fixed npc-then-vehicle order; byte offsets are the range times
/// [`INSTANCE_STRIDE`]). Pure — pinned by tests.
pub(crate) fn kind_instance_range(
    npc_count: u32,
    vehicle_count: u32,
    kind: ProxyKind,
) -> Range<u64> {
    let (start, end) = match kind {
        ProxyKind::Npc => (0, npc_count as u64),
        // Chained index arithmetic (u32 total, bounded by the repack).
        ProxyKind::Vehicle => (npc_count as u64, npc_count as u64 + vehicle_count as u64),
    };
    start..end
}

/// The draw-call count a population records under a submission mode
/// (the pinned-integer headline): one per POPULATED kind instanced
/// and indirect alike (the GPU-side count changes where N lives,
/// never how many draws submit); one per INSTANCE in the per-object
/// reference. Pure — pinned by tests; the recorded draw's own count
/// must equal it.
pub(crate) fn proxy_draw_call_count(
    mode: ProxyDrawMode,
    npc_count: u32,
    vehicle_count: u32,
) -> u32 {
    match mode {
        ProxyDrawMode::PerInstance => npc_count + vehicle_count,
        ProxyDrawMode::Instanced | ProxyDrawMode::Indirect => {
            u32::from(npc_count > 0) + u32::from(vehicle_count > 0)
        }
    }
}

/// The 6-E2 indirect command state: one persistently mapped
/// host-visible buffer holding one 32-byte command block per frame
/// slot (the npc command at offset 0, the vehicle command at 16 —
/// the repack's fixed kind order). Created LAZILY at the first
/// INDIRECT-mode populated frame, inside
/// [`ProxyResources::ensure_indirect`] (the instance buffer's own
/// discipline one level deeper: the direct instanced path's
/// allocation pins pay nothing for a mechanism it never touches);
/// re-ensured identically on pipeline replacement (the finding-A
/// discipline — the replacement's draws read this buffer).
pub(crate) struct IndirectResources {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: *mut core::ffi::c_void,
}

impl IndirectResources {
    fn new(ctx: &VulkanContext) -> Result<Self, RendererError> {
        let (buffer, memory, mapped) = create_mapped_buffer(
            ctx,
            // The draw reads the commands from GPU memory — the
            // INDIRECT_BUFFER usage the validation layer demands
            // (VUID-vkCmdDrawIndirect-buffer-02709).
            vk::BufferUsageFlags::INDIRECT_BUFFER,
            // One disjoint 32-byte region per frame slot — the
            // fence-before-write discipline needs no smaller carve.
            INDIRECT_SLOT_BYTES as u64 * FRAMES_IN_FLIGHT as u64,
        )?;
        Ok(IndirectResources {
            buffer,
            memory,
            mapped,
        })
    }

    /// Copies this frame's two command bytes into `slot`'s mapped
    /// region — the pure [`crate::proxy_indirect`] packing of the
    /// pinned mesh vertex counts and this frame's per-kind counts.
    ///
    /// # Safety
    /// The caller must own `slot` — its fence wait already passed
    /// (the same contract as [`ProxyResources::write_slot`], which
    /// the frame flow satisfies immediately before this call).
    pub(super) unsafe fn write_slot(&self, slot: usize, npc_count: u32, vehicle_count: u32) {
        let block = indirect_slot_commands(npc_count, vehicle_count);
        // SAFETY: the documented caller contract — this slot's
        // fence wait passed, so the previous submission through this
        // slot (whose draws read this region) has completed.
        unsafe {
            let dest = self
                .mapped
                .byte_add(slot * INDIRECT_SLOT_BYTES)
                .cast::<u8>();
            core::ptr::copy_nonoverlapping(block.as_ptr(), dest, INDIRECT_SLOT_BYTES);
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

/// Device-lifetime proxy resources: the two mesh VBOs and the mapped
/// instance buffer. Created lazily at the first populated corner
/// frame; owned by [`super::PipelineState`] (rebuilt identically on
/// pipeline replacement — the finding-A re-ensure discipline, since
/// the proxy pipeline that reads these buffers dies with that state).
pub(crate) struct ProxyResources {
    npc_vbo: vk::Buffer,
    npc_memory: vk::DeviceMemory,
    /// The NPC mesh's vertex count (108 — measured from the
    /// generated mesh, pinned by proxy_mesh's own tests).
    npc_vertices: u32,
    vehicle_vbo: vk::Buffer,
    vehicle_memory: vk::DeviceMemory,
    vehicle_vertices: u32,
    instances: vk::Buffer,
    instances_memory: vk::DeviceMemory,
    instances_mapped: *mut core::ffi::c_void,
    /// One frame slot's region of the instance buffer, bytes
    /// ([`MAX_INSTANCES`] * [`INSTANCE_STRIDE`] — the repack's bound,
    /// one region per frames-in-flight slot).
    slot_bytes: u64,
    /// The 6-E2 indirect command state — lazily created at the
    /// first INDIRECT-mode populated frame (see [`IndirectResources`]).
    pub(super) indirect: Option<IndirectResources>,
}

impl ProxyResources {
    pub(super) fn new(ctx: &VulkanContext) -> Result<Self, RendererError> {
        let npc = humanoid_mesh();
        let vehicle = sedan_mesh();
        // Partial-initialization cleanup (corrective finding G):
        // handles start null; a failure releases exactly what exists.
        let mut partial = ProxyResources {
            npc_vbo: vk::Buffer::null(),
            npc_memory: vk::DeviceMemory::null(),
            npc_vertices: npc.len() as u32,
            vehicle_vbo: vk::Buffer::null(),
            vehicle_memory: vk::DeviceMemory::null(),
            vehicle_vertices: vehicle.len() as u32,
            instances: vk::Buffer::null(),
            instances_memory: vk::DeviceMemory::null(),
            instances_mapped: core::ptr::null_mut(),
            slot_bytes: (MAX_INSTANCES * INSTANCE_STRIDE) as u64,
            indirect: None,
        };
        if let Err(error) = partial.build(ctx, &npc, &vehicle) {
            partial.release(ctx);
            return Err(error);
        }
        Ok(partial)
    }

    fn build(
        &mut self,
        ctx: &VulkanContext,
        npc: &[ProxyVertex],
        vehicle: &[ProxyVertex],
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

    /// Creates the 6-E2 indirect command state if absent (the lazy
    /// discipline: the first INDIRECT-mode populated frame pays the
    /// one-time allocation; direct instanced frames never do). A
    /// failure leaves the prior state fully intact.
    pub(super) fn ensure_indirect(&mut self, ctx: &VulkanContext) -> Result<(), RendererError> {
        if self.indirect.is_some() {
            return Ok(());
        }
        match IndirectResources::new(ctx) {
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

    /// Records the proxy draws for one populated frame. The proxy
    /// PIPELINE is bound by the caller; this binds each populated
    /// kind's descriptor set (the corner material set, with the same
    /// per-slot FrameParams dynamic offset the corner mesh draws
    /// use), its mesh at binding 0, and its instance range at
    /// binding 1, then submits per the upload's mode: one direct
    /// `vkCmdDraw` per kind, one draw per instance in the reference
    /// mode, or — 6-E2 — one `vkCmdDrawIndirect` per kind reading
    /// this slot's command block. Returns the recorded draw-call
    /// count.
    ///
    /// # Safety
    /// `cmd` must be recording inside the proxy pipeline's render
    /// pass with `layout` its pipeline layout; `sets` must be the
    /// corner material sets the frame flow validated for every
    /// POPULATED kind (a null set for an empty kind is never bound);
    /// `slot`'s regions must hold this frame's `upload.bytes` and,
    /// in the indirect mode, its command block (the frame flow
    /// writes both after the slot's fence wait).
    // Private recording plumbing: the context, target, layout, corner
    // accessor, slot, upload, and validated sets are exactly what the
    // record step holds — bundling them would hide the call's story
    // for no gain (the render_frame precedent).
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn draw(
        &self,
        ctx: &VulkanContext,
        cmd: vk::CommandBuffer,
        layout: vk::PipelineLayout,
        corner: &CornerResources,
        slot: usize,
        upload: &ProxyFrameUpload<'_>,
        sets: [vk::DescriptorSet; 2],
    ) -> u32 {
        let frame_offset = slot as u32 * corner.dynamic_stride();
        let slot_start = self.slot_bytes * slot as u64;
        let mut calls = 0;
        for (kind, set) in [ProxyKind::Npc, ProxyKind::Vehicle].into_iter().zip(sets) {
            let (vbo, vertex_count, count, kind_index) = match kind {
                ProxyKind::Npc => (self.npc_vbo, self.npc_vertices, upload.npc_count, 0),
                ProxyKind::Vehicle => (
                    self.vehicle_vbo,
                    self.vehicle_vertices,
                    upload.vehicle_count,
                    1,
                ),
            };
            if count == 0 {
                continue;
            }
            let range = kind_instance_range(upload.npc_count, upload.vehicle_count, kind);
            let base = slot_start + range.start * INSTANCE_STRIDE as u64;
            // SAFETY: the render pass, pipeline, layout, and this
            // slot's written instance region are live; the set was
            // validated before recording (null only for empty kinds,
            // skipped above); all counts and offsets are the repack's
            // bounded values.
            unsafe {
                ctx.device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    layout,
                    0,
                    &[set],
                    &[frame_offset],
                );
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
                                // portability contract).
                                ctx.device.cmd_draw_indirect(
                                    cmd,
                                    indirect.buffer,
                                    offset,
                                    1,
                                    INDIRECT_COMMAND_BYTES as u32,
                                );
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

    pub(super) fn destroy(mut self, ctx: &VulkanContext) {
        self.release(ctx);
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
            ctx.device.destroy_buffer(self.npc_vbo, None);
            ctx.device.free_memory(self.npc_memory, None);
            self.instances = vk::Buffer::null();
            self.instances_memory = vk::DeviceMemory::null();
            self.vehicle_vbo = vk::Buffer::null();
            self.vehicle_memory = vk::DeviceMemory::null();
            self.npc_vbo = vk::Buffer::null();
            self.npc_memory = vk::DeviceMemory::null();
        }
    }
}
