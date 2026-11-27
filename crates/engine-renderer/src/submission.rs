//! The scene-independent frame submission contract (P4 D1, stage S1).
//!
//! One typed, borrow-only struct — [`FrameSubmission`] — carries
//! everything one frame needs: the camera, the output size, the
//! dynamic layer batches (world/foreground/HUD/debug text), and the
//! optional scene half ([`SceneFrame`]) declaring this frame's pass
//! requirements (light, shadows, population, sky, post, wet). No
//! method-per-feature-combination exists: a pass runs when its field
//! is set — the one-Option-per-layer shape the internal frame flow
//! already consumes, now named by one struct instead of positional
//! parameters.
//!
//! **The contract invariant** (the safe layer's existing validation
//! discipline, made the contract's own): a submission is validated
//! WHOLE, before any frame state is touched. A refused submission
//! renders nothing, touches no synchronization state, and leaves
//! every sticky setting and capture request intact. The order is the
//! per-feature entries' own: scene/preference coherence (checked by
//! [`crate::Renderer::submit_frame`]), then material indices, then
//! the instance repack, the pose-index-vs-blocks check, and the pose
//! repack.
//!
//! **Zero-cost statement:** every type here is a borrow bundle —
//! `Copy`, no `Vec`, no `Box`, no trait objects. The adapter
//! ([`adapt_frame`]) is stack assembly the compiler can elide
//! entirely; it introduces no per-frame heap allocation anywhere.
//!
//! **Stage S1 scope (additive):** the four `render_corner*` methods
//! and `render_layers` keep their routes UNCHANGED —
//! [`crate::Renderer::submit_frame`] is a new route that maps the
//! submission onto the SAME internals through [`adapt_frame`]. The
//! sky/post/wet state is PER FRAME here (the contract's shape) while
//! the old routes keep their sticky setters; S3 migrates the
//! stickies into per-frame fields and S6 removes the old routes. The
//! two renderer-level policies the adapter still reads — the proxy
//! submission mode and the TEST-ONLY ripple debug lane — ride
//! [`SubmissionPolicy`] until their own migration steps.

use engine_core::ripple::RippleField;

use crate::lit_scene::LightParams;
use crate::overlay::OverlayVertex;
use crate::renderer::{ProxyDrawMode, ProxyPopulation, RippleDebugMode, SceneLayer, SkyFrame};
use crate::scene::SceneVertex;
use crate::shadows::ShadowBias;
use crate::vulkan::{self, RendererError};

/// Everything one frame needs, scene-independent. Validated whole,
/// BEFORE any frame state is touched (the capture-refusal
/// discipline) — see the module docs for the invariant.
#[derive(Debug, Clone, Copy)]
pub struct FrameSubmission<'a> {
    /// The one main camera (the foreground layer's independent
    /// projection is the second camera — the existing two-camera
    /// shape, no speculative multi-viewport).
    pub camera: FrameCamera,
    /// The frame's output: the current window size in pixels.
    pub output: OutputTarget,
    /// The dynamic layer batches (the `render_layers` path's inputs).
    pub layers: DynamicLayers<'a>,
    /// This frame's retained-scene requirements — `None` is the
    /// layers-only shape (every corner-family pass off); `Some(_)`
    /// declares which scene passes run (see [`SceneFrame`]).
    pub scene: Option<SceneFrame<'a>>,
}

/// The frame's main camera: the view-projection matrix and the world
/// eye position (the corner family's own argument pair, unified).
#[derive(Debug, Clone, Copy)]
pub struct FrameCamera {
    pub view_proj: [[f32; 4]; 4],
    pub eye: [f32; 3],
}

/// The frame's output target: the current window size in pixels (the
/// application owns window state — the Stage 1 shell discipline; the
/// renderer never queries SDL after construction). A zero extent (a
/// minimized window) presents nothing — the frame flow's own
/// contract.
#[derive(Debug, Clone, Copy)]
pub struct OutputTarget {
    pub window_size: (u32, u32),
}

/// The frame's dynamic layer batches, in the fixed draw order the
/// frame flow already records: world and foreground share the scene
/// slot (world first), HUD and the built debug text share the
/// overlay slot (HUD first). At capacity, foreground takes precedence
/// over world geometry and HUD over diagnostics — `render_layers`'s
/// documented policy, unchanged.
#[derive(Debug, Clone, Copy, Default)]
pub struct DynamicLayers<'a> {
    /// The world batch (the `render_layers` path's region geometry;
    /// an empty slice draws nothing).
    pub world: &'a [SceneVertex],
    /// The optional foreground batch with its own projection (kept:
    /// the independent-depth second camera).
    pub foreground: Option<SceneLayer<'a>>,
    /// The HUD overlay vertices.
    pub hud: &'a [OverlayVertex],
    /// The debug text (built into the retained overlay buffer exactly
    /// like `render_layers`'s path; `None` clears it).
    pub debug_text: Option<&'a str>,
}

