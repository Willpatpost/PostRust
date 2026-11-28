// ============================================================== the C5b tests
//
// (V2X-CP3-B-P2A-C5b) The provider-coordinated Range-terminal
// observation on LocalFileMedium: the head read AND the whole C5a
// walk inside ONE HEAD.lock scope. The genuine campaign is
// DIRECT-WRITTEN (the A1-R discipline — the completion family's
// publication fence forbids the generic append for the /2 and /3
// markers, and no V2 publisher exists yet), with the MK, the
// founding, and the ordinary publications appended through the REAL
// path where the fence permits.

use super::*;
use crate::ancestry_accounting::StorageRole;
use crate::batch_completion::{
    BatchCompletionMarkerV2, ClassificationEntry, EntryClassification, commit_classification,
    reclamation_complete_v2_idempotency_key,
};
use crate::checkpoint::{CheckpointMetadata, ReclaimedIndexEntry, checkpoint_mk_idempotency_key};
use crate::hash::hash_object;
use crate::range_completion::{
    RangeCompletionContext, RangeCompletionMarkerV3, RangeCoverageObservation,
    commit_journal_obligations, commit_range_coverage,
    reclamation_complete_v3_range_idempotency_key,
};
use crate::range_installation::{
    CoordinatedObservation, CoordinatedUnresolvedObligations, NotEstablished, NotObservedTerminus,
    RangeInstallationLimits, RangeInstallationObservation,
};
use crate::reclaim::{
    PlanBatch, ReclaimPlan, ReclaimUnit, ReclamationBegin, ReclamationLedgerV2,
    reclamation_begin_idempotency_key, reclamation_ledger_v2_idempotency_key,
    reclamation_plan_idempotency_key,
};

pub(super) fn plain_object(payload: &[u8]) -> Vec<u8> {
    let mut enc = engine_core::Encoder::new();
    enc.write_str("sektor7/save/1");
    enc.write_bytes(payload);
    enc.into_bytes()
}

/// The direct-writing chain builder over a REAL opened medium:
/// every keyed evidence record and its payload object written
/// through the store's own content-addressed layout, the HEAD
/// file written through the store's own framing.
struct DirectBuilder<'a> {
    medium: &'a mut LocalFileMedium,
    root: std::path::PathBuf,
    head: Hash,
    head_epoch: u64,
    tick: u64,
}

impl<'a> DirectBuilder<'a> {
    fn new(
        medium: &'a mut LocalFileMedium,
        root: std::path::PathBuf,
        seed: Hash,
        epoch: u64,
    ) -> DirectBuilder<'a> {
        DirectBuilder {
            medium,
            root,
            head: seed,
            head_epoch: epoch,
            tick: 0,
        }
    }

    /// Direct-writes one keyed evidence revision (the fenced
    /// families' only available route in tests) referencing one
    /// payload object.
    fn write(&mut self, key: Vec<u8>, payload: Vec<u8>) -> Hash {
        let payload_hash = hash_object(&payload);
        self.tick += 1;
        let record = RevisionRecord::regular(self.head, self.head_epoch, self.tick)
            .with_idempotency_key(key)
            .with_content_refs(vec![payload_hash]);
        let id = record.id();
        std::fs::write(self.medium.object_path_pub(&id), record.canonical_bytes()).unwrap();
        std::fs::write(self.medium.object_path_pub(&payload_hash), payload).unwrap();
        self.head = id;
        self.head_epoch += 1;
        id
    }

    /// Points the store's HEAD file at the given record (the
    /// store's own framing).
    fn set_head(&self, id: Hash) {
        let bytes = {
            let mut enc = engine_core::Encoder::new();
            enc.write_str(HEAD_DOMAIN);
            enc.write_u32(HEAD_VERSION);
            enc.write_bytes(id.as_bytes());
            enc.into_bytes()
        };
        std::fs::write(self.root.join(HEAD_FILE), bytes).unwrap();
    }
}

/// The genuine campaign facts (the C5a fixture shape, parameter
/// `with_terminal` controlling the terminal's publication).
pub(super) struct Campaign {
    pub(super) terminal: Option<Hash>,
    pub(super) head: Hash,
    pub(super) head_epoch: u64,
    pub(super) mk: Hash,
    pub(super) plan_record: Hash,
    pub(super) begin_record: Hash,
    pub(super) marker_records: Vec<Hash>,
    pub(super) expected: RangeCompletionContext,
}

