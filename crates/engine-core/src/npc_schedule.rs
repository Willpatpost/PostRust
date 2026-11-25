//! The deterministic schedule core (the Era-2 NPC-AI design pass §6 —
//! the calendar layer): authored recurring and one-off commitments as
//! PURE DATA, the fixed-order due selection, and the fast-forward
//! next-due query — ALL pure, fully tested, no clocks, no I/O, no
//! unordered iteration, no new dependencies.
//!
//! # DETERMINISM CONTRACT
//! Every answer here is a pure function of (the authored schedule,
//! the logical [`Tick`]): same data + same tick ⇒ the same due set,
//! in the same fixed order, byte-identical on every platform. A
//! schedule holds no clocks (all times are spine ticks — wall-clock
//! time never enters, the `clock.rs` law), no RNG, no I/O; iteration
//! runs over the authored `Vec` order — never a `HashMap`'s (the
//! design pass §8.3 fixed-evaluation-order rule, the fingerprint
//! rule P4). The module contains no floats at all: the due math is
//! integer u64 arithmetic with checked overflow (an unrepresentable
//! next occurrence is `None`, never a wraparound and never a panic —
//! the `clock.rs` saturation framing).
//!
//! # THE AUTHORING BOUNDARY (design §6.1)
//! A schedule is authored data in the clip table's discipline:
//! a list of commitments whose fields are all plain integers —
//! versioned, wire-encodable (the record's representation
//! discipline, §3; the byte encoding itself belongs to the future
//! wire package, where every field here is a fixed-width integer
//! load). The game ships the content (which NPC does what, when,
//! where); the engine ships the query machinery — this module.
//!
//! # THE EXECUTION BOUNDARY (design §6.2/§6.3)
//! This module answers WHEN commitments are due. Execution is the
//! spine's: a due commitment becomes a wake event in the NPC's region
//! lane (the existing `Schedule::AtLogicalTime` machinery), the
//! selector turns it into an activity, and a time-skip is the queue
//! fast-forward over exactly the ticks [`next_due`] steps through.
//! That wiring — and the lane classification's Gate Keeper
//! composition — belongs to the future behavior package; nothing
//! here schedules, executes, or persists. The pure calendar queries
//! only.

use std::num::NonZeroU64;

use crate::clock::Tick;
use crate::event::Field;
use crate::fingerprint::Fingerprinter;

/// One authored commitment: when it fires, how it repeats, what it
/// does, and whom/where it drives. PURE DATA — no behavior, no
/// clocks; the spine fires it, the selector (the future §4 package)
/// interprets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Commitment {
    /// The first tick the commitment is due at. For a recurring
    /// cadence the occurrences are `start + k · period` (k = 0, 1,
    /// 2, …); for a one-shot, exactly `start`.
    pub start: Tick,
    /// When the commitment repeats.
    pub cadence: Cadence,
    /// What the commitment does when it fires.
    pub activity: ActivityDescriptor,
    /// Whom and where the commitment drives.
    pub target: Target,
}

impl Commitment {
    /// Whether this commitment is due at exactly `tick`.
    ///
    /// Recurring (`start + k·period`, `k ≥ 0`): due iff
    /// `tick ≥ start` and `tick − start` is a multiple of the
    /// period — the
    /// occurrence grid is anchored at `start`, so two commitments
    /// with the same period but different starts never silently
    /// align (their synchrony is authored, visible data — the §6.2
    /// measured hazard this module keeps explicit).
    ///
    /// One-shot: due iff `tick == start` EXACTLY. The choice is
    /// pinned: a one-shot's wake is a single logical moment (a
    /// single spine event at its tick); every such tick is reachable
    /// through [`next_due`], so exactness loses nothing under
    /// fast-forward and keeps the query a pure function of the tick
    /// (no "already fired" state exists here — execution state is
    /// the spine's, never the calendar's).
    fn is_due_at(&self, tick: Tick) -> bool {
        match self.cadence {
            Cadence::OneShot => tick == self.start,
            Cadence::Every { period_ticks } => {
                tick.as_u64() >= self.start.as_u64()
                    && (tick.as_u64() - self.start.as_u64()).is_multiple_of(period_ticks.get())
            }
        }
    }

