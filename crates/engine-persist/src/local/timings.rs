//! The diagnostics counters (split from the local monolith by change
//! reason, P3-L9 pure move): the phase-separated append timings and
//! the deterministic validation-work counters, their accessors, and
//! the clock helper - observational only, never asserted on wall-clock
//! values.

use std::time::Instant;

use super::LocalFileMedium;

/// Phase-separated development timings (observational, nanoseconds;
/// not performance claims — B5 owns the hardware analysis).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AppendTimings {
    /// Record canonicalization + structural validation + hashing.
    pub encode_nanos: u64,
    /// Object writes, file syncs, and the objects-directory sync.
    pub object_write_sync_nanos: u64,
    /// HEAD.tmp write + fsync + rename + root-directory sync.
    pub head_publish_nanos: u64,
    /// The observed-HEAD validation phase (cursor work).
    pub validation_nanos: u64,
    /// The last lock-held reconciliation.
    pub reconcile_nanos: u64,
}

/// Deterministic validation-work counters (asserted by the O(1)-cursor
/// tests; timing assertions are never used for this property).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WalkStats {
    /// Records verified in the last cold walk.
    pub cold_walk_records: u64,
    /// Records walked during the last append's validation phase
    /// (0 for an append to the already-verified head).
    pub last_append_walked: u64,
    /// Records walked during the last reconciliation.
    pub last_reconcile_walked: u64,
}

impl LocalFileMedium {
    /// The last append/reconcile's phase timings (observational).
    pub fn timings(&self) -> AppendTimings {
        self.timings
    }

    /// The deterministic validation-work counters.
    pub fn walk_stats(&self) -> WalkStats {
        self.walks
    }
}

pub(super) fn elapsed_nanos(from: Instant) -> u64 {
    u64::try_from(from.elapsed().as_nanos()).unwrap_or(u64::MAX)
}
