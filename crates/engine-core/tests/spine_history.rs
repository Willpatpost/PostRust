//! PP-B2 — the `spine-history/2` compaction test matrix (the GO §5
//! acceptance list at the engine-core level): the explicit
//! compaction operation and its typed refusals; the compacted round
//! trip and byte determinism; the two-independent-compactions
//! agreement; the prefix-summary chaining across successive
//! compactions; the full-vs-compacted twin continuation equivalence;
//! the version and domain distinctness of the two forms; the
//! corruption matrix over every compacted load gate (through an
//! independent test-only walk of the save/2 layout — the B1
//! reference-codec discipline, so the production decoder is never
//! the only implementation verifying the layout); the accessor and
//! fingerprint contracts (loud panics naming the safe paths, `try_*`
//! safety, the compacted domains' comparison semantics); and the E3
//! complement-of-unfinished membership equivalence against the
//! ledger ground truth.

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};

use common::{
    GoldenWorld, expect_error, golden_spine, patch_u8, patch_u64, restore_golden, scenario,
};
use engine_core::{CompactionError, EventId, Fingerprinter, HistoryKind, Spine, World};

// ------------------------------------------------------------ helpers

fn world_fingerprint(spine: &Spine<GoldenWorld>) -> u64 {
    let mut fp = Fingerprinter::new();
    spine.world().fingerprint(&mut fp);
    fp.finish()
}

/// Asserts a compacted twin and its uncompacted control are
/// logically indistinguishable: authoritative counters, queue
/// depths, both watermark measures, the held fingerprint (valid on
/// both representations), and the world fingerprint — the strongest
/// cross-representation comparisons available (the four
/// ledger-hashing fingerprints are full-history APIs by contract and
/// are asserted unavailable on the twin in the accessor tests).
fn assert_twin_equivalent(control: &Spine<GoldenWorld>, twin: &Spine<GoldenWorld>, context: &str) {
    assert_eq!(control.clock(), twin.clock(), "{context}: clock");
    assert_eq!(control.stats(), twin.stats(), "{context}: stats");
    assert_eq!(
        control.pending_count(),
        twin.pending_count(),
        "{context}: pending"
    );
    assert_eq!(
        control.blocked_count(),
        twin.blocked_count(),
        "{context}: blocked"
    );
    assert_eq!(control.held_count(), twin.held_count(), "{context}: held");
    assert_eq!(
        control.max_executed_due(),
        twin.max_executed_due(),
        "{context}: max executed due (the carried value)"
    );
    assert_eq!(
        control.processed_through(),
        twin.processed_through(),
        "{context}: watermark"
    );
    assert_eq!(
        control.held_fingerprint(),
        twin.held_fingerprint(),
        "{context}: held fingerprint"
    );
    assert_eq!(
        world_fingerprint(control),
        world_fingerprint(twin),
        "{context}: world fingerprint"
    );
    // Conservation, recomputed from the public counters.
    for spine in [control, twin] {
        let stats = spine.stats();
        let conserved = stats.executed
            + spine.pending_count() as u64
            + spine.blocked_count() as u64
            + spine.held_count() as u64;
        assert_eq!(stats.scheduled, conserved, "{context}: conservation");
    }
}

/// The independent test-only walk of a `sektor7/save/2` blob: records
/// the byte offsets of every scalar the corruption tests patch and
/// the tail record positions. Asserts the layout is consumed exactly.
struct V2Walk {
    next_seq_at: usize,
    stats_at: [usize; 7],
    first_unfinished_seq: u64,
    max_present_at: usize,
    max_tick_at: usize,
    summary_len_at: usize,
    summarized_at: usize,
    tail_seq_ats: Vec<usize>,
    tail_late_at: Vec<usize>,
    tail_intended_due: Vec<u64>,
}

