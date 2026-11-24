//! One Corner of the City. One deterministic pose per presented frame
//! — or, with `--free-look` (Era 1), the interactive eyeball camera:
//! presentation only, never a measured claim.
use crate::corner_population::{Block, Phase, corner_population, corner_population_phases};
use crate::frame_timing::FrameTiming;
use crate::math::{Mat4, camera_forward, camera_right, camera_view, mul, percentile, perspective};
use crate::render_demo::free_flight_look_delta;
use engine_core::timing::TimingSamples;
use engine_renderer::config::FRAMES_IN_FLIGHT;
use engine_renderer::lit_scene::{
    LightParams, RouteWaypoint, SCENE_EXTENT, camera_route, day_preset, night_preset,
};
use engine_renderer::{
    AllocationStats, DemoScenePreference, PresentModePreference, ProxyDrawMode, ProxyPopulation,
    RenderConfig, Renderer, ValidationPreference,
};
use engine_shell::{DriverPreference, Keycode, ShellApp, ShellConfig, pace_until};
use std::time::{Duration, Instant};

const STEPS: usize = 120;
const WARMUP: usize = 60;

/// The demo's proxy materials (the caller policy `ProxyPopulation`
/// carries): NPCs with the concrete facade set (broad dim specular
/// — matte clothing), vehicles with the dark stone set (tighter,
/// slightly brighter — paint sheen). Documented choice, not engine
/// policy.
const NPC_MATERIAL: u32 = 1;
const VEHICLE_MATERIAL: u32 = 2;

/// Two seconds per segment at the reference 60 Hz. Uncapped runs present
/// the identical sequence. Shared waypoints appear once, including both ends.
pub fn route_poses() -> impl ExactSizeIterator<Item = RouteWaypoint> {
    let route = camera_route();
    (0..(route.len() - 1) * STEPS + 1).map(move |frame| {
        let start = route[frame / STEPS];
        let Some(end) = route.get(frame / STEPS + 1) else {
            return start;
        };
        let t = (frame % STEPS) as f32 / STEPS as f32;
        let lerp = |a: [f32; 3], b: [f32; 3]| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t);
        RouteWaypoint {
            position: lerp(start.position, end.position),
            look_at: lerp(start.look_at, end.look_at),
        }
    })
}

/// The orientation (yaw, pitch) that aims the camera from a
/// waypoint position at its look-at target — the EXACT expression
/// the scripted route's view matrix evaluates, extracted into one
/// tested place so the free-look camera can START on the route's
/// entry waypoint (the mode's first presented frame is
/// matrix-identical to the route's first frame; the two paths share
/// the conversion, never re-derive it).
pub fn waypoint_orientation(offset: [f32; 3]) -> (f32, f32) {
    let [x, y, z] = offset;
    ((-x).atan2(-z), y.atan2(x.hypot(z)))
}

pub fn view_projection(pose: RouteWaypoint, aspect: f32) -> Mat4 {
    let offset = std::array::from_fn(|i| pose.look_at[i] - pose.position[i]);
    let (yaw, pitch) = waypoint_orientation(offset);
    mul(
        &perspective(65_f32.to_radians(), aspect, 0.1, 150.),
        &camera_view(pose.position, yaw, pitch, 0.),
    )
}

/// The free-look mode's options; `None` in [`run`] keeps the
/// scripted measurement route, `Some` replaces the route's pose
/// with the interactive camera (presentation only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreeLookOptions {
    /// Auto-quit after this many PRESENTED frames (the offscreen
    /// smoke's bounded exit); `None` runs until Esc or window close.
    pub frames: Option<u64>,
}

/// Mouse-look sensitivity (radians per pixel) — the Stage-2 render
/// demo's own constant, reused verbatim so both free cameras feel
/// identical (the shared `free_flight_look_delta` mapping over it).
const FREE_LOOK_SENSITIVITY: f32 = 0.0025;
/// Pitch clamp (radians) — the Stage-2 render demo's value: a hair
/// under ±90°, so straight up/down stays reachable and the gimbal
/// line is never crossed.
const FREE_LOOK_PITCH_LIMIT: f32 = 1.54;
/// Walk and sprint speeds (m/s) — the Stage-2 render demo's values.
const FREE_LOOK_WALK_SPEED: f32 = 4.0;
const FREE_LOOK_SPRINT_SPEED: f32 = 12.0;
/// The eye-height floor: the scripted route's own eye height (the
/// street-level viewing band — the eyeball camera never sinks into
/// the ground plane it exists to look at).
const FREE_LOOK_EYE_MIN_Y: f32 = 1.7;
/// The eye ceiling: above the tallest building mass (22 m) for the
/// bird's-eye view of the whole intersection.
const FREE_LOOK_EYE_MAX_Y: f32 = 30.0;

