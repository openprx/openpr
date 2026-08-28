//! Termination-cause resolution (ADR-0014 sections 1.1, 1.2, 3).
//!
//! This module holds no `unsafe` code: it is pure classification logic over the raw signals the
//! zygote and child collected (`wait4` status/rusage, shared-page flags, frame decode outcome).
//! Keeping it separate from `zygote.rs`/`child.rs` means the priority rule and the "belt and
//! suspenders" self-measured-CPU cross-check (see [`resolve`] doc comment) can be unit-tested
//! without forking a real process.

use crate::result::IsolationOracle;

/// The frozen v0.4 safety ceilings this calibration host measures against
/// (`contracts/limits-v1.md`, "Isolated decode/apply" table).
///
/// These are the values under calibration, not values the calibration run is free to invent.
pub const DECODE_APPLY_CPU_MS_MAX: f64 = 50.0;
pub const DECODE_APPLY_WALL_MS_MAX: f64 = 100.0;
/// `u64` view of [`DECODE_APPLY_WALL_MS_MAX`], for `wall_budget_ms`-style parameters that take a
/// whole millisecond count (e.g. [`crate::isolation::zygote::run_one_case`]).
///
/// `DECODE_APPLY_WALL_MS_MAX` is a fixed, whole-millisecond frozen contract value, so this
/// conversion is always exact -- it can never actually truncate or lose sign.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub const DECODE_APPLY_WALL_MS_MAX_U64: u64 = DECODE_APPLY_WALL_MS_MAX as u64;
pub const ISOLATED_APPLY_MEMORY_BYTES_MAX: u64 = 134_217_728;
/// ADR-0014 section 3: `RLIMIT_AS` backstop, independent of the allocator-counted ceiling above.
pub const ADDRESS_SPACE_BACKSTOP_BYTES_MAX: u64 = 512 * 1024 * 1024;

/// One resolved termination/observation event, pre-serialization.
///
/// Carries the same fields as `result::IsolationTerminationCause` but keeps `source` as
/// `&'static str` here since every caller in this module passes a literal; the `String`
/// conversion happens once at the serialization boundary in `bin/isolation_calibrate.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminationEvent {
    pub cause: IsolationOracle,
    pub source: &'static str,
}

/// Priority order from ADR-0014 section 1.2: `memory_ceiling > cpu_ceiling >
/// cpu_ceiling_enforcement_failed > wall_ceiling > crashed > completed`.
///
/// `address_space_backstop` is not in that ADR sentence (the ADR calls it out separately as "an
/// independent terminal state, not the memory ceiling") — this implementation ranks it
/// immediately below `memory_ceiling`, since both are memory-domain signals and
/// `address_space_backstop` should not be masked by a CPU/wall cause that happened to also be
/// present. This ranking choice is *not* specified by ADR-0014 and is flagged in the delivery
/// report as a place where the ADR does not fully specify behavior.
const PRIORITY: [IsolationOracle; 7] = [
    IsolationOracle::MemoryCeiling,
    IsolationOracle::AddressSpaceBackstop,
    IsolationOracle::CpuCeiling,
    IsolationOracle::CpuCeilingEnforcementFailed,
    IsolationOracle::WallCeiling,
    IsolationOracle::Crashed,
    IsolationOracle::Completed,
];

fn priority_rank(cause: IsolationOracle) -> usize {
    PRIORITY
        .iter()
        .position(|candidate| *candidate == cause)
        .unwrap_or(PRIORITY.len())
}

