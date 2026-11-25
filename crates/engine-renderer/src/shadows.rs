//! The directional-shadow deterministic core (Cycle 6-F — see
//! `docs/AUTONOMOUS-CYCLE-PROTOCOL.md` §2 item 2): the light-space
//! orthographic projection, the single cascade's bounds, the
//! slope-scaled depth-bias model, and the CPU raycaster oracle that
//! pins the shadow map's depth values and the shadowed/unshadowed
//! pixel equivalence. ONE CASCADE, the corner scene, no framework —
//! the `proxy_instances.rs` pattern: everything deterministic and
//! pure lives here, fully tested; the implementer wires the
//! depth-only pass, the shadowed lit pipeline, and the capture-based
//! gates against these functions.
//!
//! DETERMINISM CONTRACT: every operation is pure IEEE f32 arithmetic
//! in a FIXED order. `sqrt` (basis normalization) is IEEE-754
//! correctly-rounded — one of the four basic operations the standard
//! requires to be bit-exact — unlike `sin`/`cos`, whose bits vary
//! across libm platforms (the standing libm-free discipline; see
//! `proxy_instances` for the turn-fraction workaround when a
//! transcendental would otherwise be needed). No packing here packs
//! bias or matrix bytes through anything but `f32` arithmetic and
//! `to_bits`.
//!
//! DEPTH CONVENTION: the light-space depth of a point is its
//! distance TOWARD the light, mapped linearly from the cascade's
//! near plane (0) to its far plane (1) — Vulkan's [0, 1] depth with
//! `LESS` comparison, so an occluder (closer to the light) carries a
//! SMALLER depth than the receiver behind it, exactly like the main
//! pass. The map's clear value is 1.0 (no occluder).

use crate::lit_scene::SceneBox;

/// The compare-time depth-bias parameters (caller policy, shared
/// verbatim by the shadowed shader's `shadow_factor` and the CPU
/// oracle — see [`depth_bias`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowBias {
    /// The constant floor, in [0, 1] depth units.
    pub constant: f32,
    /// The slope-scale multiplier (grazing surfaces bias harder).
    pub slope_scale: f32,
}

/// The shadow map's default texel resolution (square). One cascade
/// over the 40 m corner at 1024² resolves ~6 cm per texel — ample
/// for the building-mass shadows this package draws; revisitable
/// with rig evidence, never silently.
pub const SHADOW_MAP_EXTENT: u32 = 1024;

/// The self-hit skip distance for shadow rays: a receiver's own
/// surface sits at t = 0 along its shadow ray, so hits strictly
/// closer than this are the receiver itself, not an occluder
/// (mirrored by the shader's compare-time bias floor).
pub const SELF_HIT_EPSILON: f32 = 1.0 / 512.0;

/// One directional-light shadow cascade: the light-space
/// orthonormal basis, the orthographic bounds covering the scene's
/// world AABB, and the world→light-clip matrix the depth-only pass
/// and the shadowed lit pipeline share.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowCascade {
    /// The unit vector TOWARD the light (the projection's depth
    /// axis; the shadow camera looks along −light).
    pub light: [f32; 3],
    /// The map's horizontal axis (unit, world space).
    pub right: [f32; 3],
    /// The map's vertical axis (unit, world space).
    pub up: [f32; 3],
    /// The orthographic bounds in light-space axes: min/max of
    /// `dot(right, p)` and `dot(up, p)` over the scene AABB, and the
    /// near/far depths along `light`.
    pub min_right: f32,
    pub max_right: f32,
    pub min_up: f32,
    pub max_up: f32,
    pub near: f32,
    pub far: f32,
    /// The world→light-clip matrix (column-major, the repo's `Mat4`
    /// convention: clip = Σ columns[i] · p[i]; NDC z ∈ [0, 1]).
    pub matrix: [[f32; 4]; 4],
}

/// The scene's world AABB (the static corner masses; the ground
/// plane at y = 0 lies inside it). Pure data walk in fixed order.
pub fn scene_bounds(boxes: &[SceneBox]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for box_ in boxes {
        for axis in 0..3 {
            min[axis] = min[axis].min(box_.min[axis]);
            max[axis] = max[axis].max(box_.max[axis]);
        }
    }
    (min, max)
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len == 0.0 {
        return v;
    }
    [v[0] / len, v[1] / len, v[2] / len]
}

