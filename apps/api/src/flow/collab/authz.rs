//! `ADR-0012` §3 effective-permission computation and §3.1 commit-time `authz_epoch` fencing.
//!
//! The v0.5 authorization surface writes `flow_object_grants`, inheritance, workspace baselines,
//! and workspace membership. This module supplies both their effective-permission evaluator and
//! their commit-time epoch fencing primitives. The collab WebSocket layer must (a) compute the
//! *read* side of the same effective-permission rule so ticket issuance and `open` gate on it
//! today, using whatever `flow_object_grants` rows/`inherit_from_parent` flags a v0.5 admin
//! surface — or a direct SQL seed in a test — puts there, and (b) enforce the commit-time fencing
//! barrier so that whenever *something* advances `flow_workspace_settings.authz_epoch` (the v0.5
//! `grant/membership/parent_id` write path), an in-flight content write cannot straddle that change
//! and land after the revocation. [`advance_epoch`] is the strict primitive for Flow-native
//! authorization changes, while [`advance_epoch_if_present`] lets general workspace-member routes
//! participate without creating Flow settings for workspaces that have never enabled Flow.

#![allow(clippy::items_after_statements, clippy::too_long_first_doc_paragraph)]

use std::collections::{HashMap, HashSet};

use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use uuid::Uuid;

use crate::error::ApiError;

/// `ADR-0012` §2's totally ordered permission grades. Declaration order is ascending, so
/// `#[derive(Ord)]` gives exactly the contract's `view < comment < edit < full_access`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PermissionLevel {
    /// No access at all, and the bottom of the order.
    ///
    /// Deliberately **not** a `flow_object_grants.level` value ([`Self::parse`] never yields it,
    /// and the table's `flow_object_grants_level_check` does not list it): it is the answer to
    /// "the caller holds nothing here", which `ADR-0012` §3's boundary row asks for and the four
    /// storable grades cannot express. The boundary row reads
    /// "`max(边界节点及其以下的显式 grant)`；**workspace 基线不再适用**" — with no grant at or below
    /// the boundary that maximum is over the *empty* set, i.e. nothing, not `view`. Returning
    /// `View` there (what this module did before the v0.5 authorization surface landed) leaves a
    /// smaller copy of exactly the hole `ADR-0012` R16 was revised to close: an author who
    /// restricts a page to make it private still hands every workspace member read access, so
    /// "限制访问" only half works. The v0.5 gate text allows either answer
    /// ("边界下的成员确实降到 view/**无权**"); this picks 无权, which is also what Notion's
    /// "restrict access" does.
    Denied,
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

    /// The wire spelling used in `rest-api-v1.md`'s `permission_changes.{caller,affected}`
    /// `before_level`/`after_level` fields. [`Self::Denied`] is not a storable grade, so it
    /// renders as `"none"` rather than as one of the four `flow_object_grants.level` values.
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Denied => "none",
            Self::View => "view",
            Self::Comment => "comment",
            Self::Edit => "edit",
            Self::FullAccess => "full_access",
        }
    }

    /// Parses a caller-supplied grade for `flow_object_grants.level`. Rejects `"none"`: a
    /// principal with no access is expressed by *omitting* them from the grants list, never by
    /// storing a row that grants nothing.
    pub fn parse_grant_level(raw: &str) -> Option<Self> {
        Self::parse(raw)
    }
}

/// `limits-v1.md`'s frozen `grants_per_request_max`: the number of `grants[]` entries a single
/// `PUT /flow/objects/{object_id}/grants` (or an `inheritance` call's `initial_grants`) may carry.
pub const GRANTS_PER_REQUEST_MAX: usize = 100;

/// `limits-v1.md`'s `object_grants_max`, **frozen at 100 on 2026-08-31**. It is deliberately equal
/// to [`GRANTS_PER_REQUEST_MAX`], because both write paths replace the whole list rather than
/// merging into it: `PUT .../grants` has always done so, and `PUT .../inheritance`'s
/// `initial_grants` was ruled a replacement too (`ADR-0012` §4.1). A result therefore holds
/// exactly as many rows as the request carried, and a request may carry at most 100.
///
/// So this ceiling is **unreachable by any legal v0.5 request**, and the contract says so rather
/// than asking for a boundary case nobody can construct. The check below stays as the fail-closed
/// last line, not as a reachable branch. Introducing any *incremental* grant path — one that adds
/// to existing rows instead of replacing them — makes it reachable again, and that release owes
/// the boundary case.
pub const OBJECT_GRANTS_MAX: usize = 100;

/// `ADR-0012` §3's inheritance-chain depth limit, pinned to `limits-v1.md`'s frozen
/// `tree_depth_max`. Depth is counted the way `collab_core::limits::depth_of` counts it — a root
/// object has depth 0 and its direct child depth 1 — so this is a bound on `parent_id` *hops*,
/// not on nodes.
pub(crate) const TREE_DEPTH_MAX: usize = 32;

/// Nodes in a chain that sits exactly at [`TREE_DEPTH_MAX`]: depths `0..=32`, i.e. 33 rows joined
/// by 32 hops. `gates/gate-commands.md` requires `depth=32` — and an authorization boundary
/// landing exactly on the deepest node — to be evaluated *in full*, so this many nodes is legal
/// and must never be truncated ("不得以性能为由把鉴权深度降回 20").
pub(crate) const MAX_CHAIN_NODES: usize = TREE_DEPTH_MAX + 1;

struct ChainNode {
    id: Uuid,
    inherit_from_parent: bool,
}

/// How a `parent_id` walk that started at some object ended.
///
/// The walk itself is shared by the read path ([`fetch_chain`], which turns anything but
/// `Complete` into a denial) and the write path ([`ensure_parent_can_adopt_child`], which has to
/// tell an over-deep parent apart from a corrupted one because the two are different rejections
/// on the wire — `limit_exceeded{limit_kind:"tree_depth"}` vs `invalid_update`). Keeping one SQL
/// walk behind one enum is what stops those two sides from ever disagreeing about what "the
/// chain" is.
enum ChainWalk {
    /// Starts at the requested object and terminates at a genuine root within
    /// [`MAX_CHAIN_NODES`].
    Complete(Vec<ChainNode>),
    /// The requested object itself has no row in this workspace.
    Missing,
    /// More rows came back than [`MAX_CHAIN_NODES`] allows: the chain is deeper than
    /// `tree_depth_max`.
    TooDeep,
    /// The walk visited the same object twice: the `parent_id` edges form a cycle. Only cycles
    /// short enough to close inside the probe bound are reported here; a longer one is
    /// indistinguishable from a genuinely over-deep chain and comes back as [`Self::TooDeep`].
    /// Both are denials, so the distinction is about the error a caller sees, never about
    /// whether it is one.
    Cyclic,
    /// The walk stopped without reaching a root inside the probe bound: a `parent_id` cycle, or
    /// an ancestor row that is absent from this workspace.
    Incomplete,
}

