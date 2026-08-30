//! Server-side process isolation for `LoroCollabEngine::import_update`, per `ADR-0014` and
//! `contracts/limits-v1.md`'s "Isolated decode/apply" table (`decode_apply_cpu_ms_max` = 50ms,
//! `decode_apply_wall_ms_max` = 100ms, `isolated_apply_memory_bytes_max` = 128 MiB).
//!
//! [`isolated_apply`] is the entry point `apps/api`'s `flow::collab::write::hydrate_and_apply`
//! calls. See [`host`]'s module doc for the full design: a fresh worker process per call, why that
//! (not `ADR-0014`'s fork-no-exec zygote) is the right primitive inside a live multi-threaded
//! async server, and exactly what is/isn't carried over from the `spikes/collab-shared`
//! calibration host.
//!
//! - [`wire`] -- the request/response codec (pure, `unsafe`-free).
//! - [`alloc`] -- the counting `GlobalAlloc` that is the online memory-ceiling enforcer. Only
//!   installed as the process's actual global allocator by `src/bin/isolated_apply_worker.rs`.
//! - [`child_runtime`] -- worker-side primitives: single-threadedness verification, the
//!   `RLIMIT_AS` backstop, arming/disarming the `SIGPROF` CPU-ceiling timer.
//! - [`host`] -- parent-side orchestration: spawn, wall watchdog, response-frame decode,
//!   exit-signal classification.
//!
//! # Why `unsafe_code` is allowed here
//!
//! The workspace root `Cargo.toml` sets `unsafe_code = "deny"` via `[workspace.lints.rust]`, cast
//! as a crate-level `-D unsafe_code` flag by Cargo -- not `forbid`, so it can be locally
//! overridden. This module tree is the only place in `collab-core` that needs raw
//! `setitimer`/`sigaction`/`setrlimit`/`kill`/`poll`-family syscalls; every individual `unsafe`
//! block still carries its own `// SAFETY:` comment (the workspace clippy lint
//! `undocumented_unsafe_blocks = "deny"` is *not* overridden and still applies here).
#![allow(unsafe_code, clippy::too_long_first_doc_paragraph)]

pub mod alloc;
pub mod child_runtime;
pub mod host;
pub mod wire;

pub use host::{
    DECODE_APPLY_CPU_MS_MAX, DECODE_APPLY_WALL_MS_MAX, ISOLATED_APPLY_MEMORY_BYTES_MAX, IsolatedApplyError,
    IsolatedApplySuccess, WORKER_BINARY_PATH_ENV, isolated_apply,
};
