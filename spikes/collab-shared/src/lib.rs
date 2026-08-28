//! Candidate-agnostic shared runner for the v0.3 Loro/Yrs convergence corpus and benchmark.
//!
//! Structural rule (work package 3b): `collab-loro` and `collab-yrs-yjs` each provide only an
//! `impl CorpusEngine`/`impl CollabEngine` adapter over their own engine. Fixture generation, the
//! corpus runner, the benchmark sampler, and the result-document shapes all live here, once, so
//! fairness between the two candidates cannot drift.

pub mod benchmark;
pub mod corpus;
pub mod engine;
pub mod error;
pub mod fixture;
pub mod frontier;
#[cfg(target_os = "linux")]
pub mod isolation;
pub mod limits;
pub mod operation;
pub mod order;
pub mod result;
pub mod rng;
pub mod semantic;

pub use engine::{CollabEngine, CorpusEngine, Diff};
pub use error::{CollabError, InputLimits};
pub use frontier::Frontier;
pub use limits::{DocumentLimits, LimitViolation};
pub use operation::{NodeId, NodeKind, Operation};
pub use semantic::{SemanticNode, SemanticSnapshot};
