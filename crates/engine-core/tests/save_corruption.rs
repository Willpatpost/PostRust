//! Stage 5 Batch 1 corruption corpus: malformed and intentionally
//! mutated input must fail closed with typed errors — no panic, no
//! undefined behavior, no unbounded allocation, and never a
//! success-shaped value for corrupted state. Mutations are targeted
//! through the independent reference codec's offset map or crafted by
//! its re-serializer, so every documented failure class is exercised
//! against real production bytes.

mod common;

use common::{
    effective_due, expect_error, golden_spine, patch_u8, patch_u32, patch_u64, ref_read, ref_write,
    restore_golden,
};
use engine_core::SaveError;

fn golden_bytes() -> Vec<u8> {
    golden_spine().save_bytes()
}

fn queued_entry(parsed: &common::RefSave) -> common::RefEvent {
    parsed
        .near
        .first()
        .or_else(|| parsed.far.first())
        .expect("the golden scenario queues obligations")
        .clone()
}

#[test]
fn truncation_rejected_at_every_byte_boundary() {
    let bytes = golden_bytes();
    for cut in 0..bytes.len() {
        let result = restore_golden(&bytes[..cut]);
        assert!(
            result.is_err(),
            "a {cut}-byte prefix must not restore (error: {:?})",
            result.err()
        );
    }
    assert!(restore_golden(&bytes).is_ok());
}

#[test]
fn trailing_bytes_are_rejected() {
    let mut bytes = golden_bytes();
    bytes.push(0);
    assert!(matches!(
        expect_error(&bytes),
        SaveError::Trailing { leftover: 1 }
    ));
}

#[test]
fn unsupported_version_is_rejected_by_name() {
    let mut bytes = golden_bytes();
    let parsed = ref_read(&bytes);
    patch_u32(&mut bytes, parsed.version_at, 2);
    assert!(matches!(
        expect_error(&bytes),
        SaveError::Version {
            observed: 2,
            supported: 1
        }
    ));
}

#[test]
fn domain_tag_corruption_is_rejected() {
    let bytes = golden_bytes();
    let parsed = ref_read(&bytes);

    let mut header = bytes.clone();
    patch_u8(&mut header, 10, b'x'); // inside "sektor7/save/1"
    assert!(matches!(expect_error(&header), SaveError::Domain { .. }));

    let mut spine_tag = bytes.clone();
    patch_u8(&mut spine_tag, parsed.spine_tag_at + 9, b'x'); // inside "engine-core/spine-save/1"
    assert!(matches!(expect_error(&spine_tag), SaveError::Domain { .. }));

    let mut world = bytes.clone();
    patch_u8(&mut world, parsed.world_tag_at + 9, b'x'); // inside the world domain tag
    assert!(matches!(expect_error(&world), SaveError::Domain { .. }));
}

#[test]
fn invalid_enum_discriminants_are_rejected() {
    let bytes = golden_bytes();
    let parsed = ref_read(&bytes);
    let target = queued_entry(&parsed);

    let mut field = bytes.clone();
    patch_u8(&mut field, target.field_at, 2);
    assert!(matches!(expect_error(&field), SaveError::Enum { .. }));

    let mut kind = bytes.clone();
    patch_u8(&mut kind, target.kind_at, 9);
    assert!(matches!(expect_error(&kind), SaveError::Enum { .. }));

    let record = &parsed.ledger[0];
    let mut late = bytes.clone();
    patch_u8(&mut late, record.late_at, 2);
    assert!(matches!(expect_error(&late), SaveError::Enum { .. }));
}

#[test]
fn oversized_counts_are_rejected_before_allocation() {
    let bytes = golden_bytes();
    let parsed = ref_read(&bytes);

    // Beyond the documented collection maximum.
    let mut beyond_max = bytes.clone();
    patch_u64(&mut beyond_max, parsed.near_count_at, u64::MAX);
    assert!(matches!(expect_error(&beyond_max), SaveError::Limit { .. }));

    // Under the documented maximum but beyond the bytes remaining —
    // the per-section element bound fires before any allocation.
    let mut impossible = bytes.clone();
    let remaining_after = (bytes.len() - (parsed.near_count_at + 8)) as u64;
    patch_u64(
        &mut impossible,
        parsed.near_count_at,
        remaining_after / 38 + 1,
    );
    assert!(matches!(expect_error(&impossible), SaveError::Limit { .. }));

    // A dependency count inside a queued entry.
    let target = queued_entry(&parsed);
    let mut deps = bytes.clone();
    patch_u64(&mut deps, target.dep_count_at, 1 << 20);
    assert!(matches!(expect_error(&deps), SaveError::Limit { .. }));

    // The world section's byte length.
    let mut world = bytes.clone();
    patch_u64(&mut world, parsed.world_len_at, u64::MAX);
    assert!(matches!(expect_error(&world), SaveError::Limit { .. }));
}

