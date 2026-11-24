//! Safe public renderer API (Stage 2). All Vulkan unsafe lives in
//! [`crate::vulkan`]; this module owns the four Vulkan states, exposes the
//! safe surface, and collects instrumentation.

use ash::vk;
use engine_core::timing::TimingSamples;
use std::time::Duration;

use sdl3::video::Window as SdlWindow;

use crate::config::{RenderConfig, TargetFaultSite, TargetFaultStage, present_mode_str};
use crate::overlay::{AtlasText, OverlayVertex};
use crate::scene::SceneVertex;
use crate::vulkan::{
    self, CapturedFrame, FrameState, PipelineState, RendererError, SwapchainState, VulkanContext,
    drain_validation_messages, environment_pending, pick_depth_format, pick_surface_format,
};

/// One captured shadow-map readback (the 6-F known-answer path): the
/// depth-only pass's map, one f32 per texel, row-major with row 0
/// first, at the map's square extent. The depths are UNBIASED
/// light-space values in [0, 1] (the compare-time analytical bias is
/// applied at lookup, never written) — exactly the CPU raycaster's
/// `t / (far − near)`.
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowMapReadback {
    /// The map's square texel extent.
    pub extent: u32,
    /// `extent × extent` depths, row-major, row 0 first.
    pub depths: Vec<f32>,
}

/// A batch of triangles with its own transform into clip space. A
/// foreground batch typically supplies camera-local vertices and a projection.
#[derive(Debug, Clone, Copy)]
pub struct SceneLayer<'a> {
    pub vertices: &'a [SceneVertex],
    pub view_proj: &'a [[f32; 4]; 4],
}

/// One proxy population for a populated corner frame: the NPC and
/// vehicle instance lists plus the per-kind material indices into the
/// corner scene's material table (which specular/shininess set each
/// kind draws with — CALLER policy, not engine policy; the demo and
/// the equivalence gate document their choices). The renderer packs
/// both lists into its retained instance buffer in the fixed
/// npc-then-vehicle order.
#[derive(Debug, Clone, Copy)]
pub struct ProxyPopulation<'a> {
    pub npc: &'a [crate::proxy_instances::ProxyInstance],
    pub npc_material: u32,
    pub vehicle: &'a [crate::proxy_instances::ProxyInstance],
    pub vehicle_material: u32,
}

/// How proxy instances are submitted (the 6-E equivalence gate's
/// axis): `Instanced` — one draw per populated proxy kind (the
/// production path, the package's headline); `PerInstance` — one
/// draw per INSTANCE through the same shader, bytes, and descriptor
/// sets (the pre-6-E per-object submission shape, kept as the
/// N-draws reference for the pixel-equivalence gate and the
/// before/after draw-call evidence — production paths never set it);
/// `Indirect` — the 6-E2 mechanism: one `vkCmdDrawIndirect` per
/// populated kind with the instance count read from GPU memory at
/// draw time (the packed [`crate::proxy_indirect`] command block),
/// byte-identical output to `Instanced` — the submission stops
/// carrying the count so a later GPU pass can author it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProxyDrawMode {
    #[default]
    Instanced,
    PerInstance,
    Indirect,
}

/// Actual dynamic uploads and draws recorded in this frame.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GeometryStats {
    /// World plus foreground vertices.
    pub scene_drawn: u32,
    /// Portion of `scene_drawn` used by the foreground.
    pub foreground_drawn: u32,
    /// HUD plus diagnostic vertices.
    pub overlay_drawn: u32,
    pub scene_omitted: u32,
    pub overlay_omitted: u32,
    pub dynamic_draw_calls: u32,
    /// Proxy draw calls recorded this frame (the 6-E headline: one
    /// per populated kind in [`ProxyDrawMode::Instanced`] and
    /// [`ProxyDrawMode::Indirect`] alike — the GPU-side count
    /// changes where N lives, never the draw-call count — and one
    /// per instance in [`ProxyDrawMode::PerInstance`]). Counts proxy
    /// submission only — the corner mesh's own material draws are
    /// not included. Zero on unpopulated frames.
    pub proxy_draw_calls: u32,
    pub uploaded_bytes: u64,
}

/// Per-frame result surfaced to the application.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FrameResult {
    pub presented: bool,
    pub recreated: bool,
    pub suboptimal: bool,
    /// The drawable was temporarily unavailable (zero-sized surface
    /// capabilities — minimized or mid-transition): nothing happened;
    /// poll and try again next frame.
    pub deferred: bool,
    pub gpu_render_ms: Option<f64>,
    pub geometry: GeometryStats,
}

