//! Fixed-timestep accumulator for the corner demo's interactive loops.
//! Drives the crowd locomotion, behavior spine, and weather at exactly
//! 1/60 per tick.
//!
//! # Design
//!
//! Two modes of operation:
//!
//! 1. **Scripted route** (601-frame measured): Frame-index-driven. One
//!    tick per presented frame at exactly 60 Hz. Uses `DEMO_CLOCK_DT_F32`
//!    directly - bit-identical to the old `1.0 / 60.0` f32 literals.
//!
//! 2. **Free-look mode**: Accumulator-driven. Uses `FixedTimestepAccumulator`
//!    to collect presented-frame sim time and drive coherent clocks for
//!    crowd, rain, and day/night progression at exactly 1/60 per tick.
//!    The tick extraction is O(1) in the stall length (R03's corrective:
//!    exact bounded arithmetic — one integer division and one remainder
//!    over the accumulator's total nanoseconds, never `elapsed / dt`
//!    iterations), and the per-frame consumption is COHERENT (R04's
//!    corrective: the crowd and the weather consume the SAME extracted
//!    ticks on every frame that attempts presentation — see
//!    [`FixedTimestepAccumulator::advance_coherent`]).
//!
//! # dt value
//!
//! `DEMO_CLOCK_DT_F32` is computed as `1.0 / DEMO_CLOCK_HZ as f32` using
//! single-precision division, ensuring bit-identical values with the
//! original `1.0 / 60.0` literals.
//!
//! `demo_clock_dt()` returns a `Duration` for use as a tick threshold in
//! the accumulator only - the Duration quantizes to nanoseconds and is
//! NOT used for arithmetic that must match the original f32 literals.

use std::time::Duration;

/// The fixed simulation tick rate: one tick per 1/60 s of accumulated
/// sim time.
pub const DEMO_CLOCK_HZ: f64 = 60.0;

/// The fixed simulation tick delta as f32 (for API compatibility).
/// This is computed as `1.0 / DEMO_CLOCK_HZ as f32` - single-precision
/// division, exactly matching the old `1.0 / 60.0` f32 literals.
pub const DEMO_CLOCK_DT_F32: f32 = 1.0 / DEMO_CLOCK_HZ as f32;

/// The fixed simulation tick delta as f64 (for accumulator precision).
pub const DEMO_CLOCK_DT_F64: f64 = 1.0 / DEMO_CLOCK_HZ;

/// The fixed simulation tick delta as a Duration (computed at runtime
/// since Duration::from_secs_f64 isn't const).
pub fn demo_clock_dt() -> Duration {
    Duration::from_secs_f64(1.0 / DEMO_CLOCK_HZ)
}

/// The catch-up cap: the maximum number of simulation ticks we will
/// advance in a single frame to catch up after a stall. Beyond this
/// cap, excess time is DROPPED VISIBLY — a counter increments and
/// a log line appears at the session boundary (the brief's explicit
/// requirement: never silently discard).
///
/// JUSTIFICATION: the existing corner demo runs at 60 fps with one
/// tick per frame. If the render rate drops to 30 fps, the accumulator
/// has ~2 ticks worth of time, and we advance 2 ticks. At 20 fps,
/// ~3 ticks, and we advance 3 ticks. The cap prevents runaway catch-up
/// on severe stalls (GPU lockups, massive GC pauses, window move/resize
/// stalls) that would advance the world hours into the future in one
/// frame.
///
/// The cap is chosen as 5 ticks: even at 12 fps (a severe bottleneck
/// for this demo), we advance 5 ticks per frame, catching up in
/// reasonable time while preventing absurd forward leaps. The visible
/// drop counter logs when time exceeds the cap, so operators see the
/// degradation and know the world state was not fully advanced.
pub const MAX_TICKS_PER_FRAME: u32 = 5;

