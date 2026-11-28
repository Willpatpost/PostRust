//! `SqliteMedium` — the PP-B1 production canonical medium (the
//! owner-ratified D-PP-1): the accepted [`CanonicalMedium`] contract
//! over **one SQLite database per campaign**, `journal_mode=WAL` with
//! `synchronous=FULL` — **both verified by querying the actual
//! configuration at open** (a request is never assumed granted).
//!
//! **The transaction contract (the ratified §3.2):** one
//! `BEGIN IMMEDIATE` transaction contains every required object
//! insert, the revision-record insert, and the `meta.head` update;
//! **the successful `COMMIT` is the linearization and acceptance
//! boundary** — `Accepted` is returned only after it, carrying the
//! honest [`SyncTier::TransactionalWalFull`] tier. `SQLITE_BUSY` at
//! `BEGIN` is **contention, never a stale parent and never an
//! uncertain commit** (no transaction work began) — the bounded
//! internal retry is SQLite's own busy-timeout schedule, and its
//! expiry is the typed [`MediumError::Contention`] abort. A mid-
//! transaction failure attempts `ROLLBACK`: success is the typed
//! error (nothing installed), a failed rollback is
//! `Unknown{RollbackUncertain}`; a failed `COMMIT` is
//! `Unknown{CommitUncertain}` — never a guessed outcome, reconciled
//! under a fresh committed read. `RejectedStaleParent` is returned
//! only after the head read **inside the transaction** verified a
//! genuinely changed authoritative parent.
//!
//! **`reconcile_unknown`** establishes `Installed` only when the
//! candidate is HEAD or is present in HEAD's **fully validated**
//! ancestor chain; absence from a validated chain is `NotInstalled`;
//! inability to validate is `Unverifiable` — the uncertainty is never
//! converted into a guess, and a corrupt chain never yields a false
//! confirmation.
//!
//! **The trusted cursor** (the §5.3.2 model, identical semantics to
//! `LocalFileMedium`): session-scoped in-memory derived state; O(1)
//! validation work per append to the already-verified head; a bounded
//! suffix walk on external divergence; sound invalidation forces the
//! full cold walk; fail-closed on non-descendant histories. The cold
//! walk enforces every ratified rule: complete record hash
//! verification, checked epoch linkage, chain-wide idempotency-key
//! uniqueness, content-reference integrity, founding coherence, and
//! strictly-earlier dependency ancestry.
//!
//! **No quarantine protocol:** an uncommitted transaction leaves
//! nothing (there are no orphan files); an unreferenced row in
//! `objects` is harmless content-addressed evidence a later retry
//! reuses idempotently; a hash-mismatching row is fail-closed
//! evidence, never rewritten in place. The engine never attempts
//! `.recover` or any destructive repair.
//!
//! **The read-only audit mode** ([`SqliteMedium::open_read_only`]):
//! a `SQLITE_OPEN_READ_ONLY` connection — the WAL snapshot lets the
//! full validation walk run without blocking the writer; `append`
//! refuses with the typed error before any transaction begins.
//!
//! **The checkpoint policy:** SQLite's passive auto-checkpoint runs
//! in operation; [`SqliteMedium::checkpoint_truncate`] issues the
//! ratified `TRUNCATE` checkpoint for the graceful lifecycle
//! boundaries (the session close, the backup capture).
//!
//! **The fault seam** ([`SqliteFaultControl`], test infrastructure):
//! the transaction-phase injection points for the outcome matrix and
//! the real-process crash tests — `Abort` terminates the process at
//! the boundary (a genuine crash), `InjectError` surfaces the phase's
//! failure for the honest classification. A medium constructed
//! without a control never consults the seam.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};

use crate::composite_manifest;
use crate::hash::{Hash, hash_object};
use crate::local::WalkStats;
use crate::medium::{
    AppendOutcome, CanonicalMedium, ChainSummary, MediumError, ObjectClass, Reconciliation,
    StoredObject, SyncTier, UnknownReason,
};
use crate::record::{AuthorityMode, RecordKind, RevisionRecord};

mod backup;
mod meta;
mod reclaim_exec;
mod rows;
mod walks;

