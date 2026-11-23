// The Cycle 6-F shadowed lit-scene shaders (see
// docs/AUTONOMOUS-CYCLE-PROTOCOL.md §2 item 2): lit_scene.wgsl's
// equation with the direct light term gated by the shadow map —
// the shadow factor multiplies BOTH the diffuse and the specular
// direct contributions; the ambient term is untouched (sky light
// reaches shadowed surfaces).
//
//   color = albedo * (ambient + light * NdotL * shadow)
//         + specular * light * NdotL * shadow * (NdotH ^ shininess)
//
// with `shadow` ∈ {0, 1} (HARD shadows, one cascade, no filtering —
// the 6-F scope; the compare is plain arithmetic, not a compare
// sampler, so the CPU oracle mirrors it exactly):
//
//   shadow = 0  iff  the receiver's light depth, biased, exceeds the
//                    map's stored depth at the receiver's projected
//                    texel.
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

struct VertexOut {
    @builtin(position) position: vec4<f32>,
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

// The hard shadow factor for a surface point (the CPU oracle
// shadows::is_shadowed plus the texel compare mirrors this exactly).
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
    let map_depth = textureLoad(shadow_map, texel, 0);
    // The compare-time bias (shadows::depth_bias's expression):
    // constant + slope_scale * (1/max(|n·l|, floor) − 1).
    let l = normalize(frame.light_dir.xyz);
    let facing = max(abs(dot(normal, l)), 1.0 / 64.0);
    let bias = frame.shadow_control.y + frame.shadow_control.z * (1.0 / facing - 1.0);
    if (ndc.z - bias > map_depth) {
        return 0.0;
    }
    return 1.0;
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
    // The 6-F gate: the direct terms multiply by the shadow factor —
    // full shadow on back-facing surfaces (the oracle's step gate),
    // the map's verdict otherwise.
    var shadow: f32 = 0.0;
    if (n_dot_l > 0.0) {
        shadow = shadow_factor(world_pos, n);
    }
    let albedo = textureSample(albedo_tex, albedo_sampler, uv).rgb;
    let lit = albedo * (frame.ambient.rgb + frame.light_color.rgb * n_dot_l * shadow)
            + material.specular.rgb * frame.light_color.rgb * n_dot_l * shadow * spec;
    return vec4<f32>(lit, 1.0);
}
