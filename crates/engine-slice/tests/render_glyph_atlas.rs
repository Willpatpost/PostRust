//! Full-frame RGBA comparison against the legacy per-set-pixel geometry.
use engine_renderer::overlay::{
    AtlasText, OverlayVertex, build_overlay_vertices, push_rect_px, push_text_px,
};
use engine_renderer::{CapturedFrame, RenderConfig, Renderer, ValidationPreference};
use engine_shell::{DriverPreference, ShellApp, ShellConfig};

const DEBUG: &str = "Atlas A Wj/%\n  ? \u{e9} []";

fn capture(
    renderer: &mut Renderer,
    hud: &[OverlayVertex],
    debug: Option<&str>,
    size: (u32, u32),
) -> CapturedFrame {
    renderer.request_frame_capture().unwrap();
    let frame = renderer
        .render_layers(&[[0.0; 4]; 4], &[], None, hud, debug, size)
        .unwrap();
    assert!(frame.presented);
    assert_eq!(frame.geometry.overlay_omitted, 0);
    let image = renderer.take_frame_capture().unwrap().unwrap();
    assert_eq!((image.width, image.height), size);
    image
}

// Independently captured from the original solid GLSL pipeline at 4b87175,
// B8G8R8A8_UNORM on lavapipe. Assign a palette index within two UNORM
// steps per channel (blend rounding), then pin its position at every pixel.
// The live A/B below additionally compares all four channels exactly.
fn assert_legacy_fixture(frame: &CapturedFrame) {
    const COLORS: [[u8; 4]; 8] = [
        [19, 51, 103, 166],
        [5, 5, 10, 255],
        [160, 225, 184, 217],
        [130, 70, 70, 128],
        [71, 94, 37, 191],
        [43, 89, 45, 191],
        [38, 255, 102, 255],
        [192, 20, 194, 191],
    ];
    let mut hash = 0xcbf29ce484222325_u64;
    for y in 0..240 {
        for x in 0..640 {
            let pixel = frame.pixel_rgba(x, y);
            let index = COLORS
                .iter()
                .position(|color| color.iter().zip(pixel).all(|(a, b)| a.abs_diff(b) <= 2))
                .unwrap_or_else(|| panic!("unexpected legacy color at ({x},{y}): {pixel:?}"));
            hash = (hash ^ index as u64).wrapping_mul(0x100000001b3);
        }
    }
    assert_eq!(hash, 0xcc1edbcb75aa4101, "original glyph/panel coverage");
}

#[test]
#[ignore = "requires a Vulkan drawable and the Vulkan validation layer"]
fn atlas_matches_legacy_rgba_and_reuses_init_allocations() {
    let mut app = ShellApp::new(ShellConfig {
        title: "Cycle 6-A glyph atlas".into(),
        width: 640,
        height: 240,
        target_fps: 60,
        driver: DriverPreference::Real,
    })
    .unwrap();
    let mut renderer = Renderer::new(
        app.window(),
        RenderConfig {
            validation: ValidationPreference::On,
            replace_pipelines: true,
            ..Default::default()
        },
    )
    .unwrap();
    let size = app.window().size_in_pixels();
    assert!(size.0 >= 640 && size.1 >= 240);
    let init = renderer.allocations();
    assert_eq!(
        (
            init.texture_memory,
            init.descriptor_pools,
            init.descriptor_sets,
            init.samplers
        ),
        (1, 1, 1, 1)
    );
    assert!(renderer.stats().validation_active);
    let _ = app.frame();

    let mut background = Vec::new();
    // A translucent panel makes an erroneous zero-alpha write in an unlit
    // texel observable, even when its RGB matches the background.
    push_rect_px(
        &mut background,
        size,
        [0., 0., 590., 215.],
        [0.1, 0.3, 0.6, 0.65],
    );
    let mut legacy = background.clone();
    let mut atlas = background.clone();
    let mut font = AtlasText::default();
    let printable: String = (32u8..127)
        .map(char::from)
        .enumerate()
        .flat_map(|(i, ch)| {
            (i != 0 && i % 32 == 0)
                .then_some('\n')
                .into_iter()
                .chain([ch])
        })
        .collect();
    for (pos, scale, text, color) in [
        (
            [15.125, 65.25],
            1.5,
            printable.as_str(),
            [0.95, 0.35, 0.15, 0.5],
        ),
        ([-3., 162.], 1., "j/r _\u{e9}\t", [0.15, 1.0, 0.4, 1.0]),
        ([620., 225.], 3., "MZ", [1.0, 0.1, 1.0, 0.75]),
    ] {
        push_text_px(&mut legacy, size, pos, scale, text, color);
        font.push_text_px(&mut atlas, size, pos, scale, text, color);
    }
    // A rectangle after text pins painter's order within a mixed HUD.
    for hud in [&mut legacy, &mut atlas] {
        push_rect_px(hud, size, [57., 67., 25., 8.], [0.2, 0.4, 0.1, 0.75]);
    }
    legacy.extend(build_overlay_vertices(DEBUG, size, [0.72, 1.0, 0.78, 0.85]));
    let base = capture(&mut renderer, &background, None, size);
    let reference = capture(&mut renderer, &legacy, None, size);
    assert_legacy_fixture(&reference);
    let compare = |image: &CapturedFrame| {
        assert_eq!(reference.format, image.format);
        let mut lit = 0;
        let mut mismatches = 0;
        let mut first = None;
        for y in 0..size.1 {
            for x in 0..size.0 {
                let expected = reference.pixel_rgba(x, y);
                let actual = image.pixel_rgba(x, y);
                lit += usize::from(expected != base.pixel_rgba(x, y));
                if expected != actual {
                    mismatches += 1;
                    first.get_or_insert((x, y, expected, actual));
                }
            }
        }
        assert!(lit > 4000, "visible reference ink: {lit}");
        assert_ne!(
            reference.pixel_rgba(20, 20),
            reference.pixel_rgba(610, 220),
            "panel control"
        );
        assert_eq!(mismatches, 0, "first RGBA mismatch {first:?}");
        println!(
            "{}x{}: {} changed reference pixels, {mismatches} RGBA mismatches",
            size.0, size.1, lit
        );
    };
    compare(&capture(&mut renderer, &atlas, Some(DEBUG), size));
    // Warm both independent upload slots before counting identical frames.
    compare(&capture(&mut renderer, &atlas, Some(DEBUG), size));
    let warm = renderer.allocations();
    for _ in 0..4 {
        compare(&capture(&mut renderer, &atlas, Some(DEBUG), size));
        assert_eq!(renderer.allocations(), warm);
    }
    let replacement = renderer
        .render_frame(&[[0.; 4]; 4], &[], None, (size.0 + 8, size.1 + 8))
        .unwrap();
    assert!(replacement.recreated);
    compare(&capture(&mut renderer, &atlas, Some(DEBUG), size));
    let after = renderer.allocations();
    assert_eq!(
        (
            after.texture_memory,
            after.descriptor_pools,
            after.descriptor_sets,
            after.samplers
        ),
        (1, 1, 1, 1)
    );
    assert_eq!(
        renderer.stats().validation_errors,
        0,
        "{:?}",
        renderer.stats().recent_validation
    );
    println!("Vulkan allocations: init {init:?}; steady {warm:?}; after replacement {after:?}");
}
