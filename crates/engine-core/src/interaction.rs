//! Cycle 2 C2-P1 — the minimal reusable interaction framework.
//!
//! Engine MECHANICS only: a spatial/facing query over interaction
//! targets and a deterministic selection. What interactions DO —
//! the target kinds, their effects, their feedback — is game policy
//! and lives in the application layer (`engine-slice`'s playable
//! demo), which owns the `InteractionTarget` inventory and reacts to
//! the selected id. This mirrors the engine/game boundary the player
//! module established (mechanics in core, policy in the demo).
//!
//! Determinism: selection is a pure function of (targets, eye,
//! forward, query) — nearest surface distance wins, ties break by
//! the lowest id, iteration order is the caller's slice. Input
//! validity (the R4/R4b fail-visible discipline extended, challenge
//! F5): a non-finite eye or forward REFUSES selection (`None`), and
//! targets with non-finite positions are skipped — never an
//! unpredictable comparison outcome.
//!
//! P2 B06 (the interaction half) — the DISPOSITION: stay linear,
//! with evidence (the charter §2 no-change requirement). The census
//! is O(targets) with one sqrt and one normalize per target and
//! ZERO allocation; the real inventories are a handful (the
//! playable demo's gatehouse offers at most four targets), and the
//! query runs per INTERACT INPUT EDGE — never per frame, never per
//! tick. The design's family-D gate (population ≥ 512 AND ≥ 4
//! queries per tick, P2-simulation-spatial.md §2.2.3) fails both
//! clauses by two orders of magnitude at every measured workload,
//! and the design's L6 sequencing rule reserves family-D index
//! adoption for after the census lane proves the pattern (Q-F/Q-G
//! "may later ride the same index"). [`select_target`] is therefore
//! the permanent small-workload fast path; the stay-linear evidence
//! is measured in `select_target_census_disposition_evidence`.

/// One interactable world entity, as the engine sees it: an identity,
/// a position, and an interaction extent (the radius around the
/// position within which the entity is considered "at" its surface).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InteractionTarget {
    pub id: u32,
    pub position: [f32; 3],
    pub radius: f32,
}

/// The selection query: the player's reach and the facing cone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InteractionQuery {
    /// Maximum surface distance (target extent discounted) at which a
    /// target is offered (m). The R2 input contract: finite and
    /// nonnegative — an invalid reach offers nothing.
    pub range: f32,
    /// The facing cone as the cosine of its half angle: a target is
    /// offered only when the direction to it dots against the look
    /// direction by at least this (1.0 = dead ahead only; 0.0 =
    /// the full forward hemisphere). The R2 input contract: finite
    /// and within [−1.0, 1.0] (the cosine range) — a non-finite or
    /// out-of-range threshold offers nothing, never a bypassed
    /// facing check.
    pub cone_half_angle_cos: f32,
}

impl Default for InteractionQuery {
    fn default() -> Self {
        // The playable demo's policy values (a 2.5 m reach and a
        // 45° half-angle cone), kept here only as a neutral default.
        InteractionQuery {
            range: 2.5,
            cone_half_angle_cos: std::f32::consts::FRAC_1_SQRT_2,
        }
    }
}