/// Walks `object_id`'s ancestor chain via `parent_id`, starting at the object itself and ending at
/// the root, in that order, and classifies how it ended.
///
/// The probe deliberately runs one hop past the legal maximum so that an over-deep or cyclic chain
/// is *detected* rather than coming back looking like a valid short one.
async fn walk_chain<C: ConnectionTrait>(conn: &C, workspace_id: Uuid, object_id: Uuid) -> Result<ChainWalk, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        id: Uuid,
        parent_id: Option<Uuid>,
        inherit_from_parent: bool,
    }

    // One `WITH RECURSIVE` round trip for the whole chain, replacing one `SELECT` per hop. This
    // walk runs on the commit-time content-write path that `ADR-0010` holds to a 25 ms
    // `document_lock_hold_ms_p95_max` budget, where 33 serial round trips are not affordable —
    // and `gate-commands.md` names the recursive CTE as the reference implementation.
    //
    // `$3` bounds the recursion *inside* the database, so a `parent_id` cycle terminates instead
    // of spinning. It is deliberately one hop past the legal maximum: walking to depth
    // `MAX_CHAIN_NODES` yields up to `MAX_CHAIN_NODES + 1` rows, which is what lets an over-deep
    // or cyclic chain be *detected* below rather than come back looking like a valid short one.
    let probe_depth = i64::try_from(MAX_CHAIN_NODES).unwrap_or(i64::MAX);
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "WITH RECURSIVE chain AS ( \
             SELECT o.id, o.parent_id, o.inherit_from_parent, 0 AS depth \
               FROM flow_objects o \
              WHERE o.id = $1 AND o.workspace_id = $2 \
             UNION ALL \
             SELECT p.id, p.parent_id, p.inherit_from_parent, c.depth + 1 \
               FROM chain c \
               JOIN flow_objects p ON p.id = c.parent_id AND p.workspace_id = $2 \
              WHERE c.parent_id IS NOT NULL AND c.depth < $3::int \
         ) \
         SELECT id, parent_id, inherit_from_parent FROM chain ORDER BY depth",
        vec![object_id.into(), workspace_id.into(), probe_depth.into()],
    ))
    .all(conn)
    .await?;

    // `ORDER BY depth` keeps `chain[0]` the object itself with each later entry its parent —
    // `effective_permission`'s `chain.get(..=index)` ("the boundary node and everything below
    // it") depends on exactly this order.
    let Some(top) = rows.last() else {
        return Ok(ChainWalk::Missing);
    };
    // A cycle re-enters a node the walk has already visited. Checked before the length test
    // because a short cycle also *looks* over-deep: the recursion spins until the probe bound and
    // comes back with `MAX_CHAIN_NODES + 1` rows, so without this the write path would report a
    // corrupted chain as a depth-limit violation.
    let mut seen: Vec<Uuid> = Vec::with_capacity(rows.len());
    for row in &rows {
        if seen.contains(&row.id) {
            return Ok(ChainWalk::Cyclic);
        }
        seen.push(row.id);
    }
    if rows.len() > MAX_CHAIN_NODES {
        return Ok(ChainWalk::TooDeep);
    }
    if top.parent_id.is_some() {
        // The walk did not reach a root: either the recursion bound cut a longer chain or a
        // `parent_id` cycle short, or the ancestor row it names is absent from this workspace
        // (`flow_objects_parent_workspace_fk` should make the latter impossible — treat a
        // violated invariant as a denial, never as a licence to judge on a partial chain).
        return Ok(ChainWalk::Incomplete);
    }

    Ok(ChainWalk::Complete(
        rows.into_iter()
            .map(|row| ChainNode {
                id: row.id,
                inherit_from_parent: row.inherit_from_parent,
            })
            .collect(),
    ))
}

/// Walks `object_id`'s ancestor chain via `parent_id`, starting at the object itself and ending at
/// the root, in that order.
///
/// Fail-closed by construction: this returns a chain only when it is *complete* — it starts at
/// `object_id` and terminates at a genuine root (`parent_id IS NULL`) within
/// [`MAX_CHAIN_NODES`]. A truncated chain can hide the authorization boundary that
/// [`effective_permission`] exists to find, and a hidden boundary silently re-applies the
/// workspace baseline — exactly the escalation `ADR-0012` R16 was revised to remove. So a chain
/// deeper than [`TREE_DEPTH_MAX`], a `parent_id` cycle, and a chain whose named ancestor row is
/// missing from this workspace are all rejected outright rather than evaluated partially, per
/// `gates/gate-commands.md`: "`depth=33`、继承链成环、或链不完整…**必须 fail closed** 并返回可
/// 判定的拒绝，不得像 R15 之前那样把 21-32 层静默截断成别的结果".
///
/// # Errors
/// `NotFound` when `object_id` itself has no row in `workspace_id` (the caller's object simply
/// does not exist — a 404, not a broken invariant). `Forbidden` when the chain is over-deep,
/// cyclic, or incomplete. Propagates a database read failure otherwise.
async fn fetch_chain<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    object_id: Uuid,
) -> Result<Vec<ChainNode>, ApiError> {
    match walk_chain(conn, workspace_id, object_id).await? {
        ChainWalk::Complete(chain) => Ok(chain),
        ChainWalk::Missing => Err(ApiError::NotFound("flow object not found".to_string())),
        ChainWalk::TooDeep => Err(ApiError::Forbidden(
            "object inheritance chain is deeper than the frozen tree depth limit".to_string(),
        )),
        ChainWalk::Cyclic => Err(ApiError::Forbidden("object inheritance chain is cyclic".to_string())),
        ChainWalk::Incomplete => Err(ApiError::Forbidden(
            "object inheritance chain is incomplete".to_string(),
        )),
    }
}

/// The write-side counterpart of [`fetch_chain`]: refuses a `parent_id` that would create an
/// object whose own inheritance chain the read side could never evaluate.
///
/// `ADR-0012` §3 freezes the inheritance-chain depth at `limits-v1.md`'s `tree_depth_max` (32,
/// root at depth 0), and `gate-commands.md` requires `depth=33` and a `parent_id` cycle to fail
/// closed. Enforcing that *only* on the read side leaves the row writable: a child of a parent
/// already sitting at depth 32 lands in the table and is then permanently un-authorizable for
/// every non-admin principal (`fetch_chain` returns `Forbidden` for it and for its whole subtree)
/// — a durable, self-inflicted denial of service that no read-side check can undo. The same
/// argument applies to attaching a child under an already-cyclic or already-broken chain: the row
/// is accepted and instantly dead.
///
/// The database only guards `parent_id <> id` (`flow_objects_parent_not_self_check`, migration
/// `0054`), so `A -> B -> A` is a perfectly legal row pair as far as `PostgreSQL` is concerned; this
/// walk is what actually rejects it. v0.4 has no path that *re-parents* an existing object (only
/// `create_object` writes `parent_id`, always with a freshly generated `id`), so a cycle cannot be
/// closed by today's surface — but this check is the one that must hold when v0.5's cross-parent
/// `move` command arrives, and it costs one already-budgeted recursive CTE outside any lock.
///
/// # Errors
/// `BadRequest` when `parent_id` has no row in `workspace_id`. A typed `limit_exceeded` carrying
/// `limits-v1.md`'s frozen `limit_kind = "tree_depth"` when the child would sit deeper than
/// `TREE_DEPTH_MAX`. A typed `invalid_update` when the parent's own chain is cyclic or incomplete
/// (a cycle too long to close inside the probe bound is reported as the `tree_depth` limit
/// instead — still a refusal, just under the other name).
/// Propagates a database read failure otherwise.
pub async fn ensure_parent_can_adopt_child<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    parent_id: Uuid,
) -> Result<(), ApiError> {
    match walk_chain(conn, workspace_id, parent_id).await? {
        ChainWalk::Complete(chain) => {
            // `chain.len()` is the parent's own depth + 1, so the child's chain would have
            // `chain.len() + 1` nodes. The child is legal exactly while that stays within
            // `MAX_CHAIN_NODES` (33 nodes = depths 0..=32) — the same boundary `fetch_chain`
            // enforces on the way back out, so a row this accepts is always one the read path can
            // still evaluate.
            let child_nodes = chain.len().saturating_add(1);
            if child_nodes > MAX_CHAIN_NODES {
                return Err(ApiError::limit_exceeded(
                    "parent_object_id is already at the frozen tree depth limit",
                    "tree_depth",
                    Some(serde_json::json!(TREE_DEPTH_MAX)),
                    Some(serde_json::json!(child_nodes.saturating_sub(1))),
                    None,
                ));
            }
            Ok(())
        }
        ChainWalk::Missing => Err(ApiError::BadRequest("parent_object_id not found".to_string())),
        ChainWalk::TooDeep => Err(ApiError::limit_exceeded(
            "parent_object_id's inheritance chain is already deeper than the frozen tree depth limit",
            "tree_depth",
            Some(serde_json::json!(TREE_DEPTH_MAX)),
            None,
            None,
        )),
        ChainWalk::Cyclic => Err(ApiError::invalid_update(
            "parent_object_id's inheritance chain is cyclic",
        )),
        ChainWalk::Incomplete => Err(ApiError::invalid_update(
            "parent_object_id's inheritance chain is incomplete",
        )),
    }
}

