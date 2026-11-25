//! The deterministic NPC behavior core (Era 2 — the living world's
//! mind, per `docs/ERA2-NPC-AI-DESIGN-PASS.md` §3–§4): the NPC
//! record's state contract, the need counters, the rule-table
//! selection machinery, and the per-NPC RNG stream — ALL pure,
//! fully tested, no I/O, no wall clocks, no Vulkan.
//!
//! # DETERMINISM CONTRACT
//! Every decision this module produces is byte-identical on every
//! platform: the selector is a pure function over its inputs, the
//! rule table is data iterated in FIXED order, the needs arithmetic
//! is fixed-order IEEE f32, the tie-breaks consume the NPC's own
//! RNG stream in fixed order, and the identity's wire encoding is
//! `to_bits` little-endian in a fixed field order. A fixed rule
//! table over a fixed record produces the same activity sequence
//! forever — pinned by digest.
//!
//! # THE ENGINE/POLICY BOUNDARY (the design's rule, §4.1)
//! This module ships the selector MACHINERY: the evaluation loop,
//! the scoring arithmetic, the tie-break discipline, the digest
//! pinning. The rule TABLE — which activities rank where, what
//! needs exist, what decay rates apply — is policy data authored
//! above the engine (in the game layer). No cyberpunk-specific
//! behavior lives here.
//!
//! # THE TWO-LANE RECORD (the design's §3, M-29/M-30's shape)
//! `NpcIdentity` is the DURABLE lane: stable identity facets that
//! shape future behavior and survive dormancy — wire-encodable,
//! fingerprintable, byte-exact through save/load. `NpcRoutine` is
//! the RECONSTRUCTIBLE lane: the current activity, needs, route
//! progress — captured on deactivation, derived on activation,
//! never persisted as durable truth. The two are deliberately
//! separate types so the commit boundary never blurs.

/// A stable NPC identity — never reused (permanent deaths, M-13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NpcId(pub u32);

/// The generic activity kinds the machinery can execute. Policy
/// defines the SEMANTICS of each kind's parameters; the engine
/// defines only the execution shapes (§4.2's event cadences).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ActivityKind {
    /// No active pursuit — the idle baseline.
    Idle,
    /// Move along a route toward a waypoint anchor.
    MoveTo {
        /// The route index (policy data).
        route: u32,
        /// The destination waypoint index on that route.
        waypoint: u32,
    },
    /// Execute a schedule commitment's activity.
    Fulfill {
        /// The commitment entry index in the schedule table.
        commitment: u32,
    },
    /// A reactive response to a perception (policy's reaction
    /// table decides what this means — the engine only executes
    /// the shape).
    React {
        /// The perception class that triggered the reaction.
        class: u32,
        /// The perceived NPC the reaction concerns.
        subject: NpcId,
    },
}

/// One selected activity: the kind plus the selection's provenance
/// (which rule won, and at which evaluation) — carried for
/// diagnostics and the decision digest, never for authority.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Activity {
    pub kind: ActivityKind,
    /// The winning rule's index in the table (diagnostic).
    pub rule: u32,
}

/// The durable identity lane (§3's rule of thumb: persists through
/// the commit pipeline). Wire-encodable, byte-exact.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NpcIdentity {
    pub id: NpcId,
    /// The home cell on the region grid (the committed anchor).
    pub home_cell: (u8, u8),
    /// The region membership tag.
    pub region: u8,
    /// The appearance/animation seed (the placement-table pattern).
    pub appearance_seed: u32,
}

/// The reconstructible routine lane: the per-NPC active state.
/// Captured before discard on deactivation, derived on activation;
/// NEVER persisted as durable truth (§3's discipline).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NpcRoutine {
    /// The currently executing activity.
    pub current: Activity,
    /// The need counters, policy-defined semantics, fixed-order
    /// arithmetic. Each in [0, 1] (satiety-shaped: 1 = fully
    /// satisfied, decaying toward 0).
    pub needs: [f32; MAX_NEEDS],
    /// The route progress anchor (the last committed waypoint).
    pub route_progress: u32,
}