/// The fixed-timestep accumulator for the corner demo's interactive
/// loops. Accumulates presented-frame `sim_advance` time from
/// `FrameTiming` and drives the crowd/rain at exactly 1/60 per tick,
/// with bounded catch-up and visible drop accounting.
pub struct FixedTimestepAccumulator {
    /// Accumulated simulation time since the last tick.
    accumulator: Duration,
    /// Total dropped time (excess beyond the cap) across the session.
    dropped_time: Duration,
    /// Number of frames where time was dropped beyond the cap.
    dropped_frames: u64,
    /// Total ticks advanced (for the exit summary).
    ticks_advanced: u64,
}

impl FixedTimestepAccumulator {
    /// Constructs a new accumulator starting from zero.
    pub fn new() -> Self {
        Self {
            accumulator: Duration::ZERO,
            dropped_time: Duration::ZERO,
            dropped_frames: 0,
            ticks_advanced: 0,
        }
    }

    /// Feeds a new frame's sim advance and returns the number of ticks
    /// to advance (0 if not enough time accumulated).
    ///
    /// # Cap semantics
    ///
    /// If more than MAX_TICKS_PER_FRAME complete ticks fit in the
    /// accumulator, we advance exactly MAX_TICKS_PER_FRAME ticks.
    /// The excess complete ticks are DROPPED (removed from the
    /// accumulator, not returned) and counted in `dropped_time`.
    /// Only the sub-tick remainder (fractional time less than one dt)
    /// stays in the accumulator for the next frame.
    ///
    /// Example: if 7 complete ticks fit but cap is 5:
    /// - Advance 5 ticks (ticks_advanced += 5)
    /// - Remove 2 complete ticks from accumulator (dropped)
    /// - Keep sub-tick remainder (if any)
    /// - dropped_time += 2 * dt
    ///
    /// # Bounded arithmetic (R03's corrective)
    ///
    /// The tick count and the sub-tick remainder are computed by ONE
    /// integer division and ONE remainder over the accumulator's total
    /// nanoseconds — O(1) in the stall length (a Duration stall of
    /// hours costs the same as a sub-tick frame; the pre-fix form
    /// iterated `elapsed / dt` times BEFORE the cap applied — O(n)
    /// work whose result the cap then clamped to 5). THE EXACTNESS
    /// ARGUMENT: `Duration` is integer nanoseconds internally and its
    /// `+=`/`-=`/`>=` are exact integer arithmetic on the total, so
    /// the iterating loop's k-th pass held exactly
    /// `total − k·dt_nanos` nanoseconds and it stopped at exactly
    /// k = ⌊total / dt_nanos⌋ leaving `total mod dt_nanos` — the
    /// division form computes those two values directly, with NO
    /// data-dependent iteration. The tick counter SATURATES at
    /// `u32::MAX` (accumulated time beyond ~828 days of ticks): the
    /// iterating form had no defined semantics there (debug builds
    /// panicked on the counter's overflow; release builds silently
    /// wrapped and corrupted the drop accounting); the saturated
    /// count still lands on the cap, so the returned tick count is
    /// identical for every input the old form could process at all.
    /// The equivalence is pinned against the verbatim iterating form
    /// by the inline test
    /// `huge_stall_advances_in_o1_matching_the_iterating_form_exactly`.
    pub fn advance(&mut self, sim_advance: Duration) -> u32 {
        self.accumulator += sim_advance;

        // THE TICK COUNT BY DIVISION (R03): ⌊total / dt⌋ — the exact
        // count the iterating loop computed, saturating at u32::MAX
        // (see the bounded-arithmetic doc above; the cap below clamps
        // the returned count to MAX_TICKS_PER_FRAME regardless).
        let dt = Duration::from_secs_f64(1.0 / 60.0);
        let total_nanos = self.accumulator.as_nanos();
        let mut ticks = u32::try_from(total_nanos / dt.as_nanos()).unwrap_or(u32::MAX);
        // THE SUB-TICK REMAINDER: `total mod dt` — exactly what the
        // iterating loop left in the accumulator. Strictly below one
        // dt (~16.7 ms), so the u64 conversion is lossless by
        // construction.
        self.accumulator = Duration::from_nanos(
            u64::try_from(total_nanos % dt.as_nanos())
                .expect("the sub-tick remainder is below one dt, far under u64::MAX"),
        );

        // Apply cap: if we have more ticks than allowed, drop the excess.
        // The dropped ticks are COMPLETE ticks (not partial) that exceed
        // the cap. The sub-tick remainder stays in the accumulator.
        if ticks > MAX_TICKS_PER_FRAME {
            let excess_ticks = ticks - MAX_TICKS_PER_FRAME;
            // Remove the excess complete ticks from dropped_time calculation
            let excess_time = Duration::from_secs_f64(excess_ticks as f64 * dt.as_secs_f64());
            self.dropped_time += excess_time;
            self.dropped_frames += 1;
            ticks = MAX_TICKS_PER_FRAME;
        }

        self.ticks_advanced += u64::from(ticks);
        ticks
    }

