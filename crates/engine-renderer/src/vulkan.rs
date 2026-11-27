//! The Vulkan backend — **all** engine-renderer unsafe code lives here
//! (Stage 2 authorization, item 1). Everything else in this crate stays
//! `unsafe`-free; `engine-core`'s `unsafe_code = deny` is untouched.
//!
//! # Invariants manifest
//!
//! **Ownership** — every raw Vulkan object is owned by exactly one struct
//! field, created in `new`/`recreate` and destroyed exactly once, in
//! [`VulkanContext::destroy`], [`SwapchainState::destroy`],
//! [`PipelineState::destroy`], or [`FrameState::destroy`]. ash handles are
//! non-owning pointers: dropping them destroys nothing, so a handle set to
//! `*::null()` after destruction marks it dead. `Renderer` (the safe
//! wrapper) owns all four states and calls their `destroy` methods from
//! its `Drop` in reverse creation order after `device_wait_idle`.
//!
//! **Lifetime** — every Vulkan call happens while its owning context is
//! alive. The SDL-created surface outlives the instance teardown by being
//! destroyed (via `khr::surface::Instance::destroy_surface`) **before**
//! `destroy_instance`. No borrowed Vulkan reference escapes this module;
//! the safe API exposes only copies, counts, and strings.
//!
//! **Synchronization** — classic frames-in-flight (2 slots): each slot owns
//! one command buffer, one `image_available` semaphore, and one
//! signaled-at-submit fence (`in_flight`). `render_frame` waits/resets the
//! slot fence before acquire, guards against acquiring an image another
//! in-flight submission still reads with per-image fences
//! (`images_in_flight`), and submits with `image_available` waited at
//! `COLOR_ATTACHMENT_OUTPUT`. **Presentation-wait semaphores are indexed by
//! swapchain image, not frame slot** (corrective finding C): the submit
//! fence proves only that the graphics work — including the acquire-wait —
//! finished, never that the presentation engine finished *waiting* on the
//! present semaphore, so slot-indexed reuse could signal a semaphore with
//! an outstanding present wait; indexing by the acquired image ties reuse
//! to re-acquisition of that image, and `device_wait_idle` before every
//! destroy/recreate drains outstanding presents. **Each swapchain image
//! owns its depth attachment** (finding A), so concurrent in-flight
//! submissions into different images never share depth state. **The
//! overlay vertex buffer for each slot grows on demand** after that
//! slot's fence wait. Other slots retain their own allocations. The
//! command pool allows individual command-buffer resets (finding D), and
//! the per-slot fence proves a buffer is no longer pending when its
//! re-record begins. GPU timestamps are written only when the queue family
//! supports them and read only after the slot fence proves that slot's
//! queries resolved at least once (finding E).
//!
//! **Lit materials** - the optional corner owns a static mesh, three image sets
//! and a coherent UBO at device lifetime. The dynamic frame offset selects the
//! fenced slot. Each material descriptor points past all slots into a read-only tail.
//! The render-pass-dependent lit pipeline is released before those descriptors.
//!
//! **Instanced proxies (6-E)** — the optional proxy state (`PipelineState::
//! proxy`, lazily created at the first populated corner frame) owns two
//! device-local mesh VBOs (staged, fence-waited uploads) and one persistently
//! mapped host-visible instance buffer with a disjoint region per frame slot.
//! Slot S's region is written only inside the frame flow that first waited
//! slot S's fence, immediately before recording the draw that reads it. The
//! proxy pipeline and layout die with their `PipelineState`; a replacement
//! re-ensures identical resources before installation (finding A). Proxy
//! descriptor sets are the CORNER's material sets (shared ownership — never
//! destroyed by the proxy state), bound with the corner's per-slot FrameParams
//! dynamic offset.
//!
//! **Indirect proxy commands (6-E2)** — the proxy state's OPTIONAL indirect
//! member (one level lazier: created at the first INDIRECT-mode populated
//! frame only, so the direct path's allocation pins pay nothing) is one
//! persistently mapped host-visible INDIRECT_BUFFER with one 32-byte command
//! block per frame slot (npc command at +0, vehicle at +16 — the repack's
//! fixed kind order; the bytes are the pure `proxy_indirect` packing with
//! `firstVertex`/`firstInstance` pinned zero, so no `drawIndirectFirstInstance`
//! feature is required). The write discipline is the instance buffer's own:
//! slot S's block is written only inside the frame flow that first waited
//! slot S's fence, immediately before recording the `vkCmdDrawIndirect`
//! (drawCount 1 per kind) that reads it; the HOST_COHERENT write is visible
//! to that submission's draws by queue-submit ordering. A pipeline
//! replacement re-ensures the command buffer with the same resources
//! (finding A). The buffer is destroyed only while the device is idle,
//! after the draws reading it.
//!
//! **Destruction order** — `device_wait_idle` → command buffers/pool,
//! present semaphores, acquire semaphores, fences, query pool → swapchain
//! framebuffers, views, per-image depth views/images/memories, swapchain
//! (framebuffers **before** the render pass they reference) → pipelines,
//! layouts, render pass and scene buffers → atlas pool, layout, sampler, image
//! view, image and memory → device. Frame slots release
//! their mapped overlay buffers during frame-state teardown. Failed
//! swapchain replacements release the retired chain and leave a pending
//! recreation for the next renderable frame. At context teardown (after
//! every PipelineState) the retained skinned state releases BEFORE the
//! corner it binds (reverse creation order).

