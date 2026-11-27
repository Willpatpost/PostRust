// The Cycle 6-F shadowed instanced-proxy shaders (Era-2 stage 2B):
// the vehicle lit path with soft shadows, the same fragment logic as
// instanced_proxy_skinned_shadowed.wgsl (both proxy fragment shaders
// are textually identical — the two paths share the shadowed tinted
// equation).
//
// THE SHADOWED EQUATION (the Era-1 soft-shadow gate: the direct terms
// multiply by the PCF factor — full shadow on back-facing surfaces,
// the filtered map verdict otherwise):
//   color = albedo * (ambient + light * NdotL * shadow)
//         + specular * light * NdotL * shadow * (NdotH ^ shininess)
//
// BINDINGS:
//   0: albedo texture (texture_2d<f32>)
//   1: albedo sampler (sampler)
//   2: FrameParams uniform (view_proj, light_dir, light_color, ambient + light_view_proj + shadow_control)
//   3: MaterialParams uniform (specular vec4)
//   4: shadow_map (texture_depth_2d, textureLoad only)
//   Note: bindings 0/1 share descriptor 0 (the existing WGSL-to-SPIR-V quirk).
//
// VERTEX INPUT LOCATIONS:
//   0: pos (vec3<f32>) — per-vertex
//   1: normal (vec3<f32>) — per-vertex
//   2: uv (vec2<f32>) — per-vertex
//   3-6: i_col0..i_col3 (vec4<f32> each) — instance rate
//   7: i_tint (vec4<f32>) — instance rate
//   Note: NO locations 8-10 (plain proxy, no skinning), NO @invariant
//   on position (this pipeline's gates compare within this pipeline
//   only; the wet ground's EQUAL contract does not involve proxies).

// @core:albedo_bindings

// The per-frame parameters: lit_scene's block extended for the
// shadowed pipeline (the light-space matrix + the shadow control).
// @core:frame_params
// @core:frame_params_shadow_tail
// @core:frame_params_close

// @core:material_params

// @core:shadow_map

// @core:vertex_out_proxy

@vertex
fn vs_main(@location(0) pos: vec3<f32>,
           @location(1) normal: vec3<f32>,
           @location(2) uv: vec2<f32>,
           @location(3) i_col0: vec4<f32>,
           @location(4) i_col1: vec4<f32>,
           @location(5) i_col2: vec4<f32>,
           @location(6) i_col3: vec4<f32>,
           @location(7) i_tint: vec4<f32>)
    -> VertexOut {
// @core:instance_transform
    let world = m * vec4<f32>(pos, 1.0);
    // Uniform scale: the upper-3x3 basis columns rotate (and scale)
    // the normal; normalize restores unit length.
    let n = normalize(mat3x3<f32>(i_col0.xyz, i_col1.xyz, i_col2.xyz) * normal);
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
// @core:shadow_gate
// @core:albedo_sample_tinted
// @core:lit_equation_shadowed
    return vec4<f32>(lit, 1.0);
}
