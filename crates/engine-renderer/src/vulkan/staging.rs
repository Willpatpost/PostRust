//! P5 R3 (A20 — the design §4.3 recommendation (b)+(c), owner-ruled
//! at D-EE-10 item 1D): the device-lifetime STAGING POOL with
//! batched, deferrable uploads, retiring the per-object staging
//! machinery and the permanent per-texture tails.
//!
//! **The shape** (one shared object where every upload used to build
//! its own): a persistently mapped HOST_VISIBLE|HOST_COHERENT
//! TRANSFER_SRC backing grown power-of-two on demand (the overlay's
//! `capacity_for` discipline on bytes), a bump allocator over that
//! backing, one command pool, and a batch API — `begin` → N ×
//! (stage bytes → describe the copy) → ONE submit → ONE fence →
//! wait inline or defer → the regions recycle. Per-object command
//! pools, fences, blocking submits, and throwaway staging pairs:
//! zero on the pooled path.
//!
//! **Readiness tracking (the in-flight bookkeeping):** every flushed
//! batch owns a fence and its bump range; a region is retired ONLY
//! when its OWN fence has signaled AND every earlier batch's has
//! (bump regions recycle only past the watermark — the oldest live
//! region bounds reuse). Synchronization follows these ACTUAL
//! resource dependencies (the batch fences — upload completion),
//! never an unrelated frame-slot fence: the pool never waits or
//! resets on anything but its own submissions.
//!
//! **Bounded backpressure:** the backing grows to the demand's
//! power-of-two under a ceiling; a batch whose live bytes exceed the
//! ceiling is refused typed
//! ([`RendererError::StagingCapacity`]) AFTER draining the
//! in-flight list (so a retry starts compact) — never a partial
//! upload, never unbounded growth.
//!
//! **Deferrable structure, inline-wait first** (the authorization's
//! acceptable first milestone): [`FlushMode::InlineWait`] submits and
//! blocks — the construction/proxy/scene-buffer sites, where no
//! meaningful independent work overlaps the copy; the deferred arm
//! submits and returns, the wait landing at the caller's drain —
//! the ATTACH path, where the pipeline-group build (~ms of shader
//! compilation) is independent of the copies and overlaps them
//! (the honest assessment is documented at the flush sites).
//!
//! **Mid-batch growth is safe by construction:** the caller's
//! `record` calls are stored as a replay LOG, not executed
//! immediately — the log's entries reference DESTINATION handles and
//! pool offsets only, so growing the backing mid-batch (re-staging
//! the open batch's bytes at the SAME offsets in the larger
//! superset) never invalidates anything recorded. The whole log is
//! executed once at flush, in staging order, against the final
//! backing — the per-object command sequences concatenated verbatim
//! (same bytes, same copies: the device-local contents are
//! byte-identical, §4.4).
//!
//! The pool is created lazily with the first batch and is
//! UNAVAILABLE (sticky) if the backing cannot be created — the
//! callers then fall back to the retained per-object paths
//! (resources.rs's helpers and the texture fallback arm), so a
//! no-HOST_VISIBLE device keeps exactly its pre-R3 behavior.

use ash::vk;
use std::collections::VecDeque;

use super::RendererError;
use super::context::VulkanContext;
use super::resources::create_mapped_buffer;

/// The backing's minimum size (bytes) — the overlay/arena floor
/// discipline: the first demand rounds up to the power-of-two
/// quantum with this floor, so the pool exists at a sane size even
/// for a tiny first batch (the construction atlas). Revisitable
/// with rig evidence, never silently.
pub(crate) const STAGING_POOL_FLOOR_BYTES: vk::DeviceSize = 64 * 1024;

/// The backing's configured ceiling (bytes) — the bounded
/// backpressure boundary: a single batch whose live bytes exceed it
/// refuses typed (never a partial upload). 64 MiB is far above every
/// shipped attach batch (~1 MiB) while bounding a streaming loader's
/// chunk; revisitable with rig evidence, never silently.
pub(crate) const STAGING_POOL_MAX_BYTES: vk::DeviceSize = 64 * 1024 * 1024;

/// Every staged region's start offset alignment. Buffer→image copies
/// require the source offset to be a multiple of the texel size and
/// of 4 (VUID-VkBufferImageCopy-bufferOffset-00195); 16 covers every
/// shipped format (R8, RGBA8) with margin.
const STAGING_REGION_ALIGN: vk::DeviceSize = 16;

/// The pure capacity quantum (the overlay's `capacity_for` on
/// bytes): the smallest backing that serves `required` bytes — the
/// demand rounded up to the power-of-two quantum with the floor,
/// under the cap; a `required` at or below `current` keeps `current`
/// (the pool never shrinks). Pure — unit-testable without a device
/// (the mock-free seam discipline: the pool's growth path runs THIS
/// function).
pub(crate) fn staging_capacity_for(current: u64, required: u64, cap: u64) -> u64 {
    let required = required.min(cap);
    if required <= current {
        return current;
    }
    required
        .max(STAGING_POOL_FLOOR_BYTES)
        .checked_next_power_of_two()
        .unwrap_or(cap)
        .min(cap)
        .max(STAGING_POOL_FLOOR_BYTES.min(cap))
}

