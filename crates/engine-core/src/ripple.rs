//! The deterministic ripple model (Era 2 package C, Stage 1 — rain ripples
//! on wet ground).
//!
//! THE MODEL (from the design seed): the rain's kill-plane deaths ARE the
//! impact events. Every particle dying at y <= 0 is a raindrop hitting the
//! ground: the ripple set derives from the Emitter's death positions + the
//! demo clock (the rain campaign's digest ALREADY pins the death stream —
//! the ripple field adds zero entropy).
//!
//! ONE RIPPLE:
//!   center: [x, z] from the death's position
//!   birth_tick: the death's tick
//!   a0: the perturbation amplitude (RIPPLE_A0)
//!
//! THE RING PROFILE (libm-free, the spec's decision):
//!   profile = a0 * fade * (1 - |dist - radius| / w)
//!   with:
//!     - age = (tick - birth) as f32
//!     - radius = age * RIPPLE_RADIUS_PER_TICK
//!     - fade = 1.0 - age / RIPPLE_LIFETIME_TICKS as f32 (clamped at 0)
//!     - dist = sqrt((ground_x - center_x)^2 + (ground_z - center_z)^2)
//!     - w = RIPPLE_WIDTH
//!
//! THE LIBM-FREE FORM (derived algebra in comments):
//!   The gradient contribution = ±(a0 * fade / w) * (dx, dz) / dist
//!   where sign = sign(dist - radius).
//!
//!   To avoid sqrt, we work in SQUARED-DISTANCE DOMAIN:
//!     dist_sq = dx^2 + dz^2
//!     radius_sq = radius * radius
//!     w_sq = 2 * radius * w + w * w  [exact algebra: (radius+w)^2 - radius^2]
//!
//!   profile_slope = a0 * fade * (radius_sq - dist_sq) / (w * w_sq)
//!   perturbation = profile_slope * (dx, dz)  [unnormalized, magnitude carried by profile]
//!
//!   This form is PURE multiplies/adds — no libm calls.
//!
//!   ALGEBRAIC DERIVATION (pin this, not Rust code; doc-test will fail if uncommented):
//!   ```
//!   // Original: profile = a0 * fade * (1 - |dist - radius| / w)
//!   // Gradient: grad = ±(a0 * fade / w) * (dx, dz) / dist, sign from (dist - radius).
//!
//!   // In squared distance domain:
//!   //   dist_sq = dx^2 + dz^2, radius_sq = radius^2
//!   //   w_sq = 2 * radius * w + w * w = (radius+w)^2 - radius^2
//!
//!   // profile_slope = a0 * fade * (radius_sq - dist_sq) / (w * w_sq)
//!   // perturbation = profile_slope * (dx, dz)
//!
//!   // This form is pure multiplies/adds — no libm calls.
//!
//!   // The ring's effective width depends on radius:
//!   //   band in dist_sq: [radius_sq, (radius+w)^2]
//!   //   width in dist_sq: (radius+w)^2 - radius_sq = 2*radius*w + w^2
//!
//!   // At large radius, the band in dist_sq is wider, but in actual
//!   // distance it's still w. The visual difference is negligible at
//!   // the authored scales.
//!   ```
//!
//! THE IMPLEMENTATION (the spec's decision, documented above):
//!   perturbation = (a0 * fade / w_sq) * (radius_sq - dist_sq) * (dx, dz)
//!   where:
//!     - w_sq = 2 * radius * w + w * w
//!     - radius_sq = radius * radius
//!     - dx = ground[0] - center[0]
//!     - dz = ground[1] - center[1]
//!
//!   The sign (radius_sq - dist_sq) gives the correct flip across the ring.

use crate::fingerprint::Fingerprinter;
use crate::particle::Particle;

/// One raindrop's impact on the wet ground (the kill-plane death
/// observed at the demo clock): the ring's center and birth.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ripple {
    /// The center of the ripple (x, z coordinates from death position).
    pub center: [f32; 2],
    /// The tick when this ripple was born (the death tick).
    pub birth: u64,
    /// The perturbation amplitude (constant RIPPLE_A0).
    pub a0: f32,
}

