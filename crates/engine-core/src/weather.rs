//! The weather model (Era 2 package C — particles + weather): the
//! weather state as pure data, the rain-emitter derivation (the
//! listener-relative discipline — particles spawn in the OBSERVED
//! volume, supplied by the caller as an axis-aligned box), and the
//! composition with the day/night cycle (the frame the demo
//! renders from).
//!
//! # DETERMINISM CONTRACT
//! The weather state and its derivation are pure functions of their
//! inputs: the intensity clamps to [0, 1] with plain comparisons,
//! the rain derivation is fixed-order IEEE f32 (the band extension,
//! the rate scaling, the jitter vector — each a single add or
//! multiply), and no transcendental appears anywhere. Same inputs —
//! the same derived emitter configuration, byte-identical,
//! cross-platform.
//!
//! # THE BOUNDARY NOTE (why the composition lives here)
//! The renderer's `atmosphere_at` (engine-renderer's sky.rs) is the
//! sky model's own pure-f32 blend over a plain `t` in [0, 1]; its
//! engine-core dependency is diagnostics-scoped (timing samples),
//! so game weather state deliberately does not flow through
//! renderer types. The composition lives HERE as pure data, and the
//! DEMO frame boundary carries both halves — this
//! [`WeatherFrame`] and the renderer's own `AtmosphereFrame`. The
//! dependency-graph-driven boundary, not an omission.
//!
//! # THE POLICY BOUNDARY
//! Rain's numbers (rate, band height, fall speed, jitters,
//! lifetime, gravity, drag) are POLICY — authored in the demo as a
//! [`RainPolicy`] and passed as arguments. The engine carries no
//! rain constants; [`rain_emitter_config`] is pure arithmetic over
//! its inputs.

use crate::particle::{EmitterConfig, SpawnVolume};

/// The weather class: which precipitation regime is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherClass {
    /// No precipitation — the clear path draws nothing.
    Clear,
    /// Rain: the rain-emitter derivation applies.
    Rain,
}

impl WeatherClass {
    /// Whether the class is raining.
    pub fn is_raining(self) -> bool {
        matches!(self, WeatherClass::Rain)
    }
}

/// The weather state as pure data: the class plus the rain
/// intensity in [0, 1].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weather {
    /// The weather class (Clear or Rain).
    pub class: WeatherClass,
    /// Rain intensity in [0,1].
    pub intensity: f32,
}

impl Weather {
    /// The clear weather state (intensity 0).
    pub fn clear() -> Self {
        Self {
            class: WeatherClass::Clear,
            intensity: 0.0,
        }
    }

    /// The rain weather state, the intensity clamped to [0, 1].
    /// The RAW-state form: interpolation and blend code may feed it
    /// any finite value (the clamp keeps the state well-formed).
    /// POLICY data — author-supplied intensities — should use
    /// [`Weather::rain_validated`] instead (the fail-closed house
    /// form: a policy author's out-of-range value is a refusal, not
    /// a silent clamp; the verifier seat's first-review note, folded
    /// 2026-11-25).
    pub fn rain(intensity: f32) -> Self {
        Self {
            class: WeatherClass::Rain,
            intensity: intensity.clamp(0.0, 1.0),
        }
    }

    /// The rain weather state for POLICY data: the intensity must be
    /// finite and inside [0, 1] — a typed refusal otherwise (never a
    /// silent clamp hiding the caller's data error).
    pub fn rain_validated(intensity: f32) -> Result<Self, &'static str> {
        if !intensity.is_finite() {
            return Err("the rain intensity must be finite");
        }
        if !(0.0..=1.0).contains(&intensity) {
            return Err("the rain intensity must be inside [0, 1]");
        }
        Ok(Self {
            class: WeatherClass::Rain,
            intensity,
        })
    }

    /// Whether it is raining.
    pub fn is_raining(self) -> bool {
        self.class.is_raining()
    }

    /// The effective rain intensity: the clamped intensity when
    /// raining, 0 when clear.
    pub fn rain_intensity(self) -> f32 {
        if self.is_raining() {
            self.intensity
        } else {
            0.0
        }
    }
}

impl Default for Weather {
    fn default() -> Self {
        Self::clear()
    }
}

/// The composed atmosphere frame: the weather state together with
/// the day/night cycle position the renderer's `atmosphere_at`
/// consumes. One struct at the demo frame boundary (the composition
/// the mission names; see the module docs for why it lives here and
/// not on the renderer's AtmosphereFrame).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeatherFrame {
    /// The weather state.
    pub weather: Weather,
    /// The day/night cycle position in [0, 1] (t=0 day, t=1 night —
    /// `atmosphere_at`'s own parameter, clamped here).
    pub day_night: f32,
}

