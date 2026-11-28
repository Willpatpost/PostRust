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
//!
//! **(T29, commit 1) The battery extension — the audit's five oracle
//! axes plus the residency census, ADDITIVE to everything above:**
//! (1) the dispatch state set — every `(observed, trusted)`
//! combination with its accepted cursor state or refusal, the
//! cursor's survival/discard disposition pinned per arm;
//! (2) the full frozen-rule classification matrix per walk form
//! (cold, excluding, suffix) — the constructible rule×form cells
//! fired with expected-rule assertions, the unconstructible and
//! not-applicable cells documented in place (the hash-fixed-point
//! cycles, the decode-guarded `founding-missing`, the suffix walk's
//! no-index-merge/no-founding-coherence arms); (3) the
//! conflict-ordering pins — multi-violation records proving WHICH
//! rule fires FIRST, derived from the core's check sequence (new
//! teeth: the battery proved the rules fire, never their
//! precedence); (4) the stale-cursor family — the external-writer
//! states including REPEATED cursor advancement with the
//! per-provider walk-stats accounting pinned across the sequence;
//! (5) provider failure at walk read points — transport-level read
//! failures (the local `Io` class, the sqlite `Sqlite` class) that
//! must refuse typed fail-closed on both, with the honest
//! seam-vs-transport comparison rule stated in the test: the
//! seam-independent classifications (`Chain { rule }`,
//! `MissingObject`, `ReferencedObjectCorrupt`) compare
//! byte-identically as the battery always has; the transport
//! variants are legitimately provider-shaped and compare as
//! refused-with-typed-error on both. The residency census (the
//! R07-§7 shape) rides instrumented test-local `ChainReads` mirrors
//! of each provider's read surface — asserted summary-equal to the
//! production cold-walk seams — so the census's measured rows
//! (whole-read volume, populations, wall time) observe the
//! production walk core over genuinely provider-built stores.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rusqlite::{Connection, params};

use crate::chain_walk::{self, ChainReads};
use crate::checkpoint::{CheckpointMetadata, ReclaimedIndexEntry, SkipEntry, compute_range_digest};
use crate::composite_manifest::{
    self, CompositeManifest, HistoryKind, ManifestEntry, SectionKind, TAG_BLOCKED, TAG_DIRECTOR,
    TAG_FAR, TAG_HEADER, TAG_HELD, TAG_LEDGER_META, TAG_LEDGER_RECORDS, TAG_NEAR, TAG_WORLD,
};
use crate::hash::{Hash, hash_object};
use crate::local::{
    HEAD_DOMAIN, HEAD_FILE, HEAD_VERSION, LocalFileMedium, MAX_OBJECT_BYTES, OBJECTS_DIR,
};
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
#[derive(Clone)]
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
    /// (T29) Stores ARBITRARY bytes at a CHOSEN identity — the
    /// streaming-discriminating corpus (a mid-object byte flip kept
    /// under the original hash; truncations at the stream-window
    /// boundaries under the original hash; hand-framed envelopes).
    CorruptObjectAt(Hash, Vec<u8>),
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
            Step::CorruptObjectAt(hash, bytes) => {
                fs::write(medium.object_path_pub(hash), bytes).unwrap();
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
            Step::CorruptObjectAt(hash, bytes) => {
                conn.execute(
                    "UPDATE objects SET bytes = ?1 WHERE hash = ?2",
                    params![bytes.to_vec(), hash.as_bytes()],
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
    /// The CURRENT call's scratch root (a fresh subdirectory per
    /// `opened_pair` call — the Windows lesson, runs 46155517625 →
    /// 46157193468: reuse-by-wipe depends on Windows delete
    /// semantics, where a handle held anywhere in the tree — a
    /// medium, a scanner, anything — makes `remove_dir_all` fail
    /// with os error 32, and the failure mode is un-diagnosable from
    /// the pod. A fresh subdirectory per call provides the SAME
    /// isolation the wipe existed for (successive sub-cases never
    /// observe each other's stores) with no deletion at all; the
    /// whole case tree is cleaned best-effort at `Drop`, after the
    /// mediums are gone).
    current: std::cell::Cell<usize>,
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
        Case {
            root,
            current: std::cell::Cell::new(0),
        }
    }

    /// The next per-call scratch root: a fresh subdirectory, never a
    /// reused one. Interior mutability because the test battery
    /// passes `&Case` through its helpers.
    fn fresh_root(&self) -> PathBuf {
        let n = self.current.get() + 1;
        self.current.set(n);
        self.root.join(format!("call-{n}"))
    }

    fn local_root(&self) -> PathBuf {
        let n = self.current.get();
        self.root.join(format!("call-{n}")).join("local")
    }

    fn db_path(&self) -> PathBuf {
        let n = self.current.get();
        self.root.join(format!("call-{n}")).join("db.sqlite")
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
/// unreferenced-object sweep has nothing to act on). Every call
/// builds in its OWN fresh subdirectory (`Case::fresh_root`) —
/// successive sub-cases sharing one `Case` never observe each other's
/// mutated stores, and nothing is ever deleted mid-test (the Windows
/// delete-semantics trap, see `Case::current`'s doc).
fn opened_pair(case: &Case, steps: &[Step]) -> (LocalFileMedium, SqliteMedium) {
    let root = case.fresh_root();
    fs::create_dir_all(&root).unwrap();
    let local_root = root.join("local");
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

// ================================================== (T29) the battery extension
//
// The audit's five oracle axes and the residency census (commit 1,
// zero production change): every fixture below is additive — the
// frozen observables, assertions, and byte-identities above stand
// untouched. The instrumented readers mirror each provider's own
// documented load semantics so the census's measured rows ride the
// production walk core (`crate::chain_walk::walk_chain_excluding`,
// the exact free function the providers' seams call) over
// genuinely provider-built stores; every census test asserts the
// mirror walk's summary equals the production seam's summary on the
// same store, so a drifting mirror cannot silently flatter the rows.

/// The measured rows the instrumented readers collect (the census's
/// directly observed volume/count terms — allocator-free, visible
/// through the reader seam the walks already consume).
#[derive(Debug, Default, Clone, Copy)]
struct ReaderInstrumentation {
    /// Whole content-object reads (the `read_object_bytes` calls —
    /// the legitimate whole holds only, after the streaming rewire).
    whole_object_reads: u64,
    /// Whole content-object BYTES returned (the volume row).
    whole_object_bytes: u64,
    /// Record-byte reads.
    record_reads: u64,
    /// The parent-present probes.
    presence_probes: u64,
    /// (T29, commit 2) The unverified prefix reads (the streaming
    /// verify's leading windows).
    prefix_read_calls: u64,
    /// (T29, commit 2) The streaming verified reads (the
    /// `verify_object_streaming` calls — every content reference's
    /// verification once the streaming rewire lands).
    stream_verified_calls: u64,
    /// (T29, commit 2) The total BYTES the streaming verifies folded
    /// (the streamed volume row — the bytes READ, never held).
    stream_verified_bytes: u64,
}

/// (T29) The local store's reader mirror — the layout's own
/// hash-verified referenced load semantics (stat →
/// NotFound→`MissingObject`, over-limit→`Format`, read error →
/// typed `Io`, hash mismatch → `ReferencedObjectCorrupt`; the
/// presence probe is the object path's `is_file`), instrumented.
struct LocalStoreReader<'a> {
    root: &'a Path,
    instrumentation: ReaderInstrumentation,
}

impl LocalStoreReader<'_> {
    fn object_path(&self, hash: &Hash) -> PathBuf {
        self.root.join(OBJECTS_DIR).join(hash.hex())
    }

    /// The layout's `load_referenced_at`, mirrored exactly.
    fn load_referenced(&self, hash: &Hash) -> Result<Vec<u8>, MediumError> {
        let path = self.object_path(hash);
        let metadata = fs::metadata(&path).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => MediumError::MissingObject { hash: *hash },
            _ => MediumError::Io {
                op: "stat object",
                path: path.display().to_string(),
                source: e,
            },
        })?;
        if metadata.len() > MAX_OBJECT_BYTES {
            return Err(MediumError::Format(engine_core::SaveError::Limit {
                section: "object-store",
                kind: "byte-length",
                value: metadata.len(),
                max: MAX_OBJECT_BYTES,
            }));
        }
        let bytes = fs::read(&path).map_err(|e| MediumError::Io {
            op: "read object",
            path: path.display().to_string(),
            source: e,
        })?;
        if hash_object(&bytes) != *hash {
            return Err(MediumError::ReferencedObjectCorrupt { hash: *hash });
        }
        Ok(bytes)
    }
}

impl ChainReads for LocalStoreReader<'_> {
    fn read_record_bytes(&mut self, id: &Hash) -> Result<Vec<u8>, MediumError> {
        self.instrumentation.record_reads += 1;
        self.load_referenced(id)
    }

    fn read_object_bytes(&mut self, hash: &Hash) -> Result<Vec<u8>, MediumError> {
        self.instrumentation.whole_object_reads += 1;
        let bytes = self.load_referenced(hash)?;
        self.instrumentation.whole_object_bytes += bytes.len() as u64;
        Ok(bytes)
    }

    fn record_present(&mut self, id: &Hash) -> Result<bool, MediumError> {
        self.instrumentation.presence_probes += 1;
        Ok(self.object_path(id).is_file())
    }

    // (T29, commit 2) The mirror of the local adapter's streaming
    // pair: the unverified prefix read and the streaming verified
    // read (the leading window + the positioned continuation through
    // the core driver — the adapter's own shape).
    fn read_object_prefix(&mut self, hash: &Hash, len: usize) -> Result<Vec<u8>, MediumError> {
        self.instrumentation.prefix_read_calls += 1;
        let path = self.object_path(hash);
        let metadata = fs::metadata(&path).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => MediumError::MissingObject { hash: *hash },
            _ => MediumError::Io {
                op: "stat object",
                path: path.display().to_string(),
                source: e,
            },
        })?;
        if metadata.len() > MAX_OBJECT_BYTES {
            return Err(MediumError::Format(engine_core::SaveError::Limit {
                section: "object-store",
                kind: "byte-length",
                value: metadata.len(),
                max: MAX_OBJECT_BYTES,
            }));
        }
        let mut file = fs::File::open(&path).map_err(|e| MediumError::Io {
            op: "read object",
            path: path.display().to_string(),
            source: e,
        })?;
        let want = len.min(usize::try_from(metadata.len()).unwrap_or(len));
        let mut buffer = vec![0u8; want];
        let mut read = 0usize;
        while read < buffer.len() {
            let n =
                io::Read::read(&mut file, &mut buffer[read..]).map_err(|e| MediumError::Io {
                    op: "read object",
                    path: path.display().to_string(),
                    source: e,
                })?;
            if n == 0 {
                buffer.truncate(read);
                break;
            }
            read += n;
        }
        Ok(buffer)
    }

    fn verify_object_streaming(
        &mut self,
        hash: &Hash,
    ) -> Result<crate::chain_walk::VerifiedPrefix, MediumError> {
        self.instrumentation.stream_verified_calls += 1;
        let leading = self.read_object_prefix(hash, crate::chain_walk::STREAM_CHUNK_BYTES)?;
        let path = self.object_path(hash);
        let mut file = fs::File::open(&path).map_err(|e| MediumError::Io {
            op: "read object",
            path: path.display().to_string(),
            source: e,
        })?;
        io::Seek::seek(&mut file, io::SeekFrom::Start(leading.len() as u64)).map_err(|e| {
            MediumError::Io {
                op: "seek object",
                path: path.display().to_string(),
                source: e,
            }
        })?;
        let result = crate::chain_walk::verify_streamed_object(hash, leading, |buf| {
            let mut read = 0usize;
            while read < buf.len() {
                let n =
                    io::Read::read(&mut file, &mut buf[read..]).map_err(|e| MediumError::Io {
                        op: "read object",
                        path: path.display().to_string(),
                        source: e,
                    })?;
                if n == 0 {
                    break;
                }
                read += n;
            }
            Ok(read)
        })?;
        self.instrumentation.stream_verified_bytes += result.total_len;
        Ok(result)
    }
}

/// (T29) The sqlite store's reader mirror — the rows' own
/// transaction-scoped load semantics (the row loads by hash with the
/// referenced-corruption escalation; `record_exists` as the presence
/// probe), instrumented.
struct SqliteStoreReader<'a> {
    conn: &'a Connection,
    instrumentation: ReaderInstrumentation,
}

