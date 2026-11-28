//! §9.1/§9.3's COMPUTED-BYTES census (design §0.1's carve arithmetic
//! and §6.1's target arithmetic) — pure functions over the PUBLIC
//! engine-renderer constants, no device. Per the design's §12: the
//! arithmetic is the PREDICTION the device counters check; the
//! counters (the `tests/census_vram.rs` battery's AllocationStats
//! tuple) are the authoritative numbers.
//!
//! The §0.1 closed forms (all constants public in engine-renderer):
//!
//! - instance carve: `MAX_INSTANCES × INSTANCE_STRIDE ×
//!   FRAMES_IN_FLIGHT` = 4096 × 96 × 2 = 786,432 B mapped — one
//!   disjoint region per frame slot holding a full bounded
//!   population (proxy_draw.rs's carve).
//! - pose carve: `MAX_POSE_BLOCKS × POSE_STRIDE × FRAMES_IN_FLIGHT`
//!   = 4096 × 704 × 2 = 5,767,168 B mapped HOST_VISIBLE (the
//!   retained SkinnedRetained's SLOT_STRIDE per slot).
//! - region carve: `region_scene_capacity_vertices() ×
//!   size_of::<SceneVertex>()` = 73,600 × 24 = 1,766,400 B mapped
//!   (the capacity function already multiplies by
//!   FRAMES_IN_FLIGHT; SceneVertex is 24 B).
//! - TOTAL: 8,320,000 B — the design's §0.1 headline, CONSTANT
//!   regardless of the scene's actual population (garage 6 or
//!   corner 24 or 4k — the residency finding R1's demand-sized
//!   arenas address; the BEFORE side of §9.3's row).
//!
//! The §6.1 target arithmetic (per extent, RGBA16F = 8 B/px,
//! depth = the negotiated format's bytes/px, FRAMES_IN_FLIGHT = 2
//! slots, bloom mips at the 1/4 extent):
//!
//! - reflection: 2 × (px × 8 + px × depth_bpp) — full-extent
//!   RGBA16F color + depth per slot.
//! - post: 2 × (px × 8 + px × depth_bpp + 2 × (px/16 × 8)) — HDR
//!   color + depth + the two bloom mips per slot.
//! - shadow: `SHADOW_MAP_EXTENT² × depth_bpp` — the single
//!   1024² cascade (format prediction: the renderer's negotiated
//!   depth format; the census counters check it).

// A06 E-S2: POSE_STRIDE moved to engine-animation at E-S1 (it folds
// the digest-pinned pose bytes); MAX_POSE_BLOCKS is the renderer's
// capacity mirror and stays on the renderer's animation module.
use engine_animation::POSE_STRIDE;
use engine_renderer::animation::MAX_POSE_BLOCKS;
use engine_renderer::config::FRAMES_IN_FLIGHT;
use engine_renderer::proxy_instances::{INSTANCE_STRIDE, MAX_INSTANCES};
use engine_renderer::scene::{SceneVertex, region_scene_capacity_vertices};
use engine_renderer::shadows::SHADOW_MAP_EXTENT;

/// The census extents (§9.1's axis): 720p, 1080p, 1440p, 4K —
/// 16:9, each 2.25× the previous in pixels except 1080p→1440p
/// (1.78×); 4K is exactly 9× 720p (the §6.1 linear-scaling check).
pub const CENSUS_EXTENTS: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)];

/// The §0.1 mapped-carve census (all three dynamic carves, computed
/// from the public constants — the §9.3 BEFORE row: the carves are
/// demand-INDEPENDENT today, which is the finding).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MappedCarves {
    /// The instance buffer's whole mapped size (both slots).
    pub instance_bytes: u64,
    /// The pose storage's whole mapped size (both slots).
    pub pose_bytes: u64,
    /// The region scene buffer's whole mapped size (both slots).
    pub region_bytes: u64,
    /// The three carves' sum — §0.1's 8,320,000 B headline.
    pub total_bytes: u64,
}