/// What the attached scene contributes THIS FRAME — the
/// pass-requirements declaration. Each field names a pass and its
/// data; `None`/`false` means the pass does not run. No
/// method-per-combination can exist (the contract's point).
#[derive(Debug, Clone, Copy)]
pub struct SceneFrame<'a> {
    /// The directional light (shared by the lit, shadowed, and wet
    /// passes).
    pub light: LightParams,
    /// The shadow pass: the compare-time bias plus the scene bounds
    /// the cascade is computed over (the corner route's value —
    /// `shadows::scene_bounds(lit_scene::intersection_boxes())` —
    /// until S2's scene handle authors it).
    pub shadows: Option<ShadowFrame>,
    /// The proxy population (the existing typed struct, verbatim:
    /// npc/vehicle/rain kinds, the npc pose blocks, per-kind material
    /// indices).
    pub population: Option<ProxyPopulation<'a>>,
    /// The sky pass (per frame here; the sticky `set_sky` setting fed
    /// the old routes).
    pub sky: Option<SkyFrame>,
    /// Whether the frame renders through the HDR post chain (the
    /// sticky `set_post` setting fed the old routes).
    pub post: bool,
    /// The wet-ground reflection (the sticky `set_wet`/`
    /// set_ripple_field` settings fed the old routes; the camera and
    /// light are derived from the frame).
    pub wet: Option<WetFrame<'a>>,
}

/// The shadow pass's declaration: the compare-time bias parameters
/// (caller policy, shared verbatim by the shader and the CPU oracle)
/// and the scene bounds the single cascade covers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowFrame {
    pub bias: ShadowBias,
    pub bounds: SceneBounds,
}

/// A scene's world axis-aligned bounding box — the shadow cascade's
/// projection domain. Plain data: the scene author supplies it (the
/// corner route's value folds `lit_scene::intersection_boxes()`
/// through `shadows::scene_bounds`); S2's `SceneDescription` carries
/// it at attach time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// The wet-ground reflection's per-frame data: the ripple field (if
/// any). The reflection camera and light derive from the frame's own
/// camera and the scene's light — the caller supplies nothing
/// duplicated.
#[derive(Clone, Copy)]
pub struct WetFrame<'a> {
    /// The live ripple field (`None` is the rainless wet path — the
    /// empty uniform, byte-identical to Era-1's).
    pub ripple: Option<&'a RippleField>,
}

impl std::fmt::Debug for WetFrame<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The field is engine-core state without a Debug impl; the
        // presence bit is the submission-level fact.
        f.debug_struct("WetFrame")
            .field("ripple", &self.ripple.is_some())
            .finish()
    }
}

/// The renderer-level policy the S1 adapter still reads per frame:
/// the two settings that stay renderer-owned (set through
/// [`crate::Renderer::set_proxy_draw_mode`] and
/// [`crate::Renderer::set_ripple_debug_mode`]) until their migration
/// steps move them into the submission. `Copy` — collected from the
/// renderer before the mutable frame flow begins.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SubmissionPolicy {
    /// The proxy submission mode (the equivalence gate's axis — see
    /// [`ProxyDrawMode`]; the production default is `Instanced`).
    pub proxy_draw_mode: ProxyDrawMode,
    /// The TEST-ONLY ripple debug lane (see [`RippleDebugMode`]);
    /// `Off` in every production frame.
    pub ripple_debug: RippleDebugMode,
}

