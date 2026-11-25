//! The deterministic rain-ripple core (Era 2 package C — the
//! weather extension to the wet-ground reflection): the ripple
//! field's pure-data policy, the bounded fixed-array ring pool with
//! its typed spawn-overflow refusal, and the pinned fixed-order
//! ring aging — ALL pure, fully tested, no I/O, no wall clocks, no
//! Vulkan, no new dependencies. The particle core's discipline
//! (pure data, fixed-order arithmetic, FNV digest pins) applied to
//! the rings a raindrop's landing leaves on the ground.
//!
//! # DETERMINISM CONTRACT
//! The ripple field is a pure function of the OBSERVED death
//! sequence (the kill-plane death positions the particle core's
//! `Emitter::tick_observing_kill_deaths` hands over, in its sweep
//! order) plus the policy: same inputs → byte-identical state, on
//! every platform, forever. No transcendental appears anywhere on
//! a byte path — aging is one `+= dt`, the fade is applied at read
//! time as one multiply and one subtract (fixed order), the spawn
//! is a fixed-struct store. The pool is a FIXED-SIZE array with an
//! active prefix (zero allocation in every method); overflow above
//! the bound is a typed, counted, fail-visible refusal — never a
//! silent drop (the `audio_triggers.rs` overflow-counter
//! discipline).
//!
//! # THE PINNED ORDERS
//! * THE OBSERVE ORDER: deaths are observed in the emitter's
//!   integration sweep order (pool order — spawn order among
//!   survivors); the decimation counter advances once per observed
//!   death; a ring appends at the active prefix's end (spawn order
//!   among survivors, the particle pool's stable-compaction
//!   discipline).
//! * THE TICK ORDER: (1) age every active ring `+= dt` in pool
//!   order; (2) the expiry test — a ring dies iff
//!   `age >= lifetime`; (3) the stable compaction — survivors
//!   rewrite the prefix in order.
//!
//! # THE ENGINE/POLICY BOUNDARY
//! Every constant — the lifetime, the expansion speed, the band
//! width, the amplitude, the decimation — is policy data authored
//! above the engine and validated by [`RipplePolicy::validate`]
//! before use. No rain numbers live here; the demo authors the
//! policy and feeds the field the observed death sequence.
//!
//! # THE MEASURED CONTEXT
//! The pool bound is authored from the DEMO campaign's measured
//! kill-death rate: 1.475/s over 12.2 s — 18 kill deaths vs 5286
//! lifetime deaths, max 19 deaths in one tick (measured 2026-11-25
//! by replicating the RainLayer campaign, seed 0xDEADBEEF, 132
//! warm ticks + 600 campaign ticks at dt 1/60). At lifetime 2.5 s
//! the steady state is ~3.7 live rings, so the 32 bound carries
//! ~8.5x headroom.

use crate::fingerprint::Fingerprinter;
use std::cmp::Ordering;

/// The ripple pool's bound (fixed, like PARTICLE_POOL_BOUND): one
/// field holds at most this many live rings. 32 is authored from
/// the MEASURED kill-death rate (1.475/s x 2.5 s lifetime ≈ 3.7
/// live rings at steady state — 8.5x headroom; the study's
/// original 8/s rate assumption was wrong by 5x, measured
/// 2026-11-25).
pub const RIPPLE_POOL_BOUND: usize = 32;

/// The pool's fill value: a dead, at-rest ring. Never observed
/// outside the active prefix — the array exists only so every
/// method allocates nothing.
const RIPPLE_ZERO: RippleRing = RippleRing {
    center: [0.0; 2],
    age: 0.0,
    amplitude: 0.0,
};

/// The typed spawn overflow (fail-visible, the audio_triggers
/// overflow-counter discipline: the caller MUST observe it, never
/// swallow it). Counts ring spawn attempts dropped above
/// [`RIPPLE_POOL_BOUND`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RippleSpawnOverflow {
    /// Ring spawn attempts dropped above the bound.
    pub dropped: u32,
}

/// One ripple ring's state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RippleRing {
    /// The ring's center on the ground plane (the death's x/z —
    /// the landing-column approximation).
    pub center: [f32; 2],
    /// Age in seconds since the drop landed; dead at >= lifetime.
    pub age: f32,
    /// The ring's peak amplitude (policy.amplitude at spawn —
    /// per-ring so a future policy can vary it; the fade is applied
    /// at read time, never stored).
    pub amplitude: f32,
}

/// The ripple policy as pure data (authored by the demo layer).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RipplePolicy {
    /// Ring lifetime in seconds (the ring expires when age >= this).
    pub lifetime: f32,
    /// Ring expansion speed in m/s (radius = speed * age).
    pub speed: f32,
    /// The ring band's half-width in meters (the triangle bump's
    /// extent around the radius).
    pub width: f32,
    /// The peak perturbation amplitude (unitless scale).
    pub amplitude: f32,
    /// One ring per N observed kill deaths (rate scaling; 1 = every
    /// landing drop leaves a ring).
    pub decimation: u32,
}

/// The per-tick report (the caller MUST observe — the overflow half
/// especially).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RippleTickReport {
    /// Ring spawn attempts REFUSED this tick (observed deaths above
    /// the pool bound — counted, never silently dropped). NOTE:
    /// spawns happen at observe_death time (between ticks), so this
    /// field carries the refusal count since the last tick call.
    pub refused: u32,
    /// Active rings after the tick.
    pub active: u32,
}

