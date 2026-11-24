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

use engine_renderer::lit_scene::{
    GROUND_Y, LightParams, RouteWaypoint, SCENE_EXTENT, day_preset, intersection_boxes,
    lit_surface_color, materials,
};
use engine_renderer::planar::{mirrored_eye, reflection_blend};
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

/// The whole known-answer gate at one depth-format preference
/// (finding 19's "across D32 and a supported fallback depth format"
/// half — the 6/n commit measured the D32 pass; this branch adds the
/// forced fallback pass on the same bands). The skip condition is
/// the REPORTED format, never a guess (the knob's own semantics — a
/// device without D24_S8 falls back to the negotiated pick and the
/// pass is skipped with a printed line).
fn wet_ground_gate_at(preference: DepthFormatPreference, label: &str) {
    let mut app = ShellApp::new(ShellConfig {
        title: format!("Era 1 wet ground ({label})"),
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
            depth_format: preference,
            ..Default::default()
        },
    )
    .unwrap();
    let size = app.window().size_in_pixels();
    assert_eq!(size, (256, 256));
    assert!(renderer.stats().validation_active);
    let forced_name = match preference {
        DepthFormatPreference::Auto => None,
        DepthFormatPreference::ForceD24S8 => Some("D24_UNORM_S8_UINT"),
        DepthFormatPreference::ForceD16 => Some("D16_UNORM"),
    };
    if let Some(wanted) = forced_name
        && renderer.stats().depth_format != wanted
    {
        println!(
            "wet ground {label}: the device lacks {wanted} (reported {}) — the              forced fallback pass is skipped",
            renderer.stats().depth_format
        );
        return;
    }
    println!(
        "wet ground {label}: forced {} honored",
        renderer.stats().depth_format
    );
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

    // THE GRAZING POSE (finding 18's corrective): a street-level eye
    // looking down-and-east across the open ground before the
    // southeast mass — the checked pixels' reflected rays climb to
    // its WEST face (the big flat wall at x=6, normal −X, z ∈
    // [−40,−6]) — a LIT face under the day light (normal −X against
    // dir_toward_light (−0.5, 0.707, −0.5): N·L = +0.5, so the
    // reflection term carries real diffuse AND specular; the former
    // pose's east face (normal +X) clamped N·L to zero and the
    // wrong-eye defect was invisible — the review's finding). The
    // reflection term's view vector is the VIRTUAL (mirrored) eye
    // (planar::mirrored_eye over the ground plane); the ground
    // pixel's own Schlick blend keeps the real eye.
    let light = day_preset();
    let eye = [-12.0, 2.0, 12.0];
    let aim = [-2.0, 0.0, 2.0];
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
            // Finding 18 correction: the reflection term's view vector
            // should use the virtual (mirrored) eye, not the real eye.
            // The virtual eye = ground-plane reflection of real eye:
            // virtual_eye = [eye[0], -eye[1], eye[2]] (ground plane y=0).
            // The Fresnel blend at ground keeps the real eye (the
            // Schlick n·v at the ground uses the actual view angle).
            let mat = materials()[hit.material];
            let texture = procedural_texture(mat.texture);
            let [tx, ty] = hit
                .uv_axes
                .map(|m| ((m / 4.).rem_euclid(1.) * TEXTURE_SIZE as f32).floor() as usize);
            let index = 4 * (ty * TEXTURE_SIZE as usize + tx);
            let albedo = std::array::from_fn(|c| texture[index + c] as f32 / 255.);
            let virtual_eye = mirrored_eye(eye, GROUND_Y);
            let view_virtual = std::array::from_fn(|i| virtual_eye[i] - hit.pos[i]);
            let reflection = lit_surface_color(albedo, hit.normal, view_virtual, mat, light);
            // The blend at this pixel's own view angle (real eye).
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

    // FINDING 19's coverage verification (the review's "wet coverage
    // at near and far oblique ground pixels"): the EQUAL depth test
    // must admit the re-drawn ground fragments across the whole
    // obliquity range, not just the aim patch — a cross-pipeline
    // position divergence (the finding's failure mode) silently
    // rejects the overlay exactly where the ground is most oblique,
    // and the aim patch alone cannot see it. Two bands on the SAME
    // captures, each through the full known-answer chain, with two
    // guards the aim patch never needed (its camera rays reach open
    // road close-in): the FAR band's rays travel low enough to be
    // blocked by building masses before the ground (the visibility
    // guard — a downward ray from above ground can only meet a box
    // BEFORE its ground hit, since the boxes rise from y=0), and
    // both bands can project ground beyond the mesh's
    // ±SCENE_EXTENT quad (the on-mesh guard). The reflection term
    // extends the aim patch's chain with the mirror pass's CLEAR
    // color — the ambient (corner.rs :: FrameParams::clear_color)
    // — for the pixels whose reflected rays climb into open sky.
    // Every checked pixel carries its own known answer, and each
    // band must contain pixels whose expected wet−dry delta EXCEEDS
    // the tolerance (the self-discrimination control, the finding-13
    // discipline): an EQUAL rejection leaves the wet pixel at its
    // dry value, and exactly those pixels fail.
    #[derive(Default)]
    struct BandCensus {
        checked: usize,
        box_reflections: usize,
        clear_reflections: usize,
        discriminating: usize,
        b_sum: f32,
        max_error: u8,
    }
    let mut far_band = BandCensus::default();
    let mut near_band = BandCensus::default();
    for (label, rows, columns, census) in [
        ("far", (102u32, 156), (110u32, 170), &mut far_band),
        ("near", (160u32, 250), (110u32, 170), &mut near_band),
    ] {
        for y in rows.0..=rows.1 {
            for x in columns.0..=columns.1 {
                let ray = pixel_ray(forward, right, up, x, y, size);
                if ray[1] >= -1e-4 {
                    continue; // the horizon or above
                }
                let t_ground = -eye[1] / ray[1];
                // The visibility guard: a building mass between the
                // eye and the ground point makes this pixel a wall
                // pixel — the overlay never draws there.
                if raycast_boxes(eye, ray).is_some() {
                    continue;
                }
                let p = [
                    eye[0] + ray[0] * t_ground,
                    GROUND_Y,
                    eye[2] + ray[2] * t_ground,
                ];
                // The on-mesh guard: the ground quad spans
                // ±SCENE_EXTENT; beyond it there is no depth to
                // EQUAL-match and no overlay.
                if p[0].abs() > SCENE_EXTENT || p[2].abs() > SCENE_EXTENT {
                    continue;
                }
                // The Schlick blend at this pixel's own view angle
                // (the real eye — the ground term).
                let view_ground = [eye[0] - p[0], eye[1] - p[1], eye[2] - p[2]];
                let view_length = (view_ground[0] * view_ground[0]
                    + view_ground[1] * view_ground[1]
                    + view_ground[2] * view_ground[2])
                    .sqrt();
                let n_dot_v = (view_ground[1] / view_length).clamp(0.0, 1.0);
                let b = reflection_blend(n_dot_v);
                // The reflection term: the reflected ray's first box
                // hit shaded from the VIRTUAL eye (the aim patch's
                // own chain), or the mirror pass's clear color when
                // it climbs into open sky.
                let reflected = [ray[0], -ray[1], ray[2]];
                let reflection: [f32; 3] = match raycast_boxes(p, reflected) {
                    Some(hit) => {
                        census.box_reflections += 1;
                        let mat = materials()[hit.material];
                        let texture = procedural_texture(mat.texture);
                        let [tx, ty] = hit.uv_axes.map(|m| {
                            ((m / 4.).rem_euclid(1.) * TEXTURE_SIZE as f32).floor() as usize
                        });
                        let index = 4 * (ty * TEXTURE_SIZE as usize + tx);
                        let albedo = std::array::from_fn(|c| texture[index + c] as f32 / 255.);
                        let virtual_eye = mirrored_eye(eye, GROUND_Y);
                        let view_virtual = std::array::from_fn(|i| virtual_eye[i] - hit.pos[i]);
                        lit_surface_color(albedo, hit.normal, view_virtual, mat, light)
                    }
                    None => {
                        census.clear_reflections += 1;
                        light.ambient
                    }
                };
                // The composite over the dry pixel (the aim patch's
                // own u8-space form).
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
                census.max_error = census.max_error.max(error);
                assert!(
                    error <= TOLERANCE,
                    "{label} pixel ({x},{y}) ground {p:?} blend {b:.4}: expected \
                     {expected:?}, got {pixel:?} (reflection {reflection:?}, dry \
                     {dry_pixel:?})"
                );
                census.checked += 1;
                census.b_sum += b;
                // Self-discrimination: this pixel's known answer
                // must be ABLE to fail on an EQUAL rejection — the
                // expected wet−dry delta has to exceed the
                // tolerance, or the check is vacuous for the
                // finding's failure mode.
                let delta = expected
                    .iter()
                    .zip(dry_pixel)
                    .map(|(e, d)| e.abs_diff(d))
                    .max()
                    .unwrap();
                if delta > TOLERANCE {
                    census.discriminating += 1;
                }
            }
        }
    }
    for (label, band) in [("far", &far_band), ("near", &near_band)] {
        println!(
            "{label} band: {} ground pixels ({} box reflections, {} clear), \
             {} discriminating, mean blend {:.3}, max error {}/255",
            band.checked,
            band.box_reflections,
            band.clear_reflections,
            band.discriminating,
            band.b_sum / band.checked as f32,
            band.max_error
        );
    }
    assert!(
        far_band.checked >= 2350,
        "the far band checked {} ground pixels (measured 2619 on the \
         2026-11-24 lavapipe run — the count is CPU-geometry-deterministic)",
        far_band.checked
    );
    assert!(
        far_band.discriminating >= 850,
        "the far band's known answers must be able to fail on an EQUAL \
         rejection — {} of {} pixels discriminate (measured 1086)",
        far_band.discriminating,
        far_band.checked
    );
    assert!(
        near_band.checked >= 5000,
        "the near band checked {} ground pixels (measured 5551 on the \
         2026-11-24 lavapipe run — the count is CPU-geometry-deterministic)",
        near_band.checked
    );
    assert!(
        near_band.discriminating >= 1200,
        "the near band's known answers must be able to fail on an EQUAL \
         rejection — {} of {} pixels discriminate (measured 1508)",
        near_band.discriminating,
        near_band.checked
    );
    // The obliquity separation: the far band must be measurably MORE
    // oblique than the near band (deeper mean Schlick blends) — two
    // bands sampling the same regime would not verify "near and
    // far" anything.
    let far_mean_b = far_band.b_sum / far_band.checked as f32;
    let near_mean_b = near_band.b_sum / near_band.checked as f32;
    assert!(
        far_mean_b > near_mean_b + 0.05,
        "the far band's mean blend {far_mean_b:.3} must exceed the near \
         band's {near_mean_b:.3} by a visible margin"
    );

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
                continue; // not looking at ground (horizon or above)
            }
            // First intersect the camera ray with opaque geometry
            // (only ground-visible rays qualify — a roof pixel must
            // not be checked as ground). The ground is at y=0.
            let t_ground = -high_eye[1] / ray[1];
            let p = [
                high_eye[0] + ray[0] * t_ground,
                0.0,
                high_eye[2] + ray[2] * t_ground,
            ];
            // Check that the ground pixel is actually visible (no
            // opaque geometry between camera and ground). The
            // overhead check needs visible ground to have contrast
            // between wet and dry.
            let ground_hit = raycast_boxes(high_eye, ray);
            if ground_hit.is_none() {
                // The ray hits nothing before ground — it's visible.
                // But the ground hit itself might not be ground —
                // check it's not a roof pixel that just happens to
                // have negative ray[1].
                let reflected = [ray[0], -ray[1], ray[2]];
                if raycast_boxes(p, reflected).is_some() {
                    continue; // the reflection hits something — not open ground
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
        "Era-1 wet-ground gates: {checked} grazing + {overhead_checked} overhead + {} far \
         + {} near band pixels vs planar.rs's chain within {TOLERANCE}/255 (measured max \
         {max_error}/{overhead_error}/{}), the dry path byte-identical, the reflection \
         state paid once, every composition clean",
        far_band.checked,
        near_band.checked,
        far_band.max_error.max(near_band.max_error)
    );
}

