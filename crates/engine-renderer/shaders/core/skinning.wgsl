    // The skinning equation: per-term full mat4*vec4 transforms,
    // weight scaling, left-to-right sums, normalize after the sum.
    let base = u32(i_pose.x) * 11u;
    let m0 = poses[base + joints.x];
    let m1 = poses[base + joints.y];
    let m2 = poses[base + joints.z];
    let m3 = poses[base + joints.w];
    let p0 = m0 * vec4<f32>(pos, 1.0);
    let p1 = m1 * vec4<f32>(pos, 1.0);
    let p2 = m2 * vec4<f32>(pos, 1.0);
    let p3 = m3 * vec4<f32>(pos, 1.0);
    let skinned = weights.x * p0 + weights.y * p1 + weights.z * p2 + weights.w * p3;
    let n0 = mat3x3<f32>(m0[0].xyz, m0[1].xyz, m0[2].xyz) * normal;
    let n1 = mat3x3<f32>(m1[0].xyz, m1[1].xyz, m1[2].xyz) * normal;
    let n2 = mat3x3<f32>(m2[0].xyz, m2[1].xyz, m2[2].xyz) * normal;
    let n3 = mat3x3<f32>(m3[0].xyz, m3[1].xyz, m3[2].xyz) * normal;
    let skinned_n = normalize(weights.x * n0 + weights.y * n1 + weights.z * n2 + weights.w * n3);
