// The instanced-proxy shaders (Cycle 6-E) — WGSL, following the
// lit-scene precedent (see lit_scene.wgsl; the pinned naga 29.0.4's
// wgsl-in supports neither var<push_constant> nor glsl-in sampled
// textures — the probed findings documented there).
//
// THE EQUATION (identical to lit_scene.wgsl, term-for-term — the
// 6-B CPU oracle crate::lit_scene::lit_surface_color pins it, with
// the per-instance tint multiplied into the albedo BEFORE the
// equation: lit_surface_color(albedo * tint, ...)):
//   color = (albedo * tint) * (ambient + light_color * NdotL)
//         + specular * light_color * NdotL * (NdotH ^ shininess)
//
// THE INSTANCING MODEL: locations 0-2 are PER-VERTEX (pos, normal,
// uv — the proxy mesh); locations 3-7 are PER-INSTANCE (the four
// column-major matrix columns of crate::proxy_instances::
// instance_columns, and the tint vec4) — the Vulkan-side vertex
// input state marks binding 1 with VK_VERTEX_INPUT_RATE_INSTANCE;
// WGSL sees them all as ordinary vertex inputs. The implementer
// wires the two vertex buffers (mesh + packed instance bytes) and
// issues ONE draw with instanceCount = N.
//
// BINDINGS: IDENTICAL to lit_scene.wgsl (0/1 albedo + sampler,
// 2 FrameParams, 3 MaterialParams) — the 6-B descriptor foundation
// is reused as-is; no new bindings exist in this shader.

@group(0) @binding(0)
var albedo_tex: texture_2d<f32>;
@group(0) @binding(1)
var albedo_sampler: sampler;

// The per-frame parameters (view_proj + lighting), bound as a
// uniform buffer at group 0 binding 2 — the SAME layout as
// lit_scene.wgsl (the eye position rides the w components).
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

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tint: vec3<f32>,
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
    let m = mat4x4<f32>(i_col0, i_col1, i_col2, i_col3);
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
           @location(2) uv: vec2<f32>,
           @location(3) tint: vec3<f32>)
    -> @location(0) vec4<f32> {
    // Identical to lit_scene.wgsl's fs_main except the tint: the
    // per-instance tint multiplies the sampled albedo BEFORE the
    // equation (the oracle composition documented in the header).
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
    let albedo = textureSample(albedo_tex, albedo_sampler, uv).rgb * tint;
    let lit = albedo * (frame.ambient.rgb + frame.light_color.rgb * n_dot_l)
            + material.specular.rgb * frame.light_color.rgb * n_dot_l * spec;
    return vec4<f32>(lit, 1.0);
}