/// Cheap existence-and-ownership probe: does `object_id` have a row in `workspace_id`?
///
/// This is the *only* thing [`effective_permission`]'s workspace-admin override needs from the
/// database, and it deliberately does not walk the inheritance chain: `ADR-0012` §4.1 point 3
/// ("workspace admin 兜底永不可被边界切断，管理员救援路径…") and the `authz_boundary_self_lockout_
/// guarded` gate both require an admin to keep `full_access` precisely when the chain is broken,
/// so running [`fetch_chain`] first would take the rescue path away exactly when it is needed.
async fn object_exists_in_workspace<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    object_id: Uuid,
) -> Result<bool, ApiError> {
    let row = conn
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM flow_objects WHERE id = $1 AND workspace_id = $2",
            vec![object_id.into(), workspace_id.into()],
        ))
        .await?;
    Ok(row.is_some())
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
/// `default_member_level`). Missing or invalid settings fail closed rather than manufacturing a
/// minimum permission.
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
    let row = row.ok_or_else(|| ApiError::NotFound("flow workspace settings not found".to_string()))?;
    PermissionLevel::parse(&row.default_member_level)
        .ok_or_else(|| ApiError::Forbidden("flow workspace baseline is invalid".to_string()))
}

/// `ADR-0012` §3's effective-permission rule.
///
/// `principal_kind` is `"user"` or `"bot"` (matches `flow_object_grants.principal_kind`); `role`
/// is the caller's `workspace_members.role` (`"owner"`/`"admin"`/`"member"`/...), used only for
/// the workspace-admin override and the workspace baseline.
///
/// # Errors
/// `NotFound` when `object_id` has no row in `workspace_id`. `Forbidden` when the inheritance
/// chain is over-deep, cyclic, or incomplete — see [`fetch_chain`], which fails closed instead of
/// judging on a partial chain. Propagates a database read failure otherwise; every caller must
/// treat any `Err` as a denial and must not fall back to a default level.
pub async fn effective_permission<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    object_id: Uuid,
    principal_kind: &str,
    principal_id: Uuid,
    role: &str,
) -> Result<PermissionLevel, ApiError> {
    // Workspace admin always keeps `full_access` regardless of any authorization boundary
    // (`ADR-0012` §3: "workspace admin 永远保留 full_access（可审计的管理员兜底）"; §4.1 point 3:
    // "workspace admin 兜底永不可被边界切断"; gate `authz_boundary_self_lockout_guarded`:
    // "边界永不切断 admin 兜底").
    //
    // What the contract grants is `full_access` *inside this workspace* — every one of those four
    // statements says "workspace admin", and `role` itself comes from the caller's
    // `workspace_members` row for `workspace_id`. So the override is gated on the object actually
    // belonging to `workspace_id` first. Before that gate this function was the one place where a
    // caller-supplied `object_id` and `workspace_id` were never compared at all: `fetch_chain`'s
    // `WHERE o.id = $1 AND o.workspace_id = $2` is the only join between them, and the admin
    // return jumped over it, so an admin got `full_access` for another tenant's object id and for
    // ids that do not exist — the same shape as the cross-tenant ticket hole fixed in `5ca6845`,
    // which survived there only for `owner`/`admin`.
    //
    // Deliberately a single-row probe rather than `fetch_chain`: an admin must keep the rescue
    // path when the chain is cyclic, over-deep, or broken (`ADR-0012` §4.1 point 3 exists
    // precisely because an authorization boundary can lock its own author out and only an admin
    // can undo it), and `fetch_chain` denies all three. Ownership is a different question from
    // chain health, and only the first one belongs in front of the admin override.
    // `ADR-0012` §4.1 point 5 (2026-08-30): **the admin fallback belongs to people, not to bots.**
    // `middleware::bot_auth::bot_role_from_permissions` synthesizes `role = "admin"` for any token
    // carrying `BotPermission::Admin`, so without the `principal_kind` half of this condition every
    // admin bot would short-circuit straight past every authorization boundary the moment
    // `flow_object_grants` became writable — "被授予方同时又是万能兜底者，是自相矛盾的". The
    // justification for the fallback is an *auditable human rescue* of a self-lockout (§4.1 point
    // 3); a script is not who that is for. A bot that needs a restricted subtree gets an explicit
    // `flow_object_grants` row like any other grantee.
    //
    // This does **not** revoke the bot's workspace-level admin powers (feature flag, legacy
    // import, ...): those never pass through this function, and below, a bot with `role = "admin"`
    // still picks up `full_access` from `workspace_baseline` wherever no boundary intervenes —
    // which is every object v0.4 could reach. The narrowing is exactly the object-level boundary
    // bypass §4.1 point 5 names, and nothing else.
    if principal_kind == "user" && (role == "owner" || role == "admin") {
        if object_exists_in_workspace(conn, workspace_id, object_id).await? {
            return Ok(PermissionLevel::FullAccess);
        }
        // Collapses "belongs to another workspace" and "does not exist" into one answer, so the
        // override cannot be used as a cross-tenant existence oracle.
        return Err(ApiError::NotFound("flow object not found".to_string()));
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
            // Unreachable: `index` came from `chain.iter().enumerate()`. Fail closed with an
            // error rather than inventing a permission level if that ever stops holding.
            tracing::error!(index, len = chain.len(), "authz: boundary index outside the chain");
            return Err(ApiError::Internal);
        };
        // No grant at or below the boundary ⇒ the maximum is taken over the empty set, which is
        // *nothing*, not `view` — see [`PermissionLevel::Denied`].
        Ok(best_grant_in(bounded).unwrap_or(PermissionLevel::Denied))
    } else {
        // No boundary anywhere up to the root: effective = max(all chain grants, baseline).
        let baseline = workspace_baseline(conn, workspace_id, role).await?;
        Ok(best_grant_in(&chain).map_or(baseline, |g| g.max(baseline)))
    }
}

