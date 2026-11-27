//! Basic diagnostics: a severity-threshold record store with an optional
//! stderr echo and BOUNDED retention.
//!
//! Wall-clock timestamps are deliberately excluded — diagnostics must never
//! make logical state non-deterministic. Records carry a level and a message
//! only. Retention is bounded (the A21 corrective): the store keeps at most
//! its capacity of the MOST RECENT records, evicting oldest-first in
//! amortized-O(1) halves when full, with every eviction counted in a
//! lifetime `dropped_total` (observable — a bounded store never silently
//! loses the knowledge that it lost records). Consumers needing full
//! history drain periodically and keep it externally. Richer metrics buses
//! arrive with the subsystems that need them (anti-abstraction rule,
//! `docs/TECH-ARCHITECTURE.md` §8).

use std::io::Write;
use std::num::NonZeroUsize;

/// Diagnostic severity. `Off` disables logging entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Off,
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Off => "off",
            Level::Trace => "trace",
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }

    pub fn parse(text: &str) -> Option<Level> {
        match text.trim_ascii().to_ascii_lowercase().as_str() {
            "off" => Some(Level::Off),
            "trace" => Some(Level::Trace),
            "debug" => Some(Level::Debug),
            "info" => Some(Level::Info),
            "warn" | "warning" => Some(Level::Warn),
            "error" => Some(Level::Error),
            _ => None,
        }
    }
}

/// One diagnostic record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub level: Level,
    pub message: String,
}

/// The default retention capacity: 4096 most-recent records (~a few
/// hundred KB worst case with typical messages). Large enough that no
/// ordinary session ever evicts; small enough that a runaway Trace
/// session cannot grow the engine's resident memory unboundedly.
pub const DEFAULT_RETENTION: usize = 4096;

/// Severity-threshold record store with bounded retention and an optional
/// stderr echo.
///
/// The store keeps at most `capacity` records (the MOST RECENT ones);
/// evictions are oldest-first in amortized-O(1) halves and every evicted
/// record is counted in the lifetime `dropped_total` (see [`Self::dropped_total`]).
/// `records()` and `count_at_or_above()` answer over the RETAINED set —
/// totals including evictions need the dropped counter. The echo is
/// opt-in (`with_stderr_echo`); the default constructor performs no I/O.
#[derive(Debug, Clone)]
pub struct Diagnostics {
    min_level: Level,
    records: Vec<Record>,
    capacity: usize,
    dropped_total: u64,
    echo_stderr: bool,
}

impl Diagnostics {
    pub fn new(min_level: Level) -> Self {
        Self::with_retention_inner(min_level, DEFAULT_RETENTION, false)
    }

    pub fn with_stderr_echo(min_level: Level) -> Self {
        Self::with_retention_inner(min_level, DEFAULT_RETENTION, true)
    }

    /// A store with an explicit retention capacity (the bounded buffer's
    /// size — `NonZeroUsize` so a "store" that retains nothing cannot be
    /// constructed; use `Level::Off` to disable recording entirely).
    pub fn with_retention(min_level: Level, capacity: NonZeroUsize) -> Self {
        Self::with_retention_inner(min_level, capacity.get(), false)
    }

    fn with_retention_inner(min_level: Level, capacity: usize, echo_stderr: bool) -> Self {
        Diagnostics {
            min_level,
            records: Vec::new(),
            capacity,
            dropped_total: 0,
            echo_stderr,
        }
    }

    pub fn set_min_level(&mut self, level: Level) {
        self.min_level = level;
    }

    pub fn min_level(&self) -> Level {
        self.min_level
    }

    /// Whether a record at `level` would pass the threshold — the guard
    /// for eager `format!` construction at hot-path call sites: wrapping
    /// the message construction in `if diag.would_log(level)` keeps a
    /// suppressed record from allocating at all, instead of building a
    /// `String` the threshold immediately discards (the spine schedules
    /// and executes events every tick; with `Level::Off` those message
    /// constructions must cost nothing).
    pub fn would_log(&self, level: Level) -> bool {
        self.min_level != Level::Off && level >= self.min_level
    }

