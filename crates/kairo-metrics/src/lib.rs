//! Measurement primitives for Kairo.
//!
//! Everything here is deliberately platform-agnostic and dependency-free so the numbers
//! that decide whether Kairo ships are themselves testable on any machine. Windows-specific
//! emission (ETW) lives in the app layer and feeds these same structures.
//!
//! Two instruments matter in Phase 0:
//!
//! * [`StartupTimeline`] — the cold-start story. One marker per named milestone from
//!   process entry to first presented frame.
//! * [`FrameStats`] — the frame-pacing story. Rolling window with p50/p99 and a dropped-frame
//!   count against an expected refresh period.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

/// Ordered cold-start milestones, measured from construction.
///
/// Construct this as the first statement of `main` (or an `.init_array` constructor, once we
/// care about the first ~200 µs) so that `ProcessStart` is as close to process entry as the
/// runtime allows.
pub struct StartupTimeline {
    origin: Instant,
    marks: Vec<(&'static str, Duration)>,
}

impl StartupTimeline {
    /// Begin recording. Place at the top of the entry point.
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
            marks: Vec::with_capacity(16),
        }
    }

    /// Record a milestone at the current instant.
    pub fn mark(&mut self, name: &'static str) {
        let at = self.origin.elapsed();
        self.marks.push((name, at));
    }

    /// Time since the timeline began.
    pub fn elapsed(&self) -> Duration {
        self.origin.elapsed()
    }

    /// Instant recorded for `name`, if it was marked.
    pub fn at(&self, name: &str) -> Option<Duration> {
        self.marks
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, at)| *at)
    }

    /// Aligned table of milestones with the delta from the previous mark.
    ///
    /// The delta column is the useful one: it localises a cold-start regression to a phase.
    pub fn report(&self) -> String {
        let mut out = String::from("startup timeline\n");
        let mut prev = Duration::ZERO;
        for (name, at) in &self.marks {
            let _ = writeln!(
                out,
                "  {:>8.3} ms  (+{:>7.3} ms)  {}",
                at.as_secs_f64() * 1e3,
                at.saturating_sub(prev).as_secs_f64() * 1e3,
                name
            );
            prev = *at;
        }
        out
    }
}

impl Default for StartupTimeline {
    fn default() -> Self {
        Self::new()
    }
}

/// Rolling frame-pacing statistics against an expected refresh period.
///
/// A frame is "dropped" when it exceeds [`FrameStats::DROP_THRESHOLD`] times the expected
/// period. The threshold is deliberately above 1.0 because a vsync-locked renderer
/// legitimately produces frames slightly longer than the nominal period.
#[derive(Debug, Clone)]
pub struct FrameStats {
    period: Duration,
    capacity: usize,
    samples: VecDeque<Duration>,
    dropped: u64,
    total: u64,
}

impl FrameStats {
    /// Frame is dropped at 1.5× the expected period: catches a missed vsync, not jitter.
    pub const DROP_THRESHOLD: f64 = 1.5;

    /// `capacity` is the rolling window size; 600 ≈ 10 s at 60 Hz.
    pub fn new(period: Duration, capacity: usize) -> Self {
        Self {
            period,
            capacity: capacity.max(1),
            samples: VecDeque::with_capacity(capacity.max(1)),
            dropped: 0,
            total: 0,
        }
    }

    /// Default window sized for `hz` refresh rate.
    pub fn for_refresh_hz(hz: u32) -> Self {
        let hz = hz.max(1);
        let period = Duration::from_nanos(1_000_000_000 / u64::from(hz));
        Self::new(period, hz as usize * 10)
    }

    /// Record one frame interval.
    pub fn push(&mut self, frame: Duration) {
        self.total += 1;
        if frame.as_secs_f64() > self.period.as_secs_f64() * Self::DROP_THRESHOLD {
            self.dropped += 1;
        }
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(frame);
    }

    /// Number of samples currently in the rolling window.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether no frames have been recorded yet.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Total frames recorded since construction.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Total dropped frames since construction.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Sorted samples, ascending.
    fn sorted(&self) -> Vec<Duration> {
        let mut v: Vec<Duration> = self.samples.iter().copied().collect();
        v.sort_unstable();
        v
    }

