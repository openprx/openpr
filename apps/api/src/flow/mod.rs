//! Sylvode Flow's server-side domain logic, layered per `versions/v0.4-flow-alpha.md` ("Rust 与
//! API": "`apps/api/src/flow/` 分为 model、command、query、repository、projection、policy").
//!
//! - [`model`]: wire-shape response types (`FlowObjectView`, `AcceptedChange`, list/history
//!   response envelopes) shared by every handler in `crate::routes::flow`.
//! - [`policy`]: the workspace-membership + `flow_enabled` gate every handler runs first.
//! - [`command`]: the one write path this package ships — object creation.
//! - [`query`]: the three read paths — get, list, history.
//! - [`repository`]: hand-written parameterized SQL against `flow_objects` / `collab_documents` /
//!   `flow_object_projections` / `collab_updates` / `business_events` / `event_dispatch` (no
//!   SeaORM entities, matching every module past migration 0024).
//! - [`projection`]: turns a `collab_core::SemanticSnapshot` into the JSON/plain-text/markdown
//!   shapes the projection table and the REST responses use.
//!
//! `apps/api/src/routes/flow.rs` is the thin axum-handler layer on top of this module: request
//! parsing/validation of HTTP-specific shape (path/query extraction) lives there, while this
//! module owns the domain rules (idempotency, workspace/project/parent validation, the CRDT
//! document lifecycle, and the SQL).

pub mod collab;
pub mod command;
pub mod model;
pub mod policy;
pub mod projection;
pub mod query;
pub mod repository;