fn walk_v2(bytes: &[u8]) -> V2Walk {
    let mut pos = 0usize;
    let mut take_u64 = |pos: &mut usize| -> u64 {
        let value = u64::from_le_bytes(
            bytes[*pos..*pos + 8]
                .try_into()
                .expect("the walk has 8 bytes"),
        );
        *pos += 8;
        value
    };
    fn skip_tag(bytes: &[u8], pos: &mut usize) {
        let len = u64::from_le_bytes(bytes[*pos..*pos + 8].try_into().expect("8 bytes")) as usize;
        *pos += 8 + len;
    }
    fn walk_entries(
        bytes: &[u8],
        pos: &mut usize,
        take_u64: &mut dyn FnMut(&mut usize) -> u64,
    ) -> Option<u64> {
        let count = take_u64(pos) as usize;
        let mut first = None;
        for _ in 0..count {
            let seq = take_u64(pos);
            if first.is_none() {
                first = Some(seq);
            }
            *pos += 4; // class
            *pos += 1; // field
            let kind = bytes[*pos];
            *pos += 1;
            *pos += if kind == 0 { 8 } else { 16 };
            *pos += 8; // payload
            let deps = take_u64(pos) as usize;
            *pos += 8 * deps;
        }
        first
    }
    skip_tag(bytes, &mut pos); // sektor7/save/2
    pos += 4; // version
    skip_tag(bytes, &mut pos); // engine-core/spine-history/2
    take_u64(&mut pos); // clock
    let next_seq_at = pos;
    take_u64(&mut pos); // next_seq
    pos += 4; // budget
    pos += 4; // near_reserve
    let mut stats_at = [0usize; 7];
    for slot in stats_at.iter_mut() {
        *slot = pos;
        pos += 8;
    }
    let near_first = walk_entries(bytes, &mut pos, &mut take_u64);
    let far_first = walk_entries(bytes, &mut pos, &mut take_u64);
    walk_entries(bytes, &mut pos, &mut take_u64); // blocked
    walk_entries(bytes, &mut pos, &mut take_u64); // held
    let max_present_at = pos;
    assert_eq!(
        bytes[pos], 1,
        "the walk: a real compacted save carries the max"
    );
    pos += 1;
    let max_tick_at = pos;
    pos += 8;
    let summary_len_at = pos;
    assert_eq!(take_u64(&mut pos), 32, "the walk: the summary is 32 bytes");
    pos += 32;
    let summarized_at = pos;
    pos += 8;
    let tail_count = take_u64(&mut pos) as usize;
    let mut tail_seq_ats = Vec::with_capacity(tail_count);
    let mut tail_late_at = Vec::with_capacity(tail_count);
    let mut tail_intended_due = Vec::with_capacity(tail_count);
    for _ in 0..tail_count {
        tail_seq_ats.push(pos);
        pos += 8; // seq
        pos += 4; // class
        pos += 1; // field
        tail_intended_due.push(take_u64(&mut pos)); // intended due
        pos += 8; // executed tick
        pos += 1; // schedule kind
        tail_late_at.push(pos);
        pos += 1; // late
    }
    skip_tag(bytes, &mut pos); // world domain
    let world_len = u64::from_le_bytes(bytes[pos..pos + 8].try_into().expect("8 bytes")) as usize;
    pos += 8 + world_len;
    assert_eq!(pos, bytes.len(), "the walk: the save consumed exactly");
    V2Walk {
        next_seq_at,
        stats_at,
        first_unfinished_seq: near_first
            .or(far_first)
            .expect("the scenario has unfinished work"),
        max_present_at,
        max_tick_at,
        summary_len_at,
        summarized_at,
        tail_seq_ats,
        tail_late_at,
        tail_intended_due,
    }
}

/// A compacted save of the golden scenario retaining `retain` records.
fn compacted_bytes(retain: u64) -> Vec<u8> {
    let mut spine = golden_spine();
    spine.compact(retain).expect("the golden scenario compacts");
    spine.save_bytes()
}

// --------------------------------------------------------- the matrix

#[test]
fn compaction_refuses_degenerate_windows() {
    let mut spine = golden_spine();
    let ledger_len = spine.ledger_retained().len() as u64;
    assert!(ledger_len > 0, "the golden scenario executed work");
    assert_eq!(
        spine.compact(ledger_len),
        Err(CompactionError::NothingToSummarize {
            ledger_len,
            retain: ledger_len
        })
    );
    assert_eq!(
        spine.compact(ledger_len + 1),
        Err(CompactionError::RetainExceedsLedger {
            ledger_len,
            retain: ledger_len + 1
        })
    );
    // An empty ledger has nothing to summarize either.
    let mut empty = Spine::new(GoldenWorld::new(), 4);
    assert_eq!(
        empty.compact(0),
        Err(CompactionError::NothingToSummarize {
            ledger_len: 0,
            retain: 0
        })
    );
    assert_eq!(
        empty.compact(1),
        Err(CompactionError::RetainExceedsLedger {
            ledger_len: 0,
            retain: 1
        })
    );
    // The refusals mutate nothing: the spine is still Full, the
    // ledger intact.
    assert_eq!(spine.history_kind(), HistoryKind::Full);
    assert_eq!(spine.ledger_retained().len() as u64, ledger_len);
}