impl SqliteStoreReader<'_> {
    fn load_row(
        &self,
        sql: &'static str,
        op: &'static str,
        hash: &Hash,
    ) -> Result<Vec<u8>, MediumError> {
        let bytes: Option<Vec<u8>> = self
            .conn
            .query_row(sql, params![hash.as_bytes()], |row| row.get(0))
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(MediumError::Sqlite {
                    op,
                    detail: other.to_string(),
                }),
            })?;
        let Some(bytes) = bytes else {
            return Err(MediumError::MissingObject { hash: *hash });
        };
        if hash_object(&bytes) != *hash {
            return Err(MediumError::ReferencedObjectCorrupt { hash: *hash });
        }
        Ok(bytes)
    }
}

impl ChainReads for SqliteStoreReader<'_> {
    fn read_record_bytes(&mut self, id: &Hash) -> Result<Vec<u8>, MediumError> {
        self.instrumentation.record_reads += 1;
        self.load_row(
            "SELECT bytes FROM records WHERE hash = ?1",
            "read record",
            id,
        )
    }

    fn read_object_bytes(&mut self, hash: &Hash) -> Result<Vec<u8>, MediumError> {
        self.instrumentation.whole_object_reads += 1;
        let bytes = self.load_row(
            "SELECT bytes FROM objects WHERE hash = ?1",
            "read object",
            hash,
        )?;
        self.instrumentation.whole_object_bytes += bytes.len() as u64;
        Ok(bytes)
    }

    fn record_present(&mut self, id: &Hash) -> Result<bool, MediumError> {
        self.instrumentation.presence_probes += 1;
        match self.conn.query_row(
            "SELECT 1 FROM records WHERE hash = ?1",
            params![id.as_bytes()],
            |_| Ok(()),
        ) {
            Ok(()) => Ok(true),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
            Err(error) => Err(MediumError::Sqlite {
                op: "record exists",
                detail: error.to_string(),
            }),
        }
    }

    // (T29, commit 2) The mirror of the sqlite adapter's streaming
    // pair: the substr prefix read and the length-bounded window
    // continuation through the core driver (the adapter's own shape —
    // the pinned rusqlite's incremental blob I/O is feature-gated
    // off; the substr substrate is the design's sanctioned fallback).
    fn read_object_prefix(&mut self, hash: &Hash, len: usize) -> Result<Vec<u8>, MediumError> {
        self.instrumentation.prefix_read_calls += 1;
        let bytes: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT substr(bytes, 1, ?2) FROM objects WHERE hash = ?1",
                params![hash.as_bytes(), len as i64],
                |row| row.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(MediumError::Sqlite {
                    op: "read object",
                    detail: other.to_string(),
                }),
            })?;
        bytes.ok_or(MediumError::MissingObject { hash: *hash })
    }

    fn verify_object_streaming(
        &mut self,
        hash: &Hash,
    ) -> Result<crate::chain_walk::VerifiedPrefix, MediumError> {
        self.instrumentation.stream_verified_calls += 1;
        let leading = self.read_object_prefix(hash, crate::chain_walk::STREAM_CHUNK_BYTES)?;
        let total: u64 = self
            .conn
            .query_row(
                "SELECT length(bytes) FROM objects WHERE hash = ?1",
                params![hash.as_bytes()],
                |row| {
                    let len: i64 = row.get(0)?;
                    Ok(u64::try_from(len).unwrap_or(0))
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => MediumError::MissingObject { hash: *hash },
                other => MediumError::Sqlite {
                    op: "read object",
                    detail: other.to_string(),
                },
            })?;
        let mut offset = leading.len() as u64;
        let conn = self.conn;
        let result = crate::chain_walk::verify_streamed_object(hash, leading, |buf| {
            if offset >= total {
                return Ok(0);
            }
            let window: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT substr(bytes, ?2, ?3) FROM objects WHERE hash = ?1",
                    params![hash.as_bytes(), offset as i64 + 1, buf.len() as i64],
                    |row| row.get(0),
                )
                .map_err(|e| MediumError::Sqlite {
                    op: "read object",
                    detail: e.to_string(),
                })?;
            let Some(window) = window else {
                return Ok(0);
            };
            let read = window.len().min(buf.len());
            buf[..read].copy_from_slice(&window[..read]);
            offset += read as u64;
            Ok(read)
        })?;
        self.instrumentation.stream_verified_bytes += result.total_len;
        Ok(result)
    }
}

/// Runs the production cold walk over the local store's mirror
/// reader and computes the census — with the mirror-vs-production
/// summary equality asserted (the rows ride only if the mirror
/// observes exactly what the provider's own walk observes).
/// `largest_whole_read_object_bytes` is the caller-measured largest
/// object the streaming walk holds WHOLE (roots/metadata only).
fn census_over_local(
    label: &str,
    case: &Case,
    local: &LocalFileMedium,
    head: Hash,
    largest_whole_read_object_bytes: u64,
    dependency_edges: u64,
) -> chain_walk::walk_census::WalkCensus {
    let local_root = case.local_root();
    let mut reader = LocalStoreReader {
        root: &local_root,
        instrumentation: ReaderInstrumentation::default(),
    };
    let walked = chain_walk::walk_chain_excluding(&mut reader, head, &no_exclusion())
        .unwrap_or_else(|e| panic!("the {label} mirror walk must succeed: {e:?}"));
    let production = local.walk_differential_cold(head, &no_exclusion()).unwrap();
    assert_eq!(
        format!("{:?}", walked.summary),
        format!("{:?}", production.0),
        "the {label} mirror walk must observe the production walk's summary"
    );
    let instrumentation = reader.instrumentation;
    let census =
        chain_walk::walk_census::census(&walked, largest_whole_read_object_bytes, dependency_edges);
    println!(
        "t29 census ({label}, local): {} whole reads ({} B), {} streaming verifies \
         ({} B), {} record reads — {census:?}",
        instrumentation.whole_object_reads,
        instrumentation.whole_object_bytes,
        instrumentation.stream_verified_calls,
        instrumentation.stream_verified_bytes,
        instrumentation.record_reads
    );
    census
}

/// The sqlite twin of [`census_over_local`].
fn census_over_sqlite(
    label: &str,
    sqlite: &SqliteMedium,
    head: Hash,
    largest_whole_read_object_bytes: u64,
    dependency_edges: u64,
) -> chain_walk::walk_census::WalkCensus {
    let mut reader = SqliteStoreReader {
        conn: sqlite.walk_differential_conn(),
        instrumentation: ReaderInstrumentation::default(),
    };
    let walked = chain_walk::walk_chain_excluding(&mut reader, head, &no_exclusion())
        .unwrap_or_else(|e| panic!("the {label} mirror walk must succeed: {e:?}"));
    let production = sqlite
        .walk_differential_cold(head, &no_exclusion())
        .unwrap();
    assert_eq!(
        format!("{:?}", walked.summary),
        format!("{:?}", production.0),
        "the {label} mirror walk must observe the production walk's summary"
    );
    let instrumentation = reader.instrumentation;
    let census =
        chain_walk::walk_census::census(&walked, largest_whole_read_object_bytes, dependency_edges);
    println!(
        "t29 census ({label}, sqlite): {} whole reads ({} B), {} streaming verifies \
         ({} B), {} record reads — {census:?}",
        instrumentation.whole_object_reads,
        instrumentation.whole_object_bytes,
        instrumentation.stream_verified_calls,
        instrumentation.stream_verified_bytes,
        instrumentation.record_reads
    );
    census
}

/// The census ladder's plain shape: F(0) → N−1 regulars — every
/// record referencing the shared object A (every third also B:
/// C = 2), every odd regular keyed (K = (N−1)/2), every 4th
/// depending on its grandparent (the returned edge count). All
/// populations scale exactly with N at fixed shape.
fn census_plain_chain(n: u64) -> (Chain, u64) {
    let object_a = plain_object(b"t29-census-a");
    let object_b = plain_object(b"t29-census-b");
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-pc", 0)
        .with_content_refs(vec![object_a.hash()]);
    let mut steps = vec![
        Step::Object(object_a.clone()),
        Step::Object(object_b.clone()),
        Step::Record(founding.clone()),
    ];
    let mut ids = vec![founding.id()];
    let mut parent = founding.id();
    let mut edges = 0u64;
    for i in 1..n {
        let mut record =
            RevisionRecord::regular(parent, i - 1, i).with_content_refs(if i % 3 == 0 {
                vec![object_a.hash(), object_b.hash()]
            } else {
                vec![object_a.hash()]
            });
        if i % 2 == 1 {
            record = record.with_idempotency_key(format!("t29-pk{i}").into_bytes());
        }
        if i % 4 == 0 {
            record = record.with_depends_on(vec![ids[(i - 2) as usize]]);
            edges += 1;
        }
        parent = record.id();
        ids.push(record.id());
        steps.push(Step::Record(record));
    }
    steps.push(Step::Head(parent));
    (
        Chain {
            steps,
            head: parent,
            ids,
        },
        edges,
    )
}

/// The census ladder's reclaim shape: F(0) → r1..rm (epochs 1..m,
/// every other keyed) → SK → MK (the metadata indexing the whole
/// range, self-skip to F); the range records and SK are physically
/// absent — the PP-B3 gap — so the walk establishes N = 2 records
/// with R = m reclaimed entries and K = m/2 reclaimed keys. The
/// reclaimed population scales exactly with m. Returns the chain and
/// the store's largest object (the metadata object — the caller's
/// measured o_max input).
fn census_reclaim_chain(m: u64) -> (Chain, u64) {
    let object = plain_object(b"t29-census-r");
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-rc", 0)
        .with_content_refs(vec![object.hash()]);
    let mut steps = vec![Step::Object(object.clone()), Step::Record(founding.clone())];
    let mut parent = founding.id();
    let mut index = Vec::with_capacity(m as usize);
    let mut range_ids = Vec::with_capacity(m as usize);
    for i in 1..=m {
        let mut record =
            RevisionRecord::regular(parent, i - 1, i).with_content_refs(vec![object.hash()]);
        if i % 2 == 1 {
            record = record.with_idempotency_key(format!("t29-rk{i}").into_bytes());
        }
        index.push(ReclaimedIndexEntry::new(
            record.idempotency_key.clone(),
            record.id(),
            i,
        ));
        parent = record.id();
        range_ids.push(record.id());
        steps.push(Step::Record(record));
    }
    let sk = RevisionRecord::regular(parent, m, m + 1).with_content_refs(vec![object.hash()]);
    let empty_range: [&[u8]; 0] = [];
    let (digest, seed) = compute_range_digest(&founding.id(), &empty_range);
    let metadata = CheckpointMetadata::new(
        sk.id(),
        founding.id(),
        vec![SkipEntry::self_skip(founding.id())],
        index,
        digest,
        seed,
    );
    let meta_object = StoredObject::new(metadata.canonical_bytes()).unwrap();
    let reclaim_largest = meta_object.bytes().len() as u64;
    let mk =
        RevisionRecord::regular(sk.id(), m + 1, m + 2).with_content_refs(vec![meta_object.hash()]);
    steps.push(Step::Object(meta_object));
    steps.push(Step::Record(sk.clone()));
    steps.push(Step::Record(mk.clone()));
    // The physical PP-B3 gap: the whole range and SK vanish (the
    // shared content object stays — F still references it).
    for id in range_ids {
        steps.push(Step::RemoveRecord(id));
    }
    steps.push(Step::RemoveRecord(sk.id()));
    steps.push(Step::Head(mk.id()));
    (
        Chain {
            steps,
            head: mk.id(),
            ids: vec![founding.id(), mk.id()],
        },
        reclaim_largest,
    )
}

// ==================================================== (T29) the census tests

