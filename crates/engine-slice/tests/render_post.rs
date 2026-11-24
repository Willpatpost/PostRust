//! The Era-1 post gates (see docs/CYCLE6-ERA1-BRIEF.md, package 6):
//! the tonemap/bloom equations vs `post.rs` through the REAL render
//! path — known-answer pixels at the sun-disc center (an HDR value
//! above 1, blooming), the bloom falloff ring just outside the disc
//! (the brief's "the sun disc deliberately exceeds 1.0 so it blooms"
//! pixel check), and the far corners (bloom exactly zero — the pure
//! ACES tonemap of the sky); the postless path's byte-identity
//! before and after post frames; the post state's one-time
//! allocation delta and warmed constancy; and the composition
//! smokes (the plain HDR path, the night moon, the shadowed+sky
//! chain, the populated chain).
//!
//! THE POSE: the same above-the-masses eye the sky gate's sun pose
//! uses (25 m > the 22 m tallest mass; the whole 256×256 frame is
//! pure sky — every oracle pixel is the sky equation through the
//! post chain, no mesh to model). `#[ignore]`d like the other
//! lavapipe smokes: it requires `SDL_VIDEODRIVER=offscreen` plus a
//! software Vulkan ICD (lavapipe) and the validation layer. CI's
//! Linux runner executes it enforced (the workflow's post-gate
//! step); the per-stage `println!` evidence below is what the CI
//! log must show. Run explicitly:
//!
//! ```sh
//! SDL_VIDEODRIVER=offscreen cargo test -p engine-slice --test render_post -- \
//!     --ignored --nocapture
//! ```

use engine_renderer::RendererError;
use engine_renderer::post::{
    bloom_at_pixel, bloom_mip_from_hdr, bright_pass, post_pixel, tonemap, tonemap_rgb,
};
use engine_renderer::shadows::ShadowBias;
use engine_renderer::sky::{atmosphere_at, sky_color};
use engine_renderer::{
    DemoScenePreference, DepthFormatPreference, RenderConfig, Renderer, SkyFrame, TargetFaultSite,
    TargetFaultStage, ValidationPreference,
};
use engine_shell::{DriverPreference, ShellApp, ShellConfig};

use engine_renderer::lit_scene::day_preset;
use engine_renderer::lit_scene::{LightParams, RouteWaypoint};
use engine_slice::corner_demo::view_projection;
use engine_slice::math::{camera_forward, camera_right};

/// The binary's SDL serialization (the engine-shell test pattern —
/// render_instanced's own copy). The guard is declared FIRST so it
/// drops LAST (reverse declaration order): the app and renderer
/// are fully dropped before the lock releases.
fn sdl_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}

/// One aimed camera: the corner route's own projection (65°, near
/// 0.1 / far 150) from a position toward a look-at target.
fn aimed(position: [f32; 3], toward: [f32; 3]) -> [[f32; 4]; 4] {
    view_projection(
        RouteWaypoint {
            position,
            look_at: toward,
        },
        1.0,
    )
}

/// The camera basis for a pose (the yaw/pitch the projection
/// derived): forward, right, and up (roll is zero on this path).
fn basis(position: [f32; 3], toward: [f32; 3]) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let offset = std::array::from_fn(|i| toward[i] - position[i]);
    let (yaw, pitch) = engine_slice::corner_demo::waypoint_orientation(offset);
    let forward = camera_forward(yaw, pitch, 0.0);
    let right = camera_right(yaw, pitch, 0.0);
    let rotation = engine_slice::math::camera_rotation(yaw, pitch, 0.0);
    let up = [rotation[1][0], rotation[1][1], rotation[1][2]];
    (forward, right, up)
}

/// The CPU ray for one raster pixel (the math-NDC reconstruction
/// from the camera basis — the same ray the sky pass's per-fragment
/// reconstruction produces, pinned at 0/255 by the sky gate).
fn pixel_ray(
    forward: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
    x: u32,
    y: u32,
    size: (u32, u32),
) -> [f32; 3] {
    let nx = (x as f32 + 0.5) * 2. / size.0 as f32 - 1.;
    let ny = (y as f32 + 0.5) * 2. / size.1 as f32 - 1.;
    let math_y = -ny;
    let tan_half = (65_f32.to_radians() / 2.0).tan();
    let ray = std::array::from_fn(|axis| {
        forward[axis] + nx * tan_half * right[axis] + math_y * tan_half * up[axis]
    });
    let length = (ray[0] * ray[0] + ray[1] * ray[1] + ray[2] * ray[2]).sqrt();
    ray.map(|component| component / length)
}

/// The CPU ray for one raster pixel of a NON-SQUARE frame — `pixel_ray`
/// above with the horizontal tangent carrying the frame's ASPECT
/// (perspective's matrix[0][0] = focal/aspect: a point at NDC nx lies at
/// x/(−z) = nx·aspect·tan_half in view space; the vertical term needs no
/// factor). `aimed_at_aspect` below must supply that same aspect to the
/// GPU's projection — the pair is the sky gate's off-axis block's proven
/// model (its 264×200 disc-edge probes pinned it at 0/255 on this same
/// ICD). At a square frame the factor is exactly one and this reduces to
/// `pixel_ray` bit-for-bit.
fn pixel_ray_aspected(
    forward: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
    x: u32,
    y: u32,
    size: (u32, u32),
) -> [f32; 3] {
    let nx = (x as f32 + 0.5) * 2. / size.0 as f32 - 1.;
    let ny = (y as f32 + 0.5) * 2. / size.1 as f32 - 1.;
    let math_y = -ny;
    let tan_half = (65_f32.to_radians() / 2.0).tan();
    let aspect = size.0 as f32 / size.1 as f32;
    let mut ray = std::array::from_fn(|axis| {
        forward[axis] + nx * tan_half * aspect * right[axis] + math_y * tan_half * up[axis]
    });
    let length = (ray[0] * ray[0] + ray[1] * ray[1] + ray[2] * ray[2]).sqrt();
    ray = ray.map(|component| component / length);
    ray
}

/// `aimed` with the frame's TRUE aspect — the projection the renderer
/// actually rasterizes a non-square target with (`aimed` hardcodes 1.0,
/// the square-frame special case the 256×256 gate sits on; a rectangular
/// target under it is a stretched square, NOT the non-square coverage
/// the review's rectangular-target check is about).
fn aimed_at_aspect(position: [f32; 3], toward: [f32; 3], aspect: f32) -> [[f32; 4]; 4] {
    view_projection(
        RouteWaypoint {
            position,
            look_at: toward,
        },
        aspect,
    )
}

fn quantized(color: [f32; 3]) -> [u8; 3] {
    color.map(|c| (255. * c.clamp(0., 1.)).round() as u8)
}

/// The documented tolerance: the chain is textureLoad-exact and the
/// composite's scene fetch is a 1:1 bilinear at pixel centers (a
/// pure texel read), so the remaining error budgets are the f16
/// storage quantization of the HDR target and the mip (≈0.2/255
/// per value, averaged down by the blur), the hardware bilinear's
/// accumulation order on the bloom upsample (≈1 ulp), and the
/// double normalization of the disc direction (1 ulp at the disc's
/// ramp). The sky pass itself is pinned at 0/255. The measured max
/// error over every checked pixel is printed — the pin below is
/// the ceiling with the budget documented.
const TOLERANCE: u8 = 2;

/// The every-pixel tiny-extent cells' tolerance. The full-frame walk
/// lands on each cell's SINGLE mid-ramp row — the row whose ray sits
/// inside the disc's soft edge, where the color swings the disc's full
/// additive height (~217/255 in R) over the ramp's 0.00012 dot-units
/// (a slope of ~1.8·10⁶/255 per unit dot): the residual sub-ulp dot
/// difference between the shader's inverse-matrix ray and the CPU
/// basis reconstruction — invisible everywhere else (the raw-sky
/// off-axis gate measured 1/255 at ITS ramp, PR #25; the post chain's
/// f16/ACES additions are ≤0.1/255 by analysis) — lands at 3-4/255
/// there, BOTH signs observed (+3 at 1×256's top ramp, −4 at 2×256's
/// bottom ramp). MEASURED; every non-ramp row measures 0-1/255, the
/// TOLERANCE-2 budget.
const TOLERANCE_RAMP: u8 = 4;

