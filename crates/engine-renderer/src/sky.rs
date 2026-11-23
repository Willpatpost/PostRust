//! The procedural sky deterministic core (the Era-1 "atmosphere"
//! scaffold — see `docs/CYCLE6-ERA1-BRIEF.md`): the sky-gradient and
//! sun-disc color function, the day/night sky palettes, and the
//! light-parameter interpolation that drives the full cycle. Pure
//! data and pure IEEE f32 arithmetic in fixed order — the shader
//! mirrors the equation term-for-term (the lit-scene precedent).
//!
//! DETERMINISM CONTRACT: no libm. The gradient uses `smoothstep`
//! (arithmetic: 3t^2−2t^3), the sun disc uses a smooth threshold on
//! `dot(view, sun)` with FIXED literal constants (precomputed — the
//! cosine of the disc's angular radius written as a literal, never
//! a runtime `cos` call), and the cycle interpolation special-cases
//! its endpoints so t=0 and t=1 are BIT-EQUAL to the presets (a
//! naive lerp does not guarantee endpoint bit-equality; the test
//! pins this).
//!
//! STYLIZED-COHERENT (the owner-ratified Era-1 fidelity target): a
//! clean gradient sky with a visible warm sun disc and a cool
//! moonlit night — not scattering physics. The realism ceiling
//! begins with Era 3's asset pipeline.

use crate::lit_scene::LightParams;

/// The sun disc's angular half-radius, ~2.0 degrees — as the FIXED
/// literal cosine of that angle (0.99939 in f32; precomputed, never
/// a runtime transcendental).
const SUN_DISC_COS_RADIUS: f32 = 0.999_39;
/// The disc's soft edge, the cosine span over which the disc ramps
/// 0→1 (~0.4 degrees wider than the disc): a fixed literal.
const SUN_DISC_COS_SOFTNESS: f32 = 0.000_12;
/// The smoothstep ramp (arithmetic, fixed order).
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One sky palette: the horizon and zenith colors and the sun/moon
/// disc color, all lerped by the cycle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyPalette {
    /// The horizon band color (looking level).
    pub horizon: [f32; 3],
    /// The zenith color (looking up).
    pub zenith: [f32; 3],
    /// The celestial disc's color (the sun by day, the moon by
    /// night — one disc term, the palette decides which).
    pub disc: [f32; 3],
}

/// The day sky palette: warm light horizon, deeper blue zenith, a
/// warm near-white sun.
pub fn day_sky() -> SkyPalette {
    SkyPalette {
        horizon: [0.72, 0.82, 0.94],
        zenith: [0.22, 0.42, 0.78],
        disc: [1.00, 0.96, 0.88],
    }
}

/// The night sky palette: deep cool horizon, near-black zenith, a
/// cool pale moon.
pub fn night_sky() -> SkyPalette {
    SkyPalette {
        horizon: [0.10, 0.13, 0.22],
        zenith: [0.015, 0.025, 0.06],
        disc: [0.85, 0.90, 1.00],
    }
}

/// The sky color for one view direction, in the given palette, with
/// the celestial disc at `disc_dir` (unit, toward the body).
///
/// THE EQUATION (the shader mirrors term-for-term):
///   gradient = lerp(horizon, zenith, smoothstep(-0.08, 0.45, view.y))
///   d        = dot(view, disc_dir)
///   disc     = smoothstep(SUN_DISC_COS_RADIUS - SOFTNESS,
///                         SUN_DISC_COS_RADIUS, d)
///   color    = gradient + disc * disc_color
/// clamped to [0, 2] per channel (the additive sun may exceed 1 for
/// bloom to pick up; the tonemap brings it back).
pub fn sky_color(view_dir: [f32; 3], palette: &SkyPalette, disc_dir: [f32; 3]) -> [f32; 3] {
    let g = smoothstep(-0.08, 0.45, view_dir[1]);
    let d = view_dir[0] * disc_dir[0] + view_dir[1] * disc_dir[1] + view_dir[2] * disc_dir[2];
    let disc = smoothstep(
        SUN_DISC_COS_RADIUS - SUN_DISC_COS_SOFTNESS,
        SUN_DISC_COS_RADIUS,
        d,
    );
    [
        (palette.horizon[0]
            + (palette.zenith[0] - palette.horizon[0]) * g
            + disc * palette.disc[0])
            .clamp(0.0, 2.0),
        (palette.horizon[1]
            + (palette.zenith[1] - palette.horizon[1]) * g
            + disc * palette.disc[1])
            .clamp(0.0, 2.0),
        (palette.horizon[2]
            + (palette.zenith[2] - palette.horizon[2]) * g
            + disc * palette.disc[2])
            .clamp(0.0, 2.0),
    ]
}