/// The input half of one `vulkan::render_frame` call, typed — the
/// parameter set the S1 adapter builds from a validated submission.
/// Field-for-field the existing per-feature entries' inline
/// arguments (the four owning states and the construction-time
/// config stay out: they are the renderer's own). This struct is the
/// equivalence proof's comparison target — the module test builds
/// `render_corner`'s parameter set by hand and asserts the adapter
/// reproduces it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FrameInputs<'x> {
    /// The current window size in pixels.
    pub window_size: (u32, u32),
    /// The main camera's view-projection.
    pub view_proj: [[f32; 4]; 4],
    /// The world batch.
    pub region_vertices: &'x [SceneVertex],
    /// The independent-projection foreground batch.
    pub foreground: Option<SceneLayer<'x>>,
    /// The HUD vertices.
    pub hud: &'x [OverlayVertex],
    /// The built debug-overlay vertices (the retained F-05 buffer's
    /// content for this frame; empty when no debug text was given).
    pub debug_overlay: &'x [OverlayVertex],
    /// The corner family's FrameParams — `Some` on every scene-backed
    /// frame (the ground re-draw needs the current camera in the
    /// corner buffer regardless of the lighting branch).
    pub corner_params: Option<vulkan::corner::FrameParams>,
    /// The repacked proxy population (validated and repacked whole
    /// before this struct exists).
    pub proxy: Option<vulkan::proxy_draw::ProxyFrameUpload<'x>>,
    /// The shadowed lit params (the extended FrameParams block plus
    /// the cascade matrix).
    pub shadowed: Option<vulkan::shadow_draw::ShadowFrameParams>,
    /// The sky pass's parameters.
    pub sky: Option<vulkan::sky_draw::SkyFrameParams>,
    /// Whether the frame renders through the HDR post chain.
    pub post: bool,
    /// The wet reflection's camera description (`None` keeps the dry
    /// path).
    pub wet: Option<vulkan::reflection_draw::WetCamera>,
}

