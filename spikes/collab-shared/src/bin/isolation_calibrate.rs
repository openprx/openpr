//! `isolation-calibrate` — the ADR-0014 work package 3a calibration entry point.
//!
//! This binary is itself the "single-threaded zygote" ADR-0014 section 0 requires: `main` does
//! nothing that could start a background thread before handing off to
//! `collab_shared::isolation::zygote::run_one_case`, which asserts `/proc/self/task == 1`
//! immediately before every `fork()`. Running each case as a fresh process invocation of this
//! binary (rather than, say, a `#[test]` inside a multi-threaded `cargo test` harness process) is
//! what makes that precondition something this tool can actually satisfy rather than merely hope
//! for — see the delivery report for why a `cargo test` binary cannot offer the same guarantee.
//!
//! # This is calibration output, not gate evidence
//!
//! Every JSON object this binary prints is intentionally a different shape from
//! `docs/schemas/sylvode-flow-convergence-result-v1.schema.json`'s `$defs/isolation_case` — that
//! schema's `head_hash_before`/`frontier_hash_before`/etc. fields are meaningful only for a real
//! Flow document apply, and this tool's fixtures are synthetic CPU/memory/wall probes, not
//! document applies. Nothing printed here should be interpreted as
//! `isolated_apply_not_async_timeout: passed`; that hard gate stays `pending` regardless (see
//! ADR-0014's "未决" section and `crate::isolation` module docs).
//!
//! # Usage
//!
//! ```text
//! isolation-calibrate run <fixture>
//! isolation-calibrate fault <mode>
//! isolation-calibrate calibrate [--samples N]
//! ```
//!
//! `<fixture>` is one of: `completed`, `cpu-hog`, `wall-hog`, `memory-hog`,
//! `address-space-backstop`, `crashed`, `allocation-baseline`.
//! `<mode>` is one of: `persistent-block`, `transient-block-restore`, `ignore` — always runs the
//! `cpu-hog` workload under that `SIGPROF` fault injection.
//!
//! This binary is intentionally not built with `clap` or any other new dependency: the task this
//! implements restricts new dependencies to `libc` and a small CRC32 crate.
#![allow(clippy::print_stdout, clippy::print_stderr)]

