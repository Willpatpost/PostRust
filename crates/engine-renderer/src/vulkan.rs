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
//! recreation for the next renderable frame.

// Unsafe containment (Stage 2 authorization, item 1): this module-level
// allow overrides the crate-level `#![deny(unsafe_code)]` in lib.rs —
// deny is overridable at closer scope, and this is the ONLY override.
#![allow(unsafe_code)]

use std::cell::Cell;
use std::ffi::{CStr, CString};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use ash::ext;
use ash::khr;
use ash::vk;
use ash::{Device, Entry, Instance};

use crate::config::DepthFormatPreference;

#[path = "corner.rs"]
pub(crate) mod corner;
#[path = "descriptors.rs"]
pub mod descriptors;
#[path = "post_draw.rs"]
pub(crate) mod post_draw;
#[path = "proxy_draw.rs"]
pub(crate) mod proxy_draw;
#[path = "reflection_draw.rs"]
pub(crate) mod reflection_draw;
#[path = "shadow_draw.rs"]
pub(crate) mod shadow_draw;
#[path = "sky_draw.rs"]
pub(crate) mod sky_draw;
#[path = "textures.rs"]
pub mod textures;

use crate::SceneLayer;
use crate::config::{
    DemoScenePreference, DynamicClamp, FRAMES_IN_FLIGHT, PresentModePreference, RenderConfig,
    TargetFaultSite, TargetFaultStage, dynamic_geometry_clamp, resolve_present_mode,
};
use crate::overlay::OverlayVertex;
use crate::scene::SceneVertex;
use descriptors::DescriptorFoundation;
use textures::{SamplerPool, Texture};

use sdl3::video::Window as SdlWindow;

const VALIDATION_LAYER_NAME: &str = "VK_LAYER_KHRONOS_validation";
/// Shader entry point for all Stage 2 pipelines.
const ENTRY_MAIN: &CStr = c"main";
/// Upper bound for any host-side fence wait (nanoseconds). A stuck
/// submission must surface as an error the application can report,
/// never as an unresponsive window (rig finding 2026-10-20: pacing
/// faster than a vsync-locked present mode can drain wedged the frame
/// loop — 240 fps pacing against FIFO on a 60 Hz display).
const WAIT_TIMEOUT_NS: u64 = 2_000_000_000;

/// Validation messages from the driver callback: (class, text) where
/// class 2 = error, 1 = warning, 0 = info. Bounded; drained by
/// [`crate::Renderer`] each frame.
static VALIDATION_QUEUE: Mutex<Vec<(u8, String)>> = Mutex::new(Vec::new());

/// Updates to the hint are serialized with queue mutations. An inactive
/// renderer can skip the queue mutex when no environment message is pending.
static ENVIRONMENT_PENDING: AtomicBool = AtomicBool::new(false);

/// Whether the queue holds environment diagnostics awaiting a drain
/// (cheap atomic read — the per-frame drain gate consults this).
pub(crate) fn environment_pending() -> bool {
    ENVIRONMENT_PENDING.load(Ordering::Relaxed)
}

/// Takes the queued messages and clears their hint under the same lock.
pub(crate) fn drain_validation_messages() -> Vec<(u8, String)> {
    let mut queue = VALIDATION_QUEUE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    ENVIRONMENT_PENDING.store(false, Ordering::Relaxed);
    std::mem::take(&mut *queue)
}

/// Records a renderer-observed environment limitation into the
/// validation diagnostics queue (class 1 = warning) — the existing
/// channel every run summary drains, so the limitation is explicitly
/// diagnosable without new architecture (Mac 2 review follow-up B).
fn queue_environment_message(text: String) {
    if let Ok(mut queue) = VALIDATION_QUEUE.lock()
        && queue.len() < 256
    {
        queue.push((1, text));
        ENVIRONMENT_PENDING.store(true, Ordering::Relaxed);
    }
}

unsafe extern "system" fn validation_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _types: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT,
    _user_data: *mut core::ffi::c_void,
) -> vk::Bool32 {
    if let Some(record) = (!data.is_null()).then(|| unsafe { &*data }) {
        let text = unsafe { CStr::from_ptr(record.p_message) }
            .to_string_lossy()
            .into_owned();
        let class = if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
            2
        } else if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::WARNING) {
            1
        } else {
            0
        };
        if let Ok(mut queue) = VALIDATION_QUEUE.lock()
            && queue.len() < 256
        {
            queue.push((class, text));
        }
    }
    vk::FALSE
}

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

/// Core Vulkan objects. Created once; destroyed once in `destroy`.
pub(crate) struct VulkanContext {
    // Load-bearing lifetime field (never read): Entry owns the loaded
    // Vulkan loader (libloading::Library) — dropping it while the
    // instance is alive would unload the library under live handles.
    #[allow(dead_code)]
    pub entry: Entry,
    pub instance: Instance,
    pub messenger: vk::DebugUtilsMessengerEXT,
    pub debug_utils: ext::debug_utils::Instance,
    pub surface_ext: khr::surface::Instance,
    pub swapchain_ext: khr::swapchain::Device,
    pub physical_device: vk::PhysicalDevice,
    pub device: Device,
    pub graphics_queue: vk::Queue,
    pub graphics_family: u32,
    pub surface: vk::SurfaceKHR,
    pub timestamp_period_ns: f64,
    /// Timestamp valid bits for the graphics queue family; 0 means the
    /// family supports no timestamp queries (corrective finding E).
    pub timestamp_valid_bits: u32,
    pub gpu_name: String,
    pub device_type: String,
    pub api_version: String,
    pub supported_present_modes: Vec<vk::PresentModeKHR>,
    /// The renderer's pipeline cache, owned at DEVICE lifetime (the
    /// batch-3 review's lifecycle finding 2): created once with the
    /// context and destroyed at its teardown — AFTER every pipeline
    /// state that used it — so swapchain recreations and pipeline
    /// replacements REUSE the warmed cache instead of each building a
    /// fresh empty one that dies with its state.
    pub pipeline_cache: vk::PipelineCache,
    corner: Option<corner::CornerResources>,
    texture: Texture,
    samplers: SamplerPool,
    descriptors: DescriptorFoundation,
    memory_allocations: Cell<u64>,
    texture_allocations: Cell<u64>,
    descriptor_pools: Cell<u64>,
    descriptor_sets: Cell<u64>,
    sampler_creations: Cell<u64>,
    /// vkCmdDrawIndirect issuances since renderer construction —
    /// the 6-E2 ISSUANCE observation (the Era-1 review's
    /// gate-by-gate disposition for render_instanced: "one command
    /// trace or recording assertion that verifies vkCmdDrawIndirect
    /// was issued"). The proxy draw-call count cannot distinguish
    /// the indirect submission from the direct one (both count one
    /// per populated kind — by design, the headline), and the
    /// release-build fallback silently submits the identical
    /// geometry through cmd_draw when the indirect state is
    /// somehow missing; this counter is incremented AT the
    /// cmd_draw_indirect call site, so a frame that claims the
    /// indirect path but fell back observes as zero. Monotonic
    /// like the allocation counters (frame deltas, not live
    /// counts; survives pipeline replacement).
    indirect_draws_issued: Cell<u64>,
    /// Diagnostic fault-injection seam: arms a one-shot plan for the
    /// corrective finding 3+8 verification. The `u32` ordinal is
    /// 1-based and decrements per MATCHING check across the renderer's
    /// lifetime (the plan is NOT reset when consumed, only when the
    /// ordinal reaches 0). This cumulative decrement allows the same
    /// site/stage to fire multiple times across multiple target
    /// builders (e.g., shadow's per-slot loop). The plan is NEVER
    /// touched by real device calls — it is test-only and diagnostic.
    target_fault: Cell<Option<(TargetFaultSite, TargetFaultStage, u32)>>,
    /// The one-shot acquire-out-of-date injection (finding 6's
    /// verification knob — the driver-forced
    /// `VK_ERROR_OUT_OF_DATE_KHR` acquire path, unreachable from the
    /// test API otherwise). Diagnostic-only, test-only: consumed at
    /// the FIRST acquire site of the next frame so the retry acquire
    /// inside the out-of-date arm is structurally real.
    acquire_out_of_date_armed: Cell<bool>,
    /// Creation counts for Era-1 target builders' Vulkan objects (the
    /// review's finding 8 acquisitions: image, memory, view, framebuffer
    /// — post chain, wet chain, shadow chain). These are monotonic
    /// counters, recorded only on success. (Destructions count every
    /// call since release paths drain owned vectors exactly once.)
    target_images: Cell<u64>,
    target_image_views: Cell<u64>,
    target_framebuffers: Cell<u64>,
    target_memories: Cell<u64>,
    /// Destruction counts for the same objects — recorded on every
    /// destroy call (not just on success, since release paths always
    /// free what they own).
    target_image_frees: Cell<u64>,
    target_view_frees: Cell<u64>,
    target_framebuffer_frees: Cell<u64>,
    target_memory_frees: Cell<u64>,
}

impl VulkanContext {
    pub(crate) fn new(
        window: &SdlWindow,
        atlas: &crate::glyph_atlas::GlyphAtlas,
        config: &RenderConfig,
    ) -> Result<(VulkanContext, bool, bool), RendererError> {
        let entry = unsafe { Entry::load() }
            .map_err(|error| RendererError::NoLoader(format!("{error}")))?;

        // SDL must hold the Vulkan loader open before the extension
        // query: its offscreen video driver (headless smoke) only
        // advertises VK_EXT_headless_surface when its own enumeration
        // can see it, and that enumeration needs SDL's loader handle.
        // Windowed drivers return their static platform list either
        // way. The library stays open for the process lifetime next to
        // ash's own dlopen; SDL_Quit releases SDL's handle.
        if !unsafe { sdl3::sys::vulkan::SDL_Vulkan_LoadLibrary(core::ptr::null()) } {
            return Err(RendererError::Surface(
                "SDL could not open the Vulkan loader".into(),
            ));
        }

        let sdl_extensions = window
            .vulkan_instance_extensions()
            .map_err(|error| RendererError::Surface(format!("{error}")))?;

        let want_validation = match config.validation {
            crate::config::ValidationPreference::Off => false,
            crate::config::ValidationPreference::On => true,
            crate::config::ValidationPreference::Auto => cfg!(debug_assertions),
        };

        // Mac 2, change A: the loader's advertised instance extensions
        // decide portability enumeration. Enumeration failure is an
        // environment limitation, not an application error — an empty
        // advertised set preserves the pre-Mac-2 behavior exactly — but
        // it is recorded through the renderer's existing diagnostics
        // queue so the run summary makes the limitation explicit (Mac 2
        // review follow-up B).
        let loader_advertised: Vec<String> =
            match unsafe { entry.enumerate_instance_extension_properties(None) } {
                Ok(properties) => properties
                    .iter()
                    .map(|property| {
                        unsafe { CStr::from_ptr(property.extension_name.as_ptr()) }
                            .to_string_lossy()
                            .into_owned()
                    })
                    .collect(),
                Err(error) => {
                    queue_environment_message(format!(
                        "vkEnumerateInstanceExtensionProperties failed ({error:?}) — \
                         VK_KHR_portability_enumeration not enabled: environment \
                         limitation, not an application error"
                    ));
                    Vec::new()
                }
            };
        let mut plan =
            InstanceExtensionPlan::select(&sdl_extensions, &loader_advertised, want_validation);
        let mut layer_cstrings: Vec<CString> = Vec::new();
        if want_validation {
            layer_cstrings.push(CString::new(VALIDATION_LAYER_NAME).unwrap());
        }

        let api_version = vk::make_api_version(0, 1, 1, 0);
        let app_info = vk::ApplicationInfo::default()
            .application_name(c"Sektor7")
            .application_version(vk::make_api_version(0, 0, 2, 0))
            .engine_name(c"Sektor7 engine")
            .engine_version(vk::make_api_version(0, 0, 2, 0))
            .api_version(api_version);

        let mut instance = None;
        let mut validation_active = false;
        let mut sync_validation = false;
        if want_validation {
            // Try with validation; failure here is an environment
            // limitation (no layer installed), not an application error.
            // The fallback drops ONLY the validation extension/layer —
            // portability enumeration and its flag survive (Mac 2
            // authorization §4), so a missing layer can never hide
            // MoltenVK devices.
            let (_extension_cstrings, extension_names) =
                extension_name_pointers(&plan.extensions());
            let layer_names: Vec<*const core::ffi::c_char> =
                layer_cstrings.iter().map(|c| c.as_ptr()).collect();
            let mut debug_info = vk::DebugUtilsMessengerCreateInfoEXT::default()
                .message_severity(
                    vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                        | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                        | vk::DebugUtilsMessageSeverityFlagsEXT::INFO,
                )
                .message_type(
                    vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                        | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                        | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
                )
                .pfn_user_callback(Some(validation_callback));
            // Finding 9's explicitly-enabled synchronization validation
            // (the Era-1 review's check: the sync-proof gaps were
            // exercised under core validation only — sync modeling was
            // off). Chained ONLY on the validation attempt (never the
            // fallback): VK_EXT_validation_features is a layer-side
            // pNext — the loader passes it through and the KHRONOS
            // layer activates its synchronization modeling (the
            // negative-control proof: neutralizing the shadow pass's
            // producer-side subpass dependency on this exact build
            // fires SYNC-HAZARD-WRITE-AFTER-WRITE +
            // SYNC-HAZARD-READ-AFTER-WRITE at the map's sampled reads,
            // 44 errors the gates' validation_errors == 0 assertions
            // catch). Auto (debug builds) keeps core validation only —
            // no development-loop cost; On is the diagnostics arm.
            let sync_enables = [vk::ValidationFeatureEnableEXT::SYNCHRONIZATION_VALIDATION];
            let mut validation_features =
                vk::ValidationFeaturesEXT::default().enabled_validation_features(&sync_enables);
            let create_info = vk::InstanceCreateInfo::default()
                .application_info(&app_info)
                .flags(plan.create_flags())
                .enabled_extension_names(&extension_names)
                .enabled_layer_names(&layer_names)
                .push_next(&mut debug_info)
                .push_next(&mut validation_features);
            match unsafe { entry.create_instance(&create_info, None) } {
                Ok(created) => {
                    instance = Some(created);
                    validation_active = true;
                    sync_validation = true;
                }
                Err(_) => {
                    plan = plan.without_validation();
                    layer_cstrings.clear();
                }
            }
        }
        let instance = match instance {
            Some(created) => created,
            None => {
                // The fallback differs from the attempt above only in
                // validation — portability enumeration and its flag are
                // the same decision in both attempts.
                let (_extension_cstrings, extension_names) =
                    extension_name_pointers(&plan.extensions());
                let create_info = vk::InstanceCreateInfo::default()
                    .application_info(&app_info)
                    .flags(plan.create_flags())
                    .enabled_extension_names(&extension_names);
                unsafe { entry.create_instance(&create_info, None) }
                    .map_err(|error| RendererError::Vulkan(format!("create_instance: {error}")))?
            }
        };

        // Partial-initialization cleanup (corrective finding G): every
        // fallible step from here on releases what was already created
        // before returning the error, so no path leaks or double-frees.
        let surface_ext = khr::surface::Instance::new(&entry, &instance);
        let debug_utils = ext::debug_utils::Instance::new(&entry, &instance);
        let surface = match unsafe { window.vulkan_create_surface(instance.handle()) } {
            Ok(surface) => surface,
            Err(error) => {
                unsafe { instance.destroy_instance(None) };
                return Err(RendererError::Surface(format!("{error}")));
            }
        };
        let messenger = if validation_active {
            let info = vk::DebugUtilsMessengerCreateInfoEXT::default()
                .message_severity(
                    vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                        | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
                )
                .message_type(
                    vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                        | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                        | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
                )
                .pfn_user_callback(Some(validation_callback));
            match unsafe { debug_utils.create_debug_utils_messenger(&info, None) } {
                Ok(messenger) => messenger,
                Err(error) => {
                    unsafe {
                        surface_ext.destroy_surface(surface, None);
                        instance.destroy_instance(None);
                    }
                    return Err(RendererError::Vulkan(format!("messenger: {error}")));
                }
            }
        } else {
            vk::DebugUtilsMessengerEXT::null()
        };

        let Some((physical_device, graphics_family, portability_subset)) =
            pick_physical_device(&instance, &surface_ext, surface)
        else {
            unsafe {
                if messenger != vk::DebugUtilsMessengerEXT::null() {
                    debug_utils.destroy_debug_utils_messenger(messenger, None);
                }
                surface_ext.destroy_surface(surface, None);
                instance.destroy_instance(None);
            }
            return Err(RendererError::NoDevice(
                "no device with graphics+present queues, swapchain extension, and surface formats"
                    .into(),
            ));
        };

        let properties = unsafe { instance.get_physical_device_properties(physical_device) };
        let gpu_name = unsafe { CStr::from_ptr(properties.device_name.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        let device_type = match properties.device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU => "discrete",
            vk::PhysicalDeviceType::INTEGRATED_GPU => "integrated",
            vk::PhysicalDeviceType::VIRTUAL_GPU => "virtual",
            vk::PhysicalDeviceType::CPU => "cpu",
            _ => "other",
        }
        .to_string();
        let api_version = format!(
            "{}.{}.{}",
            vk::api_version_major(properties.api_version),
            vk::api_version_minor(properties.api_version),
            vk::api_version_patch(properties.api_version)
        );
        let timestamp_period_ns = f64::from(properties.limits.timestamp_period);
        // Timestamp support is a queue-family property (corrective
        // finding E): 0 valid bits means timestamps are unsupported and
        // the frame path skips query writes and reads entirely.
        let timestamp_valid_bits = unsafe {
            instance
                .get_physical_device_queue_family_properties(physical_device)
                .get(graphics_family as usize)
                .map_or(0, |family| family.timestamp_valid_bits)
        };

        let supported_present_modes = unsafe {
            surface_ext.get_physical_device_surface_present_modes(physical_device, surface)
        }
        .unwrap_or_default();

        // Mac 2, change B: swapchain always; portability subset when the
        // chosen device advertises it (required when advertised —
        // VUID-VkDeviceCreateInfo-pProperties-04451; MoltenVK device
        // creation fails without it). Inert on devices that don't.
        let device_extension_names: Vec<*const core::ffi::c_char> =
            select_device_extensions(portability_subset)
                .iter()
                .map(|name| name.as_ptr())
                .collect();
        let queue_info = vk::DeviceQueueCreateInfo::default()
            .queue_family_index(graphics_family)
            .queue_priorities(&[1.0]);
        let queue_infos = [queue_info];
        let device_create = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .enabled_extension_names(&device_extension_names);
        let device_creation =
            unsafe { instance.create_device(physical_device, &device_create, None) };
        let device = match device_creation {
            Ok(device) => device,
            Err(error) => {
                unsafe {
                    if messenger != vk::DebugUtilsMessengerEXT::null() {
                        debug_utils.destroy_debug_utils_messenger(messenger, None);
                    }
                    surface_ext.destroy_surface(surface, None);
                    instance.destroy_instance(None);
                }
                return Err(RendererError::Vulkan(format!("create_device: {error}")));
            }
        };
        let graphics_queue = unsafe { device.get_device_queue(graphics_family, 0) };
        let swapchain_ext = khr::swapchain::Device::new(&instance, &device);
        // The device-lifetime pipeline cache (the batch-3 review's
        // lifecycle finding 2): created once with the context, so
        // every pipeline build — initial, swapchain-recreation, and
        // format-change replacement alike — goes through the SAME
        // warmed cache; destroyed at context teardown, after every
        // pipeline state that used it (Renderer::drop destroys the
        // pipeline state before the context). An empty
        // PipelineCacheCreateInfo starts a fresh in-memory cache (disk
        // persistence remains a separately authorized follow-up).
        // Failure here runs the established partial-initialization
        // cleanup (the batch-3 review's P2): the messenger, device,
        // surface, and instance all exist at this point and are
        // released before the error escapes - never leaked.
        let pipeline_cache = match unsafe {
            device.create_pipeline_cache(&vk::PipelineCacheCreateInfo::default(), None)
        } {
            Ok(cache) => cache,
            Err(error) => {
                unsafe {
                    if messenger != vk::DebugUtilsMessengerEXT::null() {
                        debug_utils.destroy_debug_utils_messenger(messenger, None);
                    }
                    device.destroy_device(None);
                    surface_ext.destroy_surface(surface, None);
                    instance.destroy_instance(None);
                }
                return Err(RendererError::vk(error, "pipeline cache creation"));
            }
        };

        let mut context = VulkanContext {
            entry,
            instance,
            messenger,
            debug_utils,
            surface_ext,
            swapchain_ext,
            physical_device,
            device,
            graphics_queue,
            graphics_family,
            surface,
            timestamp_period_ns,
            timestamp_valid_bits,
            gpu_name,
            device_type,
            api_version,
            supported_present_modes,
            pipeline_cache,
            corner: None,
            texture: Default::default(),
            samplers: Default::default(),
            descriptors: Default::default(),
            memory_allocations: Cell::new(0),
            texture_allocations: Cell::new(0),
            descriptor_pools: Cell::new(0),
            descriptor_sets: Cell::new(0),
            sampler_creations: Cell::new(0),
            indirect_draws_issued: Cell::new(0),
            target_fault: Cell::new(None),
            acquire_out_of_date_armed: Cell::new(false),
            target_images: Cell::new(0),
            target_image_views: Cell::new(0),
            target_framebuffers: Cell::new(0),
            target_memories: Cell::new(0),
            target_image_frees: Cell::new(0),
            target_view_frees: Cell::new(0),
            target_framebuffer_frees: Cell::new(0),
            target_memory_frees: Cell::new(0),
        };
        if let Err(e) = context.init_atlas(atlas) {
            context.destroy();
            return Err(e);
        }
        if config.demo_scene == DemoScenePreference::Corner {
            match corner::CornerResources::new(&context) {
                Ok(resources) => {
                    context.corner = Some(resources);
                }
                Err(e) => {
                    context.destroy();
                    return Err(e);
                }
            }
        }
        Ok((context, validation_active, sync_validation))
    }

    fn init_atlas(&mut self, atlas: &crate::glyph_atlas::GlyphAtlas) -> Result<(), RendererError> {
        use crate::glyph_atlas::{ATLAS_HEIGHT, ATLAS_WIDTH};
        self.texture = Texture::upload(
            self,
            vk::Format::R8_UNORM,
            vk::Extent2D {
                width: ATLAS_WIDTH,
                height: ATLAS_HEIGHT,
            },
            atlas.atlas_bitmap(),
        )?;
        self.samplers = SamplerPool::new(self)?;
        self.descriptors =
            DescriptorFoundation::new(self, self.texture.view, self.samplers.nearest())?;
        Ok(())
    }

    /// SAFETY: callers pass allocation info for this device and own the returned memory.
    unsafe fn allocate_memory(
        &self,
        info: &vk::MemoryAllocateInfo<'_>,
    ) -> Result<vk::DeviceMemory, vk::Result> {
        let memory = unsafe { self.device.allocate_memory(info, None) }?;
        self.memory_allocations
            .set(self.memory_allocations.get() + 1);
        Ok(memory)
    }

    pub(crate) fn allocations(&self) -> crate::AllocationStats {
        crate::AllocationStats {
            device_memory: self.memory_allocations.get(),
            texture_memory: self.texture_allocations.get(),
            texture_bytes: self.texture.allocation_bytes
                + self.corner.as_ref().map_or(0, |r| r.texture_bytes()),
            descriptor_pools: self.descriptor_pools.get(),
            descriptor_sets: self.descriptor_sets.get(),
            samplers: self.sampler_creations.get(),
            target_images: self.target_images.get(),
            target_image_views: self.target_image_views.get(),
            target_framebuffers: self.target_framebuffers.get(),
            target_memories: self.target_memories.get(),
            target_image_frees: self.target_image_frees.get(),
            target_view_frees: self.target_view_frees.get(),
            target_framebuffer_frees: self.target_framebuffer_frees.get(),
            target_memory_frees: self.target_memory_frees.get(),
        }
    }

    /// Arms a one-shot target-construction fault plan. The `ordinal`
    /// is 1-based and decrements per MATCHING check across the
    /// renderer's lifetime (cumulative, not per-builder). The plan
    /// is consumed only when the ordinal reaches 0. Diagnostic-only,
    /// test-only: never produced by real device calls.
    pub(crate) fn set_target_fault(
        &self,
        site: TargetFaultSite,
        stage: TargetFaultStage,
        ordinal: u32,
    ) {
        // A zero ordinal can never fire (it would underflow below);
        // clamp to 1 so an armed plan is always fireable by its first
        // matching check.
        self.target_fault.set(Some((site, stage, ordinal.max(1))));
    }

    /// Checks if a fault should be injected for the given site and
    /// stage. Returns `Ok(())` immediately if no plan is armed or if
    /// the site/stage don't match (branch-cheap; runs only during
    /// construction, never per-frame). On match: decrements the
    /// ordinal; when it reaches 0, consumes the plan and returns
    /// `Err(RendererError::InjectedFault)`. The logic lives in
    /// [`advance_target_fault_plan`] (pure, unit-tested without a
    /// device — the mock-free seam tests below).
    pub(crate) fn check_target_fault(
        &self,
        site: TargetFaultSite,
        stage: TargetFaultStage,
    ) -> Result<(), RendererError> {
        let mut plan = self.target_fault.get();
        match advance_target_fault_plan(&mut plan, site, stage) {
            Some(error) => {
                self.target_fault.set(plan);
                Err(error)
            }
            None => {
                self.target_fault.set(plan);
                Ok(())
            }
        }
    }

    /// Arms a one-shot acquire-out-of-date injection (finding 6's
    /// verification knob — the driver-forced
    /// `VK_ERROR_OUT_OF_DATE_KHR` acquire path, unreachable from the
    /// test API otherwise). Diagnostic-only, test-only: the plan is
    /// consumed at the first acquire site of the next frame so the
    /// retry acquire inside the out-of-date arm is structurally
    /// real.
    pub(crate) fn set_acquire_out_of_date(&self) {
        self.acquire_out_of_date_armed.set(true);
    }

    /// Returns and disarms the one-shot acquire-out-of-date plan.
    /// `true` exactly once after `set_acquire_out_of_date`, then
    /// `false` until the next arm. The `Cell::replace(false)` pattern
    /// ensures consumption in a single frame.
    pub(crate) fn take_acquire_out_of_date(&self) -> bool {
        self.acquire_out_of_date_armed.replace(false)
    }

    /// Create a target image, incrementing the creation counter on
    /// success. The caller's `what` string is used for error
    /// reporting. SAFETY: callers pass valid image create info for
    /// this device and own the returned image.
    unsafe fn create_target_image(
        &self,
        info: &vk::ImageCreateInfo,
        what: &str,
    ) -> Result<vk::Image, RendererError> {
        // SAFETY: fresh object; the caller owns it from creation.
        let image = unsafe { self.device.create_image(info, None) }
            .map_err(|error| RendererError::vk(error, what))?;
        self.target_images.set(self.target_images.get() + 1);
        Ok(image)
    }

    /// Create a target image view, incrementing the creation counter
    /// on success. SAFETY: callers pass valid view create info for
    /// this device and own the returned view.
    unsafe fn create_target_image_view(
        &self,
        info: &vk::ImageViewCreateInfo,
        what: &str,
    ) -> Result<vk::ImageView, RendererError> {
        // SAFETY: fresh object; the caller owns it from creation.
        let view = unsafe { self.device.create_image_view(info, None) }
            .map_err(|error| RendererError::vk(error, what))?;
        self.target_image_views
            .set(self.target_image_views.get() + 1);
        Ok(view)
    }

    /// Create a target framebuffer, incrementing the creation counter
    /// on success. SAFETY: callers pass valid framebuffer create info
    /// for this device and own the returned framebuffer.
    unsafe fn create_target_framebuffer(
        &self,
        info: &vk::FramebufferCreateInfo,
        what: &str,
    ) -> Result<vk::Framebuffer, RendererError> {
        // SAFETY: fresh object; the caller owns it from creation.
        let framebuffer = unsafe { self.device.create_framebuffer(info, None) }
            .map_err(|error| RendererError::vk(error, what))?;
        self.target_framebuffers
            .set(self.target_framebuffers.get() + 1);
        Ok(framebuffer)
    }

    /// Allocate target memory, incrementing BOTH the target memory
    /// counter AND the device_memory counter (so device_memory stays
    /// complete renderer-wide). SAFETY: callers pass valid memory
    /// allocate info for this device and own the returned memory.
    unsafe fn allocate_target_memory(
        &self,
        info: &vk::MemoryAllocateInfo,
        what: &str,
    ) -> Result<vk::DeviceMemory, RendererError> {
        // SAFETY: callers pass allocation info for this device and
        // own the returned memory.
        match unsafe { self.allocate_memory(info) } {
            Ok(memory) => {
                self.target_memories.set(self.target_memories.get() + 1);
                Ok(memory)
            }
            Err(result) => Err(RendererError::vk(result, what)),
        }
    }

    /// Destroy a target image, incrementing the destruction counter.
    /// The caller must own the image. This is called on every
    /// destruction path since release always drains owned vectors.
    unsafe fn destroy_target_image(&self, image: vk::Image) {
        unsafe { self.device.destroy_image(image, None) };
        self.target_image_frees
            .set(self.target_image_frees.get() + 1);
    }

    /// Destroy a target image view, incrementing the destruction
    /// counter.
    unsafe fn destroy_target_image_view(&self, view: vk::ImageView) {
        unsafe { self.device.destroy_image_view(view, None) };
        self.target_view_frees.set(self.target_view_frees.get() + 1);
    }

    /// Destroy a target framebuffer, incrementing the destruction
    /// counter.
    unsafe fn destroy_target_framebuffer(&self, framebuffer: vk::Framebuffer) {
        unsafe { self.device.destroy_framebuffer(framebuffer, None) };
        self.target_framebuffer_frees
            .set(self.target_framebuffer_frees.get() + 1);
    }

    /// Free target memory, incrementing the destruction counter.
    unsafe fn free_target_memory(&self, memory: vk::DeviceMemory) {
        unsafe { self.device.free_memory(memory, None) };
        self.target_memory_frees
            .set(self.target_memory_frees.get() + 1);
    }

    /// Full teardown in manifest order (assumes swapchain/pipelines/frames
    /// were already destroyed by the owning Renderer).
    pub(crate) fn destroy(&mut self) {
        if let Some(corner) = self.corner.take() {
            corner.destroy(self);
        }
        std::mem::take(&mut self.descriptors).destroy(self);
        std::mem::take(&mut self.samplers).destroy(self);
        std::mem::take(&mut self.texture).destroy(self);
        unsafe {
            if self.messenger != vk::DebugUtilsMessengerEXT::null() {
                self.debug_utils
                    .destroy_debug_utils_messenger(self.messenger, None);
                self.messenger = vk::DebugUtilsMessengerEXT::null();
            }
            // The pipeline cache is a device object: released before
            // the device, after every pipeline state that used it
            // (destroyed by the owning Renderer before this teardown).
            self.device
                .destroy_pipeline_cache(self.pipeline_cache, None);
            self.device.destroy_device(None);
            self.surface_ext.destroy_surface(self.surface, None);
            self.instance.destroy_instance(None);
        }
    }
}

/// Instance-creation extension plan (Mac 2, change A; corrected per the
/// Mac 2 review follow-up A): the enabled extension list is **derived**
/// from three components rather than stored — SDL's driver-required
/// extensions (deduplicated, first occurrence first), the
/// portability-enumeration decision, and validation-**added**
/// debug-utils state. Derivation makes the review's correctness
/// properties structural: no input can produce a duplicate entry, and
/// `without_validation` clears only validation-added state, so an
/// SDL-required extension (including an SDL-required
/// `VK_EXT_debug_utils`) can never be removed by the fallback. The
/// Mac 0-era `pop()` ordering hazard and the name-filter hazard are
/// both eliminated by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
struct InstanceExtensionPlan {
    /// SDL's driver-required extensions, deduplicated, first occurrence
    /// first — never removed by any later state change.
    sdl_required: Vec<String>,
    /// `VK_KHR_portability_enumeration` is enabled — appended at
    /// creation time unless SDL itself required it; the instance must
    /// set `ENUMERATE_PORTABILITY_KHR`.
    portability_enumeration: bool,
    /// Validation-added `VK_EXT_debug_utils` state — appended at
    /// creation time unless SDL itself required it; the only component
    /// the validation fallback clears.
    validation: bool,
}

impl InstanceExtensionPlan {
    /// SDL's required list is deduplicated (first occurrence first);
    /// the loader's advertised extensions enable portability
    /// enumeration unless SDL already required it; validation wanting
    /// adds debug-utils state.
    fn select(
        sdl_required: &[String],
        loader_advertised: &[String],
        want_validation: bool,
    ) -> InstanceExtensionPlan {
        let portability = khr::portability_enumeration::NAME
            .to_string_lossy()
            .into_owned();
        let advertised = loader_advertised.contains(&portability);
        let sdl_requires_portability = sdl_required.contains(&portability);
        InstanceExtensionPlan {
            sdl_required: dedup_keep_first(sdl_required),
            portability_enumeration: advertised || sdl_requires_portability,
            validation: want_validation,
        }
    }

