//! The store's physical layout and OS lock discipline (split from the
//! local monolith by change reason, P3-L9 pure move): the open ladder
//! that creates the documented directory shape, the single-writer
//! HEAD.lock pair, the content-addressed object-path resolution, and
//! the hash-verified object reads over the layout. These change when
//! the physical store's shape or its lock protocol changes; the wire
//! constants stay re-exported at the module root (the shared contract
//! surface, the S4 WAIT_TIMEOUT_NS precedent).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
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
        object_path_at(&self.root, hash)
    }

    /// The test-visible object-namespace path (the OC-C1 reader
    /// fixtures hand-write begin-family objects the publication
    /// gates rightly refuse — the content-addressed layout is the
    /// store's own, never a publication route).
    #[cfg(any(test, feature = "composite-test-fixture"))]
    #[doc(hidden)]
    pub fn object_path_pub(&self, hash: &Hash) -> PathBuf {
        object_path_at(&self.root, hash)
    }

    pub(super) fn load_object(&self, hash: &Hash) -> Result<Vec<u8>, MediumError> {
        load_object_at(&self.root, hash)
    }

    /// Loads an object a validation walk reached by reference: a hash
    /// mismatch here is referenced corruption — fail closed with the
    /// specific condition (the file is never replaced or deleted).
    pub(super) fn load_referenced(&self, hash: &Hash) -> Result<Vec<u8>, MediumError> {
        load_referenced_at(&self.root, hash)
    }
    pub(super) fn read_head_file(&self) -> Result<Option<Hash>, MediumError> {
        super::read_head_at(&self.root)
    }
}

// -------------------------------------------------- the root-path loads
// (P3-L9b) The free-function forms of the layout's loads — the shared
// walk core's local reader adapter holds a `&Path` (disjoint from the
// provider's `&mut` cursor borrow), so the loads are expressed over
// the root rather than the medium.

/// The content-addressed object path under a store root.
pub(super) fn object_path_at(root: &Path, hash: &Hash) -> PathBuf {
    root.join(OBJECTS_DIR).join(hash.hex())
}

/// The hash-verified object load under a store root.
pub(super) fn load_object_at(root: &Path, hash: &Hash) -> Result<Vec<u8>, MediumError> {
    let path = object_path_at(root, hash);
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

/// The referenced-corruption escalation of the object load under a
/// store root.
pub(super) fn load_referenced_at(root: &Path, hash: &Hash) -> Result<Vec<u8>, MediumError> {
    load_object_at(root, hash).map_err(|error| match error {
        MediumError::HashMismatch { .. } => MediumError::ReferencedObjectCorrupt { hash: *hash },
        other => other,
    })
}

// ------------------------------------------------- (T29) the streaming reads

/// (T29) Reads a bounded prefix of an object's bytes under a store
/// root — the UNVERIFIED prefix op (the streaming verify's leading
/// window; hash verification NOT implied, never a substitute for it).
/// The layout's own load semantics govern the failure classes:
/// NotFound → `MissingObject`, over-limit → `Format`, read failure →
/// the typed `Io`; a shorter-than-requested result means the object
/// itself is shorter (EOF is data here, never an error).
pub(super) fn read_object_prefix_at(
    root: &Path,
    hash: &Hash,
    len: usize,
) -> Result<Vec<u8>, MediumError> {
    let path = object_path_at(root, hash);
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
    let mut file = File::open(&path).map_err(|e| io_err("read object", &path, e))?;
    let want = len.min(usize::try_from(metadata.len()).unwrap_or(len));
    let mut buffer = vec![0u8; want];
    let mut read = 0usize;
    while read < buffer.len() {
        let n = file
            .read(&mut buffer[read..])
            .map_err(|e| io_err("read object", &path, e))?;
        if n == 0 {
            buffer.truncate(read);
            break;
        }
        read += n;
    }
    Ok(buffer)
}

/// (T29) Opens an object for positioned continuation reads at
/// `offset` — the streaming verify's window source after the leading
/// prefix window (sequential reads from the seek position; the medium
/// is immutable within the walk's single coordination scope).
pub(super) fn open_object_window_at(
    root: &Path,
    hash: &Hash,
    offset: u64,
) -> Result<(File, PathBuf), MediumError> {
    let path = object_path_at(root, hash);
    let mut file = File::open(&path).map_err(|e| io_err("read object", &path, e))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| io_err("seek object", &path, e))?;
    Ok((file, path))
}

fn platform_tier() -> SyncTier {
    if cfg!(windows) {
        SyncTier::ProcessCrashOnly
    } else {
        SyncTier::FullPosixSequence
    }
}