/// The §0.1 computed carves. Pure over the public constants (the
/// same numbers on every platform; integer arithmetic only).
pub fn mapped_carves() -> MappedCarves {
    let instance_bytes = (MAX_INSTANCES * INSTANCE_STRIDE) as u64 * FRAMES_IN_FLIGHT as u64;
    let pose_bytes = (MAX_POSE_BLOCKS * POSE_STRIDE) as u64 * FRAMES_IN_FLIGHT as u64;
    let region_bytes = region_scene_capacity_vertices() as u64 * size_of::<SceneVertex>() as u64;
    MappedCarves {
        instance_bytes,
        pose_bytes,
        region_bytes,
        total_bytes: instance_bytes + pose_bytes + region_bytes,
    }
}

/// The §6.1 computed target residency per extent (the PREDICTION
/// the battery's target counters check; Vulkan `requirements.size`
/// ≥ these sums by alignment — the counters are authoritative).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComputedTargets {
    /// Both slots' reflection color+depth images (the wet chain).
    pub reflection_bytes: u64,
    /// Both slots' HDR color+depth plus the four bloom mips (the
    /// post chain — two mips per slot at the 1/4 extent).
    pub post_bytes: u64,
    /// The single shadow cascade's map.
    pub shadow_bytes: u64,
    /// The three chains' sum.
    pub total_bytes: u64,
}

/// Bytes per pixel of the renderer's NEGOTIATED depth format (the
/// `RenderStats::depth_format` string): D32_SFLOAT and
/// D24_UNORM_S8_UINT are 4 B/px, D16_UNORM 2 B/px. Anything else
/// reads as 4 (fail-visible in the printed row's format column,
/// never a silent wrong prediction — the census battery prints the
/// raw format string alongside).
pub fn depth_bytes_per_pixel(depth_format: &str) -> u64 {
    match depth_format {
        "D16_UNORM" => 2,
        // D32_SFLOAT, D24_UNORM_S8_UINT (the negotiated set
        // pick_depth_format chooses among), and the documented
        // default for any future addition.
        _ => 4,
    }
}

