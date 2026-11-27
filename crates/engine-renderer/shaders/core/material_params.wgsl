struct MaterialParams {
    specular: vec4<f32>,    // rgb specular color, a = shininess (even power of two)
};
@group(0) @binding(3)
var<uniform> material: MaterialParams;
