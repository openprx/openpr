//! Workspace and effective object gates shared by every Flow REST read path.
//!
//! `ADR-0012` §3 makes workspace membership and `flow_enabled` necessary but no longer sufficient:
//! each object is evaluated through its grant/inheritance chain. The cache used here is bound to
//! the current `authz_epoch`; cache misses are batchable, database-authoritative evaluations.

use axum::http::Extensions;
use platform::app::AppState;
use uuid::Uuid;

use crate::error::ApiError;
use crate::middleware::bot_auth::require_workspace_access;

use super::repository;
use super::{
    collab,
    collab::authz,
    collab::permission_cache::{PermissionCache, PrincipalKind},
};

use authz::PermissionLevel;

/// A read is evaluated from scratch at most this many times when authorization changes under it.
/// The bound prevents active membership churn from becoming an unbounded request.
pub(crate) const AUTHORIZATION_READ_ATTEMPTS: usize = 3;

/// One read request's authenticated principal and epoch snapshot.
pub struct FlowReadContext {
    workspace_id: Uuid,
    actor_id: Uuid,
    role: String,
    principal_kind: PrincipalKind,
    authz_epoch: i64,
}

/// Proof that [`require_flow_object_access`] authorized one object at `view` or above.
pub struct AuthorizedFlowObject {
    object_id: Uuid,
    context: FlowReadContext,
}

impl AuthorizedFlowObject {
    #[must_use]
    pub const fn object_id(&self) -> Uuid {
        self.object_id
    }

    #[must_use]
    pub const fn workspace_id(&self) -> Uuid {
        self.context.workspace_id
    }

    #[must_use]
    pub(super) const fn context(&self) -> &FlowReadContext {
        &self.context
    }
}

impl FlowReadContext {
    #[must_use]
    pub const fn workspace_id(&self) -> Uuid {
        self.workspace_id
    }

    #[must_use]
    pub const fn is_bot(&self) -> bool {
        matches!(self.principal_kind, PrincipalKind::Bot)
    }

    #[must_use]
    pub(super) const fn actor_id(&self) -> Uuid {
        self.actor_id
    }

    #[must_use]
    pub(super) const fn principal_kind(&self) -> PrincipalKind {
        self.principal_kind
    }

    #[must_use]
    pub(super) fn is_human_admin(&self) -> bool {
        self.principal_kind == PrincipalKind::User && matches!(self.role.as_str(), "owner" | "admin")
    }
}

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

/// Starts a list/read request with workspace membership, feature flag, and a current epoch.
pub async fn begin_flow_read(
    state: &AppState,
    extensions: &Extensions,
    workspace_id: Uuid,
) -> Result<FlowReadContext, ApiError> {
    let (actor_id, role, is_bot) = require_flow_workspace_access(state, extensions, workspace_id).await?;
    let authz_epoch = authz::read_epoch(&state.db, workspace_id).await?;
    Ok(FlowReadContext {
        workspace_id,
        actor_id,
        role,
        principal_kind: if is_bot {
            PrincipalKind::Bot
        } else {
            PrincipalKind::User
        },
        authz_epoch,
    })
}

/// Object-level read entry: workspace gate, epoch-checked cache, then DB authority on a miss.
///
/// Membership denial and insufficient effective permission both collapse to `not_found`, matching
/// the absent/cross-workspace answer and preventing this object-id-only route from becoming an
/// existence oracle. Feature-disabled and unauthenticated retain their existing semantics.
pub async fn require_flow_object_access(
    state: &AppState,
    extensions: &Extensions,
    workspace_id: Uuid,
    object_id: Uuid,
    minimum: PermissionLevel,
) -> Result<Option<AuthorizedFlowObject>, ApiError> {
    let actor = require_workspace_access(state, extensions, workspace_id)
        .await
        .map_err(collapse_object_denial)?;
    require_flow_enabled(state, workspace_id).await?;
    let context = FlowReadContext {
        workspace_id,
        actor_id: actor.0,
        role: actor.1,
        principal_kind: if actor.2 {
            PrincipalKind::Bot
        } else {
            PrincipalKind::User
        },
        authz_epoch: authz::read_epoch(&state.db, workspace_id).await?,
    };
    let Some(visible) = authorize_flow_objects(state, &context, &[object_id], minimum).await? else {
        return Ok(None);
    };
    if visible.first().copied() != Some(true) {
        return Err(object_not_found());
    }
    Ok(Some(AuthorizedFlowObject { object_id, context }))
}