/// The overflow error (typed refusal, the audio_triggers pattern).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RippleOverflow {
    /// Ripples dropped above [`RIPPLE_CAPACITY`] this observation.
    pub dropped: u32,
}

/// The validation error for non-finite death positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RippleNonFinite;

impl std::fmt::Display for RippleNonFinite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ripple/non-finite-death-position")
    }
}

impl std::error::Error for RippleNonFinite {}

/// The error type for observe_deaths (typed refusal discipline).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RippleError {
    /// The live set is full (capacity exceeded).
    Overflow(RippleOverflow),
    /// A death position is non-finite.
    NonFinite(RippleNonFinite),
}

/// The authored constants (policy at the engine boundary — the
/// demo composes; document each derivation).
///
/// RIPPLE_RADIUS_PER_TICK: the ring's expansion rate in meters per tick.
/// At 60 Hz, 0.8 m/s = 0.8 / 60 m/tick.
pub const RIPPLE_RADIUS_PER_TICK: f32 = 0.8 / 60.0;

/// RIPPLE_LIFETIME_TICKS: how many ticks a ripple lives.
/// 2.5 seconds at 60 Hz = 150 ticks.
pub const RIPPLE_LIFETIME_TICKS: u32 = 150;

/// RIPPLE_A0: the perturbation amplitude (meters of surface displacement).
pub const RIPPLE_A0: f32 = 0.02;

/// RIPPLE_WIDTH: the ring's width in meters.
/// The ring profile is a triangle bump of width w centered at radius.
pub const RIPPLE_WIDTH: f32 = 0.12;

/// RIPPLE_CAPACITY: the live-set bound (fixed array, the belt
/// above the arithmetic — never a silent drop if a future policy
/// raises the intake). THE ARITHMETIC (the design seed's units
/// error, corrected here): the deaths at the demo's rain policy
/// are 480/s = 8 PER TICK (the seed's "8/s" conflated the units);
/// a ripple-per-death field would hold 8 × 150 = 1200 live. The
/// model therefore authors an INTAKE LOD instead: at most
/// [`RIPPLE_INTAKE_PER_TICK`] new ripples per tick (the first
/// deaths in slot order — the deterministic sample), so the live
/// set holds at most lifetime × intake = 150 + the in-flight tick
/// ≈ 151; 160 is the belt.
pub const RIPPLE_CAPACITY: usize = 160;

/// The intake LOD: at most this many new ripples per tick (the
/// presentation-density policy — the wet surface shows at most one
/// new ring per frame at the demo's clock; the remaining deaths
/// are the LOD's DOCUMENTED SKIP, a visual policy, not a pool
/// overflow — the conservation of DEATHS lives in the rain
/// campaign's own census).
pub const RIPPLE_INTAKE_PER_TICK: usize = 1;

/// RippleField: the bounded dense live set (a fixed array + len).
/// The zero-allocation discipline: construction-time allocation only.
/// The eviction is oldest-first, the overflow is typed (RippleOverflow).
#[derive(Debug, Clone)]
pub struct RippleField {
    /// The fixed array of ripples.
    ripples: [Ripple; RIPPLE_CAPACITY],
    /// The length of the live prefix.
    len: usize,
}

impl RippleField {
    /// Constructs a new empty ripple field.
    pub fn new() -> Self {
        Self {
            ripples: [Ripple {
                center: [0.0; 2],
                birth: 0,
                a0: 0.0,
            }; RIPPLE_CAPACITY],
            len: 0,
        }
    }