use meta::{
    META_MIGRATION_PENDING, META_PROVENANCE, SCHEMA_SQL, configure_writable,
    decode_migration_pending, migration_pending_bytes, read_meta_head, read_meta_row,
    verify_authority_row,
};
use reclaim_exec::{
    observe_range_terminal_tx, reclaim_authorized_tx, reclaim_pending_authorized,
    verify_begin_authority_tx, verify_begin_authority_v2_tx,
};
use rows::{
    append_inserts, load_object_verified, load_record_verified, load_referenced_object,
    rollback_or_unknown,
};
use walks::{SqliteCursor, advance_cursor, validate_observed, walk_chain};
// The test batteries' `use super::*` globs resolve these child-module
// names through the root (the S4 private-use-binding precedent).
#[cfg(test)]
use reclaim_exec::{
    find_installed_begin_tx, reconcile_begin_publication, reconcile_begin_publication_v2,
};
#[cfg(test)]
use rows::{insert_object_once, insert_record_once};
#[cfg(test)]
use walks::walk_chain_excluding;

/// The connection configuration. The busy timeout IS the bounded
/// contention retry (SQLite's internal backoff schedule); its expiry
/// surfaces as the typed [`MediumError::Contention`] abort.
#[derive(Debug, Clone, Copy)]
pub struct SqliteMediumConfig {
    /// How long `BEGIN IMMEDIATE` (and lock-taking statements) retry
    /// internally before surfacing `SQLITE_BUSY`.
    pub busy_timeout: Duration,
}

impl Default for SqliteMediumConfig {
    fn default() -> Self {
        SqliteMediumConfig {
            busy_timeout: Duration::from_millis(5_000),
        }
    }
}

// ------------------------------------------------------------ fault seam

/// The transaction-phase injection points (the SQLite outcome
/// matrix's crash windows). Each maps to a precise point in the
/// append transaction; `Abort` terminates the process there (a
/// genuine crash — the real-process tests), `InjectError` surfaces
/// the phase's failure for the honest classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SqliteFaultPoint {
    /// After the first object insert, further inserts pending.
    MidObjectInserts,
    /// All rows inserted, the `meta.head` update pending.
    ObjectsInserted,
    /// The head updated, the `COMMIT` pending (an injected error here
    /// simulates the commit failure → `Unknown{CommitUncertain}`).
    CommitFailure,
    /// Inside the rollback attempt after a mid-transaction failure
    /// (an injected error here → `Unknown{RollbackUncertain}`).
    AtRollback,
    /// The commit succeeded, before the acknowledgment returns (the
    /// post-commit process-kill window).
    AfterCommit,
}

impl SqliteFaultPoint {
    /// The stable name used by the process-test harness environment.
    pub fn name(&self) -> &'static str {
        match self {
            SqliteFaultPoint::MidObjectInserts => "mid-object-inserts",
            SqliteFaultPoint::ObjectsInserted => "objects-inserted",
            SqliteFaultPoint::CommitFailure => "commit-failure",
            SqliteFaultPoint::AtRollback => "at-rollback",
            SqliteFaultPoint::AfterCommit => "after-commit",
        }
    }

    /// Parses the harness name form.
    pub fn from_name(name: &str) -> Option<Self> {
        const POINTS: &[SqliteFaultPoint] = &[
            SqliteFaultPoint::MidObjectInserts,
            SqliteFaultPoint::ObjectsInserted,
            SqliteFaultPoint::CommitFailure,
            SqliteFaultPoint::AtRollback,
            SqliteFaultPoint::AfterCommit,
        ];
        POINTS.iter().copied().find(|point| point.name() == name)
    }
}

/// What the seam does when it fires at an armed point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqliteFaultAction {
    /// The process aborts abruptly at the boundary — a real crash
    /// (the OS and SQLite's own locking recover).
    Abort,
    /// The phase's operation surfaces an injected failure; the medium
    /// must classify it honestly.
    InjectError,
}

/// An armed map of point → action. Test-only; constructed via
/// [`SqliteMedium::open_with`].
#[derive(Debug, Clone, Default)]
pub struct SqliteFaultControl {
    armed: BTreeMap<SqliteFaultPoint, SqliteFaultAction>,
}

impl SqliteFaultControl {
    pub fn new() -> Self {
        Self::default()
    }

    /// Arms a point (builder form).
    pub fn arm(mut self, point: SqliteFaultPoint, action: SqliteFaultAction) -> Self {
        self.armed.insert(point, action);
        self
    }

    fn check(&self, point: SqliteFaultPoint) -> Result<(), MediumError> {
        match self.armed.get(&point) {
            None => Ok(()),
            Some(SqliteFaultAction::Abort) => std::process::abort(),
            Some(SqliteFaultAction::InjectError) => Err(MediumError::Sqlite {
                op: "fault-injected",
                detail: point.name().to_owned(),
            }),
        }
    }
}

/// The SQLite-backed production canonical medium.
pub struct SqliteMedium {
    conn: Connection,
    db_path: PathBuf,
    cursor: Option<SqliteCursor>,
    read_only: bool,
    fault: Option<SqliteFaultControl>,
    walks: WalkStats,
}

