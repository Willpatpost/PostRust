//! THE P4 SECOND-SCENE GATE (the design's §7, leg 2 — "the gate is
//! the teeth"): The Garage — engine-slice's authored workshop
//! interior — rendered ENTIRELY through the public API
//! (`Renderer::attach_scene` + `Renderer::submit_frame`), with ZERO
//! corner types in the frame path (the description, the light, the
//! sky, the population, and the camera are all
//! [`crate::garage_scene`]'s own). The gate's legs:
//!
//! 1. THE COMPOSED PRESENTATION — the full authored stack (shadows +
//!    sky + post + the 6-mechanic population) presents through one
//!    submission, validation clean.
//! 2. DOUBLE-RUN BYTE-EQUALITY (the A24 pattern) — the SAME
//!    submission, plain and composed, presents byte-identical
//!    captures.
//! 3. THE ALLOCATION CONSTANCY — the submission route's frames
//!    allocate nothing over a warmed renderer (the adapter is stack
//!    assembly; the second scene pays no generalization tax — the
//!    design's "generalization must pay its way on small scenes
//!    too", witnessed on a 20 m slab).
//! 4. THE INK DIFFERENTIALS — the wet floor (the scene's own
//!    GroundRoles: the oiled receiver re-drawn Schlick-blended over
//!    the mirrored walls), the lamp shadows, and the population each
//!    visibly move pixels at the same camera (each feature axis
//!    isolated, one submission apart).
//! 5. THE SHADOW-MAP KNOWN ANSWER — the depth-only pass's map, texel
//!    by texel against the CPU raycaster (`map_texel_depth`) over
//!    THE GARAGE'S OWN authored box table (the render_shadows
//!    discipline, parameterized by the second scene's content — the
//!    strongest constructible proof that the shadow machinery serves
//!    authored content it has never seen).
//!
//! `#[ignore]`d like the binary's other lavapipe gates: it requires
//! `SDL_VIDEODRIVER=offscreen` plus a software Vulkan ICD (lavapipe)
//! and the validation layer. Run explicitly:
//!
//! ```sh
//! SDL_VIDEODRIVER=offscreen cargo test -p engine-slice --test render_garage -- \
//!     --ignored --nocapture
//! ```

use engine_renderer::shadows::{cascade_for, map_texel_depth};
use engine_renderer::{
    DemoScenePreference, ProxyPopulation, RenderConfig, Renderer, ValidationPreference, WetFrame,
};
use engine_shell::{DriverPreference, ShellApp, ShellConfig};
use engine_slice::corner_demo::view_projection;
use engine_slice::corner_population::Block;
use engine_slice::garage_scene::{
    GARAGE_NPC_MATERIAL, GARAGE_VEHICLE_MATERIAL, garage_bias, garage_camera, garage_light,
    garage_masses, garage_phases, garage_population, garage_scene_description, garage_sky,
};

const SIZE: (u32, u32) = (512, 512);

fn sdl_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}

/// One garage frame's submission — the second scene's own frame
/// declaration, built from [`crate::garage_scene`]'s authored pieces
/// exactly as the demo route builds it. The feature axes are the
/// caller's (the differentials toggle one at a time).
#[allow(clippy::too_many_arguments)]
fn garage_submission<'a>(
    matrix: &[[f32; 4]; 4],
    eye: [f32; 3],
    shadows: bool,
    population: Option<ProxyPopulation<'a>>,
    sky: bool,
    post: bool,
    wet: bool,
    size: (u32, u32),
) -> engine_renderer::FrameSubmission<'a> {
    engine_renderer::FrameSubmission {
        camera: engine_renderer::FrameCamera {
            view_proj: *matrix,
            eye,
        },
        output: engine_renderer::OutputTarget { window_size: size },
        layers: engine_renderer::DynamicLayers::default(),
        scene: Some(engine_renderer::SceneFrame {
            light: garage_light(),
            shadows: shadows.then(|| engine_renderer::ShadowFrame {
                bias: garage_bias(),
                bounds: engine_slice::garage_scene::garage_bounds(),
            }),
            population,
            sky: sky.then(garage_sky),
            post,
            wet: wet.then_some(WetFrame { ripple: None }),
        }),
    }
}

/// The differing-pixel census (the ink-discipline counter).
fn count_differing_pixels(a: &[u8], b: &[u8]) -> usize {
    a.chunks(4).zip(b.chunks(4)).filter(|(x, y)| x != y).count()
}

