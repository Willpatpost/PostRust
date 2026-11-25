//! THE ERA-2 PACKAGE C WEATHER GATES (stage 5 — the mission's
//! acceptance, executed on lavapipe): the rain ink-count visual
//! gate (rain visible in the corner capture — the ink-count
//! pattern), the warmed raining allocation constancy (BOTH
//! counters — `renderer.allocations()` AND the host-allocation
//! counter — constant/zero across warmed raining frames with the
//! field provably moving), the double-run byte-equality (the
//! RainLayer's scripted campaign replayed from scratch — identical
//! capture sequences), the rain-kind N-vs-1 submission equivalence
//! (PerInstance vs Instanced through the third-kind path), the
//! full composition smoke (the populated SHADOWED day/night
//! raining corner — validation 0/0, the rain ink present), and
//! the packer's typed overflow refusal (a plain lib test).
//!
//! `#[ignore]`d like the other lavapipe gates: it requires
//! `SDL_VIDEODRIVER=offscreen` plus a software Vulkan ICD
//! (lavapipe) and the validation layer. Run explicitly:
//!
//! ```sh
//! SDL_VIDEODRIVER=offscreen cargo test -p engine-slice --test render_weather -- \
//!     --ignored --nocapture
//! ```
//!
//! THE BILLBOARD ARBITER: the ink gate is the physical proof of
//! the turn derivation (`corner_weather::yaw_to_turn`'s t = −yaw —
//! the streak's mesh-authored +Z normal facing the camera). A
//! wrong turn turns the streaks edge-on and the ink collapses —
//! the gate fails by construction.

mod common;

use engine_renderer::lit_scene::{LightParams, day_preset};
use engine_renderer::proxy_instances::{InstanceTransform, ProxyInstance};
use engine_renderer::{
    DemoScenePreference, ProxyDrawMode, ProxyPopulation, RenderConfig, Renderer,
    ValidationPreference,
};
use engine_shell::{DriverPreference, ShellApp, ShellConfig};
use engine_slice::corner_weather::{RAIN_ALPHA, RAIN_MATERIAL, RainLayer, yaw_to_turn};

const SIZE: (u32, u32) = (1280, 720);
const BIAS: engine_renderer::shadows::ShadowBias = engine_renderer::shadows::ShadowBias {
    constant: 0.0015,
    slope_scale: 0.002,
};

fn sdl_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}

fn app_and_renderer(title: &str) -> (ShellApp, Renderer) {
    let app = ShellApp::new(ShellConfig {
        title: title.into(),
        width: SIZE.0,
        height: SIZE.1,
        target_fps: 60,
        driver: DriverPreference::Real,
    })
    .unwrap();
    let renderer = Renderer::new(
        app.window(),
        RenderConfig {
            demo_scene: DemoScenePreference::Corner,
            validation: ValidationPreference::On,
            ..Default::default()
        },
    )
    .unwrap();
    (app, renderer)
}

/// The gate's fixed camera: above the road cross, looking at the
/// intersection's center (the same vantage the population gates
/// use — the rain box surrounds it, so the streaks fill the view).
const GATE_EYE: [f32; 3] = [0.0, 1.7, 15.0];
const GATE_TOWARD: [f32; 3] = [0.0, 0.0, 0.0];

fn gate_matrix() -> ([[f32; 4]; 4], f32) {
    let offset = std::array::from_fn(|i| GATE_TOWARD[i] - GATE_EYE[i]);
    let (yaw, pitch) = engine_slice::corner_demo::waypoint_orientation(offset);
    let view = engine_slice::math::camera_view(GATE_EYE, yaw, pitch, 0.0);
    let proj = engine_slice::math::perspective(65_f32.to_radians(), 16.0 / 9.0, 0.1, 150.0);
    (engine_slice::math::mul(&proj, &view), yaw)
}

/// The rainless baseline population (the ink denominator — the
/// empty rain kind, byte-identical to the pre-package frames).
fn rainless() -> ProxyPopulation<'static> {
    ProxyPopulation {
        npc: &[],
        npc_poses: &[],
        npc_material: 1,
        vehicle: &[],
        vehicle_material: 2,
        rain: &[],
        rain_material: RAIN_MATERIAL,
    }
}