    /// Appends one Ripple per death (center = position[0..2], a0 = RIPPLE_A0).
    ///
    /// The method:
    /// 1. First evicts expired ripples (age >= LIFETIME — oldest-first).
    /// 2. Then adds new ripples from the deaths list.
    /// 3. Returns RippleError::Overflow if the set would exceed capacity.
    /// 4. Returns RippleError::NonFinite if any death position is non-finite.
    ///
    /// The field's state is preserved on any error (atomic refusal).
    pub fn observe_deaths(&mut self, deaths: &[Particle], tick: u64) -> Result<(), RippleError> {
        // First, evict expired ripples (oldest-first, dense prefix order).
        self.evict_expired(tick);

        // The non-finite gate (atomic refusal discipline: checked
        // over the INTAKE before any mutation).
        let intake: &[Particle] = if deaths.len() > RIPPLE_INTAKE_PER_TICK {
            &deaths[..RIPPLE_INTAKE_PER_TICK]
        } else {
            deaths
        };
        for death in intake {
            if !death.position[0].is_finite() || !death.position[2].is_finite() {
                return Err(RippleError::NonFinite(RippleNonFinite));
            }
        }

        // The intake (the LOD's first-in-slot-order sample).
        let mut dropped: u32 = 0;
        for death in intake {
            if self.len < RIPPLE_CAPACITY {
                self.ripples[self.len] = Ripple {
                    center: [death.position[0], death.position[2]],
                    birth: tick,
                    a0: RIPPLE_A0,
                };
                self.len += 1;
            } else {
                dropped += 1;
            }
        }

        if dropped > 0 {
            return Err(RippleError::Overflow(RippleOverflow { dropped }));
        }

        Ok(())
    }

    /// Evicts expired ripples (oldest-first, by birth tick).
    /// A ripple is expired if age >= RIPPLE_LIFETIME_TICKS.
    /// Also handles the case where tick < birth (age would underflow),
    /// treating it as expired since the ripple can't have been born yet.
    fn evict_expired(&mut self, tick: u64) {
        let lifetime_ticks = RIPPLE_LIFETIME_TICKS as u64;
        let mut write = 0usize;

        for read in 0..self.len {
            let ripple = self.ripples[read];
            // Handle potential underflow: if tick < birth, the ripple hasn't been born yet.
            // We can treat it as "not yet active" and keep it (or consider it expired).
            // The spec says expired = age >= LIFETIME, so age < LIFETIME means keep.
            // If tick < birth, the age calculation would underflow, so we need special handling.
            // Since birth is always <= current observation tick in normal operation,
            // this branch shouldn't be hit. But for safety, we use saturating_sub.
            let age = tick.saturating_sub(ripple.birth);
            if age < lifetime_ticks {
                // Keep this ripple.
                self.ripples[write] = ripple;
                write += 1;
            }
            // Else: expired, skip (drop).
        }

        self.len = write;
    }

