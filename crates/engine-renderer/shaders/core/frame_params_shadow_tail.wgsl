    // The 6-F extension (the unshadowed shader ignores this tail).
    light_view_proj: mat4x4<f32>,
    shadow_control: vec4<f32>, // x = map extent (texels), y = bias
                               // constant, z = bias slope scale,
                               // w = unused (0)
