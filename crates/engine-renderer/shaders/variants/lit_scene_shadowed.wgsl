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

// @core:albedo_bindings

// The per-frame parameters: lit_scene's block extended for the
// shadowed pipeline (the light-space matrix + the shadow control).
// @core:frame_params
// @core:frame_params_shadow_tail
// @core:frame_params_close

// @core:material_params

// @core:shadow_map

// The position output is `@invariant` (Era-1 review finding 19):
// this shadowed pipeline and the ordinary lit_scene.wgsl one must
// write bit-identical ground depth for the wet ground's EQUAL
// re-draw (ground_reflect.wgsl) — identical inputs and operation
// order do NOT establish that across separately compiled pipelines
// (the Vulkan invariance rules); the Invariant decoration does.
// vulkan.rs::position_invariance pins the emitted SPIR-V
// decorations from the pinned naga.
// @core:vertex_stage_lit

// @core:lighting_helpers

// @core:pcf

@fragment
fn fs_main(@location(0) world_normal: vec3<f32>,
           @location(1) world_pos: vec3<f32>,
           @location(2) uv: vec2<f32>)
    -> @location(0) vec4<f32> {
// @core:lighting_prologue
// @core:specular_terms
    // The Era-1 soft-shadow gate: the direct terms multiply by the
    // PCF factor — full shadow on back-facing surfaces (the oracle's
    // step gate), the filtered map verdict otherwise (k/9).
// @core:shadow_gate
// @core:albedo_sample
// @core:lit_equation_shadowed
    return vec4<f32>(lit, 1.0);
}