#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup (see render_overflow)"]
fn post_pixels_match_post_rs_and_the_disc_blooms() {
    let _guard = sdl_lock();
    let mut app = ShellApp::new(ShellConfig {
        title: "Era 1 post".into(),
        width: 256,
        height: 256,
        target_fps: 60,
        driver: DriverPreference::Real,
    })
    .unwrap();
    let mut renderer = Renderer::new(
        app.window(),
        RenderConfig {
            demo_scene: DemoScenePreference::Corner,
            validation: ValidationPreference::On,
            replace_pipelines: true,
            ..Default::default()
        },
    )
    .unwrap();
    let size = app.window().size_in_pixels();
    assert_eq!(size, (256, 256));
    assert!(renderer.stats().validation_active);
    let _ = app.frame();

    // THE PRE-POST PINS: the skyless corner's own (render_sky's —
    // the post state does not exist yet).
    let pre = renderer.allocations();
    assert_eq!(
        (
            pre.texture_memory,
            pre.descriptor_pools,
            pre.descriptor_sets,
            pre.samplers
        ),
        (4, 2, 4, 2),
        "the pre-post corner pins (render_corner's own)"
    );

    // THE UNCHANGED-PATH BASELINE: a plain corner frame, post off
    // and skyless — the byte-identity control the postless path is
    // held to below.
    let day = atmosphere_at(0.0);
    let sun_dir = day.disc_dir;
    let high_eye = [0.0, 25.0, 0.0];
    let toward_sun = std::array::from_fn(|i| high_eye[i] + sun_dir[i]);
    let matrix = aimed(high_eye, toward_sun);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, high_eye, day_preset(), size)
            .unwrap()
            .presented
    );
    let plain = renderer.take_frame_capture().unwrap().unwrap();

    // THE POST FRAMES: the sky + the chain (the disc is the bloom
    // feeder the gate pins).
    renderer.set_sky(Some(SkyFrame {
        palette: day.sky,
        disc_dir: sun_dir,
    }));
    renderer.set_post(true);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, high_eye, day_preset(), size)
            .unwrap()
            .presented
    );
    let posted = renderer.take_frame_capture().unwrap().unwrap();

    // THE POST STATE'S ONE-TIME DELTA (measured, pinned with the
    // causes): the extent-bound targets (8 images: HDR + depth +
    // two bloom mips per frame slot), the PostParams uniform, the
    // sky state's uniform, and the capture machinery's destination
    // buffer (this test's first capture, as in render_sky); the
    // bloom and post pools/sets and the linear sampler.
    let warm = renderer.allocations();
    assert_eq!(
        (
            warm.device_memory - pre.device_memory,
            warm.descriptor_pools - pre.descriptor_pools,
            warm.descriptor_sets - pre.descriptor_sets,
            warm.samplers - pre.samplers,
            warm.texture_memory - pre.texture_memory
        ),
        (11, 3, 9, 1, 0),
        "the post targets + params uniform + sky state + capture buffer, the bloom/post/sky pools and sets, the linear sampler"
    );

    // THE CPU ORACLE: the whole frame is pure sky (the pose clears
    // every mass), so the HDR image is sky_color of every pixel's
    // own ray and the chain reference runs over it exactly — the
    // same values the shader chain computes (textureLoad-exact
    // fetches, the fixed-order arithmetic post.rs pins).
    let (forward, right, up) = basis(high_eye, toward_sun);
    let mut hdr = Vec::with_capacity((size.0 * size.1) as usize);
    for y in 0..size.1 {
        for x in 0..size.0 {
            let ray = pixel_ray(forward, right, up, x, y, size);
            hdr.push(sky_color(ray, &day.sky, sun_dir));
        }
    }
    let mip = bloom_mip_from_hdr(&hdr, size);
    let oracle = |x: u32, y: u32| -> [u8; 4] {
        let hdr_texel = hdr[(y * size.0 + x) as usize];
        let bloom = bloom_at_pixel(&mip, size, (x, y));
        let presented = quantized(post_pixel(hdr_texel, bloom));
        [presented[0], presented[1], presented[2], 255]
    };

    // A. THE DISC CENTER (the brief's tonemap gate): an HDR value
    //    above 1 (the sun disc, saturating ~1.2-1.7) through the
    //    FULL chain — the bloom at full strength + the ACES
    //    composite — vs the CPU reference. The 5×5 center patch.
    let center = size.0 / 2;
    let mut max_error = 0u8;
    let mut checked = 0usize;
    for y in center - 2..=center + 2 {
        for x in center - 2..=center + 2 {
            let expected = oracle(x, y);
            let pixel = posted.pixel_rgba(x, y);
            let error = expected
                .iter()
                .zip(pixel)
                .map(|(e, a)| e.abs_diff(a))
                .max()
                .unwrap();
            max_error = max_error.max(error);
            assert!(
                error <= TOLERANCE,
                "disc-center pixel ({x},{y}): expected {expected:?}, got {pixel:?}"
            );
            checked += 1;
        }
    }
    println!("disc center: 25 pixels vs the full-chain oracle, max error {max_error}/255");

    // B. THE BLOOM RING (the brief's "the sun disc deliberately
    //    exceeds 1.0 so it blooms" pixel check): the pixels just
    //    outside the disc (radius ~11 px — past the disc's ~8-px
    //    radius and its ~1-px soft edge, inside the blur's ~16-px
    //    reach). Their own HDR is the pure gradient (below 1, no
    //    bright-pass of their own) — the presented value is lifted
    //    by the disc's blurred energy, exactly what the oracle
    //    computes, and measurably ABOVE the no-bloom tonemap (the
    //    directional form of the gate).
    let mut ring_lift = 0i32;
    let mut last_predicted_lift = 0i32; // captured for summary print
    for (dx, dy) in [(11i32, 0), (-11, 0), (0, 11), (0, -11)] {
        let (x, y) = ((center as i32 + dx) as u32, (center as i32 + dy) as u32);
        let expected = oracle(x, y);
        let pixel = posted.pixel_rgba(x, y);
        let error = expected
            .iter()
            .zip(pixel)
            .map(|(e, a)| e.abs_diff(a))
            .max()
            .unwrap();
        max_error = max_error.max(error);
        assert!(
            error <= TOLERANCE,
            "bloom-ring pixel ({x},{y}): expected {expected:?}, got {pixel:?}"
        );
        checked += 1;
        // The directional check: the presented pixel exceeds the
        // no-bloom tonemap of its own HDR value (the bloom's lift,
        // channel-wise on the strongest channel).
        // Findings 16 correction: use captured RGB minus no-bloom
        // baseline for the directional assertion and diagnostics;
        // keep the independent full-chain error check unchanged.
        let ray = pixel_ray(forward, right, up, x, y, size);
        let no_bloom = quantized(tonemap_rgb(sky_color(ray, &day.sky, sun_dir)));
        // Measured lift: the captured (presented) pixel minus no-bloom.
        let captured_lift = pixel
            .iter()
            .zip(no_bloom)
            .map(|(e, n)| (*e as i32) - (n as i32))
            .max()
            .unwrap();
        // Predicted lift: expected minus no-bloom (the oracle's prediction).
        let predicted_lift = expected
            .iter()
            .zip(no_bloom)
            .map(|(e, n)| (*e as i32) - (n as i32))
            .max()
            .unwrap();
        ring_lift = ring_lift.max(captured_lift);
        assert!(
            captured_lift >= 2,
            "bloom-ring pixel ({x},{y}): captured lift {captured_lift}/255 >= 2/255, predicted {predicted_lift}/255: pixel {pixel:?} vs no-bloom {no_bloom:?}"
        );
        // Capture predicted_lift for the summary (all 4 pixels use the
        // same no-bloom baseline since they're on the same ring).
        last_predicted_lift = predicted_lift;
    }
    println!(
        "bloom ring: 4 pixels vs the oracle within {TOLERANCE}/255, captured lift ≥2/255 above the no-bloom tonemap (max captured lift {ring_lift}/255, predicted {last_predicted_lift}/255)"
    );

    // C. THE FAR CORNERS (the pure tonemap): bloom is EXACTLY zero
    //    there (the sky values are below 1, so the bright pass is
    //    exactly 0 — the blur of zeros is zero), which makes these
    //    pixels the brief's "an HDR value tonemaps to the
    //    CPU-computed output" in its purest form — the ACES curve
    //    alone, no bloom term. The 3×3 patches at the frame's four
    //    corners double as the orientation detector: a vertical
    //    sampling flip in the composite would swap the top and
    //    bottom corners' rays, and the gradient differs top
    //    (zenith-ward) from bottom (horizon-ward) — the corner
    //    checks fail loudly under any mirror.
    for (cx, cy) in [
        (2, 2),
        (2, size.1 - 5),
        (size.0 - 5, 2),
        (size.0 - 5, size.1 - 5),
    ] {
        for y in cy..cy + 3 {
            for x in cx..cx + 3 {
                let expected = oracle(x, y);
                let pixel = posted.pixel_rgba(x, y);
                let error = expected
                    .iter()
                    .zip(pixel)
                    .map(|(e, a)| e.abs_diff(a))
                    .max()
                    .unwrap();
                max_error = max_error.max(error);
                assert!(
                    error <= TOLERANCE,
                    "far-corner pixel ({x},{y}): expected {expected:?}, got {pixel:?}"
                );
                checked += 1;
            }
        }
    }
    println!(
        "far corners: 36 pixels vs the pure-tonemap oracle (bloom exactly 0), max error so far {max_error}/255"
    );

    // THE POSTLESS PATH RESTORED: post off and skyless again — the
    // frame is BYTE-IDENTICAL to the baseline (the post state
    // exists and stays allocated; the postless path ignores it).
    renderer.set_post(false);
    renderer.set_sky(None);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, high_eye, day_preset(), size)
            .unwrap()
            .presented
    );
    let restored = renderer.take_frame_capture().unwrap().unwrap();
    assert_eq!(
        restored.bytes, plain.bytes,
        "the postless path is byte-identical before and after post frames"
    );
    // And the post frame visibly differs from the plain one (the
    // tonemap changes the whole frame — the filmic curve brightens
    // the mid-tones).
    let mut differ = 0usize;
    for y in 0..size.1 {
        for x in 0..size.0 {
            differ += usize::from(posted.pixel_rgba(x, y) != plain.pixel_rgba(x, y));
        }
    }
    assert!(
        differ > 10_000,
        "the post chain visibly changes the frame ({differ})"
    );
    println!(
        "postless path: byte-identical to the baseline; the post frame differs in {differ} of {} pixels",
        size.0 * size.1
    );

    // WARMED CONSTANCY: two more post frames — the allocation set
    // constant, byte-identical captures.
    renderer.set_post(true);
    renderer.set_sky(Some(SkyFrame {
        palette: day.sky,
        disc_dir: sun_dir,
    }));
    for _ in 0..2 {
        renderer.request_frame_capture().unwrap();
        assert!(
            renderer
                .render_corner(&matrix, high_eye, day_preset(), size)
                .unwrap()
                .presented
        );
        let repeat = renderer.take_frame_capture().unwrap().unwrap();
        assert_eq!(
            renderer.allocations(),
            warm,
            "warmed post allocations constant"
        );
        assert_eq!(repeat.bytes, posted.bytes, "byte-identical post frames");
    }
    println!("warmed post allocations: {warm:?}");

    // THE COMPOSITION SMOKES (one clean presented frame each; the
    // full batteries are their packages' own):
    // - the plain HDR path (post, no sky — the background is the
    //   clear color through the tonemap);
    renderer.set_sky(None);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, high_eye, day_preset(), size)
            .unwrap()
            .presented
    );
    let _ = renderer.take_frame_capture().unwrap().unwrap();
    // - the night moon through the chain;
    let night = atmosphere_at(1.0);
    renderer.set_sky(Some(SkyFrame {
        palette: night.sky,
        disc_dir: night.disc_dir,
    }));
    let toward_moon = std::array::from_fn(|i| high_eye[i] + night.disc_dir[i]);
    let night_matrix = aimed(high_eye, toward_moon);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(
                &night_matrix,
                high_eye,
                engine_renderer::lit_scene::night_preset(),
                size
            )
            .unwrap()
            .presented
    );
    let _ = renderer.take_frame_capture().unwrap().unwrap();
    // - the shadowed path through the chain (the depth pass + the
    //   shadowed HDR pipeline + the composite);
    renderer.set_sky(Some(SkyFrame {
        palette: day.sky,
        disc_dir: sun_dir,
    }));
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner_shadowed(
                &matrix,
                high_eye,
                day_preset(),
                engine_slice::corner_demo::demo_bias(),
                size
            )
            .unwrap()
            .presented
    );
    let _ = renderer.take_frame_capture().unwrap().unwrap();
    // - the populated path through the chain (the v2 silhouettes'
    //   instanced draws inside the HDR pass).
    let (npc, vehicle) = engine_slice::corner_population::corner_population();
    // The Era-2 rest-pose blocks (the skinning input at rest —
    // bit-exact identity, so the composition's pixels are the
    // authored-silhouette equation).
    let npc_poses = vec![engine_renderer::animation::rest_pose_block(); npc.len()];
    let population = engine_renderer::ProxyPopulation {
        npc: &npc,
        npc_poses: &npc_poses,
        npc_material: 1,
        vehicle: &vehicle,
        vehicle_material: 2,
    };
    renderer.request_frame_capture().unwrap();
    let populated_frame = renderer
        .render_corner_populated(&matrix, high_eye, day_preset(), &population, size)
        .unwrap();
    assert!(populated_frame.presented);
    assert!(populated_frame.geometry.proxy_draw_calls > 0);
    let _ = renderer.take_frame_capture().unwrap().unwrap();

    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "{:?}",
        renderer.stats().recent_validation
    );
    println!(
        "Era-1 post gates: {checked} pixels vs post.rs's chain within {TOLERANCE}/255 (measured max {max_error}), \
         the bloom ring lifted ≥2/255, the postless path byte-identical, the post state paid once, \
         every corner-family composition clean"
    );
}

