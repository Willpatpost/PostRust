struct FrameParams {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,   // xyz = toward light (normalized), w = eye.x
    light_color: vec4<f32>, // rgb irradiance, w = eye.y
    ambient: vec4<f32>,     // rgb ambient, w = eye.z
