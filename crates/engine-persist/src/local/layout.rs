//! The store's physical layout and OS lock discipline (split from the
//! local monolith by change reason, P3-L9 pure move): the open ladder
//! that creates the documented directory shape, the single-writer
//! HEAD.lock pair, the content-addressed object-path resolution, and
//! the hash-verified object reads over the layout. These change when
//! the physical store's shape or its lock protocol changes; the wire
//! constants stay re-exported at the module root (the shared contract
//! surface, the S4 WAIT_TIMEOUT_NS precedent).

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use crate::fault::FaultControl;
use crate::hash::{Hash, hash_object};
use crate::medium::{MediumError, SyncTier};
use crate::record::AuthorityMode;

use super::{
    AppendTimings, HEAD_LOCK, LocalFileMedium, MAX_OBJECT_BYTES, MIGRATED_MARKER, OBJECTS_DIR,
    QUARANTINE_DIR, WalkStats, io_err,
};

impl LocalFileMedium {
    /// Opens (creating the layout) and performs the session open: the
    /// cold founding → HEAD validation and the unreferenced-object
    /// sweep, both under the lock.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, MediumError> {
        Self::open_with(root, None)
    }

    /// Test-infrastructure constructor with the crash-window fault
    /// seam armed (production uses [`Self::open`]).
    pub fn open_with(
        root: impl Into<PathBuf>,
        fault: Option<FaultControl>,
    ) -> Result<Self, MediumError> {
        let root = root.into();
        // The §6.1 seal admission (PP-B1): a MIGRATED tombstone means
        // this directory is archived evidence, never a live source —
        // refuse before touching anything.
        if root.join(MIGRATED_MARKER).is_file() {
            return Err(MediumError::Sealed {
                detail: format!(
                    "the store at {} was migrated offline — the directory is \
                     archived evidence, never a live campaign source",
                    root.display()
                ),
            });
        }
        let objects = root.join(OBJECTS_DIR);
        let quarantine = root.join(QUARANTINE_DIR);
        fs::create_dir_all(&objects).map_err(|e| io_err("create objects dir", &objects, e))?;
        fs::create_dir_all(&quarantine)
            .map_err(|e| io_err("create quarantine dir", &quarantine, e))?;
        let lock_path = root.join(HEAD_LOCK);
        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| io_err("open HEAD.lock", &lock_path, e))?;
        let mut medium = LocalFileMedium {
            root,
            lock_file,
            cursor: None,
            fault,
            timings: AppendTimings::default(),
            walks: WalkStats::default(),
            platform_tier: platform_tier(),
        };
        // Session open — no fault points fire here: the crash matrix
        // targets the append/reconcile paths.
        medium.lock()?;
        let opened = match medium.read_head_file() {
            Ok(Some(id)) => {
                medium.cold_walk_from(id)?;
                medium.sweep_unreferenced()?;
                Ok(())
            }
            Ok(None) => {
                // No authoritative HEAD (never founded, or an
                // interrupted founding attempt): nothing is
                // referenced — sweep every object into quarantine,
                // preserving the attempt's evidence without
                // manufacturing a founding revision.
                medium.sweep_unreferenced().map(|_| ())
            }
            Err(error) => Err(error),
        };
        let _ = medium.unlock();
        opened.map(|()| medium)
    }
    /// The store root path (the PP-B1 provider seam's location
    /// reporting — diagnostics and the fresh-handle reuse).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The campaign identity established by the session open (or a
    /// later validated append) — founding id, authority mode, and
    /// campaign id, available without re-walking. `None` before the
    /// store is founded.
    pub fn campaign(&self) -> Option<(Hash, AuthorityMode, &str)> {
        self.cursor.as_ref().map(|cursor| {
            (
                cursor.founding,
                cursor.authority,
                cursor.campaign_id.as_str(),
            )
        })
    }

    // ------------------------------------------------------------ locking

    pub(super) fn lock(&self) -> Result<(), MediumError> {
        self.lock_file
            .lock()
            .map_err(|e| io_err("lock HEAD.lock", &self.root.join(HEAD_LOCK), e))
    }

    pub(super) fn unlock(&self) -> Result<(), MediumError> {
        self.lock_file
            .unlock()
            .map_err(|e| io_err("unlock HEAD.lock", &self.root.join(HEAD_LOCK), e))
    }
    // ----------------------------------------------------------- internals

    pub(super) fn object_path(&self, hash: &Hash) -> PathBuf {
        self.root.join(OBJECTS_DIR).join(hash.hex())
    }

    /// The test-visible object-namespace path (the OC-C1 reader
    /// fixtures hand-write begin-family objects the publication
    /// gates rightly refuse — the content-addressed layout is the
    /// store's own, never a publication route).
    #[cfg(any(test, feature = "composite-test-fixture"))]
    #[doc(hidden)]
    pub fn object_path_pub(&self, hash: &Hash) -> PathBuf {
        self.root.join(OBJECTS_DIR).join(hash.hex())
    }

    pub(super) fn load_object(&self, hash: &Hash) -> Result<Vec<u8>, MediumError> {
        let path = self.object_path(hash);
        let metadata = fs::metadata(&path).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => MediumError::MissingObject { hash: *hash },
            _ => io_err("stat object", &path, e),
        })?;
        if metadata.len() > MAX_OBJECT_BYTES {
            return Err(MediumError::Format(engine_core::SaveError::Limit {
                section: "object-store",
                kind: "byte-length",
                value: metadata.len(),
                max: MAX_OBJECT_BYTES,
            }));
        }
        let bytes = fs::read(&path).map_err(|e| io_err("read object", &path, e))?;
        let observed = hash_object(&bytes);
        if observed != *hash {
            return Err(MediumError::HashMismatch {
                expected: *hash,
                observed,
                context: "object-store",
            });
        }
        Ok(bytes)
    }

    /// Loads an object a validation walk reached by reference: a hash
    /// mismatch here is referenced corruption — fail closed with the
    /// specific condition (the file is never replaced or deleted).
    pub(super) fn load_referenced(&self, hash: &Hash) -> Result<Vec<u8>, MediumError> {
        self.load_object(hash).map_err(|error| match error {
            MediumError::HashMismatch { .. } => {
                MediumError::ReferencedObjectCorrupt { hash: *hash }
            }
            other => other,
        })
    }
    pub(super) fn read_head_file(&self) -> Result<Option<Hash>, MediumError> {
        super::read_head_at(&self.root)
    }
}

fn platform_tier() -> SyncTier {
    if cfg!(windows) {
        SyncTier::ProcessCrashOnly
    } else {
        SyncTier::FullPosixSequence
    }
}
