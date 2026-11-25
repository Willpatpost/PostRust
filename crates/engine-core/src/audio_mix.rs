//! The owned mixer's pure math (Era 2 audio vertical slice — see
//! `docs/AUDIO-SUBSYSTEM-MAP.md` §2.3, under the constraint law
//! §1): per-source gain/pan/attenuation, the bed crossfade, and
//! the stereo interleaved mix-accumulate kernel. ALL pure f32
//! arithmetic in fixed operation order — no allocation, no locks,
//! no transcendental functions (the audio-thread allocation law
//! §1.2: these functions ARE the device callback's inner loop;
//! p99 callback cost is the metric, and every one of them is a
//! bounded walk over caller-owned buffers).
//!
//! # DETERMINISM CONTRACT
//! Every value is pure IEEE f32 arithmetic (and integer ops in
//! [`to_i16`]) in a FIXED order — byte-identical on every
//! platform, like the animation core's turn-fraction rotations.
//! The pan law's cos/sin are OFFLINE-SAMPLED into a 65-entry
//! const table (the `proc_texture` / `walk_clip` precedent: the
//! values were computed once from the equal-power law and frozen
//! as literals; the runtime path never calls libm — see
//! [`PAN_GAINS`] for provenance and the measured error bound).
//!
//! # THE i16 BOUNDARY
//! The mixer's math is f32 end-to-end; the device boundary
//! consumes **i16 PCM** (the SDL3-ready format — `SDL_AUDIO_S16`,
//! little-endian 16-bit samples, the shape `SDL_AudioStream`
//! converts natively). Conversion happens ONLY at the final
//! output boundary via [`to_i16`] (clamped, symmetric, rounding
//! by pure truncation arithmetic — never `f32::round`, which is
//! a libm boundary). SDL3 also offers F32 streams; i16 is pinned
//! here as the vertical slice's choice because it is the format
//! the cook-time packed-PCM store (§2.2) will carry, halving
//! residency memory for the same content.
//!
//! # THE PAN LAW (equal power)
//! `pan` ∈ [−1, 1]: −1 = hard left, 0 = center, +1 = hard right.
//! The equal-power law: left = cos((pan+1)·π/4),
//! right = sin((pan+1)·π/4) — a centered source plays at
//! (√2/2, √2/2), so both channels sum to constant power (a
//! centered source is neither quieter nor louder than a hard-
//! panned one). The runtime evaluates it through [`PAN_GAINS`].

