//! The planar-reflection deterministic core (the Era-1 scaffold —
//! see `docs/CYCLE6-ERA1-BRIEF.md`): the ground-plane mirror matrix
//! and the angle-based reflection blend. The wet-asphalt namesake:
//! the ground re-renders the scene mirrored below y = GROUND_Y into
//! an offscreen target, and the ground shader blends it by view
//! angle (grazing views reflect, overhead views stay matte).
//!
//! DETERMINISM CONTRACT: the mirror matrix is pure fixed-order
//! arithmetic; the blend is Schlick's approximation with the
//! five-multiplication pow5 (no `pow`, no libm — the repeated-
//! squaring discipline).

/// The ground-plane reflection matrix (column-major, the repo's
/// convention): maps (x, y, z) to (x, 2·plane_y − y, z) — the
/// mirrored world the reflection pass renders. The mirrored camera
/// = this matrix times the camera; the implementer also flips the
/// winding (the mirror inverts handedness — cull faces swap; the
/// brief documents the Vulkan-side `flipViewportY` alternative and
/// its equivalence).
pub fn ground_reflection_matrix(plane_y: f32) -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, -1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 2.0 * plane_y, 0.0, 1.0],
    ]
}

/// The reflection blend factor by view angle — Schlick's
/// approximation with the water-ish base reflectivity:
///   r(n_dot_v) = r0 + (1 − r0)(1 − n_dot_v)^5
/// with r0 = 0.02 (the fixed literal) and the fifth power by
/// repeated multiplication. `n_dot_v` is the clamped dot of the
/// surface normal and the unit view direction.
pub const REFLECTION_R0: f32 = 0.02;

/// The blend: matte at normal incidence (~r0), near-fully
/// reflective at grazing angles.
pub fn reflection_blend(n_dot_v: f32) -> f32 {
    let n = n_dot_v.clamp(0.0, 1.0);
    let one_minus = 1.0 - n;
    let pow5 = one_minus * one_minus * one_minus * one_minus * one_minus;
    REFLECTION_R0 + (1.0 - REFLECTION_R0) * pow5
}

/// The A19 field-extent bound radius's ULP travel budget (the
/// packer's padding factor): the fragment shader measures a
/// ground point's distance to the bound center through its own
/// rounded chain (two subtractions, two multiplies, one add, the
/// sqrt — each ≤ 1 ulp) and admits band points through the rounded
/// band compare, so a point a ring's band genuinely contains can
/// measure up to ~14 ulps FARTHER from the bound center than the
/// exact triangle-inequality reach
/// (|P − cᵢ| ≤ radiusᵢ + width ⇒ |P − bc| ≤ |cᵢ − bc| + radiusᵢ +
/// width), while this CPU packing's own chain can round the reach
/// DOWN by a few ulps. The pad — a relative 2⁻¹⁹ (16–32 ulps
/// depending on the mantissa) — covers the whole travel budget
/// with margin, at invisible cost (a few ulps of radius on a
/// ground-scale bound). The no-false-negative property test
/// verifies the margin empirically over adversarial layouts; both
/// operands and the sum are exact f32 values.
pub const RIPPLE_BOUND_ULP_TRAVEL: f32 = 1.0 + 1.0 / 524_288.0;

/// THE A19 FIELD-EXTENT BOUND (the ripple uniform's rows[2] — the
/// shader's early-out gate): the conservative bounding circle over
/// a field's LIVE rings. The center is the live centers'
/// bounding-box center, `(min + max) / 2` per axis; the radius is
/// the max over the live rings of `|centerᵢ − bc| + radiusᵢ +
/// width` — `radiusᵢ = speed × ageᵢ`, the SAME f32 product the
/// shader computes per ring — then padded by
/// [`RIPPLE_BOUND_ULP_TRAVEL`]. An empty slice packs the zero
/// bound (count 0 gates it away in the shader; the value is never
/// read). Pure function, no allocation; the renderer's packer, the
/// gate-side oracle row builders, and the property tests all share
/// THIS one implementation (the packer↔shader mirror's single
/// source for the claim).
pub fn ripple_field_bound(
    rings: &[engine_core::ripple::RippleRing],
    speed: f32,
    width: f32,
) -> ([f32; 2], f32) {
    let Some(first) = rings.first() else {
        return ([0.0, 0.0], 0.0);
    };
    let mut min = first.center;
    let mut max = first.center;
    for ring in rings {
        let [cx, cz] = ring.center;
        min[0] = min[0].min(cx);
        max[0] = max[0].max(cx);
        min[1] = min[1].min(cz);
        max[1] = max[1].max(cz);
    }
    let center = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
    let mut reach = 0.0f32;
    for ring in rings {
        // The center offset in the shader's own dist form (the
        // sqrt-of-dot chain — mirrored so the pad's ULP budget is
        // derived against the same arithmetic shape).
        let delta = [ring.center[0] - center[0], ring.center[1] - center[1]];
        let offset = (delta[0] * delta[0] + delta[1] * delta[1]).sqrt();
        let ring_radius = speed * ring.age;
        reach = reach.max(offset + ring_radius + width);
    }
    (center, reach * RIPPLE_BOUND_ULP_TRAVEL)
}

/// THE RIPPLE ORACLE (the CPU mirror of ground_reflect.wgsl's
/// ripple perturbation, term-for-term — the lit_surface_color
/// discipline applied to the ripple field; the technique mined
/// from PR #54's `perturb` with credit). Given the SAME packed
/// uniform rows the shader reads (reflection_draw's
/// RippleUniformData) and a ground point (x, z), returns the
/// (world tangent displacement, roughen) the shader computes at
/// that point — the displacement in WORLD METERS since the
/// finding-19 corrective (the shader projects it into screen UV
/// separately; see [`projected_uv_offset`]). The GPU/CPU sqrt may
/// differ by ULPs — the known-answer gates compare under the
/// reflection gates' documented tolerance class, never
/// bit-exactly.
///
/// THE ROW CONTRACT (the shader's layout, term-for-term — the A19
/// versioned claim included): rows[0] = (live count, expansion
/// speed m/s, band width m, tangent-displacement gain m); rows[1] =
/// (roughen gain, debug mode, ..) — NEVER READ by this function
/// (the roughen gain is applied in the shader's blend stage and
/// the debug bits are test-only term suppressors; this oracle
/// mirrors the SHIPPED mode-0 path); rows[2] = the field-extent
/// bound (center_x, center_z, radius, ..); rows[3 + i] =
/// (center_x, center_z, age s, current amplitude). A `rows`
/// shorter than 3 + count PANICS at the ring read — fail-visible,
/// never a silent wrong answer (the shader's pool bound makes the
/// well-formed case impossible to trip). The arithmetic mirrors
/// the WGSL in its exact order: the count gate, the A19
/// field-extent early-out (outside the bound circle the identity
/// stands directly — value-exact, since every band gate is false
/// out there by the bound's conservativeness; the harness compares
/// this against the loop by feeding an infinite-radius bound row),
/// the pool-order ring loop, `dist = sqrt(dx² + dz²)`, `radius =
/// speed × age`, `band = |dist − radius|` gated `band < width` (the
/// bump's own support), the signed slope `amp / width` (positive
/// inside the ring line, negative outside — the triangle
/// gradient's piecewise-constant stylization), the radial direction
/// with the `dist == 0` zero-contribution select, `world_offset +=
/// dir × (signed × gain)`, `roughen += amp × (1 − band / width)`.
/// Pure function, no allocation, no transcendentals.
pub fn ripple_perturbation(rows: &[[f32; 4]], ground: [f32; 2]) -> ([f32; 2], f32) {
    let mut uv_offset = [0.0f32; 2];
    let mut roughen = 0.0f32;
    // The count gate (the shader's `if (ripple_count > 0)` — the
    // empty field's identity: the loop never runs, the zeros stand).
    let count = rows[0][0] as i32;
    if count > 0 {
        // THE FIELD-EXTENT EARLY-OUT (A19 — the shader's bound gate,
        // mirrored): outside the packed bound circle the identity
        // stands directly. The values are the same the loop would
        // produce for an out-of-band fragment (every band gate false
        // by the bound's conservativeness — the no-false-negative
        // property), so the mirror is value-exact either way; the
        // gate is mirrored for term-for-term fidelity.
        let delta = [ground[0] - rows[2][0], ground[1] - rows[2][1]];
        let bound_dist = (delta[0] * delta[0] + delta[1] * delta[1]).sqrt();
        if bound_dist <= rows[2][2] {
            for i in 0..count as usize {
                let ring = rows[3 + i];
                let delta = [ground[0] - ring[0], ground[1] - ring[1]];
                let dist = (delta[0] * delta[0] + delta[1] * delta[1]).sqrt();
                let radius = rows[0][1] * ring[2];
                let band = (dist - radius).abs();
                let width = rows[0][2];
                if band < width {
                    // The signed slope: +amp/width inside the ring line,
                    // − outside (one subtraction and a sign — fixed
                    // order, the shader's select).
                    let slope = ring[3] / width;
                    let signed = if dist > radius { -slope } else { slope };
                    // The radial direction (dist == 0 has none: zero
                    // contribution — the exact center of a ring).
                    let dir = if dist > 0.0 {
                        [delta[0] / dist, delta[1] / dist]
                    } else {
                        [0.0, 0.0]
                    };
                    let push = dir.map(|component| component * (signed * rows[0][3]));
                    uv_offset = [uv_offset[0] + push[0], uv_offset[1] + push[1]];
                    roughen += ring[3] * (1.0 - band / width);
                }
            }
        }
    }
    (uv_offset, roughen)
}

