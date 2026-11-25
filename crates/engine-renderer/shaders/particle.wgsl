// The particle shaders (Era-2 package C — the rain kind, the third
// populated proxy kind): UNLIT TINTED QUADS WITH ALPHA. The mission's
// named option, chosen and documented here: rain is translucent
// sky-lit water, not a lit surface — the per-instance tint carries
// the frame's atmosphere-derived color, the texture's alpha shapes
// the streak, and blending composes it over the scene. The lit
// equation does not apply to billboarded water streaks (meaningless
// normals on a billboard; the specular term on water specks reads as
// noise, not signal), so the fragment stage is the pure tinted
// texture fetch below.
//
// THE DEPTH CONTRACT (the pipeline assembly mirrors this exactly):
// depth TEST on, depth WRITE off — rain is OCCLUDED by geometry
// (a streak behind a building disappears) and never occludes
// (a streak in front must not punch a hole later streaks and the
// ground re-draw fall into). Blending: SRC_ALPHA /
// ONE_MINUS_SRC_ALPHA, ADD (color), ONE / ZERO, ADD (alpha) — the
// overlay precedent's attachment state verbatim.
//
// THE INSTANCING MODEL: IDENTICAL to instanced_proxy.wgsl —
// locations 0-2 are PER-VERTEX (pos, normal, uv — the rain streak
// mesh's 32-byte ProxyVertex; the normal is present in the mesh
// format and IGNORED here), locations 3-7 are PER-INSTANCE (the
// four column-major matrix columns plus the tint vec4 — location
// 7's .a IS THE INSTANCE ALPHA, the Era-2 package C claim on the
// packing's former hard-coded 1.0 slot; see
// crate::proxy_instances::ProxyInstance::alpha).
//
// BINDINGS: the SAME set layout as the lit family — 0/1 albedo +
// sampler, 2 FrameParams (view_proj only is read; the block keeps
// the corner FrameParams layout so the shared corner material sets
// bind unchanged). The material binding 3 is simply not declared —
// the unlit equation has no specular term; the SET LAYOUT stays the
// 4-binding corner layout (an unbound-by-shader descriptor is
// legal), so no descriptor foundation changes.

@group(0) @binding(0)
var albedo_tex: texture_2d<f32>;
@group(0) @binding(1)
var albedo_sampler: sampler;

// The per-frame parameters — the SAME block layout as
// instanced_proxy.wgsl (the corner FrameParams region the shared
// sets bind); only view_proj is read here.
struct FrameParams {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,   // xyz = toward light (normalized), w = eye.x
    light_color: vec4<f32>, // rgb irradiance, w = eye.y
    ambient: vec4<f32>,     // rgb ambient, w = eye.z
};
@group(0) @binding(2)
var<uniform> frame: FrameParams;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // rgb = the per-instance tint, a = the per-instance alpha.
    @location(1) tint: vec4<f32>,
};

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
    // The instance transform: columns are the T*R*S basis images
    // (crate::proxy_instances::instance_columns' pinned convention).
    // The normal input is declared to keep the mesh binding's
    // location map intact and deliberately unused (billboarded
    // streaks have no lighting normal).
    let m = mat4x4<f32>(i_col0, i_col1, i_col2, i_col3);
    let world = m * vec4<f32>(pos, 1.0);
    var out: VertexOut;
    out.position = frame.view_proj * world;
    out.uv = uv;
    out.tint = i_tint;
    return out;
}

@fragment
fn fs_main(@location(0) uv: vec2<f32>,
           @location(1) tint: vec4<f32>)
    -> @location(0) vec4<f32> {
    // The unlit tinted form: the texture's alpha (the streak shape)
    // times the instance alpha (the weather intensity's per-streak
    // coverage), the rgb tinted by the atmosphere-derived color.
    let tex = textureSample(albedo_tex, albedo_sampler, uv);
    return vec4<f32>(tex.rgb * tint.rgb, tex.a * tint.a);
}
