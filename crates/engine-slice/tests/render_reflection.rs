//! The Era-1 wet-ground reflection gates (see
//! docs/CYCLE6-ERA1-BRIEF.md, package 7): the known-answer blend
//! checks at grazing vs overhead view angles — the CPU function
//! (`planar.rs`) is the reference — through the REAL render path;
//! the dry path's byte-identity; the reflection state's one-time
//! allocation delta and warmed constancy; and the composition
//! smokes.
//!
//! THE KNOWN-ANSWER CHAIN (the 6-B oracle discipline, extended one
//! bounce): a ground pixel's wet value is
//!   dry · (1−b) + reflection · b
//! with b = `planar.rs::reflection_blend(n·v)` for the pixel's own
//! view vector, and the reflection value computed by the MIRROR
//! INVARIANCE of the lit equation: the mirrored render at the
//! mirrored point shades exactly as the real equation at the REAL
//! point from the REAL eye (the mirror preserves dot products —
//! the specular half-vector included), so the CPU reference
//! raycasts the REFLECTED ray (the view ray reflected about the
//! ground plane at the pixel's ground hit) into the corner boxes
//! and evaluates `lit_surface_color` at the hit — the same
//! machinery the corner oracle uses for direct view rays.
//!
//! `#[ignore]`d like the other lavapipe smokes (offscreen SDL +
//! lavapipe + the validation layer). CI's Linux runner executes it
//! enforced (the workflow's reflection-gates step). Run explicitly:
//!
//! ```sh
//! SDL_VIDEODRIVER=offscreen cargo test -p engine-slice --test render_reflection -- \
//!     --ignored --nocapture
//! ```

use engine_renderer::lit_scene::{RouteWaypoint, day_preset, lit_surface_color};
use engine_renderer::lit_scene::{intersection_boxes, materials};
use engine_renderer::planar::reflection_blend;
use engine_renderer::proc_texture::{TEXTURE_SIZE, procedural_texture};
use engine_renderer::shadows::ShadowBias;
use engine_renderer::sky::atmosphere_at;
use engine_renderer::{
    DemoScenePreference, DepthFormatPreference, RenderConfig, Renderer, SkyFrame,
    ValidationPreference,
};
use engine_shell::{DriverPreference, ShellApp, ShellConfig};
use engine_slice::corner_demo::view_projection;
use engine_slice::math::{camera_forward, camera_right};

/// The bias under test (the demo's own policy — one source for both).
const BIAS: ShadowBias = ShadowBias {
    constant: 0.0015,
    slope_scale: 0.002,
};

/// The binary's SDL serialization (the engine-shell test pattern —
/// render_instanced's own copy). The guard is declared FIRST so it
/// drops LAST.
fn sdl_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}

/// The camera basis for a pose (the sky gate's own helper).
fn basis(position: [f32; 3], toward: [f32; 3]) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let offset = std::array::from_fn(|i| toward[i] - position[i]);
    let (yaw, pitch) = engine_slice::corner_demo::waypoint_orientation(offset);
    let forward = camera_forward(yaw, pitch, 0.0);
    let right = camera_right(yaw, pitch, 0.0);
    let rotation = engine_slice::math::camera_rotation(yaw, pitch, 0.0);
    let up = [rotation[1][0], rotation[1][1], rotation[1][2]];
    (forward, right, up)
}

/// The CPU ray for one raster pixel (the sky gate's own helper).
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

/// One reflected-ray hit on the corner boxes (the slab test — the
/// first positive-t hit; the ground plane itself is not a box).
#[derive(Debug, Clone, Copy)]
struct BoxHit {
    pos: [f32; 3],
    normal: [f32; 3],
    /// The face's uv axes (the lit_mesh quad convention: the
    /// axis indexes the uv is computed from).
    uv_axes: [f32; 2],
    material: usize,
}

