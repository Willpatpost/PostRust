//! Frame synchronization and commands (P4 S4 — design §5.1's frame
//! row): `FrameState` — per-slot command buffers, fences, per-image
//! present-wait semaphores, timestamps, overlay slots — the frame
//! outcome, and the full wait -> acquire -> (recreate) -> record ->
//! submit -> present flow (`render_frame`, the coordinator's host
//! half). The S4 split was a pure move from vulkan.rs; A04 (design
//! §3.4 + §5.4) then collapsed the flow's parameter set and named
//! its phases — no behavior change in either step:
//!
//! **The §3.4 collapse (19 → 6):** the four owning states stay four
//! explicit `&mut` parameters (the ownership argument the original
//! parameter-list comment defended — the design agrees with it and
//! disagrees only about the input half); the fifteen input/policy
//! parameters collapsed into TWO values: [`FramePolicy`]
//! (construction-time config, `Copy`) and [`InternalFrame`] (the
//! validated, repacked submission one level down, plus the [`FramePlan`]
//! derived from it — a borrow bundle). The same Options and slices
//! flow through, named by one struct instead of positional
//! parameters; the preparation pair and `recreate_swapchain` take
//! the same two values (the policy one level down each time).
//!
//! **The §5.4 phases:** `render_frame` is now the phase ladder in
//! call order — `validate_proxy_materials` → `prepare_frame` (the
//! S4-extracted preparation pair) → `wait_slot_fence` →
//! `acquire_with_retry` → `guard_image_and_read_timestamps` →
//! `upload_frame_data` → `present_semaphore_for` →
//! `record_command_buffer` → `commit_slot_fence` →
//! `submit_command_buffer` → `present_image`, with phases 5-8
//! grouped in [`acquire_to_submit`] and every error inside them
//! routed through [`dispose_failed_acquisition`] (CA-10's
//! acquisition disposition — the slot fence's reset is DEFERRED to
//! the commit between record and submit, so no post-acquire failure
//! can strand the slot). Each phase is a
//! private function of this module — not a phase trait, not
//! extensible at runtime; the ordering is the code. Where S3/S4 had
//! already landed a piece of a phase (the preparation pair, the
//! per-family record functions, the plan), the phase below is the
//! remaining wiring only — each phase's doc names the overlap.
//!
//! **Zero-cost statement (§3.4):** [`FramePolicy`] and
//! [`InternalFrame`] are `Copy` borrow bundles — no `Vec`, no `Box`,
//! no trait objects; construction is stack assembly the compiler
//! elides. No per-frame heap allocation and no dyn dispatch were
//! introduced anywhere in the flow. The behavioral half of the proof
//! is the batteries' identical `FrameOutcome`s and allocation
//! counters (the A04 gate set); the type-level half is
//! `policy_tests::the_collapsed_frame_parameters_are_copy_stack_data`.

use ash::vk;

use super::RendererError;
use super::WAIT_TIMEOUT_NS;
use super::capture::{CaptureState, ShadowReadbackState};
use super::context::VulkanContext;
use super::frame_plan::FramePlan;
use super::frame_prepare::{FramePreparationResult, prepare_after_recreation, prepare_frame};
use super::pipelines::PipelineState;
use super::resources::create_mapped_buffer;
use super::swapchain::{SwapchainState, mark_suboptimal};
use super::uploads::{LayerPriority, upload_layers};
use super::{FenceTimeoutClass, classify_fence_timeout};
use super::{corner, post_draw, proxy_draw, scene_record};
use crate::config::{DemoScenePreference, DynamicClamp, FRAMES_IN_FLIGHT, PresentModePreference};
use crate::overlay::OverlayVertex;
use crate::proxy_instances::population_write_needed;
use crate::submission::FrameInputs;

/// A single frame slot owns this mapping. The slot fence must have
/// signaled before growing or writing it.
#[derive(Default)]
struct OverlaySlot {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: *mut core::ffi::c_void,
    capacity: u32,
}

impl OverlaySlot {
    fn capacity_for(current: u32, required: usize) -> u32 {
        let limit = crate::overlay::overlay_capacity_vertices() / FRAMES_IN_FLIGHT;
        let required = required.min(limit as usize) as u32;
        if required <= current {
            return current;
        }
        required.next_power_of_two().min(limit)
    }

    fn reserve(&mut self, ctx: &VulkanContext, required: usize) -> Result<(), RendererError> {
        let capacity = Self::capacity_for(self.capacity, required);
        if capacity != self.capacity {
            let bytes = u64::from(capacity) * size_of::<OverlayVertex>() as u64;
            let (buffer, memory, mapped) =
                create_mapped_buffer(ctx, vk::BufferUsageFlags::VERTEX_BUFFER, bytes)?;
            self.destroy(ctx);
            *self = Self {
                buffer,
                memory,
                mapped,
                capacity,
            };
        }
        Ok(())
    }

    fn destroy(&mut self, ctx: &VulkanContext) {
        if self.capacity > 0 {
            unsafe {
                ctx.device.unmap_memory(self.memory);
                ctx.device.destroy_buffer(self.buffer, None);
                ctx.device.free_memory(self.memory, None);
            }
            *self = Self::default();
        }
    }
}

/// Sync objects, command buffers, and the timestamp pool, per frames-in-
/// flight slot, plus per-swapchain-image state.
pub(crate) struct FrameState {
    overlay_slots: [OverlaySlot; FRAMES_IN_FLIGHT as usize],
    /// P5 §3.3 (A18/R2): the per-slot dirty-generation stamps — the
    /// write half of the dirty-generation protocol. Each stamp
    /// records the population generation whose bytes that slot's
    /// mapped region last consumed, written ONLY together with those
    /// bytes (the upload site), so a matching nonzero generation
    /// PROVES the region already holds byte-identical bytes — the
    /// ping-pong correctness by construction (with
    /// FRAMES_IN_FLIGHT = 2 a stable population pays one write per
    /// slot per generation and then zero). 0 = unknown (the initial
    /// state; what every always-dirty generation-0 write resets to;
    /// and what every resource replacement clears — swapchain
    /// recreation and arena growth both replace the mapped buffers
    /// with no content copy).
    pub population_stamps: PopulationStamps,
    /// Created with `RESET_COMMAND_BUFFER` (corrective finding D): the
    /// per-slot command buffers are re-recorded every frame, and the
    /// per-slot fence proves a buffer is no longer pending when its
    /// re-record begins.
    pub command_pool: vk::CommandPool,
    pub command_buffers: Vec<vk::CommandBuffer>,
    pub image_available: Vec<vk::Semaphore>,
    /// Presentation-wait semaphores indexed by **swapchain image**
    /// (corrective finding C): the submit fence proves the graphics work
    /// finished, never that the presentation engine stopped waiting on
    /// the present semaphore — slot-indexed reuse could therefore signal
    /// a semaphore with an outstanding wait. Indexing by image ties reuse
    /// to re-acquisition of that image. Grown on demand; released only
    /// after `device_wait_idle`.
    pub present_semaphores: Vec<vk::Semaphore>,
    pub in_flight: Vec<vk::Fence>,
    pub images_in_flight: Vec<vk::Fence>,
    pub timestamp_pool: vk::QueryPool,
    /// False when the graphics queue family exposes no timestamp bits
    /// (corrective finding E).
    pub timestamps_supported: bool,
    /// Whether this slot's queries have resolved at least once (finding
    /// E): never block-read unwritten queries.
    pub timestamps_written: [bool; FRAMES_IN_FLIGHT as usize],
    /// A present reported suboptimal — recreate before the next frame.
    pub needs_recreate: bool,
    /// The 6-F shadow-map readback handshake (single-flight, like
    /// the color capture; survives pipeline replacements).
    pub(crate) shadow_readback: ShadowReadbackState,
    /// Hint-gated surface probing (perf batch 3): true while the
    /// surface's drawable is believed unavailable (the previous frame
    /// deferred), so minimized/transitional surfaces keep probing
    /// until valid while ordinary frames skip the per-frame driver
    /// round-trip entirely.
    pub surface_probe_pending: bool,
    pub frame_slot: usize,
    pub image_index: u32,
    /// The bounded frame-capture readback (lazily allocated on first
    /// request — the orientation corrective's evidence path).
    pub capture: CaptureState,
}

/// The per-slot population stamps (P5 §3.3, A18/R2): one generation
/// record per frame slot per population region — the instance bytes,
/// the pose blocks, and the 6-E2 indirect command block (which rides
/// the INSTANCE generation; its bytes are a pure function of the
/// per-kind counts). The gate itself is
/// [`crate::proxy_instances::generation_write_needed`] — ONE pure
/// decision function shared by this frame state, the upload site,
/// and the census's write leg; the oracle harness at this module's
/// bottom pins the whole protocol against the always-write path.
///
/// **T05 (strict audit IV):** the indirect stamp carries its own
/// per-kind count record (`indirect_counts`) — the indirect-count
/// identity is SEPARATE from the payload identity. A submission can
/// present byte-identical instance bytes under a different
/// npc/vehicle/rain partition (the byte-only oracle's blind spot);
/// the indirect gate then writes on the count change even at a
/// matching generation, because the command block's bytes — the
/// per-kind counts — moved while every byte-length stayed green.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PopulationStamps {
    pub instance: [u64; FRAMES_IN_FLIGHT as usize],
    pub pose: [u64; FRAMES_IN_FLIGHT as usize],
    pub indirect: [u64; FRAMES_IN_FLIGHT as usize],
    /// T05: the per-kind counts each slot's indirect block was last
    /// written with ([npc, vehicle, rain]) — the indirect-count
    /// identity beside the generation.
    pub indirect_counts: [[u32; 3]; FRAMES_IN_FLIGHT as usize],
    /// T02 (strict audit IV, §2.3): the per-slot EPOCH halves — the
    /// allocation epoch of the carve each stamp's bytes were written
    /// INTO, recorded together with those bytes at the upload site.
    /// 0 = unknown (never equal to a minted epoch, which start at 1):
    /// a REPLACED carve (a growth commit's or a pipeline
    /// replacement's fresh publication) can never inherit a previous
    /// identity's stamp — the resource-window half of the gate
    /// [`crate::proxy_instances::population_write_needed`], the
    /// correctness bearer the audit's delayed-invalidation finding
    /// demanded (T05's submission-time clears close the identity
    /// window; the two compose).
    pub instance_epochs: [u64; FRAMES_IN_FLIGHT as usize],
    pub pose_epochs: [u64; FRAMES_IN_FLIGHT as usize],
    pub indirect_epochs: [u64; FRAMES_IN_FLIGHT as usize],
}

impl Default for PopulationStamps {
    fn default() -> Self {
        Self {
            instance: [0; FRAMES_IN_FLIGHT as usize],
            pose: [0; FRAMES_IN_FLIGHT as usize],
            indirect: [0; FRAMES_IN_FLIGHT as usize],
            indirect_counts: [[0; 3]; FRAMES_IN_FLIGHT as usize],
            instance_epochs: [0; FRAMES_IN_FLIGHT as usize],
            pose_epochs: [0; FRAMES_IN_FLIGHT as usize],
            indirect_epochs: [0; FRAMES_IN_FLIGHT as usize],
        }
    }
}

impl PopulationStamps {
    /// RESOURCE-REPLACEMENT INVALIDATION (P5 §3.3's mandated
    /// correctness rule): clear every stamp to unknown. Called at
    /// the two replacement sites — swapchain recreation (the
    /// replacement pipeline state re-creates the instance and
    /// indirect buffers with NO content copy) and arena growth (the
    /// ensure ladder's `device_wait_idle` re-creation of a mapped
    /// carve at a larger size, again with no content copy). After a
    /// replacement, no stamp can vouch for any slot's content, so
    /// every population write re-fires for one frame — correctness
    /// first, the growth events' log-bounded frequency keeps the
    /// cost negligible.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// T05 (strict audit IV) — the INSTANCE half's REPACK
    /// invalidation: no slot's instance or indirect stamp may keep
    /// vouching after the CPU half regenerated the retained bytes.
    /// The hazard is the COLLIDING generation: a second source's own
    /// counter can present the SAME nonzero number the stamps
    /// already record, so the bare-generation gate would skip and
    /// the mapped regions would keep the FIRST source's bytes. The
    /// renderer calls this on every repacked instance half (the
    /// [`crate::submission::PopulationRepacked`] signal), at
    /// SUBMISSION time — before the frame flow, so a deferred or
    /// failed frame has already invalidated (a later frame of the
    /// new source re-admits through the CPU gate and writes both
    /// slots). Every slot is cleared: the OTHER slot's stamp is
    /// exactly the one that can equal the colliding generation. One
    /// redundant rewrite per slot per regeneration event — a bump
    /// already rewrote every slot once under the bare-generation
    /// rule, so single-source write counts do not move.
    pub(crate) fn clear_instance(&mut self) {
        self.instance = [0; FRAMES_IN_FLIGHT as usize];
        self.indirect = [0; FRAMES_IN_FLIGHT as usize];
        self.indirect_counts = [[0; 3]; FRAMES_IN_FLIGHT as usize];
        self.instance_epochs = [0; FRAMES_IN_FLIGHT as usize];
        self.indirect_epochs = [0; FRAMES_IN_FLIGHT as usize];
    }

    /// T05 — the POSE half's repack invalidation (the instance
    /// method's mirror, scoped to the pose stamps alone: the
    /// garage's split keeps the static instance half's skips while
    /// the advancing pose half rewrites).
    pub(crate) fn clear_pose(&mut self) {
        self.pose = [0; FRAMES_IN_FLIGHT as usize];
        self.pose_epochs = [0; FRAMES_IN_FLIGHT as usize];
    }
}

impl FrameState {
    pub(crate) fn new(
        ctx: &VulkanContext,
        swapchain_image_count: u32,
    ) -> Result<FrameState, RendererError> {
        // Partial-initialization cleanup (corrective finding G).
        let mut state = FrameState {
            overlay_slots: std::array::from_fn(|_| OverlaySlot::default()),
            population_stamps: PopulationStamps::default(),
            command_pool: vk::CommandPool::null(),
            command_buffers: Vec::new(),
            image_available: Vec::new(),
            present_semaphores: Vec::new(),
            in_flight: Vec::new(),
            images_in_flight: vec![vk::Fence::null(); swapchain_image_count as usize],
            timestamp_pool: vk::QueryPool::null(),
            timestamps_supported: ctx.timestamp_valid_bits > 0,
            timestamps_written: [false; FRAMES_IN_FLIGHT as usize],
            needs_recreate: false,
            surface_probe_pending: false,
            shadow_readback: ShadowReadbackState::default(),
            frame_slot: 0,
            image_index: 0,
            capture: CaptureState::dead(),
        };
        if let Err(error) = state.build(ctx, swapchain_image_count) {
            state.destroy(ctx);
            return Err(error);
        }
        Ok(state)
    }

