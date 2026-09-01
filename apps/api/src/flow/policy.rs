//! Workspace-level gates every Flow REST handler must pass before touching `flow_objects`.
//!
//! v0.4 has no `flow_object_grants` yet (`ADR-0012` only lands the schema this version).
//!
//! The effective policy is exactly the workspace baseline `domain-model-v1.md` describes
//! (workspace admin gets `full_access`; a member gets the workspace's `default_member_level`,
//! frozen at `edit` in v0.4), which — since `flow_object_grants` is reserved-but-empty this
//! version — collapses to plain workspace membership: any member can read, and any member can
//! write, with zero regression from pre-Flow behavior. This module does not implement
//! `flow_object_grants`/`inherit_from_parent` inheritance — that is `ADR-0012`'s v0.5 surface.

use axum::http::Extensions;
use platform::app::AppState;
use uuid::Uuid;

use crate::error::ApiError;
use crate::middleware::bot_auth::require_workspace_access;

use super::repository;

/// Workspace membership (`unauthenticated`/`forbidden`/`not_found` per `error-mapping-v1.md`)
/// *and* the workspace's `flow_enabled` rollout flag.
///
/// Checked independently of any UI navigation gate, as `v0.4-flow-alpha.md` requires ("API 和
/// WebSocket 必须独立执行同一 flag 检查，不能只依赖 UI"). Returns `(actor_id, role, is_bot)`,
/// mirroring [`require_workspace_access`].
pub async fn require_flow_workspace_access(
    state: &AppState,
    extensions: &Extensions,
    workspace_id: Uuid,
) -> Result<(Uuid, String, bool), ApiError> {
    let actor = require_workspace_access(state, extensions, workspace_id).await?;
    require_flow_enabled(state, workspace_id).await?;
    Ok(actor)
}

/// Plain workspace membership, deliberately *not* gated on `flow_enabled`.
///
/// `GET /workspaces/{workspace_id}/features/flow` is how a caller discovers whether Flow is
/// enabled at all, so gating it on `require_flow_enabled` would make the flag unreadable exactly
/// when a caller most needs to read it (before it is ever turned on). `rest-api-v1.md` lists this
/// endpoint's auth as plain "member user/bot read", not the `OwnedBy(FlowObject)`/workspace-access
/// gate every object endpoint uses.
pub async fn require_flow_feature_read_access(
    state: &AppState,
    extensions: &Extensions,
    workspace_id: Uuid,
) -> Result<(Uuid, String, bool), ApiError> {
    require_workspace_access(state, extensions, workspace_id).await
}

/// Workspace membership *and* an admin-level role, for `PUT /workspaces/{workspace_id}/features/flow`.
///
/// `rest-api-v1.md`: "workspace admin user 或 policy-approved Flow admin bot；显式 workspace admin
/// policy". `require_workspace_access` already synthesizes `role="admin"` for a bot token carrying
/// `BotPermission::Admin` (see `middleware::bot_auth::bot_role_from_permissions`) and the human
/// workspace role otherwise, so one role check here covers both actor kinds — matching the
/// `role != "owner" && role != "admin"` gate every other workspace-admin endpoint in this crate
/// uses (`routes::webhook::verify_workspace_admin`, `routes::workspace`, ...).
pub async fn require_flow_workspace_admin_access(
    state: &AppState,
    extensions: &Extensions,
    workspace_id: Uuid,
) -> Result<(Uuid, String, bool), ApiError> {
    let actor = require_workspace_access(state, extensions, workspace_id).await?;
    if actor.1 != "owner" && actor.1 != "admin" {
        return Err(ApiError::Forbidden("workspace admin access required".to_string()));
    }
    Ok(actor)
}

/// `feature_disabled` (`error-mapping-v1.md`: `Forbidden`/403/HTTP 200) when the workspace has no
/// `flow_workspace_settings` row yet, or has one with `flow_enabled = false`.
///
/// A missing row is deliberately treated the same as an explicit `false`: `flow_workspace_settings`
/// is provisioned lazily (nothing in this package inserts a default row), so "never turned on" and
/// "turned off" must fail exactly the same way (fail closed, not fail open on absence).
pub async fn require_flow_enabled(state: &AppState, workspace_id: Uuid) -> Result<(), ApiError> {
    require_flow_enabled_on(&state.db, workspace_id).await
}

/// [`require_flow_enabled`] against a bare connection instead of the whole [`AppState`].
///
/// The collab ticket and WebSocket-upgrade paths (`flow::collab::ticket`, `routes::collab`) need
/// the identical gate but sit below `AppState` — `ticket::issue` is generic over
/// `ConnectionTrait` so it can run inside a transaction. Both entry points share this one body so
/// the rejection stays a single `feature_disabled` shape (`error-mapping-v1.md`: `Forbidden` /
/// 403 / HTTP 200) rather than each caller re-spelling the message.
///
/// # Errors
/// `Forbidden` when the workspace has no `flow_workspace_settings` row or has one with
/// `flow_enabled = false`. Propagates a database failure otherwise.
pub async fn require_flow_enabled_on<C: sea_orm::ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
) -> Result<(), ApiError> {
    if repository::fetch_flow_enabled(conn, workspace_id).await? {
        Ok(())
    } else {
        Err(ApiError::Forbidden(
            "flow is not enabled for this workspace".to_string(),
        ))
    }
}
