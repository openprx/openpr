//! Production Loro CRDT adapter for Sylvode Flow.
//!
//! The v0.3 spike (`spikes/collab-shared` + `spikes/collab-loro`) evaluated Loro against a second
//! candidate (`collab-yrs-yjs`) behind a shared, engine-agnostic harness so the comparison could
//! not be tilted by adapter-specific shortcuts. That evaluation selected Loro
//! (`decisions/ADR-000x` in `/opt/working/sylvode-flow`); this crate is the promotion of the
//! winning adapter to a real workspace member.
//!
//! What moved here from the spike, and why:
//! - [`error`], [`frontier`], [`operation`], [`semantic`], [`limits`]: the engine-agnostic types
//!   and boundary-validation policy every adapter had to honor. They have no dependency on which
//!   engine is underneath, so they carry over unchanged.
//! - [`engine`]: the `CollabEngine` sync contract (`load`/`import_update`/`export_snapshot`/
//!   `export_from`/`frontier`) plus [`engine::LoroCollabEngine`], the concrete Loro adapter.
//!
//! What deliberately did *not* move: `spikes/collab-shared`'s `corpus`, `benchmark`, `fixture`,
//! `rng`, and `order` modules, and `spikes/collab-loro`'s process-isolation harness. Those exist to
//! let the convergence corpus fuzz and time two candidates fairly against each other; now that
//! selection is final there is exactly one adapter, so that comparison machinery has no job left
//! to do here. `spikes/` is left untouched as the evaluation evidence trail.
//!
//! [`isolation`] is a distinct, new module -- not a port of `spikes/collab-shared::isolation`. That
//! spike module is `ADR-0014`'s v0.3 *calibration* host (explicitly scoped, by that ADR's own
//! section 8, as "a selection/acceptance facility, not a v0.4 production safety boundary"); this
//! crate's `isolation` is the real, production-facing enforcement of `contracts/limits-v1.md`'s
//! "Isolated decode/apply" ceilings for `apps/api`'s collab write path, spawning a fresh worker
//! process per call rather than reusing the spike's fork-no-exec zygote (see
//! `isolation::host`'s module doc for why that specific mechanism does not fit a live,
//! multi-threaded async server). It reuses that ADR's online-enforcement *mechanisms*
//! (`SIGPROF`/`setitimer`, a counting `GlobalAlloc`, an independent wall watchdog) over a
//! different process-creation primitive, not its code.
//!
//! `CorpusEngine` (the spike's second, harness-only trait providing `new_empty`/`apply_operation`/
//! `semantic_snapshot`) is likewise not carried over as a generic trait: with a single production
//! engine there is nothing left to abstract over, so those three capabilities are plain inherent
//! methods on [`engine::LoroCollabEngine`] instead.

pub mod engine;
pub mod error;
pub mod frontier;
#[cfg(target_os = "linux")]
pub mod isolation;
pub mod limits;
pub mod operation;
pub mod semantic;

pub use engine::{CollabEngine, Diff, EngineMetadata, LoroCollabEngine, metadata};
pub use error::{CollabError, InputLimits};
pub use frontier::Frontier;
pub use limits::{DocumentLimits, LimitViolation};
pub use operation::{NodeId, NodeKind, Operation};
pub use semantic::{SemanticNode, SemanticSnapshot};