/// Builds the single shadow cascade for a light direction and the
/// scene's world AABB: the light-space basis (deterministic — the
/// reference axis flips only when the light is near-vertical, a
/// fixed threshold), the bounds (the AABB's 8 corners projected in
/// fixed order), and the matrix.
pub fn cascade_for(light_dir: [f32; 3], scene_min: [f32; 3], scene_max: [f32; 3]) -> ShadowCascade {
    let light = normalize3(light_dir);
    // The reference axis for the map's vertical: +Y unless the light
    // is near-vertical (the cross product would degenerate); the
    // 0.999 threshold is fixed and documented.
    let reference = if dot3(light, [0.0, 1.0, 0.0]).abs() < 0.999 {
        [0.0, 1.0, 0.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let right = normalize3(cross3(reference, light));
    let up = cross3(light, right);

    // The AABB's 8 corners, in fixed binary-count order.
    let mut min_right = f32::MAX;
    let mut max_right = f32::MIN;
    let mut min_up = f32::MAX;
    let mut max_up = f32::MIN;
    let mut near = f32::MAX;
    let mut far = f32::MIN;
    for corner in 0..8 {
        let world = [
            if corner & 1 == 0 {
                scene_min[0]
            } else {
                scene_max[0]
            },
            if corner & 2 == 0 {
                scene_min[1]
            } else {
                scene_max[1]
            },
            if corner & 4 == 0 {
                scene_min[2]
            } else {
                scene_max[2]
            },
        ];
        let r = dot3(right, world);
        let u = dot3(up, world);
        let d = dot3(light, world);
        min_right = min_right.min(r);
        max_right = max_right.max(r);
        min_up = min_up.min(u);
        max_up = max_up.max(u);
        near = near.min(d);
        far = far.max(d);
    }

    let span_right = max_right - min_right;
    let span_up = max_up - min_up;
    let span_depth = far - near;
    let center_right = (min_right + max_right) * 0.5;
    let center_up = (min_up + max_up) * 0.5;
    // clip.x = (dot(right,p) − center_right) · 2/span_right, and the
    // same shape for y. DEPTH: (far − dot(light,p)) / span — the
    // point CLOSEST to the light maps to 0 and the farthest to 1,
    // the camera-like [0, 1] LESS convention the depth-only pass and
    // the shadowed compare both rely on (an occluder nearer the
    // light carries the SMALLER depth; the map's clear value 1.0
    // means "nothing nearer than the farthest plane"). The inverted
    // form (dot − near) was the 6-F build's one real convention bug:
    // it made the GROUND win the depth test over every roof (the
    // vertical-light probe caught it — the whole map read the
    // ground's depth).
    // Column-major: column `a` is the world axis-a unit vector's
    // light-clip image — the basis components distribute ACROSS
    // columns (row `o` carries output o's scale). Writing whole
    // scaled basis vectors as columns is the transpose bug the
    // corner-round-trip tests below exist to catch.
    let sx = 2.0 / span_right;
    let sy = 2.0 / span_up;
    let sz = 1.0 / span_depth;
    let matrix = [
        [right[0] * sx, up[0] * sy, -light[0] * sz, 0.0],
        [right[1] * sx, up[1] * sy, -light[1] * sz, 0.0],
        [right[2] * sx, up[2] * sy, -light[2] * sz, 0.0],
        [-center_right * sx, -center_up * sy, far * sz, 1.0],
    ];

    ShadowCascade {
        light,
        right,
        up,
        min_right,
        max_right,
        min_up,
        max_up,
        near,
        far,
        matrix,
    }
}

/// A point's light-space depth in [0, 1]: 0 closest to the light,
/// 1 farthest (the camera-like LESS convention the map rasterizes
/// and the shadowed compare uses).
pub fn light_depth(cascade: &ShadowCascade, point: [f32; 3]) -> f32 {
    (cascade.far - dot3(cascade.light, point)) / (cascade.far - cascade.near)
}

/// The slope-scaled compare-time depth bias (the Vulkan
/// `vkCmdSetDepthBias` equation's software form, applied at COMPARE
/// time so both the shader and this CPU oracle compute it from the
/// same inputs): `bias = constant + slope_scale · m`, where `m` is
/// the receiver's light-space depth slope. For the corner scene's
/// axis-aligned faces under a directional light the slope reduces to
/// a fixed function of the surface normal (documented; the shader
/// mirrors this exact expression).
pub fn depth_bias(normal: [f32; 3], light: [f32; 3], constant: f32, slope_scale: f32) -> f32 {
    // The light-space depth slope of a plane with world normal n:
    // the depth axis is `light`, so the slope is the plane's depth
    // change per light-space horizontal step — |dot(n, light)| in
    // the denominator pushes grazing surfaces toward the max-slope
    // clamp, the classic peter-panning/lacing trade.
    let facing = dot3(normal, light).abs().max(1.0 / 64.0);
    constant + slope_scale * (1.0 / facing - 1.0)
}

/// The ray–AABB slab test: the entry distance along `dir` (unit) or
/// `None` when the ray misses or starts outside. Fixed operation
/// order; ties across boxes resolve to the EARLIER table entry (the
/// caller's iteration order is the tie-break).
fn ray_box_entry(origin: [f32; 3], dir: [f32; 3], min: [f32; 3], max: [f32; 3]) -> Option<f32> {
    let mut t_enter = 0.0f32;
    let mut t_exit = f32::INFINITY;
    for axis in 0..3 {
        if dir[axis] == 0.0 {
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return None;
            }
            continue;
        }
        let inv = 1.0 / dir[axis];
        let (t0, t1) = if inv >= 0.0 {
            (
                (min[axis] - origin[axis]) * inv,
                (max[axis] - origin[axis]) * inv,
            )
        } else {
            (
                (max[axis] - origin[axis]) * inv,
                (min[axis] - origin[axis]) * inv,
            )
        };
        t_enter = t_enter.max(t0);
        t_exit = t_exit.min(t1);
        if t_enter > t_exit {
            return None;
        }
    }
    Some(t_enter)
}

/// The first occluder hit along the shadow ray from `point` toward
/// the light: the distance to the nearest box whose entry is
/// strictly beyond the self-hit epsilon, or `None` when the point
/// sees the light. Boxes iterate in table order (the deterministic
/// tie-break). The ground plane is NOT an occluder (nothing stands
/// below it).
pub fn shadow_ray_hit(point: [f32; 3], cascade: &ShadowCascade, boxes: &[SceneBox]) -> Option<f32> {
    shadow_ray_hit_detail(point, cascade, boxes).map(|(_, t)| t)
}

/// The detailed form (the map oracle's): the hit box's table index
/// alongside the distance, for edge-margin computation.
pub fn shadow_ray_hit_detail(
    point: [f32; 3],
    cascade: &ShadowCascade,
    boxes: &[SceneBox],
) -> Option<(usize, f32)> {
    let mut nearest: Option<(usize, f32)> = None;
    for (index, box_) in boxes.iter().enumerate() {
        if let Some(t) = ray_box_entry(point, cascade.light, box_.min, box_.max)
            && t > SELF_HIT_EPSILON
            && nearest.is_none_or(|(_, best)| t < best)
        {
            nearest = Some((index, t));
        }
    }
    nearest
}

// ============================================================================
// The populated oracle (the Era-2 proxy-shadows stage): the SAME raycaster
// extended with the per-instance occluders — the NPC's POSED boxes (the
// skin matrices the renderer draws from, so the oracle and the depth pass
// see the same geometry) and the sedan's authored parts (the body, the
// cabin, the four wheel CYLINDERS — matching sedan_mesh part-for-part, so
// no wheel-tread texel mismatches an AABB stand-in).
//
// PER-INSTANCE BOUNDS CULLING (the affordability discipline the mission
// demands measured-first): every occluder carries its pose's WORLD AABB,
// computed ONCE per frame at [`NpcOccluder::new`]/[`VehicleOccluder::new`]
// — never per ray — and the ray walks test the AABB FIRST (one slab test)
// before the 11 box tests or 6 vehicle part tests. The AABB contains its
// boxes by construction, so the cull never changes an answer, only the
// cost. (A BVH only if measurements demand it — the gate's census prints
// the per-texel test counts.)
//
// DETERMINISM CONTRACT: unchanged — pure IEEE f32 in fixed order. The
// structured frames invert by TRANSPOSE-and-divide (the rotation's exact
// transpose, the uniform scale's exact divide), never a general matrix
// inverse; sqrt (the wheel's quadratic) is IEEE-correctly-rounded.
// ============================================================================

use crate::animation::{HUMANOID_BOXES, HUMANOID_JOINTS, POSE_STRIDE, mat3_multiply};
use crate::proxy_instances::{ProxyInstance, instance_columns};

/// The sedan's occluder parts in MODEL space (sedan_mesh's own
/// geometry, matched part-for-part — the oracle never approximates
/// what the depth pass rasterizes): the body box, the cabin box, and
/// the four wheel cylinders (center, radius, half-width along the
/// local X axle).
pub const SEDAN_BODY: ([f32; 3], [f32; 3]) = ([-0.95, 0.35, -2.20], [0.95, 0.90, 2.20]);
pub const SEDAN_CABIN: ([f32; 3], [f32; 3]) = ([-0.82, 0.90, -1.10], [0.82, 1.40, 1.10]);
/// The four wheels: (axle center, radius, half-width), local X axis.
pub const SEDAN_WHEELS: [([f32; 3], f32, f32); 4] = [
    ([-0.85, 0.35, -1.45], 0.35, 0.125),
    ([-0.85, 0.35, 1.45], 0.35, 0.125),
    ([0.85, 0.35, -1.45], 0.35, 0.125),
    ([0.85, 0.35, 1.45], 0.35, 0.125),
];

/// A structured rigid frame — rotation (column-major flat 3x3, the
/// [`crate::animation`] convention), translation, uniform scale — with
/// the EXACT world→local mapping: `local = Rᵀ·(p − t) / s` (the
/// rotation's transpose and the scale's divide; no general matrix
/// inverse anywhere on this path).
#[derive(Debug, Clone, Copy)]
struct Frame3 {
    /// Column-major flat: rotation[col*3 + row].
    rotation: [f32; 9],
    translation: [f32; 3],
    scale: f32,
}

impl Frame3 {
    /// The frame's rotation applied to a direction (the repo's
    /// column-major application: component r = Σ꜀ rot[c·3 + r]·v[c]).
    fn rotate(self, v: [f32; 3]) -> [f32; 3] {
        [
            self.rotation[0] * v[0] + self.rotation[3] * v[1] + self.rotation[6] * v[2],
            self.rotation[1] * v[0] + self.rotation[4] * v[1] + self.rotation[7] * v[2],
            self.rotation[2] * v[0] + self.rotation[5] * v[1] + self.rotation[8] * v[2],
        ]
    }

    /// A world point in the authored frame: Rᵀ·(p − t)/s. THE
    /// TRANSPOSE APPLICATION (the pinned known answer: under a
    /// quarter yaw — whose rotation maps +X to −Z — the inverse maps
    /// the world −Z back to local +X; the test below pins it):
    /// Rᵀ's rows are R's columns, i.e. the flat array's consecutive
    /// triples — component r = rot[3r]·v₀ + rot[3r+1]·v₁ + rot[3r+2]·v₂
    /// over the offset — then the scale divide.
    fn to_local(self, p: [f32; 3]) -> [f32; 3] {
        let v = [
            p[0] - self.translation[0],
            p[1] - self.translation[1],
            p[2] - self.translation[2],
        ];
        [
            (self.rotation[0] * v[0] + self.rotation[1] * v[1] + self.rotation[2] * v[2])
                / self.scale,
            (self.rotation[3] * v[0] + self.rotation[4] * v[1] + self.rotation[5] * v[2])
                / self.scale,
            (self.rotation[6] * v[0] + self.rotation[7] * v[1] + self.rotation[8] * v[2])
                / self.scale,
        ]
    }

    /// A world direction in the authored frame: Rᵀ·d/s (the same
    /// transpose application as [`Self::to_local`]). The mapping is
    /// affine in the ray PARAMETER: p_local(τ) = local_origin +
    /// τ·(Rᵀ·d)/s — the parameter τ is unchanged (the local
    /// direction's LENGTH scales by 1/s, which no slab test cares
    /// about), so entries return in WORLD distance units, directly
    /// comparable with the static boxes' hits.
    fn dir_to_local(self, d: [f32; 3]) -> [f32; 3] {
        [
            (self.rotation[0] * d[0] + self.rotation[1] * d[1] + self.rotation[2] * d[2])
                / self.scale,
            (self.rotation[3] * d[0] + self.rotation[4] * d[1] + self.rotation[5] * d[2])
                / self.scale,
            (self.rotation[6] * d[0] + self.rotation[7] * d[1] + self.rotation[8] * d[2])
                / self.scale,
        ]
    }
}

/// One NPC occluder for the populated raycaster: the instance, its
/// evaluated skin-matrix blocks (the same bytes the renderer draws
/// from — indexed by the instance's `pose_index`), and the pose's
/// WORLD AABB (the bounds-cull input, computed once here).
#[derive(Clone, Copy)]
pub struct NpcOccluder<'a> {
    pub instance: &'a ProxyInstance,
    /// The pose blocks LIST (pose-index order; each block is 11
    /// column-major skin matrices).
    pub blocks: &'a [[[[f32; 4]; 4]; HUMANOID_JOINTS]],
    /// The posed figure's world AABB (all 11 boxes, instance
    /// transform applied).
    pub world_bounds: ([f32; 3], [f32; 3]),
}

impl<'a> NpcOccluder<'a> {
    /// Builds the occluder, computing the world AABB (the posed
    /// boxes' world corners unioned — the posed model AABB
    /// transformed by the instance; per-frame work, never per-ray).
    pub fn new(
        instance: &'a ProxyInstance,
        blocks: &'a [[[[f32; 4]; 4]; HUMANOID_JOINTS]],
    ) -> Self {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        let block = &blocks[pose_slot(instance)];
        for (box_index, &(lo, hi)) in HUMANOID_BOXES.iter().enumerate() {
            let frame = npc_box_frame(instance, block, box_index);
            for corner in 0..8 {
                let local = [
                    if corner & 1 == 0 { lo[0] } else { hi[0] },
                    if corner & 2 == 0 { lo[1] } else { hi[1] },
                    if corner & 4 == 0 { lo[2] } else { hi[2] },
                ];
                // The frame's forward application: t + s·R·local.
                let rotated = frame.rotate(local);
                let world = [
                    frame.translation[0] + frame.scale * rotated[0],
                    frame.translation[1] + frame.scale * rotated[1],
                    frame.translation[2] + frame.scale * rotated[2],
                ];
                for axis in 0..3 {
                    min[axis] = min[axis].min(world[axis]);
                    max[axis] = max[axis].max(world[axis]);
                }
            }
        }
        NpcOccluder {
            instance,
            blocks,
            world_bounds: (min, max),
        }
    }
}

/// One vehicle occluder: the sedan's parts under the placement, with
/// the whole vehicle's world AABB (the parts unioned — the wheels'
/// outer corners included, so the bounds contain every part).
#[derive(Clone, Copy)]
pub struct VehicleOccluder<'a> {
    pub instance: &'a ProxyInstance,
    pub world_bounds: ([f32; 3], [f32; 3]),
}

impl<'a> VehicleOccluder<'a> {
    pub fn new(instance: &'a ProxyInstance) -> Self {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        let frame = vehicle_frame(instance);
        for (lo, hi) in [SEDAN_BODY, SEDAN_CABIN] {
            absorb_box(frame, lo, hi, &mut min, &mut max);
        }
        for (center, radius, half_width) in SEDAN_WHEELS {
            // The wheel's bounding box (the cull's bounds only — the
            // RAY TEST stays the exact cylinder).
            let lo = [
                center[0] - half_width,
                center[1] - radius,
                center[2] - radius,
            ];
            let hi = [
                center[0] + half_width,
                center[1] + radius,
                center[2] + radius,
            ];
            absorb_box(frame, lo, hi, &mut min, &mut max);
        }
        VehicleOccluder {
            instance,
            world_bounds: (min, max),
        }
    }
}

