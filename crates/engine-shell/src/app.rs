//! The Stage 1 application shell: window + event pump + input intents +
//! a CPU-paced application loop with BOUNDED frame-interval retention
//! (R05, the A21 corrective's shape in the shell: the loop's retained
//! interval sample is O(capacity) resident memory, never O(frames); the
//! frame count stays exact in every mode).

use std::num::NonZeroUsize;
use std::thread;
use std::time::{Duration, Instant};

use engine_core::diag::{Diagnostics, Level};
use sdl3::EventPump;
use sdl3::event::{Event, WindowEvent};
use sdl3::mouse::MouseUtil;
use sdl3::video::{Window, WindowBuildError};

use crate::input::{InputSnapshot, InputState};
use crate::platform::{DriverPreference, Platform};

/// The hybrid pacing spin margin: sleep until `target - SPIN_MARGIN`,
/// then spin the remainder. `thread::sleep` on Windows quantizes to
/// the OS timer resolution (1–15.6 ms depending on timeBeginPeriod),
/// which alone can exceed the ENTIRE budget at high target rates
/// (6.9 ms at 144 FPS, 4.2 ms at 240 FPS) — the bounded spin removes
/// that quantization from the measured frame-interval tail. The cost
/// is at most 2 ms of one core per PACED frame (uncapped frames never
/// pace); OS time still never enters simulation logic (the loop's own
/// Stage 1 invariant — this is presentation policy only).
pub const SPIN_MARGIN: Duration = Duration::from_millis(2);

/// Hybrid pacing to an absolute target instant (perf batch 3): sleep
/// most of the gap, then spin the last [`SPIN_MARGIN`] — the shared
/// form of [`ShellApp::run`]'s cadence and the graphical demos' own
/// `--fps` pacing (the former sleep-only form made the frame-interval
/// p99 measure the OS timer, not the engine, at high target rates).
pub fn pace_until(target: Instant) {
    let now = Instant::now();
    if target <= now {
        return;
    }
    let wake_at = target - SPIN_MARGIN;
    if wake_at > now {
        thread::sleep(wake_at - now);
    }
    while Instant::now() < target {
        core::hint::spin_loop();
    }
}

/// Configuration for [`ShellApp::new`].
pub struct ShellConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
    /// Target loop rate for frame pacing. Pacing uses OS monotonic time
    /// exclusively — the authoritative campaign simulation clock belongs
    /// to engine-core, is not present in Stage 1, and OS time never enters
    /// simulation logic (Stage 1 authorization, item 2).
    pub target_fps: u32,
    pub driver: DriverPreference,
}

/// What the frame callback wants the loop to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopControl {
    Continue,
    Quit,
}

/// Why the application loop stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitReason {
    /// A window close request or SDL quit event arrived.
    Requested,
    /// The frame callback returned [`LoopControl::Quit`].
    Callback,
}

/// The default bound of the loop's frame-interval retention: the 4096
/// most-recent intervals (64 KiB at 16 bytes per `Duration`) — the A21
/// default carried into the shell. Large enough that the Stage 1
/// acceptance measurement stays whole-run exact (the 60-second soak at
/// its 60 fps target is ≈3601 frames, inside the bound); small enough
/// that no run length can grow the shell's resident memory without
/// bound (the pre-R05 `Vec` grew one entry per frame for the whole
/// run — NEW-SHELL-4).
pub const DEFAULT_INTERVAL_RETENTION: usize = 4096;

