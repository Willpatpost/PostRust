//! The deterministic particle core (Era 2 package C — particles +
//! weather): the emitter's pure-data configuration, the bounded
//! fixed-array pool with its typed spawn-overflow refusal, and the
//! pinned fixed-order ballistic integration — ALL pure, fully
//! tested, no I/O, no wall clocks, no Vulkan, no new dependencies.
//! The animation core's discipline (pure data, fixed-order
//! arithmetic, FNV digest pins) applied to unbound particles.
//!
//! # DETERMINISM CONTRACT
//! Every byte this module produces is byte-identical on every
//! platform. The spawn stream is a SplitMix64 stream (the
//! `npc_behavior::NpcRng` family — the same finalizer, seeded per
//! emitter from the configuration's own seed); the integration is
//! the PINNED fixed order documented below; the pool is a
//! FIXED-SIZE array with an active prefix (zero allocation in the
//! tick path); overflow above the bound is a typed, counted,
//! fail-visible refusal — never a silent drop (the
//! `audio_triggers.rs` overflow-counter discipline, Package B's
//! shape). A fixed seed's particle trajectory sequence is
//! byte-identical forever — pinned by digest.
//!
//! # THE PINNED INTEGRATION ORDER (per particle, per tick, exactly)
//! 1. `velocity[1] -= gravity * dt` — gravity pulls toward -Y.
//! 2. `velocity[0] *= drag_factor; velocity[1] *= drag_factor;
//!    velocity[2] *= drag_factor` — drag is a per-step multiply by
//!    a fixed constant (no exponentials, no libm).
//! 3. `position[0] += velocity[0] * dt; position[1] +=
//!    velocity[1] * dt; position[2] += velocity[2] * dt`.
//! 4. `life -= dt`.
//! 5. Death test: dead iff `life <= 0.0` OR (`kill_y` is `Some` and
//!    `position[1] <= kill_y`). A kill-plane death is observed at
//!    the IMPACT CROSSING — the linear interpolation between the
//!    pre-step and post-step positions where the step straddles the
//!    plane (see [`kill_plane_impact`]) — never at the
//!    post-integration position below the plane (finding 12's
//!    impact-XZ correction: the ring lands where the drop crossed
//!    the world's receiver, not up to one full step past it).
//!
//! No transcendental function appears anywhere on a byte path —
//! gravity and drag are fixed constants (the libm-free law).
//!
//! # THE STABLE POOL
//! Survivors keep their relative order: the compaction sweeps the
//! active prefix in fixed index order and writes survivors to the
//! front; spawns append at the active prefix's end. The particle
//! order is therefore spawn order among survivors — part of the
//! deterministic contract, the digest depends on it.
//!
//! # THE ENGINE/POLICY BOUNDARY
//! Every constant — the spawn volume, the rate, the velocities and
//! jitters, the lifetimes, gravity, drag, the kill plane, the seed —
//! is policy data authored above the engine and validated by
//! [`EmitterConfig::validate`] before use. No rain numbers live
//! here; `weather.rs` derives the rain policy's emitter from the
//! observed volume.

use crate::fingerprint::Fingerprinter;
use std::cmp::Ordering;

/// The per-emitter particle pool bound (fixed, like MAX_NEEDS): one
/// emitter owns at most this many live particles, held in a
/// fixed-size array — the tick path never allocates.
pub const PARTICLE_POOL_BOUND: usize = 1024;

/// The spawn accumulator's ceiling — the pathological-dt guard: the
/// per-tick gain `dt * rate` clamps here BEFORE the drain loop, so
/// one tick spawns at most 1_000 particles and the drain loop is
/// structurally bounded. Still deterministic: a fixed constant
/// applied in the fixed order, before any spawn decision.
pub const SPAWN_ACCUMULATOR_CAP: f32 = 1_000.0;

/// The pool's fill value: a dead, at-rest particle. Never observed
/// outside the active prefix — the array exists only so the tick
/// path allocates nothing.
const PARTICLE_ZERO: Particle = Particle {
    position: [0.0; 3],
    velocity: [0.0; 3],
    life: 0.0,
};

/// The typed spawn overflow (fail-visible, the audio_triggers
/// overflow-counter discipline: the caller MUST observe it, never
/// swallow it). Counts spawn attempts dropped above the pool bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnOverflow {
    /// Spawn attempts dropped above [`PARTICLE_POOL_BOUND`] this tick.
    pub dropped: u32,
}

/// One particle's state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle {
    /// World-space position (meters, Y up).
    pub position: [f32; 3],
    /// Velocity (meters per second).
    pub velocity: [f32; 3],
    /// Remaining life in seconds; dead at <= 0.
    pub life: f32,
}

/// The axis-aligned spawn volume (pure data; min <= max per axis).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpawnVolume {
    /// The volume's lower corner.
    pub min: [f32; 3],
    /// The volume's upper corner.
    pub max: [f32; 3],
}

/// The emitter configuration as pure data. All fields
/// policy-supplied; validated by [`EmitterConfig::validate`] before
/// use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmitterConfig {
    /// The volume new particles spawn uniformly inside (per-axis
    /// `min + draw * (max - min)`, fixed axis order x, y, z).
    pub volume: SpawnVolume,
    /// Spawn rate in particles per second.
    pub rate: f32,
    /// The base initial velocity (fixed constants).
    pub base_velocity: [f32; 3],
    /// Per-axis velocity jitter amplitude; the draw scales each axis
    /// in [-amplitude, +amplitude] (fixed axis order x, y, z).
    pub velocity_jitter: [f32; 3],
    /// Lifetime bounds in seconds [min, max]; the draw picks
    /// `min + draw * (max - min)`.
    pub lifetime: [f32; 2],
    /// Gravity magnitude in m/s^2, applied to -Y.
    pub gravity: f32,
    /// The per-step velocity multiply (a fixed constant factor,
    /// applied every tick; 1.0 = no drag). Meaningful at a fixed dt
    /// — the demo pins dt = 1/60 (the locomotion batch's
    /// render-then-step clock).
    pub drag_factor: f32,
    /// The optional kill plane: a particle dies when position[1] <=
    /// kill_y (rain dies at the ground). None = lifetime only.
    pub kill_y: Option<f32>,
    /// The spawn-stream seed (the per-emitter own-stream rule).
    pub seed: u64,
}

/// The per-tick report (the caller MUST observe — the overflow half
/// especially; ignoring the return is the swallowed-refusal
/// failure).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickReport {
    /// Particles spawned this tick.
    pub spawned: u32,
    /// Spawn attempts dropped above the pool bound this tick.
    pub overflow: Option<SpawnOverflow>,
    /// Active particles after the tick.
    pub active: u32,
}

/// One emitter: its configuration, spawn stream, and bounded pool.
/// The pool is owned inline (7 f32 per particle at the current
/// bound — one fixed footprint for the emitter's lifetime, zero per
/// tick).
pub struct Emitter {
    /// The validated configuration (immutable for the emitter's
    /// lifetime — reset replays it).
    config: EmitterConfig,
    /// The SplitMix64 stream state (seeded from the config's seed).
    rng_state: u64,
    /// The fractional spawn carry (units of one particle).
    spawn_accumulator: f32,
    /// The bounded pool; only the prefix `[..active]` is meaningful.
    particles: [Particle; PARTICLE_POOL_BOUND],
    /// The live-prefix length.
    active: usize,
    /// Cumulative dropped spawn attempts (the observable total; the
    /// per-tick report carries the delta).
    overflow_total: u32,
}