    fn build(
        &mut self,
        ctx: &VulkanContext,
        swapchain_image_count: u32,
    ) -> Result<(), RendererError> {
        let pool_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
            .queue_family_index(ctx.graphics_family);
        self.command_pool = unsafe { ctx.device.create_command_pool(&pool_info, None) }
            .map_err(|error| RendererError::vk(error, "create_command_pool"))?;
        let allocate_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(FRAMES_IN_FLIGHT);
        self.command_buffers = unsafe { ctx.device.allocate_command_buffers(&allocate_info) }
            .map_err(|error| RendererError::vk(error, "allocate_command_buffers"))?;

        for _ in 0..FRAMES_IN_FLIGHT {
            let semaphore = unsafe {
                ctx.device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
            }
            .map_err(|error| RendererError::vk(error, "create_semaphore"))?;
            self.image_available.push(semaphore);
            let fence = unsafe {
                ctx.device.create_fence(
                    &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                    None,
                )
            }
            .map_err(|error| RendererError::vk(error, "create_fence"))?;
            self.in_flight.push(fence);
        }
        self.sync_present_semaphores(ctx, swapchain_image_count)?;

        if self.timestamps_supported {
            let query_info = vk::QueryPoolCreateInfo::default()
                .query_type(vk::QueryType::TIMESTAMP)
                .query_count(FRAMES_IN_FLIGHT * 2);
            self.timestamp_pool = unsafe { ctx.device.create_query_pool(&query_info, None) }
                .map_err(|error| RendererError::vk(error, "create_query_pool"))?;
        }
        Ok(())
    }

    /// Grows the per-image presentation-wait semaphore table (corrective
    /// finding C). Entries are never removed — recreation idles the
    /// device first, so outstanding present waits have drained.
    fn sync_present_semaphores(
        &mut self,
        ctx: &VulkanContext,
        image_count: u32,
    ) -> Result<(), RendererError> {
        let needed = present_semaphore_len_for(self.present_semaphores.len(), image_count as usize);
        while self.present_semaphores.len() < needed {
            let semaphore = unsafe {
                ctx.device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
            }
            .map_err(|error| RendererError::vk(error, "create present semaphore"))?;
            self.present_semaphores.push(semaphore);
        }
        Ok(())
    }

    /// Returns the present-wait semaphore for the acquired image
    /// (corrective finding C), growing the table if the index exceeds it.
    fn present_semaphore_for(
        &mut self,
        ctx: &VulkanContext,
        image_index: u32,
    ) -> Result<vk::Semaphore, RendererError> {
        // CA-10 fault knob: the PRESENT-SEMAPHORE boundary (Phase
        // 6.5, post-acquire) — a plan armed here fires before the
        // creation, with the slot fence still signaled and nothing
        // recorded (the fail-fast position this growth already
        // holds).
        ctx.check_lifecycle_fault(crate::LifecycleFaultSite::FramePresentSemaphore)?;
        self.sync_present_semaphores(ctx, image_index + 1)?;
        Ok(self.present_semaphores[image_index as usize])
    }

    /// Re-syncs the per-image tables for a new swapchain (the image count
    /// may change across recreation).
    pub(crate) fn resize_images(
        &mut self,
        ctx: &VulkanContext,
        swapchain_image_count: u32,
    ) -> Result<(), RendererError> {
        self.images_in_flight = vec![vk::Fence::null(); swapchain_image_count as usize];
        self.sync_present_semaphores(ctx, swapchain_image_count)
    }

    pub(crate) fn destroy(&mut self, ctx: &VulkanContext) {
        unsafe {
            ctx.device.destroy_query_pool(self.timestamp_pool, None);
            for fence in &self.in_flight {
                ctx.device.destroy_fence(*fence, None);
            }
            self.in_flight.clear();
            for semaphore in &self.present_semaphores {
                ctx.device.destroy_semaphore(*semaphore, None);
            }
            self.present_semaphores.clear();
            for semaphore in &self.image_available {
                ctx.device.destroy_semaphore(*semaphore, None);
            }
            self.image_available.clear();
            self.capture.destroy(ctx);
            for overlay in &mut self.overlay_slots {
                overlay.destroy(ctx);
            }
            ctx.device.destroy_command_pool(self.command_pool, None);
        }
    }
}

/// Outcome of one `render_frame` call.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct FrameOutcome {
    pub presented: bool,
    pub recreated: bool,
    pub suboptimal: bool,
    /// The drawable was temporarily unavailable (zero-sized surface
    /// capabilities) — nothing was created, destroyed, or presented.
    pub deferred: bool,
    pub gpu_render_ms: Option<f64>,
    /// F-03 Step 1: the frame's region-scene and overlay capacity
    /// decisions (supplied / per-slot capacity / drawn / omitted). Both
    /// stay default — all zero — on deferred or minimized frames, which
    /// record nothing; the overlay capacity is the current slot's
    /// allocation, while the region capacity
    /// reads 0 until the lazy region buffer allocates. Nonzero omitted
    /// counts are REPORTED upstream, never fatal here.
    pub region: DynamicClamp,
    pub foreground_drawn: u32,
    pub overlay: DynamicClamp,
    /// The proxy draw calls recorded this frame (the 6-E headline:
    /// one per populated kind instanced; one per instance in the
    /// per-object reference). Zero on unpopulated and deferred
    /// frames. The corner mesh's own material draws are NOT counted
    /// here — this counts proxy submission only.
    pub proxy_draw_calls: u32,
    /// vkCmdDrawIndirect issuances THIS frame (the 6-E2 issuance
    /// observation — the delta of the context's monotonic counter
    /// across this frame's recording). One per populated kind in
    /// the indirect mode and ZERO in every other mode; zero on a
    /// deferred frame. Distinguishes the indirect submission from
    /// the direct one, which the draw-call count deliberately
    /// cannot (the release fallback draws the identical geometry
    /// through cmd_draw — this field is what observes the
    /// difference).
    pub indirect_draws_issued: u32,
    /// P5 §1.3 (A11/R1): the frame's instance-arena facts — the
    /// population supplied (npc + vehicle + rain) and the carve's
    /// per-slot capacity AFTER this frame's ensure (0 on unpopulated
    /// and deferred frames, the region discipline). The safe layer
    /// accumulates them into `RenderStats`' capacity/peak-used
    /// fields.
    pub instance_supplied: u32,
    pub instance_capacity: u32,
    /// P5 §1.3: the frame's pose-arena facts — blocks supplied and
    /// the carve's per-slot block capacity after the ensure (0 on
    /// unpopulated and deferred frames).
    pub pose_supplied: u32,
    pub pose_capacity: u32,
    /// P5 §1.6: arena growth events THIS frame (the delta of the
    /// context's monotonic counter — the indirect-issuance
    /// observation's exact discipline). A growth event is a carve
    /// RE-CREATION at a larger size (the initial demand-sized carve
    /// is not an event: nothing existed to replace, nothing could
    /// have referenced it, no stall); each event stalls the frame
    /// once under `device_wait_idle`, bounded by the power-of-two
    /// quantum to at most `log2(cap / 64)` per session. Deferred
    /// frames record nothing at their own return; the counter is
    /// monotonic, so a growth that preceded an acquire-time
    /// deferral (the one deferral arm after the ensure ladder)
    /// lands on the NEXT frame's delta — no event is ever lost,
    /// at most attributed to the following frame.
    pub arena_growth_events: u32,
    /// P5 §3.3 (A18/R2): population mapped-region writes the
    /// dirty-generation protocol SKIPPED this frame (instance, pose,
    /// and indirect blocks whose per-slot stamp already recorded the
    /// submission's generation — the between-tick and
    /// static-placement frames' whole saving). Zero on unpopulated,
    /// deferred, and every generation-0 (un-opted) frame; at most 3
    /// (one per region). The safe layer accumulates it into
    /// `RenderStats::population_writes_skipped`.
    pub population_writes_skipped: u32,
}

/// Construction-time frame policy (P4 D2 §3.4, stage A04): the
/// `RenderConfig` values the frame flow reads per call, collapsed
/// from three positional parameters into one `Copy` value — the
/// policy half of the 19 → 6 parameter collapse. The four owning
/// states stay four explicit `&mut` parameters through every
/// internal function (the design's §5.2 anti-god-object rule); this
/// struct carries ONLY configuration, never state.
///
/// **The design's `scene_attached` adaptation (documented):** §3.4
/// sketched the third field as `scene_attached: bool`, written when
/// the `Corner` construction preference was the scene-carrying arm.
/// S6 deleted that arm — authored scene content now reaches the
/// renderer only through `attach_scene`, and the attached handle's
/// state IS `ctx.corner` (read directly by the flow; §3.4's own
/// "the attached handle's state (was ctx.corner)" note). The
/// preference that remains is the static Stage-2 demonstration
/// scene's `Included` opt-in (design §3.2 keeps it), and the
/// pipeline-replacement path consumes it
/// (`recreate_swapchain` → `PipelineState::new`) — so the policy
/// carries the preference itself, verbatim, and the attached-scene
/// fact needs no boolean mirror.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FramePolicy {
    /// The present-mode preference a recreation re-resolves against
    /// (`RenderConfig::present_mode`).
    pub present_preference: PresentModePreference,
    /// The verification seam that forces pipeline replacement on
    /// every recreation (`RenderConfig::replace_pipelines`,
    /// `--replace-pipelines`).
    pub replace_pipelines: bool,
    /// The static demonstration scene's opt-in (P3,
    /// `RenderConfig::demo_scene`) — the LIVE preference every
    /// replacement pipeline is built with.
    pub demo_scene: DemoScenePreference,
    /// P5 §1.3 (A11/R1) + CA-12 (F12): the instance arena's
    /// VALIDATED cap — the construction-time
    /// [`crate::config::ArenaBudgets`] value (`RenderConfig`'s
    /// `instance_capacity` budget after the shared representable-
    /// capacity narrowing), the lazy-ensure ladder's growth ceiling
    /// for the instance carve. `u32` — the ONE domain the carve, the
    /// growth quantum, and the CPU repack's refusal boundary share
    /// (the pre-fix `usize` here re-narrowed at the carve sites).
    pub instance_capacity: u32,
    /// P5 §1.3 + CA-12: the pose arena's VALIDATED cap (the
    /// `ArenaBudgets` value — device-checked against the binding-5
    /// storage range and offset alignment at construction) — the
    /// retained pose storage's growth ceiling, the same number the
    /// CPU pose repack refuses above.
    pub pose_capacity: u32,
    /// P5 §1.3: the region arena's configured per-slot vertex budget
    /// (`RenderConfig::region_capacity`) — the region carve's growth
    /// ceiling AND the F-03 clamp's boundary (unchanged semantics:
    /// truncation above it stays fail-visible with the counters).
    pub region_capacity: u32,
}

/// One validated frame, one level down (P4 D2 §3.4, stage A04): the
/// input half of a `render_frame` call as ONE value — the
/// [`FrameInputs`] the safe layer's adapter built from a validated
/// `FrameSubmission` (S1's equivalence target), plus the
/// [`FramePlan`] derived from it ONCE (S3's single derivation, now
/// at this bundle's construction). This is the frame half of the
/// 19 → 6 collapse: the same Options and slices that flowed through
/// fifteen positional parameters, named by one struct.
///
/// **Zero-cost statement (§3.4):** a borrow bundle — `Copy`, no
/// `Vec`, no `Box`, no trait objects; construction is stack assembly
/// the compiler elides. The batteries' identical `FrameOutcome`s and
/// allocation counters are the behavioral half of the proof.
///
/// **The design's field-list adaptation (documented):** §3.4 sketched
/// `{ submission, scene, ripple_debug }` — pre-S1 text, written
/// before the submission contract landed. The landed shape: the
/// SUBMISSION arrives already validated and repacked (the adapter
/// runs in the safe layer, device-free, per the §3.3
/// validate-whole-before-any-frame-state invariant — `FrameInputs`
/// is "the validated submission, one level down"); the SCENE stays
/// `ctx.corner` — a reference carved out of the context cannot
/// coexist with the `&mut ctx` the four-state ownership rule
/// requires, and post-S6 `ctx.corner` IS the attached handle's
/// state; and `ripple_debug` is folded at adaptation time into the
/// wet camera's packed ripple uniform (the retained test-only
/// sticky's only consumer, through `adapt_frame`'s policy
/// parameter).
#[derive(Debug, Clone, Copy)]
pub(crate) struct InternalFrame<'a> {
    /// The validated, repacked input set (the safe layer adapter's
    /// output — `adapt_frame`, or the layers-only literal).
    pub inputs: FrameInputs<'a>,
    /// The frame's explicit pass-uses declaration, derived once at
    /// construction (S3's `FramePlan`, consumed by the preparation
    /// pair and the record cascade).
    pub plan: FramePlan,
}

impl<'a> InternalFrame<'a> {
    /// Builds the flow's frame bundle from the adapter's output,
    /// deriving the [`FramePlan`] ONCE — the S3 single derivation,
    /// performed at exactly the same point relative to the flow as
    /// when it sat inline at the top of `render_frame`'s body
    /// (before any synchronization state is touched, on the same
    /// flat facts every route hands the frame flow).
    pub(crate) fn new(inputs: FrameInputs<'a>) -> Self {
        // THE FRAME PLAN (P4 D2 §4.2, stage S3): the explicit pass-uses
        // declaration, derived ONCE here from the input set — the same
        // flat facts every route hands the frame flow. The THREE
        // preparation sites below (the pre-recreation ensure, the
        // Recreated arm's re-run, and the out-of-date path's
        // re-preparation) and the record cascade's pass-variant decisions
        // (the post/main branch, the wet ground's placement, the overlay
        // upload gate) consume it instead of re-deriving the uses from
        // the positional Options — the preparation triplication the
        // design's §1.4 finding named. The equivalence to the implicit
        // derivation this flow previously performed at each site is
        // pinned by `frame_plan`'s unit test (the S1/S2 pattern).
        let foreground_vertices = inputs.foreground.map_or(&[][..], |layer| layer.vertices);
        let plan = FramePlan::derive(
            inputs.region_vertices,
            foreground_vertices,
            inputs.hud,
            inputs.debug_overlay,
            inputs.proxy.as_ref(),
            inputs.shadowed.as_ref(),
            inputs.sky.as_ref(),
            inputs.post,
            inputs.wet.as_ref(),
        );
        Self { inputs, plan }
    }
}

/// The acquire phase's result (§5.4): an acquired image, or the
/// deferral signal (every deferred arm inside `acquire_with_retry`
/// already latched what it owes — the recreation intent in
/// `frames.needs_recreate` — exactly as the inline arms did).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Acquisition {
    Acquired {
        image_index: u32,
        suboptimal: bool,
        /// The out-of-date arm performed the inline recreation (the
        /// public `recreated` contract: callers learn the generation
        /// changed).
        recreated: bool,
    },
    Deferred,
}

