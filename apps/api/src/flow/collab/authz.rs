//! `ADR-0012` §3 effective-permission computation and §3.1 commit-time `authz_epoch` fencing.
//!
//! v0.4 ships no application surface that writes `flow_object_grants` / flips
//! `inherit_from_parent` / changes `default_member_level` (that is `ADR-0012`'s v0.5 authorization
//! surface — the migration comment on `flow_object_grants` calls it out explicitly as reserved).
//! This module still has a real job in v0.4: the collab WebSocket layer must (a) compute the
//! *read* side of the same effective-permission rule so ticket issuance and `open` gate on it
//! today, using whatever `flow_object_grants` rows/`inherit_from_parent` flags a v0.5 admin
//! surface — or a direct SQL seed in a test — puts there, and (b) enforce the commit-time fencing
//! barrier so that whenever *something* advances `flow_workspace_settings.authz_epoch` (the v0.5
//! `grant/membership/parent_id` write path, someday), an in-flight content write cannot straddle
//! that change and land after the revocation. [`advance_epoch_for_test`] is the minimal internal
//! primitive that stands in for "an authorization-changing transaction" in this package: it is not
//! reachable from any route, but it is the exact same one-line `UPDATE ... RETURNING authz_epoch`
//! v0.5's grant-revoke endpoint will call, so exercising it here is not exercising a fake.

#![allow(clippy::items_after_statements, clippy::too_long_first_doc_paragraph)]

use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use uuid::Uuid;

use crate::error::ApiError;

/// `ADR-0012` §2's totally ordered permission grades. Declaration order is ascending, so
/// `#[derive(Ord)]` gives exactly the contract's `view < comment < edit < full_access`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PermissionLevel {
    View,
    Comment,
    Edit,
    FullAccess,
}

impl PermissionLevel {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "view" => Some(Self::View),
            "comment" => Some(Self::Comment),
            "edit" => Some(Self::Edit),
            "full_access" => Some(Self::FullAccess),
            _ => None,
        }
    }
}

/// Bounds the ancestor walk. Matches `ADR-0012` §3's frozen `tree_depth_max` (32): a chain longer
/// than that would mean `flow_objects.parent_id` has a cycle or the schema's depth invariant was
/// otherwise violated, neither of which this read path should loop forever trying to honor.
const MAX_CHAIN_HOPS: usize = 32;

struct ChainNode {
    id: Uuid,
    inherit_from_parent: bool,
}

/// Walks `object_id`'s ancestor chain via `parent_id`, starting at the object itself, in that
/// order, bounded to [`MAX_CHAIN_HOPS`] hops.
async fn fetch_chain<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    object_id: Uuid,
) -> Result<Vec<ChainNode>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        id: Uuid,
        parent_id: Option<Uuid>,
        inherit_from_parent: bool,
    }

    let mut chain = Vec::new();
    let mut current = Some(object_id);
    let mut hops = 0usize;
    while let Some(node_id) = current {
        if hops > MAX_CHAIN_HOPS {
            break;
        }
        hops += 1;
        let row = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, parent_id, inherit_from_parent FROM flow_objects WHERE id = $1 AND workspace_id = $2",
            vec![node_id.into(), workspace_id.into()],
        ))
        .one(conn)
        .await?;
        let Some(row) = row else { break };
        current = row.parent_id;
        chain.push(ChainNode {
            id: row.id,
            inherit_from_parent: row.inherit_from_parent,
        });
    }
    Ok(chain)
}

/// Explicit grants for one principal across a set of object ids, keyed by `object_id`.
async fn fetch_grants<C: ConnectionTrait>(
    conn: &C,
    object_ids: &[Uuid],
    principal_kind: &str,
    principal_id: Uuid,
) -> Result<Vec<(Uuid, PermissionLevel)>, ApiError> {
    if object_ids.is_empty() {
        return Ok(Vec::new());
    }
    #[derive(FromQueryResult)]
    struct Row {
        object_id: Uuid,
        level: String,
    }
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT object_id, level FROM flow_object_grants \
         WHERE object_id = ANY($1) AND principal_kind = $2 AND principal_id = $3",
        vec![object_ids.to_vec().into(), principal_kind.into(), principal_id.into()],
    ))
    .all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| PermissionLevel::parse(&row.level).map(|level| (row.object_id, level)))
        .collect())
}

