//! THE P5 §9.4 BATTERY (the attach/upload census — A20/R3's
//! before/after instrument): the staging machinery's cost at scene
//! attach, through the upload counters on
//! `engine_renderer::AllocationStats` (`upload_submits` — one per
//! POOLED batch since R3 (was: one per object); the blocking-wait
//! total (inline waits + the deferred arm's drain);
//! `staged_buffer_uploads` — the throwaway HOST_VISIBLE staging
//! pairs (0 on pooled devices since R3);
//! `texture_staging_tail_bytes` — the permanent per-texture tails
//! (RETIRED by R3: 0); plus R3's own pool columns
//! (`staging_pool_bytes` / `staging_pool_growth_events` /
//! `staging_bound_region_bytes` — the one-pool, growth, and
//! readiness observables). One fresh renderer per scene (corner,
//! garage): construction read, TIMED attach, attach read, then warm
//! dry frames proving the FRAME path never touches the staging
//! machinery (the counters freeze — the attach-only boundary).
//!
//! THE BEFORE ROW (E-12's archived record, reproduced on this
//! lane's base tree `3a419a7` the same session): corner/garage 5
//! submits (4 at attach, 1 at construction), 1 staged pair, 794,624
//! B tails, tex-bytes 1,589,248, dev-mem 10. THE AFTER ROW (R3):
//! 2 submits (1 at attach, 1 at construction), 0 pairs, 0 tails,
//! tex-bytes 794,624 (image-only — the tails were exactly 50% of
//! the texture residency), dev-mem 12 (the pool's two backing
//! allocations: the 64-KiB floor at construction and the one
//! growth to the attach batch's demand quantum).
//!
//! `#[ignore]`d (a real device + windows). THE RIG RUN:
//!
//! ```sh
//! cargo test -p engine-slice --test census_attach --release -- \
//!     --ignored --nocapture --test-threads=1
//! ```
//!
//! Extent-independent by construction (mesh/texture uploads do not
//! scale with the window) — the 1280×720 run IS the row.

use std::time::Instant;

use engine_renderer::{
    DemoScenePreference, LifecycleFaultSite, RenderConfig, Renderer, ValidationPreference,
    lit_scene::day_preset,
};
use engine_shell::{DriverPreference, ShellApp, ShellConfig};

use engine_slice::corner_demo::{corner_submission, route_poses, view_projection};
use engine_slice::corner_scene::{attach_corner_scene, corner_bounds};
use engine_slice::garage_scene::garage_scene_description;

fn sdl_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}

fn fresh_renderer(title: &str) -> (ShellApp, Renderer) {
    let app = ShellApp::new(ShellConfig {
        title: title.into(),
        width: 1280,
        height: 720,
        target_fps: 60,
        driver: DriverPreference::Real,
    })
    .unwrap();
    let renderer = Renderer::new(
        app.window(),
        RenderConfig {
            validation: ValidationPreference::On,
            demo_scene: DemoScenePreference::Omitted,
            ..Default::default()
        },
    )
    .unwrap();
    (app, renderer)
}