/// The full frame flow: wait → acquire → (recreate if needed) → record →
/// submit → present. Called by the safe [`crate::Renderer`] wrapper.
// Private frame orchestration (P4 D2 §3.4, stage A04): the four owning
// states stay four explicit `&mut` parameters — the ownership argument
// this list always defended — and the fifteen input/policy parameters
// collapsed into the `Copy` policy and the borrow-only frame bundle
// (see [`FramePolicy`] and [`InternalFrame`]). The body below is the
// §5.4 phase ladder in call order; each phase function carries its own
// verbatim-moved comments and safety arguments.
pub(crate) fn render_frame(
    ctx: &mut VulkanContext,
    swapchain: &mut SwapchainState,
    pipes: &mut PipelineState,
    frames: &mut FrameState,
    policy: FramePolicy,
    frame: &InternalFrame<'_>,
) -> Result<FrameOutcome, RendererError> {
    let mut outcome = FrameOutcome::default();
    let slot = frames.frame_slot;
    // The 6-E2 issuance observation's frame baseline: the recording
    // below (this frame's command buffer) increments the context's
    // monotonic vkCmdDrawIndirect counter at each call site; the
    // delta lands on the outcome after the submission. Deferred and
    // refused frames record nothing, so their delta is zero.
    let indirect_issued_before = ctx.indirect_draws_issued.get();
    // P5 §1.6's arena-growth baseline, the same discipline: the
    // lazy-ensure ladder (prepare, below) may re-create a carve at a
    // larger size under `device_wait_idle`; the monotonic counter's
    // delta is this frame's growth events. Deferred frames record
    // nothing at their own return; a growth that preceded an
    // acquire-time deferral lands on the next frame's delta (the
    // counter is monotonic — no event is lost).
    let arena_growth_before = ctx.arena_growth_events.get();

    // Phase 1 — validate_inputs: the proxy material validation,
    // before ANY synchronization state is touched.
    let proxy_sets = validate_proxy_materials(ctx, frame.inputs.proxy.as_ref())?;

    // Phase 2 — prepare: swapchain checks, lazy state, and target
    // creation before acquire (finding 20, Phase A; findings 3-8).
    // The coordinator owns the outcome flags: the preparation pair
    // reports Deferred/Recreated/Proceeded and this match sets
    // FrameOutcome::{deferred, recreated} from those arms.
    match prepare_frame(ctx, swapchain, pipes, frames, policy, frame)? {
        FramePreparationResult::Deferred => {
            outcome.deferred = true;
            return Ok(outcome);
        }
        FramePreparationResult::Recreated => {
            outcome.recreated = true;
            // Re-run preparation against the new generation.
            match prepare_frame(ctx, swapchain, pipes, frames, policy, frame)? {
                FramePreparationResult::Deferred => {
                    outcome.deferred = true;
                    return Ok(outcome);
                }
                FramePreparationResult::Recreated => {
                    // This should never happen - recreate once per frame.
                    // If it does, we'll retry in the acquire loop.
                    frames.needs_recreate = true;
                    outcome.deferred = true;
                    return Ok(outcome);
                }
                FramePreparationResult::Proceeded => {}
            }
        }
        FramePreparationResult::Proceeded => {}
    }

    // Phase 3 — wait_slot_fence (plus this slot's overlay growth,
    // which the design's §1.2 inventory grouped with the wait).
    wait_slot_fence(ctx, frames, frame, slot)?;

    // Phase 4 — acquire_with_retry: the out-of-date inline recreation
    // and the retry acquire live inside.
    let acquisition = acquire_with_retry(ctx, swapchain, pipes, frames, policy, frame, slot)?;
    let (image_index, suboptimal) = match acquisition {
        Acquisition::Acquired {
            image_index,
            suboptimal,
            recreated,
        } => {
            outcome.recreated |= recreated;
            (image_index, suboptimal)
        }
        Acquisition::Deferred => {
            outcome.deferred = true;
            return Ok(outcome);
        }
    };
    outcome.suboptimal = suboptimal;
    frames.image_index = image_index;

    // P5 §3.3 (A18/R2) — RESOURCE-REPLACEMENT INVALIDATION, the
    // growth arm — DEMOTED TO A BELT by T02 (strict audit IV, §2.3):
    // the epoch gate at the upload site
    // ([`crate::proxy_instances::population_write_needed`], the
    // carve's minted allocation epoch beside the generation) is the
    // CORRECTNESS BEARER — a replaced carve's fresh epoch can never
    // equal a surviving stamp's, so the delayed-invariant hazard the
    // audit sketched (a growth event swallowed by a later error
    // before this clear runs, the next frame's baseline absorbing
    // the counter) is closed at the RESOURCE side, not by the call
    // sequence. The clear is KEPT as defense-in-depth (one redundant
    // rewrite per growth event — the growth events are log-bounded,
    // and the belt also covers the conservative region-growth arm
    // below); the recreation arm of the invalidation lives in
    // `recreate_swapchain` (the pipeline replacement re-publishes
    // the instance and indirect carves with FRESH EPOCHS there —
    // the same rule at the replacement site).
    if ctx.arena_growth_events.get() != arena_growth_before {
        frames.population_stamps.clear();
    }

    // Phases 5-8 — `acquire_to_submit`: everything between the
    // acquisition and the submission. CA-10 (the stranded slot): the
    // slot fence's RESET and the per-image ownership PUBLISH are
    // DEFERRED to the commit step between record and submit (see
    // [`commit_slot_fence`]), so every fallible step below runs with
    // the fence still carrying its previous signal — a failure at
    // ANY of them cannot strand the slot. Any error inside gets the
    // acquisition's disposition (the release in
    // [`dispose_failed_acquisition`]) instead of propagating raw.
    let submitted = acquire_to_submit(
        ctx,
        swapchain,
        pipes,
        frames,
        frame,
        proxy_sets,
        &mut outcome,
        slot,
        image_index,
    );
    let (gpu_render_ms, _world_drawn, present_semaphore) = match submitted {
        Ok(triple) => triple,
        Err(error) => {
            return Err(dispose_failed_acquisition(
                ctx,
                swapchain,
                frames,
                slot,
                image_index,
                error,
            ));
        }
    };

    // Phase 9 — present.
    present_image(
        ctx,
        swapchain,
        frames,
        image_index,
        present_semaphore,
        &mut outcome,
    )?;

    outcome.gpu_render_ms = gpu_render_ms;
    outcome.indirect_draws_issued =
        (ctx.indirect_draws_issued.get() - indirect_issued_before) as u32;
    outcome.arena_growth_events = (ctx.arena_growth_events.get() - arena_growth_before) as u32;
    frames.frame_slot = (slot + 1) % FRAMES_IN_FLIGHT as usize;
    Ok(outcome)
}

/// Phases 5-8 (§5.4, CA-10's restructure): the per-image guard wait
/// and timestamp read, the frame's uploads, the present-wait
/// semaphore, the command-buffer recording, the slot-fence COMMIT
/// (the deferred reset + per-image ownership publish), and the
/// submission. The slot fence stays SIGNALLED through everything
/// before the commit — the property that makes every error below
/// strand-free — and the commit is the last fallible step before
/// the submit (the reset itself is the one CA-10 boundary whose
/// failure leaves the fence state indeterminate; its disposition is
/// the fence-state discriminator in [`dispose_failed_acquisition`]).
#[allow(clippy::too_many_arguments)]
fn acquire_to_submit(
    ctx: &VulkanContext,
    swapchain: &SwapchainState,
    pipes: &PipelineState,
    frames: &mut FrameState,
    frame: &InternalFrame<'_>,
    proxy_sets: Option<[vk::DescriptorSet; 3]>,
    outcome: &mut FrameOutcome,
    slot: usize,
    image_index: u32,
) -> Result<(Option<f64>, u32, vk::Semaphore), RendererError> {
    // Phase 5 — guard_and_timestamps: the per-image guard wait (the
    // LAST wait that can reference this slot's previous signal —
    // the reset is still deferred) and the previous use's timestamp
    // read (Phase 3's slot-fence wait proved them resolved).
    let gpu_render_ms = guard_image_and_read_timestamps(ctx, frames, slot, image_index)?;

    // Phase 6 — upload_frame_data: the mapped-slot writes this
    // slot's fence wait now owns (infallible today — the Result is
    // the boundary contract; the CA-10 fault knob sits at its top).
    let world_drawn = upload_frame_data(ctx, pipes, frames, frame, outcome, slot)?;

    // The present-wait semaphore is owned by the acquired image
    // (corrective finding C). Grown BEFORE recording begins (the
    // fail-fast discipline: a creation failure must not leave a
    // recorded-but-unsubmitted command buffer against a reset fence
    // and an acquired image) — and since CA-10, BEFORE the fence
    // commit as well, so the failure stays strand-free by
    // construction.
    let present_semaphore = frames.present_semaphore_for(ctx, image_index)?;

    // Phase 7 — record: the pass cascade into this slot's command
    // buffer.
    record_command_buffer(
        ctx,
        swapchain,
        pipes,
        frames,
        frame,
        proxy_sets,
        slot,
        image_index,
        world_drawn,
        outcome,
    )?;

    // Phase 7.5 — the slot-fence COMMIT (CA-10): the reset and the
    // per-image ownership publish, deferred until every fallible
    // step before the submission has succeeded.
    commit_slot_fence(ctx, frames, slot)?;

    // Phase 8 — submit.
    submit_command_buffer(ctx, frames, present_semaphore, slot)?;

    Ok((gpu_render_ms, world_drawn, present_semaphore))
}

/// Phase 1 — `validate_inputs` (§5.4): the proxy materials are
/// validated before ANY synchronization state is touched (the
/// capture-refusal discipline: a refusal here can never leave a fence
/// reset-but-unsignaled or an acquired image's semaphore unconsumed).
/// A populated kind whose material index falls outside the attached
/// scene's table is a typed error; an EMPTY kind may carry any index
/// (it draws nothing). The Era-2 package C rain kind joins the
/// validation exactly like the other kinds. Extracted verbatim from
/// `render_frame`'s body at A04; `ctx` is an immutable reborrow — the
/// validation only reads the attached scene's material table.
fn validate_proxy_materials(
    ctx: &VulkanContext,
    proxy: Option<&proxy_draw::ProxyFrameUpload<'_>>,
) -> Result<Option<[vk::DescriptorSet; 3]>, RendererError> {
    let proxy_sets = match (ctx.corner.as_ref(), proxy) {
        (Some(corner), Some(upload)) => {
            let npc_set = corner.material_set(upload.npc_material);
            let vehicle_set = corner.material_set(upload.vehicle_material);
            let rain_set = corner.material_set(upload.rain_material);
            if (upload.npc_count > 0 && npc_set.is_none())
                || (upload.vehicle_count > 0 && vehicle_set.is_none())
                || (upload.rain_count > 0 && rain_set.is_none())
            {
                return Err(RendererError::Vulkan(format!(
                    "proxy material index outside the scene material table \
                     (npc {}, vehicle {}, rain {})",
                    upload.npc_material, upload.vehicle_material, upload.rain_material
                )));
            }
            Some([
                npc_set.unwrap_or(vk::DescriptorSet::null()),
                vehicle_set.unwrap_or(vk::DescriptorSet::null()),
                rain_set.unwrap_or(vk::DescriptorSet::null()),
            ])
        }
        // A proxy upload without scene resources is structurally
        // unreachable (the safe layer requires an attached scene —
        // attach_scene is the only path, and it sets the context's
        // scene resources) — treated as the
        // unpopulated frame rather than an error.
        (None, Some(_)) | (None, None) | (Some(_), None) => None,
    };
    Ok(proxy_sets)
}

/// Phase 3 — `wait_slot_fence` (§5.4, the slot-fence wait plus the
/// overlay slot growth the design's §1.2 inventory grouped with it
/// at the wait's line range): waits for this slot's previous
/// submission, then grows only the fenced slot's overlay buffer —
/// before acquire, so a failure leaves the image-available semaphore
/// and fence reusable.
fn wait_slot_fence(
    ctx: &VulkanContext,
    frames: &mut FrameState,
    frame: &InternalFrame<'_>,
    slot: usize,
) -> Result<(), RendererError> {
    // Wait for this slot's previous submission. The fence is reset only
    // AFTER the per-image guard below: under present backpressure the
    // driver can hand a slot the image that slot last rendered, and
    // resetting first would leave that guard waiting on a fence only
    // THIS frame's submission will signal — the 240-pacing-against-FIFO
    // wedge (rig finding, 2026-10-20: "wait image fence timed out").
    //
    // T06 §4.3: a TIMEOUT is classified through ONE `get_fence_status`
    // before the fence is called stuck — the wait's bounded timeout
    // surfaces as the TYPED stuck-fence refusal only when the device
    // itself reports the fence unsignaled (a loss at the poll surfaces
    // as the typed loss instead; a fence that signaled between the
    // wait and the poll proceeds — never a false stuck report).
    let waited = unsafe {
        ctx.device
            .wait_for_fences(&[frames.in_flight[slot]], true, WAIT_TIMEOUT_NS)
    };
    match waited {
        Ok(()) => {}
        Err(vk::Result::TIMEOUT) => match classify_fence_timeout(unsafe {
            ctx.device.get_fence_status(frames.in_flight[slot])
        }) {
            FenceTimeoutClass::Signaled => {}
            FenceTimeoutClass::Stuck => {
                return Err(RendererError::StuckFence {
                    site: "wait_slot_fence (the slot's previous submission)",
                });
            }
            FenceTimeoutClass::DeviceLost => {
                return Err(RendererError::DeviceLost {
                    operation: "get_fence_status (slot fence, after wait timeout)".into(),
                });
            }
            FenceTimeoutClass::PollError(error) => {
                return Err(RendererError::vk(error, "get_fence_status (slot fence)"));
            }
        },
        Err(error) => {
            return Err(RendererError::vk(
                error,
                "wait_for_fences (slot fence timed out — a submission never completed; \
                 presenting faster than the present mode drains?)",
            ));
        }
    }

    // Allocation can fail. Grow only the fenced slot, before acquire, so
    // a failure leaves the image-available semaphore and fence reusable.
    frames.overlay_slots[slot].reserve(
        ctx,
        frame
            .inputs
            .hud
            .len()
            .saturating_add(frame.inputs.debug_overlay.len()),
    )?;
    Ok(())
}

