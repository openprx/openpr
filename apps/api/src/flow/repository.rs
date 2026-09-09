// Local SQL row types stay beside the queries whose column shapes they mirror, matching the
// convention `apps/api/src/routes/label.rs` and every module past migration 0024 already follow:
// hand-written parameterized SQL, no SeaORM entities.
#![allow(clippy::items_after_statements, clippy::too_long_first_doc_paragraph)]

use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use sea_orm::{
    AccessMode, ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, IsolationLevel, Statement,
    TransactionTrait,
};
use serde_json::Value;
use uuid::Uuid;

use crate::error::ApiError;

/// One `flow_objects` row joined with its (always-present, see `command::create_object`) document
/// and projection. The source row shape for [`super::model::FlowObjectView`].
#[derive(Debug, FromQueryResult)]
pub struct ObjectViewRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    pub object_type: String,
    pub lifecycle_status: String,
    pub governance_metadata: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
    pub document_id: Uuid,
    pub document_seq: i64,
    pub document_frontier: Vec<u8>,
    pub projection_title: String,
    pub projection_state: Value,
    pub projection_document_seq: i64,
}

const OBJECT_VIEW_SELECT: &str = r"
    SELECT
        fo.id, fo.workspace_id, fo.project_id, fo.parent_id, fo.object_type, fo.lifecycle_status,
        fo.governance_metadata, fo.created_at, fo.updated_at, fo.archived_at,
        cd.id AS document_id, cd.head_seq AS document_seq, cd.head_frontier AS document_frontier,
        p.title AS projection_title, p.state AS projection_state,
        p.document_seq AS projection_document_seq
    FROM flow_objects fo
    INNER JOIN collab_documents cd ON cd.object_id = fo.id
    INNER JOIN flow_object_projections p ON p.object_id = fo.id
";

pub async fn fetch_object_view<C: ConnectionTrait>(
    conn: &C,
    object_id: Uuid,
) -> Result<Option<ObjectViewRow>, ApiError> {
    let sql = format!("{OBJECT_VIEW_SELECT} WHERE fo.id = $1");
    Ok(ObjectViewRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        vec![object_id.into()],
    ))
    .one(conn)
    .await?)
}

/// `workspace_id` for an object, used by object-scoped endpoints (no `workspace_id` path segment)
/// to run the same `require_flow_workspace_access` gate as the workspace-scoped endpoints.
pub async fn fetch_object_workspace<C: ConnectionTrait>(conn: &C, object_id: Uuid) -> Result<Option<Uuid>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        workspace_id: Uuid,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM flow_objects WHERE id = $1",
        vec![object_id.into()],
    ))
    .one(conn)
    .await?;
    Ok(row.map(|r| r.workspace_id))
}

/// Immutable accepted update needed to reconstruct the semantic state at a historical seq.
#[derive(Debug, FromQueryResult)]
pub struct DiffUpdateRow {
    pub seq: i64,
    pub bytes: Vec<u8>,
    pub content_hash: String,
    pub before_frontier: Vec<u8>,
    pub after_frontier: Vec<u8>,
}

/// One MVCC-consistent view of an object's document head and all retained updates through the
/// caller's requested upper sequence.
pub struct DiffHistoryRows {
    pub workspace_id: Uuid,
    pub document_id: Uuid,
    pub snapshot: Vec<u8>,
    pub snapshot_seq: i64,
    pub snapshot_frontier: Vec<u8>,
    pub head_seq: i64,
    pub head_frontier: Vec<u8>,
    pub updates: Vec<DiffUpdateRow>,
}

/// Loads the data needed by `GET .../diff` in one `REPEATABLE READ READ ONLY` snapshot.
///
/// v0.5 retains every `collab_updates` row, including rows older than the document's advanced
/// snapshot pointer. Those rows provide the exact seq-to-frontier index while Loro's full
/// snapshot retains the operation history needed to fork at either frontier; using only the
/// current snapshot state would silently substitute `snapshot_seq` for older requests.
pub async fn fetch_diff_history(
    db: &DatabaseConnection,
    object_id: Uuid,
    to_seq: i64,
    row_limit: u64,
) -> Result<Option<DiffHistoryRows>, ApiError> {
    #[derive(FromQueryResult)]
    struct DocumentRow {
        workspace_id: Uuid,
        document_id: Uuid,
        snapshot: Vec<u8>,
        snapshot_seq: i64,
        snapshot_frontier: Vec<u8>,
        head_seq: i64,
        head_frontier: Vec<u8>,
    }

    let tx = db
        .begin_with_config(Some(IsolationLevel::RepeatableRead), Some(AccessMode::ReadOnly))
        .await?;
    let document = DocumentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT fo.workspace_id, cd.id AS document_id, cd.snapshot, cd.snapshot_seq, \
                cd.snapshot_frontier, cd.head_seq, cd.head_frontier \
           FROM flow_objects fo \
           JOIN collab_documents cd ON cd.object_id = fo.id \
          WHERE fo.id = $1",
        vec![object_id.into()],
    ))
    .one(&tx)
    .await?;
    let Some(document) = document else {
        tx.commit().await?;
        return Ok(None);
    };

    let history_upper = if to_seq > document.head_seq {
        0
    } else if to_seq == 0 && document.head_seq > 0 {
        1
    } else {
        to_seq
    };
    let updates = DiffUpdateRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT seq, bytes, content_hash, before_frontier, after_frontier \
           FROM collab_updates \
          WHERE document_id = $1 AND seq >= 1 AND seq <= $2 \
          ORDER BY seq ASC \
          LIMIT $3",
        vec![
            document.document_id.into(),
            history_upper.into(),
            i64::try_from(row_limit.saturating_add(1)).unwrap_or(i64::MAX).into(),
        ],
    ))
    .all(&tx)
    .await?;
    tx.commit().await?;

    Ok(Some(DiffHistoryRows {
        workspace_id: document.workspace_id,
        document_id: document.document_id,
        snapshot: document.snapshot,
        snapshot_seq: document.snapshot_seq,
        snapshot_frontier: document.snapshot_frontier,
        head_seq: document.head_seq,
        head_frontier: document.head_frontier,
        updates,
    }))
}