    /// The enabled extension list in creation order: SDL-required
    /// (deduplicated) first, then portability enumeration when enabled
    /// and not already required, then the validation-added debug-utils
    /// extension when wanted and not already required. No duplicates by
    /// construction.
    fn extensions(&self) -> Vec<String> {
        let mut extensions = self.sdl_required.clone();
        let portability = khr::portability_enumeration::NAME
            .to_string_lossy()
            .into_owned();
        if self.portability_enumeration && !extensions.contains(&portability) {
            extensions.push(portability);
        }
        if self.validation {
            let debug_utils = ext::debug_utils::NAME.to_string_lossy().into_owned();
            if !extensions.contains(&debug_utils) {
                extensions.push(debug_utils);
            }
        }
        extensions
    }

    /// Validation fallback: clear the validation-ADDED component only —
    /// SDL-required entries (including an SDL-required debug-utils) and
    /// the portability decision are untouched, so the instance flag
    /// survives too.
    fn without_validation(&self) -> InstanceExtensionPlan {
        InstanceExtensionPlan {
            sdl_required: self.sdl_required.clone(),
            portability_enumeration: self.portability_enumeration,
            validation: false,
        }
    }

    /// Instance flags derived from the same plan that produces the
    /// extension list — flag and extension cannot diverge in either
    /// creation attempt.
    fn create_flags(&self) -> vk::InstanceCreateFlags {
        if self.portability_enumeration {
            vk::InstanceCreateFlags::ENUMERATE_PORTABILITY_KHR
        } else {
            vk::InstanceCreateFlags::empty()
        }
    }
}

/// First-occurrence deduplication of a name list (Mac 2 review
/// follow-up A): SDL's required list is authoritative but not
/// guaranteed duplicate-free.
fn dedup_keep_first(items: &[String]) -> Vec<String> {
    let mut unique: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        if !unique.contains(item) {
            unique.push(item.clone());
        }
    }
    unique
}

/// Owned CString copies of an extension list plus raw pointers into
/// them; the CStrings live as long as the pointers' use (both drop
/// together at the end of each creation attempt).
fn extension_name_pointers(extensions: &[String]) -> (Vec<CString>, Vec<*const core::ffi::c_char>) {
    let owned: Vec<CString> = extensions
        .iter()
        .map(|name| CString::new(name.as_str()).unwrap_or_default())
        .collect();
    let pointers = owned.iter().map(|c| c.as_ptr()).collect();
    (owned, pointers)
}

/// Device-extension capability scan (Mac 2, change B): which
/// policy-relevant extensions a candidate device advertises.
/// `VK_KHR_swapchain` stays mandatory for candidacy;
/// `VK_KHR_portability_subset` only parameterizes device creation — the
/// spec requires enabling it when advertised
/// (`VUID-VkDeviceCreateInfo-pProperties-04451`), which is why MoltenVK
/// device creation fails without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DeviceExtensionCapabilities {
    swapchain: bool,
    portability_subset: bool,
}

fn device_extension_capabilities(advertised: &[String]) -> DeviceExtensionCapabilities {
    let has = |name: &CStr| advertised.contains(&name.to_string_lossy().into_owned());
    DeviceExtensionCapabilities {
        swapchain: has(khr::swapchain::NAME),
        portability_subset: has(khr::portability_subset::NAME),
    }
}

/// Device-extension selection for the chosen device (Mac 2, change B):
/// swapchain always; portability subset only when advertised. The
/// single source for device creation — no second selection path.
fn select_device_extensions(portability_subset: bool) -> Vec<&'static CStr> {
    let mut extensions = vec![khr::swapchain::NAME];
    if portability_subset {
        extensions.push(khr::portability_subset::NAME);
    }
    extensions
}

/// Candidate selection; the result carries the chosen device's
/// portability-subset capability alongside the queue family (Mac 2,
/// change B) — an extension of the existing tuple, not a divergent
/// selection path.
fn pick_physical_device(
    instance: &Instance,
    surface_ext: &khr::surface::Instance,
    surface: vk::SurfaceKHR,
) -> Option<(vk::PhysicalDevice, u32, bool)> {
    let devices = unsafe { instance.enumerate_physical_devices() }.ok()?;
    let mut best: Option<(u64, vk::PhysicalDevice, u32, bool)> = None;
    for device in devices {
        let properties = unsafe { instance.get_physical_device_properties(device) };
        let score = match properties.device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU => 100,
            vk::PhysicalDeviceType::INTEGRATED_GPU => 10,
            _ => 1,
        };
        let families = unsafe { instance.get_physical_device_queue_family_properties(device) };
        let mut chosen_family = None;
        for (index, family) in families.iter().enumerate() {
            if family.queue_flags.contains(vk::QueueFlags::GRAPHICS)
                && unsafe {
                    surface_ext.get_physical_device_surface_support(device, index as u32, surface)
                }
                .unwrap_or(false)
            {
                chosen_family = Some(index as u32);
                break;
            }
        }
        let Some(family) = chosen_family else {
            continue;
        };
        let advertised: Vec<String> =
            unsafe { instance.enumerate_device_extension_properties(device) }
                .unwrap_or_default()
                .iter()
                .map(|property| {
                    unsafe { CStr::from_ptr(property.extension_name.as_ptr()) }
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
        // Mac 2, change B: swapchain stays mandatory; portability subset
        // is recorded for device creation, not for candidacy.
        let capabilities = device_extension_capabilities(&advertised);
        if !capabilities.swapchain {
            continue;
        }
        let formats = unsafe { surface_ext.get_physical_device_surface_formats(device, surface) }
            .unwrap_or_default();
        if formats.is_empty() {
            continue;
        }
        if best.is_none_or(|(best_score, _, _, _)| score > best_score) {
            best = Some((score, device, family, capabilities.portability_subset));
        }
    }
    best.map(|(_, device, family, portability_subset)| (device, family, portability_subset))
}

/// Swapchain + attachments + framebuffers. Recreated on resize,
/// out-of-date, and suboptimal-present.
pub(crate) struct SwapchainState {
    pub swapchain: vk::SwapchainKHR,
    /// Last drawable size requested by the application. A surface can pin
    /// or clamp its actual extent to a different size.
    requested_size: (u32, u32),
    /// The raw swapchain images (framebuffer-owner side of the views —
    /// kept for the bounded frame-capture readback, which copies the
    /// presented image; the swapchain itself owns their lifetime).
    pub images: Vec<vk::Image>,
    pub views: Vec<vk::ImageView>,
    pub framebuffers: Vec<vk::Framebuffer>,
    /// One depth attachment per swapchain image (corrective finding A):
    /// two in-flight submissions render into different images, so they
    /// never share depth state.
    pub depth_images: Vec<vk::Image>,
    pub depth_memories: Vec<vk::DeviceMemory>,
    pub depth_views: Vec<vk::ImageView>,
    pub extent: vk::Extent2D,
    pub format: vk::Format,
    pub present_mode: vk::PresentModeKHR,
    pub image_count: u32,
    pub depth_format: vk::Format,
    /// Whether the surface's swapchain images support TRANSFER_SRC
    /// usage (the frame-capture readback path; requested at creation
    /// whenever supported).
    pub transfer_src_supported: bool,
    /// Monotonic generation for cache invalidation (corrective finding 7).
    /// Vulkan permits handle reuse, so a numerical view handle could
    /// recur after destruction. The generation increases forever so
    /// retired generations' keys can never recur.
    pub generation: u64,
}

/// Picks the surface format (BGRA8_UNORM + sRGB non-linear preferred — the
/// classic 24-bit surface). Called once before pipelines, which bake the
/// format into the render pass; recreation re-picks and rebuilds if it
/// changed (rare).
pub(crate) fn pick_surface_format(
    ctx: &VulkanContext,
) -> Result<(vk::Format, vk::ColorSpaceKHR), RendererError> {
    let formats = unsafe {
        ctx.surface_ext
            .get_physical_device_surface_formats(ctx.physical_device, ctx.surface)
    }
    .map_err(|error| RendererError::vk(error, "surface formats"))?;
    formats
        .iter()
        .find(|f| {
            f.format == vk::Format::B8G8R8A8_UNORM
                && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
        })
        .or_else(|| formats.first())
        .map(|f| (f.format, f.color_space))
        .ok_or_else(|| RendererError::NoDevice("surface has no formats".into()))
}

impl SwapchainState {
    /// `depth_format` is chosen once by the owning renderer and shared
    /// with the pipeline state, so the render pass and the depth
    /// attachments cannot drift apart (corrective finding F).
    /// `old_swapchain` is set during recreation: creating the replacement
    /// while the current swapchain still owns the native window is the
    /// documented `VkSwapchainCreateInfoKHR::oldSwapchain` flow — without
    /// it, native-window platforms (Win32) fail with
    /// ERROR_NATIVE_WINDOW_IN_USE_KHR, found on the lead's rig when
    /// maximizing (corrective follow-up, 2026-10-20). Null for a fresh
    /// renderer.
    // Swapchain creation inputs plus the recreation linkage; bundling
    // would hide which object pairs with which lifetime.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        ctx: &VulkanContext,
        window_size: (u32, u32),
        preference: PresentModePreference,
        format: vk::Format,
        color_space: vk::ColorSpaceKHR,
        render_pass: vk::RenderPass,
        depth_format: vk::Format,
        old_swapchain: vk::SwapchainKHR,
    ) -> Result<SwapchainState, RendererError> {
        // Partial-initialization cleanup (corrective finding G): build
        // into a state whose handles start dead; on failure `destroy`
        // releases exactly what was created (null/empty entries are
        // no-ops per the Vulkan spec).
        let mut state = SwapchainState {
            swapchain: vk::SwapchainKHR::null(),
            requested_size: window_size,
            images: Vec::new(),
            views: Vec::new(),
            framebuffers: Vec::new(),
            depth_images: Vec::new(),
            depth_memories: Vec::new(),
            depth_views: Vec::new(),
            extent: vk::Extent2D::default(),
            format,
            present_mode: vk::PresentModeKHR::FIFO,
            image_count: 0,
            depth_format,
            transfer_src_supported: false,
            generation: next_swapchain_generation(),
        };
        if let Err(error) = state.build(
            ctx,
            window_size,
            preference,
            format,
            color_space,
            render_pass,
            old_swapchain,
        ) {
            state.destroy(ctx);
            return Err(error);
        }
        Ok(state)
    }

    // Same rationale as `new`: the creation inputs plus the recreation
    // linkage, spread for lifetime clarity.
    #[allow(clippy::too_many_arguments)]
    fn build(
        &mut self,
        ctx: &VulkanContext,
        window_size: (u32, u32),
        preference: PresentModePreference,
        format: vk::Format,
        color_space: vk::ColorSpaceKHR,
        render_pass: vk::RenderPass,
        old_swapchain: vk::SwapchainKHR,
    ) -> Result<(), RendererError> {
        let surface = ctx.surface;
        let capabilities = unsafe {
            ctx.surface_ext
                .get_physical_device_surface_capabilities(ctx.physical_device, surface)
        }
        .map_err(|error| RendererError::vk(error, "surface capabilities"))?;

        let extent = clamp_extent(window_size, &capabilities);
        if extent.width == 0 || extent.height == 0 {
            return Err(RendererError::Vulkan(
                "zero window extent (minimized?)".into(),
            ));
        }
        self.extent = extent;
        self.present_mode = resolve_present_mode(preference, &ctx.supported_present_modes);

        let desired_images = (capabilities.min_image_count + 1).max(FRAMES_IN_FLIGHT);
        let max_images = if capabilities.max_image_count == 0 {
            desired_images
        } else {
            capabilities.max_image_count
        };
        let image_count = desired_images.min(max_images);

        // Frame-capture readback (the orientation corrective): the
        // bounded capture path copies the presented image to the host,
        // which requires TRANSFER_SRC usage on the swapchain images —
        // requested whenever the surface supports it (the spec only
        // guarantees COLOR_ATTACHMENT; universal in practice).
        self.transfer_src_supported = capabilities
            .supported_usage_flags
            .contains(vk::ImageUsageFlags::TRANSFER_SRC);
        let mut image_usage = vk::ImageUsageFlags::COLOR_ATTACHMENT;
        if self.transfer_src_supported {
            image_usage |= vk::ImageUsageFlags::TRANSFER_SRC;
        }

        let create_info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(image_count)
            .image_format(format)
            .image_color_space(color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(image_usage)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(capabilities.current_transform)
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(self.present_mode)
            .old_swapchain(old_swapchain)
            .clipped(true);
        self.swapchain = unsafe { ctx.swapchain_ext.create_swapchain(&create_info, None) }
            .map_err(|error| RendererError::vk(error, "create_swapchain"))?;
        let images = unsafe { ctx.swapchain_ext.get_swapchain_images(self.swapchain) }
            .map_err(|error| RendererError::vk(error, "get_swapchain_images"))?;
        self.image_count = images.len() as u32;
        // Kept for the capture readback (CopyImageToBuffer needs the
        // raw images; the views are the rendering side).
        self.images = images;

        let subresource = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            level_count: 1,
            layer_count: 1,
            ..Default::default()
        };
        for image in &self.images {
            let view_info = vk::ImageViewCreateInfo::default()
                .image(*image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
                .subresource_range(subresource);
            let view = unsafe { ctx.device.create_image_view(&view_info, None) }
                .map_err(|error| RendererError::vk(error, "create_image_view"))?;
            self.views.push(view);
        }

        // One depth attachment per swapchain image (corrective finding
        // A): in-flight submissions into different images never share
        // depth state.
        for _ in &self.images {
            let depth_info = vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .extent(vk::Extent3D {
                    width: extent.width,
                    height: extent.height,
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(1)
                .format(self.depth_format)
                .tiling(vk::ImageTiling::OPTIMAL)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
                .samples(vk::SampleCountFlags::TYPE_1)
                .sharing_mode(vk::SharingMode::EXCLUSIVE);
            let depth_image = unsafe { ctx.device.create_image(&depth_info, None) }
                .map_err(|error| RendererError::vk(error, "create depth image"))?;
            self.depth_images.push(depth_image);
            let memory_requirements =
                unsafe { ctx.device.get_image_memory_requirements(depth_image) };
            let depth_memory_index = find_memory_type(
                ctx,
                memory_requirements.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .ok_or_else(|| RendererError::NoDevice("no DEVICE_LOCAL memory type".into()))?;
            let allocate_info = vk::MemoryAllocateInfo::default()
                .allocation_size(memory_requirements.size)
                .memory_type_index(depth_memory_index);
            let depth_memory = unsafe { ctx.allocate_memory(&allocate_info) }
                .map_err(|error| RendererError::vk(error, "allocate depth memory"))?;
            self.depth_memories.push(depth_memory);
            unsafe { ctx.device.bind_image_memory(depth_image, depth_memory, 0) }
                .map_err(|error| RendererError::vk(error, "bind depth memory"))?;
            let depth_view_info = vk::ImageViewCreateInfo::default()
                .image(depth_image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(self.depth_format)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::DEPTH,
                    level_count: 1,
                    layer_count: 1,
                    ..Default::default()
                });
            let depth_view = unsafe { ctx.device.create_image_view(&depth_view_info, None) }
                .map_err(|error| RendererError::vk(error, "create depth view"))?;
            self.depth_views.push(depth_view);
        }

        for index in 0..self.views.len() {
            let attachments = [self.views[index], self.depth_views[index]];
            let framebuffer_info = vk::FramebufferCreateInfo::default()
                .render_pass(render_pass)
                .attachments(&attachments)
                .width(extent.width)
                .height(extent.height)
                .layers(1);
            let framebuffer = unsafe { ctx.device.create_framebuffer(&framebuffer_info, None) }
                .map_err(|error| RendererError::vk(error, "create_framebuffer"))?;
            self.framebuffers.push(framebuffer);
        }

        Ok(())
    }

    pub(crate) fn destroy(&mut self, ctx: &VulkanContext) {
        unsafe {
            for framebuffer in &self.framebuffers {
                ctx.device.destroy_framebuffer(*framebuffer, None);
            }
            self.framebuffers.clear();
            for view in &self.depth_views {
                ctx.device.destroy_image_view(*view, None);
            }
            self.depth_views.clear();
            for image in &self.depth_images {
                ctx.device.destroy_image(*image, None);
            }
            self.depth_images.clear();
            for memory in &self.depth_memories {
                ctx.device.free_memory(*memory, None);
            }
            self.depth_memories.clear();
            for view in &self.views {
                ctx.device.destroy_image_view(*view, None);
            }
            self.views.clear();
            // The raw swapchain images are owned by the swapchain
            // itself (destroyed below) — only the handles are dropped.
            self.images.clear();
            if self.swapchain != vk::SwapchainKHR::null() {
                ctx.swapchain_ext.destroy_swapchain(self.swapchain, None);
                self.swapchain = vk::SwapchainKHR::null();
            }
        }
    }
}

fn clamp_extent(
    window_size: (u32, u32),
    capabilities: &vk::SurfaceCapabilitiesKHR,
) -> vk::Extent2D {
    let current = capabilities.current_extent;
    // Zero-sized inputs are deliberately NOT forced to 1: a zero extent
    // means the drawable is temporarily unavailable, which render_frame
    // defers before any swapchain is created — the 1×1 fabrication was
    // the Win32 minimize/transition defect (VUID
    // VkSwapchainCreateInfoKHR-pNext-07781, found on the rig 2026-10-20).
    // max == 0 reads as unbounded, per the swapchain extension's
    // convention for extent limits.
    let clamp =
        |value: u32, min: u32, max: u32| value.clamp(min, if max == 0 { u32::MAX } else { max });
    let (width, height) = if current.width == u32::MAX && current.height == u32::MAX {
        window_size
    } else {
        (current.width, current.height)
    };
    vk::Extent2D {
        width: clamp(
            width,
            capabilities.min_image_extent.width,
            capabilities.max_image_extent.width,
        ),
        height: clamp(
            height,
            capabilities.min_image_extent.height,
            capabilities.max_image_extent.height,
        ),
    }
}

/// True when the surface's drawable is temporarily unavailable — a
/// zero-sized current extent (Win32 reports (0,0) for minimized and
/// mid-transition surfaces; with min/max also 0 there is provably no
/// valid extent). Pure policy over the capabilities.
pub(crate) fn surface_extent_unavailable(capabilities: &vk::SurfaceCapabilitiesKHR) -> bool {
    capabilities.current_extent.width == 0 || capabilities.current_extent.height == 0
}

/// Live availability probe used once per frame before any recreation
/// trigger: a deferred frame creates nothing, destroys nothing, and
/// errors on nothing.
fn drawable_extent(
    ctx: &VulkanContext,
    window_size: (u32, u32),
) -> Result<Option<vk::Extent2D>, RendererError> {
    let capabilities = unsafe {
        ctx.surface_ext
            .get_physical_device_surface_capabilities(ctx.physical_device, ctx.surface)
    }
    .map_err(|error| RendererError::vk(error, "surface capabilities"))?;
    Ok((!surface_extent_unavailable(&capabilities))
        .then(|| clamp_extent(window_size, &capabilities)))
}

/// The hint-gated probe decision (pure over the frame-state flags —
/// extracted for the state-transition regression, the batch-3 review's
/// corrective item 3). THE LIFECYCLE CONTRACT (finding 1): a
/// `ProbeDeferred` outcome defers the frame WITHOUT touching
/// `needs_recreate` — a pending recreation's intent survives the
/// deferral and is consumed only by the recreate block on a frame
/// where the probe reports the drawable available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SurfaceProbeDecision {
    /// No evidence of trouble — no driver round-trip this frame.
    Skip,
    /// A probe was warranted and the drawable is available: clear the
    /// deferral hint; a pending recreation proceeds to its own
    /// consume-and-recreate block.
    ProbeAvailable,
    /// A probe was warranted and the drawable is unavailable: defer
    /// the frame, latch the deferral hint, and PRESERVE any pending
    /// recreation intent.
    ProbeDeferred,
}

/// The pure decision table: `surface_available` is the probed answer,
/// consulted only when the hints warrant a probe.
pub(crate) fn surface_probe_decision(
    surface_probe_pending: bool,
    needs_recreate: bool,
    surface_available: bool,
) -> SurfaceProbeDecision {
    if !surface_probe_pending && !needs_recreate {
        SurfaceProbeDecision::Skip
    } else if surface_available {
        SurfaceProbeDecision::ProbeAvailable
    } else {
        SurfaceProbeDecision::ProbeDeferred
    }
}

/// Depth format aspect flags for a given Vulkan format.
///
/// D32_SFLOAT and D16_UNORM → `DEPTH`
/// D24_UNORM_S8_UINT → `DEPTH | STENCIL`
/// Other formats → `COLOR` (only valid where the caller knows the format
/// is a depth format and the caller is responsible for ensuring this).
///
/// Pure function — unit-testable without a GPU.
pub(crate) fn depth_aspect(format: vk::Format) -> vk::ImageAspectFlags {
    match format {
        vk::Format::D32_SFLOAT | vk::Format::D16_UNORM => vk::ImageAspectFlags::DEPTH,
        vk::Format::D24_UNORM_S8_UINT => {
            vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL
        }
        // For color images and depth formats we don't explicitly handle,
        // COLOR is the safe default (the caller must not pass a depth
        // format this doesn't cover).
        _ => vk::ImageAspectFlags::COLOR,
    }
}

#[cfg(test)]
mod depth_format_tests {
    use super::depth_aspect;
    use ash::vk;

    #[test]
    fn d32_sfloat_uses_depth_aspect() {
        assert_eq!(
            depth_aspect(vk::Format::D32_SFLOAT),
            vk::ImageAspectFlags::DEPTH
        );
    }

    #[test]
    fn d24_unorm_s8_uint_uses_depth_stencil_aspect() {
        assert_eq!(
            depth_aspect(vk::Format::D24_UNORM_S8_UINT),
            vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL
        );
    }

    #[test]
    fn d16_unorm_uses_depth_aspect() {
        assert_eq!(
            depth_aspect(vk::Format::D16_UNORM),
            vk::ImageAspectFlags::DEPTH
        );
    }

    #[test]
    fn color_formats_use_color_aspect() {
        assert_eq!(
            depth_aspect(vk::Format::B8G8R8A8_UNORM),
            vk::ImageAspectFlags::COLOR
        );
    }
}

/// Depth format for the graphics device; chosen **once per renderer** and
/// shared by the render pass and every depth attachment, so they cannot
/// drift apart (corrective finding F).
///
/// Uses the preference to override the negotiated pick only if the device's
/// optimal tiling supports the forced format; otherwise falls back to the
/// negotiated pick. Never creates an invalid device state.
pub(crate) fn pick_depth_format(
    instance: &Instance,
    physical_device: vk::PhysicalDevice,
    preference: DepthFormatPreference,
) -> vk::Format {
    // Query support for each format once for the fallback logic.
    let supports_d32 = unsafe {
        instance
            .get_physical_device_format_properties(physical_device, vk::Format::D32_SFLOAT)
            .optimal_tiling_features
            .contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT)
    };
    let supports_d24_s8 = unsafe {
        instance
            .get_physical_device_format_properties(physical_device, vk::Format::D24_UNORM_S8_UINT)
            .optimal_tiling_features
            .contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT)
    };
    let supports_d16 = unsafe {
        instance
            .get_physical_device_format_properties(physical_device, vk::Format::D16_UNORM)
            .optimal_tiling_features
            .contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT)
    };

    // Resolve the preference: use the forced format only if supported.
    crate::config::resolve_depth_format(preference, supports_d24_s8, supports_d16, {
        // The default negotiated pick (D32 > D24_S8 > D16).
        if supports_d32 {
            vk::Format::D32_SFLOAT
        } else if supports_d24_s8 {
            vk::Format::D24_UNORM_S8_UINT
        } else {
            vk::Format::D16_UNORM
        }
    })
}