/// Phase 4 — `acquire_with_retry` (§5.4): the first acquire, the
/// injected-out-of-date simulation, the out-of-date inline recreation
/// (through `prepare_after_recreation`), and the retry acquire.
/// OUT_OF_DATE retries once. The fence stays signaled until an image
/// has actually been acquired, including when both attempts defer.
fn acquire_with_retry(
    ctx: &mut VulkanContext,
    swapchain: &mut SwapchainState,
    pipes: &mut PipelineState,
    frames: &mut FrameState,
    policy: FramePolicy,
    frame: &InternalFrame<'_>,
    slot: usize,
) -> Result<Acquisition, RendererError> {
    // The one-shot fault knobs, checked in arm order (device loss
    // first — CA-13's knob models the terminal outcome, so it wins
    // over the recoverable out-of-date when both are armed; only
    // the TAKEN plan is consumed, the other stays armed for the
    // next frame — the one-shot discipline holds for both):
    //
    // - CA-13's device-loss plan (F13/A14): the FIRST acquire site
    //   returns VK_ERROR_DEVICE_LOST instead of the real call — the
    //   faithful driver contract (DEVICE_LOST means no image was
    //   acquired), the same argument as the out-of-date knob below.
    //   The final match's generic arm converts it through
    //   RendererError::vk into the TYPED DeviceLost classification
    //   whose operation context ("acquire_next_image") rides the
    //   variant; the renderer's entry gates latch it.
    // - Finding 6's out-of-date plan: the injected result models the
    //   driver contract exactly (ERROR_OUT_OF_DATE_KHR means no
    //   image was acquired and the semaphore is untouched —
    //   precisely the state the out-of-date arm below already
    //   handles; skipping the real call is therefore the faithful
    //   simulation, not an approximation). The plan is consumed at
    //   the first site only, so the retry acquire inside the
    //   out-of-date arm remains structurally real.
    let mut acquire_result = if ctx.take_acquire_device_lost() {
        Err(vk::Result::ERROR_DEVICE_LOST)
    } else if ctx.take_acquire_out_of_date() {
        Err(vk::Result::ERROR_OUT_OF_DATE_KHR)
    } else {
        unsafe {
            ctx.swapchain_ext.acquire_next_image(
                swapchain.swapchain,
                100_000_000,
                frames.image_available[slot],
                vk::Fence::null(),
            )
        }
    };
    let mut recreated = false;
    if let Err(vk::Result::ERROR_OUT_OF_DATE_KHR) = acquire_result {
        // The out-of-date inline recreation uses prepare_after_recreation
        // to handle probe, recreate, state-ensure, target-ensure, capture-ensure,
        // and retry acquire (finding 20, Phase A; findings 3-8).
        match prepare_after_recreation(ctx, swapchain, pipes, frames, policy, frame)? {
            FramePreparationResult::Deferred => {
                return Ok(Acquisition::Deferred);
            }
            FramePreparationResult::Recreated => {
                // This should never happen - prepare_after_recreation already
                // retried acquire. If it returns Recreated, something went wrong.
                frames.needs_recreate = true;
                return Ok(Acquisition::Deferred);
            }
            FramePreparationResult::Proceeded => {
                // The inline recreation happened inside the helper — the
                // original arm reported it on the outcome (the public
                // recreated contract: callers learn the generation changed).
                recreated = true;
                // Retry acquire after preparation.
                acquire_result = unsafe {
                    ctx.swapchain_ext.acquire_next_image(
                        swapchain.swapchain,
                        100_000_000,
                        frames.image_available[slot],
                        vk::Fence::null(),
                    )
                };
            }
        }
    }
    match acquire_result {
        Ok((image_index, suboptimal)) => Ok(Acquisition::Acquired {
            image_index,
            suboptimal,
            recreated,
        }),
        Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
            frames.needs_recreate = true;
            Ok(Acquisition::Deferred)
        }
        Err(vk::Result::TIMEOUT) => Err(RendererError::Vulkan(
            "acquire_next_image timed out: no swapchain image became available in 100 ms \
             (present mode not draining — vsync-locked FIFO paced beyond the display rate?)"
                .into(),
        )),
        Err(error) => Err(RendererError::vk(error, "acquire_next_image")),
    }
}

/// Phase 5 — `guard_image_and_read_timestamps` (§5.4; the per-image
/// guard wait, CA-10's restructure of the old
/// `reset_and_timestamps`): guards against acquiring an image
/// another in-flight submission still reads, then reads the previous
/// use's timestamps. THE WAIT DISCIPLINE IS UNCHANGED from the
/// pre-CA-10 code — this is still "the last wait that can reference
/// this slot's previous signal" — but the RESET that used to follow
/// it here is DEFERRED to [`commit_slot_fence`] between record and
/// submit: every fallible step between this guard and the commit
/// (the uploads, the present semaphore, the recording) now runs with
/// the slot fence still carrying its previous signal, so a failure
/// at any of them leaves the fence waitable and the slot reusable
/// (the stranded-slot fix — see [`dispose_failed_acquisition`]).
fn guard_image_and_read_timestamps(
    ctx: &VulkanContext,
    frames: &mut FrameState,
    slot: usize,
    image_index: u32,
) -> Result<Option<f64>, RendererError> {
    if let Some(image_fence) = mut_at(&mut frames.images_in_flight, image_index as usize) {
        if *image_fence != vk::Fence::null() {
            let waited = unsafe {
                ctx.device
                    .wait_for_fences(&[*image_fence], true, WAIT_TIMEOUT_NS)
            };
            match waited {
                Ok(()) => {}
                // T06 §4.3: the same one-poll classification as the
                // slot wait — a timeout is stuck only when the device
                // says so, and a loss at the poll surfaces typed.
                Err(vk::Result::TIMEOUT) => {
                    let polled = unsafe { ctx.device.get_fence_status(*image_fence) };
                    match classify_fence_timeout(polled) {
                        FenceTimeoutClass::Signaled => {}
                        FenceTimeoutClass::Stuck => {
                            return Err(RendererError::StuckFence {
                                site: "guard_image (the image's previous submission)",
                            });
                        }
                        FenceTimeoutClass::DeviceLost => {
                            return Err(RendererError::DeviceLost {
                                operation: "get_fence_status (image fence, after wait timeout)"
                                    .into(),
                            });
                        }
                        FenceTimeoutClass::PollError(error) => {
                            return Err(RendererError::vk(error, "get_fence_status (image fence)"));
                        }
                    }
                }
                Err(error) => {
                    return Err(RendererError::vk(
                        error,
                        "wait image fence (timed out — a submission never completed)",
                    ));
                }
            }
        }
        // The per-image ownership STAGING (CA-10): the slot fence is
        // recorded as the image's owner BEFORE the fence is reset —
        // publishing here is safe because the slot fence is still
        // SIGNALED (a future guard waiting on it passes immediately)
        // and the PUBLISHING slot is the one this frame is about to
        // reset-and-submit; if anything below fails before the
        // commit, `dispose_failed_acquisition` restores the previous
        // owner or nulls the entry (the frame that would have
        // signaled it never ran).
        *image_fence = frames.in_flight[slot];
    }

    // Timestamps from this slot's previous use are resolved now —
    // Phase 3's slot-fence wait proved the submission complete (the
    // fence is still signaled; the read does not consume it). The
    // first use of a slot has no samples yet, and unsupported
    // timestamps are never queried (corrective finding E).
    let gpu_render_ms =
        if timestamp_sample_ready(frames.timestamps_supported, frames.timestamps_written[slot]) {
            read_timestamps(ctx, frames, slot)
        } else {
            None
        };
    Ok(gpu_render_ms)
}

/// Phase 7.5 — `commit_slot_fence` (CA-10's deferred reset): resets
/// this slot's fence at the LAST fallible boundary before the
/// submit, completing the per-image ownership publication the guard
/// phase staged (the slot fence it recorded there becomes the image's
/// real owner the moment this frame's submission signals it). The
/// reset must precede the submission (a submit signals the fence; it
/// must be unsignaled), and every wait that can reference the slot's
/// PREVIOUS signal (Phase 3's slot wait, the per-image guard) has
/// completed by construction — the pre-CA-10 proof, preserved with
/// the reset simply moved later in the same frame.
fn commit_slot_fence(
    ctx: &VulkanContext,
    frames: &mut FrameState,
    slot: usize,
) -> Result<(), RendererError> {
    // Safe to reset only now: every wait that can reference this
    // slot's previous signal has run (Phase 3's wait; the image
    // guard in Phase 5 — the last one).
    unsafe {
        ctx.device
            .reset_fences(&[frames.in_flight[slot]])
            .map_err(|error| RendererError::vk(error, "reset_fences"))?;
    }
    Ok(())
}

/// The acquisition's disposition (CA-10): EVERY error after a
/// successful acquire gets an explicit release instead of
/// propagating raw. The pre-CA-10 code reset the slot fence before
/// the fallible upload/semaphore/record/submit chain, so any error
/// in that chain left the fence reset-but-never-signaled — the next
/// frame on the slot blocked the full 2 s
/// [`WAIT_TIMEOUT_NS`][super::WAIT_TIMEOUT_NS] and failed with the
/// slot-fence timeout, and the acquired image's
/// `image_available` signal stayed pending for the slot's next
/// acquire (signaling an already-signaled binary semaphore). This
/// release defines the disposition for each piece:
///
/// **The acquired image + the acquire semaphore** — RETURNED
/// together: the WSI-canonical release presents the image WAITING
/// `image_available[slot]`, so the presentation is ordered against
/// the acquire's implicit layout transition (the sync-validation
/// WRITE_AFTER_READ the rig caught when an earlier draft presented
/// with no waits) and the wait consumes the signal at the
/// presentation — one operation returning the image to the pool and
/// retiring the semaphore's signal. The content is whatever the
/// image last held (the failed frame drew nothing into it — a
/// visible one-frame staleness on an already-reported error path).
/// An out-of-date refusal is the normal path's own deferred-
/// recreation arm (latch the intent, probe next frame); because a
/// REJECTED present may not have consumed the wait, the semaphore
/// is then REPLACED (below) rather than trusted. A HARD present
/// failure holds the image (the pool shrinks by one, fail-visible
/// as the typed acquire timeout once failures exhaust it) and the
/// semaphore is replaced the same way.
///
/// **The slot fence** — healed when the fence-state discriminator
/// (`get_fence_status`) finds it unsignaled: the fence is SIGNALED
/// for every failure before the commit (never reset this frame),
/// and NOT_READY after a committed reset whose submission failed or
/// never ran (including a reset that itself failed indeterminately
/// — the status reports the actual state). The heal is ONE empty
/// submission (no command buffers, no waits) carrying the fence —
/// the canonical signal-a-fence idiom — and a status error (device
/// lost) takes the heal arm as the conservative direction.
///
/// **The drain proof** — after the release operations, ONE
/// `queue_wait_idle` proves the presentation's semaphore wait (and
/// the heal submission) COMPLETED before this function returns:
/// the next frame's acquire requires the semaphore with NO pending
/// operations, and a queue-asynchronous consume would race the
/// reuse (the rig's second catch — the pending-operations
/// validation error). The idle is bounded by the presentation
/// engine's drain (a FIFO present processes within a vblank or
/// two) — tens of milliseconds against the strand's 2 seconds.
///
/// **The per-image ownership** — nulled when the discriminator
/// found the fence reset (the publishing frame never submitted, so
/// no work references the image; the image's previous real owner
/// completed — the guard waited on it — so the null costs a future
/// acquirer nothing but a skipped wait). Left UNTOUCHED when the
/// fence is still signaled (the previous owner's entry may still be
/// legitimately waited on; the guard's own wait failure returned
/// before the publication).
///
/// **The recorded command buffer** — reset (best-effort): a
/// mid-recording failure leaves the buffer in the recording state,
/// which the next frame's `begin_command_buffer` would refuse; the
/// pool's `RESET_COMMAND_BUFFER` flag makes the reset legal at any
/// time after the slot's previous submission completed (Phase 3's
/// wait proved it). A failed reset surfaces at the next frame's
/// begin as a typed error — fail-visible, never a silent strand.
///
/// **Escalation** — if the heal submission, the drain, or the
/// rejected-present semaphore replacement fails, the slot's
/// synchronization state is unrecoverable: the ORIGINAL error is
/// wrapped in [`RendererError::FrameSlotUnrecoverable`], the typed
/// terminal refusal — the caller must treat this renderer as
/// failed and construct a fresh one. Never a silent strand.
fn dispose_failed_acquisition(
    ctx: &VulkanContext,
    swapchain: &SwapchainState,
    frames: &mut FrameState,
    slot: usize,
    image_index: u32,
    error: RendererError,
) -> RendererError {
    // Best-effort command-buffer reset (a recording-state buffer
    // would refuse the next frame's begin; a clean buffer resets
    // harmlessly — see the doc above).
    unsafe {
        let _ = ctx.device.reset_command_buffer(
            frames.command_buffers[slot],
            vk::CommandBufferResetFlags::empty(),
        );
    }
    // The fence-state discriminator (see the doc above).
    let fence_signaled =
        unsafe { ctx.device.get_fence_status(frames.in_flight[slot]) }.unwrap_or(false);
    let heal_fence = !fence_signaled;
    if heal_fence {
        // The ownership published at the guard must not survive a
        // frame that never submitted: null it.
        if let Some(image_fence) = mut_at(&mut frames.images_in_flight, image_index as usize) {
            *image_fence = vk::Fence::null();
        }
    }
    // The image return + the acquire semaphore's consume: the
    // WSI-canonical release present (see the doc above).
    let present_wait_semaphores = [frames.image_available[slot]];
    let present_swapchains = [swapchain.swapchain];
    let present_image_indices = [image_index];
    let present_info = vk::PresentInfoKHR::default()
        .wait_semaphores(&present_wait_semaphores)
        .swapchains(&present_swapchains)
        .image_indices(&present_image_indices);
    let semaphore_trusted = match unsafe {
        ctx.swapchain_ext
            .queue_present(ctx.graphics_queue, &present_info)
    } {
        Ok(suboptimal) => {
            if suboptimal {
                mark_suboptimal(&mut frames.needs_recreate);
            }
            // A processed present consumed the wait: the semaphore
            // is trusted idle once the drain below completes.
            true
        }
        Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
            // The normal present path's own deferred arm: the
            // presentation is gone, the recreation intent and the
            // deferral hint latch. A rejected present may not have
            // consumed the wait — the semaphore is replaced below.
            mark_suboptimal(&mut frames.needs_recreate);
            frames.surface_probe_pending = true;
            false
        }
        Err(_present_error) => {
            // The image stays held (pool shrinkage, fail-visible as
            // the typed acquire timeout once failures exhaust it);
            // the semaphore may hold its signal — replaced below.
            false
        }
    };
    // The fence heal (see the doc above): one empty submission
    // carrying the slot fence, only when the discriminator found it
    // unsignaled.
    if heal_fence {
        let fence = frames.in_flight[slot];
        // SAFETY: an empty-batch submission with the fence — the
        // signal-a-fence idiom; no command buffers, no waits.
        if let Err(release_error) =
            unsafe { ctx.device.queue_submit(ctx.graphics_queue, &[], fence) }
                .map_err(|error| RendererError::vk(error, "release submit (fence heal)"))
        {
            return RendererError::FrameSlotUnrecoverable {
                source: Box::new(error),
                release: Box::new(release_error),
            };
        }
    }
    // The drain proof (see the doc above): the presentation's
    // semaphore wait and the heal submission are complete when this
    // returns — the slot's next acquire finds no pending operation.
    if let Err(release_error) = super::device_idle(ctx) {
        return RendererError::FrameSlotUnrecoverable {
            source: Box::new(error),
            release: Box::new(release_error),
        };
    }
    if !semaphore_trusted {
        // The release present was rejected or failed: the acquire
        // semaphore's signal may still be pending, and a pending
        // signal poisons the slot's next acquire (signaling an
        // already-signaled binary semaphore). REPLACE it — the
        // drain above proved no queued operation refers to it, so
        // the destroy is legal whatever its signal state, and the
        // fresh semaphore gives the slot a clean acquire.
        let stale = frames.image_available[slot];
        if stale != vk::Semaphore::null() {
            unsafe {
                ctx.device.destroy_semaphore(stale, None);
            }
        }
        let fresh = match unsafe {
            ctx.device
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
        }
        .map_err(|error| RendererError::vk(error, "release (acquire semaphore replacement)"))
        {
            Ok(fresh) => fresh,
            Err(release_error) => {
                return RendererError::FrameSlotUnrecoverable {
                    source: Box::new(error),
                    release: Box::new(release_error),
                };
            }
        };
        frames.image_available[slot] = fresh;
    }
    error
}

/// CA-12 (F12): the admitted-counts-vs-physical-carve check before
/// upload — PURE so the bound is unit-testable without a device.
/// Unreachable on the validated domain (the CPU repack and the
/// device carve share one construction-validated cap, and the carve
/// never sits below the frame's demand), but the pre-fix write sites
/// carried only a `debug_assert` — in release a mismatch wrote past
/// the mapped slot. The backstop converts that to a typed refusal;
/// the region buffer needs no twin (its F-03 `DynamicClamp`
/// truncates fail-visibly by design).
pub(crate) fn admitted_bytes_fit(
    admitted: usize,
    slot_bytes: u64,
    what: &str,
) -> Result<(), RendererError> {
    if admitted as u64 > slot_bytes {
        return Err(RendererError::Vulkan(format!(
            "the admitted {what} needs {admitted} B but the frame slot's physical \
             carve is {slot_bytes} B (the repack/carve domains disagree — \
             refusing rather than writing past the mapping)"
        )));
    }
    Ok(())
}

