//! Sylvode Flow's WebSocket collaboration layer (`collab-protocol-v1.md`, `ADR-0007`, `ADR-0010`,
//! `ADR-0012` §3).
//!
//! - [`authz`]: `ADR-0012` §3 effective-permission computation and §3.1 commit-time `authz_epoch`
//!   fencing.
//! - [`origin`]: `ADR-0007`'s strict `Origin` allowlist and normalization.
//! - [`ticket`]: one-time, 60s TTL WebSocket tickets (`collab_tickets`).
//! - [`limits`]: the transport/server-side numeric ceilings `contracts/limits-v1.md` freezes for
//!   this layer (frame/update/presence sizes, warm cache budgets, document lock budgets) — the
//!   structural per-operation limits in [`collab_core::limits`] are a different, engine-level
//!   concern and are not duplicated here.
//! - [`bootstrap`]: the `REPEATABLE READ READ ONLY` consistency loader shared by the WebSocket
//!   `snapshot` frame (`collab-protocol-v1.md` "一致性 bootstrap/snapshot").
//! - [`cache`]: the bounded per-document warm cache (`ADR-0010`, decision B).
//! - [`coordinator`]: the instance-local, per-document coordinator (`ADR-0010`'s "第 0 层").
//! - [`egress`]: the per-session outbound `accepted` sequencer (`collab-protocol-v1.md` "accepted
//!   出站顺序") — strict per-subscription seq monotonicity, gap backfill from `collab_updates`,
//!   `resync(reason="outbound_gap")` when it cannot.
//! - [`frame`]: the wire shapes of every Collab Protocol v1 frame.
//! - [`write`]: the server write algorithm (`ADR-0010`'s "写入算法" / `collab-protocol-v1.md`'s
//!   "服务端写入顺序"), including the commit-time epoch fencing barrier and the document row lock.
//! - [`registry`]: the single-instance session registry presence/broadcast use.
//! - [`session`]: the per-connection WebSocket actor loop.
//! - [`snapshot`]: snapshot advancement (gate 7 `minimal_snapshot_advancement_bounds_tail`) —
//!   candidate build/validation outside any lock, a short fixed-write locked commit, and the
//!   soft/hard trigger policy from `limits-v1.md`'s "Server persistence path budgets".

pub mod authz;
pub mod bootstrap;
pub mod cache;
pub mod coordinator;
pub mod egress;
pub mod frame;
pub mod limits;
pub mod origin;
pub mod permission_cache;
pub mod registry;
pub mod revocation;
pub mod runtime;
pub mod session;
pub mod snapshot;
pub mod ticket;
pub mod write;

pub(super) fn cache_db_permission(
    cache: &permission_cache::PermissionCache,
    workspace_id: uuid::Uuid,
    principal_kind: permission_cache::PrincipalKind,
    principal_id: uuid::Uuid,
    object_id: uuid::Uuid,
    level: authz::PermissionLevel,
    authz_epoch: i64,
) {
    cache.put(
        workspace_id,
        principal_kind,
        principal_id,
        object_id,
        level,
        authz_epoch,
    );
}