/// Selects the interaction target: the nearest (surface distance,
/// extent discounted) target within `range` and inside the facing
/// cone; ties break to the lowest id. Returns `None` when nothing is
/// offered, when the eye or forward is non-finite (the F5 refusal),
/// when the forward vector is degenerate, or when the QUERY is
/// invalid (the R2 contract: a non-finite or negative reach, or a
/// non-finite cone threshold outside the cosine range [−1, 1] — an
/// invalid query offers nothing, never a bypassed facing check).
/// Targets with non-finite positions or radii, or negative radii,
/// are SKIPPED — an invalid target never displaces a valid
/// candidate (an infinite radius would otherwise collapse the
/// surface distance to zero and offer from arbitrarily far away).
pub fn select_target(
    targets: &[InteractionTarget],
    eye: [f32; 3],
    forward: [f32; 3],
    query: &InteractionQuery,
) -> Option<u32> {
    if !query.range.is_finite() || query.range < 0.0 {
        return None;
    }
    if !query.cone_half_angle_cos.is_finite()
        || query.cone_half_angle_cos > 1.0
        || query.cone_half_angle_cos < -1.0
    {
        return None;
    }
    if !eye.iter().all(|v| v.is_finite()) || !forward.iter().all(|v| v.is_finite()) {
        return None;
    }
    let forward_len =
        (forward[0] * forward[0] + forward[1] * forward[1] + forward[2] * forward[2]).sqrt();
    if forward_len == 0.0 || !forward_len.is_finite() {
        return None;
    }
    let look = [
        forward[0] / forward_len,
        forward[1] / forward_len,
        forward[2] / forward_len,
    ];
    let mut best: Option<(u32, f32)> = None;
    for target in targets {
        if !target.position.iter().all(|v| v.is_finite())
            || !target.radius.is_finite()
            || target.radius < 0.0
        {
            continue;
        }
        let delta = [
            target.position[0] - eye[0],
            target.position[1] - eye[1],
            target.position[2] - eye[2],
        ];
        let distance = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
        if distance == 0.0 {
            // The eye is inside the target's extent: dead-ahead by
            // definition (surface 0.0 — the same surface a distant
            // target with radius ≥ distance clamps to, so the
            // documented lowest-id tie governs here exactly as the
            // normal branch's equality does; the pre-fix form used
            // strict less, silently preferring iteration order over
            // the contract the module header promises).
            let candidate = (target.id, 0.0f32);
            let better = best.is_none_or(|(best_id, best_d)| {
                candidate.1 < best_d || (candidate.1 == best_d && target.id < best_id)
            });
            if better {
                best = Some(candidate);
            }
            continue;
        }
        let direction = [
            delta[0] / distance,
            delta[1] / distance,
            delta[2] / distance,
        ];
        let dot = direction[0] * look[0] + direction[1] * look[1] + direction[2] * look[2];
        if dot < query.cone_half_angle_cos {
            continue;
        }
        let surface = (distance - target.radius).max(0.0);
        if surface > query.range {
            continue;
        }
        let better = match best {
            None => true,
            Some((best_id, best_surface)) => {
                surface < best_surface || (surface == best_surface && target.id < best_id)
            }
        };
        if better {
            best = Some((target.id, surface));
        }
    }
    best.map(|(id, _)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(id: u32, position: [f32; 3]) -> InteractionTarget {
        InteractionTarget {
            id,
            position,
            radius: 0.0,
        }
    }

    fn eye_forward() -> ([f32; 3], [f32; 3]) {
        ([0.0, 1.6, 0.0], [0.0, 0.0, -1.0])
    }

    #[test]
    fn selects_nearest_target_in_range() {
        let (eye, forward) = eye_forward();
        let query = InteractionQuery {
            range: 5.0,
            ..InteractionQuery::default()
        };
        let targets = [target(1, [0.0, 1.0, -2.0]), target(2, [0.0, 1.0, -4.0])];
        assert_eq!(select_target(&targets, eye, forward, &query), Some(1));
        // A large extent discounts the surface distance: the "far"
        // target with a 3 m radius is nearer by surface.
        let targets = [
            target(1, [0.0, 1.0, -2.0]),
            InteractionTarget {
                id: 2,
                position: [0.0, 1.0, -4.0],
                radius: 3.0,
            },
        ];
        assert_eq!(select_target(&targets, eye, forward, &query), Some(2));
    }

    #[test]
    fn cone_rejects_behind_and_beside() {
        let (eye, forward) = eye_forward();
        let query = InteractionQuery::default(); // 45° half angle
        let behind = [target(1, [0.0, 1.0, 2.0])];
        assert_eq!(
            select_target(&behind, eye, forward, &query),
            None,
            "a target behind the look direction is never offered"
        );
        let beside = [target(1, [2.0, 1.6, -0.0])]; // ~90° off-axis
        assert_eq!(
            select_target(&beside, eye, forward, &query),
            None,
            "a target beside the cone is not offered"
        );
        let ahead = [target(1, [2.0, 1.6, -0.0]), target(2, [0.5, 1.6, -2.0])];
        assert_eq!(
            select_target(&ahead, eye, forward, &query),
            Some(2),
            "the in-cone target is offered even beside an off-cone one"
        );
    }

    #[test]
    fn tie_breaks_by_id_stable() {
        let (eye, forward) = eye_forward();
        let query = InteractionQuery::default();
        // Two targets mirrored across the path, equidistant.
        let targets = [target(7, [1.0, 1.6, -2.0]), target(3, [-1.0, 1.6, -2.0])];
        let first = select_target(&targets, eye, forward, &query);
        let second = select_target(&targets, eye, forward, &query);
        assert_eq!(
            first,
            Some(3),
            "equal surface distance breaks to the lower id"
        );
        assert_eq!(first, second, "the tie-break is stable across calls");
    }

    #[test]
    fn empty_and_out_of_range_return_none() {
        let (eye, forward) = eye_forward();
        let query = InteractionQuery {
            range: 2.0,
            ..InteractionQuery::default()
        };
        assert_eq!(select_target(&[], eye, forward, &query), None);
        let far = [target(1, [0.0, 1.0, -10.0])];
        assert_eq!(select_target(&far, eye, forward, &query), None);
    }

    #[test]
    fn selection_is_order_independent() {
        let (eye, forward) = eye_forward();
        let query = InteractionQuery {
            range: 5.0,
            ..InteractionQuery::default()
        };
        let a = [target(1, [0.0, 1.0, -3.0]), target(2, [0.4, 1.4, -2.0])];
        let b = [a[1], a[0]];
        assert_eq!(
            select_target(&a, eye, forward, &query),
            select_target(&b, eye, forward, &query),
            "slice order never changes the selection"
        );
    }

    #[test]
    fn nonfinite_inputs_refused_or_skipped() {
        let (eye, forward) = eye_forward();
        let query = InteractionQuery::default();
        // A non-finite eye or forward refuses selection outright.
        assert_eq!(
            select_target(
                &[target(1, [0.0, 1.0, -2.0])],
                [f32::NAN, 1.6, 0.0],
                forward,
                &query
            ),
            None
        );
        assert_eq!(
            select_target(
                &[target(1, [0.0, 1.0, -2.0])],
                eye,
                [0.0, f32::INFINITY, 0.0],
                &query
            ),
            None
        );
        // A non-finite target position is skipped, never an
        // unpredictable comparison: the valid target still selects.
        let targets = [target(2, [f32::NAN, 0.0, 0.0]), target(1, [0.0, 1.0, -2.0])];
        assert_eq!(select_target(&targets, eye, forward, &query), Some(1));
        let all_bad = [target(1, [f32::NAN, 0.0, 0.0])];
        assert_eq!(select_target(&all_bad, eye, forward, &query), None);
    }

    #[test]
    fn query_validity_contract() {
        // R2: an invalid query offers nothing — a non-finite or
        // negative reach, or a non-finite or out-of-range cone
        // threshold (the cosine range is [-1, 1]).
        let (eye, forward) = eye_forward();
        let valid = InteractionQuery::default();
        let targets = [target(1, [0.0, 1.0, -2.0])];
        for query in [
            InteractionQuery {
                range: f32::NAN,
                ..valid
            },
            InteractionQuery {
                range: -1.0,
                ..valid
            },
            InteractionQuery {
                range: f32::INFINITY,
                ..valid
            },
            InteractionQuery {
                cone_half_angle_cos: f32::NAN,
                ..valid
            },
            InteractionQuery {
                cone_half_angle_cos: f32::INFINITY,
                ..valid
            },
            InteractionQuery {
                cone_half_angle_cos: 1.5,
                ..valid
            },
            InteractionQuery {
                cone_half_angle_cos: -1.5,
                ..valid
            },
        ] {
            assert_eq!(
                select_target(&targets, eye, forward, &query),
                None,
                "the invalid query {query:?} offers nothing"
            );
        }
        // The cosine-range boundaries are VALID thresholds: 1.0 is
        // dead-ahead-only (offering a perfectly aligned target);
        // -1.0 accepts the full sphere.
        let dead_ahead = InteractionQuery {
            cone_half_angle_cos: 1.0,
            ..valid
        };
        let aligned = [target(1, [0.0, 1.6, -2.0])];
        assert_eq!(
            select_target(&aligned, eye, forward, &dead_ahead),
            Some(1),
            "a perfectly dead-ahead target is offered at the 1.0 boundary"
        );
        let full_sphere = InteractionQuery {
            cone_half_angle_cos: -1.0,
            ..valid
        };
        let behind = [target(1, [0.0, 1.0, 2.0])];
        assert_eq!(
            select_target(&behind, eye, forward, &full_sphere),
            Some(1),
            "-1.0 is the defined full-sphere boundary, not an error"
        );
    }

    #[test]
    fn target_radius_validity() {
        // R2: an infinite radius never offers (the defect: the
        // surface distance collapses to zero from arbitrarily far
        // away); a negative radius never offers; invalid targets
        // never displace a valid candidate.
        let (eye, forward) = eye_forward();
        let query = InteractionQuery::default();
        let infinite_far = InteractionTarget {
            id: 9,
            position: [0.0, 1.0, -50.0],
            radius: f32::INFINITY,
        };
        assert_eq!(
            select_target(&[infinite_far], eye, forward, &query),
            None,
            "an infinite radius never offers, however far the target"
        );
        let negative = InteractionTarget {
            id: 9,
            position: [0.0, 1.0, -2.0],
            radius: -1.0,
        };
        assert_eq!(
            select_target(&[negative], eye, forward, &query),
            None,
            "a negative radius never offers"
        );
        let valid = target(1, [0.0, 1.0, -2.0]);
        assert_eq!(
            select_target(&[infinite_far, negative, valid], eye, forward, &query),
            Some(1),
            "invalid targets do not displace the valid candidate"
        );
    }

    /// P2-L1 (the B06 conformance fix): two EYE-INSIDE targets tie at
    /// surface 0.0 — the documented lowest-id tie governs, never
    /// iteration order (the pre-fix zero-distance branch used strict
    /// less, silently preferring whichever came first).
    #[test]
    fn zero_distance_ties_break_by_lowest_id() {
        let (eye, forward) = eye_forward();
        let query = InteractionQuery {
            range: 5.0,
            ..InteractionQuery::default()
        };
        // The higher id FIRST: the pre-fix form returned it (first
        // encountered at equal distance); the contract returns 3.
        let targets = [target(7, eye), target(3, eye)];
        assert_eq!(
            select_target(&targets, eye, forward, &query),
            Some(3),
            "the eye-inside tie breaks by the lowest id, never iteration order"
        );
        // Both orders agree (order-independence is the point).
        let reversed = [target(3, eye), target(7, eye)];
        assert_eq!(select_target(&reversed, eye, forward, &query), Some(3));
    }

    /// P2-L1: an eye-inside target (surface 0.0) and a distant target
    /// whose radius clamps its surface to 0.0 tie — the lowest id
    /// wins in EITHER encounter order (the mixed-shape contract the
    /// normal branch's equality already half-carried).
    #[test]
    fn zero_distance_ties_with_clamped_surface_by_lowest_id() {
        let (eye, forward) = eye_forward();
        let query = InteractionQuery {
            range: 10.0,
            ..InteractionQuery::default()
        };
        let inside_high = target(9, eye);
        let clamped_low = InteractionTarget {
            id: 4,
            position: [0.0, 1.0, -5.0],
            radius: 6.0,
        };
        assert_eq!(
            select_target(&[inside_high, clamped_low], eye, forward, &query),
            Some(4)
        );
        assert_eq!(
            select_target(&[clamped_low, inside_high], eye, forward, &query),
            Some(4),
            "the tie is order-independent"
        );
    }

    /// P2 B06: the stay-linear disposition's EVIDENCE — the census
    /// cost from the demo scale (4 targets — the playable demo's
    /// gatehouse inventory) up past the design's family-D gate
    /// population, observational (printed, sanity-bounded only —
    /// the F-06 no-thresholds-before-variance rule): the per-query
    /// cost at demo scale is sub-microsecond with zero allocation,
    /// and the linear census's growth (one sqrt + one normalize per
    /// target) stays in the tens of microseconds even at 4096
    /// targets — a cost the HUMAN input-edge frequency (per interact
    /// press, never per frame or tick) cannot amortize a
    /// per-population index build against. The census still selects
    /// correctly at every measured size (the measurement measures
    /// real work).
    #[test]
    fn select_target_census_disposition_evidence() {
        let (eye, forward) = eye_forward();
        let query = InteractionQuery {
            range: 2.5,
            ..InteractionQuery::default()
        };
        // Alternating ahead/behind placements at distances spanning
        // the reach boundary (the offered and not-offered classes:
        // the cone rejects the behind half, the range rejects the
        // far ahead ones).
        let make = |count: usize| {
            (0..count)
                .map(|i| {
                    let ahead = i % 2 == 0;
                    let distance = 0.5 + (i % 9) as f32 * 0.35;
                    InteractionTarget {
                        id: i as u32 + 1,
                        position: [
                            eye[0],
                            eye[1],
                            eye[2] + if ahead { -distance } else { distance },
                        ],
                        radius: 0.0,
                    }
                })
                .collect::<Vec<_>>()
        };
        for count in [4usize, 64, 512, 4096] {
            let targets = make(count);
            // The census still selects: the nearest ahead target
            // (id 1 at 0.5 m) is offered at every size.
            assert_eq!(
                select_target(&targets, eye, forward, &query),
                Some(1),
                "the census selects correctly at count {count}"
            );
            let runs = ((2_000_000 / count as u64).clamp(1_000, 200_000)) as usize;
            let start = std::time::Instant::now();
            for _ in 0..runs {
                select_target(&targets, eye, forward, &query);
            }
            let ns = start.elapsed().as_secs_f64() / runs as f64 * 1e9;
            println!("[interaction-census] targets {count:5} · {ns:9.1} ns/query · {runs} runs");
            assert!(
                ns.is_finite() && ns < 50_000_000.0,
                "count {count}: the census is absurdly slow: {ns} ns"
            );
        }
    }
}