// Unsafe containment (Stage 2 authorization, item 1): this module-level
// allow overrides the crate-level `#![deny(unsafe_code)]` in lib.rs —
// deny is overridable at closer scope, and this is the ONLY override.
#![allow(unsafe_code)]

// The P4 S4 responsibility split (design §5.1): this file is the mod
// root + the shared error type, the shared fence-wait bound, and the
// re-export surface below; the responsibilities live in the submodule
// tree. The split is a PURE MOVE — every `crate::vulkan::<name>` and
// `super::<name>` path the crate used before S4 still resolves here.
mod capture;
mod context;
mod frame;
mod pipelines;
mod resources;
mod swapchain;
mod uploads;

// The pass-resource family modules (design §5.1's passes row): the
// files live under `vulkan/passes/` but stay declared HERE — not under
// a `passes` parent module — so their module paths
// (`crate::vulkan::<name>`) and their own `super::` references into
// this root are exactly what they were when the files sat at `src/`.
#[path = "vulkan/passes/corner.rs"]
pub(crate) mod corner;
#[path = "descriptors.rs"]
pub mod descriptors;
#[path = "frame_plan.rs"]
pub(crate) mod frame_plan;
#[path = "frame_prepare.rs"]
pub(crate) mod frame_prepare;
#[path = "vulkan/passes/post_draw.rs"]
pub(crate) mod post_draw;
#[path = "vulkan/passes/proxy_draw.rs"]
pub(crate) mod proxy_draw;
#[path = "vulkan/passes/reflection_draw.rs"]
pub(crate) mod reflection_draw;
#[path = "vulkan/passes/scene_record.rs"]
pub(crate) mod scene_record;
#[path = "vulkan/passes/shadow_draw.rs"]
pub(crate) mod shadow_draw;
#[path = "vulkan/passes/skinned_draw.rs"]
pub(crate) mod skinned_draw;
#[path = "vulkan/passes/sky_draw.rs"]
pub(crate) mod sky_draw;
#[path = "textures.rs"]
pub mod textures;

/// The P4 S5 shader/host layout pins (design §6.4): the WGSL-side
/// uniform-struct layouts the build script resolves from the COMPOSED
/// variant sources (member offsets from the parsed IR — the same
/// values the SPIR-V writer emits — sizes through naga's WGSL
/// memory-layout table), cross-checked for agreement across every
/// variant consumer before emission. The host mirrors
/// ([`corner::FrameParams`], [`shadow_draw::ShadowFrameParams`], the
/// material tail's `[f32; 4]`, the depth pass's light-matrix write)
/// assert against these constants: a layout drift between the Rust
/// packing and the WGSL consumption fails the BUILD, on whichever
/// side moved. Generated file — never edited by hand.
pub(crate) mod shader_layout_pins {
    include!(concat!(env!("OUT_DIR"), "/layout_pins.rs"));
}

// The pre-split surface of this module, re-exported so no sibling
// (renderer.rs, submission.rs, scene_description.rs, lib.rs) and no
// child (the pass families, frame_prepare, frame_plan, scene_record)
// observes the split.
pub use capture::{CaptureFormat, CapturedFrame};
pub(crate) use capture::{
    drain_validation_messages, environment_pending, last_capture_refusal, request_capture,
    request_shadow_readback, take_capture, take_shadow_readback,
};
pub(crate) use context::{VulkanContext, device_idle, pick_depth_format, pick_surface_format};
pub(crate) use frame::{FrameOutcome, FrameState, render_frame};
pub(crate) use pipelines::PipelineState;
pub(crate) use swapchain::{
    SurfaceProbeDecision, SwapchainState, consume_recreate, mark_suboptimal, surface_probe_decision,
};
// Names only this module tree reaches (private bindings: a child sees
// its ancestors' private names, so the pass families' `super::<name>`
// import lines are unchanged).
use ash::vk;
use context::depth_aspect;
use resources::{
    create_device_local_buffer_with_data, create_mapped_buffer, create_shader_module,
    find_memory_type, with_upload_commands,
};
use swapchain::{drawable_extent, recreate_swapchain};
use textures::{SamplerPool, Texture};

