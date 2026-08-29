// Local SQL row types stay beside the queries whose column shapes they mirror, matching the
// convention `apps/api/src/routes/label.rs` and every module past migration 0024 already follow:
// hand-written parameterized SQL, no SeaORM entities.
#![allow(clippy::items_after_statements)]

use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
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
}

pub async fn fetch_parent_object<C: ConnectionTrait>(conn: &C, parent_id: Uuid) -> Result<Option<ParentRow>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        workspace_id: Uuid,
        lifecycle_status: String,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id, lifecycle_status FROM flow_objects WHERE id = $1",
        vec![parent_id.into()],
    ))
    .one(conn)
    .await?;
    Ok(row.map(|r| ParentRow {
        workspace_id: r.workspace_id,
        lifecycle_status: r.lifecycle_status,
    }))
}

/// A prior `business_events` row for this `(workspace_id, idempotency_key)`, if any.
///
/// Present when the key has been used before (`business_events_idempotency` unique index); the
/// create command uses this to replay idempotently instead of hitting the unique-violation path.
pub struct IdempotentEvent {
    pub id: Uuid,
    pub event_type: String,
    pub aggregate_id: String,
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
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, event_type, aggregate_id FROM business_events WHERE workspace_id = $1 AND idempotency_key = $2",
        vec![workspace_id.into(), idempotency_key.into()],
    ))
    .one(conn)
    .await?;
    Ok(row.map(|r| IdempotentEvent {
        id: r.id,
        event_type: r.event_type,
        aggregate_id: r.aggregate_id,
    }))
}

pub struct NewFlowObject {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub object_type: String,
    pub parent_id: Option<Uuid>,
    pub created_by: Uuid,
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