#[test]
fn noncanonical_lane_order_is_rejected() {
    let bytes = golden_bytes();
    let mut parsed = ref_read(&bytes);
    {
        // The far lane is guaranteed two entries by construction (the
        // due-20 cause and the due-25 seed); the near lane's depth
        // depends on how the follow-up budget played out.
        let lane = if parsed.near.len() >= 2 {
            &mut parsed.near
        } else if parsed.far.len() >= 2 {
            &mut parsed.far
        } else {
            panic!("the golden scenario must queue at least two obligations in one lane");
        };
        lane.reverse();
    }
    let mutated = ref_write(&parsed);
    assert!(matches!(expect_error(&mutated), SaveError::Order { .. }));
}

#[test]
fn duplicate_identity_is_rejected() {
    // Cross-section: an identity already present in the far lane
    // injected into the near lane (kept in canonical lane order so the
    // identity gate, not the order gate, fires).
    let bytes = golden_bytes();
    let mut parsed = ref_read(&bytes);
    let mut injected = parsed.far.first().expect("far entry").clone();
    // Correction A's lane-field-coherence gate fires during the section
    // read, so the injected identity carries the target lane's field —
    // the cross-section identity gate is what this test exercises.
    injected.field = 0;
    parsed.near.push(injected);
    parsed
        .near
        .sort_by_key(|event| (effective_due(event), event.seq));
    let mutated = ref_write(&parsed);
    assert!(matches!(
        expect_error(&mutated),
        SaveError::Duplicate { .. }
    ));
}

#[test]
fn duplicate_ledger_identity_is_rejected() {
    let mut bytes = golden_bytes();
    let parsed = ref_read(&bytes);
    assert!(parsed.ledger.len() >= 2);
    patch_u64(&mut bytes, parsed.ledger[1].seq_at, parsed.ledger[0].seq);
    assert!(matches!(
        expect_error(&bytes),
        SaveError::Duplicate {
            section: "spine-save/ledger",
            ..
        }
    ));
}

#[test]
fn contradictory_statistics_are_rejected() {
    let bytes = golden_bytes();
    let parsed = ref_read(&bytes);

    // stats.executed off by one: the structural/ledger-count gates.
    let mut executed = bytes.clone();
    patch_u64(&mut executed, parsed.stats_at[1], parsed.stats[1] + 1);
    assert!(matches!(expect_error(&executed), SaveError::Integrity(_)));

    // stats.scheduled off by one: the dense-allocation gate.
    let mut scheduled = bytes.clone();
    patch_u64(&mut scheduled, parsed.stats_at[0], parsed.stats[0] + 1);
    assert!(matches!(expect_error(&scheduled), SaveError::Integrity(_)));

    // A counter beyond its structural bound (a tick can be
    // budget-exhausted at most once per elapsed tick).
    let mut exhausted = bytes.clone();
    patch_u64(&mut exhausted, parsed.stats_at[6], parsed.clock + 1);
    assert!(matches!(
        expect_error(&exhausted),
        SaveError::Integrity("spine-save/counter-bounds")
    ));
}

#[test]
fn fabricated_frontier_counters_are_rejected() {
    // A frontier pushed to u64::MAX with counters kept consistent
    // through the dense-allocation and structural-conservation gates
    // still cannot be real: the full-fidelity ledger count contradicts
    // the claimed executions. (The conservation sums are checked
    // arithmetic; with save/1's invariants they cannot overflow once
    // the preceding gates hold, so a fabricated frontier fails at the
    // next gate instead — the Overflow class is the defensive
    // checked-arithmetic failure for future format evolution.)
    let bytes = golden_bytes();
    let parsed = ref_read(&bytes);
    let unfinished =
        (parsed.near.len() + parsed.far.len() + parsed.blocked.len() + parsed.held.len()) as u64;

    let mut fabricated = bytes.clone();
    patch_u64(&mut fabricated, parsed.next_seq_at, u64::MAX);
    patch_u64(&mut fabricated, parsed.stats_at[0], u64::MAX);
    patch_u64(&mut fabricated, parsed.stats_at[1], u64::MAX - unfinished);
    assert!(matches!(
        expect_error(&fabricated),
        SaveError::Integrity("spine-save/ledger-count")
    ));
}

