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
//! **Stage S1 scope (additive, completed at S6):** the four
//! `render_corner*` methods and `render_layers` kept their routes
//! while [`crate::Renderer::submit_frame`] was proven against
//! them — the S1 gate's byte-equality and the type-level proof
//! below. S6 then deleted the four corner entries and the sticky
//! sky/post/wet setters: `submit_frame` is THE scene-backed
//! production route, and the sky/post/wet state is PER FRAME
//! here (the contract's shape — the submission is the one
//! declaration of the frame's passes). The two renderer-level
//! policies the adapter still reads — the proxy submission mode
//! and the TEST-ONLY ripple debug lane — ride
//! [`SubmissionPolicy`] until their own migration steps.
//!
//! **Stage S2 extension:** the population validation's
//! material-table bound moved behind the attach-time scene
//! handle — [`adapt_frame`] now takes the ATTACHED table's
//! length (the S1-era read of `lit_scene::materials().len()`;
//! the corner's value is pinned to the reference table by the
//! slice-side `corner_scene` equality gate) instead of reading a
//! renderer-internal constant.

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
    /// the cascade is computed over — AUTHORED content, supplied
    /// with the attached scene's description (the corner route's
    /// value — `shadows::scene_bounds(lit_scene::intersection_boxes())`
    /// — is authored in engine-slice's `corner_scene` since S6; a
    /// `submit_frame` caller authors their own).
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
/// through `shadows::scene_bounds`); the `SceneDescription` carries
/// it at attach time and the caller echoes it into the submission's
/// [`SceneFrame`] on shadowed frames.
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
/// steps move them into the submission, PLUS the two population
/// caps (P5 §1.3, A11/R1) — the construction-time budget fields
/// ([`crate::config::InstanceCapacity`] /
/// [`crate::config::PoseCapacity`]) the adapter's repacks bound
/// against, collected here so the typed refusals fire at the
/// CONFIGURED cap (defaults = today's constants; the CAP, never the
/// current carve, is the boundary). `Copy` — collected from the
/// renderer before the mutable frame flow begins.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SubmissionPolicy {
    /// The proxy submission mode (the equivalence gate's axis — see
    /// [`ProxyDrawMode`]; the production default is `Instanced`).
    pub proxy_draw_mode: ProxyDrawMode,
    /// The TEST-ONLY ripple debug lane (see [`RippleDebugMode`]);
    /// `Off` in every production frame.
    pub ripple_debug: RippleDebugMode,
    /// P5 §1.3: the instance arena's configured cap — the instance
    /// repack's typed-refusal boundary.
    pub instance_capacity: usize,
    /// P5 §1.3: the pose arena's configured cap — the pose repack's
    /// typed-refusal boundary.
    pub pose_capacity: usize,
}

/// The retained repack buffers' generation records (P5 §3.3,
/// A18/R2): which generation's bytes `Renderer`'s `instance_bytes`
/// and `pose_bytes` each hold. This is the CPU half of the
/// dirty-generation protocol — the adapter's gate consults and
/// updates it beside the buffers it describes, so the retained
/// bytes and their record can never disagree. 0 = unknown (the
/// initial state, and what every generation-0 — always-dirty —
/// submission resets to). The per-frame-slot write half lives in
/// `FrameState`'s stamps; the two levels are independent by design
/// (the buffer always ends up holding the declared generation's
/// bytes; the slot stamp decides only whether those bytes need
/// copying into THIS slot's mapped region).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PopulationGenerations {
    /// The generation whose bytes the retained instance buffer
    /// holds (the packed npc+vehicle+rain population).
    pub instance: u64,
    /// The generation whose bytes the retained pose buffer holds
    /// (the packed pose-block palette).
    pub pose: u64,
}

/// The input half of one `vulkan::render_frame` call, typed — the
/// parameter set the S1 adapter builds from a validated submission.
/// Field-for-field the existing per-feature entries' inline
/// arguments (the four owning states and the construction-time
/// config stay out: they are the renderer's own). This struct is the
/// equivalence proof's comparison target — the module test builds
/// `render_corner`'s parameter set by hand and asserts the adapter
/// reproduces it. Since A04 (design §3.4) the frame flow consumes
/// this set wrapped in
/// [`InternalFrame`](crate::vulkan::frame::InternalFrame) — the same
/// bundle carrying the derived
/// [`FramePlan`](crate::vulkan::frame_plan::FramePlan) — as ONE
/// parameter beside the `Copy`
/// [`FramePolicy`](crate::vulkan::frame::FramePolicy).
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