/// Monotonic swapchain generation counter for cache invalidation.
/// Vulkan permits handle reuse, so a numerical view handle could recur
/// after destruction. The generation is host-side bookkeeping only,
/// never packed into any deterministic byte path; it increases forever
/// so retired generations' keys can never recur. (corrective finding 7)
pub(crate) static SWAPCHAIN_GENERATION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);

pub(crate) fn next_swapchain_generation() -> u64 {
    SWAPCHAIN_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn find_memory_type(
    ctx: &VulkanContext,
    type_bits: u32,
    properties: vk::MemoryPropertyFlags,
) -> Option<u32> {
    let memory_properties = unsafe {
        ctx.instance
            .get_physical_device_memory_properties(ctx.physical_device)
    };
    for (index, memory_type) in memory_properties.memory_types.iter().enumerate() {
        if type_bits & (1 << index) != 0 && memory_type.property_flags.contains(properties) {
            return Some(index as u32);
        }
    }
    None
}

/// Render pass, pipelines, and the scene/overlay vertex + index buffers.
pub(crate) struct PipelineState {
    pub render_pass: vk::RenderPass,
    lit_pipeline: vk::Pipeline,
    lit_layout: vk::PipelineLayout,
    proxy_pipeline: vk::Pipeline,
    proxy_layout: vk::PipelineLayout,
    pub scene_pipeline: vk::Pipeline,
    pub scene_layout: vk::PipelineLayout,
    pub overlay_pipeline: vk::Pipeline,
    pub overlay_layout: vk::PipelineLayout,
    pub scene_vbo: vk::Buffer,
    pub scene_vbo_memory: vk::DeviceMemory,
    pub scene_ibo: vk::Buffer,
    pub scene_ibo_memory: vk::DeviceMemory,
    pub scene_index_count: u32,
    pub region_vbo: vk::Buffer,
    pub region_vbo_memory: vk::DeviceMemory,
    /// The Cycle 6-E instanced-proxy resources (the two mesh VBOs +
    /// the mapped per-slot instance buffer), created LAZILY at the
    /// first populated corner frame — the region-buffer precedent:
    /// the unpopulated path pays nothing. Dies with this state (the
    /// proxy pipeline reading it does); a replacement re-ensures an
    /// identical set before installation (finding A).
    pub proxy: Option<proxy_draw::ProxyResources>,
    /// The Cycle 6-F shadow state (the per-slot shadow maps, the
    /// depth-only pass, the shadowed lit pipeline, and the readback
    /// path), created LAZILY at the first shadowed corner frame —
    /// the region-buffer precedent: the unshadowed corner path pays
    /// nothing and its allocation pins are untouched. Dies with this
    /// state (the pipelines reading it do); a replacement re-ensures
    /// an identical set before installation (finding A).
    pub shadows: Option<shadow_draw::ShadowResources>,
    /// The Era-1 sky state (the fullscreen-triangle pipeline + the
    /// mapped per-slot SkyParams uniform), created LAZILY at the
    /// first sky frame — the region-buffer precedent: the skyless
    /// path pays nothing and its allocation pins are untouched. Dies
    /// with this state (the pipeline reading it does); a replacement
    /// re-ensures an identical set before installation (finding A).
    pub sky: Option<sky_draw::SkyResources>,
    /// The Era-1 post state (the HDR render-target chain: the HDR
    /// scene pass, the bloom mip chain, and the composite pass),
    /// created LAZILY at the first post frame — the region-buffer
    /// precedent: the postless path pays nothing and its allocation
    /// pins are untouched (a postless frame's bytes are identical
    /// to the pre-package renderer's). Dies with this state; a
    /// replacement re-ensures an identical set before installation
    /// (finding A).
    pub post: Option<post_draw::PostResources>,
    /// The Era-1 wet-ground reflection state (the mirrored-scene
    /// pass + the ground re-draw), created LAZILY at the first wet
    /// frame — the same discipline: the dry path pays nothing and
    /// stays byte-identical. Depends on the POST STATE'S HDR RENDER
    /// PASS OBJECT (the reflection pass is render-pass-compatible
    /// with it — the identical attachment descriptions), so a wet
    /// renderer always carries the post state; the post CHAIN
    /// itself stays optional.
    pub reflection: Option<reflection_draw::ReflectionResources>,
    /// Per-slot vertex capacity of the dynamic region-scene buffer
    /// (Stage 4 graphical integration — the same disjoint per-slot
    /// pattern as the overlay).
    pub region_capacity: u32,
    /// Byte size of one frame slot's region-scene region.
    pub region_slot_bytes: vk::DeviceSize,
    region_mapped: *mut core::ffi::c_void,
}

impl PipelineState {
    /// `depth_format` comes from the renderer's single pick (corrective
    /// finding F) so the render pass matches the swapchain's depth
    /// attachments exactly. `demo_scene` is the P3 preference: the
    /// static Stage-2 demonstration scene is uploaded ONLY on the
    /// explicit opt-in — an omitted renderer owns no scene buffers and
    /// the record step's guarded draw skips the static scene.
    pub(crate) fn new(
        ctx: &VulkanContext,
        color_format: vk::Format,
        depth_format: vk::Format,
        demo_scene: DemoScenePreference,
    ) -> Result<PipelineState, RendererError> {
        // Partial-initialization cleanup (corrective finding G): handles
        // start dead; on failure `destroy` releases exactly what exists.
        let mut state = PipelineState {
            render_pass: vk::RenderPass::null(),
            lit_pipeline: vk::Pipeline::null(),
            lit_layout: vk::PipelineLayout::null(),
            proxy_pipeline: vk::Pipeline::null(),
            proxy_layout: vk::PipelineLayout::null(),
            scene_pipeline: vk::Pipeline::null(),
            scene_layout: vk::PipelineLayout::null(),
            overlay_pipeline: vk::Pipeline::null(),
            overlay_layout: vk::PipelineLayout::null(),
            scene_vbo: vk::Buffer::null(),
            scene_vbo_memory: vk::DeviceMemory::null(),
            scene_ibo: vk::Buffer::null(),
            scene_ibo_memory: vk::DeviceMemory::null(),
            scene_index_count: 0,
            region_vbo: vk::Buffer::null(),
            region_vbo_memory: vk::DeviceMemory::null(),
            region_capacity: 0,
            region_slot_bytes: 0,
            region_mapped: core::ptr::null_mut(),
            proxy: None,
            shadows: None,
            sky: None,
            post: None,
            reflection: None,
        };
        if let Err(error) = state.build(ctx, color_format, depth_format, demo_scene) {
            state.destroy(ctx);
            return Err(error);
        }
        Ok(state)
    }

    fn build(
        &mut self,
        ctx: &VulkanContext,
        color_format: vk::Format,
        depth_format: vk::Format,
        demo_scene: DemoScenePreference,
    ) -> Result<(), RendererError> {
        Self::create_pipelines(ctx, color_format, depth_format, self)?;

        // Scene buffers: upload once through a staging buffer into
        // device-local memory — ONLY on the explicit opt-in (Cycle 3
        // P3): the reusable renderer must not impose the Stage-2
        // demonstration content on a game's scene. `Omitted` leaves
        // the handles null and `scene_index_count` 0, so the record
        // step's guarded draw skips the static scene entirely and the
        // renderer pays no scene-buffer allocation at all.
        if demo_scene == DemoScenePreference::Included {
            let (vertices, indices) = crate::scene::build_scene();
            self.scene_index_count = indices.len() as u32;
            let (scene_vbo, scene_vbo_memory) = create_device_local_buffer_with_data(
                ctx,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                bytemuck::cast_slice(&vertices),
            )?;
            self.scene_vbo = scene_vbo;
            self.scene_vbo_memory = scene_vbo_memory;
            let (scene_ibo, scene_ibo_memory) = create_device_local_buffer_with_data(
                ctx,
                vk::BufferUsageFlags::INDEX_BUFFER,
                bytemuck::cast_slice(&indices),
            )?;
            self.scene_ibo = scene_ibo;
            self.scene_ibo_memory = scene_ibo_memory;
        }

        // Dynamic region-scene buffer (Stage 4 graphical integration):
        // NOT created here — it is allocated lazily on first use
        // (`ensure_region_capacity`) so the default renderer path
        // (no dynamic region geometry) pays no allocation, mapping, or
        // memory cost (Stage 4 final-verification finding). The pattern
        // itself is the overlay's: persistently mapped, disjoint
        // per-slot regions, fenced-slot writes.
        Ok(())
    }

    /// Allocates the dynamic region-scene buffer on first use (the
    /// overlay's mapped-buffer pattern): persistently mapped host
    /// visible memory with one disjoint region per frame slot, sized by
    /// `region_scene_capacity_vertices()` divided by
    /// `FRAMES_IN_FLIGHT`. Called from `render_frame` OUTSIDE command
    /// recording whenever a frame carries region geometry; a no-op once
    /// allocated. Partial-state safe: `destroy` handles the null
    /// handles this leaves on the default path.
    pub(crate) fn ensure_region_capacity(
        &mut self,
        ctx: &VulkanContext,
    ) -> Result<(), RendererError> {
        if self.region_vbo != vk::Buffer::null() {
            return Ok(());
        }
        self.region_capacity = crate::scene::region_scene_capacity_vertices() / FRAMES_IN_FLIGHT;
        self.region_slot_bytes = self.region_capacity as vk::DeviceSize
            * std::mem::size_of::<SceneVertex>() as vk::DeviceSize;
        let region_size = self.region_slot_bytes * FRAMES_IN_FLIGHT as vk::DeviceSize;
        let (region_vbo, region_vbo_memory, region_mapped) =
            create_mapped_buffer(ctx, vk::BufferUsageFlags::VERTEX_BUFFER, region_size)?;
        self.region_vbo = region_vbo;
        self.region_vbo_memory = region_vbo_memory;
        self.region_mapped = region_mapped;
        Ok(())
    }

    /// Allocates the Cycle 6-E proxy resources on first populated
    /// corner frame (the same lazy discipline as the region buffer:
    /// the unpopulated corner path pays nothing). A no-op once
    /// allocated. Partial-state safe: `destroy` handles the `None`
    /// this leaves on the unpopulated path.
    pub(crate) fn ensure_proxy_resources(
        &mut self,
        ctx: &VulkanContext,
    ) -> Result<(), RendererError> {
        if self.proxy.is_some() {
            return Ok(());
        }
        self.proxy = Some(proxy_draw::ProxyResources::new(ctx)?);
        Ok(())
    }

    /// Allocates the Cycle 6-F shadow state on the first shadowed
    /// corner frame (the same lazy discipline: the unshadowed corner
    /// path pays nothing — its allocation pins are untouched). The
    /// shadowed lit pipeline renders inside THIS state's render pass,
    /// so the ensure carries it; a no-op once allocated.
    pub(crate) fn ensure_shadow_resources(
        &mut self,
        ctx: &VulkanContext,
    ) -> Result<(), RendererError> {
        if self.shadows.is_some() {
            return Ok(());
        }
        let corner = ctx.corner.as_ref().ok_or_else(|| {
            RendererError::Vulkan("shadows require the Corner scene preference".into())
        })?;
        self.shadows = Some(shadow_draw::ShadowResources::new(
            ctx,
            self.render_pass,
            corner,
        )?);
        Ok(())
    }

    /// Allocates the Era-1 sky state on the first sky frame (the
    /// same lazy discipline: the skyless path pays nothing — its
    /// allocation pins are untouched). The sky pipeline renders
    /// inside THIS state's render pass, so the ensure carries it; a
    /// no-op once allocated.
    pub(crate) fn ensure_sky_resources(
        &mut self,
        ctx: &VulkanContext,
    ) -> Result<(), RendererError> {
        if self.sky.is_some() {
            return Ok(());
        }
        self.sky = Some(sky_draw::SkyResources::new(ctx, self.render_pass)?);
        Ok(())
    }

    /// Allocates the Era-1 post state on the first post frame (the
    /// same lazy discipline: the postless path pays nothing — its
    /// allocation pins are untouched, and a postless frame's bytes
    /// are identical to the pre-package renderer's). The state's
    /// extent-bound targets are NOT built here (they need the
    /// live swapchain's extent and views — the frame flow ensures
    /// them after the acquire); a no-op once allocated.
    pub(crate) fn ensure_post_resources(
        &mut self,
        ctx: &VulkanContext,
        swapchain_format: vk::Format,
        depth_format: vk::Format,
    ) -> Result<(), RendererError> {
        if self.post.is_some() {
            return Ok(());
        }
        if ctx.corner.is_none() {
            return Err(RendererError::Vulkan(
                "the post chain requires the Corner scene preference".into(),
            ));
        }
        self.post = Some(post_draw::PostResources::new(
            ctx,
            swapchain_format,
            depth_format,
            self.lit_layout,
            self.proxy_layout,
        )?);
        Ok(())
    }

    /// Allocates the Era-1 wet-ground reflection state on the first
    /// wet frame (the same lazy discipline: the dry path pays
    /// nothing and stays byte-identical). The reflection pass rides
    /// the POST STATE'S HDR RENDER PASS — a wet renderer always
    /// carries the post state (the pass object), whether or not
    /// the post chain itself runs.
    pub(crate) fn ensure_reflection_resources(
        &mut self,
        ctx: &VulkanContext,
    ) -> Result<(), RendererError> {
        if self.reflection.is_some() {
            return Ok(());
        }
        let corner = ctx.corner.as_ref().ok_or_else(|| {
            RendererError::Vulkan("the wet reflection requires the Corner scene preference".into())
        })?;
        let post = self.post.as_ref().ok_or_else(|| {
            RendererError::Vulkan(
                "the wet reflection requires the post state's HDR render pass".into(),
            )
        })?;
        self.reflection = Some(reflection_draw::ReflectionResources::new(
            ctx,
            corner,
            post.hdr_render_pass(),
            self.render_pass,
        )?);
        Ok(())
    }

    /// Ensures every state the CURRENT frame's active layers need,
    /// against the live swapchain generation — idempotent (each
    /// ensure_* is an existence check), called BEFORE the recreation
    /// decision (so the replacement knows which lazy states to
    /// replicate — the finding-A discipline) and AGAIN after any
    /// recreation (so the generation that draws carries every active
    /// variant — review finding 6: the replacement replicates only
    /// proxy/indirect/shadow/post, never sky, reflection, or the HDR
    /// variants).
    #[allow(clippy::too_many_arguments)]
    fn ensure_frame_targets_and_resources(
        &mut self,
        ctx: &mut VulkanContext,
        swapchain: &SwapchainState,
        slot: usize,
        proxy: Option<&proxy_draw::ProxyFrameUpload<'_>>,
        shadowed: Option<&shadow_draw::ShadowFrameParams>,
        sky: Option<&sky_draw::SkyFrameParams>,
        post: bool,
        wet: Option<&reflection_draw::WetCamera>,
        region_vertices: &[SceneVertex],
        foreground_vertices: &[SceneVertex],
    ) -> Result<(), RendererError> {
        // Lazy region-scene buffer (Stage 4 final-verification finding):
        // allocated on first use only, outside command recording — the
        // default renderer path (no dynamic region geometry) pays no
        // allocation, mapping, or memory cost.
        if !region_vertices.is_empty() || !foreground_vertices.is_empty() {
            self.ensure_region_capacity(ctx)?;
        }

        // The 6-E lazy proxy resources (the same discipline): created at
        // the first populated frame only, outside command recording —
        // the unpopulated corner path pays nothing. Structural invariant:
        // a proxy upload implies corner resources (the safe layer's
        // scene-preference gate), hence `ctx.corner` in the pattern.
        // The 6-E2 indirect command state is one level lazier still:
        // created at the first INDIRECT-mode populated frame only, so
        // the direct instanced path's allocation pins (the 6-E gates)
        // pay nothing for the mechanism they never touch.
        if proxy.is_some() && ctx.corner.is_some() {
            self.ensure_proxy_resources(ctx)?;
            if proxy.is_some_and(|upload| matches!(upload.mode, crate::ProxyDrawMode::Indirect))
                && let Some(proxy_state) = self.proxy.as_mut()
            {
                proxy_state.ensure_indirect(ctx)?;
            }
        }

        // The 6-F lazy shadow state (the same discipline): created at
        // the first shadowed frame only, outside command recording — the
        // unshadowed corner path pays nothing. Structural invariant: a
        // shadowed frame implies corner resources (the safe layer's
        // scene-preference gate), which the ensure itself re-checks.
        if shadowed.is_some() {
            self.ensure_shadow_resources(ctx)?;
        }

        // The Era-1 lazy sky state (the same discipline): created at the
        // first sky frame only, outside command recording — the skyless
        // path pays nothing, and a pipeline replacement re-ensures an
        // identical set at the next sky frame (the 6-F finding-A shape).
        if sky.is_some() {
            self.ensure_sky_resources(ctx)?;
        }

        // The Era-1 lazy post state (the same discipline): created at
        // the first post frame only, outside command recording — the
        // postless path pays nothing and stays byte-identical. The
        // family pipelines that depend on LAZY states (the shadowed
        // and sky HDR variants) are ensured in the same order the
        // frame flow ensures their states (shadows, then sky, then
        // post — so the layouts exist by the time the variant
        // pipelines bind them).
        if post {
            self.ensure_post_resources(ctx, swapchain.format, swapchain.depth_format)?;
            if shadowed.is_some()
                && let (Some(post_state), Some(shadow_state)) = (&mut self.post, &self.shadows)
            {
                let layout = shadow_state.shadowed(slot).1;
                post_state.ensure_shadowed_hdr(ctx, layout)?;
            }
            if sky.is_some()
                && let (Some(post_state), Some(sky_state)) = (&mut self.post, &self.sky)
            {
                let layout = sky_state.set_and_layout().1;
                post_state.ensure_sky_hdr(ctx, layout)?;
            }
        }

        // The Era-1 lazy wet-reflection state (the same discipline): a
        // wet renderer always carries the post state (the reflection
        // pass rides its HDR render pass object) — ensured ABOVE the
        // post chain's own ensure so the pass exists; the chain itself
        // stays optional.
        if wet.is_some() {
            self.ensure_post_resources(ctx, swapchain.format, swapchain.depth_format)?;
            self.ensure_reflection_resources(ctx)?;
        }

        Ok(())
    }

    /// Builds the render pass + pipelines for a (possibly new) swapchain
    /// format into `out`. Handles are assigned as they are created
    /// (corrective finding G): a mid-build failure leaves them owned by
    /// the caller's partial state, whose `destroy` releases them.
    fn create_pipelines(
        ctx: &VulkanContext,
        color_format: vk::Format,
        depth_format: vk::Format,
        out: &mut PipelineState,
    ) -> Result<(), RendererError> {
        // Shader modules are consumed by the pipelines; tracked so they
        // are destroyed exactly once whether the build succeeds or fails.
        let mut modules: Vec<vk::ShaderModule> = Vec::new();
        let result = Self::assemble_pipelines(ctx, color_format, depth_format, out, &mut modules);
        unsafe {
            for module in modules.drain(..) {
                ctx.device.destroy_shader_module(module, None);
            }
        }
        result
    }

    fn assemble_pipelines(
        ctx: &VulkanContext,
        color_format: vk::Format,
        depth_format: vk::Format,
        out: &mut PipelineState,
        modules: &mut Vec<vk::ShaderModule>,
    ) -> Result<(), RendererError> {
        let color_attachment = vk::AttachmentDescription::default()
            .format(color_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);
        // The depth attachment's stencil ops are DONT_CARE for the
        // same reason post_draw.rs and shadow_draw.rs state
        // (corrective finding 4): no pipeline in this renderer
        // performs stencil operations — the attachment is depth
        // testing only. Explicit rather than defaulted: ash's
        // AttachmentDescription::default() zero-fills, and
        // stencil_load_op = 0 is LOAD, which the validation layer
        // rejects for a stencil-capable format (the D24 fallback)
        // with an UNDEFINED initialLayout — VUID-VkAttachmentDescription-
        // format-06700 (the forced-D24 post-lifecycle gate caught
        // exactly this: the pass was silent on D32 only because
        // D32_SFLOAT has no stencil aspect).
        let depth_attachment = vk::AttachmentDescription::default()
            .format(depth_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let attachments = [color_attachment, depth_attachment];
        let color_reference = vk::AttachmentReference {
            attachment: 0,
            layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        };
        let depth_reference = vk::AttachmentReference {
            attachment: 1,
            layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
        };
        let color_references = [color_reference];
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_references)
            .depth_stencil_attachment(&depth_reference);
        let dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .src_access_mask(vk::AccessFlags::empty())
            .dst_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            );
        let subpasses = [subpass];
        let dependencies = [dependency];
        let render_pass_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        out.render_pass = unsafe { ctx.device.create_render_pass(&render_pass_info, None) }
            .map_err(|error| RendererError::vk(error, "create_render_pass"))?;

        let scene_vert = include_bytes!(concat!(env!("OUT_DIR"), "/scene.vert.spv"));
        let scene_frag = include_bytes!(concat!(env!("OUT_DIR"), "/scene.frag.spv"));
        let atlas_spv = include_bytes!(concat!(env!("OUT_DIR"), "/overlay_atlas.spv"));

        let scene_vert_module = create_shader_module(ctx, scene_vert)?;
        modules.push(scene_vert_module);
        let scene_frag_module = create_shader_module(ctx, scene_frag)?;
        modules.push(scene_frag_module);
        let overlay_vert_module = create_shader_module(ctx, atlas_spv)?;
        modules.push(overlay_vert_module);
        let overlay_frag_module = create_shader_module(ctx, atlas_spv)?;
        modules.push(overlay_frag_module);

        let push_range = vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(64);
        let push_ranges = [push_range];
        let scene_layout_info =
            vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&push_ranges);
        out.scene_layout = unsafe { ctx.device.create_pipeline_layout(&scene_layout_info, None) }
            .map_err(|error| RendererError::vk(error, "scene layout"))?;
        let layouts = [ctx.descriptors.layout];
        let overlay_layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts);
        out.overlay_layout = unsafe {
            ctx.device
                .create_pipeline_layout(&overlay_layout_info, None)
        }
        .map_err(|error| RendererError::vk(error, "overlay layout"))?;

        let scene_binding = vk::VertexInputBindingDescription {
            binding: 0,
            stride: std::mem::size_of::<SceneVertex>() as u32,
            input_rate: vk::VertexInputRate::VERTEX,
        };
        let scene_attributes = [
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 0,
                format: vk::Format::R32G32B32_SFLOAT,
                offset: 0,
            },
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 1,
                format: vk::Format::R32G32B32_SFLOAT,
                offset: 12,
            },
        ];
        let overlay_binding = vk::VertexInputBindingDescription {
            binding: 0,
            stride: std::mem::size_of::<OverlayVertex>() as u32,
            input_rate: vk::VertexInputRate::VERTEX,
        };
        let overlay_attributes = [
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 0,
                format: vk::Format::R32G32_SFLOAT,
                offset: 0,
            },
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 1,
                format: vk::Format::R32G32B32A32_SFLOAT,
                offset: 8,
            },
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 2,
                format: vk::Format::R32G32_SFLOAT,
                offset: std::mem::offset_of!(OverlayVertex, uv) as u32,
            },
        ];

        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        // The orientation corrective (A5 — the winding audit): with
        // `perspective()` no longer negating Y, exactly ONE Y flip
        // remains on every geometry chain — the shader-boundary
        // conversion — so every triangle's screen-space winding is
        // REVERSED relative to the old double-flip chain, and BOTH
        // pipelines treat CLOCKWISE framebuffer winding as
        // front-facing. The overlay's glyph quads face the SAME
        // direction as the scene geometry (both authored for the
        // y-up, pre-flip convention): under the old shared
        // COUNTER_CLOCKWISE state they were BACK-facing and CULLED —
        // measured on the software ICD (0 glyph pixels with
        // COUNTER_CLOCKWISE, correct top-left glyphs with the facing
        // corrected) — no earlier test ever asserted overlay pixels,
        // so the glyphs had never actually rendered. Both pipelines
        // still cull BACK faces.
        let scene_rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::BACK)
            .front_face(vk::FrontFace::CLOCKWISE)
            .line_width(1.0);
        let overlay_rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::BACK)
            .front_face(vk::FrontFace::CLOCKWISE)
            .line_width(1.0);
        let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state =
            vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let color_blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA);
        let color_blend_attachments = [color_blend_attachment];
        let color_blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&color_blend_attachments);

        let scene_depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(true)
            .depth_compare_op(vk::CompareOp::LESS);
        let scene_bindings = [scene_binding];
        let scene_vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&scene_bindings)
            .vertex_attribute_descriptions(&scene_attributes);
        let scene_input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let scene_stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(scene_vert_module)
                .name(ENTRY_MAIN),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(scene_frag_module)
                .name(ENTRY_MAIN),
        ];
        let scene_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&scene_stages)
            .vertex_input_state(&scene_vertex_input)
            .input_assembly_state(&scene_input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&scene_rasterizer)
            .multisample_state(&multisampling)
            .depth_stencil_state(&scene_depth)
            .color_blend_state(&color_blend)
            .dynamic_state(&dynamic_state)
            .layout(out.scene_layout)
            .render_pass(out.render_pass)
            .subpass(0);
        out.scene_pipeline = unsafe {
            ctx.device
                .create_graphics_pipelines(ctx.pipeline_cache, &[scene_info], None)
        }
        .map_err(|(_, error)| RendererError::vk(error, "scene pipeline"))?[0];

        if let Some(corner) = &ctx.corner {
            use crate::lit_mesh::LitVertex;
            let module = create_shader_module(
                ctx,
                include_bytes!(concat!(env!("OUT_DIR"), "/lit_scene.spv")),
            )?;
            modules.push(module);
            let layouts = [corner.layout];
            // SAFETY: corner owns its descriptor layout across pipeline replacements;
            // the partial PipelineState owns the pipeline layout immediately.
            out.lit_layout = unsafe {
                ctx.device.create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts),
                    None,
                )
            }
            .map_err(|e| RendererError::vk(e, "lit layout"))?;
            let stages = [
                scene_stages[0].module(module).name(c"vs_main"),
                scene_stages[1].module(module).name(c"fs_main"),
            ];
            let bindings = [vk::VertexInputBindingDescription {
                stride: size_of::<LitVertex>() as u32,
                ..scene_binding
            }];
            let attributes = [
                scene_attributes[0],
                vk::VertexInputAttributeDescription {
                    offset: std::mem::offset_of!(LitVertex, normal) as u32,
                    ..scene_attributes[1]
                },
                vk::VertexInputAttributeDescription {
                    location: 2,
                    binding: 0,
                    format: vk::Format::R32G32_SFLOAT,
                    offset: std::mem::offset_of!(LitVertex, uv) as u32,
                },
            ];
            let inputs = vk::PipelineVertexInputStateCreateInfo::default()
                .vertex_binding_descriptions(&bindings)
                .vertex_attribute_descriptions(&attributes);
            // The corner mesh winds CCW about its outward normals.
            let raster = scene_rasterizer.front_face(vk::FrontFace::COUNTER_CLOCKWISE);
            let info = scene_info
                .stages(&stages)
                .vertex_input_state(&inputs)
                .rasterization_state(&raster)
                .layout(out.lit_layout);
            // SAFETY: same render pass, depth and viewport contract as the world pipeline.
            out.lit_pipeline = unsafe {
                ctx.device
                    .create_graphics_pipelines(ctx.pipeline_cache, &[info], None)
            }
            .map_err(|(_, e)| RendererError::vk(e, "lit pipeline"))?[0];

            // The Cycle 6-E instanced-proxy pipeline (see
            // docs/CYCLE6-6E-BRIEF.md): the SAME render pass,
            // descriptor layout, rasterizer (the proxy meshes wind
            // CCW about their outward normals — the lit convention),
            // and depth contract as the lit pipeline. The vertex
            // input carries TWO bindings: binding 0 the proxy mesh at
            // VERTEX rate (the lit 32-byte pos/normal/uv format,
            // locations 0-2), binding 1 the packed instance bytes at
            // INSTANCE rate (stride INSTANCE_STRIDE, locations 3-7
            // as R32G32B32A32_SFLOAT: the four column-major matrix
            // columns then the tint vec4 — the
            // proxy_instances::pack layout, byte-pinned by tests).
            let module = create_shader_module(
                ctx,
                include_bytes!(concat!(env!("OUT_DIR"), "/instanced_proxy.spv")),
            )?;
            modules.push(module);
            // SAFETY: corner owns its descriptor layout across pipeline
            // replacements; the partial PipelineState owns the pipeline
            // layout immediately.
            out.proxy_layout = unsafe {
                ctx.device.create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts),
                    None,
                )
            }
            .map_err(|e| RendererError::vk(e, "proxy layout"))?;
            let proxy_stages = [
                scene_stages[0].module(module).name(c"vs_main"),
                scene_stages[1].module(module).name(c"fs_main"),
            ];
            let mesh_binding = vk::VertexInputBindingDescription {
                binding: 0,
                stride: std::mem::size_of::<crate::proxy_mesh::ProxyVertex>() as u32,
                input_rate: vk::VertexInputRate::VERTEX,
            };
            let instance_binding = vk::VertexInputBindingDescription {
                binding: 1,
                stride: crate::proxy_instances::INSTANCE_STRIDE as u32,
                input_rate: vk::VertexInputRate::INSTANCE,
            };
            let proxy_bindings = [mesh_binding, instance_binding];
            let vec4 = vk::Format::R32G32B32A32_SFLOAT;
            let proxy_attributes = [
                scene_attributes[0],
                scene_attributes[1],
                vk::VertexInputAttributeDescription {
                    location: 2,
                    binding: 0,
                    format: vk::Format::R32G32_SFLOAT,
                    offset: std::mem::offset_of!(crate::proxy_mesh::ProxyVertex, uv) as u32,
                },
                vk::VertexInputAttributeDescription {
                    location: 3,
                    binding: 1,
                    format: vec4,
                    offset: 0,
                },
                vk::VertexInputAttributeDescription {
                    location: 4,
                    binding: 1,
                    format: vec4,
                    offset: 16,
                },
                vk::VertexInputAttributeDescription {
                    location: 5,
                    binding: 1,
                    format: vec4,
                    offset: 32,
                },
                vk::VertexInputAttributeDescription {
                    location: 6,
                    binding: 1,
                    format: vec4,
                    offset: 48,
                },
                vk::VertexInputAttributeDescription {
                    location: 7,
                    binding: 1,
                    format: vec4,
                    offset: 64,
                },
            ];
            let proxy_inputs = vk::PipelineVertexInputStateCreateInfo::default()
                .vertex_binding_descriptions(&proxy_bindings)
                .vertex_attribute_descriptions(&proxy_attributes);
            let proxy_info = vk::GraphicsPipelineCreateInfo::default()
                .stages(&proxy_stages)
                .vertex_input_state(&proxy_inputs)
                .input_assembly_state(&scene_input_assembly)
                .viewport_state(&viewport_state)
                .rasterization_state(&raster)
                .multisample_state(&multisampling)
                .depth_stencil_state(&scene_depth)
                .color_blend_state(&color_blend)
                .dynamic_state(&dynamic_state)
                .layout(out.proxy_layout)
                .render_pass(out.render_pass)
                .subpass(0);
            // SAFETY: same render pass, depth and viewport contract as the lit pipeline.
            out.proxy_pipeline = unsafe {
                ctx.device
                    .create_graphics_pipelines(ctx.pipeline_cache, &[proxy_info], None)
            }
            .map_err(|(_, e)| RendererError::vk(e, "proxy pipeline"))?[0];
        }

        let overlay_blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD);
        let overlay_blend_attachments = [overlay_blend_attachment];
        let overlay_blend = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(&overlay_blend_attachments);
        let overlay_no_depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(false)
            .depth_write_enable(false);
        let overlay_bindings = [overlay_binding];
        let overlay_vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&overlay_bindings)
            .vertex_attribute_descriptions(&overlay_attributes);
        let overlay_input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let overlay_stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(overlay_vert_module)
                .name(c"vs_main"),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(overlay_frag_module)
                .name(c"fs_main"),
        ];
        let overlay_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&overlay_stages)
            .vertex_input_state(&overlay_vertex_input)
            .input_assembly_state(&overlay_input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&overlay_rasterizer)
            .multisample_state(&multisampling)
            .depth_stencil_state(&overlay_no_depth)
            .color_blend_state(&overlay_blend)
            .dynamic_state(&dynamic_state)
            .layout(out.overlay_layout)
            .render_pass(out.render_pass)
            .subpass(0);
        out.overlay_pipeline = unsafe {
            ctx.device
                .create_graphics_pipelines(ctx.pipeline_cache, &[overlay_info], None)
        }
        .map_err(|(_, error)| RendererError::vk(error, "overlay pipeline"))?[0];

        Ok(())
    }

    pub(crate) fn destroy(&mut self, ctx: &VulkanContext) {
        unsafe {
            if let Some(proxy) = self.proxy.take() {
                proxy.destroy(ctx);
            }
            if let Some(shadows) = self.shadows.take() {
                shadows.destroy(ctx);
            }
            if let Some(sky) = self.sky.take() {
                sky.destroy(ctx);
            }
            if let Some(post) = self.post.take() {
                post.destroy(ctx);
            }
            if let Some(reflection) = self.reflection.take() {
                reflection.destroy(ctx);
            }
            if !self.region_mapped.is_null() {
                ctx.device.unmap_memory(self.region_vbo_memory);
                self.region_mapped = core::ptr::null_mut();
            }
            ctx.device.destroy_buffer(self.region_vbo, None);
            ctx.device.free_memory(self.region_vbo_memory, None);
            ctx.device.destroy_buffer(self.scene_ibo, None);
            ctx.device.free_memory(self.scene_ibo_memory, None);
            ctx.device.destroy_buffer(self.scene_vbo, None);
            ctx.device.free_memory(self.scene_vbo_memory, None);
            ctx.device.destroy_pipeline(self.lit_pipeline, None);
            ctx.device.destroy_pipeline_layout(self.lit_layout, None);
            ctx.device.destroy_pipeline(self.proxy_pipeline, None);
            ctx.device.destroy_pipeline_layout(self.proxy_layout, None);
            ctx.device.destroy_pipeline(self.overlay_pipeline, None);
            ctx.device
                .destroy_pipeline_layout(self.overlay_layout, None);
            ctx.device.destroy_pipeline(self.scene_pipeline, None);
            ctx.device.destroy_pipeline_layout(self.scene_layout, None);
            ctx.device.destroy_render_pass(self.render_pass, None);
        }
    }
}

