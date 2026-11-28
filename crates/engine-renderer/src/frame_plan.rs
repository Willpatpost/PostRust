//! The frame plan — the explicit pass-uses declaration (P4 D2 §4.2,
//! stage S3).
//!
//! Before S3 the frame flow derived "which passes does this frame
//! use" IMPLICITLY, from the positional `Option` parameters, at
//! every site that needed the answer: the lazy-ensure dispatcher
//! (`PipelineState::ensure_frame_targets_and_resources`) re-derived
//! it at THREE call sites — `prepare_frame`'s pre-recreation ensure,
//! the `Recreated` arm's re-run, and `prepare_after_recreation`'s
//! out-of-date path — and the record cascade re-derived the wet
//! ground's placement twice more (the main pass's branch and the
//! post chain's HDR scene pass each rebuilt
//! [`GroundRedrawPass`] from the wet `Option` and the post bool).
//! The presence-driven discipline itself is sound (lazy, either-way,
//! flat allocation counters across replacement — the
//! allocation-constancy batteries' proof); what it lacked was a
//! TYPED declaration of uses written once (the design's §1.4
//! finding: the layered family-growth conditions were inline
//! boolean ladders that had to stay in lockstep with the recording
//! cascade's guards — the scene_record belt-checks exist precisely
//! because they could drift).
//!
//! This module is that declaration. [`FramePlan::derive`] is called
//! ONCE per frame, at the [`InternalFrame`](crate::vulkan::frame::
//! InternalFrame) construction that precedes `vulkan::render_frame`
//! (A04 moved the call from the top of `render_frame`'s body to the
//! entry bundle's constructor — the same single derivation, the same
//! point relative to the flow: before any synchronization state is
//! touched) — and the three preparation sites plus the record
//! cascade's pass-variant decisions consume the resulting
//! [`FramePlan`] instead of re-deriving. It is a thin coordinator,
//! exactly the design's §4.1 negative justification made positive:
//! **no render graph** (the pass set is ≤7, the order is fixed by
//! documented blend/dependency semantics, barriers are hand-proven
//! under sync-validation), **no runtime dispatch** (a `Copy` struct
//! read by direct field access — no trait objects, no `Box`, no
//! `dyn`), **no needless allocation** (stack assembly; the
//! allocation-constancy tuples in the lavapipe batteries are the
//! proof this introduces none).
//!
//! THE EQUIVALENCE BAR (the S1/S2 pattern extended): the submit_frame
//! route maps a [`crate::submission::FrameSubmission`] onto the same
//! flat input set the per-feature entries construct inline (pinned
//! by `submission`'s own unit test), and this module's test pins the
//! next link — the [`FramePlan`] derived from that input set equals
//! the plan the submission's `SceneFrame` fields declare, field for
//! field, with the expected side built from the SAME expressions the
//! pre-S3 code performed implicitly at each site. If either the
//! adapter or the plan derivation drifts, the test fails.
//!
//! WHAT STAYS OUT (deliberate, each with its reason):
//!
//! - **The frame slot** stays owned by `FrameState` and read live at
//!   each consumer — it is advanced only at the frame's END
//!   (`render_frame`'s final statement), so it cannot change
//!   mid-frame and carrying it here would duplicate state without
//!   consolidating any re-derivation.
//! - **The capture and shadow-readback request bits** stay LIVE
//!   reads (`frames.capture.pending` / `frames.shadow_readback.pending`).
//!   The capture bit is prepare-MUTABLE: a refused capture ensure
//!   clears `pending` mid-frame, and the `Recreated` arm re-runs
//!   preparation — freezing the bit in the plan would re-attempt the
//!   ensure against the replacement swapchain where the current code
//!   (correctly, observably) does not. The shadow-readback bit is
//!   cleared only at record time so a frozen copy would be
//!   equivalent, but it is kept live with capture for one rule: the
//!   plan declares the SUBMISSION's uses; request-side diagnostics
//!   state is frame state, not submission data (the design's §5.5
//!   capture/diagnostics separation).
//! - **The lighting-variant bind family** (the design sketch's
//!   `lit_variant`) is consolidated on the RECORDING side, as
//!   `scene_record::LitBind` — the genuine per-variant variation
//!   (which descriptor family and dynamic stride each kind's bind
//!   comes from) lives with the arms that construct it; the plan's
//!   [`PassUses::shadow_depth`] bit is the variant's declaration.

