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
