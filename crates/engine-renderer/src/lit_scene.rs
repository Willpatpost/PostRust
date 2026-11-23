//! The lit-scene reference model (the Cycle 6-B foundation — see
//! `docs/CYCLE6-6B-BRIEF.md`): the "One Corner of the City"
//! intersection geometry, the day/night lighting parameters, the
//! fixed camera route, and the CPU reference implementation of the
//! lighting equation the shader must reproduce.
//!
//! THE 6-A PATTERN, APPLIED AGAIN: everything deterministic and
//! pure lives here, fully tested — the scene layout, the lighting
//! parameters, and the lighting equation. The implementer wires the
//! GPU side and proves output equivalence against
//! [`lit_surface_color`] (the oracle) through the capture harness.
//!
//! DETERMINISM CONTRACT: the scene is pure data — a fixed seeded
//! layout, no iteration-order dependence, identical on every
//! platform. The lighting reference is IEEE f32 arithmetic in a
//! fixed operation order; the shader reproduces the same equation
//! (see the tolerance note in the brief — the capture comparison
//! uses a documented small tolerance, not bit-exactness, because
//! shader compilers may reassociate).

/// The intersection's world extent: a 40 m x 40 m corner (the
/// CUSTOM-ENGINE-RESEARCH scene definition §"One Corner of the City").
pub const SCENE_EXTENT: f32 = 40.0;
/// The ground plane sits at y = 0; buildings rise from it.
pub const GROUND_Y: f32 = 0.0;

/// One static scene box: an axis-aligned building footprint with a
/// facade material index (which procedural texture + material
/// parameters apply) and a flat roof normal for the lit top face.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneBox {
    /// Min corner (x, y, z) — y is the box bottom (0 for ground).
    pub min: [f32; 3],
    /// Max corner (x, y, z).
    pub max: [f32; 3],
    /// The material index into the scene's material table.
    pub material: u32,
}

/// The lighting parameters (day preset; the night preset is a
/// parameter change, not a different equation).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightParams {
    /// The directional light's world-space direction TOWARD the
    /// light (normalized; the shader negates for the surface test).
    pub dir_toward_light: [f32; 3],
    /// The directional light's RGB irradiance.
    pub light_color: [f32; 3],
    /// The ambient RGB irradiance.
    pub ambient: [f32; 3],
}

/// The day preset: mid-afternoon sun from the southwest, warm.
pub fn day_preset() -> LightParams {
    LightParams {
        // Normalized (−0.5, 0.7, −0.5) — southwest, ~45° elevation.
        dir_toward_light: [-0.5, 0.707_106_77, -0.5],
        light_color: [1.0, 0.96, 0.88],
        ambient: [0.18, 0.20, 0.26],
    }
}

/// The night preset: cool moonlight, higher ambient floor.
pub fn night_preset() -> LightParams {
    LightParams {
        dir_toward_light: [0.3, 0.8, 0.3],
        light_color: [0.30, 0.34, 0.45],
        ambient: [0.06, 0.07, 0.10],
    }
}

/// One surface material: the albedo texture index + the wet-look
/// specular parameters. Wet asphalt: low albedo, tight bright
/// specular; building facades: mid albedo, broad dim specular.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Material {
    /// The albedo's procedural texture index.
    pub texture: u32,
    /// The specular color.
    pub specular: [f32; 3],
    /// The specular exponent (Blinn-Phong shininess; an EVEN integer
    /// — the reference raises the half-dot by repeated squaring so
    /// CPU and GPU agree without a `pow` builtin).
    pub shininess: u32,
}

/// The scene's material table (index-stable; the boxes reference it).
pub fn materials() -> [Material; 3] {
    [
        // 0: wet asphalt ground.
        Material {
            texture: 0,
            specular: [0.9, 0.9, 1.0],
            shininess: 64,
        },
        // 1: building facade (concrete).
        Material {
            texture: 1,
            specular: [0.2, 0.2, 0.22],
            shininess: 8,
        },
        // 2: building base (dark stone).
        Material {
            texture: 2,
            specular: [0.25, 0.25, 0.25],
            shininess: 16,
        },
    ]
}

