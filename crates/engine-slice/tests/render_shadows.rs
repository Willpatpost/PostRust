//! The Cycle 6-F shadow gates (see docs/AUTONOMOUS-CYCLE-PROTOCOL.md
//! §2 item 2): the shadow map's depth values pinned against the CPU
//! raycaster (the known-answer check — every texel is the deterministic
//! core's `map_texel_depth`), and the shadowed/unshadowed pixel
//! equivalence (the render_corner oracle discipline applied to the
//! shadow term: a fully-shadowed ground patch renders exactly
//! `albedo · ambient`, a fully-lit patch renders exactly the
//! unshadowed equation, and the shadow visibly changes the frame).
//!
//! `#[ignore]`d like the other lavapipe smokes: it requires
//! `SDL_VIDEODRIVER=offscreen` plus a software Vulkan ICD (lavapipe)
//! and the validation layer. The dev pod needs the user-local ICD
//! setup (see render_overflow). CI's Linux runner executes it
//! enforced (the workflow's shadow-gate step); the per-stage
//! `println!` evidence below is what the CI log must show. Run
//! explicitly:
//!
//! ```sh
//! SDL_VIDEODRIVER=offscreen cargo test -p engine-slice --test render_shadows -- \
//!     --ignored --nocapture
//! ```

use engine_renderer::lit_scene::{
    LightParams, day_preset, intersection_boxes, lit_surface_color, materials,
};
use engine_renderer::proc_texture::{TEXTURE_SIZE, procedural_texture};
use engine_renderer::shadows::{
    self, ShadowBias, ShadowCascade, is_shadowed, map_texel_depth, scene_bounds,
};
use engine_renderer::{
    CapturedFrame, DemoScenePreference, RenderConfig, Renderer, ValidationPreference,
};
use engine_shell::{DriverPreference, ShellApp, ShellConfig};

/// The bias under test (the demo's own policy — one source for both).
const BIAS: ShadowBias = ShadowBias {
    constant: 0.0015,
    slope_scale: 0.002,
};

fn corner_cascade(light: LightParams) -> ShadowCascade {
    let boxes = intersection_boxes();
    let (min, max) = scene_bounds(&boxes);
    shadows::cascade_for(light.dir_toward_light, min, max)
}

/// The ground-patch orthographic camera (the render_corner
/// `check_patch` shape, centered on an arbitrary patch): NDC.x =
/// (x−cx)/h, NDC.y = −(z−cz)/h, depth −y + 0.5 (0.5 on the ground),
/// eye straight above the center.
fn ground_camera(center: [f32; 2], half: f32) -> ([[f32; 4]; 4], [f32; 3]) {
    let matrix = [
        [1.0 / half, 0.0, 0.0, 0.0],
        [0.0, 0.0, -1.0, 0.0],
        [0.0, -1.0 / half, 0.0, 0.0],
        [-center[0] / half, center[1] / half, 0.5, 1.0],
    ];
    (matrix, [center[0], 1.7, center[1]])
}

fn capture_corner(
    renderer: &mut Renderer,
    matrix: &[[f32; 4]; 4],
    eye: [f32; 3],
    light: LightParams,
    size: (u32, u32),
) -> CapturedFrame {
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(matrix, eye, light, size)
            .unwrap()
            .presented
    );
    renderer.take_frame_capture().unwrap().unwrap()
}

fn capture_shadowed(
    renderer: &mut Renderer,
    matrix: &[[f32; 4]; 4],
    eye: [f32; 3],
    light: LightParams,
    size: (u32, u32),
) -> CapturedFrame {
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner_shadowed(matrix, eye, light, BIAS, size)
            .unwrap()
            .presented
    );
    renderer.take_frame_capture().unwrap().unwrap()
}