    /// The next occurrence STRICTLY AFTER `tick` — the per-entry
    /// re-arm anchor the execution layer chains wake events on
    /// (design §6.2: the wake's follow-up chain). Checked
    /// arithmetic throughout: `None` when no further occurrence is
    /// representable (a one-shot past its start; an occurrence
    /// beyond the u64 tick space — the `clock.rs` saturation
    /// framing, here as absence rather than a frozen clock).
    ///
    /// Recurring (`start + k·period`, `k ≥ 0`): the next multiple of
    /// the period past `tick`, anchored at `start`. One-shot:
    /// `None` once `tick` has reached `start`.
    pub fn next_occurrence(&self, tick: Tick) -> Option<Tick> {
        if tick.as_u64() < self.start.as_u64() {
            // Both cadences fire at `start` before anything later.
            return Some(self.start);
        }
        match self.cadence {
            Cadence::OneShot => None, // `start` already reached or passed.
            Cadence::Every { period_ticks } => {
                let period = period_ticks.get();
                let elapsed = tick.as_u64() - self.start.as_u64();
                // Occurrences sit at k·period after start; the first
                // strictly after `tick` needs k·period > elapsed.
                let k = (elapsed / period).checked_add(1)?;
                let offset = k.checked_mul(period)?;
                Some(Tick(self.start.as_u64().checked_add(offset)?))
            }
        }
    }
}

/// When a commitment repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cadence {
    /// Fires exactly once, at [`Commitment::start`] — the committed
    /// one-off (the durable lane, §6.1).
    OneShot,
    /// Fires at [`Commitment::start`] and every `period_ticks`
    /// thereafter, forever (the recurring day-pattern, §6.1). The
    /// period is a [`NonZeroU64`] so a zero period is UNREPRESENTABLE:
    /// the due math divides by it, and totality by construction is
    /// stronger than a runtime check (no panic path exists to test).
    Every {
        /// The period in ticks, always ≥ 1 by type.
        period_ticks: NonZeroU64,
    },
}

impl Cadence {
    /// The recurring cadence — the typed zero refusal: a zero period
    /// is `None` (never a panic, never a silently-every-tick
    /// commitment), matching the typed-refusal discipline the
    /// renderer's packers pin.
    pub fn every(period_ticks: u64) -> Option<Self> {
        NonZeroU64::new(period_ticks).map(|period_ticks| Cadence::Every { period_ticks })
    }

    /// The one-shot cadence.
    pub const fn one_shot() -> Self {
        Cadence::OneShot
    }
}

/// What the commitment does when it fires — pure descriptor data the
/// future selector consumes; this module never interprets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivityDescriptor {
    /// The activity kind: an opaque game-policy tag (the
    /// [`crate::event::EventClass`] shape — the engine gives it no
    /// meaning; the game's rule table names its own kinds).
    pub kind: u16,
    /// The execution-lane classification, fixed AT AUTHORING TIME —
    /// design §6.3's rule that "the schedule layer must classify
    /// commitments by lane as data at authoring time, so the future
    /// Gate Keeper has no runtime classification to invent". Near
    /// commitments ride the guaranteed near reservation (the §6.2
    /// synchrony absorbent); far commitments the far lane.
    pub field: Field,
}

/// Whom and where the commitment drives. PURE DATA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    /// The stable NPC identity the commitment belongs to (§3's
    /// identity facet — never reused; u32 holds any plausible
    /// population with the 1000× clause's headroom).
    pub npc: u32,
    /// The location anchor: a region-local cell in the same
    /// `(u8, u8)` shape the campaign workload's committed positions
    /// use (§4.3's coarse committed location — continuous precision
    /// is the near tier's reconstructible state, never the
    /// calendar's). The grid's semantics are game policy.
    pub anchor: (u8, u8),
}

/// A schedule: an ordered list of commitments. PURE DATA — no
/// clocks, no I/O, no code at authoring time (§6.1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NpcSchedule {
    /// The commitments in AUTHORED order. That order is the FIXED
    /// iteration order every query preserves (multiple commitments
    /// due at the same tick yield in ascending index order — the
    /// known-answer tests pin it); authoring reorders it, nothing
    /// at runtime ever does.
    pub entries: Vec<Commitment>,
}

impl NpcSchedule {
    /// The number of commitment entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the schedule has no entries (an empty schedule is due
    /// at no tick and never fast-forwards — the known-answer pin).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The indices [`due_commitments`] yields — an iterator over
/// `schedule.entries` positions, in fixed authored order, with NO
/// allocation (a lazily advanced cursor, nothing built).
#[derive(Debug, Clone)]
pub struct DueIndices<'a> {
    schedule: &'a NpcSchedule,
    tick: Tick,
    cursor: usize,
}

impl<'a> Iterator for DueIndices<'a> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        while self.cursor < self.schedule.entries.len() {
            let index = self.cursor;
            self.cursor += 1;
            if self.schedule.entries[index].is_due_at(self.tick) {
                return Some(index);
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.schedule.entries.len() - self.cursor;
        (0, Some(remaining))
    }
}