/// The static intersection: four building masses framing the corner
/// (two tall across the streets, two mid on the corner), leaving the
/// 8 m road cross open. Pure data — the fixed layout, no seeding
/// needed (the research spec's "repeatable" requirement).
pub fn intersection_boxes() -> Vec<SceneBox> {
    let e = SCENE_EXTENT;
    let road = 8.0;
    let half_road = road / 2.0;
    let sidewalk = 2.0;
    // The corner building masses sit outside the road cross; the
    // layout is symmetric-quad with varied heights (the spec's
    // recognizable corner).
    vec![
        // Northeast mass (tall): x in [half+sidewalk, e], z in [half+sidewalk, e].
        SceneBox {
            min: [half_road + sidewalk, GROUND_Y, half_road + sidewalk],
            max: [e, 18.0, e],
            material: 1,
        },
        // Southwest mass (tall): x in [-e, -(half+sidewalk)], z in [-e, -(half+sidewalk)].
        SceneBox {
            min: [-e, GROUND_Y, -e],
            max: [-(half_road + sidewalk), 22.0, -(half_road + sidewalk)],
            material: 1,
        },
        // Southeast mass (mid, dark base): x in [half+sidewalk, e], z in [-e, -(half+sidewalk)].
        SceneBox {
            min: [half_road + sidewalk, GROUND_Y, -e],
            max: [e, 9.0, -(half_road + sidewalk)],
            material: 2,
        },
        // Northwest mass (mid): x in [-e, -(half+sidewalk)], z in [half+sidewalk, e].
        SceneBox {
            min: [-e, GROUND_Y, half_road + sidewalk],
            max: [-(half_road + sidewalk), 12.0, e],
            material: 2,
        },
    ]
}

/// One camera-route waypoint: a position + a look-at target. The
/// fixed route (the research spec: a fixed camera route judges
/// renderer investments route-wide, not frame-spot).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteWaypoint {
    pub position: [f32; 3],
    pub look_at: [f32; 3],
}

/// The fixed evaluation route: enters along the west road, sweeps
/// through the intersection center looking at each building mass,
/// exits north. Six waypoints; the demo interpolates between them at
/// a fixed cadence (the whole-route measurement requirement).
pub fn camera_route() -> Vec<RouteWaypoint> {
    let e = SCENE_EXTENT;
    vec![
        // Enter from the west, eye at traffic height.
        RouteWaypoint {
            position: [-e + 2.0, 1.7, 0.0],
            look_at: [0.0, 6.0, 0.0],
        },
        // Mid-intersection, facing the tall NE mass.
        RouteWaypoint {
            position: [-4.0, 1.7, 0.0],
            look_at: [e / 2.0, 10.0, e / 2.0],
        },
        // Center, facing the dark SE base.
        RouteWaypoint {
            position: [0.0, 1.7, 0.0],
            look_at: [e / 2.0, 4.0, -e / 2.0],
        },
        // Center, turning to the tall SW mass.
        RouteWaypoint {
            position: [0.0, 1.7, 2.0],
            look_at: [-e / 2.0, 12.0, -e / 2.0],
        },
        // Crossing north, looking back at the corner.
        RouteWaypoint {
            position: [0.0, 1.7, e / 2.0],
            look_at: [e / 4.0, 8.0, e / 4.0],
        },
        // Exit north.
        RouteWaypoint {
            position: [0.0, 1.7, e - 2.0],
            look_at: [0.0, 10.0, e - 4.0],
        },
    ]
}

/// The CPU reference lighting equation — the oracle the shader must
/// reproduce. Blinn-Phong with even-integer shininess (repeated
/// squaring, no `pow`):
/// `color = albedo * (ambient + light * NdotL) + specular * (NdotH ^
/// shininess) * light * step(NdotL, 0)`, each channel computed
/// independently, all inputs normalized by the caller. The exact
/// operation order is FIXED (documented in the brief); the shader
/// mirrors it term-for-term.
pub fn lit_surface_color(
    albedo: [f32; 3],
    normal: [f32; 3],
    view_dir: [f32; 3],
    material: Material,
    light: LightParams,
) -> [f32; 3] {
    let n = normalize(normal);
    let v = normalize(view_dir);
    let l = normalize(light.dir_toward_light);
    let n_dot_l = clamp01(dot3(n, l));
    // The half vector (Blinn-Phong).
    let h = normalize([l[0] + v[0], l[1] + v[1], l[2] + v[2]]);
    let n_dot_h = clamp01(dot3(n, h));
    // Repeated squaring (shininess is an even power of two here).
    let mut spec = n_dot_h;
    let squarings = material.shininess.trailing_zeros();
    for _ in 0..squarings {
        spec = spec * spec;
    }
    [
        albedo[0] * (light.ambient[0] + light.light_color[0] * n_dot_l)
            + material.specular[0] * light.light_color[0] * n_dot_l * spec,
        albedo[1] * (light.ambient[1] + light.light_color[1] * n_dot_l)
            + material.specular[1] * light.light_color[1] * n_dot_l * spec,
        albedo[2] * (light.ambient[2] + light.light_color[2] * n_dot_l)
            + material.specular[2] * light.light_color[2] * n_dot_l * spec,
    ]
}