#[test]
fn compacted_round_trip_preserves_logical_state() {
    let control = golden_spine();
    let bytes = compacted_bytes(3);
    // Byte determinism: two encodings of the same state are identical.
    assert_eq!(compacted_bytes(3), bytes);
    let twin = restore_golden(&bytes).expect("the compacted save restores");
    // The restored representation is Compacted with the exact cut.
    assert_eq!(
        twin.history_kind(),
        HistoryKind::Compacted {
            summarized_records: control.ledger_retained().len() as u64 - 3,
            retained_records: 3,
            prefix_summary: match twin.history_kind() {
                HistoryKind::Compacted { prefix_summary, .. } => prefix_summary,
                HistoryKind::Full => unreachable!("asserted above"),
            },
        }
    );
    assert_twin_equivalent(&control, &twin, "the compacted round trip");
    // The retained tail is the control ledger's verbatim suffix.
    assert_eq!(
        twin.ledger_retained(),
        &control.ledger_retained()[control.ledger_retained().len() - 3..]
    );
}

#[test]
fn two_independent_compactions_of_the_same_history_agree() {
    // Two separately constructed instances of the same deterministic
    // scenario, compacted independently — identical representations.
    let a = compacted_bytes(5);
    let b = compacted_bytes(5);
    assert_eq!(a, b, "independent compactions produce identical bytes");
    let spine_a = restore_golden(&a).unwrap();
    let spine_b = restore_golden(&b).unwrap();
    assert_eq!(
        spine_a.compacted_fingerprint(),
        spine_b.compacted_fingerprint()
    );
    assert_eq!(
        spine_a.compacted_state_fingerprint(),
        spine_b.compacted_state_fingerprint()
    );
}

#[test]
fn prefix_summary_chains_across_successive_compactions() {
    // The control runs the full history uncompacted; the twin
    // compacts, restores, continues, and compacts again. The second
    // compaction chains the first's summary, so the final summaries
    // and representations agree with the single equivalent
    // compaction of the same total history.
    let mut control = golden_spine();
    let mut twin = restore_golden(&compacted_bytes(4)).unwrap();
    control.run_ticks(9);
    twin.run_ticks(9);
    assert_twin_equivalent(&control, &twin, "after the continuation");
    // The twin's retained records are exactly the control's records
    // since the cut.
    let cut = match twin.history_kind() {
        HistoryKind::Compacted {
            summarized_records, ..
        } => summarized_records as usize,
        HistoryKind::Full => unreachable!("the twin restored a compacted save"),
    };
    assert_eq!(twin.ledger_retained(), &control.ledger_retained()[cut..]);
    // Summarize everything on both sides.
    control.compact(0).unwrap();
    twin.compact(0).unwrap();
    assert_eq!(
        twin.history_kind(),
        control.history_kind(),
        "the chained compaction reaches the same cut"
    );
    assert_eq!(
        twin.save_bytes(),
        control.save_bytes(),
        "the chained summary and the single equivalent compaction agree"
    );
}