/// Clamps a free-look eye position to the corner scene's world AABB
/// — ±[`SCENE_EXTENT`] horizontally, the eye band vertically. Pure
/// demo-side policy (no collision: the camera may pass THROUGH the
/// building masses; the clamp bounds the world, it does not navigate
/// it — the brief's "clamp to the world AABB" exactly, nothing more).
pub fn clamp_eye(position: [f32; 3]) -> [f32; 3] {
    [
        position[0].clamp(-SCENE_EXTENT, SCENE_EXTENT),
        position[1].clamp(FREE_LOOK_EYE_MIN_Y, FREE_LOOK_EYE_MAX_Y),
        position[2].clamp(-SCENE_EXTENT, SCENE_EXTENT),
    ]
}

/// The corner demo's interactive camera (Era 1's `--free-look`): the
/// Stage-2 6-DoF machinery (the shared, orientation-hardened look
/// mapping over engine-slice's `camera_forward`/`camera_right`/
/// `camera_view`) wired to the shell's input intents — position from
/// WASD (+R/F vertical, Shift sprint), yaw/pitch from relative
/// mouse — and clamped to the world AABB. PRESENTATION-ONLY by
/// construction: the camera state never reaches the spine, a
/// fingerprint, or any measured claim, and the scripted route's
/// series print only when the mode is off. [`FreeLookCamera::step`]
/// is pure input→state (deterministic; unit-tested with no window
/// and no GPU).
pub struct FreeLookCamera {
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

impl FreeLookCamera {
    /// Starts exactly on a route waypoint — the conversion the
    /// scripted path uses, so the mode's first presented frame is
    /// matrix-identical to the route's first (the eyeball session
    /// begins where the measurement route begins).
    pub fn at_waypoint(pose: RouteWaypoint) -> Self {
        let offset = std::array::from_fn(|i| pose.look_at[i] - pose.position[i]);
        let (yaw, pitch) = waypoint_orientation(offset);
        FreeLookCamera {
            position: pose.position,
            yaw,
            pitch,
        }
    }

    /// Applies one frame of input. `look_delta` is the shell's
    /// relative-mouse delta (pixels, collected only while captured);
    /// the movement axes are the WASD intents (+1/−1/0) plus the R/F
    /// vertical; `dt` is the frame's capped camera delta (see
    /// [`FrameTiming`] — post-stall frames never teleport the view).
    /// Roll is fixed at zero: the eyeball camera is the brief's
    /// position/yaw/pitch (the Stage-2 roll axis is a flight-camera
    /// feature a street-level scene has no use for).
    pub fn step(
        &mut self,
        look_delta: (f32, f32),
        forward: i8,
        strafe: i8,
        vertical: i8,
        sprint: bool,
        dt: f32,
    ) {
        let (yaw_delta, pitch_delta) = free_flight_look_delta(look_delta, FREE_LOOK_SENSITIVITY);
        self.yaw += yaw_delta;
        self.pitch =
            (self.pitch + pitch_delta).clamp(-FREE_LOOK_PITCH_LIMIT, FREE_LOOK_PITCH_LIMIT);
        let speed = if sprint {
            FREE_LOOK_SPRINT_SPEED
        } else {
            FREE_LOOK_WALK_SPEED
        };
        let forward_dir = camera_forward(self.yaw, self.pitch, 0.);
        let right_dir = camera_right(self.yaw, self.pitch, 0.);
        for axis in 0..3 {
            self.position[axis] += forward_dir[axis] * f32::from(forward) * speed * dt
                + right_dir[axis] * f32::from(strafe) * speed * dt;
        }
        self.position[1] += f32::from(vertical) * speed * dt;
        self.position = clamp_eye(self.position);
    }

    /// The view-projection for the CURRENT pose — the route's own
    /// projection (65° Vulkan perspective, near 0.1 / far 150) over
    /// the 6-DoF view matrix. Only the POSE differs from the
    /// scripted path; the projection never does.
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        mul(
            &perspective(65_f32.to_radians(), aspect, 0.1, 150.),
            &camera_view(self.position, self.yaw, self.pitch, 0.),
        )
    }
}

struct Distribution {
    live: TimingSamples,
    exact_ms: Vec<f64>,
}

impl Distribution {
    fn new(frames: usize) -> Self {
        Self {
            live: TimingSamples::default(),
            exact_ms: Vec::with_capacity(frames),
        }
    }

    fn record(&mut self, ms: f64) {
        self.exact_ms.push(ms);
        self.live.record(Duration::from_secs_f64(ms / 1000.));
    }

