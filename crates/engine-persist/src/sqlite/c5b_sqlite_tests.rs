// ============================================================== the C5b tests
//
// (V2X-CP3-B-P2A-C5b) The provider-coordinated Range-terminal
// observation on SqliteMedium: ONE consistent read transaction whose
// snapshot binds at its first statement — the head read and the
// whole walk inside the single snapshot. The genuine campaign is
// direct-inserted (the completion family's publication fence forbids
// the generic append for the /2 and /3 markers, and no V2 publisher
// exists yet).
use super::*;
use crate::ancestry_accounting::StorageRole;
use crate::batch_completion::{
    BatchCompletionContext, BatchCompletionMarkerV2, ClassificationEntry, EntryClassification,
    commit_classification, reclamation_complete_v2_idempotency_key,
};
use crate::checkpoint::{CheckpointMetadata, ReclaimedIndexEntry, checkpoint_mk_idempotency_key};
use crate::hash::hash_object;
use crate::range_completion::{
    RangeCompletionContext, RangeCompletionMarkerV3, RangeCoverageObservation,
    commit_journal_obligations, commit_range_coverage,
    reclamation_complete_v3_range_idempotency_key,
};
use crate::range_installation::{
    CoordinatedObservation, NotObservedTerminus, RangeInstallationLimits,
    RangeInstallationObservation,
};
use crate::reclaim::{
    PlanBatch, ReclaimPlan, ReclaimUnit, ReclamationBegin, ReclamationLedgerV2,
    reclamation_begin_idempotency_key, reclamation_ledger_v2_idempotency_key,
    reclamation_plan_idempotency_key,
};

/// The direct-inserting chain builder over the medium's own
/// connection: each keyed evidence record + its payload object
/// inserted through the store's own one-shot helpers inside a
/// short transaction, the meta head updated at the end.
struct SqliteDirectBuilder<'a> {
    conn: &'a mut rusqlite::Connection,
    head: Hash,
    head_epoch: u64,
    tick: u64,
}

impl SqliteDirectBuilder<'_> {
    fn new(conn: &mut rusqlite::Connection, seed: Hash, epoch: u64) -> SqliteDirectBuilder<'_> {
        SqliteDirectBuilder {
            conn,
            head: seed,
            head_epoch: epoch,
            tick: 0,
        }
    }

    fn write(&mut self, key: Vec<u8>, payload: Vec<u8>) -> Hash {
        let tx = self.conn.transaction().unwrap();
        let payload_object = StoredObject::new(payload).unwrap();
        insert_object_once(&tx, &None, &payload_object).unwrap();
        self.tick += 1;
        let record = RevisionRecord::regular(self.head, self.head_epoch, self.tick)
            .with_idempotency_key(key)
            .with_content_refs(vec![payload_object.hash()]);
        let id = record.id();
        insert_record_once(&tx, &record, &record.canonical_bytes(), &id).unwrap();
        tx.commit().unwrap();
        self.head = id;
        self.head_epoch += 1;
        id
    }

    fn set_head(&self, id: Hash) {
        self.conn
            .execute(
                "UPDATE meta SET value = ?1 WHERE key = 'head'",
                params![id.as_bytes()],
            )
            .unwrap();
    }
}

pub(super) struct Campaign {
    pub(super) head: Hash,
    pub(super) head_epoch: u64,
    pub(super) mk: Hash,
    pub(super) expected: RangeCompletionContext,
}

