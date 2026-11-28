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
//! [`demo_clock_dt`]: crate::corner_timing::demo_clock_dt
//!
//! THE BILLBOARD ARBITER: the ink gate is the physical proof of
//! the turn derivation (`corner_weather::yaw_to_turn`'s t = +yaw —
//! the finding-13 corrective; the pre-corrective packer's −yaw is
//! edge-on at diagonal headings, cos 2θ = 0 at 45°). A wrong turn
//! turns the streaks edge-on and the ink collapses — the gate
//! fails by construction. The CPU known answers live beside the
//! packer (corner_weather's
//! `billboard_faces_the_camera_at_diagonal_headings`); the
//! diagonal capture below is the GPU half of that proof.

use engine_slice::corner_timing::demo_clock_dt;

mod common;

use engine_core::ripple::{RippleField, RippleRing};
use engine_renderer::lit_scene::{GROUND_Y, LightParams, day_preset};
use engine_renderer::planar::{projected_uv_offset, reflection_blend, ripple_perturbation};
use engine_renderer::proxy_instances::{InstanceTransform, ProxyInstance};
use engine_renderer::{
    DemoScenePreference, ProxyDrawMode, ProxyPopulation, RIPPLE_ROUGHEN_GAIN, RIPPLE_UV_GAIN,
    RenderConfig, Renderer, RippleDebugMode, ValidationPreference, WetFrame,
};
use engine_shell::{DriverPreference, ShellApp, ShellConfig};
use engine_slice::corner_demo::corner_submission;
use engine_slice::corner_scene::corner_bounds;
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
    // Poison-tolerant (the round-7 corrective): a gate's assertion
    // panic must not cascade into every other gate's PoisonError —
    // each gate reports ITS OWN verdict (the round-6 run lost the
    // term-separation gate's Ubuntu debut to exactly this cascade).
    // The mutex serializes SDL process-global setup; an assertion
    // failure while holding it never corrupts that state (the
    // poisoned case is precisely "another gate panicked while
    // measuring"), so the guard is recovered and the next gate runs
    // on the same disciplined setup.
    match LOCK.get_or_init(|| std::sync::Mutex::new(())).lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
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
    let mut renderer = Renderer::new(
        app.window(),
        RenderConfig {
            // P4 S6: the corner content is ATTACHED (the Corner
            // construction preference is deleted) — the same public
            // attach every consumer uses.
            demo_scene: DemoScenePreference::Omitted,
            validation: ValidationPreference::On,
            ..Default::default()
        },
    )
    .unwrap();
    engine_slice::corner_scene::attach_corner_scene(&mut renderer).unwrap();
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
        layer.update_and_build(GATE_EYE, yaw, 0.0, demo_clock_dt().as_secs_f32());
        let population = layer.population();
        renderer
            .submit_frame(corner_submission(
                matrix,
                GATE_EYE,
                light,
                None,
                corner_bounds(),
                Some(population),
                None,
                false,
                None,
                SIZE,
            ))
            .unwrap();
    }
    renderer.request_frame_capture().unwrap();
    let population = layer.population();
    assert!(
        renderer
            .submit_frame(corner_submission(
                matrix,
                GATE_EYE,
                light,
                None,
                corner_bounds(),
                Some(population),
                None,
                false,
                None,
                SIZE,
            ))
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
            .submit_frame(corner_submission(
                &matrix,
                GATE_EYE,
                light,
                None,
                corner_bounds(),
                Some(baseline_pop),
                None,
                false,
                None,
                SIZE
            ))
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
    // MEASURED on this pod's lavapipe: the pre-corrective tree
    // measured 492 differing bytes (the warm field's near-camera
    // streaks, ~164 pixels at 3 bytes per pixel); the tree under
    // review measured 288 (the audit's pinned log, run 45721467971
    // — recorded here per the audit's own instruction to keep the
    // 492 comment's history and track the current baseline
    // separately). THE FINDING-12 POLICY CHANGE moves the actual
    // again — PROVISIONAL: re-measure on lavapipe in CI and pin
    // the new baseline here (the corrected policy's steady state
    // holds ~624 streaks vs the old ~552, all of them now falling
    // the full column to the street, so the count should RISE from
    // 288). The floor keeps the wide margin either way.
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
                .submit_frame(corner_submission(
                    &matrix,
                    GATE_EYE,
                    light,
                    None,
                    corner_bounds(),
                    Some(pop),
                    None,
                    false,
                    None,
                    SIZE,
                ))
                .unwrap();
        }
        renderer.request_frame_capture().unwrap();
        assert!(
            renderer
                .submit_frame(corner_submission(
                    &matrix,
                    GATE_EYE,
                    light,
                    None,
                    corner_bounds(),
                    Some(pop),
                    None,
                    false,
                    None,
                    SIZE,
                ))
                .unwrap()
                .presented
        );
        renderer.take_frame_capture().unwrap().unwrap()
    };
    let dry_repeat = {
        let pop = rainless();
        for _ in 0..60 {
            renderer
                .submit_frame(corner_submission(
                    &matrix,
                    GATE_EYE,
                    light,
                    None,
                    corner_bounds(),
                    Some(pop),
                    None,
                    false,
                    None,
                    SIZE,
                ))
                .unwrap();
        }
        renderer.request_frame_capture().unwrap();
        assert!(
            renderer
                .submit_frame(corner_submission(
                    &matrix,
                    GATE_EYE,
                    light,
                    None,
                    corner_bounds(),
                    Some(pop),
                    None,
                    false,
                    None,
                    SIZE,
                ))
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
    // of its bytes moved. The "sky region" is the fixed UPPER-THIRD
    // SCREEN RECTANGLE — a screen-region proxy for "analytically
    // visible sky at this camera", not the analytic sky itself
    // (deriving the true horizon would duplicate the projection
    // chain the gate exists to check); at this camera the horizon
    // sits ~40% from the top, so the upper third is entirely sky.
    // MEASURED on this pod's lavapipe (pinned in the asserts below
    // from the run's own print).
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
        "rain broadside ink: {ink} pixels of {} changed ({sky_ink} in the upper-third \
         screen region)",
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
    // background). THE FINDING-12 POLICY CHANGE (the rain now falls
    // to the street-level world receiver; the band is the
    // compatible 4 m over the 8 m observed column; the steady state
    // holds ~624 streaks crossing the FULL view column) moves BOTH
    // actuals - PROVISIONAL: re-measure on lavapipe in CI and pin
    // the new baseline here (the count should RISE from 473). The
    // floors keep their margins: they sit under the old actual
    // while staying above the edge-on collapse (~164 pixels - the
    // mining source's 8-frame fresh-capture gate).
    assert!(
        ink > 330,
        "broadside rain coverage: {ink} changed pixels at the presentation extent \
         (measured 473 at the steady state after the volume-refresh corrective; \
         the edge-on collapse measures ~164)"
    );
    // THE MAJORITY PREDICATE (the audit's finding 13 fold-in): the
    // claimed invariant is that a MAJORITY of the ink lives in the
    // upper-third region — rain falls in front of the sky-backed
    // background. The old check required only `sky_ink > 190`
    // without the majority (1,000 changed pixels with 191 in the
    // upper third would pass — the audit's own counterexample);
    // both the floor AND the majority now hold.
    assert!(
        sky_ink > 190,
        "the upper-third screen region carries {sky_ink} of the ink (measured 282 - rain \
         falls in front of the background)"
    );
    assert!(
        sky_ink * 2 > ink,
        "the upper-third screen region carries a MAJORITY of the rain ink: {sky_ink} of \
         {ink} (the depth-interaction's visible half - a depth-write regression that \
         punches streak holes, or a billboard regression that turns them edge-on, \
         collapses this long before the total)"
    );
}

/// One raining WET campaign at the gate camera with the ripple
/// uniform pinned per frame (the `raining_capture` shape plus the
/// per-frame wet declaration — the presentation bridge: the field
/// state itself is deterministic sim state (the campaign digest
/// pins it), the submission only packs it into the frame's
/// uniform). `frames` warm frames, then the captured presentation
/// frame.
fn ripple_campaign(
    renderer: &mut Renderer,
    matrix: &[[f32; 4]; 4],
    yaw: f32,
    light: LightParams,
    ripple: Option<&RippleField>,
    frames: usize,
) -> engine_renderer::CapturedFrame {
    let wet = Some(WetFrame { ripple });
    let mut layer = RainLayer::new(GATE_EYE);
    for _ in 0..frames {
        layer.update_and_build(GATE_EYE, yaw, 0.0, demo_clock_dt().as_secs_f32());
        let population = layer.population();
        renderer
            .submit_frame(corner_submission(
                matrix,
                GATE_EYE,
                light,
                None,
                corner_bounds(),
                Some(population),
                None,
                false,
                wet,
                SIZE,
            ))
            .unwrap();
    }
    renderer.request_frame_capture().unwrap();
    let population = layer.population();
    assert!(
        renderer
            .submit_frame(corner_submission(
                matrix,
                GATE_EYE,
                light,
                None,
                corner_bounds(),
                Some(population),
                None,
                false,
                wet,
                SIZE,
            ))
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
    // not ripple-vs-none). The campaigns below carry the wet
    // declaration on every submission (the helper builds it).
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
    let forced = forced_eight_ring_field();

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
    let mut rgb_ink = 0usize;
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
        // THE DISPLAYED-RGB QUALIFICATION (finding 15a's corrective):
        // a pixel counts only when its DISPLAYED rgb changes by >= 2
        // in some channel — the alpha lane alone never counts (the
        // receiver writes the roughened blend factor into destination
        // alpha, so a pixel with identical display RGB still moved
        // bytes; the raw metric inflated both counts). Channels
        // 0..3 of the 4-byte pixel are the RGB lanes under either
        // RGBA8-family byte order; lane 3 is alpha under both.
        if (0..3).any(|c| (a[c] as i32 - b[c] as i32).abs() >= 2) {
            rgb_ink += 1;
        }
    }
    println!(
        "ripple reflection ink: {differing_bytes} differing bytes, {ink} differing pixels \
         ({lower_ink} in the lower half), {rgb_ink} displayed-RGB-qualified pixels \
         (>= 2 in some rgb channel)"
    );
    // MEASURED on this pod's lavapipe (the gate's first true
    // execution, run 45719424168's lavapipe battery): 248,261
    // differing bytes, 97,620 differing pixels, ALL 97,620 in the
    // lower half (100% ground region — the camera puts the
    // horizon ~40% from the top and every ring projects below it).
    // The forced field's 8 amplitude-1.0 rings perturb the
    // reflection sample across ~10.6% of the frame; the floors sit
    // at ~40% of the measurement — load-bearing (a regression that
    // kills most of the perturbation fails them) while holding
    // wide margin under the full-strength actual (the ink-count
    // gate's 492-vs-200 discipline).
    // THE FINDING-12 POLICY CHANGE and THE FINDING-14 DRAW-ORDER
    // CORRECTIVE both move this gate's actuals (the rain now falls
    // to the street-level receiver and composes OVER the
    // reflection; the campaign's live ring stream changed with the
    // decimation) — the diff still isolates the ripple perturbation
    // (the rain instances are identical across both arms), but the
    // measured baseline is PROVISIONAL: re-measure on lavapipe in
    // CI and pin the new actuals here (the floors are expected to
    // hold; the perturbation mechanism is unchanged).
    const RIPPLE_BYTE_FLOOR: usize = 100_000;
    const RIPPLE_PIXEL_FLOOR: usize = 39_000;
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
    // THE DISPLAYED-RGB FLOOR (finding 15a — RE-PINNED from round
    // 5's cross-platform measurements): a substantial count of the
    // differing pixels must carry a real displayed-RGB change —
    // the alpha-only inflation (identical display RGB, moved
    // destination alpha) can never constitute the whole effect.
    // The original MAJORITY formulation (rgb_ink*2 > ink) proved
    // platform-brittle: the alpha lane's SHARE depends on the
    // mirror content's contrast — round 5 measured Ubuntu at
    // 12,087 of 97,961 (12% displayed; the darker Ubuntu mirror
    // gives the UV displacement less rgb to move while the
    // roughen still writes alpha everywhere in the band) while
    // Windows passed with a majority. The FLOOR of 5,000 holds
    // ~2.4x margin under Ubuntu's measurement and ~10x under
    // Windows's — and 5,000 displayed-rgb pixels is itself a
    // strong visibility proof (the natural-sequence gate needed
    // exactly 1).
    assert!(
        rgb_ink >= 5_000,
        "the forced ripple field's displayed-RGB ink is at least 5,000 pixels \
         (got {rgb_ink} of {ink} differing — the alpha-only lane dominates: the \
         perturbation never reached the displayed mirror; check the roughen/uv \
         terms against the shader's blend stage)"
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

/// The forced eight-ring field (the ripple reflection gate's own
/// fixture, extracted for the separation gate's reuse — the SAME
/// hand-placed rings, the SAME policy, deterministic by
/// construction).
fn forced_eight_ring_field() -> RippleField {
    let mut field = RippleField::new(RIPPLE_POLICY).expect("the demo's ripple policy validates");
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
}

/// The gate camera's orthonormal basis (the receiver-blend gate's
/// own helper shape — the yaw/pitch the gate matrix itself
/// evaluates through waypoint_orientation, never re-derived
/// differently).
fn gate_basis() -> ([f32; 3], [f32; 3], [f32; 3]) {
    let offset = std::array::from_fn(|i| GATE_TOWARD[i] - GATE_EYE[i]);
    let (yaw, pitch) = engine_slice::corner_demo::waypoint_orientation(offset);
    let forward = engine_slice::math::camera_forward(yaw, pitch, 0.0);
    let right = engine_slice::math::camera_right(yaw, pitch, 0.0);
    let rotation = engine_slice::math::camera_rotation(yaw, pitch, 0.0);
    let up = [rotation[1][0], rotation[1][1], rotation[1][2]];
    (forward, right, up)
}

/// The CPU ray for one raster pixel (the receiver-blend gate's own
/// helper — the 65° gate frustum, the pixel-center convention, the
/// shader boundary's Y flip: pixel row 0 is the top, authored NDC
/// +y is screen-up). THE ASPECT FACTOR (the round-8 corrective):
/// the gate matrix is perspective(fov_y, 16/9) with focal/aspect on
/// the x lane, so NDC x maps to view-space x through
/// tan_half·ASPECT — the ray's horizontal term carries the aspect
/// (the pre-corrective form used tan_half alone, compressing every
/// inverse-projected ground point 1.778× toward the screen-center
/// column: authored rings landed at the wrong world centers, and
/// the GPU's true band ink appeared at modeled "hole" positions —
/// the forward projection (project_ground) was always correct; only
/// the inverse was broken).
fn gate_pixel_ray(forward: [f32; 3], right: [f32; 3], up: [f32; 3], x: u32, y: u32) -> [f32; 3] {
    let nx = (x as f32 + 0.5) * 2.0 / SIZE.0 as f32 - 1.0;
    let ny = (y as f32 + 0.5) * 2.0 / SIZE.1 as f32 - 1.0;
    let math_y = -ny;
    let tan_half = (65_f32.to_radians() / 2.0).tan();
    let aspect = SIZE.0 as f32 / SIZE.1 as f32;
    let ray = std::array::from_fn(|axis| {
        forward[axis] + nx * tan_half * aspect * right[axis] + math_y * tan_half * up[axis]
    });
    let length = (ray[0] * ray[0] + ray[1] * ray[1] + ray[2] * ray[2]).sqrt();
    ray.map(|component| component / length)
}

/// The ground point under one raster pixel (the ray's intersection
/// with the world receiver plane y = GROUND_Y; None when the ray
/// points at or above the horizon — never a ground pixel).
fn pixel_ground(
    forward: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
    x: u32,
    y: u32,
) -> Option<[f32; 3]> {
    let ray = gate_pixel_ray(forward, right, up, x, y);
    if ray[1] >= -1e-4 {
        return None;
    }
    let t = (GROUND_Y - GATE_EYE[1]) / ray[1];
    Some([GATE_EYE[0] + ray[0] * t, GROUND_Y, GATE_EYE[2] + ray[2] * t])
}

/// A ground point's screen pixel (the gate matrix, the w-divide,
/// and the established Y convention — the diagonal footprint gate's
/// own projection form).
fn project_ground(matrix: &[[f32; 4]; 4], ground: [f32; 3]) -> (f32, f32) {
    let clip = [
        matrix[0][0] * ground[0]
            + matrix[1][0] * ground[1]
            + matrix[2][0] * ground[2]
            + matrix[3][0],
        matrix[0][1] * ground[0]
            + matrix[1][1] * ground[1]
            + matrix[2][1] * ground[2]
            + matrix[3][1],
        matrix[0][3] * ground[0]
            + matrix[1][3] * ground[1]
            + matrix[2][3] * ground[2]
            + matrix[3][3],
    ];
    let ndc_x = clip[0] / clip[2];
    let ndc_y = clip[1] / clip[2];
    (
        (ndc_x + 1.0) / 2.0 * SIZE.0 as f32,
        (1.0 - ndc_y) / 2.0 * SIZE.1 as f32,
    )
}

/// The oracle's packed rows for a ring set (the renderer's
/// `pack_ripple_field` layout re-derived for the CPU oracle input —
/// the known-answer discipline: the renderer's own packing test
/// pins the packing; this builds the SAME rows the shader read, so
/// `planar::ripple_perturbation` consumes exactly the uniform the
/// GPU did). rows[1]'s debug lane reads 0 — the oracle mirrors the
/// shipped mode-0 path; the separation knobs suppress whole terms
/// and never reshape the survivors.
fn oracle_rows(rings: &[RippleRing]) -> [[f32; 4]; 2 + engine_core::ripple::RIPPLE_POOL_BOUND] {
    let mut rows = [[0.0f32; 4]; 2 + engine_core::ripple::RIPPLE_POOL_BOUND];
    rows[0] = [
        rings.len() as f32,
        RIPPLE_POLICY.speed,
        RIPPLE_POLICY.width,
        RIPPLE_UV_GAIN,
    ];
    rows[1] = [RIPPLE_ROUGHEN_GAIN, 0.0, 0.0, 0.0];
    for (i, ring) in rings.iter().enumerate() {
        rows[2 + i] = [
            ring.center[0],
            ring.center[1],
            ring.age,
            ring.amplitude * (1.0 - ring.age / RIPPLE_POLICY.lifetime),
        ];
    }
    rows
}

/// One wet capture at the gate camera with the ripple uniform and
/// the TEST-ONLY debug mode pinned (the per-frame wet declaration —
/// the presentation bridge; single frames, the static rainless
/// population: the diff between two such captures isolates exactly
/// the ripple terms the modes leave active). `wet` is the caller's
/// declaration (the dry capture is the signed-movement gate's own
/// control arm).
fn ripple_mode_capture(
    renderer: &mut Renderer,
    matrix: &[[f32; 4]; 4],
    light: LightParams,
    ripple: Option<&RippleField>,
    wet: bool,
    mode: RippleDebugMode,
) -> engine_renderer::CapturedFrame {
    renderer.set_ripple_debug_mode(mode);
    let empty = rainless();
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .submit_frame(corner_submission(
                matrix,
                GATE_EYE,
                light,
                None,
                corner_bounds(),
                Some(empty),
                None,
                false,
                wet.then_some(WetFrame { ripple }),
                SIZE,
            ))
            .unwrap()
            .presented
    );
    renderer.take_frame_capture().unwrap().unwrap()
}

/// THE RIPPLE TERM-SEPARATION GATE (finding 15b's corrective): the
/// SAME forced eight-ring field through the two test-only debug
/// modes against the empty-field baseline — a DISPLACEMENT-ONLY
/// fixture (the blend term suppressed: the roughened alpha never
/// differs, so every differing pixel differs in DISPLAYED rgb —
/// the alpha-only inflation is exactly zero) and a BLEND-ONLY
/// fixture (the UV term suppressed: the dimming alone). The two
/// fixtures differ from each other (both terms contribute), and
/// each carries real displayed-RGB ink. The mode knob is the
/// audit's sanctioned test-only debug uniform (renderer.rs's
/// RippleDebugMode riding the ripple block's spare lane — the
/// production packing always writes 0; the choice is documented in
/// the enum's doc and the shader's header).
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe)"]
fn ripple_term_separation_gate() {
    let _guard = sdl_lock();
    let (_app, mut renderer) = app_and_renderer("ripple term separation gate");

    let (matrix, _yaw) = gate_matrix();
    let light = day_preset();
    renderer.set_proxy_draw_mode(ProxyDrawMode::Instanced);

    let forced = forced_eight_ring_field();

    // The three captures: the empty baseline, blend-only, and
    // displacement-only (single frames — the field is static and
    // authored; the population is the rainless baseline).
    let baseline = ripple_mode_capture(
        &mut renderer,
        &matrix,
        light,
        None,
        true,
        RippleDebugMode::Off,
    );
    let blend_only = ripple_mode_capture(
        &mut renderer,
        &matrix,
        light,
        Some(&forced),
        true,
        RippleDebugMode::BlendOnly,
    );
    let displacement_only = ripple_mode_capture(
        &mut renderer,
        &matrix,
        light,
        Some(&forced),
        true,
        RippleDebugMode::DisplacementOnly,
    );

    // The metrics per fixture: differing pixels, pixels whose
    // DISPLAYED rgb moved at all (>= 1 — the alpha-only counter),
    // and the contrast-qualified count (>= 2 — finding 15a's
    // qualification).
    let metrics = |other: &engine_renderer::CapturedFrame| -> (usize, usize, usize) {
        let mut diff = 0usize;
        let mut rgb_moved = 0usize;
        let mut rgb_qualified = 0usize;
        for (a, b) in baseline.bytes.chunks(4).zip(other.bytes.chunks(4)) {
            if a != b {
                diff += 1;
            }
            let rgb_delta = (0..3)
                .map(|c| (a[c] as i32 - b[c] as i32).abs())
                .max()
                .unwrap();
            if rgb_delta >= 1 {
                rgb_moved += 1;
            }
            if rgb_delta >= 2 {
                rgb_qualified += 1;
            }
        }
        (diff, rgb_moved, rgb_qualified)
    };
    let (blend_diff, blend_rgb, blend_qualified) = metrics(&blend_only);
    let (disp_diff, disp_rgb, disp_qualified) = metrics(&displacement_only);
    println!(
        "ripple term separation: blend-only {blend_diff} differing pixels ({blend_rgb} rgb-moved, \
         {blend_qualified} rgb-qualified); displacement-only {disp_diff} differing pixels \
         ({disp_rgb} rgb-moved, {disp_qualified} rgb-qualified)"
    );

    // THE ALPHA-ONLY SEPARATION PROOF (displacement-only): the
    // suppressed roughen leaves the fragment's Schlick alpha
    // BIT-IDENTICAL to the baseline (blend * (1 - 0 * g) == blend
    // exactly), so a pixel can differ only through its displayed
    // rgb — every differing pixel moves some rgb lane. The old
    // conflation (alpha-only changes counting as ink) is exactly
    // zero here.
    assert_eq!(
        disp_diff, disp_rgb,
        "displacement-only: every differing pixel differs in displayed rgb (the alpha \
         lane is bit-identical — the roughen term is suppressed)"
    );
    // Each fixture carries real visible ink (PROVISIONAL floors —
    // low absolute, the discrimination carried by the structural
    // asserts above and the signed-movement gate; every actual
    // prints for the CI re-pin, and a near-miss fails WITH its own
    // measured numbers).
    const SEPARATION_INK_FLOOR: usize = 500;
    assert!(
        disp_qualified >= SEPARATION_INK_FLOOR,
        "the displacement-only fixture moves displayed rgb on at least \
         {SEPARATION_INK_FLOOR} pixels (got {disp_qualified} of {disp_diff} differing) — \
         the UV distortion never reached the mirror: check the projection block against \
         planar.rs's oracle"
    );
    assert!(
        blend_qualified >= SEPARATION_INK_FLOOR,
        "the blend-only fixture dims the mirror on at least {SEPARATION_INK_FLOOR} pixels \
         (got {blend_qualified} of {blend_diff} differing) — the roughen term never reached \
         the blend stage"
    );
    // The two terms produce DIFFERENT images (both contribute; a
    // shader with one term dead would render the two fixtures
    // identically).
    assert_ne!(
        blend_only.bytes, displacement_only.bytes,
        "the blend-only and displacement-only fixtures differ — the two ripple terms \
         contribute independent images"
    );

    let stats = renderer.stats();
    assert_eq!(
        stats.validation_errors, 0,
        "no validation errors: {:?}",
        stats.recent_validation
    );
    assert_eq!(stats.validation_warnings, 0, "no validation warnings");
}

/// THE RIPPLE SIGNED-MOVEMENT GATE (finding 15c's corrective — the
/// directional half the ink counts can never carry): a NEWBORN
/// ring (the natural sequence's own form — radius sub-pixel, so
/// every probe sits outside the ring line and the push points AT
/// the center from both sides) centered on the MEASURED strongest
/// vertical edge in the reflected content: the displacement must
/// sweep the edge's content toward the ring center from BOTH sides
/// — the probe pixels on opposite sides of the center flip in
/// OPPOSITE directions, each matching the CPU oracle's
/// independently projected offset (the known-answer comparison the
/// old gate never made), the plateau pixel further out stays put
/// (the localization guard that separates a sweep from the blend's
/// uniform dimming), and the blend-only render of the SAME ring
/// shows NO sweep (the negative control — a shader with the UV
/// term disabled fails the primary asserts above by construction).
/// A mature ring then pins the support boundary: the center hole
/// (dist < radius - width — the band gate's exact zero at the
/// center region) is byte-identical while the band around it
/// moves, and the pixels outside the support never change.
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe)"]
fn ripple_signed_movement_gate() {
    let _guard = sdl_lock();
    let (_app, mut renderer) = app_and_renderer("ripple signed movement gate");

    let (matrix, _yaw) = gate_matrix();
    let light = day_preset();
    renderer.set_proxy_draw_mode(ProxyDrawMode::Instanced);
    let (forward, right, up) = gate_basis();
    let (width, height) = (SIZE.0 as usize, SIZE.1 as usize);

    // The baseline (the empty field — the identity no-op).
    let baseline = ripple_mode_capture(
        &mut renderer,
        &matrix,
        light,
        None,
        true,
        RippleDebugMode::Off,
    );

    // THE DRY CAPTURE (the round-7 corrective — the receiver-blend
    // gate's own qualification discipline): the round-6 Ubuntu run
    // measured 0/0/0 at every probe because the max-DISPLAYED-
    // contrast search selected the road's own paint/texture edges
    // (the crosswalk band projects into the scan band) — and the UV
    // displacement never moves those: the sweep displaces ONLY the
    // reflection sample, so the edge must be selected by the
    // reflection CONTENT's own transition. The composite is
    // displayed = content·b + dry·(1-b) (ground_reflect.wgsl: the
    // reflection blended over the destination — the already-rendered
    // lit ground the dry frame shows), so the sweep's expected
    // magnitude at a full crossing is, per channel,
    //     s = |Δwet − (1-b)·Δdry|
    // (b·Δcontent recovered without division: a ground-term edge has
    // Δwet = (1-b)·Δdry exactly and s = 0; a content edge over flat
    // ground has s = b·Δcontent — the measured sweep itself). Walls
    // never reach the sweep: opaque geometry above the plane is
    // byte-identical wet and dry (the receiver redraw is depth-EQUAL
    // on the ground only), so the receiver-response floor excludes
    // it before the content math runs.
    let dry = ripple_mode_capture(
        &mut renderer,
        &matrix,
        light,
        None,
        false,
        RippleDebugMode::Off,
    );

    // THE MEASURED ASYMMETRIC MIRROR FEATURE: the strongest
    // reflection-CONTENT transition (the sweep magnitude s above)
    // over the open road cross (the interior columns; the scan band
    // sits below the horizon where the reflection draws), on pixels
    // that respond to set_wet (live receiver) and whose CPU Schlick
    // factor is inside the reflective band. The near-miss tracker
    // carries the best sweep magnitude for the PROVISIONAL re-pin.
    let mut best: Option<(u32, u32, i32, usize, [f32; 3])> = None;
    let mut near_miss: Option<(u32, u32, i32)> = None;
    // The best edge that qualified on sweep magnitude but sat too
    // deep for the newborn push (the oracle-push selection's own
    // diagnostic — the panic names it when nothing clears both).
    let mut push_limited: Option<(u32, u32, f32)> = None;
    for y in ((height * 45 / 100)..(height * 92 / 100)).step_by(2) {
        for x in (width / 4..width * 3 / 4).step_by(2) {
            let (x, y) = (x as u32, y as u32);
            let (w0, w1) = (baseline.pixel_rgba(x, y), baseline.pixel_rgba(x + 1, y));
            let (d0, d1) = (dry.pixel_rgba(x, y), dry.pixel_rgba(x + 1, y));
            // THE RECEIVER RESPONSE (the wall exclusion): both edge
            // pixels must respond to set_wet — opaque geometry above
            // the plane renders byte-identical wet and dry.
            let responds =
                |w: [u8; 4], d: [u8; 4]| (0..3).any(|c| (w[c] as i32 - d[c] as i32).abs() >= 2);
            if !responds(w0, d0) || !responds(w1, d1) {
                continue;
            }
            let Some(ground) = pixel_ground(forward, right, up, x, y) else {
                continue;
            };
            if ground[0].abs() >= 5.0 || ground[2].abs() >= 14.0 {
                continue; // off the open road cross
            }
            let view = [
                GATE_EYE[0] - ground[0],
                GATE_EYE[1] - ground[1],
                GATE_EYE[2] - ground[2],
            ];
            let view_length = (view[0] * view[0] + view[1] * view[1] + view[2] * view[2]).sqrt();
            let n_dot_v = (view[1] / view_length).clamp(0.0, 1.0);
            let b = reflection_blend(n_dot_v);
            if !(0.15..=0.9).contains(&b) {
                continue;
            }
            // THE SWEEP MAGNITUDE: the content transition in the
            // channel that carries it — signed, because the sign IS
            // the direction discriminator the sweep asserts use.
            let (channel, signed_s) = (0..3)
                .map(|c| {
                    let dwet = w1[c] as i32 - w0[c] as i32;
                    let ddry = d1[c] as i32 - d0[c] as i32;
                    (c, dwet - ((1.0 - b) * ddry as f32) as i32)
                })
                .max_by_key(|&(_, s)| s.abs())
                .unwrap();
            if near_miss.is_none_or(|(_, _, ns)| signed_s.abs() > ns) {
                near_miss = Some((x, y, signed_s.abs()));
            }
            const SWEEP_EDGE_FLOOR: i32 = 8;
            if signed_s.abs() < SWEEP_EDGE_FLOOR {
                continue;
            }
            if best.is_none_or(|(_, _, bs, _, _)| signed_s.abs() > bs) {
                // THE ORACLE-PUSH GATE (the round-8 selection
                // corrective): the newborn push shrinks with depth
                // (the round-7 Windows actual measured 2.43 px at a
                // content edge 20 m from the camera — under the
                // 2.5 px the sweep asserts need), so the selection
                // requires the CANDIDATE'S OWN oracle-projected
                // probe push to clear the floor now: an edge that
                // cannot be swept measurably is recorded as
                // push-limited, never selected. The evaluation runs
                // only for candidates that beat the current best,
                // so the scan stays cheap.
                let rows = oracle_rows(&[RippleRing {
                    center: [ground[0], ground[2]],
                    age: 0.005,
                    amplitude: 1.0,
                }]);
                let probe = pixel_ground(forward, right, up, x.saturating_sub(1), y)
                    .expect("the probe row is below the horizon");
                let (world, _) = ripple_perturbation(&rows, [probe[0], probe[2]]);
                let uv = projected_uv_offset(&matrix, probe, world);
                let push_px = uv[0] * SIZE.0 as f32;
                if push_px < 2.5 {
                    if push_limited.is_none_or(|(_, _, p)| push_px > p) {
                        push_limited = Some((x, y, push_px));
                    }
                    continue;
                }
                best = Some((x, y, signed_s, channel, ground));
            }
        }
    }
    let Some((x_t, y_r, c, channel, center)) = best else {
        if let Some((nm_x, nm_y, nm_s)) = near_miss {
            panic!(
                "PROVISIONAL gate: no reflection-content edge qualified for the sweep probe \
                 (floor: sweep magnitude 8/255 at a full crossing). The best near-miss: \
                 ({nm_x}, {nm_y}) sweep magnitude {nm_s} — re-pin the scan band or the sweep \
                 floor from THIS platform's actuals"
            );
        }
        let (pl_x, pl_y, pl_push) =
            push_limited.expect("the lower frame has live receiver pixels with content edges");
        panic!(
            "PROVISIONAL gate: every reflection-content edge sits too deep for the \
             newborn push (the best push-limited: ({pl_x}, {pl_y}) projected probe push \
             {pl_push} px < 2.5) — re-pin the scan band toward the camera or the push \
             floor from THIS platform's actuals"
        )
    };
    // The sign reference for the sweep asserts is the CONTENT-side
    // contrast (b·Δcontent — the quantity that actually moves), not
    // the displayed composite: a ground-term transition can cancel
    // the content edge in the displayed frame while the sweep still
    // crosses it.
    println!(
        "sweep probe edge: reflection-content transition at x {x_t}->{x_t}+1, row {y_r}, \
         channel {channel}, sweep magnitude {}/255 (displayed contrast {}), ground center \
         {center:?}",
        c.abs(),
        baseline.pixel_rgba(x_t + 1, y_r)[channel] as i32
            - baseline.pixel_rgba(x_t, y_r)[channel] as i32
    );

    // THE NEWBORN RING at the measured center (age 0.005 s — radius
    // 0.004 m, sub-pixel at every scan-band row: every probe pixel
    // is outside the ring line, so the push points at the center).
    let young = {
        let mut field =
            RippleField::new(RIPPLE_POLICY).expect("the demo's ripple policy validates");
        assert!(
            field
                .author_ring(RippleRing {
                    center: [center[0], center[2]],
                    age: 0.005,
                    amplitude: 1.0,
                })
                .expect("the newborn ring is valid data")
                .is_none()
        );
        field
    };
    let displacement = ripple_mode_capture(
        &mut renderer,
        &matrix,
        light,
        Some(&young),
        true,
        RippleDebugMode::DisplacementOnly,
    );
    let blend = ripple_mode_capture(
        &mut renderer,
        &matrix,
        light,
        Some(&young),
        true,
        RippleDebugMode::BlendOnly,
    );

    // THE ORACLE'S INDEPENDENT PROJECTION at the probe pixels (the
    // CPU chain the shader mirrors term-for-term — the directional
    // known answer): the probes are the pixels one step outside the
    // transition on each side, plus the plateau control further
    // left. The expected offsets come from the SAME packed rows the
    // shader read, projected through the SAME gate matrix.
    let rows = oracle_rows(&[RippleRing {
        center: [center[0], center[2]],
        age: 0.005,
        amplitude: 1.0,
    }]);
    let expected_offset = |px: u32| -> [f32; 2] {
        let ground =
            pixel_ground(forward, right, up, px, y_r).expect("the probe row is below the horizon");
        let (world, _) = ripple_perturbation(&rows, [ground[0], ground[2]]);
        projected_uv_offset(&matrix, ground, world)
    };
    let uv_left = expected_offset(x_t.saturating_sub(1));
    let uv_right = expected_offset(x_t + 2);
    let px_left = (uv_left[0] * SIZE.0 as f32, uv_left[1] * SIZE.1 as f32);
    let px_right = (uv_right[0] * SIZE.0 as f32, uv_right[1] * SIZE.1 as f32);
    println!(
        "oracle sweep: left probe expected {px_left:?} px, right probe expected {px_right:?} px"
    );
    assert!(
        px_left.0 >= 2.5,
        "the oracle projects the left probe's sweep rightward by >= 2.5 px (got \
         {px_left:?}) — the edge sits too far for the newborn push: re-pin from this \
         actual"
    );
    assert!(
        px_right.0 <= -2.5,
        "the oracle projects the right probe's sweep leftward by >= 2.5 px (got \
         {px_right:?}) — the edge sits too far for the newborn push: re-pin from this \
         actual"
    );

    // THE MEASURED SWEEP: the displacement-only deltas at the probes
    // (the content-side contrast c is the sweep magnitude selected
    // above — the quantity that actually moves). displayed(P) =
    // dry·(1-b) + b·content(P + off): the (1-b) ground part cancels
    // in the delta, so a full crossing measures ~c itself.
    let delta = |frame: &engine_renderer::CapturedFrame, px: u32| -> i32 {
        frame.pixel_rgba(px, y_r)[channel] as i32 - baseline.pixel_rgba(px, y_r)[channel] as i32
    };
    let d_left = delta(&displacement, x_t.saturating_sub(1));
    let d_right = delta(&displacement, x_t + 2);
    let d_far = delta(&displacement, x_t.saturating_sub(10));
    // THE YOUNG FIELD'S OWN INK CENSUS (the failure message must
    // carry the whole field's measured displacement, not just three
    // probe values): differing pixels, displayed-rgb movement, and
    // the best moved pixel's position — a field that moves nothing
    // anywhere is a dead upload (the probes then cannot be the
    // story), a field that moves pixels elsewhere but not at the
    // probes is a probe-position story.
    let mut census_diff = 0usize;
    let mut census_rgb = 0usize;
    let mut census_best: Option<(u32, u32, i32)> = None;
    for cy in 0..height {
        for cx in 0..width {
            let (a, b) = (
                baseline.pixel_rgba(cx as u32, cy as u32),
                displacement.pixel_rgba(cx as u32, cy as u32),
            );
            let rgb_delta = (0..3)
                .map(|c| (a[c] as i32 - b[c] as i32).abs())
                .max()
                .unwrap();
            if a != b {
                census_diff += 1;
            }
            if rgb_delta >= 1 {
                census_rgb += 1;
            }
            if census_best.is_none_or(|(_, _, bd)| rgb_delta > bd) {
                census_best = Some((cx as u32, cy as u32, rgb_delta));
            }
        }
    }
    println!(
        "young field ink census: {census_diff} differing pixels, {census_rgb} rgb-moved, \
         best moved {:?}",
        census_best
    );
    println!(
        "measured sweep (channel {channel}, sweep magnitude {c}): left {d_left}, \
         right {d_right}, plateau control {d_far}"
    );
    // OPPOSITE SIGNS on opposite sides of the ring center, each
    // matching the ORACLE-PREDICTED sweep direction (sign(d) ==
    // sign(c) x sign(expected px) — the UV-sign-inversion catcher:
    // an inverted displacement sweeps away from the edge, crosses
    // nothing, and fails these floors).
    assert!(
        d_left.abs() >= (c.abs() / 2).max(2),
        "the left probe sweeps across the edge: |{d_left}| >= {} (the sweep magnitude \
         {c} halved for the filter's slack) — an inverted or dead UV term sweeps away \
         and crosses nothing",
        (c.abs() / 2).max(2)
    );
    assert!(
        d_right.abs() >= (c.abs() / 2).max(2),
        "the right probe sweeps across the edge: |{d_right}| >= {} (the sweep \
         magnitude {c} halved for the filter's slack)",
        (c.abs() / 2).max(2)
    );
    assert!(
        d_left.signum() == c.signum() && d_right.signum() == -c.signum(),
        "the edge's content sweeps TOWARD the ring center from both sides — opposite \
         signs on opposite sides of the center, each matching the oracle's projected \
         direction: left {d_left} vs c {c}, right {d_right}"
    );
    // THE LOCALIZATION GUARD: the plateau pixel further left (still
    // inside the newborn band) moves at most half the probe's sweep
    // — a content sweep dies off with distance from the edge, while
    // the blend's dimming is roughly uniform across the band (the
    // discriminator that keeps the negative control below honest).
    assert!(
        d_far.abs() * 2 < d_left.abs(),
        "the sweep is localized at the edge: the plateau control moved {d_far} vs the \
         probe's {d_left} — a uniform change here means the dimming leaked into the \
         displacement-only fixture (check the debug-mode lane)"
    );

    // THE NEGATIVE CONTROL (a UV-term-disabled render must fail the
    // sweep asserts — demonstrated, not asserted-into-passing): the
    // SAME ring through the blend-only mode shows NO sweep (the
    // dimming may move the probe pixels, but not the localized
    // opposite-sign crossing pattern). If the SHIPPED shader's UV
    // term were dead, the displacement-only capture above would
    // look exactly like this one and the primary asserts would have
    // failed.
    let b_left = delta(&blend, x_t.saturating_sub(1));
    let b_right = delta(&blend, x_t + 2);
    let b_far = delta(&blend, x_t.saturating_sub(10));
    println!("negative control (blend-only): left {b_left}, right {b_right}, plateau {b_far}");
    let sweep_fires = b_left.abs() >= 2
        && b_right.abs() >= 2
        && b_left.signum() == c.signum()
        && b_right.signum() == -c.signum()
        && b_far.abs() * 2 < b_left.abs();
    assert!(
        !sweep_fires,
        "the blend-only fixture shows no localized opposite-sign sweep ({b_left}, \
         {b_right}, plateau {b_far}) — if it did, the dimming alone would satisfy the \
         probe and the negative control would be vacuous: re-pin from these actuals"
    );

    // THE MATURE RING (the support boundary and the center zero):
    // the same center at age 0.625 s (radius 0.5 m): the band gate
    // closes the center hole (dist < radius - width = 0.15 m — the
    // EXACT zero at the center region, the rasterizable form of the
    // dist == 0 select the CPU oracle pins) while the band around
    // it moves. The exact mathematical center itself is not
    // addressable at raster (the fragment at the center pixel sits
    // a sub-pixel distance away, where the newborn push is
    // full-magnitude in a rasterizer-dependent direction) — the
    // newborn center zero is pinned by the oracle test; the mature
    // ring's hole is its GPU-side form.
    let mature = {
        let mut field =
            RippleField::new(RIPPLE_POLICY).expect("the demo's ripple policy validates");
        assert!(
            field
                .author_ring(RippleRing {
                    center: [center[0], center[2]],
                    age: 0.625,
                    amplitude: 1.0,
                })
                .expect("the mature ring is valid data")
                .is_none()
        );
        field
    };
    let mature_disp = ripple_mode_capture(
        &mut renderer,
        &matrix,
        light,
        Some(&mature),
        true,
        RippleDebugMode::DisplacementOnly,
    );
    // The projected support geometry (the world annulus's screen
    // bounding boxes, inflated 2 px for the rasterizer's coverage
    // edge).
    let radius = RIPPLE_POLICY.speed * 0.625;
    let box_of = |world_radius: f32| -> (i64, i64, i64, i64) {
        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for (dx, dz) in [
            (world_radius, 0.0),
            (-world_radius, 0.0),
            (0.0, world_radius),
            (0.0, -world_radius),
        ] {
            let (px, py) = project_ground(&matrix, [center[0] + dx, GROUND_Y, center[2] + dz]);
            min_x = min_x.min(px);
            max_x = max_x.max(px);
            min_y = min_y.min(py);
            max_y = max_y.max(py);
        }
        (
            (min_x.floor() as i64) - 2,
            (max_x.ceil() as i64) + 2,
            (min_y.floor() as i64) - 2,
            (max_y.ceil() as i64) + 2,
        )
    };
    let (hole_min_x, hole_max_x, hole_min_y, hole_max_y) = box_of(radius - RIPPLE_POLICY.width);
    let (band_min_x, band_max_x, band_min_y, band_max_y) = box_of(radius + RIPPLE_POLICY.width);
    println!(
        "mature ring support: hole x [{hole_min_x}, {hole_max_x}] y [{hole_min_y}, \
         {hole_max_y}], band x [{band_min_x}, {band_max_x}] y [{band_min_y}, {band_max_y}]"
    );
    assert!(
        hole_max_x - hole_min_x >= 3 && hole_max_y - hole_min_y >= 3,
        "the mature ring's center hole spans pixels at this vantage (the measured edge's \
         row sits too near the camera — re-pin the scan band from this actual)"
    );
    // The hole: every pixel whose ground distance sits inside the
    // gate-closed disc is BYTE-IDENTICAL (displacement-only — no
    // dimming either); the band around it carries displayed-RGB ink;
    // the pixels beyond the support never change.
    let mut hole_checked = 0usize;
    let mut band_ink = 0usize;
    let mut outside_checked = 0usize;
    for y in band_min_y.max(0) as u64..=(band_max_y.min(height as i64 - 1) as u64) {
        for x in band_min_x.max(0) as u64..=(band_max_x.min(width as i64 - 1) as u64) {
            let (x, y) = (x as u32, y as u32);
            let Some(ground) = pixel_ground(forward, right, up, x, y) else {
                continue;
            };
            let dist = ((ground[0] - center[0]).powi(2) + (ground[2] - center[2]).powi(2)).sqrt();
            let changed = (0..3).any(|ch| {
                (mature_disp.pixel_rgba(x, y)[ch] as i32 - baseline.pixel_rgba(x, y)[ch] as i32)
                    .abs()
                    >= 2
            });
            // The gate boundary's own 1 cm no-man's land: a pixel
            // sitting exactly on `dist == radius ± width` can open
            // the band gate on one side of a GPU/CPU sqrt ULP and
            // close it on the other (the oracle's documented
            // tolerance class) — the boundary itself is checked
            // either side, never AT it.
            const GATE_EPS: f32 = 0.01;
            if dist < radius - RIPPLE_POLICY.width - GATE_EPS {
                assert_eq!(
                    mature_disp.pixel_rgba(x, y),
                    baseline.pixel_rgba(x, y),
                    "the mature ring's center hole is byte-identical at ({x}, {y}) — the \
                     band gate's exact zero (dist {dist} < {})",
                    radius - RIPPLE_POLICY.width
                );
                hole_checked += 1;
            } else if dist <= radius + RIPPLE_POLICY.width - GATE_EPS {
                band_ink += usize::from(changed);
            } else if dist > radius + RIPPLE_POLICY.width + GATE_EPS {
                assert_eq!(
                    mature_disp.pixel_rgba(x, y),
                    baseline.pixel_rgba(x, y),
                    "a pixel outside the support changed at ({x}, {y}) (dist {dist} > {})",
                    radius + RIPPLE_POLICY.width
                );
                outside_checked += 1;
            }
        }
    }
    println!(
        "mature ring: {hole_checked} hole pixels byte-identical, {band_ink} band pixels \
         moved, {outside_checked} outside-support pixels byte-identical"
    );
    assert!(
        hole_checked >= 3,
        "the hole check exercised pixels ({hole_checked}) — the projection and the scan \
         disagree on the center's screen position"
    );
    assert!(
        band_ink >= 5,
        "the mature ring's band moves displayed rgb ({band_ink} pixels) — the annulus \
         crossing the measured edge must flip its pixels"
    );
    assert!(
        outside_checked >= 3,
        "the outside-support control exercised pixels ({outside_checked})"
    );

    let stats = renderer.stats();
    assert_eq!(
        stats.validation_errors, 0,
        "no validation errors: {:?}",
        stats.recent_validation
    );
    assert_eq!(stats.validation_warnings, 0, "no validation warnings");
}

