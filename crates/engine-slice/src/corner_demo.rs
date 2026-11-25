//! One Corner of the City. One deterministic pose per presented frame
//! — or, with `--free-look` (Era 1), the interactive eyeball camera:
//! presentation only, never a measured claim.
use crate::corner_population::Block;
use crate::frame_timing::FrameTiming;
use crate::math::{Mat4, camera_forward, camera_right, camera_view, mul, percentile, perspective};
use crate::render_demo::free_flight_look_delta;
use engine_core::audio_triggers::{
    ActivityObservation, ClassMapping, SoundEmitter, SoundTrigger, project_triggers,
};
use engine_core::lod::LodLevel;
use engine_core::npc_behavior::NpcId;
use engine_core::timing::TimingSamples;
use engine_renderer::config::FRAMES_IN_FLIGHT;
use engine_renderer::lit_scene::{
    LightParams, RouteWaypoint, SCENE_EXTENT, camera_route, day_preset, night_preset,
};
use engine_renderer::{
    AllocationStats, DemoScenePreference, PresentModePreference, ProxyDrawMode, RenderConfig,
    Renderer, ValidationPreference,
};
use engine_shell::{
    AudioDevice, AudioError, DriverPreference, Keycode, Listener, SOUND_CLASS_FOOTSTEP, ShellApp,
    ShellConfig, pace_until,
};
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

/// The demo's walking activity class — POLICY vocabulary on the
/// demo's side of the trigger projection's engine/policy boundary:
/// one walking NPC's one footstep (the observation carries it; the
/// mapping below projects it onto the device layer's footstep sound
/// class).
const WALKING_ACTIVITY_CLASS: u32 = 1;

/// The demo's class mapping table (POLICY data above the engine's
/// projection): the walking activity class projects onto the SDL3
/// device layer's [`SOUND_CLASS_FOOTSTEP`].
const AUDIO_CLASS_MAPPINGS: [ClassMapping; 1] = [ClassMapping {
    activity_class: WALKING_ACTIVITY_CLASS,
    sound_class: SOUND_CLASS_FOOTSTEP,
}];

/// The footstep interval (seconds per step). PRESENTATION-SIDE
/// timing, NOT deterministic state: which footstep fires when is the
/// audible rendering's business (the audio map's constraint 1.1:
/// "the audible rendering is outside the contract") — this constant
/// and the accumulator it drives never enter the spine, a
/// fingerprint, or a measured claim. 0.65 s is a reasonable walk
/// cadence at the crowd's strolling speeds.
const AUDIO_STEP_INTERVAL: f32 = 0.65;

/// The rendering bridge's per-frame crowd state: the routes, states,
/// and mask from `corner_population_locomotion`, the RETAINED instance
/// buffer (mutated in place by `corner_instances_into`), the street
/// palette, the RETAINED pose buffer, and the (unchanged) vehicle
/// table. `step` advances one deterministic 1/60 tick and re-evaluates
/// both buffers — zero allocation after the first frame.
///
/// When behavior is on, the crowd holds the `Spine<CornerCrowdWorld>`
/// (constructed once: `corner_population_locomotion()` → the world
/// takes the states/present, the crowd keeps the routes; the 24 first
/// wakes armed at construction) and `step()` becomes the composed tick:
/// `spine.run_tick()` then `world.composed_tick(1/60)` then the
/// instance/pose re-packs (the states now live IN the world — borrow
/// them out: `spine.world().states`). Render-then-step, frame 0 =
/// part 4's construction (byte-identical — the composed mode's first
/// frame equals the plain populated mode's first frame; documented).
struct LocomotionCrowd {
    routes: Vec<engine_core::npc_locomotion::Route>,
    states: Vec<engine_core::npc_locomotion::NpcLocomotion>,
    present: Vec<bool>,
    instances: Vec<engine_renderer::proxy_instances::ProxyInstance>,
    tints: [[f32; 3]; 24],
    poses: Vec<Block>,
    vehicles: Vec<engine_renderer::proxy_instances::ProxyInstance>,
    /// When behavior is on, the behavior spine and world.
    spine: Option<engine_core::spine::Spine<crate::corner_behavior::CornerCrowdWorld>>,
}