/// Phase 6 — `upload_frame_data` (§5.1's uploads row, §5.4's phase):
/// the mapped per-slot writes this slot's fence wait now owns — the
/// region/foreground layers, the overlay vertices, and the proxy
/// instance/pose/indirect blocks. Returns the world batch's drawn
/// count (the record phase's foreground first-instance offset).
fn upload_frame_data(
    ctx: &VulkanContext,
    pipes: &PipelineState,
    frames: &mut FrameState,
    frame: &InternalFrame<'_>,
    outcome: &mut FrameOutcome,
    slot: usize,
) -> Result<u32, RendererError> {
    // CA-10 fault knob: the UPLOAD boundary (Phase 6, post-acquire).
    // The mapped writes below are infallible today — the Result this
    // function returns is the boundary contract, and the knob keeps
    // the boundary's disposition testable (a plan armed here fires
    // after the acquire, before any write, with the slot fence still
    // signaled — the release must leave the slot reusable).
    ctx.check_lifecycle_fault(crate::LifecycleFaultSite::FrameUpload)?;
    let inputs = &frame.inputs;
    let region_vertices = inputs.region_vertices;
    let foreground = inputs.foreground;
    let foreground_vertices = foreground.map_or(&[][..], |layer| layer.vertices);
    let hud = inputs.hud;
    let debug_overlay = inputs.debug_overlay;
    let proxy = inputs.proxy.as_ref();
    // Both scene layers fit in the existing mapped slot. Copy directly from
    // their retained application buffers without concatenating CPU geometry.
    let mut world_drawn = 0;
    if !region_vertices.is_empty() || !foreground_vertices.is_empty() {
        (outcome.region, world_drawn) = unsafe {
            upload_layers(
                pipes.region_mapped,
                slot,
                pipes.region_capacity,
                region_vertices,
                foreground_vertices,
                LayerPriority::Second,
            )
        };
        outcome.foreground_drawn = outcome.region.drawn - world_drawn;
    }
    let overlay = &frames.overlay_slots[slot];
    outcome.overlay.capacity = overlay.capacity;
    // The overlay upload's gate is the plan's overlay bit (the
    // frame's explicit declaration — the same emptiness check the
    // plan derived once at the top).
    if frame.plan.uses.overlay {
        (outcome.overlay, _) = unsafe {
            upload_layers(
                overlay.mapped,
                0,
                overlay.capacity,
                hud,
                debug_overlay,
                LayerPriority::First,
            )
        };
    }

    // The 6-E instance upload: this slot's region of the mapped
    // instance buffer, written after the slot's fence wait (the
    // region/overlay discipline — the previous submission through
    // this slot, which read this region, has completed). The bytes
    // are the safe layer's repacked population (≤ the region's
    // bound by the repack's validated capacity). The 6-E2 indirect
    // command block follows the identical discipline in the same
    // place: this slot's 32 command bytes (the pure proxy_indirect
    // packing of the pinned mesh vertex counts and this frame's
    // per-kind counts), written only in the indirect mode (the
    // state exists — the frame flow's ensure ran before the fence
    // wait, outside command recording).
    if let (Some(proxy_state), Some(upload)) = (&pipes.proxy, proxy) {
        // P5 §1.3's observability half: the frame's arena facts,
        // recorded at the write site (the capacities are the carve's
        // AFTER this frame's ensure — the growth, if any, already
        // landed in prepare; the region discipline of reading them
        // per frame, never guessing).
        outcome.instance_supplied = upload
            .npc_count
            .saturating_add(upload.vehicle_count)
            .saturating_add(upload.rain_count);
        outcome.instance_capacity = proxy_state.capacity();
        // P5 §3.3 (A18/R2) — the per-frame-slot generation gate: the
        // stamp records what THIS slot's region last consumed, so a
        // matching nonzero generation proves the region already
        // holds byte-identical bytes and the write is skipped (the
        // safe layer's buffer records guarantee the retained bytes
        // ARE the declared generation's — the two levels are
        // independent by construction). Generation 0 (always-dirty)
        // always writes and resets the stamp, so an opted-in
        // generation can never skip against a slot holding some
        // other caller's bytes.
        // T02 §2.3: the EXTENDED gate — the carve's allocation epoch
        // beside the generation. A REPLACED carve (a growth commit
        // or a pipeline replacement published a fresh epoch) can
        // never inherit a surviving stamp, so the interrupted-growth
        // hazard the audit sketched (growth commits, then a
        // downstream failure swallows the delta clear, then a retry
        // at the surviving generation) rewrites through the epoch
        // mismatch — the correctness bearer; the frame flow's
        // growth-delta clear below is the belt.
        let instance_carve_epoch = proxy_state.allocation_epoch();
        if population_write_needed(
            frames.population_stamps.instance_epochs[slot],
            frames.population_stamps.instance[slot],
            instance_carve_epoch,
            upload.instance_generation,
        ) {
            // CA-12 (F12): the admitted-counts-vs-physical-carve check
            // BEFORE the upload — the audit's backstop at the one place
            // the repacked bytes meet the carve. Unreachable on the
            // validated domain (the CPU repack and the carve share one
            // cap, and the carve never sits below the frame's demand),
            // but the pre-fix write carried only a `debug_assert` — in
            // release a mismatch wrote past the mapped slot. Typed
            // refusal instead.
            admitted_bytes_fit(
                upload.bytes.len(),
                proxy_state.slot_bytes(),
                "instance population",
            )?;
            unsafe { proxy_state.write_slot(slot, upload.bytes) };
            frames.population_stamps.instance[slot] = upload.instance_generation;
            frames.population_stamps.instance_epochs[slot] = instance_carve_epoch;
        } else {
            outcome.population_writes_skipped += 1;
        }
        // The indirect command block rides the INSTANCE generation
        // (NEW-REND-08's cached-indirect observation): its bytes are
        // the pure proxy_indirect packing of the per-kind counts —
        // an instance-side fact — so the same stamp decision gates
        // it, tracked on its own stamp (the block is its own buffer,
        // lazily created only in the indirect mode). T05: the
        // indirect-count identity is SEPARATE from the payload
        // identity — the gate ALSO writes when the per-kind counts
        // differ from the slot's record, because a partition change
        // (byte-identical payload, different npc/vehicle/rain split)
        // moves the command block's bytes while every byte-length
        // oracle stays green.
        if let Some(indirect) = proxy_state.indirect.as_ref()
            && matches!(upload.mode, crate::ProxyDrawMode::Indirect)
        {
            let submitted_counts = [upload.npc_count, upload.vehicle_count, upload.rain_count];
            // T02: the indirect block rides the INSTANCE carve's
            // epoch (its buffer is published only beside that carve).
            if population_write_needed(
                frames.population_stamps.indirect_epochs[slot],
                frames.population_stamps.indirect[slot],
                instance_carve_epoch,
                upload.instance_generation,
            ) || frames.population_stamps.indirect_counts[slot] != submitted_counts
            {
                unsafe {
                    indirect.write_slot(
                        slot,
                        upload.npc_count,
                        upload.vehicle_count,
                        upload.rain_count,
                    )
                };
                frames.population_stamps.indirect[slot] = upload.instance_generation;
                frames.population_stamps.indirect_epochs[slot] = instance_carve_epoch;
                frames.population_stamps.indirect_counts[slot] = submitted_counts;
            } else {
                outcome.population_writes_skipped += 1;
            }
        }
        // The Era-2 pose bytes: this slot's region of the mapped pose
        // buffer, written under the SAME fence discipline as the
        // instance bytes above (the context's RETAINED skinned state
        // exists whenever a populated frame runs — the ensure is
        // unconditional on proxy presence; the debug_assert co-pins
        // the structural invariant the way the indirect path does).
        // The generation gate is the POSE generation — the garage's
        // split: static instances skip their half while the advancing
        // walk cycle keeps writing its own.
        if let Some(retained) = ctx.skinned_retained.as_ref() {
            outcome.pose_supplied = (upload.pose_bytes.len() / crate::animation::POSE_STRIDE)
                .min(u32::MAX as usize) as u32;
            outcome.pose_capacity = retained.blocks_capacity();
            // T02 §2.3: the pose carve's epoch beside the pose
            // generation — the same extended gate (the pose carve's
            // growth commits mint fresh epochs; the retained state
            // survives pipeline replacements, which do NOT re-mint).
            let pose_carve_epoch = retained.allocation_epoch();
            if population_write_needed(
                frames.population_stamps.pose_epochs[slot],
                frames.population_stamps.pose[slot],
                pose_carve_epoch,
                upload.pose_generation,
            ) {
                // CA-12 (F12): the same admitted-vs-carve backstop on the
                // pose path (the instance check's own discipline — the
                // pre-fix `debug_assert` alone gated this write in
                // release).
                admitted_bytes_fit(
                    upload.pose_bytes.len(),
                    retained.slot_stride(),
                    "pose population",
                )?;
                unsafe { retained.write_slot(slot, upload.pose_bytes) };
                frames.population_stamps.pose[slot] = upload.pose_generation;
                frames.population_stamps.pose_epochs[slot] = pose_carve_epoch;
            } else {
                outcome.population_writes_skipped += 1;
            }
        } else {
            debug_assert!(
                false,
                "the skinned ensure discipline makes a missing pose buffer \
                 unreachable on a populated frame"
            );
        }
    }
    Ok(world_drawn)
}

