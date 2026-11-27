struct VertexOut {
    @invariant @builtin(position) position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec3<f32>,
           @location(1) normal: vec3<f32>,
           @location(2) uv: vec2<f32>)
    -> VertexOut {
    var out: VertexOut;
    out.position = frame.view_proj * vec4<f32>(pos, 1.0);
    out.world_normal = normal;
    out.world_pos = pos;
    out.uv = uv;
    return out;
}