/// The full known-answer map comparison: every texel's GPU depth vs
/// the CPU raycaster's, skipping (and counting) texels whose hit
/// sits within `margin` texels of its surface's edges (rasterization
/// coverage at edges is implementation territory; interior texels
/// are exact). Returns (compared, skipped, mismatches, max_error).
fn compare_map(
    cascade: &ShadowCascade,
    boxes: &[engine_renderer::lit_scene::SceneBox],
    depths: &[f32],
    extent: u32,
    tolerance: f32,
    margin_texels: f32,
) -> (usize, usize, usize, f32) {
    let boxes_table = intersection_boxes();
    let texel_world = (cascade.max_right - cascade.min_right).max(cascade.max_up - cascade.min_up)
        / extent as f32;
    let mut compared = 0;
    let mut skipped = 0;
    let mut mismatches = 0;
    let mut max_error = 0.0f32;
    for y in 0..extent {
        for x in 0..extent {
            let expected = map_texel_depth(cascade, boxes, x, y, extent);
            let Some(hit) = expected.hit else {
                // Clear texel: no surface at all — exact.
                let gpu = depths[(y * extent + x) as usize];
                if (gpu - 1.0).abs() > tolerance {
                    mismatches += 1;
                }
                compared += 1;
                continue;
            };
            // The edge margin: the hit's distance to its surface's
            // PERIMETER — the two axes tangential to the hit face.
            // The face axis itself is where the hit lies (distance
            // zero by definition): infer it as the axis whose bound
            // the hit touches, then measure the other two. For the
            // ground quad the face axis is y. Rasterization coverage
            // at perimeters is implementation territory; interior
            // texels are exact.
            let (bounds, axes): ([[f32; 3]; 2], [usize; 3]) = match expected.box_index {
                Some(index) => {
                    let box_ = &boxes_table[index];
                    ([box_.min, box_.max], [0, 1, 2])
                }
                None => ([[-40.0, 0.0, -40.0], [40.0, 0.0, 40.0]], [1, 1, 1]),
            };
            let mut on_face = 0;
            let mut tangential = [true; 3];
            for &axis in &axes {
                if (bounds[0][axis] - hit[axis]).abs() < 1e-3
                    || (bounds[1][axis] - hit[axis]).abs() < 1e-3
                {
                    tangential[axis] = false;
                    on_face += 1;
                }
            }
            if on_face >= 2 {
                // A corner-edge graze (the hit touches bounds on two
                // axes — a roof/wall edge): the texel's coverage is
                // genuinely ambiguous between the meeting surfaces
                // and whatever lies past them. Implementation
                // territory — skip.
                skipped += 1;
                continue;
            }
            let mut min_edge = f32::MAX;
            for axis in 0..3 {
                if !tangential[axis] {
                    continue;
                }
                let lo = (bounds[0][axis] - hit[axis]).abs();
                let hi = (bounds[1][axis] - hit[axis]).abs();
                min_edge = min_edge.min(lo).min(hi);
            }
            // A hit touching no bound (numerically interior to all
            // three axes — the ray entered through a face it has
            // already left behind): the conservative full-margin
            // form.
            if min_edge == f32::MAX {
                for axis in 0..3 {
                    let lo = (bounds[0][axis] - hit[axis]).abs();
                    let hi = (bounds[1][axis] - hit[axis]).abs();
                    min_edge = min_edge.min(lo).min(hi);
                }
            }
            if min_edge < margin_texels * texel_world {
                skipped += 1;
                continue;
            }
            let gpu = depths[(y * extent + x) as usize];
            let error = (gpu - expected.depth).abs();
            max_error = max_error.max(error);
            if error > tolerance {
                mismatches += 1;
                if mismatches <= 10 {
                    println!(
                        "MISMATCH texel ({x},{y}): gpu {gpu:.6} vs cpu {:.6} (box {:?}, hit {:?})",
                        expected.depth, expected.box_index, expected.hit
                    );
                }
            }
            compared += 1;
        }
    }
    (compared, skipped, mismatches, max_error)
}