/// (T29, commit 1) THE RESIDENCY CENSUS at the R07 ladder — the
/// design §6.1's measured obligation: the §2 estimator rows over
/// really-walked chains at N ∈ {512, 2048, 8192} (the R07 ladder),
/// BOTH providers, the small-object plain shape (N, C, K, D scaling
/// at fixed shape) and the reclaim shape (R and the reclaimed-key
/// share of K scaling). The structural claims pinned: the
/// cursor-resident row is linear in N (and in R on the reclaim
/// shape); the per-validation churn row is the three-clone cost —
/// linear, the commit-2 teeth row that must pin ZERO after the
/// borrow fix; the cold-open wall-time rows are printed
/// (pod-observational, serial per provider, the T39 discipline).
/// The design's §2.1 worked example (≈2.7 MiB at N = 8,192 with
/// C = 1.5N, K = N/2, R = N/4) is compared against in the printed
/// row — the ladder's fixed shape differs by design (the census
/// measures THIS shape's populations; the example was an estimate
/// under its own).
#[test]
fn walker_residency_census_rows_at_the_r07_ladder() {
    let case = Case::new("t29-ladder");
    // (T29, commit 2) The plain ladder holds NOTHING whole under the
    // streaming mechanics — its objects are plain content references
    // (no roots, no metadata): the largest whole-read input is zero.
    let plain_largest_whole_read = 0u64;
    let mut plain_rows: Vec<(u64, u64, u64)> = Vec::new();
    for n in [512u64, 2048, 8192] {
        // Every scale builds from a wiped case root (the census
        // populations must be exactly this scale's, never the prior
        // scale's residue).
        let _ = fs::remove_dir_all(&case.root);
        fs::create_dir_all(&case.root).unwrap();
        let (chain, edges) = census_plain_chain(n);
        let local_root = case.local_root();
        {
            let scratch = LocalFileMedium::open(&local_root).unwrap();
            run_local_steps(&scratch, &local_root, &chain.steps);
        }
        {
            let scratch = SqliteMedium::open(case.db_path()).unwrap();
            run_sqlite_steps(&scratch, &chain.steps);
        }
        // The timed cold opens (serial, one provider at a time).
        let started = Instant::now();
        let local = LocalFileMedium::open(&local_root).unwrap();
        let local_open = started.elapsed();
        let started = Instant::now();
        let sqlite = SqliteMedium::open(case.db_path()).unwrap();
        let sqlite_open = started.elapsed();
        // The populations this shape defines (measured by the walk).
        let keyed = (1..n).filter(|i| i % 2 == 1).count() as u64;
        let expected_key_bytes: u64 = (1..n)
            .filter(|i| i % 2 == 1)
            .map(|i| 72 + format!("t29-pk{i}").len() as u64)
            .sum();
        let local_census = census_over_local(
            "plain",
            &case,
            &local,
            chain.head,
            plain_largest_whole_read,
            edges,
        );
        let sqlite_census = census_over_sqlite(
            "plain",
            &sqlite,
            chain.head,
            plain_largest_whole_read,
            edges,
        );
        for (label, census) in [("local", local_census), ("sqlite", sqlite_census)] {
            assert_eq!(census.records, n, "the {label} population at {n}");
            assert_eq!(census.content_objects, 2, "the {label} population at {n}");
            assert_eq!(census.keys, keyed, "the {label} population at {n}");
            assert_eq!(census.reclaimed_entries, 0, "the {label} population at {n}");
            assert_eq!(
                census.dependency_edges, edges,
                "the {label} population at {n}"
            );
            // The estimator rows over those populations.
            assert_eq!(
                census.reachable_bytes,
                80 * (n + 2),
                "the {label} reachable row at {n}"
            );
            assert_eq!(
                census.record_ids_bytes,
                80 * n,
                "the {label} record-ids row at {n}"
            );
            assert_eq!(
                census.seen_keys_bytes, expected_key_bytes,
                "the {label} seen-keys row at {n}"
            );
            // (T29, commit 2 — the movement's cause: the borrow fix)
            // THE CHURN ROW PINS ZERO: the whole-read form's three
            // cursor-set deep clones per divergent-head validation
            // are deleted (the commit-1 rows were 80·N + the key
            // bytes, linear in N: 61,641 / 247,253 / 990,677 B).
            assert_eq!(
                census.validation_churn_bytes, 0,
                "the {label} churn row at {n} (the clones deleted)"
            );
            // (T29, commit 2 — the movement's cause: the streaming
            // rewire) The object-peak row: ONE reused ~64 KiB stream
            // window, no whole holds on the plain shape (the
            // commit-1 row was 2·o_max).
            assert_eq!(
                census.object_buffer_peak_bytes,
                64 * 1024,
                "the {label} object-peak row at {n} (the streaming window)"
            );
        }
        plain_rows.push((
            n,
            local_census.cursor_resident_bytes,
            sqlite_census.cursor_resident_bytes,
        ));
        println!(
            "t29 census row: N {n} plain — local open {local_open:?}, sqlite open \
             {sqlite_open:?}, cursor-resident {} B (~{} B/record) vs the design's \
             worked-example estimate 2.7 MiB at its own C=1.5N/K=N/2/R=N/4 shape",
            local_census.cursor_resident_bytes,
            local_census.cursor_resident_bytes / n
        );
    }
    // THE STRUCTURAL CLAIM: linearity in N at fixed shape — the 4x
    // ladder steps must scale the cursor-resident row 4x (the
    // estimator is exact over exact populations, so the band is
    // tight; the R07 census used the same band shape).
    for (small, large) in plain_rows.windows(2).map(|w| (w[0], w[1])) {
        let ratio = large.1 as f64 / small.1 as f64;
        assert!(
            ratio > 3.9 && ratio < 4.1,
            "the 512→2048→8192 cursor-resident ratio {ratio:.3} must track the 4x ladder"
        );
        let sqlite_ratio = large.2 as f64 / small.2 as f64;
        assert!(
            sqlite_ratio > 3.9 && sqlite_ratio < 4.1,
            "the sqlite cursor-resident ratio {sqlite_ratio:.3} must track the 4x ladder"
        );
    }
    // The reclaim shape: R (and the reclaimed-key share of K) scales.
    let mut reclaim_rows: Vec<(u64, u64)> = Vec::new();
    for m in [512u64, 2048, 8192] {
        let (chain, reclaim_largest) = census_reclaim_chain(m);
        let (local, sqlite) = opened_pair(&case, &chain.steps);
        let keyed = (1..=m).filter(|i| i % 2 == 1).count() as u64;
        let local_census =
            census_over_local("reclaim", &case, &local, chain.head, reclaim_largest, 0);
        let sqlite_census = census_over_sqlite("reclaim", &sqlite, chain.head, reclaim_largest, 0);
        for census in [local_census, sqlite_census] {
            assert_eq!(census.records, 2, "the reclaim walk walks MK and F at {m}");
            assert_eq!(
                census.reclaimed_entries, m,
                "the reclaimed population at {m}"
            );
            assert_eq!(census.keys, keyed, "the reclaimed-key population at {m}");
            assert_eq!(
                census.reclaimed_index_bytes,
                88 * m,
                "the reclaimed-index row at {m}"
            );
            // (T29, commit 2 — the movement's cause: the borrow fix)
            // THE CHURN ROW PINS ZERO on the reclaim shape too (the
            // commit-1 rows carried the cloned reclaimed index:
            // 65,897 / 263,797 / 1,056,373 B, linear in R).
            assert_eq!(
                census.validation_churn_bytes, 0,
                "the reclaim churn row at {m} (the clones deleted)"
            );
            // (T29, commit 2 — the movement's cause: the streaming
            // rewire) The object-peak row: the stream window PLUS the
            // one legitimate whole hold (the checkpoint metadata
            // object — the reclaim walk's only root-class read).
            assert_eq!(
                census.object_buffer_peak_bytes,
                64 * 1024 + reclaim_largest,
                "the reclaim object-peak row at {m}"
            );
        }
        reclaim_rows.push((m, local_census.cursor_resident_bytes));
    }
    for (small, large) in reclaim_rows.windows(2).map(|w| (w[0], w[1])) {
        let ratio = large.1 as f64 / small.1 as f64;
        assert!(
            ratio > 3.9 && ratio < 4.1,
            "the reclaimed cursor-resident ratio {ratio:.3} must track the 4x ladder"
        );
    }
}

/// (T29) THE OBJECT-PEAK ROW at the large-object shape — the design
/// §2.2's dominant term, measured BEFORE and AFTER the streaming
/// rewire: with L5's derived frame maximum (~5.25 MiB) as the
/// stand-in for a large session-snapshot content object, a record
/// referencing TWO such objects held the first whole across the
/// second's whole read under the whole-read mechanics (the commit-1
/// rows: whole-read volume 16,515,116 B, object-buffer peak
/// 11,010,048 B = 2·o_max against a cursor-resident of 720 B — the
/// transient exceeding the resident it builds). THE COMMIT-2
/// MEASURED OBLIGATION, pinned here: ZERO plain-object whole loads
/// at this shape (the whole-read rows empty; every content reference
/// STREAMS — the streamed-volume row carrying the same bytes read,
/// never held), and the object-buffer peak row showing the
/// ~64 KiB stream window + root terms (no root at this shape: the
/// plain window alone).
#[test]
fn walker_census_object_peak_at_the_large_object_shape() {
    let case = Case::new("t29-peak");
    // ~5.25 MiB objects (the design's L5 frame-max stand-in).
    const L5_MAX_BYTES: usize = 5 * 1024 * 1024 + 256 * 1024;
    const FRAMING_BYTES: usize = 8 + "sektor7/save/1".len() + 8;
    let payload = L5_MAX_BYTES - FRAMING_BYTES;
    let big_a = plain_object(&vec![0x5a; payload]);
    let big_b = plain_object(&vec![0xa5; payload]);
    let small = plain_object(b"t29-peak-small");
    let founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-peak", 0)
        .with_content_refs(vec![small.hash()]);
    let r1 = RevisionRecord::regular(founding.id(), 0, 1)
        .with_content_refs(vec![big_a.hash(), big_b.hash()]);
    let r2 = RevisionRecord::regular(r1.id(), 1, 2).with_content_refs(vec![big_a.hash()]);
    let steps = vec![
        Step::Object(small),
        Step::Object(big_a.clone()),
        Step::Object(big_b.clone()),
        Step::Record(founding),
        Step::Record(r1),
        Step::Record(r2.clone()),
        Step::Head(r2.id()),
    ];
    let (local, sqlite) = opened_pair(&case, &steps);
    let o_max = big_a.bytes().len() as u64;
    assert_eq!(
        big_b.bytes().len() as u64,
        o_max,
        "the two large objects match"
    );
    // The production cross-provider byte-identity stands at this
    // shape too (the summaries agree, content objects = 3).
    let observation = compare_cold("l5-max", &local, &sqlite, r2.id(), &no_exclusion());
    let summary = observation.summary.as_deref().unwrap();
    assert!(summary.contains("records: 3") && summary.contains("content_objects: 3"));
    // The measured read rows, both providers.
    let local_root = case.local_root();
    let mut local_reader = LocalStoreReader {
        root: &local_root,
        instrumentation: ReaderInstrumentation::default(),
    };
    let walked =
        chain_walk::walk_chain_excluding(&mut local_reader, r2.id(), &no_exclusion()).unwrap();
    let local_instrumentation = local_reader.instrumentation;
    let mut sqlite_reader = SqliteStoreReader {
        conn: sqlite.walk_differential_conn(),
        instrumentation: ReaderInstrumentation::default(),
    };
    let sqlite_walked =
        chain_walk::walk_chain_excluding(&mut sqlite_reader, r2.id(), &no_exclusion()).unwrap();
    assert_eq!(
        format!("{:?}", walked.summary),
        format!("{:?}", sqlite_walked.summary),
        "the two providers' mirror walks agree at the large-object shape"
    );
    // THE MEASURED OBLIGATION: every content reference STREAMS — ZERO
    // plain-object whole loads (the commit-1 whole-read rows were 4
    // calls / 16,515,116 B; the whole-read form now serves ONLY the
    // root-class domains, absent at this shape), the streamed volume
    // carrying the same four reference verifications (founding's
    // small + r1's two + r2's repeat of A — the repeat-read row's
    // I/O cost, its byte residency gone).
    let small_len = plain_object(b"t29-peak-small").bytes().len() as u64;
    for (provider, instrumentation) in [
        ("local", local_instrumentation),
        ("sqlite", sqlite_reader.instrumentation),
    ] {
        assert_eq!(
            instrumentation.whole_object_reads, 0,
            "the {provider} whole-read calls at the large-object shape (streaming)"
        );
        assert_eq!(
            instrumentation.whole_object_bytes, 0,
            "the {provider} whole-read volume at the large-object shape (streaming)"
        );
        assert_eq!(
            instrumentation.stream_verified_calls, 4,
            "the {provider} streaming verifications at the large-object shape"
        );
        assert!(
            instrumentation.stream_verified_bytes >= 3 * o_max + small_len,
            "the {provider} streamed volume {} B carries the same reference bytes, \
             read never held",
            instrumentation.stream_verified_bytes
        );
    }
    // The census's peak row: the ~64 KiB stream window, no whole hold
    // at this shape (the commit-1 row was 11,010,048 B = 2·o_max).
    let census = chain_walk::walk_census::census(&walked, 0, 0);
    assert_eq!(census.object_buffer_peak_bytes, 64 * 1024);
    assert!(
        census.walk_transient_peak_bytes < 200 * 1024,
        "the walk-transient peak {} B must be the stream terms, not the object \
         bytes (the commit-1 row was {} B)",
        census.walk_transient_peak_bytes,
        2 * o_max + census.walked_bytes + census.positions_bytes
    );
    println!(
        "t29 census row: the large-object shape AFTER the streaming rewire (o_max \
         {o_max} B) — whole-read volume 0 B, streamed volume {} B local / {} B \
         sqlite, object-buffer peak {} B (the stream window; was 2·o_max = {} B), \
         walk-transient peak {} B vs cursor-resident {} B",
        local_instrumentation.stream_verified_bytes,
        sqlite_reader.instrumentation.stream_verified_bytes,
        census.object_buffer_peak_bytes,
        2 * o_max,
        census.walk_transient_peak_bytes,
        census.cursor_resident_bytes
    );
}

