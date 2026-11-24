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
/// environment limitations.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DemoScenePreference {
    #[default]
    Omitted,
    Included,
    /// Upload the static lit intersection and its material descriptors.
    Corner,
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

/// Renderer configuration. Presentation *pacing* (frame-rate targets,
/// uncapped measurement) stays in the application — the renderer never
/// imposes an engine-wide frame-rate ceiling, and rendering cadence is
/// deliberately independent of the future fixed-rate campaign simulation
/// (Stage 3, gated).
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