pub struct ListFilter {
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub unprojected: bool,
    pub object_type: Option<String>,
    pub parent_id: Option<Uuid>,
    pub title_prefix: Option<String>,
    pub include_archived: bool,
    /// Keyset cursor: `(created_at, id)` of the last row of the previous page, exclusive.
    pub after: Option<(DateTime<Utc>, Uuid)>,
    /// Already clamped to `1..=100` by the handler.
    pub limit: u64,
}

pub async fn list_objects<C: ConnectionTrait>(conn: &C, filter: &ListFilter) -> Result<Vec<ObjectViewRow>, ApiError> {
    let mut sql = format!("{OBJECT_VIEW_SELECT} WHERE fo.workspace_id = $1");
    let mut values: Vec<sea_orm::Value> = vec![filter.workspace_id.into()];

    fn push_eq(sql: &mut String, values: &mut Vec<sea_orm::Value>, column: &str, value: sea_orm::Value) {
        values.push(value);
        let _ = write!(sql, " AND {column} = ${}", values.len());
    }

    if let Some(project_id) = filter.project_id {
        push_eq(&mut sql, &mut values, "fo.project_id", project_id.into());
    } else if filter.unprojected {
        sql.push_str(" AND fo.project_id IS NULL");
    }
    if let Some(object_type) = &filter.object_type {
        push_eq(&mut sql, &mut values, "fo.object_type", object_type.clone().into());
    }
    if let Some(parent_id) = filter.parent_id {
        push_eq(&mut sql, &mut values, "fo.parent_id", parent_id.into());
    }
    if !filter.include_archived {
        sql.push_str(" AND fo.lifecycle_status != 'archived'");
    }
    if let Some(title_prefix) = &filter.title_prefix {
        values.push(format!("{}%", title_prefix.replace('%', "\\%").replace('_', "\\_")).into());
        let _ = write!(sql, " AND p.title ILIKE ${} ESCAPE '\\'", values.len());
    }
    if let Some((created_at, id)) = filter.after {
        values.push(created_at.into());
        let created_at_param = values.len();
        values.push(id.into());
        let id_param = values.len();
        let _ = write!(sql, " AND (fo.created_at, fo.id) > (${created_at_param}, ${id_param})");
    }
    values.push(i64::try_from(filter.limit).unwrap_or(i64::MAX).into());
    let _ = write!(sql, " ORDER BY fo.created_at ASC, fo.id ASC LIMIT ${}", values.len());

    Ok(
        ObjectViewRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .all(conn)
            .await?,
    )
}

/// Candidate metadata for the projection-lag endpoint. Deliberately contains no projection
/// content, document bytes, or filtering counts.
#[derive(Debug, FromQueryResult)]
pub struct ProjectionLagRow {
    pub object_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub head_seq: i64,
    pub projection_seq: i64,
}

pub struct ProjectionLagFilter {
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub after: Option<(DateTime<Utc>, Uuid)>,
    pub limit: u64,
}

/// Reads one stable keyset page of projection-lag candidates before policy filtering.
///
/// `DeclaredProject?` is fail-closed: a declared project selects exactly that project; omission
/// selects only `project_id IS NULL` objects rather than silently widening to the whole workspace.
pub async fn list_projection_lag_candidates<C: ConnectionTrait>(
    conn: &C,
    filter: &ProjectionLagFilter,
) -> Result<Vec<ProjectionLagRow>, ApiError> {
    let mut sql = String::from(
        "SELECT fo.id AS object_id, fo.created_at, cd.head_seq, p.document_seq AS projection_seq \
           FROM flow_objects fo \
           JOIN collab_documents cd ON cd.object_id = fo.id \
           JOIN flow_object_projections p ON p.object_id = fo.id \
          WHERE fo.workspace_id = $1",
    );
    let mut values: Vec<sea_orm::Value> = vec![filter.workspace_id.into()];
    if let Some(project_id) = filter.project_id {
        values.push(project_id.into());
        let _ = write!(sql, " AND fo.project_id = ${}", values.len());
    } else {
        sql.push_str(" AND fo.project_id IS NULL");
    }
    if let Some((created_at, id)) = filter.after {
        values.push(created_at.into());
        let created_at_index = values.len();
        values.push(id.into());
        let id_index = values.len();
        let _ = write!(sql, " AND (fo.created_at, fo.id) > (${created_at_index}, ${id_index})");
    }
    values.push(i64::try_from(filter.limit).unwrap_or(i64::MAX).into());
    let _ = write!(sql, " ORDER BY fo.created_at, fo.id LIMIT ${}", values.len());

    Ok(
        ProjectionLagRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .all(conn)
            .await?,
    )
}

pub async fn fetch_flow_enabled<C: ConnectionTrait>(conn: &C, workspace_id: Uuid) -> Result<bool, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        flow_enabled: bool,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT flow_enabled FROM flow_workspace_settings WHERE workspace_id = $1",
        vec![workspace_id.into()],
    ))
    .one(conn)
    .await?;
    Ok(row.is_some_and(|r| r.flow_enabled))
}

/// `Some(workspace_id)` when `project_id` exists at all, regardless of which workspace it belongs
/// to.
///
/// The caller compares it against the request's own `workspace_id` so a cross-workspace
/// `project_id` fails the same `invalid_update` way a nonexistent one does, without leaking
/// whether the id exists elsewhere.
pub async fn fetch_project_workspace<C: ConnectionTrait>(conn: &C, project_id: Uuid) -> Result<Option<Uuid>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        workspace_id: Uuid,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(conn)
    .await?;
    Ok(row.map(|r| r.workspace_id))
}

pub struct ParentRow {
    pub workspace_id: Uuid,
    pub lifecycle_status: String,
    /// The parent's project scope. `create_object` needs it because `ADR-0013` §2.2 R17 makes
    /// "a non-root object sits in its parent's project scope" an invariant, and the write path
    /// has to reject a mismatch with a decidable error instead of letting
    /// `flow_objects_parent_project_fk` answer with a 500.
    pub project_id: Option<Uuid>,
}

pub async fn fetch_parent_object<C: ConnectionTrait>(conn: &C, parent_id: Uuid) -> Result<Option<ParentRow>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        workspace_id: Uuid,
        lifecycle_status: String,
        project_id: Option<Uuid>,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id, lifecycle_status, project_id FROM flow_objects WHERE id = $1",
        vec![parent_id.into()],
    ))
    .one(conn)
    .await?;
    Ok(row.map(|r| ParentRow {
        workspace_id: r.workspace_id,
        lifecycle_status: r.lifecycle_status,
        project_id: r.project_id,
    }))
}