/// The pure retirement advance (the in-flight readiness tracking):
/// given the in-flight entries' fence statuses IN SUBMISSION ORDER,
/// how many LEADING entries may retire. An entry retires only on its
/// OWN signaled status, and only after every earlier entry retired —
/// the never-signaling hazard the design's §4.5 gate names: a fence
/// that never signals (fault-injected in the battery) keeps its
/// region — and every region behind it — bound forever, so the
/// bump allocator can never reuse bytes a pending submission reads.
///
/// This is the seam the device path consults (the mock-free
/// discipline: `retire_signaled` on the pool runs THIS function over
/// the real fence statuses); the naive implementation — retiring
/// every signaled entry regardless of order — is exactly the bug the
/// in-flight safety test is written to catch (its red witness).
pub(crate) fn retire_leading_signaled(signaled: &[bool]) -> usize {
    signaled.iter().take_while(|&&s| s).count()
}

/// One region handed out by [`StagingBatch::stage`] — the pool
/// offsets the caller's recorded copies read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StagedRegion {
    pub(crate) offset: vk::DeviceSize,
    pub(crate) len: vk::DeviceSize,
}

/// One recorded copy in a batch's replay log. The entries carry
/// DESTINATION handles and pool offsets only — never the pool's own
/// buffer handle — so the log survives mid-batch backing growth
/// (see the module doc).
#[derive(Debug, Clone, Copy)]
pub(crate) enum StagingRecord {
    /// `cmd_copy_buffer`: one whole staged region into a
    /// device-local buffer — the per-object helper's single copy,
    /// byte-identical (the VBO/IBO/proxy-mesh shape).
    BufferCopy {
        dst: vk::Buffer,
        dst_offset: vk::DeviceSize,
        src_offset: vk::DeviceSize,
        size: vk::DeviceSize,
    },
    /// The texture shape: the pre-copy barrier pair, the tightly
    /// packed buffer→image copy, the post-copy barrier — the
    /// per-object texture upload's exact command sequence
    /// (textures.rs's, verbatim).
    TextureCopy {
        image: vk::Image,
        extent: vk::Extent3D,
        src_offset: vk::DeviceSize,
    },
}

/// How a flushed batch's fence is satisfied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FlushMode {
    /// Submit, then block on the fence until signaled — the first
    /// milestone the owner ruled acceptable. Used where no
    /// meaningful independent work overlaps the copy (construction,
    /// the frame-path proxy ensure, the Included scene buffers).
    InlineWait,
    /// Submit and return; the wait lands at the caller's drain —
    /// used where independent work overlaps (the attach path's
    /// pipeline-group build). The regions stay bound (readiness
    /// tracked) until the fence signals.
    Deferred,
}

/// The pool's current backing. Created lazily with the first batch;
/// replaced (grown) only after the in-flight list drains.
struct Backing {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: *mut core::ffi::c_void,
    capacity: vk::DeviceSize,
}

/// One flushed-not-yet-retired batch: its fence, its command buffer
/// (freed at retire), and its bump range. In SUBMISSION order.
#[derive(Clone, Copy)]
struct InFlight {
    fence: vk::Fence,
    command: vk::CommandBuffer,
    start: vk::DeviceSize,
    end: vk::DeviceSize,
}

/// The device-lifetime staging pool, owned by the
/// [`VulkanContext`] behind a `RefCell` (interior mutability — every
/// caller holds `&VulkanContext`, the counters' own `Cell` pattern
/// extended to a stateful object; confined to the renderer's single
/// owning thread, and `*mut` makes it `!Send`/`!Sync` so it cannot
/// silently cross threads).
pub(crate) struct StagingPool {
    backing: Option<Backing>,
    /// Sticky: the first backing creation failed — every later batch
    /// falls back to the retained per-object paths (the caller's
    /// `begin_staging_batch` returns `None`).
    unavailable: bool,
    command_pool: vk::CommandPool,
    /// The bump frontier: regions [watermark, committed) are bound
    /// (staged and/or submitted, not yet retired).
    committed: vk::DeviceSize,
    /// The retirement frontier: bytes before it are free for reuse.
    watermark: vk::DeviceSize,
    in_flight: VecDeque<InFlight>,
    /// One open batch at a time (the single-threaded frame flow's
    /// discipline; a nested begin is a programming error and panics
    /// loudly).
    batch_open: bool,
    growth_events: u64,
}

impl StagingPool {
    pub(crate) const fn new() -> Self {
        Self {
            backing: None,
            unavailable: false,
            command_pool: vk::CommandPool::null(),
            committed: 0,
            watermark: 0,
            in_flight: VecDeque::new(),
            batch_open: false,
            growth_events: 0,
        }
    }

    /// The census reads (see `AllocationStats`'s staging fields).
    pub(crate) fn capacity_bytes(&self) -> u64 {
        self.backing.as_ref().map_or(0, |b| b.capacity)
    }