/// The maximum number of need counters a record carries (the
/// machinery's fixed bound; policy uses fewer if it wants).
pub const MAX_NEEDS: usize = 8;

/// The rule table entry (POLICY data — the engine only defines the
/// evaluation). A rule names the activity it selects, the need it
/// serves (or none), its base priority, and the minimum need level
/// below which it applies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rule {
    /// The activity this rule selects when it wins.
    pub activity: ActivityKind,
    /// The need index this rule serves (None = unconditional).
    pub need: Option<u8>,
    /// The base priority, higher wins among applicable rules.
    pub base_priority: u8,
    /// The rule applies only when its need is below this level
    /// (the hungry-rules-fire-when-hungry shape).
    pub applies_below: f32,
}

/// The world view: a READ SNAPSHOT of the queryable state the
/// policy grants the NPC (§4.1 — never raw spine internals). The
/// engine defines the carrier; the policy defines what fills it.
#[derive(Debug, Clone, Copy, Default)]
pub struct WorldView {
    /// The index of the commitment due this tick (from the
    /// schedule layer), if any.
    pub due_commitment: Option<u32>,
    /// The perception classes fired this tick (from the perception
    /// layer), paired with their subjects, in fixed order.
    pub perceptions: [(u32, NpcId); MAX_PERCEPTIONS],
    /// How many entries of `perceptions` are live.
    pub perception_count: u8,
    /// The time-of-day band [0, 1) (the atmosphere's cycle
    /// position, quantized by the caller).
    pub time_band: f32,
}

/// The per-NPC perception bound (fixed, like MAX_NEEDS).
pub const MAX_PERCEPTIONS: usize = 4;

/// The per-NPC deterministic RNG stream (§8.3's own-stream rule):
/// a SplitMix64 finalizer seeded from the NPC's identity — the
/// same family as the repo's procedural textures (`mix64`), pure
/// and cross-platform.
#[derive(Debug, Clone, Copy)]
pub struct NpcRng {
    state: u64,
}

impl NpcRng {
    /// The stream for an NPC: seeded from the identity, never
    /// shared (the design's own-stream rule).
    pub fn for_npc(id: NpcId) -> Self {
        Self {
            state: mix64(u64::from(id.0)),
        }
    }

    /// The next draw in [0, 1) — fixed order, pure.
    pub fn next_unit(&mut self) -> f32 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        (mix64(self.state) >> 40) as f32 / (1u64 << 24) as f32
    }

    /// An integer draw below `bound` (the tie-break shape).
    pub fn next_below(&mut self, bound: u32) -> u32 {
        if bound <= 1 {
            return 0;
        }
        (self.next_unit() * bound as f32) as u32 % bound
    }
}