/// One row in the object scope a lifecycle command reasons about.
///
/// `has_shared_descendants` is repeated on every returned row deliberately: it is a fact about the
/// requested root, not an individual row. Keeping it in the same statement as the subtree snapshot
/// prevents the permission tier from being computed from a separately-timed grants query.
#[derive(Debug, Clone, PartialEq, Eq, FromQueryResult)]
pub struct LifecycleScopeRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    pub object_type: String,
    pub lifecycle_status: String,
    pub has_shared_descendants: bool,
    pub invalid_tree: bool,
}

/// Reads the one object affected by the v0.4 `archive|restore` request semantics.
///
/// The frozen command payload has no cascade operation, so descendants are not part of this
/// request's impact set merely because they exist. The boolean fact columns remain in the row
/// shape so the permission classifier can continue to describe the broader lifecycle tiers that
/// a future, explicitly registered cascade command would use.
pub async fn lifecycle_scope<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    object_id: Uuid,
    _tree_depth_max: i64,
) -> Result<Vec<LifecycleScopeRow>, ApiError> {
    Ok(LifecycleScopeRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT o.id, o.workspace_id, o.project_id, o.parent_id, o.object_type, \
                o.lifecycle_status, false AS has_shared_descendants, false AS invalid_tree \
           FROM flow_objects o \
          WHERE o.id = $1 AND o.workspace_id = $2",
        vec![object_id.into(), workspace_id.into()],
    ))
    .all(conn)
    .await?)
}

/// Applies one reversible lifecycle transition to the exact, already-locked impact set.
pub async fn set_objects_lifecycle<C: ConnectionTrait>(
    conn: &C,
    object_ids: &[Uuid],
    lifecycle_status: &str,
    archived_at: Option<DateTime<Utc>>,
    updated_by: Option<Uuid>,
) -> Result<u64, ApiError> {
    if object_ids.is_empty() {
        return Ok(0);
    }
    let result = conn
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_objects SET lifecycle_status = $2, archived_at = $3, updated_at = now(), updated_by = $4 \
             WHERE id = ANY(string_to_array($1, ',')::uuid[])",
            vec![
                uuid_list(object_ids).into(),
                lifecycle_status.into(),
                archived_at.into(),
                updated_by.into(),
            ],
        ))
        .await?;
    Ok(result.rows_affected())
}

/// A prior `business_events` row for this `(workspace_id, idempotency_key)`, if any.
///
/// Present when the key has been used before (`business_events_idempotency` unique index); the
/// create command uses this to replay idempotently instead of hitting the unique-violation path.
pub struct IdempotentEvent {
    pub id: Uuid,
    pub event_type: String,
    pub aggregate_id: String,
    pub metadata: serde_json::Value,
}

pub async fn find_idempotent_event<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    idempotency_key: &str,
) -> Result<Option<IdempotentEvent>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        id: Uuid,
        event_type: String,
        aggregate_id: String,
        metadata: serde_json::Value,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, event_type, aggregate_id, metadata FROM business_events \
         WHERE workspace_id = $1 AND idempotency_key = $2",
        vec![workspace_id.into(), idempotency_key.into()],
    ))
    .one(conn)
    .await?;
    Ok(row.map(|r| IdempotentEvent {
        id: r.id,
        event_type: r.event_type,
        aggregate_id: r.aggregate_id,
        metadata: r.metadata,
    }))
}

pub struct NewFlowObject {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub object_type: String,
    pub parent_id: Option<Uuid>,
    /// `None` when the actor is a bot (`REFERENCES users(id)`).
    pub created_by: Option<Uuid>,
}

pub async fn insert_flow_object<C: ConnectionTrait>(conn: &C, object: &NewFlowObject) -> Result<(), ApiError> {
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO flow_objects (id, workspace_id, project_id, object_type, parent_id, created_by, updated_by)
            VALUES ($1, $2, $3, $4, $5, $6, $6)
        ",
        vec![
            object.id.into(),
            object.workspace_id.into(),
            object.project_id.into(),
            object.object_type.clone().into(),
            object.parent_id.into(),
            object.created_by.into(),
        ],
    ))
    .await?;
    Ok(())
}

pub struct NewCollabDocument {
    pub id: Uuid,
    pub object_id: Uuid,
    pub format_version: String,
    pub snapshot: Vec<u8>,
    pub frontier: Vec<u8>,
}

pub async fn insert_collab_document<C: ConnectionTrait>(conn: &C, doc: &NewCollabDocument) -> Result<(), ApiError> {
    #[allow(clippy::cast_possible_wrap)]
    let byte_count = doc.snapshot.len() as i64;
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO collab_documents (
                id, object_id, engine, format_version,
                snapshot, snapshot_frontier, snapshot_seq,
                head_frontier, head_seq, byte_count, update_count
            )
            VALUES ($1, $2, 'loro', $3, $4, $5, 0, $5, 0, $6, 0)
        ",
        vec![
            doc.id.into(),
            doc.object_id.into(),
            doc.format_version.clone().into(),
            doc.snapshot.clone().into(),
            doc.frontier.clone().into(),
            byte_count.into(),
        ],
    ))
    .await?;
    Ok(())
}

pub struct NewProjection {
    pub object_id: Uuid,
    pub document_seq: i64,
    pub document_frontier: Vec<u8>,
    pub title: String,
    pub state: Value,
    pub plain_text: String,
}

pub async fn insert_projection<C: ConnectionTrait>(conn: &C, projection: &NewProjection) -> Result<(), ApiError> {
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO flow_object_projections
                (object_id, document_seq, document_frontier, title, state, plain_text, projection_version)
            VALUES ($1, $2, $3, $4, $5, $6, 1)
        ",
        vec![
            projection.object_id.into(),
            projection.document_seq.into(),
            projection.document_frontier.clone().into(),
            projection.title.clone().into(),
            projection.state.clone().into(),
            projection.plain_text.clone().into(),
        ],
    ))
    .await?;
    Ok(())
}

