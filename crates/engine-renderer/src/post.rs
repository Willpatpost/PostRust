//! The post-processing deterministic core (the Era-1 scaffold — see
//! `docs/CYCLE6-ERA1-BRIEF.md`): the tonemap curve and the bloom
//! kernel, both pure fixed-order arithmetic. The post pass renders
//! the scene to an offscreen HDR-capable target, then the shader
//! mirrors these equations over it before presentation (the
//! implementer wires the render-target chain — the architectural
//! step this package exists for).
//!
//! DETERMINISM CONTRACT: the ACES-approximation tonemap is the
//! Narkowicz rational polynomial (pure arithmetic, no libm); the
//! bloom kernel is a fixed separable 5-tap Gaussian written as
//! literal constants (never a runtime `exp`).

/// The ACES approximation (Narkowicz 2015): a rational curve
/// mapping HDR [0, ∞) to display [0, 1]. Fixed operation order:
///   f(x) = clamp((x(2.51x + 0.03)) / (x(2.43x + 0.59) + 0.14), 0, 1)
/// The polynomial is defined for x >= 0; negative inputs (which the
/// unguarded rational would map toward 1) clamp to black — the
/// documented guard, mirrored by the shader. The shader mirrors
/// term-for-term.
pub fn tonemap(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let num = x * (2.51 * x + 0.03);
    let den = x * (2.43 * x + 0.59) + 0.14;
    (num / den).clamp(0.0, 1.0)
}

/// The tonemap applied per channel.
pub fn tonemap_rgb(color: [f32; 3]) -> [f32; 3] {
    [tonemap(color[0]), tonemap(color[1]), tonemap(color[2])]
}

/// The bloom pass's separable 5-tap Gaussian, in fixed literal
/// weights (normalized to sum exactly 1.0; the odd tap lands on the
/// center). Applied horizontally then vertically over a
/// downsampled bright-pass of the HDR target (the brief defines the
/// chain; the weights are the deterministic core).
pub const BLOOM_TAPS: u32 = 5;
pub const BLOOM_WEIGHTS: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

/// The bright-pass threshold: pixels below this luminance contribute
/// nothing to bloom (a fixed literal; the sun disc in `sky.rs`
/// deliberately exceeds 1.0 so it blooms).
pub const BLOOM_THRESHOLD: f32 = 1.0;

/// The soft knee around the threshold (a smoothstep ramp, not a
/// hard cut — arithmetic).
pub fn bright_pass(x: f32) -> f32 {
    // smoothstep(1.0, 1.5, x): 0 below 1.0, 1 above 1.5, the ramp
    // between (fixed constants, fixed order).
    let t = ((x - 1.0) / 0.5).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t) * x
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tonemap known anchors: 0 maps to 0; the filmic mid-gray 0.18
    /// maps to ~0.267 (the documented ACES behavior — the curve
    /// brightens mid-tones, it is NOT identity there); 1.0 maps
    /// below 1; huge values approach but never exceed 1.
    #[test]
    fn tonemap_anchors() {
        assert_eq!(tonemap(0.0), 0.0);
        let mid = tonemap(0.18);
        assert!((mid - 0.267).abs() < 0.005, "the ACES mid-gray: {mid}");
        let one = tonemap(1.0);
        assert!(one > 0.75 && one < 1.0, "1.0 maps below 1: {one}");
        assert!(tonemap(10.0) > one, "monotone at high end");
        assert!(tonemap(100.0) <= 1.0 && tonemap(100.0) > 0.95, "asymptote");
    }

    /// Monotone and bounded over a swept range, and negative inputs
    /// clamp to 0.
    #[test]
    fn tonemap_monotone_bounded_clamped() {
        let mut last = -1.0f32;
        for step in 0..100 {
            let x = step as f32 * 0.1;
            let y = tonemap(x);
            assert!(y >= last - 1e-7, "monotone at {x}");
            assert!((0.0..=1.0).contains(&y));
            last = y;
        }
        assert_eq!(tonemap(-0.5), 0.0);
    }

    /// The per-channel form is the scalar applied independently.
    #[test]
    fn tonemap_rgb_independent() {
        let out = tonemap_rgb([0.18, 1.0, 10.0]);
        assert_eq!(out[0], tonemap(0.18));
        assert_eq!(out[1], tonemap(1.0));
        assert_eq!(out[2], tonemap(10.0));
    }

    /// The bloom kernel: exactly 5 taps, symmetric, sums to exactly
    /// 1.0 (the normalization contract — a weighted average must
    /// not shift brightness). The shape comparisons are constant by
    /// design — they pin the table; the lint is allowed for that
    /// reason.
    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn bloom_kernel_normalized_symmetric() {
        assert_eq!(BLOOM_TAPS, 5);
        assert_eq!(BLOOM_WEIGHTS[0], BLOOM_WEIGHTS[4]);
        assert_eq!(BLOOM_WEIGHTS[1], BLOOM_WEIGHTS[3]);
        assert!(BLOOM_WEIGHTS[2] > BLOOM_WEIGHTS[1]);
        let sum: f32 = BLOOM_WEIGHTS.iter().sum();
        assert_eq!(sum, 1.0, "weights sum exactly 1.0");
    }

    /// The bright pass: 0 below threshold, identity well above, the
    /// soft knee between (and a plain HDR value like the sun's 2.0
    /// passes at full strength — the bloom feeder contract).
    #[test]
    fn bright_pass_threshold_and_knee() {
        assert_eq!(bright_pass(0.5), 0.0);
        assert_eq!(bright_pass(1.0), 0.0);
        assert!(
            (bright_pass(2.0) - 2.0).abs() < 1e-6,
            "well above: identity"
        );
        let knee = bright_pass(1.25);
        assert!(knee > 0.0 && knee < 1.25, "the knee ramps: {knee}");
        assert!(bright_pass(-1.0) <= 0.0, "negatives never bloom");
    }
}