pub(super) fn genuine_campaign(
    medium: &mut LocalFileMedium,
    root: std::path::PathBuf,
    with_terminal: bool,
) -> Campaign {
    let members = [
        hash_object(b"c5b-sk"),
        hash_object(b"c5b-m1"),
        hash_object(b"c5b-m2"),
        hash_object(b"c5b-m3"),
        hash_object(b"c5b-m4"),
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
        hash_object(b"c5b-sk-parent"),
        vec![],
        index,
        range_digest,
        seed,
    );
    let mut builder = DirectBuilder::new(medium, root, members[0], 60);
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
                objects: vec![hash_object(b"c5b-o1")],
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
    let _ = audit1_record;
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
    // The Batch markers (the C1 classification commitments),
    // each bound to its OWN batch's audit record.
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
                    hash_object(b"c5b-o1"),
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
        let context = crate::batch_completion::BatchCompletionContext {
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
    // The Range terminal (the C3 commitments over the observed
    // marker set and the plan's journal obligations).
    let expected = RangeCompletionContext {
        mk,
        plan: plan_record,
        begin: begin_record,
    };
    let terminal = if with_terminal {
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
        Some(builder.write(
            reclamation_complete_v3_range_idempotency_key(&mk),
            marker.canonical_bytes(),
        ))
    } else {
        None
    };
    let head = builder.head;
    let head_epoch = builder.head_epoch;
    builder.set_head(head);
    Campaign {
        terminal,
        head,
        head_epoch,
        mk,
        plan_record,
        begin_record,
        marker_records,
        expected,
    }
}

#[test]
fn the_unfounded_store_refuses_typed() {
    let root = std::env::temp_dir().join(format!(
        "s7-c5b-unfounded-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut medium = LocalFileMedium::open(&root).unwrap();
    let expected = RangeCompletionContext {
        mk: hash_object(b"c5b-any-mk"),
        plan: hash_object(b"c5b-any-plan"),
        begin: hash_object(b"c5b-any-begin"),
    };
    assert!(matches!(
        medium.observe_range_terminal_under_coordination(
            &expected,
            &RangeInstallationLimits::default()
        ),
        Err(MediumError::Chain { rule: "unfounded" })
    ));
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}

pub(super) fn a_root(label: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "s7-c5b-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

pub(super) fn inventory(root: &std::path::Path) -> Vec<(std::path::PathBuf, u64)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(std::path::PathBuf, u64)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                walk(&entry.path(), out);
            } else {
                out.push((entry.path(), entry.metadata().unwrap().len()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

#[test]
fn the_coordinated_positive_on_localfile() {
    let root = a_root("positive");
    let mut medium = LocalFileMedium::open(&root).unwrap();
    let campaign = genuine_campaign(&mut medium, root.clone(), true);
    let coordinated = medium
        .observe_range_terminal_under_coordination(
            &campaign.expected,
            &RangeInstallationLimits::default(),
        )
        .unwrap();
    let CoordinatedObservation {
        coordinated_head,
        observation,
        unresolved,
    } = coordinated;
    assert_eq!(coordinated_head, campaign.head);
    assert_eq!(coordinated_head, campaign.terminal.unwrap());
    let RangeInstallationObservation::ObservedOnAncestry {
        observed_head,
        candidate,
        covering_mk,
        plan_record,
        begin_record,
        batch_markers,
        unresolved: observation_unresolved,
    } = observation
    else {
        panic!("the genuine campaign observes positively");
    };
    assert_eq!(observed_head, coordinated_head);
    assert_eq!(candidate.revision, campaign.terminal.unwrap());
    assert_eq!(covering_mk, campaign.mk);
    assert_eq!(plan_record, campaign.plan_record);
    assert_eq!(begin_record, campaign.begin_record);
    assert_eq!(batch_markers, campaign.marker_records);
    assert_eq!(
        observation_unresolved,
        crate::range_installation::UnresolvedObligations::all()
    );
    // R8: the exhaustive twelve-field destructure at the
    // coordinated level.
    let CoordinatedUnresolvedObligations {
        provider_canonical_head: NotEstablished,
        begin_durability_authority: NotEstablished,
        founding_derived_seed_authenticity: NotEstablished,
        original_range_digest_historical_correctness: NotEstablished,
        original_object_closures: NotEstablished,
        batch_classification_historical_correctness: NotEstablished,
        physical_retirement: NotEstablished,
        journal_retirement_and_ownership: NotEstablished,
        recovery_convergence: NotEstablished,
        crossing_permission: NotEstablished,
        startup_safety: NotEstablished,
        head_currency_beyond_scope: NotEstablished,
    } = unresolved;
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_absent_terminal_under_coordination_classifies() {
    // No terminal published: the complete walk through the
    // evidence to the EXPECTED covering checkpoint — the
    // absence bound to the coordinated head.
    let root = a_root("absent");
    let mut medium = LocalFileMedium::open(&root).unwrap();
    let campaign = genuine_campaign(&mut medium, root.clone(), false);
    let coordinated = medium
        .observe_range_terminal_under_coordination(
            &campaign.expected,
            &RangeInstallationLimits::default(),
        )
        .unwrap();
    let CoordinatedObservation {
        coordinated_head,
        observation,
        unresolved: _,
    } = coordinated;
    assert_eq!(coordinated_head, campaign.head);
    assert!(matches!(
        observation,
        RangeInstallationObservation::NotObservedOnAncestry {
            terminus: NotObservedTerminus::ExpectedCoveringCheckpoint { mk },
            ..
        } if mk == campaign.mk
    ));
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_stale_after_scope_contract_is_snapshotted() {
    // After the coordination scope closes and a publication
    // lands, the earlier CoordinatedObservation remains a valid
    // snapshot (its head-currency field the honest naming), and
    // a FRESH observation over the new head sees the new head.
    let root = a_root("stale");
    let mut medium = LocalFileMedium::open(&root).unwrap();
    let campaign = genuine_campaign(&mut medium, root.clone(), true);
    let first = medium
        .observe_range_terminal_under_coordination(
            &campaign.expected,
            &RangeInstallationLimits::default(),
        )
        .unwrap();
    assert_eq!(first.coordinated_head, campaign.head);
    // The later publication, direct-written (the head moves on
    // disk behind the open medium's back — the A3 lesson: the
    // open medium's cursor never learns of the direct-written
    // chain, and the REAL append path would refuse the CAS
    // against its stale in-memory state; the head-movement fact
    // is what this contract tests).
    let object = StoredObject::new(plain_object(b"c5b-later")).unwrap();
    let record = RevisionRecord::regular(campaign.head, campaign.head_epoch, 99)
        .with_content_refs(vec![object.hash()]);
    let later_head = record.id();
    std::fs::write(
        medium.object_path_pub(&later_head),
        record.canonical_bytes(),
    )
    .unwrap();
    std::fs::write(medium.object_path_pub(&object.hash()), object.bytes()).unwrap();
    {
        let bytes = {
            let mut enc = engine_core::Encoder::new();
            enc.write_str(HEAD_DOMAIN);
            enc.write_u32(HEAD_VERSION);
            enc.write_bytes(later_head.as_bytes());
            enc.into_bytes()
        };
        std::fs::write(root.join(HEAD_FILE), bytes).unwrap();
    }
    // The earlier snapshot is unchanged — an owned value bound
    // to its own coordinated head (never re-verified).
    assert_eq!(first.coordinated_head, campaign.head);
    // The fresh observation sees the NEW head (and the terminal
    // below it).
    let second = medium
        .observe_range_terminal_under_coordination(
            &campaign.expected,
            &RangeInstallationLimits::default(),
        )
        .unwrap();
    assert_eq!(second.coordinated_head, later_head);
    assert!(matches!(
        second.observation,
        RangeInstallationObservation::ObservedOnAncestry { .. }
    ));
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_observation_is_read_only_on_localfile() {
    // The store's complete file inventory is byte-identical
    // before and after a coordinated observation.
    let root = a_root("readonly");
    let mut medium = LocalFileMedium::open(&root).unwrap();
    let campaign = genuine_campaign(&mut medium, root.clone(), true);
    let before = inventory(&root);
    let _ = medium
        .observe_range_terminal_under_coordination(
            &campaign.expected,
            &RangeInstallationLimits::default(),
        )
        .unwrap();
    let after = inventory(&root);
    assert_eq!(before, after, "the observation mutated nothing");
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_foreign_boundary_under_coordination_classifies() {
    // A later checkpoint published above the terminal: the walk
    // stops at the foreign boundary — absence NOT claimed, the
    // boundary never crossed.
    let root = a_root("boundary");
    let mut medium = LocalFileMedium::open(&root).unwrap();
    let campaign = genuine_campaign(&mut medium, root.clone(), true);
    // The foreign checkpoint whose SK is the terminal.
    let foreign_member = hash_object(b"c5b-foreign-member");
    let foreign_metadata = CheckpointMetadata::new(
        campaign.head,
        hash_object(b"c5b-foreign-sk-parent"),
        vec![],
        vec![ReclaimedIndexEntry::new(
            Some(b"key-foreign".to_vec()),
            foreign_member,
            7,
        )],
        [3u8; 32],
        [5u8; 32],
    );
    let mut medium = medium;
    let head_epoch = 71u64;
    let foreign_record = RevisionRecord::regular(campaign.head, head_epoch, 100)
        .with_idempotency_key(checkpoint_mk_idempotency_key(&campaign.head))
        .with_content_refs(vec![hash_object(&foreign_metadata.canonical_bytes())]);
    let foreign_mk = foreign_record.id();
    std::fs::write(
        medium.object_path_pub(&foreign_mk),
        foreign_record.canonical_bytes(),
    )
    .unwrap();
    std::fs::write(
        medium.object_path_pub(&hash_object(&foreign_metadata.canonical_bytes())),
        foreign_metadata.canonical_bytes(),
    )
    .unwrap();
    {
        let bytes = {
            let mut enc = engine_core::Encoder::new();
            enc.write_str(HEAD_DOMAIN);
            enc.write_u32(HEAD_VERSION);
            enc.write_bytes(foreign_mk.as_bytes());
            enc.into_bytes()
        };
        std::fs::write(root.join(HEAD_FILE), bytes).unwrap();
    }
    let coordinated = medium
        .observe_range_terminal_under_coordination(
            &campaign.expected,
            &RangeInstallationLimits::default(),
        )
        .unwrap();
    let CoordinatedObservation {
        coordinated_head,
        observation,
        unresolved: _,
    } = coordinated;
    assert_eq!(coordinated_head, foreign_mk);
    assert!(matches!(
        observation,
        RangeInstallationObservation::HiddenBehindCheckpointBoundary { boundary_mk, .. }
            if boundary_mk == foreign_mk
    ));
    drop(medium);
    let _ = std::fs::remove_dir_all(&root);
}