/// The single, domain-transaction `event_dispatch` row `events-v1.md`/`domain-model-v1.md`
/// require ("由域事务写入，且恰好一行 ... 事务内不查询订阅目录").
///
/// `document_id` and `accepted_seq` stay `NULL`: they are filled only for `flow.content.accepted`
/// (`event_dispatch_document_id_fill_check`), and `flow.object.created` is a governance event, not
/// a content update.
pub async fn insert_event_dispatch<C: ConnectionTrait>(
    conn: &C,
    event_id: Uuid,
    workspace_id: Uuid,
    event_type: &str,
    max_attempts: i32,
) -> Result<(), ApiError> {
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO event_dispatch (id, event_id, workspace_id, event_type, max_attempts)
            VALUES ($1, $2, $3, $4, $5)
        ",
        vec![
            Uuid::new_v4().into(),
            event_id.into(),
            workspace_id.into(),
            event_type.into(),
            max_attempts.into(),
        ],
    ))
    .await?;
    Ok(())
}

/// One `flow_integrity_records` row to write (`ADR-0013` §4's verbatim column list, minus `id`/
/// `detected_at`/`status`/`resolved_at`, which the table itself defaults).
pub struct IntegrityRecordInput<'a> {
    pub workspace_id: Uuid,
    /// `flow_integrity_records_kind_check`: `^[a-z][a-z0-9_]*$`.
    pub kind: &'a str,
    /// `flow_integrity_records_subject_kind_check`: `^[a-z][a-z0-9_]*$`.
    pub subject_kind: &'a str,
    pub subject_id: &'a str,
    pub detected_by: &'a str,
    /// Must never carry document/body content or CRDT bytes (`events-v1.md` redaction rules,
    /// reused here per the table's own `COMMENT ON TABLE flow_integrity_records`).
    pub details_redacted: Value,
}

/// Records an invariant drift a database constraint could not catch by itself (`ADR-0013` §4):
/// an operational fact, never a `business_events` row, never delivered or replayed. Callers write
/// this in the same fail-closed path that already rejects the request — this call never changes
/// whether the request is rejected, only whether the rejection leaves an audit trail.
pub async fn insert_integrity_record<C: ConnectionTrait>(
    conn: &C,
    input: IntegrityRecordInput<'_>,
) -> Result<Uuid, ApiError> {
    let id = Uuid::new_v4();
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO flow_integrity_records
                (id, workspace_id, kind, subject_kind, subject_id, detected_by, details_redacted)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
        ",
        vec![
            id.into(),
            input.workspace_id.into(),
            input.kind.into(),
            input.subject_kind.into(),
            input.subject_id.into(),
            input.detected_by.into(),
            input.details_redacted.into(),
        ],
    ))
    .await?;
    Ok(id)
}

/// A `flow_workspace_settings` row (`GET|PUT /workspaces/{workspace_id}/features/flow`).
#[derive(Debug, Clone, FromQueryResult)]
pub struct FlowSettingsRow {
    pub flow_enabled: bool,
    pub default_member_level: String,
    pub authz_epoch: i64,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<Uuid>,
}

/// Reads a `flow_workspace_settings` row.
///
/// `None` when the workspace has never had a row written — the handler synthesizes the column
/// defaults (`flow_enabled=false`, `default_member_level='edit'`, `authz_epoch=0`) with
/// `updated_at`/`updated_by` as `null` rather than this function inventing a timestamp for a write
/// that never happened.
pub async fn fetch_flow_settings<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
) -> Result<Option<FlowSettingsRow>, ApiError> {
    Ok(FlowSettingsRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT flow_enabled, default_member_level, authz_epoch, updated_at, updated_by \
         FROM flow_workspace_settings WHERE workspace_id = $1",
        vec![workspace_id.into()],
    ))
    .one(conn)
    .await?)
}

/// Provisions the `flow_workspace_settings` row on first write, at column defaults.
///
/// (`flow_enabled=false`, `default_member_level='edit'`), so the subsequent `FOR UPDATE` read in
/// the same transaction always finds a row regardless of whether one existed before this call.
pub async fn ensure_flow_settings_row<C: ConnectionTrait>(conn: &C, workspace_id: Uuid) -> Result<(), ApiError> {
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_workspace_settings (workspace_id) VALUES ($1) ON CONFLICT (workspace_id) DO NOTHING",
        vec![workspace_id.into()],
    ))
    .await?;
    Ok(())
}

/// Reads the settings row for update, inside the caller's transaction.
///
/// Holds the row lock [`ensure_flow_settings_row`] guarantees exists, so a concurrent `PUT` on the
/// same workspace serializes rather than racing on `flow_enabled`.
pub async fn fetch_flow_settings_for_update<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
) -> Result<Option<FlowSettingsRow>, ApiError> {
    Ok(FlowSettingsRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT flow_enabled, default_member_level, authz_epoch, updated_at, updated_by \
         FROM flow_workspace_settings WHERE workspace_id = $1 FOR UPDATE",
        vec![workspace_id.into()],
    ))
    .one(conn)
    .await?)
}

/// Writes the new `flow_enabled` and `default_member_level` values and stamps the updater.
///
/// The caller advances `authz_epoch` in the same transaction before invoking this function when
/// the baseline changes. Keeping that decision in the command layer lets a pure feature-flag
/// transition leave the authorization epoch untouched.
pub async fn update_flow_settings<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    flow_enabled: bool,
    default_member_level: &str,
    // `None` when the actor is a bot: this column is `REFERENCES users(id)` and a bot id is
    // not a user id.
    updated_by: Option<Uuid>,
) -> Result<(), ApiError> {
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_workspace_settings SET flow_enabled = $2, default_member_level = $3, \
         updated_at = now(), updated_by = $4 \
         WHERE workspace_id = $1",
        vec![
            workspace_id.into(),
            flow_enabled.into(),
            default_member_level.into(),
            updated_by.into(),
        ],
    ))
    .await?;
    Ok(())
}

pub struct HistoryFilter {
    pub document_id: Uuid,
    pub before_seq: Option<i64>,
    /// Already clamped to `1..=100` by the handler.
    pub limit: u64,
}

