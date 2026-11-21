//! Bounded whole-run timing statistics for observational measurements.

use std::time::Duration;

// Keep six bits of relative precision and represent the shortest intervals
// exactly. The upper bound for any bucket is within 1/64 of its lower bound.
const PRECISION_BITS: u32 = 6;
const SUB_BUCKETS: usize = 1 << PRECISION_BITS;
const BUCKET_COUNT: usize = (65 - PRECISION_BITS as usize) * SUB_BUCKETS;

#[derive(Debug, Clone)]
pub struct TimingSamples {
    counts: Box<[usize]>,
    count: usize,
    max_ns: u64,
}

impl Default for TimingSamples {
    fn default() -> Self {
        Self {
            counts: vec![0; BUCKET_COUNT].into_boxed_slice(),
            count: 0,
            max_ns: 0,
        }
    }
}

impl TimingSamples {
    pub fn record(&mut self, duration: Duration) {
        let ns = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        let shift = 63_u32
            .saturating_sub(ns.leading_zeros())
            .saturating_sub(PRECISION_BITS);
        let index = shift as usize * SUB_BUCKETS + (ns >> shift) as usize;
        self.counts[index] += 1;
        self.count += 1;
        self.max_ns = self.max_ns.max(ns);
    }

    pub fn sample_count(&self) -> usize {
        self.count
    }

    pub fn max_ms(&self) -> f64 {
        self.max_ns as f64 / 1_000_000.0
    }

    /// Nearest-rank percentile, returned as the bucket's upper bound in
    /// milliseconds and limited to the largest actual sample.
    pub fn percentile_ms(&self, quantile: f64) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let mut rank = ((quantile * self.count as f64).ceil() as usize).clamp(1, self.count);
        for (index, &count) in self.counts.iter().enumerate() {
            if rank <= count {
                let shift = (index / SUB_BUCKETS).saturating_sub(1);
                let value = index - shift * SUB_BUCKETS;
                let upper = ((value as u64) << shift) | ((1_u64 << shift) - 1);
                return upper.min(self.max_ns) as f64 / 1_000_000.0;
            }
            rank -= count;
        }
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_run_keeps_early_stalls_with_bounded_percentile_error() {
        let mut samples = TimingSamples::default();
        assert_eq!(samples.percentile_ms(0.95), 0.0);
        for _ in 0..30 {
            samples.record(Duration::from_millis(1750));
        }
        for _ in 0..2000 {
            samples.record(Duration::from_millis(4));
        }
        assert_eq!(samples.sample_count(), 2030);
        assert!((4.0..=4.0 * (1.0 + 1.0 / 64.0)).contains(&samples.percentile_ms(0.95)));
        assert_eq!(samples.percentile_ms(0.99), 1750.0);
        assert_eq!(samples.max_ms(), 1750.0);

        let mut samples = TimingSamples::default();
        samples.record(Duration::ZERO);
        samples.record(Duration::from_nanos(u64::MAX - 10));
        samples.record(Duration::MAX);
        assert_eq!(samples.percentile_ms(0.1), 0.0);
        assert_eq!(samples.percentile_ms(1.0), samples.max_ms());
    }
}
