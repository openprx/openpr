//! Sylvode Flow's server-side domain logic, layered per `versions/v0.4-flow-alpha.md` ("Rust 与
//! API": "`apps/api/src/flow/` 分为 model、command、query、repository、projection、policy").
//!
//! - [`model`]: wire-shape response types (`FlowObjectView`, `AcceptedChange`, list/history
//!   response envelopes) shared by every handler in `crate::routes::flow`.
//! - [`policy`]: the workspace-membership + `flow_enabled` gate every handler runs first.
//! - [`command`]: the shared object command dispatcher and object creation path.
//! - [`collections`]: v0.6 Collection schema/view/Record commands and synchronous typed
//!   projections; every Record remains an independent collaborative document.
//! - [`grants`]: `ADR-0012`'s v0.5 authorization surface — `flow_object_grants` and the
//!   `inherit_from_parent` boundary, with the §4.1 self-lockout guard and the §3.1 epoch lock.
//! - [`move_object`]: `ADR-0012` §4's cross-parent move, and the only command in the frozen v0.5
//!   set that walks `ADR-0013` §2's multi-document ordered-lock path.
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

pub mod bridge;
pub mod collab;
pub mod collections;
pub mod command;
pub mod event_origin;
pub mod event_policy;
pub mod grants;
pub mod import;
pub mod maintenance;
pub mod model;
pub mod move_object;
pub mod operations;
pub mod package;
pub mod policy;
pub mod projection;
pub mod query;
pub mod relations;
pub mod repository;
pub mod search;