#[derive(Debug, FromQueryResult)]
pub struct HistoryRow {
    pub seq: i64,
    pub actor_id: Option<Uuid>,
    pub origin_surface: String,
    pub message: Option<String>,
    pub semantic_summary: Value,
    pub created_at: DateTime<Utc>,
}

/// `collab_updates` joined with the `business_events` row its `event_id` points at.
///
/// Surfaces the caller-facing `message` (`business_events.metadata->>'message'`) and a JSON
/// summary of what changed (`business_events.payload`). Always empty in this package: nothing
/// here ever inserts a `collab_updates` row (that only happens through the content-command
/// endpoint, not shipped in this package), so this query is exercised now purely so the shape is
/// right once that lands.
pub async fn fetch_history<C: ConnectionTrait>(conn: &C, filter: &HistoryFilter) -> Result<Vec<HistoryRow>, ApiError> {
    let mut sql = String::from(
        r"
            SELECT
                cu.seq, cu.actor_id, cu.origin_surface,
                be.metadata ->> 'message' AS message,
                be.payload AS semantic_summary,
                cu.created_at
            FROM collab_updates cu
            INNER JOIN business_events be ON be.id = cu.event_id
            WHERE cu.document_id = $1
        ",
    );
    let mut values: Vec<sea_orm::Value> = vec![filter.document_id.into()];
    if let Some(before_seq) = filter.before_seq {
        values.push(before_seq.into());
        let _ = write!(sql, " AND cu.seq < ${}", values.len());
    }
    values.push(i64::try_from(filter.limit).unwrap_or(i64::MAX).into());
    let _ = write!(sql, " ORDER BY cu.seq DESC LIMIT ${}", values.len());

    Ok(
        HistoryRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .all(conn)
            .await?,
    )
}

// ================================================================================================
// flow_import_jobs / flow_import_lineage (`migrations/0055_flow_import_jobs.sql`).
//
// No command/query module in this package writes or reads these yet (the ADR-0003 inventory is
// zero-row, so the importer surface itself is not required — see the migration file's header);
// these functions exist so the table has a typed data-access layer ready for whichever package
// wires the importer command up, matching how this file's other `insert_*`/`fetch_*` functions
// predate their own command wiring in this codebase's history.
// ================================================================================================

/// A `flow_import_jobs` row (`model::ImportJobView`'s source shape).
#[derive(Debug, Clone, FromQueryResult)]
pub struct ImportJobRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub kind: String,
    pub source_workspace_id: Option<Uuid>,
    pub mapping_hash: String,
    pub package_sha256: Option<String>,
    pub status: String,
    pub report: Option<Value>,
    pub error: Option<String>,
    pub idempotency_key: String,
    pub request_body_hash: String,
    pub audit_event_id: Option<Uuid>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const IMPORT_JOB_SELECT: &str = r"
    SELECT
        id, workspace_id, kind, source_workspace_id, mapping_hash, package_sha256, status, report,
        error, idempotency_key, request_body_hash, audit_event_id, started_at, finished_at,
        created_at, updated_at
    FROM flow_import_jobs
";

/// Scoped by `workspace_id` so a job id from another workspace behaves like a nonexistent one
/// (the same not-found-safe pattern `fetch_object_view`'s callers apply via `fetch_object_workspace`).
pub async fn fetch_import_job<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    job_id: Uuid,
) -> Result<Option<ImportJobRow>, ApiError> {
    Ok(ImportJobRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("{IMPORT_JOB_SELECT} WHERE id = $1 AND workspace_id = $2"),
        vec![job_id.into(), workspace_id.into()],
    ))
    .one(conn)
    .await?)
}

/// The prior job for this `(workspace_id, kind, idempotency_key)`, if any
/// (`flow_import_jobs_idempotency_key_key`). Callers compare `request_body_hash` against the new
/// request to tell an exact replay (`export-package-v1.md`: "相同 key+body ... 返回原 job") apart
/// from body drift, which is a conflict rather than a second job.
pub async fn find_import_job_by_idempotency_key<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    kind: &str,
    idempotency_key: &str,
) -> Result<Option<ImportJobRow>, ApiError> {
    Ok(ImportJobRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("{IMPORT_JOB_SELECT} WHERE workspace_id = $1 AND kind = $2 AND idempotency_key = $3"),
        vec![workspace_id.into(), kind.into(), idempotency_key.into()],
    ))
    .one(conn)
    .await?)
}

/// Fields needed to insert a new `flow_import_jobs` row at commit time. Preview never reaches this
/// function (`domain-model-v1.md`: "preview/staging 不是 canonical Flow state").
pub struct NewImportJob<'a> {
    pub id: Uuid,
    pub workspace_id: Uuid,
    /// `flow_import_jobs_kind_check`: `"legacy_pages"` or `"flow_package"`.
    pub kind: &'a str,
    pub source_workspace_id: Option<Uuid>,
    pub mapping_hash: &'a str,
    pub package_sha256: Option<&'a str>,
    pub artifact_id: Option<Uuid>,
    pub conflict_policy: Option<&'a str>,
    pub external_reference_policy: Option<&'a str>,
    pub request: Value,
    pub idempotency_key: &'a str,
    pub request_body_hash: &'a str,
    pub actor_user_id: Option<Uuid>,
}

/// Inserts the job at `status = 'pending'`; callers move it to `'running'` and then to a terminal
/// status with [`start_import_job`] / [`finish_import_job`] as the synchronous commit executes.
pub async fn insert_import_job<C: ConnectionTrait>(conn: &C, job: &NewImportJob<'_>) -> Result<(), ApiError> {
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO flow_import_jobs (
                id, workspace_id, kind, source_workspace_id, mapping_hash, package_sha256,
                artifact_id, conflict_policy, external_reference_policy, request,
                idempotency_key, request_body_hash, actor_user_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
        ",
        vec![
            job.id.into(),
            job.workspace_id.into(),
            job.kind.into(),
            job.source_workspace_id.into(),
            job.mapping_hash.into(),
            job.package_sha256.into(),
            job.artifact_id.into(),
            job.conflict_policy.into(),
            job.external_reference_policy.into(),
            job.request.clone().into(),
            job.idempotency_key.into(),
            job.request_body_hash.into(),
            job.actor_user_id.into(),
        ],
    ))
    .await?;
    Ok(())
}