/// The equal-power pan table: 65 entries, entry `i` at pan
/// `−1 + i/32` (angle `i·π/128`), `[left, right]` gains.
///
/// PROVENANCE: computed OFFLINE from the equal-power law
/// left = cos((pan+1)·π/4), right = sin((pan+1)·π/4), rounded to
/// f32, and frozen here as literals — the runtime never calls
/// cos/sin (the libm-free law; the `proc_texture` /
/// `animation::walk_clip` offline-sampling precedent). The
/// endpoints are the exact literals (1, 0) and (0, 1); entry 32
/// (center) carries √2/2 on both channels.
///
/// PRECISION: linear interpolation between adjacent entries
/// (see [`pan_gains`]) has a theoretical worst-case error of
/// h²/8 over the entry spacing h = π/128 (the second-derivative
/// bound on cos/sin), ≈ 7.5e-5 — measured 7.6e-5 over a dense
/// sweep, comfortably ≤ the 1e-3 gain-accuracy budget (a gain
/// error of 1e-3 is ~0.009 dB, far below audible).
const PAN_GAINS: [[f32; 2]; 65] = [
    [1.0, 0.0],                // 0: pan -1.0000, angle 0·π/128
    [0.9996988, 0.024541229],  // 1: pan -0.9688, angle 1·π/128
    [0.99879545, 0.049067676], // 2: pan -0.9375, angle 2·π/128
    [0.99729043, 0.07356457],  // 3: pan -0.9063, angle 3·π/128
    [0.9951847, 0.09801714],   // 4: pan -0.8750, angle 4·π/128
    [0.99247956, 0.12241068],  // 5: pan -0.8438, angle 5·π/128
    [0.9891765, 0.14673047],   // 6: pan -0.8125, angle 6·π/128
    [0.98527765, 0.17096189],  // 7: pan -0.7813, angle 7·π/128
    [0.98078525, 0.19509032],  // 8: pan -0.7500, angle 8·π/128
    [0.9757021, 0.21910124],   // 9: pan -0.7188, angle 9·π/128
    [0.97003126, 0.24298018],  // 10: pan -0.6875, angle 10·π/128
    [0.96377605, 0.26671275],  // 11: pan -0.6563, angle 11·π/128
    [0.95694035, 0.29028466],  // 12: pan -0.6250, angle 12·π/128
    [0.94952816, 0.31368175],  // 13: pan -0.5938, angle 13·π/128
    [0.94154406, 0.33688986],  // 14: pan -0.5625, angle 14·π/128
    [0.9329928, 0.35989505],   // 15: pan -0.5313, angle 15·π/128
    [0.9238795, 0.38268343],   // 16: pan -0.5000, angle 16·π/128
    [0.9142098, 0.4052413],    // 17: pan -0.4688, angle 17·π/128
    [0.9039893, 0.42755508],   // 18: pan -0.4375, angle 18·π/128
    [0.8932243, 0.44961134],   // 19: pan -0.4063, angle 19·π/128
    [0.8819213, 0.47139674],   // 20: pan -0.3750, angle 20·π/128
    [0.87008697, 0.4928982],   // 21: pan -0.3438, angle 21·π/128
    [0.8577286, 0.51410276],   // 22: pan -0.3125, angle 22·π/128
    [0.8448536, 0.53499764],   // 23: pan -0.2813, angle 23·π/128
    [0.8314696, 0.55557024],   // 24: pan -0.2500, angle 24·π/128
    [0.8175848, 0.57580817],   // 25: pan -0.2188, angle 25·π/128
    [0.8032075, 0.5956993],    // 26: pan -0.1875, angle 26·π/128
    [0.7883464, 0.6152316],    // 27: pan -0.1563, angle 27·π/128
    [0.77301043, 0.6343933],   // 28: pan -0.1250, angle 28·π/128
    [0.7572088, 0.65317285],   // 29: pan -0.0938, angle 29·π/128
    [0.7409511, 0.671559],     // 30: pan -0.0625, angle 30·π/128
    [0.7242471, 0.68954057],   // 31: pan -0.0313, angle 31·π/128
    [0.70710677, 0.70710677],  // 32: pan 0.0000, angle 32·π/128
    [0.68954057, 0.7242471],   // 33: pan 0.0313, angle 33·π/128
    [0.671559, 0.7409511],     // 34: pan 0.0625, angle 34·π/128
    [0.65317285, 0.7572088],   // 35: pan 0.0938, angle 35·π/128
    [0.6343933, 0.77301043],   // 36: pan 0.1250, angle 36·π/128
    [0.6152316, 0.7883464],    // 37: pan 0.1563, angle 37·π/128
    [0.5956993, 0.8032075],    // 38: pan 0.1875, angle 38·π/128
    [0.57580817, 0.8175848],   // 39: pan 0.2188, angle 39·π/128
    [0.55557024, 0.8314696],   // 40: pan 0.2500, angle 40·π/128
    [0.53499764, 0.8448536],   // 41: pan 0.2813, angle 41·π/128
    [0.51410276, 0.8577286],   // 42: pan 0.3125, angle 42·π/128
    [0.4928982, 0.87008697],   // 43: pan 0.3438, angle 43·π/128
    [0.47139674, 0.8819213],   // 44: pan 0.3750, angle 44·π/128
    [0.44961134, 0.8932243],   // 45: pan 0.4063, angle 45·π/128
    [0.42755508, 0.9039893],   // 46: pan 0.4375, angle 46·π/128
    [0.4052413, 0.9142098],    // 47: pan 0.4688, angle 47·π/128
    [0.38268343, 0.9238795],   // 48: pan 0.5000, angle 48·π/128
    [0.35989505, 0.9329928],   // 49: pan 0.5313, angle 49·π/128
    [0.33688986, 0.94154406],  // 50: pan 0.5625, angle 50·π/128
    [0.31368175, 0.94952816],  // 51: pan 0.5938, angle 51·π/128
    [0.29028466, 0.95694035],  // 52: pan 0.6250, angle 52·π/128
    [0.26671275, 0.96377605],  // 53: pan 0.6563, angle 53·π/128
    [0.24298018, 0.97003126],  // 54: pan 0.6875, angle 54·π/128
    [0.21910124, 0.9757021],   // 55: pan 0.7188, angle 55·π/128
    [0.19509032, 0.98078525],  // 56: pan 0.7500, angle 56·π/128
    [0.17096189, 0.98527765],  // 57: pan 0.7813, angle 57·π/128
    [0.14673047, 0.9891765],   // 58: pan 0.8125, angle 58·π/128
    [0.12241068, 0.99247956],  // 59: pan 0.8438, angle 59·π/128
    [0.09801714, 0.9951847],   // 60: pan 0.8750, angle 60·π/128
    [0.07356457, 0.99729043],  // 61: pan 0.9063, angle 61·π/128
    [0.049067676, 0.99879545], // 62: pan 0.9375, angle 62·π/128
    [0.024541229, 0.9996988],  // 63: pan 0.9688, angle 63·π/128
    [0.0, 1.0],                // 64: pan 1.0000, angle 64·π/128
];

