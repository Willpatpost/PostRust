    let n = normalize(world_normal);
    let eye = vec3<f32>(frame.light_dir.w, frame.light_color.w, frame.ambient.w);
    let v = normalize(eye - world_pos);
    let l = normalize(frame.light_dir.xyz);
    let n_dot_l = clamp(dot(n, l), 0.0, 1.0);
    let h = normalize(l + v);
    let n_dot_h = clamp(dot(n, h), 0.0, 1.0);