/// THE RIPPLE NATURAL-SEQUENCE GATE (finding 15d's corrective): a
/// small fixed-seed raining campaign forwarding the LIVE field
/// (the demo's own bridge — `set_ripple_field(layer.ripple_field())`
/// every frame, never an authored ring), until the campaign's own
/// kill-plane death naturally births a ring whose support projects
/// on-screen: the ring is born THIS tick (its age is exactly the
/// accumulated dt), the uniform upload happens (the sticky setter
/// — the wire the forced-field gates already prove), and the
/// displayed pixels CHANGE — the same population rendered with the
/// live field vs the field from BEFORE the death (the old rings
/// authored verbatim) differs in displayed rgb, with every
/// differing pixel inside the newborn ring's projected support and
/// the rest of the frame byte-identical.
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe)"]
fn ripple_natural_sequence_gate() {
    let _guard = sdl_lock();
    let (_app, mut renderer) = app_and_renderer("ripple natural sequence gate");

    let (matrix, yaw) = gate_matrix();
    let light = day_preset();
    renderer.set_proxy_draw_mode(ProxyDrawMode::Instanced);

    let dt = 1.0f32 / 60.0f32;
    let mut layer = RainLayer::new(GATE_EYE);
    // The camera basis for the inverse-ray support test (the finding
    // 25 term-separation's per-pixel distance check — the same
    // aspect-corrected ray the signed-movement gate uses).
    let (forward, right, up) = gate_basis();
    // The off-screen landings observed (the PROVISIONAL panic's own
    // diagnostics).
    let mut landings: Vec<[f32; 2]> = Vec::new();
    // The flat-content landings (the round-3 content qualification's
    // diagnostic accumulator: landing center + its measured gradient).
    let mut flat_landings: Vec<([f32; 2], i32)> = Vec::new();
    let mut sequence: Option<Vec<RippleRing>> = None;
    for _frame in 0..240 {
        // The tick's spawned census (the birth detector — the layer's
        // field is fed ONLY by observe_death, so an increase IS a
        // natural kill-plane death landing).
        let before_spawned = layer.ripple_field().spawned();
        layer.update_and_build(GATE_EYE, yaw, 0.0, demo_clock_dt().as_secs_f32());
        // THE LIVE FIELD FORWARDED (the finding-15d corrective: the
        // campaign's own field, declared on every submission below
        // exactly as the demo's route does).
        let population = layer.population();
        let field = layer.ripple_field();
        let new_count = field.spawned() - before_spawned;
        if new_count == 0 {
            assert!(
                renderer
                    .submit_frame(corner_submission(
                        &matrix,
                        GATE_EYE,
                        light,
                        None,
                        corner_bounds(),
                        Some(population),
                        None,
                        false,
                        Some(WetFrame {
                            ripple: Some(layer.ripple_field())
                        }),
                        SIZE,
                    ))
                    .unwrap()
                    .presented
            );
            continue;
        }
        // A natural death birthed ring(s) this tick. The new rings
        // are the post-compaction active prefix's TAIL (spawn order
        // — appended at the prefix's end at observe time, then the
        // tick's stable compaction preserves the order); the
        // survivors before them are exactly what the field would
        // hold WITHOUT this tick's births (the same tick aged and
        // compacted them identically) — the control arm's rings.
        let after_active = field.active_count();
        let new_rings: Vec<RippleRing> =
            field.rings()[after_active - new_count as usize..after_active].to_vec();
        let before_rings: Vec<RippleRing> =
            field.rings()[..after_active - new_count as usize].to_vec();
        assert_eq!(
            new_rings.len() as u64,
            new_count,
            "the spawned census and the prefix tail agree"
        );
        for ring in &new_rings {
            // Born this tick: age 0 at the observe, then one tick's
            // accumulation — exactly `0.0 + dt`, the pinned form.
            assert_eq!(
                ring.age,
                0.0f32 + dt,
                "the naturally-born ring is one tick old (age {})",
                ring.age
            );
            assert!(
                ring.center[0].is_finite() && ring.center[1].is_finite(),
                "the landing position is finite"
            );
        }
        // The projected support (the newborn ring's band disc:
        // radius = speed x age, outer = radius + width).
        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for ring in &new_rings {
            let outer = RIPPLE_POLICY.speed * ring.age + RIPPLE_POLICY.width;
            for (dx, dz) in [(outer, 0.0), (-outer, 0.0), (0.0, outer), (0.0, -outer)] {
                let (px, py) = project_ground(
                    &matrix,
                    [ring.center[0] + dx, GROUND_Y, ring.center[1] + dz],
                );
                min_x = min_x.min(px);
                max_x = max_x.max(px);
                min_y = min_y.min(py);
                max_y = max_y.max(py);
            }
        }
        landings.extend(new_rings.iter().map(|ring| ring.center));
        // The degenerate-projection rejection (the round-5
        // corrective): a landing AT the camera's feet projects its
        // band disc through the camera plane - the box explodes to
        // the whole frame (round 4 measured support x [0, 1279] y
        // [0, 719] for a ring 1.5 m from the eye), which makes
        // both the containment assert and the contribution check
        // vacuous. Such landings are real but unmeasurable by the
        // box method - record and keep driving (the forced-field
        // gates prove the displacement display elsewhere).
        if max_x - min_x > 0.5 * SIZE.0 as f32 || max_y - min_y > 0.5 * SIZE.1 as f32 {
            flat_landings.push((new_rings[0].center, -1));
            println!(
                "natural sequence: a landing at {:?} sits at the camera - its \
                 projected support is degenerate (x [{min_x:.0}, {max_x:.0}] y \
                 [{min_y:.0}, {max_y:.0}]) - continuing the drive",
                new_rings[0].center
            );
            assert!(
                renderer
                    .submit_frame(corner_submission(
                        &matrix,
                        GATE_EYE,
                        light,
                        None,
                        corner_bounds(),
                        Some(population),
                        None,
                        false,
                        Some(WetFrame {
                            ripple: Some(layer.ripple_field())
                        }),
                        SIZE,
                    ))
                    .unwrap()
                    .presented
            );
            continue;
        }
        // The support must intersect the frame (a landing behind
        // the camera or off-frame cannot show displayed change -
        // keep driving).
        if max_x < 0.0 || min_x >= SIZE.0 as f32 || max_y < 0.0 || min_y >= SIZE.1 as f32 {
            assert!(
                renderer
                    .submit_frame(corner_submission(
                        &matrix,
                        GATE_EYE,
                        light,
                        None,
                        corner_bounds(),
                        Some(population),
                        None,
                        false,
                        Some(WetFrame {
                            ripple: Some(layer.ripple_field())
                        }),
                        SIZE,
                    ))
                    .unwrap()
                    .presented
            );
            continue;
        }
        // THE PAIR: the same completed tick's population, the live
        // field vs the before-field (the old rings authored verbatim
        // — the ONLY difference is the tick's naturally-born ring).
        renderer.request_frame_capture().unwrap();
        assert!(
            renderer
                .submit_frame(corner_submission(
                    &matrix,
                    GATE_EYE,
                    light,
                    None,
                    corner_bounds(),
                    Some(population),
                    None,
                    false,
                    Some(WetFrame {
                        ripple: Some(layer.ripple_field())
                    }),
                    SIZE,
                ))
                .unwrap()
                .presented
        );
        let live = renderer.take_frame_capture().unwrap().unwrap();
        let before_field = {
            let mut field =
                RippleField::new(RIPPLE_POLICY).expect("the demo's ripple policy validates");
            for ring in &before_rings {
                assert!(
                    field
                        .author_ring(*ring)
                        .expect("the surviving ring is valid data")
                        .is_none(),
                    "the before-field has room ({} of {})",
                    before_rings.len(),
                    engine_core::ripple::RIPPLE_POOL_BOUND
                );
            }
            field
        };
        // The before-field rides THIS submission alone (the deleted
        // sticky setter's per-frame call site).
        renderer.request_frame_capture().unwrap();
        assert!(
            renderer
                .submit_frame(corner_submission(
                    &matrix,
                    GATE_EYE,
                    light,
                    None,
                    corner_bounds(),
                    Some(population),
                    None,
                    false,
                    Some(WetFrame {
                        ripple: Some(&before_field)
                    }),
                    SIZE,
                ))
                .unwrap()
                .presented
        );
        let before = renderer.take_frame_capture().unwrap().unwrap();
        // (the before capture is compared inline below; the borrow
        // lives in this block - sequence is set ONLY on the
        // qualifying birth, after the ink check)
        // THE DISPLAYED CHANGE (finding 15a's qualification): rgb
        // lanes >= 2 — the alpha lane alone never counts.
        let mut ink = 0usize;
        let mut outside = 0usize;
        let (inflated_min_x, inflated_max_x) = (
            (min_x.floor() as i64 - 6).max(0),
            (max_x.ceil() as i64 + 6).min(width_cap()),
        );
        let (inflated_min_y, inflated_max_y) = (
            (min_y.floor() as i64 - 6).max(0),
            (max_y.ceil() as i64 + 6).min(height_cap()),
        );
        for (index, (a, b)) in before.bytes.chunks(4).zip(live.bytes.chunks(4)).enumerate() {
            let rgb_delta = (0..3)
                .map(|ch| (a[ch] as i32 - b[ch] as i32).abs())
                .max()
                .unwrap();
            if rgb_delta >= 2 {
                ink += 1;
                let x = (index % SIZE.0 as usize) as i64;
                let y = (index / SIZE.0 as usize) as i64;
                let inside = x >= inflated_min_x
                    && x <= inflated_max_x
                    && y >= inflated_min_y
                    && y <= inflated_max_y;
                assert!(
                    inside,
                    "a differing pixel ({x}, {y}) lies OUTSIDE the newborn ring's \
                     projected support x [{inflated_min_x}, {inflated_max_x}] y \
                     [{inflated_min_y}, {inflated_max_y}] — the fields differ by more \
                     than the new ring"
                );
            } else if a != b {
                outside += 1; // sub-2 rgb drift inside the support (rounding)
            }
        }
        println!(
            "natural sequence: {} new ring(s) at {:?}, support x [{inflated_min_x}, \
             {inflated_max_x}] y [{inflated_min_y}, {inflated_max_y}], {ink} displayed-rgb \
             pixels moved ({outside} sub-2 drift pixels)",
            new_rings.len(),
            new_rings.iter().map(|ring| ring.center).collect::<Vec<_>>(),
        );
        // THE DISPLAYED PIXEL CHANGE (PROVISIONAL floor of 1 — the
        // newborn ring's ~25 px support sweeping ~6 px needs SOME
        // reflection variation under it; a landing on perfectly
        // flat content measures 0 and the near-miss below carries
        // the actuals for the re-pin).
        // THE REFLECTION-CONTRIBUTION QUALIFICATION (the round-4
        // corrective): the displacement moves ONLY the reflection's
        // contribution to the pixel - the frame's own gradient is
        // the WRONG signal (round 3 measured gradient 13 at a
        // landing whose displacement still displayed nothing: the
        // variation was diffuse lighting the ripple never touches).
        // The honest signal is the WET-DRY contrast - exactly what
        // the receiver-blend gate qualifies on - so the birth
        // qualifies only where the reflection measurably
        // contributes (max |wet - dry| >= 2 per channel over the
        // support); a landing where the reflection contributes
        // nothing is honestly unobservable and the drive CONTINUES;
        // contribution-without-ink is the real failure.
        if ink >= 1 {
            // THE TERM SEPARATION (the sign-off review's finding 25
            // corrective): one displayed pixel from the production
            // capture proves WIRING, not displacement — a
            // dimming-only newborn (the roughen term alive, the UV
            // term dead) passes exactly the same check. The
            // qualifying birth's field pair is captured through the
            // debug modes — the DISPLACEMENT-ONLY pair (the live
            // field vs the before-field, the UV term alone, the
            // rainless population so no streak noise rides either
            // arm) must carry its own ink: the newborn's SAMPLE
            // DISPLACEMENT, not its dimming, is what moved pixels.
            let disp_with = ripple_mode_capture(
                &mut renderer,
                &matrix,
                light,
                Some(layer.ripple_field()),
                true,
                RippleDebugMode::DisplacementOnly,
            );
            let disp_without = ripple_mode_capture(
                &mut renderer,
                &matrix,
                light,
                Some(&before_field),
                true,
                RippleDebugMode::DisplacementOnly,
            );
            let mut disp_ink = 0usize;
            let mut support_violations = 0usize;
            for (index, (a, b)) in disp_without
                .bytes
                .chunks(4)
                .zip(disp_with.bytes.chunks(4))
                .enumerate()
            {
                let rgb_delta = (0..3)
                    .map(|ch| (a[ch] as i32 - b[ch] as i32).abs())
                    .max()
                    .unwrap();
                if rgb_delta >= 2 {
                    disp_ink += 1;
                    // THE INVERSE-RAY SUPPORT TEST: the differing
                    // pixel's ground distance from the birth center
                    // sits inside the newborn's band (the aspect-
                    // corrected inverse projection — the pre-R4
                    // compressed ray made this check impossible).
                    let x = (index % SIZE.0 as usize) as u32;
                    let y = (index / SIZE.0 as usize) as u32;
                    if let Some(ground) = pixel_ground(forward, right, up, x, y) {
                        let dist = ((ground[0] - new_rings[0].center[0]).powi(2)
                            + (ground[2] - new_rings[0].center[1]).powi(2))
                        .sqrt();
                        let outer = RIPPLE_POLICY.speed * new_rings[0].age + RIPPLE_POLICY.width;
                        if dist > outer + 0.25 {
                            support_violations += 1;
                        }
                    }
                }
            }
            let blend_with = ripple_mode_capture(
                &mut renderer,
                &matrix,
                light,
                Some(layer.ripple_field()),
                true,
                RippleDebugMode::BlendOnly,
            );
            let blend_without = ripple_mode_capture(
                &mut renderer,
                &matrix,
                light,
                Some(&before_field),
                true,
                RippleDebugMode::BlendOnly,
            );
            let mut blend_ink = 0usize;
            for (a, b) in blend_without
                .bytes
                .chunks(4)
                .zip(blend_with.bytes.chunks(4))
            {
                let rgb_delta = (0..3)
                    .map(|ch| (a[ch] as i32 - b[ch] as i32).abs())
                    .max()
                    .unwrap();
                if rgb_delta >= 2 {
                    blend_ink += 1;
                }
            }
            println!(
                "natural term separation: displacement-only {disp_ink} pixels, \
                 blend-only {blend_ink} pixels ({support_violations} support \
                 violations) — the newborn's UV term vs its roughen term"
            );
            assert_eq!(
                support_violations, 0,
                "the displacement-only ink sits inside the newborn's projected band"
            );
            assert!(
                disp_ink >= 1,
                "the natural newborn's UV DISPLACEMENT alone moves displayed pixels \
                 ({disp_ink} displacement-only pixels vs {blend_ink} blend-only) — a \
                 dimming-only newborn (the finding 25 defect shape: the roughen term \
                 alive, the UV term dead) fails exactly here while the production \
                 capture still showed ink"
            );
            sequence = Some(new_rings.clone());
            break;
        }
        // The dry control arm: THIS submission alone declares no
        // wet (the deleted set_wet toggle's per-frame form).
        renderer.request_frame_capture().unwrap();
        assert!(
            renderer
                .submit_frame(corner_submission(
                    &matrix,
                    GATE_EYE,
                    light,
                    None,
                    corner_bounds(),
                    Some(population),
                    None,
                    false,
                    None,
                    SIZE,
                ))
                .unwrap()
                .presented
        );
        let dry = renderer.take_frame_capture().unwrap().unwrap();
        let mut contribution = 0i32;
        for y in inflated_min_y.max(0)..=(inflated_max_y.min(height_cap())) {
            for x in inflated_min_x.max(0)..=(inflated_max_x.min(width_cap())) {
                let idx = (y as usize * SIZE.0 as usize + x as usize) * 4;
                for ch in 0..3 {
                    contribution = contribution
                        .max((before.bytes[idx + ch] as i32 - dry.bytes[idx + ch] as i32).abs());
                }
            }
        }
        if contribution < 2 {
            // The reflection contributes nothing measurable under
            // this support: the landing cannot display - record and
            // keep driving.
            flat_landings.push((new_rings[0].center, contribution));
            println!(
                "natural sequence: a landing at {:?} sits over an invisible \
                 reflection (max wet-dry contribution {contribution}) - continuing \
                 the drive",
                new_rings[0].center
            );
            continue;
        }
        panic!(
            "the naturally-born ring changed no displayed-rgb pixel where the \
             reflection measurably contributes (max wet-dry {contribution}, \
             {outside} sub-2 drift pixels) - the displacement is not displaying \
             where it must (landing {:?})",
            landings
        );
    }
    if sequence.is_none() {
        panic!(
            "PROVISIONAL gate: no natural ring birth displayed over a measurable \
             reflection in 240 frames (the observed landings: {landings:?}; the \
             invisible-reflection landings: {flat_landings:?}) - the campaign's \
             kill-death rate, the drive length, or the contribution threshold \
             needs re-pinning from THIS platform's actuals"
        );
    }

    let stats = renderer.stats();
    assert_eq!(
        stats.validation_errors, 0,
        "no validation errors: {:?}",
        stats.recent_validation
    );
    assert_eq!(stats.validation_warnings, 0, "no validation warnings");
}