/// Selects the commitments due at `tick`, as INDICES into
/// `schedule.entries` — never the commitments themselves copied out,
/// never an allocation (a cursor over the authored order).
///
/// FIXED ORDER: ascending entry index (the authored order), so
/// multiple commitments due at the same tick yield in a stable,
/// cross-platform sequence — the §8.3 fixed-evaluation-order rule.
/// A schedule with no entry due at `tick` yields nothing.
///
/// The indices (not references) are the contract because the caller
/// consumes them against the same immutable schedule slice — one
/// borrow, no lifetime juggling, and the due SET is fully determined
/// by (schedule, tick) alone.
pub fn due_commitments(schedule: &NpcSchedule, tick: Tick) -> DueIndices<'_> {
    DueIndices {
        schedule,
        tick,
        cursor: 0,
    }
}

/// The next tick ANY commitment in the schedule comes due, strictly
/// after `tick` — the fast-forward primitive (design §6.3: skip cost
/// tracks scheduled events, so the skip advances due-tick to
/// due-tick, never scanwise through quiet ticks).
///
/// `None` when no due tick remains: the empty schedule, and
/// schedules whose every next occurrence overflows the u64 tick
/// space (checked arithmetic — the `clock.rs` saturation framing).
/// The fold runs in authored entry order (the fixed-operation-order
/// discipline; the minimum itself is order-invariant, but the walk
/// is not left to chance).
#[must_use = "a next-due that is never advanced fast-forwards nothing"]
pub fn next_due(schedule: &NpcSchedule, tick: Tick) -> Option<Tick> {
    let mut best: Option<Tick> = None;
    for entry in schedule.entries.iter() {
        if let Some(candidate) = entry.next_occurrence(tick) {
            best = Some(match best {
                None => candidate,
                Some(current) if candidate < current => candidate,
                Some(current) => current,
            });
        }
    }
    best
}