    /// Nearest-rank percentile, `p` in `0.0..=1.0`. `None` when the window is empty.
    pub fn percentile(&self, p: f64) -> Option<Duration> {
        if self.samples.is_empty() {
            return None;
        }
        let p = p.clamp(0.0, 1.0);
        let sorted = self.sorted();
        let rank = (p * sorted.len() as f64).ceil() as usize;
        let idx = rank.saturating_sub(1).min(sorted.len() - 1);
        Some(sorted[idx])
    }

    /// Median frame interval.
    pub fn p50(&self) -> Option<Duration> {
        self.percentile(0.50)
    }

    /// 99th percentile frame interval — the number that correlates with visible stutter.
    pub fn p99(&self) -> Option<Duration> {
        self.percentile(0.99)
    }

    /// Worst frame in the window.
    pub fn max(&self) -> Option<Duration> {
        self.samples.iter().copied().max()
    }

    /// Arithmetic mean frame interval.
    pub fn mean(&self) -> Option<Duration> {
        if self.samples.is_empty() {
            return None;
        }
        let sum: Duration = self.samples.iter().sum();
        Some(sum / self.samples.len() as u32)
    }

    /// Drop ratio over the whole run, in `0.0..=1.0`.
    pub fn drop_ratio(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.dropped as f64 / self.total as f64
    }

    /// Human-readable summary line for the console harness.
    pub fn report(&self) -> String {
        let ms = |d: Option<Duration>| match d {
            Some(d) => format!("{:.2}", d.as_secs_f64() * 1e3),
            None => "-".into(),
        };
        format!(
            "frames {} | p50 {} ms | p99 {} ms | max {} ms | dropped {} ({:.2}%)",
            self.total,
            ms(self.p50()),
            ms(self.p99()),
            ms(self.max()),
            self.dropped,
            self.drop_ratio() * 100.0
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    #[test]
    fn timeline_records_marks_in_order() {
        let mut t = StartupTimeline::new();
        t.mark("window");
        t.mark("device");
        assert_eq!(t.at("window"), Some(t.at("window").unwrap()));
        assert!(t.at("window").unwrap() <= t.at("device").unwrap());
        assert_eq!(t.at("missing"), None);
        assert!(t.report().contains("window"));
    }

    #[test]
    fn empty_stats_report_as_absent() {
        let s = FrameStats::new(ms(16), 8);
        assert!(s.is_empty());
        assert_eq!(s.p50(), None);
        assert_eq!(s.p99(), None);
        assert_eq!(s.max(), None);
        assert_eq!(s.mean(), None);
        assert_eq!(s.drop_ratio(), 0.0);
    }

    #[test]
    fn percentile_uses_nearest_rank() {
        let mut s = FrameStats::new(ms(16), 16);
        for v in [1, 2, 3, 4] {
            s.push(ms(v));
        }
        assert_eq!(s.p50(), Some(ms(2)));
        assert_eq!(s.percentile(1.0), Some(ms(4)));
        assert_eq!(s.percentile(0.0), Some(ms(1)));
    }

    #[test]
    fn window_is_bounded_and_tracks_totals() {
        let mut s = FrameStats::new(ms(16), 3);
        for _ in 0..10 {
            s.push(ms(16));
        }
        assert_eq!(s.len(), 3);
        assert_eq!(s.total(), 10);
        assert_eq!(s.dropped(), 0);
    }

    #[test]
    fn slow_frames_count_as_dropped() {
        let mut s = FrameStats::new(ms(16), 64);
        s.push(ms(16));
        s.push(ms(17)); // jitter, not a drop
        s.push(ms(33)); // missed vsync
        assert_eq!(s.total(), 3);
        assert_eq!(s.dropped(), 1);
        assert!(s.drop_ratio() > 0.33 && s.drop_ratio() < 0.34);
    }

    #[test]
    fn mean_is_arithmetic() {
        let mut s = FrameStats::new(ms(16), 8);
        s.push(ms(10));
        s.push(ms(20));
        assert_eq!(s.mean(), Some(ms(15)));
    }

    #[test]
    fn refresh_sized_window() {
        let s = FrameStats::for_refresh_hz(144);
        assert_eq!(s.capacity, 1440);
        assert_eq!(s.period, Duration::from_nanos(1_000_000_000 / 144));
    }
}