/// Item 1 — Finding 16's asymmetric discriminator (the review's check:
/// "An asymmetric, off-center bright patch in a rectangular target would
/// also discriminate swapped blur axes or a horizontal mirror that a
/// centered disc and vertically varying gradient can hide.")
///
/// THE BLIND SPOT, proven first: the existing 256×256 gate puts the disc
/// DEAD CENTER under a square projection, and the day gradient varies
/// with ELEVATION ONLY — so the left and right corners of any row hold
/// equal values and the centered disc's bloom ring is symmetric: a
/// horizontal mirror of that entire fixture is byte-identical. Nothing
/// in it can see a mirrored composite.
///
/// THE FIXTURE: a second renderer at an ODD RECTANGULAR extent — 121×75,
/// both axes odd, aspect ≈1.61 (the odd axes also serve the odd-extent
/// coverage: floor-divided mip extents and every fractional upsample
/// weight; item 2's ambient cell runs at its own SECOND odd extent,
/// 89×55) — under the frame's TRUE aspect (`aimed_at_aspect`,
/// the sky gate's off-axis block's proven model: the GPU projection
/// carries size.0/size.1, and the CPU oracle below reconstructs rays
/// with the same factor — a square projection stretched onto a
/// rectangular target is NOT this coverage), with the view axis tilted
/// AWAY from the sun in a compound east-and-UP direction so the disc
/// lands fully interior and clearly off-center toward the frame's
/// lower-left. The tilt is UP, not down: the day sun sits at 45°
/// elevation and the 65° vertical fov puts the frame's bottom edge at
/// (view elevation − 32.5°) — an up-tilted axis keeps EVERY row above
/// the horizon and the frame pure sky (the pose clears every mass a
/// fortiori: it looks strictly higher than the proven-clear 256×256
/// sun pose). A down-tilted variant would put the ground plane in the
/// bottom rows and the sky-only oracle below would fail loudly at
/// TOLERANCE 2 — the guard that keeps this fixture honest.
///
/// Probes, all against the full-chain CPU oracle rebuilt at 121×75 with
/// the aspect-corrected rays (`pixel_ray_aspected` → `sky_color` →
/// `bloom_mip_from_hdr` → `post_pixel`), TOLERANCE 2 — the same budget
/// the square gate measures 0/255 under:
/// - The disc-center 5×5 patch, with the disc's position ASSERTED
///   off-center by ≥12% of BOTH axes and interior with margin — under a
///   horizontal mirror this probe reads plain sky: the discriminator.
/// - The bloom ring at (±ring_d, 0) and (0, ±ring_d) around the disc
///   (ring_d CHOSEN by the CPU oracle's own predicted lift — the scan
///   below maximizes the minimum over the four directions and requires
///   ≥2/255, so the fixture can never silently rest on a probe the
///   chain cannot lift): full-chain error ≤ TOLERANCE plus the
///   finding-16 measured-lift form — captured minus the probe's own
///   no-bloom tonemap ≥ 2/255.
/// - The four corner 3×3 patches: the vertical-gradient orientation
///   detector (a vertical flip swaps zenith-ward and horizon-ward
///   corners; the values differ row to row).
///
/// DOCUMENTED HONESTLY (the 4/n commit's precedent): a pure blur-AXIS
/// swap is mathematically UNOBSERVABLE in this chain — `fs_blur_h` and
/// `fs_blur_v` (shaders/bloom.wgsl) share
/// the same five weights (0.0625/0.25/0.375/0.25/0.0625), the same clamp
/// to-edge policy, and same-extent mips, so the separable double sum is
/// symmetric under axis exchange; no fixture can or needs to discriminate it.
/// What the off-center rectangular fixture DOES discriminate: the horizontal
/// mirror (disc-center probe), vertical flip (corner gradient probes), and
/// any future per-axis divergence of weights or clamp policy (the ring probes
/// sit on both axes).
///
/// SDL discipline: one EventPump alive at a time — this gate creates its
/// app fresh under the binary's `sdl_lock`; item 2's gate drops its own
/// renderers before the next.
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup (see render_overflow)"]
fn post_discriminator_asymmetric_disc_and_bloom_lift() {
    let _guard = sdl_lock();

    // --- THE 121×75 RENDERER (odd rectangular; aspect ≈1.61) ---
    let mut app = ShellApp::new(ShellConfig {
        title: "post asymmetric discriminator".into(),
        width: 121,
        height: 75,
        target_fps: 60,
        driver: DriverPreference::Real,
    })
    .unwrap();
    let mut renderer = Renderer::new(
        app.window(),
        RenderConfig {
            demo_scene: DemoScenePreference::Corner,
            validation: ValidationPreference::On,
            ..Default::default()
        },
    )
    .unwrap();
    let size = app.window().size_in_pixels();
    assert_eq!(size, (121, 75));
    assert!(renderer.stats().validation_active);
    let _ = app.frame();

    let day = atmosphere_at(0.0);
    let sun_dir = day.disc_dir;
    let high_eye = [0.0, 25.0, 0.0];

    // The aim: the view axis tilted AWAY from the sun in a compound
    // east-and-UP tangent direction, sized to put the disc off-center
    // toward the frame's lower-left. The tangent offsets a (east) and
    // b (up) map through the camera basis's yaw/pitch to the disc's
    // NDC — a = 0.30·tan_half·aspect and b = 0.70·tan_half land it at
    // NDC (−0.47, −0.31) ≈ (26% of width, 65% of height): off-center
    // on BOTH axes (24% / 15%) with the disc (angular radius ≈2°,
    // ~2-3 px here) far from every edge. The UP half matters twice:
    // it keeps the frame's bottom edge above the horizon (pure sky —
    // see the doc comment), and it gives the vertical off-center
    // margin. (The constants were chosen against the CPU camera math
    // — waypoint_orientation's yaw/pitch over the aim, then the
    // aspected ray grid — the same arithmetic the oracle below runs;
    // the GPU agreed on the first render.)
    let tan_half = (65_f32.to_radians() / 2.0).tan();
    let aspect = size.0 as f32 / size.1 as f32;
    // A horizontal perpendicular to the sun direction (the world-up
    // cross, normalized; nonzero for any non-vertical sun).
    let east = {
        let raw = [-sun_dir[2], 0.0, sun_dir[0]];
        let len = (raw[0] * raw[0] + raw[1] * raw[1] + raw[2] * raw[2]).sqrt();
        [raw[0] / len, raw[1] / len, raw[2] / len]
    };
    // The sun's OWN up-tangent: world-up projected onto the plane
    // perpendicular to the sun (positive y — an UP tilt, never the
    // down-tilt whose ground rows would break the pure-sky premise).
    let up_tangent = {
        let dot = sun_dir[1]; // sun · world_up
        let raw = [-sun_dir[0] * dot, 1.0 - sun_dir[1] * dot, -sun_dir[2] * dot];
        let len = (raw[0] * raw[0] + raw[1] * raw[1] + raw[2] * raw[2]).sqrt();
        [raw[0] / len, raw[1] / len, raw[2] / len]
    };
    let a = 0.30 * tan_half * aspect;
    let b = 0.70 * tan_half;
    let aim_dir: [f32; 3] =
        std::array::from_fn(|axis| sun_dir[axis] + a * east[axis] + b * up_tangent[axis]);
    let aim_len =
        (aim_dir[0] * aim_dir[0] + aim_dir[1] * aim_dir[1] + aim_dir[2] * aim_dir[2]).sqrt();
    let aim_dir = aim_dir.map(|component| component / aim_len);
    // The pure-sky guard, derived not assumed: the view axis's elevation
    // minus the half-fov is the frame's bottom-edge elevation — it must
    // sit above the horizon (the sun's 45° plus the up-tilt minus 32.5°
    // leaves ≈31.5° of margin here).
    let bottom_edge_elevation = aim_dir[1].asin().to_degrees() - 65_f32 / 2.0;
    assert!(
        bottom_edge_elevation > 5.0,
        "the fixture's bottom edge must stay above the horizon (view elevation {}° − 32.5° = {}°)",
        aim_dir[1].asin().to_degrees(),
        bottom_edge_elevation
    );
    let toward = std::array::from_fn(|i| high_eye[i] + aim_dir[i]);
    let matrix = aimed_at_aspect(high_eye, toward, aspect);

    renderer.set_sky(Some(SkyFrame {
        palette: day.sky,
        disc_dir: sun_dir,
    }));
    renderer.set_post(true);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, high_eye, day_preset(), size)
            .unwrap()
            .presented
    );
    let posted = renderer.take_frame_capture().unwrap().unwrap();

    // THE CPU ORACLE at 121×75: the same arithmetic as the square gate,
    // with the rays reconstructed through the frame's TRUE aspect —
    // the pair (aimed_at_aspect, pixel_ray_aspected) is the model the
    // sky gate's off-axis block pinned at 0/255 on this ICD.
    let (forward, right, up) = basis(high_eye, toward);
    let hdr: Vec<[f32; 3]> = (0..size.1)
        .flat_map(|y| (0..size.0).map(move |x| (x, y)))
        .map(|(x, y)| {
            let ray = pixel_ray_aspected(forward, right, up, x, y, size);
            sky_color(ray, &day.sky, sun_dir)
        })
        .collect();
    let mip = bloom_mip_from_hdr(&hdr, size);
    let oracle = |x: u32, y: u32| -> [u8; 4] {
        let hdr_texel = hdr[(y * size.0 + x) as usize];
        let bloom = bloom_at_pixel(&mip, size, (x, y));
        let presented = quantized(post_pixel(hdr_texel, bloom));
        [presented[0], presented[1], presented[2], 255]
    };

    // Find the disc center from the ray-dot with the sun direction
    // (deterministic, like render_sky's off-axis probe): the pixel whose
    // aspect-corrected reconstructed ray dots highest with the sun
    // direction is the disc center's projection — the TRUE disc
    // position under the non-square projection.
    let mut best = (0u32, 0u32, -1.0f32);
    for y in 0..size.1 {
        for x in 0..size.0 {
            let ray = pixel_ray_aspected(forward, right, up, x, y, size);
            let dot = ray[0] * sun_dir[0] + ray[1] * sun_dir[1] + ray[2] * sun_dir[2];
            if dot > best.2 {
                best = (x, y, dot);
            }
        }
    }
    let (cx, cy) = (best.0, best.1);
    // The projected disc center must be OFF-CENTER on BOTH axes (≥12%
    // from the 50% middle) and interior with room for the ring probes —
    // a centered disc is exactly the blind spot this gate exists to
    // close, and an edge-grazing disc would clamp its ring probes.
    let cx_pct = cx as f32 / size.0 as f32;
    let cy_pct = cy as f32 / size.1 as f32;
    assert!(
        (cx_pct - 0.5).abs() >= 0.12,
        "the disc center is off-center by ≥12% of width (actual {cx_pct:.2})"
    );
    assert!(
        (cy_pct - 0.5).abs() >= 0.12,
        "the disc center is off-center by ≥12% of height (actual {cy_pct:.2})"
    );
    // The ring scan's widest candidate (the interior guard below uses
    // it): every candidate distance must keep its probes off the frame
    // edge, with the disc's own ~3 px radius plus the soft edge left
    // between it and the probe.
    const RING_D_MAX: i32 = 12;
    assert!(
        cx as i32 - RING_D_MAX - 2 >= 0
            && cx as i32 + RING_D_MAX + 2 < size.0 as i32
            && cy as i32 - RING_D_MAX - 2 >= 0
            && cy as i32 + RING_D_MAX + 2 < size.1 as i32,
        "the disc center ({cx},{cy}) must keep the ±{RING_D_MAX} ring probes interior"
    );
    println!(
        "disc center (121×75): projected to ({cx},{cy}) = ({cx_pct:.2}, {cy_pct:.2}) of frame, \
         bottom edge {bottom_edge_elevation:.1}° above the horizon"
    );

    let mut max_error = 0u8;
    let mut checked = 0usize;

    // A. THE DISC CENTER 5×5 PATCH (the discriminator under horizontal mirror).
    for y in cy.saturating_sub(2)..=(cy + 2).min(size.1 - 1) {
        for x in cx.saturating_sub(2)..=(cx + 2).min(size.0 - 1) {
            let expected = oracle(x, y);
            let pixel = posted.pixel_rgba(x, y);
            let error = expected
                .iter()
                .zip(pixel.iter())
                .map(|(e, a)| e.abs_diff(*a))
                .max()
                .unwrap();
            max_error = max_error.max(error);
            assert!(
                error <= TOLERANCE,
                "disc-center pixel ({x},{y}): expected {expected:?}, got {pixel:?}"
            );
            checked += 1;
        }
    }
    println!(
        "disc center 5×5: {checked} pixels vs the full-chain oracle, max error {max_error}/255"
    );

    // B. THE BLOOM RING at ±ring_d around the disc center, on BOTH
    // axes (the four cardinal directions). ring_d is CHOSEN by the
    // CPU oracle's own predicted lift — the scan maximizes the
    // MINIMUM predicted lift over the four directions across
    // d ∈ [5, 12] (5 clears the disc's own ~3 px radius plus its soft
    // edge; 12 rides the blur's reach: ±2 mip texels = ±8 scene px
    // plus the upsample's bilinear spread) and REQUIRES ≥2/255, so the
    // fixture can never silently rest on a probe the chain cannot
    // lift — a probe the oracle cannot lift fails the scan loudly.
    let no_bloom_at = |x: u32, y: u32| -> [u8; 3] {
        let ray = pixel_ray_aspected(forward, right, up, x, y, size);
        quantized(tonemap_rgb(sky_color(ray, &day.sky, sun_dir)))
    };
    let predicted_lift_at = |d: i32| -> i32 {
        let mut worst = i32::MAX;
        for (dx, dy) in [(d, 0), (-d, 0), (0, d), (0, -d)] {
            let (x, y) = ((cx as i32 + dx) as u32, (cy as i32 + dy) as u32);
            let expected = oracle(x, y);
            let no_bloom = no_bloom_at(x, y);
            let lift = expected
                .iter()
                .zip(no_bloom.iter())
                .map(|(e, n)| (*e as i32) - (*n as i32))
                .max()
                .unwrap();
            worst = worst.min(lift);
        }
        worst
    };
    let mut ring_d = 0i32;
    let mut best_lift = i32::MIN;
    for d in 5..=RING_D_MAX {
        let lift = predicted_lift_at(d);
        println!("ring candidate d={d}: minimum predicted lift {lift}/255");
        if lift > best_lift {
            best_lift = lift;
            ring_d = d;
        }
    }
    assert!(
        best_lift >= 2,
        "no ring distance in [5, {RING_D_MAX}] lifts all four probes ≥2/255 (best {best_lift} at d={ring_d}) — the fixture's bloom is too thin to measure"
    );
    let mut ring_lift = 0i32;
    for (dx, dy) in [(ring_d, 0), (-ring_d, 0), (0, ring_d), (0, -ring_d)] {
        let (x, y) = ((cx as i32 + dx) as u32, (cy as i32 + dy) as u32);
        let expected = oracle(x, y);
        let pixel = posted.pixel_rgba(x, y);
        let error = expected
            .iter()
            .zip(pixel.iter())
            .map(|(e, a)| e.abs_diff(*a))
            .max()
            .unwrap();
        max_error = max_error.max(error);
        assert!(
            error <= TOLERANCE,
            "bloom-ring pixel ({x},{y}): expected {expected:?}, got {pixel:?}"
        );
        checked += 1;

        // Finding 16 correction: use captured RGB minus no-bloom baseline.
        let no_bloom = no_bloom_at(x, y);
        let captured_lift = pixel
            .iter()
            .zip(no_bloom.iter())
            .map(|(e, n)| (*e as i32) - (*n as i32))
            .max()
            .unwrap();
        ring_lift = ring_lift.max(captured_lift);
        assert!(
            captured_lift >= 2,
            "bloom-ring pixel ({x},{y}): captured lift {captured_lift}/255 >= 2/255 (pixel {pixel:?} vs no-bloom {no_bloom:?})"
        );
    }
    println!(
        "bloom ring (d={ring_d}): 4 pixels vs the oracle within {TOLERANCE}/255, captured lift ≥2/255 (max captured lift {ring_lift}/255, predicted {best_lift}/255)"
    );

    // C. THE FOUR CORNERS 3×3 PATCHES (the orientation detector): a
    // vertical flip swaps the zenith-ward and horizon-ward corners — the
    // gradient differs row to row, so the values differ loudly. At the
    // SAME TOLERANCE 2 as the square gate: the fixture's pure-sky guard
    // (bottom edge above the horizon) is what makes these probes
    // oracle-comparable at all — under the honest budget, any geometry
    // in view or any aspect error in the ray reconstruction fails here.
    for (px, py) in [
        (2, 2),
        (2, size.1 - 5),
        (size.0 - 5, 2),
        (size.0 - 5, size.1 - 5),
    ] {
        for y in py..py + 3 {
            for x in px..px + 3 {
                let expected = oracle(x, y);
                let pixel = posted.pixel_rgba(x, y);
                let error = expected
                    .iter()
                    .zip(pixel.iter())
                    .map(|(e, a)| e.abs_diff(*a))
                    .max()
                    .unwrap();
                max_error = max_error.max(error);
                assert!(
                    error <= TOLERANCE,
                    "far-corner pixel ({x},{y}): expected {expected:?}, got {pixel:?} (error {error})"
                );
                checked += 1;
            }
        }
    }
    println!("far corners: 36 pixels vs the pure-tonemap oracle, max error so far {max_error}/255");

    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "{:?}",
        renderer.stats().recent_validation
    );
    println!(
        "Era-1 post asymmetric discriminator: {checked} pixels vs the chain oracle within {TOLERANCE}/255 (max {max_error}), \
         the disc center off-center ({cx_pct:.2}, {cy_pct:.2}), bloom ring lifted ≥2/255"
    );
}