/// Raw, unclassified observations the zygote collected about one case.
///
/// Every field here is something the parent (or, for `child_self_measured_cpu_ms`, the child via
/// the result frame) actually measured — this struct intentionally has no derived/computed fields
/// so `resolve`'s logic is the single place that turns raw signals into `IsolationOracle` causes.
// Each bool below is an independent raw OS-level signal (wait4 status bits, shared-page flags,
// frame decode outcome) that `resolve` cross-references, not a set of mutually exclusive states a
// state machine would model -- collapsing them into enums would not remove any of the seven
// independent axes of information this type carries.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy)]
pub struct RawObservation {
    pub wifexited: bool,
    pub wexitstatus: i32,
    pub wifsignaled: bool,
    pub wtermsig: i32,
    pub wall_watchdog_fired: bool,
    /// `wait4`'s `rusage.ru_utime + ru_stime`, in ms — the ADR's "post-hoc evidence" CPU source,
    /// always available for any reaped child regardless of whether it wrote a result frame.
    pub parent_rusage_cpu_ms: f64,
    /// The child's own `arm-time` vs `window-close` `CLOCK_PROCESS_CPUTIME_ID` delta, in ms —
    /// only present when the result frame decoded successfully (the child had to survive long
    /// enough to report it).
    pub child_self_measured_cpu_ms: Option<f64>,
    pub memory_cause_flag: bool,
    pub address_space_backstop_flag: bool,
    /// Disposition/mask mismatch detected by the child's own window-close readback
    /// (ADR-0014 section 1.1.3). Forces `verdict: failed` regardless of matched cause.
    pub meter_tampered_flag: bool,
    /// `false` when the CRC32/length frame failed to validate, or the frame's presence/absence
    /// disagreed with the `wait4` exit status (ADR-0014 section 2: "`read` 到 EOF 与 `wait4`
    /// 退出状态双重确认，不一致判 `crashed`").
    pub frame_integrity_ok: bool,
}

/// Resolves raw observations into the `termination_causes[]` event array plus the single
/// highest-priority cause used as the case's primary oracle.
///
/// Beyond the ADR-0014 section 1.1.4 rule (wall watchdog fired **and** CPU ≥ 50ms →
/// `cpu_ceiling_enforcement_failed`), this generalizes the same check to fire **whenever**
/// self-measured (or, if unavailable, `wait4`-observed) CPU is at or over the 50ms ceiling but no
/// `cpu_ceiling` (`SIGPROF`) cause was recorded — not only when the wall watchdog also fired.
/// This closes a gap the ADR's own text acknowledges but does not solve: a child that blocks
/// `SIGPROF`, burns CPU past the ceiling, then *drains the now-pending signal via
/// `sigtimedwait`* before restoring the mask can finish "normally" well inside the 100ms wall
/// budget, so the wall watchdog never fires and the ADR's stated rule alone would never catch it.
/// Cross-checking `CLOCK_PROCESS_CPUTIME_ID` (which a blocked/drained signal cannot falsify)
/// against the 50ms ceiling independent of whether the wall watchdog also fired is what makes the
/// `transient block-then-restore` SIGPROF fault-injection case in `child.rs` fail-closed; see the
/// delivery report for why the window-close disposition/mask readback alone cannot see this case.
#[must_use]
pub fn resolve(observation: &RawObservation) -> Vec<TerminationEvent> {
    let mut events: Vec<TerminationEvent> = Vec::new();

    if observation.memory_cause_flag {
        events.push(TerminationEvent {
            cause: IsolationOracle::MemoryCeiling,
            source: "counting_allocator",
        });
    }
    if observation.address_space_backstop_flag {
        events.push(TerminationEvent {
            cause: IsolationOracle::AddressSpaceBackstop,
            source: "rlimit_as_backstop",
        });
    }
    if observation.wifsignaled && observation.wtermsig == libc::SIGPROF {
        events.push(TerminationEvent {
            cause: IsolationOracle::CpuCeiling,
            source: "sigprof_signal",
        });
    }

    let has_clean_cpu_ceiling = events.iter().any(|event| event.cause == IsolationOracle::CpuCeiling);
    let cpu_delta_ms = observation
        .child_self_measured_cpu_ms
        .unwrap_or(observation.parent_rusage_cpu_ms);
    if !has_clean_cpu_ceiling && cpu_delta_ms >= DECODE_APPLY_CPU_MS_MAX {
        let source = if observation.wall_watchdog_fired {
            "wall_watchdog_rusage_crosscheck"
        } else {
            "self_measured_rusage_crosscheck"
        };
        events.push(TerminationEvent {
            cause: IsolationOracle::CpuCeilingEnforcementFailed,
            source,
        });
    }

    let has_cpu_family_cause = events.iter().any(|event| {
        matches!(
            event.cause,
            IsolationOracle::CpuCeiling | IsolationOracle::CpuCeilingEnforcementFailed
        )
    });
    if observation.wall_watchdog_fired && !has_cpu_family_cause {
        events.push(TerminationEvent {
            cause: IsolationOracle::WallCeiling,
            source: "wall_watchdog",
        });
    }

    let has_memory_or_cpu_cause = !events.is_empty();
    if observation.wifsignaled && !has_memory_or_cpu_cause {
        events.push(TerminationEvent {
            cause: IsolationOracle::Crashed,
            source: "wait_status_signal",
        });
    }
    if !observation.frame_integrity_ok && events.is_empty() {
        events.push(TerminationEvent {
            cause: IsolationOracle::Crashed,
            source: "frame_integrity",
        });
    }
    if events.is_empty() && observation.wifexited && observation.wexitstatus != 0 {
        events.push(TerminationEvent {
            cause: IsolationOracle::Crashed,
            source: "nonzero_exit_status",
        });
    }

    events
}

