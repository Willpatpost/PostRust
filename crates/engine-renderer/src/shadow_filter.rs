//! The shadow-filter deterministic core (the Era-1 scaffold — see
//! `docs/CYCLE6-ERA1-BRIEF.md`): the percentage-closer-filter (PCF)
//! tap table and the CPU-side filtered shadow factor. The 6-F gate
//! validated the depth map itself to 2.68e-7 against the CPU
//! raycaster; THIS core defines how both the shader and the oracle
//! FILTER that validated map, so the soft-shadow gate compares the
//! shader's output against the CPU PCF over the same bytes.
//!
//! DETERMINISM CONTRACT: the tap offsets are fixed integer texel
//! offsets (a literal table); the filtered factor is the fraction of
//! taps whose compare passes — pure counting over f32 compares in a
//! FIXED order (row-major, the shader mirrors it).

/// The PCF tap grid: 3×3, offsets in INTEGER TEXELS (the shader
/// scales by 1.0 / SHADOW_MAP_EXTENT; the CPU oracle samples the
/// same texel neighborhoods of the validated map). Row-major order
/// is the fixed evaluation order both sides share.
pub const PCF_GRID: u32 = 3;
/// The nine offsets, row-major: (-1,-1) (0,-1) (1,-1) (-1,0) (0,0)
/// (1,0) (-1,1) (0,1) (1,1).
pub const PCF_OFFSETS: [(i32, i32); 9] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (0, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

/// The CPU-side PCF factor: the fraction of the nine taps whose
/// depth compare passes. `sample_depth` maps a texel offset to the
/// map's depth at the projected texel plus that offset (the caller
/// reads the VALIDATED map; out-of-bounds taps return the map's
/// clear value 1.0 — the convention both sides share, so edge
/// texels soften rather than harden).
///
/// THE EQUATION (the shader mirrors term-for-term):
///   factor = (1/9) * sum over taps of
///       [ reference <= depth_at(offset) + bias ]
pub fn pcf_factor<S>(reference_depth: f32, bias: f32, sample_depth: S) -> f32
where
    S: Fn(i32, i32) -> f32,
{
    let mut passing = 0u32;
    for &(dx, dy) in PCF_OFFSETS.iter() {
        let depth = sample_depth(dx, dy);
        if reference_depth <= depth + bias {
            passing += 1;
        }
    }
    passing as f32 / 9.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All-pass and all-fail anchors, and the exact fractions for
    /// partial coverage (the count is the factor — a 3-of-9 penumbra
    /// is exactly 1/3).
    #[test]
    fn pcf_factor_anchors_and_fractions() {
        let all_lit = pcf_factor(0.5, 0.0, |_, _| 0.9);
        assert_eq!(all_lit, 1.0);
        let all_shadowed = pcf_factor(0.9, 0.0, |_, _| 0.5);
        assert_eq!(all_shadowed, 0.0);
        // The center and the four edge-neighbors lit, corners dark:
        // exactly 5/9.
        let mixed = pcf_factor(
            0.5,
            0.0,
            |dx, dy| {
                if dx == 0 || dy == 0 { 0.9 } else { 0.1 }
            },
        );
        assert_eq!(mixed, 5.0 / 9.0);
        // Only the center passes: exactly 1/9.
        let center_only = pcf_factor(
            0.5,
            0.0,
            |dx, dy| {
                if dx == 0 && dy == 0 { 0.9 } else { 0.1 }
            },
        );
        assert_eq!(center_only, 1.0 / 9.0);
    }

    /// The bias shifts the compare: with bias b, a tap passes when
    /// reference <= depth + b (the acne/panning tradeoff the 6-F
    /// model already documents — the same ShadowBias, shared by
    /// both sides).
    #[test]
    fn pcf_bias_shifts_the_compare() {
        // reference slightly BELOW all depths: all pass without bias.
        assert_eq!(pcf_factor(0.50, 0.0, |_, _| 0.55), 1.0);
        // reference slightly ABOVE all depths: all fail without
        // bias, all pass with bias >= the gap.
        assert_eq!(pcf_factor(0.60, 0.0, |_, _| 0.55), 0.0);
        assert_eq!(pcf_factor(0.60, 0.06, |_, _| 0.55), 1.0);
        // A half-gap bias changes nothing (still fails).
        assert_eq!(pcf_factor(0.60, 0.03, |_, _| 0.55), 0.0);
    }

    /// The offsets: exactly nine, row-major, spanning the 3×3 grid
    /// with the center present exactly once (the table IS the
    /// contract — a wrong table fails here, not in the shader).
    #[test]
    fn pcf_offsets_table_shape() {
        assert_eq!(PCF_OFFSETS.len(), 9);
        assert_eq!(PCF_GRID, 3);
        assert!(PCF_OFFSETS.contains(&(0, 0)));
        for x in -1..=1 {
            for y in -1..=1 {
                assert!(PCF_OFFSETS.contains(&(x, y)), "offset ({x},{y}) present");
            }
        }
        // Row-major order pinned (the evaluation order both sides
        // share): the first three share dy = -1.
        assert_eq!(PCF_OFFSETS[0].1, -1);
        assert_eq!(PCF_OFFSETS[1].1, -1);
        assert_eq!(PCF_OFFSETS[2].1, -1);
        assert_eq!(PCF_OFFSETS[4], (0, 0), "the center at index 4");
    }

    /// Out-of-bounds convention: the caller maps out-of-range taps
    /// to the clear value 1.0 (documented, both sides) — taps
    /// outside the map read as lit (no occluder). A projected texel
    /// at the map's corner (0,0) has five of its nine taps outside
    /// (every offset with a negative x or y); the four in-bounds
    /// taps read dark (failing the compare) and the five clear taps
    /// pass — exactly 5/9.
    #[test]
    fn pcf_out_of_bounds_reads_clear() {
        let factor = pcf_factor(0.5, 0.0, |dx, dy| {
            // A caller honoring the convention at the corner texel
            // (0,0): taps at negative coordinates are out of bounds
            // and read the clear value; in-bounds taps read dark.
            if dx < 0 || dy < 0 { 1.0 } else { 0.1 }
        });
        assert_eq!(factor, 5.0 / 9.0);
    }
}