/// How [`ShellApp::run`] retains frame intervals for [`FrameStats`]
/// (R05, the bounded-retention policy — the A21 corrective's shape in
/// the shell).
///
/// The run's frame COUNT is exact in every mode (a plain `u64`
/// lifetime counter, never sampled). What the policy governs is the
/// percentile sample:
///
/// - [`IntervalRetention::Bounded`] (the default): the most recent
///   `capacity` intervals are retained. When the store is full the
///   OLDEST half is evicted (amortized O(1) per frame — one drain per
///   `capacity/2` pushes, the A21 half-drain) and every eviction is
///   counted in the run's `dropped_total`, observable through the
///   [`IntervalSample`] in [`RunSummary`] — a bounded store never
///   silently loses the knowledge that it lost intervals. Resident
///   memory is O(capacity), not O(frames).
/// - [`IntervalRetention::Exact`]: the documented escape hatch for
///   MEASUREMENT routes — every interval of the run is retained, so
///   [`FrameStats`] is identical to the historical whole-run form.
///   Memory grows one entry per frame for the whole run; take it only
///   for duration-bounded benchmark runs, never for interactive ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntervalRetention {
    /// Retain at most `capacity` most-recent intervals. `NonZeroUsize`
    /// so a store that retains nothing — and can therefore answer no
    /// percentile at all — cannot be constructed.
    Bounded(NonZeroUsize),
    /// Retain every interval of the run (measurement routes only).
    Exact,
}

impl Default for IntervalRetention {
    /// The default policy: the bounded window at
    /// [`DEFAULT_INTERVAL_RETENTION`].
    fn default() -> Self {
        Self::Bounded(
            NonZeroUsize::new(DEFAULT_INTERVAL_RETENTION)
                .expect("DEFAULT_INTERVAL_RETENTION is non-zero by construction"),
        )
    }
}

/// Frame-interval statistics over a whole run.
///
/// PROVENANCE (the R05 statistics-honesty record): `count`, `avg_ms`
/// and `max_ms` are EXACT over the whole run in every retention mode —
/// a lifetime frame counter, a running interval sum, and a running
/// interval max, each O(1) memory. `p50_ms`, `p95_ms` and `p99_ms` are
/// nearest-rank order statistics over the intervals the run RETAINED:
/// every interval under [`IntervalRetention::Exact`], otherwise the
/// most recent bounded window. A sampled percentile is labeled as such
/// by the [`IntervalSample`] carried next to it in [`RunSummary`] — it
/// never silently passes itself off as a whole-run figure.
///
/// Sample quality at the default bound (4096 intervals): the
/// nearest-rank p50/p95/p99 of a 4096-interval window resolve their
/// RANK to roughly ±0.8/±0.3/±0.2 percentile points of the underlying
/// distribution (binomial rank noise `sqrt(p(1-p)/N)`), and the window
/// samples the run's RECENT pacing regime rather than its whole
/// history — exactly the recency a pacing diagnostic wants, and exactly
/// why a benchmark report opts into [`IntervalRetention::Exact`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameStats {
    pub count: u64,
    pub avg_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

impl FrameStats {
    /// The exact whole-run form: nearest-rank percentiles over the FULL
    /// interval slice. This is the pre-R05 computation, unchanged — the
    /// exact-retention path's [`FrameStats`] is identical to the
    /// historical one (pinned against an independent oracle by the
    /// inline equivalence test), and the bounded path reuses it over
    /// the retained window for the percentile fields.
    fn from_intervals(intervals: &[Duration]) -> FrameStats {
        if intervals.is_empty() {
            return FrameStats {
                count: 0,
                avg_ms: 0.0,
                p50_ms: 0.0,
                p95_ms: 0.0,
                p99_ms: 0.0,
                max_ms: 0.0,
            };
        }
        let mut ms: Vec<f64> = intervals.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
        ms.sort_by(|a, b| a.total_cmp(b));
        let pick = |q: f64| {
            let idx = ((q * ms.len() as f64).ceil() as usize)
                .saturating_sub(1)
                .min(ms.len() - 1);
            ms[idx]
        };
        FrameStats {
            count: ms.len() as u64,
            avg_ms: ms.iter().sum::<f64>() / ms.len() as f64,
            p50_ms: pick(0.50),
            p95_ms: pick(0.95),
            p99_ms: pick(0.99),
            max_ms: ms[ms.len() - 1],
        }
    }
}

