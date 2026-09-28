//! Nearest-rank percentile math for the platform harness (lap-c0e /
//! TASK-601). The exact definition must match
//! `scripts/raw-development/benchmark.mjs` `percentile()` so the validator
//! can recompute p95 from the recorded raw samples and reject any invented
//! number.

/// Nearest-rank percentile: rank = ceil(q * n), clamped into 1..=n; the
/// sample at that rank (ascending) is the result. `q` must be in (0, 1].
pub fn nearest_rank_percentile(samples: &[f64], q: f64) -> f64 {
    assert!(!samples.is_empty(), "percentile of an empty sample set");
    assert!((0.0..=1.0).contains(&q) && q > 0.0, "q must be in (0, 1]");
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("finite samples"));
    let rank = (q * sorted.len() as f64).ceil() as usize;
    let index = rank.clamp(1, sorted.len()) - 1;
    sorted[index]
}

pub fn summarize(samples: &[f64]) -> (f64, f64, f64, f64) {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("finite samples"));
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    (
        sorted[0],
        mean,
        nearest_rank_percentile(samples, 0.50),
        nearest_rank_percentile(samples, 0.95),
    )
}

#[cfg(test)]
mod tests {
    use super::nearest_rank_percentile;

    #[test]
    fn p95_matches_nearest_rank_definition() {
        let samples = [40.0, 41.0, 42.0];
        // rank = ceil(0.95 * 3) = 3 -> third sample ascending
        assert_eq!(nearest_rank_percentile(&samples, 0.95), 42.0);
    }

    #[test]
    fn p95_of_two_samples_is_the_higher_one() {
        let samples = [100.0, 120.0];
        // rank = ceil(0.95 * 2) = 2 -> second sample
        assert_eq!(nearest_rank_percentile(&samples, 0.95), 120.0);
    }

    #[test]
    fn p95_of_twenty_samples_is_the_nineteenth() {
        let samples: Vec<f64> = (1..=20).map(|v| v as f64).collect();
        // rank = ceil(0.95 * 20) = 19
        assert_eq!(nearest_rank_percentile(&samples, 0.95), 19.0);
        assert_eq!(nearest_rank_percentile(&samples, 0.50), 10.0);
    }

    #[test]
    fn p95_ignores_insertion_order() {
        let a = [5.0, 1.0, 9.0, 3.0, 7.0, 2.0, 8.0, 6.0, 4.0, 10.0];
        let b = [10.0, 2.0, 8.0, 4.0, 6.0, 1.0, 9.0, 3.0, 7.0, 5.0];
        assert_eq!(
            nearest_rank_percentile(&a, 0.95),
            nearest_rank_percentile(&b, 0.95)
        );
    }
}