#[test]
fn full_and_compacted_saves_are_version_distinct() {
    let full = golden_spine().save_bytes();
    let compacted = compacted_bytes(3);
    // A save/1 blob with the version field claiming 2 is refused.
    let mut patched = full.clone();
    let version_at = 8 + "sektor7/save/1".len();
    common::patch_u32(&mut patched, version_at, 2);
    assert!(matches!(
        expect_error(&patched),
        engine_core::SaveError::Version { observed: 2, .. }
    ));
    // A save/2 blob with the version field claiming 1 is refused.
    let mut patched = compacted.clone();
    let version_at = 8 + "sektor7/save/2".len();
    common::patch_u32(&mut patched, version_at, 1);
    assert!(matches!(
        expect_error(&patched),
        engine_core::SaveError::Version { observed: 1, .. }
    ));
    // An unknown outer domain is a typed domain error naming the
    // expected tag.
    let mut patched = full.clone();
    patched[8 + "sektor7/save/".len()] = b'9';
    assert!(matches!(
        expect_error(&patched),
        engine_core::SaveError::Domain { .. }
    ));
    // A save/2 blob whose spine section carries the wrong domain is
    // refused by the section gate.
    let mut patched = compacted.clone();
    let spine_tag_at = 8 + "sektor7/save/2".len() + 4 + 8;
    patched[spine_tag_at + "engine-core/spine-history/".len()] = b'9';
    assert!(matches!(
        expect_error(&patched),
        engine_core::SaveError::Domain { .. }
    ));
}

#[test]
fn save_dispatch_selects_the_form() {
    // A Full spine writes save/1; a compacted spine writes save/2 —
    // the first wire field (the length-prefixed domain tag) pins it.
    let full = golden_spine().save_bytes();
    assert_eq!(&full[8..8 + "sektor7/save/1".len()], b"sektor7/save/1");
    let compacted = compacted_bytes(2);
    assert_eq!(&compacted[8..8 + "sektor7/save/2".len()], b"sektor7/save/2");
}

#[test]
fn compacted_corruption_matrix() {
    // Retain all but one record so the tail spans many ticks and
    // includes the scenario's late executions.
    let bytes = compacted_bytes(golden_spine().ledger_retained().len() as u64 - 1);
    assert!(restore_golden(&bytes).is_ok(), "the fixture restores");
    let walk = walk_v2(&bytes);

    let reject = |mutate: &dyn Fn(&mut Vec<u8>)| {
        let mut patched = bytes.clone();
        mutate(&mut patched);
        expect_error(&patched)
    };

    // The empty summary — an explicit compaction summarizes at least
    // one record.
    let error = reject(&|patched: &mut Vec<u8>| {
        patch_u64(patched, walk.summarized_at, 0);
    });
    assert_eq!(
        error,
        engine_core::SaveError::Integrity("spine-history/empty-summary")
    );

    // The count identity: summarized + retained == executed.
    let error = reject(&|patched: &mut Vec<u8>| {
        let observed = u64::from_le_bytes(
            patched[walk.summarized_at..walk.summarized_at + 8]
                .try_into()
                .unwrap(),
        );
        patch_u64(patched, walk.summarized_at, observed + 1);
    });
    assert_eq!(
        error,
        engine_core::SaveError::Integrity("spine-history/count-identity")
    );

    // The summary must be exactly 32 bytes.
    let error = reject(&|patched: &mut Vec<u8>| {
        patch_u64(patched, walk.summary_len_at, 31);
    });
    assert_eq!(
        error,
        engine_core::SaveError::Integrity("spine-history/summary-length")
    );

    // The tail's frontier bound: a record claiming an unallocated seq.
    let error = reject(&|patched: &mut Vec<u8>| {
        let next_seq = u64::from_le_bytes(
            patched[walk.next_seq_at..walk.next_seq_at + 8]
                .try_into()
                .unwrap(),
        );
        patch_u64(patched, walk.tail_seq_ats[0], next_seq);
    });
    assert_eq!(
        error,
        engine_core::SaveError::Integrity("spine-save/frontier")
    );

    // The tail's disjointness from unfinished work.
    let error = reject(&|patched: &mut Vec<u8>| {
        patch_u64(patched, walk.tail_seq_ats[0], walk.first_unfinished_seq);
    });
    assert_eq!(
        error,
        engine_core::SaveError::Integrity("spine-save/ledger-overlaps-unfinished")
    );

    // A tail record's lateness flag disagreeing with its derivation.
    let error = reject(&|patched: &mut Vec<u8>| {
        let flipped = u8::from(patched[walk.tail_late_at[0]] == 0);
        patch_u8(patched, walk.tail_late_at[0], flipped);
    });
    assert_eq!(
        error,
        engine_core::SaveError::Integrity("spine-save/clock-coherence")
    );

    // The late-count bound: the statistic cannot fall below the
    // retained records' own late count.
    let tail_late = walk
        .tail_late_at
        .iter()
        .filter(|&&at| bytes[at] == 1)
        .count() as u64;
    assert!(tail_late > 0, "the scenario's tail includes late records");
    let error = reject(&|patched: &mut Vec<u8>| {
        patch_u64(patched, walk.stats_at[3], tail_late - 1);
    });
    assert_eq!(
        error,
        engine_core::SaveError::Integrity("spine-history/late-count-bound")
    );

    // The carried max below the retained tail's maximum intended due.
    let tail_max = walk.tail_intended_due.iter().copied().max().unwrap();
    let error = reject(&|patched: &mut Vec<u8>| {
        patch_u64(patched, walk.max_tick_at, tail_max - 1);
    });
    assert_eq!(
        error,
        engine_core::SaveError::Integrity("spine-history/max-below-tail")
    );

    // The carried max beyond the clock.
    let clock = u64::from_le_bytes(
        bytes[walk.max_tick_at - 8 - 8 - 4 - 4 - 8..]
            .iter()
            .take(8)
            .copied()
            .collect::<Vec<u8>>()
            .try_into()
            .unwrap(),
    );
    let error = reject(&|patched: &mut Vec<u8>| {
        patch_u64(patched, walk.max_tick_at, clock + 1);
    });
    assert_eq!(
        error,
        engine_core::SaveError::Integrity("spine-history/max-clock-coherence")
    );

    // A missing carried max: the present flag cleared and the tick
    // bytes removed — a structurally consistent rebuild, so the
    // max-required gate itself fires.
    let mut no_max = bytes[..walk.max_present_at].to_vec();
    no_max.push(0);
    no_max.extend_from_slice(&bytes[walk.max_present_at + 9..]);
    assert_eq!(
        expect_error(&no_max),
        engine_core::SaveError::Integrity("spine-history/max-required")
    );

    // Trailing bytes are corruption.
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(matches!(
        expect_error(&trailing),
        engine_core::SaveError::Trailing { .. }
    ));

    // Every truncation is rejected — no prefix of a save parses whole.
    for cut in 0..bytes.len() {
        assert!(
            restore_golden(&bytes[..cut]).is_err(),
            "a prefix of {cut} bytes must not parse as a complete save"
        );
    }
}

