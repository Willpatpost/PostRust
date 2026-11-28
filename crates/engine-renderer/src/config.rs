//! Renderer configuration and pure selection policies (Stage 2).

use ash::vk;

/// Preferred Vulkan present mode (Stage 2 authorization, item 3).
///
/// The engine's long-term direction is configurable high-refresh
/// presentation (60/144/240+ FPS targets and an uncapped measurement
/// mode); the present-mode choice is one axis of that. `Auto` selects the
/// vsync-on baseline (FIFO — always supported); explicit preferences fall
/// back to FIFO when the device does not expose them, and the resolved
/// mode plus the full supported set are reported in the render stats.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PresentModePreference {
    /// FIFO (vsync on) — the guaranteed baseline.
    #[default]
    Auto,
    Fifo,
    /// Triple-buffered low-latency vsync; falls back to FIFO.
    Mailbox,
    /// Tearing-allowed uncapped present; falls back to FIFO.
    Immediate,
}

impl PresentModePreference {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim_ascii().to_ascii_lowercase().as_str() {
            "auto" | "fifo" => Some(PresentModePreference::Auto),
            "mailbox" => Some(PresentModePreference::Mailbox),
            "immediate" => Some(PresentModePreference::Immediate),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PresentModePreference::Auto | PresentModePreference::Fifo => "fifo",
            PresentModePreference::Mailbox => "mailbox",
            PresentModePreference::Immediate => "immediate",
        }
    }
}

/// Validation-layer preference. `Auto` enables validation in debug builds
/// when the layer is installed; the renderer records whether it is
/// actually active so application errors stay distinguishable from
/// environment limitations. `On` additionally chains the
/// `VK_EXT_validation_features` enablement for SYNCHRONIZATION
/// validation (finding 9's explicit enablement — the layer's
/// execution/memory dependency modeling, so hazard-class errors are
/// counted in `RenderStats::validation_errors`); `Auto` keeps core
/// validation only (no development-loop cost).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ValidationPreference {
    #[default]
    Auto,
    On,
    Off,
}

impl ValidationPreference {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim_ascii().to_ascii_lowercase().as_str() {
            "auto" => Some(ValidationPreference::Auto),
            "on" => Some(ValidationPreference::On),
            "off" => Some(ValidationPreference::Off),
            _ => None,
        }
    }
}

/// The Stage-2 demonstration scene preference (Cycle 3, P3): scene
/// content is application data, not an engine mechanism — the
/// renderer must not impose the demonstration scene (ground plate,
/// grid, cube field) on a game's scene. `Omitted` — the DEFAULT —
/// uploads and draws no static demo scene; `Included` is the explicit
/// opt-in that preserves the original rendering demonstration
/// (`--render`, `--player`, `--demo-persist`, the render smokes)
/// exactly. The preference is per-renderer-instance and is carried
/// across every pipeline replacement.
///
/// P4 S6 deleted the third arm, `Corner`: the lit intersection's
/// authored content now reaches the renderer only through
/// [`crate::Renderer::attach_scene`] (engine-slice's
/// `corner_scene::corner_scene_description` builds it) — the
/// construction-time content import the arm performed was the
/// last authored-content path left inside the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DemoScenePreference {
    #[default]
    Omitted,
    Included,
}

/// Verification scenario for depth format fallback testing
/// (corrective finding 4): override the negotiated depth format.
/// Never set by normal operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DepthFormatPreference {
    /// Use the device's optimal negotiated depth format (the default).
    #[default]
    Auto,
    /// Force D24_UNORM_S8_UINT if supported; fall back to negotiated pick.
    ForceD24S8,
    /// Force D16_UNORM if supported; fall back to negotiated pick.
    ForceD16,
}

/// Diagnostic fault-injection site: which Era-1 target builder a
/// [`Renderer::inject_target_fault`] plan targets. Test-only,
/// diagnostic-only: never produced by real device calls, never
/// consumed by real code. (corrective finding 3+8 verification knobs)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetFaultSite {
    /// The post chain's post_draw::PostTargets
    Post,
    /// The reflection chain's reflection_draw::ReflectionTargets
    Wet,
    /// The shadow chain's shadow_draw::ShadowTargets
    Shadow,
}

/// Diagnostic fault-injection stage: which fallible step inside a
/// target builder's per-object construction sequence fires.
/// Test-only, diagnostic-only: never produced by real device calls,
/// never consumed by real code. (corrective finding 3+8 verification
/// knobs)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetFaultStage {
    /// The image creation step
    Image,
    /// The memory type lookup step
    MemoryLookup,
    /// The memory allocation step
    Memory,
    /// The memory binding step
    Bind,
    /// The image view creation step
    View,
    /// The framebuffer creation step
    Framebuffer,
}

