//! THE P5 R0 §9.5 BATTERY (the ripple rows — A19/R4's before/after
//! axis): the wet route at {empty, ~3.7-ring steady, 32-ring
//! saturation} × {720p, 1440p}, the gpu_ms percentiles per cell.
//! The field state is the ONLY variable — every cell renders the
//! same populated corner submission (24+2 crowd, no rain instances:
//! the rain-instance axis belongs to §9.6's composition row) with
//! the WetFrame's ripple field swapped per state, so the row
//! isolates the per-fragment ring loop's cost exactly the way the
//! finding names it (the loop bounded by the field's active
//! extent). The identity gates (render_weather's ink/term
//! separations) re-run in the same rig sessions per the design's
//! §9.5 note — this battery adds the timestamp row, not a
//! replacement for them.
//!
//! `#[ignore]`d (a real device + windows). THE RIG RUN:
//!
//! ```sh
//! cargo test -p engine-slice --test census_ripple --release -- \
//!     --ignored --nocapture --test-threads=1
//! ```
//!
//! Non-ignored leg: the saturated field's construction (32 rings,
//! the pool bound — pure CPU, so the battery's fixture is pinned
//! wherever the lib tests run).

use engine_renderer::{ProxyDrawMode, ProxyPopulation};
use engine_shell::ShellApp;

use engine_slice::census::composition::corner_renderer_at;
use engine_slice::census::wet::{WET_ROUTE_EYE, saturated_field, steady_rain_layer};
use engine_slice::corner_demo::{corner_submission, route_poses, view_projection};
use engine_slice::corner_population::{corner_population, evaluate_population_poses};
use engine_slice::corner_scene::corner_bounds;

fn sdl_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}

/// The three field states (§9.5's axis), labeled with the measured
/// live count where the state is constructed through the real rain
/// machinery.
enum FieldState {
    Empty,
    Steady,
    Saturated,
}

impl FieldState {
    fn label(&self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Steady => "~3.7-ring steady",
            Self::Saturated => "32-ring saturation",
        }
    }
}

/// One cell: `frames` wet submissions with the cell's field state,
/// returning (frame-samples, gpu-samples) milliseconds.
fn render_wet_cell(
    app: &mut ShellApp,
    renderer: &mut engine_renderer::Renderer,
    state: &FieldState,
    frames: usize,
) -> (Vec<f64>, Vec<f64>) {
    let size = renderer.stats().extent;
    let aspect = size.0 as f32 / size.1 as f32;
    let pose = route_poses().next().expect("the route's first waypoint");
    let matrix = view_projection(pose, aspect);
    let (npc, vehicles) = corner_population();
    let phases = engine_slice::corner_population::corner_population_phases();
    let mut poses = Vec::new();
    evaluate_population_poses(0, &phases, &mut poses);
    let population = ProxyPopulation {
        instance_generation: 0,
        pose_generation: 0,
        npc: &npc,
        npc_poses: &poses,
        npc_material: 1,
        vehicle: &vehicles,
        vehicle_material: 2,
        rain: &[],
        rain_material: 0,
    };
    renderer.set_proxy_draw_mode(ProxyDrawMode::Instanced);
    // The steady state's field comes from the REAL rain machinery
    // (warm-started; borrowed per frame the way the demo borrows
    // it); the saturated state from the deterministic authored 32.
    let steady = (matches!(state, FieldState::Steady)).then(steady_rain_layer);
    let saturated = (matches!(state, FieldState::Saturated)).then(saturated_field);
    let mut frame_ms = Vec::with_capacity(frames);
    let mut gpu_ms = Vec::with_capacity(frames);
    for _ in 0..frames {
        let _ = app.frame();
        let wet = Some(engine_renderer::WetFrame {
            ripple: steady
                .as_ref()
                .map(|layer| layer.ripple_field())
                .or(saturated.as_ref()),
        });
        let started = std::time::Instant::now();
        let result = renderer
            .submit_frame(corner_submission(
                &matrix,
                pose.position,
                engine_renderer::lit_scene::day_preset(),
                None,
                corner_bounds(),
                Some(population),
                None,
                false,
                wet,
                size,
            ))
            .unwrap();
        assert!(result.presented, "the wet cell presents");
        frame_ms.push(started.elapsed().as_secs_f64() * 1e3);
        if let Some(ms) = result.gpu_render_ms {
            gpu_ms.push(ms);
        }
    }
    (frame_ms, gpu_ms)
}

/// One row's percentile block (sorted, nearest-rank — the house
/// convention).
fn percentiles(samples: &mut [f64]) -> (f64, f64, f64, f64) {
    samples.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    let n = samples.len();
    let at = |q: f64| {
        samples[(((n as f64) * q).ceil() as usize)
            .saturating_sub(1)
            .min(n - 1)]
    };
    (at(0.50), at(0.95), at(0.99), samples[n - 1])
}

/// THE RIPPLE ROWS (the rig leg): 3 states × 2 extents.
#[test]
#[ignore = "requires a real Vulkan device and windows (the rig run: release, serial, archived per E-8)"]
fn ripple_timestamp_rows() {
    let _guard = sdl_lock();
    println!(
        "P5 R0 §9.5 — the ripple rows (gpu_ms on the wet route; the field state is the only variable)"
    );
    println!(
        "  row: state | extent | gpu-ms p50/p95/p99/max | frame-ms p50/p99/max | live-rings | n"
    );
    for extent in [(1280, 720), (2560, 1440)] {
        for state in [FieldState::Empty, FieldState::Steady, FieldState::Saturated] {
            let (mut app, mut renderer) =
                corner_renderer_at("P5 census ripple", extent).expect("the cell constructs");
            // Warm both slots on the wet path before measuring (the
            // lazy reflection chain + per-slot resources).
            let _ = render_wet_cell(&mut app, &mut renderer, &state, 4);
            let (mut frames, mut gpu) = render_wet_cell(&mut app, &mut renderer, &state, 120);
            assert_eq!(renderer.stats().validation_errors, 0);
            let live = match state {
                FieldState::Empty => 0,
                FieldState::Steady => steady_rain_layer().ripple_field().active_count(),
                FieldState::Saturated => saturated_field().active_count(),
            };
            let (p50, p95, p99, max) = percentiles(&mut gpu);
            let (f50, _, f99, fmax) = percentiles(&mut frames);
            println!(
                "  {} | {}x{} | {:.3}/{:.3}/{:.3}/{:.3} | {:.2}/{:.2}/{:.2} | {} | {}",
                state.label(),
                extent.0,
                extent.1,
                p50,
                p95,
                p99,
                max,
                f50,
                f99,
                fmax,
                live,
                gpu.len()
            );
            drop(renderer);
            drop(app);
        }
    }
}

/// The saturated fixture is pinned wherever the battery compiles
/// (the §9.5 axis's worst-case endpoint — 32 rings, the pool
/// bound).
#[test]
fn saturated_fixture_is_the_pool_bound() {
    let field = saturated_field();
    assert_eq!(field.active_count(), 32);
    let _eye = WET_ROUTE_EYE;
}
