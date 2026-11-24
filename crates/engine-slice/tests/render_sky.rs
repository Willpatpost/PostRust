//! The Era-1 sky gates (see docs/CYCLE6-ERA1-BRIEF.md, package 3):
//! the known-answer pixel checks at the gradient extremes and the
//! disc center against the CPU function (`sky::sky_color` — the 6-B
//! oracle discipline), the composition proof (the sky changes ONLY
//! the uncovered background — a fully-covered frame is
//! byte-identical with the sky on and off), and the lazily-created
//! sky state's allocation delta and warmed constancy.
//!
//! `#[ignore]`d like the other lavapipe smokes: it requires
//! `SDL_VIDEODRIVER=offscreen` plus a software Vulkan ICD (lavapipe)
//! and the validation layer. CI's Linux runner executes it enforced
//! (the workflow's sky-gate step); the per-stage `println!` evidence
//! below is what the CI log must show. Run explicitly:
//!
//! ```sh
//! SDL_VIDEODRIVER=offscreen cargo test -p engine-slice --test render_sky -- \
//!     --ignored --nocapture
//! ```

use engine_renderer::sky::{atmosphere_at, sky_color};
use engine_renderer::{
    DemoScenePreference, RenderConfig, Renderer, SkyFrame, ValidationPreference,
};
use engine_shell::{DriverPreference, ShellApp, ShellConfig};

use engine_renderer::lit_scene::RouteWaypoint;
use engine_renderer::lit_scene::{day_preset, night_preset};
use engine_slice::corner_demo::view_projection;
use engine_slice::math::{camera_forward, camera_right};

/// One aimed camera: the corner route's own projection (65°, near
/// 0.1 / far 150) from a position toward a look-at target — the
/// screen-center ray is definitionally the normalized target offset
/// (the known-answer property the inverse's unit test pins).
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
/// derived): forward, right, and up (the rotation's column 1 —
/// roll is zero on this path).
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
/// from the camera basis — the same ray the shader's inverse-based
/// reconstruction produces; the y flip is the raster row's, matching
/// the compiler's position adjustment the orientation pin guards).
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
    // Raster row 0 is the frame's top; the math-NDC y carries the
    // negated raster ny (the compiler's y adjustment on the emitted
    // position — the A4 orientation pin's subject).
    let math_y = -ny;
    let tan_half = (65_f32.to_radians() / 2.0).tan();
    let mut ray = std::array::from_fn(|axis| {
        forward[axis] + nx * tan_half * right[axis] + math_y * tan_half * up[axis]
    });
    let length = (ray[0] * ray[0] + ray[1] * ray[1] + ray[2] * ray[2]).sqrt();
    ray = ray.map(|component| component / length);
    ray
}

fn quantized(color: [f32; 3]) -> [u8; 3] {
    // The LDR swapchain's UNORM write saturates at 1.0 (the disc's
    // HDR headroom above 1 clips — the post-processing package's HDR
    // target restores it; documented).
    color.map(|c| (255. * c.clamp(0., 1.)).round() as u8)
}