/// Upper bound for any host-side fence wait (nanoseconds). A stuck
/// submission must surface as an error the application can report,
/// never as an unresponsive window (rig finding 2026-10-20: pacing
/// faster than a vsync-locked present mode can drain wedged the frame
/// loop — 240 fps pacing against FIFO on a 60 Hz display).
const WAIT_TIMEOUT_NS: u64 = 2_000_000_000;

/// Why the renderer could not initialize — environment limitations are
/// distinct from application/runtime errors (Stage 2 authorization, item 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RendererError {
    /// The Vulkan loader library is absent (no Vulkan runtime installed).
    NoLoader(String),
    /// No physical device exposes the required queue + swapchain support.
    NoDevice(String),
    /// A Vulkan call failed at runtime.
    Vulkan(String),
    /// SDL could not provide the window's Vulkan surface.
    Surface(String),
    /// A frame-capture request was refused because another capture is
    /// already outstanding (pending, or recorded but unconsumed) — the
    /// bounded single-flight lifecycle policy (the post-orientation
    /// hardening): a second copy could otherwise overwrite the earlier
    /// readback's destination while it is still unconsumed.
    CaptureOutstanding,
    /// The swapchain's surface format is outside the capture API's
    /// supported 4-byte-per-pixel RGBA8/BGRA8 family — refused with a
    /// typed error BEFORE any GPU copy command is recorded (a fallback
    /// surface format such as R16G16B16A16_SFLOAT needs 8 bytes per
    /// pixel and would overflow the capture destination). The
    /// renderer's own surface-format fallback behavior is unaffected:
    /// only CAPTURE refuses.
    CaptureUnsupportedFormat(String),
    /// The capture destination's byte size overflowed checked
    /// arithmetic (width × height × the format footprint).
    CaptureSizingOverflow,
    /// A pending capture was dropped: the swapchain changed between
    /// the pre-acquire capture preparation and the recording of the
    /// frame (the acquire path may recreate the swapchain), so the
    /// prepared destination no longer matches the image being
    /// recorded. Never recorded against a mismatched destination; the
    /// frame itself renders normally.
    CaptureDropped(String),
    /// A proxy population was refused by the instance packer (the
    /// D2 no-partial-overflow discipline): over-capacity total or an
    /// invalid per-field value. Nothing was rendered and no frame
    /// state was touched.
    ProxyPack(crate::proxy_instances::InstancePackError),
    /// The pose-block repack's typed refusal (over-capacity — the
    /// same no-partial-overflow discipline the instance pack
    /// carries; the Era-2 skinning path).
    PosePack(crate::animation::PosePackError),
    /// A fault deliberately injected through the renderer's diagnostic
    /// seam (the corrective finding 3+8 verification knobs): never
    /// produced by a real Vulkan call. Distinguishes an injected
    /// recoverable construction failure from a genuine device error.
    InjectedFault(String),
}

impl RendererError {
    pub(crate) fn vk(result: vk::Result, what: &str) -> Self {
        RendererError::Vulkan(format!("{what}: {result:?}"))
    }
}

impl std::fmt::Display for RendererError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RendererError::NoLoader(message) => write!(
                f,
                "no Vulkan loader available — is a Vulkan runtime installed? ({message})"
            ),
            RendererError::NoDevice(message) => {
                write!(f, "no suitable Vulkan device ({message})")
            }
            RendererError::Vulkan(message) => write!(f, "Vulkan error: {message}"),
            RendererError::Surface(message) => {
                write!(f, "SDL could not create the Vulkan surface: {message}")
            }
            RendererError::CaptureOutstanding => write!(
                f,
                "frame capture refused: another capture is outstanding (take it before \
                 requesting again)"
            ),
            RendererError::CaptureUnsupportedFormat(format) => write!(
                f,
                "frame capture refused: the surface format {format} is outside the \
                 supported 4-byte RGBA8/BGRA8 family"
            ),
            RendererError::CaptureSizingOverflow => write!(
                f,
                "frame capture refused: the destination size overflowed checked arithmetic"
            ),
            RendererError::CaptureDropped(reason) => {
                write!(f, "frame capture dropped: {reason}")
            }
            RendererError::ProxyPack(error) => write!(
                f,
                "proxy population refused (no partial buffer was produced): {error:?}"
            ),
            RendererError::PosePack(error) => write!(
                f,
                "pose blocks refused (no partial buffer was produced): {error:?}"
            ),
            RendererError::InjectedFault(message) => write!(
                f,
                "injected fault: {message} (diagnostic-only, not a device error)"
            ),
        }
    }
}

impl From<crate::proxy_instances::InstancePackError> for RendererError {
    fn from(error: crate::proxy_instances::InstancePackError) -> Self {
        RendererError::ProxyPack(error)
    }
}

impl std::error::Error for RendererError {}