    /// Records a message at `level` unless it falls below the threshold.
    /// More severe records always pass; `Off` suppresses everything.
    /// When the store is at capacity the OLDEST half is evicted
    /// (amortized O(1) per record) and counted in `dropped_total`.
    pub fn log(&mut self, level: Level, message: impl Into<String>) {
        if self.min_level == Level::Off || level < self.min_level {
            return;
        }
        if self.records.len() == self.capacity {
            // Evict the oldest half (at least one — a capacity of 1
            // must still make room). The drain is amortized O(1) per
            // record: one drain per capacity/2 pushes.
            let evict = (self.capacity / 2).max(1);
            self.records.drain(..evict);
            self.dropped_total += evict as u64;
        }
        let record = Record {
            level,
            message: message.into(),
        };
        if self.echo_stderr {
            let _ = writeln!(
                std::io::stderr(),
                "[{}] {}",
                record.level.as_str(),
                record.message
            );
        }
        self.records.push(record);
    }

    /// The lifetime count of records evicted by the retention bound —
    /// a bounded store never silently loses the knowledge that it lost
    /// records. Survives [`Self::clear`] (which clears only the
    /// retained set).
    pub fn dropped_total(&self) -> u64 {
        self.dropped_total
    }

    /// The retention capacity (the maximum retained-record count).
    pub fn retention(&self) -> usize {
        self.capacity
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// Number of records at or above `level`.
    pub fn count_at_or_above(&self, level: Level) -> usize {
        self.records.iter().filter(|r| r.level >= level).count()
    }

    pub fn clear(&mut self) {
        self.records.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_drops_less_severe_records() {
        let mut diag = Diagnostics::new(Level::Warn);
        diag.log(Level::Trace, "quiet");
        diag.log(Level::Info, "also quiet");
        diag.log(Level::Warn, "kept");
        diag.log(Level::Error, "kept too");
        assert_eq!(diag.records().len(), 2);
        assert_eq!(diag.count_at_or_above(Level::Error), 1);
    }

    #[test]
    fn off_suppresses_everything() {
        let mut diag = Diagnostics::new(Level::Off);
        diag.log(Level::Error, "suppressed");
        assert!(diag.records().is_empty());
    }

    #[test]
    fn would_log_matches_the_threshold_gates() {
        let diag = Diagnostics::new(Level::Warn);
        assert!(!diag.would_log(Level::Trace));
        assert!(!diag.would_log(Level::Info));
        assert!(diag.would_log(Level::Warn));
        assert!(diag.would_log(Level::Error));
        let off = Diagnostics::new(Level::Off);
        assert!(!off.would_log(Level::Error));
    }

    #[test]
    fn level_parse_round_trips() {
        for level in [
            Level::Off,
            Level::Trace,
            Level::Debug,
            Level::Info,
            Level::Warn,
            Level::Error,
        ] {
            assert_eq!(Level::parse(level.as_str()), Some(level));
        }
        assert_eq!(Level::parse("nonsense"), None);
    }

    /// A21: the bounded retention evicts oldest-first in halves, keeps
    /// the MOST RECENT records, and counts every eviction.
    #[test]
    fn bounded_retention_evicts_oldest_and_counts_dropped() {
        let mut diag = Diagnostics::with_retention(Level::Info, NonZeroUsize::new(4).unwrap());
        for i in 0..10 {
            diag.log(Level::Info, format!("message {i}"));
        }
        // The retained set is the most recent 4, in order.
        let retained: Vec<&str> = diag.records().iter().map(|r| r.message.as_str()).collect();
        assert_eq!(
            retained,
            vec!["message 6", "message 7", "message 8", "message 9"]
        );
        // 10 logged, 4 retained → 6 evicted (in three half-drains of 2).
        assert_eq!(diag.dropped_total(), 6);
        assert_eq!(diag.retention(), 4);
        // The counters survive clear (the retained set clears; the
        // lifetime evidence does not).
        diag.clear();
        assert!(diag.records().is_empty());
        assert_eq!(diag.dropped_total(), 6);
    }

    /// A21: a capacity of 1 still makes room (the max(1) eviction guard).
    #[test]
    fn capacity_one_retains_the_latest() {
        let mut diag = Diagnostics::with_retention(Level::Info, NonZeroUsize::new(1).unwrap());
        diag.log(Level::Info, "first");
        diag.log(Level::Info, "second");
        assert_eq!(diag.records().len(), 1);
        assert_eq!(diag.records()[0].message, "second");
        assert_eq!(diag.dropped_total(), 1);
    }

    /// A21: Off still records (and allocates) nothing — the
    /// spine-allocation pins depend on this shape.
    #[test]
    fn off_stays_empty_with_the_bounded_fields() {
        let mut diag = Diagnostics::new(Level::Off);
        for i in 0..100 {
            diag.log(Level::Error, format!("suppressed {i}"));
        }
        assert!(diag.records().is_empty());
        assert_eq!(diag.dropped_total(), 0);
        assert_eq!(diag.retention(), DEFAULT_RETENTION);
    }
}
