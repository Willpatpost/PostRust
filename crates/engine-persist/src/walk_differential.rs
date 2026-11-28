//! (P3-L9b) The local/sqlite walk differential battery — the A13
//! Option 2 safety net (the design §2.4.3 mandate), written FIRST and
//! green on the UNTOUCHED providers (the inverted red-then-green
//! discipline: the battery exists and passes before any walk core is
//! extracted; the extraction then has to keep it green).
//!
//! **What it does:** a generated corpus of chains — valid chains at
//! several shapes, divergent rewrites, gapped chains covered and
//! uncovered by MK skip tables, corrupt-referenced objects and
//! records, unfounded heads, the reachable cycle form, epoch
//! -regression chains (including the `u64::MAX` wrap guard and the
//! gap-aware forms), duplicate-idempotency-key chains (on-chain,
//! suffix-vs-trusted, and the reclaimed-index collisions), every
//! dependency rule, the founding rules, format failures, and the
//! K2-C2B virtual-exclusion variants — is built BYTE-IDENTICALLY
//! through both providers' stores (content addressing makes the
//! record and object bytes provider-independent; the builders differ
//! only in the physical seam: files under `objects/` + the HEAD file
//! vs `records`/`objects` rows + the `meta.head` row), then observed
//! through BOTH providers' CURRENT walk forms. The observations —
//! the cold-walk `(ChainSummary, cursor)` pairs, the post-dispatch
//! trusted-cursor state, and the refusal errors — are compared for
//! byte-identical `Debug` output, with per-case expected-rule
//! assertions so the battery has teeth on WHICH rule fired, never
//! just on equality.
//!
//! **Content-addressing's own corpus limits (honest):** a record's
//! identity is the hash of its bytes, so a literal parent cycle
//! (x.parent == y.id AND y.parent == x.id) and a literal
//! self-dependency are hash fixed points — unconstructible. The
//! reachable cycle form is a skip-table jump whose target names an
//! already-walked record; the reachable parentless-regular shape
//! refuses at the record DECODE (the kind/parent coherence check),
//! which the battery covers as a format differential — the walks'
//! own `founding-missing` branch is decode-guarded defensive code no
//! validly-encoded store can reach.
//!
//! **The frozen observables:** summaries, cursors, refusal strings.
//! The `WalkStats` counters are DELIBERATELY not compared — they are
//! the providers' own diagnostics discipline (the L9 record's
//! difference (b): local counts cold walks into `cold_walk_records`,
//! sqlite counts the post-invalidation cold walks into
//! `last_append_walked`), each pinned by its own provider's tests.
//!
//! **The suffix composite-closure corner — the divergence this
//! battery DISCOVERED on the untouched tree and the unification
//! RESOLVED (the proof, never silent):** pre-unification, a suffix
//! record whose first content reference was a `/2` root failing
//! closure verification behaved differently on the two providers —
//! local's two-pass form (the closure recomputed in `absorb_suffix`
//! after the walk) let the refusal escape directly with the trusted
//! cursor SURVIVING and the rule string
//! `composite-suffix-closure-verification`; sqlite's R15 one-pass
//! form verified the closure during the walk, routing the failure
//! into the sound-invalidation arm (cursor discarded, the cold walk
//! re-deriving the refusal as `composite-closure-verification` — the
//! string both cold walks already used; local's suffix string was
//! the odd one out of the five walk sites, an artifact of the
//! uncoordinated R10/R15 correctives, and the one place the
//! "Chain refusal rules are IDENTICAL in both" record was false).
//! The unified core takes the R15 discipline on both providers and
//! the one rule string — the corner test now asserts STRICT
//! byte-identity there (the errors equal in value, the cursors
//! discarded on both, the rule the cold-walk string), pinning the
//! resolution exactly where the divergence used to be.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use crate::checkpoint::{CheckpointMetadata, ReclaimedIndexEntry, SkipEntry};
use crate::composite_manifest::{
    self, CompositeManifest, HistoryKind, ManifestEntry, SectionKind, TAG_BLOCKED, TAG_DIRECTOR,
    TAG_FAR, TAG_HEADER, TAG_HELD, TAG_LEDGER_META, TAG_LEDGER_RECORDS, TAG_NEAR, TAG_WORLD,
};
use crate::hash::{Hash, hash_object};
use crate::local::{HEAD_DOMAIN, HEAD_FILE, HEAD_VERSION, LocalFileMedium};
use crate::medium::{CanonicalMedium, ChainSummary, MediumError, StoredObject};
use crate::record::{AuthorityMode, RecordKind, RevisionRecord};
use crate::sqlite::SqliteMedium;
use engine_core::Encoder;

// ============================================================= fixtures

/// A plain framed content object (the batteries' standard payload).
fn plain_object(payload: &[u8]) -> StoredObject {
    let mut enc = Encoder::new();
    enc.write_str("sektor7/save/1");
    enc.write_bytes(payload);
    StoredObject::new(enc.into_bytes()).unwrap()
}

/// The full-grammar `/2` composite manifest fixture (the exclusion
/// battery's known-valid section structure): six singleton sections,
/// a two-sealed-plus-one-tail ledger run under boundary 4, world, and
/// director — eleven distinct chunks. `seed` varies the payloads so
/// distinct fixtures never collide on identities.
fn composite_fixture(seed: u8) -> (StoredObject, Vec<StoredObject>) {
    let mut nonce = seed;
    let mut payload = move || {
        nonce = nonce.wrapping_add(1);
        vec![nonce; 10 + nonce as usize]
    };
    let mut entries: Vec<ManifestEntry> = Vec::new();
    let mut chunks: Vec<StoredObject> = Vec::new();
    for tag in [
        TAG_HEADER,
        TAG_NEAR,
        TAG_FAR,
        TAG_BLOCKED,
        TAG_HELD,
        TAG_LEDGER_META,
    ] {
        let bytes = payload();
        let object = composite_manifest::encode_chunk_envelope(tag, &bytes);
        entries.push(ManifestEntry {
            tag,
            identity: object.hash(),
            payload_length: bytes.len() as u64,
            kind: SectionKind::Singleton,
        });
        chunks.push(object);
    }
    let sealed_width = 4u64 * 31;
    for _ in 0..2 {
        let bytes = {
            let mut sealed = payload();
            sealed.resize(sealed_width as usize, nonce);
            sealed
        };
        let object = composite_manifest::encode_chunk_envelope(TAG_LEDGER_RECORDS, &bytes);
        entries.push(ManifestEntry {
            tag: TAG_LEDGER_RECORDS,
            identity: object.hash(),
            payload_length: sealed_width,
            kind: SectionKind::LedgerSealed,
        });
        chunks.push(object);
    }
    let tail = {
        let mut bytes = payload();
        bytes.resize(2 * 31, nonce);
        bytes
    };
    let object = composite_manifest::encode_chunk_envelope(TAG_LEDGER_RECORDS, &tail);
    entries.push(ManifestEntry {
        tag: TAG_LEDGER_RECORDS,
        identity: object.hash(),
        payload_length: tail.len() as u64,
        kind: SectionKind::LedgerTail,
    });
    chunks.push(object);
    for tag in [TAG_WORLD, TAG_DIRECTOR] {
        let bytes = payload();
        let object = composite_manifest::encode_chunk_envelope(tag, &bytes);
        entries.push(ManifestEntry {
            tag,
            identity: object.hash(),
            payload_length: bytes.len() as u64,
            kind: SectionKind::Singleton,
        });
        chunks.push(object);
    }
    let manifest = CompositeManifest {
        history_kind: HistoryKind::Full,
        ledger_boundary: 4,
        composed_length: entries.iter().map(|entry| entry.payload_length).sum(),
        entries,
    };
    let root = StoredObject::new(composite_manifest::encode(&manifest)).unwrap();
    (root, chunks)
}

// ==================================================== the corpus spec

/// One provider-neutral store mutation. The executors differ only in
/// the physical seam; the bytes (hence every identity) are identical.
enum Step {
    /// A content object at its own identity.
    Object(StoredObject),
    /// RAW unframed bytes stored as an object at their own hash —
    /// the malformed-domain corpus (the walk's domain peek must
    /// refuse the same malformed length prefix on both providers;
    /// `StoredObject::new` would reject the bytes before they could
    /// ever be stored).
    RawObject(Vec<u8>),
    /// RAW undecodable bytes stored AS A RECORD at their own hash —
    /// the sqlite seam keeps records in their own table (the row's
    /// index columns are irrelevant to the walk, which loads by hash
    /// and decodes the bytes).
    RawRecord(Vec<u8>),
    /// A revision record (its canonical bytes at its own identity).
    Record(RevisionRecord),
    /// A revision record whose SQLite ROW suppresses the `idem`
    /// column while its decoded bytes keep the key — the
    /// duplicate-key corpus shape (the row's unique index is an
    /// enforcement fast-path; the walk's byte-level rule is the
    /// authority and must catch what the row was not allowed to
    /// refuse).
    RecordIdemSuppressed(RevisionRecord),
    /// The authoritative head (the HEAD file / the `meta.head` row).
    Head(Hash),
    /// Removes a content object (the file / the `objects` row).
    RemoveObject(Hash),
    /// Removes a record (the file / the `records` row).
    RemoveRecord(Hash),
    /// Replaces an object's stored bytes with hash-mismatching
    /// garbage (referenced corruption on load).
    CorruptObject(Hash),
    /// Replaces a record's stored bytes with hash-mismatching garbage
    /// (referenced corruption on load).
    CorruptRecord(Hash),
}

