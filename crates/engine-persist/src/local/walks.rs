//! The local provider's walk adapters over the shared walk core
//! (P3-L9b, the A13 Option 2 walk half): the reader seam over the
//! store's root-path loads, the observed-HEAD validation dispatch
//! with LOCAL's diagnostics discipline, the cold-walk entry points,
//! and the O(1) cursor advances. The walk MACHINERY — the cold walk
//! with the PP-B3 gap-aware skip descent and the K2-C2B
//! virtual-exclusion variant, the bounded suffix walk with the
//! verified composite closure, the suffix absorption, and the frozen
//! refusal rules — lives in [`crate::chain_walk`], shared with the
//! sqlite provider; this file owns what the design keeps
//! provider-side (the L9 record's four differences, each with its
//! disposition documented in the core's module doc): the loaders
//! (the lock-held hash-verified file loads — records and content
//! objects are the same content-addressed files here, and the
//! parent-present probe is the object path's `is_file`), the
//! methods-and-stats shape (the free-fn core wrapped in
//! `LocalFileMedium` methods, the cold walks counted into
//! `cold_walk_records`), and nothing else — the suffix closure
//! timing and the refusal rules are the core's.

use std::collections::BTreeSet;
use std::path::Path;

use crate::chain_walk::{
    ChainReads, ValidationOutcome, WalkCursor, advance_cursor as core_advance_cursor,
    validate_observed as core_validate_observed, walk_chain_excluding as core_walk_chain_excluding,
};
use crate::hash::Hash;
use crate::medium::{ChainSummary, MediumError};
use crate::record::RevisionRecord;

use super::layout::{load_referenced_at, object_path_at};
use super::{Cursor, LocalFileMedium};

/// (P3-L9b) The local adapter over the shared walk core's reader
/// seam: the store's hash-verified object loads and the object-path
/// presence probe, expressed over the store root (a `&Path` disjoint
/// from the provider's `&mut` cursor borrow, so the core's
/// `validate_observed` can run inside a `&mut self` method). Records
/// and content objects are the SAME files in this store (the
/// layout's single content-addressed namespace), so both reads are
/// the referenced-corruption-escalating load; the parent-present
/// probe is the object path's `is_file` (infallible — the sqlite
/// seam's `record_exists` probe is the fallible form).
pub(super) struct LocalChainReads<'a> {
    root: &'a Path,
}

impl ChainReads for LocalChainReads<'_> {
    fn read_record_bytes(&mut self, id: &Hash) -> Result<Vec<u8>, MediumError> {
        load_referenced_at(self.root, id)
    }

    fn read_object_bytes(&mut self, hash: &Hash) -> Result<Vec<u8>, MediumError> {
        load_referenced_at(self.root, hash)
    }

    fn record_present(&mut self, id: &Hash) -> Result<bool, MediumError> {
        Ok(object_path_at(self.root, id).is_file())
    }
}

impl LocalFileMedium {
    // -------------------------------------------------------- validation

    /// Validates the observed HEAD against the trusted cursor: O(0)
    /// record walks on the verified head, a bounded suffix walk on
    /// external divergence, a cold walk with no cursor, and fail-closed
    /// divergence when HEAD vanished or is not a descendant. The
    /// dispatch is the shared core's; LOCAL's diagnostics discipline
    /// maps the outcome (the cold walks count into
    /// `cold_walk_records`, never into `last_append_walked`).
    pub(super) fn validate_observed(&mut self, observed: Option<Hash>) -> Result<(), MediumError> {
        let mut reads = LocalChainReads {
            root: self.root.as_path(),
        };
        let outcome = core_validate_observed(&mut reads, &mut self.cursor, observed)?;
        match outcome {
            ValidationOutcome::NoObservedHead => {}
            ValidationOutcome::VerifiedHead => self.walks.last_append_walked = 0,
            ValidationOutcome::SuffixAbsorbed { walked } => {
                self.walks.last_append_walked = walked;
            }
            ValidationOutcome::RebuiltFromInvalidation { records }
            | ValidationOutcome::FreshColdEstablished { records } => {
                self.walks.cold_walk_records = records;
            }
        }
        Ok(())
    }

    /// The cold founding → HEAD walk (the shared core's, over this
    /// store's reader seam): rebuilds the trusted cursor and returns
    /// the summary.
    pub(super) fn cold_walk_from(&mut self, head: Hash) -> Result<ChainSummary, MediumError> {
        let (summary, cursor) = self.cold_walk_excluding(head, &BTreeSet::new())?;
        self.walks.cold_walk_records = summary.records;
        self.cursor = Some(cursor);
        Ok(summary)
    }

    /// The authenticated virtual-exclusion survivor walk (K2-C2B
    /// P2): the shared core's cold walk with the AUDITED
    /// retiring-record set treated as virtually absent. **P2-F1: the
    /// simulated post-retirement view is returned as DATA — the
    /// authoritative trusted cursor, the walk counters, and every
    /// other provider state are untouched (the walk is a pure `&self`
    /// computation), so an unsuccessful walk, an authorization
    /// refusal, or a resource limit leaves the provider's ordinary
    /// state exactly as it was, and no append, validation, or sweep
    /// ever observes the simulated view.** The caller decides
    /// whether and when to establish a cursor consistent with the
    /// post-deletion physical state.
    pub(super) fn cold_walk_excluding(
        &self,
        head: Hash,
        exclude: &BTreeSet<Hash>,
    ) -> Result<(ChainSummary, Cursor), MediumError> {
        let mut reads = LocalChainReads {
            root: self.root.as_path(),
        };
        let walked = core_walk_chain_excluding(&mut reads, head, exclude)?;
        Ok((walked.summary, walked.cursor))
    }

    /// Advances the cursor over an accepted append (O(1) work).
    pub(super) fn advance_cursor(&mut self, record: &RevisionRecord, id: &Hash) {
        self.advance_cursor_with_closure(record, id, None);
    }

    /// The cursor advance with the optional VERIFIED composite
    /// closure (the K2-C1 corrective): a composite append's chunks
    /// join the reachable set through the admission-verified
    /// manifest, keeping the hot cursor and the cold walk on the
    /// SAME definition of reachability.
    pub(super) fn advance_cursor_with_closure(
        &mut self,
        record: &RevisionRecord,
        id: &Hash,
        composite_closure: Option<&[Hash]>,
    ) {
        let cursor: &mut Option<WalkCursor> = &mut self.cursor;
        core_advance_cursor(cursor, record, id, composite_closure);
    }
}