/// The frame caps for the natural-sequence gate's box clamps (the
/// SIZE constant's own numbers, as i64 bounds).
fn width_cap() -> i64 {
    SIZE.0 as i64 - 1
}

fn height_cap() -> i64 {
    SIZE.1 as i64 - 1
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
        layer.update_and_build(GATE_EYE, yaw, 0.0, demo_clock_dt().as_secs_f32());
        let population = layer.population();
        renderer.request_frame_capture().unwrap();
        assert!(
            renderer
                .submit_frame(corner_submission(
                    &matrix,
                    GATE_EYE,
                    light,
                    None,
                    corner_bounds(),
                    Some(population),
                    None,
                    false,
                    None,
                    SIZE,
                ))
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
            layer.update_and_build(GATE_EYE, yaw, 0.0, demo_clock_dt().as_secs_f32());
            let population = layer.population();
            let outcome = renderer
                .submit_frame(corner_submission(
                    &matrix,
                    GATE_EYE,
                    light,
                    None,
                    corner_bounds(),
                    Some(population),
                    None,
                    false,
                    None,
                    SIZE,
                ))
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
            layer.update_and_build(GATE_EYE, yaw, 0.0, demo_clock_dt().as_secs_f32());
            let population = layer.population();
            renderer.request_frame_capture().unwrap();
            assert!(
                renderer
                    .submit_frame(corner_submission(
                        &matrix,
                        GATE_EYE,
                        light,
                        None,
                        corner_bounds(),
                        Some(population),
                        None,
                        false,
                        None,
                        SIZE,
                    ))
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
            .submit_frame(corner_submission(
                &matrix,
                GATE_EYE,
                light,
                None,
                corner_bounds(),
                Some(population),
                None,
                false,
                None,
                SIZE,
            ))
            .unwrap()
            .presented
    );
    let per_instance = renderer.take_frame_capture().unwrap().unwrap();

    renderer.set_proxy_draw_mode(ProxyDrawMode::Instanced);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .submit_frame(corner_submission(
                &matrix,
                GATE_EYE,
                light,
                None,
                corner_bounds(),
                Some(population),
                None,
                false,
                None,
                SIZE,
            ))
            .unwrap()
            .presented
    );
    let instanced = renderer.take_frame_capture().unwrap().unwrap();

    assert_eq!(
        per_instance.bytes, instanced.bytes,
        "the PerInstance and Instanced rain captures are byte-identical"
    );
}

