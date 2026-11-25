//! The planar-reflection deterministic core (the Era-1 scaffold —
//! see `docs/CYCLE6-ERA1-BRIEF.md`): the ground-plane mirror matrix
//! and the angle-based reflection blend. The wet-asphalt namesake:
//! the ground re-renders the scene mirrored below y = GROUND_Y into
//! an offscreen target, and the ground shader blends it by view
//! angle (grazing views reflect, overhead views stay matte).
//!
//! DETERMINISM CONTRACT: the mirror matrix is pure fixed-order
//! arithmetic; the blend is Schlick's approximation with the
//! five-multiplication pow5 (no `pow`, no libm — the repeated-
//! squaring discipline).

/// The ground-plane reflection matrix (column-major, the repo's
/// convention): maps (x, y, z) to (x, 2·plane_y − y, z) — the
/// mirrored world the reflection pass renders. The mirrored camera
/// = this matrix times the camera; the implementer also flips the
/// winding (the mirror inverts handedness — cull faces swap; the
/// brief documents the Vulkan-side `flipViewportY` alternative and
/// its equivalence).
pub fn ground_reflection_matrix(plane_y: f32) -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, -1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 2.0 * plane_y, 0.0, 1.0],
    ]
}

/// The reflection blend factor by view angle — Schlick's
/// approximation with the water-ish base reflectivity:
///   r(n_dot_v) = r0 + (1 − r0)(1 − n_dot_v)^5
/// with r0 = 0.02 (the fixed literal) and the fifth power by
/// repeated multiplication. `n_dot_v` is the clamped dot of the
/// surface normal and the unit view direction.
pub const REFLECTION_R0: f32 = 0.02;

/// The blend: matte at normal incidence (~r0), near-fully
/// reflective at grazing angles.
pub fn reflection_blend(n_dot_v: f32) -> f32 {
    let n = n_dot_v.clamp(0.0, 1.0);
    let one_minus = 1.0 - n;
    let pow5 = one_minus * one_minus * one_minus * one_minus * one_minus;
    REFLECTION_R0 + (1.0 - REFLECTION_R0) * pow5
}

/// THE RIPPLE ORACLE (the CPU mirror of ground_reflect.wgsl's
/// ripple perturbation, term-for-term — the lit_surface_color
/// discipline applied to the ripple field; the technique mined
/// from PR #54's `perturb` with credit). Given the SAME packed
/// uniform rows the shader reads (reflection_draw's
/// RippleUniformData) and a ground point (x, z), returns the
/// (uv_offset, roughen) the shader computes at that point. The
/// GPU/CPU sqrt may differ by ULPs — the known-answer gates
/// compare under the reflection gates' documented tolerance class,
/// never bit-exactly.
///
/// THE ROW CONTRACT (the shader's layout, term-for-term):
/// rows[0] = (live count, expansion speed m/s, band width m, uv
/// gain); rows[1] = (roughen gain, ..); rows[2 + i] = (center_x,
/// center_z, age s, current amplitude). A `rows` shorter than
/// 2 + count PANICS at the ring read — fail-visible, never a
/// silent wrong answer (the shader's pool bound makes the
/// well-formed case impossible to trip). The arithmetic mirrors
/// the WGSL in its exact order: the count gate, the pool-order
/// ring loop, `dist = sqrt(dx² + dz²)`, `radius = speed × age`,
/// `band = |dist − radius|` gated `band < width` (the bump's own
/// support), the signed slope `amp / width` (positive inside the
/// ring line, negative outside — the triangle gradient's
/// piecewise-constant stylization), the radial direction with the
/// `dist == 0` zero-contribution select, `uv_offset += dir ×
/// (signed × uv_gain)`, `roughen += amp × (1 − band / width)`.
/// Pure function, no allocation.
pub fn ripple_perturbation(rows: &[[f32; 4]], ground: [f32; 2]) -> ([f32; 2], f32) {
    let mut uv_offset = [0.0f32; 2];
    let mut roughen = 0.0f32;
    // The count gate (the shader's `if (ripple_count > 0)` — the
    // empty field's identity: the loop never runs, the zeros stand).
    let count = rows[0][0] as i32;
    if count > 0 {
        for i in 0..count as usize {
            let ring = rows[2 + i];
            let delta = [ground[0] - ring[0], ground[1] - ring[1]];
            let dist = (delta[0] * delta[0] + delta[1] * delta[1]).sqrt();
            let radius = rows[0][1] * ring[2];
            let band = (dist - radius).abs();
            let width = rows[0][2];
            if band < width {
                // The signed slope: +amp/width inside the ring line,
                // − outside (one subtraction and a sign — fixed
                // order, the shader's select).
                let slope = ring[3] / width;
                let signed = if dist > radius { -slope } else { slope };
                // The radial direction (dist == 0 has none: zero
                // contribution — the exact center of a ring).
                let dir = if dist > 0.0 {
                    [delta[0] / dist, delta[1] / dist]
                } else {
                    [0.0, 0.0]
                };
                let push = dir.map(|component| component * (signed * rows[0][3]));
                uv_offset = [uv_offset[0] + push[0], uv_offset[1] + push[1]];
                roughen += ring[3] * (1.0 - band / width);
            }
        }
    }
    (uv_offset, roughen)
}