/// Diagnostic fault-injection site for the renderer LIFECYCLE paths
/// (the CA-01/CA-10/CA-11 verification knobs — the lifecycle/sync
/// trio's answer to [`TargetFaultSite`]'s target-builder knobs): each
/// variant names ONE fallible step in the scene-attach path, the
/// post-acquire frame ladder, or the arena-growth replacement, and
/// [`Renderer::inject_lifecycle_fault`] arms a one-shot plan that
/// makes the ordinal-th matching step return
/// [`crate::RendererError::InjectedFault`] INSTEAD of its real device
/// call. Test-only, diagnostic-only: never produced by real device
/// calls, never consumed by real code.
///
/// The sites, by cluster:
///
/// **CA-01 (attach atomicity)** — the check sits immediately AFTER
/// each fallible creation step in
/// [`crate::Renderer::attach_scene`]'s two halves, so the injected
/// failure fires with that step's objects created and the rollback
/// guards below it must clean them up:
/// [`Self::SceneVertices`]/[`Self::SceneTexture`]/[`Self::SceneSampler`]/[`Self::SceneUniforms`]/[`Self::SceneDescriptors`]
/// (the corner-resource build) and
/// [`Self::LitModule`]/[`Self::LitLayout`]/[`Self::LitPipeline`]/[`Self::ProxyModule`]/[`Self::ProxyLayout`]/[`Self::ProxyPipeline`]/[`Self::RainModule`]/[`Self::RainPipeline`]
/// (the corner-pipeline group).
///
/// **CA-10 (the stranded slot)** — the four post-acquire frame
/// boundaries: [`Self::FrameUpload`], [`Self::FramePresentSemaphore`],
/// [`Self::FrameRecord`], [`Self::FrameSubmit`].
///
/// **CA-11 (the growth handle loss)** — the fallible `device_idle`
/// BETWEEN the new carve's allocation and the old carve's
/// retirement: [`Self::RegionGrowthIdle`], [`Self::InstanceGrowthIdle`],
/// [`Self::PoseGrowthIdle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleFaultSite {
    // ---- CA-01: the scene-attach path (corner-resource half) ----
    /// `CornerResources`' device-local vertex buffer upload.
    SceneVertices,
    /// One `CornerResources` texture upload (the ordinal selects
    /// which of the table's textures).
    SceneTexture,
    /// `CornerResources`' sampler pool.
    SceneSampler,
    /// `CornerResources`' mapped uniform buffer.
    SceneUniforms,
    /// `CornerResources`' descriptor pool/layout/set allocation.
    SceneDescriptors,
    // ---- CA-01: the scene-attach path (pipeline-group half) ----
    /// The lit shader module creation.
    LitModule,
    /// The lit pipeline layout creation.
    LitLayout,
    /// The lit pipeline creation.
    LitPipeline,
    /// The proxy shader module creation.
    ProxyModule,
    /// The proxy pipeline layout creation.
    ProxyLayout,
    /// The proxy pipeline creation.
    ProxyPipeline,
    /// The rain shader module creation.
    RainModule,
    /// The rain pipeline creation.
    RainPipeline,
    // ---- CA-10: the post-acquire frame boundaries ----
    /// The frame's mapped uploads (Phase 6).
    FrameUpload,
    /// The present-wait semaphore creation for the acquired image.
    FramePresentSemaphore,
    /// The command-buffer recording (Phase 7).
    FrameRecord,
    /// The queue submission (Phase 8).
    FrameSubmit,
    // ---- CA-11: the arena-growth idle windows ----
    /// The region carve's `device_idle` (after the new buffer, before
    /// the old one dies).
    RegionGrowthIdle,
    /// The instance carve's `device_idle` (same window).
    InstanceGrowthIdle,
    /// The pose carve's `device_idle` (same window).
    PoseGrowthIdle,
}

/// P5 §1.3 (A11/R1): the instance arena's configured HARD CAP — the
/// typed-refusal boundary ([`crate::proxy_instances::InstancePackError::Capacity`]
/// fires above it). The CAP, never the current carve, is the
/// boundary: the carve itself is demand-sized under this cap (see
/// [`arena_capacity_for`]), so "graceful capacity saturation" is
/// preserved exactly — a supply above the cap refuses with the same
/// typed error whether the carve is at 64 instances or at the cap.
///
/// The DEFAULT is [`crate::proxy_instances::MAX_INSTANCES`] —
/// today's constant — so every existing `RenderConfig` construction
/// (none of them name a budget field) resolves to exactly the
/// pre-P5 cap: refusals byte-identical, callers unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceCapacity(pub usize);

impl Default for InstanceCapacity {
    fn default() -> Self {
        Self(crate::proxy_instances::MAX_INSTANCES)
    }
}

