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
//! raining corner — validation 0/0, the rain ink present), the
//! packer's typed overflow refusal (a plain lib test), and the
//! ripple reflection gate (the forced hand-authored ripple field's
//! perturbation ink on the wet ground — the
//! setter→uniform→shader wire's visual acceptance).
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

use engine_core::ripple::{RippleField, RippleRing};
use engine_renderer::lit_scene::{LightParams, day_preset};
use engine_renderer::proxy_instances::{InstanceTransform, ProxyInstance};
use engine_renderer::{
    DemoScenePreference, ProxyDrawMode, ProxyPopulation, RenderConfig, Renderer,
    ValidationPreference,
};
use engine_shell::{DriverPreference, ShellApp, ShellConfig};
use engine_slice::corner_weather::{
    RAIN_ALPHA, RAIN_MATERIAL, RIPPLE_POLICY, RainLayer, yaw_to_turn,
};

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

/// THE BROADSIDE INK GATE AT THE PRESENTATION EXTENT (the
/// implementation seat's strengthening fold-in — the mining source
/// era2/pc-impl-seat-stage45's presentation-extent gate, adapted to
/// this harness): the PIXEL metric (not bytes — a changed pixel
/// counts once however many of its channels moved) over the FULL
/// 1280×720 extent, a 60-frame steady-state campaign (the field at
/// its census equilibrium, not the 8-frame fresh capture), the dry
/// campaign's own double-run as the baseline discipline, and THE
/// SKY-REGION CHECK — the fixed upper-third rectangle must carry a
/// majority of the ink (rain falls IN FRONT of the background: the
/// depth interaction's visible half; a depth-write regression that
/// punches streak holes, or a billboard regression that turns them
/// edge-on, collapses this count long before the total).
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe)"]
fn rain_ink_broadside_at_presentation_extent() {
    let _guard = sdl_lock();
    let (_app, mut renderer) = app_and_renderer("rain broadside ink gate");
    let (matrix, yaw) = gate_matrix();
    let light = day_preset();

    // The dry baseline, double-run (byte-identical — the baseline
    // discipline).
    renderer.set_proxy_draw_mode(ProxyDrawMode::Instanced);
    let dry = {
        let pop = rainless();
        for _ in 0..60 {
            renderer
                .render_corner_populated(&matrix, GATE_EYE, light, &pop, SIZE)
                .unwrap();
        }
        renderer.request_frame_capture().unwrap();
        assert!(
            renderer
                .render_corner_populated(&matrix, GATE_EYE, light, &pop, SIZE)
                .unwrap()
                .presented
        );
        renderer.take_frame_capture().unwrap().unwrap()
    };
    let dry_repeat = {
        let pop = rainless();
        for _ in 0..60 {
            renderer
                .render_corner_populated(&matrix, GATE_EYE, light, &pop, SIZE)
                .unwrap();
        }
        renderer.request_frame_capture().unwrap();
        assert!(
            renderer
                .render_corner_populated(&matrix, GATE_EYE, light, &pop, SIZE)
                .unwrap()
                .presented
        );
        renderer.take_frame_capture().unwrap().unwrap()
    };
    assert_eq!(
        dry.bytes, dry_repeat.bytes,
        "the dry campaign's double-run is byte-identical"
    );

    // The raining campaign: the 60-frame steady state.
    let raining = raining_capture(&mut renderer, &matrix, yaw, light, 60);

    // The pixel metric: 4-byte pixels, a pixel counts once when any
    // of its bytes moved. MEASURED on this pod's lavapipe (pinned in
    // the asserts below from the run's own print).
    assert_eq!(dry.bytes.len(), raining.bytes.len());
    let (width, height) = (SIZE.0 as usize, SIZE.1 as usize);
    let mut ink = 0usize;
    let mut sky_ink = 0usize;
    let sky_pixels = width * height / 3;
    for (index, (a, b)) in dry.bytes.chunks(4).zip(raining.bytes.chunks(4)).enumerate() {
        if a != b {
            ink += 1;
            if index < sky_pixels {
                sky_ink += 1;
            }
        }
    }
    println!(
        "rain broadside ink: {ink} pixels of {} changed ({sky_ink} in the sky region)",
        width * height
    );
    // MEASURED on this pod's lavapipe, this tree, this campaign -
    // RE-PINNED at the volume-refresh corrective: the landed
    // package C installed the OBSERVED BOX as the spawn volume
    // (collapsing the spawn band to eye level: drops materialized
    // at eye height, big near-camera streaks - the 1660/1350 the
    // gate originally pinned). The rain-ripples branch's corrective
    // re-derives the quantized BAND above the observed volume
    // (rain_emitter_config's own form - drops fall from above, the
    // physically-correct model); the steady-state coverage at this
    // vantage is now 473 pixels total, 282 in the sky region (the
    // 60% sky majority holds - rain still falls in front of the
    // background). The floors sit ~70% under the measurement while
    // staying ~2x above the edge-on collapse (~164 pixels - the
    // mining source's 8-frame fresh-capture gate).
    assert!(
        ink > 330,
        "broadside rain coverage: {ink} changed pixels at the presentation extent \
         (measured 473 at the steady state after the volume-refresh corrective; \
         the edge-on collapse measures ~164)"
    );
    assert!(
        sky_ink > 190,
        "the sky region carries {sky_ink} of the ink (measured 282 - rain falls \
         in front of the background)"
    );
}

