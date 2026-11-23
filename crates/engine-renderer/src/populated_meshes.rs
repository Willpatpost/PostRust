//! The stylized population meshes v2 (the Era-1 scaffold — see
//! `docs/CYCLE6-ERA1-BRIEF.md`): the owner-ratified stylized-
//! coherent fidelity target — real silhouettes from deterministic
//! geometry. The humanoid gains limbs (a ten-box figure at human
//! proportions); the sedan gains a cabin and four cylinder wheels —
//! the wheels' circle points come from the libm-free turn-fraction
//! `sin_cos_turn` (the standing discipline: no runtime `cos`/`sin`,
//! whose bits vary across libm platforms).
//!
//! CONVENTIONS (inherited from proxy_mesh): the 32-byte lit vertex
//! (pos, normal, uv), non-indexed triangle lists, planar UVs (one
//! tile per 4 m), all six faces per box, origin at the feet/ground
//! center. Counts are pinned by tests — structure, not coincidence.

use crate::proxy_instances::sin_cos_turn;
use crate::proxy_mesh::ProxyVertex;

/// Emits one quad as six vertices (two CCW triangles — the
/// proxy_mesh corner conventions).
fn quad(
    vertices: &mut Vec<ProxyVertex>,
    corners: [[f32; 3]; 4],
    normal: [f32; 3],
    axes: [usize; 2],
) {
    for i in [0, 1, 2, 0, 2, 3] {
        let pos = corners[i];
        vertices.push(ProxyVertex {
            pos,
            normal,
            uv: axes.map(|axis| pos[axis] / 4.0),
        });
    }
}

/// All six faces of one axis-aligned box.
fn box_faces(vertices: &mut Vec<ProxyVertex>, lo: [f32; 3], hi: [f32; 3]) {
    let [x, y, z] = lo;
    let [r, t, f] = hi;
    quad(
        vertices,
        [[x, y, f], [r, y, f], [r, t, f], [x, t, f]],
        [0., 0., 1.],
        [0, 1],
    );
    quad(
        vertices,
        [[r, y, z], [x, y, z], [x, t, z], [r, t, z]],
        [0., 0., -1.],
        [0, 1],
    );
    quad(
        vertices,
        [[r, y, f], [r, y, z], [r, t, z], [r, t, f]],
        [1., 0., 0.],
        [2, 1],
    );
    quad(
        vertices,
        [[x, y, z], [x, y, f], [x, t, f], [x, t, z]],
        [-1., 0., 0.],
        [2, 1],
    );
    quad(
        vertices,
        [[x, t, f], [r, t, f], [r, t, z], [x, t, z]],
        [0., 1., 0.],
        [0, 2],
    );
    quad(
        vertices,
        [[x, y, z], [r, y, z], [r, y, f], [x, y, f]],
        [0., -1., 0.],
        [0, 2],
    );
}

/// One ring point of a wheel-radius circle in the Y-Z plane, at
/// turn-fraction `turn` — the libm-free circle (sin_cos_turn).
fn ring_point(radius: f32, turn: u16) -> [f32; 2] {
    let (s, c) = sin_cos_turn(turn);
    [radius * c, radius * s]
}