/// The per-pixel ground oracle for one patch (the check_patch
/// discipline): every interior pixel's world point is known exactly;
/// the expected color is the 6-B equation with the direct terms
/// multiplied by the CPU oracle's shadow verdict for that point.
fn check_ground_patch(
    renderer: &mut Renderer,
    shadowed: bool,
    center: [f32; 2],
    half: f32,
    light: LightParams,
    size: (u32, u32),
    cascade: &ShadowCascade,
) {
    let (matrix, eye) = ground_camera(center, half);
    // The fixture validates itself: all five probe points agree with
    // the requested regime (no magic placement without proof).
    let boxes = intersection_boxes();
    for (dx, dz) in [
        (0.0f32, 0.0f32),
        (0.9, 0.9),
        (0.9, -0.9),
        (-0.9, 0.9),
        (-0.9, -0.9),
    ] {
        let probe = [center[0] + dx * half, 0.0, center[1] + dz * half];
        assert_eq!(
            is_shadowed(probe, cascade, &boxes),
            shadowed,
            "the patch at {center:?} must be uniformly {} ({probe:?})",
            if shadowed { "shadowed" } else { "lit" }
        );
    }
    let image = if shadowed {
        capture_shadowed(renderer, &matrix, eye, light, size)
    } else {
        capture_corner(renderer, &matrix, eye, light, size)
    };
    let mat = materials()[0];
    let texture = procedural_texture(mat.texture);
    let mut largest_error = 0;
    let mut checked = 0;
    for y in 8..size.1 - 8 {
        for x in 8..size.0 - 8 {
            let dx = (x as f32 + 0.5) * 2. / size.0 as f32 - 1.;
            let dy = (y as f32 + 0.5) * 2. / size.1 as f32 - 1.;
            // The camera's col2 negates z into clip.y and the shader
            // epilogue negates it again — Vulkan's top row carries
            // the SMALL z — so the world point is cz + dy·h (the
            // sign the unshadowed check_patch form carries through
            // its own scale).
            let pos = [center[0] + dx * half, 0.0, center[1] + dy * half];
            let uv_axes = [pos[0], pos[2]];
            let [tx, ty] =
                uv_axes.map(|m| ((m / 4.).rem_euclid(1.) * TEXTURE_SIZE as f32).floor() as usize);
            let index = 4 * (ty * TEXTURE_SIZE as usize + tx);
            let albedo = std::array::from_fn(|c| texture[index + c] as f32 / 255.);
            let view = std::array::from_fn(|i| eye[i] - pos[i]);
            // The shadowed equation with the CPU oracle's verdict:
            // the direct terms zeroed in shadow (the shader's exact
            // form — ambient untouched, the sky reaches shadowed
            // surfaces), the full unshadowed equation in light.
            let expected = if shadowed {
                [
                    albedo[0] * light.ambient[0],
                    albedo[1] * light.ambient[1],
                    albedo[2] * light.ambient[2],
                ]
            } else {
                lit_surface_color(albedo, [0., 1., 0.], view, mat, light)
            }
            .map(|c| (255. * c.clamp(0., 1.)).round() as u8);
            let pixel = image.pixel_rgba(x, y);
            let error = expected
                .iter()
                .zip(pixel)
                .map(|(e, a)| e.abs_diff(a))
                .max()
                .unwrap();
            largest_error = largest_error.max(error);
            assert!(
                error <= 2,
                "patch {center:?} pixel ({x},{y}) world {pos:?}: expected {expected:?}, \
                 got {pixel:?}"
            );
            assert_eq!(pixel[3], 255);
            checked += 1;
        }
    }
    println!(
        "ground patch at {center:?} ({}): {checked} pixels, max RGB error {largest_error}/255",
        if shadowed { "shadowed" } else { "lit" }
    );
}