/// Successful Vulkan allocation calls since renderer construction and the
/// resident byte size of the texture allocations (including staging tails).
/// The additional fields count only the Era-1 target builders' Vulkan
/// objects (image, memory, view, framebuffer — post chain, wet chain,
/// shadow chain). Texture/corner/overlay allocations stay outside this
/// scope; the existing fields' semantics are unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocationStats {
    pub device_memory: u64,
    pub texture_memory: u64,
    pub texture_bytes: u64,
    pub descriptor_pools: u64,
    pub descriptor_sets: u64,
    pub samplers: u64,
    /// Monotonic creation count of Era-1 target builder images
    /// (post, wet, shadow).
    pub target_images: u64,
    /// Monotonic creation count of Era-1 target builder image views.
    pub target_image_views: u64,
    /// Monotonic creation count of Era-1 target builder framebuffers.
    pub target_framebuffers: u64,
    /// Monotonic creation count of Era-1 target builder memory allocations.
    pub target_memories: u64,
    /// Monotonic destruction count of Era-1 target builder images.
    pub target_image_frees: u64,
    /// Monotonic destruction count of Era-1 target builder image views.
    pub target_view_frees: u64,
    /// Monotonic destruction count of Era-1 target builder framebuffers.
    pub target_framebuffer_frees: u64,
    /// Monotonic destruction count of Era-1 target builder memory allocations.
    pub target_memory_frees: u64,
}

/// Long-lived renderer statistics (Stage 2 authorization, item 4).
///
/// These are instrumentation readings of the simple Stage 2 scene — they
/// are not claims about the first game's performance (TECH-ARCHITECTURE
/// §6, stage-table policy).
#[derive(Debug, Clone)]
pub struct RenderStats {
    pub gpu_name: String,
    pub device_type: String,
    pub api_version: String,
    pub present_mode: String,
    pub supported_present_modes: Vec<String>,
    pub surface_format: String,
    pub extent: (u32, u32),
    pub swapchain_images: u32,
    pub frames_presented: u64,
    pub swapchain_recreations: u64,
    pub validation_active: bool,
    /// Synchronization validation requested-and-accepted on the
    /// validation instance (finding 9's explicit enablement: the
    /// VK_EXT_validation_features pNext chained when
    /// `ValidationPreference::On` and the layer accepted it — a
    /// failed attempt falls back with this false alongside
    /// `validation_active`). When true, the `validation_errors`
    /// count includes the layer's synchronization-hazard findings
    /// (SYNC-HAZARD-*), so every gate asserting
    /// `validation_errors == 0` now also proves the render passes'
    /// execution and memory dependencies.
    pub sync_validation: bool,
    pub gpu_timestamps_supported: bool,
    pub validation_errors: u64,
    pub validation_warnings: u64,
    /// Last validation messages (bounded), most recent last.
    pub recent_validation: Vec<String>,
    /// F-03 Step 1 (fail-visible over fail-silent): cumulative
    /// dynamic-geometry vertices omitted because a frame's supply
    /// exceeded the frame slot's buffer capacity, with the peak
    /// supplied and the per-slot capacity for context. Zero in every
    /// ordinary run (the H3 proof); nonzero is REPORTED in the run
    /// summary, never fatal — the capacity/refusal policy is Step 2,
    /// separately authorized. The region capacity reads 0 until the
    /// lazy region buffer allocates (no region frames).
    pub region_truncated_vertices: u64,
    pub region_supplied_peak_vertices: u32,
    pub region_capacity_vertices: u32,
    pub overlay_truncated_vertices: u64,
    pub overlay_supplied_peak_vertices: u32,
    pub overlay_capacity_vertices: u32,
    gpu_timing: TimingSamples,
    /// The negotiated depth format (corrective finding 4, F):
    /// the renderer's chosen depth format, reported so tests can
    /// verify fallbacks and the DepthFormatPreference knob.
    pub depth_format: String,
}

impl RenderStats {
    /// GPU render time (timestamp-derived), millisecond percentiles.
    pub fn gpu_ms_p50(&self) -> f64 {
        self.gpu_timing.percentile_ms(0.50)
    }

    pub fn gpu_ms_p95(&self) -> f64 {
        self.gpu_timing.percentile_ms(0.95)
    }

    pub fn gpu_ms_p99(&self) -> f64 {
        self.gpu_timing.percentile_ms(0.99)
    }

    pub fn gpu_ms_max(&self) -> f64 {
        self.gpu_timing.max_ms()
    }

    pub fn gpu_sample_count(&self) -> usize {
        self.gpu_timing.sample_count()
    }
}