/// The instance's pose slot (the `pose_index` mapping — the blocks
/// list is indexed by pose block, not by npc list position).
fn pose_slot(instance: &ProxyInstance) -> usize {
    instance.pose_index as usize
}

/// The NPC box (b, joint BOX_JOINT[b])'s world frame: W = I·Mⱼ
/// decomposed into the structured form — rotation R_I·R_Mⱼ (the
/// instance's rotation — its upper 3x3 divided by the exact authored
/// scale — composed with the skin matrix's rotation), translation
/// upper(I)·t_Mⱼ + t_I (the affine compose, the scale folded through
/// upper(I)), scale s.
fn npc_box_frame(
    instance: &ProxyInstance,
    skin: &[[[f32; 4]; 4]; HUMANOID_JOINTS],
    box_index: usize,
) -> Frame3 {
    let joint = crate::animation::BOX_JOINT[box_index];
    let skin_mat = &skin[joint];
    let cols = instance_columns(&instance.transform);
    let s = instance.transform.scale;
    // R_I (flat column-major) = the instance's upper 3x3 / s.
    let r_instance = [
        cols[0][0] / s,
        cols[0][1] / s,
        cols[0][2] / s,
        cols[1][0] / s,
        cols[1][1] / s,
        cols[1][2] / s,
        cols[2][0] / s,
        cols[2][1] / s,
        cols[2][2] / s,
    ];
    // R_Mⱼ = the skin matrix's upper 3x3 (the skin matrices carry no
    // scale — pure rotations, by construction).
    let r_skin = [
        skin_mat[0][0],
        skin_mat[0][1],
        skin_mat[0][2],
        skin_mat[1][0],
        skin_mat[1][1],
        skin_mat[1][2],
        skin_mat[2][0],
        skin_mat[2][1],
        skin_mat[2][2],
    ];
    // rotation = R_I · R_Mⱼ (the ONE column-major 3x3 product —
    // animation.rs's own implementation, the convention's single
    // source).
    let rotation = mat3_multiply(r_instance, r_skin);
    // translation = upper(I) · t_Mⱼ + t_I.
    let t_skin = [skin_mat[3][0], skin_mat[3][1], skin_mat[3][2]];
    let scaled = [
        cols[0][0] * t_skin[0] + cols[1][0] * t_skin[1] + cols[2][0] * t_skin[2],
        cols[0][1] * t_skin[0] + cols[1][1] * t_skin[1] + cols[2][1] * t_skin[2],
        cols[0][2] * t_skin[0] + cols[1][2] * t_skin[1] + cols[2][2] * t_skin[2],
    ];
    let translation = [
        scaled[0] + cols[3][0],
        scaled[1] + cols[3][1],
        scaled[2] + cols[3][2],
    ];
    Frame3 {
        rotation,
        translation,
        scale: s,
    }
}

/// The vehicle's world frame (the placement alone — the parts are
/// authored in model space).
fn vehicle_frame(instance: &ProxyInstance) -> Frame3 {
    let cols = instance_columns(&instance.transform);
    let s = instance.transform.scale;
    Frame3 {
        rotation: [
            cols[0][0] / s,
            cols[0][1] / s,
            cols[0][2] / s,
            cols[1][0] / s,
            cols[1][1] / s,
            cols[1][2] / s,
            cols[2][0] / s,
            cols[2][1] / s,
            cols[2][2] / s,
        ],
        translation: [cols[3][0], cols[3][1], cols[3][2]],
        scale: s,
    }
}

/// Unions an authored box's world corners into the running bounds.
fn absorb_box(frame: Frame3, lo: [f32; 3], hi: [f32; 3], min: &mut [f32; 3], max: &mut [f32; 3]) {
    for corner in 0..8 {
        let local = [
            if corner & 1 == 0 { lo[0] } else { hi[0] },
            if corner & 2 == 0 { lo[1] } else { hi[1] },
            if corner & 4 == 0 { lo[2] } else { hi[2] },
        ];
        let rotated = frame.rotate(local);
        let world = [
            frame.translation[0] + frame.scale * rotated[0],
            frame.translation[1] + frame.scale * rotated[1],
            frame.translation[2] + frame.scale * rotated[2],
        ];
        for axis in 0..3 {
            min[axis] = min[axis].min(world[axis]);
            max[axis] = max[axis].max(world[axis]);
        }
    }
}

/// Ray vs an authored AABB under a structured frame: the ray maps to
/// the authored frame (the exact transpose/divide) and the slab test
/// runs there. Returns the entry distance (WORLD units — the frame
/// mapping preserves the ray parameter) and the hit point in the
/// AUTHORED frame (the edge-margin computation's input).
fn ray_frame_box_entry(
    origin: [f32; 3],
    dir: [f32; 3],
    frame: Frame3,
    lo: [f32; 3],
    hi: [f32; 3],
) -> Option<(f32, [f32; 3])> {
    let local_origin = frame.to_local(origin);
    let local_dir = frame.dir_to_local(dir);
    let t = ray_box_entry(local_origin, local_dir, lo, hi)?;
    let hit = [
        local_origin[0] + t * local_dir[0],
        local_origin[1] + t * local_dir[1],
        local_origin[2] + t * local_dir[2],
    ];
    Some((t, hit))
}

/// Ray vs an X-axis finite cylinder (the sedan wheel's exact form —
/// the tread is a circle in the local y-z plane, the caps the x
/// slab): the near entry where the ray is simultaneously inside the
/// radius and the slab. Handles the axis-parallel ray (the circle
/// membership at the cap). Returns the entry distance (the caller
/// maps the ray first, so the parameter is the world parameter).
fn ray_x_cylinder_entry(
    local_origin: [f32; 3],
    local_dir: [f32; 3],
    center: [f32; 3],
    radius: f32,
    half_width: f32,
) -> Option<f32> {
    let o = [
        local_origin[0] - center[0],
        local_origin[1] - center[1],
        local_origin[2] - center[2],
    ];
    let d = local_dir;
    // The circle quadratic in (y, z).
    let a = d[1] * d[1] + d[2] * d[2];
    let b = 2.0 * (o[1] * d[1] + o[2] * d[2]);
    let c = o[1] * o[1] + o[2] * o[2] - radius * radius;
    let inside_circle = |p: [f32; 3]| p[1] * p[1] + p[2] * p[2] <= radius * radius;
    let in_slab = |t: f32| (o[0] + t * d[0]).abs() <= half_width;
    let point_at = |t: f32| [o[0] + t * d[0], o[1] + t * d[1], o[2] + t * d[2]];
    // The tread entry (the near quadratic root), valid when the
    // crossing sits within the slab.
    if a > 0.0 {
        let disc = b * b - 4.0 * a * c;
        if disc >= 0.0 {
            let t = (-b - disc.sqrt()) / (2.0 * a);
            if t >= 0.0 && in_slab(t) {
                return Some(t);
            }
        }
    } else {
        // The ray runs along the axle: inside the (infinite)
        // cylinder everywhere or nowhere.
        if c > 0.0 {
            return None;
        }
    }
    // The near cap: the slab plane the ray enters from, valid when
    // the crossing is inside the circle.
    if d[0] != 0.0 {
        let near_x = if d[0] > 0.0 { -half_width } else { half_width };
        let t = (near_x - o[0]) / d[0];
        if t >= 0.0 && inside_circle(point_at(t)) {
            return Some(t);
        }
    }
    None
}

/// The surface a populated-map texel hit (the margin-skip classes —
/// the gate computes each class's edge margin its own way).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OccluderSurface {
    /// No hit — the map's clear value.
    None,
    /// The ground quad.
    Ground,
    /// Static box (the intersection_boxes table index).
    Building(usize),
    /// NPC (list index)'s mesh box (emission order).
    NpcBox(usize, usize),
    /// Vehicle (list index)'s part: 0 body, 1 cabin, 2..=5 wheels.
    VehiclePart(usize, usize),
}

/// One map texel's expected depth in a POPULATED scene: the static
/// oracle's answer extended with the instance occluders — the first
/// surface the light's ray reaches, static boxes, instances, or the
/// ground, whichever is nearest.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PopulatedTexelExpectation {
    pub depth: f32,
    /// The hit point in world space (None for no hit).
    pub hit: Option<[f32; 3]>,
    pub surface: OccluderSurface,
    /// For OBB-part hits (NPC boxes, vehicle body/cabin): the hit in
    /// the part's AUTHORED frame plus the authored bounds — the
    /// edge-margin computation's inputs. Wheel hits carry the
    /// wheel's bounding box (a conservative margin — the tread's
    /// tangent band skips slightly more than exact; documented).
    pub local: Option<([f32; 3], [f32; 3], [f32; 3])>,
}

/// The map walk's running best hit: the surface, the entry distance,
/// and the local-frame margin inputs for OBB parts.
type BestMargin = Option<([f32; 3], [f32; 3], [f32; 3])>;
type BestHit = Option<(OccluderSurface, f32, BestMargin)>;

