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
    pub fn advance(&mut self, sim_advance: Duration) -> u32 {
        self.accumulator += sim_advance;

        // Count how many complete ticks fit in the accumulator.
        let mut ticks = 0u32;
        let dt = Duration::from_secs_f64(1.0 / 60.0);
        while self.accumulator >= dt {
            ticks += 1;
            self.accumulator -= dt;
        }

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
}