/// What [`FrameStats`]'s percentile fields were computed over for one
/// run — the R05 honesty surface. A bounded sample never silently
/// passes itself off as the whole run: `exact` names the retention
/// mode that produced the numbers, `retained` is the sample size the
/// percentiles actually saw, `dropped_total` is the run's lifetime
/// eviction count (always 0 under exact retention), and `capacity` is
/// the bound itself (absent under exact retention).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntervalSample {
    /// Whether the percentiles are whole-run exact (`true` only under
    /// [`IntervalRetention::Exact`]).
    pub exact: bool,
    /// The number of intervals the percentiles were computed over.
    pub retained: usize,
    /// The run's lifetime count of intervals evicted by the bound.
    pub dropped_total: u64,
    /// The retention bound (`None` under exact retention).
    pub capacity: Option<NonZeroUsize>,
}

/// The loop's frame-interval collector (R05): the exact lifetime
/// aggregates plus the retention-bounded percentile sample. Private —
/// the public surface is [`FrameStats`] (the numbers) and
/// [`IntervalSample`] (their provenance). Per-run like the `Vec` it
/// replaces, so `dropped_total` is a run-lifetime counter.
struct IntervalCollector {
    retention: IntervalRetention,
    /// The retained intervals: the whole run under
    /// [`IntervalRetention::Exact`], the most recent window otherwise.
    intervals: Vec<Duration>,
    /// The exact lifetime frame count — never sampled.
    frames: u64,
    /// Lifetime evictions by the retention bound (this run's).
    dropped_total: u64,
    /// Exact running interval sum in nanoseconds (exact as an `f64`
    /// for any run accumulating less than 2^53 nanoseconds ≈ 104 days
    /// of frame time — beyond any shell session, and past it the
    /// conversion rounds once, still beats the per-term `f64` sum it
    /// replaces).
    total_nanos: u128,
    /// Exact running interval max.
    max: Duration,
}

impl IntervalCollector {
    fn new(retention: IntervalRetention) -> Self {
        IntervalCollector {
            retention,
            intervals: Vec::new(),
            frames: 0,
            dropped_total: 0,
            total_nanos: 0,
            max: Duration::ZERO,
        }
    }

    /// Records one frame interval: the exact aggregates first (every
    /// mode), then the retained sample under the retention policy.
    /// When the bounded store is full the OLDEST half is evicted
    /// (amortized O(1) per frame — one drain per `capacity/2` pushes,
    /// the A21 half-drain) and counted in `dropped_total`.
    fn record(&mut self, interval: Duration) {
        self.frames += 1;
        self.total_nanos += interval.as_nanos();
        if interval > self.max {
            self.max = interval;
        }
        if let IntervalRetention::Bounded(capacity) = self.retention
            && self.intervals.len() == capacity.get()
        {
            // Evict the oldest half (at least one — a capacity of 1
            // must still make room).
            let evict = (capacity.get() / 2).max(1);
            self.intervals.drain(..evict);
            self.dropped_total += evict as u64;
        }
        self.intervals.push(interval);
    }

    /// The exact lifetime frame count.
    fn frames(&self) -> u64 {
        self.frames
    }

    /// The run's [`FrameStats`]. Exact retention answers the historical
    /// whole-run form verbatim; bounded retention answers the exact
    /// `count`/`avg`/`max` over the whole run with the percentiles over
    /// the retained window (the provenance is [`Self::sample`]).
    fn stats(&self) -> FrameStats {
        match self.retention {
            IntervalRetention::Exact => FrameStats::from_intervals(&self.intervals),
            IntervalRetention::Bounded(_) => {
                let mut stats = FrameStats::from_intervals(&self.intervals);
                if self.frames == 0 {
                    return stats;
                }
                stats.count = self.frames;
                stats.avg_ms = self.total_nanos as f64 / (1e6 * self.frames as f64);
                stats.max_ms = self.max.as_secs_f64() * 1000.0;
                stats
            }
        }
    }

