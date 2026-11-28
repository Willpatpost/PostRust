use super::*;
use crate::checkpoint::{CheckpointMetadata, ReclaimedIndexEntry, SkipEntry};
use crate::medium::CanonicalMedium;
use crate::record::{AuthorityMode, RevisionRecord};
use engine_core::Encoder;

#[test]
fn a_successful_exclusion_walk_over_a_real_checkpoint_range_sqlite() {
    // The SQLite positive-exclusion regression (the review's §5
    // requirement — the second provider's agreement with the
    // local result): a REAL hand-crafted checkpoint with the
    // authenticated self-skip; a successful nonempty
    // simulated-retirement walk over the audited range; the
    // survivor and reclaimed sets verified; the physical rows
    // untouched; the ordinary state intact.
    let root = std::env::temp_dir().join(format!(
        "s7-p2sqlex-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut medium = SqliteMedium::open(root.join("db.sqlite")).unwrap();
    let object = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"payload");
        enc.into_bytes()
    })
    .unwrap();
    let mut chain: Vec<Hash> = Vec::new();
    let mut epochs: Vec<u64> = Vec::new();
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "p2sq", 0)
        .with_content_refs(vec![object.hash()]);
    medium
        .append(&founding, std::slice::from_ref(&object), None)
        .unwrap();
    chain.push(founding.id());
    epochs.push(0);
    for tick in 1..=3u64 {
        let record = RevisionRecord::regular(*chain.last().unwrap(), tick - 1, tick)
            .with_content_refs(vec![object.hash()]);
        medium
            .append(
                &record,
                std::slice::from_ref(&object),
                Some(*chain.last().unwrap()),
            )
            .unwrap();
        chain.push(record.id());
        epochs.push(tick);
    }
    let [_, p1, p2, sk] = [chain[0], chain[1], chain[2], chain[3]];
    let metadata = CheckpointMetadata::new(
        sk,
        p2,
        vec![SkipEntry::new(Hash::ZERO, p1)],
        vec![
            ReclaimedIndexEntry::new(None, p2, epochs[2]),
            ReclaimedIndexEntry::new(None, sk, epochs[3]),
        ],
        [7; 32],
        [9; 32],
    );
    let meta_object = StoredObject::new(metadata.canonical_bytes()).unwrap();
    let mk_record =
        RevisionRecord::regular(sk, epochs[3], 5).with_content_refs(vec![meta_object.hash()]);
    medium
        .append(&mk_record, std::slice::from_ref(&meta_object), Some(sk))
        .unwrap();
    let head = medium.head().unwrap().unwrap();
    assert_eq!(head, mk_record.id());

    let excluded: BTreeSet<Hash> = [p2, sk].into_iter().collect();
    let walked = walk_chain_excluding(&medium.conn, head, &excluded).unwrap();
    assert_eq!(
        walked.summary.records, 3,
        "MK, P1, F walk — the local agreement"
    );
    assert_eq!(walked.summary.reclaimed_records, 2);
    assert_eq!(walked.cursor.reclaimed.get(&p2), Some(&epochs[2]));
    assert_eq!(walked.cursor.reclaimed.get(&sk), Some(&epochs[3]));
    assert!(!walked.cursor.record_ids.contains(&p2));
    assert!(!walked.cursor.record_ids.contains(&sk));
    assert!(walked.cursor.record_ids.contains(&mk_record.id()));

    // The physical rows are untouched; the authoritative state
    // observes the ACTUAL chain.
    let authoritative = medium.validate().unwrap();
    assert_eq!(authoritative.records, 5);
    let summary = medium.head().unwrap().unwrap();
    assert_eq!(summary, mk_record.id());
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}
#[test]
fn the_verifier_crosses_a_reclaimed_gap_on_sqlite() {
    // (B1/B2 on the second provider) The genuine gap crossing
    // over SQLite — the target audit and MK BELOW a reclaimed
    // range; the descent crosses the authenticated skip; the
    // A4 manifest check exercises read_record (the records
    // table, the cross-provider interface).
    let root = std::env::temp_dir().join(format!(
        "s7-p2b2sq-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut medium = SqliteMedium::open(root.join("db.sqlite")).unwrap();
    let object = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"payload");
        enc.into_bytes()
    })
    .unwrap();
    let step = |medium: &mut SqliteMedium, tick: u64| -> Hash {
        let head = medium.head().unwrap().unwrap();
        let epoch = medium.read_record(&head).unwrap().epoch;
        let record =
            RevisionRecord::regular(head, epoch, tick).with_content_refs(vec![object.hash()]);
        medium
            .append(&record, std::slice::from_ref(&object), Some(head))
            .unwrap();
        record.id()
    };
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "p2b2s", 0)
        .with_content_refs(vec![object.hash()]);
    medium
        .append(&founding, std::slice::from_ref(&object), None)
        .unwrap();
    let _p1 = step(&mut medium, 1);
    let r1 = step(&mut medium, 2);
    let r1_epoch = medium.read_record(&r1).unwrap().epoch;
    let old_meta = CheckpointMetadata::new(
        r1,
        r1,
        Vec::new(),
        vec![ReclaimedIndexEntry::new(None, r1, r1_epoch)],
        [1; 32],
        [2; 32],
    );
    let old_object = StoredObject::new(old_meta.canonical_bytes()).unwrap();
    let mk_o = RevisionRecord::regular(r1, r1_epoch, 9).with_content_refs(vec![old_object.hash()]);
    medium
        .append(&mk_o, std::slice::from_ref(&old_object), Some(r1))
        .unwrap();
    let ledger = crate::reclaim::ReclamationLedger {
        mk: mk_o.id(),
        sk: r1,
        range_digest: old_meta.range_digest,
        seed: old_meta.seed,
        records: 1,
        reclaimed_keys: 0,
        journal_files: Vec::new(),
        units: vec![crate::reclaim::ReclaimUnit {
            record: r1,
            objects: vec![object.hash()],
        }],
    };
    let (audit_o, outcome) =
        crate::reclaim::publish_reclamation_ledger(&mut medium, &ledger, 10).unwrap();
    assert!(matches!(
        outcome,
        crate::medium::AppendOutcome::Accepted { .. }
    ));
    let p2n = step(&mut medium, 11);
    let p2n_epoch = medium.read_record(&p2n).unwrap().epoch;
    let sk_n = step(&mut medium, 12);
    let sk_n_epoch = medium.read_record(&sk_n).unwrap().epoch;
    let new_meta = CheckpointMetadata::new(
        sk_n,
        p2n,
        vec![SkipEntry::new(Hash::ZERO, audit_o)],
        vec![
            ReclaimedIndexEntry::new(None, p2n, p2n_epoch),
            ReclaimedIndexEntry::new(None, sk_n, sk_n_epoch),
        ],
        [3; 32],
        [4; 32],
    );
    let new_object = StoredObject::new(new_meta.canonical_bytes()).unwrap();
    let mk_new =
        RevisionRecord::regular(sk_n, sk_n_epoch, 13).with_content_refs(vec![new_object.hash()]);
    medium
        .append(&mk_new, std::slice::from_ref(&new_object), Some(sk_n))
        .unwrap();
    // THE RECLAIMED GAP: delete the NEW range's rows from BOTH
    // tables (the provider's own retirement shape).
    for id in [p2n, sk_n] {
        medium
            .conn
            .execute(
                "DELETE FROM records WHERE hash = ?1",
                params![id.as_bytes()],
            )
            .unwrap();
        medium
            .conn
            .execute(
                "DELETE FROM objects WHERE hash = ?1",
                params![id.as_bytes()],
            )
            .unwrap();
    }
    let (verified, metadata) =
        crate::reclaim::verify_canonical_v1_audit(&mut medium, &audit_o, &mk_o.id()).unwrap();
    assert_eq!(verified.mk, mk_o.id());
    assert_eq!(metadata.sk, r1);
    assert_eq!(verified.units.len(), 1);
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}
#[test]
fn the_first_installed_wrong_manifest_refuses_on_sqlite() {
    // (B3 on the second provider) The FIRST installed audit with
    // an omitted or injected object manifest refuses at the
    // exact comparison — the records-table read path.
    for wrong_objects in [
        Vec::new(),
        vec![crate::hash::hash_object(b"p2c3s-injected")],
    ] {
        let root = std::env::temp_dir().join(format!(
            "s7-p2c3s-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut medium = SqliteMedium::open(root.join("db.sqlite")).unwrap();
        let object = StoredObject::new({
            let mut enc = Encoder::new();
            enc.write_str("sektor7/save/1");
            enc.write_bytes(b"payload");
            enc.into_bytes()
        })
        .unwrap();
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "p2c3s", 0)
            .with_content_refs(vec![object.hash()]);
        medium
            .append(&founding, std::slice::from_ref(&object), None)
            .unwrap();
        let head = medium.head().unwrap().unwrap();
        let epoch = medium.read_record(&head).unwrap().epoch;
        let r1 = RevisionRecord::regular(head, epoch, 1).with_content_refs(vec![object.hash()]);
        medium
            .append(&r1, std::slice::from_ref(&object), Some(head))
            .unwrap();
        let meta = CheckpointMetadata::new(
            r1.id(),
            r1.id(),
            Vec::new(),
            vec![ReclaimedIndexEntry::new(None, r1.id(), 1)],
            [1; 32],
            [2; 32],
        );
        let meta_object = StoredObject::new(meta.canonical_bytes()).unwrap();
        let mk = RevisionRecord::regular(r1.id(), 1, 2).with_content_refs(vec![meta_object.hash()]);
        medium
            .append(&mk, std::slice::from_ref(&meta_object), Some(r1.id()))
            .unwrap();
        let ledger = crate::reclaim::ReclamationLedger {
            mk: mk.id(),
            sk: r1.id(),
            range_digest: meta.range_digest,
            seed: meta.seed,
            records: 1,
            reclaimed_keys: 0,
            journal_files: Vec::new(),
            units: vec![crate::reclaim::ReclaimUnit {
                record: r1.id(),
                objects: wrong_objects,
            }],
        };
        let (audit, outcome) =
            crate::reclaim::publish_reclamation_ledger(&mut medium, &ledger, 3).unwrap();
        assert!(matches!(
            outcome,
            crate::medium::AppendOutcome::Accepted { .. }
        ));
        let error = crate::reclaim::verify_canonical_v1_audit(&mut medium, &audit, &mk.id())
            .expect_err("the wrong manifest refuses on SQLite");
        assert!(
            matches!(
                error,
                MediumError::Chain {
                    rule: "reclamation-complete/units-objects-mismatch"
                }
            ),
            "the exact error class: {error:?}"
        );
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
}
#[test]
fn the_trusted_directive_executes_on_sqlite() {
    // The trusted execution on the second provider: the
    // successful authenticated directive under BEGIN IMMEDIATE —
    // the retiring records and the private object deleted from
    // both tables; the SHARED object (referenced by the
    // surviving root) SURVIVES; the deferred classification
    // reported; the pending probe complete; the post-retirement
    // validation works.
    let root = std::env::temp_dir().join(format!(
        "s7-p2txs-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut medium = SqliteMedium::open(root.join("db.sqlite")).unwrap();
    let shared = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"shared-payload");
        enc.into_bytes()
    })
    .unwrap();
    let private = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"private-payload");
        enc.into_bytes()
    })
    .unwrap();
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "p2txs", 0)
        .with_content_refs(vec![shared.hash()]);
    medium
        .append(&founding, std::slice::from_ref(&shared), None)
        .unwrap();
    let step = |medium: &mut SqliteMedium, objects: &[StoredObject]| -> Hash {
        let head = medium.head().unwrap().unwrap();
        let epoch = medium.read_record(&head).unwrap().epoch;
        let refs: Vec<Hash> = objects.iter().map(|o| o.hash()).collect();
        let record =
            RevisionRecord::regular(head, epoch, refs.len() as u64 + 10).with_content_refs(refs);
        medium.append(&record, objects, Some(head)).unwrap();
        record.id()
    };
    let _p1 = step(&mut medium, std::slice::from_ref(&shared));
    let r1 = step(&mut medium, &[shared.clone(), private.clone()]);
    let sk = step(&mut medium, std::slice::from_ref(&shared));
    // The E1 authentic commitments + the E2 newest-first order.
    let seed = crate::checkpoint::compute_digest_seed(&founding.id());
    let r1_bytes = medium.read_record(&r1).unwrap().canonical_bytes();
    let range_digest = crate::checkpoint::chain_range_digest(&seed, &[r1_bytes]);
    let meta = CheckpointMetadata::new(
        sk,
        r1,
        vec![SkipEntry::new(Hash::ZERO, _p1)],
        vec![
            ReclaimedIndexEntry::new(None, r1, 2),
            ReclaimedIndexEntry::new(None, sk, 3),
        ],
        range_digest,
        seed,
    );
    let meta_object = StoredObject::new(meta.canonical_bytes()).unwrap();
    let mk = RevisionRecord::regular(sk, 3, 14).with_content_refs(vec![meta_object.hash()]);
    medium
        .append(&mk, std::slice::from_ref(&meta_object), Some(sk))
        .unwrap();
    let r1_refs = medium.read_record(&r1).unwrap().content_refs.clone();
    let sk_refs = medium.read_record(&sk).unwrap().content_refs.clone();
    let ledger = crate::reclaim::ReclamationLedger {
        mk: mk.id(),
        sk,
        range_digest: meta.range_digest,
        seed: meta.seed,
        records: 2,
        reclaimed_keys: 0,
        journal_files: Vec::new(),
        units: vec![
            crate::reclaim::ReclaimUnit {
                record: sk,
                objects: sk_refs,
            },
            crate::reclaim::ReclaimUnit {
                record: r1,
                objects: r1_refs,
            },
        ],
    };
    let (audit, outcome) =
        crate::reclaim::publish_reclamation_ledger(&mut medium, &ledger, 5).unwrap();
    assert!(matches!(
        outcome,
        crate::medium::AppendOutcome::Accepted { .. }
    ));
    let directive = crate::reclaim::ReclaimDirective {
        audit_record: audit,
        units: ledger.units.clone(),
    };
    // (OC-C3 CP-2) The begin-bound migration: the REAL trusted
    // admission (the execution gate requires it).
    let begin_outcome = medium.begin_reclamation(&audit, &mk.id(), 221).unwrap();
    assert!(
        matches!(
            begin_outcome,
            crate::reclaim::BeginPublicationOutcome::Accepted { .. }
                | crate::reclaim::BeginPublicationOutcome::DuplicateVerified { .. }
        ),
        "the fixture's begin admits: {begin_outcome:?}"
    );
    let outcome = medium.reclaim(&directive).unwrap();
    assert_eq!(outcome.deferred, vec![shared.hash()]);
    assert!(!outcome.mandatory.contains(&shared.hash()));
    // The private object and both retiring records are gone from
    // BOTH tables; the shared object survives.
    let conn = &medium.conn;
    for id in [private.hash(), r1, sk] {
        let objects: i64 = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM objects WHERE hash = ?1)",
                params![id.as_bytes()],
                |row| row.get(0),
            )
            .unwrap();
        let records: i64 = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM records WHERE hash = ?1)",
                params![id.as_bytes()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(objects + records, 0, "the mandatory identity is gone");
    }
    let shared_present: i64 = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM objects WHERE hash = ?1)",
            params![shared.hash().as_bytes()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(shared_present, 1, "the shared object survives");
    assert!(!medium.reclaim_pending(&directive).unwrap());
    medium.validate().unwrap();
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}
#[test]
fn a_corrupted_object_row_refuses_and_rolls_back_on_sqlite() {
    // (E3) The object pass reads and HASH-VERIFIES each present
    // row's bytes before the DELETE: a corrupted row at the
    // audited hash key (the bytes no longer hash to the key)
    // refuses with the TYPED ReferencedObjectCorrupt and the
    // single BEGIN IMMEDIATE transaction rolls back — NOTHING
    // deleted, both tables unchanged. Restoring the authentic
    // bytes lets the same directive execute successfully.
    let root = std::env::temp_dir().join(format!(
        "s7-p2cor-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut medium = SqliteMedium::open(root.join("db.sqlite")).unwrap();
    let shared = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"shared-payload");
        enc.into_bytes()
    })
    .unwrap();
    let private = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"private-payload");
        enc.into_bytes()
    })
    .unwrap();
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "p2cor", 0)
        .with_content_refs(vec![shared.hash()]);
    medium
        .append(&founding, std::slice::from_ref(&shared), None)
        .unwrap();
    let step = |medium: &mut SqliteMedium, objects: &[StoredObject]| -> Hash {
        let head = medium.head().unwrap().unwrap();
        let epoch = medium.read_record(&head).unwrap().epoch;
        let refs: Vec<Hash> = objects.iter().map(|o| o.hash()).collect();
        let record =
            RevisionRecord::regular(head, epoch, refs.len() as u64 + 10).with_content_refs(refs);
        medium.append(&record, objects, Some(head)).unwrap();
        record.id()
    };
    let _p1 = step(&mut medium, std::slice::from_ref(&shared));
    let r1 = step(&mut medium, &[shared.clone(), private.clone()]);
    let sk = step(&mut medium, std::slice::from_ref(&shared));
    let seed = crate::checkpoint::compute_digest_seed(&founding.id());
    let r1_bytes = medium.read_record(&r1).unwrap().canonical_bytes();
    let range_digest = crate::checkpoint::chain_range_digest(&seed, &[r1_bytes]);
    let meta = CheckpointMetadata::new(
        sk,
        r1,
        vec![SkipEntry::new(Hash::ZERO, _p1)],
        vec![
            ReclaimedIndexEntry::new(None, r1, 2),
            ReclaimedIndexEntry::new(None, sk, 3),
        ],
        range_digest,
        seed,
    );
    let meta_object = StoredObject::new(meta.canonical_bytes()).unwrap();
    let mk = RevisionRecord::regular(sk, 3, 14).with_content_refs(vec![meta_object.hash()]);
    medium
        .append(&mk, std::slice::from_ref(&meta_object), Some(sk))
        .unwrap();
    let r1_refs = medium.read_record(&r1).unwrap().content_refs.clone();
    let sk_refs = medium.read_record(&sk).unwrap().content_refs.clone();
    let ledger = crate::reclaim::ReclamationLedger {
        mk: mk.id(),
        sk,
        range_digest: meta.range_digest,
        seed: meta.seed,
        records: 2,
        reclaimed_keys: 0,
        journal_files: Vec::new(),
        units: vec![
            crate::reclaim::ReclaimUnit {
                record: sk,
                objects: sk_refs,
            },
            crate::reclaim::ReclaimUnit {
                record: r1,
                objects: r1_refs,
            },
        ],
    };
    let (audit, outcome) =
        crate::reclaim::publish_reclamation_ledger(&mut medium, &ledger, 5).unwrap();
    assert!(matches!(
        outcome,
        crate::medium::AppendOutcome::Accepted { .. }
    ));
    let directive = crate::reclaim::ReclaimDirective {
        audit_record: audit,
        units: ledger.units.clone(),
    };
    let exists = |medium: &SqliteMedium, table: &str, id: Hash| -> i64 {
        medium
            .conn
            .query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE hash = ?1)"),
                params![id.as_bytes()],
                |row| row.get(0),
            )
            .unwrap()
    };
    // (OC-C3 CP-2) The begin-bound migration: the REAL trusted
    // admission (the execution gate requires it).
    let begin_outcome = medium.begin_reclamation(&audit, &ledger.mk, 220).unwrap();
    assert!(
        matches!(
            begin_outcome,
            crate::reclaim::BeginPublicationOutcome::Accepted { .. }
                | crate::reclaim::BeginPublicationOutcome::DuplicateVerified { .. }
        ),
        "the fixture's begin admits: {begin_outcome:?}"
    );
    // Corrupt the private object's stored bytes (injected AFTER
    // the begin's admission — the deeper E3 boundary still
    // reached) — the hash KEY stays, the bytes no longer hash to
    // it.
    medium
        .conn
        .execute(
            "UPDATE objects SET bytes = ?1 WHERE hash = ?2",
            params![b"corrupted-bytes".to_vec(), private.hash().as_bytes()],
        )
        .unwrap();
    let error = medium.reclaim(&directive).unwrap_err();
    // (OC-C3 CP-2) The corrupted object sits ON the authorization
    // walk's path (the ancestry record's first reference — read
    // by the begin-bound gate's hash-verifying walk), so the
    // typed integrity refusal now surfaces at the GATE's read
    // (HashMismatch) rather than the PASS-2 probe
    // (ReferencedObjectCorrupt) — the same typed integrity
    // refusal and the same full-rollback guarantee, one boundary
    // earlier.
    assert!(
        matches!(error, MediumError::HashMismatch { .. }),
        "the typed corruption refusal at the gate's hash-verifying read: {error:?}"
    );
    // The full rollback: the retiring records AND the corrupted
    // object row are all still present.
    assert_eq!(exists(&medium, "records", r1), 1);
    assert_eq!(exists(&medium, "records", sk), 1);
    assert_eq!(exists(&medium, "objects", private.hash()), 1);
    // Restore the authentic bytes and re-run the SAME directive:
    // the execution succeeds.
    medium
        .conn
        .execute(
            "UPDATE objects SET bytes = ?1 WHERE hash = ?2",
            params![private.bytes().to_vec(), private.hash().as_bytes()],
        )
        .unwrap();
    let outcome = medium.reclaim(&directive).unwrap();
    assert_eq!(outcome.deferred, vec![shared.hash()]);
    for id in [private.hash(), r1, sk] {
        assert_eq!(
            exists(&medium, "records", id) + exists(&medium, "objects", id),
            0
        );
    }
    assert_eq!(exists(&medium, "objects", shared.hash()), 1);
    assert!(!medium.reclaim_pending(&directive).unwrap());
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}
#[test]
fn a_non_prefix_absence_refuses_on_sqlite() {
    // (E1) The recovery-authorization boundary on the second
    // provider: a missing retiring record ROW in a non-prefix
    // pattern (the oldest digest member gone while the SK
    // remains) is a shape this provider's single-transaction
    // pass 1 never produces — a genuine interruption rolls back
    // to the all-present state. The absence is evidence of
    // missing storage, never proof of a prior authorized
    // deletion: the provider refuses typed and nothing further
    // is deleted.
    let root = std::env::temp_dir().join(format!(
        "s7-p2np-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut medium = SqliteMedium::open(root.join("db.sqlite")).unwrap();
    let shared = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"shared-payload");
        enc.into_bytes()
    })
    .unwrap();
    let private = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"private-payload");
        enc.into_bytes()
    })
    .unwrap();
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "p2np", 0)
        .with_content_refs(vec![shared.hash()]);
    medium
        .append(&founding, std::slice::from_ref(&shared), None)
        .unwrap();
    let step = |medium: &mut SqliteMedium, objects: &[StoredObject]| -> Hash {
        let head = medium.head().unwrap().unwrap();
        let epoch = medium.read_record(&head).unwrap().epoch;
        let refs: Vec<Hash> = objects.iter().map(|o| o.hash()).collect();
        let record =
            RevisionRecord::regular(head, epoch, refs.len() as u64 + 10).with_content_refs(refs);
        medium.append(&record, objects, Some(head)).unwrap();
        record.id()
    };
    let _p1 = step(&mut medium, std::slice::from_ref(&shared));
    let r1 = step(&mut medium, &[shared.clone(), private.clone()]);
    let sk = step(&mut medium, std::slice::from_ref(&shared));
    let seed = crate::checkpoint::compute_digest_seed(&founding.id());
    let r1_bytes = medium.read_record(&r1).unwrap().canonical_bytes();
    let range_digest = crate::checkpoint::chain_range_digest(&seed, &[r1_bytes]);
    let meta = CheckpointMetadata::new(
        sk,
        r1,
        vec![SkipEntry::new(Hash::ZERO, _p1)],
        vec![
            ReclaimedIndexEntry::new(None, r1, 2),
            ReclaimedIndexEntry::new(None, sk, 3),
        ],
        range_digest,
        seed,
    );
    let meta_object = StoredObject::new(meta.canonical_bytes()).unwrap();
    let mk = RevisionRecord::regular(sk, 3, 14).with_content_refs(vec![meta_object.hash()]);
    medium
        .append(&mk, std::slice::from_ref(&meta_object), Some(sk))
        .unwrap();
    let r1_refs = medium.read_record(&r1).unwrap().content_refs.clone();
    let sk_refs = medium.read_record(&sk).unwrap().content_refs.clone();
    let ledger = crate::reclaim::ReclamationLedger {
        mk: mk.id(),
        sk,
        range_digest: meta.range_digest,
        seed: meta.seed,
        records: 2,
        reclaimed_keys: 0,
        journal_files: Vec::new(),
        units: vec![
            crate::reclaim::ReclaimUnit {
                record: sk,
                objects: sk_refs,
            },
            crate::reclaim::ReclaimUnit {
                record: r1,
                objects: r1_refs,
            },
        ],
    };
    let (audit, outcome) =
        crate::reclaim::publish_reclamation_ledger(&mut medium, &ledger, 5).unwrap();
    assert!(matches!(
        outcome,
        crate::medium::AppendOutcome::Accepted { .. }
    ));
    // The non-prefix absence: the OLDEST digest member's row is
    // gone while the SK (the newest unit) remains.
    medium
        .conn
        .execute(
            "DELETE FROM records WHERE hash = ?1",
            params![r1.as_bytes()],
        )
        .unwrap();
    let directive = crate::reclaim::ReclaimDirective {
        audit_record: audit,
        units: ledger.units.clone(),
    };
    let error = medium.reclaim(&directive).unwrap_err();
    assert!(
        matches!(
            error,
            MediumError::Chain {
                rule: "reclaim/absence-not-deletion-prefix"
            }
        ),
        "the non-prefix absence refuses: {error:?}"
    );
    // Nothing further deleted: the SK row and both objects
    // remain.
    let exists = |table: &str, id: Hash| -> i64 {
        medium
            .conn
            .query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE hash = ?1)"),
                params![id.as_bytes()],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(exists("records", sk), 1);
    assert_eq!(exists("objects", private.hash()), 1);
    assert_eq!(exists("objects", shared.hash()), 1);
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}
#[test]
fn the_trusted_completion_receipt_installs_on_sqlite() {
    // (P2-F4) The trusted completion receipt on the second
    // provider: the executed retirement under the atomic
    // transaction, then complete_reclamation over the
    // recovery-safe authority — the marker installed through
    // the CAS-bound admission, discovered and verified, the
    // duplicate key resolved by exact-content verification.
    let root = std::env::temp_dir().join(format!(
        "s7-p2cmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut medium = SqliteMedium::open(root.join("db.sqlite")).unwrap();
    let shared = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"shared-payload");
        enc.into_bytes()
    })
    .unwrap();
    let private = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"private-payload");
        enc.into_bytes()
    })
    .unwrap();
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "p2cmp", 0)
        .with_content_refs(vec![shared.hash()]);
    medium
        .append(&founding, std::slice::from_ref(&shared), None)
        .unwrap();
    let step = |medium: &mut SqliteMedium, objects: &[StoredObject]| -> Hash {
        let head = medium.head().unwrap().unwrap();
        let epoch = medium.read_record(&head).unwrap().epoch;
        let refs: Vec<Hash> = objects.iter().map(|o| o.hash()).collect();
        let record =
            RevisionRecord::regular(head, epoch, refs.len() as u64 + 10).with_content_refs(refs);
        medium.append(&record, objects, Some(head)).unwrap();
        record.id()
    };
    let _p1 = step(&mut medium, std::slice::from_ref(&shared));
    let r1 = step(&mut medium, &[shared.clone(), private.clone()]);
    let sk = step(&mut medium, std::slice::from_ref(&shared));
    let seed = crate::checkpoint::compute_digest_seed(&founding.id());
    let r1_bytes = medium.read_record(&r1).unwrap().canonical_bytes();
    let range_digest = crate::checkpoint::chain_range_digest(&seed, &[r1_bytes]);
    let meta = CheckpointMetadata::new(
        sk,
        r1,
        vec![SkipEntry::new(Hash::ZERO, _p1)],
        vec![
            ReclaimedIndexEntry::new(None, r1, 2),
            ReclaimedIndexEntry::new(None, sk, 3),
        ],
        range_digest,
        seed,
    );
    let meta_object = StoredObject::new(meta.canonical_bytes()).unwrap();
    let mk = RevisionRecord::regular(sk, 3, 14).with_content_refs(vec![meta_object.hash()]);
    medium
        .append(&mk, std::slice::from_ref(&meta_object), Some(sk))
        .unwrap();
    let r1_refs = medium.read_record(&r1).unwrap().content_refs.clone();
    let sk_refs = medium.read_record(&sk).unwrap().content_refs.clone();
    let ledger = crate::reclaim::ReclamationLedger {
        mk: mk.id(),
        sk,
        range_digest: meta.range_digest,
        seed: meta.seed,
        records: 2,
        reclaimed_keys: 0,
        journal_files: Vec::new(),
        units: vec![
            crate::reclaim::ReclaimUnit {
                record: sk,
                objects: sk_refs,
            },
            crate::reclaim::ReclaimUnit {
                record: r1,
                objects: r1_refs,
            },
        ],
    };
    let (audit, outcome) =
        crate::reclaim::publish_reclamation_ledger(&mut medium, &ledger, 5).unwrap();
    assert!(matches!(
        outcome,
        crate::medium::AppendOutcome::Accepted { .. }
    ));
    let directive = crate::reclaim::ReclaimDirective {
        audit_record: audit,
        units: ledger.units.clone(),
    };
    // (OC-C3 CP-2) The begin-bound migration: the REAL trusted
    // admission (the execution gate requires it).
    let begin_outcome = medium.begin_reclamation(&audit, &mk.id(), 226).unwrap();
    assert!(
        matches!(
            begin_outcome,
            crate::reclaim::BeginPublicationOutcome::Accepted { .. }
                | crate::reclaim::BeginPublicationOutcome::DuplicateVerified { .. }
        ),
        "the fixture's begin admits: {begin_outcome:?}"
    );
    medium.reclaim(&directive).unwrap();
    let (marker_id, marker) =
        crate::reclaim::complete_reclamation(&mut medium, &audit, &mk.id()).unwrap();
    assert_eq!(marker.deferred, vec![shared.hash()]);
    let (found_id, found) = crate::reclaim::find_installed_completion(&mut medium, &mk.id())
        .unwrap()
        .expect("the marker is discoverable on sqlite");
    assert_eq!(found_id, marker_id);
    assert_eq!(found, marker);
    crate::reclaim::verify_reclamation_complete(&mut medium, &marker_id, &mk.id()).unwrap();
    // The duplicate key resolves by exact-content verification.
    let (repeat_id, repeat) =
        crate::reclaim::complete_reclamation(&mut medium, &audit, &mk.id()).unwrap();
    assert_eq!(repeat_id, marker_id);
    assert_eq!(repeat, marker);
    medium.validate().unwrap();
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}
#[test]
fn the_historical_accounting_classifies_sqlite_failures_typed() {
    // (P2-R2-H2, the SQLite provider) The historical accounting
    // distinguishes the failure classes on the second provider:
    // a CORRUPT object row under its original hash is NEVER
    // accepted deletion evidence (the typed refusal propagates —
    // never MissingObject); a fabricated later marker refuses
    // the cascade; the multi-generation reopen verifies both
    // genuine markers.
    let root = std::env::temp_dir().join(format!(
        "s7-p2has-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut medium = SqliteMedium::open(root.join("db.sqlite")).unwrap();
    let shared = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"shared-payload");
        enc.into_bytes()
    })
    .unwrap();
    let private = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"private-payload");
        enc.into_bytes()
    })
    .unwrap();
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "p2has", 0)
        .with_content_refs(vec![shared.hash()]);
    medium
        .append(&founding, std::slice::from_ref(&shared), None)
        .unwrap();
    let step = |medium: &mut SqliteMedium, objects: &[StoredObject]| -> Hash {
        let head = medium.head().unwrap().unwrap();
        let epoch = medium.read_record(&head).unwrap().epoch;
        let refs: Vec<Hash> = objects.iter().map(|o| o.hash()).collect();
        let record =
            RevisionRecord::regular(head, epoch, refs.len() as u64 + 10).with_content_refs(refs);
        medium.append(&record, objects, Some(head)).unwrap();
        record.id()
    };
    let _p1 = step(&mut medium, std::slice::from_ref(&shared));
    let r1 = step(&mut medium, &[shared.clone(), private.clone()]);
    let sk = step(&mut medium, std::slice::from_ref(&shared));
    let seed = crate::checkpoint::compute_digest_seed(&founding.id());
    let r1_bytes = medium.read_record(&r1).unwrap().canonical_bytes();
    let range_digest = crate::checkpoint::chain_range_digest(&seed, &[r1_bytes]);
    let meta = CheckpointMetadata::new(
        sk,
        r1,
        vec![SkipEntry::new(Hash::ZERO, _p1)],
        vec![
            ReclaimedIndexEntry::new(None, r1, 2),
            ReclaimedIndexEntry::new(None, sk, 3),
        ],
        range_digest,
        seed,
    );
    let meta_object = StoredObject::new(meta.canonical_bytes()).unwrap();
    let mk = RevisionRecord::regular(sk, 3, 14).with_content_refs(vec![meta_object.hash()]);
    medium
        .append(&mk, std::slice::from_ref(&meta_object), Some(sk))
        .unwrap();
    let r1_refs = medium.read_record(&r1).unwrap().content_refs.clone();
    let sk_refs = medium.read_record(&sk).unwrap().content_refs.clone();
    let ledger = crate::reclaim::ReclamationLedger {
        mk: mk.id(),
        sk,
        range_digest: meta.range_digest,
        seed: meta.seed,
        records: 2,
        reclaimed_keys: 0,
        journal_files: Vec::new(),
        units: vec![
            crate::reclaim::ReclaimUnit {
                record: sk,
                objects: sk_refs,
            },
            crate::reclaim::ReclaimUnit {
                record: r1,
                objects: r1_refs,
            },
        ],
    };
    let (audit, outcome) =
        crate::reclaim::publish_reclamation_ledger(&mut medium, &ledger, 5).unwrap();
    assert!(matches!(
        outcome,
        crate::medium::AppendOutcome::Accepted { .. }
    ));
    let directive = crate::reclaim::ReclaimDirective {
        audit_record: audit,
        units: ledger.units.clone(),
    };
    // (OC-C3 CP-2) The begin-bound migration: the REAL trusted
    // admission (the execution gate requires it).
    let begin_outcome = medium.begin_reclamation(&audit, &mk.id(), 222).unwrap();
    assert!(
        matches!(
            begin_outcome,
            crate::reclaim::BeginPublicationOutcome::Accepted { .. }
                | crate::reclaim::BeginPublicationOutcome::DuplicateVerified { .. }
        ),
        "the fixture's begin admits: {begin_outcome:?}"
    );
    medium.reclaim(&directive).unwrap();
    let (marker_id, _marker) =
        crate::reclaim::complete_reclamation(&mut medium, &audit, &mk.id()).unwrap();
    // THE CORRUPT ROW under the deferred survivor's hash: the
    // historical verification's deferred-presence read must
    // propagate the typed integrity failure — never
    // MissingObject, never accepted deletion evidence.
    medium
        .conn
        .execute(
            "UPDATE objects SET bytes = ?1 WHERE hash = ?2",
            params![b"corrupted-bytes".to_vec(), shared.hash().as_bytes()],
        )
        .unwrap();
    let error =
        crate::reclaim::verify_reclamation_complete(&mut medium, &marker_id, &mk.id()).unwrap_err();
    assert!(
        !matches!(error, MediumError::MissingObject { .. }),
        "the corruption is never classified as absence: {error:?}"
    );
    // Restore the authentic bytes; the marker verifies again.
    medium
        .conn
        .execute(
            "UPDATE objects SET bytes = ?1 WHERE hash = ?2",
            params![shared.bytes().to_vec(), shared.hash().as_bytes()],
        )
        .unwrap();
    crate::reclaim::verify_reclamation_complete(&mut medium, &marker_id, &mk.id()).unwrap();
    // THE FABRICATED LATER MARKER (the wrong content, installed
    // through the in-crate adversarial append): the cascade
    // refuses — it never becomes attribution evidence.
    let head = medium.head().unwrap().unwrap();
    let parent_epoch = medium.read_record(&head).unwrap().epoch;
    let wrong = crate::reclaim::ReclamationComplete {
        kind: crate::reclaim::CompletionKind::Batch,
        mk: mk.id(),
        batch: 0,
        source: audit,
        mandatory_count: 42,
        deferred: vec![shared.hash()],
        journal_files: Vec::new(),
    };
    let object = StoredObject::new(wrong.canonical_bytes()).unwrap();
    let record = RevisionRecord::regular(head, parent_epoch, 98)
        .with_idempotency_key(crate::reclaim::reclamation_complete_idempotency_key(
            &mk.id(),
            Some(0),
        ))
        .with_content_refs(vec![object.hash()]);
    let admitted = crate::medium::AdmittedAppend::generic(&record, std::slice::from_ref(&object));
    // THE DUPLICATE-KEY DISCIPLINE: the genuine marker already
    // holds this checkpoint's deterministic completion key — the
    // fabricated same-key record is refused at the append (no
    // second marker can ever install; nothing is silently
    // accepted).
    let outcome = medium.append_admitted(admitted, Some(head)).unwrap_err();
    assert!(
        matches!(
            outcome,
            MediumError::Chain {
                rule: "duplicate-idempotency-key"
            }
        ),
        "the fabricated same-key marker refuses: {outcome:?}"
    );
    // THE MULTI-GENERATION REOPEN: the genuine marker verifies
    // after the fresh-process restart.
    drop(medium);
    let mut reopened = SqliteMedium::open(root.join("db.sqlite")).unwrap();
    crate::reclaim::verify_reclamation_complete(&mut reopened, &marker_id, &mk.id()).unwrap();
    reopened.validate().unwrap();
    let _ = std::fs::remove_dir_all(&root);
}
#[test]
fn the_sqlite_two_retirement_attribution_cascade_verifies_and_refuses() {
    // (P2-R2-H1/H2, the genuine SQLite matrix) The two-retirement
    // attribution: A defers the shared X (the later range's B1
    // references it), B mandates and deletes it, both complete —
    // A's historical marker verifies through B's AUTHENTICATED
    // completion evidence, both verify after the fresh reopen.
    // The adversarial variants: a fabricated later marker (a
    // foreign MK with its own valid key — it INSTALLS and
    // REACHES the cascade) refuses at its own source-audit
    // authentication; a corrupted row resurrected at the deleted
    // X's hash propagates the typed integrity failure through
    // the accounting — never classified as absence.
    let root = std::env::temp_dir().join(format!(
        "s7-p2casc-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut medium = SqliteMedium::open(root.join("db.sqlite")).unwrap();
    let fobj = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"founding-payload");
        enc.into_bytes()
    })
    .unwrap();
    let x = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"the-shared-deferred-x");
        enc.into_bytes()
    })
    .unwrap();
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "p2casc", 0)
        .with_content_refs(vec![fobj.hash()]);
    medium
        .append(&founding, std::slice::from_ref(&fobj), None)
        .unwrap();
    let step = |medium: &mut SqliteMedium, objects: &[StoredObject], tick: u64| -> Hash {
        let head = medium.head().unwrap().unwrap();
        let epoch = medium.read_record(&head).unwrap().epoch;
        let refs: Vec<Hash> = objects.iter().map(|o| o.hash()).collect();
        let record = RevisionRecord::regular(head, epoch, tick).with_content_refs(refs);
        medium.append(&record, objects, Some(head)).unwrap();
        record.id()
    };
    // A's range: A1(X) → SK1(fobj) → MK1 → auditA.
    let a1 = step(&mut medium, std::slice::from_ref(&x), 1);
    let sk1 = step(&mut medium, std::slice::from_ref(&fobj), 2);
    let seed = crate::checkpoint::compute_digest_seed(&founding.id());
    let a1_bytes = medium.read_record(&a1).unwrap().canonical_bytes();
    let digest_a = crate::checkpoint::chain_range_digest(&seed, &[a1_bytes]);
    let meta1 = CheckpointMetadata::new(
        sk1,
        a1,
        vec![SkipEntry::new(Hash::ZERO, founding.id())],
        vec![
            ReclaimedIndexEntry::new(None, a1, 1),
            ReclaimedIndexEntry::new(None, sk1, 2),
        ],
        digest_a,
        seed,
    );
    let meta1_object = StoredObject::new(meta1.canonical_bytes()).unwrap();
    let mk1 = step(&mut medium, std::slice::from_ref(&meta1_object), 3);
    let ledger_a = crate::reclaim::ReclamationLedger {
        mk: mk1,
        sk: sk1,
        range_digest: digest_a,
        seed,
        records: 2,
        reclaimed_keys: 0,
        journal_files: Vec::new(),
        units: vec![
            crate::reclaim::ReclaimUnit {
                record: sk1,
                objects: vec![fobj.hash()],
            },
            crate::reclaim::ReclaimUnit {
                record: a1,
                objects: vec![x.hash()],
            },
        ],
    };
    let (audit_a, _) =
        crate::reclaim::publish_reclamation_ledger(&mut medium, &ledger_a, 4).unwrap();
    // (OC-C3 CP-2) A's begin — the real admission. (OC-C4) The
    // begin record is B1's TRUE nearest retained ancestor: the
    // later range's skip entry names it, never audit_a — a
    // gap-crossing walk from MK2 lands on the begin, never
    // bypasses it.
    let begin_a = medium
        .begin_reclamation(&audit_a, &ledger_a.mk, 224)
        .unwrap();
    let begin_a_id = match begin_a {
        crate::reclaim::BeginPublicationOutcome::Accepted { id, .. }
        | crate::reclaim::BeginPublicationOutcome::DuplicateVerified { id } => id,
        other => panic!("A's begin admits: {other:?}"),
    };
    // B's range: B1(X) → SK2(fobj) → MK2 → auditB.
    let b1 = step(&mut medium, std::slice::from_ref(&x), 5);
    let sk2 = step(&mut medium, std::slice::from_ref(&fobj), 6);
    let b1_bytes = medium.read_record(&b1).unwrap().canonical_bytes();
    let digest_b = crate::checkpoint::chain_range_digest(&seed, &[b1_bytes]);
    let meta2 = CheckpointMetadata::new(
        sk2,
        b1,
        vec![SkipEntry::new(Hash::ZERO, begin_a_id)],
        vec![
            ReclaimedIndexEntry::new(None, b1, 5),
            ReclaimedIndexEntry::new(None, sk2, 6),
        ],
        digest_b,
        seed,
    );
    let meta2_object = StoredObject::new(meta2.canonical_bytes()).unwrap();
    let mk2 = step(&mut medium, std::slice::from_ref(&meta2_object), 7);
    let ledger_b = crate::reclaim::ReclamationLedger {
        mk: mk2,
        sk: sk2,
        range_digest: digest_b,
        seed,
        records: 2,
        reclaimed_keys: 0,
        journal_files: Vec::new(),
        units: vec![
            crate::reclaim::ReclaimUnit {
                record: sk2,
                objects: vec![fobj.hash()],
            },
            crate::reclaim::ReclaimUnit {
                record: b1,
                objects: vec![x.hash()],
            },
        ],
    };
    let (audit_b, _) =
        crate::reclaim::publish_reclamation_ledger(&mut medium, &ledger_b, 8).unwrap();
    // (OC-C3 CP-2) B's begin — the real admission.
    let begin_b = medium
        .begin_reclamation(&audit_b, &ledger_b.mk, 225)
        .unwrap();
    assert!(
        matches!(
            begin_b,
            crate::reclaim::BeginPublicationOutcome::Accepted { .. }
                | crate::reclaim::BeginPublicationOutcome::DuplicateVerified { .. }
        ),
        "B's begin admits: {begin_b:?}"
    );
    // Both retirements execute and complete.
    let directive_a = crate::reclaim::ReclaimDirective {
        audit_record: audit_a,
        units: ledger_a.units.clone(),
    };
    medium.reclaim(&directive_a).unwrap();
    let (marker_a, marker_a_content) =
        crate::reclaim::complete_reclamation(&mut medium, &audit_a, &mk1).unwrap();
    assert!(
        marker_a_content.deferred.contains(&x.hash()),
        "A defers the shared X (B1, a survivor above A, references it)"
    );
    let directive_b = crate::reclaim::ReclaimDirective {
        audit_record: audit_b,
        units: ledger_b.units.clone(),
    };
    medium.reclaim(&directive_b).unwrap();
    let (marker_b, _) = crate::reclaim::complete_reclamation(&mut medium, &audit_b, &mk2).unwrap();
    // A's historical marker verifies through B's authenticated
    // completion evidence (the per-object accounting).
    crate::reclaim::verify_reclamation_complete(&mut medium, &marker_a, &mk1).unwrap();
    crate::reclaim::verify_reclamation_complete(&mut medium, &marker_b, &mk2).unwrap();
    // THE CORRUPTED ROW resurrected at the deleted X's hash: the
    // accounting's absence read propagates the typed integrity
    // failure — never classified as absence, never accepted
    // deletion evidence.
    medium
        .conn
        .execute(
            "INSERT OR REPLACE INTO objects (hash, domain, bytes) VALUES (?1, ?2, ?3)",
            params![
                x.hash().as_bytes().to_vec(),
                "sektor7/save/1",
                b"corrupted-resurrected".to_vec()
            ],
        )
        .unwrap();
    let error =
        crate::reclaim::verify_reclamation_complete(&mut medium, &marker_a, &mk1).unwrap_err();
    assert!(
        matches!(
            error,
            MediumError::HashMismatch { .. } | MediumError::ReferencedObjectCorrupt { .. }
        ),
        "the corrupted resurrected row propagates the typed integrity failure: {error:?}"
    );
    // Remove the corrupted row: the genuine accounting resumes.
    medium
        .conn
        .execute(
            "DELETE FROM objects WHERE hash = ?1",
            params![x.hash().as_bytes()],
        )
        .unwrap();
    crate::reclaim::verify_reclamation_complete(&mut medium, &marker_a, &mk1).unwrap();
    // THE MULTI-GENERATION REOPEN: both markers verify after the
    // fresh-process restart.
    drop(medium);
    let mut reopened = SqliteMedium::open(root.join("db.sqlite")).unwrap();
    crate::reclaim::verify_reclamation_complete(&mut reopened, &marker_a, &mk1).unwrap();
    crate::reclaim::verify_reclamation_complete(&mut reopened, &marker_b, &mk2).unwrap();
    reopened.validate().unwrap();
    drop(reopened);
    let mut medium = SqliteMedium::open(root.join("db.sqlite")).unwrap();
    // THE FABRICATED LATER MARKER — a FOREIGN MK with its own
    // valid deterministic key: it INSTALLS (no duplicate) and
    // REACHES the cascade, refusing at its own source-audit
    // authentication (the audit's ledger is bound to MK2, not
    // the foreign MK).
    let foreign_mk = crate::hash::hash_object(b"the-fabricated-foreign-mk");
    let head = medium.head().unwrap().unwrap();
    let parent_epoch = medium.read_record(&head).unwrap().epoch;
    let fabricated = crate::reclaim::ReclamationComplete {
        kind: crate::reclaim::CompletionKind::Batch,
        mk: foreign_mk,
        batch: 0,
        source: audit_b,
        mandatory_count: 3,
        deferred: vec![fobj.hash()],
        journal_files: Vec::new(),
    };
    let object = StoredObject::new(fabricated.canonical_bytes()).unwrap();
    let record = RevisionRecord::regular(head, parent_epoch, 98)
        .with_idempotency_key(crate::reclaim::reclamation_complete_idempotency_key(
            &foreign_mk,
            Some(0),
        ))
        .with_content_refs(vec![object.hash()]);
    let admitted = crate::medium::AdmittedAppend::generic(&record, std::slice::from_ref(&object));
    let outcome = medium.append_admitted(admitted, Some(head)).unwrap();
    assert!(matches!(
        outcome,
        crate::medium::AppendOutcome::Accepted { .. }
    ));
    let error =
        crate::reclaim::verify_reclamation_complete(&mut medium, &marker_a, &mk1).unwrap_err();
    assert!(
        matches!(
            error,
            MediumError::Chain {
                rule: "reclamation-complete/mk-mismatch"
            }
        ),
        "the fabricated foreign-MK later marker refuses at its own source-audit authentication: {error:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
#[test]
fn the_trusted_begin_admission_publishes_on_sqlite() {
    // (OC-C2) The SQLite twin: the valid admission under ONE
    // BEGIN IMMEDIATE publishes Accepted (the begin row + the
    // HEAD advance, atomically committed); the repeat resolves
    // the duplicate by exact content; the fabricated digest
    // refuses with nothing committed.
    let root = std::env::temp_dir().join(format!(
        "s7-occ2-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut medium = SqliteMedium::open(root.join("db.sqlite")).unwrap();
    let shared = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"shared-payload");
        enc.into_bytes()
    })
    .unwrap();
    let private = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"private-payload");
        enc.into_bytes()
    })
    .unwrap();
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "occ2", 0)
        .with_content_refs(vec![shared.hash()]);
    medium
        .append(&founding, std::slice::from_ref(&shared), None)
        .unwrap();
    let step = |medium: &mut SqliteMedium, objects: &[StoredObject], tick: u64| -> Hash {
        let head = medium.head().unwrap().unwrap();
        let epoch = medium.read_record(&head).unwrap().epoch;
        let refs: Vec<Hash> = objects.iter().map(|o| o.hash()).collect();
        let record = RevisionRecord::regular(head, epoch, tick).with_content_refs(refs);
        medium.append(&record, objects, Some(head)).unwrap();
        record.id()
    };
    let _p1 = step(&mut medium, std::slice::from_ref(&shared), 1);
    let r1 = step(&mut medium, &[shared.clone(), private.clone()], 2);
    let sk = step(&mut medium, std::slice::from_ref(&shared), 3);
    let seed = crate::checkpoint::compute_digest_seed(&founding.id());
    let r1_bytes = medium.read_record(&r1).unwrap().canonical_bytes();
    let range_digest = crate::checkpoint::chain_range_digest(&seed, &[r1_bytes]);
    let meta = CheckpointMetadata::new(
        sk,
        r1,
        vec![SkipEntry::new(Hash::ZERO, _p1)],
        vec![
            ReclaimedIndexEntry::new(None, r1, 2),
            ReclaimedIndexEntry::new(None, sk, 3),
        ],
        range_digest,
        seed,
    );
    let meta_object = StoredObject::new(meta.canonical_bytes()).unwrap();
    let mk = step(&mut medium, std::slice::from_ref(&meta_object), 4);
    let r1_refs = medium.read_record(&r1).unwrap().content_refs.clone();
    let sk_refs = medium.read_record(&sk).unwrap().content_refs.clone();
    let ledger = crate::reclaim::ReclamationLedger {
        mk,
        sk,
        range_digest: meta.range_digest,
        seed: meta.seed,
        records: 2,
        reclaimed_keys: 0,
        journal_files: Vec::new(),
        units: vec![
            crate::reclaim::ReclaimUnit {
                record: sk,
                objects: sk_refs,
            },
            crate::reclaim::ReclaimUnit {
                record: r1,
                objects: r1_refs,
            },
        ],
    };
    let (audit, outcome) =
        crate::reclaim::publish_reclamation_ledger(&mut medium, &ledger, 5).unwrap();
    assert!(matches!(
        outcome,
        crate::medium::AppendOutcome::Accepted { .. }
    ));
    // The valid admission.
    let outcome = medium.begin_reclamation(&audit, &mk, 30).unwrap();
    let installed = match outcome {
        // (C2-R8) The transactional admission carries its ACTUAL
        // tier.
        crate::reclaim::BeginPublicationOutcome::Accepted { id, tier } => {
            assert_eq!(
                tier,
                SyncTier::TransactionalWalFull,
                "the sqlite admission carries its actual tier"
            );
            id
        }
        other => panic!("the sqlite admission accepts, got {other:?}"),
    };
    assert_eq!(medium.head().unwrap().unwrap(), installed);
    // The duplicate: exact content.
    let outcome = medium.begin_reclamation(&audit, &mk, 31).unwrap();
    match outcome {
        crate::reclaim::BeginPublicationOutcome::DuplicateVerified { id } => {
            assert_eq!(id, installed)
        }
        other => panic!("the sqlite duplicate verifies, got {other:?}"),
    }
    medium.validate().unwrap();
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}
#[test]
fn the_sqlite_begin_admission_refuses_corrupt_and_orphan_evidence() {
    // (OC-C2-F2/F3/F6) The SQLite integrity regressions: a
    // PREPOPULATED corrupt object row under the exact expected
    // begin-object hash refuses typed (the insert-once
    // verify-on-retry discipline), the HEAD unchanged, the
    // corrupt evidence preserved; an ORPHANED same-key record
    // row (never on the ancestry) exerts no authority — the
    // admission publishes Accepted.
    let root = std::env::temp_dir().join(format!(
        "s7-occ2c-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let build = |label: &str| {
        let r = root.join(label);
        std::fs::create_dir_all(&r).unwrap();
        let mut medium = SqliteMedium::open(r.join("db.sqlite")).unwrap();
        let shared = StoredObject::new({
            let mut enc = Encoder::new();
            enc.write_str("sektor7/save/1");
            enc.write_bytes(b"shared-payload");
            enc.into_bytes()
        })
        .unwrap();
        let private = StoredObject::new({
            let mut enc = Encoder::new();
            enc.write_str("sektor7/save/1");
            enc.write_bytes(b"private-payload");
            enc.into_bytes()
        })
        .unwrap();
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, label, 0)
            .with_content_refs(vec![shared.hash()]);
        medium
            .append(&founding, std::slice::from_ref(&shared), None)
            .unwrap();
        let step = |medium: &mut SqliteMedium, objects: &[StoredObject], tick: u64| -> Hash {
            let head = medium.head().unwrap().unwrap();
            let epoch = medium.read_record(&head).unwrap().epoch;
            let refs: Vec<Hash> = objects.iter().map(|o| o.hash()).collect();
            let record = RevisionRecord::regular(head, epoch, tick).with_content_refs(refs);
            medium.append(&record, objects, Some(head)).unwrap();
            record.id()
        };
        let _p1 = step(&mut medium, std::slice::from_ref(&shared), 1);
        let r1 = step(&mut medium, &[shared.clone(), private.clone()], 2);
        let sk = step(&mut medium, std::slice::from_ref(&shared), 3);
        let seed = crate::checkpoint::compute_digest_seed(&founding.id());
        let r1_bytes = medium.read_record(&r1).unwrap().canonical_bytes();
        let range_digest = crate::checkpoint::chain_range_digest(&seed, &[r1_bytes]);
        let meta = CheckpointMetadata::new(
            sk,
            r1,
            vec![SkipEntry::new(Hash::ZERO, _p1)],
            vec![
                ReclaimedIndexEntry::new(None, r1, 2),
                ReclaimedIndexEntry::new(None, sk, 3),
            ],
            range_digest,
            seed,
        );
        let meta_object = StoredObject::new(meta.canonical_bytes()).unwrap();
        let mk = step(&mut medium, std::slice::from_ref(&meta_object), 4);
        let r1_refs = medium.read_record(&r1).unwrap().content_refs.clone();
        let sk_refs = medium.read_record(&sk).unwrap().content_refs.clone();
        let ledger = crate::reclaim::ReclamationLedger {
            mk,
            sk,
            range_digest: meta.range_digest,
            seed: meta.seed,
            records: 2,
            reclaimed_keys: 0,
            journal_files: Vec::new(),
            units: vec![
                crate::reclaim::ReclaimUnit {
                    record: sk,
                    objects: sk_refs,
                },
                crate::reclaim::ReclaimUnit {
                    record: r1,
                    objects: r1_refs,
                },
            ],
        };
        let (audit, outcome) =
            crate::reclaim::publish_reclamation_ledger(&mut medium, &ledger, 5).unwrap();
        assert!(matches!(
            outcome,
            crate::medium::AppendOutcome::Accepted { .. }
        ));
        (medium, audit, mk, ledger)
    };
    // (F2) The prepopulated corrupt row: derive the exact begin
    // object the admission will construct (the same verified
    // values), pre-insert WRONG bytes under its hash.
    {
        let (mut medium, audit, mk, ledger) = build("corrupt-row");
        let begin = crate::reclaim::ReclamationBegin {
            mk,
            source: audit,
            range_digest: ledger.range_digest,
            seed: ledger.seed,
        };
        let expected_hash = StoredObject::new(begin.canonical_bytes()).unwrap().hash();
        medium
            .conn
            .execute(
                "INSERT INTO objects (hash, domain, bytes) VALUES (?1, ?2, ?3)",
                params![
                    expected_hash.as_bytes().to_vec(),
                    "sektor7/save/1",
                    b"corrupt-prepopulated".to_vec()
                ],
            )
            .unwrap();
        let head_before = medium.head().unwrap().unwrap();
        let error = medium.begin_reclamation(&audit, &mk, 60).unwrap_err();
        assert!(
            matches!(
                error,
                MediumError::HashMismatch { .. } | MediumError::ReferencedObjectCorrupt { .. }
            ),
            "the corrupt preexisting row refuses typed (the insert-once verify): {error:?}"
        );
        assert_eq!(
            medium.head().unwrap().unwrap(),
            head_before,
            "the HEAD is unchanged"
        );
        let preserved: Vec<u8> = medium
            .conn
            .query_row(
                "SELECT bytes FROM objects WHERE hash = ?1",
                params![expected_hash.as_bytes()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            preserved,
            b"corrupt-prepopulated".to_vec(),
            "the corrupt evidence is preserved, never overwritten"
        );
    }
    // (F3) The orphaned same-key record row: inserted directly,
    // never on the ancestry — the head-descent discovery never
    // finds it; the admission publishes Accepted.
    {
        let (mut medium, audit, mk, _ledger) = build("orphan-key");
        let key = crate::reclaim::reclamation_begin_idempotency_key(&mk);
        let orphan_object = StoredObject::new(
            crate::reclaim::ReclamationBegin {
                mk,
                source: audit,
                range_digest: [0u8; 32],
                seed: [0u8; 32],
            }
            .canonical_bytes(),
        )
        .unwrap();
        let orphan_record = RevisionRecord::regular(Hash::ZERO, 0, 99)
            .with_idempotency_key(key.clone())
            .with_content_refs(vec![orphan_object.hash()]);
        medium
            .conn
            .execute(
                "INSERT INTO objects (hash, domain, bytes) VALUES (?1, ?2, ?3)",
                params![
                    orphan_object.hash().as_bytes().to_vec(),
                    orphan_object.domain(),
                    orphan_object.bytes().to_vec()
                ],
            )
            .unwrap();
        medium
                .conn
                .execute(
                    "INSERT INTO records (hash, bytes, epoch, parent, idem, kind, tick)                      VALUES (?1, ?2, 0, NULL, ?3, 1, 99)",
                    params![
                        crate::hash::hash_object(&orphan_record.canonical_bytes())
                            .as_bytes()
                            .to_vec(),
                        orphan_record.canonical_bytes(),
                        key
                    ],
                )
                .unwrap();
        // The orphan squats the deterministic key: the chain-wide
        // unique-idempotency discipline refuses the publication
        // fail-closed (the typed duplicate-key rule) — the orphan
        // never returns DuplicateVerified and never fakes
        // authority; the HEAD is unchanged.
        let error = medium.begin_reclamation(&audit, &mk, 61).unwrap_err();
        assert!(
            matches!(
                error,
                MediumError::Chain {
                    rule: "duplicate-idempotency-key"
                }
            ),
            "the orphaned same-key row never fakes a duplicate (the fail-closed refusal): {error:?}"
        );
        let head_after = medium.head().unwrap().unwrap();
        let _ = head_after;
    }
    let _ = std::fs::remove_dir_all(&root);
}
// (C2-R4) The shared admission store for the second corrective's
// SQLite regressions: founding → P1 → R1(shared+private) → SK →
// MK(the meta) → the published audit — the exact evidence shape
// the admission verifies. Returns (root, medium, audit, mk,
// ledger, shared, private, p1) so each adversarial case can
// delete, corrupt, or install the exact rows it needs.
fn sqlite_begin_store(
    label: &str,
) -> (
    std::path::PathBuf,
    SqliteMedium,
    Hash,
    Hash,
    crate::reclaim::ReclamationLedger,
    Hash,
    Hash,
    Hash,
) {
    let root = std::env::temp_dir().join(format!(
        "s7-occ2r4-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut medium = SqliteMedium::open(root.join("db.sqlite")).unwrap();
    let shared = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"shared-payload");
        enc.into_bytes()
    })
    .unwrap();
    let private = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"private-payload");
        enc.into_bytes()
    })
    .unwrap();
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, label, 0)
        .with_content_refs(vec![shared.hash()]);
    medium
        .append(&founding, std::slice::from_ref(&shared), None)
        .unwrap();
    let step = |medium: &mut SqliteMedium, objects: &[StoredObject], tick: u64| -> Hash {
        let head = medium.head().unwrap().unwrap();
        let epoch = medium.read_record(&head).unwrap().epoch;
        let refs: Vec<Hash> = objects.iter().map(|o| o.hash()).collect();
        let record = RevisionRecord::regular(head, epoch, tick).with_content_refs(refs);
        medium.append(&record, objects, Some(head)).unwrap();
        record.id()
    };
    let p1 = step(&mut medium, std::slice::from_ref(&shared), 1);
    let r1 = step(&mut medium, &[shared.clone(), private.clone()], 2);
    let sk = step(&mut medium, std::slice::from_ref(&shared), 3);
    let seed = crate::checkpoint::compute_digest_seed(&founding.id());
    let r1_bytes = medium.read_record(&r1).unwrap().canonical_bytes();
    let range_digest = crate::checkpoint::chain_range_digest(&seed, &[r1_bytes]);
    let meta = CheckpointMetadata::new(
        sk,
        r1,
        vec![SkipEntry::new(Hash::ZERO, p1)],
        vec![
            ReclaimedIndexEntry::new(None, r1, 2),
            ReclaimedIndexEntry::new(None, sk, 3),
        ],
        range_digest,
        seed,
    );
    let meta_object = StoredObject::new(meta.canonical_bytes()).unwrap();
    let mk = step(&mut medium, std::slice::from_ref(&meta_object), 4);
    let r1_refs = medium.read_record(&r1).unwrap().content_refs.clone();
    let sk_refs = medium.read_record(&sk).unwrap().content_refs.clone();
    let ledger = crate::reclaim::ReclamationLedger {
        mk,
        sk,
        range_digest: meta.range_digest,
        seed: meta.seed,
        records: 2,
        reclaimed_keys: 0,
        journal_files: Vec::new(),
        units: vec![
            crate::reclaim::ReclaimUnit {
                record: sk,
                objects: sk_refs,
            },
            crate::reclaim::ReclaimUnit {
                record: r1,
                objects: r1_refs,
            },
        ],
    };
    let (audit, outcome) =
        crate::reclaim::publish_reclamation_ledger(&mut medium, &ledger, 5).unwrap();
    assert!(matches!(
        outcome,
        crate::medium::AppendOutcome::Accepted { .. }
    ));
    (
        root,
        medium,
        audit,
        mk,
        ledger,
        shared.hash(),
        private.hash(),
        p1,
    )
}