/// One atmosphere frame: the lighting parameters AND the sky
/// palette together, at one cycle position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AtmosphereFrame {
    pub light: LightParams,
    pub sky: SkyPalette,
    /// The celestial disc's direction (unit, toward the body) — the
    /// sun by day, the moon by night (the presets' light directions).
    pub disc_dir: [f32; 3],
}

/// The full day/night cycle at `t` in [0, 1]: t=0 is EXACTLY the
/// day presets, t=1 EXACTLY the night presets (special-cased for
/// bit-equality — the naive lerp does not guarantee it; the test
/// pins this), and between them both the light and the sky palette
/// interpolate in fixed order. `cycle` is intended to advance with
/// the demo clock (the brief wires it); every intermediate value is
/// deterministic.
pub fn atmosphere_at(t: f32) -> AtmosphereFrame {
    let day_light = crate::lit_scene::day_preset();
    let night_light = crate::lit_scene::night_preset();
    let day_sky_palette = day_sky();
    let night_sky_palette = night_sky();
    if t <= 0.0 {
        return AtmosphereFrame {
            light: day_light,
            sky: day_sky_palette,
            disc_dir: day_light.dir_toward_light,
        };
    }
    if t >= 1.0 {
        return AtmosphereFrame {
            light: night_light,
            sky: night_sky_palette,
            disc_dir: night_light.dir_toward_light,
        };
    }
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    AtmosphereFrame {
        light: LightParams {
            dir_toward_light: [
                lerp(
                    day_light.dir_toward_light[0],
                    night_light.dir_toward_light[0],
                ),
                lerp(
                    day_light.dir_toward_light[1],
                    night_light.dir_toward_light[1],
                ),
                lerp(
                    day_light.dir_toward_light[2],
                    night_light.dir_toward_light[2],
                ),
            ],
            light_color: [
                lerp(day_light.light_color[0], night_light.light_color[0]),
                lerp(day_light.light_color[1], night_light.light_color[1]),
                lerp(day_light.light_color[2], night_light.light_color[2]),
            ],
            ambient: [
                lerp(day_light.ambient[0], night_light.ambient[0]),
                lerp(day_light.ambient[1], night_light.ambient[1]),
                lerp(day_light.ambient[2], night_light.ambient[2]),
            ],
        },
        sky: SkyPalette {
            horizon: [
                lerp(day_sky_palette.horizon[0], night_sky_palette.horizon[0]),
                lerp(day_sky_palette.horizon[1], night_sky_palette.horizon[1]),
                lerp(day_sky_palette.horizon[2], night_sky_palette.horizon[2]),
            ],
            zenith: [
                lerp(day_sky_palette.zenith[0], night_sky_palette.zenith[0]),
                lerp(day_sky_palette.zenith[1], night_sky_palette.zenith[1]),
                lerp(day_sky_palette.zenith[2], night_sky_palette.zenith[2]),
            ],
            disc: [
                lerp(day_sky_palette.disc[0], night_sky_palette.disc[0]),
                lerp(day_sky_palette.disc[1], night_sky_palette.disc[1]),
                lerp(day_sky_palette.disc[2], night_sky_palette.disc[2]),
            ],
        },
        // The disc direction follows the light direction (the sun
        // sets as the moon rises — one shared axis in this model).
        disc_dir: [
            lerp(
                day_light.dir_toward_light[0],
                night_light.dir_toward_light[0],
            ),
            lerp(
                day_light.dir_toward_light[1],
                night_light.dir_toward_light[1],
            ),
            lerp(
                day_light.dir_toward_light[2],
                night_light.dir_toward_light[2],
            ),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cycle endpoints are BIT-EQUAL to the presets — the
    /// special-case contract (a naive lerp would not guarantee it).
    #[test]
    fn atmosphere_endpoints_bit_equal_presets() {
        let dawn = atmosphere_at(0.0);
        assert_eq!(dawn.light, crate::lit_scene::day_preset());
        assert_eq!(dawn.sky, day_sky());
        let dusk = atmosphere_at(1.0);
        assert_eq!(dusk.light, crate::lit_scene::night_preset());
        assert_eq!(dusk.sky, night_sky());
        // Beyond the range clamps to the same endpoints.
        assert_eq!(atmosphere_at(-0.5).light, crate::lit_scene::day_preset());
        assert_eq!(atmosphere_at(1.5).light, crate::lit_scene::night_preset());
    }

    /// The cycle is monotone: the ambient dims and the zenith
    /// darkens as t rises 0→1 (the day-to-night direction).
    #[test]
    fn atmosphere_monotone_day_to_night() {
        let mut last = f32::INFINITY;
        for step in 0..=10 {
            let t = step as f32 / 10.0;
            let frame = atmosphere_at(t);
            let ambient_sum =
                frame.light.ambient[0] + frame.light.ambient[1] + frame.light.ambient[2];
            assert!(ambient_sum <= last + 1e-6, "ambient dims: t={t}");
            last = ambient_sum;
        }
        let mid = atmosphere_at(0.5);
        assert!(mid.sky.zenith[2] < day_sky().zenith[2]);
        assert!(mid.sky.zenith[2] > night_sky().zenith[2]);
    }

    /// The sky gradient: a level view lands between horizon and
    /// zenith (the smoothstep is mid-ramp at y = 0 — six percent
    /// toward zenith, NOT the pure horizon color); an up view is
    /// exactly the zenith (the ramp saturates); every output finite
    /// and within [0, 2].
    #[test]
    fn sky_gradient_extremes() {
        let palette = day_sky();
        // The sun placed off the up axis (so the up view tests the
        // pure gradient, not the disc).
        let sun = [0.0, 0.6, 0.8];
        let level = sky_color([1.0, 0.0, 0.0], &palette, sun);
        assert!(
            level[0] < palette.horizon[0]
                && level[0] > palette.horizon[0] - 0.2 * (palette.horizon[0] - palette.zenith[0]),
            "level view near-horizon: {level:?}"
        );
        let up = sky_color([0.0, 1.0, 0.0], &palette, sun);
        assert!((up[2] - palette.zenith[2]).abs() < 1e-5);
        for dir in [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.3, -0.8],
        ] {
            let c = sky_color(dir, &palette, sun);
            for ch in c {
                assert!(ch.is_finite() && (0.0..=2.0).contains(&ch));
            }
        }
    }

    /// The sun disc: looking directly at the body lights up beyond
    /// plain gradient (the bloom feeder); looking away adds nothing.
    #[test]
    fn sun_disc_appears_and_fades() {
        let palette = day_sky();
        let sun = [0.0, 1.0, 0.0];
        let at_sun = sky_color([0.0, 1.0, 0.0], &palette, sun);
        let away = sky_color([0.0, -1.0, 0.0], &palette, sun);
        // Looking straight at the sun: the disc term adds the full
        // disc color on top of the gradient — comfortably above the
        // away view plus a solid margin (the exact-equality trap the
        // 0.5 threshold fell into is documented here).
        assert!(
            at_sun[0] > away[0] + 0.3,
            "disc adds light: {at_sun:?} vs {away:?}"
        );
        // Slightly off the disc: no disc contribution (beyond the
        // softness span).
        let off = sky_color([0.1, 0.995, 0.0], &palette, sun);
        let off_grad = sky_color(
            [0.1, 0.995, 0.0],
            &SkyPalette {
                disc: [0.0; 3],
                ..palette
            },
            sun,
        );
        assert!(
            (off[0] - off_grad[0]).abs() < 1e-3,
            "off-disc: no disc term"
        );
    }
}