#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup (see render_overflow)"]
fn sky_pixels_match_sky_color_and_never_touch_covered_geometry() {
    let mut app = ShellApp::new(ShellConfig {
        title: "Era 1 sky".into(),
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

    // THE LAZY PROOF: the skyless corner's allocation pins hold
    // BEFORE the first sky frame (the skyless path pays nothing).
    let skyless = renderer.allocations();
    assert_eq!(
        (
            skyless.texture_memory,
            skyless.descriptor_pools,
            skyless.descriptor_sets,
            skyless.samplers
        ),
        (4, 2, 4, 2),
        "the pre-sky corner pins (render_corner's own)"
    );

    // THE KNOWN-ANSWER POSES (the brief's gradient extremes and the
    // disc center): a 5×5 patch around the screen center per pose —
    // every pixel against sky_color of ITS OWN CPU-reconstructed
    // ray (the saturated regions make the sun/zenith poses robust;
    // the horizon pose exercises the mid-ramp through the per-pixel
    // rays). The day palette with the day disc.
    // THE SUN POSE SITS ABOVE THE MASSES (the package-6 first-run
    // correction): from the street-level origin the sun at 45°
    // elevation toward the SOUTHWEST lands INSIDE the southwest
    // building's silhouette (the ray enters its column at 10.2 m <
    // the 22 m top — occluded on ANY correct renderer), so the disc
    // center is checked from an eye above the tallest mass instead
    // (25 m > 22 m: the whole frame is pure sky, the disc dead
    // center). The gradient poses (zenith, horizon) look along the
    // OPEN road axes and stay at the street-level eye.
    let day = atmosphere_at(0.0);
    let sun_dir = day.disc_dir;
    renderer.set_sky(Some(SkyFrame {
        palette: day.sky,
        disc_dir: sun_dir,
    }));
    let eye = [0.0, 1.7, 0.0];
    let high_eye = [0.0, 25.0, 0.0];
    let poses = [
        (
            "at the sun",
            high_eye,
            std::array::from_fn(|i| high_eye[i] + sun_dir[i]),
        ),
        // Steeply up (~63° elevation): the gradient's ramp saturates
        // — the pure zenith extreme (the sun sits off this axis, no
        // disc).
        ("zenith", eye, [0.0, 3.7, -1.0]),
        // Level along +X: the ramp's mid-band — the horizon extreme.
        ("horizon", eye, [1.0, 1.7, 0.0]),
    ];
    let mut total_checked = 0usize;
    let mut worst_error = 0u8;
    for (label, position, toward) in poses {
        let matrix = aimed(position, toward);
        renderer.request_frame_capture().unwrap();
        assert!(
            renderer
                .render_corner(&matrix, position, day_preset(), size)
                .unwrap()
                .presented
        );
        let image = renderer.take_frame_capture().unwrap().unwrap();
        let (forward, right, up) = basis(position, toward);
        let center = size.0 / 2;
        for y in center - 2..=center + 2 {
            for x in center - 2..=center + 2 {
                let ray = pixel_ray(forward, right, up, x, y, size);
                let expected = quantized(sky_color(ray, &day.sky, sun_dir));
                let pixel = image.pixel_rgba(x, y);
                let error = expected
                    .iter()
                    .zip(pixel)
                    .map(|(e, a)| e.abs_diff(a))
                    .max()
                    .unwrap();
                worst_error = worst_error.max(error);
                assert!(
                    error <= 2,
                    "{label} pixel ({x},{y}) ray {ray:?}: expected {expected:?}, got {pixel:?}"
                );
                assert_eq!(pixel[3], 255);
                total_checked += 1;
            }
        }
        println!(
            "{label}: 25 center pixels vs sky_color of their own rays, max error {worst_error}/255"
        );
    }

    // THE NIGHT DISC: the moon through the night palette (the same
    // high-eye discipline — the moon at ~61° elevation toward the
    // northeast sits inside the northeast mass's silhouette from
    // the street-level origin, so the check rides the above-mass
    // eye like the sun pose).
    let night = atmosphere_at(1.0);
    renderer.set_sky(Some(SkyFrame {
        palette: night.sky,
        disc_dir: night.disc_dir,
    }));
    let toward = std::array::from_fn(|i| high_eye[i] + night.disc_dir[i]);
    let matrix = aimed(high_eye, toward);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, high_eye, night_preset(), size)
            .unwrap()
            .presented
    );
    let image = renderer.take_frame_capture().unwrap().unwrap();
    let (forward, right, up) = basis(high_eye, toward);
    let center = size.0 / 2;
    let ray = pixel_ray(forward, right, up, center, center, size);
    let expected = quantized(sky_color(ray, &night.sky, night.disc_dir));
    let pixel = image.pixel_rgba(center, center);
    assert!(
        expected
            .iter()
            .zip(pixel)
            .map(|(e, a)| e.abs_diff(a))
            .max()
            .unwrap()
            <= 2,
        "the moon at the night disc center: expected {expected:?}, got {pixel:?}"
    );
    println!("night moon: the disc-center pixel vs sky_color, within 2/255");

    // THE COMPOSITION PROOF: a fully-covered frame (the ground
    // camera looks straight down — every pixel is the corner mesh's
    // ground) is BYTE-IDENTICAL with the sky on and off; a
    // sky-visible pose differs only in the uncovered background.
    let ground_matrix = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, -0.125, 0.0],
        [0.0, -1.0, 0.0, 0.0],
        [0.0, 0.0, 0.5, 1.0],
    ];
    renderer.set_sky(None);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&ground_matrix, [0.0, 1.7, 0.0], day_preset(), size)
            .unwrap()
            .presented
    );
    let without_sky = renderer.take_frame_capture().unwrap().unwrap();
    renderer.set_sky(Some(SkyFrame {
        palette: day.sky,
        disc_dir: sun_dir,
    }));
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&ground_matrix, [0.0, 1.7, 0.0], day_preset(), size)
            .unwrap()
            .presented
    );
    let with_sky = renderer.take_frame_capture().unwrap().unwrap();
    assert_eq!(with_sky.format, without_sky.format);
    assert_eq!(
        with_sky.bytes, without_sky.bytes,
        "the fully-covered frame is byte-identical with the sky on and off"
    );
    // The sky-visible pose: the route's entry waypoint (the
    // buildings' silhouettes against the sky).
    let route = engine_renderer::lit_scene::camera_route();
    let pose = route[0];
    let matrix = view_projection(pose, 1.0);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, pose.position, day_preset(), size)
            .unwrap()
            .presented
    );
    let sky_visible = renderer.take_frame_capture().unwrap().unwrap();
    renderer.set_sky(None);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, pose.position, day_preset(), size)
            .unwrap()
            .presented
    );
    let plain = renderer.take_frame_capture().unwrap().unwrap();
    let mut differ = 0usize;
    for y in 0..size.1 {
        for x in 0..size.0 {
            differ += usize::from(sky_visible.pixel_rgba(x, y) != plain.pixel_rgba(x, y));
        }
    }
    println!(
        "composition: the covered frame byte-identical; the sky-visible pose differs in {differ} of {} pixels (the background)",
        size.0 * size.1
    );
    assert!(differ > 2_000, "the sky visibly replaces the background");

    // THE ALLOCATION DELTA (the lazily-created sky state): exactly
    // one mapped uniform (buffer + memory), one pool, one set —
    // no textures, no samplers. The device_memory delta is TWO on
    // the first execution (found when the gate finally ran): the
    // sky state's uniform AND the frame-capture machinery's
    // destination buffer, which allocates lazily at THIS test's
    // first capture request (the snapshot above precedes any
    // capture — the skyless corner run never requested one). The
    // capture buffer is the orientation package's own one-time
    // cost, not the sky state's — documented here because this
    // test is the first in the line to snapshot before capturing.
    // Then WARMED CONSTANCY across repeated sky frames
    // (byte-identical captures with them).
    let warm = renderer.allocations();
    assert_eq!(
        (
            warm.device_memory - skyless.device_memory,
            warm.descriptor_pools - skyless.descriptor_pools,
            warm.descriptor_sets - skyless.descriptor_sets,
            warm.samplers - skyless.samplers,
            warm.texture_memory - skyless.texture_memory
        ),
        (2, 1, 1, 0, 0),
        "the sky state's and the capture buffer's one-time allocation deltas"
    );
    renderer.set_sky(Some(SkyFrame {
        palette: day.sky,
        disc_dir: sun_dir,
    }));
    for _ in 0..2 {
        renderer.request_frame_capture().unwrap();
        assert!(
            renderer
                .render_corner(&matrix, pose.position, day_preset(), size)
                .unwrap()
                .presented
        );
        let repeat = renderer.take_frame_capture().unwrap().unwrap();
        assert_eq!(
            renderer.allocations(),
            warm,
            "warmed sky allocations constant"
        );
        assert_eq!(repeat.bytes, sky_visible.bytes, "byte-identical sky frames");
    }
    println!(
        "warmed sky allocations: {warm:?} (delta over skyless: +2 memory counting the capture buffer, +1 pool, +1 set)"
    );

    // The shadowed path composes with the sky (one presented frame,
    // clean validation — the full shadow gates are the shadow
    // package's own).
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner_shadowed(
                &matrix,
                pose.position,
                day_preset(),
                engine_slice::corner_demo::demo_bias(),
                size
            )
            .unwrap()
            .presented
    );
    let _ = renderer.take_frame_capture().unwrap().unwrap();

    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "{:?}",
        renderer.stats().recent_validation
    );
    println!(
        "Era-1 sky gates: {total_checked} day pixels + the night disc vs sky_color within 2/255, \
         the covered frame byte-identical, the background replaced, the sky state paid once"
    );
}
