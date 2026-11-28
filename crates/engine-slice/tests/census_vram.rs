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
use engine_slice::census::repack::synthetic_population;

/// THE B14/R5 ALIASED TARGET BYTES (the after-columns): the depth
/// pair's duplicate removed — one per-slot full-extent depth pair
/// per composition that runs BOTH the wet and post families (the
/// shared image the two passes alias; a family running alone keeps
/// exactly its own pair, so its row is unchanged). Pure arithmetic
/// beside `computed_targets` — the counters check it on the rig.
fn aliased_targets(
    targets: ComputedTargets,
    family: &engine_slice::census::composition::FamilySpec,
    depth_bpp: u64,
    extent: (u32, u32),
) -> u64 {
    let duplicate_depth_pair = engine_renderer::config::FRAMES_IN_FLIGHT as u64
        * extent.0 as u64
        * extent.1 as u64
        * depth_bpp;
    let total = targets.total_bytes;
    if family.wet && family.post {
        total - duplicate_depth_pair
    } else {
        total
    }
}

fn sdl_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}

/// The computed columns for every cell of the table (the §6.1
/// prediction at the negotiated 4 B/px depth — the battery prints
/// the device's actual depth format alongside its measured rows),
/// and the B14/R5 AFTER columns (the depth-pair aliasing's
/// per-family arithmetic — the duplicate pair removed only from the
/// both-family compositions).
#[test]
fn computed_columns_match_the_design() {
    let carves: MappedCarves = mapped_carves();
    assert_eq!(carves.total_bytes, 8_320_000);
    let family_axis: Vec<_> = CENSUS_FAMILIES.to_vec();
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
        // The B14/R5 after-columns: the aliasing touches ONLY the
        // compositions running both target families — the single
        // families keep their exact before-bytes (their allocation
        // pins' arithmetic), and the both-family rows drop exactly
        // the duplicate depth pair (2 × px × 4 at D32).
        for family in &family_axis {
            let after = aliased_targets(targets, family, 4, extent);
            if family.wet && family.post {
                assert_eq!(
                    after,
                    targets.total_bytes
                        - engine_renderer::config::FRAMES_IN_FLIGHT as u64
                            * extent.0 as u64
                            * extent.1 as u64
                            * 4,
                    "the both-family row at {extent:?} drops the duplicate depth pair"
                );
            } else {
                assert_eq!(
                    after, targets.total_bytes,
                    "the single-family row at {extent:?} is unchanged by the aliasing"
                );
            }
        }
    }
    assert_eq!(depth_bytes_per_pixel("D32_SFLOAT"), 4);
}

/// THE CENSUS (the rig leg): 6 families × 4 extents, one table.
/// Since B14/R5 the table also prints the MEASURING DEVICE (the
/// gpu_name/device_type stats — the integrated-GPU run's evidence
/// column) and the AFTER column (the aliased target bytes — the
/// depth pair's duplicate removed from the both-family rows).
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
         shadow/reflection/post B | depth-format | device (type) | aliased-total B"
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
            let aliased = aliased_targets(targets, &family, depth_bpp, extent);
            println!(
                "  {} | {}x{} | {} {} {} {} {} {} | {} {} {} {} ({} {} {} {}) | {} \
                 {:.2} {} {} | {} {} {} | {} | {} ({}) | {}",
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
                stats.depth_format,
                stats.gpu_name,
                stats.device_type,
                aliased
            );
            // The family's chains are observable in the target
            // counters: a family without a chain builds none of its
            // images (the attributable-delta discipline — the
            // census's own consistency check, not a pixel gate).
            if !family.wet && !family.post && !family.shadows {
                assert_eq!(a.target_images, 0, "the dry family builds no targets");
            }
            // THE B14/R5 AFTER-SHAPE PINS (the aliasing's own
            // counter evidence): the single families keep their
            // exact before-counts (the shared pair replaces their
            // own one-for-one — the (7,2,8,2)/(11,3,9,1,0) pins'
            // arithmetic); the both-family compositions carry ONE
            // depth pair, not two — two fewer images and memories
            // than the before-table at every extent.
            let counters = (
                a.target_images,
                a.target_image_views,
                a.target_framebuffers,
                a.target_memories,
            );
            match (family.wet, family.post, family.shadows) {
                (false, false, false) => {
                    assert_eq!(counters, (0, 0, 0, 0), "the dry row {extent:?}");
                }
                (false, false, true) => {
                    assert_eq!(
                        counters,
                        (2, 2, 2, 2),
                        "the +shadows row {extent:?} — untouched by the target aliasing"
                    );
                }
                (false, true, false) => {
                    assert_eq!(
                        counters,
                        (8, 8, 9, 8),
                        "the +post row {extent:?} — unchanged by the aliasing"
                    );
                }
                (true, false, false) => {
                    assert_eq!(
                        counters,
                        (4, 4, 2, 4),
                        "the +wet row {extent:?} — unchanged by the aliasing"
                    );
                }
                (true, true, false) => {
                    assert_eq!(
                        counters,
                        (10, 12, 11, 10),
                        "the +wet+post row {extent:?} — one depth pair, not two \
                         (was (12, 12, 11, 12) before B14/R5)"
                    );
                }
                (true, true, true) => {
                    assert_eq!(
                        counters,
                        (12, 14, 13, 12),
                        "the maximal row {extent:?} — one depth pair, not two \
                         (was (14, 14, 13, 14) before B14/R5)"
                    );
                }
                // The census axis is closed (six families, the arms
                // above cover all six bit combinations it contains).
                _ => unreachable!("the census family axis is closed"),
            }
            drop(renderer);
            drop(app);
        }
    }
}