// ============================================== (T29) the five oracle axes

/// (T29, axis 1) THE DISPATCH STATE SET — every `(observed, trusted)`
/// combination the validation dispatch enumerates, with its accepted
/// cursor state or refusal AND the cursor's survival/discard
/// disposition pinned per arm (byte-identical across providers, the
/// battery's standing discipline). The arms: (None, None) nothing
/// observed; (Some(cursor_head), Some) the O(0) verified head with
/// the cursor fingerprint UNCHANGED and the walk counters zeroed;
/// (Some(descendant), Some) the absorbed suffix with the cursor
/// ADVANCED; (Some(divergent rewrite), Some) the divergence refusal
/// with the cursor KEPT (the divergence arm never discards);
/// (Some(corrupt suffix), Some) the sound invalidation with the
/// cursor DISCARDED and the cold walk re-deriving the refusal;
/// (Some(id), None) the fresh cold establishment; (None, Some) the
/// vanished head — the fail-closed divergence, cursor KEPT.
#[test]
fn dispatch_states_enumerate_the_observed_trusted_grid_identically() {
    let case = Case::new("t29-states");
    // (None, None): nothing observed, nothing trusted.
    {
        let (mut local, mut sqlite) = opened_pair(&case, &[]);
        let observation = compare_head("state-none-none", &mut local, &mut sqlite);
        assert_eq!(observation.observed, Some(None));
        assert_eq!(observation.cursor, None);
        assert!(observation.error.is_none());
    }
    // (Some(cursor_head), Some): the O(0) verified-head arm.
    {
        let (chain, _object) = plain_chain();
        let (mut local, mut sqlite) = opened_pair(&case, &chain.steps);
        let before = normalize_cursor(local.walk_differential_cursor());
        let observation = compare_head("state-verified-head", &mut local, &mut sqlite);
        assert_eq!(observation.observed, Some(Some(chain.head)));
        assert_eq!(
            observation.cursor, before,
            "the O(0) arm never moves the cursor"
        );
        assert_eq!(
            local.walk_stats().last_append_walked,
            0,
            "the local O(0) accounting"
        );
        assert_eq!(
            sqlite.walk_stats().last_append_walked,
            0,
            "the sqlite O(0) accounting"
        );
    }
    // (Some(descendant), Some): the absorbed external suffix.
    {
        let (chain, object) = plain_chain();
        let (mut local, mut sqlite) = opened_pair(&case, &chain.steps);
        let before = local.walk_differential_cursor();
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
        let observation = compare_head("state-absorbed", &mut local, &mut sqlite);
        assert_eq!(observation.observed, Some(Some(r6.id())));
        assert_ne!(
            observation.cursor, before,
            "the absorbed suffix advances the cursor"
        );
        assert_eq!(
            local.walk_stats().last_append_walked,
            1,
            "the local O(suffix) accounting"
        );
        assert_eq!(
            sqlite.walk_stats().last_append_walked,
            1,
            "the sqlite O(suffix) accounting"
        );
    }
    // (Some(divergent rewrite), Some): the refusal keeps the cursor.
    {
        let (chain, object) = plain_chain();
        let (mut local, mut sqlite) = opened_pair(&case, &chain.steps);
        let before = normalize_cursor(local.walk_differential_cursor());
        let foreign = plain_object(b"t29-state-foreign");
        let foreign_founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-st", 0)
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
        let observation = compare_head("state-divergent", &mut local, &mut sqlite);
        head_refusal(&observation, "divergence");
        assert_eq!(
            observation.cursor, before,
            "the divergence arm never discards the trusted cursor"
        );
    }
    // (Some(corrupt suffix), Some): the sound invalidation discards it.
    {
        let (chain, _object) = plain_chain();
        let (mut local, mut sqlite) = opened_pair(&case, &chain.steps);
        let fresh = plain_object(b"t29-state-corrupt");
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
        let observation = compare_head("state-invalidation", &mut local, &mut sqlite);
        head_refusal(&observation, "ReferencedObjectCorrupt");
        assert_eq!(
            observation.cursor, None,
            "the sound invalidation discards the cursor on both"
        );
    }
    // (Some(id), None): the fresh cold establishment.
    {
        let (mut local, mut sqlite) = opened_pair(&case, &[]);
        {
            let object = plain_object(b"t29-state-founding");
            let founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-sf", 0)
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
        let observation = compare_head("state-cold-establishment", &mut local, &mut sqlite);
        assert!(observation.error.is_none(), "{observation:?}");
        assert!(
            observation.observed.is_some_and(|head| head.is_some()),
            "the external founding is observed: {observation:?}"
        );
        assert!(
            observation.cursor.is_some(),
            "the cold establishment installs the cursor on both"
        );
    }
    // (None, Some): the vanished head — divergence, cursor kept.
    {
        let (chain, _object) = plain_chain();
        let (mut local, mut sqlite) = opened_pair(&case, &chain.steps);
        let before = normalize_cursor(local.walk_differential_cursor());
        fs::remove_file(case.local_root().join(HEAD_FILE)).unwrap();
        sqlite
            .walk_differential_conn()
            .execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('head', x'')",
                [],
            )
            .unwrap();
        let observation = compare_head("state-vanished-head", &mut local, &mut sqlite);
        head_refusal(&observation, "divergence");
        assert_eq!(
            observation.cursor, before,
            "the vanished-head divergence keeps the trusted cursor"
        );
    }
}

