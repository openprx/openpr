//! The three empirical measurements work package 3a exists to answer (ADR-0014 "未决" section).
//!
//! No `unsafe` code lives directly in this module — it drives
//! [`crate::isolation::zygote::run_one_case`] in a loop and computes statistics over the results.

use crate::isolation::child::{FaultMode, FixtureKind};
use crate::isolation::termination::{DECODE_APPLY_WALL_MS_MAX, DECODE_APPLY_WALL_MS_MAX_U64};
use crate::isolation::zygote::{self, CaseRunOutcome};
use crate::result::IsolationOracle;

/// Nominal wall-watchdog budget the [`FixtureKind::WallHog`] fixture is run against for
/// calibration purposes (matches [`DECODE_APPLY_WALL_MS_MAX`], kept as its own named constant
/// here so the calibration's "deviation from 100ms" framing reads directly rather than through an
/// `as u64` cast at every call site).
const NOMINAL_WALL_BUDGET_MS: f64 = DECODE_APPLY_WALL_MS_MAX;

/// Minimum sample count the task requires for every calibration measurement ("≥30 次采样").
pub const MIN_SAMPLES: usize = 30;

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Distribution {
    pub sample_count: usize,
    pub median: f64,
    pub p95: f64,
    pub max: f64,
    pub min: f64,
}

impl Distribution {
    #[must_use]
    pub fn from_samples(mut samples: Vec<f64>) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let sample_count = samples.len();
        let median = percentile(&samples, 0.50);
        let p95 = percentile(&samples, 0.95);
        // `samples` was just confirmed non-empty above, but `.last()`/`.first()` avoid indexing
        // (`indexing_slicing` is a deny-level workspace lint) rather than relying on that
        // invariant holding at the panic-risking index expression itself.
        let (Some(max), Some(min)) = (samples.last().copied(), samples.first().copied()) else {
            return None;
        };
        Some(Self {
            sample_count,
            median,
            p95,
            max,
            min,
        })
    }
}