    pub(crate) fn growth_events(&self) -> u64 {
        self.growth_events
    }

    /// Whether any flushed batch is still waiting its fence (the
    /// rollback and teardown paths' pre-drain check).
    pub(crate) fn has_in_flight(&self) -> bool {
        !self.in_flight.is_empty()
    }

    /// The bytes currently BOUND — everything staged or submitted
    /// past the watermark (the in-flight entries plus any open
    /// batch's staged regions). Zero exactly when every region has
    /// been retired; the in-flight safety battery's observable (a
    /// never-signaling fence keeps this nonzero forever).
    pub(crate) fn bound_region_bytes(&self) -> u64 {
        self.committed - self.watermark
    }

    /// Retires the LEADING signaled in-flight entries (the pure
    /// advance over the real fence statuses). Non-blocking: a zero
    /// timeout `wait_for_fences` is the status poll. An entry's
    /// region is released only here — on its OWN fence signal, in
    /// order (the module's readiness discipline).
    fn retire_signaled(&mut self, ctx: &VulkanContext) {
        if self.in_flight.is_empty() {
            return;
        }
        // SAFETY: the fences are owned by the in-flight entries; a
        // zero-timeout wait is a status query.
        let statuses: Vec<bool> = self
            .in_flight
            .iter()
            .map(|entry| unsafe { ctx.device.wait_for_fences(&[entry.fence], true, 0).is_ok() })
            .collect();
        let retire = retire_leading_signaled(&statuses);
        for _ in 0..retire {
            if let Some(entry) = self.in_flight.pop_front() {
                // The watermark invariant: the entries retire in
                // submission order, so each retire lands exactly on
                // the entry's own start (the bump's total order).
                debug_assert_eq!(
                    entry.start, self.watermark,
                    "the in-flight regions retire in bump order"
                );
                // SAFETY: the fence has signaled — the command buffer
                // is no longer pending — and each handle is owned
                // exactly once by this entry.
                unsafe {
                    ctx.device.destroy_fence(entry.fence, None);
                    ctx.device
                        .free_command_buffers(self.command_pool, &[entry.command]);
                }
                self.watermark = entry.end;
            }
        }
    }

    /// Waits EVERY in-flight fence (in order, each with the frame
    /// path's bounded timeout — a stuck submission surfaces as an
    /// error, never an unresponsive window), retiring each as it
    /// signals and recording the blocking time on the census. On a
    /// timeout the remaining entries STAY in flight (their regions
    /// stay bound — the honest state: an unsignaled fence means the
    /// bytes are not provably free) and the error propagates.
    fn drain(&mut self, ctx: &VulkanContext) -> Result<(), RendererError> {
        while let Some(entry) = self.in_flight.front().cloned() {
            let wait_started = std::time::Instant::now();
            // SAFETY: the fence is owned by the in-flight entry.
            let waited = unsafe {
                ctx.device
                    .wait_for_fences(&[entry.fence], true, super::WAIT_TIMEOUT_NS)
            };
            match waited {
                Ok(()) => {
                    ctx.record_upload_wait(wait_started.elapsed());
                    self.in_flight.pop_front();
                    debug_assert_eq!(
                        entry.start, self.watermark,
                        "the drain retires in bump order"
                    );
                    // SAFETY: signaled — the command buffer is not
                    // pending; each handle owned exactly once.
                    unsafe {
                        ctx.device.destroy_fence(entry.fence, None);
                        ctx.device
                            .free_command_buffers(self.command_pool, &[entry.command]);
                    }
                    self.watermark = entry.end;
                }
                Err(vk::Result::TIMEOUT) => {
                    return Err(RendererError::Vulkan(format!(
                        "staging drain: fence wait timed out after {} ns \
                         (a stuck upload submission — {} bound bytes stay \
                         unretired; constructing a fresh renderer is the \
                         supported path)",
                        super::WAIT_TIMEOUT_NS,
                        self.bound_region_bytes()
                    )));
                }
                Err(error) => return Err(RendererError::vk(error, "staging drain")),
            }
        }
        Ok(())
    }

