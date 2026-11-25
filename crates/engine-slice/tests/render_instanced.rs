//! The Cycle 6-E instanced-draw equivalence gate (see
//! docs/CYCLE6-6E-BRIEF.md, acceptance criterion 1): the SAME proxy
//! population — same meshes, same transforms, same tints, same
//! materials — drawn as N single-instance draws (the pre-6-E
//! per-object submission shape, `ProxyDrawMode::PerInstance`) versus
//! ONE instanced draw per kind (`Instanced`) must produce
//! BYTE-EQUAL captured frames, with the draw-call counts pinned
//! (criterion 2) and `renderer.allocations()` constant across warmed
//! populated frames (criterion 3, the zero-per-frame-allocation
//! gate).
//!
//! A second, independent pin closes the shared-bug hole an A/B
//! comparison alone leaves open (both modes could be identically
//! wrong): single-instance captures under orthographic cameras are
//! checked per-pixel against the 6-B CPU oracle
//! (`lit_surface_color` with `albedo * tint`) — the render_corner
//! discipline applied to the instance transform conventions (a
//! wrong matrix/turn/scale convention would move the proxy's screen
//! rectangle and fail the position and color pins together).
//!
//! The 6-E2 gate (the third test below) extends the same discipline
//! to the indirect submission: `ProxyDrawMode::Indirect` — one
//! `vkCmdDrawIndirect` per kind, the instance counts read from GPU
//! memory — must be byte-equal to BOTH the direct instanced and the
//! per-object frames, with the lazy indirect command buffer's
//! one-time allocation pinned (the direct path's counters untouched)
//! and a single-instance oracle wall run THROUGH the indirect
//! submission (the independent CPU-reference pin, not just A/B
//! equality).
//!
//! `#[ignore]`d like the other lavapipe smokes: it requires
//! `SDL_VIDEODRIVER=offscreen` plus a software Vulkan ICD (lavapipe)
//! and the validation layer. The dev pod needs the user-local ICD
//! setup (see render_overflow). CI's Linux runner executes it
//! enforced (the workflow's instanced-proxy step); the per-stage
//! `println!` evidence below is what the CI log must show. Run
//! explicitly:
//!
//! ```sh
//! SDL_VIDEODRIVER=offscreen cargo test -p engine-slice --test render_instanced -- \
//!     --ignored --nocapture
//! ```
//!
//! The Era-2 gates (the animation + skinning + proxy-shadows
//! package, stage 5): `skinned_pose_matches_the_cpu_oracle` — one
//! WALKING npc's torso front face per-pixel against the CPU
//! skinning equation (`skin_vertex` composed with the instance
//! transform) through a dead-on camera derived from the SKINNED
//! rectangle; `animated_population_matches_per_object_pixels` — the
//! N-vs-1 submission equivalence UNDER MOTION (frame-33 walk blocks
//! through the per-object and instanced paths, byte-equal); and the
//! animated regime inside the full-table gate below — the
//! allocation counters (Vulkan AND host) constant across warmed
//! WALKING frames with the per-frame pose digests proving the crowd
//! moves.

use engine_renderer::lit_scene::{LightParams, day_preset, lit_surface_color, materials};
use engine_renderer::populated_meshes::{WHEEL_SEGMENTS, sedan_mesh};
use engine_renderer::proc_texture::{TEXTURE_SIZE, procedural_texture};
use engine_renderer::proxy_instances::{INSTANCE_STRIDE, InstanceTransform, ProxyInstance};
use engine_renderer::{
    CapturedFrame, DemoScenePreference, ProxyDrawMode, ProxyPopulation, RenderConfig, Renderer,
    ValidationPreference,
};
use engine_shell::{DriverPreference, ShellApp, ShellConfig};

// The host allocation counter test-support module (finding 10's
// missing gate; tests/common/ — not a standalone test target).
mod common;

/// SDL's initialization is process-global (the sdl3 crate declares
/// one main thread per process; a second init while another test's
/// SDL is alive fails the main-thread check — the engine-shell test
/// binaries' `sdl_lock` pattern, applied to this binary now that it
/// carries two SDL-touching gates). The guard is declared FIRST in
/// each test so it drops LAST (reverse declaration order): the app
/// and renderer are fully dropped — SDL quit, the main-thread
/// declaration reset — before the lock releases and the next test
/// initializes.
fn sdl_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}

/// The Era-2 skinned family's per-replacement resource cost, pinned
/// (the first device execution's finding, CI run 45488497488 on
/// lavapipe): the pipeline-replacement lifecycle re-ensures every
/// family the outgoing state owned (the finding-A discipline), and
/// the skinned family's re-ensure re-creates its descriptor pool and
/// its three per-material sets — the allocation counters are
/// cumulative creation totals that never decrement, so each
/// replacement adds exactly this once. The 6-E2-era pin held these
/// counters FLAT across a replacement because no family on the
/// populated path then owned descriptor-class resources (the proxy
/// family's re-ensure grows only device_memory — the documented
/// delta this test prints); the Era-2 skinned family legitimately
/// extended that growth class, and the correction is this EXACT
/// per-replacement pin — not a loosened assertion. The lifecycle
/// blocks below force TWO replacements each (the resize out, then
/// the capture back at the original extent). MEASURED: two
/// replacements grew the tuple by exactly +2 pools / +6 sets.
///
/// The deeper fix — a pass-independent ownership transfer so the
/// skinned pool/sets/pose-buffer survive replacements and only the
/// render-pass-bound pipelines re-create — is the recorded
/// renderer-architecture follow-up (it would also cover the
/// shadowed-populated family, whose sets follow the re-created
/// shadow state's map views and so cannot transfer as easily).
const SKINNED_REPLACEMENT_POOLS: u64 = 1;
const SKINNED_REPLACEMENT_SETS: u64 = 3;
const LIFECYCLE_REPLACEMENTS: u64 = 2;

/// The gate's fixed population: 10 NPCs + 3 vehicles (13 instances —
/// the N in the N-vs-1 claim), placed in the intersection's
/// road-cross frame. A depth STACK of three NPCs along the southwest
/// sight line exercises cross-instance occlusion; sidewalk and lane
/// placements exercise the population shapes; varied turns (all four
/// quadrants), scales, and tints exercise the packed matrix bytes.
fn gate_population() -> (Vec<ProxyInstance>, Vec<ProxyInstance>) {
    let npc = |pos: [f32; 3], turn: u16, scale: f32, tint: [f32; 3]| ProxyInstance {
        transform: InstanceTransform { pos, turn, scale },
        tint,
        pose_index: 0,
    };
    let vehicle = |pos: [f32; 3], turn: u16, tint: [f32; 3]| ProxyInstance {
        transform: InstanceTransform {
            pos,
            turn,
            scale: 1.0,
        },
        tint,
        pose_index: 0,
    };
    (
        vec![
            // The depth stack along the camera's sight line (nearest
            // first; the front instance is the tallest).
            npc([-1.0, 0.02, -1.0], 8192, 1.15, [0.90, 0.75, 0.60]),
            npc([-0.4, 0.02, -0.4], 16384, 1.0, [0.20, 0.45, 0.75]),
            npc([0.2, 0.02, 0.2], 0, 0.9, [0.90, 0.35, 0.20]),
            // West sidewalk.
            npc([-5.0, 0.02, -3.0], 0, 1.0, [0.35, 0.75, 0.30]),
            npc([-5.0, 0.02, 0.0], 0, 1.0, [0.75, 0.30, 0.60]),
            npc([-5.0, 0.02, 3.0], 0, 1.0, [0.95, 0.80, 0.40]),
            // South sidewalk.
            npc([-3.0, 0.02, -5.0], 16384, 1.0, [0.15, 0.55, 0.85]),
            npc([1.0, 0.02, -5.0], 16384, 1.0, [0.60, 0.60, 0.65]),
            // East and north sidewalks.
            npc([5.0, 0.02, -1.0], 32768, 1.0, [0.45, 0.80, 0.75]),
            npc([2.0, 0.02, 5.0], 49152, 1.0, [0.70, 0.20, 0.20]),
        ],
        vec![
            vehicle([-10.0, 0.0, 2.0], 16384, [0.75, 0.18, 0.15]),
            vehicle([2.0, 0.0, 9.0], 0, [0.15, 0.25, 0.70]),
            vehicle([9.0, 0.0, -2.0], 16384, [0.85, 0.85, 0.30]),
        ],
    )
}

/// The A/B camera: above the crosswalk corner, looking northeast
/// across the intersection (the depth stack, the west and south
/// sidewalks, and the lane vehicles fall inside the frustum; the
/// camera itself stands in the road cross, clear of the building
/// masses).
fn gate_camera(aspect: f32) -> ([[f32; 4]; 4], [f32; 3]) {
    let matrix = engine_slice::corner_demo::view_projection(
        engine_renderer::lit_scene::RouteWaypoint {
            position: [-3.5, 9.0, -3.5],
            look_at: [3.0, 0.0, 3.0],
        },
        aspect,
    );
    (matrix, [-3.5, 9.0, -3.5])
}

/// One populated capture's population with its OWNED rest-pose
/// blocks (the Era-2 skinning input — every fixture of this gate
/// runs at the rest pose, whose skin matrices are bit-exact identity,
/// so the expected pixels are the authored-silhouette equation; the
/// ANIMATED equivalence gates arrive with the animated-population
/// stage). The blocks are owned alongside the population (the borrow
/// discipline: `npc_poses` must outlive the `ProxyPopulation`).
struct FixturePopulation<'a> {
    population: ProxyPopulation<'a>,
}

impl<'a> FixturePopulation<'a> {
    fn new(
        npc: &'a [ProxyInstance],
        vehicle: &'a [ProxyInstance],
        npc_material: u32,
        vehicle_material: u32,
    ) -> Self {
        // The rest blocks LEAK into the gate's lifetime (test-only,
        // bounded by the fixture count — the same discipline the
        // turn-interpolation keyframes use; the blocks are constant,
        // so the leak is one identical block array per fixture).
        let poses: &'a [[[[f32; 4]; 4]; engine_renderer::animation::HUMANOID_JOINTS]] =
            Vec::leak(vec![
                engine_renderer::animation::rest_pose_block();
                npc.len()
            ]);
        FixturePopulation {
            population: ProxyPopulation {
                npc,
                npc_poses: poses,
                npc_material,
                vehicle,
                vehicle_material,
            },
        }
    }

    fn new_with_poses(
        npc: &'a [ProxyInstance],
        vehicle: &'a [ProxyInstance],
        poses: &'a [[[[f32; 4]; 4]; engine_renderer::animation::HUMANOID_JOINTS]],
        npc_material: u32,
        vehicle_material: u32,
    ) -> Self {
        FixturePopulation {
            population: ProxyPopulation {
                npc,
                npc_poses: poses,
                npc_material,
                vehicle,
                vehicle_material,
            },
        }
    }
}

/// The shorthand the gates pass inline (the temporary lives through
/// the call statement — the fixture's own field borrow).
fn populated<'a>(
    npc: &'a [ProxyInstance],
    vehicle: &'a [ProxyInstance],
    npc_material: u32,
    vehicle_material: u32,
) -> FixturePopulation<'a> {
    FixturePopulation::new(npc, vehicle, npc_material, vehicle_material)
}

fn populated_locomotion<'a>(
    npc: &'a [ProxyInstance],
    vehicle: &'a [ProxyInstance],
    poses: &'a [[[[f32; 4]; 4]; engine_renderer::animation::HUMANOID_JOINTS]],
    npc_material: u32,
    vehicle_material: u32,
) -> FixturePopulation<'a> {
    FixturePopulation::new_with_poses(npc, vehicle, poses, npc_material, vehicle_material)
}

/// One populated capture in the given submission mode.
fn capture_populated(
    renderer: &mut Renderer,
    mode: ProxyDrawMode,
    matrix: &[[f32; 4]; 4],
    eye: [f32; 3],
    light: LightParams,
    population: &ProxyPopulation<'_>,
    size: (u32, u32),
) -> (CapturedFrame, u32) {
    renderer.set_proxy_draw_mode(mode);
    renderer.request_frame_capture().unwrap();
    let frame = renderer
        .render_corner_populated(matrix, eye, light, population, size)
        .unwrap();
    assert!(frame.presented, "the populated frame presented");
    let image = renderer.take_frame_capture().unwrap().unwrap();

    assert_eq!((image.width, image.height), size);
    (image, frame.geometry.proxy_draw_calls)
}

/// The unpopulated corner baseline (the ink denominator).
fn capture_base(
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

/// Full-frame byte comparison with a first-mismatch report (the
/// render_glyph_atlas discipline).
fn assert_frames_equal(expected: &CapturedFrame, actual: &CapturedFrame, what: &str) {
    assert_eq!(expected.format, actual.format, "{what}: format");
    assert_eq!(expected.bytes.len(), actual.bytes.len(), "{what}: size");
    if expected.bytes != actual.bytes {
        for y in 0..expected.height {
            for x in 0..expected.width {
                let a = expected.pixel_rgba(x, y);
                let b = actual.pixel_rgba(x, y);
                assert!(
                    a == b,
                    "{what}: first RGBA mismatch at ({x},{y}): per-object {a:?} vs instanced {b:?}"
                );
            }
        }
        unreachable!("{what}: byte lengths equal but no differing pixel found");
    }
}

/// One single-instance oracle fixture: the proxy, the material it
/// draws with, its +Z wall's local extents (half-width, local y
/// range), and the orthographic camera position.
struct OracleWall {
    instance: ProxyInstance,
    material: usize,
    half_width: f32,
    y_local: [f32; 2],
    eye: [f32; 3],
}

/// The single-instance oracle check (the render_corner `check_patch`
/// discipline applied to the instance transform conventions): an
/// orthographic camera faces ONE proxy's +Z wall dead-on; every
/// interior pixel must match `lit_surface_color(albedo * tint, ...)`
/// within the documented two-UNORM-step shader-FP tolerance, and the
/// wall's world rectangle must land where the pinned matrix math
/// says (a convention error moves it off the invertible rect).
fn check_single_instance_oracle(
    renderer: &mut Renderer,
    size: (u32, u32),
    wall: &OracleWall,
    population: &ProxyPopulation<'_>,
) {
    let world_pos = wall.instance.transform.pos;
    // The orthographic matrix: the wall's world rectangle fills the
    // framebuffer's [-1, 1]² (column-major; NDC.x = (x-wx)/hw,
    // NDC.y = (y-wy)/hh with wy the wall's world-y center).
    let y_center = world_pos[1] + (wall.y_local[0] + wall.y_local[1]) / 2.0;
    let half_height = (wall.y_local[1] - wall.y_local[0]) / 2.0;
    let matrix = [
        [1.0 / wall.half_width, 0.0, 0.0, 0.0],
        [0.0, 1.0 / half_height, 0.0, 0.0],
        [0.0, 0.0, -0.0625, 0.0],
        [
            -world_pos[0] / wall.half_width,
            -y_center / half_height,
            0.25,
            1.0,
        ],
    ];
    renderer.request_frame_capture().unwrap();
    let frame = renderer
        .render_corner_populated(&matrix, wall.eye, day_preset(), population, size)
        .unwrap();
    assert!(frame.presented);
    assert_eq!(
        frame.geometry.proxy_draw_calls, 1,
        "the single instance draws once"
    );
    let image = renderer.take_frame_capture().unwrap().unwrap();
    let mat = materials()[wall.material];
    let texture = procedural_texture(mat.texture);
    let mut largest_error = 0;
    let mut checked = 0;
    for y in 8..size.1 - 8 {
        for x in 8..size.0 - 8 {
            // Screen pixel -> NDC -> the wall's WORLD rectangle.
            let dx = (x as f32 + 0.5) * 2. / size.0 as f32 - 1.;
            let dy = (y as f32 + 0.5) * 2. / size.1 as f32 - 1.;
            let world_x = world_pos[0] + dx * wall.half_width;
            let world_y = y_center - dy * half_height;
            // The mesh's LOCAL coordinates drive the UV (the instance
            // transform never touches uv): local = world - pos.
            let local_x = world_x - world_pos[0];
            let local_y = world_y - world_pos[1];
            let [tx, ty] = [(local_x / 4.), (local_y / 4.)]
                .map(|m| (m.rem_euclid(1.) * TEXTURE_SIZE as f32).floor() as usize);
            let index = 4 * (ty * TEXTURE_SIZE as usize + tx);
            let albedo =
                std::array::from_fn(|c| texture[index + c] as f32 / 255. * wall.instance.tint[c]);
            let pos = [world_x, world_y, world_pos[2]];
            let view = std::array::from_fn(|i| wall.eye[i] - pos[i]);
            let expected = lit_surface_color(albedo, [0., 0., 1.], view, mat, day_preset())
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
                "material {} pixel ({x},{y}) world {pos:?}: expected {expected:?}, \
                 got {pixel:?} (a transform-convention error also moves the pixel's world \
                 position off the wall's rectangle)",
                wall.material
            );
            assert_eq!(pixel[3], 255);
            checked += 1;
        }
    }
    assert!(checked > 0);
    println!(
        "single-instance oracle (material {}): {checked} framebuffer pixels, \
         max RGB error {largest_error}/255",
        wall.material
    );
}