/// One ripple field: its validated policy and bounded ring pool.
/// The pool is owned inline (4 f32 per ring at the current bound —
/// one fixed footprint for the field's lifetime, zero per tick).
pub struct RippleField {
    /// The validated policy (immutable for the field's lifetime —
    /// reset replays it).
    policy: RipplePolicy,
    /// The bounded pool; only the prefix `[..active]` is meaningful.
    rings: [RippleRing; RIPPLE_POOL_BOUND],
    /// The live-prefix length.
    active: usize,
    /// The decimation counter: kill deaths observed since the last
    /// ring spawn.
    deaths_since_ring: u32,
    /// Cumulative ring spawns.
    spawned: u64,
    /// Cumulative ring expiries.
    expired: u64,
    /// Cumulative refused spawn attempts.
    refused_total: u64,
    /// The per-tick report's carry: refusals accumulated since the
    /// last tick call (deaths are observed BETWEEN ticks, so the
    /// report must carry them across the boundary; cleared by
    /// tick).
    refused_since_tick: u32,
}

impl RipplePolicy {
    /// Validates the policy. Every refusal is a `&'static str`
    /// reason (the npc_memory validation-refusal style); NaN
    /// anywhere is a refusal — the ordering gates go through
    /// [`gt_total`], which decides over `partial_cmp` so a NaN
    /// side never passes (a NaN has no ordering), and the
    /// positivity gates check `is_finite` directly (an infinite
    /// speed or lifetime is as unusable as a NaN). Amplitude 0.0
    /// IS legal — a no-op field, the A/B gate's baseline.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.lifetime.is_finite() || !gt_total(self.lifetime, 0.0) {
            return Err("ripple/lifetime-not-positive");
        }
        if !self.speed.is_finite() || !gt_total(self.speed, 0.0) {
            return Err("ripple/speed-not-positive");
        }
        if !self.width.is_finite() || !gt_total(self.width, 0.0) {
            return Err("ripple/width-not-positive");
        }
        if !self.amplitude.is_finite() {
            return Err("ripple/amplitude-non-finite");
        }
        if self.decimation == 0 {
            return Err("ripple/decimation-zero");
        }
        Ok(())
    }
}

impl RippleField {
    /// Constructs the field: validates the policy first — a
    /// refusal never constructs (the validation-refusal
    /// discipline). The pool starts empty (the active prefix is
    /// zero length; the fill value is never observed).
    pub fn new(policy: RipplePolicy) -> Result<Self, &'static str> {
        policy.validate()?;
        Ok(Self {
            policy,
            rings: [RIPPLE_ZERO; RIPPLE_POOL_BOUND],
            active: 0,
            deaths_since_ring: 0,
            spawned: 0,
            expired: 0,
            refused_total: 0,
            refused_since_tick: 0,
        })
    }

    /// The field's policy.
    pub fn policy(&self) -> &RipplePolicy {
        &self.policy
    }

    /// The active prefix — the live rings in spawn order among
    /// survivors.
    pub fn rings(&self) -> &[RippleRing] {
        &self.rings[..self.active]
    }

    /// The live-ring count (the active prefix's length).
    pub fn active_count(&self) -> usize {
        self.active
    }

    /// Cumulative ring spawns (the census observer).
    pub fn spawned(&self) -> u64 {
        self.spawned
    }

    /// Cumulative ring expiries (the census observer).
    pub fn expired(&self) -> u64 {
        self.expired
    }

    /// Cumulative refused spawn attempts (the census observer).
    pub fn refused_total(&self) -> u64 {
        self.refused_total
    }

    /// Back to the constructed state: the pool empties, the census
    /// counters and the decimation counter clear — the determinism
    /// proof's tool (drive, reset, replay: byte-identical).
    pub fn reset(&mut self) {
        self.rings = [RIPPLE_ZERO; RIPPLE_POOL_BOUND];
        self.active = 0;
        self.deaths_since_ring = 0;
        self.spawned = 0;
        self.expired = 0;
        self.refused_total = 0;
        self.refused_since_tick = 0;
    }

    /// THE SPAWN PATH (the observe order: deaths arrive in the
    /// emitter's sweep order, one call per kill death). The
    /// decimation counter advances; every `decimation`-th death
    /// attempts a ring. An attempt appends at the active prefix's
    /// end — `center` is the death's x/z under the LANDING-COLUMN
    /// approximation: the kill plane sits at eye height, and the
    /// remaining fall's lateral drift (<= 0.6 m/s horizontal vs
    /// ~9 m/s terminal over <= 1.7 m) is <= ~0.11 m, inside the
    /// authored ring width 0.35 m — the column, not the point, is
    /// the policy's own resolution. A full pool REFUSES typed (the
    /// ring is NOT spawned, the state is otherwise untouched) —
    /// never a silent drop.
    pub fn observe_death(&mut self, position: [f32; 3]) -> Option<RippleSpawnOverflow> {
        self.deaths_since_ring += 1;
        if self.deaths_since_ring < self.policy.decimation {
            return None; // counted, not yet a ring
        }
        self.deaths_since_ring = 0;
        if self.active < RIPPLE_POOL_BOUND {
            self.rings[self.active] = RippleRing {
                center: [position[0], position[2]],
                age: 0.0,
                amplitude: self.policy.amplitude,
            };
            self.active += 1;
            self.spawned += 1;
            None
        } else {
            self.refused_total += 1;
            self.refused_since_tick += 1;
            Some(RippleSpawnOverflow { dropped: 1 })
        }
    }

    /// AUTHORS one ring directly (the presentation gates' forced-
    /// field path — the ripple reflection gate hand-places rings in
    /// the camera's ground view; the demo's own path stays
    /// [`Self::observe_death`] with its landing-column derivation).
    /// The ring is stored VERBATIM at the active prefix's end —
    /// center, age, and amplitude exactly as supplied, never
    /// re-derived — and counted in `spawned` (a census spawn: an
    /// authored ring is one of {live ring, expired ring, typed
    /// refusal}, the conservation law's own terms). The decimation
    /// counter is untouched (no death was observed). REFUSALS, both
    /// fail-visible, the state otherwise untouched: a non-finite
    /// lane (center, age, or amplitude) refuses with
    /// `ripple/authored-ring-non-finite` — the policy validate's own
    /// NaN discipline applied to the ring's data; a negative age
    /// refuses with `ripple/authored-ring-age-negative` (rings are
    /// born at age 0). A full pool refuses typed exactly like
    /// `observe_death` (the ring is NOT stored, `refused_total`
    /// counts it, the per-tick report carries it). An age at or past
    /// the lifetime is legal data — the next tick's expiry law
    /// compacts it (no special case) — so authoring callers keep
    /// `age < lifetime` while the ring must render.
    pub fn author_ring(
        &mut self,
        ring: RippleRing,
    ) -> Result<Option<RippleSpawnOverflow>, &'static str> {
        if !ring.center[0].is_finite()
            || !ring.center[1].is_finite()
            || !ring.age.is_finite()
            || !ring.amplitude.is_finite()
        {
            return Err("ripple/authored-ring-non-finite");
        }
        if ring.age < 0.0 {
            return Err("ripple/authored-ring-age-negative");
        }
        if self.active < RIPPLE_POOL_BOUND {
            self.rings[self.active] = ring;
            self.active += 1;
            self.spawned += 1;
            Ok(None)
        } else {
            self.refused_total += 1;
            self.refused_since_tick += 1;
            Ok(Some(RippleSpawnOverflow { dropped: 1 }))
        }
    }

    /// One fixed-order tick. THE PINNED ORDER: (1) the age sweep —
    /// every active ring gains `dt` in pool order; (2) the expiry
    /// test and stable compaction — a ring dies iff
    /// `age >= lifetime`, survivors rewrite the prefix in order
    /// (spawn order among survivors, the particle pool's
    /// discipline); (3) the report — `refused` carries the
    /// refusals accumulated since the last tick call (deaths are
    /// observed between ticks), `active` is the post-compaction
    /// count. Zero allocation; the fixed-size array is walked in
    /// place.
    pub fn tick(&mut self, dt: f32) -> RippleTickReport {
        // 1. Age sweep, pool order.
        for ring in &mut self.rings[..self.active] {
            ring.age += dt;
        }
        // 2. Expiry + stable compaction.
        let mut write = 0usize;
        let mut expired_this_tick: u64 = 0;
        for i in 0..self.active {
            let ring = self.rings[i];
            if ring.age >= self.policy.lifetime {
                expired_this_tick += 1;
            } else {
                self.rings[write] = ring;
                write += 1;
            }
        }
        self.active = write;
        self.expired += expired_this_tick;
        // 3. The report (the refusal carry crosses the observe/tick
        //    boundary, then clears).
        let refused = self.refused_since_tick;
        self.refused_since_tick = 0;
        RippleTickReport {
            refused,
            active: self.active as u32,
        }
    }

    /// Folds every ACTIVE ring's `center[0].to_bits()`,
    /// `center[1].to_bits()`, `age.to_bits()`, and
    /// `amplitude.to_bits()` into the FNV-1a builder in pool order
    /// — x, z, age, amplitude per ring, fixed (the particle
    /// fold_state pattern: the visible state is the pin).
    pub fn fold_state(&self, hash: &mut Fingerprinter) {
        for ring in &self.rings[..self.active] {
            hash.write_u32(ring.center[0].to_bits());
            hash.write_u32(ring.center[1].to_bits());
            hash.write_u32(ring.age.to_bits());
            hash.write_u32(ring.amplitude.to_bits());
        }
    }
}

