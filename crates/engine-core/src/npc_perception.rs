//! The deterministic perception core (the Era-2 NPC-AI design pass §5
//! — the sensing layer): the INVERTED QUERY primitive (which NPCs are
//! within a radius of a point), the tiered perception BUDGET table,
//! and the durable perception TRACE record with its canonical byte
//! encoding — ALL pure, fully tested, no I/O, no unordered iteration,
//! no new dependencies.
//!
//! # DETERMINISM CONTRACT
//! Every answer here is a pure function of (the query, the
//! population slice): same inputs ⇒ the same perceived set, in the
//! same fixed order, byte-identical on every platform. Iteration is
//! the caller's population order (a slice walk — never a `HashMap`'s;
//! §8.3's fixed-evaluation-order rule). The geometry is squared
//! f32 distances — pure IEEE `+`/`*`/comparisons, correctly rounded
//! and cross-platform deterministic for these operations, with a
//! FIXED operation order (dx, then dy, then dx² + dy²); NO sqrt is
//! taken (the `lod.rs` precedent: squared distances choose the
//! answer), so no libm exists anywhere in this module. The trace
//! encoding is `to_le_bytes` in a FIXED field order — the integer
//! form of the animation core's `to_bits` little-endian packing.
//!
//! # THE INVERTED QUERY (design §5.1)
//! Perception is event-side, not NPC-side: a durable-significant
//! event's commit step enumerates its witness set — the NPCs within
//! the event's perception radius — so cost is O(active NPCs in
//! radius) per significant event, zero for the population at large
//! (never N NPCs × M events). This module's census is the LINEAR,
//! exact form (no spatial partition exists in the simulation
//! substrate — §10.2's verified list; the measured-trigger rule
//! holds: any future index must preserve BOTH the inversion and the
//! fixed order, or determinism breaks). Tier gating is the caller's
//! composition: a dormant/far NPC is not in any active region's
//! census, so it witnesses nothing (§5.1 — the census this module
//! walks already excludes it).
//!
//! # THE POLICY BOUNDARY (design §5.2)
//! Perception classes, radii per class, and line-of-sight rules are
//! game policy. The engine ships the census mechanism (here), the
//! budget table (here), the memory-record schema (the trace, here),
//! and the retention machinery — the future packages wire them.

use crate::clock::Tick;
use crate::fingerprint::Fingerprinter;

/// One census record: the stable identity and the continuous
/// world-space position the inverted query measures against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NpcRecord {
    /// The stable NPC identity (§3's identity facet — never reused;
    /// u32 holds any plausible population with the 1000× clause's
    /// headroom). The census the caller presents is in IDENTITY
    /// order by composition discipline (§8.3) — this module's own
    /// contract is the weaker, sufficient FIXED population order.
    pub id: u32,
    /// The continuous world-space position (§4.3's position bridge:
    /// the near tier's locomotion state carries f32 positions with
    /// fixed operation order; the committed location stays coarse).
    /// A non-finite coordinate makes the record imperceivable (the
    /// fail-closed rule below — NaN can never authorize a witness).
    pub pos: [f32; 2],
}

/// The indices [`perceive`] yields — an iterator over `population`
/// positions, in fixed population order, with NO allocation (a
/// lazily advanced cursor, nothing built).
#[derive(Debug, Clone)]
pub struct PerceivedIndices<'a> {
    population: &'a [NpcRecord],
    center: [f32; 2],
    /// The squared radius, precomputed once (the fixed-operation-
    /// order discipline: `radius · radius` happens exactly once, at
    /// iterator construction — never per record).
    radius_sq: f32,
    /// Whether the radius admits any perception at all (the
    /// fail-closed rule); `false` yields nothing.
    admissible: bool,
    cursor: usize,
}

impl<'a> Iterator for PerceivedIndices<'a> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        if !self.admissible {
            return None; // The fail-closed rule: an inadmissible radius yields nothing.
        }
        while self.cursor < self.population.len() {
            let index = self.cursor;
            self.cursor += 1;
            if record_in_radius(&self.population[index], self.center, self.radius_sq) {
                return Some(index);
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.population.len() - self.cursor;
        (0, Some(remaining))
    }
}

/// Whether one record falls within the precomputed squared radius —
/// the FIXED operation order: dx = pos[0] − center[0], then
/// dy = pos[1] − center[1], then dist_sq = dx·dx + dy·dy, then the
/// single INCLUSIVE comparison `dist_sq ≤ radius_sq`. Pure IEEE f32
/// arithmetic; a non-finite dist_sq (NaN or overflow) compares false
/// — the record is imperceivable, never a panic.
fn record_in_radius(record: &NpcRecord, center: [f32; 2], radius_sq: f32) -> bool {
    let dx = record.pos[0] - center[0];
    let dy = record.pos[1] - center[1];
    let dist_sq = dx * dx + dy * dy;
    dist_sq <= radius_sq
}