fn create_shader_module(
    ctx: &VulkanContext,
    spirv: &[u8],
) -> Result<vk::ShaderModule, RendererError> {
    let words: Vec<u32> = spirv
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| u32::from_le_bytes(*word))
        .collect();
    let info = vk::ShaderModuleCreateInfo::default().code(&words);
    unsafe { ctx.device.create_shader_module(&info, None) }
        .map_err(|error| RendererError::vk(error, "create_shader_module"))
}

/// Records an init-time transfer on the graphics queue. The staging resources
/// may be released or reused only after the fence signals.
fn with_upload_commands(
    ctx: &VulkanContext,
    record: impl FnOnce(vk::CommandBuffer),
) -> Result<(), RendererError> {
    let mut pool = vk::CommandPool::null();
    let mut fence = vk::Fence::null();
    let result = (|| {
        let pool_info =
            vk::CommandPoolCreateInfo::default().queue_family_index(ctx.graphics_family);
        pool = unsafe { ctx.device.create_command_pool(&pool_info, None) }
            .map_err(|error| RendererError::vk(error, "staging command pool"))?;
        let command_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let command = unsafe { ctx.device.allocate_command_buffers(&command_info) }
            .map_err(|error| RendererError::vk(error, "staging command buffer"))?[0];
        unsafe {
            ctx.device
                .begin_command_buffer(
                    command,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|error| RendererError::vk(error, "begin staging"))?;
            record(command);
            ctx.device
                .end_command_buffer(command)
                .map_err(|error| RendererError::vk(error, "end staging"))?;
            fence = ctx
                .device
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .map_err(|error| RendererError::vk(error, "staging fence"))?;
            let staging_commands = [command];
            let submit_info = vk::SubmitInfo::default().command_buffers(&staging_commands);
            ctx.device
                .queue_submit(ctx.graphics_queue, &[submit_info], fence)
                .map_err(|error| RendererError::vk(error, "staging submit"))?;
            ctx.device
                .wait_for_fences(&[fence], true, u64::MAX)
                .map_err(|error| RendererError::vk(error, "staging wait"))?;
        }
        Ok(())
    })();
    // SAFETY: a successful submit was waited above (or the device was lost).
    // Destroying the pool also releases its single primary command buffer.
    unsafe {
        ctx.device.destroy_fence(fence, None);
        ctx.device.destroy_command_pool(pool, None);
    }
    result
}

fn create_device_local_buffer_with_data(
    ctx: &VulkanContext,
    usage: vk::BufferUsageFlags,
    data: &[u8],
) -> Result<(vk::Buffer, vk::DeviceMemory), RendererError> {
    let size = data.len() as vk::DeviceSize;
    // Partial-initialization cleanup (corrective finding G): tracked
    // handles are released when any step fails; the success path inside
    // the closure releases the one-shot upload resources itself and
    // nulls them so "non-null ⇒ needs release" always holds.
    let mut buffer = vk::Buffer::null();
    let mut memory = vk::DeviceMemory::null();
    let mut staging = vk::Buffer::null();
    let mut staging_memory = vk::DeviceMemory::null();
    let result = (|| -> Result<(), RendererError> {
        let buffer_info = vk::BufferCreateInfo::default()
            .size(size)
            // The staging copy's destination must allow transfers
            // (VUID-vkCmdCopyBuffer-dstBuffer-00120 — surfaced by the
            // CI validation-enabled smoke, invisible to the layer-less
            // pod kit).
            .usage(usage | vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        buffer = unsafe { ctx.device.create_buffer(&buffer_info, None) }
            .map_err(|error| RendererError::vk(error, "create_buffer"))?;
        let requirements = unsafe { ctx.device.get_buffer_memory_requirements(buffer) };
        let memory_index = find_memory_type(
            ctx,
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .ok_or_else(|| RendererError::NoDevice("no DEVICE_LOCAL memory".into()))?;
        let memory_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_index);
        memory = unsafe { ctx.allocate_memory(&memory_info) }
            .map_err(|error| RendererError::vk(error, "allocate_memory"))?;
        unsafe { ctx.device.bind_buffer_memory(buffer, memory, 0) }
            .map_err(|error| RendererError::vk(error, "bind_buffer_memory"))?;

        // Staging upload: HOST_VISIBLE staging buffer → copy command → fence wait.
        let staging_info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        staging = unsafe { ctx.device.create_buffer(&staging_info, None) }
            .map_err(|error| RendererError::vk(error, "create staging"))?;
        let staging_requirements = unsafe { ctx.device.get_buffer_memory_requirements(staging) };
        let staging_index = find_memory_type(
            ctx,
            staging_requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or_else(|| RendererError::NoDevice("no HOST_VISIBLE memory".into()))?;
        let staging_memory_info = vk::MemoryAllocateInfo::default()
            .allocation_size(staging_requirements.size)
            .memory_type_index(staging_index);
        staging_memory = unsafe { ctx.allocate_memory(&staging_memory_info) }
            .map_err(|error| RendererError::vk(error, "allocate staging memory"))?;
        unsafe { ctx.device.bind_buffer_memory(staging, staging_memory, 0) }
            .map_err(|error| RendererError::vk(error, "bind staging memory"))?;
        unsafe {
            let mapped = ctx
                .device
                .map_memory(staging_memory, 0, size, vk::MemoryMapFlags::empty())
                .map_err(|error| RendererError::vk(error, "map staging"))?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), mapped as *mut u8, data.len());
            ctx.device.unmap_memory(staging_memory);
        }

        with_upload_commands(ctx, |command| unsafe {
            ctx.device.cmd_copy_buffer(
                command,
                staging,
                buffer,
                &[vk::BufferCopy {
                    src_offset: 0,
                    dst_offset: 0,
                    size,
                }],
            );
        })?;
        unsafe {
            ctx.device.destroy_buffer(staging, None);
            staging = vk::Buffer::null();
            ctx.device.free_memory(staging_memory, None);
            staging_memory = vk::DeviceMemory::null();
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok((buffer, memory)),
        Err(error) => {
            unsafe {
                if staging != vk::Buffer::null() {
                    ctx.device.destroy_buffer(staging, None);
                }
                if staging_memory != vk::DeviceMemory::null() {
                    ctx.device.free_memory(staging_memory, None);
                }
                if buffer != vk::Buffer::null() {
                    ctx.device.destroy_buffer(buffer, None);
                }
                if memory != vk::DeviceMemory::null() {
                    ctx.device.free_memory(memory, None);
                }
            }
            Err(error)
        }
    }
}

fn create_mapped_buffer(
    ctx: &VulkanContext,
    usage: vk::BufferUsageFlags,
    size: vk::DeviceSize,
) -> Result<(vk::Buffer, vk::DeviceMemory, *mut core::ffi::c_void), RendererError> {
    // Partial-initialization cleanup (corrective finding G).
    let mut buffer = vk::Buffer::null();
    let mut memory = vk::DeviceMemory::null();
    let result = (|| -> Result<*mut core::ffi::c_void, RendererError> {
        let buffer_info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        buffer = unsafe { ctx.device.create_buffer(&buffer_info, None) }
            .map_err(|error| RendererError::vk(error, "create overlay buffer"))?;
        let requirements = unsafe { ctx.device.get_buffer_memory_requirements(buffer) };
        let memory_index = find_memory_type(
            ctx,
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or_else(|| RendererError::NoDevice("no HOST_VISIBLE memory".into()))?;
        let memory_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_index);
        memory = unsafe { ctx.allocate_memory(&memory_info) }
            .map_err(|error| RendererError::vk(error, "allocate overlay memory"))?;
        unsafe { ctx.device.bind_buffer_memory(buffer, memory, 0) }
            .map_err(|error| RendererError::vk(error, "bind overlay memory"))?;
        let mapped = unsafe {
            ctx.device
                .map_memory(memory, 0, size, vk::MemoryMapFlags::empty())
                .map_err(|error| RendererError::vk(error, "map overlay"))?
        };
        Ok(mapped)
    })();
    match result {
        Ok(mapped) => Ok((buffer, memory, mapped)),
        Err(error) => {
            unsafe {
                if buffer != vk::Buffer::null() {
                    ctx.device.destroy_buffer(buffer, None);
                }
                if memory != vk::DeviceMemory::null() {
                    ctx.device.free_memory(memory, None);
                }
            }
            Err(error)
        }
    }
}

/// The capture API's supported swapchain formats (the
/// post-orientation hardening's format contract): the 4-byte-per-
/// pixel RGBA8/BGRA8 family — UNORM and sRGB variants share the byte
/// footprint and channel order semantics. Returns the per-pixel byte
/// footprint, or `None` for every format outside the family: a
/// fallback surface format such as `R16G16B16A16_SFLOAT` (8 bytes per
/// pixel) must be REFUSED before any copy command is recorded, never
/// under-allocated and overwritten. The ordinary renderer's
/// surface-format fallback behavior is untouched — only capture
/// refuses.
pub(crate) fn capture_format_footprint(format: vk::Format) -> Option<usize> {
    match format {
        vk::Format::B8G8R8A8_UNORM
        | vk::Format::B8G8R8A8_SRGB
        | vk::Format::R8G8B8A8_UNORM
        | vk::Format::R8G8B8A8_SRGB => Some(4),
        _ => None,
    }
}

/// The capture destination's byte length, via checked arithmetic —
/// `None` when the width × height × footprint product overflows
/// usize (the typed [`RendererError::CaptureSizingOverflow`] refusal).
pub(crate) fn capture_byte_len(width: u32, height: u32, footprint: usize) -> Option<usize> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(footprint))
}