/// Item 2 — Finding 17's GPU-side edge feed (the review's check:
/// "Feed negative, zero, midtone and over-one HDR into the actual shader
/// chain. Use 1×N, 2×N and an odd rectangular offscreen extent so window-
/// system minimums do not prevent exercising the contract.")
///
/// The review: CPU tonemapping explicitly maps nonpositive input to zero.
/// The shader evaluates the rational function for all inputs. At −0.5 the
/// CPU returns black and the shader clamps to white. In addition, the
/// minimum bloom extent is one texel. For a scene axis of length one or
/// two, some 4i+1 and 4i+2 source taps are outside the image. The CPU
/// explicitly supplies zero. The shader calculates the source extent but
/// issues four unconditional textureLoad calls. The CPU's zero-fill premise
/// is therefore unproven for these accepted dimensions.
///
/// (a) TINY EXTENTS: renderers at 1×256 and 2×256 (sky on, post on, the
/// existing high-eye sun pose under the frame's TRUE aspect —
/// `aimed_at_aspect`). Compare EVERY pixel of the frame against the
/// CPU oracle at that extent, aspect-corrected rays. At width 1 the
/// mip's x-box taps (4i+1, 4i+2) are ALL out of range — the bright mip
/// is provably all-zero (the CPU oracle computes it) — so presented =
/// pure tonemap(sky); at width 2 the box is HALF out of range (the
/// safe_load half-OOB contract) and both columns sit inside the disc's
/// angular radius at the sun's rows (the disc band IS present — a
/// square projection would push the columns ±17.6° off the sun and
/// silently drop it). These cells execute the actual bloom.wgsl
/// safe_load guards on device. The 1×256 window is ACCEPTED by the
/// offscreen driver + lavapipe (MEASURED; the gate asserts it — the
/// review's own "so window-system minimums do not prevent exercising
/// the contract" premise, held to rather than assumed).
///
/// (b) THE AMBIENT FEED at a SECOND odd rectangular extent, 89×55
/// (sky OFF, post ON, the
/// geometry-clearing high-eye pose): `render_corner(&matrix, high_eye,
/// light, size)` with a custom `LightParams` whose ambient is
/// [-0.5, 0.0, 0.18] — the HDR clear color IS the ambient
/// (corner::FrameParams::clear_color). The oracle is the FULL uniform
/// field through the real chain functions (bright → blur → upsample of
/// a constant is the constant): the negative and zero channels must
/// present EXACT black (the pre-4/n shader clamped −0.5 to WHITE — a
/// 255-vs-0 discrimination), the midtone channel the ACES value. Then
/// a second frame with ambient [1.7, 1.7, 1.7]: over-one through ACES
/// WITH the surviving bloom term (1.7 is above the bright-pass
/// threshold, so presented = tonemap(1.7 + 1.7·0.15) — a tonemap-only
/// oracle misses it by ~5/255). A five-pixel spread per frame; any
/// deviation is a geometry bleed or coordinate error, never a
/// tolerance matter.
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup (see render_overflow)"]
fn post_edge_cases_tiny_extents_and_ambient_feed() {
    let _guard = sdl_lock();

    // --- (a) TINY EXTENTS: 1×256 and 2×256 ---
    // PROBE: does SDL/lavapipe accept 1×256?
    let probe_app = ShellApp::new(ShellConfig {
        title: "post tiny-extent probe".into(),
        width: 1,
        height: 256,
        target_fps: 60,
        driver: DriverPreference::Real,
    });
    let (accepted_width, probe_msg) = match probe_app {
        Ok(app) => {
            let size = app.window().size_in_pixels();
            drop(app);
            if size == (1, 256) {
                (1, "1×256 window accepted".to_string())
            } else {
                (
                    size.0,
                    format!("1×256 refused, smallest accepted width is {0}", size.0),
                )
            }
        }
        Err(_) => {
            // If app creation fails entirely, we need to use a different width
            let fallback_app = ShellApp::new(ShellConfig {
                title: "post tiny-extent fallback".into(),
                width: 2,
                height: 256,
                target_fps: 60,
                driver: DriverPreference::Real,
            })
            .unwrap();
            let size = fallback_app.window().size_in_pixels();
            drop(fallback_app);
            (
                size.0,
                format!("1×256 creation failed, using {0}×256", size.0),
            )
        }
    };
    println!("tiny-extent probe: {probe_msg}");
    // The 1×N cell is the review's own requirement ("Use 1×N, 2×N ...
    // so window-system minimums do not prevent exercising the
    // contract") — the offscreen driver exists precisely to have no
    // such minimums. If this environment ever refuses width 1, the gate
    // fails LOUDLY with the probe's message rather than silently
    // degrading to a wider frame: the 4i+1/4i+2 ALL-out-of-range cell
    // (every x-tap of the downsample box beyond the image) exists only
    // at width 1.
    assert!(
        accepted_width == 1,
        "the offscreen environment must accept a 1×256 window ({probe_msg})"
    );

    let day = atmosphere_at(0.0);
    let sun_dir = day.disc_dir;
    let high_eye = [0.0, 25.0, 0.0];
    let toward_sun = std::array::from_fn(|i| high_eye[i] + sun_dir[i]);

    // Test with the accepted width.
    for width in [accepted_width, 2] {
        let mut app = ShellApp::new(ShellConfig {
            title: format!("post tiny-extent {width}×256"),
            width,
            height: 256,
            target_fps: 60,
            driver: DriverPreference::Real,
        })
        .unwrap();
        let mut renderer = Renderer::new(
            app.window(),
            RenderConfig {
                demo_scene: DemoScenePreference::Corner,
                validation: ValidationPreference::On,
                ..Default::default()
            },
        )
        .unwrap();
        let size = app.window().size_in_pixels();
        assert_eq!(
            size,
            (width, 256),
            "the renderer honors the requested extent"
        );
        assert!(renderer.stats().validation_active);
        let _ = app.frame();

        renderer.set_sky(Some(SkyFrame {
            palette: day.sky,
            disc_dir: sun_dir,
        }));
        renderer.set_post(true);
        // The TRUE aspect into the projection (aimed_at_aspect): at
        // width 2 the columns sit ±0.28° of azimuth from the view axis
        // — both INSIDE the disc's ~2° angular radius, so the disc band
        // is present at the sun's rows on both columns and the oracle
        // below must model it (a square projection here would push the
        // columns ±17.6° off the sun and silently drop the disc — the
        // stretched-square accident this gate must not rest on).
        let aspect = size.0 as f32 / size.1 as f32;
        let matrix = aimed_at_aspect(high_eye, toward_sun, aspect);
        renderer.request_frame_capture().unwrap();
        assert!(
            renderer
                .render_corner(&matrix, high_eye, day_preset(), size)
                .unwrap()
                .presented
        );
        let posted = renderer.take_frame_capture().unwrap().unwrap();

        // THE CPU ORACLE at this extent, aspect-corrected rays. At
        // width 1 the single column's nx is exactly 0 (the frame's
        // center ray — aspect-irrelevant); at width 2 both columns'
        // rays hug the view axis per the comment above.
        let (forward, right, up) = basis(high_eye, toward_sun);
        let hdr: Vec<[f32; 3]> = (0..size.1)
            .flat_map(|y| (0..size.0).map(move |x| (x, y)))
            .map(|(x, y)| {
                let ray = pixel_ray_aspected(forward, right, up, x, y, size);
                sky_color(ray, &day.sky, sun_dir)
            })
            .collect();
        let mip = bloom_mip_from_hdr(&hdr, size);
        let oracle = |x: u32, y: u32| -> [u8; 4] {
            let hdr_texel = hdr[(y * size.0 + x) as usize];
            let bloom = bloom_at_pixel(&mip, size, (x, y));
            let presented = quantized(post_pixel(hdr_texel, bloom));
            [presented[0], presented[1], presented[2], 255]
        };

        // Compare EVERY pixel of the frame against the oracle — the
        // whole column at width 1, both columns at width 2 (the
        // half-OOB downsample box differs between the columns' mip
        // texels only through the y taps; the x taps are the contract).
        // At width 1 the bright mip is provably ALL ZERO (the CPU
        // oracle computes it: every 4i+1/4i+2 x-tap is beyond the
        // image, so the box averages zeros) — presented = the pure
        // tonemap of the sky; at width 2 the box is HALF out of range
        // (the 4/n safe_load contract, executed on device).
        //
        // TOLERANCE_RAMP, not TOLERANCE: the every-pixel walk crosses
        // the disc's SOFT EDGE — each cell has ONE mid-ramp row where
        // the ~1.8·10⁶/255-per-dot ramp slope amplifies the CPU/GPU
        // ray-arithmetic residual to 3-4/255 (MEASURED, both signs —
        // the constant's doc comment carries the full analysis; the
        // raw-sky path measured 1/255 at its ramp in PR #25). Away
        // from the ramp rows the error is 0-1.
        let mut max_error = 0u8;
        let mut checked = 0usize;
        for y in 0..size.1 {
            for x in 0..size.0 {
                let expected = oracle(x, y);
                let pixel = posted.pixel_rgba(x, y);
                let error = expected
                    .iter()
                    .zip(pixel.iter())
                    .map(|(e, a)| e.abs_diff(*a))
                    .max()
                    .unwrap();
                max_error = max_error.max(error);
                assert!(
                    error <= TOLERANCE_RAMP,
                    "tiny-extent ({width}×256) pixel ({x},{y}): expected {expected:?}, got {pixel:?}"
                );
                checked += 1;
            }
        }
        println!(
            "tiny-extent {width}×256: {checked} pixels vs the oracle within {TOLERANCE}/255, max error {max_error}/255"
        );

        assert_eq!(
            renderer.stats().validation_errors,
            0,
            "{:?}",
            renderer.stats().recent_validation
        );

        // Drop the renderer and app before the next iteration or ambient feed test.
        drop(renderer);
        drop(app);
    }

    // --- (b) THE AMBIENT FEED at 89×55 (sky OFF, post ON) ---
    // Create a fresh 89×55 renderer for the ambient feed test.
    let mut app = ShellApp::new(ShellConfig {
        title: "post ambient feed 89×55".into(),
        width: 89,
        height: 55,
        target_fps: 60,
        driver: DriverPreference::Real,
    })
    .unwrap();
    let mut renderer = Renderer::new(
        app.window(),
        RenderConfig {
            demo_scene: DemoScenePreference::Corner,
            validation: ValidationPreference::On,
            ..Default::default()
        },
    )
    .unwrap();
    let size = app.window().size_in_pixels();
    assert_eq!(size, (89, 55));
    assert!(renderer.stats().validation_active);
    let _ = app.frame();

    // The geometry-clearing high-eye pose (skyless — the HDR target
    // holds NOTHING but the clear color, which IS the ambient:
    // corner::FrameParams::clear_color returns [r, g, b, 1.0] from
    // light.ambient). The same true-aspect discipline as item 1.
    let toward = std::array::from_fn(|i| high_eye[i] + sun_dir[i]);
    let matrix = aimed_at_aspect(high_eye, toward, size.0 as f32 / size.1 as f32);

    // The uniform-field oracle: the FULL chain over a constant HDR
    // field — the bright pass, the blur, and the upsample of a uniform
    // value are that value (the box average, the clamped taps, and the
    // bilinear weights all reproduce a constant; the CPU oracle's own
    // arithmetic, never a hand-derived shortcut). For ambients at or
    // below 1 the bright pass zeroes the field and presented =
    // tonemap(ambient); for 1.7 the bloom term SURVIVES — presented =
    // tonemap(1.7 + 1.7·0.15) — the term a tonemap-only oracle would
    // miss by ~5/255.
    let uniform_oracle = |ambient: [f32; 3]| -> [u8; 4] {
        let hdr = vec![ambient; (size.0 * size.1) as usize];
        let mip = bloom_mip_from_hdr(&hdr, size);
        let bloom = bloom_at_pixel(&mip, size, (size.0 / 2, size.1 / 2));
        let presented = quantized(post_pixel(ambient, bloom));
        [presented[0], presented[1], presented[2], 255]
    };

    // A five-pixel spread (the four corners and the center): the frame
    // is uniform by construction, so any pixel that deviates is a
    // geometry bleed or a coordinate error — not a tolerance matter.
    let spread = [
        (0u32, 0u32),
        (size.0 - 1, 0),
        (0, size.1 - 1),
        (size.0 - 1, size.1 - 1),
        (size.0 / 2, size.1 / 2),
    ];

    // (b-i) ambient = [-0.5, 0.0, 0.18] — negative → black, zero →
    // black, midtone → ACES, channel-wise in ONE frame. The R and G
    // channels are asserted EXACTLY zero: before the 4/n shader guard,
    // post.wgsl evaluated the ACES rational at −0.5 and clamped to
    // WHITE — a 255-vs-0 discrimination no tolerance could miss; the
    // exact-zero assertion keeps that margin honest forever.
    let light_neg = LightParams {
        ambient: [-0.5, 0.0, 0.18],
        ..day_preset()
    };
    renderer.set_sky(None);
    renderer.set_post(true);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, high_eye, light_neg, size)
            .unwrap()
            .presented
    );
    let ambient_neg = renderer.take_frame_capture().unwrap().unwrap();
    let expected_neg = uniform_oracle([-0.5, 0.0, 0.18]);
    println!(
        "ambient feed [-0.5, 0.0, 0.18]: expected {expected_neg:?} (negative/zero black, midtone tonemapped)"
    );
    assert_eq!(
        expected_neg[0], 0,
        "the CPU contract maps the negative channel to exactly black"
    );
    assert_eq!(
        expected_neg[1], 0,
        "the CPU contract maps the zero channel to exactly black"
    );
    for (x, y) in spread {
        let pixel = ambient_neg.pixel_rgba(x, y);
        assert_eq!(
            pixel[0], 0,
            "ambient feed [-0.5, 0.0, 0.18] at ({x},{y}): the negative channel must present EXACT black (the pre-guard shader clamped it to white)"
        );
        assert_eq!(
            pixel[1], 0,
            "ambient feed [-0.5, 0.0, 0.18] at ({x},{y}): the zero channel must present EXACT black"
        );
        assert!(
            pixel[2].abs_diff(expected_neg[2]) <= TOLERANCE,
            "ambient feed [-0.5, 0.0, 0.18] at ({x},{y}): midtone channel {pixel:?} vs expected {expected_neg:?}"
        );
    }

    // (b-ii) ambient = [1.7, 1.7, 1.7] — over-one through the ACTUAL
    // ACES composite WITH the surviving bloom term (1.7 is above the
    // bright-pass threshold, so the chain adds 0.15·1.7 before the
    // tonemap — the uniform oracle above computes it through the real
    // pipeline).
    let light_over = LightParams {
        ambient: [1.7, 1.7, 1.7],
        ..day_preset()
    };
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, high_eye, light_over, size)
            .unwrap()
            .presented
    );
    let ambient_over = renderer.take_frame_capture().unwrap().unwrap();
    let expected_over = uniform_oracle([1.7, 1.7, 1.7]);
    println!(
        "ambient feed [1.7, 1.7, 1.7]: expected {expected_over:?} (over-one through ACES + the uniform bloom term)"
    );
    for (x, y) in spread {
        let pixel = ambient_over.pixel_rgba(x, y);
        for channel in 0..3 {
            let error = pixel[channel].abs_diff(expected_over[channel]);
            assert!(
                error <= TOLERANCE,
                "ambient feed [1.7, 1.7, 1.7] at ({x},{y}) channel {channel}: pixel {pixel:?} vs expected {expected_over:?} (error {error})"
            );
        }
    }

    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "{:?}",
        renderer.stats().recent_validation
    );
    println!(
        "Era-1 post edge cases: tiny extents ({accepted_width}×256, 2×256) + ambient feed verified"
    );
}
/// (corrective/frame-preparation, findings 4 + 7) The depth-format
/// fallback matrix and the post lifecycle across swapchain
/// recreations. Finding 4: with each forced fallback depth format
/// (the `DepthFormatPreference` knob — the `replace_pipelines`
/// precedent), the post chain renders the SAME pure-sky oracle the
/// package's own gate pins (create, render, resize, capture — the
/// review's check), because the negotiated format reaches the HDR
/// depth attachment's view aspect and the reflection-free post path
/// cleanly. Finding 7: post on → post off → TWO recreations (resize
/// away, resize back to the ORIGINAL extent) → post on — the chain
/// renders the oracle again at the original extent (a retired
/// framebuffer set cached on a recycled view handle would not), the
/// warmed allocation set is unchanged (no leak growth from the
/// rebuild), and a repeat capture is byte-identical. The knob's
/// outcome is read from the renderer's reported depth format — the
/// skip condition is the REPORT, never a guess.
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup (see render_overflow)"]
fn depth_fallbacks_and_the_post_lifecycle() {
    let _guard = sdl_lock();
    /// The full presented image the pure-sky CPU chain computes for
    /// an already-built HDR texel field (the first gate's own
    /// arithmetic, materialized for probe reuse).
    fn presented_from_hdr(hdr: &[[f32; 3]], size: (u32, u32)) -> Vec<[u8; 4]> {
        let mip = bloom_mip_from_hdr(hdr, size);
        let mut image = Vec::with_capacity((size.0 * size.1) as usize);
        for y in 0..size.1 {
            for x in 0..size.0 {
                let bloom = bloom_at_pixel(&mip, size, (x, y));
                let presented = quantized(post_pixel(hdr[(y * size.0 + x) as usize], bloom));
                image.push([presented[0], presented[1], presented[2], 255]);
            }
        }
        image
    }

    /// The first gate's probe set — the disc-center 5×5 patch (the
    /// bloom feeder) and the four far-corner 3×3 patches (the pure
    /// ACES tonemap, the orientation detector) — against a presented
    /// oracle image.
    fn check_probes(
        capture: &engine_renderer::CapturedFrame,
        oracle: &[[u8; 4]],
        size: (u32, u32),
        what: &str,
    ) {
        let center = size.0 / 2;
        let mut max_error = 0u8;
        let mut checked = 0usize;
        for y in center - 2..=center + 2 {
            for x in center - 2..=center + 2 {
                let expected = oracle[(y * size.0 + x) as usize];
                let actual = capture.pixel_rgba(x, y);
                let error = expected
                    .iter()
                    .zip(actual)
                    .map(|(e, a)| e.abs_diff(a))
                    .max()
                    .unwrap();
                max_error = max_error.max(error);
                assert!(
                    error <= TOLERANCE,
                    "{what} disc-center pixel ({x},{y}): expected {expected:?}, got {actual:?}"
                );
                checked += 1;
            }
        }
        for (cx, cy) in [
            (2, 2),
            (2, size.1 - 5),
            (size.0 - 5, 2),
            (size.0 - 5, size.1 - 5),
        ] {
            for y in cy..cy + 3 {
                for x in cx..cx + 3 {
                    let expected = oracle[(y * size.0 + x) as usize];
                    let actual = capture.pixel_rgba(x, y);
                    let error = expected
                        .iter()
                        .zip(actual)
                        .map(|(e, a)| e.abs_diff(a))
                        .max()
                        .unwrap();
                    max_error = max_error.max(error);
                    assert!(
                        error <= TOLERANCE,
                        "{what} far-corner pixel ({x},{y}): expected {expected:?}, got {actual:?}"
                    );
                    checked += 1;
                }
            }
        }
        println!("{what}: {checked} probe pixels vs the chain oracle, max error {max_error}/255");
    }

    let day = atmosphere_at(0.0);
    let sun_dir = day.disc_dir;
    let high_eye = [0.0, 25.0, 0.0];
    let toward_sun = std::array::from_fn(|i| high_eye[i] + sun_dir[i]);
    let matrix = aimed(high_eye, toward_sun);

    // --- FINDING 4: the forced-fallback matrix (the post-only cell
    // of the review's post-only / wet-only / combined matrix; the wet
    // cell is render_reflection's own gate, the combined cell is
    // render_reflection's first-frame gate).
    for (preference, forced_name) in [
        (DepthFormatPreference::ForceD24S8, "D24_UNORM_S8_UINT"),
        (DepthFormatPreference::ForceD16, "D16_UNORM"),
    ] {
        let app = ShellApp::new(ShellConfig {
            title: "post-depth-fallback".into(),
            width: 256,
            height: 256,
            target_fps: 60,
            driver: DriverPreference::Real,
        })
        .unwrap();
        let mut renderer = Renderer::new(
            app.window(),
            RenderConfig {
                demo_scene: DemoScenePreference::Corner,
                validation: ValidationPreference::On,
                depth_format: preference,
                ..Default::default()
            },
        )
        .unwrap();
        let reported = renderer.stats().depth_format.clone();
        if reported != forced_name {
            println!(
                "post fallback {forced_name}: the device lacks it (reported {reported}); the \
                 knob fell back to the negotiated pick — variant skipped"
            );
            drop(renderer);
            drop(app);
            continue;
        }
        println!("post fallback: forced {forced_name} honored");
        renderer.set_sky(Some(SkyFrame {
            palette: day.sky,
            disc_dir: sun_dir,
        }));
        renderer.set_post(true);

        // Create, render, capture — the oracle at the original size.
        let size = app.window().size_in_pixels();
        renderer.request_frame_capture().unwrap();
        assert!(
            renderer
                .render_corner(&matrix, high_eye, day_preset(), size)
                .unwrap()
                .presented
        );
        let posted = renderer.take_frame_capture().unwrap().unwrap();
        let (forward, right, up) = basis(high_eye, toward_sun);
        let hdr = (0..size.1)
            .flat_map(|y| (0..size.0).map(move |x| (x, y)))
            .map(|(x, y)| {
                let ray = pixel_ray(forward, right, up, x, y, size);
                sky_color(ray, &day.sky, sun_dir)
            })
            .collect::<Vec<_>>();
        check_probes(
            &posted,
            &presented_from_hdr(&hdr, size),
            size,
            &format!("post fallback {forced_name} at 256"),
        );

        // Resize (a forced recreation), render, capture — the SAME
        // oracle recomputed at the new extent.
        let resized = (size.0 + 8, size.1 + 8);
        renderer.request_frame_capture().unwrap();
        assert!(
            renderer
                .render_corner(&matrix, high_eye, day_preset(), resized)
                .unwrap()
                .presented
        );
        let resized_capture = renderer.take_frame_capture().unwrap().unwrap();
        let hdr_resized = (0..resized.1)
            .flat_map(|y| (0..resized.0).map(move |x| (x, y)))
            .map(|(x, y)| {
                let ray = pixel_ray(forward, right, up, x, y, resized);
                sky_color(ray, &day.sky, sun_dir)
            })
            .collect::<Vec<_>>();
        check_probes(
            &resized_capture,
            &presented_from_hdr(&hdr_resized, resized),
            resized,
            &format!("post fallback {forced_name} after resize"),
        );

        // Warmed constancy on the replacement generation.
        let warm = renderer.allocations();
        renderer.request_frame_capture().unwrap();
        assert!(
            renderer
                .render_corner(&matrix, high_eye, day_preset(), resized)
                .unwrap()
                .presented
        );
        let repeat = renderer.take_frame_capture().unwrap().unwrap();
        assert_eq!(
            renderer.allocations(),
            warm,
            "warmed fallback allocations constant"
        );
        assert_eq!(
            repeat.bytes, resized_capture.bytes,
            "byte-identical fallback frames"
        );
        assert_eq!(
            renderer.stats().validation_errors,
            0,
            "{:?}",
            renderer.stats().recent_validation
        );
        println!("post fallback {forced_name}: warmed allocations constant, validation clean");
        drop(renderer);
        drop(app);
    }

    // --- FINDING 7: the post lifecycle across two recreations,
    // returning to the ORIGINAL extent (the retired-handle cache-hit
    // shape; the generation key makes it structurally impossible —
    // this pins the rebuild behavior it replaces).
    let app = ShellApp::new(ShellConfig {
        title: "post-lifecycle".into(),
        width: 256,
        height: 256,
        target_fps: 60,
        driver: DriverPreference::Real,
    })
    .unwrap();
    let mut renderer = Renderer::new(
        app.window(),
        RenderConfig {
            demo_scene: DemoScenePreference::Corner,
            validation: ValidationPreference::On,
            ..Default::default()
        },
    )
    .unwrap();
    let size = app.window().size_in_pixels();
    renderer.set_sky(Some(SkyFrame {
        palette: day.sky,
        disc_dir: sun_dir,
    }));
    renderer.set_post(true);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, high_eye, day_preset(), size)
            .unwrap()
            .presented
    );
    let reference = renderer.take_frame_capture().unwrap().unwrap();
    let (forward, right, up) = basis(high_eye, toward_sun);
    let hdr = (0..size.1)
        .flat_map(|y| (0..size.0).map(move |x| (x, y)))
        .map(|(x, y)| {
            let ray = pixel_ray(forward, right, up, x, y, size);
            sky_color(ray, &day.sky, sun_dir)
        })
        .collect::<Vec<_>>();
    let reference_oracle = presented_from_hdr(&hdr, size);
    check_probes(
        &reference,
        &reference_oracle,
        size,
        "post lifecycle reference",
    );
    let warm = renderer.allocations();

    // Post OFF, then two recreations — away and BACK to the original
    // extent — then post ON again. The allocation ledger is asserted
    // as an EXACT rebuild budget, not a flat equality: AllocationStats
    // counts successful allocation CALLS since construction (the
    // documented monotonic semantics the pipeline-replacement gates
    // pin their deltas against), and a swapchain recreation necessarily
    // makes new calls — corrective finding A's own one-depth-per-image
    // design rebuilds the depth set at the new extent, and corrective
    // finding 7's generation key rebuilds the post bundle against the
    // new views. What must NOT happen: the postless frame allocating
    // anything, the away recreation touching pools/sets (post is OFF —
    // the bundle must not rebuild while nothing reads it), the back
    // frame rebuilding the bundle MORE than once, or the warmed final
    // frame drifting. (The first version of this check asserted a flat
    // `== warm` across the two recreations — unpassable by design, and
    // masked behind the stencil-load validation failure until that was
    // fixed; see the branch's finding-4 corrective.)
    renderer.set_post(false);
    renderer.request_frame_capture().unwrap();
    let postless = renderer
        .render_corner(&matrix, high_eye, day_preset(), size)
        .unwrap();
    // THE COORDINATOR FLAG CONTRACT (the finding-20 phase-A seam's
    // own gate — the extraction once dropped exactly these flags,
    // and no pixel gate could see it): a same-size postless frame
    // neither defers nor recreates; the minimized path (a zero
    // extent) defers with nothing presented and no state touched;
    // a size change recreates and reports it. These are the public
    // FrameResult fields callers poll (the poll-and-retry contract).
    assert!(postless.presented);
    assert!(!postless.deferred && !postless.recreated);
    let _ = renderer.take_frame_capture().unwrap().unwrap();
    renderer.request_frame_capture().unwrap();
    let minimized = renderer
        .render_corner(&matrix, high_eye, day_preset(), (0, 0))
        .unwrap();
    assert!(
        !minimized.presented && minimized.deferred && !minimized.recreated,
        "the minimized frame defers without presenting (got presented {} deferred {} \
         recreated {})",
        minimized.presented,
        minimized.deferred,
        minimized.recreated
    );
    // The deferral must leave the pending capture request ALIVE for
    // the next frame that records (the frame-sync discipline: a
    // deferred frame touches no synchronization state, and the
    // capture is a request, not an obligation of THIS frame). The
    // typed CaptureOutstanding refusal on a second request is
    // itself the proof the first is still pending.
    let _ = renderer.request_frame_capture().unwrap_err();
    let recovered = renderer
        .render_corner(&matrix, high_eye, day_preset(), size)
        .unwrap();
    assert!(
        recovered.presented && !recovered.deferred,
        "the frame after the deferral proceeds (the probe-pending flag cleared)"
    );
    assert!(
        renderer.take_frame_capture().unwrap().is_some(),
        "the capture request survived the deferred frame"
    );
    assert!(
        !renderer.take_frame_capture().unwrap().is_some(),
        "the capture was consumed by the take above"
    );
    assert_eq!(
        renderer.allocations(),
        warm,
        "the postless, minimized, and recovery frames allocate nothing (the lazy states \
         stay put; a deferral touches no state)"
    );
    let away = (size.0 + 8, size.1 + 8);
    renderer.request_frame_capture().unwrap();
    let awayed = renderer
        .render_corner(&matrix, high_eye, day_preset(), away)
        .unwrap();
    assert!(
        awayed.presented && awayed.recreated && !awayed.deferred,
        "the away frame reports its swapchain recreation (got presented {} recreated {} \
         deferred {})",
        awayed.presented,
        awayed.recreated,
        awayed.deferred
    );
    let _ = renderer.take_frame_capture().unwrap().unwrap();
    let after_away = renderer.allocations();
    // The away recreation's only growth is the replacement chain's
    // per-image depth set (finding A): device_memory by the image
    // count, nothing else. N is self-calibrated here — the image count
    // is the device's to choose (this pod's lavapipe: 6).
    let depth_per_chain = after_away.device_memory - warm.device_memory;
    assert!(
        depth_per_chain > 0,
        "the away recreation rebuilt its depth set (found {depth_per_chain} depth memories)"
    );
    assert_eq!(
        (after_away.descriptor_pools, after_away.descriptor_sets),
        (warm.descriptor_pools, warm.descriptor_sets),
        "post OFF: the away recreation must not rebuild the post bundle"
    );
    renderer.set_post(true);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, high_eye, day_preset(), size)
            .unwrap()
            .presented
    );
    let restored = renderer.take_frame_capture().unwrap().unwrap();
    check_probes(
        &restored,
        &reference_oracle,
        size,
        "post lifecycle restored at the original extent",
    );

    // Determinism at the final state, no leak growth across the
    // recreations, and a clean validation record.
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, high_eye, day_preset(), size)
            .unwrap()
            .presented
    );
    let repeat = renderer.take_frame_capture().unwrap().unwrap();
    assert_eq!(
        repeat.bytes, restored.bytes,
        "byte-identical restored frames"
    );
    // The EXACT rebuild budget: across the whole lifecycle the growth
    // is two replacement depth sets (one per recreation, N memories
    // each) plus ONE post bundle (2 pools, 8 sets, 8 memories — its
    // images). A second bundle rebuild (a per-frame or per-acquire
    // churn) fails the pools/sets equality; a stale bundle fails the
    // pixel oracle above; a per-frame drift fails the warmed equality
    // below.
    let final_stats = renderer.allocations();
    assert_eq!(
        (
            final_stats.device_memory,
            final_stats.descriptor_pools,
            final_stats.descriptor_sets
        ),
        (
            warm.device_memory + 2 * depth_per_chain + 8,
            warm.descriptor_pools + 2,
            warm.descriptor_sets + 8
        ),
        "the lifecycle grew exactly two depth sets and one post bundle"
    );
    assert_eq!(
        renderer.allocations(),
        final_stats,
        "the warmed restored frame allocates nothing"
    );
    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "{:?}",
        renderer.stats().recent_validation
    );
    println!(
        "post lifecycle: restored at the original extent through the oracle, the exact \
         rebuild budget ({depth_per_chain} depth memories per chain × 2 + one post bundle), \
         no warmed drift, validation clean"
    );
}

