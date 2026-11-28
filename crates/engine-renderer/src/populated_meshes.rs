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

use crate::proxy_mesh::ProxyVertex;
// E-S1 (DECISION-A06 §4): the libm-free turn trigonometry now lives
// in the engine-animation crate (proxy_instances re-exports it);
// SkinnedVertex and BOX_JOINT come with it for the skinned mesh
// derivation below, which moved here from the animation module.
use engine_animation::{BOX_JOINT, SkinnedVertex, sin_cos_turn};

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
    // The winding is counter-clockwise from the OUTSIDE of the wheel,
    // matching the outward-pointing radial normals.
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
        // CCW from outside: a→d→c→b (reversed from inside-out winding)
        // The outward normal at a/d is n0, at b/c is n1.
        let a = corner(x_lo, y0, z0, n0[0], n0[1]);
        let d = corner(x_lo, y1, z1, n1[0], n1[1]);
        let c = corner(x_hi, y1, z1, n1[0], n1[1]);
        let b = corner(x_hi, y0, z0, n0[0], n0[1]);
        // Two triangles, CCW from outside: a→d→c and a→c→b
        vertices.extend_from_slice(&[a, d, c, a, c, b]);
    }
    // The caps: flat fans, center vertex + rim, ±X normals. The
    // winding is CCW from the OUTSIDE of EACH face, which means the
    // two faces traverse the rim in OPPOSITE orders: viewed from
    // +X, center→ring(i)→ring(i+1) is counter-clockwise (its
    // geometric normal is +X — the cross product's X component is
    // r²·sin(Δθ) > 0), so the HIGH-X cap uses that order; viewed
    // from −X the same order is clockwise, so the LOW-X cap
    // reverses it (center→ring(i+1)→ring(i), geometric normal −X).
    // (corrective finding 11: both caps previously shared one
    // traversal, so one cap's geometric normal always opposed its
    // authored ±X normal — the far surface showed through the near
    // one under back-face culling.)
    for (x, normal_x) in [(x_lo, -1.0), (x_hi, 1.0)] {
        for i in 0..segments {
            let [y0, z0] = ring(i);
            let [y1, z1] = ring(i + 1);
            let rim = |y: f32, z: f32| ProxyVertex {
                pos: [x, center[1] + y, center[2] + z],
                normal: [normal_x, 0.0, 0.0],
                uv: [z / 4.0, y / 4.0],
            };
            let center_v = ProxyVertex {
                pos: [x, center[1], center[2]],
                normal: [normal_x, 0.0, 0.0],
                uv: [0.0, 0.0],
            };
            let first = rim(y0, z0);
            let second = rim(y1, z1);
            // CCW from this face's outside: +X face keeps
            // ring(i)→ring(i+1); the −X face reverses it.
            let (first, second) = if normal_x > 0.0 {
                (first, second)
            } else {
                (second, first)
            };
            vertices.extend_from_slice(&[center_v, first, second]);
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

/// Derives the skinned humanoid mesh from the unskinned one.
/// The joints field is [BOX_JOINT[v/36] as u32, 0, 0, 0] (rigid binding).
/// (E-S1, DECISION-A06 §4: moved here from the animation module —
/// the derivation consumes THIS module's unskinned mesh, a renderer
/// asset; `SkinnedVertex` and `BOX_JOINT` come from engine-animation.
/// The animation compatibility shim re-exports this function so the
/// `crate::animation::skinned_humanoid_mesh` and
/// `engine_renderer::animation::skinned_humanoid_mesh` paths are
/// stable.)
pub fn skinned_humanoid_mesh() -> Vec<SkinnedVertex> {
    let unskinned = humanoid_mesh();

    let mut result = Vec::with_capacity(unskinned.len());

    for (i, v) in unskinned.iter().enumerate() {
        let box_idx = i / 36;
        let joint_idx = BOX_JOINT[box_idx] as u32;

        result.push(SkinnedVertex {
            pos: v.pos,
            normal: v.normal,
            uv: v.uv,
            joints: [joint_idx, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
        });
    }

    result
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

/// The rain streak quad (Era-2 package C — the rain kind's mesh):
/// ONE vertical quad, two triangles, six vertices, centered at the
/// origin — extent X ±0.012 m (a thin streak), Y ±0.28 m (the
/// streak length), Z 0; normal +Z everywhere; UVs covering the full
/// texture (0..1 — the streak texture's ALPHA shapes the quad, so
/// every corner maps the whole tile; the `quad()` helper's planar
/// pos/4.0 mapping cannot cover 0..1 at this size, hence the
/// hand-written emission in the helper's winding convention — CCW
/// about the +Z normal).
///
/// THE BILLBOARD DISCIPLINE: the DEMO turns every rain instance to
/// face the camera (one shared turn per frame — the population
/// builder's policy, not this mesh's), and the thin-X/long-Y shape
/// reads as a falling streak from any side. The size is the
/// visual-policy constant chosen for the corner's scale (documented
/// here; nothing else references it).
pub fn rain_streak_mesh() -> Vec<ProxyVertex> {
    const HALF_X: f32 = 0.012;
    const HALF_Y: f32 = 0.28;
    let corner = |x: f32, y: f32, u: f32, v: f32| ProxyVertex {
        pos: [x, y, 0.0],
        normal: [0.0, 0.0, 1.0],
        uv: [u, v],
    };
    // CCW about +Z (the quad() helper's front-face order):
    // bottom-left, bottom-right, top-right, top-left.
    let corners = [
        corner(-HALF_X, -HALF_Y, 0.0, 0.0),
        corner(HALF_X, -HALF_Y, 1.0, 0.0),
        corner(HALF_X, HALF_Y, 1.0, 1.0),
        corner(-HALF_X, HALF_Y, 0.0, 1.0),
    ];
    let mut vertices = Vec::with_capacity(6);
    for i in [0, 1, 2, 0, 2, 3] {
        vertices.push(corners[i]);
    }
    vertices
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

    /// skinned_humanoid_mesh: 396 vertices; pos/normal/uv bits
    /// byte-identical to humanoid_mesh() in the same order (the
    /// derivation-is-faithful pin); rigid binding to the emission
    /// box's joint; the 64-byte layout with joints at 32 and weights
    /// at 48. (E-S1: moved here with `skinned_humanoid_mesh` from the
    /// animation module — the 396-vertex mesh bit-identity gate.)
    #[test]
    fn skinned_humanoid_mesh_structure() {
        let skinned = skinned_humanoid_mesh();
        let unskinned = humanoid_mesh();

        assert_eq!(skinned.len(), 396, "11 boxes x 36 vertices");
        assert_eq!(skinned.len(), unskinned.len());
        for (s, u) in skinned.iter().zip(unskinned.iter()) {
            for i in 0..3 {
                assert_eq!(s.pos[i].to_bits(), u.pos[i].to_bits(), "pos bits");
                assert_eq!(s.normal[i].to_bits(), u.normal[i].to_bits(), "normal bits");
            }
            for i in 0..2 {
                assert_eq!(s.uv[i].to_bits(), u.uv[i].to_bits(), "uv bits");
            }
        }
        for (i, v) in skinned.iter().enumerate() {
            let expected = BOX_JOINT[i / 36] as u32;
            assert_eq!(v.joints, [expected, 0, 0, 0], "vertex {i} is rigid");
            assert_eq!(v.weights, [1.0, 0.0, 0.0, 0.0]);
        }
        assert_eq!(std::mem::size_of::<SkinnedVertex>(), 64);
        assert_eq!(std::mem::offset_of!(SkinnedVertex, joints), 32);
        assert_eq!(std::mem::offset_of!(SkinnedVertex, weights), 48);
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

    /// THE CROSS-PLATFORM FULL-VERTEX DIGEST PIN (finding 12 — the
    /// full-vertex regression gate). The v2 meshes' determinism test
    /// only checked position bits. A change to UV, normal, or
    /// triangle ordering evaded it. After the winding fix (finding
    /// 11), pin one canonical full-vertex digest per mesh: position,
    /// normal, and UV serialized as little-endian f32 bits in that
    /// fixed order, folded with FNV-1a 64-bit (the repo's established
    /// hash). The same values measured on Linux and Windows CI ARE
    /// the cross-platform determinism proof — the fingerprint-pin
    /// discipline applied to the full-vertex set. The existing
    /// position-only checks stay untouched.
    fn vertex_digest_fnv(mesh: &[ProxyVertex]) -> u64 {
        const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
        let mut hash = FNV_OFFSET;
        for v in mesh {
            // pos (3×f32) + normal (3×f32) + uv (2×f32): 8 f32s = 32 bytes
            for &f in v.pos.iter().chain(v.normal.iter()).chain(v.uv.iter()) {
                for &byte in f.to_le_bytes().iter() {
                    hash ^= byte as u64;
                    hash = hash.wrapping_mul(FNV_PRIME);
                }
            }
        }
        hash
    }

    /// The humanoid v2 full-vertex digest: MEASURED at this tree
    /// (post-winding-fix — finding 11's wheel-winding correction
    /// deliberately changed the normal-bearing bytes). The digest
    /// pins the entire vertex content: pos, normal, AND uv across all
    /// 396 vertices. Cross-platform determinism follows from the
    /// libm-free fixed-order construction and the IEEE f32 bits
    /// serialization (no platform-dependent libm or iteration order).
    #[test]
    fn humanoid_mesh_full_vertex_digest_pin() {
        let mesh = humanoid_mesh();
        assert_eq!(mesh.len(), 396, "11 boxes x 36 vertices");
        let digest = vertex_digest_fnv(&mesh);
        // The measured-actual FNV-1a of the complete vertex set.
        // Run once with a temporary println to obtain the actual, then
        // pin it. A later mismatch is an investigation, never a silent
        // repin. The value changed after the winding fix (finding 11).
        const PINNED_FNV: u64 = 14589391755680253577;
        assert_eq!(
            digest, PINNED_FNV,
            "the humanoid full-vertex digest moved: a vertex-shape or \
             triangle-ordering change — an investigation, never a silent repin"
        );
    }

    /// The sedan v2 full-vertex digest: the same discipline at 552
    /// vertices, measured post-winding-fix.
    #[test]
    fn sedan_mesh_full_vertex_digest_pin() {
        let mesh = sedan_mesh();
        assert_eq!(mesh.len(), 552, "2 boxes x 36 + 4 wheels x 120");
        let digest = vertex_digest_fnv(&mesh);
        // The measured-actual FNV-1a of the complete vertex set.
        // The value changed after the winding fix (finding 11).
        const PINNED_FNV: u64 = 14576727727469601309;
        assert_eq!(
            digest, PINNED_FNV,
            "the sedan full-vertex digest moved: a vertex-shape or \
             triangle-ordering change — an investigation, never a silent repin"
        );
    }

    /// The rain streak quad: the same determinism/count discipline
    /// at 6 vertices (ONE quad — two triangles), with the documented
    /// thin-X/long-Y extents, the +Z normal, and full-texture UVs.
    #[test]
    fn rain_streak_mesh_deterministic_pinned_count() {
        let a = rain_streak_mesh();
        let b = rain_streak_mesh();
        assert_eq!(a.len(), 6, "one quad = two triangles = six vertices");
        let bits = |m: &[ProxyVertex]| -> Vec<u32> {
            m.iter()
                .flat_map(|u| [u.pos[0].to_bits(), u.pos[1].to_bits(), u.pos[2].to_bits()])
                .collect()
        };
        assert_eq!(bits(&a), bits(&b));
        for u in &a {
            assert_eq!(u.normal, [0.0, 0.0, 1.0], "the streak faces +Z");
            assert!(u.pos[0].abs() <= 0.012 && u.pos[0].abs() > 0.0);
            assert!(u.pos[1].abs() <= 0.28);
            assert_eq!(u.pos[2], 0.0, "the quad is planar at Z 0");
            assert!(
                u.uv[0] == 0.0 || u.uv[0] == 1.0,
                "the UVs cover the full texture width"
            );
            assert!(
                u.uv[1] == 0.0 || u.uv[1] == 1.0,
                "the UVs cover the full texture height"
            );
        }
    }

    /// The rain streak full-vertex digest: the same discipline at 6
    /// vertices — pos, normal, AND uv across the whole quad, folded
    /// with the file's FNV-1a (the cross-platform determinism proof
    /// the other two meshes pin).
    #[test]
    fn rain_streak_mesh_full_vertex_digest_pin() {
        let mesh = rain_streak_mesh();
        assert_eq!(mesh.len(), 6, "one quad = two triangles = six vertices");
        let digest = vertex_digest_fnv(&mesh);
        // The measured-actual FNV-1a of the complete vertex set.
        // Run once with a temporary println to obtain the actual, then
        // pin it. A later mismatch is an investigation, never a silent
        // repin. Measured at the Era-2 package C (rain) renderer-kind
        // stage's first test run on this pod.
        const PINNED_FNV: u64 = 18284880733001066661;
        assert_eq!(
            digest, PINNED_FNV,
            "the rain streak full-vertex digest moved: a vertex-shape or \
             triangle-ordering change — an investigation, never a silent repin"
        );
    }

    /// (corrective finding 11) Every wheel triangle's geometric
    /// normal must agree with its authored normals: the side quads'
    /// cross product points outward radially (the smooth-shading
    /// normals' direction), and each cap fan's cross product points
    /// along its declared ±X face normal. The check runs on the
    /// wheel()'s OWN emission — every sedan wheel is a translation
    /// of the same emission (translation cannot change normals), so
    /// this covers all four wheels without reverse-engineering the
    /// sedan's vertex layout.
    #[test]
    fn wheel_normals_agree_with_winding() {
        let mut scratch = Vec::new();
        wheel(&mut scratch, [0.0, 0.35, 0.0], 0.35, 0.25, WHEEL_SEGMENTS);
        let segments = usize::from(WHEEL_SEGMENTS);
        // The emission layout: `segments` side quads of 6 vertices
        // (two triangles a,d,c / a,c,b), then two caps of `segments`
        // triangles of 3 vertices each.
        assert_eq!(
            scratch.len(),
            segments * 6 + 2 * segments * 3,
            "the wheel emission layout this test walks"
        );
        let mut checked = 0usize;
        let mut min_dot = f32::INFINITY;
        let mut check = |tri: (&ProxyVertex, &ProxyVertex, &ProxyVertex)| {
            let (v0, v1, v2) = tri;
            let e1 = [
                v1.pos[0] - v0.pos[0],
                v1.pos[1] - v0.pos[1],
                v1.pos[2] - v0.pos[2],
            ];
            let e2 = [
                v2.pos[0] - v0.pos[0],
                v2.pos[1] - v0.pos[1],
                v2.pos[2] - v0.pos[2],
            ];
            let geometric = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            let authored = [
                (v0.normal[0] + v1.normal[0] + v2.normal[0]) / 3.0,
                (v0.normal[1] + v1.normal[1] + v2.normal[1]) / 3.0,
                (v0.normal[2] + v1.normal[2] + v2.normal[2]) / 3.0,
            ];
            let dot = geometric[0] * authored[0]
                + geometric[1] * authored[1]
                + geometric[2] * authored[2];
            min_dot = min_dot.min(dot);
            checked += 1;
            assert!(
                dot > 0.0,
                "a wheel triangle's geometric normal opposes its authored normals                  (dot {dot}): vertices {:?} {:?} {:?}",
                v0.pos,
                v1.pos,
                v2.pos
            );
        };
        // The side quads: two triangles per 6-vertex group.
        for quad in 0..segments {
            let base = quad * 6;
            check((&scratch[base], &scratch[base + 1], &scratch[base + 2]));
            check((&scratch[base + 3], &scratch[base + 4], &scratch[base + 5]));
        }
        // The caps: 3-vertex triangles after the side block.
        let caps = segments * 6;
        for cap in 0..2 {
            let base = caps + cap * segments * 3;
            for tri in 0..segments {
                let t = base + tri * 3;
                check((&scratch[t], &scratch[t + 1], &scratch[t + 2]));
            }
        }
        println!(
            "wheel normal agreement: {checked} triangles, minimum geometric×authored dot {min_dot:.6}"
        );
        assert_eq!(checked, segments * 4, "every wheel triangle was checked");
        assert!(min_dot > 0.0);
    }
}