/// P5 §1.3 (A11/R1): the pose arena's configured hard cap — the
/// typed-refusal boundary ([`crate::animation::PosePackError::Capacity`]
/// fires above it), mirroring [`InstanceCapacity`] (the pose bound
/// has always mirrored the instance bound). The DEFAULT is
/// [`crate::animation::MAX_POSE_BLOCKS`] — today's constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoseCapacity(pub usize);

impl Default for PoseCapacity {
    fn default() -> Self {
        Self(crate::animation::MAX_POSE_BLOCKS)
    }
}

/// P5 §1.3 (A11/R1): the dynamic region-scene arena's configured
/// budget, in PER-FRAME-SLOT vertices (one frame's world+foreground
/// output must fit one slot; the F-03 clamp truncates above it,
/// fail-visible with the counters). The DEFAULT is the closed
/// form's per-slot share —
/// `region_scene_capacity_vertices() / FRAMES_IN_FLIGHT` =
/// `((8 + 1000) * 36 + 512)` = 36,800 — whose derivation
/// (8 Full regions + 1000 NPC proxy boxes at 36 non-indexed
/// vertices, plus the 512 hand margin) stays documented at
/// [`crate::scene::region_scene_capacity_vertices`]. This is the
/// A05-coupling closure the P4 design named: the renderer's buffer
/// size is now a CONFIGURED budget whose default DERIVES from the
/// closed form, not the demo arithmetic welded into the carve site
/// — no engine-core constant ties the simulation's shape to the
/// renderer's residency after this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionCapacity(pub u32);

impl Default for RegionCapacity {
    fn default() -> Self {
        Self(crate::scene::region_scene_capacity_vertices() / FRAMES_IN_FLIGHT)
    }
}

/// Renderer configuration. Presentation *pacing* (frame-rate targets,
/// uncapped measurement) stays in the application — the renderer never
/// imposes an engine-wide frame-rate ceiling, and rendering cadence is
/// deliberately independent of the future fixed-rate campaign simulation
/// (Stage 3, gated).
///
/// Since P5 §1.3 (A11/R1) the three mapped arenas (instance, pose,
/// region) are demand-sized under the three budget fields below; the
/// fields' DEFAULTS are today's constants, so a config that names none
/// of them behaves at the cap exactly as the pre-P5 renderer did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RenderConfig {
    pub present_mode: PresentModePreference,
    pub validation: ValidationPreference,
    /// Verification scenario (`--replace-pipelines`): force every
    /// swapchain recreation to replace the pipeline state, exercising
    /// the surface-format-change lifecycle — including the dynamic
    /// region-scene buffer carry-over (Stage 4 final-review finding A)
    /// — without a real format change, which resizing alone cannot
    /// produce (the CI resize smoke keeps the surface format). Never
    /// set by normal operation.
    pub replace_pipelines: bool,
    /// The static Stage-2 demonstration scene draws only on explicit
    /// opt-in (Cycle 3, P3) — see [`DemoScenePreference`]. The
    /// default omits it: a game's scene is the dynamic geometry the
    /// application supplies, never content the renderer imposes.
    pub demo_scene: DemoScenePreference,
    /// Verification scenario for depth format fallback testing
    /// (corrective finding 4): override the negotiated depth format.
    /// Never set by normal operation — use `Auto` to allow the
    /// renderer to pick the device's optimal format. `ForceD24S8`
    /// and `ForceD16` select the named format ONLY if the device's
    /// optimal tiling supports DEPTH_STENCIL_ATTACHMENT (else the
    /// negotiated pick is used, never creating an invalid device
    /// state). The chosen depth format is reported in the renderer's
    /// stats.
    pub depth_format: DepthFormatPreference,
    /// P5 §1.3 (A11/R1): the instance arena's hard cap (see
    /// [`InstanceCapacity`]). Default = MAX_INSTANCES.
    pub instance_capacity: InstanceCapacity,
    /// P5 §1.3 (A11/R1): the pose arena's hard cap (see
    /// [`PoseCapacity`]). Default = MAX_POSE_BLOCKS.
    pub pose_capacity: PoseCapacity,
    /// P5 §1.3 (A11/R1): the region arena's per-slot vertex budget
    /// (see [`RegionCapacity`]). Default = the closed form's
    /// per-slot share, 36,800.
    pub region_capacity: RegionCapacity,
}

/// Frames simultaneously in flight (sync objects per slot). Fixed at 2 for
/// Stage 2; the value is a classic double-buffer CPU/GPU overlap.
pub const FRAMES_IN_FLIGHT: u32 = 2;

