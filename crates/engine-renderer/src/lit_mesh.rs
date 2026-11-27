//! Static triangle lists grouped by material. Each 32-byte vertex carries
//! world position, outward unit face normal and planar UVs (one tile per 4 m).
use crate::lit_scene::SceneBox;
use bytemuck::{Pod, Zeroable};
use std::ops::Range;

/// One lit-mesh vertex — the lit pipelines' 32-byte input (the
/// layout `lit_scene.wgsl`'s locations 0-2 read: world position,
/// outward unit face normal, planar UVs). The public type behind
/// [`crate::scene_description::SceneMesh`]'s vertex buffer.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct LitVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

fn quad(vertices: &mut Vec<LitVertex>, corners: [[f32; 3]; 4], normal: [f32; 3], axes: [usize; 2]) {
    for i in [0, 1, 2, 0, 2, 3] {
        let pos = corners[i];
        vertices.push(LitVertex {
            pos,
            normal,
            uv: axes.map(|axis| pos[axis] / 4.0),
        });
    }
}

/// Builds a lit mesh from an authored box table (P4 S2's
/// parameterization of the former `corner_mesh()` — the emission
/// is pure MECHANISM shared by the attach path and the CPU
/// oracle; the boxes, the ground plate's extent and height, and
/// the material count are the caller's AUTHORED content).
///
/// ONE draw range per material, in material order: material 0's
/// range starts with the ground quad (a `ground_extent` square at
/// `ground_y` — the wet receiver's position in the corner
/// authoring), then each material's boxes contribute their four
/// walls and roof (the underside is buried in the ground). The
/// ranges are contiguous and cover the whole vertex buffer — the
/// shape [`crate::scene_description::SceneDescription::validate`]
/// requires and the shadow depth pass's whole-mesh draw assumes.
pub(crate) fn mesh_from_boxes(
    boxes: &[SceneBox],
    ground_extent: f32,
    ground_y: f32,
    material_count: usize,
) -> (Vec<LitVertex>, Vec<Range<u32>>) {
    let mut vertices = Vec::new();
    let e = ground_extent;
    let y = ground_y;
    let draws: Vec<Range<u32>> = (0..material_count)
        .map(|material| {
            let start = vertices.len() as u32;
            if material == 0 {
                quad(
                    &mut vertices,
                    [[-e, y, e], [e, y, e], [e, y, -e], [-e, y, -e]],
                    [0., 1., 0.],
                    [0, 2],
                );
            }
            for b in boxes.iter().filter(|b| b.material as usize == material) {
                let [x, y, z] = b.min;
                let [r, t, f] = b.max;
                // Four walls and roof. The underside is buried in the ground.
                quad(
                    &mut vertices,
                    [[x, y, f], [r, y, f], [r, t, f], [x, t, f]],
                    [0., 0., 1.],
                    [0, 1],
                );
                quad(
                    &mut vertices,
                    [[r, y, z], [x, y, z], [x, t, z], [r, t, z]],
                    [0., 0., -1.],
                    [0, 1],
                );
                quad(
                    &mut vertices,
                    [[r, y, f], [r, y, z], [r, t, z], [r, t, f]],
                    [1., 0., 0.],
                    [2, 1],
                );
                quad(
                    &mut vertices,
                    [[x, y, z], [x, y, f], [x, t, f], [x, t, z]],
                    [-1., 0., 0.],
                    [2, 1],
                );
                quad(
                    &mut vertices,
                    [[x, t, f], [r, t, f], [r, t, z], [x, t, z]],
                    [0., 1., 0.],
                    [0, 2],
                );
            }
            start..vertices.len() as u32
        })
        .collect();
    (vertices, draws)
}