#[test]
fn unnormalized_configuration_is_rejected() {
    let bytes = golden_bytes();
    let parsed = ref_read(&bytes);

    let mut zero = bytes.clone();
    patch_u32(&mut zero, parsed.budget_at, 0);
    assert!(matches!(
        expect_error(&zero),
        SaveError::Integrity("spine-save/budget-zero")
    ));

    let mut reserve = bytes.clone();
    patch_u32(&mut reserve, parsed.near_reserve_at, parsed.budget + 1);
    assert!(matches!(
        expect_error(&reserve),
        SaveError::Integrity("spine-save/reserve-exceeds-budget")
    ));
}

#[test]
fn dependency_beyond_the_allocation_frontier_is_rejected() {
    let mut bytes = golden_bytes();
    let parsed = ref_read(&bytes);
    let blocked = parsed.blocked.first().expect("blocked entry");
    assert!(
        !blocked.deps.is_empty(),
        "the golden blocked obligation carries dependencies"
    );
    let first_dep_at = blocked.dep_count_at + 8;
    patch_u64(&mut bytes, first_dep_at, parsed.next_seq);
    assert!(matches!(
        expect_error(&bytes),
        SaveError::Integrity("spine-save/dependency-frontier")
    ));
}

#[test]
fn ledger_overlapping_unfinished_work_is_rejected() {
    let mut bytes = golden_bytes();
    let parsed = ref_read(&bytes);
    let queued = queued_entry(&parsed);
    patch_u64(&mut bytes, parsed.ledger[0].seq_at, queued.seq);
    assert!(matches!(
        expect_error(&bytes),
        SaveError::Integrity("spine-save/ledger-overlaps-unfinished")
    ));
}

#[test]
fn clock_incoherence_is_rejected() {
    let bytes = golden_bytes();
    let parsed = ref_read(&bytes);

    // An execution recorded in the future.
    let mut future = bytes.clone();
    patch_u64(
        &mut future,
        parsed.ledger[0].executed_tick_at,
        parsed.clock + 1,
    );
    assert!(matches!(
        expect_error(&future),
        SaveError::Integrity("spine-save/clock-coherence")
    ));

    // The encoded lateness flag contradicting its derivation.
    let on_time = parsed
        .ledger
        .iter()
        .find(|record| record.late == 0)
        .expect("an on-time record");
    let mut flag = bytes.clone();
    patch_u8(&mut flag, on_time.late_at, 1);
    assert!(matches!(
        expect_error(&flag),
        SaveError::Integrity("spine-save/clock-coherence")
    ));
}

#[test]
fn blocked_without_unmet_dependencies_is_rejected() {
    let bytes = golden_bytes();
    let mut parsed = ref_read(&bytes);
    let mut stripped = parsed.blocked[0].clone();
    stripped.deps.clear();
    parsed.blocked[0] = stripped;
    let mutated = ref_write(&parsed);
    assert!(matches!(
        expect_error(&mutated),
        SaveError::Integrity("spine-save/blocked-without-unmet-deps")
    ));
}

#[test]
fn world_section_trailing_bytes_are_rejected() {
    let bytes = golden_bytes();
    let mut parsed = ref_read(&bytes);
    parsed.world_bytes.push(0);
    let mutated = ref_write(&parsed);
    assert!(matches!(expect_error(&mutated), SaveError::Trailing { .. }));
}

#[test]
fn lane_field_mismatch_is_rejected() {
    // Review correction A: a serialized field disagreeing with its
    // containing lane section is not a live state — the spine routes
    // every obligation to the queue matching its field — and is
    // rejected, never silently re-laned.
    let bytes = golden_bytes();
    let parsed = ref_read(&bytes);
    let near = parsed.near.first().expect("a near-lane entry");
    let mut far_in_near = bytes.clone();
    patch_u8(&mut far_in_near, near.field_at, 1);
    assert!(matches!(
        expect_error(&far_in_near),
        SaveError::Integrity("spine-save/lane-field-coherence")
    ));
    let far = parsed.far.first().expect("a far-lane entry");
    let mut near_in_far = bytes.clone();
    patch_u8(&mut near_in_far, far.field_at, 0);
    assert!(matches!(
        expect_error(&near_in_far),
        SaveError::Integrity("spine-save/lane-field-coherence")
    ));
}

