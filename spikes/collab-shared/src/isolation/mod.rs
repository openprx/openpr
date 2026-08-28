//! Sylvode Flow v0.3 work package 3a: the isolated-apply **calibration** host (ADR-0014).
//!
//! # Scope — read before extending this module
//!
//! This is a `GO-CALIBRATION`, not a `GO-GATE`, deliverable (ADR-0014 section 10). It exists to
//! answer three empirical questions the ADR says cannot be answered by argument alone —
//! `ITIMER_PROF` resolution/lateness (see [`calibrate::measure_itimer_prof_resolution`]),
//! `rss_allocator_ratio_bound` (see [`calibrate::measure_rss_allocator_ratio`]), and
//! `fork_overhead_ms` distribution (see [`calibrate::measure_fork_overhead`]) — and to prove the
//! host's own fail-closed properties (the `SIGPROF` fault-injection modes in `child.rs`).
//!
//! **It does not produce gate evidence.** Nothing in this module, nor its `isolation-calibrate`
//! binary, writes to `/opt/working/sylvode-flow/evidence/`, and no output here should be read as
//! `isolated_apply_not_async_timeout: passed` — that hard gate stays `pending` per ADR-0014's
//! "未决" section regardless of what any individual fixture case in this module reports.
//!
//! # Module map
//!
//! - [`shared_page`] — the `mmap`'d `MAP_SHARED|MAP_ANONYMOUS` measurement page (ADR-0014 §0, §2).
//! - [`alloc`] — the counting `GlobalAlloc` that is the *online* memory-ceiling enforcer (§1, §3).
//!   Only installed as the process's actual global allocator by `src/bin/isolation_calibrate.rs`.
//! - [`frame`] — the `[length][crc32][payload]` result-frame wire protocol (§2).
//! - [`termination`] — pure classification logic: raw observations → `termination_causes[]` →
//!   primary cause → verdict (§1.1, §1.2, §3). No `unsafe` code.
//! - [`child`] — child-process-side logic: arming `SIGPROF` (§1.1), the fixture workloads, the
//!   three built-in `SIGPROF` fault-injection modes (§10 self-assertion 4).
//! - [`zygote`] — parent-side orchestration: the single-thread precondition (§10 self-assertion 1),
//!   `fork`, the wall watchdog, `wait4` reaping, and assembling what `termination` classifies.
//! - [`calibrate`] — the three empirical measurement loops for work package 3a's open questions.
//!
//! # Why `unsafe_code` is allowed here
//!
//! The workspace root `Cargo.toml` sets `unsafe_code = "deny"` via `[workspace.lints.rust]`, cast
//! as a crate-level `-D unsafe_code` flag by Cargo — not `forbid`, so it can be locally overridden.
//! This module is the one place in `collab-shared` that needs raw `fork`/`mmap`/`sigaction`/
//! `setitimer`/`wait4`/`setrlimit`-family syscalls the ADR's process model requires, and every
//! individual `unsafe` block still carries its own `// SAFETY:` comment (workspace clippy lint
//! `undocumented_unsafe_blocks = "deny"` is *not* overridden and still applies inside this module).
#![allow(unsafe_code)]

pub mod alloc;
pub mod calibrate;
pub mod child;
pub mod frame;
pub mod shared_page;
pub mod termination;
pub mod zygote;