#[cfg(test)]
mod policy_tests {
    use super::*;

    /// The 6-E per-kind instance ranges: the fixed
    /// npc-then-vehicle-then-rain order (rain LAST — the Era-2
    /// package C third kind) partitions the slot (disjoint,
    /// contiguous, covering the total), and the draw-call count is
    /// the pinned headline — one per populated kind instanced, one
    /// per instance in the per-object reference.
    #[test]
    fn proxy_kind_ranges_and_draw_call_counts() {
        use crate::ProxyDrawMode;
        use proxy_draw::{ProxyKind, kind_instance_range, proxy_draw_call_count};

        let stride = crate::proxy_instances::INSTANCE_STRIDE as u64;
        // A mixed population: npc [0, 3), vehicle [3, 5), rain [5, 6).
        let npc = kind_instance_range(3, 2, 1, ProxyKind::Npc);
        let vehicle = kind_instance_range(3, 2, 1, ProxyKind::Vehicle);
        let rain = kind_instance_range(3, 2, 1, ProxyKind::Rain);
        assert_eq!(npc, 0..3);
        assert_eq!(vehicle, 3..5);
        assert_eq!(rain, 5..6);
        assert_eq!(npc.end, vehicle.start, "contiguous");
        assert_eq!(vehicle.end, rain.start, "contiguous");
        assert_eq!(rain.end, 6, "covers the total");
        // Byte offsets (the vertex-buffer binding arithmetic).
        assert_eq!(npc.start * stride, 0);
        assert_eq!(vehicle.start * stride, 3 * stride);
        assert_eq!(rain.start * stride, 5 * stride);

        // The pinned integers: the populated corner scene's proxy
        // submission count under each mode. The 6-E2 indirect mode
        // records the SAME count as the direct instanced mode — the
        // GPU-side count changes where N lives, never how many draws
        // submit (the headline must not regress). The rain kind
        // joins every arm exactly like the other kinds.
        assert_eq!(
            proxy_draw_call_count(ProxyDrawMode::Instanced, 24, 2, 0),
            2,
            "one per kind (the rain kind empty — the renderer-kind \
             stage's dry path)"
        );
        assert_eq!(
            proxy_draw_call_count(ProxyDrawMode::Instanced, 24, 2, 512),
            3,
            "one per kind with rain populated"
        );
        assert_eq!(
            proxy_draw_call_count(ProxyDrawMode::Indirect, 24, 2, 512),
            3,
            "the indirect mode: still one per kind"
        );
        assert_eq!(
            proxy_draw_call_count(ProxyDrawMode::PerInstance, 24, 2, 0),
            26,
            "one per instance"
        );
        assert_eq!(
            proxy_draw_call_count(ProxyDrawMode::PerInstance, 24, 2, 512),
            538,
            "one per instance with rain populated"
        );
        assert_eq!(
            proxy_draw_call_count(ProxyDrawMode::Indirect, 0, 2, 0),
            1,
            "empty kind draws nothing"
        );
        assert_eq!(
            proxy_draw_call_count(ProxyDrawMode::Indirect, 0, 0, 512),
            1,
            "a rain-only population is one draw"
        );
        assert_eq!(
            proxy_draw_call_count(ProxyDrawMode::PerInstance, 0, 0, 0),
            0
        );
        assert_eq!(
            proxy_draw_call_count(ProxyDrawMode::Indirect, 0, 0, 0),
            0,
            "empty population"
        );
    }
}

#[cfg(test)]
mod orientation {
    const SCENE_VERT_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/scene.vert.spv"));
    const OVERLAY_VERT_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/overlay.vert.spv"));
    const SCENE_FRAG_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/scene.frag.spv"));
    const OVERLAY_FRAG_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/overlay.frag.spv"));

    /// Re-parses a build-emitted SPIR-V module through the pinned
    /// naga, WITHOUT the front end's own coordinate re-adjustment
    /// (naga's spv-in `Options::adjust_coordinate_space` defaults to
    /// true and would fold the flip away — the pin must observe the
    /// emitted words exactly as the GPU receives them).
    fn parse_emitted(bytes: &[u8], name: &str) -> naga::Module {
        let options = naga::front::spv::Options {
            adjust_coordinate_space: false,
            ..Default::default()
        };
        naga::front::spv::parse_u8_slice(bytes, &options).unwrap_or_else(|error| {
            panic!("the build-emitted {name} re-parses through naga: {error:?}")
        })
    }