/// One captured frame's readback (the bounded capture path — the
/// orientation corrective's visual-evidence route).
///
/// The bytes are the swapchain image's texels as presented: tightly
/// packed rows, TOP row first (the framebuffer origin), one pixel per
/// 4 bytes in the swapchain's channel order (`format`). The capture
/// is taken AFTER the frame's render pass and BEFORE presentation —
/// exactly the image the display receives.
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// Tightly packed RGBA8-family texel rows, top row first.
    pub bytes: Vec<u8>,
    /// The swapchain format the bytes were copied in.
    pub format: CaptureFormat,
}

/// The capture's channel order (the swapchain's picked format).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureFormat {
    /// Byte order B, G, R, A — the renderer's preferred
    /// `B8G8R8A8_UNORM` surface.
    Bgra8,
    /// Byte order R, G, B, A.
    Rgba8,
    /// Any other format: the raw bytes are returned, but
    /// [`CapturedFrame::pixel_rgba`] refuses to interpret them.
    Other,
}

impl CapturedFrame {
    /// The RGBA tuple at pixel `(x, y)` — row 0 is the TOP row, x from
    /// the left (the framebuffer convention; the projection-orientation
    /// regressions assert against this). Panics for `Other` formats.
    pub fn pixel_rgba(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(
            x < self.width && y < self.height,
            "pixel ({x}, {y}) outside the {width}×{height} capture",
            width = self.width,
            height = self.height
        );
        let index = (y * self.width + x) as usize * 4;
        match self.format {
            CaptureFormat::Bgra8 => [
                self.bytes[index + 2],
                self.bytes[index + 1],
                self.bytes[index],
                self.bytes[index + 3],
            ],
            CaptureFormat::Rgba8 => [
                self.bytes[index],
                self.bytes[index + 1],
                self.bytes[index + 2],
                self.bytes[index + 3],
            ],
            CaptureFormat::Other => {
                panic!(
                    "the captured format is not an RGBA8-family format — refusing to guess the channel order"
                )
            }
        }
    }
}

/// The lazily-allocated frame-capture readback state (the orientation
/// corrective): a persistently mapped host-visible destination buffer
/// plus the one-shot request handshake. Allocated on first request and
/// reallocated when the swapchain extent changes; a capture records a
/// layout barrier pair and a CopyImageToBuffer into the frame's own
/// command buffer, so the slot's submission fence proves completion.
pub(crate) struct CaptureState {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: *mut core::ffi::c_void,
    byte_len: usize,
    width: u32,
    height: u32,
    format: vk::Format,
    /// A capture was requested for the next recorded frame (survives
    /// deferred frames — retried until a frame actually records).
    pending: bool,
    /// A capture was recorded; `take_capture` waits the slot's fence.
    ready: bool,
    slot: usize,
    /// The most recent refusal or drop (None in the ordinary path) —
    /// set when a pending request is refused or dropped so the frame
    /// can render normally; observable through
    /// [`Renderer::last_capture_refusal`].
    last_refusal: Option<RendererError>,
}

impl CaptureState {
    fn dead() -> Self {
        Self {
            buffer: vk::Buffer::null(),
            memory: vk::DeviceMemory::null(),
            mapped: core::ptr::null_mut(),
            byte_len: 0,
            width: 0,
            height: 0,
            format: vk::Format::UNDEFINED,
            pending: false,
            ready: false,
            slot: 0,
            last_refusal: None,
        }
    }

    fn destroy(&mut self, ctx: &VulkanContext) {
        unsafe {
            if self.buffer != vk::Buffer::null() {
                if !self.mapped.is_null() {
                    ctx.device.unmap_memory(self.memory);
                }
                ctx.device.destroy_buffer(self.buffer, None);
                self.buffer = vk::Buffer::null();
            }
            if self.memory != vk::DeviceMemory::null() {
                ctx.device.free_memory(self.memory, None);
                self.memory = vk::DeviceMemory::null();
            }
            self.mapped = core::ptr::null_mut();
            self.ready = false;
        }
    }

    /// (Re)allocates for the current swapchain extent, OUTSIDE command
    /// recording (the same discipline as the lazy region buffer). A
    /// no-op when the existing buffer already matches. The FORMAT
    /// CONTRACT and the checked sizing run FIRST — pure refusals that
    /// touch no GPU state, so an unsupported surface format (e.g. an
    /// 8-byte-per-pixel fallback) is refused with a typed error before
    /// any allocation or copy command, and the byte length can never
    /// exceed what the format actually requires. The TRANSFER-SOURCE
    /// capability is validated INDEPENDENTLY of destination
    /// reusability (the cache-validation corrective's defect 2): a
    /// recreated swapchain without TRANSFER_SRC is refused even when
    /// the cached destination would otherwise be reused — and the
    /// refusal stays pure (no GPU state is touched). The cache reuse
    /// itself validates the FULL extent (the corrective's defect 1):
    /// width AND height, never just the byte count — 1280×720 and
    /// 960×960 share a pixel count but not a shape, and the reported
    /// capture dimensions must always correspond to the image
    /// actually copied.
    fn ensure(
        &mut self,
        ctx: &VulkanContext,
        swapchain: &SwapchainState,
    ) -> Result<(), RendererError> {
        let footprint = capture_format_footprint(swapchain.format).ok_or_else(|| {
            RendererError::CaptureUnsupportedFormat(format!("{:?}", swapchain.format))
        })?;
        let width = swapchain.extent.width;
        let height = swapchain.extent.height;
        let needed = capture_byte_len(width, height, footprint)
            .ok_or(RendererError::CaptureSizingOverflow)?;
        if !swapchain.transfer_src_supported {
            return Err(RendererError::NoDevice(
                "the surface's swapchain images do not support TRANSFER_SRC — frame \
                 capture unavailable"
                    .into(),
            ));
        }
        if self.buffer != vk::Buffer::null()
            && self.width == width
            && self.height == height
            && self.format == swapchain.format
        {
            return Ok(());
        }
        self.destroy(ctx);
        let (buffer, memory, mapped) = create_mapped_buffer(
            ctx,
            vk::BufferUsageFlags::TRANSFER_DST,
            needed as vk::DeviceSize,
        )?;
        self.buffer = buffer;
        self.memory = memory;
        self.mapped = mapped;
        self.byte_len = needed;
        self.width = width;
        self.height = height;
        self.format = swapchain.format;
        Ok(())
    }

    /// The record-time guard predicate (the cache-validation
    /// corrective): the prepared destination may serve the image
    /// being recorded ONLY when the FULL extent (width AND height —
    /// never just the byte count), the format, AND the current
    /// swapchain's TRANSFER-SOURCE capability all match — a
    /// mid-frame swapchain recreation may change the shape, the
    /// format, or the capability between the pre-acquire preparation
    /// and the recording.
    fn matches(&self, width: u32, height: u32, format: vk::Format, transfer_src: bool) -> bool {
        self.buffer != vk::Buffer::null()
            && transfer_src
            && self.width == width
            && self.height == height
            && self.format == format
    }
}

/// Requests that the NEXT recorded frame be copied back to the host.
/// ONE capture outstanding at a time (the bounded single-flight
/// lifecycle policy): refused with a typed error while an earlier
/// request is pending (not yet recorded) or ready-but-unconsumed — a
/// second copy could otherwise overwrite the earlier readback's
/// destination while it is still unconsumed.
pub(crate) fn request_capture(frames: &mut FrameState) -> Result<(), RendererError> {
    if frames.capture.pending || frames.capture.ready {
        return Err(RendererError::CaptureOutstanding);
    }
    frames.capture.last_refusal = None;
    frames.capture.pending = true;
    Ok(())
}

/// Requests the NEXT shadowed frame's shadow-map readback (the 6-F
/// known-answer path): the depth-only pass's map is copied to a host
/// buffer inside that frame's command buffer. Single-flight like the
/// color capture. The handshake lives in the frame state, so the
/// request survives pipeline replacements; the shadow STATE itself
/// must exist (the renderer's request path ensures it).
pub(crate) fn request_shadow_readback(frames: &mut FrameState) -> Result<(), RendererError> {
    if frames.shadow_readback.pending || frames.shadow_readback.ready {
        return Err(RendererError::CaptureOutstanding);
    }
    frames.shadow_readback.pending = true;
    Ok(())
}

/// Takes the completed shadow-map readback, if one is ready: waits
/// the recording slot's fence (the color capture's take discipline)
/// and returns the map's depths (row-major, row 0 first, one f32 per
/// texel — the D32_SFLOAT bits as written by the depth-only pass).
pub(crate) fn take_shadow_readback(
    ctx: &VulkanContext,
    pipes: &mut PipelineState,
    frames: &mut FrameState,
) -> Result<Option<Vec<f32>>, RendererError> {
    if !frames.shadow_readback.ready {
        return Ok(None);
    }
    let Some(shadows) = pipes.shadows.as_ref() else {
        // The state was replaced after the readback recorded: the
        // buffer holding the copy is gone — a typed drop (the color
        // capture's mismatch path), reported rather than silently
        // empty.
        frames.shadow_readback.ready = false;
        return Err(RendererError::CaptureDropped(
            "the shadow state was replaced before the readback was taken".into(),
        ));
    };
    let slot = frames.shadow_readback.slot;
    // SAFETY: the fence is the recording frame's own — the same
    // fence the next frame on that slot waits.
    let depths = unsafe { shadows.readback_depths(ctx, frames.in_flight[slot]) };
    frames.shadow_readback.ready = false;
    Ok(depths)
}

/// The most recent capture refusal or drop, if any — observable for
/// diagnosis; a refused capture never poisons frame synchronization.
pub(crate) fn last_capture_refusal(frames: &FrameState) -> Option<&RendererError> {
    frames.capture.last_refusal.as_ref()
}

/// Takes the completed capture: waits the recording slot's fence (the
/// same fence the next frame waits — a later submission on that slot
/// can only have started after this capture's fence signaled), copies
/// the mapped bytes out, and clears the handshake. `Ok(None)` when no
/// capture is ready.
pub(crate) fn take_capture(
    ctx: &VulkanContext,
    frames: &mut FrameState,
) -> Result<Option<CapturedFrame>, RendererError> {
    if !frames.capture.ready {
        return Ok(None);
    }
    let slot = frames.capture.slot;
    let (byte_len, width, height, captured_format) = {
        let capture = &frames.capture;
        (
            capture.byte_len,
            capture.width,
            capture.height,
            capture.format,
        )
    };
    unsafe {
        ctx.device
            .wait_for_fences(&[frames.in_flight[slot]], true, WAIT_TIMEOUT_NS)
            .map_err(|error| RendererError::vk(error, "wait_for_fences (capture)"))?;
    }
    let mut bytes = vec![0u8; byte_len];
    unsafe {
        std::ptr::copy_nonoverlapping(
            frames.capture.mapped as *const u8,
            bytes.as_mut_ptr(),
            byte_len,
        );
    }
    frames.capture.ready = false;
    let format = match captured_format {
        vk::Format::B8G8R8A8_UNORM | vk::Format::B8G8R8A8_SRGB => CaptureFormat::Bgra8,
        vk::Format::R8G8B8A8_UNORM | vk::Format::R8G8B8A8_SRGB => CaptureFormat::Rgba8,
        _ => CaptureFormat::Other,
    };
    Ok(Some(CapturedFrame {
        width,
        height,
        bytes,
        format,
    }))
}

/// A single frame slot owns this mapping. The slot fence must have
/// signaled before growing or writing it.
#[derive(Default)]
struct OverlaySlot {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: *mut core::ffi::c_void,
    capacity: u32,
}

impl OverlaySlot {
    fn capacity_for(current: u32, required: usize) -> u32 {
        let limit = crate::overlay::overlay_capacity_vertices() / FRAMES_IN_FLIGHT;
        let required = required.min(limit as usize) as u32;
        if required <= current {
            return current;
        }
        required.next_power_of_two().min(limit)
    }

    fn reserve(&mut self, ctx: &VulkanContext, required: usize) -> Result<(), RendererError> {
        let capacity = Self::capacity_for(self.capacity, required);
        if capacity != self.capacity {
            let bytes = u64::from(capacity) * size_of::<OverlayVertex>() as u64;
            let (buffer, memory, mapped) =
                create_mapped_buffer(ctx, vk::BufferUsageFlags::VERTEX_BUFFER, bytes)?;
            self.destroy(ctx);
            *self = Self {
                buffer,
                memory,
                mapped,
                capacity,
            };
        }
        Ok(())
    }

    fn destroy(&mut self, ctx: &VulkanContext) {
        if self.capacity > 0 {
            unsafe {
                ctx.device.unmap_memory(self.memory);
                ctx.device.destroy_buffer(self.buffer, None);
                ctx.device.free_memory(self.memory, None);
            }
            *self = Self::default();
        }
    }
}

/// The shadow-map readback's single-flight handshake (the color
/// capture's shape; lives in `FrameState` so a request survives
/// pipeline replacements — the shadow STATE is rebuilt, the request
/// is not).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShadowReadbackState {
    pending: bool,
    ready: bool,
    slot: usize,
}

/// Sync objects, command buffers, and the timestamp pool, per frames-in-
/// flight slot, plus per-swapchain-image state.
pub(crate) struct FrameState {
    overlay_slots: [OverlaySlot; FRAMES_IN_FLIGHT as usize],
    /// Created with `RESET_COMMAND_BUFFER` (corrective finding D): the
    /// per-slot command buffers are re-recorded every frame, and the
    /// per-slot fence proves a buffer is no longer pending when its
    /// re-record begins.
    pub command_pool: vk::CommandPool,
    pub command_buffers: Vec<vk::CommandBuffer>,
    pub image_available: Vec<vk::Semaphore>,
    /// Presentation-wait semaphores indexed by **swapchain image**
    /// (corrective finding C): the submit fence proves the graphics work
    /// finished, never that the presentation engine stopped waiting on
    /// the present semaphore — slot-indexed reuse could therefore signal
    /// a semaphore with an outstanding wait. Indexing by image ties reuse
    /// to re-acquisition of that image. Grown on demand; released only
    /// after `device_wait_idle`.
    pub present_semaphores: Vec<vk::Semaphore>,
    pub in_flight: Vec<vk::Fence>,
    pub images_in_flight: Vec<vk::Fence>,
    pub timestamp_pool: vk::QueryPool,
    /// False when the graphics queue family exposes no timestamp bits
    /// (corrective finding E).
    pub timestamps_supported: bool,
    /// Whether this slot's queries have resolved at least once (finding
    /// E): never block-read unwritten queries.
    pub timestamps_written: [bool; FRAMES_IN_FLIGHT as usize],
    /// A present reported suboptimal — recreate before the next frame.
    pub needs_recreate: bool,
    /// The 6-F shadow-map readback handshake (single-flight, like
    /// the color capture; survives pipeline replacements).
    pub(crate) shadow_readback: ShadowReadbackState,
    /// Hint-gated surface probing (perf batch 3): true while the
    /// surface's drawable is believed unavailable (the previous frame
    /// deferred), so minimized/transitional surfaces keep probing
    /// until valid while ordinary frames skip the per-frame driver
    /// round-trip entirely.
    pub surface_probe_pending: bool,
    pub frame_slot: usize,
    pub image_index: u32,
    /// The bounded frame-capture readback (lazily allocated on first
    /// request — the orientation corrective's evidence path).
    pub capture: CaptureState,
}

impl FrameState {
    pub(crate) fn new(
        ctx: &VulkanContext,
        swapchain_image_count: u32,
    ) -> Result<FrameState, RendererError> {
        // Partial-initialization cleanup (corrective finding G).
        let mut state = FrameState {
            overlay_slots: std::array::from_fn(|_| OverlaySlot::default()),
            command_pool: vk::CommandPool::null(),
            command_buffers: Vec::new(),
            image_available: Vec::new(),
            present_semaphores: Vec::new(),
            in_flight: Vec::new(),
            images_in_flight: vec![vk::Fence::null(); swapchain_image_count as usize],
            timestamp_pool: vk::QueryPool::null(),
            timestamps_supported: ctx.timestamp_valid_bits > 0,
            timestamps_written: [false; FRAMES_IN_FLIGHT as usize],
            needs_recreate: false,
            surface_probe_pending: false,
            shadow_readback: ShadowReadbackState::default(),
            frame_slot: 0,
            image_index: 0,
            capture: CaptureState::dead(),
        };
        if let Err(error) = state.build(ctx, swapchain_image_count) {
            state.destroy(ctx);
            return Err(error);
        }
        Ok(state)
    }

    fn build(
        &mut self,
        ctx: &VulkanContext,
        swapchain_image_count: u32,
    ) -> Result<(), RendererError> {
        let pool_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
            .queue_family_index(ctx.graphics_family);
        self.command_pool = unsafe { ctx.device.create_command_pool(&pool_info, None) }
            .map_err(|error| RendererError::vk(error, "create_command_pool"))?;
        let allocate_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(FRAMES_IN_FLIGHT);
        self.command_buffers = unsafe { ctx.device.allocate_command_buffers(&allocate_info) }
            .map_err(|error| RendererError::vk(error, "allocate_command_buffers"))?;

        for _ in 0..FRAMES_IN_FLIGHT {
            let semaphore = unsafe {
                ctx.device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
            }
            .map_err(|error| RendererError::vk(error, "create_semaphore"))?;
            self.image_available.push(semaphore);
            let fence = unsafe {
                ctx.device.create_fence(
                    &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                    None,
                )
            }
            .map_err(|error| RendererError::vk(error, "create_fence"))?;
            self.in_flight.push(fence);
        }
        self.sync_present_semaphores(ctx, swapchain_image_count)?;

        if self.timestamps_supported {
            let query_info = vk::QueryPoolCreateInfo::default()
                .query_type(vk::QueryType::TIMESTAMP)
                .query_count(FRAMES_IN_FLIGHT * 2);
            self.timestamp_pool = unsafe { ctx.device.create_query_pool(&query_info, None) }
                .map_err(|error| RendererError::vk(error, "create_query_pool"))?;
        }
        Ok(())
    }

    /// Grows the per-image presentation-wait semaphore table (corrective
    /// finding C). Entries are never removed — recreation idles the
    /// device first, so outstanding present waits have drained.
    fn sync_present_semaphores(
        &mut self,
        ctx: &VulkanContext,
        image_count: u32,
    ) -> Result<(), RendererError> {
        let needed = present_semaphore_len_for(self.present_semaphores.len(), image_count as usize);
        while self.present_semaphores.len() < needed {
            let semaphore = unsafe {
                ctx.device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
            }
            .map_err(|error| RendererError::vk(error, "create present semaphore"))?;
            self.present_semaphores.push(semaphore);
        }
        Ok(())
    }

    /// Returns the present-wait semaphore for the acquired image
    /// (corrective finding C), growing the table if the index exceeds it.
    fn present_semaphore_for(
        &mut self,
        ctx: &VulkanContext,
        image_index: u32,
    ) -> Result<vk::Semaphore, RendererError> {
        self.sync_present_semaphores(ctx, image_index + 1)?;
        Ok(self.present_semaphores[image_index as usize])
    }

    /// Re-syncs the per-image tables for a new swapchain (the image count
    /// may change across recreation).
    pub(crate) fn resize_images(
        &mut self,
        ctx: &VulkanContext,
        swapchain_image_count: u32,
    ) -> Result<(), RendererError> {
        self.images_in_flight = vec![vk::Fence::null(); swapchain_image_count as usize];
        self.sync_present_semaphores(ctx, swapchain_image_count)
    }

    pub(crate) fn destroy(&mut self, ctx: &VulkanContext) {
        unsafe {
            ctx.device.destroy_query_pool(self.timestamp_pool, None);
            for fence in &self.in_flight {
                ctx.device.destroy_fence(*fence, None);
            }
            self.in_flight.clear();
            for semaphore in &self.present_semaphores {
                ctx.device.destroy_semaphore(*semaphore, None);
            }
            self.present_semaphores.clear();
            for semaphore in &self.image_available {
                ctx.device.destroy_semaphore(*semaphore, None);
            }
            self.image_available.clear();
            self.capture.destroy(ctx);
            for overlay in &mut self.overlay_slots {
                overlay.destroy(ctx);
            }
            ctx.device.destroy_command_pool(self.command_pool, None);
        }
    }
}

#[derive(Clone, Copy)]
enum LayerPriority {
    First,
    Second,
}

/// Copies consecutive triangle batches into one fenced, persistently mapped
/// slot. Both copy lengths and draws consume the same F-03 clamp decision.
/// Returns the number of copied vertices from the first batch.
///
/// # Safety
/// `mapped` must be the live host-coherent mapping for the buffer of `T`
/// vertices, with at least `slot + 1` slots each holding `capacity` vertices.
/// The caller must own this slot (its preceding GPU submission has completed).
unsafe fn upload_layers<T: bytemuck::Pod>(
    mapped: *mut core::ffi::c_void,
    slot: usize,
    capacity: u32,
    first: &[T],
    second: &[T],
    priority: LayerPriority,
) -> (DynamicClamp, u32) {
    let clamp = dynamic_geometry_clamp(first.len().saturating_add(second.len()), capacity);
    let first_len = match priority {
        LayerPriority::First => first.len().min(clamp.drawn as usize),
        LayerPriority::Second => (clamp.drawn as usize).saturating_sub(second.len()),
    };
    let second_len = clamp.drawn as usize - first_len;
    unsafe {
        let dest = mapped.cast::<T>().add(slot * capacity as usize);
        std::ptr::copy_nonoverlapping(first.as_ptr(), dest, first_len);
        std::ptr::copy_nonoverlapping(second.as_ptr(), dest.add(first_len), second_len);
    }
    (clamp, first_len as u32)
}