/// Phase 7 — `record` (§4.2(2), the cascade): the pass recording
/// into this slot's command buffer, in the fixed order the module
/// manifest documents — shadow depth → wet mirrored-scene (+barrier)
/// → the scene pass (the post chain's HDR passes or the main pass
/// with the corner scene recorder / region layers) → the shadow
/// readback → timestamps → the capture readback. The per-family
/// record functions (`record_depth_pass`, `record_reflection_pass`,
/// `post_state.record`, `record_corner_scene`) were already
/// extracted (finding 20 Phase B / S2-S4); this phase is the WIRING
/// the design's §5.4 named — the ladder that sequences them and owns
/// the pass-variant branch (the plan's `post` bit) and the overlay
/// draw. `ctx`, `pipes`, and `swapchain` are immutable here: the
/// cascade records through the states; only the frame state's
/// readback handshake bits mutate.
#[allow(clippy::too_many_arguments)]
fn record_command_buffer(
    ctx: &VulkanContext,
    swapchain: &SwapchainState,
    pipes: &PipelineState,
    frames: &mut FrameState,
    frame: &InternalFrame<'_>,
    proxy_sets: Option<[vk::DescriptorSet; 3]>,
    slot: usize,
    image_index: u32,
    world_drawn: u32,
    outcome: &mut FrameOutcome,
) -> Result<(), RendererError> {
    let frame_inputs = &frame.inputs;
    let plan = &frame.plan;
    // CA-10 fault knob: the RECORD boundary (Phase 7, post-acquire,
    // after the semaphore growth) — a plan armed here fires with
    // nothing recorded and the slot fence still signaled; the
    // release's command-buffer reset covers the mid-recording shape
    // (a failure later in this cascade) identically.
    ctx.check_lifecycle_fault(crate::LifecycleFaultSite::FrameRecord)?;
    let corner_params = frame_inputs.corner_params.as_ref();
    let proxy = frame_inputs.proxy.as_ref();
    let shadowed = frame_inputs.shadowed.as_ref();
    let sky = frame_inputs.sky.as_ref();
    let wet = frame_inputs.wet.as_ref();
    let view_proj = &frame_inputs.view_proj;
    let foreground = frame_inputs.foreground;

    // Record.
    let command = frames.command_buffers[slot];
    let framebuffer = swapchain.framebuffers[image_index as usize];
    let extent = swapchain.extent;
    // The clear color (hoisted: the Era-1 post chain's HDR scene
    // pass clears the SAME color the main pass would — the value
    // is identical, only the attachment's format differs).
    let clear_color = corner_params.map_or_else(
        || shadowed.map_or([0.02, 0.02, 0.04, 1.0], |params| params.clear_color()),
        corner::FrameParams::clear_color,
    );
    let clear_values = [
        vk::ClearValue {
            color: vk::ClearColorValue {
                float32: clear_color,
            },
        },
        vk::ClearValue {
            depth_stencil: vk::ClearDepthStencilValue {
                depth: 1.0,
                stencil: 0,
            },
        },
    ];

    // Disjoint window-depth intervals allow self-occlusion in both layers
    // using one depth attachment, without a clear, barrier or extra pass.
    // The gap at the boundary also accommodates fixed-point depth formats.
    // Even D16 gives a viewmodel 4096 depth levels. Leave one full
    // D16 quantum between intervals, including both clip-plane endpoints.
    const FOREGROUND_MAX_DEPTH: f32 = 1.0 / 16.0;
    let world_min_depth = if outcome.foreground_drawn > 0 {
        FOREGROUND_MAX_DEPTH + 1.0 / 65535.0
    } else {
        0.0
    };
    let viewport = vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: extent.width as f32,
        height: extent.height as f32,
        min_depth: world_min_depth,
        max_depth: 1.0,
    };
    let scissor = vk::Rect2D {
        extent,
        offset: vk::Offset2D { x: 0, y: 0 },
    };

    // The targets were ensured pre-acquire — finding 3's stranded-frame
    // corrective. This comment marks the original call sites (removed).

    unsafe {
        ctx.device
            .begin_command_buffer(
                command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(|error| RendererError::vk(error, "begin_command_buffer"))?;

        if frames.timestamps_supported {
            ctx.device
                .cmd_reset_query_pool(command, frames.timestamp_pool, (slot as u32) * 2, 2);
            ctx.device.cmd_write_timestamp(
                command,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                frames.timestamp_pool,
                (slot as u32) * 2,
            );
        }

        // The 6-F shadow depth pass — recorded BEFORE the main render
        // pass (the pass's final subpass dependency orders the map's
        // writes before the main pass's fragment reads). The extended
        // FrameParams + light matrix are written into this slot's
        // mapped region first (the fence wait above owns the slot).
        if let (Some(corner), Some(shadow_state), Some(params)) =
            (&ctx.corner, &pipes.shadows, shadowed)
        {
            shadow_state.write_frame(slot, params);
            shadow_state.record_depth_pass(
                ctx,
                command,
                corner,
                slot,
                pipes.shadowed_proxy.as_ref(),
                pipes.proxy.as_ref(),
                ctx.skinned_retained.as_ref(),
                proxy,
            );
        }

        // The Era-1 wet reflection's mirrored-scene pass (package 7)
        // — recorded BEFORE the scene pass: the mirrored world
        // rendered into this slot's offscreen target (the world-
        // mirror construction, planar.rs's matrix), then the
        // barrier ordering its writes before the ground re-draw's
        // fragment reads inside the scene pass that follows. The
        // ground re-draw itself lands INSIDE whichever scene pass
        // runs (the post branch's or the main one's), after the
        // mesh draws.
        if let (Some(corner), Some(reflection_state), Some(post_state), Some(camera)) =
            (&ctx.corner, &pipes.reflection, &pipes.post, wet)
        {
            // SAFETY: this slot's fence wait passed above (the
            // mirrored params region is disjoint per slot); the
            // command buffer is recording, before the scene pass;
            // the targets were ensured against the live extent and
            // this same HDR pass object after the acquire.
            reflection_state.write_frame(slot, camera);
            reflection_state.record_reflection_pass(
                ctx,
                corner,
                command,
                post_state.hdr_render_pass(),
                slot,
                extent,
                clear_color,
                &corner.reflected_ranges(),
            );
            // THE B14/R5 DEPTH-PAIR WAW: the barrier's depth entry
            // rides exactly the frames whose scene pass is the HDR
            // one (this wet frame renders through the post chain) —
            // this slot's shared depth image, the same image the
            // reflection pass above just wrote and the HDR scene
            // pass below clears and writes. The ensure ladder above
            // built (or matched) both families' extent targets with
            // the shared bundle on this plan, so the expect is the
            // ensure discipline's own invariant.
            reflection_state.record_read_barrier(
                ctx,
                command,
                slot,
                plan.depth_pair_waw().then(|| {
                    pipes
                        .shared_depth
                        .as_ref()
                        .expect("a wet+post frame's ensures created the shared depth bundle")
                        .images()[slot]
                }),
            );
        }

        // The Era-1 post chain (package 6): a post frame REPLACES
        // the main render pass with the HDR chain — the scene pass
        // into the offscreen R16G16B16A16 target, the bloom mips,
        // and the composite + tonemap into the acquired swapchain
        // image (post_draw's module header documents the whole
        // sequence and its synchronization). A postless frame takes
        // the main pass below, byte-identical to the pre-package
        // renderer (the post state may exist — the belt check
        // keeps a postless frame off the chain regardless). The
        // branch is the plan's post bit — the scene-pass variant
        // declaration (S3).
        if plan.uses.post
            && let Some(post_state) = &pipes.post
        {
            let inputs = post_draw::PostFrameInputs {
                slot,
                image_index,
                extent,
                clear_color,
                corner_params,
                proxy,
                proxy_sets,
                shadowed,
                sky,
                // The wet receiver's placement, derived once in the
                // plan (the HDR variant here — the post chain's scene
                // pass is the one a wet post frame records through).
                ground: plan.ground,
            };
            // SAFETY: the command buffer is recording past the
            // shadow depth pass; this slot's fence wait passed (the
            // uniform writes inside are disjoint per slot); the
            // targets were ensured against the live swapchain
            // after the acquire; `image_index` is the acquired
            // image.
            outcome.proxy_draw_calls = post_state.record(ctx, pipes, command, &inputs);
        } else {
            ctx.device.cmd_begin_render_pass(
                command,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(pipes.render_pass)
                    .framebuffer(framebuffer)
                    .render_area(scissor)
                    .clear_values(&clear_values),
                vk::SubpassContents::INLINE,
            );
            ctx.device.cmd_set_viewport(command, 0, &[viewport]);
            ctx.device.cmd_set_scissor(command, 0, &[scissor]);
            // The unified corner-scene recorder (finding 20, Phase B):
            // extracts the shared sky/corner/shadowed-ordinary/proxy sequence
            // from the main pass and HDR pass. Returns Some(proxy_calls) when
            // corner-family (shadowed or ordinary) was recorded, None when
            // neither occurred (the else-branch body runs below).
            let targets = scene_record::SceneTargetPipelines {
                sky: pipes.sky.as_ref().map(|s| s.pipeline()),
                lit: Some(pipes.lit_pipeline),
                shadowed: pipes.shadows.as_ref().map(|s| s.shadowed(slot).0),
                proxy: Some(pipes.proxy_pipeline),
                // The rain kind's main-pass pipeline (the unlit
                // blended particle object — the third populated
                // kind).
                proxy_rain: Some(pipes.rain_pipeline),
                proxy_skinned: pipes.skinned.as_ref().map(|s| s.pipeline()),
                shadowed_proxy_skinned: pipes
                    .shadowed_proxy
                    .as_ref()
                    .map(|s| s.skinned_shadowed_pipeline()),
                shadowed_proxy_vehicle: pipes
                    .shadowed_proxy
                    .as_ref()
                    .map(|s| s.plain_shadowed_pipeline()),
            };
            // The wet receiver's placement (finding 14): the ground
            // re-draw moved INSIDE the scene recorder, between the
            // opaque kinds and the translucent rain kind — the
            // ordered blend (the reflection over the opaque ground,
            // the rain over the reflection). The recorder's own
            // comments document the defect this corrects. The
            // placement is the plan's ground field — derived once
            // (the Main variant on a wet postless frame; the HDR
            // variant routes through the post chain's own recorder).
            let ground = plan.ground;
            match scene_record::record_corner_scene(
                ctx,
                pipes,
                command,
                slot,
                targets,
                corner_params,
                shadowed,
                sky,
                proxy,
                proxy_sets,
                ground,
            ) {
                Some(calls) => {
                    outcome.proxy_draw_calls = calls;
                }
                None => {
                    // The Stage-2 scene_pipeline/region/foreground else-branch body
                    // (the non-corner path: static scene + dynamic region/foreground).
                    ctx.device.cmd_bind_pipeline(
                        command,
                        vk::PipelineBindPoint::GRAPHICS,
                        pipes.scene_pipeline,
                    );
                    ctx.device.cmd_push_constants(
                        command,
                        pipes.scene_layout,
                        vk::ShaderStageFlags::VERTEX,
                        0,
                        bytemuck::bytes_of(view_proj),
                    );
                    // The static demonstration scene draws only when the explicit
                    // opt-in uploaded it (`scene_index_count > 0`, P3) — an
                    // omitted renderer binds nothing and draws nothing here; the
                    // scene pipeline below still draws the dynamic region
                    // geometry through the same bound state.
                    if pipes.scene_index_count > 0 {
                        ctx.device
                            .cmd_bind_vertex_buffers(command, 0, &[pipes.scene_vbo], &[0]);
                        ctx.device.cmd_bind_index_buffer(
                            command,
                            pipes.scene_ibo,
                            0,
                            vk::IndexType::UINT32,
                        );
                        ctx.device
                            .cmd_draw_indexed(command, pipes.scene_index_count, 1, 0, 0, 0);
                    }

                    if outcome.region.drawn > 0 {
                        let region_offset = (slot as vk::DeviceSize) * pipes.region_slot_bytes;
                        ctx.device.cmd_bind_vertex_buffers(
                            command,
                            0,
                            &[pipes.region_vbo],
                            &[region_offset],
                        );
                        if world_drawn > 0 {
                            ctx.device.cmd_draw(command, world_drawn, 1, 0, 0);
                        }
                        if let Some(layer) = foreground.filter(|_| outcome.foreground_drawn > 0) {
                            let near_viewport = vk::Viewport {
                                min_depth: 0.0,
                                max_depth: FOREGROUND_MAX_DEPTH,
                                ..viewport
                            };
                            ctx.device.cmd_set_viewport(command, 0, &[near_viewport]);
                            ctx.device.cmd_push_constants(
                                command,
                                pipes.scene_layout,
                                vk::ShaderStageFlags::VERTEX,
                                0,
                                bytemuck::bytes_of(layer.view_proj),
                            );
                            ctx.device.cmd_draw(
                                command,
                                outcome.foreground_drawn,
                                1,
                                world_drawn,
                                0,
                            );
                        }
                    }

                    // The Era-1 wet reflection's ground re-draw for
                    // the NON-CORNER path (the recorder never ran —
                    // the corner family's redraw lives inside it
                    // now, finding 14; this else-branch keeps the
                    // pre-corrective position, after the draws,
                    // byte-identical for the static-scene frames).
                    // The gate is the plan's wet bit (the frame's
                    // explicit declaration). SAFETY: the command
                    // buffer is inside the main render pass, past
                    // the scene draws; this slot's corner
                    // FrameParams region holds the true camera (the
                    // frame flow's write).
                    if plan.uses.wet
                        && let (Some(reflection_state), Some(corner)) =
                            (&pipes.reflection, &ctx.corner)
                    {
                        reflection_state.record_ground_redraw(
                            ctx,
                            corner,
                            command,
                            slot,
                            false,
                            corner.receiver_range(),
                        );
                    }
                }
            }

            if outcome.overlay.drawn > 0 {
                ctx.device.cmd_bind_pipeline(
                    command,
                    vk::PipelineBindPoint::GRAPHICS,
                    pipes.overlay_pipeline,
                );
                ctx.device.cmd_bind_vertex_buffers(
                    command,
                    0,
                    &[frames.overlay_slots[slot].buffer],
                    &[0],
                );
                ctx.device.cmd_bind_descriptor_sets(
                    command,
                    vk::PipelineBindPoint::GRAPHICS,
                    pipes.overlay_layout,
                    0,
                    &[ctx.descriptors.set],
                    &[],
                );
                ctx.device.cmd_draw(command, outcome.overlay.drawn, 1, 0, 0);
            }

            // The Era-1 wet reflection's ground re-draw inside the
            // MAIN pass moved INTO the scene recorder (finding 14:
            // between the opaque kinds and the translucent rain
            // kind — the ordered blend; see the recorder's comments
            // and the GroundRedrawPass::Main argument above). The
            // non-corner else-branch keeps its own call above. The
            // mirrored-scene pass and its barrier preceded this pass
            // (the frame flow's recording order, above the post
            // branch).

            ctx.device.cmd_end_render_pass(command);
        }
        // The 6-F known-answer readback: after the main pass the
        // shadow map sits in SHADER_READ — transition to TRANSFER_SRC
        // and copy the recording slot's map into the mapped buffer
        // (the submission fence proves the copy complete before
        // `take_shadow_map_capture` reads it). The handshake lives in
        // the frame state (replacement-safe). The request is consumed
        // ONLY when the frame actually ran the depth pass — the SAME
        // triple the depth-pass block above requires (corner scene,
        // shadow state, shadowed params) — so a plain frame leaves
        // `pending` untouched and the next shadowed frame records it
        // (the capture survives deferred frames until a frame
        // actually produces the map; the old-layout declaration
        // SHADER_READ below is then always the depth pass's final
        // layout, never a stale one).
        if frames.shadow_readback.pending
            && let (Some(_), Some(shadow_state), Some(_)) = (&ctx.corner, &pipes.shadows, shadowed)
        {
            shadow_state.record_readback(ctx, command, slot);
            frames.shadow_readback.pending = false;
            frames.shadow_readback.ready = true;
            frames.shadow_readback.slot = slot;
            frames.shadow_readback.buffer = shadow_state.readback_buffer();
        }
        if frames.timestamps_supported {
            ctx.device.cmd_write_timestamp(
                command,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                frames.timestamp_pool,
                (slot as u32) * 2 + 1,
            );
        }
        // The bounded frame-capture readback (the orientation
        // corrective's evidence path): after the render pass the image
        // sits in PRESENT_SRC — transition to TRANSFER_SRC, copy the
        // whole image into the mapped buffer, and transition back, all
        // inside this frame's own command buffer so the submission
        // fence proves the copy complete before `take_capture` reads.
        // The record-time guard (the post-orientation hardening + the
        // cache-validation corrective): the acquire path may have
        // recreated the swapchain AFTER the pre-acquire preparation —
        // a destination mismatched in extent, format, OR transfer-
        // source capability is never copied into; the capture is
        // DROPPED (a typed, observable drop) and the frame renders
        // normally.
        if frames.capture.pending {
            if frames.capture.matches(
                extent.width,
                extent.height,
                swapchain.format,
                swapchain.transfer_src_supported,
            ) {
                let image = swapchain.images[image_index as usize];
                let subresource = vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    level_count: 1,
                    layer_count: 1,
                    ..Default::default()
                };
                let to_transfer = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                    .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .image(image)
                    .subresource_range(subresource)
                    .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
                ctx.device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[to_transfer],
                );
                let copy = vk::BufferImageCopy::default()
                    .buffer_offset(0)
                    .buffer_row_length(0)
                    .buffer_image_height(0)
                    .image_subresource(vk::ImageSubresourceLayers {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        mip_level: 0,
                        base_array_layer: 0,
                        layer_count: 1,
                    })
                    .image_extent(vk::Extent3D {
                        width: extent.width,
                        height: extent.height,
                        depth: 1,
                    });
                ctx.device.cmd_copy_image_to_buffer(
                    command,
                    image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    frames.capture.buffer,
                    &[copy],
                );
                // The transfer write must be visible to the host reader.
                // The buffer is HOST_VISIBLE|HOST_COHERENT (create_mapped_buffer),
                // so no explicit invalidate is needed — the barrier establishes
                // the availability/visibility dependency.
                let buffer_barrier = vk::BufferMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::HOST_READ)
                    .buffer(frames.capture.buffer)
                    .offset(0)
                    .size(frames.capture.byte_len as vk::DeviceSize)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
                ctx.device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::HOST,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[buffer_barrier],
                    &[],
                );
                let to_present = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                    .image(image)
                    .subresource_range(subresource)
                    .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                    .dst_access_mask(vk::AccessFlags::empty())
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
                ctx.device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[to_present],
                );
                frames.capture.pending = false;
                frames.capture.ready = true;
                frames.capture.slot = slot;
            } else {
                // The prepared destination does not match the image
                // being recorded (the swapchain changed mid-frame —
                // its extent, format, or transfer-source capability):
                // drop the request — never copy into a mismatched or
                // unsupported destination — and record the typed,
                // observable drop.
                frames.capture.pending = false;
                frames.capture.last_refusal = Some(RendererError::CaptureDropped(format!(
                    "the swapchain changed between the capture preparation and the \
                     recording (extent {}×{}, format {:?}, transfer-src {})",
                    extent.width, extent.height, swapchain.format, swapchain.transfer_src_supported
                )));
            }
        }
        ctx.device
            .end_command_buffer(command)
            .map_err(|error| RendererError::vk(error, "end_command_buffer"))?;
    }
    Ok(())
}

/// Phase 8 — `submit` (§5.4): queue the recorded buffer — waiting
/// the acquire semaphore, signaling the image's present-wait
/// semaphore (corrective finding C) and the slot fence.
fn submit_command_buffer(
    ctx: &VulkanContext,
    frames: &mut FrameState,
    present_semaphore: vk::Semaphore,
    slot: usize,
) -> Result<(), RendererError> {
    // CA-10 fault knob: the SUBMIT boundary (Phase 8) — the one
    // post-acquire failure that lands AFTER the fence commit (the
    // reset ran, the ownership published): a plan armed here fires
    // with the fence reset-but-unsignaled, exercising the release's
    // heal arm (the discriminator finds NOT_READY; the empty
    // release submission re-signals the fence while consuming the
    // acquire semaphore).
    ctx.check_lifecycle_fault(crate::LifecycleFaultSite::FrameSubmit)?;
    let command = frames.command_buffers[slot];
    let wait_stage = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
    let wait_semaphores = [frames.image_available[slot]];
    let signal_semaphores = [present_semaphore];
    let commands = [command];
    let submit_info = vk::SubmitInfo::default()
        .wait_semaphores(&wait_semaphores)
        .wait_dst_stage_mask(&wait_stage)
        .command_buffers(&commands)
        .signal_semaphores(&signal_semaphores);
    unsafe {
        ctx.device
            .queue_submit(ctx.graphics_queue, &[submit_info], frames.in_flight[slot])
            .map_err(|error| RendererError::vk(error, "queue_submit"))?;
    }
    if frames.timestamps_supported {
        frames.timestamps_written[slot] = true;
    }
    Ok(())
}

