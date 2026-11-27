//! The rain layer for the corner demo (Era 2 package C — particles +
//! weather, stage 4 demo + stage 5 gates). The LocomotionCrowd
//! pattern: a `RainLayer` owning the emitter, retained instance
//! buffer, and census accumulators — zero per-frame allocation after
//! the first frame.
//!
//! # DETERMINISM CONTRACT
//! Pure data + pure functions. The rain layer is deterministic: the
//! same inputs (weather state, volume, policy, seed, camera yaw,
//! day/night position) produce byte-identical output on every
//! platform. The emitter stream is seeded from the configuration's
//! seed; the billboard turn is a fixed-order integer conversion; the
//! instance buffer packing uses `to_bits` and `to_le_bytes` — pure
//! IEEE f32, no libm. Two runs with the same inputs produce
//! byte-identical captures.
//!
//! # THE DEMO PATH
//! The demo runs `RainLayer` through two modes:
//! - Scripted route mode: the camera follows `route_poses()`, the
//!   rain volume refreshes around the frustum AABB intersected with
//!   the scene world AABB (the documented `frustum ∩ aabb` discipline).
//! - Free-look mode: the camera is the `FreeLookCamera`, the yaw
//!   drives the billboard turn directly.
//!
//! The rain layer is enabled only with `--weather rain` and
//! `--populated`. The demo's policy constants are documented
//! below. The census lines in the exit summary surface the
//! per-campaign totals (the rain's and the ripple's).
use crate::corner_timing::demo_clock_dt;
use engine_core::particle::{Emitter, SpawnVolume};
use engine_core::weather::{RainPolicy, Weather};
use engine_renderer::proxy_instances::{InstanceTransform, ProxyInstance};

/// The rain policy's fixed constants (the demo authors these; the
/// engine carries no rain numbers). THE FINDING-12 POLICY
/// CORRECTION (2026-11-25): the rain now actually falls to the
/// ground — the three volumes are SEPARATE (the camera-following
/// OBSERVATION column, the overhead SPAWN BAND derived above it,
/// and the WORLD RECEIVER PLANE at street level), and the spawn
/// height, fall parameters, and lifetime are chosen TOGETHER so a
/// drop born in the band reaches the receiver before expiry. The
/// pre-corrective policy (band_height 12, lifetime [0.9, 1.4],
/// kill at the observed floor) killed rain at the initial eye
/// height: the lowest spawn was 13.7 m, the greatest initial
/// downward speed 9.15 m/s, the drag terminal ≈ 8.92 m/s, and even
/// 85 integration steps travel under 13 m before expiry — the
/// drops could never reach the real ground, and the rare kill
/// deaths "landed" 1.7 m above the drawn street.
///
/// THE COMPATIBLE PARAMETER SET (every number below derives):
/// - The OBSERVED column (see [`quantized_listener_volume`]): 8 m
///   above the eye. At the demo's street eyes (y = 1.7) the band
///   floor sits at 9.7 m above the receiver.
/// - The BAND: 4 m tall — spawns uniform in y ∈ [9.7, 13.7] at the
///   street eyes.
/// - The FALL: base −9.0 m/s with ±0.15 vertical jitter, gravity
///   9.81 and drag 0.982 per 1/60 step hold the terminal at
///   −g·dt·d/(1−d) ≈ −8.92 m/s, so a drop's average downward speed
///   stays within [8.85, 9.15] over the whole fall (the relaxation
///   from ±0.15 off-terminal decays by 0.982 per step — under 2%
///   over a 2 s fall).
/// - The LIFETIME [1.65, 2.15] s EXCEEDS the worst-case fall time:
///   the highest spawn (13.7 m) at the slowest average speed
///   (8.85 m/s) lands in 1.548 s < 1.65 s — EVERY street-level drop
///   reaches the receiver before expiry (the landing is the norm;
///   midair expiry is the honest absence, reached only when the
///   camera climbs enough that the band-following spawn height
///   outgrows the lifetime — the free-look ceiling at eye 30 puts
///   the band at [38, 42] m, a 4.3–4.7 s fall no lifetime
///   survives).
/// - THE POOL BOUND: 480 spawns/s × the mean residence
///   (fall distance mean 11.7 m / mean speed 9.0 = 1.30 s at the
///   street eyes; the residence is CAPPED by the lifetime — mean
///   1.9 s — when the camera climbs) stays under 1024: the street
///   steady state holds ≈ 624 live drops, the climb ceiling
///   ≈ 480 × 1.9 = 912, both with headroom (the per-tick gain is
///   exactly 8.0 — 480/60 in f32 — so the steady state is the
///   queue's mean, not a burst).
pub const RAIN_POLICY: RainPolicy = RainPolicy {
    full_rate: 480.0,
    band_height: 4.0,
    fall_speed: 9.0,
    lateral_jitter: 0.6,
    vertical_jitter: 0.15,
    lifetime: [1.65, 2.15],
    gravity: 9.81,
    drag_factor: 0.982,
    receiver_y: engine_renderer::lit_scene::GROUND_Y,
};

/// The rain emitter seed (per-emitter own-stream rule — the
/// deterministic core's pattern: a fixed seed produces byte-identical
/// streams on every platform).
pub const RAIN_SEED: u64 = 0xDEADBEEF;

/// The ripple policy's fixed constants (the demo authors these; the
/// engine carries no ripple numbers). The derivations:
/// lifetime 2.5 s and speed 0.8 m/s give a max ring radius of
/// 2.0 m; the width 0.35 m covers the residual landing drift (the
/// impact XZ is computed at the plane crossing between integration
/// endpoints — finding 12's correction — so the old landing-column
/// approximation is retired); amplitude 1.0 is the unit scale the
/// renderer's gains consume. THE FINDING-12 RE-DERIVATION: at the
/// corrected policy every street-level drop LANDS (the kill-death
/// rate rises from the old design's 1.475/s to the full spawn rate
/// 480/s), so decimation 1 would fill the 32-ring pool in a
/// fraction of a second. Decimation 320 gates the ring births to
/// 480/320 = 1.5/s — the SAME visible ring density the owner
/// walked (the old design's measured 1.475 ring/s), 1.5 × 2.5 ≈
/// 3.8 live rings, 8.5x headroom under the 32 bound, NO LOD.
pub const RIPPLE_POLICY: engine_core::ripple::RipplePolicy = engine_core::ripple::RipplePolicy {
    lifetime: 2.5,
    speed: 0.8,
    width: 0.35,
    amplitude: 1.0,
    decimation: 320,
};

/// The rain coverage per instance — the authored constant that
/// controls the visual density. The demo's rain is translucent
/// streaks, so the alpha is less than 1.0. Measured at 0.35 (the
/// rain-gate's coverage claim).
pub const RAIN_ALPHA: f32 = 0.35;

/// The rain streak material index into the corner table — the
/// recorded design's choice: material 0 (the wet-asphalt noise
/// texture — a seeded value-noise tile whose ALPHA shape reads as
/// a streak; the corner table has THREE entries, index-stable:
/// 0 asphalt, 1 facade, 2 base).
pub const RAIN_MATERIAL: u32 = 0;

