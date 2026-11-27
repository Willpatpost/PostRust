    let lit = albedo * (frame.ambient.rgb + frame.light_color.rgb * n_dot_l * shadow)
            + material.specular.rgb * frame.light_color.rgb * n_dot_l * shadow * spec;