#[test]
fn late_execution_counter_reconciliation_is_enforced() {
    // Review correction B: the statistic must equal the ledger's count
    // of late records exactly — the individual records unchanged, the
    // statistic mutated in both directions.
    let bytes = golden_bytes();
    let parsed = ref_read(&bytes);
    assert!(parsed.stats[3] > 0, "the golden scenario executes late");
    let mut inflated = bytes.clone();
    patch_u64(&mut inflated, parsed.stats_at[3], parsed.stats[3] + 1);
    assert!(matches!(
        expect_error(&inflated),
        SaveError::Integrity("spine-save/late-execution-count")
    ));
    let mut deflated = bytes.clone();
    patch_u64(&mut deflated, parsed.stats_at[3], parsed.stats[3] - 1);
    assert!(matches!(
        expect_error(&deflated),
        SaveError::Integrity("spine-save/late-execution-count")
    ));
}

#[test]
fn decreasing_execution_tick_is_rejected() {
    // Review correction C: an otherwise structurally consistent ledger
    // whose executed ticks regress. The purpose-built scenario (budget
    // 1, four due-1 obligations) executes one per tick at ticks 1..4;
    // patching the final record's executed tick down to 2 keeps every
    // per-record invariant (due ≤ executed ≤ clock; the lateness flag
    // still matches its derivation — 2 > 1) while breaking the
    // chronological nondecrease.
    let mut spine = engine_core::Spine::with_reservation(common::GoldenWorld::new(), 1, 1);
    for payload in [3u64, 7, 11, 15] {
        spine.schedule(common::at(1, payload)).unwrap();
    }
    spine.run_ticks(4);
    let bytes = spine.save_bytes();
    assert!(
        restore_golden(&bytes).is_ok(),
        "the purpose-built save must be valid before mutation"
    );
    let mut parsed = ref_read(&bytes);
    let last = parsed.ledger.len() - 1;
    assert_eq!(parsed.ledger[last].executed_tick, 4);
    assert_eq!(parsed.ledger[last].late, 1);
    parsed.ledger[last].executed_tick = 2;
    let mutated = ref_write(&parsed);
    assert!(matches!(
        expect_error(&mutated),
        SaveError::Integrity("spine-save/ledger-order")
    ));
}