/// Outcome of one `render_frame` call.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct FrameOutcome {
    pub presented: bool,
    pub recreated: bool,
    pub suboptimal: bool,
    /// The drawable was temporarily unavailable (zero-sized surface
    /// capabilities) — nothing was created, destroyed, or presented.
    pub deferred: bool,
    pub gpu_render_ms: Option<f64>,
    /// F-03 Step 1: the frame's region-scene and overlay capacity
    /// decisions (supplied / per-slot capacity / drawn / omitted). Both
    /// stay default — all zero — on deferred or minimized frames, which
    /// record nothing; the overlay capacity is the current slot's
    /// allocation, while the region capacity
    /// reads 0 until the lazy region buffer allocates. Nonzero omitted
    /// counts are REPORTED upstream, never fatal here.
    pub region: DynamicClamp,
    pub foreground_drawn: u32,
    pub overlay: DynamicClamp,
    /// The proxy draw calls recorded this frame (the 6-E headline:
    /// one per populated kind instanced; one per instance in the
    /// per-object reference). Zero on unpopulated and deferred
    /// frames. The corner mesh's own material draws are NOT counted
    /// here — this counts proxy submission only.
    pub proxy_draw_calls: u32,
    /// vkCmdDrawIndirect issuances THIS frame (the 6-E2 issuance
    /// observation — the delta of the context's monotonic counter
    /// across this frame's recording). One per populated kind in
    /// the indirect mode and ZERO in every other mode; zero on a
    /// deferred frame. Distinguishes the indirect submission from
    /// the direct one, which the draw-call count deliberately
    /// cannot (the release fallback draws the identical geometry
    /// through cmd_draw — this field is what observes the
    /// difference).
    pub indirect_draws_issued: u32,
}

/// The full frame flow: wait → acquire → (recreate if needed) → record →
/// submit → present. Called by the safe [`crate::Renderer`] wrapper.
// Private frame orchestration: the four owning states plus the per-frame
// inputs are exactly the parameters the record/submit/present sequence
// needs; bundling them would hide the ownership story for no gain.
// The Era-1 sky pass rides the same flat list (the one-Option-per-layer
// shape; the sky is a layer like the shadowed one, drawn first when
// present). The Era-1 post chain rides it as ONE bool (the chain has no
// per-frame data at all — its only uniform is extent-static), and the
// wet reflection as ONE Option (the camera description its mirrored
// math consumes; None keeps the dry path).
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_frame(
    ctx: &mut VulkanContext,
    swapchain: &mut SwapchainState,
    pipes: &mut PipelineState,
    frames: &mut FrameState,
    window_size: (u32, u32),
    present_preference: PresentModePreference,
    replace_pipelines: bool,
    demo_scene: DemoScenePreference,
    view_proj: &[[f32; 4]; 4],
    region_vertices: &[SceneVertex],
    foreground: Option<SceneLayer<'_>>,
    hud: &[OverlayVertex],
    debug_overlay: &[OverlayVertex],
    corner_params: Option<&corner::FrameParams>,
    proxy: Option<&proxy_draw::ProxyFrameUpload<'_>>,
    shadowed: Option<&shadow_draw::ShadowFrameParams>,
    sky: Option<&sky_draw::SkyFrameParams>,
    post: bool,
    wet: Option<&reflection_draw::WetCamera>,
) -> Result<FrameOutcome, RendererError> {
    let mut outcome = FrameOutcome::default();
    let slot = frames.frame_slot;
    // The 6-E2 issuance observation's frame baseline: the recording
    // below (this frame's command buffer) increments the context's
    // monotonic vkCmdDrawIndirect counter at each call site; the
    // delta lands on the outcome after the submission. Deferred and
    // refused frames record nothing, so their delta is zero.
    let indirect_issued_before = ctx.indirect_draws_issued.get();

    // The proxy materials are validated before ANY synchronization
    // state is touched (the capture-refusal discipline: a refusal
    // here can never leave a fence reset-but-unsignaled or an
    // acquired image's semaphore unconsumed). A populated kind whose
    // material index falls outside the corner table is a typed error;
    // an EMPTY kind may carry any index (it draws nothing).
    let proxy_sets = match (ctx.corner.as_ref(), proxy) {
        (Some(corner), Some(upload)) => {
            let npc_set = corner.material_set(upload.npc_material);
            let vehicle_set = corner.material_set(upload.vehicle_material);
            if (upload.npc_count > 0 && npc_set.is_none())
                || (upload.vehicle_count > 0 && vehicle_set.is_none())
            {
                return Err(RendererError::Vulkan(format!(
                    "proxy material index outside the corner material table \
                     (npc {}, vehicle {})",
                    upload.npc_material, upload.vehicle_material
                )));
            }
            Some([
                npc_set.unwrap_or(vk::DescriptorSet::null()),
                vehicle_set.unwrap_or(vk::DescriptorSet::null()),
            ])
        }
        // A proxy upload without corner resources is structurally
        // unreachable (the safe layer requires the Corner scene
        // preference, whose renderer construction built them) —
        // treated as the unpopulated frame rather than an error.
        (None, Some(_)) | (None, None) | (Some(_), None) => None,
    };

    // Minimized windows report a zero extent — present nothing.
    if window_size.0 == 0 || window_size.1 == 0 {
        frames.surface_probe_pending = true;
        outcome.deferred = true;
        return Ok(outcome);
    }

    // A minimized or transitioning surface can report zero-sized
    // capabilities while the application-side window size stays stale
    // and nonzero (Win32: currentExtent and min/max all 0) — the
    // drawable is temporarily unavailable. Defer the whole frame BEFORE
    // any recreation trigger: no swapchain is created, destroyed, or
    // retried and no error is raised; rendering resumes when the
    // surface reports a valid extent again (correction of the 1×1
    // fabrication defect, VUID-VkSwapchainCreateInfoKHR-pNext-07781,
    // found on the rig 2026-10-20).
    let size_changed = swapchain.requested_size != window_size;
    if size_changed || frames.surface_probe_pending || frames.needs_recreate {
        let extent = drawable_extent(ctx, window_size)?;
        if surface_probe_decision(
            frames.surface_probe_pending || size_changed,
            frames.needs_recreate,
            extent.is_some(),
        ) == SurfaceProbeDecision::ProbeDeferred
        {
            frames.surface_probe_pending = true;
            outcome.deferred = true;
            return Ok(outcome);
        }
        frames.surface_probe_pending = false;
        // A changed application request triggers one recreation even on a
        // surface with a fixed extent. The new swapchain records that request.
        frames.needs_recreate |= size_changed || extent != Some(swapchain.extent);
    }

    // Ensure all lazy frame state before any acquire or recreation
    // decision — the pre-acquire preparation discipline (finding 3's
    // stranded-frame corrective, and finding 6's replacement-variant
    // completeness). Each ensure_* is idempotent (an existence check),
    // so calling it twice (before and after recreation) is safe.
    let foreground_vertices = foreground.map_or(&[][..], |layer| layer.vertices);
    pipes.ensure_frame_targets_and_resources(
        ctx,
        swapchain,
        slot,
        proxy,
        shadowed,
        sky,
        post,
        wet,
        region_vertices,
        foreground_vertices,
    )?;

    // A previous present reported suboptimal — recreate before drawing
    // (verification requirement: suboptimal must eventually trigger
    // recreation).
    if consume_recreate(&mut frames.needs_recreate) {
        recreate_swapchain(
            ctx,
            swapchain,
            pipes,
            frames,
            window_size,
            present_preference,
            replace_pipelines,
            demo_scene,
        )?;
        outcome.recreated = true;
        // The generation that draws carries every active variant —
        // calling the helper here ensures sky, reflection, and HDR
        // variant pipelines exist on the new generation (finding 6).
        let foreground_vertices = foreground.map_or(&[][..], |layer| layer.vertices);
        pipes.ensure_frame_targets_and_resources(
            ctx,
            swapchain,
            slot,
            proxy,
            shadowed,
            sky,
            post,
            wet,
            region_vertices,
            foreground_vertices,
        )?;
    }

    // The capture readback's destination buffer, prepared BEFORE any
    // acquire or fence reset (the post-orientation hardening's
    // frame-sync rule): a refusal here touches no synchronization
    // state, so it can never leave a fence reset-but-unsignaled or an
    // acquired image's semaphore unconsumed. A refused capture
    // (unsupported format, no TRANSFER_SRC, sizing overflow) is NOT a
    // frame error — the request is dropped, recorded as the last
    // refusal, and the frame renders normally; a later request can
    // try again. Deferred frames keep the request alive for the next
    // frame that records.
    if frames.capture.pending
        && let Err(error) = frames.capture.ensure(ctx, swapchain)
    {
        frames.capture.pending = false;
        frames.capture.last_refusal = Some(error);
    }

    // The Era-1 post chain's and the wet reflection's extent-bound
    // targets: prepared HERE — BEFORE the slot-fence wait, the fence
    // reset, and the acquire (finding 3's stranded-frame corrective,
    // the capture-ensure discipline above): the swapchain's extent
    // and views are equally live at this point (only a recreation
    // can change them, and every recreation site precedes this
    // block), so a target-creation failure returns with every sync
    // object untouched — no reset-but-unsignaled fence, no signaled
    // acquire semaphore left unconsumed. A rebuild releases the old
    // bundle only when the extent or view set changed, which only a
    // recreation (already device-idle) can cause — the rebuild-safety
    // proof in post_draw's module header. The swapchain generation
    // replaces the view handle for cache invalidation (corrective
    // finding 7): Vulkan permits handle reuse, so a numerical view
    // handle could recur after destruction.
    if post && let Some(post_state) = &mut pipes.post {
        post_state.ensure_targets(
            ctx,
            swapchain.extent,
            &swapchain.views,
            swapchain.generation,
        )?;
    }
    if wet.is_some()
        && let (Some(post_state), Some(reflection_state), Some(corner)) =
            (&pipes.post, &mut pipes.reflection, ctx.corner.as_ref())
    {
        reflection_state.ensure_targets(
            ctx,
            swapchain.extent,
            corner,
            post_state.hdr_render_pass(),
            swapchain.generation,
            swapchain.depth_format,
        )?;
    }

    // Wait for this slot's previous submission. The fence is reset only
    // AFTER the per-image guard below: under present backpressure the
    // driver can hand a slot the image that slot last rendered, and
    // resetting first would leave that guard waiting on a fence only
    // THIS frame's submission will signal — the 240-pacing-against-FIFO
    // wedge (rig finding, 2026-10-20: "wait image fence timed out").
    unsafe {
        ctx.device
            .wait_for_fences(&[frames.in_flight[slot]], true, WAIT_TIMEOUT_NS)
            .map_err(|error| RendererError::vk(error, "wait_for_fences (slot fence timed out — a submission never completed; presenting faster than the present mode drains?)"))?;
    }

    // Allocation can fail. Grow only the fenced slot, before acquire, so
    // a failure leaves the image-available semaphore and fence reusable.
    frames.overlay_slots[slot].reserve(ctx, hud.len().saturating_add(debug_overlay.len()))?;

    // OUT_OF_DATE retries once. The fence stays signaled until an image
    // has actually been acquired, including when both attempts defer.
    // If the acquire-out-of-date plan is armed, inject the error at the
    // FIRST acquire site only (the retry acquire inside the out-of-date
    // arm is NEVER injected — the plan is consumed at the first site
    // only, so the retry remains structurally real). The injected result
    // models the driver contract exactly (ERROR_OUT_OF_DATE_KHR means no
    // image was acquired and the semaphore is untouched — precisely the
    // state the out-of-date arm below already handles; skipping the real
    // call is therefore the faithful simulation, not an approximation).
    let mut acquire_result = if ctx.take_acquire_out_of_date() {
        Err(vk::Result::ERROR_OUT_OF_DATE_KHR)
    } else {
        unsafe {
            ctx.swapchain_ext.acquire_next_image(
                swapchain.swapchain,
                100_000_000,
                frames.image_available[slot],
                vk::Fence::null(),
            )
        }
    };
    if let Err(vk::Result::ERROR_OUT_OF_DATE_KHR) = acquire_result {
        // The out-of-date inline recreation ALSO probes first (the
        // batch-3 review's directive: every recreation path checks
        // surface availability before recreating) — a mid-transition
        // surface must not be recreated into. Unavailable: PRESERVE
        // the already-signaled slot fence (vkWaitForFences does not
        // consume a signaled fence — it stays signaled until the next
        // submission's owner resets it, so the next frame's wait
        // passes and every images_in_flight reference stays valid;
        // destroying and recreating the fence here would leave
        // per-image references to a dead handle — the review's fence
        // finding), latch the deferral hint, PRESERVE the recreation
        // intent, and defer. No image was acquired, so no semaphore
        // waits.
        if surface_probe_decision(false, true, drawable_extent(ctx, window_size)?.is_some())
            == SurfaceProbeDecision::ProbeDeferred
        {
            mark_suboptimal(&mut frames.needs_recreate);
            frames.surface_probe_pending = true;
            outcome.deferred = true;
            return Ok(outcome);
        }
        recreate_swapchain(
            ctx,
            swapchain,
            pipes,
            frames,
            window_size,
            present_preference,
            replace_pipelines,
            demo_scene,
        )?;
        outcome.recreated = true;
        // The generation that draws carries every active variant —
        // re-running the state-ensure helper here creates sky,
        // reflection, and the HDR variant pipelines on the new
        // generation (finding 6: the replacement replicates only
        // proxy/indirect/shadow/post; without this call the first
        // frame after THIS recreation silently skips sky and wet
        // passes).
        pipes.ensure_frame_targets_and_resources(
            ctx,
            swapchain,
            slot,
            proxy,
            shadowed,
            sky,
            post,
            wet,
            region_vertices,
            foreground_vertices,
        )?;
        // The targets were ensured pre-acquire against the OLD
        // generation — repeat preparation against the replacement
        // before retrying the out-of-date acquire (finding 3's
        // stranded-frame corrective; the same frame-sync discipline,
        // still before any acquire of the new chain). The swapchain
        // generation replaces the view handle for cache invalidation
        // (corrective finding 7).
        if post && let Some(post_state) = &mut pipes.post {
            post_state.ensure_targets(
                ctx,
                swapchain.extent,
                &swapchain.views,
                swapchain.generation,
            )?;
        }
        if wet.is_some()
            && let (Some(post_state), Some(reflection_state), Some(corner)) =
                (&pipes.post, &mut pipes.reflection, ctx.corner.as_ref())
        {
            reflection_state.ensure_targets(
                ctx,
                swapchain.extent,
                corner,
                post_state.hdr_render_pass(),
                swapchain.generation,
                swapchain.depth_format,
            )?;
        }
        // The capture ensure is repeated here too (the same frame-sync
        // discipline: a failure here cannot strand a frame).
        if frames.capture.pending
            && let Err(error) = frames.capture.ensure(ctx, swapchain)
        {
            frames.capture.pending = false;
            frames.capture.last_refusal = Some(error);
        }
        acquire_result = unsafe {
            ctx.swapchain_ext.acquire_next_image(
                swapchain.swapchain,
                100_000_000,
                frames.image_available[slot],
                vk::Fence::null(),
            )
        };
    }
    let (image_index, suboptimal) = match acquire_result {
        Ok((index, suboptimal)) => (index, suboptimal),
        Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
            frames.needs_recreate = true;
            outcome.deferred = true;
            return Ok(outcome);
        }
        Err(vk::Result::TIMEOUT) => {
            return Err(RendererError::Vulkan(
                "acquire_next_image timed out: no swapchain image became available in 100 ms \
                 (present mode not draining — vsync-locked FIFO paced beyond the display rate?)"
                    .into(),
            ));
        }
        Err(error) => return Err(RendererError::vk(error, "acquire_next_image")),
    };
    outcome.suboptimal = suboptimal;
    frames.image_index = image_index;
    if let Some(image_fence) = mut_at(&mut frames.images_in_flight, image_index as usize) {
        if *image_fence != vk::Fence::null() {
            unsafe {
                ctx.device
                    .wait_for_fences(&[*image_fence], true, WAIT_TIMEOUT_NS)
                    .map_err(|error| {
                        RendererError::vk(
                            error,
                            "wait image fence (timed out — a submission never completed)",
                        )
                    })?;
            }
        }
        *image_fence = frames.in_flight[slot];
    }

    // Safe to reset only now: the image guard above is the last wait
    // that can reference this slot's previous signal.
    unsafe {
        ctx.device
            .reset_fences(&[frames.in_flight[slot]])
            .map_err(|error| RendererError::vk(error, "reset_fences"))?;
    }

    // Timestamps from this slot's previous use are resolved now (the
    // fence above proves it). The first use of a slot has no samples
    // yet, and unsupported timestamps are never queried (corrective
    // finding E).
    let gpu_render_ms =
        if timestamp_sample_ready(frames.timestamps_supported, frames.timestamps_written[slot]) {
            read_timestamps(ctx, frames, slot)
        } else {
            None
        };

    // Record.
    let command = frames.command_buffers[slot];
    let framebuffer = swapchain.framebuffers[image_index as usize];
    let extent = swapchain.extent;
    // The clear color (hoisted: the Era-1 post chain's HDR scene
    // pass clears the SAME color the main pass would — the value
    // is identical, only the attachment's format differs).
    let clear_color = corner_params.map_or_else(
        || shadowed.map_or([0.02, 0.02, 0.04, 1.0], |params| params.clear_color()),
        corner::FrameParams::clear_color,
    );
    let clear_values = [
        vk::ClearValue {
            color: vk::ClearColorValue {
                float32: clear_color,
            },
        },
        vk::ClearValue {
            depth_stencil: vk::ClearDepthStencilValue {
                depth: 1.0,
                stencil: 0,
            },
        },
    ];
    // Both scene layers fit in the existing mapped slot. Copy directly from
    // their retained application buffers without concatenating CPU geometry.
    let mut world_drawn = 0;
    if !region_vertices.is_empty() || !foreground_vertices.is_empty() {
        (outcome.region, world_drawn) = unsafe {
            upload_layers(
                pipes.region_mapped,
                slot,
                pipes.region_capacity,
                region_vertices,
                foreground_vertices,
                LayerPriority::Second,
            )
        };
        outcome.foreground_drawn = outcome.region.drawn - world_drawn;
    }
    let overlay = &frames.overlay_slots[slot];
    outcome.overlay.capacity = overlay.capacity;
    if !hud.is_empty() || !debug_overlay.is_empty() {
        (outcome.overlay, _) = unsafe {
            upload_layers(
                overlay.mapped,
                0,
                overlay.capacity,
                hud,
                debug_overlay,
                LayerPriority::First,
            )
        };
    }

    // The 6-E instance upload: this slot's region of the mapped
    // instance buffer, written after the slot's fence wait (the
    // region/overlay discipline — the previous submission through
    // this slot, which read this region, has completed). The bytes
    // are the safe layer's repacked population (≤ the region's
    // bound by the repack's validated capacity). The 6-E2 indirect
    // command block follows the identical discipline in the same
    // place: this slot's 32 command bytes (the pure proxy_indirect
    // packing of the pinned mesh vertex counts and this frame's
    // per-kind counts), written only in the indirect mode (the
    // state exists — the frame flow's ensure ran before the fence
    // wait, outside command recording).
    if let (Some(proxy_state), Some(upload)) = (&pipes.proxy, proxy) {
        unsafe { proxy_state.write_slot(slot, upload.bytes) };
        if let Some(indirect) = proxy_state.indirect.as_ref()
            && matches!(upload.mode, crate::ProxyDrawMode::Indirect)
        {
            unsafe { indirect.write_slot(slot, upload.npc_count, upload.vehicle_count) };
        }
    }

    // Disjoint window-depth intervals allow self-occlusion in both layers
    // using one depth attachment, without a clear, barrier or extra pass.
    // The gap at the boundary also accommodates fixed-point depth formats.
    // Even D16 gives a viewmodel 4096 depth levels. Leave one full
    // D16 quantum between intervals, including both clip-plane endpoints.
    const FOREGROUND_MAX_DEPTH: f32 = 1.0 / 16.0;
    let world_min_depth = if outcome.foreground_drawn > 0 {
        FOREGROUND_MAX_DEPTH + 1.0 / 65535.0
    } else {
        0.0
    };
    let viewport = vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: extent.width as f32,
        height: extent.height as f32,
        min_depth: world_min_depth,
        max_depth: 1.0,
    };
    let scissor = vk::Rect2D {
        extent,
        offset: vk::Offset2D { x: 0, y: 0 },
    };

    // The present-wait semaphore is owned by the acquired image
    // (corrective finding C).
    let present_semaphore = frames.present_semaphore_for(ctx, image_index)?;

    // The targets were ensured pre-acquire — finding 3's stranded-frame
    // corrective. This comment marks the original call sites (removed).

    unsafe {
        ctx.device
            .begin_command_buffer(
                command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(|error| RendererError::vk(error, "begin_command_buffer"))?;

        if frames.timestamps_supported {
            ctx.device
                .cmd_reset_query_pool(command, frames.timestamp_pool, (slot as u32) * 2, 2);
            ctx.device.cmd_write_timestamp(
                command,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                frames.timestamp_pool,
                (slot as u32) * 2,
            );
        }

        // The 6-F shadow depth pass — recorded BEFORE the main render
        // pass (the pass's final subpass dependency orders the map's
        // writes before the main pass's fragment reads). The extended
        // FrameParams + light matrix are written into this slot's
        // mapped region first (the fence wait above owns the slot).
        if let (Some(corner), Some(shadow_state), Some(params)) =
            (&ctx.corner, &pipes.shadows, shadowed)
        {
            shadow_state.write_frame(slot, params);
            shadow_state.record_depth_pass(ctx, command, corner, slot);
        }

        // The Era-1 wet reflection's mirrored-scene pass (package 7)
        // — recorded BEFORE the scene pass: the mirrored world
        // rendered into this slot's offscreen target (the world-
        // mirror construction, planar.rs's matrix), then the
        // barrier ordering its writes before the ground re-draw's
        // fragment reads inside the scene pass that follows. The
        // ground re-draw itself lands INSIDE whichever scene pass
        // runs (the post branch's or the main one's), after the
        // mesh draws.
        if let (Some(corner), Some(reflection_state), Some(post_state), Some(camera)) =
            (&ctx.corner, &pipes.reflection, &pipes.post, wet)
        {
            // SAFETY: this slot's fence wait passed above (the
            // mirrored params region is disjoint per slot); the
            // command buffer is recording, before the scene pass;
            // the targets were ensured against the live extent and
            // this same HDR pass object after the acquire.
            reflection_state.write_frame(slot, camera);
            reflection_state.record_reflection_pass(
                ctx,
                corner,
                command,
                post_state.hdr_render_pass(),
                slot,
                extent,
                clear_color,
            );
            reflection_state.record_read_barrier(ctx, command, slot);
        }

        // The Era-1 post chain (package 6): a post frame REPLACES
        // the main render pass with the HDR chain — the scene pass
        // into the offscreen R16G16B16A16 target, the bloom mips,
        // and the composite + tonemap into the acquired swapchain
        // image (post_draw's module header documents the whole
        // sequence and its synchronization). A postless frame takes
        // the main pass below, byte-identical to the pre-package
        // renderer (the post state may exist — the belt check
        // keeps a postless frame off the chain regardless).
        if let (Some(post_state), true) = (&pipes.post, post) {
            let inputs = post_draw::PostFrameInputs {
                slot,
                image_index,
                extent,
                clear_color,
                corner_params,
                proxy,
                proxy_sets,
                shadowed,
                sky,
                wet: wet.is_some(),
            };
            // SAFETY: the command buffer is recording past the
            // shadow depth pass; this slot's fence wait passed (the
            // uniform writes inside are disjoint per slot); the
            // targets were ensured against the live swapchain
            // after the acquire; `image_index` is the acquired
            // image.
            outcome.proxy_draw_calls = post_state.record(ctx, pipes, command, &inputs);
        } else {
            ctx.device.cmd_begin_render_pass(
                command,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(pipes.render_pass)
                    .framebuffer(framebuffer)
                    .render_area(scissor)
                    .clear_values(&clear_values),
                vk::SubpassContents::INLINE,
            );
            ctx.device.cmd_set_viewport(command, 0, &[viewport]);
            ctx.device.cmd_set_scissor(command, 0, &[scissor]);
            // The Era-1 sky draw: FIRST inside the main render pass —
            // the fullscreen triangle replaces the clear-color
            // background with the sky equation, with NO depth
            // interaction (no test, no write), so every scene draw below
            // behaves exactly as it did over the clear (the
            // unchanged-path proof: only the uncovered background
            // changes). The ensure above guaranteed the state whenever
            // `sky` is Some; the belt check keeps a skyless frame on the
            // skyless path even if a prior sky frame created the state.
            if let (Some(sky_state), Some(params)) = (&pipes.sky, sky) {
                // SAFETY (both calls): this slot's fence wait passed above
                // (the uniform region is disjoint from every other
                // slot); the recording command buffer is inside the main
                // render pass, before the scene draws.
                sky_state.write_frame(slot, params);
                sky_state.record(ctx, command, slot);
            }
            // The corner buffer must be written before the scene pass for
            // both the shadowed and ordinary paths (finding 5: the wet
            // ground re-draw reads this camera block; the ordinary branch
            // used to write it, the shadowed branch now depends on it).
            if let (Some(resources), Some(params)) = (&ctx.corner, corner_params) {
                // SAFETY: the slot's fence wait passed above; the coherent
                // frame regions do not overlap other slots or the immutable
                // material tail.
                resources.write_frame(slot, params);
            }
            if shadowed.is_some()
                && let (Some(resources), Some(shadow_state)) = (&ctx.corner, &pipes.shadows)
            {
                // The 6-F shadowed corner draws: the shadowed lit
                // pipeline inside the main render pass, binding the
                // shadow state's per-material sets for this slot (the
                // map view rides binding 4; the FrameParams dynamic
                // offset selects this slot's extended region — written
                // before the depth pass above). The corner buffer
                // was written before this branch (the 6-F correction
                // to finding 5: both shadowed and ordinary branches
                // now supply the true camera). The ensure above
                // guaranteed the shadow state whenever `shadowed` is
                // Some; the belt check keeps an unshadowed frame on the
                // unshadowed path even if a prior shadowed frame created
                // the state.
                let (pipeline, layout, sets, frame_offset) = shadow_state.shadowed(slot);
                ctx.device
                    .cmd_bind_pipeline(command, vk::PipelineBindPoint::GRAPHICS, pipeline);
                resources.draw(ctx, command, layout, &sets, frame_offset);
            } else if let (Some(resources), Some(_params)) = (&ctx.corner, corner_params) {
                ctx.device.cmd_bind_pipeline(
                    command,
                    vk::PipelineBindPoint::GRAPHICS,
                    pipes.lit_pipeline,
                );
                // The corner buffer was written above (the 6-F
                // shadowed path now supplies the camera, so the
                // ordinary branch no longer writes). The slot's
                // fence wait passed; the coherent frame regions do
                // not overlap other slots or the immutable material
                // tail.
                resources.draw(
                    ctx,
                    command,
                    pipes.lit_layout,
                    resources.sets(),
                    slot as u32 * resources.frame_stride(),
                );
                // The 6-E instanced-proxy draws (same render pass, after
                // the static corner mesh — the corner ground plane wins
                // the coplanar depth at proxy footprints deterministically
                // in BOTH submission modes, since the corner mesh draws
                // first and proxy order is the packed instance order).
                if let (Some(proxy_state), Some(upload), Some(sets)) =
                    (&pipes.proxy, proxy, proxy_sets)
                {
                    ctx.device.cmd_bind_pipeline(
                        command,
                        vk::PipelineBindPoint::GRAPHICS,
                        pipes.proxy_pipeline,
                    );
                    // SAFETY: the proxy pipeline is bound with its own
                    // layout inside its render pass; `sets` were validated
                    // before recording; this slot's instance region holds
                    // this frame's bytes (written above, after the fence
                    // wait).
                    let recorded = proxy_state.draw(
                        ctx,
                        command,
                        pipes.proxy_layout,
                        resources,
                        slot,
                        upload,
                        sets,
                    );
                    // The recorded count and the pure submission model must
                    // agree (the unit test pins the model; this co-pins the
                    // loop against it in debug builds).
                    debug_assert_eq!(
                        recorded,
                        proxy_draw::proxy_draw_call_count(
                            upload.mode,
                            upload.npc_count,
                            upload.vehicle_count
                        )
                    );
                    outcome.proxy_draw_calls = recorded;
                }
            } else {
                ctx.device.cmd_bind_pipeline(
                    command,
                    vk::PipelineBindPoint::GRAPHICS,
                    pipes.scene_pipeline,
                );
                ctx.device.cmd_push_constants(
                    command,
                    pipes.scene_layout,
                    vk::ShaderStageFlags::VERTEX,
                    0,
                    bytemuck::bytes_of(view_proj),
                );
                // The static demonstration scene draws only when the explicit
                // opt-in uploaded it (`scene_index_count > 0`, P3) — an
                // omitted renderer binds nothing and draws nothing here; the
                // scene pipeline below still draws the dynamic region
                // geometry through the same bound state.
                if pipes.scene_index_count > 0 {
                    ctx.device
                        .cmd_bind_vertex_buffers(command, 0, &[pipes.scene_vbo], &[0]);
                    ctx.device.cmd_bind_index_buffer(
                        command,
                        pipes.scene_ibo,
                        0,
                        vk::IndexType::UINT32,
                    );
                    ctx.device
                        .cmd_draw_indexed(command, pipes.scene_index_count, 1, 0, 0, 0);
                }

                if outcome.region.drawn > 0 {
                    let region_offset = (slot as vk::DeviceSize) * pipes.region_slot_bytes;
                    ctx.device.cmd_bind_vertex_buffers(
                        command,
                        0,
                        &[pipes.region_vbo],
                        &[region_offset],
                    );
                    if world_drawn > 0 {
                        ctx.device.cmd_draw(command, world_drawn, 1, 0, 0);
                    }
                    if let Some(layer) = foreground.filter(|_| outcome.foreground_drawn > 0) {
                        let near_viewport = vk::Viewport {
                            min_depth: 0.0,
                            max_depth: FOREGROUND_MAX_DEPTH,
                            ..viewport
                        };
                        ctx.device.cmd_set_viewport(command, 0, &[near_viewport]);
                        ctx.device.cmd_push_constants(
                            command,
                            pipes.scene_layout,
                            vk::ShaderStageFlags::VERTEX,
                            0,
                            bytemuck::bytes_of(layer.view_proj),
                        );
                        ctx.device
                            .cmd_draw(command, outcome.foreground_drawn, 1, world_drawn, 0);
                    }
                }
            }

            if outcome.overlay.drawn > 0 {
                ctx.device.cmd_bind_pipeline(
                    command,
                    vk::PipelineBindPoint::GRAPHICS,
                    pipes.overlay_pipeline,
                );
                ctx.device.cmd_bind_vertex_buffers(
                    command,
                    0,
                    &[frames.overlay_slots[slot].buffer],
                    &[0],
                );
                ctx.device.cmd_bind_descriptor_sets(
                    command,
                    vk::PipelineBindPoint::GRAPHICS,
                    pipes.overlay_layout,
                    0,
                    &[ctx.descriptors.set],
                    &[],
                );
                ctx.device.cmd_draw(command, outcome.overlay.drawn, 1, 0, 0);
            }

            // The Era-1 wet reflection's ground re-draw (package 7):
            // inside the MAIN render pass, after the mesh draws —
            // the ground quad re-rasterized (EQUAL depth) with the
            // reflection sampled at the same screen position and
            // the Schlick blend on the alpha stage. The mirrored-
            // scene pass and its barrier preceded this pass (the
            // frame flow's recording order, above the post branch).
            // SAFETY: the command buffer is inside the main render
            // pass, past the mesh draws; this slot's corner
            // FrameParams region holds the true camera (the frame
            // flow's write).
            if wet.is_some()
                && let (Some(reflection_state), Some(corner)) = (&pipes.reflection, &ctx.corner)
            {
                // SAFETY: the outer recording block's contract (the
                // command buffer records inside the main render
                // pass, past the mesh draws).
                reflection_state.record_ground_redraw(ctx, corner, command, slot, false);
            }

            ctx.device.cmd_end_render_pass(command);
        }
        // The 6-F known-answer readback: after the main pass the
        // shadow map sits in SHADER_READ — transition to TRANSFER_SRC
        // and copy the recording slot's map into the mapped buffer
        // (the submission fence proves the copy complete before
        // `take_shadow_map_capture` reads it). The handshake lives in
        // the frame state (replacement-safe).
        if frames.shadow_readback.pending
            && let Some(shadow_state) = &pipes.shadows
        {
            shadow_state.record_readback(ctx, command, slot);
            frames.shadow_readback.pending = false;
            frames.shadow_readback.ready = true;
            frames.shadow_readback.slot = slot;
        }
        if frames.timestamps_supported {
            ctx.device.cmd_write_timestamp(
                command,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                frames.timestamp_pool,
                (slot as u32) * 2 + 1,
            );
        }
        // The bounded frame-capture readback (the orientation
        // corrective's evidence path): after the render pass the image
        // sits in PRESENT_SRC — transition to TRANSFER_SRC, copy the
        // whole image into the mapped buffer, and transition back, all
        // inside this frame's own command buffer so the submission
        // fence proves the copy complete before `take_capture` reads.
        // The record-time guard (the post-orientation hardening + the
        // cache-validation corrective): the acquire path may have
        // recreated the swapchain AFTER the pre-acquire preparation —
        // a destination mismatched in extent, format, OR transfer-
        // source capability is never copied into; the capture is
        // DROPPED (a typed, observable drop) and the frame renders
        // normally.
        if frames.capture.pending {
            if frames.capture.matches(
                extent.width,
                extent.height,
                swapchain.format,
                swapchain.transfer_src_supported,
            ) {
                let image = swapchain.images[image_index as usize];
                let subresource = vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    level_count: 1,
                    layer_count: 1,
                    ..Default::default()
                };
                let to_transfer = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                    .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .image(image)
                    .subresource_range(subresource)
                    .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
                ctx.device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[to_transfer],
                );
                let copy = vk::BufferImageCopy::default()
                    .buffer_offset(0)
                    .buffer_row_length(0)
                    .buffer_image_height(0)
                    .image_subresource(vk::ImageSubresourceLayers {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        mip_level: 0,
                        base_array_layer: 0,
                        layer_count: 1,
                    })
                    .image_extent(vk::Extent3D {
                        width: extent.width,
                        height: extent.height,
                        depth: 1,
                    });
                ctx.device.cmd_copy_image_to_buffer(
                    command,
                    image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    frames.capture.buffer,
                    &[copy],
                );
                let to_present = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                    .image(image)
                    .subresource_range(subresource)
                    .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                    .dst_access_mask(vk::AccessFlags::empty())
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
                ctx.device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[to_present],
                );
                frames.capture.pending = false;
                frames.capture.ready = true;
                frames.capture.slot = slot;
            } else {
                // The prepared destination does not match the image
                // being recorded (the swapchain changed mid-frame —
                // its extent, format, or transfer-source capability):
                // drop the request — never copy into a mismatched or
                // unsupported destination — and record the typed,
                // observable drop.
                frames.capture.pending = false;
                frames.capture.last_refusal = Some(RendererError::CaptureDropped(format!(
                    "the swapchain changed between the capture preparation and the \
                     recording (extent {}×{}, format {:?}, transfer-src {})",
                    extent.width, extent.height, swapchain.format, swapchain.transfer_src_supported
                )));
            }
        }
        ctx.device
            .end_command_buffer(command)
            .map_err(|error| RendererError::vk(error, "end_command_buffer"))?;

        // Submit: wait for the acquire semaphore, signal the image's
        // present-wait semaphore (corrective finding C) and the slot
        // fence.
        let wait_stage = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let wait_semaphores = [frames.image_available[slot]];
        let signal_semaphores = [present_semaphore];
        let commands = [command];
        let submit_info = vk::SubmitInfo::default()
            .wait_semaphores(&wait_semaphores)
            .wait_dst_stage_mask(&wait_stage)
            .command_buffers(&commands)
            .signal_semaphores(&signal_semaphores);
        ctx.device
            .queue_submit(ctx.graphics_queue, &[submit_info], frames.in_flight[slot])
            .map_err(|error| RendererError::vk(error, "queue_submit"))?;
        if frames.timestamps_supported {
            frames.timestamps_written[slot] = true;
        }
    }

    // Present.
    let present_wait_semaphores = [present_semaphore];
    let present_swapchains = [swapchain.swapchain];
    let present_image_indices = [image_index];
    let present_info = vk::PresentInfoKHR::default()
        .wait_semaphores(&present_wait_semaphores)
        .swapchains(&present_swapchains)
        .image_indices(&present_image_indices);
    match unsafe {
        ctx.swapchain_ext
            .queue_present(ctx.graphics_queue, &present_info)
    } {
        Ok(present_suboptimal) => {
            outcome.presented = true;
            if present_suboptimal || outcome.suboptimal {
                outcome.suboptimal = true;
                mark_suboptimal(&mut frames.needs_recreate);
            }
        }
        Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
            // DEFERRED, not inline (the batch-3 review's lifecycle
            // finding: every recreation entry point honors surface
            // availability): the presentation is gone; latch the
            // recreation intent and the deferral hint — the next
            // frame's probe-gated consume-and-recreate block rebuilds
            // once the drawable is available again. This frame's
            // submission completed and its fence will signal normally;
            // no synchronization state is touched here.
            mark_suboptimal(&mut frames.needs_recreate);
            frames.surface_probe_pending = true;
        }
        Err(error) => return Err(RendererError::vk(error, "queue_present")),
    }

    outcome.gpu_render_ms = gpu_render_ms;
    outcome.indirect_draws_issued =
        (ctx.indirect_draws_issued.get() - indirect_issued_before) as u32;
    frames.frame_slot = (slot + 1) % FRAMES_IN_FLIGHT as usize;
    Ok(outcome)
}