/// One raining WET campaign at the gate camera with the ripple
/// uniform pinned per frame (the `raining_capture` shape plus the
/// sticky setter — the presentation bridge: the field state itself
/// is deterministic sim state (the campaign digest pins it), the
/// setter only copies it into the frame's uniform). `frames` warm
/// frames, then the captured presentation frame.
fn ripple_campaign(
    renderer: &mut Renderer,
    matrix: &[[f32; 4]; 4],
    yaw: f32,
    light: LightParams,
    ripple: Option<&RippleField>,
    frames: usize,
) -> engine_renderer::CapturedFrame {
    let mut layer = RainLayer::new(GATE_EYE);
    for _ in 0..frames {
        layer.update_and_build(GATE_EYE, yaw, 0.0);
        let population = layer.population();
        renderer.set_ripple_field(ripple);
        renderer
            .render_corner_populated(matrix, GATE_EYE, light, &population, SIZE)
            .unwrap();
    }
    renderer.request_frame_capture().unwrap();
    let population = layer.population();
    renderer.set_ripple_field(ripple);
    assert!(
        renderer
            .render_corner_populated(matrix, GATE_EYE, light, &population, SIZE)
            .unwrap()
            .presented
    );
    renderer.take_frame_capture().unwrap().unwrap()
}