    /// The orientation pin's REPRESENTATION-ROBUST form (the batch-3
    /// review's corrective item 4): checks the EMITTED WORDS' dataflow
    /// directly — an access chain into the Position builtin's Y
    /// component (index 1), loaded, negated, and stored back — instead
    /// of a naga-round-trip whose expression arrangement depends on
    /// how a particular naga build reconstructs the module. The
    /// emitted shader carries the adjustment whenever this dataflow
    /// exists, however the instructions are ordered or numbered; the
    /// former structural matcher failed on equivalent-but-differently-
    /// arranged emissions (reproduced: the isolated release-profile
    /// build-script emission).
    fn spv_negates_position_y(bytes: &[u8]) -> bool {
        use std::collections::HashSet;
        // Stable SPIR-V core opcode and enum numbers.
        const OP_STORE: u32 = 62;
        const OP_LOAD: u32 = 61;
        const OP_ACCESS_CHAIN: u32 = 65;
        const OP_CONSTANT: u32 = 43;
        const OP_DECORATE: u32 = 71;
        const OP_FNEGATE: u32 = 127;
        // OpDecorate's operands are [target, Decoration, ...params]:
        // the Decoration enum value for BuiltIn, then the BuiltIn enum
        // value for Position.
        const DECORATION_BUILTIN: u32 = 11;
        const BUILTIN_POSITION: u32 = 0;

        let words: Vec<u32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| u32::from_le_bytes(*word))
            .collect();
        if words.len() < 5 || words[0] != 0x0723_0203 {
            return false;
        }
        // (opcode, operands) pairs in stream order; a malformed stream
        // (zero count or overrun) is not the adjustment.
        let mut instructions: Vec<(u32, Vec<u32>)> = Vec::new();
        let mut cursor = 5;
        while cursor < words.len() {
            let head = words[cursor];
            let count = (head >> 16) as usize;
            if count == 0 || cursor + count > words.len() {
                return false;
            }
            instructions.push((head & 0xffff, words[cursor + 1..cursor + count].to_vec()));
            cursor += count;
        }
        // The ids decorated BuiltIn Position (the gl_Position output):
        // OpDecorate [target, BuiltIn(11), Position(0)].
        let position_vars: HashSet<u32> = instructions
            .iter()
            .filter(|(opcode, operands)| {
                *opcode == OP_DECORATE
                    && operands.len() >= 3
                    && operands[1] == DECORATION_BUILTIN
                    && operands[2] == BUILTIN_POSITION
            })
            .map(|(_, operands)| operands[0])
            .collect();
        if position_vars.is_empty() {
            return false;
        }
        // The scalar-1 constant ids (the Y component's index).
        let one_constants: HashSet<u32> = instructions
            .iter()
            .filter(|(opcode, operands)| {
                *opcode == OP_CONSTANT && operands.len() == 3 && operands[2] == 1
            })
            .map(|(_, operands)| operands[1])
            .collect();
        // The dataflow: accessChain(position, 1) -> load -> negate ->
        // store back into the same access chain.
        for (opcode, operands) in &instructions {
            // [result type, result, base, index] — exactly one index.
            if *opcode != OP_ACCESS_CHAIN
                || operands.len() != 4
                || !position_vars.contains(&operands[2])
                || !one_constants.contains(&operands[3])
            {
                continue;
            }
            let access = operands[1];
            let load = instructions.iter().find_map(|(op, o)| {
                (*op == OP_LOAD && o.len() == 3 && o[2] == access).then(|| o[1])
            });
            let Some(load) = load else { continue };
            let negate = instructions.iter().find_map(|(op, o)| {
                (*op == OP_FNEGATE && o.len() == 3 && o[2] == load).then(|| o[1])
            });
            let Some(negate) = negate else { continue };
            if instructions
                .iter()
                .any(|(op, o)| *op == OP_STORE && o.len() == 2 && o[0] == access && o[1] == negate)
            {
                return true;
            }
        }
        false
    }

    /// The orientation corrective's A4 pin: the BUILD-EMITTED vertex
    /// shaders carry the compiler-generated Y-coordinate adjustment
    /// (naga 29.0.4's default `WriterFlags::ADJUST_COORDINATE_SPACE`
    /// epilogue — the OpFNegate on `gl_Position.y`), and the emitted
    /// fragment shaders carry none. This is the single authoritative
    /// Vulkan Y-axis conversion the corrected `perspective()` relies
    /// on: if a future naga/toolchain change stops emitting the
    /// adjustment, the world flips upside down again and THIS pin
    /// fails — the exact failure mode that caused the owner-reported
    /// defect (the projection used to compensate with its own second
    /// flip).
    #[test]
    fn emitted_vertex_shaders_carry_the_position_y_adjustment() {
        // Well-formedness prelude: the emitted bytes re-parse through
        // the pinned naga (the raw matcher below checks the semantics;
        // this keeps the re-parse property the pin always had).
        let _ = parse_emitted(SCENE_VERT_SPV, "scene.vert.spv");
        let _ = parse_emitted(OVERLAY_VERT_SPV, "overlay.vert.spv");
        let _ = parse_emitted(SCENE_FRAG_SPV, "scene.frag.spv");
        let _ = parse_emitted(OVERLAY_FRAG_SPV, "overlay.frag.spv");
        // The semantic pin, representation-robust (the raw dataflow on
        // the emitted words — profile- and arrangement-independent).
        assert!(
            spv_negates_position_y(SCENE_VERT_SPV),
            "the emitted scene.vert.spv must negate gl_Position.y (the naga \
             ADJUST_COORDINATE_SPACE epilogue) — its absence flips the world"
        );
        assert!(
            spv_negates_position_y(OVERLAY_VERT_SPV),
            "the emitted overlay.vert.spv must negate gl_Position.y — the \
             overlay's NDC authoring is correct only WITH the adjustment"
        );
        // Negative controls: no Position output, no adjustment to find.
        assert!(
            !spv_negates_position_y(SCENE_FRAG_SPV),
            "the fragment shader has no position output — a match here means \
             the matcher matches structure that is not the adjustment"
        );
        assert!(
            !spv_negates_position_y(OVERLAY_FRAG_SPV),
            "the fragment shader has no position output — a match here means \
             the matcher matches structure that is not the adjustment"
        );
    }
}