/// Safe-to-call device idle used by the renderer's ordered teardown.
pub(crate) fn device_idle(ctx: &VulkanContext) -> Result<(), RendererError> {
    unsafe { ctx.device.device_wait_idle() }
        .map_err(|error| RendererError::vk(error, "device_wait_idle"))
}

fn mut_at<T>(slice: &mut [T], index: usize) -> Option<&mut T> {
    slice.get_mut(index)
}

fn read_timestamps(ctx: &VulkanContext, frames: &FrameState, slot: usize) -> Option<f64> {
    let mut stamps = [0u64; 2];
    let result = unsafe {
        ctx.device.get_query_pool_results(
            frames.timestamp_pool,
            (slot as u32) * 2,
            &mut stamps,
            vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
        )
    };
    if result.is_err() {
        return None;
    }
    // Timestamp counters may be narrower than 64 bits (corrective
    // finding E): mask the difference to the valid width so a wrapped
    // counter still yields the correct interval.
    let bits = ctx.timestamp_valid_bits.min(64);
    let mask = if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    let delta = stamps[1].wrapping_sub(stamps[0]) & mask;
    Some(delta as f64 * ctx.timestamp_period_ns / 1_000_000.0)
}

/// Swapchain recreation (corrective findings F + G): idle the device
/// (draining outstanding presents), then build replacement resources.
/// Passing oldSwapchain retires it even when vkCreateSwapchainKHR fails.
/// A failed replacement releases the retired handle and retries with a
/// fresh swapchain before the next image acquisition.
/// When the surface format changed, the replacement pipelines are built
/// first — framebuffers are always created against the render pass that
/// will be current after the swap, never a stale one.
#[allow(clippy::too_many_arguments)]
fn recreate_swapchain(
    ctx: &mut VulkanContext,
    swapchain: &mut SwapchainState,
    pipes: &mut PipelineState,
    frames: &mut FrameState,
    window_size: (u32, u32),
    present_preference: PresentModePreference,
    replace_pipelines: bool,
    demo_scene: DemoScenePreference,
) -> Result<(), RendererError> {
    // Keep the request pending across failures, including an error before
    // the replacement reaches vkCreateSwapchainKHR.
    frames.needs_recreate = true;
    unsafe { ctx.device.device_wait_idle() }
        .map_err(|error| RendererError::vk(error, "device_wait_idle"))?;
    let depth_format = swapchain.depth_format;
    let (new_format, new_color_space) = pick_surface_format(ctx)?;
    // Build the replacement pipelines first when the format changed
    // (finding F) — or when the verification flag forces the
    // replacement path (`--replace-pipelines`, exercising the
    // format-change lifecycle without a real format change; the CI
    // resize smoke cannot otherwise reach it because resizing keeps
    // the surface format). On failure the old state is untouched and
    // the partial replacement cleans itself up (finding G). The
    // replacement is built with the LIVE demo-scene preference (P3):
    // a recreated pipeline renders exactly what its predecessor did —
    // an omitted renderer must not gain demo content by recreating,
    // and an opted-in demonstration must not lose it.
    let replacement_pipes = if replace_pipelines || new_format != swapchain.format {
        let mut replacement = PipelineState::new(ctx, new_format, depth_format, demo_scene)?;
        // Stage 4 final-review finding A: a replacement pipeline must
        // be fully prepared for dynamic region drawing BEFORE it
        // becomes active. If the outgoing state owns a lazily
        // allocated region-scene buffer, the replacement allocates its
        // own before installation — a failure destroys the replacement
        // and leaves the old state valid (the finding-G discipline),
        // so no half-installed pipeline state and, afterward, no null
        // mapped-buffer writes or null region-VBO draws. The default
        // `--render` path never has a region buffer, so it never pays
        // this allocation.
        if pipes.region_vbo != vk::Buffer::null()
            && let Err(error) = replacement.ensure_region_capacity(ctx)
        {
            replacement.destroy(ctx);
            return Err(error);
        }
        // The 6-E companion: a populated renderer's proxy resources
        // must likewise exist on the replacement BEFORE installation
        // (the same finding-A discipline — the replacement pipeline
        // reads the same lazily-created mesh VBOs and instance
        // buffer, so a half-installed state must never draw through
        // null handles). The 6-E2 indirect command state re-ensures
        // the same way when the outgoing state owned one: the
        // replacement's indirect draws read the same lazily-created
        // command buffer.
        if pipes.proxy.is_some() {
            if let Err(error) = replacement.ensure_proxy_resources(ctx) {
                replacement.destroy(ctx);
                return Err(error);
            }
            if pipes
                .proxy
                .as_ref()
                .is_some_and(|state| state.indirect.is_some())
                && let Some(state) = replacement.proxy.as_mut()
                && let Err(error) = state.ensure_indirect(ctx)
            {
                replacement.destroy(ctx);
                return Err(error);
            }
        }
        // The 6-F companion: a shadowed renderer's shadow state must
        // likewise exist on the replacement BEFORE installation (the
        // same finding-A discipline — the replacement's shadowed
        // pipeline renders inside the replacement's own render pass,
        // against the replacement's own maps).
        if pipes.shadows.is_some()
            && let Err(error) = replacement.ensure_shadow_resources(ctx)
        {
            replacement.destroy(ctx);
            return Err(error);
        }
        // The Era-1 post companion: a post renderer's post state
        // must likewise exist on the replacement BEFORE
        // installation (the same finding-A discipline — the
        // replacement's post pass targets the NEW swapchain
        // format, so the state is rebuilt against it here; the
        // extent-bound targets build at the replacement's first
        // frame, against the live views). The shadowed/sky HDR
        // VARIANTS are NOT built here: they bind the shadow/sky
        // states' OWN pipeline layouts, and the replacement's
        // shadow/sky states may not exist yet — the frame flow's
        // family-growth block creates them against the LIVE
        // states' layouts at the first frame that needs them.
        if pipes.post.is_some()
            && let Err(error) = replacement.ensure_post_resources(ctx, new_format, depth_format)
        {
            replacement.destroy(ctx);
            return Err(error);
        }
        Some(replacement)
    } else {
        None
    };
    let render_pass_for_new = replacement_pipes
        .as_ref()
        .map_or(pipes.render_pass, |replacement| replacement.render_pass);
    match SwapchainState::new(
        ctx,
        window_size,
        present_preference,
        new_format,
        new_color_space,
        render_pass_for_new,
        depth_format,
        // After a failed replacement, this is null and the native window
        // is free. Otherwise Vulkan requires the current live handle.
        swapchain.swapchain,
    ) {
        Ok(mut new_state) => {
            // The old handle is already retired.
            if let Err(error) = frames.resize_images(ctx, new_state.image_count) {
                new_state.destroy(ctx);
                swapchain.destroy(ctx);
                if let Some(mut replacement) = replacement_pipes {
                    replacement.destroy(ctx);
                }
                return Err(error);
            }
            // Install, then release the old state — framebuffers before
            // the render pass they reference.
            let mut old_swapchain = std::mem::replace(swapchain, new_state);
            let old_pipes =
                replacement_pipes.map(|replacement| std::mem::replace(pipes, replacement));
            old_swapchain.destroy(ctx);
            if let Some(mut old_pipes) = old_pipes {
                old_pipes.destroy(ctx);
            }
            frames.needs_recreate = false;
            Ok(())
        }
        Err(error) => {
            // The constructor cleaned its partial replacement. Discard
            // the old chain even if the failure preceded retirement.
            swapchain.destroy(ctx);
            if let Some(mut replacement) = replacement_pipes {
                replacement.destroy(ctx);
            }
            Err(error)
        }
    }
}

/// Timestamp read policy (corrective finding E): query results are read
/// only when the queue family supports timestamps AND the slot's queries
/// have resolved at least once — never block-read unwritten queries.
pub(crate) fn timestamp_sample_ready(supported: bool, written: bool) -> bool {
    supported && written
}

/// Presentation-wait semaphore table growth (corrective finding C): the
/// table only ever grows to cover the required length — entries are never
/// invalidated, because recreation idles the device first.
pub(crate) fn present_semaphore_len_for(current_len: usize, required: usize) -> usize {
    current_len.max(required)
}

/// Records that a present reported suboptimal — the next frame recreates
/// before drawing.
pub(crate) fn mark_suboptimal(needs_recreate: &mut bool) {
    *needs_recreate = true;
}