#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup (see render_overflow)"]
fn wet_ground_blends_the_mirrored_scene_by_planar_rs() {
    let _guard = sdl_lock();
    // Finding 19's check: "Verify wet coverage at near and far oblique
    // ground pixels across D32 and a supported fallback depth format."
    // The whole gate runs at BOTH: the negotiated pick (D32 on the
    // pod's lavapipe) and the forced D24_S8 fallback (the format with
    // the stencil aspect — the aspect/attachment class the D32 pass
    // can never exercise).
    wet_ground_gate_at(DepthFormatPreference::Auto, "negotiated (D32)");
    wet_ground_gate_at(DepthFormatPreference::ForceD24S8, "forced D24_S8");
}

/// FINDING 18'S BEYOND-TOLERANCE DISCRIMINATION — the review's check:
/// "Pick a lit material and pose where changing only the specular
/// eye moves expected RGB beyond tolerance." The 4/n commit fixed
/// the oracle (the reflected hit shades toward the VIRTUAL,
/// ground-mirrored eye; the ground Fresnel keeps the real eye) and
/// moved the grazing pose to the southeast mass's lit west face —
/// but at the scene's fixed materials and day_preset the swing
/// bounds at ~1-2/255, below the gate's 3/255 tolerance, so the
/// fixture could not itself discriminate a real-eye regression.
///
/// The method: a CPU-side sensitivity search over (eye pose, light
/// direction, light-color scale) — the swing is
/// 255·b·|reflection(virtual eye) − reflection(real eye)| per
/// channel (the dry term cancels in the difference; only the
/// specular term is view-dependent — "changing only the specular
/// eye" exactly). LightParams is a per-frame argument, so the search
/// needs no material change. When a candidate clears 2× the gate
/// tolerance, the second half lands the GPU gate there: the capture
/// must match the VIRTUAL-eye composite within tolerance while the
/// REAL-eye composite differs beyond it at the same probes. When no
/// candidate clears it, the measured bound is PINNED (a tripwire: a
/// future material or geometry change that creates sensitivity
/// fails this test and prompts the full gate — never a silent pass).
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup (see render_overflow)"]
fn the_grazing_specular_eye_swing_discriminates_beyond_tolerance_or_pins_the_bound() {
    let _guard = sdl_lock();
    let size = (256u32, 256u32);
    let aim = [2.0, 0.0, 2.0];

    /// The mirror chain at one pixel: (blend, reflection toward the
    /// virtual eye, reflection toward the real eye, ground hit).
    fn chain_at(
        eye: [f32; 3],
        basis: ([f32; 3], [f32; 3], [f32; 3]),
        x: u32,
        y: u32,
        size: (u32, u32),
        light: LightParams,
        textures: &[Vec<u8>; 3],
    ) -> Option<(f32, [f32; 3], [f32; 3])> {
        let (forward, right, up) = basis;
        let ray = pixel_ray(forward, right, up, x, y, size);
        if ray[1] >= -1e-4 {
            return None; // not a ground pixel
        }
        let t_ground = -eye[1] / ray[1];
        let p = [
            eye[0] + ray[0] * t_ground,
            GROUND_Y,
            eye[2] + ray[2] * t_ground,
        ];
        let reflected = [ray[0], -ray[1], ray[2]];
        let hit = raycast_boxes(p, reflected)?;
        let mat = materials()[hit.material];
        let texture = &textures[hit.material];
        let [tx, ty] = hit
            .uv_axes
            .map(|m| ((m / 4.).rem_euclid(1.) * TEXTURE_SIZE as f32).floor() as usize);
        let index = 4 * (ty * TEXTURE_SIZE as usize + tx);
        let albedo = std::array::from_fn(|c| texture[index + c] as f32 / 255.);
        let virtual_eye = mirrored_eye(eye, GROUND_Y);
        let view_virtual = std::array::from_fn(|i| virtual_eye[i] - hit.pos[i]);
        let view_real = std::array::from_fn(|i| eye[i] - hit.pos[i]);
        let reflection_virtual = lit_surface_color(albedo, hit.normal, view_virtual, mat, light);
        let reflection_real = lit_surface_color(albedo, hit.normal, view_real, mat, light);
        let view_ground = [eye[0] - p[0], eye[1] - p[1], eye[2] - p[2]];
        let view_length = (view_ground[0] * view_ground[0]
            + view_ground[1] * view_ground[1]
            + view_ground[2] * view_ground[2])
            .sqrt();
        let n_dot_v = (view_ground[1] / view_length).clamp(0.0, 1.0);
        Some((
            reflection_blend(n_dot_v),
            reflection_virtual,
            reflection_real,
        ))
    }

    // The searched space (documented, bounded): the grazing-pose
    // family looking at the southeast mass's west face (four eye
    // heights), the day light plus a direction grid over its lit
    // faces, and color scales on the day direction.
    // The materials' textures, generated ONCE (the search evaluates
    // ~157k chain points; a per-call procedural_texture would
    // regenerate 256x256 texels each time).
    let textures: [Vec<u8>; 3] =
        std::array::from_fn(|i| procedural_texture(materials()[i].texture));
    let day = day_preset();
    let mut lights: Vec<(String, LightParams)> = vec![("day".into(), day)];
    for (elev_deg, azim_deg) in [
        (30.0_f32, 225.0_f32),
        (30.0, 135.0),
        (60.0, 225.0),
        (60.0, 135.0),
    ] {
        let (e, a) = (elev_deg.to_radians(), azim_deg.to_radians());
        lights.push((
            format!("elev {elev_deg:.0} azim {azim_deg:.0}"),
            LightParams {
                dir_toward_light: [e.cos() * a.cos(), e.sin(), e.cos() * a.sin()],
                ..day
            },
        ));
    }
    for scale in [2.0_f32, 4.0, 8.0] {
        lights.push((
            format!("day x{scale:.0}"),
            LightParams {
                light_color: day.light_color.map(|c| c * scale),
                ..day
            },
        ));
    }
    let mut best_swing = 0.0_f32;
    let mut best = (
        [-12.0_f32, 2.0, 12.0],
        day,
        String::new(),
        128u32,
        128u32,
        [0.0_f32; 3],
        [0.0_f32; 3],
        0.0_f32,
    );
    for height in [1.0_f32, 2.0, 3.0, 5.0] {
        let eye = [-12.0, height, 12.0];
        let eye_basis = basis(eye, aim);
        for (light_name, light) in &lights {
            for y in (60..200u32).step_by(2) {
                for x in (60..200u32).step_by(2) {
                    let Some((b, rv, rr)) = chain_at(eye, eye_basis, x, y, size, *light, &textures)
                    else {
                        continue;
                    };
                    // The OBSERVABLE swing: the presented composite
                    // clamps the reflection to [0,1] BEFORE the blend
                    // (a saturated x8-light candidate looked like 23/255
                    // raw and rendered IDENTICAL composites — both
                    // clamped to white). The dry term still cancels:
                    // composite_v - composite_r = 255*b*(clamp(rv) -
                    // clamp(rr)) per channel.
                    let swing = rv
                        .iter()
                        .zip(rr)
                        .map(|(v, r)| 255.0 * b * (v.clamp(0.0, 1.0) - r.clamp(0.0, 1.0)).abs())
                        .fold(0.0_f32, f32::max);
                    if swing > best_swing {
                        best_swing = swing;
                        best = (eye, *light, light_name.clone(), x, y, rv, rr, b);
                    }
                }
            }
        }
    }
    let (best_eye, best_light, best_light_name, best_x, best_y, best_rv, best_rr, best_b) = best;
    println!(
        "specular-eye swing search: max {best_swing:.1}/255 at eye {best_eye:?} light \
         {best_light_name}, pixel ({best_x},{best_y}) blend {best_b:.3} — virtual {:?} vs real \
         {:?}",
        best_rv.map(|c| (255.0 * c.clamp(0.0, 1.0)).round() as u8),
        best_rr.map(|c| (255.0 * c.clamp(0.0, 1.0)).round() as u8)
    );

    // The discrimination bar: 2x the gate tolerance (a swing at or
    // below tolerance could hide inside the comparison's slack).
    const BAR: f32 = 2.0 * TOLERANCE as f32;
    assert!(
        best_swing > 0.5,
        "the search found no lit reflection at all — the space or the chain is broken"
    );
    if best_swing < BAR {
        println!(
            "BOUND: the maximum swing ({best_swing:.1}/255) stays below the {BAR:.0}/255 bar \
             over the searched space (4 grazing heights x {} lights, 70 x 70 probe rays each) \
             — the dedicated material the review names is genuinely required at the scene's \
             fixed materials; the assertions below pin the bound",
            lights.len()
        );
        assert!(
            best_swing < BAR,
            "the bound moved past the bar ({best_swing:.1} >= {BAR:.0}) — land the GPU gate \
             half (see the review's finding 18 check)"
        );
        return;
    }

    // === THE GPU GATE at the searched pose: the wet capture must
    // match the VIRTUAL-eye composite within tolerance while the
    // REAL-eye composite differs beyond it at the same probes.
    let app = ShellApp::new(ShellConfig {
        title: "finding-18 specular-eye swing".into(),
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
    let size_in_pixels = app.window().size_in_pixels();
    assert_eq!(
        size_in_pixels, size,
        "the fixture renders at the searched extent"
    );
    let best_basis = basis(best_eye, aim);
    let matrix = view_projection(
        RouteWaypoint {
            position: best_eye,
            look_at: aim,
        },
        1.0,
    );

    // The dry baseline, then the wet frame, at the searched light.
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, best_eye, best_light, size)
            .unwrap()
            .presented
    );
    let dry = renderer.take_frame_capture().unwrap().unwrap();
    renderer.set_wet(true);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, best_eye, best_light, size)
            .unwrap()
            .presented
    );
    let wet = renderer.take_frame_capture().unwrap().unwrap();

    // The probes: the 5x5 patch around the argmax pixel (each must
    // carry a box reflection and a swing of its own).
    let mut checked = 0usize;
    let mut discriminating = 0usize;
    let mut max_error = 0u8;
    let mut min_discrimination = u8::MAX;
    for y in best_y.saturating_sub(2)..=best_y + 2 {
        for x in best_x.saturating_sub(2)..=best_x + 2 {
            let Some((b, rv, rr)) =
                chain_at(best_eye, best_basis, x, y, size, best_light, &textures)
            else {
                continue;
            };
            let dry_pixel = dry.pixel_rgba(x, y);
            let composite = |reflection: [f32; 3]| -> [u8; 3] {
                std::array::from_fn(|c| {
                    (dry_pixel[c] as f32 * (1.0 - b) + 255.0 * reflection[c].clamp(0.0, 1.0) * b)
                        .round() as u8
                })
            };
            let expected_virtual = composite(rv);
            let expected_real = composite(rr);
            let pixel = wet.pixel_rgba(x, y);
            let error = expected_virtual
                .iter()
                .zip(pixel)
                .map(|(e, a)| e.abs_diff(a))
                .max()
                .unwrap();
            assert!(
                error <= TOLERANCE,
                "wet pixel ({x},{y}): expected the VIRTUAL-eye composite {expected_virtual:?}, \
                 got {pixel:?} (blend {b:.3}, dry {dry_pixel:?})"
            );
            let discrimination = expected_virtual
                .iter()
                .zip(expected_real)
                .map(|(v, r)| v.abs_diff(r))
                .max()
                .unwrap();
            // The oracle is strict at EVERY probe; the discrimination
            // is a FLOOR over the patch (the swing varies per pixel —
            // a neighbor can carry none while the argmax carries the
            // bar; the patch exists to prove the argmax is not a
            // one-pixel fluke).
            if discrimination > TOLERANCE {
                discriminating += 1;
            }
            max_error = max_error.max(error);
            min_discrimination = min_discrimination.min(discrimination);
            checked += 1;
        }
    }
    assert!(checked >= 9, "the patch carried {checked} reflected probes");
    assert!(
        discriminating >= 9,
        "only {discriminating} of {checked} patch pixels discriminate the specular eye \
         beyond {TOLERANCE}/255 — the argmax was a fluke"
    );
    println!(
        "the specular-eye gate at the searched pose: {checked} probe pixels, max error \
         {max_error}/255 vs the virtual-eye oracle, {discriminating} pixels discriminate the \
         real-vs-virtual eye beyond {TOLERANCE}/255"
    );
    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "validation: {:?}",
        renderer.stats().recent_validation
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