impl SqliteMedium {
    /// Opens (creating if absent) the campaign database with the
    /// ratified configuration, performing the session open: the
    /// schema creation, the WAL/FULL **verification**, and the cold
    /// founding → HEAD validation that establishes the trusted
    /// cursor.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, MediumError> {
        Self::open_with(path, &SqliteMediumConfig::default(), None)
    }

    /// The full constructor with the configuration and the optional
    /// test fault seam (production passes `None`).
    pub fn open_with(
        path: impl Into<PathBuf>,
        config: &SqliteMediumConfig,
        fault: Option<SqliteFaultControl>,
    ) -> Result<Self, MediumError> {
        let db_path = path.into();
        let conn = Connection::open(&db_path).map_err(|e| sqlite_err("open", e))?;
        configure_writable(&conn, config)?;
        conn.execute_batch(SCHEMA_SQL)
            .map_err(|e| sqlite_err_or_contention("create schema", e))?;
        // The unfounded sentinel: an empty head row, seeded once.
        conn.execute(
            "INSERT OR IGNORE INTO meta (key, value) VALUES ('head', x'')",
            [],
        )
        .map_err(|e| sqlite_err_or_contention("seed head row", e))?;
        // §6.1's target lifecycle (the review corrective): a
        // migration-pending database is not a live campaign — the
        // normal open refuses it; only `open_migration_target`
        // proceeds (and the activation removes the marker).
        if let Some(row) = read_meta_row(&conn, META_MIGRATION_PENDING)? {
            decode_migration_pending(&row)?;
            return Err(MediumError::MigrationPending {
                detail: format!(
                    "{} is an unactivated migration target — complete the \
                     migration (a re-run resumes the copy) or remove the \
                     unactivated target; it is not a live campaign",
                    db_path.display()
                ),
            });
        }
        Self::finish_open(conn, db_path, false, fault)
    }

    /// The read-only audit connection: the WAL snapshot lets the full
    /// validation walk run without blocking any writer; `append`
    /// refuses with the typed error before any transaction begins. A
    /// database that cannot be opened or read (including a WAL the
    /// read-only connection cannot map) fails closed with the typed
    /// error — never a silent empty view.
    pub fn open_read_only(path: impl Into<PathBuf>) -> Result<Self, MediumError> {
        let db_path = path.into();
        let conn = Connection::open_with_flags(
            &db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|e| sqlite_err("open read-only", e))?;
        // journal_mode is persistent in the database header — query
        // (never set) on a read-only connection and verify.
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .map_err(|e| sqlite_err("query journal_mode", e))?;
        if mode != "wal" {
            return Err(MediumError::Sqlite {
                op: "verify journal_mode",
                detail: format!("expected wal, observed {mode}"),
            });
        }
        // §6.1's target lifecycle (the review corrective): a pending
        // migration target is not a live campaign — the audit open
        // refuses it too.
        if let Some(row) = read_meta_row(&conn, META_MIGRATION_PENDING)? {
            decode_migration_pending(&row)?;
            return Err(MediumError::MigrationPending {
                detail: format!(
                    "{} is an unactivated migration target — not a live campaign",
                    db_path.display()
                ),
            });
        }
        Self::finish_open(conn, db_path, true, None)
    }

    /// §6.1's target lifecycle (the review corrective): opens the
    /// database AS a migration target — the only constructor that
    /// proceeds past the pending refusal. A fresh unfounded database
    /// receives the persistent `migration-pending` marker (from that
    /// instant it is distinguishable from a standalone campaign, so
    /// an interrupted migration can never be mistaken for one); a
    /// pending target resumes. Anything else refuses: an already
    /// activated target (the provenance row present — a completed
    /// migration is never re-migrated in place) and a founded
    /// standalone campaign (no marker — never hijacked into a
    /// migration).
    pub fn open_migration_target(path: impl Into<PathBuf>) -> Result<Self, MediumError> {
        let db_path = path.into();
        let conn = Connection::open(&db_path).map_err(|e| sqlite_err("open", e))?;
        configure_writable(&conn, &SqliteMediumConfig::default())?;
        conn.execute_batch(SCHEMA_SQL)
            .map_err(|e| sqlite_err_or_contention("create schema", e))?;
        conn.execute(
            "INSERT OR IGNORE INTO meta (key, value) VALUES ('head', x'')",
            [],
        )
        .map_err(|e| sqlite_err_or_contention("seed head row", e))?;
        match read_meta_row(&conn, META_MIGRATION_PENDING)? {
            None => {
                if read_meta_row(&conn, META_PROVENANCE)?.is_some() {
                    return Err(MediumError::Sqlite {
                        op: "open_migration_target",
                        detail: "the database is already an activated migration \
                                 target — never re-migrate it in place"
                            .to_owned(),
                    });
                }
                if read_meta_head(&conn)?.is_some() {
                    return Err(MediumError::Sqlite {
                        op: "open_migration_target",
                        detail: "the database is a founded standalone campaign — \
                                 a migration target must be a fresh database"
                            .to_owned(),
                    });
                }
                conn.execute(
                    "INSERT INTO meta (key, value) VALUES (?1, ?2)",
                    params![META_MIGRATION_PENDING, migration_pending_bytes()],
                )
                .map_err(|e| sqlite_err("write migration-pending row", e))?;
            }
            Some(row) => decode_migration_pending(&row)?,
        }
        Self::finish_open(conn, db_path, false, None)
    }

    /// The shared open tail: the head read, the cold walk, the
    /// authority cross-check, and the cursor establishment.
    fn finish_open(
        mut conn: Connection,
        db_path: PathBuf,
        read_only: bool,
        fault: Option<SqliteFaultControl>,
    ) -> Result<Self, MediumError> {
        let tx = conn
            .transaction()
            .map_err(|e| sqlite_err("open read transaction", e))?;
        let observed = read_meta_head(&tx)?;
        let mut walks = WalkStats::default();
        let cursor = match observed {
            Some(head) => {
                let walked = walk_chain(&tx, head)?;
                walks.cold_walk_records = walked.summary.records;
                Some(walked.cursor)
            }
            None => None,
        };
        verify_authority_row(&tx, cursor.as_ref())?;
        tx.commit()
            .map_err(|e| sqlite_err("commit open read transaction", e))?;
        Ok(SqliteMedium {
            conn,
            db_path,
            cursor,
            read_only,
            fault,
            walks,
        })
    }

    /// The database file path (diagnostics and the tooling).
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// The campaign identity established by the session open (or a
    /// later validated append) — founding id, authority mode, and
    /// campaign id, without re-walking. `None` before the store is
    /// founded.
    pub fn campaign(&self) -> Option<(Hash, AuthorityMode, &str)> {
        self.cursor.as_ref().map(|cursor| {
            (
                cursor.founding,
                cursor.authority,
                cursor.campaign_id.as_str(),
            )
        })
    }

    /// The deterministic validation-work counters (the O(1)-cursor
    /// test assertions).
    pub fn walk_stats(&self) -> WalkStats {
        self.walks
    }

    /// Test-only (the `composite-test-fixture` feature — never
    /// compiled into a production build): the trusted cursor's
    /// accounting — `(head, epoch, distinct content objects)`. The
    /// cold walk establishes these fields and the hot and suffix
    /// paths maintain them under the same definition, so this is the
    /// direct hot/cold/suffix equivalence observation (a subsequent
    /// `validate()` cold-walks by design and cannot prove it).
    /// `None` before the store is founded.
    #[cfg(feature = "composite-test-fixture")]
    pub fn trusted_cursor_accounting(&self) -> Option<(Hash, u64, u64)> {
        self.cursor
            .as_ref()
            .map(|cursor| (cursor.head, cursor.epoch, cursor.content_objects))
    }
}