/// A built chain: the store steps, the head, and the record ids
/// oldest-first (the corpus's shared fixture values).
struct Chain {
    steps: Vec<Step>,
    head: Hash,
    ids: Vec<Hash>,
}

impl Chain {
    /// A regular record appended above this chain's head (the suffix
    /// corpus's producer).
    fn suffix_record(&self, object: &StoredObject, tick: u64) -> RevisionRecord {
        let epoch = u64::try_from(self.ids.len()).unwrap() - 1;
        RevisionRecord::regular(self.head, epoch, tick).with_content_refs(vec![object.hash()])
    }
}

/// F1 — the plain valid chain: F → R1(k1, deps F) → R2(k2, two
/// objects) → R3(deps R2) → R4(k4) → R5. Six records, two content
/// objects, keys and dependencies exercising the trusted indexes.
fn plain_chain() -> (Chain, StoredObject) {
    let object = plain_object(b"wdiff-plain");
    let second = plain_object(b"wdiff-second");
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "wdiff", 0)
        .with_content_refs(vec![object.hash()]);
    let mut steps = vec![
        Step::Object(object.clone()),
        Step::Object(second.clone()),
        Step::Record(founding.clone()),
    ];
    let mut ids = vec![founding.id()];
    let mut parent = founding.id();
    for (epoch, tick) in (0..5u64).zip(1..=5u64) {
        let mut record =
            RevisionRecord::regular(parent, epoch, tick).with_content_refs(if tick == 2 {
                vec![object.hash(), second.hash()]
            } else {
                vec![object.hash()]
            });
        if tick == 1 {
            record = record
                .with_idempotency_key(b"wdiff-k1".to_vec())
                .with_depends_on(vec![ids[0]]);
        }
        if tick == 2 {
            record = record.with_idempotency_key(b"wdiff-k2".to_vec());
        }
        if tick == 3 {
            record = record.with_depends_on(vec![ids[1]]);
        }
        if tick == 4 {
            record = record.with_idempotency_key(b"wdiff-k4".to_vec());
        }
        steps.push(Step::Record(record.clone()));
        ids.push(record.id());
        parent = record.id();
    }
    let head = parent;
    steps.push(Step::Head(head));
    (Chain { steps, head, ids }, object)
}

/// F2/F4 — the checkpoint chain: F(0) → P1(1) → P2(2) → SK(3) →
/// MK(4), the MK carrying a checkpoint metadata object with the skip
/// table and reclaim index the `fixture` closure derives from the
/// four pre-MK record ids `[F, P1, P2, SK]` (the skip targets and
/// index entries name chain records, so the closure receives their
/// ids before the MK exists). `delete_range` removes the P2 and SK
/// records (and their content object) — the physical PP-B3 gap the
/// self-skip covers.
fn mk_chain(
    fixture: impl FnOnce(&[Hash; 4]) -> (Vec<SkipEntry>, Vec<ReclaimedIndexEntry>, bool),
) -> Chain {
    let object = plain_object(b"wdiff-mk");
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "wdiff-mk", 0)
        .with_content_refs(vec![object.hash()]);
    let p1 = RevisionRecord::regular(founding.id(), 0, 1).with_content_refs(vec![object.hash()]);
    let p2 = RevisionRecord::regular(p1.id(), 1, 2).with_content_refs(vec![object.hash()]);
    let sk = RevisionRecord::regular(p2.id(), 2, 3).with_content_refs(vec![object.hash()]);
    let ids = [founding.id(), p1.id(), p2.id(), sk.id()];
    let (skips, index, delete_range) = fixture(&ids);
    let metadata = CheckpointMetadata::new(sk.id(), p2.id(), skips, index, [7; 32], [9; 32]);
    let meta_object = StoredObject::new(metadata.canonical_bytes()).unwrap();
    let mk = RevisionRecord::regular(sk.id(), 3, 4).with_content_refs(vec![meta_object.hash()]);
    let mut steps = vec![
        Step::Object(object.clone()),
        Step::Object(meta_object),
        Step::Record(founding.clone()),
        Step::Record(p1.clone()),
        Step::Record(p2.clone()),
        Step::Record(sk.clone()),
        Step::Record(mk.clone()),
    ];
    if delete_range {
        // The physical PP-B3 gap: the P2 and SK records vanish (their
        // shared content object stays — F and P1 still reference it,
        // and the sqlite objects row is harmless retained evidence).
        steps.push(Step::RemoveRecord(p2.id()));
        steps.push(Step::RemoveRecord(sk.id()));
    }
    steps.push(Step::Head(mk.id()));
    Chain {
        steps,
        head: mk.id(),
        ids: vec![founding.id(), p1.id(), p2.id(), sk.id(), mk.id()],
    }
}

/// F3 — the composite chain: F(plain) → R1(the manifest's shared tail
/// chunk as its content object) → SNAP(the `/2` root, a regular
/// record whose closure is verified transitively through the
/// manifest). `seed` varies the fixture so chains never share
/// identities.
fn composite_chain(seed: u8) -> (Chain, usize) {
    let (root, chunks) = composite_fixture(seed);
    let shared = chunks.last().unwrap().clone();
    let plain = plain_object(b"wdiff-snap");
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "wdiff-snap", 0)
        .with_content_refs(vec![plain.hash()]);
    let r1 = RevisionRecord::regular(founding.id(), 0, 1).with_content_refs(vec![shared.hash()]);
    let snap = RevisionRecord::regular(r1.id(), 1, 2).with_content_refs(vec![root.hash()]);
    let mut steps = vec![
        Step::Object(plain),
        Step::Object(root),
        Step::Record(founding.clone()),
        Step::Record(r1.clone()),
    ];
    for chunk in &chunks {
        steps.push(Step::Object(chunk.clone()));
    }
    steps.push(Step::Record(snap.clone()));
    steps.push(Step::Head(snap.id()));
    // The cold walk's content-object count: plain + root + every
    // distinct chunk identity (the shared tail is one of them).
    let expected_objects = 2 + chunks.len();
    (
        Chain {
            steps,
            head: snap.id(),
            ids: vec![founding.id(), r1.id(), snap.id()],
        },
        expected_objects,
    )
}

// ======================================================= the executors

fn write_local_head(root: &Path, head: Hash) {
    let mut enc = Encoder::new();
    enc.write_str(HEAD_DOMAIN);
    enc.write_u32(HEAD_VERSION);
    enc.write_bytes(head.as_bytes());
    fs::write(root.join(HEAD_FILE), enc.into_bytes()).unwrap();
}

/// Executes the spec against the local store's physical seam (the
/// content-addressed object files and the HEAD file). Records and
/// content objects are the same files in this store — the records'
/// rows distinction is the sqlite seam's alone.
fn run_local_steps(medium: &LocalFileMedium, root: &Path, steps: &[Step]) {
    for step in steps {
        match step {
            Step::Object(object) => {
                fs::write(medium.object_path_pub(&object.hash()), object.bytes()).unwrap();
            }
            Step::RawObject(bytes) => {
                fs::write(medium.object_path_pub(&hash_object(bytes)), bytes).unwrap();
            }
            Step::RawRecord(bytes) => {
                fs::write(medium.object_path_pub(&hash_object(bytes)), bytes).unwrap();
            }
            Step::Record(record) | Step::RecordIdemSuppressed(record) => {
                fs::write(
                    medium.object_path_pub(&record.id()),
                    record.canonical_bytes(),
                )
                .unwrap();
            }
            Step::Head(head) => write_local_head(root, *head),
            Step::RemoveObject(hash) | Step::RemoveRecord(hash) => {
                fs::remove_file(medium.object_path_pub(hash)).unwrap();
            }
            Step::CorruptObject(hash) | Step::CorruptRecord(hash) => {
                fs::write(medium.object_path_pub(hash), b"wdiff-corrupt-garbage-bytes").unwrap();
            }
        }
    }
}

fn sqlite_put_record(conn: &Connection, record: &RevisionRecord, row_idem: bool) {
    let kind = match record.kind {
        RecordKind::Founding { .. } => 0i64,
        RecordKind::Regular => 1i64,
    };
    conn.execute(
        "INSERT OR REPLACE INTO records (hash, bytes, epoch, parent, idem, kind, tick) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            record.id().as_bytes(),
            record.canonical_bytes(),
            record.epoch as i64,
            record.parent.as_ref().map(|hash| &hash.as_bytes()[..]),
            if row_idem {
                record.idempotency_key.as_deref()
            } else {
                None
            },
            kind,
            record.tick as i64,
        ],
    )
    .unwrap();
}

