//! `ADR-0012`'s v0.5 authorization *write* surface: the three endpoints that create, change and
//! read `flow_object_grants` rows and flip `flow_objects.inherit_from_parent`.
//!
//! ```text
//! GET /api/v1/flow/objects/{object_id}/grants
//! PUT /api/v1/flow/objects/{object_id}/grants
//! PUT /api/v1/flow/objects/{object_id}/inheritance
//! ```
//!
//! v0.4 shipped the *read* half of `ADR-0012` §3 (`super::collab::authz::effective_permission`:
//! the inheritance chain, the authorization boundary, the fail-closed walk) plus the §3.1
//! commit-time fence, against rows only a direct SQL seed could put there. This module is the
//! other half — the surface that actually writes them — and the two rules it exists to enforce
//! that a bare `UPDATE` would not:
//!
//! - **§4.1 self-lockout guard.** Setting a boundary, or clearing the grants under one, can leave
//!   the caller with no way back in. That may be a deliberate hand-over, but it must not happen
//!   silently: the post-state is computed *inside the same transaction that would commit it*, and
//!   a caller who would lose `full_access` without `confirm_self_lockout = true` gets the whole
//!   transaction rolled back as `policy_rejected`.
//! - **§4.1 point 3, the admin rescue path.** A workspace admin's `full_access` is never cut by a
//!   boundary, so `effective_permission` returns `FullAccess` for them unconditionally — which
//!   means an admin can never trip the self-lockout guard and can always undo someone else's
//!   boundary. That is not a special case in this module; it falls out of the shared rule, which
//!   is why this module deliberately has no admin branch of its own.
//!
//! Everything here runs `super::collab::authz::effective_permission` as the single source of
//! truth, before *and* after the mutation, on the transaction's own snapshot. `dry_run` is the
//! same code path with a `ROLLBACK` instead of a `COMMIT`, which is what makes
//! `rest-api-v1.md`'s "按与正式请求**完全相同**的 post-state 算法计算并返回同一份
//! `permission_changes` 摘要" true by construction rather than by two implementations agreeing.

use platform::app::AppState;
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use crate::error::ApiError;
use crate::events::{BusinessEventInput, FlowDispatchSpec, insert_flow_event};

use super::collab::authz::{self, GRANTS_PER_REQUEST_MAX, OBJECT_GRANTS_MAX, PermissionLevel};
use super::event_origin::CommandOrigin;
use super::event_policy::FLOW_PERMISSION_EVENT_TYPE_PREFIX;

/// The boundary-change event, named once so [`primary_event_index`] and the producer that pushes
/// it cannot drift apart.
const INHERITANCE_CHANGED_EVENT_TYPE: &str = "flow.permission.inheritance_changed";
use super::repository;

/// `flow_object_grants.principal_kind`'s registered values (the table's
/// `flow_object_grants_principal_kind_check`). Rejected here as well so a caller gets a typed
/// `invalid_update` instead of a database constraint error.
const PRINCIPAL_KINDS: &[&str] = &["user", "bot"];

/// One requested `flow_object_grants` row, as it arrives on the wire.
#[derive(Debug, Clone)]
pub struct GrantRequest {
    pub principal_kind: String,
    pub principal_id: Uuid,
    pub level: String,
}

/// A validated [`GrantRequest`].
#[derive(Debug, Clone, Copy)]
struct Grant {
    kind: PrincipalKind,
    id: Uuid,
    level: PermissionLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PrincipalKind {
    User,
    Bot,
}

impl PrincipalKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Bot => "bot",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "user" => Some(Self::User),
            "bot" => Some(Self::Bot),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Wire shapes (`rest-api-v1.md`, the `grants`/`inheritance` rows)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct GrantItem {
    pub principal_kind: String,
    pub principal_id: Uuid,
    pub level: String,
    pub granted_by: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct InheritedGrantItem {
    pub object_id: Uuid,
    pub principal_kind: String,
    pub principal_id: Uuid,
    pub level: String,
}

/// `GET /flow/objects/{object_id}/grants`.
///
/// `items`/`inherited` are the full roster and are only populated for a caller holding
/// `full_access` on the object ("对象 `view` 起可读自身 effective，`full_access` 才可读完整名单");
/// a `view`/`comment`/`edit` caller gets its own `effective_level` and the boundary flag and
/// nothing else, so the endpoint cannot be used to enumerate who else can see a page.
#[derive(Debug, Serialize)]
pub struct GrantsView {
    pub items: Vec<GrantItem>,
    pub inherit_from_parent: bool,
    pub inherited: Vec<InheritedGrantItem>,
    pub effective_level: String,
}

#[derive(Debug, Serialize)]
pub struct CallerPermissionChange {
    pub before_level: String,
    pub after_level: String,
    pub loses_full_access: bool,
}

#[derive(Debug, Serialize)]
pub struct AffectedPermissionChange {
    pub principal_kind: String,
    pub principal_id: Uuid,
    pub before_level: String,
    pub after_level: String,
}

#[derive(Debug, Serialize)]
pub struct PermissionChanges {
    pub caller: CallerPermissionChange,
    pub affected: Vec<AffectedPermissionChange>,
}

#[derive(Debug, Serialize)]
pub struct SetGrantsView {
    pub applied: bool,
    pub event_id: Option<Uuid>,
    pub permission_changes: PermissionChanges,
}

#[derive(Debug, Serialize)]
pub struct SetInheritanceView {
    pub inherit_from_parent: bool,
    pub applied: bool,
    pub event_id: Option<Uuid>,
    pub permission_changes: PermissionChanges,
}

// ---------------------------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------------------------

/// The authenticated caller, in the shape `effective_permission` judges principals in.
pub struct Caller {
    pub actor_id: Uuid,
    /// `"user"` or `"bot"` — `flow_object_grants.principal_kind`'s vocabulary, and the parameter
    /// `ADR-0012` §4.1 point 5 makes the admin fallback conditional on.
    pub principal_kind: String,
    /// The caller's `workspace_members.role`, or the role
    /// `middleware::bot_auth::bot_role_from_permissions` synthesized for a bot token.
    pub role: String,
    /// Where the call came from, declared by the transport that accepted it.
    ///
    /// Lives on `Caller` rather than as a separate parameter because
    /// `v0.5-collaboration.md` treats it as one audit fact — "审计……保存认证 actor、
    /// origin(surface/session/tool)、causation" — and because `Caller` is already the value every
    /// step of this module's transaction threads down to [`write_events`]. Both `PUT` endpoints
    /// read `source`/`correlation_id`/`causation_id` off it instead of writing a literal.
    pub origin: CommandOrigin,
}

impl Caller {
    fn is_bot(&self) -> bool {
        // `require_flow_workspace_access` only ever produces these two spellings; anything else is
        // treated as a bot, i.e. as the *less* privileged reading.
        self.principal_kind != "user"
    }

    /// `flow_object_grants.granted_by` references `users(id)`, so a bot caller records `NULL`
    /// there rather than a foreign key violation; the bot's identity is carried by the
    /// `business_events` row instead, exactly as `flow_import_jobs.actor_user_id` does.
    fn granted_by(&self) -> Option<Uuid> {
        if self.is_bot() { None } else { Some(self.actor_id) }
    }
}

pub struct SetGrantsInput {
    pub object_id: Uuid,
    pub caller: Caller,
    /// The complete replacement roster. An empty vector clears every explicit grant on the object
    /// ("空数组即清空显式授予"), which is why this is a `Vec` and not an `Option<Vec>`.
    pub grants: Vec<GrantRequest>,
    pub confirm_self_lockout: bool,
    pub dry_run: bool,
    pub idempotency_key: String,
}

pub struct SetInheritanceInput {
    pub object_id: Uuid,
    pub caller: Caller,
    pub inherit_from_parent: bool,
    /// Grants committed in the *same transaction* as the boundary change (`ADR-0012` §4.1 point
    /// 2), so no window exists in which the subtree has a boundary but nobody to administer it.
    ///
    /// `Some(list)` is a **whole-table replacement**, exactly the semantics of
    /// `PUT .../grants` ("`initial_grants` 的语义 = 替换，不是合并", `ADR-0012` §4.1 point 2,
    /// ruled 2026-08-31): after the commit the object's explicit grants are *exactly* `list`,
    /// so `Some(vec![])` clears them. Merging would leave grants that predate the boundary in
    /// place, which defeats the boundary's entire purpose, and would be the one path able to
    /// walk past `object_grants_max` — the very structural fact `limits-v1.md` freezes that
    /// ceiling on ("结果条数恒等于请求条数").
    ///
    /// `None` is "do not touch the grants at all", the only way to flip the boundary flag on
    /// its own. It is a distinct value rather than an empty list precisely because the wire
    /// field is optional (`rest-api-v1.md`: `initial_grants?`) and an omitted field must not
    /// silently wipe the roster.
    pub initial_grants: Option<Vec<GrantRequest>>,
    pub confirm_self_lockout: bool,
    pub dry_run: bool,
    pub idempotency_key: String,
}

// ---------------------------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------------------------

const IDEMPOTENCY_KEY_MIN_BYTES: usize = 1;
const IDEMPOTENCY_KEY_MAX_BYTES: usize = 128;

fn validate_idempotency_key(key: &str) -> Result<(), ApiError> {
    if (IDEMPOTENCY_KEY_MIN_BYTES..=IDEMPOTENCY_KEY_MAX_BYTES).contains(&key.len()) {
        return Ok(());
    }
    Err(ApiError::BadRequest(format!(
        "idempotency_key must be {IDEMPOTENCY_KEY_MIN_BYTES}-{IDEMPOTENCY_KEY_MAX_BYTES} bytes"
    )))
}

/// Turns the wire list into validated grants, rejecting anything the database would reject and
/// two things it would not: a duplicated `(principal_kind, principal_id)` (whose "winner" would
/// otherwise depend on statement order) and a request longer than `limits-v1.md`'s frozen
/// `grants_per_request_max`.
fn validate_grants(requested: &[GrantRequest]) -> Result<Vec<Grant>, ApiError> {
    if requested.len() > GRANTS_PER_REQUEST_MAX {
        return Err(ApiError::limit_exceeded(
            "too many grants in one request",
            "grants_per_request",
            Some(json!(GRANTS_PER_REQUEST_MAX)),
            Some(json!(requested.len())),
            None,
        ));
    }
    let mut grants: Vec<Grant> = Vec::with_capacity(requested.len());
    for entry in requested {
        let Some(kind) = PrincipalKind::parse(&entry.principal_kind) else {
            return Err(ApiError::invalid_update(format!(
                "principal_kind must be one of {PRINCIPAL_KINDS:?}"
            )));
        };
        let Some(level) = PermissionLevel::parse_grant_level(&entry.level) else {
            return Err(ApiError::invalid_update(
                "level must be one of [\"view\", \"comment\", \"edit\", \"full_access\"]".to_string(),
            ));
        };
        if grants
            .iter()
            .any(|held| held.kind == kind && held.id == entry.principal_id)
        {
            return Err(ApiError::invalid_update(
                "grants must not name the same principal twice".to_string(),
            ));
        }
        grants.push(Grant {
            kind,
            id: entry.principal_id,
            level,
        });
    }
    Ok(grants)
}

// ---------------------------------------------------------------------------------------------
// SQL
// ---------------------------------------------------------------------------------------------

#[derive(FromQueryResult)]
struct GrantRow {
    object_id: Uuid,
    principal_kind: String,
    principal_id: Uuid,
    level: String,
    granted_by: Option<Uuid>,
    created_at: chrono::DateTime<chrono::Utc>,
}

async fn grants_on<C: ConnectionTrait>(conn: &C, object_ids: &[Uuid]) -> Result<Vec<GrantRow>, ApiError> {
    if object_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(GrantRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT object_id, principal_kind, principal_id, level, granted_by, created_at \
           FROM flow_object_grants WHERE object_id = ANY($1) \
          ORDER BY object_id, principal_kind, principal_id",
        vec![object_ids.to_vec().into()],
    ))
    .all(conn)
    .await?)
}

async fn delete_grants_for(tx: &DatabaseTransaction, object_id: Uuid, keep: &[Grant]) -> Result<(), ApiError> {
    // Deleting by "not in the replacement set" rather than "delete all, then insert" keeps the
    // `created_at` of an unchanged row stable, which the audit trail and the `GET` response both
    // surface.
    let kinds: Vec<String> = keep.iter().map(|g| g.kind.as_str().to_string()).collect();
    let ids: Vec<Uuid> = keep.iter().map(|g| g.id).collect();
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM flow_object_grants g \
          WHERE g.object_id = $1 \
            AND NOT EXISTS ( \
                  SELECT 1 FROM unnest($2::text[], $3::uuid[]) AS keep(principal_kind, principal_id) \
                   WHERE keep.principal_kind = g.principal_kind AND keep.principal_id = g.principal_id \
                )",
        vec![object_id.into(), kinds.into(), ids.into()],
    ))
    .await?;
    Ok(())
}