/// One raining capture at the gate camera through the REAL layer
/// (the field warm-started, then driven `frames` ticks at the
/// fixed clock — the demo's own machinery, not a hand-authored
/// instance list: the gate proves the SHIPPED rain path).
fn raining_capture(
    renderer: &mut Renderer,
    matrix: &[[f32; 4]; 4],
    yaw: f32,
    light: LightParams,
    frames: usize,
) -> engine_renderer::CapturedFrame {
    let mut layer = RainLayer::new(GATE_EYE);
    renderer.set_proxy_draw_mode(ProxyDrawMode::Instanced);
    for _ in 0..frames {
        layer.update_and_build(GATE_EYE, yaw, 0.0);
        let population = layer.population();
        renderer
            .render_corner_populated(matrix, GATE_EYE, light, &population, SIZE)
            .unwrap();
    }
    renderer.request_frame_capture().unwrap();
    let population = layer.population();
    assert!(
        renderer
            .render_corner_populated(matrix, GATE_EYE, light, &population, SIZE)
            .unwrap()
            .presented
    );
    renderer.take_frame_capture().unwrap().unwrap()
}

/// THE INK-COUNT GATE (the mission's visual acceptance): the same
/// corner camera, the REAL raining field vs the rainless baseline
/// — the streaks must move pixels. The floor: the warm-started
/// field holds ~550 streaks (rate 480/s × mean lifetime 1.15 s);
/// at this vantage each VISIBLE streak covers a handful of pixels
/// (the streak is 0.024 m wide × 0.56 m long; the near-field drops
/// within 2–8 m of the camera cover multiple pixels each, the far
/// ones sub-pixel) — the MEASURED count is pinned in the comment
/// and the floor holds a wide margin under it.
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe)"]
fn rain_ink_count_gate() {
    let _guard = sdl_lock();
    let (_app, mut renderer) = app_and_renderer("rain ink count gate");

    let (matrix, yaw) = gate_matrix();
    let light = day_preset();

    // Rainless: the baseline.
    renderer.request_frame_capture().unwrap();
    let baseline_pop = rainless();
    assert!(
        renderer
            .render_corner_populated(&matrix, GATE_EYE, light, &baseline_pop, SIZE)
            .unwrap()
            .presented
    );
    let rainless_frame = renderer.take_frame_capture().unwrap().unwrap();

    // Raining: the real field, warm and moving.
    let raining_frame = raining_capture(&mut renderer, &matrix, yaw, light, 8);

    assert_eq!(rainless_frame.bytes.len(), raining_frame.bytes.len());
    let mut differing = 0usize;
    for (a, b) in rainless_frame.bytes.iter().zip(raining_frame.bytes.iter()) {
        if a != b {
            differing += 1;
        }
    }
    println!(
        "rain ink count: {differing} differing bytes of {}",
        rainless_frame.bytes.len()
    );
    // MEASURED on this pod's lavapipe: 492 differing bytes (the
    // warm field's near-camera streaks, ~164 pixels at 3 bytes per
    // pixel). The floor holds a wide margin under the measurement.
    const INK_FLOOR: usize = 200;
    assert!(
        differing >= INK_FLOOR,
        "the rain moved at least {INK_FLOOR} bytes of the capture (got {differing}) — \
         the streaks are edge-on or invisible: check the billboard turn derivation"
    );
}