/// The FNV-1a 64 digest over a SAMPLED DUE-SEQUENCE: for every tick
/// `t` in `0..tick_span`, for every due index `i` (in [`due_commitments`]
/// order), the pair (`t` as u64, `i` as u32), little-endian, folded
/// through the repo's [`Fingerprinter`] constants. Fixed field widths
/// make the pair encoding unambiguous (no length prefixes needed).
///
/// THE CROSS-PLATFORM BYTE PIN: a fixed authored schedule's due
/// sequence hashes to one u64 everywhere (integer arithmetic only —
/// no libm exists to vary). A divergence from the pinned test value
/// is an investigation, never a silent re-pin (the frozen-pin
/// discipline). Two schedules with the same due sequence digest
/// equal BY CONSTRUCTION — the digest pins the sequence, not the
/// authored fields.
#[must_use = "a digest that is never compared proves nothing"]
pub fn due_sequence_digest(schedule: &NpcSchedule, tick_span: u64) -> u64 {
    let mut hasher = Fingerprinter::new();
    for tick in 0..tick_span {
        for index in due_commitments(schedule, Tick(tick)) {
            hasher.write_u64(tick);
            hasher.write_u32(index as u32);
        }
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The commitment constructor the fixtures share (default
    /// descriptor/target fields — the due math never reads them).
    fn commitment(start: Tick, cadence: Cadence, npc: u32) -> Commitment {
        Commitment {
            start,
            cadence,
            activity: ActivityDescriptor {
                kind: 1,
                field: Field::Near,
            },
            target: Target {
                npc,
                anchor: (0, 0),
            },
        }
    }

    /// KNOWN ANSWER: a recurring commitment at period 10 from tick 0
    /// is due at 0, 10, 20, … and ONLY there — the occurrence grid,
    /// anchored at start, excludes every between-tick.
    #[test]
    fn recurring_period_ten_known_answers() {
        let schedule = NpcSchedule {
            entries: vec![commitment(Tick(0), Cadence::every(10).unwrap(), 0)],
        };
        for due in [0u64, 10, 20, 30, 100] {
            let due_set: Vec<usize> = due_commitments(&schedule, Tick(due)).collect();
            assert_eq!(due_set, vec![0], "due at tick {due}");
        }
        for quiet in [1u64, 2, 9, 11, 19, 21, 99, 105] {
            let due_set: Vec<usize> = due_commitments(&schedule, Tick(quiet)).collect();
            assert!(due_set.is_empty(), "not due at tick {quiet}");
        }
    }

    /// KNOWN ANSWER: a recurring commitment anchored at a later
    /// start is due at start + k·period — never before start, never
    /// off-grid after it.
    #[test]
    fn recurring_from_later_start() {
        let schedule = NpcSchedule {
            entries: vec![commitment(Tick(100), Cadence::every(7).unwrap(), 0)],
        };
        for due in [100u64, 107, 114] {
            let due_set: Vec<usize> = due_commitments(&schedule, Tick(due)).collect();
            assert_eq!(due_set, vec![0], "due at tick {due}");
        }
        for quiet in [0u64, 99, 101, 106, 108, 113, 115] {
            let due_set: Vec<usize> = due_commitments(&schedule, Tick(quiet)).collect();
            assert!(due_set.is_empty(), "not due at tick {quiet}");
        }
    }

    /// KNOWN ANSWER: a one-shot is due at exactly its tick — not the
    /// tick before, not the tick after (the pinned choice: a single
    /// logical moment, reachable through next_due under fast-forward).
    #[test]
    fn one_shot_due_only_at_its_tick() {
        let schedule = NpcSchedule {
            entries: vec![commitment(Tick(25), Cadence::one_shot(), 0)],
        };
        assert_eq!(
            due_commitments(&schedule, Tick(25)).collect::<Vec<_>>(),
            vec![0]
        );
        for quiet in [0u64, 24, 26, 100] {
            assert!(
                due_commitments(&schedule, Tick(quiet)).next().is_none(),
                "not due at tick {quiet}"
            );
        }
    }

    /// KNOWN ANSWER: multiple commitments due at the same tick yield
    /// in FIXED AUTHORED ORDER — ascending entry index, skipping the
    /// not-due entry between them (the §8.3 fixed-evaluation-order
    /// rule; a reorder of the authored list reorders the yields, by
    /// construction and visibly).
    #[test]
    fn multiple_due_same_tick_in_fixed_order() {
        let schedule = NpcSchedule {
            entries: vec![
                commitment(Tick(0), Cadence::every(20).unwrap(), 0), // due 0, 20, 40…
                commitment(Tick(10), Cadence::one_shot(), 1),        // due 10 only
                commitment(Tick(0), Cadence::every(10).unwrap(), 2), // due 0, 10, 20…
            ],
        };
        assert_eq!(
            due_commitments(&schedule, Tick(0)).collect::<Vec<_>>(),
            vec![0, 2]
        );
        assert_eq!(
            due_commitments(&schedule, Tick(10)).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(
            due_commitments(&schedule, Tick(20)).collect::<Vec<_>>(),
            vec![0, 2]
        );
        assert_eq!(
            due_commitments(&schedule, Tick(30)).collect::<Vec<_>>(),
            vec![2]
        );
        assert!(
            due_commitments(&schedule, Tick(35)).next().is_none(),
            "a quiet tick yields nothing"
        );
    }

    /// KNOWN ANSWER: the empty schedule is due at no tick and never
    /// fast-forwards (next_due is None from any tick — the skip has
    /// nowhere to go).
    #[test]
    fn empty_schedule_is_due_nowhere() {
        let schedule = NpcSchedule::default();
        assert!(schedule.is_empty());
        assert_eq!(schedule.len(), 0);
        for tick in [0u64, 1, 1_000] {
            assert!(due_commitments(&schedule, Tick(tick)).next().is_none());
        }
        assert_eq!(next_due(&schedule, Tick(0)), None);
        assert_eq!(
            due_sequence_digest(&schedule, 64),
            Fingerprinter::new().finish()
        );
    }

    /// KNOWN ANSWER: next_due across boundaries — strictly after the
    /// argument, the first grid multiple beyond it; a one-shot's
    /// boundary is its own tick (None at or past it); the minimum
    /// across entries wins; a start not yet reached is itself next.
    #[test]
    fn next_due_across_boundaries() {
        let recurring = NpcSchedule {
            entries: vec![commitment(Tick(0), Cadence::every(10).unwrap(), 0)],
        };
        assert_eq!(next_due(&recurring, Tick(0)), Some(Tick(10)));
        assert_eq!(next_due(&recurring, Tick(9)), Some(Tick(10)));
        assert_eq!(next_due(&recurring, Tick(10)), Some(Tick(20)));
        assert_eq!(next_due(&recurring, Tick(15)), Some(Tick(20)));

        let anchored = NpcSchedule {
            entries: vec![commitment(Tick(100), Cadence::every(7).unwrap(), 0)],
        };
        assert_eq!(next_due(&anchored, Tick(0)), Some(Tick(100)));
        assert_eq!(next_due(&anchored, Tick(100)), Some(Tick(107)));
        assert_eq!(next_due(&anchored, Tick(105)), Some(Tick(107)));
        assert_eq!(next_due(&anchored, Tick(107)), Some(Tick(114)));

        let one_shot = NpcSchedule {
            entries: vec![commitment(Tick(30), Cadence::one_shot(), 0)],
        };
        assert_eq!(next_due(&one_shot, Tick(29)), Some(Tick(30)));
        assert_eq!(next_due(&one_shot, Tick(30)), None);
        assert_eq!(next_due(&one_shot, Tick(31)), None);

        // The minimum across entries: the one-shot at 25 beats the
        // recurring grid's 30 from tick 20.
        let mixed = NpcSchedule {
            entries: vec![
                commitment(Tick(0), Cadence::every(10).unwrap(), 0),
                commitment(Tick(25), Cadence::one_shot(), 1),
            ],
        };
        assert_eq!(next_due(&mixed, Tick(20)), Some(Tick(25)));
        assert_eq!(next_due(&mixed, Tick(26)), Some(Tick(30)));
    }

    /// The tick-space ceiling: an occurrence that would overflow the
    /// u64 tick is `None` (checked arithmetic — never a wraparound,
    /// never a panic), while one that lands exactly on `u64::MAX` is
    /// a real, due tick.
    #[test]
    fn next_due_saturates_to_none_not_wraparound() {
        // start + 10 == u64::MAX: representable, and due.
        let fits = NpcSchedule {
            entries: vec![commitment(
                Tick(u64::MAX - 10),
                Cadence::every(10).unwrap(),
                0,
            )],
        };
        assert_eq!(next_due(&fits, Tick(u64::MAX - 10)), Some(Tick(u64::MAX)));
        assert_eq!(
            due_commitments(&fits, Tick(u64::MAX)).collect::<Vec<_>>(),
            vec![0]
        );
        assert_eq!(next_due(&fits, Tick(u64::MAX)), None);

        // start + period overflows: no further occurrence exists.
        let overflows = NpcSchedule {
            entries: vec![commitment(
                Tick(u64::MAX - 5),
                Cadence::every(10).unwrap(),
                0,
            )],
        };
        assert_eq!(next_due(&overflows, Tick(u64::MAX - 5)), None);
        assert_eq!(next_due(&overflows, Tick(0)), Some(Tick(u64::MAX - 5)));
    }

    /// The typed zero refusal: a zero period is unrepresentable
    /// (`None` from the constructor, never a panic, never a
    /// silently-every-tick commitment).
    #[test]
    fn cadence_zero_period_is_refused() {
        assert_eq!(Cadence::every(0), None);
        assert!(Cadence::every(1).is_some());
        assert_eq!(Cadence::one_shot(), Cadence::OneShot);
    }

    /// The digest fixture: recurring at period 10 from 0, a one-shot
    /// at 25, and a period-7 grid anchored at 3 — three authored
    /// shapes whose due sequence over 0..40 crosses (recurring,
    /// one-shot, two-due-at-one-tick in fixed order at tick 10).
    fn digest_fixture() -> NpcSchedule {
        NpcSchedule {
            entries: vec![
                commitment(Tick(0), Cadence::every(10).unwrap(), 0),
                commitment(Tick(25), Cadence::one_shot(), 1),
                commitment(Tick(3), Cadence::every(7).unwrap(), 2),
            ],
        }
    }

    /// THE DIGEST PIN: the fixture's sampled due-sequence (span 40)
    /// hashes to this FNV-1a 64 value on every platform — integer
    /// arithmetic only, so there is no libm to vary. A moved value is
    /// an investigation (a due-semantics or folding-order change),
    /// never a silent re-pin.
    #[test]
    fn due_sequence_digest_pin() {
        assert_eq!(
            due_sequence_digest(&digest_fixture(), 40),
            PINNED_DUE_DIGEST
        );
    }

    /// The measured-actual FNV-1a of the fixture's due sequence.
    const PINNED_DUE_DIGEST: u64 = 11_670_746_710_459_133_860;

    /// The digest is a pure function of the due sequence: same
    /// schedule ⇒ same digest; an empty span ⇒ the empty digest (the
    /// FNV offset basis); a span that includes a due tick differs
    /// from one that excludes it.
    #[test]
    fn due_sequence_digest_is_pure_and_span_sensitive() {
        let schedule = digest_fixture();
        assert_eq!(
            due_sequence_digest(&schedule, 40),
            due_sequence_digest(&digest_fixture(), 40),
            "same authored data, same digest"
        );
        let empty = Fingerprinter::new().finish();
        assert_eq!(due_sequence_digest(&schedule, 0), empty);
        assert_ne!(due_sequence_digest(&schedule, 1), empty, "tick 0 is due");
    }
}