/// Moves a `pending` job to `running` and stamps `started_at`.
pub async fn start_import_job<C: ConnectionTrait>(conn: &C, job_id: Uuid) -> Result<(), ApiError> {
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_import_jobs SET status = 'running', started_at = now(), updated_at = now() WHERE id = $1",
        vec![job_id.into()],
    ))
    .await?;
    Ok(())
}

/// Moves a job to a terminal status (`'completed'` or `'failed'`), stamping `finished_at` and
/// recording the outcome (`flow_import_jobs_terminal_status_check` requires `finished_at` for
/// both). `report`/`error`/`audit_event_id` are mutually informative, not mutually exclusive at
/// the schema level: a `'failed'` job may still carry a partial `report` for diagnostics.
pub async fn finish_import_job<C: ConnectionTrait>(
    conn: &C,
    job_id: Uuid,
    status: &str,
    report: Option<Value>,
    error: Option<&str>,
    audit_event_id: Option<Uuid>,
) -> Result<(), ApiError> {
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE flow_import_jobs
            SET status = $2, report = $3, error = $4, audit_event_id = $5,
                finished_at = now(), updated_at = now()
            WHERE id = $1
        ",
        vec![
            job_id.into(),
            status.into(),
            report.into(),
            error.into(),
            audit_event_id.into(),
        ],
    ))
    .await?;
    Ok(())
}

/// A `flow_import_lineage` row (`model::ImportLineageView`'s source shape).
#[derive(Debug, Clone, FromQueryResult)]
pub struct ImportLineageRow {
    pub source_id: Uuid,
    pub source_content_hash: String,
    pub target_object_id: Uuid,
    pub target_document_id: Uuid,
    pub result: String,
}

/// A prior lineage row for this exact `(target_workspace_id,source_kind,source_id,
/// source_content_hash)` (`flow_import_lineage_idempotency_key`), if any. The importer reuses its
/// `target_object_id`/`target_document_id` instead of creating a duplicate when this is `Some`.
pub async fn find_import_lineage<C: ConnectionTrait>(
    conn: &C,
    target_workspace_id: Uuid,
    source_kind: &str,
    source_id: Uuid,
    source_content_hash: &str,
) -> Result<Option<ImportLineageRow>, ApiError> {
    Ok(ImportLineageRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT source_id, source_content_hash, target_object_id, target_document_id, result
            FROM flow_import_lineage
            WHERE target_workspace_id = $1 AND source_kind = $2 AND source_id = $3 AND source_content_hash = $4
        ",
        vec![
            target_workspace_id.into(),
            source_kind.into(),
            source_id.into(),
            source_content_hash.into(),
        ],
    ))
    .one(conn)
    .await?)
}

/// Every lineage row a job produced, in the order its items were imported (`imported_at`), for the
/// status endpoints' `items[]`/`object_mapping[]`.
pub async fn list_import_lineage_for_job<C: ConnectionTrait>(
    conn: &C,
    import_id: Uuid,
) -> Result<Vec<ImportLineageRow>, ApiError> {
    Ok(ImportLineageRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT source_id, source_content_hash, target_object_id, target_document_id, result
            FROM flow_import_lineage
            WHERE import_id = $1
            ORDER BY imported_at ASC
        ",
        vec![import_id.into()],
    ))
    .all(conn)
    .await?)
}

/// Fields needed to insert one `flow_import_lineage` row inside the importer's single commit
/// transaction (`legacy-pages-import-v1.md`: "execute 对所选 source set 采用单一事务").
pub struct NewImportLineage<'a> {
    pub import_id: Uuid,
    /// `flow_import_lineage_source_kind_check`: v0.4 only ever writes `"legacy_pages"`.
    pub source_kind: &'a str,
    /// `flow_import_lineage_source_table_check`: fixed to `"pages"` for `legacy_pages`.
    pub source_table: &'a str,
    pub source_id: Uuid,
    pub source_workspace_id: Uuid,
    pub target_workspace_id: Uuid,
    pub source_content_hash: &'a str,
    pub target_object_id: Uuid,
    pub target_document_id: Uuid,
    /// `flow_import_lineage_result_check`: `"created"` or `"reused"`.
    pub result: &'a str,
}

pub async fn insert_import_lineage<C: ConnectionTrait>(
    conn: &C,
    lineage: &NewImportLineage<'_>,
) -> Result<(), ApiError> {
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO flow_import_lineage (
                import_id, source_kind, source_table, source_id, source_workspace_id,
                target_workspace_id, source_content_hash, target_object_id, target_document_id, result
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        ",
        vec![
            lineage.import_id.into(),
            lineage.source_kind.into(),
            lineage.source_table.into(),
            lineage.source_id.into(),
            lineage.source_workspace_id.into(),
            lineage.target_workspace_id.into(),
            lineage.source_content_hash.into(),
            lineage.target_object_id.into(),
            lineage.target_document_id.into(),
            lineage.result.into(),
        ],
    ))
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// `flow::move_object` (`ADR-0012` §4 / `ADR-0013` §2) — cross-parent move
// ---------------------------------------------------------------------------------------------

/// The `flow_objects` governance columns a cross-parent move reads and rewrites.
///
/// `project_id` is here because `ADR-0013` §2.2 derives the command's *contended existing document
/// set* from it ("`move_object` 的集合由对象当前 `project_id` 推出"), so it is not incidental
/// metadata on this path — it is the input the whole lock plan is computed from, and the value the
/// locked phase re-reads to detect set drift.
#[derive(Debug, Clone, PartialEq, Eq, FromQueryResult)]
pub struct MovableObjectRow {
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    pub object_type: String,
    pub lifecycle_status: String,
}

const MOVABLE_OBJECT_COLUMNS: &str = "workspace_id, project_id, parent_id, object_type, lifecycle_status";

/// Reads one object's move-relevant governance columns without taking any lock (the lock-free
/// prepare phase).
pub async fn fetch_movable_object<C: ConnectionTrait>(
    conn: &C,
    object_id: Uuid,
) -> Result<Option<MovableObjectRow>, ApiError> {
    Ok(MovableObjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {MOVABLE_OBJECT_COLUMNS} FROM flow_objects WHERE id = $1"),
        vec![object_id.into()],
    ))
    .one(conn)
    .await?)
}