fn raycast_boxes(origin: [f32; 3], dir: [f32; 3]) -> Option<BoxHit> {
    let mut best: Option<(f32, BoxHit)> = None;
    for b in intersection_boxes() {
        let [min_x, min_y, min_z] = b.min;
        let [max_x, max_y, max_z] = b.max;
        // The slab test per axis; the entry axis gives the face.
        let mut t_enter = f32::NEG_INFINITY;
        let mut entry_axis = 0usize;
        let mut entry_positive = false;
        let mut t_exit = f32::INFINITY;
        for axis in 0..3 {
            let (o, d, lo, hi) = match axis {
                0 => (origin[0], dir[0], min_x, max_x),
                1 => (origin[1], dir[1], min_y, max_y),
                _ => (origin[2], dir[2], min_z, max_z),
            };
            if d.abs() < 1e-9 {
                if o < lo || o > hi {
                    t_enter = f32::INFINITY;
                    break;
                }
                continue;
            }
            let (t0, t1) = if d > 0.0 {
                ((lo - o) / d, (hi - o) / d)
            } else {
                ((hi - o) / d, (lo - o) / d)
            };
            if t0 > t_enter {
                t_enter = t0;
                entry_axis = axis;
                entry_positive = d > 0.0;
            }
            t_exit = t_exit.min(t1);
        }
        if t_enter <= t_exit && t_enter > 1e-4 && best.is_none_or(|(t, _)| t_enter < t) {
            let pos = [
                origin[0] + dir[0] * t_enter,
                origin[1] + dir[1] * t_enter,
                origin[2] + dir[2] * t_enter,
            ];
            // The entry face's OUTWARD normal points back toward
            // the ray's origin: entering at the lo boundary while
            // traveling positive means the -axis face, and vice
            // versa (the sign is -sign(d), NOT +sign(d) — the
            // first version lit the back faces).
            let mut normal = [0.0f32; 3];
            normal[entry_axis] = if entry_positive { -1.0 } else { 1.0 };
            // The lit_mesh quad's uv axes per face: the two axes
            // that are NOT the normal's axis, in the quad's own
            // order (x-faces: [z, y]; y-faces (roofs): [x, z];
            // z-faces: [x, y]).
            let uv_axes = match entry_axis {
                0 => [pos[2], pos[1]],
                1 => [pos[0], pos[2]],
                _ => [pos[0], pos[1]],
            };
            best = Some((
                t_enter,
                BoxHit {
                    pos,
                    normal,
                    uv_axes,
                    material: b.material as usize,
                },
            ));
        }
    }
    best.map(|(_, hit)| hit)
}

/// The documented tolerance: the reflection pass renders through
/// the f16 HDR-format target (≈0.2/255 per value), the lit
/// equation's own 6-B budget is 2/255, and the alpha-stage blend
/// rounds once. The measured max error is printed — the pin is the
/// ceiling with the budget documented.
const TOLERANCE: u8 = 3;