#[cfg(test)]
mod position_invariance {
    //! The Era-1 review's finding 19 pin: the three pipelines whose
    //! vertex position outputs must be bit-identical across
    //! separately compiled pipelines — the ordinary lit scene, the
    //! shadowed lit scene, and the wet ground's EQUAL re-draw —
    //! carry the SPIR-V `Invariant` decoration on their position
    //! outputs, emitted by the SAME pinned naga (29.0.4) that
    //! build.rs compiles them with. Equal inputs and operation
    //! order do not establish bit-identical clip positions across
    //! separately compiled pipelines (the Vulkan invariance rules);
    //! the decoration is what constrains the driver, and this pin
    //! is what observes it — if a future naga/toolchain change
    //! stops emitting `Invariant` for `@invariant`, the wet
    //! ground's EQUAL depth test loses its portability guarantee
    //! and THIS test fails.
    const LIT_SCENE_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/lit_scene.spv"));
    const LIT_SCENE_SHADOWED_SPV: &[u8] =
        include_bytes!(concat!(env!("OUT_DIR"), "/lit_scene_shadowed.spv"));
    const GROUND_REFLECT_SPV: &[u8] =
        include_bytes!(concat!(env!("OUT_DIR"), "/ground_reflect.spv"));
    // Negative controls: pipelines with position outputs that are
    // NOT part of the wet ground's EQUAL contract — the instanced
    // proxies (their instances never cover the ground quad the
    // overlay re-draws) and the sky (drawn before the scene, never
    // EQUAL-matched by the overlay). A match here means the matcher
    // fires on structure that is not the decoration.
    const INSTANCED_PROXY_SPV: &[u8] =
        include_bytes!(concat!(env!("OUT_DIR"), "/instanced_proxy.spv"));
    const SKY_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sky.spv"));

    /// Re-parses a build-emitted SPIR-V module through the pinned
    /// naga (the orientation pin's own prelude — well-formedness of
    /// the exact bytes the GPU receives, without the front end's
    /// coordinate re-adjustment folding anything away).
    fn parse_emitted(bytes: &[u8], name: &str) -> naga::Module {
        let options = naga::front::spv::Options {
            adjust_coordinate_space: false,
            ..Default::default()
        };
        naga::front::spv::parse_u8_slice(bytes, &options).unwrap_or_else(|error| {
            panic!("the build-emitted {name} re-parses through naga: {error:?}")
        })
    }