/// The workspace baseline (`ADR-0012` §3: admin ⇒ `full_access`; member ⇒
/// `default_member_level`, frozen at `edit` in v0.4).
async fn workspace_baseline<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    role: &str,
) -> Result<PermissionLevel, ApiError> {
    if role == "owner" || role == "admin" {
        return Ok(PermissionLevel::FullAccess);
    }
    #[derive(FromQueryResult)]
    struct Row {
        default_member_level: String,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT default_member_level FROM flow_workspace_settings WHERE workspace_id = $1",
        vec![workspace_id.into()],
    ))
    .one(conn)
    .await?;
    Ok(row
        .and_then(|r| PermissionLevel::parse(&r.default_member_level))
        .unwrap_or(PermissionLevel::View))
}

/// `ADR-0012` §3's effective-permission rule.
///
/// `principal_kind` is `"user"` or `"bot"` (matches `flow_object_grants.principal_kind`); `role`
/// is the caller's `workspace_members.role` (`"owner"`/`"admin"`/`"member"`/...), used only for
/// the workspace-admin override and the workspace baseline.
///
/// # Errors
/// Propagates a database read failure.
pub async fn effective_permission<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    object_id: Uuid,
    principal_kind: &str,
    principal_id: Uuid,
    role: &str,
) -> Result<PermissionLevel, ApiError> {
    // Workspace admin always keeps `full_access` regardless of any authorization boundary
    // (`ADR-0012` §3: "workspace admin 永远保留 full_access（可审计的管理员兜底）").
    if role == "owner" || role == "admin" {
        return Ok(PermissionLevel::FullAccess);
    }

    let chain = fetch_chain(conn, workspace_id, object_id).await?;
    let chain_ids: Vec<Uuid> = chain.iter().map(|n| n.id).collect();
    let grants = fetch_grants(conn, &chain_ids, principal_kind, principal_id).await?;
    let grant_at = |id: Uuid| grants.iter().find(|(gid, _)| *gid == id).map(|(_, level)| *level);

    // Walk from the object upward; the first node with `inherit_from_parent = false` is the
    // authorization boundary (`ADR-0012` §3: "遇到第一个 inherit_from_parent=false 的节点即停").
    let mut boundary_index = None;
    for (index, node) in chain.iter().enumerate() {
        if !node.inherit_from_parent {
            boundary_index = Some(index);
            break;
        }
    }

    let best_grant_in =
        |nodes: &[ChainNode]| -> Option<PermissionLevel> { nodes.iter().filter_map(|node| grant_at(node.id)).max() };

    if let Some(index) = boundary_index {
        // Boundary present: only grants at/under the boundary count; workspace baseline does
        // not apply (`ADR-0012` §3: "workspace 基线不再适用"). `chain[..=index]` is exactly
        // "the boundary node and everything below it" since `chain[0]` is the object itself.
        let Some(bounded) = chain.get(..=index) else {
            return Ok(PermissionLevel::View);
        };
        Ok(best_grant_in(bounded).unwrap_or(PermissionLevel::View))
    } else {
        // No boundary anywhere up to the root: effective = max(all chain grants, baseline).
        let baseline = workspace_baseline(conn, workspace_id, role).await?;
        Ok(best_grant_in(&chain).map_or(baseline, |g| g.max(baseline)))
    }
}

/// The `flow_workspace_settings.authz_epoch` row's current value, read outside any lock (used
/// when a caller needs "the epoch effective permission was computed against", e.g. before opening
/// a collab session or before starting an isolated apply).
///
/// # Errors
/// `NotFound` if the workspace has no `flow_workspace_settings` row (mirrors
/// [`super::super::policy::require_flow_enabled`]'s fail-closed treatment of a missing row).
pub async fn read_epoch<C: ConnectionTrait>(conn: &C, workspace_id: Uuid) -> Result<i64, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        authz_epoch: i64,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT authz_epoch FROM flow_workspace_settings WHERE workspace_id = $1",
        vec![workspace_id.into()],
    ))
    .one(conn)
    .await?;
    row.map(|r| r.authz_epoch)
        .ok_or_else(|| ApiError::NotFound("flow workspace settings not found".to_string()))
}