/// THE ATTACH CENSUS (AFTER R3): per scene — the construction split
/// (the renderer's own atlas batch), the TIMED attach, the counters
/// at attach, and the frame-path freeze proof — now pinning the
/// pooled AFTER shape (one submit per batch, no pairs, no tails,
/// one pool with the growth event, and the readiness tracking fully
/// retired at the attach's drain).
#[test]
#[ignore = "requires a real Vulkan device and windows (the rig run: release, serial, archived per E-8)"]
fn attach_upload_census() {
    let _guard = sdl_lock();
    println!("P5 §9.4 — the attach/upload census (AFTER R3: the pooled batches)");
    println!(
        "  row: scene | attach-wall-ms | submits | blocking-wait-ms | staged-pairs | \
         tail-B | tex-mem tex-bytes | dev-mem | pool-B growth bound-B"
    );

    // The corner scene (the batteries' own attach path).
    {
        let (mut app, mut renderer) = fresh_renderer("P5 census attach (corner)");
        let constructed = renderer.allocations();
        // THE CONSTRUCTION SHAPE: the atlas rode the pool's first
        // batch (ONE submit; the pool's 64-KiB floor backing exists
        // — the one pool), and nothing else touched the machinery.
        assert_eq!(constructed.upload_submits, 1, "the atlas batch: one submit");
        assert_eq!(constructed.staged_buffer_uploads, 0);
        assert_eq!(constructed.texture_staging_tail_bytes, 0);
        assert_eq!(constructed.staging_pool_bytes, 65_536, "the floor backing");
        assert_eq!(constructed.staging_pool_growth_events, 0);
        assert_eq!(
            constructed.staging_bound_region_bytes, 0,
            "the inline atlas wait retired its region"
        );
        let attach_started = Instant::now();
        attach_corner_scene(&mut renderer).unwrap();
        let attach_wall = attach_started.elapsed();
        let attached = renderer.allocations();
        let _ = app.frame();
        // THE ATTACH SHAPE (the census's AFTER row, pinned): ONE
        // deferred batch for the mesh VBO and every texture — one
        // submit, drained at the attach's end (the deferred-wait-
        // before-first-use discipline: the first frame cannot
        // precede the attach's return) — with the pool grown ONCE to
        // the batch's demand quantum, no throwaway pair, no tail,
        // and the readiness tracking fully retired (bound 0).
        assert_eq!(
            attached.upload_submits,
            constructed.upload_submits + 1,
            "the attach's whole staging phase is ONE batch (was 4 submits before R3)"
        );
        assert_eq!(
            attached.staged_buffer_uploads, 0,
            "the throwaway pairs are gone"
        );
        assert_eq!(
            attached.texture_staging_tail_bytes, 0,
            "the tails are retired"
        );
        assert_eq!(
            attached.texture_memory, 4,
            "one allocation per texture, image-only"
        );
        assert_eq!(
            attached.texture_bytes, 794_624,
            "the image-only sum (was 1,589,248 with the tails — exactly the \
             794,624-B tail total E-12 measured)"
        );
        assert_eq!(
            attached.staging_pool_growth_events, 2,
            "the attach batch grew the pool twice: once when the first texture \
             exceeded the 64-KiB floor plus the mesh, once when the third \
             exceeded the intermediate quantum (the mesh itself fits the floor)"
        );
        assert_eq!(
            attached.staging_pool_bytes, 1_048_576,
            "the corner attach's demand quantum: 1 MiB (mesh + 3 textures)"
        );
        assert_eq!(
            attached.staging_bound_region_bytes, 0,
            "the attach's drain retired the deferred batch's region \
             (the wait completed before first use)"
        );
        // Warm dry frames: the frame path must not move ANY of the
        // staging counters (the pool is attach-time machinery — the
        // boundary R3 established).
        let matrix = view_projection(
            route_poses()
                .next()
                .expect("the route has a first waypoint"),
            16.0 / 9.0,
        );
        let submission = corner_submission(
            &matrix,
            [0.0, 1.7, 15.0],
            day_preset(),
            None,
            corner_bounds(),
            None,
            None,
            false,
            None,
            (1280, 720),
        );
        for _ in 0..4 {
            assert!(renderer.submit_frame(submission).unwrap().presented);
        }
        let after_frames = renderer.allocations();
        assert_eq!(
            (
                after_frames.upload_submits,
                after_frames.staged_buffer_uploads,
                after_frames.texture_staging_tail_bytes,
                after_frames.staging_pool_bytes,
                after_frames.staging_pool_growth_events,
                after_frames.staging_bound_region_bytes
            ),
            (
                attached.upload_submits,
                attached.staged_buffer_uploads,
                attached.texture_staging_tail_bytes,
                attached.staging_pool_bytes,
                attached.staging_pool_growth_events,
                attached.staging_bound_region_bytes
            ),
            "the frame path never touches the staging machinery"
        );
        assert_eq!(renderer.stats().validation_errors, 0);
        println!(
            "  corner | {:.2} | {} ({} at attach, {} at construction) | {:.2} | {} | {} | {} {} | {} | {} {} {}",
            attach_wall.as_secs_f64() * 1e3,
            attached.upload_submits,
            attached.upload_submits - constructed.upload_submits,
            constructed.upload_submits,
            attached.upload_blocking_wait_ns as f64 / 1e6,
            attached.staged_buffer_uploads,
            attached.texture_staging_tail_bytes,
            attached.texture_memory,
            attached.texture_bytes,
            attached.device_memory,
            attached.staging_pool_bytes,
            attached.staging_pool_growth_events,
            attached.staging_bound_region_bytes
        );
    }

    // The garage scene (the second scene's own attach — the
    // small-scene generalization tax, counted).
    {
        let (mut app, mut renderer) = fresh_renderer("P5 census attach (garage)");
        let constructed = renderer.allocations();
        let attach_started = Instant::now();
        renderer.attach_scene(garage_scene_description()).unwrap();
        let attach_wall = attach_started.elapsed();
        let attached = renderer.allocations();
        let _ = app.frame();
        // The garage's attach is the same pooled shape (the generic
        // CornerResources path): one batch, no pairs, no tails.
        assert_eq!(
            attached.upload_submits,
            constructed.upload_submits + 1,
            "the garage attach is one batch too"
        );
        assert_eq!(attached.staged_buffer_uploads, 0);
        assert_eq!(attached.texture_staging_tail_bytes, 0);
        assert_eq!(attached.texture_bytes, 794_624);
        assert_eq!(attached.staging_bound_region_bytes, 0);
        assert_eq!(renderer.stats().validation_errors, 0);
        println!(
            "  garage | {:.2} | {} ({} at attach, {} at construction) | {:.2} | {} | {} | {} {} | {} | {} {} {}",
            attach_wall.as_secs_f64() * 1e3,
            attached.upload_submits,
            attached.upload_submits - constructed.upload_submits,
            constructed.upload_submits,
            attached.upload_blocking_wait_ns as f64 / 1e6,
            attached.staged_buffer_uploads,
            attached.texture_staging_tail_bytes,
            attached.texture_memory,
            attached.texture_bytes,
            attached.device_memory,
            attached.staging_pool_bytes,
            attached.staging_pool_growth_events,
            attached.staging_bound_region_bytes
        );
    }
}