/// The SplitMix64 finalizer (the repo's standard — see
/// proc_texture.rs; identical bits on every platform).
fn mix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// THE SELECTOR (§4.1 — the pure function). Evaluates the ordered
/// rule table: a rule is APPLICABLE when its need (if any) is below
/// its `applies_below` level; among applicable rules the SCORE is
/// base_priority + the served need's URGENCY (1 − need level, the
/// hungrier the stronger); the highest score wins; ties break by
/// the RNG stream drawing among the tied rules' indices in table
/// order. A due commitment always overrides the table (the
/// schedule's authority, §6's precedence). A fired perception of a
/// REACTIVE class is offered to the policy's rules like any other
/// input — the engine imposes no fear semantics.
///
/// Same inputs — same activity, byte-identical, cross-platform.
pub fn select_activity(
    routine: &NpcRoutine,
    view: &WorldView,
    rules: &[Rule],
    rng: &mut NpcRng,
) -> Activity {
    // The schedule's authority: a due commitment is the activity.
    // (The policy's escape hatch — a rule table can mark rules as
    // overriding commitments — is NOT engine machinery; §6's
    // precedence stands.)
    if let Some(commitment) = view.due_commitment {
        return Activity {
            kind: ActivityKind::Fulfill { commitment },
            rule: u32::MAX, // the schedule lane, not a table rule
        };
    }
    // The ordered scan: collect the applicable rules' scores in
    // table order (fixed iteration — no sort, no allocation).
    let mut best_score: f32 = f32::MIN;
    let mut best_index: u32 = u32::MAX;
    let mut tied_count: u32 = 0;
    for (index, rule) in rules.iter().enumerate() {
        let applicable = match rule.need {
            Some(need) => routine.needs[need as usize] < rule.applies_below,
            None => true,
        };
        if !applicable {
            continue;
        }
        let urgency = match rule.need {
            Some(need) => 1.0 - routine.needs[need as usize],
            None => 0.0,
        };
        let score = f32::from(rule.base_priority) + urgency;
        if score > best_score {
            best_score = score;
            best_index = index as u32;
            tied_count = 1;
        } else if score == best_score {
            tied_count += 1;
        }
    }
    if best_index == u32::MAX {
        // No applicable rule: the idle baseline (the machinery's
        // floor — never an error).
        return Activity {
            kind: ActivityKind::Idle,
            rule: u32::MAX,
        };
    }
    if tied_count > 1 {
        // The tie-break: draw among the tied indices — the tied
        // set is re-derived in table order (fixed) and the draw
        // consumes the stream exactly once.
        let pick = rng.next_below(tied_count);
        let mut seen: u32 = 0;
        for (index, rule) in rules.iter().enumerate() {
            let applicable = match rule.need {
                Some(need) => routine.needs[need as usize] < rule.applies_below,
                None => true,
            };
            if !applicable {
                continue;
            }
            let urgency = match rule.need {
                Some(need) => 1.0 - routine.needs[need as usize],
                None => 0.0,
            };
            let score = f32::from(rule.base_priority) + urgency;
            if score == best_score {
                if seen == pick {
                    return Activity {
                        kind: rule.activity,
                        rule: index as u32,
                    };
                }
                seen += 1;
            }
        }
        // Unreachable (the tied set is at least tied_count large).
        return Activity {
            kind: ActivityKind::Idle,
            rule: u32::MAX,
        };
    }
    Activity {
        kind: rules[best_index as usize].activity,
        rule: best_index,
    }
}

/// The need decay (the fixed-order arithmetic — §4.1's counters):
/// each need decays by `rate` per elapsed tick, clamped to [0, 1].
/// Pure; the caller owns the cadence.
pub fn decay_needs(routine: &mut NpcRoutine, rates: &[f32; MAX_NEEDS], ticks: u32) {
    for (need, rate) in routine.needs.iter_mut().zip(rates.iter()) {
        let amount = rate * ticks as f32;
        *need = (*need - amount).clamp(0.0, 1.0);
    }
}