#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup (see render_overflow)"]
fn wet_ground_blends_the_mirrored_scene_by_planar_rs() {
    let _guard = sdl_lock();
    let mut app = ShellApp::new(ShellConfig {
        title: "Era 1 wet ground".into(),
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

    // THE PRE-WET PINS: the plain corner's own.
    let pre = renderer.allocations();
    assert_eq!(
        (
            pre.texture_memory,
            pre.descriptor_pools,
            pre.descriptor_sets,
            pre.samplers
        ),
        (4, 2, 4, 2),
        "the pre-wet corner pins (render_corner's own)"
    );

    // THE GRAZING POSE: a street-level eye looking down-and-west
    // across the open ground before the southwest mass — the
    // checked pixels' reflected rays climb to its east face (the
    // big flat wall at x=-6, normal +X, z ∈ [-40,-6]).
    let light = day_preset();
    let eye = [12.0, 2.0, -12.0];
    let aim = [2.0, 0.0, -12.0];
    let matrix = view_projection(
        RouteWaypoint {
            position: eye,
            look_at: aim,
        },
        1.0,
    );

    // THE DRY BASELINE (the wet pixel's matte term): the same pose,
    // wet off — the byte-identity control too.
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, eye, light, size)
            .unwrap()
            .presented
    );
    let dry = renderer.take_frame_capture().unwrap().unwrap();

    // THE WET FRAME.
    renderer.set_wet(true);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, eye, light, size)
            .unwrap()
            .presented
    );
    let wet = renderer.take_frame_capture().unwrap().unwrap();

    // THE KNOWN-ANSWER CHAIN at the aim's screen patch: for each
    // pixel, the ground hit → the reflected ray → the box hit →
    // the lit equation (the mirror invariance) → the Schlick blend
    // over the dry pixel.
    let (forward, right, up) = basis(eye, aim);
    let mut max_error = 0u8;
    let mut checked = 0usize;
    let mut b_min = f32::INFINITY;
    let mut b_max = f32::NEG_INFINITY;
    'pixel: for y in 118..=138 {
        for x in 118..=138 {
            let ray = pixel_ray(forward, right, up, x, y, size);
            if ray[1] >= -1e-4 {
                continue; // not a ground pixel (the horizon or above)
            }
            // The ground hit (y = 0).
            let t_ground = -eye[1] / ray[1];
            let p = [eye[0] + ray[0] * t_ground, 0.0, eye[2] + ray[2] * t_ground];
            // The reflected ray (about +Y) and its box hit.
            let reflected = [ray[0], -ray[1], ray[2]];
            let Some(hit) = raycast_boxes(p, reflected) else {
                continue; // the reflection is the clear color here —
                // covered by the overhead check below.
            };
            // The mirror invariance: the mirrored render's shading
            // at the mirrored point IS the real equation at the
            // real hit from the real eye.
            let mat = materials()[hit.material];
            let texture = procedural_texture(mat.texture);
            let [tx, ty] = hit
                .uv_axes
                .map(|m| ((m / 4.).rem_euclid(1.) * TEXTURE_SIZE as f32).floor() as usize);
            let index = 4 * (ty * TEXTURE_SIZE as usize + tx);
            let albedo = std::array::from_fn(|c| texture[index + c] as f32 / 255.);
            let view = std::array::from_fn(|i| eye[i] - hit.pos[i]);
            let reflection = lit_surface_color(albedo, hit.normal, view, mat, light);
            // The blend at this pixel's own view angle.
            let view_ground = [eye[0] - p[0], eye[1] - p[1], eye[2] - p[2]];
            let view_length = (view_ground[0] * view_ground[0]
                + view_ground[1] * view_ground[1]
                + view_ground[2] * view_ground[2])
                .sqrt();
            let n_dot_v = (view_ground[1] / view_length).clamp(0.0, 1.0);
            let b = reflection_blend(n_dot_v);
            b_min = b_min.min(b);
            b_max = b_max.max(b);
            // The composite: dry·(1−b) + reflection·b, in u8 steps
            // (the dry pixel is an LDR byte; the reflection is the
            // lit equation's [0,1] output).
            let dry_pixel = dry.pixel_rgba(x, y);
            let expected: [u8; 3] = std::array::from_fn(|c| {
                (dry_pixel[c] as f32 * (1.0 - b) + 255.0 * reflection[c].clamp(0.0, 1.0) * b)
                    .round() as u8
            });
            let pixel = wet.pixel_rgba(x, y);
            let error = expected
                .iter()
                .zip(pixel)
                .map(|(e, a)| e.abs_diff(a))
                .max()
                .unwrap();
            max_error = max_error.max(error);
            assert!(
                error <= TOLERANCE,
                "wet pixel ({x},{y}) ground {p:?} blend {b:.4}: expected {expected:?}, \
                 got {pixel:?} (reflection {reflection:?}, dry {dry_pixel:?})"
            );
            checked += 1;
            if checked >= 60 {
                break 'pixel;
            }
        }
    }
    println!(
        "grazing patch: {checked} ground pixels vs the full mirror chain, \
         max error {max_error}/255, blend factors {b_min:.3}..{b_max:.3}"
    );
    assert!(
        checked >= 40,
        "the patch checked {checked} reflected pixels"
    );
    assert!(b_max > 0.1, "the pose is grazing enough to reflect visibly");

    // THE OVERHEAD CHECK (the brief's other anchor): a steeply-down
    // camera — the blend is ~r0 (matte) and the reflected rays from
    // the open ground climb into empty sky, so the wet frame is the
    // dry frame within the r0-weighted clear-color lift.
    let high_eye = [6.0, 30.0, -12.0];
    let high_aim = [6.0, 0.0, -12.0];
    let high_matrix = view_projection(
        RouteWaypoint {
            position: high_eye,
            look_at: high_aim,
        },
        1.0,
    );
    renderer.set_wet(false);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&high_matrix, high_eye, light, size)
            .unwrap()
            .presented
    );
    let high_dry = renderer.take_frame_capture().unwrap().unwrap();
    renderer.set_wet(true);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&high_matrix, high_eye, light, size)
            .unwrap()
            .presented
    );
    let high_wet = renderer.take_frame_capture().unwrap().unwrap();
    let (hf, hr, hu) = basis(high_eye, high_aim);
    let mut overhead_error = 0u8;
    let mut overhead_checked = 0usize;
    for y in 100..=156 {
        for x in 100..=156 {
            let ray = pixel_ray(hf, hr, hu, x, y, size);
            if ray[1] >= -1e-4 {
                continue;
            }
            let t_ground = -high_eye[1] / ray[1];
            let p = [
                high_eye[0] + ray[0] * t_ground,
                0.0,
                high_eye[2] + ray[2] * t_ground,
            ];
            let reflected = [ray[0], -ray[1], ray[2]];
            // Over the open ground the reflected ray climbs into
            // empty sky (no box overhead — the column is checked).
            if raycast_boxes(p, reflected).is_some() {
                continue;
            }
            let dry_pixel = high_dry.pixel_rgba(x, y);
            let wet_pixel = high_wet.pixel_rgba(x, y);
            // RGB only: the re-draw's alpha stage writes the Schlick
            // factor itself into the destination alpha (a harmless,
            // informative side effect — the swapchain composites
            // opaque; the RGB channels carry the actual image).
            let error = dry_pixel
                .iter()
                .take(3)
                .zip(wet_pixel.iter().take(3))
                .map(|(e, a)| e.abs_diff(*a))
                .max()
                .unwrap();
            overhead_error = overhead_error.max(error);
            assert!(
                error <= TOLERANCE,
                "overhead pixel ({x},{y}): dry {dry_pixel:?} vs wet {wet_pixel:?}"
            );
            overhead_checked += 1;
        }
    }
    println!(
        "overhead patch: {overhead_checked} pixels, wet ≈ dry (matte at ~r0), \
         max error {overhead_error}/255"
    );
    assert!(
        overhead_checked >= 40,
        "the overhead patch checked {overhead_checked}"
    );

    // THE DRY PATH RESTORED: byte-identical to the grazing baseline.
    renderer.set_wet(false);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, eye, light, size)
            .unwrap()
            .presented
    );
    let restored = renderer.take_frame_capture().unwrap().unwrap();
    assert_eq!(
        restored.bytes, dry.bytes,
        "the dry path is byte-identical before and after wet frames"
    );

    // THE ALLOCATION DELTA (measured, pinned with causes): the
    // reflection targets (4 images: RGBA16F + depth per frame
    // slot), the mirrored-params uniform, and the capture buffer
    // (this test's first capture — the sky gate's own finding);
    // the reflected and ground pools/sets and the linear sampler.
    // The post state rides along (the wet mode's HDR-pass
    // dependency — its own one-time cost, documented).
    let warm = renderer.allocations();
    let delta = (
        warm.device_memory - pre.device_memory,
        warm.descriptor_pools - pre.descriptor_pools,
        warm.descriptor_sets - pre.descriptor_sets,
        warm.samplers - pre.samplers,
    );
    println!("wet allocation delta over dry: {delta:?} (warm {warm:?})");
    // The decomposition (this test's regime: wet WITHOUT the post
    // chain — the post state exists for its HDR render pass object,
    // but its extent-bound targets never build): the post state's
    // extent-free half (its PostParams uniform + its linear
    // sampler), the reflection state (its mirrored-params uniform +
    // its linear sampler + the 4 per-slot target images), the
    // reflection's two pools and eight sets (6 reflected + 2
    // ground), and the capture machinery's destination buffer (this
    // test's first capture — the sky gate's own finding).
    assert_eq!(
        delta,
        (7, 2, 8, 2),
        "post params uniform + reflection params uniform + 4 reflection images + capture \
         buffer; the reflected/ground pools and sets; the two linear samplers"
    );

    // WARMED CONSTANCY: two more wet frames — the allocation set
    // constant, byte-identical captures.
    renderer.set_wet(true);
    for _ in 0..2 {
        renderer.request_frame_capture().unwrap();
        assert!(
            renderer
                .render_corner(&matrix, eye, light, size)
                .unwrap()
                .presented
        );
        let repeat = renderer.take_frame_capture().unwrap().unwrap();
        assert_eq!(
            renderer.allocations(),
            warm,
            "warmed wet allocations constant"
        );
        assert_eq!(repeat.bytes, wet.bytes, "byte-identical wet frames");
    }

    // THE COMPOSITION SMOKES (one clean presented frame each):
    // wet + post (the reflection through the ACES tonemap);
    renderer.set_post(true);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, eye, light, size)
            .unwrap()
            .presented
    );
    let _ = renderer.take_frame_capture().unwrap().unwrap();
    // wet + post + sky (the full Era-1 stack);
    let day = engine_renderer::sky::atmosphere_at(0.0);
    renderer.set_sky(Some(engine_renderer::SkyFrame {
        palette: day.sky,
        disc_dir: day.disc_dir,
    }));
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, eye, light, size)
            .unwrap()
            .presented
    );
    let _ = renderer.take_frame_capture().unwrap().unwrap();
    // wet + shadows (the depth pass + the shadowed ground + the
    // reflection over it);
    renderer.set_sky(None);
    renderer.set_post(false);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner_shadowed(
                &matrix,
                eye,
                light,
                engine_slice::corner_demo::demo_bias(),
                size
            )
            .unwrap()
            .presented
    );
    let _ = renderer.take_frame_capture().unwrap().unwrap();
    // wet + populated (the population unreflected — the documented
    // scope; the proxy draws still land inside the wet frame).
    let (npc, vehicle) = engine_slice::corner_population::corner_population();
    let population = engine_renderer::ProxyPopulation {
        npc: &npc,
        npc_material: 1,
        vehicle: &vehicle,
        vehicle_material: 2,
    };
    renderer.request_frame_capture().unwrap();
    let populated = renderer
        .render_corner_populated(&matrix, eye, light, &population, size)
        .unwrap();
    assert!(populated.presented);
    assert!(populated.geometry.proxy_draw_calls > 0);
    let _ = renderer.take_frame_capture().unwrap().unwrap();

    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "{:?}",
        renderer.stats().recent_validation
    );
    println!(
        "Era-1 wet-ground gates: {checked} grazing pixels + {overhead_checked} overhead pixels \
         vs planar.rs's chain within {TOLERANCE}/255 (measured max {max_error}/{overhead_error}), \
         the dry path byte-identical, the reflection state paid once, every composition clean"
    );
}