/// The INVERTED QUERY: which NPCs in `population` are within
/// `radius` of `query_center` — indices into `population`, in FIXED
/// POPULATION ORDER, no allocation.
///
/// THE BOUNDARY IS INCLUSIVE (pinned): a record at squared distance
/// exactly `radius²` IS perceived (`≤`, not `<`) — an NPC standing
/// precisely on the event's perception radius witnesses it. The
/// known-answer tests pin the choice with the exact 3-4-5 diagonal
/// (25.0 = 5.0² bit-exactly in f32), so a flip to exclusive fails a
/// named test, not a witness count.
///
/// THE FAIL-CLOSED RULE: a radius that is negative, non-finite, or
/// whose square overflows to non-finite perceives NOBODY — a
/// degenerate query is a caller's validation bug, never a panic and
/// never an over-perception here (the `lod.rs` non-finite policy:
/// NaN/Infinity cannot authorize an authoritative outcome; the
/// squared-threshold finiteness check is its precedent). Likewise a
/// record at a non-finite position never perceives (its dist_sq
/// compares false).
///
/// THE ORDER RULE: yields are in population order — the caller
/// presents the census in identity order (§8.3), and whatever index
/// later accelerates the census must preserve that order (§5.1's
/// determinism constraint). Indices, not references, for the same
/// one-borrow reason [`crate::npc_schedule::due_commitments`]
/// documents.
pub fn perceive(
    query_center: [f32; 2],
    radius: f32,
    population: &[NpcRecord],
) -> PerceivedIndices<'_> {
    let admissible = radius.is_finite() && radius >= 0.0;
    let radius_sq = if admissible {
        radius * radius
    } else {
        // Any value that compares false against every finite dist_sq;
        // `admissible == false` already gates the yields, and this
        // keeps the stored square well-defined regardless.
        f32::NEG_INFINITY
    };
    let admissible = admissible && radius_sq.is_finite();
    PerceivedIndices {
        population,
        center: query_center,
        radius_sq,
        admissible,
        cursor: 0,
    }
}

impl PerceivedIndices<'_> {
    /// Whether the query admitted any perception at all (the
    /// fail-closed rule above); exposed so a caller can distinguish
    /// "radius too small" from "radius invalid" at its own policy
    /// layer — the iterator itself simply yields nothing either way.
    #[must_use]
    pub fn is_admissible(&self) -> bool {
        self.admissible
    }
}

/// The per-tier perception budget table, indexed by LOD level:
/// 0 = Full, 1 = Simplified, 2 = Event-driven, 3 = Dormant (the
/// `lod.rs` [`crate::lod::LodLevel`] ordering, most to least
/// detailed). PURE TABLE — a lookup, never a computation.
///
/// THE VALUES: Full 32, Simplified 8, Event-driven 2, Dormant 0.
/// Dormant = 0 is pinned by design §5.1 ("a dormant/far NPC is not
/// in any active region's witness census, so it witnesses nothing")
/// — a dormant NPC's perception budget is storage, nothing else.
/// The other three are [ASSUMPTION] workload guesses pending the
/// design §8.5 measurement plan (witness enumeration cost per
/// significant event); revisitable only with rig evidence, never
/// silently (the `MAX_INSTANCES` discipline). An out-of-range level
/// (anything past 3) is 0 — fail closed, the budget table's form of
/// the perception fail-closed rule.
pub const PERCEPTION_BUDGETS: [usize; 4] = [32, 8, 2, 0];

/// The perception budget at a LOD level: how many perception
/// records a tier's NPC may produce per opportunity — the cap that
/// keeps a perception storm (a riot) degrading to deferred work
/// within the lane budget, never dropped consequences (design §5.2,
/// the corrected acceptance discipline). See [`PERCEPTION_BUDGETS`]
/// for the tier mapping and each value's evidence key.
#[must_use = "a budget that is never checked caps nothing"]
pub fn perception_budget(lod_level: u8) -> usize {
    PERCEPTION_BUDGETS
        .get(usize::from(lod_level))
        .copied()
        .unwrap_or(0)
}

