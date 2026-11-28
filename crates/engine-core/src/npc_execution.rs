//! The NPC AI execution layer (Era 2 — the living world's mind, per
//! `docs/ERA2-NPC-AI-DESIGN-PASS.md` §4.1, §4.2, §5.1, §5.2, §6.2, §8.4):
//! the event payload codecs, the wake/witness evaluation primitives, and
//! the witness census machinery — ALL pure, fully tested, no I/O, no clocks,
//! no Vulkan, no new dependencies.
//!
//! # DETERMINISM CONTRACT
//! Every answer this module produces is byte-identical on every platform:
//! the codecs use fixed-bit-layout u64 payloads, the selector machinery
//! (`npc_behavior.rs`) is pure, the perception query (`npc_perception.rs`)
//! uses fixed-operation-order squared distances, the census walks the
//! population slice in fixed order, and the fingerprint uses FNV-1a over
//! little-endian fixed-width integers. A fixed campaign over a fixed
//! execution schedule produces the same activity stream forever — pinned
//! by digest.
//!
//! # THE ENGINE/POLICY BOUNDARY (the design's rule, §4.1, §6.2)
//! This module ships the EXECUTION LAYER MACHINERY: the payload decoding
//! and validation, the wake/witness evaluators that call the selector,
//! the census enumerator that walks population in fixed order, and the
//! execution-stream digest. The RULE TABLE — which activities rank where,
//! what needs exist, what decay rates apply — is policy data authored
//! above the engine (in the game layer). No cyberpunk-specific behavior
//! lives here.
//!
//! # THE EXECUTION BOUNDARY (design §6.2)
//! The spine fires events with opaque u64 payloads. This module defines
//! the NPC-AI event namespace (tags 0xA1, 0xA2) and decodes them into
//! (npc, commitment) or (npc, cause_class) pairs, then calls the
//! appropriate evaluator. The caller (the game-side World implementation)
//! owns per-NPC state (routine, RNG, last_decay_tick, perception trace
//! memory) and re-arms via ctx.schedule_at. A late wake shifts its effects
//! honestly instead of stacking impossible deadlines — the spine's honesty
//! rule (§3.2), here implemented by anchoring the re-arm to the actual
//! execution tick.

use crate::clock::Tick;
use crate::lod::LodLevel;
use crate::npc_behavior::{
    Activity, ActivityKind, MAX_NEEDS, NpcId, NpcRng, NpcRoutine, Rule, WorldView, decay_needs,
    select_activity, validate_rule_table,
};
use crate::npc_perception::{CensusIndex, NpcRecord, PerceptionTrace, perceive, perception_budget};
use crate::npc_schedule::NpcSchedule;

/// The NPC-AI event family: the commitment wake (a calendar
/// occurrence firing as a spine event).
pub const WAKE_TAG: u8 = 0xA1;
/// The NPC-AI event family: the perception-driven witness wake.
pub const WITNESS_TAG: u8 = 0xA2;

/// Packs a commitment-wake payload: tag(8) | npc(32) | arg(24) —
/// the npc identity in bits 55..24 (full 32 bits: the design's 1000×
/// headroom clause — Experiment-B's 1,000 near × 1,000 = 1M > 16
/// bits), the commitment index in bits 23..0.
///
/// THE TYPED REFUSAL: `None` iff `commitment ≥ 2^24` — an arg that
/// cannot fit the 24-bit field is refused, NEVER silently truncated
/// (the `Cadence::every` zero-period discipline; the game-side
/// `routine_payload` masks because a truncated cursor is recoverable
/// game state, but a truncated EVENT IDENTITY would silently address
/// another NPC's commitment — the engine-side line is absolute). The
/// decoder cannot catch this after the fact: the 24-bit field loses
/// the information, so the refusal must live at this boundary.
pub fn wake_payload(npc: u32, commitment: u32) -> Option<u64> {
    if commitment >= 1 << 24 {
        return None;
    }
    Some(((WAKE_TAG as u64) << 56) | ((npc as u64) << 24) | commitment as u64)
}

/// Decodes a wake payload: `Some((npc, commitment))` only when the
/// tag byte matches `WAKE_TAG` (the arg field is < 2^24 by
/// construction — the packer's typed refusal holds the bound).
pub fn wake_payload_parts(payload: u64) -> Option<(u32, u32)> {
    if (payload >> 56) as u8 != WAKE_TAG {
        return None;
    }
    Some((
        ((payload >> 24) & 0xFFFF_FFFF) as u32,
        (payload & 0xFF_FFFF) as u32,
    ))
}

/// Packs a witness-wake payload: tag(8) | npc(32) | arg(24) — the
/// same layout, the significant event's cause class as the argument.
/// The typed refusal is identical: `None` iff `cause_class ≥ 2^24`.
pub fn witness_payload(npc: u32, cause_class: u32) -> Option<u64> {
    if cause_class >= 1 << 24 {
        return None;
    }
    Some(((WITNESS_TAG as u64) << 56) | ((npc as u64) << 24) | cause_class as u64)
}

/// Decodes a witness payload: `Some((npc, cause_class))` only when
/// the tag matches `WITNESS_TAG`.
pub fn witness_payload_parts(payload: u64) -> Option<(u32, u32)> {
    if (payload >> 56) as u8 != WITNESS_TAG {
        return None;
    }
    Some((
        ((payload >> 24) & 0xFFFF_FFFF) as u32,
        (payload & 0xFF_FFFF) as u32,
    ))
}

/// What a commitment wake's evaluation produces.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WakeOutcome {
    /// The selected activity (schedule authority: a due commitment
    /// always wins — ActivityKind::Fulfill).
    pub activity: Activity,
    /// The routine after lazy need decay (the decay happens per EVENT,
    /// not per tick — §4.2's EventDriven cost shape).
    pub routine_after: NpcRoutine,
    /// The re-arm: THIS entry's next occurrence strictly after the
    /// ACTUAL execution tick (None = the chain ends — a one-shot past
    /// its start, or an unrepresentable next tick).
    pub next_wake: Option<Tick>,
}

/// Evaluates a commitment wake AT ITS ACTUAL EXECUTION TICK (the
/// spine's honesty rule: an overdue cause shifts its effects, never
/// stacks impossible deadlines):
///
/// 1. lazy decay: decay_needs over elapsed = executed_tick −
///    last_decay_tick (saturating; clamped to u32::MAX with a doc note);
/// 2. the view: due_commitment = Some(commitment) — the EVENT's own
///    authority, never a calendar re-derivation (a late one-shot is
///    not "due at" its execution tick anymore; the payload is the truth);
/// 3. select_activity (a due commitment always wins);
/// 4. the honest re-arm: the entry's next occurrence strictly after
///    executed_tick — a late wake SKIPS missed occurrences, never
///    fires them in a burst.
///
/// Returns None (fail closed) iff `commitment` ≥ schedule.entries.len()
/// — the corrupted-input refusal — or the rule table fails
/// [`validate_rule_table`] (the admission refusal: a
/// policy-authored out-of-range need index never reaches the
/// selector's both passes).
///
/// # CONTRACT
/// The caller tracks per-NPC `last_decay_tick` and sets it to
/// `executed_tick` after each evaluation. The routine_after.current
/// = the chosen activity; needs = decayed; route_progress unchanged.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_wake(
    schedule: &NpcSchedule,
    commitment: u32,
    executed_tick: Tick,
    last_decay_tick: Tick,
    routine: &NpcRoutine,
    rules: &[Rule],
    rng: &mut NpcRng,
    rates: &[f32; MAX_NEEDS],
) -> Option<WakeOutcome> {
    // Fail closed on corrupted input: commitment index out of range.
    if commitment as usize >= schedule.entries.len() {
        return None;
    }
    // Fail closed on an inadmissible rule table (the
    // admission refusal — an out-of-range need index is refused
    // here, typed, before the selector can ever index with it).
    if validate_rule_table(rules).is_err() {
        return None;
    }

    // Lazy decay: elapsed ticks since last decay, clamped to u32::MAX.
    // (The schedule layer uses u64 ticks; the behavior layer uses u32
    // decay counts — a saturating clamp keeps this safe.)
    let elapsed = executed_tick
        .as_u64()
        .saturating_sub(last_decay_tick.as_u64());
    let elapsed = if elapsed > u32::MAX as u64 {
        u32::MAX
    } else {
        elapsed as u32
    };

    // Clone the routine to avoid mutating the input.
    let mut routine_after = *routine;
    decay_needs(&mut routine_after, rates, elapsed);

    // The view: the EVENT's commitment index is the due commitment
    // (schedule authority — a due commitment always wins).
    let view = WorldView {
        due_commitment: Some(commitment),
        ..Default::default()
    };

    // Select the activity (the selector's authority), and make it
    // the routine's current activity — the evaluation IS the
    // selection boundary (§4.1), so the advanced routine carries
    // the choice as its state.
    let activity = select_activity(&routine_after, &view, rules, rng);
    routine_after.current = activity;

    // The honest re-arm: the entry's next occurrence strictly after
    // executed_tick — a late wake SKIPS missed occurrences, never
    // fires them in a burst.
    let entry = &schedule.entries[commitment as usize];
    let next_wake = entry.next_occurrence(executed_tick);

    Some(WakeOutcome {
        activity,
        routine_after,
        next_wake,
    })
}

/// What a witness wake's evaluation produces.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WitnessOutcome {
    pub activity: Activity,
    pub routine_after: NpcRoutine,
}

/// Evaluates a witness wake: lazy decay (as above), then the view
/// carrying the FIRED perceptions — the NEWEST ≤ MAX_PERCEPTIONS
/// entries of the witness's memory (the LAST bounded entries of the
/// supplied slice, in the slice's own fixed order — the suffix,
/// never the head: taking the FIRST entries would silently drop the
/// newest) — and NO due commitment
/// (a witness wake is not a calendar event). The selector offers
/// the perceptions to the rule table like any other input (the
/// engine imposes no fear semantics — §4.1); a rule with a witness
/// matcher can gate on a fired class and bind its observed subject
/// (npc_behavior's matcher contract).
///
/// Returns `None` (fail closed) iff the rule table fails
/// [`validate_rule_table`] (the admission refusal) — the same
/// typed refusal the wake path takes, on the witness path too.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_witness(
    routine: &NpcRoutine,
    perceptions: &[(u32, NpcId)],
    executed_tick: Tick,
    last_decay_tick: Tick,
    rules: &[Rule],
    rng: &mut NpcRng,
    rates: &[f32; MAX_NEEDS],
) -> Option<WitnessOutcome> {
    // The admission refusal (fail closed, typed — never a panic in
    // the selector's passes).
    validate_rule_table(rules).ok()?;

    // Lazy decay (same as wake).
    let elapsed = executed_tick
        .as_u64()
        .saturating_sub(last_decay_tick.as_u64());
    let elapsed = if elapsed > u32::MAX as u64 {
        u32::MAX
    } else {
        elapsed as u32
    };

    let mut routine_after = *routine;
    decay_needs(&mut routine_after, rates, elapsed);

    // The view: the NEWEST bounded SUFFIX of the supplied slice (the
    // last ≤ MAX_PERCEPTIONS entries, fixed order), no due
    // commitment (a witness wake is not a calendar event).
    let mut view = WorldView::default();
    let count = perceptions.len().min(crate::npc_behavior::MAX_PERCEPTIONS);
    let suffix_start = perceptions.len() - count;
    for (i, &(class, subject)) in perceptions[suffix_start..].iter().enumerate() {
        view.perceptions[i] = (class, subject);
    }
    view.perception_count = count as u8;

    // Select the activity; the advanced routine carries the choice
    // (the selection boundary, as in evaluate_wake).
    let activity = select_activity(&routine_after, &view, rules, rng);
    routine_after.current = activity;

    Some(WitnessOutcome {
        activity,
        routine_after,
    })
}

