//! Compare real, textured surfaces through the production corner pipeline.
use engine_renderer::lit_scene::{
    LightParams, day_preset, lit_surface_color, materials, night_preset,
};
use engine_renderer::proc_texture::{TEXTURE_SIZE, procedural_texture};
use engine_renderer::{DemoScenePreference, RenderConfig, Renderer, ValidationPreference};
use engine_shell::{DriverPreference, ShellApp, ShellConfig};

fn check_patch(renderer: &mut Renderer, size: (u32, u32), material: usize, light: LightParams) {
    // Orthographic 2 m square on the ground or on the +Z facade at z=-6.
    // The building crops lie fully within the SW concrete and SE stone faces.
    let cx = [0., -12., 12.][material];
    let cy = 4.;
    let ground = material == 0;
    let (matrix, eye, normal) = if ground {
        (
            [
                [1., 0., 0., 0.],
                [0., 0., -0.125, 0.],
                [0., -1., 0., 0.],
                [0., 0., 0.5, 1.],
            ],
            [0., 1.7, 0.],
            [0., 1., 0.],
        )
    } else {
        (
            [
                [1., 0., 0., 0.],
                [0., 1., 0., 0.],
                [0., 0., -0.0625, 0.],
                [-cx, -cy, 0.25, 1.],
            ],
            [cx, cy, 4.],
            [0., 0., 1.],
        )
    };
    renderer.request_frame_capture().unwrap();
    assert!(
        renderer
            .render_corner(&matrix, eye, light, size)
            .unwrap()
            .presented
    );
    let frame = renderer.take_frame_capture().unwrap().unwrap();
    assert_eq!((frame.width, frame.height), size);
    assert!(matches!(
        renderer.stats().surface_format.as_str(),
        "B8G8R8A8_UNORM" | "R8G8B8A8_UNORM"
    ));
    let mat = materials()[material];
    let texture = procedural_texture(mat.texture);
    let mut largest_error = 0;
    for y in 8..size.1 - 8 {
        for x in 8..size.0 - 8 {
            let dx = (x as f32 + 0.5) * 2. / size.0 as f32 - 1.;
            let dy = (y as f32 + 0.5) * 2. / size.1 as f32 - 1.;
            let pos = if ground {
                [dx, 0., dy]
            } else {
                [cx + dx, cy - dy, -6.]
            };
            let uv_axes = [pos[0], if ground { pos[2] } else { pos[1] }];
            let [tx, ty] =
                uv_axes.map(|m| ((m / 4.).rem_euclid(1.) * TEXTURE_SIZE as f32).floor() as usize);
            let index = 4 * (ty * TEXTURE_SIZE as usize + tx);
            let albedo = std::array::from_fn(|c| texture[index + c] as f32 / 255.);
            let view = std::array::from_fn(|i| eye[i] - pos[i]);
            let expected = lit_surface_color(albedo, normal, view, mat, light)
                .map(|c| (255. * c.clamp(0., 1.)).round() as u8);
            let pixel = frame.pixel_rgba(x, y);
            // Two 8-bit UNORM codes per RGB channel cover shader FP
            // reassociation and final attachment conversion; alpha is exact.
            let error = expected
                .iter()
                .zip(pixel)
                .map(|(e, a)| e.abs_diff(a))
                .max()
                .unwrap();
            largest_error = largest_error.max(error);
            assert!(
                error <= 2,
                "material {material} pixel ({x},{y}) world {pos:?}: expected {expected:?}, got {pixel:?}"
            );
            assert_eq!(pixel[3], 255);
        }
    }
    println!(
        "material {material}, light {:?}, 240x240 framebuffer pixels: max RGB error {largest_error}/255",
        light.dir_toward_light
    );
}

#[test]
#[ignore = "requires a Vulkan drawable and validation layer"]
fn corner_pixels_match_oracle_and_warmed_route_reuses_allocations() {
    let mut app = ShellApp::new(ShellConfig {
        title: "One Corner of the City".into(),
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
    // The directional fixture generates an off-axis, unsaturated specular
    // highlight. Both actual presets also exercise diffuse and ambient-only.
    let highlight = LightParams {
        dir_toward_light: [0.2, 1., 0.1],
        light_color: [0.6, 0.5, 0.7],
        ambient: [0.08, 0.09, 0.1],
    };
    for light in [highlight, day_preset(), night_preset()] {
        for material in 0..3 {
            check_patch(&mut renderer, size, material, light);
        }
    }
    let warm = renderer.allocations();
    assert_eq!(
        (
            warm.texture_memory,
            warm.descriptor_pools,
            warm.descriptor_sets,
            warm.samplers
        ),
        (4, 2, 4, 2)
    );
    for pose in engine_slice::corner_demo::route_poses() {
        let matrix = engine_slice::corner_demo::view_projection(pose, 1.);
        assert!(
            renderer
                .render_corner(&matrix, pose.position, day_preset(), size)
                .unwrap()
                .presented
        );
        assert_eq!(renderer.allocations(), warm);
    }
    // Force replacement of the render pass and lit pipeline while retaining
    // all static textures/sets, then repeat the oracle on the replacement.
    let replacement = renderer
        .render_corner(
            &[[0.; 4]; 4],
            [0.; 3],
            night_preset(),
            (size.0 + 8, size.1 + 8),
        )
        .unwrap();
    assert!(replacement.recreated);
    check_patch(&mut renderer, size, 1, night_preset());
    let after = renderer.allocations();
    assert_eq!(
        (
            after.texture_memory,
            after.descriptor_pools,
            after.descriptor_sets,
            after.samplers
        ),
        (4, 2, 4, 2)
    );
    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "{:?}",
        renderer.stats().recent_validation
    );
    println!("601 warmed presentations: {warm:?}; after pipeline replacement: {after:?}");
}