/// The activity-sequence digest (the cross-platform byte pin —
/// the FNV discipline): hashes a sampled sequence of selected
/// activities' discriminant data (kind tag, rule index, and the
/// kind's parameters as to_bits) in fixed order.
pub fn decision_digest(samples: &[Activity]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for activity in samples {
        let mut feed = |bytes: &[u8]| {
            for &byte in bytes {
                hash ^= byte as u64;
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        feed(&activity.rule.to_le_bytes());
        match activity.kind {
            ActivityKind::Idle => feed(&[0]),
            ActivityKind::MoveTo { route, waypoint } => {
                feed(&[1]);
                feed(&route.to_le_bytes());
                feed(&waypoint.to_le_bytes());
            }
            ActivityKind::Fulfill { commitment } => {
                feed(&[2]);
                feed(&commitment.to_le_bytes());
            }
            ActivityKind::React { class, subject } => {
                feed(&[3]);
                feed(&class.to_le_bytes());
                feed(&subject.0.to_le_bytes());
            }
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routine(needs: [f32; MAX_NEEDS]) -> NpcRoutine {
        NpcRoutine {
            current: Activity {
                kind: ActivityKind::Idle,
                rule: u32::MAX,
            },
            needs,
            route_progress: 0,
        }
    }

    #[allow(dead_code)]
    fn identity(id: u32) -> NpcIdentity {
        NpcIdentity {
            id: NpcId(id),
            home_cell: (0, 0),
            region: 0,
            appearance_seed: id,
        }
    }

    /// The schedule's authority: a due commitment overrides every
    /// rule, whatever the needs.
    #[test]
    fn due_commitment_overrides_the_table() {
        let mut rng = NpcRng::for_npc(NpcId(1));
        let rules = [Rule {
            activity: ActivityKind::MoveTo {
                route: 0,
                waypoint: 3,
            },
            need: Some(0),
            base_priority: 200,
            applies_below: 0.9,
        }];
        let view = WorldView {
            due_commitment: Some(7),
            ..Default::default()
        };
        let chosen = select_activity(&routine([0.0; MAX_NEEDS]), &view, &rules, &mut rng);
        assert_eq!(
            chosen.kind,
            ActivityKind::Fulfill { commitment: 7 },
            "the schedule lane wins"
        );
    }

    /// The urgency ranking: the hungrier need's rule outranks the
    /// higher base priority of a satisfied need's rule.
    #[test]
    fn urgency_outranks_base_priority() {
        let mut rng = NpcRng::for_npc(NpcId(2));
        let rules = [
            Rule {
                activity: ActivityKind::MoveTo {
                    route: 1,
                    waypoint: 0,
                },
                need: Some(0),
                base_priority: 50,
                applies_below: 0.9,
            },
            Rule {
                activity: ActivityKind::Idle,
                need: Some(1),
                base_priority: 10,
                applies_below: 0.9,
            },
        ];
        // Need 0 at 0.1 (urgency 0.9 + base 50 = 50.9); need 1 at
        // 0.95 (not applicable — above applies_below).
        let chosen = select_activity(
            &routine([0.1, 0.95, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]),
            &WorldView::default(),
            &rules,
            &mut rng,
        );
        assert_eq!(
            chosen.kind,
            ActivityKind::MoveTo {
                route: 1,
                waypoint: 0
            }
        );
        // Flip: need 0 satisfied, need 1 starving — the idle rule
        // wins on urgency despite the lower base.
        let chosen = select_activity(
            &routine([0.95, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]),
            &WorldView::default(),
            &rules,
            &mut rng,
        );
        assert_eq!(chosen.kind, ActivityKind::Idle);
    }

    /// The applicability gate: a rule whose need is above its
    /// applies_below never fires, whatever its priority.
    #[test]
    fn satisfied_needs_do_not_fire_their_rules() {
        let mut rng = NpcRng::for_npc(NpcId(3));
        let rules = [Rule {
            activity: ActivityKind::React {
                class: 9,
                subject: NpcId(99),
            },
            need: Some(2),
            base_priority: 255,
            applies_below: 0.5,
        }];
        let chosen = select_activity(
            &routine([1.0; MAX_NEEDS]),
            &WorldView::default(),
            &rules,
            &mut rng,
        );
        assert_eq!(
            chosen.kind,
            ActivityKind::Idle,
            "no applicable rule -> idle"
        );
    }

    /// The tie-break: two equally-scoring rules resolve by the
    /// NPC's own stream — deterministically (the same NPC and
    /// inputs always pick the same one; different NPCs differ).
    #[test]
    fn ties_break_by_the_own_stream_deterministically() {
        let rules = [
            Rule {
                activity: ActivityKind::Idle,
                need: None,
                base_priority: 10,
                applies_below: 1.0,
            },
            Rule {
                activity: ActivityKind::MoveTo {
                    route: 5,
                    waypoint: 5,
                },
                need: None,
                base_priority: 10,
                applies_below: 1.0,
            },
        ];
        let view = WorldView::default();
        let r = routine([1.0; MAX_NEEDS]);
        // The same NPC: the same choice every time.
        let mut rng_a = NpcRng::for_npc(NpcId(10));
        let first = select_activity(&r, &view, &rules, &mut rng_a);
        let mut rng_b = NpcRng::for_npc(NpcId(10));
        let second = select_activity(&r, &view, &rules, &mut rng_b);
        assert_eq!(first, second, "byte-identical across evaluations");
        // Across a population of streams, both branches occur (the
        // tie-break actually varies — not a constant).
        let mut counts = [0u32; 2];
        for id in 0..64u32 {
            let mut rng = NpcRng::for_npc(NpcId(id));
            let chosen = select_activity(&r, &view, &rules, &mut rng);
            counts[chosen.rule as usize] += 1;
        }
        assert!(counts[0] > 0 && counts[1] > 0, "the tie varies: {counts:?}");
    }

    /// The RNG stream: pure, seeded per-NPC, in [0, 1).
    #[test]
    fn rng_stream_is_pure_and_bounded() {
        let mut a = NpcRng::for_npc(NpcId(42));
        let mut b = NpcRng::for_npc(NpcId(42));
        for _ in 0..100 {
            let x = a.next_unit();
            let y = b.next_unit();
            assert_eq!(x.to_bits(), y.to_bits(), "bit-identical draws");
            assert!((0.0..1.0).contains(&x));
        }
        // Different NPCs differ.
        let mut c = NpcRng::for_npc(NpcId(43));
        assert_ne!(a.next_unit().to_bits(), c.next_unit().to_bits());
    }

    /// The need decay: fixed-order, clamped, tick-proportional.
    #[test]
    fn need_decay_clamps_and_scales() {
        let mut r = routine([0.5, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
        let rates = [0.1, 0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        decay_needs(&mut r, &rates, 2);
        assert!((r.needs[0] - 0.3).abs() < 1e-6, "0.5 - 0.1*2");
        assert!((r.needs[1] - 0.4).abs() < 1e-6, "1.0 - 0.3*2");
        assert_eq!(r.needs[2], 0.0, "floor holds");
        decay_needs(&mut r, &rates, 100);
        assert_eq!(r.needs[0], 0.0, "clamped at the floor");
    }

    /// THE DECISION DIGEST PIN: a fixed rule table over a fixed
    /// need trajectory produces this FNV digest — cross-platform,
    /// forever (the pin discipline; a move is an investigation).
    #[test]
    fn decision_digest_pin() {
        let rules = [
            Rule {
                activity: ActivityKind::Idle,
                need: None,
                base_priority: 5,
                applies_below: 1.0,
            },
            Rule {
                activity: ActivityKind::MoveTo {
                    route: 2,
                    waypoint: 7,
                },
                need: Some(0),
                base_priority: 10,
                applies_below: 0.8,
            },
            Rule {
                activity: ActivityKind::React {
                    class: 4,
                    subject: NpcId(9),
                },
                need: Some(3),
                base_priority: 8,
                applies_below: 0.6,
            },
        ];
        let mut r = routine([1.0; MAX_NEEDS]);
        let rates = [0.05, 0.0, 0.0, 0.07, 0.0, 0.0, 0.0, 0.0];
        let mut rng = NpcRng::for_npc(NpcId(77));
        let mut samples = Vec::new();
        for _ in 0..24 {
            decay_needs(&mut r, &rates, 1);
            let chosen = select_activity(&r, &WorldView::default(), &rules, &mut rng);
            samples.push(chosen);
        }
        let digest = decision_digest(&samples);
        assert_eq!(
            digest, PINNED_DIGEST,
            "the decision digest moved: a selection change — an \
             investigation, never a silent repin"
        );
    }

    /// The measured-actual pin (24 decisions of the pinned
    /// trajectory above — the f32 arithmetic's actual bits).
    const PINNED_DIGEST: u64 = 1277463877274275604;
}
