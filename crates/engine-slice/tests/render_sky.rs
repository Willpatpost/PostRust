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
use engine_slice::corner_demo::{corner_submission, view_projection};
use engine_slice::corner_scene::{attach_corner_scene, corner_bounds};
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
    // The horizontal tangent carries the ASPECT (perspective's
    // matrix[0][0] = focal/aspect: a point at NDC nx lies at
    // x/(−z) = nx·aspect·tan_half in view space — the vertical
    // term needs no factor). The square-frame poses above pass
    // aspect 1.0 where the factor is exactly one; the non-square
    // off-axis disc-edge block is the coverage that distinguishes
    // them (the review's own disposition note).
    let aspect = size.0 as f32 / size.1 as f32;
    let mut ray = std::array::from_fn(|axis| {
        forward[axis] + nx * tan_half * aspect * right[axis] + math_y * tan_half * up[axis]
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
            demo_scene: DemoScenePreference::Omitted,
            validation: ValidationPreference::On,
            replace_pipelines: true,
            ..Default::default()
        },
    )
    .unwrap();
    // P4 S6: the corner content is ATTACHED (the Corner
    // construction preference is deleted).
    attach_corner_scene(&mut renderer).unwrap();
    let size = app.window().size_in_pixels();
    assert_eq!(size, (256, 256));
    assert!(renderer.stats().validation_active);
    let _ = app.frame();

    // THE PER-FRAME SKY DECLARATION (P4 S6: the deleted set_sky
    // sticky's state, carried by each submission — the same
    // discipline the corner demo's route carries): the known-answer
    // poses below assign the palette before the first submission.
    let mut sky: Option<SkyFrame>;

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
    sky = Some(SkyFrame {
        palette: day.sky,
        disc_dir: sun_dir,
    });
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
                .submit_frame(corner_submission(
                    &matrix,
                    position,
                    day_preset(),
                    None,
                    corner_bounds(),
                    None,
                    sky,
                    false,
                    None,
                    size
                ))
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
    sky = Some(SkyFrame {
        palette: night.sky,
        disc_dir: night.disc_dir,
    });
    let toward = std::array::from_fn(|i| high_eye[i] + night.disc_dir[i]);
    let matrix = aimed(high_eye, toward);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .submit_frame(corner_submission(
                &matrix,
                high_eye,
                night_preset(),
                None,
                corner_bounds(),
                None,
                sky,
                false,
                None,
                size
            ))
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
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .submit_frame(corner_submission(
                &ground_matrix,
                [0.0, 1.7, 0.0],
                day_preset(),
                None,
                corner_bounds(),
                None,
                sky,
                false,
                None,
                size
            ))
            .unwrap()
            .presented
    );
    let without_sky = renderer.take_frame_capture().unwrap().unwrap();
    sky = Some(SkyFrame {
        palette: day.sky,
        disc_dir: sun_dir,
    });
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .submit_frame(corner_submission(
                &ground_matrix,
                [0.0, 1.7, 0.0],
                day_preset(),
                None,
                corner_bounds(),
                None,
                sky,
                false,
                None,
                size
            ))
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
            .submit_frame(corner_submission(
                &matrix,
                pose.position,
                day_preset(),
                None,
                corner_bounds(),
                None,
                sky,
                false,
                None,
                size
            ))
            .unwrap()
            .presented
    );
    let sky_visible = renderer.take_frame_capture().unwrap().unwrap();
    sky = None;
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .submit_frame(corner_submission(
                &matrix,
                pose.position,
                day_preset(),
                None,
                corner_bounds(),
                None,
                sky,
                false,
                None,
                size
            ))
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
    sky = Some(SkyFrame {
        palette: day.sky,
        disc_dir: sun_dir,
    });
    for _ in 0..2 {
        renderer.request_frame_capture().unwrap();
        assert!(
            renderer
                .submit_frame(corner_submission(
                    &matrix,
                    pose.position,
                    day_preset(),
                    None,
                    corner_bounds(),
                    None,
                    sky,
                    false,
                    None,
                    size
                ))
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
            .submit_frame(corner_submission(
                &matrix,
                pose.position,
                day_preset(),
                Some(engine_slice::corner_demo::demo_bias()),
                corner_bounds(),
                None,
                sky,
                false,
                None,
                size
            ))
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
    // THE OFF-AXIS DISC EDGE AT A NON-SQUARE ASPECT (the Era-1
    // review's gate-by-gate disposition: "An off-axis disc edge at
    // a non-square aspect would extend the angular coverage" — the
    // square-frame disc-center patches above cannot see an aspect
    // error in the ray reconstruction: at 1.0 the x and y scalings
    // are identical, so a swapped or miscomputed aspect cancels).
    // A SECOND renderer at 264×200 (aspect 1.32), the view axis
    // offset ~8° from the sun in a COMPOUND direction (both screen
    // axes carry the offset — a single-axis offset could still
    // cancel a one-axis reconstruction error), so the disc lands
    // clearly off-center with its soft edge inside the probed
    // patch; every patch pixel against sky_color of ITS OWN
    // reconstructed ray at the non-square extent. (The first app
    // is dropped first: SDL allows one EventPump alive at a time —
    // the depth-fallback loop's own precedent. Everything on the
    // first renderer is asserted by this point.)
    drop(renderer);
    drop(app);
    {
        let app2 = ShellApp::new(ShellConfig {
            title: "Era 1 sky off-axis".into(),
            width: 264,
            height: 200,
            target_fps: 60,
            driver: DriverPreference::Real,
        })
        .unwrap();
        let mut renderer2 = Renderer::new(
            app2.window(),
            RenderConfig {
                demo_scene: DemoScenePreference::Omitted,
                validation: ValidationPreference::On,
                ..Default::default()
            },
        )
        .unwrap();
        attach_corner_scene(&mut renderer2).unwrap();
        let size2 = app2.window().size_in_pixels();
        assert_eq!(size2, (264, 200));
        // The aim: the sun direction perturbed ~8° toward a
        // compound perpendicular (east-and-up in the sun's own
        // frame — both tangent components nonzero).
        let sun = day.disc_dir;
        // A horizontal perpendicular to the sun direction (the
        // world-up cross, normalized; nonzero for any non-vertical
        // sun).
        let east = {
            let raw = [-sun[2], 0.0, sun[0]];
            let len = (raw[0] * raw[0] + raw[1] * raw[1] + raw[2] * raw[2]).sqrt();
            [raw[0] / len, raw[1] / len, raw[2] / len]
        };
        let north_of_east = [
            sun[1] * east[2] - sun[2] * east[1],
            sun[2] * east[0] - sun[0] * east[2],
            sun[0] * east[1] - sun[1] * east[0],
        ];
        let offset_deg = 8_f32.to_radians();
        let (sin_o, cos_o) = offset_deg.sin_cos();
        let tangent = [
            east[0] + north_of_east[0],
            east[1] + north_of_east[1],
            east[2] + north_of_east[2],
        ];
        let tangent_len =
            (tangent[0] * tangent[0] + tangent[1] * tangent[1] + tangent[2] * tangent[2]).sqrt();
        let aim_dir: [f32; 3] =
            std::array::from_fn(|axis| sun[axis] * cos_o + (tangent[axis] / tangent_len) * sin_o);
        let eye2 = [0.0, 25.0, 0.0];
        let toward2 = std::array::from_fn(|i| eye2[i] + aim_dir[i]);
        let matrix2 = view_projection(
            RouteWaypoint {
                position: eye2,
                look_at: toward2,
            },
            size2.0 as f32 / size2.1 as f32,
        );
        let sky2 = Some(SkyFrame {
            palette: day.sky,
            disc_dir: sun,
        });
        renderer2.request_frame_capture().unwrap();
        assert!(
            renderer2
                .submit_frame(corner_submission(
                    &matrix2,
                    eye2,
                    day_preset(),
                    None,
                    corner_bounds(),
                    None,
                    sky2,
                    false,
                    None,
                    size2
                ))
                .unwrap()
                .presented
        );
        let image2 = renderer2.take_frame_capture().unwrap().unwrap();
        let (forward2, right2, up2) = basis(eye2, toward2);
        // The projected disc center: the pixel whose reconstructed
        // ray dots highest with the sun direction (deterministic —
        // the frame is pure sky around the disc).
        let mut best = (0u32, 0u32, -1.0f32);
        for y in 0..size2.1 {
            for x in 0..size2.0 {
                let ray = pixel_ray(forward2, right2, up2, x, y, size2);
                let dot = ray[0] * sun[0] + ray[1] * sun[1] + ray[2] * sun[2];
                if dot > best.2 {
                    best = (x, y, dot);
                }
            }
        }
        let (cx, cy, _) = best;
        assert!(
            cx != size2.0 / 2 || cy != size2.1 / 2,
            "the disc is genuinely off-axis (projected center ({cx},{cy}) vs the frame \
             center ({},{}))",
            size2.0 / 2,
            size2.1 / 2
        );
        // The 13×13 patch around the projected center: the disc's
        // interior, its full soft edge, and the sky beyond it — at
        // the non-square extent, off-axis.
        let mut edge_checked = 0usize;
        let mut edge_worst = 0u8;
        let mut ramp_pixels = 0usize;
        for y in cy.saturating_sub(6)..=(cy + 6).min(size2.1 - 1) {
            for x in cx.saturating_sub(6)..=(cx + 6).min(size2.0 - 1) {
                let ray = pixel_ray(forward2, right2, up2, x, y, size2);
                let expected = quantized(sky_color(ray, &day.sky, sun));
                let pixel = image2.pixel_rgba(x, y);
                let error = expected
                    .iter()
                    .zip(pixel)
                    .map(|(e, a)| e.abs_diff(a))
                    .max()
                    .unwrap();
                edge_worst = edge_worst.max(error);
                assert!(
                    error <= 2,
                    "off-axis disc-edge pixel ({x},{y}) ray {ray:?}: expected {expected:?}, \
                     got {pixel:?} (a non-square ray-reconstruction error moves the disc's \
                     edge off its expected ramp position)"
                );
                assert_eq!(pixel[3], 255);
                // The self-discrimination control: the patch must
                // actually straddle the RAMP (pixels whose expected
                // color is neither pure gradient nor saturated disc
                // — the edge itself), or the check is vacuous for
                // the finding it exists to catch.
                let gradient = sky_color(ray, &day.sky, [0.0, 1.0, 0.0]);
                let disc_delta = expected
                    .iter()
                    .zip(quantized(gradient))
                    .map(|(e, g)| e.abs_diff(g))
                    .max()
                    .unwrap();
                if (1..=254).contains(&disc_delta) {
                    ramp_pixels += 1;
                }
                edge_checked += 1;
            }
        }
        assert!(
            ramp_pixels >= 8,
            "the off-axis patch straddles the disc's soft edge ({ramp_pixels} ramp pixels \
             of {edge_checked} checked)"
        );
        assert_eq!(renderer2.stats().validation_errors, 0);
        println!(
            "off-axis disc edge (264×200): {edge_checked} patch pixels around the projected \
             center ({cx},{cy}), {ramp_pixels} on the soft ramp, max error {edge_worst}/255"
        );
        total_checked += edge_checked;
    }

    println!(
        "Era-1 sky gates: {total_checked} day pixels + the night disc vs sky_color within 2/255, \
         the covered frame byte-identical, the background replaced, the sky state paid once"
    );
}