/// The Stage 2 Vulkan renderer. Rendering cadence is caller-driven — the
/// renderer imposes no frame-rate ceiling, and rendering is deliberately
/// independent of the future fixed-rate campaign simulation (Stage 3,
/// gated). Drop performs the ordered teardown documented in
/// [`crate::vulkan`].
pub struct Renderer {
    ctx: VulkanContext,
    swapchain: SwapchainState,
    pipes: PipelineState,
    frames: FrameState,
    config: RenderConfig,
    stats: RenderStats,
    overlay_enabled: bool,
    overlay_color: [f32; 4],
    /// The F-05 retained overlay-vertex buffer: rebuilt in place every
    /// frame (the capacity from the first frames is reused — no
    /// per-frame allocation or growth; the vertex content is
    /// identical to the allocating form).
    overlay_vertices: Vec<OverlayVertex>,
    atlas_text: AtlasText,
    /// The 6-E retained instance-byte buffer (the same F-05
    /// discipline, applied through
    /// [`crate::proxy_instances::repack_populations`]): repacked in
    /// place every populated frame; the capacity from the first
    /// populated frames is reused — no per-frame allocation or
    /// growth.
    instance_bytes: Vec<u8>,
    proxy_draw_mode: ProxyDrawMode,
    /// The Era-1 sky layer's sticky setting (the set_overlay_enabled
    /// precedent): `Some` makes every subsequent corner-family frame
    /// draw the sky pass first; `None` keeps the skyless byte-identical
    /// path.
    sky: Option<SkyFrame>,
    /// The Era-1 post chain's sticky setting (the same precedent):
    /// `true` makes every subsequent corner-family frame render
    /// through the HDR chain (the scene pass into the R16G16B16A16
    /// target, the bloom mips, the composite + tonemap into the
    /// swapchain); `false` keeps the postless byte-identical path.
    /// The region path (`render_layers`) stays postless regardless —
    /// the chain is a corner-family feature (the brief's reference
    /// scene), documented like the sky's scope.
    post: bool,
    /// The Era-1 wet-ground reflection's sticky setting: `true`
    /// makes every subsequent corner-family frame render the
    /// mirrored scene into an offscreen target and re-draw the
    /// ground Schlick-blended over it (planar.rs's matrix and
    /// blend); `false` keeps the dry byte-identical path. Carries
    /// the post state implicitly (the reflection pass rides its
    /// HDR render pass object) — the post CHAIN itself stays
    /// optional.
    wet: bool,
}

/// The Era-1 sky layer's frame description (the sticky `set_sky`
/// setting): the palette and the celestial disc's direction. The
/// renderer derives the pass's remaining input (the inverse
/// view-projection) per frame from the camera matrix it is already
/// given — the caller supplies nothing per frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyFrame {
    /// The gradient + disc palette (see [`crate::sky::SkyPalette`];
    /// the day/night presets and `atmosphere_at` provide them).
    pub palette: crate::sky::SkyPalette,
    /// The direction TOWARD the celestial body (the sun by day, the
    /// moon by night; the presets' light directions). The renderer
    /// normalizes it (the CPU core's contract).
    pub disc_dir: [f32; 3],
}