    fn report(&self, label: &str) {
        let n = self.exact_ms.len();
        if n == 0 {
            println!("{label}: unavailable (0 samples)");
            return;
        }
        let [p50, p95, p99] = [0.5, 0.95, 0.99].map(|q| self.live.percentile_ms(q));
        println!(
            "live {label} p50/p95/p99 ms (bucket upper bounds, n={n}): {p50:.6} / {p95:.6} / {p99:.6}"
        );
        let [p50, p95, p99] = [0.5, 0.95, 0.99].map(|q| percentile(&self.exact_ms, q));
        println!(
            "exact {label} p50/p95/p99 ms (nearest rank, n={n}): {p50:.6} / {p95:.6} / {p99:.6}"
        );
    }
}

// Read the OS process high-water mark once, after the final submission.
#[cfg(target_os = "linux")]
fn peak_resident_kib() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[cfg(windows)]
fn peak_resident_kib() -> Option<u64> {
    let query = format!("(Get-Process -Id {}).PeakWorkingSet64", std::process::id());
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &query])
        .output()
        .ok()?;
    let bytes: u64 = std::str::from_utf8(&out.stdout).ok()?.trim().parse().ok()?;
    Some(bytes / 1024)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn peak_resident_kib() -> Option<u64> {
    None
}

/// The demo's shadow-bias policy (documented per the API contract):
/// roughly two texel quanta of constant floor plus a slope term —
/// the corner scene's axis-aligned faces under the day/night lights
/// stay acne-free at the 1024² cascade while the peter-panning stays
/// under one texel on the ground (the equivalence gate's margins
/// measure both).
pub fn demo_bias() -> engine_renderer::shadows::ShadowBias {
    engine_renderer::shadows::ShadowBias {
        constant: 0.0015,
        slope_scale: 0.002,
    }
}