// --------------------------------------------------- the CanonicalMedium impl

impl CanonicalMedium for SqliteMedium {
    fn begin_reclamation_v2(
        &mut self,
        mk: &Hash,
        tick: u64,
    ) -> Result<crate::reclaim::BeginPublicationOutcome, MediumError> {
        // (V2X-CP2) The routing delegation to the trusted
        // transactional V2 admission — no rewiring of any
        // destructive or completion boundary.
        SqliteMedium::begin_reclamation_v2(self, mk, tick)
    }

    fn verify_begin_authority_v2(
        &mut self,
        mk: &Hash,
    ) -> Result<crate::reclaim::BeginAuthority, MediumError> {
        // (V2X-CP2) One consistent read snapshot for the whole
        // recovery-safe verification; the truthful tier captured
        // before the transaction borrows the connection.
        let tier = self.durability_tier();
        let tx = self
            .conn
            .transaction()
            .map_err(|e| sqlite_err("v2 begin-authority transaction", e))?;
        let result = verify_begin_authority_v2_tx(&tx, mk, tier);
        drop(tx);
        result
    }

    fn observe_range_terminal_under_coordination(
        &mut self,
        expected: &crate::range_completion::RangeCompletionContext,
        limits: &crate::range_installation::RangeInstallationLimits,
    ) -> Result<crate::range_installation::CoordinatedObservation, MediumError> {
        // (V2X-CP3-B-P2A-C5b) ONE consistent read snapshot for the
        // whole coordinated observation: the head read and the
        // entire C5a walk inside a single read transaction whose
        // snapshot binds at its first statement (a concurrent
        // writer's commit is either wholly visible before the
        // snapshot or wholly invisible after it — internal
        // consistency by the snapshot semantics, no new machinery).
        // Read-only: the transaction is dropped, never committed.
        let tx = self
            .conn
            .transaction()
            .map_err(|e| sqlite_err("range observation transaction", e))?;
        let result = observe_range_terminal_tx(&tx, expected, limits);
        drop(tx);
        result
    }