    /// R04's coherent extraction: one [`advance`], one shared
    /// per-frame consumption plan. Both subsystems consume the
    /// returned counts — the weather BEFORE the submission (finding
    /// 18's ordering: the presented frame shows one completed rain
    /// tick), the crowd AFTER the submission attempt, on EITHER
    /// outcome (presented or deferred). The day/night cycle rides the
    /// accumulator's cumulative [`ticks_advanced`](Self::ticks_advanced)
    /// as before — the monotone cycle position is consumption of the
    /// same ticks, never a re-derivation.
    pub fn advance_coherent(&mut self, sim_advance: Duration) -> CoherentTickShares {
        let ticks = self.advance(sim_advance);
        CoherentTickShares {
            weather: ticks,
            crowd: ticks,
        }
    }

    /// Accrues time WITHOUT extracting ticks: minimized-window frames
    /// collect their interval here (the campaign clock never silently
    /// pauses) and the world catches up — bounded by the cap — on the
    /// next presented frame's `advance`.
    pub fn accrue(&mut self, sim_advance: Duration) {
        self.accumulator += sim_advance;
    }

    /// Returns the current accumulator value (for diagnostics).
    pub fn accumulator(&self) -> Duration {
        self.accumulator
    }

    /// Returns the total dropped time across the session.
    pub fn dropped_time(&self) -> Duration {
        self.dropped_time
    }

    /// Returns the number of frames where time was dropped.
    pub fn dropped_frames(&self) -> u64 {
        self.dropped_frames
    }

    /// Returns the total ticks advanced (for the exit summary).
    pub fn ticks_advanced(&self) -> u64 {
        self.ticks_advanced
    }

    /// Resets the accumulator (used for tests and diagnostics).
    pub fn reset(&mut self) {
        self.accumulator = Duration::ZERO;
        self.dropped_time = Duration::ZERO;
        self.dropped_frames = 0;
        self.ticks_advanced = 0;
    }
}

impl Default for FixedTimestepAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