#[test]
fn accessor_and_fingerprint_contracts() {
    let control = golden_spine();
    let twin = restore_golden(&compacted_bytes(4)).unwrap();

    // The safe paths report unavailability — never a value.
    assert!(twin.try_ledger_full().is_none());
    assert!(twin.try_engine_fingerprint().is_none());
    assert!(twin.try_lane_fingerprint().is_none());
    assert!(twin.try_state_fingerprint().is_none());
    assert!(twin.try_lane_state_fingerprint().is_none());
    // On Full they equal the direct calls.
    assert_eq!(control.try_ledger_full(), Some(control.ledger()));
    assert_eq!(
        control.try_engine_fingerprint(),
        Some(control.engine_fingerprint())
    );

    // The loud paths name the remedy (fail-visible, never silent).
    for (api, remedy) in [
        ("ledger()", "ledger_retained()"),
        ("engine_fingerprint()", "try_engine_fingerprint()"),
        ("state_fingerprint()", "try_state_fingerprint()"),
        ("lane_fingerprint()", "try_lane_fingerprint()"),
        ("lane_state_fingerprint()", "try_lane_state_fingerprint()"),
    ] {
        let result = catch_unwind(AssertUnwindSafe(|| match api {
            "ledger()" => {
                twin.ledger();
            }
            "engine_fingerprint()" => {
                twin.engine_fingerprint();
            }
            "state_fingerprint()" => {
                twin.state_fingerprint();
            }
            "lane_fingerprint()" => {
                twin.lane_fingerprint();
            }
            _ => {
                twin.lane_state_fingerprint();
            }
        }));
        let panic = result.expect_err("the full-history API must surface loudly");
        let message = panic
            .downcast_ref::<String>()
            .expect("the panic payload is a formatted string");
        assert!(message.contains(api), "the panic names {api}");
        assert!(
            message.contains(remedy),
            "the panic names the remedy {remedy}"
        );
    }

    // The compacted domains: two same-cut instances agree...
    let twin_b = restore_golden(&compacted_bytes(4)).unwrap();
    assert_eq!(twin.compacted_fingerprint(), twin_b.compacted_fingerprint());
    assert_eq!(
        twin.compacted_state_fingerprint(),
        twin_b.compacted_state_fingerprint()
    );
    // ...different cuts differ (same logical state, different
    // compaction cut)...
    let other_cut = restore_golden(&compacted_bytes(2)).unwrap();
    assert_ne!(
        twin.compacted_fingerprint(),
        other_cut.compacted_fingerprint()
    );
    // ...and a Full instance cannot compare through them (loudly).
    let result = catch_unwind(AssertUnwindSafe(|| control.compacted_fingerprint()));
    assert!(
        result.is_err(),
        "compacted_fingerprint on Full panics loudly"
    );

    // held_fingerprint hashes no ledger and is valid on both.
    assert_eq!(control.held_fingerprint(), twin.held_fingerprint());
}

