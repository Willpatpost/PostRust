//! The schema and the meta rows (split from the sqlite monolith by
//! change reason, P3-L9 pure move): the ratified DDL, the meta row
//! keys, the head row read, the authority and provenance rows, the
//! migration-pending lifecycle, the writable-connection
//! configuration, and the transactional migration activation.
//! These change when the database's own metadata shape changes.

use rusqlite::{Connection, TransactionBehavior, params};

use engine_core::{Decoder, Encoder};

use crate::hash::Hash;
use crate::medium::MediumError;
use crate::record::AuthorityMode;

use super::walks::SqliteCursor;
use super::{SqliteMedium, SqliteMediumConfig, no_rows_as_none, sqlite_err};

/// The `meta` row keys (the design's §3.1 layout).
const META_HEAD: &str = "head";
pub(super) const META_AUTHORITY: &str = "authority";
#[allow(dead_code)]
pub(super) const META_PROVENANCE: &str = "provenance";

/// §6.1's migration-target lifecycle (the PP-B1 review corrective):
/// present while the database is an unactivated migration target —
/// the normal opens refuse it; only `open_migration_target`
/// proceeds, and the activation removes it transactionally.
pub(super) const META_MIGRATION_PENDING: &str = "migration-pending";

/// The ratified schema (the design's §3.1 DDL, `IF NOT EXISTS` for
/// open-time idempotence — two concurrent openers serialize on
/// SQLite's own write lock). The `(32)`-style length annotations of
/// the design are documentation: the insert paths enforce the hash
/// width structurally (`Hash` is exactly 32 bytes).
pub(super) const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS objects (
  hash   BLOB PRIMARY KEY,
  domain TEXT NOT NULL,
  bytes  BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS records (
  hash   BLOB PRIMARY KEY,
  bytes  BLOB NOT NULL,
  epoch  INTEGER NOT NULL,
  parent BLOB,
  idem   BLOB,
  kind   INTEGER NOT NULL,
  tick   INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS records_idem_unique
  ON records(idem) WHERE idem IS NOT NULL;
CREATE INDEX IF NOT EXISTS records_parent ON records(parent);
CREATE INDEX IF NOT EXISTS records_epoch  ON records(epoch);
";

impl SqliteMedium {
    /// §6.1's activation (PP-B1, the review corrective — now
    /// transactionally coherent): ONE `BEGIN IMMEDIATE` transaction
    /// that reads the committed head and verifies it equals the
    /// sealed source's head, REQUIRES the `migration-pending` marker
    /// (the target lifecycle's precondition — an activation without
    /// a pending target is an incoherent lifecycle), writes the
    /// provenance row write-once (a contradiction is fail-closed),
    /// and REMOVES the pending marker — the complete
    /// pending→activated transition commits atomically. The caller
    /// owns the seal-first ordering (the legacy `MIGRATED` tombstone
    /// must be verified before this is called).
    pub fn mark_migrated_from(&mut self, source_head: &Hash) -> Result<(), MediumError> {
        let encoded = provenance_bytes(source_head);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| sqlite_err("provenance transaction", e))?;
        // The binding head is verified INSIDE the transaction — the
        // activation cannot interleave with a concurrent head change.
        let observed = read_meta_head(&tx)?;
        if observed != Some(*source_head) {
            return Err(MediumError::Chain { rule: "divergence" });
        }
        // The lifecycle state machine: a FRESH activation requires
        // the pending marker (the target lifecycle's precondition —
        // an activation without a pending target or a completed
        // binding is an incoherent lifecycle); a COMPLETED
        // activation (the provenance row already binding this
        // migration — the interruption between the activation and
        // the archive) is the idempotent no-op.
        let pending = read_meta_row(&tx, META_MIGRATION_PENDING)?;
        match read_meta_row(&tx, META_PROVENANCE)? {
            None => {
                let marker = pending.ok_or_else(|| MediumError::Sqlite {
                    op: "mark_migrated_from",
                    detail: "the database carries no migration-pending marker \
                                 and no completed activation — it is not an \
                                 unactivated migration target"
                        .to_owned(),
                })?;
                decode_migration_pending(&marker)?;
                tx.execute(
                    "INSERT INTO meta (key, value) VALUES (?1, ?2)",
                    params![META_PROVENANCE, encoded],
                )
                .map_err(|e| sqlite_err("insert provenance row", e))?;
                // The lifecycle transition: the pending marker
                // leaves with the activation — one commit.
                tx.execute(
                    "DELETE FROM meta WHERE key = ?1",
                    params![META_MIGRATION_PENDING],
                )
                .map_err(|e| sqlite_err("clear migration-pending row", e))?;
            }
            Some(existing) => {
                if existing != encoded {
                    return Err(MediumError::CorruptHead {
                        detail: "the provenance row contradicts this migration".to_owned(),
                    });
                }
                // The completed activation — idempotent. A leftover
                // pending marker (an impossible mid-transaction state,
                // evidence of tampering) still clears.
                if pending.is_some() {
                    tx.execute(
                        "DELETE FROM meta WHERE key = ?1",
                        params![META_MIGRATION_PENDING],
                    )
                    .map_err(|e| sqlite_err("clear migration-pending row", e))?;
                }
            }
        }
        tx.commit()
            .map_err(|e| sqlite_err("commit provenance row", e))?;
        Ok(())
    }
}

/// Reads and verifies the `meta.head` row: the empty blob is the
/// unfounded sentinel; exactly 32 bytes is the head record's
/// identity; anything else is fail-closed corruption.
pub(super) fn read_meta_head(conn: &Connection) -> Result<Option<Hash>, MediumError> {
    let value: Vec<u8> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![META_HEAD],
            |row| row.get(0),
        )
        .map_err(|e| sqlite_err("read head row", e))?;
    match value.len() {
        0 => Ok(None),
        32 => Hash::from_bytes(&value)
            .map(Some)
            .ok_or_else(|| MediumError::CorruptHead {
                detail: "head hash width".to_owned(),
            }),
        _ => Err(MediumError::CorruptHead {
            detail: format!("head row has {} bytes (expected 0 or 32)", value.len()),
        }),
    }
}
/// The §6.1 provenance row's value: the sealed legacy chain's head
/// (the binding the activation writes once; the row's key carries
/// its meaning, the authority-row precedent).
fn provenance_bytes(source_head: &Hash) -> Vec<u8> {
    source_head.as_bytes().to_vec()
}