    /// The provenance of [`Self::stats`]'s percentile fields — what
    /// sample they were computed over (never a silent sample).
    fn sample(&self) -> IntervalSample {
        IntervalSample {
            exact: matches!(self.retention, IntervalRetention::Exact),
            retained: self.intervals.len(),
            dropped_total: self.dropped_total,
            capacity: match self.retention {
                IntervalRetention::Bounded(capacity) => Some(capacity),
                IntervalRetention::Exact => None,
            },
        }
    }
}

/// Summary of one application-loop run.
pub struct RunSummary {
    pub frames: u64,
    pub wall_duration: Duration,
    pub frame_stats: FrameStats,
    /// The provenance of `frame_stats`'s percentile fields (R05):
    /// whole-run exact or bounded most-recent window, the sample size
    /// the percentiles saw, and the run's eviction count.
    pub interval_sample: IntervalSample,
    pub quit_reason: QuitReason,
    pub rss_start_kib: Option<u64>,
    pub rss_end_kib: Option<u64>,
}

/// The Stage 1 application shell: a real SDL3 window, the OS event pump,
/// the input-intent state machine, and a CPU-driven paced loop. Rendering
/// and presentation are Stage 2 and are absent by authorization.
pub struct ShellApp {
    platform: Platform,
    window: Window,
    pump: EventPump,
    mouse: MouseUtil,
    input: InputState,
    diag: Diagnostics,
    frame_target: Duration,
    interval_retention: IntervalRetention,
}

impl ShellApp {
    pub fn new(config: ShellConfig) -> Result<ShellApp, String> {
        let platform = Platform::init(config.driver)?;
        let video = platform.sdl().video().map_err(|e| e.to_string())?;
        let window = video
            .window(&config.title, config.width, config.height)
            .position_centered()
            .resizable()
            .build()
            .map_err(|e: WindowBuildError| e.to_string())?;
        let pump = platform.sdl().event_pump().map_err(|e| e.to_string())?;
        let mouse = platform.sdl().mouse();
        let frame_target = Duration::from_secs_f64(1.0 / f64::from(config.target_fps.max(1)));
        Ok(ShellApp {
            platform,
            window,
            pump,
            mouse,
            input: InputState::new(),
            diag: Diagnostics::new(Level::Off),
            frame_target,
            interval_retention: IntervalRetention::default(),
        })
    }

    /// The SDL window id (events are not filtered by id; Stage 1 is a
    /// single-window shell).
    pub fn window_id(&self) -> u32 {
        self.window.id()
    }

    pub fn window_size(&self) -> (u32, u32) {
        self.window.size()
    }

    /// The underlying SDL3 window — Stage 2 renderer surface creation.
    pub fn window(&self) -> &Window {
        &self.window
    }

    pub fn is_held(&self, key: sdl3::keyboard::Keycode) -> bool {
        self.input.is_held(key)
    }

    /// Applies relative mouse capture to the window and mirrors it in the
    /// input state (look deltas are collected only while captured).
    pub fn set_relative_mouse(&mut self, on: bool) {
        self.mouse.set_relative_mouse_mode(&self.window, on);
        self.input.set_relative_capture(on);
    }

    /// The relative-mouse mode SDL reports for this window.
    pub fn relative_mouse(&self) -> bool {
        self.mouse.relative_mouse_mode(&self.window)
    }

    /// Raises the shell diagnostics level. Focus loss/regain records are
    /// logged at Info.
    pub fn set_diag_level(&mut self, level: Level) {
        self.diag.set_min_level(level);
    }

    /// The shell's diagnostics records (shared engine-core subsystem).
    pub fn diagnostics(&self) -> &Diagnostics {
        &self.diag
    }

