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

/// The bright pass applied per channel (the bloom mip's downsample
/// filter — the shader's `fs_bright` mirrors this per channel).
pub fn bright_pass_rgb(color: [f32; 3]) -> [f32; 3] {
    [
        bright_pass(color[0]),
        bright_pass(color[1]),
        bright_pass(color[2]),
    ]
}

// The Era-1 post chain (package 6 — the render-target chain's
// deterministic core): the HDR scene target is downsampled to a 1/4
// bloom mip (the bright pass above), blurred separably, and
// composited back before the tonemap. Everything below is PURE
// fixed-order f32 arithmetic over texel values — the exact
// reference the shader chain mirrors (`shaders/bloom.wgsl` for the
// downsample and blur, `shaders/post.wgsl` for the composite); the
// lavapipe gate (`tests/render_post.rs`) compares presented pixels
// against these functions through the real render path.

/// The bloom mip's resolution divisor: the mip runs at 1/4 the
/// scene extent on both axes (floor division — the composite's
/// CLAMP_TO_EDGE bilinear covers the sub-divisible tail).
pub const BLOOM_DOWNSAMPLE: u32 = 4;

/// The composite's bloom strength (renderer policy, fixed): the
/// blurred bright energy is added back at 15% — a visible warm
/// glow around the sun disc without washing the mid-tones (the
/// stylized-coherent target; the shader uniform carries this same
/// constant, uploaded from here — the single source).
pub const BLOOM_STRENGTH: f32 = 0.15;

/// The bloom mip's extent for a scene extent (floor division, both
/// axes, at least one texel so degenerate windows stay valid).
pub fn bloom_extent(extent: (u32, u32)) -> (u32, u32) {
    (
        (extent.0 / BLOOM_DOWNSAMPLE).max(1),
        (extent.1 / BLOOM_DOWNSAMPLE).max(1),
    )
}

/// The 2×2 HDR texel box one bloom texel averages — the BILINEAR
/// footprint: a 1/4-res bilinear downsample at bloom texel (i, j)
/// samples the scene at uv·scene_extent = (4i+2, 4j+2), which
/// bilinear weights 0.5/0.5 between scene texels 4i+1 and 4i+2 on
/// each axis — exactly this box, 0.25 each. `hdr` is row-major
/// scene texels; out-of-range fetches read zero (the WGSL
/// `textureLoad` convention, mirrored).
pub fn downsample_texel(
    hdr: &[[f32; 3]],
    scene_extent: (u32, u32),
    bloom_texel: (u32, u32),
) -> [f32; 3] {
    let fetch = |x: u32, y: u32| -> [f32; 3] {
        if x >= scene_extent.0 || y >= scene_extent.1 {
            [0.0; 3]
        } else {
            hdr[(y * scene_extent.0 + x) as usize]
        }
    };
    let base_x = bloom_texel.0 * BLOOM_DOWNSAMPLE + 1;
    let base_y = bloom_texel.1 * BLOOM_DOWNSAMPLE + 1;
    let a = fetch(base_x, base_y);
    let b = fetch(base_x + 1, base_y);
    let c = fetch(base_x, base_y + 1);
    let d = fetch(base_x + 1, base_y + 1);
    // Fixed order: ((a + b) + c) + d, then the 0.25 scale — the
    // shader's `fs_bright` accumulation, term for term.
    [
        (a[0] + b[0] + c[0] + d[0]) * 0.25,
        (a[1] + b[1] + c[1] + d[1]) * 0.25,
        (a[2] + b[2] + c[2] + d[2]) * 0.25,
    ]
}

/// The composite — the post pass's equation (post.wgsl's
/// `fs_main` mirrors): `tonemap(hdr + bloom · BLOOM_STRENGTH)`
/// per channel, the bloom mip's bilinear value at this pixel
/// supplied by the caller ([`bloom_at_pixel`]).
pub fn post_pixel(hdr: [f32; 3], bloom: [f32; 3]) -> [f32; 3] {
    tonemap_rgb([
        hdr[0] + bloom[0] * BLOOM_STRENGTH,
        hdr[1] + bloom[1] * BLOOM_STRENGTH,
        hdr[2] + bloom[2] * BLOOM_STRENGTH,
    ])
}