/// The rain layer for one corner frame: the emitter, the retained
/// instance buffer, the census accumulators, and (Era-2 package C
/// recursive item 1) the ripple field every kill-plane death feeds.
///
/// CONSTRUCTION:
/// - `Weather::rain(1.0)` — the flag's intensity.
/// - Fixed authored seed: `RAIN_SEED`.
/// - Warm start: 132 ticks at dt = 1/60 against the route's first
///   camera position — the first presented frame is already wet.
/// - Volume: the frustum AABB intersected with the scene AABB.
/// - The ripple field: constructed with `RIPPLE_POLICY`, every
///   warm kill death observed, the field ticked per warm tick (the
///   same pinned composition as the campaign).
///
/// PER FRAME (update-then-present, dt = 1/60 exactly — the
/// finding-18 corrective: rain and rings present from ONE
/// completed tick):
/// 1. Volume refresh, LIKE-FOR-LIKE (the corrective): the quantized
///    listener box around THIS frame's camera eye (a fixed
///    half-extent box, x/z-quantized to 4 m cells), from which the
///    BAND is re-derived (rain_emitter_config's own form — the
///    spawn band ABOVE the observed column) and compared against
///    the emitter's config volume; a change installs the fresh
///    BAND, never the observed box (the landed defect installed the
///    observed box once and collapsed the band to eye level — the
///    corrective's target; see update_and_build's step 1). The
///    WORLD RECEIVER PLANE never refreshes — it is the scene's
///    street level, fixed for the layer's whole life (finding 12:
///    climbing moves only the band, never the receiver).
/// 2. THE TICK: the pinned composition —
///    `emitter.tick_observing_kill_deaths(1.0/60.0, ripple
///    observe)` (arithmetic-identical to `tick`, the core's
///    one-code-path delegation; every kill-plane death observes its
///    ripple ring at the IMPACT crossing) followed by
///    `ripples.tick(1.0/60.0)`; the reports carried into the
///    census.
/// 3. THE PACK (a pure read — no simulation advance during it):
///    for every POST-TICK `particles()` entry — `pos: p.position`,
///    `turn:` the billboard turn (t = +yaw·65536/τ, computed ONCE
///    per build — see [`yaw_to_turn`]), `scale: 1.0`, `tint:` the
///    atmosphere-derived color for THIS frame's cycle position,
///    `pose_index: 0`, `alpha: RAIN_ALPHA`.
/// 4. Render with `ProxyPopulation { …, rain: &layer.instances, … }`
///    — the drops, the billboards' turn, and the ripple uniform all
///    carry the SAME completed tick's state (the demo calls
///    update_and_build BEFORE building the frame population; the
///    crowd's own locomotion tick stays render-then-step — the two
///    disciplines are deliberately different, documented at the
///    call sites).
pub struct RainLayer {
    emitter: Emitter,
    instances: Vec<ProxyInstance>,
    spawned: u64,
    died: u64,
    refused: u64,
    final_active: u32,
    volume_refresh_count: u64,
    /// The ripple field (Era-2 package C recursive item 1): every
    /// kill-plane death observes a ring (the landing-column
    /// approximation), the field ticks after the emitter — the
    /// PINNED COMPOSITION ORDER ripple.rs's campaign digest rests
    /// on (emitter first with mid-tick observation, then the field
    /// tick; a ring born this tick is dt old at the tick's end).
    ripples: engine_core::ripple::RippleField,
    /// The ripple census (the rain census's discipline — accumulated
    /// over the layer's WHOLE lifetime including the warm start;
    /// kept in lockstep with the field's own counters by delta).
    ripple_spawned: u64,
    ripple_expired: u64,
    ripple_refused: u64,
    /// The KILL-DEATH census (finding 12's measured contact rate):
    /// every death whose kill condition held — a real landing on
    /// the world receiver plane, as opposed to a midair lifetime
    /// expiry (the honest absence). The `died` census above counts
    /// BOTH; this counter separates the contacts.
    kill_deaths: u64,
}

impl RainLayer {
    /// Constructs the rain layer with warm start (132 ticks at
    /// dt=1/60 against the route's first camera position). The first
    /// presented frame is already wet.
    pub fn new(camera_position: [f32; 3]) -> Self {
        Self::with_ripple_policy(
            camera_position,
            RIPPLE_POLICY,
            demo_clock_dt().as_secs_f32(),
        )
    }

    /// The A/B-diagnostic constructor (the ripple-visibility
    /// harness): the SAME layer construction with ONE policy knob
    /// varied (the decimation — the admission rate), everything
    /// else identical (the seed, the volume, the warm start, the
    /// composition order).
    pub fn with_ripple_policy(
        camera_position: [f32; 3],
        policy: engine_core::ripple::RipplePolicy,
        dt: f32,
    ) -> Self {
        // The initial volume: the frustum AABB intersected with the
        // scene AABB. The camera is at `camera_position`, looking at
        // the corner scene's center. The frustum half-extent in X/Z
        // at near plane: d/proj[0][0] and d/proj[1][1] (perspective
        // focal lengths). For simplicity, we use the quantized
        // listener box around the eye (the documented simpler
        // alternative).
        let volume = quantized_listener_volume(camera_position);

        // Build the emitter config from the policy.
        let weather = Weather::rain(1.0);
        let config =
            engine_core::weather::rain_emitter_config(weather, &RAIN_POLICY, &volume, RAIN_SEED)
                .expect("the rain config validates");

        let mut emitter = Emitter::new(config).expect("the emitter config is valid");
        let mut ripples =
            engine_core::ripple::RippleField::new(policy).expect("the ripple policy validates");

        // Warm start: 132 ticks at dt = 1/60 — the first presented
        // frame is already wet. THE WARM CENSUS IS COUNTED (the
        // layer's conservation law covers its WHOLE lifetime — the
        // warm-start births and deaths are real campaign events;
        // the exit summary's numbers include them). The emitter
        // ticks through tick_observing_kill_deaths — arithmetic-
        // identical to tick (the core's one-code-path delegation)
        // — so every warm kill-plane death ALSO observes its ripple
        // ring (the landing-column approximation), then the field
        // ticks: THE PINNED COMPOSITION ORDER (emitter first with
        // mid-tick observation, then the field tick; a ring born
        // this tick is dt old at the tick's end — ripple.rs's
        // campaign digest depends on this exact order).
        let mut spawned: u64 = 0;
        let mut died: u64 = 0;
        let mut refused: u64 = 0;
        let mut previous_active: u64 = 0;
        let mut warm_refusals: u64 = 0;
        let mut warm_kill_deaths: u64 = 0;
        for _ in 0..132 {
            let report = emitter
                .tick_observing_kill_deaths(dt, |position| {
                    warm_kill_deaths += 1;
                    // The typed spawn refusal, counted at the site
                    // (never swallowed — unreachable at this policy's
                    // 8.5x headroom, counted anyway; the field's own
                    // refused_total counts the same events, so the two
                    // are equal by construction).
                    if let Some(overflow) = ripples.observe_death(position) {
                        warm_refusals += u64::from(overflow.dropped);
                    }
                })
                .expect("the warm-start dt is finite and nonnegative");
            record_census(&mut spawned, &mut died, &mut refused, &report);
            previous_active = u64::from(report.active);
            ripples
                .tick(dt)
                .expect("the warm-start dt is finite and nonnegative");
        }

        let warm_active = previous_active as u32;
        // The ripple census starts at the warm window's totals —
        // the field's own whole-lifetime counters (spawned/expired)
        // and the site's refusal count (equal to refused_total by
        // construction: one increment per typed refusal on both
        // channels).
        let ripple_spawned = ripples.spawned();
        let ripple_expired = ripples.expired();
        Self {
            emitter,
            instances: Vec::with_capacity(1024),
            spawned,
            died,
            refused,
            final_active: warm_active,
            volume_refresh_count: 0,
            ripples,
            ripple_spawned,
            ripple_expired,
            ripple_refused: warm_refusals,
            kill_deaths: warm_kill_deaths,
        }
    }

