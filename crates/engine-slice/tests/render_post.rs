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

use engine_renderer::post::{bloom_at_pixel, bloom_mip_from_hdr, post_pixel, tonemap_rgb};
use engine_renderer::sky::{atmosphere_at, sky_color};
use engine_renderer::{
    DemoScenePreference, DepthFormatPreference, RenderConfig, Renderer, SkyFrame,
    ValidationPreference,
};
use engine_shell::{DriverPreference, ShellApp, ShellConfig};

use engine_renderer::lit_scene::RouteWaypoint;
use engine_renderer::lit_scene::day_preset;
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
        let ray = pixel_ray(forward, right, up, x, y, size);
        let no_bloom = quantized(tonemap_rgb(sky_color(ray, &day.sky, sun_dir)));
        let lift = expected
            .iter()
            .zip(no_bloom)
            .map(|(e, n)| (*e as i32) - (n as i32))
            .max()
            .unwrap();
        ring_lift = ring_lift.max(lift);
        assert!(
            lift >= 2,
            "bloom-ring pixel ({x},{y}): the bloom lifts it ({lift}/255): {expected:?} vs {no_bloom:?}"
        );
    }
    println!(
        "bloom ring: 4 pixels vs the oracle within {TOLERANCE}/255, lifted ≥2/255 above the no-bloom tonemap (max lift {ring_lift}/255)"
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
    let population = engine_renderer::ProxyPopulation {
        npc: &npc,
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
    fn check_probes(image: &[[u8; 4]], oracle: &[[u8; 4]], size: (u32, u32), what: &str) {
        let center = size.0 / 2;
        let mut max_error = 0u8;
        let mut checked = 0usize;
        for y in center - 2..=center + 2 {
            for x in center - 2..=center + 2 {
                let expected = oracle[(y * size.0 + x) as usize];
                let actual = image[(y * size.0 + x) as usize];
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
                    let actual = image[(y * size.0 + x) as usize];
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
            posted.bytes.as_chunks::<4>().0,
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
            resized_capture.bytes.as_chunks::<4>().0,
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
        reference.bytes.as_chunks::<4>().0,
        &reference_oracle,
        size,
        "post lifecycle reference",
    );
    let warm = renderer.allocations();

    // Post OFF, then two recreations — away and BACK to the original
    // extent — then post ON again.
    renderer.set_post(false);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, high_eye, day_preset(), size)
            .unwrap()
            .presented
    );
    let _ = renderer.take_frame_capture().unwrap().unwrap();
    let away = (size.0 + 8, size.1 + 8);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, high_eye, day_preset(), away)
            .unwrap()
            .presented
    );
    let _ = renderer.take_frame_capture().unwrap().unwrap();
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
        restored.bytes.as_chunks::<4>().0,
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
    assert_eq!(
        renderer.allocations(),
        warm,
        "the post lifecycle grew no allocations across two recreations"
    );
    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "{:?}",
        renderer.stats().recent_validation
    );
    println!(
        "post lifecycle: restored at the original extent through the oracle, allocations \
         unchanged, validation clean"
    );
}