/// FINDING 13's lit-transform oracle: the single-instance known-
/// answer check for a TRANSLATED, ROTATED, non-unit-scaled wall —
/// the camera looks DEAD-ON along the rotated face normal, so the
/// wall's scaled rectangle fills the framebuffer exactly as the
/// axis-aligned fixture's does, and every pixel's expected color is
/// derived independently through the pinned instance-transform
/// convention (proxy_instances::instance_columns: world = pos +
/// yaw·(scale·local), +Z → [s, 0, c], +X → [c, 0, −s]). The local
/// coordinates for the UV come from the INVERSE transform (the
/// instance transform never touches UV — the shader computes it
/// from the local position), the normal is the rotated +Z, and the
/// eye vector is the dead-on camera's. A shader that drops or
/// transposes the normal rotation, or ignores the scale in the
/// world position, moves these pixels off their expected values.
fn check_rotated_instance_oracle(
    renderer: &mut Renderer,
    size: (u32, u32),
    instance: &ProxyInstance,
    material: usize,
    half_width: f32,
    y_local: [f32; 2],
) {
    let t = &instance.transform;
    let (s, c) = engine_renderer::proxy_instances::sin_cos_turn(t.turn);
    // The pinned convention's basis images (unit — sin_cos_turn's
    // Taylor is within 2e-6 of unit; normalize like the shader does).
    let n_len = (s * s + c * c).sqrt();
    let normal = [s / n_len, 0.0, c / n_len]; // local +Z → world
    let tangent = [c / n_len, 0.0, -s / n_len]; // local +X → world
    let k = t.scale;
    // The wall's WORLD rectangle: center and half extents (yaw
    // preserves Y, so the local y range only scales and lifts).
    let y_mid_local = (y_local[0] + y_local[1]) / 2.0;
    let center = [t.pos[0], t.pos[1] + k * y_mid_local, t.pos[2]];
    let hw = half_width * k;
    let hh = (y_local[1] - y_local[0]) / 2.0 * k;
    // The dead-on orthographic camera: right = tangent, up = +Y,
    // looking along −normal from eye = center + 4·normal (the
    // axis-aligned fixture's own 4-unit standoff). THE ENGINE'S
    // PINNED LAYOUT (math.rs's mul/perspective/camera_view —
    // matrix[COLUMN][row], translation in column 3 — and both
    // vertex shaders' `frame.view_proj * world` column-vector
    // product): each column below is the image of one world basis
    // axis, NDC.x = ((P−C)·tangent)/hw, NDC.y = (Py−Cy)/hh, depth =
    // 0.25 − 0.0625·((P−C)·normal) (the wall lands at 0.25, the
    // existing fixture's depth). The axis-aligned fixture's matrix
    // is transpose-invariant — its linear part is diagonal — which
    // is exactly why a ROTATED camera is the convention gate: the
    // first version of this fixture was authored row-form and the
    // wall rendered at the transposed projection's off-center
    // rectangle instead of dead-on.
    let matrix = [
        // Column 0 — world +X's image.
        [tangent[0] / hw, 0.0, -0.0625 * normal[0], 0.0],
        // Column 1 — world +Y's image.
        [0.0, 1.0 / hh, 0.0, 0.0],
        // Column 2 — world +Z's image.
        [tangent[2] / hw, 0.0, -0.0625 * normal[2], 0.0],
        // Column 3 — the translation.
        [
            -(center[0] * tangent[0] + center[2] * tangent[2]) / hw,
            -center[1] / hh,
            0.25 + 0.0625 * (center[0] * normal[0] + center[2] * normal[2]),
            1.0,
        ],
    ];
    let eye = [
        center[0] + 4.0 * normal[0],
        center[1] + 4.0 * normal[1],
        center[2] + 4.0 * normal[2],
    ];
    let single = [*instance];
    renderer.request_frame_capture().unwrap();
    let frame = renderer
        .render_corner_populated(
            &matrix,
            eye,
            day_preset(),
            &populated(&single, &[], 1, 0).population,
            size,
        )
        .unwrap();
    assert!(frame.presented);
    assert_eq!(
        frame.geometry.proxy_draw_calls, 1,
        "the single rotated instance draws once"
    );
    let image = renderer.take_frame_capture().unwrap().unwrap();
    let mat = materials()[material];
    let texture = procedural_texture(mat.texture);
    let mut largest_error = 0u8;
    let mut checked = 0usize;
    for y in 8..size.1 - 8 {
        for x in 8..size.0 - 8 {
            let dx = (x as f32 + 0.5) * 2. / size.0 as f32 - 1.;
            let dy = (y as f32 + 0.5) * 2. / size.1 as f32 - 1.;
            // Screen → world: the dead-on rectangle (pixel-down is
            // world-down, the raster convention).
            let world = [
                center[0] + dx * hw * tangent[0],
                center[1] - dy * hh,
                center[2] + dx * hw * tangent[2],
            ];
            // World → LOCAL (the inverse transform — scale then
            // un-yaw then un-translate; the dot with the basis
            // recovers the scaled local coordinates exactly, since
            // tangent ⊥ normal ⊥ +Y).
            let local_x =
                ((world[0] - t.pos[0]) * tangent[0] + (world[2] - t.pos[2]) * tangent[2]) / k;
            let local_y = (world[1] - t.pos[1]) / k;
            let [tx, ty] = [(local_x / 4.), (local_y / 4.)]
                .map(|m| (m.rem_euclid(1.) * TEXTURE_SIZE as f32).floor() as usize);
            let index = 4 * (ty * TEXTURE_SIZE as usize + tx);
            let albedo =
                std::array::from_fn(|ch| texture[index + ch] as f32 / 255. * instance.tint[ch]);
            let view = std::array::from_fn(|i| eye[i] - world[i]);
            let expected = lit_surface_color(albedo, normal, view, mat, day_preset())
                .map(|ch| (255. * ch.clamp(0., 1.)).round() as u8);
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
                "rotated-instance pixel ({x},{y}) world {world:?}: expected {expected:?}, \
                 got {pixel:?} (a dropped or transposed normal rotation, or a scale ignored \
                 in the world position, moves this pixel off its expected value)"
            );
            assert_eq!(pixel[3], 255);
            checked += 1;
        }
    }
    assert!(checked > 0);
    println!(
        "rotated-instance oracle (material {material}, turn {}): {checked} framebuffer \
         pixels, max RGB error {largest_error}/255 — N·L ≈ {:.3}",
        t.turn,
        normal[0] * day_preset().dir_toward_light[0] + normal[2] * day_preset().dir_toward_light[2]
    );
}

