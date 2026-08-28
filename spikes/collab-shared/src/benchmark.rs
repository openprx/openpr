//! Candidate-agnostic benchmark sampler, matching `testing/benchmark-spec.md`.
//!
//! 5 warm-up runs, at least 30 measured samples, median/p95/p99/min/max reported, no outliers
//! dropped, and timing that excludes fixture generation (the caller passes a closure that does
//! only the timed work).

pub const MIN_WARMUP: usize = 5;
pub const MIN_SAMPLES: usize = 30;

/// Every measured sample, in the order they were taken, plus the derived distribution stats.
///
/// `raw_samples` is never filtered or truncated — "不删除异常值" — so the caller can also inspect
/// it directly rather than trusting only the summary.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SampleStats {
    pub warmup_count: usize,
    pub sample_count: usize,
    pub median: f64,
    pub p95: f64,
    pub p99: f64,
    pub min: f64,
    pub max: f64,
    pub raw_samples: Vec<f64>,
}

fn stats_from_raw_ms_samples(warmup_count: usize, raw_samples: Vec<f64>) -> SampleStats {
    let sample_count = raw_samples.len();
    let (median, p95, p99, min, max) = distribution_stats(&raw_samples);
    SampleStats {
        warmup_count,
        sample_count,
        median,
        p95,
        p99,
        min,
        max,
        raw_samples,
    }
}

/// Runs `warmup` untimed iterations of `work`, then `samples` timed iterations, and returns the
/// resulting distribution.
///
/// `warmup` and `samples` are clamped up to the spec minimums ([`MIN_WARMUP`], [`MIN_SAMPLES`])
/// rather than silently producing an under-powered result.
pub fn sample_duration<F: FnMut()>(warmup: usize, samples: usize, mut work: F) -> SampleStats {
    let warmup_count = warmup.max(MIN_WARMUP);
    let sample_count = samples.max(MIN_SAMPLES);

    for _ in 0..warmup_count {
        work();
    }

    let mut raw_samples = Vec::with_capacity(sample_count);
    for _ in 0..sample_count {
        let start = std::time::Instant::now();
        work();
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        raw_samples.push(elapsed_ms);
    }

    stats_from_raw_ms_samples(warmup_count, raw_samples)
}

/// Runs two workloads under identical warm-up/sample counts, **interleaved per iteration**.
///
/// (`work_a` then `work_b`, repeated) rather than one candidate run to completion before the
/// other starts — `testing/benchmark-spec.md`'s "两候选 ... 顺序交替" requirement. This is what
/// keeps a same-run thermal/scheduler drift affecting both candidates symmetrically instead of
/// biasing whichever one happened to go first.
///
/// Unlike [`sample_duration`], each closure reports its own elapsed milliseconds (`-> f64`)
/// rather than being wrapped by an `Instant` the sampler owns. This lets a caller exclude
/// per-sample setup (e.g. reloading a fresh sink replica before timing just the `import_update`
/// call on it) from the measured duration while still paying that setup cost once per sample
/// rather than sharing one mutated instance across samples — "计时不包含 fixture 生成" without
/// forcing every metric's setup to live entirely outside the sample loop.
pub fn sample_alternating<FA, FB>(
    warmup: usize,
    samples: usize,
    mut work_a: FA,
    mut work_b: FB,
) -> (SampleStats, SampleStats)
where
    FA: FnMut() -> f64,
    FB: FnMut() -> f64,
{
    let warmup_count = warmup.max(MIN_WARMUP);
    let sample_count = samples.max(MIN_SAMPLES);

    for _ in 0..warmup_count {
        let _ = work_a();
        let _ = work_b();
    }

    let mut raw_a = Vec::with_capacity(sample_count);
    let mut raw_b = Vec::with_capacity(sample_count);
    for _ in 0..sample_count {
        raw_a.push(work_a());
        raw_b.push(work_b());
    }

    (
        stats_from_raw_ms_samples(warmup_count, raw_a),
        stats_from_raw_ms_samples(warmup_count, raw_b),
    )
}

/// (median, p95, p99, min, max) over `values`, without mutating or truncating the input slice.
#[must_use]
pub fn distribution_stats(values: &[f64]) -> (f64, f64, f64, f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0, 0.0, 0.0);
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    // Sample counts are bounded by how many benchmark iterations actually ran (never anywhere
    // near 2^52), so the usize <-> f64 round trip below is always exact; `.get()` still guards
    // the final index against a hypothetical off-by-one in the rounding rather than trusting the
    // arithmetic to stay in bounds.
    let percentile = |p: f64| -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let len_minus_one = sorted.len() as f64 - 1.0;
        let rank = (p * len_minus_one).round();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let index = rank.clamp(0.0, len_minus_one) as usize;
        sorted
            .get(index)
            .copied()
            .unwrap_or_else(|| sorted.last().copied().unwrap_or(0.0))
    };

    let median = percentile(0.50);
    let p95 = percentile(0.95);
    let p99 = percentile(0.99);
    let min = sorted.first().copied().unwrap_or(0.0);
    let max = sorted.last().copied().unwrap_or(0.0);
    (median, p95, p99, min, max)
}

