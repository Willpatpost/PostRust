//! The quiescent capture boundary (split from the sqlite monolith
//! by change reason, P3-L9 pure move): the ratified TRUNCATE
//! checkpoint and the VACUUM INTO backup capture with its
//! journal-mode restoration. These change when the lifecycle
//! capture discipline changes.

use std::path::Path;

use rusqlite::Connection;

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
    /// capture — makes it uncontended), then copy the complete
    /// validated schema and chain to the target through SQLite's
    /// **native backup API**. The target must not exist (a prior file
    /// is never quietly overwritten — the typed refusal below; the
    /// offline boundary and the caller's fresh-output-directory rule
    /// bound the check-then-create window). Online
    /// (concurrent-writer) backup is deferred by design — this is the
    /// offline tool's capture.
    ///
    /// (CA-19) **ONE PATH REPRESENTATION, LOSSLESS:** the capture
    /// used to pass a lossy `to_string_lossy()` string to
    /// `VACUUM INTO ?1` (SQL-text path) and then open the ORIGINAL
    /// native `Path` — two conversions that can identify DIFFERENT
    /// files wherever the OS allows non-UTF-8 names (Unix raw bytes;
    /// Windows unpaired surrogates): `VACUUM INTO` would write the
    /// substituted-name sibling while the native open failed — or
    /// worse, on Unix, silently targeted a different file than the
    /// one later read. The native `Connection::backup` API crosses
    /// no SQL-text boundary at all: the target crosses as
    /// `AsRef<Path>` into `Connection::open`, whose conversion is
    /// lossless on Unix (raw `OsStr` bytes) and a typed
    /// `InvalidPath` refusal on Windows (never a substitution) — one
    /// representation, one conversion site, fail-closed everywhere.
    /// The pages come out byte-comparable to the source's logical
    /// content (the page copy, not `VACUUM INTO`'s compaction — the
    /// acceptance is the chain validation, which the caller runs;
    /// the capture's own journal-mode restoration below is
    /// unchanged).
    pub fn backup_into(&mut self, target: &Path) -> Result<(), MediumError> {
        self.checkpoint_truncate()?;
        if target.try_exists().map_err(|e| MediumError::Io {
            op: "probe the backup target",
            path: target.display().to_string(),
            source: e,
        })? {
            return Err(MediumError::Sqlite {
                op: "backup_into",
                detail: format!(
                    "the capture target {} already exists — a prior file is \
                     never quietly overwritten",
                    target.display()
                ),
            });
        }
        self.conn
            .backup("main", target, None)
            .map_err(|e| sqlite_err("backup the capture", e))?;
        // (CA-20) The capture's own artifact flush: the pages are
        // SQLite's writes into a fresh file — flush them explicitly
        // so the capture function's contract is "a complete,
        // flushed file", never "bytes the pager may still hold"
        // (whatever SQLite's own sync discipline did, this is
        // provable from the caller).
        {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(target)
                .map_err(|e| MediumError::Io {
                    op: "flush the capture",
                    path: target.display().to_string(),
                    source: e,
                })?;
            file.sync_all().map_err(|e| MediumError::Io {
                op: "flush the capture",
                path: target.display().to_string(),
                source: e,
            })?;
        }
        // The captured file must carry the medium's own persistent
        // journal mode: the page copy carries the source's WAL header,
        // and the read-only audit open verifies WAL (never sets it).
        // Restore the mode on the capture itself — set, then verified
        // by reading it back (never assumed granted); the clean close
        // folds any sidecar away.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::medium::CanonicalMedium;
    use crate::record::{AuthorityMode, RevisionRecord};
    use crate::sqlite::SqliteMedium;
    use std::path::PathBuf;

    /// A unique temp db path (the sqlite_tests fixture discipline,
    /// local to this module; no `tempfile` dependency).
    fn temp_db(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "s7-ca19-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create the test directory");
        dir.join("store.db")
    }

    /// Removes the db's directory and its WAL/SHM sidecars (the
    /// sqlite_tests cleanup discipline, local; the dedicated
    /// directory keeps sibling assertions meaningful).
    fn cleanup_db(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
        let mut wal = path.as_os_str().to_os_string();
        wal.push("-wal");
        let _ = std::fs::remove_file(&wal);
        let mut shm = path.as_os_str().to_os_string();
        shm.push("-shm");
        let _ = std::fs::remove_file(&shm);
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }

    /// A founded medium carrying a small real chain (the
    /// sqlite_tests fixture discipline, at the capture's scale).
    fn founded_chain(label: &str) -> (PathBuf, SqliteMedium) {
        let path = temp_db(label);
        let mut medium = SqliteMedium::open(&path).unwrap();
        let founding =
            RevisionRecord::founding(AuthorityMode::Standalone, "ca19-capture-campaign", 0);
        medium.append(&founding, &[], None).unwrap();
        let record = RevisionRecord::regular(founding.id(), 0, 140);
        medium.append(&record, &[], Some(founding.id())).unwrap();
        (path, medium)
    }

    /// (CA-19) The native capture is the accepted mechanism's equal:
    /// the captured database validates to the source's exact chain
    /// summary — the acceptance the caller runs (the backup's
    /// coherence check), unchanged by the page-copy-for-VACUUM
    /// mechanism swap.
    #[test]
    fn the_native_capture_validates_to_the_source_chain() {
        let (path, mut medium) = founded_chain("ca19-native-capture");
        let summary = medium.validate().unwrap();
        let target = path.with_file_name("ca19-native-target.db");
        medium.backup_into(&target).unwrap();
        let mut captured = SqliteMedium::open_read_only(&target).unwrap();
        assert_eq!(captured.validate().unwrap(), summary);
        drop(captured);
        drop(medium);
        cleanup_db(&path);
        cleanup_db(&target);
    }

    /// (CA-19) The never-quiet-overwrite contract `VACUUM INTO` gave
    /// for free is now the capture's own typed refusal: an existing
    /// target refuses before any page is written.
    #[test]
    fn the_capture_refuses_an_existing_target() {
        let (path, mut medium) = founded_chain("ca19-existing-target");
        let target = path.with_file_name("ca19-existing-target.db");
        std::fs::write(&target, b"prior evidence - never overwritten").unwrap();
        assert!(matches!(
            medium.backup_into(&target),
            Err(MediumError::Sqlite {
                op: "backup_into",
                ..
            })
        ));
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"prior evidence - never overwritten",
            "the prior file is untouched"
        );
        drop(medium);
        cleanup_db(&path);
        let _ = std::fs::remove_file(&target);
    }

    /// (CA-19, Windows) THE NON-UNICODE TARGET: a path carrying an
    /// unpaired surrogate (constructible through the WTF-16 OS
    /// surface Windows paths are) is NOT representable as UTF-8. The
    /// old shape passed `to_string_lossy()` to `VACUUM INTO` — the
    /// substituted U+FFFD name — while the native open failed: two
    /// files, one wrong. The native API refuses the path typed
    /// (`InvalidPath`, before any file is created) and leaves NO
    /// sibling under the substituted name: the parent directory
    /// stays empty.
    #[cfg(windows)]
    #[test]
    fn the_non_representable_target_refuses_leaving_no_substituted_sibling() {
        use std::os::windows::ffi::OsStringExt as _;

        let (path, mut medium) = founded_chain("ca19-non-unicode");
        let parent = path.parent().unwrap().to_path_buf();
        // The target's name carries an unpaired surrogate — legal in
        // a Windows path, impossible in UTF-8.
        let name = std::ffi::OsString::from_wide(&[0xD800, b'D' as u16, b'B' as u16]);
        let target = parent.join(name);
        assert!(
            target.to_str().is_none(),
            "the fixture must be non-representable (an unpaired surrogate)"
        );
        assert!(medium.backup_into(&target).is_err(), "the capture refuses");
        // NO SUBSTITUTED SIBLING: the lossy conversion's U+FFFD name
        // never materialized — the refusal happened before any file
        // was created, and the parent holds only the source medium's
        // own files.
        let substituted = target.to_string_lossy().to_string();
        let entries: Vec<std::ffi::OsString> = std::fs::read_dir(&parent)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(
            entries.iter().all(|name| {
                name.to_string_lossy() != substituted
                    && !name.to_string_lossy().contains('\u{FFFD}')
            }),
            "the parent holds no substituted sibling: {entries:?}"
        );
        assert!(
            !entries.is_empty(),
            "the source medium's own files are still present: {entries:?}"
        );
        drop(medium);
        cleanup_db(&path);
    }

    /// (CA-19, Unix) The native path is BYTE-FAITHFUL where the OS
    /// allows it: a non-UTF-8 target (raw invalid-UTF-8 bytes) backs
    /// up AT THE NATIVE NAME — the file exists under exactly those
    /// bytes and validates to the source chain — and no substituted
    /// (U+FFFD) sibling exists.
    #[cfg(unix)]
    #[test]
    fn the_non_utf8_target_backs_up_at_the_native_name() {
        use std::os::unix::ffi::OsStrExt as _;

        let (path, mut medium) = founded_chain("ca19-non-unicode");
        let parent = path.parent().unwrap().to_path_buf();
        let name = std::ffi::OsString::from_vec(vec![0xFF, b'.', b'd', b'b']);
        let target = parent.join(name);
        assert!(target.to_str().is_none(), "the fixture must be non-UTF-8");
        let summary = medium.validate().unwrap();
        medium.backup_into(&target).unwrap();
        assert!(target.exists(), "the capture exists at the native bytes");
        let mut captured = SqliteMedium::open_read_only(&target).unwrap();
        assert_eq!(captured.validate().unwrap(), summary);
        drop(captured);
        drop(medium);
        cleanup_db(&path);
        let _ = std::fs::remove_file(&target);
    }
}
