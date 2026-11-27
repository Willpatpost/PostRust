//! One Corner of the City. One deterministic pose per presented frame
//! — or, with `--free-look` (Era 1), the interactive eyeball camera:
//! presentation only, never a measured claim.
use crate::corner_population::Block;
use crate::corner_timing::{DEMO_CLOCK_DT_F32, FixedTimestepAccumulator};
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

/// One presented frame's footstep observations from the COMMITTED
/// locomotion states — finding 10's corrective: the contacts derive
/// from CROSSINGS OF THE COMMITTED WALKING PHASE, never from a
/// reset-on-stand accumulator. A footstep fires when NPC i's
/// `stride_phase` crosses a HALF-STRIDE boundary (0.5, or the wrap
/// at 1.0) between the previous and current observation — two
/// contacts per stride cycle, the cadence the animation core's own
/// `WALK_STRIDE_SECONDS` (1.1 s over the 1.54 m stride at 1.4 m/s)
/// implies, individually scaled by each NPC's own `speed_scale`.
///
/// WHY THE COMMITTED PHASE (not a presentation-side accumulator):
/// the locomotion state already carries the phase, it is updated
/// only while actually walking (`npc_locomotion` step 9 — distance
/// over stride, frozen when blocked, the no-foot-slide rule), and it
/// is KEPT EXACTLY across a stand→resume (the behavior seam's own
/// known answer: no redraw, no reset). Cadence therefore follows
/// ACTUAL movement — distance walked, individual speed scale, and
/// each NPC's own seed-drawn phase (the same phase the visible walk
/// pose samples, so the audible contacts track the rendered feet) —
/// and two walkers that stop and restart on the same tick resume
/// from their OWN frozen phases: the pre-corrective accumulator
/// reset both to zero every standing frame, synchronizing them
/// forever; the committed phase cannot synchronize them.
///
/// PRESENTATION-SIDE and READ-ONLY (the audio map's constraint 1.1:
/// "the audible rendering is outside the contract"): this function
/// never mutates the spine's states; `last_phases` is the demo's own
/// previous-frame tracker, `f32::NAN` before an NPC's first
/// observation (the priming frame records without firing, so the
/// first audible frame can never invent a crossing that never
/// happened). Neither the tracker nor the observations enter the
/// spine, a fingerprint, or a measured claim.
fn project_footstep_observations(
    states: &[engine_core::npc_locomotion::NpcLocomotion],
    position_of: impl Fn(usize) -> [f32; 3],
    last_phases: &mut [f32],
    observations: &mut Vec<ActivityObservation>,
) {
    for (i, state) in states.iter().enumerate() {
        let previous = last_phases[i];
        last_phases[i] = state.stride_phase;
        // The priming frame: record without firing.
        if !previous.is_finite() {
            continue;
        }
        // Standing is silence — and the tracker keeps the frozen
        // committed phase, so a resume continues from it (the
        // anti-synchronization property; see the headless tests).
        if state.speed <= 0.0 {
            continue;
        }
        // The half-stride boundary crossing: the committed phase
        // lives in [0, 1), so `phase * 2` floors to 0 or 1 and a
        // change between observations is a crossing of 0.5 (0 → 1)
        // or the wrap (1 → 0). A blocked walker's phase is frozen,
        // so a walker pinned against a wall is audibly still too.
        let before = (previous * 2.0) as i32;
        let after = (state.stride_phase * 2.0) as i32;
        if before != after {
            observations.push(ActivityObservation {
                emitter: SoundEmitter::Npc(NpcId(i as u32)),
                activity_class: WALKING_ACTIVITY_CLASS,
                position: position_of(i),
                tier: LodLevel::Full,
            });
        }
    }
}

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
///
/// When rain is enabled, the crowd also owns the `RainLayer` that
/// drives the rain particles and builds the instance buffer per frame.
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
    /// When rain is enabled, the rain layer that owns the emitter
    /// and builds instances per frame.
    rain: Option<crate::corner_weather::RainLayer>,
    /// Fixed-timestep accumulator for the crowd's simulation steps.
    /// Accumulates presented-frame sim time and drives the world at
    /// exactly 1/60 per tick, with bounded catch-up and visible drop
    /// accounting beyond the cap.
    #[allow(dead_code)]
    step_accumulator: FixedTimestepAccumulator,
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
                alpha: 1.0,
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
            rain: None,
            step_accumulator: FixedTimestepAccumulator::new(),
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
                alpha: 1.0,
                pose_index: 0,
            },
        );
        crate::corner_routes::corner_instances_into(&states, &tints, &mut instances);
        let mut poses = Vec::new();
        crate::corner_routes::evaluate_population_poses_locomotion(&states, &mut poses);
        let (_, vehicles) = crate::corner_population::corner_population();

        // Build the behavior world with the 64/near32 reservation.
        // THE ROLLING STREAM MODE (the audit's finding 5 correction):
        // the demo owns this world for the whole session, so the
        // behavior stream is folded into the rolling digest and
        // counters — no per-entry retention, no reservation to
        // outgrow (the exit summary carries the digest).
        let world =
            crate::corner_behavior::CornerCrowdWorld::new_rolling(states.clone(), present.clone());
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
            rain: None,
            step_accumulator: FixedTimestepAccumulator::new(),
        }
    }

    /// Constructs the crowd with the rain layer warm-started (the
    /// first route waypoint's eye — the first presented frame is
    /// already wet). `ripple_decimation` is the walk-time density
    /// knob (the ripple-visibility diagnostic's own ladder: None =
    /// the shipped 320:1 policy and its exact pins; a value swaps
    /// ONLY the admission rate — the owner's future-walk decision
    /// tool, never a silent production change).
    fn with_rain(ripple_decimation: Option<u32>) -> Self {
        let mut crowd = Self::new();
        crowd.rain = Some(match ripple_decimation {
            None => crate::corner_weather::RainLayer::new([-5.0, 1.7, 13.0]),
            Some(decimation) => crate::corner_weather::RainLayer::with_ripple_policy(
                [-5.0, 1.7, 13.0],
                engine_core::ripple::RipplePolicy {
                    decimation,
                    ..crate::corner_weather::RIPPLE_POLICY
                },
                crate::corner_timing::demo_clock_dt().as_secs_f32(),
            ),
        });
        crowd
    }

    /// The plain step: batch, instances, poses. Advances the crowd
    /// by the given dt (from the fixed-timestep accumulator).
    fn step_plain(&mut self, dt: f32) {
        engine_core::npc_locomotion::advance_batch(
            &self.routes,
            &mut self.states,
            &self.present,
            dt,
            None,
            0.3,
            1.75,
        );
        crate::corner_routes::corner_instances_into(&self.states, &self.tints, &mut self.instances);
        crate::corner_routes::evaluate_population_poses_locomotion(&self.states, &mut self.poses);
    }

    /// The composed step: spine.run_tick() then composed_tick() then
    /// instances/poses re-packs (borrow states from the world).
    /// Advances the world by the given dt (from the fixed-timestep
    /// accumulator).
    fn step_composed(&mut self, dt: f32) {
        let spine = self.spine.as_mut().expect("behavior spine exists");
        spine.run_tick();
        let world = spine.world_mut();
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
    /// Uses the fixed-timestep accumulator to determine how many
    /// ticks to advance based on accumulated wall time.
    fn step(&mut self, dt: f32) {
        if self.spine.is_some() {
            self.step_composed(dt);
        } else {
            self.step_plain(dt);
        }
    }

    fn population(&self) -> engine_renderer::ProxyPopulation<'_> {
        engine_renderer::ProxyPopulation {
            npc: &self.instances,
            npc_poses: &self.poses,
            npc_material: NPC_MATERIAL,
            vehicle: &self.vehicles,
            vehicle_material: VEHICLE_MATERIAL,
            // The rain kind is populated when the layer exists.
            rain: self
                .rain
                .as_ref()
                .map(|r| r.population().rain)
                .unwrap_or(&[]),
            rain_material: self
                .rain
                .as_ref()
                .map(|_| crate::corner_weather::RAIN_MATERIAL)
                .unwrap_or(0),
        }
    }

    /// Updates the rain layer for one frame, building instances and
    /// ticking the emitter. `camera_position` and `camera_yaw` are
    /// from the current frame's view. `day_night` is the cycle position.
    /// `dt` is the fixed-timestep delta (typically DEMO_CLOCK_DT = 1/60).
    fn update_rain(&mut self, camera_position: [f32; 3], camera_yaw: f32, day_night: f32, dt: f32) {
        if let Some(rain) = &mut self.rain {
            let _report = rain.update_and_build(camera_position, camera_yaw, day_night, dt);
        }
    }

    /// Returns the rain layer's exit summary.
    fn rain_exit_summary(&self) -> Option<String> {
        self.rain.as_ref().map(|r| r.exit_summary())
    }

    /// Returns the rain layer's ripple census line (the rain line's
    /// own shape — printed alongside it at exit).
    fn ripple_exit_summary(&self) -> Option<String> {
        self.rain.as_ref().map(|r| r.ripple_exit_summary())
    }

    /// Returns the rain layer's folded ripple state for the digest
    /// pin (None when the rain layer is off).
    fn ripple_fold_state(&self) -> Option<u64> {
        self.rain.as_ref().map(|r| r.fold_ripple_state())
    }

    /// Returns the rain layer's folded state for the digest pin.
    fn rain_fold_state(&self) -> Option<u64> {
        self.rain.as_ref().map(|r| r.fold_state())
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
// The rain layer adds one more parameter (weather mode) and Era 2
// package B part 3's audio flag one more bool.
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
    weather: Option<String>,
    ripple_decimation: Option<u32>,
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
    // When weather is "rain", the crowd includes the warm-started rain layer.
    let mut crowd = populated.then(|| {
        if weather.as_ref().is_some_and(|w| w == "rain") {
            if let Some(decimation) = ripple_decimation {
                println!(
                    "rain layer: enabled (the emitter seed 0xDEADBEEF, policy rate=480/s) \
                     — WALK-TIME DENSITY: ripple decimation {decimation}:1 (the shipped \
                     policy is 320:1; this route's campaign and pins are the DIAGNOSTIC \
                     arm, not the shipped configuration)"
                );
            } else {
                println!("rain layer: enabled (the emitter seed 0xDEADBEEF, policy rate=480/s)");
            }
            if behavior {
                println!(
                    "behavior spine composed: 24 schedules, the commitment wakes drive the crowd"
                );
                let mut crowd = LocomotionCrowd::with_behavior();
                crowd.rain = Some(match ripple_decimation {
                    None => crate::corner_weather::RainLayer::new([-5.0, 1.7, 13.0]),
                    Some(decimation) => crate::corner_weather::RainLayer::with_ripple_policy(
                        [-5.0, 1.7, 13.0],
                        engine_core::ripple::RipplePolicy {
                            decimation,
                            ..crate::corner_weather::RIPPLE_POLICY
                        },
                        crate::corner_timing::demo_clock_dt().as_secs_f32(),
                    ),
                });
                crowd
            } else {
                LocomotionCrowd::with_rain(ripple_decimation)
            }
        } else if behavior {
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
            night,
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
        let frame_start = Instant::now();
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
        // THE RAIN UPDATE PRECEDES THE SUBMISSION (the audit's
        // finding 18 corrective — rain and rings present from ONE
        // completed tick): the layer advances its emitter and
        // ripple field one tick with THIS frame's camera (the pose
        // below is the same pose the view matrix renders), then
        // packs the post-tick instances with this frame's yaw and
        // atmosphere. The crowd's own locomotion tick stays
        // render-then-step below (the two disciplines are
        // deliberately different: the crowd's k-ticks-per-frame
        // contract predates the rain; the rain's contract is the
        // single completed tick). For the scripted route the camera
        // yaw is derived from the route's waypoint (the SAME
        // expression the view matrix uses — never re-derived
        // differently); the tint rides the day/night CYCLE position
        // when the cycle is active (the same t the atmosphere above
        // consumed — the rain darkens with the sky), else the
        // static preset (night included — the route distinguishes
        // it the same way the light preset does).
        if let Some(crowd) = crowd.as_mut() {
            let offset = std::array::from_fn(|j| pose.look_at[j] - pose.position[j]);
            let (yaw, _pitch) = crate::corner_demo::waypoint_orientation(offset);
            let rain_t = day_night
                .map(|period| engine_renderer::sky::cycle_position(i as f32 / 60.0, period))
                .unwrap_or(if night { 1.0 } else { 0.0 });
            crowd.update_rain(pose.position, yaw, rain_t, DEMO_CLOCK_DT_F32);
        }
        // The rain-ripple field (Era-2 package C recursive item 1):
        // packed into the renderer's sticky wet state BEFORE the
        // submission, from the JUST-COMPLETED tick above — frame i
        // renders the ripple field and the rain instances of the
        // SAME tick (the finding-18 corrective; a dry session
        // clears it — the empty uniform, the wet path's Era-1
        // byte-identity). The sticky setter is the presentation
        // bridge: the field state itself is deterministic sim state
        // (the campaign digest pins it) — the setter only copies it
        // into the frame's uniform.
        if wet {
            renderer.set_ripple_field(
                crowd
                    .as_ref()
                    .and_then(|c| c.rain.as_ref().map(|r| r.ripple_field())),
            );
        }
        // The per-frame population: the crowd's CURRENT state (frame i
        // shows the crowd after i ticks — the render-then-step
        // discipline for the NPCs; the rain instances inside are the
        // just-completed tick's pack from the update above).
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
        // presented frame i; advance the crowd based on accumulated
        // wall time at exactly 1/60 per tick, with bounded catch-up
        // and visible drop accounting beyond the cap. The rain
        // layer's own tick moved ABOVE the submission (finding 18);
        // only the locomotion batch steps here.
        if let Some(crowd) = crowd.as_mut() {
            // Step the crowd once per presented frame at the reference
            // 60 Hz cadence. The crowd.step() contract is render-then-step:
            // the k-th presented frame shows the crowd after k ticks.
            crowd.step(DEMO_CLOCK_DT_F32);
        }
        if measured.contains(&i) {
            cpu.record((end - frame_start).as_secs_f64() * 1000.);
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
            pace_until(frame_start + period);
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
            "behavior spine composed: {} wake executions, {} arrivals over the route (exit); \
             behavior stream: {} entries folded to digest {} (rolling — the running demo \
             retains no per-entry history, the bounded record finding 5 asked for); \
             crossing-guard settles {}, phase clamps {} (zero on the authored table — \
             nonzero is an authoring defect)",
            world.wake_executions,
            world.arrivals,
            world.stream_entries,
            world.stream_digest,
            world.guard_settles,
            world.phase_clamps
        );
    }
    // Rain layer exit summary (if enabled).
    if let Some(crowd) = &crowd {
        if let Some(summary) = crowd.rain_exit_summary() {
            println!("{}", summary);
        }
        if let Some(summary) = crowd.ripple_exit_summary() {
            println!("{}", summary);
        }
        if let Some(digest) = crowd.rain_fold_state() {
            println!("rain digest: {}", digest);
        }
        if let Some(digest) = crowd.ripple_fold_state() {
            println!("ripple digest: {}", digest);
        }
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
    night: bool,
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
    // The per-NPC committed-phase tracker (finding 10's corrective —
    // see `project_footstep_observations`): `last_phases[i]` carries
    // NPC i's committed `stride_phase` as of the previous observed
    // frame, `f32::NAN` before the first observation (the priming
    // frame). It is NEVER reset on a standing frame — the committed
    // phase itself freezes when the NPC stands (`npc_locomotion`
    // step 9 runs only at speed > 0, and the behavior seam's
    // stand→resume keeps the phase exactly), so two walkers that
    // stop and restart on the same tick resume from their OWN
    // frozen phases and can never re-synchronize. The rig-walk
    // corrective's anti-lockstep property survives the same way:
    // `start_route` draws each walker's initial phase from its own
    // seed, so the 24 walkers' contacts spread across the cadence
    // without any presentation-side offset. PRESENTATION-SIDE TIMING
    // outside the contract (the audio map's constraint 1.1: "the
    // audible rendering is outside the contract") — this array never
    // enters the spine, a fingerprint, or a measured claim.
    let mut last_phases: [f32; 24] = [f32::NAN; 24];
    // The observation/trigger buffers, retained across frames (zero
    // allocation after the first audio frame — the crowd's own
    // retained-buffer discipline).
    let mut observations: Vec<ActivityObservation> = Vec::new();
    let mut triggers: Vec<SoundTrigger> = Vec::new();
    let mut camera = FreeLookCamera::at_waypoint(camera_route()[0]);
    let mut timing = FrameTiming::new();
    let mut sim_accumulator = FixedTimestepAccumulator::new();
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
        // never teleports the view). The accumulator collects sim
        // time and drives the crowd/rain/day/night clocks per tick
        // at exactly 1/60 per tick.
        let split = timing.begin_frame(frame_start, !snapshot.minimized);
        if snapshot.minimized {
            // The campaign clock never silently pauses (the
            // no-silent-discard rule): minimized time ACCRUES in the
            // accumulator — the world catches up, bounded by the cap,
            // on the next presented frame.
            sim_accumulator.accrue(split.sim_advance);
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        // ONE accumulator feed per frame: the world (crowd, rain,
        // day/night) advances in coherent 1/60 ticks off the SAME
        // accumulated wall time, with bounded catch-up (the cap) and
        // visible drop accounting beyond it.
        let ticks = sim_accumulator.advance(split.sim_advance);
        // Era 1 package 4: the free-look clock drives the cycle from
        // the CUMULATIVE tick count (the session's total advanced
        // ticks × the fixed dt — a monotone cycle position, never a
        // per-frame reset). The cycle advances the atmosphere — the
        // light AND the sky layer.
        let cycle_t = day_night.map(|period| {
            engine_renderer::sky::cycle_position(
                sim_accumulator.ticks_advanced() as f32 * DEMO_CLOCK_DT_F32,
                period,
            )
        });
        if let Some(t) = cycle_t {
            let atmosphere = engine_renderer::sky::atmosphere_at(t);
            light = atmosphere.light;
            renderer.set_sky(Some(engine_renderer::SkyFrame {
                palette: atmosphere.sky,
                disc_dir: atmosphere.disc_dir,
            }));
        }
        // THE RAIN UPDATE PRECEDES THE SUBMISSION (the audit's
        // finding 18 corrective — rain and rings present from ONE
        // completed tick): per-tick with the camera's CURRENT yaw and
        // the cycle's current tint, the same `ticks` the crowd steps
        // on below (one clock, all three subsystems).
        if let Some(crowd) = crowd.as_mut() {
            let rain_t = cycle_t.unwrap_or(if night { 1.0 } else { 0.0 });
            // Advance rain once per tick.
            for _ in 0..ticks {
                crowd.update_rain(camera.position, camera.yaw, rain_t, DEMO_CLOCK_DT_F32);
            }
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
        // The rain update ran ABOVE (per-tick, before the submission)
        // — the frame renders the ripple field and the rain instances
        // of the SAME tick. free-look does not carry the `wet` flag —
        // the renderer's own sticky setting IS the state (set at
        // startup from the flag; reading it back is exact). The
        // sticky setter is the presentation bridge: the field state
        // itself is deterministic sim state (the campaign digest pins
        // it) — the setter only copies it into the frame's uniform.
        if renderer.wet() {
            renderer.set_ripple_field(
                crowd
                    .as_ref()
                    .and_then(|c| c.rain.as_ref().map(|r| r.ripple_field())),
            );
        }
        // The crowd's CURRENT state (presentation-only determinism:
        // presented frame k shows the crowd after k ticks — the
        // render-then-step discipline for the NPCs; the rain
        // instances inside are the just-completed tick's pack from
        // the update above).
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
        // The crowd's tick (render-then-step — the SAME `ticks` the
        // rain and the cycle advanced on above: one accumulator, one
        // clock; a deferred frame presents nothing and the crowd
        // skips its ticks — presentation-only divergence in a mode
        // that claims nothing, documented rather than silently
        // dropped). The rain layer's own tick moved ABOVE the
        // submission (finding 18); only the locomotion batch steps
        // here.
        if let Some(crowd) = crowd.as_mut() {
            // Step the crowd for each accumulated tick.
            for _ in 0..ticks {
                crowd.step(DEMO_CLOCK_DT_F32);
            }
        }
        // Era 2 package B part 3 — THE AUDIBLE CROWD (after the
        // composed tick, presentation-side): the walking NPCs'
        // footsteps through the trigger projection into the SDL3
        // device layer, and the listener from the camera's own pose.
        // Skipped identically to the tick above (a frame that steps
        // nothing accumulates nothing — feet only move on presented
        // frames).
        if let (Some(audio), Some(crowd)) = (audio_device.as_mut(), crowd.as_ref()) {
            let crowd: &LocomotionCrowd = crowd;
            observations.clear();
            // Finding 10's corrective: the footstep contacts derive
            // from crossings of the COMMITTED walking phase (see
            // `project_footstep_observations`) — read-only over the
            // crowd's states, cadence following actual movement, and
            // stop→restart can never synchronize walkers. The
            // observation position is the instance position — the
            // same [x, 0.02, z] the rendering bridge packs (the
            // instances were re-packed by the step above, so the
            // audible contact is at the visible walker).
            project_footstep_observations(
                crowd.states(),
                |i| crowd.instances[i].transform.pos,
                &mut last_phases,
                &mut observations,
            );
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
            // Finding 9's corrective: THE ATOMIC PUBLICATION — the
            // trigger batch and the listener snapshot go to the
            // device TOGETHER, in ONE command-cell acquisition (the
            // old wiring published the triggers, unlocked, then
            // published the listener in a SECOND acquisition, so a
            // callback draining in between spatialized the new
            // triggers against the OLD camera, and batched frames
            // collapsed onto the latest listener). Each drained
            // batch is now panned against the listener it was
            // published with. The listener itself: the camera's own
            // basis — camera_forward/camera_right are the rotation
            // matrix's columns, so they arrive pre-normalized and
            // orthogonal (the Listener contract); the falloff is the
            // slice's spatial policy (reference distance 4, rolloff
            // 1).
            audio.submit_triggers(
                &triggers,
                Listener {
                    position: camera.position,
                    forward: camera_forward(camera.yaw, camera.pitch, 0.),
                    right: camera_right(camera.yaw, camera.pitch, 0.),
                    ref_distance: 4.0,
                    rolloff: 1.0,
                },
            );
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
            "audio: callbacks {}, write rejections {}, worst callback {} ns, \
             voices active {}, voices stolen {}, triggers dropped {}; residency {} bytes",
            stats.callbacks,
            stats.write_rejections,
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

    // ------------------------------------------------- finding 10
    // The footstep projection's headless deciding tests: trigger
    // ticks and positions compared directly against hand-built
    // committed locomotion states (the projection is read-only, so
    // no spine is needed — the states are the same Copy type the
    // crowd owns).

    /// A walking state fixture at an authored phase (the committed
    /// fields the projection reads: speed and stride_phase; the rest
    /// are inert shape).
    fn walking_state(phase: f32, speed: f32) -> engine_core::npc_locomotion::NpcLocomotion {
        engine_core::npc_locomotion::NpcLocomotion {
            position: [0.0, 0.0],
            facing: 0,
            route: 0,
            leg: 0,
            leg_distance: 0.0,
            destination: 1,
            speed,
            speed_scale: 1.0,
            lateral: 0.0,
            stride_phase: phase,
        }
    }

    /// Advances one committed tick the way `npc_locomotion` step 9
    /// does (phase += distance / stride, wrapped into [0, 1), frozen
    /// at speed 0) — the tests' own model of the committed walk, so
    /// the projection is exercised against the state's real
    /// arithmetic at the crowd's own constants (1.4 m/s, 1.54 m
    /// stride, 1/60 tick).
    fn advance_phase(state: &mut engine_core::npc_locomotion::NpcLocomotion) {
        const STRIDE_LENGTH: f32 = 1.54;
        if state.speed > 0.0 {
            state.stride_phase += state.speed / 60.0 / STRIDE_LENGTH;
            if state.stride_phase >= 1.0 {
                state.stride_phase -= state.stride_phase.floor();
            }
        }
    }

    /// The half-stride boundary known answers: the priming frame
    /// records without firing; ordinary in-half movement does not
    /// fire; the 0.5 crossing fires; the wrap fires; a standing NPC
    /// (frozen phase) never fires. The observation carries the
    /// walker's identity, the instance position, the walking class,
    /// and the Full tier.
    #[test]
    fn footstep_contacts_fire_on_half_stride_crossings() {
        let mut states = [walking_state(0.9, 1.4)];
        let mut last_phases = [f32::NAN; 1];
        let mut observations = Vec::new();
        let position_of = |i: usize| [10.0 + i as f32, 0.02, -3.0];
        // Priming: the first observation records, never fires.
        project_footstep_observations(&states, position_of, &mut last_phases, &mut observations);
        assert!(observations.is_empty(), "the priming frame does not fire");
        // Ordinary movement inside a half-stride: no contact.
        advance_phase(&mut states[0]);
        project_footstep_observations(&states, position_of, &mut last_phases, &mut observations);
        assert!(observations.is_empty(), "no boundary, no contact");
        // The 0.5 crossing: one contact, carrying identity and
        // position.
        states[0].stride_phase = 0.49;
        last_phases[0] = 0.49;
        advance_phase(&mut states[0]);
        assert!(states[0].stride_phase > 0.5, "the fixture crossed 0.5");
        project_footstep_observations(&states, position_of, &mut last_phases, &mut observations);
        assert_eq!(observations.len(), 1, "the 0.5 crossing fires");
        assert_eq!(
            observations[0].emitter,
            SoundEmitter::Npc(NpcId(0)),
            "the walker's identity"
        );
        assert_eq!(
            observations[0].position,
            [10.0, 0.02, -3.0],
            "the instance position the rendering bridge packs"
        );
        assert_eq!(observations[0].activity_class, WALKING_ACTIVITY_CLASS);
        assert_eq!(observations[0].tier, LodLevel::Full);
        observations.clear();
        // The wrap: 0.99 → 0.01 is the 1.0 boundary — one contact.
        states[0].stride_phase = 0.99;
        last_phases[0] = 0.99;
        advance_phase(&mut states[0]);
        assert!(states[0].stride_phase < 0.5, "the fixture wrapped");
        project_footstep_observations(&states, position_of, &mut last_phases, &mut observations);
        assert_eq!(observations.len(), 1, "the wrap crossing fires");
        observations.clear();
        // Standing: the committed phase freezes, so nothing crosses.
        states[0].speed = 0.0;
        for _ in 0..10 {
            advance_phase(&mut states[0]);
            project_footstep_observations(
                &states,
                position_of,
                &mut last_phases,
                &mut observations,
            );
            assert!(observations.is_empty(), "standing is silence");
        }
    }

    /// FINDING 10's DECIDING TEST — stop → simultaneous restart must
    /// NOT synchronize. Two walkers with different committed phases
    /// (the seed draws), walking the same speed, both STOP on the
    /// same tick, stand, then both RESUME on the same tick. The
    /// pre-corrective accumulator reset to zero on every standing
    /// frame, so the two struck together on every interval after the
    /// restart; the committed-phase derivation keeps each walker's
    /// OWN frozen phase across the stand, so the two fire sequences
    /// stay offset — never the same trigger tick — while both keep
    /// walking (cadence preserved) and the stand itself is silent.
    #[test]
    fn stop_and_simultaneous_restart_never_synchronizes_walkers() {
        let mut states = [walking_state(0.2, 1.4), walking_state(0.75, 1.4)];
        let mut last_phases = [f32::NAN; 2];
        let mut observations = Vec::new();
        let position_of = |i: usize| [i as f32, 0.02, 0.0];
        let npc_of = |observation: &ActivityObservation| match observation.emitter {
            SoundEmitter::Npc(NpcId(id)) => id as usize,
            SoundEmitter::World(_) => usize::MAX,
        };
        let mut fires: [Vec<u64>; 2] = [Vec::new(), Vec::new()];
        let mut tick: u64 = 0;
        // Prime, then walk 30 ticks (both fire, never together —
        // phases 0.2 and 0.75 are 0.55 of a cycle apart, which at
        // 0.0151515 cycles/tick is never an integer number of ticks
        // from a shared half-stride boundary).
        project_footstep_observations(&states, position_of, &mut last_phases, &mut observations);
        for _ in 0..30 {
            tick += 1;
            for state in &mut states {
                advance_phase(state);
            }
            observations.clear();
            project_footstep_observations(
                &states,
                position_of,
                &mut last_phases,
                &mut observations,
            );
            for observation in &observations {
                fires[npc_of(observation)].push(tick);
            }
        }
        assert!(
            !fires[0].is_empty() && !fires[1].is_empty(),
            "both walk audibly"
        );
        assert!(
            fires[0].iter().all(|tick| !fires[1].contains(tick)),
            "different phases never share a trigger tick while walking"
        );
        // Both STOP on the same tick; the stand is silent.
        for state in &mut states {
            state.speed = 0.0;
        }
        for _ in 0..20 {
            tick += 1;
            for state in &mut states {
                advance_phase(state);
            }
            observations.clear();
            project_footstep_observations(
                &states,
                position_of,
                &mut last_phases,
                &mut observations,
            );
            assert!(observations.is_empty(), "the simultaneous stand is silence");
        }
        // Both RESUME on the same tick, the same speed: each
        // continues from its OWN frozen phase — the fire sequences
        // stay offset (this is the assertion the reset-on-stand
        // accumulator could never hold).
        for state in &mut states {
            state.speed = 1.4;
        }
        for _ in 0..100 {
            tick += 1;
            for state in &mut states {
                advance_phase(state);
            }
            observations.clear();
            project_footstep_observations(
                &states,
                position_of,
                &mut last_phases,
                &mut observations,
            );
            for observation in &observations {
                fires[npc_of(observation)].push(tick);
            }
        }
        let walker_a_after_resume = fires[0].iter().filter(|&&t| t > 50).count();
        let walker_b_after_resume = fires[1].iter().filter(|&&t| t > 50).count();
        assert!(
            walker_a_after_resume >= 2 && walker_b_after_resume >= 2,
            "both keep their cadence after the simultaneous restart"
        );
        assert!(
            fires[0].iter().all(|tick| !fires[1].contains(tick)),
            "the simultaneous restart never synchronizes the walkers"
        );
    }

    /// Cadence follows ACTUAL movement: two walkers at the same
    /// phase but different effective speeds (the crowd's own
    /// speed_scale spread, 1.0 vs 1.1 × the 1.4 m/s base over the
    /// 1.54 m stride) produce different contact counts over the
    /// same window — 250 ticks gives 3.79 cycles (7 half-stride
    /// contacts) against 4.17 cycles (8 contacts), the exact
    /// arithmetic of the committed phase advance.
    #[test]
    fn different_stride_speeds_produce_different_cadences() {
        let mut states = [walking_state(0.0, 1.4), walking_state(0.0, 1.4 * 1.1)];
        let mut last_phases = [f32::NAN; 2];
        let mut observations = Vec::new();
        let position_of = |_: usize| [0.0, 0.02, 0.0];
        let mut slower_fires = 0;
        let mut faster_fires = 0;
        project_footstep_observations(&states, position_of, &mut last_phases, &mut observations);
        for _ in 0..250 {
            for state in &mut states {
                advance_phase(state);
            }
            observations.clear();
            project_footstep_observations(
                &states,
                position_of,
                &mut last_phases,
                &mut observations,
            );
            for observation in &observations {
                match observation.emitter {
                    SoundEmitter::Npc(NpcId(0)) => slower_fires += 1,
                    SoundEmitter::Npc(NpcId(1)) => faster_fires += 1,
                    SoundEmitter::Npc(_) | SoundEmitter::World(_) => unreachable!(),
                }
            }
        }
        assert_eq!(slower_fires, 7, "3.79 cycles → 7 half-stride contacts");
        assert_eq!(faster_fires, 8, "4.17 cycles → 8 half-stride contacts");
    }
}