/// THE §4.5 GATE BOUNDARY (the design's mandatory in-flight safety
/// test): a fault-injected NEVER-SIGNALING fence must never have its
/// region reset. The fault
/// ([`LifecycleFaultSite::StagingFenceNeverSignals`]) makes the
/// attach's deferred batch submit WITHOUT its fence — a structurally
/// real never-signaling fence — so the attach's drain times out
/// fail-visibly (a stuck upload surfaces as an error, never an
/// unresponsive window) and the readiness tracking keeps the
/// region BOUND (the RED witness: a naive retire-anything-signaled
/// implementation — or a retire-on-flush one — reads bound 0 here
/// and lets the bump allocator reuse bytes a pending submission
/// reads; this leg is written to catch exactly that bug shape).
/// The clean twin leg then proves the ordinary lifecycle: the
/// deferred batch drains at the attach's end (bound 0, the wait
/// before first use) and the following frames stay validation-clean
/// with sync validation ON.
#[test]
#[ignore = "requires a real Vulkan device and windows (the rig run: release, serial, archived per E-8)"]
fn in_flight_safety_never_signaling_fence() {
    let _guard = sdl_lock();

    // ---- THE FAULTED LEG: the stuck fence keeps its region bound.
    {
        let (app, mut renderer) = fresh_renderer("P5 R3 in-flight safety (faulted)");
        renderer.inject_lifecycle_fault(LifecycleFaultSite::StagingFenceNeverSignals, 1);
        let refused = attach_corner_scene(&mut renderer)
            .expect_err("the never-signaling fence fails the attach's drain visibly");
        match &refused {
            engine_renderer::RendererError::Vulkan(message) => {
                assert!(
                    message.contains("staging drain"),
                    "the typed drain timeout (got: {message})"
                );
            }
            other => panic!("expected the drain's timeout error, got {other:?}"),
        }
        let stuck = renderer.allocations();
        assert_eq!(
            stuck.upload_submits, 2,
            "the batch WAS submitted (the census counts it)"
        );
        assert!(
            stuck.staging_bound_region_bytes > 0,
            "THE RED WITNESS: the never-signaled region is still bound \
             (bound {} — a naive reset would read 0 and reuse the bytes)",
            stuck.staging_bound_region_bytes
        );
        assert_eq!(
            renderer.stats().validation_errors,
            0,
            "the stuck submission itself is validation-clean: {:?}",
            renderer.stats().recent_validation
        );
        // The teardown protocol: dropping the renderer idles the
        // device (the manifest's destroy order) and releases the
        // orphaned fence — no hang, no leak.
        drop(renderer);
        drop(app);
    }

    // ---- THE CLEAN TWIN: the deferred batch drains before first use.
    {
        let (app, mut renderer) = fresh_renderer("P5 R3 in-flight safety (clean)");
        attach_corner_scene(&mut renderer).expect("the clean attach drains its batch");
        let drained = renderer.allocations();
        assert_eq!(
            drained.staging_bound_region_bytes, 0,
            "the deferred batch's region retired at the attach's drain"
        );
        assert_eq!(drained.upload_submits, 2);
        // First use immediately after the drained attach: the frames
        // draw the uploaded content with sync validation ON.
        let matrix = view_projection(
            route_poses()
                .next()
                .expect("the route has a first waypoint"),
            16.0 / 9.0,
        );
        let submission = corner_submission(
            &matrix,
            [0.0, 1.7, 15.0],
            day_preset(),
            None,
            corner_bounds(),
            None,
            None,
            false,
            None,
            (1280, 720),
        );
        for _ in 0..4 {
            assert!(renderer.submit_frame(submission).unwrap().presented);
        }
        let stats = renderer.stats();
        assert_eq!(
            (stats.validation_errors, stats.validation_warnings),
            (0, 0),
            "the deferred wait completed before first use: {:?}",
            stats.recent_validation
        );
        assert_eq!(
            renderer.allocations().staging_bound_region_bytes,
            0,
            "the frames never re-bind staging"
        );
        drop(renderer);
        drop(app);
        println!(
            "in-flight safety: the faulted fence kept its region bound and failed the \
             drain visibly; the clean twin drained before first use (validation 0/0)"
        );
    }
}
