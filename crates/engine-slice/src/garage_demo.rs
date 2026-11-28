//! The Garage demo route (P4 S6 — the design's §7 second scene's
//! CLI surface): the scripted presentation route over
//! [`crate::garage_scene`]'s authored content, rendered ENTIRELY
//! through the public API — `Renderer::attach_scene` +
//! `Renderer::submit_frame`, the same submission contract the
//! corner route uses, with ZERO renderer internals and ZERO
//! corner types. The route sweeps the door-side orbit at a fixed
//! 60 Hz cadence (the corner route's own shape at garage scale),
//! always with shadows + sky + post on (the scene's authored
//! stack), the wet floor behind `--wet`, and the population's
//! pose blocks advancing with the presented-frame counter (the
//! deterministic-clock discipline — frame k always shows pose
//! t_k). PRESENTATION-ONLY: no timing distribution prints (the
//! corner route remains the measurement route); the exit summary
//! carries the correctness lines (presented count, validation,
//! the warmed-allocation constancy when no recreation
//! intervened).

use crate::corner_demo::view_projection;
use crate::garage_scene::{
    GARAGE_NPC_MATERIAL, GARAGE_ROUTE_FRAMES, GARAGE_VEHICLE_MATERIAL, garage_bias, garage_camera,
    garage_light, garage_phases, garage_population, garage_scene_description, garage_sky,
};
use engine_renderer::lit_scene::RouteWaypoint;
use engine_renderer::{
    DemoScenePreference, PresentModePreference, ProxyPopulation, RenderConfig, Renderer,
    ValidationPreference,
};
use engine_shell::{DriverPreference, Keycode, ShellApp, ShellConfig, pace_until};
use std::time::{Duration, Instant};

/// The demo's frame cadence (the corner route's own reference).
const FPS: u32 = 60;