#[test]
#[ignore = "requires SDL_VIDEODRIVER=offscreen and a software Vulkan ICD (lavapipe); the dev pod needs the user-local ICD setup (see render_overflow)"]
fn the_garage_renders_through_the_public_api_with_its_own_known_answers() {
    let _guard = sdl_lock();
    let mut app = ShellApp::new(ShellConfig {
        title: "the P4 second scene".into(),
        width: SIZE.0,
        height: SIZE.1,
        target_fps: 60,
        driver: DriverPreference::Real,
    })
    .unwrap();
    let mut renderer = Renderer::new(
        app.window(),
        RenderConfig {
            // THE SECOND SCENE'S PROOF, leg 1: no construction
            // preference, no corner content — the garage attaches
            // through the same public API every consumer uses.
            demo_scene: DemoScenePreference::Omitted,
            validation: ValidationPreference::On,
            ..Default::default()
        },
    )
    .unwrap();
    renderer.attach_scene(garage_scene_description()).unwrap();
    let size = app.window().size_in_pixels();
    assert_eq!(size, SIZE);
    assert!(renderer.stats().validation_active);
    let _ = app.frame();

    // The authored camera (frame 60 of the scripted sweep — mid-arc,
    // the workshop's interior filling the frame) and the population's
    // retained pose buffer (frame 60's deterministic clock).
    let camera = garage_camera(60);
    let matrix = view_projection(camera, size.0 as f32 / size.1 as f32);
    let (npcs, vehicles) = garage_population();
    let phases = garage_phases();
    let mut poses: Vec<Block> = Vec::new();
    engine_slice::corner_population::evaluate_population_poses(60, &phases, &mut poses);
    let population = ProxyPopulation {
        instance_generation: 0,
        pose_generation: 0,
        npc: &npcs,
        npc_poses: &poses,
        npc_material: GARAGE_NPC_MATERIAL,
        vehicle: &vehicles,
        vehicle_material: GARAGE_VEHICLE_MATERIAL,
        rain: &[],
        rain_material: 0,
    };

    // ---- LEG 1 + 2: the composed frame, double-run ----
    // TWO warm composed frames (the rig-caught corrective):
    // FRAMES_IN_FLIGHT is 2 and the second slot's lazily-created
    // per-slot resources (their device memories) land on the SECOND
    // submission — warming one frame left the constancy leg reading
    // that one-time slot warmup as a per-frame allocation (+1
    // device_memory on the rig). The corner batteries warm past both
    // slots for the same reason; the one-time post/sky/wet/shadow
    // lazy targets build HERE, never inside a compared capture.
    let composed = garage_submission(
        &matrix,
        camera.position,
        true,
        Some(population),
        true,
        true,
        true,
        size,
    );
    assert!(renderer.submit_frame(composed).unwrap().presented);
    assert!(renderer.submit_frame(composed).unwrap().presented);
    // The capture machinery's destination buffer is ALSO a one-time
    // allocation (render_post/render_sky's "this test's first capture"
    // accounting — a mapped buffer + its device memory): LEG 3's
    // compared frames are this renderer's first captures, so the
    // destination must be warmed here too or the constancy leg reads
    // the capture machinery as a per-frame allocation (the +1
    // device_memory the rig caught).
    renderer.request_frame_capture().unwrap();
    assert!(renderer.submit_frame(composed).unwrap().presented);
    let _ = renderer.take_frame_capture().unwrap().unwrap();
    let warm = renderer.allocations();
    renderer.request_frame_capture().unwrap();
    assert!(renderer.submit_frame(composed).unwrap().presented);
    let composed_a = renderer.take_frame_capture().unwrap().unwrap();
    renderer.request_frame_capture().unwrap();
    assert!(renderer.submit_frame(composed).unwrap().presented);
    let composed_b = renderer.take_frame_capture().unwrap().unwrap();
    assert_eq!(
        composed_a.bytes, composed_b.bytes,
        "LEG 2 (composed): the same submission double-runs byte-identical"
    );
    assert_eq!(
        renderer.allocations(),
        warm,
        "LEG 3: the submission route's composed frames allocate nothing over the \
         warmed renderer (no generalization tax on the small scene)"
    );

    // The plain frame, double-run (the all-off control).
    let plain = garage_submission(
        &matrix,
        camera.position,
        false,
        None,
        false,
        false,
        false,
        size,
    );
    renderer.request_frame_capture().unwrap();
    assert!(renderer.submit_frame(plain).unwrap().presented);
    let plain_a = renderer.take_frame_capture().unwrap().unwrap();
    renderer.request_frame_capture().unwrap();
    assert!(renderer.submit_frame(plain).unwrap().presented);
    let plain_b = renderer.take_frame_capture().unwrap().unwrap();
    assert_eq!(
        plain_a.bytes, plain_b.bytes,
        "LEG 2 (plain): the same submission double-runs byte-identical"
    );

    // ---- LEG 4: the ink differentials (one feature axis at a time) ----
    let mut capture = |submission: engine_renderer::FrameSubmission<'_>| -> Vec<u8> {
        renderer.request_frame_capture().unwrap();
        assert!(renderer.submit_frame(submission).unwrap().presented);
        renderer.take_frame_capture().unwrap().unwrap().bytes
    };
    // The population's silhouettes (composed minus population).
    let unpopulated = capture(garage_submission(
        &matrix,
        camera.position,
        true,
        None,
        true,
        true,
        true,
        size,
    ));
    let population_ink = count_differing_pixels(&composed_a.bytes, &unpopulated);
    assert!(
        population_ink > 200,
        "the 6 mechanics + van visibly draw ({population_ink} differing pixels)"
    );
    // The lamp shadows (composed minus shadows).
    let unshadowed = capture(garage_submission(
        &matrix,
        camera.position,
        false,
        Some(population),
        true,
        true,
        true,
        size,
    ));
    let shadow_ink = count_differing_pixels(&composed_a.bytes, &unshadowed);
    assert!(
        shadow_ink > 500,
        "the work lamp's casts visibly draw ({shadow_ink} differing pixels — the walls' \
         and crates' shadows across the floor)"
    );
    // The wet floor (composed minus wet) — the scene's own GroundRoles.
    let dry = capture(garage_submission(
        &matrix,
        camera.position,
        true,
        Some(population),
        true,
        true,
        false,
        size,
    ));
    let wet_ink = count_differing_pixels(&composed_a.bytes, &dry);
    assert!(
        wet_ink > 500,
        "the oiled floor's reflection visibly draws ({wet_ink} differing pixels — the \
         mirrored walls and crates over the receiver range)"
    );
    // The composed frame is living composed content, not two
    // accidental plains (the ink floor against the plain control).
    let composed_ink = count_differing_pixels(&composed_a.bytes, &plain_a.bytes);
    assert!(
        composed_ink > 10_000,
        "the composed frame visibly differs from the plain one ({composed_ink} differing \
         pixels — every feature axis contributes)"
    );

    // ---- LEG 5: the shadow-map known answer over the garage's own boxes ----
    // The CPU cascade over the AUTHORED bounds (the same computation
    // the submission's ShadowFrame drives), then every texel against
    // the raycaster over the AUTHORED box table — clear texels exact,
    // interior texels exact within the render_shadows tolerance, the
    // edge band (rasterization coverage at box perimeters)
    // skipped-and-counted as in the corner gate.
    renderer.request_shadow_map_capture().unwrap();
    assert!(
        renderer
            .submit_frame(garage_submission(
                &matrix,
                camera.position,
                true,
                None,
                false,
                false,
                false,
                size,
            ))
            .unwrap()
            .presented
    );
    let map = renderer
        .take_shadow_map_capture()
        .unwrap()
        .expect("the shadow readback completed");
    let bounds = engine_slice::garage_scene::garage_bounds();
    let cascade = cascade_for(garage_light().dir_toward_light, bounds.min, bounds.max);
    let boxes = garage_masses();
    let mut compared = 0usize;
    let mut skipped = 0usize;
    let mut mismatches = 0usize;
    for y in 0..map.extent {
        for x in 0..map.extent {
            let expected = map_texel_depth(&cascade, boxes, x, y, map.extent);
            let gpu = map.depths[(y * map.extent + x) as usize];
            let Some(hit) = expected.hit else {
                // Clear texel: no surface at all — exact.
                if !gpu.is_finite() || (gpu - 1.0).abs() > 1e-4 {
                    mismatches += 1;
                }
                compared += 1;
                continue;
            };
            // The edge margin: skip texels whose hit sits within
            // 0.1 m of its surface's perimeter (the corner gate's
            // implementation-territory band; the garage's faces are
            // metres wide, so the interior dominates — the margin is
            // world-metres, not texel-scaled).
            // The edge margin, the corner gate's Finding-14 form (the
            // rig-caught corrective): infer the FACE axis (the axis
            // whose bound the hit touches — distance ~0 by
            // definition) and measure the perimeter on the two
            // TANGENTIAL axes only. The lane's first form measured
            // all three axes, so every face hit carried its own
            // face-axis zero and landed in the skip band — zero
            // texels compared.
            let mut on_face = 0;
            let mut tangential = [true; 3];
            let mut min_edge = f32::MAX;
            match expected.box_index {
                Some(index) => {
                    let box_ = &boxes[index];
                    for axis in 0..3 {
                        if (box_.min[axis] - hit[axis]).abs() < 1e-3
                            || (box_.max[axis] - hit[axis]).abs() < 1e-3
                        {
                            tangential[axis] = false;
                            on_face += 1;
                        }
                    }
                    if on_face >= 2 {
                        // A corner-edge graze (two bounds touched — a
                        // roof/wall edge): genuinely ambiguous
                        // coverage. Implementation territory — skip.
                        skipped += 1;
                        continue;
                    }
                    for axis in 0..3 {
                        if !tangential[axis] {
                            continue;
                        }
                        let lo = (box_.min[axis] - hit[axis]).abs();
                        let hi = (box_.max[axis] - hit[axis]).abs();
                        min_edge = min_edge.min(lo).min(hi);
                    }
                    if min_edge == f32::MAX {
                        // Interior to all three axes (the ray entered
                        // through a face it has left behind): the
                        // conservative full-margin form.
                        for (axis, hit_axis) in hit.iter().enumerate() {
                            let lo = (box_.min[axis] - hit_axis).abs();
                            let hi = (box_.max[axis] - hit_axis).abs();
                            min_edge = min_edge.min(lo).min(hi);
                        }
                    }
                }
                None => {
                    // The ground quad: no perimeter in the BOX table,
                    // but the drawn floor IS a bounded quad — the
                    // mesh's plate spans ±GARAGE_EXTENT on X/Z at
                    // y = GROUND_Y (mesh_from_boxes' added receiver
                    // quad). The corner gate's plate-perimeter logic
                    // on the tangential axes (X/Z) — plus the case
                    // the corner's oversized plate never faces: the
                    // garage's plate EQUALS the cascade AABB, so the
                    // ray shear lands some ground hits BEYOND the
                    // drawn floor — no geometry exists there, the GPU
                    // texel is clear, and it compares as a clear
                    // texel rather than against the phantom ground
                    // depth.
                    let plate = engine_slice::garage_scene::GARAGE_EXTENT;
                    for &axis in &[0, 2] {
                        let lo = (-plate - hit[axis]).abs();
                        let hi = (plate - hit[axis]).abs();
                        min_edge = min_edge.min(lo).min(hi);
                    }
                }
            }
            if min_edge < 0.1 {
                skipped += 1;
                continue;
            }
            if expected.box_index.is_none()
                && (hit[0].abs() > engine_slice::garage_scene::GARAGE_EXTENT
                    || hit[2].abs() > engine_slice::garage_scene::GARAGE_EXTENT)
            {
                // A ground hit beyond the drawn floor plate: the
                // clear-texel comparison (the perimeter band above
                // already absorbed the transition).
                if !gpu.is_finite() || (gpu - 1.0).abs() > 1e-4 {
                    mismatches += 1;
                }
                compared += 1;
                continue;
            }
            if !gpu.is_finite() || (gpu - expected.depth).abs() > 1e-3 {
                mismatches += 1;
            }
            compared += 1;
        }
    }
    assert_eq!(
        mismatches, 0,
        "every compared garage shadow texel matches the CPU raycaster ({compared} compared, \
         {skipped} edge-band skips)"
    );
    assert!(
        compared > 100_000,
        "the garage's map is mostly interior texels ({compared} compared of {} total)",
        map.depths.len()
    );

    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "{:?}",
        renderer.stats().recent_validation
    );
    println!(
        "the garage through the public API: composed {composed_ink} differing pixels from the \
         plain control (population {population_ink}, shadows {shadow_ink}, wet floor \
         {wet_ink}); the shadow map {compared} texel-for-texel vs the CPU raycaster over \
         the authored boxes ({skipped} edge skips); double-run byte-identical plain and \
         composed; allocations constant",
    );
}