impl LocomotionCrowd {
    /// The plain (non-behavior) constructor.
    fn new() -> Self {
        let (routes, states, present) = crate::corner_routes::corner_population_locomotion();
        let tints = crate::corner_population::corner_npc_tints();
        let mut instances = Vec::with_capacity(24);
        instances.resize(
            24,
            engine_renderer::proxy_instances::ProxyInstance {
                transform: engine_renderer::proxy_instances::InstanceTransform {
                    pos: [0.0, 0.0, 0.0],
                    turn: 0,
                    scale: 0.0,
                },
                tint: [0.0, 0.0, 0.0],
                pose_index: 0,
            },
        );
        crate::corner_routes::corner_instances_into(&states, &tints, &mut instances);
        let mut poses = Vec::new();
        crate::corner_routes::evaluate_population_poses_locomotion(&states, &mut poses);
        let (_, vehicles) = crate::corner_population::corner_population();
        LocomotionCrowd {
            routes,
            states,
            present,
            instances,
            tints,
            poses,
            vehicles,
            spine: None,
        }
    }

    /// The behavior spine constructor: the 24 schedules/routines/rngs,
    /// the locomotion states and present mask from part 4's constructor,
    /// and the routes (cloned once at construction). The 40 initial wake
    /// events are armed at construction.
    fn with_behavior() -> Self {
        let (routes, states, present) = crate::corner_routes::corner_population_locomotion();
        let tints = crate::corner_population::corner_npc_tints();
        let mut instances = Vec::with_capacity(24);
        instances.resize(
            24,
            engine_renderer::proxy_instances::ProxyInstance {
                transform: engine_renderer::proxy_instances::InstanceTransform {
                    pos: [0.0, 0.0, 0.0],
                    turn: 0,
                    scale: 0.0,
                },
                tint: [0.0, 0.0, 0.0],
                pose_index: 0,
            },
        );
        crate::corner_routes::corner_instances_into(&states, &tints, &mut instances);
        let mut poses = Vec::new();
        crate::corner_routes::evaluate_population_poses_locomotion(&states, &mut poses);
        let (_, vehicles) = crate::corner_population::corner_population();

        // Build the behavior world with the 64/near32 reservation.
        let world = crate::corner_behavior::CornerCrowdWorld::new(states.clone(), present.clone());
        let mut spine = engine_core::spine::Spine::with_reservation(world, 64, 32);

        // Arm EVERY schedule entry at its own first occurrence.
        let world = spine.world();
        let mut specs: Vec<engine_core::event::EventSpec> = Vec::new();
        for npc in 0..24 {
            let schedule = &world.schedules[npc];
            for (entry_idx, entry) in schedule.entries.iter().enumerate() {
                let payload =
                    engine_core::npc_execution::wake_payload(npc as u32, entry_idx as u32)
                        .expect("the entry index packs");
                specs.push(engine_core::event::EventSpec {
                    class: crate::corner_behavior::WAKE_CLASS,
                    schedule: engine_core::event::Schedule::AtLogicalTime { due: entry.start },
                    payload,
                    field: entry.activity.field,
                    depends_on: Vec::new(),
                });
            }
        }
        let _ = world;
        for spec in specs {
            spine.schedule(spec).expect("arming is legal");
        }

        LocomotionCrowd {
            routes,
            states,
            present,
            instances,
            tints,
            poses,
            vehicles,
            spine: Some(spine),
        }
    }

    /// The plain step: batch, instances, poses.
    fn step_plain(&mut self) {
        engine_core::npc_locomotion::advance_batch(
            &self.routes,
            &mut self.states,
            &self.present,
            1.0 / 60.0,
            None,
            0.3,
            1.75,
        );
        crate::corner_routes::corner_instances_into(&self.states, &self.tints, &mut self.instances);
        crate::corner_routes::evaluate_population_poses_locomotion(&self.states, &mut self.poses);
    }