/// Executes the spec against the sqlite store's physical seam (the
/// `records`/`objects` rows and the `meta.head` row) through the
/// battery's connection seam — the same hand-craft discipline the
/// in-tree sqlite batteries use. A founding record also seeds the
/// authority row, exactly as the real append path does (the
/// open-time cross-check requires it).
fn run_sqlite_steps(medium: &SqliteMedium, steps: &[Step]) {
    let conn = medium.walk_differential_conn();
    for step in steps {
        match step {
            Step::Object(object) => {
                conn.execute(
                    "INSERT OR REPLACE INTO objects (hash, domain, bytes) VALUES (?1, ?2, ?3)",
                    params![object.hash().as_bytes(), object.domain(), object.bytes()],
                )
                .unwrap();
            }
            Step::RawObject(bytes) => {
                conn.execute(
                    "INSERT OR REPLACE INTO objects (hash, domain, bytes) VALUES (?1, ?2, ?3)",
                    params![hash_object(bytes).as_bytes(), "raw", bytes],
                )
                .unwrap();
            }
            Step::RawRecord(bytes) => {
                conn.execute(
                    "INSERT OR REPLACE INTO records (hash, bytes, epoch, parent, idem, kind, tick) \
                     VALUES (?1, ?2, 0, NULL, NULL, 0, 0)",
                    params![hash_object(bytes).as_bytes(), bytes],
                )
                .unwrap();
            }
            Step::Record(record) => {
                sqlite_put_record(conn, record, true);
                if record.parent.is_none() {
                    medium.walk_differential_seed_authority(record);
                }
            }
            Step::RecordIdemSuppressed(record) => {
                sqlite_put_record(conn, record, false);
                if record.parent.is_none() {
                    medium.walk_differential_seed_authority(record);
                }
            }
            Step::Head(head) => {
                conn.execute(
                    "INSERT OR REPLACE INTO meta (key, value) VALUES ('head', ?1)",
                    params![head.as_bytes()],
                )
                .unwrap();
            }
            Step::RemoveObject(hash) => {
                conn.execute(
                    "DELETE FROM objects WHERE hash = ?1",
                    params![hash.as_bytes()],
                )
                .unwrap();
            }
            Step::RemoveRecord(hash) => {
                conn.execute(
                    "DELETE FROM records WHERE hash = ?1",
                    params![hash.as_bytes()],
                )
                .unwrap();
            }
            Step::CorruptObject(hash) => {
                conn.execute(
                    "UPDATE objects SET bytes = ?1 WHERE hash = ?2",
                    params![b"wdiff-corrupt-garbage-bytes".to_vec(), hash.as_bytes()],
                )
                .unwrap();
            }
            Step::CorruptRecord(hash) => {
                conn.execute(
                    "UPDATE records SET bytes = ?1 WHERE hash = ?2",
                    params![b"wdiff-corrupt-garbage-bytes".to_vec(), hash.as_bytes()],
                )
                .unwrap();
            }
        }
    }
}

// ===================================================== the store pairs

/// One corpus case's scratch area (dropped — with the mediums dropped
/// first by declaration order — so Windows file locks release before
/// the cleanup).
struct Case {
    root: PathBuf,
}