#[cfg(test)]
mod tests {
    use super::{MIN_SAMPLES, MIN_WARMUP, distribution_stats, sample_alternating, sample_duration};

    #[test]
    fn enforces_spec_minimums_even_when_asked_for_fewer() {
        let stats = sample_duration(0, 0, || {});
        assert_eq!(stats.warmup_count, MIN_WARMUP);
        assert_eq!(stats.sample_count, MIN_SAMPLES);
        assert_eq!(stats.raw_samples.len(), MIN_SAMPLES);
    }

    #[test]
    fn keeps_every_raw_sample_including_outliers() {
        let mut call = 0usize;
        let stats = sample_duration(MIN_WARMUP, MIN_SAMPLES, || {
            call += 1;
        });
        assert_eq!(stats.raw_samples.len(), MIN_SAMPLES);
        // warmup + samples both invoked the closure.
        assert_eq!(call, MIN_WARMUP + MIN_SAMPLES);
    }

    #[test]
    fn distribution_stats_matches_hand_computed_percentiles() {
        let values: Vec<f64> = (1..=100).map(f64::from).collect();
        let (median, p95, p99, min, max) = distribution_stats(&values);
        // `min`/`max` are picked (not computed via arithmetic) from a fixed 1..=100 integer-
        // derived input, so they are exactly 1.0/100.0 -- exact equality is the correct check.
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(min, 1.0);
            assert_eq!(max, 100.0);
        }
        assert!((median - 50.5).abs() < 1.0);
        assert!((94.0..=96.0).contains(&p95));
        assert!((98.0..=100.0).contains(&p99));
    }

    #[test]
    fn sample_alternating_interleaves_per_iteration_not_run_to_completion() {
        use std::cell::RefCell;
        use std::rc::Rc;

        // `sample_alternating` calls `work_a`/`work_b` serially in the same thread (see its own
        // doc comment / implementation above): no actual concurrency is ever in play here, so a
        // `Rc<RefCell<_>>` shared log is sufficient and correct -- an `Arc<Mutex<_>>` would only
        // add unneeded synchronization overhead for a single-threaded call pattern.
        let order: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
        let order_a = Rc::clone(&order);
        let order_b = Rc::clone(&order);

        let (stats_a, stats_b) = sample_alternating(
            MIN_WARMUP,
            MIN_SAMPLES,
            move || {
                order_a.borrow_mut().push("a");
                1.0
            },
            move || {
                order_b.borrow_mut().push("b");
                2.0
            },
        );

        assert_eq!(stats_a.sample_count, MIN_SAMPLES);
        assert_eq!(stats_b.sample_count, MIN_SAMPLES);
        // Every reported sample was hardcoded to exactly `1.0`/`2.0` by the closures above, never
        // computed via floating-point arithmetic, so exact equality is the correct check here.
        #[allow(clippy::float_cmp)]
        let all_ones = stats_a.raw_samples.iter().all(|value| *value == 1.0);
        #[allow(clippy::float_cmp)]
        let all_twos = stats_b.raw_samples.iter().all(|value| *value == 2.0);
        assert!(all_ones);
        assert!(all_twos);

        let recorded = order.borrow();
        // Every consecutive pair must be exactly ["a", "b"] -- proof this never ran candidate A
        // to completion before starting candidate B.
        for pair in recorded.chunks(2) {
            assert_eq!(pair, ["a", "b"]);
        }
    }

    #[test]
    fn sample_alternating_excludes_only_what_the_closure_excludes() {
        // Each closure reports its own elapsed time rather than being wall-clock-wrapped by the
        // sampler, so untimed setup work inside the closure (simulated here by a busy spin before
        // the closure returns a fixed, unrelated duration) must not leak into `raw_samples`.
        let (stats_a, _stats_b) = sample_alternating(
            MIN_WARMUP,
            MIN_SAMPLES,
            || {
                let mut counter = 0u64;
                for _ in 0..10_000 {
                    counter = counter.wrapping_add(1);
                }
                std::hint::black_box(counter);
                7.5
            },
            || 0.0,
        );
        // Every reported sample was hardcoded to exactly `7.5` by the closure above, never
        // computed via floating-point arithmetic, so exact equality is the correct check.
        #[allow(clippy::float_cmp)]
        let all_expected = stats_a.raw_samples.iter().all(|value| *value == 7.5);
        assert!(all_expected);
    }
}
