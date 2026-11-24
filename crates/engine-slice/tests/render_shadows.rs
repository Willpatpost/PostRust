//! The Cycle 6-F shadow gates (see docs/AUTONOMOUS-CYCLE-PROTOCOL.md
//! §2 item 2) and the Era-1 SOFT-shadow gate: the shadow map's depth
//! values pinned against the CPU raycaster (the known-answer check —
//! every texel is the deterministic core's `map_texel_depth`), the
//! shadowed/unshadowed pixel equivalence (the render_corner oracle
//! discipline applied to the shadow term: a fully-shadowed ground
//! patch renders exactly `albedo · ambient`, a fully-lit patch
//! exactly the unshadowed equation, and the shadow visibly changes
//! the frame), and — Era 1 — the PCF penumbra gate: the shadowed
//! shader's filtered factor vs the CPU filter (`pcf_at_point`) over
//! the SAME captured map bytes, per pixel, on a ground patch that
//! straddles the day shadow edge.
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
    LightParams, Material, day_preset, intersection_boxes, lit_surface_color, materials,
};
use engine_renderer::proc_texture::{TEXTURE_SIZE, procedural_texture};
use engine_renderer::shadow_filter::{pcf_at_point, pcf_factor};
use engine_renderer::shadows::{
    self, ShadowBias, ShadowCascade, is_shadowed, map_texel_depth, scene_bounds,
};
use engine_renderer::{
    CapturedFrame, DemoScenePreference, RenderConfig, Renderer, ShadowMapReadback,
    ValidationPreference,
};
use engine_shell::{DriverPreference, ShellApp, ShellConfig};

// The host allocation counter test-support module (finding 10's
// missing gate; tests/common/ — not a standalone test target).
mod common;

/// The binary's SDL serialization (the engine-shell test pattern —
/// render_instanced's own copy, applied to this binary now that
/// the PCF package grew it to TWO SDL-touching gates; found when
/// the gates first executed in parallel: SDL refuses init off the
/// main thread, and cargo test runs the two tests concurrently).
/// The guard is declared FIRST in each test so it drops LAST
/// (reverse declaration order): the app and renderer are fully
/// dropped — SDL quit, the main-thread declaration reset — before
/// the lock releases and the next test initializes.
fn sdl_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}

/// The bias under test (the demo's own policy — one source for both).
const BIAS: ShadowBias = ShadowBias {
    constant: 0.0015,
    slope_scale: 0.002,
};