/// Maps a submission onto the frame flow's input set — THE S1
/// ADAPTER. Device-free by construction (the repack buffers are
/// caller-owned; [`crate::Renderer::submit_frame`] passes its
/// retained F-05 buffers), so the mapping is unit-testable without a
/// GPU: the module test builds `render_corner`'s argument shape and
/// asserts the produced [`FrameInputs`] equals the parameter set that
/// entry constructs inline.
///
/// Validation order — the populated entries' own sequence, preserved
/// exactly: material indices vs the corner table, then the instance
/// repack, then the pose-index-vs-blocks check, then the pose repack.
/// Every refusal is a typed error that leaves both repack buffers
/// and all frame state untouched (an atomic repack, never a partial
/// buffer).
pub(crate) fn adapt_frame<'a, 'x>(
    policy: SubmissionPolicy,
    submission: &'x FrameSubmission<'a>,
    debug_overlay: &'x [OverlayVertex],
    instance_bytes: &'x mut Vec<u8>,
    pose_bytes: &'x mut Vec<u8>,
) -> Result<FrameInputs<'x>, RendererError> {
    let camera = submission.camera;
    let layers = &submission.layers;
    // The layers-only shape (`render_layers`'): every scene-half
    // input stays off — no corner params, no population, no shadows,
    // no sky, no post chain, no wet reflection.
    let Some(scene) = &submission.scene else {
        return Ok(FrameInputs {
            window_size: submission.output.window_size,
            view_proj: camera.view_proj,
            region_vertices: layers.world,
            foreground: layers.foreground,
            hud: layers.hud,
            debug_overlay,
            corner_params: None,
            proxy: None,
            shadowed: None,
            sky: None,
            post: false,
            wet: None,
        });
    };
    let light = scene.light;
    let eye = camera.eye;
    let view_proj = camera.view_proj;
    // The population: the populated entries' own validation +
    // repack sequence, verbatim. `None` skips every check exactly
    // like the unpopulated entries.
    let proxy = match &scene.population {
        Some(population) => {
            let material_table = crate::lit_scene::materials().len() as u32;
            if population.npc_material >= material_table
                || population.vehicle_material >= material_table
                || population.rain_material >= material_table
            {
                return Err(RendererError::Vulkan(format!(
                    "proxy material index outside the corner material table \
                     ({} materials, npc {}, vehicle {}, rain {})",
                    material_table,
                    population.npc_material,
                    population.vehicle_material,
                    population.rain_material
                )));
            }
            crate::proxy_instances::repack_populations(
                instance_bytes,
                population.npc,
                population.vehicle,
                population.rain,
            )
            .map_err(RendererError::ProxyPack)?;
            // The pose contract (the capture-refusal discipline):
            // every POPULATED npc's pose index must select a supplied
            // block; the block-count bound is the repack's own typed
            // refusal below.
            if population
                .npc
                .iter()
                .any(|instance| instance.pose_index as usize >= population.npc_poses.len())
            {
                return Err(RendererError::Vulkan(format!(
                    "a npc pose index falls outside the supplied pose blocks \
                     ({} blocks, highest index {})",
                    population.npc_poses.len(),
                    population
                        .npc
                        .iter()
                        .map(|instance| instance.pose_index)
                        .max()
                        .unwrap_or(0)
                )));
            }
            crate::animation::repack_poses(pose_bytes, population.npc_poses)
                .map_err(RendererError::PosePack)?;
            Some(vulkan::proxy_draw::ProxyFrameUpload {
                bytes: instance_bytes.as_slice(),
                pose_bytes: pose_bytes.as_slice(),
                npc_count: population.npc.len() as u32,
                vehicle_count: population.vehicle.len() as u32,
                rain_count: population.rain.len() as u32,
                npc_material: population.npc_material,
                vehicle_material: population.vehicle_material,
                rain_material: population.rain_material,
                mode: policy.proxy_draw_mode,
            })
        }
        None => None,
    };
    // The shadowed lit params (`render_corner_shadowed`'s own
    // construction, verbatim): the single cascade over the bounds the
    // submission declares, and the extended FrameParams block the
    // shadowed shader reads.
    let shadowed = scene.shadows.map(|shadows| {
        let cascade = crate::shadows::cascade_for(
            light.dir_toward_light,
            shadows.bounds.min,
            shadows.bounds.max,
        );
        let [r, g, b] = light.ambient;
        vulkan::shadow_draw::ShadowFrameParams {
            view_proj,
            light_dir: [
                light.dir_toward_light[0],
                light.dir_toward_light[1],
                light.dir_toward_light[2],
                eye[0],
            ],
            light_color: [
                light.light_color[0],
                light.light_color[1],
                light.light_color[2],
                eye[1],
            ],
            ambient: [r, g, b, eye[2]],
            light_view_proj: cascade.matrix,
            shadow_control: [
                crate::shadows::SHADOW_MAP_EXTENT as f32,
                shadows.bias.constant,
                shadows.bias.slope_scale,
                0.0,
            ],
        }
    });
    // The corner FrameParams — ALWAYS on a scene-backed frame (the
    // shadowed entries pass it too: the ground re-draw needs the
    // current camera in the corner buffer regardless of the lighting
    // branch).
    let corner_params = vulkan::corner::FrameParams::new(view_proj, eye, light);
    // The sky pass's parameters (`Renderer::sky_params`'s body,
    // verbatim: the CPU-computed inverse the ray reconstruction
    // consumes).
    let sky = scene.sky.map(|frame| {
        vulkan::sky_draw::SkyFrameParams::new(
            crate::sky::mat4_inverse(&view_proj),
            frame.palette,
            frame.disc_dir,
        )
    });
    // The wet reflection's camera description (`Renderer::wet_camera`'s
    // body): the true camera plus the packed ripple uniform — packed
    // HERE from the submission's field exactly as `set_ripple_field`
    // packs the sticky one (the same pure function, the same debug
    // lane).
    let wet = scene.wet.map(|wet| vulkan::reflection_draw::WetCamera {
        view_proj,
        eye,
        light,
        ripple: crate::renderer::pack_ripple_field(wet.ripple, policy.ripple_debug),
    });
    Ok(FrameInputs {
        window_size: submission.output.window_size,
        view_proj,
        region_vertices: layers.world,
        foreground: layers.foreground,
        hud: layers.hud,
        debug_overlay,
        corner_params: Some(corner_params),
        proxy,
        shadowed,
        sky,
        post: scene.post,
        wet,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::pack_ripple_field;

    /// THE S1 EQUIVALENCE PROOF (device-free, the type-level bar): a
    /// [`FrameSubmission`] shaped exactly like `render_corner`'s
    /// argument list — camera, light, window size, no population, no
    /// shadows, no dynamic layers — maps through [`adapt_frame`]
    /// onto the IDENTICAL parameter set `render_corner` constructs
    /// inline and hands `vulkan::render_frame`. Every expected value
    /// below is built from the SAME expressions that entry contains
    /// (cited per field); if either side drifts, this fails. Two
    /// configurations are proven: the all-off plain frame, and the
    /// sticky-on frame (sky + post + wet with a live ripple field —
    /// the settings the demo sets through `set_sky`/`set_post`/
    /// `set_wet`/`set_ripple_field`, which the entry reads from its
    /// sticky members and the submission carries per frame).
    #[test]
    fn plain_corner_submission_maps_to_render_corner_parameter_set() {
        // render_corner's argument shape (renderer.rs:621–627): a
        // view-projection, an eye, the day preset, a window size.
        let matrix = [
            [0.9, 0.0, 0.0, 0.0],
            [0.0, 0.9, 0.0, 0.0],
            [0.0, 0.0, 0.9, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let eye = [4.0, 1.7, 12.0];
        let light = crate::lit_scene::day_preset();
        let size = (1280_u32, 720_u32);
        // The renderer-level policy the entry reads from its own
        // fields: the production proxy mode, the shipped debug lane.
        let policy = SubmissionPolicy {
            proxy_draw_mode: ProxyDrawMode::default(),
            ripple_debug: RippleDebugMode::default(),
        };

        // ---- Configuration 1: the plain all-off frame ----
        let submission = FrameSubmission {
            camera: FrameCamera {
                view_proj: matrix,
                eye,
            },
            output: OutputTarget { window_size: size },
            layers: DynamicLayers::default(),
            scene: Some(SceneFrame {
                light,
                shadows: None,
                population: None,
                sky: None,
                post: false,
                wet: None,
            }),
        };
        let mut instance_bytes = Vec::new();
        let mut pose_bytes = Vec::new();
        let inputs = adapt_frame(
            policy,
            &submission,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
        )
        .expect("the plain corner submission validates");
        // render_corner's own construction (renderer.rs:633–656): the
        // corner FrameParams, empty dynamic layers, and — every
        // sticky off — no sky, no post, no wet camera.
        let expected = FrameInputs {
            window_size: size,
            view_proj: matrix,
            region_vertices: &[],
            foreground: None,
            hud: &[],
            debug_overlay: &[],
            corner_params: Some(vulkan::corner::FrameParams::new(matrix, eye, light)),
            proxy: None,
            shadowed: None,
            sky: None,
            post: false,
            wet: None,
        };
        assert_eq!(
            inputs, expected,
            "the plain corner frame's full parameter set"
        );

        // ---- Configuration 2: sky + post + wet on ----
        // The sticky-on frame: the entry's sky_params (renderer.rs
        // :1114–1122) and wet_camera (:1096–1108) read the sticky
        // members set through set_sky/set_post/set_wet/
        // set_ripple_field; the submission carries the same state per
        // frame. The expected values below replicate those bodies —
        // the same expressions, the same pure functions.
        let field = ripple_field();
        let sky = SkyFrame {
            palette: crate::sky::day_sky(),
            disc_dir: [-0.5, 0.707_106_77, -0.5],
        };
        let submission = FrameSubmission {
            camera: FrameCamera {
                view_proj: matrix,
                eye,
            },
            output: OutputTarget { window_size: size },
            layers: DynamicLayers::default(),
            scene: Some(SceneFrame {
                light,
                shadows: None,
                population: None,
                sky: Some(sky),
                post: true,
                wet: Some(WetFrame {
                    ripple: Some(&field),
                }),
            }),
        };
        let mut instance_bytes = Vec::new();
        let mut pose_bytes = Vec::new();
        let inputs = adapt_frame(
            policy,
            &submission,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
        )
        .expect("the sticky-on submission validates");
        let expected = FrameInputs {
            window_size: size,
            view_proj: matrix,
            region_vertices: &[],
            foreground: None,
            hud: &[],
            debug_overlay: &[],
            corner_params: Some(vulkan::corner::FrameParams::new(matrix, eye, light)),
            proxy: None,
            shadowed: None,
            // sky_params' body: SkyFrameParams::new over the
            // CPU-computed inverse, the palette, the disc direction.
            sky: Some(vulkan::sky_draw::SkyFrameParams::new(
                crate::sky::mat4_inverse(&matrix),
                sky.palette,
                sky.disc_dir,
            )),
            post: true,
            // wet_camera's body: the true camera and light plus the
            // ripple uniform packed exactly as set_ripple_field packs
            // it (renderer.rs:1077's own expression).
            wet: Some(vulkan::reflection_draw::WetCamera {
                view_proj: matrix,
                eye,
                light,
                ripple: pack_ripple_field(Some(&field), RippleDebugMode::Off),
            }),
        };
        assert_eq!(
            inputs, expected,
            "the sticky-on corner frame's full parameter set"
        );
    }

    /// The ripple fixture (renderer.rs's own test policy, verbatim):
    /// one observed kill death at a known position, one tick — one
    /// live ring, so the wet packing runs over real data (the empty
    /// field is the trivial all-zeros case).
    fn ripple_field() -> RippleField {
        let policy = engine_core::ripple::RipplePolicy {
            lifetime: 2.5,
            speed: 0.8,
            width: 0.35,
            amplitude: 1.0,
            decimation: 1,
        };
        let mut field = RippleField::new(policy).expect("the policy validates");
        assert_eq!(field.observe_death([-3.5, 1.7, 8.25]), None);
        let report = field
            .tick(1.0 / 60.0)
            .expect("the fixture dt is finite and nonnegative");
        assert_eq!(report.active, 1);
        field
    }
}