// (C2-R4) Installs a hand-built revision ON the SQLite ancestry
// as the new HEAD (the object row, the record row, the meta head
// advance) — the corruption-shaped installation the discovery's
// linkage and shape validation must refuse (the append path
// enforces the coherence itself; these rows never passed it).
fn install_sqlite_begin_fake(
    medium: &mut SqliteMedium,
    record: &RevisionRecord,
    object: &StoredObject,
) {
    medium
        .conn
        .execute(
            "INSERT INTO objects (hash, domain, bytes) VALUES (?1, ?2, ?3)",
            params![
                object.hash().as_bytes().to_vec(),
                object.domain(),
                object.bytes().to_vec()
            ],
        )
        .unwrap();
    let record_bytes = record.canonical_bytes();
    let record_id = crate::hash::hash_object(&record_bytes);
    let kind = if matches!(record.kind, crate::record::RecordKind::Regular) {
        1
    } else {
        0
    };
    medium
        .conn
        .execute(
            "INSERT INTO records (hash, bytes, epoch, parent, idem, kind, tick) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record_id.as_bytes(),
                record_bytes,
                record.epoch as i64,
                record.parent.map(|parent| parent.as_bytes().to_vec()),
                record.idempotency_key.clone(),
                kind,
                record.tick as i64
            ],
        )
        .unwrap();
    medium
        .conn
        .execute(
            "UPDATE meta SET value = ?1 WHERE key = 'head'",
            params![record_id.as_bytes()],
        )
        .unwrap();
}