#[test]
fn npc_world_save_v2_declarations_refuse_typed_before_any_allocation() {
    // T21 (the corruption half of the decode-admission boundary):
    // crafted npc-world-save/2 section bytes — a declaration whose
    // whole-section minimum exceeds the bytes present, a truncation
    // inside a column, and an adversarial duplicate id encoded
    // through the total raw codec — all refuse typed (Limit /
    // Truncated / Duplicate), never by panic or unbounded
    // allocation, under the wire maximum AND a small configured
    // budget alike.
    use engine_core::lod::LodLevel;
    use engine_core::npc_behavior::{NpcId, NpcIdentity};
    use engine_core::npc_world_save::{NpcRegionBlock, NpcWorldSectionV2, SectionDecodeLimits};
    use engine_core::world_config::WorldConfig;
    use engine_core::{Decoder, Encoder};

    // A plausible huge declaration: 2 regions, 64 identities, a
    // grid, and 64 bytes of "records" — every per-field bound passes
    // (the generic one-byte-per-element count bound included: 64
    // identities, 65 bytes remaining after the count), but the
    // section's own minimum (2 × 10 framing + 64 × 35 dormant
    // identity bytes = 2,260) exceeds the 65 bytes actually
    // remaining. The typed Limit fires before the block vector or
    // any column reserves, under every budget.
    let mut enc = Encoder::new();
    enc.write_u64(1337); // seed
    enc.write_u64(6); // activity period
    enc.write_u64(4); // simplify factor
    enc.write_u64(12); // commitment window
    enc.write_u8(1); // region-LOD path enabled
    enc.write_count(2); // region count
    enc.write_count(64); // npc count
    enc.write_u8(4); // region grid
    for _ in 0..64 {
        enc.write_u8(0xAA); // sixty-four bytes actually present
    }
    let bytes = enc.into_bytes();
    for limits in [
        SectionDecodeLimits::WIRE_MAXIMUM,
        SectionDecodeLimits::new(64, u64::MAX),
        SectionDecodeLimits::new(64, 1 << 20),
    ] {
        assert!(
            matches!(
                NpcWorldSectionV2::restore_with_limits(&mut Decoder::new(&bytes), &limits),
                Err(SaveError::Limit {
                    kind: "section-minimum-bytes",
                    ..
                })
            ),
            "the huge declaration refuses typed under {limits:?}"
        );
    }
    // A truncation the minimum gate cannot see (an ACTIVE section
    // sits above its all-dormant minimum, so the declaration's
    // minimum fits while the input is cut inside a record column)
    // still refuses typed at the read — the walk never repairs.
    let mut enc = Encoder::new();
    enc.write_u64(1337);
    enc.write_u64(6);
    enc.write_u64(4);
    enc.write_u64(12);
    enc.write_u8(1);
    enc.write_count(1); // region count
    enc.write_count(2); // npc count
    enc.write_u8(4); // region grid
    enc.write_u8(0); // near
    enc.write_u8(0); // lod: Full (the section sits above its minimum)
    enc.write_count(2); // identity count
    for id in [0u32, 1] {
        let identity = NpcIdentity {
            id: NpcId(id),
            home_cell: (0, 0),
            region: 0,
            appearance_seed: id,
        };
        for byte in identity.wire_bytes() {
            enc.write_u8(byte);
        }
    }
    for _ in 0..2 {
        enc.write_u64(10); // steps
    }
    for _ in 0..2 {
        enc.write_u64(1); // routine cursor
    }
    for _ in 0..2 {
        enc.write_u8(0); // position x
        enc.write_u8(0); // position y
    }
    for _ in 0..2 {
        enc.write_u64(1); // represented-through
    }
    for _ in 0..2 {
        enc.write_u64(0); // pending signal
    }
    for _ in 0..2 {
        enc.write_u64(0); // milestone
    }
    for _ in 0..2 {
        enc.write_u64(0); // credit
    }
    let bytes = enc.into_bytes();
    // An above-minimum truncation (the ACTIVE section sits above its
    // all-dormant minimum, so the whole-input gate passes while the
    // input is 3 bytes short of the section's actual shape) refuses
    // typed at the stride gate — the credits column's wire minimum
    // (2 x 8 = 16) against the 13 bytes actually present — BEFORE
    // its reservation.
    let mut truncated = bytes.clone();
    truncated.truncate(truncated.len() - 3);
    assert!(matches!(
        NpcWorldSectionV2::restore(&mut Decoder::new(&truncated)),
        Err(SaveError::Limit {
            kind: "credits-count",
            ..
        })
    ));
    // A cut inside the header itself refuses at the read — the walk
    // never repairs, and no allocation has been made.
    assert!(matches!(
        NpcWorldSectionV2::restore(&mut Decoder::new(&bytes[..30])),
        Err(SaveError::Truncated)
    ));
    // An adversarial duplicate id across regions, encoded through
    // the TOTAL raw codec (encoding is never repair): the bounded
    // cross-region merge refuses it typed, naming the id.
    let duplicate = NpcWorldSectionV2 {
        seed: 42,
        activity_period: 5,
        simplify_factor: 3,
        commitment_window: 9,
        region_lods_enabled: true,
        config: WorldConfig {
            region_count: 2,
            npc_count: 3,
            region_grid: 16,
        },
        regions: vec![
            NpcRegionBlock {
                near: true,
                lod: LodLevel::Full,
                identities: vec![
                    NpcIdentity {
                        id: NpcId(3),
                        home_cell: (3, 0),
                        region: 0,
                        appearance_seed: 3,
                    },
                    NpcIdentity {
                        id: NpcId(7),
                        home_cell: (7, 0),
                        region: 0,
                        appearance_seed: 7,
                    },
                ],
                steps: vec![3, 7],
                routine_cursor: vec![1, 3],
                positions: vec![(3, 0), (7, 0)],
                represented_through: vec![engine_core::Tick(1), engine_core::Tick(2)],
                milestones: vec![0, 1],
                pending_signals: vec![0, 0],
                credits: vec![0, 2],
            },
            NpcRegionBlock {
                near: false,
                lod: LodLevel::Dormant,
                identities: vec![NpcIdentity {
                    id: NpcId(7),
                    home_cell: (7, 1),
                    region: 1,
                    appearance_seed: 77,
                }],
                steps: vec![7],
                routine_cursor: Vec::new(),
                positions: Vec::new(),
                represented_through: Vec::new(),
                milestones: vec![1],
                pending_signals: Vec::new(),
                credits: vec![0],
            },
        ],
    };
    let mut enc = Encoder::new();
    duplicate.save(&mut enc); // the raw codec is total: it encodes the invalid section
    let bytes = enc.into_bytes();
    assert!(matches!(
        NpcWorldSectionV2::restore(&mut Decoder::new(&bytes)),
        Err(SaveError::Duplicate { seq: 7, .. })
    ));
}
