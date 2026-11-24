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

use crate::shadows::{ShadowBias, ShadowCascade};

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

/// The point-level PCF oracle: the FULL convention the Era-1
/// shadowed shader (`lit_scene_shadowed.wgsl`'s `shadow_factor`)
/// evaluates, mirrored term-for-term on the CPU — the projection
/// through the cascade matrix (the same column-major multiply, the
/// same divide by w), the outside-the-box early return (factor 1.0),
/// the y-flipped texel expression with the truncation and the
/// +1-edge clamp, the slope-scaled compare-time bias
/// ([`crate::shadows::depth_bias`]'s expression from the same
/// inputs), and the nine-tap row-major loop with the
/// out-of-bounds-reads-clear convention. `tap_depth` maps an
/// IN-BOUNDS map texel to its stored depth (the gate reads the
/// CAPTURED map bytes — the map the shader filtered, already proven
/// to 2.68e-7 by the 6-F gates).
///
/// DETERMINISM CONTRACT: pure f32 arithmetic in the shader's exact
/// operation order; the compare is the CPU core's own form
/// (`reference <= depth + bias`). The remaining GPU/CPU divergence
/// (fma contraction in the matrix multiply, the WGSL `normalize`)
/// is ulp-scale — the gate's exposure classes own it (documented
/// there).
pub fn pcf_at_point<S>(
    cascade: &ShadowCascade,
    extent: u32,
    point: [f32; 3],
    normal: [f32; 3],
    bias: ShadowBias,
    tap_depth: S,
) -> f32
where
    S: Fn(u32, u32) -> f32,
{
    // The projection: clip = Σ columns[i] · point[i] (the repo's
    // column-major convention; the shader's mat4x4 multiply), then
    // the divide by w (exactly 1.0 for this orthographic matrix, a
    // float no-op both sides share).
    let clip: [f32; 4] = std::array::from_fn(|i| {
        cascade.matrix[0][i] * point[0]
            + cascade.matrix[1][i] * point[1]
            + cascade.matrix[2][i] * point[2]
            + cascade.matrix[3][i]
    });
    let w = clip[3];
    let ndc = [clip[0] / w, clip[1] / w, clip[2] / w];
    if !(-1.0..=1.0).contains(&ndc[0])
        || !(-1.0..=1.0).contains(&ndc[1])
        || !(0.0..=1.0).contains(&ndc[2])
    {
        // Outside the cascade's orthographic box: the light is
        // unblocked by construction (the box covers the whole
        // scene) — the shader's own early return.
        return 1.0;
    }
    // The y-flipped texel expression (the map's row axis carries the
    // flipped NDC y — the depth-only pass's vertex epilogue), the
    // f32→u32 truncation, and the +1-edge clamp: the shader's exact
    // form, term-for-term.
    let extent_f = extent as f32;
    let fx = (ndc[0] + 1.0) * 0.5 * extent_f;
    let fy = (1.0 - (ndc[1] + 1.0) * 0.5) * extent_f;
    let texel_x = (fx as u32).min(extent - 1);
    let texel_y = (fy as u32).min(extent - 1);
    // The compare-time bias: the same expression the shader and
    // shadows::depth_bias evaluate from the same inputs.
    let facing = (normal[0] * cascade.light[0]
        + normal[1] * cascade.light[1]
        + normal[2] * cascade.light[2])
        .abs()
        .max(1.0 / 64.0);
    let bias_value = bias.constant + bias.slope_scale * (1.0 / facing - 1.0);
    // The nine taps: out-of-bounds reads map to the clear value 1.0
    // (the convention both sides share), in-bounds reads to the
    // caller's map.
    pcf_factor(ndc[2], bias_value, |dx, dy| {
        let x = texel_x as i32 + dx;
        let y = texel_y as i32 + dy;
        if x < 0 || y < 0 || x >= extent as i32 || y >= extent as i32 {
            1.0
        } else {
            tap_depth(x as u32, y as u32)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lit_scene::{day_preset, intersection_boxes};
    use crate::shadows::{
        SHADOW_MAP_EXTENT, cascade_for, is_shadowed, map_texel_depth, scene_bounds,
    };

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

    /// The point-level oracle against a hand-computed STEP map
    /// through the REAL projection: the synthetic vertical-light
    /// cascade (right = +Z, up = +X — the shadows.rs fixture), a
    /// map that reads 0.1 at texels with x <= K and 1.0 beyond, and
    /// receivers placed at a chosen texel's CENTER-RAY GROUND
    /// CROSSING (a point whose projection lands at that texel's
    /// center — the texel_ray_origin round-trip). With the ground
    /// receiver's reference depth exactly 0.75 ((far − 0)/span) and
    /// zero bias, the 0.1 texels fail the compare and the 1.0
    /// texels pass, so the factor is pure tap-counting arithmetic —
    /// the fractions hand-computed per anchor position.
    #[test]
    fn pcf_at_point_pins_the_step_map_through_the_real_projection() {
        let cascade = cascade_for([0.0, 1.0, 0.0], [-1.0, -1.0, -1.0], [3.0, 3.0, 3.0]);
        assert_eq!(cascade.right, [0.0, 0.0, 1.0]);
        assert_eq!(cascade.up, [1.0, 0.0, 0.0]);
        let n = 64u32;
        let k = 40u32;
        let step_map = |x: u32, _y: u32| if x <= k { 0.1 } else { 1.0 };
        // The anchor ground point for texel (tx, ty): its center
        // ray's ground crossing (the vertical light descends along
        // −Y from the far plane at y = 3).
        let anchor = |tx: u32, ty: u32| {
            let origin = crate::shadows::texel_ray_origin(&cascade, tx, ty, n);
            [origin[0], 0.0, origin[2]]
        };
        let zero_bias = ShadowBias {
            constant: 0.0,
            slope_scale: 0.0,
        };
        let ground = [0.0, 1.0, 0.0];
        let factor = |tx: u32, ty: u32| {
            pcf_at_point(&cascade, n, anchor(tx, ty), ground, zero_bias, step_map)
        };
        // Anchor (k, 5): every tap row reads x = k−1 (fail), k
        // (fail), k+1 (pass) — exactly 3 of 9.
        assert_eq!(factor(k, 5), 3.0 / 9.0);
        // Anchor (k+1, 5): fail, pass, pass per row — exactly 6 of 9.
        assert_eq!(factor(k + 1, 5), 6.0 / 9.0);
        // Deep occluded (all taps <= k): exactly 0. Deep clear (the
        // taps at 62/63 read 1.0 and the x = 64 tap is OUT OF
        // BOUNDS, reading the clear value): exactly 1.
        assert_eq!(factor(2, 5), 0.0);
        assert_eq!(factor(63, 5), 1.0);
        // Determinism (the fixed tap order and compare form).
        assert_eq!(factor(k, 5), 3.0 / 9.0);
    }

    /// The out-of-bounds convention through the real projection: the
    /// anchor at the map's corner texel (0, 0) has five of its nine
    /// taps outside (every offset with a negative x or y reads the
    /// clear value 1.0 and passes); the four in-bounds taps read the
    /// step map's 0.1 and fail — exactly 5/9, the scaffold corner
    /// case reproduced at the point level.
    #[test]
    fn pcf_at_point_out_of_bounds_taps_read_clear_at_the_corner() {
        let cascade = cascade_for([0.0, 1.0, 0.0], [-1.0, -1.0, -1.0], [3.0, 3.0, 3.0]);
        let n = 64u32;
        let step_map = |x: u32, _y: u32| if x <= 40 { 0.1 } else { 1.0 };
        let origin = crate::shadows::texel_ray_origin(&cascade, 0, 0, n);
        let anchor = [origin[0], 0.0, origin[2]];
        let factor = pcf_at_point(
            &cascade,
            n,
            anchor,
            [0.0, 1.0, 0.0],
            ShadowBias {
                constant: 0.0,
                slope_scale: 0.0,
            },
            step_map,
        );
        assert_eq!(factor, 5.0 / 9.0);
    }

    /// The 6-F bias discipline under the filter, at the REAL map
    /// extent (the Era-1 "no new acne, no peter-panning" gate,
    /// CPU-side over the modeled map): every sampled raycaster-LIT
    /// ground point either reads the full factor 1.0 (interior lit
    /// ground — the ground's own map texels pass the biased compare,
    /// the margin analysis: the in-texel receiver offset costs at
    /// most ~0.0006 of depth against the 0.0015 constant bias) or
    /// sits within two light-space texels of a shadow boundary (the
    /// penumbra, where occluder texels legitimately fail). A failing
    /// tap with NO boundary nearby would be self-shadow acne — the
    /// count pinned at zero. Detachment (peter-panning): the ground
    /// immediately northeast of the SW mass's walls reads the FULL
    /// shadow factor (the umbra touches its caster's base — no lit
    /// band), and the penumbra band itself exists on this scene at
    /// this extent (all three regimes present along the flip walk).
    #[test]
    fn corner_pcf_holds_the_bias_discipline_at_the_real_extent() {
        let boxes = intersection_boxes();
        let (min, max) = scene_bounds(boxes);
        let cascade = cascade_for(day_preset().dir_toward_light, min, max);
        let extent = SHADOW_MAP_EXTENT;
        let bias = ShadowBias {
            constant: 0.0015,
            slope_scale: 0.002,
        };
        let ground = [0.0, 1.0, 0.0];
        let modeled = |x: u32, y: u32| map_texel_depth(&cascade, boxes, x, y, extent).depth;
        let factor_at =
            |point: [f32; 3]| pcf_at_point(&cascade, extent, point, ground, bias, modeled);

        // The acne sweep: raycaster-lit ground outside the masses.
        // The DIRECT classification (no boundary heuristics): a lit
        // point's failing tap is legitimate penumbra when the tap's
        // texel reads an OCCLUDER depth (a box hit — the receiver
        // sits in that box's shadow volume, within ~1.5 texels of
        // the boundary in the tap's direction, diagonal included);
        // it is SELF-SHADOW ACNE exactly when the tap's texel reads
        // the GROUND itself (the receiver's own plane failing the
        // biased compare — the margin analysis: the 9-tap diagonal
        // reach costs at most ~0.0009 of depth against the 0.0015
        // constant bias). The count pinned at zero.
        let mut lit_interior = 0usize;
        let mut penumbral = 0usize;
        let mut self_tap_failures = 0usize;
        for gx in -38..=38 {
            for gz in -38..=38 {
                let point = [gx as f32, 0.0, gz as f32];
                let inside = boxes.iter().any(|b| {
                    b.min[0] < point[0]
                        && point[0] < b.max[0]
                        && b.min[2] < point[2]
                        && point[2] < b.max[2]
                });
                if inside || is_shadowed(point, &cascade, boxes) {
                    continue;
                }
                let factor = factor_at(point);
                if factor == 1.0 {
                    lit_interior += 1;
                    continue;
                }
                // The anchor (pcf_at_point's own projection,
                // re-derived for the tap classification).
                let clip: [f32; 4] = std::array::from_fn(|i| {
                    cascade.matrix[0][i] * point[0]
                        + cascade.matrix[1][i] * point[1]
                        + cascade.matrix[2][i] * point[2]
                        + cascade.matrix[3][i]
                });
                let ndc = [clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]];
                let extent_f = extent as f32;
                let fx = (ndc[0] + 1.0) * 0.5 * extent_f;
                let fy = (1.0 - (ndc[1] + 1.0) * 0.5) * extent_f;
                let texel_x = (fx as u32).min(extent - 1);
                let texel_y = (fy as u32).min(extent - 1);
                let facing = cascade.light[1].abs().max(1.0 / 64.0);
                let bias_value = bias.constant + bias.slope_scale * (1.0 / facing - 1.0);
                let mut failing_ground_taps = 0;
                for &(dx, dy) in PCF_OFFSETS.iter() {
                    let x = texel_x as i32 + dx;
                    let y = texel_y as i32 + dy;
                    if x < 0 || y < 0 || x >= extent as i32 || y >= extent as i32 {
                        continue; // the clear value passes by construction
                    }
                    let texel = map_texel_depth(&cascade, boxes, x as u32, y as u32, extent);
                    if ndc[2] > texel.depth + bias_value && texel.box_index.is_none() {
                        failing_ground_taps += 1;
                    }
                }
                if failing_ground_taps > 0 {
                    self_tap_failures += 1;
                } else {
                    penumbral += 1;
                }
            }
        }
        assert_eq!(
            self_tap_failures, 0,
            "no lit ground self-shadowed under the bias (failing GROUND taps)"
        );
        assert!(lit_interior > 200, "interior lit ground: {lit_interior}");
        assert!(
            penumbral > 0,
            "the lit side's penumbra is present: {penumbral}"
        );

        // Detachment: deep in the SW mass's umbra immediately off its
        // walls (0.3 m out — past the one-texel penumbra), the factor
        // is the FULL shadow on both wall faces; adjacent to the base
        // (0.05 m out) some shadow is already present (no fully-lit
        // band at the caster — a detached shadow would show one).
        for (x, z) in [(-5.7, -20.0), (-5.7, -12.0), (-20.0, -5.7), (-12.0, -5.7)] {
            let deep = [x, 0.0, z];
            assert!(
                is_shadowed(deep, &cascade, boxes),
                "the probe at ({x}, {z}) sits in the wall's umbra"
            );
            assert_eq!(factor_at(deep), 0.0, "full shadow at the caster's base");
        }
        let adjacent = [-5.95, 0.0, -16.0];
        assert!(is_shadowed(adjacent, &cascade, boxes));
        let adjacent_factor = factor_at(adjacent);
        assert!(
            adjacent_factor < 1.0,
            "shadow present adjacent to the caster's base: {adjacent_factor}"
        );

        // The flip walk (the gate fixture's own line, [0,0] toward
        // [12,3]): all three regimes exist at the real extent — deep
        // shadow, penumbra, and full light.
        let (ux, uz) = {
            let len = (12.0f32 * 12.0 + 3.0 * 3.0).sqrt();
            (12.0 / len, 3.0 / len)
        };
        let mut regimes = [false; 3];
        let mut steps = 0;
        while steps <= 1200 {
            let t = steps as f32 * 0.01;
            let point = [ux * t, 0.0, uz * t];
            if !(-40.0..=40.0).contains(&point[2]) {
                break;
            }
            let factor = factor_at(point);
            match factor {
                0.0 => regimes[0] = true,
                f if f > 0.0 && f < 1.0 => regimes[1] = true,
                _ => regimes[2] = true,
            }
            if regimes.iter().all(|&seen| seen) {
                break;
            }
            steps += 1;
        }
        assert!(
            regimes.iter().all(|&seen| seen),
            "deep shadow, penumbra, and full light all present along the walk: {regimes:?} ({steps} steps)"
        );
    }
}
