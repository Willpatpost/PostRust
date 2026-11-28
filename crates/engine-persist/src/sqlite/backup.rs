//! The quiescent capture boundary (split from the sqlite monolith
//! by change reason, P3-L9 pure move): the ratified TRUNCATE
//! checkpoint and the VACUUM INTO backup capture with its
//! journal-mode restoration. These change when the lifecycle
//! capture discipline changes.

use std::path::Path;

use rusqlite::{Connection, params};

use crate::medium::MediumError;

use super::{SqliteMedium, sqlite_err};

impl SqliteMedium {
    /// The ratified `TRUNCATE` checkpoint — the graceful lifecycle
    /// boundary operation (the session close, the backup capture).
    /// Returns fail-closed typed errors; never a destructive repair.
    pub fn checkpoint_truncate(&mut self) -> Result<(), MediumError> {
        let (busy, _log, _checkpointed): (i64, i64, i64) = self
            .conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(|e| sqlite_err("wal_checkpoint(TRUNCATE)", e))?;
        if busy != 0 {
            return Err(MediumError::Contention {
                detail: "the TRUNCATE checkpoint could not obtain the checkpoint lock".to_owned(),
            });
        }
        Ok(())
    }
    /// §6.2's capture (PP-B1): the quiescent backup boundary — fold
    /// the WAL into the main database file (the ratified TRUNCATE
    /// checkpoint; the caller's quiescence contract — no canonical
    /// write, journal write, or rotation in flight across the
    /// capture — makes it uncontended), then `VACUUM INTO` the
    /// target: a complete, self-contained snapshot of the validated
    /// schema and chain. The target must not exist (a prior file is
    /// never quietly overwritten). Online (concurrent-writer) backup
    /// is deferred by design — this is the offline tool's capture.
    pub fn backup_into(&mut self, target: &Path) -> Result<(), MediumError> {
        self.checkpoint_truncate()?;
        self.conn
            .execute(
                "VACUUM INTO ?1",
                params![target.to_string_lossy().to_string()],
            )
            .map_err(|e| sqlite_err("VACUUM INTO", e))?;
        // The captured file must carry the medium's own persistent
        // journal mode: `VACUUM INTO` emits a rollback-journal
        // database, and the read-only audit open verifies WAL (never
        // sets it). Restore the mode on the capture itself — set,
        // then verified by reading it back (never assumed granted);
        // the clean close folds any sidecar away.
        let capture = Connection::open(target).map_err(|e| sqlite_err("open the capture", e))?;
        capture
            .execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| sqlite_err("set the capture's journal mode", e))?;
        let mode: String = capture
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .map_err(|e| sqlite_err("verify the capture's journal mode", e))?;
        drop(capture);
        if mode != "wal" {
            return Err(MediumError::Sqlite {
                op: "set the capture's journal mode",
                detail: format!("expected wal, observed {mode}"),
            });
        }
        Ok(())
    }
}