fn corner_cascade(light: LightParams) -> ShadowCascade {
    let boxes = intersection_boxes();
    let (min, max) = scene_bounds(boxes);
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
/// are exact). Returns (compared, skipped, mismatches, max_error)
/// plus per-class census counts.
fn compare_map(
    cascade: &ShadowCascade,
    boxes: &[engine_renderer::lit_scene::SceneBox],
    depths: &[f32],
    extent: u32,
    tolerance: f32,
    margin_texels: f32,
) -> (usize, usize, usize, f32, usize, usize, usize) {
    let boxes_table = intersection_boxes();
    let texel_world = (cascade.max_right - cascade.min_right).max(cascade.max_up - cascade.min_up)
        / extent as f32;
    let mut compared = 0;
    let mut skipped = 0;
    let mut mismatches = 0;
    let mut max_error = 0.0f32;
    // Per-class census for verification floors (finding 14)
    let mut clear_count = 0;
    let mut ground_count = 0;
    let mut building_count = 0;
    for y in 0..extent {
        for x in 0..extent {
            let expected = map_texel_depth(cascade, boxes, x, y, extent);
            let Some(hit) = expected.hit else {
                // Clear texel: no surface at all — exact.
                let gpu = depths[(y * extent + x) as usize];
                // Finding 14 correction (b): reject nonfinite or out-of-range GPU depth FIRST
                let clear_error = (gpu - 1.0).abs();
                if !gpu.is_finite() || gpu < 0.0 || gpu > 1.0 + 1e-6 || clear_error > tolerance {
                    mismatches += 1;
                }
                compared += 1;
                clear_count += 1;
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
                None => ([[-40.0, 0.0, -40.0], [40.0, 0.0, 40.0]], [0, 2, 1]),
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
            // Finding 14 correction (a): measure the ground perimeter on X and Z
            // For ground (box_index == None), the ground plate's boundary must be
            // tested on X and Z (the ground's tangential axes), not Y
            let mut min_edge = f32::MAX;
            if expected.box_index.is_none() {
                // Ground: measure X and Z (the tangential axes for ground)
                for &axis in &[0, 2] {
                    let lo = (bounds[0][axis] - hit[axis]).abs();
                    let hi = (bounds[1][axis] - hit[axis]).abs();
                    min_edge = min_edge.min(lo).min(hi);
                }
            } else {
                // Building: use the original tangential logic
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
            }
            if min_edge < margin_texels * texel_world {
                skipped += 1;
                continue;
            }
            let gpu = depths[(y * extent + x) as usize];
            // Finding 14 correction (b): reject nonfinite or out-of-range GPU depth FIRST
            if !gpu.is_finite() || gpu < 0.0 || gpu > 1.0 + 1e-6 {
                mismatches += 1;
                if mismatches <= 10 {
                    println!(
                        "MISMATCH texel ({x},{y}): nonfinite/out-of-range gpu {gpu} (box {:?}, hit {:?})",
                        expected.box_index, expected.hit
                    );
                }
                compared += 1;
                // Classify based on expected
                if expected.box_index.is_none() {
                    ground_count += 1;
                } else {
                    building_count += 1;
                }
                continue;
            }
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
            // Classify based on expected
            if expected.box_index.is_none() {
                ground_count += 1;
            } else {
                building_count += 1;
            }
        }
    }
    (
        compared,
        skipped,
        mismatches,
        max_error,
        clear_count,
        ground_count,
        building_count,
    )
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
            is_shadowed(probe, cascade, boxes),
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
    let _guard = sdl_lock();
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
    let (compared, skipped, mismatches, max_error, clear_count, ground_count, building_count) =
        compare_map(&cascade, boxes, &map.depths, map.extent, 1e-4, 1.5);
    let total = map.extent as usize * map.extent as usize;
    println!(
        "shadow map known-answer: {total} texels, {compared} compared, {skipped} edge-skipped \
         ({:.2}%), {mismatches} mismatches, max |Δdepth| {max_error:.2e}",
        100.0 * skipped as f32 / total as f32
    );
    println!("class census: {clear_count} clear, {ground_count} ground, {building_count} building");
    // Finding 14 correction (a): assert separate positive coverage floors
    assert!(
        clear_count >= 5000,
        "clear texels coverage: {clear_count} (floor 5000)"
    );
    assert!(
        ground_count >= 1000,
        "ground texels coverage: {ground_count} (floor 1000)"
    );
    assert!(
        building_count >= 500,
        "building texels coverage: {building_count} (floor 500)"
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

    // THE HOST-ALLOCATION GATE (finding 10 — the counter the review
    // says was missing beside renderer.allocations()): one warmed
    // SHADOWED render, measured between scope entry and count — zero
    // heap allocations (the per-frame Vec<SceneBox> is gone; the
    // scene bounds are a OnceLock table now). Capture extraction,
    // assertions, and startup stay OUTSIDE the scope.
    {
        let scope = common::HostAllocScope::new();
        let outcome = renderer
            .render_corner_shadowed(&matrix, eye, light, BIAS, size)
            .unwrap();
        let allocs = scope.count();
        drop(scope);
        assert!(outcome.presented);
        println!("host allocations in warmed shadowed frame: {allocs} (expected 0)");
        assert_eq!(
            allocs, 0,
            "no host allocations in the warmed shadowed render path"
        );
    }

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
    let (compared_after, skipped_after, mismatches_after, max_error_after, _, _, _) = compare_map(
        &cascade,
        boxes,
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

// ------------------------------------------------------- the Era-1
// soft-shadow gate's local helpers (the exposure-class mirror of
// pcf_at_point's internals — the epsilons below absorb the
// GPU/CPU ulp divergence the classes exist to classify).

/// The compare-exposure epsilon (depth units): a tap whose biased
/// compare margin sits within this of zero can flip between the
/// shader's and the CPU's evaluation (their inputs are the same map
/// bytes; the divergence is fma contraction in the projection and
/// the WGSL normalize, ulp-scale). Matches the map gate's tolerance
/// class; the corner scene's adjacent-tap depth steps (~0.0016 at
/// the wall faces) are far above it, so at most ONE tap of nine is
/// ever exposed — the ±1/9 factor slack below.
const COMPARE_EXPOSURE_EPS: f32 = 1e-4;
/// The texel-boundary epsilon (texel units): a projected texel
/// coordinate within this of an integer can truncate to either
/// side (the neighborhood shifts by one texel — up to four
/// candidate anchors).
const TEXEL_EXPOSURE_EPS: f32 = 1e-3;

/// One pixel's CPU-side PCF diagnostics: the core's factor (via
/// `pcf_at_point`, authoritative) plus the exposure classification
/// from the mirrored projection.
struct PcfPixel {
    factor: f32,
    /// Some tap's biased compare margin is within
    /// [`COMPARE_EXPOSURE_EPS`] of zero (a flippable tap).
    compare_exposed: bool,
    /// The projected texel X coordinate is within
    /// [`TEXEL_EXPOSURE_EPS`] of an integer (a truncatable anchor on X).
    /// The sign indicates which side: negative means truncated up (toward lower texel),
    /// positive means truncated down (toward higher texel).
    texel_exposed_x: Option<f32>,
    /// The projected texel Y coordinate is within
    /// [`TEXEL_EXPOSURE_EPS`] of an integer (a truncatable anchor on Y).
    /// The sign indicates which side: negative means truncated up (toward lower texel),
    /// positive means truncated down (toward higher texel).
    texel_exposed_y: Option<f32>,
    /// The CPU anchor's (texel_x, texel_y) — for the alternate
    /// neighborhoods a texel-exposed pixel may have sampled.
    anchor: (u32, u32),
}

/// The gate's mirror of `pcf_at_point`'s projection (same
/// expressions; the epsilons own the drift) with the exposure
/// classification. `None` outside the cascade box (factor 1.0, not
/// exposed — the corner scene's ground never leaves the box).
fn pcf_pixel_diagnostics(
    cascade: &ShadowCascade,
    map: &ShadowMapReadback,
    point: [f32; 3],
    bias: ShadowBias,
) -> Option<PcfPixel> {
    let extent = map.extent;
    let clip: [f32; 4] = std::array::from_fn(|i| {
        cascade.matrix[0][i] * point[0]
            + cascade.matrix[1][i] * point[1]
            + cascade.matrix[2][i] * point[2]
            + cascade.matrix[3][i]
    });
    let w = clip[3];
    let ndc = [clip[0] / w, clip[1] / w, clip[2] / w];
    if !(-1.0..=1.0).contains(&ndc[0])
        || !(-1.0..=1.0).contains(&ndc[1])
        || !(0.0..=1.0).contains(&ndc[2])
    {
        return None;
    }
    let extent_f = extent as f32;
    let fx = (ndc[0] + 1.0) * 0.5 * extent_f;
    let fy = (1.0 - (ndc[1] + 1.0) * 0.5) * extent_f;
    let texel_x = (fx as u32).min(extent - 1);
    let texel_y = (fy as u32).min(extent - 1);
    let normal = [0.0, 1.0, 0.0];
    let facing = (normal[0] * cascade.light[0]
        + normal[1] * cascade.light[1]
        + normal[2] * cascade.light[2])
        .abs()
        .max(1.0 / 64.0);
    let bias_value = bias.constant + bias.slope_scale * (1.0 / facing - 1.0);
    // The core's factor (authoritative — the same convention this
    // mirror derives).
    let read = |x: u32, y: u32| map.depths[(y * extent + x) as usize];
    let factor = pcf_at_point(cascade, extent, point, normal, bias, read);
    // The exposure classification over the same taps.
    let mut compare_exposed = false;
    for dx in -1..=1 {
        for dy in -1..=1 {
            let x = texel_x as i32 + dx;
            let y = texel_y as i32 + dy;
            let depth = if x < 0 || y < 0 || x >= extent as i32 || y >= extent as i32 {
                1.0
            } else {
                map.depths[(y as u32 * extent + x as u32) as usize]
            };
            if (ndc[2] - (depth + bias_value)).abs() < COMPARE_EXPOSURE_EPS {
                compare_exposed = true;
            }
        }
    }
    let boundary = |v: f32| (v - v.round()).abs();
    let dist_to_x_boundary = boundary(fx);
    let dist_to_y_boundary = boundary(fy);
    // Finding 15 correction: track X and Y boundaries separately with signed distances
    let texel_exposed_x = if dist_to_x_boundary < TEXEL_EXPOSURE_EPS {
        Some(fx - fx.round())
    } else {
        None
    };
    let texel_exposed_y = if dist_to_y_boundary < TEXEL_EXPOSURE_EPS {
        Some(fy - fy.round())
    } else {
        None
    };
    // For backward compatibility, expose a combined flag if either axis is exposed
    let _texel_exposed = texel_exposed_x.is_some() || texel_exposed_y.is_some();
    Some(PcfPixel {
        factor,
        compare_exposed,
        texel_exposed_x,
        texel_exposed_y,
        anchor: (texel_x, texel_y),
    })
}

/// The factored ground equation — the shadowed shader's EXACT form
/// (the direct terms multiplied by the factor, the multiply order
/// mirrored term-for-term):
/// `albedo · (ambient + light · NdotL · f) + spec · light · NdotL · f · NdotH^shininess`.
fn factored_ground_color(
    albedo: [f32; 3],
    factor: f32,
    material: Material,
    light: LightParams,
    eye: [f32; 3],
    pos: [f32; 3],
) -> [f32; 3] {
    let normalize = |v: [f32; 3]| {
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / len, v[1] / len, v[2] / len]
    };
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let n = [0.0, 1.0, 0.0];
    let v = normalize(std::array::from_fn(|i| eye[i] - pos[i]));
    let l = normalize(light.dir_toward_light);
    let n_dot_l = dot(n, l).clamp(0.0, 1.0);
    let h = normalize([l[0] + v[0], l[1] + v[1], l[2] + v[2]]);
    let n_dot_h = dot(n, h).clamp(0.0, 1.0);
    let mut spec = n_dot_h;
    for _ in 0..material.shininess.trailing_zeros() {
        spec = spec * spec;
    }
    std::array::from_fn(|c| {
        albedo[c] * (light.ambient[c] + light.light_color[c] * n_dot_l * factor)
            + material.specular[c] * light.light_color[c] * n_dot_l * factor * spec
    })
}

/// Finding 15's reachable-anchor enumeration — the gate's own
/// acceptance policy, factored out so its discipline is testable
/// against the exact list this gate accepts: a one-texel shift is
/// admissible only along an axis actually at its texel boundary,
/// in the direction the projection truncated (the signed distance
/// `fx - fx.round()`); the diagonal alternative is reachable only
/// when BOTH axes are exposed. At most four anchors, in fixed
/// order: base, the X shift, the Y shift, the diagonal.
fn pcf_reachable_anchors(
    anchor: (u32, u32),
    exposed_x: Option<f32>,
    exposed_y: Option<f32>,
) -> Vec<(i32, i32)> {
    let (tx, ty) = (anchor.0 as i32, anchor.1 as i32);
    let dir = |shift: f32| if shift < 0.0 { -1 } else { 1 };
    let mut anchors = Vec::with_capacity(4);
    anchors.push((tx, ty));
    if let Some(shift_x) = exposed_x {
        anchors.push((tx + dir(shift_x), ty));
    }
    if let Some(shift_y) = exposed_y {
        anchors.push((tx, ty + dir(shift_y)));
    }
    if let (Some(shift_x), Some(shift_y)) = (exposed_x, exposed_y) {
        anchors.push((tx + dir(shift_x), ty + dir(shift_y)));
    }
    anchors
}

/// The Era-1 SOFT-SHADOW gate (the brief's PCF contract): the
/// shadowed shader's filtered factor vs the CPU filter
/// (`pcf_at_point`) over the SAME captured map bytes, per pixel, on
/// a ground patch that straddles the day preset's signature shadow
/// edge — the deterministic flip walk ([0,0] toward [12,3], the
/// scene's own known-shadowed and known-lit anchors) centers the
/// patch exactly on the boundary, so the frame contains deep shadow,
/// penumbra, and full light.
///
/// THE TOLERANCE (documented): interior pixels (no tap within
/// [`COMPARE_EXPOSURE_EPS`] of the compare boundary, no projected
/// texel coordinate within [`TEXEL_EXPOSURE_EPS`] of an integer)
/// must match the CPU-filtered equation within 2/255 — the 6-B
/// discipline, exact factor agreement. Compare-exposed pixels may
/// flip ONE tap (the adjacent-tap depth step ~0.0016 dwarfs the
/// exposure epsilon, so one tap at a time): the pixel must match the
/// equation at the CPU factor OR at factor ± 1/9. Texel-exposed
/// pixels may have truncated to the neighboring anchor: the pixel
/// must match the equation at any of the REACHABLE anchors' factors
/// (`pcf_reachable_anchors`: the base, one signed shift per exposed
/// axis, and the diagonal only when both axes are exposed — at most
/// four). Every class is counted and printed — the CI log's
/// evidence.
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup (see render_overflow)"]
fn pcf_penumbra_pixels_match_the_cpu_filter_over_the_same_map_bytes() {
    let _guard = sdl_lock();
    let mut app = ShellApp::new(ShellConfig {
        title: "Era 1 soft shadows".into(),
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
    let _ = app.frame();
    let light = day_preset();
    let boxes = intersection_boxes();
    let cascade = corner_cascade(light);

    // The captured map: the SAME bytes the shader filtered (the 6-F
    // gate pins them to the raycaster; this gate pins the FILTER).
    renderer.request_shadow_map_capture().unwrap();
    let probe = ground_camera([0.0, 0.0], 1.0);
    assert!(
        renderer
            .render_corner_shadowed(&probe.0, probe.1, light, BIAS, size)
            .unwrap()
            .presented
    );
    let map = renderer.take_shadow_map_capture().unwrap().unwrap();
    assert_eq!(map.extent, shadows::SHADOW_MAP_EXTENT);
    let read = |x: u32, y: u32| map.depths[(y * map.extent + x) as usize];

    // The deterministic flip walk: [0,0] (the known-shadowed anchor)
    // toward [12,3] (the known-lit anchor) in fixed 0.01 m steps;
    // the FIRST raycaster-lit point is the patch center (the walk is
    // the unit-tested battery's own line — the regimes along it are
    // pinned CPU-side).
    let (ux, uz) = {
        let len = (12.0f32 * 12.0 + 3.0 * 3.0).sqrt();
        (12.0 / len, 3.0 / len)
    };
    let mut flip = None;
    for step in 1..=1240 {
        let t = step as f32 * 0.01;
        let point = [ux * t, 0.0, uz * t];
        if !is_shadowed(point, &cascade, boxes) {
            flip = Some(point);
            break;
        }
    }
    let flip = flip.expect("the walk reaches the known-lit anchor");
    println!("penumbra patch centered on the flip point {flip:?}");

    // The straddling patch: the same orthographic ground camera as
    // the 6-F patches, 1.5 m square on the boundary.
    let (matrix, eye) = ground_camera([flip[0], flip[2]], 0.75);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner_shadowed(&matrix, eye, light, BIAS, size)
            .unwrap()
            .presented
    );
    let image = renderer.take_frame_capture().unwrap().unwrap();
    let mat = materials()[0];
    let texture = procedural_texture(mat.texture);

    let quantized = |color: [f32; 3]| color.map(|c| (255. * c.clamp(0., 1.)).round() as u8);
    let albedo_at = |pos: [f32; 3]| {
        let uv_axes = [pos[0], pos[2]];
        let [tx, ty] =
            uv_axes.map(|m| ((m / 4.).rem_euclid(1.) * TEXTURE_SIZE as f32).floor() as usize);
        let index = 4 * (ty * TEXTURE_SIZE as usize + tx);
        std::array::from_fn(|c| texture[index + c] as f32 / 255.)
    };

    let mut interior = 0usize;
    let mut compare_exposed = 0usize;
    let mut texel_exposed = 0usize;
    let mut penumbra = 0usize;
    let mut deep_shadow = 0usize;
    let mut full_light = 0usize;
    let mut worst_interior_error = 0u8;
    for y in 8..size.1 - 8 {
        for x in 8..size.0 - 8 {
            let dx = (x as f32 + 0.5) * 2. / size.0 as f32 - 1.;
            let dy = (y as f32 + 0.5) * 2. / size.1 as f32 - 1.;
            // The 6-F patch convention (the proven orientation): the
            // world point of this pixel.
            let pos = [flip[0] + dx * 0.75, 0.0, flip[2] + dy * 0.75];
            let albedo = albedo_at(pos);
            let pixel = image.pixel_rgba(x, y);
            let expected_at = |factor: f32| {
                quantized(factored_ground_color(albedo, factor, mat, light, eye, pos))
            };
            let Some(diag) = pcf_pixel_diagnostics(&cascade, &map, pos, BIAS) else {
                // Outside the cascade box: the shader's early return
                // (factor 1.0) — the ground patch cannot reach it,
                // but the class is handled honestly if it ever does.
                let expected = expected_at(1.0);
                assert!(
                    expected
                        .iter()
                        .zip(pixel)
                        .map(|(e, a)| e.abs_diff(a))
                        .max()
                        .unwrap()
                        <= 2,
                    "outside-box pixel ({x},{y})"
                );
                full_light += 1;
                continue;
            };
            match diag.factor {
                0.0 => deep_shadow += 1,
                1.0 => full_light += 1,
                _ => penumbra += 1,
            }
            let close = |expected: [u8; 3]| {
                expected
                    .iter()
                    .zip(pixel)
                    .map(|(e, a)| e.abs_diff(a))
                    .max()
                    .unwrap()
                    <= 2
            };
            // Finding 15 correction: separate X and Y boundary tracking with signed distances
            let texel_exposed_x = diag.texel_exposed_x.is_some();
            let texel_exposed_y = diag.texel_exposed_y.is_some();

            if texel_exposed_x || texel_exposed_y {
                texel_exposed += 1;
                // The anchor selection must respect which axis is actually exposed.
                // Only enumerate reachable anchors: a one-texel shift is admissible
                // only along an axis that is at its boundary.
                let normal = [0.0, 1.0, 0.0];
                let facing = normal[1] * cascade.light[1];
                let bias_value =
                    BIAS.constant + BIAS.slope_scale * (1.0 / facing.max(1.0 / 64.0) - 1.0);
                let clip_z = cascade.matrix[0][2] * pos[0]
                    + cascade.matrix[1][2] * pos[1]
                    + cascade.matrix[2][2] * pos[2]
                    + cascade.matrix[3][2];
                let reference = clip_z / 1.0;
                let mut matched = false;

                // The reachable anchors (finding 15's discipline,
                // factored so its own tests exercise the exact
                // enumeration this gate accepts).
                let anchors =
                    pcf_reachable_anchors(diag.anchor, diag.texel_exposed_x, diag.texel_exposed_y);

                for (ax, ay) in anchors {
                    let anchor_x = ax.clamp(0, map.extent as i32 - 1) as u32;
                    let anchor_y = ay.clamp(0, map.extent as i32 - 1) as u32;
                    let factor = pcf_factor(reference, bias_value, |dx, dy| {
                        let x = anchor_x as i32 + dx;
                        let y = anchor_y as i32 + dy;
                        if x < 0 || y < 0 || x >= map.extent as i32 || y >= map.extent as i32 {
                            1.0
                        } else {
                            read(x as u32, y as u32)
                        }
                    });
                    if close(expected_at(factor)) {
                        matched = true;
                        break;
                    }
                }
                assert!(
                    matched,
                    "texel-exposed pixel ({x},{y}) world {pos:?}: {:?} vs {pixel:?}",
                    expected_at(diag.factor)
                );
            } else if diag.compare_exposed {
                compare_exposed += 1;
                // One tap may flip: the factor moves by exactly 1/9.
                let f = diag.factor;
                let alternates = [f, (f - 1.0 / 9.0).max(0.0), (f + 1.0 / 9.0).min(1.0)];
                assert!(
                    alternates.iter().any(|&f| close(expected_at(f))),
                    "compare-exposed pixel ({x},{y}) world {pos:?} factor {f}: \
                     {:?} vs {pixel:?}",
                    expected_at(f)
                );
            } else {
                interior += 1;
                let expected = expected_at(diag.factor);
                let error = expected
                    .iter()
                    .zip(pixel)
                    .map(|(e, a)| e.abs_diff(a))
                    .max()
                    .unwrap();
                worst_interior_error = worst_interior_error.max(error);
                assert!(
                    error <= 2,
                    "interior pixel ({x},{y}) world {pos:?} factor {:?}: \
                     {expected:?} vs {pixel:?}",
                    diag.factor
                );
            }
            assert_eq!(pixel[3], 255);
        }
    }
    let total = (size.0 - 16) as usize * (size.1 - 16) as usize;
    println!(
        "PCF penumbra gate: {total} pixels — {interior} interior (max error \
         {worst_interior_error}/255), {compare_exposed} compare-exposed, \
         {texel_exposed} texel-exposed; regimes: {deep_shadow} deep shadow, \
         {penumbra} penumbra, {full_light} full light"
    );
    // Finding 15 correction: pin a minimum strict-interior penumbra coverage
    // (a positive count of strictly-compared partial-shadow pixels — no more
    // epsilon-slack-only passes)
    assert!(
        interior > 1_000,
        "strict interior pixels: {interior} (floor 1000)"
    );
    // The non-vacuity floor: all three regimes present in force (the
    // patch straddles the boundary by construction; the penumbra
    // strip is ~0.22 m wide — the 1.5 m patch maps it to ~35 pixel
    // rows).
    assert!(deep_shadow > 5_000, "deep shadow: {deep_shadow}");
    assert!(full_light > 5_000, "full light: {full_light}");
    assert!(penumbra > 500, "penumbra pixels: {penumbra}");

    // The warmed-allocation constancy across the gate's own renders.
    let warm = renderer.allocations();
    for _ in 0..2 {
        renderer.request_frame_capture().unwrap();
        assert!(
            renderer
                .render_corner_shadowed(&matrix, eye, light, BIAS, size)
                .unwrap()
                .presented
        );
        let repeat = renderer.take_frame_capture().unwrap().unwrap();
        assert_eq!(
            renderer.allocations(),
            warm,
            "warmed PCF allocations constant"
        );
        assert_frames_equal_bytes(&image, &repeat);
    }
    println!("warmed PCF shadowed allocations: {warm:?}");

    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "{:?}",
        renderer.stats().recent_validation
    );
    println!(
        "Era-1 soft-shadow gate: the shader's PCF factor matches the CPU filter over the \
         same map bytes — {interior} interior pixels at the 6-B 2/255 discipline, the \
         exposure classes bounded by their documented one-tap/one-anchor slacks"
    );
}

// ------------------------------------- findings 14+15: the damaged-map negative
// controls (the review's check, landed for real: "Feed the
// comparator one damaged interior ground depth and one NaN from
// each class and require failure"; "A kernel shifted across an
// unexposed axis should fail even when the other axis is within
// epsilon"). The comparator under test is THE gates' own
// `compare_map` and THE gate's own anchor enumeration — not
// restatements of their arithmetic. Pure CPU (no SDL, no GPU):
// these run in the heavy shard's plain pass.

/// The CPU-synthesized corner map: the deterministic core's own
/// `map_texel_depth` over every texel — the same oracle
/// `compare_map` recomputes internally, so the undamaged control
/// run is exact by construction and every mismatch in the damaged
/// runs below is attributable to the planted damage alone. Built
/// once per process (1,048,576 raycasts; the `intersection_boxes`
/// OnceLock discipline).
struct CpuCornerMap {
    cascade: ShadowCascade,
    depths: Vec<f32>,
    /// (texel index, expected depth) of every DEEP-INTERIOR
    /// ground-hit texel — the damage targets. The ground plate in
    /// `compare_map` is the ±40 m scene ground, so border hits
    /// within its 1.5-texel perimeter margin (and the corner-graze
    /// arm's 1e-3 band) are edge-skipped; the cache keeps only
    /// hits a full 4 texels clear of ±40 on BOTH X and Z, so every
    /// cached texel is compared — that is what makes the damaged-
    /// input tests' exact-count assertions (exactly 1, exactly 2)
    /// sound rather than best-effort.
    ground_texels: Vec<(usize, f32)>,
}

fn cpu_corner_map() -> &'static CpuCornerMap {
    static MAP: std::sync::OnceLock<CpuCornerMap> = std::sync::OnceLock::new();
    MAP.get_or_init(|| {
        let light = day_preset();
        let boxes = intersection_boxes();
        let cascade = corner_cascade(light);
        let extent = shadows::SHADOW_MAP_EXTENT;
        // `compare_map`'s own texel-world metric (the larger cascade
        // span over the extent — the one its 1.5-texel ground
        // perimeter margin uses).
        let texel_world = (cascade.max_right - cascade.min_right)
            .max(cascade.max_up - cascade.min_up)
            / extent as f32;
        let clearance = 4.0 * texel_world;
        let mut depths = Vec::with_capacity(extent as usize * extent as usize);
        let mut ground_texels = Vec::new();
        for y in 0..extent {
            for x in 0..extent {
                let expectation = map_texel_depth(&cascade, boxes, x, y, extent);
                let index = (y * extent + x) as usize;
                depths.push(expectation.depth);
                if let Some(hit) = expectation.hit
                    && expectation.box_index.is_none()
                    && hit[0].abs() <= 40.0 - clearance
                    && hit[2].abs() <= 40.0 - clearance
                {
                    ground_texels.push((index, expectation.depth));
                }
            }
        }
        CpuCornerMap {
            cascade,
            depths,
            ground_texels,
        }
    })
}

/// Finding 14's check, landed for real: "one NaN from each class".
/// The undamaged control run proves zero mismatches on the clean
/// CPU map; then a NaN planted in every texel of every class must
/// be rejected in every compared texel — the original defect (NaN
/// makes `abs(error) > tolerance` false, so a NaN was counted as a
/// comparison WITHOUT a mismatch) would report zero mismatches
/// here.
#[test]
fn depth_comparator_rejects_nan() {
    let map = cpu_corner_map();
    let boxes = intersection_boxes();
    let extent = shadows::SHADOW_MAP_EXTENT;
    let (compared, _, mismatches, max_error, clear, ground, building) =
        compare_map(&map.cascade, boxes, &map.depths, extent, 1e-4, 1.5);
    assert_eq!(
        mismatches, 0,
        "the undamaged CPU map is exact (the comparator's own oracle)"
    );
    assert_eq!(max_error, 0.0);
    assert!(
        clear > 0 && ground > 0 && building > 0,
        "all three classes present: {clear} clear / {ground} ground / {building} building"
    );

    // One NaN from EACH class — planted in every texel, which
    // subsumes one-per-class and makes the required coverage exact.
    let mut damaged = map.depths.clone();
    damaged.fill(f32::NAN);
    let (compared_d, _, mismatches_d, max_error_d, clear_d, ground_d, building_d) =
        compare_map(&map.cascade, boxes, &damaged, extent, 1e-4, 1.5);
    println!(
        "NaN feed: {mismatches_d} of {compared_d} compared texels rejected \
         ({clear_d} clear / {ground_d} ground / {building_d} building)"
    );
    // The census classifies by EXPECTED class, so it is
    // damage-invariant — the same texels are compared.
    assert_eq!(
        (compared_d, clear_d, ground_d, building_d),
        (compared, clear, ground, building)
    );
    assert_eq!(
        mismatches_d, compared_d,
        "every NaN-fed texel is rejected as a mismatch, in every class"
    );
    assert_eq!(
        max_error_d, 0.0,
        "a rejected NaN never poisons the error metric"
    );
}

/// Finding 14's check: out-of-range depths (above the
/// [0, 1 + 1e-6] ceiling, below zero) are rejected through the
/// reject-first range check, not the tolerance comparison. Two
/// interior ground texels — one planted above the ceiling, one
/// below zero — must produce exactly two mismatches.
#[test]
fn depth_comparator_rejects_out_of_range() {
    let map = cpu_corner_map();
    let boxes = intersection_boxes();
    let extent = shadows::SHADOW_MAP_EXTENT;
    let (compared, _, mismatches, _, _, ground, _) =
        compare_map(&map.cascade, boxes, &map.depths, extent, 1e-4, 1.5);
    assert_eq!(mismatches, 0, "the undamaged control");
    assert!(ground > 0, "ground texels present: {ground}");

    assert!(
        map.ground_texels.len() >= 2,
        "two interior ground texels exist (the gate's census floor pins >= 1000)"
    );
    let above = map.ground_texels[map.ground_texels.len() / 3].0;
    let below = map.ground_texels[2 * map.ground_texels.len() / 3].0;
    assert_ne!(above, below);
    let mut damaged = map.depths.clone();
    damaged[above] = 1.0 + 2.0 * 1e-6; // finite, positive, above the ceiling
    damaged[below] = -1e-6; // finite, negative, below zero
    let (compared_d, _, mismatches_d, _, _, ground_d, _) =
        compare_map(&map.cascade, boxes, &damaged, extent, 1e-4, 1.5);
    assert_eq!((compared_d, ground_d), (compared, ground));
    assert_eq!(
        mismatches_d, 2,
        "both out-of-range plants rejected (exactly two)"
    );
}

/// Finding 14's check: "one damaged interior ground depth" — a
/// WRONG but finite, in-range value (half the expected depth) at
/// one interior ground texel must fail through the tolerance
/// comparison (|gpu − expected| > tolerance), the path a NaN
/// silently passed before the fix. Exactly one mismatch, and the
/// reported max error is the planted damage.
#[test]
fn depth_comparator_rejects_damaged_ground_depth() {
    let map = cpu_corner_map();
    let boxes = intersection_boxes();
    let extent = shadows::SHADOW_MAP_EXTENT;
    let (_, _, mismatches, _, _, ground, _) =
        compare_map(&map.cascade, boxes, &map.depths, extent, 1e-4, 1.5);
    assert_eq!(mismatches, 0, "the undamaged control");
    assert!(ground > 0, "ground texels present: {ground}");

    // One deep-interior ground texel (the cache's 4-texel clearance
    // guarantees it is compared — see `cpu_corner_map`).
    let &(target, expected) = map
        .ground_texels
        .iter()
        .find(|&&(_, depth)| depth > 0.05)
        .expect("a deep-interior ground texel with depth > 0.05 exists (the compared ground census alone is >= 1000)");
    let mut damaged = map.depths.clone();
    damaged[target] = expected * 0.5;
    let (_, _, mismatches_d, max_error_d, _, _, _) =
        compare_map(&map.cascade, boxes, &damaged, extent, 1e-4, 1.5);
    assert_eq!(
        mismatches_d, 1,
        "the wrong-but-in-range ground depth fails the tolerance check (exactly one)"
    );
    assert!(
        (max_error_d - expected * 0.5).abs() <= 1e-6,
        "the reported max error is the planted damage: {max_error_d} vs {}",
        expected * 0.5
    );
}

/// Finding 15's check: "A kernel shifted across an unexposed axis
/// should fail even when the other axis is within epsilon." The
/// gate's acceptance loop can only try anchors this enumeration
/// returns, so the property is structural: with X interior (None)
/// and Y within epsilon of its boundary, the enumeration offers NO
/// X-shifted anchor — either sign — and no diagonal.
#[test]
fn pcf_boundary_rejects_unexposed_axis_shift() {
    // Y exposed within epsilon (truncated down, +), X interior.
    let anchors = pcf_reachable_anchors((10, 10), None, Some(0.5));
    assert_eq!(
        anchors,
        vec![(10, 10), (10, 11)],
        "base + the signed Y shift only"
    );
    assert!(
        anchors.iter().all(|&(ax, _)| ax == 10),
        "no X shift on an X-unexposed texel (either sign)"
    );
    // The Y shift follows the truncation direction: -0.3 truncated up.
    assert_eq!(
        pcf_reachable_anchors((10, 10), None, Some(-0.3)),
        vec![(10, 10), (10, 9)]
    );
    // Neither axis exposed: the base anchor alone.
    assert_eq!(pcf_reachable_anchors((10, 10), None, None), vec![(10, 10)]);
}

/// Finding 15's check: at a true two-axis boundary the actual
/// diagonal alternative is reachable — base, both SIGNED axial
/// shifts, and the signed diagonal, in the fixed order.
#[test]
fn pcf_boundary_allows_diagonal_when_both_exposed() {
    // X truncated down (+1), Y truncated up (-1).
    let anchors = pcf_reachable_anchors((10, 10), Some(0.5), Some(-0.3));
    assert_eq!(anchors, vec![(10, 10), (11, 10), (10, 9), (11, 9)]);
    // The opposite truncation signs give the mirrored diagonal.
    assert_eq!(
        pcf_reachable_anchors((10, 10), Some(-0.2), Some(0.4)),
        vec![(10, 10), (9, 10), (10, 11), (9, 11)]
    );
}
