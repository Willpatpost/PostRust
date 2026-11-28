//! Cycle 3 P2 — the audio DEVICE-path smoke: the real SDL audio
//! subsystem (the dummy driver — the same discipline the shell's
//! other SDL tests use), the default-device open, the per-cue stream
//! binding, the push-model dispatch, and the refcounted lifecycle
//! churn, in ONE test (SDL's process-wide initialization is not safe
//! to race across parallel test threads — the one-SDL-init-per-binary
//! rule the shell's suites already follow).
//!
//! The dummy driver is present in every vendored SDL build (the pod
//! and CI-Linux compile it as the no-hardware fallback; the owner's
//! macOS/Windows machines carry real drivers), so this exercises the
//! FULL device path deterministically everywhere — while the
//! no-device degradation path (the observable silent status) runs on
//! platforms where the open genuinely fails, and is additionally
//! pinned by the pure `Audio::disabled` tests in the library battery.

use engine_shell::audio::{Audio, AudioStatus, CUES, Cue};
use engine_shell::{DriverPreference, Platform};

#[test]
fn the_device_path_dispatch_and_lifecycle_are_clean() {
    let platform = Platform::init(DriverPreference::Dummy).expect("the platform");

    // 1. The device path: initialize → Active on the forced dummy
    //    driver (NEVER an error — a failure here means the vendored
    //    build dropped the dummy driver, which every platform of
    //    record carries).
    let mut audio = Audio::initialize(&platform);
    match audio.status() {
        AudioStatus::Active { driver, device } => {
            assert_eq!(driver, "dummy", "the forced dummy driver is active");
            assert!(!device.is_empty(), "the device name is carried");
        }
        AudioStatus::Silent { reason } => {
            panic!("the dummy audio driver was expected: {reason}");
        }
    }

    // 2. The push-model dispatch: every cue through the real stream
    //    path, repeatedly, with bounded wall cost (each play is one
    //    non-blocking buffered SDL call plus SDL's internal copy —
    //    or, under CA-22's queue budget, one queued-bytes read and a
    //    counted refusal: the dummy driver drains at its own pace,
    //    so an overload burst may refuse fail-visibly, which keeps
    //    the path bounded rather than breaking it).
    let start = std::time::Instant::now();
    let pushes = 100 * CUES.len() as u64;
    for _ in 0..100 {
        for cue in CUES {
            audio.play(cue);
        }
    }
    let dispatch_window = start.elapsed();
    assert!(
        dispatch_window.as_millis() < 2_000,
        "{pushes} cue pushes are bounded: {dispatch_window:?}"
    );
    let per_push_us = dispatch_window.as_micros() / u128::from(pushes);
    println!(
        "[audio-smoke] {pushes} cue pushes in {dispatch_window:?} (~{per_push_us} µs per push)"
    );
    assert!(
        audio.status().is_active(),
        "the dummy device stays active after playback"
    );

    // CA-22's refusal telemetry is readable on the real path: the
    // budget is sized from the cue table (always > 0 on an active
    // surface), and the refusal count is carried fail-visibly. The
    // dummy driver's drain pace is platform/timing-dependent, so the
    // count is PRINTED here, never asserted — the overload response
    // itself (refusal visible, never silent unbounded growth) is
    // pinned by the library battery's CueBackpressure tests.
    let budget = audio.cue_queue_budget_bytes();
    let refused = audio.cues_refused();
    assert!(budget > 0, "an active surface carries a real budget");
    println!("[audio-smoke] cue queue budget {budget} bytes/stream; {refused} cues refused");

    // 3. The refcounted lifecycle: five full initialize → play →
    //    drop cycles inside one platform — the SDL AUDIO subsystem
    //    quits when the last handle drops and reinitializes on the
    //    next construction, without leaks or panics. (The
    //    process-wide SDL init/quit is the shell's lifecycle, once
    //    per process — not cycled here.) The first re-initialization
    //    also measures the steady-state construction cost (the very
    //    first initialize above pays the one-time subsystem start).
    let start = std::time::Instant::now();
    let mut audio = Audio::initialize(&platform);
    let reinit = start.elapsed();
    assert!(audio.status().is_active());
    audio.play(Cue::Interact);
    audio.play(Cue::EncounterComplete);
    drop(audio);
    for cycle in 0..4 {
        let mut audio = Audio::initialize(&platform);
        assert!(
            audio.status().is_active(),
            "cycle {cycle} activates: {:?}",
            audio.status()
        );
        audio.play(Cue::Interact);
        audio.play(Cue::EncounterComplete);
        drop(audio);
    }
    println!(
        "[audio-smoke] 5 lifecycle cycles clean (re-initialize ~{} µs); dispatch + device + lifecycle EXECUTED",
        reinit.as_micros()
    );
}