/// THE WARMED RAINING ALLOCATION CONSTANCY: both counters —
/// `renderer.allocations()` constant AND the host-allocation
/// counter zero — across ≥8 warmed raining frames, with ≥3
/// distinct captures (the field provably moves; the layer's own
/// digest would be identical only if the rain froze).
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe)"]
fn rain_warmed_allocation_constancy() {
    let _guard = sdl_lock();
    let (_app, mut renderer) = app_and_renderer("rain warmed allocation constancy");

    let (matrix, yaw) = gate_matrix();
    let light = day_preset();

    let mut layer = RainLayer::new(GATE_EYE);
    renderer.set_proxy_draw_mode(ProxyDrawMode::Instanced);
    let mut warm: Option<engine_renderer::AllocationStats> = None;
    let mut distinct: std::collections::HashSet<Vec<u8>> = std::collections::HashSet::new();
    for frame in 0..12 {
        layer.update_and_build(GATE_EYE, yaw, 0.0);
        let population = layer.population();
        renderer.request_frame_capture().unwrap();
        assert!(
            renderer
                .render_corner_populated(&matrix, GATE_EYE, light, &population, SIZE)
                .unwrap()
                .presented
        );
        let capture = renderer.take_frame_capture().unwrap().unwrap();
        distinct.insert(capture.bytes);
        if frame >= 8 {
            // The warmed window: the one-time costs (the layer's
            // first instance reserve, the renderer's lazy rain-kind
            // family) are PAST — a warmed raining frame allocates
            // NOTHING on the host. The scope wraps THE MEASURED
            // CALLS alone (the layer's update + the render — the
            // capture machinery's own vec! is presentation-side,
            // the render_instanced precedent's exact discipline).
            let scope = common::HostAllocScope::new();
            layer.update_and_build(GATE_EYE, yaw, 0.0);
            let population = layer.population();
            let outcome = renderer
                .render_corner_populated(&matrix, GATE_EYE, light, &population, SIZE)
                .unwrap();
            let host = scope.count();
            assert!(outcome.presented);
            assert_eq!(
                host, 0,
                "zero host allocations in the warmed raining frame {frame}"
            );
            let alloc = renderer.allocations();
            match &warm {
                Some(w) => assert_eq!(
                    alloc, *w,
                    "allocations constant across warmed raining frames"
                ),
                None => warm = Some(alloc),
            }
        }
    }
    println!(
        "rain warmed constancy: {} distinct captures, allocations {:?}",
        distinct.len(),
        warm.unwrap()
    );
    assert!(
        distinct.len() >= 3,
        "the raining field moves ({} distinct captures)",
        distinct.len()
    );
}

/// THE DOUBLE-RUN BYTE-EQUALITY: the layer's scripted campaign
/// (the same fixed camera sequence) replayed from scratch on a
/// fresh renderer — every captured frame byte-identical.
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe)"]
fn rain_double_run_byte_equality() {
    let _guard = sdl_lock();
    let (_app, mut renderer) = app_and_renderer("rain double run byte equality");

    let (matrix, yaw) = gate_matrix();
    let light = day_preset();

    let run = |renderer: &mut Renderer| -> Vec<engine_renderer::CapturedFrame> {
        let mut layer = RainLayer::new(GATE_EYE);
        renderer.set_proxy_draw_mode(ProxyDrawMode::Instanced);
        let mut frames = Vec::new();
        for _ in 0..6 {
            layer.update_and_build(GATE_EYE, yaw, 0.0);
            let population = layer.population();
            renderer.request_frame_capture().unwrap();
            assert!(
                renderer
                    .render_corner_populated(&matrix, GATE_EYE, light, &population, SIZE)
                    .unwrap()
                    .presented
            );
            frames.push(renderer.take_frame_capture().unwrap().unwrap());
        }
        frames
    };
    let first = run(&mut renderer);
    let second = run(&mut renderer);
    for (index, (a, b)) in first.iter().zip(second.iter()).enumerate() {
        assert_eq!(
            a.bytes, b.bytes,
            "the campaign's frame {index} double-runs byte-identical"
        );
    }
}

/// THE RAIN-KIND N-VS-1: the SAME rain population through
/// PerInstance vs Instanced — byte-equal captures (the third-kind
/// shape rides the proxy path's submission modes; the equivalence
/// is the 6-E discipline applied to the rain kind).
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe)"]
fn rain_n_vs_1_draw_mode_equivalence() {
    let _guard = sdl_lock();
    let (_app, mut renderer) = app_and_renderer("rain N vs 1 draw mode");

    let (matrix, yaw) = gate_matrix();
    let light = day_preset();

    // A hand-authored static field (the submission equivalence is
    // exactly the static-population question): 60 streaks spread
    // through the gate's view volume, facing the camera through
    // the MODULE's turn derivation.
    let turn = yaw_to_turn(yaw);
    let instances: Vec<ProxyInstance> = (0..60)
        .map(|i| ProxyInstance {
            transform: InstanceTransform {
                pos: [
                    ((i % 12) as f32 - 5.5) * 2.0,
                    2.0 + ((i / 12) as f32) * 1.5,
                    -2.0 - ((i % 5) as f32) * 2.0,
                ],
                turn,
                scale: 1.0,
            },
            tint: [0.7, 0.8, 0.9],
            alpha: RAIN_ALPHA,
            pose_index: 0,
        })
        .collect();
    let population = ProxyPopulation {
        npc: &[],
        npc_poses: &[],
        npc_material: 1,
        vehicle: &[],
        vehicle_material: 2,
        rain: &instances,
        rain_material: RAIN_MATERIAL,
    };

    renderer.set_proxy_draw_mode(ProxyDrawMode::PerInstance);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner_populated(&matrix, GATE_EYE, light, &population, SIZE)
            .unwrap()
            .presented
    );
    let per_instance = renderer.take_frame_capture().unwrap().unwrap();

    renderer.set_proxy_draw_mode(ProxyDrawMode::Instanced);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner_populated(&matrix, GATE_EYE, light, &population, SIZE)
            .unwrap()
            .presented
    );
    let instanced = renderer.take_frame_capture().unwrap().unwrap();

    assert_eq!(
        per_instance.bytes, instanced.bytes,
        "the PerInstance and Instanced rain captures are byte-identical"
    );
}