/// The same columns under `FOR UPDATE` — `ADR-0013` §2.1's "object / ancestor 行" lock-rank layer,
/// taken *after* the workspace `authz_epoch` row and *before* any `collab_documents` row.
///
/// Callers that lock more than one object row must call this once per id in ascending `id` order,
/// for the same reason the document layer is ordered.
pub async fn lock_movable_object<C: ConnectionTrait>(
    conn: &C,
    object_id: Uuid,
) -> Result<Option<MovableObjectRow>, ApiError> {
    Ok(MovableObjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {MOVABLE_OBJECT_COLUMNS} FROM flow_objects WHERE id = $1 FOR UPDATE"),
        vec![object_id.into()],
    ))
    .one(conn)
    .await?)
}

/// The navigator object (and its document) that holds ordering for one *scope* — a workspace and
/// either one project or the projectless scope.
///
/// `ADR-0012` §1 leaves the navigator CRDT document holding "只持有排序（position key）与显示元
/// 数据，不再持有父子关系", so this is the document a move has to rewrite when an object leaves one
/// scope for another. `project_id = None` selects the projectless navigator via `IS NOT DISTINCT
/// FROM`, which treats `NULL` as a value rather than as "unknown" — a plain `=` would silently
/// match nothing and make every projectless move look like "this scope has no navigator".
///
/// Returns `None` when the scope has no navigator object at all; a workspace is not required to
/// have one, and a move into or out of such a scope simply has no ordering entry to maintain
/// there.
pub async fn fetch_navigator_document<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
) -> Result<Option<NavigatorDocumentRow>, ApiError> {
    Ok(NavigatorDocumentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT fo.id AS object_id, cd.id AS document_id \
           FROM flow_objects fo JOIN collab_documents cd ON cd.object_id = fo.id \
          WHERE fo.workspace_id = $1 AND fo.object_type = 'navigator' \
            AND fo.project_id IS NOT DISTINCT FROM $2 AND fo.lifecycle_status = 'active' \
          ORDER BY fo.created_at, fo.id LIMIT 1",
        vec![workspace_id.into(), project_id.into()],
    ))
    .one(conn)
    .await?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FromQueryResult)]
pub struct NavigatorDocumentRow {
    pub object_id: Uuid,
    pub document_id: Uuid,
}

/// How many `parent_id` hops the deepest descendant of `object_id` sits below it (0 for a leaf).
///
/// The write-side depth rule for a move needs this and `create_object`'s check cannot supply it:
/// creating a child adds one node of known height 0, while moving relocates a whole subtree, so
/// the constraint is `depth(new_parent) + 1 + height(subtree) <= tree_depth_max`. The recursion is
/// bounded by `$3` for the same reason `authz::walk_chain`'s is — a corrupted `parent_id` cycle
/// below the object must terminate the query rather than spin.
pub async fn subtree_height<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    object_id: Uuid,
    probe_depth: i64,
) -> Result<i64, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        height: i64,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "WITH RECURSIVE subtree AS ( \
             SELECT o.id, 0 AS depth FROM flow_objects o \
              WHERE o.id = $1 AND o.workspace_id = $2 \
             UNION ALL \
             SELECT c.id, s.depth + 1 FROM subtree s \
               JOIN flow_objects c ON c.parent_id = s.id AND c.workspace_id = $2 \
              WHERE s.depth < $3::int \
         ) \
         SELECT COALESCE(max(depth), 0)::bigint AS height FROM subtree",
        vec![object_id.into(), workspace_id.into(), probe_depth.into()],
    ))
    .one(conn)
    .await?;
    Ok(row.map_or(0, |r| r.height))
}

/// The distinct project scopes of `object_id`'s **strict** descendants.
///
/// `None` is a scope like any other here (the unprojected navigator is a real document that real
/// ordering entries live in), so the result is deliberately `Option<Uuid>` rather than a filtered
/// list of ids -- collapsing `NULL` away would make a subtree that straddles a project and the
/// unprojected scope look single-scoped, which is the exact shape `ADR-0013` §2.2 R17 exists to
/// catch.
///
/// Same recursion and same `probe_depth` termination as [`subtree_height`], for the same reason: a
/// corrupted `parent_id` cycle below the object must end the query rather than spin.
pub async fn descendant_project_scopes<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    object_id: Uuid,
    probe_depth: i64,
) -> Result<Vec<Option<Uuid>>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        project_id: Option<Uuid>,
    }
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "WITH RECURSIVE subtree AS ( \
             SELECT o.id, 0 AS depth FROM flow_objects o \
              WHERE o.id = $1 AND o.workspace_id = $2 \
             UNION ALL \
             SELECT c.id, s.depth + 1 FROM subtree s \
               JOIN flow_objects c ON c.parent_id = s.id AND c.workspace_id = $2 \
              WHERE s.depth < $3::int \
         ) \
         SELECT DISTINCT o.project_id FROM subtree s \
           JOIN flow_objects o ON o.id = s.id \
          WHERE s.depth > 0",
        vec![object_id.into(), workspace_id.into(), probe_depth.into()],
    ))
    .all(conn)
    .await?;
    Ok(rows.into_iter().map(|r| r.project_id).collect())
}

/// How many rows currently break the `ADR-0013` §2.2 R17 parent/child project-scope invariant.
///
/// Reads `flow_object_project_scope_violations` (migration `0056`) rather than restating its
/// query, so "the constraint is enforced" and "the monitor says it is enforced" cannot drift apart
/// -- a test asserting on a private copy of the predicate would keep passing after the view was
/// changed to look at something else.
pub async fn project_scope_violation_count<C: ConnectionTrait>(conn: &C) -> Result<i64, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        violations: i64,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT count(*)::bigint AS violations FROM flow_object_project_scope_violations",
        vec![],
    ))
    .one(conn)
    .await?;
    Ok(row.map_or(0, |r| r.violations))
}

