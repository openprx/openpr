//! Post-commit, single-instance authorization revocation for active collaboration sessions.
//!
//! This is a timeliness path, not the correctness barrier: content writes remain protected by
//! `authz_epoch` fencing in [`super::write`]. Every producer calls this only after its mutation
//! commits. Database/re-evaluation failure is fail closed for the affected live candidates, while
//! the producer still returns its already-committed success.

use std::collections::{HashMap, HashSet};

use platform::app::AppState;
use sea_orm::{DbBackend, FromQueryResult, Statement};
use uuid::Uuid;

use super::authz::{self, PermissionLevel};
use super::registry::{ActiveSession, SessionRegistry};
use super::{COLLAB_SESSION_PRINCIPAL_KIND, MINIMUM_COLLAB_SESSION_LEVEL};
use crate::error::ApiErrorKind;
use crate::flow::repository;

const AUTHORIZATION_CLOSE_CODE: u16 = match ApiErrorKind::PolicyRejected.ws_close_code() {
    Some(code) => code,
    None => 1008,
};
const FEATURE_DISABLED_CLOSE_CODE: u16 = match ApiErrorKind::FeatureDisabled.ws_close_code() {
    Some(code) => code,
    None => 1008,
};
const AUTHORIZATION_CLOSE_REASON: &str = "authorization revoked";
const FEATURE_DISABLED_CLOSE_REASON: &str = "feature disabled";
const SUBTREE_DEPTH_PROBE: i64 = 33;

pub(super) fn permission_requires_revocation(level: PermissionLevel) -> bool {
    level < MINIMUM_COLLAB_SESSION_LEVEL
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RevocationStats {
    pub candidates: usize,
    pub revoked: usize,
    pub presence_removed: usize,
    pub disconnected: usize,
}

#[derive(FromQueryResult)]
struct RoleRow {
    user_id: Uuid,
    role: String,
}

async fn current_roles(
    state: &AppState,
    workspace_id: Uuid,
    user_ids: &[Uuid],
) -> Result<HashMap<Uuid, String>, sea_orm::DbErr> {
    if user_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = RoleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT user_id, role FROM workspace_members WHERE workspace_id = $1 AND user_id = ANY($2)",
        vec![workspace_id.into(), user_ids.to_vec().into()],
    ))
    .all(&state.db)
    .await?;
    Ok(rows.into_iter().map(|row| (row.user_id, row.role)).collect())
}

fn apply_revocations(registry: &SessionRegistry, candidates: usize, revoked: &[ActiveSession]) -> RevocationStats {
    let presence_removed = registry.remove_presence_for_sessions(revoked);
    let disconnected =
        registry.disconnect_authorization_sessions(revoked, AUTHORIZATION_CLOSE_CODE, AUTHORIZATION_CLOSE_REASON);
    RevocationStats {
        candidates,
        revoked: revoked.len(),
        presence_removed,
        disconnected,
    }
}

async fn revalidate_candidates(
    state: &AppState,
    registry: &SessionRegistry,
    workspace_id: Uuid,
    candidates: Vec<ActiveSession>,
) -> RevocationStats {
    let candidate_count = candidates.len();
    if candidates.is_empty() {
        return RevocationStats::default();
    }

    let mut by_user: HashMap<Uuid, Vec<ActiveSession>> = HashMap::new();
    for candidate in candidates {
        by_user.entry(candidate.user_id).or_default().push(candidate);
    }
    let user_ids: Vec<Uuid> = by_user.keys().copied().collect();
    let roles = match current_roles(state, workspace_id, &user_ids).await {
        Ok(roles) => roles,
        Err(err) => {
            tracing::warn!(%workspace_id, %err, "session revocation role lookup failed; closing all affected candidates");
            let revoked: Vec<ActiveSession> = by_user.into_values().flatten().collect();
            return apply_revocations(registry, candidate_count, &revoked);
        }
    };

    let mut revoked = Vec::new();
    for (user_id, user_sessions) in by_user {
        let Some(role) = roles.get(&user_id) else {
            revoked.extend(user_sessions);
            continue;
        };
        let mut object_ids: Vec<Uuid> = user_sessions.iter().map(|session| session.object_id).collect();
        object_ids.sort_unstable();
        object_ids.dedup();
        let levels = match authz::effective_permissions(
            &state.db,
            workspace_id,
            &object_ids,
            COLLAB_SESSION_PRINCIPAL_KIND,
            user_id,
            role,
        )
        .await
        {
            Ok(levels) => levels.into_iter().collect::<HashMap<_, _>>(),
            Err(err) => {
                tracing::warn!(%workspace_id, %user_id, ?err, "session permission re-evaluation failed; closing this principal's candidates");
                revoked.extend(user_sessions);
                continue;
            }
        };
        revoked.extend(user_sessions.into_iter().filter(|session| {
            levels
                .get(&session.object_id)
                .is_none_or(|level| permission_requires_revocation(*level))
        }));
    }
    apply_revocations(registry, candidate_count, &revoked)
}