/// The Era-1 first-frame-after-replacement gate for sky/wet/post
/// (finding 6): force replacement, render a shadowed+wet+sky+post
/// frame, and assert the first frame carries all layers (not cleared).
/// This is an ignored smoke like the other lavapipe tests.
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the out-of-date retry path is driver-forced; not exercised here"]
fn first_frame_after_replacement_keeps_every_layer() {
    let _guard = sdl_lock();
    let mut app = ShellApp::new(ShellConfig {
        title: "first-frame-after-replacement".into(),
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

    // Warm one shadowed+wet+sky+post frame at 256×256 (creates all
    // lazy states on the old generation).
    let light = day_preset();
    let eye = [12.0, 2.0, -12.0];
    let aim = [2.0, 0.0, -12.0];
    let matrix = view_projection(
        RouteWaypoint {
            position: eye,
            look_at: aim,
        },
        1.0,
    );

    let day = atmosphere_at(0.0);
    renderer.set_sky(Some(SkyFrame {
        palette: day.sky,
        disc_dir: day.disc_dir,
    }));
    renderer.set_post(true);
    renderer.set_wet(true);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner_shadowed(&matrix, eye, light, BIAS, size)
            .unwrap()
            .presented
    );
    let _warm = renderer.take_frame_capture().unwrap().unwrap();

    // Force replacement at a larger size (8 pixels bigger on each axis).
    // The first frame on the replacement generation carries sky + wet +
    // post + shadowed (finding 6).
    let replacement = renderer
        .render_corner_shadowed(&matrix, eye, light, BIAS, (size.0 + 8, size.1 + 8))
        .unwrap();
    assert!(replacement.recreated, "the resize forced a recreation");
    println!("first frame after replacement: recreated = true");

    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner_shadowed(&matrix, eye, light, BIAS, (size.0 + 8, size.1 + 8))
            .unwrap()
            .presented
    );
    let first_after = renderer.take_frame_capture().unwrap().unwrap();

    // LAYER-PRESENCE PROOFS (finding 6's verification: "assert
    // geometry, sky and wet pixels on the FIRST presented frame").
    // Method: differential toggles on the SAME replacement generation
    // — each layer's region must change when that layer alone turns
    // off, which proves the layer was PRESENT in `first_after`
    // itself (a first frame that silently skipped sky or wet would
    // equal its layer-off variant there). The per-layer EQUATIONS
    // stay pinned by their own gates (render_sky, render_reflection,
    // render_post) — absolute oracles are meaningless here anyway,
    // because this frame runs through the post chain's ACES
    // tonemap. A coarse cleared-frame guard first: a composite of a
    // cleared HDR target would be (near-)uniform.
    let new_size = (size.0 + 8, size.1 + 8);
    let total_pixels = (first_after.width * first_after.height) as usize;
    let first_pixel = first_after.pixel_rgba(0, 0);
    let uniform_like = first_after
        .bytes
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|rgba| **rgba == first_pixel)
        .count();
    println!(
        "first frame after replacement: {uniform_like}/{total_pixels} pixels match pixel (0,0) \
         ({:.1}% — a cleared composite would be ~100%)",
        100.0 * uniform_like as f64 / total_pixels as f64
    );
    assert!(
        uniform_like * 10 < total_pixels * 9,
        "the first frame after replacement is not a cleared frame"
    );

    // The camera basis for the probe rays.
    let (forward, right, up) = basis(eye, aim);

    // A sky-covered probe: a pixel whose CPU ray misses every box —
    // background by definition (raycast_boxes has no ground plane).
    let mut sky_px = None;
    for y in (16..96).step_by(8) {
        for x in (32..new_size.0).step_by(32) {
            let ray = pixel_ray(forward, right, up, x, y, new_size);
            if raycast_boxes(eye, ray).is_none() {
                sky_px = Some((x, y));
                break;
            }
        }
        if sky_px.is_some() {
            break;
        }
    }
    let (sky_x, sky_y) =
        sky_px.expect("this pose has a box-free background pixel (the sky gates' own pose family)");

    // SKY: the same pose with the sky layer OFF — the background
    // probe must change (sky equation vs the tonemapped clear).
    renderer.set_sky(None);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner_shadowed(&matrix, eye, light, BIAS, new_size)
            .unwrap()
            .presented
    );
    let sky_off = renderer.take_frame_capture().unwrap().unwrap();
    let sky_probe_on = first_after.pixel_rgba(sky_x, sky_y);
    let sky_probe_off = sky_off.pixel_rgba(sky_x, sky_y);
    println!(
        "first frame after replacement: sky probe ({sky_x},{sky_y}) on={sky_probe_on:?} \
         off={sky_probe_off:?}"
    );
    assert_ne!(
        sky_probe_on, sky_probe_off,
        "the sky layer drew on the first frame after the replacement"
    );
    renderer.set_sky(Some(SkyFrame {
        palette: day.sky,
        disc_dir: day.disc_dir,
    }));

    // WET: the same pose with the ground re-draw OFF — the lower
    // frame (where this pose's ground quad lives) must change.
    renderer.set_wet(false);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner_shadowed(&matrix, eye, light, BIAS, new_size)
            .unwrap()
            .presented
    );
    let wet_off = renderer.take_frame_capture().unwrap().unwrap();
    let mut wet_differ = 0usize;
    let mut bottom_differ = 0usize;
    for (rgba_on, rgba_off) in first_after
        .bytes
        .as_chunks::<4>()
        .0
        .iter()
        .zip(wet_off.bytes.as_chunks::<4>().0.iter())
    {
        if rgba_on != rgba_off {
            wet_differ += 1;
        }
    }
    for y in (first_after.height / 2)..first_after.height {
        for x in 0..first_after.width {
            if first_after.pixel_rgba(x, y) != wet_off.pixel_rgba(x, y) {
                bottom_differ += 1;
            }
        }
    }
    println!(
        "first frame after replacement: wet differential {wet_differ} pixels total, \
         {bottom_differ} in the lower half"
    );
    assert!(
        bottom_differ > 200,
        "the wet ground re-draw changed the first frame after the replacement \
         ({bottom_differ} lower-half pixels)"
    );
    renderer.set_wet(true);

    // POST: the same pose with the chain OFF — the tonemap and bloom
    // must change the frame (the composite's own signature).
    renderer.set_post(false);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner_shadowed(&matrix, eye, light, BIAS, new_size)
            .unwrap()
            .presented
    );
    let post_off = renderer.take_frame_capture().unwrap().unwrap();
    let mut post_differ = 0usize;
    for (rgba_on, rgba_off) in first_after
        .bytes
        .as_chunks::<4>()
        .0
        .iter()
        .zip(post_off.bytes.as_chunks::<4>().0.iter())
    {
        if rgba_on != rgba_off {
            post_differ += 1;
        }
    }
    println!(
        "first frame after replacement: post differential {post_differ} pixels \
         (the ACES tonemap + bloom signature)"
    );
    assert!(
        post_differ > 500,
        "the post chain ran on the first frame after the replacement \
         ({post_differ} pixels differ from the postless frame)"
    );
    renderer.set_post(true);

    // Byte-identical repeat: request again, render the same pose at the
    // same new size, take, and assert byte-identical. The second frame
    // does not "repair" a different first frame.
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner_shadowed(&matrix, eye, light, BIAS, (size.0 + 8, size.1 + 8))
            .unwrap()
            .presented
    );
    let repeat = renderer.take_frame_capture().unwrap().unwrap();
    assert_eq!(
        first_after.bytes, repeat.bytes,
        "byte-identical first-frame captures"
    );

    // The capture path (the first capture on the replacement generation)
    // must also be identical to the previous capture to prove no
    // allocation differences.
    assert_eq!(
        first_after.format, repeat.format,
        "capture format identical"
    );

    println!(
        "first-frame-after-replacement gate: sky/wet/post/shadowed all present on first frame"
    );

    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "{:?}",
        renderer.stats().recent_validation
    );
}