impl WeatherFrame {
    /// Composes the frame, clamping the cycle position to [0, 1]
    /// (the weather state clamps at its own constructors).
    pub fn new(weather: Weather, day_night: f32) -> Self {
        Self {
            weather,
            day_night: day_night.clamp(0.0, 1.0),
        }
    }
}

/// The rain policy's fixed constants (the demo authors these; the
/// engine carries no rain numbers).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RainPolicy {
    /// Particles per second at intensity 1.0.
    pub full_rate: f32,
    /// The spawn volume's height above the observed volume's floor.
    pub band_height: f32,
    /// The base fall speed (m/s, negative Y).
    pub fall_speed: f32,
    /// Horizontal jitter amplitude (m/s, both X and Z).
    pub lateral_jitter: f32,
    /// Vertical jitter amplitude (m/s).
    pub vertical_jitter: f32,
    /// Lifetime bounds (seconds).
    pub lifetime: [f32; 2],
    /// Gravity (m/s^2).
    pub gravity: f32,
    /// The per-step drag factor (see particle.rs's
    /// terminal-velocity derivation; paired with gravity so the
    /// fixed point is fall_speed).
    pub drag_factor: f32,
    /// The WORLD RECEIVER PLANE's height — street level, where rain
    /// dies and contacts are observed (the kill plane). A WORLD
    /// constant authored once with the scene, deliberately NOT
    /// derived from the observed volume: the observed box's floor
    /// rides the camera, so deriving the kill plane from it made
    /// rain "land" at the initial eye height while the drawn ground
    /// sits at the world's street level (the audit's finding 12 —
    /// a death spawned a ring on the drawn ground even though the
    /// drop crossed a plane 1.7 m above it). Climbing the camera
    /// never moves this plane; only the overhead spawn band follows
    /// the eye.
    pub receiver_y: f32,
}