// The four-way union's arg arithmetic: each branch's run() stayed at
// or under clippy's 7 (the 6-E stack's 7 with the 6-E2 proxy-instanced
// flag; 6-F's 5); the UNION carries both feature axes and crosses to
// 8. The flat flag list is the CLI's own shape (one bool per demo
// mode, main.rs passes them straight through); grouping them into a
// config struct would restructure both packages' code inside a merge
// resolution — the render_frame precedent (vulkan.rs) is the allow.
// Era 1's free-look grows the list by ONE parameter (its options
// struct — the mode's two CLI fields grouped — not two flat flags),
// the sky layer by one bool, the day/night cycle by one Option, and
// the post chain by one bool (its own setting is data-free — the
// sticky renderer flag is the whole mode).
#[allow(clippy::too_many_arguments)]
pub fn run(
    night: bool,
    fps: u32,
    present: PresentModePreference,
    validation: ValidationPreference,
    populated: bool,
    proxy_reference: bool,
    proxy_instanced: bool,
    shadows: bool,
    free_look: Option<FreeLookOptions>,
    sky: bool,
    day_night: Option<f32>,
    post: bool,
    wet: bool,
) -> Result<(), String> {
    let mut app = ShellApp::new(ShellConfig {
        title: "One Corner of the City".into(),
        width: 1280,
        height: 720,
        target_fps: fps.max(1),
        driver: DriverPreference::Real,
    })?;
    let mut renderer = Renderer::new(
        app.window(),
        RenderConfig {
            present_mode: present,
            validation,
            demo_scene: DemoScenePreference::Corner,
            ..Default::default()
        },
    )
    .map_err(|e| e.to_string())?;
    let (npc, vehicle) = if populated {
        corner_population()
    } else {
        (Vec::new(), Vec::new())
    };
    // The Era-2 animated population: the placement table's PHASE
    // column (8 waiters stand, 16 crossers/strollers walk at
    // hand-spread offsets — no lockstep marching) and the retained
    // pose-block buffer the frame loop evaluates into (in place,
    // zero allocation after the first frame — the deterministic
    // clock: presented frame k shows pose t_k, the day/night
    // cycle's own precedent).
    let phases = if populated {
        corner_population_phases()
    } else {
        Vec::new()
    };
    let mut pose_blocks: Vec<Block> = Vec::new();
    if populated {
        crate::corner_population::evaluate_population_poses(0, &phases, &mut pose_blocks);
    }
    // The submission mode, in evidence order: the per-object
    // reference (one draw per instance — the pre-6-E shape, the
    // before/after measurement mode), the direct 6-E instanced
    // submission (--proxy-instanced, the indirect path's A/B
    // partner), and — the populated route's DEFAULT since 6-E2 —
    // the indirect submission (one vkCmdDrawIndirect per kind, the
    // instance counts read from GPU memory; the mechanism the
    // deferred GPU-culling package builds on). The default exercises
    // the shipped mechanism at route scale; the renderer API's own
    // default remains the direct instanced mode (the 6-E contract).
    let proxy_mode = if proxy_reference {
        ProxyDrawMode::PerInstance
    } else if proxy_instanced {
        ProxyDrawMode::Instanced
    } else {
        ProxyDrawMode::Indirect
    };
    renderer.set_proxy_draw_mode(proxy_mode);
    // Era 1 package 6: the post chain (the sticky renderer setting
    // — the HDR render-target chain + the bloom mips + the ACES
    // composite, post.rs's equations through the real render path).
    // Composes with EVERY corner flag (the sky's disc is the bloom
    // feeder the gate pins); the region path stays postless (the
    // setting's documented scope).
    renderer.set_post(post);
    // Era 1 package 7: the wet-ground reflection (the sticky
    // renderer setting — the mirrored scene into the offscreen
    // target, the ground re-drawn Schlick-blended). Composes with
    // every corner flag; the mirrored scene is the building masses
    // (the sky and the proxies are the documented follow-ups).
    renderer.set_wet(wet);
    if wet {
        println!(
            "wet ground: the planar reflection (the mirrored scene about y={}, the ground \
             re-drawn Schlick-blended, r0 {})",
            engine_renderer::lit_scene::GROUND_Y,
            engine_renderer::planar::REFLECTION_R0
        );
    }
    if post {
        println!(
            "post: the HDR chain (R16G16B16A16_SFLOAT scene target -> 1/{} bloom mips -> the \
             ACES composite, bloom strength {})",
            engine_renderer::post::BLOOM_DOWNSAMPLE,
            engine_renderer::post::BLOOM_STRENGTH
        );
    }
    // Era 1 package 3: the sky layer, driven from atmosphere_at's
    // palettes (the endpoints are BIT-EQUAL to the day/night
    // presets — the pinned special case; the day/night cycle
    // package advances t continuously). The disc rides the SAME
    // direction as the light (the sun by day, the moon by night).
    if sky {
        let atmosphere = engine_renderer::sky::atmosphere_at(if night { 1.0 } else { 0.0 });
        renderer.set_sky(Some(engine_renderer::SkyFrame {
            palette: atmosphere.sky,
            disc_dir: atmosphere.disc_dir,
        }));
        println!(
            "sky: the atmosphere_at {} palette behind the scene (horizon {:?}, zenith {:?}, the disc at {:?})",
            if night { "night" } else { "day" },
            atmosphere.sky.horizon,
            atmosphere.sky.zenith,
            atmosphere.disc_dir
        );
    }
    // Era 1 package 4: the day/night cycle drives the atmosphere
    // (the light AND the sky) from the day endpoint — a static
    // --night start alongside it is contradictory, refused upfront.
    if night && day_night.is_some() {
        return Err(
            "--night and --day-night together is contradictory (the cycle drives the preset from day)"
                .into(),
        );
    }
    let stats = renderer.stats();
    let size = stats.extent;
    let preset = if night { "night" } else { "day" };
    println!(
        "One Corner of the City | {preset} | {} | Vulkan {} | {}x{} native | {} | present={} | fps={} (0=uncapped) | validation={} | debug_assertions={}",
        stats.gpu_name,
        stats.api_version,
        size.0,
        size.1,
        stats.surface_format,
        stats.present_mode,
        fps,
        stats.validation_active,
        cfg!(debug_assertions)
    );
    if populated {
        let mode_label = match proxy_mode {
            ProxyDrawMode::PerInstance => "per-object reference mode",
            ProxyDrawMode::Instanced => "instanced mode",
            ProxyDrawMode::Indirect => "indirect mode",
        };
        println!(
            "animated population: {} walking, {} standing (the {} s walk cycle, per-NPC phases; \
             presented frame k shows pose t_k at the {} Hz reference)",
            phases
                .iter()
                .filter(|p| matches!(p, Phase::Walk(_)))
                .count(),
            phases.iter().filter(|p| matches!(p, Phase::Stand)).count(),
            crate::corner_population::WALK_STRIDE_SECONDS,
            crate::corner_population::DEMO_CLOCK_HZ
        );
        println!(
            "proxy population: {} instances ({} npc + {} vehicle), {} draw calls per frame ({})",
            npc.len() + vehicle.len(),
            npc.len(),
            vehicle.len(),
            if proxy_reference {
                (npc.len() + vehicle.len()) as u32
            } else {
                u32::from(!npc.is_empty()) + u32::from(!vehicle.is_empty())
            },
            mode_label
        );
    }
    if validation == ValidationPreference::On && !stats.validation_active {
        return Err("requested validation layer is unavailable".into());
    }
    if shadows {
        let bias = demo_bias();
        println!(
            "directional shadows: one cascade ({} texels), bias constant {:.4} / slope {:.4}",
            engine_renderer::shadows::SHADOW_MAP_EXTENT,
            bias.constant,
            bias.slope_scale
        );
    }
    let aspect = size.0 as f32 / size.1 as f32;
    let mut light = if night { night_preset() } else { day_preset() };
    let bias = demo_bias();
    let pace = (fps > 0).then(|| Duration::from_secs_f64(1. / f64::from(fps)));
    // Era 1 package 4: the day/night cycle's banner (the wiring
    // below advances the atmosphere — the light AND the sky layer,
    // which the cycle implies — with each loop's own demo clock).
    if let Some(period) = day_night {
        println!(
            "day/night cycle: {period}s period (the atmosphere advances with the demo clock; \
             the sky and the lit scene ride the same atmosphere_at frame)"
        );
    }
    // Era 1's free-look: the interactive eyeball camera returns its
    // own presentation-only summary; the scripted measurement route
    // below is untouched (its series print only when the mode is off).
    if let Some(options) = free_look {
        return run_free_look(
            &mut app,
            &mut renderer,
            populated.then_some((&npc[..], &vehicle[..], &phases[..])),
            &mut pose_blocks,
            shadows,
            light,
            bias,
            day_night,
            options,
            pace,
        );
    }
    let mut poses = route_poses().peekable();
    let count = poses.len();
    let mut pose = *poses.peek().unwrap();
    let measured = WARMUP..WARMUP + count;
    let lag = FRAMES_IN_FLIGHT as usize;
    let mut cpu = Distribution::new(count);
    let mut intervals = Distribution::new(count);
    let mut gpu = Distribution::new(count);
    let mut previous = Instant::now();
    for i in 0..measured.end + lag {
        let start = Instant::now();
        let input = app.frame();
        if input.quit_requested || input.minimized || app.window().size_in_pixels() != size {
            return Err("route interrupted by close, minimize or resize".into());
        }
        if measured.contains(&i) {
            pose = poses.next().unwrap();
        }
        // Era 1 package 4: the atmosphere advances with the demo
        // clock — the presented-frame count at the route's own
        // reference 60 Hz cadence (deterministic: frame k always
        // shows cycle position t_k; the measurement series are
        // untouched — the same submission work per frame).
        if let Some(period) = day_night {
            let atmosphere = engine_renderer::sky::atmosphere_at(
                engine_renderer::sky::cycle_position(i as f32 / 60.0, period),
            );
            light = atmosphere.light;
            renderer.set_sky(Some(engine_renderer::SkyFrame {
                palette: atmosphere.sky,
                disc_dir: atmosphere.disc_dir,
            }));
        }
        // The walk cycle's deterministic tick: presented frame i at
        // the 60 Hz reference advances every walking npc's clip
        // (the phase column spreads them across the stride; the
        // standing waiters keep the rest block — the evaluation's
        // own contract).
        if populated {
            crate::corner_population::evaluate_population_poses(
                i as u64,
                &phases,
                &mut pose_blocks,
            );
        }
        // The per-frame population (fresh after the evaluation — the
        // pose blocks are re-borrowed each frame, never held across
        // the evaluation).
        let frame_population = populated.then(|| ProxyPopulation {
            npc: &npc,
            npc_poses: &pose_blocks,
            npc_material: NPC_MATERIAL,
            vehicle: &vehicle,
            vehicle_material: VEHICLE_MATERIAL,
        });
        let matrix = view_projection(pose, aspect);
        // Submission selection: the four-way union
        // (--populated --shadows together is the Era-2 stage 4b path).
        let frame = if shadows && populated {
            renderer.render_corner_populated_shadowed(
                &matrix,
                pose.position,
                light,
                bias,
                frame_population.as_ref().unwrap(),
                size,
            )
        } else if shadows {
            renderer.render_corner_shadowed(&matrix, pose.position, light, bias, size)
        } else if let Some(population) = frame_population.as_ref() {
            renderer.render_corner_populated(&matrix, pose.position, light, population, size)
        } else {
            renderer.render_corner(&matrix, pose.position, light, size)
        }
        .map_err(|e| e.to_string())?;
        let end = Instant::now();
        if !frame.presented || frame.recreated || frame.suboptimal {
            return Err(format!(
                "route presentation interrupted at submission {i}: {frame:?}"
            ));
        }
        if populated && frame.geometry.proxy_draw_calls == 0 {
            return Err("populated route drew no proxy calls".into());
        }
        if measured.contains(&i) {
            cpu.record((end - start).as_secs_f64() * 1000.);
            intervals.record((end - previous).as_secs_f64() * 1000.);
        }
        // Query results belong to the previous submission in this slot.
        // Hold the exit pose for two drain submissions outside the CPU capture.
        if (measured.start + lag..measured.end + lag).contains(&i)
            && let Some(ms) = frame.gpu_render_ms
        {
            gpu.record(ms);
        }
        previous = end;
        if let Some(period) = pace {
            pace_until(start + period);
        }
    }
    if renderer.stats().gpu_timestamps_supported && gpu.exact_ms.len() != count {
        return Err(format!(
            "incomplete GPU timestamp capture: {} of {count}",
            gpu.exact_ms.len()
        ));
    }
    let stats = renderer.stats();
    if stats.validation_errors > 0 {
        return Err(format!(
            "{} validation errors: {:?}",
            stats.validation_errors, stats.recent_validation
        ));
    }
    println!(
        "{WARMUP} warmup + {count} route frames + {lag} timestamp drain; validation warnings/errors {}/{}",
        stats.validation_warnings, stats.validation_errors
    );
    let peak = peak_resident_kib().map_or_else(|| "unavailable".into(), |v| format!("{v} KiB"));
    cpu.report("CPU frame work (poll through present, includes Vulkan waits)");
    intervals.report("frame interval (consecutive present returns, includes pacing)");
    gpu.report("GPU render pass");
    println!(
        "peak process resident memory: {peak}; Vulkan allocations: {:?}",
        renderer.allocations()
    );
    Ok(())
}