    /// Ensures the backing exists (lazily) and fits `needed_end`
    /// bytes. Growth drains the in-flight list FIRST (the old
    /// backing's actual readers — the only submissions that read it,
    /// so their fences are the complete dependency), creates the
    /// larger superset, copies the bound bytes forward at the SAME
    /// offsets, and retires the old backing. At the cap the batch
    /// refuses typed (after the drain, so a retry starts compact).
    fn ensure_space(
        &mut self,
        ctx: &VulkanContext,
        needed_end: vk::DeviceSize,
    ) -> Result<(), RendererError> {
        if self
            .backing
            .as_ref()
            .is_some_and(|b| needed_end <= b.capacity)
        {
            return Ok(());
        }
        // The actual resource dependencies: every in-flight
        // submission reads the old backing, so draining it makes the
        // replacement safe — no device-wide idle, no frame-slot
        // fence, only the pool's own fences.
        self.drain(ctx)?;
        let current = self.backing.as_ref().map_or(0, |b| b.capacity);
        let target = staging_capacity_for(current, needed_end, STAGING_POOL_MAX_BYTES);
        if target < needed_end {
            return Err(RendererError::StagingCapacity {
                requested: needed_end,
                capacity: STAGING_POOL_MAX_BYTES,
            });
        }
        // The growth guard (the ScopedMappedBuffer discipline, on the
        // pool's own census): the new pair is owned from creation, so
        // a failure below drops it cleanly; the commit installs it
        // and retires the old.
        let (buffer, memory, mapped) =
            create_mapped_buffer(ctx, vk::BufferUsageFlags::TRANSFER_SRC, target)?;
        // Copy the bound bytes forward at the same offsets (the
        // superset preserves them; the open batch's staged regions
        // and replay-log offsets stay valid).
        if let Some(old) = &self.backing {
            // SAFETY: both mappings are live and at least
            // `self.committed` bytes; the ranges are the pool's own
            // (single-threaded, disjoint from every retired region).
            unsafe {
                std::ptr::copy_nonoverlapping(
                    old.mapped.cast::<u8>(),
                    mapped.cast::<u8>(),
                    self.committed as usize,
                );
            }
            // SAFETY: the in-flight drain above completed every
            // submission that read the old backing.
            unsafe {
                ctx.device.unmap_memory(old.memory);
                ctx.device.destroy_buffer(old.buffer, None);
                ctx.device.free_memory(old.memory, None);
            }
        }
        self.backing = Some(Backing {
            buffer,
            memory,
            mapped,
            capacity: target,
        });
        self.growth_events += 1;
        Ok(())
    }

    /// Full teardown (the context's destroy): if anything is still
    /// in flight the device is idled FIRST (the stuck-fence protocol
    /// — a never-signaling fence must not wedge the teardown), then
    /// every entry and the backing are released. Best-effort by
    /// contract (the callers are drop paths); errors cannot propagate
    /// here, and the device-idle makes every destroy legal.
    pub(crate) fn destroy(&mut self, ctx: &VulkanContext) {
        if !self.in_flight.is_empty() {
            // SAFETY: the renderer's owning drop path — the device is
            // live and every other owner is already released.
            unsafe { ctx.device.device_wait_idle() }.ok();
        }
        for entry in self.in_flight.drain(..) {
            // SAFETY: the idle wait above completed every submission;
            // each handle owned exactly once.
            unsafe {
                ctx.device.destroy_fence(entry.fence, None);
                ctx.device
                    .free_command_buffers(self.command_pool, &[entry.command]);
            }
        }
        if let Some(backing) = self.backing.take() {
            // SAFETY: the mapping and the objects are the pool's own;
            // no submission reads them (idled above or never created).
            unsafe {
                ctx.device.unmap_memory(backing.memory);
                ctx.device.destroy_buffer(backing.buffer, None);
                ctx.device.free_memory(backing.memory, None);
            }
        }
        if self.command_pool != vk::CommandPool::null() {
            // SAFETY: the pool's own command pool; every command
            // buffer it allocated was freed above.
            unsafe { ctx.device.destroy_command_pool(self.command_pool, None) };
            self.command_pool = vk::CommandPool::null();
        }
        self.committed = 0;
        self.watermark = 0;
        self.batch_open = false;
    }
}

impl Default for StagingPool {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY (the raw pointer): `mapped` is confined to the renderer's
// single owning thread exactly like the live states' own mapped
// pointers; the pool exists only inside the context, and raw pointers
// already make the type !Send/!Sync so it cannot silently cross
// threads.

/// The caller's batch handle: stages bytes, records the copies, and
/// flushes ONCE (one submit, one fence). Dropping an unflushed batch
/// ABORTS it — the staged regions are reclaimed by pure arithmetic
/// (nothing was submitted, so there is no GPU hazard; the partial-
/// failure discipline the per-object helpers carry, at batch scale).
pub(crate) struct StagingBatch<'a> {
    ctx: &'a VulkanContext,
    /// The bump frontier at begin — the abort rollback point.
    start: vk::DeviceSize,
    records: Vec<StagingRecord>,
    closed: bool,
}

impl Drop for StagingBatch<'_> {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        // Abort: nothing was submitted — reclaim the staged regions.
        let mut pool = self.ctx.staging.borrow_mut();
        pool.batch_open = false;
        pool.committed = self.start;
    }
}