/// Batch form of [`effective_permission`] for read-side candidate pages.
///
/// One recursive CTE resolves every requested object's complete leaf-to-root chain, and one grant
/// query loads this principal's rows across their union. This avoids one recursive CTE plus one
/// grant round trip per object. The API permission cache calls this only for misses; write paths
/// continue to call [`effective_permission`] directly under their fencing transaction. A
/// candidate whose own chain is missing, too deep, cyclic, or incomplete resolves to
/// [`PermissionLevel::Denied`] without poisoning the rest of the batch. That is fail closed for
/// the candidate while allowing list scans to hide corrupt rows and continue; an explicitly
/// requested object is still rejected by its caller because `Denied` cannot satisfy even `view`.
///
/// Because this is a separately optimized implementation, the real-database test
/// `batch_and_single_effective_permissions_are_strictly_equal` pins every result to the single
/// evaluator for the same fixture.
pub async fn effective_permissions<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    object_ids: &[Uuid],
    principal_kind: &str,
    principal_id: Uuid,
    role: &str,
) -> Result<Vec<(Uuid, PermissionLevel)>, ApiError> {
    if object_ids.is_empty() {
        return Ok(Vec::new());
    }

    if principal_kind == "user" && (role == "owner" || role == "admin") {
        #[derive(FromQueryResult)]
        struct ObjectRow {
            id: Uuid,
        }
        let existing = ObjectRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM flow_objects WHERE workspace_id = $1 AND id = ANY($2)",
            vec![workspace_id.into(), object_ids.to_vec().into()],
        ))
        .all(conn)
        .await?;
        let existing: HashSet<Uuid> = existing.into_iter().map(|row| row.id).collect();
        return Ok(object_ids
            .iter()
            .copied()
            .map(|id| {
                let level = if existing.contains(&id) {
                    PermissionLevel::FullAccess
                } else {
                    PermissionLevel::Denied
                };
                (id, level)
            })
            .collect());
    }

    #[derive(FromQueryResult)]
    struct Row {
        seed_id: Uuid,
        id: Uuid,
        parent_id: Option<Uuid>,
        inherit_from_parent: bool,
        depth: i32,
        cycle: bool,
    }
    let probe_depth = i64::try_from(MAX_CHAIN_NODES).unwrap_or(i64::MAX);
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "WITH RECURSIVE chain AS ( \
             SELECT o.id AS seed_id, o.id, o.parent_id, o.inherit_from_parent, 0 AS depth, \
                    ARRAY[o.id]::uuid[] AS path, false AS cycle \
               FROM flow_objects o \
              WHERE o.workspace_id = $1 AND o.id = ANY($2) \
             UNION ALL \
             SELECT c.seed_id, p.id, p.parent_id, p.inherit_from_parent, c.depth + 1, \
                    c.path || p.id, p.id = ANY(c.path) \
               FROM chain c \
               JOIN flow_objects p ON p.id = c.parent_id AND p.workspace_id = $1 \
              WHERE c.parent_id IS NOT NULL AND c.depth < $3::int AND NOT c.cycle \
         ) \
         SELECT seed_id, id, parent_id, inherit_from_parent, depth, cycle \
           FROM chain ORDER BY seed_id, depth",
        vec![workspace_id.into(), object_ids.to_vec().into(), probe_depth.into()],
    ))
    .all(conn)
    .await?;

    struct BatchNode {
        id: Uuid,
        parent_id: Option<Uuid>,
        inherit_from_parent: bool,
        depth: i32,
        cycle: bool,
    }
    let mut chains: HashMap<Uuid, Vec<BatchNode>> = HashMap::new();
    for row in rows {
        chains.entry(row.seed_id).or_default().push(BatchNode {
            id: row.id,
            parent_id: row.parent_id,
            inherit_from_parent: row.inherit_from_parent,
            depth: row.depth,
            cycle: row.cycle,
        });
    }

    let mut chain_ids = HashSet::new();
    let mut invalid_chains = HashSet::new();
    let tree_depth_max = i32::try_from(TREE_DEPTH_MAX).map_err(|_| ApiError::Internal)?;
    for object_id in object_ids {
        let Some(chain) = chains.get(object_id) else {
            invalid_chains.insert(*object_id);
            continue;
        };
        let Some(top) = chain.last() else {
            invalid_chains.insert(*object_id);
            continue;
        };
        if chain.iter().any(|node| node.cycle || node.depth > tree_depth_max) || top.parent_id.is_some() {
            invalid_chains.insert(*object_id);
            continue;
        }
        chain_ids.extend(chain.iter().map(|node| node.id));
    }

    let chain_ids: Vec<Uuid> = chain_ids.into_iter().collect();
    let grants: HashMap<Uuid, PermissionLevel> = fetch_grants(conn, &chain_ids, principal_kind, principal_id)
        .await?
        .into_iter()
        .collect();
    let baseline = workspace_baseline(conn, workspace_id, role).await?;
    let mut resolved = Vec::with_capacity(object_ids.len());
    for object_id in object_ids {
        if invalid_chains.contains(object_id) {
            resolved.push((*object_id, PermissionLevel::Denied));
            continue;
        }
        let Some(chain) = chains.get(object_id) else {
            resolved.push((*object_id, PermissionLevel::Denied));
            continue;
        };
        let boundary_index = chain.iter().position(|node| !node.inherit_from_parent);
        let applicable = boundary_index.map_or(chain.as_slice(), |index| chain.get(..=index).unwrap_or_default());
        let best_grant = applicable.iter().filter_map(|node| grants.get(&node.id).copied()).max();
        let level = if boundary_index.is_some() {
            best_grant.unwrap_or(PermissionLevel::Denied)
        } else {
            best_grant.map_or(baseline, |grant| grant.max(baseline))
        };
        resolved.push((*object_id, level));
    }
    Ok(resolved)
}

/// One object's inheritance chain, resolved and classified for a caller that needs to *show* it
/// rather than just be judged against it (`GET /flow/objects/{object_id}/grants`'s `inherited[]`).
///
/// `ids` is leaf-first — `ids[0]` is the object itself — exactly as [`effective_permission`] sees
/// it, and `boundary_index` is the index of the first `inherit_from_parent = false` node, so
/// `ids[1..=boundary_index]` is "the ancestors whose grants still reach this object" and
/// `ids[1..]` is that same set when no boundary exists. Deriving both from the *same* walk is what
/// stops a share panel from listing an ancestor grant that no longer applies.
pub struct InheritanceChain {
    pub ids: Vec<Uuid>,
    pub boundary_index: Option<usize>,
}

impl InheritanceChain {
    /// The ancestor ids whose grants actually contribute to this object's effective permission —
    /// empty when the object is itself the authorization boundary.
    #[must_use]
    pub fn contributing_ancestors(&self) -> &[Uuid] {
        let end = self
            .boundary_index
            .map_or(self.ids.len(), |index| index.saturating_add(1));
        self.ids.get(1..end).unwrap_or(&[])
    }
}

/// Resolves `object_id`'s inheritance chain with the same fail-closed walk [`effective_permission`]
/// uses.
///
/// # Errors
/// `NotFound` when the object has no row in this workspace; `Forbidden` when the chain is
/// over-deep, cyclic, or incomplete. Propagates a database read failure otherwise.
pub async fn inheritance_chain<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    object_id: Uuid,
) -> Result<InheritanceChain, ApiError> {
    let chain = fetch_chain(conn, workspace_id, object_id).await?;
    let boundary_index = chain.iter().position(|node| !node.inherit_from_parent);
    Ok(InheritanceChain {
        ids: chain.into_iter().map(|node| node.id).collect(),
        boundary_index,
    })
}

/// Takes the conflicting (`FOR UPDATE`) lock on the workspace's `authz_epoch` row, held to the
/// caller's commit.
///
/// `ADR-0012` §3.1 point 2: an authorization change takes the exclusive lock on the same row a
/// content write takes `FOR SHARE` on, "两者因此不可能交叉成功". Taking it as the *first* statement
/// of an authorization transaction is also what fixes the lock rank — every later row this
/// transaction touches (`flow_object_grants`, `flow_objects`) is acquired after the epoch row,
/// matching the order [`fence_epoch_for_share`]'s callers use.
///
/// # Errors
/// `NotFound` if the workspace has no `flow_workspace_settings` row. Propagates a database read
/// failure otherwise.
pub async fn lock_epoch_for_update<C: ConnectionTrait>(conn: &C, workspace_id: Uuid) -> Result<i64, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        authz_epoch: i64,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT authz_epoch FROM flow_workspace_settings WHERE workspace_id = $1 FOR UPDATE",
        vec![workspace_id.into()],
    ))
    .one(conn)
    .await?;
    row.map(|r| r.authz_epoch)
        .ok_or_else(|| ApiError::NotFound("flow workspace settings not found".to_string()))
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

/// Reads the current authorization epoch under `FOR SHARE`, held until the caller's transaction
/// ends. Ticket issuance uses this before its authoritative membership/object permission reads so
/// an authorization writer cannot commit between the check and the ticket row insertion.
///
/// # Errors
/// `NotFound` if the workspace has no Flow settings. Propagates database failures otherwise.
pub async fn lock_epoch_for_share<C: ConnectionTrait>(conn: &C, workspace_id: Uuid) -> Result<i64, ApiError> {
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
    row.map(|row| row.authz_epoch)
        .ok_or_else(|| ApiError::NotFound("flow workspace settings not found".to_string()))
}