async fn upsert_grant(
    tx: &DatabaseTransaction,
    workspace_id: Uuid,
    object_id: Uuid,
    grant: Grant,
    granted_by: Option<Uuid>,
) -> Result<(), ApiError> {
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_object_grants (workspace_id, object_id, principal_kind, principal_id, level, granted_by) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (object_id, principal_kind, principal_id) \
         DO UPDATE SET level = EXCLUDED.level, granted_by = EXCLUDED.granted_by, updated_at = now()",
        vec![
            workspace_id.into(),
            object_id.into(),
            grant.kind.as_str().into(),
            grant.id.into(),
            grant.level.as_wire().into(),
            granted_by.into(),
        ],
    ))
    .await?;
    Ok(())
}

async fn count_grants_on<C: ConnectionTrait>(conn: &C, object_id: Uuid) -> Result<i64, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        total: i64,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT count(*)::bigint AS total FROM flow_object_grants WHERE object_id = $1",
        vec![object_id.into()],
    ))
    .one(conn)
    .await?;
    Ok(row.map_or(0, |r| r.total))
}

/// Reads and row-locks the object's own boundary flag, so a concurrent `PUT .../inheritance` on
/// the same object serializes behind this one rather than both computing a post-state from the
/// same "before".
async fn lock_inherit_flag(tx: &DatabaseTransaction, workspace_id: Uuid, object_id: Uuid) -> Result<bool, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        inherit_from_parent: bool,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT inherit_from_parent FROM flow_objects WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
        vec![object_id.into(), workspace_id.into()],
    ))
    .one(tx)
    .await?;
    row.map(|r| r.inherit_from_parent)
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))
}

async fn set_inherit_flag(
    tx: &DatabaseTransaction,
    object_id: Uuid,
    inherit_from_parent: bool,
    updated_by: Option<Uuid>,
) -> Result<(), ApiError> {
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_objects SET inherit_from_parent = $2, updated_at = now(), \
                updated_by = COALESCE($3, updated_by) \
          WHERE id = $1",
        vec![object_id.into(), inherit_from_parent.into(), updated_by.into()],
    ))
    .await?;
    Ok(())
}

/// The `workspace_members.role` of each named user, for the workspace baseline half of
/// `effective_permission`. Missing users fall back to `"member"`, the least privileged reading.
async fn roles_of<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    user_ids: &[Uuid],
) -> Result<Vec<(Uuid, String)>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        user_id: Uuid,
        role: String,
    }
    if user_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT user_id, role FROM workspace_members WHERE workspace_id = $1 AND user_id = ANY($2)",
        vec![workspace_id.into(), user_ids.to_vec().into()],
    ))
    .all(conn)
    .await?;
    Ok(rows.into_iter().map(|r| (r.user_id, r.role)).collect())
}

// ---------------------------------------------------------------------------------------------
// Post-state evaluation
// ---------------------------------------------------------------------------------------------