    /// Updates the volume, ticks the emitter and ripple field, then
    /// builds the instances from the COMPLETED tick (the
    /// finding-18 corrective: the pack is a pure read AFTER the
    /// simulation advance, never before it — the presented drops,
    /// their billboard turn, and the ripple field all carry ONE
    /// tick's state). `camera_position` is the listener's world
    /// position (drives the observation column and, through it,
    /// the overhead band — never the world receiver plane);
    /// `camera_yaw` is the listener's yaw (for the billboard turn,
    /// computed once per build); `day_night` is the cycle position
    /// for the tint derivation. `dt` is the fixed-timestep delta
    /// (typically 1/60). Returns the tick report for census
    /// accumulation.
    pub fn update_and_build(
        &mut self,
        camera_position: [f32; 3],
        camera_yaw: f32,
        day_night: f32,
        dt: f32,
    ) -> engine_core::particle::TickReport {
        // 1. Volume refresh, LIKE-FOR-LIKE (the corrective): the
        //    emitter's config volume is the BAND rain_emitter_config
        //    derives from an observed box (the spawn band ABOVE the
        //    observed column — weather.rs's own contract); the
        //    refresh must compare and install the band derived from
        //    the FRESH observed box, never the observed box itself.
        //    THE LANDED DEFECT this corrects: the band-vs-observed
        //    comparison always differed, and the first refresh
        //    installed the OBSERVED box as the spawn volume — the
        //    rain then spawned at eye level, the kill-death
        //    fraction jumped, and the ripple pool saturated (the
        //    stop's evidence). The refresh triggers exactly when
        //    the quantized observed box changes (the same 4 m cell
        //    crossings as before); only the INSTALLED volume is
        //    corrected. THE RECEIVER PLANE is never part of the
        //    refresh: `set_volume` keeps the construction kill_y
        //    (the world's street level — finding 12's separation),
        //    so a climbing camera raises the band and leaves the
        //    ground where it is.
        let observed = quantized_listener_volume(camera_position);
        let band = engine_core::weather::rain_emitter_config(
            Weather::rain(1.0),
            &RAIN_POLICY,
            &observed,
            RAIN_SEED,
        )
        .expect("the band derivation validates")
        .volume;
        if self.emitter.config().volume != band {
            self.emitter
                .set_volume(band)
                .expect("the volume refresh is valid");
            self.volume_refresh_count += 1;
        }

        // 2. THE TICK (update first — finding 18): the pinned
        //    composition advances the simulation one completed step
        //    BEFORE the pack reads it. THE PINNED COMPOSITION ORDER
        //    (ripple.rs's campaign digest): the emitter ticks FIRST
        //    through tick_observing_kill_deaths — arithmetic-identical
        //    to tick (the core's one-code-path delegation) — with
        //    every kill-plane death observing its ripple ring at
        //    the IMPACT CROSSING position AS IT HAPPENS (mid-tick,
        //    in sweep order), THEN the ripple field ticks (every
        //    live ring ages one dt, the newborn included: a ring
        //    born this tick is dt old at the tick's end). The typed
        //    ripple spawn refusal is counted at the site (never
        //    swallowed); the ripple census reads the field's own
        //    counters (spawned/expired deltas around the composed
        //    tick) and the ripple report's refusal carry is
        //    asserted in agreement with the site count.
        let ripple_spawned_before = self.ripples.spawned();
        let ripple_expired_before = self.ripples.expired();
        let mut ripple_refused_this_tick: u32 = 0;
        let report = self
            .emitter
            .tick_observing_kill_deaths(dt, |position| {
                self.kill_deaths += 1;
                if let Some(overflow) = self.ripples.observe_death(position) {
                    ripple_refused_this_tick += overflow.dropped;
                }
            })
            .expect("the demo's dt is finite and nonnegative");
        let ripple_report = self
            .ripples
            .tick(dt)
            .expect("the demo's dt is finite and nonnegative");
        assert_eq!(
            ripple_report.refused, ripple_refused_this_tick,
            "the ripple report's refusal carry must agree with the site's count"
        );
        self.ripple_spawned += self.ripples.spawned() - ripple_spawned_before;
        self.ripple_expired += self.ripples.expired() - ripple_expired_before;
        self.ripple_refused += u64::from(ripple_refused_this_tick);
        // THE CENSUS (finding 16's correction): the shared
        // accumulator's law — the emitter's admitted count and its
        // own death observation, consumed directly.
        record_census(
            &mut self.spawned,
            &mut self.died,
            &mut self.refused,
            &report,
        );
        self.final_active = report.active;

        // 3. THE PACK — a pure read of the COMPLETED tick (finding
        //    18: no simulation advance during the read; the
        //    instances, their turn, and the ripple uniform all
        //    carry this tick). The turn and tint are computed ONCE
        //    per build (finding 13: the turn is a per-population
        //    constant, never a per-particle derivation).
        let turn = yaw_to_turn(camera_yaw);
        let tint = atmosphere_ambient(day_night);
        let particles = self.emitter.particles();
        self.instances.clear();
        self.instances.reserve(particles.len());
        for p in particles {
            self.instances.push(ProxyInstance {
                transform: InstanceTransform {
                    pos: p.position,
                    // The billboard turn: the derived t = +yaw (the
                    // quad's mesh-authored +Z normal faces the
                    // camera — see yaw_to_turn's derivation).
                    turn,
                    scale: 1.0,
                },
                tint,
                alpha: RAIN_ALPHA,
                pose_index: 0,
            });
        }

        report
    }

    /// The world receiver plane this layer's rain dies at (the
    /// emitter config's kill plane — a world constant for the
    /// layer's whole life; the finding-12 tests assert it never
    /// moves while the camera climbs).
    pub fn receiver_plane(&self) -> f32 {
        self.emitter
            .config()
            .kill_y
            .expect("the rain config always carries the world receiver plane")
    }

    /// The whole-lifetime KILL-DEATH count (finding 12's measured
    /// contact rate): every real landing on the world receiver
    /// plane, the warm start's included — the midair lifetime
    /// expiries (the honest absence) are excluded (they live in
    /// the `died` census and the exit summary's spawn/death
    /// conservation, not here).
    pub fn kill_deaths(&self) -> u64 {
        self.kill_deaths
    }

    /// Returns the population for rendering.
    pub fn population(&self) -> engine_renderer::ProxyPopulation<'_> {
        engine_renderer::ProxyPopulation {
            npc: &[],
            npc_poses: &[],
            npc_material: 0,
            vehicle: &[],
            vehicle_material: 0,
            rain: &self.instances,
            rain_material: RAIN_MATERIAL,
        }
    }

    /// The exit summary's rain census line.
    pub fn exit_summary(&self) -> String {
        format!(
            "rain: spawned={} died={} refused={} final-active={} volume-refreshes={}",
            self.spawned, self.died, self.refused, self.final_active, self.volume_refresh_count
        )
    }

    /// The exit summary's ripple census line (the rain line's own
    /// shape): the layer's whole-lifetime counters and the live
    /// ring count at exit.
    pub fn ripple_exit_summary(&self) -> String {
        format!(
            "ripples: spawned={} expired={} refused={} active={}",
            self.ripple_spawned,
            self.ripple_expired,
            self.ripple_refused,
            self.ripples.active_count()
        )
    }

    /// The live ripple field (the renderer's set_ripple_field input —
    /// the demo wiring reads it per frame under render-then-step).
    pub fn ripple_field(&self) -> &engine_core::ripple::RippleField {
        &self.ripples
    }

    /// Folds the emitter state for the digest pin.
    pub fn fold_state(&self) -> u64 {
        let mut hash = engine_core::fingerprint::Fingerprinter::new();
        self.emitter.fold_state(&mut hash);
        hash.finish()
    }

    /// Folds the ripple field's state for the digest pin (the
    /// field's own fold_state — the campaign digest's tool).
    pub fn fold_ripple_state(&self) -> u64 {
        let mut hash = engine_core::fingerprint::Fingerprinter::new();
        self.ripples.fold_state(&mut hash);
        hash.finish()
    }
}

