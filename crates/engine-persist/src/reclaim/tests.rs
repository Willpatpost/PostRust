use super::*;
use crate::hash::hash_object;

fn sample_ledger() -> ReclamationLedger {
    ReclamationLedger {
        mk: hash_object(b"sample-mk"),
        sk: hash_object(b"sample-sk"),
        range_digest: [7; 32],
        seed: [9; 32],
        records: 3,
        reclaimed_keys: 2,
        journal_files: vec!["journal.t1".to_owned()],
        units: vec![
            ReclaimUnit {
                record: hash_object(b"sample-record-a"),
                objects: vec![hash_object(b"sample-object-a")],
            },
            ReclaimUnit {
                record: hash_object(b"sample-record-b"),
                objects: Vec::new(),
            },
            ReclaimUnit {
                record: hash_object(b"sample-record-c"),
                objects: vec![
                    hash_object(b"sample-object-c1"),
                    hash_object(b"sample-object-c2"),
                ],
            },
        ],
    }
}

#[test]
fn the_round_trip_is_byte_deterministic() {
    let ledger = sample_ledger();
    let bytes = ledger.canonical_bytes();
    let restored = ReclamationLedger::decode(&bytes).unwrap();
    assert_eq!(restored, ledger);
    assert_eq!(restored.canonical_bytes(), bytes);
}

#[test]
fn the_domain_and_version_are_refused() {
    let bytes = sample_ledger().canonical_bytes();
    let mut wrong_domain = Vec::new();
    let mut enc = Encoder::new();
    enc.write_str("sektor7/other/1");
    wrong_domain.extend_from_slice(&enc.into_bytes());
    wrong_domain.extend_from_slice(&bytes[8 + RECLAMATION_LEDGER_DOMAIN.len()..]);
    assert!(matches!(
        ReclamationLedger::decode(&wrong_domain),
        Err(SaveError::Domain { .. })
    ));

    let mut wrong_version = bytes.clone();
    let version_at = 8 + RECLAMATION_LEDGER_DOMAIN.len();
    wrong_version[version_at..version_at + 4].copy_from_slice(&2u32.to_le_bytes());
    assert!(matches!(
        ReclamationLedger::decode(&wrong_version),
        Err(SaveError::Version { observed: 2, .. })
    ));
}