impl Case {
    fn new(label: &str) -> Case {
        let root = std::env::temp_dir().join(format!(
            "s7-wdiff-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        Case { root }
    }

    fn local_root(&self) -> PathBuf {
        self.root.join("local")
    }

    fn db_path(&self) -> PathBuf {
        self.root.join("db.sqlite")
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Builds the SAME store through both providers' seams and opens the
/// observation mediums over it — the session open's cold walk
/// establishes the trusted cursor on both (the foundations are valid
/// and fully referenced by construction, so the local open's
/// unreferenced-object sweep has nothing to act on). The case's
/// scratch is WIPED first: every call builds from a clean root, so
/// successive sub-cases sharing one `Case` never observe each other's
/// mutated stores (the prior sub-case's mediums are dropped at its
/// block's end, before this runs).
fn opened_pair(case: &Case, steps: &[Step]) -> (LocalFileMedium, SqliteMedium) {
    let _ = fs::remove_dir_all(&case.root);
    fs::create_dir_all(&case.root).unwrap();
    let local_root = case.local_root();
    {
        let scratch = LocalFileMedium::open(&local_root).unwrap();
        run_local_steps(&scratch, &local_root, steps);
    }
    let local = LocalFileMedium::open(&local_root).unwrap();
    {
        let scratch = SqliteMedium::open(case.db_path()).unwrap();
        run_sqlite_steps(&scratch, steps);
    }
    let sqlite = SqliteMedium::open(case.db_path()).unwrap();
    (local, sqlite)
}

/// The external second session (the real append path — the realistic
/// suffix producer).
fn external_local(case: &Case) -> LocalFileMedium {
    LocalFileMedium::open(case.local_root()).unwrap()
}

fn external_sqlite(case: &Case) -> SqliteMedium {
    SqliteMedium::open(case.db_path()).unwrap()
}

// ===================================================== the observations

/// One cold-walk observation: the summary and cursor fingerprints on
/// success, the refusal's `Debug` string on failure.
#[derive(Debug, PartialEq, Eq)]
struct ColdObservation {
    summary: Option<String>,
    cursor: Option<String>,
    error: Option<String>,
}

fn cold_outcome(result: Result<(ChainSummary, String), MediumError>) -> ColdObservation {
    match result {
        Ok((summary, cursor)) => ColdObservation {
            summary: Some(format!("{summary:?}")),
            cursor: normalize_cursor(Some(cursor)),
            error: None,
        },
        Err(error) => ColdObservation {
            summary: None,
            cursor: None,
            error: Some(format!("{error:?}")),
        },
    }
}

/// Strips the provider's cursor struct-name prefix from the `Debug`
/// fingerprint — the field names, types, and order are the shared
/// §5.3.2 contract, so the remainder is the byte-identical comparison
/// form (`Cursor { … }` vs `SqliteCursor { … }`).
fn normalize_cursor(cursor: Option<String>) -> Option<String> {
    cursor.map(|text| text[text.find('{').unwrap()..].to_string())
}

/// Runs BOTH providers' current cold-walk forms over the same
/// (head, exclusion) and asserts the observations byte-identical.
fn compare_cold(
    label: &str,
    local: &LocalFileMedium,
    sqlite: &SqliteMedium,
    head: Hash,
    exclude: &BTreeSet<Hash>,
) -> ColdObservation {
    let local_observation = cold_outcome(local.walk_differential_cold(head, exclude));
    let sqlite_observation = cold_outcome(sqlite.walk_differential_cold(head, exclude));
    assert_eq!(
        local_observation, sqlite_observation,
        "the cold walk differs on {label}"
    );
    local_observation
}

/// One observed-HEAD dispatch observation: the `head()` outcome and
/// the post-dispatch trusted-cursor fingerprint.
#[derive(Debug, PartialEq, Eq)]
struct HeadObservation {
    observed: Option<Option<Hash>>,
    error: Option<String>,
    cursor: Option<String>,
}

fn local_head_observation(medium: &mut LocalFileMedium) -> HeadObservation {
    let observed = medium.head();
    HeadObservation {
        observed: observed.as_ref().ok().copied(),
        error: observed.err().map(|error| format!("{error:?}")),
        cursor: normalize_cursor(medium.walk_differential_cursor()),
    }
}

fn sqlite_head_observation(medium: &mut SqliteMedium) -> HeadObservation {
    let observed = medium.head();
    HeadObservation {
        observed: observed.as_ref().ok().copied(),
        error: observed.err().map(|error| format!("{error:?}")),
        cursor: normalize_cursor(medium.walk_differential_cursor()),
    }
}

/// Drives BOTH providers' observed-HEAD validation (the public
/// `head()` — the lock/transaction-scoped `validate_observed`
/// dispatch) and asserts the observations byte-identical.
fn compare_head(
    label: &str,
    local: &mut LocalFileMedium,
    sqlite: &mut SqliteMedium,
) -> HeadObservation {
    let local_observation = local_head_observation(local);
    let sqlite_observation = sqlite_head_observation(sqlite);
    assert_eq!(
        local_observation, sqlite_observation,
        "the observed-HEAD validation differs on {label}"
    );
    local_observation
}

/// The per-case expected-rule assertion — the battery's teeth on
/// WHICH refusal fired, never just on cross-provider equality.
fn cold_refusal(observation: &ColdObservation, rule: &str) {
    let Some(error) = observation.error.as_deref() else {
        panic!("expected the {rule} refusal, got {observation:?}");
    };
    assert!(
        error.contains(rule),
        "expected the {rule} refusal, got {error}"
    );
}

fn head_refusal(observation: &HeadObservation, rule: &str) {
    let Some(error) = observation.error.as_deref() else {
        panic!("expected the {rule} refusal, got {observation:?}");
    };
    assert!(
        error.contains(rule),
        "expected the {rule} refusal, got {error}"
    );
}

fn no_exclusion() -> BTreeSet<Hash> {
    BTreeSet::new()
}

fn exclusion(hashes: &[Hash]) -> BTreeSet<Hash> {
    hashes.iter().copied().collect()
}

// ========================================================== the battery

/// The valid chains: founding-only, the plain six-record chain (keys,
/// dependencies, two objects), the checkpoint chain (the MK staging
/// with no gaps), and the composite chain (the verified `/2` closure
/// reachability) — summaries and cursors byte-identical.
#[test]
fn valid_chains_cold_walk_identically() {
    let case = Case::new("valid");
    {
        // The founding-only store.
        let object = plain_object(b"wdiff-founding-only");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "wdiff-fo", 0)
            .with_content_refs(vec![object.hash()]);
        let steps = vec![
            Step::Object(object),
            Step::Record(founding.clone()),
            Step::Head(founding.id()),
        ];
        let (local, sqlite) = opened_pair(&case, &steps);
        let observation = compare_cold(
            "founding-only",
            &local,
            &sqlite,
            founding.id(),
            &no_exclusion(),
        );
        let summary = observation.summary.as_deref().unwrap();
        assert!(summary.contains("records: 1"), "the summary: {summary}");
    }
    {
        let (chain, _object) = plain_chain();
        let (local, sqlite) = opened_pair(&case, &chain.steps);
        let observation = compare_cold("plain", &local, &sqlite, chain.head, &no_exclusion());
        let summary = observation.summary.as_deref().unwrap();
        assert!(summary.contains("records: 6"), "the summary: {summary}");
        assert!(
            summary.contains("content_objects: 2"),
            "the summary: {summary}"
        );
        assert!(
            summary.contains("checkpoints: []"),
            "no checkpoints on the plain chain: {summary}"
        );
    }
    {
        let chain = mk_chain(|ids| {
            (
                vec![SkipEntry::new(Hash::ZERO, ids[1])],
                vec![
                    ReclaimedIndexEntry::new(None, ids[2], 2),
                    ReclaimedIndexEntry::new(None, ids[3], 3),
                ],
                false,
            )
        });
        let (local, sqlite) = opened_pair(&case, &chain.steps);
        let observation = compare_cold("with-mk", &local, &sqlite, chain.head, &no_exclusion());
        let summary = observation.summary.as_deref().unwrap();
        assert!(summary.contains("records: 5"), "the summary: {summary}");
        assert!(
            summary.contains("reclaimed_records: 0"),
            "the present range is not reclaimed: {summary}"
        );
        assert!(
            !summary.contains("checkpoints: []"),
            "the checkpoint is staged: {summary}"
        );
    }
    {
        let (chain, expected_objects) = composite_chain(3);
        let (local, sqlite) = opened_pair(&case, &chain.steps);
        let observation = compare_cold("composite", &local, &sqlite, chain.head, &no_exclusion());
        let summary = observation.summary.as_deref().unwrap();
        assert!(summary.contains("records: 3"), "the summary: {summary}");
        assert!(
            summary.contains(&format!("content_objects: {expected_objects}")),
            "the closure identities are reachable: {summary}"
        );
    }
}

/// The PP-B3 gapped chains: the covered physical gap (the self-skip
/// jump, the reclaimed membership universe), the uncovered gap, the
/// covered-but-wrong-epoch target (the gap-aware linkage refusal),
/// and the covered-but-unkeyed skip (no entry for the standing
/// record).
#[test]
fn gapped_chains_cold_walk_identically() {
    let case = Case::new("gapped");
    {
        // The covered physical gap: F → P1 → [P2, SK reclaimed] → MK.
        let chain = mk_chain(|ids| {
            (
                vec![SkipEntry::new(Hash::ZERO, ids[1])],
                vec![
                    ReclaimedIndexEntry::new(None, ids[2], 2),
                    ReclaimedIndexEntry::new(None, ids[3], 3),
                ],
                true,
            )
        });
        let (local, sqlite) = opened_pair(&case, &chain.steps);
        let observation = compare_cold("covered-gap", &local, &sqlite, chain.head, &no_exclusion());
        let summary = observation.summary.as_deref().unwrap();
        assert!(
            summary.contains("records: 3") && summary.contains("reclaimed_records: 2"),
            "MK, P1, F walk with the reclaimed membership: {summary}"
        );
    }
    {
        // The uncovered gap: no MK at all, the parent simply absent.
        let (chain, _object) = plain_chain();
        let absent = chain.ids[2];
        let (local, sqlite) = opened_pair(&case, &chain.steps);
        run_local_steps(&local, &case.local_root(), &[Step::RemoveRecord(absent)]);
        run_sqlite_steps(&sqlite, &[Step::RemoveRecord(absent)]);
        let observation = compare_cold(
            "uncovered-gap",
            &local,
            &sqlite,
            chain.head,
            &no_exclusion(),
        );
        cold_refusal(&observation, "MissingObject");
    }
    {
        // The covered gap whose jump target violates the gap-aware
        // strictly-earlier epoch linkage: the standing record R(2)
        // jumps to a present side record also at epoch 2 — not
        // strictly below. The side branch is internally coherent
        // (F(0) → A(1) → side(2)) so ONLY the gap-aware bound
        // distinguishes it — a normal-linkage failure must not mask
        // the gap rule.
        let object = plain_object(b"wdiff-wrong-epoch");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "wdiff-we", 0)
            .with_content_refs(vec![object.hash()]);
        let base =
            RevisionRecord::regular(founding.id(), 0, 1).with_content_refs(vec![object.hash()]);
        let middle =
            RevisionRecord::regular(founding.id(), 0, 2).with_content_refs(vec![object.hash()]);
        let side_record =
            RevisionRecord::regular(middle.id(), 1, 3).with_content_refs(vec![object.hash()]);
        let standing =
            RevisionRecord::regular(base.id(), 1, 2).with_content_refs(vec![object.hash()]);
        let sk =
            RevisionRecord::regular(standing.id(), 2, 3).with_content_refs(vec![object.hash()]);
        let metadata = CheckpointMetadata::new(
            sk.id(),
            standing.id(),
            vec![SkipEntry::new(standing.id(), side_record.id())],
            vec![ReclaimedIndexEntry::new(None, base.id(), 1)],
            [7; 32],
            [9; 32],
        );
        let meta_object = StoredObject::new(metadata.canonical_bytes()).unwrap();
        let mk = RevisionRecord::regular(sk.id(), 3, 4).with_content_refs(vec![meta_object.hash()]);
        let base_id = base.id();
        let steps = vec![
            Step::Object(object),
            Step::Object(meta_object),
            Step::Record(founding),
            Step::Record(base),
            Step::Record(middle),
            Step::Record(side_record),
            Step::Record(standing),
            Step::Record(sk),
            Step::Record(mk.clone()),
            Step::Head(mk.id()),
            Step::RemoveRecord(base_id),
        ];
        // The invalid chain is installed AFTER the mediums open (an
        // unfounded open never walks it); the shim's cold walk
        // observes it directly.
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold(
            "wrong-epoch-target",
            &local,
            &sqlite,
            mk.id(),
            &no_exclusion(),
        );
        cold_refusal(&observation, "epoch-regression");
    }
    {
        // The covered-but-unkeyed skip: the gap stands on a record
        // the table has no entry for.
        let object = plain_object(b"wdiff-unkeyed");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "wdiff-uk", 0)
            .with_content_refs(vec![object.hash()]);
        let base =
            RevisionRecord::regular(founding.id(), 0, 1).with_content_refs(vec![object.hash()]);
        let standing =
            RevisionRecord::regular(base.id(), 1, 2).with_content_refs(vec![object.hash()]);
        let sk =
            RevisionRecord::regular(standing.id(), 2, 3).with_content_refs(vec![object.hash()]);
        let metadata = CheckpointMetadata::new(
            sk.id(),
            standing.id(),
            vec![SkipEntry::new(
                hash_object(b"wdiff-other-record"),
                founding.id(),
            )],
            vec![ReclaimedIndexEntry::new(None, base.id(), 1)],
            [7; 32],
            [9; 32],
        );
        let meta_object = StoredObject::new(metadata.canonical_bytes()).unwrap();
        let mk = RevisionRecord::regular(sk.id(), 3, 4).with_content_refs(vec![meta_object.hash()]);
        let base_id = base.id();
        let steps = vec![
            Step::Object(object),
            Step::Object(meta_object),
            Step::Record(founding),
            Step::Record(base),
            Step::Record(standing),
            Step::Record(sk),
            Step::Record(mk.clone()),
            Step::Head(mk.id()),
            Step::RemoveRecord(base_id),
        ];
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold("unkeyed-skip", &local, &sqlite, mk.id(), &no_exclusion());
        cold_refusal(&observation, "MissingObject");
    }
}

/// The corrupt-referenced and unfounded-head shapes: a corrupt
/// content object, a corrupt record, a missing referenced object, and
/// a head naming a nonexistent record — every refusal identical with
/// the specific hash carried.
#[test]
fn corrupt_and_missing_references_refuse_identically() {
    let case = Case::new("corrupt");
    let (chain, object) = plain_chain();
    {
        let (local, sqlite) = opened_pair(&case, &chain.steps);
        run_local_steps(
            &local,
            &case.local_root(),
            &[Step::CorruptObject(object.hash())],
        );
        run_sqlite_steps(&sqlite, &[Step::CorruptObject(object.hash())]);
        let observation = compare_cold(
            "corrupt-object",
            &local,
            &sqlite,
            chain.head,
            &no_exclusion(),
        );
        cold_refusal(&observation, "ReferencedObjectCorrupt");
    }
    {
        let (local, sqlite) = opened_pair(&case, &chain.steps);
        run_local_steps(
            &local,
            &case.local_root(),
            &[Step::CorruptRecord(chain.ids[3])],
        );
        run_sqlite_steps(&sqlite, &[Step::CorruptRecord(chain.ids[3])]);
        let observation = compare_cold(
            "corrupt-record",
            &local,
            &sqlite,
            chain.head,
            &no_exclusion(),
        );
        cold_refusal(&observation, "ReferencedObjectCorrupt");
    }
    {
        let (local, sqlite) = opened_pair(&case, &chain.steps);
        run_local_steps(
            &local,
            &case.local_root(),
            &[Step::RemoveObject(object.hash())],
        );
        run_sqlite_steps(&sqlite, &[Step::RemoveObject(object.hash())]);
        let observation = compare_cold(
            "missing-object",
            &local,
            &sqlite,
            chain.head,
            &no_exclusion(),
        );
        cold_refusal(&observation, "MissingObject");
    }
    {
        let (local, sqlite) = opened_pair(&case, &chain.steps);
        let bogus = hash_object(b"wdiff-nonexistent-head");
        run_local_steps(&local, &case.local_root(), &[Step::Head(bogus)]);
        run_sqlite_steps(&sqlite, &[Step::Head(bogus)]);
        let observation = compare_cold("unfounded-head", &local, &sqlite, bogus, &no_exclusion());
        cold_refusal(&observation, "MissingObject");
    }
}

/// The structural chain rules: the reachable cycle form (a skip jump
/// targeting an already-walked record), the epoch regression, the
/// `u64::MAX` no-successor wrap guard, the founding-epoch coherence,
/// the decode-coherence refusal (the parentless-regular shape — see
/// the module doc's content-addressing limits), the undecodable
/// record, and the malformed first-object domain prefix.
#[test]
fn structural_chain_rules_refuse_identically() {
    let case = Case::new("structural");
    {
        // The reachable cycle: the skip jump targets the MK itself —
        // an already-walked record.
        let object = plain_object(b"wdiff-revisit");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "wdiff-rv", 0)
            .with_content_refs(vec![object.hash()]);
        let base =
            RevisionRecord::regular(founding.id(), 0, 1).with_content_refs(vec![object.hash()]);
        let standing =
            RevisionRecord::regular(base.id(), 1, 2).with_content_refs(vec![object.hash()]);
        let sk =
            RevisionRecord::regular(standing.id(), 2, 3).with_content_refs(vec![object.hash()]);
        let metadata = CheckpointMetadata::new(
            sk.id(),
            standing.id(),
            // The jump targets SK itself — an already-walked record
            // (and never an indexed one, so the metadata decodes).
            vec![SkipEntry::new(standing.id(), sk.id())],
            vec![ReclaimedIndexEntry::new(None, base.id(), 1)],
            [7; 32],
            [9; 32],
        );
        let meta_object = StoredObject::new(metadata.canonical_bytes()).unwrap();
        let mk = RevisionRecord::regular(sk.id(), 3, 4).with_content_refs(vec![meta_object.hash()]);
        let base_id = base.id();
        let steps = vec![
            Step::Object(object),
            Step::Object(meta_object),
            Step::Record(founding),
            Step::Record(base),
            Step::Record(standing),
            Step::Record(sk.clone()),
            Step::Record(mk.clone()),
            Step::Head(mk.id()),
            Step::RemoveRecord(base_id),
        ];
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold("cycle", &local, &sqlite, mk.id(), &no_exclusion());
        cold_refusal(&observation, "cycle");
    }
    {
        // The epoch regression: A(3) directly on F(0).
        let object = plain_object(b"wdiff-epoch");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "wdiff-ep", 0)
            .with_content_refs(vec![object.hash()]);
        let a = RevisionRecord::regular(founding.id(), 2, 1).with_content_refs(vec![object.hash()]);
        let steps = vec![
            Step::Object(object),
            Step::Record(founding),
            Step::Record(a.clone()),
            Step::Head(a.id()),
        ];
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation =
            compare_cold("epoch-regression", &local, &sqlite, a.id(), &no_exclusion());
        cold_refusal(&observation, "epoch-regression");
    }
    {
        // The u64::MAX wrap guard: a record at MAX has no successor.
        let object = plain_object(b"wdiff-max");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "wdiff-max", 0)
            .with_content_refs(vec![object.hash()]);
        let mut maxed =
            RevisionRecord::regular(founding.id(), 0, 1).with_content_refs(vec![object.hash()]);
        maxed.epoch = u64::MAX;
        let top = RevisionRecord::regular(maxed.id(), 0, 2).with_content_refs(vec![object.hash()]);
        let steps = vec![
            Step::Object(object),
            Step::Record(founding),
            Step::Record(maxed),
            Step::Record(top.clone()),
            Step::Head(top.id()),
        ];
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold("epoch-max", &local, &sqlite, top.id(), &no_exclusion());
        cold_refusal(&observation, "epoch-regression");
    }
    {
        // The founding-epoch coherence: a founding record at epoch 3.
        let object = plain_object(b"wdiff-kind");
        let mut founding = RevisionRecord::founding(AuthorityMode::Standalone, "wdiff-ki", 0)
            .with_content_refs(vec![object.hash()]);
        founding.epoch = 3;
        let steps = vec![
            Step::Object(object),
            Step::Record(founding.clone()),
            Step::Head(founding.id()),
        ];
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold(
            "kind-incoherent",
            &local,
            &sqlite,
            founding.id(),
            &no_exclusion(),
        );
        cold_refusal(&observation, "kind-incoherent");
    }
    {
        // The parentless-regular shape refuses at the record DECODE
        // (the kind/parent coherence) — identically on both.
        let object = plain_object(b"wdiff-parentless");
        let mut parentless =
            RevisionRecord::regular(Hash::ZERO, 0, 1).with_content_refs(vec![object.hash()]);
        parentless.parent = None;
        let top =
            RevisionRecord::regular(parentless.id(), 1, 2).with_content_refs(vec![object.hash()]);
        let steps = vec![
            Step::Object(object),
            Step::Record(parentless),
            Step::Record(top.clone()),
            Step::Head(top.id()),
        ];
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold(
            "parentless-regular",
            &local,
            &sqlite,
            top.id(),
            &no_exclusion(),
        );
        cold_refusal(&observation, "Format");
    }
    {
        // The undecodable record: stored bytes that hash to the head
        // identity but do not decode as a record (stored raw — the
        // bytes are deliberately unframed).
        let garbage = b"wdiff-not-a-record-at-all";
        let bogus = hash_object(garbage);
        let (local, sqlite) = opened_pair(&case, &{
            let (chain, _object) = plain_chain();
            chain.steps
        });
        let steps = [Step::RawRecord(garbage.to_vec()), Step::Head(bogus)];
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold("format-record", &local, &sqlite, bogus, &no_exclusion());
        cold_refusal(&observation, "Format");
    }
    {
        // The malformed first-object domain prefix: the record's
        // first content object does not frame a domain string (the
        // absurd length prefix refuses the peek identically).
        let garbage = [0xff_u8, 0xfe, 0xfd, 0xfc, 0xfb];
        let garbage_id = hash_object(&garbage);
        let (chain, _object) = plain_chain();
        let (local, sqlite) = opened_pair(&case, &chain.steps);
        let record = RevisionRecord::regular(chain.head, 5, 9).with_content_refs(vec![garbage_id]);
        let steps = [
            Step::RawObject(garbage.to_vec()),
            Step::Record(record.clone()),
            Step::Head(record.id()),
        ];
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold(
            "format-domain",
            &local,
            &sqlite,
            record.id(),
            &no_exclusion(),
        );
        cold_refusal(&observation, "Format");
    }
}

