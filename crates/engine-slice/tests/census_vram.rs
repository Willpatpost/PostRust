//! THE P5 R0 §9.1 BATTERY (the VRAM census — the row that gates
//! §6/B14): per feature family {dry, +shadows, +post, +wet,
//! +wet+post, maximal composition} × extent {1280×720, 1920×1080,
//! 2560×1440, 3840×2160}: ONE fresh renderer per cell (the corner
//! scene attached, the family's frames warmed past both frame
//! slots and every lazy chain build), then the FULL
//! AllocationStats tuple, texture_memory/bytes, the NEW §9.4
//! upload counters, and the COMPUTED §0.1 carves / §6.1 target
//! prediction printed as one table. The computed columns are the
//! prediction; the counters are the authoritative numbers (the
//! design's §12 discipline — the census CHECKS the arithmetic).
//!
//! `#[ignore]`d like the binary's other lavapipe/rig gates: it
//! requires a real Vulkan device and windows. THE RIG RUN (the
//! E-8 discipline — release, serial, archived output):
//!
//! ```sh
//! cargo test -p engine-slice --test census_vram --release -- \
//!     --ignored --nocapture --test-threads=1
//! ```
//!
//! Non-ignored leg: `computed_columns_match_the_design` — the
//! table's computed columns for all 24 cells (pure arithmetic, no
//! device) so the battery's own legend is pinned wherever it runs.

use engine_slice::census::carve::{
    CENSUS_EXTENTS, ComputedTargets, MappedCarves, computed_targets, depth_bytes_per_pixel,
    mapped_carves,
};
use engine_slice::census::composition::{CENSUS_FAMILIES, corner_renderer_at, warm_family};

fn sdl_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}

/// The computed columns for every cell of the table (the §6.1
/// prediction at the negotiated 4 B/px depth — the battery prints
/// the device's actual depth format alongside its measured rows).
#[test]
fn computed_columns_match_the_design() {
    let carves: MappedCarves = mapped_carves();
    assert_eq!(carves.total_bytes, 8_320_000);
    for extent in CENSUS_EXTENTS {
        let targets: ComputedTargets = computed_targets(extent, 4);
        assert!(targets.reflection_bytes > 0 && targets.post_bytes > 0);
        // The chains are additive and the shadow map is
        // extent-independent.
        assert_eq!(
            targets.total_bytes,
            targets.reflection_bytes + targets.post_bytes + targets.shadow_bytes
        );
        assert_eq!(targets.shadow_bytes, 4_194_304);
    }
    assert_eq!(depth_bytes_per_pixel("D32_SFLOAT"), 4);
}

/// THE CENSUS (the rig leg): 6 families × 4 extents, one table.
#[test]
#[ignore = "requires a real Vulkan device and windows (the rig run: release, serial, archived per E-8)"]
fn vram_census_table() {
    let _guard = sdl_lock();
    println!(
        "P5 R0 §9.1 — the VRAM census (one row per family x extent; the tuple is AllocationStats)"
    );
    println!(
        "  computed carves (§0.1, demand-independent): instance {} / pose {} / region {} = {} B",
        mapped_carves().instance_bytes,
        mapped_carves().pose_bytes,
        mapped_carves().region_bytes,
        mapped_carves().total_bytes
    );
    println!(
        "  row: family | extent | dev-mem tex-mem tex-bytes pools sets samplers | targets \
         img/view/fb/mem (frees) | uploads submits/wait-ms/staged/tail-B | computed \
         shadow/reflection/post B | depth-format"
    );
    for family in CENSUS_FAMILIES {
        for extent in CENSUS_EXTENTS {
            let (mut app, mut renderer) =
                corner_renderer_at("P5 census vram", extent).expect("the cell constructs");
            let presented =
                warm_family(&mut app, &mut renderer, &family).expect("the family's frames present");
            assert!(presented >= 6, "warmed past both slots ({presented})");
            let stats = renderer.stats();
            assert_eq!(
                stats.validation_errors, 0,
                "family {} at {extent:?}: {:?}",
                family.name, stats.recent_validation
            );
            let a = renderer.allocations();
            let depth_bpp = depth_bytes_per_pixel(&stats.depth_format);
            let targets = computed_targets(extent, depth_bpp);
            println!(
                "  {} | {}x{} | {} {} {} {} {} {} | {} {} {} {} ({} {} {} {}) | {} \
                 {:.2} {} {} | {} {} {} | {}",
                family.name,
                extent.0,
                extent.1,
                a.device_memory,
                a.texture_memory,
                a.texture_bytes,
                a.descriptor_pools,
                a.descriptor_sets,
                a.samplers,
                a.target_images,
                a.target_image_views,
                a.target_framebuffers,
                a.target_memories,
                a.target_image_frees,
                a.target_view_frees,
                a.target_framebuffer_frees,
                a.target_memory_frees,
                a.upload_submits,
                a.upload_blocking_wait_ns as f64 / 1e6,
                a.staged_buffer_uploads,
                a.texture_staging_tail_bytes,
                if family.shadows {
                    targets.shadow_bytes
                } else {
                    0
                },
                if family.wet {
                    targets.reflection_bytes
                } else {
                    0
                },
                if family.post { targets.post_bytes } else { 0 },
                stats.depth_format
            );
            // The family's chains are observable in the target
            // counters: a family without a chain builds none of its
            // images (the attributable-delta discipline — the
            // census's own consistency check, not a pixel gate).
            if !family.wet && !family.post && !family.shadows {
                assert_eq!(a.target_images, 0, "the dry family builds no targets");
            }
            drop(renderer);
            drop(app);
        }
    }
}
