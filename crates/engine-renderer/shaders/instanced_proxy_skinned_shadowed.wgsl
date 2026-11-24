// The Cycle 6-F shadowed skinned instanced-proxy shaders (Era-2
// stage 2B): the NPC lit path with skeletal animation and soft
// shadows, the shadow_factor function copied VERBATIM from
// lit_scene_shadowed.wgsl (the 6-F CPU oracle shadows::shadow_filter
// pins it, the shadowed lit shader's PCF factor is byte-identical to
// this implementation).
//
// THE SKINNING EQUATION (the exact order from skin_vertex):
//   let base = u32(i_pose.x) * 11u;
//   let m0 = poses[base + joints.x];
//   let m1 = poses[base + joints.y];
//   let m2 = poses[base + joints.z];
//   let m3 = poses[base + joints.w];
//   let p0 = m0 * vec4<f32>(pos, 1.0);
//   let p1 = m1 * vec4<f32>(pos, 1.0);
//   let p2 = m2 * vec4<f32>(pos, 1.0);
//   let p3 = m3 * vec4<f32>(pos, 1.0);
//   let skinned = weights.x * p0 + weights.y * p1 + weights.z * p2 + weights.w * p3;
//   let n0 = mat3x3<f32>(m0[0].xyz, m0[1].xyz, m0[2].xyz) * normal;
//   let n1 = mat3x3<f32>(m1[0].xyz, m1[1].xyz, m1[2].xyz) * normal;
//   let n2 = mat3x3<f32>(m2[0].xyz, m2[1].xyz, m2[2].xyz) * normal;
//   let n3 = mat3x3<f32>(m3[0].xyz, m3[1].xyz, m3[2].xyz) * normal;
//   let skinned_n = normalize(weights.x * n0 + weights.y * n1 + weights.z * n2 + weights.w * n3);
//   let world = instance_matrix * skinned;
//   let n = normalize(instance_upper_3x3 * skinned_n);
//   out.position = frame.view_proj * world;
//
// THE SHADOWED EQUATION (the Era-1 soft-shadow gate: the direct terms
// multiply by the PCF factor — full shadow on back-facing surfaces,
// the filtered map verdict otherwise):
//   albedo = textureSample(albedo_tex, albedo_sampler, uv).rgb * tint;
//   color = albedo * (ambient + light * NdotL * shadow)
//         + specular * light * NdotL * shadow * (NdotH ^ shininess)
//
// BINDINGS:
//   0: albedo texture (texture_2d<f32>)
//   1: albedo sampler (sampler)
//   2: FrameParams uniform (view_proj, light_dir, light_color, ambient + light_view_proj + shadow_control)
//   3: MaterialParams uniform (specular vec4)
//   4: shadow_map (texture_depth_2d, textureLoad only)
//   5: poses storage buffer (array<mat4x4<f32>, read-only> — the skin matrices)
//   Note: bindings 0/1 share descriptor 0 (the existing WGSL-to-SPIR-V quirk).
//
// VERTEX INPUT LOCATIONS:
//   0: pos (vec3<f32>) — per-vertex
//   1: normal (vec3<f32>) — per-vertex
//   2: uv (vec2<f32>) — per-vertex
//   3-6: i_col0..i_col3 (vec4<f32> each) — instance rate
//   7: i_tint (vec4<f32>) — instance rate
//   8: joints (vec4<u32>) — per-vertex (skinned only)
//   9: weights (vec4<f32>) — per-vertex (skinned only)
//   10: i_pose (vec4<f32>, x = pose block index) — instance rate
//   Note: NO @invariant on position (this pipeline's gates compare
//   within this pipeline only; the wet ground's EQUAL contract does
//   not involve skinned proxies).

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

// The pose storage buffer: per-instance skin matrices (the 6-B
// binding map reserves 5 for this purpose — the Era-2 skinning
// convention).
@group(0) @binding(5)
var<storage, read> poses: array<mat4x4<f32>>;

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
           @location(7) i_tint: vec4<f32>,
           @location(8) joints: vec4<u32>,
           @location(9) weights: vec4<f32>,
           @location(10) i_pose: vec4<f32>)
    -> VertexOut {
    // The skinning equation: per-term full mat4*vec4 transforms,
    // weight scaling, left-to-right sums, normalize after the sum.
    let base = u32(i_pose.x) * 11u;
    let m0 = poses[base + joints.x];
    let m1 = poses[base + joints.y];
    let m2 = poses[base + joints.z];
    let m3 = poses[base + joints.w];
    let p0 = m0 * vec4<f32>(pos, 1.0);
    let p1 = m1 * vec4<f32>(pos, 1.0);
    let p2 = m2 * vec4<f32>(pos, 1.0);
    let p3 = m3 * vec4<f32>(pos, 1.0);
    let skinned = weights.x * p0 + weights.y * p1 + weights.z * p2 + weights.w * p3;
    let n0 = mat3x3<f32>(m0[0].xyz, m0[1].xyz, m0[2].xyz) * normal;
    let n1 = mat3x3<f32>(m1[0].xyz, m1[1].xyz, m1[2].xyz) * normal;
    let n2 = mat3x3<f32>(m2[0].xyz, m2[1].xyz, m2[2].xyz) * normal;
    let n3 = mat3x3<f32>(m3[0].xyz, m3[1].xyz, m3[2].xyz) * normal;
    let skinned_n = normalize(weights.x * n0 + weights.y * n1 + weights.z * n2 + weights.w * n3);
    // The instance transform: columns are the T*R*S basis images
    // (crate::proxy_instances::instance_columns' pinned convention).
    let m = mat4x4<f32>(i_col0, i_col1, i_col2, i_col3);
    let world = m * skinned;
    // Uniform scale: the upper-3x3 basis columns rotate (and scale)
    // the normal; normalize restores unit length.
    let n = normalize(mat3x3<f32>(i_col0.xyz, i_col1.xyz, i_col2.xyz) * skinned_n);
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
           @location(2) uv: vec2<f32>,
           @location(3) tint: vec3<f32>)
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
    // The per-instance tint multiplies the sampled albedo BEFORE the
    // equation (the proxy composition: albedo * tint).
    var shadow: f32 = 0.0;
    if (n_dot_l > 0.0) {
        shadow = shadow_factor(world_pos, n);
    }
    let albedo = textureSample(albedo_tex, albedo_sampler, uv).rgb * tint;
    let lit = albedo * (frame.ambient.rgb + frame.light_color.rgb * n_dot_l * shadow)
            + material.specular.rgb * frame.light_color.rgb * n_dot_l * shadow * spec;
    return vec4<f32>(lit, 1.0);
}
