// The bloom chain shader (Era-1 package 6 — see
// docs/CYCLE6-ERA1-BRIEF.md) — the post.rs chain mirrored
// term-for-term: the bright-passed 2×2 box downsample to the 1/4
// mip, then the separable [1,4,6,4,1]/16 blur. The composite
// (tonemap + the bloom add-back) lives in post.wgsl, unchanged
// from the scaffold.
//
// THE EQUATIONS (post.rs):
//   downsample(i,j) = bright((a + b + c + d) · 0.25) over scene
//                    texels (4i+1, 4j+1)..(4i+2, 4j+2) — exactly
//                    the four texels a bilinear 1/4 downsample
//                    would weight 0.25 each
//   blur(texel)     = Σ_k w_k · mip[clamp(texel + off_k)] with
//                    w = [1,4,6,4,1]/16 (bit-equal to
//                    post.rs's BLOOM_WEIGHTS — power-of-two
//                    fractions) and off = −2..+2 on ONE axis
//
// EXACTNESS: every fetch is a textureLoad (an exact texel read —
// the CPU oracle reads the same texels; no filter ambiguity).
// The edge taps clamp to the mip range (the CLAMP_TO_EDGE
// convention post.rs's blur_texel mirrors); the accumulation
// order is fixed tap order (left to right), the CPU's order.
//
// BINDINGS: 0 = the source image (the HDR scene target for the
// downsample pass, the previous mip for the blur passes). No
// sampler (nothing is filtered) and no uniforms (the extents come
// from textureDimensions) — the binding layout post.wgsl's map
// covers, finalized and documented by this package.

@group(0) @binding(0)
var src_tex: texture_2d<f32>;

fn smoothstep_f(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = clamp((x - edge0) / (edge1 - edge0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// Mirrors post.rs :: bright_pass exactly (the 1.0→1.5 knee).
fn bright_pass(x: f32) -> f32 {
    return smoothstep_f(1.0, 1.5, x) * x;
}

// This fragment's texel index: frag.xy is (i + 0.5, j + 0.5) —
// the +0.5 is exact in f32 at every rasterized extent this
// renderer handles, so the truncation below is exact.
fn frag_texel(frag: vec4<f32>) -> vec2<i32> {
    return vec2<i32>(frag.xy - vec2<f32>(0.5, 0.5));
}

fn clamp_texel(texel: vec2<i32>, extent: vec2<i32>) -> vec2<i32> {
    return clamp(texel, vec2<i32>(0, 0), extent - vec2<i32>(1, 1));
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // The fullscreen triangle (the standard 3-vertex cover); the
    // bloom passes need no varyings — everything derives from
    // frag_coord.
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let p = positions[index];
    return vec4<f32>(p, 0.0, 1.0);
}

@fragment
fn fs_bright(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let scene_extent = vec2<i32>(textureDimensions(src_tex));
    let texel = frag_texel(frag);
    // The 2×2 bilinear footprint in the scene image (the box a
    // linear-filtered 1/4 downsample would average).
    let base = texel * 4 + vec2<i32>(1, 1);
    let a = textureLoad(src_tex, base, 0).rgb;
    let b = textureLoad(src_tex, base + vec2<i32>(1, 0), 0).rgb;
    let c = textureLoad(src_tex, base + vec2<i32>(0, 1), 0).rgb;
    let d = textureLoad(src_tex, base + vec2<i32>(1, 1), 0).rgb;
    // Fixed order: ((a + b) + c) + d, then the 0.25 scale.
    let avg = (a + b + c + d) * 0.25;
    return vec4<f32>(bright_pass(avg.r), bright_pass(avg.g), bright_pass(avg.b), 1.0);
}

@fragment
fn fs_blur_h(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let extent = vec2<i32>(textureDimensions(src_tex));
    let texel = frag_texel(frag);
    var acc = vec3<f32>(0.0, 0.0, 0.0);
    // The five taps at −2..+2 on the x axis, fixed order, edge
    // clamped — post.rs :: blur_texel(…, true), term for term.
    acc = acc + textureLoad(src_tex, clamp_texel(texel + vec2<i32>(-2, 0), extent), 0).rgb * 0.0625;
    acc = acc + textureLoad(src_tex, clamp_texel(texel + vec2<i32>(-1, 0), extent), 0).rgb * 0.25;
    acc = acc + textureLoad(src_tex, clamp_texel(texel + vec2<i32>( 0, 0), extent), 0).rgb * 0.375;
    acc = acc + textureLoad(src_tex, clamp_texel(texel + vec2<i32>( 1, 0), extent), 0).rgb * 0.25;
    acc = acc + textureLoad(src_tex, clamp_texel(texel + vec2<i32>( 2, 0), extent), 0).rgb * 0.0625;
    return vec4<f32>(acc, 1.0);
}

@fragment
fn fs_blur_v(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let extent = vec2<i32>(textureDimensions(src_tex));
    let texel = frag_texel(frag);
    var acc = vec3<f32>(0.0, 0.0, 0.0);
    // The same five taps on the y axis — post.rs :: blur_texel(…,
    // false). (`0 + v` is bit-identical to `v`, so the acc form
    // is the CPU's chained sum exactly.)
    acc = acc + textureLoad(src_tex, clamp_texel(texel + vec2<i32>(0, -2), extent), 0).rgb * 0.0625;
    acc = acc + textureLoad(src_tex, clamp_texel(texel + vec2<i32>(0, -1), extent), 0).rgb * 0.25;
    acc = acc + textureLoad(src_tex, clamp_texel(texel + vec2<i32>(0,  0), extent), 0).rgb * 0.375;
    acc = acc + textureLoad(src_tex, clamp_texel(texel + vec2<i32>(0,  1), extent), 0).rgb * 0.25;
    acc = acc + textureLoad(src_tex, clamp_texel(texel + vec2<i32>(0,  2), extent), 0).rgb * 0.0625;
    return vec4<f32>(acc, 1.0);
}