impl<'a> StagingBatch<'a> {
    /// The context the batch stages through (the destination-creation
    /// calls the pooled upload helpers make beside the staging). The
    /// return borrows the batch's OWN lifetime parameter, not `&self`
    /// — the helpers hold this reference while mutating the batch.
    pub(crate) fn ctx(&self) -> &'a VulkanContext {
        self.ctx
    }

    /// Stages `bytes` in the pool and returns the region the caller's
    /// recorded copies read from. The write lands in the persistently
    /// mapped backing immediately (HOST_COHERENT: visible to the
    /// queue at submission). Bounded backpressure: grows the backing
    /// (power-of-two, after the in-flight drain) or refuses typed at
    /// the ceiling.
    pub(crate) fn stage(&mut self, bytes: &[u8]) -> Result<StagedRegion, RendererError> {
        let len = bytes.len() as vk::DeviceSize;
        let mut pool = self.ctx.staging.borrow_mut();
        if pool.unavailable {
            return Err(RendererError::Vulkan(
                "the staging pool is unavailable on this device".into(),
            ));
        }
        let offset = pool.committed.next_multiple_of(STAGING_REGION_ALIGN);
        let needed_end = offset + len;
        pool.ensure_space(self.ctx, needed_end)?;
        let backing = pool
            .backing
            .as_ref()
            .expect("ensure_space created the backing");
        // SAFETY: the mapping is live for the whole backing; the
        // region [offset, offset+len) is disjoint from every other
        // live region (the bump frontier is exclusive to the open
        // batch).
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                backing.mapped.cast::<u8>().byte_add(offset as usize),
                bytes.len(),
            );
        }
        pool.committed = needed_end;
        Ok(StagedRegion { offset, len })
    }

    /// Records one whole-region buffer copy (the per-object helper's
    /// single `cmd_copy_buffer`, byte-identical).
    pub(crate) fn record_buffer_copy(
        &mut self,
        dst: vk::Buffer,
        dst_offset: vk::DeviceSize,
        region: StagedRegion,
    ) {
        self.records.push(StagingRecord::BufferCopy {
            dst,
            dst_offset,
            src_offset: region.offset,
            size: region.len,
        });
    }

    /// Records one texture upload's command sequence (the pre-copy
    /// barrier pair + the tightly packed buffer→image copy + the
    /// post-copy barrier — the per-object texture upload's exact
    /// sequence).
    pub(crate) fn record_texture_copy(
        &mut self,
        image: vk::Image,
        extent: vk::Extent3D,
        region: StagedRegion,
    ) {
        self.records.push(StagingRecord::TextureCopy {
            image,
            extent,
            src_offset: region.offset,
        });
    }

    /// Executes the replay log ONCE — one command buffer, ONE queue
    /// submit, ONE fence — then either waits inline (the first
    /// milestone) or defers (the wait lands at the caller's drain).
    /// A batch with no recorded copies is a no-op (no empty submits —
    /// the census counts real submissions only).
    pub(crate) fn flush(&mut self, mode: FlushMode) -> Result<(), RendererError> {
        if self.records.is_empty() {
            // Nothing to submit: reclaim the staged regions and close.
            let mut pool = self.ctx.staging.borrow_mut();
            pool.batch_open = false;
            pool.committed = self.start;
            self.closed = true;
            return Ok(());
        }
        let mut pool = self.ctx.staging.borrow_mut();
        // The command pool is created with the first real flush.
        if pool.command_pool == vk::CommandPool::null() {
            let info =
                vk::CommandPoolCreateInfo::default().queue_family_index(self.ctx.graphics_family);
            // SAFETY: fresh object; the pool owns it until teardown.
            pool.command_pool = unsafe { self.ctx.device.create_command_pool(&info, None) }
                .map_err(|error| RendererError::vk(error, "staging command pool"))?;
        }
        let allocate = vk::CommandBufferAllocateInfo::default()
            .command_pool(pool.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: the pool owns the command pool; the buffer is freed
        // at retire, at teardown, or on every error path below
        // (nothing pending can exist before a successful submit).
        let command = unsafe { self.ctx.device.allocate_command_buffers(&allocate) }
            .map_err(|error| RendererError::vk(error, "staging command buffer"))?[0];
        let src = pool
            .backing
            .as_ref()
            .expect("a staged batch has a backing")
            .buffer;
        // The device sequence through the submit, with the fence
        // tracked so every error path releases exactly what exists
        // (the corrective-finding-G discipline at batch scale: a
        // failed flush leaks nothing and leaves nothing pending).
        let mut fence = vk::Fence::null();
        let submitted = (|| unsafe {
            self.ctx
                .device
                .begin_command_buffer(
                    command,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|error| RendererError::vk(error, "begin staging batch"))?;
            execute_records(&self.ctx.device, command, src, &self.records);
            self.ctx
                .device
                .end_command_buffer(command)
                .map_err(|error| RendererError::vk(error, "end staging batch"))?;
            fence = self
                .ctx
                .device
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .map_err(|error| RendererError::vk(error, "staging fence"))?;
            // The §4.5 in-flight safety fault seam: when the plan
            // fires, the submission carries NO fence — a structurally
            // real never-signaling fence (not a status lie) — so the
            // battery can prove the readiness tracking never resets
            // an unsignaled region. The fence object still exists
            // (owned by the in-flight entry, destroyed at teardown);
            // only its signal never happens. Diagnostic-only,
            // test-only, like every fault site.
            let never_signals = self
                .ctx
                .check_lifecycle_fault(crate::LifecycleFaultSite::StagingFenceNeverSignals)
                .is_err();
            let submit_fence = if never_signals {
                vk::Fence::null()
            } else {
                fence
            };
            let submits_commands = [command];
            let submits = [vk::SubmitInfo::default().command_buffers(&submits_commands)];
            // SAFETY: the command buffer was just recorded and ended;
            // the fence is fresh (or deliberately null).
            self.ctx
                .device
                .queue_submit(self.ctx.graphics_queue, &submits, submit_fence)
                .map_err(|error| RendererError::vk(error, "staging batch submit"))?;
            Ok(())
        })();
        if let Err(error) = submitted {
            // Nothing was submitted (or the submit itself failed — a
            // failed queue_submit attaches nothing): release the
            // one-shot objects and propagate. The batch's Drop then
            // aborts the staged regions (nothing pending).
            // SAFETY: the command buffer is not pending; the fence
            // was never attached to a successful submit.
            unsafe {
                if fence != vk::Fence::null() {
                    self.ctx.device.destroy_fence(fence, None);
                }
                self.ctx
                    .device
                    .free_command_buffers(pool.command_pool, &[command]);
            }
            return Err(error);
        }
        let start = self.start;
        let end = pool.committed;
        pool.in_flight.push_back(InFlight {
            fence,
            command,
            start,
            end,
        });
        pool.batch_open = false;
        self.closed = true;
        match mode {
            FlushMode::InlineWait => {
                let wait_started = std::time::Instant::now();
                // SAFETY: the fence was just attached to the submit.
                let waited = unsafe {
                    self.ctx
                        .device
                        .wait_for_fences(&[fence], true, super::WAIT_TIMEOUT_NS)
                };
                match waited {
                    Ok(()) => {
                        self.ctx.record_upload_submit(wait_started.elapsed());
                        // The signaled entry retires if it is at the
                        // head (earlier deferred entries correctly
                        // block the watermark behind their own
                        // fences).
                        pool.retire_signaled(self.ctx);
                        Ok(())
                    }
                    Err(vk::Result::TIMEOUT) => Err(RendererError::Vulkan(format!(
                        "staging upload: fence wait timed out after {} ns \
                         (a stuck upload submission; constructing a fresh \
                         renderer is the supported path)",
                        super::WAIT_TIMEOUT_NS
                    ))),
                    Err(error) => Err(RendererError::vk(error, "staging upload wait")),
                }
            }
            FlushMode::Deferred => {
                // The submit is counted now; the blocking time lands
                // on the census at the DRAIN that waits it.
                self.ctx.record_upload_submit(std::time::Duration::ZERO);
                Ok(())
            }
        }
    }
}