/// Enumerates a significant event's witness set into the CALLER-OWNED
/// `out` scratch (cleared first — the F-02 caller-held-scratch
/// discipline: the warm event path allocates nothing, the scratch's
/// capacity is reused): every population record within `radius` of
/// `center`, as POPULATION INDICES in fixed population order,
/// EXCLUDING NPCs whose tier's perception budget is zero (Dormant
/// under the landed table — the I2/I3 invariant: a dormant NPC
/// witnesses nothing). The census is INVERTED (one walk per
/// significant event, never N NPCs × M events) and ORDERED
/// (population order — §8.3). An inadmissible radius (NaN, negative,
/// non-finite) yields nothing: `perceive`'s own fail-closed gate is
/// the single source of that rule, never re-implemented here.
///
/// `tiers` is indexed by population position; a missing tier entry
/// is a caller bug this function fails closed on (that NPC is not
/// enumerated — never an over-perception).
pub fn enumerate_witnesses(
    center: [f32; 2],
    radius: f32,
    population: &[NpcRecord],
    tiers: &[LodLevel],
    out: &mut Vec<u32>,
) {
    out.clear();
    for idx in perceive(center, radius, population) {
        // The landed budget table is the authority: a tier whose
        // budget is zero perceives nothing (Dormant today; if the
        // table ever changes, the census follows it — the exclusion
        // is never re-derived here).
        let Some(tier) = tiers.get(idx) else {
            continue;
        };
        if perception_budget(tier_level(*tier)) == 0 {
            continue;
        }
        out.push(idx as u32);
    }
}

/// The `LodLevel` → level-number mapping `perception_budget` indexes
/// (`PERCEPTION_BUDGETS`' documented ordering: 0 = Full, 1 =
/// Simplified, 2 = EventDriven, 3 = Dormant — `lod.rs`'s declaration
/// order, most to least detailed).
fn tier_level(level: LodLevel) -> u8 {
    match level {
        LodLevel::Full => 0,
        LodLevel::Simplified => 1,
        LodLevel::EventDriven => 2,
        LodLevel::Dormant => 3,
    }
}

/// The INDEXED sibling of [`enumerate_witnesses`] (P2 L6 — the
/// composition seam's D-family census index entry): the identical
/// witness census — every population record within `radius` of
/// `center`, as POPULATION INDICES in fixed population order,
/// EXCLUDING zero-budget tiers (Dormant) — answered through a
/// built [`CensusIndex`] instead of the linear walk. Bit-identical
/// to [`enumerate_witnesses`] on the same (center, radius,
/// population, tiers): the index's yields are the linear walk's own
/// yields (see `npc_perception`'s module docs for the superset
/// argument), so this composition's tier filter — the SAME filter,
/// over the SAME ordered indices, `perception_budget` never
/// re-derived — produces the identical `out` by construction.
///
/// The composition calls
/// `CensusIndexScratch::build`/`build_if_advisable` once per tick
/// (or per population-change batch — the cadence the composition
/// owns, per the P2 design §2.2.2) and passes the handle to every
/// census that batch; `out` is the same caller-owned scratch
/// (cleared first, the F-02 discipline). The population inside the
/// handle is the slice the caller built it from, enforced by the
/// borrow — never a mismatched slice.
pub fn enumerate_witnesses_indexed(
    center: [f32; 2],
    radius: f32,
    index: &mut CensusIndex<'_, '_>,
    tiers: &[LodLevel],
    out: &mut Vec<u32>,
) {
    out.clear();
    for idx in index.perceive(center, radius) {
        // The landed budget table is the authority: a tier whose
        // budget is zero perceives nothing (Dormant today; if the
        // table ever changes, the census follows it — the exclusion
        // is never re-derived here). Identical to the linear
        // composition's filter, by construction.
        let Some(tier) = tiers.get(idx) else {
            continue;
        };
        if perception_budget(tier_level(*tier)) == 0 {
            continue;
        }
        out.push(idx as u32);
    }
}

/// Appends `trace` to an NPC's bounded memory with oldest-first
/// eviction at the tier's perception budget (§5.2's retention
/// machinery: bounded depth, oldest-first expiry; the depth IS the
/// landed four-tier budget table's value). The Vec's capacity is
/// never reduced, so the warm path is amortized zero-allocation once
/// capacity reaches the bound.
///
/// A budget of zero retains nothing (the append is refused — the
/// Dormant shape: the trace never enters memory).
pub fn retain_trace(memory: &mut Vec<PerceptionTrace>, trace: PerceptionTrace, budget: usize) {
    if budget == 0 {
        return;
    }
    if memory.len() >= budget {
        memory.remove(0);
    }
    memory.push(trace);
}

/// One recorded evaluation in the composed behavior stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamEntry {
    pub tick: Tick,
    pub npc: u32,
    pub activity: Activity,
}