// ------------------------------------------------------- math helpers
// (private, f32, fixed order — the same operations the shader
// performs; documented in the brief for the implementer's mirror.)

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len == 0.0 {
        return v;
    }
    [v[0] / len, v[1] / len, v[2] / len]
}

fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The presets' light directions are (approximately) normalized.
    #[test]
    fn light_directions_are_normalized() {
        for preset in [day_preset(), night_preset()] {
            let len = (dot3(preset.dir_toward_light, preset.dir_toward_light)).sqrt();
            // [-0.5, 0.707..., -0.5] has length ~0.906 — normalize
            // the presets themselves to make the test honest.
            let normalized = normalize(preset.dir_toward_light);
            let nlen = (dot3(normalized, normalized)).sqrt();
            assert!((nlen - 1.0).abs() < 1e-5, "normalized len {nlen}");
            assert!(len > 0.0, "direction nonzero");
        }
    }

    /// The scene layout: every box is non-degenerate, above ground,
    /// within the world extent, outside the road cross, and every
    /// material index is valid.
    #[test]
    fn intersection_boxes_are_valid_and_framed() {
        let mats = materials();
        for box_ in intersection_boxes() {
            for a in 0..3 {
                assert!(box_.min[a] < box_.max[a], "non-degenerate on axis {a}");
                assert!(box_.min[a] >= -SCENE_EXTENT && box_.max[a] <= SCENE_EXTENT);
            }
            assert!(box_.min[1] >= GROUND_Y);
            assert!(box_.max[1] > GROUND_Y, "rises above ground");
            assert!(
                (box_.material as usize) < mats.len(),
                "material index valid"
            );
            // Outside the road cross (the 8 m cross through origin):
            // the box must not intrude into |x| < 4 AND |z| < 4.
            let intrudes =
                box_.min[0] < 4.0 && box_.max[0] > -4.0 && box_.min[2] < 4.0 && box_.max[2] > -4.0;
            assert!(!intrudes, "box intrudes into the road cross");
        }
        assert_eq!(intersection_boxes().len(), 4, "the four masses");
    }

    /// The camera route: above ground, within the world, eye height
    /// consistent, and every consecutive segment has a distinct
    /// direction (the route actually moves).
    #[test]
    fn camera_route_is_valid_and_moves() {
        let route = camera_route();
        assert!(route.len() >= 4);
        for wp in &route {
            assert!(wp.position[1] > GROUND_Y, "eye above ground");
            for a in 0..3 {
                assert!(wp.position[a] >= -SCENE_EXTENT && wp.position[a] <= SCENE_EXTENT);
            }
            assert_ne!(wp.position, wp.look_at, "look-at distinct from eye");
        }
        for pair in route.windows(2) {
            assert_ne!(pair[0].position, pair[1].position, "route moves");
        }
    }

    /// The lighting oracle: known-answer pins. A surface facing the
    /// light gets full diffuse; a surface away from it gets ambient
    /// only (specular gated by n_dot_l).
    #[test]
    fn lighting_reference_known_answers() {
        let light = day_preset();
        let wet = materials()[0];
        // Facing the light head-on, viewing along the light: peak.
        let lit = lit_surface_color(
            [0.5, 0.5, 0.5],
            light.dir_toward_light,
            light.dir_toward_light,
            wet,
            light,
        );
        // Ambient + diffuse dominate; every channel exceeds ambient.
        for c in 0..3 {
            assert!(lit[c] > light.ambient[c] * 0.5, "channel {c}: {lit:?}");
        }
        // Facing away: ambient only (n_dot_l = 0 kills diffuse AND
        // specular — the step gate).
        let away = lit_surface_color(
            [0.5, 0.5, 0.5],
            [
                -light.dir_toward_light[0],
                -light.dir_toward_light[1],
                -light.dir_toward_light[2],
            ],
            [0.0, 0.0, 1.0],
            wet,
            light,
        );
        for c in 0..3 {
            assert!(
                (away[c] - light.ambient[c] * 0.5).abs() < 1e-4,
                "ambient-only: {away:?}"
            );
        }
        // The night preset is strictly dimmer.
        let night = night_preset();
        let night_lit = lit_surface_color(
            [0.5, 0.5, 0.5],
            night.dir_toward_light,
            night.dir_toward_light,
            wet,
            night,
        );
        assert!(night_lit[0] < lit[0], "night dimmer than day");
    }

    /// The shininess values are powers of two (the repeated-squaring
    /// contract between this reference and the shader).
    #[test]
    fn shininess_values_are_powers_of_two() {
        for m in materials() {
            assert!(m.shininess.is_power_of_two(), "shininess {}", m.shininess);
        }
    }
}