#[test]
fn compacted_twin_continuation_equivalence() {
    let mut control = scenario(4242, 6, 2);
    let mut twin = {
        let mut spine = scenario(4242, 6, 2);
        spine.compact(2).unwrap();
        restore_golden(&spine.save_bytes()).unwrap()
    };
    // Pre-continuation equivalence, then identical continuations.
    assert_twin_equivalent(&control, &twin, "pre-continuation");
    control.run_ticks(11);
    twin.run_ticks(11);
    assert_twin_equivalent(&control, &twin, "post-continuation");
    // The twin's retained records are the control's records since the
    // cut — the continuation appended identical records.
    let cut = match twin.history_kind() {
        HistoryKind::Compacted {
            summarized_records, ..
        } => summarized_records as usize,
        HistoryKind::Full => unreachable!("the twin restored a compacted save"),
    };
    assert_eq!(twin.ledger_retained(), &control.ledger_retained()[cut..]);

    // Dependency resolvability after compaction: a follow-up may
    // depend on a summarized (executed) identity, and unknown
    // identities are still rejected.
    let summarized_seq = EventId { seq: 0 };
    assert!(twin.is_executed(summarized_seq));
    let spec = engine_core::EventSpec::new(
        engine_core::EventClass::new(9),
        engine_core::Schedule::AtLogicalTime {
            due: twin.clock().saturating_add(5),
        },
        5,
    )
    .with_dependencies(vec![summarized_seq]);
    assert!(twin.schedule(spec).is_ok());
    let unknown = EventId {
        seq: twin.stats().scheduled + 100,
    };
    let bad = engine_core::EventSpec::new(
        engine_core::EventClass::new(9),
        engine_core::Schedule::AtLogicalTime {
            due: twin.clock().saturating_add(5),
        },
        6,
    )
    .with_dependencies(vec![unknown]);
    assert!(twin.schedule(bad).is_err());
}

#[test]
fn e3_executed_membership_matches_the_ledger_ground_truth() {
    // The complement-of-unfinished representation's membership query
    // equals the ledger's own executed set — the ground truth —
    // across scheduling, execution, suspension, and resumption.
    fn check(spine: &Spine<GoldenWorld>) {
        let Some(ledger) = spine.try_ledger_full() else {
            panic!("the E3 control spine stays Full");
        };
        let executed: std::collections::BTreeSet<u64> =
            ledger.iter().map(|record| record.id.seq).collect();
        let next_seq = spine.stats().scheduled;
        for seq in 0..next_seq {
            assert_eq!(
                spine.is_executed(EventId { seq }),
                executed.contains(&seq),
                "membership mismatch at #{seq}"
            );
        }
    }
    let mut spine = scenario(9001, 7, 3);
    check(&spine);
    spine.run_ticks(4);
    check(&spine);
    spine.suspend_matching(|view| view.payload % 2 == 1);
    check(&spine);
    spine.run_ticks(3);
    check(&spine);
    spine.resume_matching(|_| true);
    check(&spine);
    spine.run_ticks(5);
    check(&spine);
}

// ---------------------------------------- R2 frontier authority (D-EE-11)