/// (T29, axis 2) THE FULL FROZEN-RULE CLASSIFICATION MATRIX per walk
/// form — every rule in the frozen table fired with an expected-rule
/// assertion in the forms that can produce it, the cells the corpus
/// cannot construct or the walk form cannot produce documented in
/// place. The battery above already fires (cold) `epoch-regression`,
/// `duplicate-idempotency-key` (on-chain and index-collision),
/// `depends-on-unresolvable`, `cycle` (the reachable form),
/// `kind-incoherent`, `checkpoint-index-incoherent` (the two-MK
/// duplicate), `MissingObject`, `Format`, `ReferencedObjectCorrupt`;
/// (suffix) `epoch-regression`, `divergence`,
/// `ReferencedObjectCorrupt`, `duplicate-idempotency-key`,
/// `depends-on-unresolvable`, `Format`, `MissingObject`,
/// `composite-closure-verification`; (excluding) `MissingObject`.
/// THE MISSING CELLS THIS TEST FIRES: (cold)
/// `composite-closure-verification` and the manifest-decode
/// `Format`; (excluding) `epoch-regression` (the exclusion-routed
/// jump to a not-strictly-earlier target), `cycle` (the
/// exclusion-routed revisit), and `checkpoint-index-incoherent`.
/// The impossible cells, honestly: (cold) `divergence` — the cold
/// walk has no trusted counterpart to diverge from; (any)
/// `founding-missing` — a parentless regular record refuses at the
/// record DECODE (the kind/parent coherence check), so the walk's
/// own branch is decode-guarded defensive code no validly-encoded
/// store can reach (the battery's parentless-regular decode
/// differential is the reachable form); (suffix) `cycle` — a literal
/// parent cycle is a hash fixed point, unconstructible, and the
/// suffix walk has no skip tables (its revisit guard is the same
/// fixed-point barrier); (suffix) `kind-incoherent`,
/// `founding-missing`, `checkpoint-index-incoherent` — the suffix
/// walk checks neither founding coherence (a parentless suffix
/// record is the `divergence` arm) nor merges MK indexes (a foreign
/// MK validates like any other record and is not absorbed).
#[test]
fn frozen_rule_classification_covers_the_constructible_matrix() {
    let case = Case::new("t29-matrix");
    // COLD composite-closure-verification: the SNAP record's /2 root
    // fails closure (the manifest declares HEADER but the chunk's
    // envelope is tagged NEAR) — the suffix corner above is the
    // battery's existing form; the cold form is the missing cell.
    {
        let (reference_root, reference_chunks) = composite_fixture(61);
        let mismatched = composite_manifest::encode_chunk_envelope(TAG_NEAR, b"t29-wrong-tag");
        let reference = composite_manifest::decode(reference_root.bytes()).unwrap();
        let mut entries = Vec::new();
        for entry in &reference.entries {
            if entry.tag == TAG_HEADER {
                entries.push(ManifestEntry {
                    tag: TAG_HEADER,
                    identity: mismatched.hash(),
                    payload_length: b"t29-wrong-tag".len() as u64,
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
        let plain = plain_object(b"t29-matrix-plain");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-mx", 0)
            .with_content_refs(vec![plain.hash()]);
        let r1 = RevisionRecord::regular(founding.id(), 0, 1).with_content_refs(vec![plain.hash()]);
        let snap = RevisionRecord::regular(r1.id(), 1, 2).with_content_refs(vec![root.hash()]);
        let mut steps = vec![
            Step::Object(plain),
            Step::Object(mismatched),
            Step::Object(root),
            Step::Record(founding),
            Step::Record(r1),
        ];
        for chunk in &reference_chunks {
            steps.push(Step::Object(chunk.clone()));
        }
        steps.push(Step::Record(snap.clone()));
        steps.push(Step::Head(snap.id()));
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold(
            "cold-closure-failure",
            &local,
            &sqlite,
            snap.id(),
            &no_exclusion(),
        );
        cold_refusal(&observation, "composite-closure-verification");
    }
    // COLD Format: the /2-framed root whose payload is not a manifest.
    {
        let (chain, _object) = plain_chain();
        let (local, sqlite) = opened_pair(&case, &chain.steps);
        let mut enc = Encoder::new();
        enc.write_str(composite_manifest::SESSION_SNAPSHOT_V2_DOMAIN);
        enc.write_bytes(b"t29-malformed-manifest-body");
        let fake_root = StoredObject::new(enc.into_bytes()).unwrap();
        let snap =
            RevisionRecord::regular(chain.head, 5, 6).with_content_refs(vec![fake_root.hash()]);
        let steps = [
            Step::Object(fake_root),
            Step::Record(snap.clone()),
            Step::Head(snap.id()),
        ];
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold(
            "cold-manifest-format",
            &local,
            &sqlite,
            snap.id(),
            &no_exclusion(),
        );
        cold_refusal(&observation, "Format");
    }
    // EXCLUDING epoch-regression: the EXCLUSION routes the jump (the
    // parent is physically present but audited-retiring), and the
    // skip target sits at the standing record's own epoch — not
    // strictly below — the gap-aware linkage refusal.
    {
        let object = plain_object(b"t29-mx-epoch");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-me", 0)
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
        // NOTE: the base record stays PRESENT — the exclusion makes
        // it virtually absent (the K2-C2B P2 routing).
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
        ];
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold(
            "excluding-epoch-regression",
            &local,
            &sqlite,
            mk.id(),
            &exclusion(&[base_id]),
        );
        cold_refusal(&observation, "epoch-regression");
    }
    // EXCLUDING cycle: the exclusion-routed jump targets an
    // already-walked record (the MK itself).
    {
        let object = plain_object(b"t29-mx-cycle");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-mc", 0)
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
            Step::Record(sk),
            Step::Record(mk.clone()),
            Step::Head(mk.id()),
        ];
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold(
            "excluding-cycle",
            &local,
            &sqlite,
            mk.id(),
            &exclusion(&[base_id]),
        );
        cold_refusal(&observation, "cycle");
    }
    // EXCLUDING checkpoint-index-incoherent: the two-MK duplicate
    // index hash under a nonempty exclusion (the membership-universe
    // merge is exclusion-independent by design — the excluded
    // records join the reclaimed universe through the indexes, and
    // the duplicate refuses identically).
    {
        let object = plain_object(b"t29-mx-two-mk");
        let shared_absent = hash_object(b"t29-mx-absent-revision");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-mt", 0)
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
            "excluding-index-duplicate",
            &local,
            &sqlite,
            mk2.id(),
            &exclusion(&[hash_object(b"t29-mx-unrelated")]),
        );
        cold_refusal(&observation, "checkpoint-index-incoherent");
    }
}

/// (T29, axis 3) THE CONFLICT-ORDERING PINS — multi-violation records
/// proving WHICH rule fires FIRST, derived from the shared core's
/// check sequence (per record: the revisit guard → decode → epoch
/// linkage → duplicate key → content references (with the
/// domain/manifest/metadata decodes and the closure verification
/// after the whole refs loop) → parent/founding; the ancestry
/// deferred until the chain is complete; the suffix boundary checked
/// at `until` after the whole suffix). These are NEW teeth: the
/// battery above proves the rules fire, never their precedence.
#[test]
fn conflict_ordering_pins_the_cores_check_sequence() {
    let case = Case::new("t29-order");
    // (a) COLD: a record pair where the LOWER record is both
    // epoch-regressive (against the record above it) and
    // duplicate-keyed — the linkage check at the lower record's read
    // precedes its key check (the design's named ordering example:
    // the regressive-and-duplicate record must refuse the linkage).
    {
        let object = plain_object(b"t29-ord-a");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-oa", 0)
            .with_content_refs(vec![object.hash()])
            .with_idempotency_key(b"t29-ord-key".to_vec());
        let regressive = RevisionRecord::regular(founding.id(), 0, 1)
            .with_content_refs(vec![object.hash()])
            .with_idempotency_key(b"t29-ord-key".to_vec());
        // The head sits at epoch 3 over the regressive record's
        // epoch 1 — the linkage 1+1 ≠ 3 fires at the regressive
        // record's read, before its duplicate key. The sqlite row
        // suppresses the idem column (the row index is an enforcement
        // fast-path; the walk's byte-level rule is the authority).
        let head =
            RevisionRecord::regular(regressive.id(), 2, 2).with_content_refs(vec![object.hash()]);
        let steps = vec![
            Step::Object(object),
            Step::Record(founding),
            Step::RecordIdemSuppressed(regressive),
            Step::Record(head.clone()),
            Step::Head(head.id()),
        ];
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold(
            "order-linkage-vs-key",
            &local,
            &sqlite,
            head.id(),
            &no_exclusion(),
        );
        cold_refusal(&observation, "epoch-regression");
        assert!(
            !observation
                .error
                .as_deref()
                .unwrap()
                .contains("duplicate-idempotency-key"),
            "the duplicate-key violation must NOT fire first: {:?}",
            observation.error
        );
    }
    // (b) COLD: duplicate key beats the unresolvable dependency (the
    // per-record key check runs in-walk; the ancestry is deferred).
    {
        let object = plain_object(b"t29-ord-b");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-ob", 0)
            .with_content_refs(vec![object.hash()])
            .with_idempotency_key(b"t29-ord-key".to_vec());
        let record = RevisionRecord::regular(founding.id(), 0, 1)
            .with_content_refs(vec![object.hash()])
            .with_idempotency_key(b"t29-ord-key".to_vec())
            .with_depends_on(vec![hash_object(b"t29-ord-nope")]);
        let top = RevisionRecord::regular(record.id(), 1, 2).with_content_refs(vec![object.hash()]);
        let steps = vec![
            Step::Object(object),
            Step::Record(founding),
            Step::RecordIdemSuppressed(record),
            Step::Record(top.clone()),
            Step::Head(top.id()),
        ];
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold(
            "order-key-vs-depends",
            &local,
            &sqlite,
            top.id(),
            &no_exclusion(),
        );
        cold_refusal(&observation, "duplicate-idempotency-key");
        assert!(
            !observation
                .error
                .as_deref()
                .unwrap()
                .contains("depends-on-unresolvable"),
            "the deferred ancestry must not fire first: {:?}",
            observation.error
        );
    }
    // (c) COLD: duplicate key beats the corrupt content reference
    // (the key check at the lower record's read precedes its refs
    // loop — the corrupt object is referenced by the LOWER record).
    {
        let object = plain_object(b"t29-ord-c");
        let corrupted = plain_object(b"t29-ord-c-corrupted");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-oc", 0)
            .with_content_refs(vec![object.hash()]);
        let lower = RevisionRecord::regular(founding.id(), 0, 1)
            .with_content_refs(vec![corrupted.hash()])
            .with_idempotency_key(b"t29-ord-key".to_vec());
        let head = RevisionRecord::regular(lower.id(), 1, 2)
            .with_content_refs(vec![object.hash()])
            .with_idempotency_key(b"t29-ord-key".to_vec());
        let steps = vec![
            Step::Object(object),
            Step::Object(corrupted.clone()),
            Step::Record(founding),
            Step::RecordIdemSuppressed(lower),
            Step::Record(head.clone()),
            Step::Head(head.id()),
            Step::CorruptObject(corrupted.hash()),
        ];
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold(
            "order-key-vs-corrupt",
            &local,
            &sqlite,
            head.id(),
            &no_exclusion(),
        );
        cold_refusal(&observation, "duplicate-idempotency-key");
        assert!(
            !observation
                .error
                .as_deref()
                .unwrap()
                .contains("ReferencedObjectCorrupt"),
            "the corrupt reference must not fire first: {:?}",
            observation.error
        );
    }
    // (d) COLD: duplicate key beats the uncovered parent gap (the
    // lower record carries BOTH the duplicate key and the absent
    // parent — its key check precedes its parent probe).
    {
        let object = plain_object(b"t29-ord-d");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-od", 0)
            .with_content_refs(vec![object.hash()])
            .with_idempotency_key(b"t29-ord-key".to_vec());
        let gap =
            RevisionRecord::regular(founding.id(), 0, 1).with_content_refs(vec![object.hash()]);
        let lower = RevisionRecord::regular(gap.id(), 1, 2)
            .with_content_refs(vec![object.hash()])
            .with_idempotency_key(b"t29-ord-key".to_vec());
        let head = RevisionRecord::regular(lower.id(), 2, 3)
            .with_content_refs(vec![object.hash()])
            .with_idempotency_key(b"t29-ord-key".to_vec());
        let gap_id = gap.id();
        let steps = vec![
            Step::Object(object),
            Step::Record(founding),
            Step::Record(gap),
            Step::RecordIdemSuppressed(lower),
            Step::RecordIdemSuppressed(head.clone()),
            Step::Head(head.id()),
            Step::RemoveRecord(gap_id),
        ];
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold(
            "order-key-vs-gap",
            &local,
            &sqlite,
            head.id(),
            &no_exclusion(),
        );
        cold_refusal(&observation, "duplicate-idempotency-key");
        assert!(
            !observation
                .error
                .as_deref()
                .unwrap()
                .contains("MissingObject"),
            "the uncovered gap must not fire first: {:?}",
            observation.error
        );
    }
    // (e) COLD: the epoch linkage beats the corrupt content
    // reference (the linkage error fires at the lower record's read,
    // before its refs loop could load the corrupted object).
    {
        let object = plain_object(b"t29-ord-e");
        let corrupted = plain_object(b"t29-ord-e-corrupted");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-oe", 0)
            .with_content_refs(vec![object.hash()]);
        let regressive =
            RevisionRecord::regular(founding.id(), 0, 1).with_content_refs(vec![corrupted.hash()]);
        // The head at epoch 3 over the regressive epoch 1: the
        // linkage fires at the regressive record's read.
        let head =
            RevisionRecord::regular(regressive.id(), 2, 2).with_content_refs(vec![object.hash()]);
        let steps = vec![
            Step::Object(object),
            Step::Record(founding),
            Step::Record(regressive),
            Step::Record(head.clone()),
            Step::Head(head.id()),
            Step::CorruptObject(corrupted.hash()),
        ];
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_cold(
            "order-linkage-vs-corrupt",
            &local,
            &sqlite,
            head.id(),
            &no_exclusion(),
        );
        cold_refusal(&observation, "epoch-regression");
        assert!(
            !observation
                .error
                .as_deref()
                .unwrap()
                .contains("ReferencedObjectCorrupt"),
            "the corrupt reference must not fire first: {:?}",
            observation.error
        );
    }
    // (f) SUFFIX: the duplicate key beats the bad boundary epoch (the
    // key is checked during the record loop; the boundary at `until`
    // after the whole suffix) — the design's named example. NOTE the
    // honest observability limit this pins around: through the PUBLIC
    // `head()` dispatch a suffix refusal routes through the sound
    // invalidation, whose cold walk re-derives its own rule (a
    // boundary violation re-derives as epoch-regression) — so the
    // suffix walk's OWN precedence is pinned by driving
    // `chain_walk::walk_suffix` directly over the mirror reader with
    // the trusted sets a real dispatch would pass (the same free
    // function the providers' dispatch calls).
    {
        let (chain, object) = plain_chain();
        let (local, sqlite) = opened_pair(&case, &chain.steps);
        let late = RevisionRecord::regular(chain.head, 6, 7)
            .with_content_refs(vec![object.hash()])
            .with_idempotency_key(b"wdiff-k1".to_vec());
        let steps = [
            Step::RecordIdemSuppressed(late.clone()),
            Step::Head(late.id()),
        ];
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let local_root = case.local_root();
        let mut reader = LocalStoreReader {
            root: &local_root,
            instrumentation: ReaderInstrumentation::default(),
        };
        let trusted =
            chain_walk::walk_chain_excluding(&mut reader, chain.head, &no_exclusion()).unwrap();
        let suffix = chain_walk::walk_suffix(
            &mut reader,
            late.id(),
            chain.head,
            trusted.cursor.epoch,
            &trusted.cursor.seen_keys,
            &trusted.cursor.record_ids,
            &trusted.cursor.reclaimed,
        );
        let error = format!("{:?}", suffix.err().unwrap());
        assert!(
            error.contains("duplicate-idempotency-key"),
            "the suffix key precedence: {error}"
        );
        assert!(
            !error.contains("epoch-regression"),
            "the boundary check must not fire first: {error}"
        );
        let mut reader = SqliteStoreReader {
            conn: sqlite.walk_differential_conn(),
            instrumentation: ReaderInstrumentation::default(),
        };
        let trusted =
            chain_walk::walk_chain_excluding(&mut reader, chain.head, &no_exclusion()).unwrap();
        let suffix = chain_walk::walk_suffix(
            &mut reader,
            late.id(),
            chain.head,
            trusted.cursor.epoch,
            &trusted.cursor.seen_keys,
            &trusted.cursor.record_ids,
            &trusted.cursor.reclaimed,
        );
        let error = format!("{:?}", suffix.err().unwrap());
        assert!(
            error.contains("duplicate-idempotency-key"),
            "the sqlite suffix key precedence: {error}"
        );
    }
    // (g) SUFFIX: the mid-suffix linkage beats the duplicate key (the
    // linkage check precedes the key check per record).
    {
        let (chain, object) = plain_chain();
        let (mut local, mut sqlite) = opened_pair(&case, &chain.steps);
        let middle = RevisionRecord::regular(chain.head, 3, 8)
            .with_content_refs(vec![object.hash()])
            .with_idempotency_key(b"wdiff-k1".to_vec());
        let mut top =
            RevisionRecord::regular(chain.head, 6, 7).with_content_refs(vec![object.hash()]);
        // top's parent is middle (epoch 4 + 1 ≠ 7 → the linkage
        // refusal fires at middle's read, before its duplicate key).
        top.parent = Some(middle.id());
        let steps = [
            Step::Record(middle.clone()),
            Step::Record(top.clone()),
            Step::Head(top.id()),
        ];
        run_local_steps(&local, &case.local_root(), &steps);
        run_sqlite_steps(&sqlite, &steps);
        let observation = compare_head("order-suffix-linkage-vs-key", &mut local, &mut sqlite);
        head_refusal(&observation, "epoch-regression");
        assert!(
            !observation
                .error
                .as_deref()
                .unwrap()
                .contains("duplicate-idempotency-key"),
            "the duplicate key must not fire first: {:?}",
            observation.error
        );
    }
}

/// (T29, axis 4) THE STALE-CURSOR FAMILY — the external-writer states
/// including REPEATED cursor advancement: N successive external
/// appends each validated by the standing session's `head()`, the
/// O(suffix)-per-step and O(0)-on-verified-head accounting pinned
/// across the WHOLE sequence through both providers' walk-stats
/// disciplines (local: `last_append_walked` per step and
/// `cold_walk_records` frozen at the open value — no rebuild ever
/// fires; sqlite: `last_append_walked` per step). The absorbed-append,
/// divergent-rewrite (cursor kept), vanished-head, and
/// corrupted-suffix (sound invalidation) states are pinned by the
/// dispatch-state test above; this test's contribution is the
/// SEQUENCE — the audit's "repeated cursor advancement" scenario.
#[test]
fn stale_cursor_repeated_advancement_is_bounded_per_step() {
    let case = Case::new("t29-repeated");
    const STEPS: u64 = 8;
    let (chain, object) = plain_chain();
    let (mut local, mut sqlite) = opened_pair(&case, &chain.steps);
    let cold_at_open = local.walk_stats().cold_walk_records;
    assert_eq!(cold_at_open, 6, "the plain chain's six records");
    let mut parent = chain.head;
    let mut previous_cursor = normalize_cursor(local.walk_differential_cursor());
    for step in 1..=STEPS {
        // The parent's epoch: the plain chain's head sits at epoch 5,
        // so step s appends over the parent at epoch 4 + s.
        let record = RevisionRecord::regular(parent, 4 + step, 100 + step)
            .with_content_refs(vec![object.hash()]);
        {
            let mut external = external_local(&case);
            external
                .append(&record, std::slice::from_ref(&object), Some(parent))
                .unwrap();
            drop(external);
            let mut external = external_sqlite(&case);
            external
                .append(&record, std::slice::from_ref(&object), Some(parent))
                .unwrap();
            drop(external);
        }
        let observation = compare_head("repeated-advancement", &mut local, &mut sqlite);
        assert_eq!(
            observation.observed,
            Some(Some(record.id())),
            "step {step} observes the external append"
        );
        assert!(observation.error.is_none(), "step {step}: {observation:?}");
        assert_ne!(
            observation.cursor, previous_cursor,
            "step {step} advances the cursor"
        );
        previous_cursor = observation.cursor;
        // The O(suffix) accounting per step: exactly the one walked
        // record — never a cold rebuild (the frozen open counter).
        assert_eq!(
            local.walk_stats().last_append_walked,
            1,
            "the local step {step} walks the one-record suffix"
        );
        assert_eq!(
            sqlite.walk_stats().last_append_walked,
            1,
            "the sqlite step {step} walks the one-record suffix"
        );
        assert_eq!(
            local.walk_stats().cold_walk_records,
            cold_at_open,
            "no cold rebuild fires across the sequence (step {step})"
        );
        parent = record.id();
    }
    // The O(0) follow-up on the now-verified head.
    let observation = compare_head("repeated-advancement-o1", &mut local, &mut sqlite);
    assert_eq!(observation.cursor, previous_cursor);
    assert_eq!(local.walk_stats().last_append_walked, 0, "the local O(0)");
    assert_eq!(sqlite.walk_stats().last_append_walked, 0, "the sqlite O(0)");
}

/// (T29, axis 5) PROVIDER FAILURE AT WALK READ POINTS — fault
/// injection at the reads themselves, both providers refusing typed
/// and fail-closed, never accepting. The existing FaultPoint /
/// SqliteFaultPoint seams fire only in the append/reconcile paths
/// (no walk read point consults them), so the read-point fault forms
/// are the transport-level failures: locally an object path replaced
/// by a directory (the stat succeeds, the read fails — the typed
/// `Io` class), on sqlite a dropped table (the row load fails — the
/// typed `Sqlite` class). THE HONEST COMPARISON RULE (stated, not
/// assumed away): the seam-independent classifications
/// (`Chain { rule }`, `MissingObject`, `ReferencedObjectCorrupt`)
/// compare byte-identically as the battery always has; the transport
/// variants (`Io` vs `Sqlite`) are legitimately provider-shaped and
/// compare as REFUSED-WITH-TYPED-ERROR ON BOTH — this test asserts
/// the refusal and the typed shape per provider without pretending
/// the transport strings could match.
#[test]
fn provider_read_failures_refuse_typed_fail_closed() {
    let case = Case::new("t29-provider-failure");
    // The transport-failure pair at the content-object read point.
    {
        let (chain, object) = plain_chain();
        let (local, _sqlite) = opened_pair(&case, &chain.steps);
        // Local: the referenced object's path becomes a directory.
        let path = case
            .local_root()
            .join(OBJECTS_DIR)
            .join(object.hash().hex());
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let local_observation =
            cold_outcome(local.walk_differential_cold(chain.head, &no_exclusion()));
        // Windows delete semantics (2026-11-29, runs 46155517625 +
        // 46156442963): the local medium's still-open handles let the
        // scratch wipe fail silently (the first run's swallowed
        // failure surfacing as a mystery unwrap far from its cause),
        // and the FIRST PAIR's sqlite medium holds the db-file lock
        // at the second wipe — the loud wipe naming it "os error 32".
        // Unix unlink semantics mask both (handles never block
        // removal). BOTH first-pair mediums are dropped BEFORE the
        // scratch is reused; the observations were captured above as
        // owned outcomes, not borrows.
        drop(local);
        drop(_sqlite);
        // Sqlite: the objects table is dropped under the same walk.
        let (chain2, _object2) = plain_chain();
        let (_local2, sqlite2) = opened_pair(&case, &chain2.steps);
        sqlite2
            .walk_differential_conn()
            .execute("DROP TABLE objects", [])
            .unwrap();
        let sqlite_observation =
            cold_outcome(sqlite2.walk_differential_cold(chain2.head, &no_exclusion()));
        // Both refuse typed, fail-closed, never accept.
        for (provider, observation) in [
            ("local", &local_observation),
            ("sqlite", &sqlite_observation),
        ] {
            assert!(
                observation.error.is_some(),
                "the {provider} read failure must refuse, got {observation:?}"
            );
            assert!(
                observation.summary.is_none() && observation.cursor.is_none(),
                "the {provider} read failure never accepts: {observation:?}"
            );
        }
        // The transport variants are provider-shaped BY DESIGN — the
        // documented rule: refused-with-typed-error on both, the
        // strings compared only within their own transport class.
        assert!(
            local_observation.error.as_deref().unwrap().contains("Io {"),
            "the local transport class is Io: {:?}",
            local_observation.error
        );
        assert!(
            sqlite_observation
                .error
                .as_deref()
                .unwrap()
                .contains("Sqlite {"),
            "the sqlite transport class is Sqlite: {:?}",
            sqlite_observation.error
        );
    }
    // The transport-failure pair at the record read point.
    {
        let (chain, _object) = plain_chain();
        let (local, _sqlite) = opened_pair(&case, &chain.steps);
        let path = case.local_root().join(OBJECTS_DIR).join(chain.ids[2].hex());
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let local_observation =
            cold_outcome(local.walk_differential_cold(chain.head, &no_exclusion()));
        assert!(local_observation.error.is_some());
        assert!(local_observation.cursor.is_none());
        let (chain2, _object2) = plain_chain();
        let (local2, sqlite2) = opened_pair(&case, &chain2.steps);
        drop(local2);
        sqlite2
            .walk_differential_conn()
            .execute("DROP TABLE records", [])
            .unwrap();
        let sqlite_observation =
            cold_outcome(sqlite2.walk_differential_cold(chain2.head, &no_exclusion()));
        assert!(sqlite_observation.error.is_some());
        assert!(sqlite_observation.cursor.is_none());
        assert!(
            sqlite_observation
                .error
                .as_deref()
                .unwrap()
                .contains("Sqlite {"),
            "the sqlite transport class: {:?}",
            sqlite_observation.error
        );
    }
    // THE SEAM-INDEPENDENT CONTROL: the same battery's
    // seam-independent classifications compare byte-identically —
    // the absent parent's `MissingObject` is the identical refusal
    // on both providers even though the probes differ (the object
    // path's `is_file` vs the `records` row's absence).
    {
        let (chain, _object) = plain_chain();
        let (local, sqlite) = opened_pair(&case, &chain.steps);
        let absent = chain.ids[2];
        run_local_steps(&local, &case.local_root(), &[Step::RemoveRecord(absent)]);
        run_sqlite_steps(&sqlite, &[Step::RemoveRecord(absent)]);
        let local_observation =
            cold_outcome(local.walk_differential_cold(chain.head, &no_exclusion()));
        let sqlite_observation =
            cold_outcome(sqlite.walk_differential_cold(chain.head, &no_exclusion()));
        assert_eq!(
            local_observation, sqlite_observation,
            "the seam-independent classification compares byte-identically"
        );
        cold_refusal(&local_observation, "MissingObject");
    }
}

// ============================================= (T29, commit 2) the equivalence
//
// THE L5 BAR (the `79b2fde` journal streaming differential's own
// discipline): the production streaming walks asserted EQUAL to the
// whole-read reference (`chain_walk::whole_read_reference`, the
// pre-streaming forms preserved verbatim under cfg(test)) over the
// full corpus — states by equality (the summary AND the cursor), the
// errors by their complete Debug AND Display surfaces — with the
// corpus gaining the streaming-discriminating shapes: a corrupt byte
// in the MIDDLE of a large object (beyond the leading window), a
// corrupt byte in the domain length prefix, an envelope whose
// declared payload length disagrees with its stored size, and
// truncations at the stream-window boundaries. Plus the sqlite
// substrate's own preflight: the substr windows concatenate to
// exactly the whole row (the pinned rusqlite's incremental blob I/O
// is feature-gated off — the design's sanctioned fallback, verified
// here at landing).

/// One cold equivalence case: the store steps and the head to walk.
struct EquivalenceCase {
    label: &'static str,
    steps: Vec<Step>,
    head: Hash,
}

/// Runs the production streaming walk AND the whole-read reference
/// walk over the SAME mirror reader, asserting full equality — the
/// summary and the cursor on success, the complete error surfaces on
/// refusal.
fn assert_streaming_equals_reference<R: ChainReads>(
    label: &str,
    provider: &str,
    open_reader: impl Fn() -> R,
    head: Hash,
) {
    let mut production_reader = open_reader();
    let production =
        chain_walk::walk_chain_excluding(&mut production_reader, head, &no_exclusion());
    let mut reference_reader = open_reader();
    let reference = chain_walk::whole_read_reference::walk_chain_excluding_reference(
        &mut reference_reader,
        head,
        &no_exclusion(),
    );
    match (production, reference) {
        (Ok(production), Ok(reference)) => {
            assert_eq!(
                format!("{:?}", production.summary),
                format!("{:?}", reference.summary),
                "the {label} summaries differ ({provider})"
            );
            assert_eq!(
                format!("{:?}", production.cursor),
                format!("{:?}", reference.cursor),
                "the {label} cursors differ ({provider})"
            );
        }
        (Err(production), Err(reference)) => {
            assert_eq!(
                format!("{production:?}"),
                format!("{reference:?}"),
                "the {label} refusal Debug surfaces differ ({provider})"
            );
            assert_eq!(
                format!("{production}"),
                format!("{reference}"),
                "the {label} refusal Display surfaces differ ({provider})"
            );
        }
        (production, reference) => panic!(
            "the {label} acceptance differs ({provider}): streaming accepted={} \
             reference accepted={}",
            production.is_ok(),
            reference.is_ok()
        ),
    }
}

/// The suffix form: the production streaming suffix walk vs the
/// whole-read reference suffix walk over the same reader, with the
/// trusted sets a real dispatch would pass (established by the
/// production cold walk over the base chain).
fn assert_suffix_streaming_equals_reference<R: ChainReads>(
    label: &str,
    provider: &str,
    open_reader: impl Fn() -> R,
    base_head: Hash,
    suffix_head: Hash,
) {
    let mut reader = open_reader();
    let trusted = chain_walk::walk_chain_excluding(&mut reader, base_head, &no_exclusion())
        .unwrap_or_else(|e| panic!("the {label} base walk must succeed ({provider}): {e:?}"));
    let production = chain_walk::walk_suffix(
        &mut reader,
        suffix_head,
        base_head,
        trusted.cursor.epoch,
        &trusted.cursor.seen_keys,
        &trusted.cursor.record_ids,
        &trusted.cursor.reclaimed,
    );
    let reference = chain_walk::whole_read_reference::walk_suffix_reference(
        &mut reader,
        suffix_head,
        base_head,
        trusted.cursor.epoch,
        trusted.cursor.seen_keys.clone(),
        trusted.cursor.record_ids.clone(),
        trusted.cursor.reclaimed.clone(),
    );
    let fingerprint =
        |suffix: &Result<Vec<chain_walk::VerifiedSuffixEntry>, MediumError>| match suffix {
            Ok(entries) => format!(
                "{:?}",
                entries
                    .iter()
                    .map(|entry| (
                        entry.id,
                        format!("{:?}", entry.record),
                        &entry.composite_closure
                    ))
                    .collect::<Vec<_>>()
            ),
            Err(error) => format!("{error:?} | {error}"),
        };
    assert_eq!(
        fingerprint(&production),
        fingerprint(&reference),
        "the {label} suffix observations differ ({provider})"
    );
}

/// (T29, commit 2) THE EQUIVALENCE PROOF — the streaming paths vs
/// the whole-read reference over the full corpus, both providers:
/// the valid families (founding-only, plain, the gapped reclaim
/// chain, the composite chain), the refusal families the frozen
/// rules define, and the streaming-discriminating shapes (the
/// mid-object byte flip under the original identity, the domain
/// length-prefix flip under the object's own identity, the
/// declared-length disagreement, the window-boundary truncations).
/// The suffix form runs the same proof over the dispatch's own
/// trusted-set flow.
#[test]
fn streaming_paths_equal_the_whole_read_reference_over_the_corpus() {
    let case = Case::new("t29-equivalence");
    // The sqlite substrate preflight: the substr windows concatenate
    // to exactly the whole row (the design's sanctioned fallback,
    // verified at landing — the pinned rusqlite's incremental blob
    // I/O is feature-gated off).
    {
        let (chain, _object) = plain_chain();
        let (local, sqlite) = opened_pair(&case, &chain.steps);
        drop(local);
        let conn = sqlite.walk_differential_conn();
        let object = plain_object(b"t29-substr-preflight-a-bit-longer");
        conn.execute(
            "INSERT OR REPLACE INTO objects (hash, domain, bytes) VALUES (?1, ?2, ?3)",
            params![object.hash().as_bytes(), object.domain(), object.bytes()],
        )
        .unwrap();
        let whole: Vec<u8> = conn
            .query_row(
                "SELECT bytes FROM objects WHERE hash = ?1",
                params![object.hash().as_bytes()],
                |row| row.get(0),
            )
            .unwrap();
        let mut windows: Vec<u8> = Vec::new();
        let mut offset = 0i64;
        loop {
            let window: Vec<u8> = conn
                .query_row(
                    "SELECT substr(bytes, ?2, ?3) FROM objects WHERE hash = ?1",
                    params![object.hash().as_bytes(), offset + 1, 8],
                    |row| row.get(0),
                )
                .unwrap();
            if window.is_empty() {
                break;
            }
            windows.extend_from_slice(&window);
            offset += window.len() as i64;
        }
        assert_eq!(
            windows, whole,
            "the substr windows must concatenate to the whole row (the substrate \
             verification — an empty blob, never NULL, past the end)"
        );
    }
    // The corpus.
    let mut corpus: Vec<EquivalenceCase> = Vec::new();
    {
        // The valid families.
        let object = plain_object(b"t29-eq-founding");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-ef", 0)
            .with_content_refs(vec![object.hash()]);
        corpus.push(EquivalenceCase {
            label: "founding-only",
            steps: vec![
                Step::Object(object),
                Step::Record(founding.clone()),
                Step::Head(founding.id()),
            ],
            head: founding.id(),
        });
    }
    {
        let (chain, _object) = plain_chain();
        corpus.push(EquivalenceCase {
            label: "plain",
            steps: chain.steps,
            head: chain.head,
        });
    }
    {
        let chain = census_reclaim_chain(16).0;
        corpus.push(EquivalenceCase {
            label: "gapped-reclaim",
            steps: chain.steps,
            head: chain.head,
        });
    }
    {
        let (chain, _expected) = composite_chain(71);
        corpus.push(EquivalenceCase {
            label: "composite",
            steps: chain.steps,
            head: chain.head,
        });
    }
    {
        // The refusal families.
        let (chain, object) = plain_chain();
        let mut steps = chain.steps.clone();
        steps.push(Step::CorruptObject(object.hash()));
        corpus.push(EquivalenceCase {
            label: "corrupt-object",
            steps,
            head: chain.head,
        });
    }
    {
        // The corrupt SECOND reference (the shared-first shape: the
        // record's refs are [the shared object, the corrupt second] —
        // a stale prefix cache serving the first ref's verdict for
        // the second would accept the corruption, the seeded
        // regression this shape exists to catch).
        let (chain, _object) = plain_chain();
        let second = plain_object(b"wdiff-second");
        let mut steps = chain.steps.clone();
        steps.push(Step::CorruptObject(second.hash()));
        corpus.push(EquivalenceCase {
            label: "corrupt-second-reference",
            steps,
            head: chain.head,
        });
    }
    {
        // A VALID large object (beyond the streaming window): an
        // early-exiting stream that stops before the object's end
        // would falsely refuse it, the seeded regression this shape
        // exists to catch — both paths must ACCEPT.
        const L5_MAX_BYTES: usize = 5 * 1024 * 1024 + 256 * 1024;
        const FRAMING_BYTES: usize = 8 + "sektor7/save/1".len() + 8;
        let big = plain_object(&vec![0x7e; L5_MAX_BYTES - FRAMING_BYTES]);
        let small = plain_object(b"t29-eq-valid-big-small");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-evb", 0)
            .with_content_refs(vec![small.hash()]);
        let r1 = RevisionRecord::regular(founding.id(), 0, 1).with_content_refs(vec![big.hash()]);
        let r2 = RevisionRecord::regular(r1.id(), 1, 2).with_content_refs(vec![big.hash()]);
        corpus.push(EquivalenceCase {
            label: "valid-large-object",
            steps: vec![
                Step::Object(small),
                Step::Object(big),
                Step::Record(founding),
                Step::Record(r1),
                Step::Record(r2.clone()),
                Step::Head(r2.id()),
            ],
            head: r2.id(),
        });
    }
    {
        let (chain, _object) = plain_chain();
        let mut steps = chain.steps.clone();
        steps.push(Step::CorruptRecord(chain.ids[3]));
        corpus.push(EquivalenceCase {
            label: "corrupt-record",
            steps,
            head: chain.head,
        });
    }
    {
        let (chain, object) = plain_chain();
        let mut steps = chain.steps.clone();
        steps.push(Step::RemoveObject(object.hash()));
        corpus.push(EquivalenceCase {
            label: "missing-object",
            steps,
            head: chain.head,
        });
    }
    {
        let (chain, _object) = plain_chain();
        let mut steps = chain.steps.clone();
        steps.push(Step::RemoveRecord(chain.ids[2]));
        corpus.push(EquivalenceCase {
            label: "uncovered-gap",
            steps,
            head: chain.head,
        });
    }
    {
        let object = plain_object(b"t29-eq-epoch");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-ee", 0)
            .with_content_refs(vec![object.hash()]);
        let regressive =
            RevisionRecord::regular(founding.id(), 2, 1).with_content_refs(vec![object.hash()]);
        corpus.push(EquivalenceCase {
            label: "epoch-regression",
            steps: vec![
                Step::Object(object),
                Step::Record(founding),
                Step::Record(regressive.clone()),
                Step::Head(regressive.id()),
            ],
            head: regressive.id(),
        });
    }
    {
        let object = plain_object(b"t29-eq-dup");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-ed", 0)
            .with_content_refs(vec![object.hash()]);
        let first = RevisionRecord::regular(founding.id(), 0, 1)
            .with_content_refs(vec![object.hash()])
            .with_idempotency_key(b"t29-eq-key".to_vec());
        let second = RevisionRecord::regular(first.id(), 1, 2)
            .with_content_refs(vec![object.hash()])
            .with_idempotency_key(b"t29-eq-key".to_vec());
        corpus.push(EquivalenceCase {
            label: "duplicate-key",
            steps: vec![
                Step::Object(object),
                Step::Record(founding),
                Step::RecordIdemSuppressed(first),
                Step::RecordIdemSuppressed(second.clone()),
                Step::Head(second.id()),
            ],
            head: second.id(),
        });
    }
    {
        let own = plain_object(b"t29-eq-dep");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-eq", 0)
            .with_content_refs(vec![own.hash()]);
        let record = RevisionRecord::regular(founding.id(), 0, 1)
            .with_content_refs(vec![own.hash()])
            .with_depends_on(vec![hash_object(b"t29-eq-nope")]);
        let top = RevisionRecord::regular(record.id(), 1, 2).with_content_refs(vec![own.hash()]);
        corpus.push(EquivalenceCase {
            label: "depends-unresolvable",
            steps: vec![
                Step::Object(own),
                Step::Record(founding),
                Step::Record(record),
                Step::Record(top.clone()),
                Step::Head(top.id()),
            ],
            head: top.id(),
        });
    }
    {
        // The format families.
        let garbage = [0xff_u8, 0xfe, 0xfd, 0xfc, 0xfb];
        let garbage_id = hash_object(&garbage);
        let (chain, _object) = plain_chain();
        let record = RevisionRecord::regular(chain.head, 5, 9).with_content_refs(vec![garbage_id]);
        let mut steps = chain.steps.clone();
        steps.push(Step::RawObject(garbage.to_vec()));
        steps.push(Step::Record(record.clone()));
        steps.push(Step::Head(record.id()));
        corpus.push(EquivalenceCase {
            label: "format-domain",
            steps,
            head: record.id(),
        });
    }
    {
        let garbage = b"t29-eq-not-a-record";
        let bogus = hash_object(garbage);
        corpus.push(EquivalenceCase {
            label: "format-record",
            steps: vec![Step::RawRecord(garbage.to_vec()), Step::Head(bogus)],
            head: bogus,
        });
    }
    {
        // THE STREAMING-DISCRIMINATING SHAPES. (a) A corrupt byte in
        // the MIDDLE of a large object (beyond the leading window),
        // stored under the ORIGINAL identity: the hash must catch it
        // exactly as the whole read does (an early-exit stream that
        // stops at the prefix would ACCEPT it — the seeded
        // regression this shape exists to catch).
        const L5_MAX_BYTES: usize = 5 * 1024 * 1024 + 256 * 1024;
        const FRAMING_BYTES: usize = 8 + "sektor7/save/1".len() + 8;
        let big = plain_object(&vec![0xc3; L5_MAX_BYTES - FRAMING_BYTES]);
        let original_id = big.hash();
        let mut flipped = big.bytes().to_vec();
        let mid = flipped.len() / 2 + 12_345;
        flipped[mid] ^= 0xff;
        let small = plain_object(b"t29-eq-small");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-em", 0)
            .with_content_refs(vec![small.hash()]);
        let r1 = RevisionRecord::regular(founding.id(), 0, 1).with_content_refs(vec![original_id]);
        corpus.push(EquivalenceCase {
            label: "mid-object-byte-flip",
            steps: vec![
                Step::Object(small),
                Step::Object(big),
                Step::Record(founding),
                Step::Record(r1.clone()),
                Step::Head(r1.id()),
                Step::CorruptObjectAt(original_id, flipped),
            ],
            head: r1.id(),
        });
        // (b) A corrupt byte in the domain LENGTH prefix, stored at
        // the flipped bytes' OWN identity: the bounded domain read
        // refuses identically (a prefix-cache serving stale bytes
        // across references would diverge here — the second seeded
        // regression's shape). Stored RAW (the flipped length prefix
        // no longer frames as a `StoredObject`; the store is
        // domain-agnostic and the identity is the bytes' own).
        let mut prefix_flipped = plain_object(b"t29-eq-prefix").bytes().to_vec();
        prefix_flipped[3] ^= 0xff;
        let flipped_id = hash_object(&prefix_flipped);
        let pf = plain_object(b"t29-eq-pf");
        let founding_p = RevisionRecord::founding(AuthorityMode::Standalone, "t29-ep", 0)
            .with_content_refs(vec![pf.hash()]);
        let r_p =
            RevisionRecord::regular(founding_p.id(), 0, 1).with_content_refs(vec![flipped_id]);
        corpus.push(EquivalenceCase {
            label: "domain-prefix-byte-flip",
            steps: vec![
                Step::Object(pf),
                Step::RawObject(prefix_flipped),
                Step::Record(founding_p),
                Step::Record(r_p.clone()),
                Step::Head(r_p.id()),
            ],
            head: r_p.id(),
        });
        // (c) Truncations at the stream-window boundaries, stored
        // under the ORIGINAL identity (the hash catches each exactly
        // as the whole read does — a short-read-as-EOF misreading
        // would diverge here; the third seeded regression's shape).
        for (label, cut) in [
            (
                "window-boundary-truncation",
                crate::chain_walk::STREAM_CHUNK_BYTES,
            ),
            (
                "window-boundary-truncation-plus-one",
                crate::chain_walk::STREAM_CHUNK_BYTES + 1,
            ),
            (
                "two-window-truncation",
                2 * crate::chain_walk::STREAM_CHUNK_BYTES,
            ),
        ] {
            let big_t = plain_object(&vec![0x3c; L5_MAX_BYTES - FRAMING_BYTES]);
            let original = big_t.hash();
            let truncated = big_t.bytes()[..cut.min(big_t.bytes().len())].to_vec();
            let small_t = plain_object(b"t29-eq-trunc-small");
            let founding_t = RevisionRecord::founding(AuthorityMode::Standalone, "t29-et", 0)
                .with_content_refs(vec![small_t.hash()]);
            let r_t =
                RevisionRecord::regular(founding_t.id(), 0, 1).with_content_refs(vec![original]);
            corpus.push(EquivalenceCase {
                label,
                steps: vec![
                    Step::Object(small_t),
                    Step::Object(big_t),
                    Step::Record(founding_t),
                    Step::Record(r_t.clone()),
                    Step::Head(r_t.id()),
                    Step::CorruptObjectAt(original, truncated),
                ],
                head: r_t.id(),
            });
        }
    }
    {
        // (d) The declared-length disagreement: a hand-framed chunk
        // envelope whose payload length prefix declares 1 MiB over an
        // 18-byte payload — the whole form refuses at the payload
        // read's truncation, the streaming form at the accounting;
        // the ONE rule string both map to.
        let mut envelope = Vec::new();
        let mut enc = Encoder::new();
        enc.write_str(composite_manifest::SESSION_CHUNK_DOMAIN);
        enc.write_str(TAG_HEADER);
        envelope.extend_from_slice(&enc.into_bytes());
        envelope.extend_from_slice(&(1u64 << 20).to_le_bytes());
        envelope.extend_from_slice(b"t29-short-payload");
        let mismatched = StoredObject::new(envelope).unwrap();
        let (reference_root, reference_chunks) = composite_fixture(81);
        let reference = composite_manifest::decode(reference_root.bytes()).unwrap();
        let mut entries = Vec::new();
        for entry in &reference.entries {
            if entry.tag == TAG_HEADER {
                entries.push(ManifestEntry {
                    tag: TAG_HEADER,
                    identity: mismatched.hash(),
                    payload_length: 1 << 20,
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
        let plain = plain_object(b"t29-eq-dl");
        let founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-eq-dl", 0)
            .with_content_refs(vec![plain.hash()]);
        let r1 = RevisionRecord::regular(founding.id(), 0, 1).with_content_refs(vec![plain.hash()]);
        let snap = RevisionRecord::regular(r1.id(), 1, 2).with_content_refs(vec![root.hash()]);
        let mut steps = vec![
            Step::Object(plain),
            Step::Object(mismatched),
            Step::Object(root),
            Step::Record(founding),
            Step::Record(r1),
        ];
        for chunk in &reference_chunks {
            steps.push(Step::Object(chunk.clone()));
        }
        steps.push(Step::Record(snap.clone()));
        steps.push(Step::Head(snap.id()));
        corpus.push(EquivalenceCase {
            label: "declared-length-disagreement",
            steps,
            head: snap.id(),
        });
    }
    // Every corpus case, both providers, production vs reference.
    for equivalence in &corpus {
        let (local, sqlite) = opened_pair(&case, &[]);
        run_local_steps(&local, &case.local_root(), &equivalence.steps);
        run_sqlite_steps(&sqlite, &equivalence.steps);
        let local_root = case.local_root();
        assert_streaming_equals_reference(
            equivalence.label,
            "local",
            || LocalStoreReader {
                root: &local_root,
                instrumentation: ReaderInstrumentation::default(),
            },
            equivalence.head,
        );
        let conn = sqlite.walk_differential_conn();
        assert_streaming_equals_reference(
            equivalence.label,
            "sqlite",
            || SqliteStoreReader {
                conn,
                instrumentation: ReaderInstrumentation::default(),
            },
            equivalence.head,
        );
    }
    // The suffix form: the same proof through the dispatch's own
    // trusted-set flow — the valid absorption, the boundary
    // violation, the duplicate key, the divergent rewrite, the valid
    // composite closure, and the composite chunk corrupted mid-object
    // (the streaming discriminator inside the suffix walk).
    {
        let (chain, object) = plain_chain();
        let valid = chain.suffix_record(&object, 6);
        let (local, sqlite) = opened_pair(&case, &[]);
        let mut suffix_steps = chain.steps.clone();
        suffix_steps.push(Step::Record(valid.clone()));
        suffix_steps.push(Step::Head(valid.id()));
        run_local_steps(&local, &case.local_root(), &suffix_steps);
        run_sqlite_steps(&sqlite, &suffix_steps);
        let local_root = case.local_root();
        assert_suffix_streaming_equals_reference(
            "suffix-valid",
            "local",
            || LocalStoreReader {
                root: &local_root,
                instrumentation: ReaderInstrumentation::default(),
            },
            chain.head,
            valid.id(),
        );
        let conn = sqlite.walk_differential_conn();
        assert_suffix_streaming_equals_reference(
            "suffix-valid",
            "sqlite",
            || SqliteStoreReader {
                conn,
                instrumentation: ReaderInstrumentation::default(),
            },
            chain.head,
            valid.id(),
        );
    }
    {
        let (chain, object) = plain_chain();
        let late = RevisionRecord::regular(chain.head, 6, 7).with_content_refs(vec![object.hash()]);
        let (local, sqlite) = opened_pair(&case, &[]);
        let mut suffix_steps = chain.steps.clone();
        suffix_steps.push(Step::Record(late.clone()));
        suffix_steps.push(Step::Head(late.id()));
        run_local_steps(&local, &case.local_root(), &suffix_steps);
        run_sqlite_steps(&sqlite, &suffix_steps);
        let local_root = case.local_root();
        assert_suffix_streaming_equals_reference(
            "suffix-boundary",
            "local",
            || LocalStoreReader {
                root: &local_root,
                instrumentation: ReaderInstrumentation::default(),
            },
            chain.head,
            late.id(),
        );
    }
    {
        let (chain, object) = plain_chain();
        let dup = RevisionRecord::regular(chain.head, 5, 6)
            .with_content_refs(vec![object.hash()])
            .with_idempotency_key(b"wdiff-k1".to_vec());
        let (local, sqlite) = opened_pair(&case, &[]);
        let mut suffix_steps = chain.steps.clone();
        suffix_steps.push(Step::RecordIdemSuppressed(dup.clone()));
        suffix_steps.push(Step::Head(dup.id()));
        run_local_steps(&local, &case.local_root(), &suffix_steps);
        run_sqlite_steps(&sqlite, &suffix_steps);
        let local_root = case.local_root();
        assert_suffix_streaming_equals_reference(
            "suffix-duplicate-key",
            "local",
            || LocalStoreReader {
                root: &local_root,
                instrumentation: ReaderInstrumentation::default(),
            },
            chain.head,
            dup.id(),
        );
    }
    {
        let (chain, object) = plain_chain();
        let foreign = plain_object(b"t29-eq-divergent");
        let foreign_founding = RevisionRecord::founding(AuthorityMode::Standalone, "t29-eqd", 0)
            .with_content_refs(vec![foreign.hash()]);
        let divergent = RevisionRecord::regular(foreign_founding.id(), 0, 7)
            .with_content_refs(vec![object.hash()]);
        let (local, sqlite) = opened_pair(&case, &[]);
        let mut suffix_steps = chain.steps.clone();
        suffix_steps.push(Step::Object(foreign));
        suffix_steps.push(Step::Record(foreign_founding));
        suffix_steps.push(Step::Record(divergent.clone()));
        suffix_steps.push(Step::Head(divergent.id()));
        run_local_steps(&local, &case.local_root(), &suffix_steps);
        run_sqlite_steps(&sqlite, &suffix_steps);
        let local_root = case.local_root();
        assert_suffix_streaming_equals_reference(
            "suffix-divergence",
            "local",
            || LocalStoreReader {
                root: &local_root,
                instrumentation: ReaderInstrumentation::default(),
            },
            chain.head,
            divergent.id(),
        );
    }
    {
        // The valid composite suffix + the mid-chunk corruption.
        let (chain, _object) = plain_chain();
        let (root, chunks) = composite_fixture(91);
        let record = RevisionRecord::regular(chain.head, 5, 6).with_content_refs(vec![root.hash()]);
        let (local, sqlite) = opened_pair(&case, &[]);
        let mut suffix_steps = chain.steps.clone();
        suffix_steps.push(Step::Object(root));
        suffix_steps.push(Step::Record(record.clone()));
        for chunk in &chunks {
            suffix_steps.push(Step::Object(chunk.clone()));
        }
        suffix_steps.push(Step::Head(record.id()));
        run_local_steps(&local, &case.local_root(), &suffix_steps);
        run_sqlite_steps(&sqlite, &suffix_steps);
        let local_root = case.local_root();
        assert_suffix_streaming_equals_reference(
            "suffix-composite",
            "local",
            || LocalStoreReader {
                root: &local_root,
                instrumentation: ReaderInstrumentation::default(),
            },
            chain.head,
            record.id(),
        );
        let conn = sqlite.walk_differential_conn();
        assert_suffix_streaming_equals_reference(
            "suffix-composite",
            "sqlite",
            || SqliteStoreReader {
                conn,
                instrumentation: ReaderInstrumentation::default(),
            },
            chain.head,
            record.id(),
        );
        // The corrupted-chunk variant: a byte flipped in the middle
        // of one chunk under its original identity — the suffix
        // walk's closure verification must refuse identically on both
        // paths.
        let mut flipped = chunks[2].bytes().to_vec();
        let mid = flipped.len() / 2 + 77;
        flipped[mid] ^= 0xff;
        let original = chunks[2].hash();
        let (local, sqlite) = opened_pair(&case, &[]);
        let mut corrupt_steps = suffix_steps.clone();
        corrupt_steps.push(Step::CorruptObjectAt(original, flipped));
        run_local_steps(&local, &case.local_root(), &corrupt_steps);
        run_sqlite_steps(&sqlite, &corrupt_steps);
        let local_root = case.local_root();
        assert_suffix_streaming_equals_reference(
            "suffix-composite-corrupt-chunk",
            "local",
            || LocalStoreReader {
                root: &local_root,
                instrumentation: ReaderInstrumentation::default(),
            },
            chain.head,
            record.id(),
        );
        let conn = sqlite.walk_differential_conn();
        assert_suffix_streaming_equals_reference(
            "suffix-composite-corrupt-chunk",
            "sqlite",
            || SqliteStoreReader {
                conn,
                instrumentation: ReaderInstrumentation::default(),
            },
            chain.head,
            record.id(),
        );
    }
}