    /// The emitted words as (opcode, operands) pairs in stream
    /// order; `None` on a malformed stream (not a valid module).
    fn instructions(bytes: &[u8]) -> Option<Vec<(u32, Vec<u32>)>> {
        let words: Vec<u32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| u32::from_le_bytes(*word))
            .collect();
        if words.len() < 5 || words[0] != 0x0723_0203 {
            return None;
        }
        let mut parsed = Vec::new();
        let mut cursor = 5;
        while cursor < words.len() {
            let head = words[cursor];
            let count = (head >> 16) as usize;
            if count == 0 || cursor + count > words.len() {
                return None;
            }
            parsed.push((head & 0xffff, words[cursor + 1..cursor + count].to_vec()));
            cursor += count;
        }
        Some(parsed)
    }

    // Stable SPIR-V core opcode and enum numbers (the orientation
    // pin's own convention): OpMemberDecorate decorates a struct
    // member, OpDecorate a whole object; the Decoration enum values
    // BuiltIn = 11 and Invariant = 18; the BuiltIn enum value
    // Position = 0.
    const OP_DECORATE: u32 = 71;
    const OP_MEMBER_DECORATE: u32 = 64;
    const DECORATION_BUILTIN: u32 = 11;
    const DECORATION_INVARIANT: u32 = 18;
    const BUILTIN_POSITION: u32 = 0;

    /// The (struct, member) pairs whose member decoration is
    /// `decoration` (with the BuiltIn parameter when required).
    fn member_pairs(
        instructions: &[(u32, Vec<u32>)],
        decoration: u32,
        parameter: Option<u32>,
    ) -> Vec<(u32, u32)> {
        instructions
            .iter()
            .filter(|(opcode, operands)| {
                *opcode == OP_MEMBER_DECORATE
                    && operands.len() >= 3
                    && operands[2] == decoration
                    && parameter.is_none_or(|value| operands.get(3) == Some(&value))
            })
            .map(|(_, operands)| (operands[0], operands[1]))
            .collect()
    }

    /// The object ids decorated `decoration` (with the BuiltIn
    /// parameter when required).
    fn decorated_ids(
        instructions: &[(u32, Vec<u32>)],
        decoration: u32,
        parameter: Option<u32>,
    ) -> Vec<u32> {
        instructions
            .iter()
            .filter(|(opcode, operands)| {
                *opcode == OP_DECORATE
                    && operands.len() >= 2
                    && operands[1] == decoration
                    && parameter.is_none_or(|value| operands.get(2) == Some(&value))
            })
            .map(|(_, operands)| operands[0])
            .collect()
    }

    /// The semantic pin: SOME position output — a struct member
    /// carrying BOTH BuiltIn(Position) and Invariant, or a whole
    /// object carrying both — exists in the emitted words. The
    /// member form is what the pinned naga emits for a WGSL struct
    /// output (`@invariant @builtin(position)`); the object form is
    /// accepted too so the pin survives an equivalent-but-rearranged
    /// emission (the orientation pin's representation-robustness
    /// lesson).
    fn marks_a_position_output_invariant(bytes: &[u8]) -> bool {
        let Some(instructions) = instructions(bytes) else {
            return false;
        };
        let position_members =
            member_pairs(&instructions, DECORATION_BUILTIN, Some(BUILTIN_POSITION));
        let invariant_members = member_pairs(&instructions, DECORATION_INVARIANT, None);
        if position_members
            .iter()
            .any(|pair| invariant_members.contains(pair))
        {
            return true;
        }
        let position_ids = decorated_ids(&instructions, DECORATION_BUILTIN, Some(BUILTIN_POSITION));
        let invariant_ids = decorated_ids(&instructions, DECORATION_INVARIANT, None);
        position_ids.iter().any(|id| invariant_ids.contains(id))
    }

    #[test]
    fn wet_equal_pipelines_mark_the_position_output_invariant() {
        // Well-formedness prelude: the emitted bytes re-parse through
        // the pinned naga before the raw matcher checks their words.
        let _ = parse_emitted(LIT_SCENE_SPV, "lit_scene.spv");
        let _ = parse_emitted(LIT_SCENE_SHADOWED_SPV, "lit_scene_shadowed.spv");
        let _ = parse_emitted(GROUND_REFLECT_SPV, "ground_reflect.spv");
        let _ = parse_emitted(INSTANCED_PROXY_SPV, "instanced_proxy.spv");
        let _ = parse_emitted(SKY_SPV, "sky.spv");
        // The semantic pin, one per EQUAL-contract pipeline.
        assert!(
            marks_a_position_output_invariant(LIT_SCENE_SPV),
            "lit_scene.spv must decorate its position output Invariant (finding 19) — \
             the wet ground's EQUAL depth test loses its cross-pipeline guarantee"
        );
        assert!(
            marks_a_position_output_invariant(LIT_SCENE_SHADOWED_SPV),
            "lit_scene_shadowed.spv must decorate its position output Invariant \
             (finding 19) — the shadowed scene must write bit-identical ground depth"
        );
        assert!(
            marks_a_position_output_invariant(GROUND_REFLECT_SPV),
            "ground_reflect.spv must decorate its position output Invariant (finding \
             19) — the EQUAL re-draw must reproduce the scene's clip positions"
        );
        // Negative controls: unmarked position outputs do not match.
        assert!(
            !marks_a_position_output_invariant(INSTANCED_PROXY_SPV),
            "instanced_proxy.spv carries no @invariant — a match here means the \
             matcher fires on structure that is not the decoration"
        );
        assert!(
            !marks_a_position_output_invariant(SKY_SPV),
            "sky.spv carries no @invariant — a match here means the matcher fires \
             on structure that is not the decoration"
        );
    }
}

#[cfg(test)]
mod shader_composition {
    //! The P4 S5 composition guard (design §6.3 + §8-S5's removal
    //! condition, made executable): the variant shells under
    //! `shaders/variants/` compose from the shared fragments under
    //! `shaders/core/` at `// @core:<name>` anchor lines; the build
    //! script performs the splice and fails on a missing core. These
    //! tests pin the SOURCE TREE's shape so a regression — a pasted-
    //! back equation, an orphaned fragment, a restored monolith —
    //! fails `cargo test`, not just review.

