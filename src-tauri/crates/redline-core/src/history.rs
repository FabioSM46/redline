//! Metrics history: a fixed-size ring buffer of timestamped `GpuSnapshot`s.
//! Pure data structure: no clock, no sysfs. The caller supplies the timestamps.

use crate::sensors::GpuSnapshot;
use serde::Serialize;
use std::collections::VecDeque;

/// 15 minutes at 1 Hz.
pub const CAPACITY: usize = 900;

/// One reading of one GPU. Serialized as
/// `{ "timestamp_ms": <u64>, "snapshot": { <GpuSnapshot fields> } }`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Sample {
    /// Wall-clock milliseconds since the Unix epoch, when the reading was taken.
    pub timestamp_ms: u64,
    /// Missing sensors stay `None` (`null`), never zero.
    pub snapshot: GpuSnapshot,
}

/// The last `CAPACITY` samples of one GPU, oldest first.
#[derive(Debug, Clone, Default)]
pub struct History {
    samples: VecDeque<Sample>,
}

impl History {
    pub fn new() -> Self {
        Self {
            samples: VecDeque::with_capacity(CAPACITY),
        }
    }

    /// Appends a sample; once the buffer holds `CAPACITY` samples the oldest one is dropped.
    pub fn push(&mut self, timestamp_ms: u64, snapshot: GpuSnapshot) {
        if self.samples.len() == CAPACITY {
            self.samples.pop_front();
        }
        self.samples.push_back(Sample {
            timestamp_ms,
            snapshot,
        });
    }

    /// The most recent sample, if any.
    pub fn latest(&self) -> Option<&Sample> {
        self.samples.back()
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The samples of the last `seconds` seconds, in chronological order. The window ends at
    /// the newest sample and is open at the start: a sample exactly `seconds` older than the
    /// newest one is excluded, so at 1 Hz a 60 s window holds at most 60 samples.
    pub fn window(&self, seconds: u64) -> Vec<Sample> {
        let Some(newest) = self.latest() else {
            return Vec::new();
        };
        let cutoff = newest
            .timestamp_ms
            .saturating_sub(seconds.saturating_mul(1000));
        // Walk back from the newest sample: a wall clock that jumped backwards must not
        // pull older samples into the window.
        let count = self
            .samples
            .iter()
            .rev()
            .take_while(|s| s.timestamp_ms > cutoff && s.timestamp_ms <= newest.timestamp_ms)
            .count();
        self.samples
            .iter()
            .skip(self.samples.len() - count)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(fan: u64) -> GpuSnapshot {
        GpuSnapshot {
            fan_rpm: Some(fan),
            ..Default::default()
        }
    }

    /// `n` samples at 1 Hz starting at `start_ms`; the fan value is the sample index.
    fn filled(start_ms: u64, n: usize) -> History {
        let mut h = History::new();
        for i in 0..n {
            h.push(start_ms + i as u64 * 1000, snap(i as u64));
        }
        h
    }

    fn timestamps(samples: &[Sample]) -> Vec<u64> {
        samples.iter().map(|s| s.timestamp_ms).collect()
    }

    #[test]
    fn empty_buffer_has_no_latest_and_empty_windows() {
        let h = History::new();
        assert!(h.is_empty());
        assert!(h.latest().is_none());
        assert!(h.window(60).is_empty());
        assert!(h.window(0).is_empty());
    }

    #[test]
    fn wraps_around_at_capacity_dropping_the_oldest() {
        let h = filled(1_000_000, CAPACITY + 5);
        assert_eq!(h.len(), CAPACITY);
        let all = h.window(10_000);
        assert_eq!(all.len(), CAPACITY);
        assert_eq!(all[0].snapshot.fan_rpm, Some(5), "the five oldest are gone");
        assert_eq!(
            h.latest().unwrap().snapshot.fan_rpm,
            Some(CAPACITY as u64 + 4)
        );
    }

    #[test]
    fn window_is_chronological_and_open_at_the_start() {
        let h = filled(1_000_000, 120);
        let w = h.window(60);
        assert_eq!(w.len(), 60);
        let ts = timestamps(&w);
        assert!(ts.windows(2).all(|p| p[0] < p[1]), "oldest first");
        assert_eq!(*ts.last().unwrap(), 1_000_000 + 119 * 1000);
        assert_eq!(ts[0], 1_000_000 + 60 * 1000);
    }

    #[test]
    fn window_longer_than_the_buffer_returns_everything() {
        let h = filled(1_000_000, 30);
        assert_eq!(h.window(900).len(), 30);
        assert_eq!(filled(1_000_000, CAPACITY).window(900).len(), CAPACITY);
    }

    #[test]
    fn window_of_zero_seconds_is_empty() {
        assert!(filled(1_000_000, 10).window(0).is_empty());
    }

    #[test]
    fn window_ignores_gaps_in_sampling() {
        let mut h = History::new();
        h.push(1_000, snap(0));
        h.push(2_000, snap(1));
        // sampling paused for 100 seconds
        h.push(102_000, snap(2));
        h.push(103_000, snap(3));
        assert_eq!(timestamps(&h.window(60)), vec![102_000, 103_000]);
        assert_eq!(timestamps(&h.window(300)).len(), 4);
    }

    #[test]
    fn window_stops_at_a_backwards_clock_jump() {
        let mut h = History::new();
        h.push(50_000, snap(0));
        h.push(10_000, snap(1)); // wall clock set back
        h.push(11_000, snap(2));
        assert_eq!(timestamps(&h.window(60)), vec![10_000, 11_000]);
    }

    #[test]
    fn missing_sensors_stay_null_when_serialized() {
        let mut h = History::new();
        h.push(1_234, snap(900));
        let json = serde_json::to_value(h.window(60)).unwrap();
        assert_eq!(json[0]["timestamp_ms"], 1_234);
        assert_eq!(json[0]["snapshot"]["fan_rpm"], 900);
        assert!(json[0]["snapshot"]["temp_edge_c"].is_null());
        assert!(json[0]["snapshot"]["power_w"].is_null());
    }
}
