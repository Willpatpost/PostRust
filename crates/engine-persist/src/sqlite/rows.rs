//! The object/record row IO (split from the sqlite monolith by
//! change reason, P3-L9 pure move): the transactional insert phases
//! (insert-once, verify-on-retry), the rollback-or-unknown
//! classification, and the hash-verified row loads. These change
//! when the table-level storage discipline changes.

use rusqlite::{Connection, params};

use crate::hash::{Hash, hash_object};
use crate::medium::{AppendOutcome, MediumError, StoredObject, UnknownReason};
use crate::record::{RecordKind, RevisionRecord};

use super::meta::{META_AUTHORITY, authority_bytes};
use super::walks::SqliteCursor;
use super::{
    SqliteFaultControl, SqliteFaultPoint, fault_check, is_constraint, no_rows_as_none, sqlite_err,
};

/// The transaction's insert phases: every object insert-once with
/// verify-on-retry, the fault windows, and the record row.
pub(super) fn append_inserts(
    tx: &rusqlite::Transaction<'_>,
    cursor: &Option<SqliteCursor>,
    record: &RevisionRecord,
    objects: &[StoredObject],
    record_bytes: &[u8],
    record_id: &Hash,
    fault: &Option<SqliteFaultControl>,
) -> Result<(), MediumError> {
    for (index, object) in objects.iter().enumerate() {
        if index == 0 {
            fault_check(fault, SqliteFaultPoint::MidObjectInserts)?;
        }
        insert_object_once(tx, cursor, object)?;
    }
    fault_check(fault, SqliteFaultPoint::ObjectsInserted)?;
    insert_record_once(tx, record, record_bytes, record_id)?;
    // The founding authority row — set once at founding, never
    // updated (the design's §3.1 metadata contract).
    if record.parent.is_none()
        && let RecordKind::Founding {
            authority,
            campaign_id,
        } = &record.kind
    {
        let encoded = authority_bytes(record_id, *authority, campaign_id);
        let present: Option<Vec<u8>> = tx
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![META_AUTHORITY],
                |row| row.get(0),
            )
            .map(Some)
            .or_else(no_rows_as_none)
            .map_err(|e| sqlite_err("read authority row", e))?;
        match present {
            None => {
                tx.execute(
                    "INSERT INTO meta (key, value) VALUES (?1, ?2)",
                    params![META_AUTHORITY, encoded],
                )
                .map_err(|e| sqlite_err("insert authority row", e))?;
            }
            Some(existing) => {
                if existing != encoded {
                    return Err(MediumError::CorruptHead {
                        detail: "the authority row contradicts the founding record".to_owned(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Insert-once with verify-on-retry: an existing row is accepted only
/// if its stored bytes hash to the expected identity; a mismatch is
/// fail-closed corruption (referenced when the cursor reaches the
/// identity, otherwise the stored-row mismatch) — the row is never
/// rewritten (it is the evidence; SQLite is never modified in place
/// outside transactions).
pub(super) fn insert_object_once(
    tx: &rusqlite::Transaction<'_>,
    cursor: &Option<SqliteCursor>,
    object: &StoredObject,
) -> Result<(), MediumError> {
    let hash = object.hash();
    let existing: Option<Vec<u8>> = tx
        .query_row(
            "SELECT bytes FROM objects WHERE hash = ?1",
            params![hash.as_bytes()],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(no_rows_as_none)
        .map_err(|e| sqlite_err("read existing object", e))?;
    match existing {
        Some(bytes) => {
            if hash_object(&bytes) != hash {
                let referenced = cursor
                    .as_ref()
                    .is_some_and(|cursor| cursor.reachable.contains(&hash));
                return Err(if referenced {
                    MediumError::ReferencedObjectCorrupt { hash }
                } else {
                    MediumError::HashMismatch {
                        expected: hash,
                        observed: hash_object(&bytes),
                        context: "sqlite-object-insert",
                    }
                });
            }
        }
        None => {
            tx.execute(
                "INSERT INTO objects (hash, domain, bytes) VALUES (?1, ?2, ?3)",
                params![hash.as_bytes(), object.domain(), object.bytes()],
            )
            .map_err(|e| sqlite_err("insert object", e))?;
        }
    }
    Ok(())
}

/// The record row: insert-once with verify-on-retry against the
/// records table; the chain-wide idempotency-key uniqueness is
/// enforced by the partial unique index — its violation is the
/// `duplicate-idempotency-key` chain rule (nothing installed; retry
/// only with corrected input).
pub(super) fn insert_record_once(
    tx: &rusqlite::Transaction<'_>,
    record: &RevisionRecord,
    record_bytes: &[u8],
    record_id: &Hash,
) -> Result<(), MediumError> {
    let existing: Option<Vec<u8>> = tx
        .query_row(
            "SELECT bytes FROM records WHERE hash = ?1",
            params![record_id.as_bytes()],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(no_rows_as_none)
        .map_err(|e| sqlite_err("read existing record", e))?;
    match existing {
        Some(bytes) => {
            // Content addressing makes a byte mismatch impossible for
            // a true retry; a mismatch is stored-row corruption.
            if bytes != record_bytes {
                return Err(MediumError::HashMismatch {
                    expected: *record_id,
                    observed: hash_object(&bytes),
                    context: "sqlite-record-insert",
                });
            }
        }
        None => {
            let kind = match record.kind {
                RecordKind::Founding { .. } => 0i64,
                RecordKind::Regular => 1i64,
            };
            let parent: Option<&[u8]> = record.parent.as_ref().map(|hash| &hash.as_bytes()[..]);
            let idem: Option<&[u8]> = record.idempotency_key.as_deref();
            tx.execute(
                "INSERT INTO records (hash, bytes, epoch, parent, idem, kind, tick) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    record_id.as_bytes(),
                    record_bytes,
                    record.epoch as i64,
                    parent,
                    idem,
                    kind,
                    record.tick as i64,
                ],
            )
            .map_err(|e| {
                if is_constraint(&e) {
                    MediumError::Chain {
                        rule: "duplicate-idempotency-key",
                    }
                } else {
                    sqlite_err("insert record", e)
                }
            })?;
        }
    }
    Ok(())
}

/// The rollback classification: a clean rollback propagates the typed
/// error (nothing installed); a failed rollback (including the
/// injected one) is `Unknown{RollbackUncertain}` — the disposition is
/// genuinely unknown.
pub(super) fn rollback_or_unknown(
    tx: rusqlite::Transaction<'_>,
    fault: &Option<SqliteFaultControl>,
    error: MediumError,
) -> Result<AppendOutcome, MediumError> {
    if let Err(injected) = fault_check(fault, SqliteFaultPoint::AtRollback) {
        let _ = injected;
        return Ok(AppendOutcome::Unknown {
            reason: UnknownReason::RollbackUncertain,
        });
    }
    match tx.rollback() {
        Ok(()) => Err(error),
        Err(_) => Ok(AppendOutcome::Unknown {
            reason: UnknownReason::RollbackUncertain,
        }),
    }
}

// ------------------------------------------------------------- the reads

/// Reads and verifies the `meta.head` row: the empty blob is the
/// Loads a record row's canonical bytes, hash-verified.
pub(super) fn load_record_verified(conn: &Connection, id: &Hash) -> Result<Vec<u8>, MediumError> {
    let bytes: Option<Vec<u8>> = conn
        .query_row(
            "SELECT bytes FROM records WHERE hash = ?1",
            params![id.as_bytes()],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(no_rows_as_none)
        .map_err(|e| sqlite_err("read record", e))?;
    let Some(bytes) = bytes else {
        return Err(MediumError::MissingObject { hash: *id });
    };
    if hash_object(&bytes) != *id {
        return Err(MediumError::HashMismatch {
            expected: *id,
            observed: hash_object(&bytes),
            context: "sqlite-record-store",
        });
    }
    Ok(bytes)
}

/// Loads an object row's bytes, hash-verified (the plain mismatch
/// form; the walk's referenced form escalates to
/// `ReferencedObjectCorrupt`).
pub(super) fn load_object_verified(conn: &Connection, hash: &Hash) -> Result<Vec<u8>, MediumError> {
    let bytes = load_object(conn, hash)?;
    if hash_object(&bytes) != *hash {
        return Err(MediumError::HashMismatch {
            expected: *hash,
            observed: hash_object(&bytes),
            context: "sqlite-object-store",
        });
    }
    Ok(bytes)
}

fn load_object(conn: &Connection, hash: &Hash) -> Result<Vec<u8>, MediumError> {
    let bytes: Option<Vec<u8>> = conn
        .query_row(
            "SELECT bytes FROM objects WHERE hash = ?1",
            params![hash.as_bytes()],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(no_rows_as_none)
        .map_err(|e| sqlite_err("read object", e))?;
    bytes.ok_or(MediumError::MissingObject { hash: *hash })
}

// ------------------------------------------------------------- the walks

/// The cold founding → HEAD walk (the complete validation): every
/// record row hash-verified and decoded, epoch-linked with checked
/// arithmetic, key-unique; every referenced object loaded and
/// hash-verified; every `depends_on` resolving to a strictly earlier
/// revision of the same authoritative history (retained, or
/// reclaim-indexed with its indexed epoch). Rebuilds the cursor.
///
/// **The compacted mode (PP-B3):** an absent parent ROW is a
/// legitimately-reclaimed revision only when the most recent on-chain
/// The walk's referenced record load — escalates a hash mismatch to
/// `ReferencedObjectCorrupt` (fail closed; the row is never replaced).
pub(super) fn load_referenced_record(conn: &Connection, id: &Hash) -> Result<Vec<u8>, MediumError> {
    load_record_verified(conn, id).map_err(|error| match error {
        MediumError::HashMismatch { .. } => MediumError::ReferencedObjectCorrupt { hash: *id },
        other => other,
    })
}

pub(super) fn load_referenced_object(
    conn: &Connection,
    hash: &Hash,
) -> Result<Vec<u8>, MediumError> {
    load_object_verified(conn, hash).map_err(|error| match error {
        MediumError::HashMismatch { .. } => MediumError::ReferencedObjectCorrupt { hash: *hash },
        other => other,
    })
}

/// Whether a record row exists (the compacted walk's gap detection —
/// a legitimately-reclaimed parent is an absent row, covered by the
/// covering MK's skip table; the binding is the raw identity bytes,
/// the table's established key form).
pub(super) fn record_exists(conn: &Connection, id: &Hash) -> Result<bool, MediumError> {
    match conn.query_row(
        "SELECT 1 FROM records WHERE hash = ?1",
        params![id.as_bytes()],
        |_| Ok(()),
    ) {
        Ok(()) => Ok(true),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(error) => Err(sqlite_err("record exists", error)),
    }
}