    /// THE FIELD EVALUATION: perturbs a ground point based on all ripples.
    ///
    /// Iterate the dense prefix in slot order; per ripple:
    ///   age = (tick - birth) as f32
    ///   radius = age * RIPPLE_RADIUS_PER_TICK
    ///   fade = 1.0 - age / RIPPLE_LIFETIME_TICKS as f32 (clamped at 0)
    ///   dx = ground[0] - center[0]
    ///   dz = ground[1] - center[1]
    ///   dist_sq = dx*dx + dz*dz
    ///
    /// The perturbation contribution from this ripple:
    ///   radius_sq = radius * radius
    ///   w_sq = w_sq_for_radius(radius, RIPPLE_WIDTH)
    ///   diff = radius_sq - dist_sq  // positive inside the band, negative outside
    ///   slope = RIPPLE_A0 * fade * diff / (RIPPLE_WIDTH * w_sq)
    ///   contribution = slope * [dx, dz]
    ///
    /// The spec's squared-distance-domain form (from the design seed):
    ///   profile_slope = a0 * fade * (radius_sq - dist_sq) / (w * w_sq)
    ///   perturbation = profile_slope * (dx, dz)
    ///
    /// Note: the sign (radius_sq - dist_sq) is opposite of (dist - radius),
    /// so the gradient points inward when outside and outward when inside.
    /// THE FIELD EVALUATION: perturbs a ground point based on all
    /// live ripples. THE PINNED ORDER: iterate the dense prefix in
    /// slot order; per ripple:
    ///   age = (tick − birth) saturating (the same guard the
    ///         eviction carries — a future birth is unreachable
    ///         through the observe discipline, the belt remains)
    ///   radius = age · RIPPLE_RADIUS_PER_TICK
    ///   fade = max(0, 1 − age / RIPPLE_LIFETIME_TICKS)
    ///   dx, dz = ground − center;  dist_sq = dx² + dz²
    ///   THE BAND GATE (the repair's crux — the first draft's ramp
    ///   had NO gate, so every ripple perturbed every ground point
    ///   with a magnitude GROWING with distance): the contribution
    ///   is ZERO unless dist_sq lies in [inner_sq, outer_sq] with
    ///   inner = max(0, radius − w) and outer = radius + w (both
    ///   squared — pure multiplies). A young ring (radius < w)
    ///   reaches the center: inner_sq = 0.
    ///   Inside the band: slope = a0 · fade / w_sq (w_sq =
    ///   2·radius·w + w² — the exact (radius+w)² − radius² band
    ///   width in squared space, the metric band's image under the
    ///   ≈1/(2·radius) change of variables — the derivation in the
    ///   module header); the contribution = slope · (radius_sq −
    ///   dist_sq) · (dx, dz) — positive inside the ring's peak,
    ///   negative outside it, ZERO at the band edges by the gate.
    ///   PURE multiplies/adds — no sqrt, no libm, fixed order.
    pub fn perturb(&self, ground: [f32; 2], tick: u64) -> [f32; 2] {
        let mut perturbation = [0.0; 2];
        let w = RIPPLE_WIDTH;

        for ripple in &self.ripples[..self.len] {
            let age = tick.saturating_sub(ripple.birth) as f32;
            let radius = age * RIPPLE_RADIUS_PER_TICK;
            let fade = (1.0 - age / RIPPLE_LIFETIME_TICKS as f32).max(0.0);

            let dx = ground[0] - ripple.center[0];
            let dz = ground[1] - ripple.center[1];
            let dist_sq = dx * dx + dz * dz;

            let radius_sq = radius * radius;
            // THE BAND GATE: [max(0, radius−w), radius+w] squared.
            let inner = (radius - w).max(0.0);
            let inner_sq = inner * inner;
            let outer = radius + w;
            let outer_sq = outer * outer;
            if dist_sq < inner_sq || dist_sq > outer_sq {
                continue;
            }

            // The squared-space band width (never zero: w > 0).
            let w_sq = 2.0 * radius * w + w * w;
            if w_sq == 0.0 {
                continue;
            }

            // THE CONTRIBUTION — the squared-space triangle's
            // GRADIENT (the repair's second correction: the first
            // draft multiplied the slope by (radius_sq − dist_sq),
            // making a PARABOLA at full amplitude at the band edges
            // — only the gate zeroed it, a visible discontinuity;
            // the triangle P(s) = a0·fade·(1 − |s − radius_sq|/w_sq)
            // has gradient ∓a0·fade/w_sq along the radial direction,
            // TAPERING by fade and bounded everywhere):
            //   sign = +1 inside the ring's peak, −1 outside it
            //   contribution = sign · (a0·fade/w_sq) · (dx, dz)
            // — magnitude ≤ a0·fade·outer/w_sq (≈ a0/(2w) at the
            // ring: ~8 cm of reflection offset, the visible ripple).
            let slope = ripple.a0 * fade / w_sq;
            let sign = if dist_sq < radius_sq { 1.0 } else { -1.0 };
            perturbation[0] += sign * slope * dx;
            perturbation[1] += sign * slope * dz;
        }

        perturbation
    }

    /// Folds every live ripple's state into the FNV-1a builder.
    /// Format: center.x.bits, center.z.bits, birth, a0.bits per ripple, dense order.
    pub fn fold_state(&self, hash: &mut Fingerprinter) {
        for ripple in &self.ripples[..self.len] {
            hash.write_u32(ripple.center[0].to_bits());
            hash.write_u32(ripple.center[1].to_bits());
            hash.write_u64(ripple.birth);
            hash.write_u32(ripple.a0.to_bits());
        }
    }

