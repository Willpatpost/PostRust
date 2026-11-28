//! The Vulkan context and device capabilities (P4 S4 — design §5.1's
//! context row): instance/device construction with the partial-
//! initialization cleanup ladder, the extension plan, physical-device
//! and format selection, the allocation/fault diagnostic cells, the
//! device-lifetime retained states (`pipeline_cache`, `skinned_retained`,
//! the attached scene resources), and the teardown manifest. Pure move
//! from vulkan.rs (stage S4); no logic changed — only location and
//! visibility.

use std::cell::Cell;
use std::ffi::{CStr, CString};

use ash::ext;
use ash::khr;
use ash::vk;
use ash::{Device, Entry, Instance};

use sdl3::video::Window as SdlWindow;

use super::RendererError;
use super::capture::{queue_environment_message, validation_callback};
use super::corner;
use super::descriptors::DescriptorFoundation;
use super::resources::advance_target_fault_plan;
use super::skinned_draw;
use super::staging::StagingPool;
use super::textures::{SamplerPool, Texture};
use crate::config::{DepthFormatPreference, RenderConfig, TargetFaultSite, TargetFaultStage};
use std::cell::RefCell;

const VALIDATION_LAYER_NAME: &str = "VK_LAYER_KHRONOS_validation";

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
    pub(super) corner: Option<corner::CornerResources>,
    /// The skinned family's PASS-INDEPENDENT resources (the Era-2
    /// ownership transfer): the mapped pose storage, the skinned set
    /// layout/pool/sets, and the skinned pipeline layout — created
    /// LAZILY once at the first populated frame EVER, then INHERITED
    /// by every pipeline generation (the `pipeline_cache` precedent:
    /// device-lifetime objects shared across swapchain recreations
    /// and pipeline replacements, so a replacement rebuilds only the
    /// pass-bound pipelines and the allocation counters stay flat
    /// across the replacement lifecycle). Valid because these
    /// objects bind no render pass — only the corner's own resources
    /// (ctx-owned, same lifetime) plus the family's own pose buffer.
    /// Released at context teardown, BEFORE the corner (reverse
    /// creation order), and after every PipelineState whose
    /// pipelines read it (the Renderer's drop order guarantees
    /// pipes.destroy precedes ctx.destroy).
    pub(crate) skinned_retained: Option<skinned_draw::SkinnedRetained>,
    pub(super) texture: Texture,
    pub(super) samplers: SamplerPool,
    pub(super) descriptors: DescriptorFoundation,
    memory_allocations: Cell<u64>,
    pub(super) texture_allocations: Cell<u64>,
    pub(super) descriptor_pools: Cell<u64>,
    pub(super) descriptor_sets: Cell<u64>,
    pub(super) sampler_creations: Cell<u64>,
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
    pub(super) indirect_draws_issued: Cell<u64>,
    /// P5 §1.6 (A11/R1): arena growth events since renderer
    /// construction — every RE-CREATION of a mapped carve at a
    /// larger size (the instance/pose/region arenas' overlay-style
    /// growth; the initial demand-sized carve is not an event:
    /// nothing existed to replace). Each event stalls its frame once
    /// under `device_wait_idle` (the growth paths' documented
    /// discipline); the frame flow reads the DELTA per frame (the
    /// indirect-issuance observation's exact shape) into
    /// `FrameOutcome::arena_growth_events` → `RenderStats`.
    pub(super) arena_growth_events: Cell<u64>,
    /// Diagnostic fault-injection seam: arms a one-shot plan for the
    /// corrective finding 3+8 verification. The `u32` ordinal is
    /// 1-based and decrements per MATCHING check across the renderer's
    /// lifetime (the plan is NOT reset when consumed, only when the
    /// ordinal reaches 0). This cumulative decrement allows the same
    /// site/stage to fire multiple times across multiple target
    /// builders (e.g., shadow's per-slot loop). The plan is NEVER
    /// touched by real device calls — it is test-only and diagnostic.
    target_fault: Cell<Option<(TargetFaultSite, TargetFaultStage, u32)>>,
    /// Diagnostic fault-injection seam for the LIFECYCLE paths (the
    /// CA-01/CA-10/CA-11 verification knobs — the same one-shot
    /// plan/ordinal discipline as `target_fault` above, one flat
    /// site enum naming a single fallible step: see
    /// [`crate::LifecycleFaultSite`]). Test-only, diagnostic-only:
    /// never touched by real device calls.
    lifecycle_fault: Cell<Option<(crate::LifecycleFaultSite, u32)>>,
    /// CA-01 census counters (see `crate::AllocationStats`'s
    /// `corner_pipeline_objects` / `corner_pipeline_object_frees` —
    /// the field docs are the contract): creation counts of the
    /// corner pipeline group's staged objects, and the scoped
    /// owner's destruction counts.
    corner_pipeline_objects: Cell<u64>,
    corner_pipeline_object_frees: Cell<u64>,
    /// CA-11 census counters (see `crate::AllocationStats`'s
    /// `arena_replacements` / `arena_replacement_frees`): staging
    /// counts of the arena growth paths' replacement buffer pairs,
    /// and the scoped owner's destruction counts.
    arena_replacements: Cell<u64>,
    arena_replacement_frees: Cell<u64>,
    /// The one-shot acquire-out-of-date injection (finding 6's
    /// verification knob — the driver-forced
    /// `VK_ERROR_OUT_OF_DATE_KHR` acquire path, unreachable from the
    /// test API otherwise). Diagnostic-only, test-only: consumed at
    /// the FIRST acquire site of the next frame so the retry acquire
    /// inside the out-of-date arm is structurally real.
    acquire_out_of_date_armed: Cell<bool>,
    /// CA-13's one-shot acquire DEVICE-LOSS injection (F13/A14's
    /// verification knob — the `VK_ERROR_DEVICE_LOST` acquire path,
    /// unreachable from the test API otherwise): consumed at the
    /// FIRST acquire site of the next frame, where the generic error
    /// arm converts it through [`RendererError::vk`] into the TYPED
    /// [`RendererError::DeviceLost`] the renderer's latch consumes.
    /// Diagnostic-only, test-only.
    acquire_device_lost_armed: Cell<bool>,
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
    /// P5 R0 (§9.4, the attach/upload census): the one-shot staging
    /// submission count and its accumulated blocking-wait time (see
    /// `crate::AllocationStats::upload_submits` /
    /// `upload_blocking_wait_ns` — the field docs are the contract).
    /// Interior-mutable like every counter here: the upload helpers
    /// take `&VulkanContext`.
    upload_submits: Cell<u64>,
    upload_blocking_wait_ns: Cell<u64>,
    /// P5 R0 (§9.4): the throwaway staging-pair count
    /// (`staged_buffer_uploads`).
    staged_buffer_uploads: Cell<u64>,
    /// P5 R0 (§9.4): the permanent per-texture staging tail bytes
    /// (`texture_staging_tail_bytes`).
    texture_staging_tail_bytes: Cell<u64>,
    /// P5 R3 (A20, design §4.3): the device-lifetime STAGING POOL —
    /// one shared, persistently mapped, power-of-two-grown backing
    /// replacing every per-object staging pair and the per-texture
    /// tails. Interior-mutable like every counter here (the callers
    /// hold `&VulkanContext`); confined to the renderer's single
    /// owning thread (the mapped pointer makes it `!Send`/`!Sync`).
    /// See [`super::staging`] — the module doc is the design.
    pub(super) staging: RefCell<StagingPool>,
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
            skinned_retained: None,
            texture: Default::default(),
            samplers: Default::default(),
            descriptors: Default::default(),
            memory_allocations: Cell::new(0),
            texture_allocations: Cell::new(0),
            descriptor_pools: Cell::new(0),
            descriptor_sets: Cell::new(0),
            sampler_creations: Cell::new(0),
            indirect_draws_issued: Cell::new(0),
            arena_growth_events: Cell::new(0),
            target_fault: Cell::new(None),
            lifecycle_fault: Cell::new(None),
            corner_pipeline_objects: Cell::new(0),
            corner_pipeline_object_frees: Cell::new(0),
            arena_replacements: Cell::new(0),
            arena_replacement_frees: Cell::new(0),
            acquire_out_of_date_armed: Cell::new(false),
            acquire_device_lost_armed: Cell::new(false),
            target_images: Cell::new(0),
            target_image_views: Cell::new(0),
            target_framebuffers: Cell::new(0),
            target_memories: Cell::new(0),
            target_image_frees: Cell::new(0),
            target_view_frees: Cell::new(0),
            target_framebuffer_frees: Cell::new(0),
            target_memory_frees: Cell::new(0),
            upload_submits: Cell::new(0),
            upload_blocking_wait_ns: Cell::new(0),
            staged_buffer_uploads: Cell::new(0),
            texture_staging_tail_bytes: Cell::new(0),
            staging: RefCell::new(StagingPool::new()),
        };
        if let Err(e) = context.init_atlas(atlas) {
            context.destroy();
            return Err(e);
        }
        // The corner content NO LONGER uploads here (P4 S2, completed
        // at S6): construction builds NO scene resources — every
        // description (the corner's included) reaches the context
        // through `VulkanContext::attach_scene_resources` behind the
        // `SceneDescription` handle, called by
        // `Renderer::attach_scene` on the consumer's side of the
        // boundary.
        Ok((context, validation_active, sync_validation))
    }

    /// Attaches the scene's device-lifetime resources (P4 S2 — the
    /// design's §3.2 attach step): builds the corner resource set
    /// from a VALIDATED description (the caller validates whole,
    /// device-free, first — a refusal builds nothing). ONE scene
    /// per context: the device-lifetime pass families (the
    /// retained skinned state, the shadow/reflection per-material
    /// sets) bind these buffers, so a second attachment would
    /// orphan them — refused here, and a fresh renderer is the
    /// supported path for different content. A failed build
    /// changes nothing (the builder's partial-failure discipline
    /// releases its own partials; `corner` stays `None`). Since
    /// CA-01 the INSTALL is itself a STAGING step: the caller
    /// (`Renderer::attach_scene`) completes the attach by building
    /// the dependent corner pipeline group, and rolls back through
    /// [`Self::release_staged_scene_resources`] if THAT fails — so
    /// the one-scene budget is consumed only by a fully-completed
    /// attach.
    pub(crate) fn attach_scene_resources(
        &mut self,
        description: &crate::scene_description::SceneDescription,
    ) -> Result<(), RendererError> {
        if self.corner.is_some() {
            return Err(RendererError::Vulkan(
                "a scene is already attached (one scene per renderer — \
                 construct a fresh renderer to attach different content)"
                    .into(),
            ));
        }
        let resources = corner::CornerResources::new(self, description)?;
        self.corner = Some(resources);
        Ok(())
    }

    /// CA-01 (attach atomicity): releases the STAGED scene resources
    /// — the set `attach_scene_resources` installed while the attach
    /// was still assembling its dependent pipeline group. Called
    /// only from [`crate::Renderer::attach_scene`]'s rollback guard
    /// when the corner-pipeline ensure fails: the context returns to
    /// exactly its pre-attach state (no installed resources, the
    /// one-scene budget unconsumed), so a retry of the SAME
    /// description is as clean as a first attach. This is NOT a
    /// detach API for attached scenes — a scene whose pipelines
    /// committed stays for the renderer's lifetime (the pass
    /// families bind its buffers); only a never-completed attach's
    /// staging is releasable.
    ///
    /// P5 R3 (A20): the attach's staging batch may be IN FLIGHT here
    /// (the build's deferred flush; the pipeline-group failure this
    /// rollback serves happens after it) — the batch's copies write
    /// the very images and buffers being released, so the pool
    /// drains FIRST (the actual resource dependency). A drain that
    /// times out (a stuck submission) falls back to
    /// `device_wait_idle` before the destroys — the releases must
    /// proceed (the rollback is infallible by contract; the ORIGINAL
    /// error propagates, never this best-effort cleanup's).
    pub(crate) fn release_staged_scene_resources(&mut self) {
        // P5 R3: the in-flight check first (the Ref borrow must end
        // before the drain's mutable borrow begins).
        let staging_in_flight = self.staging.borrow().has_in_flight();
        if staging_in_flight && self.drain_staging().is_err() {
            // SAFETY: the rollback's last resort — the stuck fence
            // cannot be waited, so the whole device idles before any
            // destroy (teardown's own protocol).
            unsafe { self.device.device_wait_idle() }.ok();
        }
        if let Some(resources) = self.corner.take() {
            resources.destroy(self);
        }
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
    pub(super) unsafe fn allocate_memory(
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
            upload_submits: self.upload_submits.get(),
            upload_blocking_wait_ns: self.upload_blocking_wait_ns.get(),
            staged_buffer_uploads: self.staged_buffer_uploads.get(),
            texture_staging_tail_bytes: self.texture_staging_tail_bytes.get(),
            staging_pool_bytes: self.staging.borrow().capacity_bytes(),
            staging_pool_growth_events: self.staging.borrow().growth_events(),
            staging_bound_region_bytes: self.staging.borrow().bound_region_bytes(),
            corner_pipeline_objects: self.corner_pipeline_objects.get(),
            corner_pipeline_object_frees: self.corner_pipeline_object_frees.get(),
            arena_replacements: self.arena_replacements.get(),
            arena_replacement_frees: self.arena_replacement_frees.get(),
        }
    }

    /// P5 R0 (§9.4): records one COMPLETED one-shot staging
    /// submission (the `with_upload_commands` helper's whole
    /// lifetime — pool, buffer, fence, submit, blocking wait,
    /// destruction) together with the wall time spent blocked in its
    /// `wait_for_fences`. Called only after the wait succeeded, so a
    /// failed upload counts neither the submit nor its partial wait
    /// (fail-visible, never overcounted). Since P5 R3 the SAME census
    /// carries the pooled batches: one submit per flush (inline or
    /// deferred — the deferred arm's blocking time lands here
    /// through [`Self::record_upload_wait`] at the drain that waits
    /// it).
    pub(super) fn record_upload_submit(&self, blocking_wait: std::time::Duration) {
        self.upload_submits.set(self.upload_submits.get() + 1);
        self.record_upload_wait(blocking_wait);
    }

    /// P5 R3 (§9.4): the wait-time half of the upload census on its
    /// own — the deferred flush counts its submit at submission and
    /// its blocking time only when the drain actually waits the
    /// fence (never overcounted, never lost).
    pub(super) fn record_upload_wait(&self, blocking_wait: std::time::Duration) {
        let ns = u64::try_from(blocking_wait.as_nanos()).unwrap_or(u64::MAX);
        self.upload_blocking_wait_ns
            .set(self.upload_blocking_wait_ns.get().saturating_add(ns));
    }

    /// P5 R0 (§9.4): records one device-local buffer upload staged
    /// through a throwaway HOST_VISIBLE pair (the
    /// `create_device_local_buffer_with_data` helper — one staging
    /// buffer plus one staging memory created and destroyed per
    /// call). Called only after the upload succeeded.
    pub(super) fn record_staged_buffer_upload(&self) {
        self.staged_buffer_uploads
            .set(self.staged_buffer_uploads.get() + 1);
    }

    /// P5 R0 (§9.4): records one texture's PERMANENT staging tail —
    /// the bytes of the texture's single device-local allocation
    /// beyond the image's own memory requirement (staging buffer +
    /// alignment padding), resident for the texture's lifetime.
    /// Called only after the allocation succeeded.
    pub(super) fn record_texture_staging_tail(&self, tail_bytes: u64) {
        self.texture_staging_tail_bytes.set(
            self.texture_staging_tail_bytes
                .get()
                .saturating_add(tail_bytes),
        );
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

    /// Arms a one-shot lifecycle fault plan (the CA-01/CA-10/CA-11
    /// verification knobs — the `target_fault` discipline on the
    /// lifecycle site enum). The `ordinal` is 1-based and decrements
    /// per MATCHING check across the renderer's lifetime; the plan
    /// is consumed when the ordinal reaches 0. Diagnostic-only,
    /// test-only: never produced by real device calls.
    pub(crate) fn set_lifecycle_fault(&self, site: crate::LifecycleFaultSite, ordinal: u32) {
        // A zero ordinal can never fire (it would underflow below);
        // clamp to 1 so an armed plan is always fireable by its
        // first matching check — the target-fault seam's own rule.
        self.lifecycle_fault.set(Some((site, ordinal.max(1))));
    }

    /// Checks a lifecycle fault plan for the named step. Identical
    /// semantics to [`Self::check_target_fault`]: `Ok(())` when no
    /// plan matches; on match the ordinal decrements and the fault
    /// fires (consuming the plan) at zero. The advance logic lives
    /// in [`super::resources::advance_lifecycle_fault_plan`] (pure,
    /// unit-tested without a device — the mock-free discipline).
    pub(crate) fn check_lifecycle_fault(
        &self,
        site: crate::LifecycleFaultSite,
    ) -> Result<(), RendererError> {
        let mut plan = self.lifecycle_fault.get();
        match super::resources::advance_lifecycle_fault_plan(&mut plan, site) {
            Some(error) => {
                self.lifecycle_fault.set(plan);
                Err(error)
            }
            None => {
                self.lifecycle_fault.set(plan);
                Ok(())
            }
        }
    }

    /// CA-01: records the creation of one corner pipeline group
    /// object (the group's staged layouts and pipelines — the
    /// census pair's creation half).
    pub(super) fn record_corner_pipeline_object(&self) {
        self.corner_pipeline_objects
            .set(self.corner_pipeline_objects.get() + 1);
    }

    /// CA-01: records the scoped owner's release of one corner
    /// pipeline group object (the census pair's destruction half).
    pub(super) fn record_corner_pipeline_object_free(&self) {
        self.corner_pipeline_object_frees
            .set(self.corner_pipeline_object_frees.get() + 1);
    }

    /// CA-11: records one staged arena-carve replacement (the scoped
    /// mapped-buffer owner's creation half).
    pub(super) fn record_arena_replacement_staged(&self) {
        self.arena_replacements
            .set(self.arena_replacements.get() + 1);
    }

    /// CA-11: records the scoped owner's release of one staged
    /// arena-carve replacement (the destruction half).
    pub(super) fn record_arena_replacement_released(&self) {
        self.arena_replacement_frees
            .set(self.arena_replacement_frees.get() + 1);
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

    /// Arms the one-shot acquire DEVICE-LOSS injection (CA-13's
    /// verification knob — F13/A14: the `VK_ERROR_DEVICE_LOST`
    /// acquire path, unreachable from the test API otherwise; real
    /// device removal stays a separate validation question, the
    /// audit's own boundary). Diagnostic-only, test-only: consumed
    /// at the FIRST acquire site of the next frame, where the
    /// generic error arm converts it through [`RendererError::vk`]
    /// into the typed [`RendererError::DeviceLost`] — the faithful
    /// driver contract (DEVICE_LOST means no image was acquired),
    /// the same argument as the out-of-date knob.
    pub(crate) fn set_acquire_device_lost(&self) {
        self.acquire_device_lost_armed.set(true);
    }

    /// Returns and disarms the one-shot acquire device-loss plan.
    /// `true` exactly once after `set_acquire_device_lost`, then
    /// `false` until the next arm.
    pub(crate) fn take_acquire_device_lost(&self) -> bool {
        self.acquire_device_lost_armed.replace(false)
    }

    /// CA-12 (F12): the pose arena's binding-5 descriptor facts,
    /// read once at construction for the shared representable-
    /// capacity validation
    /// ([`crate::config::ArenaBudgets::validate`]): the
    /// `STORAGE_BUFFER_DYNAMIC` range ceiling
    /// (`maxStorageBufferRange`) and offset-alignment requirement
    /// (`minStorageBufferOffsetAlignment`) the pose carve's per-slot
    /// stride must fit. SAFETY: querying the live physical device
    /// this context was built on (the corner's own build does the
    /// same).
    pub(crate) fn pose_binding_limits(&self) -> (u64, u64) {
        let limits = unsafe {
            self.instance
                .get_physical_device_properties(self.physical_device)
        }
        .limits;
        (
            u64::from(limits.max_storage_buffer_range),
            limits.min_storage_buffer_offset_alignment.max(1),
        )
    }

    /// Create a target image, incrementing the creation counter on
    /// success. The caller's `what` string is used for error
    /// reporting. SAFETY: callers pass valid image create info for
    /// this device and own the returned image.
    pub(super) unsafe fn create_target_image(
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
    pub(super) unsafe fn create_target_image_view(
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
    pub(super) unsafe fn create_target_framebuffer(
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
    pub(super) unsafe fn allocate_target_memory(
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
    pub(super) unsafe fn destroy_target_image(&self, image: vk::Image) {
        unsafe { self.device.destroy_image(image, None) };
        self.target_image_frees
            .set(self.target_image_frees.get() + 1);
    }

    /// Destroy a target image view, incrementing the destruction
    /// counter.
    pub(super) unsafe fn destroy_target_image_view(&self, view: vk::ImageView) {
        unsafe { self.device.destroy_image_view(view, None) };
        self.target_view_frees.set(self.target_view_frees.get() + 1);
    }

    /// Destroy a target framebuffer, incrementing the destruction
    /// counter.
    pub(super) unsafe fn destroy_target_framebuffer(&self, framebuffer: vk::Framebuffer) {
        unsafe { self.device.destroy_framebuffer(framebuffer, None) };
        self.target_framebuffer_frees
            .set(self.target_framebuffer_frees.get() + 1);
    }

    /// Free target memory, incrementing the destruction counter.
    pub(super) unsafe fn free_target_memory(&self, memory: vk::DeviceMemory) {
        unsafe { self.device.free_memory(memory, None) };
        self.target_memory_frees
            .set(self.target_memory_frees.get() + 1);
    }

    /// Allocates the skinned family's PASS-INDEPENDENT resources on
    /// the first populated frame EVER (the same lazy discipline as
    /// the pipeline states, one level up in ownership: the state
    /// lives HERE, at device lifetime, so every pipeline generation
    /// inherits it and a replacement never re-creates it). The
    /// unpopulated path pays nothing.
    ///
    /// Since P5 §1.3 (A11/R1) the pose carve is DEMAND-SIZED under
    /// the configured cap: the first populated frame carves
    /// `max(64, first-demand)` blocks (pow2, clamped to the cap),
    /// and a later frame whose pose-block supply exceeds the carve
    /// (still under the cap) grows it once through
    /// [`SkinnedRetained::ensure_capacity`] — the device-lifetime
    /// tier's `device_wait_idle` re-ensure, counted on the arena
    /// counter. `required` is this frame's block demand (the frame
    /// plan's `pose_demand_blocks`).
    pub(crate) fn ensure_skinned_retained(
        &mut self,
        required_blocks: u32,
        cap: u32,
    ) -> Result<(), RendererError> {
        // The growth dispatch: take the retained state out so the
        // growth path can borrow the context (its device, its
        // counters) beside the &mut state — put back on BOTH arms
        // (the growth path is failure-atomic: it creates the new
        // buffer before the old one dies, so an error leaves the
        // prior state exactly as it was).
        if let Some(mut retained) = self.skinned_retained.take() {
            let target =
                crate::config::arena_capacity_for(retained.blocks_capacity(), required_blocks, cap);
            let result = retained.ensure_capacity(self, target);
            self.skinned_retained = Some(retained);
            return result;
        }
        let corner = self.corner.as_ref().ok_or_else(|| {
            RendererError::Vulkan("skinned proxy requires an attached scene".into())
        })?;
        // The carve's first demand: the quantum's floor applies (an
        // empty pose supply still carves the floor so the binding-5
        // vertex input always has a live buffer — a populated frame
        // with no npc blocks yet draws zero npc instances, never
        // binds a null buffer). The creation floor is 1 block: a
        // degenerate zero-cap config refuses every non-empty pose
        // supply at the CAP while the empty supply keeps a valid
        // (704 B, 16-aligned) buffer — a zero-sized VkBuffer is
        // spec-invalid, never created. (CA-12: `cap` is the
        // construction-VALIDATED u32 — device-checked against the
        // binding-5 range/alignment at `Renderer::new`.)
        let capacity = crate::config::arena_capacity_for(0, required_blocks.max(1), cap).max(1);
        let retained = skinned_draw::SkinnedRetained::new(self, corner, capacity)?;
        self.skinned_retained = Some(retained);
        Ok(())
    }

    /// P5 §1.6 (A11/R1): records one arena growth event (a carve
    /// RE-CREATION at a larger size — see the field's doc). Called
    /// by the three growth paths only, after the replacement
    /// committed.
    pub(crate) fn record_arena_growth(&self) {
        self.arena_growth_events
            .set(self.arena_growth_events.get() + 1);
    }

    /// Full teardown in manifest order (assumes swapchain/pipelines/frames
    /// were already destroyed by the owning Renderer).
    pub(crate) fn destroy(&mut self) {
        // The retained skinned state FIRST (reverse creation order:
        // the corner exists from construction, the retained state
        // from the first populated frame). Valid because every
        // PipelineState — whose skinned pipelines read these objects
        // — was already destroyed by the owning Renderer
        // (Renderer::drop: frames → swapchain → pipes.destroy →
        // ctx.destroy).
        if let Some(mut retained) = self.skinned_retained.take() {
            retained.release(self);
        }
        if let Some(corner) = self.corner.take() {
            corner.destroy(self);
        }
        std::mem::take(&mut self.descriptors).destroy(self);
        std::mem::take(&mut self.samplers).destroy(self);
        std::mem::take(&mut self.texture).destroy(self);
        // The staging pool LAST of the resource owners (reverse
        // creation order: its backing may carry the atlas/attach
        // batches staged through it, and its teardown idles the
        // device first when anything is still in flight — the
        // stuck-fence protocol; see [`super::staging`]). Valid after
        // every state above: no pool region outlives its batch.
        // Taken out of the context first so the pool's `&mut` and
        // the context's `&` never alias.
        let mut staging = std::mem::take(&mut self.staging);
        staging.get_mut().destroy(self);
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
        // THE OFFSCREEN-SURFACE PARITY (the local-lavapipe
        // corrective): SDL's offscreen driver advertises
        // VK_EXT_headless_surface itself on Linux, but on Windows
        // it names no window-system surface at all - starving the
        // instance of the only surface path an offscreen window
        // can use (the surface path's offscreen branch then fails
        // with "VK_EXT_headless_surface extension is not enabled").
        //
        // THE SIGN-OFF REVIEW'S FINDING 23 CORRECTIVE, two parts:
        // (1) THE PREREQUISITE - VK_EXT_headless_surface DEPENDS on
        // VK_KHR_surface (the instance-extension dependency in its
        // specification): enabling headless alone produces an
        // invalid extension list the validation layer flags and a
        // lenient driver might still accept - the plan now enables
        // BOTH, each exactly once, whenever it adds the headless
        // one, and adds NEITHER when the loader cannot supply the
        // prerequisite (a loader advertising headless without
        // KHR_surface is itself malformed - the honest behavior is
        // the SDL list untouched, the surface path failing visibly
        // at creation, never a spec-invalid instance).
        // (2) THE PLATFORM CONDITION - the pre-corrective substring
        // test (`e.contains("surface")`) let SDL's GENERIC
        // VK_KHR_surface alone satisfy it, blocking the fallback
        // for a list that names no PLATFORM surface (no
        // win32/metal/xlib/wayland/android/headless backend) - the
        // driver-less generic name cannot create any window
        // surface. The condition now requires an actual platform
        // backend extension; a windowed SDL list is never touched,
        // and a real-GPU loader that lacks the headless extension
        // is unchanged.
        // The correct extension for macOS is VK_EXT_metal_surface.
        // VK_KHR_metal_surface does not exist (removed by the P1-B09 fix).
        // VK_MVK_macos_surface is deprecated and superseded by
        // VK_EXT_metal_surface (2020-07-31), but it IS a real extension
        // name legacy MoltenVK loaders advertise — it stays in the list so
        // a legacy windowed macOS list is still classified as having a
        // platform surface and never receives the headless fallback.
        // VK_KHR_display_surface does not exist; display WSI is
        // VK_KHR_display (which provides vkCreateDisplayPlaneSurfaceKHR),
        // a different name no SDL list carries, so no entry is needed here.
        const PLATFORM_SURFACE_EXTENSIONS: &[&str] = &[
            "VK_KHR_win32_surface",
            "VK_KHR_xlib_surface",
            "VK_KHR_xcb_surface",
            "VK_KHR_wayland_surface",
            "VK_KHR_android_surface",
            "VK_EXT_metal_surface",
            "VK_EXT_headless_surface",
            "VK_MVK_macos_surface",
            "VK_GGP_stream_descriptor_surface",
            "VK_NN_vi_surface",
            "VK_FUCHSIA_imagepipe_surface",
            "VK_QNX_screen_surface",
        ];
        let headless = "VK_EXT_headless_surface".to_string();
        let khr_surface = "VK_KHR_surface".to_string();
        let mut required: Vec<String> = sdl_required.to_vec();
        let has_platform_surface = required
            .iter()
            .any(|e| PLATFORM_SURFACE_EXTENSIONS.contains(&e.as_str()));
        let can_supply_prerequisite =
            required.contains(&khr_surface) || loader_advertised.contains(&khr_surface);
        if !has_platform_surface && loader_advertised.contains(&headless) && can_supply_prerequisite
        {
            if !required.contains(&khr_surface) {
                required.push(khr_surface);
            }
            if !required.contains(&headless) {
                required.push(headless);
            }
        }
        InstanceExtensionPlan {
            sdl_required: dedup_keep_first(&required),
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

/// Safe-to-call device idle used by the renderer's ordered teardown.
pub(crate) fn device_idle(ctx: &VulkanContext) -> Result<(), RendererError> {
    unsafe { ctx.device.device_wait_idle() }
        .map_err(|error| RendererError::vk(error, "device_wait_idle"))
}

#[cfg(test)]
mod policy_tests {
    use super::*;

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
    fn offscreen_sdl_without_a_surface_gets_headless_from_the_loader() {
        // THE WINDOWS OFFSCREEN PARITY: SDL's offscreen driver on
        // Windows names no window-system surface; the loader
        // (lavapipe) offers the headless one - the plan must add
        // BOTH it and its VK_KHR_surface prerequisite, each exactly
        // once (the sign-off review's finding 23 corrective: the
        // pre-corrective plan enabled headless alone - an invalid
        // extension list under the extension's own dependency).
        let plan = InstanceExtensionPlan::select(
            &[],
            &strings(&["VK_KHR_surface", "VK_EXT_headless_surface"]),
            false,
        );
        assert_eq!(
            plan.extensions(),
            strings(&["VK_KHR_surface", "VK_EXT_headless_surface"])
        );
        // A windowed SDL list is never touched.
        let windowed = InstanceExtensionPlan::select(
            &strings(&["VK_KHR_surface", "VK_KHR_win32_surface"]),
            &strings(&["VK_KHR_surface", "VK_EXT_headless_surface"]),
            false,
        );
        assert_eq!(
            windowed.extensions(),
            strings(&["VK_KHR_surface", "VK_KHR_win32_surface"])
        );
        // No loader offer, no addition (the real-GPU offscreen case
        // is unchanged - it still fails at the surface path, as
        // before).
        let gpu = InstanceExtensionPlan::select(&[], &strings(&["VK_KHR_surface"]), false);
        assert!(gpu.extensions().is_empty());
        // THE GENERIC-ONLY LIST (finding 23's second half): SDL
        // naming just the base VK_KHR_surface carries no platform
        // backend - the pre-corrective substring test let it block
        // the fallback; the platform condition now adds the
        // headless backend beside it.
        let generic_only = InstanceExtensionPlan::select(
            &strings(&["VK_KHR_surface"]),
            &strings(&["VK_KHR_surface", "VK_EXT_headless_surface"]),
            false,
        );
        assert_eq!(
            generic_only.extensions(),
            strings(&["VK_KHR_surface", "VK_EXT_headless_surface"]),
            "the generic-only list gets the headless backend (no platform surface named)"
        );
        // A list that already carries headless is untouched (no
        // duplicate, the prerequisite already implied).
        let already_headless = InstanceExtensionPlan::select(
            &strings(&["VK_KHR_surface", "VK_EXT_headless_surface"]),
            &strings(&["VK_KHR_surface", "VK_EXT_headless_surface"]),
            false,
        );
        assert_eq!(
            already_headless.extensions(),
            strings(&["VK_KHR_surface", "VK_EXT_headless_surface"])
        );
        // THE LOADER MISSING THE PREREQUISITE: a loader advertising
        // headless without KHR_surface is itself malformed - the
        // plan adds NEITHER (the SDL list untouched, the surface
        // path failing visibly at creation, never a spec-invalid
        // instance).
        let malformed_loader =
            InstanceExtensionPlan::select(&[], &strings(&["VK_EXT_headless_surface"]), false);
        assert!(
            malformed_loader.extensions().is_empty(),
            "no prerequisite, no headless - never an invalid extension list"
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
    fn macos_windowed_sdl_list_with_metal_surface_does_not_get_headless_fallback() {
        // P1-B09 corrective test: a macOS windowed SDL list advertising
        // VK_EXT_metal_surface must NOT get VK_EXT_headless_surface added,
        // even if the loader offers it. The metal surface extension is a
        // platform surface, so has_platform_surface is true and the
        // headless fallback is blocked. This test specifically validates
        // the fix for the wrong "VK_KHR_metal_surface" literal that was
        // misclassifying macOS windowed lists as headless candidates.
        let plan = InstanceExtensionPlan::select(
            &strings(&["VK_KHR_surface", "VK_EXT_metal_surface"]),
            &strings(&["VK_KHR_surface", "VK_EXT_headless_surface"]),
            false,
        );
        // The SDL list is untouched: metal surface is recognized, so no
        // headless fallback is added.
        assert_eq!(
            plan.extensions(),
            strings(&["VK_KHR_surface", "VK_EXT_metal_surface"])
        );
        // Verify the loader offer did not trigger the fallback.
        assert!(
            !plan
                .extensions()
                .contains(&"VK_EXT_headless_surface".to_string())
        );
    }
}