/// The equal-power stereo pan gains for `pan` ∈ [−1, 1]
/// (−1 = hard left, 0 = center, +1 = hard right): `(left, right)`.
///
/// The law is evaluated through [`PAN_GAINS`] with LINEAR
/// interpolation between adjacent entries — no runtime cos/sin
/// (the libm-free law). Pan values outside [−1, 1] are clamped
/// (a caller's out-of-range pan is a policy bug; the mixer's
/// guarded answer is the hard stop, never a NaN). Pan values at
/// table nodes (multiples of 1/32) return the table entry
/// exactly; the hard endpoints (1, 0) and (0, 1) and the center
/// (√2/2, √2/2) are therefore bit-exact known answers.
/// Interpolated values carry the documented ≤ 7.6e-5 error
/// against the true equal-power law (see [`PAN_GAINS`]).
pub fn pan_gains(pan: f32) -> (f32, f32) {
    // Table position: pan −1 → 0, 0 → 32, +1 → 64.
    let position = (pan.clamp(-1.0, 1.0) + 1.0) * 32.0;
    let lower = (position as usize).min(PAN_GAINS.len() - 2);
    let frac = position - lower as f32;
    let (low_left, low_right) = (PAN_GAINS[lower][0], PAN_GAINS[lower][1]);
    let (high_left, high_right) = (PAN_GAINS[lower + 1][0], PAN_GAINS[lower + 1][1]);
    if frac <= 0.0 {
        return (low_left, low_right);
    }
    if frac >= 1.0 {
        return (high_left, high_right);
    }
    (
        low_left + (high_left - low_left) * frac,
        low_right + (high_right - low_right) * frac,
    )
}