/// R2 arm 1 — acknowledgments beyond executed history: the register
/// rule's FLOOR (the standing cut) was enforced, but neither the ack
/// nor the join-time vouch had a CEILING — a consumer could ack
/// ordinals that were never executed and the frontier would vouch
/// for records that do not exist (the red witness at base `a3bd984`:
/// `acknowledge_retention_through(executed + 1_000)` was ACCEPTED
/// and made the consumer the frontier's leader). The fix: the
/// executed-ordinal space's extent (`summarized + retained`, the
/// count identity) is the ceiling, refused TYPED and fail-visible at
/// both ack and register — never clamped (an ack for nonexistent
/// records is a caller bug; the typed-refusal discipline —
/// Regression/UnknownConsumer — is the precedent).
#[test]
fn r2_ack_beyond_executed_history_is_refused_typed() {
    use engine_core::spine::{
        RetentionAckError, RetentionConsumerId, RetentionPolicy, RetentionRegisterError,
    };
    use std::num::NonZeroU64;

    let consumer = RetentionConsumerId::new("r2-ceiling");
    let mut spine = scenario(4242, 6, 2);
    let executed = spine.stats().executed;
    assert!(executed > 0, "the scenario executed records");
    assert_eq!(
        spine.ledger_retained().len() as u64,
        executed,
        "an uncut spine's extent is the executed count"
    );
    spine.arm_retention_frontier(RetentionPolicy::new(
        NonZeroU64::new(1).unwrap(),
        NonZeroU64::new(30).unwrap(),
    ));
    spine
        .register_retention_consumer(consumer, 0)
        .expect("a fresh consumer registers at zero");

    // The beyond-history ack: refused typed, with the extent named.
    assert!(matches!(
        spine.acknowledge_retention_through(consumer, executed + 1_000),
        Err(RetentionAckError::BeyondExecutedHistory {
            id,
            executed_through: extent,
            attempted_through,
        }) if id == consumer && extent == executed && attempted_through == executed + 1_000
    ));
    // The refused ack mutated nothing: the consumer's standing ack is
    // still zero (it is the holder, not the leader).
    assert_eq!(spine.retention_consumer_lag(consumer), Some(0));

    // The boundary is INCLUSIVE: the executed extent itself is the
    // largest honest ack (a consumer that durably processed
    // everything).
    spine
        .acknowledge_retention_through(consumer, executed)
        .expect("acking exactly the executed extent is honest");
    assert_eq!(spine.retention_consumer_lag(consumer), Some(0));

    // The join-time vouch has the same ceiling (a late joiner cannot
    // offer a read point past records that were never executed).
    let late = RetentionConsumerId::new("r2-late-ceiling");
    assert!(matches!(
        spine.register_retention_consumer(late, executed + 1),
        Err(RetentionRegisterError::BeyondExecutedHistory {
            id,
            executed_through: extent,
            offered_read_point,
        }) if id == late && extent == executed && offered_read_point == executed + 1
    ));
    spine
        .register_retention_consumer(late, executed)
        .expect("joining at exactly the executed extent is honest");
}