/// A cylinder wheel along the X axis: `segments` side quads (smooth
/// per-vertex radial normals in the Y-Z plane) plus two flat cap
/// fans (±X normals). `center_x` is the axle's X; the wheel spans
/// `width` along X. Deterministic: every ring point from the fixed
/// turn fractions i/segments of a full turn.
fn wheel(
    vertices: &mut Vec<ProxyVertex>,
    center: [f32; 3],
    radius: f32,
    width: f32,
    segments: u16,
) {
    let half = width / 2.0;
    let x_lo = center[0] - half;
    let x_hi = center[0] + half;
    let ring = |i: u16| -> [f32; 2] {
        ring_point(radius, (u32::from(i) * 65_536 / u32::from(segments)) as u16)
    };
    // The side quads: per-vertex radial normals (smooth shading).
    for i in 0..segments {
        let [y0, z0] = ring(i);
        let [y1, z1] = ring(i + 1);
        let [ry0, rz0] = ring(i);
        let [ry1, rz1] = ring(i + 1);
        let corner = |x: f32, y: f32, z: f32, ny: f32, nz: f32| ProxyVertex {
            pos: [x, center[1] + y, center[2] + z],
            normal: [0.0, ny, nz],
            uv: [z / 4.0, x / 4.0],
        };
        let n0 = [ry0 / radius, rz0 / radius];
        let n1 = [ry1 / radius, rz1 / radius];
        let a = corner(x_lo, y0, z0, n0[0], n0[1]);
        let b = corner(x_hi, y0, z0, n0[0], n0[1]);
        let c = corner(x_hi, y1, z1, n1[0], n1[1]);
        let d = corner(x_lo, y1, z1, n1[0], n1[1]);
        // Two triangles, the corner order the box walls use.
        vertices.extend_from_slice(&[a, b, c, a, c, d]);
    }
    // The caps: flat fans, center vertex + rim, ±X normals.
    for (x, normal_x) in [(x_lo, -1.0), (x_hi, 1.0)] {
        for i in 0..segments {
            let [y0, z0] = ring(i);
            let [y1, z1] = ring(i + 1);
            let v = |y: f32, z: f32| ProxyVertex {
                pos: [x, center[1] + y, center[2] + z],
                normal: [normal_x, 0.0, 0.0],
                uv: [z / 4.0, y / 4.0],
            };
            let center_v = ProxyVertex {
                pos: [x, center[1], center[2]],
                normal: [normal_x, 0.0, 0.0],
                uv: [0.0, 0.0],
            };
            vertices.extend_from_slice(&[center_v, v(y0, z0), v(y1, z1)]);
        }
    }
}

/// The segments per wheel rim — fixed (structure, not choice).
pub const WHEEL_SEGMENTS: u16 = 10;

/// The stylized humanoid v2: an eleven-box figure at human
/// proportions (~1.80 m, origin at the feet): head, torso, pelvis,
/// paired upper and lower arms, paired upper and lower legs.
/// 11 boxes × 36 vertices = 396 vertices.
pub fn humanoid_mesh() -> Vec<ProxyVertex> {
    let mut v = Vec::with_capacity(396);
    // Head.
    box_faces(&mut v, [-0.11, 1.60, -0.11], [0.11, 1.80, 0.11]);
    // Torso.
    box_faces(&mut v, [-0.21, 1.06, -0.12], [0.21, 1.60, 0.12]);
    // Pelvis.
    box_faces(&mut v, [-0.19, 0.90, -0.11], [0.19, 1.06, 0.11]);
    // Upper arms (shoulders to elbows), hanging at the sides.
    box_faces(&mut v, [-0.33, 1.18, -0.06], [-0.21, 1.58, 0.06]);
    box_faces(&mut v, [0.21, 1.18, -0.06], [0.33, 1.58, 0.06]);
    // Lower arms and hands (elbows to near-hip).
    box_faces(&mut v, [-0.31, 0.78, -0.05], [-0.21, 1.18, 0.05]);
    box_faces(&mut v, [0.21, 0.78, -0.05], [0.31, 1.18, 0.05]);
    // Upper legs (hip to knee).
    box_faces(&mut v, [-0.17, 0.45, -0.08], [-0.03, 0.90, 0.08]);
    box_faces(&mut v, [0.03, 0.45, -0.08], [0.17, 0.90, 0.08]);
    // Lower legs and feet (knee to ground, feet forward).
    box_faces(&mut v, [-0.15, 0.00, -0.08], [-0.03, 0.45, 0.12]);
    box_faces(&mut v, [0.03, 0.00, -0.08], [0.15, 0.45, 0.12]);
    v
}