    /// Sets the loop's frame-interval retention policy (R05). The
    /// default is the bounded most-recent window at
    /// [`DEFAULT_INTERVAL_RETENTION`] — O(capacity) resident memory for
    /// any run length. Measurement routes that must report whole-run
    /// exact percentiles opt into [`IntervalRetention::Exact`] (the
    /// documented escape hatch — memory then grows one entry per frame
    /// for the run's duration, so take it only for duration-bounded
    /// benchmark runs). The frame count is exact in every mode.
    /// [`ShellApp::run`] snapshots the policy when the loop starts; a
    /// change made from inside a frame callback applies to the next
    /// run.
    pub fn set_interval_retention(&mut self, retention: IntervalRetention) {
        self.interval_retention = retention;
    }

    /// Pushes an event onto SDL's queue. Used by tests and tooling — this is
    /// event dispatch, not physical device input.
    pub fn push_event(&mut self, event: Event) -> Result<(), String> {
        let events = self.platform.sdl().event().map_err(|e| e.to_string())?;
        events.push_event(event).map_err(|e| e.to_string())
    }

    /// Pumps pending OS events through the input state machine and returns
    /// this frame's intent snapshot. Focus transitions are recorded in the
    /// shell's diagnostics at Info (Stage 1 corrective pass, review
    /// finding 4) — core input behavior is unchanged.
    pub fn frame(&mut self) -> InputSnapshot {
        for event in self.pump.poll_iter() {
            if let Event::Window { win_event, .. } = &event {
                match win_event {
                    WindowEvent::FocusLost => self.diag.log(
                        Level::Info,
                        "keyboard focus lost — held keys and pending look deltas cleared",
                    ),
                    WindowEvent::FocusGained => {
                        self.diag.log(Level::Info, "keyboard focus regained");
                    }
                    _ => {}
                }
            }
            self.input.process(&event);
        }
        self.input.poll()
    }

    /// Runs the CPU-paced application loop until quit is requested or the
    /// callback returns [`LoopControl::Quit`]. Pacing uses OS monotonic
    /// time exclusively; if the loop falls behind the target cadence it
    /// resets the cadence instead of busy-catching-up. Frame-interval
    /// statistics are collected under the shell's interval-retention
    /// policy ([`Self::set_interval_retention`], R05); the frame count
    /// is exact in every mode.
    pub fn run<F>(&mut self, mut on_frame: F) -> RunSummary
    where
        F: FnMut(&mut ShellApp, &InputSnapshot) -> LoopControl,
    {
        let rss_start = resident_set_kib();
        let run_start = Instant::now();
        let mut frame_begin = run_start;
        let mut next_frame_at = run_start;
        let mut intervals = IntervalCollector::new(self.interval_retention);
        let mut control = LoopControl::Continue;
        let mut quit_reason = QuitReason::Callback;

        while control == LoopControl::Continue && !self.input.quit_requested() {
            next_frame_at += self.frame_target;
            let snapshot = self.frame();
            control = on_frame(self, &snapshot);
            if self.input.quit_requested() {
                // A window close or SDL quit request is the observable
                // cause even when the callback also quits on seeing it
                // (Stage 1 corrective pass, review finding 3).
                quit_reason = QuitReason::Requested;
            } else if control == LoopControl::Quit {
                quit_reason = QuitReason::Callback;
            }
            let frame_end = Instant::now();
            if next_frame_at > frame_end {
                pace_until(next_frame_at);
            } else {
                next_frame_at = frame_end;
            }
            let next_begin = Instant::now();
            intervals.record(next_begin - frame_begin);
            frame_begin = next_begin;
        }

        RunSummary {
            frames: intervals.frames(),
            wall_duration: run_start.elapsed(),
            frame_stats: intervals.stats(),
            interval_sample: intervals.sample(),
            quit_reason,
            rss_start_kib: rss_start,
            rss_end_kib: resident_set_kib(),
        }
    }

