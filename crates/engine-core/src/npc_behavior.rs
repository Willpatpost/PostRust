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
//! RNG stream in fixed order, and the identity's wire encoding
//! ([`NpcIdentity::wire_bytes`]) is little-endian fixed-width
//! integers in a fixed field order — the codec this module owns,
//! pinned by an asymmetric golden record and round trip (finding 7's
//! correction: the claim now names the bytes actually exercised). A
//! fixed rule table over a fixed record produces the same activity
//! sequence forever — pinned by digest.
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
//! shape future behavior and survive dormancy — wire-encodable
//! through this module's own codec ([`NpcIdentity::wire_bytes`] /
//! [`NpcIdentity::from_wire`]), fingerprintable, byte-exact through
//! save/load. `NpcRoutine` is the RECONSTRUCTIBLE lane: the current
//! activity, needs, route progress — captured on deactivation,
//! derived on activation, never persisted as durable truth. The two
//! are deliberately separate types so the commit boundary never
//! blurs.

use std::cmp::Ordering;

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
/// the commit pipeline). Wire-encodable through THIS module's codec
/// — [`NpcIdentity::wire_bytes`]/[`NpcIdentity::from_wire`], the
/// seam the docs claim, byte-exact through save/load (pinned by an
/// asymmetric golden record and round trip).
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

impl NpcIdentity {
    /// The wire stride: id (u32), home_cell (2 × u8), region (u8),
    /// and appearance_seed (u32) — 11 bytes total.
    pub const WIRE_STRIDE: usize = 11;

    /// THE WIRE ENCODING — the actual serialization seam (finding
    /// 7's correction: the durable lane's docs claimed a fixed wire
    /// order and byte-exact save/load, but the reviewed tree had no
    /// encoder for the type; this IS the encoder, and the golden
    /// record below pins it). Fixed field order, little-endian
    /// fixed-width integers: id (u32 LE), home_cell.0 (u8),
    /// home_cell.1 (u8), region (u8), appearance_seed (u32 LE).
    /// Total: every call site that persists an identity goes through
    /// these bytes.
    pub fn wire_bytes(&self) -> [u8; Self::WIRE_STRIDE] {
        let mut bytes = [0u8; Self::WIRE_STRIDE];
        bytes[0..4].copy_from_slice(&self.id.0.to_le_bytes());
        bytes[4] = self.home_cell.0;
        bytes[5] = self.home_cell.1;
        bytes[6] = self.region;
        bytes[7..11].copy_from_slice(&self.appearance_seed.to_le_bytes());
        bytes
    }

    /// Decodes the wire encoding — fail-closed and ASYMMETRIC
    /// against [`Self::wire_bytes`]: encode is total, decode refuses
    /// anything but the exact stride (a truncated or extended record
    /// never decodes; every well-strided byte pattern decodes, so
    /// the golden record plus the no-ignored-bytes property below
    /// are the pin's whole truth).
    pub fn from_wire(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::WIRE_STRIDE {
            return None;
        }
        Some(Self {
            id: NpcId(u32::from_le_bytes(bytes[0..4].try_into().ok()?)),
            home_cell: (bytes[4], bytes[5]),
            region: bytes[6],
            appearance_seed: u32::from_le_bytes(bytes[7..11].try_into().ok()?),
        })
    }
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

/// The per-rule witness matcher (finding 6's minimal policy
/// predicate): the perception class this rule REACTS to. A rule
/// carrying a matcher applies ONLY while the view holds a live
/// perception of that class — so a policy can distinguish an empty
/// observation from witnessing the class — and when such a rule
/// WINS, the FIRST matching perception's subject (in the view's
/// fixed order) is bound into the selected activity's `React`
/// subject slot (the observed-subject store's single-slot form:
/// `React { subject }` is the only activity kind that names an NPC,
/// so it is the only slot to bind; other kinds use the matcher as a
/// pure applicability gate). The authored `React { subject }` field
/// is the never-reached fallback for hand-driven selections outside
/// the matcher path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WitnessMatcher {
    /// The perception class that must be fired this tick for the
    /// rule to apply.
    pub class: u32,
}

/// The rule table entry (POLICY data — the engine only defines the
/// evaluation). A rule names the activity it selects, the need it
/// serves (or none), its base priority, the minimum need level
/// below which it applies, and optionally the perception class it
/// reacts to (the witness matcher above).
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
    /// The optional witness matcher (None = the rule ignores the
    /// perceptions, the pre-matcher behavior exactly).
    pub witness: Option<WitnessMatcher>,
}

