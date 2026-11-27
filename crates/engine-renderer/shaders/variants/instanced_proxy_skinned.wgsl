// The Cycle 6-E skinned instanced-proxy shaders (Era-2 stage 2B): the
// NPC lit path with skeletal animation, mirroring the CPU oracle
// crate::animation::skin_vertex term-for-term (the 6-B CPU crate pins
// the unskinned proxy, the rest-pose proof shows skin_matrices at the
// all-zero pose produces bit-exact identity, so this path renders the
// authored mesh exactly when at rest).
//
// THE SKINNING EQUATION (the exact order from skin_vertex):
//   let base = u32(i_pose.x) * 11u;
//   let m0 = poses[base + joints.x];
//   let m1 = poses[base + joints.y];
//   let m2 = poses[base + joints.z];
//   let m3 = poses[base + joints.w];
//   let p0 = m0 * vec4<f32>(pos, 1.0);
//   let p1 = m1 * vec4<f32>(pos, 1.0);
//   let p2 = m2 * vec4<f32>(pos, 1.0);
//   let p3 = m3 * vec4<f32>(pos, 1.0);
//   let skinned = weights.x * p0 + weights.y * p1 + weights.z * p2 + weights.w * p3;
//   let n0 = mat3x3<f32>(m0[0].xyz, m0[1].xyz, m0[2].xyz) * normal;
//   let n1 = mat3x3<f32>(m1[0].xyz, m1[1].xyz, m1[2].xyz) * normal;
//   let n2 = mat3x3<f32>(m2[0].xyz, m2[1].xyz, m2[2].xyz) * normal;
//   let n3 = mat3x3<f32>(m3[0].xyz, m3[1].xyz, m3[2].xyz) * normal;
//   let skinned_n = normalize(weights.x * n0 + weights.y * n1 + weights.z * n2 + weights.w * n3);
//   let world = instance_matrix * skinned;
//   let n = normalize(instance_upper_3x3 * skinned_n);
//   out.position = frame.view_proj * world;
//
// BINDINGS:
//   0: albedo texture (texture_2d<f32>)
//   1: albedo sampler (sampler)
//   2: FrameParams uniform (view_proj, light_dir, light_color, ambient)
//   3: MaterialParams uniform (specular vec4)
//   5: poses storage buffer (array<mat4x4<f32>, read-only> — the skin matrices)
//   Note: bindings 0/1 share descriptor 0 (the existing WGSL-to-SPIR-V quirk).
//
// VERTEX INPUT LOCATIONS:
//   0: pos (vec3<f32>) — per-vertex
//   1: normal (vec3<f32>) — per-vertex
//   2: uv (vec2<f32>) — per-vertex
//   3-6: i_col0..i_col3 (vec4<f32> each) — instance rate
//   7: i_tint (vec4<f32>) — instance rate
//   8: joints (vec4<u32>) — per-vertex (skinned only)
//   9: weights (vec4<f32>) — per-vertex (skinned only)
//   10: i_pose (vec4<f32>, x = pose block index) — instance rate

// @core:albedo_bindings

// The per-frame parameters (view_proj + lighting), bound as a
// uniform buffer at group 0 binding 2 — the SAME layout as
// lit_scene.wgsl (the eye position rides the w components).
// @core:frame_params
// @core:frame_params_close

// The per-draw material parameters (specular rgb + shininess).
// @core:material_params

// @core:pose_storage

// @core:vertex_out_proxy

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
    -> VertexOut {
// @core:skinning
// @core:instance_transform
    let world = m * skinned;
    // Uniform scale: the upper-3x3 basis columns rotate (and scale)
    // the normal; normalize restores unit length.
    let n = normalize(mat3x3<f32>(i_col0.xyz, i_col1.xyz, i_col2.xyz) * skinned_n);
    var out: VertexOut;
    out.position = frame.view_proj * world;
    out.world_normal = n;
    out.world_pos = world.xyz;
    out.uv = uv;
    out.tint = i_tint.rgb;
    return out;
}

// @core:lighting_helpers

@fragment
fn fs_main(@location(0) world_normal: vec3<f32>,
           @location(1) world_pos: vec3<f32>,
           @location(2) uv: vec2<f32>,
           @location(3) tint: vec3<f32>)
    -> @location(0) vec4<f32> {
    // Identical to lit_scene.wgsl's fs_main except the tint: the
    // per-instance tint multiplies the sampled albedo BEFORE the
    // equation (the oracle composition documented in the header).
// @core:lighting_prologue
    // Shininess arrives in material.specular.a (the even power of
    // two; the trailing-zero count is the squaring count — the CPU
    // contract).
// @core:specular_terms
// @core:albedo_sample_tinted
// @core:lit_equation
    return vec4<f32>(lit, 1.0);
}