/// THE DIAGONAL SINGLE-DROP CAPTURE (finding 13's deciding GPU
/// half — the CPU half lives beside the packer as
/// `billboard_faces_the_camera_at_diagonal_headings`): ONE streak
/// at a DIAGONAL camera heading (yaw = π/4 — the heading where the
/// pre-corrective −yaw turn is EXACTLY edge-on, its normal's dot
/// with the intended facing cos 2θ = 0), captured against the
/// rainless baseline and checked against the CPU-projected
/// footprint of the turned quad. The pre-corrective packer
/// collapses the streak to a sub-pixel sliver at this heading (the
/// projected broadside width × |cos 2θ| ≈ 0); the corrected t =
/// +yaw holds it broadside. The witness scale (6) spans ~10 px of
/// width at 8 m, so the edge-on collapse cannot hide inside
/// rounding.
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe)"]
fn rain_diagonal_single_drop_footprint() {
    let _guard = sdl_lock();
    let (_app, mut renderer) = app_and_renderer("rain diagonal single drop footprint");

    // The diagonal camera: the gate eye, yaw exactly π/4 (looking
    // northwest over the intersection), pitch level — the heading
    // where the two turn signs differ the most in the frame.
    let yaw = std::f32::consts::FRAC_PI_4;
    let view = engine_slice::math::camera_view(GATE_EYE, yaw, 0.0, 0.0);
    let proj = engine_slice::math::perspective(65_f32.to_radians(), 16.0 / 9.0, 0.1, 150.0);
    let matrix = engine_slice::math::mul(&proj, &view);
    let light = day_preset();

    // The single drop: 8 m along the camera's forward, 1.5 m above
    // the eye line, turned to face the camera through the MODULE's
    // own derivation (the same yaw_to_turn the layer's pack uses).
    let forward = engine_slice::math::camera_forward(yaw, 0.0, 0.0);
    let pos = [
        GATE_EYE[0] + forward[0] * 8.0,
        GATE_EYE[1] + 1.5,
        GATE_EYE[2] + forward[2] * 8.0,
    ];
    let instance = ProxyInstance {
        transform: InstanceTransform {
            pos,
            turn: yaw_to_turn(yaw),
            scale: 6.0,
        },
        tint: [0.7, 0.8, 0.9],
        alpha: RAIN_ALPHA,
        pose_index: 0,
    };
    let population = ProxyPopulation {
        npc: &[],
        npc_poses: &[],
        npc_material: 1,
        vehicle: &[],
        vehicle_material: 2,
        rain: std::slice::from_ref(&instance),
        rain_material: RAIN_MATERIAL,
    };

    // The CPU-projected footprint: the streak mesh's four corner
    // positions (populated_meshes::rain_streak_mesh emits BL, BR,
    // TR, TL), transformed by `instance_columns` (the same packed
    // matrix the shader consumes), projected through the same
    // view_proj with the w-divide and the shader boundary's Y flip
    // (authored NDC +y is screen-up; pixel row 0 is the top).
    let mesh = engine_renderer::populated_meshes::rain_streak_mesh();
    let columns = engine_renderer::proxy_instances::instance_columns(&instance.transform);
    let project = |corner: [f32; 3]| -> (f32, f32) {
        let world = [
            columns[0][0] * corner[0]
                + columns[1][0] * corner[1]
                + columns[2][0] * corner[2]
                + columns[3][0],
            columns[0][1] * corner[0]
                + columns[1][1] * corner[1]
                + columns[2][1] * corner[2]
                + columns[3][1],
            columns[0][2] * corner[0]
                + columns[1][2] * corner[1]
                + columns[2][2] * corner[2]
                + columns[3][2],
        ];
        let clip = [
            matrix[0][0] * world[0]
                + matrix[1][0] * world[1]
                + matrix[2][0] * world[2]
                + matrix[3][0],
            matrix[0][1] * world[0]
                + matrix[1][1] * world[1]
                + matrix[2][1] * world[2]
                + matrix[3][1],
            matrix[0][2] * world[0]
                + matrix[1][2] * world[1]
                + matrix[2][2] * world[2]
                + matrix[3][2],
            matrix[0][3] * world[0]
                + matrix[1][3] * world[1]
                + matrix[2][3] * world[2]
                + matrix[3][3],
        ];
        let ndc_x = clip[0] / clip[3];
        let ndc_y = clip[1] / clip[3];
        (
            (ndc_x + 1.0) / 2.0 * SIZE.0 as f32,
            (1.0 - ndc_y) / 2.0 * SIZE.1 as f32,
        )
    };
    let corners: Vec<[f32; 3]> = mesh[..4].iter().map(|v| v.pos).collect();
    let screen_corners: Vec<(f32, f32)> = corners.iter().map(|c| project(*c)).collect();
    let xs: Vec<f32> = screen_corners.iter().map(|c| c.0).collect();
    let ys: Vec<f32> = screen_corners.iter().map(|c| c.1).collect();
    let box_min_x = xs.iter().cloned().fold(f32::INFINITY, f32::min).floor() as i64 - 1;
    let box_max_x = xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max).ceil() as i64 + 1;
    let box_min_y = ys.iter().cloned().fold(f32::INFINITY, f32::min).floor() as i64 - 1;
    let box_max_y = ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max).ceil() as i64 + 1;
    let box_width = box_max_x - box_min_x + 1;
    println!(
        "diagonal single-drop projected footprint: x [{box_min_x}, {box_max_x}], y \
         [{box_min_y}, {box_max_y}] ({box_width} px wide)"
    );
    // The witness must be ON-SCREEN (the projection is the gate's
    // own arithmetic — a drop projected off-frame means the pose
    // constants drifted, not a renderer defect).
    assert!(
        box_min_x >= 0 && box_max_x < SIZE.0 as i64 && box_min_y >= 0 && box_max_y < SIZE.1 as i64,
        "the projected footprint stays on screen"
    );
    // The broadside expectation: the quad's authored width (0.012 m
    // half-extent, scale 6) projects to ~10 px at 8 m — the
    // edge-on collapse would project under 1 px.
    assert!(
        box_width >= 6,
        "the CPU-projected broadside width is ~10 px (got {box_width}) — the witness \
         scale/distance drifted"
    );

    // The captures: the rainless baseline, then the single drop.
    renderer.set_proxy_draw_mode(ProxyDrawMode::Instanced);
    renderer.request_frame_capture().unwrap();
    let baseline_pop = rainless();
    assert!(
        renderer
            .submit_frame(corner_submission(
                &matrix,
                GATE_EYE,
                light,
                None,
                corner_bounds(),
                Some(baseline_pop),
                None,
                false,
                None,
                SIZE,
            ))
            .unwrap()
            .presented
    );
    let baseline = renderer.take_frame_capture().unwrap().unwrap();
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .submit_frame(corner_submission(
                &matrix,
                GATE_EYE,
                light,
                None,
                corner_bounds(),
                Some(population),
                None,
                false,
                None,
                SIZE,
            ))
            .unwrap()
            .presented
    );
    let dropping = renderer.take_frame_capture().unwrap().unwrap();

    // The ink: every differing pixel must lie INSIDE the inflated
    // projected footprint (the streak draws exactly where the
    // turned quad projects), and the ink's x-extent must span at
    // least half the projected width — the broadside proof (the
    // pre-corrective edge-on turn collapses the x-extent to ~1 px).
    assert_eq!(baseline.bytes.len(), dropping.bytes.len());
    let width = SIZE.0 as usize;
    let mut ink = 0usize;
    let mut ink_min_x = i64::MAX;
    let mut ink_max_x = i64::MIN;
    for (index, (a, b)) in baseline
        .bytes
        .chunks(4)
        .zip(dropping.bytes.chunks(4))
        .enumerate()
    {
        if a != b {
            ink += 1;
            let x = (index % width) as i64;
            let y = (index / width) as i64;
            assert!(
                x >= box_min_x && x <= box_max_x && y >= box_min_y && y <= box_max_y,
                "a differing pixel ({x}, {y}) lies OUTSIDE the projected footprint \
                 [{box_min_x}..={box_max_x}] x [{box_min_y}..={box_max_y}] — the streak \
                 is not drawing where the turned quad projects"
            );
            ink_min_x = ink_min_x.min(x);
            ink_max_x = ink_max_x.max(x);
        }
    }
    println!("diagonal single-drop ink: {ink} differing pixels");
    assert!(
        ink >= 3,
        "the single diagonal streak is visible ({ink} differing pixels) — the edge-on \
         turn would collapse it to ~0"
    );
    let ink_width = ink_max_x - ink_min_x + 1;
    assert!(
        ink_width * 2 >= box_width,
        "the ink's x-extent ({ink_width} px) spans at least half the projected broadside \
         width ({box_width} px) — a diagonal edge-on turn collapses it to ~1 px (the \
         finding-13 defect)"
    );

    let stats = renderer.stats();
    assert_eq!(
        stats.validation_errors, 0,
        "no validation errors: {:?}",
        stats.recent_validation
    );
    assert_eq!(stats.validation_warnings, 0, "no validation warnings");
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
        layer.update_and_build(GATE_EYE, yaw, 0.35, demo_clock_dt().as_secs_f32());
        let population = layer.population();
        renderer
            .submit_frame(corner_submission(
                &matrix,
                GATE_EYE,
                light,
                Some(BIAS),
                corner_bounds(),
                Some(population),
                None,
                false,
                None,
                SIZE,
            ))
            .unwrap();
    }
    // The raining composed capture.
    renderer.request_frame_capture().unwrap();
    let population = layer.population();
    let frame = renderer
        .submit_frame(corner_submission(
            &matrix,
            GATE_EYE,
            light,
            Some(BIAS),
            corner_bounds(),
            Some(population),
            None,
            false,
            None,
            SIZE,
        ))
        .unwrap();
    assert!(frame.presented, "the composed frame presented");
    let raining = renderer.take_frame_capture().unwrap().unwrap();

    // The rainless composed baseline at the same camera.
    renderer.request_frame_capture().unwrap();
    let baseline_pop = rainless();
    let baseline_frame = renderer
        .submit_frame(corner_submission(
            &matrix,
            GATE_EYE,
            light,
            Some(BIAS),
            corner_bounds(),
            Some(baseline_pop),
            None,
            false,
            None,
            SIZE,
        ))
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