/// Phase 9 — `present` (§5.4): hand the acquired image to the
/// presentation engine, waiting the image's present semaphore. An
/// out-of-date present is DEFERRED, not inline (the batch-3 review's
/// lifecycle finding: every recreation entry point honors surface
/// availability) — the presentation is gone; the recreation intent
/// and the deferral hint latch, and this frame's submission fence
/// still signals normally.
fn present_image(
    ctx: &VulkanContext,
    swapchain: &SwapchainState,
    frames: &mut FrameState,
    image_index: u32,
    present_semaphore: vk::Semaphore,
    outcome: &mut FrameOutcome,
) -> Result<(), RendererError> {
    let present_wait_semaphores = [present_semaphore];
    let present_swapchains = [swapchain.swapchain];
    let present_image_indices = [image_index];
    let present_info = vk::PresentInfoKHR::default()
        .wait_semaphores(&present_wait_semaphores)
        .swapchains(&present_swapchains)
        .image_indices(&present_image_indices);
    match unsafe {
        ctx.swapchain_ext
            .queue_present(ctx.graphics_queue, &present_info)
    } {
        Ok(present_suboptimal) => {
            outcome.presented = true;
            if present_suboptimal || outcome.suboptimal {
                outcome.suboptimal = true;
                mark_suboptimal(&mut frames.needs_recreate);
            }
        }
        Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
            // DEFERRED, not inline (the batch-3 review's lifecycle
            // finding: every recreation entry point honors surface
            // availability): the presentation is gone; latch the
            // recreation intent and the deferral hint — the next
            // frame's probe-gated consume-and-recreate block rebuilds
            // once the drawable is available again. This frame's
            // submission completed and its fence will signal normally;
            // no synchronization state is touched here.
            mark_suboptimal(&mut frames.needs_recreate);
            frames.surface_probe_pending = true;
        }
        Err(error) => return Err(RendererError::vk(error, "queue_present")),
    }
    Ok(())
}

fn mut_at<T>(slice: &mut [T], index: usize) -> Option<&mut T> {
    slice.get_mut(index)
}

fn read_timestamps(ctx: &VulkanContext, frames: &FrameState, slot: usize) -> Option<f64> {
    let mut stamps = [0u64; 2];
    let result = unsafe {
        ctx.device.get_query_pool_results(
            frames.timestamp_pool,
            (slot as u32) * 2,
            &mut stamps,
            vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
        )
    };
    if result.is_err() {
        return None;
    }
    // Timestamp counters may be narrower than 64 bits (corrective
    // finding E): mask the difference to the valid width so a wrapped
    // counter still yields the correct interval.
    let bits = ctx.timestamp_valid_bits.min(64);
    let mask = if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    let delta = stamps[1].wrapping_sub(stamps[0]) & mask;
    Some(delta as f64 * ctx.timestamp_period_ns / 1_000_000.0)
}

/// Timestamp read policy (corrective finding E): query results are read
/// only when the queue family supports timestamps AND the slot's queries
/// have resolved at least once — never block-read unwritten queries.
pub(crate) fn timestamp_sample_ready(supported: bool, written: bool) -> bool {
    supported && written
}

/// Presentation-wait semaphore table growth (corrective finding C): the
/// table only ever grows to cover the required length — entries are never
/// invalidated, because recreation idles the device first.
pub(crate) fn present_semaphore_len_for(current_len: usize, required: usize) -> usize {
    current_len.max(required)
}

#[cfg(test)]
mod policy_tests {
    use super::*;
    use crate::proxy_instances::generation_write_needed;

