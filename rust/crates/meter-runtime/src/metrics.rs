//! The real-time gates measured in place: packet → engine and packet → publish latency, frames counted and dropped.
//! Recording is a relaxed atomic add into a fixed log-linear histogram, so it never allocates or locks.

use std::sync::atomic::{AtomicU64, Ordering};

const SUB_BITS: u32 = 5;
const HALF: usize = 1 << (SUB_BITS - 1);
const BUCKETS: usize = 64 * HALF;

/// Microsecond latencies, 32 buckets per power of two (≤ 3 % error).
pub struct Histogram {
    counts: Box<[AtomicU64]>,
    max: AtomicU64,
}

impl Default for Histogram {
    fn default() -> Self {
        Self { counts: (0..BUCKETS).map(|_| AtomicU64::new(0)).collect(), max: AtomicU64::new(0) }
    }
}

fn index(value: u64) -> usize {
    if value < 1 << SUB_BITS {
        return value as usize;
    }
    let shift = 63 - value.leading_zeros() - SUB_BITS + 1;
    ((shift as usize) << (SUB_BITS - 1)) + (value >> shift) as usize
}

/// The highest value a bucket holds.
fn upper(index: usize) -> u64 {
    if index < 1 << SUB_BITS {
        return index as u64;
    }
    let shift = (index >> (SUB_BITS - 1)) - 1;
    let sub = (index - (shift << (SUB_BITS - 1))) as u64;
    ((sub + 1) << shift) - 1
}

impl Histogram {
    pub fn record(&self, microseconds: u64) {
        self.counts[index(microseconds).min(BUCKETS - 1)].fetch_add(1, Ordering::Relaxed);
        self.max.fetch_max(microseconds, Ordering::Relaxed);
    }

    pub fn summary(&self) -> Latency {
        let counts: Vec<u64> = self.counts.iter().map(|count| count.load(Ordering::Relaxed)).collect();
        let total: u64 = counts.iter().sum();
        let max = self.max.load(Ordering::Relaxed);
        let quantile = |q: f64| {
            if total == 0 {
                return 0;
            }
            let rank = ((total as f64 * q).ceil() as u64).max(1);
            let mut seen = 0;
            for (index, count) in counts.iter().enumerate() {
                seen += count;
                if seen >= rank {
                    return upper(index).min(max);
                }
            }
            max
        };
        Latency { count: total, p50_us: quantile(0.5), p99_us: quantile(0.99), p999_us: quantile(0.999), max_us: max }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Latency {
    pub count: u64,
    pub p50_us: u64,
    pub p99_us: u64,
    pub p999_us: u64,
    pub max_us: u64,
}

#[derive(Default)]
pub struct Metrics {
    /// The frame's capture timestamp to the end of `CaptureAnalysis::add` (live sources only: replay timestamps are
    /// the file's).
    pub packet_to_engine: Histogram,
    /// The oldest frame a snapshot includes to the moment it is handed to the UI.
    pub packet_to_publish: Histogram,
    /// How long the engine thread spends in one tick (saves, report, publishing) and in one report rebuild: frames
    /// wait meanwhile.
    pub tick: Histogram,
    pub report: Histogram,
    pub frames: AtomicU64,
    pub publishes: AtomicU64,
    /// Frames the capture thread dropped because the engine fell behind, over every capture of this runtime.
    pub channel_dropped: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MetricsSnapshot {
    pub packet_to_engine: Latency,
    pub packet_to_publish: Latency,
    pub tick: Latency,
    pub report: Latency,
    pub frames: u64,
    pub publishes: u64,
    pub channel_dropped: u64,
}

impl Metrics {
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            packet_to_engine: self.packet_to_engine.summary(),
            packet_to_publish: self.packet_to_publish.summary(),
            tick: self.tick.summary(),
            report: self.report.summary(),
            frames: self.frames.load(Ordering::Relaxed),
            publishes: self.publishes.load(Ordering::Relaxed),
            channel_dropped: self.channel_dropped.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_are_contiguous_and_bounded() {
        for value in 0..100_000u64 {
            let bucket = index(value);
            assert!(value <= upper(bucket), "{value} above bucket {bucket}");
            assert!(bucket == 0 || value > upper(bucket - 1), "{value} below bucket {bucket}");
        }
        let histogram = Histogram::default();
        for value in 1..=1000 {
            histogram.record(value);
        }
        let summary = histogram.summary();
        assert_eq!(summary.count, 1000);
        assert!((490..=520).contains(&summary.p50_us), "{summary:?}");
        assert!((980..=1000).contains(&summary.p99_us), "{summary:?}");
        assert_eq!(summary.max_us, 1000);
    }
}