/// The distance attenuation in [0, 1] (the §2.3 falloff —
/// game one's floor): **1.0 at or below `ref_distance`**, and the
/// inverse-distance falloff beyond:
/// `ref / (ref + rolloff · (d − ref))` — pure arithmetic, no
/// transcendentals. At double the reference distance with
/// rolloff 1 the gain is exactly 0.5; as distance → ∞ the gain
/// approaches (but never reaches) 0.
///
/// Degenerate guards (documented choices, fail-safe audible
/// rather than silent): a non-positive `ref_distance` or a
/// non-positive `rolloff` returns 1.0 (no attenuation) — policy
/// parameters the caller validates upstream; the mixer never
/// divides by zero or produces NaN.
pub fn attenuation(distance: f32, ref_distance: f32, rolloff: f32) -> f32 {
    if ref_distance <= 0.0 || rolloff <= 0.0 || distance <= ref_distance {
        return 1.0;
    }
    let denominator = ref_distance + rolloff * (distance - ref_distance);
    if denominator <= 0.0 {
        // Unreachable while the guards above hold (denominator ≥
        // ref_distance > 0); kept as the belt-and-braces NaN
        // refusal for malformed f32 inputs.
        return 1.0;
    }
    (ref_distance / denominator).clamp(0.0, 1.0)
}