/// One durable perception trace — WHAT a perception durably records
/// (design §5.3's witness memory entry, the engine-schema half): the
/// observer, the perceived, the logical tick, and the perception
/// class. PURE DATA — all integers, so the libm-free discipline
/// holds trivially and the canonical encoding is exact.
///
/// The trace is the record's episodic-memory lane input; what
/// happens to it (relationship deltas, reputation candidates,
/// retention) is the future capture-ledger package's composition —
/// this module only fixes its shape and its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerceptionTrace {
    /// The perceiving (witnessing) NPC's stable identity.
    pub observer: u32,
    /// The perceived NPC's stable identity (the witnessed actor —
    /// for event-side witnesses, the NPC the significant event
    /// concerned).
    pub perceived: u32,
    /// The logical campaign tick the perception was recorded at
    /// (never wall-clock time — the `clock.rs` law).
    pub tick: Tick,
    /// The perception class: an opaque u32 game-policy tag (§5.2 —
    /// which classes are durable-significant, and which are
    /// reputation-relevant, are the owner's open decisions §9.2;
    /// the engine gives the field no meaning).
    pub class: u32,
}

/// One trace's canonical byte size: four little-endian fixed-width
/// integers — observer (4), perceived (4), tick (8), class (4).
pub const TRACE_STRIDE: usize = 20;

impl PerceptionTrace {
    /// The canonical byte encoding: `observer`, `perceived`, `tick`,
    /// `class` — each little-endian at its fixed offset (0, 4, 8,
    /// 16). THE FIXED FIELD ORDER IS THE WIRE CONTRACT: the integer
    /// form of the animation core's `to_bits` little-endian packing;
    /// a reordering is a versioned layout change, never a silent
    /// reshuffle. Byte-identical on every platform for the same
    /// trace (all fields are fixed-width integers).
    #[must_use]
    pub fn canonical_bytes(&self) -> [u8; TRACE_STRIDE] {
        let mut bytes = [0u8; TRACE_STRIDE];
        bytes[0..4].copy_from_slice(&self.observer.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.perceived.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.tick.as_u64().to_le_bytes());
        bytes[16..20].copy_from_slice(&self.class.to_le_bytes());
        bytes
    }
}