impl Renderer {
    pub fn new(window: &SdlWindow, config: RenderConfig) -> Result<Renderer, RendererError> {
        let atlas_text = AtlasText::default();
        let (mut ctx, validation_active, sync_validation) =
            VulkanContext::new(window, atlas_text.atlas(), &config)?;
        // Cross-stage cleanup (corrective finding G): each constructor
        // cleans up its own partials; this ladder releases completed
        // stages when a later stage fails.
        let (format, color_space) = match pick_surface_format(&ctx) {
            Ok(picked) => picked,
            Err(error) => {
                ctx.destroy();
                return Err(error);
            }
        };
        // The depth format is chosen once and shared by the render pass
        // and every depth attachment (corrective finding F).
        let depth_format =
            pick_depth_format(&ctx.instance, ctx.physical_device, config.depth_format);
        // The demo-scene preference is fixed at construction (P3): the
        // static Stage-2 demonstration scene uploads only on the
        // explicit opt-in, and every later pipeline replacement is
        // built with this same live preference.
        let mut pipes = match PipelineState::new(&ctx, format, depth_format, config.demo_scene) {
            Ok(pipes) => pipes,
            Err(error) => {
                ctx.destroy();
                return Err(error);
            }
        };
        let (width, height) = window.size_in_pixels();
        let mut swapchain = match SwapchainState::new(
            &ctx,
            (width, height),
            config.present_mode,
            format,
            color_space,
            pipes.render_pass,
            depth_format,
            vk::SwapchainKHR::null(),
        ) {
            Ok(swapchain) => swapchain,
            Err(error) => {
                pipes.destroy(&ctx);
                ctx.destroy();
                return Err(error);
            }
        };
        let frames = match FrameState::new(&ctx, swapchain.image_count) {
            Ok(frames) => frames,
            Err(error) => {
                swapchain.destroy(&ctx);
                pipes.destroy(&ctx);
                ctx.destroy();
                return Err(error);
            }
        };
        let stats = RenderStats {
            gpu_name: ctx.gpu_name.clone(),
            device_type: ctx.device_type.clone(),
            api_version: ctx.api_version.clone(),
            present_mode: present_mode_str(swapchain.present_mode).to_string(),
            supported_present_modes: ctx
                .supported_present_modes
                .iter()
                .map(|mode| present_mode_str(*mode).to_string())
                .collect(),
            surface_format: format!("{:?}", swapchain.format),
            extent: (swapchain.extent.width, swapchain.extent.height),
            swapchain_images: swapchain.image_count,
            frames_presented: 0,
            swapchain_recreations: 0,
            validation_active,
            sync_validation,
            gpu_timestamps_supported: frames.timestamps_supported,
            validation_errors: 0,
            validation_warnings: 0,
            recent_validation: Vec::new(),
            region_truncated_vertices: 0,
            region_supplied_peak_vertices: 0,
            region_capacity_vertices: 0,
            overlay_truncated_vertices: 0,
            overlay_supplied_peak_vertices: 0,
            overlay_capacity_vertices: 0,
            gpu_timing: TimingSamples::default(),
            depth_format: format!("{:?}", depth_format),
        };
        Ok(Renderer {
            ctx,
            swapchain,
            pipes,
            frames,
            config,
            stats,
            overlay_enabled: true,
            overlay_color: [0.72, 1.0, 0.78, 0.85],
            overlay_vertices: Vec::new(),
            atlas_text,
            instance_bytes: Vec::new(),
            proxy_draw_mode: ProxyDrawMode::default(),
            sky: None,
            post: false,
            wet: false,
        })
    }

    /// Renders one frame. `window_size` is the current window size in
    /// pixels — the application owns window state (the Stage 1 shell), and
    /// the renderer never queries SDL after construction. A minimized
    /// window (zero extent) presents nothing. `region_scene` is dynamic
    /// per-frame geometry (Stage 4 graphical integration: region area
    /// plates + NPC render proxies) drawn through the scene pipeline from
    /// a per-frame-slot persistently mapped buffer — the overlay's
    /// dynamic-update pattern; an empty slice draws nothing (the Stage
    /// 2/3 default path). `overlay_text` renders the debug overlay when
    /// overlays are enabled.
    pub fn render_frame(
        &mut self,
        view_proj: &[[f32; 4]; 4],
        region_scene: &[SceneVertex],
        overlay_text: Option<&str>,
        window_size: (u32, u32),
    ) -> Result<FrameResult, RendererError> {
        self.render_layers(
            view_proj,
            region_scene,
            None,
            &[],
            overlay_text,
            window_size,
        )
    }

    /// Draws the world, an optional foreground with independent depth,
    /// and one screen-space batch. World and foreground share the same
    /// scene slot, in that order. HUD and diagnostics share the overlay
    /// slot in that order. At capacity, foreground takes precedence over
    /// world geometry and HUD takes precedence over diagnostics. Omitted
    /// vertex counts are returned in `FrameResult::geometry`.
    /// All supplied batches are triangle lists (three
    /// vertices per primitive). The diagnostics toggle controls only the text.
    pub fn render_layers(
        &mut self,
        view_proj: &[[f32; 4]; 4],
        world: &[SceneVertex],
        foreground: Option<SceneLayer<'_>>,
        hud: &[OverlayVertex],
        debug_text: Option<&str>,
        window_size: (u32, u32),
    ) -> Result<FrameResult, RendererError> {
        // The F-05 overlay-vertex reuse (the audit's measured
        // candidate): the glyph vertices are rebuilt into the
        // renderer's own retained buffer — the capacity from the
        // first frames is reused every frame (no per-frame
        // allocation or growth; the vertex content is identical).
        if self.overlay_enabled {
            if let Some(text) = debug_text {
                self.atlas_text.build_overlay_vertices_into(
                    &mut self.overlay_vertices,
                    text,
                    window_size,
                    self.overlay_color,
                );
            } else {
                self.overlay_vertices.clear();
            }
        } else {
            self.overlay_vertices.clear();
        }
        let outcome = vulkan::render_frame(
            &mut self.ctx,
            &mut self.swapchain,
            &mut self.pipes,
            &mut self.frames,
            window_size,
            self.config.present_mode,
            self.config.replace_pipelines,
            self.config.demo_scene,
            view_proj,
            world,
            foreground,
            hud,
            &self.overlay_vertices,
            None,
            None,
            None,
            // The sky layer is a corner-family feature (the sticky
            // setting's documented scope); the general region path
            // stays skyless.
            None,
            // The post chain likewise (the region path stays
            // postless — the setting's documented scope), and the
            // wet reflection with it.
            false,
            None,
        )?;
        Ok(self.finish_frame(outcome))
    }