/// Resolves the depth format preference against the device's optimal tiling
/// features. `Auto` ALWAYS returns the negotiated pick (the default path is
/// byte-identical to the pre-knob renderer); a forced preference returns its
/// format only when the device's optimal tiling supports
/// DEPTH_STENCIL_ATTACHMENT, else the negotiated pick — never an invalid
/// device state. Pure function — unit-testable without a GPU.
/// (corrective finding 4)
pub(crate) fn resolve_depth_format(
    preference: DepthFormatPreference,
    supports_d24_s8: bool,
    supports_d16: bool,
    negotiated: vk::Format,
) -> vk::Format {
    use vk::Format as F;
    match preference {
        DepthFormatPreference::Auto => negotiated,
        DepthFormatPreference::ForceD24S8 if supports_d24_s8 => F::D24_UNORM_S8_UINT,
        DepthFormatPreference::ForceD16 if supports_d16 => F::D16_UNORM,
        // The forced format is unsupported on this device — the
        // negotiated pick, never an invalid device state.
        DepthFormatPreference::ForceD24S8 | DepthFormatPreference::ForceD16 => negotiated,
    }
}

/// Resolves the present mode against the device's supported set. `FIFO`
/// is guaranteed by the Vulkan spec, so every fallback lands on a valid
/// mode. Pure function — unit-testable without a GPU.
pub(crate) fn resolve_present_mode(
    preference: PresentModePreference,
    supported: &[vk::PresentModeKHR],
) -> vk::PresentModeKHR {
    use vk::PresentModeKHR as Mode;
    let has = |mode: Mode| supported.contains(&mode);
    match preference {
        PresentModePreference::Auto | PresentModePreference::Fifo => Mode::FIFO,
        PresentModePreference::Mailbox => {
            if has(Mode::MAILBOX) {
                Mode::MAILBOX
            } else {
                Mode::FIFO
            }
        }
        PresentModePreference::Immediate => {
            if has(Mode::IMMEDIATE) {
                Mode::IMMEDIATE
            } else {
                Mode::FIFO
            }
        }
    }
}

pub(crate) fn present_mode_str(mode: vk::PresentModeKHR) -> &'static str {
    use vk::PresentModeKHR as Mode;
    match mode {
        Mode::FIFO => "fifo",
        Mode::FIFO_RELAXED => "fifo-relaxed",
        Mode::MAILBOX => "mailbox",
        Mode::IMMEDIATE => "immediate",
        _ => "other",
    }
}

/// The single capacity decision for one dynamic-geometry upload+draw
/// pair (F-03 Step 1, fail-visible over fail-silent): what the frame
/// supplied, the frame slot's vertex capacity, the vertex count
/// actually copied and drawn, and the count omitted.
///
/// Computed ONCE per frame per buffer at the record step — both the
/// upload byte bound (`drawn` × vertex size) and the draw count consume
/// `drawn`, so one physical truncation is counted exactly once (never
/// once per historical clamp site) and the copy can never disagree
/// with the draw: no out-of-bounds copy, no inflated draw count. The
/// omitted count is reported upstream, never fatal — the
/// capacity/refusal policy is Step 2, separately authorized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct DynamicClamp {
    /// Vertices the caller supplied this frame.
    pub supplied: u32,
    /// The frame slot's vertex capacity.
    pub capacity: u32,
    /// Vertices actually copied and drawn (`<= capacity` and
    /// `<= supplied`).
    pub drawn: u32,
    /// Vertices omitted (`supplied − drawn`; 0 in capacity). Raw
    /// vertex counts — an overflow can cut a 36-vertex proxy box
    /// mid-primitive, and the count says so.
    pub truncated: u32,
}

/// The decision is pure and central: `supplied` arrives as the
/// caller's slice length and converts saturating, so a supply beyond
/// `u32::MAX` (unreachable at 24 B/vertex) degrades instead of
/// wrapping — the bound is structural rather than memory luck.
pub(crate) fn dynamic_geometry_clamp(supplied: usize, capacity: u32) -> DynamicClamp {
    let supplied = supplied.min(u32::MAX as usize) as u32;
    DynamicClamp {
        supplied,
        capacity,
        drawn: supplied.min(capacity),
        truncated: supplied.saturating_sub(capacity),
    }
}

/// The demand quantum's floor (P5 §1.3's `[ASSUMPTION]`, documented
/// here per the design): the smallest carve any of the three mapped
/// arenas starts at. 64 keeps a first frame's tiny population from
/// producing a micro-carve that would re-grow on nearly every early
/// frame (1 → 2 → 4 → … would be six events before the population
/// merely reaches the floor), while bounding the growth-event count
/// to `log2(cap / 64)` — at most 6 for the default 4,096-instance
/// caps, exactly the design's "≤ 6 events 64→4096" amortization.
pub(crate) const ARENA_FLOOR: u32 = 64;

