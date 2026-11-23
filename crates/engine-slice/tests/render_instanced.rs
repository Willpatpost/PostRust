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

use engine_renderer::lit_scene::{LightParams, day_preset, lit_surface_color, materials};
use engine_renderer::proc_texture::{TEXTURE_SIZE, procedural_texture};
use engine_renderer::proxy_instances::{INSTANCE_STRIDE, InstanceTransform, ProxyInstance};
use engine_renderer::{
    CapturedFrame, DemoScenePreference, ProxyDrawMode, ProxyPopulation, RenderConfig, Renderer,
    ValidationPreference,
};
use engine_shell::{DriverPreference, ShellApp, ShellConfig};

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
    };
    let vehicle = |pos: [f32; 3], turn: u16, tint: [f32; 3]| ProxyInstance {
        transform: InstanceTransform {
            pos,
            turn,
            scale: 1.0,
        },
        tint,
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

fn populated<'a>(
    npc: &'a [ProxyInstance],
    vehicle: &'a [ProxyInstance],
    npc_material: u32,
    vehicle_material: u32,
) -> ProxyPopulation<'a> {
    ProxyPopulation {
        npc,
        npc_material,
        vehicle,
        vehicle_material,
    }
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
    let population = populated(&npc, &vehicle, 1, 2);

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
        &population,
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
        &population,
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
        &population,
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
            let (image, calls) =
                capture_populated(&mut renderer, mode, &matrix, eye, light, &population, size);
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
        .render_corner_populated(&matrix, eye, light, &population, (size.0 + 8, size.1 + 8))
        .unwrap();
    assert!(replacement.recreated, "the resize forced a recreation");
    let (after, calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::Instanced,
        &matrix,
        eye,
        light,
        &population,
        size,
    );
    assert_eq!(calls, 2);
    let after_stats = renderer.allocations();
    assert_eq!(
        (
            after_stats.texture_memory,
            after_stats.descriptor_pools,
            after_stats.descriptor_sets,
            after_stats.samplers
        ),
        (
            warm.texture_memory,
            warm.descriptor_pools,
            warm.descriptor_sets,
            warm.samplers
        ),
        "the corner's static allocations survived the replacement"
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
            &population,
            size,
        );
        assert_eq!(calls, 2);
        assert_frames_equal(&after, &image, "post-replacement stability");
        assert_eq!(renderer.allocations(), after_stats);
    }

    // The independent oracle pins (single instance, orthographic,
    // per-pixel against the 6-B CPU reference with albedo * tint):
    // the NPC torso's +Z wall and the vehicle body's +Z wall.
    let npc_wall = OracleWall {
        instance: ProxyInstance {
            transform: InstanceTransform {
                pos: [0.0, 0.02, 0.0],
                turn: 0,
                scale: 1.0,
            },
            tint: [0.9, 0.7, 0.6],
        },
        material: 1,
        half_width: 0.24,
        y_local: [0.85, 1.45],
        eye: [0.0, 1.17, 4.0],
    };
    let single_npc = [npc_wall.instance];
    check_single_instance_oracle(
        &mut renderer,
        size,
        &npc_wall,
        &populated(&single_npc, &[], 1, 0),
    );
    let vehicle_wall = OracleWall {
        instance: ProxyInstance {
            transform: InstanceTransform {
                pos: [0.0, 0.0, 0.0],
                turn: 0,
                scale: 1.0,
            },
            tint: [0.75, 0.18, 0.15],
        },
        material: 2,
        half_width: 0.9,
        y_local: [0.35, 1.0],
        eye: [0.0, 0.675, 6.0],
    };
    let single_vehicle = [vehicle_wall.instance];
    check_single_instance_oracle(
        &mut renderer,
        size,
        &vehicle_wall,
        &populated(&[], &single_vehicle, 0, 2),
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
            &populated(&over, &single_vehicle, 1, 2),
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
            &populated(&single_npc, &[], 3, 0),
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
        &population,
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
    let (npc, vehicle) = engine_slice::corner_population::corner_population();
    let total = npc.len() + vehicle.len();
    assert_eq!((npc.len(), vehicle.len(), total), (24, 2, 26));
    // The demo's caller policy: NPCs with the facade set, vehicles
    // with the dark stone set (corner_demo's documented choice).
    let population = populated(&npc, &vehicle, 1, 2);

    // THE FULL-TABLE EQUIVALENCE: 26 single-instance draws vs one
    // instanced draw per kind — byte-equal frames.
    let (reference, reference_calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::PerInstance,
        &matrix,
        eye,
        light,
        &population,
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
        &population,
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
        &population,
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
            let (image, calls) =
                capture_populated(&mut renderer, mode, &matrix, eye, light, &population, size);
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
    let (npc, vehicle) = engine_slice::corner_population::corner_population();
    let total = npc.len() + vehicle.len();
    assert_eq!((npc.len(), vehicle.len(), total), (24, 2, 26));
    let population = populated(&npc, &vehicle, 1, 2);

    // THE THREE-WAY EQUIVALENCE: per-object (26 draws) vs direct
    // instanced (2) vs indirect (2) — byte-equal frames.
    let (reference, reference_calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::PerInstance,
        &matrix,
        eye,
        light,
        &population,
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
        &population,
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
        &population,
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
        &population,
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
    for mode in [
        ProxyDrawMode::Indirect,
        ProxyDrawMode::Instanced,
        ProxyDrawMode::PerInstance,
    ] {
        for _ in 0..2 {
            let (image, calls) =
                capture_populated(&mut renderer, mode, &matrix, eye, light, &population, size);
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
    println!("warmed indirect-path populated allocations: {indirect_warm:?}");

    // The pipeline-replacement lifecycle (the finding-A re-ensure):
    // a forced recreation replaces the pipelines AND the proxy
    // resources — including the indirect command buffer, whose
    // draws the replacement records. Populated indirect frames
    // continue drawing byte-identically; the corner's static
    // allocations survive; the counters resume constant.
    renderer.set_proxy_draw_mode(ProxyDrawMode::Indirect);
    let replacement = renderer
        .render_corner_populated(&matrix, eye, light, &population, (size.0 + 8, size.1 + 8))
        .unwrap();
    assert!(replacement.recreated, "the resize forced a recreation");
    let (after, calls) = capture_populated(
        &mut renderer,
        ProxyDrawMode::Indirect,
        &matrix,
        eye,
        light,
        &population,
        size,
    );
    assert_eq!(calls, 2);
    assert_frames_equal(&indirect, &after, "the re-ensured indirect path");
    let after_stats = renderer.allocations();
    assert_eq!(
        (
            after_stats.texture_memory,
            after_stats.descriptor_pools,
            after_stats.descriptor_sets,
            after_stats.samplers
        ),
        (
            indirect_warm.texture_memory,
            indirect_warm.descriptor_pools,
            indirect_warm.descriptor_sets,
            indirect_warm.samplers
        ),
        "the corner's static allocations survived the replacement"
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
            &population,
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
    // cannot pass this).
    let npc_wall = OracleWall {
        instance: ProxyInstance {
            transform: InstanceTransform {
                pos: [0.0, 0.02, 0.0],
                turn: 0,
                scale: 1.0,
            },
            tint: [0.9, 0.7, 0.6],
        },
        material: 1,
        half_width: 0.24,
        y_local: [0.85, 1.45],
        eye: [0.0, 1.17, 4.0],
    };
    let single_npc = [npc_wall.instance];
    renderer.set_proxy_draw_mode(ProxyDrawMode::Indirect);
    check_single_instance_oracle(
        &mut renderer,
        size,
        &npc_wall,
        &populated(&single_npc, &[], 1, 0),
    );

    // The typed refusals reach the caller through the indirect path
    // too (nothing rendered, no frame state touched): 4097 NPCs is
    // one past MAX_INSTANCES (the TOTAL-population bound).
    let over: Vec<_> = vec![single_npc[0]; 4097];
    let refused = renderer
        .render_corner_populated(&matrix, eye, light, &populated(&over, &[], 1, 0), size)
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
        &population,
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
    let commands = engine_renderer::proxy_indirect::indirect_slot_commands(24, 2);
    let word = |at: usize| u32::from_le_bytes(commands[at..at + 4].try_into().unwrap());
    assert_eq!(engine_renderer::proxy_indirect::INDIRECT_SLOT_BYTES, 32);
    assert_eq!((word(0), word(4), word(16), word(20)), (108, 24, 72, 2));
    println!(
        "indirect gate: {total} instances, per-object 26 == direct 2 == indirect 2 draws, \
         byte-equal frames, oracle wall within 2/255 through the indirect submission, \
         one-time command-buffer allocation, zero per-frame allocation across warmed frames"
    );
}