/// The populated map oracle: the expected depth the depth-only pass
/// rasterizes at texel (x, y) with the instances drawn — the static
/// walk (boxes + ground, [`map_texel_depth`]'s own geometry) plus the
/// per-instance occluders, nearest wins. The per-instance bounds cull
/// runs FIRST (the AABB slab test — one per instance); a culled
/// instance's boxes are never tested (the AABB contains them, so the
/// cull never changes the answer).
pub fn map_texel_depth_populated(
    cascade: &ShadowCascade,
    boxes: &[SceneBox],
    npcs: &[NpcOccluder<'_>],
    vehicles: &[VehicleOccluder<'_>],
    x: u32,
    y: u32,
    extent: u32,
) -> PopulatedTexelExpectation {
    let origin = texel_ray_origin(cascade, x, y, extent);
    let span = cascade.far - cascade.near;
    let descent = [-cascade.light[0], -cascade.light[1], -cascade.light[2]];
    // The best hit: (surface, t, local-margin inputs).
    let mut best: BestHit = None;
    let consider = |surface: OccluderSurface, t: f32, local: BestMargin, best: &mut BestHit| {
        if best.is_none_or(|(_, t_best, _)| t < t_best) {
            *best = Some((surface, t, local));
        }
    };
    // The static boxes (map_texel_depth's own walk, t >= 0 — the
    // far-plane origin touches the tallest roof at depth 0).
    for (index, box_) in boxes.iter().enumerate() {
        if let Some(t) = ray_box_entry(origin, descent, box_.min, box_.max)
            && t >= 0.0
        {
            consider(OccluderSurface::Building(index), t, None, &mut best);
        }
    }
    // The instances — the bounds cull first.
    for (npc_index, npc) in npcs.iter().enumerate() {
        let (bmin, bmax) = npc.world_bounds;
        if ray_box_entry(origin, descent, bmin, bmax).is_none() {
            continue;
        }
        let blocks = &npc.blocks[pose_slot(npc.instance)];
        for (box_index, &(lo, hi)) in HUMANOID_BOXES.iter().enumerate() {
            let frame = npc_box_frame(npc.instance, blocks, box_index);
            if let Some((t, local_hit)) = ray_frame_box_entry(origin, descent, frame, lo, hi)
                && t >= 0.0
            {
                consider(
                    OccluderSurface::NpcBox(npc_index, box_index),
                    t,
                    Some((local_hit, lo, hi)),
                    &mut best,
                );
            }
        }
    }
    for (vehicle_index, vehicle) in vehicles.iter().enumerate() {
        let (bmin, bmax) = vehicle.world_bounds;
        if ray_box_entry(origin, descent, bmin, bmax).is_none() {
            continue;
        }
        let frame = vehicle_frame(vehicle.instance);
        for (part, &(lo, hi)) in [SEDAN_BODY, SEDAN_CABIN].iter().enumerate() {
            if let Some((t, local_hit)) = ray_frame_box_entry(origin, descent, frame, lo, hi)
                && t >= 0.0
            {
                consider(
                    OccluderSurface::VehiclePart(vehicle_index, part),
                    t,
                    Some((local_hit, lo, hi)),
                    &mut best,
                );
            }
        }
        for (wheel, &(center, radius, half_width)) in SEDAN_WHEELS.iter().enumerate() {
            let local_origin = frame.to_local(origin);
            let local_dir = frame.dir_to_local(descent);
            if let Some(t) =
                ray_x_cylinder_entry(local_origin, local_dir, center, radius, half_width)
                && t >= 0.0
            {
                // The conservative wheel margin: the bounding box.
                let lo = [
                    center[0] - half_width,
                    center[1] - radius,
                    center[2] - radius,
                ];
                let hi = [
                    center[0] + half_width,
                    center[1] + radius,
                    center[2] + radius,
                ];
                let local_hit = [
                    local_origin[0] + t * local_dir[0],
                    local_origin[1] + t * local_dir[1],
                    local_origin[2] + t * local_dir[2],
                ];
                consider(
                    OccluderSurface::VehiclePart(vehicle_index, 2 + wheel),
                    t,
                    Some((local_hit, lo, hi)),
                    &mut best,
                );
            }
        }
    }
    // The ground quad (the last candidate — map_texel_depth's form).
    if cascade.light[1] > 0.0 && origin[1] > 0.0 {
        let t_ground = origin[1] / cascade.light[1];
        if best.is_none_or(|(_, t, _)| t_ground < t) {
            let crossing = [
                origin[0] + descent[0] * t_ground,
                0.0,
                origin[2] + descent[2] * t_ground,
            ];
            let inside = crossing[0].abs() <= crate::lit_scene::SCENE_EXTENT
                && crossing[2].abs() <= crate::lit_scene::SCENE_EXTENT;
            if inside && t_ground >= 0.0 {
                best = Some((OccluderSurface::Ground, t_ground, None));
            }
        }
    }
    match best {
        Some((surface, t, local)) => PopulatedTexelExpectation {
            depth: t / span,
            hit: Some([
                origin[0] + descent[0] * t,
                origin[1] + descent[1] * t,
                origin[2] + descent[2] * t,
            ]),
            surface,
            local,
        },
        None => PopulatedTexelExpectation {
            depth: 1.0,
            hit: None,
            surface: OccluderSurface::None,
            local: None,
        },
    }
}

/// The populated shadow-TESTING oracle (the receiver gates': a point
/// is in shadow when any static box or instance occluder stands
/// strictly beyond the self-hit epsilon along the ray toward the
/// light — the epsilon gate applies PER OCCLUDER, exactly as
/// [`shadow_ray_hit_detail`] applies it, so a receiver's own surface
/// can never mask a real occluder behind it). The ground is not an
/// occluder (nothing stands below it).
pub fn is_shadowed_populated(
    point: [f32; 3],
    cascade: &ShadowCascade,
    boxes: &[SceneBox],
    npcs: &[NpcOccluder<'_>],
    vehicles: &[VehicleOccluder<'_>],
) -> bool {
    let mut nearest = f32::INFINITY;
    for box_ in boxes {
        if let Some(t) = ray_box_entry(point, cascade.light, box_.min, box_.max)
            && t > SELF_HIT_EPSILON
        {
            nearest = nearest.min(t);
        }
    }
    for npc in npcs {
        let (bmin, bmax) = npc.world_bounds;
        if ray_box_entry(point, cascade.light, bmin, bmax).is_none() {
            continue;
        }
        let blocks = &npc.blocks[pose_slot(npc.instance)];
        for (box_index, &(lo, hi)) in HUMANOID_BOXES.iter().enumerate() {
            let frame = npc_box_frame(npc.instance, blocks, box_index);
            if let Some((t, _)) = ray_frame_box_entry(point, cascade.light, frame, lo, hi)
                && t > SELF_HIT_EPSILON
            {
                nearest = nearest.min(t);
            }
        }
    }
    for vehicle in vehicles {
        let (bmin, bmax) = vehicle.world_bounds;
        if ray_box_entry(point, cascade.light, bmin, bmax).is_none() {
            continue;
        }
        let frame = vehicle_frame(vehicle.instance);
        for (lo, hi) in [SEDAN_BODY, SEDAN_CABIN] {
            if let Some((t, _)) = ray_frame_box_entry(point, cascade.light, frame, lo, hi)
                && t > SELF_HIT_EPSILON
            {
                nearest = nearest.min(t);
            }
        }
        for (center, radius, half_width) in SEDAN_WHEELS {
            let local_origin = frame.to_local(point);
            let local_dir = frame.dir_to_local(cascade.light);
            if let Some(t) =
                ray_x_cylinder_entry(local_origin, local_dir, center, radius, half_width)
                && t > SELF_HIT_EPSILON
            {
                nearest = nearest.min(t);
            }
        }
    }
    nearest.is_finite()
}

/// The stride documentation re-export (the gate's pose-block sizing
/// arithmetic): one NPC pose block is POSE_STRIDE bytes.
#[allow(dead_code)]
const _: () = assert!(POSE_STRIDE == HUMANOID_JOINTS * 64);

/// One map texel's expected depth (the known-answer oracle): the
/// first surface the LIGHT's ray reaches, cast from the cascade's
/// far plane down along −light — exactly the first FRONT-facing
/// surface the depth-only pass rasterizes (a building's lit-side
/// entry — roof or lit wall — or the ground quad's crossing). The
/// shadow-TESTING oracle ignores the ground as an occluder; the MAP
/// oracle cannot (the mesh draws it). Depths map into [0, 1] (1 −
/// t/span from the far plane), with the clear value 1.0 for no hit.
/// `hit`/`box_index` carry the surface for the gate's edge-margin
/// skip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TexelExpectation {
    pub depth: f32,
    /// The hit point on the rasterized surface (None for no hit).
    pub hit: Option<[f32; 3]>,
    /// The hit box's table index (None = the ground quad).
    pub box_index: Option<usize>,
}

/// The expected depth the depth-only pass rasterizes at map texel
/// `(x, y)` (row 0 first — the framebuffer convention the
/// shader-boundary y flip produces; see [`texel_ray_origin`]).
pub fn map_texel_depth(
    cascade: &ShadowCascade,
    boxes: &[SceneBox],
    x: u32,
    y: u32,
    extent: u32,
) -> TexelExpectation {
    let origin = texel_ray_origin(cascade, x, y, extent);
    let span = cascade.far - cascade.near;
    // The descending light ray: −light.
    let descent = [-cascade.light[0], -cascade.light[1], -cascade.light[2]];
    // The map cast accepts t >= 0 (unlike the shadow-TESTING cast's
    // SELF_HIT_EPSILON, which exists for receiver self-hits): the
    // far plane TOUCHES the scene's max-dot surface — the tallest
    // roof — and the map legitimately rasterizes that surface at
    // depth 0, exactly where the far-plane origin sits.
    let mut best: Option<(Option<usize>, f32)> = None;
    for (index, box_) in boxes.iter().enumerate() {
        if let Some(t) = ray_box_entry(origin, descent, box_.min, box_.max)
            && t >= 0.0
            && best.is_none_or(|(_, t_best)| t < t_best)
        {
            best = Some((Some(index), t));
        }
    }
    // The ground quad: the ray descends through y = 0 within the
    // scene extent (the far-plane origins sit above/behind the
    // scene; the light is above the horizon for both presets).
    if cascade.light[1] > 0.0 && origin[1] > 0.0 {
        let t_ground = origin[1] / cascade.light[1];
        if best.is_none_or(|(_, t)| t_ground < t) {
            let crossing = [
                origin[0] + descent[0] * t_ground,
                0.0,
                origin[2] + descent[2] * t_ground,
            ];
            let inside = crossing[0].abs() <= crate::lit_scene::SCENE_EXTENT
                && crossing[2].abs() <= crate::lit_scene::SCENE_EXTENT;
            if inside && t_ground >= 0.0 {
                best = Some((None, t_ground));
            }
        }
    }
    match best {
        Some((box_index, t)) => TexelExpectation {
            // t measured from the far plane (depth 0, the light
            // side): depth = t/span — the fixed convention's form.
            depth: t / span,
            hit: Some([
                origin[0] + descent[0] * t,
                origin[1] + descent[1] * t,
                origin[2] + descent[2] * t,
            ]),
            box_index,
        },
        None => TexelExpectation {
            depth: 1.0,
            hit: None,
            box_index: None,
        },
    }
}

/// Whether a surface point is in shadow (the CPU oracle the pixel
/// gate asserts against; the shader's compare mirrors it).
pub fn is_shadowed(point: [f32; 3], cascade: &ShadowCascade, boxes: &[SceneBox]) -> bool {
    shadow_ray_hit(point, cascade, boxes).is_some()
}

/// Inverts one shadow-map texel to the world-space origin of its
/// LIGHT ray — the point on the cascade's FAR plane (the light
/// side): casting DOWN along −light from here reaches exactly the
/// surfaces the depth-only pass rasterizes (the first FRONT-facing
/// surface from the light — the lit faces), which is why the map
/// oracle casts from this side while the shadow-TESTING oracle
/// (receiver → light) casts the other way. The texel center's NDC
/// carries the shader-boundary Y flip — the naga epilogue negates
/// `gl_Position.y`, so framebuffer row j carries NDC y = −((j +
/// 0.5)·2/n − 1).
pub fn texel_ray_origin(cascade: &ShadowCascade, x: u32, y: u32, extent: u32) -> [f32; 3] {
    let ndc_x = (x as f32 + 0.5) * 2.0 / extent as f32 - 1.0;
    let ndc_y = -((y as f32 + 0.5) * 2.0 / extent as f32 - 1.0);
    let r = cascade.min_right + (ndc_x + 1.0) * 0.5 * (cascade.max_right - cascade.min_right);
    let u = cascade.min_up + (ndc_y + 1.0) * 0.5 * (cascade.max_up - cascade.min_up);
    [
        cascade.right[0] * r + cascade.up[0] * u + cascade.light[0] * cascade.far,
        cascade.right[1] * r + cascade.up[1] * u + cascade.light[1] * cascade.far,
        cascade.right[2] * r + cascade.up[2] * u + cascade.light[2] * cascade.far,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lit_scene::{day_preset, intersection_boxes};

    fn corner_cascade() -> ShadowCascade {
        let boxes = intersection_boxes();
        let (min, max) = scene_bounds(boxes);
        cascade_for(day_preset().dir_toward_light, min, max)
    }

    /// The basis is orthonormal, right-handed in (light, right, up),
    /// and the reference-axis flip engages only near-vertical.
    #[test]
    fn cascade_basis_is_orthonormal() {
        for light in [
            day_preset().dir_toward_light,
            night_preset_dir(),
            [0.0, 1.0, 0.0],
            [0.0, 0.9999, 0.0],
            [0.7, 0.7, 0.0],
        ] {
            let (min, max) = scene_bounds(intersection_boxes());
            let cascade = cascade_for(light, min, max);
            let unit = |v: [f32; 3]| (dot3(v, v)).sqrt();
            assert!((unit(cascade.light) - 1.0).abs() < 1e-6, "light unit");
            assert!((unit(cascade.right) - 1.0).abs() < 1e-6, "right unit");
            assert!((unit(cascade.up) - 1.0).abs() < 1e-6, "up unit");
            assert!(dot3(cascade.right, cascade.up).abs() < 1e-6, "r⊥u");
            assert!(dot3(cascade.light, cascade.right).abs() < 1e-6, "l⊥r");
            assert!(dot3(cascade.light, cascade.up).abs() < 1e-6, "l⊥u");
        }
        // The near-vertical light flips the reference axis (right
        // stays non-degenerate) instead of producing a zero cross.
        // With the light along +Y, +Y IS the depth axis, so the
        // map's up becomes +X (the flipped reference).
        let (min, max) = scene_bounds(intersection_boxes());
        let vertical = cascade_for([0.0, 1.0, 0.0], min, max);
        assert!(vertical.right != [0.0; 3]);
        assert_eq!(
            vertical.up,
            [1.0, 0.0, 0.0],
            "vertical light: +Y is the depth axis"
        );
    }

    fn night_preset_dir() -> [f32; 3] {
        crate::lit_scene::night_preset().dir_toward_light
    }

    /// The matrix maps the scene AABB's corners into the clip box:
    /// NDC x, y ∈ [−1, 1] and depth ∈ [0, 1], with the near-plane
    /// corner at depth 0 and the far at 1 (the [0, 1] LESS contract).
    #[test]
    fn cascade_matrix_covers_the_scene() {
        let cascade = corner_cascade();
        let (min, max) = scene_bounds(intersection_boxes());
        for corner in 0..8 {
            let world = [
                if corner & 1 == 0 { min[0] } else { max[0] },
                if corner & 2 == 0 { min[1] } else { max[1] },
                if corner & 4 == 0 { min[2] } else { max[2] },
            ];
            let clip: [f32; 4] = std::array::from_fn(|i| {
                cascade.matrix[0][i] * world[0]
                    + cascade.matrix[1][i] * world[1]
                    + cascade.matrix[2][i] * world[2]
                    + cascade.matrix[3][i]
            });
            assert_eq!(clip[3], 1.0);
            assert!(
                clip[0] >= -1.5 && clip[0] <= 1.5,
                "NDC x within the padded bounds: {}",
                clip[0]
            );
            assert!(
                clip[1] >= -1.5 && clip[1] <= 1.5,
                "NDC y within the padded bounds: {}",
                clip[1]
            );
            assert!(clip[2] >= -1e-4 && clip[2] <= 1.0 + 1e-4, "depth in [0,1]");
        }
        // A point ON the near plane maps to depth 0; light_depth
        // agrees with the matrix's z row.
        let probe = [4.0, 0.0, 4.0];
        let depth = light_depth(&cascade, probe);
        let clip_z = cascade.matrix[0][2] * probe[0]
            + cascade.matrix[1][2] * probe[1]
            + cascade.matrix[2][2] * probe[2]
            + cascade.matrix[3][2];
        assert!((depth - clip_z).abs() < 1e-6, "{depth} vs {clip_z}");
    }

    /// The CPU raycaster on synthetic geometry with hand-computed
    /// answers: a lone 2 m cube under a vertical and a horizontal
    /// light, the self-hit epsilon, and the nearest-of-two
    /// tie-break (table order).
    #[test]
    fn raycaster_pins_synthetic_geometry() {
        let cube = SceneBox {
            min: [0.0, 0.0, 0.0],
            max: [2.0, 2.0, 2.0],
            material: 0,
        };
        let vertical = cascade_for([0.0, 1.0, 0.0], [-1.0, -1.0, -1.0], [3.0, 3.0, 3.0]);
        // Below the cube, light overhead: the cube's shadow.
        assert!(is_shadowed([1.0, -1.0, 1.0], &vertical, &[cube]));
        // Above the cube: the ray leaves toward the light.
        assert!(!is_shadowed([1.0, 3.0, 1.0], &vertical, &[cube]));
        // Beside the cube: clear sky.
        assert!(!is_shadowed([3.0, 1.0, 1.0], &vertical, &[cube]));
        // ON the cube's bottom face: its own surface at t = 0 — the
        // epsilon skips it (no self-shadowing).
        assert!(!is_shadowed([1.0, 0.0, 1.0], &vertical, &[cube]));
        // Inside the cube: entry at t = 0, skipped the same way.
        assert!(!is_shadowed([1.0, 1.0, 1.0], &vertical, &[cube]));

        let east = cascade_for([1.0, 0.0, 0.0], [-3.0, -1.0, -1.0], [3.0, 3.0, 3.0]);
        // West of the cube, light due east: shadowed (entry at t=1).
        assert_eq!(shadow_ray_hit([-1.0, 1.0, 1.0], &east, &[cube]), Some(1.0));
        // East of it: lit.
        assert!(!is_shadowed([3.0, 1.0, 1.0], &east, &[cube]));
        // ON the west face: t = 0, the epsilon skip.
        assert_eq!(
            shadow_ray_hit([0.0, 1.0, 1.0], &east, &[cube]),
            None,
            "the receiver's own face is not an occluder"
        );

        // Nearest of two occluders wins; equal distances resolve to
        // the earlier table entry (both hit at the same t here, the
        // first listed is reported — the deterministic tie-break).
        let near = SceneBox {
            min: [1.0, 0.0, 0.0],
            max: [2.0, 2.0, 2.0],
            material: 0,
        };
        let far = SceneBox {
            min: [3.0, 0.0, 0.0],
            max: [4.0, 2.0, 2.0],
            material: 0,
        };
        assert_eq!(
            shadow_ray_hit([-1.0, 1.0, 1.0], &east, &[far, near]),
            Some(2.0),
            "the nearer box's entry distance"
        );
    }

    /// The corner scene under the day preset: the 45° sun with the
    /// 22 m southwest mass shadows the cross center (the scene's
    /// signature shadow — the reverse ray reaches the mass's near
    /// wall plane at y = 8.49 < 22), the scene has BOTH lit and
    /// shadowed ground (a scene where everything is one or the other
    /// would make the pixel gate vacuous), and a point inside a mass
    /// does not self-shadow.
    #[test]
    fn raycaster_pins_the_corner_shadows() {
        let boxes = intersection_boxes();
        let cascade = corner_cascade();
        // The center: shadowed by the SW mass's near edge.
        assert!(
            is_shadowed([0.0, 0.0, 0.0], &cascade, boxes),
            "the day sun's signature shadow falls on the cross center"
        );
        // Deep inside the tall SW mass: only itself along the ray.
        assert!(!is_shadowed([-20.0, 10.0, -20.0], &cascade, boxes));
        // Both regimes exist on the ground: sample the road cross
        // and the world square's open ring outside the masses.
        let mut lit = 0;
        let mut shadowed = 0;
        for x in -39..=39 {
            for z in -39..=39 {
                let point = [x as f32, 0.0, z as f32];
                let inside = boxes.iter().any(|b| {
                    b.min[0] < point[0]
                        && point[0] < b.max[0]
                        && b.min[2] < point[2]
                        && point[2] < b.max[2]
                });
                if inside {
                    continue;
                }
                if is_shadowed(point, &cascade, boxes) {
                    shadowed += 1;
                } else {
                    lit += 1;
                }
            }
        }
        assert!(lit > 200, "lit ground samples: {lit}");
        assert!(shadowed > 200, "shadowed ground samples: {shadowed}");
    }

    /// The texel inversion is a true inverse of the matrix: a
    /// texel's ray origin maps back into the same texel's NDC cell
    /// (x within the cell, y within the cell AFTER the flip), at
    /// depth 1 (the far plane — the light side the oracle casts
    /// from).
    #[test]
    fn texel_ray_round_trips_through_the_matrix() {
        let cascade = corner_cascade();
        let extent = 64u32;
        for (x, y) in [(0u32, 0u32), (31, 32), (63, 63), (17, 40)] {
            let origin = texel_ray_origin(&cascade, x, y, extent);
            let clip: [f32; 4] = std::array::from_fn(|i| {
                cascade.matrix[0][i] * origin[0]
                    + cascade.matrix[1][i] * origin[1]
                    + cascade.matrix[2][i] * origin[2]
                    + cascade.matrix[3][i]
            });
            // The cell's x range (before the y flip) and the flipped
            // y range the framebuffer row sees: NDC → texel centers.
            let cell = |v: f32, n: u32| (v + 1.0) * n as f32 / 2.0 - 0.5;
            let cx = cell(clip[0], extent);
            let cy = cell(-clip[1], extent);
            assert!((cx - x as f32).abs() < 1e-3, "texel x {x}: {cx}");
            assert!((cy - y as f32).abs() < 1e-3, "texel y {y}: {cy}");
            assert!(
                (clip[2] - 0.0).abs() < 1e-6,
                "the origin sits on the far plane (depth 0, closest to the light)"
            );
        }
    }

    /// The bias model: facing surfaces get the constant floor,
    /// grazing surfaces grow (the peter-panning trade), and the
    /// parameters pass through linearly.
    #[test]
    fn depth_bias_scales_with_grazing() {
        let light = [0.0, 1.0, 0.0];
        let facing = depth_bias([0.0, 1.0, 0.0], light, 0.001, 0.005);
        assert!((facing - 0.001).abs() < 1e-9, "the constant floor");
        let grazing = depth_bias([1.0, 0.0, 0.0], light, 0.001, 0.005);
        assert!(grazing > facing * 10.0, "grazing surfaces bias hard");
        let doubled = depth_bias([0.0, 1.0, 0.0], light, 0.002, 0.005);
        assert!(
            (doubled - 0.002).abs() < 1e-9,
            "the constant passes through"
        );
    }

    /// The map oracle on the synthetic cube (hand-computed): a
    /// vertical light over a 2 m cube inside a [−1, 3]³ AABB — the
    /// cascade's basis is right = +Z, up = +X (the reference flip),
    /// near = −1, far = 3, span = 4. The center texel's ray descends
    /// from the far plane (y = 3) onto the cube's ROOF (y = 2) at
    /// t = 1 → depth 1/4 = 0.25 — the first front-facing surface
    /// from the light (the closest-to-light occluder wins the LESS
    /// depth test), exactly what the depth-only pass rasterizes.
    /// Determinism is pinned alongside.
    #[test]
    fn map_texel_depth_pins_the_synthetic_cube() {
        let cube = SceneBox {
            min: [0.0, 0.0, 0.0],
            max: [2.0, 2.0, 2.0],
            material: 0,
        };
        let cascade = cascade_for([0.0, 1.0, 0.0], [-1.0, -1.0, -1.0], [3.0, 3.0, 3.0]);
        assert_eq!(cascade.right, [0.0, 0.0, 1.0]);
        assert_eq!(cascade.up, [1.0, 0.0, 0.0]);
        assert!((cascade.near + 1.0).abs() < 1e-6);
        assert!((cascade.far - 3.0).abs() < 1e-6);
        let n = 64;
        let center = map_texel_depth(&cascade, &[cube], n / 2, n / 2, n);
        assert!((center.depth - 0.25).abs() < 1e-4, "{}", center.depth);
        assert_eq!(center.box_index, Some(0));
        let hit = center.hit.unwrap();
        assert!((hit[1] - 2.0).abs() < 1e-3, "the cube's roof");
        // A corner texel off the cube: the ground quad's crossing
        // (never the clear value inside the extent — the ray
        // descends through y = 0).
        let corner = map_texel_depth(&cascade, &[cube], 0, 0, n);
        assert!(corner.depth >= 0.0 && corner.depth <= 1.0);
        assert_eq!(corner.box_index, None, "the ground, not the cube");
        // Determinism.
        assert_eq!(map_texel_depth(&cascade, &[cube], n / 2, n / 2, n), center);
    }

    /// The corner scene's map oracle covers every expectation kind
    /// (box hits, ground hits) with depths in [0, 1] — the coarse
    /// census the GPU gate's full-map comparison rests on.
    #[test]
    fn map_texel_depth_censuses_the_corner_scene() {
        let boxes = intersection_boxes();
        let cascade = corner_cascade();
        let n = 128;
        let mut box_hits = 0;
        let mut ground_hits = 0;
        for y in (0..n).step_by(3) {
            for x in (0..n).step_by(3) {
                let texel = map_texel_depth(&cascade, boxes, x, y, n);
                assert!(texel.depth >= 0.0 && texel.depth <= 1.0);
                match texel.box_index {
                    Some(index) => {
                        let hit = texel.hit.unwrap();
                        let box_ = &boxes[index];
                        for axis in 0..3 {
                            assert!(
                                hit[axis] >= box_.min[axis] - 1e-3
                                    && hit[axis] <= box_.max[axis] + 1e-3,
                                "the hit lies on its box: {hit:?}"
                            );
                        }
                        box_hits += 1;
                    }
                    None if texel.hit.is_some() => {
                        assert!(texel.hit.unwrap()[1].abs() < 1e-3, "ground hits lie on y=0");
                        ground_hits += 1;
                    }
                    None => {}
                }
            }
        }
        assert!(box_hits > 100, "box texels: {box_hits}");
        // The ground visible FROM THE LIGHT is the lit ground — a
        // minority of the map (the buildings occlude the rest from
        // the light's viewpoint; that occlusion IS the shadow map's
        // content), so the sampled bound is small by design.
        assert!(ground_hits > 30, "ground texels: {ground_hits}");
    }

    // ============================================================
    // The populated-oracle battery (the Era-2 proxy-shadows stage):
    // the structured frame's hand answers (the transpose class this
    // module's own Frame3 initially got wrong — the known answer
    // exists to catch it), the OBB and cylinder ray tests, the
    // bounds-cull safety, and the populated map/receiver oracles on
    // synthetic geometry and a spread population.
    // ============================================================

    use super::{
        Frame3, NpcOccluder, OccluderSurface, VehicleOccluder, is_shadowed_populated,
        map_texel_depth_populated, npc_box_frame, ray_frame_box_entry, ray_x_cylinder_entry,
        texel_ray_origin,
    };
    use crate::animation::{HUMANOID_BOXES, rest_pose_block, skin_matrices};
    use crate::proxy_instances::{InstanceTransform, ProxyInstance, instance_columns};

    fn identity_frame(translation: [f32; 3], scale: f32) -> Frame3 {
        Frame3 {
            rotation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            translation,
            scale,
        }
    }

    fn quarter_yaw_frame() -> Frame3 {
        // instance_columns at a quarter turn, scale 1, at the origin.
        let cols = instance_columns(&InstanceTransform {
            pos: [0.0; 3],
            turn: 16384,
            scale: 1.0,
        });
        Frame3 {
            rotation: [
                cols[0][0], cols[0][1], cols[0][2], cols[1][0], cols[1][1], cols[1][2], cols[2][0],
                cols[2][1], cols[2][2],
            ],
            translation: [cols[3][0], cols[3][1], cols[3][2]],
            scale: 1.0,
        }
    }

    /// THE TRANSPOSE PIN (the class this module's own Frame3 first
    /// got wrong): the local mapping is Rᵀ·(p − t)/s — under the
    /// quarter yaw (which maps local +X to world −Z) the inverse
    /// maps the world −Z back to local +X; the forward rotation maps
    /// +X to −Z; and the two compose to the identity round trip.
    #[test]
    fn frame3_local_mapping_known_answers() {
        // The identity frame: pure translation and scale.
        let frame = identity_frame([1.0, 2.0, 3.0], 2.0);
        let local = frame.to_local([3.0, 5.0, 7.0]);
        assert!((local[0] - 1.0).abs() < 1e-6, "{local:?}");
        assert!((local[1] - 1.5).abs() < 1e-6, "{local:?}");
        assert!((local[2] - 2.0).abs() < 1e-6, "{local:?}");

        // The quarter yaw: world −Z is local +X, and the forward
        // rotation maps local +X to world −Z.
        let frame = quarter_yaw_frame();
        let local = frame.to_local([0.0, 0.0, -1.0]);
        assert!(
            (local[0] - 1.0).abs() < 2e-5,
            "the inverse maps −Z to +X: {local:?}"
        );
        assert!(local[1].abs() < 2e-5 && local[2].abs() < 2e-5, "{local:?}");
        let forward = frame.rotate([1.0, 0.0, 0.0]);
        assert!(
            (forward[2] + 1.0).abs() < 2e-5,
            "the rotation maps +X to −Z: {forward:?}"
        );

        // The round trip on an off-axis vector (Taylor tolerance).
        let v = [0.3, -0.7, 0.2];
        let world = frame.rotate(v);
        let back = frame.to_local(world);
        for axis in 0..3 {
            assert!((back[axis] - v[axis]).abs() < 2e-5, "axis {axis}: {back:?}");
        }
    }

    /// Ray vs the authored box under a frame: the identity case's
    /// hand answer, and the ROTATED case (the box extending along
    /// local +X, the ray descending along world −Z, the quarter yaw
    /// exchanging the axes — the entry lands on the local x = 0
    /// face at the hand-computed distance).
    #[test]
    fn ray_frame_box_entry_known_answers() {
        let frame = identity_frame([0.0; 3], 1.0);
        let hit = ray_frame_box_entry(
            [5.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            frame,
            [-1.0, -1.0, -1.0],
            [1.0, 1.0, 1.0],
        )
        .expect("the axis ray hits");
        assert!((hit.0 - 4.0).abs() < 1e-6, "entry at t=4: {}", hit.0);
        assert!(
            (hit.1[0] - 1.0).abs() < 1e-6,
            "the local hit on the +X face"
        );

        // The rotated case: local box [0..2]×[−1..1]×[−1..1]; the ray
        // descends along −Z from (0, 0, 5). Under the quarter yaw
        // (local +X → world −Z), the local ray runs along +X from
        // local (−5, 0, 0) — the entry is the local x = 0 face at
        // world distance 5 (the world z = 0 plane).
        let frame = quarter_yaw_frame();
        let hit = ray_frame_box_entry(
            [0.0, 0.0, 5.0],
            [0.0, 0.0, -1.0],
            frame,
            [0.0, -1.0, -1.0],
            [2.0, 1.0, 1.0],
        )
        .expect("the rotated ray hits");
        assert!((hit.0 - 5.0).abs() < 2e-5, "entry at world t=5: {}", hit.0);
        assert!(
            hit.1[0].abs() < 2e-5,
            "the local hit on the x=0 face: {:?}",
            hit.1
        );
        assert!(
            hit.1[1].abs() < 2e-5 && hit.1[2].abs() < 2e-5,
            "{:?}",
            hit.1
        );
    }

    /// Ray vs the X-axis cylinder: the tread entry from above, the
    /// cap entry along the axle, and the clean miss — hand-computed.
    #[test]
    fn ray_x_cylinder_entry_known_answers() {
        // From above, dead center: the tread top at y=+1 from y=5.
        let t = ray_x_cylinder_entry([0.0, 5.0, 0.0], [0.0, -1.0, 0.0], [0.0; 3], 1.0, 0.5);
        assert!((t.unwrap() - 4.0).abs() < 1e-6, "the tread entry: {t:?}");

        // Along the axle from inside the circle: the near cap at
        // x = −0.5 from x = −3.
        let t = ray_x_cylinder_entry([-3.0, 0.0, 0.5], [1.0, 0.0, 0.0], [0.0; 3], 1.0, 0.5);
        assert!((t.unwrap() - 2.5).abs() < 1e-6, "the cap entry: {t:?}");

        // The parallel miss (outside the circle):
        let t = ray_x_cylinder_entry([-3.0, 0.0, 2.0], [1.0, 0.0, 0.0], [0.0; 3], 1.0, 0.5);
        assert!(t.is_none(), "the parallel ray outside the circle");

        // The descend miss (the ray's z offset exceeds the radius):
        let t = ray_x_cylinder_entry([0.0, 5.0, 3.0], [0.0, -1.0, 0.0], [0.0; 3], 1.0, 0.5);
        assert!(t.is_none(), "the offset ray misses the tread");
    }

    /// A rest-pose NPC's world AABB equals the authored mesh bounds
    /// translated; a posed NPC's bounds CONTAIN every posed box's
    /// corners (the cull-safety property: every ray that hits any
    /// posed box also hits the world AABB — the cull can only skip
    /// misses, never a hit).
    #[test]
    fn npc_world_bounds_and_cull_safety() {
        let rest = rest_pose_block();
        let blocks = [rest];
        let npc = ProxyInstance {
            transform: InstanceTransform {
                pos: [10.0, 0.0, -10.0],
                turn: 0,
                scale: 1.0,
            },
            tint: [1.0; 3],
            alpha: 1.0,
            pose_index: 0,
        };
        let occluder = NpcOccluder::new(&npc, &blocks);
        let (min, max) = occluder.world_bounds;
        // The v2 humanoid's own extents: |x| ≤ 0.33, y ∈ [0, 1.8],
        // |z| ≤ 0.12 (the torso — the placement table's 0.36/0.15
        // numbers are the bounding-CIRCLE radii, a different thing
        // than the axis-aligned extents).
        assert!((max[0] - (10.0 + 0.33)).abs() < 1e-4, "{max:?}");
        assert!(
            min[1].abs() < 1e-4 && (max[1] - 1.8).abs() < 1e-4,
            "{min:?} {max:?}"
        );
        assert!((min[2] - (-10.0 - 0.12)).abs() < 1e-4, "{min:?}");
        assert!((max[2] - (-10.0 + 0.12)).abs() < 1e-4, "{max:?}");

        // The cull-safety containment over a posed walk sample.
        let pose = crate::animation::sample_pose(&crate::animation::walk_clip(), 0.3);
        let joint_worlds =
            crate::animation::compose_joint_matrices(&crate::animation::humanoid_skeleton(), &pose);
        let posed = skin_matrices(&joint_worlds, &crate::animation::humanoid_skeleton());
        let posed_blocks = [posed];
        let occluder = NpcOccluder::new(&npc, &posed_blocks);
        let mut box_hits = 0;
        let mut culled_would_hide = 0;
        // Rays over a patch of the ground around the NPC, upward.
        for x in -60..60 {
            for z in -60..60 {
                let origin = [10.0 + x as f32 * 0.05, 0.0, -10.0 + z as f32 * 0.05];
                let up = [0.0, 1.0, 0.0];
                let mut any_box = None;
                for (box_index, &(lo, hi)) in HUMANOID_BOXES.iter().enumerate() {
                    let frame = npc_box_frame(&npc, &posed_blocks[0], box_index);
                    if let Some((t, _)) = ray_frame_box_entry(origin, up, frame, lo, hi) {
                        any_box = Some(any_box.map_or(t, |best: f32| best.min(t)));
                    }
                }
                if any_box.is_some() {
                    box_hits += 1;
                    let (bmin, bmax) = occluder.world_bounds;
                    if ray_box_entry(origin, up, bmin, bmax).is_none() {
                        culled_would_hide += 1;
                    }
                }
            }
        }
        assert!(box_hits > 50, "the patch crosses the figure: {box_hits}");
        assert_eq!(
            culled_would_hide, 0,
            "the bounds cull hid {culled_would_hide} real box hits — the containment is broken"
        );
    }

    /// The populated map oracle on synthetic geometry: a vertical
    /// light over one rest-pose NPC at the origin — the center texel
    /// (whose ray descends through the figure's axis) hits the HEAD
    /// roof (emission box 0) at the hand-computed depth, where the
    /// UNPOPULATED oracle reads the ground; the differential is the
    /// pin (adding the instance changes the texel's surface class
    /// and depth exactly as authored).
    #[test]
    fn map_texel_depth_populated_synthetic() {
        let rest = rest_pose_block();
        let blocks = [rest];
        let npc = ProxyInstance {
            transform: InstanceTransform {
                pos: [0.0, 0.0, 0.0],
                turn: 0,
                scale: 1.0,
            },
            tint: [1.0; 3],
            alpha: 1.0,
            pose_index: 0,
        };
        let npcs = [NpcOccluder::new(&npc, &blocks)];
        // The vertical cascade over the AABB [−2..2]×[0..2]×[−2..2]:
        // right = +Z, up = +X (the reference flip), near 0, far 2.
        let cascade = cascade_for([0.0, 1.0, 0.0], [-2.0, 0.0, -2.0], [2.0, 2.0, 2.0]);
        let n = 64;
        // The ground-only oracle at the center texel: depth 1.0 (the
        // ground at t = span = 2) — no static boxes.
        let unpopulated = map_texel_depth_populated(&cascade, &[], &[], &[], n / 2, n / 2, n);
        assert_eq!(unpopulated.surface, OccluderSurface::Ground);
        assert!(
            (unpopulated.depth - 1.0).abs() < 1e-4,
            "{}",
            unpopulated.depth
        );

        // With the NPC: the head roof (y = 1.80) at t = 2 − 1.8 =
        // 0.2, depth 0.2/2 = 0.1.
        let populated = map_texel_depth_populated(&cascade, &[], &npcs, &[], n / 2, n / 2, n);
        assert_eq!(
            populated.surface,
            OccluderSurface::NpcBox(0, 0),
            "the center ray hits the head (emission box 0): {:?}",
            populated.surface
        );
        assert!((populated.depth - 0.1).abs() < 1e-3, "{}", populated.depth);
        let hit = populated.hit.expect("the hit point exists");
        assert!((hit[1] - 1.8).abs() < 1e-2, "the head roof: {hit:?}");
        // The local-margin inputs: the local hit on the box's roof
        // plane (local y = hi[1] = 1.80).
        let (local, _lo, hi) = populated.local.expect("the OBB hit carries local inputs");
        assert!(
            (local[1] - hi[1]).abs() < 1e-2,
            "the local roof plane: {local:?}"
        );

        // Determinism.
        assert_eq!(
            map_texel_depth_populated(&cascade, &[], &npcs, &[], n / 2, n / 2, n),
            populated
        );

        // The pose-index indirection: two NPCs sharing ONE pose block
        // (both pose_index 0 — the dedup shape) read the same block.
        let npc_b = ProxyInstance {
            transform: InstanceTransform {
                pos: [1.5, 0.0, 0.0],
                turn: 0,
                scale: 1.0,
            },
            tint: [1.0; 3],
            alpha: 1.0,
            pose_index: 0,
        };
        let two = [
            NpcOccluder::new(&npc, &blocks),
            NpcOccluder::new(&npc_b, &blocks),
        ];
        // The texel over NPC B's head (B at world (1.5, ·, 0); the
        // vertical cascade's RIGHT axis is world +Z (the map's x)
        // and its UP axis is world +X (the map's y, y-flipped) — so
        // world x = 1.5 is texel y = 8 and world z = 0 is texel x =
        // 32): B is list index 1, reading the SHARED block 0 (the
        // dedup shape — both pose_index 0).
        let b = map_texel_depth_populated(&cascade, &[], &two, &[], 32, 8, n);
        assert_eq!(
            b.surface,
            OccluderSurface::NpcBox(1, 0),
            "npc B's head, through the shared pose block"
        );
    }

    /// The populated receiver oracle on synthetic geometry: the
    /// ground under the NPC is shadowed, the ground beside it is
    /// lit, the NPC's own roof is not self-shadowed (the per-occluder
    /// epsilon), and the sedan's body and wheel both shadow the
    /// ground under them.
    #[test]
    fn is_shadowed_populated_synthetic() {
        let rest = rest_pose_block();
        let blocks = [rest];
        let npc = ProxyInstance {
            transform: InstanceTransform {
                pos: [0.0, 0.0, 0.0],
                turn: 0,
                scale: 1.0,
            },
            tint: [1.0; 3],
            alpha: 1.0,
            pose_index: 0,
        };
        let npcs = [NpcOccluder::new(&npc, &blocks)];
        let cascade = cascade_for([0.0, 1.0, 0.0], [-2.0, 0.0, -2.0], [2.0, 2.0, 2.0]);
        assert!(
            is_shadowed_populated([0.0, 0.0, 0.0], &cascade, &[], &npcs, &[]),
            "the ground under the figure is shadowed"
        );
        assert!(
            !is_shadowed_populated([1.9, 0.0, 1.9], &cascade, &[], &npcs, &[]),
            "the ground away from the figure is lit"
        );
        assert!(
            !is_shadowed_populated([0.0, 1.8, 0.0], &cascade, &[], &npcs, &[]),
            "the head's own roof is not self-shadowed"
        );

        let vehicle = ProxyInstance {
            transform: InstanceTransform {
                pos: [0.0, 0.0, 0.0],
                turn: 0,
                scale: 1.0,
            },
            tint: [1.0; 3],
            alpha: 1.0,
            pose_index: 0,
        };
        let vehicles = [VehicleOccluder::new(&vehicle)];
        assert!(
            is_shadowed_populated([0.0, 0.0, 0.0], &cascade, &[], &[], &vehicles),
            "the ground under the sedan's body is shadowed"
        );
        assert!(
            is_shadowed_populated([0.85, 0.0, 1.30], &cascade, &[], &[], &vehicles),
            "the ground beside a wheel's contact is inside the wheel's shadow"
        );
        assert!(
            !is_shadowed_populated([2.0, 0.0, 0.0], &cascade, &[], &[], &vehicles),
            "the ground beside the sedan is lit"
        );
    }

    /// A spread population's populated-map census: the surface
    /// classes all occur (buildings, ground, NPC boxes, vehicle
    /// parts), depths stay in [0, 1], and the cull's affordability
    /// evidence prints (the sampled instance-box test counts — the
    /// measured-first discipline; a BVH only if these numbers demand
    /// it). The FULL 24-NPC corner table's census belongs to the
    /// lavapipe gate (engine-slice owns the placement table); this
    /// unit census pins the oracle's coverage classes on a local
    /// spread with varied facings and placements.
    #[test]
    fn map_texel_depth_populated_censuses_a_spread() {
        let rest = rest_pose_block();
        let blocks = [rest]; // every npc shares pose block 0 (the dedup shape)
        let instance = |pos: [f32; 3], turn: u16| ProxyInstance {
            transform: InstanceTransform {
                pos,
                turn,
                scale: 1.0,
            },
            tint: [1.0; 3],
            alpha: 1.0,
            pose_index: 0,
        };
        let npc_table = [
            instance([-5.0, 0.02, -4.5], 0),
            instance([-5.0, 0.02, 4.5], 32768),
            instance([-4.5, 0.02, 5.1], 16384),
            instance([4.5, 0.02, 5.1], 49152),
            instance([-5.0, 0.02, 1.5], 0),
            instance([5.0, 0.02, 8.0], 0),
            instance([-7.5, 0.02, 5.0], 16384),
            instance([-7.0, 0.02, -5.0], 16384),
            // Two far-EAST-arm placements — the SW mass's
            // northeast-falling swept shadow covers the whole road
            // cross (its 34×34×22 m volume Minkowski-swept by the
            // 45° sun reaches ≈16 m past every near edge; the first
            // census runs measured the north, south, and west arms
            // ALL shadowed — the geometry lesson, recorded here), and
            // the east arm beyond it is the scene's lit open road.
            // Probed: the texels over these centers hit the npcs'
            // pelvis and torso boxes directly.
            instance([25.0, 0.02, 0.0], 49152),
            instance([30.0, 0.02, 1.5], 32768),
        ];
        let vehicle_table = [
            instance([9.0, 0.0, 2.0], 49152),
            instance([2.0, 0.0, 9.0], 0),
        ];
        let npcs: Vec<_> = npc_table
            .iter()
            .map(|instance| NpcOccluder::new(instance, &blocks))
            .collect();
        let vehicles: Vec<_> = vehicle_table.iter().map(VehicleOccluder::new).collect();
        let boxes = intersection_boxes();
        let cascade = corner_cascade();
        // A 512-map at step 4 (≈0.16 m texels over the 40 m scene):
        // the coarse 128-map's 0.6 m texels cannot reliably intersect
        // a 0.66 m-wide NPC (the first census runs measured zero npc
        // texels there — the density lesson, recorded here).
        let n = 512;
        let mut census = std::collections::BTreeMap::new();
        let mut npc_box_tests = 0usize;
        let mut npc_cull_skips = 0usize;
        for y in (0..n).step_by(4) {
            for x in (0..n).step_by(4) {
                let texel = map_texel_depth_populated(&cascade, boxes, &npcs, &vehicles, x, y, n);
                assert!(texel.depth >= 0.0 && texel.depth <= 1.0, "{}", texel.depth);
                let key = match texel.surface {
                    OccluderSurface::None => "clear",
                    OccluderSurface::Ground => "ground",
                    OccluderSurface::Building(_) => "building",
                    OccluderSurface::NpcBox(..) => "npc",
                    OccluderSurface::VehiclePart(..) => "vehicle",
                };
                *census.entry(key).or_insert(0usize) += 1;
                // The cull's accounting: an npc whose bounds the
                // texel ray misses contributes no box tests.
                let origin = texel_ray_origin(&cascade, x, y, n);
                let descent = [-cascade.light[0], -cascade.light[1], -cascade.light[2]];
                for npc in &npcs {
                    let (bmin, bmax) = npc.world_bounds;
                    if ray_box_entry(origin, descent, bmin, bmax).is_none() {
                        npc_cull_skips += 1;
                    } else {
                        npc_box_tests += HUMANOID_BOXES.len();
                    }
                }
            }
        }
        let sampled = (0..n).step_by(4).count().pow(2);
        // The guaranteed per-instance coverage: the texel whose ray
        // passes through each instance's world center (computed by
        // the cascade's own inversion — the texel_ray_origin round
        // trip), queried alongside the sweep. A standing figure is
        // THIN (0.24 m deep at rest), so a stepped sweep can miss it
        // (the first runs measured zero npc texels at two densities —
        // the density lesson); the targeted texels make the npc
        // coverage deterministic.
        let mut targeted = 0usize;
        let mut npc_hits = 0usize;
        for (index, npc) in npcs.iter().enumerate() {
            let center = [
                (npc.world_bounds.0[0] + npc.world_bounds.1[0]) * 0.5,
                (npc.world_bounds.0[1] + npc.world_bounds.1[1]) * 0.5,
                (npc.world_bounds.0[2] + npc.world_bounds.1[2]) * 0.5,
            ];
            // The light-space projection of the center → the texel.
            let clip: [f32; 4] = std::array::from_fn(|i| {
                cascade.matrix[0][i] * center[0]
                    + cascade.matrix[1][i] * center[1]
                    + cascade.matrix[2][i] * center[2]
                    + cascade.matrix[3][i]
            });
            let tx = ((clip[0] + 1.0) * 0.5 * n as f32) as u32;
            let ty = ((1.0 - (clip[1] + 1.0) * 0.5) * n as f32) as u32;
            let (tx, ty) = (tx.min(n - 1), ty.min(n - 1));
            let texel = map_texel_depth_populated(&cascade, boxes, &npcs, &vehicles, tx, ty, n);
            // The texel over the center hits the npc when the light
            // sees it, or a BUILDING when the npc stands in a
            // building's shadow (the map correctly records the
            // nearer occluder — the npc's shadowed-ness is the
            // RECEIVER oracle's assertion, not this one). Anything
            // else (ground/clear under a 1.8 m figure) is a bug.
            assert!(
                matches!(texel.surface, OccluderSurface::NpcBox(i, _) if i == index)
                    || matches!(texel.surface, OccluderSurface::Building(_)),
                "the texel over npc {index}'s center hits that npc or its shadowing \
                 building: {:?} at ({tx}, {ty})",
                texel.surface
            );
            if matches!(texel.surface, OccluderSurface::NpcBox(..)) {
                npc_hits += 1;
            }
            targeted += 1;
        }
        for (index, vehicle) in vehicles.iter().enumerate() {
            let center = [
                (vehicle.world_bounds.0[0] + vehicle.world_bounds.1[0]) * 0.5,
                (vehicle.world_bounds.0[1] + vehicle.world_bounds.1[1]) * 0.5,
                (vehicle.world_bounds.0[2] + vehicle.world_bounds.1[2]) * 0.5,
            ];
            let clip: [f32; 4] = std::array::from_fn(|i| {
                cascade.matrix[0][i] * center[0]
                    + cascade.matrix[1][i] * center[1]
                    + cascade.matrix[2][i] * center[2]
                    + cascade.matrix[3][i]
            });
            let tx = ((clip[0] + 1.0) * 0.5 * n as f32) as u32;
            let ty = ((1.0 - (clip[1] + 1.0) * 0.5) * n as f32) as u32;
            let (tx, ty) = (tx.min(n - 1), ty.min(n - 1));
            let texel = map_texel_depth_populated(&cascade, boxes, &npcs, &vehicles, tx, ty, n);
            // The same shadow-truth as the npcs: a building between
            // the light and the vehicle means the map records the
            // building (the vehicle stands in its shadow — the
            // westbound sedan at (9, ·, 2) sits in the SW mass's
            // band, MEASURED here; the map oracle is correct).
            assert!(
                matches!(texel.surface, OccluderSurface::VehiclePart(i, _) if i == index)
                    || matches!(texel.surface, OccluderSurface::Building(_)),
                "the texel over vehicle {index}'s center hits that vehicle or its \
                 shadowing building: {:?} at ({tx}, {ty})",
                texel.surface
            );
            targeted += 1;
        }
        // The far-east-arm placements guarantee the light sees SOME npc
        // directly (the map records its boxes, not just its shadow).
        assert!(
            npc_hits >= 2,
            "at least the far-SE npcs are directly visible to the light: {npc_hits}"
        );
        println!(
            "populated map census ({sampled} swept texels + {targeted} targeted, \
             {npc_hits} directly-visible npc centers): {census:?}; \
             npc box tests {npc_box_tests}, cull skips {npc_cull_skips} (per swept texel: {:.1} \
             tests, {:.1} skips)",
            npc_box_tests as f32 / sampled as f32,
            npc_cull_skips as f32 / sampled as f32
        );
        assert!(census["building"] > 100, "building texels: {:?}", census);
        assert!(census["ground"] > 30, "ground texels: {:?}", census);
    }
}
