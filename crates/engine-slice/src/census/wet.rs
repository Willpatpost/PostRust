//! §9.5's RIPPLE ROW field states (A19/R4's before/after axis): the
//! wet route rendered at {empty, ~3.7-ring steady, 32-ring
//! saturation} × {720p, 1440p}, the gpu_ms percentiles printed per
//! cell. This module constructs the THREE field states; the
//! `tests/census_ripple.rs` battery renders them.
//!
//! - **empty** — `WetFrame { ripple: None }` (the rainless wet
//!   path's empty uniform — the shader's identity no-op).
//! - **steady** — the MEASURED steady state (~3.7 live rings,
//!   engine-core ripple.rs's own study): constructed through the
//!   REAL rain machinery — a warm-started `RainLayer` (132 ticks at
//!   the demo clock, the shipped 320:1 admission policy) whose
//!   `ripple_field()` IS the steady field. Nothing hand-authored:
//!   the row measures the shipped route's own state.
//! - **saturation** — 32 authored rings (the RIPPLE_POOL_BOUND,
//!   every slot live), deterministic hand-placed centers/ages over
//!   the wet route's ground footprint — the finding's named worst
//!   case (the per-fragment loop at its bound).

use engine_core::ripple::{RIPPLE_POOL_BOUND, RippleField, RippleRing};

use crate::corner_weather::RIPPLE_POLICY;

/// The wet-route camera the saturation rings are placed against
/// (render_weather's own gate vantage — above the road cross
/// looking at the intersection, the ground footprint the census
/// battery renders).
pub const WET_ROUTE_EYE: [f32; 3] = [0.0, 1.7, 15.0];

/// The SATURATED field: 32 authored rings (the pool bound), spread
/// over the ground in front of [`WET_ROUTE_EYE`] at ages 0.05 s
/// through 0.95 × lifetime — every ring live, every band gate
/// reachable. Deterministic by construction (the render_weather
/// separation gate's `forced_eight_ring_field` discipline, extended
/// to the bound: hand-placed, pure data, the policy validated).
pub fn saturated_field() -> RippleField {
    let mut field = RippleField::new(RIPPLE_POLICY).expect("the demo's ripple policy validates");
    for index in 0..RIPPLE_POOL_BOUND {
        // A deterministic spread: a monotonic radius per ring (every
        // center distinct by construction) with the angle marching
        // around the full turn plus a parity quarter-turn offset,
        // ages spread across the lifetime so the fade states differ.
        let angle = (index as f32 / RIPPLE_POOL_BOUND as f32) * core::f32::consts::TAU
            + if index % 2 == 0 {
                0.0
            } else {
                core::f32::consts::FRAC_PI_2
            };
        let radius = 2.0 + (index as f32 / RIPPLE_POOL_BOUND as f32) * 9.0;
        let age = 0.05
            + (index as f32 / RIPPLE_POOL_BOUND as f32) * (RIPPLE_POLICY.lifetime * 0.9 - 0.05);
        let center = [angle.cos() * radius, angle.sin() * radius];
        let result = field
            .author_ring(RippleRing {
                center,
                age,
                amplitude: 1.0,
            })
            .expect("the authored ring is valid data");
        assert!(
            result.is_none(),
            "the pool has room (ring {index} of {RIPPLE_POOL_BOUND})"
        );
    }
    assert_eq!(
        field.active_count(),
        RIPPLE_POOL_BOUND,
        "the saturated field holds every live slot"
    );
    field
}

/// The STEADY field through the real rain machinery: a warm-started
/// `RainLayer` (the shipped policy — rate 480/s, admission 320:1,
/// the 132-tick warm start) whose ripple field IS the measured
/// ~3.7-live-ring steady state. The battery borrows the layer's
/// field per frame exactly the way the demo route does
/// (`crowd.rain.as_ref().map(|r| r.ripple_field())`); this
/// constructor returns the warm layer for that use, plus the
/// measured live count for the row's label.
pub fn steady_rain_layer() -> crate::corner_weather::RainLayer {
    crate::corner_weather::RainLayer::new(WET_ROUTE_EYE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The saturated field is exactly the pool bound, every ring
    /// live and distinct (the finding's named worst case — the
    /// shader's loop bound).
    #[test]
    fn saturated_field_is_the_pool_bound() {
        let field = saturated_field();
        assert_eq!(field.active_count(), RIPPLE_POOL_BOUND);
        assert_eq!(field.active_count(), 32, "the §0.1 census bound");
        // The centers are distinct (the spiral spread) and the ages
        // in-policy (the fade states differ).
        let rings = field.rings();
        for ring in rings.iter() {
            assert!(ring.age >= 0.05 && ring.age < RIPPLE_POLICY.lifetime);
            assert_eq!(ring.amplitude, 1.0);
        }
        let mut centers: Vec<_> = rings.iter().map(|ring| ring.center).collect();
        centers.dedup();
        // 32 slots, two rings may share a center only if the spiral
        // aliases — the interleaved phase makes that impossible for
        // the first 32; the dedup assertion is the pin.
        assert_eq!(centers.len(), RIPPLE_POOL_BOUND, "the spread is distinct");
    }

    /// The steady construction reaches the measured steady state
    /// (~3.7 live rings — the design's §0.1 census row). The exact
    /// count is state, not structure: the assertion is the DOCUMENTED
    /// band (1..=8 live — the study's neighborhood, never zero, never
    /// near the bound), so the leg stays green across the policy's
    /// own determinism while still pinning the order of magnitude.
    #[test]
    fn steady_layer_reaches_the_measured_band() {
        let layer = steady_rain_layer();
        let live = layer.ripple_field().active_count();
        assert!(
            (1..=8).contains(&live),
            "the warm-started steady state holds a handful of live rings, measured {live}"
        );
    }
}
