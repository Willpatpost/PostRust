//! The deterministic proxy meshes (the Cycle 6-E scaffold — see
//! `docs/CYCLE6-6E-BRIEF.md`): the shared geometry every instance of
//! a kind draws from — ONE vertex buffer per proxy kind, N instances
//! through it per draw. Both meshes are generated from fixed
//! constants in fixed order (the `lit_mesh` discipline): byte-
//! identical vertices on every platform, per-face unit normals,
//! planar UVs (one tile per 4 m, the lit-scene convention).
//!
//! CONVENTIONS: the vertex format matches the lit pipeline's
//! (pos, normal, uv — 32 bytes, non-indexed triangle lists); each
//! mesh's LOCAL ORIGIN is its placement point — the humanoid
//! proxy's origin is at its FEET, the vehicle proxy's at its GROUND
//! CENTER — so an [`crate::proxy_instances::InstanceTransform`]
//! position places the mesh directly. All six faces are emitted
//! (proxies may be placed at any height; the underside is not
//! assumed buried, unlike the static corner geometry).

use bytemuck::{Pod, Zeroable};

/// The proxy vertex: world-position-relative local position, outward
/// unit face normal, planar UV — the lit pipeline's 32-byte format.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct ProxyVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

/// Emits one axis-aligned box face as six vertices (two CCW
/// triangles, the `lit_mesh` corner conventions), with the planar
/// UV mapping over the face's two varying axes (one tile per 4 m).
fn face(
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

/// Emits all six faces of one axis-aligned box (min corner `lo`,
/// max corner `hi`) — the four walls, the roof, and the UNDERSIDE
/// (its winding is the roof's reverse, its normal −Y).
fn box_faces(vertices: &mut Vec<ProxyVertex>, lo: [f32; 3], hi: [f32; 3]) {
    let [x, y, z] = lo;
    let [r, t, f] = hi;
    // +Z wall.
    face(
        vertices,
        [[x, y, f], [r, y, f], [r, t, f], [x, t, f]],
        [0., 0., 1.],
        [0, 1],
    );
    // −Z wall.
    face(
        vertices,
        [[r, y, z], [x, y, z], [x, t, z], [r, t, z]],
        [0., 0., -1.],
        [0, 1],
    );
    // +X wall.
    face(
        vertices,
        [[r, y, f], [r, y, z], [r, t, z], [r, t, f]],
        [1., 0., 0.],
        [2, 1],
    );
    // −X wall.
    face(
        vertices,
        [[x, y, z], [x, y, f], [x, t, f], [x, t, z]],
        [-1., 0., 0.],
        [2, 1],
    );
    // Roof (+Y).
    face(
        vertices,
        [[x, t, f], [r, t, f], [r, t, z], [x, t, z]],
        [0., 1., 0.],
        [0, 2],
    );
    // Underside (−Y): the roof's corner circuit reversed.
    face(
        vertices,
        [[x, y, z], [r, y, z], [r, y, f], [x, y, f]],
        [0., -1., 0.],
        [0, 2],
    );
}

/// The humanoid proxy: a three-box block figure (~1.72 m tall,
/// origin at the feet, footprint within 0.5 m). 3 boxes × 6 faces ×
/// 6 vertices = 108 vertices.
pub fn npc_proxy_mesh() -> Vec<ProxyVertex> {
    let mut vertices = Vec::with_capacity(108);
    // Legs.
    box_faces(&mut vertices, [-0.18, 0.0, -0.12], [0.18, 0.85, 0.12]);
    // Torso.
    box_faces(&mut vertices, [-0.24, 0.85, -0.14], [0.24, 1.45, 0.14]);
    // Head.
    box_faces(&mut vertices, [-0.11, 1.45, -0.11], [0.11, 1.72, 0.11]);
    vertices
}

/// The vehicle proxy: a two-box sedan-ish block (~4.2 m long, 1.8 m
/// wide, 1.5 m tall, origin at ground center). 2 boxes × 6 faces ×
/// 6 vertices = 72 vertices.
pub fn vehicle_proxy_mesh() -> Vec<ProxyVertex> {
    let mut vertices = Vec::with_capacity(72);
    // Body.
    box_faces(&mut vertices, [-0.9, 0.35, -2.1], [0.9, 1.0, 2.1]);
    // Cabin.
    box_faces(&mut vertices, [-0.8, 1.0, -1.3], [0.8, 1.5, 1.3]);
    vertices
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Determinism: each mesh generates bit-identical vertices
    /// across calls, at the pinned counts (the lit-mesh discipline:
    /// counts are structure, not coincidence).
    #[test]
    fn npc_proxy_mesh_deterministic_and_pinned() {
        let a = npc_proxy_mesh();
        let b = npc_proxy_mesh();
        assert_eq!(a.len(), 108, "3 boxes x 6 faces x 6 vertices");
        let bits = |v: &[ProxyVertex]| -> Vec<u32> {
            v.iter()
                .flat_map(|u| {
                    std::iter::once(u.pos[0].to_bits())
                        .chain(std::iter::once(u.pos[1].to_bits()))
                        .chain(std::iter::once(u.pos[2].to_bits()))
                })
                .collect()
        };
        assert_eq!(bits(&a), bits(&b), "byte-identical across calls");
    }

    /// The vehicle mesh: the same discipline.
    #[test]
    fn vehicle_proxy_mesh_deterministic_and_pinned() {
        let a = vehicle_proxy_mesh();
        let b = vehicle_proxy_mesh();
        assert_eq!(a.len(), 72, "2 boxes x 6 faces x 6 vertices");
        let bits = |v: &[ProxyVertex]| -> Vec<u32> {
            v.iter()
                .flat_map(|u| {
                    std::iter::once(u.pos[0].to_bits())
                        .chain(std::iter::once(u.pos[1].to_bits()))
                        .chain(std::iter::once(u.pos[2].to_bits()))
                })
                .collect()
        };
        assert_eq!(bits(&a), bits(&b), "byte-identical across calls");
    }

    /// Validity: every vertex finite, every normal unit-length, UVs
    /// finite, and the placement bounds hold (the documented local
    /// origins and footprints — what the instance transform contract
    /// above them relies on).
    #[test]
    fn proxy_meshes_are_valid_and_bounded() {
        for mesh in [npc_proxy_mesh(), vehicle_proxy_mesh()] {
            for vertex in &mesh {
                assert!(vertex.pos.iter().all(|v| v.is_finite()));
                assert!(vertex.uv.iter().all(|v| v.is_finite()));
                let len = (vertex.normal[0] * vertex.normal[0]
                    + vertex.normal[1] * vertex.normal[1]
                    + vertex.normal[2] * vertex.normal[2])
                    .sqrt();
                assert!((len - 1.0).abs() < 1e-6, "unit normal: {:?}", vertex.normal);
            }
        }
        for vertex in npc_proxy_mesh() {
            assert!(vertex.pos[1] >= 0.0 && vertex.pos[1] <= 1.72, "NPC height");
            assert!(
                vertex.pos[0].abs() <= 0.25 && vertex.pos[2].abs() <= 0.25,
                "NPC footprint"
            );
        }
        for vertex in vehicle_proxy_mesh() {
            assert!(
                vertex.pos[1] >= 0.0 && vertex.pos[1] <= 1.5,
                "vehicle height"
            );
            assert!(vertex.pos[0].abs() <= 0.9, "vehicle width");
            assert!(vertex.pos[2].abs() <= 2.1, "vehicle length");
        }
    }
}