    /// THE A04 ZERO-COST STATEMENT (design §3.4, the type-level
    /// half): the collapsed parameter set is plain stack data —
    /// [`FramePolicy`] and [`InternalFrame`] are `Copy` borrow
    /// bundles (no `Vec`, no `Box`, no trait objects), so the 19→6
    /// collapse introduced no per-frame heap allocation and no
    /// clone. The behavioral half of the proof is the batteries'
    /// identical `FrameOutcome`s and allocation counters (S1's
    /// byte-equality gates and the allocation-constancy tuples,
    /// which run unchanged against the collapsed flow).
    #[test]
    fn the_collapsed_frame_parameters_are_copy_stack_data() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<FramePolicy>();
        assert_copy::<InternalFrame<'static>>();
        assert_copy::<FramePlan>();
        assert_copy::<FrameOutcome>();
    }

    #[test]
    fn timestamp_gate_requires_support_and_a_resolved_sample() {
        for supported in [false, true] {
            for written in [false, true] {
                assert_eq!(
                    timestamp_sample_ready(supported, written),
                    supported && written,
                    "supported={supported} written={written}"
                );
            }
        }
    }

    #[test]
    fn present_semaphore_table_only_grows() {
        let mut len = 2;
        for acquired in [0u32, 1, 5, 3] {
            len = present_semaphore_len_for(len, acquired as usize + 1);
            assert!(len > acquired as usize);
        }
        assert!(len >= 6);
        assert_eq!(present_semaphore_len_for(6, 4), 6);
    }

    #[test]
    fn overlay_slots_grow_on_demand_to_the_existing_vertex_budget() {
        let limit = crate::overlay::overlay_capacity_vertices() / FRAMES_IN_FLIGHT;
        assert_eq!(OverlaySlot::capacity_for(0, 0), 0);
        assert_eq!(OverlaySlot::capacity_for(0, 37), 64);
        assert_eq!(OverlaySlot::capacity_for(64, 0), 64);
        assert_eq!(OverlaySlot::capacity_for(64, 64), 64);
        assert_eq!(OverlaySlot::capacity_for(64, usize::MAX), limit);
        assert_eq!(OverlaySlot::capacity_for(limit, usize::MAX), limit);
    }

    /// CA-12 (F12): the upload backstop's bound — exactly the
    /// carve's byte length admits, one byte past it refuses with the
    /// population named, zero always admits (an empty population
    /// writes nothing). The pre-fix sites gated this with
    /// `debug_assert` alone (a release-mode write past the mapping).
    #[test]
    fn admitted_bytes_fit_exactly_at_the_carve_and_refuses_past_it() {
        use crate::animation::POSE_STRIDE;
        use crate::proxy_instances::INSTANCE_STRIDE;
        // The floor quantum's real carves: 64 instances × 96 B and
        // 64 pose blocks × 704 B.
        let instance_slot = 64 * INSTANCE_STRIDE as u64;
        let pose_slot = 64 * POSE_STRIDE as u64;
        admitted_bytes_fit(instance_slot as usize, instance_slot, "instance population")
            .expect("exactly the carve admits");
        admitted_bytes_fit(pose_slot as usize, pose_slot, "pose population")
            .expect("exactly the carve admits");
        admitted_bytes_fit(0, 0, "instance population").expect("an empty population admits");
        for (what, admitted, slot_bytes) in [
            (
                "instance population",
                instance_slot as usize + 1,
                instance_slot,
            ),
            ("pose population", pose_slot as usize + 1, pose_slot),
            ("instance population", usize::MAX, instance_slot),
        ] {
            let error = admitted_bytes_fit(admitted, slot_bytes, what)
                .expect_err("one byte past the carve refuses");
            let RendererError::Vulkan(text) = &error else {
                panic!("the refusal is a Vulkan error: {error:?}");
            };
            assert!(
                text.contains(what)
                    && text.contains("physical carve")
                    && text.contains("refusing rather than writing past the mapping"),
                "the refusal names the population and the carve: {text}"
            );
        }
    }

    /// P5 §3.3 (A18/R2) — THE §3.5 ORACLE HARNESS, the write half
    /// (the uploads policy_tests class, device-free): a fuzzed
    /// (generation, population) sequence driven through BOTH
    /// protocol levels — the gated repacks (the safe layer's CPU
    /// half) and the per-slot stamp gates (this module's upload
    /// half, modeled as slot-region byte stores exactly the mapped
    /// writes perform) — asserting at EVERY frame that each slot's
    /// regions hold EXACTLY the bytes the always-write path
    /// produces (the oracle: fresh buffers, generation 0, every
    /// region copied every frame). The slot index rotates the frame
    /// flow's own ping-pong (frame_slot advancing modulo
    /// FRAMES_IN_FLIGHT), so the between-generation slot-N-2 hazard
    /// (the design's named risk) is exercised by construction: a
    /// stable population pays one write per slot per generation and
    /// then zero, and a bumped generation rewrites BOTH slots.
    #[test]
    fn population_generation_protocol_matches_the_always_write_oracle() {
        use crate::animation::{
            compose_joint_matrices, humanoid_skeleton, repack_poses_generation, rest_pose_block,
            skin_matrices, walk_clip,
        };
        use crate::proxy_indirect::pack_indirect_block;
        use crate::proxy_instances::{
            InstanceTransform, ProxyInstance, repack_populations_generation,
        };

        let slots = FRAMES_IN_FLIGHT as usize;
        let skeleton = humanoid_skeleton();
        let clip = walk_clip();
        let rest = rest_pose_block();
        let walker_at = |t: f32| {
            let pose = crate::animation::sample_pose(&clip, t);
            let joint_worlds = compose_joint_matrices(&skeleton, &pose);
            skin_matrices(&joint_worlds, &skeleton)
        };
        let instance = |x: f32, turn: u16, pose_index: u32| ProxyInstance {
            transform: InstanceTransform {
                pos: [x, 0.02, -2.0],
                turn,
                scale: 1.0,
            },
            tint: [0.8, 0.7, 0.6],
            alpha: 1.0,
            pose_index,
        };
        // The protocol side: the renderer's retained buffers + their
        // generation records + the frame state's stamps + the slot
        // regions (the mapped carves' model).
        let mut gated_bytes = Vec::new();
        let mut gated_poses = Vec::new();
        let mut generations = crate::submission::PopulationGenerations::default();
        let mut stamps = PopulationStamps::default();
        let mut slot_instances = vec![Vec::<u8>::new(); slots];
        let mut slot_poses = vec![Vec::<u8>::new(); slots];
        let mut slot_indirect = vec![Vec::<u8>::new(); slots];
        // T02 §2.3: the modeled carve epochs (the minted identities
        // the extended gate compares). The growth events below
        // replace a carve WITHOUT any stamp clear — the belt-missing
        // shape the audit's delayed-invalidation finding sketched —
        // so the ORACLE equality holds only if the epoch alone
        // forces the rewrite (the red witness of the bare-generation
        // gate: it would skip and keep the replaced carve's
        // uninitialized bytes).
        let mut instance_carve_epoch = 1_u64;
        let mut pose_carve_epoch = 1_u64;
        // The oracle side: always-write (generation 0, fresh records,
        // every region copied every frame).
        let mut oracle_bytes = Vec::new();
        let mut oracle_poses = Vec::new();
        let mut oracle_generations = crate::submission::PopulationGenerations::default();
        let mut oracle_slot_instances = vec![Vec::<u8>::new(); slots];
        let mut oracle_slot_poses = vec![Vec::<u8>::new(); slots];
        let mut oracle_slot_indirect = vec![Vec::<u8>::new(); slots];
        // The sequence state: a render-then-step crowd — instances
        // move on ticks (every 3rd frame), poses advance on ticks
        // (every 2nd frame), and between ticks the population is
        // byte-identical (the submission the protocol must make
        // free). An always-dirty caller interleaves on the frames
        // congruent to 7 mod 11 (the un-opted route sharing the
        // renderer).
        let mut rng: u64 = 0x13198A2E03707344;
        let mut instance_generation = 1_u64;
        let mut pose_generation = 1_u64;
        let mut x = 1.0_f32;
        let mut pose_t = 0.0_f32;
        let mut skipped = 0_usize;
        let mut wrote = 0_usize;
        for frame in 0..48_u64 {
            let slot = (frame as usize) % slots;
            rng = rng.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let tick = frame % 3 == 0;
            let pose_tick = frame % 2 == 0;
            if tick {
                instance_generation += 1;
                x += (rng % 100) as f32 * 0.01;
            }
            if pose_tick {
                pose_generation += 1;
                pose_t = (pose_t + 0.017) % 1.1;
            }
            let instances = [instance(x, 1000, 0), instance(-3.0, 40000, 1)];
            let poses = [rest, walker_at(pose_t)];
            let un_opted = frame % 11 == 7;
            let (declared_instance, declared_pose) = if un_opted {
                (0, 0)
            } else {
                (instance_generation, pose_generation)
            };
            // T02: the modeled carve replacements — growth commits
            // (or pipeline replacements) publishing FRESH epochs with
            // no content copy and NO stamp clear (the belt's absence
            // isolates the epoch gate: the oracle below still must
            // match).
            if frame % 17 == 5 {
                instance_carve_epoch += 1;
            }
            if frame % 13 == 9 {
                pose_carve_epoch += 1;
            }
            // ---- The gated path (both levels) ----
            let instance_decision = repack_populations_generation(
                &mut gated_bytes,
                &instances,
                &[],
                &[],
                crate::proxy_instances::MAX_INSTANCES,
                declared_instance,
                &mut generations.instance,
            )
            .unwrap();
            let pose_decision = repack_poses_generation(
                &mut gated_poses,
                &poses[..],
                crate::animation::MAX_POSE_BLOCKS,
                declared_pose,
                &mut generations.pose,
            )
            .unwrap();
            // T05 (strict audit IV): the renderer's submission-time
            // stamp invalidation, modeled — a REPACKED half's stamps
            // reset on every slot before the gates (a colliding
            // generation number from a second source must not keep
            // a stamp vouching for the first source's bytes). For
            // this single-source sequence the rule is
            // outcome-identical to the former bare-generation gate
            // (a bump already rewrote every slot once), which is
            // exactly what the oracle below re-proves frame by
            // frame.
            if instance_decision == crate::proxy_instances::GenerationDecision::Repacked {
                stamps.clear_instance();
            }
            if pose_decision == crate::proxy_instances::GenerationDecision::Repacked {
                stamps.clear_pose();
            }
            if population_write_needed(
                stamps.instance_epochs[slot],
                stamps.instance[slot],
                instance_carve_epoch,
                declared_instance,
            ) {
                slot_instances[slot].clear();
                slot_instances[slot].extend_from_slice(&gated_bytes);
                stamps.instance[slot] = declared_instance;
                stamps.instance_epochs[slot] = instance_carve_epoch;
                wrote += 1;
            } else {
                skipped += 1;
            }
            let indirect_block = pack_indirect_block(396, 2, 552, 0, 6, 0);
            if population_write_needed(
                stamps.indirect_epochs[slot],
                stamps.indirect[slot],
                instance_carve_epoch,
                declared_instance,
            ) {
                slot_indirect[slot] = indirect_block.to_vec();
                stamps.indirect[slot] = declared_instance;
                stamps.indirect_epochs[slot] = instance_carve_epoch;
                wrote += 1;
            } else {
                skipped += 1;
            }
            if population_write_needed(
                stamps.pose_epochs[slot],
                stamps.pose[slot],
                pose_carve_epoch,
                declared_pose,
            ) {
                slot_poses[slot].clear();
                slot_poses[slot].extend_from_slice(&gated_poses);
                stamps.pose[slot] = declared_pose;
                stamps.pose_epochs[slot] = pose_carve_epoch;
                wrote += 1;
            } else {
                skipped += 1;
            }
            // ---- The oracle path (always-write) ----
            repack_populations_generation(
                &mut oracle_bytes,
                &instances,
                &[],
                &[],
                crate::proxy_instances::MAX_INSTANCES,
                0,
                &mut oracle_generations.instance,
            )
            .unwrap();
            repack_poses_generation(
                &mut oracle_poses,
                &poses[..],
                crate::animation::MAX_POSE_BLOCKS,
                0,
                &mut oracle_generations.pose,
            )
            .unwrap();
            for (region, bytes) in [
                (&mut oracle_slot_instances[slot], &oracle_bytes),
                (&mut oracle_slot_poses[slot], &oracle_poses),
            ] {
                region.clear();
                region.extend_from_slice(bytes);
            }
            oracle_slot_indirect[slot] = indirect_block.to_vec();
            // ---- The equivalence: every region, every frame ----
            assert_eq!(
                slot_instances, oracle_slot_instances,
                "frame {frame}: every slot's instance region is the always-write bytes"
            );
            assert_eq!(
                slot_poses, oracle_slot_poses,
                "frame {frame}: every slot's pose region is the always-write bytes"
            );
            assert_eq!(
                slot_indirect, oracle_slot_indirect,
                "frame {frame}: every slot's indirect block is the always-write bytes"
            );
        }
        // The protocol actually did its job: skips occurred (the
        // between-tick frames) AND writes occurred (the ticks, the
        // always-dirty interleave, and the first pass over each
        // slot) — a harness that skipped nothing or wrote nothing
        // proves nothing.
        assert!(skipped > 0, "the sequence exercised generation skips");
        assert!(wrote > 0, "the sequence exercised real writes");
        // The ping-pong shape's headline: after the LAST stable
        // stretch (no tick on the trailing frames), every slot's
        // stamp records the current generations — the stable
        // population pays one write per slot per generation and then
        // zero. The final two frames are tick-free by construction
        // (47 % 3 != 0, 46 % 3 != 0), so both slots must be stamped
        // at the final pose generation... asserted through the gate
        // itself: no further write is needed on either slot.
        for slot in 0..slots {
            assert!(
                !population_write_needed(
                    stamps.instance_epochs[slot],
                    stamps.instance[slot],
                    instance_carve_epoch,
                    instance_generation
                ) || stamps.instance[slot] == 0,
                "slot {slot}: a stable population on a stable carve needs no further write"
            );
        }
    }

    /// T02 (strict audit IV, §2.3) — THE EPOCH GATE'S DECISION TABLE,
    /// isolated (the resource window alone, no repack clear, no
    /// growth-delta belt): a REPLACED carve (a fresh minted epoch)
    /// rewrites at EVERY generation — even the surviving one —
    /// because the bytes vouched for the OLD identity; the unknown
    /// stamp (epoch 0, never minted) writes; the stable
    /// epoch+generation pair skips (the R2 reuse preserved exactly);
    /// the generation-0 always-dirty rule is unchanged. This is the
    /// CPU half of the interrupted-growth witness (the rig leg
    /// drives the end-to-end property; the epoch arm is isolated
    /// HERE because on a live head the T05 clears fire beside it).
    #[test]
    fn the_epoch_gate_decision_table_pins_the_resource_window() {
        use crate::proxy_instances::population_write_needed;
        // The stable pair skips (the R2 win, unchanged).
        assert!(!population_write_needed(1, 7, 1, 7));
        // A generation change at the same epoch writes.
        assert!(population_write_needed(1, 7, 1, 8));
        // Generation 0 is always-dirty.
        assert!(population_write_needed(1, 7, 1, 0));
        // THE EPOCH ARM: the carve was replaced (a fresh minted
        // epoch) — the rewrite fires at EVERY generation, including
        // the one the stamp already records (the delayed-
        // invalidation hazard's exact shape: the growth committed,
        // the frame failed before the delta clear, the retry
        // presents the surviving generation).
        assert!(population_write_needed(1, 7, 2, 7));
        assert!(
            population_write_needed(1, 0, 2, 0),
            "generation 0 stays always-dirty"
        );
        assert!(population_write_needed(1, 7, 2, 8));
        // The unknown stamp (epoch 0 — the initial state and what
        // every clear resets to) can never equal a minted epoch
        // (they start at 1): a fresh carve never inherits a stamp.
        assert!(population_write_needed(0, 7, 1, 7));
        assert!(population_write_needed(0, 0, 1, 0));
        // A stamp written into an OLDER epoch of the SAME carve
        // identity cannot occur (epochs are minted per publication);
        // the table's symmetry is checked anyway — any mismatch
        // writes.
        assert!(population_write_needed(2, 7, 1, 7));
    }

    /// T02 §2.3 — THE BELT-MISSING WITNESS, slot-level: both slots
    /// stamped at (epoch E, generation g); the carve's growth commit
    /// publishes epoch E+1 with NO stamp clear (the frame flow's
    /// delta-clear belt absent — the interrupted-growth shape); the
    /// extended gate rewrites BOTH slots at the SAME generation g
    /// (the bare-generation gate — the pre-fix rule — returns FALSE
    /// and would keep vouching the replaced carve's uninitialized
    /// bytes: the RED witness, constructible at the audit's SHA
    /// where no T05 clear fired either). After the rewrite the
    /// stable skip returns (the R2 win preserved).
    #[test]
    fn an_epoch_bump_rewrites_both_slots_without_any_clear() {
        use crate::proxy_instances::{generation_write_needed, population_write_needed};
        let slots = FRAMES_IN_FLIGHT as usize;
        let mut stamps = PopulationStamps {
            instance: [5; FRAMES_IN_FLIGHT as usize],
            indirect: [5; FRAMES_IN_FLIGHT as usize],
            pose: [5; FRAMES_IN_FLIGHT as usize],
            instance_epochs: [1; FRAMES_IN_FLIGHT as usize],
            indirect_epochs: [1; FRAMES_IN_FLIGHT as usize],
            pose_epochs: [1; FRAMES_IN_FLIGHT as usize],
            ..Default::default()
        };
        // The stable pair skips under BOTH gates (the pre-fix
        // behavior at a stable carve).
        for slot in 0..slots {
            assert!(!generation_write_needed(stamps.instance[slot], 5));
            assert!(!population_write_needed(
                stamps.instance_epochs[slot],
                stamps.instance[slot],
                1,
                5
            ));
        }
        // THE CARVE REPLACEMENT (the growth commit's fresh epoch),
        // with NO clear of any kind:
        let replaced_epoch = 2;
        for slot in 0..slots {
            // The RED half: the bare-generation gate alone returns
            // FALSE (it cannot see the carve's identity).
            assert!(
                !generation_write_needed(stamps.instance[slot], 5),
                "the bare gate is exactly the delayed-invariant hazard"
            );
            // The GREEN half: the extended gate forces the rewrite.
            assert!(
                population_write_needed(
                    stamps.instance_epochs[slot],
                    stamps.instance[slot],
                    replaced_epoch,
                    5
                ),
                "slot {slot}: a replaced carve rewrites at the surviving generation"
            );
            // The write records the NEW epoch beside the generation.
            stamps.instance_epochs[slot] = replaced_epoch;
        }
        // The stable skip returns on the new carve (the R2 win).
        for slot in 0..slots {
            assert!(!population_write_needed(
                stamps.instance_epochs[slot],
                stamps.instance[slot],
                replaced_epoch,
                5
            ));
        }
        // T05's clears still reset the epoch halves (the identity
        // window composing with the resource window).
        stamps.clear_instance();
        for slot in 0..slots {
            assert_eq!(stamps.instance_epochs[slot], 0);
            assert_eq!(stamps.indirect_epochs[slot], 0);
            assert!(population_write_needed(
                stamps.instance_epochs[slot],
                stamps.instance[slot],
                replaced_epoch,
                5
            ));
        }
        stamps.clear_pose();
        for slot in 0..slots {
            assert_eq!(stamps.pose_epochs[slot], 0);
        }
    }

    /// P5 §3.3 — the ping-pong shape, directly: a STABLE opted-in
    /// population over six frames on two slots writes each slot's
    /// region exactly ONCE (its first frame) and skips every later
    /// frame; a generation bump rewrites both slots exactly once
    /// more. This is the design's §3.3(1) headline made assertable
    /// at the stamp level.
    #[test]
    fn stable_population_writes_each_slot_once_per_generation() {
        let slots = FRAMES_IN_FLIGHT as usize;
        let mut stamps = PopulationStamps::default();
        let mut writes = vec![0_usize; slots];
        for frame in 0..6 {
            let slot = frame % slots;
            if generation_write_needed(stamps.instance[slot], 7) {
                stamps.instance[slot] = 7;
                writes[slot] += 1;
            }
        }
        assert_eq!(writes, vec![1; slots], "one write per slot per generation");
        // The bump: both slots rewrite once.
        let mut bumped = vec![0_usize; slots];
        for frame in 0..(2 * slots) {
            let slot = frame % slots;
            if generation_write_needed(stamps.instance[slot], 8) {
                stamps.instance[slot] = 8;
                bumped[slot] += 1;
            }
        }
        assert_eq!(
            bumped,
            vec![1; slots],
            "the bumped generation rewrites both slots"
        );
        // An interleaved always-dirty frame resets the stamp it
        // writes, so the next opted-in frame cannot skip against it.
        let slot = 0;
        assert!(generation_write_needed(stamps.instance[slot], 0));
        stamps.instance[slot] = 0;
        assert!(
            generation_write_needed(stamps.instance[slot], 8),
            "a generation-0 write broke the stamp's vouching"
        );
    }

    /// P5 §3.3 — THE RESOURCE-REPLACEMENT INVALIDATION semantics:
    /// `clear()` returns every stamp to unknown, so a population
    /// whose generation a slot ALREADY recorded writes again after
    /// a replacement (a carve growth or a swapchain recreation
    /// replaced the mapped buffer with no content copy — the new
    /// region holds nothing the stamp can vouch for). The wiring
    /// sites are `recreate_swapchain` (the recreation arm) and the
    /// frame flow's arena-growth delta (the growth arm); this test
    /// pins the state contract both sites rely on.
    #[test]
    fn stamp_clear_forces_rewrites_after_replacement() {
        let mut stamps = PopulationStamps::default();
        for slot in 0..FRAMES_IN_FLIGHT as usize {
            stamps.instance[slot] = 5;
            stamps.pose[slot] = 5;
            stamps.indirect[slot] = 5;
            assert!(!generation_write_needed(stamps.instance[slot], 5));
            assert!(!generation_write_needed(stamps.pose[slot], 5));
            assert!(!generation_write_needed(stamps.indirect[slot], 5));
        }
        stamps.clear();
        for slot in 0..FRAMES_IN_FLIGHT as usize {
            assert_eq!(stamps.instance[slot], 0, "slot {slot}: unknown again");
            assert_eq!(stamps.pose[slot], 0);
            assert_eq!(stamps.indirect[slot], 0);
            assert!(generation_write_needed(stamps.instance[slot], 5));
            assert!(generation_write_needed(stamps.pose[slot], 5));
            assert!(generation_write_needed(stamps.indirect[slot], 5));
        }
    }

    /// T05 (strict audit IV) — THE SLOT-LEVEL COLLISION WITNESS,
    /// green half: the CPU half REPACKED the retained buffer at a
    /// generation some slot's stamp already records (two sources
    /// with colliding generation spaces — the second source
    /// re-admits at the same nonzero number, or a same-generation
    /// count change forces the count guard). The bare-generation
    /// gate (`generation_write_needed(stamp, generation)`) alone
    /// returns FALSE and the mapped region would keep the FIRST
    /// source's bytes — the RED run demonstrated exactly that skip.
    /// The production rule (the renderer's stamp invalidation on
    /// the [`crate::submission::PopulationRepacked`] signal,
    /// [`PopulationStamps::clear_instance`]) resets the half's
    /// stamps on every slot BEFORE the gate — modeled here as the
    /// renderer performs it.
    #[test]
    fn a_colliding_generation_repack_must_rewrite_the_slot_regions() {
        let slots = FRAMES_IN_FLIGHT as usize;
        // Both slots consumed source A's generation-5 bytes.
        let mut stamps = PopulationStamps {
            instance: [5; FRAMES_IN_FLIGHT as usize],
            indirect: [5; FRAMES_IN_FLIGHT as usize],
            ..Default::default()
        };
        assert_eq!(slots, 2, "the pinned ping-pong shape");
        // The CPU half repacked source B at the SAME generation 5
        // (the collision): the renderer's submission-time
        // invalidation fires — every slot, because the OTHER slot's
        // stamp is exactly the one equal to the colliding
        // generation.
        stamps.clear_instance();
        for slot in 0..slots {
            assert!(
                generation_write_needed(stamps.instance[slot], 5),
                "slot {slot}: a repacked buffer at a colliding generation must rewrite"
            );
            assert!(
                generation_write_needed(stamps.indirect[slot], 5),
                "slot {slot}: the indirect block must follow the repack"
            );
        }
    }

    /// T05 — THE INDIRECT PARTITION COLLISION WITNESS, green half:
    /// byte-identical instance payload, DIFFERENT npc/vehicle/rain
    /// partition, same generation. The payload gate reuses
    /// (correctly — the bytes are identical, even the adapter's
    /// debug byte-verify passes), but the indirect command block is
    /// a pure function of the PER-KIND COUNTS, which moved. The
    /// production rule (the per-kind count record beside the
    /// indirect stamp) writes on the count change even at a
    /// matching generation — modeled here as the upload site
    /// performs it. The RED run demonstrated the generation-only
    /// gate skipping exactly this shape.
    #[test]
    fn an_indirect_partition_change_at_a_matching_generation_must_rewrite() {
        let slots = FRAMES_IN_FLIGHT as usize;
        // Both slots hold source A's (2 npc, 1 vehicle, 0 rain)
        // indirect block at generation 5.
        let recorded_counts = [2_u32, 1, 0];
        let mut stamps = PopulationStamps {
            indirect: [5; FRAMES_IN_FLIGHT as usize],
            indirect_counts: [recorded_counts; FRAMES_IN_FLIGHT as usize],
            ..Default::default()
        };
        // Source C: the same three instances, partitioned (1, 2, 0)
        // — identical concatenated payload bytes, different counts.
        let submitted_counts = [1_u32, 2, 0];
        assert_ne!(recorded_counts, submitted_counts);
        for slot in 0..slots {
            let write = generation_write_needed(stamps.indirect[slot], 5)
                || stamps.indirect_counts[slot] != submitted_counts;
            assert!(
                write,
                "slot {slot}: the per-kind counts moved — the indirect \
                 block must rewrite even at a matching generation"
            );
            // And the write records the new counts (the identity the
            // next frame compares against).
            stamps.indirect[slot] = 5;
            stamps.indirect_counts[slot] = submitted_counts;
        }
        // A stable resubmission of source C now skips again (the
        // counts record vouches — the R2 reuse preserved).
        for slot in 0..slots {
            assert!(
                !(generation_write_needed(stamps.indirect[slot], 5)
                    || stamps.indirect_counts[slot] != submitted_counts),
                "slot {slot}: the recorded counts keep the stable skip"
            );
        }
    }
}