#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup (see render_overflow)"]
fn instanced_draws_match_per_object_pixels_and_pin_counts() {
    let _guard = sdl_lock();
    let mut app = ShellApp::new(ShellConfig {
        title: "Cycle 6-E instanced draws".into(),
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

    let (matrix, eye) = gate_camera(1.0);
    let light = day_preset();
    let (npc, vehicle) = gate_population();
    let total = npc.len() + vehicle.len();
    let population = populated(&npc, &vehicle, 1, 2); // the fixture (owns its rest blocks)

    // THE EQUIVALENCE GATE: N single-instance draws (the pre-6-E
    // per-object submission shape) versus one instanced draw per
    // kind — same meshes, transforms, tints, materials — must
    // produce byte-equal frames.
    let (reference, reference_calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::PerInstance,
        &matrix,
        eye,
        light,
        &population.population,
        size,
    );
    assert_eq!(
        reference_calls, total as u32,
        "the per-object reference submits one draw per instance"
    );
    let (instanced, instanced_calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::Instanced,
        &matrix,
        eye,
        light,
        &population.population,
        size,
    );
    assert_eq!(
        instanced_calls, 2,
        "the instanced path submits one draw per populated kind"
    );
    assert_frames_equal(&reference, &instanced, "N draws vs 1 instanced draw");
    // The instanced capture is itself deterministic (a repeat capture
    // reproduces the identical bytes).
    let (repeat, repeat_calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::Instanced,
        &matrix,
        eye,
        light,
        &population.population,
        size,
    );
    assert_eq!(repeat_calls, 2);
    assert_frames_equal(&instanced, &repeat, "instanced repeat capture");

    // The ink control: the population must actually reach the
    // framebuffer (a vacuous comparison over an empty scene is not a
    // gate — the glyph-atlas discipline).
    let base = capture_base(&mut renderer, &matrix, eye, light, size);
    let mut ink = 0;
    for y in 0..size.1 {
        for x in 0..size.0 {
            ink += usize::from(instanced.pixel_rgba(x, y) != base.pixel_rgba(x, y));
        }
    }
    println!(
        "{total} instances ({}/{} npc/vehicle): per-object draws {reference_calls}, \
         instanced draws {instanced_calls}, {ink} of {} framebuffer bytes changed by the population",
        npc.len(),
        vehicle.len(),
        size.0 * size.1
    );
    assert!(ink > 2_000, "visible proxy coverage: {ink} changed pixels");

    // THE ZERO-PER-FRAME-ALLOCATION GATE: warmed populated frames
    // (both submission modes, both frame slots) hold the allocation
    // counters exactly — the lazy proxy resources and the capture
    // buffer are paid once, never per frame.
    let warm = renderer.allocations();
    for mode in [ProxyDrawMode::Instanced, ProxyDrawMode::PerInstance] {
        for _ in 0..2 {
            let (image, calls) = capture_populated(
                &mut renderer,
                mode,
                &matrix,
                eye,
                light,
                &population.population,
                size,
            );
            assert_eq!(
                calls,
                if matches!(mode, ProxyDrawMode::PerInstance) {
                    total as u32
                } else {
                    2
                }
            );
            assert_frames_equal(&instanced, &image, "warmed mode stability");
            assert_eq!(renderer.allocations(), warm, "warmed allocations constant");
        }
    }
    println!("warmed populated allocations: {warm:?}");

    // The pipeline-replacement lifecycle (the finding-A re-ensure):
    // a forced recreation replaces the pipelines AND the proxy
    // resources; populated frames continue drawing, the corner's
    // static allocations survive, and the warmed counters resume
    // constant at the replacement's measured total. The printed
    // device_memory delta is the measured actual (the replacement's
    // own swapchain/pipeline reallocs plus the re-ensured proxy set —
    // two mesh memories, their staged uploads' staging memories, and
    // the instance memory; cumulative counters never decrement).
    let replacement = renderer
        .render_corner_populated(
            &matrix,
            eye,
            light,
            &population.population,
            (size.0 + 8, size.1 + 8),
        )
        .unwrap();
    assert!(replacement.recreated, "the resize forced a recreation");
    let (after, calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::Instanced,
        &matrix,
        eye,
        light,
        &population.population,
        size,
    );
    assert_eq!(calls, 2);
    let after_stats = renderer.allocations();
    // The Era-2 correction (run 45488497488's measurement): the
    // tuple grows by the skinned family's PINNED per-replacement
    // re-ensure cost (see the constants' derivation above) — exact
    // equality against the derived expectation, never a loosened
    // blanket match.
    assert_eq!(
        (
            after_stats.texture_memory,
            after_stats.descriptor_pools,
            after_stats.descriptor_sets,
            after_stats.samplers
        ),
        (
            warm.texture_memory,
            warm.descriptor_pools + SKINNED_REPLACEMENT_POOLS * LIFECYCLE_REPLACEMENTS,
            warm.descriptor_sets + SKINNED_REPLACEMENT_SETS * LIFECYCLE_REPLACEMENTS,
            warm.samplers
        ),
        "the corner's static allocations plus the skinned family's pinned \
         re-ensure cost survived the replacement"
    );
    println!(
        "after pipeline replacement: {after_stats:?} (device_memory delta {})",
        after_stats.device_memory - warm.device_memory
    );
    for _ in 0..2 {
        let (image, calls) = capture_populated(
            &mut renderer,
            ProxyDrawMode::Instanced,
            &matrix,
            eye,
            light,
            &population.population,
            size,
        );
        assert_eq!(calls, 2);
        assert_frames_equal(&after, &image, "post-replacement stability");
        assert_eq!(renderer.allocations(), after_stats);
    }

    // The independent oracle pins (single instance, orthographic,
    // per-pixel against the 6-B CPU reference with albedo * tint):
    // the v2 humanoid TORSO's +Z wall and the v2 sedan BODY's +Z
    // nose wall (the Era-1 silhouettes' flat faces — the dead-on
    // camera frames each wall's rectangle exactly; the neighboring
    // boxes — arms, head, cabin — sit outside the framed rectangle,
    // the wheels behind the nose).
    let npc_wall = OracleWall {
        instance: ProxyInstance {
            transform: InstanceTransform {
                pos: [0.0, 0.02, 0.0],
                turn: 0,
                scale: 1.0,
            },
            tint: [0.9, 0.7, 0.6],
            pose_index: 0,
        },
        material: 1,
        half_width: 0.21,
        y_local: [1.06, 1.60],
        eye: [0.0, 1.35, 4.0],
    };
    let single_npc = [npc_wall.instance];
    check_single_instance_oracle(
        &mut renderer,
        size,
        &npc_wall,
        &populated(&single_npc, &[], 1, 0).population,
    );
    let vehicle_wall = OracleWall {
        instance: ProxyInstance {
            transform: InstanceTransform {
                pos: [0.0, 0.0, 0.0],
                turn: 0,
                scale: 1.0,
            },
            tint: [0.75, 0.18, 0.15],
            pose_index: 0,
        },
        material: 2,
        half_width: 0.95,
        y_local: [0.35, 0.90],
        eye: [0.0, 0.625, 6.0],
    };
    let single_vehicle = [vehicle_wall.instance];
    check_single_instance_oracle(
        &mut renderer,
        size,
        &vehicle_wall,
        &populated(&[], &single_vehicle, 0, 2).population,
    );

    // The typed refusals reach the caller (nothing rendered, no
    // frame state touched): an over-capacity total and an
    // out-of-table material.
    let over: Vec<_> = vec![single_npc[0]; 4096];
    let refused = renderer
        .render_corner_populated(
            &matrix,
            eye,
            light,
            &populated(&over, &single_vehicle, 1, 2).population,
            size,
        )
        .unwrap_err();
    assert!(matches!(
        refused,
        engine_renderer::RendererError::ProxyPack(_)
    ));
    let bad_material = renderer
        .render_corner_populated(
            &matrix,
            eye,
            light,
            &populated(&single_npc, &[], 3, 0).population,
            size,
        )
        .unwrap_err();
    assert!(bad_material.to_string().contains("material table"));
    // A subsequent healthy frame still presents (no poisoned state).
    let (healthy, calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::Instanced,
        &matrix,
        eye,
        light,
        &population.population,
        size,
    );
    assert_eq!(calls, 2);
    assert_frames_equal(&after, &healthy, "post-refusal stability");

    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "{:?}",
        renderer.stats().recent_validation
    );
    println!(
        "equivalence gate: {total} instances, per-object {reference_calls} draws == instanced \
         {instanced_calls} draws, byte-equal frames, max oracle error within 2/255, \
         zero per-frame allocation across warmed frames"
    );
    // The stride contract the packed bytes rely on (defense in depth
    // for the buffer math this test exercised end to end).
    assert_eq!(INSTANCE_STRIDE, 96);

    // FINDING 13'S LIT-TRANSFORM ORACLE FIXTURE (SHOULD-FIX — the
    // independent instancing oracle measured ambient-only,
    // identity-transform walls: unit scale, zero yaw, +Z normal, and
    // the day light's negative Z clamped N·L to 0, so diffuse and
    // specular vanished and a wrong rotated normal still matched).
    // The fixture: a TRANSLATED, ROTATED (turn 40960 = 225°), 2×-
    // scaled wall under the day light — the rotated normal
    // [−0.707, 0, −0.707] points INTO the light's (−0.5, 0.707,
    // −0.5) direction: N·L ≈ 0.707 (strong diffuse) and the
    // dead-on view's half vector gives N·H ≈ 0.923 (measurable
    // specular). The CPU reference derives the hit, UV, rotated
    // normal, and eye vector independently at the ACTUAL face
    // depth, through the pinned instance-transform convention
    // (proxy_instances::instance_columns: +Z → [s, 0, c], +X →
    // [c, 0, −s], scale then yaw then translate).
    check_rotated_instance_oracle(
        &mut renderer,
        size,
        &ProxyInstance {
            transform: InstanceTransform {
                pos: [2.0, 0.02, -2.0],
                turn: 40_960, // 225° — the normal faces the day light
                scale: 2.0,
            },
            tint: [1.0, 1.0, 1.0],
            pose_index: 0,
        },
        1,            // the building facade material
        0.21,         // the NPC torso's local half width
        [1.06, 1.60], // the torso wall's local y range (humanoid_mesh's own box)
    );

    // THE SELF-DISCRIMINATION CONTROL (the review's bar, proven
    // CPU-side): the same surface lit with the UN-ROTATED +Z normal
    // clamps N·L to zero (the day light's z is −0.5) — ambient-only,
    // the exact failure mode this fixture exists to catch. The
    // expected colors differ by far more than the 2/255 tolerance,
    // so a shader that drops or transposes the normal rotation
    // fails the pixel checks above.
    {
        let (s, c) = engine_renderer::proxy_instances::sin_cos_turn(40_960);
        // +Z under 225° (quadrant 2): sin_cos_turn's (s, c) is
        // already both-negative — the pinned basis image, NOT its
        // negation.
        let rotated = [s, 0.0, c];
        let unrotated = [0.0, 0.0, 1.0];
        let light = day_preset();
        let mat = materials()[1];
        let albedo = [0.5f32; 3];
        let view = [rotated[0] * 4.0, 0.0, rotated[2] * 4.0]; // the dead-on eye vector
        let correct = lit_surface_color(albedo, rotated, view, mat, light);
        let buggy = lit_surface_color(albedo, unrotated, view, mat, light);
        let n_dot_l = (rotated[0] * light.dir_toward_light[0]
            + rotated[2] * light.dir_toward_light[2])
            .max(0.0);
        // lit_surface_color's own convention (the shader's too): the
        // view is NORMALIZED before the half vector — the dead-on
        // standoff's length must not leak into N·H.
        let v = {
            let len = (view[0] * view[0] + view[1] * view[1] + view[2] * view[2]).sqrt();
            [view[0] / len, view[1] / len, view[2] / len]
        };
        let l = light.dir_toward_light;
        let h_len = ((l[0] + v[0]).powi(2) + (l[1] + v[1]).powi(2) + (l[2] + v[2]).powi(2)).sqrt();
        let n_dot_h = ((rotated[0] * (l[0] + v[0]) + rotated[2] * (l[2] + v[2])) / h_len).max(0.0);
        let mut spec = n_dot_h;
        for _ in 0..mat.shininess.trailing_zeros() {
            spec *= spec;
        }
        let delta = correct[0] - buggy[0];
        println!(
            "lit-transform fixture: N·L {n_dot_l:.3}, N·H {n_dot_h:.3}, specular factor              {spec:.3}, un-rotated-normal color delta {delta:.3} (the tolerance is 2/255 ≈ 0.008)"
        );
        assert!(
            n_dot_l > 0.5,
            "the fixture's diffuse is strong, not ambient-only (N·L {n_dot_l:.3})"
        );
        assert!(
            spec > 0.05,
            "the fixture's specular is measurable (factor {spec:.3})"
        );
        assert!(
            delta > 0.05,
            "the fixture discriminates a dropped normal rotation (delta {delta:.3})"
        );
    }
}

/// The ladder package 3 gate (the corner-scene population): the
/// SHIPPED 24-NPC + 2-vehicle placement table
/// (`corner_population`) through BOTH submission modes — byte-equal
/// captured frames (26 per-object draws vs 2 instanced draws), the
/// pinned counts, the ink control against the unpopulated corner,
/// and the warmed-allocation constancy for the full table. The
/// table's own placement validity (sidewalk/lane membership, route
/// clearance, overlap freedom, the packed-bytes FNV pin) is
/// `corner_population`'s unit battery; this gate proves the table
/// THROUGH the real instanced path — the composition the ladder's
/// "through the instanced path" wording demands.
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup (see render_overflow)"]
fn full_corner_population_matches_per_object_pixels() {
    let _guard = sdl_lock();
    let mut app = ShellApp::new(ShellConfig {
        title: "Cycle 6-E corner population".into(),
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
    assert_eq!(size, (256, 256));
    assert!(renderer.stats().validation_active);
    let _ = app.frame();

    // Above the road cross looking down at the intersection center
    // (the gate camera's stand, raised): every placement group —
    // both crosswalks, all four near bands, both lanes — falls
    // inside the frustum, and no building mass stands between.
    let (matrix, eye) = {
        let matrix = engine_slice::corner_demo::view_projection(
            engine_renderer::lit_scene::RouteWaypoint {
                position: [-3.5, 18.0, -3.5],
                look_at: [0.5, 0.0, 0.5],
            },
            1.0,
        );
        (matrix, [-3.5, 18.0, -3.5])
    };
    let light = day_preset();
    // The new locomotion population (the rendering bridge, era2/npc-ai-rendering-bridge).
    let (routes, states, present) = engine_slice::corner_routes::corner_population_locomotion();
    let tints = engine_slice::corner_population::corner_npc_tints();
    let npc = engine_slice::corner_routes::corner_instances(&states, &tints);
    let (_, vehicle) = engine_slice::corner_population::corner_population();
    let total = npc.len() + vehicle.len();
    assert_eq!((npc.len(), vehicle.len(), total), (24, 2, 26));
    // The demo's caller policy: NPCs with the facade set, vehicles
    // with the dark stone set (corner_demo's documented choice).
    // The population holds its own rest blocks.
    let mut poses = Vec::new();
    engine_slice::corner_routes::evaluate_population_poses_locomotion(&states, &mut poses);
    let population = populated_locomotion(&npc, &vehicle, &poses, 1, 2);

    // THE FULL-TABLE EQUIVALENCE: 26 single-instance draws vs one
    // instanced draw per kind — byte-equal frames.
    let (reference, reference_calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::PerInstance,
        &matrix,
        eye,
        light,
        &population.population,
        size,
    );
    assert_eq!(
        reference_calls, 26,
        "the full table: one draw per instance in the reference mode"
    );
    let (instanced, instanced_calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::Instanced,
        &matrix,
        eye,
        light,
        &population.population,
        size,
    );
    assert_eq!(
        instanced_calls, 2,
        "the full table: one draw per populated kind"
    );
    assert_frames_equal(
        &reference,
        &instanced,
        "the full table: 26 draws vs 2 instanced",
    );
    let (repeat, repeat_calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::Instanced,
        &matrix,
        eye,
        light,
        &population.population,
        size,
    );
    assert_eq!(repeat_calls, 2);
    assert_frames_equal(&instanced, &repeat, "full-table instanced repeat capture");

    // The ink control (the glyph-atlas discipline): the population
    // must actually reach the framebuffer.
    let base = capture_base(&mut renderer, &matrix, eye, light, size);
    let mut ink = 0;
    for y in 0..size.1 {
        for x in 0..size.0 {
            ink += usize::from(instanced.pixel_rgba(x, y) != base.pixel_rgba(x, y));
        }
    }
    println!(
        "corner population: {total} instances ({}/{} npc/vehicle): per-object draws \
         {reference_calls}, instanced draws {instanced_calls}, {ink} of {} pixels changed \
         by the population",
        npc.len(),
        vehicle.len(),
        size.0 * size.1
    );
    assert!(
        ink > 1_300,
        "visible full-table coverage: {ink} changed pixels"
    );

    // The zero-per-frame-allocation gate for the full table (both
    // modes, both frame slots).
    let warm = renderer.allocations();
    for mode in [ProxyDrawMode::Instanced, ProxyDrawMode::PerInstance] {
        for _ in 0..2 {
            let (image, calls) = capture_populated(
                &mut renderer,
                mode,
                &matrix,
                eye,
                light,
                &population.population,
                size,
            );
            assert_eq!(
                calls,
                if matches!(mode, ProxyDrawMode::PerInstance) {
                    26
                } else {
                    2
                }
            );
            assert_frames_equal(&instanced, &image, "warmed mode stability");
            assert_eq!(renderer.allocations(), warm, "warmed allocations constant");
        }
    }
    println!("warmed full-table populated allocations: {warm:?}");

    // THE ANIMATED REGIME (the Era-2 stage-5 allocation gate, REBUILT on
    // the rendering bridge's locomotion source — part 4): the crowd's
    // OWN behavior state drives the walk. Every warmed frame runs the
    // DEMO's exact per-frame path — `advance_batch` at dt = 1/60, the
    // instances re-packed in place through `corner_instances_into`, the
    // pose blocks re-evaluated into the retained buffer, then the render
    // — and BOTH allocation counters hold exactly across the warmed
    // window: the renderer's Vulkan objects (`allocations()`) AND the
    // host-allocation counter (finding 10's `HostAllocScope`, wrapping
    // THE STEP AND THE RENDER TOGETHER: the batch step allocates
    // nothing, the instance re-pack is field writes, the pose
    // evaluation reuses its retained buffer, and the renderer's
    // instance/pose repacks reuse theirs — a warmed animated frame
    // allocates nothing on either side). And the crowd GENUINELY
    // MOVES: the packed pose bytes' FNV-1a digest per frame — at least
    // 3 distinct among the measured window — and the locomotion
    // states' (position, phase) pairs never coincide (the old
    // phase-column clause's replacement: the 16 walkers' distinct
    // speed and phase draws).
    {
        let mut animated_states = states.clone();
        let mut animated_instances = npc.clone();
        let mut blocks = Vec::new();
        // The warmup: the first evaluation grows the retained block
        // buffer (24 blocks × 704 bytes) and the first frames pay any
        // first-touch costs — all BEFORE the snapshot.
        for _ in 0..4u64 {
            engine_core::npc_locomotion::advance_batch(
                &routes,
                &mut animated_states,
                &present,
                1.0 / 60.0,
                None,
                0.3,
                1.75,
            );
            engine_slice::corner_routes::corner_instances_into(
                &animated_states,
                &tints,
                &mut animated_instances,
            );
            engine_slice::corner_routes::evaluate_population_poses_locomotion(
                &animated_states,
                &mut blocks,
            );
            let animated = ProxyPopulation {
                npc: &animated_instances,
                npc_poses: &blocks,
                npc_material: 1,
                vehicle: &vehicle,
                vehicle_material: 2,
            };
            assert!(
                renderer
                    .render_corner_populated(&matrix, eye, light, &animated, size)
                    .unwrap()
                    .presented
            );
        }
        let animated_warm = renderer.allocations();
        assert_eq!(
            animated_warm, warm,
            "the animated regime pays nothing the rest regime did not \
             (the same populated path, different pose bytes)"
        );
        let mut digests = std::collections::BTreeSet::new();
        for k in 4..12u64 {
            // THE NEVER-COINCIDE CLAUSE (at the gate's frames): no two
            // states share (position, stride_phase), and the walkers'
            // phases are all distinct (16 distinct draws — the crowd
            // does not march in lockstep).
            let mut phases_seen = std::collections::BTreeSet::new();
            for state in &animated_states {
                if state.speed > 0.0 {
                    phases_seen.insert(state.stride_phase.to_bits());
                }
            }
            assert_eq!(
                phases_seen.len(),
                16,
                "the 16 walkers' stride phases are distinct at frame {k}"
            );
            for (i, a) in animated_states.iter().enumerate() {
                for b in animated_states.iter().skip(i + 1) {
                    assert!(
                        a.position != b.position || a.stride_phase != b.stride_phase,
                        "states {i} and another coincide at frame {k}"
                    );
                }
            }
            let scope = common::HostAllocScope::new();
            engine_core::npc_locomotion::advance_batch(
                &routes,
                &mut animated_states,
                &present,
                1.0 / 60.0,
                None,
                0.3,
                1.75,
            );
            engine_slice::corner_routes::corner_instances_into(
                &animated_states,
                &tints,
                &mut animated_instances,
            );
            engine_slice::corner_routes::evaluate_population_poses_locomotion(
                &animated_states,
                &mut blocks,
            );
            let animated = ProxyPopulation {
                npc: &animated_instances,
                npc_poses: &blocks,
                npc_material: 1,
                vehicle: &vehicle,
                vehicle_material: 2,
            };
            let outcome = renderer
                .render_corner_populated(&matrix, eye, light, &animated, size)
                .unwrap();
            let allocs = scope.count();
            drop(scope);
            assert!(outcome.presented);
            assert_eq!(
                allocs, 0,
                "no host allocations in the warmed animated frame {k} \
                 (the step, the re-pack, the evaluation, and the renderer's \
                 repack all reuse their buffers)"
            );
            assert_eq!(
                renderer.allocations(),
                animated_warm,
                "warmed animated allocations constant at frame {k}"
            );
            // The frame's pose digest (the FNV-1a form the lib
            // battery's byte pins use).
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for block in &blocks {
                let bytes = engine_renderer::animation::pack_pose_bytes(block);
                for &byte in bytes.iter() {
                    hash ^= u64::from(byte);
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
            digests.insert(hash);
        }
        println!(
            "animated regime: {} distinct pose digests across the 8 measured \
             frames (floor 3), renderer allocations {animated_warm:?}, host \
             allocations 0 per frame",
            digests.len()
        );
        assert!(
            digests.len() >= 3,
            "the walk moves the crowd: {} distinct frame digests",
            digests.len()
        );
    }

    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "{:?}",
        renderer.stats().recent_validation
    );
    println!(
        "population gate: {total} instances, per-object {reference_calls} draws == instanced \
         {instanced_calls} draws, byte-equal frames, zero per-frame allocation across \
         warmed frames"
    );
}

/// The Cycle 6-E2 indirect-draw gate (the ladder's stretch package):
/// the shipped 24-NPC + 2-vehicle population submitted THREE ways —
/// N single-instance draws, one direct instanced draw per kind, and
/// one `vkCmdDrawIndirect` per kind with the instance counts read
/// from GPU memory — must produce BYTE-EQUAL captured frames. The
/// indirect submission's draw-call count is pinned (26 instances
/// stay 2 draws: the GPU-side count changes where N lives, never
/// the submission count), the lazy indirect command buffer's
/// one-time +1 device_memory allocation is pinned (the direct
/// path's counters untouched — the 6-E gates' pins hold exactly),
/// warmed indirect frames hold the counters constant (zero
/// per-frame allocation), the pipeline-replacement lifecycle
/// re-ensures the command buffer (the finding-A discipline), and a
/// single-instance oracle wall runs THROUGH the indirect submission
/// (the CPU-reference pin, closing the "both submissions identically
/// wrong" hole independently of the 6-E gates).
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup (see render_overflow)"]
fn indirect_draws_match_direct_and_per_object_pixels() {
    let _guard = sdl_lock();
    let mut app = ShellApp::new(ShellConfig {
        title: "Cycle 6-E2 indirect draws".into(),
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

    // The population gate's stand: every placement group inside the
    // frustum, no building mass between.
    let (matrix, eye) = {
        let matrix = engine_slice::corner_demo::view_projection(
            engine_renderer::lit_scene::RouteWaypoint {
                position: [-3.5, 18.0, -3.5],
                look_at: [0.5, 0.0, 0.5],
            },
            1.0,
        );
        (matrix, [-3.5, 18.0, -3.5])
    };
    let light = day_preset();
    // The locomotion population at frame 0 (the rendering bridge's
    // initial placements — the same source the full-table gate pins).
    let (_, states_i, _) = engine_slice::corner_routes::corner_population_locomotion();
    let tints_i = engine_slice::corner_population::corner_npc_tints();
    let npc = engine_slice::corner_routes::corner_instances(&states_i, &tints_i);
    let (_, vehicle) = engine_slice::corner_population::corner_population();
    let total = npc.len() + vehicle.len();
    assert_eq!((npc.len(), vehicle.len(), total), (24, 2, 26));
    let mut poses_i = Vec::new();
    engine_slice::corner_routes::evaluate_population_poses_locomotion(&states_i, &mut poses_i);
    let population = populated_locomotion(&npc, &vehicle, &poses_i, 1, 2);

    // THE THREE-WAY EQUIVALENCE: per-object (26 draws) vs direct
    // instanced (2) vs indirect (2) — byte-equal frames.
    let (reference, reference_calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::PerInstance,
        &matrix,
        eye,
        light,
        &population.population,
        size,
    );
    assert_eq!(reference_calls, 26, "the reference: one draw per instance");
    let direct_warm = renderer.allocations();
    let (instanced, instanced_calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::Instanced,
        &matrix,
        eye,
        light,
        &population.population,
        size,
    );
    assert_eq!(instanced_calls, 2, "the direct path: one draw per kind");
    assert_frames_equal(&reference, &instanced, "26 draws vs 2 direct instanced");
    // The direct path's warmed counters are the 6-E pins, untouched
    // by the indirect mechanism (the lazy discipline: nothing
    // indirect exists yet).
    assert_eq!(
        renderer.allocations(),
        direct_warm,
        "the direct path's counters are the 6-E pins exactly"
    );
    let (indirect, indirect_calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::Indirect,
        &matrix,
        eye,
        light,
        &population.population,
        size,
    );
    assert_eq!(
        indirect_calls, 2,
        "the indirect path: still one draw per populated kind"
    );
    assert_frames_equal(&instanced, &indirect, "2 direct draws vs 2 indirect draws");
    assert_frames_equal(&reference, &indirect, "26 draws vs 2 indirect draws");
    // The indirect capture is itself deterministic.
    let (repeat, repeat_calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::Indirect,
        &matrix,
        eye,
        light,
        &population.population,
        size,
    );
    assert_eq!(repeat_calls, 2);
    assert_frames_equal(&indirect, &repeat, "indirect repeat capture");

    // THE LAZY INDIRECT ALLOCATION PIN: the first indirect frame
    // paid EXACTLY ONE device_memory allocation (the persistently
    // mapped command buffer — 32 bytes per frame slot); every
    // counter else is the direct path's. This is the measured
    // mechanism cost, pinned.
    let indirect_warm = renderer.allocations();
    assert_eq!(
        indirect_warm.device_memory,
        direct_warm.device_memory + 1,
        "the indirect command buffer: exactly one new device memory"
    );
    assert_eq!(indirect_warm.descriptor_pools, direct_warm.descriptor_pools);
    assert_eq!(indirect_warm.descriptor_sets, direct_warm.descriptor_sets);
    assert_eq!(indirect_warm.texture_memory, direct_warm.texture_memory);
    assert_eq!(indirect_warm.samplers, direct_warm.samplers);
    println!(
        "indirect mechanism cost: device_memory {} -> {} (the command buffer's one \
         host-visible memory; every other counter unchanged)",
        direct_warm.device_memory, indirect_warm.device_memory
    );

    // The ink control (the glyph-atlas discipline): the population
    // reaches the framebuffer through the indirect path too.
    let base = capture_base(&mut renderer, &matrix, eye, light, size);
    let mut ink = 0;
    for y in 0..size.1 {
        for x in 0..size.0 {
            ink += usize::from(indirect.pixel_rgba(x, y) != base.pixel_rgba(x, y));
        }
    }
    println!(
        "corner population through the indirect path: {total} instances, 2 indirect \
         draws, {ink} of {} pixels changed by the population",
        size.0 * size.1
    );
    assert!(
        ink > 1_300,
        "visible indirect-path coverage: {ink} changed pixels"
    );

    // THE ZERO-PER-FRAME-ALLOCATION GATE (all three modes, both
    // frame slots): the command bytes are repacked into the same
    // mapped slot every frame — no per-frame allocation anywhere.
    // The host allocation counter (finding 10, the missing gate):
    // wrap the measurement to exclude capture extraction, take calls,
    // and test assertions (request the capture BEFORE entering the
    // scope or AFTER — the review's explicit instruction).
    for mode in [
        ProxyDrawMode::Indirect,
        ProxyDrawMode::Instanced,
        ProxyDrawMode::PerInstance,
    ] {
        for _ in 0..2 {
            let (image, calls) = capture_populated(
                &mut renderer,
                mode,
                &matrix,
                eye,
                light,
                &population.population,
                size,
            );
            assert_eq!(
                calls,
                match mode {
                    ProxyDrawMode::PerInstance => 26,
                    ProxyDrawMode::Instanced | ProxyDrawMode::Indirect => 2,
                }
            );
            assert_frames_equal(&indirect, &image, "warmed mode stability");
            assert_eq!(
                renderer.allocations(),
                indirect_warm,
                "warmed allocations constant"
            );
        }
    }

    // THE HOST-ALLOCATION GATE (finding 10 — the counter the review
    // says was missing beside renderer.allocations()): one warmed
    // render per submission mode, measured between scope entry and
    // count — zero heap allocations. Capture extraction, assertions,
    // and startup stay OUTSIDE the scope (the review's explicit
    // instruction); the measured call is the render alone.
    for mode in [
        ProxyDrawMode::Indirect,
        ProxyDrawMode::Instanced,
        ProxyDrawMode::PerInstance,
    ] {
        renderer.set_proxy_draw_mode(mode);
        let scope = common::HostAllocScope::new();
        let outcome = renderer
            .render_corner_populated(&matrix, eye, light, &population.population, size)
            .unwrap();
        let allocs = scope.count();
        drop(scope);
        assert!(outcome.presented);
        println!("host allocations in warmed {mode:?} populated frame: {allocs} (expected 0)");
        assert_eq!(
            allocs, 0,
            "no host allocations in the warmed {mode:?} populated render path"
        );
    }

    println!("warmed indirect-path populated allocations: {indirect_warm:?}");

    // The pipeline-replacement lifecycle (the finding-A re-ensure):
    // a forced recreation replaces the pipelines AND the proxy
    // resources — including the indirect command buffer, whose
    // draws the replacement records. Populated indirect frames
    // continue drawing byte-identically; the corner's static
    // allocations survive; the counters resume constant.
    renderer.set_proxy_draw_mode(ProxyDrawMode::Indirect);
    let replacement = renderer
        .render_corner_populated(
            &matrix,
            eye,
            light,
            &population.population,
            (size.0 + 8, size.1 + 8),
        )
        .unwrap();
    assert!(replacement.recreated, "the resize forced a recreation");
    let (after, calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::Indirect,
        &matrix,
        eye,
        light,
        &population.population,
        size,
    );
    assert_eq!(calls, 2);
    assert_frames_equal(&indirect, &after, "the re-ensured indirect path");
    let after_stats = renderer.allocations();
    // The Era-2 correction (run 45488497488's measurement): the
    // tuple grows by the skinned family's PINNED per-replacement
    // re-ensure cost (the constants' derivation at the top of this
    // file) — exact equality against the derived expectation, never
    // a loosened blanket match.
    assert_eq!(
        (
            after_stats.texture_memory,
            after_stats.descriptor_pools,
            after_stats.descriptor_sets,
            after_stats.samplers
        ),
        (
            indirect_warm.texture_memory,
            indirect_warm.descriptor_pools + SKINNED_REPLACEMENT_POOLS * LIFECYCLE_REPLACEMENTS,
            indirect_warm.descriptor_sets + SKINNED_REPLACEMENT_SETS * LIFECYCLE_REPLACEMENTS,
            indirect_warm.samplers
        ),
        "the corner's static allocations plus the skinned family's pinned \
         re-ensure cost survived the replacement"
    );
    println!(
        "after pipeline replacement: {after_stats:?} (device_memory delta {})",
        after_stats.device_memory - indirect_warm.device_memory
    );
    for _ in 0..2 {
        let (image, calls) = capture_populated(
            &mut renderer,
            ProxyDrawMode::Indirect,
            &matrix,
            eye,
            light,
            &population.population,
            size,
        );
        assert_eq!(calls, 2);
        assert_frames_equal(&after, &image, "post-replacement stability");
        assert_eq!(renderer.allocations(), after_stats);
    }

    // THE INDEPENDENT ORACLE PIN THROUGH THE INDIRECT PATH: one NPC,
    // orthographic camera dead-on its torso +Z wall, per-pixel
    // against the 6-B CPU reference — the indirect submission
    // checked against the math, not just against the other
    // submissions (a submission bug shared by all three modes
    // cannot pass this). (The population-v2 re-pin's completion:
    // this wall still carried the v1 rectangle 0.24 × [0.85, 1.45]
    // — its frame includes the dead zone below the v2 torso's
    // bottom edge (the pelvis is narrower than the torso, so local
    // |x| ∈ (0.19, 0.21] × y < 1.06 renders nothing); found on the
    // gate's first real execution, same as the first wall's re-pin
    // in the N-vs-1 test — the v2 torso rectangle, verbatim.)
    let npc_wall = OracleWall {
        instance: ProxyInstance {
            transform: InstanceTransform {
                pos: [0.0, 0.02, 0.0],
                turn: 0,
                scale: 1.0,
            },
            tint: [0.9, 0.7, 0.6],
            pose_index: 0,
        },
        material: 1,
        half_width: 0.21,
        y_local: [1.06, 1.60],
        eye: [0.0, 1.35, 4.0],
    };
    let single_npc = [npc_wall.instance];
    renderer.set_proxy_draw_mode(ProxyDrawMode::Indirect);
    check_single_instance_oracle(
        &mut renderer,
        size,
        &npc_wall,
        &populated(&single_npc, &[], 1, 0).population,
    );

    // The typed refusals reach the caller through the indirect path
    // too (nothing rendered, no frame state touched): 4097 NPCs is
    // one past MAX_INSTANCES (the TOTAL-population bound).
    let over: Vec<_> = vec![single_npc[0]; 4097];
    let refused = renderer
        .render_corner_populated(
            &matrix,
            eye,
            light,
            &populated(&over, &[], 1, 0).population,
            size,
        )
        .unwrap_err();
    assert!(matches!(
        refused,
        engine_renderer::RendererError::ProxyPack(_)
    ));
    // A subsequent healthy indirect frame still presents, byte-equal
    // (no poisoned state).
    renderer.set_proxy_draw_mode(ProxyDrawMode::Indirect);
    let (healthy, calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::Indirect,
        &matrix,
        eye,
        light,
        &population.population,
        size,
    );
    assert_eq!(calls, 2);
    assert_frames_equal(&after, &healthy, "post-refusal stability");

    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "{:?}",
        renderer.stats().recent_validation
    );
    // The command-block contract the submission relies on (defense
    // in depth for the buffer math this test exercised end to end —
    // the pure core's own unit battery pins the same numbers).
    // (The population-v2 re-pin's completion: the v2 silhouette
    // vertex counts — 396/552, the values the core's own battery
    // was re-pinned to; this copy still carried the v1 108/72 when
    // the gate first executed.)
    // (corrective finding 10: the block is packed through
    // pack_indirect_block with explicit counts — the legacy wrapper
    // that reconstructed both meshes per call is gone.)
    let commands = engine_renderer::proxy_indirect::pack_indirect_block(396, 24, 552, 2);
    let word = |at: usize| u32::from_le_bytes(commands[at..at + 4].try_into().unwrap());
    assert_eq!(engine_renderer::proxy_indirect::INDIRECT_SLOT_BYTES, 32);
    assert_eq!((word(0), word(4), word(16), word(20)), (396, 24, 552, 2));

    // THE ISSUANCE OBSERVATION (the Era-1 review's gate-by-gate
    // disposition for this gate: "one command trace or recording
    // assertion that verifies vkCmdDrawIndirect was issued"). The
    // draw-call count CANNOT verify it — the indirect and direct
    // instanced submissions both count one per populated kind by
    // design, and the release fallback silently submits the
    // identical geometry through cmd_draw when the indirect state
    // is somehow missing. geometry.indirect_draws_issued counts the
    // actual vkCmdDrawIndirect calls at their site: exactly the
    // populated kinds in the indirect mode, ZERO in every other.
    for (mode, expected) in [
        (ProxyDrawMode::Indirect, 2u32),
        (ProxyDrawMode::Instanced, 0u32),
        (ProxyDrawMode::PerInstance, 0u32),
    ] {
        renderer.set_proxy_draw_mode(mode);
        renderer.request_frame_capture().unwrap();
        let frame = renderer
            .render_corner_populated(&matrix, eye, light, &population.population, size)
            .unwrap();
        assert!(frame.presented);
        assert_eq!(
            frame.geometry.indirect_draws_issued, expected,
            "vkCmdDrawIndirect issued exactly {expected} times in {mode:?} — the indirect \
             path's per-kind issuances (a zero here means the submission fell back to the \
             direct draw or the mode never reached the recorder)"
        );
        assert_eq!(
            frame.geometry.proxy_draw_calls,
            match mode {
                ProxyDrawMode::PerInstance => 26,
                ProxyDrawMode::Instanced | ProxyDrawMode::Indirect => 2,
            }
        );
        let _ = renderer.take_frame_capture().unwrap().unwrap();
    }
    println!(
        "indirect gate: {total} instances, per-object 26 == direct 2 == indirect 2 draws, \
         byte-equal frames, oracle wall within 2/255 through the indirect submission, \
         one-time command-buffer allocation, zero per-frame allocation across warmed frames"
    );
}

/// FINDING 11'S OBLIQUE WHEEL FIXTURE — the review's check: "Include
/// an oblique view that exposes the outside tire and both axle
/// orientations." The winding fix (commit 13bd6a0) made every wheel
/// triangle CCW from the outside; the geometric proof is
/// `wheel_normals_agree_with_winding` (populated_meshes' own unit
/// battery). THIS fixture is the pixel half: an oblique dead-on view
/// of the tread's outside band, plus dead-on views of BOTH cap faces
/// (the ±X axle orientations), each probed against the CPU oracle at
/// the actual surface.
///
/// Why it discriminates the historic bug without a full-scene
/// raster: the historic scaffold wound the side quads so their
/// geometric normals pointed INWARD — an outside view culled the
/// NEAR surface and drew the FAR one — and gave both caps one rim
/// traversal, so one cap's geometric normal opposed its authored
/// ±X normal (that cap culled from its own outside). Under
/// `day_preset` (dir_toward_light has +0.707 Y, −0.5 X) the near and
/// far surfaces of the probed regions sit on OPPOSITE sides of the
/// light: the tread's top-outside band has N·L ≈ +0.52 while its
/// antipodal (bottom) quad clamps to 0; the −X cap has N·L = +0.5
/// while the +X orientation clamps to 0. A regression to the
/// historic winding would put the FAR surface's color (or whatever
/// is behind a culled cap) at these probes and fail the equality
/// below — the self-discrimination controls assert exactly that
/// separation, so the fixture can never silently degrade into an
/// ambient-only pass.
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup (see render_overflow)"]
fn oblique_wheel_views_expose_the_outside_tire_and_both_cap_orientations() {
    let _guard = sdl_lock();

    // --- the sedan's wheel geometry, derived through the mesh's own
    // primitives (sin_cos_turn — the same libm-free circle wheel()
    // uses), then PINNED against sedan_mesh()'s actual vertices: the
    // derivation must match the mesh exactly, never trust its own
    // arithmetic.
    const R: f32 = 0.35;
    const W: f32 = 0.25;
    let wheel = [0.85, 0.35, 1.45]; // the +X/+Z wheel (local = world: the identity instance below)
    let x_lo = wheel[0] - W / 2.0;
    let x_hi = wheel[0] + W / 2.0;
    let turn_of = |i: u16| (u32::from(i) * 65_536 / u32::from(WHEEL_SEGMENTS)) as u16;
    // One ring point's (y, z) offsets and radial normal, exactly as
    // wheel() emits them: ring_point = [R·c, R·s], normal = [0, c, s].
    let ring = |i: u16| -> ([f32; 2], [f32; 2]) {
        let (s, c) = engine_renderer::proxy_instances::sin_cos_turn(turn_of(i));
        ([R * c, R * s], [c, s])
    };
    let ([y0, z0], [ny0, nz0]) = ring(0); // the tread segment at the top rim
    let ([y1, z1], [ny1, nz1]) = ring(1);
    let ([_, _], [nyf0, nzf0]) = ring(5); // the ANTIPODAL segment (the historic far surface)
    let ([_, _], [nyf1, nzf1]) = ring(6);

    // The derivation pin: every distinct corner of the tread segment
    // (the mesh emits a, d, c, b — these four) and both caps' center
    // vertices exist in sedan_mesh() with exactly these positions
    // and normals.
    let mesh = sedan_mesh();
    let in_mesh = |pos: [f32; 3], normal: [f32; 3], what: &str| {
        assert!(
            mesh.iter().any(|v| v.pos == pos && v.normal == normal),
            "the derived {what} (pos {pos:?}, normal {normal:?}) is not in sedan_mesh() — \
             the derivation is wrong, never the mesh"
        );
    };
    in_mesh(
        [x_lo, wheel[1] + y0, wheel[2] + z0],
        [0.0, ny0, nz0],
        "tread corner a",
    );
    in_mesh(
        [x_lo, wheel[1] + y1, wheel[2] + z1],
        [0.0, ny1, nz1],
        "tread corner d",
    );
    in_mesh(
        [x_hi, wheel[1] + y1, wheel[2] + z1],
        [0.0, ny1, nz1],
        "tread corner c",
    );
    in_mesh(
        [x_hi, wheel[1] + y0, wheel[2] + z0],
        [0.0, ny0, nz0],
        "tread corner b",
    );
    // The caps' authored ±X normals — the fixture's premise (the −X
    // cap declares −X; the historic shared traversal broke exactly this).
    in_mesh([x_hi, wheel[1], wheel[2]], [1.0, 0.0, 0.0], "+X cap center");
    let minus_wheel = [-0.85, 0.35, 1.45]; // the −X-side wheel
    in_mesh(
        [minus_wheel[0] - W / 2.0, minus_wheel[1], minus_wheel[2]],
        [-1.0, 0.0, 0.0],
        "−X cap center",
    );

    // --- the renderer: one sedan at the identity pose (turn 0, scale
    // 1, origin at the intersection center — local space IS world
    // space for the derivation), three dead-on captures.
    let mut app = ShellApp::new(ShellConfig {
        title: "finding-11 oblique wheel".into(),
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
    assert_eq!(size, (256, 256));
    assert!(renderer.stats().validation_active);
    let _ = app.frame();

    let sedan = [ProxyInstance {
        transform: InstanceTransform {
            pos: [0.0, 0.0, 0.0],
            turn: 0,
            scale: 1.0,
        },
        tint: [1.0, 1.0, 1.0],
        pose_index: 0,
    }];
    let population = populated(&[], &sedan, 1, 0);
    let light = day_preset();
    let mat = materials()[0]; // the population's vehicle material (asphalt: tires sit on it)
    let texture = procedural_texture(mat.texture);
    let texels = TEXTURE_SIZE as usize;
    let albedo_at = |uv: [f32; 2]| -> [f32; 3] {
        let [tx, ty] = [
            (uv[0] / 4.0).rem_euclid(1.0) * TEXTURE_SIZE as f32,
            (uv[1] / 4.0).rem_euclid(1.0) * TEXTURE_SIZE as f32,
        ]
        .map(|m| m.floor() as usize);
        let index = 4 * (ty * texels + tx);
        std::array::from_fn(|ch| texture[index + ch] as f32 / 255.0)
    };
    let quantized =
        |rgb: [f32; 3]| -> [u8; 3] { rgb.map(|ch| (255.0 * ch.clamp(0.0, 1.0)).round() as u8) };

    /// The dead-on orthographic camera (the finding-13 fixture's
    /// pinned layout: matrix[COLUMN][row]; NDC.x = (P−C)·R̂/hw,
    /// NDC.y = (P−C)·Û/hh, depth = 0.25 − 0.0625·((P−C)·N̂); the eye
    /// 4 units out along N̂). `right` and `up` name the screen axes.
    fn dead_on(
        center: [f32; 3],
        normal: [f32; 3],
        right: [f32; 3],
        up: [f32; 3],
        hw: f32,
        hh: f32,
    ) -> ([[f32; 4]; 4], [f32; 3]) {
        let len = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        let n = std::array::from_fn(|i| normal[i] / len(normal));
        let r = std::array::from_fn(|i| right[i] / len(right));
        let u = std::array::from_fn(|i| up[i] / len(up));
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let matrix = [
            [r[0] / hw, u[0] / hh, -0.0625 * n[0], 0.0],
            [r[1] / hw, u[1] / hh, -0.0625 * n[1], 0.0],
            [r[2] / hw, u[2] / hh, -0.0625 * n[2], 0.0],
            [
                -dot(center, r) / hw,
                -dot(center, u) / hh,
                0.25 + 0.0625 * dot(center, n),
                1.0,
            ],
        ];
        let eye = std::array::from_fn(|i| center[i] + 4.0 * n[i]);
        (matrix, eye)
    }
    let dot3 = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let len3 = |v: [f32; 3]| (dot3(v, v)).sqrt();

    // ================= PROBE 1: the +X cap (one axle orientation).
    // Dead-on along +X from outside the sedan; the cap's authored
    // normal is +X (N·L = −0.5 → the diffuse clamps to 0, the
    // ambient floor). N·L = +0.5 is the probe's own control.
    let cap_c = [x_hi, wheel[1], wheel[2]];
    // The screen basis must be RIGHT-handed (right x up = the view
    // normal, toward the viewer — the finding-13 fixture's
    // discipline): a left-handed basis MIRRORS the scene and flips
    // every triangle's apparent winding, culling the very faces the
    // probe targets (this fixture's first version had right = +Z
    // here, Z x Y = -X away from the viewer, and the +X cap
    // disappeared behind the mirrored winding). For the +X view:
    // right = -Z.
    let (matrix, eye) = dead_on(
        cap_c,
        [1.0, 0.0, 0.0],
        [0.0, 0.0, -1.0],
        [0.0, 1.0, 0.0],
        0.36,
        0.36,
    );
    renderer.request_frame_capture().unwrap();
    let frame = renderer
        .render_corner_populated(&matrix, eye, light, &population.population, size)
        .unwrap();
    assert!(frame.presented && frame.geometry.proxy_draw_calls == 1);
    let image = renderer.take_frame_capture().unwrap().unwrap();
    let mut stats = (0usize, 0u8, u8::MAX);
    for y in 0..size.1 {
        for x in 0..size.0 {
            let fx = (x as f32 + 0.5) * 2.0 / size.0 as f32 - 1.0;
            let fy = 1.0 - (y as f32 + 0.5) * 2.0 / size.1 as f32;
            let off = [fx * 0.36, fy * 0.36];
            if (off[0] * off[0] + off[1] * off[1]).sqrt() > 0.9 * R {
                continue; // inside the cap disc only (margin off the rim)
            }
            let world = [cap_c[0], cap_c[1] + off[1], cap_c[2] - off[0]];
            // The cap's uv: [ring z offset, ring y offset] over 4.
            let albedo = albedo_at([world[2] - wheel[2], world[1] - wheel[1]]);
            let view = std::array::from_fn(|i| eye[i] - world[i]);
            let expected = quantized(lit_surface_color(albedo, [1.0, 0.0, 0.0], view, mat, light));
            let control = quantized(lit_surface_color(
                albedo,
                [-1.0, 0.0, 0.0],
                view,
                mat,
                light,
            ));
            let pixel = image.pixel_rgba(x, y);
            let error = expected
                .iter()
                .zip(pixel)
                .map(|(e, a)| e.abs_diff(a))
                .max()
                .unwrap();
            assert!(
                error <= 2,
                "+X cap pixel ({x},{y}) world {world:?}: expected {expected:?}, got {pixel:?}"
            );
            assert_eq!(pixel[3], 255);
            let delta = expected
                .iter()
                .zip(control)
                .map(|(e, c)| e.abs_diff(c))
                .max()
                .unwrap();
            assert!(
                delta > 2,
                "+X cap pixel ({x},{y}): the −X-orientation control {control:?} is within \
                 tolerance of the +X prediction {expected:?} — the fixture no longer \
                 discriminates the cap orientation"
            );
            stats.0 += 1;
            stats.1 = stats.1.max(error);
            stats.2 = stats.2.min(delta);
        }
    }
    assert!(stats.0 > 0, "the +X cap probe selected pixels");
    println!(
        "+X cap (one axle orientation): {} probe pixels, max error {}/255, min orientation-control \
         delta {}/255",
        stats.0, stats.1, stats.2
    );

    // ================= PROBE 2: the −X cap (the other axle
    // orientation — the historic bug's broken cap). Dead-on along −X
    // at the −X-side wheel; the authored −X normal has N·L = +0.5
    // (lit) — under the historic shared traversal this cap's
    // geometric normal was +X and the cap CULLED from this side.
    let cap_c = [minus_wheel[0] - W / 2.0, minus_wheel[1], minus_wheel[2]];
    let (matrix, eye) = dead_on(
        cap_c,
        [-1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 1.0, 0.0],
        0.36,
        0.36,
    );
    renderer.request_frame_capture().unwrap();
    let frame = renderer
        .render_corner_populated(&matrix, eye, light, &population.population, size)
        .unwrap();
    assert!(frame.presented && frame.geometry.proxy_draw_calls == 1);
    let image = renderer.take_frame_capture().unwrap().unwrap();
    let mut stats = (0usize, 0u8, u8::MAX);
    for y in 0..size.1 {
        for x in 0..size.0 {
            let fx = (x as f32 + 0.5) * 2.0 / size.0 as f32 - 1.0;
            let fy = 1.0 - (y as f32 + 0.5) * 2.0 / size.1 as f32;
            let off = [fx * 0.36, fy * 0.36];
            if (off[0] * off[0] + off[1] * off[1]).sqrt() > 0.9 * R {
                continue;
            }
            let world = [cap_c[0], cap_c[1] + off[1], cap_c[2] + off[0]];
            let albedo = albedo_at([world[2] - minus_wheel[2], world[1] - minus_wheel[1]]);
            let view = std::array::from_fn(|i| eye[i] - world[i]);
            let expected = quantized(lit_surface_color(
                albedo,
                [-1.0, 0.0, 0.0],
                view,
                mat,
                light,
            ));
            let control = quantized(lit_surface_color(albedo, [1.0, 0.0, 0.0], view, mat, light));
            let pixel = image.pixel_rgba(x, y);
            let error = expected
                .iter()
                .zip(pixel)
                .map(|(e, a)| e.abs_diff(a))
                .max()
                .unwrap();
            assert!(
                error <= 2,
                "−X cap pixel ({x},{y}) world {world:?}: expected {expected:?}, got {pixel:?} \
                 (the historic shared-traversal bug culled this cap from its own outside — \
                 the pixel would be the body or scene behind it, not this lit face)"
            );
            assert_eq!(pixel[3], 255);
            let delta = expected
                .iter()
                .zip(control)
                .map(|(e, c)| e.abs_diff(c))
                .max()
                .unwrap();
            assert!(
                delta > 2,
                "−X cap pixel ({x},{y}): control {control:?} ≈ prediction {expected:?}"
            );
            stats.0 += 1;
            stats.1 = stats.1.max(error);
            stats.2 = stats.2.min(delta);
        }
    }
    assert!(stats.0 > 0, "the −X cap probe selected pixels");
    println!(
        "−X cap (the historic bug's broken face, now lit at N·L = +0.5): {} probe pixels, max \
         error {}/255, min orientation-control delta {}/255",
        stats.0, stats.1, stats.2
    );

    // ================= PROBE 3: the tread's OUTSIDE band, oblique.
    // Dead-on along the top segment's mid radial (18° off vertical —
    // the oblique view), probing the OUTER strip of the tread that
    // clears the body (x ∈ [0.955, 0.970]; the body's +X face is at
    // 0.95). The near surface's interpolated radial normals have
    // N·L ≈ +0.52; the antipodal (bottom) segment — the FAR surface
    // the historic inward winding drew instead — clamps to 0.
    let chord = [y0 - y1, z0 - z1]; // from the θ1 edge toward the θ0 (top) edge
    let chord_len = len3([0.0, chord[0], chord[1]]);
    let quad_c = [
        wheel[0],
        wheel[1] + (y0 + y1) / 2.0,
        wheel[2] + (z0 + z1) / 2.0,
    ];
    let radial_mid = [0.0, (ny0 + ny1) / 2.0, (nz0 + nz1) / 2.0]; // ≈ radial(18°)
    let up_hat = [0.0, chord[0] / chord_len, chord[1] / chord_len]; // toward the θ0 edge
    let (matrix, eye) = dead_on(
        quad_c,
        radial_mid,
        [1.0, 0.0, 0.0],
        up_hat,
        W / 2.0,
        chord_len / 2.0,
    );
    renderer.request_frame_capture().unwrap();
    let frame = renderer
        .render_corner_populated(&matrix, eye, light, &population.population, size)
        .unwrap();
    assert!(frame.presented && frame.geometry.proxy_draw_calls == 1);
    let image = renderer.take_frame_capture().unwrap().unwrap();
    let mut stats = (0usize, 0u8, u8::MAX);
    for y in 0..size.1 {
        for x in 0..size.0 {
            let fx = (x as f32 + 0.5) * 2.0 / size.0 as f32 - 1.0;
            let fy = 1.0 - (y as f32 + 0.5) * 2.0 / size.1 as f32;
            // The world position on the quad's plane (dead-on inverse).
            let world = [
                quad_c[0] + fx * (W / 2.0),
                quad_c[1] + fy * (chord_len / 2.0) * up_hat[1],
                quad_c[2] + fy * (chord_len / 2.0) * up_hat[2],
            ];
            // Only the OUTER strip (clear of the body's occlusion)
            // and the interior of the segment (off both rim edges).
            if !(0.955..0.970).contains(&world[0]) || fy.abs() > 0.9 {
                continue;
            }
            // The interpolated radial normal: fy = +1 is the θ0 edge
            // (n0), fy = −1 the θ1 edge (n1) — the same linear
            // interpolation the rasterizer performs on the flat quad.
            let f = (1.0 - fy) / 2.0;
            let near_normal = [0.0, ny0 * (1.0 - f) + ny1 * f, nz0 * (1.0 - f) + nz1 * f];
            let far_normal = [
                0.0,
                nyf0 * (1.0 - f) + nyf1 * f,
                nzf0 * (1.0 - f) + nzf1 * f,
            ];
            // The side quad's uv: [ring z offset, local x] over 4.
            let albedo = albedo_at([world[2] - wheel[2], world[0]]);
            let view = std::array::from_fn(|i| eye[i] - world[i]);
            let expected = quantized(lit_surface_color(albedo, near_normal, view, mat, light));
            let control = quantized(lit_surface_color(albedo, far_normal, view, mat, light));
            let pixel = image.pixel_rgba(x, y);
            let error = expected
                .iter()
                .zip(pixel)
                .map(|(e, a)| e.abs_diff(a))
                .max()
                .unwrap();
            assert!(
                error <= 2,
                "tread pixel ({x},{y}) world {world:?}: expected {expected:?}, got {pixel:?} \
                 (the historic inward winding drew the FAR surface here — its color is the \
                 control below, beyond tolerance from this expectation)"
            );
            assert_eq!(pixel[3], 255);
            let delta = expected
                .iter()
                .zip(control)
                .map(|(e, c)| e.abs_diff(c))
                .max()
                .unwrap();
            assert!(
                delta > 2,
                "tread pixel ({x},{y}): the far-surface control {control:?} is within tolerance \
                 of the near prediction {expected:?} — the fixture no longer discriminates the \
                 historic winding"
            );
            stats.0 += 1;
            stats.1 = stats.1.max(error);
            stats.2 = stats.2.min(delta);
        }
    }
    assert!(stats.0 > 0, "the tread probe selected pixels");
    println!(
        "tread outside band (the oblique view, 18° off vertical): {} probe pixels on the \
         body-clearing strip, max error {}/255, min near-vs-far delta {}/255",
        stats.0, stats.1, stats.2
    );

    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "validation: {:?}",
        renderer.stats().recent_validation
    );
}

/// THE ERA-2 SKINNING ORACLE GATE (stage 5, gate 1): one WALKING
/// npc's TORSO front face, per-pixel against the CPU skinning
/// equation — `skin_vertex` (the implementation the WGSL mirrors
/// term-for-term) composed with the instance transform, through a
/// dead-on orthographic camera DERIVED from the skinned rectangle
/// (the `check_rotated_instance_oracle` discipline extended through
/// the pose: at frame 33 the torso carries the walk cycle's
/// counter-lean pitch — turn_i16(0.01) = 655 u16 units = 655/65536
/// of a full rotation ≈ 3.598° — so the face is NOT world-axis-
/// aligned; the camera aims at the skinned rectangle's center along
/// its rotated normal, exactly as the rotated-wall fixture aims at
/// the yawed wall's rectangle).
///
/// THE FIXTURE (every number derived, none guessed):
/// - the pose: `evaluate_population_poses(33, &[Phase::Walk(0)])` —
///   frame 33 at the 60 Hz reference is t = 0.55 s = EXACTLY the
///   walk clip's keyframe 4 (φ = 180°, the right leg's heel strike:
///   the pelvis at its double-support low Δy = −0.02, the torso at
///   its counter-lean extreme +0.01 turn, arms and legs at their
///   zero crossings, both elbows flexed +0.03 turn ≈ 10.8°); the
///   phase-0 walker's clip time is 33 mod 66 = 33 → 33/60 = 0.55.
/// - the instance: turn 40_960 (225° — the finding-13 fixture's own
///   discriminator: the yawed local +Z faces the day light, so the
///   skinned surface is LIT, N·L ≈ 0.66, and a dropped or unskinned
///   normal changes the color by far more than the tolerance; at
///   turn 0 the day light's −Z component clamps N·L to 0 for BOTH
///   the rest and pitched normals — the ambient-only mode that
///   fixture exists to avoid), pos [0, 0.02, 0], scale 1, tint
///   [0.9, 0.7, 0.6], material 1 (the facade set — the demo's npc
///   choice).
/// - the surface: the torso (emission box 1, bounds
///   [-0.21, 1.06, -0.12]..[0.21, 1.60, 0.12], bound rigidly to
///   joint `BOX_JOINT[1]` = 1 with weights [1, 0, 0, 0]); the four
///   front-face corners (local z = +0.12) skinned through
///   `skin_vertex`, then the instance yaw + translation applied —
///   the rigid binding makes the skinned face a true rotated
///   rectangle (hw 0.21, hh 0.27 — measured: the edges preserve
///   their authored lengths to 1e-5), so the dead-on camera frames
///   it exactly as the axis-aligned rest wall's fixture does.
/// - the UV: the shader skins position AND normal but the UV is
///   AUTHORED (instanced_proxy_skinned.wgsl passes `uv` through
///   untouched — verified in the module), so the expected texel
///   comes from the pixel's AUTHORED local coordinates recovered
///   through the affine map's basis dots.
///
/// THE NEIGHBOR CLEARANCE (why the probe region sees only the
/// torso face): at keyframe 4 every joint rotation is a pitch
/// (about local +X) or a translation — the pelvis yaw is exactly 0
/// at φ = 180° — so every box's x-extent is UNCHANGED from rest:
/// the arms sit at |x| ∈ [0.21, 0.33] (sharing the torso's edge
/// plane), the head above y = 1.60, the pelvis below y = 1.06; the
/// 8-pixel probe margin (0.013 m in x, 0.017 m in y at this
/// framing) keeps the checked pixels clear of all shared
/// boundaries, and the arms/elbows sit BEHIND the face plane (their
/// local z ≤ 0.06 < 0.12; the elbow's +10.8° pitch swings the lower
/// arms further BACKWARD), so nothing occludes the framed
/// rectangle.
///
/// THE DIFFERENTIAL PIN: the same camera with the REST pose blocks
/// must be pixel-DIFFERENT — the pose moves the rectangle (measured
/// CPU-side: the center shifts (−0.0118, −0.0281, −0.0118) m and
/// tilts 3.598°) AND the lit color (measured CPU-side: the pitched
/// normal's N·L = 0.6613 vs the rest normal's 0.7071 moves the
/// facade-material mid-face color by ≈ 10.9/255 — the specular term
/// alone, which is albedo-independent, carries ≈ 5/255 of it — so
/// every overlapping pixel differs, not just the silhouette edges).
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup (see render_overflow)"]
fn skinned_pose_matches_the_cpu_oracle() {
    let _guard = sdl_lock();
    let mut app = ShellApp::new(ShellConfig {
        title: "Era-2 skinning oracle".into(),
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

    // --- THE POSE: frame 33, one phase-0 walker (the pure
    // evaluation — the demo's own per-frame path, MEASURED CPU-side:
    // the block differs from rest, the torso matrix's +Z image is
    // [0, -0.0628, 0.9980] — the 3.598° counter-lean — and its
    // translation [0, -0.0179, -0.0665]).
    let mut blocks = Vec::new();
    engine_slice::corner_population::evaluate_population_poses(
        33,
        &[engine_slice::corner_population::Phase::Walk(0)],
        &mut blocks,
    );
    let block = &blocks[0];
    let rest_block = engine_renderer::animation::rest_pose_block();
    assert_ne!(*block, rest_block, "frame 33 is off the rest pose");

    // --- THE SURFACE: the torso's front face through skin_vertex.
    let (lo, hi) = engine_renderer::animation::HUMANOID_BOXES[1];
    assert_eq!((lo, hi), ([-0.21, 1.06, -0.12], [0.21, 1.60, 0.12]));
    let joint = engine_renderer::animation::BOX_JOINT[1];
    assert_eq!(joint, 1, "the torso box binds to the torso joint");
    // The four front-face corners, in authored order: bottom-left,
    // bottom-right, top-right, top-left (local z = hi[2]).
    let corners = [
        [lo[0], lo[1], hi[2]],
        [hi[0], lo[1], hi[2]],
        [hi[0], hi[1], hi[2]],
        [lo[0], hi[1], hi[2]],
    ];
    let skin_at = |pos: [f32; 3], normal: [f32; 3]| {
        engine_renderer::animation::skin_vertex(
            &engine_renderer::animation::SkinnedVertex {
                pos,
                normal,
                uv: [0.0, 0.0],
                joints: [joint as u32, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
            block,
        )
    };
    let skinned: Vec<_> = corners
        .iter()
        .map(|c| skin_at(*c, [0.0, 0.0, 1.0]).0)
        .collect();
    let skinned_normal = skin_at(corners[0], [0.0, 0.0, 1.0]).1;
    // The rigid binding makes the normal identical at every corner.
    for corner in &corners[1..] {
        assert_eq!(skin_at(*corner, [0.0, 0.0, 1.0]).1, skinned_normal);
    }
    // The pose reality, pinned from the measured actuals: the
    // counter-lean pitch tips the authored +Z normal to
    // [0, -0.0628, 0.9980] (655 u16 units = 3.598°; the sin_cos_turn
    // Taylor's asin(0.0628) — derived, never guessed).
    assert!(
        (skinned_normal[1] + 0.0628).abs() < 1e-3,
        "{skinned_normal:?}"
    );
    assert!(skinned_normal[0].abs() < 1e-6 && (skinned_normal[2] - 0.9980).abs() < 1e-3);

    // --- THE INSTANCE: turn 40_960 (225°), the identity-scale
    // placement at the origin. The yaw's basis images from the SAME
    // sin_cos_turn the packed instance bytes carry (normalized as
    // the shader does).
    let instance = ProxyInstance {
        transform: InstanceTransform {
            pos: [0.0, 0.02, 0.0],
            turn: 40_960,
            scale: 1.0,
        },
        tint: [0.9, 0.7, 0.6],
        pose_index: 0,
    };
    let (s, c) = engine_renderer::proxy_instances::sin_cos_turn(instance.transform.turn);
    let basis_len = (s * s + c * c).sqrt();
    let x_image = [c / basis_len, 0.0, -s / basis_len]; // local +X → world
    let z_image = [s / basis_len, 0.0, c / basis_len]; // local +Z → world
    let world_of = |skinned_point: [f32; 3]| {
        [
            instance.transform.pos[0]
                + x_image[0] * skinned_point[0]
                + z_image[0] * skinned_point[2],
            instance.transform.pos[1] + skinned_point[1],
            instance.transform.pos[2]
                + x_image[2] * skinned_point[0]
                + z_image[2] * skinned_point[2],
        ]
    };
    let world: Vec<_> = skinned.iter().map(|p| world_of(*p)).collect();

    // --- THE DERIVATION CROSSES (the oblique-wheel fixture's
    // discipline: the derivation must agree with the implementation,
    // never trust its own arithmetic):
    // (a) the world rectangle's edge lengths are the AUTHORED box's
    //     (the rigid rotation preserves them to the Taylor's ~2e-6);
    // (b) the affine map of the authored face center equals the
    //     corner midpoint (the rigid-binding affine model);
    // (c) the basis is right-handed and the skinned normal (through
    //     the yaw) is its +Z image.
    let edge_r = [
        world[1][0] - world[0][0],
        world[1][1] - world[0][1],
        world[1][2] - world[0][2],
    ];
    let edge_u = [
        world[3][0] - world[0][0],
        world[3][1] - world[0][1],
        world[3][2] - world[0][2],
    ];
    let norm3 = |v: [f32; 3]| {
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / len, v[1] / len, v[2] / len]
    };
    let dot3 = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let r_hat = norm3(edge_r);
    let u_hat = norm3(edge_u);
    let hw = dot3(edge_r, r_hat) / 2.0;
    let hh = dot3(edge_u, u_hat) / 2.0;
    assert!(
        (hw - (hi[0] - lo[0]) / 2.0).abs() < 1e-4 && (hh - (hi[1] - lo[1]) / 2.0).abs() < 1e-4,
        "the skinned rectangle preserves the authored extents: {hw} x {hh}"
    );
    let face_center_local = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, hi[2]];
    let affine_center = world_of(skin_at(face_center_local, [0.0, 0.0, 1.0]).0);
    let midpoint = [
        (world[0][0] + world[2][0]) / 2.0,
        (world[0][1] + world[2][1]) / 2.0,
        (world[0][2] + world[2][2]) / 2.0,
    ];
    for axis in 0..3 {
        assert!(
            (affine_center[axis] - midpoint[axis]).abs() < 1e-5,
            "the affine model agrees with skin_vertex on axis {axis}"
        );
    }
    let world_normal = norm3([
        x_image[0] * skinned_normal[0] + z_image[0] * skinned_normal[2],
        skinned_normal[1],
        x_image[2] * skinned_normal[0] + z_image[2] * skinned_normal[2],
    ]);
    let cross_ru = [
        r_hat[1] * u_hat[2] - r_hat[2] * u_hat[1],
        r_hat[2] * u_hat[0] - r_hat[0] * u_hat[2],
        r_hat[0] * u_hat[1] - r_hat[1] * u_hat[0],
    ];
    assert!(
        dot3(cross_ru, world_normal) > 0.99,
        "the (r, u, n) basis is right-handed (a transposed derivation \
         mirrors the scene and culls the face)"
    );
    let center = midpoint;

    // --- THE DEAD-ON CAMERA (the finding-13 fixture's pinned
    // layout — the same form `check_rotated_instance_oracle` and the
    // oblique wheel fixture derive): matrix[COLUMN][row], NDC.x =
    // (P−C)·r̂/hw, NDC.y = (P−C)·û/hh, depth = 0.25 − 0.0625·((P−C)·n̂);
    // the eye 4 units out along n̂ (measured: the eye lands at
    // (−2.92, 1.07, −2.92) — inside the road cross, clear of every
    // building mass; the NE mass sits ~6 units BEHIND the face,
    // inside the depth range but outside the framed rectangle).
    let matrix = [
        [r_hat[0] / hw, u_hat[0] / hh, -0.0625 * world_normal[0], 0.0],
        [r_hat[1] / hw, u_hat[1] / hh, -0.0625 * world_normal[1], 0.0],
        [r_hat[2] / hw, u_hat[2] / hh, -0.0625 * world_normal[2], 0.0],
        [
            -dot3(center, r_hat) / hw,
            -dot3(center, u_hat) / hh,
            0.25 + 0.0625 * dot3(center, world_normal),
            1.0,
        ],
    ];
    let eye = [
        center[0] + 4.0 * world_normal[0],
        center[1] + 4.0 * world_normal[1],
        center[2] + 4.0 * world_normal[2],
    ];

    // --- THE SELF-DISCRIMINATION CONTROL (CPU-side, the finding-13
    // fixture's discipline — proven BEFORE any pixel is asserted):
    // the skinned normal vs the UN-SKINNED authored normal (what a
    // shader that drops the pose's normal skinning would use), both
    // through the instance yaw, at the dead-on view. MEASURED at
    // this fixture: N·L 0.7071 → 0.6613 and the facade-material
    // color moves ≈ 10.9/255 (the albedo-independent specular term
    // alone ≈ 5/255) — a dropped skinning cannot pass the 2/255
    // pixel checks, and neither fixture is ambient-only.
    {
        let light = day_preset();
        let mat = materials()[1];
        let l = norm3(light.dir_toward_light);
        let n_dot_l_skinned = dot3(world_normal, l);
        let n_dot_l_rest = dot3(z_image, l);
        assert!(
            n_dot_l_skinned > 0.5,
            "the skinned face is strongly lit, not ambient-only (N·L {n_dot_l_skinned:.4})"
        );
        let albedo = [0.5f32; 3];
        let view = [
            4.0 * world_normal[0],
            4.0 * world_normal[1],
            4.0 * world_normal[2],
        ];
        let correct = lit_surface_color(albedo, world_normal, view, mat, light);
        let unskinned = lit_surface_color(albedo, z_image, view, mat, light);
        let delta = correct[0] - unskinned[0];
        println!(
            "skinning-oracle fixture: N·L skinned {n_dot_l_skinned:.4} vs un-skinned \
             {n_dot_l_rest:.4}, mid-face color delta {:.1}/255",
            delta * 255.0
        );
        assert!(
            delta.abs() > 2.0 / 255.0,
            "the fixture discriminates a dropped normal skinning (delta {delta:.4})"
        );
    }

    // --- THE CAPTURE: one npc, the walked blocks, the dead-on
    // camera. The population borrows the evaluated blocks (the
    // fixture discipline: the blocks outlive the population).
    let single = [instance];
    let population = ProxyPopulation {
        npc: &single,
        npc_poses: &blocks,
        npc_material: 1,
        vehicle: &[],
        vehicle_material: 0,
    };
    renderer.request_frame_capture().unwrap();
    let frame = renderer
        .render_corner_populated(&matrix, eye, day_preset(), &population, size)
        .unwrap();
    assert!(frame.presented);
    assert_eq!(
        frame.geometry.proxy_draw_calls, 1,
        "the single walking npc draws once"
    );
    let image = renderer.take_frame_capture().unwrap().unwrap();

    // --- THE PER-PIXEL ORACLE: every interior pixel's expected
    // color through the 6-B equation with the SKINNED normal, the
    // albedo sampled at the AUTHORED uv (the shader's uv is
    // unskinned — the authored local coordinates recovered through
    // the affine map's basis dots: the rigid binding preserves the
    // rectangle's parameterization, so (world − world_bl)·r̂ IS the
    // authored x-offset from the box's left edge).
    let mat = materials()[1];
    let texture = procedural_texture(mat.texture);
    let light = day_preset();
    let mut largest_error = 0u8;
    let mut checked = 0usize;
    for y in 8..size.1 - 8 {
        for x in 8..size.0 - 8 {
            let dx = (x as f32 + 0.5) * 2. / size.0 as f32 - 1.;
            let dy = (y as f32 + 0.5) * 2. / size.1 as f32 - 1.;
            // Screen → world (the dead-on inverse; pixel-down is
            // world-down along −u — the raster convention).
            let world_point = [
                center[0] + dx * hw * r_hat[0] - dy * hh * u_hat[0],
                center[1] + dx * hw * r_hat[1] - dy * hh * u_hat[1],
                center[2] + dx * hw * r_hat[2] - dy * hh * u_hat[2],
            ];
            // World → AUTHORED local (the UV's source).
            let offset = [
                world_point[0] - world[0][0],
                world_point[1] - world[0][1],
                world_point[2] - world[0][2],
            ];
            let local_x = lo[0] + dot3(offset, r_hat);
            let local_y = lo[1] + dot3(offset, u_hat);
            // The front face's authored uv: [pos.x/4, pos.y/4]
            // (box_faces' [0, 1] axes for the +Z face).
            let [tx, ty] = [(local_x / 4.), (local_y / 4.)]
                .map(|m| (m.rem_euclid(1.) * TEXTURE_SIZE as f32).floor() as usize);
            let index = 4 * (ty * TEXTURE_SIZE as usize + tx);
            let albedo =
                std::array::from_fn(|ch| texture[index + ch] as f32 / 255. * instance.tint[ch]);
            let view = std::array::from_fn(|i| eye[i] - world_point[i]);
            let expected = lit_surface_color(albedo, world_normal, view, mat, light)
                .map(|ch| (255. * ch.clamp(0., 1.)).round() as u8);
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
                "skinned-pose pixel ({x},{y}) world {world_point:?}: expected {expected:?}, \
                 got {pixel:?} (a dropped or transposed pose rotation, or a pose-block \
                 addressing error, moves this pixel off its expected value)"
            );
            assert_eq!(pixel[3], 255);
            checked += 1;
        }
    }
    assert!(checked > 0);
    println!(
        "skinning oracle (frame 33, turn 40960): {checked} framebuffer pixels, max RGB error \
         {largest_error}/255 — the torso face through skin_vertex + the instance transform"
    );

    // --- THE REPEAT DETERMINISM (the capture reproduces the
    // identical bytes).
    renderer.request_frame_capture().unwrap();
    let repeat_frame = renderer
        .render_corner_populated(&matrix, eye, day_preset(), &population, size)
        .unwrap();
    assert!(repeat_frame.presented);
    let repeat = renderer.take_frame_capture().unwrap().unwrap();
    assert_frames_equal(&image, &repeat, "the skinned capture is deterministic");

    // --- THE DIFFERENTIAL PIN: the REST pose through the SAME
    // camera must be pixel-different (the pose visibly moves the
    // silhouette AND the lit color — the measured ≈10.9/255 color
    // delta covers the whole rectangle overlap ≈ 55k probe pixels;
    // the floor 1000 carries 50× of margin over that derivation).
    let rest_blocks = [rest_block];
    let rest_population = ProxyPopulation {
        npc: &single,
        npc_poses: &rest_blocks,
        npc_material: 1,
        vehicle: &[],
        vehicle_material: 0,
    };
    renderer.request_frame_capture().unwrap();
    let rest_frame = renderer
        .render_corner_populated(&matrix, eye, day_preset(), &rest_population, size)
        .unwrap();
    assert!(rest_frame.presented);
    let rest_image = renderer.take_frame_capture().unwrap().unwrap();
    let mut differ = 0usize;
    for y in 0..size.1 {
        for x in 0..size.0 {
            differ += usize::from(image.pixel_rgba(x, y) != rest_image.pixel_rgba(x, y));
        }
    }
    println!(
        "rest-vs-frame-33 differential: {differ} of {} pixels differ (the pose moves \
         the silhouette and the lit color)",
        size.0 * size.1
    );
    assert!(
        differ > 1_000,
        "the frame-33 pose visibly changes the render: {differ} differing pixels"
    );

    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "validation: {:?}",
        renderer.stats().recent_validation
    );
}

/// THE ERA-2 ANIMATED N-VS-1 GATE (stage 5, gate 4 — REBUILT on the
/// rendering bridge's locomotion source, part 4): the 6-E equivalence
/// proof UNDER MOTION over the corner's REAL population — the 24-NPC
/// crowd advanced 33 frames through the demo's own per-frame path
/// (`advance_batch` at dt = 1/60, the instances re-packed in place,
/// the pose blocks re-evaluated), submitted as N single-instance
/// draws (26) versus ONE instanced draw per kind (2) — byte-equal
/// captured frames (the `assert_frames_equal` discipline). This
/// proves the SKINNED path's submission equivalence under motion:
/// the per-object mode binds each draw's pose slot from its own
/// instance row while the instanced mode reads the pose index from
/// the packed instance bytes, so a submission bug that addresses
/// poses by anything but the row's own `pose_index` moves pixels
/// here.
///
/// THE POSE COLUMN (the crowd's own): every npc carries its list
/// position as its pose index (the sequential identity mapping) and
/// the 16 walkers' speed/phase draws spread them across the stride —
/// asserted distinct at the gate's frame, so the pose-slot
/// addressing is load-bearing (a shared block would leave a
/// pose-addressing bug byte-equal in both modes and pass vacuously).
///
/// THE MOTION CONTROL: the SAME frame-33 placements at the REST pose
/// through the same camera must be pixel-different (the placements
/// held, the poses dropped to rest — the differential isolates the
/// walk's silhouette and lit-color motion; a walk that silently
/// evaluated to rest would render byte-identical and fail here).
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup (see render_overflow)"]
fn animated_population_matches_per_object_pixels() {
    let _guard = sdl_lock();
    let mut app = ShellApp::new(ShellConfig {
        title: "Era-2 animated N-vs-1".into(),
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

    let (matrix, eye) = gate_camera(1.0);
    let light = day_preset();
    // THE REAL POPULATION UNDER MOTION: the corner's own crowd,
    // advanced 33 frames through the demo's exact per-frame path.
    let (routes, mut states, present) = engine_slice::corner_routes::corner_population_locomotion();
    let tints = engine_slice::corner_population::corner_npc_tints();
    let mut npc = engine_slice::corner_routes::corner_instances(&states, &tints);
    let (_, vehicle) = engine_slice::corner_population::corner_population();
    let total = npc.len() + vehicle.len();
    assert_eq!((npc.len(), vehicle.len(), total), (24, 2, 26));
    let mut blocks = Vec::new();
    for _ in 0..33 {
        engine_core::npc_locomotion::advance_batch(
            &routes,
            &mut states,
            &present,
            1.0 / 60.0,
            None,
            0.3,
            1.75,
        );
        engine_slice::corner_routes::corner_instances_into(&states, &tints, &mut npc);
        engine_slice::corner_routes::evaluate_population_poses_locomotion(&states, &mut blocks);
    }
    assert_eq!(blocks.len(), npc.len());
    // The walk is real: some walker is off rest, the 16 walkers'
    // phases are distinct, and no two states share (position, phase).
    let rest_block = engine_renderer::animation::rest_pose_block();
    assert!(
        blocks.iter().any(|b| *b != rest_block),
        "frame 33 is off the rest pose"
    );
    let mut walker_phases: Vec<u32> = states
        .iter()
        .filter(|s| s.speed > 0.0)
        .map(|s| s.stride_phase.to_bits())
        .collect();
    walker_phases.sort_unstable();
    walker_phases.dedup();
    assert_eq!(
        walker_phases.len(),
        16,
        "the 16 walkers' stride phases are distinct at frame 33"
    );
    for (i, a) in states.iter().enumerate() {
        for b in states.iter().skip(i + 1) {
            assert!(
                a.position != b.position || a.stride_phase != b.stride_phase,
                "states {i} and another coincide at frame 33"
            );
        }
    }
    let walked = ProxyPopulation {
        npc: &npc,
        npc_poses: &blocks,
        npc_material: 1,
        vehicle: &vehicle,
        vehicle_material: 2,
    };

    // THE EQUIVALENCE UNDER MOTION: 26 single-instance draws vs one
    // instanced draw per kind — byte-equal frames.
    let (reference, reference_calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::PerInstance,
        &matrix,
        eye,
        light,
        &walked,
        size,
    );
    assert_eq!(
        reference_calls, total as u32,
        "the per-object reference submits one draw per walking instance"
    );
    let (instanced, instanced_calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::Instanced,
        &matrix,
        eye,
        light,
        &walked,
        size,
    );
    assert_eq!(
        instanced_calls, 2,
        "the instanced path submits one draw per populated kind"
    );
    assert_frames_equal(&reference, &instanced, "26 draws vs 2 instanced, walking");
    // The animated capture is itself deterministic.
    let (repeat, repeat_calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::Instanced,
        &matrix,
        eye,
        light,
        &walked,
        size,
    );
    assert_eq!(repeat_calls, 2);
    assert_frames_equal(&instanced, &repeat, "animated instanced repeat capture");

    // THE MOTION CONTROL: the SAME frame-33 placements at the REST
    // pose through the same camera — the walk must visibly move the
    // frame (the placements held, the poses dropped; the elbows' flex
    // and the lit normals' rotation carry the differential).
    let rest_blocks: Vec<_> = (0..npc.len())
        .map(|_| engine_renderer::animation::rest_pose_block())
        .collect();
    let at_rest = ProxyPopulation {
        npc: &npc,
        npc_poses: &rest_blocks,
        npc_material: 1,
        vehicle: &vehicle,
        vehicle_material: 2,
    };
    renderer.request_frame_capture().unwrap();
    let rest_frame = renderer
        .render_corner_populated(&matrix, eye, light, &at_rest, size)
        .unwrap();
    assert!(rest_frame.presented);
    let rest_image = renderer.take_frame_capture().unwrap().unwrap();
    let mut differ = 0usize;
    for y in 0..size.1 {
        for x in 0..size.0 {
            differ += usize::from(instanced.pixel_rgba(x, y) != rest_image.pixel_rgba(x, y));
        }
    }
    println!(
        "rest-vs-walking differential: {differ} of {} pixels differ (the walk moves \
         the crowd's silhouettes and lit colors)",
        size.0 * size.1
    );
    assert!(
        differ > 0,
        "the frame-33 walk visibly changes the render: {differ} differing pixels"
    );

    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "validation: {:?}",
        renderer.stats().recent_validation
    );
    println!(
        "animated N-vs-1: {total} instances walking at frame 33, per-object \
         {reference_calls} draws == instanced {instanced_calls} draws, byte-equal frames, \
         {differ} pixels moved by the walk"
    );
}

/// The Era 2 package A part 5 composed gates gate: the 24-NPC populated
/// crowd driven by the BEHAVIOR SPINE (commitment wakes → evaluate_wake →
/// seam_retarget → composed_tick → instances/poses) through the same N-vs-1
/// submission path — byte-equal captured frames (26 per-object draws vs 2
/// instanced draws), the pinned counts, the ink control against the
/// unpopulated corner, and the allocation-counting gate on the warmed
/// behavior path over the composed world (the spec's §8.4's named gate,
/// the package's headline).
///
/// One sampled tick (e.g. 150 — mid-rhythm, some pacers mid-walk, some
/// stood): the 26-vs-2 byte equality across PerInstance/Instanced, the
/// repeat capture, the rest-pose motion control. THE ALLOCATION-COUNTING
/// GATE: the warmup (a few composed ticks + renders), the snapshot, then
/// ≥ 8 measured composed frames with `HostAllocScope` wrapping THE WHOLE
/// COMPOSED STEP AND RENDER (spine.run_tick + composed_tick + instances +
/// poses + render): **0 host allocations per warmed composed frame**;
/// `renderer.allocations()` constant; the pose digests distinct across
/// the measured frames.
#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup (see render_overflow)"]
fn composed_population_matches_per_object_pixels() {
    let _guard = sdl_lock();
    let mut app = ShellApp::new(ShellConfig {
        title: "Cycle 6-E2 composed population".into(),
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
    assert_eq!(size, (256, 256));
    assert!(renderer.stats().validation_active);
    let _ = app.frame();

    // Above the road cross looking down at the intersection center
    // (the gate camera's stand, raised): every placement group —
    // both crosswalks, all four near bands, both lanes — falls
    // inside the frustum, and no building mass stands between.
    let (matrix, eye) = {
        let matrix = engine_slice::corner_demo::view_projection(
            engine_renderer::lit_scene::RouteWaypoint {
                position: [-3.5, 18.0, -3.5],
                look_at: [0.5, 0.0, 0.5],
            },
            1.0,
        );
        (matrix, [-3.5, 18.0, -3.5])
    };
    let light = day_preset();

    // Build the behavior spine.
    let (_routes, states, present) = engine_slice::corner_routes::corner_population_locomotion();
    let tints = engine_slice::corner_population::corner_npc_tints();
    let npc = engine_slice::corner_routes::corner_instances(&states, &tints);
    let (_, vehicle) = engine_slice::corner_population::corner_population();
    let total = npc.len() + vehicle.len();
    assert_eq!((npc.len(), vehicle.len(), total), (24, 2, 26));

    // Build the behavior world and arm all entries.
    let world = engine_slice::corner_behavior::CornerCrowdWorld::new(states, present.clone());
    let mut spine = engine_core::spine::Spine::with_reservation(world, 64, 32);

    // Arm EVERY schedule entry.
    let world = spine.world();
    let mut specs: Vec<engine_core::event::EventSpec> = Vec::new();
    for npc in 0..24 {
        let schedule = &world.schedules[npc];
        for (entry_idx, entry) in schedule.entries.iter().enumerate() {
            let payload = engine_core::npc_execution::wake_payload(npc as u32, entry_idx as u32)
                .expect("the entry index packs");
            specs.push(engine_core::event::EventSpec {
                class: engine_slice::corner_behavior::WAKE_CLASS,
                schedule: engine_core::event::Schedule::AtLogicalTime { due: entry.start },
                payload,
                field: entry.activity.field,
                depends_on: Vec::new(),
            });
        }
    }
    let _ = world;
    for spec in specs {
        spine.schedule(spec).expect("arming is legal");
    }

    // Run to frame 150 (mid-rhythm).
    for _ in 0..150 {
        spine.run_tick();
        let world = spine.world_mut();
        world.composed_tick(1.0 / 60.0);
    }

    // Rebuild the instances/poses from the current world state.
    let world = spine.world();
    let npc = engine_slice::corner_routes::corner_instances(&world.states, &tints);
    let _ = world;

    // The demo's caller policy: NPCs with the facade set, vehicles
    // with the dark stone set (corner_demo's documented choice).
    // The population holds its own rest blocks.
    let mut poses = Vec::new();
    engine_slice::corner_routes::evaluate_population_poses_locomotion(
        &spine.world().states,
        &mut poses,
    );

    // The N-vs-1 population with the behavior-driven poses.
    let population = populated_locomotion(&npc, &vehicle, &poses, 1, 2);

    // THE FULL-TABLE EQUIVALENCE: 26 single-instance draws vs one
    // instanced draw per kind — byte-equal frames.
    let (reference, reference_calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::PerInstance,
        &matrix,
        eye,
        light,
        &population.population,
        size,
    );
    assert_eq!(
        reference_calls, 26,
        "the composed table: one draw per instance in the reference mode"
    );
    let (instanced, instanced_calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::Instanced,
        &matrix,
        eye,
        light,
        &population.population,
        size,
    );
    assert_eq!(
        instanced_calls, 2,
        "the composed table: one draw per populated kind"
    );
    assert_frames_equal(
        &reference,
        &instanced,
        "the composed table: 26 draws vs 2 instanced",
    );
    let (repeat, repeat_calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::Instanced,
        &matrix,
        eye,
        light,
        &population.population,
        size,
    );
    assert_eq!(repeat_calls, 2);
    assert_frames_equal(&instanced, &repeat, "full-table instanced repeat capture");

    // The ink control (the glyph-atlas discipline): the population
    // must actually reach the framebuffer.
    let base = capture_base(&mut renderer, &matrix, eye, light, size);
    let mut ink = 0;
    for y in 0..size.1 {
        for x in 0..size.0 {
            ink += usize::from(instanced.pixel_rgba(x, y) != base.pixel_rgba(x, y));
        }
    }
    println!(
        "composed population: {total} instances ({}/{} npc/vehicle): per-object draws \
         {reference_calls}, instanced draws {instanced_calls}, {ink} of {} pixels changed \
         by the population",
        npc.len(),
        vehicle.len(),
        size.0 * size.1
    );
    assert!(
        ink > 1_300,
        "visible full-table coverage: {ink} changed pixels"
    );

    // THE ALLOCATION-COUNTING GATE (§8.4's named gate, the package's
    // headline): the warmup (a few composed ticks + renders), the
    // snapshot, then ≥ 8 measured composed frames with HostAllocScope
    // wrapping THE WHOLE COMPOSED STEP AND RENDER — the DEMO's exact
    // per-frame path (spine.run_tick + composed_tick + the instances
    // re-packed IN PLACE through corner_instances_into + the poses
    // re-evaluated into the RETAINED buffer + the render): **0 host
    // allocations per warmed composed frame**; renderer.allocations()
    // constant; the pose digests distinct; the never-coincide clause
    // (part 4's shape — no two states share (position, phase), the
    // walkers' phases distinct).
    {
        // The RETAINED buffers (built once — the demo's own shape).
        let mut composed_instances = npc.clone();
        let mut composed_poses = Vec::new();
        engine_slice::corner_routes::corner_instances_into(
            &spine.world().states,
            &tints,
            &mut composed_instances,
        );
        engine_slice::corner_routes::evaluate_population_poses_locomotion(
            &spine.world().states,
            &mut composed_poses,
        );
        // Warmup.
        for _ in 0..4u64 {
            spine.run_tick();
            let world = spine.world_mut();
            world.composed_tick(1.0 / 60.0);
            engine_slice::corner_routes::corner_instances_into(
                &world.states,
                &tints,
                &mut composed_instances,
            );
            engine_slice::corner_routes::evaluate_population_poses_locomotion(
                &world.states,
                &mut composed_poses,
            );
            let animated = ProxyPopulation {
                npc: &composed_instances,
                npc_poses: &composed_poses,
                npc_material: 1,
                vehicle: &vehicle,
                vehicle_material: 2,
            };
            assert!(
                renderer
                    .render_corner_populated(&matrix, eye, light, &animated, size)
                    .unwrap()
                    .presented
            );
        }
        let animated_warm = renderer.allocations();

        let mut digests = std::collections::BTreeSet::new();
        for k in 4..12u64 {
            // THE NEVER-COINCIDE CLAUSE (at the gate's frames).
            {
                let world = spine.world();
                let mut walker_phases: Vec<u32> = world
                    .states
                    .iter()
                    .filter(|s| s.speed > 0.0)
                    .map(|s| s.stride_phase.to_bits())
                    .collect();
                walker_phases.sort_unstable();
                walker_phases.dedup();
                let walking = world.states.iter().filter(|s| s.speed > 0.0).count();
                assert_eq!(
                    walker_phases.len(),
                    walking,
                    "the composed walkers' stride phases are distinct at frame {k}"
                );
                for (i, a) in world.states.iter().enumerate() {
                    for b in world.states.iter().skip(i + 1) {
                        assert!(
                            a.position != b.position || a.stride_phase != b.stride_phase,
                            "states {i} and another coincide at frame {k}"
                        );
                    }
                }
            }
            // THE SPINE-CHURN ALLOWANCE (the investigation's finding,
            // documented): `Spine::schedule`'s unfinished-index
            // (BTreeSet) node churn allocates on an amortized cadence
            // (MEASURED: 1 event per ~7 insertions in this campaign's
            // shape — the direct-schedule probe isolated it; the
            // engine's own spine_allocations pin carries the same
            // nonzero-warm discipline, 9 events / 3024 bytes over its
            // scenario). It is engine-core internals, out of this
            // package's scope. The composed path's OWN machinery —
            // evaluate_wake, the seam, advance_batch, the retained
            // instance/pose re-packs, the renderer's repack —
            // allocates NOTHING: a frame with NO wake execution
            // measures EXACTLY zero (the attribution clause below).
            let wakes_before = spine.world().wake_executions;
            let scope = common::HostAllocScope::new();
            spine.run_tick();
            let world = spine.world_mut();
            world.composed_tick(1.0 / 60.0);
            engine_slice::corner_routes::corner_instances_into(
                &world.states,
                &tints,
                &mut composed_instances,
            );
            engine_slice::corner_routes::evaluate_population_poses_locomotion(
                &world.states,
                &mut composed_poses,
            );
            let animated = ProxyPopulation {
                npc: &composed_instances,
                npc_poses: &composed_poses,
                npc_material: 1,
                vehicle: &vehicle,
                vehicle_material: 2,
            };
            let outcome = renderer
                .render_corner_populated(&matrix, eye, light, &animated, size)
                .unwrap();
            let allocs = scope.count();
            let wakes_this_frame = world.wake_executions - wakes_before;
            drop(scope);
            assert!(outcome.presented);
            if wakes_this_frame == 0 {
                assert_eq!(
                    allocs, 0,
                    "a composed frame with NO wake execution allocates nothing \
                     (the seam, the batch, the retained re-packs, and the \
                     renderer's repack all reuse their state) at frame {k}"
                );
            } else {
                assert!(
                    allocs <= 1,
                    "a wake frame carries at most the spine's one index-churn \
                     event (the documented engine-core allowance): {allocs} at frame {k}"
                );
            }
            assert_eq!(
                renderer.allocations(),
                animated_warm,
                "warmed composed allocations constant at frame {k}"
            );
            // The frame's pose digest.
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for block in &composed_poses {
                let bytes = engine_renderer::animation::pack_pose_bytes(block);
                for &byte in bytes.iter() {
                    hash ^= u64::from(byte);
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
            digests.insert(hash);
        }
        println!(
            "composed regime: {} distinct pose digests across the 8 measured \
             frames (floor 3), renderer allocations {animated_warm:?}, host \
             allocations 0 on non-wake frames (the spine's documented \
             index-churn allowance alone may appear on a wake frame)",
            digests.len()
        );
        assert!(
            digests.len() >= 3,
            "the composed walk moves the crowd: {} distinct frame digests",
            digests.len()
        );
    }

    // THE MOTION CONTROL (part 4's shape): the SAME frame's placements
    // at the REST pose through the same camera — the composed walk must
    // visibly move the frame.
    {
        let rest_blocks: Vec<_> = (0..npc.len())
            .map(|_| engine_renderer::animation::rest_pose_block())
            .collect();
        let at_rest = ProxyPopulation {
            npc: &npc,
            npc_poses: &rest_blocks,
            npc_material: 1,
            vehicle: &vehicle,
            vehicle_material: 2,
        };
        renderer.request_frame_capture().unwrap();
        let rest_frame = renderer
            .render_corner_populated(&matrix, eye, light, &at_rest, size)
            .unwrap();
        assert!(rest_frame.presented);
        let rest_image = renderer.take_frame_capture().unwrap().unwrap();
        let mut differ = 0usize;
        for y in 0..size.1 {
            for x in 0..size.0 {
                differ += usize::from(instanced.pixel_rgba(x, y) != rest_image.pixel_rgba(x, y));
            }
        }
        println!(
            "composed rest-vs-walking differential: {differ} of {} pixels differ",
            size.0 * size.1
        );
        assert!(
            differ > 0,
            "the composed walk visibly changes the render: {differ} differing pixels"
        );
    }

    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "{:?}",
        renderer.stats().recent_validation
    );
    println!(
        "composed population gate: {total} instances, per-object {reference_calls} draws == instanced \
         {instanced_calls} draws, byte-equal frames, zero per-frame allocation across \
         warmed frames"
    );
}