/// Batch-authorizes object ids against one request epoch, preserving input order.
///
/// Cache hits are accepted only at `context.authz_epoch`. All misses are resolved together by
/// [`authz::effective_permissions`], then the epoch is read again before anything is cached or
/// returned. Epoch movement returns an internal retry signal so the handler can restart the whole
/// read; no mixed-epoch result is returned.
pub async fn authorize_flow_objects(
    state: &AppState,
    context: &FlowReadContext,
    object_ids: &[Uuid],
    minimum: PermissionLevel,
) -> Result<Option<Vec<bool>>, ApiError> {
    if !ensure_epoch_current(state, context).await? {
        return Ok(None);
    }
    let cache = PermissionCache::for_state(state)?;
    let mut levels = vec![None; object_ids.len()];
    let mut misses = Vec::new();
    let mut miss_positions = Vec::new();
    for (index, object_id) in object_ids.iter().copied().enumerate() {
        if let Some(level) = cache.get(
            context.workspace_id,
            context.principal_kind,
            context.actor_id,
            object_id,
            context.authz_epoch,
        ) {
            if let Some(slot) = levels.get_mut(index) {
                *slot = Some(level);
            }
        } else {
            misses.push(object_id);
            miss_positions.push(index);
        }
    }

    if !misses.is_empty() {
        let resolved = authz::effective_permissions(
            &state.db,
            context.workspace_id,
            &misses,
            context.principal_kind.as_str(),
            context.actor_id,
            &context.role,
        )
        .await
        .map_err(collapse_object_denial)?;
        if !ensure_epoch_current(state, context).await? {
            return Ok(None);
        }
        for ((object_id, level), index) in resolved.into_iter().zip(miss_positions) {
            collab::cache_db_permission(
                &cache,
                context.workspace_id,
                context.principal_kind,
                context.actor_id,
                object_id,
                level,
                context.authz_epoch,
            );
            if let Some(slot) = levels.get_mut(index) {
                *slot = Some(level);
            }
        }
    }

    levels
        .into_iter()
        .map(|level| level.map(|level| level >= minimum).ok_or(ApiError::Internal))
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

/// Reports whether a read still belongs to its original epoch.
///
/// `false` is an internal retry signal, not a caller-visible authorization denial; the handler
/// re-runs membership, feature, authorization, and data reads from scratch before returning
/// anything.
pub async fn ensure_epoch_current(state: &AppState, context: &FlowReadContext) -> Result<bool, ApiError> {
    #[cfg(test)]
    inject_epoch_change_if_planned(state, context.workspace_id).await?;
    let current = authz::read_epoch(&state.db, context.workspace_id).await?;
    Ok(current == context.authz_epoch)
}

/// No frozen error code describes authorization churn after the bounded retries are exhausted.
/// Keep the pre-existing fail-closed `Forbidden` wire behavior until the contract adds one;
/// callers must not branch on this message.
pub(crate) fn authorization_read_unstable() -> ApiError {
    ApiError::Forbidden("authorization changed repeatedly while the read was being evaluated".to_string())
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct EpochChangePlan {
    checks_before_change: usize,
    remaining_changes: usize,
}

#[cfg(test)]
fn epoch_change_plans() -> &'static parking_lot::Mutex<std::collections::HashMap<Uuid, EpochChangePlan>> {
    static PLANS: std::sync::OnceLock<parking_lot::Mutex<std::collections::HashMap<Uuid, EpochChangePlan>>> =
        std::sync::OnceLock::new();
    PLANS.get_or_init(|| parking_lot::Mutex::new(std::collections::HashMap::new()))
}

/// Installs a workspace-scoped, deterministic race for real-database route tests. Production
/// builds contain neither the registry nor this injection seam.
#[cfg(test)]
pub(crate) fn plan_epoch_changes_for_test(workspace_id: Uuid, checks_before_change: usize, changes: usize) {
    epoch_change_plans().lock().insert(
        workspace_id,
        EpochChangePlan {
            checks_before_change,
            remaining_changes: changes,
        },
    );
}

#[cfg(test)]
async fn inject_epoch_change_if_planned(state: &AppState, workspace_id: Uuid) -> Result<(), ApiError> {
    let should_change = {
        let mut plans = epoch_change_plans().lock();
        let Some(plan) = plans.get_mut(&workspace_id) else {
            return Ok(());
        };
        if plan.checks_before_change > 0 {
            plan.checks_before_change -= 1;
            false
        } else if plan.remaining_changes > 0 {
            plan.remaining_changes -= 1;
            let finished = plan.remaining_changes == 0;
            if finished {
                plans.remove(&workspace_id);
            }
            true
        } else {
            plans.remove(&workspace_id);
            false
        }
    };
    if should_change {
        authz::advance_epoch(&state.db, workspace_id).await?;
    }
    Ok(())
}

pub(crate) fn collapse_object_denial(err: ApiError) -> ApiError {
    match err {
        ApiError::Forbidden(_) | ApiError::NotFound(_) => object_not_found(),
        other => other,
    }
}

pub(crate) fn object_not_found() -> ApiError {
    ApiError::NotFound("flow object not found".to_string())
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