/// The free-look loop (Era 1 package 1): the SAME renderer
/// submission selection as the scripted route (plain / populated /
/// shadowed), driven by the interactive camera instead of the route
/// iterator. PRESENTATION-ONLY by construction: no timing
/// distribution is recorded and no percentile series prints — the
/// camera state (position/yaw/pitch, dt-scaled input) never touches
/// a fingerprint or a measured claim. The exit summary carries the
/// correctness lines only (presented count, validation, the
/// warmed-allocation constancy when the session never recreated).
#[allow(clippy::too_many_arguments)]
fn run_free_look(
    app: &mut ShellApp,
    renderer: &mut Renderer,
    population: Option<(
        &[engine_renderer::proxy_instances::ProxyInstance],
        &[engine_renderer::proxy_instances::ProxyInstance],
        &[Phase],
    )>,
    pose_blocks: &mut Vec<Block>,
    shadows: bool,
    mut light: LightParams,
    bias: engine_renderer::shadows::ShadowBias,
    day_night: Option<f32>,
    options: FreeLookOptions,
    pace: Option<Duration>,
) -> Result<(), String> {
    // Relative mouse capture is the mouse-look contract: the shell
    // collects deltas only while captured (and clears them on focus
    // loss — its own anti-stuck discipline covers this camera too).
    app.set_relative_mouse(true);
    println!(
        "free-look: presentation only — WASD move · mouse look · R/F up/down · Shift sprint · Esc quit; \
         starts on the route's entry waypoint, clamped to the scene's world AABB; \
         no measurement series (the scripted route's print without --free-look)"
    );
    let mut camera = FreeLookCamera::at_waypoint(camera_route()[0]);
    let mut timing = FrameTiming::new();
    let mut presented: u64 = 0;
    // The standing warmed-allocations gate: snapshot after the
    // route's own warmup count, assert at exit when no swapchain
    // recreation intervened (a resize legitimately reallocates; the
    // no-recreation regime — the offscreen smoke's own — is the
    // gated one, and a resized session prints instead of asserting).
    let mut warm: Option<AllocationStats> = None;
    loop {
        let frame_start = Instant::now();
        let snapshot = app.frame();
        if snapshot.quit_requested || snapshot.keys_pressed.contains(&Keycode::Escape) {
            break;
        }
        // Frame-boundary timing: the camera step uses the capped
        // frame delta (FrameTiming's 100 ms cap — a post-stall frame
        // never teleports the view) and takes NO step on minimized
        // frames (no presentation, no movement — the render-demo
        // discipline).
        let split = timing.begin_frame(frame_start, !snapshot.minimized);
        // Era 1 package 4: the free-look clock drives the cycle (the
        // wall elapsed time FrameTiming accumulates — presentation
        // policy only; the interactive session makes no measured
        // claim).
        if let Some(period) = day_night {
            let atmosphere =
                engine_renderer::sky::atmosphere_at(engine_renderer::sky::cycle_position(
                    timing.total_sim_advanced().as_secs_f32(),
                    period,
                ));
            light = atmosphere.light;
            renderer.set_sky(Some(engine_renderer::SkyFrame {
                palette: atmosphere.sky,
                disc_dir: atmosphere.disc_dir,
            }));
        }
        if snapshot.minimized {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        let vertical = i8::from(app.is_held(Keycode::R)) - i8::from(app.is_held(Keycode::F));
        let sprint = app.is_held(Keycode::LShift);
        camera.step(
            snapshot.look_delta,
            snapshot.move_forward,
            snapshot.move_strafe,
            vertical,
            sprint,
            split
                .camera_dt
                .map(|delta| delta.as_secs_f32())
                .unwrap_or_default(),
        );
        // The live window size: free-look TOLERATES resize (the
        // renderer's recreation path is exercised by it, never an
        // error — the scripted route refuses resize because it
        // invalidates the fixed-cadence measurement, and this mode
        // claims nothing).
        let size = app.window().size_in_pixels();
        let aspect = if size.1 == 0 {
            16. / 9.
        } else {
            size.0 as f32 / size.1 as f32
        };
        // The walk tick (presentation-only determinism: the
        // presented-frame count drives the clip, the same counter
        // the scripted route uses).
        let frame_population = population.map(|(npc, vehicle, phases)| {
            crate::corner_population::evaluate_population_poses(presented, phases, pose_blocks);
            ProxyPopulation {
                npc,
                npc_poses: pose_blocks,
                npc_material: NPC_MATERIAL,
                vehicle,
                vehicle_material: VEHICLE_MATERIAL,
            }
        });
        let matrix = camera.view_projection(aspect);
        let frame = if shadows && let Some(population) = &frame_population {
            renderer.render_corner_populated_shadowed(
                &matrix,
                camera.position,
                light,
                bias,
                population,
                size,
            )
        } else if shadows {
            renderer.render_corner_shadowed(&matrix, camera.position, light, bias, size)
        } else if let Some(population) = frame_population.as_ref() {
            renderer.render_corner_populated(&matrix, camera.position, light, population, size)
        } else {
            renderer.render_corner(&matrix, camera.position, light, size)
        }
        .map_err(|e| e.to_string())?;
        if frame.deferred {
            // The drawable is temporarily unavailable (mid-transition):
            // poll quietly, present nothing, retry — no recreation storm.
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        if !frame.presented {
            return Err(format!("free-look presentation failed: {frame:?}"));
        }
        if population.is_some() && frame.geometry.proxy_draw_calls == 0 {
            return Err("free-look populated session drew no proxy calls".into());
        }
        presented += 1;
        if presented == WARMUP as u64 {
            warm = Some(renderer.allocations());
        }
        if let Some(limit) = options.frames
            && presented >= limit
        {
            break;
        }
        if let Some(period) = pace {
            pace_until(frame_start + period);
        }
    }
    let stats = renderer.stats();
    if stats.validation_errors > 0 {
        return Err(format!(
            "{} validation errors: {:?}",
            stats.validation_errors, stats.recent_validation
        ));
    }
    println!(
        "free-look: {presented} presented frames; validation warnings/errors {}/{}; swapchain recreations {}",
        stats.validation_warnings, stats.validation_errors, stats.swapchain_recreations
    );
    match warm {
        Some(warm) if stats.swapchain_recreations == 0 => {
            if renderer.allocations() != warm {
                return Err(format!(
                    "free-look presentation changed the warmed allocation set: \
                     {warm:?} -> {:?}",
                    renderer.allocations()
                ));
            }
            println!("warmed Vulkan allocations held across the session: {warm:?}");
        }
        _ => println!(
            "Vulkan allocations: {:?} (no warmed-constancy claim: the session resized or never warmed)",
            renderer.allocations()
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_visits_each_waypoint_and_keeps_its_target_in_frame() {
        let poses: Vec<_> = route_poses().collect();
        assert_eq!(poses.len(), 601);
        assert_eq!(
            poses.iter().step_by(STEPS).copied().collect::<Vec<_>>(),
            camera_route()
        );
        for pose in poses {
            let matrix = view_projection(pose, 16. / 9.);
            let [x, y, z] = pose.look_at;
            let clip: [f32; 4] = std::array::from_fn(|i| {
                matrix[0][i] * x + matrix[1][i] * y + matrix[2][i] * z + matrix[3][i]
            });
            assert!(clip[3] > 0. && clip[2] > 0. && clip[2] < clip[3]);
            assert!(clip[0].abs() < 1e-4 && clip[1].abs() < 1e-4, "{clip:?}");
        }
    }

    /// The shared-paths property: the free-look camera STARTS on the
    /// route's entry waypoint with the SAME orientation conversion,
    /// so its first frame's matrix is identical to the scripted
    /// route's first frame — exact equality, not a tolerance (the
    /// two paths evaluate the same expressions).
    #[test]
    fn free_look_starts_matrix_identical_to_the_route_entry() {
        let pose = camera_route()[0];
        let camera = FreeLookCamera::at_waypoint(pose);
        assert_eq!(camera.position, pose.position);
        assert_eq!(
            camera.view_projection(16. / 9.),
            view_projection(pose, 16. / 9.)
        );
    }

    /// The world-AABB clamp: an inside point is returned UNCHANGED
    /// (exactly — no epsilon drift), each horizontal face pulls to
    /// the ±SCENE_EXTENT boundary, and the vertical band is the eye
    /// policy (the route's eye height at the floor, the bird's-eye
    /// ceiling above the tallest mass).
    #[test]
    fn clamp_eye_holds_the_world_aabb_and_the_eye_band() {
        let inside = [3.0, 5.0, -7.0];
        assert_eq!(clamp_eye(inside), inside);
        assert_eq!(clamp_eye([41.5, 5.0, 0.0])[0], SCENE_EXTENT);
        assert_eq!(clamp_eye([-55.0, 5.0, 0.0])[0], -SCENE_EXTENT);
        assert_eq!(clamp_eye([0.0, 5.0, 48.0])[2], SCENE_EXTENT);
        assert_eq!(clamp_eye([0.0, 5.0, -44.0])[2], -SCENE_EXTENT);
        assert_eq!(clamp_eye([0.0, -3.0, 0.0])[1], FREE_LOOK_EYE_MIN_Y);
        assert_eq!(clamp_eye([0.0, 99.0, 0.0])[1], FREE_LOOK_EYE_MAX_Y);
    }

    /// The movement math at the Stage-2 machinery's exact speeds:
    /// zero orientation looks down −Z with +X right, so forward
    /// walks −Z, strafe walks +X, R climbs, and Shift sprints at
    /// 12 m/s — all exact binary-arithmetic results (4.0 / 12.0 and
    /// the dt values are exact in f32; the basis vectors at zero
    /// angles are exact).
    #[test]
    fn free_look_step_moves_along_the_camera_basis_at_the_stage2_speeds() {
        let mut camera = FreeLookCamera {
            position: [0.0, 1.7, 0.0],
            yaw: 0.0,
            pitch: 0.0,
        };
        camera.step((0.0, 0.0), 1, 0, 0, false, 0.5);
        assert_eq!(camera.position, [0.0, 1.7, -2.0]);
        camera.step((0.0, 0.0), 0, 1, 0, false, 0.5);
        assert_eq!(camera.position, [2.0, 1.7, -2.0]);
        camera.step((0.0, 0.0), 0, 0, 1, false, 0.25);
        assert_eq!(camera.position, [2.0, 2.7, -2.0]);
        camera.step((0.0, 0.0), 1, 0, 0, true, 0.5);
        assert_eq!(camera.position, [2.0, 2.7, -8.0]);
    }

    /// Runaway input stops at the world bounds: a long sprint frame
    /// cannot carry the eye past the AABB, on either axis pair.
    #[test]
    fn free_look_step_clamps_runaway_input_to_the_aabb() {
        let mut camera = FreeLookCamera {
            position: [-38.0, 1.7, 0.0],
            yaw: 0.0,
            pitch: 0.0,
        };
        camera.step((0.0, 0.0), 0, -1, 0, true, 10.0);
        assert_eq!(camera.position, [-SCENE_EXTENT, 1.7, 0.0]);
        camera.step((0.0, 0.0), 0, 0, 1, true, 10.0);
        assert_eq!(camera.position[1], FREE_LOOK_EYE_MAX_Y);
    }

    /// The look mapping composed with the step: mouse-right turns
    /// the view toward world +X and mouse-up pitches it up — the
    /// shared, orientation-hardened convention (free-flight and
    /// player mappings agree; this pins the free-look composition).
    #[test]
    fn free_look_step_follows_the_shared_look_mapping() {
        let mut camera = FreeLookCamera {
            position: [0.0, 1.7, 0.0],
            yaw: 0.0,
            pitch: 0.0,
        };
        camera.step((100.0, 0.0), 0, 0, 0, false, 0.0);
        assert_eq!(camera.position, [0.0, 1.7, 0.0], "no move input, no drift");
        let look = camera_forward(camera.yaw, camera.pitch, 0.0);
        assert!(look[0] > 0.2, "mouse-right turns toward +X: {look:?}");
        assert!(look[2] < 0.8, "off the −Z axis: {look:?}");

        let mut camera = FreeLookCamera {
            position: [0.0, 1.7, 0.0],
            yaw: 0.0,
            pitch: 0.0,
        };
        camera.step((0.0, -100.0), 0, 0, 0, false, 0.0);
        let look = camera_forward(camera.yaw, camera.pitch, 0.0);
        assert!(look[1] > 0.2, "mouse-up pitches the view up: {look:?}");
    }

    /// The pitch pin: sustained upward mouse input parks the pitch
    /// at the Stage-2 limit (a hair under straight-up) and holds it
    /// there exactly — never past, never oscillating.
    #[test]
    fn free_look_pitch_pins_at_the_stage2_limit() {
        let mut camera = FreeLookCamera {
            position: [0.0, 1.7, 0.0],
            yaw: 0.0,
            pitch: 0.0,
        };
        for _ in 0..1000 {
            camera.step((0.0, -1000.0), 0, 0, 0, false, 0.0);
        }
        assert_eq!(camera.pitch, FREE_LOOK_PITCH_LIMIT);
        assert!(camera.pitch < std::f32::consts::FRAC_PI_2);
    }
}