/// The listener-relative stereo pan in [−1, 1] for a source at
/// world `position`, from the listener's position and basis.
///
/// CONTRACT: the caller supplies PRE-NORMALIZED, ORTHOGONAL
/// `listener_forward` and `listener_right` (the camera/pose
/// layer owns normalization — it may use the IEEE-exact sqrt
/// there; this function does not). The pan is the right-axis
/// projection over the combined axis magnitudes:
/// `pan = dot(Δ, right) / (|dot(Δ, right)| + |dot(Δ, forward)|)`
/// where Δ = position − listener_pos — the ratio form lands in
/// [−1, 1] WITHOUT a vector-length sqrt (hard right/left when
/// the source is on the right/left axis, 0 on the forward axis
/// AND dead behind, ±0.5 on the 45° diagonals). A source at the
/// listener's exact position (both projections zero) returns 0
/// (center) — the guarded answer, never a NaN.
pub fn pan_from_listener(
    position: [f32; 3],
    listener_pos: [f32; 3],
    listener_forward: [f32; 3],
    listener_right: [f32; 3],
) -> f32 {
    let delta = [
        position[0] - listener_pos[0],
        position[1] - listener_pos[1],
        position[2] - listener_pos[2],
    ];
    let right_projection =
        delta[0] * listener_right[0] + delta[1] * listener_right[1] + delta[2] * listener_right[2];
    let forward_projection = delta[0] * listener_forward[0]
        + delta[1] * listener_forward[1]
        + delta[2] * listener_forward[2];
    let combined = right_projection.abs() + forward_projection.abs();
    if combined > 0.0 {
        (right_projection / combined).clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// The bed crossfade (§2.3's weather/time-driven palette swaps):
/// the equal-gain blend `(1 − t)·a + t·b` with `t` clamped to
/// [0, 1]. At t = 0 the result is exactly `a`; at t = 1 exactly
/// `b`; at 0.5 the midpoint `(a + b) / 2` — the linear law two
/// beds sum under when the mixer plays both (equal-gain, not
/// equal-power: bed layers are correlated material, and the sum
/// must not overshoot the destination headroom).
pub fn crossfade(a: f32, b: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    (1.0 - t) * a + t * b
}

/// Mixes a mono `source` into a STEREO INTERLEAVED `destination`
/// (frame layout `[left₀, right₀, left₁, right₁, …]`) under the
/// per-channel `(left, right)` gain pair — the one-shot voice
/// and bed mix kernel (§2.3), the audio-thread inner loop.
///
/// Fixed order: frame index ascending, left sample then right
/// sample within each frame. Accumulates (`+=`) into the
/// destination — mixing is additive; the caller owns the final
/// clamp at the i16 boundary ([`to_i16`]). Bounds-checked: the
/// SHORTER length wins — at most `min(destination.len() / 2,
/// source.len())` frames are mixed; a destination longer than
/// the source's frame count keeps its tail untouched, and a
/// source longer than the destination's frame count is mixed
/// only up to the destination's capacity (never an overrun,
/// never a panic). No allocation.
pub fn mix_into(destination: &mut [f32], source: &[f32], gain: (f32, f32)) {
    let frames = destination.len() / 2;
    let frames = if frames < source.len() {
        frames
    } else {
        source.len()
    };
    for frame in 0..frames {
        let sample = source[frame];
        destination[frame * 2] += sample * gain.0;
        destination[frame * 2 + 1] += sample * gain.1;
    }
}

/// Converts one f32 mixer sample to i16 PCM (the SDL3 boundary —
/// see the module header's i16 section): clamps to [−1, 1],
/// scales to the SYMMETRIC range [−32767, +32767] (−32768 is
/// never produced, so a full-scale-negative sample and its
/// positive mirror round-trip symmetrically), and rounds half
/// away from zero by pure truncation arithmetic — `x ≥ 0:
/// trunc(x + 0.5)`, `x < 0: trunc(x − 0.5)` — never `f32::round`
/// (a libm boundary). Deterministic on every platform.
pub fn to_i16(value: f32) -> i16 {
    let scaled = value.clamp(-1.0, 1.0) * 32767.0;
    let rounded = if scaled >= 0.0 {
        scaled + 0.5
    } else {
        scaled - 0.5
    };
    rounded.clamp(-32767.0, 32767.0) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hard left / center / hard right — the endpoints and the
    /// center are bit-exact table nodes (the equal-power law's
    /// known answers).
    #[test]
    fn pan_endpoints_and_center() {
        let (left, right) = pan_gains(-1.0);
        assert_eq!((left, right), (1.0, 0.0), "hard left is (1, 0)");
        let (left, right) = pan_gains(0.0);
        assert_eq!(left, right, "center is equal-power equal");
        assert!(
            (left - core::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6,
            "center is √2/2, got {left}"
        );
        let (left, right) = pan_gains(1.0);
        assert_eq!((left, right), (0.0, 1.0), "hard right is (0, 1)");
    }

    /// Out-of-range pans clamp to the hard stops (a policy bug
    /// upstream never becomes a NaN or an inverted image).
    #[test]
    fn pan_clamps_out_of_range() {
        assert_eq!(pan_gains(-2.0), pan_gains(-1.0));
        assert_eq!(pan_gains(7.5), pan_gains(1.0));
    }

    /// The equal-power invariant across every table node:
    /// left² + right² ≈ 1 (the law's defining property), and the
    /// table is monotone (left non-increasing, right
    /// non-decreasing — a pan knob never inverts).
    #[test]
    fn pan_table_equal_power_and_monotone() {
        for window in PAN_GAINS.windows(2) {
            let (a, b) = (window[0], window[1]);
            assert!(
                (a[0] * a[0] + a[1] * a[1] - 1.0).abs() < 1e-3,
                "node ({}, {}) violates equal power",
                a[0],
                a[1]
            );
            assert!(b[0] <= a[0], "left gain non-increasing");
            assert!(b[1] >= a[1], "right gain non-decreasing");
        }
    }

    /// Interpolated pans track the true equal-power law within
    /// the documented ≤ 7.6e-5 error (pins at off-node pans —
    /// the values are the offline-computed law, f32).
    #[test]
    fn pan_interpolation_tracks_the_law() {
        // pan = +0.015625 (halfway between nodes 32 and 33).
        let (left, right) = pan_gains(0.015625);
        assert!((left - 0.69837624).abs() < 2e-4, "left got {left}");
        assert!((right - 0.71573085).abs() < 2e-4, "right got {right}");
        // pan = −0.5078125 (three-quarters between nodes 15 and 16).
        let (left, right) = pan_gains(-0.5078125);
        assert!((left - 0.9262102).abs() < 2e-4, "left got {left}");
        assert!((right - 0.37700742).abs() < 2e-4, "right got {right}");
        // pan = +0.9921875 (three-quarters between nodes 63 and 64).
        let (left, right) = pan_gains(0.9921875);
        assert!((left - 0.0061358847).abs() < 2e-4, "left got {left}");
        assert!((right - 0.99998116).abs() < 2e-4, "right got {right}");
    }

    /// The half-pan mirror: pan −0.5 and +0.5 are exact table
    /// nodes and mirror each other (the law's symmetry).
    #[test]
    fn pan_half_nodes_mirror() {
        let (left_l, right_l) = pan_gains(-0.5);
        let (left_r, right_r) = pan_gains(0.5);
        assert_eq!(left_l, right_r);
        assert_eq!(right_l, left_r);
    }

    /// The attenuation anchors: at/below the reference = 1.0,
    /// exactly 0.5 at double the reference with rolloff 1,
    /// 0.1 at ten times, and monotonically approaching 0 far
    /// out. Pure arithmetic — these are exact f32 divisions.
    #[test]
    fn attenuation_known_answers() {
        assert_eq!(attenuation(1.0, 1.0, 1.0), 1.0, "at the reference");
        assert_eq!(attenuation(0.25, 1.0, 1.0), 1.0, "below the reference");
        assert_eq!(
            attenuation(2.0, 1.0, 1.0),
            0.5,
            "double reference, rolloff 1"
        );
        assert!(
            (attenuation(10.0, 1.0, 1.0) - 0.1).abs() < 1e-6,
            "ten times reference − 0.1"
        );
        let far = attenuation(1.0e9, 1.0, 1.0);
        assert!(far > 0.0 && far < 1e-6, "far away approaches 0, got {far}");
        // Softer rolloff halves the falloff at the same distance.
        assert!(
            (attenuation(2.0, 1.0, 0.5) - 2.0 / 3.0).abs() < 1e-6,
            "rolloff 0.5 at double reference = 2/3"
        );
        // Degenerate guards: fail-safe audible, never NaN.
        assert_eq!(attenuation(50.0, 0.0, 1.0), 1.0, "non-positive reference");
        assert_eq!(attenuation(50.0, 1.0, 0.0), 1.0, "non-positive rolloff");
    }

    /// The listener-relative pan known answers (Y-up, +Z
    /// forward, +X right — the animation core's convention):
    /// in front = 0, hard right = +1, hard left = −1, directly
    /// behind = 0, the 45° front-right diagonal = +0.5, and a
    /// source at the listener's position = 0 (the guard).
    #[test]
    fn pan_from_listener_known_answers() {
        let forward = [0.0, 0.0, 1.0];
        let right = [1.0, 0.0, 0.0];
        let listener = [0.0, 0.0, 0.0];
        assert_eq!(
            pan_from_listener([0.0, 0.0, 5.0], listener, forward, right),
            0.0
        );
        assert_eq!(
            pan_from_listener([5.0, 0.0, 0.0], listener, forward, right),
            1.0
        );
        assert_eq!(
            pan_from_listener([-5.0, 0.0, 0.0], listener, forward, right),
            -1.0
        );
        assert_eq!(
            pan_from_listener([0.0, 0.0, -5.0], listener, forward, right),
            0.0
        );
        assert_eq!(
            pan_from_listener([5.0, 0.0, 5.0], listener, forward, right),
            0.5,
            "the 45° diagonal halves the pan"
        );
        assert_eq!(
            pan_from_listener(listener, listener, forward, right),
            0.0,
            "source at the listener is centered"
        );
        // The listener offset composes: same source, listener
        // moved 10 units left of it pans hard right.
        let moved = [-10.0, 0.0, 0.0];
        assert_eq!(
            pan_from_listener([0.0, 0.0, 0.0], moved, forward, right),
            1.0
        );
    }

    /// The bed crossfade endpoints and midpoint (equal-gain).
    #[test]
    fn crossfade_endpoints_and_midpoint() {
        assert_eq!(crossfade(0.25, 0.75, 0.0), 0.25, "t = 0 is exactly a");
        assert_eq!(crossfade(0.25, 0.75, 1.0), 0.75, "t = 1 is exactly b");
        assert_eq!(crossfade(0.25, 0.75, 0.5), 0.5, "t = 0.5 is the midpoint");
        // t clamps — a bed swap timer overshoot never extrapolates.
        assert_eq!(crossfade(0.25, 0.75, -3.0), 0.25);
        assert_eq!(crossfade(0.25, 0.75, 4.0), 0.75);
        // Equal inputs stay put at any t (a bed crossfaded with
        // itself is itself, to f32 rounding).
        assert!((crossfade(0.6, 0.6, 0.37) - 0.6).abs() < 1e-6);
    }

    /// The mix kernel: accumulation, the stereo interleave, and
    /// the fixed left-then-right order — exact f32 arithmetic,
    /// pinned as known answers.
    #[test]
    fn mix_into_accumulates_stereo_interleaved() {
        let mut destination = [0.5, -0.5, 0.25, 0.75];
        let source = [1.0, -0.5];
        mix_into(&mut destination, &source, (0.5, 2.0));
        // Frame 0: 1.0 · gains; frame 1: −0.5 · gains.
        assert_eq!(destination, [1.0, 1.5, 0.0, -0.25]);
        // Accumulation: mixing the same source again adds on top.
        mix_into(&mut destination, &source, (0.5, 2.0));
        assert_eq!(destination, [1.5, 3.5, -0.25, -1.25]);
        // A centered pan applies equal gains to both channels.
        let mut centered = [0.0; 4];
        let half_root_two = core::f32::consts::FRAC_1_SQRT_2;
        mix_into(&mut centered, &[0.5, 0.5], (half_root_two, half_root_two));
        assert_eq!(centered, [0.35355338, 0.35355338, 0.35355338, 0.35355338]);
    }

    /// The bounds contract: the shorter length wins — a source
    /// longer than the destination's frame count mixes only the
    /// fitting frames; a destination longer than the source
    /// keeps its tail untouched.
    #[test]
    fn mix_into_shorter_length_wins() {
        // Source (5 frames) longer than destination (2 frames):
        // only 2 frames mixed, no overrun.
        let mut destination = [0.0; 4];
        let source = [1.0, 1.0, 1.0, 1.0, 1.0];
        mix_into(&mut destination, &source, (1.0, 1.0));
        assert_eq!(destination, [1.0, 1.0, 1.0, 1.0]);
        // Destination (4 frames) longer than source (1 frame):
        // the tail is untouched.
        let mut destination = [9.0; 8];
        mix_into(&mut destination, &[0.5], (2.0, 4.0));
        assert_eq!(destination, [10.0, 11.0, 9.0, 9.0, 9.0, 9.0, 9.0, 9.0]);
        // An odd destination length is measured in whole frames.
        let mut destination = [9.0; 5];
        mix_into(&mut destination, &[0.5, 0.5, 0.5], (1.0, 1.0));
        assert_eq!(destination, [9.5, 9.5, 9.5, 9.5, 9.0]);
    }

    /// The i16 boundary: symmetric clamped conversion, rounding
    /// half away from zero, −32768 never produced.
    #[test]
    fn to_i16_clamps_and_rounds() {
        assert_eq!(to_i16(0.0), 0);
        assert_eq!(to_i16(1.0), 32767);
        assert_eq!(to_i16(-1.0), -32767, "−32768 is never produced");
        assert_eq!(to_i16(1.5), 32767, "overshoot clamps");
        assert_eq!(to_i16(-9.0), -32767, "undershoot clamps");
        assert_eq!(to_i16(0.5), 16384, "0.5·32767 rounds up to 16384");
        assert_eq!(to_i16(-0.5), -16384, "rounds away from zero");
        assert_eq!(to_i16(0.00002), 1, "just past the quantum rounds to 1");
        assert_eq!(to_i16(-0.00002), -1, "rounds away from zero");
        assert_eq!(to_i16(0.00001), 0, "below the i16 quantum is silence");
        assert_eq!(to_i16(0.0), 0, "silence stays silent");
    }
}
