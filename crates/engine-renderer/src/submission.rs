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
//! every sticky setting and capture request intact. The order
//! (R1, D-EE-11 — the CA-10 deferred-commit discipline): scene/
//! preference coherence (checked by [`crate::Renderer::submit_frame`]),
//! then material indices, then the pose-index-vs-blocks admission
//! check, then the instance and pose repacks — every PURE validation
//! runs before any buffer or record mutation, so a refusal commits
//! nothing an identical retry could reuse (the one documented
//! refusal-precedence change: a population invalid in both an
//! instance field and a pose index reports the pose-index refusal
//! first; pre-R1 the instance pack error fired first).
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
///
/// **T05 (strict audit IV) — the identity records beside the
/// generation:** the raw caller generation is a PER-SOURCE counter
/// (every demo's starts at 1), so the number alone cannot say WHO
/// submitted it. `instance_source` / `pose_source` record the
/// source token whose bytes each buffer holds; the adapter resets
/// the generation record to unknown whenever a submission's token
/// differs (a second source interleaving, or the same source after
/// its storage moved), which forces the repack below — two LIVE
/// sources with colliding generation spaces cannot wrongly skip
/// (the audit's demonstrated collision, made unrepresentable at the
/// renderer's own seam, with no caller-side identity convention to
/// follow). See [`crate::proxy_instances::InstanceSourceToken`]'s
/// docs for the collision contract.
///
/// **R1 (D-EE-11) — the lifetime scope beside the token:** a
/// pointer-derived token cannot see a lifetime end — allocator
/// address recycling can hand a dropped source's exact addresses
/// (and thus its token) to a new source. `instance_digest` /
/// `pose_digest` record the CONTENT digest of the population whose
/// bytes each buffer holds (the FNV-1a over the same byte stream
/// the packing writes, taken at every repack); the adapter resets
/// the generation record to unknown whenever a would-reuse at a
/// matching token and generation carries a DIFFERENT digest — so a
/// recycled source (or any missed bump) re-admits instead of
/// inheriting the prior source's validated generation. Fail-safe in
/// both directions: a reset never vouches for anything, and a
/// re-admission is one redundant repack, never a stale skip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PopulationGenerations {
    /// The generation whose bytes the retained instance buffer
    /// holds (the packed npc+vehicle+rain population).
    pub instance: u64,
    /// The generation whose bytes the retained pose buffer holds
    /// (the packed pose-block palette).
    pub pose: u64,
    /// T05: the source whose bytes the retained instance buffer
    /// holds (the default all-zero token matches no live
    /// submission — the first submission always re-admits).
    pub instance_source: crate::proxy_instances::InstanceSourceToken,
    /// T05: the source whose bytes the retained pose buffer holds.
    pub pose_source: crate::animation::PoseSourceToken,
    /// R1: the content digest of the instance population whose
    /// bytes the retained instance buffer holds (the
    /// [`crate::proxy_instances::population_digest`] of the admitted
    /// submission, recorded at every repack). 0 = unknown — the
    /// default record; a live population's digest is the FNV offset
    /// basis at minimum, never 0.
    pub instance_digest: u64,
    /// R1: the pose half's mirror — the content digest of the pose
    /// blocks whose bytes the retained pose buffer holds (this
    /// module's `pose_blocks_digest` of the admitted submission).
    pub pose_digest: u64,
}

/// R1 (D-EE-11) — the POSE population's content digest: FNV-1a over
/// the pose blocks' f32 bit patterns in the exact stream
/// `engine_animation::pack_pose_bytes` writes (joint-major,
/// column-major, little-endian — the digest equals the FNV of the
/// packed pose bytes by construction, not by duplication). The pose
/// half's lifetime scope beside
/// [`crate::animation::PoseSourceToken`]: recorded at every pose
/// repack, consulted on every would-reuse, so a recycled pose source
/// (or a missed pose bump) re-admits instead of inheriting a prior
/// source's validated generation. Private here (the adapter is its
/// only caller; the instance half's digest lives beside its packing
/// in `proxy_instances.rs` as `population_digest`).
fn pose_blocks_digest(blocks: &[[[[f32; 4]; 4]; crate::animation::HUMANOID_JOINTS]]) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET_BASIS;
    for block in blocks {
        for matrix in block {
            for column in matrix {
                for value in column {
                    for byte in value.to_bits().to_le_bytes() {
                        hash ^= byte as u64;
                        hash = hash.wrapping_mul(FNV_PRIME);
                    }
                }
            }
        }
    }
    hash
}