    /// The number of live ripples.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the field is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Default for RippleField {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ring profile's known answers through THE REAL `perturb`
    /// (the first draft re-implemented the formula inside the test
    /// and asserted its own arithmetic — circular; this one calls
    /// the field). One ripple at the origin, born tick 0; evaluated
    /// at tick 60: age 60, radius = 60·(0.8/60) = 0.8 m, fade =
    /// 1 − 60/150 = 0.6, w = 0.12. THE GATED FORM'S EXPECTATIONS:
    /// - At the peak (ground = [0.8, 0]): diff = 0 → [0, 0].
    /// - INSIDE the band (ground = [0.75, 0]): dist_sq = 0.5625 <
    ///   radius_sq = 0.64 → diff > 0 → the x contribution positive.
    /// - OUTSIDE the peak but inside the band (ground = [0.85, 0]):
    ///   dist_sq = 0.7225 > 0.64 → diff < 0 → negative.
    /// - AT the outer band edge (ground = [0.92, 0]): dist = 0.92 =
    ///   radius + w → THE GATE → exactly [0, 0].
    /// - FAR outside (ground = [5.0, 0]): the gate → [0, 0].
    /// - AT the center (ground = [0, 0]): dist 0 < inner = 0.68 →
    ///   the gate → [0, 0] (the first draft's ungated ramp gave the
    ///   center the LARGEST perturbation — the divergence the
    ///   repair's gate closes).
    #[test]
    fn the_ring_profile_is_the_pinned_squared_space_form() {
        let mut field = RippleField::new();
        field.ripples[0] = Ripple {
            center: [0.0, 0.0],
            birth: 0,
            a0: RIPPLE_A0,
        };
        field.len = 1;
        let tick = 60u64;

        // THE PEAK IS THE SIGN BOUNDARY, not a zero: the triangle's
        // gradient is MAXIMUM at the apex (the first draft expected
        // zero there — the profile/gradient confusion). The sign at
        // the exact peak is ULP-arbitrary (the two radius
        // expressions round differently), so the pin is the FLIP:
        // full magnitude, opposite signs, either side.
        let just_inside = field.perturb([0.79, 0.0], tick);
        let just_outside = field.perturb([0.81, 0.0], tick);
        assert!(
            just_inside[0] > 0.0 && just_outside[0] < 0.0,
            "the sign flips across the peak (got {just_inside:?} / {just_outside:?})"
        );

        let inner_wall = field.perturb([0.75, 0.0], tick);
        assert!(
            inner_wall[0] > 0.0,
            "inside the ring: positive x (got {inner_wall:?})"
        );

        let outer_wall = field.perturb([0.85, 0.0], tick);
        assert!(
            outer_wall[0] < 0.0,
            "outside the peak, inside the band: negative x (got {outer_wall:?})"
        );

        // The outer band edge in exact arithmetic is dist = 0.92 —
        // not exactly representable in f32, and the boundary
        // compares two differently-rounded expressions (the ULP
        // lesson): pin the zero at a SAFELY-outside point.
        let outside = field.perturb([0.95, 0.0], tick);
        assert_eq!(outside, [0.0, 0.0], "outside the band: the gate zeroes it");

        let far = field.perturb([5.0, 0.0], tick);
        assert_eq!(far, [0.0, 0.0], "far outside: the gate zeroes it");

        let center = field.perturb([0.0, 0.0], tick);
        assert_eq!(
            center,
            [0.0, 0.0],
            "the center (inside the inner edge): the gate zeroes it"
        );

        // The magnitude's hand computation at the banded point
        // (ground = [0.75, 0]): dx = 0.75, dist_sq = 0.5625,
        // radius_sq = 0.64, w_sq = 2·0.8·0.12 + 0.12² = 0.2064,
        // slope = 0.02·0.6/0.2064, diff = 0.0775 →
        // x = slope·0.75 — written in the SAME fixed order the
        // field evaluates (the radius from the code's own
        // expression — its f32 rounding is part of the contract).
        let radius = 60.0 * RIPPLE_RADIUS_PER_TICK;
        let w_sq = 2.0 * radius * RIPPLE_WIDTH + RIPPLE_WIDTH * RIPPLE_WIDTH;
        let expected_x = RIPPLE_A0 * 0.6 / w_sq * 0.75;
        assert!(
            (inner_wall[0] - expected_x).abs() < 1e-9,
            "the banded magnitude matches the hand arithmetic (got {}, expected {})",
            inner_wall[0],
            expected_x
        );
    }

    /// The perturbation is zero on the empty field (the wet-only identity).
    #[test]
    fn the_perturbation_is_zero_on_the_empty_field() {
        let field = RippleField::new();
        let ground = [1.0, 2.0];
        let tick = 100;

        let perturbation = field.perturb(ground, tick);
        assert_eq!(perturbation, [0.0, 0.0], "empty field perturbation");
    }