    /// Kept for Stage 3: the platform handle that will host the
    /// campaign-clock integration behind its own gate.
    pub fn platform(&self) -> &Platform {
        &self.platform
    }
}

/// Resident set size in KiB (Linux only; `None` elsewhere). Used for soak
/// memory-trend reporting.
#[cfg(target_os = "linux")]
pub fn resident_set_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            return rest.trim().trim_end_matches("kB").trim().parse().ok();
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
pub fn resident_set_kib() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pre-R05 whole-run computation, VERBATIM — the independent
    /// equivalence oracle (the S6/C4 precedent: the old body is
    /// deliberately re-implemented here, separate from
    /// `FrameStats::from_intervals`, so any drift in the shipped form
    /// fails the pin instead of agreeing with itself).
    fn legacy_full_vec_stats(intervals: &[Duration]) -> FrameStats {
        if intervals.is_empty() {
            return FrameStats {
                count: 0,
                avg_ms: 0.0,
                p50_ms: 0.0,
                p95_ms: 0.0,
                p99_ms: 0.0,
                max_ms: 0.0,
            };
        }
        let mut ms: Vec<f64> = intervals.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
        ms.sort_by(|a, b| a.total_cmp(b));
        let pick = |q: f64| {
            let idx = ((q * ms.len() as f64).ceil() as usize)
                .saturating_sub(1)
                .min(ms.len() - 1);
            ms[idx]
        };
        FrameStats {
            count: ms.len() as u64,
            avg_ms: ms.iter().sum::<f64>() / ms.len() as f64,
            p50_ms: pick(0.50),
            p95_ms: pick(0.95),
            p99_ms: pick(0.99),
            max_ms: ms[ms.len() - 1],
        }
    }

    /// R05 (1), the equivalence: the exact-retention path's FrameStats
    /// is identical — field for field, bit for bit — to the pre-R05
    /// full-Vec form across fixture interval sequences, including the
    /// empty, single-frame, and long-run shapes.
    #[test]
    fn exact_retention_matches_the_pre_r05_full_vec_form() {
        let steady = |i: u64| Duration::from_nanos(16_666_667 + (i % 97) * 31);
        let fixtures: Vec<(&str, Vec<Duration>)> = vec![
            ("empty", vec![]),
            ("single frame", vec![Duration::from_millis(17)]),
            (
                "two frames",
                vec![Duration::from_millis(16), Duration::from_millis(18)],
            ),
            ("all-equal", vec![Duration::from_millis(16); 64]),
            ("zero intervals", vec![Duration::ZERO; 7]),
            (
                "monotone ramp",
                (0..100)
                    .map(|i| Duration::from_nanos(1_000_000 + i * 50_000))
                    .collect(),
            ),
            (
                "steady with tail spikes",
                (0..600)
                    .map(|i| {
                        if i % 97 == 0 {
                            Duration::from_millis(41)
                        } else {
                            Duration::from_nanos(16_666_667)
                        }
                    })
                    .collect(),
            ),
            ("long run", (0..10_000).map(steady).collect()),
        ];
        for (name, sequence) in fixtures {
            let mut collector = IntervalCollector::new(IntervalRetention::Exact);
            for interval in &sequence {
                collector.record(*interval);
            }
            assert_eq!(
                collector.stats(),
                legacy_full_vec_stats(&sequence),
                "fixture {name}: exact retention must be the historical form"
            );
            // The exact path never evicts: the whole run is the sample.
            assert_eq!(collector.frames(), sequence.len() as u64, "fixture {name}");
            let sample = collector.sample();
            assert!(
                sample.exact
                    && sample.capacity.is_none()
                    && sample.dropped_total == 0
                    && sample.retained == sequence.len(),
                "fixture {name}: the exact sample is labeled exact"
            );
        }
    }

    /// R05 (2), the bound: the bounded retention evicts oldest-first in
    /// amortized halves, keeps the MOST RECENT intervals, never exceeds
    /// capacity, and counts every eviction exactly (the A21 template:
    /// 10 recorded, 4 retained → 6 evicted in three half-drains of 2).
    #[test]
    fn bounded_retention_evicts_oldest_and_counts_dropped() {
        let mut collector =
            IntervalCollector::new(IntervalRetention::Bounded(NonZeroUsize::new(4).unwrap()));
        let sequence: Vec<Duration> = (0..10).map(|i| Duration::from_nanos(1_000 + i)).collect();
        for interval in &sequence {
            collector.record(*interval);
            assert!(
                collector.intervals.len() <= 4,
                "resident entries never exceed capacity"
            );
        }
        // The retained set is the most recent 4, in order.
        assert_eq!(collector.intervals, &sequence[6..]);
        // 10 recorded, 4 retained → 6 evicted (three half-drains of 2).
        assert_eq!(collector.dropped_total, 6);
        assert_eq!(collector.frames(), 10);
        // The exact aggregates survive the evictions: count, avg, and
        // max cover the WHOLE run, not the window.
        let stats = collector.stats();
        assert_eq!(stats.count, 10);
        let expected_avg = sequence.iter().map(|d| d.as_nanos()).sum::<u128>() as f64
            / (1e6 * sequence.len() as f64);
        assert!((stats.avg_ms - expected_avg).abs() < 1e-12);
        assert_eq!(stats.max_ms, legacy_full_vec_stats(&sequence).max_ms);

        // A capacity of 1 still makes room (the max(1) eviction guard).
        let mut one =
            IntervalCollector::new(IntervalRetention::Bounded(NonZeroUsize::new(1).unwrap()));
        for i in 0..10 {
            one.record(Duration::from_nanos(5_000 + i));
        }
        assert_eq!(one.intervals, &[Duration::from_nanos(5_009)]);
        assert_eq!(one.dropped_total, 9);
        assert_eq!(one.frames(), 10);

        // An odd capacity evicts floor(capacity/2): the arithmetic stays
        // exact at every step (dropped + retained == frames).
        let mut odd =
            IntervalCollector::new(IntervalRetention::Bounded(NonZeroUsize::new(5).unwrap()));
        for i in 0..10u64 {
            odd.record(Duration::from_nanos(2_000 + i));
            assert_eq!(
                odd.dropped_total + odd.intervals.len() as u64,
                i + 1,
                "dropped + retained == frames at every step"
            );
        }
        // r1–r5 fill; r6 evicts 2; r7 refills; r8 evicts 2; r9 refills;
        // r10 evicts 2 → three drains of 2, 6 dropped, 4 retained.
        assert_eq!(odd.dropped_total, 6);
        assert_eq!(odd.intervals.len(), 4);
    }

    /// R05 (2), the long run: across a 10_000-frame fixture run the
    /// resident entry count NEVER exceeds capacity, the retained window
    /// is always exactly the most recent intervals in order, and
    /// dropped + retained == frames exactly.
    #[test]
    fn bounded_retention_never_exceeds_capacity_across_a_long_run() {
        let capacity = NonZeroUsize::new(64).unwrap();
        let mut collector = IntervalCollector::new(IntervalRetention::Bounded(capacity));
        let mut sequence: Vec<Duration> = Vec::new();
        for i in 0..10_000u64 {
            let interval = Duration::from_nanos(16_600_000 + (i % 991) * 2_700);
            sequence.push(interval);
            collector.record(interval);
            assert!(
                collector.intervals.len() <= capacity.get(),
                "resident entries never exceed capacity (frame {i})"
            );
        }
        let sample = collector.sample();
        assert!(!sample.exact);
        assert_eq!(sample.capacity, Some(capacity));
        assert_eq!(sample.retained, collector.intervals.len());
        assert_eq!(sample.dropped_total + sample.retained as u64, 10_000);
        // The retained window is exactly the most recent intervals.
        let retained = collector.intervals.len();
        assert_eq!(collector.intervals, &sequence[sequence.len() - retained..]);
        // The frame count is exact under bounded retention with heavy
        // eviction, and the exact aggregates cover the whole run.
        assert_eq!(collector.frames(), 10_000);
        let stats = collector.stats();
        assert_eq!(stats.count, 10_000);
        assert_eq!(
            stats.max_ms,
            legacy_full_vec_stats(&sequence).max_ms,
            "the running max is the whole-run max"
        );
        let expected_avg = sequence.iter().map(|d| d.as_nanos()).sum::<u128>() as f64
            / (1e6 * sequence.len() as f64);
        assert!(
            (stats.avg_ms - expected_avg).abs() < 1e-9,
            "the running-sum avg is the whole-run avg"
        );
    }

    /// R05 (3): the frame count is exact under BOTH modes — the plain
    /// `u64` counter is never sampled (bounded mode here evicts the
    /// vast majority of 1_000 frames yet counts 1_000).
    #[test]
    fn frame_count_exact_under_both_modes() {
        for retention in [
            IntervalRetention::Exact,
            IntervalRetention::Bounded(NonZeroUsize::new(8).unwrap()),
        ] {
            let mut collector = IntervalCollector::new(retention);
            for i in 0..1_000u64 {
                collector.record(Duration::from_nanos(16_000_000 + i % 13));
            }
            assert_eq!(collector.frames(), 1_000);
            assert_eq!(collector.stats().count, 1_000);
            assert!(collector.sample().dropped_total + collector.sample().retained as u64 == 1_000);
        }
    }

    /// The empty run answers the all-zero statistics in both modes (the
    /// divide-by-zero guard on the exact aggregates).
    #[test]
    fn empty_run_answers_zeros_in_both_modes() {
        let zero = legacy_full_vec_stats(&[]);
        for retention in [
            IntervalRetention::Exact,
            IntervalRetention::Bounded(NonZeroUsize::new(4).unwrap()),
        ] {
            let collector = IntervalCollector::new(retention);
            assert_eq!(collector.stats(), zero);
            assert_eq!(collector.frames(), 0);
            assert_eq!(collector.sample().retained, 0);
        }
    }

    /// R05 statistics honesty: under bounded retention the percentiles
    /// are the RETAINED WINDOW's nearest-rank figures — labeled as such
    /// by `IntervalSample` — while count/avg/max stay whole-run exact.
    /// An early 100 ms spike that has aged out of the window is gone
    /// from the window's percentiles but survives in the exact max.
    #[test]
    fn bounded_percentiles_are_the_retained_window_s() {
        let mut collector =
            IntervalCollector::new(IntervalRetention::Bounded(NonZeroUsize::new(4).unwrap()));
        collector.record(Duration::from_millis(100));
        for _ in 0..8 {
            collector.record(Duration::from_millis(16));
        }
        // Capacity 4, 9 frames: three half-drains of 2 → 6 dropped,
        // 3 retained (all steady — the spike evicted with the oldest).
        let sample = collector.sample();
        assert_eq!(sample.dropped_total, 6);
        assert_eq!(sample.retained, 3);
        assert!(!sample.exact);
        let stats = collector.stats();
        let window = legacy_full_vec_stats(&[Duration::from_millis(16); 3]);
        assert_eq!(stats.p50_ms, window.p50_ms);
        assert_eq!(stats.p95_ms, window.p95_ms);
        assert_eq!(stats.p99_ms, window.p99_ms);
        // The whole-run exact aggregates keep the evicted spike.
        assert_eq!(stats.count, 9);
        assert_eq!(
            stats.max_ms,
            Duration::from_millis(100).as_secs_f64() * 1000.0
        );
        assert!((stats.avg_ms - 228.0 / 9.0).abs() < 1e-9);
    }
}