    /// The composed step: spine.run_tick() then composed_tick() then
    /// instances/poses re-packs (borrow states from the world).
    fn step_composed(&mut self) {
        let spine = self.spine.as_mut().expect("behavior spine exists");
        spine.run_tick();
        let world = spine.world_mut();
        let dt = 1.0 / 60.0;
        world.composed_tick(dt);
        // Borrow states from the world and re-pack instances/poses.
        crate::corner_routes::corner_instances_into(
            &world.states,
            &self.tints,
            &mut self.instances,
        );
        crate::corner_routes::evaluate_population_poses_locomotion(&world.states, &mut self.poses);
    }

    /// One step: plain or composed depending on behavior flag.
    fn step(&mut self) {
        if self.spine.is_some() {
            self.step_composed();
        } else {
            self.step_plain();
        }
    }

    fn population(&self) -> engine_renderer::ProxyPopulation<'_> {
        engine_renderer::ProxyPopulation {
            npc: &self.instances,
            npc_poses: &self.poses,
            npc_material: NPC_MATERIAL,
            vehicle: &self.vehicles,
            vehicle_material: VEHICLE_MATERIAL,
        }
    }

    /// The crowd's CURRENT locomotion states — the same states the
    /// rendering bridge re-packs into the instance buffer each step:
    /// the local table in the plain mode, the spine world's in the
    /// composed mode (the states live IN the world when behavior is
    /// on; borrow them out the same way `step_composed` does).
    fn states(&self) -> &[engine_core::npc_locomotion::NpcLocomotion] {
        match &self.spine {
            Some(spine) => &spine.world().states,
            None => &self.states,
        }
    }
}

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
// The behavior spine adds one more flag; the union stays at 8 args.
// Era 2 package B part 3's audio flag adds one more bool (9).
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
    behavior: bool,
    audio: bool,
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
    // THE RENDERING BRIDGE (Era 2 package A part 4): the populated
    // route's crowd is BEHAVIOR STATE — the locomotion population
    // (routes, states, mask) stepped per presented frame at the
    // deterministic clock, the instances and pose blocks re-evaluated
    // into RETAINED buffers (zero allocation after the first frame).
    // Presentation never enters the spine: presented frame k shows the
    // crowd after k batch ticks at DEMO_CLOCK_HZ (render-then-step —
    // frame 0 is the construction; the day/night cycle's own
    // deterministic-clock precedent).
    //
    // When behavior is on, the crowd holds the `Spine<CornerCrowdWorld>`
    // (constructed once: `corner_population_locomotion()` → the world
    // takes the states/present, the crowd keeps the routes; the 40
    // initial wake events armed at construction) and `step()` becomes
    // the composed tick: `spine.run_tick()` then `world.composed_tick(1/60)`
    // then the instance/pose re-packs (the states now live IN the world).
    let mut crowd = populated.then(|| {
        if behavior {
            println!("behavior spine composed: 24 schedules, the commitment wakes drive the crowd");
            LocomotionCrowd::with_behavior()
        } else {
            LocomotionCrowd::new()
        }
    });
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
    if let Some(crowd) = &crowd {
        let mode_label = match proxy_mode {
            ProxyDrawMode::PerInstance => "per-object reference mode",
            ProxyDrawMode::Instanced => "instanced mode",
            ProxyDrawMode::Indirect => "indirect mode",
        };
        let walking = crowd.states.iter().filter(|s| s.speed > 0.0).count();
        println!(
            "animated population: {walking} walking, {} standing (the {} s walk cycle, per-NPC \
             speed and phase draws on the corner's walking lanes; presented frame k shows the \
             crowd after k ticks at the {} Hz reference)",
            crowd.states.len() - walking,
            crate::corner_population::WALK_STRIDE_SECONDS,
            crate::corner_population::DEMO_CLOCK_HZ
        );
        if crowd.spine.is_some() {
            println!(
                "behavior spine composed: 24 schedules, the commitment wakes drive the crowd \
                 (the exit summary carries the counts)"
            );
        }
        println!(
            "proxy population: {} instances ({} npc + {} vehicle), {} draw calls per frame ({})",
            crowd.instances.len() + crowd.vehicles.len(),
            crowd.instances.len(),
            crowd.vehicles.len(),
            if proxy_reference {
                (crowd.instances.len() + crowd.vehicles.len()) as u32
            } else {
                u32::from(!crowd.instances.is_empty()) + u32::from(!crowd.vehicles.is_empty())
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
            crowd.as_mut(),
            shadows,
            light,
            bias,
            day_night,
            options,
            pace,
            audio,
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
        // The per-frame population: the crowd's CURRENT state (frame i
        // shows the crowd after i ticks — the render-then-step
        // discipline; the step follows the submit below).
        let frame_population = crowd.as_ref().map(|crowd| crowd.population());
        let matrix = view_projection(pose, aspect);
        // Submission selection: the four-way union
        // (--populated --shadows together is the Era-2 stage 4b path).
        let frame = if shadows && let Some(population) = frame_population.as_ref() {
            renderer.render_corner_populated_shadowed(
                &matrix,
                pose.position,
                light,
                bias,
                population,
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
        // THE CROWD'S TICK (render-then-step): the submission above
        // presented frame i; one deterministic 1/60 batch step now
        // advances the crowd for frame i+1 — the instances and pose
        // blocks re-evaluated into their retained buffers (zero
        // allocation; the presentation-only crowd never enters the
        // spine, a fingerprint, or a measured claim).
        if let Some(crowd) = crowd.as_mut() {
            crowd.step();
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
    if let Some(crowd) = &crowd
        && let Some(spine) = &crowd.spine
    {
        let world = spine.world();
        println!(
            "behavior spine composed: {} wake executions, {} arrivals over the route (exit)",
            world.wake_executions, world.arrivals
        );
    }
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
    mut crowd: Option<&mut LocomotionCrowd>,
    shadows: bool,
    mut light: LightParams,
    bias: engine_renderer::shadows::ShadowBias,
    day_night: Option<f32>,
    options: FreeLookOptions,
    pace: Option<Duration>,
    audio: bool,
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
    // Era 2 package B part 3: the SDL3 audio device layer behind the
    // walking crowd. LAZY (constructed here, on first audio use —
    // never at startup) and DEGRADING: the typed [`AudioError`]
    // refusals are printed silence, never an error — the frame path
    // continues without sound (audio is never a prerequisite; the
    // shell's dummy-driver precedent).
    let mut audio_device: Option<AudioDevice> = if audio {
        match AudioDevice::try_new(app.platform()) {
            Ok(device) => {
                println!(
                    "audio: device opened ({} / {})",
                    device.driver(),
                    device.device_name()
                );
                Some(device)
            }
            Err(AudioError::NoDevice) => {
                println!("audio: no device (typed silence — the frame path continues)");
                None
            }
            Err(AudioError::StreamOpen(reason)) => {
                println!(
                    "audio: the stream refused to open ({reason}) — typed silence, \
                     the frame path continues"
                );
                None
            }
        }
    } else {
        None
    };
    // The per-NPC step accumulator (fixed [f32; 24] — the crowd's NPC
    // count). PRESENTATION-SIDE timing, NOT deterministic state:
    // which footstep fires when is the audible rendering's business
    // (the audio map's constraint 1.1: "the audible rendering is
    // outside the contract") — this array never enters the spine, a
    // fingerprint, or a measured claim.
    let mut step_accumulators = [0.0_f32; 24];
    // The observation/trigger buffers, retained across frames (zero
    // allocation after the first audio frame — the crowd's own
    // retained-buffer discipline).
    let mut observations: Vec<ActivityObservation> = Vec::new();
    let mut triggers: Vec<SoundTrigger> = Vec::new();
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
        // The crowd's CURRENT state (presentation-only determinism:
        // presented frame k shows the crowd after k ticks — the same
        // render-then-step discipline the scripted route uses; the
        // step follows the submit below).
        let frame_population = crowd.as_ref().map(|crowd| crowd.population());
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
        if crowd.is_some() && frame.geometry.proxy_draw_calls == 0 {
            return Err("free-look populated session drew no proxy calls".into());
        }
        // The crowd's tick (render-then-step — one deterministic 1/60
        // batch step per PRESENTED frame; a minimized or deferred frame
        // presents nothing and steps nothing).
        if let Some(crowd) = crowd.as_mut() {
            crowd.step();
        }
        // Era 2 package B part 3 — THE AUDIBLE CROWD (after the
        // composed tick, presentation-side): the walking NPCs'
        // footsteps through the trigger projection into the SDL3
        // device layer, and the listener from the camera's own pose.
        // Skipped identically to the tick above (a frame that steps
        // nothing accumulates nothing — feet only move on presented
        // frames).
        if let (Some(audio), Some(crowd)) = (audio_device.as_mut(), crowd.as_ref()) {
            observations.clear();
            // The step accumulator: each WALKING NPC (speed > 0 from
            // the crowd states — the same states the rendering bridge
            // re-packs) accumulates the tick's own dt (the
            // deterministic 1/60 the crowd stepped, so footsteps stay
            // in lockstep with the visible walking — never the wall
            // clock, which uncapped presentation decouples); past the
            // step interval one footstep observation fires and the
            // accumulator resets. A standing NPC resets (no
            // half-steps carry across a stand).
            for (i, state) in crowd.states().iter().enumerate() {
                if state.speed > 0.0 {
                    step_accumulators[i] += 1.0 / 60.0;
                    if step_accumulators[i] >= AUDIO_STEP_INTERVAL {
                        step_accumulators[i] = 0.0;
                        observations.push(ActivityObservation {
                            emitter: SoundEmitter::Npc(NpcId(i as u32)),
                            activity_class: WALKING_ACTIVITY_CLASS,
                            // The instance position — the same
                            // [x, 0.02, z] the rendering bridge reads.
                            position: crowd.instances[i].transform.pos,
                            // Free-look is the near field: the Full
                            // LOD tier projects.
                            tier: LodLevel::Full,
                        });
                    }
                } else {
                    step_accumulators[i] = 0.0;
                }
            }
            // The projection (the pure trigger core) then the device
            // submission. The typed overflow refusal is observed
            // fail-visibly — unreachable at the crowd's 24-NPC scale
            // against the 64-trigger pool bound, but never swallowed.
            if let Err(overflow) =
                project_triggers(&observations, &AUDIO_CLASS_MAPPINGS, &mut triggers)
            {
                println!(
                    "audio: trigger pool overflow ({} dropped — fail-visible)",
                    overflow.dropped
                );
            }
            audio.submit_triggers(&triggers);
            // The listener: the camera's own basis — camera_forward/
            // camera_right are the rotation matrix's columns, so they
            // arrive pre-normalized and orthogonal (the Listener
            // contract); the falloff is the slice's spatial policy
            // (reference distance 4, rolloff 1).
            audio.set_listener(Listener {
                position: camera.position,
                forward: camera_forward(camera.yaw, camera.pitch, 0.),
                right: camera_right(camera.yaw, camera.pitch, 0.),
                ref_distance: 4.0,
                rolloff: 1.0,
            });
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
    // Era 2 package B part 3: the audio exit summary — the
    // fail-visible counters (§4's gates) plus the counted residency,
    // only when the device opened (the silent degradations printed
    // their reason at construction and carry nothing further).
    if let Some(audio) = &audio_device {
        let stats = audio.stats();
        println!(
            "audio: underruns {}, worst callback {} ns, voices active {}, \
             voices stolen {}, triggers dropped {}; residency {} bytes",
            stats.underruns,
            stats.worst_callback_ns,
            stats.voices_active,
            stats.voices_stolen,
            stats.triggers_dropped,
            audio.residency_bytes()
        );
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
