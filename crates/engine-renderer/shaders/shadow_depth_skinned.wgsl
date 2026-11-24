// The Cycle 6-F skinned depth-only shadow-pass shader (Era-2 stage
// 2B): the NPC cast into the shadow map, the skin matrices applied
// through the storage buffer (binding 5 — the 6-B binding map's
// reserved engine-wide number for the pose buffer).
//
// TRANSPORT: the pinned naga 29.0.4's wgsl-in has no
// var<push_constant> (the probed finding) — the light matrix rides a
// uniform buffer at binding 0 of the shadow pipeline's own set. The
// vertex format extends the lit pipeline's (pos/normal/uv, locations
// 0-2) with the skinning inputs (joints/weights at 8-9, i_pose at
// 10); the mesh VBO is the skinned proxy mesh.
//
// The build-time ADJUST_COORDINATE_SPACE epilogue negates
// gl_Position.y here as in every vertex shader — the shadow map's
// rows therefore carry the FLIPPED light-space NDC y, which the
// CPU-side texel inversion (shadows::texel_ray_origin) and the
// shadowed lit shader's lookup both account for.
//
// BINDINGS:
//   0: LightMatrix uniform (light_view_proj mat4x4<f32>)
//   5: poses storage buffer (array<mat4x4<f32>, read-only> — the skin matrices)

struct LightMatrix {
    light_view_proj: mat4x4<f32>,
};
@group(0) @binding(0)
var<uniform> light: LightMatrix;

// The pose storage buffer: per-instance skin matrices (the 6-B
// binding map reserves 5 for this purpose — the Era-2 skinning
// convention).
@group(0) @binding(5)
var<storage, read> poses: array<mat4x4<f32>>;

@vertex
fn vs_main(@location(0) pos: vec3<f32>,
           @location(1) normal: vec3<f32>,
           @location(2) uv: vec2<f32>,
           @location(3) i_col0: vec4<f32>,
           @location(4) i_col1: vec4<f32>,
           @location(5) i_col2: vec4<f32>,
           @location(6) i_col3: vec4<f32>,
           @location(7) i_tint: vec4<f32>,
           @location(8) joints: vec4<u32>,
           @location(9) weights: vec4<f32>,
           @location(10) i_pose: vec4<f32>)
    -> @builtin(position) vec4<f32> {
    // The skinning equation: per-term full mat4*vec4 transforms,
    // weight scaling, left-to-right sums, normalize after the sum.
    let base = u32(i_pose.x) * 11u;
    let m0 = poses[base + joints.x];
    let m1 = poses[base + joints.y];
    let m2 = poses[base + joints.z];
    let m3 = poses[base + joints.w];
    let p0 = m0 * vec4<f32>(pos, 1.0);
    let p1 = m1 * vec4<f32>(pos, 1.0);
    let p2 = m2 * vec4<f32>(pos, 1.0);
    let p3 = m3 * vec4<f32>(pos, 1.0);
    let skinned = weights.x * p0 + weights.y * p1 + weights.z * p2 + weights.w * p3;
    let n0 = mat3x3<f32>(m0[0].xyz, m0[1].xyz, m0[2].xyz) * normal;
    let n1 = mat3x3<f32>(m1[0].xyz, m1[1].xyz, m1[2].xyz) * normal;
    let n2 = mat3x3<f32>(m2[0].xyz, m2[1].xyz, m2[2].xyz) * normal;
    let n3 = mat3x3<f32>(m3[0].xyz, m3[1].xyz, m3[2].xyz) * normal;
    let skinned_n = normalize(weights.x * n0 + weights.y * n1 + weights.z * n2 + weights.w * n3);
    // The instance transform: columns are the T*R*S basis images
    // (crate::proxy_instances::instance_columns' pinned convention).
    let m = mat4x4<f32>(i_col0, i_col1, i_col2, i_col3);
    let world = m * skinned;
    // Return the light-space clip position. The epilogue's
    // ADJUST_COORDINATE_SPACE negates y in the generated SPIR-V.
    return light.light_view_proj * world;
}

// Depth-only: the rasterizer writes the interpolated light-space
// depth ([0, 1] LESS — an occluder nearer the light carries the
// smaller value); no fragment outputs exist.
@fragment
fn fs_main() {}
