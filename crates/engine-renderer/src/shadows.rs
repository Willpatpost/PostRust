//! The directional-shadow deterministic core (Cycle 6-F — see
//! `docs/AUTONOMOUS-CYCLE-PROTOCOL.md` §2 item 2): the light-space
//! orthographic projection, the single cascade's bounds, the
//! slope-scaled depth-bias model, and the CPU raycaster oracle that
//! pins the shadow map's depth values and the shadowed/unshadowed
//! pixel equivalence. ONE CASCADE, the corner scene, no framework —
//! the `proxy_instances.rs` pattern: everything deterministic and
//! pure lives here, fully tested; the implementer wires the
//! depth-only pass, the shadowed lit pipeline, and the capture-based
//! gates against these functions.
//!
//! DETERMINISM CONTRACT: every operation is pure IEEE f32 arithmetic
//! in a FIXED order. `sqrt` (basis normalization) is IEEE-754
//! correctly-rounded — one of the four basic operations the standard
//! requires to be bit-exact — unlike `sin`/`cos`, whose bits vary
//! across libm platforms (the standing libm-free discipline; see
//! `proxy_instances` for the turn-fraction workaround when a
//! transcendental would otherwise be needed). No packing here packs
//! bias or matrix bytes through anything but `f32` arithmetic and
//! `to_bits`.
//!
//! DEPTH CONVENTION: the light-space depth of a point is its
//! distance TOWARD the light, mapped linearly from the cascade's
//! near plane (0) to its far plane (1) — Vulkan's [0, 1] depth with
//! `LESS` comparison, so an occluder (closer to the light) carries a
//! SMALLER depth than the receiver behind it, exactly like the main
//! pass. The map's clear value is 1.0 (no occluder).

use crate::lit_scene::SceneBox;

/// The compare-time depth-bias parameters (caller policy, shared
/// verbatim by the shadowed shader's `shadow_factor` and the CPU
/// oracle — see [`depth_bias`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowBias {
    /// The constant floor, in [0, 1] depth units.
    pub constant: f32,
    /// The slope-scale multiplier (grazing surfaces bias harder).
    pub slope_scale: f32,
}

/// The shadow map's default texel resolution (square). One cascade
/// over the 40 m corner at 1024² resolves ~6 cm per texel — ample
/// for the building-mass shadows this package draws; revisitable
/// with rig evidence, never silently.
pub const SHADOW_MAP_EXTENT: u32 = 1024;

/// The self-hit skip distance for shadow rays: a receiver's own
/// surface sits at t = 0 along its shadow ray, so hits strictly
/// closer than this are the receiver itself, not an occluder
/// (mirrored by the shader's compare-time bias floor).
pub const SELF_HIT_EPSILON: f32 = 1.0 / 512.0;

/// One directional-light shadow cascade: the light-space
/// orthonormal basis, the orthographic bounds covering the scene's
/// world AABB, and the world→light-clip matrix the depth-only pass
/// and the shadowed lit pipeline share.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowCascade {
    /// The unit vector TOWARD the light (the projection's depth
    /// axis; the shadow camera looks along −light).
    pub light: [f32; 3],
    /// The map's horizontal axis (unit, world space).
    pub right: [f32; 3],
    /// The map's vertical axis (unit, world space).
    pub up: [f32; 3],
    /// The orthographic bounds in light-space axes: min/max of
    /// `dot(right, p)` and `dot(up, p)` over the scene AABB, and the
    /// near/far depths along `light`.
    pub min_right: f32,
    pub max_right: f32,
    pub min_up: f32,
    pub max_up: f32,
    pub near: f32,
    pub far: f32,
    /// The world→light-clip matrix (column-major, the repo's `Mat4`
    /// convention: clip = Σ columns[i] · p[i]; NDC z ∈ [0, 1]).
    pub matrix: [[f32; 4]; 4],
}

