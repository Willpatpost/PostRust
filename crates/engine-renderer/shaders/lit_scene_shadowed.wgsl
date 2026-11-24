// The Cycle 6-F shadowed lit-scene shaders (see
// docs/AUTONOMOUS-CYCLE-PROTOCOL.md §2 item 2), Era-1 soft-shadow
// form: lit_scene.wgsl's equation with the direct light term gated
// by the PCF-filtered shadow map — the factor multiplies BOTH the
// diffuse and the specular direct contributions; the ambient term
// is untouched (sky light reaches shadowed surfaces).
//
//   color = albedo * (ambient + light * NdotL * shadow)
//         + specular * light * NdotL * shadow * (NdotH ^ shininess)
//
// with `shadow` = the Era-1 PCF factor (shadow_filter.rs's
// `pcf_factor` mirrored term-for-term — k/9, the fraction of the
// nine fixed row-major taps whose biased compare passes):
//
//   shadow = (1/9) * Σ over taps [ ref <= map_depth(tap) + bias ]
//
// The taps are INTEGER TEXEL offsets around the receiver's
// projected texel; out-of-bounds taps read the map's clear value
// 1.0 (lit — the convention both sides share, so edge texels soften
// rather than harden). The compare is the CPU core's own form
// (`reference <= depth + bias` — NOT the 6-F hard form's
// `ref − bias <= depth`; the two differ by float rounding, and the
// soft-shadow gate compares this shader's factor against the CPU
// filter over the SAME map bytes, so the arithmetic form must be
// the same one).
//
// THE COMPARE-TIME BIAS (mirroring shadows::depth_bias): the slope
// term reduces, for the axis-aligned corner geometry under a
// directional light, to a fixed function of the world normal and the
// light direction — the shader evaluates the SAME expression the CPU
// oracle does, from the same FrameParams-carried constants.
//
// BINDINGS: 0/1 albedo + sampler, 2 FrameParams, 3 MaterialParams —
// IDENTICAL to lit_scene.wgsl — plus 4: the shadow map as a
// texture_depth_2d, read with textureLoad (exact texel fetch: no
// sampler, no filtering, no compare-op magic — the compare is the
// shader's own arithmetic). FrameParams extends lit_scene's block
// with the light matrix and the shadow control vector (the
// unshadowed shader reads only the block's 112-byte prefix).

@group(0) @binding(0)
var albedo_tex: texture_2d<f32>;
@group(0) @binding(1)
var albedo_sampler: sampler;

// The per-frame parameters: lit_scene's block extended for the
// shadowed pipeline (the light-space matrix + the shadow control).
struct FrameParams {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,   // xyz = toward light (normalized), w = eye.x
    light_color: vec4<f32>, // rgb irradiance, w = eye.y
    ambient: vec4<f32>,     // rgb ambient, w = eye.z
    // The 6-F extension (the unshadowed shader ignores this tail).
    light_view_proj: mat4x4<f32>,
    shadow_control: vec4<f32>, // x = map extent (texels), y = bias
                               // constant, z = bias slope scale,
                               // w = unused (0)
};
@group(0) @binding(2)
var<uniform> frame: FrameParams;

struct MaterialParams {
    specular: vec4<f32>,    // rgb specular color, a = shininess (even power of two)
};
@group(0) @binding(3)
var<uniform> material: MaterialParams;

@group(0) @binding(4)
var shadow_map: texture_depth_2d;

// The position output is `@invariant` (Era-1 review finding 19):
// this shadowed pipeline and the ordinary lit_scene.wgsl one must
// write bit-identical ground depth for the wet ground's EQUAL
// re-draw (ground_reflect.wgsl) — identical inputs and operation
// order do NOT establish that across separately compiled pipelines
// (the Vulkan invariance rules); the Invariant decoration does.
// vulkan.rs::position_invariance pins the emitted SPIR-V
// decorations from the pinned naga.
struct VertexOut {
    @invariant @builtin(position) position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec3<f32>,
           @location(1) normal: vec3<f32>,
           @location(2) uv: vec2<f32>)
    -> VertexOut {
    var out: VertexOut;
    out.position = frame.view_proj * vec4<f32>(pos, 1.0);
    out.world_normal = normal;
    out.world_pos = pos;
    out.uv = uv;
    return out;
}

fn square_pow(base: f32, squarings: u32) -> f32 {
    var result = base;
    for (var i: u32 = 0u; i < squarings; i = i + 1u) {
        result = result * result;
    }
    return result;
}

fn trailing_zeros(v: u32) -> u32 {
    var count: u32 = 0u;
    var x = v;
    while ((x & 1u) == 0u && count < 32u) {
        x = x >> 1u;
        count = count + 1u;
    }
    return count;
}