/// THE CENSUS ACCUMULATOR (finding 16's correction — one law, both
/// windows): attempted = admitted + refused, and admitted = expired
/// plus live for an initially empty pool. The emitter's `spawned` IS
/// the admitted count (incremented only on admission) and `deaths`
/// is the emitter's own observation — never the retired
/// `spawned − refused` derivation that subtracted refusals from the
/// already-admitted count before deriving deaths and underflowed on
/// a zero-admission saturation tick (0 admitted − ≥1 refused).
fn record_census(
    spawned: &mut u64,
    died: &mut u64,
    refused: &mut u64,
    report: &engine_core::particle::TickReport,
) {
    *spawned += u64::from(report.spawned);
    *died += u64::from(report.deaths);
    *refused += report.overflow.map(|o| u64::from(o.dropped)).unwrap_or(0);
}

/// The observed column's height above the eye (the OBSERVATION
/// volume's y-extent — the first of finding 12's three separated
/// volumes): what the player watches falling. The SPAWN BAND rides
/// its ceiling (see [`RAIN_POLICY`]'s derivation), and the WORLD
/// RECEIVER PLANE is elsewhere entirely (the scene's street level
/// — the kill plane, never derived from this box).
const OBSERVED_COLUMN_HEIGHT: f32 = 8.0;

/// The quantized listener volume: a fixed half-extent box around the
/// eye, x/z-quantized to 4 m cells so the field does not slide
/// continuously. The half-extent is 20 m (the corner scene's width);
/// the y-extent is [`OBSERVED_COLUMN_HEIGHT`] above the eye (the
/// observed column — the band derivation's floor input, and nothing
/// else: the kill plane is the policy's world receiver, not this
/// box's floor).
fn quantized_listener_volume(eye: [f32; 3]) -> SpawnVolume {
    // Quantize x/z to 4 m cells.
    let quantize = |v: f32| -> f32 { (v / 4.0).round() * 4.0 };
    let qx = quantize(eye[0]);
    let qz = quantize(eye[2]);
    SpawnVolume {
        min: [qx - 20.0, eye[1], qz - 20.0],
        max: [qx + 20.0, eye[1] + OBSERVED_COLUMN_HEIGHT, qz + 20.0],
    }
}

/// Converts the camera's yaw (radians, math.rs's convention: zero
/// looks down −Z, positive rotates toward −X) to the rain quad's
/// billboard turn (u16 turn fractions, `instance_columns`'
/// convention). THE DERIVATION (physical, verified against both
/// matrices — the audit's finding 13 corrective: the sign was
/// backwards, turning the streaks EDGE-ON at diagonal headings
/// where the dot product with the intended normal collapses to
/// cos 2θ = 0 at 45°): `instance_columns` maps the streak mesh's
/// +Z normal to the world direction (sin t, 0, cos t) at turn t;
/// the camera at yaw θ looks along (−sin θ, 0, −cos θ)
/// (math.rs's `camera_forward` — R·(0,0,−1) = −(column 2)), so the
/// quad faces the camera when its normal is the horizontal
/// BACKWARD vector (sin θ, 0, cos θ) — i.e. **t = +θ**. NO
/// quarter-turn offset: at yaw 0 the camera looks down −Z and the
/// mesh-authored +Z normal already faces it. The quantizer is
/// correctly-rounded (`.round()` — deterministic IEEE) with the
/// total saturating cast and `rem_euclid` (negative yaws wrap
/// forward) — the libm-free law on the packed path: no runtime
/// sin/cos anywhere near the instance bytes.
pub fn yaw_to_turn(yaw: f32) -> u16 {
    const TAU: f32 = 2.0 * std::f32::consts::PI;
    const SCALE: f32 = 65536.0 / TAU;
    let turn = (yaw * SCALE).round();
    let turn = (turn as i64).rem_euclid(65536);
    turn as u16
}