/// One blur pass at a mip texel: the 5 [`BLOOM_WEIGHTS`] taps at
/// ±2 texels along one axis, CLAMPED at the mip borders (the
/// CLAMP_TO_EDGE convention the shader's manual texel clamp
/// mirrors), accumulated in fixed tap order — the shader's
/// `fs_blur_h` / `fs_blur_v` accumulation, term for term.
pub fn blur_texel(
    mip: &[[f32; 3]],
    mip_extent: (u32, u32),
    texel: (u32, u32),
    horizontal: bool,
) -> [f32; 3] {
    let fetch = |x: i64, y: i64| -> [f32; 3] {
        let x = x.clamp(0, mip_extent.0 as i64 - 1) as u32;
        let y = y.clamp(0, mip_extent.1 as i64 - 1) as u32;
        mip[(y * mip_extent.0 + x) as usize]
    };
    let (x, y) = (texel.0 as i64, texel.1 as i64);
    let taps: [[f32; 3]; 5] = if horizontal {
        [
            fetch(x - 2, y),
            fetch(x - 1, y),
            fetch(x, y),
            fetch(x + 1, y),
            fetch(x + 2, y),
        ]
    } else {
        [
            fetch(x, y - 2),
            fetch(x, y - 1),
            fetch(x, y),
            fetch(x, y + 1),
            fetch(x, y + 2),
        ]
    };
    // Fixed order: the taps accumulate left to right (0 + t0·w0,
    // then each next tap added — `0 + v` is bit-identical to `v`,
    // so this is the shader's chained sum exactly).
    let mut acc = [0.0f32; 3];
    for (tap, weight) in taps.iter().zip(BLOOM_WEIGHTS.iter()) {
        for channel in 0..3 {
            acc[channel] += tap[channel] * weight;
        }
    }
    acc
}

/// The full bloom mip from an HDR image (row-major texels): the
/// bright-passed 2×2 box downsample ([`downsample_texel`] +
/// [`bright_pass_rgb`]), then the horizontal blur pass over it,
/// then the vertical blur pass over THAT — the exact pass order
/// the GPU chain records (bright → blur H → blur V).
pub fn bloom_mip_from_hdr(hdr: &[[f32; 3]], scene_extent: (u32, u32)) -> Vec<[f32; 3]> {
    let mip_extent = bloom_extent(scene_extent);
    let bright = (0..mip_extent.1)
        .flat_map(|y| (0..mip_extent.0).map(move |x| (x, y)))
        .map(|(x, y)| bright_pass_rgb(downsample_texel(hdr, scene_extent, (x, y))))
        .collect::<Vec<_>>();
    let blurred_h = (0..mip_extent.1)
        .flat_map(|y| (0..mip_extent.0).map(move |x| (x, y)))
        .map(|(x, y)| blur_texel(&bright, mip_extent, (x, y), true))
        .collect::<Vec<_>>();
    (0..mip_extent.1)
        .flat_map(|y| (0..mip_extent.0).map(move |x| (x, y)))
        .map(|(x, y)| blur_texel(&blurred_h, mip_extent, (x, y), false))
        .collect()
}

