//! THE P5 R0 §9.4 BATTERY (the attach/upload census — A20/R3's
//! before row): the staging machinery's cost at scene attach,
//! through the four NEW read-only counters on
//! `engine_renderer::AllocationStats` (`upload_submits` — each a
//! fresh pool + command buffer + fence + ONE submit + ONE blocking
//! wait, then destruction; `upload_blocking_wait_ns` — the
//! blocking-wait total; `staged_buffer_uploads` — the throwaway
//! HOST_VISIBLE staging pairs; `texture_staging_tail_bytes` — the
//! permanent per-texture tails). One fresh renderer per scene
//! (corner, garage): construction read, TIMED attach, attach read,
//! then warm dry frames proving the FRAME path never touches the
//! one-shot staging machinery (the counters freeze — the
//! attach-only boundary R3's pool then takes over).
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

use engine_renderer::{DemoScenePreference, RenderConfig, Renderer, ValidationPreference};
use engine_shell::{DriverPreference, ShellApp, ShellConfig};

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

/// THE ATTACH CENSUS: per scene — the construction split (the
/// renderer's own atlas upload), the TIMED attach, the counters at
/// attach, and the frame-path freeze proof.
#[test]
#[ignore = "requires a real Vulkan device and windows (the rig run: release, serial, archived per E-8)"]
fn attach_upload_census() {
    let _guard = sdl_lock();
    println!("P5 R0 §9.4 — the attach/upload census (BEFORE R3: per-object staging)");
    println!(
        "  row: scene | attach-wall-ms | submits | blocking-wait-ms | staged-pairs | \
         tail-B | tex-mem tex-bytes | dev-mem"
    );

    // The corner scene (the batteries' own attach path).
    {
        let (mut app, mut renderer) = fresh_renderer("P5 census attach (corner)");
        let constructed = renderer.allocations();
        let attach_started = Instant::now();
        engine_slice::corner_scene::attach_corner_scene(&mut renderer).unwrap();
        let attach_wall = attach_started.elapsed();
        let attached = renderer.allocations();
        let _ = app.frame();
        // Warm dry frames: the frame path must not move ANY of the
        // four staging counters (the one-shot machinery is
        // attach-only today — the boundary R3's pool changes).
        let matrix = engine_slice::corner_demo::view_projection(
            engine_slice::corner_demo::route_poses()
                .next()
                .expect("the route has a first waypoint"),
            16.0 / 9.0,
        );
        let submission = engine_slice::corner_demo::corner_submission(
            &matrix,
            [0.0, 1.7, 15.0],
            engine_renderer::lit_scene::day_preset(),
            None,
            engine_slice::corner_scene::corner_bounds(),
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
                after_frames.texture_staging_tail_bytes
            ),
            (
                attached.upload_submits,
                attached.staged_buffer_uploads,
                attached.texture_staging_tail_bytes
            ),
            "the frame path never touches the one-shot staging machinery"
        );
        assert_eq!(renderer.stats().validation_errors, 0);
        println!(
            "  corner | {:.2} | {} ({} at attach, {} at construction) | {:.2} | {} | {} | {} {} | {}",
            attach_wall.as_secs_f64() * 1e3,
            attached.upload_submits,
            attached.upload_submits - constructed.upload_submits,
            constructed.upload_submits,
            attached.upload_blocking_wait_ns as f64 / 1e6,
            attached.staged_buffer_uploads,
            attached.texture_staging_tail_bytes,
            attached.texture_memory,
            attached.texture_bytes,
            attached.device_memory
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
        println!(
            "  garage | {:.2} | {} ({} at attach, {} at construction) | {:.2} | {} | {} | {} {} | {}",
            attach_wall.as_secs_f64() * 1e3,
            attached.upload_submits,
            attached.upload_submits - constructed.upload_submits,
            constructed.upload_submits,
            attached.upload_blocking_wait_ns as f64 / 1e6,
            attached.staged_buffer_uploads,
            attached.texture_staging_tail_bytes,
            attached.texture_memory,
            attached.texture_bytes,
            attached.device_memory
        );
        assert_eq!(renderer.stats().validation_errors, 0);
        assert!(
            attached.upload_submits >= constructed.upload_submits,
            "attach only adds uploads"
        );
    }
}