/// The commit-time fencing barrier (`ADR-0012` §3.1, `collab-protocol-v1.md` §"鉴权与连接" point
/// 7): takes `SELECT ... FOR SHARE` on the workspace's `authz_epoch` row — held to the caller's
/// commit — and rejects if the epoch it reads is not the `checked_epoch` the caller computed
/// permission against outside the transaction.
///
/// This is not a pre-insert check: the `FOR SHARE` blocks until any concurrent authorization
/// change (which takes `FOR UPDATE` on the same row, see [`advance_epoch`]) either
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
/// v0.5's `super::super::grants` write path calls this as the last step of every authorization
/// change it commits; v0.4 had no caller and named it `advance_epoch_for_test` for that reason.
///
/// # Errors
/// `NotFound` if the workspace has no `flow_workspace_settings` row. Propagates a database write
/// failure otherwise.
pub async fn advance_epoch<C: ConnectionTrait>(conn: &C, workspace_id: Uuid) -> Result<i64, ApiError> {
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

/// Advances `authz_epoch` when this workspace already has Flow settings.
///
/// General workspace membership routes are not Flow provisioning routes. They call this as the
/// first statement in the same transaction as their membership mutation: an existing settings row
/// is locked and advanced before any member row is touched, while an absent row is a successful
/// `None` and is never created as a side effect.
///
/// # Errors
/// Propagates a database write failure.
pub async fn advance_epoch_if_present<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
) -> Result<Option<i64>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        authz_epoch: i64,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_workspace_settings SET authz_epoch = authz_epoch + 1 \
         WHERE workspace_id = $1 RETURNING authz_epoch",
        vec![workspace_id.into()],
    ))
    .one(conn)
    .await?;
    Ok(row.map(|row| row.authz_epoch))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::PermissionLevel;

    #[test]
    fn permission_levels_are_totally_ordered_per_adr_0012() {
        assert!(PermissionLevel::Denied < PermissionLevel::View);
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
        // `Denied` is not a storable grade: it must never round-trip out of a `level` column.
        assert!(PermissionLevel::parse("none").is_none());
        assert_eq!(PermissionLevel::Denied.as_wire(), "none");
    }

    #[test]
    fn chain_bounds_match_adr_0012_tree_depth_max() {
        // Depth is counted with the root at 0 (`collab_core::limits::depth_of`), so a chain at
        // exactly `tree_depth_max = 32` has 33 nodes. Getting this wrong in either direction is a
        // security bug: one node short truncates legal depth-32 chains (and can hide an
        // authorization boundary), one node long accepts a chain the limit forbids.
        assert_eq!(super::TREE_DEPTH_MAX, 32);
        assert_eq!(super::MAX_CHAIN_NODES, 33);
    }
}

// ---- Real-database tests (opt-in via `OPENPR_TEST_DATABASE_URL`) ----
//
// Same scratch-database convention as `super::write`'s database tests: a throwaway database per
// test, migrated from `migrations/*.sql` on disk, dropped on the way out. These exercise the
// `ADR-0012` §3 effective-permission rule against real `flow_objects.parent_id` chains, which is
// the only way to cover the fail-closed cases `gates/gate-commands.md` requires (`depth=33`, a
// `parent_id` cycle, an incomplete chain) — they are properties of the recursive walk, not of any
// pure function.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::indexing_slicing,
    clippy::too_many_lines
)]
mod database_tests {
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
    use uuid::Uuid;

    use super::{PermissionLevel, effective_permission, effective_permissions};
    use crate::error::ApiError;

    /// Nodes in the deepest chain `ADR-0012` §3 permits, written as a literal on purpose: these
    /// tests must pin the *contract* (`limits-v1.md`'s `tree_depth_max = 32`, root at depth 0, so
    /// depths `0..=32`), not whatever `super::MAX_CHAIN_NODES` currently happens to say. Deriving
    /// the fixture sizes from the implementation constant would make the fixtures slide along with
    /// an off-by-one and quietly keep passing.
    const DEEPEST_LEGAL_CHAIN_NODES: usize = 33;

    /// One node past the limit: depth 33, which `gate-commands.md` requires to fail closed.
    const FIRST_ILLEGAL_CHAIN_NODES: usize = 34;

    const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

    struct Scratch {
        db: DatabaseConnection,
        name: String,
        admin_url: String,
    }

    impl Scratch {
        async fn drop_self(self) {
            let Self { db, name, admin_url } = self;
            drop(db);
            let Ok(admin) = Database::connect(&admin_url).await else {
                return;
            };
            let _ = admin
                .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                .await;
        }
    }

    async fn scratch(label: &str) -> Option<Scratch> {
        let admin_url = std::env::var(TEST_DATABASE_URL_ENV).ok()?;
        let admin = Database::connect(&admin_url)
            .await
            .unwrap_or_else(|err| panic!("{TEST_DATABASE_URL_ENV} is set but unusable: {err}"));

        let name = format!("openpr_collab_authz_{label}");
        let quoted = format!("\"{name}\"");
        admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS {quoted} WITH (FORCE)"))
            .await
            .unwrap_or_else(|err| panic!("could not reset scratch database {name}: {err}"));
        admin
            .execute_unprepared(&format!("CREATE DATABASE {quoted}"))
            .await
            .unwrap_or_else(|err| panic!("could not create scratch database {name}: {err}"));

        let (prefix, _) = admin_url.rsplit_once('/')?;
        let url = format!("{prefix}/{name}");
        let db = Database::connect(&url)
            .await
            .unwrap_or_else(|err| panic!("could not connect to scratch database {name}: {err}"));

        migrate(&db).await;

        Some(Scratch { db, name, admin_url })
    }