impl Rule {
    /// The served need's current level — fail-closed on an
    /// out-of-range index (finding 6's defense-in-depth: admission
    /// refuses such tables via [`validate_rule_table`], and the
    /// selector treats an unvalidated out-of-range rule as
    /// inapplicable — never a panic in either selection pass).
    fn need_level(&self, routine: &NpcRoutine) -> Option<f32> {
        self.need
            .and_then(|need| routine.needs.get(need as usize).copied())
    }

    /// Whether the rule applies: the need gate (a served need below
    /// `applies_below`) AND the witness gate (a live perception of
    /// the matcher's class, when the rule carries one).
    fn applicable(&self, routine: &NpcRoutine, view: &WorldView) -> bool {
        if let Some(level) = self.need_level(routine) {
            // The need gate — the original comparison direction
            // verbatim, decided through `partial_cmp` so a NaN level
            // (no ordering) never passes it.
            if !matches!(level.partial_cmp(&self.applies_below), Some(Ordering::Less)) {
                return false;
            }
        }
        // Out-of-range need index: fail closed, never a panic.
        if self.need.is_some() && self.need_level(routine).is_none() {
            return false;
        }
        if let Some(matcher) = self.witness
            && !view_live(view)
                .iter()
                .any(|&(class, _)| class == matcher.class)
        {
            return false;
        }
        true
    }

    /// The urgency term (1 − served need level; 0 unconditional).
    fn urgency(&self, routine: &NpcRoutine) -> f32 {
        match self.need_level(routine) {
            Some(level) => 1.0 - level,
            None => 0.0,
        }
    }

    /// The observed subject of the FIRST live perception matching
    /// the rule's witness class (the binding source; None when the
    /// rule carries no matcher or the class is not fired).
    fn matched_subject(&self, view: &WorldView) -> Option<NpcId> {
        let matcher = self.witness?;
        view_live(view)
            .iter()
            .find(|&&(class, _)| class == matcher.class)
            .map(|&(_, subject)| subject)
    }
}

/// The view's live perceptions, clamped to the fixed bound (a
/// policy-authored `perception_count` above
/// [`MAX_PERCEPTIONS`] reads only the array's own extent — fail
/// closed, never an out-of-bounds index).
fn view_live(view: &WorldView) -> &[(u32, NpcId)] {
    let count = usize::from(view.perception_count).min(MAX_PERCEPTIONS);
    &view.perceptions[..count]
}