    fn checkpoint(&mut self) -> Result<(), MediumError> {
        self.checkpoint_truncate()
    }

    fn head(&mut self) -> Result<Option<Hash>, MediumError> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| sqlite_err("head read transaction", e))?;
        let observed = read_meta_head(&tx)?;
        validate_observed(&tx, &mut self.cursor, &mut self.walks, observed)?;
        tx.commit()
            .map_err(|e| sqlite_err("commit head read transaction", e))?;
        Ok(observed)
    }

    fn append_admitted(
        &mut self,
        admitted: crate::medium::AdmittedAppend,
        expected_parent: Option<Hash>,
    ) -> Result<AppendOutcome, MediumError> {
        let (record, objects, composite) = admitted.parts();
        let record = record.clone();
        let objects: Vec<StoredObject> = objects.to_vec();
        if self.read_only {
            return Err(MediumError::Sqlite {
                op: "append",
                detail: "the connection is read-only (the audit mode)".to_owned(),
            });
        }
        // Structural validation (pre-window; a failure is a clean,
        // retryable error — nothing has been written): the record must
        // be canonically representable and reference exactly the
        // supplied object set. Identical to the file medium's checks.
        let record_bytes = record.canonical_bytes();
        let reparsed = RevisionRecord::decode(&record_bytes).map_err(MediumError::Format)?;
        if reparsed != record {
            return Err(MediumError::Chain {
                rule: "record-noncanonical",
            });
        }
        let record_id = hash_object(&record_bytes);
        let supplied: BTreeSet<Hash> = objects.iter().map(StoredObject::hash).collect();
        let declared: BTreeSet<Hash> = record.content_refs.iter().copied().collect();
        if !composite && supplied != declared {
            return Err(MediumError::Chain {
                rule: "content-set-mismatch",
            });
        }
        if composite && !supplied.is_superset(&declared) {
            return Err(MediumError::Chain {
                rule: "composite-content-missing",
            });
        }

        // The transaction: BEGIN IMMEDIATE — the write lock. BUSY
        // here is contention (the bounded busy-timeout retry expired):
        // a typed abort — never a stale parent (no comparison ran),
        // never an uncertain commit (no work began).
        let tx = match self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
        {
            Ok(tx) => tx,
            Err(error) if is_busy(&error) => {
                return Err(MediumError::Contention {
                    detail: format!(
                        "the write lock was unavailable within the busy timeout at {}",
                        self.db_path.display()
                    ),
                });
            }
            Err(error) => return Err(sqlite_err("begin immediate", error)),
        };

        // The observed head, read under the transaction (a committed
        // read — the basis for every stale classification), then the
        // trusted-cursor validation of it.
        let observed = read_meta_head(&tx)?;
        if let Err(error) = validate_observed(&tx, &mut self.cursor, &mut self.walks, observed) {
            let _ = tx.rollback();
            return Err(error);
        }

        // The stale comparison — only after the in-transaction read.
        if observed != expected_parent {
            let _ = tx.rollback();
            return Ok(AppendOutcome::RejectedStaleParent { observed });
        }

        // The record-level coherence checks (against the validated
        // cursor — identical rules to the file medium's).
        match (observed, &record.kind) {
            (None, RecordKind::Founding { .. }) => {
                if record.epoch != 0 || record.parent.is_some() {
                    let _ = tx.rollback();
                    return Err(MediumError::Chain {
                        rule: "kind-incoherent",
                    });
                }
                if !record.depends_on.is_empty() {
                    let _ = tx.rollback();
                    return Err(MediumError::Chain {
                        rule: "depends-on-unresolvable",
                    });
                }
            }
            (Some(head), RecordKind::Regular) => {
                let Some(cursor) = self.cursor.as_ref() else {
                    unreachable!("validation established the cursor for a founded head")
                };
                // Checked arithmetic: an epoch at u64::MAX has no
                // valid successor and must fail closed, never wrap.
                if cursor.epoch.checked_add(1) != Some(record.epoch) {
                    let _ = tx.rollback();
                    return Err(MediumError::Chain {
                        rule: "epoch-regression",
                    });
                }
                if record.parent != Some(head) {
                    let _ = tx.rollback();
                    return Err(MediumError::Chain {
                        rule: "parent-mismatch",
                    });
                }
                if let Some(key) = &record.idempotency_key
                    && cursor.seen_keys.contains(key)
                {
                    let _ = tx.rollback();
                    return Err(MediumError::Chain {
                        rule: "duplicate-idempotency-key",
                    });
                }
                for dependency in &record.depends_on {
                    let resolves = cursor.record_ids.contains(dependency)
                        || cursor
                            .reclaimed
                            .get(dependency)
                            .is_some_and(|&dep_epoch| dep_epoch < record.epoch);
                    if !resolves {
                        let _ = tx.rollback();
                        return Err(MediumError::Chain {
                            rule: "depends-on-unresolvable",
                        });
                    }
                }
            }
            (observed, _) => {
                let _ = tx.rollback();
                return Ok(AppendOutcome::RejectedStaleParent { observed });
            }
        }

        // The K2-C2A PUBLICATION FENCE: revalidate every REUSED
        // composite dependency inside the write transaction,
        // immediately before the inserts — the admission's
        // caller-side reads are advisory; a dependency that vanished
        // (a concurrent connection's committed delete) or changed
        // between the admission and this publication REFUSES typed,
        // with the transaction rolled back and nothing installed.
        // The loader is the transaction-scoped hash-verifying read
        // the walks use — no public storage operation, no nested
        // transaction.
        if composite {
            let supplied: BTreeSet<Hash> = objects.iter().map(StoredObject::hash).collect();
            let fenced = crate::composite_append::fence_reused_dependencies(
                objects[0].bytes(),
                &supplied,
                |identity| load_referenced_object(&tx, identity),
            );
            if let Err(error) = fenced {
                let _ = tx.rollback();
                return Err(error);
            }
        }

        // The inserts: every object (insert-once, verify-on-retry),
        // then the record row. An in-window failure takes the rollback
        // path: a clean rollback is the typed error (nothing
        // installed); a failed rollback is Unknown.
        let transactional = append_inserts(
            &tx,
            &self.cursor,
            &record,
            &objects,
            &record_bytes,
            &record_id,
            &self.fault,
        );
        if let Err(error) = transactional {
            return rollback_or_unknown(tx, &self.fault, error);
        }

        // The head update — the transaction's final write.
        let updated = tx
            .execute(
                "UPDATE meta SET value = ?1 WHERE key = 'head'",
                params![record_id.as_bytes()],
            )
            .map_err(|e| sqlite_err("update head", e));
        match updated {
            Ok(1) => {}
            Ok(_) => {
                let error = MediumError::CorruptHead {
                    detail: "the head row vanished inside the transaction".to_owned(),
                };
                return rollback_or_unknown(tx, &self.fault, error);
            }
            Err(error) => return rollback_or_unknown(tx, &self.fault, error),
        }

        // The commit — the linearization and acceptance boundary. A
        // failure (including the injected one) is Unknown: the
        // disposition is established only by reconcile_unknown.
        if let Err(injected) = fault_check(&self.fault, SqliteFaultPoint::CommitFailure) {
            let _ = tx.rollback();
            let _ = injected;
            return Ok(AppendOutcome::Unknown {
                reason: UnknownReason::CommitUncertain,
            });
        }
        if let Err(error) = tx.commit() {
            let _ = error;
            return Ok(AppendOutcome::Unknown {
                reason: UnknownReason::CommitUncertain,
            });
        }

        // The post-commit window (the real-process kill point).
        if let Err(error) = fault_check(&self.fault, SqliteFaultPoint::AfterCommit) {
            let _ = error;
            return Ok(AppendOutcome::Unknown {
                reason: UnknownReason::CommitUncertain,
            });
        }

        // The trusted cursor advances over the accepted revision —
        // O(1) work; the R12 corrective carries the VERIFIED
        // composite closure (the manifest's identities — the
        // admission already verified every chunk; the manifest
        // re-decode here is the same bounded object).
        if composite {
            let manifest =
                composite_manifest::decode(objects[0].bytes()).map_err(MediumError::Format)?;
            advance_cursor(
                &mut self.cursor,
                &record,
                &record_id,
                Some(&manifest.closure_identities()),
            );
        } else {
            advance_cursor(&mut self.cursor, &record, &record_id, None);
        }
        Ok(AppendOutcome::Accepted {
            id: record_id,
            tier: SyncTier::TransactionalWalFull,
        })
    }

    fn read_record(&mut self, id: &Hash) -> Result<RevisionRecord, MediumError> {
        let bytes = load_record_verified(&self.conn, id)?;
        RevisionRecord::decode(&bytes).map_err(MediumError::Format)
    }

    fn read_object(&mut self, hash: &Hash) -> Result<Vec<u8>, MediumError> {
        load_object_verified(&self.conn, hash)
    }

    fn classify_object(&mut self, hash: &Hash) -> Result<ObjectClass, MediumError> {
        // The SAME full verified read as read_object — the complete
        // object bytes are loaded and hash-checked against the
        // identity (a missing object fails MissingObject, a corrupted
        // object fails HashMismatch); only the small classification
        // crosses back to the caller, eliminating the byte transfer
        // without eliminating the verification.
        let bytes = load_object_verified(&self.conn, hash)?;
        Ok(ObjectClass::of(&bytes))
    }

    fn validate(&mut self) -> Result<ChainSummary, MediumError> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| sqlite_err("validate read transaction", e))?;
        let observed = read_meta_head(&tx)?;
        let Some(head) = observed else {
            let _ = tx.rollback();
            return Err(MediumError::Chain { rule: "unfounded" });
        };
        // Sound invalidation first: the full walk rebuilds the cursor
        // from the durable truth alone.
        self.cursor = None;
        let walked = walk_chain(&tx, head)?;
        let summary = walked.summary;
        self.walks.cold_walk_records = summary.records;
        self.cursor = Some(walked.cursor);
        tx.commit()
            .map_err(|e| sqlite_err("commit validate read transaction", e))?;
        Ok(summary)
    }

    fn reconcile_unknown(&mut self, candidate: &Hash) -> Result<Reconciliation, MediumError> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| sqlite_err("reconcile read transaction", e))?;
        let observed = read_meta_head(&tx)?;
        let result = match observed {
            Some(head) => {
                // Full verification before any classification: the
                // cold walk establishes a fully valid authoritative
                // chain. Installed only when the candidate is HEAD or
                // a member of the walked revision-id set — a matching
                // parent hash alone is never proof. An inability to
                // validate is Unverifiable (never a false
                // confirmation, never a guess).
                match walk_chain(&tx, head) {
                    Ok(walked) => {
                        self.walks.last_reconcile_walked = walked.summary.records;
                        if head == *candidate || walked.cursor.record_ids.contains(candidate) {
                            Reconciliation::Installed { id: *candidate }
                        } else {
                            // Not installed in a verified chain: any
                            // candidate row is unreferenced
                            // content-addressed evidence — retained
                            // (never deleted), reused idempotently by
                            // an honest retry.
                            Reconciliation::NotInstalled { head: Some(head) }
                        }
                    }
                    Err(error) => Reconciliation::Unverifiable {
                        reason: error.to_string(),
                    },
                }
            }
            None => Reconciliation::NotInstalled { head: None },
        };
        tx.commit()
            .map_err(|e| sqlite_err("commit reconcile read transaction", e))?;
        Ok(result)
    }

    fn durability_tier(&self) -> SyncTier {
        SyncTier::TransactionalWalFull
    }

    /// (V2-W, W3A) The SQLite provider's effective per-object byte
    /// capacity: the connection's ACTUAL `SQLITE_MAX_LENGTH` runtime
    /// limit (the maximum length of any bound string or blob — the
    /// honest bound a reclamation publication's stored object must
    /// fit), queried live through the runtime limit API.
    fn effective_object_byte_limit(&mut self) -> Result<u64, MediumError> {
        let observed = self
            .conn
            .limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH)
            .map_err(|error| sqlite_err("query length limit", error))?;
        Ok(observed.max(0) as u64)
    }

    fn reclaim(
        &mut self,
        directive: &crate::reclaim::ReclaimDirective,
    ) -> Result<crate::reclaim::ReclaimOutcome, MediumError> {
        // THE TRUSTED BOUNDARY (K2-C2B P2): ONE write-serialized
        // transaction covers the authentication, the partition, and
        // the deletion — the decision and its destructive
        // consequences are atomic (an interruption is the full
        // rollback: nothing deleted, the retry from scratch; the
        // manifest-driven convergence holds trivially).
        if self.read_only {
            return Err(MediumError::Sqlite {
                op: "reclaim",
                detail: "the connection is read-only (the audit mode)".to_owned(),
            });
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| sqlite_err("begin reclaim transaction", e))?;
        let outcome = reclaim_authorized_tx(&tx, directive);
        // (E4) The cursor lifecycle: the mutation phase was reached —
        // the pre-retirement cursor is never authoritative afterward
        // (a rollback leaves nothing deleted, but the invalidated
        // state is uniformly safe; the next operation rebuilds via
        // the authenticated walk).
        self.cursor = None;
        let outcome = outcome?;
        // (OC-C3 CP-2) THE HONEST COMMIT UNCERTAINTY: an injected
        // CommitFailure (the established seam) rolls the transaction
        // back; a real COMMIT error leaves the rollback outcome
        // UNPROVEN (rusqlite 0.40.2's Drop-rollback failures are
        // silent). Both are the single typed
        // reclaim/commit-uncertain class — never success, never a
        // definite absence: the crossing STOPs, and the next
        // authorized pass independently re-verifies the begin and
        // the actual deletion state under a fresh transaction.
        if fault_check(&self.fault, SqliteFaultPoint::CommitFailure).is_err() {
            let _ = tx.rollback();
            return Err(MediumError::Chain {
                rule: "reclaim/commit-uncertain",
            });
        }
        if tx.commit().is_err() {
            return Err(MediumError::Chain {
                rule: "reclaim/commit-uncertain",
            });
        }
        Ok(outcome)
    }

    fn reclaim_pending(
        &mut self,
        directive: &crate::reclaim::ReclaimDirective,
    ) -> Result<bool, MediumError> {
        // The Mandatory-only probe (the advisory read over the plain
        // connection — the same logical authority contract).
        reclaim_pending_authorized(&self.conn, directive)
    }

    fn begin_reclamation(
        &mut self,
        audit: &Hash,
        mk: &Hash,
        tick: u64,
    ) -> Result<crate::reclaim::BeginPublicationOutcome, MediumError> {
        // (OC-C3 CP-1) The routing delegation to the accepted OC-C2
        // admission — UNCHANGED semantics, no rewiring of any
        // destructive or completion boundary.
        SqliteMedium::begin_reclamation(self, audit, mk, tick)
    }

    fn verify_begin_authority(
        &mut self,
        audit: &Hash,
        mk: &Hash,
    ) -> Result<crate::reclaim::BeginAuthority, MediumError> {
        // (OC-C3 CP-1) One consistent read snapshot for the whole
        // verification; the truthful tier captured before the
        // transaction borrows the connection.
        let tier = self.durability_tier();
        let tx = self
            .conn
            .transaction()
            .map_err(|e| sqlite_err("begin-authority transaction", e))?;
        let result = verify_begin_authority_tx(&tx, audit, mk, tier);
        drop(tx);
        result
    }
}