    async fn migrate(db: &DatabaseConnection) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .expect("migrations directory is readable")
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "no migration file was found in {dir}");
        for path in files {
            let sql = std::fs::read_to_string(&path).expect("a migration file is readable");
            db.execute_unprepared(&sql)
                .await
                .unwrap_or_else(|err| panic!("applying {} failed: {err}", path.display()));
        }
    }

    macro_rules! scratch_or_skip {
        ($label:expr) => {
            match scratch($label).await {
                Some(scratch) => scratch,
                None => {
                    eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
                    return;
                }
            }
        };
    }

    async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
        db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .await
            .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
    }

    /// A workspace with `default_member_level = 'edit'` (the `ADR-0012` §3 migration-safety
    /// default) plus an `owner` and a plain `member`.
    struct Fixture {
        workspace_id: Uuid,
        member_id: Uuid,
    }

    async fn seed_workspace(db: &DatabaseConnection) -> Fixture {
        let workspace_id = Uuid::new_v4();
        let owner_id = Uuid::new_v4();
        let member_id = Uuid::new_v4();
        for user_id in [owner_id, member_id] {
            exec(
                db,
                "INSERT INTO users (id, email, password_hash, name, role, is_active) \
                 VALUES ($1, $2, '!', 'test', 'user', true)",
                vec![user_id.into(), format!("{user_id}@authz.test").into()],
            )
            .await;
        }
        exec(
            db,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'authz test', $3)",
            vec![
                workspace_id.into(),
                format!("ws-{workspace_id}").into(),
                owner_id.into(),
            ],
        )
        .await;
        for (user_id, role) in [(owner_id, "owner"), (member_id, "member")] {
            exec(
                db,
                "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, $3)",
                vec![workspace_id.into(), user_id.into(), role.into()],
            )
            .await;
        }
        exec(
            db,
            "INSERT INTO flow_workspace_settings (workspace_id, flow_enabled, default_member_level) \
             VALUES ($1, true, 'edit')",
            vec![workspace_id.into()],
        )
        .await;
        Fixture {
            workspace_id,
            member_id,
        }
    }

    async fn insert_object(
        db: &DatabaseConnection,
        workspace_id: Uuid,
        parent_id: Option<Uuid>,
        inherit_from_parent: bool,
    ) -> Uuid {
        let id = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO flow_objects (id, workspace_id, object_type, parent_id, inherit_from_parent) \
             VALUES ($1, $2, 'page', $3, $4)",
            vec![
                id.into(),
                workspace_id.into(),
                parent_id.into(),
                inherit_from_parent.into(),
            ],
        )
        .await;
        id
    }

    /// Builds a root-to-leaf chain of `nodes` objects (its leaf therefore sits at depth
    /// `nodes - 1`) and returns the ids **leaf-first**, the same order `fetch_chain` must produce.
    ///
    /// `boundary_from_root`, when set, is the index *counted from the root* of the single node
    /// whose `inherit_from_parent` is false.
    async fn build_chain(
        db: &DatabaseConnection,
        workspace_id: Uuid,
        nodes: usize,
        boundary_from_root: Option<usize>,
    ) -> Vec<Uuid> {
        let mut ids = Vec::with_capacity(nodes);
        let mut parent = None;
        for index in 0..nodes {
            let inherit = boundary_from_root != Some(index);
            let id = insert_object(db, workspace_id, parent, inherit).await;
            ids.push(id);
            parent = Some(id);
        }
        ids.reverse();
        ids
    }

    async fn grant(db: &DatabaseConnection, workspace_id: Uuid, object_id: Uuid, principal_id: Uuid, level: &str) {
        exec(
            db,
            "INSERT INTO flow_object_grants (workspace_id, object_id, principal_kind, principal_id, level) \
             VALUES ($1, $2, 'user', $3, $4)",
            vec![workspace_id.into(), object_id.into(), principal_id.into(), level.into()],
        )
        .await;
    }

    async fn member_level(db: &DatabaseConnection, fx: &Fixture, object_id: Uuid) -> Result<PermissionLevel, ApiError> {
        effective_permission(db, fx.workspace_id, object_id, "user", fx.member_id, "member").await
    }

    fn assert_forbidden(result: &Result<PermissionLevel, ApiError>, what: &str) {
        match result {
            Err(ApiError::Forbidden(_)) => {}
            Err(other) => panic!("{what}: expected Forbidden, got {other:?}"),
            Ok(level) => panic!("{what}: expected Forbidden, but permission resolved to {level:?} (fail-open)"),
        }
    }

    // ---- the fail-open regressions ----

    /// The core escalation: the authorization boundary sits *above* the depth the walk may reach.
    /// Truncating the chain hides it, `boundary_index` comes back `None`, and the `else` branch
    /// re-applies the workspace baseline (`edit`) — precisely the defect `ADR-0012` R16 was
    /// revised to remove ("断开继承也切不断 baseline"). The only correct answer is a decidable
    /// denial.
    #[tokio::test]
    async fn a_boundary_above_the_depth_limit_is_rejected_not_downgraded_to_the_baseline() {
        let scratch = scratch_or_skip!("deep_boundary");
        let fx = seed_workspace(&scratch.db).await;

        // 40 nodes (leaf at depth 39) with the boundary 4 below the root — far above any cut.
        let chain = build_chain(&scratch.db, fx.workspace_id, 40, Some(4)).await;

        let result = member_level(&scratch.db, &fx, chain[0]).await;
        assert_forbidden(&result, "over-deep chain hiding a boundary");

        scratch.drop_self().await;
    }

    /// `flow_objects_parent_workspace_fk` normally makes a cross-workspace parent impossible, so
    /// this drops the constraint to reproduce that invariant being violated. The point is what
    /// the read path does when it *is* violated: `WHERE ... AND workspace_id = $2` finds no
    /// ancestor row, and the old walk read that as "the chain ends here" — losing the boundary
    /// above and falling back to the baseline.
    #[tokio::test]
    async fn a_parent_in_another_workspace_is_rejected_not_treated_as_a_root() {
        let scratch = scratch_or_skip!("cross_workspace_parent");
        let fx = seed_workspace(&scratch.db).await;
        let other = seed_workspace(&scratch.db).await;

        scratch
            .db
            .execute_unprepared("ALTER TABLE flow_objects DROP CONSTRAINT flow_objects_parent_workspace_fk")
            .await
            .expect("the composite parent FK can be dropped for this fixture");

        // The boundary lives in the other workspace, so a walk that stops at the workspace edge
        // never sees it and would hand back the `edit` baseline instead.
        let foreign_root = insert_object(&scratch.db, other.workspace_id, None, false).await;
        let child = insert_object(&scratch.db, fx.workspace_id, Some(foreign_root), true).await;

        let result = member_level(&scratch.db, &fx, child).await;
        assert_forbidden(&result, "parent in another workspace");

        scratch.drop_self().await;
    }

    /// A `parent_id` cycle is blocked by no schema constraint (only self-parenting is) and never
    /// reaches a root, so the old bounded loop returned its first 33 nodes as though they were a
    /// complete chain — no boundary found, baseline applied.
    #[tokio::test]
    async fn a_parent_id_cycle_is_rejected() {
        let scratch = scratch_or_skip!("parent_cycle");
        let fx = seed_workspace(&scratch.db).await;

        let a = insert_object(&scratch.db, fx.workspace_id, None, true).await;
        let b = insert_object(&scratch.db, fx.workspace_id, Some(a), true).await;
        exec(
            &scratch.db,
            "UPDATE flow_objects SET parent_id = $1 WHERE id = $2",
            vec![b.into(), a.into()],
        )
        .await;

        let result = member_level(&scratch.db, &fx, b).await;
        assert_forbidden(&result, "parent_id cycle");

        scratch.drop_self().await;
    }

    // ---- the off-by-one, nailed from both sides ----

    /// `tree_depth_max = 32` with the root at depth 0 means a legal chain spans depths `0..=32`:
    /// 33 nodes joined by 32 hops. The boundary is placed on the *root* — the 33rd and last node
    /// — so this fails if the walk stops even one node early, which would both deny a legitimate
    /// object and, in the old code, hide the boundary.
    #[tokio::test]
    async fn a_boundary_on_the_deepest_legal_node_is_still_seen() {
        let scratch = scratch_or_skip!("depth_32_boundary");
        let fx = seed_workspace(&scratch.db).await;

        let chain = build_chain(&scratch.db, fx.workspace_id, DEEPEST_LEGAL_CHAIN_NODES, Some(0)).await;
        let leaf = chain[0];
        let root = chain[DEEPEST_LEGAL_CHAIN_NODES - 1];

        // No grant anywhere: the boundary must suppress the `edit` baseline entirely.
        let level = member_level(&scratch.db, &fx, leaf)
            .await
            .expect("a chain at exactly tree_depth_max must evaluate, not be rejected");
        assert_eq!(
            level,
            PermissionLevel::Denied,
            "a boundary at depth 32 must still cut the workspace baseline"
        );

        // ...and a grant on that same boundary node must be reachable at that depth.
        grant(&scratch.db, fx.workspace_id, root, fx.member_id, "full_access").await;
        let level = member_level(&scratch.db, &fx, leaf).await.expect("still evaluates");
        assert_eq!(
            level,
            PermissionLevel::FullAccess,
            "the boundary node at depth 32 belongs to chain[..=index]"
        );

        scratch.drop_self().await;
    }

    /// One node deeper than legal must fail closed (`gate-commands.md`: "`depth=33` … 必须 fail
    /// closed"), even though nothing else about the chain is malformed.
    #[tokio::test]
    async fn a_chain_one_node_past_the_limit_is_rejected() {
        let scratch = scratch_or_skip!("depth_33_rejected");
        let fx = seed_workspace(&scratch.db).await;

        let chain = build_chain(&scratch.db, fx.workspace_id, FIRST_ILLEGAL_CHAIN_NODES, None).await;
        let result = member_level(&scratch.db, &fx, chain[0]).await;
        assert_forbidden(&result, "a chain of 34 nodes (depth 33)");

        // ...while its parent, sitting at exactly the limit, still resolves normally.
        let at_limit = member_level(&scratch.db, &fx, chain[1])
            .await
            .expect("depth 32 is legal and must resolve");
        assert_eq!(
            at_limit,
            PermissionLevel::Edit,
            "no boundary ⇒ the edit baseline applies"
        );

        scratch.drop_self().await;
    }

    // ---- no regression in the normal boundary semantics ----

    #[tokio::test]
    async fn boundary_and_baseline_semantics_are_unchanged() {
        let scratch = scratch_or_skip!("boundary_semantics");
        let fx = seed_workspace(&scratch.db).await;

        // (1) No boundary, no grant ⇒ the workspace baseline (`edit`).
        let plain = build_chain(&scratch.db, fx.workspace_id, 3, None).await;
        assert_eq!(
            member_level(&scratch.db, &fx, plain[0]).await.expect("resolves"),
            PermissionLevel::Edit
        );

        // (2) No boundary, a grant *below* the baseline ⇒ max(grant, baseline) = baseline.
        grant(&scratch.db, fx.workspace_id, plain[0], fx.member_id, "view").await;
        assert_eq!(
            member_level(&scratch.db, &fx, plain[0]).await.expect("resolves"),
            PermissionLevel::Edit,
            "max(view, edit) is edit"
        );

        // (3) No boundary, an ancestor grant *above* the baseline ⇒ that grant wins.
        grant(&scratch.db, fx.workspace_id, plain[2], fx.member_id, "full_access").await;
        assert_eq!(
            member_level(&scratch.db, &fx, plain[0]).await.expect("resolves"),
            PermissionLevel::FullAccess
        );

        // (4) A boundary on the object itself ⇒ the baseline stops applying, and with no grant at
        //     or under the boundary the member drops to `view`.
        let restricted = build_chain(&scratch.db, fx.workspace_id, 3, Some(2)).await;
        assert_eq!(
            member_level(&scratch.db, &fx, restricted[0]).await.expect("resolves"),
            PermissionLevel::Denied,
            "restrict-access must actually restrict"
        );

        // (5) `restricted[1]` is the root, which sits *above* the boundary at `restricted[0]`, so
        //     its grant must not count; a grant on the object itself is at the boundary and does.
        grant(&scratch.db, fx.workspace_id, restricted[1], fx.member_id, "full_access").await;
        assert_eq!(
            member_level(&scratch.db, &fx, restricted[0]).await.expect("resolves"),
            PermissionLevel::Denied,
            "a grant above the boundary must not leak through it"
        );
        grant(&scratch.db, fx.workspace_id, restricted[0], fx.member_id, "comment").await;
        assert_eq!(
            member_level(&scratch.db, &fx, restricted[0]).await.expect("resolves"),
            PermissionLevel::Comment
        );

        scratch.drop_self().await;
    }

    /// The optimized page evaluator is independent code, so its oracle is the DB-direct single
    /// evaluator, never a cache read. The fixture covers baseline, the boundary node and a
    /// descendant below a boundary with no grants, a grant below another boundary, an inherited
    /// grant above the baseline, the admin override, and a mixed absent id.
    #[tokio::test]
    async fn batch_and_single_effective_permissions_are_strictly_equal() {
        let scratch = scratch_or_skip!("batch_equals_single");
        let fx = seed_workspace(&scratch.db).await;
        let plain = build_chain(&scratch.db, fx.workspace_id, 3, None).await;
        let restricted = build_chain(&scratch.db, fx.workspace_id, 3, Some(1)).await;
        let ungranted = build_chain(&scratch.db, fx.workspace_id, 4, Some(1)).await;
        grant(&scratch.db, fx.workspace_id, restricted[0], fx.member_id, "comment").await;
        grant(&scratch.db, fx.workspace_id, plain[2], fx.member_id, "full_access").await;
        let object_ids = vec![
            plain[0],
            plain[1],
            restricted[0],
            restricted[2],
            ungranted[2],
            ungranted[0],
        ];

        let batch = effective_permissions(
            &scratch.db,
            fx.workspace_id,
            &object_ids,
            "user",
            fx.member_id,
            "member",
        )
        .await
        .expect("the batch resolves");
        let mut singles = Vec::new();
        for object_id in &object_ids {
            let level = effective_permission(&scratch.db, fx.workspace_id, *object_id, "user", fx.member_id, "member")
                .await
                .expect("the single evaluator resolves the same fixture");
            singles.push((*object_id, level));
        }

        assert_eq!(
            batch, singles,
            "batch results must match DB-direct singles in input order"
        );
        assert_eq!(
            batch.get(4).map(|(_, level)| *level),
            Some(PermissionLevel::Denied),
            "the boundary node itself has no grant and must not inherit the workspace baseline"
        );
        assert_eq!(
            batch.get(5).map(|(_, level)| *level),
            Some(PermissionLevel::Denied),
            "a descendant below an ungranted boundary must not inherit the workspace baseline"
        );

        let absent = Uuid::new_v4();
        let mixed = effective_permissions(
            &scratch.db,
            fx.workspace_id,
            &[plain[0], absent],
            "user",
            fx.member_id,
            "member",
        )
        .await
        .expect("one absent candidate cannot poison its healthy batch peer");
        assert_eq!(mixed[0], (plain[0], PermissionLevel::FullAccess));
        assert_eq!(mixed[1], (absent, PermissionLevel::Denied));
        assert!(
            matches!(
                effective_permission(&scratch.db, fx.workspace_id, absent, "user", fx.member_id, "member").await,
                Err(ApiError::NotFound(_))
            ),
            "an explicitly requested absent object remains a rejection"
        );

        let admin_batch = effective_permissions(
            &scratch.db,
            fx.workspace_id,
            &[plain[0], absent],
            "user",
            fx.member_id,
            "admin",
        )
        .await
        .expect("admin batch evaluates every candidate independently");
        assert_eq!(admin_batch[0], (plain[0], PermissionLevel::FullAccess));
        assert_eq!(admin_batch[1], (absent, PermissionLevel::Denied));

        exec(
            &scratch.db,
            "DELETE FROM flow_workspace_settings WHERE workspace_id = $1",
            vec![fx.workspace_id.into()],
        )
        .await;
        let single_missing = effective_permission(
            &scratch.db,
            fx.workspace_id,
            object_ids[0],
            "user",
            fx.member_id,
            "member",
        )
        .await;
        let batch_missing = effective_permissions(
            &scratch.db,
            fx.workspace_id,
            &object_ids,
            "user",
            fx.member_id,
            "member",
        )
        .await;
        assert!(
            matches!(single_missing, Err(ApiError::NotFound(_))),
            "single evaluator manufactured a baseline: {single_missing:?}"
        );
        assert!(
            matches!(batch_missing, Err(ApiError::NotFound(_))),
            "batch evaluator manufactured a baseline: {batch_missing:?}"
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn batch_effective_permissions_deny_only_the_corrupt_candidate() {
        let scratch = scratch_or_skip!("batch_corrupt_chains");
        let fx = seed_workspace(&scratch.db).await;
        let healthy = build_chain(&scratch.db, fx.workspace_id, 2, None).await[0];

        let too_deep = build_chain(&scratch.db, fx.workspace_id, FIRST_ILLEGAL_CHAIN_NODES, None).await;
        let result = effective_permissions(
            &scratch.db,
            fx.workspace_id,
            &[too_deep[0], healthy],
            "user",
            fx.member_id,
            "member",
        )
        .await
        .expect("an over-deep candidate must not poison its healthy peer");
        assert_eq!(
            result,
            vec![(too_deep[0], PermissionLevel::Denied), (healthy, PermissionLevel::Edit)]
        );

        let cycle_a = insert_object(&scratch.db, fx.workspace_id, None, true).await;
        let cycle_b = insert_object(&scratch.db, fx.workspace_id, Some(cycle_a), true).await;
        exec(
            &scratch.db,
            "UPDATE flow_objects SET parent_id = $1 WHERE id = $2",
            vec![cycle_b.into(), cycle_a.into()],
        )
        .await;
        let result = effective_permissions(
            &scratch.db,
            fx.workspace_id,
            &[cycle_b, healthy],
            "user",
            fx.member_id,
            "member",
        )
        .await
        .expect("a cyclic candidate must not poison its healthy peer");
        assert_eq!(result[0], (cycle_b, PermissionLevel::Denied));
        assert_eq!(result[1], (healthy, PermissionLevel::Edit));

        scratch
            .db
            .execute_unprepared("ALTER TABLE flow_objects DROP CONSTRAINT flow_objects_parent_workspace_fk")
            .await
            .expect("the composite parent FK can be dropped for the broken-chain fixture");
        let foreign = seed_workspace(&scratch.db).await;
        let foreign_parent = insert_object(&scratch.db, foreign.workspace_id, None, true).await;
        let broken_child = insert_object(&scratch.db, fx.workspace_id, Some(foreign_parent), true).await;
        let result = effective_permissions(
            &scratch.db,
            fx.workspace_id,
            &[broken_child, healthy],
            "user",
            fx.member_id,
            "member",
        )
        .await
        .expect("a broken candidate must not poison its healthy peer");
        assert_eq!(result[0], (broken_child, PermissionLevel::Denied));
        assert_eq!(result[1], (healthy, PermissionLevel::Edit));

        scratch.drop_self().await;
    }

    /// `ADR-0012` §3 keeps a workspace admin at `full_access` no matter what the chain says
    /// ("可审计的管理员兜底") — including over a chain this module now refuses to evaluate for
    /// anybody else.
    #[tokio::test]
    async fn a_workspace_admin_keeps_full_access_over_any_chain() {
        let scratch = scratch_or_skip!("admin_fallback");
        let fx = seed_workspace(&scratch.db).await;

        let restricted = build_chain(&scratch.db, fx.workspace_id, 3, Some(2)).await;
        let over_deep = build_chain(&scratch.db, fx.workspace_id, FIRST_ILLEGAL_CHAIN_NODES, None).await;

        for role in ["owner", "admin"] {
            for object_id in [restricted[0], over_deep[0]] {
                let level = effective_permission(&scratch.db, fx.workspace_id, object_id, "user", fx.member_id, role)
                    .await
                    .expect("the admin fallback never depends on the chain");
                assert_eq!(level, PermissionLevel::FullAccess, "role {role} must keep full_access");
            }
        }

        scratch.drop_self().await;
    }

    /// ★ The admin override grants `full_access` **inside this workspace** — every statement of
    /// it says so (`ADR-0012` §3's table row "workspace admin 永远保留 `full_access`（可审计的管理员
    /// 兜底）", §3's baseline "admin ⇒ `full_access`", §4.1 point 3 "workspace admin 兜底永不可被
    /// 边界切断", and the `authz_boundary_self_lockout_guarded` gate's "边界永不切断 admin 兜底").
    /// It said nothing about objects that are not in the workspace at all.
    ///
    /// Before the ownership probe was put in front of it, this function was the one place where a
    /// caller-supplied `object_id` and `workspace_id` were never compared: `fetch_chain`'s
    /// `WHERE o.id = $1 AND o.workspace_id = $2` is their only join, and the admin return jumped
    /// over it. An `owner`/`admin` therefore got `full_access` for another tenant's object id, and
    /// for ids that do not exist anywhere — the same shape as the cross-tenant ticket hole fixed
    /// in `5ca6845`, which is exactly why that hole only ever reproduced for `owner`/`admin`.
    #[tokio::test]
    async fn the_admin_override_does_not_reach_outside_its_own_workspace() {
        let scratch = scratch_or_skip!("admin_scope");
        let fx = seed_workspace(&scratch.db).await;
        let other = seed_workspace(&scratch.db).await;

        let foreign_object = insert_object(&scratch.db, other.workspace_id, None, true).await;
        let absent_object = Uuid::new_v4();

        for role in ["owner", "admin"] {
            for (object_id, what) in [
                (foreign_object, "another workspace's object"),
                (absent_object, "an object that does not exist"),
            ] {
                // `fx.member_id` is a member of `fx.workspace_id` only; `role` is that workspace's
                // role. Both answers must collapse to the same `NotFound`, so the override cannot
                // double as a cross-tenant existence oracle either.
                match effective_permission(&scratch.db, fx.workspace_id, object_id, "user", fx.member_id, role).await {
                    Err(ApiError::NotFound(_)) => {}
                    Ok(level) => panic!("role {role} got {level:?} for {what} (cross-tenant fail-open)"),
                    Err(other) => panic!("role {role}: expected NotFound for {what}, got {other:?}"),
                }
            }
        }

        scratch.drop_self().await;
    }

    /// The other half of the same change: gating the override on *ownership* must not gate it on
    /// *chain health*. `ADR-0012` §4.1 point 3 and the `authz_boundary_self_lockout_guarded` gate
    /// exist precisely because an authorization boundary can lock its own author out and only an
    /// admin can undo it, so an admin has to keep `full_access` over a chain this module refuses
    /// to evaluate for anybody else. Running `fetch_chain` before the override — instead of the
    /// single-row ownership probe — would take the rescue path away exactly when it is needed.
    #[tokio::test]
    async fn a_workspace_admin_keeps_the_rescue_path_when_the_chain_is_broken() {
        let scratch = scratch_or_skip!("admin_rescue");
        let fx = seed_workspace(&scratch.db).await;

        // (1) a `parent_id` cycle
        let a = insert_object(&scratch.db, fx.workspace_id, None, true).await;
        let b = insert_object(&scratch.db, fx.workspace_id, Some(a), true).await;
        exec(
            &scratch.db,
            "UPDATE flow_objects SET parent_id = $1 WHERE id = $2",
            vec![b.into(), a.into()],
        )
        .await;

        // (2) a chain past the depth limit, and (3) an authorization boundary that has cut every
        //     non-admin off the object.
        let over_deep = build_chain(&scratch.db, fx.workspace_id, FIRST_ILLEGAL_CHAIN_NODES, None).await;
        let restricted = build_chain(&scratch.db, fx.workspace_id, 3, Some(2)).await;

        // Every one of these is `Forbidden` for a plain member...
        assert_forbidden(&member_level(&scratch.db, &fx, b).await, "cyclic chain, member");
        assert_forbidden(
            &member_level(&scratch.db, &fx, over_deep[0]).await,
            "over-deep chain, member",
        );

        // ...and still `full_access` for the workspace's own admins.
        for role in ["owner", "admin"] {
            for (object_id, what) in [
                (b, "an object inside a parent_id cycle"),
                (over_deep[0], "an object below the depth limit"),
                (restricted[0], "an object behind an authorization boundary"),
            ] {
                let level = effective_permission(&scratch.db, fx.workspace_id, object_id, "user", fx.member_id, role)
                    .await
                    .unwrap_or_else(|err| panic!("role {role} lost the rescue path for {what}: {err:?}"));
                assert_eq!(
                    level,
                    PermissionLevel::FullAccess,
                    "role {role} must keep full_access for {what}"
                );
            }
        }

        scratch.drop_self().await;
    }

    /// A missing *starting* object is an ordinary 404, not a broken invariant — turning it into a
    /// 500, or into a silent `view`, would both be wrong.
    #[tokio::test]
    async fn a_missing_object_is_not_found_rather_than_internal() {
        let scratch = scratch_or_skip!("missing_object");
        let fx = seed_workspace(&scratch.db).await;

        match member_level(&scratch.db, &fx, Uuid::new_v4()).await {
            Err(ApiError::NotFound(_)) => {}
            other => panic!("expected NotFound for an absent object, got {other:?}"),
        }

        scratch.drop_self().await;
    }
}
