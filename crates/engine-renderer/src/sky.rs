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

/// The inverse of a column-major 4×4 (the repo's `Mat4` convention:
/// `m[column][row]`, clip = Σ columns[i] · p[i]) — pure IEEE f32
/// arithmetic in FIXED order (the cofactor expansion, transcribed
/// through a row-major scratch so the indexing convention cannot
/// transpose silently; the round-trip tests pin the contract).
///
/// This exists for the sky pass: the fullscreen triangle's vertex
/// shader reconstructs the view ray from
/// `inverse(view_proj) · NDC`, so the CPU computes the inverse the
/// shader consumes. Singular inputs return the identity (never
/// reached by the camera matrices this exists for — a perspective
/// × rigid-view product with near > 0 is always invertible; the
/// contract is documented and tested).
pub fn mat4_inverse(m: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    // Row-major scratch: rm[row][col] = m[col][row] (the repo's
    // column-major convention transcribed once, so the cofactor
    // indexing below reads naturally and cannot transpose silently).
    let rm: [[f32; 4]; 4] = std::array::from_fn(|row| std::array::from_fn(|col| m[col][row]));
    // The 3×3 determinant over explicit row/column picks, fixed
    // operation order (the Sarrus expansion).
    let det3 = |rows: [usize; 3], cols: [usize; 3]| -> f32 {
        let (r0, r1, r2) = (rows[0], rows[1], rows[2]);
        let (c0, c1, c2) = (cols[0], cols[1], cols[2]);
        rm[r0][c0] * (rm[r1][c1] * rm[r2][c2] - rm[r1][c2] * rm[r2][c1])
            - rm[r0][c1] * (rm[r1][c0] * rm[r2][c2] - rm[r1][c2] * rm[r2][c0])
            + rm[r0][c2] * (rm[r1][c0] * rm[r2][c1] - rm[r1][c1] * rm[r2][c0])
    };
    // The index sets excluding one row/column, in fixed order.
    const REST: [[usize; 3]; 4] = [[1, 2, 3], [0, 2, 3], [0, 1, 3], [0, 1, 2]];
    // The cofactor C(i,j) = (−1)^{i+j} · det(minor without row i,
    // column j).
    let cofactor = |i: usize, j: usize| -> f32 {
        let sign = if (i + j).is_multiple_of(2) { 1.0 } else { -1.0 };
        sign * det3(REST[i], REST[j])
    };
    // The determinant (the row-0 cofactor expansion).
    let determinant = rm[0][0] * cofactor(0, 0)
        + rm[0][1] * cofactor(0, 1)
        + rm[0][2] * cofactor(0, 2)
        + rm[0][3] * cofactor(0, 3);
    if determinant == 0.0 {
        return [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
    }
    let inverse_determinant = 1.0 / determinant;
    // The adjugate is the cofactor matrix TRANSPOSED: inverse(r,c) =
    // C(c,r)/det — scaled once per element by the reciprocal,
    // written back into the column-major output layout.
    std::array::from_fn(|column| {
        std::array::from_fn(|row| cofactor(column, row) * inverse_determinant)
    })
}

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

/// The day/night CYCLE position for a demo-clock reading: the
/// triangle wave over the period — `t` advances 0→1 over the first
/// half (day→night) and back 1→0 over the second (night→day),
/// smooth at both endpoints (no sunrise snap), every intermediate
/// value the deterministic [`atmosphere_at`] blend. `elapsed` may
/// run arbitrarily long or negative (the phase wraps through
/// `rem_euclid`); `period` must be positive (the CLI validates —
/// the caller's contract).
pub fn cycle_position(elapsed_seconds: f32, period_seconds: f32) -> f32 {
    // The phase advances over HALF periods: 0→1 across the first
    // half (day→night), 1→0 across the second (night→day).
    let phase = (elapsed_seconds / period_seconds * 2.0).rem_euclid(2.0);
    if phase <= 1.0 { phase } else { 2.0 - phase }
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

    /// The inverse: identity inverts to itself, a singular input
    /// returns the identity (the documented contract), and a
    /// translate+scale product round-trips to the identity.
    #[test]
    fn mat4_inverse_identity_singular_and_round_trip() {
        let identity = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        assert_eq!(mat4_inverse(&identity), identity);
        let singular = [
            [1.0, 0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        assert_eq!(mat4_inverse(&singular), identity, "the documented fallback");
        // A column-major translate+scale: column 0 = (2,0,0,0),
        // column 3 = (10, 20, 30, 1).
        let ts = [
            [2.0, 0.0, 0.0, 0.0],
            [0.0, 2.0, 0.0, 0.0],
            [0.0, 0.0, 2.0, 0.0],
            [10.0, 20.0, 30.0, 1.0],
        ];
        let inverse = mat4_inverse(&ts);
        let product = mul4(&ts, &inverse);
        assert_near_identity(&product);
        let product = mul4(&inverse, &ts);
        assert_near_identity(&product);
    }

    /// THE RAY PROPERTY (what the sky pass consumes): for a
    /// perspective × rigid-view product, the inverse maps NDC (0,0)
    /// back onto the camera's forward axis — every depth slice's
    /// point lies on the forward ray, and the axis points at the
    /// camera's look-at target. The sky gate's known-answer pixels
    /// rest on this (the screen-center ray is definitionally the
    /// camera axis, independent of the reconstruction).
    #[test]
    fn mat4_inverse_maps_the_center_axis_to_the_camera_forward() {
        // A representative view-projection: the corner route's own
        // shape (65° Vulkan perspective, near 0.1, far 150) times a
        // yawed/pitched rigid view.
        let perspective = |aspect: f32| -> [[f32; 4]; 4] {
            let fov = 65_f32.to_radians();
            let (near, far) = (0.1f32, 150.0f32);
            let focal = 1.0 / (fov / 2.0).tan();
            // engine-slice math.rs's own form (column-major, the
            // [0,1] Vulkan depth, w = −z_view: the camera looks down
            // −Z_view).
            let mut matrix = [[0.0f32; 4]; 4];
            matrix[0][0] = focal / aspect;
            matrix[1][1] = focal;
            matrix[2][2] = far / (near - far);
            matrix[2][3] = -1.0;
            matrix[3][2] = near * far / (near - far);
            matrix
        };
        // The rigid rotation, ROW-MAJOR r[row][col] (an orthonormal
        // yaw-then-pitch composition; the test is self-consistent —
        // any orthonormal R exercises the inverse the same way).
        let (yaw, pitch) = (0.7f32, 0.25f32);
        let (sy, cy) = (yaw.sin(), yaw.cos());
        let (sp, cp) = (pitch.sin(), pitch.cos());
        let r = [
            [cy, sp * sy, -cp * sy, 0.0],
            [0.0, cp, sp, 0.0],
            [sy, -sp * cy, cp * cy, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let position = [3.0, 1.7, -5.0];
        // The view: Rᵀ · T(−position). Its LINEAR part in the
        // column-major storage equals r's array element-wise
        // (view[c][r] = R(c, r) — the transpose folds into the
        // storage convention); the translation column is
        // −(R's rows)·position per output row: view[3][i] =
        // −Σ_j r[j][i]·p[j].
        let mut view = r;
        for i in 0..3 {
            view[3][i] = -(r[0][i] * position[0] + r[1][i] * position[1] + r[2][i] * position[2]);
        }
        let view_proj = mul4(&perspective(1.0), &view);
        let inverse = mat4_inverse(&view_proj);
        // The camera's forward axis: the local −Z axis in world —
        // the negated COLUMN 2 of R.
        let forward = [-r[0][2], -r[1][2], -r[2][2]];
        for depth in [0.0f32, 0.5, 1.0] {
            let clip: [f32; 4] = std::array::from_fn(|i| {
                inverse[0][i] * 0.0 + inverse[1][i] * 0.0 + inverse[2][i] * depth + inverse[3][i]
            });
            let point = [clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]];
            // The point lies on the forward ray from the camera
            // position (parallel to the axis; positive scalings only
            // for depth > 0).
            let offset = [
                point[0] - position[0],
                point[1] - position[1],
                point[2] - position[2],
            ];
            let length =
                (offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2]).sqrt();
            let scale = if length == 0.0 {
                0.0
            } else {
                (offset[0] * forward[0] + offset[1] * forward[1] + offset[2] * forward[2]) / length
            };
            assert!(
                (scale - 1.0).abs() < 1e-3,
                "depth {depth}: the center ray runs along the forward axis (cos {scale})"
            );
            if depth > 0.0 {
                assert!(length > 0.0, "depth {depth}: in front of the camera");
            }
        }
    }

    /// The day/night cycle's clock: the triangle wave advances
    /// 0→1 over the first half period and back over the second,
    /// wraps for arbitrarily long (or negative) elapsed times, and
    /// composed with `atmosphere_at` the endpoints are EXACTLY the
    /// presets (the brief's gate — the full wiring's endpoints,
    /// bit-equal through the cycle, not just through direct t).
    #[test]
    fn cycle_position_is_the_triangle_wave_with_preset_endpoints() {
        let period = 20.0f32;
        // The endpoints and halves, hand-computed.
        assert_eq!(cycle_position(0.0, period), 0.0);
        assert_eq!(cycle_position(5.0, period), 0.5);
        assert_eq!(cycle_position(10.0, period), 1.0);
        assert_eq!(cycle_position(15.0, period), 0.5);
        assert_eq!(cycle_position(20.0, period), 0.0);
        // The wraparound: two periods later, the same positions (the
        // 41 s entry carries float phase arithmetic — tolerance).
        for (elapsed, expected) in [(25.0, 0.5), (30.0, 1.0), (41.0, 0.1), (60.0, 0.0)] {
            let position = cycle_position(elapsed, period);
            assert!(
                (position - expected).abs() < 1e-5,
                "{elapsed}s: {position} vs {expected}"
            );
        }
        // Negative elapsed wraps forward (rem_euclid).
        assert_eq!(cycle_position(-10.0, period), 1.0);
        assert_eq!(cycle_position(-15.0, period), 0.5);
        // Monotone up the first half period (elapsed 0→10 s),
        // down the second (10→20 s) — no snap anywhere.
        let mut last = 0.0f32;
        for step in 0..=40 {
            let elapsed = step as f32 * 0.5;
            let t = cycle_position(elapsed, period);
            if step <= 20 {
                assert!(t >= last, "rising at {elapsed}s: {t} after {last}");
            } else {
                assert!(t <= last, "falling at {elapsed}s: {t} after {last}");
            }
            last = t;
        }
        // THE ENDPOINT COMPOSITION (the wiring's gate): the cycle's
        // day and night instants are BIT-EQUAL to the presets.
        assert_eq!(
            atmosphere_at(cycle_position(0.0, period)).light,
            crate::lit_scene::day_preset()
        );
        assert_eq!(atmosphere_at(cycle_position(0.0, period)).sky, day_sky());
        assert_eq!(
            atmosphere_at(cycle_position(10.0, period)).light,
            crate::lit_scene::night_preset()
        );
        assert_eq!(atmosphere_at(cycle_position(10.0, period)).sky, night_sky());
        // And a full period later, dawn again — the same day bit
        // for bit.
        assert_eq!(atmosphere_at(cycle_position(20.0, period)).sky, day_sky());
    }

    fn mul4(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
        std::array::from_fn(|column| {
            std::array::from_fn(|row| {
                let mut sum = 0.0;
                for k in 0..4 {
                    sum += a[k][row] * b[column][k];
                }
                sum
            })
        })
    }

    fn assert_near_identity(m: &[[f32; 4]; 4]) {
        for (column, values) in m.iter().enumerate() {
            for (row, value) in values.iter().enumerate() {
                let expected = f32::from(column == row);
                assert!(
                    (value - expected).abs() < 1e-4,
                    "element ({row},{column}): {value} vs {expected}"
                );
            }
        }
    }
}