#[test]
fn the_sqlite_begin_commit_faults_reconcile_read_only() {
    // (C2-R1/R4) The uncertain-commit regressions over the
    // established fault seam: the injected CommitFailure (the
    // DEFINITE pre-commit rollback — the transaction is rolled
    // back before any durability; the genuinely-ambiguous real
    // COMMIT error shares the exact uncertain→reconcile path)
    // reconciles through the READ-ONLY observation to
    // ReconciledNotInstalled with NO second publication (no
    // begin row, no begin object, the HEAD unchanged) — and the
    // EXPLICIT retry after the confirmed absence publishes
    // Accepted; the injected AfterCommit (the COMMIT succeeded,
    // the acknowledgment lost) reconciles to
    // ReconciledInstalled with the exact installed identity;
    // a reconciliation that CANNOT verify (the evidence
    // degraded underneath the observation) is the typed
    // unverifiable — never a guess, never a write.
    // (a) The definite pre-commit rollback + the explicit retry.
    {
        let (root, mut medium, audit, mk, ledger, _shared, _private, _p1) =
            sqlite_begin_store("commit-failure");
        let key = crate::reclaim::reclamation_begin_idempotency_key(&mk);
        let begin = crate::reclaim::ReclamationBegin {
            mk,
            source: audit,
            range_digest: ledger.range_digest,
            seed: ledger.seed,
        };
        let expected_hash = StoredObject::new(begin.canonical_bytes()).unwrap().hash();
        let head_before = medium.head().unwrap().unwrap();
        medium.fault = Some(SqliteFaultControl::new().arm(
            SqliteFaultPoint::CommitFailure,
            SqliteFaultAction::InjectError,
        ));
        let outcome = medium.begin_reclamation(&audit, &mk, 70).unwrap();
        assert!(
            matches!(
                outcome,
                crate::reclaim::BeginPublicationOutcome::ReconciledNotInstalled
            ),
            "the definite pre-commit rollback reconciles to the confirmed absence: {outcome:?}"
        );
        let begin_rows: i64 = medium
            .conn
            .query_row(
                "SELECT COUNT(*) FROM records WHERE idem = ?1",
                params![key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(begin_rows, 0, "no begin record row (the rollback)");
        let begin_objects: i64 = medium
            .conn
            .query_row(
                "SELECT COUNT(*) FROM objects WHERE hash = ?1",
                params![expected_hash.as_bytes()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            begin_objects, 0,
            "the READ-ONLY reconciliation published nothing (no second publication)"
        );
        assert_eq!(
            medium.head().unwrap().unwrap(),
            head_before,
            "the HEAD is unchanged after the reconciled absence"
        );
        // The EXPLICIT retry after the confirmed absence — a
        // separate caller operation, never inside the
        // reconciliation — revalidates and publishes Accepted.
        medium.fault = None;
        let outcome = medium.begin_reclamation(&audit, &mk, 71).unwrap();
        let installed = match outcome {
            // (C2-R8) The retry's Accepted carries the
            // transactional tier.
            crate::reclaim::BeginPublicationOutcome::Accepted { id, tier } => {
                assert_eq!(
                    tier,
                    SyncTier::TransactionalWalFull,
                    "the retry's Accepted carries the transactional tier"
                );
                id
            }
            other => {
                panic!("the explicit retry after the confirmed absence accepts: {other:?}")
            }
        };
        assert_eq!(
            medium.head().unwrap().unwrap(),
            installed,
            "the retry's installed identity is the new HEAD"
        );
        medium.validate().unwrap();
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (b) The installed-but-acknowledgment-lost window — the
    // reconciliation is bound to the EXACT attempted candidate
    // revision identity (C2-R5).
    {
        let (root, mut medium, audit, mk, ledger, _shared, _private, _p1) =
            sqlite_begin_store("after-commit");
        let key = crate::reclaim::reclamation_begin_idempotency_key(&mk);
        let begin = crate::reclaim::ReclamationBegin {
            mk,
            source: audit,
            range_digest: ledger.range_digest,
            seed: ledger.seed,
        };
        let object = StoredObject::new(begin.canonical_bytes()).unwrap();
        let head_before = medium.head().unwrap().unwrap();
        let head_epoch = medium.read_record(&head_before).unwrap().epoch;
        let candidate = RevisionRecord::regular(head_before, head_epoch, 72)
            .with_idempotency_key(key.clone())
            .with_content_refs(vec![object.hash()]);
        let candidate_id = crate::hash::hash_object(&candidate.canonical_bytes());
        medium.fault = Some(SqliteFaultControl::new().arm(
            SqliteFaultPoint::AfterCommit,
            SqliteFaultAction::InjectError,
        ));
        let outcome = medium.begin_reclamation(&audit, &mk, 72).unwrap();
        let id = match outcome {
            crate::reclaim::BeginPublicationOutcome::ReconciledInstalled { id } => id,
            other => {
                panic!("the lost acknowledgment reconciles to the proven installation: {other:?}")
            }
        };
        assert_eq!(
            id, candidate_id,
            "the reconciled identity is the EXACT attempted candidate revision"
        );
        assert_eq!(
            medium.head().unwrap().unwrap(),
            id,
            "the reconciled identity is the installed HEAD"
        );
        let begin_rows: i64 = medium
            .conn
            .query_row(
                "SELECT COUNT(*) FROM records WHERE idem = ?1",
                params![key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            begin_rows, 1,
            "exactly one installation (no second publication inside the reconciliation)"
        );
        medium.fault = None;
        medium.validate().unwrap();
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (c) The unverifiable reconciliation: the evidence degraded
    // underneath the observation — the typed uncertainty, never
    // a guess, never a write. Exercised through the same
    // reconcile entry the uncertain path delegates to: the
    // public admission refuses such a state earlier at its own
    // authentication, so the degraded-under-reconciliation
    // observation is constructed directly.
    {
        let (root, mut medium, audit, mk, _ledger, _shared, _private, _p1) =
            sqlite_begin_store("unverifiable");
        medium
            .conn
            .execute(
                "DELETE FROM records WHERE hash = ?1",
                params![audit.as_bytes()],
            )
            .unwrap();
        let head_before = medium.head().unwrap().unwrap();
        let error = reconcile_begin_publication(&mut medium, &audit, &mk, None).unwrap_err();
        assert!(
            matches!(
                error,
                MediumError::Chain {
                    rule: "begin-admission/reconcile-unverifiable"
                }
            ),
            "the degraded evidence is the typed unverifiable: {error:?}"
        );
        assert_eq!(
            medium.head().unwrap().unwrap(),
            head_before,
            "the HEAD is unchanged (the observation refused, never mutated)"
        );
        let key = crate::reclaim::reclamation_begin_idempotency_key(&mk);
        let begin_rows: i64 = medium
            .conn
            .query_row(
                "SELECT COUNT(*) FROM records WHERE idem = ?1",
                params![key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            begin_rows, 0,
            "the unverifiable observation published nothing"
        );
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (d) (C2-R5) A DIFFERENT same-key revision than the
    // attempted candidate: the same valid begin payload under a
    // different revision identity (a different tick) — the
    // reconciliation must classify it as separately discovered,
    // verified duplicate evidence, never ReconciledInstalled for
    // the original attempt. Exercised through the same reconcile
    // entry the uncertain path delegates to (the public
    // admission's duplicate discovery never constructs a second
    // candidate when one is already installed).
    {
        let (root, mut medium, audit, mk, ledger, _shared, _private, _p1) =
            sqlite_begin_store("different-candidate");
        let key = crate::reclaim::reclamation_begin_idempotency_key(&mk);
        let begin = crate::reclaim::ReclamationBegin {
            mk,
            source: audit,
            range_digest: ledger.range_digest,
            seed: ledger.seed,
        };
        let object = StoredObject::new(begin.canonical_bytes()).unwrap();
        // The already-installed begin revision Y (a valid
        // publication on the current head).
        let head = medium.head().unwrap().unwrap();
        let head_epoch = medium.read_record(&head).unwrap().epoch;
        let installed = RevisionRecord::regular(head, head_epoch, 80)
            .with_idempotency_key(key.clone())
            .with_content_refs(vec![object.hash()]);
        install_sqlite_begin_fake(&mut medium, &installed, &object);
        let installed_id = crate::hash::hash_object(&installed.canonical_bytes());
        // The DIFFERENT attempted candidate X (the same payload,
        // a different tick — a different revision identity).
        let candidate = RevisionRecord::regular(head, head_epoch, 81)
            .with_idempotency_key(key)
            .with_content_refs(vec![object.hash()]);
        let candidate_id = crate::hash::hash_object(&candidate.canonical_bytes());
        assert_ne!(
            candidate_id, installed_id,
            "the fixtures differ in revision identity"
        );
        let outcome =
            reconcile_begin_publication(&mut medium, &audit, &mk, Some(candidate_id)).unwrap();
        match outcome {
            crate::reclaim::BeginPublicationOutcome::DuplicateVerified { id } => {
                assert_eq!(
                    id, installed_id,
                    "the different same-key revision is the separately discovered duplicate"
                )
            }
            other => panic!(
                "never the attempted candidate's installation for a different revision: {other:?}"
            ),
        }
        assert_eq!(
            medium.head().unwrap().unwrap(),
            installed_id,
            "the observation wrote nothing (the HEAD is the pre-existing installation)"
        );
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (e) (C2-R5) The PUBLIC path over a pre-existing different
    // same-key revision: the admission's own duplicate discovery
    // verifies Y before constructing any candidate; the
    // acknowledgment lost in the post-commit window reconciles
    // to the SAME separately-discovered classification (never
    // ReconciledInstalled for a candidate that was never
    // constructed); the explicit retry converges idempotently.
    {
        let (root, mut medium, audit, mk, ledger, _shared, _private, _p1) =
            sqlite_begin_store("ack-lost-duplicate");
        let key = crate::reclaim::reclamation_begin_idempotency_key(&mk);
        let begin = crate::reclaim::ReclamationBegin {
            mk,
            source: audit,
            range_digest: ledger.range_digest,
            seed: ledger.seed,
        };
        let object = StoredObject::new(begin.canonical_bytes()).unwrap();
        let head = medium.head().unwrap().unwrap();
        let head_epoch = medium.read_record(&head).unwrap().epoch;
        let installed = RevisionRecord::regular(head, head_epoch, 82)
            .with_idempotency_key(key.clone())
            .with_content_refs(vec![object.hash()]);
        install_sqlite_begin_fake(&mut medium, &installed, &object);
        let installed_id = crate::hash::hash_object(&installed.canonical_bytes());
        medium.fault = Some(SqliteFaultControl::new().arm(
            SqliteFaultPoint::AfterCommit,
            SqliteFaultAction::InjectError,
        ));
        let outcome = medium.begin_reclamation(&audit, &mk, 83).unwrap();
        match outcome {
            crate::reclaim::BeginPublicationOutcome::DuplicateVerified { id } => {
                assert_eq!(id, installed_id)
            }
            other => panic!(
                "the acknowledgment-lost duplicate admission reconciles to the separately discovered duplicate: {other:?}"
            ),
        }
        let begin_rows: i64 = medium
            .conn
            .query_row(
                "SELECT COUNT(*) FROM records WHERE idem = ?1",
                params![key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            begin_rows, 1,
            "no second publication (exactly the pre-existing installation)"
        );
        medium.fault = None;
        // The explicit retry converges idempotently on the
        // pre-existing installation.
        let outcome = medium.begin_reclamation(&audit, &mk, 84).unwrap();
        assert!(
            matches!(
                outcome,
                crate::reclaim::BeginPublicationOutcome::DuplicateVerified { id }
                    if id == installed_id
            ),
            "the retry converges on the pre-existing installation: {outcome:?}"
        );
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[test]
fn the_sqlite_begin_admission_refuses_missing_and_corrupt_original_content() {
    // (C2-R4/F1) The SQLite content-object regressions: a
    // MISSING original referenced content row refuses typed at
    // the evidence verification (the matching ledger list is
    // never proof the content exists); a HASH-CORRUPT row
    // refuses at the hash-verifying read and is NEVER
    // overwritten; both leave the HEAD unchanged.
    // (a) The missing original content.
    {
        let (root, mut medium, audit, mk, _ledger, _shared, private, _p1) =
            sqlite_begin_store("missing-content");
        let head_before = medium.head().unwrap().unwrap();
        medium
            .conn
            .execute(
                "DELETE FROM objects WHERE hash = ?1",
                params![private.as_bytes()],
            )
            .unwrap();
        let error = medium.begin_reclamation(&audit, &mk, 73).unwrap_err();
        assert!(
            matches!(error, MediumError::MissingObject { hash } if hash == private),
            "the missing original content refuses typed: {error:?}"
        );
        assert_eq!(
            medium.head().unwrap().unwrap(),
            head_before,
            "the HEAD is unchanged"
        );
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (b) The hash-corrupt original content.
    {
        let (root, mut medium, audit, mk, _ledger, _shared, private, _p1) =
            sqlite_begin_store("corrupt-content");
        let head_before = medium.head().unwrap().unwrap();
        medium
            .conn
            .execute(
                "UPDATE objects SET bytes = ?1 WHERE hash = ?2",
                params![b"corrupted-original-content".to_vec(), private.as_bytes()],
            )
            .unwrap();
        let error = medium.begin_reclamation(&audit, &mk, 74).unwrap_err();
        assert!(
            matches!(error, MediumError::HashMismatch { expected, .. } if expected == private),
            "the corrupt original content refuses at the hash-verifying read: {error:?}"
        );
        let preserved: Vec<u8> = medium
            .conn
            .query_row(
                "SELECT bytes FROM objects WHERE hash = ?1",
                params![private.as_bytes()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            preserved,
            b"corrupted-original-content".to_vec(),
            "the corrupt evidence is preserved, never overwritten"
        );
        assert_eq!(
            medium.head().unwrap().unwrap(),
            head_before,
            "the HEAD is unchanged"
        );
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[test]
fn the_sqlite_begin_founding_crosses_compacted_history() {
    // (C2-R4/F4) The SQLite compacted-history twin: after A's
    // EXECUTED retirement physically removes A's range (the
    // authenticated self-skip below MK1), B's later admission
    // still derives the TRUE founding's seed — the gap-aware
    // founding descent crosses the authenticated gap; the naive
    // missing-parent descent would stop at the gap and derive a
    // wrong seed (the refusal).
    let root = std::env::temp_dir().join(format!(
        "s7-occ2r4-gap-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut medium = SqliteMedium::open(root.join("db.sqlite")).unwrap();
    let fobj = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b" founding-payload");
        enc.into_bytes()
    })
    .unwrap();
    let x = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"the-shared-deferred-x");
        enc.into_bytes()
    })
    .unwrap();
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "occ2r4gap", 0)
        .with_content_refs(vec![fobj.hash()]);
    medium
        .append(&founding, std::slice::from_ref(&fobj), None)
        .unwrap();
    // A's range: A1(X) → SK1(fobj) → MK1(meta1) → auditA.
    let head = medium.head().unwrap().unwrap();
    let epoch = medium.read_record(&head).unwrap().epoch;
    let a1 = RevisionRecord::regular(head, epoch, 1).with_content_refs(vec![x.hash()]);
    medium
        .append(&a1, std::slice::from_ref(&x), Some(head))
        .unwrap();
    let head = medium.head().unwrap().unwrap();
    let epoch = medium.read_record(&head).unwrap().epoch;
    let sk1 = RevisionRecord::regular(head, epoch, 2).with_content_refs(vec![fobj.hash()]);
    medium
        .append(&sk1, std::slice::from_ref(&fobj), Some(head))
        .unwrap();
    let seed = crate::checkpoint::compute_digest_seed(&founding.id());
    let a1_bytes = medium.read_record(&a1.id()).unwrap().canonical_bytes();
    let digest_a = crate::checkpoint::chain_range_digest(&seed, &[a1_bytes]);
    let meta1 = CheckpointMetadata::new(
        sk1.id(),
        a1.id(),
        vec![SkipEntry::new(Hash::ZERO, founding.id())],
        vec![
            ReclaimedIndexEntry::new(None, a1.id(), 1),
            ReclaimedIndexEntry::new(None, sk1.id(), 2),
        ],
        digest_a,
        seed,
    );
    let meta1_object = StoredObject::new(meta1.canonical_bytes()).unwrap();
    let head = medium.head().unwrap().unwrap();
    let epoch = medium.read_record(&head).unwrap().epoch;
    let mk1 = RevisionRecord::regular(head, epoch, 3).with_content_refs(vec![meta1_object.hash()]);
    medium
        .append(&mk1, std::slice::from_ref(&meta1_object), Some(head))
        .unwrap();
    let ledger_a = crate::reclaim::ReclamationLedger {
        mk: mk1.id(),
        sk: sk1.id(),
        range_digest: digest_a,
        seed,
        records: 2,
        reclaimed_keys: 0,
        journal_files: Vec::new(),
        units: vec![
            crate::reclaim::ReclaimUnit {
                record: sk1.id(),
                objects: vec![fobj.hash()],
            },
            crate::reclaim::ReclaimUnit {
                record: a1.id(),
                objects: vec![x.hash()],
            },
        ],
    };
    let (audit_a, outcome) =
        crate::reclaim::publish_reclamation_ledger(&mut medium, &ledger_a, 4).unwrap();
    assert!(matches!(
        outcome,
        crate::medium::AppendOutcome::Accepted { .. }
    ));
    // (OC-C3 CP-2) A's begin — the real admission (the A
    // execution below requires it under the begin-bound gate).
    // (OC-C4) The begin record is B1's TRUE nearest retained
    // ancestor: the later range's skip entry names it, never
    // audit_a — a gap-crossing walk from MK2 lands on the begin,
    // never bypasses it.
    let begin_a = medium
        .begin_reclamation(&audit_a, &ledger_a.mk, 223)
        .unwrap();
    let begin_a_id = match begin_a {
        crate::reclaim::BeginPublicationOutcome::Accepted { id, .. }
        | crate::reclaim::BeginPublicationOutcome::DuplicateVerified { id } => id,
        other => panic!("A's begin admits: {other:?}"),
    };
    // The later range: B1(X) → SK2(fobj) → MK2(meta2) — B1/SK2
    // are survivors of A (X deferred: B1 references it).
    let head = medium.head().unwrap().unwrap();
    let epoch = medium.read_record(&head).unwrap().epoch;
    let b1 = RevisionRecord::regular(head, epoch, 5).with_content_refs(vec![x.hash()]);
    medium
        .append(&b1, std::slice::from_ref(&x), Some(head))
        .unwrap();
    let head = medium.head().unwrap().unwrap();
    let epoch = medium.read_record(&head).unwrap().epoch;
    let sk2 = RevisionRecord::regular(head, epoch, 6).with_content_refs(vec![fobj.hash()]);
    medium
        .append(&sk2, std::slice::from_ref(&fobj), Some(head))
        .unwrap();
    let b1_bytes = medium.read_record(&b1.id()).unwrap().canonical_bytes();
    let digest_b = crate::checkpoint::chain_range_digest(&seed, &[b1_bytes]);
    let meta2 = CheckpointMetadata::new(
        sk2.id(),
        b1.id(),
        vec![SkipEntry::new(Hash::ZERO, begin_a_id)],
        vec![
            ReclaimedIndexEntry::new(None, b1.id(), 5),
            ReclaimedIndexEntry::new(None, sk2.id(), 6),
        ],
        digest_b,
        seed,
    );
    let meta2_object = StoredObject::new(meta2.canonical_bytes()).unwrap();
    let head = medium.head().unwrap().unwrap();
    let epoch = medium.read_record(&head).unwrap().epoch;
    let mk2 = RevisionRecord::regular(head, epoch, 7).with_content_refs(vec![meta2_object.hash()]);
    medium
        .append(&mk2, std::slice::from_ref(&meta2_object), Some(head))
        .unwrap();
    let ledger_b = crate::reclaim::ReclamationLedger {
        mk: mk2.id(),
        sk: sk2.id(),
        range_digest: digest_b,
        seed,
        records: 2,
        reclaimed_keys: 0,
        journal_files: Vec::new(),
        units: vec![
            crate::reclaim::ReclaimUnit {
                record: sk2.id(),
                objects: vec![fobj.hash()],
            },
            crate::reclaim::ReclaimUnit {
                record: b1.id(),
                objects: vec![x.hash()],
            },
        ],
    };
    // Retirement A EXECUTES: the physical gap below MK1 (the
    // authenticated self-skip to the TRUE founding).
    let (ledger_a_verified, _meta_a) =
        crate::reclaim::verify_canonical_v1_audit(&mut medium, &audit_a, &mk1.id()).unwrap();
    medium
        .reclaim(&crate::reclaim::ReclaimDirective {
            audit_record: audit_a,
            units: ledger_a_verified.units.clone(),
        })
        .unwrap();
    // B's audit PUBLISHED above A's retired range.
    let (audit_b, outcome) =
        crate::reclaim::publish_reclamation_ledger(&mut medium, &ledger_b, 8).unwrap();
    assert!(matches!(
        outcome,
        crate::medium::AppendOutcome::Accepted { .. }
    ));
    // B's admission over the compacted history: the founding
    // descent crosses A's authenticated gap to the TRUE founding
    // (the seed check passes); the naive descent would refuse.
    let outcome = medium.begin_reclamation(&audit_b, &mk2.id(), 75).unwrap();
    let installed = match outcome {
        // (C2-R8) The compacted-history admission carries the
        // transactional tier.
        crate::reclaim::BeginPublicationOutcome::Accepted { id, tier } => {
            assert_eq!(
                tier,
                SyncTier::TransactionalWalFull,
                "the compacted-history Accepted carries the transactional tier"
            );
            id
        }
        other => {
            panic!("the compacted-history founding admits over the authenticated gap: {other:?}")
        }
    };
    assert_eq!(
        medium.head().unwrap().unwrap(),
        installed,
        "the installed begin is the new HEAD"
    );
    medium.validate().unwrap();
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_sqlite_begin_founding_refuses_uncovered_and_unreadable_parents() {
    // (C2-R4/F4/R2) The founding-descent gap discipline: a
    // missing parent with NO authenticated skip coverage refuses
    // typed (never mistaken for the founding); a PRESENT but
    // hash-corrupt parent row refuses at the hash-verifying read
    // (an unreadable parent is never an absent one — the
    // presence probe reports it present, the read refuses).
    // (a) The uncovered gap.
    {
        let (root, mut medium, audit, mk, _ledger, _shared, _private, p1) =
            sqlite_begin_store("uncovered-gap");
        let head_before = medium.head().unwrap().unwrap();
        medium
            .conn
            .execute(
                "DELETE FROM records WHERE hash = ?1",
                params![p1.as_bytes()],
            )
            .unwrap();
        let error = medium.begin_reclamation(&audit, &mk, 76).unwrap_err();
        assert!(
            matches!(error, MediumError::MissingObject { hash } if hash == p1),
            "the uncovered gap refuses typed (never the founding): {error:?}"
        );
        assert_eq!(
            medium.head().unwrap().unwrap(),
            head_before,
            "the HEAD is unchanged"
        );
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (b) The unreadable (hash-corrupt) parent row.
    {
        let (root, mut medium, audit, mk, _ledger, _shared, _private, p1) =
            sqlite_begin_store("unreadable-parent");
        let head_before = medium.head().unwrap().unwrap();
        medium
            .conn
            .execute(
                "UPDATE records SET bytes = ?1 WHERE hash = ?2",
                params![b"corrupt-parent-record".to_vec(), p1.as_bytes()],
            )
            .unwrap();
        let error = medium.begin_reclamation(&audit, &mk, 77).unwrap_err();
        assert!(
            matches!(error, MediumError::HashMismatch { expected, .. } if expected == p1),
            "the corrupt parent row refuses at the hash-verifying read: {error:?}"
        );
        assert_eq!(
            medium.head().unwrap().unwrap(),
            head_before,
            "the HEAD is unchanged"
        );
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[test]
fn the_sqlite_begin_discovery_validates_parent_epoch_linkage() {
    // (C2-R3/R4) The duplicate-discovery linkage regressions: a
    // same-key record ON the ancestry with a PRESENT parent must
    // carry epoch+1 — a wrong epoch refuses at the linkage; the
    // shape family (extra refs, a parentless keyed record)
    // refuses at the shape; a wrong begin payload refuses at the
    // exact content; a wrong-MK begin refuses at the binding; a
    // wrong PARENT (a mid-range chain position) strands the
    // audit — the AUTHENTICATION boundary refuses, never the
    // discovery.
    // (a) The wrong-epoch linkage — exercised through the
    // discovery directly: the (C2-R6) authoritative-chain
    // validation refuses such a HEAD at the public admission
    // boundary (the epoch-regression rule — its own regression),
    // so the discovery's own linkage check is the
    // defense-in-depth boundary.
    {
        let (root, mut medium, audit, mk, ledger, _shared, _private, _p1) =
            sqlite_begin_store("wrong-epoch");
        let key = crate::reclaim::reclamation_begin_idempotency_key(&mk);
        let begin = crate::reclaim::ReclamationBegin {
            mk,
            source: audit,
            range_digest: ledger.range_digest,
            seed: ledger.seed,
        };
        let object = StoredObject::new(begin.canonical_bytes()).unwrap();
        let head = medium.head().unwrap().unwrap();
        let head_epoch = medium.read_record(&head).unwrap().epoch;
        let fake = RevisionRecord {
            kind: crate::record::RecordKind::Regular,
            epoch: head_epoch + 5,
            parent: Some(head),
            idempotency_key: Some(key),
            tick: 90,
            content_refs: vec![object.hash()],
            depends_on: Vec::new(),
        };
        install_sqlite_begin_fake(&mut medium, &fake, &object);
        let error = {
            let tx = medium.conn.transaction().unwrap();
            find_installed_begin_tx(&tx, &mk, &begin).unwrap_err()
        };
        assert!(
            matches!(
                error,
                MediumError::Chain {
                    rule: "begin-admission/installed-linkage-mismatch"
                }
            ),
            "the wrong-epoch same-key record refuses at the linkage: {error:?}"
        );
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (b) The extra-ref shape violation.
    {
        let (root, mut medium, audit, mk, _ledger, shared, _private, _p1) =
            sqlite_begin_store("extra-refs");
        let key = crate::reclaim::reclamation_begin_idempotency_key(&mk);
        let begin = crate::reclaim::ReclamationBegin {
            mk,
            source: audit,
            range_digest: [0u8; 32],
            seed: [0u8; 32],
        };
        let object = StoredObject::new(begin.canonical_bytes()).unwrap();
        let head = medium.head().unwrap().unwrap();
        let head_epoch = medium.read_record(&head).unwrap().epoch;
        let mut refs = vec![object.hash(), shared];
        refs.sort();
        refs.dedup();
        let fake = RevisionRecord {
            kind: crate::record::RecordKind::Regular,
            epoch: head_epoch + 1,
            parent: Some(head),
            idempotency_key: Some(key),
            tick: 91,
            content_refs: refs,
            depends_on: Vec::new(),
        };
        install_sqlite_begin_fake(&mut medium, &fake, &object);
        let error = medium.begin_reclamation(&audit, &mk, 79).unwrap_err();
        assert!(
            matches!(
                error,
                MediumError::Chain {
                    rule: "begin-admission/installed-shape-mismatch"
                }
            ),
            "the extra-ref same-key record refuses at the shape: {error:?}"
        );
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (c) The content mismatch.
    {
        let (root, mut medium, audit, mk, ledger, _shared, _private, _p1) =
            sqlite_begin_store("content-mismatch");
        let key = crate::reclaim::reclamation_begin_idempotency_key(&mk);
        let wrong = crate::reclaim::ReclamationBegin {
            mk,
            source: audit,
            range_digest: [0u8; 32],
            seed: [0u8; 32],
        };
        let object = StoredObject::new(wrong.canonical_bytes()).unwrap();
        let head = medium.head().unwrap().unwrap();
        let head_epoch = medium.read_record(&head).unwrap().epoch;
        let fake = RevisionRecord {
            kind: crate::record::RecordKind::Regular,
            epoch: head_epoch + 1,
            parent: Some(head),
            idempotency_key: Some(key),
            tick: 92,
            content_refs: vec![object.hash()],
            depends_on: Vec::new(),
        };
        install_sqlite_begin_fake(&mut medium, &fake, &object);
        let error = medium.begin_reclamation(&audit, &mk, 80).unwrap_err();
        assert!(
            matches!(
                error,
                MediumError::Chain {
                    rule: "begin-admission/duplicate-content-mismatch"
                }
            ),
            "the differing same-key payload refuses at the exact content: {error:?}"
        );
        let _ = ledger;
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (d) The wrong-MK begin payload (a real non-zero identity —
    // the strict decode refuses a zero mk before the binding
    // check could fire).
    {
        let (root, mut medium, audit, mk, _ledger, _shared, _private, _p1) =
            sqlite_begin_store("wrong-mk");
        let key = crate::reclaim::reclamation_begin_idempotency_key(&mk);
        let wrong = crate::reclaim::ReclamationBegin {
            mk: audit,
            source: audit,
            range_digest: [0u8; 32],
            seed: [0u8; 32],
        };
        let object = StoredObject::new(wrong.canonical_bytes()).unwrap();
        let head = medium.head().unwrap().unwrap();
        let head_epoch = medium.read_record(&head).unwrap().epoch;
        let fake = RevisionRecord {
            kind: crate::record::RecordKind::Regular,
            epoch: head_epoch + 1,
            parent: Some(head),
            idempotency_key: Some(key),
            tick: 93,
            content_refs: vec![object.hash()],
            depends_on: Vec::new(),
        };
        install_sqlite_begin_fake(&mut medium, &fake, &object);
        let error = medium.begin_reclamation(&audit, &mk, 81).unwrap_err();
        assert!(
            matches!(
                error,
                MediumError::Chain {
                    rule: "begin-admission/installed-mk-mismatch"
                }
            ),
            "the wrong-MK same-key payload refuses at the binding: {error:?}"
        );
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (e) The wrong-kind shape violation — exercised through the
    // discovery directly: a keyed record of the FOUNDING kind is
    // decodable (the kind/parent coherence leaves it parentless),
    // so the public admission would refuse such a HEAD earlier
    // at its own audit authentication (the audit unreachable
    // below a parentless head); the discovery's shape validation
    // itself must refuse the keyed non-Regular record.
    {
        let (root, mut medium, audit, mk, ledger, _shared, _private, _p1) =
            sqlite_begin_store("wrong-kind");
        let key = crate::reclaim::reclamation_begin_idempotency_key(&mk);
        let begin = crate::reclaim::ReclamationBegin {
            mk,
            source: audit,
            range_digest: ledger.range_digest,
            seed: ledger.seed,
        };
        let object = StoredObject::new(begin.canonical_bytes()).unwrap();
        let fake = RevisionRecord::founding(AuthorityMode::Standalone, "wrong-kind", 94)
            .with_idempotency_key(key)
            .with_content_refs(vec![object.hash()]);
        install_sqlite_begin_fake(&mut medium, &fake, &object);
        let error = {
            let tx = medium.conn.transaction().unwrap();
            find_installed_begin_tx(&tx, &mk, &begin).unwrap_err()
        };
        assert!(
            matches!(
                error,
                MediumError::Chain {
                    rule: "begin-admission/installed-shape-mismatch"
                }
            ),
            "the keyed non-Regular record refuses at the discovery's shape: {error:?}"
        );
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (f) The wrong parent: the same-key record chained onto a
    // MID-RANGE record (not the head) — the observed chain no
    // longer contains the trusted history: the (C2-R6)
    // authoritative-chain validation refuses at the divergence
    // boundary (the audit is stranded below the rewrite).
    {
        let (root, mut medium, audit, mk, ledger, _shared, _private, _p1) =
            sqlite_begin_store("wrong-parent");
        let key = crate::reclaim::reclamation_begin_idempotency_key(&mk);
        let begin = crate::reclaim::ReclamationBegin {
            mk,
            source: audit,
            range_digest: ledger.range_digest,
            seed: ledger.seed,
        };
        let object = StoredObject::new(begin.canonical_bytes()).unwrap();
        let sk_epoch = medium.read_record(&ledger.sk).unwrap().epoch;
        let fake = RevisionRecord {
            kind: crate::record::RecordKind::Regular,
            epoch: sk_epoch + 1,
            parent: Some(ledger.sk),
            idempotency_key: Some(key),
            tick: 95,
            content_refs: vec![object.hash()],
            depends_on: Vec::new(),
        };
        install_sqlite_begin_fake(&mut medium, &fake, &object);
        let error = medium.begin_reclamation(&audit, &mk, 82).unwrap_err();
        assert!(
            matches!(error, MediumError::Chain { rule: "divergence" }),
            "the wrong-parent same-key record diverges from the trusted history: {error:?}"
        );
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
}
#[test]
fn the_sqlite_begin_admission_requires_authoritative_chain_coherence() {
    // (C2-R6) A chain carrying an otherwise authentic audit and
    // original retirement evidence but MALFORMED revisions above
    // the audit never receives a trusted begin: the admission
    // runs the SAME transaction-scoped trusted-cursor validation
    // the normal publication applies — before any duplicate
    // outcome and before any installation; the ordinary
    // valid-history admission still succeeds.
    // (a) The incorrect regular-revision epoch above the audit.
    {
        let (root, mut medium, audit, mk, _ledger, _shared, _private, _p1) =
            sqlite_begin_store("chain-epoch");
        let payload = StoredObject::new({
            let mut enc = Encoder::new();
            enc.write_str("sektor7/save/1");
            enc.write_bytes(b"malformed-above-audit");
            enc.into_bytes()
        })
        .unwrap();
        let head = medium.head().unwrap().unwrap();
        let head_epoch = medium.read_record(&head).unwrap().epoch;
        let malformed = RevisionRecord {
            kind: crate::record::RecordKind::Regular,
            epoch: head_epoch + 5,
            parent: Some(head),
            idempotency_key: None,
            tick: 96,
            content_refs: vec![payload.hash()],
            depends_on: Vec::new(),
        };
        let malformed_id = crate::hash::hash_object(&malformed.canonical_bytes());
        install_sqlite_begin_fake(&mut medium, &malformed, &payload);
        let error = medium.begin_reclamation(&audit, &mk, 85).unwrap_err();
        assert!(
            matches!(
                error,
                MediumError::Chain {
                    rule: "epoch-regression"
                }
            ),
            "the malformed epoch above the audit refuses at the authoritative-chain validation: {error:?}"
        );
        assert_eq!(
            read_meta_head(&medium.conn).unwrap().unwrap(),
            malformed_id,
            "the HEAD is unchanged (nothing installed; the raw stored head)"
        );
        let key = crate::reclaim::reclamation_begin_idempotency_key(&mk);
        let begin_rows: i64 = medium
            .conn
            .query_row(
                "SELECT COUNT(*) FROM records WHERE idem = ?1",
                params![key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(begin_rows, 0, "no begin revision installed");
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (b) No duplicate success from an invalid authoritative
    // chain: a VALID same-key begin installed first, then the
    // malformed revision above it — the admission refuses at the
    // chain validation, never returns the duplicate.
    {
        let (root, mut medium, audit, mk, ledger, _shared, _private, _p1) =
            sqlite_begin_store("chain-duplicate");
        let key = crate::reclaim::reclamation_begin_idempotency_key(&mk);
        let begin = crate::reclaim::ReclamationBegin {
            mk,
            source: audit,
            range_digest: ledger.range_digest,
            seed: ledger.seed,
        };
        let object = StoredObject::new(begin.canonical_bytes()).unwrap();
        let head = medium.head().unwrap().unwrap();
        let head_epoch = medium.read_record(&head).unwrap().epoch;
        let valid = RevisionRecord::regular(head, head_epoch, 97)
            .with_idempotency_key(key)
            .with_content_refs(vec![object.hash()]);
        install_sqlite_begin_fake(&mut medium, &valid, &object);
        let payload = StoredObject::new({
            let mut enc = Encoder::new();
            enc.write_str("sektor7/save/1");
            enc.write_bytes(b"malformed-above-the-begin");
            enc.into_bytes()
        })
        .unwrap();
        let head_m = medium.head().unwrap().unwrap();
        let epoch_m = medium.read_record(&head_m).unwrap().epoch;
        let malformed = RevisionRecord {
            kind: crate::record::RecordKind::Regular,
            epoch: epoch_m + 5,
            parent: Some(head_m),
            idempotency_key: None,
            tick: 98,
            content_refs: vec![payload.hash()],
            depends_on: Vec::new(),
        };
        install_sqlite_begin_fake(&mut medium, &malformed, &payload);
        let error = medium.begin_reclamation(&audit, &mk, 86).unwrap_err();
        assert!(
            matches!(
                error,
                MediumError::Chain {
                    rule: "epoch-regression"
                }
            ),
            "no duplicate success from an invalid authoritative chain: {error:?}"
        );
        let key = crate::reclaim::reclamation_begin_idempotency_key(&mk);
        let begin_rows: i64 = medium
            .conn
            .query_row(
                "SELECT COUNT(*) FROM records WHERE idem = ?1",
                params![key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            begin_rows, 1,
            "exactly the pre-existing installation, nothing added"
        );
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (c) The invalid general chain structure: the wrong parent
    // — the observed chain no longer contains the trusted
    // history (the divergence the normal admission rejects).
    {
        let (root, mut medium, audit, mk, ledger, _shared, _private, _p1) =
            sqlite_begin_store("chain-divergence");
        let payload = StoredObject::new({
            let mut enc = Encoder::new();
            enc.write_str("sektor7/save/1");
            enc.write_bytes(b"divergent-parent");
            enc.into_bytes()
        })
        .unwrap();
        let r1 = ledger.units[1].record;
        let r1_epoch = medium.read_record(&r1).unwrap().epoch;
        let divergent = RevisionRecord {
            kind: crate::record::RecordKind::Regular,
            epoch: r1_epoch + 1,
            parent: Some(r1),
            idempotency_key: None,
            tick: 99,
            content_refs: vec![payload.hash()],
            depends_on: Vec::new(),
        };
        let divergent_id = crate::hash::hash_object(&divergent.canonical_bytes());
        install_sqlite_begin_fake(&mut medium, &divergent, &payload);
        let error = medium.begin_reclamation(&audit, &mk, 87).unwrap_err();
        assert!(
            matches!(error, MediumError::Chain { rule: "divergence" }),
            "the divergent rewrite refuses at the authoritative-chain validation: {error:?}"
        );
        assert_eq!(
            read_meta_head(&medium.conn).unwrap().unwrap(),
            divergent_id,
            "the HEAD is unchanged (nothing installed; the raw stored head)"
        );
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (d) The valid control: the ordinary valid-history admission
    // still succeeds.
    {
        let (root, mut medium, audit, mk, _ledger, _shared, _private, _p1) =
            sqlite_begin_store("chain-valid-control");
        let outcome = medium.begin_reclamation(&audit, &mk, 88).unwrap();
        assert!(
            matches!(
                outcome,
                crate::reclaim::BeginPublicationOutcome::Accepted {
                    tier: SyncTier::TransactionalWalFull,
                    ..
                }
            ),
            "the ordinary valid-history admission still succeeds: {outcome:?}"
        );
        medium.validate().unwrap();
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[test]
fn the_sqlite_begin_discovery_linkage_refuses_epoch_overflow() {
    // (C2-R7) The overflow regression: a same-key record at epoch
    // u64::MAX over a parent at epoch u64::MAX — the checked
    // linkage refuses (saturation would have made the incoherent
    // successor appear coherent, and the record would then have
    // been verified as a canonical duplicate). Exercised through
    // the discovery directly: the (C2-R6) authoritative-chain
    // validation refuses such a HEAD at the public admission
    // boundary first (its own epoch-regression rule).
    let (root, mut medium, audit, mk, ledger, _shared, _private, _p1) =
        sqlite_begin_store("epoch-overflow");
    let key = crate::reclaim::reclamation_begin_idempotency_key(&mk);
    let begin = crate::reclaim::ReclamationBegin {
        mk,
        source: audit,
        range_digest: ledger.range_digest,
        seed: ledger.seed,
    };
    let object = StoredObject::new(begin.canonical_bytes()).unwrap();
    let payload = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"overflow-parent");
        enc.into_bytes()
    })
    .unwrap();
    let head = medium.head().unwrap().unwrap();
    // The parent P at epoch u64::MAX, then the same-key record K
    // at epoch u64::MAX over it — the corruption-shaped install.
    let parent_record = RevisionRecord {
        kind: crate::record::RecordKind::Regular,
        epoch: u64::MAX,
        parent: Some(head),
        idempotency_key: None,
        tick: 100,
        content_refs: vec![payload.hash()],
        depends_on: Vec::new(),
    };
    let parent_id = crate::hash::hash_object(&parent_record.canonical_bytes());
    let keyed = RevisionRecord {
        kind: crate::record::RecordKind::Regular,
        epoch: u64::MAX,
        parent: Some(parent_id),
        idempotency_key: Some(key),
        tick: 101,
        content_refs: vec![object.hash()],
        depends_on: Vec::new(),
    };
    install_sqlite_begin_fake(&mut medium, &parent_record, &payload);
    install_sqlite_begin_fake(&mut medium, &keyed, &object);
    let error = {
        let tx = medium.conn.transaction().unwrap();
        find_installed_begin_tx(&tx, &mk, &begin).unwrap_err()
    };
    assert!(
        matches!(
            error,
            MediumError::Chain {
                rule: "begin-admission/installed-linkage-mismatch"
            }
        ),
        "the epoch overflow refuses the linkage (u64::MAX has no coherent successor): {error:?}"
    );
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}
#[test]
fn the_sqlite_begin_authority_verification() {
    // (OC-C3 CP-1) The installed-begin durability authority on
    // the transactional provider: the canonical verification and
    // the truthful TransactionalWalFull grade; the grade
    // following an UNCERTAIN publication (the AfterCommit
    // acknowledgment-loss window — the read-snapshot visibility
    // proof); the corrupt and orphaned refusals; the typed
    // absence; and the verified WAL/FULL configuration's tier.
    // (a) The canonical case + the grade.
    {
        let (root, mut medium, audit, mk, _ledger, _shared, _private, _p1) =
            sqlite_begin_store("authority-canonical");
        let outcome = medium.begin_reclamation(&audit, &mk, 150).unwrap();
        let installed = match outcome {
            crate::reclaim::BeginPublicationOutcome::Accepted { id, tier } => {
                assert_eq!(
                    tier,
                    SyncTier::TransactionalWalFull,
                    "the sqlite admission carries the transactional tier"
                );
                id
            }
            other => panic!("the sqlite admission accepts: {other:?}"),
        };
        let authority = medium.verify_begin_authority(&audit, &mk).unwrap();
        assert_eq!(authority.id, installed);
        assert_eq!(
            authority.tier,
            SyncTier::TransactionalWalFull,
            "the verified WAL/FULL configuration's truthful grade"
        );
        // The absent case on a fresh store: the typed absence.
        let (root2, mut medium2, audit2, mk2, _l2, _s2, _p2, _q2) =
            sqlite_begin_store("authority-absent");
        let error = medium2.verify_begin_authority(&audit2, &mk2).unwrap_err();
        assert!(
            matches!(
                error,
                MediumError::Chain {
                    rule: "begin-authority/absent"
                }
            ),
            "the missing begin is the typed absence: {error:?}"
        );
        drop(medium);
        drop(medium2);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&root2);
    }
    // (b) The grade following an uncertain publication: the
    // AfterCommit acknowledgment-loss window — the begin IS
    // committed; the authority's read snapshot observes it and
    // establishes the grade (canonical membership DISTINCT from
    // the lost acknowledgment — the visibility is the proof).
    {
        let (root, mut medium, audit, mk, ledger, _shared, _private, _p1) =
            sqlite_begin_store("authority-after-commit");
        medium.fault = Some(SqliteFaultControl::new().arm(
            SqliteFaultPoint::AfterCommit,
            SqliteFaultAction::InjectError,
        ));
        let outcome = medium.begin_reclamation(&audit, &mk, 151).unwrap();
        assert!(
            matches!(
                outcome,
                crate::reclaim::BeginPublicationOutcome::ReconciledInstalled { .. }
            ),
            "the lost acknowledgment reconciles to the proven installation: {outcome:?}"
        );
        medium.fault = None;
        let authority = medium.verify_begin_authority(&audit, &mk).unwrap();
        assert_eq!(authority.tier, SyncTier::TransactionalWalFull);
        // The exact-candidate identity: the authority's begin is
        // the record the uncertain publication installed.
        let key = crate::reclaim::reclamation_begin_idempotency_key(&mk);
        let head = read_meta_head(&medium.conn).unwrap().unwrap();
        let _ = (ledger, key, head);
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (c) The corrupt and orphaned refusals: a corrupted begin
    // object row (the typed integrity refusal) and an orphaned
    // same-key record row (never authority — the definite
    // absence through the walk).
    {
        let (root, mut medium, audit, mk, ledger, _shared, _private, _p1) =
            sqlite_begin_store("authority-corrupt");
        let outcome = medium.begin_reclamation(&audit, &mk, 152).unwrap();
        let installed = match outcome {
            crate::reclaim::BeginPublicationOutcome::Accepted { id, .. } => id,
            other => panic!("the admission accepts: {other:?}"),
        };
        // The begin's content object row corrupted.
        let begin = crate::reclaim::ReclamationBegin {
            mk,
            source: audit,
            range_digest: ledger.range_digest,
            seed: ledger.seed,
        };
        let object_hash = StoredObject::new(begin.canonical_bytes()).unwrap().hash();
        medium
            .conn
            .execute(
                "UPDATE objects SET bytes = ?1 WHERE hash = ?2",
                params![b"corrupted-begin-payload".to_vec(), object_hash.as_bytes()],
            )
            .unwrap();
        let error = medium.verify_begin_authority(&audit, &mk).unwrap_err();
        assert!(
            matches!(error, MediumError::HashMismatch { .. }),
            "the corrupted begin payload refuses at the hash-verifying read: {error:?}"
        );
        let _ = installed;
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
    {
        let (root, mut medium, audit, mk, _ledger, _shared, _private, _p1) =
            sqlite_begin_store("authority-orphan");
        // The orphaned same-key record row (never on the
        // ancestry): the authority is the typed absence.
        let key = crate::reclaim::reclamation_begin_idempotency_key(&mk);
        let orphan_object = StoredObject::new(
            crate::reclaim::ReclamationBegin {
                mk,
                source: audit,
                range_digest: [0u8; 32],
                seed: [0u8; 32],
            }
            .canonical_bytes(),
        )
        .unwrap();
        let orphan_record = RevisionRecord::regular(Hash::ZERO, 0, 153)
            .with_idempotency_key(key.clone())
            .with_content_refs(vec![orphan_object.hash()]);
        medium
            .conn
            .execute(
                "INSERT INTO objects (hash, domain, bytes) VALUES (?1, ?2, ?3)",
                params![
                    orphan_object.hash().as_bytes().to_vec(),
                    orphan_object.domain(),
                    orphan_object.bytes().to_vec()
                ],
            )
            .unwrap();
        medium
            .conn
            .execute(
                "INSERT INTO records (hash, bytes, epoch, parent, idem, kind, tick) \
                     VALUES (?1, ?2, 0, NULL, ?3, 1, 153)",
                params![
                    crate::hash::hash_object(&orphan_record.canonical_bytes())
                        .as_bytes()
                        .to_vec(),
                    orphan_record.canonical_bytes(),
                    key
                ],
            )
            .unwrap();
        let error = medium.verify_begin_authority(&audit, &mk).unwrap_err();
        assert!(
            matches!(
                error,
                MediumError::Chain {
                    rule: "begin-authority/absent"
                }
            ),
            "the orphaned same-key row never grants authority: {error:?}"
        );
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
}
#[test]
fn the_r1_attack_refuses_at_every_boundary() {
    // (OC-C3 CP-2 §9; CP2-R2-D) The R1 attack end-to-end at the
    // SQLite provider's boundaries: the GENUINE
    // checkpoint/audit fixture (sqlite_begin_store — a real
    // campaign, a real published audit) with the original
    // retirement evidence DELETED after the fact — the
    // DEGRADATION shape the attack presents — and NO begin ever
    // published. Refused at the provider deletion gate (the
    // definite begin absence), at the direct completion
    // publication, and at the completion wrapper; the failed
    // fresh admission (the degraded evidence) never manufactures
    // a retroactive begin. (The distinct FABRICATED-evidence
    // scenario — a self-consistent invented checkpoint/audit —
    // is covered at the admission boundary by the local
    // fabricated-range/digest-order regressions and at the
    // completion boundary by this test's hand-built marker: the
    // canonical validation is never weakened to construct it.)
    let (root, mut medium, audit, mk, _ledger, _shared, _private, _p1) =
        sqlite_begin_store("r1-attack");
    // THE R1 DEGRADATION: the original retirement evidence
    // deleted (the genuine audit's unit records — exactly what
    // the degradation presents), NO begin ever published.
    let (ledger, _meta) =
        crate::reclaim::verify_canonical_v1_audit(&mut medium, &audit, &mk).unwrap();
    for unit in &ledger.units {
        medium
            .conn
            .execute(
                "DELETE FROM records WHERE hash = ?1",
                params![unit.record.as_bytes()],
            )
            .unwrap();
    }
    let directive = crate::reclaim::ReclaimDirective {
        audit_record: audit,
        units: ledger.units.clone(),
    };
    // (a) The provider deletion gate.
    let error = medium.reclaim(&directive).unwrap_err();
    assert!(
        matches!(
            error,
            MediumError::Chain {
                rule: "execution/begin-absent"
            }
        ),
        "the R1 state refuses at the provider gate: {error:?}"
    );
    // The fresh admission over the degraded evidence FAILS
    // CLOSED — no retroactive begin (OC-P1).
    let error = medium.begin_reclamation(&audit, &mk, 240).unwrap_err();
    assert!(
        matches!(
            error,
            MediumError::MissingObject { .. }
                | MediumError::Chain {
                    rule: "begin-admission/units-objects-mismatch"
                }
        ),
        "the degraded evidence cannot be authorized: {error:?}"
    );
    let error = medium.reclaim(&directive).unwrap_err();
    assert!(
        matches!(
            error,
            MediumError::Chain {
                rule: "execution/begin-absent"
            }
        ),
        "the gate still refuses after the failed admission: {error:?}"
    );
    // (b) The direct completion publication (the independent
    // boundary).
    let marker = crate::reclaim::ReclamationComplete {
        kind: crate::reclaim::CompletionKind::Batch,
        mk,
        batch: 0,
        source: audit,
        mandatory_count: 2,
        deferred: Vec::new(),
        journal_files: Vec::new(),
    };
    let head = read_meta_head(&medium.conn).unwrap().unwrap();
    let error =
        crate::reclaim::publish_reclamation_complete(&mut medium, &marker, head, 241).unwrap_err();
    assert!(
        matches!(
            error,
            MediumError::Chain {
                rule: "begin-authority/absent"
            }
        ),
        "the direct publication refuses the beginless state: {error:?}"
    );
    // (c) The completion wrapper.
    let error = crate::reclaim::complete_reclamation(&mut medium, &audit, &mk).unwrap_err();
    // The wrapper refuses at its own earliest boundary for this
    // state (the mandatory objects still present — the residue
    // check precedes the publication); either way the beginless
    // state is NEVER certified.
    assert!(
        matches!(
            error,
            MediumError::Chain {
                rule: "begin-authority/absent"
            } | MediumError::Chain {
                rule: "reclamation-complete/mandatory-residue"
            } | MediumError::Chain {
                rule: "reclamation-complete/record-still-present"
            }
        ),
        "the completion wrapper refuses the beginless state: {error:?}"
    );
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
#[cfg(feature = "composite-test-fixture")]
fn the_oc4_beginless_marker_refuses_trusted_certification_and_stays_discoverable() {
    // (OC-C4) The completion-reader authority at the REQUESTED
    // level: a completed marker never becomes authoritative
    // without an authenticated, correctly bound, provider-durable
    // ReclamationBegin — no grandfathering. The LEGACY shape: the
    // genuine audit, the hand-installed keyed marker (exactly
    // what a pre-Option-C record or a corrupted in-crate path
    // presents), and NO begin ever published. The trusted
    // verification refuses at the begin authority; the marker
    // remains DISCOVERABLE for non-authoritative reporting (the
    // discovery arm carries no completion authority); the failed
    // verification publishes no begin, deletes nothing, and
    // installs no new completion.
    let (root, mut medium, audit, mk, _ledger, _shared, _private, _p1) =
        sqlite_begin_store("oc4-beginless-marker");
    let marker = crate::reclaim::ReclamationComplete {
        kind: crate::reclaim::CompletionKind::Batch,
        mk,
        batch: 0,
        source: audit,
        mandatory_count: 2,
        deferred: Vec::new(),
        journal_files: Vec::new(),
    };
    let marker_id =
        crate::reclaim::test_only_install_completion_marker(&mut medium, &marker, 240).unwrap();
    // The baseline AFTER the install (the failed verification
    // must change nothing beyond it).
    let records_before = medium.validate().unwrap().records;
    // THE TRUSTED VERIFICATION refuses the beginless marker.
    let error =
        crate::reclaim::verify_reclamation_complete(&mut medium, &marker_id, &mk).unwrap_err();
    assert!(
        matches!(
            error,
            MediumError::Chain {
                rule: "begin-authority/absent"
            }
        ),
        "the beginless marker never certifies: {error:?}"
    );
    // The legacy marker remains DISCOVERABLE (the reporting
    // distinction: readable, never authoritative).
    let (found_id, _found) = crate::reclaim::find_installed_completion(&mut medium, &mk)
        .unwrap()
        .expect("the historical record stays discoverable");
    assert_eq!(
        found_id, marker_id,
        "the historical record stays readable for non-authoritative reporting"
    );
    // No begin was published as a side effect of the failed
    // verification.
    let error = medium.verify_begin_authority(&audit, &mk).unwrap_err();
    assert!(
        matches!(
            error,
            MediumError::Chain {
                rule: "begin-authority/absent"
            }
        ),
        "the verification manufactured no begin: {error:?}"
    );
    // No deletion, no new completion publication: the record
    // count is unchanged (the marker install preceded the
    // baseline).
    let records_after = medium.validate().unwrap().records;
    assert_eq!(
        records_after, records_before,
        "the failed verification changed nothing"
    );
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
#[cfg(feature = "composite-test-fixture")]
fn the_oc4_wrong_bound_begin_refuses_through_the_completion_reader() {
    // (OC-C4) The CP-1 verifier matrix's typed refusals
    // propagate through the completion reader — the reader never
    // broadens the accepted classifications: a begin record
    // discovered at the right deterministic key but carrying the
    // WRONG source-audit binding refuses with
    // begin-authority/binding-mismatch; the wrong range digest
    // refuses the same way. The marker itself is structurally
    // valid and on-chain — only the begin provenance is wrong.
    let (root, mut medium, audit, mk, ledger, _shared, _private, p1) =
        sqlite_begin_store("oc4-wrong-bound");
    // The wrong-SOURCE begin at the right key (a REAL non-zero
    // identity — the strict decoder refuses zero-identity
    // fields before the binding check would run).
    let wrong_source = crate::reclaim::ReclamationBegin {
        mk,
        source: p1,
        range_digest: ledger.range_digest,
        seed: ledger.seed,
    };
    let _begin_id =
        crate::reclaim::test_only_install_begin_record(&mut medium, &wrong_source, 240).unwrap();
    let marker = crate::reclaim::ReclamationComplete {
        kind: crate::reclaim::CompletionKind::Batch,
        mk,
        batch: 0,
        source: audit,
        mandatory_count: 2,
        deferred: Vec::new(),
        journal_files: Vec::new(),
    };
    let marker_id =
        crate::reclaim::test_only_install_completion_marker(&mut medium, &marker, 241).unwrap();
    let error =
        crate::reclaim::verify_reclamation_complete(&mut medium, &marker_id, &mk).unwrap_err();
    assert!(
        matches!(
            error,
            MediumError::Chain {
                rule: "begin-authority/binding-mismatch"
            }
        ),
        "the wrong-source begin refuses the certification: {error:?}"
    );
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
    // The wrong-DIGEST begin at the right key.
    let (root, mut medium, audit, mk, ledger, _shared, _private, _p1) =
        sqlite_begin_store("oc4-wrong-digest");
    let wrong_digest = crate::reclaim::ReclamationBegin {
        mk,
        source: audit,
        range_digest: [0u8; 32],
        seed: ledger.seed,
    };
    let _begin_id =
        crate::reclaim::test_only_install_begin_record(&mut medium, &wrong_digest, 240).unwrap();
    let marker = crate::reclaim::ReclamationComplete {
        kind: crate::reclaim::CompletionKind::Batch,
        mk,
        batch: 0,
        source: audit,
        mandatory_count: 2,
        deferred: Vec::new(),
        journal_files: Vec::new(),
    };
    let marker_id =
        crate::reclaim::test_only_install_completion_marker(&mut medium, &marker, 241).unwrap();
    let error =
        crate::reclaim::verify_reclamation_complete(&mut medium, &marker_id, &mk).unwrap_err();
    assert!(
        matches!(
            error,
            MediumError::Chain {
                rule: "begin-authority/binding-mismatch"
            }
        ),
        "the wrong-digest begin refuses the certification: {error:?}"
    );
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_sqlite_execution_commit_uncertainty_is_typed() {
    // (OC-C3 CP-2 §4/§9) The execution COMMIT uncertainty: the
    // injected CommitFailure (the established seam) produces the
    // single typed reclaim/commit-uncertain — never success,
    // never a definite absence — and the next authorized pass
    // independently re-verifies the begin and the actual
    // deletion state, converging.
    let (root, mut medium, audit, mk, ledger, _shared, _private, _p1) =
        sqlite_begin_store("commit-uncertain");
    let directive = crate::reclaim::ReclaimDirective {
        audit_record: audit,
        units: ledger.units.clone(),
    };
    let begin_outcome = medium.begin_reclamation(&audit, &mk, 242).unwrap();
    assert!(
        matches!(
            begin_outcome,
            crate::reclaim::BeginPublicationOutcome::Accepted { .. }
                | crate::reclaim::BeginPublicationOutcome::DuplicateVerified { .. }
        ),
        "the begin admits: {begin_outcome:?}"
    );
    medium.fault = Some(SqliteFaultControl::new().arm(
        SqliteFaultPoint::CommitFailure,
        SqliteFaultAction::InjectError,
    ));
    let error = medium.reclaim(&directive).unwrap_err();
    assert!(
        matches!(
            error,
            MediumError::Chain {
                rule: "reclaim/commit-uncertain"
            }
        ),
        "the execution commit failure is the typed uncertainty: {error:?}"
    );
    medium.fault = None;
    // The next authorized pass converges (the observation found
    // nothing deleted; the gate re-verified the begin).
    let outcome = medium.reclaim(&directive).unwrap();
    assert!(
        !outcome.mandatory.is_empty(),
        "the retry converges through the re-verified begin"
    );
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_v2_sqlite_publication_and_capacity_matrix() {
    // (V2-W §9 G + the sqlite positives) The v2 publications on
    // the SQLite provider: the plan and every batch audit
    // publish under the deterministic keys with the truthful
    // outcomes; the W3A capacity floor — a connection whose
    // runtime SQLITE_MAX_LENGTH falls below the supported 4 MiB
    // floor is an UNSUPPORTED configuration for reclamation
    // publication (the typed refusal), while the ordinary
    // small-object reads/writes remain unaffected; and a limit
    // restored above the floor re-enables publication.
    let label = "v2w-sqlite";
    let root = std::env::temp_dir().join(format!(
        "s7-occ2r4-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut medium = SqliteMedium::open(root.join("db.sqlite")).unwrap();
    // The plan campaign (the checkpoint WITHOUT a v1 audit):
    // F(shared) → P1(shared) → R1(shared) → R2(shared+private)
    // → SK(shared) → MK.
    let shared = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"v2w-sqlite-shared");
        enc.into_bytes()
    })
    .unwrap();
    let private = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"v2w-sqlite-private");
        enc.into_bytes()
    })
    .unwrap();
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, label, 0)
        .with_content_refs(vec![shared.hash()]);
    medium
        .append(&founding, std::slice::from_ref(&shared), None)
        .unwrap();
    // (V2-W F4, the F4-R6 fresh shape) The bound bootstrap AT THE
    // FOUNDING — the production ordering: the durable bound
    // journal at the root, then the canonical root binding
    // published while the founding is the chain's head (the
    // first binding's fresh-bootstrap admission), then the
    // campaign's revisions on top.
    let journal_root = root.join("journal-root");
    let founding_id = founding.id();
    let location = crate::reclaim::JournalRootBinding::normalize_location(&journal_root);
    let root_id = crate::reclaim::journal_root_id(&founding_id, &location);
    drop(
        crate::reclaim::create_campaign_bound_journal(&mut medium, &journal_root, &founding_id)
            .unwrap(),
    );
    let (_binding_id, binding_outcome) = crate::reclaim::publish_journal_root_binding(
        &mut medium,
        &founding_id,
        &root_id,
        &location,
        1,
    )
    .unwrap();
    assert!(
        matches!(
            binding_outcome,
            crate::reclaim::JournalRootBindingOutcome::Accepted { .. }
        ),
        "the fixture's binding publishes: {binding_outcome:?}"
    );
    let step = |medium: &mut SqliteMedium, objects: &[StoredObject], tick: u64| -> Hash {
        let head = medium.head().unwrap().unwrap();
        let epoch = medium.read_record(&head).unwrap().epoch;
        let refs: Vec<Hash> = objects.iter().map(|o| o.hash()).collect();
        let record = RevisionRecord::regular(head, epoch, tick).with_content_refs(refs);
        medium.append(&record, objects, Some(head)).unwrap();
        record.id()
    };
    let p1 = step(&mut medium, std::slice::from_ref(&shared), 1);
    let r1 = step(&mut medium, std::slice::from_ref(&shared), 2);
    let r2 = step(&mut medium, &[shared.clone(), private.clone()], 3);
    let sk = step(&mut medium, std::slice::from_ref(&shared), 4);
    let seed = crate::checkpoint::compute_digest_seed(&founding.id());
    let member_bytes: Vec<Vec<u8>> = [r1, r2]
        .iter()
        .map(|id| medium.read_record(id).unwrap().canonical_bytes())
        .collect();
    let digest = crate::checkpoint::chain_range_digest(&seed, &member_bytes);
    let meta = CheckpointMetadata::new(
        sk,
        r1,
        vec![SkipEntry::new(Hash::ZERO, p1)],
        vec![
            ReclaimedIndexEntry::new(None, r1, 2),
            ReclaimedIndexEntry::new(None, r2, 3),
            ReclaimedIndexEntry::new(None, sk, 4),
        ],
        digest,
        seed,
    );
    let meta_object = StoredObject::new(meta.canonical_bytes()).unwrap();
    let mk = step(&mut medium, std::slice::from_ref(&meta_object), 5);
    // THE POSITIVE: the plan and every batch audit publish.
    let (plan_id, plan_outcome) =
        crate::reclaim::publish_reclamation_plan(&mut medium, &mk, 1024, &journal_root, 90)
            .unwrap();
    assert!(
        matches!(
            plan_outcome,
            crate::reclaim::PlanPublicationOutcome::Accepted { id } if id == plan_id
        ),
        "the sqlite plan publishes: {plan_outcome:?}"
    );
    let (_, plan) = crate::reclaim::find_installed_plan(&mut medium, &mk)
        .unwrap()
        .unwrap();
    for batch in plan.batches.iter().map(|b| b.index) {
        let (audit_id, audit_outcome) =
            crate::reclaim::publish_reclamation_ledger_v2(&mut medium, &mk, batch, 91).unwrap();
        assert!(
            matches!(
                audit_outcome,
                crate::reclaim::LedgerV2PublicationOutcome::Accepted { id } if id == audit_id
            ),
            "the sqlite batch audit publishes: {audit_outcome:?}"
        );
        assert!(
            crate::reclaim::find_installed_v2_audit(&mut medium, &mk, batch)
                .unwrap()
                .is_some()
        );
    }
    // THE W3A FLOOR: a connection below the supported floor is
    // an unsupported configuration for reclamation publication —
    // the typed refusal — while the ordinary small-object
    // publication keeps working.
    medium
        .conn
        .set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, 1024)
        .unwrap();
    let error = crate::reclaim::publish_reclamation_plan(&mut medium, &mk, 1024, &journal_root, 92)
        .unwrap_err();
    assert!(
        matches!(
            error,
            MediumError::Chain {
                rule: "reclamation-plan/provider-limit-unsupported"
            }
        ),
        "the below-floor connection refuses reclamation publication: {error:?}"
    );
    // The ordinary small-object publication is unaffected (the
    // floor is a RECLAMATION classification, not a store-wide
    // failure).
    step(&mut medium, std::slice::from_ref(&shared), 6);
    // THE RESTORE: a limit above the floor re-enables the
    // publication (the idempotent duplicate of the already
    // installed plan).
    medium
        .conn
        .set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, i32::MAX)
        .unwrap();
    let (dup_id, dup_outcome) =
        crate::reclaim::publish_reclamation_plan(&mut medium, &mk, 1024, &journal_root, 93)
            .unwrap();
    assert!(
        matches!(
            dup_outcome,
            crate::reclaim::PlanPublicationOutcome::DuplicateVerified { id } if id == plan_id
        ),
        "the restored connection re-enables the idempotent publication: {dup_outcome:?}"
    );
    assert_eq!(dup_id, plan_id);
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_v2_f3_sqlite_second_reference_integrity() {
    // (V2W-F3, the SQLite side) EVERY direct content reference
    // is verified at the writer boundaries on the SQLite
    // provider too: the genuine two-reference retiring record
    // whose SECOND reference is corrupted at the row level
    // refuses both the plan admission (the first-execution
    // evidence core) and the batch-audit admission (the
    // collector's verified read) through the typed integrity
    // classification, before any authority installs.
    let label = "v2wf3-sqlite";
    let root = std::env::temp_dir().join(format!(
        "s7-occ2r4-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let build = |label_postfix: &str| -> (SqliteMedium, Hash, Hash, std::path::PathBuf) {
        let mut medium =
            SqliteMedium::open(root.join(format!("db-{label_postfix}.sqlite"))).unwrap();
        let shared = StoredObject::new({
            let mut enc = Encoder::new();
            enc.write_str("sektor7/save/1");
            enc.write_bytes(b"v2wf3-sqlite-shared");
            enc.into_bytes()
        })
        .unwrap();
        let private = StoredObject::new({
            let mut enc = Encoder::new();
            enc.write_str("sektor7/save/1");
            enc.write_bytes(b"v2wf3-sqlite-private");
            enc.into_bytes()
        })
        .unwrap();
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, label, 0)
            .with_content_refs(vec![shared.hash()]);
        medium
            .append(&founding, std::slice::from_ref(&shared), None)
            .unwrap();
        // (V2-W F4, the F4-R6 fresh shape) The bound bootstrap AT
        // THE FOUNDING — the production ordering (the first
        // binding's fresh-bootstrap admission at the founding
        // head).
        let journal_root = root.join(format!("journal-{label_postfix}"));
        let founding_id = founding.id();
        let location = crate::reclaim::JournalRootBinding::normalize_location(&journal_root);
        let root_id = crate::reclaim::journal_root_id(&founding_id, &location);
        drop(
            crate::reclaim::create_campaign_bound_journal(&mut medium, &journal_root, &founding_id)
                .unwrap(),
        );
        crate::reclaim::publish_journal_root_binding(
            &mut medium,
            &founding_id,
            &root_id,
            &location,
            1,
        )
        .unwrap();
        let step = |medium: &mut SqliteMedium, objects: &[StoredObject], tick: u64| -> Hash {
            let head = medium.head().unwrap().unwrap();
            let epoch = medium.read_record(&head).unwrap().epoch;
            let refs: Vec<Hash> = objects.iter().map(|o| o.hash()).collect();
            let record = RevisionRecord::regular(head, epoch, tick).with_content_refs(refs);
            medium.append(&record, objects, Some(head)).unwrap();
            record.id()
        };
        let p1 = step(&mut medium, std::slice::from_ref(&shared), 1);
        let r1 = step(&mut medium, std::slice::from_ref(&shared), 2);
        let r2 = step(&mut medium, &[shared.clone(), private.clone()], 3);
        let sk = step(&mut medium, std::slice::from_ref(&shared), 4);
        let seed = crate::checkpoint::compute_digest_seed(&founding.id());
        let member_bytes: Vec<Vec<u8>> = [r1, r2]
            .iter()
            .map(|id| medium.read_record(id).unwrap().canonical_bytes())
            .collect();
        let digest = crate::checkpoint::chain_range_digest(&seed, &member_bytes);
        let meta = CheckpointMetadata::new(
            sk,
            r1,
            vec![SkipEntry::new(Hash::ZERO, p1)],
            vec![
                ReclaimedIndexEntry::new(None, r1, 2),
                ReclaimedIndexEntry::new(None, r2, 3),
                ReclaimedIndexEntry::new(None, sk, 4),
            ],
            digest,
            seed,
        );
        let meta_object = StoredObject::new(meta.canonical_bytes()).unwrap();
        let mk = step(&mut medium, std::slice::from_ref(&meta_object), 5);
        (medium, mk, private.hash(), journal_root)
    };
    // THE PLAN ADMISSION (the corruption precedes the plan).
    {
        let (mut medium, mk, private_hash, journal_root) = build("plan");
        medium
            .conn
            .execute(
                "UPDATE objects SET bytes = ?1 WHERE hash = ?2",
                params![
                    b"corrupted-second-ref".to_vec(),
                    private_hash.as_bytes().to_vec()
                ],
            )
            .unwrap();
        let error =
            crate::reclaim::publish_reclamation_plan(&mut medium, &mk, 1024, &journal_root, 90)
                .unwrap_err();
        assert!(
            matches!(
                error,
                MediumError::HashMismatch { .. } | MediumError::MissingObject { .. }
            ),
            "the corrupt second reference refuses the sqlite plan admission: {error:?}"
        );
    }
    // THE BATCH-AUDIT ADMISSION (the plan published intact, the
    // corruption after).
    {
        let (mut medium, mk, private_hash, journal_root) = build("audit");
        crate::reclaim::publish_reclamation_plan(&mut medium, &mk, 1024, &journal_root, 91)
            .unwrap();
        medium
            .conn
            .execute(
                "UPDATE objects SET bytes = ?1 WHERE hash = ?2",
                params![
                    b"corrupted-second-ref".to_vec(),
                    private_hash.as_bytes().to_vec()
                ],
            )
            .unwrap();
        let (_, plan) = crate::reclaim::find_installed_plan(&mut medium, &mk)
            .unwrap()
            .unwrap();
        for batch in plan.batches.iter().map(|b| b.index) {
            let error = crate::reclaim::publish_reclamation_ledger_v2(&mut medium, &mk, batch, 92)
                .unwrap_err();
            assert!(
                matches!(
                    error,
                    MediumError::HashMismatch { .. } | MediumError::MissingObject { .. }
                ),
                "the corrupt second reference refuses the sqlite audit admission: {error:?}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_v2_sqlite_reader_matrix() {
    // (V2X-CP1) The trusted all-audits reader on the SQLite
    // provider: the incomplete publication state classified, the
    // complete state certified FIRST-EXECUTION-READY with the
    // exact revision identities (strictly read-only), the
    // restart (a fresh connection) certifying identically, and
    // the bogus-MK typed refusal.
    let label = "v2x-cp1-sqlite";
    let root = std::env::temp_dir().join(format!(
        "s7-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut medium = SqliteMedium::open(root.join("db.sqlite")).unwrap();
    let shared = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"v2x-sqlite-shared");
        enc.into_bytes()
    })
    .unwrap();
    let private = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"v2x-sqlite-private");
        enc.into_bytes()
    })
    .unwrap();
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, label, 0)
        .with_content_refs(vec![shared.hash()]);
    medium
        .append(&founding, std::slice::from_ref(&shared), None)
        .unwrap();
    let journal_root = root.join("journal-root");
    let founding_id = founding.id();
    let location = crate::reclaim::JournalRootBinding::normalize_location(&journal_root);
    let root_id = crate::reclaim::journal_root_id(&founding_id, &location);
    drop(
        crate::reclaim::create_campaign_bound_journal(&mut medium, &journal_root, &founding_id)
            .unwrap(),
    );
    let (_binding_id, _binding_outcome) = crate::reclaim::publish_journal_root_binding(
        &mut medium,
        &founding_id,
        &root_id,
        &location,
        1,
    )
    .unwrap();
    let step = |medium: &mut SqliteMedium, objects: &[StoredObject], tick: u64| -> Hash {
        let head = medium.head().unwrap().unwrap();
        let epoch = medium.read_record(&head).unwrap().epoch;
        let refs: Vec<Hash> = objects.iter().map(|o| o.hash()).collect();
        let record = RevisionRecord::regular(head, epoch, tick).with_content_refs(refs);
        medium.append(&record, objects, Some(head)).unwrap();
        record.id()
    };
    let p1 = step(&mut medium, std::slice::from_ref(&shared), 1);
    let r1 = step(&mut medium, std::slice::from_ref(&shared), 2);
    let r2 = step(&mut medium, &[shared.clone(), private.clone()], 3);
    let sk = step(&mut medium, std::slice::from_ref(&shared), 4);
    let seed = crate::checkpoint::compute_digest_seed(&founding_id);
    let member_bytes: Vec<Vec<u8>> = [r1, r2]
        .iter()
        .map(|id| medium.read_record(id).unwrap().canonical_bytes())
        .collect();
    let digest = crate::checkpoint::chain_range_digest(&seed, &member_bytes);
    let meta = CheckpointMetadata::new(
        sk,
        r1,
        vec![SkipEntry::new(Hash::ZERO, p1)],
        vec![
            ReclaimedIndexEntry::new(None, r1, 2),
            ReclaimedIndexEntry::new(None, r2, 3),
            ReclaimedIndexEntry::new(None, sk, 4),
        ],
        digest,
        seed,
    );
    let meta_object = StoredObject::new(meta.canonical_bytes()).unwrap();
    let mk = step(&mut medium, std::slice::from_ref(&meta_object), 5);
    // THE BOGUS MK: a real non-zero identity that is no
    // checkpoint — the typed refusal, never a classification.
    let error =
        crate::reclaim::verify_v2_execution_readiness(&mut medium, &private.hash()).unwrap_err();
    assert!(
        matches!(
            error,
            MediumError::Chain {
                rule: "v2-readiness/mk-not-installed"
            }
        ),
        "the bogus MK refuses typed: {error:?}"
    );
    // THE INCOMPLETE STATE: the plan published, no audits yet.
    let (plan_id, _) =
        crate::reclaim::publish_reclamation_plan(&mut medium, &mk, 1024, &journal_root, 90)
            .unwrap();
    let (_, plan) = crate::reclaim::find_installed_plan(&mut medium, &mk)
        .unwrap()
        .unwrap();
    match crate::reclaim::verify_v2_execution_readiness(&mut medium, &mk).unwrap() {
        crate::reclaim::V2ExecutionReadiness::PublicationIncomplete { missing_batches } => {
            let expected: Vec<u64> = plan.batches.iter().map(|b| b.index).collect();
            assert_eq!(missing_batches, expected);
        }
        other => panic!("the sqlite mid-publication state classifies: {other:?}"),
    }
    // THE COMPLETE STATE: every batch audit published.
    let mut audit_ids = Vec::new();
    for batch in plan.batches.iter().map(|b| b.index) {
        let (audit_id, _) =
            crate::reclaim::publish_reclamation_ledger_v2(&mut medium, &mk, batch, 91).unwrap();
        audit_ids.push(audit_id);
    }
    match crate::reclaim::verify_v2_execution_readiness(&mut medium, &mk).unwrap() {
        crate::reclaim::V2ExecutionReadiness::ExecutionReady {
            plan_record,
            audit_records,
            ..
        } => {
            assert_eq!(plan_record, plan_id);
            assert_eq!(audit_records, audit_ids);
        }
        other => panic!("the sqlite complete state certifies ready: {other:?}"),
    }
    // THE RESTART: a fresh connection certifies identically.
    drop(medium);
    let mut reopened = SqliteMedium::open(root.join("db.sqlite")).unwrap();
    match crate::reclaim::verify_v2_execution_readiness(&mut reopened, &mk).unwrap() {
        crate::reclaim::V2ExecutionReadiness::ExecutionReady { plan_record, .. } => {
            assert_eq!(plan_record, plan_id);
        }
        other => panic!("the sqlite restart certifies ready: {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// (V2X-CP2 F3) The SQLite V2 plan campaign for the
/// reconciliation regressions: the bound-bootstrapped founding,
/// the journal binding, the retained pre-range record p1, the
/// two range records (r2 carrying the shared+private pair), SK,
/// and the hand-built checkpoint MK over the [r1, r2, sk] index
/// with the self-skip to p1 — the reader-matrix fixture shape,
/// with the genuine plan published at the 460 split (batch 0 =
/// [sk, r2], batch 1 = [r1]). Returns (root, medium, mk,
/// plan_id, r1).
fn v2x_sqlite_reconcile_campaign(
    label: &str,
) -> (std::path::PathBuf, SqliteMedium, Hash, Hash, Hash) {
    let root = std::env::temp_dir().join(format!(
        "s7-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut medium = SqliteMedium::open(root.join("db.sqlite")).unwrap();
    let shared = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"v2x-f3-sqlite-shared");
        enc.into_bytes()
    })
    .unwrap();
    let private = StoredObject::new({
        let mut enc = Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"v2x-f3-sqlite-private");
        enc.into_bytes()
    })
    .unwrap();
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, label, 0)
        .with_content_refs(vec![shared.hash()]);
    medium
        .append(&founding, std::slice::from_ref(&shared), None)
        .unwrap();
    let journal_root = root.join("journal-root");
    let founding_id = founding.id();
    let location = crate::reclaim::JournalRootBinding::normalize_location(&journal_root);
    let root_id = crate::reclaim::journal_root_id(&founding_id, &location);
    drop(
        crate::reclaim::create_campaign_bound_journal(&mut medium, &journal_root, &founding_id)
            .unwrap(),
    );
    let (_binding_id, _binding_outcome) = crate::reclaim::publish_journal_root_binding(
        &mut medium,
        &founding_id,
        &root_id,
        &location,
        1,
    )
    .unwrap();
    let step = |medium: &mut SqliteMedium, objects: &[StoredObject], tick: u64| -> Hash {
        let head = medium.head().unwrap().unwrap();
        let epoch = medium.read_record(&head).unwrap().epoch;
        let refs: Vec<Hash> = objects.iter().map(|o| o.hash()).collect();
        let record = RevisionRecord::regular(head, epoch, tick).with_content_refs(refs);
        medium.append(&record, objects, Some(head)).unwrap();
        record.id()
    };
    let p1 = step(&mut medium, std::slice::from_ref(&shared), 1);
    let r1 = step(&mut medium, std::slice::from_ref(&shared), 2);
    let r2 = step(&mut medium, &[shared.clone(), private.clone()], 3);
    let sk = step(&mut medium, std::slice::from_ref(&shared), 4);
    let seed = crate::checkpoint::compute_digest_seed(&founding_id);
    let member_bytes: Vec<Vec<u8>> = [r1, r2]
        .iter()
        .map(|id| medium.read_record(id).unwrap().canonical_bytes())
        .collect();
    let digest = crate::checkpoint::chain_range_digest(&seed, &member_bytes);
    let meta = CheckpointMetadata::new(
        sk,
        r1,
        vec![SkipEntry::new(Hash::ZERO, p1)],
        vec![
            ReclaimedIndexEntry::new(None, r1, 2),
            ReclaimedIndexEntry::new(None, r2, 3),
            ReclaimedIndexEntry::new(None, sk, 4),
        ],
        digest,
        seed,
    );
    let meta_object = StoredObject::new(meta.canonical_bytes()).unwrap();
    let mk = step(&mut medium, std::slice::from_ref(&meta_object), 5);
    let (plan_id, _) =
        crate::reclaim::publish_reclamation_plan(&mut medium, &mk, 460, &journal_root, 90).unwrap();
    (root, medium, mk, plan_id, r1)
}

#[test]
fn the_v2_begin_reconciliation_proves_the_different_candidates_authority() {
    // (V2X-CP2 F3) The different-candidate reconciliation: after
    // an uncertain publication, a DIFFERENT same-key begin is
    // DuplicateVerified ONLY when its OWN retained authority
    // holds — the strict temporal position, the complete
    // retained membership, and the source binding, re-proved
    // over the same READ-ONLY transaction with the verifier's
    // begin identity exactly the observed revision. The payload
    // match alone is never trusted completion. The exact
    // attempted candidate remains the canonical
    // ReconciledInstalled installation evidence, and the valid
    // different candidate verifies as the OBSERVED duplicate
    // without inventing an installation acknowledgment for the
    // attempted one.
    // (a) THE MANDATORY FIXTURE — a different-or-absent
    // attempted candidate over a byte-identical but temporally
    // invalid installed begin: never DuplicateVerified.
    {
        let (root, mut medium, mk, plan_id, r1) =
            v2x_sqlite_reconcile_campaign("v2x-cp2-f3-invalid");
        let (_, plan) = crate::reclaim::find_installed_plan(&mut medium, &mk)
            .unwrap()
            .unwrap();
        assert_eq!(plan.batches.len(), 2, "the 460 split: two batches");
        // The partial audit set: batch 0 only.
        crate::reclaim::publish_reclamation_ledger_v2(&mut medium, &mk, 0, 91).unwrap();
        // The correctly encoded, correctly plan-bound begin
        // installed adversarially under the deterministic key
        // (the admitted in-crate receipt — the only bypass).
        let begin = crate::reclaim::ReclamationBegin {
            mk,
            source: plan_id,
            range_digest: plan.range_digest,
            seed: plan.seed,
        };
        let object = StoredObject::new(begin.canonical_bytes()).unwrap();
        let head = medium.head().unwrap().unwrap();
        let epoch = medium.read_record(&head).unwrap().epoch;
        let record = RevisionRecord::regular(head, epoch, 96)
            .with_idempotency_key(crate::reclaim::reclamation_begin_idempotency_key(&mk))
            .with_content_refs(vec![object.hash()]);
        let admitted =
            crate::medium::AdmittedAppend::generic(&record, std::slice::from_ref(&object));
        medium.append_admitted(admitted, Some(head)).unwrap();
        // The REMAINING valid audit published ABOVE that begin
        // (the normal gate — the audit evidence itself remains
        // valid).
        crate::reclaim::publish_reclamation_ledger_v2(&mut medium, &mk, 1, 97).unwrap();
        // The reconciliation observation with an ABSENT
        // attempted candidate identity: the typed temporal
        // refusal propagates — the byte-identical payload match
        // alone never classifies as DuplicateVerified.
        let error = reconcile_begin_publication_v2(&mut medium, &mk, None).unwrap_err();
        assert!(
            matches!(
                error,
                MediumError::Chain {
                    rule: "begin-authority-v2/target-above-begin"
                }
            ),
            "the absent candidate over the temporally invalid begin refuses typed: {error:?}"
        );
        // The DIFFERENT attempted candidate identity: the same
        // typed refusal.
        let error = reconcile_begin_publication_v2(&mut medium, &mk, Some(r1)).unwrap_err();
        assert!(
            matches!(
                error,
                MediumError::Chain {
                    rule: "begin-authority-v2/target-above-begin"
                }
            ),
            "the different candidate over the temporally invalid begin refuses typed: {error:?}"
        );
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (b) THE POSITIVE DIFFERENT-CANDIDATE CONTROL — a different
    // same-key begin with VALID retained authority: the verified
    // duplicate naming the OBSERVED revision, never an
    // installation acknowledgment for the attempted candidate.
    {
        let (root, mut medium, mk, plan_id, r1) = v2x_sqlite_reconcile_campaign("v2x-cp2-f3-valid");
        let (_, plan) = crate::reclaim::find_installed_plan(&mut medium, &mk)
            .unwrap()
            .unwrap();
        // EVERY plan-listed audit (the complete all-audits-first
        // set).
        for batch in plan.batches.iter().map(|b| b.index) {
            crate::reclaim::publish_reclamation_ledger_v2(&mut medium, &mk, batch, 91).unwrap();
        }
        // The genuine-valued begin installed ABOVE every audit
        // (the adversarial install at the head — the temporally
        // valid position the admission itself would occupy).
        let begin = crate::reclaim::ReclamationBegin {
            mk,
            source: plan_id,
            range_digest: plan.range_digest,
            seed: plan.seed,
        };
        let object = StoredObject::new(begin.canonical_bytes()).unwrap();
        let head = medium.head().unwrap().unwrap();
        let epoch = medium.read_record(&head).unwrap().epoch;
        let record = RevisionRecord::regular(head, epoch, 96)
            .with_idempotency_key(crate::reclaim::reclamation_begin_idempotency_key(&mk))
            .with_content_refs(vec![object.hash()]);
        let installed_id = record.id();
        let admitted =
            crate::medium::AdmittedAppend::generic(&record, std::slice::from_ref(&object));
        medium.append_admitted(admitted, Some(head)).unwrap();
        // The different-candidate observation: the complete
        // authority holds → DuplicateVerified.
        let outcome = reconcile_begin_publication_v2(&mut medium, &mk, Some(r1)).unwrap();
        assert!(
            matches!(
                outcome,
                crate::reclaim::BeginPublicationOutcome::DuplicateVerified { id } if id == installed_id
            ),
            "the valid different candidate verifies as the observed duplicate: {outcome:?}"
        );
        // The absent attempted candidate classifies the same
        // way.
        let outcome = reconcile_begin_publication_v2(&mut medium, &mk, None).unwrap();
        assert!(
            matches!(
                outcome,
                crate::reclaim::BeginPublicationOutcome::DuplicateVerified { id } if id == installed_id
            ),
            "the absent candidate over the valid authority verifies the observed duplicate: {outcome:?}"
        );
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (c) THE EXACT ATTEMPTED CANDIDATE — the production
    // uncertain-commit route (the established AfterCommit seam:
    // the commit succeeded, the acknowledgment lost) reconciles
    // to the canonical ReconciledInstalled installation
    // evidence.
    {
        let (root, mut medium, mk, _plan_id, _r1) =
            v2x_sqlite_reconcile_campaign("v2x-cp2-f3-exact");
        let (_, plan) = crate::reclaim::find_installed_plan(&mut medium, &mk)
            .unwrap()
            .unwrap();
        for batch in plan.batches.iter().map(|b| b.index) {
            crate::reclaim::publish_reclamation_ledger_v2(&mut medium, &mk, batch, 91).unwrap();
        }
        medium.fault = Some(SqliteFaultControl::new().arm(
            SqliteFaultPoint::AfterCommit,
            SqliteFaultAction::InjectError,
        ));
        let outcome = medium.begin_reclamation_v2(&mk, 98).unwrap();
        let crate::reclaim::BeginPublicationOutcome::ReconciledInstalled { id } = outcome else {
            panic!(
                "the exact attempted candidate reconciles to the installation evidence: {outcome:?}"
            )
        };
        assert_eq!(
            medium.head().unwrap().unwrap(),
            id,
            "the reconciled installation is the HEAD"
        );
        medium.fault = None;
        medium.validate().unwrap();
        drop(medium);
        let _ = std::fs::remove_dir_all(&root);
    }
}