/// THE PROJECTION STAGE (finding 19's corrective — the CPU mirror
/// of ground_reflect.wgsl's projection block, term-for-term): a
/// WORLD tangent displacement `world_offset` (meters, x/z) at the
/// ground point `ground` (the full 3D point — the shader's
/// `world_pos`; the displacement never touches y) becomes a
/// screen-UV offset by projecting BOTH the point and the displaced
/// point through the frame's view-projection, w-dividing into
/// authored NDC, and differencing with the established Y
/// convention (authored NDC +y is screen-UP, the vertex-boundary
/// epilogue's flip replayed): `u = (ndc_x + 1)/2`,
/// `v = (1 − ndc_y)/2` — so `du = +½·Δndc_x`, `dv = −½·Δndc_y`.
/// The clip evaluation is the repo's column-major k-ascending sum,
/// the same fixed order the WGSL matrix-vector product spells.
///
/// THE ZERO DISPLACEMENT is the exact identity: a zero offset
/// yields bit-equal clips, a (0, −0.0) difference, and
/// `screen_uv + (0, −0.0) == screen_uv` in IEEE arithmetic —
/// value-identical sampling (the shader's header note). Pure
/// function, no allocation, no transcendentals (multiply-add only
/// — the CPU byte-path discipline holds).
pub fn projected_uv_offset(
    view_proj: &[[f32; 4]; 4],
    ground: [f32; 3],
    world_offset: [f32; 2],
) -> [f32; 2] {
    // Column-major clip: clip[row] = Σₖ vp[column k][row] · p[k],
    // k ascending — the WGSL `frame.view_proj * vec4(p, 1.0)`.
    let clip = |p: [f32; 3]| -> [f32; 4] {
        std::array::from_fn(|row| {
            view_proj[0][row] * p[0]
                + view_proj[1][row] * p[1]
                + view_proj[2][row] * p[2]
                + view_proj[3][row]
        })
    };
    let p = clip(ground);
    let q = clip([
        ground[0] + world_offset[0],
        ground[1],
        ground[2] + world_offset[1],
    ]);
    let ndc_p = [p[0] / p[3], p[1] / p[3]];
    let ndc_q = [q[0] / q[3], q[1] / q[3]];
    [0.5 * (ndc_q[0] - ndc_p[0]), -0.5 * (ndc_q[1] - ndc_p[1])]
}

/// The mirrored world's view-projection (package 7, the reflection
/// pass's own matrix): `view_proj × M` in the repo's column-major
/// convention — the reflection pass renders the MIRRORED world
/// (`M·v`) through the UNCHANGED camera, so every point ON the
/// ground plane lands at exactly its true clip position (the
/// mirror fixes the plane) and the ground's screen footprint
/// samples the mirrored scene behind it at the same texel — the
/// planar-reflection property the whole pass rests on. Fixed
/// operation order (the k-sum per element, k ascending).
pub fn mirrored_view_projection(view_proj: &[[f32; 4]; 4], plane_y: f32) -> [[f32; 4]; 4] {
    let mirror = ground_reflection_matrix(plane_y);
    std::array::from_fn(|column| {
        std::array::from_fn(|row| {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += view_proj[k][row] * mirror[column][k];
            }
            sum
        })
    })
}