    /// The sign flips across the ring (inside vs outside the band).
    #[test]
    fn the_sign_flips_across_the_ring() {
        let mut field = RippleField::new();

        // Add one ripple at origin, birth at tick 0.
        // At tick 60, age = 1.0, radius = 0.8 m.
        field.ripples[0] = Ripple {
            center: [0.0, 0.0],
            birth: 0,
            a0: RIPPLE_A0,
        };
        field.len = 1;

        let tick = 60;
        let radius = 60.0 * RIPPLE_RADIUS_PER_TICK; // = 0.8
        let _radius_sq = radius * radius; // = 0.64 (used in perturb, but not directly here)

        // Inside the band: dist = 0.5 < radius
        // Inside the band, inside the peak (dist = 0.75, the band
        // is 0.68–0.92): the inner wall — positive x. (The first
        // draft probed 0.5 — INSIDE THE INNER EDGE, gated to zero.)
        let inside_pert = field.perturb([0.75, 0.0], tick);
        assert!(
            inside_pert[0] > 0.0,
            "inside the ring's peak: positive x (got {inside_pert:?})"
        );

        // PAST the peak but INSIDE the band (dist = 0.85, the band
        // is 0.68..0.92): diff < 0 → negative x.
        let past_peak = field.perturb([0.85, 0.0], tick);
        assert!(
            past_peak[0] < 0.0,
            "past the peak, inside the band: negative x (got {past_peak:?})"
        );

        // OUTSIDE the band (dist = 1.0 > 0.92): THE GATE → exactly
        // zero (the first draft asserted a negative sign here — the
        // ungated ramp's divergent tail, caught in the repair).
        let outside_pert = field.perturb([1.0, 0.0], tick);
        assert_eq!(
            outside_pert,
            [0.0, 0.0],
            "outside the band: the gate zeroes it"
        );
    }

    /// The deaths drive the set and the eviction is oldest-first.
    #[test]
    fn the_deaths_drive_the_set_and_the_eviction_is_oldest_first() {
        let mut field = RippleField::new();

        // The eviction discipline, driven directly over the dense
        // set (the observe path's eviction is the same function;
        // the overflow test covers the capacity boundary):
        // 32 ripples at ascending births.
        for i in 0..32 {
            field.ripples[i] = Ripple {
                center: [i as f32, 0.0],
                birth: i as u64,
                a0: RIPPLE_A0,
            };
            field.len += 1;
        }

        assert_eq!(field.len(), 32, "field should be full");

        // Evict at tick 150: birth 0 is expired (age = 150 >= 150).
        field.evict_expired(150);
        assert_eq!(field.len(), 31, "one ripple expired");

        // The surviving ripple with oldest birth should be at index 0.
        assert_eq!(field.ripples[0].birth, 1, "oldest surviving ripple");

        // Evict at tick 200: all remaining ripples (births 1-31) have age >= 150.
        // All 31 expire, leaving 0 ripples.
        field.evict_expired(200);
        assert_eq!(field.len(), 0, "all ripples expired");

        // Add exactly 32 ripples to fill the field.
        for i in 0..32 {
            field.ripples[i] = Ripple {
                center: [i as f32, 0.0],
                birth: 1000u64 + i as u64,
                a0: RIPPLE_A0,
            };
            field.len += 1;
        }

        assert_eq!(field.len(), 32, "field should be full");

        // Evict at tick 1200: all ripples (births 1000-1031) have age >= 150.
        // All 32 expire, leaving 0 ripples.
        field.evict_expired(1200);
        assert_eq!(field.len(), 0, "all ripples expired");

        // Evict at tick 1199: ripples with birth <= 1049 are expired.
        // Births 1000-1031 are all <= 1049, so all expire.
        field.evict_expired(1199);
        assert_eq!(field.len(), 0, "all ripples expired at tick 1199");

        // Add 20 ripples with various ages.
        for i in 0..20 {
            field.ripples[i] = Ripple {
                center: [i as f32, 0.0],
                birth: 1000u64 + (i as u64) * 10, // births: 1000, 1010, 1020, ..., 1090
                a0: RIPPLE_A0,
            };
            field.len += 1;
        }

        assert_eq!(field.len(), 20, "field should have 20 ripples");

        // Evict at tick 1160: ripples with birth <= 1010 have age >= 150.
        // Births 1000 and 1010 are expired (ages 160 and 150).
        field.evict_expired(1160);
        assert_eq!(field.len(), 18, "2 ripples expired");

        // The first ripple should be birth 1020.
        assert_eq!(field.ripples[0].birth, 1020, "oldest surviving ripple");
    }

