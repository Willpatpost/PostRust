// The procedural sky shader (Era-1) — WGSL, the sky.rs equation
// mirrored term-for-term (the lit-scene precedent). Rendered as a
// fullscreen triangle behind the scene (depth = far); the view ray
// is reconstructed from the inverse view-projection by the
// implementer (the brief documents the standard NDC->ray
// reconstruction) — the COLOR equation below is the pinned part.
//
// THE EQUATION (sky.rs :: sky_color):
//   gradient = lerp(horizon, zenith, smoothstep(-0.08, 0.45, dir.y))
//   disc     = smoothstep(0.99927, 0.99939, dot(dir, disc_dir))
//   color    = clamp(gradient + disc * disc_color, 0, 2)
// with the smoothstep form t*t*(3-2t) and FIXED literal constants
// (no transcendental calls — the libm-free discipline).
//
// BINDINGS: the palette and disc direction ride the per-frame
// uniform at group 0 binding 2 (the existing FrameParams layout is
// EXTENDED by this package's brief — see docs/CYCLE6-ERA1-BRIEF.md;
// the implementer documents the final layout).

struct SkyParams {
    view_proj_inv: mat4x4<f32>,
    horizon: vec4<f32>,   // rgb palette, a unused
    zenith: vec4<f32>,    // rgb palette, a unused
    disc_color: vec4<f32>,// rgb palette, a unused
    disc_dir: vec4<f32>,  // xyz unit direction, a unused
};
@group(0) @binding(2)
var<uniform> frame: SkyParams;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) view_dir: vec3<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    // The fullscreen triangle (the standard 3-vertex cover).
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let p = positions[index];
    var out: VertexOut;
    out.position = vec4<f32>(p, 0.999999, 1.0);
    // The view ray: the far-plane point through this NDC position.
    let near_point = frame.view_proj_inv * vec4<f32>(p, 0.0, 1.0);
    let far_point = frame.view_proj_inv * vec4<f32>(p, 1.0, 1.0);
    out.view_dir = normalize(far_point.xyz / far_point.w - near_point.xyz / near_point.w);
    return out;
}

fn smoothstep_f(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = clamp((x - edge0) / (edge1 - edge0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

@fragment
fn fs_main(@location(0) view_dir: vec3<f32>) -> @location(0) vec4<f32> {
    // Mirrors sky.rs :: sky_color exactly, term-for-term.
    let g = smoothstep_f(-0.08, 0.45, view_dir.y);
    let d = dot(view_dir, frame.disc_dir.xyz);
    let disc = smoothstep_f(0.99927, 0.99939, d);
    let gradient = frame.horizon.rgb + (frame.zenith.rgb - frame.horizon.rgb) * g;
    let color = gradient + disc * frame.disc_color.rgb;
    return vec4<f32>(clamp(color, vec3<f32>(0.0), vec3<f32>(2.0)), 1.0);
}