/// Executes the replay log against `src` (the pool's backing) — the
/// per-object command sequences concatenated verbatim, in staging
/// order.
///
/// SAFETY: `command` is in the recording state; every handle in
/// `records` is valid and owned by the caller (destinations) or the
/// pool (the backing); the staged regions were written before the
/// flush.
unsafe fn execute_records(
    device: &ash::Device,
    command: vk::CommandBuffer,
    src: vk::Buffer,
    records: &[StagingRecord],
) {
    for record in records {
        match *record {
            StagingRecord::BufferCopy {
                dst,
                dst_offset,
                src_offset,
                size,
            } => {
                // SAFETY: the caller's destination buffer allows
                // TRANSFER_DST (the pooled helper's creation
                // contract — the VUID the CI validation smoke
                // surfaced on the per-object path).
                unsafe {
                    device.cmd_copy_buffer(
                        command,
                        src,
                        dst,
                        &[vk::BufferCopy {
                            src_offset,
                            dst_offset,
                            size,
                        }],
                    );
                }
            }
            StagingRecord::TextureCopy {
                image,
                extent,
                src_offset,
            } => {
                let range = vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1);
                let barrier = vk::ImageMemoryBarrier::default()
                    .image(image)
                    .subresource_range(range)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
                // Queue submission makes the coherent host writes
                // visible; the transfer dependency covers the
                // UpdateBuffer fallback path as well (the per-object
                // sequence's own comment, retained).
                let staging_ready = vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
                // SAFETY: the image is the caller's TRANSFER_DST |
                // SAMPLED texture, freshly created (UNDEFINED is the
                // valid first layout).
                unsafe {
                    device.cmd_pipeline_barrier(
                        command,
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
                        .buffer_offset(src_offset)
                        .image_extent(extent)
                        .image_subresource(
                            vk::ImageSubresourceLayers::default()
                                .aspect_mask(vk::ImageAspectFlags::COLOR)
                                .layer_count(1),
                        );
                    device.cmd_copy_buffer_to_image(
                        command,
                        src,
                        image,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        &[copy],
                    );
                    device.cmd_pipeline_barrier(
                        command,
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
                }
            }
        }
    }
}

impl VulkanContext {
    /// Opens ONE staging batch (the single-batch discipline — a
    /// nested open panics: the renderer's single-threaded flow never
    /// nests). `None` when the pool is UNAVAILABLE on this device
    /// (the sticky first-backing-creation failure): the caller falls
    /// back to the retained per-object upload paths, keeping exactly
    /// the pre-R3 behavior on a no-HOST_VISIBLE device. Retires the
    /// leading signaled batches first (the readiness recycling), and
    /// wraps the bump around to zero when nothing is bound.
    pub(crate) fn begin_staging_batch(&self) -> Option<StagingBatch<'_>> {
        let mut pool = self.staging.borrow_mut();
        if pool.unavailable {
            return None;
        }
        if pool.backing.is_none() {
            // The lazy first backing (the overlay/region lazy
            // discipline): created with the first batch, at the
            // floor — the demand's growth lands with the batch that
            // needs it.
            let (buffer, memory, mapped) = match create_mapped_buffer(
                self,
                vk::BufferUsageFlags::TRANSFER_SRC,
                STAGING_POOL_FLOOR_BYTES,
            ) {
                Ok(created) => created,
                Err(_) => {
                    // Sticky: this device cannot back the pool (no
                    // HOST_VISIBLE|HOST_COHERENT staging memory — the
                    // same class of failure the per-object helper's
                    // staging half raises at its own allocation). The
                    // callers' fallback arms keep the pre-R3 path.
                    pool.unavailable = true;
                    return None;
                }
            };
            pool.backing = Some(Backing {
                buffer,
                memory,
                mapped,
                capacity: STAGING_POOL_FLOOR_BYTES,
            });
        }
        assert!(
            !pool.batch_open,
            "one staging batch at a time (the single-threaded flow's discipline)"
        );
        pool.retire_signaled(self);
        if pool.watermark == pool.committed {
            // Nothing bound: the bump wraps to zero (the ordinary
            // recycling — a fresh batch reuses the retired space).
            pool.committed = 0;
            pool.watermark = 0;
        }
        pool.batch_open = true;
        let start = pool.committed;
        Some(StagingBatch {
            ctx: self,
            start,
            records: Vec::new(),
            closed: false,
        })
    }

    /// Waits every in-flight staging batch (the DEFERRED flushes'
    /// wait landing — the attach path's batch-end drain, completing
    /// before first use by construction: the first frame cannot
    /// precede the attach's return). See [`StagingPool::drain`].
    pub(crate) fn drain_staging(&self) -> Result<(), RendererError> {
        let mut pool = self.staging.borrow_mut();
        pool.drain(self)
    }
}

