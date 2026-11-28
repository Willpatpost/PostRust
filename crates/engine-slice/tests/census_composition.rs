//! THE P5 R0 §9.6 BATTERY (the combined-scene curves — the
//! charter's "measure composition overhead, not sum of
//! microbenchmarks"): the maximal-composition route (E-9's
//! owner-walked flag set: populated + behavior + shadows + rain +
//! wet + day-night 20 + post + sky — free-look/audio documented
//! off the scripted census route) at each census extent, printing
//! total frame p50/p95/p99/max, GPU render ms, presented count,
//! and validation. RE-RUN AFTER EVERY LANE: the no-regression bar
//! (a small-workload regression cannot hide behind a
//! large-workload win).
//!
//! `#[ignore]`d (a real device + windows). THE RIG RUN (per
//! extent, serial, archived per E-8):
//!
//! ```sh
//! cargo test -p engine-slice --test census_composition --release -- \
//!     --ignored --nocapture --test-threads=1
//! ```

use engine_slice::census::carve::CENSUS_EXTENTS;
use engine_slice::census::composition::run_maximal_composition;

fn sdl_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}

/// THE CURVES (the rig leg): the maximal composition at all four
/// census extents, 60 warmup + 300 measured route frames each
/// (inside the scripted route's 601 frames; the E-8 sample
/// convention's neighborhood — raise for the archived rig row as
/// the owner prefers).
#[test]
#[ignore = "requires a real Vulkan device and windows (the rig run: release, serial, archived per E-8; re-run after every P5 lane)"]
fn maximal_composition_curves() {
    let _guard = sdl_lock();
    for extent in CENSUS_EXTENTS {
        let row = run_maximal_composition(extent, 60, 300).expect("the route completes");
        assert_eq!(row.extent, extent);
        assert_eq!(row.presented, (60 + 300 + 2) as u64);
        assert_eq!(
            row.validation_errors, 0,
            "the composed route stays clean: {:?}",
            row
        );
        assert!(row.gpu_ms.is_some(), "the rig supports timestamps");
    }
}