/// Whether `a > b` holds as a total decision — `false` when either
/// side is NaN (a NaN has no ordering, so the validation's gates
/// refuse NaN lifetimes, speeds, and widths: "NaN anywhere is a
/// refusal"). Defined locally so this module owns its own gates,
/// mirroring how particle.rs defines its own.
fn gt_total(a: f32, b: f32) -> bool {
    matches!(a.partial_cmp(&b), Some(Ordering::Greater))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::particle::{Emitter, SpawnVolume, TickReport};
    use crate::weather::{RainPolicy, Weather, rain_emitter_config};

    /// The test-authored policy (mirrors the demo's future
    /// constants): lifetime 2.5 s, expansion 0.8 m/s, band
    /// half-width 0.35 m, peak amplitude 1.0, one ring per landing
    /// drop.
    fn ripple_policy() -> RipplePolicy {
        RipplePolicy {
            lifetime: 2.5,
            speed: 0.8,
            width: 0.35,
            amplitude: 1.0,
            decimation: 1,
        }
    }

    /// The DEMO's rain policy (corner_weather::RAIN_POLICY,
    /// verbatim): the campaign fixture's numbers.
    fn demo_rain_policy() -> RainPolicy {
        RainPolicy {
            full_rate: 480.0,
            band_height: 12.0,
            fall_speed: 9.0,
            lateral_jitter: 0.6,
            vertical_jitter: 0.15,
            lifetime: [0.9, 1.4],
            gravity: 9.81,
            drag_factor: 0.982,
        }
    }

    /// The DEMO's exact rain emitter (corner_weather::RainLayer at
    /// the route's first eye, verified against corner_weather.rs):
    /// seed 0xDEADBEEF, `Weather::rain(1.0)`, RAIN_POLICY's
    /// constants, and the quantized listener volume at eye
    /// [-5.0, 1.7, 13.0] — derived: qx = round(-5/4)*4 = -4,
    /// qz = round(13/4)*4 = 12, half-extent 20, the 12 m band above
    /// the eye — the box [-24.0, 1.7, -8.0] to [16.0, 13.7, 32.0]
    /// (12 + 20 = 32; the stage-1 fixture's 28.0 was an arithmetic
    /// slip in the derivation write-up — the corrective re-pins the
    /// digest below for the true box; the kill counts are
    /// UNAFFECTED, kills are y-dynamics only — the RainLayer's
    /// warm window measured the same 5 on both boxes). Through
    /// `rain_emitter_config` the band rides the observed
    /// ceiling (spawn y in [13.7, 25.7]) and the kill plane sits at
    /// the observed floor (y = 1.7).
    fn demo_rain_emitter() -> Emitter {
        let observed = SpawnVolume {
            min: [-24.0, 1.7, -8.0],
            max: [16.0, 13.7, 32.0],
        };
        let config = rain_emitter_config(
            Weather::rain(1.0),
            &demo_rain_policy(),
            &observed,
            0xDEADBEEF,
        )
        .expect("the demo's rain config validates");
        Emitter::new(config).expect("the demo's emitter constructs")
    }

    /// Every refusal branch of validate(), each returning its exact
    /// reason — and a refusal never constructs a field. Amplitude
    /// 0.0 PASSES (the no-op field is legal — the A/B gate's
    /// baseline).
    #[test]
    fn validation_refusals() {
        let base = || ripple_policy();
        assert_eq!(base().validate(), Ok(()), "the baseline passes");
        // Lifetime: zero, negative, NaN, and both infinities (an
        // infinite lifetime is as unusable as a NaN).
        for lifetime in [0.0, -2.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut bad = base();
            bad.lifetime = lifetime;
            assert_eq!(
                bad.validate(),
                Err("ripple/lifetime-not-positive"),
                "lifetime {lifetime}"
            );
        }
        // Speed: zero, negative, NaN, infinite.
        for speed in [0.0, -0.8, f32::NAN, f32::INFINITY] {
            let mut bad = base();
            bad.speed = speed;
            assert_eq!(
                bad.validate(),
                Err("ripple/speed-not-positive"),
                "speed {speed}"
            );
        }
        // Width: zero, negative, NaN, infinite.
        for width in [0.0, -0.35, f32::NAN, f32::INFINITY] {
            let mut bad = base();
            bad.width = width;
            assert_eq!(
                bad.validate(),
                Err("ripple/width-not-positive"),
                "width {width}"
            );
        }
        // Amplitude: non-finite refuses (NaN and both infinities).
        for amplitude in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut bad = base();
            bad.amplitude = amplitude;
            assert_eq!(
                bad.validate(),
                Err("ripple/amplitude-non-finite"),
                "amplitude {amplitude}"
            );
        }
        // Amplitude 0.0 IS legal: the no-op field (the A/B gate's
        // baseline) constructs.
        let mut no_op = base();
        no_op.amplitude = 0.0;
        assert_eq!(no_op.validate(), Ok(()), "amplitude 0.0 is legal");
        assert!(
            RippleField::new(no_op).is_ok(),
            "the no-op field constructs"
        );
        // Decimation zero.
        let mut bad = base();
        bad.decimation = 0;
        assert_eq!(bad.validate(), Err("ripple/decimation-zero"));
        // A refusal never constructs.
        let mut bad = base();
        bad.lifetime = 0.0;
        assert!(
            RippleField::new(bad).is_err(),
            "the refusal blocks construction"
        );
    }

    /// KNOWN ANSWER — aging and expiry: dt = 1/60, lifetime 0.1 s.
    /// The death tick is derived by REPLICATING the accumulation
    /// (the known_answer_kill_plane pattern: f32 age accumulation
    /// is NOT k*dt, so the test runs the same loop) — the ring dies
    /// on exactly that tick, at exactly the accumulated age, and
    /// the census counts one expiry.
    #[test]
    fn known_answer_aging_and_expiry() {
        let dt = 1.0f32 / 60.0f32;
        let mut policy = ripple_policy();
        policy.lifetime = 0.1;
        let mut field = RippleField::new(policy).expect("the policy validates");
        assert_eq!(
            field.observe_death([1.0, 0.0, 2.0]),
            None,
            "decimation 1: one ring"
        );
        assert_eq!(field.active_count(), 1);
        // The replica: the first tick count whose accumulated age
        // reaches the lifetime.
        let mut ticks_to_death = 0u32;
        let mut age = 0.0f32;
        while age < policy.lifetime {
            age += dt;
            ticks_to_death += 1;
        }
        assert!(ticks_to_death >= 1, "the ring expires inside the window");
        // Drive in lockstep with a second replica accumulator: the
        // ring's age after n ticks is the SAME f32 accumulation.
        let mut replica = 0.0f32;
        for t in 1..=ticks_to_death {
            let report = field.tick(dt);
            replica += dt;
            if t < ticks_to_death {
                assert_eq!(report.active, 1, "tick {t}: still alive");
                assert_eq!(
                    field.rings()[0].age,
                    replica,
                    "tick {t}: the accumulated age (the replica)"
                );
            } else {
                assert_eq!(report.active, 0, "dead EXACTLY when age >= lifetime");
                assert_eq!(report.refused, 0, "no refusals anywhere in this test");
            }
        }
        assert_eq!(field.expired(), 1, "the census counts the expiry");
        assert_eq!(field.spawned(), 1);
        assert_eq!(field.active_count(), 0);
        assert_eq!(
            field.rings().len(),
            0,
            "the prefix is empty after the expiry"
        );
    }

    /// KNOWN ANSWER — the spawn shape: one observed death at a
    /// known position stores the landing column (the death's x/z),
    /// age exactly 0.0, and the policy's amplitude; one tick ages
    /// the ring to exactly the f32 expression `0.0 + dt`.
    #[test]
    fn known_answer_spawn_shape() {
        let dt = 1.0f32 / 60.0f32;
        let policy = ripple_policy();
        let mut field = RippleField::new(policy).expect("the policy validates");
        let position = [-3.5, 1.7, 8.25];
        assert_eq!(
            field.observe_death(position),
            None,
            "decimation 1: every death spawns"
        );
        assert_eq!(field.active_count(), 1);
        assert_eq!(field.spawned(), 1);
        let ring = field.rings()[0];
        assert_eq!(
            ring.center,
            [position[0], position[2]],
            "the landing column"
        );
        assert_eq!(ring.age, 0.0, "born at zero");
        assert_eq!(
            ring.amplitude, policy.amplitude,
            "the policy's amplitude at spawn"
        );
        // One tick: the age is the exact f32 expression the sweep
        // performs.
        let report = field.tick(dt);
        assert_eq!(report.active, 1);
        assert_eq!(report.refused, 0);
        assert_eq!(field.rings()[0].age, 0.0f32 + dt, "one tick of aging");
        assert_eq!(field.expired(), 0, "lifetime 2.5 s: nothing expires");
        assert_eq!(
            field.rings()[0].center,
            [position[0], position[2]],
            "aging never moves the center"
        );
    }

    /// KNOWN ANSWER — the authoring path: `author_ring` stores the
    /// supplied ring VERBATIM (center, age, amplitude — the
    /// amplitude is NOT re-derived from the policy, the age is NOT
    /// reset to zero), appends at the active prefix's end in call
    /// order, and counts the spawn; the refusals (non-finite lane,
    /// negative age) leave the state untouched; an authored ring at
    /// the lifetime dies at the next tick's expiry law (the tick's
    /// own law, no special case); the census law
    /// `spawned == expired + active` holds over authored rings
    /// exactly as over observed ones; the typed capacity refusal is
    /// the observe_death shape (counted, carried by the report).
    #[test]
    fn author_ring_known_answers() {
        let dt = 1.0f32 / 60.0f32;
        let mut field = RippleField::new(ripple_policy()).expect("the policy validates");
        // Two hand-authored rings — amplitude 0.5 is NOT the
        // policy's 1.0 and age 0.25 is NOT zero: verbatim storage is
        // the known answer.
        let rings = [
            RippleRing {
                center: [-3.0, 8.0],
                age: 0.25,
                amplitude: 0.5,
            },
            RippleRing {
                center: [2.5, 4.5],
                age: 0.75,
                amplitude: 0.5,
            },
        ];
        for (i, ring) in rings.iter().copied().enumerate() {
            assert_eq!(
                field.author_ring(ring),
                Ok(None),
                "ring {i}: the pool has room"
            );
        }
        assert_eq!(field.rings(), &rings[..], "verbatim, in call order");
        assert_eq!(field.active_count(), 2);
        assert_eq!(field.spawned(), 2, "an authored ring is a census spawn");
        assert_eq!(
            field.deaths_since_ring, 0,
            "authoring observes no death (the private counter's white-box read)"
        );
        // One tick ages the authored rings like any other.
        let report = field.tick(dt);
        assert_eq!(report.active, 2);
        assert_eq!(report.refused, 0);
        assert_eq!(field.rings()[0].age, 0.25f32 + dt, "the aged authored ring");
        assert_eq!(field.rings()[1].age, 0.75f32 + dt);
        // The data refusals: non-finite lane, negative age — the
        // state untouched by both.
        let mut before = Fingerprinter::new();
        field.fold_state(&mut before);
        assert_eq!(
            field.author_ring(RippleRing {
                center: [f32::NAN, 0.0],
                age: 0.0,
                amplitude: 1.0,
            }),
            Err("ripple/authored-ring-non-finite")
        );
        assert_eq!(
            field.author_ring(RippleRing {
                center: [0.0, 0.0],
                age: -0.1,
                amplitude: 1.0,
            }),
            Err("ripple/authored-ring-age-negative")
        );
        let mut after = Fingerprinter::new();
        field.fold_state(&mut after);
        assert_eq!(
            before.finish(),
            after.finish(),
            "the refusals wrote nothing"
        );
        // An authored ring AT the lifetime dies at the next tick
        // (the expiry law, no special case).
        assert_eq!(
            field.author_ring(RippleRing {
                center: [0.0, 0.0],
                age: ripple_policy().lifetime,
                amplitude: 1.0,
            }),
            Ok(None)
        );
        let report = field.tick(dt);
        assert_eq!(report.active, 2, "the at-lifetime ring expired");
        assert_eq!(field.expired(), 1);
        assert_eq!(
            field.spawned(),
            field.expired() + field.active_count() as u64,
            "the census law holds over authored rings"
        );
        // The typed capacity refusal: fill to the bound, one more.
        while field.active_count() < RIPPLE_POOL_BOUND {
            assert_eq!(
                field.author_ring(RippleRing {
                    center: [1.0, 1.0],
                    age: 0.0,
                    amplitude: 1.0,
                }),
                Ok(None),
                "the fill authors while there is room"
            );
        }
        assert_eq!(
            field.author_ring(RippleRing {
                center: [9.0, 9.0],
                age: 0.0,
                amplitude: 1.0,
            }),
            Ok(Some(RippleSpawnOverflow { dropped: 1 })),
            "the refusal is typed"
        );
        assert_eq!(field.refused_total(), 1, "counted, never silently dropped");
        let report = field.tick(dt);
        assert_eq!(report.refused, 1, "the report carries the authored refusal");
        assert_eq!(report.active, RIPPLE_POOL_BOUND as u32);
    }

    /// KNOWN ANSWER — the decimation: decimation 3 over seven
    /// deaths spawns exactly 2 rings, at the 3rd and 6th observed
    /// positions in order; the counter ends at 1 (7 - 2*3: one
    /// death counted toward the next ring). The per-death ring
    /// count is floor(n/3) — the policy's own arithmetic, derived
    /// per death.
    #[test]
    fn decimation_known_answer() {
        let mut policy = ripple_policy();
        policy.decimation = 3;
        let mut field = RippleField::new(policy).expect("the policy validates");
        let deaths: [[f32; 3]; 7] = [
            [0.0, 1.7, 0.0],
            [1.0, 1.7, 1.0],
            [2.0, 1.7, 2.0],
            [3.0, 1.7, 3.0],
            [4.0, 1.7, 4.0],
            [5.0, 1.7, 5.0],
            [6.0, 1.7, 6.0],
        ];
        for (i, &position) in deaths.iter().enumerate() {
            assert_eq!(
                field.observe_death(position),
                None,
                "death {}: the pool never fills here",
                i + 1
            );
            assert_eq!(
                field.active_count(),
                (i + 1) / 3,
                "death {}: floor(n/3) rings after n = {} deaths",
                i + 1,
                i + 1
            );
        }
        assert_eq!(field.spawned(), 2, "rings at deaths 3 and 6");
        assert_eq!(field.expired(), 0, "no ticks: nothing expires");
        assert_eq!(
            field.deaths_since_ring, 1,
            "7 - 2*3: one death counted toward the next ring (the private \
             counter's white-box read — the child module's privilege)"
        );
        // The ring centers are the 3rd and 6th deaths' xz, in
        // observe order.
        assert_eq!(
            field.rings()[0].center,
            [deaths[2][0], deaths[2][2]],
            "the 3rd death's xz"
        );
        assert_eq!(
            field.rings()[1].center,
            [deaths[5][0], deaths[5][2]],
            "the 6th death's xz"
        );
        assert_eq!(field.rings()[0].age, 0.0);
        assert_eq!(field.rings()[1].age, 0.0);
    }

    /// THE TYPED OVERFLOW, never silent: fill the pool to the bound
    /// with distinct landing columns, then one more death — the
    /// return is the typed refusal, refused_total counts it, the
    /// pool stays at the bound, and the folded state is UNCHANGED by
    /// the refused attempt (the refused_refresh_preserves_state
    /// pattern). The per-tick report then carries the refusal
    /// across the observe/tick boundary and clears.
    #[test]
    fn overflow_typed_never_silent() {
        let mut field = RippleField::new(ripple_policy()).expect("the policy validates");
        for i in 0..RIPPLE_POOL_BOUND {
            let position = [i as f32, 0.0, -(i as f32)];
            assert_eq!(
                field.observe_death(position),
                None,
                "death {i}: the pool has room"
            );
        }
        assert_eq!(
            field.active_count(),
            RIPPLE_POOL_BOUND,
            "the pool saturates"
        );
        assert_eq!(field.spawned(), RIPPLE_POOL_BOUND as u64);
        // Fold the state before the refused attempt.
        let mut before = Fingerprinter::new();
        field.fold_state(&mut before);
        // One more: the typed refusal (the ring is NOT spawned, the
        // state is otherwise untouched).
        let position = [99.0, 0.0, -99.0];
        assert_eq!(
            field.observe_death(position),
            Some(RippleSpawnOverflow { dropped: 1 }),
            "the refusal is typed"
        );
        assert_eq!(field.refused_total(), 1, "counted, never silently dropped");
        assert_eq!(
            field.active_count(),
            RIPPLE_POOL_BOUND,
            "the pool stays at the bound"
        );
        assert_eq!(
            field.spawned(),
            RIPPLE_POOL_BOUND as u64,
            "the refusal spawned nothing"
        );
        // The refused attempt left the state untouched.
        let mut after = Fingerprinter::new();
        field.fold_state(&mut after);
        assert_eq!(
            before.finish(),
            after.finish(),
            "the refused attempt wrote nothing"
        );
        // The report carries the refusal across the observe/tick
        // boundary, then clears.
        let report = field.tick(1.0 / 60.0);
        assert_eq!(report.refused, 1, "the report carries the refusal");
        assert_eq!(report.active, RIPPLE_POOL_BOUND as u32);
        let report = field.tick(1.0 / 60.0);
        assert_eq!(report.refused, 0, "cleared by the previous tick");
        assert_eq!(report.active, RIPPLE_POOL_BOUND as u32);
        assert_eq!(
            field.expired(),
            0,
            "lifetime 2.5 s: nothing expires in two ticks"
        );
    }

    /// THE STABLE POOL: four rings spawned two ticks apart (staggered
    /// effective ages, so the expiries stagger) — the survivors must
    /// always be an IN-ORDER subsequence of the previous prefix (the
    /// particle stable_order_compaction pattern: the compaction
    /// never reorders; a ring is identified by its center, which
    /// never moves).
    #[test]
    fn stable_order_compaction() {
        let dt = 1.0f32 / 60.0f32;
        let mut policy = ripple_policy();
        policy.lifetime = 0.1; // short: the cohort expires inside the window
        let mut field = RippleField::new(policy).expect("the policy validates");
        let centers: [[f32; 2]; 4] = [[-1.0, 0.5], [0.0, 1.0], [1.0, 1.5], [2.0, 2.0]];
        let mut next = 0usize;
        for step in 0..40u32 {
            if step % 2 == 0 && next < centers.len() {
                let position = [centers[next][0], 0.0, centers[next][1]];
                assert_eq!(
                    field.observe_death(position),
                    None,
                    "step {step}: the pool has room"
                );
                next += 1;
            }
            // The pre-tick prefix (a spawn appends — trivially
            // order-preserving); the tick compacts.
            let previous: Vec<[f32; 2]> = field.rings().iter().map(|r| r.center).collect();
            let report = field.tick(dt);
            let current: Vec<[f32; 2]> = field.rings().iter().map(|r| r.center).collect();
            let mut scan = 0usize;
            for center in &current {
                while scan < previous.len() && previous[scan] != *center {
                    scan += 1;
                }
                assert!(
                    scan < previous.len(),
                    "step {step}: {center:?} broke the spawn order"
                );
                scan += 1;
            }
            assert_eq!(
                report.active,
                current.len() as u32,
                "step {step}: the report agrees"
            );
            assert_eq!(report.refused, 0, "step {step}: no refusals");
        }
        assert_eq!(next, centers.len(), "the whole cohort spawned");
        assert_eq!(
            field.active_count(),
            0,
            "the whole cohort expired (lifetime 0.1 << 40 ticks)"
        );
        assert_eq!(field.spawned(), 4);
        assert_eq!(field.expired(), 4, "every spawned ring expired");
    }

    /// THE CENSUS CONSERVATION (the corner_weather law's form, over
    /// the field's OWN counters): every decimation-gated spawn
    /// attempt is exactly one of {live ring, expired ring, typed
    /// refusal} — `spawned + refused_total == expired + active +
    /// refused_total` (equivalently `spawned == expired + active`:
    /// no ring vanishes and none duplicates; the brief's literal
    /// right-side form `spawned == expired + active + refused_total`
    /// is this same law with the attempt total on the left — it
    /// reduces to the happy-path form exactly when refused == 0,
    /// which the pre-refusal samples assert). Sampled at every tick
    /// of a mixed campaign: staggered spawns interleaved with
    /// expiries, then a forced refusal by filling the pool to the
    /// bound, then the full cohort's expiry.
    #[test]
    fn census_conservation() {
        let dt = 1.0f32 / 60.0f32;
        let mut policy = ripple_policy();
        policy.lifetime = 0.12; // short: expiries interleave with spawns
        let mut field = RippleField::new(policy).expect("the policy validates");
        // Phase 1 — spawns and expiries interleaved.
        for step in 0..24u32 {
            if step % 3 == 0 {
                let position = [step as f32 * 0.5, 0.0, -1.0 + step as f32 * 0.25];
                assert_eq!(
                    field.observe_death(position),
                    None,
                    "step {step}: the pool has room"
                );
            }
            let report = field.tick(dt);
            assert_eq!(
                report.refused, 0,
                "step {step}: the happy path refuses nothing"
            );
            assert_eq!(
                field.refused_total(),
                0,
                "step {step}: the happy-path form holds verbatim (refused == 0)"
            );
            let attempts = field.spawned() + field.refused_total();
            let outcomes = field.expired() + field.active_count() as u64 + field.refused_total();
            assert_eq!(
                attempts, outcomes,
                "step {step}: every gated attempt is a live ring, an expiry, or a typed refusal"
            );
        }
        // Phase 2 — the forced refusal: fill to the bound, then
        // three more attempts (each refused typed).
        while field.active_count() < RIPPLE_POOL_BOUND {
            assert_eq!(
                field.observe_death([9.0, 0.0, 9.0]),
                None,
                "the fill spawns while there is room"
            );
        }
        for refusal in 1..=3u32 {
            assert_eq!(
                field.observe_death([99.0, 0.0, -99.0]),
                Some(RippleSpawnOverflow { dropped: 1 }),
                "refusal {refusal}: typed, never silent"
            );
            let attempts = field.spawned() + field.refused_total();
            let outcomes = field.expired() + field.active_count() as u64 + field.refused_total();
            assert_eq!(
                attempts, outcomes,
                "refusal {refusal}: the law holds with the refusal on both sides"
            );
        }
        // The report carries all three refusals in one tick.
        let report = field.tick(dt);
        assert_eq!(
            report.refused, 3,
            "the three refusals cross the boundary together"
        );
        assert_eq!(report.active, RIPPLE_POOL_BOUND as u32);
        // Phase 3 — the full cohort expires; the law holds at every
        // tick to the empty end.
        for step in 0..24u32 {
            let report = field.tick(dt);
            let attempts = field.spawned() + field.refused_total();
            let outcomes = field.expired() + field.active_count() as u64 + field.refused_total();
            assert_eq!(attempts, outcomes, "expiry step {step}: the law holds");
            assert_eq!(report.refused, 0, "expiry step {step}: no new refusals");
        }
        assert_eq!(field.active_count(), 0, "the pool empties");
        assert_eq!(field.refused_total(), 3);
        assert_eq!(
            field.spawned(),
            field.expired(),
            "at the empty end: spawned == expired"
        );
    }

    /// One campaign run: the demo's rain emitter coupled with a
    /// ripple field. THE PINNED COMPOSITION ORDER (per tick — the
    /// digest depends on it): the emitter ticks FIRST, every kill
    /// death observed into the field as it happens (a ring born
    /// this tick enters at age 0.0 mid-tick), THEN the field ticks
    /// (every live ring ages one dt, the newborn included: a ring
    /// born this tick is dt old at the tick's end).
    struct CampaignRun {
        emitter: Emitter,
        field: RippleField,
        kill_deaths: u64,
    }

    impl CampaignRun {
        fn new() -> Self {
            Self {
                emitter: demo_rain_emitter(),
                field: RippleField::new(ripple_policy()).expect("the policy validates"),
                kill_deaths: 0,
            }
        }

        /// One tick of the pinned composition order. Returns both
        /// reports (the caller compares them across runs).
        fn step(&mut self, dt: f32) -> (TickReport, RippleTickReport) {
            let kill_deaths = &mut self.kill_deaths;
            let field = &mut self.field;
            let tick_report = self.emitter.tick_observing_kill_deaths(dt, |position| {
                *kill_deaths += 1;
                let overflow = field.observe_death(position);
                assert!(
                    overflow.is_none(),
                    "the demo campaign never fills the ripple pool (~3.7 live rings at \
                     steady state, 8.5x headroom)"
                );
            });
            let ripple_report = self.field.tick(dt);
            (tick_report, ripple_report)
        }

        /// Folds the field's state into the hasher (the sample
        /// point's digest contribution).
        fn fold(&self, hash: &mut Fingerprinter) {
            self.field.fold_state(hash);
        }

        /// Back to the constructed state — BOTH halves (the emitter
        /// restarts its stream, the field empties and clears its
        /// census).
        fn reset(&mut self) {
            self.emitter.reset();
            self.field.reset();
            self.kill_deaths = 0;
        }
    }

    /// THE INTEGRATION FLAVOR: the demo's exact rain campaign (seed
    /// 0xDEADBEEF, RAIN_POLICY, the quantized listener volume at the
    /// route's first eye — `demo_rain_emitter` above) driven 132
    /// warm ticks + 600 campaign ticks at dt 1/60, every kill death
    /// observed into the field — double-run from scratch: identical
    /// per-tick reports (emitter AND field) and identical field
    /// digests at every 60th campaign tick. The kill-death count
    /// matches the MEASURED actual: exactly 18 over the whole 732
    /// ticks (measured 2026-11-25 by replicating the RainLayer
    /// campaign — 18 kill deaths vs 5286 lifetime deaths; a
    /// different number is a STOP-and-investigate, never a re-pin).
    #[test]
    fn fixed_seed_campaign_deterministic() {
        let dt = 1.0f32 / 60.0f32;
        let mut run_a = CampaignRun::new();
        let mut run_b = CampaignRun::new();
        for t in 1..=(132u32 + 600) {
            assert_eq!(
                run_a.step(dt),
                run_b.step(dt),
                "tick {t}: identical reports (emitter and field)"
            );
            // Fold at every 60th tick of the 600-tick campaign (the
            // warm start's 132 ticks precede).
            if t > 132 && (t - 132) % 60 == 0 {
                let mut hash_a = Fingerprinter::new();
                run_a.fold(&mut hash_a);
                let mut hash_b = Fingerprinter::new();
                run_b.fold(&mut hash_b);
                assert_eq!(
                    hash_a.finish(),
                    hash_b.finish(),
                    "campaign tick {}: identical field digests",
                    t - 132
                );
            }
        }
        assert_eq!(
            run_a.kill_deaths, 18,
            "the MEASURED kill-death count over the whole 732 ticks"
        );
        assert_eq!(run_b.kill_deaths, 18, "the double-run agrees");
        assert_eq!(
            run_a.field.active_count(),
            run_b.field.active_count(),
            "the double-run's final pools agree"
        );
    }

    /// THE DIGEST PIN: the same campaign (the 132-tick warm start,
    /// then the 600-tick campaign), the FIELD folded at every 60th
    /// campaign tick (campaign tick 600's fold IS the final state's).
    /// The pinned u64 is the MEASURED actual of that campaign —
    /// cross-platform by f32 determinism, forever; a move is an
    /// investigation, never a silent repin.
    #[test]
    fn digest_pin() {
        let dt = 1.0f32 / 60.0f32;
        let mut run = CampaignRun::new();
        let mut hasher = Fingerprinter::new();
        for t in 1..=(132u32 + 600) {
            let (tick_report, ripple_report) = run.step(dt);
            assert_eq!(
                tick_report.overflow, None,
                "tick {t}: the particle pool never overflows (480 x 1.15 = 552 < 1024)"
            );
            assert_eq!(
                ripple_report.refused, 0,
                "tick {t}: the ripple pool never refuses (8.5x headroom)"
            );
            if t > 132 && (t - 132) % 60 == 0 {
                run.fold(&mut hasher);
            }
        }
        assert_eq!(run.kill_deaths, 18, "the measured kill-death count");
        assert_eq!(
            hasher.finish(),
            PINNED_CAMPAIGN_DIGEST,
            "the ripple campaign digest moved: an investigation, never a silent repin"
        );
    }

    /// The measured-actual campaign digest (the fixed campaign's
    /// field state, folded at campaign ticks 60, 120, ..., 600 —
    /// measured 2026-11-25, cross-platform by f32 determinism).
    /// RE-PINNED with cause (the z-extent corrective): the stage-1
    /// fixture's observed box carried an arithmetic slip in the
    /// derivation write-up (max z 28.0; the true quantized box is
    /// max z 32.0 — qz + 20). The z draws change, so the digest
    /// moved from 8954201978366776380 to the value below; the kill
    /// counts are UNAFFECTED (kills are y-dynamics only — the
    /// RainLayer's warm window measured the same 5 kill deaths on
    /// both boxes), and the fixture now IS the demo's exact emitter
    /// (the RainLayer's own campaign digest — engine-slice's
    /// ripple_campaign_digest_pin — pins the SAME u64: one
    /// campaign, two proofs, one digest).
    const PINNED_CAMPAIGN_DIGEST: u64 = 6_501_874_864_611_424_535;

    /// Reset replays identically (the particle reset pattern): drive
    /// 300 ticks (folding every 60th), reset BOTH halves, replay in
    /// lockstep with a fresh never-reset control — identical
    /// reports and an identical digest against the pre-reset
    /// trajectory.
    #[test]
    fn reset_replays_identically() {
        let dt = 1.0f32 / 60.0f32;
        let mut run = CampaignRun::new();
        let mut first = Fingerprinter::new();
        for t in 1..=300u32 {
            run.step(dt);
            if t % 60 == 0 {
                run.fold(&mut first);
            }
        }
        let pre_reset_kill_deaths = run.kill_deaths;
        let mut control = CampaignRun::new();
        run.reset();
        assert_eq!(run.field.active_count(), 0, "reset empties the pool");
        assert_eq!(run.field.spawned(), 0, "reset clears the census");
        assert_eq!(run.field.refused_total(), 0);
        assert_eq!(run.kill_deaths, 0);
        let mut replay = Fingerprinter::new();
        for t in 1..=300u32 {
            assert_eq!(run.step(dt), control.step(dt), "tick {t}: replay == fresh");
            if t % 60 == 0 {
                run.fold(&mut replay);
            }
        }
        assert_eq!(
            replay.finish(),
            first.finish(),
            "the replayed digest matches the pre-reset trajectory"
        );
        assert_eq!(
            run.kill_deaths, pre_reset_kill_deaths,
            "the kill deaths replay"
        );
        assert_eq!(
            run.kill_deaths, control.kill_deaths,
            "replay == fresh census"
        );
    }
}