    /// Draws the resident corner mesh with this camera and preset.
    pub fn render_corner(
        &mut self,
        view_proj: &[[f32; 4]; 4],
        eye: [f32; 3],
        light: crate::lit_scene::LightParams,
        size: (u32, u32),
    ) -> Result<FrameResult, RendererError> {
        if self.config.demo_scene != crate::DemoScenePreference::Corner {
            return Err(RendererError::Vulkan(
                "render_corner requires the Corner scene preference".into(),
            ));
        }
        let params = vulkan::corner::FrameParams::new(*view_proj, eye, light);
        let sky = self.sky_params(view_proj);
        let wet = self.wet_camera(view_proj, eye, light);
        let outcome = vulkan::render_frame(
            &mut self.ctx,
            &mut self.swapchain,
            &mut self.pipes,
            &mut self.frames,
            size,
            self.config.present_mode,
            self.config.replace_pipelines,
            self.config.demo_scene,
            view_proj,
            &[],
            None,
            &[],
            &[],
            Some(&params),
            None,
            None,
            sky.as_ref(),
            self.post,
            wet.as_ref(),
        )?;
        Ok(self.finish_frame(outcome))
    }

    /// Draws the resident corner mesh PLUS a proxy population, the
    /// Cycle 6-E path: the instances are repacked into the retained
    /// buffer (zero per-frame allocation after the first populated
    /// frames) and submitted per [`ProxyDrawMode`] — one draw call
    /// per populated kind in the production `Instanced` mode and the
    /// 6-E2 `Indirect` mode alike (the latter reads the instance
    /// counts from the GPU-side command block — see
    /// [`crate::proxy_indirect`]). The
    /// population is validated BEFORE any frame state is touched: a
    /// typed refusal (over-capacity total, invalid field, or a
    /// material index outside the corner table) renders nothing and
    /// leaves the frame flow untouched.
    pub fn render_corner_populated(
        &mut self,
        view_proj: &[[f32; 4]; 4],
        eye: [f32; 3],
        light: crate::lit_scene::LightParams,
        population: &ProxyPopulation<'_>,
        size: (u32, u32),
    ) -> Result<FrameResult, RendererError> {
        if self.config.demo_scene != crate::DemoScenePreference::Corner {
            return Err(RendererError::Vulkan(
                "render_corner_populated requires the Corner scene preference".into(),
            ));
        }
        let material_table = crate::lit_scene::materials().len() as u32;
        if population.npc_material >= material_table
            || population.vehicle_material >= material_table
        {
            return Err(RendererError::Vulkan(format!(
                "proxy material index outside the corner material table \
                 ({} materials, npc {}, vehicle {})",
                material_table, population.npc_material, population.vehicle_material
            )));
        }
        crate::proxy_instances::repack_populations(
            &mut self.instance_bytes,
            population.npc,
            population.vehicle,
        )
        .map_err(RendererError::ProxyPack)?;
        let params = vulkan::corner::FrameParams::new(*view_proj, eye, light);
        let upload = vulkan::proxy_draw::ProxyFrameUpload {
            bytes: &self.instance_bytes,
            npc_count: population.npc.len() as u32,
            vehicle_count: population.vehicle.len() as u32,
            npc_material: population.npc_material,
            vehicle_material: population.vehicle_material,
            mode: self.proxy_draw_mode,
        };
        let sky = self.sky_params(view_proj);
        let wet = self.wet_camera(view_proj, eye, light);
        let outcome = vulkan::render_frame(
            &mut self.ctx,
            &mut self.swapchain,
            &mut self.pipes,
            &mut self.frames,
            size,
            self.config.present_mode,
            self.config.replace_pipelines,
            self.config.demo_scene,
            view_proj,
            &[],
            None,
            &[],
            &[],
            Some(&params),
            Some(&upload),
            None,
            sky.as_ref(),
            self.post,
            wet.as_ref(),
        )?;
        Ok(self.finish_frame(outcome))
    }