/// Consumes a pending recreation request; true when one was scheduled.
pub(crate) fn consume_recreate(needs_recreate: &mut bool) -> bool {
    let pending = *needs_recreate;
    *needs_recreate = false;
    pending
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    /// The 6-E per-kind instance ranges: the fixed npc-then-vehicle
    /// order partitions the slot (disjoint, contiguous, covering the
    /// total), and the draw-call count is the pinned headline — one
    /// per populated kind instanced, one per instance in the
    /// per-object reference.
    #[test]
    fn proxy_kind_ranges_and_draw_call_counts() {
        use crate::ProxyDrawMode;
        use proxy_draw::{ProxyKind, kind_instance_range, proxy_draw_call_count};

        let stride = crate::proxy_instances::INSTANCE_STRIDE as u64;
        // A mixed population: npc [0, 3), vehicle [3, 5).
        let npc = kind_instance_range(3, 2, ProxyKind::Npc);
        let vehicle = kind_instance_range(3, 2, ProxyKind::Vehicle);
        assert_eq!(npc, 0..3);
        assert_eq!(vehicle, 3..5);
        assert_eq!(npc.end, vehicle.start, "contiguous");
        assert_eq!(vehicle.end, 5, "covers the total");
        // Byte offsets (the vertex-buffer binding arithmetic).
        assert_eq!(npc.start * stride, 0);
        assert_eq!(vehicle.start * stride, 3 * stride);

        // The pinned integers: the populated corner scene's proxy
        // submission count under each mode. The 6-E2 indirect mode
        // records the SAME count as the direct instanced mode — the
        // GPU-side count changes where N lives, never how many draws
        // submit (the headline must not regress).
        assert_eq!(
            proxy_draw_call_count(ProxyDrawMode::Instanced, 24, 2),
            2,
            "one per kind"
        );
        assert_eq!(
            proxy_draw_call_count(ProxyDrawMode::Indirect, 24, 2),
            2,
            "the indirect mode: still one per kind"
        );
        assert_eq!(
            proxy_draw_call_count(ProxyDrawMode::PerInstance, 24, 2),
            26,
            "one per instance"
        );
        assert_eq!(
            proxy_draw_call_count(ProxyDrawMode::Indirect, 0, 2),
            1,
            "empty kind draws nothing"
        );
        assert_eq!(proxy_draw_call_count(ProxyDrawMode::PerInstance, 0, 0), 0);
        assert_eq!(
            proxy_draw_call_count(ProxyDrawMode::Indirect, 0, 0),
            0,
            "empty population"
        );
    }

    #[test]
    fn layered_upload_preserves_order_truncation_and_neighboring_slots() {
        // Use canaries in the surrounding slots; this exercises the exact
        // pointer arithmetic and F-03 decision used by both vertex buffers.
        let mut storage = [0u32; 12];
        for (first, second, expected, first_count, omitted) in [
            (&[10, 11][..], &[20, 21, 22][..], [10, 11, 20, 21], 2, 1),
            (&[1, 2, 3, 4, 5], &[8], [1, 2, 3, 4], 4, 2),
            (&[], &[3, 4], [3, 4, 0, 0], 0, 0),
        ] {
            storage.fill(0);
            let (decision, split) = unsafe {
                super::upload_layers(
                    storage.as_mut_ptr().cast(),
                    1,
                    4,
                    first,
                    second,
                    LayerPriority::First,
                )
            };
            assert_eq!(&storage[4..8], &expected);
            assert_eq!(&storage[..4], &[0; 4]);
            assert_eq!(&storage[8..], &[0; 4]);
            assert_eq!(split, first_count);
            assert_eq!(decision.truncated, omitted);
            assert_eq!(decision.supplied, decision.drawn + omitted);
        }
        for (foreground, expected, world_count) in [
            (&[7, 8, 9][..], [1, 7, 8, 9], 1),
            (&[7, 8, 9, 10, 11][..], [7, 8, 9, 10], 0),
            (&[][..], [1, 2, 3, 4], 4),
        ] {
            let world = [1u32, 2, 3, 4, 5];
            let (decision, split) = unsafe {
                super::upload_layers(
                    storage.as_mut_ptr().cast(),
                    1,
                    4,
                    &world,
                    foreground,
                    LayerPriority::Second,
                )
            };
            assert_eq!(&storage[4..8], &expected);
            assert_eq!(&storage[..4], &[0; 4]);
            assert_eq!(&storage[8..], &[0; 4]);
            assert_eq!(split, world_count);
            assert_eq!(
                decision.truncated as usize,
                world.len() + foreground.len() - 4
            );
        }
    }

    #[test]
    fn timestamp_gate_requires_support_and_a_resolved_sample() {
        for supported in [false, true] {
            for written in [false, true] {
                assert_eq!(
                    timestamp_sample_ready(supported, written),
                    supported && written,
                    "supported={supported} written={written}"
                );
            }
        }
    }

    #[test]
    fn present_semaphore_table_only_grows() {
        let mut len = 2;
        for acquired in [0u32, 1, 5, 3] {
            len = present_semaphore_len_for(len, acquired as usize + 1);
            assert!(len > acquired as usize);
        }
        assert!(len >= 6);
        assert_eq!(present_semaphore_len_for(6, 4), 6);
    }

    #[test]
    fn suboptimal_present_schedules_recreation_until_consumed() {
        let mut needs_recreate = false;
        mark_suboptimal(&mut needs_recreate);
        assert!(needs_recreate);
        assert!(consume_recreate(&mut needs_recreate));
        assert!(!needs_recreate);
        assert!(!consume_recreate(&mut needs_recreate));
    }

    #[test]
    fn overlay_slots_grow_on_demand_to_the_existing_vertex_budget() {
        let limit = crate::overlay::overlay_capacity_vertices() / FRAMES_IN_FLIGHT;
        assert_eq!(OverlaySlot::capacity_for(0, 0), 0);
        assert_eq!(OverlaySlot::capacity_for(0, 37), 64);
        assert_eq!(OverlaySlot::capacity_for(64, 0), 64);
        assert_eq!(OverlaySlot::capacity_for(64, 64), 64);
        assert_eq!(OverlaySlot::capacity_for(64, usize::MAX), limit);
        assert_eq!(OverlaySlot::capacity_for(limit, usize::MAX), limit);
    }

    fn capabilities(current: [u32; 2], min: [u32; 2], max: [u32; 2]) -> vk::SurfaceCapabilitiesKHR {
        vk::SurfaceCapabilitiesKHR {
            current_extent: vk::Extent2D {
                width: current[0],
                height: current[1],
            },
            min_image_extent: vk::Extent2D {
                width: min[0],
                height: min[1],
            },
            max_image_extent: vk::Extent2D {
                width: max[0],
                height: max[1],
            },
            ..Default::default()
        }
    }

    #[test]
    fn zero_sized_surface_capabilities_defer_rendering() {
        // The Win32 minimize/transition signal: current, min, and max
        // all report (0,0) — no valid extent exists.
        assert!(surface_extent_unavailable(&capabilities(
            [0, 0],
            [0, 0],
            [0, 0]
        )));
        assert!(surface_extent_unavailable(&capabilities(
            [0, 720],
            [0, 0],
            [0, 0]
        )));
    }

    #[test]
    fn valid_and_unspecified_surfaces_remain_renderable() {
        assert!(!surface_extent_unavailable(&capabilities(
            [1280, 720],
            [1, 1],
            [2560, 1440]
        )));
        // Unspecified extent (headless/software surfaces) stays
        // renderable — the application drives the size.
        assert!(!surface_extent_unavailable(&capabilities(
            [u32::MAX, u32::MAX],
            [1, 1],
            [2560, 1440]
        )));
    }

    #[test]
    fn extent_selection_never_fabricates_nonzero_from_zero() {
        // The rig defect: clamping must not invent a 1×1 swapchain from
        // zero-sized capabilities. The frame path defers before any
        // swapchain ever sees this result.
        let extent = clamp_extent((1280, 720), &capabilities([0, 0], [0, 0], [0, 0]));
        assert_eq!((extent.width, extent.height), (0, 0));
    }

    #[test]
    fn extent_selection_respects_pinned_and_unspecified_surfaces() {
        let pinned = clamp_extent(
            (1280, 720),
            &capabilities([1920, 1080], [1, 1], [3840, 2160]),
        );
        assert_eq!((pinned.width, pinned.height), (1920, 1080));
        let driven = clamp_extent(
            (1280, 720),
            &capabilities([u32::MAX, u32::MAX], [1, 1], [3840, 2160]),
        );
        assert_eq!((driven.width, driven.height), (1280, 720));
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn portability_enumeration_enabled_with_flag_when_advertised() {
        let plan = InstanceExtensionPlan::select(
            &strings(&["VK_KHR_surface", "VK_EXT_metal_surface"]),
            &strings(&[
                "VK_KHR_surface",
                "VK_KHR_portability_enumeration",
                "VK_EXT_debug_utils",
            ]),
            false,
        );
        assert!(plan.portability_enumeration);
        assert_eq!(
            plan.create_flags(),
            vk::InstanceCreateFlags::ENUMERATE_PORTABILITY_KHR
        );
        assert_eq!(
            plan.extensions(),
            strings(&[
                "VK_KHR_surface",
                "VK_EXT_metal_surface",
                "VK_KHR_portability_enumeration",
            ])
        );
    }

    #[test]
    fn portability_enumeration_absent_keeps_existing_behavior() {
        let plan = InstanceExtensionPlan::select(
            &strings(&["VK_KHR_surface", "VK_EXT_headless_surface"]),
            &strings(&["VK_KHR_surface", "VK_EXT_debug_utils"]),
            false,
        );
        assert!(!plan.portability_enumeration);
        assert_eq!(plan.create_flags(), vk::InstanceCreateFlags::empty());
        assert_eq!(
            plan.extensions(),
            strings(&["VK_KHR_surface", "VK_EXT_headless_surface"])
        );
    }

    #[test]
    fn sdl_required_extensions_preserved_without_duplicates() {
        // SDL's list keeps its order and content; portability enumeration
        // is appended exactly once even when the loader also advertises
        // it, and never appended on top of an SDL-required entry.
        let plan = InstanceExtensionPlan::select(
            &strings(&["VK_KHR_surface", "VK_EXT_metal_surface"]),
            &strings(&["VK_KHR_portability_enumeration"]),
            false,
        );
        assert_eq!(
            plan.extensions(),
            strings(&[
                "VK_KHR_surface",
                "VK_EXT_metal_surface",
                "VK_KHR_portability_enumeration",
            ])
        );
        // Defensive: if SDL itself required the extension, it is not
        // duplicated — but the flag decision still holds.
        let already = InstanceExtensionPlan::select(
            &strings(&["VK_KHR_surface", "VK_KHR_portability_enumeration"]),
            &strings(&["VK_KHR_portability_enumeration"]),
            false,
        );
        assert_eq!(
            already.extensions(),
            strings(&["VK_KHR_surface", "VK_KHR_portability_enumeration"])
        );
        assert!(already.portability_enumeration);
    }

    #[test]
    fn validation_fallback_keeps_portability_and_drops_only_debug_utils() {
        // Mac 2 authorization §4 hazard: the fallback must remove the
        // validation-only extension by name — never the portability
        // extension that precedes it — and the flag decision survives.
        let with = InstanceExtensionPlan::select(
            &strings(&["VK_KHR_surface", "VK_EXT_metal_surface"]),
            &strings(&["VK_KHR_portability_enumeration"]),
            true,
        );
        assert!(with.validation);
        let without = with.without_validation();
        assert!(!without.validation);
        assert!(without.portability_enumeration);
        assert_eq!(without.create_flags(), with.create_flags());
        assert_eq!(
            without.extensions(),
            strings(&[
                "VK_KHR_surface",
                "VK_EXT_metal_surface",
                "VK_KHR_portability_enumeration",
            ])
        );
    }

    #[test]
    fn validation_disabled_adds_no_debug_utils_extension() {
        let plan = InstanceExtensionPlan::select(
            &strings(&["VK_KHR_surface"]),
            &strings(&["VK_KHR_portability_enumeration"]),
            false,
        );
        assert!(!plan.validation);
        assert!(
            !plan
                .extensions()
                .contains(&"VK_EXT_debug_utils".to_string())
        );
        // Fallback on a plan that never had validation is a no-op.
        assert_eq!(plan.without_validation(), plan);
    }

    #[test]
    fn device_portability_subset_enabled_only_when_advertised() {
        assert_eq!(
            select_device_extensions(true),
            vec![khr::swapchain::NAME, khr::portability_subset::NAME]
        );
        assert_eq!(select_device_extensions(false), vec![khr::swapchain::NAME]);
    }

    #[test]
    fn device_capability_scan_keeps_swapchain_mandatory() {
        // Swapchain absence disqualifies a device (candidacy enforces
        // the scan's verdict); portability subset alone neither
        // qualifies nor disqualifies — it only parameterizes creation.
        let neither = device_extension_capabilities(&strings(&["VK_KHR_maintenance1"]));
        assert!(!neither.swapchain);
        assert!(!neither.portability_subset);

        let both = device_extension_capabilities(&strings(&[
            "VK_KHR_swapchain",
            "VK_KHR_portability_subset",
            "VK_KHR_something_else",
        ]));
        assert!(both.swapchain);
        assert!(both.portability_subset);

        let subset_only = device_extension_capabilities(&strings(&["VK_KHR_portability_subset"]));
        assert!(!subset_only.swapchain);
        assert!(subset_only.portability_subset);
    }

    #[test]
    fn sdl_input_duplicates_are_deduplicated_first_occurrence_first() {
        // Mac 2 review follow-up A: SDL's required list is authoritative
        // but not guaranteed duplicate-free — the enabled list must be.
        let plan = InstanceExtensionPlan::select(
            &strings(&[
                "VK_KHR_surface",
                "VK_KHR_surface",
                "VK_EXT_metal_surface",
                "VK_KHR_surface",
            ]),
            &strings(&["VK_KHR_portability_enumeration"]),
            false,
        );
        assert_eq!(
            plan.extensions(),
            strings(&[
                "VK_KHR_surface",
                "VK_EXT_metal_surface",
                "VK_KHR_portability_enumeration",
            ])
        );
    }

    #[test]
    fn validation_fallback_preserves_sdl_required_debug_utils() {
        // Mac 2 review follow-up A: if SDL itself required
        // VK_EXT_debug_utils, the fallback removes only the
        // validation-ADDED state — the required entry survives,
        // unduplicated.
        let with = InstanceExtensionPlan::select(
            &strings(&["VK_KHR_surface", "VK_EXT_debug_utils"]),
            &strings(&["VK_KHR_portability_enumeration"]),
            true,
        );
        // Validation wanting adds no duplicate on top of the required
        // entry.
        assert_eq!(
            with.extensions(),
            strings(&[
                "VK_KHR_surface",
                "VK_EXT_debug_utils",
                "VK_KHR_portability_enumeration",
            ])
        );
        let without = with.without_validation();
        assert!(!without.validation);
        assert_eq!(
            without.extensions(),
            strings(&[
                "VK_KHR_surface",
                "VK_EXT_debug_utils",
                "VK_KHR_portability_enumeration",
            ])
        );
        assert_eq!(without.create_flags(), with.create_flags());
    }

    #[test]
    fn environment_enumeration_failure_is_queued_as_a_warning() {
        // Mac 2 review follow-up B: a loader-enumeration failure must
        // be explicitly diagnosable — it rides the existing validation
        // diagnostics queue the run summary drains.
        drain_validation_messages();
        queue_environment_message("enumeration probe".to_string());
        assert!(environment_pending());
        let queued = drain_validation_messages();
        assert!(!environment_pending());
        assert_eq!(queued, vec![(1, String::from("enumeration probe"))]);
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

    /// The post-orientation hardening's H3 format contract: the
    /// supported set is EXACTLY the 4-byte-per-pixel RGBA8/BGRA8
    /// family; everything else — including the audited 8-byte
    /// fallbacks that would overflow a `width × height × 4`
    /// destination — is refused (typed, before any copy command).
    #[test]
    fn capture_formats_are_the_four_byte_family_only() {
        use super::capture_format_footprint;
        use ash::vk;

        for format in [
            vk::Format::B8G8R8A8_UNORM,
            vk::Format::B8G8R8A8_SRGB,
            vk::Format::R8G8B8A8_UNORM,
            vk::Format::R8G8B8A8_SRGB,
        ] {
            assert_eq!(
                capture_format_footprint(format),
                Some(4),
                "the supported family: {format:?}"
            );
        }
        for format in [
            vk::Format::R16G16B16A16_SFLOAT,
            vk::Format::R16G16B16A16_UNORM,
            vk::Format::R32G32B32A32_SFLOAT,
            vk::Format::B8G8R8A8_SNORM,
            vk::Format::R5G6B5_UNORM_PACK16,
            vk::Format::A8B8G8R8_UNORM_PACK32,
        ] {
            assert_eq!(
                capture_format_footprint(format),
                None,
                "outside the family — refused, never under-allocated: {format:?}"
            );
        }
    }

    /// The post-orientation hardening's H3 checked sizing: real
    /// extents fit; the u32-extreme product overflows usize and
    /// refuses rather than wrapping.
    #[test]
    fn capture_sizing_is_checked() {
        use super::capture_byte_len;
        assert_eq!(capture_byte_len(1920, 1080, 4), Some(1920 * 1080 * 4));
        assert_eq!(
            capture_byte_len(u32::MAX, u32::MAX, 4),
            None,
            "u32-extreme extents must refuse instead of wrapping"
        );
    }

    /// The final cache-validation corrective's defect-1 regression: a
    /// cached 1280×720 destination must NOT be reused for a 960×960
    /// swapchain even though the two extents share EXACTLY the same
    /// pixel count (and therefore byte length) — the reported capture
    /// dimensions must always correspond to the image actually
    /// copied. Fail-first at the held candidate: the reuse predicate
    /// compared only the byte length and the format, so this
    /// FAILED before the fix (log cc-failfirst-defect1.log).
    #[test]
    fn capture_cache_reuse_validates_the_full_extent() {
        use super::{CaptureState, capture_byte_len};
        use ash::vk;
        use ash::vk::Handle;

        let cached = CaptureState {
            buffer: vk::Buffer::from_raw(1),
            byte_len: capture_byte_len(1280, 720, 4).expect("the cached size"),
            width: 1280,
            height: 720,
            format: vk::Format::B8G8R8A8_UNORM,
            ..CaptureState::dead()
        };
        // The defect's exact shape: same pixel count, different
        // dimensions — must be rejected.
        assert!(
            !cached.matches(960, 960, vk::Format::B8G8R8A8_UNORM, true),
            "a 1280×720 cache must not serve a 960×960 swapchain (same \
             byte count, different shape)"
        );
        // The matching extent still reuses (the positive control —
        // the hot path does not reallocate).
        assert!(cached.matches(1280, 720, vk::Format::B8G8R8A8_UNORM, true));
        // A format change is likewise rejected even at the same
        // extent.
        assert!(!cached.matches(1280, 720, vk::Format::B8G8R8A8_SRGB, true));
    }

    /// The final cache-validation corrective's defect-2 regression:
    /// the record-time guard requires the CURRENT swapchain's
    /// transfer-source capability even when the destination is
    /// otherwise a perfect match — a recreated swapchain without
    /// TRANSFER_SRC must never reach the image-to-buffer copy. The
    /// same predicate drives `ensure()`'s reuse decision, with the
    /// capability checked FIRST and independently (the refusal stays
    /// pure). Fail-first: with the capability term disabled (the
    /// pre-fix defect form), this test fails (log
    /// cc-failfirst-defect2.log).
    #[test]
    fn capture_record_guard_requires_transfer_source() {
        use super::CaptureState;
        use ash::vk;
        use ash::vk::Handle;

        let cached = CaptureState {
            buffer: vk::Buffer::from_raw(1),
            byte_len: super::capture_byte_len(1280, 720, 4).expect("the cached size"),
            width: 1280,
            height: 720,
            format: vk::Format::B8G8R8A8_UNORM,
            ..CaptureState::dead()
        };
        // The capability loss with an otherwise perfect match: the
        // guard must refuse.
        assert!(
            !cached.matches(1280, 720, vk::Format::B8G8R8A8_UNORM, false),
            "a swapchain without TRANSFER_SRC must never be served from the \
             cache, however well it matches"
        );
        // With the capability present, the same match serves (the
        // positive control).
        assert!(cached.matches(1280, 720, vk::Format::B8G8R8A8_UNORM, true));
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
mod surface_probe_tests {
    use super::{SurfaceProbeDecision, surface_probe_decision};

    /// The hint-gated probe state table (the batch-3 review's
    /// corrective item 3) - the lifecycle transitions the render loop
    /// implements over this pure decision:
    /// - an ordinary frame (no hints) NEVER probes - no driver
    ///   round-trip;
    /// - a pending recreation on an available surface proceeds (the
    ///   recreation is consumed by its own block - the decision never
    ///   touches the flag);
    /// - a pending recreation on an UNAVAILABLE surface defers AND
    ///   PRESERVES the intent (the review's finding 1: the flag
    ///   survives the deferral; the recreation happens on recovery);
    /// - a prior deferral re-probes until available, then clears the
    ///   hint (recovery);
    /// - the stale-nonzero-window mid-transition case reaches the same
    ///   table through the pending-recreation hint the out-of-date
    ///   acquire path sets.
    ///
    /// The corrective-round extension: EVERY recreation entry point
    /// now routes through this table — the proactive-resize path
    /// (extent mismatch) and the out-of-date PRESENT path both latch
    /// the recreation intent plus the deferral hint and defer, so
    /// their recreation happens only on a frame where the probe
    /// reports the drawable available (ProbeAvailable); and NO
    /// deferral path destroys the slot fence — vkWaitForFences does
    /// not consume a signaled fence, so every images_in_flight
    /// reference stays valid by construction across acquisition
    /// failure, deferral, and recovery (the review's fence finding).
    #[test]
    fn probe_decision_table_preserves_recreation_intent_through_deferral() {
        // Ordinary frame: no evidence of trouble, no probe - the
        // availability answer is not even consulted.
        assert_eq!(
            surface_probe_decision(false, false, true),
            SurfaceProbeDecision::Skip
        );
        assert_eq!(
            surface_probe_decision(false, false, false),
            SurfaceProbeDecision::Skip
        );
        // Pending recreation, drawable available: proceed - the
        // consume-and-recreate block runs this frame.
        assert_eq!(
            surface_probe_decision(false, true, true),
            SurfaceProbeDecision::ProbeAvailable
        );
        // Pending recreation, drawable unavailable: DEFER - and the
        // decision's contract keeps needs_recreate untouched, so the
        // intent survives to the recovery frame (finding 1).
        assert_eq!(
            surface_probe_decision(false, true, false),
            SurfaceProbeDecision::ProbeDeferred
        );
        assert_eq!(
            surface_probe_decision(true, true, false),
            SurfaceProbeDecision::ProbeDeferred
        );
        // Recovery: the deferred hint re-probes; available clears it.
        assert_eq!(
            surface_probe_decision(true, false, true),
            SurfaceProbeDecision::ProbeAvailable
        );
        assert_eq!(
            surface_probe_decision(true, true, true),
            SurfaceProbeDecision::ProbeAvailable
        );
    }
}

/// The fault plan's pure advance logic, extracted so the seam is
/// unit-testable without a Vulkan device (the mock-free discipline:
/// the tests below exercise THIS function, the one
/// [`VulkanContext::check_target_fault`] actually runs).
///
/// A plan matches only when both the site and the stage equal the
/// check's; a match decrements the ordinal, and the fault fires when
/// it reaches zero (consuming the plan). Non-matching checks leave
/// the plan untouched. The ordinal is 1-based; `set_target_fault`
/// clamps it to at least 1.
fn advance_target_fault_plan(
    plan: &mut Option<(TargetFaultSite, TargetFaultStage, u32)>,
    site: TargetFaultSite,
    stage: TargetFaultStage,
) -> Option<RendererError> {
    let (plan_site, plan_stage, ordinal) = (*plan)?;
    if plan_site != site || plan_stage != stage {
        return None;
    }
    let remaining = ordinal - 1;
    if remaining == 0 {
        *plan = None;
        Some(RendererError::InjectedFault(format!("{site:?} {stage:?}")))
    } else {
        *plan = Some((plan_site, plan_stage, remaining));
        None
    }
}

#[cfg(test)]
mod target_fault_seam_tests {
    use super::advance_target_fault_plan;
    use crate::RendererError;
    use crate::{TargetFaultSite, TargetFaultStage};

    /// A fired plan is consumed: the second matching check passes.
    #[test]
    fn plan_fires_once_then_stays_consumed() {
        let mut plan = Some((TargetFaultSite::Post, TargetFaultStage::Image, 1));
        assert!(
            advance_target_fault_plan(&mut plan, TargetFaultSite::Post, TargetFaultStage::Image)
                .is_some()
        );
        assert_eq!(plan, None);
        assert!(
            advance_target_fault_plan(&mut plan, TargetFaultSite::Post, TargetFaultStage::Image)
                .is_none()
        );
        assert_eq!(plan, None);
    }

    /// Only a matching site AND stage decrements the ordinal; every
    /// other check combination leaves the plan untouched.
    #[test]
    fn ordinal_decrements_only_on_matching_site_and_stage() {
        let mut plan = Some((TargetFaultSite::Post, TargetFaultStage::Image, 3));
        for (site, stage) in [
            (TargetFaultSite::Wet, TargetFaultStage::Image),
            (TargetFaultSite::Shadow, TargetFaultStage::Image),
            (TargetFaultSite::Post, TargetFaultStage::Memory),
            (TargetFaultSite::Post, TargetFaultStage::Framebuffer),
        ] {
            assert!(advance_target_fault_plan(&mut plan, site, stage).is_none());
            assert_eq!(
                plan,
                Some((TargetFaultSite::Post, TargetFaultStage::Image, 3))
            );
        }
        // Three matching checks: the first two decrement, the third fires.
        assert!(
            advance_target_fault_plan(&mut plan, TargetFaultSite::Post, TargetFaultStage::Image)
                .is_none()
        );
        assert_eq!(
            plan,
            Some((TargetFaultSite::Post, TargetFaultStage::Image, 2))
        );
        assert!(
            advance_target_fault_plan(&mut plan, TargetFaultSite::Post, TargetFaultStage::Image)
                .is_none()
        );
        assert_eq!(
            plan,
            Some((TargetFaultSite::Post, TargetFaultStage::Image, 1))
        );
        assert!(
            advance_target_fault_plan(&mut plan, TargetFaultSite::Post, TargetFaultStage::Image)
                .is_some()
        );
        assert_eq!(plan, None);
    }

    /// The fired error names the site and stage (the gate asserts the
    /// variant; the message carries the cell for diagnostics).
    #[test]
    fn the_fired_fault_is_the_injected_variant_naming_its_cell() {
        let mut plan = Some((TargetFaultSite::Shadow, TargetFaultStage::Bind, 1));
        match advance_target_fault_plan(&mut plan, TargetFaultSite::Shadow, TargetFaultStage::Bind)
        {
            Some(RendererError::InjectedFault(message)) => {
                assert!(message.contains("Shadow"), "message: {message}");
                assert!(message.contains("Bind"), "message: {message}");
            }
            other => panic!("expected InjectedFault, got {other:?}"),
        }
    }

    /// An unarmed plan (None) never fires and stays None.
    #[test]
    fn an_unarmed_plan_never_fires() {
        let mut plan = None;
        for site in [
            TargetFaultSite::Post,
            TargetFaultSite::Wet,
            TargetFaultSite::Shadow,
        ] {
            for stage in [
                TargetFaultStage::Image,
                TargetFaultStage::MemoryLookup,
                TargetFaultStage::Memory,
                TargetFaultStage::Bind,
                TargetFaultStage::View,
                TargetFaultStage::Framebuffer,
            ] {
                assert!(advance_target_fault_plan(&mut plan, site, stage).is_none());
            }
        }
        assert_eq!(plan, None);
    }
}
