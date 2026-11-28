//! Resource allocation and retirement helpers (P4 S4 — design §5.1's
//! resources row): the memory-type lookup, shader-module creation, the
//! static-upload staging path (`with_upload_commands` ->
//! `create_device_local_buffer_with_data`), and the persistently mapped
//! buffer pattern — A20's P5 seam, moved not redesigned. The fault-
//! injection PLAN arithmetic lives here too (the design's resources row
//! owns the fault-injection cells; the cells themselves are
//! `VulkanContext` fields — §5.2 keeps the context's fields exactly —
//! so the movable half, the pure advance logic and its seam tests,
//! lands in this module). Pure move from vulkan.rs (stage S4); no logic
//! changed — only location and visibility.
//!
//! P5 R3 (A20): the per-object staging helpers are now the
//! UNAVAILABLE-POOL fallback — every attach-time buffer upload stages
//! through [`super::staging`]'s pool when a batch exists
//! ([`create_device_local_buffer_staged`], the destination-creation
//! half shared byte-for-byte with the retained helper); the throwaway
//! HOST_VISIBLE pairs survive only for devices that cannot back the
//! pool, keeping exactly the pre-R3 behavior there.

use ash::vk;

use super::RendererError;
use super::context::VulkanContext;
use crate::config::{TargetFaultSite, TargetFaultStage};

pub(super) fn find_memory_type(
    ctx: &VulkanContext,
    type_bits: u32,
    properties: vk::MemoryPropertyFlags,
) -> Option<u32> {
    let memory_properties = unsafe {
        ctx.instance
            .get_physical_device_memory_properties(ctx.physical_device)
    };
    for (index, memory_type) in memory_properties.memory_types.iter().enumerate() {
        if type_bits & (1 << index) != 0 && memory_type.property_flags.contains(properties) {
            return Some(index as u32);
        }
    }
    None
}

pub(super) fn create_shader_module(
    ctx: &VulkanContext,
    spirv: &[u8],
) -> Result<vk::ShaderModule, RendererError> {
    let words: Vec<u32> = spirv
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| u32::from_le_bytes(*word))
        .collect();
    let info = vk::ShaderModuleCreateInfo::default().code(&words);
    unsafe { ctx.device.create_shader_module(&info, None) }
        .map_err(|error| RendererError::vk(error, "create_shader_module"))
}

/// Records an init-time transfer on the graphics queue. The staging resources
/// may be released or reused only after the fence signals.
///
/// P5 R0 (§9.4, the attach/upload census): every COMPLETED call
/// records one one-shot staging submission plus its blocking-wait
/// duration through [`VulkanContext::record_upload_submit`] — the
/// helper's whole per-object machinery (fresh pool, command
/// buffer, fence, submit, blocking wait, destruction) is exactly
/// what the census row counts, and exactly what A20/R3's pool
/// replaces. A call that fails before the wait counts nothing
/// (never overcounted); the timing is two `Instant::now()` reads
/// on an attach-only path, never the frame path.
pub(super) fn with_upload_commands(
    ctx: &VulkanContext,
    record: impl FnOnce(vk::CommandBuffer),
) -> Result<(), RendererError> {
    let mut pool = vk::CommandPool::null();
    let mut fence = vk::Fence::null();
    let mut blocking_wait = std::time::Duration::ZERO;
    let result = (|| {
        let pool_info =
            vk::CommandPoolCreateInfo::default().queue_family_index(ctx.graphics_family);
        pool = unsafe { ctx.device.create_command_pool(&pool_info, None) }
            .map_err(|error| RendererError::vk(error, "staging command pool"))?;
        let command_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let command = unsafe { ctx.device.allocate_command_buffers(&command_info) }
            .map_err(|error| RendererError::vk(error, "staging command buffer"))?[0];
        unsafe {
            ctx.device
                .begin_command_buffer(
                    command,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|error| RendererError::vk(error, "begin staging"))?;
            record(command);
            ctx.device
                .end_command_buffer(command)
                .map_err(|error| RendererError::vk(error, "end staging"))?;
            fence = ctx
                .device
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .map_err(|error| RendererError::vk(error, "staging fence"))?;
            let staging_commands = [command];
            let submit_info = vk::SubmitInfo::default().command_buffers(&staging_commands);
            ctx.device
                .queue_submit(ctx.graphics_queue, &[submit_info], fence)
                .map_err(|error| RendererError::vk(error, "staging submit"))?;
            let wait_started = std::time::Instant::now();
            ctx.device
                .wait_for_fences(&[fence], true, u64::MAX)
                .map_err(|error| RendererError::vk(error, "staging wait"))?;
            blocking_wait = wait_started.elapsed();
        }
        Ok(())
    })();
    if result.is_ok() {
        ctx.record_upload_submit(blocking_wait);
    }
    // SAFETY: a successful submit was waited above (or the device was lost).
    // Destroying the pool also releases its single primary command buffer.
    unsafe {
        ctx.device.destroy_fence(fence, None);
        ctx.device.destroy_command_pool(pool, None);
    }
    result
}