impl EmitterConfig {
    /// Validates the configuration. Every refusal is a
    /// `&'static str` reason (the npc_memory validation-refusal
    /// style); NaN anywhere is a refusal — the ordering gates go
    /// through [`le_total`]/[`gt_total`], which decide over
    /// `partial_cmp` so a NaN side never passes (a NaN has no
    /// ordering), and the scalar gates check `is_finite` directly.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !le_total(self.volume.min[0], self.volume.max[0]) {
            return Err("particle/volume-min-above-max-x");
        }
        if !le_total(self.volume.min[1], self.volume.max[1]) {
            return Err("particle/volume-min-above-max-y");
        }
        if !le_total(self.volume.min[2], self.volume.max[2]) {
            return Err("particle/volume-min-above-max-z");
        }
        if !le_total(self.lifetime[0], self.lifetime[1]) {
            return Err("particle/lifetime-order");
        }
        if !gt_total(self.lifetime[0], 0.0) {
            return Err("particle/lifetime-min-not-positive");
        }
        if !self.rate.is_finite() || self.rate < 0.0 {
            return Err("particle/rate-negative-or-non-finite");
        }
        if !self.gravity.is_finite() || self.gravity < 0.0 {
            return Err("particle/gravity-negative-or-non-finite");
        }
        if !self.drag_factor.is_finite() || self.drag_factor <= 0.0 || self.drag_factor > 1.0 {
            return Err("particle/drag-factor-outside-unit-range");
        }
        for &velocity in &self.base_velocity {
            if !velocity.is_finite() {
                return Err("particle/base-velocity-non-finite");
            }
        }
        for &jitter in &self.velocity_jitter {
            if !jitter.is_finite() {
                return Err("particle/velocity-jitter-non-finite");
            }
        }
        if let Some(kill_y) = self.kill_y
            && !kill_y.is_finite()
        {
            return Err("particle/kill-y-non-finite");
        }
        Ok(())
    }
}

