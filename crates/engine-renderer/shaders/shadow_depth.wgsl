// The Cycle 6-F depth-only shadow-pass shaders (see
// docs/AUTONOMOUS-CYCLE-PROTOCOL.md §2 item 2): the static corner
// scene re-projected into the light's orthographic clip space, depth
// written to the shadow map — no color attachments, no bias at WRITE
// time (the compare-time analytical bias keeps the map exactly
// CPU-predictable: the known-answer raycaster pins these depths).
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

struct LightMatrix {
    light_view_proj: mat4x4<f32>,
};
@group(0) @binding(0)
var<uniform> light: LightMatrix;

@vertex
fn vs_main(@location(0) pos: vec3<f32>,
           @location(1) normal: vec3<f32>,
           @location(2) uv: vec2<f32>)
    -> @builtin(position) vec4<f32> {
    return light.light_view_proj * vec4<f32>(pos, 1.0);
}

// Depth-only: the rasterizer writes the interpolated light-space
// depth ([0, 1] LESS — an occluder nearer the light carries the
// smaller value); no fragment outputs exist.
@fragment
fn fs_main() {}