/// The scene's world AABB (the static corner masses; the ground
/// plane at y = 0 lies inside it). Pure data walk in fixed order.
pub fn scene_bounds(boxes: &[SceneBox]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for box_ in boxes {
        for axis in 0..3 {
            min[axis] = min[axis].min(box_.min[axis]);
            max[axis] = max[axis].max(box_.max[axis]);
        }
    }
    (min, max)
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len == 0.0 {
        return v;
    }
    [v[0] / len, v[1] / len, v[2] / len]
}

/// Builds the single shadow cascade for a light direction and the
/// scene's world AABB: the light-space basis (deterministic — the
/// reference axis flips only when the light is near-vertical, a
/// fixed threshold), the bounds (the AABB's 8 corners projected in
/// fixed order), and the matrix.
pub fn cascade_for(light_dir: [f32; 3], scene_min: [f32; 3], scene_max: [f32; 3]) -> ShadowCascade {
    let light = normalize3(light_dir);
    // The reference axis for the map's vertical: +Y unless the light
    // is near-vertical (the cross product would degenerate); the
    // 0.999 threshold is fixed and documented.
    let reference = if dot3(light, [0.0, 1.0, 0.0]).abs() < 0.999 {
        [0.0, 1.0, 0.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let right = normalize3(cross3(reference, light));
    let up = cross3(light, right);

    // The AABB's 8 corners, in fixed binary-count order.
    let mut min_right = f32::MAX;
    let mut max_right = f32::MIN;
    let mut min_up = f32::MAX;
    let mut max_up = f32::MIN;
    let mut near = f32::MAX;
    let mut far = f32::MIN;
    for corner in 0..8 {
        let world = [
            if corner & 1 == 0 {
                scene_min[0]
            } else {
                scene_max[0]
            },
            if corner & 2 == 0 {
                scene_min[1]
            } else {
                scene_max[1]
            },
            if corner & 4 == 0 {
                scene_min[2]
            } else {
                scene_max[2]
            },
        ];
        let r = dot3(right, world);
        let u = dot3(up, world);
        let d = dot3(light, world);
        min_right = min_right.min(r);
        max_right = max_right.max(r);
        min_up = min_up.min(u);
        max_up = max_up.max(u);
        near = near.min(d);
        far = far.max(d);
    }

    let span_right = max_right - min_right;
    let span_up = max_up - min_up;
    let span_depth = far - near;
    let center_right = (min_right + max_right) * 0.5;
    let center_up = (min_up + max_up) * 0.5;
    // clip.x = (dot(right,p) − center_right) · 2/span_right, and the
    // same shape for y. DEPTH: (far − dot(light,p)) / span — the
    // point CLOSEST to the light maps to 0 and the farthest to 1,
    // the camera-like [0, 1] LESS convention the depth-only pass and
    // the shadowed compare both rely on (an occluder nearer the
    // light carries the SMALLER depth; the map's clear value 1.0
    // means "nothing nearer than the farthest plane"). The inverted
    // form (dot − near) was the 6-F build's one real convention bug:
    // it made the GROUND win the depth test over every roof (the
    // vertical-light probe caught it — the whole map read the
    // ground's depth).
    // Column-major: column `a` is the world axis-a unit vector's
    // light-clip image — the basis components distribute ACROSS
    // columns (row `o` carries output o's scale). Writing whole
    // scaled basis vectors as columns is the transpose bug the
    // corner-round-trip tests below exist to catch.
    let sx = 2.0 / span_right;
    let sy = 2.0 / span_up;
    let sz = 1.0 / span_depth;
    let matrix = [
        [right[0] * sx, up[0] * sy, -light[0] * sz, 0.0],
        [right[1] * sx, up[1] * sy, -light[1] * sz, 0.0],
        [right[2] * sx, up[2] * sy, -light[2] * sz, 0.0],
        [-center_right * sx, -center_up * sy, far * sz, 1.0],
    ];

    ShadowCascade {
        light,
        right,
        up,
        min_right,
        max_right,
        min_up,
        max_up,
        near,
        far,
        matrix,
    }
}

/// A point's light-space depth in [0, 1]: 0 closest to the light,
/// 1 farthest (the camera-like LESS convention the map rasterizes
/// and the shadowed compare uses).
pub fn light_depth(cascade: &ShadowCascade, point: [f32; 3]) -> f32 {
    (cascade.far - dot3(cascade.light, point)) / (cascade.far - cascade.near)
}

/// The slope-scaled compare-time depth bias (the Vulkan
/// `vkCmdSetDepthBias` equation's software form, applied at COMPARE
/// time so both the shader and this CPU oracle compute it from the
/// same inputs): `bias = constant + slope_scale · m`, where `m` is
/// the receiver's light-space depth slope. For the corner scene's
/// axis-aligned faces under a directional light the slope reduces to
/// a fixed function of the surface normal (documented; the shader
/// mirrors this exact expression).
pub fn depth_bias(normal: [f32; 3], light: [f32; 3], constant: f32, slope_scale: f32) -> f32 {
    // The light-space depth slope of a plane with world normal n:
    // the depth axis is `light`, so the slope is the plane's depth
    // change per light-space horizontal step — |dot(n, light)| in
    // the denominator pushes grazing surfaces toward the max-slope
    // clamp, the classic peter-panning/lacing trade.
    let facing = dot3(normal, light).abs().max(1.0 / 64.0);
    constant + slope_scale * (1.0 / facing - 1.0)
}

/// The ray–AABB slab test: the entry distance along `dir` (unit) or
/// `None` when the ray misses or starts outside. Fixed operation
/// order; ties across boxes resolve to the EARLIER table entry (the
/// caller's iteration order is the tie-break).
fn ray_box_entry(origin: [f32; 3], dir: [f32; 3], min: [f32; 3], max: [f32; 3]) -> Option<f32> {
    let mut t_enter = 0.0f32;
    let mut t_exit = f32::INFINITY;
    for axis in 0..3 {
        if dir[axis] == 0.0 {
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return None;
            }
            continue;
        }
        let inv = 1.0 / dir[axis];
        let (t0, t1) = if inv >= 0.0 {
            (
                (min[axis] - origin[axis]) * inv,
                (max[axis] - origin[axis]) * inv,
            )
        } else {
            (
                (max[axis] - origin[axis]) * inv,
                (min[axis] - origin[axis]) * inv,
            )
        };
        t_enter = t_enter.max(t0);
        t_exit = t_exit.min(t1);
        if t_enter > t_exit {
            return None;
        }
    }
    Some(t_enter)
}

/// The first occluder hit along the shadow ray from `point` toward
/// the light: the distance to the nearest box whose entry is
/// strictly beyond the self-hit epsilon, or `None` when the point
/// sees the light. Boxes iterate in table order (the deterministic
/// tie-break). The ground plane is NOT an occluder (nothing stands
/// below it).
pub fn shadow_ray_hit(point: [f32; 3], cascade: &ShadowCascade, boxes: &[SceneBox]) -> Option<f32> {
    shadow_ray_hit_detail(point, cascade, boxes).map(|(_, t)| t)
}

/// The detailed form (the map oracle's): the hit box's table index
/// alongside the distance, for edge-margin computation.
pub fn shadow_ray_hit_detail(
    point: [f32; 3],
    cascade: &ShadowCascade,
    boxes: &[SceneBox],
) -> Option<(usize, f32)> {
    let mut nearest: Option<(usize, f32)> = None;
    for (index, box_) in boxes.iter().enumerate() {
        if let Some(t) = ray_box_entry(point, cascade.light, box_.min, box_.max)
            && t > SELF_HIT_EPSILON
            && nearest.is_none_or(|(_, best)| t < best)
        {
            nearest = Some((index, t));
        }
    }
    nearest
}

/// One map texel's expected depth (the known-answer oracle): the
/// first surface the LIGHT's ray reaches, cast from the cascade's
/// far plane down along −light — exactly the first FRONT-facing
/// surface the depth-only pass rasterizes (a building's lit-side
/// entry — roof or lit wall — or the ground quad's crossing). The
/// shadow-TESTING oracle ignores the ground as an occluder; the MAP
/// oracle cannot (the mesh draws it). Depths map into [0, 1] (1 −
/// t/span from the far plane), with the clear value 1.0 for no hit.
/// `hit`/`box_index` carry the surface for the gate's edge-margin
/// skip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TexelExpectation {
    pub depth: f32,
    /// The hit point on the rasterized surface (None for no hit).
    pub hit: Option<[f32; 3]>,
    /// The hit box's table index (None = the ground quad).
    pub box_index: Option<usize>,
}