/// The moved object and every descendant of it, ascending `id`, plus the **true** total.
///
/// `ids` is truncated to `id_limit` rows; `total` is not. That asymmetry is the point: a subtree
/// past `move_subtree_nodes_max` must be refused with its real size in `details.observed`, and a
/// plain `LIMIT` would report the cap back as if it were the measurement. `count(*) OVER ()` is
/// evaluated before `LIMIT` in `PostgreSQL`, so one statement gives both.
///
/// Same recursion and same `probe_depth` termination as [`subtree_height`] and
/// [`descendant_project_scopes`], for the same reason: a corrupted `parent_id` cycle below the
/// object must end the query rather than spin.
pub async fn subtree_nodes<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    object_id: Uuid,
    probe_depth: i64,
    id_limit: i64,
) -> Result<SubtreeNodes, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        id: Uuid,
        total: i64,
    }
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "WITH RECURSIVE subtree AS ( \
             SELECT o.id, 0 AS depth FROM flow_objects o \
              WHERE o.id = $1 AND o.workspace_id = $2 \
             UNION ALL \
             SELECT c.id, s.depth + 1 FROM subtree s \
               JOIN flow_objects c ON c.parent_id = s.id AND c.workspace_id = $2 \
              WHERE s.depth < $3::int \
         ) \
         SELECT s.id, count(*) OVER ()::bigint AS total FROM subtree s ORDER BY s.id LIMIT $4",
        vec![
            object_id.into(),
            workspace_id.into(),
            probe_depth.into(),
            id_limit.into(),
        ],
    ))
    .all(conn)
    .await?;
    let total = rows.first().map_or(0, |row| row.total);
    Ok(SubtreeNodes {
        ids: rows.into_iter().map(|row| row.id).collect(),
        total,
    })
}

/// [`subtree_nodes`]'s result: the (possibly truncated) id list and the untruncated node count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtreeNodes {
    /// Ascending `id`, at most the `id_limit` rows the caller asked for.
    pub ids: Vec<Uuid>,
    /// Every node in the subtree, including the ones `ids` was truncated past.
    pub total: i64,
}

/// The governance columns of a set of objects under `FOR UPDATE`, ascending `id`.
///
/// `ADR-0013` §2.1 R17 admits both lock spellings — a per-id loop or
/// `WHERE id = ANY(..) ORDER BY id FOR UPDATE` — and this is the second one, chosen here for a
/// measured reason rather than a stylistic one: the cascade locks up to
/// `move_subtree_nodes_max` (100) rows *inside* the lock-hold budget, and a per-id loop would pay
/// one client/server round trip each (0.080 ms measured, ~8 ms at N = 100) against a
/// `document_lock_hold_ms_p95_max` of 25 ms with only 2.05x headroom. The document layer keeps its
/// per-id loop, where the count is two and the order is the thing under test.
///
/// The array travels as one `text` bind parameter split server-side, so nothing is concatenated
/// into SQL.
pub async fn lock_objects_for_update<C: ConnectionTrait>(
    conn: &C,
    ids: &[Uuid],
) -> Result<Vec<LockedObjectRow>, ApiError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(LockedObjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, project_id FROM flow_objects \
          WHERE id = ANY(string_to_array($1, ',')::uuid[]) ORDER BY id FOR UPDATE",
        vec![uuid_list(ids).into()],
    ))
    .all(conn)
    .await?)
}

/// One row of [`lock_objects_for_update`].
///
/// Two columns and no `lifecycle_status`, deliberately: the caller's decision does not depend on
/// it. `archive` is a pure `lifecycle_status` flip that leaves `parent_id` and `project_id`
/// untouched (`flow::command::execute_lifecycle_command`), so an archived object is still a member
/// of its parent's subtree and `flow_objects_parent_project_fk` still applies to it — a cascade
/// that skipped archived rows would fail the constraint on its own statement.
#[derive(Debug, Clone, PartialEq, Eq, FromQueryResult)]
pub struct LockedObjectRow {
    pub id: Uuid,
    pub project_id: Option<Uuid>,
}

/// Re-parents the moved object **and** rewrites the whole subtree's `project_id`, in exactly one
/// statement.
///
/// **One statement, and it has to be one statement.** Migration `0056`'s
/// `flow_objects_parent_project_fk` is `(parent_id, project_scope_id) REFERENCES flow_objects (id,
/// project_scope_id)` and is `NOT DEFERRABLE`, so `PostgreSQL` validates it at the end of *every*
/// statement. Splitting this into "re-parent the root" + "rewrite the descendants" fails at the
/// end of the first one, with the root already in the new scope and its children still in the old
/// one: measured, every forward move comes back
/// `violates foreign key constraint "flow_objects_parent_project_fk"`. The previous
/// `set_object_parent` two-step shape is therefore not merely slower, it is unusable for a
/// cascade, which is why it no longer exists.
///
/// `ids` must be the whole subtree (the moved object first or not, order is irrelevant to the
/// statement); the caller is responsible for having derived and locked it. `new_project_id` is
/// applied to every row, `new_parent_id` only to `object_id`.
///
/// Returns the number of rows actually rewritten, so the caller can refuse rather than assume when
/// the set it locked and the set it updated disagree.
pub async fn cascade_move_subtree<C: ConnectionTrait>(
    conn: &C,
    ids: &[Uuid],
    object_id: Uuid,
    new_parent_id: Uuid,
    new_project_id: Option<Uuid>,
    // `None` when the actor is a bot: this column is `REFERENCES users(id)` and a bot id is
    // not a user id.
    updated_by: Option<Uuid>,
) -> Result<u64, ApiError> {
    let result = conn
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_objects \
                SET project_id = $2, \
                    parent_id = CASE WHEN id = $4 THEN $5 ELSE parent_id END, \
                    updated_at = now(), updated_by = $3 \
              WHERE id = ANY(string_to_array($1, ',')::uuid[])",
            vec![
                uuid_list(ids).into(),
                new_project_id.into(),
                updated_by.into(),
                object_id.into(),
                new_parent_id.into(),
            ],
        ))
        .await?;
    Ok(result.rows_affected())
}

/// A `uuid[]` bind value, as the comma-separated text `string_to_array(.., ',')::uuid[]` parses.
///
/// A `Uuid`'s `Display` is 36 hex/dash characters and nothing else, so the separator can never
/// appear inside an element and this is a total encoding, not an escaping problem. It exists
/// because the value still travels as a single **bind parameter** — no identifier, value or
/// separator is concatenated into the statement text.
fn uuid_list(ids: &[Uuid]) -> String {
    let mut out = String::with_capacity(ids.len().saturating_mul(37));
    for (index, id) in ids.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&id.to_string());
    }
    out
}