/// Re-evaluates every live session in `root_object_id`'s current subtree after a committed grant,
/// inheritance, or parent change.
///
/// The downward walk is the shared recursive CTE in [`repository::subtree_nodes`].
pub async fn revalidate_subtree_after_commit(
    state: &AppState,
    workspace_id: Uuid,
    root_object_id: Uuid,
    committed_epoch: i64,
) -> RevocationStats {
    let registry = &super::runtime::runtime().registry;
    revalidate_subtree_with_registry(state, registry, workspace_id, root_object_id, committed_epoch).await
}

/// Explicit-registry form used by `move_object`, whose tests and lock orchestration deliberately
/// inject a `CollabRuntime` instead of using the process singleton.
pub(crate) async fn revalidate_subtree_with_registry(
    state: &AppState,
    registry: &SessionRegistry,
    workspace_id: Uuid,
    root_object_id: Uuid,
    committed_epoch: i64,
) -> RevocationStats {
    let subtree = match repository::subtree_nodes(
        &state.db,
        workspace_id,
        root_object_id,
        SUBTREE_DEPTH_PROBE,
        i64::MAX,
    )
    .await
    {
        Ok(subtree) => subtree,
        Err(err) => {
            tracing::warn!(%workspace_id, %root_object_id, ?err, "subtree session lookup failed; closing all workspace candidates fail-closed");
            let candidates = registry.observe_epoch_and_workspace_sessions(workspace_id, committed_epoch);
            return apply_revocations(registry, candidates.len(), &candidates);
        }
    };
    let object_ids: HashSet<Uuid> = subtree.ids.into_iter().collect();
    let candidates = registry.observe_epoch_and_subtree_sessions(workspace_id, committed_epoch, &object_ids);
    revalidate_candidates(state, registry, workspace_id, candidates).await
}

/// Re-evaluates all local sessions after membership or workspace-baseline changes.
pub async fn revalidate_workspace_after_commit(
    state: &AppState,
    workspace_id: Uuid,
    committed_epoch: i64,
) -> RevocationStats {
    let registry = &super::runtime::runtime().registry;
    let candidates = registry.observe_epoch_and_workspace_sessions(workspace_id, committed_epoch);
    revalidate_candidates(state, registry, workspace_id, candidates).await
}

/// A disabled Flow feature admits no session at any permission grade, so no per-object evaluator
/// is needed. Presence still goes first and removal from fan-out still precedes best-effort close.
pub fn disconnect_workspace_for_disabled_feature(workspace_id: Uuid, committed_epoch: i64) -> RevocationStats {
    let registry = &super::runtime::runtime().registry;
    let candidates = registry.observe_epoch_and_workspace_sessions(workspace_id, committed_epoch);
    let presence_removed = registry.remove_presence_for_sessions(&candidates);
    let disconnected = registry.disconnect_authorization_sessions(
        &candidates,
        FEATURE_DISABLED_CLOSE_CODE,
        FEATURE_DISABLED_CLOSE_REASON,
    );
    RevocationStats {
        candidates: candidates.len(),
        revoked: candidates.len(),
        presence_removed,
        disconnected,
    }
}
