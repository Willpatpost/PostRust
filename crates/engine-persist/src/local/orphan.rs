//! The v1.4 orphan protocol (split from the local monolith by change
//! reason, P3-L9 pure move): quarantine-with-evidence-never-delete,
//! the collision-safe incident naming, and the session-open
//! unreferenced-object sweep. These change when the evidence
//! preservation rules change.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};

use crate::hash::Hash;
use crate::medium::MediumError;

use super::{LocalFileMedium, OBJECTS_DIR, QUARANTINE_DIR, io_err};

impl LocalFileMedium {
    // ---------------------------------------------------- orphan protocol

    /// Moves an object into quarantine with its bytes and a reason
    /// file — never deletes; idempotent when already moved.
    pub(super) fn quarantine_object(&self, hash: &Hash, reason: &str) -> Result<(), MediumError> {
        self.quarantine_named(&hash.hex(), reason)
    }

    /// Incident-suffix ceiling for collision-safe quarantine placement.
    const QUARANTINE_NAME_ATTEMPTS: u32 = 10_000;

    fn quarantine_named(&self, name: &str, reason: &str) -> Result<(), MediumError> {
        let objects = self.root.join(OBJECTS_DIR);
        let quarantine = self.root.join(QUARANTINE_DIR);
        let source = objects.join(name);
        // Collision-safe across BOTH namespaces — evidence and
        // metadata: an incident is placed only where its evidence
        // path AND its .reason note path are both free, whatever
        // those paths currently hold (a preserved evidence file
        // named `x.reason` is exactly as protected as a note file),
        // and the note is created with create-new semantics so no
        // quarantine operation can ever overwrite an existing file.
        // The first incident keeps the plain name; later incidents
        // append .1, .2, … — preserved evidence and provenance are
        // never destroyed. All quarantine sites run under HEAD.lock,
        // so the exists-then-place sequence is serialized; an
        // interrupted move leaves the source in place (idempotent on
        // retry), and an interrupted note-write leaves the bytes
        // preserved under their incident name.
        let mut attempt = 0u32;
        let (target, note) = loop {
            let suffix = if attempt == 0 {
                String::new()
            } else {
                format!(".{attempt}")
            };
            let target = quarantine.join(format!("{name}{suffix}"));
            let note = quarantine.join(format!("{name}{suffix}.reason"));
            if !target.exists() && !note.exists() {
                break (target, note);
            }
            attempt += 1;
            if attempt > Self::QUARANTINE_NAME_ATTEMPTS {
                return Err(io_err(
                    "quarantine collision limit",
                    &target,
                    io::Error::other("too many quarantine incidents under one name"),
                ));
            }
        };
        match fs::rename(&source, &target) {
            Ok(()) => {
                // Non-overwriting provenance creation: even an
                // overlooked collision fails loudly rather than
                // destroying evidence.
                let mut file = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&note)
                    .map_err(|e| io_err("create quarantine reason", &note, e))?;
                file.write_all(reason.as_bytes())
                    .map_err(|e| io_err("write quarantine reason", &note, e))?;
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err("quarantine object", &source, e)),
        }
    }

    /// The session-open sweep: under the lock, every `objects/` entry
    /// not reachable from HEAD is quarantined (junk names included);
    /// reachable objects were hash-verified during the cold walk.
    pub(super) fn sweep_unreferenced(&mut self) -> Result<u64, MediumError> {
        let reachable = self
            .cursor
            .as_ref()
            .map(|cursor| cursor.reachable.clone())
            .unwrap_or_default();
        let mut quarantined = 0u64;
        let dir = self.root.join(OBJECTS_DIR);
        for entry in fs::read_dir(&dir).map_err(|e| io_err("read objects dir", &dir, e))? {
            let entry = entry.map_err(|e| io_err("read objects entry", &dir, e))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let referenced = Hash::from_hex(&name).is_some_and(|hash| reachable.contains(&hash));
            if !referenced {
                self.quarantine_named(&name, "sweep-unreferenced")?;
                quarantined += 1;
            }
        }
        Ok(quarantined)
    }
}