#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup (see render_overflow)"]
fn shadow_map_depths_match_the_raycaster_and_pixels_gate() {
    let mut app = ShellApp::new(ShellConfig {
        title: "Cycle 6-F directional shadows".into(),
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
    assert!(matches!(
        renderer.stats().surface_format.as_str(),
        "B8G8R8A8_UNORM" | "R8G8B8A8_UNORM"
    ));
    let _ = app.frame();
    let light = day_preset();

    // THE LAZY DISCIPLINE: the unshadowed corner's allocation pins
    // hold BEFORE the first shadowed frame (the unshadowed path pays
    // nothing for shadows).
    let unshadowed_warm = renderer.allocations();
    assert_eq!(
        (
            unshadowed_warm.texture_memory,
            unshadowed_warm.descriptor_pools,
            unshadowed_warm.descriptor_sets,
            unshadowed_warm.samplers
        ),
        (4, 2, 4, 2),
        "the pre-6-F corner pins (render_corner's own)"
    );

    // THE KNOWN-ANSWER MAP GATE: the first shadowed frame's map vs
    // the CPU raycaster, texel by texel.
    renderer.request_shadow_map_capture().unwrap();
    let (matrix, eye) = ground_camera([0.0, 0.0], 1.0);
    assert!(
        renderer
            .render_corner_shadowed(&matrix, eye, light, BIAS, size)
            .unwrap()
            .presented
    );
    let map = renderer.take_shadow_map_capture().unwrap().unwrap();
    assert_eq!(map.extent, shadows::SHADOW_MAP_EXTENT);
    assert_eq!(map.depths.len(), map.extent as usize * map.extent as usize);
    let boxes = intersection_boxes();
    let cascade = corner_cascade(light);
    let (compared, skipped, mismatches, max_error) =
        compare_map(&cascade, &boxes, &map.depths, map.extent, 1e-4, 1.5);
    let total = map.extent as usize * map.extent as usize;
    println!(
        "shadow map known-answer: {total} texels, {compared} compared, {skipped} edge-skipped \
         ({:.2}%), {mismatches} mismatches, max |Δdepth| {max_error:.2e}",
        100.0 * skipped as f32 / total as f32
    );
    assert_eq!(mismatches, 0, "every compared texel matches the raycaster");
    assert!(
        skipped * 20 < total,
        "the edge skip stays under 5% of the map ({skipped}/{total})"
    );
    // The vacuous-gate guard: the map holds real occluders AND clear
    // sky (a map that is all one or the other pins nothing).
    let shadowed_texels = map.depths.iter().filter(|&&d| d < 0.99).count();
    println!("shadowed map texels: {shadowed_texels} of {total}");
    assert!(shadowed_texels > total / 20, "occluder coverage");
    assert!(shadowed_texels < total, "clear sky present");

    // THE PIXEL EQUIVALENCE: the shadowed ground patch renders the
    // ambient-only equation; the lit patch renders the unshadowed
    // equation (the shadowed-vs-unshadowed comparison as exact
    // per-pixel oracles — both regimes in the day scene).
    check_ground_patch(&mut renderer, true, [0.0, 0.0], 1.0, light, size, &cascade);
    check_ground_patch(
        &mut renderer,
        false,
        [12.0, 3.0],
        0.75,
        light,
        size,
        &cascade,
    );

    // The frame-level differ: the shadow visibly changes the frame
    // (the ink control — the same camera, shadowed vs unshadowed).
    let plain = capture_corner(&mut renderer, &matrix, eye, light, size);
    let shadowed_frame = capture_shadowed(&mut renderer, &matrix, eye, light, size);
    let mut differ = 0;
    for y in 0..size.1 {
        for x in 0..size.0 {
            differ += usize::from(plain.pixel_rgba(x, y) != shadowed_frame.pixel_rgba(x, y));
        }
    }
    println!(
        "shadowed-vs-unshadowed frame differ: {differ} of {} pixels",
        size.0 * size.1
    );
    assert!(differ > 2_000, "the shadow visibly changes the frame");

    // THE ZERO-PER-FRAME-ALLOCATION GATE: warmed shadowed frames hold
    // the counters exactly (the shadow state is paid once).
    let warm = renderer.allocations();
    for _ in 0..4 {
        let image = capture_shadowed(&mut renderer, &matrix, eye, light, size);
        assert_eq!(
            renderer.allocations(),
            warm,
            "warmed shadowed allocations constant"
        );
        assert_frames_equal_bytes(&shadowed_frame, &image);
    }
    println!("warmed shadowed allocations: {warm:?}");

    // THE REPLACEMENT LIFECYCLE (finding A): the forced recreation
    // rebuilds the shadow state; the map known-answer holds on the
    // replacement, and the counters resume constant.
    let replacement = renderer
        .render_corner_shadowed(&matrix, eye, light, BIAS, (size.0 + 8, size.1 + 8))
        .unwrap();
    assert!(replacement.recreated, "the resize forced a recreation");
    renderer.request_shadow_map_capture().unwrap();
    assert!(
        renderer
            .render_corner_shadowed(&matrix, eye, light, BIAS, size)
            .unwrap()
            .presented
    );
    let map_after = renderer.take_shadow_map_capture().unwrap().unwrap();
    let (compared_after, skipped_after, mismatches_after, max_error_after) = compare_map(
        &cascade,
        &boxes,
        &map_after.depths,
        map_after.extent,
        1e-4,
        1.5,
    );
    println!(
        "shadow map known-answer after replacement: {compared_after} compared, \
         {skipped_after} skipped, {mismatches_after} mismatches, \
         max |Δdepth| {max_error_after:.2e}"
    );
    assert_eq!(mismatches_after, 0, "the replacement's map matches too");
    let after = renderer.allocations();
    for _ in 0..2 {
        let image = capture_shadowed(&mut renderer, &matrix, eye, light, size);
        assert_eq!(renderer.allocations(), after);
        assert_frames_equal_bytes(&shadowed_frame, &image);
    }
    // The unshadowed path is STILL pixel-identical after everything
    // (the regression pin: shadows never touched it).
    let plain_after = capture_corner(&mut renderer, &matrix, eye, light, size);
    assert_frames_equal_bytes(&plain, &plain_after);

    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "{:?}",
        renderer.stats().recent_validation
    );
    println!(
        "6-F gates: map known-answer exact on {compared} texels, both pixel oracles within \
         2/255, {differ} frame pixels changed by the shadow, zero per-frame allocation"
    );
}

fn assert_frames_equal_bytes(expected: &CapturedFrame, actual: &CapturedFrame) {
    assert_eq!(expected.format, actual.format);
    assert_eq!(expected.bytes, actual.bytes, "byte-identical captures");
}