/// The bloom mip's BILINEAR value at one scene pixel — the post
/// pass's upsample (`textureSample` with a linear, CLAMP_TO_EDGE
/// sampler; post.wgsl's `bloom_mip` fetch). The weights are the
/// exact f32 fractions of the 1/4-res grid: a pixel's mip
/// coordinate is `(pixel + 0.5) / 4 − 0.5`, so the fractional part
/// is always one of {0.125, 0.375, 0.625, 0.875} (exact in f32,
/// summing to 1 with its complement) and the border pixels clamp
/// to the edge texel. The reference nests u-then-v (the textbook
/// bilinear); a hardware implementation's accumulation order may
/// differ by an ulp — the gate's documented tolerance covers that
/// (the values are f16-quantized; the possible divergence is far
/// below it).
pub fn bloom_at_pixel(mip: &[[f32; 3]], scene_extent: (u32, u32), pixel: (u32, u32)) -> [f32; 3] {
    let mip_extent = bloom_extent(scene_extent);
    let coord = |pixel_axis: u32, scene_axis: u32, mip_axis: u32| -> (u32, f32) {
        // (pixel + 0.5) / 4 - 0.5 in texel units, clamped to the
        // mip range (CLAMP_TO_EDGE), then split into the integer
        // texel and the exact fractional weight.
        let raw = (pixel_axis as f32 + 0.5) * (mip_axis as f32 / scene_axis as f32) - 0.5;
        let clamped = raw.clamp(0.0, (mip_axis - 1) as f32);
        let base = clamped as u32; // truncation: the value is >= 0 here
        (base.min(mip_axis - 1), clamped - base as f32)
    };
    let (x0, fx) = coord(pixel.0, scene_extent.0, mip_extent.0);
    let (y0, fy) = coord(pixel.1, scene_extent.1, mip_extent.1);
    let x1 = (x0 + 1).min(mip_extent.0 - 1);
    let y1 = (y0 + 1).min(mip_extent.1 - 1);
    let fetch = |x: u32, y: u32| mip[(y * mip_extent.0 + x) as usize];
    let lerp = |a: [f32; 3], b: [f32; 3], t: f32| {
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]
    };
    let top = lerp(fetch(x0, y0), fetch(x1, y0), fx);
    let bottom = lerp(fetch(x0, y1), fetch(x1, y1), fx);
    lerp(top, bottom, fy)
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

    /// The bright pass is EXACTLY zero below the threshold (not
    /// merely small): the far-from-the-disc gate rests on this —
    /// a dark sky contributes nothing to the bloom mip, so those
    /// presented pixels are the pure tonemap of their HDR value.
    #[test]
    fn bright_pass_is_exactly_zero_below_one() {
        for x in [0.0f32, 0.25, 0.5, 0.75, 0.999_999] {
            assert_eq!(bright_pass(x), 0.0, "x = {x}");
        }
        assert_eq!(bright_pass_rgb([0.9, 0.99, 0.999_999]), [0.0; 3]);
    }

    /// The mip extent: floor division by 4 on both axes, at least
    /// one texel (degenerate windows stay valid; the composite's
    /// CLAMP_TO_EDGE covers the sub-divisible tail).
    #[test]
    fn bloom_extent_floor_divides_by_four() {
        assert_eq!(bloom_extent((256, 256)), (64, 64));
        assert_eq!(bloom_extent((1280, 720)), (320, 180));
        assert_eq!(bloom_extent((743, 721)), (185, 180));
        assert_eq!(bloom_extent((2, 2)), (1, 1));
        assert_eq!(bloom_extent((1, 1)), (1, 1));
    }

    /// The downsample box is the bilinear footprint: bloom texel
    /// (i, j) averages scene texels (4i+1, 4j+1)..(4i+2, 4j+2),
    /// exactly 0.25 each, in the shader's accumulation order. The
    /// probe values are chosen exact in f32 so the equality is
    /// bit-level.
    #[test]
    fn downsample_box_is_the_bilinear_footprint() {
        // A 16×8 image: texel (x, y) = [x as f32, 0.0, 0.0] — the
        // fetches land on hand-computable integers.
        let extent = (16u32, 8u32);
        let hdr: Vec<[f32; 3]> = (0..extent.0 * extent.1)
            .map(|i| [(i % extent.0) as f32, 0.0, 0.0])
            .collect();
        // Bloom texel (2, 1): scene texels (9, 5), (10, 5), (9, 6),
        // (10, 6) → ((9.0 + 10.0 + 9.0 + 10.0) * 0.25) = 9.5.
        let texel = downsample_texel(&hdr, extent, (2, 1));
        assert_eq!(texel, [9.5, 0.0, 0.0]);
        // The LAST bloom texel never reads out of bounds (extent
        // divisible: texel (3, 3) on a 16×8 → (13, 13) OOB → the
        // zero-read convention).
        let texel = downsample_texel(&hdr, extent, (3, 3));
        assert_eq!(texel, [0.0; 3], "out-of-range fetches read zero");
        // A sub-divisible tail (743 wide → bloom 185): texel 184
        // reads (737, y)..(738, y) — in range, reading the tail
        // columns the bilinear footprint covers.
        let extent = (743u32, 4u32);
        let hdr: Vec<[f32; 3]> = (0..extent.0 * extent.1)
            .map(|i| [(i % extent.0) as f32, 0.0, 0.0])
            .collect();
        let texel = downsample_texel(&hdr, extent, (184, 0));
        assert_eq!(texel, [737.5, 0.0, 0.0]);
    }

    /// The downsample feeds the bright pass per channel: a box
    /// average above 1 passes (dimmed by the knee), below 1 is
    /// exactly zero — composed by [`bloom_mip_from_hdr`].
    #[test]
    fn downsample_feeds_the_bright_pass_per_channel() {
        // 8×8, all texels uniform: every box average is the value
        // itself, so the mip's bright texel is bright_pass(value).
        for value in [0.5f32, 1.25, 2.0] {
            let extent = (8u32, 8u32);
            let hdr: Vec<[f32; 3]> = (0..64).map(|_| [value, value, value]).collect();
            let mip = bloom_mip_from_hdr(&hdr, extent);
            // Interior texel (1, 1) — mip index 3 on the 2×2: its
            // whole blur neighborhood is uniform, so it stays
            // bright_pass(value).
            let expect = bright_pass_rgb([value, value, value]);
            assert_eq!(mip[3], expect, "value {value}");
        }
    }

    /// The blur taps: the weights ARE the shader's literals
    /// (bit-equal — the fractions are powers of two), the edge
    /// texels clamp (CLAMP_TO_EDGE), and a uniform field stays
    /// exactly uniform (a weighted average of equal values is the
    /// value — the weights sum to 1).
    #[test]
    fn blur_taps_clamp_and_preserve_uniform_fields() {
        assert_eq!(BLOOM_WEIGHTS, [0.0625, 0.25, 0.375, 0.25, 0.0625]);
        let extent = (4u32, 4u32);
        // A single bright column at x = 2 (values exact in f32).
        let mip: Vec<[f32; 3]> = (0..16)
            .map(|i| {
                if i % 4 == 2 {
                    [1.0, 0.0, 0.0]
                } else {
                    [0.0; 3]
                }
            })
            .collect();
        // Texel (2, 1), horizontal: taps at x = 0..4 with weights
        // [1,4,6,4,1]/16 — only x=2 is lit → 6/16.
        let out = blur_texel(&mip, extent, (2, 1), true);
        assert_eq!(out, [0.375, 0.0, 0.0]);
        // Edge texel (0, 1), horizontal: taps clamp to x ∈ {0, 0, 0,
        // 1, 2} — the lit column x=2 keeps its LAST tap (weight
        // 0.0625) → 0.0625.
        let out = blur_texel(&mip, extent, (0, 1), true);
        assert_eq!(out, [0.0625, 0.0, 0.0]);
        // Vertical on a uniform column: texel (2, 1) vertical — all
        // taps lit → 1.0.
        let out = blur_texel(&mip, extent, (2, 1), false);
        assert_eq!(out, [1.0, 0.0, 0.0]);
        // A uniform field stays exactly itself (the normalization
        // contract, through the whole chain).
        let uniform: Vec<[f32; 3]> = (0..16).map(|_| [0.5, 0.5, 0.5]).collect();
        let out = blur_texel(&uniform, extent, (3, 3), true);
        assert_eq!(out, [0.5, 0.5, 0.5]);
        let out = blur_texel(&uniform, extent, (0, 0), false);
        assert_eq!(out, [0.5, 0.5, 0.5]);
    }

    /// The bilinear upsample's weights: the four x-mod-4 phases
    /// carry exactly {0.125, 0.375, 0.625, 0.875} (powers-of-two
    /// fractions in f32), and border pixels clamp to the edge
    /// texel. Hand-computed against a mip whose texel values are
    /// x + y/10 (all exact in f32 at this scale).
    #[test]
    fn bloom_at_pixel_bilinear_weights_and_border_clamp() {
        let scene_extent = (256u32, 256u32);
        let mip_extent = bloom_extent(scene_extent);
        let mip: Vec<[f32; 3]> = (0..mip_extent.0 * mip_extent.1)
            .map(|i| {
                let (x, y) = (i % mip_extent.0, i / mip_extent.0);
                [x as f32 + y as f32 / 10.0, 0.0, 0.0]
            })
            .collect();
        // Pixel (128, 128): mip coord 128.5/4 − 0.5 = 31.625 on
        // both axes → texel 31 (weight 0.375) blends with 32
        // (0.625). Values: (31,31)=34.1, (32,31)=35.1, (31,32)=34.2,
        // (32,32)=35.2 → 34.725 → 34.825 → 34.7875.
        let out = bloom_at_pixel(&mip, scene_extent, (128, 128));
        assert!(
            (out[0] - 34.7875).abs() < 1e-4,
            "the 31.625² blend: {out:?}"
        );
        // Pixel (130, 128): x coord 32.125 (texel 32, weight 0.875),
        // y coord 31.625 → 35.225 → 35.325 → 35.2875.
        let out = bloom_at_pixel(&mip, scene_extent, (130, 128));
        assert!((out[0] - 35.2875).abs() < 1e-4, "the 32.125 blend: {out:?}");
        // Pixel (129, 128): x coord 31.875 → texel 31, weight 0.875
        // → 34.975 → 35.075 → 35.0375 (the 0.875 phase).
        let out = bloom_at_pixel(&mip, scene_extent, (129, 128));
        assert!((out[0] - 35.0375).abs() < 1e-4, "the 31.875 blend: {out:?}");
        // Pixel (127, 128): x coord 31.375 → texel 31 weight 0.375
        // blending DOWN from 32 is impossible (31 is the base) —
        // coord 31.375 = texel 31 (0.625) + texel 32 (0.375) →
        // 34.4625 → 34.5625 → 34.53125... computed: top =
        // 34.1 + (35.1−34.1)·0.375 = 34.475; bottom = 34.575; out =
        // 34.475 + 0.1·0.625 = 34.5375.
        let out = bloom_at_pixel(&mip, scene_extent, (127, 128));
        assert!((out[0] - 34.5375).abs() < 1e-4, "the 31.375 blend: {out:?}");
        // The left border (pixel 0): coord −0.375 clamps to 0 → the
        // pure edge texel.
        let out = bloom_at_pixel(&mip, scene_extent, (0, 0));
        assert_eq!(out, [0.0, 0.0, 0.0]);
        // The right border (pixel 255): coord 63.375 clamps to 63.
        let out = bloom_at_pixel(&mip, scene_extent, (255, 0));
        assert_eq!(out, [63.0, 0.0, 0.0]);
        // The y axis blends independently of x (the same phases on
        // the other axis — covered by the 2D blends above).
    }

    /// The composite (the brief's tonemap gate in pure form): an
    /// HDR value above 1 tonemaps to the CPU-computed output WITH
    /// and WITHOUT bloom — `post_pixel` is exactly
    /// `tonemap_rgb(hdr + bloom · strength)`.
    #[test]
    fn post_pixel_is_the_composite_equation() {
        // No bloom: the pure tonemap (the far-pixel contract).
        assert_eq!(
            post_pixel([1.22, 1.38, 1.66], [0.0; 3]),
            tonemap_rgb([1.22, 1.38, 1.66])
        );
        assert_eq!(
            post_pixel([0.5, 0.25, 0.0], [0.0; 3]),
            tonemap_rgb([0.5, 0.25, 0.0])
        );
        // With bloom: exactly hdr + bloom·strength per channel first.
        let hdr = [1.22f32, 1.38, 1.66];
        let bloom = [0.9f32, 0.95, 1.1];
        let expect = tonemap_rgb([
            hdr[0] + bloom[0] * BLOOM_STRENGTH,
            hdr[1] + bloom[1] * BLOOM_STRENGTH,
            hdr[2] + bloom[2] * BLOOM_STRENGTH,
        ]);
        assert_eq!(post_pixel(hdr, bloom), expect);
        // The strength is the documented policy constant.
        assert_eq!(BLOOM_STRENGTH, 0.15);
        // The downsample divisor is the documented 1/4.
        assert_eq!(BLOOM_DOWNSAMPLE, 4);
    }

    /// The full chain on a tiny hand-computable scene: a dark 8×8
    /// field (0.5) with one 6×6 bright block (2.0) covering scene
    /// texels {2..7}². Every value below is hand-derived: the
    /// downsample boxes, the knee cut, and the two clamped blur
    /// passes — all exact in f32 (the fractions stay under 12
    /// mantissa bits at this scale).
    #[test]
    fn bloom_mip_full_chain_hand_computed() {
        let extent = (8u32, 8u32);
        let mut hdr = vec![[0.5f32, 0.5, 0.5]; 64];
        for y in 2..8 {
            for x in 2..8 {
                hdr[(y * 8 + x) as usize] = [2.0, 2.0, 2.0];
            }
        }
        let mip = bloom_mip_from_hdr(&hdr, extent);
        let mip_extent = bloom_extent(extent);
        assert_eq!(mip_extent, (2, 2));
        // The bright-passed boxes (footprint (4i+1..4i+2)²):
        //   (0,0): (1,1)(2,1)(1,2)(2,2) = 0.5·3 + 2.0 → 0.875 → knee 0
        //   (1,0): (5,1)(6,1)(5,2)(6,2) = 0.5·2 + 2.0·2 → 1.25 → 0.625
        //   (0,1): (1,5)(2,5)(1,6)(2,6) = 0.5·2 + 2.0·2 → 1.25 → 0.625
        //   (1,1): (5,5)(6,5)(5,6)(6,6) = 2.0·4 → 2.0 → 2.0 (identity
        //          above the knee's top)
        // The H blur (edge-clamped taps on the 2-wide mip — x=0
        // carries weight 0.6875, x=1 weight 0.3125, and mirrored at
        // the right edge):
        //   (0,0): 0.625·0.3125 = 0.1953125
        //   (1,0): 0.625·0.6875 = 0.4296875
        //   (0,1): 0.625·0.6875 + 2.0·0.3125 = 1.0546875
        //   (1,1): 0.625·0.3125 + 2.0·0.6875 = 1.5703125
        // The V blur (the same clamped weights on the y axis):
        //   (0,0): 0.1953125·0.6875 + 1.0546875·0.3125 = 475/1024
        //   (1,0): 0.4296875·0.6875 + 1.5703125·0.3125 = 805/1024
        //   (0,1): 0.1953125·0.3125 + 1.0546875·0.6875 = 805/1024
        //   (1,1): 0.4296875·0.3125 + 1.5703125·0.6875 = 1243/1024
        // (The literals below are the exact fractions — 475/1024,
        // 805/1024, 1243/1024 — every one of them exact in f32.)
        let expect = |numerator: f32| [numerator / 1024.0; 3];
        assert_eq!(mip[0], expect(475.0));
        assert_eq!(mip[1], expect(805.0));
        assert_eq!(mip[2], expect(805.0));
        assert_eq!(mip[3], expect(1243.0));
        // The knee cut, pinned directly: the 1.25 box passes at
        // 0.625 (mid-ramp), the 0.875 boxes at exactly 0, and 2.0
        // passes at identity.
        assert_eq!(bright_pass(1.25), 0.625);
        assert_eq!(bright_pass(0.875), 0.0);
        assert_eq!(bright_pass(2.0), 2.0);
    }
}