    /// The overflow test: burst that exceeds 32, pin the typed refusal.
    ///
    /// The demo's rate: ~8/s × lifetime 2.5s ≈ 20 ripples.
    /// 32 > 20, so overflow should not fire on the demo's normal rate.
    ///
    /// But a burst of 35 ripples in one tick should exceed capacity.
    #[test]
    fn the_ripple_overflow_test() {
        // THE INTAKE LOD FIRST: a 35-death burst in one tick admits
        // exactly ONE ripple (the first in slot order) — no
        // overflow, the documented visual policy.
        let mut field = RippleField::new();
        let deaths: Vec<Particle> = (0..35)
            .map(|i| Particle {
                position: [i as f32, 1.0, 0.0],
                velocity: [0.0, 0.0, 0.0],
                life: 0.0,
            })
            .collect();
        field
            .observe_deaths(&deaths, 0)
            .expect("the intake admits one");
        assert_eq!(field.len(), 1, "the intake LOD: one ripple per tick");
        assert_eq!(
            field.ripples[0].center,
            [0.0, 0.0],
            "the FIRST death in slot order (the center is x,z)"
        );

        // THE CAPACITY BELT: structurally unreachable through the
        // public flow (the intake × lifetime arithmetic holds the
        // live set ≤ 151 < 160), so the belt is tested by direct
        // stuffing — the array filled to capacity, one more intake
        // refused TYPED (never a silent drop; a future policy
        // raising the intake lands here).
        let mut full = RippleField::new();
        for i in 0..RIPPLE_CAPACITY {
            full.ripples[i] = Ripple {
                center: [i as f32, 0.0],
                birth: u64::MAX, // never expires within the test's tick
                a0: RIPPLE_A0,
            };
        }
        full.len = RIPPLE_CAPACITY;
        let one = [Particle {
            position: [99.0, 1.0, 0.0],
            velocity: [0.0, 0.0, 0.0],
            life: 0.0,
        }];
        assert!(
            matches!(full.observe_deaths(&one, 0), Err(RippleError::Overflow(o)) if o.dropped == 1),
            "the capacity belt refuses typed"
        );
        assert_eq!(
            full.len(),
            RIPPLE_CAPACITY,
            "the field is untouched by the refusal"
        );
    }

    /// The non-finite death position refuses typed.
    #[test]
    fn the_non_finite_death_refuses_typed() {
        let mut field = RippleField::new();

        // Add a valid ripple first.
        field.ripples[0] = Ripple {
            center: [1.0, 2.0],
            birth: 100,
            a0: RIPPLE_A0,
        };
        field.len = 1;

        // The digest before.
        let mut hash_before = Fingerprinter::new();
        field.fold_state(&mut hash_before);
        let digest_before = hash_before.finish();

        // A NaN position should not modify the field.
        let death_nan = Particle {
            position: [f32::NAN, 1.0, 0.0],
            velocity: [0.0, 0.0, 0.0],
            life: 0.0,
        };

        let result = field.observe_deaths(&[death_nan], 0);
        assert!(matches!(result, Err(RippleError::NonFinite(_))));

        // After: the field should be unchanged.
        let mut hash_after = Fingerprinter::new();
        field.fold_state(&mut hash_after);
        let digest_after = hash_after.finish();

        assert_eq!(
            digest_before, digest_after,
            "the field state is untouched after NaN death"
        );
    }

