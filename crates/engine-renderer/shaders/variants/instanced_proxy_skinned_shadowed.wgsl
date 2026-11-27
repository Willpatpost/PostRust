// The Cycle 6-F shadowed skinned instanced-proxy shaders (Era-2
// stage 2B): the NPC lit path with skeletal animation and soft
// shadows, the shadow_factor function copied VERBATIM from
// lit_scene_shadowed.wgsl (the 6-F CPU oracle shadows::shadow_filter
// pins it, the shadowed lit shader's PCF factor is byte-identical to
// this implementation).
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
// THE SHADOWED EQUATION (the Era-1 soft-shadow gate: the direct terms
// multiply by the PCF factor — full shadow on back-facing surfaces,
// the filtered map verdict otherwise):
//   albedo = textureSample(albedo_tex, albedo_sampler, uv).rgb * tint;
//   color = albedo * (ambient + light * NdotL * shadow)
//         + specular * light * NdotL * shadow * (NdotH ^ shininess)
//
// BINDINGS:
//   0: albedo texture (texture_2d<f32>)
//   1: albedo sampler (sampler)
//   2: FrameParams uniform (view_proj, light_dir, light_color, ambient + light_view_proj + shadow_control)
//   3: MaterialParams uniform (specular vec4)
//   4: shadow_map (texture_depth_2d, textureLoad only)
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
//   Note: NO @invariant on position (this pipeline's gates compare
//   within this pipeline only; the wet ground's EQUAL contract does
//   not involve skinned proxies).

// @core:albedo_bindings

// The per-frame parameters: lit_scene's block extended for the
// shadowed pipeline (the light-space matrix + the shadow control).
// @core:frame_params
// @core:frame_params_shadow_tail
// @core:frame_params_close

// @core:material_params

// @core:shadow_map

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

// @core:pcf

@fragment
fn fs_main(@location(0) world_normal: vec3<f32>,
           @location(1) world_pos: vec3<f32>,
           @location(2) uv: vec2<f32>,
           @location(3) tint: vec3<f32>)
    -> @location(0) vec4<f32> {
// @core:lighting_prologue
// @core:specular_terms
    // The Era-1 soft-shadow gate: the direct terms multiply by the
    // PCF factor — full shadow on back-facing surfaces (the oracle's
    // step gate), the filtered map verdict otherwise (k/9).
    // The per-instance tint multiplies the sampled albedo BEFORE the
    // equation (the proxy composition: albedo * tint).
// @core:shadow_gate
// @core:albedo_sample_tinted
// @core:lit_equation_shadowed
    return vec4<f32>(lit, 1.0);
}