#[cfg(target_os = "linux")]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let exit_code = linux_impl::run(args.get(1..).unwrap_or(&[]));
    std::process::exit(exit_code);
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("isolation-calibrate requires target_os = \"linux\" (ADR-0014 section 0/10)");
    std::process::exit(2);
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use collab_shared::isolation::alloc::CountingAllocator;
    use collab_shared::isolation::calibrate::{self, MIN_SAMPLES};
    use collab_shared::isolation::child::{FaultMode, FixtureKind};
    use collab_shared::isolation::zygote::{self, CaseObservation, CaseRunOutcome};
    use collab_shared::result::{IsolationOracle, Status};
    use serde::Serialize;

    /// This binary's counting allocator is installed here — and only here. `collab-shared` as a
    /// library never swaps out the global allocator for its own consumers (`collab-loro-spike`,
    /// `collab-yrs-yjs-spike`, or `collab-shared`'s own `cargo test` binary), which would
    /// silently change their allocation behavior; see `isolation::alloc`'s module doc.
    #[global_allocator]
    static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

    const DEFAULT_WALL_BUDGET_MS: u64 = 100;

    pub fn run(args: &[String]) -> i32 {
        match args.first().map(String::as_str) {
            Some("run") => run_fixture_command(args.get(1)),
            Some("fault") => run_fault_command(args.get(1)),
            Some("calibrate") => run_calibrate_command(args.get(1..).unwrap_or(&[])),
            _ => {
                print_usage();
                2
            }
        }
    }

    fn print_usage() {
        eprintln!("usage:");
        eprintln!(
            "  isolation-calibrate run <completed|cpu-hog|wall-hog|memory-hog|address-space-backstop|crashed|allocation-baseline>"
        );
        eprintln!("  isolation-calibrate fault <persistent-block|transient-block-restore|ignore>");
        eprintln!("  isolation-calibrate calibrate [--samples N]");
    }

    fn parse_fixture(name: &str) -> Option<FixtureKind> {
        match name {
            "completed" => Some(FixtureKind::Completed),
            "cpu-hog" => Some(FixtureKind::CpuHog),
            "wall-hog" => Some(FixtureKind::WallHog),
            "memory-hog" => Some(FixtureKind::MemoryHog),
            "address-space-backstop" => Some(FixtureKind::AddressSpaceBackstop),
            "crashed" => Some(FixtureKind::Crashed),
            "allocation-baseline" => Some(FixtureKind::AllocationBaseline),
            _ => None,
        }
    }

    fn parse_fault_mode(name: &str) -> Option<FaultMode> {
        match name {
            "persistent-block" => Some(FaultMode::PersistentBlock),
            "transient-block-restore" => Some(FaultMode::TransientBlockThenRestore),
            "ignore" => Some(FaultMode::Ignore),
            _ => None,
        }
    }

    fn run_fixture_command(name: Option<&String>) -> i32 {
        let Some(fixture) = name.and_then(|value| parse_fixture(value)) else {
            eprintln!("error: unknown or missing fixture name");
            print_usage();
            return 2;
        };
        run_one_and_report(fixture, FaultMode::None)
    }

    fn run_fault_command(name: Option<&String>) -> i32 {
        let Some(fault_mode) = name.and_then(|value| parse_fault_mode(value)) else {
            eprintln!("error: unknown or missing fault mode name");
            print_usage();
            return 2;
        };
        run_one_and_report(FixtureKind::CpuHog, fault_mode)
    }

    fn run_one_and_report(fixture: FixtureKind, fault_mode: FaultMode) -> i32 {
        let outcome = zygote::run_one_case(1, fixture, fault_mode, DEFAULT_WALL_BUDGET_MS);
        match outcome {
            CaseRunOutcome::Ran(observation) => {
                let report = CalibrationCaseReport::from_observation(&observation, fault_mode);
                print_json(&report);
                i32::from(!observation.verdict_passed)
            }
            CaseRunOutcome::Failure(reason) => {
                let report = CalibrationRunFailure {
                    note: "calibration outcome: case did not run to a classifiable conclusion",
                    fixture_id: fixture.fixture_id(),
                    reason: format!("{reason:?}"),
                };
                print_json(&report);
                3
            }
        }
    }

    fn run_calibrate_command(args: &[String]) -> i32 {
        let mut samples = MIN_SAMPLES;
        let mut index = 0;
        while index < args.len() {
            if args.get(index).is_some_and(|value| value == "--samples") {
                if let Some(value) = args.get(index + 1).and_then(|value| value.parse::<usize>().ok()) {
                    samples = value.max(MIN_SAMPLES);
                }
                index += 2;
            } else {
                index += 1;
            }
        }

        eprintln!("running itimer_prof resolution calibration ({samples} samples)...");
        let itimer_prof_resolution = calibrate::measure_itimer_prof_resolution(samples);
        eprintln!("running rss/allocator ratio calibration ({samples} samples)...");
        let rss_allocator_ratio = calibrate::measure_rss_allocator_ratio(samples);
        eprintln!("running fork overhead calibration ({samples} samples)...");
        let fork_overhead = calibrate::measure_fork_overhead(samples);
        eprintln!("running wall watchdog resolution calibration ({samples} samples)...");
        let wall_watchdog_resolution = calibrate::measure_wall_watchdog_resolution(samples);

        let summary = CalibrationSummary {
            note: "GO-CALIBRATION output (ADR-0014 section 10) -- not gate evidence; \
                   isolated_apply_not_async_timeout remains pending regardless of these numbers",
            requested_samples: samples,
            itimer_prof_resolution,
            rss_allocator_ratio,
            fork_overhead,
            wall_watchdog_resolution,
        };
        print_json(&summary);
        0
    }

    fn print_json<T: Serialize>(value: &T) {
        match serde_json::to_string_pretty(value) {
            Ok(json) => println!("{json}"),
            Err(error) => eprintln!("error: failed to serialize output: {error}"),
        }
    }

    #[derive(Debug, Serialize)]
    struct CalibrationCaseReport {
        fixture_id: &'static str,
        fault_mode: &'static str,
        expected_oracle: IsolationOracle,
        primary_cause: IsolationOracle,
        verdict: Status,
        termination_causes: Vec<TerminationCauseReport>,
        cpu_ms: f64,
        wall_ms: f64,
        fork_overhead_ms: f64,
        total_wall_ms: f64,
        allocated_active_peak_bytes: u64,
        allocated_attempted_peak_bytes: u64,
        rss_peak_bytes: u64,
        sigprof_armed: bool,
        meter_tampered: bool,
        raw_wait_status: i32,
    }

    #[derive(Debug, Serialize)]
    struct TerminationCauseReport {
        cause: IsolationOracle,
        source: &'static str,
    }

    const fn fault_mode_label(mode: FaultMode) -> &'static str {
        match mode {
            FaultMode::None => "none",
            FaultMode::PersistentBlock => "persistent-block",
            FaultMode::TransientBlockThenRestore => "transient-block-restore",
            FaultMode::Ignore => "ignore",
        }
    }

    impl CalibrationCaseReport {
        fn from_observation(observation: &CaseObservation, fault_mode: FaultMode) -> Self {
            Self {
                fixture_id: observation.fixture.fixture_id(),
                fault_mode: fault_mode_label(fault_mode),
                expected_oracle: observation.expected_oracle,
                primary_cause: observation.primary_cause,
                verdict: if observation.verdict_passed {
                    Status::Passed
                } else {
                    Status::Failed
                },
                termination_causes: observation
                    .termination_causes
                    .iter()
                    .map(|event| TerminationCauseReport {
                        cause: event.cause,
                        source: event.source,
                    })
                    .collect(),
                cpu_ms: observation.cpu_ms,
                wall_ms: observation.wall_ms,
                fork_overhead_ms: observation.fork_overhead_ms,
                total_wall_ms: observation.total_wall_ms,
                allocated_active_peak_bytes: observation.allocated_active_peak_bytes,
                allocated_attempted_peak_bytes: observation.allocated_attempted_peak_bytes,
                rss_peak_bytes: observation.rss_peak_bytes,
                sigprof_armed: observation.sigprof_armed,
                meter_tampered: observation.raw.meter_tampered_flag,
                raw_wait_status: observation.raw_wait_status,
            }
        }
    }

    #[derive(Debug, Serialize)]
    struct CalibrationRunFailure {
        note: &'static str,
        fixture_id: &'static str,
        reason: String,
    }

    #[derive(Debug, Serialize)]
    struct CalibrationSummary {
        note: &'static str,
        requested_samples: usize,
        itimer_prof_resolution: calibrate::ItimerProfCalibration,
        rss_allocator_ratio: calibrate::RssAllocatorRatioCalibration,
        fork_overhead: calibrate::ForkOverheadCalibration,
        wall_watchdog_resolution: calibrate::WallWatchdogCalibration,
    }
}