pub(super) fn create_device_local_buffer_with_data(
    ctx: &VulkanContext,
    usage: vk::BufferUsageFlags,
    data: &[u8],
) -> Result<(vk::Buffer, vk::DeviceMemory), RendererError> {
    let size = data.len() as vk::DeviceSize;
    // Partial-initialization cleanup (corrective finding G): tracked
    // handles are released when any step fails; the success path inside
    // the closure releases the one-shot upload resources itself and
    // nulls them so "non-null ⇒ needs release" always holds.
    let mut buffer = vk::Buffer::null();
    let mut memory = vk::DeviceMemory::null();
    let mut staging = vk::Buffer::null();
    let mut staging_memory = vk::DeviceMemory::null();
    let result = (|| -> Result<(), RendererError> {
        let buffer_info = vk::BufferCreateInfo::default()
            .size(size)
            // The staging copy's destination must allow transfers
            // (VUID-vkCmdCopyBuffer-dstBuffer-00120 — surfaced by the
            // CI validation-enabled smoke, invisible to the layer-less
            // pod kit).
            .usage(usage | vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        buffer = unsafe { ctx.device.create_buffer(&buffer_info, None) }
            .map_err(|error| RendererError::vk(error, "create_buffer"))?;
        let requirements = unsafe { ctx.device.get_buffer_memory_requirements(buffer) };
        let memory_index = find_memory_type(
            ctx,
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .ok_or_else(|| RendererError::NoDevice("no DEVICE_LOCAL memory".into()))?;
        let memory_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_index);
        memory = unsafe { ctx.allocate_memory(&memory_info) }
            .map_err(|error| RendererError::vk(error, "allocate_memory"))?;
        unsafe { ctx.device.bind_buffer_memory(buffer, memory, 0) }
            .map_err(|error| RendererError::vk(error, "bind_buffer_memory"))?;

        // Staging upload: HOST_VISIBLE staging buffer → copy command → fence wait.
        let staging_info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        staging = unsafe { ctx.device.create_buffer(&staging_info, None) }
            .map_err(|error| RendererError::vk(error, "create staging"))?;
        let staging_requirements = unsafe { ctx.device.get_buffer_memory_requirements(staging) };
        let staging_index = find_memory_type(
            ctx,
            staging_requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or_else(|| RendererError::NoDevice("no HOST_VISIBLE memory".into()))?;
        let staging_memory_info = vk::MemoryAllocateInfo::default()
            .allocation_size(staging_requirements.size)
            .memory_type_index(staging_index);
        staging_memory = unsafe { ctx.allocate_memory(&staging_memory_info) }
            .map_err(|error| RendererError::vk(error, "allocate staging memory"))?;
        unsafe { ctx.device.bind_buffer_memory(staging, staging_memory, 0) }
            .map_err(|error| RendererError::vk(error, "bind staging memory"))?;
        unsafe {
            let mapped = ctx
                .device
                .map_memory(staging_memory, 0, size, vk::MemoryMapFlags::empty())
                .map_err(|error| RendererError::vk(error, "map staging"))?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), mapped as *mut u8, data.len());
            ctx.device.unmap_memory(staging_memory);
        }

        with_upload_commands(ctx, |command| unsafe {
            ctx.device.cmd_copy_buffer(
                command,
                staging,
                buffer,
                &[vk::BufferCopy {
                    src_offset: 0,
                    dst_offset: 0,
                    size,
                }],
            );
        })?;
        unsafe {
            ctx.device.destroy_buffer(staging, None);
            staging = vk::Buffer::null();
            ctx.device.free_memory(staging_memory, None);
            staging_memory = vk::DeviceMemory::null();
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            // P5 R0 (§9.4): one completed device-local buffer upload
            // through a throwaway HOST_VISIBLE staging pair (created
            // and destroyed inside the closure above) — the census
            // row's buffer-half staging object count.
            ctx.record_staged_buffer_upload();
            Ok((buffer, memory))
        }
        Err(error) => {
            unsafe {
                if staging != vk::Buffer::null() {
                    ctx.device.destroy_buffer(staging, None);
                }
                if staging_memory != vk::DeviceMemory::null() {
                    ctx.device.free_memory(staging_memory, None);
                }
                if buffer != vk::Buffer::null() {
                    ctx.device.destroy_buffer(buffer, None);
                }
                if memory != vk::DeviceMemory::null() {
                    ctx.device.free_memory(memory, None);
                }
            }
            Err(error)
        }
    }
}

/// P5 R3 (A20, design §4.3): the POOLED form of
/// [`create_device_local_buffer_with_data`] — the destination's
/// creation is the retained helper's own (byte-identical: the same
/// buffer, memory-type, and binding steps, the same partial-failure
/// discipline), while the staging rides the CALLER'S batch (one
/// shared, persistently mapped pool region instead of a throwaway
/// HOST_VISIBLE pair; the copy is recorded into the batch's replay
/// log and executes at the batch's ONE submit). Callers with no
/// batch (the pool unavailable on this device) keep the retained
/// per-object helper — exactly the pre-R3 behavior.
pub(super) fn create_device_local_buffer_staged(
    batch: &mut super::staging::StagingBatch<'_>,
    usage: vk::BufferUsageFlags,
    data: &[u8],
) -> Result<(vk::Buffer, vk::DeviceMemory), RendererError> {
    let ctx = batch.ctx();
    let size = data.len() as vk::DeviceSize;
    // Partial-initialization cleanup (corrective finding G), the
    // retained helper's own discipline: tracked handles are released
    // when any step fails.
    let mut buffer = vk::Buffer::null();
    let mut memory = vk::DeviceMemory::null();
    let result = (|| -> Result<(), RendererError> {
        let buffer_info = vk::BufferCreateInfo::default()
            .size(size)
            // The staging copy's destination must allow transfers
            // (VUID-vkCmdCopyBuffer-dstBuffer-00120 — surfaced by the
            // CI validation-enabled smoke, invisible to the layer-less
            // pod kit).
            .usage(usage | vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        buffer = unsafe { ctx.device.create_buffer(&buffer_info, None) }
            .map_err(|error| RendererError::vk(error, "create_buffer"))?;
        let requirements = unsafe { ctx.device.get_buffer_memory_requirements(buffer) };
        let memory_index = find_memory_type(
            ctx,
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .ok_or_else(|| RendererError::NoDevice("no DEVICE_LOCAL memory".into()))?;
        let memory_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_index);
        memory = unsafe { ctx.allocate_memory(&memory_info) }
            .map_err(|error| RendererError::vk(error, "allocate_memory"))?;
        unsafe { ctx.device.bind_buffer_memory(buffer, memory, 0) }
            .map_err(|error| RendererError::vk(error, "bind_buffer_memory"))?;
        // The pooled staging: one region in the shared backing, one
        // recorded copy — the bytes and the destination semantics
        // identical to the per-object path's pair-and-copy. T03 §3.3:
        // the DESTINATION registers with the open batch — on a
        // `SubmittedUnwaited` flush the registration moves to the
        // ticket's quarantine (the caller is then forbidden to
        // destroy these handles: a submitted copy may still reference
        // them); on every other disposition the registration is
        // discarded (the caller owns the pair).
        let region = batch.stage(data)?;
        batch.record_buffer_copy(buffer, 0, region);
        batch.register_destination(super::staging::StagingDestination::Buffer { buffer, memory });
        Ok(())
    })();
    match result {
        Ok(()) => Ok((buffer, memory)),
        Err(error) => {
            // SAFETY: exactly the handles created above, each exactly
            // once (null until created).
            unsafe {
                if buffer != vk::Buffer::null() {
                    ctx.device.destroy_buffer(buffer, None);
                }
                if memory != vk::DeviceMemory::null() {
                    ctx.device.free_memory(memory, None);
                }
            }
            Err(error)
        }
    }
}

pub(super) fn create_mapped_buffer(
    ctx: &VulkanContext,
    usage: vk::BufferUsageFlags,
    size: vk::DeviceSize,
) -> Result<(vk::Buffer, vk::DeviceMemory, *mut core::ffi::c_void), RendererError> {
    // Partial-initialization cleanup (corrective finding G).
    let mut buffer = vk::Buffer::null();
    let mut memory = vk::DeviceMemory::null();
    let result = (|| -> Result<*mut core::ffi::c_void, RendererError> {
        let buffer_info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        buffer = unsafe { ctx.device.create_buffer(&buffer_info, None) }
            .map_err(|error| RendererError::vk(error, "create overlay buffer"))?;
        let requirements = unsafe { ctx.device.get_buffer_memory_requirements(buffer) };
        let memory_index = find_memory_type(
            ctx,
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or_else(|| RendererError::NoDevice("no HOST_VISIBLE memory".into()))?;
        let memory_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_index);
        memory = unsafe { ctx.allocate_memory(&memory_info) }
            .map_err(|error| RendererError::vk(error, "allocate overlay memory"))?;
        unsafe { ctx.device.bind_buffer_memory(buffer, memory, 0) }
            .map_err(|error| RendererError::vk(error, "bind overlay memory"))?;
        let mapped = unsafe {
            ctx.device
                .map_memory(memory, 0, size, vk::MemoryMapFlags::empty())
                .map_err(|error| RendererError::vk(error, "map overlay"))?
        };
        Ok(mapped)
    })();
    match result {
        Ok(mapped) => Ok((buffer, memory, mapped)),
        Err(error) => {
            unsafe {
                if buffer != vk::Buffer::null() {
                    ctx.device.destroy_buffer(buffer, None);
                }
                if memory != vk::DeviceMemory::null() {
                    ctx.device.free_memory(memory, None);
                }
            }
            Err(error)
        }
    }
}

/// The fault plan's pure advance logic, extracted so the seam is
/// unit-testable without a Vulkan device (the mock-free discipline:
/// the tests below exercise THIS function, the one
/// [`VulkanContext::check_target_fault`] actually runs).
///
/// A plan matches only when both the site and the stage equal the
/// check's; a match decrements the ordinal, and the fault fires when
/// it reaches zero (consuming the plan). Non-matching checks leave
/// the plan untouched. The ordinal is 1-based; `set_target_fault`
/// clamps it to at least 1.
pub(super) fn advance_target_fault_plan(
    plan: &mut Option<(TargetFaultSite, TargetFaultStage, u32)>,
    site: TargetFaultSite,
    stage: TargetFaultStage,
) -> Option<RendererError> {
    let (plan_site, plan_stage, ordinal) = (*plan)?;
    if plan_site != site || plan_stage != stage {
        return None;
    }
    let remaining = ordinal - 1;
    if remaining == 0 {
        *plan = None;
        Some(RendererError::InjectedFault(format!("{site:?} {stage:?}")))
    } else {
        *plan = Some((plan_site, plan_stage, remaining));
        None
    }
}

/// The LIFECYCLE fault plan's advance logic (CA-01/CA-10/CA-11's
/// seam — the target plan's own discipline on the flat lifecycle
/// site enum): extracted pure so the seam is unit-testable without
/// a Vulkan device, exactly as [`advance_target_fault_plan`] is.
///
/// A plan matches only when the site equals the check's; a match
/// decrements the ordinal, and the fault fires when it reaches zero
/// (consuming the plan). Non-matching checks leave the plan
/// untouched. The ordinal is 1-based; `set_lifecycle_fault` clamps
/// it to at least 1.
pub(super) fn advance_lifecycle_fault_plan(
    plan: &mut Option<(crate::LifecycleFaultSite, u32)>,
    site: crate::LifecycleFaultSite,
) -> Option<RendererError> {
    let (plan_site, ordinal) = (*plan)?;
    if plan_site != site {
        return None;
    }
    let remaining = ordinal - 1;
    if remaining == 0 {
        *plan = None;
        Some(RendererError::InjectedFault(format!("{site:?}")))
    } else {
        *plan = Some((plan_site, remaining));
        None
    }
}

/// A mapped buffer held by a SCOPED OWNER while unpublished (CA-11
/// — the growth-handle-loss fix): [`create_mapped_buffer`] returns
/// a bare `(buffer, memory, mapped)` tuple with no destructor, so
/// any fallible step between the allocation and the replacement
/// point (the arena growth paths' `device_idle`) propagated the
/// error while DROPPING the new handles on the floor — the old
/// bindings stayed intact but the new buffer and its memory leaked
/// for the device's remaining lifetime.
///
/// This guard closes the window: it owns the triple from creation,
/// [`Drop`] destroys exactly what exists (so every `?` between
/// [`Self::create`] and [`Self::commit`] is leak-free by
/// construction), and `commit` disarms it at the replacement point
/// — the one place the caller installs the pieces into live state
/// and retirement of the old carve begins. The design alternative —
/// reordering so the fallible idle wait PRECEDES the allocation —
/// was considered and rejected: it would pin the discipline to
/// today's single fallible step, while the guard covers any future
/// fallible step in the window (the audit's stated shape), at an
/// identical stall profile (one `device_wait_idle` per growth
/// event either way).
///
/// The staging/release events are counted on the context's census
/// pair (`arena_replacements` / `arena_replacement_frees`) so the
/// growth battery can prove the release ran — the
/// target-images/target-frees counters' own discipline.
pub(super) struct ScopedMappedBuffer {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: *mut core::ffi::c_void,
    ctx: *const VulkanContext,
}

impl ScopedMappedBuffer {
    /// Creates the replacement pair through the existing
    /// partial-failure-safe helper and takes ownership of it (the
    /// census pair's staging half counted here). The caller must
    /// keep `ctx` alive and the device valid for the guard's
    /// lifetime (the renderer's single-threaded frame flow owns
    /// both).
    pub(super) fn create(
        ctx: &VulkanContext,
        usage: vk::BufferUsageFlags,
        size: vk::DeviceSize,
    ) -> Result<Self, RendererError> {
        let (buffer, memory, mapped) = create_mapped_buffer(ctx, usage, size)?;
        ctx.record_arena_replacement_staged();
        Ok(Self {
            buffer,
            memory,
            mapped,
            ctx,
        })
    }

    /// Disarms the guard and hands the pieces to the caller at the
    /// replacement point — the caller installs them into live
    /// state immediately (the steps below the commit are
    /// commit-only). After this call the guard's `Drop` is a no-op.
    pub(super) fn commit(mut self) -> (vk::Buffer, vk::DeviceMemory, *mut core::ffi::c_void) {
        let pieces = (self.buffer, self.memory, self.mapped);
        self.buffer = vk::Buffer::null();
        self.memory = vk::DeviceMemory::null();
        self.mapped = core::ptr::null_mut();
        pieces
    }
}

impl Drop for ScopedMappedBuffer {
    fn drop(&mut self) {
        if self.buffer != vk::Buffer::null() {
            let ctx = unsafe { &*self.ctx };
            unsafe {
                if !self.mapped.is_null() {
                    ctx.device.unmap_memory(self.memory);
                }
                ctx.device.destroy_buffer(self.buffer, None);
                ctx.device.free_memory(self.memory, None);
            }
            ctx.record_arena_replacement_released();
        }
    }
}

// SAFETY (the raw pointers): `ctx` and `mapped` are confined to the
// renderer's single owning thread exactly like the live states' own
// mapped pointers — the guard exists only inside one frame-flow or
// ensure call, dropped or committed before that call returns (and
// raw pointers already make the type !Send/!Sync, so it cannot
// silently cross threads).

#[cfg(test)]
mod target_fault_seam_tests {
    use super::advance_target_fault_plan;
    use crate::RendererError;
    use crate::{TargetFaultSite, TargetFaultStage};

    /// A fired plan is consumed: the second matching check passes.
    #[test]
    fn plan_fires_once_then_stays_consumed() {
        let mut plan = Some((TargetFaultSite::Post, TargetFaultStage::Image, 1));
        assert!(
            advance_target_fault_plan(&mut plan, TargetFaultSite::Post, TargetFaultStage::Image)
                .is_some()
        );
        assert_eq!(plan, None);
        assert!(
            advance_target_fault_plan(&mut plan, TargetFaultSite::Post, TargetFaultStage::Image)
                .is_none()
        );
        assert_eq!(plan, None);
    }

    /// Only a matching site AND stage decrements the ordinal; every
    /// other check combination leaves the plan untouched.
    #[test]
    fn ordinal_decrements_only_on_matching_site_and_stage() {
        let mut plan = Some((TargetFaultSite::Post, TargetFaultStage::Image, 3));
        for (site, stage) in [
            (TargetFaultSite::Wet, TargetFaultStage::Image),
            (TargetFaultSite::Shadow, TargetFaultStage::Image),
            (TargetFaultSite::Post, TargetFaultStage::Memory),
            (TargetFaultSite::Post, TargetFaultStage::Framebuffer),
        ] {
            assert!(advance_target_fault_plan(&mut plan, site, stage).is_none());
            assert_eq!(
                plan,
                Some((TargetFaultSite::Post, TargetFaultStage::Image, 3))
            );
        }
        // Three matching checks: the first two decrement, the third fires.
        assert!(
            advance_target_fault_plan(&mut plan, TargetFaultSite::Post, TargetFaultStage::Image)
                .is_none()
        );
        assert_eq!(
            plan,
            Some((TargetFaultSite::Post, TargetFaultStage::Image, 2))
        );
        assert!(
            advance_target_fault_plan(&mut plan, TargetFaultSite::Post, TargetFaultStage::Image)
                .is_none()
        );
        assert_eq!(
            plan,
            Some((TargetFaultSite::Post, TargetFaultStage::Image, 1))
        );
        assert!(
            advance_target_fault_plan(&mut plan, TargetFaultSite::Post, TargetFaultStage::Image)
                .is_some()
        );
        assert_eq!(plan, None);
    }

    /// The fired error names the site and stage (the gate asserts the
    /// variant; the message carries the cell for diagnostics).
    #[test]
    fn the_fired_fault_is_the_injected_variant_naming_its_cell() {
        let mut plan = Some((TargetFaultSite::Shadow, TargetFaultStage::Bind, 1));
        match advance_target_fault_plan(&mut plan, TargetFaultSite::Shadow, TargetFaultStage::Bind)
        {
            Some(RendererError::InjectedFault(message)) => {
                assert!(message.contains("Shadow"), "message: {message}");
                assert!(message.contains("Bind"), "message: {message}");
            }
            other => panic!("expected InjectedFault, got {other:?}"),
        }
    }

    /// An unarmed plan (None) never fires and stays None.
    #[test]
    fn an_unarmed_plan_never_fires() {
        let mut plan = None;
        for site in [
            TargetFaultSite::Post,
            TargetFaultSite::Wet,
            TargetFaultSite::Shadow,
        ] {
            for stage in [
                TargetFaultStage::Image,
                TargetFaultStage::MemoryLookup,
                TargetFaultStage::Memory,
                TargetFaultStage::Bind,
                TargetFaultStage::View,
                TargetFaultStage::Framebuffer,
            ] {
                assert!(advance_target_fault_plan(&mut plan, site, stage).is_none());
            }
        }
        assert_eq!(plan, None);
    }
}

#[cfg(test)]
mod lifecycle_fault_seam_tests {
    use super::advance_lifecycle_fault_plan;
    use crate::LifecycleFaultSite;
    use crate::RendererError;

    /// A fired plan is consumed: the second matching check passes
    /// (the target seam's own first rule, on the lifecycle enum).
    #[test]
    fn plan_fires_once_then_stays_consumed() {
        let mut plan = Some((LifecycleFaultSite::LitPipeline, 1));
        assert!(advance_lifecycle_fault_plan(&mut plan, LifecycleFaultSite::LitPipeline).is_some());
        assert_eq!(plan, None);
        assert!(advance_lifecycle_fault_plan(&mut plan, LifecycleFaultSite::LitPipeline).is_none());
        assert_eq!(plan, None);
    }

    /// Only the MATCHING site decrements the ordinal; every other
    /// site's check leaves the plan untouched — the property that
    /// lets one renderer walk an attach retry through several
    /// injections without cross-talk between the clusters.
    #[test]
    fn ordinal_decrements_only_on_the_matching_site() {
        let mut plan = Some((LifecycleFaultSite::PoseGrowthIdle, 3));
        for site in [
            LifecycleFaultSite::RegionGrowthIdle,
            LifecycleFaultSite::InstanceGrowthIdle,
            LifecycleFaultSite::SceneVertices,
            LifecycleFaultSite::FrameSubmit,
            LifecycleFaultSite::ProxyPipeline,
        ] {
            assert!(advance_lifecycle_fault_plan(&mut plan, site).is_none());
            assert_eq!(plan, Some((LifecycleFaultSite::PoseGrowthIdle, 3)));
        }
        // Three matching checks: the first two decrement, the third
        // fires and consumes the plan.
        assert!(
            advance_lifecycle_fault_plan(&mut plan, LifecycleFaultSite::PoseGrowthIdle).is_none()
        );
        assert_eq!(plan, Some((LifecycleFaultSite::PoseGrowthIdle, 2)));
        assert!(
            advance_lifecycle_fault_plan(&mut plan, LifecycleFaultSite::PoseGrowthIdle).is_none()
        );
        assert_eq!(plan, Some((LifecycleFaultSite::PoseGrowthIdle, 1)));
        assert!(
            advance_lifecycle_fault_plan(&mut plan, LifecycleFaultSite::PoseGrowthIdle).is_some()
        );
        assert_eq!(plan, None);
    }

    /// The fired error is the InjectedFault variant naming the site
    /// (the batteries assert the variant; the message carries the
    /// step for diagnostics).
    #[test]
    fn the_fired_fault_is_the_injected_variant_naming_its_step() {
        let mut plan = Some((LifecycleFaultSite::FrameSubmit, 1));
        match advance_lifecycle_fault_plan(&mut plan, LifecycleFaultSite::FrameSubmit) {
            Some(RendererError::InjectedFault(message)) => {
                assert!(message.contains("FrameSubmit"), "message: {message}");
            }
            other => panic!("expected InjectedFault, got {other:?}"),
        }
    }

    /// An unarmed plan (None) never fires and stays None, for every
    /// lifecycle site.
    #[test]
    fn an_unarmed_plan_never_fires() {
        let mut plan = None;
        for site in [
            LifecycleFaultSite::SceneVertices,
            LifecycleFaultSite::SceneTexture,
            LifecycleFaultSite::SceneSampler,
            LifecycleFaultSite::SceneUniforms,
            LifecycleFaultSite::SceneDescriptors,
            LifecycleFaultSite::LitModule,
            LifecycleFaultSite::LitLayout,
            LifecycleFaultSite::LitPipeline,
            LifecycleFaultSite::ProxyModule,
            LifecycleFaultSite::ProxyLayout,
            LifecycleFaultSite::ProxyPipeline,
            LifecycleFaultSite::RainModule,
            LifecycleFaultSite::RainPipeline,
            LifecycleFaultSite::FrameUpload,
            LifecycleFaultSite::FramePresentSemaphore,
            LifecycleFaultSite::FrameRecord,
            LifecycleFaultSite::FrameSubmit,
            LifecycleFaultSite::RegionGrowthIdle,
            LifecycleFaultSite::InstanceGrowthIdle,
            LifecycleFaultSite::PoseGrowthIdle,
        ] {
            assert!(advance_lifecycle_fault_plan(&mut plan, site).is_none());
        }
        assert_eq!(plan, None);
    }
}