// The PCF tap table (shadow_filter.rs's PCF_OFFSETS mirrored — the
// nine offsets in the FIXED row-major order both sides share).
// naga 29's wgsl-in carries no const-array dynamic indexing, so the
// table is a function of the tap index (the order is pinned by the
// CPU core's own table-shape test; a wrong table fails there, not
// here).
fn tap_offset(index: u32) -> vec2<i32> {
    if (index == 0u) { return vec2<i32>(-1, -1); }
    if (index == 1u) { return vec2<i32>( 0, -1); }
    if (index == 2u) { return vec2<i32>( 1, -1); }
    if (index == 3u) { return vec2<i32>(-1,  0); }
    if (index == 4u) { return vec2<i32>( 0,  0); }
    if (index == 5u) { return vec2<i32>( 1,  0); }
    if (index == 6u) { return vec2<i32>(-1,  1); }
    if (index == 7u) { return vec2<i32>( 0,  1); }
    return vec2<i32>(1, 1);
}

// The PCF-filtered shadow factor for a surface point (the CPU oracle
// shadow_filter.rs's pcf_at_point mirrors this exactly, over the same
// captured map bytes: the same projection, the same y-flipped texel
// expression, the same clamp, the same bias, the same tap table, the
// same compare form, the same out-of-bounds-reads-clear convention).
fn shadow_factor(world_pos: vec3<f32>, normal: vec3<f32>) -> f32 {
    let light_clip = frame.light_view_proj * vec4<f32>(world_pos, 1.0);
    let ndc = light_clip.xyz / light_clip.w;
    // Outside the cascade's orthographic box: the light is unblocked
    // by construction (the box covers the whole scene).
    if (ndc.x < -1.0 || ndc.x > 1.0 || ndc.y < -1.0 || ndc.y > 1.0
        || ndc.z < 0.0 || ndc.z > 1.0) {
        return 1.0;
    }
    // The map's row axis carries the FLIPPED NDC y (the vertex
    // epilogue's y negation in the depth-only pass); the texel index
    // is the exact fetch the CPU oracle mirrors, clamped to the last
    // texel at the +1 edges (the CPU mirror clamps identically).
    let extent = frame.shadow_control.x;
    var texel = vec2<u32>(
        vec2((ndc.x + 1.0) * 0.5 * extent, (1.0 - (ndc.y + 1.0) * 0.5) * extent)
    );
    texel = min(texel, vec2<u32>(u32(extent) - 1u));
    // The compare-time bias (shadows::depth_bias's expression):
    // constant + slope_scale * (1/max(|n·l|, floor) − 1).
    let l = normalize(frame.light_dir.xyz);
    let facing = max(abs(dot(normal, l)), 1.0 / 64.0);
    let bias = frame.shadow_control.y + frame.shadow_control.z * (1.0 / facing - 1.0);
    // THE PCF LOOP (shadow_filter.rs's pcf_factor, term-for-term):
    // nine taps in the fixed row-major order; a tap passes when the
    // receiver's depth is <= the tap's stored depth plus the bias
    // (the CPU core's own compare form); out-of-bounds taps read the
    // map's clear value 1.0 (lit).
    var passing: u32 = 0u;
    for (var tap: u32 = 0u; tap < 9u; tap = tap + 1u) {
        let offset = tap_offset(tap);
        let x = i32(texel.x) + offset.x;
        let y = i32(texel.y) + offset.y;
        var depth = 1.0;
        if (x >= 0 && y >= 0 && x < i32(extent) && y < i32(extent)) {
            depth = textureLoad(shadow_map, vec2<u32>(u32(x), u32(y)), 0);
        }
        if (ndc.z <= depth + bias) {
            passing = passing + 1u;
        }
    }
    return f32(passing) / 9.0;
}

@fragment
fn fs_main(@location(0) world_normal: vec3<f32>,
           @location(1) world_pos: vec3<f32>,
           @location(2) uv: vec2<f32>)
    -> @location(0) vec4<f32> {
    let n = normalize(world_normal);
    let eye = vec3<f32>(frame.light_dir.w, frame.light_color.w, frame.ambient.w);
    let v = normalize(eye - world_pos);
    let l = normalize(frame.light_dir.xyz);
    let n_dot_l = clamp(dot(n, l), 0.0, 1.0);
    let h = normalize(l + v);
    let n_dot_h = clamp(dot(n, h), 0.0, 1.0);
    let squarings = trailing_zeros(u32(material.specular.a));
    let spec = square_pow(n_dot_h, squarings);
    // The Era-1 soft-shadow gate: the direct terms multiply by the
    // PCF factor — full shadow on back-facing surfaces (the oracle's
    // step gate), the filtered map verdict otherwise (k/9).
    var shadow: f32 = 0.0;
    if (n_dot_l > 0.0) {
        shadow = shadow_factor(world_pos, n);
    }
    let albedo = textureSample(albedo_tex, albedo_sampler, uv).rgb;
    let lit = albedo * (frame.ambient.rgb + frame.light_color.rgb * n_dot_l * shadow)
            + material.specular.rgb * frame.light_color.rgb * n_dot_l * shadow * spec;
    return vec4<f32>(lit, 1.0);
}
