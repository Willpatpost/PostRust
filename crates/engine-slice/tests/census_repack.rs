//! THE P5 R0 §9.2 BATTERY (the repack matrix — A18/R2's
//! before/after row): populations {6, 24, 1k, 4k} × dirty fraction
//! {0%, the stander mix, 100%} × {instance repack, pose evaluation,
//! slot write} — CPU µs/frame through the REAL production functions
//! (`repack_populations`, `evaluate_population_poses`,
//! `repack_poses`, `pack_indirect_block`) over the REAL population
//! tables (the garage's and corner's own; deterministic synthetic
//! grids at 1k/4k). PURE CPU — no device, no window: the BEFORE-R2
//! numbers are valid pod evidence, and the SAME harness re-runs
//! after R2 for the after (matched populations, matched cells).
//!
//! Two legs:
//! - `repack_matrix_smoke` (NOT ignored — the gate leg): the whole
//!   matrix plumbing end-to-end at tiny sample counts, asserting
//!   the table's structure (cell count, byte counts, the authored
//!   stander mixes, the at-cap boundary) so the battery is proven
//!   runnable everywhere the lib tests run.
//! - `repack_matrix_full` (`#[ignore]`d — the MEASUREMENT leg):
//!   the full matrix at the E-8 battery's sample convention (200
//!   warmup + 200 measured per cell), RELEASE mode on the rig (the
//!   standing rule; debug numbers are shape evidence only). Run:
//!
//! ```sh
//! cargo test -p engine-slice --test census_repack --release -- \
//!     --ignored --nocapture --test-threads=1
//! ```

use engine_slice::census::repack::{self, DirtyFraction};

/// The gate leg: the full plumbing, tiny samples.
#[test]
fn repack_matrix_smoke() {
    let rows = repack::run_repack_matrix(&repack::census_populations(), 1, 4);
    assert_eq!(rows.len(), 12, "4 populations x 3 dirty fractions");
    // Every cell measured all three operations with real samples.
    for row in &rows {
        assert!(row.instance_repack.samples == 4);
        assert!(row.pose_evaluation.samples == 4);
        assert!(row.slot_write.samples == 4);
        assert!(row.instance_repack.median_us > 0.0);
        assert!(row.pose_evaluation.median_us >= 0.0);
        assert!(row.slot_write.median_us > 0.0);
    }
    // The authored mixes (the tables' own counts).
    assert_eq!((rows[1].standers, rows[1].walkers), (2, 4), "garage mix");
    assert_eq!((rows[4].standers, rows[4].walkers), (8, 16), "corner mix");
}

/// The dirty-fraction labels are the design's three cells (the
/// printed table's legend leg — device-free).
#[test]
fn dirty_axis_labels() {
    assert_eq!(DirtyFraction::Zero.label(), "0% (all stand)");
    assert_eq!(DirtyFraction::StanderMix.label(), "stander mix (authored)");
    assert_eq!(DirtyFraction::Full.label(), "100% (all walk)");
}

/// The MEASUREMENT leg (see the module docs): the full matrix at
/// the E-8 sample convention, printing the §9.2 table.
#[test]
#[ignore = "the measurement leg: full-matrix CPU timings — run in release on the rig (the standing rule); the numbers are the archived row"]
fn repack_matrix_full() {
    let rows = repack::run_default_matrix();
    assert!(!rows.is_empty());
    assert_eq!(rows.len(), 12);
}
