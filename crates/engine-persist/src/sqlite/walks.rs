//! The sqlite provider's walk adapters over the shared walk core
//! (P3-L9b, the A13 Option 2 walk half): the reader seam over the
//! transaction-scoped SQL rows, the observed-HEAD validation dispatch
//! with SQLITE's diagnostics discipline, the cold-walk entry points,
//! and the O(1) cursor advance. The walk MACHINERY — the cold walk
//! with the PP-B3 gap-aware skip descent and the K2-C2B
//! virtual-exclusion variant, the bounded suffix walk with the
//! verified composite closure (this provider's R15 form — the
//! unification's chosen closure timing), the suffix absorption, and
//! the frozen refusal rules — lives in [`crate::chain_walk`], shared
//! with the local provider; this file owns what the design keeps
//! provider-side (the L9 record's four differences, each with its
//! disposition documented in the core's module doc): the loaders
//! (the `records`-table record loads, the `objects`-table content
//! loads, and the `record_exists` presence probe — all inside the
//! caller's transaction), and the free-fns-and-stats shape (the
//! wrappers over `(&Connection, &mut Option<SqliteCursor>, &mut
//! WalkStats)`, the post-invalidation cold walks counted into
//! `last_append_walked`).
//!
//! The [`SqliteCursor`] name is the shared core's
//! [`crate::chain_walk::WalkCursor`] — the §5.3.2 ten-field field set
//! the two providers carried identically (the same names, types, and
//! order), aliased so every `sqlite`-tree reference keeps its path.

use std::collections::BTreeSet;

use rusqlite::Connection;

use crate::chain_walk::{
    ChainReads, ValidationOutcome, WalkCursor, WalkedChain, advance_cursor as core_advance_cursor,
    validate_observed as core_validate_observed, walk_chain_excluding as core_walk_chain_excluding,
};
use crate::hash::Hash;
use crate::local::WalkStats;
use crate::medium::MediumError;
use crate::record::RevisionRecord;

use super::rows::{load_referenced_object, load_referenced_record, record_exists};

/// The session-scoped trusted cursor (the §5.3.2 model — identical
/// semantics to the file medium's): the shared walk core's
/// [`WalkCursor`].
pub(super) type SqliteCursor = WalkCursor;

/// (P3-L9b) The sqlite adapter over the shared walk core's reader
/// seam: the transaction-scoped, hash-verified row loads (the
/// `records` table for record bytes, the `objects` table for content
/// bytes — both escalating a hash mismatch to referenced corruption)
/// and the `record_exists` presence probe the compacted walk's gap
/// detection consumes. ONE `&Connection` backs a whole walk (the
/// caller's transaction — the single coordination-scoped borrow).
pub(super) struct SqliteChainReads<'a> {
    conn: &'a Connection,
}

impl ChainReads for SqliteChainReads<'_> {
    fn read_record_bytes(&mut self, id: &Hash) -> Result<Vec<u8>, MediumError> {
        load_referenced_record(self.conn, id)
    }

    fn read_object_bytes(&mut self, hash: &Hash) -> Result<Vec<u8>, MediumError> {
        load_referenced_object(self.conn, hash)
    }

    fn record_present(&mut self, id: &Hash) -> Result<bool, MediumError> {
        record_exists(self.conn, id)
    }
}

// --------------------------------------------------------------- the medium

/// The cold founding → HEAD walk (the shared core's, over the
/// transaction's reader seam): every record row hash-verified and
/// decoded, epoch-linked with checked arithmetic, key-unique; every
/// referenced object loaded and hash-verified; every `depends_on`
/// resolving to a strictly earlier revision of the same authoritative
/// history (retained, or reclaim-indexed with its indexed epoch).
/// Rebuilds the cursor.
///
/// **The compacted mode (PP-B3):** an absent parent ROW is a
/// legitimately-reclaimed revision only when the most recent on-chain
/// MK's skip table covers the gap. An uncovered absence is hard
/// corruption — fail closed, never assumed reclamation.
pub(super) fn walk_chain(conn: &Connection, head: Hash) -> Result<WalkedChain, MediumError> {
    let mut reads = SqliteChainReads { conn };
    crate::chain_walk::walk_chain(&mut reads, head)
}

/// The authenticated virtual-exclusion survivor walk (K2-C2B P2):
/// the shared core's cold walk with the AUDITED retiring-record set
/// treated as virtually absent — a physically present record in the
/// exclusion routes through the SAME authenticated skip jump an
/// absent one takes, and the MK-index staging automatically carries
/// the excluded records into the reclaimed-membership universe
/// (preserving depends_on resolution and key history exactly as
/// physical reclamation does). Every walked record and object remains
/// hash-verified; an unrelated missing parent still fails closed.
pub(super) fn walk_chain_excluding(
    conn: &Connection,
    head: Hash,
    exclude: &BTreeSet<Hash>,
) -> Result<WalkedChain, MediumError> {
    let mut reads = SqliteChainReads { conn };
    core_walk_chain_excluding(&mut reads, head, exclude)
}

/// Validates the observed head against the trusted cursor: O(0)
/// record walks on the verified head, a bounded suffix walk on
/// external divergence, a cold walk with no cursor, and fail-closed
/// divergence when the head vanished or is not a descendant. The
/// dispatch is the shared core's; SQLITE's diagnostics discipline
/// maps the outcome (the suffix and the post-invalidation cold walks
/// count into `last_append_walked`).
pub(super) fn validate_observed(
    conn: &Connection,
    cursor: &mut Option<SqliteCursor>,
    walks: &mut WalkStats,
    observed: Option<Hash>,
) -> Result<(), MediumError> {
    let mut reads = SqliteChainReads { conn };
    let outcome = core_validate_observed(&mut reads, cursor, observed)?;
    match outcome {
        ValidationOutcome::NoObservedHead => {}
        ValidationOutcome::VerifiedHead => walks.last_append_walked = 0,
        ValidationOutcome::SuffixAbsorbed { walked } => walks.last_append_walked = walked,
        ValidationOutcome::RebuiltFromInvalidation { records }
        | ValidationOutcome::FreshColdEstablished { records } => {
            walks.last_append_walked = records;
        }
    }
    Ok(())
}

/// Advances the cursor over an accepted append — O(1) work (plus
/// the verified composite closure when the record is a /2 root —
/// the R12 corrective keeps the hot cursor on the same definition
/// of reachability as the cold walk).
pub(super) fn advance_cursor(
    cursor: &mut Option<SqliteCursor>,
    record: &RevisionRecord,
    id: &Hash,
    composite_closure: Option<&[Hash]>,
) {
    core_advance_cursor(cursor, record, id, composite_closure);
}
