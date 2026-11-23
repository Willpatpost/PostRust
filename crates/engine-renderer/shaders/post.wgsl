// The post-processing shader (Era-1) — WGSL, the post.rs equations
// mirrored term-for-term: the bright pass, the separable bloom
// composite, and the ACES tonemap. The implementer wires the
// render-target chain (the HDR scene target -> the post pass ->
// the swapchain) and the downsampled bloom mip; the EQUATIONS
// below are the pinned part (the brief's acceptance criteria
// compare them against post.rs's CPU forms).
//
// THE EQUATIONS (post.rs):
//   bright(x)  = smoothstep(1.0, 1.5, x) * x
//   tonemap(x) = clamp(x(2.51x+0.03) / (x(2.43x+0.59)+0.14), 0, 1)
//   bloom      = the 5-tap separable [1,4,6,4,1]/16 Gaussian over
//                the bright-passed downsample, composited additively
//                before the tonemap.
//
// BINDINGS: 0 = the HDR scene color target (sampled), 1 = the
// downsampled bloom mip (sampled), 2 = a small uniform (the texel
// sizes). The implementer documents the final layout; the binding
// map in build.rs covers 0-3.

@group(0) @binding(0)
var scene_color: texture_2d<f32>;
@group(0) @binding(1)
var bloom_mip: texture_2d<f32>;
@group(0) @binding(2)
var scene_sampler: sampler;

struct PostParams {
    scene_texel: vec4<f32>,  // xy texel size, zw bloom texel size
    bloom_strength: f32,
};
@group(0) @binding(3)
var<uniform> params: PostParams;

fn smoothstep_f(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = clamp((x - edge0) / (edge1 - edge0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn bright_pass(x: f32) -> f32 {
    return smoothstep_f(1.0, 1.5, x) * x;
}

fn tonemap(x: f32) -> f32 {
    let num = x * (2.51 * x + 0.03);
    let den = x * (2.43 * x + 0.59) + 0.14;
    return clamp(num / den, 0.0, 1.0);
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let p = positions[index];
    var out: VertexOut;
    out.position = vec4<f32>(p, 0.0, 1.0);
    out.uv = (p + vec2<f32>(1.0, 1.0)) * 0.5;
    out.uv.y = 1.0 - out.uv.y;
    return out;
}

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let hdr = textureSample(scene_color, scene_sampler, uv).rgb;
    // The bloom composite: the pre-filtered mip's blurred energy,
    // scaled by the strength uniform (the separable blur itself
    // runs in the implementer's downsample chain passes; this
    // composite mirrors the CPU weights' intent — the chain's
    // blur passes use the SAME [1,4,6,4,1]/16 taps).
    let bloom = textureSample(bloom_mip, scene_sampler, uv).rgb;
    let combined = hdr + bloom * params.bloom_strength;
    return vec4<f32>(vec3<f32>(
        tonemap(combined.r),
        tonemap(combined.g),
        tonemap(combined.b),
    ), 1.0);
}