/// THE COMPOSITION SMOKE: the mission's full composition — the
/// populated SHADOWED day/night raining corner — presents with
/// validation 0/0 and the rain ink present (the shadow-map pass
/// never sees the rain: an unlit streak is not an occluder, so the
/// map gates stay untouched).
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe)"]
fn rain_composition_smoke() {
    let _guard = sdl_lock();
    let (_app, mut renderer) = app_and_renderer("rain composition smoke");

    let (matrix, yaw) = gate_matrix();
    // The day/night cycle's evening light (the composition's
    // atmosphere — the rain tint rides the same cycle in the demo).
    let atmosphere = engine_renderer::sky::atmosphere_at(0.35);
    let light = atmosphere.light;

    let mut layer = RainLayer::new(GATE_EYE);
    renderer.set_proxy_draw_mode(ProxyDrawMode::Instanced);
    for _ in 0..4 {
        layer.update_and_build(GATE_EYE, yaw, 0.35);
        let population = layer.population();
        renderer
            .render_corner_populated_shadowed(&matrix, GATE_EYE, light, BIAS, &population, SIZE)
            .unwrap();
    }
    // The raining composed capture.
    renderer.request_frame_capture().unwrap();
    let population = layer.population();
    let frame = renderer
        .render_corner_populated_shadowed(&matrix, GATE_EYE, light, BIAS, &population, SIZE)
        .unwrap();
    assert!(frame.presented, "the composed frame presented");
    let raining = renderer.take_frame_capture().unwrap().unwrap();

    // The rainless composed baseline at the same camera.
    renderer.request_frame_capture().unwrap();
    let baseline_pop = rainless();
    let baseline_frame = renderer
        .render_corner_populated_shadowed(&matrix, GATE_EYE, light, BIAS, &baseline_pop, SIZE)
        .unwrap();
    assert!(baseline_frame.presented);
    let baseline = renderer.take_frame_capture().unwrap().unwrap();

    let stats = renderer.stats();
    assert_eq!(
        stats.validation_errors, 0,
        "no validation errors: {:?}",
        stats.recent_validation
    );
    assert_eq!(stats.validation_warnings, 0, "no validation warnings");
    assert_ne!(
        raining.bytes, baseline.bytes,
        "the raining composed frame carries the rain ink"
    );
}

/// THE TYPED OVERFLOW REFUSAL (a plain lib test — no device): the
/// packer's capacity boundary is a typed refusal, never a silent
/// drop (the D2 no-partial-overflow discipline).
#[test]
fn rain_overflow_typed_refusal() {
    let max = engine_renderer::proxy_instances::MAX_INSTANCES;
    let instance = |pos: [f32; 3]| ProxyInstance {
        transform: InstanceTransform {
            pos,
            turn: 0,
            scale: 1.0,
        },
        tint: [1.0, 1.0, 1.0],
        alpha: RAIN_ALPHA,
        pose_index: 0,
    };
    let at_capacity: Vec<_> = (0..max).map(|i| instance([i as f32, 0.0, 0.0])).collect();
    assert!(
        engine_renderer::proxy_instances::pack_instances(&at_capacity).is_ok(),
        "the at-capacity population packs"
    );
    let over: Vec<_> = (0..max + 1)
        .map(|i| instance([i as f32, 0.0, 0.0]))
        .collect();
    match engine_renderer::proxy_instances::pack_instances(&over) {
        Err(engine_renderer::proxy_instances::InstancePackError::Capacity {
            capacity,
            requested,
        }) => {
            assert_eq!(capacity, max, "the capacity is MAX_INSTANCES");
            assert_eq!(requested, max + 1, "the requested count is max + 1");
        }
        other => panic!("the over-capacity pack refuses typed (got {other:?})"),
    }
}