/// The diagnostic fault-injection seam verification gate (corrective
/// findings 3+8): the target builder's per-object construction
/// sequences register handles immediately, and partial-construction
/// failures fully unwind. The gate injects failures at every stage
/// of every builder and verifies:
/// - no frame slot is stranded by a recoverable target-construction
///   failure (finding 3)
/// - all acquired handles unwind on partial failure (finding 8)
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup"]
fn injected_target_failures_unwind_every_handle_and_never_strand_a_slot() {
    let _guard = sdl_lock();

    /// Reads the live count for a given kind (creations - destructions).
    fn live(census: &engine_renderer::AllocationStats, kind: &str) -> i64 {
        match kind {
            "images" => census.target_images as i64 - census.target_image_frees as i64,
            "views" => census.target_image_views as i64 - census.target_view_frees as i64,
            "framebuffers" => {
                census.target_framebuffers as i64 - census.target_framebuffer_frees as i64
            }
            "memories" => census.target_memories as i64 - census.target_memory_frees as i64,
            _ => panic!("unknown kind: {kind}"),
        }
    }

    /// Creates a fresh renderer with corner demo and validation. The
    /// ShellApp is returned beside the renderer and must outlive it
    /// (the renderer's surface belongs to the app's window — the
    /// existing gates' `drop(renderer); drop(app);` order); each cell
    /// keeps both alive for its whole scope.
    fn new_renderer() -> (ShellApp, engine_renderer::Renderer) {
        let app = ShellApp::new(ShellConfig {
            title: "injected-target-failures".into(),
            width: 256,
            height: 256,
            target_fps: 60,
            driver: DriverPreference::Real,
        })
        .unwrap();
        let renderer = engine_renderer::Renderer::new(
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

    /// The demo's own shadow bias (render_shadows' BIAS — one source
    /// for both; the Shadow cells' shadowed renders).
    const BIAS: ShadowBias = ShadowBias {
        constant: 0.0015,
        slope_scale: 0.002,
    };

    let day = atmosphere_at(0.0);
    let sun_dir = day.disc_dir;
    let high_eye = [0.0, 25.0, 0.0];
    let toward_sun = std::array::from_fn(|i| high_eye[i] + sun_dir[i]);
    let matrix = aimed(high_eye, toward_sun);
    let size = (256, 256);

    // PHASE 1: finding 3 (slot-stranding check)
    // "Inject an allocation or framebuffer failure on the first post or
    // wet frame, then submit two ordinary frames through the same renderer
    // and verify both slots remain usable."
    println!("=== PHASE 1: finding 3 (slot-stranding check) ===");

    // 1. Baseline: no targets constructed yet.
    // The app binding is underscore-named but STILL OWNS the ShellApp
    // until scope end (only a bare `_` would drop it immediately) —
    // the renderer's surface belongs to its window, so the app must
    // outlive the renderer (drop order: renderer, then app).
    let (_app, mut renderer) = new_renderer();
    renderer.set_sky(Some(SkyFrame {
        palette: day.sky,
        disc_dir: sun_dir,
    }));
    let pre = renderer.allocations();
    assert_eq!(
        (
            pre.target_images,
            pre.target_image_views,
            pre.target_framebuffers,
            pre.target_memories
        ),
        (0, 0, 0, 0),
        "baseline: no target objects constructed"
    );

    // 2. Inject Memory fault on first post frame.
    renderer.inject_target_fault(TargetFaultSite::Post, TargetFaultStage::Memory, 1);
    renderer.set_post(true);
    let result = renderer.render_corner(&matrix, high_eye, day_preset(), size);
    assert!(
        matches!(result, Err(RendererError::InjectedFault(_))),
        "first fault should be InjectedFault, got {:?}",
        result
    );

    // 3. Census after failure: live == 0 for every kind (partial unwound).
    let post_fault = renderer.allocations();
    for kind in ["images", "views", "framebuffers", "memories"] {
        assert_eq!(
            live(&post_fault, kind),
            0,
            "live {} after post Memory fault should be 0",
            kind
        );
    }

    // 4. Two ORDINARY frames: both slots should remain usable.
    renderer.set_post(false);
    let before_frames = renderer.stats().frames_presented;
    let r1 = renderer.render_corner(&matrix, high_eye, day_preset(), size);
    assert!(r1.unwrap().presented, "first ordinary frame should succeed");
    let r2 = renderer.render_corner(&matrix, high_eye, day_preset(), size);
    assert!(
        r2.unwrap().presented,
        "second ordinary frame should succeed"
    );
    let after_frames = renderer.stats().frames_presented;
    assert_eq!(
        after_frames - before_frames,
        2,
        "both ordinary frames were presented"
    );

    // 5. Recovery: post back on, should work and build targets.
    renderer.set_post(true);
    renderer.request_frame_capture().unwrap();
    let r3 = renderer.render_corner(&matrix, high_eye, day_preset(), size);
    let presented = r3.unwrap().presented;
    assert!(presented, "post recovery frame should succeed");
    let capture = renderer.take_frame_capture().unwrap();
    assert!(capture.is_some(), "capture should be available");
    let bytes = capture.unwrap().bytes;
    assert!(!bytes.is_empty(), "capture should have non-empty bytes");

    let post_recovery = renderer.allocations();
    for kind in ["images", "views", "framebuffers", "memories"] {
        let l = live(&post_recovery, kind);
        assert!(
            l > 0,
            "live {} after post recovery should be > 0 (actual: {})",
            kind,
            l
        );
        println!("post recovery live {}: {}", kind, l);
    }

    // 6. The wet cell on the SAME renderer. The post bundle recovered
    // in step 5 is LIVE at this point — the wet fault fires inside the
    // wet-reflection target build, which runs AFTER the post targets
    // were ensured in the same frame (the frame flow's ensure order).
    // The correct count comparison is against the PRE-ATTEMPT census:
    // the wet partial must unwind to exactly the live set that existed
    // before it (the completed post bundle survives untouched — the
    // "including earlier successfully constructed slots" requirement
    // in its phase-2 form).
    let wet_baseline = renderer.allocations();
    renderer.inject_target_fault(TargetFaultSite::Wet, TargetFaultStage::Framebuffer, 1);
    renderer.set_wet(true);
    let result = renderer.render_corner(&matrix, high_eye, day_preset(), size);
    assert!(
        matches!(result, Err(RendererError::InjectedFault(_))),
        "wet fault should be InjectedFault, got {:?}",
        result
    );

    let post_wet_fault = renderer.allocations();
    for kind in ["images", "views", "framebuffers", "memories"] {
        assert_eq!(
            live(&post_wet_fault, kind),
            live(&wet_baseline, kind),
            "live {} after wet Framebuffer fault must equal the pre-attempt \
             census (the wet partial unwound; the post bundle survived)",
            kind
        );
    }

    // Two ordinary frames again (post may stay on).
    renderer.set_wet(false);
    let before_frames = renderer.stats().frames_presented;
    let r1 = renderer.render_corner(&matrix, high_eye, day_preset(), size);
    assert!(r1.unwrap().presented, "wet recovery frame 1 should succeed");
    let r2 = renderer.render_corner(&matrix, high_eye, day_preset(), size);
    assert!(r2.unwrap().presented, "wet recovery frame 2 should succeed");
    let after_frames = renderer.stats().frames_presented;
    assert_eq!(
        after_frames - before_frames,
        2,
        "both wet recovery frames were presented"
    );

    // Recovery wet frame.
    renderer.set_wet(true);
    renderer.request_frame_capture().unwrap();
    let r3 = renderer.render_corner(&matrix, high_eye, day_preset(), size);
    assert!(r3.unwrap().presented, "wet recovery frame should succeed");
    let capture = renderer.take_frame_capture().unwrap();
    assert!(capture.is_some(), "wet capture should be available");
    let bytes = capture.unwrap().bytes;
    assert!(!bytes.is_empty(), "wet capture should have non-empty bytes");

    let post_wet_recovery = renderer.allocations();
    for kind in ["images", "views", "framebuffers", "memories"] {
        let l = live(&post_wet_recovery, kind);
        assert!(
            l > live(&wet_baseline, kind),
            "live {} after wet recovery must exceed the pre-attempt census \
             (the wet bundle was built on top of the surviving post bundle)",
            kind
        );
        println!("wet recovery live {}: {}", kind, l);
    }

    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "validation errors during phase 1: {:?}",
        renderer.stats().recent_validation
    );

    // Release phase 1's window BEFORE phase 2's fresh renderers (SDL
    // allows one EventPump per process — the existing gates' drop
    // discipline; the renderer goes first, then its window's app).
    drop(renderer);
    drop(_app);

    // PHASE 2: finding 8 (creation/destruction count matrix)
    // "Inject a failure after each acquisition and compare corresponding
    // creation/destruction counts, including earlier successfully
    // constructed slots."
    println!("=== PHASE 2: finding 8 (creation/destruction count matrix) ===");

    let sites = [
        TargetFaultSite::Post,
        TargetFaultSite::Wet,
        TargetFaultSite::Shadow,
    ];
    let stages = [
        TargetFaultStage::Image,
        TargetFaultStage::MemoryLookup,
        TargetFaultStage::Memory,
        TargetFaultStage::Bind,
        TargetFaultStage::View,
        TargetFaultStage::Framebuffer,
    ];

    for site in sites {
        for stage in &stages {
            // A FRESH renderer for each cell (app outlives renderer:
            // the drop order at scope end is renderer, then app).
            // (the underscore app still owns the window — see phase 1)
            let (_app, mut renderer) = new_renderer();
            renderer.set_sky(Some(SkyFrame {
                palette: day.sky,
                disc_dir: sun_dir,
            }));

            // Set up the appropriate state for the site.
            match site {
                TargetFaultSite::Post => {
                    renderer.set_post(true);
                }
                TargetFaultSite::Wet => {
                    // The wet-reflection target build runs AFTER the
                    // post targets were ensured in the same frame, so a
                    // first-frame wet fault leaves the COMPLETED post
                    // bundle live. Render one post-only frame first:
                    // the census it leaves is the baseline the wet
                    // partial must unwind back to.
                    renderer.set_post(true);
                    let warm = renderer.render_corner(&matrix, high_eye, day_preset(), size);
                    assert!(
                        warm.unwrap().presented,
                        "the wet cell's post-only warm frame"
                    );
                    renderer.set_wet(true);
                }
                TargetFaultSite::Shadow => {
                    // The shadow state builds lazily on the FIRST
                    // shadowed frame — the cell renders shadowed.
                }
            }

            // Baseline census.
            let pre = renderer.allocations();
            match site {
                TargetFaultSite::Wet => {
                    for kind in ["images", "views", "framebuffers", "memories"] {
                        assert!(
                            live(&pre, kind) > 0,
                            "wet cell baseline: the post-only warm frame left {} live",
                            kind
                        );
                    }
                }
                TargetFaultSite::Post | TargetFaultSite::Shadow => {
                    assert_eq!(
                        (
                            pre.target_images,
                            pre.target_image_views,
                            pre.target_framebuffers,
                            pre.target_memories
                        ),
                        (0, 0, 0, 0),
                        "baseline for site {:?} stage {:?}",
                        site,
                        stage
                    );
                }
            }

            // Inject fault and render (the Shadow cells render a
            // SHADOWED frame — the only path that builds the shadow
            // state; a plain corner frame would never fire the plan).
            renderer.inject_target_fault(site, *stage, 1);
            let result = match site {
                TargetFaultSite::Shadow => {
                    renderer.render_corner_shadowed(&matrix, high_eye, day_preset(), BIAS, size)
                }
                TargetFaultSite::Post | TargetFaultSite::Wet => {
                    renderer.render_corner(&matrix, high_eye, day_preset(), size)
                }
            };

            // Assert error.
            assert!(
                matches!(result, Err(RendererError::InjectedFault(_))),
                "site {:?} stage {:?}: expected InjectedFault, got {:?}",
                site,
                stage,
                result
            );

            // The count comparison: the site's live census equals the
            // pre-attempt baseline exactly (Post/Shadow built nothing
            // before the fault — zero; Wet unwound its partial back to
            // the surviving post bundle).
            let post_fault = renderer.allocations();
            for kind in ["images", "views", "framebuffers", "memories"] {
                assert_eq!(
                    live(&post_fault, kind),
                    live(&pre, kind),
                    "site {:?} stage {:?}: live {} must equal the pre-attempt census",
                    site,
                    stage,
                    kind
                );
            }

            // Recovery render (same path, no plan armed).
            let result = match site {
                TargetFaultSite::Shadow => {
                    renderer.render_corner_shadowed(&matrix, high_eye, day_preset(), BIAS, size)
                }
                TargetFaultSite::Post | TargetFaultSite::Wet => {
                    renderer.render_corner(&matrix, high_eye, day_preset(), size)
                }
            };
            assert!(
                result.unwrap().presented,
                "site {:?} stage {:?}: recovery should succeed",
                site,
                stage
            );

            // The site's live set grew past the baseline on recovery.
            let post_recovery = renderer.allocations();
            for kind in ["images", "views", "framebuffers", "memories"] {
                let l = live(&post_recovery, kind);
                assert!(
                    l > live(&pre, kind),
                    "site {:?} stage {:?}: live {} after recovery must exceed the \
                     pre-attempt census (actual: {})",
                    site,
                    stage,
                    kind,
                    l
                );
            }

            assert_eq!(
                renderer.stats().validation_errors,
                0,
                "site {:?} stage {:?}: validation errors: {:?}",
                site,
                stage,
                renderer.stats().recent_validation
            );

            println!(
                "site {:?} stage {:?}: census before {:?}, after fault {:?}, recovery live: images={}, views={}, framebuffers={}, memories={}",
                site,
                stage,
                (
                    pre.target_images,
                    pre.target_image_views,
                    pre.target_framebuffers,
                    pre.target_memories
                ),
                (
                    post_fault.target_images,
                    post_fault.target_image_views,
                    post_fault.target_framebuffers,
                    post_fault.target_memories
                ),
                live(&post_recovery, "images"),
                live(&post_recovery, "views"),
                live(&post_recovery, "framebuffers"),
                live(&post_recovery, "memories")
            );
        }
    }

    // Late-ordinal cells (earlier successfully constructed slots also unwind).
    println!("=== PHASE 2: late ordinal cells ===");

    let late_cells = [
        (TargetFaultSite::Post, TargetFaultStage::Image, 5),
        (TargetFaultSite::Wet, TargetFaultStage::View, 3),
        (TargetFaultSite::Shadow, TargetFaultStage::View, 2),
    ];

    for (site, stage, ordinal) in late_cells {
        // (the underscore app still owns the window — see phase 1)
        let (_app, mut renderer) = new_renderer();
        renderer.set_sky(Some(SkyFrame {
            palette: day.sky,
            disc_dir: sun_dir,
        }));

        match site {
            TargetFaultSite::Post => {
                renderer.set_post(true);
            }
            TargetFaultSite::Wet => {
                renderer.set_post(true);
                let warm = renderer.render_corner(&matrix, high_eye, day_preset(), size);
                assert!(
                    warm.unwrap().presented,
                    "the late wet cell's post-only warm frame"
                );
                renderer.set_wet(true);
            }
            TargetFaultSite::Shadow => {
                // Shadowed renders below build the shadow state.
            }
        }

        // Baseline.
        let pre = renderer.allocations();
        match site {
            TargetFaultSite::Wet => {
                for kind in ["images", "views", "framebuffers", "memories"] {
                    assert!(
                        live(&pre, kind) > 0,
                        "late wet cell baseline: the post-only warm frame left {} live",
                        kind
                    );
                }
            }
            TargetFaultSite::Post | TargetFaultSite::Shadow => {
                assert_eq!(
                    (
                        pre.target_images,
                        pre.target_image_views,
                        pre.target_framebuffers,
                        pre.target_memories
                    ),
                    (0, 0, 0, 0),
                    "late baseline for site {:?} stage {:?} ordinal {}",
                    site,
                    stage,
                    ordinal
                );
            }
        }

        // Inject fault with higher ordinal (several successful slots before fault).
        renderer.inject_target_fault(site, stage, ordinal);
        let result = match site {
            TargetFaultSite::Shadow => {
                renderer.render_corner_shadowed(&matrix, high_eye, day_preset(), BIAS, size)
            }
            TargetFaultSite::Post | TargetFaultSite::Wet => {
                renderer.render_corner(&matrix, high_eye, day_preset(), size)
            }
        };

        assert!(
            matches!(result, Err(RendererError::InjectedFault(_))),
            "late site {:?} stage {:?} ordinal {}: expected InjectedFault, got {:?}",
            site,
            stage,
            ordinal,
            result
        );

        // All earlier-constructed handles still unwind (live equals the
        // pre-attempt census — the ordinal guarantees several complete
        // objects existed when the fault fired).
        let post_fault = renderer.allocations();
        for kind in ["images", "views", "framebuffers", "memories"] {
            assert_eq!(
                live(&post_fault, kind),
                live(&pre, kind),
                "late site {:?} stage {:?} ordinal {}: live {} must equal the \
                 pre-attempt census",
                site,
                stage,
                ordinal,
                kind
            );
        }

        // Recovery.
        let result = match site {
            TargetFaultSite::Shadow => {
                renderer.render_corner_shadowed(&matrix, high_eye, day_preset(), BIAS, size)
            }
            TargetFaultSite::Post | TargetFaultSite::Wet => {
                renderer.render_corner(&matrix, high_eye, day_preset(), size)
            }
        };
        assert!(
            result.unwrap().presented,
            "late site {:?} stage {:?} ordinal {}: recovery should succeed",
            site,
            stage,
            ordinal
        );

        let post_recovery = renderer.allocations();
        for kind in ["images", "views", "framebuffers", "memories"] {
            let l = live(&post_recovery, kind);
            assert!(
                l > live(&pre, kind),
                "late site {:?} stage {:?} ordinal {}: live {} after recovery must \
                 exceed the pre-attempt census (actual: {})",
                site,
                stage,
                ordinal,
                kind,
                l
            );
        }

        assert_eq!(
            renderer.stats().validation_errors,
            0,
            "late site {:?} stage {:?} ordinal {}: validation errors: {:?}",
            site,
            stage,
            ordinal,
            renderer.stats().recent_validation
        );

        println!(
            "late site {:?} stage {:?} ordinal {}: census before {:?}, after fault {:?}, recovery live: images={}, views={}, framebuffers={}, memories={}",
            site,
            stage,
            ordinal,
            (
                pre.target_images,
                pre.target_image_views,
                pre.target_framebuffers,
                pre.target_memories
            ),
            (
                post_fault.target_images,
                post_fault.target_image_views,
                post_fault.target_framebuffers,
                post_fault.target_memories
            ),
            live(&post_recovery, "images"),
            live(&post_recovery, "views"),
            live(&post_recovery, "framebuffers"),
            live(&post_recovery, "memories")
        );
    }
}

/// Edge case test for tonemap and bright_pass: the shader's branch
/// structure must match the CPU's (nonpositive → 0, positive → rational).
#[test]
fn tonemap_and_bright_pass_edge_cases() {
    // Tonemap: negative/zero → 0, positive → rational.
    assert_eq!(tonemap_rgb([-0.5, 0.0, 0.18]), [0.0, 0.0, tonemap(0.18)]);
    // Bright pass: negative/zero/below threshold → 0, above → identity.
    assert_eq!(bright_pass(-0.5), 0.0);
    assert_eq!(bright_pass(0.0), 0.0);
    assert_eq!(bright_pass(0.5), 0.0);
    assert_eq!(bright_pass(1.0), 0.0);
    assert!((bright_pass(2.0) - 2.0).abs() < 1e-6);
}