/// The Garage. `frames` bounds the route for smokes (`None` runs
/// the full sweep then exits); `wet` turns on the oiled floor's
/// reflection (the scene's own ground roles).
pub fn run(
    frames: Option<u64>,
    wet: bool,
    present: PresentModePreference,
    validation: ValidationPreference,
) -> Result<(), String> {
    let mut app = ShellApp::new(ShellConfig {
        title: "The Garage (the P4 second scene)".into(),
        width: 1280,
        height: 720,
        target_fps: FPS,
        driver: DriverPreference::Real,
    })?;
    let mut renderer = Renderer::new(
        app.window(),
        RenderConfig {
            present_mode: present,
            validation,
            demo_scene: DemoScenePreference::Omitted,
            ..Default::default()
        },
    )
    .map_err(|e| e.to_string())?;
    // THE SECOND SCENE'S PROOF, leg 1 (the design's §7): the
    // garage content is attached through the SAME public
    // attach_scene the corner uses — no construction preference,
    // no renderer-internal content import, no corner type
    // anywhere in this module.
    let description = garage_scene_description();
    renderer
        .attach_scene(description.clone())
        .map_err(|e| e.to_string())?;
    let stats = renderer.stats();
    let size = stats.extent;
    println!(
        "The Garage | the P4 second scene | {} | Vulkan {} | {}x{} native | {} | present={} | \
         validation={} | debug_assertions={}",
        stats.gpu_name,
        stats.api_version,
        size.0,
        size.1,
        stats.surface_format,
        stats.present_mode,
        stats.validation_active,
        cfg!(debug_assertions)
    );
    if validation == ValidationPreference::On && !stats.validation_active {
        return Err("requested validation layer is unavailable".into());
    }
    println!(
        "garage: 3 materials (the compiled family shape), {} mesh vertices, bounds \
         [{:?} .. {:?}], 6 mechanics + 1 van; shadows + sky + post always on{}",
        description.mesh.vertices.len(),
        description.bounds.min,
        description.bounds.max,
        if wet {
            ", wet floor (the oiled slab's reflection)"
        } else {
            ""
        }
    );
    let light = garage_light();
    let sky = garage_sky();
    let bias = garage_bias();
    let bounds = description.bounds;
    // The population's retained buffers (the corner demo's own
    // zero-allocation discipline): the placement is static, the
    // pose blocks re-evaluated per presented frame.
    let (npcs, vehicles) = garage_population();
    let phases = garage_phases();
    let mut poses: Vec<crate::corner_population::Block> = Vec::new();
    crate::corner_population::evaluate_population_poses(0, &phases, &mut poses);
    let pace = Duration::from_secs_f64(1. / f64::from(FPS));
    let mut warm: Option<engine_renderer::AllocationStats> = None;
    let mut presented: u64 = 0;
    let limit = frames.map(|n| n.clamp(1, 1_000_000));
    let mut input = engine_shell::InputSnapshot::default();
    loop {
        let frame_start = Instant::now();
        app.frame_into(&mut input);
        if input.quit_requested || input.keys_pressed.contains(&Keycode::Escape) {
            break;
        }
        if input.minimized {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        // The live window size (free-look's own tolerance: resize
        // exercises the recreation path, never an error here).
        let size = app.window().size_in_pixels();
        let aspect = if size.1 == 0 {
            16. / 9.
        } else {
            size.0 as f32 / size.1 as f32
        };
        // The deterministic clock: frame k shows the orbit's
        // vantage k and the population's pose t_k (the corner
        // route's own render-then-step discipline at garage
        // scale — the placement is static, only the poses
        // advance).
        let camera: RouteWaypoint = garage_camera(presented);
        crate::corner_population::evaluate_population_poses(presented, &phases, &mut poses);
        let population = ProxyPopulation {
            npc: &npcs,
            npc_poses: &poses,
            npc_material: GARAGE_NPC_MATERIAL,
            vehicle: &vehicles,
            vehicle_material: GARAGE_VEHICLE_MATERIAL,
            rain: &[],
            rain_material: 0,
        };
        let matrix = view_projection(camera, aspect);
        // THE SECOND SCENE'S PROOF, leg 2: the frame is ONE public
        // submission carrying the scene's whole declaration —
        // light, shadows over the authored bounds, the population,
        // the sky, the post chain, and (behind --wet) the floor's
        // reflection. No entry-point selection exists to migrate:
        // the submission IS the frame.
        let frame = renderer
            .submit_frame(engine_renderer::FrameSubmission {
                camera: engine_renderer::FrameCamera {
                    view_proj: matrix,
                    eye: camera.position,
                },
                output: engine_renderer::OutputTarget { window_size: size },
                layers: engine_renderer::DynamicLayers::default(),
                scene: Some(engine_renderer::SceneFrame {
                    light,
                    shadows: Some(engine_renderer::ShadowFrame { bias, bounds }),
                    population: Some(population),
                    sky: Some(sky),
                    post: true,
                    wet: wet.then_some(engine_renderer::WetFrame { ripple: None }),
                }),
            })
            .map_err(|e| e.to_string())?;
        if frame.deferred {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        if !frame.presented {
            return Err(format!("garage presentation failed: {frame:?}"));
        }
        if frame.geometry.proxy_draw_calls == 0 {
            return Err("the garage population drew no proxy calls".into());
        }
        presented += 1;
        if presented == 30 {
            warm = Some(renderer.allocations());
        }
        if let Some(limit) = limit
            && presented >= limit
        {
            break;
        }
        if presented >= GARAGE_ROUTE_FRAMES && limit.is_none() {
            break;
        }
        pace_until(frame_start + pace);
    }
    let stats = renderer.stats();
    if stats.validation_errors > 0 {
        return Err(format!(
            "{} validation errors: {:?}",
            stats.validation_errors, stats.recent_validation
        ));
    }
    println!(
        "garage: {presented} presented frames; validation warnings/errors {}/{}; swapchain \
         recreations {}",
        stats.validation_warnings, stats.validation_errors, stats.swapchain_recreations
    );
    match warm {
        Some(warm) if stats.swapchain_recreations == 0 => {
            if renderer.allocations() != warm {
                return Err(format!(
                    "garage presentation changed the warmed allocation set: {warm:?} -> {:?}",
                    renderer.allocations()
                ));
            }
            println!("warmed Vulkan allocations held across the session: {warm:?}");
        }
        _ => println!(
            "Vulkan allocations: {:?} (no warmed-constancy claim: the session resized or never \
             warmed)",
            renderer.allocations()
        ),
    }
    Ok(())
}
