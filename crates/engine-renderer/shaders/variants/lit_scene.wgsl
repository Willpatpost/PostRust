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

// @core:albedo_bindings

// The per-frame parameters (view_proj + lighting), bound as a
// uniform buffer at group 0 binding 2 by the implementer.
// @core:frame_params
// @core:frame_params_close

// The per-draw material parameters (specular rgb + shininess).
// @core:material_params

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
// @core:vertex_stage_lit

// @core:lighting_helpers

@fragment
fn fs_main(@location(0) world_normal: vec3<f32>,
           @location(1) world_pos: vec3<f32>,
           @location(2) uv: vec2<f32>)
    -> @location(0) vec4<f32> {
    // Static geometry supplies outward face normals; back faces are culled.
// @core:lighting_prologue
    // Shininess arrives in material.specular.a (the even power of
    // two; the trailing-zero count is the squaring count — the CPU
    // contract).
// @core:specular_terms
// @core:albedo_sample
// @core:lit_equation
    return vec4<f32>(lit, 1.0);
}