/// The expected depth the depth-only pass rasterizes at map texel
/// `(x, y)` (row 0 first — the framebuffer convention the
/// shader-boundary y flip produces; see [`texel_ray_origin`]).
pub fn map_texel_depth(
    cascade: &ShadowCascade,
    boxes: &[SceneBox],
    x: u32,
    y: u32,
    extent: u32,
) -> TexelExpectation {
    let origin = texel_ray_origin(cascade, x, y, extent);
    let span = cascade.far - cascade.near;
    // The descending light ray: −light.
    let descent = [-cascade.light[0], -cascade.light[1], -cascade.light[2]];
    // The map cast accepts t >= 0 (unlike the shadow-TESTING cast's
    // SELF_HIT_EPSILON, which exists for receiver self-hits): the
    // far plane TOUCHES the scene's max-dot surface — the tallest
    // roof — and the map legitimately rasterizes that surface at
    // depth 0, exactly where the far-plane origin sits.
    let mut best: Option<(Option<usize>, f32)> = None;
    for (index, box_) in boxes.iter().enumerate() {
        if let Some(t) = ray_box_entry(origin, descent, box_.min, box_.max)
            && t >= 0.0
            && best.is_none_or(|(_, t_best)| t < t_best)
        {
            best = Some((Some(index), t));
        }
    }
    // The ground quad: the ray descends through y = 0 within the
    // scene extent (the far-plane origins sit above/behind the
    // scene; the light is above the horizon for both presets).
    if cascade.light[1] > 0.0 && origin[1] > 0.0 {
        let t_ground = origin[1] / cascade.light[1];
        if best.is_none_or(|(_, t)| t_ground < t) {
            let crossing = [
                origin[0] + descent[0] * t_ground,
                0.0,
                origin[2] + descent[2] * t_ground,
            ];
            let inside = crossing[0].abs() <= crate::lit_scene::SCENE_EXTENT
                && crossing[2].abs() <= crate::lit_scene::SCENE_EXTENT;
            if inside && t_ground >= 0.0 {
                best = Some((None, t_ground));
            }
        }
    }
    match best {
        Some((box_index, t)) => TexelExpectation {
            // t measured from the far plane (depth 0, the light
            // side): depth = t/span — the fixed convention's form.
            depth: t / span,
            hit: Some([
                origin[0] + descent[0] * t,
                origin[1] + descent[1] * t,
                origin[2] + descent[2] * t,
            ]),
            box_index,
        },
        None => TexelExpectation {
            depth: 1.0,
            hit: None,
            box_index: None,
        },
    }
}