/// The idempotency-key rules: the on-chain duplicate (the row index
/// suppressed — the walk's byte-level rule must catch it), the
/// reclaimed-index key colliding with an on-chain key, and the
/// two-MK duplicate index hash.
#[test]
fn idempotency_key_rules_refuse_identically() {
    let case = Case::new("idempotency");
    {
        // The on-chain duplicate: two records carrying the same key.
        let object = plain_object(b"wdiff-dup-key");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "wdiff-dk", 0)
            .with_content_refs(vec![object.hash()]);
        let first = RevisionRecord::regular(founding.id(), 0, 1)
            .with_content_refs(vec![object.hash()])
            .with_idempotency_key(b"wdiff-dup".to_vec());
        let second = RevisionRecord::regular(first.id(), 1, 2)
            .with_content_refs(vec![object.hash()])
            .with_idempotency_key(b"wdiff-dup".to_vec());
        let steps = vec![
            Step::Object(object),
            Step::Record(founding),
            Step::Record(first),
            Step::RecordIdemSuppressed(second.clone()),
            Step::Head(second.id()),
        ];
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold(
            "duplicate-key",
            &local,
            &sqlite,
            second.id(),
            &no_exclusion(),
        );
        cold_refusal(&observation, "duplicate-idempotency-key");
    }
    {
        // The reclaimed-index key colliding with an on-chain key: the
        // index entry for the (absent) P2 carries P1's key.
        let object = plain_object(b"wdiff-index-key");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "wdiff-ik", 0)
            .with_content_refs(vec![object.hash()]);
        let p1 = RevisionRecord::regular(founding.id(), 0, 1)
            .with_content_refs(vec![object.hash()])
            .with_idempotency_key(b"wdiff-collide".to_vec());
        let p2 = RevisionRecord::regular(p1.id(), 1, 2).with_content_refs(vec![object.hash()]);
        let sk = RevisionRecord::regular(p2.id(), 2, 3).with_content_refs(vec![object.hash()]);
        let metadata = CheckpointMetadata::new(
            sk.id(),
            p2.id(),
            vec![SkipEntry::new(Hash::ZERO, p1.id())],
            vec![ReclaimedIndexEntry::new(
                Some(b"wdiff-collide".to_vec()),
                p2.id(),
                2,
            )],
            [7; 32],
            [9; 32],
        );
        let meta_object = StoredObject::new(metadata.canonical_bytes()).unwrap();
        let mk = RevisionRecord::regular(sk.id(), 3, 4).with_content_refs(vec![meta_object.hash()]);
        let p2_id = p2.id();
        let sk_id = sk.id();
        let steps = vec![
            Step::Object(object),
            Step::Object(meta_object),
            Step::Record(founding),
            Step::Record(p1),
            Step::Record(p2),
            Step::Record(sk),
            Step::Record(mk.clone()),
            Step::Head(mk.id()),
            Step::RemoveRecord(p2_id),
            Step::RemoveRecord(sk_id),
        ];
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold(
            "index-key-collision",
            &local,
            &sqlite,
            mk.id(),
            &no_exclusion(),
        );
        cold_refusal(&observation, "duplicate-idempotency-key");
    }
    {
        // The two-MK duplicate index hash: both checkpoints index the
        // same absent revision — the membership universe refuses the
        // second insertion.
        let object = plain_object(b"wdiff-two-mk");
        let shared_absent = hash_object(b"wdiff-absent-revision");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "wdiff-tm", 0)
            .with_content_refs(vec![object.hash()]);
        let p1 =
            RevisionRecord::regular(founding.id(), 0, 1).with_content_refs(vec![object.hash()]);
        let sk1 = RevisionRecord::regular(p1.id(), 1, 2).with_content_refs(vec![object.hash()]);
        let first_meta = StoredObject::new(
            CheckpointMetadata::new(
                sk1.id(),
                p1.id(),
                vec![],
                vec![ReclaimedIndexEntry::new(None, shared_absent, 1)],
                [7; 32],
                [9; 32],
            )
            .canonical_bytes(),
        )
        .unwrap();
        let mk1 =
            RevisionRecord::regular(sk1.id(), 2, 3).with_content_refs(vec![first_meta.hash()]);
        let sk2 = RevisionRecord::regular(mk1.id(), 3, 4).with_content_refs(vec![object.hash()]);
        let second_meta = StoredObject::new(
            CheckpointMetadata::new(
                sk2.id(),
                mk1.id(),
                vec![],
                vec![ReclaimedIndexEntry::new(None, shared_absent, 2)],
                [7; 32],
                [9; 32],
            )
            .canonical_bytes(),
        )
        .unwrap();
        let mk2 =
            RevisionRecord::regular(sk2.id(), 4, 5).with_content_refs(vec![second_meta.hash()]);
        let steps = vec![
            Step::Object(object),
            Step::Object(first_meta),
            Step::Object(second_meta),
            Step::Record(founding),
            Step::Record(p1),
            Step::Record(sk1),
            Step::Record(mk1),
            Step::Record(sk2),
            Step::Record(mk2.clone()),
            Step::Head(mk2.id()),
        ];
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold(
            "index-hash-duplicate",
            &local,
            &sqlite,
            mk2.id(),
            &no_exclusion(),
        );
        cold_refusal(&observation, "checkpoint-index-incoherent");
    }
}