/// The mirrored camera eye (the reflected viewpoint the reflection
/// pass's lighting math uses).
pub fn mirrored_eye(eye: [f32; 3], plane_y: f32) -> [f32; 3] {
    [eye[0], 2.0 * plane_y - eye[1], eye[2]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lit_scene::GROUND_Y;

    /// The mirror fixes the plane, flips height symmetrically, and
    /// is its own inverse (reflecting twice is the identity).
    #[test]
    fn mirror_fixes_plane_and_flips() {
        let m = ground_reflection_matrix(0.0);
        // Column-major: v' = sum(columns[i] * v[i]).
        let reflect = |v: [f32; 3]| -> [f32; 3] {
            [
                m[0][0] * v[0] + m[1][0] * v[1] + m[2][0] * v[2] + m[3][0],
                m[0][1] * v[0] + m[1][1] * v[1] + m[2][1] * v[2] + m[3][1],
                m[0][2] * v[0] + m[1][2] * v[1] + m[2][2] * v[2] + m[3][2],
            ]
        };
        // A point ON the plane is fixed.
        assert_eq!(reflect([3.0, 0.0, -7.0]), [3.0, 0.0, -7.0]);
        // A point above maps symmetrically below.
        assert_eq!(reflect([3.0, 5.0, -7.0]), [3.0, -5.0, -7.0]);
        // Twice is identity.
        let once = reflect([1.0, 2.5, 3.0]);
        assert_eq!(reflect(once), [1.0, 2.5, 3.0]);
        // The non-zero plane offset: y=GROUND_Y plane.
        let g = ground_reflection_matrix(GROUND_Y);
        assert_eq!(
            [
                g[0][0] * 0.0 + g[1][0] * GROUND_Y + g[2][0] * 0.0 + g[3][0],
                g[0][1] * 0.0 + g[1][1] * GROUND_Y + g[2][1] * 0.0 + g[3][1],
                g[0][2] * 0.0 + g[1][2] * GROUND_Y + g[2][2] * 0.0 + g[3][2],
            ],
            [0.0, GROUND_Y, 0.0],
            "GROUND_Y is the fixed plane"
        );
    }

    /// The blend anchors: exactly r0 at normal incidence, ~1 at
    /// grazing (the f32 sum r0 + (1−r0) lands one ulp below 1.0 —
    /// the tolerance documents it), monotone between, clamped
    /// outside [0,1].
    #[test]
    fn blend_anchors_and_monotone() {
        assert_eq!(reflection_blend(1.0), REFLECTION_R0);
        let grazing = reflection_blend(0.0);
        assert!(grazing > 0.95, "grazing near 1: {grazing}");
        let mut last = f32::INFINITY;
        for step in 0..=20 {
            let n = step as f32 / 20.0;
            let b = reflection_blend(n);
            // The blend DECREASES as n rises (grazing reflects, head-on
            // is matte) — the monotone direction of Schlick's curve.
            assert!(b <= last + 1e-7, "monotone at n={n}");
            assert!((REFLECTION_R0..=1.0 + 1e-6).contains(&b));
            last = b;
        }
        // Clamped inputs do not explode.
        assert_eq!(reflection_blend(-0.5), reflection_blend(0.0));
        assert_eq!(reflection_blend(1.5), REFLECTION_R0);
    }

    /// THE ORACLE KNOWN ANSWERS (hand-computed, the #54 `perturb`
    /// lesson folded in): one ring at the origin, the demo's
    /// authored policy (speed 0.8 m/s, width 0.35 m, amplitude 1.0,
    /// lifetime 2.5 s), aged 60 ticks — the age is the ACCUMULATED
    /// 60 x dt (the accumulation loop replicated, never 60*dt —
    /// f32 accumulation is the pinned form). The radius is
    /// speed x age; the packed row's amplitude is the fade-at-read
    /// expression. The push is the WORLD tangent displacement in
    /// meters (slope x [`crate::renderer::RIPPLE_UV_GAIN`] — the
    /// finding-19 semantics: the gain converts the unitless slope
    /// into meters of ground-plane push; the projection into
    /// screen UV is [`projected_uv_offset`]'s own stage, pinned by
    /// its own tests below). Probes: ON the ring line (the sign
    /// boundary — the gradient is MAXIMUM at the apex, the sign at
    /// the exact peak is ULP-arbitrary, so the FLIP is asserted
    /// either side), inside the band (positive x direction),
    /// outside the peak (negative), OUTSIDE the band (exactly zero
    /// — the gate), and the exact center (the direction select).
    #[test]
    fn ripple_oracle_known_answers() {
        // The accumulated 60-tick age (the replication loop).
        let dt = 1.0f32 / 60.0f32;
        let mut age = 0.0f32;
        for _ in 0..60 {
            age += dt;
        }
        // The packed rows exactly as the renderer's packing builds
        // them: rows[0] = (count, speed, width, tangent-displacement
        // gain), rows[2] = the A19 field-extent bound (the shared
        // ripple_field_bound over the live prefix — one ring here, so
        // the bound is that ring's outer band edge), rows[3] =
        // (center_x, center_z, age, faded amplitude).
        let faded = 1.0f32 * (1.0f32 - age / 2.5f32);
        let ring = engine_core::ripple::RippleRing {
            center: [0.0, 0.0],
            age,
            amplitude: 1.0,
        };
        let mut rows = [[0.0f32; 4]; 3 + engine_core::ripple::RIPPLE_POOL_BOUND];
        rows[0] = [1.0, 0.8, 0.35, crate::renderer::RIPPLE_UV_GAIN];
        rows[1] = [crate::renderer::RIPPLE_ROUGHEN_GAIN, 0.0, 0.0, 0.0];
        let (bound_center, bound_radius) = ripple_field_bound(&[ring], 0.8, 0.35);
        rows[2] = [bound_center[0], bound_center[1], bound_radius, 0.0];
        rows[3] = [0.0, 0.0, age, faded];
        let radius = 0.8f32 * age;
        let width = 0.35f32;
        let slope = faded / width;
        let push = slope * crate::renderer::RIPPLE_UV_GAIN;

        // ON the ring line (the positive x axis, dist == radius):
        // the apex — the magnitude is the full slope x gain and the
        // roughen is the full faded amplitude (band == 0 up to the
        // sqrt's ULPs). The SIGN here is ULP-arbitrary (which side
        // of the apex dist lands on) — the flip is asserted below.
        let (uv, roughen) = ripple_perturbation(&rows, [radius, 0.0]);
        assert!(
            (uv[0].abs() - push).abs() < 1e-5 * push,
            "the apex carries the full gradient: uv {uv:?} vs push {push}"
        );
        assert!(uv[1].abs() < 1e-7, "the push is radial (x axis): {uv:?}");
        assert!(
            (roughen - faded).abs() < 1e-6 * faded,
            "the apex roughen is the faded amplitude: {roughen} vs {faded}"
        );

        // INSIDE the band (dist < radius, positive x direction):
        // the signed slope is POSITIVE — the push points away from
        // the center (+x).
        let (uv, roughen) = ripple_perturbation(&rows, [radius - width * 0.5, 0.0]);
        assert!(uv[0] > 0.0, "inside the ring line the push is +x: {uv:?}");
        assert!(
            (uv[0].abs() - push).abs() < 1e-5 * push,
            "the triangle gradient is piecewise-constant: {uv:?} vs push {push}"
        );
        assert!(uv[1].abs() < 1e-7, "radial on the x axis: {uv:?}");
        assert!(
            roughen > 0.0 && roughen < faded,
            "inside the band the coverage is a positive fraction: {roughen}"
        );

        // OUTSIDE the peak (dist > radius, still inside the band):
        // the signed slope flips NEGATIVE — the push points back
        // toward the center (-x). THE #54 LESSON: the gradient is
        // MAXIMUM at the apex and its sign at the exact peak is
        // ULP-arbitrary — the flip is proven EITHER SIDE, never at
        // the boundary itself.
        let (uv, _) = ripple_perturbation(&rows, [radius + width * 0.5, 0.0]);
        assert!(uv[0] < 0.0, "outside the ring line the push is -x: {uv:?}");
        assert!(
            (uv[0].abs() - push).abs() < 1e-5 * push,
            "the same magnitude, the opposite sign: {uv:?} vs push {push}"
        );

        // OUTSIDE the band: the gate `band < width` fails — the
        // contribution is EXACTLY zero. This probe sits at radius +
        // width + 0.1, OUTSIDE the single ring's honest bound too,
        // so the A19 early-out returns the identity directly — the
        // loop's own gate zero is proven by the two-ring gap probe
        // below (a point inside the bound but outside every band).
        let (uv, roughen) = ripple_perturbation(&rows, [radius + width + 0.1, 0.0]);
        assert_eq!(uv, [0.0, 0.0], "outside the band: the exact gate zero");
        assert_eq!(roughen, 0.0, "outside the band: no coverage");

        // THE BAND GATE'S OWN ZERO, INSIDE THE BOUND (the gap between
        // two rings): two mature rings 10 m apart, the probe at the
        // midpoint — inside the two-ring bound (the bound covers both
        // rings' reaches) but outside BOTH bands (5 − radius > width),
        // so the loop RUNS and every gate fails: the identity is the
        // LOOP's product here, not the early-out's.
        let gap_rows = {
            let mut rows = [[0.0f32; 4]; 3 + engine_core::ripple::RIPPLE_POOL_BOUND];
            rows[0] = [2.0, 0.8, 0.35, crate::renderer::RIPPLE_UV_GAIN];
            rows[1] = [crate::renderer::RIPPLE_ROUGHEN_GAIN, 0.0, 0.0, 0.0];
            let rings = [
                engine_core::ripple::RippleRing {
                    center: [-5.0, 0.0],
                    age,
                    amplitude: 1.0,
                },
                engine_core::ripple::RippleRing {
                    center: [5.0, 0.0],
                    age,
                    amplitude: 1.0,
                },
            ];
            let (center, bound) = ripple_field_bound(&rings, 0.8, 0.35);
            rows[2] = [center[0], center[1], bound, 0.0];
            for (i, ring) in rings.iter().enumerate() {
                rows[3 + i] = [
                    ring.center[0],
                    ring.center[1],
                    ring.age,
                    1.0f32 * (1.0f32 - ring.age / 2.5f32),
                ];
            }
            rows
        };
        assert!(
            radius + width < 5.0,
            "the fixture's bands leave a gap at the midpoint (radius {radius} + width \
             {width} vs the 5 m half-separation)"
        );
        let (uv, roughen) = ripple_perturbation(&gap_rows, [0.0, 0.0]);
        assert_eq!(
            uv,
            [0.0, 0.0],
            "inside the bound, outside both bands: the loop's own gate zero"
        );
        assert_eq!(roughen, 0.0, "the gap carries no coverage");

        // THE EXACT CENTER (dist == 0): the mature ring's radius
        // (≈ 0.79 m) exceeds the width, so the gate itself zeroes
        // this point.
        let (uv, roughen) = ripple_perturbation(&rows, [0.0, 0.0]);
        assert_eq!(
            uv,
            [0.0, 0.0],
            "the center: the gate's zero (radius > width)"
        );
        assert_eq!(roughen, 0.0);

        // THE DIRECTION SELECT's own proof (the gate OPEN at the
        // center): a YOUNG ring — 2 accumulated ticks, radius ≈
        // 0.027 m < width — whose band covers the center. The push
        // would be maximal there (the inside slope); the
        // `dist == 0` select zeroes the direction, so the uv
        // contribution is EXACTLY [0, 0] while the roughen
        // (direction-free) stays positive.
        let mut young_age = 0.0f32;
        for _ in 0..2 {
            young_age += dt;
        }
        let young_faded = 1.0f32 * (1.0f32 - young_age / 2.5f32);
        let young_ring = engine_core::ripple::RippleRing {
            center: [0.0, 0.0],
            age: young_age,
            amplitude: 1.0,
        };
        let mut young_rows = [[0.0f32; 4]; 3 + engine_core::ripple::RIPPLE_POOL_BOUND];
        young_rows[0] = rows[0];
        let (young_center, young_bound) = ripple_field_bound(&[young_ring], 0.8, 0.35);
        young_rows[2] = [young_center[0], young_center[1], young_bound, 0.0];
        young_rows[3] = [0.0, 0.0, young_age, young_faded];
        let young_radius = 0.8f32 * young_age;
        assert!(
            young_radius < width,
            "the young ring's band covers its own center"
        );
        let (uv, roughen) = ripple_perturbation(&young_rows, [0.0, 0.0]);
        assert_eq!(
            uv,
            [0.0, 0.0],
            "dist == 0 has no radial direction: the select's exact zero"
        );
        let expected_roughen = young_faded * (1.0f32 - young_radius / width);
        assert!(
            roughen > 0.0 && (roughen - expected_roughen).abs() < 1e-6 * expected_roughen,
            "the coverage is direction-free and positive at the center: {roughen} vs \
             {expected_roughen}"
        );
    }

    /// THE IDENTITY GATE's CPU HALF: the empty field (count 0) is a
    /// BIT-IDENTICAL no-op — the oracle returns exactly
    /// ([0.0, 0.0], 0.0) for every ground point, both for the
    /// all-zero rows (the never-set uniform — the A19 bound row
    /// included, zeros the count gate never reads) and the
    /// realistic rainless rows (a live policy's params with count 0
    /// — every ring expired; the loop never runs, the params are
    /// never read).
    #[test]
    fn empty_field_is_the_bit_identity() {
        let zero_rows = [[0.0f32; 4]; 3 + engine_core::ripple::RIPPLE_POOL_BOUND];
        let mut param_rows = [[0.0f32; 4]; 3 + engine_core::ripple::RIPPLE_POOL_BOUND];
        param_rows[0] = [0.0, 0.8, 0.35, crate::renderer::RIPPLE_UV_GAIN];
        param_rows[1] = [crate::renderer::RIPPLE_ROUGHEN_GAIN, 0.0, 0.0, 0.0];
        for ground in [
            [0.0, 0.0],
            [12.5, -7.25],
            [-40.0, 33.0],
            [0.001, 999.0],
            [1234.5, -1234.5],
        ] {
            assert_eq!(
                ripple_perturbation(&zero_rows, ground),
                ([0.0, 0.0], 0.0),
                "the all-zero rows are the exact identity at {ground:?}"
            );
            assert_eq!(
                ripple_perturbation(&param_rows, ground),
                ([0.0, 0.0], 0.0),
                "count 0 skips the loop regardless of the params at {ground:?}"
            );
        }
    }

    /// THE A19 BATTERY's shared row packer (the test-side twin of
    /// renderer.rs's pack_ripple_field for the oracle's input — the
    /// same rows the shader reads: the params row, the shared
    /// [`ripple_field_bound`] row, the ring rows at rows[3 + i];
    /// the fade uses the test policy's 2.5 s lifetime, the
    /// known-answer fixture's own convention).
    /// `verbatim_loop` inflates the bound radius to INFINITY — the
    /// early-out never fires, the loop runs ungated: the reference
    /// arm the equivalence property compares against (a TEST-ONLY
    /// row form no production pack ever carries).
    fn a19_rows(
        rings: &[engine_core::ripple::RippleRing],
        speed: f32,
        width: f32,
        verbatim_loop: bool,
    ) -> [[f32; 4]; 3 + engine_core::ripple::RIPPLE_POOL_BOUND] {
        let mut rows = [[0.0f32; 4]; 3 + engine_core::ripple::RIPPLE_POOL_BOUND];
        rows[0] = [
            rings.len() as f32,
            speed,
            width,
            crate::renderer::RIPPLE_UV_GAIN,
        ];
        let (center, radius) = ripple_field_bound(rings, speed, width);
        rows[2] = [
            center[0],
            center[1],
            if verbatim_loop { f32::INFINITY } else { radius },
            0.0,
        ];
        for (i, ring) in rings.iter().enumerate() {
            rows[3 + i] = [
                ring.center[0],
                ring.center[1],
                ring.age,
                ring.amplitude * (1.0 - ring.age / 2.5),
            ];
        }
        rows
    }

    /// A deterministic LCG (the sim-side seeded-sweep discipline —
    /// no rand dependency, bit-for-bit reproducible): returns
    /// uniform units in [0, 1).
    struct SeededSweep(u64);

    impl SeededSweep {
        fn unit(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) as f32 / (1u64 << 31) as f32
        }
        /// A symmetric sample in [-scale, scale].
        fn centered(&mut self, scale: f32) -> f32 {
            (self.unit() * 2.0 - 1.0) * scale
        }
    }

    /// The A19 battery's adversarial field fixtures: layouts aimed
    /// at the bound's exact corners — the newborn disk (radius 0),
    /// the near-lifetime ring, the two-ring bbox diagonal (max
    /// center offset), the four-corner square (every ring at the
    /// bbox center's diagonal reach), coincident centers, the
    /// axis-aligned pair (the triangle-inequality equality case —
    /// the probe direction collinear with the center offset), the
    /// huge-coordinate field (ULP travel at 10⁴ scale), and seeded
    /// random fields up to the pool bound.
    fn a19_fields() -> Vec<Vec<engine_core::ripple::RippleRing>> {
        let ring = |center: [f32; 2], age: f32| engine_core::ripple::RippleRing {
            center,
            age,
            amplitude: 1.0,
        };
        let mut fields = vec![
            vec![ring([0.0, 0.0], 0.0)],
            vec![ring([3.25, -7.5], 2.4)],
            vec![ring([-8.0, -6.0], 1.0), ring([8.0, 6.0], 0.4)],
            vec![
                ring([-6.0, -6.0], 0.8),
                ring([6.0, -6.0], 1.4),
                ring([-6.0, 6.0], 0.2),
                ring([6.0, 6.0], 1.9),
            ],
            vec![ring([2.5, 9.75], 0.0); engine_core::ripple::RIPPLE_POOL_BOUND],
            vec![ring([-10.0, 0.0], 1.0), ring([10.0, 0.0], 0.25)],
            vec![
                ring([9_999.0, 9_998.0], 0.5),
                ring([10_001.0, 10_002.0], 1.5),
                ring([10_000.0, 9_999.0], 0.9),
            ],
        ];
        // The seeded random fields: three shapes (one ring, a
        // handful, the full pool bound) over the demo policy's
        // ground scale.
        for count in [1usize, 5, engine_core::ripple::RIPPLE_POOL_BOUND] {
            let mut sweep = SeededSweep(0x2545_F491_4F6C_DD1D);
            let field: Vec<_> = (0..count)
                .map(|_| {
                    ring(
                        [sweep.centered(20.0), sweep.centered(20.0)],
                        sweep.unit() * 2.4,
                    )
                })
                .collect();
            fields.push(field);
        }
        fields
    }

    /// The A19 battery's probe set for one field: per ring, the
    /// adversarial distances (the exact center, the apex, the inner
    /// and outer band edges — the outer probed just inside the
    /// gate, `radius + width · (1 − 2⁻¹²)`, the farthest point the
    /// band admits) along the eight compass directions AND the two
    /// directions collinear with the bound center (the triangle-
    /// inequality equality axes), plus a seeded random sweep over
    /// the wider ground.
    fn a19_probes(
        rings: &[engine_core::ripple::RippleRing],
        speed: f32,
        width: f32,
        bound_center: [f32; 2],
    ) -> Vec<[f32; 2]> {
        let mut probes = Vec::new();
        let mut push_at = |center: [f32; 2], dir: [f32; 2], dist: f32| {
            probes.push([center[0] + dir[0] * dist, center[1] + dir[1] * dist]);
        };
        let compass = [
            [1.0, 0.0],
            [-1.0, 0.0],
            [0.0, 1.0],
            [0.0, -1.0],
            [1.0, 1.0],
            [1.0, -1.0],
            [-1.0, 1.0],
            [-1.0, -1.0],
        ];
        for ring in rings {
            let radius = speed * ring.age;
            let mut dirs = compass.to_vec();
            // The collinear-with-bc axes: toward and away from the
            // bound center (the equality case's directions).
            let to_bc = [
                bound_center[0] - ring.center[0],
                bound_center[1] - ring.center[1],
            ];
            let bc_len = (to_bc[0] * to_bc[0] + to_bc[1] * to_bc[1]).sqrt();
            if bc_len > 0.0 {
                dirs.push([to_bc[0] / bc_len, to_bc[1] / bc_len]);
                dirs.push([-to_bc[0] / bc_len, -to_bc[1] / bc_len]);
            }
            for mut dir in dirs {
                // Normalize (the diagonals arrive unnormalized; the
                // axis and collinear directions are unit already).
                let len = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
                dir = [dir[0] / len, dir[1] / len];
                push_at(ring.center, dir, 0.0);
                push_at(ring.center, dir, radius);
                push_at(ring.center, dir, (radius - width).max(0.0) * 0.999);
                push_at(ring.center, dir, (radius + width) * (1.0 - 1.0 / 4096.0));
                push_at(ring.center, dir, (radius + width) * 0.5);
            }
        }
        let mut sweep = SeededSweep(0x0DDB_1A5E_5BAD_1DEA);
        for _ in 0..64 {
            probes.push([sweep.centered(30.0), sweep.centered(30.0)]);
        }
        probes
    }

    /// THE A19 NO-FALSE-NEGATIVE PROPERTY (the design §5.5's gate
    /// boundary — the charter's "no false-negative culling" bar):
    /// for randomized AND adversarial fields, the packed bound
    /// NEVER excludes a fragment any ring's band contains — every
    /// ground point where the verbatim loop produces a non-identity
    /// contribution must sit inside the bound circle, measured
    /// through the shader's own dist form (the sqrt-of-dot chain
    /// the early-out runs). A failure here is the one hazard the
    /// localization could introduce: an early-out that changes a
    /// banded fragment's value.
    #[test]
    fn the_field_extent_bound_never_culls_a_banded_fragment() {
        let policies = [(0.8f32, 0.35f32), (3.0, 0.05), (0.1, 1.5)];
        for (speed, width) in policies {
            for rings in a19_fields() {
                let (bound_center, bound_radius) = ripple_field_bound(&rings, speed, width);
                let rows = a19_rows(&rings, speed, width, true);
                for probe in a19_probes(&rings, speed, width, bound_center) {
                    let (uv, roughen) = ripple_perturbation(&rows, probe);
                    if (uv, roughen) == ([0.0, 0.0], 0.0) {
                        continue; // an identity output early-outs harmlessly
                    }
                    let delta = [probe[0] - bound_center[0], probe[1] - bound_center[1]];
                    let dist = (delta[0] * delta[0] + delta[1] * delta[1]).sqrt();
                    assert!(
                        dist <= bound_radius,
                        "a banded fragment at {probe:?} (field of {} rings, policy \
                         {speed}/{width}) sits OUTSIDE the packed bound: dist {dist} > \
                         radius {bound_radius} — a false-negative cull (the ULP travel \
                         budget is too tight)",
                        rings.len()
                    );
                }
            }
        }
    }

    /// THE A19 EQUIVALENCE BATTERY (the design §5.5's first bullet,
    /// the CPU half of the ink gates' digit-for-digit bar): for
    /// randomized and adversarial fields, fragments inside AND
    /// outside the bound produce IDENTICAL fragment outputs — the
    /// oracle with the honest packed bound equals the oracle with
    /// the verbatim-loop (infinite) bound row at every probe,
    /// bit-for-bit. This is the identity contract made executable:
    /// the early-out is value-transparent everywhere it fires.
    #[test]
    fn the_early_out_is_value_transparent_against_the_verbatim_loop() {
        let policies = [(0.8f32, 0.35f32), (3.0, 0.05), (0.1, 1.5)];
        for (speed, width) in policies {
            for rings in a19_fields() {
                let (bound_center, _) = ripple_field_bound(&rings, speed, width);
                let honest = a19_rows(&rings, speed, width, false);
                let verbatim = a19_rows(&rings, speed, width, true);
                for probe in a19_probes(&rings, speed, width, bound_center) {
                    assert_eq!(
                        ripple_perturbation(&honest, probe),
                        ripple_perturbation(&verbatim, probe),
                        "the early-out changed a fragment's value at {probe:?} (field \
                         of {} rings, policy {speed}/{width})",
                        rings.len()
                    );
                }
                // Far-outside probes (the early-out's home ground):
                // points far beyond any bound still agree, and the
                // honest arm returns the exact identity there.
                for probe in [
                    [100.0, 100.0],
                    [-100.0, 60.0],
                    [0.0, -250.0],
                    [9_000.0, 9_000.0],
                ] {
                    assert_eq!(
                        ripple_perturbation(&honest, probe),
                        ripple_perturbation(&verbatim, probe),
                        "a far-outside probe at {probe:?} still matches the verbatim \
                         loop"
                    );
                    assert_eq!(
                        ripple_perturbation(&honest, probe),
                        ([0.0, 0.0], 0.0),
                        "outside every band and bound: the exact identity at {probe:?}"
                    );
                }
            }
        }
    }

    /// THE A19 VERSIONED-ROW CLAIM's cross-side agreement (the
    /// design §5.5's "one layout test"): the WGSL struct and the
    /// Rust-side uniform row count agree — params0, params1, the
    /// bound row, then the rings array at engine-core's pool bound,
    /// with [`crate::vulkan::reflection_draw::RIPPLE_UNIFORM_ROWS`]
    /// == 3 + bound. The shader source is read from the crate's
    /// shaders directory (the shader_composition tests' own
    /// pattern); a regression — a struct edit without the packer's
    /// versioned claim, or the reverse — fails here, not on the
    /// GPU.
    #[test]
    fn the_ripple_uniform_layout_agrees_with_the_shader_struct() {
        let source = std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("shaders/ground_reflect.wgsl"),
        )
        .expect("the shader source reads from the crate's shaders directory");
        let start = source
            .find("struct RippleField {")
            .expect("ground_reflect.wgsl declares its ripple uniform struct (binding 4's block)");
        let body = &source[start
            ..source[start..]
                .find("};")
                .map(|end| start + end + 2)
                .expect("the struct closes")];
        let order = |member: &str| {
            body.find(member)
                .unwrap_or_else(|| panic!("the struct declares {member}"))
        };
        let params0 = order("params0");
        let params1 = order("params1");
        let bound = order("bound");
        let rings = order("rings");
        assert!(
            params0 < params1 && params1 < bound && bound < rings,
            "the struct's member order is params0, params1, bound, rings (the A19 \
             versioned claim sits between the params and the ring array)"
        );
        assert!(
            body.contains(&format!(
                "array<vec4<f32>, {}>",
                engine_core::ripple::RIPPLE_POOL_BOUND
            )),
            "the rings array carries engine-core's pool bound"
        );
        assert_eq!(
            crate::vulkan::reflection_draw::RIPPLE_UNIFORM_ROWS,
            3 + engine_core::ripple::RIPPLE_POOL_BOUND,
            "the Rust-side row count: two params rows + the bound row + the rings"
        );
    }

    /// THE PROJECTION TESTS' OWN CAMERA KIT (the finding-19 deciding
    /// tests' shared builders): the repo's y-up `perspective` and a
    /// rigid `camera_view` at an authored pose — the SAME
    /// `engine_slice::math` forms the demo and the gates use, so the
    /// convention under test (authored NDC +y up, the shader-boundary
    /// flip) is the shipped one, never a re-derivation.
    #[cfg(test)]
    mod projection_kit {
        /// The repo's y-up perspective (engine_slice::math's own
        /// form, replicated for the device-free unit test).
        pub fn perspective(fov_y_radians: f32, aspect: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
            let focal = 1.0 / (fov_y_radians / 2.0).tan();
            let mut matrix = [[0.0f32; 4]; 4];
            matrix[0][0] = focal / aspect;
            matrix[1][1] = focal;
            matrix[2][2] = far / (near - far);
            matrix[2][3] = -1.0;
            matrix[3][2] = near * far / (near - far);
            matrix
        }

        /// A rigid view at an authored position/yaw/pitch (the math
        /// module's Ry·Rx in column-major storage, then the
        /// Rᵀ·T(−position) view form — replicated for the
        /// device-free unit test).
        pub fn camera_view(position: [f32; 3], yaw: f32, pitch: f32) -> [[f32; 4]; 4] {
            let (cy, sy) = (yaw.cos(), yaw.sin());
            let (cp, sp) = (pitch.cos(), pitch.sin());
            // R = Ry(yaw)·Rx(pitch), column-major [column][row].
            let rotation = [
                [cy, 0.0, -sy, 0.0],
                [sy * sp, cp, cy * sp, 0.0],
                [sy * cp, -sp, cy * cp, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ];
            let mut view = [[0.0f32; 4]; 4];
            for column in 0..3 {
                for row in 0..3 {
                    view[column][row] = rotation[row][column];
                }
            }
            for row in 0..3 {
                view[3][row] = -(rotation[row][0] * position[0]
                    + rotation[row][1] * position[1]
                    + rotation[row][2] * position[2]);
            }
            view[3][3] = 1.0;
            view
        }

        pub fn mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
            let mut out = [[0.0f32; 4]; 4];
            for (column, out_column) in out.iter_mut().enumerate() {
                for row in 0..4 {
                    out_column[row] = a[0][row] * b[column][0]
                        + a[1][row] * b[column][1]
                        + a[2][row] * b[column][2]
                        + a[3][row] * b[column][3];
                }
            }
            out
        }

        /// The pixel-space displacement a uv offset spans at an
        /// extent (the probe metric: uv × (width, height)).
        pub fn pixels(uv: [f32; 2], extent: (f32, f32)) -> (f32, f32) {
            (uv[0] * extent.0, uv[1] * extent.1)
        }
    }

    /// FINDING 19's DECIDING TEST — THE OPPOSING YAWS: the SAME
    /// world tangent push at the SAME ground point projects to
    /// OPPOSITE screen directions under two cameras that look at
    /// the point from opposite sides (the raw world-add defect
    /// moved the sample the SAME screen direction at both — +X
    /// always pushed u positively; the projection turns with the
    /// camera). Two cameras at mirrored positions, equal
    /// distances, level: identical camera-space coordinates for
    /// the point, x axes negated — the u offsets are exact
    /// negatives and the v offsets match exactly.
    #[test]
    fn projected_offset_flips_at_opposing_yaws() {
        use projection_kit::{camera_view, mul, perspective, pixels};
        let ground = [3.0, crate::lit_scene::GROUND_Y, -4.0];
        let push = [0.0857, 0.0]; // the unit band's world push, +x
        let proj = perspective(65_f32.to_radians(), 16.0 / 9.0, 0.1, 150.0);
        // Camera A: north of the point looking +Z back at it (yaw π),
        // from z = ground−10; camera B: south of the point looking
        // −Z (yaw 0), from z = ground+10 — the two opposing yaws,
        // both 10 m back and 4 m up, level.
        let view_a = camera_view([3.0, 4.0, ground[2] - 10.0], std::f32::consts::PI, 0.0);
        let view_b = camera_view([3.0, 4.0, ground[2] + 10.0], 0.0, 0.0);
        let vp_a = mul(&proj, &view_a);
        let vp_b = mul(&proj, &view_b);
        let off_a = projected_uv_offset(&vp_a, ground, push);
        let off_b = projected_uv_offset(&vp_b, ground, push);
        // The u signs oppose: +x world is screen-right at yaw 0 and
        // screen-left at yaw π (the 180° turn flips the projection).
        assert!(
            off_a[0] * off_b[0] < 0.0,
            "the opposing yaws flip the push's screen u direction: {off_a:?} vs {off_b:?}"
        );
        assert!(
            off_b[0] > 0.0,
            "at yaw 0 (looking −Z) +x world is screen-right: {off_b:?}"
        );
        // Mirror-symmetric geometry: exact-opposite u offsets,
        // equal v offsets (the cameras' camera-space coordinates
        // for the point are identical, x negated).
        assert!(
            (off_a[0] + off_b[0]).abs() < 1e-7,
            "the mirror-symmetric cameras give exact-opposite u offsets: {off_a:?} vs {off_b:?}"
        );
        assert!(
            (off_a[1] - off_b[1]).abs() < 1e-7,
            "the v offsets agree (both look level): {off_a:?} vs {off_b:?}"
        );
        // THE RAW WORLD-ADD NEGATIVE CONTROL (documented): the old
        // defect added the world components directly to (u, v) —
        // +x pushed u positively at BOTH yaws. The opposite-sign
        // assert above is exactly the failure the defect would
        // produce; this CPU pin is the sign-inversion proof the GPU
        // gate cannot stage without a second camera.
        // The magnitude sanity: a handful of pixels at 1280 for the
        // unit push at ~10.8 m (the gain's documented vantage class).
        let (px, _) = pixels(off_b, (1280.0, 720.0));
        assert!(
            (2.0..=10.0).contains(&px),
            "the unit push spans a handful of pixels at ~11 m: {px}"
        );
    }

    /// FINDING 19's DECIDING TEST — THE TWO PITCHES: the projected
    /// v-offset of a world push along the ground's z axis depends
    /// on the pitch the camera holds over the ground — two cameras
    /// aimed at the SAME ground point (it sits at screen center for
    /// both), one shallow over the ground (high, far), one steep
    /// (low, near): the SAME world push projects to a v offset more
    /// than 3x larger under the steep pitch. The raw world-add
    /// converted a z-meter to the SAME v delta under every camera
    /// — pitch never entered; here the foreshortening carries it.
    #[test]
    fn projected_offset_feels_the_pitch() {
        use projection_kit::{camera_view, mul, perspective};
        let ground = [0.0, crate::lit_scene::GROUND_Y, 0.0];
        let push = [0.0, -0.0857]; // the world push along −z (receding:
        // the cameras sit at +z looking −Z, so away from the eye)
        let proj = perspective(65_f32.to_radians(), 16.0 / 9.0, 0.1, 150.0);
        // The shallow camera: 6 m up, 14 m back (pitch ≈ −0.405),
        // looking −Z at the point from +z (the gate camera's own
        // yaw-0 side).
        let shallow_pitch = -(6.0f32).atan2(14.0);
        let shallow = mul(&proj, &camera_view([0.0, 6.0, 14.0], 0.0, shallow_pitch));
        // The steep camera: 1.2 m up, 2.2 m back (pitch ≈ −0.499) —
        // the owner's street-level vantage class.
        let steep_pitch = -(1.2f32).atan2(2.2);
        let steep = mul(&proj, &camera_view([0.0, 1.2, 2.2], 0.0, steep_pitch));
        let off_shallow = projected_uv_offset(&shallow, ground, push);
        let off_steep = projected_uv_offset(&steep, ground, push);
        // Both cameras aim at the point: the receding push moves the
        // sample up-screen (toward the horizon) under both — the
        // SIGN is shared, the MAGNITUDE is the pitch's.
        assert!(
            off_shallow[1] < 0.0 && off_steep[1] < 0.0,
            "the receding push moves the sample up-screen under both pitches: \
             {off_shallow:?} vs {off_steep:?}"
        );
        assert!(
            off_steep[1].abs() > off_shallow[1].abs() * 3.0,
            "the steep pitch foreshortens the ground — the same world push \
             projects >3x the v offset: {off_steep:?} vs {off_shallow:?}"
        );
        // The u components stay ~zero at screen center (a pure-depth
        // push has no lateral component at yaw 0).
        assert!(
            off_shallow[0].abs() < 1e-4 && off_steep[0].abs() < 1e-4,
            "the +z push is lateral-free at yaw 0: {off_shallow:?} / {off_steep:?}"
        );
    }

    /// FINDING 19's DECIDING TEST — THE NON-SQUARE EXTENT: the same
    /// world push at the same camera under a 16:9 and a square
    /// projection: the NORMALIZED u offsets differ by exactly the
    /// aspect ratio (ndc_x carries focal/aspect), while the PIXEL
    /// displacement at the matching extents (1280×720 vs 720×720)
    //  is EQUAL — the square-pixel property (the gate's probe
    /// measures pixels; the projection must not double-count the
    /// aspect).
    #[test]
    fn projected_offset_tracks_the_aspect() {
        use projection_kit::{camera_view, mul, perspective, pixels};
        let ground = [2.0, crate::lit_scene::GROUND_Y, -6.0];
        let push = [0.0857, 0.0];
        let view = camera_view([0.0, 4.0, -14.0], 0.0, -(4.0f32).atan2(14.0));
        let wide = mul(
            &perspective(65_f32.to_radians(), 16.0 / 9.0, 0.1, 150.0),
            &view,
        );
        let square = mul(&perspective(65_f32.to_radians(), 1.0, 0.1, 150.0), &view);
        let off_wide = projected_uv_offset(&wide, ground, push);
        let off_square = projected_uv_offset(&square, ground, push);
        // The normalized u offsets scale by the aspect ratio.
        assert!(
            ((off_square[0] / off_wide[0]) - (16.0 / 9.0)).abs() < 1e-3,
            "the square projection's u offset is 16/9 x the wide one: {} vs {}",
            off_square[0],
            off_wide[0]
        );
        // The v offsets agree (the vertical focal is aspect-free).
        assert!(
            (off_wide[1] - off_square[1]).abs() < 1e-6,
            "the v offset is aspect-free: {off_wide:?} vs {off_square:?}"
        );
        // The PIXEL displacements at the matching extents are equal
        // (square pixels: 1280 x (9/16) == 720).
        let px_wide = pixels(off_wide, (1280.0, 720.0));
        let px_square = pixels(off_square, (720.0, 720.0));
        assert!(
            (px_wide.0 - px_square.0).abs() < 1e-3 && (px_wide.1 - px_square.1).abs() < 1e-3,
            "the pixel displacement is extent-consistent: {px_wide:?} vs {px_square:?}"
        );
    }

    /// FINDING 19's DECIDING TEST — THE KNOWN ASYMMETRIC EDGE: the
    /// synthetic mirror feature (a bright vertical step in the
    /// reflected content, brighter to the right) sampled through
    /// the FULL oracle chain (perturbation → projection) at probe
    /// pairs on opposite sides of a ring center: the sampled content
    /// flips in OPPOSITE directions on opposite sides of the center
    /// (the pinch), the exact center samples its own value
    /// unchanged (the dist == 0 select composed through the
    /// projection), and a sign-inverted displacement (the negative
    /// control) sweeps both probes AWAY from the edge — the
    /// inversion the GPU probe's oracle-sign check catches.
    #[test]
    fn projected_offset_sweeps_the_asymmetric_edge() {
        use projection_kit::{camera_view, mul, perspective};
        // The gate camera's own pose (the demo vantage): eye on the
        // north road looking at the intersection.
        let eye = [0.0, 1.7, 15.0];
        let proj = perspective(65_f32.to_radians(), 16.0 / 9.0, 0.1, 150.0);
        let pitch = -(1.7f32).atan2(15.0);
        let vp = mul(&proj, &camera_view(eye, 0.0, pitch));
        // The ground u coordinate of a world xz point (the
        // projection's own first stage, shared by the edge and the
        // probes).
        let ground_u = |xz: [f32; 2]| -> f32 {
            let p = [xz[0], crate::lit_scene::GROUND_Y, xz[1]];
            let clip = [
                vp[0][0] * p[0] + vp[1][0] * p[1] + vp[2][0] * p[2] + vp[3][0],
                vp[0][1] * p[0] + vp[1][1] * p[1] + vp[2][1] * p[2] + vp[3][1],
                vp[0][3] * p[0] + vp[1][3] * p[1] + vp[2][3] * p[2] + vp[3][3],
            ];
            let ndc_x = clip[0] / clip[2];
            (ndc_x + 1.0) / 2.0
        };
        // The ring: the newborn form the natural sequence births
        // (radius 0.0133 m — sub-pixel; every probe sits OUTSIDE the
        // ring line, so the push points AT the center from both
        // sides). The center sits on the road cross, ~8.4 m out.
        let dt = 1.0f32 / 60.0f32;
        let ring = engine_core::ripple::RippleRing {
            center: [1.0, 7.5],
            age: dt,
            amplitude: 1.0,
        };
        let mut rows = [[0.0f32; 4]; 3 + engine_core::ripple::RIPPLE_POOL_BOUND];
        rows[0] = [1.0, 0.8, 0.35, crate::renderer::RIPPLE_UV_GAIN];
        let center = [1.0, 7.5];
        // The A19 bound row (the honest single-ring bound: the ring's
        // own outer band edge around its center — the probes below sit
        // within 0.05 m of the center, well inside).
        let (bound_center, bound_radius) = ripple_field_bound(&[ring], 0.8, 0.35);
        rows[2] = [bound_center[0], bound_center[1], bound_radius, 0.0];
        rows[3] = [center[0], center[1], dt, 1.0];
        // The asymmetric mirror feature: a vertical edge crossing
        // the ring center's column in the reflected content — dark
        // (0.2) left, bright (0.8) right.
        let edge_u = ground_u(center);
        let content = |u: f32| if u >= edge_u { 0.8f32 } else { 0.2f32 };
        // The probes: ground points 0.05 m either side of the center
        // along world x (the gate camera's yaw is 0, so world x is
        // the screen-horizontal axis). Both are inside the band
        // (0.05 < radius + width) and outside the ring line
        // (0.05 > 0.0133): the pushes point AT the center.
        let probe = |side: f32| -> [f32; 2] {
            let ground = [center[0] + side * 0.05, center[1]];
            let (world, _) = ripple_perturbation(&rows, ground);
            projected_uv_offset(
                &vp,
                [ground[0], crate::lit_scene::GROUND_Y, ground[1]],
                world,
            )
        };
        let uv_left = probe(-1.0);
        let uv_right = probe(1.0);
        let u_left = ground_u([center[0] - 0.05, center[1]]);
        let u_right = ground_u([center[0] + 0.05, center[1]]);
        // The oracle's independent projection: the left probe's
        // sample sweeps RIGHT (toward the center — across the edge
        // into the bright side), the right probe's sweeps LEFT, each
        // by more than a screen pixel at 1280.
        assert!(
            uv_left[0] > 1.5 / 1280.0,
            "the left probe sweeps right by more than a pixel: {uv_left:?}"
        );
        assert!(
            uv_right[0] < -1.5 / 1280.0,
            "the right probe sweeps left by more than a pixel: {uv_right:?}"
        );
        // The sampled content flips in OPPOSITE directions: the left
        // probe (dark side) samples the bright side; the right probe
        // (bright side) samples the dark side.
        assert!(
            content(u_left + uv_left[0]) > content(u_left),
            "the left probe's sample crosses into the bright side"
        );
        assert!(
            content(u_right + uv_right[0]) < content(u_right),
            "the right probe's sample crosses into the dark side"
        );
        // THE EXACT CENTER (dist == 0): the select's zero composed
        // through the projection — the center samples its own value.
        let (world, _) = ripple_perturbation(&rows, center);
        assert_eq!(world, [0.0, 0.0], "the exact center has no direction");
        let uv_center = projected_uv_offset(
            &vp,
            [center[0], crate::lit_scene::GROUND_Y, center[1]],
            world,
        );
        assert_eq!(
            uv_center,
            [0.0, 0.0],
            "the zero displacement projects to the value-exact identity (0, -0.0)"
        );
        assert_eq!(
            content(edge_u + uv_center[0]),
            content(edge_u),
            "the center's sample is its own value — zero movement at the exact center"
        );
        // THE SIGN-INVERSION NEGATIVE CONTROL: negating the
        // displacement sweeps each probe's sample AWAY from the edge
        // (no crossing) — the inversion the GPU probe's oracle-sign
        // check catches (the measured flip must match the oracle's
        // predicted sweep direction).
        assert_eq!(
            content(u_left - uv_left[0]),
            content(u_left),
            "an inverted displacement sweeps the left probe away from the edge"
        );
        assert_eq!(
            content(u_right - uv_right[0]),
            content(u_right),
            "an inverted displacement sweeps the right probe away from the edge"
        );
    }

    /// THE PLANAR EQUIVALENCE (why the shader may project through the
    /// frame's TRUE view-projection — "the same VP the reflection
    /// target was rendered with"): both the ground point and its
    /// tangent-displaced neighbor live ON the ground plane, where
    /// the mirror fixes every point — the mirrored view-projection
    /// gives both BIT-IDENTICAL clip positions, so the projected uv
    /// difference is exactly equal under either matrix. This is the
    /// documented license for reusing binding 3's true VP instead
    /// of adding a mirrored-VP uniform lane.
    #[test]
    fn projected_offset_is_mirror_equivalent_on_the_plane() {
        use crate::lit_scene::GROUND_Y;
        use projection_kit::{camera_view, mul, perspective};
        let proj = perspective(65_f32.to_radians(), 16.0 / 9.0, 0.1, 150.0);
        let vp = mul(&proj, &camera_view([4.0, 5.0, -11.0], 0.6, -0.2));
        let mirrored = mirrored_view_projection(&vp, GROUND_Y);
        for (ground, push) in [
            ([3.0, GROUND_Y, -7.0], [0.0857, 0.0]),
            ([-20.5, GROUND_Y, 12.25], [0.0, -0.0857]),
            ([0.0, GROUND_Y, 0.0], [0.06, -0.04]),
        ] {
            assert_eq!(
                projected_uv_offset(&vp, ground, push),
                projected_uv_offset(&mirrored, ground, push),
                "the tangent displacement's projection is mirror-invariant at {ground:?}"
            );
        }
        // And the zero push is the value-exact identity under both.
        assert_eq!(
            projected_uv_offset(&vp, [3.0, GROUND_Y, -7.0], [0.0, 0.0]),
            [0.0, 0.0]
        );
    }

    /// THE PLANAR PROPERTY (what the reflection pass rests on): the
    /// mirrored view-projection maps every point ON the ground plane
    /// to exactly the clip position the true view-projection gives
    /// it (the mirror fixes the plane — bit-equal, since M·P = P
    /// exactly there and the composition applies M first). Points
    /// ABOVE the plane map where their mirror images belong.
    #[test]
    fn mirrored_view_projection_fixes_the_ground_plane() {
        use crate::lit_scene::GROUND_Y;
        // A representative view-projection (the corner route's own
        // shape — any nonsingular matrix exercises the composition).
        let perspective = |aspect: f32| -> [[f32; 4]; 4] {
            let fov = 65_f32.to_radians();
            let (near, far) = (0.1f32, 150.0f32);
            let focal = 1.0 / (fov / 2.0).tan();
            let mut matrix = [[0.0f32; 4]; 4];
            matrix[0][0] = focal / aspect;
            matrix[1][1] = focal;
            matrix[2][2] = far / (near - far);
            matrix[2][3] = -1.0;
            matrix[3][2] = near * far / (near - far);
            matrix
        };
        // A rigid view (yaw only — any orthonormal view works; the
        // column-major storage of Rᵀ·T(−position)).
        let (sy, cy) = (0.6f32.sin(), 0.6f32.cos());
        let position = [12.0, 6.0, -9.0];
        let mut view = [
            [cy, 0.0, -sy, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [sy, 0.0, cy, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        for view_column in view.iter_mut().take(3) {
            view_column[3] = -(view_column[0] * position[0]
                + view_column[1] * position[1]
                + view_column[2] * position[2]);
        }
        let mul = |a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]| -> [[f32; 4]; 4] {
            std::array::from_fn(|column| {
                std::array::from_fn(|row| {
                    let mut sum = 0.0;
                    for k in 0..4 {
                        sum += a[k][row] * b[column][k];
                    }
                    sum
                })
            })
        };
        let view_proj = mul(&perspective(1.0), &view);
        let mirrored = mirrored_view_projection(&view_proj, GROUND_Y);
        let clip = |m: &[[f32; 4]; 4], p: [f32; 3]| -> [f32; 4] {
            std::array::from_fn(|i| m[0][i] * p[0] + m[1][i] * p[1] + m[2][i] * p[2] + m[3][i])
        };
        // Ground points: bit-equal clip positions.
        for ground_point in [[3.0, 0.0, -7.0], [-20.5, 0.0, 12.25], [0.0, 0.0, 0.0]] {
            assert_eq!(
                clip(&mirrored, ground_point),
                clip(&view_proj, ground_point),
                "the ground point {ground_point:?} is fixed"
            );
        }
        // A point above the plane maps where its mirror image
        // belongs (the mirror of P under the true matrix).
        let above = [3.0, 5.0, -7.0];
        let mirrored_point = [3.0, -5.0, -7.0];
        assert_eq!(clip(&mirrored, above), clip(&view_proj, mirrored_point));
        // The mirrored eye composition: the reflected viewpoint.
        assert_eq!(mirrored_eye(position, GROUND_Y), [12.0, -6.0, -9.0]);
        assert_eq!(
            mirrored_eye([0.0, 1.7, 0.0], 4.0),
            [0.0, 6.3, 0.0],
            "the non-zero plane offset"
        );
    }
}
