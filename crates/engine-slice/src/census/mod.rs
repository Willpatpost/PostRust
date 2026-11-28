//! THE P5 R0 CENSUS HARNESS (design
//! `docs/engineering-excellence/designs/P5-resource-residency.md` §9
//! — the measurement plan's rows as RUNNABLE code; COUNTS BEFORE
//! MECHANISMS, the B06/L5/L6 discipline).
//!
//! What lives here, by §9 row:
//!
//! - **§9.1 (the VRAM census)** — [`carve`] computes the §0.1
//!   mapped-carve arithmetic and the §6.1 target-bytes prediction
//!   from the PUBLIC renderer constants (the arithmetic is the
//!   prediction the device counters check, per the design's §12);
//!   [`composition::CENSUS_FAMILIES`] declares the six feature
//!   families {dry, +shadows, +post, +wet, +wet+post, maximal
//!   composition} as corner-submission flag sets. The
//!   `tests/census_vram.rs` battery drives them per extent and
//!   prints the one table that gates the B14/§6 decision.
//! - **§9.2 (the repack matrix)** — [`repack`] measures the
//!   per-frame CPU cost of the three A18 operations (instance
//!   repack, pose evaluation, slot write) at populations {6, 24,
//!   1k, 4k} × dirty fraction {0%, the stander mix, 100%}. PURE
//!   CPU — the BEFORE-R2 numbers run anywhere; the same harness
//!   re-runs after R2 for the after. `tests/census_repack.rs`.
//! - **§9.3 (arena residency)** — the BEFORE side IS [`carve`]'s
//!   computed row (the carves are constant-sized today — the
//!   finding itself); the battery prints it per population cell.
//! - **§9.4 (the attach/upload census)** — the four NEW read-only
//!   counters on [`engine_renderer::AllocationStats`]
//!   (`upload_submits`, `upload_blocking_wait_ns`,
//!   `staged_buffer_uploads`, `texture_staging_tail_bytes`),
//!   recorded at the upload helpers' sites in engine-renderer.
//!   `tests/census_attach.rs` walks two fresh renderers (corner +
//!   garage) through construction and attach.
//! - **§9.5 (the ripple rows)** — [`wet`] constructs the three
//!   field states {empty, ~3.7-ring steady, 32-ring saturation};
//!   `tests/census_ripple.rs` renders the wet route per state ×
//!   {720p, 1440p} and prints the gpu_ms percentiles.
//! - **§9.6 (the combined-scene curves)** —
//!   [`composition::run_maximal_composition`] runs E-9's
//!   maximal-composition flag set (the owner-walked route) at a
//!   given extent through the demo's OWN crowd machinery, printing
//!   frame p50/p95/p99/max, GPU render ms, presented count, and
//!   validation. `tests/census_composition.rs` — re-run after
//!   EVERY lane (the no-regression bar).
//! - **§9.7 (gate boundaries)** — the at-cap typed refusals are
//!   pinned by [`repack`]'s boundary legs (4k = exactly the cap,
//!   cap+1 refuses) and the existing capacity-refusal tests; the
//!   F-03 region counters keep their own existing tests.
//!
//! DISCIPLINE (the R0 contract): everything here is ADDITIVE harness
//! code — no production behavior change, no counter semantics
//! change. The only engine-renderer edits are the four §9.4
//! counters (each documented at its field); the only corner_demo
//! edits are `pub(crate)` visibility so the census drives the
//! demo's own crowd, never a copy. The rig (release, real GPU)
//! runs the device rows; the pure-CPU rows (§9.2, §9.3, the §0.1
//! arithmetic) run anywhere and their numbers are recorded in
//! EVIDENCE.md per the E-8 discipline.

pub mod carve;
pub mod composition;
pub mod repack;
pub mod wet;

/// One measured operation's distribution — the `benchmark::report`
/// shape, in MICROSECONDS per frame (§9.2's unit; the E-8 baseline
/// battery's ms convention divided to the row's own scale).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimingStats {
    pub median_us: f64,
    pub p95_us: f64,
    pub p99_us: f64,
    pub max_us: f64,
    pub samples: usize,
}

/// Reduces sorted-by-construction sample durations to the row stats
/// (median / p95 / p99 / max µs). Nearest-rank percentiles, the
/// `benchmark::report` house convention.
pub fn timing_stats(samples: &mut [std::time::Duration]) -> TimingStats {
    assert!(
        !samples.is_empty(),
        "a timing cell needs at least one sample"
    );
    samples.sort_unstable();
    let n = samples.len();
    let us = |index: usize| samples[index].as_secs_f64() * 1e6;
    let rank = |quantile: f64| {
        (((n as f64) * quantile).ceil() as usize)
            .saturating_sub(1)
            .min(n - 1)
    };
    TimingStats {
        median_us: us(n / 2),
        p95_us: us(rank(0.95)),
        p99_us: us(rank(0.99)),
        max_us: us(n - 1),
        samples: n,
    }
}

/// Prints one timing cell as the matrix's column form (µs, three
/// significant figures, the sample count).
pub fn print_cell(label: &str, stats: &TimingStats) {
    println!(
        "  {label}: median {median:.1} / p99 {p99:.1} / max {max:.1} us (n={samples})",
        median = stats.median_us,
        p99 = stats.p99_us,
        max = stats.max_us,
        samples = stats.samples
    );
}