/// The dependency rules: the nonexistent dependency, a dependency
/// naming a real record OUTSIDE the authoritative history (the
/// constructible form of the newer-revision class — see the module
/// doc's content-addressing limits: a strictly-newer WALKED record's
/// identity cannot be known when the older record is built), a
/// reachable content hash (never a revision), and the
/// reclaimed-indexed dependency whose indexed epoch is not strictly
/// earlier.
#[test]
fn dependency_rules_refuse_identically() {
    let case = Case::new("depends");
    {
        // The nonexistent dependency.
        let own = plain_object(b"wdiff-dep-missing");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "wdiff-dm", 0)
            .with_content_refs(vec![own.hash()]);
        let record = RevisionRecord::regular(founding.id(), 0, 1)
            .with_content_refs(vec![own.hash()])
            .with_depends_on(vec![hash_object(b"wdiff-nope")]);
        let top = RevisionRecord::regular(record.id(), 1, 2).with_content_refs(vec![own.hash()]);
        let steps = vec![
            Step::Object(own),
            Step::Record(founding),
            Step::Record(record),
            Step::Record(top.clone()),
            Step::Head(top.id()),
        ];
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold("dep-missing", &local, &sqlite, top.id(), &no_exclusion());
        cold_refusal(&observation, "depends-on-unresolvable");
    }
    {
        // A dependency naming a real record outside the authoritative
        // history: the sibling branch record exists in the store but
        // is never walked — a dependency must name a prior revision
        // of the AUTHORITATIVE chain.
        let own = plain_object(b"wdiff-dep-outside");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "wdiff-do", 0)
            .with_content_refs(vec![own.hash()]);
        let middle =
            RevisionRecord::regular(founding.id(), 0, 1).with_content_refs(vec![own.hash()]);
        let sibling =
            RevisionRecord::regular(middle.id(), 1, 2).with_content_refs(vec![own.hash()]);
        let depender = RevisionRecord::regular(middle.id(), 1, 3)
            .with_content_refs(vec![own.hash()])
            .with_depends_on(vec![sibling.id()]);
        let steps = vec![
            Step::Object(own),
            Step::Record(founding),
            Step::Record(middle),
            Step::Record(sibling),
            Step::Record(depender.clone()),
            Step::Head(depender.id()),
        ];
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold(
            "dep-outside-history",
            &local,
            &sqlite,
            depender.id(),
            &no_exclusion(),
        );
        cold_refusal(&observation, "depends-on-unresolvable");
    }
    {
        // A reachable CONTENT hash is never a revision.
        let own = plain_object(b"wdiff-dep-content");
        let content = plain_object(b"wdiff-dep-content-target");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "wdiff-dc", 0)
            .with_content_refs(vec![own.hash()]);
        let record = RevisionRecord::regular(founding.id(), 0, 1)
            .with_content_refs(vec![own.hash()])
            .with_depends_on(vec![content.hash()]);
        let top = RevisionRecord::regular(record.id(), 1, 2).with_content_refs(vec![own.hash()]);
        let steps = vec![
            Step::Object(own),
            Step::Object(content),
            Step::Record(founding),
            Step::Record(record),
            Step::Record(top.clone()),
            Step::Head(top.id()),
        ];
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold("dep-content", &local, &sqlite, top.id(), &no_exclusion());
        cold_refusal(&observation, "depends-on-unresolvable");
    }
    {
        // The reclaimed-indexed dependency whose indexed epoch is not
        // strictly earlier than the depender's.
        let own = plain_object(b"wdiff-dep-reclaimed");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "wdiff-dr", 0)
            .with_content_refs(vec![own.hash()]);
        let base = RevisionRecord::regular(founding.id(), 0, 1).with_content_refs(vec![own.hash()]);
        let depender = RevisionRecord::regular(base.id(), 1, 2)
            .with_content_refs(vec![own.hash()])
            .with_depends_on(vec![base.id()]);
        let sk = RevisionRecord::regular(depender.id(), 2, 3).with_content_refs(vec![own.hash()]);
        let metadata = CheckpointMetadata::new(
            sk.id(),
            depender.id(),
            vec![SkipEntry::new(depender.id(), founding.id())],
            // The indexed epoch (2) is NOT strictly below the
            // depender's own epoch (2) — unresolvable.
            vec![ReclaimedIndexEntry::new(None, base.id(), 2)],
            [7; 32],
            [9; 32],
        );
        let meta_object = StoredObject::new(metadata.canonical_bytes()).unwrap();
        let mk = RevisionRecord::regular(sk.id(), 3, 4).with_content_refs(vec![meta_object.hash()]);
        let base_id = base.id();
        let steps = vec![
            Step::Object(own),
            Step::Object(meta_object),
            Step::Record(founding),
            Step::Record(base),
            Step::Record(depender),
            Step::Record(sk),
            Step::Record(mk.clone()),
            Step::Head(mk.id()),
            Step::RemoveRecord(base_id),
        ];
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold(
            "dep-reclaimed-epoch",
            &local,
            &sqlite,
            mk.id(),
            &no_exclusion(),
        );
        cold_refusal(&observation, "depends-on-unresolvable");
    }
}