use crate::overlay::OverlayVertex;
use crate::scene::SceneVertex;
use crate::vulkan::reflection_draw::WetCamera;
use crate::vulkan::scene_record::GroundRedrawPass;
use crate::vulkan::shadow_draw::ShadowFrameParams;
use crate::vulkan::sky_draw::SkyFrameParams;

use super::proxy_draw::ProxyFrameUpload;

/// The typed mirror of the frame flow's implicit `Option`-ladder
/// (P4 D2 §4.2): one bit per pass family the current frame uses.
/// Every field names the same fact the pre-S3 code read from a
/// positional parameter at each site — the ensure dispatcher's
/// conditions and the record cascade's pass-variant decisions, now
/// written once. `false` means the pass does not run.
///
/// The bits are the SUBMISSION-derived facts only: everything that
/// also reads live renderer state (the scene attachment, the lazy
/// family states, the request-side diagnostics bits) keeps its
/// belt-check at the consumer — the plan does not remove the lazy
/// first-frame window or the replacement frames the belt-checks
/// guard (the design's §4.2 note).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PassUses {
    /// The dynamic region/foreground layers draw this frame — either
    /// batch nonempty (the lazy region buffer's ensure condition and
    /// the mapped-slot upload's gate, previously
    /// `!region_vertices.is_empty() || !foreground_vertices.is_empty()`
    /// at each ensure site).
    pub region_layers: bool,
    /// The proxy population pass runs (a populated frame —
    /// `proxy.is_some()`, previously re-read at every ensure site and
    /// the record guards).
    pub populated: bool,
    /// The 6-E2 indirect command path (a populated frame in
    /// [`crate::ProxyDrawMode::Indirect`] — previously
    /// `proxy.is_some_and(|upload| matches!(upload.mode, Indirect))`).
    pub indirect: bool,
    /// The 6-F shadow depth pass runs (a shadowed frame —
    /// `shadowed.is_some()`, the lighting variant's declaration: the
    /// shadowed lit family replaces the ordinary one).
    pub shadow_depth: bool,
    /// The Era-1 sky pass runs (`sky.is_some()`).
    pub sky: bool,
    /// The Era-1 post chain runs (`post`): the HDR scene pass + bloom
    /// mips + composite REPLACE the main render pass — this bit is
    /// the scene-pass variant discriminant the record cascade
    /// branches on.
    pub post: bool,
    /// The Era-1 wet reflection runs (`wet.is_some()`): the
    /// mirrored-scene pass before the scene pass plus the ground
    /// receiver re-draw inside it.
    pub wet: bool,
    /// The overlay pass has vertices to draw (`!hud.is_empty() ||
    /// !debug_overlay.is_empty()` — the mapped overlay-slot upload's
    /// gate).
    pub overlay: bool,
}

/// One frame's explicit pass-uses declaration (P4 D2 §4.2): the
/// [`PassUses`] bits plus the one pass-placement fact the record
/// cascade needs beyond them — the wet ground re-draw's placement.
/// Derived ONCE per frame (see [`FramePlan::derive`]); `Copy`; read
/// by direct field access at every consumer.
///
/// Since P5 §1.3 (A11/R1) the plan also carries the frame's DEMAND
/// COUNTS — the populations and layer sizes the arena carves size
/// against. Still a declaration, not a data carrier (counts, never
/// bytes): the lazy-ensure ladder reads them to size/grow the three
/// mapped carves (instance, pose, region) toward their configured
/// caps BEFORE the frame's uploads, exactly where it already read
/// the uses bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FramePlan {
    /// The pass-uses declaration — THE explicit-uses bit set.
    pub uses: PassUses,
    /// The wet-ground receiver re-draw's placement inside the scene
    /// pass (the finding-14 ordered-blend slot): `None` on the dry
    /// frame, `Main` when the wet frame renders through the main
    /// pass, `Hdr` when it renders through the post chain's HDR
    /// scene pass. Previously derived TWICE — at the main-pass
    /// branch and inside `post_draw`'s record — from the same two
    /// facts; now written once here and consumed by both.
    pub ground: GroundRedrawPass,
    /// P5 §1.3: this frame's total dynamic-geometry supply (world
    /// plus foreground vertices, saturating — the region arena's
    /// demand quantum; the F-03 clamp stays the fail-visible
    /// boundary at the configured cap).
    pub region_demand_vertices: u32,
    /// P5 §1.3: this frame's total proxy population (npc, vehicle,
    /// and rain counts summed, saturating — the instance arena's
    /// demand).
    pub instance_demand: u32,
    /// P5 §1.3: this frame's pose-block supply — the repacked pose
    /// bytes over [`crate::animation::POSE_STRIDE`] (the pose
    /// arena's demand; exactly the blocks the safe layer validated).
    pub pose_demand_blocks: u32,
}