impl Emitter {
    /// Constructs the emitter: validates the configuration first —
    /// a refusal never constructs (the validation-refusal
    /// discipline). The spawn stream seeds from the config's seed
    /// through the SplitMix64 finalizer.
    pub fn new(config: EmitterConfig) -> Result<Self, &'static str> {
        config.validate()?;
        Ok(Self {
            rng_state: mix64(config.seed),
            config,
            spawn_accumulator: 0.0,
            particles: [PARTICLE_ZERO; PARTICLE_POOL_BOUND],
            active: 0,
            overflow_total: 0,
        })
    }

    /// The emitter's configuration.
    pub fn config(&self) -> &EmitterConfig {
        &self.config
    }

    /// The live-particle count (the active prefix's length).
    pub fn active_count(&self) -> usize {
        self.active
    }

    /// The active prefix — the live particles in spawn order among
    /// survivors.
    pub fn particles(&self) -> &[Particle] {
        &self.particles[..self.active]
    }

    /// The cumulative dropped spawn attempts (the observable total;
    /// the per-tick report carries the delta).
    pub fn overflow_total(&self) -> u32 {
        self.overflow_total
    }

    /// Back to the constructed state: the same seed restarts the
    /// stream, the pool empties, the overflow ledger clears — the
    /// determinism proof's tool (drive, reset, replay:
    /// byte-identical).
    pub fn reset(&mut self) {
        self.rng_state = mix64(self.config.seed);
        self.spawn_accumulator = 0.0;
        self.particles = [PARTICLE_ZERO; PARTICLE_POOL_BOUND];
        self.active = 0;
        self.overflow_total = 0;
    }

    /// Refreshes the spawn volume IN PLACE (the listener-following
    /// spawn box — the demo refreshes it every frame as the camera
    /// moves). Validates exactly as construction (the same refusal
    /// strings); the stream, the accumulator, and the live pool are
    /// UNTOUCHED — only where future spawns land changes. [Document
    /// why: the emitter's config is otherwise immutable; the rain
    /// volume follows the listener per the mission's
    /// listener-relative discipline.]
    pub fn set_volume(&mut self, volume: SpawnVolume) -> Result<(), &'static str> {
        // Validate exactly as construction
        let validated = EmitterConfig {
            volume,
            rate: self.config.rate,
            base_velocity: self.config.base_velocity,
            velocity_jitter: self.config.velocity_jitter,
            lifetime: self.config.lifetime,
            gravity: self.config.gravity,
            drag_factor: self.config.drag_factor,
            kill_y: self.config.kill_y,
            seed: self.config.seed,
        };
        validated.validate()?;
        self.config.volume = volume;
        Ok(())
    }

    /// One fixed-order tick (the no-observer form: this is exactly
    /// [`Emitter::tick_observing_kill_deaths`] with a no-op
    /// observer — one code path, one arithmetic; the digest pins
    /// rest on the delegation).
    pub fn tick(&mut self, dt: f32) -> TickReport {
        self.tick_observing_kill_deaths(dt, |_| {})
    }

    /// One fixed-order tick, observing every KILL-PLANE death's
    /// IMPACT position as it happens (the rain-ripple derivation's
    /// hook: a raindrop that reaches the kill plane is a drop
    /// hitting the ground — the ripple field spawns its ring at
    /// that death's impact position, in death order). The closure
    /// fires ONLY for kill-plane deaths (a lifetime death mid-air
    /// is not a landing), with the IMPACT position — the linear
    /// interpolation between the step's pre-integration and
    /// post-integration positions at the plane crossing (see
    /// [`kill_plane_impact`]; finding 12's impact-XZ correction) —
    /// in the integration sweep's fixed order.
    /// Behavior-identical to [`Emitter::tick`] — `tick` is this
    /// method with a no-op observer (one code path, one arithmetic;
    /// the digest pins rest on it).
    ///
    /// Phase 1, spawn: the accumulator gains `dt * rate`, clamps at
    /// [`SPAWN_ACCUMULATOR_CAP`], then drains one unit at a time —
    /// each unit spawns while the pool has room and COUNTS a typed
    /// drop when it does not (never a silent drop; the drain is
    /// structurally bounded by the cap). Phase 2, integrate: every
    /// active particle steps through THE PINNED INTEGRATION ORDER,
    /// the death test runs, every kill-plane death is observed
    /// BEFORE the compaction write decision, and survivors compact
    /// stably into the prefix. The report must be observed.
    pub fn tick_observing_kill_deaths<F: FnMut([f32; 3])>(
        &mut self,
        dt: f32,
        mut on_kill_death: F,
    ) -> TickReport {
        let mut spawned: u32 = 0;
        let mut dropped: u32 = 0;
        self.spawn_accumulator += dt * self.config.rate;
        if self.spawn_accumulator > SPAWN_ACCUMULATOR_CAP {
            self.spawn_accumulator = SPAWN_ACCUMULATOR_CAP;
        }
        while self.spawn_accumulator >= 1.0 {
            self.spawn_accumulator -= 1.0;
            if self.active < PARTICLE_POOL_BOUND {
                self.spawn_one();
                spawned += 1;
            } else {
                dropped += 1;
            }
        }
        let gravity = self.config.gravity;
        let drag_factor = self.config.drag_factor;
        let kill_y = self.config.kill_y;
        let mut write = 0usize;
        for i in 0..self.active {
            let mut p = self.particles[i];
            // The step's pre-integration endpoint — the
            // interpolation's left side, captured BEFORE the
            // integration mutates p (finding 12's impact crossing).
            let pre_position = p.position;
            // 1. gravity
            p.velocity[1] -= gravity * dt;
            // 2. drag (the per-step multiply, fixed axis order)
            p.velocity[0] *= drag_factor;
            p.velocity[1] *= drag_factor;
            p.velocity[2] *= drag_factor;
            // 3. position (fixed axis order)
            p.position[0] += p.velocity[0] * dt;
            p.position[1] += p.velocity[1] * dt;
            p.position[2] += p.velocity[2] * dt;
            // 4. life
            p.life -= dt;
            // 5. death test: dead iff the lifetime expired OR the
            //    kill plane was reached. A kill-plane death (the
            //    kill condition holds — alone or alongside an
            //    expired lifetime; a particle can satisfy both, it
            //    is a kill death iff the kill condition holds) is
            //    observed BEFORE the compaction write decision, at
            //    the IMPACT CROSSING between the pre-step and
            //    post-step positions, in sweep order.
            let kill_death = kill_y.is_some_and(|k| p.position[1] <= k);
            if kill_death {
                let impact = kill_plane_impact(pre_position, p.position, kill_y);
                on_kill_death(impact);
            }
            let dead = p.life <= 0.0 || kill_death;
            if !dead {
                self.particles[write] = p;
                write += 1;
            }
        }
        self.active = write;
        self.overflow_total += dropped;
        TickReport {
            spawned,
            overflow: (dropped > 0).then_some(SpawnOverflow { dropped }),
            active: self.active as u32,
        }
    }

    /// Folds every ACTIVE particle's `position.to_bits()` and
    /// `life.to_bits()` into the FNV-1a builder in pool order — x,
    /// y, z, life per particle, fixed (velocity is deliberately not
    /// folded: the visible state is the pin).
    pub fn fold_state(&self, hash: &mut Fingerprinter) {
        for p in &self.particles[..self.active] {
            hash.write_u32(p.position[0].to_bits());
            hash.write_u32(p.position[1].to_bits());
            hash.write_u32(p.position[2].to_bits());
            hash.write_u32(p.life.to_bits());
        }
    }

    /// Spawns one particle at the active prefix's end. The draw
    /// order is FIXED: position x, y, z, then velocity x, y, z,
    /// then lifetime — seven stream draws per spawn, forever in
    /// this order.
    fn spawn_one(&mut self) {
        let volume = self.config.volume;
        let base = self.config.base_velocity;
        let jitter = self.config.velocity_jitter;
        let lifetime = self.config.lifetime;
        let mut p = PARTICLE_ZERO;
        p.position[0] = volume.min[0] + self.next_unit() * (volume.max[0] - volume.min[0]);
        p.position[1] = volume.min[1] + self.next_unit() * (volume.max[1] - volume.min[1]);
        p.position[2] = volume.min[2] + self.next_unit() * (volume.max[2] - volume.min[2]);
        p.velocity[0] = base[0] + (self.next_unit() * 2.0 - 1.0) * jitter[0];
        p.velocity[1] = base[1] + (self.next_unit() * 2.0 - 1.0) * jitter[1];
        p.velocity[2] = base[2] + (self.next_unit() * 2.0 - 1.0) * jitter[2];
        p.life = lifetime[0] + self.next_unit() * (lifetime[1] - lifetime[0]);
        self.particles[self.active] = p;
        self.active += 1;
    }

    /// The next draw in [0, 1) from the emitter's own stream (the
    /// NpcRng family's arithmetic, verbatim — defined locally so
    /// this module never imports another module's RNG).
    fn next_unit(&mut self) -> f32 {
        self.rng_state = self.rng_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        (mix64(self.rng_state) >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Whether `a <= b` holds as a total decision — `false` when either
/// side is NaN (a NaN has no ordering, so the validation's ordering
/// gates refuse NaN volumes and lifetimes: "NaN anywhere is a
/// refusal"). The `partial_cmp` form keeps the NaN catch explicit
/// without a negated partial comparison.
fn le_total(a: f32, b: f32) -> bool {
    matches!(a.partial_cmp(&b), Some(Ordering::Less | Ordering::Equal))
}

/// Whether `a > b` holds as a total decision — the same NaN
/// discipline as [`le_total`].
fn gt_total(a: f32, b: f32) -> bool {
    matches!(a.partial_cmp(&b), Some(Ordering::Greater))
}

/// The IMPACT position where one integration step crossed the kill
/// plane (finding 12's impact-XZ correction). The step's endpoints
/// are `pre` (the position before the step) and `post` (the
/// position after it, at or below the plane — the caller only
/// invokes this for a kill death). When the step STRADDLES the
/// plane (`pre[1] > kill_y >= post[1]`), the crossing is the linear
/// interpolation between the endpoints: the fraction
/// `alpha = (pre.y − kill_y) / (pre.y − post.y)` of the step at
/// which the drop met the plane, applied to X and Z (the impact's
/// XZ — what the ripple ring consumes), with Y set to the plane
/// itself (the crossing's Y is the plane by definition). When the
/// particle was ALREADY at or below the plane when the step began
/// (a spawn below the receiver — degenerate authored data), the
/// crossing happened at or before the step and the honest in-step
/// estimate is the post-step position.
///
/// Pure fixed-order IEEE f32 — no transcendental, byte-identical on
/// every platform; the particle-state path is UNTOUCHED (the death
/// test and compaction read the post-integration position exactly
/// as before — only the OBSERVED impact moves).
fn kill_plane_impact(pre: [f32; 3], post: [f32; 3], kill_y: Option<f32>) -> [f32; 3] {
    match kill_y {
        Some(k) if pre[1] >= k => {
            let span = pre[1] - post[1];
            // span > 0 whenever post[1] <= k < pre[1] (the straddling
            // case this helper exists for); the clamp keeps a
            // degenerate pair (post[1] == pre[1] cannot reach the
            // death test, but arithmetic honesty costs nothing)
            // inside the step.
            let alpha = if span > 0.0 {
                ((pre[1] - k) / span).clamp(0.0, 1.0)
            } else {
                1.0
            };
            [
                pre[0] + (post[0] - pre[0]) * alpha,
                k,
                pre[2] + (post[2] - pre[2]) * alpha,
            ]
        }
        // No kill plane (unreachable from the kill-death branch) or
        // the degenerate already-below case: the post position.
        _ => post,
    }
}

/// The SplitMix64 finalizer — the same family as the repo's
/// procedural textures and `npc_behavior::NpcRng` (identical bits
/// on every platform).
fn mix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spec's rain-like campaign configuration (tests 4, 5, 9,
    /// 10): volume min [-20, 12, -20] max [20, 16, 20], rate 480,
    /// base velocity [0, -9, 0], jitter [0.6, 0.15, 0.6], lifetime
    /// [0.9, 1.4], gravity 9.81, drag 0.982, kill plane y = 0.
    fn rain_like_campaign(seed: u64) -> EmitterConfig {
        EmitterConfig {
            volume: SpawnVolume {
                min: [-20.0, 12.0, -20.0],
                max: [20.0, 16.0, 20.0],
            },
            rate: 480.0,
            base_velocity: [0.0, -9.0, 0.0],
            velocity_jitter: [0.6, 0.15, 0.6],
            lifetime: [0.9, 1.4],
            gravity: 9.81,
            drag_factor: 0.982,
            kill_y: Some(0.0),
            seed,
        }
    }

    /// KNOWN ANSWER 1 — gravity only, one step: from rest at a
    /// single point, the pinned order gives
    /// `velocity[1] = -(g*dt)` exactly (drag 1.0 is an exact no-op),
    /// `position[1] = velocity[1] * dt` (the spawn-tick integration
    /// — a particle spawned this tick is integrated this tick), and
    /// `life = lifetime - dt`. Every expected value is written as
    /// the SAME f32 expression the integration performs — bit-exact
    /// by construction, not by tolerance.
    #[test]
    fn known_answer_gravity_only_single_step() {
        let dt = 1.0f32 / 60.0f32;
        let config = EmitterConfig {
            volume: SpawnVolume {
                min: [0.0; 3],
                max: [0.0; 3],
            },
            rate: 6_000.0,
            base_velocity: [0.0; 3],
            velocity_jitter: [0.0; 3],
            lifetime: [10.0, 10.0],
            gravity: 9.81,
            drag_factor: 1.0,
            kill_y: None,
            seed: 1,
        };
        let mut emitter = Emitter::new(config).expect("the config validates");
        let report = emitter.tick(dt);
        assert!(report.spawned >= 1, "the rate spawns on the first tick");
        assert_eq!(report.active, report.spawned, "nothing dies");
        let p = &emitter.particles()[0];
        assert_eq!(p.velocity[0], 0.0);
        assert_eq!(
            p.velocity[1],
            -(9.81f32 * dt),
            "gravity, then the exact no-op drag"
        );
        assert_eq!(p.velocity[2], 0.0);
        assert_eq!(
            p.position[1],
            -(9.81f32 * dt) * dt,
            "the post-integration position"
        );
        assert_eq!(p.life, 10.0f32 - dt);
    }

    /// KNOWN ANSWER 2 — the drag terminal shape: with drag factor d
    /// and gravity g, the pinned order (gravity FIRST, then the
    /// drag multiply) gives, from rest after one tick,
    /// `v1 = -(g*dt) * d` exactly. THE FIXED POINT: the recurrence
    /// `v_{n+1} = (v_n - g*dt) * d` has the terminal velocity
    /// `v_term = (v_term - g*dt) * d`, i.e.
    /// `v_term = -g*dt*d / (1-d)`. Rain's choice — g = 9.81,
    /// d = 0.982 at dt = 1/60 — gives v_term ≈ -8.92 m/s ≈ 9 m/s
    /// downward: the drag exists to hold the fall at a believable
    /// terminal speed, not to slow it freely.
    #[test]
    fn known_answer_drag_terminal_shape() {
        let dt = 1.0f32 / 60.0f32;
        let g = 9.81f32;
        let d = 0.982f32;
        let config = EmitterConfig {
            volume: SpawnVolume {
                min: [0.0; 3],
                max: [0.0; 3],
            },
            rate: 6_000.0,
            base_velocity: [0.0; 3],
            velocity_jitter: [0.0; 3],
            lifetime: [10.0, 10.0],
            gravity: g,
            drag_factor: d,
            kill_y: None,
            seed: 2,
        };
        let mut emitter = Emitter::new(config).expect("the config validates");
        emitter.tick(dt);
        let p = &emitter.particles()[0];
        assert_eq!(
            p.velocity[1],
            -(g * dt) * d,
            "gravity, THEN the drag multiply"
        );
        assert_eq!(p.position[1], (-(g * dt) * d) * dt);
    }

    /// KNOWN ANSWER 3 — the kill plane: a particle at y = 21 with a
    /// constant -20 m/s fall and a 100 s life dies exactly on the
    /// tick its position[1] <= 0. The tick count is derived by hand
    /// from the SAME f32 arithmetic the integration performs: the
    /// spawn tick integrates with dt = 1.0 (the accumulator's exact
    /// single unit at rate 1.0 — one spawn, then silence), then
    /// every 1/60 tick falls one fixed step.
    #[test]
    fn known_answer_kill_plane() {
        let dt = 1.0f32 / 60.0f32;
        let fall = -20.0f32;
        let config = EmitterConfig {
            volume: SpawnVolume {
                min: [0.0, 21.0, 0.0],
                max: [0.0, 21.0, 0.0],
            },
            rate: 1.0,
            base_velocity: [0.0, fall, 0.0],
            velocity_jitter: [0.0; 3],
            lifetime: [100.0, 100.0],
            gravity: 0.0,
            drag_factor: 1.0,
            kill_y: Some(0.0),
            seed: 3,
        };
        // The hand count: the spawn tick's own integration first.
        let mut y = 21.0f32;
        y += fall * 1.0f32;
        assert_eq!(y, 1.0f32, "alive after the spawn tick: {y} > 0");
        let mut death_tick = 0u32;
        while y > 0.0 {
            death_tick += 1;
            y += fall * dt;
        }
        assert!(death_tick >= 1, "the fall reaches the plane");

        let mut emitter = Emitter::new(config).expect("the config validates");
        let report = emitter.tick(1.0f32);
        assert_eq!(report.spawned, 1, "dt 1.0 x rate 1.0: exactly one unit");
        assert_eq!(report.active, 1);
        assert_eq!(
            emitter.particles()[0].position[1],
            1.0f32,
            "the known spawn-tick position"
        );
        for t in 1..death_tick {
            let report = emitter.tick(dt);
            assert_eq!(report.spawned, 0, "tick {t}: no respawn inside the window");
            assert_eq!(report.active, 1, "tick {t}: still above the kill plane");
        }
        let report = emitter.tick(dt);
        assert_eq!(report.spawned, 0);
        assert_eq!(report.active, 0, "dead EXACTLY when position[1] <= kill_y");
        assert_eq!(
            emitter.overflow_total(),
            0,
            "no drops anywhere in this test"
        );
    }

    /// THE KILL-DEATH OBSERVATION (the ripple derivation's hook):
    /// the observer fires ONLY for kill-plane deaths, in the
    /// integration sweep's fixed order, at the IMPACT CROSSING —
    /// the linear interpolation between the pre-step and post-step
    /// positions where the step straddles the plane (finding 12's
    /// impact-XZ correction) — and `tick` delegates to
    /// `tick_observing_kill_deaths(dt, |_| {})` behavior-identically
    /// (the delegation-equivalence form over a 300-tick rain-like
    /// campaign).
    ///
    /// EMITTER A (the kill cohort): five particles spawn on the
    /// prime tick (dt 1.0 x rate 5.0 makes the accumulator exactly
    /// 5.0 — five spawns, drained to exactly 0.0; the window's
    /// 5/60-per-tick gain stays under one unit, so nothing
    /// respawns), fall at a fixed -20 m/s from the y = 21 line
    /// (zero gravity, drag 1.0 — the velocity is the constant
    /// -20.0), and ALL die at the kill plane y = 0 on the SAME
    /// tick (the volume's y is a point — the five integrate
    /// identically). The death tick and the pre-death state are
    /// derived by REPLICATING the accumulation (the
    /// known_answer_kill_plane pattern: f32 position accumulation
    /// is NOT k*dt, so the test runs the same loop); each observed
    /// death's x/z must equal the pre-death pool's x/z AT THE SAME
    /// INDEX (zero horizontal velocity — the interpolation
    /// preserves endpoints that do not move), and its Y is the
    /// PLANE ITSELF (0.0 — the crossing's Y is the plane by
    /// definition), never the post-integration y below it.
    ///
    /// EMITTER B (the lifetime cohort): the same fall, no kill
    /// plane, lifetime 0.05 s — every particle expires mid-air ON
    /// ITS SPAWN TICK (0.05 - 1.0 < 0, position y = 1.0 > 0) and
    /// the observer stays SILENT.
    ///
    /// EMITTER C (the both-conditions cohort): the same fall,
    /// lifetime 0.05 s, kill plane ABOVE the spawn line (25 > 21) —
    /// every particle dies with BOTH conditions true (life < 0 AND
    /// post-integration y = 1.0 <= 25) and the observer FIRES: a
    /// death is a kill death iff the kill condition holds.
    #[test]
    fn kill_death_observation_known_answer_and_delegation_equivalence() {
        let dt = 1.0f32 / 60.0f32;
        let fall = -20.0f32;
        // --- EMITTER A: five kill-plane deaths in sweep order ---
        let config_a = EmitterConfig {
            volume: SpawnVolume {
                min: [-4.0, 21.0, 0.0],
                max: [4.0, 21.0, 0.0],
            },
            rate: 5.0,
            base_velocity: [0.0, fall, 0.0],
            velocity_jitter: [0.0; 3],
            lifetime: [100.0, 100.0],
            gravity: 0.0,
            drag_factor: 1.0,
            kill_y: Some(0.0),
            seed: 0x01A,
        };
        // The hand derivation: the spawn tick's own integration
        // first (dt 1.0 — the accumulator's exact five units, then
        // silence), then the 1/60 fall until the plane.
        let mut y = 21.0f32;
        y += fall * 1.0f32;
        assert_eq!(y, 1.0f32, "alive after the spawn tick: {y} > 0");
        let mut death_tick = 0u32;
        while y > 0.0 {
            death_tick += 1;
            y += fall * dt;
        }
        assert!(death_tick >= 1, "the fall reaches the plane");
        let death_y = y; // the post-integration y at the death tick (below the plane)
        assert!(
            death_y <= 0.0 && death_y > fall * dt,
            "the death step straddles the plane: post-y {death_y} in one step past it"
        );

        let mut emitter = Emitter::new(config_a).expect("the config validates");
        let mut observed: Vec<[f32; 3]> = Vec::new();
        let report = emitter.tick_observing_kill_deaths(1.0f32, |p| observed.push(p));
        assert_eq!(report.spawned, 5, "the prime tick's exact five units");
        assert_eq!(
            report.active, 5,
            "y = 1.0 > 0: nobody dies on the spawn tick"
        );
        assert!(observed.is_empty(), "no death yet: the observer is silent");
        for t in 1..death_tick {
            let report = emitter.tick_observing_kill_deaths(dt, |p| observed.push(p));
            assert_eq!(report.spawned, 0, "tick {t}: no respawn inside the window");
            assert_eq!(report.active, 5, "tick {t}: still above the kill plane");
            assert!(observed.is_empty(), "tick {t}: still silent");
        }
        // The pre-death pool: the sweep's order source (the x/z the
        // deaths must report — they never move horizontally).
        let pre_death: Vec<[f32; 3]> = emitter.particles().iter().map(|p| p.position).collect();
        assert_eq!(pre_death.len(), 5);
        let report = emitter.tick_observing_kill_deaths(dt, |p| observed.push(p));
        assert_eq!(report.spawned, 0);
        assert_eq!(
            report.active, 0,
            "the whole cohort died EXACTLY when y <= 0"
        );
        assert_eq!(
            observed.len(),
            5,
            "every kill-plane death observed, none doubled"
        );
        for (i, position) in observed.iter().enumerate() {
            assert_eq!(
                *position,
                [pre_death[i][0], 0.0, pre_death[i][2]],
                "death {i}: the IMPACT crossing — y exactly at the plane (never the \
                 post-integration {death_y}), the x/z the interpolation preserves \
                 (zero horizontal velocity), in sweep order"
            );
        }

        // --- EMITTER B: lifetime deaths only — the observer stays
        // silent (a mid-air expiry is not a landing) ---
        let config_b = EmitterConfig {
            volume: SpawnVolume {
                min: [-4.0, 21.0, 0.0],
                max: [4.0, 21.0, 0.0],
            },
            rate: 5.0,
            base_velocity: [0.0, fall, 0.0],
            velocity_jitter: [0.0; 3],
            lifetime: [0.05, 0.05],
            gravity: 0.0,
            drag_factor: 1.0,
            kill_y: None,
            seed: 0x01B,
        };
        let mut emitter_b = Emitter::new(config_b).expect("the config validates");
        let mut observed_b: Vec<[f32; 3]> = Vec::new();
        let report_b = emitter_b.tick_observing_kill_deaths(1.0f32, |p| observed_b.push(p));
        assert_eq!(report_b.spawned, 5, "the same prime tick");
        assert_eq!(
            report_b.active, 0,
            "lifetime 0.05 - dt 1.0 < 0: expired on the spawn tick"
        );
        assert!(
            observed_b.is_empty(),
            "a lifetime death mid-air is not a landing"
        );
        for _ in 0..3 {
            emitter_b.tick_observing_kill_deaths(dt, |p| observed_b.push(p));
        }
        assert!(observed_b.is_empty(), "the silence holds");

        // --- EMITTER C: BOTH conditions true — the kill condition
        // governs (a death is a kill death iff it holds) ---
        let config_c = EmitterConfig {
            volume: SpawnVolume {
                min: [-2.0, 21.0, 3.0],
                max: [-2.0, 21.0, 3.0],
            },
            rate: 5.0,
            base_velocity: [0.0, fall, 0.0],
            velocity_jitter: [0.0; 3],
            lifetime: [0.05, 0.05],
            gravity: 0.0,
            drag_factor: 1.0,
            kill_y: Some(25.0), // ABOVE the spawn line: reached immediately
            seed: 0x01C,
        };
        let mut emitter_c = Emitter::new(config_c).expect("the config validates");
        let mut observed_c: Vec<[f32; 3]> = Vec::new();
        let report_c = emitter_c.tick_observing_kill_deaths(1.0f32, |p| observed_c.push(p));
        assert_eq!(report_c.spawned, 5, "the same prime tick");
        assert_eq!(
            report_c.active, 0,
            "life 0.05 - 1.0 < 0 AND y 1.0 <= 25: both conditions"
        );
        assert_eq!(
            observed_c.len(),
            5,
            "the kill condition holds: all five observed"
        );
        for position in &observed_c {
            assert_eq!(
                *position,
                [-2.0, 21.0f32 + fall * 1.0f32, 3.0],
                "the DEGENERATE crossing of a both-conditions death: the particle \
                 was already below the plane (21 <= 25) when the step began, so the \
                 honest in-step impact is the post-integration position"
            );
        }

        // --- THE DELEGATION EQUIVALENCE: `tick` is
        // `tick_observing_kill_deaths(dt, |_| {})` — identical
        // reports and identical final pools over a 300-tick
        // rain-like campaign.
        let mut plain = Emitter::new(rain_like_campaign(0xD1E6)).unwrap();
        let mut observing = Emitter::new(rain_like_campaign(0xD1E6)).unwrap();
        for t in 1..=300u32 {
            assert_eq!(
                plain.tick(dt),
                observing.tick_observing_kill_deaths(dt, |_| {}),
                "tick {t}: the no-op observer changes nothing"
            );
        }
        assert_eq!(
            plain.particles(),
            observing.particles(),
            "identical final pools"
        );
        let mut hash_plain = Fingerprinter::new();
        plain.fold_state(&mut hash_plain);
        let mut hash_observing = Fingerprinter::new();
        observing.fold_state(&mut hash_observing);
        assert_eq!(
            hash_plain.finish(),
            hash_observing.finish(),
            "identical final digests"
        );
    }

    /// THE IMPACT CROSSING, hand-computed (finding 12's impact-XZ
    /// correction, the helper's own known answers — every expected
    /// value written as the SAME f32 expression the helper
    /// performs, bit-exact by construction):
    /// - the straddling step with LATERAL drift: alpha is the
    ///   fraction of the step at which the drop met the plane,
    ///   applied to X and Z, with Y the plane itself;
    /// - pre exactly ON the plane: alpha 0, the impact is the
    ///   pre-step XZ at the plane;
    /// - post exactly ON the plane: alpha 1, the impact is the
    ///   post-step XZ at the plane;
    /// - the degenerate already-below pair (emitter C's shape): the
    ///   post position, verbatim;
    /// - no kill plane at all: the post position (unreachable from
    ///   the kill-death branch, honest anyway).
    #[test]
    fn kill_plane_impact_known_answers() {
        // The straddling step with lateral drift: pre y 0.4, post y
        // -0.2, plane 0 — alpha = 0.4 / 0.6.
        let pre = [10.0f32, 0.4, -6.0];
        let post = [10.5f32, -0.2, -6.25];
        let alpha = 0.4f32 / (0.4f32 - -0.2f32);
        assert_eq!(
            kill_plane_impact(pre, post, Some(0.0)),
            [
                10.0f32 + (10.5f32 - 10.0f32) * alpha,
                0.0,
                -6.0f32 + (-6.25f32 - -6.0f32) * alpha
            ],
            "the straddling crossing: the interpolated XZ at the plane"
        );
        // Pre exactly on the plane: alpha 0.
        assert_eq!(
            kill_plane_impact([1.0, 5.0, 2.0], [1.5, 4.9, 2.5], Some(5.0)),
            [1.0, 5.0, 2.0],
            "pre on the plane: the crossing is the step's start"
        );
        // Post exactly on the plane: alpha 1.
        assert_eq!(
            kill_plane_impact([1.0, 0.3, 2.0], [1.5, 0.0, 2.5], Some(0.0)),
            [1.5, 0.0, 2.5],
            "post on the plane: the crossing is the step's end"
        );
        // The degenerate already-below pair: the post position.
        assert_eq!(
            kill_plane_impact([-2.0, 21.0, 3.0], [-2.0, 1.0, 3.0], Some(25.0)),
            [-2.0, 1.0, 3.0],
            "already below at the step's start: the honest in-step estimate is post"
        );
        // No kill plane: the post position.
        assert_eq!(
            kill_plane_impact([1.0, 2.0, 3.0], [4.0, 5.0, 6.0], None),
            [4.0, 5.0, 6.0],
            "no plane: the post position"
        );
    }

    /// THE IMPACT CROSSING THROUGH THE REAL PATH, with lateral
    /// drift (the wiring proof — emitter A's zero-drift cohort
    /// cannot see a mis-captured pre-step XZ): five identical
    /// particles fall at a fixed -20 m/s while drifting at
    /// (3, 0, -5) m/s from the point (4, 21.8, 7), kill plane 0.
    /// The death tick, the pre-death state, and the expected impact
    /// are derived by REPLICATING the integration's f32
    /// accumulation and the helper's own expression — the observed
    /// deaths must equal that impact exactly, Y at the plane.
    #[test]
    fn kill_death_impact_crossing_with_lateral_drift() {
        let dt = 1.0f32 / 60.0f32;
        let fall = -20.0f32;
        let drift = [3.0f32, -5.0f32];
        let spawn = [4.0f32, 21.8, 7.0];
        let config = EmitterConfig {
            volume: SpawnVolume {
                min: spawn,
                max: spawn,
            },
            rate: 5.0,
            base_velocity: [drift[0], fall, drift[1]],
            velocity_jitter: [0.0; 3],
            lifetime: [100.0, 100.0],
            gravity: 0.0,
            drag_factor: 1.0,
            kill_y: Some(0.0),
            seed: 0x01D,
        };
        // The replica: the spawn tick's own integration first (dt
        // 1.0 — the accumulator's exact five units, then silence),
        // then the 1/60 steps until the plane, tracking the DEATH
        // STEP's pre and post endpoints.
        let mut pos = spawn;
        pos[0] += drift[0] * 1.0f32;
        pos[1] += fall * 1.0f32;
        pos[2] += drift[1] * 1.0f32;
        assert!(pos[1] > 0.0, "alive after the spawn tick");
        let mut death_tick = 0u32;
        let mut pre;
        loop {
            pre = pos;
            pos = [
                pos[0] + drift[0] * dt,
                pos[1] + fall * dt,
                pos[2] + drift[1] * dt,
            ];
            death_tick += 1;
            if pos[1] <= 0.0 {
                break;
            }
        }
        assert!(death_tick >= 1, "the drift-fall reaches the plane");
        assert!(pre[1] > 0.0 && pos[1] <= 0.0, "the death step straddles");
        // The expected impact: the helper's own expression over the
        // replicated endpoints.
        let alpha = (pre[1] / (pre[1] - pos[1])).clamp(0.0, 1.0);
        let expected = [
            pre[0] + (pos[0] - pre[0]) * alpha,
            0.0,
            pre[2] + (pos[2] - pre[2]) * alpha,
        ];

        let mut emitter = Emitter::new(config).expect("the config validates");
        let mut observed: Vec<[f32; 3]> = Vec::new();
        let report = emitter.tick_observing_kill_deaths(1.0f32, |p| observed.push(p));
        assert_eq!(report.spawned, 5, "the prime tick's exact five units");
        assert_eq!(report.active, 5, "alive after the spawn tick");
        assert!(observed.is_empty(), "no death yet: the observer is silent");
        for t in 1..death_tick {
            let report = emitter.tick_observing_kill_deaths(dt, |p| observed.push(p));
            assert_eq!(report.spawned, 0, "tick {t}: no respawn inside the window");
            assert_eq!(report.active, 5, "tick {t}: still above the kill plane");
            assert!(observed.is_empty(), "tick {t}: still silent");
        }
        let report = emitter.tick_observing_kill_deaths(dt, |p| observed.push(p));
        assert_eq!(report.spawned, 0);
        assert_eq!(report.active, 0, "the whole cohort died at the plane");
        assert_eq!(observed.len(), 5, "every kill-plane death observed");
        for (i, position) in observed.iter().enumerate() {
            assert_eq!(
                *position, expected,
                "death {i}: the interpolated crossing — the drifted XZ at the plane, \
                 never either endpoint (pre {:?}, post {:?})",
                pre, pos
            );
        }
    }

    /// THE BYTE-IDENTICAL SEQUENCE: two emitters, same config and
    /// seed, driven 600 ticks at dt = 1/60 — identical per-tick
    /// reports, identical full particle arrays (PartialEq over the
    /// prefix), and identical folded digests at every sampled tick.
    #[test]
    fn fixed_seed_sequence_byte_identical() {
        let dt = 1.0f32 / 60.0f32;
        let mut a = Emitter::new(rain_like_campaign(0xC0FFEE)).unwrap();
        let mut b = Emitter::new(rain_like_campaign(0xC0FFEE)).unwrap();
        for t in 1..=600u32 {
            assert_eq!(a.tick(dt), b.tick(dt), "tick {t}: identical reports");
            if t % 60 == 0 {
                assert_eq!(a.particles(), b.particles(), "tick {t}: identical prefixes");
                let mut ha = Fingerprinter::new();
                a.fold_state(&mut ha);
                let mut hb = Fingerprinter::new();
                b.fold_state(&mut hb);
                assert_eq!(ha.finish(), hb.finish(), "tick {t}: identical digests");
            }
        }
    }

    /// The determinism discriminant: same config, seeds 1 and 2 —
    /// the states differ (against a same-seed control that stays
    /// identical).
    #[test]
    fn different_seeds_differ() {
        let dt = 1.0f32 / 60.0f32;
        let mut a = Emitter::new(rain_like_campaign(1)).unwrap();
        let mut b = Emitter::new(rain_like_campaign(2)).unwrap();
        let mut control = Emitter::new(rain_like_campaign(1)).unwrap();
        for _ in 0..60 {
            assert_eq!(
                a.tick(dt),
                control.tick(dt),
                "the control: same seed, same reports"
            );
            let _report_b = b.tick(dt);
        }
        assert_eq!(a.particles(), control.particles(), "the control holds");
        assert_ne!(
            a.particles(),
            b.particles(),
            "different seeds produce different particle states"
        );
        let mut ha = Fingerprinter::new();
        a.fold_state(&mut ha);
        let mut hb = Fingerprinter::new();
        b.fold_state(&mut hb);
        assert_ne!(ha.finish(), hb.finish(), "the digests differ too");
    }

    /// THE TYPED OVERFLOW, never silent: rate 100_000 at dt = 1/60
    /// gains ~1666.67 units per tick — the cap clamps to 1_000, so
    /// tick 1 spawns 1_000 (the pool has room), tick 2 fills the
    /// remaining 24 slots and COUNTS the 976 dropped units, and
    /// tick 3 (identical, the second tick after saturation) counts
    /// 1_000 more. Every count is DERIVED by replicating the
    /// accumulator arithmetic (the cap included) — never an
    /// invented number — and the pool saturates exactly at
    /// PARTICLE_POOL_BOUND with the total accumulating.
    #[test]
    fn overflow_typed_never_silent() {
        let dt = 1.0f32 / 60.0f32;
        let rate = 100_000.0f32;
        let config = EmitterConfig {
            volume: SpawnVolume {
                min: [0.0; 3],
                max: [0.0; 3],
            },
            rate,
            base_velocity: [0.0; 3],
            velocity_jitter: [0.0; 3],
            lifetime: [100.0, 100.0],
            gravity: 0.0,
            drag_factor: 1.0,
            kill_y: None,
            seed: 6,
        };
        let mut emitter = Emitter::new(config).expect("the config validates");
        // The replica: the same accumulator arithmetic (cap
        // included), deriving each tick's exact unit count.
        let mut acc = 0.0f32;
        let mut active = 0usize;
        let mut total_dropped = 0u32;
        for tick in 1..=3u32 {
            acc += dt * rate;
            if acc > SPAWN_ACCUMULATOR_CAP {
                acc = SPAWN_ACCUMULATOR_CAP;
            }
            let mut units = 0u32;
            while acc >= 1.0 {
                acc -= 1.0;
                units += 1;
            }
            let capacity = (PARTICLE_POOL_BOUND - active) as u32;
            let expected_spawned = units.min(capacity);
            let expected_dropped = units - expected_spawned;
            active += expected_spawned as usize; // lifetime 100 s: nobody dies in 3 ticks
            let report = emitter.tick(dt);
            assert_eq!(
                report.spawned, expected_spawned,
                "tick {tick}: spawned derived"
            );
            assert_eq!(
                report.overflow,
                (expected_dropped > 0).then_some(SpawnOverflow {
                    dropped: expected_dropped
                }),
                "tick {tick}: the overflow is typed and derived"
            );
            assert_eq!(report.active, active as u32, "tick {tick}: active derived");
            total_dropped += expected_dropped;
            assert_eq!(
                emitter.overflow_total(),
                total_dropped,
                "tick {tick}: the observable total accumulates"
            );
        }
        assert_eq!(
            emitter.active_count(),
            PARTICLE_POOL_BOUND,
            "the pool saturates"
        );
        assert!(
            emitter.overflow_total() > 0,
            "drops were counted, never silently dropped"
        );
    }

    /// THE STABLE POOL: one cohort of five (the prime tick's dt 1.0
    /// at rate 5.0 makes the accumulator exactly 5.0 — five spawns,
    /// drained to exactly 0.0; the remaining window's 5/60-per-tick
    /// gain stays under one unit, so nothing respawns), static
    /// positions (zero velocity, zero gravity), and lifetimes drawn
    /// in [1.05, 1.15] — after the prime tick's own 1.0 they expire
    /// across the next ~9 ticks. The survivors must always be an
    /// IN-ORDER subsequence of the previous prefix: the compaction
    /// never reorders.
    #[test]
    fn stable_order_compaction() {
        let dt = 1.0f32 / 60.0f32;
        let config = EmitterConfig {
            volume: SpawnVolume {
                min: [-5.0, 0.0, 0.0],
                max: [5.0, 0.0, 0.0],
            },
            rate: 5.0,
            base_velocity: [0.0; 3],
            velocity_jitter: [0.0; 3],
            lifetime: [1.05, 1.15],
            gravity: 0.0,
            drag_factor: 1.0,
            kill_y: None,
            seed: 0x057A_B1E5,
        };
        let mut emitter = Emitter::new(config).expect("the config validates");
        let report = emitter.tick(1.0f32);
        assert_eq!(report.spawned, 5);
        assert_eq!(report.active, 5);
        let mut previous: Vec<[f32; 3]> = emitter.particles().iter().map(|p| p.position).collect();
        for t in 1..=11u32 {
            let report = emitter.tick(dt);
            assert_eq!(report.spawned, 0, "tick {t}: no respawn inside the window");
            let current: Vec<[f32; 3]> = emitter.particles().iter().map(|p| p.position).collect();
            let mut scan = 0usize;
            for position in &current {
                while scan < previous.len() && previous[scan] != *position {
                    scan += 1;
                }
                assert!(
                    scan < previous.len(),
                    "tick {t}: {position:?} broke the spawn order"
                );
                scan += 1;
            }
            previous = current;
        }
        assert_eq!(emitter.active_count(), 0, "the whole cohort expired");
    }

    /// Every refusal branch of validate(), each returning its exact
    /// reason — and a refusal never constructs an Emitter.
    #[test]
    fn validation_refusals() {
        let base = || rain_like_campaign(8);
        assert_eq!(base().validate(), Ok(()), "the baseline passes");
        // Volume ordering, per axis (min above max) — each axis
        // names itself in the refusal.
        for (axis, suffix) in ["x", "y", "z"].iter().enumerate() {
            let mut bad = base();
            bad.volume.min[axis] = 1.0;
            bad.volume.max[axis] = 0.0;
            let expected = match *suffix {
                "x" => "particle/volume-min-above-max-x",
                "y" => "particle/volume-min-above-max-y",
                _ => "particle/volume-min-above-max-z",
            };
            assert_eq!(bad.validate(), Err(expected), "axis {axis}");
        }
        // A NaN coordinate refuses through the same ordering gate
        // (a NaN comparison is false, so min <= max fails).
        let mut bad = base();
        bad.volume.min[1] = f32::NAN;
        assert_eq!(bad.validate(), Err("particle/volume-min-above-max-y"));
        // Lifetime ordering (reversed, and NaN through the gate).
        let mut bad = base();
        bad.lifetime = [2.0, 1.0];
        assert_eq!(bad.validate(), Err("particle/lifetime-order"));
        let mut bad = base();
        bad.lifetime = [f32::NAN, 1.0];
        assert_eq!(bad.validate(), Err("particle/lifetime-order"));
        // Non-positive minimum lifetime (zero and negative).
        let mut bad = base();
        bad.lifetime = [0.0, 1.0];
        assert_eq!(bad.validate(), Err("particle/lifetime-min-not-positive"));
        let mut bad = base();
        bad.lifetime = [-1.0, 1.0];
        assert_eq!(bad.validate(), Err("particle/lifetime-min-not-positive"));
        // Rate: negative, NaN, infinite.
        for rate in [-1.0, f32::NAN, f32::INFINITY] {
            let mut bad = base();
            bad.rate = rate;
            assert_eq!(bad.validate(), Err("particle/rate-negative-or-non-finite"));
        }
        // Gravity: negative, NaN, infinite.
        for gravity in [-9.81, f32::NAN, f32::INFINITY] {
            let mut bad = base();
            bad.gravity = gravity;
            assert_eq!(
                bad.validate(),
                Err("particle/gravity-negative-or-non-finite")
            );
        }
        // Drag: zero, above one, NaN, negative.
        for drag_factor in [0.0, 1.5, f32::NAN, -1.0] {
            let mut bad = base();
            bad.drag_factor = drag_factor;
            assert_eq!(
                bad.validate(),
                Err("particle/drag-factor-outside-unit-range")
            );
        }
        // Base velocity non-finite, per axis.
        for axis in 0..3 {
            let mut bad = base();
            bad.base_velocity[axis] = f32::INFINITY;
            assert_eq!(
                bad.validate(),
                Err("particle/base-velocity-non-finite"),
                "axis {axis}"
            );
        }
        // Jitter non-finite, per axis.
        for axis in 0..3 {
            let mut bad = base();
            bad.velocity_jitter[axis] = f32::NAN;
            assert_eq!(
                bad.validate(),
                Err("particle/velocity-jitter-non-finite"),
                "axis {axis}"
            );
        }
        // kill_y non-finite (NaN and infinite).
        let mut bad = base();
        bad.kill_y = Some(f32::NAN);
        assert_eq!(bad.validate(), Err("particle/kill-y-non-finite"));
        let mut bad = base();
        bad.kill_y = Some(f32::NEG_INFINITY);
        assert_eq!(bad.validate(), Err("particle/kill-y-non-finite"));
        // A refusal never constructs.
        let mut bad = base();
        bad.rate = -1.0;
        assert!(
            Emitter::new(bad).is_err(),
            "the refusal blocks construction"
        );
    }

    /// Reset replays identically: drive 300 ticks (folding every
    /// 60th), reset, drive 300 again in lockstep with a fresh
    /// never-reset control — identical reports, identical final
    /// prefix, identical digest against the pre-reset trajectory.
    #[test]
    fn reset_replays_identically() {
        let dt = 1.0f32 / 60.0f32;
        let mut emitter = Emitter::new(rain_like_campaign(0xC0FFEE)).unwrap();
        let mut first = Fingerprinter::new();
        for t in 1..=300u32 {
            let _report = emitter.tick(dt);
            if t % 60 == 0 {
                emitter.fold_state(&mut first);
            }
        }
        let mut control = Emitter::new(rain_like_campaign(0xC0FFEE)).unwrap();
        emitter.reset();
        assert_eq!(emitter.active_count(), 0, "reset empties the pool");
        let mut replay = Fingerprinter::new();
        for t in 1..=300u32 {
            assert_eq!(
                emitter.tick(dt),
                control.tick(dt),
                "tick {t}: replay == fresh"
            );
            if t % 60 == 0 {
                emitter.fold_state(&mut replay);
            }
        }
        assert_eq!(
            emitter.particles(),
            control.particles(),
            "the replayed prefix"
        );
        assert_eq!(
            replay.finish(),
            first.finish(),
            "the replayed digest matches the pre-reset trajectory"
        );
    }

    /// THE DIGEST PIN: the fixed campaign — seed 0xC0FFEE, the
    /// rain-like configuration, 600 ticks at dt = 1/60, the state
    /// folded at every 60th tick (tick 600's fold IS the final
    /// state's). The pinned u64 is the MEASURED actual of that
    /// campaign — cross-platform, forever; a move is an
    /// investigation, never a silent repin.
    #[test]
    fn digest_pin() {
        let dt = 1.0f32 / 60.0f32;
        let mut emitter = Emitter::new(rain_like_campaign(0xC0FFEE)).unwrap();
        let mut hasher = Fingerprinter::new();
        for t in 1..=600u32 {
            let report = emitter.tick(dt);
            assert_eq!(
                report.overflow, None,
                "tick {t}: the campaign never overflows"
            );
            if t % 60 == 0 {
                emitter.fold_state(&mut hasher);
            }
        }
        assert_eq!(
            hasher.finish(),
            PINNED_CAMPAIGN_DIGEST,
            "the campaign digest moved: a trajectory change — an \
             investigation, never a silent repin"
        );
    }

    /// The measured-actual campaign digest (the fixed campaign,
    /// folded at ticks 60, 120, ..., 600).
    const PINNED_CAMPAIGN_DIGEST: u64 = 16_840_467_954_493_462_700;

    /// THE REFUSED REFRESH PRESERVES STATE (the volume refresh
    /// contract: the listener-relative volume is validated before
    /// patching; a refused refresh returns the construction refusal
    /// AND leaves the state untouched — the digest before == after).
    #[test]
    fn refused_refresh_preserves_state() {
        let mut emitter = Emitter::new(rain_like_campaign(0xC0FFEE)).unwrap();
        // Drive 60 ticks to get a non-trivial state
        for _ in 0..60 {
            emitter.tick(1.0 / 60.0);
        }
        // Fold the state before
        let mut before = Fingerprinter::new();
        emitter.fold_state(&mut before);
        let before_digest = before.finish();
        // Attempt a refused refresh (inverted volume)
        let bad_volume = SpawnVolume {
            min: [20.0, 12.0, 20.0],
            max: [-20.0, 16.0, -20.0],
        };
        assert_eq!(
            emitter.set_volume(bad_volume),
            Err("particle/volume-min-above-max-x")
        );
        // Fold the state after — must be identical
        let mut after = Fingerprinter::new();
        emitter.fold_state(&mut after);
        let after_digest = after.finish();
        assert_eq!(
            before_digest, after_digest,
            "the refused refresh left the state untouched"
        );
    }

    /// THE NO-OP REFRESH PRESERVES THE STREAM (the volume refresh
    /// contract: a no-op refresh — same min/max — leaves the stream
    /// undisturbed; the next spawn draws from the SAME stream position
    /// as it would have without the call).
    #[test]
    fn no_op_refresh_preserves_stream_position() {
        let mut a = Emitter::new(rain_like_campaign(0xC0FFEE)).unwrap();
        let mut b = Emitter::new(rain_like_campaign(0xC0FFEE)).unwrap();
        // Drive both 60 ticks
        for _ in 0..60 {
            a.tick(1.0 / 60.0);
            b.tick(1.0 / 60.0);
        }
        // The no-op refresh: same volume
        let volume = rain_like_campaign(0xC0FFEE).volume;
        assert!(b.set_volume(volume).is_ok());
        // Drive one more tick — the spawns must match
        let ra = a.tick(1.0 / 60.0);
        let rb = b.tick(1.0 / 60.0);
        assert_eq!(ra, rb, "the no-op refresh preserved the stream");
        assert_eq!(a.particles(), b.particles(), "particle states match");
    }

    /// THE N-EMITTER COMPOSITION EQUIVALENCE GATE (the verifier
    /// seat's open strengthening item — the retired plural core's
    /// test 7, folded onto THIS API): four emitters (distinct seeds,
    /// volumes, rates, lifetimes) ticked 128 ticks TOGETHER — in
    /// fixed Vec order, the multi-emitter composition's shape — vs
    /// each ALONE from fresh construction: byte-identical alive
    /// prefixes, because every emitter owns its stream, accumulator,
    /// and pool. This gate exists to catch any future shared-state
    /// introduction (a shared RNG, a global spawn budget, or a
    /// static accumulator would break it); the mining source is
    /// era2/pc-wip-billboard-renderer's n_emitters_equivalence.
    #[test]
    fn n_emitters_compose_byte_identically_with_standalone() {
        let configs: Vec<EmitterConfig> = (0..4usize)
            .map(|i| {
                let mut config = rain_like_campaign(0x100 + i as u64);
                config.volume = SpawnVolume {
                    min: [-20.0 + 10.0 * i as f32, 10.0 + i as f32, -4.0],
                    max: [-12.0 + 10.0 * i as f32, 14.0 + i as f32, 4.0],
                };
                config.rate = [1.0, 2.5, 0.75, 3.7][i];
                config
            })
            .collect();
        let dt = 1.0 / 60.0;

        // TOGETHER: the fixed-order composition (Vec order is part
        // of the contract — the future multi-emitter demo's shape).
        let mut together: Vec<Emitter> =
            configs.iter().map(|c| Emitter::new(*c).unwrap()).collect();
        for _ in 0..128 {
            for emitter in &mut together {
                let report = emitter.tick(dt);
                assert!(
                    report.overflow.is_none(),
                    "the fixture's rates never saturate the pool"
                );
            }
        }

        // EACH ALONE: fresh construction, same campaign length.
        for (i, config) in configs.iter().enumerate() {
            let mut alone = Emitter::new(*config).unwrap();
            for _ in 0..128 {
                alone.tick(dt);
            }
            let composed = &together[i];
            assert_eq!(
                composed.active_count(),
                alone.active_count(),
                "emitter {i}: composed alive count == standalone alive count"
            );
            for (pa, pb) in composed.particles().iter().zip(alone.particles().iter()) {
                assert_eq!(
                    (pa.position, pa.velocity, pa.life),
                    (pb.position, pb.velocity, pb.life),
                    "emitter {i}: every alive particle byte-identical composed vs \
                     standalone (own stream, own accumulator, own pool)"
                );
            }
        }
    }
}