// ==== The P5 R1 (§1.3, A11/R1) arena legs — the design's §1.5
// mandated rows, homed in the capacity census (the R0 instrument
// the design named): the residency scaling rows and the growth
// gate boundary. Both rig-gated like the table above. ====

// A06 E-S2: POSE_STRIDE/rest_pose_block are engine-animation's
// (moved at E-S1); MAX_POSE_BLOCKS is the renderer's capacity
// mirror and keeps the renderer import.
use engine_animation::{POSE_STRIDE, rest_pose_block};
use engine_renderer::animation::MAX_POSE_BLOCKS;
use engine_renderer::config::FRAMES_IN_FLIGHT;
use engine_renderer::lit_scene::day_preset;
use engine_renderer::proxy_instances::{INSTANCE_STRIDE, MAX_INSTANCES, ProxyInstance};
use engine_renderer::scene::{SceneVertex, push_box};
use engine_renderer::shadows::ShadowBias;
use engine_renderer::{
    InstanceCapacity, ProxyPopulation, RegionCapacity, RenderConfig, Renderer, ValidationPreference,
};
use engine_shell::{DriverPreference, ShellApp, ShellConfig};
use engine_slice::corner_demo::{corner_submission, route_poses, view_projection};
use engine_slice::corner_scene::{attach_corner_scene, corner_bounds};

/// One fresh rig renderer with the corner scene attached (the
/// census cell's own construction, default budgets unless the caller
/// configures; validation ON — the census legs assert its counters,
/// the E-8 discipline).
fn arena_renderer(title: &str, config: RenderConfig) -> (ShellApp, Renderer) {
    let app = ShellApp::new(ShellConfig {
        title: title.into(),
        width: 1280,
        height: 720,
        target_fps: 60,
        driver: DriverPreference::Real,
    })
    .expect("the rig window constructs");
    let mut renderer = Renderer::new(
        app.window(),
        RenderConfig {
            validation: ValidationPreference::On,
            ..config
        },
    )
    .expect("the rig renderer constructs");
    attach_corner_scene(&mut renderer).expect("the corner scene attaches");
    (app, renderer)
}

/// The census camera (the attach battery's own first-waypoint
/// projection, 16:9 at the 1280×720 window).
fn census_matrix() -> engine_slice::math::Mat4 {
    view_projection(
        route_poses()
            .next()
            .expect("the route has a first waypoint"),
        16.0 / 9.0,
    )
}

