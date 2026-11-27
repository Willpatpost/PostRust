    var shadow: f32 = 0.0;
    if (n_dot_l > 0.0) {
        shadow = shadow_factor(world_pos, n);
    }
