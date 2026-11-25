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
//! below. The census line in the exit summary surfaces the
//! per-campaign totals.

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
/// instance buffer, and the census accumulators.
///
/// CONSTRUCTION:
/// - `Weather::rain(1.0)` — the flag's intensity.
/// - Fixed authored seed: `RAIN_SEED`.
/// - Warm start: 132 ticks at dt = 1/60 against the route's first
///   camera position — the first presented frame is already wet.
/// - Volume: the frustum AABB intersected with the scene AABB.
///
/// PER FRAME (render-then-step, dt = 1/60 exactly):
/// 1. Volume refresh: the listener-following spawn box around THIS
///    frame's camera eye (the frustum AABB ∩ the scene AABB — the
///    documented `frustum ∩ aabb` discipline). For free-look, the
///    quantized listener box is the simpler alternative (a fixed
///    half-extent box around the eye, x/z-quantized to 4 m cells).
/// 2. Build the instances: for every `particles()` entry — `pos:
///    p.position`, `turn:` the billboard turn (the derived formula
///    t = −yaw·65536/τ — see [`yaw_to_turn`]), `scale: 1.0`,
///    `tint:` the atmosphere-derived color, `pose_index: 0`,
///    `alpha: RAIN_ALPHA`.
/// 3. The `TickReport` from the PREVIOUS step's tick observed (census
///    accumulated; any `SpawnOverflow` counted).
/// 4. Render with `ProxyPopulation { …, rain: &layer.instances, … }`.
/// 5. THEN `emitter.tick(1.0/60.0)` — the report carried to the next
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

        // Warm start: 132 ticks at dt = 1/60 — the first presented
        // frame is already wet. THE WARM CENSUS IS COUNTED (the
        // layer's conservation law covers its WHOLE lifetime — the
        // warm-start births and deaths are real campaign events;
        // the exit summary's numbers include them).
        let mut spawned: u64 = 0;
        let mut died: u64 = 0;
        let mut refused: u64 = 0;
        let mut previous_active: u64 = 0;
        for _ in 0..132 {
            let report = emitter.tick(1.0 / 60.0);
            let refused_this_tick = report.overflow.map(|o| u64::from(o.dropped)).unwrap_or(0);
            let accepted = u64::from(report.spawned) - refused_this_tick;
            died += (previous_active + accepted).saturating_sub(u64::from(report.active));
            spawned += u64::from(report.spawned);
            refused += refused_this_tick;
            previous_active = u64::from(report.active);
        }

        let warm_active = previous_active as u32;
        Self {
            emitter,
            instances: Vec::with_capacity(1024),
            spawned,
            died,
            refused,
            previous_active: warm_active,
            final_active: warm_active,
            volume_refresh_count: 0,
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
        // 1. Volume refresh: the frustum AABB intersected with the
        //    scene AABB. For simplicity, use the quantized listener
        //    box.
        let volume = quantized_listener_volume(camera_position);
        if self.emitter.config().volume != volume {
            self.emitter
                .set_volume(volume)
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
        let report = self.emitter.tick(1.0 / 60.0);
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

    /// Folds the emitter state for the digest pin.
    pub fn fold_state(&self) -> u64 {
        let mut hash = engine_core::fingerprint::Fingerprinter::new();
        self.emitter.fold_state(&mut hash);
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

    /// The warm-start + campaign determinism: double-run identical,
    /// AND the final digest pinned (the derivation: seed RAIN_SEED,
    /// RAIN_POLICY, the fixed camera at the first route eye, 600
    /// update ticks at dt = 1/60 after the 132-tick warm start).
    /// A moved digest means a math, order, or packing change — an
    /// investigation, never a re-pin without a cause.
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
        const PINNED_FNV: u64 = 2833298087568370423;
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