/// Submits one populated corner frame at `instances`/`blocks` (the
/// census's synthetic population over rest blocks — deterministic,
/// the repack's own validation the proof).
fn submit_population(
    renderer: &mut Renderer,
    matrix: &engine_slice::math::Mat4,
    instances: &[ProxyInstance],
    blocks: &[[[[f32; 4]; 4]; engine_animation::HUMANOID_JOINTS]],
    shadows: bool,
) -> engine_renderer::FrameResult {
    let population = ProxyPopulation {
        instance_generation: 0,
        pose_generation: 0,
        npc: instances,
        npc_poses: blocks,
        npc_material: 1,
        vehicle: &[],
        vehicle_material: 2,
        rain: &[],
        rain_material: 0,
    };
    let submission = corner_submission(
        matrix,
        [0.0, 1.7, 15.0],
        day_preset(),
        shadows.then_some(ShadowBias {
            constant: 0.0,
            slope_scale: 1.0,
        }),
        corner_bounds(),
        Some(population),
        None,
        false,
        None,
        (1280, 720),
    );
    renderer
        .submit_frame(submission)
        .expect("the frame renders")
}

/// P5 §1.5's SCALING ROWS: resident mapped bytes per carve at the
/// census populations {garage, corner, 1k, 4k}, BEFORE (the
/// constant carves — 786,432 / 5,767,168 / 1,766,400 B) vs AFTER
/// (the demand-sized carves, read from the renderer's own capacity
/// stats after warmed frames). The 4k population sits AT the default
/// caps, so its after-carves coincide with the before-carves — the
/// at-cap batteries' pins hold by construction; the design's named
/// asymmetry (the corner's pose carve) lands at the 64-block floor.
#[test]
#[ignore = "requires a real Vulkan device and windows (the rig run: release, serial, archived per E-8)"]
fn arena_residency_scaling_rows() {
    let _guard = sdl_lock();
    println!(
        "P5 R1 §1.5 — the arena residency scaling rows \
         (BEFORE: the constant carves; AFTER: the demand-sized carves)"
    );
    println!(
        "  row: population | instances supplied/carved | pose blocks supplied/carved | \
         instance B before/after | pose B before/after | growth events"
    );
    let before_instance = MAX_INSTANCES as u64 * INSTANCE_STRIDE as u64 * FRAMES_IN_FLIGHT as u64;
    let before_pose = MAX_POSE_BLOCKS as u64 * POSE_STRIDE as u64 * FRAMES_IN_FLIGHT as u64;
    let matrix = census_matrix();
    // The census populations' pose indexes wrap to the block count
    // (the garage's 7th instance is the van analog — the vehicle
    // kind never reads its pose index, but the safe layer validates
    // every populated npc's index against the supply).
    let population_for = |instances: usize, blocks: usize| -> Vec<ProxyInstance> {
        let mut npc = synthetic_population(instances);
        for (i, instance) in npc.iter_mut().enumerate() {
            instance.pose_index = (i % blocks.max(1)) as u32;
        }
        npc
    };
    for (label, instances, blocks, carved) in [
        ("garage", 7_usize, 6_usize, 64_u32),
        ("corner", 26, 24, 64),
        ("1k", 1_002, 1_000, 1_024),
        ("4k", 4_096, 4_096, 4_096),
    ] {
        let (mut app, mut renderer) =
            arena_renderer("P5 R1 arena scaling", RenderConfig::default());
        let _ = app.frame();
        let npc = population_for(instances, blocks);
        let poses: Vec<_> = (0..blocks).map(|_| rest_pose_block()).collect();
        // Warm past both frame slots (the census cell discipline).
        for _ in 0..3 {
            let outcome = submit_population(&mut renderer, &matrix, &npc, &poses, false);
            assert!(outcome.presented, "{label}: the populated frame presents");
        }
        let stats = renderer.stats();
        assert_eq!(
            stats.validation_errors, 0,
            "{label}: {:?}",
            stats.recent_validation
        );
        assert_eq!(
            stats.instance_capacity, carved,
            "{label}: the instance carve is the demand quantum"
        );
        assert_eq!(
            stats.pose_capacity, carved,
            "{label}: the pose carve is the demand quantum"
        );
        assert_eq!(stats.instance_peak_used, instances as u32);
        assert_eq!(stats.pose_peak_used, blocks as u32);
        assert_eq!(
            stats.arena_growth_events, 0,
            "{label}: a fixed population's first carve is demand-sizing, never an event"
        );
        let after_instance =
            stats.instance_capacity as u64 * INSTANCE_STRIDE as u64 * FRAMES_IN_FLIGHT as u64;
        let after_pose = stats.pose_capacity as u64 * POSE_STRIDE as u64 * FRAMES_IN_FLIGHT as u64;
        println!(
            "  {label} | {instances}/{} | {blocks}/{} | {before_instance}/{} | \
             {before_pose}/{} | {}",
            stats.instance_capacity,
            stats.pose_capacity,
            after_instance,
            after_pose,
            stats.arena_growth_events
        );
        drop(renderer);
        drop(app);
    }
}