pub(super) fn temp_db(label: &str) -> (std::path::PathBuf, SqliteMedium) {
    let path = std::env::temp_dir().join(format!(
        "s7-c5b-{label}-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let medium = SqliteMedium::open(&path).unwrap();
    (path, medium)
}

pub(super) fn genuine_campaign(medium: &mut SqliteMedium, with_terminal: bool) -> Campaign {
    let members = [
        hash_object(b"c5b-sq-sk"),
        hash_object(b"c5b-sq-m1"),
        hash_object(b"c5b-sq-m2"),
        hash_object(b"c5b-sq-m3"),
        hash_object(b"c5b-sq-m4"),
    ];
    let range_digest = [7u8; 32];
    let seed = [9u8; 32];
    let index = vec![
        ReclaimedIndexEntry::new(None, members[0], 60),
        ReclaimedIndexEntry::new(Some(b"key-m1".to_vec()), members[1], 50),
        ReclaimedIndexEntry::new(None, members[2], 40),
        ReclaimedIndexEntry::new(Some(b"key-m3".to_vec()), members[3], 30),
        ReclaimedIndexEntry::new(None, members[4], 20),
    ];
    let metadata = CheckpointMetadata::new(
        members[0],
        hash_object(b"c5b-sq-sk-parent"),
        vec![],
        index,
        range_digest,
        seed,
    );
    let conn = &mut medium.conn;
    let mut builder = SqliteDirectBuilder::new(conn, members[0], 60);
    let mk = builder.write(
        checkpoint_mk_idempotency_key(&members[0]),
        metadata.canonical_bytes(),
    );
    let plan = ReclaimPlan {
        mk,
        sk: members[0],
        range_digest,
        seed,
        ledger_byte_limit: 2048,
        batches: vec![
            PlanBatch {
                index: 0,
                records: vec![members[0], members[1]],
            },
            PlanBatch {
                index: 1,
                records: vec![members[2], members[3], members[4]],
            },
        ],
        journal_files: vec!["journal.t1.0".to_owned(), "journal.t1.2".to_owned()],
    };
    let plan_record = builder.write(
        reclamation_plan_idempotency_key(&mk),
        plan.canonical_bytes(),
    );
    let audit0 = ReclamationLedgerV2 {
        mk,
        sk: members[0],
        batch: 0,
        range_digest,
        seed,
        records: 2,
        reclaimed_keys: 1,
        units: vec![
            ReclaimUnit {
                record: members[0],
                objects: vec![],
            },
            ReclaimUnit {
                record: members[1],
                objects: vec![hash_object(b"c5b-sq-o1")],
            },
        ],
    };
    let audit0_record = builder.write(
        reclamation_ledger_v2_idempotency_key(&mk, 0),
        audit0.canonical_bytes(),
    );
    let audit1 = ReclamationLedgerV2 {
        mk,
        sk: members[0],
        batch: 1,
        range_digest,
        seed,
        records: 3,
        reclaimed_keys: 1,
        units: vec![
            ReclaimUnit {
                record: members[2],
                objects: vec![],
            },
            ReclaimUnit {
                record: members[3],
                objects: vec![],
            },
            ReclaimUnit {
                record: members[4],
                objects: vec![],
            },
        ],
    };
    let audit1_record = builder.write(
        reclamation_ledger_v2_idempotency_key(&mk, 1),
        audit1.canonical_bytes(),
    );
    let begin = ReclamationBegin {
        mk,
        source: plan_record,
        range_digest,
        seed,
    };
    let begin_record = builder.write(
        reclamation_begin_idempotency_key(&mk),
        begin.canonical_bytes(),
    );
    let mut marker_records = Vec::new();
    let mut observations = Vec::new();
    for (batch, entries, audit) in [
        (
            0u64,
            vec![
                (
                    members[0],
                    StorageRole::Record,
                    EntryClassification::Mandatory,
                ),
                (
                    members[1],
                    StorageRole::Record,
                    EntryClassification::Mandatory,
                ),
                (
                    hash_object(b"c5b-sq-o1"),
                    StorageRole::Object,
                    EntryClassification::Deferred,
                ),
            ],
            audit0_record,
        ),
        (
            1u64,
            vec![
                (
                    members[2],
                    StorageRole::Record,
                    EntryClassification::Mandatory,
                ),
                (
                    members[3],
                    StorageRole::Record,
                    EntryClassification::Mandatory,
                ),
                (
                    members[4],
                    StorageRole::Record,
                    EntryClassification::Deferred,
                ),
            ],
            audit1_record,
        ),
    ] {
        let context = BatchCompletionContext {
            mk,
            plan: plan_record,
            batch_index: batch,
            audit,
            begin: begin_record,
        };
        let entries: Vec<ClassificationEntry> = entries
            .into_iter()
            .map(|(identity, role, classification)| ClassificationEntry {
                identity,
                role,
                classification,
                conditional_on_prior_batch: false,
            })
            .collect();
        let commitment = commit_classification(&context, entries).unwrap();
        let marker = BatchCompletionMarkerV2 {
            mk,
            plan: plan_record,
            batch_index: batch,
            audit,
            begin: begin_record,
            commitment: commitment.digest,
            counters: commitment.counters,
        };
        let record = builder.write(
            reclamation_complete_v2_idempotency_key(&mk, batch),
            marker.canonical_bytes(),
        );
        marker_records.push(record);
        observations.push(RangeCoverageObservation {
            batch_index: batch,
            marker_record: record,
            counters: commitment.counters,
        });
    }
    let expected = RangeCompletionContext {
        mk,
        plan: plan_record,
        begin: begin_record,
    };
    let head = if with_terminal {
        let coverage = commit_range_coverage(&expected, &observations).unwrap();
        let journal = commit_journal_obligations(&expected, &plan.journal_files).unwrap();
        let marker = RangeCompletionMarkerV3 {
            mk,
            plan: plan_record,
            begin: begin_record,
            coverage: coverage.digest,
            journal,
            batch_count: observations.len() as u64,
            journal_count: plan.journal_files.len() as u64,
            counters: coverage.aggregate,
        };
        builder.write(
            reclamation_complete_v3_range_idempotency_key(&mk),
            marker.canonical_bytes(),
        )
    } else {
        builder.head
    };
    let head_epoch = builder.head_epoch;
    builder.set_head(head);
    Campaign {
        head,
        head_epoch,
        mk,
        expected,
    }
}

#[test]
fn the_coordinated_positive_on_sqlite() {
    let (path, mut medium) = temp_db("positive");
    let campaign = genuine_campaign(&mut medium, true);
    let coordinated = medium
        .observe_range_terminal_under_coordination(
            &campaign.expected,
            &RangeInstallationLimits::default(),
        )
        .unwrap();
    let CoordinatedObservation {
        coordinated_head,
        observation,
        ..
    } = coordinated;
    assert_eq!(coordinated_head, campaign.head);
    assert!(matches!(
        observation,
        RangeInstallationObservation::ObservedOnAncestry { .. }
    ));
    drop(medium);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_unfounded_sqlite_store_refuses_typed() {
    let (path, mut medium) = temp_db("unfounded");
    let expected = RangeCompletionContext {
        mk: hash_object(b"c5b-sq-any-mk"),
        plan: hash_object(b"c5b-sq-any-plan"),
        begin: hash_object(b"c5b-sq-any-begin"),
    };
    assert!(matches!(
        medium.observe_range_terminal_under_coordination(
            &expected,
            &RangeInstallationLimits::default()
        ),
        Err(MediumError::Chain { rule: "unfounded" })
    ));
    drop(medium);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_absent_terminal_on_sqlite_classifies() {
    let (path, mut medium) = temp_db("absent");
    let campaign = genuine_campaign(&mut medium, false);
    let coordinated = medium
        .observe_range_terminal_under_coordination(
            &campaign.expected,
            &RangeInstallationLimits::default(),
        )
        .unwrap();
    assert!(matches!(
        coordinated.observation,
        RangeInstallationObservation::NotObservedOnAncestry {
            terminus: NotObservedTerminus::ExpectedCoveringCheckpoint { mk },
            ..
        } if mk == campaign.mk
    ));
    drop(medium);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_sqlite_snapshot_binds_before_a_later_commit() {
    // The snapshot-consistency adversary: the observation
    // transaction's first statement binds the snapshot; a
    // LATER commit by a second connection is wholly INVISIBLE
    // to the walk inside that transaction — and wholly visible
    // to the next fresh observation.
    let (path, mut medium) = temp_db("snapshot");
    let campaign = genuine_campaign(&mut medium, true);
    // The held observation transaction: the FIRST statement
    // (the meta head read) binds the snapshot.
    let tx = medium.conn.transaction().unwrap();
    let bound_head = read_meta_head(&tx).unwrap().unwrap();
    assert_eq!(bound_head, campaign.head);
    // A second RAW connection commits a later publication (the
    // head moves on disk) — raw, because SqliteMedium::open's
    // cold walk would traverse into the campaign's virtual seed
    // below the MK and refuse.
    let mut second = rusqlite::Connection::open(&path).unwrap();
    let later_payload = {
        let mut enc = engine_core::Encoder::new();
        enc.write_str("sektor7/save/1");
        enc.write_bytes(b"c5b-sq-later");
        enc.into_bytes()
    };
    let later_object = StoredObject::new(later_payload).unwrap();
    let later_record = RevisionRecord::regular(campaign.head, campaign.head_epoch, 99)
        .with_content_refs(vec![later_object.hash()]);
    {
        let tx2 = second.transaction().unwrap();
        insert_object_once(&tx2, &None, &later_object).unwrap();
        insert_record_once(
            &tx2,
            &later_record,
            &later_record.canonical_bytes(),
            &later_record.id(),
        )
        .unwrap();
        tx2.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'head'",
            params![later_record.id().as_bytes()],
        )
        .unwrap();
        tx2.commit().unwrap();
    }
    // The walk INSIDE the held snapshot still sees the bound
    // head (the later commit wholly invisible).
    let coordinated =
        observe_range_terminal_tx(&tx, &campaign.expected, &RangeInstallationLimits::default())
            .unwrap();
    assert_eq!(coordinated.coordinated_head, bound_head);
    assert!(matches!(
        coordinated.observation,
        RangeInstallationObservation::ObservedOnAncestry { .. }
    ));
    drop(tx);
    // The FRESH observation sees the later head.
    let fresh = medium
        .observe_range_terminal_under_coordination(
            &campaign.expected,
            &RangeInstallationLimits::default(),
        )
        .unwrap();
    assert_eq!(fresh.coordinated_head, later_record.id());
    drop(medium);
    drop(second);
    let _ = std::fs::remove_file(&path);
}