/// Validates a rule table at ADMISSION (finding 6's correction: a
/// policy-authored out-of-range need index used to panic inside the
/// selector's both passes; it is now refused here — typed,
/// fail-visible — before any evaluation trusts the table, and the
/// selector additionally fails closed per-rule so an unvalidated
/// table can never crash, only mis-serve). Refusals are
/// `&'static str` reasons (the npc_memory validation-refusal
/// style).
pub fn validate_rule_table(rules: &[Rule]) -> Result<(), &'static str> {
    for rule in rules {
        if let Some(need) = rule.need
            && need as usize >= MAX_NEEDS
        {
            return Err("npc/rule-need-index-out-of-range");
        }
    }
    Ok(())
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

    /// The stream's current position, as bits (finding 7's
    /// diagnostic-digest input: two worlds whose NPCs have drawn
    /// different amounts of tie-break entropy are worlds whose
    /// FUTURE behavior differs — a read-only observation, never a
    /// restoration seam).
    pub fn state_bits(&self) -> u64 {
        self.state
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
/// its `applies_below` level AND its witness matcher (if any) has a
/// live perception of its class; among applicable rules the SCORE is
/// base_priority + the served need's URGENCY (1 − need level, the
/// hungrier the stronger); the highest score wins; ties break by
/// the RNG stream drawing among the tied rules' indices in table
/// order. A due commitment always overrides the table (the
/// schedule's authority, §6's precedence). A fired perception of a
/// REACTIVE class is offered to the policy's rules like any other
/// input — the engine imposes no fear semantics — and a rule with a
/// witness matcher BINDS the matched perception's subject into its
/// `React` activity when it wins (finding 6: the perception can now
/// both gate a rule and name whom the reaction concerns).
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
    // precedence stands, and it stands OVER witness rules too: a due
    // commitment short-circuits before the table is ever walked.)
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
        if !rule.applicable(routine, view) {
            continue;
        }
        let score = f32::from(rule.base_priority) + rule.urgency(routine);
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
            if !rule.applicable(routine, view) {
                continue;
            }
            let score = f32::from(rule.base_priority) + rule.urgency(routine);
            if score == best_score {
                if seen == pick {
                    return witness_bound(rule, view, index as u32);
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
    witness_bound(&rules[best_index as usize], view, best_index)
}

/// The winning rule's activity, with the witness matcher's observed
/// subject bound into the `React` slot when the rule carries a
/// matcher and its class is fired (the matcher gates applicability,
/// so a winning matched rule always has a subject to bind; other
/// activity kinds keep their authored parameters — the matcher
/// served as a pure gate).
fn witness_bound(rule: &Rule, view: &WorldView, index: u32) -> Activity {
    let mut kind = rule.activity;
    if let Some(observed) = rule.matched_subject(view)
        && let ActivityKind::React { class, .. } = kind
    {
        kind = ActivityKind::React {
            class,
            subject: observed,
        };
    }
    Activity { kind, rule: index }
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

    fn identity(id: u32) -> NpcIdentity {
        NpcIdentity {
            id: NpcId(id),
            home_cell: (0, 0),
            region: 0,
            appearance_seed: id,
        }
    }

    /// A view carrying the given live perceptions (the test-side
    /// view author; fixed order, bounded).
    fn view_of(perceptions: &[(u32, NpcId)]) -> WorldView {
        let mut view = WorldView::default();
        for (i, &(class, subject)) in perceptions.iter().take(MAX_PERCEPTIONS).enumerate() {
            view.perceptions[i] = (class, subject);
        }
        view.perception_count = perceptions.len().min(MAX_PERCEPTIONS) as u8;
        view
    }

    /// THE WITNESS MATCHER (finding 6's deciding tests): hold needs,
    /// seed, and table fixed; change ONLY the fired class or subject
    /// — the authored reaction must change.
    #[test]
    fn witness_matcher_changes_the_reaction() {
        let rules = [
            Rule {
                activity: ActivityKind::Idle,
                need: None,
                base_priority: 5,
                applies_below: 1.0,
                witness: None,
            },
            Rule {
                // The authored subject is the never-reached fallback
                // (the matcher gates applicability, so a winning
                // matched rule always binds an observed subject).
                activity: ActivityKind::React {
                    class: 5,
                    subject: NpcId(0),
                },
                need: None,
                base_priority: 50,
                applies_below: 1.0,
                witness: Some(WitnessMatcher { class: 5 }),
            },
        ];
        let r = routine([1.0; MAX_NEEDS]);
        // EMPTY observation: the matcher gate holds the React rule
        // out — the idle baseline wins (a policy CAN distinguish an
        // empty observation from witnessing the class).
        let mut rng = NpcRng::for_npc(NpcId(11));
        let empty = select_activity(&r, &WorldView::default(), &rules, &mut rng);
        assert_eq!(empty.kind, ActivityKind::Idle);
        // The class fires: the React rule applies and wins, BINDING
        // the observed subject (never the authored fallback).
        let mut rng = NpcRng::for_npc(NpcId(11));
        let reacted = select_activity(&r, &view_of(&[(5, NpcId(42))]), &rules, &mut rng);
        assert_eq!(
            reacted.kind,
            ActivityKind::React {
                class: 5,
                subject: NpcId(42)
            },
            "the observed subject is bound into the reaction"
        );
        // ONLY the subject changes: the binding follows.
        let mut rng = NpcRng::for_npc(NpcId(11));
        let reacted_b = select_activity(&r, &view_of(&[(5, NpcId(7))]), &rules, &mut rng);
        assert_eq!(
            reacted_b.kind,
            ActivityKind::React {
                class: 5,
                subject: NpcId(7)
            },
            "a different observed subject changes the authored reaction"
        );
        // ONLY the class changes (subject held): a different class
        // firing does NOT open the gate — the matcher matches its
        // own class, never any perception.
        let mut rng = NpcRng::for_npc(NpcId(11));
        let other = select_activity(&r, &view_of(&[(6, NpcId(42))]), &rules, &mut rng);
        assert_eq!(other.kind, ActivityKind::Idle);
        // SCHEDULE PRECEDENCE STANDS over witness rules: a due
        // commitment short-circuits before the table is walked.
        let mut view = view_of(&[(5, NpcId(42))]);
        view.due_commitment = Some(7);
        let mut rng = NpcRng::for_npc(NpcId(11));
        let pinned = select_activity(&r, &view, &rules, &mut rng);
        assert_eq!(
            pinned.kind,
            ActivityKind::Fulfill { commitment: 7 },
            "the schedule lane still wins over a firing witness rule"
        );
    }

    /// THE ADMISSION REFUSAL (finding 6): an out-of-range need index
    /// is refused at rule-table admission — typed, counted — and the
    /// selector itself fails closed (never a panic) on unvalidated
    /// input.
    #[test]
    fn out_of_range_need_indices_are_refused_at_admission() {
        let mut rules = [Rule {
            activity: ActivityKind::Idle,
            need: Some(MAX_NEEDS as u8),
            base_priority: 5,
            applies_below: 1.0,
            witness: None,
        }];
        // Typed: the refusal names its reason.
        assert_eq!(
            validate_rule_table(&rules),
            Err("npc/rule-need-index-out-of-range")
        );
        // Counted: every out-of-range shape refuses (the bound, and
        // far past it).
        for bad in [MAX_NEEDS as u8, u8::MAX] {
            rules[0].need = Some(bad);
            assert_eq!(
                validate_rule_table(&rules),
                Err("npc/rule-need-index-out-of-range"),
                "need {bad} refused at admission"
            );
        }
        // The selector never panics on the unvalidated table: the
        // offending rule cannot apply (fail closed) and the idle
        // floor answers.
        rules[0].need = Some(u8::MAX);
        let mut rng = NpcRng::for_npc(NpcId(12));
        let chosen = select_activity(
            &routine([0.0; MAX_NEEDS]),
            &WorldView::default(),
            &rules,
            &mut rng,
        );
        assert_eq!(
            chosen.kind,
            ActivityKind::Idle,
            "an out-of-range need index fails closed, never a panic"
        );
        // A valid table passes admission.
        rules[0].need = Some(0);
        assert_eq!(validate_rule_table(&rules), Ok(()));
        // The unconditional and matcher-carrying shapes pass too.
        rules[0].need = None;
        rules[0].witness = Some(WitnessMatcher { class: 1 });
        assert_eq!(validate_rule_table(&rules), Ok(()));
    }

    /// THE IDENTITY WIRE SEAM (finding 7's correction): the durable
    /// lane's docs claimed a fixed wire order and byte-exact
    /// save/load — this pins the codec that now exists. The golden
    /// record is ASYMMETRIC: encode is total, decode refuses
    /// anything but the exact stride, and every wire byte is
    /// meaning-bearing (no ignored padding a perturbation could
    /// hide in).
    #[test]
    fn identity_wire_golden_record_and_asymmetric_round_trip() {
        let sample = NpcIdentity {
            id: NpcId(0x0807_0605),
            home_cell: (0x0B, 0x0A),
            region: 0x0C,
            appearance_seed: 0x100F_0E0D,
        };
        #[rustfmt::skip]
        const GOLDEN: [u8; NpcIdentity::WIRE_STRIDE] = [
            0x05, 0x06, 0x07, 0x08, // id (u32 LE)
            0x0B, 0x0A,             // home_cell (x, z)
            0x0C,                   // region
            0x0D, 0x0E, 0x0F, 0x10, // appearance_seed (u32 LE)
        ];
        assert_eq!(sample.wire_bytes(), GOLDEN, "the golden record");
        assert_eq!(
            NpcIdentity::from_wire(&GOLDEN),
            Some(sample),
            "decode(encode(x)) == x"
        );
        // ASYMMETRY: decode refuses anything but the exact stride —
        // a perturbed length never decodes.
        assert_eq!(
            NpcIdentity::from_wire(&GOLDEN[..10]),
            None,
            "a truncated record is refused"
        );
        assert_eq!(
            NpcIdentity::from_wire(&[GOLDEN.as_slice(), &[0u8]].concat()),
            None,
            "an extended record is refused"
        );
        // Decode of perturbed BYTES (stride intact) never yields the
        // original identity: no byte is ignored, so a flipped bit
        // cannot round-trip silently.
        for i in 0..GOLDEN.len() {
            let mut perturbed = GOLDEN;
            perturbed[i] ^= 1;
            let decoded =
                NpcIdentity::from_wire(&perturbed).expect("the stride still holds, so it decodes");
            assert_ne!(
                decoded, sample,
                "byte {i} is meaning-bearing: a perturbation cannot decode to the \
                 original record"
            );
        }
        // The once-unused test helper now round-trips through the
        // seam it exists for.
        assert_eq!(
            NpcIdentity::from_wire(&identity(77).wire_bytes()),
            Some(identity(77))
        );
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
            witness: None,
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
                witness: None,
            },
            Rule {
                activity: ActivityKind::Idle,
                need: Some(1),
                base_priority: 10,
                applies_below: 0.9,
                witness: None,
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
            witness: None,
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
                witness: None,
            },
            Rule {
                activity: ActivityKind::MoveTo {
                    route: 5,
                    waypoint: 5,
                },
                need: None,
                base_priority: 10,
                applies_below: 1.0,
                witness: None,
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
            Rule {
                activity: ActivityKind::React {
                    class: 4,
                    subject: NpcId(9),
                },
                need: Some(3),
                base_priority: 8,
                applies_below: 0.6,
                witness: None,
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