/// The missed-bump verify dispatch (P5 §3.3, A18/R2): on any reuse,
/// debug and test builds re-pack through the FULL-REPACK FALLBACK
/// and byte-compare (the independent reference path — a missed bump
/// is loud); the release-production stub is empty (never a
/// production frame's cost). Both arms share one signature so the
/// adapter's call site is configuration-independent.
#[cfg(any(debug_assertions, test))]
fn verify_population_reuse(
    population: &ProxyPopulation<'_>,
    policy: SubmissionPolicy,
    instance_bytes: &[u8],
    pose_bytes: &[u8],
    instance_decision: crate::proxy_instances::GenerationDecision,
    pose_decision: crate::proxy_instances::GenerationDecision,
) -> Result<(), RendererError> {
    use crate::proxy_instances::GenerationDecision;
    if instance_decision == GenerationDecision::Reused {
        crate::proxy_instances::verify_population_generation(
            instance_bytes,
            population.npc,
            population.vehicle,
            population.rain,
            policy.instance_capacity,
        )
        .map_err(RendererError::ProxyPack)?;
    }
    if pose_decision == GenerationDecision::Reused {
        crate::animation::verify_pose_generation(
            pose_bytes,
            population.npc_poses,
            policy.pose_capacity,
        )
        .map_err(RendererError::PosePack)?;
    }
    Ok(())
}

#[cfg(not(any(debug_assertions, test)))]
fn verify_population_reuse(
    _population: &ProxyPopulation<'_>,
    _policy: SubmissionPolicy,
    _instance_bytes: &[u8],
    _pose_bytes: &[u8],
    _instance_decision: crate::proxy_instances::GenerationDecision,
    _pose_decision: crate::proxy_instances::GenerationDecision,
) -> Result<(), RendererError> {
    Ok(())
}