/// The commit-time fencing barrier (`ADR-0012` §3.1, `collab-protocol-v1.md` §"鉴权与连接" point
/// 7): takes `SELECT ... FOR SHARE` on the workspace's `authz_epoch` row — held to the caller's
/// commit — and rejects if the epoch it reads is not the `checked_epoch` the caller computed
/// permission against outside the transaction.
///
/// This is not a pre-insert check: the `FOR SHARE` blocks until any concurrent authorization
/// change (which takes `FOR UPDATE` on the same row, see [`advance_epoch_for_test`]) either
/// commits or rolls back, so the two can never interleave. If an authorization change commits
/// first, this call observes the *new* epoch once unblocked and rejects rather than silently
/// proceeding on stale permission — closing exactly the window `collab-protocol-v1.md` names:
/// "A 重验 epoch=E → B 提交 E+1 并撤权 → A 插入 update 并在 B 之后 commit"。
///
/// Must be called inside the same transaction that will insert the write it is fencing, and the
/// transaction must not commit if this returns `Err`.
///
/// # Errors
/// `Conflict` when the locked epoch does not match `checked_epoch` (the caller must roll back and
/// treat this as `policy_rejected` on the wire, not retry the same transaction). Propagates a
/// database read failure otherwise.
pub async fn fence_epoch_for_share<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    checked_epoch: i64,
) -> Result<(), ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        authz_epoch: i64,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT authz_epoch FROM flow_workspace_settings WHERE workspace_id = $1 FOR SHARE",
        vec![workspace_id.into()],
    ))
    .one(conn)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::NotFound("flow workspace settings not found".to_string()));
    };
    if row.authz_epoch != checked_epoch {
        return Err(ApiError::Conflict(format!(
            "authz_epoch advanced from {checked_epoch} to {}; permission must be rechecked",
            row.authz_epoch
        )));
    }
    Ok(())
}

/// Advances `authz_epoch` by one inside the caller's transaction, via a plain `UPDATE` (which
/// takes the same row-exclusive lock `FOR UPDATE` would — `ADR-0012` §3.1: "授权类变更... 取冲突锁
/// (FOR UPDATE) 推进 epoch").
///
/// Not reachable from any route in this package: v0.4 ships no grant/inheritance/membership write
/// path (`ADR-0012` marks `flow_object_grants` reserved for v0.5). This exists so the fencing
/// barrier above has something real to test against — it is the same one-line primitive a v0.5
/// grant-revoke handler will call, not a stand-in that only exists in test code.
///
/// # Errors
/// `NotFound` if the workspace has no `flow_workspace_settings` row. Propagates a database write
/// failure otherwise.
pub async fn advance_epoch_for_test<C: ConnectionTrait>(conn: &C, workspace_id: Uuid) -> Result<i64, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        authz_epoch: i64,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_workspace_settings SET authz_epoch = authz_epoch + 1, updated_at = now() \
         WHERE workspace_id = $1 RETURNING authz_epoch",
        vec![workspace_id.into()],
    ))
    .one(conn)
    .await?;
    row.map(|r| r.authz_epoch)
        .ok_or_else(|| ApiError::NotFound("flow workspace settings not found".to_string()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::PermissionLevel;

    #[test]
    fn permission_levels_are_totally_ordered_per_adr_0012() {
        assert!(PermissionLevel::View < PermissionLevel::Comment);
        assert!(PermissionLevel::Comment < PermissionLevel::Edit);
        assert!(PermissionLevel::Edit < PermissionLevel::FullAccess);
    }

    #[test]
    fn parse_round_trips_every_registered_level() {
        for raw in ["view", "comment", "edit", "full_access"] {
            assert!(PermissionLevel::parse(raw).is_some(), "{raw} must parse");
        }
        assert!(PermissionLevel::parse("owner").is_none());
    }
}