fn fault_check(
    fault: &Option<SqliteFaultControl>,
    point: SqliteFaultPoint,
) -> Result<(), MediumError> {
    match fault {
        Some(control) => control.check(point),
        None => Ok(()),
    }
}

fn sqlite_err(op: &'static str, error: rusqlite::Error) -> MediumError {
    MediumError::Sqlite {
        op,
        detail: error.to_string(),
    }
}

fn is_busy(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::DatabaseBusy,
                ..
            },
            _
        )
    )
}

/// The open-path write statements (the schema batch, the unfounded
/// head-row seed) take the same SQLite write lock the append's
/// `BEGIN IMMEDIATE` acquires: a busy-timeout expiry under a
/// concurrent writer is the SAME ratified contention condition —
/// the typed abort, never the generic surface. Every other error
/// keeps the generic `Sqlite` mapping (never a blanket acceptance;
/// the discrimination is exactly `is_busy`).
fn sqlite_err_or_contention(op: &'static str, error: rusqlite::Error) -> MediumError {
    if is_busy(&error) {
        MediumError::Contention {
            detail: format!(
                "the write lock was unavailable within the busy timeout \
                 while {op} — a concurrent writer holds the database"
            ),
        }
    } else {
        sqlite_err(op, error)
    }
}

fn is_constraint(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::ConstraintViolation,
                ..
            },
            _
        )
    )
}

/// Maps the query-result no-rows case to `None` (the absent-row
/// reads); every other error propagates.
fn no_rows_as_none<T>(error: rusqlite::Error) -> Result<Option<T>, rusqlite::Error> {
    match error {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    }
}

#[cfg(test)]
mod c5b_sqlite_tests;
#[cfg(test)]
mod c5d_sqlite_tests;
#[cfg(test)]
mod exclusion_tests;
#[cfg(test)]
mod tests;