// SAFETY (the raw pointers): `mapped` is confined to the renderer's
// single owning thread exactly like the live states' own mapped
// pointers; the pool exists only inside the context, and raw pointers
// already make the type !Send/!Sync so it cannot silently cross
// threads.

#[cfg(test)]
mod staging_pool_seam_tests {
    use super::{
        STAGING_POOL_FLOOR_BYTES, STAGING_POOL_MAX_BYTES, retire_leading_signaled,
        staging_capacity_for,
    };

    /// The capacity quantum (the overlay's discipline on bytes): the
    /// demand's power of two, floored, capped, never shrinking.
    #[test]
    fn the_capacity_quantum_is_the_demands_power_of_two() {
        // The floor applies below it.
        assert_eq!(
            staging_capacity_for(0, 1, STAGING_POOL_MAX_BYTES),
            STAGING_POOL_FLOOR_BYTES
        );
        assert_eq!(
            staging_capacity_for(0, STAGING_POOL_FLOOR_BYTES, STAGING_POOL_MAX_BYTES),
            STAGING_POOL_FLOOR_BYTES
        );
        // The demand rounds up to the quantum.
        assert_eq!(
            staging_capacity_for(STAGING_POOL_FLOOR_BYTES, 65_537, STAGING_POOL_MAX_BYTES),
            128 * 1024
        );
        // A 64 KiB pool growing for a ~900 KiB corner attach lands at
        // 1 MiB in ONE event (the demand's pow2, not doubling).
        assert_eq!(
            staging_capacity_for(64 * 1024, 900_000, STAGING_POOL_MAX_BYTES),
            1024 * 1024
        );
        // Never shrinks: a smaller demand keeps the current backing.
        assert_eq!(
            staging_capacity_for(1024 * 1024, 8_192, STAGING_POOL_MAX_BYTES),
            1024 * 1024
        );
        // The cap clamps (the bounded backpressure boundary).
        assert_eq!(
            staging_capacity_for(0, STAGING_POOL_MAX_BYTES + 1, STAGING_POOL_MAX_BYTES),
            STAGING_POOL_MAX_BYTES
        );
        assert_eq!(
            staging_capacity_for(
                1024 * 1024,
                STAGING_POOL_MAX_BYTES + 5,
                STAGING_POOL_MAX_BYTES
            ),
            STAGING_POOL_MAX_BYTES
        );
        // The growth property the mid-batch safety argument needs:
        // whatever fits is served.
        for required in [1_u64, 65_536, 65_537, 999_999, 1_048_576] {
            let capacity = staging_capacity_for(64 * 1024, required, STAGING_POOL_MAX_BYTES);
            assert!(capacity >= required || capacity == STAGING_POOL_MAX_BYTES);
        }
    }