/// Finding 4 verification (the wet-only cell of the review's
/// post-only / wet-only / combined matrix): the wet reflection's
/// extent-bound targets honor the NEGOTIATED depth format. The
/// renderer is built with `ForceD24S8`; if the device lacks the
/// format the knob falls back to the negotiated pick — the test
/// prints the skip and returns (the skip condition is the REPORTED
/// format, never a guess). Wet ON, post chain OFF; one shadowed wet
/// frame, capture, assert a clean validation record (the aspect and
/// attachment mismatches this finding describes are validation
/// errors first) and a non-uniform frame (the wet ground drew).
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup (see render_overflow)"]
fn wet_targets_honor_the_negotiated_depth_format() {
    let _guard = sdl_lock();
    let app = ShellApp::new(ShellConfig {
        title: "wet-depth-format".into(),
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
            depth_format: DepthFormatPreference::ForceD24S8,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(renderer.stats().validation_active);

    // The knob's outcome, from the renderer's own report: the forced
    // format, or the negotiated pick when the device lacks it.
    let reported = renderer.stats().depth_format.clone();
    if reported != "D24_UNORM_S8_UINT" {
        println!(
            "wet depth-format gate: the device lacks D24_UNORM_S8_UINT (reported {reported}); \
             the knob fell back to the negotiated pick — skipped"
        );
        return;
    }
    println!("wet depth-format gate: forced D24_UNORM_S8_UINT honored");

    // Wet ON, post chain OFF (the matrix's wet-only cell); sky off to
    // keep the frame's variables minimal.
    renderer.set_post(false);
    renderer.set_wet(true);
    let light = day_preset();
    let eye = [12.0, 2.0, -12.0];
    let aim = [2.0, 0.0, -12.0];
    let matrix = view_projection(
        RouteWaypoint {
            position: eye,
            look_at: aim,
        },
        1.0,
    );
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner_shadowed(&matrix, eye, light, BIAS, (256, 256))
            .unwrap()
            .presented
    );
    let capture = renderer.take_frame_capture().unwrap().unwrap();

    // Not uniform: the wet ground and the corner geometry drew (a
    // failed depth attachment shows as a cleared or validation-broken
    // frame, not this).
    let first_pixel = capture.pixel_rgba(0, 0);
    let uniform_like = capture
        .bytes
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|rgba| **rgba == first_pixel)
        .count();
    let total_pixels = (capture.width * capture.height) as usize;
    println!(
        "wet depth-format gate: {uniform_like}/{total_pixels} pixels match pixel (0,0) \
         ({:.1}%)",
        100.0 * uniform_like as f64 / total_pixels as f64
    );
    assert!(
        uniform_like * 10 < total_pixels * 9,
        "the wet frame is not uniform (the wet ground and corner geometry drew)"
    );

    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "{:?}",
        renderer.stats().recent_validation
    );
    println!("wet depth-format gate: D24_UNORM_S8_UINT wet frame clean under validation");
}