/// R2 arm 2 — ordinary explicit compaction vs the consumer frontier:
/// at base `a3bd984` the explicit `Spine::compact` consulted only the
/// window (the red witness: `compact(1)` on a frontier-armed spine
/// with the consumer acked at 0 succeeded, summarizing records the
/// consumer still needed). The fix (the reviewer's shape (a)): the
/// ORDINARY compact is safe by default — it refuses, typed, any
/// window whose post-cut `summarized_records` would pass the minimum
/// registered acknowledgment (and an armed-but-EMPTY frontier holds
/// everything, mirroring the cadence's `min(∅)`-is-refusal rule) —
/// while the explicitly authorized administrative cut is its own
/// loud, named call shape. The gate is vacuous with no frontier
/// armed, and the authorized cut's wire bytes are the ordinary
/// cut's (the gate decides, it never re-encodes).
#[test]
fn r2_ordinary_compact_respects_the_frontier_and_the_override_is_typed() {
    use engine_core::spine::{
        AdministrativeCompaction, CompactionError, RetentionConsumerId, RetentionPolicy,
    };
    use std::num::NonZeroU64;

    let consumer = RetentionConsumerId::new("r2-gate");
    let mut spine = scenario(4242, 6, 2);
    let executed = spine.stats().executed;
    let live = spine.ledger_retained().len() as u64;
    assert_eq!(executed, live, "the scenario's spine is uncut");

    // The armed-but-EMPTY frontier holds everything: min(∅) is a
    // refusal, never a permission — the cadence's rule mirrored at
    // the explicit path.
    spine.arm_retention_frontier(RetentionPolicy::new(
        NonZeroU64::new(1).unwrap(),
        NonZeroU64::new(30).unwrap(),
    ));
    assert!(matches!(
        spine.compact(live - 1),
        Err(CompactionError::ConsumerFrontierHolds {
            consumers: 0,
            frontier_through: 0,
            summarized_records: 0,
            ..
        })
    ));
    assert!(
        matches!(spine.history_kind(), HistoryKind::Full),
        "the refused ordinary compact mutated nothing"
    );

    // A registered consumer holding at ack 0: the ordinary compact
    // refuses the cut past it, typed, with the frontier named.
    spine
        .register_retention_consumer(consumer, 0)
        .expect("register");
    assert!(matches!(
        spine.compact(1),
        Err(CompactionError::ConsumerFrontierHolds {
            ledger_len,
            retain: 1,
            summarized_records: 0,
            frontier_through: 0,
            consumers: 1,
        }) if ledger_len == live
    ));
    assert!(
        matches!(spine.history_kind(), HistoryKind::Full),
        "the refused ordinary compact mutated nothing"
    );
    assert_eq!(
        spine.ledger_retained().len() as u64,
        live,
        "the refused ordinary compact discarded nothing"
    );

    // The safe window INSIDE the frontier: the consumer releases
    // `released` ordinals and the ordinary compact of exactly that
    // prefix succeeds (safe compaction is the default, not the
    // exception).
    let released = live / 2;
    spine
        .acknowledge_retention_through(consumer, released)
        .expect("the consumer releases half the history");
    spine
        .compact(live - released)
        .expect("the ordinary compact within the frontier is safe");
    let HistoryKind::Compacted {
        summarized_records, ..
    } = spine.history_kind()
    else {
        panic!("the safe window compacted the spine");
    };
    assert_eq!(summarized_records, released);

    // The authorized administrative cut past the frontier: the named,
    // loud override performs what the ordinary path refuses, and its
    // wire bytes are byte-identical to an unarmed spine's ordinary
    // cut of the same window (the gate decides, it never re-encodes —
    // the wire stays a function of the cut sequence alone).
    let cut_more = spine.ledger_retained().len() as u64 / 2;
    let mut unarmed = scenario(4242, 6, 2);
    unarmed
        .compact(live - released - cut_more)
        .expect("the unarmed twin takes the same total window");
    spine
        .compact_authorized(
            spine.ledger_retained().len() as u64 - cut_more,
            AdministrativeCompaction::explicit_override(),
        )
        .expect("the authorized administrative cut past the frontier");
    assert_eq!(
        spine.save_bytes(),
        unarmed.save_bytes(),
        "the authorized cut and the unarmed ordinary cut agree byte for byte"
    );
    // The frontier accounting stays coherent fail-closed after the
    // override: the consumer's ack (released) sits below the new
    // standing cut, so the cadence's releasable clamp saturates at
    // zero — no firing until the ack catches up.
    spine.run_ticks(3);
    assert_eq!(
        spine.retention_stats().compactions,
        0,
        "the over-cut frontier holds: releasable saturates at zero"
    );
    let HistoryKind::Compacted {
        summarized_records, ..
    } = spine.history_kind()
    else {
        panic!("the spine stays compacted");
    };
    spine
        .acknowledge_retention_through(consumer, summarized_records + 2)
        .expect("the ack catches up past the new standing cut");
    // Push the ledger past the policy's bound so the next checkpoint
    // crossing is guaranteed (the firing assertion must not depend on
    // the scenario's residual spawn budget).
    let due = spine.clock().0 + 1;
    for i in 0..40u64 {
        spine
            .schedule(common::at(due, 9_000 + i))
            .expect("schedule");
    }
    spine.run_ticks(6);
    assert_eq!(
        spine.retention_stats().compactions,
        1,
        "the caught-up frontier fires again, measured from the new prefix"
    );
    assert_eq!(
        spine.retention_stats().last_chunk,
        2,
        "the firing is clamped to the caught-up ack's releasable 2"
    );
}
