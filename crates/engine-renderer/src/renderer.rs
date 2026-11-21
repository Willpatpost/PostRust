//! Safe public renderer API (Stage 2). All Vulkan unsafe lives in
//! [`crate::vulkan`]; this module owns the four Vulkan states, exposes the
//! safe surface, and collects instrumentation.

use ash::vk;

use sdl3::video::Window as SdlWindow;

use crate::config::{RenderConfig, present_mode_str};
use crate::overlay::{OverlayVertex, build_overlay_vertices_into};
use crate::scene::SceneVertex;
use crate::vulkan::{
    self, CapturedFrame, FrameState, PipelineState, RendererError, SwapchainState, VulkanContext,
    drain_validation_messages, pick_depth_format, pick_surface_format,
};

/// A batch of triangles with its own transform into clip space. A
/// foreground batch typically supplies camera-local vertices and a projection.
#[derive(Debug, Clone, Copy)]
pub struct SceneLayer<'a> {
    pub vertices: &'a [SceneVertex],
    pub view_proj: &'a [[f32; 4]; 4],
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

/// Long-lived renderer statistics (Stage 2 authorization, item 4).
///
/// These are instrumentation readings of the simple Stage 2 scene — they
/// are not claims about the first game's performance (TECH-ARCHITECTURE
/// §6, stage-table policy).
#[derive(Debug, Clone, Default)]
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
    /// The rolling GPU-time window (perf batch 3: a deque — the former
    /// `Vec` paid an O(n) front-drain memmove every frame once warm;
    /// push_front/pop_back is O(1) and capacity is retained).
    gpu_history_ms: std::collections::VecDeque<f64>,
    /// The SAME window maintained in sorted order at the push site
    /// (perf batch 3): an O(n) ordered insert/remove on a retained
    /// buffer — no allocation in steady state — so every percentile
    /// read is an O(1) index instead of the former per-call
    /// clone-the-history-and-sort (a 4.8 KB heap allocation plus an
    /// O(n log n) sort per overlay read, per frame).
    gpu_sorted_ms: Vec<f64>,
}

impl RenderStats {
    fn percentile(&self, quantile: f64) -> f64 {
        if self.gpu_sorted_ms.is_empty() {
            return 0.0;
        }
        // The same index formula over the same (sorted) multiset —
        // value-identical to the former clone-and-sort form, pinned
        // by the ring-equivalence unit test.
        let index = ((quantile * self.gpu_sorted_ms.len() as f64).ceil() as usize)
            .saturating_sub(1)
            .min(self.gpu_sorted_ms.len() - 1);
        self.gpu_sorted_ms[index]
    }

    /// Records one GPU-time sample into the rolling window and its
    /// sorted mirror (the perf-batch-3 ring maintenance, extracted as
    /// a method for the equivalence unit test): newest first, evicting
    /// the oldest beyond the 600-sample window; the sorted multiset is
    /// maintained at the same two points, so percentile reads never
    /// allocate or sort.
    fn record_gpu_ms(&mut self, milliseconds: f64) {
        self.gpu_history_ms.push_front(milliseconds);
        // The upper bound (count of elements <= the sample): inserting
        // there keeps the mirror ascending — the multiset semantics
        // the equivalence test pins.
        let insert_at = self
            .gpu_sorted_ms
            .partition_point(|x| x.total_cmp(&milliseconds) != std::cmp::Ordering::Greater);
        self.gpu_sorted_ms.insert(insert_at, milliseconds);
        if self.gpu_history_ms.len() > 600 {
            let evicted = self
                .gpu_history_ms
                .pop_back()
                .expect("the length check guarantees an element");
            // The lower bound (count of elements < the evicted sample):
            // the element at that index equals the evicted sample.
            let remove_at = self
                .gpu_sorted_ms
                .partition_point(|x| x.total_cmp(&evicted) == std::cmp::Ordering::Less);
            self.gpu_sorted_ms.remove(remove_at);
        }
    }