impl FramePlan {
    /// Derives the frame plan from the frame flow's input set — THE
    /// ONE DERIVATION (P4 D2 §4.2, stage S3; since A04 it lives at
    /// the `InternalFrame` construction that precedes
    /// `vulkan::render_frame`), on the same flat facts every route
    /// (the per-feature entries and the `submit_frame` adapter)
    /// already hands the frame flow: the layer batches, the
    /// population/shadowed/sky/post/wet `Option`s. Each field's
    /// expression is the exact condition the pre-S3 code performed
    /// implicitly at each consumption site (the ensure ladder, the
    /// upload gates, the record cascade's pass-variant branches) —
    /// the equivalence this constructor preserves is pinned by this
    /// module's test at the type level, and by the lavapipe
    /// batteries' byte-identity at the frame level.
    ///
    /// The slice parameters feed ONLY the emptiness checks — the
    /// data itself flows through the frame flow's existing
    /// parameters untouched (the plan is a uses declaration, not a
    /// data carrier; the record cascade's guards keep binding the
    /// data they draw with).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn derive(
        region_vertices: &[SceneVertex],
        foreground_vertices: &[SceneVertex],
        hud: &[OverlayVertex],
        debug_overlay: &[OverlayVertex],
        proxy: Option<&ProxyFrameUpload<'_>>,
        shadowed: Option<&ShadowFrameParams>,
        sky: Option<&SkyFrameParams>,
        post: bool,
        wet: Option<&WetCamera>,
    ) -> Self {
        let uses = PassUses {
            region_layers: !region_vertices.is_empty() || !foreground_vertices.is_empty(),
            populated: proxy.is_some(),
            indirect: proxy
                .is_some_and(|upload| matches!(upload.mode, crate::ProxyDrawMode::Indirect)),
            shadow_depth: shadowed.is_some(),
            sky: sky.is_some(),
            post,
            wet: wet.is_some(),
            overlay: !hud.is_empty() || !debug_overlay.is_empty(),
        };
        // P5 §1.3: the demand counts, derived ONCE beside the uses —
        // the same saturating conversions the F-03 clamp uses (a
        // supply beyond u32::MAX degrades instead of wrapping; the
        // structural bound, not memory luck). The pose count is the
        // repacked bytes over POSE_STRIDE — exactly the block count
        // the safe layer's repack validated (blocks × POSE_STRIDE
        // bytes, by the repack's own construction).
        let region_demand_vertices = (region_vertices
            .len()
            .saturating_add(foreground_vertices.len()))
        .min(u32::MAX as usize) as u32;
        let instance_demand = proxy.map_or(0, |upload| {
            upload
                .npc_count
                .saturating_add(upload.vehicle_count)
                .saturating_add(upload.rain_count)
        });
        let pose_demand_blocks = proxy.map_or(0, |upload| {
            (upload.pose_bytes.len() / crate::animation::POSE_STRIDE).min(u32::MAX as usize) as u32
        });
        // The wet receiver's placement (finding 14): the re-draw runs
        // inside whichever scene pass this frame records through —
        // the HDR one when the post chain replaced the main pass, the
        // main one otherwise. Both record sites previously rebuilt
        // this from (wet, post) independently; it is written once
        // here.
        let ground = if uses.wet {
            if uses.post {
                GroundRedrawPass::Hdr
            } else {
                GroundRedrawPass::Main
            }
        } else {
            GroundRedrawPass::None
        };
        FramePlan {
            uses,
            ground,
            region_demand_vertices,
            instance_demand,
            pose_demand_blocks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::{ProxyDrawMode, RippleDebugMode};
    use crate::shadows::ShadowBias;
    use crate::sky::day_sky;
    use crate::submission::{
        DynamicLayers, FrameCamera, FrameSubmission, OutputTarget, SceneBounds, SceneFrame,
        ShadowFrame, SubmissionPolicy, WetFrame, adapt_frame,
    };
    use crate::{ProxyPopulation, SceneLayer, SkyFrame};

    /// THE S3 EQUIVALENCE PROOF (device-free, the type-level bar —
    /// the S1/S2 pattern extended): a [`FrameSubmission`] maps
    /// through `adapt_frame` onto the frame flow's input set (the
    /// mapping S1's test pinned against the per-feature entries'
    /// inline construction), and the [`FramePlan`] derived from that
    /// input set — the production derivation `render_frame` performs
    /// once per frame — equals the plan the submission's `SceneFrame`
    /// fields DECLARE. Every expected value below is built from the
    /// SAME expressions the pre-S3 code performed implicitly at each
    /// site (cited per field); if the adapter or the derivation
    /// drifts, this fails. Four configurations are proven: the
    /// everything-on frame (every scene pass plus both dynamic layer
    /// families — the ground lands in the HDR scene pass), the
    /// wet-without-post frame (the ground lands in the MAIN pass),
    /// the all-off corner frame, and the layers-only frame.
    #[test]
    fn the_plan_derived_from_the_frame_inputs_equals_the_submissions_declaration() {
        let matrix = [
            [0.9, 0.0, 0.0, 0.0],
            [0.0, 0.9, 0.0, 0.0],
            [0.0, 0.0, 0.9, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let eye = [4.0, 1.7, 12.0];
        let light = crate::lit_scene::day_preset();
        let policy = SubmissionPolicy {
            proxy_draw_mode: ProxyDrawMode::default(),
            ripple_debug: RippleDebugMode::default(),
            instance_capacity: crate::proxy_instances::MAX_INSTANCES,
            pose_capacity: crate::animation::MAX_POSE_BLOCKS,
        };
        let world = [crate::scene::SceneVertex {
            pos: [0.0, 0.0, 0.0],
            col: [1.0, 0.0, 0.0],
        }];
        let hud = [crate::overlay::OverlayVertex {
            pos: [16.0, 16.0],
            col: [1.0, 1.0, 1.0, 1.0],
            uv: [-1.0, -1.0],
        }];
        let population = ProxyPopulation {
            npc: &[],
            npc_poses: &[],
            npc_material: 0,
            vehicle: &[],
            vehicle_material: 0,
            rain: &[],
            rain_material: 0,
        };
        let shadows = ShadowFrame {
            bias: ShadowBias {
                constant: 0.0,
                slope_scale: 1.0,
            },
            bounds: SceneBounds {
                min: [-20.0, 0.0, -20.0],
                max: [20.0, 8.0, 20.0],
            },
        };
        let sky = SkyFrame {
            palette: day_sky(),
            disc_dir: [-0.5, 0.707_106_77, -0.5],
        };

        // ---- Configuration 1: the everything-on frame ----
        // Every scene pass declared (population, shadows, sky, post,
        // wet) plus both dynamic layer families (world and
        // foreground batches, HUD) — the sticky-on populated
        // shadowed frame the demo renders through
        // render_corner_populated_shadowed with set_sky/set_post/
        // set_wet all set.
        let submission = FrameSubmission {
            camera: FrameCamera {
                view_proj: matrix,
                eye,
            },
            output: OutputTarget {
                window_size: (1280, 720),
            },
            layers: DynamicLayers {
                world: &world,
                foreground: Some(SceneLayer {
                    vertices: &world,
                    view_proj: &matrix,
                }),
                hud: &hud,
                debug_text: None,
            },
            scene: Some(SceneFrame {
                light,
                shadows: Some(shadows),
                population: Some(population),
                sky: Some(sky),
                post: true,
                wet: Some(WetFrame { ripple: None }),
            }),
        };
        let plan = derive_plan(policy, &submission);
        let scene = submission
            .scene
            .expect("the on configuration carries a scene");
        let expected = FramePlan {
            uses: PassUses {
                // The ensure ladder's region condition,
                // `!region_vertices.is_empty() ||
                // !foreground_vertices.is_empty()`, over the
                // submission's layer batches.
                region_layers: !submission.layers.world.is_empty()
                    || submission
                        .layers
                        .foreground
                        .is_some_and(|layer| !layer.vertices.is_empty()),
                // `proxy.is_some()` — the population's presence.
                populated: scene.population.is_some(),
                // The indirect bit is false in the default
                // (Instanced) mode.
                indirect: false,
                // `shadowed.is_some()` — the shadows' presence.
                shadow_depth: scene.shadows.is_some(),
                // `sky.is_some()`.
                sky: scene.sky.is_some(),
                // `post`.
                post: scene.post,
                // `wet.is_some()`.
                wet: scene.wet.is_some(),
                // `!hud.is_empty() || !debug_overlay.is_empty()`
                // (no debug text — the retained buffer is empty).
                overlay: !submission.layers.hud.is_empty(),
            },
            // The wet receiver lands inside the post chain's HDR
            // scene pass (post replaced the main pass).
            ground: GroundRedrawPass::Hdr,
            // P5 §1.3's demand counts: world + foreground vertex
            // totals, the population's kind counts, and the pose
            // blocks — all derived from the same submission fields
            // (this configuration's population slices are all EMPTY,
            // so the populated demands read 0 while the bits read
            // on).
            region_demand_vertices: (submission.layers.world.len()
                + submission
                    .layers
                    .foreground
                    .map_or(0, |layer| layer.vertices.len()))
                as u32,
            instance_demand: 0,
            pose_demand_blocks: 0,
        };
        assert_eq!(plan, expected, "the everything-on frame's plan");

        // ---- Configuration 2: wet without post ----
        // The same frame minus the post chain: the ground re-draw
        // lands in the MAIN pass, and the post bit clears.
        let submission = FrameSubmission {
            camera: FrameCamera {
                view_proj: matrix,
                eye,
            },
            output: OutputTarget {
                window_size: (1280, 720),
            },
            layers: DynamicLayers::default(),
            scene: Some(SceneFrame {
                light,
                shadows: None,
                population: None,
                sky: None,
                post: false,
                wet: Some(WetFrame { ripple: None }),
            }),
        };
        let plan = derive_plan(policy, &submission);
        let scene = submission
            .scene
            .expect("the wet configuration carries a scene");
        let expected = FramePlan {
            uses: PassUses {
                region_layers: false,
                populated: false,
                indirect: false,
                shadow_depth: false,
                sky: false,
                post: false,
                wet: scene.wet.is_some(),
                overlay: false,
            },
            ground: GroundRedrawPass::Main,
            region_demand_vertices: 0,
            instance_demand: 0,
            pose_demand_blocks: 0,
        };
        assert_eq!(plan, expected, "the wet-without-post frame's plan");

        // ---- Configuration 3: the all-off corner frame ----
        // render_corner's own shape (S1's configuration 1): a scene
        // half with every pass off, no dynamic layers.
        let submission = FrameSubmission {
            camera: FrameCamera {
                view_proj: matrix,
                eye,
            },
            output: OutputTarget {
                window_size: (1280, 720),
            },
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
        let plan = derive_plan(policy, &submission);
        let expected = FramePlan {
            uses: PassUses {
                region_layers: false,
                populated: false,
                indirect: false,
                shadow_depth: false,
                sky: false,
                post: false,
                wet: false,
                overlay: false,
            },
            ground: GroundRedrawPass::None,
            region_demand_vertices: 0,
            instance_demand: 0,
            pose_demand_blocks: 0,
        };
        assert_eq!(plan, expected, "the all-off corner frame's plan");

        // ---- Configuration 4: the layers-only frame ----
        // `render_layers`' shape (`scene: None`): every scene-half
        // pass off regardless of the layer batches' content.
        let submission = FrameSubmission {
            camera: FrameCamera {
                view_proj: matrix,
                eye,
            },
            output: OutputTarget {
                window_size: (1280, 720),
            },
            layers: DynamicLayers {
                world: &world,
                foreground: None,
                hud: &hud,
                debug_text: None,
            },
            scene: None,
        };
        let plan = derive_plan(policy, &submission);
        let expected = FramePlan {
            uses: PassUses {
                // The region layers still draw (the layers family's
                // own pass) — only the SCENE-half passes are off.
                region_layers: true,
                populated: false,
                indirect: false,
                shadow_depth: false,
                sky: false,
                post: false,
                wet: false,
                overlay: true,
            },
            ground: GroundRedrawPass::None,
            // The layers-only frame's world batch is the 1-vertex
            // fixture — the region demand counts exactly it (no
            // foreground, no population).
            region_demand_vertices: submission.layers.world.len() as u32,
            instance_demand: 0,
            pose_demand_blocks: 0,
        };
        assert_eq!(plan, expected, "the layers-only frame's plan");
    }

    /// The 6-E2 indirect bit: the plan's declaration follows the
    /// proxy submission MODE the policy carries (the axis the
    /// equivalence gates switch) — a populated frame in `Indirect`
    /// mode declares the indirect command path, the same frame in
    /// the default `Instanced` mode does not. The pre-S3 ensure read
    /// `upload.mode` (the policy's value threaded through the
    /// adapter); the plan reads it once at derivation.
    #[test]
    fn the_indirect_bit_follows_the_proxy_submission_mode() {
        let matrix = [
            [0.9, 0.0, 0.0, 0.0],
            [0.0, 0.9, 0.0, 0.0],
            [0.0, 0.0, 0.9, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let population = ProxyPopulation {
            npc: &[],
            npc_poses: &[],
            npc_material: 0,
            vehicle: &[],
            vehicle_material: 0,
            rain: &[],
            rain_material: 0,
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
                light: crate::lit_scene::day_preset(),
                shadows: None,
                population: Some(population),
                sky: None,
                post: false,
                wet: None,
            }),
        };
        let indirect = derive_plan(
            SubmissionPolicy {
                proxy_draw_mode: ProxyDrawMode::Indirect,
                ripple_debug: RippleDebugMode::default(),
                instance_capacity: crate::proxy_instances::MAX_INSTANCES,
                pose_capacity: crate::animation::MAX_POSE_BLOCKS,
            },
            &submission,
        );
        assert!(
            indirect.uses.populated && indirect.uses.indirect,
            "the indirect-mode populated frame declares the indirect path"
        );
        let instanced = derive_plan(
            SubmissionPolicy {
                proxy_draw_mode: ProxyDrawMode::Instanced,
                ripple_debug: RippleDebugMode::default(),
                instance_capacity: crate::proxy_instances::MAX_INSTANCES,
                pose_capacity: crate::animation::MAX_POSE_BLOCKS,
            },
            &submission,
        );
        assert!(
            instanced.uses.populated && !instanced.uses.indirect,
            "the instanced-mode populated frame declares population without the indirect path"
        );
    }

    /// The production derivation path, factored over the test's
    /// configurations: the submission validates and maps through the
    /// S1 adapter onto the frame flow's input set, then the frame
    /// bundle's own constructor derives the plan — the exact
    /// production path `Renderer::submit_frame` performs
    /// (`InternalFrame::new`: the same `FramePlan::derive` call and
    /// the same `foreground_vertices` computation the frame flow
    /// performed inline before A04).
    fn derive_plan(policy: SubmissionPolicy, submission: &FrameSubmission<'_>) -> FramePlan {
        let mut instance_bytes = Vec::new();
        let mut pose_bytes = Vec::new();
        let inputs = adapt_frame(
            policy,
            submission,
            3,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
        )
        .expect("the test submissions validate");
        crate::vulkan::frame::InternalFrame::new(inputs).plan
    }
}
