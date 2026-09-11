//! Post-commit, single-instance authorization revocation for active collaboration sessions.
//!
//! This is a timeliness path, not the correctness barrier: content writes remain protected by
//! `authz_epoch` fencing in [`super::write`]. Every producer calls this only after its mutation
//! commits. Confirmed revocation fails closed; an indeterminate database result uses the existing
//! recoverable drain handshake so it is never mislabeled as permanent 4403. The producer still
//! returns its already-committed success.

use std::collections::{HashMap, HashSet};

use platform::app::AppState;
use sea_orm::{DbBackend, FromQueryResult, Statement};
use uuid::Uuid;

use super::authz::{self, PermissionLevel};
use super::limits::CONNECTION_LIMIT_RETRY_AFTER_MS;
use super::registry::{ActiveSession, SessionRegistry};
use super::{COLLAB_SESSION_PRINCIPAL_KIND, MINIMUM_COLLAB_SESSION_LEVEL};
use crate::error::ApiErrorKind;

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
const AUTHORIZATION_RECHECK_ATTEMPTS: usize = 3;

pub(super) fn permission_requires_revocation(level: PermissionLevel) -> bool {
    level < MINIMUM_COLLAB_SESSION_LEVEL
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RevocationStats {
    pub candidates: usize,
    pub revoked: usize,
    pub presence_removed: usize,
    pub disconnected: usize,
    /// Sessions whose authorization could not be determined and were closed with the existing
    /// recoverable `server_draining{reason:"drain"}` / 4410 handshake instead of false 4403.
    pub retryable_drained: usize,
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

async fn current_roles_with_retry(
    state: &AppState,
    workspace_id: Uuid,
    user_ids: &[Uuid],
) -> Result<HashMap<Uuid, String>, sea_orm::DbErr> {
    let mut attempt = 1usize;
    loop {
        match current_roles(state, workspace_id, user_ids).await {
            Ok(roles) => return Ok(roles),
            Err(err) if attempt >= AUTHORIZATION_RECHECK_ATTEMPTS => return Err(err),
            Err(err) => {
                tracing::warn!(
                    %workspace_id,
                    %err,
                    attempt,
                    max_attempts = AUTHORIZATION_RECHECK_ATTEMPTS,
                    "session revocation role lookup failed; retrying"
                );
                attempt += 1;
            }
        }
    }
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
        retryable_drained: 0,
    }
}

fn partition_user_sessions(
    sessions: Vec<ActiveSession>,
    levels: &HashMap<Uuid, PermissionLevel>,
    uncertain_objects: &HashSet<Uuid>,
    revoked: &mut Vec<ActiveSession>,
    uncertain: &mut Vec<ActiveSession>,
) {
    for session in sessions {
        if uncertain_objects.contains(&session.object_id) || !levels.contains_key(&session.object_id) {
            uncertain.push(session);
        } else if levels
            .get(&session.object_id)
            .is_some_and(|level| permission_requires_revocation(*level))
        {
            revoked.push(session);
        }
    }
}

async fn levels_with_object_fallback(
    state: &AppState,
    workspace_id: Uuid,
    user_id: Uuid,
    role: &str,
    object_ids: &[Uuid],
) -> (HashMap<Uuid, PermissionLevel>, HashSet<Uuid>) {
    match authz::effective_permissions(
        &state.db,
        workspace_id,
        object_ids,
        COLLAB_SESSION_PRINCIPAL_KIND,
        user_id,
        role,
    )
    .await
    {
        Ok(levels) => (levels.into_iter().collect(), HashSet::new()),
        Err(batch_err) => {
            tracing::warn!(
                %workspace_id,
                %user_id,
                ?batch_err,
                "session permission batch re-evaluation failed; retrying per object"
            );
            let mut levels = HashMap::new();
            let mut uncertain = HashSet::new();
            for object_id in object_ids {
                match authz::effective_permissions(
                    &state.db,
                    workspace_id,
                    std::slice::from_ref(object_id),
                    COLLAB_SESSION_PRINCIPAL_KIND,
                    user_id,
                    role,
                )
                .await
                {
                    Ok(object_levels) => {
                        if let Some((_, level)) = object_levels.into_iter().find(|(id, _)| id == object_id) {
                            levels.insert(*object_id, level);
                        } else {
                            uncertain.insert(*object_id);
                        }
                    }
                    Err(err) => {
                        tracing::warn!(
                            %workspace_id,
                            %user_id,
                            %object_id,
                            ?err,
                            "session permission re-evaluation remained indeterminate for one object"
                        );
                        uncertain.insert(*object_id);
                    }
                }
            }
            (levels, uncertain)
        }
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
    let roles = match current_roles_with_retry(state, workspace_id, &user_ids).await {
        Ok(roles) => roles,
        Err(err) => {
            tracing::warn!(%workspace_id, %err, "session revocation role lookup remained indeterminate; draining candidates for reauthorization");
            let uncertain: Vec<ActiveSession> = by_user.into_values().flatten().collect();
            return RevocationStats {
                candidates: candidate_count,
                retryable_drained: registry.drain_sessions(&uncertain, CONNECTION_LIMIT_RETRY_AFTER_MS),
                ..RevocationStats::default()
            };
        }
    };

    let mut revoked = Vec::new();
    let mut uncertain = Vec::new();
    for (user_id, user_sessions) in by_user {
        let Some(role) = roles.get(&user_id) else {
            revoked.extend(user_sessions);
            continue;
        };
        let mut object_ids: Vec<Uuid> = user_sessions.iter().map(|session| session.object_id).collect();
        object_ids.sort_unstable();
        object_ids.dedup();
        let (levels, uncertain_objects) =
            levels_with_object_fallback(state, workspace_id, user_id, role, &object_ids).await;
        partition_user_sessions(user_sessions, &levels, &uncertain_objects, &mut revoked, &mut uncertain);
    }
    let mut result = apply_revocations(registry, candidate_count, &revoked);
    result.retryable_drained = registry.drain_sessions(&uncertain, CONNECTION_LIMIT_RETRY_AFTER_MS);
    result
}

/// Re-evaluates the workspace's bounded live-session set after a committed grant, inheritance, or
/// parent change.
///
/// Candidate count is capped by `connections_per_workspace_max`; evaluating this
/// bounded set removes the former unbounded subtree query and still closes only sessions whose
/// current object permission is confirmed below the collab threshold.
pub async fn revalidate_authorization_change_after_commit(
    state: &AppState,
    workspace_id: Uuid,
    committed_epoch: i64,
) -> RevocationStats {
    let registry = &super::runtime::runtime().registry;
    revalidate_authorization_change_with_registry(state, registry, workspace_id, committed_epoch).await
}

/// Explicit-registry form used by `move_object`, whose tests and lock orchestration deliberately
/// inject a `CollabRuntime` instead of using the process singleton.
pub(crate) async fn revalidate_authorization_change_with_registry(
    state: &AppState,
    registry: &SessionRegistry,
    workspace_id: Uuid,
    committed_epoch: i64,
) -> RevocationStats {
    let candidates = registry.observe_epoch_and_workspace_sessions(workspace_id, committed_epoch);
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
        retryable_drained: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(object_id: Uuid) -> ActiveSession {
        ActiveSession {
            session_id: Uuid::new_v4(),
            document_id: Uuid::new_v4(),
            object_id,
            user_id: Uuid::new_v4(),
        }
    }

    #[test]
    fn indeterminate_object_is_separate_from_revoked_and_authorized_sessions() {
        let revoked_object = Uuid::new_v4();
        let uncertain_object = Uuid::new_v4();
        let authorized_object = Uuid::new_v4();
        let sessions = vec![
            session(revoked_object),
            session(uncertain_object),
            session(authorized_object),
        ];
        let levels = HashMap::from([
            (revoked_object, PermissionLevel::Denied),
            (authorized_object, MINIMUM_COLLAB_SESSION_LEVEL),
        ]);
        let uncertain_objects = HashSet::from([uncertain_object]);
        let mut revoked = Vec::new();
        let mut uncertain = Vec::new();

        partition_user_sessions(sessions, &levels, &uncertain_objects, &mut revoked, &mut uncertain);

        assert_eq!(revoked.len(), 1);
        assert_eq!(
            revoked.first().map(|candidate| candidate.object_id),
            Some(revoked_object)
        );
        assert_eq!(uncertain.len(), 1);
        assert_eq!(
            uncertain.first().map(|candidate| candidate.object_id),
            Some(uncertain_object)
        );
    }
}