/// The mirrored world's view-projection (package 7, the reflection
/// pass's own matrix): `view_proj × M` in the repo's column-major
/// convention — the reflection pass renders the MIRRORED world
/// (`M·v`) through the UNCHANGED camera, so every point ON the
/// ground plane lands at exactly its true clip position (the
/// mirror fixes the plane) and the ground's screen footprint
/// samples the mirrored scene behind it at the same texel — the
/// planar-reflection property the whole pass rests on. Fixed
/// operation order (the k-sum per element, k ascending).
pub fn mirrored_view_projection(view_proj: &[[f32; 4]; 4], plane_y: f32) -> [[f32; 4]; 4] {
    let mirror = ground_reflection_matrix(plane_y);
    std::array::from_fn(|column| {
        std::array::from_fn(|row| {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += view_proj[k][row] * mirror[column][k];
            }
            sum
        })
    })
}

/// The mirrored camera eye (the reflected viewpoint the reflection
/// pass's lighting math uses).
pub fn mirrored_eye(eye: [f32; 3], plane_y: f32) -> [f32; 3] {
    [eye[0], 2.0 * plane_y - eye[1], eye[2]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lit_scene::GROUND_Y;

    /// The mirror fixes the plane, flips height symmetrically, and
    /// is its own inverse (reflecting twice is the identity).
    #[test]
    fn mirror_fixes_plane_and_flips() {
        let m = ground_reflection_matrix(0.0);
        // Column-major: v' = sum(columns[i] * v[i]).
        let reflect = |v: [f32; 3]| -> [f32; 3] {
            [
                m[0][0] * v[0] + m[1][0] * v[1] + m[2][0] * v[2] + m[3][0],
                m[0][1] * v[0] + m[1][1] * v[1] + m[2][1] * v[2] + m[3][1],
                m[0][2] * v[0] + m[1][2] * v[1] + m[2][2] * v[2] + m[3][2],
            ]
        };
        // A point ON the plane is fixed.
        assert_eq!(reflect([3.0, 0.0, -7.0]), [3.0, 0.0, -7.0]);
        // A point above maps symmetrically below.
        assert_eq!(reflect([3.0, 5.0, -7.0]), [3.0, -5.0, -7.0]);
        // Twice is identity.
        let once = reflect([1.0, 2.5, 3.0]);
        assert_eq!(reflect(once), [1.0, 2.5, 3.0]);
        // The non-zero plane offset: y=GROUND_Y plane.
        let g = ground_reflection_matrix(GROUND_Y);
        assert_eq!(
            [
                g[0][0] * 0.0 + g[1][0] * GROUND_Y + g[2][0] * 0.0 + g[3][0],
                g[0][1] * 0.0 + g[1][1] * GROUND_Y + g[2][1] * 0.0 + g[3][1],
                g[0][2] * 0.0 + g[1][2] * GROUND_Y + g[2][2] * 0.0 + g[3][2],
            ],
            [0.0, GROUND_Y, 0.0],
            "GROUND_Y is the fixed plane"
        );
    }

    /// The blend anchors: exactly r0 at normal incidence, ~1 at
    /// grazing (the f32 sum r0 + (1−r0) lands one ulp below 1.0 —
    /// the tolerance documents it), monotone between, clamped
    /// outside [0,1].
    #[test]
    fn blend_anchors_and_monotone() {
        assert_eq!(reflection_blend(1.0), REFLECTION_R0);
        let grazing = reflection_blend(0.0);
        assert!(grazing > 0.95, "grazing near 1: {grazing}");
        let mut last = f32::INFINITY;
        for step in 0..=20 {
            let n = step as f32 / 20.0;
            let b = reflection_blend(n);
            // The blend DECREASES as n rises (grazing reflects, head-on
            // is matte) — the monotone direction of Schlick's curve.
            assert!(b <= last + 1e-7, "monotone at n={n}");
            assert!((REFLECTION_R0..=1.0 + 1e-6).contains(&b));
            last = b;
        }
        // Clamped inputs do not explode.
        assert_eq!(reflection_blend(-0.5), reflection_blend(0.0));
        assert_eq!(reflection_blend(1.5), REFLECTION_R0);
    }

    /// THE ORACLE KNOWN ANSWERS (hand-computed, the #54 `perturb`
    /// lesson folded in): one ring at the origin, the demo's
    /// authored policy (speed 0.8 m/s, width 0.35 m, amplitude 1.0,
    /// lifetime 2.5 s), aged 60 ticks — the age is the ACCUMULATED
    /// 60 x dt (the accumulation loop replicated, never 60*dt —
    /// f32 accumulation is the pinned form). The radius is
    /// speed x age; the packed row's amplitude is the fade-at-read
    /// expression. Probes: ON the ring line (the sign boundary —
    /// the gradient is MAXIMUM at the apex, the sign at the exact
    /// peak is ULP-arbitrary, so the FLIP is asserted either
    /// side), inside the band (positive x direction), outside the
    /// peak (negative), OUTSIDE the band (exactly zero — the gate),
    /// and the exact center (the direction select).
    #[test]
    fn ripple_oracle_known_answers() {
        // The accumulated 60-tick age (the replication loop).
        let dt = 1.0f32 / 60.0f32;
        let mut age = 0.0f32;
        for _ in 0..60 {
            age += dt;
        }
        // The packed rows exactly as the renderer's packing builds
        // them: rows[0] = (count, speed, width, uv gain),
        // rows[2] = (center_x, center_z, age, faded amplitude).
        let faded = 1.0f32 * (1.0f32 - age / 2.5f32);
        let mut rows = [[0.0f32; 4]; 2 + engine_core::ripple::RIPPLE_POOL_BOUND];
        rows[0] = [1.0, 0.8, 0.35, crate::renderer::RIPPLE_UV_GAIN];
        rows[1] = [crate::renderer::RIPPLE_ROUGHEN_GAIN, 0.0, 0.0, 0.0];
        rows[2] = [0.0, 0.0, age, faded];
        let radius = 0.8f32 * age;
        let width = 0.35f32;
        let slope = faded / width;
        let push = slope * crate::renderer::RIPPLE_UV_GAIN;

        // ON the ring line (the positive x axis, dist == radius):
        // the apex — the magnitude is the full slope x gain and the
        // roughen is the full faded amplitude (band == 0 up to the
        // sqrt's ULPs). The SIGN here is ULP-arbitrary (which side
        // of the apex dist lands on) — the flip is asserted below.
        let (uv, roughen) = ripple_perturbation(&rows, [radius, 0.0]);
        assert!(
            (uv[0].abs() - push).abs() < 1e-5 * push,
            "the apex carries the full gradient: uv {uv:?} vs push {push}"
        );
        assert!(uv[1].abs() < 1e-7, "the push is radial (x axis): {uv:?}");
        assert!(
            (roughen - faded).abs() < 1e-6 * faded,
            "the apex roughen is the faded amplitude: {roughen} vs {faded}"
        );

        // INSIDE the band (dist < radius, positive x direction):
        // the signed slope is POSITIVE — the push points away from
        // the center (+x).
        let (uv, roughen) = ripple_perturbation(&rows, [radius - width * 0.5, 0.0]);
        assert!(uv[0] > 0.0, "inside the ring line the push is +x: {uv:?}");
        assert!(
            (uv[0].abs() - push).abs() < 1e-5 * push,
            "the triangle gradient is piecewise-constant: {uv:?} vs push {push}"
        );
        assert!(uv[1].abs() < 1e-7, "radial on the x axis: {uv:?}");
        assert!(
            roughen > 0.0 && roughen < faded,
            "inside the band the coverage is a positive fraction: {roughen}"
        );

        // OUTSIDE the peak (dist > radius, still inside the band):
        // the signed slope flips NEGATIVE — the push points back
        // toward the center (-x). THE #54 LESSON: the gradient is
        // MAXIMUM at the apex and its sign at the exact peak is
        // ULP-arbitrary — the flip is proven EITHER SIDE, never at
        // the boundary itself.
        let (uv, _) = ripple_perturbation(&rows, [radius + width * 0.5, 0.0]);
        assert!(uv[0] < 0.0, "outside the ring line the push is -x: {uv:?}");
        assert!(
            (uv[0].abs() - push).abs() < 1e-5 * push,
            "the same magnitude, the opposite sign: {uv:?} vs push {push}"
        );

        // OUTSIDE the band: the gate `band < width` fails — the
        // contribution is EXACTLY zero (no arithmetic runs).
        let (uv, roughen) = ripple_perturbation(&rows, [radius + width + 0.1, 0.0]);
        assert_eq!(uv, [0.0, 0.0], "outside the band: the exact gate zero");
        assert_eq!(roughen, 0.0, "outside the band: no coverage");

        // THE EXACT CENTER (dist == 0): the mature ring's radius
        // (≈ 0.79 m) exceeds the width, so the gate itself zeroes
        // this point.
        let (uv, roughen) = ripple_perturbation(&rows, [0.0, 0.0]);
        assert_eq!(
            uv,
            [0.0, 0.0],
            "the center: the gate's zero (radius > width)"
        );
        assert_eq!(roughen, 0.0);

        // THE DIRECTION SELECT's own proof (the gate OPEN at the
        // center): a YOUNG ring — 2 accumulated ticks, radius ≈
        // 0.027 m < width — whose band covers the center. The push
        // would be maximal there (the inside slope); the
        // `dist == 0` select zeroes the direction, so the uv
        // contribution is EXACTLY [0, 0] while the roughen
        // (direction-free) stays positive.
        let mut young_age = 0.0f32;
        for _ in 0..2 {
            young_age += dt;
        }
        let young_faded = 1.0f32 * (1.0f32 - young_age / 2.5f32);
        let mut young_rows = [[0.0f32; 4]; 2 + engine_core::ripple::RIPPLE_POOL_BOUND];
        young_rows[0] = rows[0];
        young_rows[2] = [0.0, 0.0, young_age, young_faded];
        let young_radius = 0.8f32 * young_age;
        assert!(
            young_radius < width,
            "the young ring's band covers its own center"
        );
        let (uv, roughen) = ripple_perturbation(&young_rows, [0.0, 0.0]);
        assert_eq!(
            uv,
            [0.0, 0.0],
            "dist == 0 has no radial direction: the select's exact zero"
        );
        let expected_roughen = young_faded * (1.0f32 - young_radius / width);
        assert!(
            roughen > 0.0 && (roughen - expected_roughen).abs() < 1e-6 * expected_roughen,
            "the coverage is direction-free and positive at the center: {roughen} vs \
             {expected_roughen}"
        );
    }

    /// THE IDENTITY GATE's CPU HALF: the empty field (count 0) is a
    /// BIT-IDENTICAL no-op — the oracle returns exactly
    /// ([0.0, 0.0], 0.0) for every ground point, both for the
    /// all-zero rows (the never-set uniform) and the realistic
    /// rainless rows (a live policy's params with count 0 — every
    /// ring expired; the loop never runs, the params are never
    /// read).
    #[test]
    fn empty_field_is_the_bit_identity() {
        let zero_rows = [[0.0f32; 4]; 2 + engine_core::ripple::RIPPLE_POOL_BOUND];
        let mut param_rows = [[0.0f32; 4]; 2 + engine_core::ripple::RIPPLE_POOL_BOUND];
        param_rows[0] = [0.0, 0.8, 0.35, crate::renderer::RIPPLE_UV_GAIN];
        param_rows[1] = [crate::renderer::RIPPLE_ROUGHEN_GAIN, 0.0, 0.0, 0.0];
        for ground in [
            [0.0, 0.0],
            [12.5, -7.25],
            [-40.0, 33.0],
            [0.001, 999.0],
            [1234.5, -1234.5],
        ] {
            assert_eq!(
                ripple_perturbation(&zero_rows, ground),
                ([0.0, 0.0], 0.0),
                "the all-zero rows are the exact identity at {ground:?}"
            );
            assert_eq!(
                ripple_perturbation(&param_rows, ground),
                ([0.0, 0.0], 0.0),
                "count 0 skips the loop regardless of the params at {ground:?}"
            );
        }
    }

    /// THE PLANAR PROPERTY (what the reflection pass rests on): the
    /// mirrored view-projection maps every point ON the ground plane
    /// to exactly the clip position the true view-projection gives
    /// it (the mirror fixes the plane — bit-equal, since M·P = P
    /// exactly there and the composition applies M first). Points
    /// ABOVE the plane map where their mirror images belong.
    #[test]
    fn mirrored_view_projection_fixes_the_ground_plane() {
        use crate::lit_scene::GROUND_Y;
        // A representative view-projection (the corner route's own
        // shape — any nonsingular matrix exercises the composition).
        let perspective = |aspect: f32| -> [[f32; 4]; 4] {
            let fov = 65_f32.to_radians();
            let (near, far) = (0.1f32, 150.0f32);
            let focal = 1.0 / (fov / 2.0).tan();
            let mut matrix = [[0.0f32; 4]; 4];
            matrix[0][0] = focal / aspect;
            matrix[1][1] = focal;
            matrix[2][2] = far / (near - far);
            matrix[2][3] = -1.0;
            matrix[3][2] = near * far / (near - far);
            matrix
        };
        // A rigid view (yaw only — any orthonormal view works; the
        // column-major storage of Rᵀ·T(−position)).
        let (sy, cy) = (0.6f32.sin(), 0.6f32.cos());
        let position = [12.0, 6.0, -9.0];
        let mut view = [
            [cy, 0.0, -sy, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [sy, 0.0, cy, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        for view_column in view.iter_mut().take(3) {
            view_column[3] = -(view_column[0] * position[0]
                + view_column[1] * position[1]
                + view_column[2] * position[2]);
        }
        let mul = |a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]| -> [[f32; 4]; 4] {
            std::array::from_fn(|column| {
                std::array::from_fn(|row| {
                    let mut sum = 0.0;
                    for k in 0..4 {
                        sum += a[k][row] * b[column][k];
                    }
                    sum
                })
            })
        };
        let view_proj = mul(&perspective(1.0), &view);
        let mirrored = mirrored_view_projection(&view_proj, GROUND_Y);
        let clip = |m: &[[f32; 4]; 4], p: [f32; 3]| -> [f32; 4] {
            std::array::from_fn(|i| m[0][i] * p[0] + m[1][i] * p[1] + m[2][i] * p[2] + m[3][i])
        };
        // Ground points: bit-equal clip positions.
        for ground_point in [[3.0, 0.0, -7.0], [-20.5, 0.0, 12.25], [0.0, 0.0, 0.0]] {
            assert_eq!(
                clip(&mirrored, ground_point),
                clip(&view_proj, ground_point),
                "the ground point {ground_point:?} is fixed"
            );
        }
        // A point above the plane maps where its mirror image
        // belongs (the mirror of P under the true matrix).
        let above = [3.0, 5.0, -7.0];
        let mirrored_point = [3.0, -5.0, -7.0];
        assert_eq!(clip(&mirrored, above), clip(&view_proj, mirrored_point));
        // The mirrored eye composition: the reflected viewpoint.
        assert_eq!(mirrored_eye(position, GROUND_Y), [12.0, -6.0, -9.0]);
        assert_eq!(
            mirrored_eye([0.0, 1.7, 0.0], 4.0),
            [0.0, 6.3, 0.0],
            "the non-zero plane offset"
        );
    }
}
