// The pose storage buffer: per-instance skin matrices (the 6-B
// binding map reserves 5 for this purpose — the Era-2 skinning
// convention).
@group(0) @binding(5)
var<storage, read> poses: array<mat4x4<f32>>;