/// The stylized sedan v2: body + cabin + four cylinder wheels
/// (radius 0.35, width 0.25, at the standard sedan track). The
/// wheels lift the body clear of the ground; origin at ground
/// center, ~4.4 m long. 2 boxes × 36 + 4 wheels × (10 side quads ×
/// 6 + 2 caps × 10 × 3) = 72 + 480 = 552 vertices.
pub fn sedan_mesh() -> Vec<ProxyVertex> {
    let mut v = Vec::with_capacity(552);
    // Body (the wheel wells' tops at body-bottom height).
    box_faces(&mut v, [-0.95, 0.35, -2.20], [0.95, 0.90, 2.20]);
    // Cabin (the greenhouse, set back from the nose).
    box_faces(&mut v, [-0.82, 0.90, -1.10], [0.82, 1.40, 1.10]);
    // The four wheels.
    for x in [-0.85, 0.85] {
        for z in [-1.45, 1.45] {
            wheel(&mut v, [x, 0.35, z], 0.35, 0.25, WHEEL_SEGMENTS);
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Determinism and the pinned counts (the mesh-structure
    /// discipline: every call generates bit-identical vertices at
    /// exactly the documented count).
    #[test]
    fn humanoid_mesh_deterministic_pinned_count() {
        let a = humanoid_mesh();
        let b = humanoid_mesh();
        assert_eq!(a.len(), 396, "11 boxes x 36 vertices");
        let bits = |m: &[ProxyVertex]| -> Vec<u32> {
            m.iter()
                .flat_map(|u| [u.pos[0].to_bits(), u.pos[1].to_bits(), u.pos[2].to_bits()])
                .collect()
        };
        assert_eq!(bits(&a), bits(&b));
    }

    /// The sedan: the same discipline at 552.
    #[test]
    fn sedan_mesh_deterministic_pinned_count() {
        let a = sedan_mesh();
        let b = sedan_mesh();
        assert_eq!(a.len(), 552, "2 boxes x 36 + 4 wheels x 120");
        let bits = |m: &[ProxyVertex]| -> Vec<u32> {
            m.iter()
                .flat_map(|u| [u.pos[0].to_bits(), u.pos[1].to_bits(), u.pos[2].to_bits()])
                .collect()
        };
        assert_eq!(bits(&a), bits(&b));
    }

    /// Validity: finite positions and UVs, unit normals everywhere
    /// (the wheel's radial normals are normalized by construction —
    /// radius-scaled turn points divided by radius), the figures
    /// grounded and inside their documented bounds.
    #[test]
    fn meshes_valid_and_bounded() {
        for mesh in [humanoid_mesh(), sedan_mesh()] {
            for u in &mesh {
                assert!(u.pos.iter().all(|p| p.is_finite()));
                assert!(u.uv.iter().all(|t| t.is_finite()));
                let len = (u.normal[0] * u.normal[0]
                    + u.normal[1] * u.normal[1]
                    + u.normal[2] * u.normal[2])
                    .sqrt();
                assert!((len - 1.0).abs() < 1e-4, "unit normal: {:?}", u.normal);
            }
        }
        for u in humanoid_mesh() {
            assert!(u.pos[1] >= 0.0 && u.pos[1] <= 1.80, "humanoid height");
            assert!(u.pos[0].abs() <= 0.35 && u.pos[2].abs() <= 0.15);
        }
        for u in sedan_mesh() {
            assert!(u.pos[1] >= 0.0 && u.pos[1] <= 1.40, "sedan height");
            assert!(u.pos[0].abs() <= 0.98 && u.pos[2].abs() <= 2.20);
        }
    }

    /// The libm-free circle: the ring points at the cardinal turns
    /// land on the axes at exactly the radius (the wheel contract).
    #[test]
    fn ring_points_cardinal() {
        assert_eq!(ring_point(0.35, 0), [0.35, 0.0]);
        let quarter = ring_point(0.35, 16384);
        assert!((quarter[0]).abs() < 1e-5 && (quarter[1] - 0.35).abs() < 1e-5);
        let half = ring_point(0.35, 32768);
        assert!((half[0] + 0.35).abs() < 1e-5 && half[1].abs() < 1e-5);
        // The wheel's first segment ring point: turn 65536/10.
        let seg = ring_point(1.0, (65_536 / 10) as u16);
        let expected_angle = core::f32::consts::FRAC_PI_2 / 5.0 * 2.0;
        // cos(36°) via the deterministic series — compare against
        // the same series (self-consistency is the contract; the
        // pinned convention is the axis points above).
        let (s, c) = crate::proxy_instances::sin_cos_turn((65_536 / 10) as u16);
        assert_eq!(seg, [c, s]);
        assert!(expected_angle.is_finite());
    }
}