/// The demand-quantum arithmetic every P5 §1.3 arena shares — the
/// overlay slot's proven `capacity_for` shape
/// (`vulkan/frame.rs`'s `OverlaySlot::capacity_for`) generalized to
/// three carves and given the 64 floor above:
///
/// - `required` is clamped to the configured `cap` FIRST (a supply
///   above the cap never grows past it — the cap, not the carve, is
///   the refusal/truncation boundary);
/// - a `required` at or below the current carve keeps it (steady
///   state: no event, no allocation);
/// - otherwise the carve is the next power of two at or above
///   `max(required, ARENA_FLOOR)`, clamped to the cap (so a demand
///   at or above the cap lands exactly ON the cap — the pre-P5
///   full carve, byte-identical behavior for over-cap workloads).
///
/// Pure — unit-testable without a GPU; the quantum table and the
/// §1.5 scaling predictions are pinned in this module's tests.
pub(crate) fn arena_capacity_for(current: u32, required: u32, cap: u32) -> u32 {
    let required = required.min(cap);
    if required <= current {
        return current;
    }
    required.max(ARENA_FLOOR).next_power_of_two().min(cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supported() -> [vk::PresentModeKHR; 1] {
        [vk::PresentModeKHR::FIFO]
    }

    fn full() -> [vk::PresentModeKHR; 4] {
        [
            vk::PresentModeKHR::FIFO,
            vk::PresentModeKHR::FIFO_RELAXED,
            vk::PresentModeKHR::MAILBOX,
            vk::PresentModeKHR::IMMEDIATE,
        ]
    }

    #[test]
    fn fifo_is_always_selected_for_auto() {
        assert_eq!(
            resolve_present_mode(PresentModePreference::Auto, &full()),
            vk::PresentModeKHR::FIFO
        );
    }

    #[test]
    fn mailbox_prefers_mailbox_when_supported() {
        assert_eq!(
            resolve_present_mode(PresentModePreference::Mailbox, &full()),
            vk::PresentModeKHR::MAILBOX
        );
    }

    #[test]
    fn mailbox_falls_back_to_fifo_when_unsupported() {
        assert_eq!(
            resolve_present_mode(PresentModePreference::Mailbox, &supported()),
            vk::PresentModeKHR::FIFO
        );
    }

    #[test]
    fn immediate_prefers_immediate_when_supported() {
        assert_eq!(
            resolve_present_mode(PresentModePreference::Immediate, &full()),
            vk::PresentModeKHR::IMMEDIATE
        );
    }

    #[test]
    fn immediate_falls_back_to_fifo_when_unsupported() {
        assert_eq!(
            resolve_present_mode(PresentModePreference::Immediate, &supported()),
            vk::PresentModeKHR::FIFO
        );
    }

    #[test]
    fn preference_parsing_round_trips() {
        assert_eq!(
            PresentModePreference::parse("mailbox"),
            Some(PresentModePreference::Mailbox)
        );
        assert_eq!(
            PresentModePreference::parse("AUTO"),
            Some(PresentModePreference::Auto)
        );
        assert_eq!(PresentModePreference::parse("nope"), None);
        assert_eq!(
            ValidationPreference::parse("off"),
            Some(ValidationPreference::Off)
        );
    }

    #[test]
    fn the_default_configuration_omits_the_demo_scene() {
        // The P3 owner-observable contract, pinned at the type level:
        // a renderer constructed with no explicit preference must not
        // opt the demonstration scene in — flipping this default back
        // to Included would re-impose demo content on every game
        // scene (the pixel-level proof is the render_scene_separation
        // smoke; this pin fails the build first).
        assert_eq!(
            RenderConfig::default().demo_scene,
            DemoScenePreference::Omitted
        );
        assert_eq!(DemoScenePreference::default(), DemoScenePreference::Omitted);
    }

    #[test]
    fn clamp_decision_in_capacity_supplies_every_vertex() {
        // In-capacity geometry is copied and drawn whole — the decision
        // reproduces today's min() values exactly.
        for (supplied, capacity) in [
            (0_usize, 36_800_u32),
            (1, 36_800),
            (36_288, 36_800),
            (299_999, 300_000),
        ] {
            let clamp = dynamic_geometry_clamp(supplied, capacity);
            assert_eq!(clamp.drawn, supplied as u32, "everything fits");
            assert_eq!(clamp.truncated, 0, "nothing omitted");
            assert_eq!(clamp.capacity, capacity);
        }
    }

    #[test]
    fn clamp_decision_at_capacity_boundary_omits_nothing() {
        let clamp = dynamic_geometry_clamp(36_800, 36_800);
        assert_eq!(clamp.drawn, 36_800);
        assert_eq!(clamp.truncated, 0);
    }

    #[test]
    fn clamp_decision_oversize_omits_exact_delta() {
        // The capacity-decision logic with oversized synthetic input —
        // exact deltas, including the smoke's mod-36 derivation
        // (push_box emits 36 verts/call; the region capacity 36,800 is
        // ≡ 8 mod 36, so a 1,050-box supply of 37,800 omits exactly
        // 1,000) and the overlay case (2,000 'A' glyphs × 168 verts =
        // 336,000 vs 300,000 → 36,000).
        for (supplied, capacity, omitted) in [
            (36_801_usize, 36_800_u32, 1_u32),
            (36_836, 36_800, 36),
            (37_800, 36_800, 1_000),
            (336_000, 300_000, 36_000),
        ] {
            let clamp = dynamic_geometry_clamp(supplied, capacity);
            assert_eq!(clamp.drawn, capacity, "the draw never exceeds capacity");
            assert_eq!(clamp.truncated, omitted, "the exact omitted delta");
            assert_eq!(
                clamp.supplied,
                clamp
                    .drawn
                    .checked_add(clamp.truncated)
                    .expect("drawn + omitted == supplied by construction"),
                "supplied is exactly drawn + omitted"
            );
        }
        // A supply beyond u32::MAX (unreachable at 24 B/vertex)
        // saturates instead of wrapping.
        let huge = dynamic_geometry_clamp(usize::MAX, 36_800);
        assert_eq!(huge.supplied, u32::MAX);
        assert_eq!(huge.drawn, 36_800);
    }

    #[test]
    fn clamp_decision_byte_bound_matches_vertex_clamp() {
        // The upload/draw agreement — no out-of-bounds copy, no
        // inflated draw — asserted per bound for BOTH dynamic vertex
        // types: the copy length is drawn × vertex size, which never
        // exceeds the slot's byte capacity nor the supplied bytes.
        use crate::overlay::OverlayVertex;
        use crate::scene::SceneVertex;

        assert_eq!(std::mem::size_of::<SceneVertex>(), 24);
        assert_eq!(std::mem::size_of::<OverlayVertex>(), 32);
        assert_eq!(size_of::<crate::overlay::SolidVertex>(), 24);
        for (supplied, capacity) in [
            (36_288_usize, 36_800_u32),
            (37_800, 36_800),
            (336_000, 300_000),
            (700_000, 300_000),
        ] {
            let clamp = dynamic_geometry_clamp(supplied, capacity);
            assert!(
                clamp.drawn <= clamp.capacity,
                "draw never exceeds the per-slot capacity"
            );
            for stride in [size_of::<SceneVertex>(), size_of::<OverlayVertex>()] {
                let slot_bytes = capacity as usize * stride;
                assert!(
                    clamp.drawn as usize * stride <= slot_bytes,
                    "copy bytes fit the slot"
                );
                assert!(
                    clamp.drawn as usize * stride <= supplied * stride,
                    "copy bytes never exceed the supplied bytes"
                );
            }
        }
    }

    #[test]
    fn clamp_decision_degenerate_capacities_refuse_whole() {
        // Zero supply draws nothing; a zero capacity (an unallocated
        // buffer) would omit everything — defensive only: the record
        // path never reaches a non-empty supply with capacity 0 (the
        // lazy region buffer allocates first).
        let empty = dynamic_geometry_clamp(0, 36_800);
        assert_eq!((empty.drawn, empty.truncated), (0, 0));
        let unallocated = dynamic_geometry_clamp(1_000, 0);
        assert_eq!((unallocated.drawn, unallocated.truncated), (0, 1_000));
    }

    /// P5 §1.3 (A11/R1): the budget defaults ARE today's constants —
    /// the compatibility contract at the cap. A config that names no
    /// budget field behaves exactly as the pre-P5 renderer at every
    /// boundary (typed refusals, F-03 truncation, the full carve for
    /// over-cap supplies).
    #[test]
    fn arena_budget_defaults_are_todays_constants() {
        use crate::animation::MAX_POSE_BLOCKS;
        use crate::proxy_instances::MAX_INSTANCES;
        use crate::scene::region_scene_capacity_vertices;

        assert_eq!(InstanceCapacity::default().0, MAX_INSTANCES);
        assert_eq!(PoseCapacity::default().0, MAX_POSE_BLOCKS);
        assert_eq!(MAX_POSE_BLOCKS, MAX_INSTANCES, "the pose mirror holds");
        assert_eq!(
            RegionCapacity::default().0,
            region_scene_capacity_vertices() / FRAMES_IN_FLIGHT
        );
        assert_eq!(RegionCapacity::default().0, 36_800);
        // The whole-config default carries them (the derive chains
        // through each field's own Default).
        let config = RenderConfig::default();
        assert_eq!(config.instance_capacity, InstanceCapacity(MAX_INSTANCES));
        assert_eq!(config.pose_capacity, PoseCapacity(MAX_POSE_BLOCKS));
        assert_eq!(config.region_capacity, RegionCapacity(36_800));
    }

    /// P5 §1.3: the demand quantum — the overlay's capacity_for
    /// shape with the 64 floor, pinned across the whole decision
    /// table (steady state, first demand, doubling growth, the cap
    /// clamp, and the at-cap landing that keeps over-cap workloads
    /// byte-identical with the pre-P5 full carve).
    #[test]
    fn arena_quantum_tables() {
        // Steady state: a demand at or below the current carve keeps
        // it — no event, no allocation.
        for (current, required) in [
            (0_u32, 0_u32),
            (64, 1),
            (64, 64),
            (128, 100),
            (4_096, 4_096),
        ] {
            assert_eq!(
                arena_capacity_for(current, required, 4_096),
                current,
                "steady state at ({current}, {required})"
            );
        }
        // First demand: the floor, then powers of two.
        for (required, carved) in [
            (1_u32, 64_u32),
            (63, 64),
            (64, 64),
            (65, 128),
            (100, 128),
            (128, 128),
            (129, 256),
            (1_000, 1_024),
            (2_049, 4_096),
        ] {
            assert_eq!(
                arena_capacity_for(0, required, 4_096),
                carved,
                "first demand {required}"
            );
        }
        // Doubling growth from an existing carve.
        for (current, required, grown) in [
            (64_u32, 65_u32, 128_u32),
            (128, 300, 512),
            (512, 513, 1_024),
            (64, 4_096, 4_096),
        ] {
            assert_eq!(
                arena_capacity_for(current, required, 4_096),
                grown,
                "growth from {current} under {required}"
            );
        }
        // The cap clamps: a supply at or above the cap lands exactly
        // ON the cap (never past it), matching the pre-P5 carve —
        // the render_overflow shape (37,800 demanded of the 36,800
        // default budget still carves exactly 36,800).
        assert_eq!(arena_capacity_for(0, 4_096, 4_096), 4_096);
        assert_eq!(arena_capacity_for(0, 4_097, 4_096), 4_096);
        assert_eq!(arena_capacity_for(128, 100_000, 4_096), 4_096);
        assert_eq!(arena_capacity_for(0, 37_800, 36_800), 36_800);
        // A configured cap below the floor still clamps (a budget of
        // 8 carves 8 — the configured boundary is always honored),
        // and a degenerate ZERO cap carves nothing here (the device
        // paths' creation floor of 1 keeps a valid buffer while the
        // CAP refuses/truncates everything — see the ensure sites).
        // A live carve never SHRINKS when the cap drops below it
        // (config is construction-time; the steady-state arm holds).
        assert_eq!(arena_capacity_for(0, 100, 8), 8);
        assert_eq!(arena_capacity_for(0, 100, 0), 0);
        assert_eq!(arena_capacity_for(128, 100, 0), 128);
    }

    /// P5 §1.5's scaling row, computed (the carve is a pure function
    /// of demand and cap — the predictions the device batteries and
    /// the census assert against): resident mapped bytes per carve
    /// at the census populations, before (the constant carves) vs
    /// after (the demand-sized carves). The pose row is the design's
    /// named asymmetry — 5,767,168 B carved against 16,896 B used at
    /// the corner (0.29%) — and the after-row is the ≤ 2× used-bytes
    /// bound the design demands (rounded to the power-of-two
    /// quantum).
    #[test]
    fn arena_scaling_predictions_the_95_census_row() {
        use crate::animation::{MAX_POSE_BLOCKS, POSE_STRIDE};
        use crate::proxy_instances::{INSTANCE_STRIDE, MAX_INSTANCES};
        use crate::scene::region_scene_capacity_vertices;

        let instance_cap = MAX_INSTANCES as u32;
        let pose_cap = MAX_POSE_BLOCKS as u32;
        // (label, instances supplied, pose blocks supplied)
        let populations = [
            ("garage", 7_u32, 6_u32),
            ("corner", 26, 24),
            ("1k", 1_002, 1_000),
            ("4k", 4_096, 4_096),
        ];
        let before_instance = MAX_INSTANCES as u64 * INSTANCE_STRIDE as u64 * 2;
        let before_pose = MAX_POSE_BLOCKS as u64 * POSE_STRIDE as u64 * 2;
        for (label, instances, poses) in populations {
            let carved_instances = arena_capacity_for(0, instances, instance_cap);
            let carved_poses = arena_capacity_for(0, poses, pose_cap);
            let after_instance =
                carved_instances as u64 * INSTANCE_STRIDE as u64 * FRAMES_IN_FLIGHT as u64;
            let after_pose = carved_poses as u64 * POSE_STRIDE as u64 * FRAMES_IN_FLIGHT as u64;
            // The design's bound: the after carve is within a 2×
            // factor of the used bytes rounded to the power-of-two
            // quantum — with the quantum's 64 FLOOR as the floor of
            // the bound (the tiny demos' carves ARE the floor: the
            // garage's 7-instance population carves 64, the design's
            // documented demand quantum) — whenever the demand is
            // under the cap.
            if instances < instance_cap {
                assert!(
                    after_instance
                        <= u64::from(ARENA_FLOOR.max(instances.saturating_mul(2)))
                            * INSTANCE_STRIDE as u64
                            * FRAMES_IN_FLIGHT as u64,
                    "{label}: instance carve {after_instance} B vs used {} B",
                    u64::from(instances) * INSTANCE_STRIDE as u64 * FRAMES_IN_FLIGHT as u64
                );
            }
            if poses < pose_cap {
                assert!(
                    after_pose
                        <= u64::from(ARENA_FLOOR.max(poses.saturating_mul(2)))
                            * POSE_STRIDE as u64
                            * FRAMES_IN_FLIGHT as u64,
                    "{label}: pose carve {after_pose} B vs used {} B",
                    u64::from(poses) * POSE_STRIDE as u64 * FRAMES_IN_FLIGHT as u64
                );
            }
            println!(
                "{label}: instance {before_instance} -> {after_instance} B \
                 (carve {carved_instances}, supplied {instances}); \
                 pose {before_pose} -> {after_pose} B \
                 (carve {carved_poses}, supplied {poses})"
            );
        }
        // The named headline rows, exact: the corner's pose carve and
        // the at-cap 4k rows (the 4k population lands ON the caps —
        // the before and after carves coincide, which is why every
        // at-cap battery's pins hold unchanged).
        let corner = populations[1];
        assert_eq!(corner.1, 26);
        assert_eq!(
            arena_capacity_for(0, corner.2, pose_cap),
            64,
            "the corner's 24 blocks carve the floor"
        );
        assert_eq!(64 * POSE_STRIDE * 2, 90_112, "the corner's after carve, B");
        assert_eq!(
            arena_capacity_for(0, 4_096, instance_cap) as u64 * INSTANCE_STRIDE as u64 * 2,
            before_instance,
            "the 4k instance carve is the full carve (at cap)"
        );
        assert_eq!(
            arena_capacity_for(0, 4_096, pose_cap) as u64 * POSE_STRIDE as u64 * 2,
            before_pose,
            "the 4k pose carve is the full carve (at cap)"
        );
        // The region's default budget derivation stays the closed
        // form's per-slot share (the A05-coupling closure's pin).
        assert_eq!(region_scene_capacity_vertices(), 36_800 * FRAMES_IN_FLIGHT);
    }

    #[test]
    fn depth_format_preference_defaults_to_auto() {
        assert_eq!(
            DepthFormatPreference::default(),
            DepthFormatPreference::Auto
        );
    }

    #[test]
    fn auto_depth_format_is_always_the_negotiated_pick() {
        use ash::vk;
        // The finding-4 contract's sharpest edge: Auto must never
        // drift off the negotiated pick, even when the device also
        // supports the forced formats (a resolver that returned
        // D24_S8 for Auto would change the DEFAULT for every
        // existing renderer on a D24-capable device).
        assert_eq!(
            resolve_depth_format(
                DepthFormatPreference::Auto,
                true,
                true,
                vk::Format::D32_SFLOAT
            ),
            vk::Format::D32_SFLOAT
        );
        assert_eq!(
            resolve_depth_format(
                DepthFormatPreference::Auto,
                false,
                false,
                vk::Format::D16_UNORM
            ),
            vk::Format::D16_UNORM
        );
    }

    #[test]
    fn forced_depth_format_honors_support_and_falls_back_cleanly() {
        use ash::vk;
        let negotiated = vk::Format::D32_SFLOAT;
        assert_eq!(
            resolve_depth_format(DepthFormatPreference::ForceD24S8, true, true, negotiated),
            vk::Format::D24_UNORM_S8_UINT
        );
        assert_eq!(
            resolve_depth_format(DepthFormatPreference::ForceD16, true, true, negotiated),
            vk::Format::D16_UNORM
        );
        // Unsupported on the device: the negotiated pick, never an
        // invalid device state.
        assert_eq!(
            resolve_depth_format(DepthFormatPreference::ForceD24S8, false, true, negotiated),
            negotiated
        );
        assert_eq!(
            resolve_depth_format(DepthFormatPreference::ForceD16, true, false, negotiated),
            negotiated
        );
    }
}