/// The K2-C2B virtual-exclusion variants: the covered exclusion (the
/// audited range routed through the authenticated self-skip), the
/// uncovered exclusion (no covering table), and the exclusion of a
/// record below SK (uncoverable by construction — SK's retirement is
/// the self-skip's business alone).
#[test]
fn virtual_exclusion_walks_agree_identically() {
    let case = Case::new("exclusion");
    {
        let chain = mk_chain(|ids| {
            (
                vec![SkipEntry::new(Hash::ZERO, ids[1])],
                vec![
                    ReclaimedIndexEntry::new(None, ids[2], 2),
                    ReclaimedIndexEntry::new(None, ids[3], 3),
                ],
                false,
            )
        });
        let (local, sqlite) = opened_pair(&case, &chain.steps);
        let observation = compare_cold(
            "covered-exclusion",
            &local,
            &sqlite,
            chain.head,
            &exclusion(&[chain.ids[2], chain.ids[3]]),
        );
        let summary = observation.summary.as_deref().unwrap();
        assert!(
            summary.contains("records: 3") && summary.contains("reclaimed_records: 2"),
            "the simulated post-retirement view: {summary}"
        );
    }
    {
        let (chain, _object) = plain_chain();
        let (local, sqlite) = opened_pair(&case, &chain.steps);
        let observation = compare_cold(
            "uncovered-exclusion",
            &local,
            &sqlite,
            chain.head,
            &exclusion(&[chain.ids[2]]),
        );
        cold_refusal(&observation, "MissingObject");
    }
    {
        // Excluding P2 while SK stays: the gap stands on SK, and no
        // skip may stand on SK (the metadata decode forbids it) — the
        // uncoverable exclusion fails closed exactly like an
        // uncovered physical absence.
        let chain = mk_chain(|ids| {
            (
                vec![SkipEntry::new(Hash::ZERO, ids[1])],
                vec![ReclaimedIndexEntry::new(None, ids[2], 2)],
                false,
            )
        });
        let (local, sqlite) = opened_pair(&case, &chain.steps);
        let observation = compare_cold(
            "below-sk-exclusion",
            &local,
            &sqlite,
            chain.head,
            &exclusion(&[chain.ids[2]]),
        );
        cold_refusal(&observation, "MissingObject");
    }
}

/// The dispatch arms through the public `head()`: the unfounded
/// store (None, None), the already-verified head (the O(0) arm), the
/// externally-founded store observed by a cursorless session (Some,
/// None — the cold establishment), and the vanished head (None, Some
/// — the fail-closed divergence).
#[test]
fn dispatch_arms_agree_identically() {
    let case = Case::new("arms");
    {
        let (mut local, mut sqlite) = opened_pair(&case, &[]);
        let observation = compare_head("none-none", &mut local, &mut sqlite);
        assert_eq!(observation.observed, Some(None));
        assert_eq!(observation.cursor, None);
    }
    {
        let (chain, _object) = plain_chain();
        let (mut local, mut sqlite) = opened_pair(&case, &chain.steps);
        let observation = compare_head("verified-head", &mut local, &mut sqlite);
        assert_eq!(observation.observed, Some(Some(chain.head)));
        assert!(observation.cursor.is_some(), "the trusted cursor stands");
    }
    {
        // (Some, None): the session opened unfounded, the store
        // founded externally afterwards.
        let (mut local, mut sqlite) = opened_pair(&case, &[]);
        {
            let object = plain_object(b"wdiff-arms");
            let founding = RevisionRecord::founding(AuthorityMode::Standalone, "wdiff-ar", 0)
                .with_content_refs(vec![object.hash()]);
            let r1 =
                RevisionRecord::regular(founding.id(), 0, 1).with_content_refs(vec![object.hash()]);
            let mut external = external_local(&case);
            external
                .append(&founding, std::slice::from_ref(&object), None)
                .unwrap();
            external
                .append(&r1, std::slice::from_ref(&object), Some(founding.id()))
                .unwrap();
            drop(external);
            let mut external = external_sqlite(&case);
            external
                .append(&founding, std::slice::from_ref(&object), None)
                .unwrap();
            external
                .append(&r1, std::slice::from_ref(&object), Some(founding.id()))
                .unwrap();
            drop(external);
        }
        let observation = compare_head("some-none", &mut local, &mut sqlite);
        assert!(observation.error.is_none(), "{observation:?}");
        assert!(
            observation.observed.is_some_and(|head| head.is_some()),
            "the external founding is observed: {observation:?}"
        );
        assert!(
            observation.cursor.is_some(),
            "the cold establishment installed the cursor on both"
        );
    }
    {
        // (None, Some): the head vanished under a standing cursor.
        let (chain, _object) = plain_chain();
        let (mut local, mut sqlite) = opened_pair(&case, &chain.steps);
        fs::remove_file(case.local_root().join(HEAD_FILE)).unwrap();
        // The sqlite unfounded sentinel is the empty head row.
        sqlite
            .walk_differential_conn()
            .execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('head', x'')",
                [],
            )
            .unwrap();
        let observation = compare_head("none-some", &mut local, &mut sqlite);
        head_refusal(&observation, "divergence");
        assert!(
            observation.cursor.is_some(),
            "the divergence arm never discards the trusted cursor"
        );
    }
}

/// The valid suffix absorptions: the external single-record suffix
/// through the REAL append path (then the O(0) follow-up on the new
/// head), and the hand-crafted composite suffix (the `/2` closure
/// absorbed into the trusted reachability — the good-path composite
/// equality).
#[test]
fn valid_suffix_absorption_agrees_identically() {
    let case = Case::new("suffix");
    {
        let (chain, object) = plain_chain();
        let (mut local, mut sqlite) = opened_pair(&case, &chain.steps);
        let r6 = chain.suffix_record(&object, 6);
        {
            let mut external = external_local(&case);
            external
                .append(&r6, std::slice::from_ref(&object), Some(chain.head))
                .unwrap();
            drop(external);
            let mut external = external_sqlite(&case);
            external
                .append(&r6, std::slice::from_ref(&object), Some(chain.head))
                .unwrap();
            drop(external);
        }
        let observation = compare_head("valid-suffix", &mut local, &mut sqlite);
        assert_eq!(observation.observed, Some(Some(r6.id())));
        assert!(observation.cursor.is_some());
        // The follow-up on the now-verified head: O(0) on both, the
        // cursor unchanged.
        let followup = compare_head("valid-suffix-o1", &mut local, &mut sqlite);
        assert_eq!(followup.cursor, observation.cursor);
    }
    {
        // The hand-crafted composite suffix: the record's single
        // reference is a valid `/2` root whose full closure the
        // suffix walk verifies and the absorption carries into the
        // trusted reachability set.
        let (chain, _object) = plain_chain();
        let (mut local, mut sqlite) = opened_pair(&case, &chain.steps);
        let (root, chunks) = composite_fixture(9);
        let record = RevisionRecord::regular(chain.head, 5, 6).with_content_refs(vec![root.hash()]);
        let mut steps = vec![Step::Object(root), Step::Record(record.clone())];
        for chunk in &chunks {
            steps.push(Step::Object(chunk.clone()));
        }
        steps.push(Step::Head(record.id()));
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_head("composite-suffix", &mut local, &mut sqlite);
        assert_eq!(observation.observed, Some(Some(record.id())));
        assert!(observation.cursor.is_some());
    }
}