/// The set of principals whose `flow_object_grants` rows this request writes: everyone who
/// already holds an explicit grant *on this object*, plus everyone the request names.
///
/// Deliberately bounded at `2 × grants_per_request_max` rather than "everyone the change could
/// possibly affect". A boundary flip also changes the effective permission of principals holding
/// grants on *ancestors* and, through the baseline, of every workspace member — neither is
/// enumerable within a bounded transaction (the second is not enumerable at all: a workspace's
/// member list is not a Flow fact). `rest-api-v1.md` does not define the extent of `affected[]`;
/// this module reports the principals whose rows it touches and says so, rather than reporting a
/// list that silently claims to be complete.
fn affected_principals(existing: &[GrantRow], requested: &[Grant]) -> Vec<(PrincipalKind, Uuid)> {
    let mut out: Vec<(PrincipalKind, Uuid)> = Vec::new();
    for row in existing {
        if let Some(kind) = PrincipalKind::parse(&row.principal_kind) {
            out.push((kind, row.principal_id));
        }
    }
    for grant in requested {
        out.push((grant.kind, grant.id));
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Runs `effective_permission` for one principal against whatever state the transaction currently
/// holds — the *only* permission evaluator this module uses, before and after the mutation alike.
async fn level_of(
    tx: &DatabaseTransaction,
    workspace_id: Uuid,
    object_id: Uuid,
    kind: PrincipalKind,
    principal_id: Uuid,
    role: &str,
) -> Result<PermissionLevel, ApiError> {
    match authz::effective_permission(tx, workspace_id, object_id, kind.as_str(), principal_id, role).await {
        Ok(level) => Ok(level),
        // An affected principal sitting behind a chain this workspace cannot evaluate holds
        // nothing, which is the honest summary line for them; it is not a reason to fail the
        // caller's whole request, whose own permission was judged separately and did resolve.
        Err(ApiError::Forbidden(_)) => Ok(PermissionLevel::Denied),
        Err(err) => Err(err),
    }
}

async fn summarize(
    tx: &DatabaseTransaction,
    workspace_id: Uuid,
    object_id: Uuid,
    principals: &[(PrincipalKind, Uuid)],
    roles: &[(Uuid, String)],
) -> Result<Vec<(PrincipalKind, Uuid, PermissionLevel)>, ApiError> {
    let mut out = Vec::with_capacity(principals.len());
    for &(kind, id) in principals {
        // A bot's synthesized `admin` role lives in its token, not in any row, so it cannot be
        // read here; `"member"` is the conservative stand-in, and per `ADR-0012` §4.1 point 5 a
        // bot never gets the admin object-level fallback anyway.
        let role = match kind {
            PrincipalKind::User => roles
                .iter()
                .find(|(user_id, _)| *user_id == id)
                .map_or("member", |(_, role)| role.as_str()),
            PrincipalKind::Bot => "member",
        };
        out.push((kind, id, level_of(tx, workspace_id, object_id, kind, id, role).await?));
    }
    Ok(out)
}

fn permission_changes(
    caller_before: PermissionLevel,
    caller_after: PermissionLevel,
    before: &[(PrincipalKind, Uuid, PermissionLevel)],
    after: &[(PrincipalKind, Uuid, PermissionLevel)],
) -> PermissionChanges {
    let affected = after
        .iter()
        .map(|&(kind, id, after_level)| {
            let before_level = before
                .iter()
                .find(|&&(before_kind, before_id, _)| before_kind == kind && before_id == id)
                .map_or(PermissionLevel::Denied, |&(_, _, level)| level);
            AffectedPermissionChange {
                principal_kind: kind.as_str().to_string(),
                principal_id: id,
                before_level: before_level.as_wire().to_string(),
                after_level: after_level.as_wire().to_string(),
            }
        })
        .collect();
    PermissionChanges {
        caller: CallerPermissionChange {
            before_level: caller_before.as_wire().to_string(),
            after_level: caller_after.as_wire().to_string(),
            loses_full_access: caller_before == PermissionLevel::FullAccess
                && caller_after < PermissionLevel::FullAccess,
        },
        affected,
    }
}

// ---------------------------------------------------------------------------------------------
// Read path
// ---------------------------------------------------------------------------------------------

/// `GET /flow/objects/{object_id}/grants`.
///
/// # Errors
/// `NotFound` when the object is not in this workspace; `Forbidden` when the caller holds nothing
/// on it (a caller who cannot read the object must not learn that it has a share list at all).
/// Propagates database read failures.
pub async fn get_grants(
    state: &AppState,
    workspace_id: Uuid,
    object_id: Uuid,
    caller: &Caller,
) -> Result<GrantsView, ApiError> {
    let effective = authz::effective_permission(
        &state.db,
        workspace_id,
        object_id,
        &caller.principal_kind,
        caller.actor_id,
        &caller.role,
    )
    .await?;
    if effective < PermissionLevel::View {
        return Err(ApiError::Forbidden(
            "insufficient permission for this object".to_string(),
        ));
    }

    let chain = authz::inheritance_chain(&state.db, workspace_id, object_id).await?;
    let inherit_from_parent = chain.boundary_index != Some(0);

    if effective < PermissionLevel::FullAccess {
        // Own effective level and the boundary flag only — never the roster.
        return Ok(GrantsView {
            items: Vec::new(),
            inherit_from_parent,
            inherited: Vec::new(),
            effective_level: effective.as_wire().to_string(),
        });
    }

    let own = grants_on(&state.db, &[object_id]).await?;
    let ancestors = chain.contributing_ancestors().to_vec();
    let inherited = grants_on(&state.db, &ancestors).await?;

    Ok(GrantsView {
        items: own
            .into_iter()
            .map(|row| GrantItem {
                principal_kind: row.principal_kind,
                principal_id: row.principal_id,
                level: row.level,
                granted_by: row.granted_by,
                created_at: row.created_at,
            })
            .collect(),
        inherit_from_parent,
        inherited: inherited
            .into_iter()
            .map(|row| InheritedGrantItem {
                object_id: row.object_id,
                principal_kind: row.principal_kind,
                principal_id: row.principal_id,
                level: row.level,
            })
            .collect(),
        effective_level: effective.as_wire().to_string(),
    })
}

// ---------------------------------------------------------------------------------------------
// Write path
// ---------------------------------------------------------------------------------------------

/// What one authorization transaction is asked to change.
enum Change {
    /// Replace the object's explicit grants with exactly this list.
    ReplaceGrants(Vec<Grant>),
    /// Flip the boundary flag. `Some(list)` *replaces* the object's explicit grants with
    /// exactly `list` in the same transaction (`ADR-0012` §4.1 point 2); `None` leaves every
    /// existing row alone.
    SetInheritance {
        inherit_from_parent: bool,
        initial_grants: Option<Vec<Grant>>,
    },
}

/// Whether the transaction [`apply_in_transaction`] built must be committed or rolled back. A
/// `dry_run` produces a complete, correct [`Outcome`] and `Rollback`, which is the whole of the
/// difference between a preview and a real request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Disposition {
    Commit,
    Rollback,
}

/// `PUT /flow/objects/{object_id}/grants`.
///
/// # Errors
/// See [`apply`] — `Forbidden` without object `full_access`, `policy_rejected` on an unconfirmed
/// self-lockout, `limit_exceeded` past the grant ceilings, `invalid_update` on a malformed entry.
pub async fn set_grants(
    state: &AppState,
    workspace_id: Uuid,
    input: SetGrantsInput,
) -> Result<SetGrantsView, ApiError> {
    validate_idempotency_key(&input.idempotency_key)?;
    let grants = validate_grants(&input.grants)?;
    let outcome = apply(
        state,
        workspace_id,
        input.object_id,
        &input.caller,
        Change::ReplaceGrants(grants),
        input.confirm_self_lockout,
        input.dry_run,
        &input.idempotency_key,
    )
    .await?;
    Ok(SetGrantsView {
        applied: outcome.applied,
        event_id: outcome.event_id,
        permission_changes: outcome.changes,
    })
}

/// `PUT /flow/objects/{object_id}/inheritance`.
///
/// # Errors
/// See [`apply`].
pub async fn set_inheritance(
    state: &AppState,
    workspace_id: Uuid,
    input: SetInheritanceInput,
) -> Result<SetInheritanceView, ApiError> {
    validate_idempotency_key(&input.idempotency_key)?;
    let initial_grants = match &input.initial_grants {
        Some(requested) => Some(validate_grants(requested)?),
        None => None,
    };
    let outcome = apply(
        state,
        workspace_id,
        input.object_id,
        &input.caller,
        Change::SetInheritance {
            inherit_from_parent: input.inherit_from_parent,
            initial_grants,
        },
        input.confirm_self_lockout,
        input.dry_run,
        &input.idempotency_key,
    )
    .await?;
    Ok(SetInheritanceView {
        inherit_from_parent: outcome.inherit_from_parent,
        applied: outcome.applied,
        event_id: outcome.event_id,
        permission_changes: outcome.changes,
    })
}

struct Outcome {
    applied: bool,
    event_id: Option<Uuid>,
    changes: PermissionChanges,
    inherit_from_parent: bool,
}

/// The one transaction both PUTs run.
///
/// Order is load-bearing, and is the `ADR-0012` §3.1 lock rank read top to bottom:
///
/// 1. `FOR UPDATE` on the workspace's `authz_epoch` row — the conflicting lock, taken first, so
///    any in-flight content write holding `FOR SHARE` on it has either committed or is blocked
///    before this transaction reads a single permission. Every later row (`flow_objects`,
///    `flow_object_grants`) is acquired after it, matching the rank content writes use.
/// 2. The caller's `full_access` check, on the transaction's own snapshot.
/// 3. The mutation.
/// 4. The post-state summary, from the same `effective_permission` used in (2), still inside the
///    transaction — so the "after" levels are the levels this commit would actually produce, not
///    a re-derivation that could disagree with it.
/// 5. The §4.1 guard, then either `ROLLBACK` (guard tripped, or `dry_run`) or the events, the
///    epoch bump and `COMMIT`.
///
/// A `dry_run` differing from a real request only in step 5 is what makes the contract's "同一份
/// `permission_changes` 摘要" hold by construction.
///
/// # Errors
/// `Forbidden` when the caller does not hold `full_access` on the object (`dry_run` included — it
/// "不得成为无权者的权限探测面"). `policy_rejected` when the caller would lose `full_access`
/// without `confirm_self_lockout`. `limit_exceeded` when the resulting roster would pass
/// `object_grants_max`. `Conflict` when the idempotency key was used for a different operation.
/// Propagates database failures.
#[allow(clippy::too_many_arguments)]
async fn apply(
    state: &AppState,
    workspace_id: Uuid,
    object_id: Uuid,
    caller: &Caller,
    change: Change,
    confirm_self_lockout: bool,
    dry_run: bool,
    idempotency_key: &str,
) -> Result<Outcome, ApiError> {
    let tx = state.db.begin().await?;

    let result = apply_in_transaction(
        &tx,
        workspace_id,
        object_id,
        caller,
        change,
        confirm_self_lockout,
        dry_run,
        idempotency_key,
    )
    .await;

    match result {
        Ok((outcome, Disposition::Commit)) => {
            tx.commit().await?;
            Ok(outcome)
        }
        // A successful answer that must not persist: the `dry_run` path. The rollback is checked
        // rather than ignored — a preview that failed to undo itself is a write, and the caller
        // has to hear about it.
        Ok((outcome, Disposition::Rollback)) => {
            tx.rollback().await?;
            Ok(outcome)
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(err)
        }
    }
}

/// The body of [`apply`], factored out so every early return rolls back through one place.
#[allow(clippy::too_many_arguments)]
async fn apply_in_transaction(
    tx: &DatabaseTransaction,
    workspace_id: Uuid,
    object_id: Uuid,
    caller: &Caller,
    change: Change,
    confirm_self_lockout: bool,
    dry_run: bool,
    idempotency_key: &str,
) -> Result<(Outcome, Disposition), ApiError> {
    // (1) The conflicting epoch lock, held to commit.
    authz::lock_epoch_for_update(tx, workspace_id).await?;

    // (2) Only `full_access` on the object may change its authorization — `dry_run` included.
    let caller_before = authz::effective_permission(
        tx,
        workspace_id,
        object_id,
        &caller.principal_kind,
        caller.actor_id,
        &caller.role,
    )
    .await?;
    if caller_before < PermissionLevel::FullAccess {
        return Err(ApiError::Forbidden(
            "full_access on this object is required to change its authorization".to_string(),
        ));
    }

    if !dry_run && let Some(existing) = repository::find_idempotent_event(tx, workspace_id, idempotency_key).await? {
        return replay(tx, workspace_id, object_id, caller_before, &existing).await;
    }

    let before_rows = grants_on(tx, &[object_id]).await?;
    let inherit_before = lock_inherit_flag(tx, workspace_id, object_id).await?;

    let (requested, inherit_after) = match &change {
        Change::ReplaceGrants(grants) => (grants.clone(), inherit_before),
        Change::SetInheritance {
            inherit_from_parent,
            initial_grants,
        } => (
            // `None` writes no grant row, so the "replacement roster" this transaction is
            // judged against is the empty list: `affected_principals` still picks up every
            // principal that already holds a row, whose effective level the boundary flip may
            // well change.
            initial_grants.clone().unwrap_or_default(),
            *inherit_from_parent,
        ),
    };

    let principals = affected_principals(&before_rows, &requested);
    let user_ids: Vec<Uuid> = principals
        .iter()
        .filter(|(kind, _)| *kind == PrincipalKind::User)
        .map(|(_, id)| *id)
        .collect();
    let roles = roles_of(tx, workspace_id, &user_ids).await?;
    let before = summarize(tx, workspace_id, object_id, &principals, &roles).await?;

    // (3) The mutation.
    let mut events: Vec<(&'static str, serde_json::Value)> = Vec::new();
    match &change {
        Change::ReplaceGrants(grants) => {
            delete_grants_for(tx, object_id, grants).await?;
            for grant in grants {
                upsert_grant(tx, workspace_id, object_id, *grant, caller.granted_by()).await?;
            }
        }
        Change::SetInheritance { initial_grants, .. } => {
            if inherit_after != inherit_before {
                set_inherit_flag(tx, object_id, inherit_after, caller.granted_by()).await?;
                events.push((
                    INHERITANCE_CHANGED_EVENT_TYPE,
                    json!({ "object_id": object_id, "inherit_from_parent": inherit_after }),
                ));
            }
            // `ADR-0012` §4.1 point 2: `initial_grants` replaces, it does not merge. The same
            // two statements `ReplaceGrants` runs, so a boundary really does redefine who can
            // see the subtree instead of quietly keeping grants that predate it.
            if let Some(grants) = initial_grants {
                delete_grants_for(tx, object_id, grants).await?;
                for grant in grants {
                    upsert_grant(tx, workspace_id, object_id, *grant, caller.granted_by()).await?;
                }
            }
        }
    }

    // `limits-v1.md`'s `object_grants_max`, checked on the *result* ("结果条目总数受
    // object_grants_max 约束") and reported as a `limit_exceeded` rather than a silent truncation
    // ("达到上限即拒绝新增条目，不静默截断已有授予").
    let total = count_grants_on(tx, object_id).await?;
    if usize::try_from(total).unwrap_or(usize::MAX) > OBJECT_GRANTS_MAX {
        return Err(ApiError::limit_exceeded(
            "object would hold more explicit grants than the ceiling allows",
            "object_grants",
            Some(json!(OBJECT_GRANTS_MAX)),
            Some(json!(total)),
            None,
        ));
    }

    // (4) The post-state, from the same evaluator, on this transaction's snapshot.
    let after = summarize(tx, workspace_id, object_id, &principals, &roles).await?;
    let caller_after = authz::effective_permission(
        tx,
        workspace_id,
        object_id,
        &caller.principal_kind,
        caller.actor_id,
        &caller.role,
    )
    .await?;
    let changes = permission_changes(caller_before, caller_after, &before, &after);

    // (5a) `ADR-0012` §4.1 point 1.
    if changes.caller.loses_full_access && !confirm_self_lockout {
        return Err(ApiError::policy_rejected_with_details(
            "this change would remove your own full_access on this object; \
             resend with confirm_self_lockout=true to proceed",
            json!({
                "action": "authz_self_lockout",
                "caller": {
                    "before_level": changes.caller.before_level,
                    "after_level": changes.caller.after_level,
                    "loses_full_access": true,
                },
            }),
        ));
    }

    // (5b) A dry run stops here with a complete summary and `Rollback`: no rows, no events, no
    // epoch bump, and the idempotency key never reached `business_events`, so the same key still
    // works for the real request afterwards.
    if dry_run {
        return Ok((
            Outcome {
                applied: false,
                event_id: None,
                changes,
                inherit_from_parent: inherit_after,
            },
            Disposition::Rollback,
        ));
    }

    // (5c) Audit, then the epoch bump, then commit.
    for (kind, id, after_level) in &after {
        let explicit_before = before_rows
            .iter()
            .find(|row| row.principal_kind == kind.as_str() && row.principal_id == *id)
            .and_then(|row| PermissionLevel::parse_grant_level(&row.level));
        let explicit_after = requested
            .iter()
            .find(|grant| grant.kind == *kind && grant.id == *id)
            .map(|grant| grant.level);
        // A boundary flip that carried no `initial_grants` touched no grant row, so every
        // principal keeps exactly the row it already had. Every other shape is a whole-table
        // replacement, and `requested` *is* the post-state roster.
        let explicit_after = if matches!(
            change,
            Change::SetInheritance {
                initial_grants: None,
                ..
            }
        ) {
            explicit_before
        } else {
            explicit_after
        };
        if explicit_before == explicit_after {
            continue;
        }
        if let Some(level) = explicit_after {
            events.push((
                "flow.permission.granted",
                json!({
                    "object_id": object_id,
                    "principal_kind": kind.as_str(),
                    "principal_id": id,
                    "level": level.as_wire(),
                }),
            ));
        } else {
            events.push((
                "flow.permission.revoked",
                json!({
                    "object_id": object_id,
                    "principal_kind": kind.as_str(),
                    "principal_id": id,
                    "old_level": explicit_before.map_or(PermissionLevel::Denied, |level| level).as_wire(),
                    "new_level": after_level.as_wire(),
                }),
            ));
        }
    }

    let event_id = write_events(tx, workspace_id, object_id, caller, &events, idempotency_key).await?;

    // Every authorization change advances the epoch in its own transaction (`ADR-0012` §3.1 point
    // 1), which is what makes a content write that checked permission before this commit fail its
    // `fence_epoch_for_share` afterwards. Unconditional, including for a request that changed no
    // row: a no-op that skipped the bump would be indistinguishable on the wire from one that did
    // not, and the cost of an extra epoch is a resync, never a wrong answer.
    authz::advance_epoch(tx, workspace_id).await?;

    Ok((
        Outcome {
            applied: true,
            event_id,
            changes,
            inherit_from_parent: inherit_after,
        },
        Disposition::Commit,
    ))
}

/// Which of a command's events is its **primary** one — the causal root the rest hang off, and
/// the only one that carries the caller's `idempotency_key`.
///
/// `events-v1.md` (2026-09-01 订正): "「直接父」= 该命令的主事件，而**主事件 = 携带调用方
/// `idempotency_key` 的那一条**". The previous wording named "command registry 里那条
/// `primary_event_type`", a registry that does not exist anywhere in this repository; the
/// corrected rule picks a property that does exist and is already enforced, because
/// `business_events` carries a unique index on `(workspace_id, idempotency_key)` — so "the one
/// with the key" is unique by construction, queryable, and assertable.
///
/// That leaves this function with the obligation the correction created: **choose** which event
/// gets the key, deterministically and from the events' own content. It must not be "index 0",
/// which is what this module did before. For a pure `set_grants` the event vector is built by
/// iterating principals, so index 0 is whichever principal the iteration reached first — making
/// the causal root of the request *arbitrary*, and different across two runs of the same request.
///
/// The rule, in order:
///
/// 1. `flow.permission.inheritance_changed` if the command produced one. A boundary change is the
///    command's headline transition — the grants that follow it are consequences of it — and
///    `ADR-0012` §4.1 admits at most one per request, so this is unambiguous.
/// 2. Otherwise the event smallest by `(event_type, principal_kind, principal_id)`. Every
///    component comes from the event's own payload, so the answer is a function of *what the
///    command did*, not of the order a `HashMap` happened to yield.
fn primary_event_index(events: &[(&'static str, serde_json::Value)]) -> Option<usize> {
    if let Some(index) = events
        .iter()
        .position(|(event_type, _)| *event_type == INHERITANCE_CHANGED_EVENT_TYPE)
    {
        return Some(index);
    }
    events
        .iter()
        .enumerate()
        .min_by_key(|(_, (event_type, payload))| {
            let text = |key: &str| payload.get(key).and_then(serde_json::Value::as_str).unwrap_or_default();
            (
                *event_type,
                text("principal_kind").to_string(),
                text("principal_id").to_string(),
            )
        })
        .map(|(index, _)| index)
}

/// Writes one command's permission events as a single causal tree and returns its primary event
/// id.
///
/// The primary event ([`primary_event_index`]) is inserted **first** so that the events derived
/// from it can name it in `causation_id` — the same parent-before-children ordering `move_object`
/// cannot have (its lock order forces children first, which is why it pre-mints the parent id
/// instead). Here nothing forces the order, so the simpler shape is the right one. Sibling
/// permission events share one transaction and therefore one `created_at` to the microsecond;
/// their insertion order carries no meaning and no contract depends on it.
async fn write_events(
    tx: &DatabaseTransaction,
    workspace_id: Uuid,
    object_id: Uuid,
    caller: &Caller,
    events: &[(&'static str, serde_json::Value)],
    idempotency_key: &str,
) -> Result<Option<Uuid>, ApiError> {
    let dispatch_max_attempts = crate::config::runtime().flow.dispatch_max_attempts;
    let Some(primary_index) = primary_event_index(events) else {
        return Ok(None);
    };
    let mut primary = None;
    // Primary first, then the rest in their natural order.
    let mut ordered: Vec<(usize, &(&'static str, serde_json::Value))> = Vec::with_capacity(events.len());
    ordered.extend(events.iter().enumerate().filter(|(index, _)| *index == primary_index));
    ordered.extend(events.iter().enumerate().filter(|(index, _)| *index != primary_index));
    for (index, (event_type, payload)) in ordered {
        let is_primary = index == primary_index;
        let outcome = insert_flow_event(
            tx,
            BusinessEventInput {
                workspace_id,
                project_id: None,
                event_type: (*event_type).to_string(),
                aggregate_type: "flow_permission".to_string(),
                aggregate_id: object_id.to_string(),
                actor_id: if caller.is_bot() { None } else { Some(caller.actor_id) },
                source: caller.origin.source_json(),
                payload: payload.clone(),
                metadata: json!({ "principal_kind": caller.principal_kind }),
                // Every event this one request writes shares the request's correlation
                // (`events-v1.md`: "首个用户请求生成 `correlation_id`").
                correlation_id: Some(caller.origin.correlation_id),
                // The primary event carries whatever caused the *command* (`None` for a first
                // user request — a meaningful "this is the root", not a missing value). Every
                // other event of the same command names the primary, so the request reconstructs
                // as one causal tree rather than N unrelated roots. `primary` is `Some` for all of
                // them because the primary is inserted first; it is read rather than defaulted so
                // that breaking that ordering surfaces as a wrong parent, not as a silent `NULL`.
                causation_id: if is_primary {
                    caller.origin.causation_id
                } else {
                    primary
                },
                // `business_events`' `(workspace_id, idempotency_key)` unique index is what makes a
                // replayed request return the original ids instead of writing a second set, and it
                // admits exactly one row per key — so exactly one event of a command may carry it,
                // and `events-v1.md` makes *that* event the primary by definition. The rest are
                // `NULL`, which the partial index ignores.
                idempotency_key: is_primary.then(|| idempotency_key.to_string()),
            },
            Some(FlowDispatchSpec {
                max_attempts: dispatch_max_attempts,
                document_id: None,
                accepted_seq: None,
            }),
        )
        .await?;
        if is_primary {
            primary = Some(outcome.event_id);
        }
    }
    Ok(primary)
}

/// A repeated `idempotency_key` returns the original event id and the *current* levels rather
/// than re-applying anything: the state the first request produced is already in place, so
/// "before" and "after" are the same thing, and saying so is more honest than replaying a diff
/// this request did not cause.
async fn replay(
    tx: &DatabaseTransaction,
    workspace_id: Uuid,
    object_id: Uuid,
    caller_level: PermissionLevel,
    existing: &repository::IdempotentEvent,
) -> Result<(Outcome, Disposition), ApiError> {
    if !existing.event_type.starts_with(FLOW_PERMISSION_EVENT_TYPE_PREFIX)
        || existing.aggregate_id != object_id.to_string()
    {
        return Err(ApiError::Conflict(
            "idempotency_key was already used for a different operation".to_string(),
        ));
    }
    let rows = grants_on(tx, &[object_id]).await?;
    let principals = affected_principals(&rows, &[]);
    let user_ids: Vec<Uuid> = principals
        .iter()
        .filter(|(kind, _)| *kind == PrincipalKind::User)
        .map(|(_, id)| *id)
        .collect();
    let roles = roles_of(tx, workspace_id, &user_ids).await?;
    let current = summarize(tx, workspace_id, object_id, &principals, &roles).await?;
    let inherit_from_parent = lock_inherit_flag(tx, workspace_id, object_id).await?;
    // Nothing was written, so the transaction is rolled back like a dry run; the ids returned are
    // the ones the original request committed.
    Ok((
        Outcome {
            applied: true,
            event_id: Some(existing.id),
            changes: permission_changes(caller_level, caller_level, &current, &current),
            inherit_from_parent,
        },
        Disposition::Rollback,
    ))
}

// ---------------------------------------------------------------------------------------------
// Real-database tests (opt-in via `OPENPR_TEST_DATABASE_URL`)
// ---------------------------------------------------------------------------------------------
//
// Same scratch-database convention as `super::command`'s and `super::collab::authz`'s database
// tests: a throwaway database per test, migrated from `migrations/*.sql` on disk, dropped on the
// way out. Every assertion here is about a rule that only exists once rows can be written, so
// none of it can be covered by a pure function test.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod primary_event_tests {
    use super::{INHERITANCE_CHANGED_EVENT_TYPE, primary_event_index};
    use serde_json::json;
    use uuid::Uuid;

    /// The rule that replaced "index 0", stated where it can be falsified deterministically.
    ///
    /// `events-v1.md` (2026-09-01 订正) makes the primary event the one carrying the caller's
    /// `idempotency_key`, and leaves this module to choose *which* event that is. The choice must
    /// not be positional: a pure `set_grants` builds its event vector by iterating principals, so
    /// "index 0" makes the causal root of the request depend on iteration order — the contract
    /// calls that "任意的，这不可接受".
    ///
    /// Every fixture here deliberately puts a **non-primary event at index 0**, which is what
    /// makes the assertion able to fail: reverting `primary_event_index` to `Some(0)` reddens
    /// every case below, with no dependence on how a `HashMap` happened to iterate that day. The
    /// database test that exercises the same rule through the real route cannot promise that —
    /// there, whether index 0 and the primary differ is up to the iteration order of the day.
    #[test]
    fn the_primary_event_is_chosen_from_content_and_never_from_position() {
        let alice = Uuid::parse_str("00000000-0000-0000-0000-0000000000a1").expect("fixture uuid");
        let bob = Uuid::parse_str("00000000-0000-0000-0000-0000000000b2").expect("fixture uuid");
        let granted = |kind: &str, id: Uuid| {
            (
                "flow.permission.granted",
                json!({ "principal_kind": kind, "principal_id": id, "level": "edit" }),
            )
        };
        let revoked = |kind: &str, id: Uuid| {
            (
                "flow.permission.revoked",
                json!({ "principal_kind": kind, "principal_id": id, "old_level": "edit" }),
            )
        };

        // (1) A boundary change is the command's headline transition wherever it sits.
        let inheritance_last = vec![
            granted("user", alice),
            (INHERITANCE_CHANGED_EVENT_TYPE, json!({ "inherit_from_parent": false })),
        ];
        assert_eq!(
            primary_event_index(&inheritance_last),
            Some(1),
            "the inheritance change is the primary even when it is not first in the vector"
        );

        // (2) No boundary change: the smallest `(event_type, principal_kind, principal_id)` wins.
        // `granted` sorts before `revoked`, so index 0's revocation must lose.
        let revoke_first = vec![revoked("user", alice), granted("user", bob)];
        assert_eq!(
            primary_event_index(&revoke_first),
            Some(1),
            "`flow.permission.granted` sorts before `flow.permission.revoked`, so index 0 is not the primary"
        );

        // (3) Same event type: `bot` sorts before `user`.
        let user_first = vec![granted("user", alice), granted("bot", bob)];
        assert_eq!(
            primary_event_index(&user_first),
            Some(1),
            "`bot` sorts before `user`, so index 0 is not the primary"
        );

        // (4) Same type and kind: the principal id breaks the tie, and reordering the very same
        // events must not move the answer — the property "index 0" cannot have.
        let ascending = vec![granted("user", alice), granted("user", bob)];
        let descending = vec![granted("user", bob), granted("user", alice)];
        assert_eq!(primary_event_index(&ascending), Some(0));
        assert_eq!(
            primary_event_index(&descending),
            Some(1),
            "the same two events in the other order must still elect the same event"
        );

        assert_eq!(
            primary_event_index(&[]),
            None,
            "a command that changed nothing has no primary"
        );
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::print_stdout,
    clippy::indexing_slicing,
    clippy::too_many_lines
)]
mod database_tests {
    use std::time::Duration;

    use platform::app::AppState;
    use platform::config::{AppConfig, Secret};
    use sea_orm::{
        ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait,
    };
    use uuid::Uuid;

    use super::{Caller, GrantRequest, SetGrantsInput, SetInheritanceInput, get_grants, set_grants, set_inheritance};
    use crate::error::{ApiError, ApiErrorKind};
    use crate::flow::collab::authz::{self, GRANTS_PER_REQUEST_MAX, OBJECT_GRANTS_MAX, PermissionLevel};
    use crate::flow::command::{
        CreateObjectInput, ExecuteCommandInput, SetFlowFeatureInput, create_object, execute_command, set_flow_feature,
    };
    use crate::flow::event_origin::{CommandOrigin, EventSource, EventSurface};
    use crate::middleware::bot_auth::bot_role_from_permissions;

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
        let name = format!("openpr_flow_grants_{label}");
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

    fn state_for(db: DatabaseConnection) -> AppState {
        AppState {
            cfg: AppConfig {
                app_name: "flow-grants-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("flow-grants-test-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db,
        }
    }

    async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
        db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .await
            .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
    }

    #[allow(clippy::struct_field_names)]
    struct Fixture {
        workspace_id: Uuid,
        owner_id: Uuid,
        member_id: Uuid,
        /// A `workspace_bots`-style principal id. `flow_object_grants.principal_id` deliberately
        /// has no foreign key, so a bare id is all a bot principal needs here.
        bot_id: Uuid,
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
                vec![user_id.into(), format!("{user_id}@grants.test").into()],
            )
            .await;
        }
        exec(
            db,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'grants test', $3)",
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
            owner_id,
            member_id,
            bot_id: Uuid::new_v4(),
        }
    }

    async fn create(state: &AppState, fx: &Fixture, object_type: &str, parent: Option<Uuid>) -> Uuid {
        create_object(
            state,
            CreateObjectInput {
                origin: crate::flow::event_origin::CommandOrigin::first_request_from(
                    crate::flow::event_origin::EventSurface::Rest,
                ),
                workspace_id: fx.workspace_id,
                actor_id: fx.owner_id,
                actor_is_bot: false,
                object_type: object_type.to_string(),
                project_id: None,
                parent_object_id: parent,
                title: "Authorization Fixture".to_string(),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
            },
        )
        .await
        .expect("object is created")
        .object
        .id
    }

    fn user(actor_id: Uuid, role: &str) -> Caller {
        Caller {
            origin: crate::flow::event_origin::CommandOrigin::first_request_from(
                crate::flow::event_origin::EventSurface::Rest,
            ),
            actor_id,
            principal_kind: "user".to_string(),
            role: role.to_string(),
        }
    }

    fn bot(actor_id: Uuid, role: &str) -> Caller {
        Caller {
            origin: crate::flow::event_origin::CommandOrigin::first_request_from(
                crate::flow::event_origin::EventSurface::Rest,
            ),
            actor_id,
            principal_kind: "bot".to_string(),
            role: role.to_string(),
        }
    }

    /// A caller whose origin is an explicit, non-REST surface — the only way to tell "the
    /// producer copied what the caller declared" apart from "the producer hardcoded REST".
    fn user_from(actor_id: Uuid, role: &str, origin: CommandOrigin) -> Caller {
        let mut caller = user(actor_id, role);
        caller.origin = origin;
        caller
    }

    #[derive(Debug, FromQueryResult)]
    struct EventRow {
        id: Uuid,
        event_type: String,
        source: serde_json::Value,
        correlation_id: Option<Uuid>,
        causation_id: Option<Uuid>,
        /// Only `write_events`' **first** event of a command carries the request's key (the
        /// `(workspace_id, idempotency_key)` unique index admits exactly one row per key), which
        /// makes this column the only reliable marker of "this row is the command's primary
        /// event". Row order cannot serve: every row of one command is inserted in one
        /// transaction and shares `created_at = now()` to the microsecond, so `ORDER BY
        /// created_at, id` breaks the tie on a random UUID.
        idempotency_key: Option<String>,
    }

    /// The committed events one command wrote, identified by the correlation it rooted — read
    /// back from the database.
    async fn events_with_correlation(db: &DatabaseConnection, correlation_id: Uuid) -> Vec<EventRow> {
        EventRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, event_type, source, correlation_id, causation_id, idempotency_key \
             FROM business_events WHERE correlation_id = $1 ORDER BY created_at, id",
            vec![correlation_id.into()],
        ))
        .all(db)
        .await
        .expect("business_events query runs")
    }

    fn grant_of(kind: &str, id: Uuid, level: &str) -> GrantRequest {
        GrantRequest {
            principal_kind: kind.to_string(),
            principal_id: id,
            level: level.to_string(),
        }
    }

    async fn level_for(db: &DatabaseConnection, fx: &Fixture, object_id: Uuid, caller: &Caller) -> PermissionLevel {
        authz::effective_permission(
            db,
            fx.workspace_id,
            object_id,
            &caller.principal_kind,
            caller.actor_id,
            &caller.role,
        )
        .await
        .unwrap_or_else(|err| panic!("effective_permission failed: {err:?}"))
    }

    async fn lifecycle_of(db: &DatabaseConnection, object_id: Uuid) -> String {
        #[derive(FromQueryResult)]
        struct Row {
            lifecycle_status: String,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT lifecycle_status FROM flow_objects WHERE id = $1",
            vec![object_id.into()],
        ))
        .one(db)
        .await
        .expect("query runs")
        .expect("object exists")
        .lifecycle_status
    }

    async fn inherit_flag(db: &DatabaseConnection, object_id: Uuid) -> bool {
        #[derive(FromQueryResult)]
        struct Row {
            inherit_from_parent: bool,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT inherit_from_parent FROM flow_objects WHERE id = $1",
            vec![object_id.into()],
        ))
        .one(db)
        .await
        .expect("query runs")
        .expect("object exists")
        .inherit_from_parent
    }

    async fn grant_count(db: &DatabaseConnection, object_id: Uuid) -> i64 {
        #[derive(FromQueryResult)]
        struct Row {
            n: i64,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM flow_object_grants WHERE object_id = $1",
            vec![object_id.into()],
        ))
        .one(db)
        .await
        .expect("query runs")
        .expect("count returns a row")
        .n
    }

    /// The object's explicit `flow_object_grants` roster, in a stable order, as
    /// `(principal_kind, principal_id, level)`. Read straight from the table rather than through
    /// `get_grants`, so a replacement that failed to delete cannot hide behind a view layer.
    async fn explicit_roster(db: &DatabaseConnection, object_id: Uuid) -> Vec<(String, Uuid, String)> {
        #[derive(FromQueryResult)]
        struct Row {
            principal_kind: String,
            principal_id: Uuid,
            level: String,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT principal_kind, principal_id, level FROM flow_object_grants \
              WHERE object_id = $1 ORDER BY principal_kind, principal_id",
            vec![object_id.into()],
        ))
        .all(db)
        .await
        .expect("query runs")
        .into_iter()
        .map(|row| (row.principal_kind, row.principal_id, row.level))
        .collect()
    }

    /// Every `principal_id` a `flow.permission.revoked` audit event named, sorted so the
    /// assertion does not depend on insertion order.
    async fn revoked_principals(db: &DatabaseConnection, workspace_id: Uuid) -> Vec<Uuid> {
        #[derive(FromQueryResult)]
        struct Row {
            principal_id: Uuid,
        }
        let mut ids: Vec<Uuid> = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT (payload->>'principal_id')::uuid AS principal_id FROM business_events \
              WHERE workspace_id = $1 AND event_type = 'flow.permission.revoked'",
            vec![workspace_id.into()],
        ))
        .all(db)
        .await
        .expect("query runs")
        .into_iter()
        .map(|row| row.principal_id)
        .collect();
        ids.sort_unstable();
        ids
    }

    async fn event_count(db: &DatabaseConnection, workspace_id: Uuid) -> i64 {
        #[derive(FromQueryResult)]
        struct Row {
            n: i64,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM business_events WHERE workspace_id = $1 AND event_type LIKE 'flow.permission.%'",
            vec![workspace_id.into()],
        ))
        .one(db)
        .await
        .expect("query runs")
        .expect("count returns a row")
        .n
    }

    async fn run_command(state: &AppState, object_id: Uuid, caller: &Caller, command: &str) -> Result<(), ApiError> {
        execute_command(
            state,
            ExecuteCommandInput {
                origin: crate::flow::event_origin::CommandOrigin::first_request_from(
                    crate::flow::event_origin::EventSurface::Rest,
                ),
                object_id,
                actor_id: caller.actor_id,
                principal_kind: caller.principal_kind.clone(),
                role: caller.role.clone(),
                command_type: command.to_string(),
                payload: serde_json::json!({}),
                expected_frontier: None,
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
                origin_client_id: "grants-test".to_string(),
            },
        )
        .await
        .map(|_| ())
    }

    fn assert_limit_kind(err: &ApiError, expected: &str) {
        assert_eq!(err.kind(), ApiErrorKind::LimitExceeded, "wrong error kind: {err:?}");
        let ApiError::Typed { details, .. } = err else {
            panic!("expected a Typed limit_exceeded error, got {err:?}");
        };
        assert_eq!(
            details.as_ref().and_then(|d| d.get("limit_kind")),
            Some(&serde_json::json!(expected)),
            "wrong limit_kind: {err:?}"
        );
    }

    fn assert_policy_rejected(result: &Result<(), ApiError>, what: &str) {
        match result {
            Err(err) if err.kind() == ApiErrorKind::PolicyRejected => {}
            Err(other) => panic!("{what}: expected policy_rejected, got {other:?}"),
            Ok(()) => panic!("{what}: expected policy_rejected, but the operation succeeded (fail-open)"),
        }
    }

    // -----------------------------------------------------------------------------------------
    // 1. An authorization boundary must cut the workspace baseline, not be max'd with it
    // -----------------------------------------------------------------------------------------

    /// ★ `ADR-0012` §3, R16's headline fix: "原设计写的是『有效权限 = 继承链最高档与 workspace
    /// 基线取高』。那样的话，只要 baseline 还是 `edit`，任何页面都不可能被降到 `view` 或无权，
    /// 断开继承也切不断 baseline —— 『限制访问』这个功能根本不工作。"
    ///
    /// Both directions are asserted, because only the pair is evidence: **before** the boundary
    /// the very same member holds the `edit` baseline and can archive the page; **after** it, the
    /// same member holds nothing and the same command is rejected. A `max(grants, baseline)`
    /// implementation passes the first assertion and fails the second, which is exactly what the
    /// mutation run shows.
    #[tokio::test]
    async fn an_authorization_boundary_cuts_the_workspace_baseline() {
        let scratch = scratch_or_skip!("boundary_cuts_baseline");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;
        let member = user(fx.member_id, "member");

        let parent = create(&state, &fx, "page", None).await;
        let child = create(&state, &fx, "page", Some(parent)).await;

        // Negative direction: no boundary ⇒ the `edit` baseline applies and really works.
        assert_eq!(
            level_for(&scratch.db, &fx, child, &member).await,
            PermissionLevel::Edit,
            "with no boundary a plain member must hold the workspace baseline"
        );
        run_command(&state, child, &member, "archive")
            .await
            .expect("a baseline `edit` member can archive a plain page before any boundary exists");
        run_command(&state, child, &member, "restore")
            .await
            .expect("...and restore it");

        // The boundary, set through the production surface by someone who holds `full_access`
        // (the workspace owner, via the `ADR-0012` §3 admin fallback).
        let view = set_inheritance(
            &state,
            fx.workspace_id,
            SetInheritanceInput {
                object_id: child,
                caller: user(fx.owner_id, "owner"),
                inherit_from_parent: false,
                initial_grants: None,
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("an owner can always set a boundary: the admin fallback keeps its own full_access");
        assert!(!view.inherit_from_parent);
        assert!(!inherit_flag(&scratch.db, child).await);

        // Positive direction: the baseline is *gone*, not merely outranked.
        assert_eq!(
            level_for(&scratch.db, &fx, child, &member).await,
            PermissionLevel::Denied,
            "a boundary must cut the workspace baseline entirely -- `max(grant, baseline)` would \
             still hand this member `edit`"
        );
        assert_policy_rejected(
            &run_command(&state, child, &member, "archive").await,
            "a member behind an authorization boundary",
        );
        assert_eq!(
            lifecycle_of(&scratch.db, child).await,
            "active",
            "the rejected archive must not have landed"
        );

        // The parent is untouched: a boundary restricts its own subtree, nothing above it.
        assert_eq!(
            level_for(&scratch.db, &fx, parent, &member).await,
            PermissionLevel::Edit,
            "the boundary must not leak upward"
        );

        // And an explicit grant at the boundary is the documented way back in.
        set_grants(
            &state,
            fx.workspace_id,
            SetGrantsInput {
                object_id: child,
                caller: user(fx.owner_id, "owner"),
                grants: vec![grant_of("user", fx.member_id, "edit")],
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("an owner can grant at the boundary");
        assert_eq!(
            level_for(&scratch.db, &fx, child, &member).await,
            PermissionLevel::Edit,
            "an explicit grant at the boundary is what restores access"
        );

        scratch.drop_self().await;
    }

    // -----------------------------------------------------------------------------------------
    // 2. `archive|restore` stays in the `edit` tier
    // -----------------------------------------------------------------------------------------

    /// `ADR-0012` §2's archive tier table, first row: "普通非根 Page、不级联的可逆软归档/恢复 ⇒
    /// `edit`". Putting it in `full_access` would take the archive capability away from every
    /// `default_member_level = edit` member who has it today, which is the zero-regression promise
    /// the same ADR makes to itself.
    ///
    /// Negative direction in the same test: the second tier row (`navigator`/root) must still
    /// require `full_access`, so this is not just "everything is `edit`".
    #[tokio::test]
    async fn archive_and_restore_stay_in_the_edit_tier_but_a_navigator_does_not() {
        let scratch = scratch_or_skip!("archive_tier");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;
        let member = user(fx.member_id, "member");

        // A member whose only permission is the `edit` baseline -- no grant anywhere.
        let page = create(&state, &fx, "page", None).await;
        assert_eq!(level_for(&scratch.db, &fx, page, &member).await, PermissionLevel::Edit);

        run_command(&state, page, &member, "archive")
            .await
            .expect("`edit` must be enough to soft-archive a plain non-root page");
        assert_eq!(lifecycle_of(&scratch.db, page).await, "archived");
        run_command(&state, page, &member, "restore")
            .await
            .expect("`edit` must be enough to restore it");
        assert_eq!(lifecycle_of(&scratch.db, page).await, "active");

        // Second tier row: a `navigator` is a root object and needs `full_access`.
        let navigator = create(&state, &fx, "navigator", None).await;
        assert_eq!(
            level_for(&scratch.db, &fx, navigator, &member).await,
            PermissionLevel::Edit,
            "the member holds `edit` here too -- what differs is what the command requires"
        );
        assert_policy_rejected(
            &run_command(&state, navigator, &member, "archive").await,
            "an `edit` member archiving a navigator",
        );
        assert_eq!(lifecycle_of(&scratch.db, navigator).await, "active");

        // ...and an explicit `full_access` grant on the navigator makes it work, proving the gate
        // is the *tier*, not the object type.
        set_grants(
            &state,
            fx.workspace_id,
            SetGrantsInput {
                object_id: navigator,
                caller: user(fx.owner_id, "owner"),
                grants: vec![grant_of("user", fx.member_id, "full_access")],
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("owner grants full_access on the navigator");
        run_command(&state, navigator, &member, "archive")
            .await
            .expect("`full_access` must be enough for a navigator");
        assert_eq!(lifecycle_of(&scratch.db, navigator).await, "archived");

        scratch.drop_self().await;
    }

    // -----------------------------------------------------------------------------------------
    // 3. An admin bot must not cross an object authorization boundary
    // -----------------------------------------------------------------------------------------

    /// ★ `ADR-0012` §4.1 point 5 and the `admin_bot_does_not_bypass_object_boundary` gate.
    /// `middleware::bot_auth::bot_role_from_permissions` synthesizes `role = "admin"` for any token
    /// carrying `BotPermission::Admin`, and `effective_permission` short-circuits on `role ==
    /// "admin"`, so the moment `flow_object_grants` became writable an admin bot would have walked
    /// through every boundary in the workspace.
    ///
    /// The gate demands *both* directions, and neither alone is evidence:
    /// (a) the admin bot is denied behind a boundary it has no grant for, and
    /// (b) the very same bot keeps its workspace-level admin powers and its synthesized role,
    ///     so nothing v0.4 shipped was taken away.
    #[tokio::test]
    async fn an_admin_bot_does_not_bypass_an_object_boundary_but_keeps_workspace_admin() {
        let scratch = scratch_or_skip!("admin_bot_boundary");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;

        // The exact role the bot middleware synthesizes for an `admin` token.
        let synthesized = bot_role_from_permissions(&["admin".to_string()]);
        assert_eq!(
            synthesized, "admin",
            "fixture premise: an admin token synthesizes role=admin"
        );
        let admin_bot = bot(fx.bot_id, &synthesized);
        let admin_user = user(fx.owner_id, "admin");

        let open_page = create(&state, &fx, "page", None).await;
        let restricted = create(&state, &fx, "page", Some(open_page)).await;

        // Premise, and half of direction (b): with no boundary the admin bot is unchanged from
        // v0.4 -- it still resolves to `full_access` on Flow objects.
        assert_eq!(
            level_for(&scratch.db, &fx, restricted, &admin_bot).await,
            PermissionLevel::FullAccess,
            "outside any boundary an admin bot must keep the v0.4 behaviour"
        );

        set_inheritance(
            &state,
            fx.workspace_id,
            SetInheritanceInput {
                object_id: restricted,
                caller: user(fx.owner_id, "owner"),
                inherit_from_parent: false,
                initial_grants: None,
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("owner sets the boundary");

        // (a) Denied behind the boundary, with no `flow_object_grants` row of its own.
        assert_eq!(
            level_for(&scratch.db, &fx, restricted, &admin_bot).await,
            PermissionLevel::Denied,
            "an admin bot must not inherit the human admin fallback through an object boundary"
        );
        assert_policy_rejected(
            &run_command(&state, restricted, &admin_bot, "archive").await,
            "an admin bot behind an authorization boundary",
        );
        // ...and it cannot change the authorization either, which is the escalation that matters.
        match set_grants(
            &state,
            fx.workspace_id,
            SetGrantsInput {
                object_id: restricted,
                caller: bot(fx.bot_id, &synthesized),
                grants: vec![grant_of("bot", fx.bot_id, "full_access")],
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        {
            Err(ApiError::Forbidden(_)) => {}
            Ok(_) => panic!("an admin bot granted itself access behind a boundary"),
            Err(other) => panic!("expected Forbidden for the admin bot's grant attempt, got {other:?}"),
        }

        // The contrast that proves this is about bots, not about the boundary: a *human* admin is
        // untouched -- §4.1 point 3's rescue path.
        assert_eq!(
            level_for(&scratch.db, &fx, restricted, &admin_user).await,
            PermissionLevel::FullAccess,
            "the human admin rescue path must survive the boundary"
        );

        // (b) The same bot's workspace-level admin operation still works: the Flow feature flag,
        // one of the v0.4-shipped `WorkspaceWide(admin)` capabilities `ADR-0012` §4.1 point 5
        // explicitly keeps ("workspace 级 admin 操作: bot 保留，行为不变").
        set_flow_feature(
            &state,
            SetFlowFeatureInput {
                origin: crate::flow::event_origin::CommandOrigin::first_request_from(
                    crate::flow::event_origin::EventSurface::Rest,
                ),
                workspace_id: fx.workspace_id,
                actor_id: fx.owner_id,
                actor_is_bot: false,
                enabled: Some(true),
                default_member_level: Some("edit".to_string()),
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("the workspace-level admin operation must remain available");
        assert_eq!(
            bot_role_from_permissions(&["admin".to_string()]),
            "admin",
            "the bot's workspace admin role must not have been narrowed away"
        );

        // (c) The documented route back in for a bot: an explicit grant, like any other grantee.
        set_grants(
            &state,
            fx.workspace_id,
            SetGrantsInput {
                object_id: restricted,
                caller: user(fx.owner_id, "owner"),
                grants: vec![grant_of("bot", fx.bot_id, "edit")],
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("owner grants the bot access explicitly");
        assert_eq!(
            level_for(&scratch.db, &fx, restricted, &admin_bot).await,
            PermissionLevel::Edit,
            "an explicit grant is how a bot reaches a restricted subtree"
        );

        scratch.drop_self().await;
    }

    // -----------------------------------------------------------------------------------------
    // 4. The self-lockout guard
    // -----------------------------------------------------------------------------------------

    /// ★ `ADR-0012` §4.1: a caller who would lose `full_access` must say so explicitly.
    ///
    /// Four assertions, and the gate needs all four:
    /// - without `confirm_self_lockout` the whole transaction is rejected and *nothing* moved
    ///   (flag, rows, events, epoch);
    /// - with it, the change goes through and the caller really is locked out;
    /// - the same-transaction escape hatch (§4.1 point 2, `initial_grants`) means a caller who
    ///   keeps itself an explicit grant never trips the guard at all;
    /// - the admin rescue path still works with the boundary in place.
    #[tokio::test]
    async fn self_lockout_needs_confirmation_and_leaves_an_admin_rescue_path() {
        let scratch = scratch_or_skip!("self_lockout");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;
        let member = user(fx.member_id, "member");

        let parent = create(&state, &fx, "page", None).await;
        let child = create(&state, &fx, "page", Some(parent)).await;

        // The member's `full_access` on `child` is *inherited* from `parent` -- the exact setup
        // §4.1 describes ("用户仅凭父级继承来的 full_access 调用 inheritance=false").
        set_grants(
            &state,
            fx.workspace_id,
            SetGrantsInput {
                object_id: parent,
                caller: user(fx.owner_id, "owner"),
                grants: vec![grant_of("user", fx.member_id, "full_access")],
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("owner grants full_access on the parent");
        assert_eq!(
            level_for(&scratch.db, &fx, child, &member).await,
            PermissionLevel::FullAccess
        );

        let epoch_before = authz::read_epoch(&scratch.db, fx.workspace_id).await.expect("epoch");
        let events_before = event_count(&scratch.db, fx.workspace_id).await;

        // (1) Unconfirmed: rejected, and the transaction left no trace.
        let err = set_inheritance(
            &state,
            fx.workspace_id,
            SetInheritanceInput {
                object_id: child,
                caller: user(fx.member_id, "member"),
                inherit_from_parent: false,
                initial_grants: None,
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: "lockout-attempt-1".to_string(),
            },
        )
        .await
        .expect_err("an unconfirmed self-lockout must be rejected");
        assert_eq!(err.kind(), ApiErrorKind::PolicyRejected, "got {err:?}");
        assert!(
            inherit_flag(&scratch.db, child).await,
            "the rejected boundary must not have been written"
        );
        assert_eq!(
            authz::read_epoch(&scratch.db, fx.workspace_id).await.expect("epoch"),
            epoch_before,
            "a rejected authorization change must not advance the epoch"
        );
        assert_eq!(
            event_count(&scratch.db, fx.workspace_id).await,
            events_before,
            "a rejected authorization change must not write an audit event"
        );

        // (2) The §4.1 point 2 escape hatch: boundary + initial grants in one transaction. The
        //     caller keeps `full_access` in the post-state, so the guard never fires -- no
        //     confirmation needed, and no intermediate state where nobody administers the subtree.
        let kept = set_inheritance(
            &state,
            fx.workspace_id,
            SetInheritanceInput {
                object_id: child,
                caller: user(fx.member_id, "member"),
                inherit_from_parent: false,
                initial_grants: Some(vec![grant_of("user", fx.member_id, "full_access")]),
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("boundary + initial grants in one transaction must not trip the guard");
        assert!(!kept.inherit_from_parent);
        assert!(!kept.permission_changes.caller.loses_full_access);
        assert_eq!(kept.permission_changes.caller.after_level, "full_access");
        assert_eq!(
            level_for(&scratch.db, &fx, child, &member).await,
            PermissionLevel::FullAccess,
            "the caller kept its own explicit grant at the boundary"
        );

        // (3) Now clearing the grants *is* a self-lockout, and needs the flag.
        let err = set_grants(
            &state,
            fx.workspace_id,
            SetGrantsInput {
                object_id: child,
                caller: user(fx.member_id, "member"),
                grants: Vec::new(),
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: "lockout-attempt-2".to_string(),
            },
        )
        .await
        .expect_err("clearing the only grant under a boundary is a self-lockout");
        assert_eq!(err.kind(), ApiErrorKind::PolicyRejected, "got {err:?}");
        assert_eq!(
            grant_count(&scratch.db, child).await,
            1,
            "the rejected clear must not have deleted anything"
        );

        // ...and with the flag it goes through, honestly reported.
        let applied = set_grants(
            &state,
            fx.workspace_id,
            SetGrantsInput {
                object_id: child,
                caller: user(fx.member_id, "member"),
                grants: Vec::new(),
                confirm_self_lockout: true,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("a confirmed self-lockout is a legitimate hand-over");
        assert!(applied.applied);
        assert!(applied.permission_changes.caller.loses_full_access);
        assert_eq!(applied.permission_changes.caller.before_level, "full_access");
        assert_eq!(applied.permission_changes.caller.after_level, "none");
        assert_eq!(
            level_for(&scratch.db, &fx, child, &member).await,
            PermissionLevel::Denied,
            "the caller really is locked out now"
        );

        // (4) §4.1 point 3: the admin rescue path is unaffected by the boundary.
        let owner = user(fx.owner_id, "owner");
        assert_eq!(
            level_for(&scratch.db, &fx, child, &owner).await,
            PermissionLevel::FullAccess,
            "workspace admin fallback must survive any boundary"
        );
        set_grants(
            &state,
            fx.workspace_id,
            SetGrantsInput {
                object_id: child,
                caller: user(fx.owner_id, "owner"),
                grants: vec![grant_of("user", fx.member_id, "full_access")],
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("the admin rescue must be able to restore the locked-out caller");
        assert_eq!(
            level_for(&scratch.db, &fx, child, &member).await,
            PermissionLevel::FullAccess,
            "rescued"
        );

        scratch.drop_self().await;
    }

    /// `rest-api-v1.md`'s `dry_run`: the same summary, and nothing written -- not a row, not an
    /// event, not the epoch, not even the idempotency key (the same key must still work for the
    /// real request afterwards).
    #[tokio::test]
    async fn a_dry_run_returns_the_same_summary_and_writes_nothing() {
        let scratch = scratch_or_skip!("dry_run");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;

        let page = create(&state, &fx, "page", None).await;
        let key = "shared-idempotency-key".to_string();
        let epoch_before = authz::read_epoch(&scratch.db, fx.workspace_id).await.expect("epoch");
        let events_before = event_count(&scratch.db, fx.workspace_id).await;

        let preview = set_grants(
            &state,
            fx.workspace_id,
            SetGrantsInput {
                object_id: page,
                caller: user(fx.owner_id, "owner"),
                grants: vec![grant_of("user", fx.member_id, "full_access")],
                confirm_self_lockout: false,
                dry_run: true,
                idempotency_key: key.clone(),
            },
        )
        .await
        .expect("a dry run by a full_access caller succeeds");
        assert!(!preview.applied);
        assert!(preview.event_id.is_none());
        assert_eq!(grant_count(&scratch.db, page).await, 0, "a dry run wrote a grant row");
        assert_eq!(
            authz::read_epoch(&scratch.db, fx.workspace_id).await.expect("epoch"),
            epoch_before,
            "a dry run advanced the epoch"
        );
        assert_eq!(
            event_count(&scratch.db, fx.workspace_id).await,
            events_before,
            "a dry run wrote an event"
        );

        // The same key still works, proving it was never consumed.
        let real = set_grants(
            &state,
            fx.workspace_id,
            SetGrantsInput {
                object_id: page,
                caller: user(fx.owner_id, "owner"),
                grants: vec![grant_of("user", fx.member_id, "full_access")],
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: key,
            },
        )
        .await
        .expect("the real request runs with the key the dry run used");
        assert!(real.applied);
        assert!(real.event_id.is_some());
        // Field-by-field parity between preview and execution.
        assert_eq!(
            serde_json::to_value(&preview.permission_changes).expect("serializes"),
            serde_json::to_value(&real.permission_changes).expect("serializes"),
            "the preview must be the same summary the real request produces"
        );

        // A caller without `full_access` cannot use `dry_run` as a probe surface.
        set_inheritance(
            &state,
            fx.workspace_id,
            SetInheritanceInput {
                object_id: page,
                caller: user(fx.owner_id, "owner"),
                inherit_from_parent: false,
                initial_grants: None,
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("owner sets a boundary");
        set_grants(
            &state,
            fx.workspace_id,
            SetGrantsInput {
                object_id: page,
                caller: user(fx.owner_id, "owner"),
                grants: vec![grant_of("user", fx.member_id, "view")],
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("owner downgrades the member to view");
        match set_grants(
            &state,
            fx.workspace_id,
            SetGrantsInput {
                object_id: page,
                caller: user(fx.member_id, "member"),
                grants: Vec::new(),
                confirm_self_lockout: false,
                dry_run: true,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        {
            Err(ApiError::Forbidden(_)) => {}
            Ok(_) => panic!("dry_run became a permission probe for a caller without full_access"),
            Err(other) => panic!("expected Forbidden, got {other:?}"),
        }

        scratch.drop_self().await;
    }

    // -----------------------------------------------------------------------------------------
    // 5. `authz_epoch` linearization: the write path takes the conflicting lock
    // -----------------------------------------------------------------------------------------

    /// ★ `ADR-0012` §3.1 point 2: "授权类变更取**冲突锁**（`FOR UPDATE`）推进 epoch"，
    /// 与内容写的 `FOR SHARE` "不可能交叉成功".
    ///
    /// Both halves are asserted against the *production* authorization path, not a stand-in:
    /// - while a content write holds `fence_epoch_for_share`, a real `set_grants` **blocks** —
    ///   which is only true if `lock_epoch_for_update` really takes a conflicting lock rather
    ///   than reading the row;
    /// - once a revocation has committed, a write that checked permission against the older epoch
    ///   is rejected by the fence and lands nothing.
    #[tokio::test]
    async fn a_revocation_and_an_in_flight_write_cannot_interleave() {
        let scratch = scratch_or_skip!("epoch_fencing");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;
        let member = user(fx.member_id, "member");

        let page = create(&state, &fx, "page", None).await;
        // A boundary plus an explicit grant: the member's access now comes from one row, so
        // deleting it is a real revocation.
        set_inheritance(
            &state,
            fx.workspace_id,
            SetInheritanceInput {
                object_id: page,
                caller: user(fx.owner_id, "owner"),
                inherit_from_parent: false,
                initial_grants: Some(vec![grant_of("user", fx.member_id, "full_access")]),
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("owner restricts the page and grants the member full_access");
        assert_eq!(
            level_for(&scratch.db, &fx, page, &member).await,
            PermissionLevel::FullAccess
        );

        // ---- half 1: mutual exclusion ----
        let checked_epoch = authz::read_epoch(&scratch.db, fx.workspace_id).await.expect("epoch");
        let writer = scratch
            .db
            .begin()
            .await
            .expect("the content write opens its transaction");
        authz::fence_epoch_for_share(&writer, fx.workspace_id, checked_epoch)
            .await
            .expect("the fence matches the epoch it checked against");

        // `B` on its own pool: the real production revocation path.
        let admin_url = std::env::var(TEST_DATABASE_URL_ENV).expect("checked by scratch_or_skip!");
        let db_url = admin_url
            .rsplit_once('/')
            .map(|(prefix, _)| format!("{prefix}/{}", scratch.name))
            .expect("db url");
        let state_b = state_for(Database::connect(&db_url).await.expect("B connects independently"));
        let workspace_id = fx.workspace_id;
        let owner_id = fx.owner_id;
        let revoker = tokio::spawn(async move {
            set_grants(
                &state_b,
                workspace_id,
                SetGrantsInput {
                    object_id: page,
                    caller: user(owner_id, "owner"),
                    grants: Vec::new(),
                    confirm_self_lockout: false,
                    dry_run: false,
                    idempotency_key: Uuid::new_v4().to_string(),
                },
            )
            .await
        });

        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(
            !revoker.is_finished(),
            "an authorization change committed while a content write held the epoch row `FOR \
             SHARE` -- `lock_epoch_for_update` is not taking a conflicting lock, so the two can \
             interleave and a revoked write can land"
        );

        writer.rollback().await.expect("the content write releases the fence");
        revoker
            .await
            .expect("revoker task joins")
            .expect("the revocation succeeds once the writer released");

        // ---- half 2: the stale permission must not land ----
        assert_eq!(
            level_for(&scratch.db, &fx, page, &member).await,
            PermissionLevel::Denied,
            "the revocation really removed the member's access"
        );
        let late_writer = scratch.db.begin().await.expect("a second write opens its transaction");
        let fenced = authz::fence_epoch_for_share(&late_writer, fx.workspace_id, checked_epoch).await;
        match fenced {
            Err(ApiError::Conflict(_)) => {}
            Ok(()) => panic!(
                "a write whose permission was checked against the pre-revocation epoch passed the \
                 fence -- the revoked write would have landed"
            ),
            Err(other) => panic!("expected Conflict from the fence, got {other:?}"),
        }
        late_writer.rollback().await.expect("rolls back");

        // End to end: the member's command is refused and nothing changed.
        assert_policy_rejected(
            &run_command(&state, page, &member, "archive").await,
            "a command by a principal whose grant was revoked",
        );
        assert_eq!(lifecycle_of(&scratch.db, page).await, "active");

        scratch.drop_self().await;
    }

    // -----------------------------------------------------------------------------------------
    // 6. `initial_grants` replaces, it does not merge
    // -----------------------------------------------------------------------------------------

    /// ★ `ADR-0012` §4.1 point 2, ruled 2026-08-31: "**`initial_grants` 的语义 = 替换，不是合并**
    /// ... 提交后该对象的显式 grant 集合恒等于本次给出的集合".
    ///
    /// This is a security assertion, not a taste one. A caller draws an authorization boundary
    /// (`inherit_from_parent=false`) *and* names who may live behind it in the same transaction;
    /// the intent is "from here down, only these principals". Merging kept every grant that
    /// predated the boundary, so the boundary cut the inheritance but not the leak it existed to
    /// stop. Three layers are asserted, because only the third is the actual harm:
    /// - the row layer: the roster afterwards is *exactly* the requested list;
    /// - the permission layer: a principal dropped from the list evaluates to `Denied`, not to a
    ///   downgraded-but-present level;
    /// - the behaviour layer: that principal's commands are refused.
    ///
    /// The audit trail is asserted too: a dropped principal must produce a
    /// `flow.permission.revoked` event, since a silent deletion is its own defect.
    #[tokio::test]
    async fn initial_grants_replaces_the_roster_and_cuts_the_principals_it_omits() {
        let scratch = scratch_or_skip!("initial_grants_replace");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;
        let member = user(fx.member_id, "member");
        let carol = Uuid::new_v4();

        let page = create(&state, &fx, "page", None).await;

        // Two explicit grants that predate the boundary: `A` = the member (`full_access`),
        // `B` = a bot (`edit`).
        set_grants(
            &state,
            fx.workspace_id,
            SetGrantsInput {
                object_id: page,
                caller: user(fx.owner_id, "owner"),
                grants: vec![
                    grant_of("user", fx.member_id, "full_access"),
                    grant_of("bot", fx.bot_id, "edit"),
                ],
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("owner seeds two explicit grants");
        assert_eq!(
            explicit_roster(&scratch.db, page).await,
            vec![
                ("bot".to_string(), fx.bot_id, "edit".to_string()),
                ("user".to_string(), fx.member_id, "full_access".to_string()),
            ],
            "fixture premise: two explicit grants exist before the boundary"
        );
        assert_eq!(
            level_for(&scratch.db, &fx, page, &member).await,
            PermissionLevel::FullAccess,
            "fixture premise: the member's explicit grant is live"
        );
        run_command(&state, page, &member, "archive")
            .await
            .expect("fixture premise: the member can act on the page before the boundary");
        run_command(&state, page, &member, "restore")
            .await
            .expect("...and restore it");

        // The boundary, carrying the complete post-boundary roster: only `C` may remain.
        set_inheritance(
            &state,
            fx.workspace_id,
            SetInheritanceInput {
                object_id: page,
                caller: user(fx.owner_id, "owner"),
                inherit_from_parent: false,
                initial_grants: Some(vec![grant_of("bot", carol, "view")]),
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("owner draws the boundary and names its roster in one transaction");

        // (1) Rows: exactly the requested list. Under the merge semantics this held three rows.
        assert!(!inherit_flag(&scratch.db, page).await);
        assert_eq!(
            explicit_roster(&scratch.db, page).await,
            vec![("bot".to_string(), carol, "view".to_string())],
            "`initial_grants` replaces: the grants that predate the boundary must be gone"
        );

        // (2) Permission: `Denied`, not a downgrade. `ADR-0012` has already ruled that behind a
        //     boundary "no grant" means no access at all.
        assert_eq!(
            level_for(&scratch.db, &fx, page, &member).await,
            PermissionLevel::Denied,
            "the member kept access across the boundary -- this is the cross-boundary leak"
        );
        assert_eq!(
            level_for(&scratch.db, &fx, page, &bot(fx.bot_id, "member")).await,
            PermissionLevel::Denied,
            "the bot kept access across the boundary"
        );
        assert_eq!(
            level_for(&scratch.db, &fx, page, &bot(carol, "member")).await,
            PermissionLevel::View,
            "the principal the request *did* name must hold exactly the level it was given"
        );

        // (3) Behaviour: the leak's actual surface.
        assert_policy_rejected(
            &run_command(&state, page, &member, "archive").await,
            "a principal dropped by a replacing `initial_grants`",
        );

        // (4) Audit: both drops are recorded, so the deletion is never silent.
        let mut expected = vec![fx.member_id, fx.bot_id];
        expected.sort_unstable();
        assert_eq!(
            revoked_principals(&scratch.db, fx.workspace_id).await,
            expected,
            "a replaced-away grant must emit `flow.permission.revoked`"
        );

        scratch.drop_self().await;
    }

    /// ★ Omitted (`None`) and empty (`Some(vec![])`) are different requests.
    ///
    /// `rest-api-v1.md` spells the wire field `initial_grants?`, so a plain boundary flip carries
    /// no list at all; combined with replacement semantics, treating "absent" as "empty" would
    /// make `PUT .../inheritance {inherit_from_parent}` silently wipe the roster. The domain type
    /// is therefore `Option<Vec<_>>`, and both halves are asserted here — plus the wire layer, so
    /// the distinction cannot be lost in `serde` on the way in.
    #[tokio::test]
    async fn an_omitted_initial_grants_list_is_not_an_empty_one() {
        // The wire layer first: this is what makes the domain distinction reachable at all.
        let omitted: crate::routes::flow::SetInheritanceRequest =
            serde_json::from_str(r#"{"inherit_from_parent":false,"idempotency_key":"k"}"#)
                .expect("a boundary flip may omit initial_grants");
        assert!(
            omitted.initial_grants.is_none(),
            "an omitted `initial_grants` must not deserialize into an empty list"
        );
        let emptied: crate::routes::flow::SetInheritanceRequest =
            serde_json::from_str(r#"{"inherit_from_parent":false,"initial_grants":[],"idempotency_key":"k"}"#)
                .expect("an explicit empty list is a legal request");
        assert_eq!(
            emptied.initial_grants.map(|grants| grants.len()),
            Some(0),
            "an explicit `initial_grants: []` must survive as `Some(empty)`"
        );

        let scratch = scratch_or_skip!("initial_grants_absent");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;
        let member = user(fx.member_id, "member");

        let page = create(&state, &fx, "page", None).await;
        set_grants(
            &state,
            fx.workspace_id,
            SetGrantsInput {
                object_id: page,
                caller: user(fx.owner_id, "owner"),
                grants: vec![
                    grant_of("user", fx.member_id, "full_access"),
                    grant_of("bot", fx.bot_id, "edit"),
                ],
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("owner seeds two explicit grants");
        let seeded = explicit_roster(&scratch.db, page).await;
        assert_eq!(seeded.len(), 2, "fixture premise");
        let revoked_before = revoked_principals(&scratch.db, fx.workspace_id).await;

        // `None`: flip the boundary, touch nothing else.
        set_inheritance(
            &state,
            fx.workspace_id,
            SetInheritanceInput {
                object_id: page,
                caller: user(fx.owner_id, "owner"),
                inherit_from_parent: false,
                initial_grants: None,
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("a boundary flip with no list is legal");
        assert!(!inherit_flag(&scratch.db, page).await);
        assert_eq!(
            explicit_roster(&scratch.db, page).await,
            seeded,
            "an omitted `initial_grants` must leave every existing row exactly as it was"
        );
        assert_eq!(
            level_for(&scratch.db, &fx, page, &member).await,
            PermissionLevel::FullAccess,
            "the untouched grant is still live"
        );
        assert_eq!(
            revoked_principals(&scratch.db, fx.workspace_id).await,
            revoked_before,
            "a request that deleted nothing must not claim a revocation in the audit trail"
        );

        // `Some(vec![])`: the explicit "clear the roster" request.
        set_inheritance(
            &state,
            fx.workspace_id,
            SetInheritanceInput {
                object_id: page,
                caller: user(fx.owner_id, "owner"),
                inherit_from_parent: false,
                initial_grants: Some(Vec::new()),
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("an explicit empty roster is legal for a caller the admin fallback keeps");
        assert_eq!(
            explicit_roster(&scratch.db, page).await,
            Vec::new(),
            "`initial_grants: []` must clear every explicit grant"
        );
        assert_eq!(
            level_for(&scratch.db, &fx, page, &member).await,
            PermissionLevel::Denied,
            "behind the boundary, a cleared roster leaves the member with nothing"
        );

        scratch.drop_self().await;
    }

    // -----------------------------------------------------------------------------------------
    // Limits
    // -----------------------------------------------------------------------------------------

    /// `limits-v1.md`'s frozen `grants_per_request_max = 100` and `object_grants_max = 100`.
    ///
    /// Since `ADR-0012` §4.1 point 2 was ruled (2026-08-31), **both** write surfaces are
    /// whole-table replacements, so on both of them the result count *is* the request count and
    /// the per-request ceiling already bounds the roster. That is exactly the structural fact
    /// `limits-v1.md` freezes `object_grants_max` on ("PUT .../grants 是整表替换 ... 结果条数恒等于
    /// 请求条数"), and the same note records that `initial_grants` "曾是唯一可能越过本上限的路径;
    /// 定为合并会抽掉上面冻结论证的地基". It is asserted here rather than left as prose: the
    /// per-request ceiling is enforced on `initial_grants` too, and a one-entry `initial_grants`
    /// against a full roster leaves one row instead of accumulating a hundred-and-first.
    #[tokio::test]
    async fn grant_count_ceilings_are_enforced_and_never_silently_truncate() {
        let scratch = scratch_or_skip!("grant_limits");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;
        let page = create(&state, &fx, "page", None).await;

        let full: Vec<GrantRequest> = (0..GRANTS_PER_REQUEST_MAX)
            .map(|_| grant_of("bot", Uuid::new_v4(), "view"))
            .collect();
        let mut over = full.clone();
        over.push(grant_of("bot", Uuid::new_v4(), "view"));

        // One past `grants_per_request_max`.
        let err = set_grants(
            &state,
            fx.workspace_id,
            SetGrantsInput {
                object_id: page,
                caller: user(fx.owner_id, "owner"),
                grants: over,
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect_err("101 entries must be refused");
        assert_limit_kind(&err, "grants_per_request");
        assert_eq!(grant_count(&scratch.db, page).await, 0, "nothing was written");

        // Exactly at the ceiling is accepted, and the resulting roster is exactly that size --
        // which is why a replace can never reach `object_grants_max`.
        set_grants(
            &state,
            fx.workspace_id,
            SetGrantsInput {
                object_id: page,
                caller: user(fx.owner_id, "owner"),
                grants: full,
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("exactly grants_per_request_max entries must be accepted");
        let at_ceiling = grant_count(&scratch.db, page).await;
        assert_eq!(at_ceiling, i64::try_from(OBJECT_GRANTS_MAX).expect("fits"));

        // `initial_grants` is bounded by the same per-request ceiling ("条目数同受
        // `grants_per_request_max` 约束"), and a refusal changes nothing -- neither the roster it
        // would have replaced nor the boundary flag the same transaction would have flipped.
        let mut over_boundary: Vec<GrantRequest> = (0..GRANTS_PER_REQUEST_MAX)
            .map(|_| grant_of("bot", Uuid::new_v4(), "view"))
            .collect();
        over_boundary.push(grant_of("bot", Uuid::new_v4(), "view"));
        let err = set_inheritance(
            &state,
            fx.workspace_id,
            SetInheritanceInput {
                object_id: page,
                caller: user(fx.owner_id, "owner"),
                inherit_from_parent: false,
                initial_grants: Some(over_boundary),
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect_err("101 initial_grants entries must be refused");
        assert_limit_kind(&err, "grants_per_request");
        assert_eq!(
            grant_count(&scratch.db, page).await,
            at_ceiling,
            "\u{201c}达到上限即拒绝新增条目，不静默截断已有授予\u{201d}"
        );
        assert!(
            inherit_flag(&scratch.db, page).await,
            "the refused transaction rolled back whole"
        );

        // And the path that used to walk past `object_grants_max` no longer exists: one entry
        // submitted against a full roster *replaces* it. Under the old merge semantics this
        // request was refused with `limit_exceeded{object_grants}` at 101 rows.
        let survivor = Uuid::new_v4();
        set_inheritance(
            &state,
            fx.workspace_id,
            SetInheritanceInput {
                object_id: page,
                caller: user(fx.owner_id, "owner"),
                inherit_from_parent: false,
                initial_grants: Some(vec![grant_of("bot", survivor, "view")]),
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("a replacing initial_grants against a full roster cannot exceed the ceiling");
        assert_eq!(
            explicit_roster(&scratch.db, page).await,
            vec![("bot".to_string(), survivor, "view".to_string())],
            "the result count is the request count on this surface too"
        );

        scratch.drop_self().await;
    }

    /// The `GET` roster is only visible to a `full_access` caller; everyone else sees their own
    /// effective level and the boundary flag, and nothing that would enumerate other principals.
    #[tokio::test]
    async fn the_grants_roster_is_only_readable_with_full_access() {
        let scratch = scratch_or_skip!("grants_read");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;
        let parent = create(&state, &fx, "page", None).await;
        let child = create(&state, &fx, "page", Some(parent)).await;

        set_grants(
            &state,
            fx.workspace_id,
            SetGrantsInput {
                object_id: parent,
                caller: user(fx.owner_id, "owner"),
                grants: vec![grant_of("user", fx.member_id, "edit")],
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("owner grants on the parent");

        let as_owner = get_grants(&state, fx.workspace_id, child, &user(fx.owner_id, "owner"))
            .await
            .expect("owner reads");
        assert_eq!(as_owner.effective_level, "full_access");
        assert!(as_owner.inherit_from_parent);
        assert_eq!(
            as_owner.inherited.len(),
            1,
            "the ancestor grant must appear in `inherited`"
        );

        let as_member = get_grants(&state, fx.workspace_id, child, &user(fx.member_id, "member"))
            .await
            .expect("member reads its own effective level");
        assert_eq!(as_member.effective_level, "edit");
        assert!(
            as_member.items.is_empty() && as_member.inherited.is_empty(),
            "a non-full_access caller must not be able to enumerate the roster"
        );

        scratch.drop_self().await;
    }

    // -----------------------------------------------------------------------------------------
    // `object_grants_max` measurement harness (the evidence behind the proposed value)
    // -----------------------------------------------------------------------------------------

    /// Measures what `object_grants_max` actually costs, so the proposed value rests on numbers
    /// rather than on a guess: a chain at the frozen `tree_depth_max` (33 nodes, depths 0..=32)
    /// with `n` grants on **every** node, timing the two reads the ceiling bounds --
    /// `effective_permission` (one principal across the whole chain) and the `GET .../grants`
    /// roster (every principal on every contributing ancestor).
    ///
    /// The assertion is deliberately loose; the point of this test is the printed table, which the
    /// accompanying report quotes. What it does gate is that neither read degrades超 the
    /// `collab_accepted_round_trip_ms_p95_max = 250 ms` envelope the authorization read sits
    /// inside.
    #[tokio::test]
    async fn object_grants_max_read_cost_is_measured_at_the_frozen_chain_depth() {
        // 33 nodes = depths 0..=32, the deepest chain `tree_depth_max` allows.
        const CHAIN_NODES: usize = 33;
        /// Repetitions each timing averages over.
        const ROUNDS: u32 = 20;

        let scratch = scratch_or_skip!("grants_cost");
        let fx = seed_workspace(&scratch.db).await;
        let mut ids = Vec::with_capacity(CHAIN_NODES);
        let mut parent: Option<Uuid> = None;
        for _ in 0..CHAIN_NODES {
            let id = Uuid::new_v4();
            exec(
                &scratch.db,
                "INSERT INTO flow_objects (id, workspace_id, object_type, parent_id) VALUES ($1, $2, 'page', $3)",
                vec![id.into(), fx.workspace_id.into(), parent.into()],
            )
            .await;
            ids.push(id);
            parent = Some(id);
        }
        let leaf = *ids.last().expect("chain is not empty");

        println!("grants_per_node | effective_permission_ms | roster_rows | roster_ms");
        let mut worst_effective = 0.0_f64;
        let mut worst_roster = 0.0_f64;
        let mut placed = 0usize;
        for target in [0usize, 10, 50, 100, 200] {
            while placed < target {
                for &node in &ids {
                    exec(
                        &scratch.db,
                        "INSERT INTO flow_object_grants (workspace_id, object_id, principal_kind, principal_id, level) \
                         VALUES ($1, $2, 'bot', $3, 'view')",
                        vec![fx.workspace_id.into(), node.into(), Uuid::new_v4().into()],
                    )
                    .await;
                }
                placed += 1;
            }

            // Warm, then measure: the first call also pays for plan caching.
            for _ in 0..3 {
                let _ = authz::effective_permission(&scratch.db, fx.workspace_id, leaf, "user", fx.member_id, "member")
                    .await
                    .expect("resolves");
            }
            let started = std::time::Instant::now();
            for _ in 0..ROUNDS {
                let _ = authz::effective_permission(&scratch.db, fx.workspace_id, leaf, "user", fx.member_id, "member")
                    .await
                    .expect("resolves");
            }
            let effective_ms = started.elapsed().as_secs_f64() * 1000.0 / f64::from(ROUNDS);

            let started = std::time::Instant::now();
            let roster = super::grants_on(&scratch.db, &ids).await.expect("roster reads");
            let roster_ms = started.elapsed().as_secs_f64() * 1000.0;

            println!(
                "{target:>15} | {effective_ms:>23.3} | {:>11} | {roster_ms:>9.3}",
                roster.len()
            );
            worst_effective = worst_effective.max(effective_ms);
            worst_roster = worst_roster.max(roster_ms);
        }

        // The authorization read lives inside `collab_accepted_round_trip_ms_p95_max = 250 ms`.
        assert!(
            worst_effective < 250.0,
            "effective_permission at the frozen chain depth took {worst_effective:.3} ms"
        );
        assert!(
            worst_roster < 250.0,
            "the full chain roster read took {worst_roster:.3} ms"
        );

        scratch.drop_self().await;
    }

    // -----------------------------------------------------------------------------------------
    // `events-v1.md` origin / correlation / causation, for both authorization commands
    // -----------------------------------------------------------------------------------------

    /// `v0.5-collaboration.md`: "审计使用 `events-v1.md` 的同一 `business_events` fact，保存认证
    /// actor、origin(surface/session/tool)、causation".
    ///
    /// Both `PUT` endpoints, each run from a surface REST could never produce, so an assertion on
    /// the committed `source` can only pass if the producer copied the caller's declaration.
    /// `set_inheritance` is run with an `initial_grants` roster so it writes **several** events in
    /// one command, which is what makes the correlation/causation half of the contract checkable
    /// here at all.
    #[tokio::test]
    async fn both_authorization_commands_stamp_the_callers_surface_and_share_one_correlation() {
        let scratch = scratch_or_skip!("grants_origin");
        let state = state_for(scratch.db.clone());
        let fx = seed_workspace(&scratch.db).await;
        let page = create(&state, &fx, "page", None).await;

        // ---- PUT .../grants, from a CLI tools-call surface ----
        let grants_origin = CommandOrigin::first_request(
            EventSource::new(EventSurface::CliToolsCall)
                .with_tool("objects.grants_set")
                .with_session("cli-session-3"),
        );
        let grants_correlation = grants_origin.correlation_id;
        set_grants(
            &state,
            fx.workspace_id,
            SetGrantsInput {
                object_id: page,
                caller: user_from(fx.owner_id, "owner", grants_origin),
                grants: vec![grant_of("user", fx.member_id, "full_access")],
                confirm_self_lockout: false,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("set_grants succeeds");

        let rows = events_with_correlation(&scratch.db, grants_correlation).await;
        assert!(
            !rows.is_empty(),
            "set_grants must record its permission transitions under the request's correlation"
        );
        let expected_source = serde_json::json!({
            "surface": "cli_tools_call",
            "tool": "objects.grants_set",
            "session": "cli-session-3",
        });
        for row in &rows {
            assert!(
                row.event_type.starts_with("flow.permission."),
                "unexpected event type {}",
                row.event_type
            );
            assert_eq!(
                row.source, expected_source,
                "'{}' was written with source {:?}, but the caller declared {:?}",
                row.event_type, row.source, expected_source
            );
        }

        // ---- PUT .../inheritance, from an MCP HTTP surface, writing several events ----
        let inheritance_origin = CommandOrigin::first_request(
            EventSource::new(EventSurface::McpHttp)
                .with_tool("objects.inheritance_set")
                .with_request("json-rpc-11"),
        );
        let inheritance_correlation = inheritance_origin.correlation_id;
        set_inheritance(
            &state,
            fx.workspace_id,
            SetInheritanceInput {
                object_id: page,
                caller: user_from(fx.owner_id, "owner", inheritance_origin),
                inherit_from_parent: false,
                // Replaces the roster set above: the boundary flip *and* the grant change are two
                // transitions of one command.
                initial_grants: Some(vec![grant_of("user", fx.owner_id, "full_access")]),
                confirm_self_lockout: true,
                dry_run: false,
                idempotency_key: Uuid::new_v4().to_string(),
            },
        )
        .await
        .expect("set_inheritance succeeds");

        let rows = events_with_correlation(&scratch.db, inheritance_correlation).await;
        assert!(
            rows.len() >= 2,
            "this request changes the boundary and the roster, so it writes more than one event; \
             got {:?}",
            rows.iter().map(|row| row.event_type.as_str()).collect::<Vec<_>>()
        );
        let expected_source = serde_json::json!({
            "surface": "mcp_http",
            "tool": "objects.inheritance_set",
            "request": "json-rpc-11",
        });
        for row in &rows {
            assert_eq!(
                row.source, expected_source,
                "'{}' was written with source {:?}, but the caller declared {:?}",
                row.event_type, row.source, expected_source
            );
        }

        // One request, one correlation, and the primary event is the causal root the rest hang
        // off (`events-v1.md`: "由 command ... 导出的下一事件把直接父 event id 写 causation_id").
        let distinct: std::collections::BTreeSet<Option<Uuid>> = rows.iter().map(|row| row.correlation_id).collect();
        assert_eq!(
            distinct,
            std::iter::once(Some(inheritance_correlation)).collect(),
            "every event of one request shares exactly one correlation"
        );

        // The primary event is the one carrying the request's `idempotency_key`, **not** the
        // first row back: all of this command's rows share one transaction timestamp, so any
        // ordering by `created_at` falls through to a random UUID tiebreak and would make this
        // test's verdict depend on which UUID happened to sort first.
        let primaries: Vec<&EventRow> = rows.iter().filter(|row| row.idempotency_key.is_some()).collect();
        assert_eq!(
            primaries.len(),
            1,
            "exactly one event of a command carries the request's idempotency key"
        );
        let primary = primaries[0];
        assert_eq!(
            primary.causation_id, None,
            "the command's primary event roots the chain for a first user request"
        );
        let derived: Vec<&EventRow> = rows.iter().filter(|row| row.id != primary.id).collect();
        assert!(
            !derived.is_empty(),
            "this request wrote more than its primary event, so there is a derived one to check"
        );
        for row in derived {
            assert_eq!(
                row.causation_id,
                Some(primary.id),
                "'{}' must name the command's primary event as its causation",
                row.event_type
            );
        }

        scratch.drop_self().await;
    }
}