#[test]
fn truncation_and_trailing_bytes_are_rejected() {
    let bytes = sample_ledger().canonical_bytes();
    for cut in 0..bytes.len() {
        assert!(
            ReclamationLedger::decode(&bytes[..cut]).is_err(),
            "a {cut}-byte prefix must not decode"
        );
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(matches!(
        ReclamationLedger::decode(&trailing),
        Err(SaveError::Trailing { .. })
    ));
}

#[test]
fn the_coherence_rules_are_enforced() {
    // The keys-within-records bound.
    let mut ledger = sample_ledger();
    ledger.reclaimed_keys = ledger.records + 1;
    assert!(matches!(
        ReclamationLedger::decode(&ledger.canonical_bytes()),
        Err(SaveError::Integrity(
            "reclamation-ledger/keys-exceed-records"
        ))
    ));
    // The zero identities (the struct form — the wire's
    // length-prefixed fields make byte patching fragile; a
    // zero-identity mk encodes and must refuse at decode).
    let mut ledger = sample_ledger();
    ledger.mk = Hash::ZERO;
    assert!(matches!(
        ReclamationLedger::decode(&ledger.canonical_bytes()),
        Err(SaveError::Integrity("reclamation-ledger/zero-identity"))
    ));
    // The manifest-count coherence (the audited record count must
    // equal the unit count).
    let mut ledger = sample_ledger();
    ledger.records = ledger.units.len() as u64 + 1;
    assert!(matches!(
        ReclamationLedger::decode(&ledger.canonical_bytes()),
        Err(SaveError::Integrity("reclamation-ledger/unit-count"))
    ));
}

#[test]
fn the_key_is_deterministic_in_mk() {
    let mk = hash_object(b"sample-mk");
    assert_eq!(
        reclamation_ledger_idempotency_key(&mk),
        reclamation_ledger_idempotency_key(&mk)
    );
    assert_ne!(
        reclamation_ledger_idempotency_key(&mk),
        reclamation_ledger_idempotency_key(&hash_object(b"other-mk"))
    );
}

// ------------------------- the K2-C2B ratified wire formats (P1)

fn sample_v2_ledger() -> ReclamationLedgerV2 {
    // The SHARED object appears in BOTH units — the legitimate
    // cross-unit repetition the sharing rules permit.
    let shared = hash_object(b"v2-shared-chunk");
    ReclamationLedgerV2 {
        mk: hash_object(b"v2-mk"),
        sk: hash_object(b"v2-sk"),
        batch: 0,
        range_digest: [3; 32],
        seed: [5; 32],
        records: 2,
        reclaimed_keys: 1,
        units: vec![
            ReclaimUnit {
                record: hash_object(b"v2-record-a"),
                objects: vec![hash_object(b"v2-object-a"), shared],
            },
            ReclaimUnit {
                record: hash_object(b"v2-record-b"),
                objects: vec![shared, hash_object(b"v2-object-b")],
            },
        ],
    }
}

#[test]
fn the_v2_ledger_round_trip_permits_cross_unit_sharing() {
    let ledger = sample_v2_ledger();
    let bytes = ledger.canonical_bytes();
    let restored = ReclamationLedgerV2::decode(&bytes).unwrap();
    assert_eq!(restored, ledger);
    assert_eq!(restored.canonical_bytes(), bytes);
}

#[test]
fn the_v2_ledger_refuses_the_domain_and_version() {
    let bytes = sample_v2_ledger().canonical_bytes();
    // The v1 domain is a different string — the domain gate
    // refuses it before any field decoding.
    let mut wrong = Vec::new();
    let mut enc = Encoder::new();
    enc.write_str(RECLAMATION_LEDGER_DOMAIN);
    wrong.extend_from_slice(&enc.into_bytes());
    wrong.extend_from_slice(&bytes[8 + RECLAMATION_LEDGER_V2_DOMAIN.len()..]);
    assert!(matches!(
        ReclamationLedgerV2::decode(&wrong),
        Err(SaveError::Domain { .. })
    ));
    let mut version = bytes.clone();
    let at = 8 + RECLAMATION_LEDGER_V2_DOMAIN.len();
    version[at..at + 4].copy_from_slice(&3u32.to_le_bytes());
    assert!(matches!(
        ReclamationLedgerV2::decode(&version),
        Err(SaveError::Version { observed: 3, .. })
    ));
}

#[test]
fn the_v2_ledger_refuses_truncation_and_trailing() {
    let bytes = sample_v2_ledger().canonical_bytes();
    for cut in 0..bytes.len() {
        assert!(
            ReclamationLedgerV2::decode(&bytes[..cut]).is_err(),
            "a {cut}-byte prefix must not decode"
        );
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(matches!(
        ReclamationLedgerV2::decode(&trailing),
        Err(SaveError::Trailing { .. })
    ));
}

#[test]
fn the_v2_ledger_enforces_the_sharing_rules() {
    // A duplicate unit RECORD across units is invalid.
    let mut ledger = sample_v2_ledger();
    ledger.units[1].record = ledger.units[0].record;
    assert!(matches!(
        ReclamationLedgerV2::decode(&ledger.canonical_bytes()),
        Err(SaveError::Integrity(
            "reclamation-ledger-v2/duplicate-unit-record"
        ))
    ));
    // A duplicate object WITHIN one unit is invalid (a record's
    // references and closure are sets).
    let mut ledger = sample_v2_ledger();
    let first = ledger.units[0].objects[0];
    ledger.units[0].objects.push(first);
    ledger.records = ledger.units.len() as u64;
    assert!(matches!(
        ReclamationLedgerV2::decode(&ledger.canonical_bytes()),
        Err(SaveError::Integrity(
            "reclamation-ledger-v2/duplicate-unit-object"
        ))
    ));
    // The count coherence and the keys bound.
    let mut ledger = sample_v2_ledger();
    ledger.records += 1;
    assert!(matches!(
        ReclamationLedgerV2::decode(&ledger.canonical_bytes()),
        Err(SaveError::Integrity("reclamation-ledger-v2/unit-count"))
    ));
    let mut ledger = sample_v2_ledger();
    ledger.reclaimed_keys = ledger.records + 1;
    assert!(matches!(
        ReclamationLedgerV2::decode(&ledger.canonical_bytes()),
        Err(SaveError::Integrity(
            "reclamation-ledger-v2/keys-exceed-records"
        ))
    ));
}

fn sample_plan() -> ReclaimPlan {
    ReclaimPlan {
        mk: hash_object(b"plan-mk"),
        sk: hash_object(b"plan-sk"),
        range_digest: [7; 32],
        seed: [9; 32],
        ledger_byte_limit: 512 * 1024 * 1024,
        batches: vec![
            PlanBatch {
                index: 0,
                records: vec![hash_object(b"plan-r1"), hash_object(b"plan-r2")],
            },
            PlanBatch {
                index: 1,
                records: vec![hash_object(b"plan-r3")],
            },
        ],
        journal_files: vec!["journal.t1.2".to_owned()],
    }
}

#[test]
fn the_plan_round_trip_and_total_units() {
    let plan = sample_plan();
    let bytes = plan.canonical_bytes();
    let restored = ReclaimPlan::decode(&bytes).unwrap();
    assert_eq!(restored, plan);
    assert_eq!(restored.canonical_bytes(), bytes);
    assert_eq!(restored.total_units(), 3);
}

#[test]
fn the_plan_enforces_the_partition_rules() {
    // A duplicate record ACROSS batches breaks the partition.
    let mut plan = sample_plan();
    let duplicated = plan.batches[0].records[0];
    plan.batches[1].records.push(duplicated);
    assert!(matches!(
        ReclaimPlan::decode(&plan.canonical_bytes()),
        Err(SaveError::Integrity("reclamation-plan/duplicate-record"))
    ));
    // The batch indices are strictly increasing from zero.
    let mut plan = sample_plan();
    plan.batches[0].index = 1;
    assert!(matches!(
        ReclaimPlan::decode(&plan.canonical_bytes()),
        Err(SaveError::Integrity("reclamation-plan/batch-index-order"))
    ));
    // No empty batch.
    let mut plan = sample_plan();
    plan.batches[1].records.clear();
    assert!(matches!(
        ReclaimPlan::decode(&plan.canonical_bytes()),
        Err(SaveError::Integrity("reclamation-plan/empty-batch"))
    ));
    // Truncation and trailing.
    let bytes = sample_plan().canonical_bytes();
    for cut in 0..bytes.len() {
        assert!(ReclaimPlan::decode(&bytes[..cut]).is_err());
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(matches!(
        ReclaimPlan::decode(&trailing),
        Err(SaveError::Trailing { .. })
    ));
}

fn sample_batch_marker() -> ReclamationComplete {
    ReclamationComplete {
        kind: CompletionKind::Batch,
        mk: hash_object(b"mark-mk"),
        batch: 1,
        source: hash_object(b"mark-ledger-record"),
        mandatory_count: 5,
        deferred: vec![hash_object(b"mark-deferred")],
        journal_files: Vec::new(),
    }
}

fn sample_range_marker() -> ReclamationComplete {
    ReclamationComplete {
        kind: CompletionKind::Range,
        mk: hash_object(b"mark-mk"),
        batch: 0,
        source: hash_object(b"mark-plan-record"),
        mandatory_count: 0,
        deferred: Vec::new(),
        journal_files: vec!["journal.t1.2".to_owned()],
    }
}

#[test]
fn the_marker_round_trips_both_kinds() {
    let batch = sample_batch_marker();
    let bytes = batch.canonical_bytes();
    let restored = ReclamationComplete::decode(&bytes).unwrap();
    assert_eq!(restored, batch);
    assert_eq!(restored.canonical_bytes(), bytes);
    let range = sample_range_marker();
    let bytes = range.canonical_bytes();
    let restored = ReclamationComplete::decode(&bytes).unwrap();
    assert_eq!(restored, range);
    assert_eq!(restored.canonical_bytes(), bytes);
}

#[test]
fn the_marker_refuses_malformed_forms() {
    // The bad kind discriminant.
    let bytes = sample_batch_marker().canonical_bytes();
    let mut bad_kind = bytes.clone();
    let at = 8 + RECLAMATION_COMPLETE_DOMAIN.len() + 4;
    bad_kind[at] = 7;
    assert!(matches!(
        ReclamationComplete::decode(&bad_kind),
        Err(SaveError::Enum { .. })
    ));
    // The duplicate deferred identity.
    let mut marker = sample_batch_marker();
    let first = marker.deferred[0];
    marker.deferred.push(first);
    assert!(matches!(
        ReclamationComplete::decode(&marker.canonical_bytes()),
        Err(SaveError::Integrity(
            "reclamation-complete/duplicate-deferred"
        ))
    ));
    // Truncation and trailing (both kinds).
    for bytes in [
        sample_batch_marker().canonical_bytes(),
        sample_range_marker().canonical_bytes(),
    ] {
        for cut in 0..bytes.len() {
            assert!(ReclamationComplete::decode(&bytes[..cut]).is_err());
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(matches!(
            ReclamationComplete::decode(&trailing),
            Err(SaveError::Trailing { .. })
        ));
    }
    // A range marker's wire decoded with the batch path's
    // expectations fails (the kind gates the field layout).
    let range_bytes = sample_range_marker().canonical_bytes();
    let mut as_batch = range_bytes.clone();
    let at = 8 + RECLAMATION_COMPLETE_DOMAIN.len() + 4;
    as_batch[at] = 0;
    assert!(ReclamationComplete::decode(&as_batch).is_err());
}

#[test]
fn the_ratified_keys_are_deterministic_and_distinct() {
    let mk = hash_object(b"key-mk");
    assert_eq!(
        reclamation_ledger_v2_idempotency_key(&mk, 0),
        reclamation_ledger_v2_idempotency_key(&mk, 0)
    );
    assert_ne!(
        reclamation_ledger_v2_idempotency_key(&mk, 0),
        reclamation_ledger_v2_idempotency_key(&mk, 1)
    );
    assert_ne!(
        reclamation_ledger_v2_idempotency_key(&mk, 0),
        reclamation_ledger_idempotency_key(&mk)
    );
    assert_eq!(
        reclamation_plan_idempotency_key(&mk),
        reclamation_plan_idempotency_key(&mk)
    );
    assert_ne!(
        reclamation_plan_idempotency_key(&mk),
        reclamation_ledger_v2_idempotency_key(&mk, 0)
    );
    assert_eq!(
        reclamation_complete_idempotency_key(&mk, Some(2)),
        reclamation_complete_idempotency_key(&mk, Some(2))
    );
    assert_ne!(
        reclamation_complete_idempotency_key(&mk, Some(2)),
        reclamation_complete_idempotency_key(&mk, None)
    );
}

#[test]
fn the_ledger_family_recognition_covers_every_version() {
    assert!(is_reclamation_ledger_family(RECLAMATION_LEDGER_DOMAIN));
    assert!(is_reclamation_ledger_family(RECLAMATION_LEDGER_V2_DOMAIN));
    assert!(is_reclamation_ledger_family(
        "engine-persist/reclamation-ledger/3"
    ));
    assert!(!is_reclamation_ledger_family(
        "engine-persist/reclamation-plan/1"
    ));
    assert!(!is_reclamation_ledger_family(
        "engine-slice/session-snapshot/2"
    ));
}

// ------------------ the P1-F1/F2/F3 review corrective

#[test]
fn the_marker_deferred_set_exceeding_the_single_unit_bound_decodes() {
    // P1-F1: a batch's combined distinct deferred set may
    // legitimately exceed the SINGLE-UNIT object bound
    // (69,632) while its ledger stays within the ratified byte
    // ceiling — the marker's decoder bound is derived from the
    // LEDGER ceiling, so this valid terminal evidence decodes.
    let deferred: Vec<Hash> = (0..69_700)
        .map(|index| hash_object(format!("deferred-{index}").as_bytes()))
        .collect();
    let marker = ReclamationComplete {
        kind: CompletionKind::Batch,
        mk: hash_object(b"mark-mk"),
        batch: 0,
        source: hash_object(b"mark-ledger-record"),
        mandatory_count: 1,
        deferred,
        journal_files: Vec::new(),
    };
    let bytes = marker.canonical_bytes();
    let restored = ReclamationComplete::decode(&bytes).unwrap();
    assert_eq!(restored.deferred.len(), 69_700);
    assert_eq!(restored.canonical_bytes(), bytes);
}

#[test]
fn the_marker_deferred_count_bound_refuses_the_derived_ceiling() {
    // The deferred-count bound itself: a declared count above
    // the ledger-derived ceiling refuses at the count (the
    // bounded read), before any identity is consumed.
    let mk = hash_object(b"mark-mk");
    let source = hash_object(b"mark-ledger-record");
    let mut enc = Encoder::new();
    enc.write_str(RECLAMATION_COMPLETE_DOMAIN);
    enc.write_u32(RECLAMATION_COMPLETE_VERSION);
    enc.write_u8(0);
    enc.write_bytes(mk.as_bytes());
    enc.write_u64(0);
    enc.write_bytes(source.as_bytes());
    enc.write_u64(0);
    enc.write_count((MAX_RECLAMATION_DEFERRED_IDENTITIES + 1) as usize);
    assert!(matches!(
        ReclamationComplete::decode(&enc.into_bytes()),
        Err(SaveError::Limit { .. })
    ));
}

#[test]
fn the_zero_identity_refusals_cover_objects_and_deferred() {
    // P1-F2: the v2 deletion-object identities and the marker's
    // deferred identities refuse the all-zero identity (the v1
    // decoder is unchanged).
    let mut ledger = sample_v2_ledger();
    ledger.units[0].objects[0] = Hash::ZERO;
    assert!(matches!(
        ReclamationLedgerV2::decode(&ledger.canonical_bytes()),
        Err(SaveError::Integrity("reclamation-ledger-v2/zero-identity"))
    ));
    let mut marker = sample_batch_marker();
    marker.deferred[0] = Hash::ZERO;
    assert!(matches!(
        ReclamationComplete::decode(&marker.canonical_bytes()),
        Err(SaveError::Integrity("reclamation-complete/zero-identity"))
    ));
}

#[test]
fn the_plan_aggregate_membership_is_bounded() {
    // P1-F3: the aggregate record count across ALL batches is
    // bounded by the checkpoint-index membership ceiling —
    // 65,538 records across two batches refuses.
    let over: Vec<Hash> = (0..32_769)
        .map(|index| hash_object(format!("over-{index}").as_bytes()))
        .collect();
    let plan = ReclaimPlan {
        mk: hash_object(b"plan-mk"),
        sk: hash_object(b"plan-sk"),
        range_digest: [1; 32],
        seed: [2; 32],
        ledger_byte_limit: 512 * 1024 * 1024,
        batches: vec![
            PlanBatch {
                index: 0,
                records: over.clone(),
            },
            PlanBatch {
                index: 1,
                records: over,
            },
        ],
        journal_files: Vec::new(),
    };
    assert!(matches!(
        ReclaimPlan::decode(&plan.canonical_bytes()),
        Err(SaveError::Integrity(
            "reclamation-plan/aggregate-membership"
        ))
    ));
}

#[test]
fn the_empty_plan_refuses() {
    // P1-F3: a zero-batch plan is not a legitimate ratified
    // state — a real retirement has at least one record.
    let mk = hash_object(b"plan-mk");
    let sk = hash_object(b"plan-sk");
    let mut enc = Encoder::new();
    enc.write_str(RECLAMATION_PLAN_DOMAIN);
    enc.write_u32(RECLAMATION_PLAN_VERSION);
    enc.write_bytes(mk.as_bytes());
    enc.write_bytes(sk.as_bytes());
    enc.write_bytes(&[0u8; 32]);
    enc.write_bytes(&[0u8; 32]);
    enc.write_u64(512 * 1024 * 1024);
    enc.write_count(0);
    enc.write_count(0);
    assert!(matches!(
        ReclaimPlan::decode(&enc.into_bytes()),
        Err(SaveError::Integrity("reclamation-plan/empty-plan"))
    ));
}

// --------------------------- the OC-C1 codec, gate, and
// digest-corrective regressions

fn sample_begin() -> ReclamationBegin {
    ReclamationBegin {
        mk: Hash::from_bytes(&[0x01; 32]).unwrap(),
        source: Hash::from_bytes(&[0x02; 32]).unwrap(),
        range_digest: [0x03; 32],
        seed: [0x04; 32],
    }
}

#[test]
fn the_begin_round_trip_is_byte_deterministic() {
    let begin = sample_begin();
    let bytes = begin.canonical_bytes();
    assert_eq!(bytes.len(), 206, "the ratified canonical length");
    let restored = ReclamationBegin::decode(&bytes).unwrap();
    assert_eq!(restored, begin);
    assert_eq!(restored.canonical_bytes(), bytes);
    // The golden vector: the exact content hash and the initial
    // byte layout (the 34-length prefix, the domain, the version).
    assert_eq!(
        hash_object(&bytes).hex(),
        "2a4348f3e1ec4f8aebb94ee751b92700e7a4bc57b697829f22d917d206ad9010"
    );
    assert_eq!(&bytes[..8], &[34, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(
        &bytes[8..42],
        b"engine-persist/reclamation-begin/1".as_slice()
    );
    assert_eq!(&bytes[42..46], &[1, 0, 0, 0]);
    assert_eq!(&bytes[46..54], &[32, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(bytes[54], 0x01);
}

#[test]
fn the_begin_deterministic_key_is_mk_bound() {
    let a = Hash::from_bytes(&[0x01; 32]).unwrap();
    let b = Hash::from_bytes(&[0x02; 32]).unwrap();
    assert_eq!(
        reclamation_begin_idempotency_key(&a),
        reclamation_begin_idempotency_key(&a)
    );
    assert_ne!(
        reclamation_begin_idempotency_key(&a),
        reclamation_begin_idempotency_key(&b)
    );
    assert_eq!(
        reclamation_begin_idempotency_key(&a),
        format!("engine-persist/reclamation-begin-key/1:{}", a.hex()).into_bytes()
    );
}

#[test]
fn the_begin_malformed_matrix_refuses_typed() {
    let begin = sample_begin();
    let bytes = begin.canonical_bytes();
    // The incorrect domain.
    let mut wrong = bytes.clone();
    wrong[9] = b'X';
    assert!(matches!(
        ReclamationBegin::decode(&wrong),
        Err(SaveError::Domain { .. })
    ));
    // The unsupported version.
    let mut wrong = bytes.clone();
    wrong[42] = 2;
    assert!(matches!(
        ReclamationBegin::decode(&wrong),
        Err(SaveError::Version { observed: 2, .. })
    ));
    // The MK undersized (31 declared): the length prefix shrinks
    // and the field's bytes shift — construct surgically.
    let undersized_mk = {
        let mut enc = Encoder::new();
        enc.write_str(RECLAMATION_BEGIN_DOMAIN);
        enc.write_u32(RECLAMATION_BEGIN_VERSION);
        enc.write_bytes(&begin.mk.as_bytes()[..31]);
        enc.write_bytes(begin.source.as_bytes());
        enc.write_bytes(&begin.range_digest);
        enc.write_bytes(&begin.seed);
        enc.into_bytes()
    };
    assert!(matches!(
        ReclamationBegin::decode(&undersized_mk),
        Err(SaveError::Integrity("reclamation-begin/hash-width"))
    ));
    // The MK oversized (33 declared): the limit check refuses.
    let oversized_mk = {
        let mut extended = begin.mk.as_bytes().to_vec();
        extended.push(0x01);
        let mut enc = Encoder::new();
        enc.write_str(RECLAMATION_BEGIN_DOMAIN);
        enc.write_u32(RECLAMATION_BEGIN_VERSION);
        enc.write_bytes(&extended);
        enc.write_bytes(begin.source.as_bytes());
        enc.write_bytes(&begin.range_digest);
        enc.write_bytes(&begin.seed);
        enc.into_bytes()
    };
    assert!(matches!(
        ReclamationBegin::decode(&oversized_mk),
        Err(SaveError::Limit { .. })
    ));
    // The source undersized/oversized: the same pair.
    let undersized_source = {
        let mut enc = Encoder::new();
        enc.write_str(RECLAMATION_BEGIN_DOMAIN);
        enc.write_u32(RECLAMATION_BEGIN_VERSION);
        enc.write_bytes(begin.mk.as_bytes());
        enc.write_bytes(&begin.source.as_bytes()[..31]);
        enc.write_bytes(&begin.range_digest);
        enc.write_bytes(&begin.seed);
        enc.into_bytes()
    };
    assert!(matches!(
        ReclamationBegin::decode(&undersized_source),
        Err(SaveError::Integrity("reclamation-begin/hash-width"))
    ));
    let oversized_source = {
        let mut extended = begin.source.as_bytes().to_vec();
        extended.push(0x02);
        let mut enc = Encoder::new();
        enc.write_str(RECLAMATION_BEGIN_DOMAIN);
        enc.write_u32(RECLAMATION_BEGIN_VERSION);
        enc.write_bytes(begin.mk.as_bytes());
        enc.write_bytes(&extended);
        enc.write_bytes(&begin.range_digest);
        enc.write_bytes(&begin.seed);
        enc.into_bytes()
    };
    assert!(matches!(
        ReclamationBegin::decode(&oversized_source),
        Err(SaveError::Limit { .. })
    ));
    // The digest undersized (31): the typed exact-width refusal —
    // never a panic (the OC-C1-E corrective).
    let undersized_digest = {
        let mut enc = Encoder::new();
        enc.write_str(RECLAMATION_BEGIN_DOMAIN);
        enc.write_u32(RECLAMATION_BEGIN_VERSION);
        enc.write_bytes(begin.mk.as_bytes());
        enc.write_bytes(begin.source.as_bytes());
        enc.write_bytes(&begin.range_digest[..31]);
        enc.write_bytes(&begin.seed);
        enc.into_bytes()
    };
    assert!(matches!(
        ReclamationBegin::decode(&undersized_digest),
        Err(SaveError::Integrity("reclamation-begin/digest-width"))
    ));
    // The digest oversized (33): the limit check.
    let oversized_digest = {
        let mut extended = begin.range_digest.to_vec();
        extended.push(0x03);
        let mut enc = Encoder::new();
        enc.write_str(RECLAMATION_BEGIN_DOMAIN);
        enc.write_u32(RECLAMATION_BEGIN_VERSION);
        enc.write_bytes(begin.mk.as_bytes());
        enc.write_bytes(begin.source.as_bytes());
        enc.write_bytes(&extended);
        enc.write_bytes(&begin.seed);
        enc.into_bytes()
    };
    assert!(matches!(
        ReclamationBegin::decode(&oversized_digest),
        Err(SaveError::Limit { .. })
    ));
    // The seed undersized: the typed width refusal.
    let undersized_seed = {
        let mut enc = Encoder::new();
        enc.write_str(RECLAMATION_BEGIN_DOMAIN);
        enc.write_u32(RECLAMATION_BEGIN_VERSION);
        enc.write_bytes(begin.mk.as_bytes());
        enc.write_bytes(begin.source.as_bytes());
        enc.write_bytes(&begin.range_digest);
        enc.write_bytes(&begin.seed[..31]);
        enc.into_bytes()
    };
    assert!(matches!(
        ReclamationBegin::decode(&undersized_seed),
        Err(SaveError::Integrity("reclamation-begin/digest-width"))
    ));
    // The seed oversized (33): the limit check.
    let oversized_seed = {
        let mut extended = begin.seed.to_vec();
        extended.push(0x04);
        let mut enc = engine_core::Encoder::new();
        enc.write_str(RECLAMATION_BEGIN_DOMAIN);
        enc.write_u32(RECLAMATION_BEGIN_VERSION);
        enc.write_bytes(begin.mk.as_bytes());
        enc.write_bytes(begin.source.as_bytes());
        enc.write_bytes(&begin.range_digest);
        enc.write_bytes(&extended);
        enc.into_bytes()
    };
    assert!(matches!(
        ReclamationBegin::decode(&oversized_seed),
        Err(SaveError::Limit { .. })
    ));
    // The zero identities.
    let zero_mk = ReclamationBegin {
        mk: Hash::ZERO,
        ..sample_begin()
    };
    assert!(matches!(
        ReclamationBegin::decode(&zero_mk.canonical_bytes()),
        Err(SaveError::Integrity("reclamation-begin/zero-identity"))
    ));
    let zero_source = ReclamationBegin {
        source: Hash::ZERO,
        ..sample_begin()
    };
    assert!(matches!(
        ReclamationBegin::decode(&zero_source.canonical_bytes()),
        Err(SaveError::Integrity("reclamation-begin/zero-identity"))
    ));
    // The truncated length prefix (the input ends mid-prefix):
    // the typed Truncated failure.
    assert!(matches!(
        ReclamationBegin::decode(&bytes[..50]),
        Err(SaveError::Truncated)
    ));
    // The truncated field content (the seed's content cut short
    // after a complete 32-length prefix): read_count's
    // remaining-bounds Limit failure — never a panic.
    assert!(matches!(
        ReclamationBegin::decode(&bytes[..205]),
        Err(SaveError::Limit { .. })
    ));
    // The single and multiple trailing bytes.
    let mut trailing_one = bytes.clone();
    trailing_one.push(0xAA);
    assert!(matches!(
        ReclamationBegin::decode(&trailing_one),
        Err(SaveError::Trailing { leftover: 1 })
    ));
    let mut trailing_many = bytes.clone();
    trailing_many.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
    assert!(matches!(
        ReclamationBegin::decode(&trailing_many),
        Err(SaveError::Trailing { leftover: 3 })
    ));
}

#[test]
fn the_persisted_ledger_digest_width_refuses_typed() {
    // (OC-C1-E) The preexisting read_digest defect: a ledger
    // whose declared digest length is undersized previously
    // REACHED copy_from_slice and panicked; the corrective
    // returns the typed Integrity failure. The oversized
    // declaration refuses at the limit check.
    let ledger = sample_ledger();
    let undersized = {
        let mut enc = Encoder::new();
        enc.write_str(RECLAMATION_LEDGER_DOMAIN);
        enc.write_u32(RECLAMATION_LEDGER_VERSION);
        enc.write_bytes(ledger.mk.as_bytes());
        enc.write_bytes(ledger.sk.as_bytes());
        enc.write_bytes(&ledger.range_digest[..31]);
        enc.write_bytes(&ledger.seed);
        enc.write_u64(ledger.records);
        enc.write_u64(ledger.reclaimed_keys);
        enc.write_count(ledger.journal_files.len());
        for name in &ledger.journal_files {
            enc.write_str(name);
        }
        enc.write_count(ledger.units.len());
        for unit in &ledger.units {
            enc.write_bytes(unit.record.as_bytes());
            enc.write_count(unit.objects.len());
            for object in &unit.objects {
                enc.write_bytes(object.as_bytes());
            }
        }
        enc.into_bytes()
    };
    assert!(matches!(
        ReclamationLedger::decode(&undersized),
        Err(SaveError::Integrity("reclamation/digest-width"))
    ));
    let oversized = {
        let mut extended = ledger.range_digest.to_vec();
        extended.push(0x07);
        let mut enc = Encoder::new();
        enc.write_str(RECLAMATION_LEDGER_DOMAIN);
        enc.write_u32(RECLAMATION_LEDGER_VERSION);
        enc.write_bytes(ledger.mk.as_bytes());
        enc.write_bytes(ledger.sk.as_bytes());
        enc.write_bytes(&extended);
        enc.write_bytes(&ledger.seed);
        enc.write_u64(ledger.records);
        enc.write_u64(ledger.reclaimed_keys);
        enc.write_count(ledger.journal_files.len());
        for name in &ledger.journal_files {
            enc.write_str(name);
        }
        enc.write_count(ledger.units.len());
        for unit in &ledger.units {
            enc.write_bytes(unit.record.as_bytes());
            enc.write_count(unit.objects.len());
            for object in &unit.objects {
                enc.write_bytes(object.as_bytes());
            }
        }
        enc.into_bytes()
    };
    assert!(matches!(
        ReclamationLedger::decode(&oversized),
        Err(SaveError::Limit { .. })
    ));
    // The valid ledger still round-trips (the compatibility
    // regression).
    let valid = ledger.canonical_bytes();
    assert_eq!(ReclamationLedger::decode(&valid).unwrap(), ledger);
}

#[test]
fn the_generic_append_refuses_the_begin_family() {
    use crate::StoredObject;
    use crate::local::LocalFileMedium;
    use crate::medium::CanonicalMedium;
    use crate::record::{AuthorityMode, RevisionRecord};
    let root = std::env::temp_dir().join(format!(
        "s7-occ1-gate-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut medium = LocalFileMedium::open(&root).unwrap();
    let plain = StoredObject::new({
        let mut enc = engine_core::Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"payload");
        enc.into_bytes()
    })
    .unwrap();
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "occ1", 0)
        .with_content_refs(vec![plain.hash()]);
    medium
        .append(&founding, std::slice::from_ref(&plain), None)
        .unwrap();
    let head = medium.head().unwrap().unwrap();
    let epoch = medium.read_record(&head).unwrap().epoch;
    // The ratified /1 begin object: the generic append refuses.
    let begin_object = StoredObject::new(sample_begin().canonical_bytes()).unwrap();
    let record =
        RevisionRecord::regular(head, epoch, 9).with_content_refs(vec![begin_object.hash()]);
    let error = medium
        .append(&record, std::slice::from_ref(&begin_object), Some(head))
        .unwrap_err();
    assert!(
        matches!(
            error,
            crate::medium::MediumError::Chain {
                rule: "reclamation-begin/generic-append-refused"
            }
        ),
        "the ratified begin refuses the generic append: {error:?}"
    );
    // An unsupported family variant (/2): equally refused.
    let unsupported = StoredObject::new({
        let mut enc = engine_core::Encoder::new();
        enc.write_str("engine-persist/reclamation-begin/2");
        enc.write_u32(1);
        enc.write_bytes(&[0x01; 32]);
        enc.write_bytes(&[0x02; 32]);
        enc.write_bytes(&[0x03; 32]);
        enc.write_bytes(&[0x04; 32]);
        enc.into_bytes()
    })
    .unwrap();
    let record =
        RevisionRecord::regular(head, epoch, 10).with_content_refs(vec![unsupported.hash()]);
    let error = medium
        .append(&record, std::slice::from_ref(&unsupported), Some(head))
        .unwrap_err();
    assert!(
        matches!(
            error,
            crate::medium::MediumError::Chain {
                rule: "reclamation-begin/generic-append-refused"
            }
        ),
        "the unsupported family variant refuses: {error:?}"
    );
    // The ordinary object publication still works.
    let record = RevisionRecord::regular(head, epoch, 11).with_content_refs(vec![plain.hash()]);
    let outcome = medium
        .append(&record, std::slice::from_ref(&plain), Some(head))
        .unwrap();
    assert!(matches!(
        outcome,
        crate::medium::AppendOutcome::Accepted { .. }
    ));
    // The existing completion-marker gate remains enforced.
    let marker = crate::reclaim::ReclamationComplete {
        kind: crate::reclaim::CompletionKind::Batch,
        mk: founding.id(),
        batch: 0,
        source: founding.id(),
        mandatory_count: 0,
        deferred: Vec::new(),
        journal_files: Vec::new(),
    };
    let marker_object = StoredObject::new(marker.canonical_bytes()).unwrap();
    let head2 = medium.head().unwrap().unwrap();
    let epoch2 = medium.read_record(&head2).unwrap().epoch;
    let record =
        RevisionRecord::regular(head2, epoch2, 12).with_content_refs(vec![marker_object.hash()]);
    let error = medium
        .append(&record, std::slice::from_ref(&marker_object), Some(head2))
        .unwrap_err();
    assert!(
        matches!(
            error,
            crate::medium::MediumError::Chain {
                rule: "reclamation-complete/generic-append-refused"
            }
        ),
        "the completion gate remains enforced: {error:?}"
    );
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_generic_append_refuses_every_completion_family_domain() {
    // (The C1-F1 corrective) The completion-marker gate covers
    // the WHOLE versioned family: the accepted V1 `/1`, the
    // C1-delivered V2 `/2` Batch marker (a GENUINE valid
    // marker — the committed classification fixture, never a
    // fabricated domain string), and an unrecognized future
    // family variant — all through the same typed refusal,
    // while unrelated content domains remain accepted and the
    // family classifier recognizes exactly the completion
    // domains.
    use crate::StoredObject;
    use crate::ancestry_accounting::StorageRole;
    use crate::batch_completion::{
        BatchCompletionContext, BatchCompletionMarkerV2, ClassificationEntry, EntryClassification,
        commit_classification,
    };
    use crate::hash::hash_object;
    use crate::local::LocalFileMedium;
    use crate::medium::CanonicalMedium;
    use crate::record::{AuthorityMode, RevisionRecord};
    let root = std::env::temp_dir().join(format!(
        "s7-c1f1-gate-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut medium = LocalFileMedium::open(&root).unwrap();
    let plain = StoredObject::new({
        let mut enc = engine_core::Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"payload");
        enc.into_bytes()
    })
    .unwrap();
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "c1f1", 0)
        .with_content_refs(vec![plain.hash()]);
    medium
        .append(&founding, std::slice::from_ref(&plain), None)
        .unwrap();
    // The family classifier's exact scope (the pure boundary):
    // every completion version is family, and no neighboring
    // namespace is.
    for domain in [
        crate::reclaim::RECLAMATION_COMPLETE_DOMAIN,
        crate::batch_completion::RECLAMATION_COMPLETE_V2_DOMAIN,
        "engine-persist/reclamation-complete/3",
    ] {
        assert!(crate::batch_completion::is_reclamation_complete_family(
            domain
        ));
    }
    for unrelated in [
        crate::reclaim::RECLAMATION_COMPLETE_KEY_DOMAIN,
        crate::batch_completion::RECLAMATION_COMPLETE_V2_KEY_DOMAIN,
        crate::reclaim::RECLAMATION_PLAN_DOMAIN,
        crate::reclaim::RECLAMATION_LEDGER_V2_DOMAIN,
        crate::reclaim::RECLAMATION_BEGIN_DOMAIN,
        crate::ancestry_witness::RECLAMATION_WITNESS_SHARD_DOMAIN,
    ] {
        assert!(!crate::batch_completion::is_reclamation_complete_family(
            unrelated
        ));
    }
    // The generic append refuses every family version with the
    // same typed rule.
    let mut refusal = |object: StoredObject, epoch_step: u64| {
        let head = medium.head().unwrap().unwrap();
        let epoch = medium.read_record(&head).unwrap().epoch;
        let record =
            RevisionRecord::regular(head, epoch, epoch_step).with_content_refs(vec![object.hash()]);
        medium
            .append(&record, std::slice::from_ref(&object), Some(head))
            .unwrap_err()
    };
    // The accepted V1 `/1` batch marker.
    let v1 = crate::reclaim::ReclamationComplete {
        kind: crate::reclaim::CompletionKind::Batch,
        mk: founding.id(),
        batch: 0,
        source: founding.id(),
        mandatory_count: 0,
        deferred: Vec::new(),
        journal_files: Vec::new(),
    };
    let error = refusal(StoredObject::new(v1.canonical_bytes()).unwrap(), 20);
    assert!(
        matches!(
            error,
            crate::medium::MediumError::Chain {
                rule: "reclamation-complete/generic-append-refused"
            }
        ),
        "the V1 /1 marker refuses the generic append: {error:?}"
    );
    // The GENUINE V2 `/2` Batch marker — the committed
    // classification over the four-entry fixture (a valid
    // marker by the C1 codec's own verification).
    let context = BatchCompletionContext {
        mk: hash_object(b"c1f1-mk"),
        plan: hash_object(b"c1f1-plan"),
        batch_index: 0,
        audit: hash_object(b"c1f1-audit"),
        begin: hash_object(b"c1f1-begin"),
    };
    let classification = vec![
        ClassificationEntry {
            identity: hash_object(b"c1f1-SK0"),
            role: StorageRole::Record,
            classification: EntryClassification::Mandatory,
            conditional_on_prior_batch: false,
        },
        ClassificationEntry {
            identity: hash_object(b"c1f1-m0a"),
            role: StorageRole::Record,
            classification: EntryClassification::Mandatory,
            conditional_on_prior_batch: false,
        },
        ClassificationEntry {
            identity: hash_object(b"c1f1-o0a"),
            role: StorageRole::Object,
            classification: EntryClassification::Mandatory,
            conditional_on_prior_batch: false,
        },
        ClassificationEntry {
            identity: hash_object(b"c1f1-os"),
            role: StorageRole::Object,
            classification: EntryClassification::Deferred,
            conditional_on_prior_batch: false,
        },
    ];
    let committed = commit_classification(&context, classification.clone()).unwrap();
    let v2 = BatchCompletionMarkerV2 {
        mk: context.mk,
        plan: context.plan,
        batch_index: context.batch_index,
        audit: context.audit,
        begin: context.begin,
        commitment: committed.digest,
        counters: committed.counters,
    };
    assert!(v2.verify_classification(classification).is_ok());
    let error = refusal(StoredObject::new(v2.canonical_bytes()).unwrap(), 21);
    assert!(
        matches!(
            error,
            crate::medium::MediumError::Chain {
                rule: "reclamation-complete/generic-append-refused"
            }
        ),
        "the valid V2 /2 Batch marker refuses the generic append: {error:?}"
    );
    // An unrecognized future family variant (`/3`): equally
    // refused — no future version can ride the generic path.
    let future = StoredObject::new({
        let mut enc = engine_core::Encoder::new();
        enc.write_str("engine-persist/reclamation-complete/3");
        enc.write_u32(3);
        enc.write_bytes(&[0x01; 32]);
        enc.into_bytes()
    })
    .unwrap();
    let error = refusal(future, 22);
    assert!(
        matches!(
            error,
            crate::medium::MediumError::Chain {
                rule: "reclamation-complete/generic-append-refused"
            }
        ),
        "the unrecognized future family variant refuses: {error:?}"
    );
    // The unrelated content domain still appends Accepted.
    let head = medium.head().unwrap().unwrap();
    let epoch = medium.read_record(&head).unwrap().epoch;
    let record = RevisionRecord::regular(head, epoch, 23).with_content_refs(vec![plain.hash()]);
    let outcome = medium
        .append(&record, std::slice::from_ref(&plain), Some(head))
        .unwrap();
    assert!(matches!(
        outcome,
        crate::medium::AppendOutcome::Accepted { .. }
    ));
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
#[cfg(feature = "composite-test-fixture")]
fn the_composite_admission_refuses_the_begin_family() {
    // (OC-C1-B, the alternative public path) The dedicated
    // composite admission cannot publish a begin-family ROOT
    // (the root must strictly decode as a composite manifest),
    // and a begin-family object cannot masquerade as a valid
    // composite CHUNK (the dependency verification fails).
    crate::composite_append::test_enable_composite_publication();
    use crate::StoredObject;
    use crate::local::LocalFileMedium;
    use crate::medium::CanonicalMedium;
    use crate::record::{AuthorityMode, RevisionRecord};
    let root = std::env::temp_dir().join(format!(
        "s7-occ1-comp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut medium = LocalFileMedium::open(&root).unwrap();
    let plain = StoredObject::new({
        let mut enc = engine_core::Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"payload");
        enc.into_bytes()
    })
    .unwrap();
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "occ1c", 0)
        .with_content_refs(vec![plain.hash()]);
    medium
        .append(&founding, std::slice::from_ref(&plain), None)
        .unwrap();
    let head = medium.head().unwrap().unwrap();
    let epoch = medium.read_record(&head).unwrap().epoch;
    let begin_object = StoredObject::new(sample_begin().canonical_bytes()).unwrap();
    // (1) The begin object as the composite ROOT: the admission
    // refuses (the root must be a manifest).
    let root_record =
        RevisionRecord::regular(head, epoch, 20).with_content_refs(vec![begin_object.hash()]);
    let request = crate::composite_append::CompositeAppend {
        record: &root_record,
        root: &begin_object,
        new_chunks: &[],
        reused_chunks: &[],
    };
    let error = medium.append_composite(&request, Some(head)).unwrap_err();
    assert!(
        matches!(
            error,
            crate::medium::MediumError::CompositeAdmission { .. }
                | crate::medium::MediumError::Format(_)
        ),
        "the begin root refuses the composite admission (the strict manifest decode): {error:?}"
    );
    // (2) The genuine chunk-dependency regression: a VALID
    // composite root manifest whose entry DECLARES the begin
    // object's content hash as its chunk identity, with the
    // begin object supplied as the corresponding new chunk —
    // the exact-set rule passes and the admission REACHES
    // verify_chunk_dependency, which refuses the begin payload
    // (it is not a session-chunk envelope). The authoritative
    // HEAD is unchanged and no rejected publication becomes
    // canonical.
    // The manifest follows the grammar's fixed section order
    // (the established fixture structure), with the begin
    // object's hash declared as the NEAR singleton's chunk
    // identity — the begin payload supplied as that chunk.
    let begin_bytes = sample_begin().canonical_bytes();
    let mut entries = Vec::new();
    let mut declared_chunks: Vec<StoredObject> = Vec::new();
    let push_singleton = |entries: &mut Vec<crate::composite_manifest::ManifestEntry>,
                          declared: &mut Vec<StoredObject>,
                          tag: &'static str,
                          payload: Vec<u8>| {
        let object = crate::composite_manifest::encode_chunk_envelope(tag, &payload);
        entries.push(crate::composite_manifest::ManifestEntry {
            tag,
            identity: object.hash(),
            payload_length: payload.len() as u64,
            kind: crate::composite_manifest::SectionKind::Singleton,
        });
        declared.push(object);
    };
    push_singleton(
        &mut entries,
        &mut declared_chunks,
        crate::composite_manifest::TAG_HEADER,
        vec![1u8; 11],
    );
    // The NEAR slot: the BEGIN object's identity declared as the
    // chunk identity, with the begin payload supplied for it (the
    // dependency verification receives the begin bytes).
    entries.push(crate::composite_manifest::ManifestEntry {
        tag: crate::composite_manifest::TAG_NEAR,
        identity: begin_object.hash(),
        payload_length: begin_bytes.len() as u64,
        kind: crate::composite_manifest::SectionKind::Singleton,
    });
    push_singleton(
        &mut entries,
        &mut declared_chunks,
        crate::composite_manifest::TAG_FAR,
        vec![2u8; 12],
    );
    push_singleton(
        &mut entries,
        &mut declared_chunks,
        crate::composite_manifest::TAG_BLOCKED,
        vec![3u8; 13],
    );
    push_singleton(
        &mut entries,
        &mut declared_chunks,
        crate::composite_manifest::TAG_HELD,
        vec![4u8; 14],
    );
    push_singleton(
        &mut entries,
        &mut declared_chunks,
        crate::composite_manifest::TAG_LEDGER_META,
        vec![5u8; 15],
    );
    let tail_payload = vec![6u8; 62];
    let tail = crate::composite_manifest::encode_chunk_envelope(
        crate::composite_manifest::TAG_LEDGER_RECORDS,
        &tail_payload,
    );
    entries.push(crate::composite_manifest::ManifestEntry {
        tag: crate::composite_manifest::TAG_LEDGER_RECORDS,
        identity: tail.hash(),
        payload_length: tail_payload.len() as u64,
        kind: crate::composite_manifest::SectionKind::LedgerTail,
    });
    declared_chunks.push(tail);
    push_singleton(
        &mut entries,
        &mut declared_chunks,
        crate::composite_manifest::TAG_WORLD,
        vec![7u8; 16],
    );
    push_singleton(
        &mut entries,
        &mut declared_chunks,
        crate::composite_manifest::TAG_DIRECTOR,
        vec![8u8; 17],
    );
    let manifest = crate::composite_manifest::CompositeManifest {
        history_kind: crate::composite_manifest::HistoryKind::Full,
        ledger_boundary: 4,
        composed_length: entries.iter().map(|e| e.payload_length).sum(),
        entries,
    };
    let manifest_root = StoredObject::new(crate::composite_manifest::encode(&manifest)).unwrap();
    let chunk_record =
        RevisionRecord::regular(head, epoch, 21).with_content_refs(vec![manifest_root.hash()]);
    // The supplied set covers every declared identity exactly
    // (the exact-set rule): the eight constructed chunks plus the
    // begin payload standing in for the NEAR slot.
    let mut new_chunks = declared_chunks;
    new_chunks.push(begin_object.clone());
    let request = crate::composite_append::CompositeAppend {
        record: &chunk_record,
        root: &manifest_root,
        new_chunks: &new_chunks,
        reused_chunks: &[],
    };
    let error = medium.append_composite(&request, Some(head)).unwrap_err();
    let detail = format!("{error:?}");
    assert!(
        detail.contains("could not be established")
            && detail.contains("engine-persist/reclamation-begin/1"),
        "the chunk-DEPENDENCY validator refuses the begin payload at the envelope's foreign-domain check (past the valid manifest and the exact-set rule): {detail}"
    );
    // No rejected publication becomes canonical: the HEAD stands
    // and the record is absent.
    assert_eq!(
        medium.head().unwrap().unwrap(),
        head,
        "the authoritative HEAD is unchanged by the refusal"
    );
    assert!(
        medium.read_record(&chunk_record.id()).is_err(),
        "the rejected publication never became canonical"
    );
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}

// (F-14) The construction-boundary regression at the v1 ledger
// publication: the non-enforcing provider model (a crafted
// well-formed head record at epoch u64::MAX — see
// crate::medium::epoch_audit_media) refuses the publication
// TYPED at the construction boundary — never the constructor's
// debug panic or the release wrap — with nothing published.
#[test]
fn the_v1_ledger_publication_refuses_a_saturated_head_epoch_typed() {
    use crate::local::LocalFileMedium;
    use crate::medium::CanonicalMedium;
    use crate::record::{AuthorityMode, RevisionRecord};
    let root = std::env::temp_dir().join(format!(
        "s7-f14-ledger-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut medium = LocalFileMedium::open(&root).unwrap();
    let plain = StoredObject::new({
        let mut enc = engine_core::Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"f14-ledger-payload");
        enc.into_bytes()
    })
    .unwrap();
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "f14-ledger", 0)
        .with_content_refs(vec![plain.hash()]);
    medium
        .append(&founding, std::slice::from_ref(&plain), None)
        .unwrap();
    let mut wrapped = crate::medium::epoch_audit_media::MaxEpochHeadMedium::over(medium);
    let error = publish_reclamation_ledger(&mut wrapped, &sample_ledger(), 7).unwrap_err();
    assert!(
        matches!(
            error,
            MediumError::Chain {
                rule: "epoch-regression"
            }
        ),
        "the saturated head refuses the v1 ledger publication typed: {error:?}"
    );
    assert_eq!(
        wrapped.true_head().unwrap(),
        Some(founding.id()),
        "the refusal published nothing"
    );
    drop(wrapped);
    let _ = std::fs::remove_dir_all(&root);
}

/// P3-L2's inversion guard: the manifest-closure loop ADDS the
/// closure's chunk identities. The lane's first form used
/// `!seen.insert(...)` — inverted against `HashSet::insert`'s
/// returns-true-when-new semantics — which silently DROPPED
/// every closure entry (the collected set would hold only the
/// content refs). The pre-collected-duplicate half of the
/// original dedupe is unreachable by construction (refs
/// overlapping closure identities refuse at duplicate-unit-
/// object; manifest-level duplicates refuse at decode) — the
/// load-bearing property is that every closure entry lands.
#[test]
fn closure_entries_are_all_collected() {
    use crate::composite_manifest::{
        CompositeManifest, HistoryKind, ManifestEntry, SectionKind, TAG_HEADER, TAG_LEDGER_META,
        encode as encode_manifest,
    };
    use crate::hash::hash_object;
    use crate::record::RevisionRecord;
    use std::collections::HashMap;

    struct FixedReads {
        record_bytes: Vec<u8>,
        objects: HashMap<crate::hash::Hash, Vec<u8>>,
    }
    impl super::BeginReads for FixedReads {
        fn read_record_bytes(
            &mut self,
            _id: &crate::hash::Hash,
        ) -> Result<Vec<u8>, crate::medium::MediumError> {
            Ok(self.record_bytes.clone())
        }
        fn read_object_bytes(
            &mut self,
            hash: &crate::hash::Hash,
        ) -> Result<Vec<u8>, crate::medium::MediumError> {
            self.objects
                .get(hash)
                .cloned()
                .ok_or(crate::medium::MediumError::MissingObject { hash: *hash })
        }
    }

    // Chunks first: a session-chunk envelope (domain, tag,
    // payload); the manifest declares each chunk's content hash.
    let chunk = |tag: &'static str| {
        let mut enc = engine_core::Encoder::new();
        enc.write_str(crate::composite_manifest::SESSION_CHUNK_DOMAIN);
        enc.write_str(tag);
        enc.write_bytes(&[]);
        let bytes = enc.into_bytes();
        (hash_object(&bytes), bytes)
    };
    let (header_id, header_bytes) = chunk(TAG_HEADER);
    let (near_id, near_bytes) = chunk(crate::composite_manifest::TAG_NEAR);
    let (far_id, far_bytes) = chunk(crate::composite_manifest::TAG_FAR);
    let (blocked_id, blocked_bytes) = chunk(crate::composite_manifest::TAG_BLOCKED);
    let (held_id, held_bytes) = chunk(crate::composite_manifest::TAG_HELD);
    let (ledger_meta_id, ledger_meta_bytes) = chunk(TAG_LEDGER_META);
    let (ledger_id, ledger_bytes) = chunk(crate::composite_manifest::TAG_LEDGER_RECORDS);
    let (world_id, world_bytes) = chunk(crate::composite_manifest::TAG_WORLD);
    let (director_id, director_bytes) = chunk(crate::composite_manifest::TAG_DIRECTOR);
    let entry = |tag: &'static str, identity| ManifestEntry {
        tag,
        identity,
        payload_length: 0,
        kind: SectionKind::Singleton,
    };
    let manifest = CompositeManifest {
        history_kind: HistoryKind::Full,
        ledger_boundary: 1,
        composed_length: 0,
        entries: vec![
            entry(TAG_HEADER, header_id),
            entry(crate::composite_manifest::TAG_NEAR, near_id),
            entry(crate::composite_manifest::TAG_FAR, far_id),
            entry(crate::composite_manifest::TAG_BLOCKED, blocked_id),
            entry(crate::composite_manifest::TAG_HELD, held_id),
            entry(TAG_LEDGER_META, ledger_meta_id),
            ManifestEntry {
                tag: crate::composite_manifest::TAG_LEDGER_RECORDS,
                identity: ledger_id,
                payload_length: 0,
                kind: SectionKind::LedgerTail,
            },
            entry(crate::composite_manifest::TAG_WORLD, world_id),
            entry(crate::composite_manifest::TAG_DIRECTOR, director_id),
        ],
    };
    let manifest_bytes = encode_manifest(&manifest);
    let manifest_identity = hash_object(&manifest_bytes);

    // The production shape: the record's single content ref is
    // the manifest (the root-only snapshot binding).
    let parent = hash_object(b"closure-inversion-parent");
    let record = RevisionRecord::regular(parent, 0, 1).with_content_refs(vec![manifest_identity]);
    let record_bytes = record.canonical_bytes();

    let mut reads = FixedReads {
        record_bytes,
        objects: HashMap::from([
            (manifest_identity, manifest_bytes),
            (header_id, header_bytes),
            (near_id, near_bytes),
            (far_id, far_bytes),
            (blocked_id, blocked_bytes),
            (held_id, held_bytes),
            (ledger_meta_id, ledger_meta_bytes),
            (ledger_id, ledger_bytes),
            (world_id, world_bytes),
            (director_id, director_bytes),
        ]),
    };
    let record_id = hash_object(b"closure-inversion-record");
    let collected =
        super::collect_unit_objects_via(&mut reads, &record_id).expect("the fixture reads resolve");
    // The ref contributes the manifest; the closure adds ALL
    // NINE declared chunk identities.
    assert_eq!(
        collected.len(),
        10,
        "1 ref + 9 closure entries — none silently dropped"
    );
    assert_eq!(collected[0], manifest_identity);
    for id in [
        header_id,
        near_id,
        far_id,
        blocked_id,
        held_id,
        ledger_meta_id,
        ledger_id,
        world_id,
        director_id,
    ] {
        assert!(
            collected.contains(&id),
            "every closure entry is collected (the inverted form dropped them all)"
        );
    }
}