    /// GPU render time (timestamp-derived), millisecond percentiles.
    pub fn gpu_ms_p50(&self) -> f64 {
        self.percentile(0.50)
    }

    pub fn gpu_ms_p95(&self) -> f64 {
        self.percentile(0.95)
    }

    pub fn gpu_ms_p99(&self) -> f64 {
        self.percentile(0.99)
    }

    pub fn gpu_sample_count(&self) -> usize {
        self.gpu_history_ms.len()
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
    overlay_vertices: Vec<crate::overlay::OverlayVertex>,
}

impl Renderer {
    pub fn new(window: &SdlWindow, config: RenderConfig) -> Result<Renderer, RendererError> {
        let (mut ctx, validation_active) = VulkanContext::new(window, &config)?;
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
        let depth_format = pick_depth_format(&ctx.instance, ctx.physical_device);
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
        let (width, height) = window.size();
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
            validation_errors: 0,
            validation_warnings: 0,
            recent_validation: Vec::new(),
            region_truncated_vertices: 0,
            region_supplied_peak_vertices: 0,
            region_capacity_vertices: 0,
            overlay_truncated_vertices: 0,
            overlay_supplied_peak_vertices: 0,
            overlay_capacity_vertices: 0,
            gpu_history_ms: std::collections::VecDeque::new(),
            gpu_sorted_ms: Vec::new(),
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
    /// slot in that order. All supplied batches are triangle lists (three
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
                build_overlay_vertices_into(
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
        let overlay = &self.overlay_vertices;
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
            overlay,
        )?;
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
        if let Some(milliseconds) = outcome.gpu_render_ms {
            // The ring + sorted mirror (perf batch 3) — the
            // equivalence-pinned maintenance method.
            self.stats.record_gpu_ms(milliseconds);
        }
        // The validation drain is gated on the layer being active
        // (perf batch 3): an inactive renderer takes no per-frame
        // mutex round-trip at all.
        if self.stats.validation_active {
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
        Ok(FrameResult {
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
                uploaded_bytes: u64::from(outcome.region.drawn) * size_of::<SceneVertex>() as u64
                    + u64::from(outcome.overlay.drawn) * size_of::<OverlayVertex>() as u64,
            },
        })
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

#[cfg(test)]
mod stats_tests {
    use super::*;

    /// The perf-batch-3 equivalence pin: the ring + sorted-mirror
    /// percentile is VALUE-IDENTICAL to the former clone-and-sort
    /// form at every step of a deterministic pseudo-random sequence —
    /// including across the 600-sample eviction boundary (driven well
    /// past it at 1500 samples) and at every read quantile. The
    /// reference side is the exact former algorithm (clone the
    /// window, sort, index); equality is bit-exact because both sides
    /// index the same multiset's same element.
    #[test]
    fn gpu_percentile_ring_matches_clone_and_sort_reference() {
        let mut stats = RenderStats::default();
        let mut reference: Vec<f64> = Vec::new();
        let mut seed = 0x1234_5678_9abc_def0u64;
        for step in 0..1500u64 {
            // A deterministic LCG (no rand dependency, no platform
            // divergence) with a sawtooth mixed in for duplicates.
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let sample = (seed >> 33) as f64 % 40.0 + (step % 7) as f64;
            stats.record_gpu_ms(sample);
            reference.push(sample);
            if reference.len() > 600 {
                reference.remove(0);
            }
            let mut sorted_reference = reference.clone();
            sorted_reference.sort_by(|a, b| a.total_cmp(b));
            for quantile in [0.50, 0.95, 0.99, 1.0] {
                let expected = ((quantile * sorted_reference.len() as f64).ceil() as usize)
                    .saturating_sub(1)
                    .min(sorted_reference.len() - 1);
                assert_eq!(
                    stats.percentile(quantile),
                    sorted_reference[expected],
                    "step {step} quantile {quantile}: the ring must match the clone-and-sort reference"
                );
            }
        }
        assert_eq!(
            stats.gpu_sample_count(),
            600,
            "the window holds exactly the last 600 samples"
        );
    }
}
