// The lit-scene shaders (Cycle 6-B) — WGSL, following the 6-A atlas
// precedent (the pinned naga's glsl-in cannot express sampled
// textures; wgsl-in can).
//
// THE EQUATION (mirrors lit_scene.rs :: lit_surface_color exactly,
// term-for-term, fixed operation order):
//   color = albedo * (ambient + light_color * NdotL)
//         + specular * light_color * NdotL * (NdotH ^ shininess)
// with: N, V, L, H normalized; NdotL/NdotH clamped to [0,1]; the
// shininess power by repeated squaring (the CPU reference's
// power-of-two contract); each channel computed independently.
//
// TRANSPORT NOTE: the pinned naga 29.0.4's wgsl-in does not
// recognize `var<push_constant>` (probed at scaffold time). The
// parameters below arrive through a uniform buffer (the implementer's
// wiring; the PUSH layout struct is defined at the bottom of this
// file as the contract). The CPU reference equation is unchanged by
// the transport.

@group(0) @binding(0)
var albedo_tex: texture_2d<f32>;
@group(0) @binding(1)
var albedo_sampler: sampler;

// The per-frame parameters (view_proj + lighting), bound as a
// uniform buffer at group 0 binding 2 by the implementer.
struct FrameParams {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,   // xyz = toward light (normalized), w unused
    light_color: vec4<f32>, // rgb irradiance, a unused
    ambient: vec4<f32>,     // rgb ambient, a unused
};
@group(0) @binding(2)
var<uniform> frame: FrameParams;

// The per-draw material parameters (specular rgb + shininess).
struct MaterialParams {
    specular: vec4<f32>,    // rgb specular color, a = shininess (even power of two)
};
@group(0) @binding(3)
var<uniform> material: MaterialParams;

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

@fragment
fn fs_main(@location(0) world_normal: vec3<f32>,
           @location(1) world_pos: vec3<f32>,
           @location(2) uv: vec2<f32>)
    -> @location(0) vec4<f32> {
    // NOTE: front_facing removed at scaffold time — the pinned naga's
    // spv writer flagged MissingBinding with it present (root cause
    // unclear; the probe without it passes). The implementer
    // investigates whether double-sided geometry needs it (the scene
    // boxes are closed volumes — back faces are depth-occluded, so
    // the sign flip is likely unnecessary; document the finding).
    let n = normalize(world_normal);
    let v = normalize(-world_pos);
    let l = normalize(frame.light_dir.xyz);
    let n_dot_l = clamp(dot(n, l), 0.0, 1.0);
    let h = normalize(l + v);
    let n_dot_h = clamp(dot(n, h), 0.0, 1.0);
    // Shininess arrives in material.specular.a (the even power of
    // two; the trailing-zero count is the squaring count — the CPU
    // contract).
    let squarings = trailing_zeros(u32(material.specular.a));
    let spec = square_pow(n_dot_h, squarings);
    let albedo = textureSample(albedo_tex, albedo_sampler, uv).rgb;
    let lit = albedo * (frame.ambient.rgb + frame.light_color.rgb * n_dot_l)
            + material.specular.rgb * frame.light_color.rgb * n_dot_l * spec;
    return vec4<f32>(lit, 1.0);
}
