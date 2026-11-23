//! Static triangle lists grouped by material. Each 32-byte vertex carries
//! world position, outward unit face normal and planar UVs (one tile per 4 m).
use crate::lit_scene::{GROUND_Y, SCENE_EXTENT, intersection_boxes};
use bytemuck::{Pod, Zeroable};
use std::ops::Range;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct LitVertex {
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

pub(crate) fn corner_mesh() -> (Vec<LitVertex>, [Range<u32>; 3]) {
    let mut vertices = Vec::new();
    let e = SCENE_EXTENT;
    let y = GROUND_Y;
    let boxes = intersection_boxes();
    let draws = std::array::from_fn(|material| {
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
    });
    (vertices, draws)
}