/// Whether a surface point is in shadow (the CPU oracle the pixel
/// gate asserts against; the shader's compare mirrors it).
pub fn is_shadowed(point: [f32; 3], cascade: &ShadowCascade, boxes: &[SceneBox]) -> bool {
    shadow_ray_hit(point, cascade, boxes).is_some()
}

/// Inverts one shadow-map texel to the world-space origin of its
/// LIGHT ray — the point on the cascade's FAR plane (the light
/// side): casting DOWN along −light from here reaches exactly the
/// surfaces the depth-only pass rasterizes (the first FRONT-facing
/// surface from the light — the lit faces), which is why the map
/// oracle casts from this side while the shadow-TESTING oracle
/// (receiver → light) casts the other way. The texel center's NDC
/// carries the shader-boundary Y flip — the naga epilogue negates
/// `gl_Position.y`, so framebuffer row j carries NDC y = −((j +
/// 0.5)·2/n − 1).
pub fn texel_ray_origin(cascade: &ShadowCascade, x: u32, y: u32, extent: u32) -> [f32; 3] {
    let ndc_x = (x as f32 + 0.5) * 2.0 / extent as f32 - 1.0;
    let ndc_y = -((y as f32 + 0.5) * 2.0 / extent as f32 - 1.0);
    let r = cascade.min_right + (ndc_x + 1.0) * 0.5 * (cascade.max_right - cascade.min_right);
    let u = cascade.min_up + (ndc_y + 1.0) * 0.5 * (cascade.max_up - cascade.min_up);
    [
        cascade.right[0] * r + cascade.up[0] * u + cascade.light[0] * cascade.far,
        cascade.right[1] * r + cascade.up[1] * u + cascade.light[1] * cascade.far,
        cascade.right[2] * r + cascade.up[2] * u + cascade.light[2] * cascade.far,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lit_scene::{day_preset, intersection_boxes};

    fn corner_cascade() -> ShadowCascade {
        let boxes = intersection_boxes();
        let (min, max) = scene_bounds(boxes);
        cascade_for(day_preset().dir_toward_light, min, max)
    }

    /// The basis is orthonormal, right-handed in (light, right, up),
    /// and the reference-axis flip engages only near-vertical.
    #[test]
    fn cascade_basis_is_orthonormal() {
        for light in [
            day_preset().dir_toward_light,
            night_preset_dir(),
            [0.0, 1.0, 0.0],
            [0.0, 0.9999, 0.0],
            [0.7, 0.7, 0.0],
        ] {
            let (min, max) = scene_bounds(intersection_boxes());
            let cascade = cascade_for(light, min, max);
            let unit = |v: [f32; 3]| (dot3(v, v)).sqrt();
            assert!((unit(cascade.light) - 1.0).abs() < 1e-6, "light unit");
            assert!((unit(cascade.right) - 1.0).abs() < 1e-6, "right unit");
            assert!((unit(cascade.up) - 1.0).abs() < 1e-6, "up unit");
            assert!(dot3(cascade.right, cascade.up).abs() < 1e-6, "r⊥u");
            assert!(dot3(cascade.light, cascade.right).abs() < 1e-6, "l⊥r");
            assert!(dot3(cascade.light, cascade.up).abs() < 1e-6, "l⊥u");
        }
        // The near-vertical light flips the reference axis (right
        // stays non-degenerate) instead of producing a zero cross.
        // With the light along +Y, +Y IS the depth axis, so the
        // map's up becomes +X (the flipped reference).
        let (min, max) = scene_bounds(intersection_boxes());
        let vertical = cascade_for([0.0, 1.0, 0.0], min, max);
        assert!(vertical.right != [0.0; 3]);
        assert_eq!(
            vertical.up,
            [1.0, 0.0, 0.0],
            "vertical light: +Y is the depth axis"
        );
    }

    fn night_preset_dir() -> [f32; 3] {
        crate::lit_scene::night_preset().dir_toward_light
    }

    /// The matrix maps the scene AABB's corners into the clip box:
    /// NDC x, y ∈ [−1, 1] and depth ∈ [0, 1], with the near-plane
    /// corner at depth 0 and the far at 1 (the [0, 1] LESS contract).
    #[test]
    fn cascade_matrix_covers_the_scene() {
        let cascade = corner_cascade();
        let (min, max) = scene_bounds(intersection_boxes());
        for corner in 0..8 {
            let world = [
                if corner & 1 == 0 { min[0] } else { max[0] },
                if corner & 2 == 0 { min[1] } else { max[1] },
                if corner & 4 == 0 { min[2] } else { max[2] },
            ];
            let clip: [f32; 4] = std::array::from_fn(|i| {
                cascade.matrix[0][i] * world[0]
                    + cascade.matrix[1][i] * world[1]
                    + cascade.matrix[2][i] * world[2]
                    + cascade.matrix[3][i]
            });
            assert_eq!(clip[3], 1.0);
            assert!(
                clip[0] >= -1.5 && clip[0] <= 1.5,
                "NDC x within the padded bounds: {}",
                clip[0]
            );
            assert!(
                clip[1] >= -1.5 && clip[1] <= 1.5,
                "NDC y within the padded bounds: {}",
                clip[1]
            );
            assert!(clip[2] >= -1e-4 && clip[2] <= 1.0 + 1e-4, "depth in [0,1]");
        }
        // A point ON the near plane maps to depth 0; light_depth
        // agrees with the matrix's z row.
        let probe = [4.0, 0.0, 4.0];
        let depth = light_depth(&cascade, probe);
        let clip_z = cascade.matrix[0][2] * probe[0]
            + cascade.matrix[1][2] * probe[1]
            + cascade.matrix[2][2] * probe[2]
            + cascade.matrix[3][2];
        assert!((depth - clip_z).abs() < 1e-6, "{depth} vs {clip_z}");
    }

    /// The CPU raycaster on synthetic geometry with hand-computed
    /// answers: a lone 2 m cube under a vertical and a horizontal
    /// light, the self-hit epsilon, and the nearest-of-two
    /// tie-break (table order).
    #[test]
    fn raycaster_pins_synthetic_geometry() {
        let cube = SceneBox {
            min: [0.0, 0.0, 0.0],
            max: [2.0, 2.0, 2.0],
            material: 0,
        };
        let vertical = cascade_for([0.0, 1.0, 0.0], [-1.0, -1.0, -1.0], [3.0, 3.0, 3.0]);
        // Below the cube, light overhead: the cube's shadow.
        assert!(is_shadowed([1.0, -1.0, 1.0], &vertical, &[cube]));
        // Above the cube: the ray leaves toward the light.
        assert!(!is_shadowed([1.0, 3.0, 1.0], &vertical, &[cube]));
        // Beside the cube: clear sky.
        assert!(!is_shadowed([3.0, 1.0, 1.0], &vertical, &[cube]));
        // ON the cube's bottom face: its own surface at t = 0 — the
        // epsilon skips it (no self-shadowing).
        assert!(!is_shadowed([1.0, 0.0, 1.0], &vertical, &[cube]));
        // Inside the cube: entry at t = 0, skipped the same way.
        assert!(!is_shadowed([1.0, 1.0, 1.0], &vertical, &[cube]));

        let east = cascade_for([1.0, 0.0, 0.0], [-3.0, -1.0, -1.0], [3.0, 3.0, 3.0]);
        // West of the cube, light due east: shadowed (entry at t=1).
        assert_eq!(shadow_ray_hit([-1.0, 1.0, 1.0], &east, &[cube]), Some(1.0));
        // East of it: lit.
        assert!(!is_shadowed([3.0, 1.0, 1.0], &east, &[cube]));
        // ON the west face: t = 0, the epsilon skip.
        assert_eq!(
            shadow_ray_hit([0.0, 1.0, 1.0], &east, &[cube]),
            None,
            "the receiver's own face is not an occluder"
        );

        // Nearest of two occluders wins; equal distances resolve to
        // the earlier table entry (both hit at the same t here, the
        // first listed is reported — the deterministic tie-break).
        let near = SceneBox {
            min: [1.0, 0.0, 0.0],
            max: [2.0, 2.0, 2.0],
            material: 0,
        };
        let far = SceneBox {
            min: [3.0, 0.0, 0.0],
            max: [4.0, 2.0, 2.0],
            material: 0,
        };
        assert_eq!(
            shadow_ray_hit([-1.0, 1.0, 1.0], &east, &[far, near]),
            Some(2.0),
            "the nearer box's entry distance"
        );
    }

    /// The corner scene under the day preset: the 45° sun with the
    /// 22 m southwest mass shadows the cross center (the scene's
    /// signature shadow — the reverse ray reaches the mass's near
    /// wall plane at y = 8.49 < 22), the scene has BOTH lit and
    /// shadowed ground (a scene where everything is one or the other
    /// would make the pixel gate vacuous), and a point inside a mass
    /// does not self-shadow.
    #[test]
    fn raycaster_pins_the_corner_shadows() {
        let boxes = intersection_boxes();
        let cascade = corner_cascade();
        // The center: shadowed by the SW mass's near edge.
        assert!(
            is_shadowed([0.0, 0.0, 0.0], &cascade, boxes),
            "the day sun's signature shadow falls on the cross center"
        );
        // Deep inside the tall SW mass: only itself along the ray.
        assert!(!is_shadowed([-20.0, 10.0, -20.0], &cascade, boxes));
        // Both regimes exist on the ground: sample the road cross
        // and the world square's open ring outside the masses.
        let mut lit = 0;
        let mut shadowed = 0;
        for x in -39..=39 {
            for z in -39..=39 {
                let point = [x as f32, 0.0, z as f32];
                let inside = boxes.iter().any(|b| {
                    b.min[0] < point[0]
                        && point[0] < b.max[0]
                        && b.min[2] < point[2]
                        && point[2] < b.max[2]
                });
                if inside {
                    continue;
                }
                if is_shadowed(point, &cascade, boxes) {
                    shadowed += 1;
                } else {
                    lit += 1;
                }
            }
        }
        assert!(lit > 200, "lit ground samples: {lit}");
        assert!(shadowed > 200, "shadowed ground samples: {shadowed}");
    }

    /// The texel inversion is a true inverse of the matrix: a
    /// texel's ray origin maps back into the same texel's NDC cell
    /// (x within the cell, y within the cell AFTER the flip), at
    /// depth 1 (the far plane — the light side the oracle casts
    /// from).
    #[test]
    fn texel_ray_round_trips_through_the_matrix() {
        let cascade = corner_cascade();
        let extent = 64u32;
        for (x, y) in [(0u32, 0u32), (31, 32), (63, 63), (17, 40)] {
            let origin = texel_ray_origin(&cascade, x, y, extent);
            let clip: [f32; 4] = std::array::from_fn(|i| {
                cascade.matrix[0][i] * origin[0]
                    + cascade.matrix[1][i] * origin[1]
                    + cascade.matrix[2][i] * origin[2]
                    + cascade.matrix[3][i]
            });
            // The cell's x range (before the y flip) and the flipped
            // y range the framebuffer row sees: NDC → texel centers.
            let cell = |v: f32, n: u32| (v + 1.0) * n as f32 / 2.0 - 0.5;
            let cx = cell(clip[0], extent);
            let cy = cell(-clip[1], extent);
            assert!((cx - x as f32).abs() < 1e-3, "texel x {x}: {cx}");
            assert!((cy - y as f32).abs() < 1e-3, "texel y {y}: {cy}");
            assert!(
                (clip[2] - 0.0).abs() < 1e-6,
                "the origin sits on the far plane (depth 0, closest to the light)"
            );
        }
    }

    /// The bias model: facing surfaces get the constant floor,
    /// grazing surfaces grow (the peter-panning trade), and the
    /// parameters pass through linearly.
    #[test]
    fn depth_bias_scales_with_grazing() {
        let light = [0.0, 1.0, 0.0];
        let facing = depth_bias([0.0, 1.0, 0.0], light, 0.001, 0.005);
        assert!((facing - 0.001).abs() < 1e-9, "the constant floor");
        let grazing = depth_bias([1.0, 0.0, 0.0], light, 0.001, 0.005);
        assert!(grazing > facing * 10.0, "grazing surfaces bias hard");
        let doubled = depth_bias([0.0, 1.0, 0.0], light, 0.002, 0.005);
        assert!(
            (doubled - 0.002).abs() < 1e-9,
            "the constant passes through"
        );
    }

    /// The map oracle on the synthetic cube (hand-computed): a
    /// vertical light over a 2 m cube inside a [−1, 3]³ AABB — the
    /// cascade's basis is right = +Z, up = +X (the reference flip),
    /// near = −1, far = 3, span = 4. The center texel's ray descends
    /// from the far plane (y = 3) onto the cube's ROOF (y = 2) at
    /// t = 1 → depth 1/4 = 0.25 — the first front-facing surface
    /// from the light (the closest-to-light occluder wins the LESS
    /// depth test), exactly what the depth-only pass rasterizes.
    /// Determinism is pinned alongside.
    #[test]
    fn map_texel_depth_pins_the_synthetic_cube() {
        let cube = SceneBox {
            min: [0.0, 0.0, 0.0],
            max: [2.0, 2.0, 2.0],
            material: 0,
        };
        let cascade = cascade_for([0.0, 1.0, 0.0], [-1.0, -1.0, -1.0], [3.0, 3.0, 3.0]);
        assert_eq!(cascade.right, [0.0, 0.0, 1.0]);
        assert_eq!(cascade.up, [1.0, 0.0, 0.0]);
        assert!((cascade.near + 1.0).abs() < 1e-6);
        assert!((cascade.far - 3.0).abs() < 1e-6);
        let n = 64;
        let center = map_texel_depth(&cascade, &[cube], n / 2, n / 2, n);
        assert!((center.depth - 0.25).abs() < 1e-4, "{}", center.depth);
        assert_eq!(center.box_index, Some(0));
        let hit = center.hit.unwrap();
        assert!((hit[1] - 2.0).abs() < 1e-3, "the cube's roof");
        // A corner texel off the cube: the ground quad's crossing
        // (never the clear value inside the extent — the ray
        // descends through y = 0).
        let corner = map_texel_depth(&cascade, &[cube], 0, 0, n);
        assert!(corner.depth >= 0.0 && corner.depth <= 1.0);
        assert_eq!(corner.box_index, None, "the ground, not the cube");
        // Determinism.
        assert_eq!(map_texel_depth(&cascade, &[cube], n / 2, n / 2, n), center);
    }

    /// The corner scene's map oracle covers every expectation kind
    /// (box hits, ground hits) with depths in [0, 1] — the coarse
    /// census the GPU gate's full-map comparison rests on.
    #[test]
    fn map_texel_depth_censuses_the_corner_scene() {
        let boxes = intersection_boxes();
        let cascade = corner_cascade();
        let n = 128;
        let mut box_hits = 0;
        let mut ground_hits = 0;
        for y in (0..n).step_by(3) {
            for x in (0..n).step_by(3) {
                let texel = map_texel_depth(&cascade, boxes, x, y, n);
                assert!(texel.depth >= 0.0 && texel.depth <= 1.0);
                match texel.box_index {
                    Some(index) => {
                        let hit = texel.hit.unwrap();
                        let box_ = &boxes[index];
                        for axis in 0..3 {
                            assert!(
                                hit[axis] >= box_.min[axis] - 1e-3
                                    && hit[axis] <= box_.max[axis] + 1e-3,
                                "the hit lies on its box: {hit:?}"
                            );
                        }
                        box_hits += 1;
                    }
                    None if texel.hit.is_some() => {
                        assert!(texel.hit.unwrap()[1].abs() < 1e-3, "ground hits lie on y=0");
                        ground_hits += 1;
                    }
                    None => {}
                }
            }
        }
        assert!(box_hits > 100, "box texels: {box_hits}");
        // The ground visible FROM THE LIGHT is the lit ground — a
        // minority of the map (the buildings occlude the rest from
        // the light's viewpoint; that occlusion IS the shadow map's
        // content), so the sampled bound is small by design.
        assert!(ground_hits > 30, "ground texels: {ground_hits}");
    }
}