/// THE RIPPLE REFLECTION GATE (Era-2 package C recursive item 1's
/// presentation acceptance — the setter→uniform→shader wire): the
/// SAME warmed raining WET campaign at the gate camera, the ripple
/// uniform pinned `None` (the empty uniform — the shader's identity
/// no-op, the rainless ripple path's byte-identity) vs a FORCED
/// hand-authored field (the demo's `RIPPLE_POLICY` with eight rings
/// hand-placed on the road cross in the camera's ground view —
/// `RippleField::author_ring`, the core's additive authoring path;
/// the field here is PRESENTATION state, not simulation: the
/// RainLayer's own field is never consulted, the deterministic sim
/// side is digest-pinned in engine-core). The rain instances are
/// byte-identical across both campaigns (the same seed, the same
/// 60-frame drive — the double-run asserts pin it), so the diff
/// isolates the ripple perturbation alone: the triangle-bump
/// gradient's uv shift plus the roughened Schlick blend on the wet
/// ground re-draw. THE GROUND-REGION CHECK (the broadside gate's
/// sky-region discipline inverted): the reflection draws on the
/// ground, which this camera maps to the LOWER half of the frame —
/// the majority of the ink must live there.
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe)"]
fn ripple_reflection_gate() {
    let _guard = sdl_lock();
    let (_app, mut renderer) = app_and_renderer("ripple reflection gate");

    let (matrix, yaw) = gate_matrix();
    let light = day_preset();

    // The wet path must be ON for both captures — the ripple
    // uniform rides the wet ground re-draw (a dry frame never
    // samples it; without this the gate would measure wet-vs-dry,
    // not ripple-vs-none).
    renderer.set_wet(true);
    renderer.set_proxy_draw_mode(ProxyDrawMode::Instanced);

    // The forced field: the DEMO's policy with eight hand-authored
    // rings on the open road cross in the camera's ground view (the
    // building masses start 6 m off-axis, so every ring's band
    // stays on visible ground; the camera sits on the north road at
    // [0, 1.7, 15] looking at the intersection). Ages spread
    // 0.35–1.0 s for radii 0.28–0.8 m at speed 0.8 m/s; amplitude
    // the policy's 1.0 unit scale (the renderer's gains consume
    // it); every ring projects into the frame's lower half (the
    // eye at 1.7 m looking slightly down — the horizon sits ~40%
    // from the top).
    let forced = {
        let mut field =
            RippleField::new(RIPPLE_POLICY).expect("the demo's ripple policy validates");
        let rings: [([f32; 2], f32); 8] = [
            ([-1.0, 3.5], 0.55),
            ([0.0, 7.0], 0.4),
            ([2.0, 9.5], 0.8),
            ([-2.5, 11.5], 1.0),
            ([3.5, 5.0], 0.5),
            ([-4.0, 6.5], 0.9),
            ([1.0, 12.5], 0.6),
            ([-0.5, 5.5], 0.35),
        ];
        for (center, age) in rings {
            assert!(
                field
                    .author_ring(RippleRing {
                        center,
                        age,
                        amplitude: 1.0,
                    })
                    .expect("the authored ring is valid data")
                    .is_none(),
                "the pool has room (8 of {})",
                engine_core::ripple::RIPPLE_POOL_BOUND
            );
        }
        assert_eq!(field.active_count(), rings.len());
        field
    };

    // The baseline: ripple None (the empty uniform — the shader's
    // identity no-op), double-run (byte-identical — the baseline
    // discipline).
    let none_a = ripple_campaign(&mut renderer, &matrix, yaw, light, None, 60);
    let none_b = ripple_campaign(&mut renderer, &matrix, yaw, light, None, 60);
    assert_eq!(
        none_a.bytes, none_b.bytes,
        "the None-ripple campaign double-runs byte-identical"
    );

    // The forced field, double-run (the authored field is static
    // across the campaign — deterministic by construction).
    let ripple_a = ripple_campaign(&mut renderer, &matrix, yaw, light, Some(&forced), 60);
    let ripple_b = ripple_campaign(&mut renderer, &matrix, yaw, light, Some(&forced), 60);
    assert_eq!(
        ripple_a.bytes, ripple_b.bytes,
        "the forced-ripple campaign double-runs byte-identical"
    );

    // The metrics: differing bytes AND differing pixels (a pixel
    // counts once — the broadside gate's pixel discipline), plus
    // the lower-half split for the ground-region check.
    assert_eq!(none_a.bytes.len(), ripple_a.bytes.len());
    let (width, height) = (SIZE.0 as usize, SIZE.1 as usize);
    let mut differing_bytes = 0usize;
    for (a, b) in none_a.bytes.iter().zip(ripple_a.bytes.iter()) {
        differing_bytes += usize::from(a != b);
    }
    let mut ink = 0usize;
    let mut lower_ink = 0usize;
    let lower_half = width * height / 2;
    for (index, (a, b)) in none_a
        .bytes
        .chunks(4)
        .zip(ripple_a.bytes.chunks(4))
        .enumerate()
    {
        if a != b {
            ink += 1;
            if index >= lower_half {
                lower_ink += 1;
            }
        }
    }
    println!(
        "ripple reflection ink: {differing_bytes} differing bytes, {ink} differing pixels \
         ({lower_ink} in the lower half)"
    );
    // MEASURED on this pod's lavapipe: PENDING the first CI run's
    // --nocapture print above — the floors below are the
    // CONSERVATIVE START, to be corrected from the run's own
    // actuals (the repo's established pattern: the ink-count gate's
    // 492-byte measurement against its 200 floor). The roughen term
    // alone guarantees ink wherever a band covers ground (the
    // Schlick blend scales by up to faded-amplitude x
    // RIPPLE_ROUGHEN_GAIN 0.5 — eight annuli of 0.6–1.15 m outer
    // radius at 2.5–11.5 m from the camera cover far more than the
    // floors), so the true count is expected well above them.
    const RIPPLE_BYTE_FLOOR: usize = 100;
    const RIPPLE_PIXEL_FLOOR: usize = 25;
    assert!(
        differing_bytes >= RIPPLE_BYTE_FLOOR,
        "the forced ripple field moved at least {RIPPLE_BYTE_FLOOR} bytes of the capture \
         (got {differing_bytes}) — the perturbation never reached the wet ground re-draw: \
         check the setter→uniform→shader wire"
    );
    assert!(
        ink >= RIPPLE_PIXEL_FLOOR,
        "the forced ripple field moved at least {RIPPLE_PIXEL_FLOOR} pixels (got {ink})"
    );
    // The ground region (the broadside gate's sky-region check
    // inverted): the ripple perturbs the wet ground re-draw, which
    // this camera maps below the midline — a majority of the ink
    // must live in the lower half, never the sky.
    assert!(
        lower_ink * 2 > ink,
        "the ripple ink lives on the wet ground: {lower_ink} of {ink} differing pixels \
         in the lower half (the reflection draws on the ground, not the sky)"
    );

    // The composition stays clean: the ripple uniform path (the
    // shader's binding-4 block, the CPU packer's layout twin)
    // trips no validation.
    let stats = renderer.stats();
    assert_eq!(
        stats.validation_errors, 0,
        "no validation errors: {:?}",
        stats.recent_validation
    );
    assert_eq!(stats.validation_warnings, 0, "no validation warnings");
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