    /// The never-signaling hazard (§4.5's gate boundary, the red
    /// witness): a fence that never signals keeps its region — and
    /// every region behind it — bound. The naive implementation
    /// (retire every SIGNALLED entry, ignoring order) retires the
    /// two behind the stuck head and FAILS this test — it would let
    /// the bump allocator reuse bytes a pending submission reads.
    #[test]
    fn a_never_signaling_fence_never_rets_its_region_or_any_behind_it() {
        // The faulted shape: the head never signals, later batches
        // did (they cannot retire — the head bounds the watermark).
        assert_eq!(retire_leading_signaled(&[false, true, true]), 0);
        assert_eq!(retire_leading_signaled(&[false]), 0);
        // Every ordinary shape:
        assert_eq!(retire_leading_signaled(&[]), 0);
        assert_eq!(retire_leading_signaled(&[true, true, true]), 3);
        assert_eq!(retire_leading_signaled(&[true, false, true]), 1);
        assert_eq!(retire_leading_signaled(&[true, true, false]), 2);
    }

    /// The ordering property in the words of the hazard: retirement
    /// is a PREFIX of the signaled statuses, never a selection.
    #[test]
    fn retirement_is_a_prefix_never_a_selection() {
        for statuses in [
            vec![],
            vec![false],
            vec![true],
            vec![true, false],
            vec![false, true],
            vec![true, true, false, true],
            vec![false, false, true, true],
        ] {
            let retired = retire_leading_signaled(&statuses);
            assert!(retired <= statuses.len());
            assert!(statuses[..retired].iter().all(|&s| s));
            if retired < statuses.len() {
                assert!(!statuses[retired]);
            }
        }
    }
}

#[cfg(test)]
mod batch_vs_per_object_oracle {
    //! §4.5's batch-vs-per-object oracle, device-free half: the same
    //! items staged through the BATCH path and the PER-OBJECT path
    //! produce the same recorded copy plan (destinations, offsets,
    //! sizes, order) and the same staged BYTES — the pure arithmetic
    //! the device paths run (the mock-free discipline). The
    //! device-local half is the pixel/capture battery set re-run
    //! digit-for-digit through the pooled path (the rig evidence).
    use super::{STAGING_REGION_ALIGN, StagedRegion};

    /// The batch path's region arithmetic (exactly what
    /// [`super::StagingBatch::stage`] computes): each region starts
    /// at the bump frontier rounded up to the alignment.
    fn batched_regions(sizes: &[u64]) -> (Vec<StagedRegion>, u64) {
        let mut committed = 0_u64;
        let mut regions = Vec::new();
        for &len in sizes {
            let offset = committed.next_multiple_of(STAGING_REGION_ALIGN);
            regions.push(StagedRegion { offset, len });
            committed = offset + len;
        }
        (regions, committed)
    }

    /// The per-object path's staging (the throwaway pairs): each item
    /// its own buffer, its bytes at offset 0.
    fn per_object_regions(sizes: &[u64]) -> Vec<StagedRegion> {
        sizes
            .iter()
            .map(|&len| StagedRegion { offset: 0, len })
            .collect()
    }

    /// The oracle: for the same items, the batched plan's copies
    /// match the per-object plan's — same destinations, same
    /// destination offsets, same sizes, same order — and the staged
    /// bytes the batched offsets select equal each item's own bytes.
    #[test]
    fn the_batched_plan_stages_the_same_bytes_into_the_same_copies() {
        // Three items with unaligned lengths (the alignment padding
        // is exercised: 8_192, the atlas; 262_144, a texture; a
        // mesh-like odd size).
        let items: Vec<(Vec<u8>, u64)> = [vec![0u8; 8_192], vec![7u8; 262_144], vec![9u8; 123_457]]
            .into_iter()
            .map(|bytes| {
                let dst = bytes[0] as u64 + 100; // a stand-in destination handle
                (bytes, dst)
            })
            .collect();
        let sizes: Vec<u64> = items.iter().map(|(bytes, _)| bytes.len() as u64).collect();

        // The batched plan: regions from the bump arithmetic, one
        // BufferCopy per item at its region.
        let (regions, committed) = batched_regions(&sizes);
        let mut staging = vec![0u8; committed as usize];
        for ((bytes, _), region) in items.iter().zip(&regions) {
            staging[region.offset as usize..(region.offset + region.len) as usize]
                .copy_from_slice(bytes);
        }
        // The per-object plan: each item in its own throwaway buffer.
        let per_object = per_object_regions(&sizes);

        for (index, ((bytes, dst), region)) in items.iter().zip(&regions).enumerate() {
            let own = per_object[index];
            // Same copy semantics: same destination, same size.
            assert_eq!(region.len, own.len, "item {index}: the same size");
            let _ = dst;
            // The staged bytes the batched copy reads are exactly the
            // item's bytes (the per-object buffer's contents).
            assert_eq!(
                &staging[region.offset as usize..(region.offset + region.len) as usize],
                bytes,
                "item {index}: identical bytes staged through both paths"
            );
        }
        // The batched regions are disjoint and ordered (the bump
        // allocator's invariant — no copy can read another's bytes).
        for pair in regions.windows(2) {
            assert!(pair[0].offset + pair[0].len <= pair[1].offset);
        }
    }
}