/// One attempted-presentation frame's coherent tick shares (R04's
/// corrective): the per-frame tick consumption plan for the free-look
/// loop's subsystems — the weather (the rain layer, plus the day/night
/// cycle through the accumulator's cumulative tick count) and the
/// crowd locomotion.
///
/// THE DISCIPLINE (one law, both subsystems): the crowd and the
/// weather consume the SAME ticks on every frame that ATTEMPTS
/// presentation, regardless of the submission's outcome. The
/// accumulator extracts the frame's ticks once
/// ([`FixedTimestepAccumulator::advance_coherent`]); the weather
/// consumes them PRE-submission (finding 18's ordering discipline —
/// the presented frame shows one completed rain tick, the order the
/// `render_weather` gates pin); the crowd consumes them AFTER the
/// submission attempt — presented OR deferred alike. The pre-fix loop
/// stepped the crowd only after a successful presentation, so a
/// DEFERRED frame (the drawable temporarily unavailable,
/// mid-transition) burned the ticks the accumulator had already
/// consumed: the rain and the day/night cycle advanced on them, the
/// crowd never stepped them, and the two clocks desynced permanently
/// — the R04 gap.
///
/// The shares are equal BY CONSTRUCTION on both arms; the type exists
/// so the lockstep is a named, unit-pinned law rather than an
/// accident of loop structure (see the inline tests
/// `deferred_frame_keeps_crowd_and_weather_in_lockstep` and
/// `deferred_frame_consumption_keeps_the_real_crowd_and_rain_in_lockstep`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoherentTickShares {
    /// The ticks the WEATHER consumes pre-submission (the rain layer;
    /// the day/night cycle consumes the accumulator's cumulative
    /// count, which this extraction already extended).
    pub weather: u32,
    /// The ticks the CROWD consumes after the submission attempt —
    /// presented or deferred alike.
    pub crowd: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_advance_yields_zero_ticks() {
        let mut acc = FixedTimestepAccumulator::new();
        assert_eq!(acc.advance(Duration::ZERO), 0);
        assert_eq!(acc.accumulator(), Duration::ZERO);
        assert_eq!(acc.ticks_advanced(), 0);
    }

    #[test]
    fn exact_one_tick_yields_one_tick() {
        let mut acc = FixedTimestepAccumulator::new();
        let dt = Duration::from_secs_f64(1.0 / 60.0);
        assert_eq!(acc.advance(dt), 1);
        assert_eq!(acc.accumulator(), Duration::ZERO);
        assert_eq!(acc.ticks_advanced(), 1);
    }

    #[test]
    fn partial_advance_yields_zero_ticks() {
        let mut acc = FixedTimestepAccumulator::new();
        let dt = Duration::from_secs_f64(1.0 / 60.0);
        assert_eq!(acc.advance(dt / 2), 0);
        assert_eq!(acc.accumulator(), dt / 2);
        assert_eq!(acc.ticks_advanced(), 0);
    }

    #[test]
    fn two_ticks_fit_yields_two_ticks() {
        let mut acc = FixedTimestepAccumulator::new();
        let dt = Duration::from_secs_f64(1.0 / 60.0);
        assert_eq!(acc.advance(dt * 2), 2);
        assert_eq!(acc.accumulator(), Duration::ZERO);
        assert_eq!(acc.ticks_advanced(), 2);
    }

    #[test]
    fn cap_restriction_applies_beyond_max() {
        let mut acc = FixedTimestepAccumulator::new();
        // 7 ticks worth of time, but cap is 5.
        let dt = Duration::from_secs_f64(1.0 / 60.0);
        let seven_ticks = dt * 7;
        assert_eq!(acc.advance(seven_ticks), 5);
        // After advancing 5 ticks, the loop removed all 7 dt values.
        // The 2 excess complete ticks are dropped (not in accumulator).
        // Sub-tick remainder is 0 (7 is exactly 7 ticks).
        assert_eq!(acc.accumulator(), Duration::ZERO);
        assert_eq!(acc.ticks_advanced(), 5);
        assert_eq!(acc.dropped_frames(), 1);
        // Dropped time is the 2 complete ticks that exceeded the cap.
        assert_eq!(acc.dropped_time(), dt * 2);
    }

    #[test]
    fn cap_at_boundary_exact() {
        let mut acc = FixedTimestepAccumulator::new();
        let dt = Duration::from_secs_f64(1.0 / 60.0);
        let five_ticks = dt * 5;
        assert_eq!(acc.advance(five_ticks), 5);
        assert_eq!(acc.accumulator(), Duration::ZERO);
        assert_eq!(acc.ticks_advanced(), 5);
        assert_eq!(acc.dropped_frames(), 0);
        assert_eq!(acc.dropped_time(), Duration::ZERO);
    }

    #[test]
    fn cap_at_boundary_plus_one() {
        let mut acc = FixedTimestepAccumulator::new();
        let dt = Duration::from_secs_f64(1.0 / 60.0);
        let six_ticks = dt * 6;
        assert_eq!(acc.advance(six_ticks), 5);
        // After advancing 5 ticks, the loop removed all 6 dt values.
        // 1 complete tick is dropped (6 - 5 = 1).
        // Sub-tick remainder is 0 (6 is exactly 6 ticks).
        assert_eq!(acc.accumulator(), Duration::ZERO);
        assert_eq!(acc.ticks_advanced(), 5);
        assert_eq!(acc.dropped_frames(), 1);
        // Dropped time is the 1 complete tick that exceeded the cap.
        assert_eq!(acc.dropped_time(), dt);
    }

    #[test]
    fn accumulated_time_persists_across_frames() {
        let mut acc = FixedTimestepAccumulator::new();
        let dt = Duration::from_secs_f64(1.0 / 60.0);
        // Frame 1: use a value that's exactly half of dt when added.
        // We use dt * 0.4 + dt * 0.6 to avoid floating point precision issues
        // with dt/2 + dt/2.
        assert_eq!(
            acc.advance(Duration::from_secs_f64(dt.as_secs_f64() * 0.4)),
            0
        );
        assert_eq!(
            acc.accumulator(),
            Duration::from_secs_f64(dt.as_secs_f64() * 0.4)
        );
        // Frame 2: add dt * 0.6 to make exactly dt.
        assert_eq!(
            acc.advance(Duration::from_secs_f64(dt.as_secs_f64() * 0.6)),
            1
        );
        assert_eq!(acc.accumulator(), Duration::ZERO);
        assert_eq!(acc.ticks_advanced(), 1);
    }

    #[test]
    fn reset_clears_all_counters() {
        let mut acc = FixedTimestepAccumulator::new();
        let dt = Duration::from_secs_f64(1.0 / 60.0);
        acc.advance(dt * 10);
        acc.reset();
        assert_eq!(acc.accumulator(), Duration::ZERO);
        assert_eq!(acc.ticks_advanced(), 0);
        assert_eq!(acc.dropped_frames(), 0);
        assert_eq!(acc.dropped_time(), Duration::ZERO);
    }

    #[test]
    fn byte_identical_at_exactly_60fps() {
        // At exactly 60 fps, each frame has exactly demo_clock_dt
        // of sim advance. The accumulator should advance exactly
        // one tick per frame, with no dropped time.
        let mut acc = FixedTimestepAccumulator::new();
        let dt = Duration::from_secs_f64(1.0 / 60.0);
        for _ in 0..100 {
            assert_eq!(acc.advance(dt), 1);
            assert_eq!(acc.accumulator(), Duration::ZERO);
            assert_eq!(acc.dropped_frames(), 0);
            assert_eq!(acc.dropped_time(), Duration::ZERO);
        }
        assert_eq!(acc.ticks_advanced(), 100);
    }

    /// The dt forms are BIT-IDENTICAL (the orchestrator's empirical
    /// check, pinned): the Duration path (nanosecond-quantized
    /// 1/60 s → f32) and the direct f32 division must produce the
    /// same float — every downstream deterministic value depends on
    /// it. If this assertion ever fires, a Duration representation
    /// change has shifted the tick delta by an ulp.
    #[test]
    fn dt_forms_are_bit_identical() {
        assert_eq!(
            demo_clock_dt().as_secs_f32().to_bits(),
            DEMO_CLOCK_DT_F32.to_bits()
        );
        assert_eq!((1.0f64 / DEMO_CLOCK_HZ) as f32, DEMO_CLOCK_DT_F32);
    }

    #[test]
    fn accrue_collects_without_stepping() {
        // Minimized frames accrue; the catch-up fires on the next
        // presented frame, bounded by the cap.
        let mut acc = FixedTimestepAccumulator::new();
        let dt = Duration::from_secs_f64(1.0 / 60.0);
        acc.accrue(dt * 3);
        assert_eq!(acc.ticks_advanced(), 0, "accrue steps nothing");
        assert_eq!(
            acc.advance(dt * 3),
            5,
            "the 6 accumulated ticks catch up to the cap"
        );
        assert_eq!(acc.dropped_frames(), 1);
        assert_eq!(acc.dropped_time(), dt);
        assert_eq!(acc.ticks_advanced(), 5);
    }

    /// R03's mandatory equivalence proof: the bounded-arithmetic
    /// `advance` computes, for a HUGE elapsed interval, the exact
    /// values the iterating form computed — asserted against the
    /// VERBATIM pre-fix loop (reimplemented here as the oracle: the
    /// loop R03's corrective deleted, byte-for-byte), over a battery
    /// that includes the one-hour single-frame stall (215,999
    /// complete ticks — the oracle performs 215,999 subtractions,
    /// the corrective performs none), the six-hour stall, the
    /// minimized-accrue catch-up (the R03 exposure path: an hour
    /// ACCRUED while minimized, then extracted by one presented
    /// frame), the cap boundaries, sub-tick splits, and a
    /// deterministic 700-frame mixed accrue/advance sequence. The
    /// O(1) claim itself is structural — `advance` contains no
    /// data-dependent iteration (the while loop is gone from the
    /// diff) — this test pins that the arithmetic that replaced it
    /// is EXACT for every battery value: the same returned tick
    /// count, the same sub-tick remainder, the same
    /// dropped_time/dropped_frames accounting, the same cumulative
    /// ticks_advanced.
    #[test]
    fn huge_stall_advances_in_o1_matching_the_iterating_form_exactly() {
        /// The VERBATIM pre-fix advance (the iterating loop R03
        /// replaced) over a minimal field mirror — the oracle.
        struct IteratingReference {
            accumulator: Duration,
            dropped_time: Duration,
            dropped_frames: u64,
            ticks_advanced: u64,
        }
        impl IteratingReference {
            fn new() -> Self {
                Self {
                    accumulator: Duration::ZERO,
                    dropped_time: Duration::ZERO,
                    dropped_frames: 0,
                    ticks_advanced: 0,
                }
            }
            fn accrue(&mut self, sim_advance: Duration) {
                self.accumulator += sim_advance;
            }
            fn advance(&mut self, sim_advance: Duration) -> u32 {
                self.accumulator += sim_advance;
                let mut ticks = 0u32;
                let dt = Duration::from_secs_f64(1.0 / 60.0);
                while self.accumulator >= dt {
                    ticks += 1;
                    self.accumulator -= dt;
                }
                if ticks > MAX_TICKS_PER_FRAME {
                    let excess_ticks = ticks - MAX_TICKS_PER_FRAME;
                    let excess_time =
                        Duration::from_secs_f64(excess_ticks as f64 * dt.as_secs_f64());
                    self.dropped_time += excess_time;
                    self.dropped_frames += 1;
                    ticks = MAX_TICKS_PER_FRAME;
                }
                self.ticks_advanced += u64::from(ticks);
                ticks
            }
        }
        #[derive(Clone, Copy)]
        enum Feed {
            Advance(Duration),
            Accrue(Duration),
        }
        let dt = Duration::from_secs_f64(1.0 / 60.0);
        let cases: &[(&str, Vec<Feed>)] = &[
            ("zero", vec![Feed::Advance(Duration::ZERO)]),
            ("one tick exactly", vec![Feed::Advance(dt)]),
            ("sub-tick half", vec![Feed::Advance(dt / 2)]),
            ("two ticks", vec![Feed::Advance(dt * 2)]),
            ("cap boundary at five", vec![Feed::Advance(dt * 5)]),
            ("cap plus one at six", vec![Feed::Advance(dt * 6)]),
            (
                "cap plus two with a remainder",
                vec![Feed::Advance(dt * 7 + dt / 3)],
            ),
            (
                "the 0.4/0.6 sub-tick split across frames",
                vec![
                    Feed::Advance(Duration::from_secs_f64(dt.as_secs_f64() * 0.4)),
                    Feed::Advance(Duration::from_secs_f64(dt.as_secs_f64() * 0.6)),
                ],
            ),
            (
                "minimized accrue then the presented catch-up",
                vec![Feed::Accrue(dt * 3), Feed::Advance(dt * 3)],
            ),
            (
                "THE ONE-HOUR SINGLE-FRAME STALL",
                vec![Feed::Advance(Duration::from_secs(3600))],
            ),
            (
                "the six-hour stall",
                vec![Feed::Advance(Duration::from_secs(6 * 3600))],
            ),
            (
                "an hour ACCRUED while minimized, then one presented frame",
                vec![Feed::Accrue(Duration::from_secs(3600)), Feed::Advance(dt)],
            ),
            (
                "hourly stalls across a session",
                (0..4)
                    .map(|_| Feed::Advance(Duration::from_secs(3600)))
                    .collect(),
            ),
        ];
        for (label, feeds) in cases {
            let mut fixed = FixedTimestepAccumulator::new();
            let mut reference = IteratingReference::new();
            for feed in feeds {
                match *feed {
                    Feed::Advance(d) => assert_eq!(
                        fixed.advance(d),
                        reference.advance(d),
                        "{label}: the returned tick count diverged"
                    ),
                    Feed::Accrue(d) => {
                        fixed.accrue(d);
                        reference.accrue(d);
                    }
                }
            }
            assert_eq!(
                fixed.accumulator(),
                reference.accumulator,
                "{label}: the sub-tick remainder diverged"
            );
            assert_eq!(
                fixed.ticks_advanced(),
                reference.ticks_advanced,
                "{label}: the cumulative tick count diverged"
            );
            assert_eq!(
                fixed.dropped_time(),
                reference.dropped_time,
                "{label}: the dropped-time accounting diverged"
            );
            assert_eq!(
                fixed.dropped_frames(),
                reference.dropped_frames,
                "{label}: the dropped-frames accounting diverged"
            );
        }
        // THE DETERMINISTIC MIXED SEQUENCE: 700 frames of a tiny LCG's
        // advances (0..=8 complete ticks plus a sub-tick remainder, so
        // the cap and the drop accounting fire too) with a minimized
        // accrue every 97th frame — every return value compared at the
        // feed, the final state compared field-for-field.
        let dt_nanos = dt.as_nanos() as u64;
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut draw = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            state >> 33
        };
        let mut fixed = FixedTimestepAccumulator::new();
        let mut reference = IteratingReference::new();
        for frame in 0..700u64 {
            let nanos = draw() % 9 * dt_nanos + draw() % dt_nanos;
            let d = Duration::from_nanos(nanos);
            if frame % 97 == 0 {
                fixed.accrue(d);
                reference.accrue(d);
            } else {
                assert_eq!(
                    fixed.advance(d),
                    reference.advance(d),
                    "the mixed sequence's frame {frame}"
                );
            }
        }
        assert_eq!(fixed.accumulator(), reference.accumulator);
        assert_eq!(fixed.ticks_advanced(), reference.ticks_advanced);
        assert_eq!(fixed.dropped_time(), reference.dropped_time);
        assert_eq!(fixed.dropped_frames(), reference.dropped_frames);
    }

    /// The saturation boundary (R03): accumulated time beyond
    /// `u32::MAX` ticks (~828 days) had NO defined semantics in the
    /// iterating form (debug builds panicked on the tick counter's
    /// overflow; release builds silently wrapped and corrupted the
    /// drop accounting). The bounded form saturates the tick count —
    /// the returned count still lands on the cap, the accounting
    /// stays finite, and the sub-tick remainder stays a remainder.
    #[test]
    fn saturation_beyond_the_u32_domain_still_caps_cleanly() {
        let mut acc = FixedTimestepAccumulator::new();
        // 1000 days of accumulated time in one extraction — ~5.18
        // billion ticks, beyond u32::MAX (~4.29 billion): the
        // iterating form's undefined domain.
        let stalled = Duration::from_secs(1000 * 24 * 3600);
        assert_eq!(acc.advance(stalled), MAX_TICKS_PER_FRAME);
        assert_eq!(
            acc.ticks_advanced(),
            u64::from(MAX_TICKS_PER_FRAME),
            "the world advances exactly the cap's ticks"
        );
        assert_eq!(acc.dropped_frames(), 1);
        // The saturated drop accounting: u32::MAX − 5 ticks' worth —
        // ~828 days (finite and ordered, where the release-mode
        // iterating form wrapped and the debug form panicked).
        assert!(acc.dropped_time() > Duration::from_secs(800 * 24 * 3600));
        assert!(acc.dropped_time() < Duration::from_secs(830 * 24 * 3600));
        // The remainder is still below one dt (the extraction's
        // invariant, saturated domain included).
        let dt = Duration::from_secs_f64(1.0 / 60.0);
        assert!(acc.accumulator() < dt);
    }

    /// R04's mandatory lockstep proof (the consumption discipline,
    /// unit level): a deferred-presentation frame's tick accounting
    /// keeps the crowd and the weather on the SAME ticks. The render
    /// harness cannot construct a genuine deferred submission (the
    /// drawable's mid-transition state is a renderer/window
    /// condition — the free-look loop's `frame.deferred` branch), so
    /// this pins the DISCIPLINE through `advance_coherent` over the
    /// finding's exact frame shape — presented, minimized (accrues
    /// only), DEFERRED (presents nothing, retries), presented (the
    /// catch-up) — with the PRE-FIX discipline (the crowd's
    /// consumption gated on presentation success) modeled inline as
    /// the discriminating reference: it DROPS the deferred frame's
    /// crowd ticks — asserted, the gap the finding named — while the
    /// coherent shares hold the lockstep (crowd total == weather
    /// total == the accumulator's ticks_advanced, the minimized frame
    /// coherent by absence).
    #[test]
    fn deferred_frame_keeps_crowd_and_weather_in_lockstep() {
        let dt = Duration::from_secs_f64(1.0 / 60.0);
        enum Outcome {
            Presented,
            Deferred,
            Minimized,
        }
        let sequence = [
            (dt, Outcome::Presented),
            (dt, Outcome::Minimized),
            (dt, Outcome::Deferred),
            (dt, Outcome::Presented),
        ];
        let mut acc = FixedTimestepAccumulator::new();
        let mut weather_total = 0u64;
        let mut crowd_total = 0u64;
        let mut pre_fix_crowd_total = 0u64;
        for (sim_advance, outcome) in sequence {
            match outcome {
                Outcome::Minimized => {
                    // The minimized branch: accrue only — both
                    // subsystems consume nothing (coherent by
                    // absence; the catch-up consumes the same shares
                    // for both on the next presented frame).
                    acc.accrue(sim_advance);
                }
                Outcome::Deferred | Outcome::Presented => {
                    let shares = acc.advance_coherent(sim_advance);
                    // The weather consumes pre-submission — finding
                    // 18's order, on EVERY attempted frame.
                    weather_total += u64::from(shares.weather);
                    // The crowd consumes post-attempt — the R04
                    // discipline, presented or deferred alike.
                    crowd_total += u64::from(shares.crowd);
                    // The PRE-FIX discipline (the discriminating
                    // reference): the crowd stepped only on presented
                    // frames, so the deferred frame's ticks burned.
                    if matches!(outcome, Outcome::Presented) {
                        pre_fix_crowd_total += u64::from(shares.crowd);
                    }
                }
            }
        }
        // The extraction: 1 tick, then the accrued+deferred frame
        // extracts 2, then 1 — four ticks total.
        assert_eq!(acc.ticks_advanced(), 4);
        assert_eq!(
            pre_fix_crowd_total, 2,
            "the pre-fix discipline dropped the deferred frame's 2 crowd ticks"
        );
        assert_ne!(
            pre_fix_crowd_total, weather_total,
            "THE R04 GAP, demonstrated: the pre-fix crowd fell behind the weather"
        );
        assert_eq!(
            crowd_total, weather_total,
            "the coherent shares hold the lockstep: both subsystems consume the same ticks"
        );
        assert_eq!(
            crowd_total,
            acc.ticks_advanced(),
            "every extracted tick is consumed by both subsystems"
        );
    }
}