/// P5 §1.5's GATE BOUNDARY — the growth path's four legs:
/// (1) a supply exceeding the carve but under the cap grows ONCE
///     and renders identically (capture byte-equality against a
///     reference renderer that carried the population from frame 1,
///     plus the counted-event and one-memory-per-carve pins);
/// (2) a supply above the CAP refuses with the exact existing typed
///     error (the CAP, never the carve, is the boundary);
/// (3) the region F-03 counter still reports the truncation that
///     accompanied a growth event (growth is accommodation, never
///     masking — the counter is not cleared);
/// (4) the pose growth under the SHADOWED family (the heaviest
///     sync path — the per-generation binding-5 rewrite) stays
///     validation-clean across the growth.
#[test]
#[ignore = "requires a real Vulkan device and windows (the rig run: release, serial, archived per E-8)"]
fn arena_growth_gate_boundary() {
    let _guard = sdl_lock();
    let matrix = census_matrix();

    // ---- LEG 1: growth under the cap, once, identically rendered.
    {
        let (mut app, mut renderer) =
            arena_renderer("P5 R1 growth gate (grown)", RenderConfig::default());
        let _ = app.frame();
        let small = synthetic_population(100);
        let small_poses: Vec<_> = (0..100).map(|_| rest_pose_block()).collect();
        let outcome = submit_population(&mut renderer, &matrix, &small, &small_poses, false);
        assert!(outcome.presented);
        let stats = renderer.stats();
        assert_eq!((stats.instance_capacity, stats.pose_capacity), (128, 128));
        assert_eq!(
            stats.arena_growth_events, 0,
            "the first carve is not an event"
        );
        // Warm the capture path BEFORE the baseline (the a6e5adc
        // corrective's own discipline: the capture destination's
        // one-time mapped buffer + memory must not read as the
        // growth frame's cost).
        renderer
            .request_frame_capture()
            .expect("the warm capture arms");
        let outcome = submit_population(&mut renderer, &matrix, &small, &small_poses, false);
        assert!(outcome.presented);
        renderer
            .take_frame_capture()
            .expect("the warm capture completes")
            .expect("the warm capture is ready");
        let warm = renderer.allocations();
        // The growth frame: 300 exceeds the 128 carve, under the
        // 4,096 cap — each carve re-creates once (instance AND pose:
        // two events, two memories).
        let grown = synthetic_population(300);
        let grown_poses: Vec<_> = (0..300).map(|_| rest_pose_block()).collect();
        renderer.request_frame_capture().expect("the capture arms");
        let outcome = submit_population(&mut renderer, &matrix, &grown, &grown_poses, false);
        assert!(outcome.presented);
        let stats = renderer.stats();
        assert_eq!(
            (stats.instance_capacity, stats.pose_capacity),
            (512, 512),
            "the growth doubled the carve to the demand's power of two"
        );
        assert_eq!(
            stats.arena_growth_events, 2,
            "one growth event per carve (instance + pose)"
        );
        assert_eq!(
            renderer.allocations().device_memory,
            warm.device_memory + 2,
            "each growth re-creates exactly one device memory"
        );
        let grown_capture = renderer
            .take_frame_capture()
            .expect("the capture completes")
            .expect("the grown frame's capture is ready");
        // Steady state after the growth: the same population again
        // allocates nothing and records no event.
        let outcome = submit_population(&mut renderer, &matrix, &grown, &grown_poses, false);
        assert!(outcome.presented);
        assert_eq!(renderer.stats().arena_growth_events, 2);
        assert_eq!(renderer.allocations().device_memory, warm.device_memory + 2);
        // SDL allows ONE event pump per process (the census's own
        // serial-window discipline): the grown renderer's window
        // closes before the reference's opens — the captures are
        // already extracted, byte-for-byte comparable across the
        // drop.
        drop(renderer);
        drop(app);
        // The reference: the SAME 300 population from frame 1 on a
        // fresh renderer — the grown renderer's frame must be
        // byte-identical (the growth changed capacity, never content).
        let (mut app2, mut renderer2) =
            arena_renderer("P5 R1 growth gate (reference)", RenderConfig::default());
        let _ = app2.frame();
        renderer2
            .request_frame_capture()
            .expect("the reference capture arms");
        let outcome = submit_population(&mut renderer2, &matrix, &grown, &grown_poses, false);
        assert!(outcome.presented);
        let reference_capture = renderer2
            .take_frame_capture()
            .expect("the reference capture completes")
            .expect("the reference capture is ready");
        assert_eq!(
            grown_capture.bytes.len(),
            reference_capture.bytes.len(),
            "the captures agree on extent"
        );
        assert_eq!(
            grown_capture.bytes, reference_capture.bytes,
            "the growth frame renders byte-identically to the from-frame-1 reference"
        );
        let stats2 = renderer2.stats();
        assert_eq!(
            (stats2.instance_capacity, stats2.pose_capacity),
            (512, 512),
            "the reference carves the same quantum from frame 1"
        );
        assert_eq!(stats2.arena_growth_events, 0);
        drop(renderer2);
        drop(app2);
    }

    // ---- LEG 2: above the CAP — the exact existing typed refusal.
    {
        let (mut app, mut renderer) = arena_renderer(
            "P5 R1 growth gate (cap)",
            RenderConfig {
                instance_capacity: InstanceCapacity(200),
                ..RenderConfig::default()
            },
        );
        let _ = app.frame();
        let at = synthetic_population(150);
        let poses: Vec<_> = (0..150).map(|_| rest_pose_block()).collect();
        let outcome = submit_population(&mut renderer, &matrix, &at, &poses, false);
        assert!(outcome.presented, "an in-cap population renders");
        assert_eq!(renderer.stats().instance_capacity, 200);
        let over = synthetic_population(201);
        let population = ProxyPopulation {
            instance_generation: 0,
            pose_generation: 0,
            npc: &over,
            npc_poses: &poses,
            npc_material: 1,
            vehicle: &[],
            vehicle_material: 2,
            rain: &[],
            rain_material: 0,
        };
        let refused = renderer
            .submit_frame(corner_submission(
                &matrix,
                [0.0, 1.7, 15.0],
                day_preset(),
                None,
                corner_bounds(),
                Some(population),
                None,
                false,
                None,
                (1280, 720),
            ))
            .expect_err("the over-cap population refuses");
        match refused {
            engine_renderer::RendererError::ProxyPack(
                engine_renderer::proxy_instances::InstancePackError::Capacity {
                    capacity,
                    requested,
                },
            ) => {
                assert_eq!(
                    (capacity, requested),
                    (200, 201),
                    "the CAP, never the carve, is the refusal boundary"
                );
            }
            other => panic!("the over-cap population refuses typed (got {other:?})"),
        }
        // A subsequent in-cap frame still presents (no poisoned state).
        let outcome = submit_population(&mut renderer, &matrix, &at, &poses, false);
        assert!(outcome.presented);
        drop(renderer);
        drop(app);
    }

    // ---- LEG 3: the region F-03 counter survives a growth event
    // (the counter is NOT cleared — growth never masks truncation).
    {
        let (mut app, mut renderer) = arena_renderer(
            "P5 R1 growth gate (region)",
            RenderConfig {
                region_capacity: RegionCapacity(500),
                ..RenderConfig::default()
            },
        );
        let _ = app.frame();
        let small_region: Vec<SceneVertex> = (0..100)
            .map(|i| SceneVertex {
                pos: [i as f32 * 0.1, 0.0, 0.0],
                col: [0.2, 0.45, 0.85],
            })
            .collect();
        let outcome = renderer
            .render_layers(&matrix, &small_region, None, &[], None, (1280, 720))
            .expect("the frame renders");
        assert!(outcome.presented);
        let stats = renderer.stats();
        assert_eq!(
            stats.region_capacity_vertices, 128,
            "the first demand's quantum"
        );
        assert_eq!(stats.region_truncated_vertices, 0);
        // The oversize supply: 600 against the 500 budget — the
        // carve GROWS to the cap (one event) AND the clamp truncates
        // exactly 100 (fail-visible, the counter NOT cleared by the
        // growth that landed in the same frame).
        let mut oversize_region = Vec::new();
        for index in 0..17_u32 {
            push_box(
                &mut oversize_region,
                [
                    -8.0 + (index % 6) as f32 * 2.0,
                    0.0,
                    -8.0 + (index / 6) as f32 * 2.0,
                ],
                0.5,
                1.0,
                [0.85, 0.45, 0.20],
            );
        }
        assert_eq!(oversize_region.len(), 612, "17 boxes x 36 verts");
        let outcome = renderer
            .render_layers(&matrix, &oversize_region, None, &[], None, (1280, 720))
            .expect("the oversize frame renders");
        assert!(outcome.presented);
        let stats = renderer.stats();
        assert_eq!(
            stats.region_capacity_vertices, 500,
            "the carve grew to the cap"
        );
        assert_eq!(stats.region_supplied_peak_vertices, 612);
        assert_eq!(
            stats.region_truncated_vertices, 114,
            "612 supplied - 498 carved (the RegionCapacity(500) triangle floor, 500 - 500%3): \
             the truncation the growth did NOT mask — the T04 whole-triangle admission \
             (the R4 census catch: this rig-#[ignore]d leg pinned the pre-T04 byte-cut \
             arithmetic 112, the T04 re-pin pass having updated render_overflow but \
             missed this leg; CI green today because the rig battery never ran — \
             corrected before the next rig run goes red)"
        );
        assert_eq!(stats.arena_growth_events, 1, "the region growth, counted");
        assert_eq!(renderer.stats().validation_errors, 0);
        drop(renderer);
        drop(app);
    }

    // ---- LEG 4: the pose growth under the SHADOWED family (the
    // heaviest sync path — the retained buffer re-created while the
    // per-generation shadowed sets bind it).
    {
        let (mut app, mut renderer) =
            arena_renderer("P5 R1 growth gate (shadowed)", RenderConfig::default());
        let _ = app.frame();
        let small = synthetic_population(100);
        let small_poses: Vec<_> = (0..100).map(|_| rest_pose_block()).collect();
        for _ in 0..3 {
            let outcome = submit_population(&mut renderer, &matrix, &small, &small_poses, true);
            assert!(outcome.presented, "the shadowed populated frames present");
        }
        let stats = renderer.stats();
        assert_eq!(stats.validation_errors, 0, "{:?}", stats.recent_validation);
        assert_eq!(stats.pose_capacity, 128);
        // The growth under shadows: the pose carve re-creates at
        // 512, the shadowed family's binding-5 sets go stale, and
        // the NEXT shadowed frame's ladder rewrites them — the whole
        // path must stay validation-clean.
        let grown = synthetic_population(300);
        let grown_poses: Vec<_> = (0..300).map(|_| rest_pose_block()).collect();
        for _ in 0..2 {
            let outcome = submit_population(&mut renderer, &matrix, &grown, &grown_poses, true);
            assert!(outcome.presented, "the shadowed growth frames present");
        }
        let stats = renderer.stats();
        assert_eq!(
            stats.pose_capacity, 512,
            "the pose carve grew under the shadowed family"
        );
        assert_eq!(
            stats.arena_growth_events, 2,
            "the instance and pose growth events under shadows"
        );
        assert_eq!(
            stats.validation_errors, 0,
            "the stale-binding rewrite is validation-clean: {:?}",
            stats.recent_validation
        );
        drop(renderer);
        drop(app);
    }
}