/// The §6.1 computed target bytes at `extent` with `depth_bpp`
/// bytes per depth texel. RGBA16F color is 8 B/px, the bloom mips
/// live at (extent/4)² per §`engine_renderer::post::BLOOM_DOWNSAMPLE`
/// (4), and every family chain keeps FRAMES_IN_FLIGHT = 2 slots.
pub fn computed_targets(extent: (u32, u32), depth_bpp: u64) -> ComputedTargets {
    let px = extent.0 as u64 * extent.1 as u64;
    let slots = FRAMES_IN_FLIGHT as u64;
    // BLOOM_DOWNSAMPLE = 4 (post.rs's pinned constant; read at the
    // call so a conscious change there moves this prediction with
    // it — the computed census never silently disagrees).
    let bloom_divisor = u64::from(engine_renderer::post::BLOOM_DOWNSAMPLE);
    let bloom_px = (extent.0 / 4).max(1) as u64 * (extent.1 / 4).max(1) as u64;
    debug_assert_eq!(
        bloom_px * bloom_divisor * bloom_divisor,
        px,
        "the census extents divide evenly by the bloom downsample"
    );
    let reflection_bytes = slots * (px * 8 + px * depth_bpp);
    let post_bytes = slots * (px * 8 + px * depth_bpp + 2 * bloom_px * 8);
    let shadow_bytes = SHADOW_MAP_EXTENT as u64 * SHADOW_MAP_EXTENT as u64 * depth_bpp;
    ComputedTargets {
        reflection_bytes,
        post_bytes,
        shadow_bytes,
        total_bytes: reflection_bytes + post_bytes + shadow_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE §0.1 PIN: the three carves and their total are exactly
    /// the design's census numbers (the workload-census table row
    /// "Mapped dynamic carves (total) … 8,320,000 B"). Any constant
    /// change anywhere in this arithmetic is a conscious repin.
    #[test]
    fn mapped_carves_match_the_design_census() {
        let carves = mapped_carves();
        assert_eq!(carves.instance_bytes, 786_432, "4096 × 96 × 2");
        assert_eq!(carves.pose_bytes, 5_767_168, "4096 × 704 × 2");
        assert_eq!(
            carves.region_bytes, 1_766_400,
            "((8+1000)×36+512)×2 slots × 24 B"
        );
        assert_eq!(carves.total_bytes, 8_320_000, "the §0.1 headline");
    }

    /// The carves are DEMAND-INDEPENDENT by construction (the §9.3
    /// BEFORE row's point): the function takes no population
    /// argument — this test pins that the garage (6), corner (24),
    /// and cap (4096) populations all face the same carve.
    #[test]
    fn carves_are_constant_across_populations() {
        // The three populations the §9.3 row names: garage 6,
        // corner 24, 4k at the cap. All read the one carve.
        for _population in [6_usize, 24, 1000, 4096] {
            assert_eq!(mapped_carves().total_bytes, 8_320_000);
        }
    }

    /// THE §6.1 PIN at the gate extent: 1280×720 with 4 B/px depth
    /// reproduces the design's arithmetic (reflection 22,118,400 B;
    /// post 23,961,600 B), and the shadow map is 1024² × 4.
    #[test]
    fn targets_match_the_design_arithmetic_at_720p() {
        let targets = computed_targets((1280, 720), 4);
        assert_eq!(
            targets.reflection_bytes, 22_118_400,
            "2 × (921,600×8 + 921,600×4)"
        );
        assert_eq!(
            targets.post_bytes, 23_961_600,
            "2 × (7,372,800 + 3,686,400 + 2×460,800)"
        );
        assert_eq!(targets.shadow_bytes, 4_194_304, "1024² × 4");
        assert_eq!(targets.total_bytes, 50_274_304);
    }

    /// §6.1's linear-scaling property: every family scales linearly
    /// with extent, and 4K is exactly 9× 720p (3,840 = 3×1,280 and
    /// 2,160 = 3×720 — the design's "4K ≈ 9× the 720p numbers").
    #[test]
    fn targets_scale_linearly_with_extent() {
        let base = computed_targets((1280, 720), 4);
        let uhd = computed_targets((3840, 2160), 4);
        assert_eq!(uhd.reflection_bytes, 9 * base.reflection_bytes);
        assert_eq!(uhd.post_bytes, 9 * base.post_bytes);
        assert_eq!(uhd.shadow_bytes, base.shadow_bytes, "the cascade is fixed");
    }

    /// The negotiated-format axis: D16 halves the depth share
    /// everywhere (the E-10 fallback battery's format), and the
    /// depth-format parser reads the three negotiated names.
    #[test]
    fn depth_format_parser_and_d16_fallback() {
        assert_eq!(depth_bytes_per_pixel("D32_SFLOAT"), 4);
        assert_eq!(depth_bytes_per_pixel("D24_UNORM_S8_UINT"), 4);
        assert_eq!(depth_bytes_per_pixel("D16_UNORM"), 2);
        assert_eq!(
            depth_bytes_per_pixel("future-format"),
            4,
            "fail-visible default"
        );
        let d32 = computed_targets((1280, 720), 4);
        let d16 = computed_targets((1280, 720), 2);
        // The color halves are untouched; each depth share halves.
        assert_eq!(
            d16.post_bytes,
            d32.post_bytes - 2 * (921_600 * 2),
            "both slots' HDR depth drops 2 B/px"
        );
    }

    /// The census extents are the design's four, ascending, 16:9.
    #[test]
    fn census_extents_are_the_design_axis() {
        assert_eq!(CENSUS_EXTENTS.len(), 4);
        for (w, h) in CENSUS_EXTENTS {
            assert_eq!(w * 9, h * 16, "{w}×{h} is 16:9");
        }
        assert!(CENSUS_EXTENTS.windows(2).all(|pair| {
            pair[0].0 < pair[1].0 && pair[0].0 * pair[0].1 < pair[1].0 * pair[1].1
        }));
    }
}