/// The atmosphere ambient color for the given day/night position.
/// Day is t=0, night is t=1. The ambient is the day/night preset's
/// `ambient` field from the LightParams.
fn atmosphere_ambient(day_night: f32) -> [f32; 3] {
    use engine_renderer::sky::atmosphere_at;
    let atmosphere = atmosphere_at(day_night);
    atmosphere.light.ambient
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corner_timing::demo_clock_dt;
    use engine_renderer::proxy_instances::instance_columns;

    /// The campaign's fixed first eye (the route's entry waypoint —
    /// the construction camera).
    const FIRST_EYE: [f32; 3] = [-5.0, 1.7, 13.0];

    /// The warm-start known answer (RE-MEASURED 2026-11-25 for the
    /// finding-12 policy corrective — the old design's 5 warm rings
    /// moved: the receiver plane is now the world's street level
    /// and every street-level drop lands, so the warm window's
    /// kill deaths are the landed share of its 1056 spawns, the
    /// ring births the decimation-320 gate over them). THE PINNED
    /// NUMBERS ARE MEASURED; a different number is a
    /// STOP-and-report, never a re-pin.
    #[test]
    fn ripple_warm_start_known_answer() {
        let layer = RainLayer::new(FIRST_EYE);
        println!(
            "warm start measured: active rings {}, spawned {}, expired {}, refused {}, \
             emitter active {}",
            layer.ripple_field().active_count(),
            layer.ripple_spawned,
            layer.ripple_expired,
            layer.ripple_refused,
            layer.final_active
        );
        assert_eq!(
            layer.ripple_field().active_count(),
            WARM_ACTIVE_RINGS,
            "the measured warm-start ring count"
        );
        assert_eq!(
            layer.ripple_spawned, WARM_RIPPLE_SPAWNED,
            "the warm census counts them"
        );
        assert_eq!(
            layer.ripple_expired, WARM_RIPPLE_EXPIRED,
            "2.2 s against the 2.5 s lifetime: the measured expiry"
        );
        assert_eq!(
            layer.ripple_refused, 0,
            "the warm window never fills the pool (decimation 320: 8.5x headroom)"
        );
        assert_eq!(layer.ripple_field().refused_total(), 0);
        // Every warm ring is younger than the lifetime.
        for ring in layer.ripple_field().rings() {
            assert!(ring.age < RIPPLE_POLICY.lifetime);
            assert_eq!(ring.amplitude, RIPPLE_POLICY.amplitude);
        }
        // THE RECEIVER (finding 12): the layer's kill plane is the
        // world's street level from construction on.
        assert_eq!(layer.receiver_plane(), engine_renderer::lit_scene::GROUND_Y);
    }

    /// The warm-start measured actuals under the finding-12 policy
    /// (see the test above for the re-pin history): 1 live ring,
    /// 1 ring born (the decimation-320 gate over the warm window's
    /// 427 landings), nothing expired (2.2 s < 2.5 s).
    const WARM_ACTIVE_RINGS: usize = 1;
    const WARM_RIPPLE_SPAWNED: u64 = 1;
    const WARM_RIPPLE_EXPIRED: u64 = 0;

    /// THE CAMPAIGN DIGEST PIN + THE COMBINED CENSUS (the landed
    /// record). RE-PINNED with cause (the finding-12 rain-contact
    /// corrective, 2026-11-25): the rain now falls to the WORLD
    /// RECEIVER (street level) — the band is the compatible 4 m
    /// over the 8 m observed column, the lifetime [1.65, 2.15]
    /// carries every street-level drop to the ground, the impact
    /// XZ is the interpolated plane crossing, and the ripple
    /// decimation is 320 over the ~480/s real landings. The census
    /// and the digest moved from the pre-corrective actuals
    /// (ripple_spawned 18, digest 6_501_874_864_611_424_535 — the
    /// old pin's history stays in git); the numbers below are the
    /// POST-CORRECTIVE landed actuals.
    ///
    /// THE CONVERGENCE (the volume-refresh corrective's proof,
    /// asserted below): at the fixed camera the quantized observed
    /// box never changes, so the like-for-like refresh never fires
    /// (`volume_refresh_count == 0` over the whole campaign) and the
    /// layer's emitter state stays the construction band throughout
    /// — the layer's campaign IS engine-core's demo-fixture
    /// campaign, and the digest below is the SAME u64 the core
    /// fixture's own `digest_pin` pins (one campaign, two proofs,
    /// one digest; the core's const is test-module-private in
    /// another crate, so the two tests pin the same measured actual
    /// independently — a mismatch between them is an investigation,
    /// never a silent divergence).
    ///
    /// The census: `ripple_spawned` is the decimation-320 gate over
    /// the campaign's real landings (the warm start's rings
    /// included) and `refused == 0` is the pool-bound proof at the
    /// demo boundary (1.5 ring/s x 2.5 s ~ 3.8 live rings, 8.5x
    /// headroom under the 32 bound). A moved number or digest is an
    /// investigation, never a re-pin without a cause.
    #[test]
    fn ripple_campaign_digest_pin() {
        let mut layer = RainLayer::new(FIRST_EYE);
        let mut hasher = engine_core::fingerprint::Fingerprinter::new();
        for tick in 1..=600u32 {
            layer.update_and_build(FIRST_EYE, 0.0, 0.0, demo_clock_dt().as_secs_f32());
            if tick % 60 == 0 {
                layer.ripple_field().fold_state(&mut hasher);
            }
        }
        println!(
            "campaign measured: ripple spawned {} expired {} refused {} died {} spawned {} \
             final active {} digest {}",
            layer.ripple_spawned,
            layer.ripple_expired,
            layer.ripple_refused,
            layer.died,
            layer.spawned,
            layer.final_active,
            hasher.finish()
        );
        // The combined census (the post-corrective landed actual).
        assert_eq!(
            layer.ripple_spawned, CAMPAIGN_RIPPLE_SPAWNED,
            "the decimation-320 ring total over the whole 132+600 window (the warm \
             start's rings included)"
        );
        assert_eq!(
            layer.ripple_refused, 0,
            "the pool-bound proof at the demo boundary (1.5/s x 2.5 s ~ 3.8 << 32)"
        );
        // THE CONVERGENCE's mechanical half: the fixed camera never
        // crosses a 4 m cell, so the like-for-like refresh never
        // fires and the campaign ran on the construction band.
        assert_eq!(
            layer.volume_refresh_count, 0,
            "the fixed-camera campaign never refreshes the volume (the convergence \
             precondition)"
        );
        // The digest pin: the field folded at every 60th campaign
        // tick (campaign ticks 60, 120, ..., 600 — the SAME sampling
        // as engine-core's fixture digest_pin; the accumulated
        // hasher, never a single final fold). MEASURED 2026-11-25,
        // post-corrective — the identical u64 to the core fixture's
        // PINNED_CAMPAIGN_DIGEST.
        assert_eq!(
            hasher.finish(),
            PINNED_CAMPAIGN_DIGEST,
            "the ripple campaign digest moved, or diverged from engine-core's fixture \
             pin (one campaign, two proofs, one digest): an investigation, never a \
             silent repin"
        );
    }

    /// The campaign's measured actuals under the finding-12 policy
    /// (see the digest test above for the re-pin history): 16 rings
    /// born over the 132+600 window (the decimation gate over 5227
    /// landings), 12 expired, 4 live at exit; the digest is the
    /// SAME u64 engine-core's fixture digest_pin pins.
    const CAMPAIGN_RIPPLE_SPAWNED: u64 = 16;
    const PINNED_CAMPAIGN_DIGEST: u64 = 13_157_441_224_951_405_170;

    /// SUSTAINED NONZERO REAL CONTACTS AT NATURAL INTENSITY (the
    /// finding-12 deciding test): at the corrected policy every
    /// street-level drop reaches the world receiver before expiry —
    /// MEASURED 2026-11-25: 4800 contacts over the 600-tick window,
    /// 480.0/s — EXACTLY the 480/s spawn rate at the steady state
    /// (every drop born lands; the warm start itself landed 427).
    /// The pre-corrective campaign measured 18 kill deaths TOTAL
    /// (the 0.34% tail); the honest-absence regime would measure
    /// zero. The floor holds a margin under the measurement.
    #[test]
    fn contacts_sustained_at_natural_intensity() {
        let mut layer = RainLayer::new(FIRST_EYE);
        let kill_deaths_at_window_start = layer.kill_deaths();
        for _ in 0..600 {
            layer.update_and_build(FIRST_EYE, 0.0, 0.0, demo_clock_dt().as_secs_f32());
        }
        let contacts = layer.kill_deaths() - kill_deaths_at_window_start;
        let seconds = 600.0 / 60.0;
        println!(
            "measured contacts: {contacts} over the 600-tick window ({:.1}/s against the \
             480/s spawn rate; final live pool {})",
            contacts as f64 / seconds,
            layer.final_active
        );
        assert!(
            contacts > CONTACTS_FLOOR,
            "sustained real contacts at natural intensity: {contacts} over 10 s (the \
             pre-corrective campaign measured 18 kill deaths total; midair expiry \
             would measure zero)"
        );
        // The contacts are REAL ground hits: the receiver stayed at
        // street level and the ripple pool never refused.
        assert_eq!(layer.receiver_plane(), engine_renderer::lit_scene::GROUND_Y);
        assert_eq!(
            layer.ripple_refused, 0,
            "the decimation gate holds the pool"
        );
    }

    /// The measured-contact floor (a margin under the campaign's
    /// own measured actual — every street drop lands, so the count
    /// is the spawned 4800 minus the ~624 live pool and the ramp).
    const CONTACTS_FLOOR: u64 = 4_000;

    /// THE THREE-VOLUME SEPARATION against ONE FIXED GROUND PLANE
    /// (finding 12's deciding test — stationary, translated, and
    /// vertically-moving cameras): the OBSERVED column and the
    /// overhead BAND follow the eye (a translated camera crosses 4
    /// m cells and refreshes the band; a climbing camera raises it
    /// every tick), while the WORLD RECEIVER PLANE never moves —
    /// contacts keep landing at street level under the stationary
    /// and translated cameras, and the climb ends in the honest
    /// absence (the band outgrows the lifetime: midair expiry, zero
    /// contacts).
    #[test]
    fn receiver_plane_fixed_across_stationary_translated_and_climbing_cameras() {
        // --- Stationary: the fixed camera never refreshes, the
        //     contacts land, and the receiver stays at street level.
        let mut layer = RainLayer::new(FIRST_EYE);
        for _ in 0..120 {
            layer.update_and_build(FIRST_EYE, 0.0, 0.0, demo_clock_dt().as_secs_f32());
            assert_eq!(
                layer.receiver_plane(),
                engine_renderer::lit_scene::GROUND_Y,
                "the receiver is the world's street level, every tick"
            );
        }
        assert_eq!(
            layer.volume_refresh_count, 0,
            "the stationary camera never refreshes"
        );
        assert!(layer.died > 0, "the stationary camera sees real contacts");

        // --- Translated: the eye crosses 4 m cells in x/z (the y
        //     held at street level), the band refreshes behind it,
        //     and the receiver is untouched — the contacts continue
        //     on the SAME fixed ground plane.
        for step in 0..120u32 {
            let eye = [
                FIRST_EYE[0] + step as f32 * 0.5,
                FIRST_EYE[1],
                FIRST_EYE[2] - step as f32 * 0.5,
            ];
            layer.update_and_build(eye, 0.0, 0.0, demo_clock_dt().as_secs_f32());
            assert_eq!(
                layer.receiver_plane(),
                engine_renderer::lit_scene::GROUND_Y,
                "step {step}: translation never moves the receiver"
            );
        }
        assert!(
            layer.volume_refresh_count > 0,
            "the translated camera crosses 4 m cells and refreshes the band \
             ({} refreshes)",
            layer.volume_refresh_count
        );
        assert!(
            layer.died > 0,
            "the translated camera still sees real contacts"
        );

        // --- Vertically moving: the eye climbs the free-look band
        //     (1.7 -> 30 m). The band rises with it (a refresh every
        //     tick — the observed column's y is not quantized), the
        //     receiver NEVER moves, and past the lifetime's reach
        //     the contacts stop entirely: drops born in the raised
        //     band expire midair (the honest absence — zero
        //     contacts, never a ring).
        let mut climbing = RainLayer::new(FIRST_EYE);
        let mut died_high_window = 0u64;
        let mut rings_high_window = 0u64;
        for step in 0..400u32 {
            let eye = [0.0, 1.7 + step as f32 * (30.0 - 1.7) / 400.0, 0.0];
            climbing.update_and_build(eye, 0.0, 0.0, demo_clock_dt().as_secs_f32());
            assert_eq!(
                climbing.receiver_plane(),
                engine_renderer::lit_scene::GROUND_Y,
                "climb step {step}: the world's ground never moves"
            );
            if step == 300 {
                // Past the lifetime's reach: the eye is above 21.5 m,
                // the band floor above 29.5 m — a fall no lifetime
                // survives, and the last low-band cohort (spawned
                // before step ~131) has drained its <= 2.15 s
                // lifetime. From here on: midair expiry only.
                died_high_window = climbing.kill_deaths();
                rings_high_window = climbing.ripple_spawned;
            }
        }
        // The high window (the last 100 ticks, the eye above ~21.9 m):
        // the street cohort has drained, the raised band's drops
        // expire midair — ZERO new contacts and ZERO new rings.
        assert_eq!(
            climbing.kill_deaths() - died_high_window,
            0,
            "the honest absence: past the lifetime's reach the high-band drops \
             expire midair (zero contacts)"
        );
        assert_eq!(
            climbing.ripple_spawned - rings_high_window,
            0,
            "no ring is born from a midair expiry (the observer stays silent)"
        );
        assert!(
            climbing.final_active > 0,
            "the rain still FALLS visibly at the climb ceiling — the drops expire \
             midair, they do not vanish at birth"
        );
        assert_eq!(
            climbing.refused, 0,
            "the climb ceiling's lifetime-capped pool (480 x 1.9 = 912) stays under \
             the 1024 bound"
        );
    }

    /// MIDAIR EXPIRY IS THE HONEST ABSENCE (the finding-12 edge,
    /// isolated): a layer held at the free-look eye ceiling (30 m —
    /// the band at [38, 42] m, a 4.3–4.7 s fall no lifetime
    /// survives) drains its street cohort and then produces ZERO
    /// contacts — no kill deaths, no rings — while the streaks
    /// keep falling (lifetime deaths, not landings).
    #[test]
    fn midair_expiry_produces_zero_contacts() {
        let mut layer = RainLayer::new(FIRST_EYE);
        let high_eye = [0.0, 30.0, 0.0];
        // The drain: the street cohort lands within its remaining
        // fall (<= 1.55 s = 93 ticks).
        for _ in 0..150 {
            layer.update_and_build(high_eye, 0.0, 0.0, demo_clock_dt().as_secs_f32());
        }
        let kill_deaths_after_drain = layer.kill_deaths();
        let rings_after_drain = layer.ripple_spawned;
        assert!(
            kill_deaths_after_drain > 0,
            "the street cohort drained onto the receiver"
        );
        // The steady high regime: 300 ticks, zero contacts.
        for _ in 0..300 {
            layer.update_and_build(high_eye, 0.0, 0.0, demo_clock_dt().as_secs_f32());
        }
        assert_eq!(
            layer.kill_deaths(),
            kill_deaths_after_drain,
            "every high-band drop expires midair: zero contacts (the honest absence)"
        );
        assert_eq!(
            layer.ripple_spawned, rings_after_drain,
            "the observer stayed silent for every midair expiry"
        );
        assert!(
            layer.final_active > 0,
            "the high-band drops live out their lifetimes midair ({} active)",
            layer.final_active
        );
        assert_eq!(layer.refused, 0, "the lifetime-capped pool never overflows");
    }

    /// The census conservation over the 600-tick campaign (the
    /// materialization conservation, engine-core's own law): every
    /// decimation-gated spawn attempt is exactly one of {live ring,
    /// expired ring, typed refusal} — `spawned + refused_total ==
    /// expired + active + refused_total` (equivalently
    /// `spawned == expired + active`; the brief's literal right
    /// side adds refused, which the happy path zeroes — engine-
    /// core's census_conservation documents the reduction), sampled
    /// at every 60th tick. The layer's census must also mirror the
    /// field's own counters at every sample (the
    /// delta-accumulation proof). The `refused == 0` half is the
    /// pool-bound proof on the post-corrective landed path.
    #[test]
    fn ripple_census_conservation() {
        let mut layer = RainLayer::new(FIRST_EYE);
        for tick in 1..=600u32 {
            layer.update_and_build(FIRST_EYE, 0.0, 0.0, demo_clock_dt().as_secs_f32());
            if tick % 60 == 0 {
                let attempts = layer.ripple_spawned + layer.ripple_refused;
                let outcomes = layer.ripple_expired
                    + layer.ripple_field().active_count() as u64
                    + layer.ripple_refused;
                assert_eq!(
                    attempts, outcomes,
                    "tick {tick}: every gated attempt is a live ring, an expiry, or a \
                     typed refusal"
                );
                // The layer's census mirrors the field's counters.
                assert_eq!(layer.ripple_spawned, layer.ripple_field().spawned());
                assert_eq!(layer.ripple_expired, layer.ripple_field().expired());
                assert_eq!(layer.ripple_refused, layer.ripple_field().refused_total());
            }
        }
        assert_eq!(
            layer.ripple_refused, 0,
            "the pool-bound proof on the post-corrective landed path (the decimation \
             gate's 8.5x headroom)"
        );
    }

    /// The warm-start + campaign determinism: double-run identical,
    /// AND the final digest pinned (the derivation: seed RAIN_SEED,
    /// RAIN_POLICY, the fixed camera at the first route eye, 600
    /// update ticks at dt = 1/60 after the 132-tick warm start).
    /// A moved digest means a math, order, or packing change — an
    /// investigation, never a re-pin without a cause.
    ///
    /// RE-PINNED with cause (the finding-12 rain-contact
    /// corrective, 2026-11-25): the receiver plane moved to the
    /// world's street level, the band to the compatible 4 m over
    /// the 8 m observed column, and the lifetime to [1.65, 2.15] —
    /// the trajectory's spawn draws, residence, and death ticks all
    /// moved, so the digest moved from 1705604326501831793 to the
    /// value below (the old pin's history stays in git). At the
    /// fixed camera the like-for-like refresh never fires — the
    /// campaign is the construction-band trajectory, the same
    /// stream engine-core's demo fixture drives.
    #[test]
    fn warm_start_plus_campaign_determinism() {
        let run = || {
            let mut layer = RainLayer::new(FIRST_EYE);
            for _ in 0..600 {
                layer.update_and_build(FIRST_EYE, 0.0, 0.0, demo_clock_dt().as_secs_f32());
            }
            layer.fold_state()
        };
        let digest_a = run();
        let digest_b = run();
        assert_eq!(digest_a, digest_b, "the campaign double-runs identical");
        println!("measured rain campaign state digest: {digest_a}");
        assert_eq!(
            digest_a, PINNED_FNV,
            "the rain campaign's state digest moved: a math, order, or packing change"
        );
    }

    /// The rain campaign's measured state digest under the
    /// finding-12 policy (see the test above for the re-pin
    /// history).
    const PINNED_FNV: u64 = 6_471_597_869_140_464_520;

    /// The census conservation (the conservation law over the
    /// layer's OWN tracked counters — NOT an identity derived from
    /// itself): attempted = admitted + refused, and admitted = died
    /// plus final-active for an initially empty pool (finding 16's
    /// correction: `spawned` is the emitter's admitted count and
    /// `died` its own death observation, so the identities hold
    /// term-for-term; the authored policy holds refused == 0 — the
    /// street steady state holds ~624 live drops and the climb
    /// ceiling's lifetime cap ~912, both under the 1024 pool bound).
    #[test]
    fn census_conservation() {
        let mut layer = RainLayer::new(FIRST_EYE);
        for _ in 0..600 {
            layer.update_and_build(FIRST_EYE, 0.0, 0.0, demo_clock_dt().as_secs_f32());
        }
        assert_eq!(
            layer.spawned,
            layer.died + u64::from(layer.final_active),
            "admitted == died + final-active (an initially empty pool; the layer's \
             own counters)"
        );
        assert_eq!(
            layer.spawned + layer.refused,
            layer.died + u64::from(layer.final_active) + layer.refused,
            "attempted == admitted + refused (both sides composed of the observable \
             counters)"
        );
        assert_eq!(
            layer.refused, 0,
            "the happy path has no refusals (rate x residence < pool bound at every \
             eye height)"
        );
        assert!(
            layer.died > 0,
            "a 600-tick campaign sees deaths (every street-level drop lands)"
        );
    }

    /// THE SATURATION AND MIXED CENSUS (finding 16's deciding test,
    /// layer side): a full pool that admits ZERO and refuses one
    /// (the retired arithmetic's exact underflow shape — it computed
    /// `admitted − refused` = 0 − 1 and panicked in debug builds,
    /// corrupting the census in release) now records honestly, and
    /// a mixed tick (admissions, refusals, and deaths together)
    /// holds the same identities. The reports are authored directly
    /// (the emitter's report is plain data) and driven through the
    /// SAME accumulator the warm start and the ordinary update use.
    #[test]
    fn census_saturation_and_mixed_ticks_never_underflow() {
        let mut spawned: u64 = 0;
        let mut died: u64 = 0;
        let mut refused: u64 = 0;
        // THE SATURATION TICK: a full pool (active == the bound),
        // zero admissions, one typed refusal, five deaths — the
        // shape the retired `spawned − refused` arithmetic
        // underflowed on (0 − 1).
        let saturation = engine_core::particle::TickReport {
            spawned: 0,
            overflow: Some(engine_core::particle::SpawnOverflow { dropped: 1 }),
            active: engine_core::particle::PARTICLE_POOL_BOUND as u32 - 5,
            deaths: 5,
        };
        record_census(&mut spawned, &mut died, &mut refused, &saturation);
        assert_eq!(spawned, 0, "zero admissions recorded");
        assert_eq!(refused, 1, "the refusal counted, never silent");
        assert_eq!(
            died, 5,
            "the deaths are the emitter's own count (no underflow)"
        );
        // THE PER-TICK IDENTITY holds on the saturation report:
        // active_before + admitted == deaths + active.
        assert_eq!(
            u64::from(saturation.active + saturation.deaths),
            u64::from(engine_core::particle::PARTICLE_POOL_BOUND as u32),
            "the saturation report is internally consistent"
        );
        // THE MIXED TICKS, from an initially empty pool: a fill
        // tick, then a tick that mixes admissions (into freed
        // slots), refusals (above them), and deaths in one report.
        let mut spawned: u64 = 0;
        let mut died: u64 = 0;
        let mut refused: u64 = 0;
        let fill = engine_core::particle::TickReport {
            spawned: 1000,
            overflow: None,
            active: 450,
            deaths: 550,
        };
        record_census(&mut spawned, &mut died, &mut refused, &fill);
        let mixed = engine_core::particle::TickReport {
            spawned: 40,
            overflow: Some(engine_core::particle::SpawnOverflow { dropped: 960 }),
            active: 460,
            deaths: 30,
        };
        // The mixed tick's own per-tick identity:
        // active_before + admitted == deaths + active.
        assert_eq!(
            u64::from(mixed.active + mixed.deaths),
            u64::from(fill.active) + u64::from(mixed.spawned),
            "the mixed report is internally consistent"
        );
        record_census(&mut spawned, &mut died, &mut refused, &mixed);
        assert_eq!(spawned, 1040);
        assert_eq!(died, 580);
        assert_eq!(refused, 960);
        // THE CUMULATIVE IDENTITIES from the empty start:
        // attempted == admitted + refused; admitted == deaths + live.
        assert_eq!(
            spawned + refused,
            1040 + 960,
            "attempted == admitted + refused"
        );
        assert_eq!(
            spawned,
            died + u64::from(mixed.active),
            "admitted == deaths + live (an initially empty pool)"
        );
    }

    /// THE PACK IS THE COMPLETED TICK (finding 18's core invariant:
    /// rain and rings present from ONE completed tick — the pack is
    /// a pure read AFTER the simulation advance). Every tick, the
    /// packed instance count must equal the tick report's
    /// post-tick active count; a pack-before-tick regression (the
    /// pre-corrective order) packs the PRE-tick pool and disagrees
    /// on every tick with births or deaths — at the corrected
    /// policy that is every tick of the steady state.
    #[test]
    fn packed_instances_carry_the_completed_tick() {
        let mut layer = RainLayer::new(FIRST_EYE);
        for tick in 1..=120u32 {
            let report = layer.update_and_build(FIRST_EYE, 0.0, 0.0, demo_clock_dt().as_secs_f32());
            assert_eq!(
                layer.instances.len(),
                report.active as usize,
                "tick {tick}: the packed snapshot IS the completed tick (the landed \
                 drops left the pack the tick they landed)"
            );
        }
    }

    /// A SUDDEN YAW CHANGE repacks the billboards with the NEW yaw
    /// (finding 18's view-staleness witness, layer-level half: the
    /// pack uses THIS call's camera, never the preceding frame's).
    #[test]
    fn sudden_yaw_change_repacks_the_billboards() {
        let mut layer = RainLayer::new(FIRST_EYE);
        let old_yaw = 0.0f32;
        let new_yaw = 0.83f32; // an off-cardinal heading, ~47.6 degrees
        layer.update_and_build(FIRST_EYE, old_yaw, 0.0, demo_clock_dt().as_secs_f32());
        let old_turn = yaw_to_turn(old_yaw);
        assert!(
            layer.instances.iter().all(|i| i.transform.turn == old_turn),
            "the first pack carries the first yaw"
        );
        layer.update_and_build(FIRST_EYE, new_yaw, 0.0, demo_clock_dt().as_secs_f32());
        let new_turn = yaw_to_turn(new_yaw);
        assert_ne!(old_turn, new_turn, "the witness yaws differ");
        assert!(
            layer.instances.iter().all(|i| i.transform.turn == new_turn),
            "every billboard carries the NEW yaw on the same call that advanced the \
             simulation — never the preceding frame's"
        );
    }

    /// A STATIC-NIGHT POSE tints the pack with the night ambient
    /// (finding 18's palette half, layer-level: the pack uses the
    /// ACTIVE atmosphere for this frame — the free-look loop's
    /// rain_t default fix rides on this contract).
    #[test]
    fn static_night_tints_the_pack() {
        let night = atmosphere_ambient(1.0);
        let day = atmosphere_ambient(0.0);
        assert_ne!(
            night, day,
            "the night and day ambients differ (the witness)"
        );
        let mut layer = RainLayer::new(FIRST_EYE);
        layer.update_and_build(FIRST_EYE, 0.0, 1.0, demo_clock_dt().as_secs_f32());
        assert!(
            !layer.instances.is_empty(),
            "the warm-started layer packs streaks on the first update"
        );
        assert!(
            layer.instances.iter().all(|i| i.tint == night),
            "a static-night pose tints every streak with the night ambient (never the \
             day default)"
        );
        // The day control: the same call at t=0 packs the day tint.
        layer.update_and_build(FIRST_EYE, 0.0, 0.0, demo_clock_dt().as_secs_f32());
        assert!(
            layer.instances.iter().all(|i| i.tint == day),
            "the day control packs the day ambient"
        );
    }

    /// THE FALLING-TO-LANDED TRANSITION IN ONE FRAME (finding 18's
    /// single-drop witness, campaign form): on the tick a ring is
    /// BORN, the drop that caused it is already ABSENT from the
    /// packed instances — the drop's disappearance and its ring's
    /// birth present in the SAME frame, from one completed tick.
    /// (The pre-corrective order packed the pre-tick pool: the
    /// dying drop rendered for one more frame beside its newborn
    /// ring.)
    #[test]
    fn falling_to_landed_transition_in_one_frame() {
        let mut layer = RainLayer::new(FIRST_EYE);
        let rings_before = layer.ripple_spawned;
        let mut ring_tick = None;
        for tick in 1..=600u32 {
            let report = layer.update_and_build(FIRST_EYE, 0.0, 0.0, demo_clock_dt().as_secs_f32());
            if layer.ripple_spawned > rings_before && ring_tick.is_none() {
                ring_tick = Some(tick);
                assert_eq!(
                    layer.instances.len(),
                    report.active as usize,
                    "the ring-birth tick's pack is the completed tick: the landed drop \
                     is gone the same frame its ring appears"
                );
                assert!(
                    layer.ripple_field().active_count() > 0,
                    "the newborn ring is live in the same frame"
                );
            }
        }
        assert!(
            ring_tick.is_some(),
            "the 600-tick campaign births at least one ring (the decimation gate's \
             1.5/s)"
        );
    }

    /// The billboard turn known answers (the TRUE derivation,
    /// hand-computed — the finding-13 corrective: t = +yaw·65536/τ,
    /// wrapped forward; the pre-corrective packer used −yaw, which
    /// is edge-on at the diagonal headings):
    /// - yaw 0 (the camera looks −Z): the +Z-authored normal already
    ///   faces it → turn 0.
    /// - yaw π/2 (looks −X): turn 16384 (the normal +X).
    /// - yaw π (looks +Z): turn 32768 (the normal −Z).
    /// - yaw −π/2 (looks +X): turn −16384 ≡ 49152 (the normal −X).
    #[test]
    fn billboard_turn_known_answers() {
        assert_eq!(yaw_to_turn(0.0), 0, "yaw 0 -> turn 0 (normal +Z)");
        assert_eq!(
            yaw_to_turn(std::f32::consts::FRAC_PI_2),
            16384,
            "yaw π/2 -> turn 16384 (normal +X, facing the −X-looking camera)"
        );
        assert_eq!(
            yaw_to_turn(std::f32::consts::PI),
            32768,
            "yaw π -> turn 32768 (normal −Z, facing the +Z-looking camera)"
        );
        assert_eq!(
            yaw_to_turn(-std::f32::consts::FRAC_PI_2),
            49152,
            "yaw −π/2 -> turn 49152 (normal −X, facing the +X-looking camera)"
        );
    }

    /// THE BILLBOARD FACES THE CAMERA AT DIAGONAL HEADINGS (finding
    /// 13's deciding test, the authored-matrix form): the dot of
    /// the streak mesh's +Z normal — transformed by
    /// `instance_columns` at the packed turn — with the camera's
    /// horizontal BACKWARD vector must be ~1 at ±45°, the
    /// cardinals, and an off-cardinal heading. The pre-corrective
    /// sign (t = −yaw) gives cos 2θ — ZERO at 45°, the edge-on
    /// collapse — asserted as the documented failure witness.
    #[test]
    fn billboard_faces_the_camera_at_diagonal_headings() {
        let facing_dot = |yaw: f32| -> f32 {
            let turn = yaw_to_turn(yaw);
            let columns = instance_columns(&InstanceTransform {
                pos: [0.0; 3],
                turn,
                scale: 1.0,
            });
            // The mesh-authored +Z normal, transformed by the packed
            // instance matrix (column 2).
            let normal = [columns[2][0], columns[2][1], columns[2][2]];
            let forward = crate::math::camera_forward(yaw, 0.0, 0.0);
            let backward = [-forward[0], 0.0, -forward[2]];
            normal[0] * backward[0] + normal[1] * backward[1] + normal[2] * backward[2]
        };
        // The corrected turn: broadside at every heading (the
        // quantizer's 1/65536 turn and sin_cos_turn's <= 2e-6 error
        // keep the dot within 1e-5 of 1).
        for yaw in [
            std::f32::consts::FRAC_PI_4,
            -std::f32::consts::FRAC_PI_4,
            std::f32::consts::FRAC_PI_2,
            std::f32::consts::PI,
            0.83, // off-cardinal, ~47.6 degrees
            -2.31,
        ] {
            let dot = facing_dot(yaw);
            assert!(
                dot > 0.999,
                "yaw {yaw:.3}: the authored transformed normal faces the camera \
                 (dot {dot:.5})"
            );
        }
        // THE PRE-CORRECTIVE WITNESS (documented, not live): the
        // negated turn's dot with the intended normal is cos 2θ —
        // at 45° it is zero (edge-on, the streaks vanish).
        let edge_on = std::f32::consts::FRAC_PI_4;
        let wrong_turn = yaw_to_turn(-edge_on);
        let columns = instance_columns(&InstanceTransform {
            pos: [0.0; 3],
            turn: wrong_turn,
            scale: 1.0,
        });
        let normal = [columns[2][0], columns[2][1], columns[2][2]];
        let forward = crate::math::camera_forward(edge_on, 0.0, 0.0);
        let backward = [-forward[0], 0.0, -forward[2]];
        let dot = normal[0] * backward[0] + normal[1] * backward[1] + normal[2] * backward[2];
        assert!(
            dot.abs() < 0.001,
            "the negated turn at 45° is edge-on (dot {dot:.5} = cos 2θ ~ 0) — the \
             collapse the corrective fixes"
        );
    }

    /// The tint composition: the day/night endpoints (t=0/t=1) match
    /// atmosphere_at's ambient exactly.
    #[test]
    fn tint_composition() {
        let day = atmosphere_ambient(0.0);
        let night = atmosphere_ambient(1.0);
        let day_atmo = engine_renderer::sky::atmosphere_at(0.0);
        let night_atmo = engine_renderer::sky::atmosphere_at(1.0);
        assert_eq!(
            day, day_atmo.light.ambient,
            "day tint matches atmosphere_at(0.0).light.ambient"
        );
        assert_eq!(
            night, night_atmo.light.ambient,
            "night tint matches atmosphere_at(1.0).light.ambient"
        );
    }
}
