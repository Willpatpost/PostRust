// The atlas overlay shaders (Cycle 6-A) — WGSL, not GLSL.
//
// THE FINDING (probed empirically against the pinned naga 29.0.4):
// the glsl-in frontend does not implement bare uniform sampler
// declarations ("variable qualifier" NotImplemented in every
// qualifier form — layout/set/binding included), so a sampled
// texture CANNOT be expressed through the legacy GLSL path. WGSL-in
// handles textures/samplers cleanly through the same pinned,
// pure-Rust, no-external-toolchain path (T-05/T-12 discipline
// preserved; the permanent shader-toolchain decision remains
// deferred — the legacy GLSL shaders stay untouched, and the atlas
// path is the first WGSL consumer).
//
// VERTEX CONVENTION: matches the legacy overlay.vert — NDC in
// (normalization stays CPU-side, the same append_quad discipline),
// color and UV pass through.
//
// BINDINGS: group 0, binding 0 = the atlas texture (R8, nearest),
// binding 1 = the sampler — SEPARATE texture and sampler bindings
// in WGSL. The SPIR-V backend maps both variables to set 0, binding 0,
// backed by one Vulkan combined image sampler.

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) tint: vec4<f32>,
    @location(1) frag_uv: vec2<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec2<f32>,
           @location(1) col: vec4<f32>,
           @location(2) uv: vec2<f32>)
    -> VertexOut {
    var out: VertexOut;
    out.position = vec4<f32>(pos, 0.0, 1.0);
    out.tint = col;
    out.frag_uv = uv;
    return out;
}

@group(0) @binding(0)
var glyph_atlas: texture_2d<f32>;
@group(0) @binding(1)
var glyph_sampler: sampler;

@fragment
fn fs_main(@location(0) tint: vec4<f32>,
           @location(1) frag_uv: vec2<f32>)
    -> @location(0) vec4<f32> {
    let lum = textureSample(glyph_atlas, glyph_sampler, frag_uv).r;
    // An unlit legacy texel emitted no fragment. Preserve destination alpha
    // as well as RGB when drawing over translucent HUD geometry.
    if lum == 0.0 { discard; }
    return vec4<f32>(tint.rgb, lum * tint.a);
}