/// Reads a meta row by key (`None` when absent).
pub(super) fn read_meta_row(conn: &Connection, key: &str) -> Result<Option<Vec<u8>>, MediumError> {
    conn.query_row(
        "SELECT value FROM meta WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .map(Some)
    .or_else(no_rows_as_none)
    .map_err(|e| sqlite_err("read meta row", e))
}

/// The migration-pending marker's wire domain (§6.1's target
/// lifecycle — the review corrective).
const MIGRATION_PENDING_DOMAIN: &str = "sektor7/engine-persist/migration-pending/1";
const MIGRATION_PENDING_VERSION: u32 = 1;

/// The pending marker's canonical bytes: the domain and its version.
pub(super) fn migration_pending_bytes() -> Vec<u8> {
    let mut enc = Encoder::new();
    enc.write_str(MIGRATION_PENDING_DOMAIN);
    enc.write_u32(MIGRATION_PENDING_VERSION);
    enc.into_bytes()
}

/// Decodes and verifies the pending marker (a corrupt marker is
/// fail-closed `CorruptHead` — the lifecycle evidence is never
/// guessed).
pub(super) fn decode_migration_pending(bytes: &[u8]) -> Result<(), MediumError> {
    let corrupt = |detail: String| MediumError::CorruptHead { detail };
    let mut dec = Decoder::new(bytes);
    dec.read_domain(MIGRATION_PENDING_DOMAIN)
        .map_err(|e| corrupt(format!("the migration-pending row: {e}")))?;
    let version = dec
        .read_u32()
        .map_err(|e| corrupt(format!("the migration-pending row: {e}")))?;
    if version != MIGRATION_PENDING_VERSION {
        return Err(corrupt(format!(
            "unsupported migration-pending version {version}"
        )));
    }
    dec.finish()
        .map_err(|e| corrupt(format!("the migration-pending row: {e}")))?;
    Ok(())
}

/// The authority row's encoding: `[32-byte founding hash][1-byte
/// authority mode][campaign id UTF-8]`.
pub(super) fn authority_bytes(
    founding: &Hash,
    authority: AuthorityMode,
    campaign_id: &str,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(Hash::WIDTH as usize + 1 + campaign_id.len());
    bytes.extend_from_slice(founding.as_bytes());
    bytes.push(match authority {
        AuthorityMode::Standalone => 0u8,
        AuthorityMode::Shared => 1u8,
    });
    bytes.extend_from_slice(campaign_id.as_bytes());
    bytes
}

/// The open-time cross-check: a founded chain must carry the matching
/// authority row; an unfounded store must not carry one. A
/// contradiction is fail-closed (the metadata was tampered with).
pub(super) fn verify_authority_row(
    conn: &Connection,
    cursor: Option<&SqliteCursor>,
) -> Result<(), MediumError> {
    let present: Option<Vec<u8>> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![META_AUTHORITY],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(no_rows_as_none)
        .map_err(|e| sqlite_err("read authority row", e))?;
    match (cursor, present) {
        (None, None) => Ok(()),
        (Some(cursor), Some(row)) => {
            let expected = authority_bytes(&cursor.founding, cursor.authority, &cursor.campaign_id);
            if row == expected {
                Ok(())
            } else {
                Err(MediumError::CorruptHead {
                    detail: "the authority row contradicts the validated chain".to_owned(),
                })
            }
        }
        (Some(_), None) => Err(MediumError::CorruptHead {
            detail: "a founded chain is missing the authority row".to_owned(),
        }),
        (None, Some(_)) => Err(MediumError::CorruptHead {
            detail: "an unfounded store carries an authority row".to_owned(),
        }),
    }
}

// ------------------------------------------------------------- utilities

/// Configures the writable connection: WAL (set and verified — the
/// statement returns the resulting mode), FULL (set as a pure setter
/// — the statement returns no row — then **verified by reading the
/// actual configuration back**, the numeric form 2), and the busy
/// timeout. A request is never assumed granted. THE TIMEOUT FIRST:
/// the busy timeout is installed BEFORE the journal-mode transition
/// — the transition may have to recover a killed writer's hot WAL
/// (taking locks momentarily), and without the timeout any transient
/// contention is an immediate `SQLITE_BUSY` (the CI flake of the
/// interrupted-run relaunch at be6ffe9's first dispatch); with it,
/// the recovery waits within the established bound.
pub(super) fn configure_writable(
    conn: &Connection,
    config: &SqliteMediumConfig,
) -> Result<(), MediumError> {
    conn.busy_timeout(config.busy_timeout)
        .map_err(|e| sqlite_err("set busy_timeout", e))?;
    let mode: String = conn
        .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
        .map_err(|e| sqlite_err("set journal_mode", e))?;
    if mode != "wal" {
        return Err(MediumError::Sqlite {
            op: "verify journal_mode",
            detail: format!("expected wal, observed {mode}"),
        });
    }
    conn.execute_batch("PRAGMA synchronous=FULL")
        .map_err(|e| sqlite_err("set synchronous", e))?;
    let sync: i64 = conn
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .map_err(|e| sqlite_err("verify synchronous", e))?;
    if sync != 2 {
        return Err(MediumError::Sqlite {
            op: "verify synchronous",
            detail: format!("expected 2 (FULL), observed {sync}"),
        });
    }
    Ok(())
}