/// The single highest-priority cause from a resolved event array, or `Completed` when the array
/// is empty (nothing terminated the case and it exited cleanly).
#[must_use]
pub fn primary_cause(events: &[TerminationEvent]) -> IsolationOracle {
    events
        .iter()
        .min_by_key(|event| priority_rank(event.cause))
        .map_or(IsolationOracle::Completed, |event| event.cause)
}

/// `verdict: passed` requires: no meter tampering detected, the frame/exit-status cross-check was
/// consistent, and the observed primary cause matches what the fixture declared it expects.
#[must_use]
pub fn verdict_passed(
    observation: &RawObservation,
    expected_oracle: IsolationOracle,
    primary: IsolationOracle,
) -> bool {
    observation.frame_integrity_ok && !observation.meter_tampered_flag && primary == expected_oracle
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_observation() -> RawObservation {
        RawObservation {
            wifexited: true,
            wexitstatus: 0,
            wifsignaled: false,
            wtermsig: 0,
            wall_watchdog_fired: false,
            parent_rusage_cpu_ms: 1.0,
            child_self_measured_cpu_ms: Some(1.0),
            memory_cause_flag: false,
            address_space_backstop_flag: false,
            meter_tampered_flag: false,
            frame_integrity_ok: true,
        }
    }

    #[test]
    fn completed_case_has_no_causes_and_passes() {
        let observation = base_observation();
        let events = resolve(&observation);
        assert!(events.is_empty());
        assert_eq!(primary_cause(&events), IsolationOracle::Completed);
        assert!(verdict_passed(
            &observation,
            IsolationOracle::Completed,
            primary_cause(&events)
        ));
    }

    #[test]
    fn sigprof_kill_resolves_to_cpu_ceiling() {
        let observation = RawObservation {
            wifexited: false,
            wifsignaled: true,
            wtermsig: libc::SIGPROF,
            parent_rusage_cpu_ms: 51.0,
            child_self_measured_cpu_ms: None,
            ..base_observation()
        };
        let events = resolve(&observation);
        assert_eq!(primary_cause(&events), IsolationOracle::CpuCeiling);
    }

    #[test]
    fn wall_kill_with_low_cpu_is_wall_ceiling() {
        let observation = RawObservation {
            wifexited: false,
            wifsignaled: true,
            wtermsig: libc::SIGKILL,
            wall_watchdog_fired: true,
            parent_rusage_cpu_ms: 3.0,
            child_self_measured_cpu_ms: None,
            ..base_observation()
        };
        let events = resolve(&observation);
        assert_eq!(primary_cause(&events), IsolationOracle::WallCeiling);
    }

    #[test]
    fn wall_kill_with_high_cpu_is_enforcement_failed_not_wall_ceiling() {
        let observation = RawObservation {
            wifexited: false,
            wifsignaled: true,
            wtermsig: libc::SIGKILL,
            wall_watchdog_fired: true,
            parent_rusage_cpu_ms: 90.0,
            child_self_measured_cpu_ms: None,
            ..base_observation()
        };
        let events = resolve(&observation);
        assert_eq!(primary_cause(&events), IsolationOracle::CpuCeilingEnforcementFailed);
    }

    /// This is the "sigtimedwait drain" fault-injection scenario: the process finishes normally
    /// (no wall kill, exits 0) but its own CPU window delta shows it blew past the ceiling. Must
    /// classify as `cpu_ceiling_enforcement_failed`, not `completed`, or the transient
    /// block-then-restore fault case in `child.rs` would incorrectly get `verdict: passed`.
    #[test]
    fn normal_exit_with_high_self_measured_cpu_is_still_enforcement_failed() {
        let observation = RawObservation {
            child_self_measured_cpu_ms: Some(62.0),
            parent_rusage_cpu_ms: 62.0,
            ..base_observation()
        };
        let events = resolve(&observation);
        assert_eq!(primary_cause(&events), IsolationOracle::CpuCeilingEnforcementFailed);
        assert!(!verdict_passed(
            &observation,
            IsolationOracle::CpuCeiling,
            primary_cause(&events)
        ));
    }

    #[test]
    fn memory_cause_flag_outranks_a_concurrent_sigprof_kill() {
        let observation = RawObservation {
            wifexited: false,
            wifsignaled: true,
            wtermsig: libc::SIGPROF,
            memory_cause_flag: true,
            parent_rusage_cpu_ms: 51.0,
            child_self_measured_cpu_ms: None,
            ..base_observation()
        };
        let events = resolve(&observation);
        assert_eq!(primary_cause(&events), IsolationOracle::MemoryCeiling);
        // Both causes are still recorded in the event array even though only one is primary.
        assert!(events.iter().any(|event| event.cause == IsolationOracle::CpuCeiling));
    }

    #[test]
    fn meter_tampered_forces_failed_even_on_matching_cause() {
        let observation = RawObservation {
            wifexited: false,
            wifsignaled: true,
            wtermsig: libc::SIGPROF,
            meter_tampered_flag: true,
            parent_rusage_cpu_ms: 51.0,
            child_self_measured_cpu_ms: None,
            ..base_observation()
        };
        let events = resolve(&observation);
        let primary = primary_cause(&events);
        assert_eq!(primary, IsolationOracle::CpuCeiling);
        assert!(!verdict_passed(&observation, IsolationOracle::CpuCeiling, primary));
    }

    #[test]
    fn frame_integrity_failure_with_no_other_signal_is_crashed() {
        let observation = RawObservation {
            wifexited: false,
            wifsignaled: true,
            wtermsig: libc::SIGSEGV,
            frame_integrity_ok: false,
            parent_rusage_cpu_ms: 0.5,
            child_self_measured_cpu_ms: None,
            ..base_observation()
        };
        let events = resolve(&observation);
        assert_eq!(primary_cause(&events), IsolationOracle::Crashed);
    }

    #[test]
    fn address_space_backstop_outranks_wall_ceiling() {
        let observation = RawObservation {
            wifexited: true,
            wexitstatus: 0,
            address_space_backstop_flag: true,
            wall_watchdog_fired: true,
            parent_rusage_cpu_ms: 5.0,
            child_self_measured_cpu_ms: Some(5.0),
            ..base_observation()
        };
        let events = resolve(&observation);
        assert_eq!(primary_cause(&events), IsolationOracle::AddressSpaceBackstop);
    }
}