/// The refusing suffixes through the public `head()` dispatch: the
/// boundary epoch regression, the parentless suffix record (the
/// divergence arm), the corrupt suffix object (the sound-invalidation
/// arm — cursor discarded, cold walk re-derives the refusal), the
/// suffix key duplicating a trusted key, and the unresolvable suffix
/// dependency.
#[test]
fn refusing_suffixes_agree_identically() {
    let case = Case::new("suffix-refuse");
    {
        // The boundary epoch: the oldest suffix revision must sit
        // exactly one epoch above the trusted head.
        let (chain, object) = plain_chain();
        let (mut local, mut sqlite) = opened_pair(&case, &chain.steps);
        let late = RevisionRecord::regular(chain.head, 6, 7).with_content_refs(vec![object.hash()]);
        let steps = [Step::Record(late.clone()), Step::Head(late.id())];
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_head("boundary-epoch", &mut local, &mut sqlite);
        head_refusal(&observation, "epoch-regression");
    }
    {
        // The divergent suffix: the observed head's ancestry reaches a
        // DIFFERENT founding record (never passing through the
        // trusted head) — divergence, fail closed, cursor kept.
        // (The parentless-regular form is decode-guarded — see the
        // module doc's content-addressing limits.)
        let (chain, object) = plain_chain();
        let (mut local, mut sqlite) = opened_pair(&case, &chain.steps);
        let foreign = plain_object(b"wdiff-divergent-foreign");
        let foreign_founding = RevisionRecord::founding(AuthorityMode::Standalone, "wdiff-div", 0)
            .with_content_refs(vec![foreign.hash()]);
        let divergent = RevisionRecord::regular(foreign_founding.id(), 0, 7)
            .with_content_refs(vec![object.hash()]);
        let steps = vec![
            Step::Object(foreign),
            Step::Record(foreign_founding),
            Step::Record(divergent.clone()),
            Step::Head(divergent.id()),
        ];
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_head("divergence", &mut local, &mut sqlite);
        head_refusal(&observation, "divergence");
        assert!(observation.cursor.is_some());
    }
    {
        // The corrupt suffix object: the sound-invalidation arm —
        // cursor discarded, the cold walk re-derives the refusal,
        // both fail closed identically.
        let (chain, _object) = plain_chain();
        let (mut local, mut sqlite) = opened_pair(&case, &chain.steps);
        let fresh = plain_object(b"wdiff-suffix-corrupt");
        let record =
            RevisionRecord::regular(chain.head, 5, 6).with_content_refs(vec![fresh.hash()]);
        let steps = [
            Step::Object(fresh.clone()),
            Step::Record(record.clone()),
            Step::Head(record.id()),
            Step::CorruptObject(fresh.hash()),
        ];
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_head("invalidation", &mut local, &mut sqlite);
        head_refusal(&observation, "ReferencedObjectCorrupt");
        assert_eq!(
            observation.cursor, None,
            "the sound invalidation discards the cursor on both"
        );
    }
    {
        // The suffix key duplicating a trusted key.
        let (chain, object) = plain_chain();
        let (mut local, mut sqlite) = opened_pair(&case, &chain.steps);
        let dup = RevisionRecord::regular(chain.head, 5, 6)
            .with_content_refs(vec![object.hash()])
            .with_idempotency_key(b"wdiff-k1".to_vec());
        let steps = [
            Step::RecordIdemSuppressed(dup.clone()),
            Step::Head(dup.id()),
        ];
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_head("suffix-duplicate-key", &mut local, &mut sqlite);
        head_refusal(&observation, "duplicate-idempotency-key");
    }
    {
        // The unresolvable suffix dependency.
        let (chain, object) = plain_chain();
        let (mut local, mut sqlite) = opened_pair(&case, &chain.steps);
        let record = RevisionRecord::regular(chain.head, 5, 6)
            .with_content_refs(vec![object.hash()])
            .with_depends_on(vec![hash_object(b"wdiff-suffix-nope")]);
        let steps = [Step::Record(record.clone()), Step::Head(record.id())];
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_head("suffix-depends", &mut local, &mut sqlite);
        head_refusal(&observation, "depends-on-unresolvable");
    }
}

/// THE suffix composite-closure corner — the divergence this battery
/// DISCOVERED on the untouched tree (the module doc's record) and the
/// unification RESOLVED. The corpus: a suffix record whose first
/// content reference is a `/2` root that FAILS closure verification
/// in three ways — a malformed manifest body, a missing chunk, and a
/// chunk failing its envelope verification. On the good path (the
/// valid composite suffix) the providers agree exactly (proved by
/// `valid_suffix_absorption_agrees_identically`).
///
/// Pre-unification the two providers disagreed here in the refusal
/// STRING (local's `composite-suffix-closure-verification` vs
/// sqlite's `composite-closure-verification`) and in the cursor
/// disposition (local's post-walk closure recompute let the trusted
/// cursor survive the refusal; sqlite's in-walk R15 verification
/// routed the failure into the sound-invalidation arm — cursor
/// discarded, cold walk re-deriving the refusal). The unified core
/// takes the R15 discipline and the one cold-walk rule string on
/// BOTH providers — this test now asserts STRICT byte-identity
/// (compare_head) plus the specific rules, pinning the resolution
/// exactly where the divergence used to be: the errors equal in
/// value, the cursors discarded on both, the rule the
/// cold-walk string both providers' cold walks always used.
#[test]
fn suffix_composite_closure_corner_is_the_documented_divergence() {
    let case = Case::new("closure-corner");
    // (a) The malformed manifest body: a `/2`-domain-framed object
    // whose payload is not a manifest.
    {
        let (chain, _object) = plain_chain();
        let (mut local, mut sqlite) = opened_pair(&case, &chain.steps);
        let mut enc = Encoder::new();
        enc.write_str(composite_manifest::SESSION_SNAPSHOT_V2_DOMAIN);
        enc.write_bytes(b"wdiff-malformed-manifest-body");
        let fake_root = StoredObject::new(enc.into_bytes()).unwrap();
        let record =
            RevisionRecord::regular(chain.head, 5, 6).with_content_refs(vec![fake_root.hash()]);
        let steps = [
            Step::Object(fake_root),
            Step::Record(record.clone()),
            Step::Head(record.id()),
        ];
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        // THE RESOLUTION: byte-identical observations — the same
        // typed refusal (the manifest decode failure), the same
        // sound-invalidation cursor disposition (discarded on both,
        // the cold walk re-deriving the refusal).
        let observation = compare_head("closure-malformed-manifest", &mut local, &mut sqlite);
        head_refusal(&observation, "Format");
        assert_eq!(
            observation.cursor, None,
            "the in-walk closure failure discards the cursor on both (the invalidation arm)"
        );
    }
    // (b) The missing chunk: a valid manifest declaring a chunk the
    // store does not carry.
    {
        let (chain, _object) = plain_chain();
        let (mut local, mut sqlite) = opened_pair(&case, &chain.steps);
        let (root, chunks) = composite_fixture(21);
        let record = RevisionRecord::regular(chain.head, 5, 6).with_content_refs(vec![root.hash()]);
        let mut steps = vec![Step::Object(root), Step::Record(record.clone())];
        for chunk in &chunks[1..] {
            steps.push(Step::Object(chunk.clone()));
        }
        steps.push(Step::Head(record.id()));
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_head("closure-missing-chunk", &mut local, &mut sqlite);
        head_refusal(&observation, "MissingObject");
        assert_eq!(observation.cursor, None);
    }
    // (c) The chunk failing its envelope verification: the manifest
    // declares TAG_HEADER but the chunk's envelope is tagged NEAR —
    // the refusal rule string that used to diverge (local's
    // `composite-suffix-closure-verification`), now the ONE
    // cold-walk string on both.
    {
        let (chain, _object) = plain_chain();
        let (mut local, mut sqlite) = opened_pair(&case, &chain.steps);
        let (reference_root, reference_chunks) = composite_fixture(33);
        let mismatched = composite_manifest::encode_chunk_envelope(TAG_NEAR, b"wdiff-wrong-tag");
        let reference = composite_manifest::decode(reference_root.bytes()).unwrap();
        let mut entries = Vec::new();
        for entry in &reference.entries {
            if entry.tag == TAG_HEADER {
                entries.push(ManifestEntry {
                    tag: TAG_HEADER,
                    identity: mismatched.hash(),
                    payload_length: b"wdiff-wrong-tag".len() as u64,
                    kind: SectionKind::Singleton,
                });
            } else {
                entries.push(entry.clone());
            }
        }
        let manifest = CompositeManifest {
            history_kind: HistoryKind::Full,
            ledger_boundary: 4,
            composed_length: entries.iter().map(|entry| entry.payload_length).sum(),
            entries,
        };
        let root = StoredObject::new(composite_manifest::encode(&manifest)).unwrap();
        let record = RevisionRecord::regular(chain.head, 5, 6).with_content_refs(vec![root.hash()]);
        let mut steps = vec![
            Step::Object(mismatched),
            Step::Object(root),
            Step::Record(record.clone()),
        ];
        for chunk in &reference_chunks {
            steps.push(Step::Object(chunk.clone()));
        }
        steps.push(Step::Head(record.id()));
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        // THE RESOLUTION: byte-identical observations with the ONE
        // rule string — the cold-walk `composite-closure-verification`
        // both providers' cold walks (and the reclaim execution walk)
        // always used; local's odd suffix string is retired.
        let observation = compare_head("closure-envelope-failure", &mut local, &mut sqlite);
        head_refusal(&observation, "composite-closure-verification");
        assert!(
            !observation
                .error
                .as_deref()
                .unwrap()
                .contains("composite-suffix-closure-verification"),
            "the retired local-only suffix rule string must not appear: {:?}",
            observation.error
        );
        assert_eq!(observation.cursor, None);
    }
}