/// T05 (strict audit IV) — the population halves' regeneration
/// report the frame state consumes: whether this frame's CPU gate
/// REPACKED the retained bytes (identity change, count change,
/// generation bump, or the always-dirty generation-0 shape). The
/// renderer uses it to invalidate the per-slot population stamps
/// BEFORE the frame flow runs — a slot stamp records "this slot's
/// mapped region holds THAT generation's bytes", and after a
/// repack at a COLLIDING generation number (the second source's
/// own counter presenting the same value) no stamp may keep
/// vouching: the write half rewrites every slot once, the safe
/// direction. A stable between-tick frame reports `false` for both
/// halves and keeps its skips (the R2 win, unchanged — a bump
/// already rewrote every slot once under the bare-generation rule,
/// so single-source write counts do not move).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PopulationRepacked {
    /// The instance half (npc+vehicle+rain bytes) regenerated.
    pub instance: bool,
    /// The pose half (the pose-block palette) regenerated.
    pub pose: bool,
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
    /// T05: the population halves' regeneration report — whether
    /// this frame's CPU gate repacked the retained instance/pose
    /// bytes. The renderer invalidates the frame state's population
    /// stamps on it before the frame flow runs (see
    /// [`PopulationRepacked`]); `false` on every unpopulated shape
    /// (nothing regenerated).
    pub population_repacked: PopulationRepacked,
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
/// Validation order — the populated entries' own sequence with the
/// R1/CA-10 correction: material indices vs the ATTACHED scene's
/// table (the S2 parameter — the corner routes' former
/// `lit_scene::materials().len()` read, now the handle's count),
/// then the pose-index-vs-blocks admission check, then the instance
/// repack, then the pose repack. Every refusal is a typed error
/// that leaves both repack buffers, the generation/identity
/// records, and all frame state untouched (an atomic repack, never
/// a partial buffer — and, since R1, never a committed record a
/// refused frame could hand to its own retry).
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
/// **T05 (strict audit IV) + R1 (D-EE-11) — the lifetime-scoped
/// identity contract beside the generation:** `generations` also
/// records each buffer's SOURCE token and CONTENT digest; a
/// submission whose token differs (a second population source
/// interleaving, the storage moved, or the per-kind partition
/// changed at a stable total) OR whose content digest diverges at a
/// would-be reuse (a recycled address presenting a dropped source's
/// exact token, or any missed in-place bump) resets that half's
/// generation record to unknown and re-admits — two live sources
/// with colliding generation spaces cannot wrongly skip, and
/// neither can a dead source's recycled addresses inherit its
/// validated generation. The pose-index admission check rides the
/// same evidence: it runs on every admitted/changed frame and skips
/// only the fully-reused between-tick frame (change-proportional
/// validation, the T18 direction). The output carries the halves'
/// regeneration report ([`PopulationRepacked`]) for the renderer's
/// stamp invalidation.
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
            population_repacked: PopulationRepacked::default(),
        });
    };
    let light = scene.light;
    let eye = camera.eye;
    let view_proj = camera.view_proj;
    // The population: the populated entries' own validation +
    // repack sequence, verbatim, with the repacks generation-gated
    // (P5 §3.3) and the identity records checked beside the
    // generations (T05). `None` skips every check exactly like the
    // unpopulated entries.
    let mut population_repacked = PopulationRepacked::default();
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
            // T05 (strict audit IV) + R1 (D-EE-11) — THE LIFETIME-
            // SCOPED IDENTITY GATE BESIDE THE GENERATION: derive the
            // submission's source tokens and content digests and
            // compare them against the buffers' records. On a TOKEN
            // mismatch (a second source interleaving — the audit's
            // two populations beginning at generation 1 with
            // identical counts — or the same source after its
            // storage moved, or a per-kind repartition at a stable
            // total, which the byte-length guard alone cannot see)
            // the generation record resets to UNKNOWN, so the repack
            // below re-admits. On a DIGEST divergence at a would-be
            // reuse — the token matched (same addresses, same
            // counts) but the content is not what the record's
            // generation was validated for: allocator address
            // recycling handing a dropped source's exact addresses
            // to a new source, or any missed in-place bump — the
            // record ALSO resets to UNKNOWN and re-admits. The
            // retained bytes become the NEW content's, never a
            // stale skip; the records mutate fail-safe (a later
            // refusal leaves them at unknown, vouching for nothing);
            // a recycled source with byte-identical content reuses
            // vacuously-correctly. The tokens and digests are
            // renderer-derived (no caller convention; see the token
            // types' docs for the composed collision contract).
            let instance_source = crate::proxy_instances::InstanceSourceToken::of(
                population.npc,
                population.vehicle,
                population.rain,
            );
            let pose_source = crate::animation::PoseSourceToken::of(population.npc_poses);
            let instance_digest = crate::proxy_instances::population_digest(
                population.npc,
                population.vehicle,
                population.rain,
            );
            let pose_digest = pose_blocks_digest(population.npc_poses);
            let total = population.npc.len() + population.vehicle.len() + population.rain.len();
            if generations.instance_source != instance_source {
                generations.instance_source = instance_source;
                generations.instance = 0;
            } else if population.instance_generation != 0
                && generations.instance == population.instance_generation
                && instance_bytes.len() == total * crate::proxy_instances::INSTANCE_STRIDE
                && generations.instance_digest != instance_digest
            {
                // R1: the would-reuse's content diverges from the
                // recorded digest — the recycled-address collision
                // or the missed bump. Re-admit (fail-safe repack).
                generations.instance = 0;
            }
            if generations.pose_source != pose_source {
                generations.pose_source = pose_source;
                generations.pose = 0;
            } else if population.pose_generation != 0
                && generations.pose == population.pose_generation
                && pose_bytes.len() == population.npc_poses.len() * crate::animation::POSE_STRIDE
                && generations.pose_digest != pose_digest
            {
                generations.pose = 0;
            }
            // T18/T05 — THE POSE-INDEX ADMISSION PREDICATE: the
            // per-npc pose-index scan below runs when the
            // population is (re)admitted, not on a fully-reused
            // frame (validation is change-proportional — the audit's
            // "validate when the trusted resource is constructed or
            // changed, not every unchanged frame"). The predicate is
            // exactly the conjunction of both repack gates' Reused
            // conditions as they stand AFTER the identity gate
            // above (token match ⇒ the records were not reset; a
            // digest divergence ⇒ they were), so it is true
            // precisely when both halves will reuse — the frame
            // whose bytes and whose pose-block count were both
            // validated at admission. The debug_assert at the
            // arm's tail pins the equivalence to the actual
            // decisions (a drift tripwire, not a check of caller
            // honesty).
            let full_reuse = population.instance_generation != 0
                && population.pose_generation != 0
                && generations.instance == population.instance_generation
                && generations.pose == population.pose_generation
                && instance_bytes.len() == total * crate::proxy_instances::INSTANCE_STRIDE
                && pose_bytes.len() == population.npc_poses.len() * crate::animation::POSE_STRIDE;
            // The pose contract (the capture-refusal discipline):
            // every POPULATED npc's pose index must select a supplied
            // block; the block-count bound is the pose repack's own
            // typed refusal below. The check runs on every ADMITTED or
            // CHANGED frame — generation-gated or not (a
            // declared-identical population that outgrew its block
            // list still refuses: a count change is a token change,
            // which the identity gate above turned into a
            // re-admission). Only the fully-reused between-tick
            // frame skips it (its indices were validated at the
            // admission this frame's records still vouch for).
            //
            // R1 (D-EE-11, Arm B) — THE CA-10 DEFERRED-COMMIT
            // DISCIPLINE: this check runs BEFORE both repacks, so an
            // unsuccessful admission establishes NOTHING — no
            // buffer byte, no generation record, no digest record.
            // Pre-R1 it ran between the instance repack and the pose
            // repack, and the instance repack had already COMMITTED
            // `generations.instance` and the repacked bytes: an
            // identical retry then computed `full_reuse` against the
            // record the REFUSAL itself established, skipped this
            // check, and admitted the invalid population (the
            // reviewer's regression witness — an unsuccessful
            // admission must not establish a reusable validated
            // generation). Hoisting the check restores the module's
            // own validated-WHOLE-before-any-state-is-touched
            // invariant for the one admission check that is
            // conditionally skipped; the repacks' own typed
            // refusals stay atomic (validation-before-mutation
            // inside each), and the identity gate's resets above
            // only ever move records toward unknown (fail-safe).
            // NOTE (a documented refusal-precedence change): a
            // population invalid in BOTH an instance field and a
            // pose index now reports the pose-index refusal first
            // (pre-R1: the instance pack error); no test pinned the
            // former precedence.
            if !full_reuse
                && population
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
            // R1: the record's digest follows the bytes — written
            // ONLY together with a successful regeneration (the
            // repack is atomic: a typed refusal above leaves the
            // prior record intact, and it truthfully describes the
            // untouched buffer; a reset record stays at unknown,
            // vouching for nothing).
            if instance_decision == crate::proxy_instances::GenerationDecision::Repacked {
                generations.instance_digest = instance_digest;
            }
            let pose_decision = crate::animation::repack_poses_generation(
                pose_bytes,
                population.npc_poses,
                policy.pose_capacity,
                population.pose_generation,
                &mut generations.pose,
            )
            .map_err(RendererError::PosePack)?;
            // R1: the pose half's digest record, the same
            // write-only-with-the-bytes discipline.
            if pose_decision == crate::proxy_instances::GenerationDecision::Repacked {
                generations.pose_digest = pose_digest;
            }
            debug_assert!(
                full_reuse
                    == (instance_decision == crate::proxy_instances::GenerationDecision::Reused
                        && pose_decision == crate::proxy_instances::GenerationDecision::Reused),
                "the pose-index admission predicate and the repack gates \
                 disagree — the identity layer drifted from the core gates"
            );
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
            // T05: the regeneration report the renderer's stamp
            // invalidation consumes (see [`PopulationRepacked`]).
            population_repacked.instance =
                instance_decision == crate::proxy_instances::GenerationDecision::Repacked;
            population_repacked.pose =
                pose_decision == crate::proxy_instances::GenerationDecision::Repacked;
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
        population_repacked,
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
            population_repacked: PopulationRepacked::default(),
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
            population_repacked: PopulationRepacked::default(),
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

    /// T05 (strict audit IV) — THE ADAPTER-LEVEL COLLISION WITNESS:
    /// two population SOURCES with colliding generation spaces (each
    /// an independently owned crowd whose counters start at 1 — the
    /// audit's exact scenario, `corner_demo`'s shape submitted by a
    /// second owner) submit in succession at the SAME nonzero
    /// generation with the SAME per-kind counts and DIFFERENT
    /// content. The raw-generation CPU gate alone returns Reused and
    /// the retained buffer keeps the FIRST source's bytes — in
    /// debug/test the missed-bump verify panics on exactly this
    /// (RED WITNESS: this test's pre-fix failure IS that panic), and
    /// in a release production frame nothing catches it: the GPU
    /// draws the wrong crowd. The identity contract (the source
    /// records beside the generation) must make the second source
    /// RE-ADMIT: repack, and the retained bytes become source B's.
    #[test]
    fn colliding_generation_spaces_re_admit_the_second_source() {
        use crate::animation::{HUMANOID_JOINTS, rest_pose_block};
        use crate::proxy_instances::{InstanceTransform, ProxyInstance, repack_populations};
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
        // Two sources' instance lists — same per-kind counts (2 npc,
        // 1 vehicle), different content, separately owned storage.
        let instance = |x: f32, y: f32, turn: u16, tint: [f32; 3]| ProxyInstance {
            transform: InstanceTransform {
                pos: [x, 0.02, y],
                turn,
                scale: 1.0,
            },
            tint,
            alpha: 1.0,
            pose_index: 0,
        };
        let source_a = vec![
            instance(1.0, -2.0, 1000, [0.9, 0.9, 0.9]),
            instance(-3.0, 4.0, 40000, [0.8, 0.6, 0.5]),
            instance(7.0, 1.0, 8192, [0.2, 0.4, 0.9]),
        ];
        let source_b = vec![
            instance(12.0, 5.0, 2000, [0.1, 0.9, 0.3]),
            instance(-8.0, -6.0, 30000, [0.5, 0.5, 0.1]),
            instance(9.0, 3.0, 4096, [0.3, 0.2, 0.7]),
        ];
        // Each source's own pose palette (the same rest content —
        // the instance half is this witness's subject).
        let poses_a: Vec<[[[f32; 4]; 4]; HUMANOID_JOINTS]> =
            vec![rest_pose_block(), rest_pose_block()];
        let poses_b: Vec<[[[f32; 4]; 4]; HUMANOID_JOINTS]> =
            vec![rest_pose_block(), rest_pose_block()];
        // The submission builder: a local fn (a closure cannot
        // express the shared parameter lifetime — the fuzz test's
        // own note), taking the population slices as parameters so
        // the same construction serves both sources and the stable
        // resubmission.
        fn build<'a>(
            instances: &'a [ProxyInstance],
            poses: &'a [[[[f32; 4]; 4]; HUMANOID_JOINTS]],
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
                        npc: &instances[..2],
                        npc_poses: poses,
                        npc_material: 0,
                        vehicle: &instances[2..],
                        vehicle_material: 0,
                        rain: &[],
                        rain_material: 0,
                        instance_generation: 5,
                        pose_generation: 5,
                    }),
                    sky: None,
                    post: false,
                    wet: None,
                }),
            }
        }
        let mut instance_bytes = Vec::new();
        let mut pose_bytes = Vec::new();
        let mut generations = PopulationGenerations::default();
        // Source A admitted at generation 5.
        let submission_a = build(&source_a, &poses_a, matrix, light);
        adapt_frame(
            policy,
            &submission_a,
            3,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
            &mut generations,
        )
        .expect("source A validates and admits");
        let a_bytes = instance_bytes.clone();
        // Source B — its OWN generation space, also at 5, the same
        // counts, different content — must RE-ADMIT (repack), not
        // reuse A's bytes. Pre-fix this is the RED arm: the raw gate
        // reuses and the debug missed-bump verify panics (MISSED
        // BUMP) — the release shape would silently draw A's crowd.
        let submission_b = build(&source_b, &poses_b, matrix, light);
        adapt_frame(
            policy,
            &submission_b,
            3,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
            &mut generations,
        )
        .expect("source B re-admits through the identity contract");
        let mut b_expected = Vec::new();
        repack_populations(&mut b_expected, &source_b[..2], &source_b[2..], &[]).unwrap();
        assert_eq!(
            instance_bytes, b_expected,
            "the retained bytes are source B's, not A's stale crowd"
        );
        assert_ne!(instance_bytes, a_bytes);
        // And the new source's between-tick frames keep their R2
        // reuse (the identity layer must not over-fire): a second B
        // submission at the same generation reuses byte-identically.
        let submission_b_again = build(&source_b, &poses_b, matrix, light);
        adapt_frame(
            policy,
            &submission_b_again,
            3,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
            &mut generations,
        )
        .expect("source B's stable resubmission validates");
        assert_eq!(instance_bytes, b_expected, "the reuse regenerated nothing");
    }

    /// R1 (D-EE-11, Arm A) — THE ADDRESS-RECYCLING WITNESS (the T05
    /// token's documented residual): source A is admitted at
    /// generation 5 and DROPPED; source B — a different population
    /// whose Vec lands at A's EXACT recycled addresses (the
    /// deterministic harness: two same-layout allocations with an
    /// explicit drop between; the immediate same-size realloc is the
    /// allocator's observed behavior on this platform) — submits at
    /// its own colliding generation 5 with the same per-kind counts
    /// and DIFFERENT content. The pointer-derived token alone cannot
    /// distinguish the sources (that is the documented residual);
    /// the lifetime-scoped identity beside it must: B RE-ADMITS and
    /// the retained bytes become B's. Pre-fix this is the RED arm —
    /// the recycled token matched, the raw gate reused A's bytes,
    /// and the missed-bump verify panicked (the red run's captured
    /// panic is itself the proof the recycling presented A's token);
    /// in a release production frame the same state rendered A's
    /// stale crowd silently.
    #[test]
    fn recycled_address_cannot_inherit_a_prior_sources_generation() {
        use crate::animation::{HUMANOID_JOINTS, rest_pose_block};
        use crate::proxy_instances::{
            InstanceSourceToken, InstanceTransform, ProxyInstance, repack_populations,
        };
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
        let instance = |x: f32, y: f32, turn: u16, tint: [f32; 3]| ProxyInstance {
            transform: InstanceTransform {
                pos: [x, 0.02, y],
                turn,
                scale: 1.0,
            },
            tint,
            alpha: 1.0,
            pose_index: 0,
        };
        // The stable pose palette — this witness's subject is the
        // instance half, so the pose source stays LIVE and unchanged
        // (its own half keeps its honest reuse throughout).
        let poses: Vec<[[[f32; 4]; 4]; HUMANOID_JOINTS]> =
            vec![rest_pose_block(), rest_pose_block()];
        fn build<'a>(
            instances: &'a [ProxyInstance],
            poses: &'a [[[[f32; 4]; 4]; HUMANOID_JOINTS]],
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
                        npc: &instances[..2],
                        npc_poses: poses,
                        npc_material: 0,
                        vehicle: &instances[2..],
                        vehicle_material: 0,
                        rain: &[],
                        rain_material: 0,
                        instance_generation: 5,
                        pose_generation: 5,
                    }),
                    sky: None,
                    post: false,
                    wet: None,
                }),
            }
        }
        let mut instance_bytes = Vec::new();
        let mut pose_bytes = Vec::new();
        let mut generations = PopulationGenerations::default();
        let mut a_packed = Vec::new();
        let a_token;
        {
            let source_a = vec![
                instance(1.0, -2.0, 1000, [0.9, 0.9, 0.9]),
                instance(-3.0, 4.0, 40000, [0.8, 0.6, 0.5]),
                instance(7.0, 1.0, 8192, [0.2, 0.4, 0.9]),
            ];
            let submission_a = build(&source_a, &poses, matrix, light);
            adapt_frame(
                policy,
                &submission_a,
                3,
                &[],
                &mut instance_bytes,
                &mut pose_bytes,
                &mut generations,
            )
            .expect("source A validates and admits");
            repack_populations(&mut a_packed, &source_a[..2], &source_a[2..], &[]).unwrap();
            a_token = InstanceSourceToken::of(&source_a[..2], &source_a[2..], &[]);
        }
        // source_a is DROPPED here — its 3-instance heap block returns
        // to the allocator, and the immediate same-layout allocation
        // below lands at the exact same addresses on this platform's
        // allocator (the recycling the token cannot see).
        let source_b = vec![
            instance(12.0, 5.0, 2000, [0.1, 0.9, 0.3]),
            instance(-8.0, -6.0, 30000, [0.5, 0.5, 0.1]),
            instance(9.0, 3.0, 4096, [0.3, 0.2, 0.7]),
        ];
        let b_token = InstanceSourceToken::of(&source_b[..2], &source_b[2..], &[]);
        let recycled = a_token == b_token;
        let submission_b = build(&source_b, &poses, matrix, light);
        adapt_frame(
            policy,
            &submission_b,
            3,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
            &mut generations,
        )
        .expect("source B re-admits through the lifetime-scoped identity");
        let mut b_expected = Vec::new();
        repack_populations(&mut b_expected, &source_b[..2], &source_b[2..], &[]).unwrap();
        assert_eq!(
            instance_bytes, b_expected,
            "the retained bytes are source B's, never A's stale crowd \
             (the allocator recycled A's exact addresses: {recycled})"
        );
        assert_ne!(instance_bytes, a_packed);
        // B's between-tick frames keep the R2 reuse (the composed
        // identity must not over-fire on the new source's stable
        // frames).
        let submission_b_again = build(&source_b, &poses, matrix, light);
        adapt_frame(
            policy,
            &submission_b_again,
            3,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
            &mut generations,
        )
        .expect("source B's stable resubmission validates");
        assert_eq!(instance_bytes, b_expected, "the reuse regenerated nothing");
    }

    /// R1 — THE CONTENT-DIGEST ROAD, deterministic (no allocator
    /// cooperation): the SAME source storage (the token matches),
    /// the SAME generation, the content CHANGED in place — the
    /// missed-bump shape, and exactly the state a recycled address
    /// presents under a matching token. This is the release-side
    /// hazard T05 documented as its narrow trade (risk 3: a missed
    /// in-place bump renders stale in release; only the debug verify
    /// panicked); the lifetime-scoped identity closes it: the
    /// recorded content digest diverges, the half re-admits, and the
    /// retained bytes are the CURRENT population's. Pre-fix RED: the
    /// raw gate reused and the missed-bump verify panicked here.
    #[test]
    fn missed_bump_at_a_matching_token_re_admits_through_the_content_digest() {
        use crate::animation::{HUMANOID_JOINTS, rest_pose_block};
        use crate::proxy_instances::{InstanceTransform, ProxyInstance, repack_populations};
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
        let instance = |x: f32, y: f32, turn: u16, tint: [f32; 3]| ProxyInstance {
            transform: InstanceTransform {
                pos: [x, 0.02, y],
                turn,
                scale: 1.0,
            },
            tint,
            alpha: 1.0,
            pose_index: 0,
        };
        let poses: Vec<[[[f32; 4]; 4]; HUMANOID_JOINTS]> =
            vec![rest_pose_block(), rest_pose_block()];
        fn build<'a>(
            instances: &'a [ProxyInstance],
            poses: &'a [[[[f32; 4]; 4]; HUMANOID_JOINTS]],
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
                        npc: &instances[..2],
                        npc_poses: poses,
                        npc_material: 0,
                        vehicle: &instances[2..],
                        vehicle_material: 0,
                        rain: &[],
                        rain_material: 0,
                        instance_generation: 5,
                        pose_generation: 5,
                    }),
                    sky: None,
                    post: false,
                    wet: None,
                }),
            }
        }
        let mut source = vec![
            instance(1.0, -2.0, 1000, [0.9, 0.9, 0.9]),
            instance(-3.0, 4.0, 40000, [0.8, 0.6, 0.5]),
            instance(7.0, 1.0, 8192, [0.2, 0.4, 0.9]),
        ];
        let mut instance_bytes = Vec::new();
        let mut pose_bytes = Vec::new();
        let mut generations = PopulationGenerations::default();
        let submission = build(&source, &poses, matrix, light);
        adapt_frame(
            policy,
            &submission,
            3,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
            &mut generations,
        )
        .expect("the population admits at generation 5");
        let admitted = instance_bytes.clone();
        // THE MISSED BUMP: the content changes in place, the declared
        // generation does not — the token still matches (same
        // storage, same counts), so only the content identity can
        // catch it.
        source[1].transform.pos[0] += 9.0;
        source[1].tint = [0.3, 0.3, 0.3];
        let submission = build(&source, &poses, matrix, light);
        adapt_frame(
            policy,
            &submission,
            3,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
            &mut generations,
        )
        .expect("the diverged content re-admits through the content digest");
        let mut expected = Vec::new();
        repack_populations(&mut expected, &source[..2], &source[2..], &[]).unwrap();
        assert_eq!(
            instance_bytes, expected,
            "the retained bytes are the CURRENT population's, never the stale ones"
        );
        assert_ne!(instance_bytes, admitted);
        // The now-honest between-tick frame keeps its R2 reuse.
        let submission = build(&source, &poses, matrix, light);
        adapt_frame(
            policy,
            &submission,
            3,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
            &mut generations,
        )
        .expect("the stable resubmission validates");
        assert_eq!(instance_bytes, expected, "the reuse regenerated nothing");
    }

    /// R1 — THE POSE HALF'S DIGEST ROAD: the same pose storage (the
    /// pose token matches), the same pose generation, but the
    /// evaluated blocks changed in place (the pose-side missed bump —
    /// and the exact shape a recycled pose source presents). The
    /// pose half's recorded content digest diverges → re-admit → the
    /// retained pose bytes are the CURRENT blocks'; the instance
    /// half (unchanged, honestly declared) keeps its reuse through
    /// the same frames. Pre-fix RED: the raw pose gate reused and
    /// the pose missed-bump verify panicked here.
    #[test]
    fn pose_missed_bump_at_a_matching_token_re_admits() {
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
        let instance = |x: f32, y: f32, turn: u16, pose_index: u32| ProxyInstance {
            transform: InstanceTransform {
                pos: [x, 0.02, y],
                turn,
                scale: 1.0,
            },
            tint: [0.8, 0.7, 0.6],
            alpha: 1.0,
            pose_index,
        };
        let instances = vec![
            instance(1.0, -2.0, 1000, 0),
            instance(-3.0, 4.0, 40000, 1),
            instance(7.0, 1.0, 8192, 0),
        ];
        fn build<'a>(
            instances: &'a [ProxyInstance],
            poses: &'a [[[[f32; 4]; 4]; HUMANOID_JOINTS]],
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
                        npc: &instances[..2],
                        npc_poses: poses,
                        npc_material: 0,
                        vehicle: &instances[2..],
                        vehicle_material: 0,
                        rain: &[],
                        rain_material: 0,
                        instance_generation: 5,
                        pose_generation: 5,
                    }),
                    sky: None,
                    post: false,
                    wet: None,
                }),
            }
        }
        let mut poses = vec![rest_pose_block(), walker_at(9000)];
        let mut instance_bytes = Vec::new();
        let mut pose_bytes = Vec::new();
        let mut generations = PopulationGenerations::default();
        let submission = build(&instances, &poses, matrix, light);
        adapt_frame(
            policy,
            &submission,
            3,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
            &mut generations,
        )
        .expect("the population admits at generation 5");
        let admitted = pose_bytes.clone();
        // THE POSE-SIDE MISSED BUMP: the evaluated blocks change in
        // place, the declared pose generation does not.
        poses[1] = walker_at(-9000);
        let submission = build(&instances, &poses, matrix, light);
        adapt_frame(
            policy,
            &submission,
            3,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
            &mut generations,
        )
        .expect("the diverged pose content re-admits through the content digest");
        let mut expected = Vec::new();
        crate::animation::repack_poses(&mut expected, &poses).unwrap();
        assert_eq!(
            pose_bytes, expected,
            "the retained pose bytes are the CURRENT blocks', never the stale ones"
        );
        assert_ne!(pose_bytes, admitted);
        // The now-honest between-tick frame keeps its R2 reuse.
        let submission = build(&instances, &poses, matrix, light);
        adapt_frame(
            policy,
            &submission,
            3,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
            &mut generations,
        )
        .expect("the stable resubmission validates");
        assert_eq!(pose_bytes, expected, "the reuse regenerated nothing");
    }

    /// R1 (D-EE-11, Arm B) — THE SUBMISSION-ORDERING WITNESS (the
    /// reviewer's regression hypothesis): an INVALID changed
    /// instance population (a bumped generation 6 whose npc carries
    /// an out-of-range pose index — field-valid, so the instance
    /// repack itself succeeds), an UNCHANGED pose source and
    /// generation, an initial refusal (the pose-index admission
    /// rejects), then an IDENTICAL retry. Pre-fix RED: the instance
    /// repack ran BEFORE the pose-index check, so the refused frame
    /// had already COMMITTED `generations.instance = 6` and the
    /// repacked `instance_bytes` — the identical retry then computed
    /// `full_reuse = true` against the record the REFUSAL
    /// established, the pose-index check was SKIPPED, and the
    /// identical invalid population ADMITTED (an out-of-range GPU
    /// pose-block read). An unsuccessful admission must not
    /// establish a reusable validated generation. Post-fix (the
    /// CA-10 deferred-commit discipline): the pose-index check runs
    /// BEFORE any buffer or record mutation, the refusal leaves the
    /// prior validated state fully intact, identical retries refuse
    /// identically, and the caller's FIXED population (valid
    /// indices, the same generation 6) admits cleanly.
    #[test]
    fn an_unsuccessful_admission_leaves_no_reusable_validated_generation() {
        use crate::animation::{HUMANOID_JOINTS, rest_pose_block};
        use crate::proxy_instances::{InstanceTransform, ProxyInstance, repack_populations};
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
        let instance = |x: f32, y: f32, turn: u16, pose_index: u32| ProxyInstance {
            transform: InstanceTransform {
                pos: [x, 0.02, y],
                turn,
                scale: 1.0,
            },
            tint: [0.8, 0.7, 0.6],
            alpha: 1.0,
            pose_index,
        };
        let poses: Vec<[[[f32; 4]; 4]; HUMANOID_JOINTS]> =
            vec![rest_pose_block(), rest_pose_block()];
        fn build<'a>(
            instances: &'a [ProxyInstance],
            poses: &'a [[[[f32; 4]; 4]; HUMANOID_JOINTS]],
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
                        npc: &instances[..2],
                        npc_poses: poses,
                        npc_material: 0,
                        vehicle: &instances[2..],
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
        let mut source = vec![
            instance(1.0, -2.0, 1000, 0),
            instance(-3.0, 4.0, 40000, 1),
            instance(7.0, 1.0, 8192, 0),
        ];
        let mut instance_bytes = Vec::new();
        let mut pose_bytes = Vec::new();
        let mut generations = PopulationGenerations::default();
        // The honest history: the source is admitted at instance
        // generation 5 / pose generation 3 with VALID indices.
        let submission = build(&source, &poses, 5, 3, matrix, light);
        adapt_frame(
            policy,
            &submission,
            3,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
            &mut generations,
        )
        .expect("the valid population admits");
        let admitted_bytes = instance_bytes.clone();
        let admitted_digest = generations.instance_digest;
        assert_eq!(generations.instance, 5);
        // THE INVALID CHANGED POPULATION: the content moves (pos and
        // the npc's pose index), the generation honestly bumps to 6,
        // but npc[0]'s pose index now falls outside the 2 supplied
        // blocks — the pose-index admission must refuse, and the
        // refusal must leave the prior validated state intact.
        source[0].transform.pos[0] += 4.0;
        source[0].pose_index = 9;
        let submission = build(&source, &poses, 6, 3, matrix, light);
        let error = adapt_frame(
            policy,
            &submission,
            3,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
            &mut generations,
        )
        .expect_err("the out-of-range pose index refuses");
        let RendererError::Vulkan(text) = &error else {
            panic!("the refusal is a Vulkan error: {error:?}");
        };
        assert_eq!(
            text,
            "a npc pose index falls outside the supplied pose blocks \
              (2 blocks, highest index 9)"
        );
        // CA-10: nothing was committed by the refused frame — the
        // records still vouch for exactly the PRIOR validated state.
        assert_eq!(
            generations.instance, 5,
            "an unsuccessful admission must not advance the record"
        );
        assert_eq!(
            generations.instance_digest, admitted_digest,
            "an unsuccessful admission must not commit new content"
        );
        assert_eq!(
            instance_bytes, admitted_bytes,
            "an unsuccessful admission must not touch the retained bytes"
        );
        // THE IDENTICAL RETRY: same population, same generations —
        // it must refuse IDENTICALLY (pre-fix RED: the refused frame
        // had already established the reusable record, the retry's
        // full_reuse skipped the check, and this expect panicked).
        let submission = build(&source, &poses, 6, 3, matrix, light);
        let retry_error = adapt_frame(
            policy,
            &submission,
            3,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
            &mut generations,
        )
        .expect_err(
            "the identical retry refuses identically — a refused \
                 frame must not have established a validated generation",
        );
        assert_eq!(
            format!("{retry_error:?}"),
            format!("{error:?}"),
            "the retry's refusal is the same refusal"
        );
        assert_eq!(generations.instance, 5);
        assert_eq!(instance_bytes, admitted_bytes);
        // THE CALLER'S FIX: the pose index returns in range at the
        // SAME bumped generation 6 — the admission now succeeds, the
        // repack runs, and the records advance WITH the success.
        source[0].pose_index = 1;
        let submission = build(&source, &poses, 6, 3, matrix, light);
        adapt_frame(
            policy,
            &submission,
            3,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
            &mut generations,
        )
        .expect("the fixed population admits at generation 6");
        let mut expected = Vec::new();
        repack_populations(&mut expected, &source[..2], &source[2..], &[]).unwrap();
        assert_eq!(instance_bytes, expected);
        assert_eq!(generations.instance, 6, "the success commits the record");
        // And the fixed population's between-tick frame keeps its
        // R2 reuse (the CA-10 discipline costs nothing on the happy
        // path).
        let submission = build(&source, &poses, 6, 3, matrix, light);
        adapt_frame(
            policy,
            &submission,
            3,
            &[],
            &mut instance_bytes,
            &mut pose_bytes,
            &mut generations,
        )
        .expect("the stable resubmission validates");
        assert_eq!(instance_bytes, expected, "the reuse regenerated nothing");
        assert_eq!(generations.instance, 6);
    }
}