/// Finding 5 verification (the shadowed wet ground's camera): the
/// ground re-draw binds the CORNER buffer's slot region — a block
/// only the ordinary lighting branch ever wrote. On a shadowed+wet
/// frame the corner region held a previous pose's camera (or
/// uninitialized bytes on the very first frame): the EQUAL depth
/// test and the Schlick Fresnel both read a stale camera. The fix
/// populates the corner block before either lighting branch on
/// every shadowed frame.
///
/// The gate: the renderer's FIRST corner frame is SHADOWED+wet
/// (the uninitialized-camera window), then four wet captures
/// alternate two distinct poses across both frame slots (A, B, A,
/// B — each slot's corner region receives a different camera), and
/// each capture is checked against the file's own MEASURED mirror
/// chain oracle (the grazing gate's model — the ground hit, the
/// reflected ray, the lit equation, the Schlick blend) over a
/// probe patch, against THAT pose's own shadowed dry capture (the
/// blend's matte term is the shadowed scene pass's ground pixel).
/// A stale camera on either slot shifts the blend factor and the
/// EQUAL-depth footprint — both fail loudly at the patch. The
/// review's post-enabled variant is covered by a differential: the
/// HDR scene pass's wet-vs-dry pixels at the same patch (through
/// the ACES tonemap the blend is not directly oracle-comparable;
/// a stale camera would reject the EQUAL-depth redraw entirely and
/// the wet frame would equal the dry one).
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup (see render_overflow)"]
fn shadowed_wet_ground_reads_the_current_camera() {
    let _guard = sdl_lock();
    let mut app = ShellApp::new(ShellConfig {
        title: "shadowed wet camera".into(),
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

    // The two poses: the grazing gate's own pose (street level,
    // looking down-and-west — the reflected rays climb to the
    // southwest mass's east face), and a second camera at a
    // different height and lateral offset aiming at the same ground
    // region (a measurably different view angle — a different Schlick
    // factor — and different ground hits, so each slot's corner
    // region receives a distinguishable camera).
    let light = day_preset();
    let poses = [
        (
            [12.0, 2.0, -12.0],
            [2.0, 0.0, -12.0],
            "grazing street level",
        ),
        (
            [12.0, 3.5, -9.0],
            [2.0, 0.0, -12.0],
            "raised lateral offset",
        ),
    ];
    let matrices = poses.map(|(eye, aim, _)| {
        view_projection(
            RouteWaypoint {
                position: eye,
                look_at: aim,
            },
            1.0,
        )
    });

    // Wet ON, post OFF, sky OFF — the finding's scope. The FIRST
    // corner frame this renderer draws is SHADOWED+wet: the corner
    // region's uninitialized-camera window.
    renderer.set_wet(true);
    renderer.set_post(false);
    renderer.set_sky(None);

    // Four wet captures alternating the poses across both frame
    // slots (A, B, A, B — the capture-only renders alternate slots
    // naturally; no dry render interleaves to disturb them).
    let wet_captures: Vec<_> = ["A", "B", "A", "B"]
        .iter()
        .map(|&pose| {
            let index = usize::from(pose == "B");
            let (eye, _aim, _) = poses[index];
            renderer.request_frame_capture().unwrap();
            assert!(
                renderer
                    .render_corner_shadowed(&matrices[index], eye, light, BIAS, size)
                    .unwrap()
                    .presented
            );
            (pose, renderer.take_frame_capture().unwrap().unwrap())
        })
        .collect();

    // The dry term per pose: the SAME shadowed scene pass with the
    // ground re-draw OFF (the blend's matte term is whatever the
    // shadowed scene pass rendered there — lit or shadowed ground,
    // the captured term keeps the model valid either way).
    renderer.set_wet(false);
    let dry_captures: Vec<_> = poses
        .iter()
        .map(|(eye, _aim, _)| {
            let index = poses.iter().position(|p| p.0 == *eye).unwrap();
            renderer.request_frame_capture().unwrap();
            assert!(
                renderer
                    .render_corner_shadowed(&matrices[index], *eye, light, BIAS, size)
                    .unwrap()
                    .presented
            );
            renderer.take_frame_capture().unwrap().unwrap()
        })
        .collect();

    // The grazing gate's own oracle, per pose, over a probe patch:
    // the ground hit → the reflected ray → the box hit → the lit
    // equation → the Schlick blend over the pose's own dry pixel.
    let mut grand_max_error = 0u8;
    for (index, (eye, aim, what)) in poses.iter().enumerate() {
        let (forward, right, up) = basis(*eye, *aim);
        let wet = wet_captures
            .iter()
            .filter(|(pose, _)| usize::from(*pose == "B") == index)
            .map(|(_, capture)| capture)
            .collect::<Vec<_>>();
        let dry = &dry_captures[index];
        let mut max_error = 0u8;
        let mut checked = 0usize;
        let mut b_max = f32::NEG_INFINITY;
        'pixel: for y in 108..=148 {
            for x in 108..=148 {
                let ray = pixel_ray(forward, right, up, x, y, size);
                if ray[1] >= -1e-4 {
                    continue; // not a ground pixel
                }
                let t_ground = -eye[1] / ray[1];
                let p = [eye[0] + ray[0] * t_ground, 0.0, eye[2] + ray[2] * t_ground];
                let reflected = [ray[0], -ray[1], ray[2]];
                let Some(hit) = raycast_boxes(p, reflected) else {
                    continue; // the reflection is the clear color here
                };
                let mat = materials()[hit.material];
                let texture = procedural_texture(mat.texture);
                let [tx, ty] = hit
                    .uv_axes
                    .map(|m| ((m / 4.).rem_euclid(1.) * TEXTURE_SIZE as f32).floor() as usize);
                let index = 4 * (ty * TEXTURE_SIZE as usize + tx);
                let albedo = std::array::from_fn(|c| texture[index + c] as f32 / 255.);
                let view = std::array::from_fn(|i| eye[i] - hit.pos[i]);
                let reflection = lit_surface_color(albedo, hit.normal, view, mat, light);
                let view_ground = [eye[0] - p[0], eye[1] - p[1], eye[2] - p[2]];
                let view_length = (view_ground[0] * view_ground[0]
                    + view_ground[1] * view_ground[1]
                    + view_ground[2] * view_ground[2])
                    .sqrt();
                let n_dot_v = (view_ground[1] / view_length).clamp(0.0, 1.0);
                let b = reflection_blend(n_dot_v);
                b_max = b_max.max(b);
                let dry_pixel = dry.pixel_rgba(x, y);
                let expected: [u8; 3] = std::array::from_fn(|c| {
                    (dry_pixel[c] as f32 * (1.0 - b) + 255.0 * reflection[c].clamp(0.0, 1.0) * b)
                        .round() as u8
                });
                for capture in &wet {
                    let pixel = capture.pixel_rgba(x, y);
                    let error = expected
                        .iter()
                        .zip(pixel)
                        .map(|(e, a)| e.abs_diff(a))
                        .max()
                        .unwrap();
                    max_error = max_error.max(error);
                    assert!(
                        error <= TOLERANCE,
                        "pose {what} ({x},{y}) ground {p:?} blend {b:.4}: expected {expected:?}, \
                         got {pixel:?} (reflection {reflection:?}, dry {dry_pixel:?})"
                    );
                    checked += 1;
                }
                if checked >= 40 {
                    break 'pixel;
                }
            }
        }
        println!(
            "shadowed+wet pose {what}: {checked} probe comparisons vs the mirror-chain oracle, \
             max error {max_error}/255, max blend {b_max:.3}"
        );
        assert!(
            checked >= 20,
            "pose {what} checked {checked} reflected pixels — the patch went vacuous"
        );
        assert!(
            b_max > 0.1,
            "pose {what} reflects visibly (b_max {b_max:.3})"
        );
        grand_max_error = grand_max_error.max(max_error);
    }

    // The post-enabled variant (the HDR scene pass): a differential —
    // the wet frame must differ from the dry frame at the probe
    // patch (through the ACES tonemap the blend is not directly
    // oracle-comparable, but a stale camera would reject the
    // EQUAL-depth ground redraw entirely and the two would match).
    renderer.set_post(true);
    renderer.set_wet(true);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner_shadowed(&matrices[0], poses[0].0, light, BIAS, size)
            .unwrap()
            .presented
    );
    let hdr_wet = renderer.take_frame_capture().unwrap().unwrap();
    renderer.set_wet(false);
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner_shadowed(&matrices[0], poses[0].0, light, BIAS, size)
            .unwrap()
            .presented
    );
    let hdr_dry = renderer.take_frame_capture().unwrap().unwrap();
    let mut hdr_differ = 0usize;
    for y in 108..=148 {
        for x in 108..=148 {
            hdr_differ += usize::from(hdr_wet.pixel_rgba(x, y) != hdr_dry.pixel_rgba(x, y));
        }
    }
    println!("shadowed+wet HDR variant: {hdr_differ} patch pixels differ wet-vs-dry");
    assert!(
        hdr_differ >= 20,
        "the HDR scene pass's ground re-draw blended ({hdr_differ} patch pixels differ)"
    );

    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "{:?}",
        renderer.stats().recent_validation
    );
    println!(
        "shadowed+wet camera gate: both slots' ground re-draws read the CURRENT camera — \
         {grand_max_error}/255 max against the measured oracle, the HDR variant blended"
    );
}