    /// The ripple campaign digest pin.
    ///
    /// A fixed campaign: Emitter with kill plane at y=0, the rain-like policy,
    /// deaths observed per tick into a RippleField, 600 ticks, fold at stride 60.
    ///
    /// The seed is 0xC0FFEE (the same as particle.rs's digest pin).
    #[test]
    fn the_ripple_campaign_digest_pin() {
        use crate::particle::{Emitter, EmitterConfig, SpawnVolume};

        // The rain-like campaign configuration.
        let config = EmitterConfig {
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
            seed: 0xC0FFEE,
        };

        let mut emitter = Emitter::new(config).expect("config validates");
        let mut field = RippleField::new();
        let mut hasher = Fingerprinter::new();

        let dt = 1.0 / 60.0;

        for t in 1..=600u32 {
            // Collect deaths using the new method.
            let mut deaths = Vec::new();
            let _report = emitter.tick_collecting_deaths(dt, &mut deaths);

            // Observe the deaths — the Result OBSERVED, never
            // swallowed (the campaign's deaths are finite by
            // construction; a refusal here is a bug, not a skip).
            field
                .observe_deaths(&deaths, t as u64)
                .expect("the rain campaign's deaths are finite and under capacity");

            // Fold at stride 60.
            if t % 60 == 0 {
                field.fold_state(&mut hasher);
            }
        }

        // The digest we're measuring.
        let digest = hasher.finish();

        // The measured digest for the fixed campaign (seed 0xC0FFEE, rain-like config,
        // 600 ticks at dt=1/60, folded at stride 60, deaths observed via tick_collecting_deaths
        // under the intake LOD — MEASURED after the intake/capacity correction; the delegate's
        // original pin measured the unreachable-capacity regime and was retired with it).
        const PINNED_RIPPLE_DIGEST: u64 = 558258865535961831;

        assert_eq!(
            digest, PINNED_RIPPLE_DIGEST,
            "ripple campaign digest moved: a ripple model change — an investigation"
        );
    }

    /// The death collection preserves the tick contract.
    ///
    /// Same campaign through plain `tick` vs `tick_collecting_deaths`:
    /// - identical TickReports
    /// - identical fold_state at every sampled tick
    /// - collected stream's own double-run identity
    #[test]
    fn the_death_collection_preserves_the_tick_contract() {
        use crate::fingerprint::Fingerprinter;
        use crate::particle::{Emitter, EmitterConfig, SpawnVolume};

        // The rain-like campaign configuration.
        let config = EmitterConfig {
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
            seed: 0xC0FFEE,
        };

        let dt = 1.0 / 60.0;

        // Run two emitters with the same config.
        let mut a = Emitter::new(config).expect("config validates");
        let mut b = Emitter::new(config).expect("config validates");

        // One uses tick(), one uses tick_collecting_deaths().
        for t in 1..=600u32 {
            let report_a = a.tick(dt);
            let mut deaths_b = Vec::new();
            let report_b = b.tick_collecting_deaths(dt, &mut deaths_b);

            assert_eq!(report_a, report_b, "tick {} reports match", t);

            // The pool state should be identical.
            assert_eq!(a.particles(), b.particles(), "tick {} particles match", t);

            // The folded digests should be identical.
            let mut ha = Fingerprinter::new();
            a.fold_state(&mut ha);
            let mut hb = Fingerprinter::new();
            b.fold_state(&mut hb);
            assert_eq!(ha.finish(), hb.finish(), "tick {} digests match", t);

            if t % 60 == 0 {
                // Sampled tick assertion.
                let _ = report_a; // silence unused warning
            }
        }

        // The collected deaths stream should be byte-identical on double-run.
        let mut deaths1 = Vec::new();
        let mut deaths2 = Vec::new();

        let mut c = Emitter::new(config).expect("config validates");
        let mut d = Emitter::new(config).expect("config validates");

        for _ in 1..=600u32 {
            let _ = c.tick_collecting_deaths(dt, &mut deaths1);
            let _ = d.tick_collecting_deaths(dt, &mut deaths2);
        }

        assert_eq!(deaths1.len(), deaths2.len(), "death counts match");
        for (i, (da, db)) in deaths1.iter().zip(deaths2.iter()).enumerate() {
            assert_eq!(da.position, db.position, "death {} position match", i);
            assert_eq!(da.velocity, db.velocity, "death {} velocity match", i);
            assert_eq!(da.life, db.life, "death {} life match", i);
        }
    }
}