    /// Draws the corner scene with DIRECTIONAL SHADOWS (Cycle 6-F):
    /// one cascade computed from the light direction and the scene's
    /// world AABB (the deterministic core's `cascade_for`), rendered
    /// through the depth-only pass, then the main pass with the
    /// shadowed lit pipeline sampling the map (Era-1 PCF soft
    /// shadows — the 3×3 `shadow_filter` tap table over the validated
    /// map, the compare-time analytical bias shared verbatim with the
    /// CPU oracle). The unshadowed `render_corner`
    /// path is untouched — pixel-identical to the pre-6-F renderer.
    /// The bias parameters are CALLER policy (documented per caller;
    /// the shader and the CPU oracle share the same expression).
    pub fn render_corner_shadowed(
        &mut self,
        view_proj: &[[f32; 4]; 4],
        eye: [f32; 3],
        light: crate::lit_scene::LightParams,
        bias: crate::shadows::ShadowBias,
        size: (u32, u32),
    ) -> Result<FrameResult, RendererError> {
        if self.config.demo_scene != crate::DemoScenePreference::Corner {
            return Err(RendererError::Vulkan(
                "render_corner_shadowed requires the Corner scene preference".into(),
            ));
        }
        let boxes = crate::lit_scene::intersection_boxes();
        let (min, max) = crate::shadows::scene_bounds(&boxes);
        let cascade = crate::shadows::cascade_for(light.dir_toward_light, min, max);
        let [r, g, b] = light.ambient;
        let params = vulkan::shadow_draw::ShadowFrameParams {
            view_proj: *view_proj,
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
                bias.constant,
                bias.slope_scale,
                0.0,
            ],
        };
        let sky = self.sky_params(view_proj);
        let wet = self.wet_camera(view_proj, eye, light);
        // The ground re-draw (finding 5) needs the current camera in the
        // corner buffer regardless of the lighting branch. Build the
        // ordinary FrameParams and pass it in corner_params.
        let corner_params = vulkan::corner::FrameParams::new(*view_proj, eye, light);
        let outcome = vulkan::render_frame(
            &mut self.ctx,
            &mut self.swapchain,
            &mut self.pipes,
            &mut self.frames,
            size,
            self.config.present_mode,
            self.config.replace_pipelines,
            self.config.demo_scene,
            view_proj,
            &[],
            None,
            &[],
            &[],
            Some(&corner_params),
            None,
            Some(&params),
            sky.as_ref(),
            self.post,
            wet.as_ref(),
        )?;
        Ok(self.finish_frame(outcome))
    }

    /// Sets the proxy submission mode (the equivalence gate's axis —
    /// see [`ProxyDrawMode`]; the default is the production
    /// `Instanced` mode).
    pub fn set_proxy_draw_mode(&mut self, mode: ProxyDrawMode) {
        self.proxy_draw_mode = mode;
    }

    /// The current proxy submission mode.
    pub fn proxy_draw_mode(&self) -> ProxyDrawMode {
        self.proxy_draw_mode
    }

    /// Sets the Era-1 sky layer (the set_overlay_enabled precedent:
    /// a sticky per-renderer setting). `Some` makes every subsequent
    /// corner-family frame draw the sky pass FIRST — the fullscreen
    /// triangle with NO depth interaction, so the scene's own
    /// fragments are untouched and only the uncovered background
    /// changes (the skyless path stays byte-identical). `None`
    /// restores the skyless path (the state is lazily created either
    /// way — the skyless allocation pins never move).
    pub fn set_sky(&mut self, sky: Option<SkyFrame>) {
        self.sky = sky;
    }

    /// The current sky setting.
    pub fn sky(&self) -> Option<SkyFrame> {
        self.sky
    }

    /// Sets the Era-1 post chain (the set_sky precedent: a sticky
    /// per-renderer setting). `true` makes every subsequent
    /// corner-family frame render through the HDR chain — the
    /// scene pass into the offscreen R16G16B16A16 target, the
    /// bloom mips, and the composite + ACES tonemap into the
    /// swapchain (`post.rs`'s equations, the shader chain's
    /// mirrors). `false` restores the postless path
    /// (byte-identical to the pre-package renderer; the post
    /// state, once created, stays allocated — the lazy
    /// discipline's either-way rule).
    pub fn set_post(&mut self, post: bool) {
        self.post = post;
    }

    /// The current post setting.
    pub fn post(&self) -> bool {
        self.post
    }

    /// Sets the Era-1 wet-ground reflection (the set_sky/set_post
    /// precedent: a sticky per-renderer setting). `true` makes
    /// every subsequent corner-family frame render the mirrored
    /// scene (the corner mesh's building masses, the world-mirror
    /// construction — planar.rs's `ground_reflection_matrix`, the
    /// winding flipped) into an offscreen RGBA16F target, then
    /// re-draw the ground with the reflection sampled at the same
    /// screen position, Schlick-blended by view angle
    /// (`planar.rs::reflection_blend`). `false` restores the dry
    /// path (byte-identical; the state, once created, stays
    /// allocated). The mirrored scene is the BUILDINGS only — the
    /// sky and the proxy population are the documented follow-ups
    /// (a wet populated frame renders the population unreflected,
    /// honestly labeled).
    pub fn set_wet(&mut self, wet: bool) {
        self.wet = wet;
    }

    /// The current wet setting.
    pub fn wet(&self) -> bool {
        self.wet
    }

    /// The wet reflection's camera description (the sticky
    /// setting's frame data — the true camera the mirrored math
    /// consumes).
    fn wet_camera(
        &self,
        view_proj: &[[f32; 4]; 4],
        eye: [f32; 3],
        light: crate::lit_scene::LightParams,
    ) -> Option<vulkan::reflection_draw::WetCamera> {
        self.wet.then_some(vulkan::reflection_draw::WetCamera {
            view_proj: *view_proj,
            eye,
            light,
        })
    }

    /// The sky pass's per-frame parameters from the sticky setting
    /// and this frame's camera matrix (the CPU-computed inverse the
    /// vertex shader reconstructs its ray from — `sky::mat4_inverse`,
    /// the tested deterministic core).
    fn sky_params(&self, view_proj: &[[f32; 4]; 4]) -> Option<vulkan::sky_draw::SkyFrameParams> {
        self.sky.map(|frame| {
            vulkan::sky_draw::SkyFrameParams::new(
                crate::sky::mat4_inverse(view_proj),
                frame.palette,
                frame.disc_dir,
            )
        })
    }

    /// Requests the NEXT shadowed frame's shadow-map readback (the
    /// 6-F known-answer path — the depth-only pass's map, one f32
    /// per texel, row-major). Single-flight like the color capture.
    /// Ensures the shadow state if no shadowed frame has run yet
    /// (the lazy discipline's request-side completion — the request
    /// precedes the frame it wants recorded).
    pub fn request_shadow_map_capture(&mut self) -> Result<(), RendererError> {
        if self.pipes.shadows.is_none() {
            if self.config.demo_scene != crate::DemoScenePreference::Corner {
                return Err(RendererError::Vulkan(
                    "shadow capture requires the Corner scene preference".into(),
                ));
            }
            self.pipes.ensure_shadow_resources(&self.ctx)?;
        }
        vulkan::request_shadow_readback(&mut self.frames)
    }

    /// Takes the completed shadow-map readback, if one is ready (the
    /// recording slot's fence is waited — the color capture's take
    /// discipline). The depths are the UNBIASED light-space depths
    /// the depth-only pass rasterized.
    pub fn take_shadow_map_capture(&mut self) -> Result<Option<ShadowMapReadback>, RendererError> {
        let depths = vulkan::take_shadow_readback(&self.ctx, &mut self.pipes, &mut self.frames)?;
        Ok(depths.map(|depths| ShadowMapReadback {
            extent: crate::shadows::SHADOW_MAP_EXTENT,
            depths,
        }))
    }

    fn finish_frame(&mut self, outcome: vulkan::FrameOutcome) -> FrameResult {
        // F-03 Step 1: accumulate the frame's capacity decisions —
        // cumulative omitted vertices, peak supplied, and the per-slot
        // capacities (region stays 0 until its lazy buffer allocates).
        // Deferred/minimized frames carry all-zero decisions, so they
        // contribute nothing. Nonzero truncation is REPORTED in the run
        // summary, never fatal.
        self.stats.region_truncated_vertices += u64::from(outcome.region.truncated);
        self.stats.region_supplied_peak_vertices = self
            .stats
            .region_supplied_peak_vertices
            .max(outcome.region.supplied);
        self.stats.region_capacity_vertices = self
            .stats
            .region_capacity_vertices
            .max(outcome.region.capacity);
        self.stats.overlay_truncated_vertices += u64::from(outcome.overlay.truncated);
        self.stats.overlay_supplied_peak_vertices = self
            .stats
            .overlay_supplied_peak_vertices
            .max(outcome.overlay.supplied);
        self.stats.overlay_capacity_vertices = self
            .stats
            .overlay_capacity_vertices
            .max(outcome.overlay.capacity);
        if outcome.presented {
            self.stats.frames_presented += 1;
        }
        if outcome.recreated {
            self.stats.swapchain_recreations += 1;
            self.stats.extent = (self.swapchain.extent.width, self.swapchain.extent.height);
            self.stats.swapchain_images = self.swapchain.image_count;
        }
        if let Some(duration) = outcome
            .gpu_render_ms
            .and_then(|ms| Duration::try_from_secs_f64(ms / 1000.0).ok())
        {
            self.stats.gpu_timing.record(duration);
        }
        // The validation drain is gated (perf batch 3, corrected per
        // the review's P2): an inactive validation layer takes no
        // per-frame mutex round-trip — EXCEPT while environment
        // diagnostics (loader limitations recorded through the queue)
        // are pending, which a cheap atomic check catches; the hint
        // clears after that drain, so the ordinary inactive frame
        // stays round-trip-free without ever suppressing the
        // environment warnings.
        if self.stats.validation_active || environment_pending() {
            for (class, message) in drain_validation_messages() {
                match class {
                    2 => {
                        self.stats.validation_errors += 1;
                        self.push_validation_message(message);
                    }
                    1 => {
                        self.stats.validation_warnings += 1;
                        self.push_validation_message(message);
                    }
                    _ => {}
                }
            }
        }
        FrameResult {
            presented: outcome.presented,
            recreated: outcome.recreated,
            suboptimal: outcome.suboptimal,
            deferred: outcome.deferred,
            gpu_render_ms: outcome.gpu_render_ms,
            geometry: GeometryStats {
                scene_drawn: outcome.region.drawn,
                foreground_drawn: outcome.foreground_drawn,
                overlay_drawn: outcome.overlay.drawn,
                scene_omitted: outcome.region.truncated,
                overlay_omitted: outcome.overlay.truncated,
                dynamic_draw_calls: u32::from(outcome.region.drawn > outcome.foreground_drawn)
                    + u32::from(outcome.foreground_drawn > 0)
                    + u32::from(outcome.overlay.drawn > 0),
                proxy_draw_calls: outcome.proxy_draw_calls,
                uploaded_bytes: u64::from(outcome.region.drawn) * size_of::<SceneVertex>() as u64
                    + u64::from(outcome.overlay.drawn) * size_of::<OverlayVertex>() as u64,
            },
        }
    }

    /// Reads cumulative allocation counters on demand.
    pub fn allocations(&self) -> AllocationStats {
        self.ctx.allocations()
    }

    /// Arms a one-shot target-construction fault (the corrective
    /// finding 3+8 verification knob): the ordinal-th matching
    /// fallible step in the named builder returns
    /// [`RendererError::InjectedFault`]. The ordinal is 1-based and
    /// decrements per MATCHING check (across all target builders of
    /// the same site). The plan is consumed only when the ordinal
    /// reaches 0. Diagnostic-only, test-only: never produced by real
    /// device calls. The `&mut self` is intentional: the one-shot
    /// intent is encoded in the type (a caller must mutably borrow to
    /// arm it).
    pub fn inject_target_fault(
        &mut self,
        site: TargetFaultSite,
        stage: TargetFaultStage,
        ordinal: u32,
    ) {
        self.ctx.set_target_fault(site, stage, ordinal);
    }

    pub fn stats(&self) -> &RenderStats {
        &self.stats
    }

    pub fn set_overlay_enabled(&mut self, enabled: bool) {
        self.overlay_enabled = enabled;
    }

    pub fn overlay_enabled(&self) -> bool {
        self.overlay_enabled
    }

    /// Requests that the NEXT rendered frame be copied back to the
    /// host (the bounded frame-capture readback). ONE capture
    /// outstanding at a time: refused with a typed error while an
    /// earlier request is pending or ready-but-unconsumed (the
    /// single-flight lifecycle policy — a second copy could otherwise
    /// overwrite the earlier readback's destination). A capture
    /// REFUSAL never poisons frame synchronization: the frame renders
    /// normally and a later request can try again.
    pub fn request_frame_capture(&mut self) -> Result<(), RendererError> {
        vulkan::request_capture(&mut self.frames)
    }

    /// Takes the completed frame capture, if one is ready: waits the
    /// recording frame's submission fence and returns the presented
    /// image's texels (top row first, the swapchain's channel order).
    /// `Ok(None)` when no capture completed. Read promptly — a later
    /// capture overwrites the shared destination buffer.
    pub fn take_frame_capture(&mut self) -> Result<Option<CapturedFrame>, RendererError> {
        vulkan::take_capture(&self.ctx, &mut self.frames)
    }

    /// The most recent capture refusal or drop, if any (`None` in the
    /// ordinary path) — observable for diagnosis. A refused or
    /// dropped capture is never a frame error: the frame rendered
    /// normally regardless.
    pub fn last_capture_refusal(&self) -> Option<&RendererError> {
        vulkan::last_capture_refusal(&self.frames)
    }

    fn push_validation_message(&mut self, message: String) {
        self.stats.recent_validation.push(message);
        let len = self.stats.recent_validation.len();
        if len > 16 {
            self.stats.recent_validation.drain(0..len - 16);
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        vulkan::device_idle(&self.ctx).ok();
        self.frames.destroy(&self.ctx);
        // Framebuffers before the render pass they reference.
        self.swapchain.destroy(&self.ctx);
        self.pipes.destroy(&self.ctx);
        self.ctx.destroy();
    }
}
