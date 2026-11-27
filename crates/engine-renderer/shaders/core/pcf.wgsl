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