/// Maps a submission onto the frame flow's input set — the
/// submission adapter (device-free by construction: the repack
/// buffers are caller-owned; [`crate::Renderer::submit_frame`]
/// passes its retained F-05 buffers), so the mapping is
/// unit-testable without a GPU: the module test builds the
/// former `render_corner` entry's argument shape and asserts the
/// produced [`FrameInputs`] equals the parameter set that entry
/// constructed inline (the expectation is hand-built in the
/// test, so the proof survives the entry's S6 deletion).
///
/// Validation order — the populated entries' own sequence, preserved
/// exactly: material indices vs the ATTACHED scene's table (the
/// S2 parameter — the corner routes' former
/// `lit_scene::materials().len()` read, now the handle's count),
/// then the instance repack, then the pose-index-vs-blocks check,
/// then the pose repack. Every refusal is a typed error that leaves
/// both repack buffers and all frame state untouched (an atomic
/// repack, never a partial buffer).
///
/// **P5 §3.3 (A18/R2) — the dirty-generation protocol's CPU half:**
/// both repacks are GENERATION-GATED (see
/// [`crate::proxy_instances::repack_populations_generation`]) —
/// validation always runs (the typed refusals stay per-frame), the
/// byte regeneration skips when the caller's declared generation
/// matches the retained buffer's record at the same packed
/// footprint. `generations` is the buffers' record, passed by the
/// renderer beside the buffers. On a reuse, debug and test builds
/// re-pack through the FULL-REPACK FALLBACK and byte-compare (the
/// independent reference path — a missed bump is loud; never a
/// release production frame's cost).
///
/// `material_table` is the attached scene's material count — the
/// population validation's bound. `0` on the layers-only shape
/// (unread there: no population path runs without a scene half).
#[allow(clippy::too_many_arguments)]
pub(crate) fn adapt_frame<'a, 'x>(
    policy: SubmissionPolicy,
    submission: &'x FrameSubmission<'a>,
    material_table: u32,
    debug_overlay: &'x [OverlayVertex],
    instance_bytes: &'x mut Vec<u8>,
    pose_bytes: &'x mut Vec<u8>,
    generations: &'x mut PopulationGenerations,
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
    // repack sequence, verbatim, with the repacks generation-gated
    // (P5 §3.3). `None` skips every check exactly like the
    // unpopulated entries.
    let proxy = match &scene.population {
        Some(population) => {
            if population.npc_material >= material_table
                || population.vehicle_material >= material_table
                || population.rain_material >= material_table
            {
                return Err(RendererError::Vulkan(format!(
                    "proxy material index outside the scene material table \
                     ({} materials, npc {}, vehicle {}, rain {})",
                    material_table,
                    population.npc_material,
                    population.vehicle_material,
                    population.rain_material
                )));
            }
            let instance_decision = crate::proxy_instances::repack_populations_generation(
                instance_bytes,
                population.npc,
                population.vehicle,
                population.rain,
                policy.instance_capacity,
                population.instance_generation,
                &mut generations.instance,
            )
            .map_err(RendererError::ProxyPack)?;
            // The pose contract (the capture-refusal discipline):
            // every POPULATED npc's pose index must select a supplied
            // block; the block-count bound is the repack's own typed
            // refusal below. The check runs on EVERY populated frame
            // — generation-gated or not (a declared-identical
            // population that outgrew its block list still refuses).
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
            let pose_decision = crate::animation::repack_poses_generation(
                pose_bytes,
                population.npc_poses,
                policy.pose_capacity,
                population.pose_generation,
                &mut generations.pose,
            )
            .map_err(RendererError::PosePack)?;
            // The missed-bump debug verify (P5 §3.3's mandated
            // mitigation, dispatched above): on any reuse, debug and
            // test builds re-pack through the full-repack fallback
            // and byte-compare — the independent reference path,
            // never a release production frame's cost.
            verify_population_reuse(
                population,
                policy,
                instance_bytes,
                pose_bytes,
                instance_decision,
                pose_decision,
            )?;
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
                instance_generation: population.instance_generation,
                pose_generation: population.pose_generation,
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

    /// THE S1 EQUIVALENCE PROOF (device-free, the type-level bar):
    /// a [`FrameSubmission`] shaped exactly like the former
    /// `render_corner` entry's argument list — camera, light,
    /// window size, no population, no shadows, no dynamic layers —
    /// maps through [`adapt_frame`] onto the IDENTICAL parameter
    /// set that entry constructed inline and handed
    /// `vulkan::render_frame` (the expectation below is built from
    /// the SAME expressions the entry contained, cited per field —
    /// the proof survives the entry's S6 deletion because the
    /// expectation is hand-built here). If either side drifts,
    /// this fails. Two configurations are proven: the all-off
    /// plain frame, and the effects-on frame (sky + post + wet
    /// with a live ripple field — the state the demo carried per
    /// frame through the submission's fields where the deleted
    /// sticky setters once fed the old route).
    #[test]
    fn plain_corner_submission_maps_to_render_corner_parameter_set() {
        // The former render_corner entry's argument shape
        // (renderer.rs at its S1-era lines): a view-projection, an
        // eye, the day preset, a window size.
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
        // fields: the production proxy mode, the shipped debug lane,
        // and the default population caps (P5 §1.3 — the constants).
        let policy = SubmissionPolicy {
            proxy_draw_mode: ProxyDrawMode::default(),
            ripple_debug: RippleDebugMode::default(),
            instance_capacity: crate::proxy_instances::MAX_INSTANCES,
            pose_capacity: crate::animation::MAX_POSE_BLOCKS,
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
        // The adapter's generation records (P5 §3.3): fresh — every
        // first submission repacks, exactly like the renderer's
        // construction-time state.
        let mut generations = PopulationGenerations::default();
        // The attached table's length (S2): the corner routes' own
        // bound — `lit_scene::materials().len()` (the former inline
        // read), pinned to the slice-side corner builder's table by
        // the corner_scene equality gate.
        let inputs = adapt_frame(
            policy,
            &submission,
            3,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
            &mut generations,
        )
        .expect("the plain corner submission validates");
        // The entry's own construction: the corner FrameParams,
        // empty dynamic layers, and — every effect off — no sky, no
        // post, no wet camera.
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
        // The effects-on frame: the entry's former sky_params and
        // wet_camera bodies read the sticky members set through the
        // deleted setters; the submission carries the same state per
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
        let mut generations = PopulationGenerations::default();
        let inputs = adapt_frame(
            policy,
            &submission,
            3,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
            &mut generations,
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
            // sky_params' former body: SkyFrameParams::new over the
            // CPU-computed inverse, the palette, the disc direction.
            sky: Some(vulkan::sky_draw::SkyFrameParams::new(
                crate::sky::mat4_inverse(&matrix),
                sky.palette,
                sky.disc_dir,
            )),
            post: true,
            // wet_camera's former body: the true camera and light
            // plus the ripple uniform packed exactly as the wet
            // submission path packs it (the same pure function).
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

    /// THE S2 EQUIVALENCE EXTENSION (device-free, the type-level
    /// bar): the population material validation reads the ATTACHED
    /// scene's table length — the parameter the renderer fills from
    /// the [`crate::SceneHandle`] — not a renderer-internal
    /// constant. The corner's attached value (3, pinned to
    /// `lit_scene::materials()` by the slice-side `corner_scene`
    /// equality gate) refuses index 3 with EXACTLY the populated
    /// entries' own refusal text (the S2 wording — "scene" for
    /// "corner"); a LARGER attached table accepts the same index —
    /// the boundary is the attachment's, proving the validation
    /// follows the handle.
    #[test]
    fn population_material_validation_reads_the_attached_table() {
        let matrix = [
            [0.9, 0.0, 0.0, 0.0],
            [0.0, 0.9, 0.0, 0.0],
            [0.0, 0.0, 0.9, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let light = crate::lit_scene::day_preset();
        let policy = SubmissionPolicy {
            proxy_draw_mode: ProxyDrawMode::default(),
            ripple_debug: RippleDebugMode::default(),
            instance_capacity: crate::proxy_instances::MAX_INSTANCES,
            pose_capacity: crate::animation::MAX_POSE_BLOCKS,
        };
        // An empty-kind population carrying material index 3: no
        // repack work, no pose blocks — the validation is the only
        // code that runs (the index is checked regardless of kind
        // population, exactly like the entries'). The generations
        // default to 0 — the un-opted caller's always-dirty shape.
        let population = crate::renderer::ProxyPopulation {
            npc: &[],
            npc_poses: &[],
            npc_material: 3,
            vehicle: &[],
            vehicle_material: 0,
            rain: &[],
            rain_material: 0,
            instance_generation: 0,
            pose_generation: 0,
        };
        let submission = FrameSubmission {
            camera: FrameCamera {
                view_proj: matrix,
                eye: [4.0, 1.7, 12.0],
            },
            output: OutputTarget {
                window_size: (1280, 720),
            },
            layers: DynamicLayers::default(),
            scene: Some(SceneFrame {
                light,
                shadows: None,
                population: Some(population),
                sky: None,
                post: false,
                wet: None,
            }),
        };
        // Against the corner's attached table (3 materials): the
        // typed refusal, the entries' own text.
        let mut instance_bytes = Vec::new();
        let mut pose_bytes = Vec::new();
        let mut generations = PopulationGenerations::default();
        let error = adapt_frame(
            policy,
            &submission,
            3,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
            &mut generations,
        )
        .expect_err("index 3 must refuse against the corner table");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert_eq!(
            text,
            "proxy material index outside the scene material table \
             (3 materials, npc 3, vehicle 0, rain 0)",
            "the entries' own refusal text (the S2 wording)"
        );
        // Against a FOUR-material attached table: the same
        // submission validates — the empty kinds repack to nothing
        // and the upload carries the (now in-table) indices.
        let mut instance_bytes = Vec::new();
        let mut pose_bytes = Vec::new();
        let mut generations = PopulationGenerations::default();
        let inputs = adapt_frame(
            policy,
            &submission,
            4,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
            &mut generations,
        )
        .expect("index 3 is inside a four-material table");
        let proxy = inputs.proxy.expect("the population maps to an upload");
        assert_eq!(proxy.npc_material, 3, "the in-table index rides through");
        assert_eq!(proxy.npc_count, 0, "the empty kinds repack to nothing");
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

    /// P5 §3.3 (A18/R2) — THE ADAPTER-LEVEL GENERATION EQUIVALENCE:
    /// a fuzzed (generation, population) sequence maps through the
    /// generation-gated adapter onto BYTE-IDENTICAL repack buffers
    /// to the always-repack path — the same-generation submissions
    /// reuse (nothing regenerated), the bumped submissions repack,
    /// and the generation-0 submissions (today's behavior) repack
    /// every frame regardless. The oracle runs the SAME sequence
    /// through the generations zeroed (the un-opted shape — the
    /// full-repack fallback), comparing both buffers after every
    /// frame. This is the CPU half of the §3.5 oracle harness (the
    /// write half lives in `vulkan::frame`'s stamp tests).
    #[test]
    fn generation_gated_adaptation_matches_the_always_repack_oracle() {
        use crate::animation::{
            HUMANOID_JOINTS, JointPose, compose_joint_matrices, humanoid_skeleton, rest_pose_block,
            skin_matrices,
        };
        use crate::proxy_instances::{InstanceTransform, ProxyInstance};
        let matrix = [
            [0.9, 0.0, 0.0, 0.0],
            [0.0, 0.9, 0.0, 0.0],
            [0.0, 0.0, 0.9, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let light = crate::lit_scene::day_preset();
        let policy = SubmissionPolicy {
            proxy_draw_mode: ProxyDrawMode::default(),
            ripple_debug: RippleDebugMode::default(),
            instance_capacity: crate::proxy_instances::MAX_INSTANCES,
            pose_capacity: crate::animation::MAX_POSE_BLOCKS,
        };
        // The population fixture: two npcs (one rest, one walking),
        // advanced by a deterministic pseudo-random walk — the shape
        // a render-then-step crowd drives (ticks bump, between-tick
        // frames do not).
        let rest = rest_pose_block();
        let skeleton = humanoid_skeleton();
        let walker_at = |turn: i16| {
            let mut pose = [JointPose {
                delta_pos: [0.0; 3],
                delta_turns: [0; 3],
            }; HUMANOID_JOINTS];
            pose[7].delta_turns = [turn, 0, 0];
            let joint_worlds = compose_joint_matrices(&skeleton, &pose);
            skin_matrices(&joint_worlds, &skeleton)
        };
        let mut instances = vec![
            ProxyInstance {
                transform: InstanceTransform {
                    pos: [1.0, 0.02, -2.0],
                    turn: 1000,
                    scale: 1.0,
                },
                tint: [0.9, 0.9, 0.9],
                alpha: 1.0,
                pose_index: 0,
            },
            ProxyInstance {
                transform: InstanceTransform {
                    pos: [-3.0, 0.02, 4.0],
                    turn: 40000,
                    scale: 1.0,
                },
                tint: [0.8, 0.6, 0.5],
                alpha: 1.0,
                pose_index: 1,
            },
        ];
        let mut poses = vec![rest, walker_at(9000)];
        // The gated state (what the renderer owns) and the oracle
        // state (the same buffers driven always-dirty).
        let mut gated_bytes = Vec::new();
        let mut gated_poses = Vec::new();
        let mut generations = PopulationGenerations::default();
        let mut oracle_bytes = Vec::new();
        let mut oracle_poses = Vec::new();
        let mut oracle_generations = PopulationGenerations::default();
        let mut rng: u64 = 0x243F_6A88_85A3_08D3;
        let mut instance_generation = 1_u64;
        let mut pose_generation = 1_u64;
        // The submission builder: a local fn (a closure cannot
        // express the shared parameter lifetime), taking the
        // population slices as parameters so the same construction
        // serves the gated path, the always-dirty oracle, and the
        // same-generation resubmission after the sequence.
        fn build<'a>(
            instances: &'a [ProxyInstance],
            poses: &'a [[[[f32; 4]; 4]; crate::animation::HUMANOID_JOINTS]],
            instance_generation: u64,
            pose_generation: u64,
            matrix: [[f32; 4]; 4],
            light: crate::lit_scene::LightParams,
        ) -> FrameSubmission<'a> {
            FrameSubmission {
                camera: FrameCamera {
                    view_proj: matrix,
                    eye: [4.0, 1.7, 12.0],
                },
                output: OutputTarget {
                    window_size: (1280, 720),
                },
                layers: DynamicLayers::default(),
                scene: Some(SceneFrame {
                    light,
                    shadows: None,
                    population: Some(crate::renderer::ProxyPopulation {
                        npc: instances,
                        npc_poses: poses,
                        npc_material: 0,
                        vehicle: &[],
                        vehicle_material: 0,
                        rain: &[],
                        rain_material: 0,
                        instance_generation,
                        pose_generation,
                    }),
                    sky: None,
                    post: false,
                    wet: None,
                }),
            }
        }
        for frame in 0..16_u64 {
            rng = rng.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let bump_instances = frame % 3 == 0;
            let bump_poses = frame % 2 == 0;
            if bump_instances {
                instance_generation += 1;
                instances[0].transform.pos[0] += (rng % 100) as f32 * 0.01;
            }
            if bump_poses {
                pose_generation += 1;
                poses[1] = walker_at(((rng >> 16) % 16_384) as i16);
            }
            // The gated path (the renderer's own state threading).
            let gated_submission = build(
                &instances,
                &poses,
                instance_generation,
                pose_generation,
                matrix,
                light,
            );
            let inputs = adapt_frame(
                policy,
                &gated_submission,
                3,
                &[],
                &mut gated_bytes,
                &mut gated_poses,
                &mut generations,
            )
            .expect("the gated submission validates");
            // The oracle path: the SAME population declared
            // always-dirty (generation 0) — the full-repack
            // fallback, today's behavior.
            let oracle_submission = build(&instances, &poses, 0, 0, matrix, light);
            let oracle = adapt_frame(
                policy,
                &oracle_submission,
                3,
                &[],
                &mut oracle_bytes,
                &mut oracle_poses,
                &mut oracle_generations,
            )
            .expect("the oracle submission validates");
            // The scalar facts first (the FrameInputs borrow the
            // repack buffers — extracting the plain values ends
            // those borrows before the buffer comparison).
            let gated_facts = inputs.proxy.map(|upload| {
                (
                    upload.npc_count,
                    upload.rain_count,
                    upload.instance_generation,
                    upload.pose_generation,
                )
            });
            let oracle_facts = oracle
                .proxy
                .map(|upload| (upload.npc_count, upload.rain_count));
            assert_eq!(
                gated_bytes, oracle_bytes,
                "frame {frame}: the instance bytes"
            );
            assert_eq!(gated_poses, oracle_poses, "frame {frame}: the pose bytes");
            let (gated_npc, gated_rain, gated_instance_gen, gated_pose_gen) =
                gated_facts.expect("the gated frame is populated");
            let (oracle_npc, oracle_rain) = oracle_facts.expect("the oracle frame is populated");
            assert_eq!(
                (gated_npc, gated_rain),
                (oracle_npc, oracle_rain),
                "frame {frame}: the mapped counts agree"
            );
            assert_eq!(
                gated_instance_gen, instance_generation,
                "frame {frame}: the declared generation rides the upload"
            );
            assert_eq!(
                gated_pose_gen, pose_generation,
                "frame {frame}: the declared pose generation rides the upload"
            );
        }
        // After the sequence the gated records hold the LAST
        // declared generations, and a same-generation resubmission
        // reuses (nothing regenerated — the between-tick shape).
        let before = gated_bytes.clone();
        let pose_before = gated_poses.clone();
        let resubmission = build(
            &instances,
            &poses,
            instance_generation,
            pose_generation,
            matrix,
            light,
        );
        adapt_frame(
            policy,
            &resubmission,
            3,
            &[],
            &mut gated_bytes,
            &mut gated_poses,
            &mut generations,
        )
        .expect("the same-generation resubmission validates");
        assert_eq!(gated_bytes, before, "the reuse regenerated nothing");
        assert_eq!(gated_poses, pose_before);
        assert_eq!(
            (generations.instance, generations.pose),
            (instance_generation, pose_generation),
            "the records hold the declared generations"
        );
    }
}