/// The FNV-1a 64 digest over a trace sequence: each trace's
/// [`PerceptionTrace::canonical_bytes`] concatenated in slice order,
/// folded through the repo's [`Fingerprinter`] constants. Fixed
/// 20-byte records make the concatenation unambiguous (no
/// length prefixes needed).
///
/// THE CROSS-PLATFORM BYTE PIN: a fixed trace sequence hashes to one
/// u64 everywhere — integer encoding only, no libm exists to vary.
/// A divergence from the pinned test value is an investigation,
/// never a silent re-pin (the frozen-pin discipline).
#[must_use = "a digest that is never compared proves nothing"]
pub fn trace_sequence_digest(traces: &[PerceptionTrace]) -> u64 {
    let mut hasher = Fingerprinter::new();
    for trace in traces.iter() {
        hasher.write_bytes(&trace.canonical_bytes());
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The five-NPC known-answer population: distances 0, 5 (the
    /// 3-4-5 diagonal — exact in f32), 5 (axis point — exact), 10,
    /// and 1 from the origin. Every coordinate is exactly
    /// representable, so every boundary case lands bit-exactly.
    fn population_of_five() -> Vec<NpcRecord> {
        vec![
            NpcRecord {
                id: 0,
                pos: [0.0, 0.0],
            }, // distance 0
            NpcRecord {
                id: 1,
                pos: [3.0, 4.0],
            }, // distance 5 (3-4-5)
            NpcRecord {
                id: 2,
                pos: [5.0, 0.0],
            }, // distance 5 (axis)
            NpcRecord {
                id: 3,
                pos: [10.0, 0.0],
            }, // distance 10
            NpcRecord {
                id: 4,
                pos: [-1.0, 0.0],
            }, // distance 1
        ]
    }

    /// KNOWN ANSWER: radii that include and exclude each of the five
    /// — full inclusion at 10, the boundary pair at 5, near-field at
    /// 2, the exact center at 0 (inclusive at distance zero).
    #[test]
    fn perceive_known_answers() {
        let population = population_of_five();
        assert_eq!(
            perceive([0.0, 0.0], 10.0, &population).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4],
            "radius 10 perceives all five"
        );
        assert_eq!(
            perceive([0.0, 0.0], 5.0, &population).collect::<Vec<_>>(),
            vec![0, 1, 2, 4],
            "radius 5 perceives all but the distance-10 record"
        );
        assert_eq!(
            perceive([0.0, 0.0], 2.0, &population).collect::<Vec<_>>(),
            vec![0, 4],
            "radius 2 perceives distances 0 and 1 only"
        );
        assert_eq!(
            perceive([0.0, 0.0], 0.0, &population).collect::<Vec<_>>(),
            vec![0],
            "radius 0 perceives the exact center only (inclusive at 0)"
        );
    }

    /// THE BOUNDARY PIN: an NPC exactly ON the radius IS perceived —
    /// the inclusive choice, pinned by name. The 3-4-5 diagonal and
    /// the axis point both sit at squared distance exactly 25.0 =
    /// 5.0² in f32, so the `≤` comparison is exercised bit-exactly;
    /// a flip to exclusive fails HERE, visibly, not as a witness
    /// count drift.
    #[test]
    fn perceive_boundary_on_radius_is_inclusive() {
        let population = population_of_five();
        let perceived: Vec<usize> = perceive([0.0, 0.0], 5.0, &population).collect();
        assert!(
            perceived.contains(&1) && perceived.contains(&2),
            "records at exactly distance 5 ARE perceived (inclusive boundary): {perceived:?}"
        );
        // Just inside and just outside stay unambiguous: 4.9 excludes
        // the boundary pair, 5.1 includes them.
        assert_eq!(
            perceive([0.0, 0.0], 4.9, &population).collect::<Vec<_>>(),
            vec![0, 4]
        );
        assert_eq!(
            perceive([0.0, 0.0], 5.1, &population).collect::<Vec<_>>(),
            vec![0, 1, 2, 4]
        );
    }

    /// THE ORDER PIN: yields follow POPULATION order, not distance
    /// order — a census authored far-near-mid yields ascending
    /// indices regardless of geometry (§5.1's ordered-iteration
    /// constraint; the identity-order discipline is the caller's
    /// composition, this module's fixedness is its foundation).
    #[test]
    fn perceive_order_is_population_order_not_distance_order() {
        let population = vec![
            NpcRecord {
                id: 7,
                pos: [10.0, 0.0],
            }, // far
            NpcRecord {
                id: 2,
                pos: [0.0, 0.0],
            }, // at center
            NpcRecord {
                id: 9,
                pos: [3.0, 4.0],
            }, // mid (distance 5)
            NpcRecord {
                id: 4,
                pos: [20.0, 0.0],
            }, // farther
        ];
        assert_eq!(
            perceive([0.0, 0.0], 6.0, &population).collect::<Vec<_>>(),
            vec![1, 2],
            "population order [1, 2], never distance order"
        );
    }

    /// THE FAIL-CLOSED PIN: a negative, NaN, infinite, or
    /// square-overflowing radius perceives NOBODY (and reports
    /// inadmissible); a record at a non-finite position is never
    /// perceived even under a huge finite radius. Never a panic,
    /// never an over-perception.
    #[test]
    fn perceive_fail_closed_on_degenerate_radii_and_positions() {
        let population = population_of_five();
        for radius in [-1.0, f32::NAN, f32::INFINITY, 1e30] {
            let mut iter = perceive([0.0, 0.0], radius, &population);
            assert!(
                iter.next().is_none(),
                "radius {radius} perceives nobody (fail closed)"
            );
            assert!(!iter.is_admissible(), "radius {radius} is inadmissible");
        }
        // The in-admissible flag distinguishes nothing-wrong from
        // invalid: a small valid radius is admissible, just empty.
        let quiet = perceive([0.0, 0.0], 0.25, &population);
        assert!(quiet.is_admissible());
        assert_eq!(quiet.collect::<Vec<_>>(), vec![0]);

        // A non-finite position never perceives (NaN compares false;
        // the rule is the comparison's own, not a special case).
        let poisoned = vec![
            NpcRecord {
                id: 0,
                pos: [0.0, 0.0],
            },
            NpcRecord {
                id: 1,
                pos: [f32::NAN, 0.0],
            },
            NpcRecord {
                id: 2,
                pos: [f32::INFINITY, 0.0],
            },
        ];
        assert_eq!(
            perceive([0.0, 0.0], 1e10, &poisoned).collect::<Vec<_>>(),
            vec![0],
            "non-finite positions are imperceivable even at a huge radius"
        );
    }

    /// The query is pure: same inputs, same yields, any number of
    /// runs (and the empty population is a valid, empty census).
    #[test]
    fn perceive_is_pure_and_empty_population_is_quiet() {
        let population = population_of_five();
        let first: Vec<usize> = perceive([1.0, 0.0], 3.0, &population).collect();
        for _ in 0..3 {
            assert_eq!(
                perceive([1.0, 0.0], 3.0, &population).collect::<Vec<_>>(),
                first,
                "byte-identical yields across runs"
            );
        }
        // Center shifted to (1, 0): distances 1, sqrt(20)≈4.47, 4,
        // 9, 2 — radius 3 perceives indices 0 and 4 only.
        assert_eq!(first, vec![0, 4]);
        assert!(
            perceive([0.0, 0.0], 5.0, &[]).next().is_none(),
            "the empty census perceives nobody"
        );
    }

    /// THE BUDGET TABLE: the four tier values, and the fail-closed
    /// out-of-range level (anything past Dormant is 0).
    #[test]
    fn perception_budget_table_values() {
        assert_eq!(perception_budget(0), 32, "Full");
        assert_eq!(perception_budget(1), 8, "Simplified");
        assert_eq!(perception_budget(2), 2, "Event-driven");
        assert_eq!(perception_budget(3), 0, "Dormant — §5.1's pinned zero");
        assert_eq!(PERCEPTION_BUDGETS, [32, 8, 2, 0]);
        for out_of_range in [4u8, 100, 255] {
            assert_eq!(
                perception_budget(out_of_range),
                0,
                "level {out_of_range} fails closed"
            );
        }
    }

    /// THE BYTE-LAYOUT PIN: a trace with every byte distinct encodes
    /// to exactly these 20 little-endian bytes — observer at 0..4,
    /// perceived at 4..8, tick at 8..16 (little-endian u64: the
    /// 0x…88 byte lands FIRST), class at 16..20. A moved byte is an
    /// investigation (a layout change is versioned, never silent).
    #[test]
    fn trace_canonical_byte_layout_pinned() {
        let trace = PerceptionTrace {
            observer: 0x0302_0100,
            perceived: 0x0706_0504,
            tick: Tick(0x1122_3344_5566_7788),
            class: 0x0A0B_0C0D,
        };
        let expected: [u8; TRACE_STRIDE] = [
            // observer 0x03020100, little-endian.
            0x00, 0x01, 0x02, 0x03, // perceived 0x07060504, little-endian.
            0x04, 0x05, 0x06, 0x07, // tick 0x1122334455667788, little-endian.
            0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11,
            // class 0x0A0B0C0D, little-endian.
            0x0D, 0x0C, 0x0B, 0x0A,
        ];
        assert_eq!(trace.canonical_bytes(), expected);
        assert_eq!(trace.canonical_bytes().len(), TRACE_STRIDE);
    }

    /// The digest fixture: three traces spanning the field ranges —
    /// a distinct-byte trace, a small-values trace, and a zero-tick
    /// maximal-class trace (enough shape to make a field-order or
    /// encoding bug visible in the digest).
    fn trace_fixture() -> Vec<PerceptionTrace> {
        vec![
            PerceptionTrace {
                observer: 3,
                perceived: 11,
                tick: Tick(0x1122_3344_5566_7788),
                class: 0x0A0B_0C0D,
            },
            PerceptionTrace {
                observer: 11,
                perceived: 3,
                tick: Tick(99),
                class: 1,
            },
            PerceptionTrace {
                observer: 0,
                perceived: 999,
                tick: Tick(0),
                class: u32::MAX,
            },
        ]
    }

    /// THE DIGEST PIN: the fixture sequence hashes to this FNV-1a 64
    /// value on every platform — fixed-width integer encoding in the
    /// fixed field order, so there is nothing to vary. A moved value
    /// is an investigation (an encoding or order change), never a
    /// silent re-pin.
    #[test]
    fn trace_sequence_digest_pin() {
        assert_eq!(trace_sequence_digest(&trace_fixture()), PINNED_TRACE_DIGEST);
    }

    /// The measured-actual FNV-1a of the fixture trace sequence.
    const PINNED_TRACE_DIGEST: u64 = 10_766_371_085_850_726_021;

    /// The digest is a pure function of the sequence: same traces ⇒
    /// same digest; the empty sequence ⇒ the FNV offset basis; a
    /// reordered sequence differs (the order IS the content — §5.1's
    /// ordered-iteration constraint made assertable).
    #[test]
    fn trace_sequence_digest_is_pure_and_order_sensitive() {
        let traces = trace_fixture();
        assert_eq!(
            trace_sequence_digest(&traces),
            trace_sequence_digest(&trace_fixture()),
            "same sequence, same digest"
        );
        assert_eq!(
            trace_sequence_digest(&[]),
            Fingerprinter::new().finish(),
            "the empty sequence is the offset basis"
        );
        let mut reordered = trace_fixture();
        reordered.swap(0, 1);
        assert_ne!(
            trace_sequence_digest(&reordered),
            trace_sequence_digest(&traces),
            "trace order is significant"
        );
    }
}