/// FNV-1a 64 (the repo's Fingerprinter constants, inline like
/// decision_digest) over, per entry, in THIS fixed order:
/// tick(u64 LE), npc(u32 LE), then EXACTLY decision_digest's per-
/// activity field sequence (rule u32 LE; kind tag byte 0/1/2/3; the
/// kind's params as LE u32s in decision_digest's order).
///
/// THE CROSS-PLATFORM BYTE PIN: a fixed campaign's stream hashes to one
/// u64 everywhere; a divergence is an investigation, never a repin.
///
/// # IMPLEMENTATION NOTES
/// The decision_digest function hashes:
///   - rule (u32 LE)
///   - kind tag byte (0=Idle, 1=MoveTo, 2=Fulfill, 3=React)
///   - params (route+waypoint or commitment or class+subject as u32 LE)
///
/// This digest folds those exact bytes in the same order: tick, npc,
/// then the same sequence decision_digest would produce for the
/// activity.
pub fn execution_digest(entries: &[StreamEntry]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;

    for entry in entries {
        // tick(u64 LE)
        let tick_bytes = entry.tick.as_u64().to_le_bytes();
        for &byte in &tick_bytes {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }

        // npc(u32 LE)
        let npc_bytes = entry.npc.to_le_bytes();
        for &byte in &npc_bytes {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }

        // Same as decision_digest's activity encoding:
        // rule u32 LE
        let rule_bytes = entry.activity.rule.to_le_bytes();
        for &byte in &rule_bytes {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }

        // kind tag byte
        let tag = match entry.activity.kind {
            ActivityKind::Idle => 0,
            ActivityKind::MoveTo { .. } => 1,
            ActivityKind::Fulfill { .. } => 2,
            ActivityKind::React { .. } => 3,
        };
        hash ^= tag as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);

        // kind params as LE u32s in decision_digest's order
        match entry.activity.kind {
            ActivityKind::Idle => {
                // No params.
            }
            ActivityKind::MoveTo { route, waypoint } => {
                let route_bytes = route.to_le_bytes();
                for &byte in &route_bytes {
                    hash ^= byte as u64;
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
                let waypoint_bytes = waypoint.to_le_bytes();
                for &byte in &waypoint_bytes {
                    hash ^= byte as u64;
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
            ActivityKind::Fulfill { commitment } => {
                let commitment_bytes = commitment.to_le_bytes();
                for &byte in &commitment_bytes {
                    hash ^= byte as u64;
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
            ActivityKind::React { class, subject } => {
                let class_bytes = class.to_le_bytes();
                for &byte in &class_bytes {
                    hash ^= byte as u64;
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
                let subject_bytes = subject.0.to_le_bytes();
                for &byte in &subject_bytes {
                    hash ^= byte as u64;
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
        }
    }

    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Field;
    use crate::npc_behavior::{NpcId, NpcRng};
    use crate::npc_perception::{CensusIndexScratch, PerceptionTrace};
    use crate::npc_schedule::{ActivityDescriptor, Cadence, Commitment, Target};

    /// A one-entry schedule targeting NPC 0, recurring every 10 from
    /// tick 0 — the shared fixture for the wake-evaluation tests.
    fn recurring_schedule() -> NpcSchedule {
        NpcSchedule {
            entries: vec![Commitment {
                start: Tick(0),
                cadence: Cadence::every(10).expect("period 10 is nonzero"),
                activity: ActivityDescriptor {
                    kind: 1,
                    field: Field::Near,
                },
                target: Target {
                    npc: 0,
                    anchor: (0, 0),
                },
            }],
        }
    }

    /// The fresh routine: idle, all needs satisfied.
    fn fresh_routine() -> NpcRoutine {
        NpcRoutine {
            current: Activity {
                kind: ActivityKind::Idle,
                rule: u32::MAX,
            },
            needs: [1.0; MAX_NEEDS],
            route_progress: 0,
        }
    }

    /// Test 1: the payload codecs — round-trips, the typed refusal,
    /// and the namespace-collision proof.
    #[test]
    fn payload_codecs_round_trip_and_refuse_foreign() {
        // Round-trips, both shapes.
        let payload = wake_payload(123_456, 67_890).expect("representable args pack");
        assert_eq!(
            wake_payload_parts(payload),
            Some((123_456, 67_890)),
            "wake round-trip"
        );
        let payload = witness_payload(54_321, 98_765).expect("representable args pack");
        assert_eq!(
            witness_payload_parts(payload),
            Some((54_321, 98_765)),
            "witness round-trip"
        );

        // The typed refusal: an arg that cannot fit the 24-bit field
        // is refused at pack time, never silently truncated (a
        // truncated commitment index would address ANOTHER NPC's
        // commitment — the engine-side line).
        assert!(
            wake_payload(1, 1 << 24).is_none(),
            "commitment >= 2^24 refused"
        );
        assert!(
            witness_payload(1, 1 << 24).is_none(),
            "cause class >= 2^24 refused"
        );
        assert!(
            wake_payload(1, u32::MAX).is_none(),
            "the extreme arg refused"
        );

        // The namespace-collision proof: the engine-slice routine
        // layout's top byte is 0x5E, never 0xA1/0xA2 — a foreign
        // payload can never decode as an NPC-AI event, and (tags
        // being distinct bytes) vice versa.
        let routine_layout = (0x5E47u64 << 48) | (123u64 << 32) | 456u64;
        assert_eq!((routine_layout >> 56) as u8, 0x5E, "the layout's top byte");
        assert!(
            wake_payload_parts(routine_layout).is_none(),
            "wake refuses it"
        );
        assert!(
            witness_payload_parts(routine_layout).is_none(),
            "witness refuses it"
        );

        // Any other tag byte is foreign too.
        let untagged = (123u64 << 24) | 456u64;
        assert!(wake_payload_parts(untagged).is_none(), "tag 0x00 refused");
        assert!(
            witness_payload_parts(untagged).is_none(),
            "tag 0x00 refused"
        );
        // A WAKE-tag payload is not a WITNESS payload and vice versa.
        let wake = wake_payload(7, 9).expect("packs");
        assert!(witness_payload_parts(wake).is_none(), "cross-shape refusal");
        let witness = witness_payload(7, 9).expect("packs");
        assert!(wake_payload_parts(witness).is_none(), "cross-shape refusal");
    }

    /// Test 2: the wake evaluation decays lazily (exact f32
    /// arithmetic — 0.125 × 4 = 0.5 bit-exactly) and selects the
    /// commitment under schedule authority.
    #[test]
    fn wake_evaluation_decays_lazily_and_selects_the_commitment() {
        let rules = [
            Rule {
                activity: ActivityKind::Idle,
                need: None,
                base_priority: 5,
                applies_below: 1.0,
                witness: None,
            },
            Rule {
                activity: ActivityKind::MoveTo {
                    route: 2,
                    waypoint: 7,
                },
                need: Some(0),
                base_priority: 10,
                applies_below: 0.8,
                witness: None,
            },
        ];
        // rate 0.125 is a power of two: 1.0 − 0.125·4 = 0.5 EXACTLY
        // in f32 (the known-answer discipline — no tolerance band).
        let mut rates = [0.0; MAX_NEEDS];
        rates[0] = 0.125;

        let mut rng = NpcRng::for_npc(NpcId(1));
        let outcome = evaluate_wake(
            &recurring_schedule(),
            0,
            Tick(4),
            Tick(0),
            &fresh_routine(),
            &rules,
            &mut rng,
            &rates,
        )
        .expect("the commitment index is in range");

        assert_eq!(outcome.routine_after.needs[0], 0.5, "1.0 - 0.125*4 exactly");
        assert_eq!(outcome.routine_after.needs[1], 1.0, "untouched needs hold");
        // Schedule authority: the due commitment is the activity,
        // selected through the schedule lane (rule == u32::MAX), and
        // the routine's current activity becomes it.
        assert_eq!(
            outcome.activity.kind,
            ActivityKind::Fulfill { commitment: 0 }
        );
        assert_eq!(
            outcome.activity.rule,
            u32::MAX,
            "the schedule lane, not a table rule"
        );
        assert_eq!(outcome.routine_after.current, outcome.activity);
        assert_eq!(
            outcome.routine_after.route_progress, 0,
            "route progress unchanged"
        );
    }

    /// Test 3: the re-arm anchors at ACTUAL execution — the honesty
    /// rule. Entry: start 0, period 10 (occurrences 0, 10, 20, 30…).
    #[test]
    fn wake_evaluation_rearm_anchors_at_actual_execution() {
        let rules = [];
        let rates = [0.0; MAX_NEEDS];
        let evaluate = |executed: u64| {
            let mut rng = NpcRng::for_npc(NpcId(2));
            evaluate_wake(
                &recurring_schedule(),
                0,
                Tick(executed),
                Tick(0),
                &fresh_routine(),
                &rules,
                &mut rng,
                &rates,
            )
            .expect("in range")
        };

        // On time: executed 0 → the next occurrence is 10.
        assert_eq!(evaluate(0).next_wake, Some(Tick(10)), "on time -> 10");
        // Late but before the next occurrence: executed 7 → still 10
        // (the shift absorbed; no double-fire).
        assert_eq!(evaluate(7).next_wake, Some(Tick(10)), "late at 7 -> 10");
        // Late past an occurrence: executed 12 → 20; the missed
        // occurrence at 10 is SKIPPED, never fired in a burst.
        assert_eq!(
            evaluate(12).next_wake,
            Some(Tick(20)),
            "late at 12 -> 20 (10 skipped)"
        );
        assert_eq!(evaluate(25).next_wake, Some(Tick(30)), "late at 25 -> 30");
        // A one-shot past its start ends the chain.
        let one_shot = NpcSchedule {
            entries: vec![Commitment {
                start: Tick(5),
                cadence: Cadence::one_shot(),
                activity: ActivityDescriptor {
                    kind: 1,
                    field: Field::Far,
                },
                target: Target {
                    npc: 0,
                    anchor: (0, 0),
                },
            }],
        };
        let mut rng = NpcRng::for_npc(NpcId(2));
        let outcome = evaluate_wake(
            &one_shot,
            0,
            Tick(9),
            Tick(0),
            &fresh_routine(),
            &rules,
            &mut rng,
            &rates,
        )
        .expect("in range");
        assert_eq!(
            outcome.next_wake, None,
            "a one-shot past its start ends the chain"
        );
    }

    /// Test 4: the corrupted-input refusal — a commitment index
    /// foreign to the schedule fails closed.
    #[test]
    fn wake_evaluation_fails_closed_on_a_foreign_commitment() {
        let mut rng = NpcRng::for_npc(NpcId(3));
        let outcome = evaluate_wake(
            &recurring_schedule(),
            1, // entries.len() == 1: index 1 is foreign
            Tick(0),
            Tick(0),
            &fresh_routine(),
            &[],
            &mut rng,
            &[0.0; MAX_NEEDS],
        );
        assert!(outcome.is_none(), "a foreign commitment index fails closed");
    }

    /// Test 5: the witness evaluation selects from the rule table
    /// (no schedule authority) and is bit-identical across
    /// evaluations of the same stream.
    #[test]
    fn witness_evaluation_selects_from_the_table() {
        let rules = [
            Rule {
                activity: ActivityKind::Idle,
                need: None,
                base_priority: 5,
                applies_below: 1.0,
                witness: None,
            },
            Rule {
                activity: ActivityKind::MoveTo {
                    route: 5,
                    waypoint: 5,
                },
                need: Some(0),
                base_priority: 10,
                applies_below: 0.6,
                witness: None,
            },
        ];
        // Need 0 at 0.4: the MoveTo rule applies (0.4 < 0.6) and
        // scores 10 + (1 − 0.4) = 10.6 > the idle baseline's 5.
        let mut routine = fresh_routine();
        routine.needs[0] = 0.4;
        // One fired perception: class 9, subject NPC 99.
        let perceptions = [(9u32, NpcId(99))];

        let mut rng_a = NpcRng::for_npc(NpcId(4));
        let first = evaluate_witness(
            &routine,
            &perceptions,
            Tick(0),
            Tick(0),
            &rules,
            &mut rng_a,
            &[0.0; MAX_NEEDS],
        )
        .expect("the authored table validates");
        assert_eq!(
            first.activity.kind,
            ActivityKind::MoveTo {
                route: 5,
                waypoint: 5
            },
            "the need-gated rule wins at need 0.4"
        );

        // Satisfied need: the gated rule does not apply (0.9 >= 0.6);
        // the idle baseline wins.
        routine.needs[0] = 0.9;
        let mut rng_b = NpcRng::for_npc(NpcId(4));
        let second = evaluate_witness(
            &routine,
            &perceptions,
            Tick(0),
            Tick(0),
            &rules,
            &mut rng_b,
            &[0.0; MAX_NEEDS],
        )
        .expect("the authored table validates");
        assert_eq!(
            second.activity.kind,
            ActivityKind::Idle,
            "the baseline wins at 0.9"
        );

        // Purity: the same inputs through a fresh stream of the same
        // seed produce the bit-identical activity.
        routine.needs[0] = 0.4;
        let mut rng_c = NpcRng::for_npc(NpcId(4));
        let third = evaluate_witness(
            &routine,
            &perceptions,
            Tick(0),
            Tick(0),
            &rules,
            &mut rng_c,
            &[0.0; MAX_NEEDS],
        )
        .expect("the authored table validates");
        assert_eq!(
            first.activity, third.activity,
            "bit-identical across evaluations"
        );
    }

    /// THE WITNESS MATCHER THROUGH THE EVALUATOR (finding 6's
    /// deciding test at the execution boundary): hold needs, seed,
    /// and table fixed; change ONLY the fired class or subject —
    /// the authored reaction changes, and the observed subject is
    /// bound through the evaluator exactly as through the selector.
    #[test]
    fn witness_evaluation_reacts_to_the_fired_class_and_subject() {
        let rules = [
            Rule {
                activity: ActivityKind::Idle,
                need: None,
                base_priority: 5,
                applies_below: 1.0,
                witness: None,
            },
            Rule {
                activity: ActivityKind::React {
                    class: 5,
                    subject: NpcId(0),
                },
                need: None,
                base_priority: 50,
                applies_below: 1.0,
                witness: Some(crate::npc_behavior::WitnessMatcher { class: 5 }),
            },
        ];
        let rates = [0.0; MAX_NEEDS];
        let evaluate = |perceptions: &[(u32, NpcId)]| {
            let mut rng = NpcRng::for_npc(NpcId(21));
            evaluate_witness(
                &fresh_routine(),
                perceptions,
                Tick(0),
                Tick(0),
                &rules,
                &mut rng,
                &rates,
            )
            .expect("the authored table validates")
        };
        // Empty observation: the idle baseline.
        assert_eq!(
            evaluate(&[]).activity.kind,
            ActivityKind::Idle,
            "an empty observation is distinguishable from the fired class"
        );
        // The class fires: the reaction carries the OBSERVED subject.
        assert_eq!(
            evaluate(&[(5, NpcId(42))]).activity.kind,
            ActivityKind::React {
                class: 5,
                subject: NpcId(42)
            },
            "the observed subject is bound"
        );
        // ONLY the subject changes: the binding follows.
        assert_eq!(
            evaluate(&[(5, NpcId(8))]).activity.kind,
            ActivityKind::React {
                class: 5,
                subject: NpcId(8)
            },
            "a different observed subject changes the authored reaction"
        );
        // ONLY the class changes: the gate stays shut.
        assert_eq!(
            evaluate(&[(6, NpcId(42))]).activity.kind,
            ActivityKind::Idle,
            "a different fired class changes the authored reaction (back to \
             the baseline — the matcher matched nothing)"
        );
    }

    /// THE NEWEST SUFFIX (finding 6's deciding test): five entries
    /// supplied, MAX_PERCEPTIONS 4 — the view must carry the LAST
    /// four (the newest), never the first four. The sole matching
    /// perception sits LAST; a take-first implementation reads the
    /// oldest four and never sees it.
    #[test]
    fn witness_evaluation_takes_the_newest_bounded_suffix() {
        let rules = [Rule {
            activity: ActivityKind::React {
                class: 5,
                subject: NpcId(0),
            },
            need: None,
            base_priority: 50,
            applies_below: 1.0,
            witness: Some(crate::npc_behavior::WitnessMatcher { class: 5 }),
        }];
        // Five entries: classes 1..=4 then the sole class-5 match
        // LAST, at index 4 — inside the newest four, outside the
        // oldest four.
        let perceptions = [
            (1u32, NpcId(10)),
            (2, NpcId(11)),
            (3, NpcId(12)),
            (4, NpcId(13)),
            (5, NpcId(14)),
        ];
        let mut rng = NpcRng::for_npc(NpcId(22));
        let outcome = evaluate_witness(
            &fresh_routine(),
            &perceptions,
            Tick(0),
            Tick(0),
            &rules,
            &mut rng,
            &[0.0; MAX_NEEDS],
        )
        .expect("the authored table validates");
        assert_eq!(
            outcome.activity.kind,
            ActivityKind::React {
                class: 5,
                subject: NpcId(14)
            },
            "the sole matching perception is LAST in a five-entry input — only \
             the newest-bounded suffix sees it"
        );
        // The bound holds: a class-1-only input (the oldest entry of
        // the five) is OUTSIDE the suffix once newer entries exist —
        // moving the class-1 match to the front of five entries does
        // not fire a class-1 matcher.
        let rules_front = [Rule {
            activity: ActivityKind::React {
                class: 1,
                subject: NpcId(0),
            },
            need: None,
            base_priority: 50,
            applies_below: 1.0,
            witness: Some(crate::npc_behavior::WitnessMatcher { class: 1 }),
        }];
        let mut rng = NpcRng::for_npc(NpcId(22));
        let outcome = evaluate_witness(
            &fresh_routine(),
            &perceptions,
            Tick(0),
            Tick(0),
            &rules_front,
            &mut rng,
            &[0.0; MAX_NEEDS],
        )
        .expect("the authored table validates");
        assert_eq!(
            outcome.activity.kind,
            ActivityKind::Idle,
            "the oldest entry (class 1, index 0) is evicted by the newer four — \
             the suffix is the NEWEST bounded entries"
        );
    }

    /// THE ADMISSION REFUSAL through both evaluators (finding 6):
    /// an out-of-range need index fails the table at admission —
    /// typed on the wake path (the existing corrupted-input refusal
    /// channel) and on the witness path (the new one).
    #[test]
    fn both_evaluators_fail_closed_on_an_out_of_range_need_index() {
        let rules = [Rule {
            activity: ActivityKind::Idle,
            need: Some(u8::MAX),
            base_priority: 5,
            applies_below: 1.0,
            witness: None,
        }];
        let mut rng = NpcRng::for_npc(NpcId(23));
        assert!(
            evaluate_wake(
                &recurring_schedule(),
                0,
                Tick(0),
                Tick(0),
                &fresh_routine(),
                &rules,
                &mut rng,
                &[0.0; MAX_NEEDS],
            )
            .is_none(),
            "the wake evaluation fails closed on the inadmissible table"
        );
        assert!(
            evaluate_witness(
                &fresh_routine(),
                &[(5, NpcId(1))],
                Tick(0),
                Tick(0),
                &rules,
                &mut rng,
                &[0.0; MAX_NEEDS],
            )
            .is_none(),
            "the witness evaluation fails closed on the inadmissible table"
        );
    }

    /// Test 6: the census is inverted, ordered, tier-gated, and
    /// clears the caller's scratch. Geometry (center (0,0), radius
    /// 5.0 — squared 25.0, the inclusive boundary the perception
    /// layer pins): NPC 0 at distance 0 (in), NPC 1 at the 3-4-5
    /// diagonal distance 5 (in, inclusive), NPC 2 at (5,0) distance
    /// 5 (in), NPC 3 at distance 6 (out), NPC 4 at distance 10
    /// (out), NPC 5 at (−1,0) distance 1 (in).
    #[test]
    fn enumerate_witnesses_is_inverted_ordered_and_tier_gated() {
        let population = [
            NpcRecord {
                id: 0,
                pos: [0.0, 0.0],
            },
            NpcRecord {
                id: 1,
                pos: [3.0, 4.0],
            },
            NpcRecord {
                id: 2,
                pos: [5.0, 0.0],
            },
            NpcRecord {
                id: 3,
                pos: [6.0, 0.0],
            },
            NpcRecord {
                id: 4,
                pos: [10.0, 0.0],
            },
            NpcRecord {
                id: 5,
                pos: [-1.0, 0.0],
            },
        ];
        // NPC 1 is Dormant: inside the radius but excluded (the
        // I2/I3 invariant — its tier's budget is zero).
        let tiers = [
            LodLevel::Full,
            LodLevel::Dormant,
            LodLevel::Full,
            LodLevel::Full,
            LodLevel::Full,
            LodLevel::Full,
        ];

        let mut out = Vec::new();
        enumerate_witnesses([0.0, 0.0], 5.0, &population, &tiers, &mut out);
        // Population order, the Dormant NPC 1 excluded.
        assert_eq!(out, vec![0, 2, 5], "indices 0, 2, 5 in population order");

        // The scratch is CLEARED by the census (the F-02 discipline):
        // a second, narrower call leaves only its own witnesses —
        // stale entries from the first call would survive a missing
        // clear and fail this exactly. Radius 1.0: NPC 0 at distance
        // 0 and NPC 5 at distance EXACTLY 1 (the inclusive boundary
        // the perception layer pins — 1.0 ≤ 1.0).
        enumerate_witnesses([0.0, 0.0], 1.0, &population, &tiers, &mut out);
        assert_eq!(
            out,
            vec![0, 5],
            "the census cleared the scratch (radius 1: NPCs 0 and 5, inclusive)"
        );
    }

    /// Test 7: the fail-closed radii — the perception layer's own
    /// admissibility gate (NaN, negative, non-finite) perceives
    /// nobody; a zero radius perceives exactly the center.
    #[test]
    fn enumerate_witnesses_fail_closed_on_invalid_radius() {
        let population = [NpcRecord {
            id: 0,
            pos: [0.0, 0.0],
        }];
        let tiers = [LodLevel::Full];
        let mut out = Vec::new();

        enumerate_witnesses([0.0, 0.0], f32::NAN, &population, &tiers, &mut out);
        assert!(out.is_empty(), "NaN radius yields nothing");
        enumerate_witnesses([0.0, 0.0], -1.0, &population, &tiers, &mut out);
        assert!(out.is_empty(), "negative radius yields nothing");
        enumerate_witnesses([0.0, 0.0], f32::INFINITY, &population, &tiers, &mut out);
        assert!(out.is_empty(), "non-finite radius yields nothing");
        enumerate_witnesses([0.0, 0.0], 0.0, &population, &tiers, &mut out);
        assert_eq!(out, vec![0], "zero radius perceives exactly the center");
    }

    /// Test 8: the retention evicts oldest-first at the budget, and a
    /// zero budget retains nothing (the Dormant shape — the append
    /// is refused, never a panic on the empty memory).
    #[test]
    fn retain_trace_evicts_oldest_first_at_the_budget() {
        let trace = |perceived: u32, tick: u64| PerceptionTrace {
            observer: 0,
            perceived,
            tick: Tick(tick),
            class: 10,
        };
        let mut memory = Vec::new();

        retain_trace(&mut memory, trace(1, 1), 2);
        retain_trace(&mut memory, trace(2, 2), 2);
        assert_eq!(memory.len(), 2, "two appends fill the budget");

        retain_trace(&mut memory, trace(3, 3), 2);
        assert_eq!(memory.len(), 2, "the third append evicts, not grows");
        assert_eq!(memory[0].perceived, 2, "the oldest (tick 1) evicted");
        assert_eq!(memory[1].perceived, 3, "the newest appended, in order");

        // The zero budget refuses the append entirely.
        let mut dormant_memory = Vec::new();
        retain_trace(&mut dormant_memory, trace(9, 9), 0);
        assert!(dormant_memory.is_empty(), "a zero budget retains nothing");
    }

    /// Test 9: the execution digest is pure and entry-sensitive.
    #[test]
    fn execution_digest_is_pure_and_entry_sensitive() {
        let entries = vec![
            StreamEntry {
                tick: Tick(10),
                npc: 1,
                activity: Activity {
                    kind: ActivityKind::Idle,
                    rule: 5,
                },
            },
            StreamEntry {
                tick: Tick(20),
                npc: 2,
                activity: Activity {
                    kind: ActivityKind::MoveTo {
                        route: 3,
                        waypoint: 4,
                    },
                    rule: 10,
                },
            },
        ];
        let digest = execution_digest(&entries);
        assert_eq!(digest, execution_digest(&entries.clone()), "pure");

        let mut moved = entries.clone();
        moved[0].tick = Tick(11);
        assert_ne!(execution_digest(&moved), digest, "tick-sensitive");

        let mut moved = entries.clone();
        moved[0].npc = 2;
        assert_ne!(execution_digest(&moved), digest, "npc-sensitive");

        let mut moved = entries.clone();
        moved[1].activity.kind = ActivityKind::MoveTo {
            route: 5,
            waypoint: 6,
        };
        assert_ne!(execution_digest(&moved), digest, "param-sensitive");

        let mut moved = entries.clone();
        moved[1].activity.rule = 11;
        assert_ne!(execution_digest(&moved), digest, "rule-sensitive");
    }

    // ------------------------------------------------------------
    // THE NPC-AI INTEGRATION BATTERY (Era 2 package 1 — §8)
    // ------------------------------------------------------------

    use crate::event::{Event, EventClass, EventSpec, Schedule};
    use crate::fingerprint::Fingerprinter;
    use crate::npc_behavior::MAX_PERCEPTIONS;
    use crate::spine::{ExecCtx, Spine, World};

    /// The test campaign's event classes (policy constants).
    const CLASS_WAKE: EventClass = EventClass::new(0xC1);
    const CLASS_WITNESS: EventClass = EventClass::new(0xC2);
    const CLASS_SIGNIFICANT: EventClass = EventClass::new(0xC3);

    /// One authored significant event: when it fires, where, its census
    /// radius, its cause class, and the actor NPC it concerns.
    #[derive(Clone, Debug)]
    struct SignificantAuthored {
        due: Tick,
        center: [f32; 2],
        radius: f32,
        cause: u32,
        actor: u32,
    }

    /// Per-NPC evaluation state (the world's reconstructible lane).
    struct WorldNpc {
        id: u32,
        routine: NpcRoutine,
        rng: NpcRng,
        last_decay_tick: Tick,
        memory: Vec<PerceptionTrace>,
    }

    /// The behavior test world: drives the npc_execution machinery over a
    /// real spine. POLICY lives here (schedules, rule table, rates, the
    /// census population) — the engine machinery stays pure.
    struct BehaviorWorld {
        npcs: Vec<WorldNpc>,
        schedules: Vec<NpcSchedule>, // npcs[i]'s authored table (entries target npc i)
        population: Vec<NpcRecord>,  // the census slice, parallel to npcs
        tiers: Vec<LodLevel>,        // per NPC
        rules: Vec<Rule>,
        rates: [f32; MAX_NEEDS],
        significant: Vec<SignificantAuthored>,
        /// The recorded composed stream (the digest's source).
        stream: Vec<StreamEntry>,
        /// Per-wake observation: (npc, commitment, intended_due, executed_tick).
        wake_log: Vec<(u32, u32, Tick, Tick)>,
        /// The reused census scratch (zero allocation on the event path).
        scratch: Vec<u32>,
        /// The P2-L6 census-index scratch (the per-batch counting
        /// sort's reusable buffers — see `use_indexed_census`).
        census_scratch: CensusIndexScratch,
        /// The census path selector (P2 L6's battery control):
        /// false = the exact linear census (every pinned test's
        /// path); true = the indexed census through
        /// `enumerate_witnesses_indexed`. Both paths must produce
        /// byte-identical streams — the L6 equivalence proof.
        use_indexed_census: bool,
        wake_executions: u64,
        witness_executions: u64,
        census_witnesses: u64,
    }

    impl crate::spine::World for BehaviorWorld {
        fn execute(&mut self, event: &Event, ctx: &mut ExecCtx) {
            if event.class == CLASS_WAKE {
                let Some((npc, commitment)) = wake_payload_parts(event.payload) else {
                    return;
                };
                let Some(npc_i) = self.npcs.iter().position(|n| n.id == npc) else {
                    return;
                };
                // Copy-in/copy-out (NpcRoutine and NpcRng are Copy) so the
                // borrowed schedule and the state never alias:
                let mut rng = self.npcs[npc_i].rng;
                let routine = self.npcs[npc_i].routine;
                let Some(outcome) = evaluate_wake(
                    &self.schedules[npc_i],
                    commitment,
                    ctx.executed_tick(),
                    self.npcs[npc_i].last_decay_tick,
                    &routine,
                    &self.rules,
                    &mut rng,
                    &self.rates,
                ) else {
                    return;
                };
                let intended = match event.schedule {
                    Schedule::AtLogicalTime { due } => due,
                    _ => ctx.executed_tick(),
                };
                self.wake_log
                    .push((npc, commitment, intended, ctx.executed_tick()));
                self.npcs[npc_i].routine = outcome.routine_after;
                self.npcs[npc_i].rng = rng;
                self.npcs[npc_i].last_decay_tick = ctx.executed_tick();
                self.stream.push(StreamEntry {
                    tick: ctx.executed_tick(),
                    npc,
                    activity: outcome.activity,
                });
                self.wake_executions += 1;
                if let Some(next) = outcome.next_wake {
                    let payload =
                        wake_payload(npc, commitment).expect("the authored index round-trips");
                    ctx.schedule_at(next, CLASS_WAKE, payload, Vec::new());
                }
            } else if event.class == CLASS_WITNESS {
                let Some((npc, _cause)) = witness_payload_parts(event.payload) else {
                    return;
                };
                let Some(npc_i) = self.npcs.iter().position(|n| n.id == npc) else {
                    return;
                };
                let mut rng = self.npcs[npc_i].rng;
                let routine = self.npcs[npc_i].routine;
                // The newest <= MAX_PERCEPTIONS traces, fixed order, stack-only:
                let memory = &self.npcs[npc_i].memory;
                let start = memory.len().saturating_sub(MAX_PERCEPTIONS);
                let mut perceptions = [(0u32, NpcId(0)); MAX_PERCEPTIONS];
                let mut count = 0usize;
                for trace in &memory[start..] {
                    perceptions[count] = (trace.class, NpcId(trace.perceived));
                    count += 1;
                }
                let outcome = evaluate_witness(
                    &routine,
                    &perceptions[..count],
                    ctx.executed_tick(),
                    self.npcs[npc_i].last_decay_tick,
                    &self.rules,
                    &mut rng,
                    &self.rates,
                )
                .expect("the authored table validates at admission");
                self.npcs[npc_i].routine = outcome.routine_after;
                self.npcs[npc_i].rng = rng;
                self.npcs[npc_i].last_decay_tick = ctx.executed_tick();
                self.stream.push(StreamEntry {
                    tick: ctx.executed_tick(),
                    npc,
                    activity: outcome.activity,
                });
                self.witness_executions += 1;
            } else if event.class == CLASS_SIGNIFICANT {
                let Some(authored) = self.significant.get(event.payload as usize) else {
                    return;
                };
                let BehaviorWorld {
                    population,
                    tiers,
                    npcs,
                    scratch,
                    census_scratch,
                    use_indexed_census,
                    ..
                } = self;
                // The P2-L6 census path: the indexed census through
                // a per-significant-event build (this world's
                // population is static, so per-event and per-tick
                // cadences coincide; a position-mutating composition
                // rebuilds per tick or per batch — the design's
                // cadence rule). The build's `None` (below the
                // forced gate's poison rule) falls back to the
                // exact linear census, so the stream never depends
                // on the index being available.
                if *use_indexed_census && let Some(mut index) = census_scratch.build(population) {
                    enumerate_witnesses_indexed(
                        authored.center,
                        authored.radius,
                        &mut index,
                        tiers,
                        scratch,
                    );
                } else {
                    enumerate_witnesses(
                        authored.center,
                        authored.radius,
                        population,
                        tiers,
                        scratch,
                    );
                }
                self.census_witnesses += scratch.len() as u64;
                for &witness_idx in scratch.iter() {
                    let idx = witness_idx as usize;
                    let witness = population[idx].id;
                    let budget = perception_budget(match tiers[idx] {
                        LodLevel::Full => 0,
                        LodLevel::Simplified => 1,
                        LodLevel::EventDriven => 2,
                        LodLevel::Dormant => 3,
                    });
                    retain_trace(
                        &mut npcs[idx].memory,
                        PerceptionTrace {
                            observer: witness,
                            perceived: authored.actor,
                            tick: ctx.executed_tick(),
                            class: authored.cause,
                        },
                        budget,
                    );
                    let payload =
                        witness_payload(witness, authored.cause).expect("class fits 24 bits");
                    ctx.follow_up_in_field(Field::Near, 0, CLASS_WITNESS, payload, vec![event.id]);
                }
            }
        }

        fn fingerprint(&self, fp: &mut Fingerprinter) {
            fp.write_str("engine-core/npc-execution-world/1");
            for npc in &self.npcs {
                fp.write_u32(npc.id);
                fp.write_u32(npc.routine.current.rule);
                fp.write_u32(match npc.routine.current.kind {
                    ActivityKind::Idle => 0,
                    ActivityKind::MoveTo { .. } => 1,
                    ActivityKind::Fulfill { .. } => 2,
                    ActivityKind::React { .. } => 3,
                });
                for need in npc.routine.needs {
                    fp.write_u64(u64::from(need.to_bits()));
                }
                fp.write_tick(npc.last_decay_tick);
                fp.write_usize(npc.memory.len());
                for trace in &npc.memory {
                    fp.write_bytes(&trace.canonical_bytes());
                }
            }
            fp.write_u64(self.wake_executions);
            fp.write_u64(self.witness_executions);
            fp.write_u64(self.census_witnesses);
        }
    }

    /// Arms all wake events and significant events into the spine.
    fn arm_campaign(spine: &mut Spine<BehaviorWorld>) {
        let world = spine.world();

        // Collect all specs first (to avoid borrow checker issues)
        let mut specs: Vec<EventSpec> = Vec::new();

        // Arm wakes: fixed order — npc asc, entry asc (the deterministic seq assignment)
        for npc_i in 0..world.npcs.len() {
            for (entry_i, entry) in world.schedules[npc_i].entries.iter().enumerate() {
                let payload = wake_payload(world.npcs[npc_i].id, entry_i as u32)
                    .expect("authored index fits");
                specs.push(EventSpec {
                    class: CLASS_WAKE,
                    schedule: Schedule::AtLogicalTime { due: entry.start },
                    payload,
                    field: entry.activity.field,
                    depends_on: Vec::new(),
                });
            }
        }
        // Arm significant events
        for (k, authored) in world.significant.iter().enumerate() {
            specs.push(EventSpec {
                class: CLASS_SIGNIFICANT,
                schedule: Schedule::AtLogicalTime { due: authored.due },
                payload: k as u64,
                field: Field::Near,
                depends_on: Vec::new(),
            });
        }

        // Drop the borrow before scheduling: the specs Vec is owned,
        // so the `world` borrow ends at the loops (NLL) and the
        // mutable `schedule` calls below are legal.
        for spec in specs {
            spine.schedule(spec).expect("arming is legal");
        }
    }

    /// Builds the standard test campaign: 8 NPCs with distinct schedules.
    fn build_campaign() -> Spine<BehaviorWorld> {
        // Population: NPC 0..7 with hand-authored positions
        // NPC 7 is Dormant (excluded from census)
        let population = [
            NpcRecord {
                id: 0,
                pos: [0.0, 0.0],
            },
            NpcRecord {
                id: 1,
                pos: [1.0, 0.0],
            },
            NpcRecord {
                id: 2,
                pos: [2.0, 0.0],
            },
            NpcRecord {
                id: 3,
                pos: [10.0, 0.0],
            },
            NpcRecord {
                id: 4,
                pos: [11.0, 0.0],
            },
            NpcRecord {
                id: 5,
                pos: [12.0, 0.0],
            },
            NpcRecord {
                id: 6,
                pos: [13.0, 0.0],
            },
            NpcRecord {
                id: 7,
                pos: [0.1, 0.1],
            }, // Inside the radius of the first significant event
        ];
        let tiers = [
            LodLevel::Full,
            LodLevel::Full,
            LodLevel::Full,
            LodLevel::Full,
            LodLevel::Full,
            LodLevel::Full,
            LodLevel::Full,
            LodLevel::Dormant, // NPC 7 is excluded from census
        ];

        // Shared rule table: idle baseline, MoveTo need-gated, React rule
        let rules = [
            Rule {
                activity: ActivityKind::Idle,
                need: None,
                base_priority: 5,
                applies_below: 1.0,
                witness: None,
            },
            Rule {
                activity: ActivityKind::MoveTo {
                    route: 1,
                    waypoint: 1,
                },
                need: Some(0),
                base_priority: 10,
                applies_below: 0.8,
                witness: None,
            },
            Rule {
                activity: ActivityKind::React {
                    class: 4,
                    subject: NpcId(2),
                },
                need: Some(3),
                base_priority: 8,
                applies_below: 0.6,
                witness: None,
            },
        ]
        .to_vec();
        let rates = [0.05, 0.0, 0.0, 0.02, 0.0, 0.0, 0.0, 0.0];

        // Per-NPC schedules (different periods/starts, mixed near/far)
        let schedules = [
            // NPC 0: recurring start 1 period 7 Near
            NpcSchedule {
                entries: vec![Commitment {
                    start: Tick(1),
                    cadence: Cadence::every(7).expect("period 7 is nonzero"),
                    activity: ActivityDescriptor {
                        kind: 2,
                        field: Field::Near,
                    },
                    target: Target {
                        npc: 0,
                        anchor: (0, 0),
                    },
                }],
            },
            // NPC 1: recurring start 2 period 11 Far
            NpcSchedule {
                entries: vec![Commitment {
                    start: Tick(2),
                    cadence: Cadence::every(11).expect("period 11 is nonzero"),
                    activity: ActivityDescriptor {
                        kind: 1,
                        field: Field::Far,
                    },
                    target: Target {
                        npc: 1,
                        anchor: (0, 0),
                    },
                }],
            },
            // NPC 2: recurring start 3 period 13 Near
            NpcSchedule {
                entries: vec![Commitment {
                    start: Tick(3),
                    cadence: Cadence::every(13).expect("period 13 is nonzero"),
                    activity: ActivityDescriptor {
                        kind: 1,
                        field: Field::Near,
                    },
                    target: Target {
                        npc: 2,
                        anchor: (0, 0),
                    },
                }],
            },
            // NPC 3: one-shot start 4 Near
            NpcSchedule {
                entries: vec![Commitment {
                    start: Tick(4),
                    cadence: Cadence::one_shot(),
                    activity: ActivityDescriptor {
                        kind: 1,
                        field: Field::Near,
                    },
                    target: Target {
                        npc: 3,
                        anchor: (0, 0),
                    },
                }],
            },
            // NPC 4: recurring start 5 period 17 Far
            NpcSchedule {
                entries: vec![Commitment {
                    start: Tick(5),
                    cadence: Cadence::every(17).expect("period 17 is nonzero"),
                    activity: ActivityDescriptor {
                        kind: 1,
                        field: Field::Far,
                    },
                    target: Target {
                        npc: 4,
                        anchor: (0, 0),
                    },
                }],
            },
            // NPC 5: recurring start 6 period 19 Near
            NpcSchedule {
                entries: vec![Commitment {
                    start: Tick(6),
                    cadence: Cadence::every(19).expect("period 19 is nonzero"),
                    activity: ActivityDescriptor {
                        kind: 1,
                        field: Field::Near,
                    },
                    target: Target {
                        npc: 5,
                        anchor: (0, 0),
                    },
                }],
            },
            // NPC 6: one-shot start 9 Far
            NpcSchedule {
                entries: vec![Commitment {
                    start: Tick(9),
                    cadence: Cadence::one_shot(),
                    activity: ActivityDescriptor {
                        kind: 1,
                        field: Field::Far,
                    },
                    target: Target {
                        npc: 6,
                        anchor: (0, 0),
                    },
                }],
            },
            // NPC 7 (Dormant): recurring start 2 period 5 Far
            NpcSchedule {
                entries: vec![Commitment {
                    start: Tick(2),
                    cadence: Cadence::every(5).expect("period 5 is nonzero"),
                    activity: ActivityDescriptor {
                        kind: 1,
                        field: Field::Far,
                    },
                    target: Target {
                        npc: 7,
                        anchor: (0, 0),
                    },
                }],
            },
        ];

        // Significant events
        let significant = vec![
            // due 20: covers NPCs 0,1,2,7 (NPC 7 is inside radius but Dormant)
            SignificantAuthored {
                due: Tick(20),
                center: [1.0, 0.0],
                radius: 2.0,
                cause: 5,
                actor: 99,
            },
            // due 50: covers NPC 3 only
            SignificantAuthored {
                due: Tick(50),
                center: [10.0, 0.0],
                radius: 1.0,
                cause: 6,
                actor: 99,
            },
        ];

        // Build world NPCs with fresh routines
        let npcs: Vec<WorldNpc> = (0..8)
            .map(|i| WorldNpc {
                id: i as u32,
                routine: NpcRoutine {
                    current: Activity {
                        kind: ActivityKind::Idle,
                        rule: u32::MAX,
                    },
                    needs: [1.0; MAX_NEEDS],
                    route_progress: 0,
                },
                rng: NpcRng::for_npc(NpcId(i as u32)),
                last_decay_tick: Tick(0),
                memory: Vec::new(),
            })
            .collect();

        // Build the world data first
        let schedules_vec = schedules.to_vec();
        let population_vec = population.to_vec();
        let tiers_vec = tiers.to_vec();
        let rules_vec = rules.to_vec();
        let significant_vec = significant;

        // Now build the world with the data
        let world = BehaviorWorld {
            npcs,
            schedules: schedules_vec,
            population: population_vec,
            tiers: tiers_vec,
            rules: rules_vec,
            rates,
            significant: significant_vec,
            stream: Vec::new(),
            wake_log: Vec::new(),
            scratch: Vec::new(),
            census_scratch: CensusIndexScratch::new(),
            use_indexed_census: false,
            wake_executions: 0,
            witness_executions: 0,
            census_witnesses: 0,
        };

        // Create spine with the world
        Spine::with_reservation(world, 32, 16)
    }

    /// Test 10: double-run byte-identical with all three paths exercised.
    #[test]
    fn seeded_campaign_double_run_byte_identical() {
        // First run
        let mut spine1 = build_campaign();
        arm_campaign(&mut spine1);
        spine1.run_ticks(200);

        // Record first run results
        let stream1 = spine1.world().stream.clone();
        let digest1 = execution_digest(&stream1);
        let mut fp1 = Fingerprinter::new();
        spine1.world().fingerprint(&mut fp1);
        let fingerprint1 = fp1.finish();
        let wake_exec1 = spine1.world().wake_executions;
        let witness_exec1 = spine1.world().witness_executions;
        let census_wit1 = spine1.world().census_witnesses;

        // Second run (independent construction, no shared state)
        let mut spine2 = build_campaign();
        arm_campaign(&mut spine2);
        spine2.run_ticks(200);

        // Record second run results
        let stream2 = spine2.world().stream.clone();
        let digest2 = execution_digest(&stream2);
        let mut fp2 = Fingerprinter::new();
        spine2.world().fingerprint(&mut fp2);
        let fingerprint2 = fp2.finish();

        // Assert byte-identical stream and fingerprint
        assert_eq!(digest1, digest2, "stream digest byte-identical across runs");
        assert_eq!(
            fingerprint1, fingerprint2,
            "world fingerprint byte-identical across runs"
        );

        // Assert all three execution paths were exercised
        assert!(wake_exec1 > 0, "wake executions occurred");
        assert!(witness_exec1 > 0, "witness executions occurred");
        assert!(census_wit1 > 0, "census witnesses occurred");
    }

    /// Test 11: stream digest pin and activity variety verification.
    #[test]
    fn seeded_campaign_stream_digest_pin() {
        // Run the campaign once
        let mut spine = build_campaign();
        arm_campaign(&mut spine);
        spine.run_ticks(200);

        // The measured stream digest (run once and capture)
        const PINNED_STREAM_DIGEST: u64 = 10778018066104731562;
        let digest = execution_digest(&spine.world().stream);
        assert_eq!(
            digest, PINNED_STREAM_DIGEST,
            "stream digest pin matches measured value"
        );

        // Assert at least one Fulfill for NPC 0's recurring chain (period 7)
        // Occurrences: 1, 8, 15, 22, 29, 36, 43, 50, 57, 64, 71, 78, 85, 92, 99, 106, 113, 120, 127, 134, 141, 148, 155, 162, 169, 176, 183, 190, 197
        // Across 200 ticks, that's at least 29 occurrences (k=0..28: 1+7*28=197)
        let npc0_fulfill_count = spine
            .world()
            .stream
            .iter()
            .filter(|e| e.npc == 0 && matches!(e.activity.kind, ActivityKind::Fulfill { .. }))
            .count();
        assert!(
            npc0_fulfill_count >= 20,
            "NPC 0 recurring chain Fulfill count >= 20 (got {})",
            npc0_fulfill_count
        );

        // Assert at least one non-Fulfill activity (witness-driven)
        let has_non_fulfill = spine
            .world()
            .stream
            .iter()
            .any(|e| !matches!(e.activity.kind, ActivityKind::Fulfill { .. }));
        assert!(
            has_non_fulfill,
            "stream contains at least one non-Fulfill entry (witness-driven)"
        );
    }

    /// Test 12: synchrony cluster executes within the near reservation.
    #[test]
    fn synchrony_cluster_executes_within_the_near_reservation() {
        // 40 NPCs, each with ONE recurring near-field commitment
        // All start 10, period 100 — they fire exactly once at tick 10
        // Budget 32, near_reserve 16: near executes 16/tick
        // Ticks 10 (16), 11 (16), 12 (8) = 40 total

        let budget = 32u32;
        let near_reserve = 16u32;

        let mut population = Vec::new();
        let mut tiers = Vec::new();
        let mut schedules = Vec::new();
        let mut npcs = Vec::new();

        for i in 0..40 {
            population.push(NpcRecord {
                id: i as u32,
                pos: [0.0, 0.0],
            });
            tiers.push(LodLevel::Full);
            schedules.push(NpcSchedule {
                entries: vec![Commitment {
                    start: Tick(10),
                    cadence: Cadence::every(100).expect("period 100 nonzero"),
                    activity: ActivityDescriptor {
                        kind: 2,
                        field: Field::Near,
                    },
                    target: Target {
                        npc: i as u32,
                        anchor: (0, 0),
                    },
                }],
            });
            npcs.push(WorldNpc {
                id: i as u32,
                routine: NpcRoutine {
                    current: Activity {
                        kind: ActivityKind::Idle,
                        rule: u32::MAX,
                    },
                    needs: [1.0; MAX_NEEDS],
                    route_progress: 0,
                },
                rng: NpcRng::for_npc(NpcId(i as u32)),
                last_decay_tick: Tick(0),
                memory: Vec::new(),
            });
        }

        let rules = [Rule {
            activity: ActivityKind::Idle,
            need: None,
            base_priority: 5,
            applies_below: 1.0,
            witness: None,
        }];
        let rates = [0.0; MAX_NEEDS];

        let significant = Vec::new();
        let scratch = Vec::new();

        let world = BehaviorWorld {
            npcs,
            schedules,
            population,
            tiers,
            rules: rules.to_vec(),
            rates,
            significant,
            stream: Vec::new(),
            wake_log: Vec::new(),
            scratch,
            census_scratch: CensusIndexScratch::new(),
            use_indexed_census: false,
            wake_executions: 0,
            witness_executions: 0,
            census_witnesses: 0,
        };

        let mut spine = Spine::with_reservation(world, budget, near_reserve);
        arm_campaign(&mut spine);
        spine.run_ticks(60);

        let world = spine.world();

        // Assert exactly 40 wake executions
        assert_eq!(world.wake_executions, 40, "exactly 40 wake executions");

        // Assert all wake_log entries with intended_due 10 executed at tick 10, 11, or 12
        for &(_npc, _commitment, intended, executed) in &world.wake_log {
            if intended == Tick(10) {
                assert!(
                    executed == Tick(10) || executed == Tick(11) || executed == Tick(12),
                    "intended_due 10 executed at {}, not 10/11/12",
                    executed
                );
            }
        }

        // Assert lane totals: 40 near_executed
        assert_eq!(spine.lane_totals().near_executed, 40, "40 near executions");
    }

    /// Test 13: late arrival shifts effects honestly — the §3.2
    /// discipline on the real queue. The geometry is engineered so
    /// the test entry executes PAST its next calendar occurrence,
    /// which is the only shape in which the honest re-arm is VISIBLE
    /// (an execution merely after the due tick but before the next
    /// occurrence re-arms to the same tick an on-time chain would —
    /// the shift would be invisible; this test proves the skip).
    ///
    /// Budget 4, near_reserve 2. The far lane's capacity is 2/tick
    /// while near work holds the reservation, then 4/tick once the
    /// near queue is empty (the three-phase policy: phase 2 far up
    /// to budget − reserve, phase 3 the remainder redistributes
    /// near-FIRST and falls through to far when no near work is
    /// due — verified against run_tick's drain order).
    ///
    /// The fixture: NPCs 0/1 hold the reservation at tick 5 (two
    /// near one-shot wakes due 5 — without them the far lane would
    /// take the whole budget). NPCs 2..15 are fourteen far one-shot
    /// fillers due 5. NPC 16 carries TWO entries: a far one-shot
    /// (index 0) and the RECURRING TEST ENTRY (index 1: start 5,
    /// period 3 — occurrences 5, 8, 11, 14, …), armed LAST in the
    /// fixed npc-asc/entry-asc order, so it carries the HIGHEST seq
    /// of the sixteen far events due tick 5.
    ///
    /// The far cohort drains: tick 5 executes 2 (the reservation
    /// held), ticks 6/7/8 execute 4 each (the near queue empty, the
    /// remainder falling through to far) — 14 through tick 8 — and
    /// the last two (NPC 16's one-shot, then the test entry) execute
    /// at tick 9. The test entry: intended_due 5, executed_tick 9 —
    /// LATE by 4. The honest re-arm: next_occurrence(9) = 11, NOT 8
    /// (the on-time chain from execution 5 would arm 8): the
    /// 8-occurrence is SKIPPED — never armed, never fired — exactly
    /// "an overdue cause shifts its effects honestly instead of
    /// stacking impossible deadlines." The chain then runs 11, 14,
    /// … through the 30-tick window.
    #[test]
    fn late_arrival_shifts_effects_honestly() {
        let budget = 4u32;
        let near_reserve = 2u32;

        let mut population = Vec::new();
        let mut tiers = Vec::new();
        let mut schedules = Vec::new();
        let mut npcs = Vec::new();

        let fresh = |id: u32| WorldNpc {
            id,
            routine: NpcRoutine {
                current: Activity {
                    kind: ActivityKind::Idle,
                    rule: u32::MAX,
                },
                needs: [1.0; MAX_NEEDS],
                route_progress: 0,
            },
            rng: NpcRng::for_npc(NpcId(id)),
            last_decay_tick: Tick(0),
            memory: Vec::new(),
        };
        let one_shot = |npc: u32, field: Field| Commitment {
            start: Tick(5),
            cadence: Cadence::one_shot(),
            activity: ActivityDescriptor { kind: 2, field },
            target: Target {
                npc,
                anchor: (0, 0),
            },
        };

        // NPCs 0/1: the near pair that holds the reservation at tick 5.
        for id in 0u32..2 {
            population.push(NpcRecord {
                id,
                pos: [0.0, 0.0],
            });
            tiers.push(LodLevel::Full);
            schedules.push(NpcSchedule {
                entries: vec![one_shot(id, Field::Near)],
            });
            npcs.push(fresh(id));
        }
        // NPCs 2..15: fourteen far one-shot fillers due tick 5.
        for id in 2u32..16 {
            population.push(NpcRecord {
                id,
                pos: [0.0, 0.0],
            });
            tiers.push(LodLevel::Full);
            schedules.push(NpcSchedule {
                entries: vec![one_shot(id, Field::Far)],
            });
            npcs.push(fresh(id));
        }
        // NPC 16: the far one-shot (index 0) then the recurring test
        // entry (index 1) — armed last, the highest seq in the cohort.
        population.push(NpcRecord {
            id: 16,
            pos: [0.0, 0.0],
        });
        tiers.push(LodLevel::Full);
        schedules.push(NpcSchedule {
            entries: vec![
                one_shot(16, Field::Far),
                Commitment {
                    start: Tick(5),
                    cadence: Cadence::every(3).expect("period 3 nonzero"),
                    activity: ActivityDescriptor {
                        kind: 2,
                        field: Field::Far,
                    },
                    target: Target {
                        npc: 16,
                        anchor: (0, 0),
                    },
                },
            ],
        });
        npcs.push(fresh(16));

        let world = BehaviorWorld {
            npcs,
            schedules,
            population,
            tiers,
            rules: vec![Rule {
                activity: ActivityKind::Idle,
                need: None,
                base_priority: 5,
                applies_below: 1.0,
                witness: None,
            }],
            rates: [0.0; MAX_NEEDS],
            significant: Vec::new(),
            stream: Vec::new(),
            wake_log: Vec::new(),
            scratch: Vec::new(),
            census_scratch: CensusIndexScratch::new(),
            use_indexed_census: false,
            wake_executions: 0,
            witness_executions: 0,
            census_witnesses: 0,
        };

        let mut spine = Spine::with_reservation(world, budget, near_reserve);
        arm_campaign(&mut spine);
        spine.run_ticks(30);

        let world = spine.world();

        // The test entry's execution log (npc 16, commitment 1).
        let chain: Vec<(u32, u32, Tick, Tick)> = world
            .wake_log
            .iter()
            .filter(|&&(npc, commitment, _, _)| npc == 16 && commitment == 1)
            .copied()
            .collect();
        assert!(
            !chain.is_empty(),
            "the test entry executed (a vacuous skip is the failure this gate exists to catch)"
        );

        // The first execution: intended 5, executed 9 — LATE by 4.
        // (Drain arithmetic: 2 far at tick 5 under the held
        // reservation, 4/tick at 6/7/8, the final two at tick 9.)
        assert_eq!(chain[0].2, Tick(5), "the armed occurrence: intended_due 5");
        assert_eq!(chain[0].3, Tick(9), "executed at tick 9 — late by 4");

        // THE HONEST SKIP: no execution of this entry ever carries
        // intended_due 8. The on-time chain (executed 5) would have
        // armed 8; the late chain re-armed from ACTUAL execution 9 to
        // 11, jumping the 8-occurrence — never a burst of missed
        // occurrences.
        assert!(
            !chain.iter().any(|&(_, _, intended, _)| intended == Tick(8)),
            "the missed 8-occurrence is skipped, never armed"
        );
        // The re-arm from actual execution: intended 11 (== 5 + 2·3,
        // the next occurrence strictly after 9).
        assert_eq!(
            chain[1].2,
            Tick(11),
            "the re-arm anchors at actual execution"
        );

        // The chain through the 30-tick window: executions whose
        // intended dues are 5 (late), then 11, 14, 17, 20, 23, 26, 29
        // — eight executions, each on time after the shift.
        let intended: Vec<Tick> = chain.iter().map(|&(_, _, i, _)| i).collect();
        assert_eq!(
            intended,
            vec![
                Tick(5),
                Tick(11),
                Tick(14),
                Tick(17),
                Tick(20),
                Tick(23),
                Tick(26),
                Tick(29)
            ],
            "the shifted chain, on time from the re-arm onward"
        );
        // Every post-shift execution is on time (the shift is paid
        // once, never compounded).
        for &(_, _, i, e) in &chain[1..] {
            assert_eq!(i, e, "on time after the honest shift (intended {i:?})");
        }
    }

    /// Test 14: witness wakes carry the census with causality.
    #[test]
    fn witness_wakes_carry_the_census_with_causality() {
        // The standard campaign (build_campaign's authored positions),
        // run past both significant events.
        //
        // The tick-20 event (center [1,0], radius 2.0): NPC 0 at
        // distance 1, NPC 1 at distance 0, NPC 2 at distance 1 — all
        // inside; NPC 7 at [0.1, 0.1] is INSIDE the radius too but
        // Dormant — its tier's budget is zero, so the census excludes
        // it (the I2/I3 invariant). Three witnesses: NPCs 0, 1, 2.
        //
        // The tick-50 event (center [10,0], radius 1.0): NPC 3 at
        // distance 0, and NPC 4 at distance EXACTLY 1.0 — inside by
        // the perception layer's PINNED inclusive boundary (1.0 ≤
        // 1.0, the boundary the machinery tests pin with the same
        // shape). Two witnesses: NPCs 3, 4.

        let mut spine = build_campaign();
        arm_campaign(&mut spine);
        spine.run_ticks(100);

        let world = spine.world();
        assert_eq!(
            world.census_witnesses, 5,
            "total census witnesses: 3 + 2 = 5"
        );

        // Witness executions: 3 from first event, 2 from second = 5
        assert_eq!(
            world.witness_executions, 5,
            "total witness executions: 3 + 2 = 5"
        );

        // NPC 0, 1, 2 should have the tick-20 trace in memory
        let npc0_has_trace = world.npcs[0]
            .memory
            .iter()
            .any(|t| t.tick == Tick(20) && t.class == 5);
        let npc1_has_trace = world.npcs[1]
            .memory
            .iter()
            .any(|t| t.tick == Tick(20) && t.class == 5);
        let npc2_has_trace = world.npcs[2]
            .memory
            .iter()
            .any(|t| t.tick == Tick(20) && t.class == 5);
        assert!(npc0_has_trace, "NPC 0 has tick-20 trace");
        assert!(npc1_has_trace, "NPC 1 has tick-20 trace");
        assert!(npc2_has_trace, "NPC 2 has tick-20 trace");

        // NPC 7 is Dormant: memory should be empty
        assert!(
            world.npcs[7].memory.is_empty(),
            "NPC 7 (Dormant) has empty memory"
        );

        // NPC 3 should have the tick-50 trace
        let npc3_has_trace = world.npcs[3]
            .memory
            .iter()
            .any(|t| t.tick == Tick(50) && t.class == 6);
        assert!(npc3_has_trace, "NPC 3 has tick-50 trace");
    }

    // -----------------------------------------------------------------
    // P2 L6 — the census-index battery at the composition seam (the
    // design's L6 acceptance row: the indexed census through
    // enumerate_witnesses's own tier composition, bit-compared
    // against the linear walk; and the full campaign through BOTH
    // census paths — the pinned execution-stream digest holds for
    // the indexed path too).
    // -----------------------------------------------------------------

    /// The seam battery's fixture population: a jittered lattice
    /// (deterministic xorshift64*, the L5/L6 battery discipline)
    /// plus hand-authored exact 3-4-5 records for the on-radius
    /// class — the exact center and the two exact distance-5
    /// records (the inclusive boundary). The NaN-position class
    /// lives in the poisoned arm below (a non-finite position
    /// refuses the whole batch's build — the fail-closed rule).
    fn census_fixture_population(seed: u64, count: usize) -> Vec<NpcRecord> {
        let mut state = seed | 1;
        let mut next = move || {
            let mut x = state;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            state = x;
            (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as u32
        };
        let mut population = Vec::with_capacity(count + 3);
        // The lattice body.
        let cols = ((count as f64).sqrt().ceil() as usize).max(1);
        for index in 0..count {
            let mut jitter =
                |lo: f32, hi: f32| lo + (hi - lo) * ((next() >> 8) as f32 / 16_777_216.0);
            population.push(NpcRecord {
                id: index as u32,
                pos: [
                    (index % cols) as f32 * 4.0 + jitter(-1.5, 1.5),
                    (index / cols) as f32 * 4.0 + jitter(-1.5, 1.5),
                ],
            });
        }
        // The known-answer tail (indices count..count+3): the exact
        // center and the two exact distance-5 records.
        population.push(NpcRecord {
            id: count as u32,
            pos: [0.0, 0.0],
        });
        population.push(NpcRecord {
            id: count as u32 + 1,
            pos: [3.0, 4.0],
        });
        population.push(NpcRecord {
            id: count as u32 + 2,
            pos: [5.0, 0.0],
        });
        population
    }

    /// Test 15 (L6): the indexed census through the SEAM —
    /// `enumerate_witnesses_indexed` vs `enumerate_witnesses` on the
    /// identical (center, radius, population, tiers), populations
    /// below and above the gate, tier patterns including Dormant
    /// records INSIDE the radius and a short `tiers` slice (the
    /// missing-entry fail-closed class), and every query class from
    /// the perception battery. The outputs must be equal as whole
    /// vectors — contents AND order.
    #[test]
    fn enumerate_witnesses_indexed_matches_linear_at_the_seam() {
        let mut scratch = CensusIndexScratch::new();
        for count in [20usize, 125, 600, 2048] {
            let population = census_fixture_population(0x5EED_0000 + count as u64, count);
            let label = format!("{count}");
            // The tier patterns: all Full; every 3rd Dormant (inside
            // the radius class — the lattice body is inside a big
            // enough radius); every 7th EventDriven.
            let all_full = vec![LodLevel::Full; population.len()];
            let mut every_third = vec![LodLevel::Full; population.len()];
            for (index, tier) in every_third.iter_mut().enumerate() {
                *tier = if index % 3 == 1 {
                    LodLevel::Dormant
                } else if index % 7 == 2 {
                    LodLevel::EventDriven
                } else {
                    LodLevel::Full
                };
            }
            // The short-slice class: a `tiers` shorter than the
            // population (the missing entries fail closed — not
            // enumerated, never an over-perception).
            let short = &all_full[..population.len() / 2];

            for tiers in [&all_full, &every_third, short] {
                let mut index_handle = scratch.build(&population).expect("grid-usable");
                for (center, radius) in [
                    ([0.0, 0.0], 5.0),      // the 3-4-5 boundary pair, inclusive
                    ([0.0, 0.0], 4.9),      // just inside (boundary excluded)
                    ([0.0, 0.0], 5.1),      // just outside (boundary included)
                    ([0.0, 0.0], 0.0),      // the exact center only
                    ([0.0, 0.0], 1e6),      // the waste-guard regime (internal fallback)
                    ([0.0, 0.0], f32::NAN), // fail-closed radius
                    ([0.0, 0.0], -1.0),     // fail-closed radius
                    ([0.0, 0.0], 1e30),     // square-overflow radius
                    ([f32::NAN, 0.0], 5.0), // NaN center (internal fallback)
                    ([4.0, 4.0], 2.0),      // a street query
                    ([8.0, 0.0], 3.0),      // a mid-lattice query
                ] {
                    let mut linear = Vec::new();
                    enumerate_witnesses(center, radius, &population, tiers, &mut linear);
                    let mut indexed = Vec::new();
                    enumerate_witnesses_indexed(
                        center,
                        radius,
                        &mut index_handle,
                        tiers,
                        &mut indexed,
                    );
                    assert_eq!(
                        indexed, linear,
                        "seam {label} at {center:?} r{radius}: identical census"
                    );
                }
            }
        }

        // THE POISONED-BATCH ARM (the fail-closed rule at the seam):
        // a NaN position refuses the whole batch's build, and the
        // composition's handler shape (the BehaviorWorld census
        // arm's own if-let-else) falls back to the exact linear
        // census — which answers identically to the pre-poison
        // population, because the poisoned record was imperceivable
        // anyway (NaN compares false; lattice record 3 sits beyond
        // the radius). Behavior never depends on the index being
        // available.
        let mut poisoned = census_fixture_population(0xDEAD_0000, 125);
        let clean = poisoned.clone();
        let poison_tiers = vec![LodLevel::Full; poisoned.len()];
        poisoned[3].pos = [f32::NAN, 0.0];
        assert!(
            scratch.build(&poisoned).is_none(),
            "a NaN position poisons the batch build (fail-closed)"
        );
        // The handler's else-arm (the refused build): the exact
        // linear census.
        let mut from_fallback = Vec::new();
        enumerate_witnesses(
            [0.0, 0.0],
            5.0,
            &poisoned,
            &poison_tiers,
            &mut from_fallback,
        );
        let mut clean_yields = Vec::new();
        enumerate_witnesses([0.0, 0.0], 5.0, &clean, &poison_tiers, &mut clean_yields);
        assert_eq!(
            from_fallback, clean_yields,
            "the poison changes nothing observable (the record was imperceivable anyway)"
        );
    }

    /// Test 16 (L6): the FULL campaign through the indexed census —
    /// the same fixed campaign, the same 200 ticks, with the world's
    /// census path switched to `enumerate_witnesses_indexed` (the
    /// per-significant-event force-build; this world's population is
    /// static, so per-event and per-tick cadences coincide). The
    /// execution-stream digest must equal the PINNED linear-path
    /// value itself (the census feeds the witness follow-ups, the
    /// memory traces, and the counters — an indexed/linear
    /// divergence anywhere moves it), the world fingerprint must be
    /// byte-identical across the two paths, and every census counter
    /// must agree. This is the L6 acceptance row's "pins that must
    /// hold" — the execution stream digest, unchanged, through the
    /// new path.
    #[test]
    fn indexed_census_campaign_matches_the_pinned_stream_digest() {
        // The linear arm (the pinned tests' own path).
        let mut linear = build_campaign();
        arm_campaign(&mut linear);
        linear.run_ticks(200);

        // The indexed arm: the same campaign, census path switched.
        let mut indexed = build_campaign();
        indexed.world_mut().use_indexed_census = true;
        arm_campaign(&mut indexed);
        indexed.run_ticks(200);

        // THE PIN (seeded_campaign_stream_digest_pin's own value,
        // the L6 requirement: the digest holds through the indexed
        // census too — unchanged, never re-pinned).
        const PINNED_STREAM_DIGEST: u64 = 10778018066104731562;
        assert_eq!(
            execution_digest(&indexed.world().stream),
            PINNED_STREAM_DIGEST,
            "the indexed census reproduces the pinned stream digest"
        );

        // Cross-path identity: stream, fingerprint, and the census
        // counters — byte-identical.
        let linear_world = linear.world();
        let indexed_world = indexed.world();
        assert_eq!(
            execution_digest(&linear_world.stream),
            execution_digest(&indexed_world.stream),
            "the two census paths produce identical streams"
        );
        let mut linear_fp = Fingerprinter::new();
        linear_world.fingerprint(&mut linear_fp);
        let mut indexed_fp = Fingerprinter::new();
        indexed_world.fingerprint(&mut indexed_fp);
        assert_eq!(
            linear_fp.finish(),
            indexed_fp.finish(),
            "the two census paths produce identical world fingerprints"
        );
        assert_eq!(
            linear_world.census_witnesses, indexed_world.census_witnesses,
            "the census witness counts agree"
        );
        assert_eq!(
            linear_world.witness_executions, indexed_world.witness_executions,
            "the witness executions agree"
        );
        assert_eq!(
            linear_world.wake_executions, indexed_world.wake_executions,
            "the wake executions agree"
        );
        // The census actually ran through the index (a vacuous pass
        // is the failure this gate exists to catch): the campaign's
        // two significant events executed with the indexed path on.
        assert!(
            indexed_world.census_witnesses == 5,
            "the campaign's 3 + 2 witnesses censused through the index"
        );
    }
}