/// Derive the rain emitter configuration for a weather state over
/// the OBSERVED volume (the camera frustum's world AABB,
/// caller-supplied — the listener-relative discipline: particles
/// spawn where they'll be seen). Returns None when it is not
/// raining (intensity <= 0 — no emitter, the clear path draws
/// nothing).
///
/// The derivation, exactly: the spawn volume is the observed volume
/// extended upward by `band_height` (rain spawns in the band ABOVE
/// the observed volume and falls through it); the rate scales the
/// policy's full rate by the rain intensity; the base velocity is
/// the fall speed on -Y with the policy's jitters; the lifetime,
/// gravity, and drag pass through; the kill plane is the policy's
/// WORLD RECEIVER PLANE (`receiver_y` — street level, a world
/// constant that never rides the observed volume's camera-following
/// floor; finding 12's three-volume separation: the OBSERVED box
/// and the overhead BAND follow the eye, the RECEIVER is the
/// world's); and the seed passes through unchanged (the caller's
/// per-emitter stream identity).
pub fn rain_emitter_config(
    weather: Weather,
    policy: &RainPolicy,
    observed: &SpawnVolume,
    seed: u64,
) -> Option<EmitterConfig> {
    if weather.rain_intensity() <= 0.0 {
        return None;
    }
    Some(EmitterConfig {
        volume: SpawnVolume {
            min: [observed.min[0], observed.max[1], observed.min[2]],
            max: [
                observed.max[0],
                observed.max[1] + policy.band_height,
                observed.max[2],
            ],
        },
        rate: policy.full_rate * weather.rain_intensity(),
        base_velocity: [0.0, -policy.fall_speed, 0.0],
        velocity_jitter: [
            policy.lateral_jitter,
            policy.vertical_jitter,
            policy.lateral_jitter,
        ],
        lifetime: policy.lifetime,
        gravity: policy.gravity,
        drag_factor: policy.drag_factor,
        kill_y: Some(policy.receiver_y),
        seed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::particle::{Emitter, PARTICLE_POOL_BOUND};

    /// The test observed volume: a 40 x 4 x 40 box with its floor
    /// at y = 0 (the corner scene's shape).
    fn observed_volume() -> SpawnVolume {
        SpawnVolume {
            min: [-20.0, 0.0, -20.0],
            max: [20.0, 4.0, 20.0],
        }
    }

    /// The test rain policy (the campaign's rain-like numbers). The
    /// receiver plane sits at street level (y = 0 — the corner
    /// scene's ground, the world constant the kill plane derives
    /// from).
    fn rain_policy() -> RainPolicy {
        RainPolicy {
            full_rate: 480.0,
            band_height: 12.0,
            fall_speed: 9.0,
            lateral_jitter: 0.6,
            vertical_jitter: 0.15,
            lifetime: [0.9, 1.4],
            gravity: 9.81,
            drag_factor: 0.982,
            receiver_y: 0.0,
        }
    }

    /// The state constructors: the intensity clamps to [0, 1], the
    /// default is clear, and the class semantics hold.
    #[test]
    fn weather_state_clamps_and_classifies() {
        assert_eq!(Weather::rain(-0.5).intensity, 0.0, "the floor clamp");
        assert_eq!(Weather::rain(1.5).intensity, 1.0, "the ceiling clamp");
        assert_eq!(Weather::rain(0.5).intensity, 0.5, "in-range passthrough");
        let default = Weather::default();
        assert_eq!(default.class, WeatherClass::Clear, "default = clear");
        assert!(!default.is_raining());
        assert_eq!(default.rain_intensity(), 0.0, "clear carries no intensity");
        assert!(!WeatherClass::Clear.is_raining());
        assert!(WeatherClass::Rain.is_raining());
        let rain = Weather::rain(0.75);
        assert!(rain.is_raining());
        assert_eq!(rain.rain_intensity(), 0.75);
    }

    /// The clear path: no emitter — the derivation returns None for
    /// clear weather AND for rain at intensity 0 (nothing draws).
    #[test]
    fn rain_derivation_returns_none_when_dry() {
        let policy = rain_policy();
        let observed = observed_volume();
        assert_eq!(
            rain_emitter_config(Weather::clear(), &policy, &observed, 1),
            None,
            "clear derives no emitter"
        );
        assert_eq!(
            rain_emitter_config(Weather::rain(0.0), &policy, &observed, 1),
            None,
            "intensity 0 derives no emitter"
        );
    }

    /// The derivation's fields, each exact: the band ABOVE the
    /// observed volume (floor riding the observed ceiling, ceiling
    /// extended by band_height), the intensity-scaled rate
    /// (480 x 0.5 = 240 — exact in f32), the policy's velocities
    /// and jitters, and the kill plane at the policy's WORLD
    /// RECEIVER (never the observed floor).
    #[test]
    fn rain_derivation_fields_are_exact() {
        let config = rain_emitter_config(
            Weather::rain(0.5),
            &rain_policy(),
            &observed_volume(),
            0xC0FFEE,
        )
        .expect("intensity 0.5 derives an emitter");
        assert_eq!(
            config.volume.min,
            [-20.0, 4.0, -20.0],
            "the band's floor rides the observed ceiling"
        );
        assert_eq!(
            config.volume.max,
            [20.0, 16.0, 20.0],
            "the band extends band_height above (4 + 12 = 16, exact)"
        );
        assert_eq!(config.rate, 240.0, "full_rate x intensity (480 x 0.5)");
        assert_eq!(config.base_velocity, [0.0, -9.0, 0.0]);
        assert_eq!(config.velocity_jitter, [0.6, 0.15, 0.6]);
        assert_eq!(config.lifetime, [0.9, 1.4]);
        assert_eq!(config.gravity, 9.81);
        assert_eq!(config.drag_factor, 0.982);
        assert_eq!(
            config.kill_y,
            Some(0.0),
            "rain dies at the policy's WORLD receiver plane (street level)"
        );
        assert_eq!(
            config.seed, 0xC0FFEE,
            "the caller's stream identity passes through"
        );
    }

    /// THE THREE-VOLUME SEPARATION (the audit's finding 12, the
    /// engine-core half): the kill plane is the policy's WORLD
    /// RECEIVER — a constant — never the observed volume's
    /// camera-following floor. An observed box whose floor rides an
    /// eye ABOVE the street (the demo starts rain at y = 1.7)
    /// still derives the street-level kill plane, and a HIGHER eye
    /// (the climbing camera) moves only the overhead band, never
    /// the receiver.
    #[test]
    fn kill_plane_is_the_world_receiver_never_the_observed_floor() {
        // The demo's starting eye: the observed floor at 1.7 — the
        // pre-corrective derivation would have "landed" rain here,
        // 1.7 m above the drawn ground.
        let eye_level = SpawnVolume {
            min: [-24.0, 1.7, -8.0],
            max: [16.0, 9.7, 32.0],
        };
        let config = rain_emitter_config(Weather::rain(1.0), &rain_policy(), &eye_level, 1)
            .expect("full rain derives");
        assert_eq!(
            config.kill_y,
            Some(0.0),
            "the receiver stays at street level (the observed floor 1.7 never lands rain)"
        );
        assert_eq!(
            config.volume.min[1], 9.7,
            "the band floor rides the observed ceiling (the band follows the eye)"
        );
        assert_eq!(config.volume.max[1], 21.7, "the band extends band_height");
        // The climbing camera: a HIGHER observed box moves the band
        // and leaves the receiver exactly where it was.
        let climbing = SpawnVolume {
            min: [-24.0, 30.0, -8.0],
            max: [16.0, 38.0, 32.0],
        };
        let climbed = rain_emitter_config(Weather::rain(1.0), &rain_policy(), &climbing, 1)
            .expect("full rain derives");
        assert_eq!(
            climbed.kill_y,
            Some(0.0),
            "climbing never moves the world's receiver plane"
        );
        assert_eq!(
            climbed.volume.min[1], 38.0,
            "the band floor rides the climbed observed ceiling"
        );
        // A different authored receiver passes through verbatim
        // (the plane is policy data, the scene's own constant).
        let mut rooftop = rain_policy();
        rooftop.receiver_y = 12.0;
        let config = rain_emitter_config(Weather::rain(1.0), &rooftop, &eye_level, 1)
            .expect("full rain derives");
        assert_eq!(
            config.kill_y,
            Some(12.0),
            "an authored receiver above the street passes through"
        );
    }

    /// The derived configuration passes the particle core's
    /// validation (the derivation never hands the machinery a
    /// refusal).
    #[test]
    fn derived_rain_config_passes_validation() {
        for intensity in [1.0, 0.5, 0.05] {
            let config = rain_emitter_config(
                Weather::rain(intensity),
                &rain_policy(),
                &observed_volume(),
                7,
            )
            .expect("rain derives");
            assert_eq!(config.validate(), Ok(()), "intensity {intensity}");
        }
    }

    /// The composed frame: the cycle position clamps to [0, 1] and
    /// the weather rides along untouched.
    #[test]
    fn weather_frame_clamps_the_cycle_position() {
        let rain = Weather::rain(0.6);
        assert_eq!(
            WeatherFrame::new(rain, -0.5).day_night,
            0.0,
            "the floor clamp"
        );
        assert_eq!(
            WeatherFrame::new(rain, 1.5).day_night,
            1.0,
            "the ceiling clamp"
        );
        let frame = WeatherFrame::new(rain, 0.25);
        assert_eq!(frame.day_night, 0.25, "in-range passthrough");
        assert_eq!(frame.weather, rain);
    }

    /// THE INTEGRATION FLAVOR: a derived rain emitter driven 240
    /// ticks at the demo's dt holds a steady state — nonempty and
    /// inside the pool bound, with the typed overflow silent-zero
    /// in the happy path (480 x 1.4 = 672 worst case < 1024).
    #[test]
    fn derived_rain_emitter_holds_a_steady_state_within_the_pool() {
        let dt = 1.0f32 / 60.0f32;
        let config = rain_emitter_config(
            Weather::rain(1.0),
            &rain_policy(),
            &observed_volume(),
            0xC0FFEE,
        )
        .expect("full rain derives");
        let mut emitter = Emitter::new(config).expect("the derived config validates");
        for tick in 1..=240u32 {
            let report = emitter.tick(dt);
            assert!(
                report.active > 0,
                "tick {tick}: the steady state is nonempty"
            );
            assert!(
                report.active <= PARTICLE_POOL_BOUND as u32,
                "tick {tick}: inside the pool bound"
            );
            assert_eq!(
                report.overflow, None,
                "tick {tick}: no drops in the steady state"
            );
        }
        assert_eq!(emitter.overflow_total(), 0, "the happy path drops nothing");
    }

    /// The validated rain constructor: the fail-closed POLICY form
    /// (the verifier seat's first-review note). In-range values
    /// construct; non-finite and out-of-range values refuse typed.
    #[test]
    fn validated_rain_refuses_out_of_range_policy_data() {
        assert!(Weather::rain_validated(0.0).is_ok());
        assert!(Weather::rain_validated(1.0).is_ok());
        assert!(Weather::rain_validated(0.5).is_ok());
        assert_eq!(
            Weather::rain_validated(f32::NAN).err(),
            Some("the rain intensity must be finite")
        );
        assert_eq!(
            Weather::rain_validated(f32::INFINITY).err(),
            Some("the rain intensity must be finite")
        );
        assert_eq!(
            Weather::rain_validated(-0.01).err(),
            Some("the rain intensity must be inside [0, 1]")
        );
        assert_eq!(
            Weather::rain_validated(1.01).err(),
            Some("the rain intensity must be inside [0, 1]")
        );
        // The raw form still clamps (the interpolation/blend path).
        assert_eq!(Weather::rain(1.5).intensity, 1.0);
        assert_eq!(Weather::rain(-0.5).intensity, 0.0);
    }
}