    use std::fs;
    use std::path::PathBuf;

    fn shaders_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shaders")
    }

    /// (file name, text) for every `.wgsl` under `shaders/<sub>`.
    fn wgsl_sources(sub: &str) -> Vec<(String, String)> {
        let dir = shaders_dir().join(sub);
        let mut out = Vec::new();
        for entry in
            fs::read_dir(&dir).unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
        {
            let path = entry
                .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
                .path();
            if path.extension().is_some_and(|ext| ext == "wgsl") {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                let text = fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
                out.push((name, text));
            }
        }
        out
    }

    /// A marker hits `text` only on a NON-COMMENT line: the shells'
    /// headers document the equations (the pre-S5 documentation
    /// sketches), and those comment lines are not live equation text.
    fn carries_marker(text: &str, marker: &str) -> bool {
        text.lines().any(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with("//") && trimmed.contains(marker)
        })
    }

    /// The composition's integrity: every anchor resolves to an
    /// existing core, and every core is spliced by at least one shell
    /// (no orphaned fragments).
    #[test]
    fn every_anchor_resolves_and_every_core_is_used() {
        let mut referenced: Vec<String> = Vec::new();
        for (shell, text) in wgsl_sources("variants") {
            for line in text.lines() {
                let Some(core) = line.trim().strip_prefix("// @core:") else {
                    continue;
                };
                let core = core.trim();
                let path = shaders_dir().join("core").join(format!("{core}.wgsl"));
                assert!(
                    path.is_file(),
                    "{shell}: anchor @core:{core} does not resolve — every anchor must name a \
                     shaders/core/{core}.wgsl"
                );
                referenced.push(core.to_owned());
            }
        }
        assert!(
            !referenced.is_empty(),
            "no anchors found — the variants tree lost its composition"
        );
        for (core, _) in wgsl_sources("core") {
            let stem = core.trim_end_matches(".wgsl");
            assert!(
                referenced.iter().any(|name| name == stem),
                "core {core} is orphaned — no variant shell splices it (delete it or wire it)"
            );
        }
    }

    /// The S5 removal condition: the shared lighting/shadow/skinning/
    /// layout semantics live ONCE, in `shaders/core/` — no shell
    /// re-declares them, and each still exists somewhere (only the
    /// duplication is gone, never the semantic).
    #[test]
    fn variant_shells_carry_no_duplicated_equation_text() {
        const MARKERS: &[&str] = &[
            "fn shadow_factor",
            "fn tap_offset",
            "fn square_pow",
            "fn trailing_zeros",
            "let base = u32(i_pose.x) * 11u;",
            "let m0 = poses[base + joints.x];",
            "var shadow_map: texture_depth_2d;",
            "var<storage, read> poses: array<mat4x4<f32>>;",
            "struct FrameParams {",
            "struct MaterialParams {",
            "struct LightMatrix {",
            "let m = mat4x4<f32>(i_col0, i_col1, i_col2, i_col3);",
            "let squarings = trailing_zeros(u32(material.specular.a));",
            "let spec = square_pow(n_dot_h, squarings);",
            "shadow = shadow_factor(world_pos, n);",
            "let lit = albedo *",
        ];
        let shells = wgsl_sources("variants");
        let cores = wgsl_sources("core");
        for marker in MARKERS {
            for (shell, text) in &shells {
                assert!(
                    !carries_marker(text, marker),
                    "{shell} re-declares `{marker}` — the shared semantics must be composed \
                     from shaders/core, never duplicated in a shell"
                );
            }
            assert!(
                cores.iter().any(|(_, text)| carries_marker(text, marker)),
                "no core carries `{marker}` — the semantic went missing, not just its \
                 duplication"
            );
        }
    }

    /// The no-dual-source rule (design §8-S5): the nine pre-S5
    /// monolithic variant sources stay deleted — the composed shells
    /// are the only source of truth for the variant set.
    #[test]
    fn the_legacy_monolithic_variant_sources_are_gone() {
        const LEGACY: &[&str] = &[
            "lit_scene.wgsl",
            "lit_scene_shadowed.wgsl",
            "instanced_proxy.wgsl",
            "instanced_proxy_shadowed.wgsl",
            "instanced_proxy_skinned.wgsl",
            "instanced_proxy_skinned_shadowed.wgsl",
            "shadow_depth.wgsl",
            "shadow_depth_instanced.wgsl",
            "shadow_depth_skinned.wgsl",
        ];
        for name in LEGACY {
            assert!(
                !shaders_dir().join(name).exists(),
                "{name} exists at the shaders root — the monolithic source must stay deleted \
                 (no dual source; the composed variant owns the name now)"
            );
        }
    }
}
