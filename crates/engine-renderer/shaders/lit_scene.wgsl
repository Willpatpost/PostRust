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
// frame and material blocks below are uniform buffers.

@group(0) @binding(0)
var albedo_tex: texture_2d<f32>;
@group(0) @binding(1)
var albedo_sampler: sampler;

// The per-frame parameters (view_proj + lighting), bound as a
// uniform buffer at group 0 binding 2 by the implementer.
struct FrameParams {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,   // xyz = toward light (normalized), w = eye.x
    light_color: vec4<f32>, // rgb irradiance, w = eye.y
    ambient: vec4<f32>,     // rgb ambient, w = eye.z
};
@group(0) @binding(2)
var<uniform> frame: FrameParams;

// The per-draw material parameters (specular rgb + shininess).
struct MaterialParams {
    specular: vec4<f32>,    // rgb specular color, a = shininess (even power of two)
};
@group(0) @binding(3)
var<uniform> material: MaterialParams;

// The position output is `@invariant` (Era-1 review finding 19):
// the wet ground's re-draw (ground_reflect.wgsl) depth-tests EQUAL
// against the depth THIS pipeline writes, and the shadowed variant
// (lit_scene_shadowed.wgsl) must produce the same depth for the
// same ground — identical inputs and operation order do NOT
// establish bit-identical clip positions across separately
// compiled pipelines (the Vulkan invariance rules); the Invariant
// decoration does. All three shaders carry it, and
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

@fragment
fn fs_main(@location(0) world_normal: vec3<f32>,
           @location(1) world_pos: vec3<f32>,
           @location(2) uv: vec2<f32>)
    -> @location(0) vec4<f32> {
    // Static geometry supplies outward face normals; back faces are culled.
    let n = normalize(world_normal);
    let eye = vec3<f32>(frame.light_dir.w, frame.light_color.w, frame.ambient.w);
    let v = normalize(eye - world_pos);
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
