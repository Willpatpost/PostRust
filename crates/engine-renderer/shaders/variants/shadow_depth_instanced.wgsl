// The Cycle 6-F depth-only shadow-pass shader for instanced proxies
// (Era-2 stage 2B): the vehicle cast into the shadow map, the same
// vertex logic as shadow_depth.wgsl (both depth-only shaders share
// the same shadow pass form).
//
// TRANSPORT: the pinned naga 29.0.4's wgsl-in has no
// var<push_constant> (the probed finding) — the light matrix rides a
// uniform buffer at binding 0 of the shadow pipeline's own set. The
// vertex format is the lit pipeline's (pos/normal/uv, locations
// 0-2); the mesh VBO is the corner scene's own.
//
// The build-time ADJUST_COORDINATE_SPACE epilogue negates
// gl_Position.y here as in every vertex shader — the shadow map's
// rows therefore carry the FLIPPED light-space NDC y, which the
// CPU-side texel inversion (shadows::texel_ray_origin) and the
// shadowed lit shader's lookup both account for.
//
// BINDINGS:
//   0: LightMatrix uniform (light_view_proj mat4x4<f32>)

// @core:light_matrix

@vertex
fn vs_main(@location(0) pos: vec3<f32>,
           @location(1) normal: vec3<f32>,
           @location(2) uv: vec2<f32>,
           @location(3) i_col0: vec4<f32>,
           @location(4) i_col1: vec4<f32>,
           @location(5) i_col2: vec4<f32>,
           @location(6) i_col3: vec4<f32>,
           @location(7) i_tint: vec4<f32>)
    -> @builtin(position) vec4<f32> {
// @core:instance_transform
    let world = m * vec4<f32>(pos, 1.0);
    // Return the light-space clip position. The epilogue's
    // ADJUST_COORDINATE_SPACE negates y in the generated SPIR-V.
    return light.light_view_proj * world;
}

// Depth-only: the rasterizer writes the interpolated light-space
// depth ([0, 1] LESS — an occluder nearer the light carries the
// smaller value); no fragment outputs exist.
@fragment
fn fs_main() {}