/// Nearest-rank percentile over an already-sorted slice.
fn percentile(sorted_samples: &[f64], fraction: f64) -> f64 {
    if sorted_samples.is_empty() {
        return 0.0;
    }
    // `sorted_samples.len()` is a calibration sample count, always small enough (well under
    // 2^52) for this usize -> f64 round trip to be exact.
    #[allow(clippy::cast_precision_loss)]
    let len_minus_one = sorted_samples.len() as f64 - 1.0;
    let rank = (len_minus_one * fraction).round();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let index = rank.clamp(0.0, len_minus_one) as usize;
    sorted_samples.get(index).copied().unwrap_or(0.0)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ItimerProfCalibration {
    /// `cpu_ms` observed at the moment `SIGPROF` actually terminated the process, per sample.
    /// Compare against the nominal 50ms `setitimer` value: `deviation = cpu_ms - 50.0`.
    pub raw_cpu_ms_samples: Vec<f64>,
    pub deviation_from_50ms: Distribution,
    /// How many of the requested samples did *not* resolve to a clean `cpu_ceiling` (e.g. the
    /// zygote refused to fork, or the case crashed for an unrelated reason) — reported rather
    /// than silently excluded.
    pub anomalous_sample_count: usize,
    pub anomalies: Vec<String>,
}

/// Work package 3a question 1: `ITIMER_PROF`'s actual delivery resolution/lateness relative to
/// the nominal 50ms value.
///
/// Measured by running a pure CPU busy-loop and observing the CPU consumed at the moment
/// `SIGPROF` actually terminates the process.
#[must_use]
pub fn measure_itimer_prof_resolution(sample_count: usize) -> ItimerProfCalibration {
    let mut raw_cpu_ms_samples = Vec::with_capacity(sample_count);
    let mut anomalies = Vec::new();

    for index in 0..sample_count {
        let generation = u64::try_from(index).unwrap_or(u64::MAX).wrapping_add(1);
        match zygote::run_one_case(
            generation,
            FixtureKind::CpuHog,
            FaultMode::None,
            DECODE_APPLY_WALL_MS_MAX_U64,
        ) {
            CaseRunOutcome::Ran(observation) => {
                if observation.primary_cause == IsolationOracle::CpuCeiling {
                    raw_cpu_ms_samples.push(observation.cpu_ms);
                } else {
                    anomalies.push(format!(
                        "sample {index}: expected cpu_ceiling, observed {:?} (raw_wait_status={})",
                        observation.primary_cause, observation.raw_wait_status
                    ));
                }
            }
            CaseRunOutcome::Failure(reason) => {
                anomalies.push(format!("sample {index}: case failed to run: {reason:?}"));
            }
        }
    }

    let deviation_samples: Vec<f64> = raw_cpu_ms_samples.iter().map(|cpu_ms| cpu_ms - 50.0).collect();
    let deviation_from_50ms = Distribution::from_samples(deviation_samples).unwrap_or(Distribution {
        sample_count: 0,
        median: 0.0,
        p95: 0.0,
        max: 0.0,
        min: 0.0,
    });

    ItimerProfCalibration {
        raw_cpu_ms_samples,
        deviation_from_50ms,
        anomalous_sample_count: anomalies.len(),
        anomalies,
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RssAllocatorRatioCalibration {
    pub ratio_samples: Vec<f64>,
    pub ratio_distribution: Distribution,
    /// ADR-0014 section 3's frozen rule: "max × 1.5".
    pub proposed_bound: f64,
    pub anomalous_sample_count: usize,
    pub anomalies: Vec<String>,
}

/// Work package 3a question 2 (`rss_allocator_ratio_bound`): `rss_delta / allocated_peak` over
/// repeated baseline applies.
///
/// `rss_delta = rss_peak_bytes - rss_baseline_bytes` (both sampled inside the same child, before
/// and after its allocation).
#[must_use]
pub fn measure_rss_allocator_ratio(sample_count: usize) -> RssAllocatorRatioCalibration {
    let mut ratio_samples = Vec::with_capacity(sample_count);
    let mut anomalies = Vec::new();

    for index in 0..sample_count {
        let generation = u64::try_from(index).unwrap_or(u64::MAX).wrapping_add(1);
        match zygote::run_one_case(
            generation,
            FixtureKind::AllocationBaseline,
            FaultMode::None,
            DECODE_APPLY_WALL_MS_MAX_U64,
        ) {
            CaseRunOutcome::Ran(observation) => {
                let Some(report) = &observation.child_report else {
                    anomalies.push(format!(
                        "sample {index}: no child report available ({:?})",
                        observation.report_decode_error
                    ));
                    continue;
                };
                if observation.allocated_active_peak_bytes == 0 {
                    anomalies.push(format!("sample {index}: allocated_active_peak_bytes was 0"));
                    continue;
                }
                let rss_delta = observation.rss_peak_bytes.saturating_sub(report.rss_baseline_bytes);
                // Both operands are real process byte counts, always far below f64's 2^52 exact
                // range (petabytes of RSS), so this ratio is computed without precision loss.
                #[allow(clippy::cast_precision_loss)]
                let ratio = rss_delta as f64 / observation.allocated_active_peak_bytes as f64;
                ratio_samples.push(ratio);
            }
            CaseRunOutcome::Failure(reason) => {
                anomalies.push(format!("sample {index}: case failed to run: {reason:?}"));
            }
        }
    }

    let ratio_distribution = Distribution::from_samples(ratio_samples.clone()).unwrap_or(Distribution {
        sample_count: 0,
        median: 0.0,
        p95: 0.0,
        max: 0.0,
        min: 0.0,
    });
    let proposed_bound = ratio_distribution.max * 1.5;

    RssAllocatorRatioCalibration {
        ratio_samples,
        ratio_distribution,
        proposed_bound,
        anomalous_sample_count: anomalies.len(),
        anomalies,
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ForkOverheadCalibration {
    pub fork_overhead_ms_samples: Vec<f64>,
    pub distribution: Distribution,
    pub anomalous_sample_count: usize,
    pub anomalies: Vec<String>,
}

/// Work package 3a question 3: `fork_overhead_ms` distribution.
///
/// Measured via the fast/trivial [`FixtureKind::Completed`] fixture so the overhead being
/// measured is dominated by `fork()` itself rather than any fixture-specific work.
#[must_use]
pub fn measure_fork_overhead(sample_count: usize) -> ForkOverheadCalibration {
    let mut fork_overhead_ms_samples = Vec::with_capacity(sample_count);
    let mut anomalies = Vec::new();

    for index in 0..sample_count {
        let generation = u64::try_from(index).unwrap_or(u64::MAX).wrapping_add(1);
        match zygote::run_one_case(
            generation,
            FixtureKind::Completed,
            FaultMode::None,
            DECODE_APPLY_WALL_MS_MAX_U64,
        ) {
            CaseRunOutcome::Ran(observation) => {
                if observation.primary_cause == IsolationOracle::Completed {
                    fork_overhead_ms_samples.push(observation.fork_overhead_ms);
                } else {
                    anomalies.push(format!(
                        "sample {index}: expected completed, observed {:?}",
                        observation.primary_cause
                    ));
                }
            }
            CaseRunOutcome::Failure(reason) => {
                anomalies.push(format!("sample {index}: case failed to run: {reason:?}"));
            }
        }
    }

    let distribution = Distribution::from_samples(fork_overhead_ms_samples.clone()).unwrap_or(Distribution {
        sample_count: 0,
        median: 0.0,
        p95: 0.0,
        max: 0.0,
        min: 0.0,
    });

    ForkOverheadCalibration {
        fork_overhead_ms_samples,
        distribution,
        anomalous_sample_count: anomalies.len(),
        anomalies,
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WallWatchdogCalibration {
    /// `wall_ms` observed at the moment the parent's wall watchdog actually reaped the killed
    /// child, per sample. Compare against the nominal 100ms wall budget:
    /// `deviation = wall_ms - 100.0`.
    pub raw_wall_ms_samples: Vec<f64>,
    pub deviation_from_100ms: Distribution,
    /// How many of the requested samples did *not* resolve to a clean `wall_ceiling` (e.g. the
    /// zygote refused to fork, the case crashed for an unrelated reason, or -- see this
    /// function's doc comment -- the fixture finished so far past the budget that some other
    /// cause classified first) -- reported rather than silently excluded.
    pub anomalous_sample_count: usize,
    pub anomalies: Vec<String>,
}

/// Work package 3a question 4 (the wall-watchdog analogue of
/// [`measure_itimer_prof_resolution`]): the wall watchdog's actual lateness relative to the
/// nominal 100ms `wall_budget_ms`.
///
/// Measured by running a fixture that blocks on wall-clock time alone (no CPU:
/// [`FixtureKind::WallHog`] sleeps) and observing `wall_ms` -- the parent-measured duration from
/// fork to reap -- at the moment the watchdog's `SIGKILL` actually took effect.
///
/// This measures a **different** thing from [`measure_itimer_prof_resolution`], not just the
/// same measurement against a different ceiling: `ITIMER_PROF`/`SIGPROF` is delivered by the
/// kernel *inside the child* against its own CPU-time clock, so that calibration's lateness comes
/// from signal-delivery/scheduling latency for a timer the kernel itself arms. The wall watchdog
/// in `zygote::read_with_wall_watchdog` is not a kernel timer at all -- it is the **parent**
/// polling `poll(2)` with a deadline computed from `Instant::now()`, checking that deadline only
/// each time `poll` returns (from data arriving or its own timeout), then issuing `SIGKILL` and
/// still having to wait for the kernel to schedule the child's actual death and for this parent's
/// blocking `wait4` to unblock. Its lateness sources are therefore the parent process's own
/// poll/wake/schedule granularity and the `SIGKILL`-to-reap handoff, not `SIGPROF` delivery --
/// callers should not assume this distribution has the same shape as
/// [`ItimerProfCalibration::deviation_from_50ms`], and this function does not attempt to make it
/// look like it does.
#[must_use]
pub fn measure_wall_watchdog_resolution(sample_count: usize) -> WallWatchdogCalibration {
    let mut raw_wall_ms_samples = Vec::with_capacity(sample_count);
    let mut anomalies = Vec::new();

    for index in 0..sample_count {
        let generation = u64::try_from(index).unwrap_or(u64::MAX).wrapping_add(1);
        match zygote::run_one_case(
            generation,
            FixtureKind::WallHog,
            FaultMode::None,
            DECODE_APPLY_WALL_MS_MAX_U64,
        ) {
            CaseRunOutcome::Ran(observation) => {
                if observation.primary_cause == IsolationOracle::WallCeiling {
                    raw_wall_ms_samples.push(observation.wall_ms);
                } else {
                    anomalies.push(format!(
                        "sample {index}: expected wall_ceiling, observed {:?} (raw_wait_status={})",
                        observation.primary_cause, observation.raw_wait_status
                    ));
                }
            }
            CaseRunOutcome::Failure(reason) => {
                anomalies.push(format!("sample {index}: case failed to run: {reason:?}"));
            }
        }
    }

    let deviation_samples: Vec<f64> = raw_wall_ms_samples
        .iter()
        .map(|wall_ms| wall_ms - NOMINAL_WALL_BUDGET_MS)
        .collect();
    let deviation_from_100ms = Distribution::from_samples(deviation_samples).unwrap_or(Distribution {
        sample_count: 0,
        median: 0.0,
        p95: 0.0,
        max: 0.0,
        min: 0.0,
    });

    WallWatchdogCalibration {
        raw_wall_ms_samples,
        deviation_from_100ms,
        anomalous_sample_count: anomalies.len(),
        anomalies,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_from_samples_computes_expected_percentiles() {
        let samples: Vec<f64> = (1..=100).map(f64::from).collect();
        let distribution = Distribution::from_samples(samples).unwrap_or(Distribution {
            sample_count: 0,
            median: 0.0,
            p95: 0.0,
            max: 0.0,
            min: 0.0,
        });
        assert_eq!(distribution.sample_count, 100);
        // `min`/`max` are picked (not computed via arithmetic) from a fixed 1..=100 integer-
        // derived input, so they are exactly 1.0/100.0 -- exact equality is the correct check.
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(distribution.min, 1.0);
            assert_eq!(distribution.max, 100.0);
        }
        // Nearest-rank median of 1..=100 is around 50-51.
        assert!((50.0..=51.0).contains(&distribution.median));
    }

    #[test]
    fn distribution_from_empty_samples_is_none() {
        assert!(Distribution::from_samples(Vec::new()).is_none());
    }
}
