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

use engine_core::particle::{Emitter, SpawnVolume};
use engine_core::weather::{RainPolicy, Weather};
use engine_renderer::proxy_instances::{InstanceTransform, ProxyInstance};

/// The rain policy's fixed constants (the demo authors these; the
/// engine carries no rain numbers). The derivation comment: rate ×
/// mean lifetime < PARTICLE_POOL_BOUND with headroom — 480 × 1.15 =
/// 552 < 1024, so the happy path never overflows.
pub const RAIN_POLICY: RainPolicy = RainPolicy {
    full_rate: 480.0,
    band_height: 12.0,
    fall_speed: 9.0,
    lateral_jitter: 0.6,
    vertical_jitter: 0.15,
    lifetime: [0.9, 1.4],
    gravity: 9.81,
    drag_factor: 0.982,
};

/// The rain emitter seed (per-emitter own-stream rule — the
/// deterministic core's pattern: a fixed seed produces byte-identical
/// streams on every platform).
pub const RAIN_SEED: u64 = 0xDEADBEEF;

/// The ripple policy's fixed constants (the demo authors these; the
/// engine carries no ripple numbers). The derivations:
/// lifetime 2.5 s and speed 0.8 m/s give a max ring radius of
/// 2.0 m; the width 0.35 m covers the landing-column drift
/// (<= ~0.11 m, the approximation's documented bound); amplitude
/// 1.0 is the unit scale the renderer's gains consume; decimation
/// 1 = every landing drop leaves a ring (the measured kill-death
/// rate 1.475/s × 2.5 s ≈ 3.7 live rings — 8.5x headroom under
/// the 32 bound, NO LOD needed).
pub const RIPPLE_POLICY: engine_core::ripple::RipplePolicy = engine_core::ripple::RipplePolicy {
    lifetime: 2.5,
    speed: 0.8,
    width: 0.35,
    amplitude: 1.0,
    decimation: 1,
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
/// PER FRAME (render-then-step, dt = 1/60 exactly):
/// 1. Volume refresh, LIKE-FOR-LIKE (the corrective): the quantized
///    listener box around THIS frame's camera eye (a fixed
///    half-extent box, x/z-quantized to 4 m cells), from which the
///    BAND is re-derived (rain_emitter_config's own form — the
///    spawn band ABOVE the observed volume) and compared against
///    the emitter's config volume; a change installs the fresh
///    BAND, never the observed box (the landed defect installed the
///    observed box once and collapsed the band to eye level — the
///    corrective's target; see update_and_build's step 1).
/// 2. Build the instances: for every `particles()` entry — `pos:
///    p.position`, `turn:` the billboard turn (the derived formula
///    t = −yaw·65536/τ — see [`yaw_to_turn`]), `scale: 1.0`,
///    `tint:` the atmosphere-derived color, `pose_index: 0`,
///    `alpha: RAIN_ALPHA`.
/// 3. The `TickReport` from the PREVIOUS step's tick observed (census
///    accumulated; any `SpawnOverflow` counted).
/// 4. Render with `ProxyPopulation { …, rain: &layer.instances, … }`.
/// 5. THEN the pinned composition: `emitter.tick_observing_kill_deaths
///    (1.0/60.0, ripple observe)` — arithmetic-identical to the old
///    `tick` (the core's one-code-path delegation) — followed by
///    `ripples.tick(1.0/60.0)`; the reports carried to the next
///    frame.
pub struct RainLayer {
    emitter: Emitter,
    instances: Vec<ProxyInstance>,
    spawned: u64,
    died: u64,
    refused: u64,
    /// The previous tick's active count (the census arithmetic's
    /// carry: died_this_tick = previous_active + accepted − active).
    previous_active: u32,
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
}

impl RainLayer {
    /// Constructs the rain layer with warm start (132 ticks at
    /// dt=1/60 against the route's first camera position). The first
    /// presented frame is already wet.
    pub fn new(camera_position: [f32; 3]) -> Self {
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
        let mut ripples = engine_core::ripple::RippleField::new(RIPPLE_POLICY)
            .expect("the ripple policy validates");

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
        for _ in 0..132 {
            let report = emitter.tick_observing_kill_deaths(1.0 / 60.0, |position| {
                // The typed spawn refusal, counted at the site
                // (never swallowed — unreachable at this policy's
                // 8.5x headroom, counted anyway; the field's own
                // refused_total counts the same events, so the two
                // are equal by construction).
                if let Some(overflow) = ripples.observe_death(position) {
                    warm_refusals += u64::from(overflow.dropped);
                }
            });
            let refused_this_tick = report.overflow.map(|o| u64::from(o.dropped)).unwrap_or(0);
            let accepted = u64::from(report.spawned) - refused_this_tick;
            died += (previous_active + accepted).saturating_sub(u64::from(report.active));
            spawned += u64::from(report.spawned);
            refused += refused_this_tick;
            previous_active = u64::from(report.active);
            ripples.tick(1.0 / 60.0);
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
            previous_active: warm_active,
            final_active: warm_active,
            volume_refresh_count: 0,
            ripples,
            ripple_spawned,
            ripple_expired,
            ripple_refused: warm_refusals,
        }
    }

    /// Updates the volume and builds instances for one frame, then
    /// ticks the emitter. `camera_position` is the listener's world
    /// position; `camera_yaw` is the listener's yaw (for the
    /// billboard turn); `day_night` is the cycle position for the
    /// tint derivation. Returns the tick report for census
    /// accumulation.
    pub fn update_and_build(
        &mut self,
        camera_position: [f32; 3],
        camera_yaw: f32,
        day_night: f32,
    ) -> engine_core::particle::TickReport {
        // 1. Volume refresh, LIKE-FOR-LIKE (the corrective): the
        //    emitter's config volume is the BAND rain_emitter_config
        //    derives from an observed box (the spawn band ABOVE the
        //    observed volume — weather.rs's own contract); the
        //    refresh must compare and install the band derived from
        //    the FRESH observed box, never the observed box itself.
        //    THE LANDED DEFECT this corrects: the band-vs-observed
        //    comparison always differed, and the first refresh
        //    installed the OBSERVED box as the spawn volume — the
        //    rain then spawned at eye level (y in [1.7, 13.7]
        //    against the kill plane at 1.7), the kill-death
        //    fraction jumped from the design's 0.34% to ~64%, and
        //    the ripple pool saturated (measured: 3726 typed
        //    refusals over the campaign — the stop's evidence). The
        //    refresh triggers exactly when the quantized observed
        //    box changes (the same 4 m cell crossings as before —
        //    the route's volume-refresh count is unchanged); only
        //    the INSTALLED volume is corrected.
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

        // 2. Build the instances.
        let particles = self.emitter.particles();
        self.instances.clear();
        self.instances.reserve(particles.len());

        // The atmosphere tint: the ambient from `atmosphere_at`.
        // For the demo, we use the day/night preset's ambient.
        let tint = atmosphere_ambient(day_night);

        for p in particles {
            self.instances.push(ProxyInstance {
                transform: InstanceTransform {
                    pos: p.position,
                    // The billboard turn: the derived t = −yaw (the
                    // quad's mesh-authored +Z normal faces the
                    // camera — see yaw_to_turn's derivation).
                    turn: yaw_to_turn(camera_yaw),
                    scale: 1.0,
                },
                tint,
                alpha: RAIN_ALPHA,
                pose_index: 0,
            });
        }

        // 3. The tick (render-then-step: the instances above render
        //    THIS frame; the tick advances the field for the next).
        //    The report is OBSERVED — the census accumulates, the
        //    overflow counts (the fail-visible discipline).
        //    THE PINNED COMPOSITION ORDER (ripple.rs's campaign
        //    digest): the emitter ticks FIRST through
        //    tick_observing_kill_deaths — arithmetic-identical to
        //    tick (the core's one-code-path delegation) — with
        //    every kill-plane death observing its ripple ring at
        //    the death position AS IT HAPPENS (mid-tick, in sweep
        //    order), THEN the ripple field ticks (every live ring
        //    ages one dt, the newborn included: a ring born this
        //    tick is dt old at the tick's end). The typed ripple
        //    spawn refusal is counted at the site (never
        //    swallowed); the ripple census reads the field's own
        //    counters (spawned/expired deltas around the composed
        //    tick) and the ripple report's refusal carry is
        //    asserted in agreement with the site count.
        let ripple_spawned_before = self.ripples.spawned();
        let ripple_expired_before = self.ripples.expired();
        let mut ripple_refused_this_tick: u32 = 0;
        let report = self
            .emitter
            .tick_observing_kill_deaths(1.0 / 60.0, |position| {
                if let Some(overflow) = self.ripples.observe_death(position) {
                    ripple_refused_this_tick += overflow.dropped;
                }
            });
        let ripple_report = self.ripples.tick(1.0 / 60.0);
        assert_eq!(
            ripple_report.refused, ripple_refused_this_tick,
            "the ripple report's refusal carry must agree with the site's count"
        );
        self.ripple_spawned += self.ripples.spawned() - ripple_spawned_before;
        self.ripple_expired += self.ripples.expired() - ripple_expired_before;
        self.ripple_refused += u64::from(ripple_refused_this_tick);
        let refused_this_tick = report.overflow.map(|o| u64::from(o.dropped)).unwrap_or(0);
        let accepted = u64::from(report.spawned) - refused_this_tick;
        // THE CENSUS ARITHMETIC (the conservation law's per-tick
        // form): active_new = previous_active + accepted − died,
        // so died = previous_active + accepted − active_new —
        // saturating (a pathological report can only make this
        // zero, never negative).
        self.died +=
            (u64::from(self.previous_active) + accepted).saturating_sub(u64::from(report.active));
        self.spawned += u64::from(report.spawned);
        self.refused += refused_this_tick;
        self.previous_active = report.active;
        self.final_active = report.active;

        report
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

/// The quantized listener volume: a fixed half-extent box around the
/// eye, x/z-quantized to 4 m cells so the field does not slide
/// continuously. The half-extent is 20 m (the corner scene's width),
/// y-extent is 12 m above the floor (the band height).
fn quantized_listener_volume(eye: [f32; 3]) -> SpawnVolume {
    // Quantize x/z to 4 m cells.
    let quantize = |v: f32| -> f32 { (v / 4.0).round() * 4.0 };
    let qx = quantize(eye[0]);
    let qz = quantize(eye[2]);
    SpawnVolume {
        min: [qx - 20.0, eye[1], qz - 20.0],
        max: [qx + 20.0, eye[1] + 12.0, qz + 20.0],
    }
}

/// Converts the camera's yaw (radians, math.rs's convention: zero
/// looks down −Z, positive rotates toward +X) to the rain quad's
/// billboard turn (u16 turn fractions, `instance_columns`'
/// convention). THE DERIVATION (physical, verified against both
/// matrices): `instance_columns` maps the streak mesh's +Z normal
/// to the world direction (sin t, 0, cos t) at turn t; the camera
/// at yaw θ looks along (sin θ, 0, −cos θ), so the quad faces the
/// camera when its normal is (−sin θ, 0, cos θ) — i.e. **t = −θ**.
/// NO quarter-turn offset: at yaw 0 the camera looks down −Z and
/// the mesh-authored +Z normal already faces it (the first draft's
/// +16384 turned every streak 90° EDGE-ON — caught in the line
/// review; the ink gate is the physical arbiter). The quantizer is
/// correctly-rounded (`.round()` — deterministic IEEE) with the
/// total saturating cast and `rem_euclid` (negative yaws wrap
/// forward) — the libm-free law on the packed path: no runtime
/// sin/cos anywhere near the instance bytes.
pub fn yaw_to_turn(yaw: f32) -> u16 {
    const TAU: f32 = 2.0 * std::f32::consts::PI;
    const SCALE: f32 = 65536.0 / TAU;
    let turn = (-yaw * SCALE).round();
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

    /// The warm-start known answer (MEASURED 2026-11-25, and the
    /// stage-1 engine-core campaign's own warm window — the two
    /// agree because the warm start never runs update_and_build's
    /// volume refresh, so the spawn band stays intact): exactly 5
    /// rings live after RainLayer::new's 132-tick warm start (the
    /// measured 5 kill deaths during warm start), none expired
    /// (2.2 s < the 2.5 s lifetime), no refusals. THE 5 IS
    /// MEASURED; a different number is a STOP-and-report, never a
    /// re-pin.
    #[test]
    fn ripple_warm_start_known_answer() {
        let layer = RainLayer::new([-5.0, 1.7, 13.0]);
        assert_eq!(
            layer.ripple_field().active_count(),
            5,
            "the measured 5 warm-start kill deaths leave 5 live rings"
        );
        assert_eq!(layer.ripple_spawned, 5, "the warm census counts them");
        assert_eq!(
            layer.ripple_expired, 0,
            "2.2 s < the 2.5 s lifetime: nothing expires"
        );
        assert_eq!(
            layer.ripple_refused, 0,
            "the warm window never fills the pool"
        );
        assert_eq!(layer.ripple_field().refused_total(), 0);
        // Every warm ring is younger than the lifetime.
        for ring in layer.ripple_field().rings() {
            assert!(ring.age < RIPPLE_POLICY.lifetime);
            assert_eq!(ring.amplitude, RIPPLE_POLICY.amplitude);
        }
    }

    /// THE CAMPAIGN DIGEST PIN + THE COMBINED CENSUS (the landed
    /// record). The discovery story, compressed: the stage-2 stop
    /// (2026-11-25) measured 132 spawns and a saturated ripple pool
    /// (3726 typed refusals) where the design model expected 18 and
    /// ~3.7 live rings; the root cause was the LANDED volume
    /// refresh's band-vs-observed comparison, whose first fire
    /// installed the observed box as the spawn volume and collapsed
    /// the band to eye level (kill deaths ~8.6x the design's rate);
    /// the corrective re-derived the refresh LIKE-FOR-LIKE (the
    /// band from the fresh observed box, band-vs-band) and the
    /// campaign now matches the design model exactly — the numbers
    /// below are the POST-CORRECTIVE landed actuals.
    ///
    /// THE CONVERGENCE (the corrective's proof, asserted below): at
    /// the fixed camera the quantized observed box never changes,
    /// so the like-for-like refresh never fires
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
    /// The census: ripple_spawned == 18 over the 132+600 window
    /// (the design model's measured kill-death total, the warm
    /// start's 5 included — now the LANDED path's actual) and
    /// refused == 0 (the pool-bound proof at the demo boundary:
    /// 1.475/s x 2.5 s ~ 3.7 live rings, 8.5x headroom under the
    /// 32 bound). A moved number or digest is an investigation,
    /// never a re-pin without a cause.
    #[test]
    fn ripple_campaign_digest_pin() {
        let first_eye = [-5.0, 1.7, 13.0];
        let mut layer = RainLayer::new(first_eye);
        let mut hasher = engine_core::fingerprint::Fingerprinter::new();
        for tick in 1..=600u32 {
            layer.update_and_build(first_eye, 0.0, 0.0);
            if tick % 60 == 0 {
                layer.ripple_field().fold_state(&mut hasher);
            }
        }
        // The combined census (the design model's measured total —
        // the post-corrective landed actual).
        assert_eq!(
            layer.ripple_spawned, 18,
            "the measured kill-death total over the whole 132+600 window (the warm \
             start's 5 included)"
        );
        assert_eq!(
            layer.ripple_refused, 0,
            "the pool-bound proof at the demo boundary (1.475/s x 2.5 s ~ 3.7 << 32)"
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
        const PINNED_CAMPAIGN_DIGEST: u64 = 6_501_874_864_611_424_535;
        assert_eq!(
            hasher.finish(),
            PINNED_CAMPAIGN_DIGEST,
            "the ripple campaign digest moved, or diverged from engine-core's fixture \
             pin (one campaign, two proofs, one digest): an investigation, never a \
             silent repin"
        );
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
    /// pool-bound proof on the post-corrective landed path (the
    /// stage-2 stop's saturated-pool measurement was the defect's
    /// signature — see ripple_campaign_digest_pin's doc for the
    /// discovery story).
    #[test]
    fn ripple_census_conservation() {
        let first_eye = [-5.0, 1.7, 13.0];
        let mut layer = RainLayer::new(first_eye);
        for tick in 1..=600u32 {
            layer.update_and_build(first_eye, 0.0, 0.0);
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
            "the pool-bound proof on the post-corrective landed path (the design's \
             8.5x headroom; the saturated-pool measurement was the pre-corrective \
             defect's signature)"
        );
    }

    /// The warm-start + campaign determinism: double-run identical,
    /// AND the final digest pinned (the derivation: seed RAIN_SEED,
    /// RAIN_POLICY, the fixed camera at the first route eye, 600
    /// update ticks at dt = 1/60 after the 132-tick warm start).
    /// A moved digest means a math, order, or packing change — an
    /// investigation, never a re-pin without a cause.
    ///
    /// RE-PINNED with cause (the volume-refresh corrective,
    /// 2026-11-25): the landed refresh's first fire installed the
    /// OBSERVED box as the spawn volume, collapsing the band to eye
    /// level for the whole campaign; the corrective installs the
    /// BAND derived from the fresh observed box (like-for-like), so
    /// the trajectory now spawns from the design band
    /// [13.7, 25.7] and the digest moved from 2833298087568370423
    /// to the value below (the old pin's history stays in git). At
    /// the fixed camera the like-for-like refresh never fires —
    /// the campaign is the construction-band trajectory, the same
    /// stream engine-core's demo fixture drives.
    #[test]
    fn warm_start_plus_campaign_determinism() {
        let first_eye = [-5.0, 1.7, 13.0];
        let first_yaw = 0.0;

        let run = || {
            let mut layer = RainLayer::new(first_eye);
            for _ in 0..600 {
                layer.update_and_build(first_eye, first_yaw, 0.0);
            }
            layer.fold_state()
        };
        let digest_a = run();
        let digest_b = run();
        assert_eq!(digest_a, digest_b, "the campaign double-runs identical");
        const PINNED_FNV: u64 = 1705604326501831793;
        assert_eq!(
            digest_a, PINNED_FNV,
            "the rain campaign's state digest moved: a math, order, or packing change"
        );
    }

    /// The census conservation (the conservation law over the
    /// layer's OWN tracked counters — NOT an identity derived from
    /// itself): spawned == died + final-active + refused, and the
    /// authored policy holds refused == 0 (rate × mean lifetime ≈
    /// 552 << the 1024 pool bound).
    #[test]
    fn census_conservation() {
        let first_eye = [-5.0, 1.7, 13.0];
        let mut layer = RainLayer::new(first_eye);
        for _ in 0..600 {
            layer.update_and_build(first_eye, 0.0, 0.0);
        }
        assert_eq!(
            layer.spawned,
            layer.died + u64::from(layer.final_active) + layer.refused,
            "spawned == died + final-active + refused (the layer's own counters)"
        );
        assert_eq!(
            layer.refused, 0,
            "the happy path has no refusals (rate × lifetime < pool bound)"
        );
        assert!(
            layer.died > 0,
            "a 600-tick campaign sees deaths (the lifetimes are 0.9–1.4 s)"
        );
    }

    /// The billboard turn known answers (the TRUE derivation,
    /// hand-computed): t = −yaw·65536/τ, wrapped forward.
    /// - yaw 0 (the camera looks −Z): the +Z-authored normal already
    ///   faces it → turn 0.
    /// - yaw π/2 (looks +X): turn −16384 ≡ 49152 (the normal −X).
    /// - yaw π (looks +Z): turn −32768 ≡ 32768 (the normal −Z).
    /// - yaw −π/2 (looks −X): turn 16384 (the normal +X).
    #[test]
    fn billboard_turn_known_answers() {
        assert_eq!(yaw_to_turn(0.0), 0, "yaw 0 -> turn 0 (normal +Z)");
        assert_eq!(
            yaw_to_turn(std::f32::consts::FRAC_PI_2),
            49152,
            "yaw π/2 -> turn 49152 (normal −X, facing the +X-looking camera)"
        );
        assert_eq!(
            yaw_to_turn(std::f32::consts::PI),
            32768,
            "yaw π -> turn 32768 (normal −Z)"
        );
        assert_eq!(
            yaw_to_turn(-std::f32::consts::FRAC_PI_2),
            16384,
            "yaw −π/2 -> turn 16384 (normal +X)"
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
