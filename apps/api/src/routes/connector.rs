use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait, Value as SeaValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    events::{BusinessEventInput, insert_business_event},
    middleware::bot_auth::BotAuthContext,
    response::{ApiResponse, PaginatedData},
};

#[derive(Debug, Deserialize)]
pub struct ListConnectorsQuery {
    pub project_id: Option<Uuid>,
    pub kind: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListInvocationsQuery {
    pub status: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ListInvocationToolCallsQuery {
    pub status: Option<String>,
    pub tool_name: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ListInvocationInboxQuery {
    pub status: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ListFormInboxQuery {
    pub status: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateInvocationRequest {
    pub actor_id: Option<Uuid>,
    pub target_agent_id: Option<Uuid>,
    pub trigger_kind: String,
    pub trigger_ref_type: Option<String>,
    pub trigger_ref_id: Option<Uuid>,
    pub connector_id: Option<Uuid>,
    pub payload: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct InvocationProgressRequest {
    pub payload: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct CompleteInvocationRequest {
    pub result: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct FailInvocationRequest {
    pub error_message: Option<String>,
    pub result: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct ReportInvocationToolCallRequest {
    pub tool_name: String,
    pub transport: Option<String>,
    pub status: String,
    pub arguments: Option<Value>,
    pub result_summary: Option<String>,
    pub error_message: Option<String>,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ConnectorReceiptRequest {
    pub status: String,
    pub idempotency_key: Option<String>,
    pub payload: Option<Value>,
    pub error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateConnectorRequest {
    pub project_id: Option<Uuid>,
    pub kind: String,
    pub name: String,
    pub description: Option<String>,
    pub endpoint: Option<String>,
    pub auth_policy: Option<Value>,
    pub capability_manifest: Option<Value>,
    pub is_active: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateConnectorRequest {
    pub project_id: Option<Uuid>,
    pub kind: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub endpoint: Option<String>,
    pub auth_policy: Option<Value>,
    pub capability_manifest: Option<Value>,
    pub is_active: Option<bool>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct ConnectorResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub webhook_id: Option<Uuid>,
    pub kind: String,
    pub name: String,
    pub description: Option<String>,
    pub endpoint: Option<String>,
    pub auth_policy: Value,
    pub capability_manifest: Value,
    pub is_active: bool,
    pub created_by: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct InvocationResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub actor_id: Option<Uuid>,
    pub target_agent_id: Option<Uuid>,
    pub source_task_id: Option<Uuid>,
    pub trigger_kind: String,
    pub trigger_ref_type: Option<String>,
    pub trigger_ref_id: Option<Uuid>,
    pub connector_id: Option<Uuid>,
    pub connector_kind: Option<String>,
    pub status: String,
    pub payload: Value,
    pub result: Option<Value>,
    pub error_message: Option<String>,
    pub audit_chain_id: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct InvocationToolCallResponse {
    pub id: Uuid,
    pub invocation_id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub actor_id: Option<Uuid>,
    pub tool_name: String,
    pub transport: String,
    pub status: String,
    pub arguments: Value,
    pub result_summary: Option<String>,
    pub error_message: Option<String>,
    pub duration_ms: i64,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: chrono::DateTime<chrono::Utc>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct EventInboxResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub source_kind: String,
    pub source_id: Option<String>,
    pub idempotency_key: String,
    pub event_type: String,
    pub payload: Value,
    pub status: String,
    pub attempts: i32,
    pub last_error: Option<String>,
    pub received_at: chrono::DateTime<chrono::Utc>,
    pub processed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, FromQueryResult)]
struct RoleRow {
    role: String,
}

#[derive(Debug, FromQueryResult)]
struct ProjectWorkspaceRow {
    workspace_id: Uuid,
}

#[derive(Debug, FromQueryResult)]
struct FormInboxScopeRow {
    workspace_id: Uuid,
    project_id: Uuid,
    form_key: String,
}

#[derive(Debug, FromQueryResult)]
struct ReceiptIdempotencyRow {
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    source_id: Option<String>,
    event_type: String,
    payload: Value,
}

struct InvocationEventInput<'a> {
    event_type: &'a str,
    invocation: &'a InvocationResponse,
    actor_id: Option<Uuid>,
    source: Value,
    previous_status: Option<&'a str>,
}

pub async fn list_connectors(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Query(query): Query<ListConnectorsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_workspace_access(&state, &claims, bot.as_ref().map(|b| &b.0), workspace_id).await?;
    if let Some(project_id) = query.project_id {
        ensure_project_in_workspace(&state, workspace_id, project_id).await?;
    }
    if let Some(kind) = query.kind.as_deref() {
        normalize_connector_kind(kind)?;
    }

    let mut where_sql = "WHERE workspace_id = $1".to_string();
    let mut values = vec![workspace_id.into()];
    let mut param_idx = 2;

    if let Some(project_id) = query.project_id {
        where_sql.push_str(&format!(" AND project_id = ${param_idx}"));
        values.push(project_id.into());
        param_idx += 1;
    }
    if let Some(kind) = query.kind {
        where_sql.push_str(&format!(" AND kind = ${param_idx}"));
        values.push(normalize_connector_kind(&kind)?.into());
    }

    let sql = format!(
        r"
            SELECT id, workspace_id, project_id, webhook_id, kind, name, description, endpoint,
                   auth_policy, capability_manifest, is_active, created_by, created_at, updated_at
            FROM connectors
            {where_sql}
            ORDER BY updated_at DESC
        "
    );

    let items = ConnectorResponse::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, &sql, values))
        .all(&state.db)
        .await?;

    Ok(ApiResponse::success(items))
}

pub async fn create_connector(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CreateConnectorRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (actor_id, is_admin, is_bot) =
        ensure_workspace_access(&state, &claims, bot.as_ref().map(|b| &b.0), workspace_id).await?;
    if !is_admin {
        return Err(ApiError::Forbidden("workspace admin or owner required".to_string()));
    }
    if let Some(project_id) = req.project_id {
        ensure_project_in_workspace(&state, workspace_id, project_id).await?;
    }

    let kind = normalize_connector_kind(&req.kind)?;
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("name is required".to_string()));
    }
    let auth_policy = ensure_json_object(req.auth_policy.unwrap_or_else(|| json!({})), "auth_policy")?;
    let capability_manifest = ensure_json_object(
        req.capability_manifest.unwrap_or_else(|| json!({})),
        "capability_manifest",
    )?;
    let connector_id = Uuid::new_v4();
    let created_by = if is_bot { None } else { Some(actor_id) };
    let tx = state.db.begin().await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
                INSERT INTO connectors (
                    id, workspace_id, project_id, kind, name, description, endpoint,
                    auth_policy, capability_manifest, is_active, created_by, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, now(), now())
            ",
        vec![
            connector_id.into(),
            workspace_id.into(),
            req.project_id.into(),
            kind.into(),
            name.to_string().into(),
            req.description.into(),
            req.endpoint.into(),
            auth_policy.into(),
            capability_manifest.into(),
            req.is_active.unwrap_or(true).into(),
            created_by.into(),
        ],
    ))
    .await?;
    let connector = find_connector_with_conn(&tx, workspace_id, connector_id).await?;
    insert_connector_event(
        &tx,
        &connector,
        "connector.created",
        created_by,
        json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
        json!({
            "connector_id": connector.id,
            "connector_kind": connector.kind,
            "project_id": connector.project_id,
            "is_active": connector.is_active
        }),
    )
    .await?;
    tx.commit().await?;

    get_connector(State(state), Extension(claims), bot, Path((workspace_id, connector_id))).await
}

pub async fn get_connector(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((workspace_id, connector_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_workspace_access(&state, &claims, bot.as_ref().map(|b| &b.0), workspace_id).await?;
    let connector = find_connector(&state, workspace_id, connector_id).await?;
    Ok(ApiResponse::success(connector))
}

pub async fn update_connector(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((workspace_id, connector_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateConnectorRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (actor_id, is_admin, is_bot) =
        ensure_workspace_access(&state, &claims, bot.as_ref().map(|b| &b.0), workspace_id).await?;
    if !is_admin {
        return Err(ApiError::Forbidden("workspace admin or owner required".to_string()));
    }
    let existing = find_connector(&state, workspace_id, connector_id).await?;
    if let Some(project_id) = req.project_id {
        ensure_project_in_workspace(&state, workspace_id, project_id).await?;
    }

    let mut set_parts = Vec::new();
    let mut values = Vec::new();
    let mut changed_fields = Vec::new();
    let mut idx = 1;

    macro_rules! push_set {
        ($column:literal, $value:expr, $field:literal) => {{
            set_parts.push(format!("{} = ${}", $column, idx));
            values.push($value);
            changed_fields.push($field);
            idx += 1;
        }};
    }

    if let Some(project_id) = req.project_id {
        push_set!("project_id", project_id.into(), "project_id");
    }
    if let Some(kind) = req.kind {
        push_set!("kind", normalize_connector_kind(&kind)?.into(), "kind");
    }
    if let Some(name) = req.name {
        let trimmed = name.trim().to_string();
        if trimmed.is_empty() {
            return Err(ApiError::BadRequest("name cannot be empty".to_string()));
        }
        push_set!("name", trimmed.into(), "name");
    }
    if let Some(description) = req.description {
        push_set!("description", description.into(), "description");
    }
    if let Some(endpoint) = req.endpoint {
        push_set!("endpoint", endpoint.into(), "endpoint");
    }
    if let Some(auth_policy) = req.auth_policy {
        push_set!(
            "auth_policy",
            ensure_json_object(auth_policy, "auth_policy")?.into(),
            "auth_policy"
        );
    }
    if let Some(capability_manifest) = req.capability_manifest {
        push_set!(
            "capability_manifest",
            ensure_json_object(capability_manifest, "capability_manifest")?.into(),
            "capability_manifest"
        );
    }
    if let Some(is_active) = req.is_active {
        push_set!("is_active", is_active.into(), "is_active");
    }

    if set_parts.is_empty() {
        return get_connector(State(state), Extension(claims), bot, Path((workspace_id, connector_id))).await;
    }

    set_parts.push("updated_at = now()".to_string());
    values.push(connector_id.into());
    values.push(workspace_id.into());
    let sql = format!(
        "UPDATE connectors SET {} WHERE id = ${} AND workspace_id = ${}",
        set_parts.join(", "),
        idx,
        idx + 1
    );
    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres, &sql, values))
        .await?;
    let connector = find_connector_with_conn(&tx, workspace_id, connector_id).await?;
    insert_connector_event(
        &tx,
        &connector,
        "connector.updated",
        if is_bot { None } else { Some(actor_id) },
        json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
        json!({
            "connector_id": connector.id,
            "connector_kind": connector.kind,
            "previous_kind": existing.kind,
            "previous_project_id": existing.project_id,
            "project_id": connector.project_id,
            "changed_fields": changed_fields
        }),
    )
    .await?;
    tx.commit().await?;

    get_connector(State(state), Extension(claims), bot, Path((workspace_id, connector_id))).await
}

pub async fn delete_connector(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((workspace_id, connector_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let (actor_id, is_admin, is_bot) =
        ensure_workspace_access(&state, &claims, bot.as_ref().map(|b| &b.0), workspace_id).await?;
    if !is_admin {
        return Err(ApiError::Forbidden("workspace admin or owner required".to_string()));
    }
    let connector = find_connector(&state, workspace_id, connector_id).await?;
    let tx = state.db.begin().await?;
    insert_connector_event(
        &tx,
        &connector,
        "connector.deleted",
        if is_bot { None } else { Some(actor_id) },
        json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
        json!({
            "connector_id": connector.id,
            "connector_kind": connector.kind,
            "project_id": connector.project_id
        }),
    )
    .await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM connectors WHERE id = $1 AND workspace_id = $2",
        vec![connector_id.into(), workspace_id.into()],
    ))
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::ok())
}

pub async fn list_project_invocations(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<ListInvocationsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let project = find_project_workspace(&state, project_id).await?;
    ensure_workspace_access(&state, &claims, bot.as_ref().map(|b| &b.0), project.workspace_id).await?;
    list_invocations_inner(&state, Some(project_id), query).await
}

pub async fn create_project_invocation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<CreateInvocationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let project = find_project_workspace(&state, project_id).await?;
    let (auth_actor_id, _, is_bot) =
        ensure_workspace_access(&state, &claims, bot.as_ref().map(|b| &b.0), project.workspace_id).await?;
    let trigger_kind = normalize_trigger_kind(&req.trigger_kind)?;
    let payload = ensure_json_object(req.payload.unwrap_or_else(|| json!({})), "payload")?;
    let (connector_id, connector_kind) = if let Some(connector_id) = req.connector_id {
        let connector = find_connector(&state, project.workspace_id, connector_id).await?;
        if let Some(connector_project_id) = connector.project_id
            && connector_project_id != project_id
        {
            return Err(ApiError::BadRequest(
                "connector does not belong to this project".to_string(),
            ));
        }
        (Some(connector.id), Some(connector.kind))
    } else {
        (None, None)
    };
    let actor_id = if is_bot {
        None
    } else {
        req.actor_id.or(Some(auth_actor_id))
    };
    let invocation_id = Uuid::new_v4();

    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
                INSERT INTO agent_invocations (
                    id, workspace_id, project_id, actor_id, target_agent_id,
                    trigger_kind, trigger_ref_type, trigger_ref_id, connector_id,
                    connector_kind, status, payload, audit_chain_id, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'pending', $11, $12, now(), now())
            ",
        vec![
            invocation_id.into(),
            project.workspace_id.into(),
            project_id.into(),
            actor_id.into(),
            req.target_agent_id.into(),
            trigger_kind.into(),
            req.trigger_ref_type.into(),
            req.trigger_ref_id.into(),
            connector_id.into(),
            connector_kind.into(),
            payload.into(),
            invocation_id.into(),
        ],
    ))
    .await?;
    let invocation = find_invocation_with_conn(&tx, invocation_id).await?;
    insert_invocation_event(
        &tx,
        InvocationEventInput {
            event_type: "invocation.created",
            invocation: &invocation,
            actor_id,
            source: json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": auth_actor_id }),
            previous_status: None,
        },
    )
    .await?;
    tx.commit().await?;

    get_invocation(State(state), Extension(claims), bot, Path(invocation_id)).await
}

pub async fn get_invocation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(invocation_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let invocation = find_invocation(&state, invocation_id).await?;
    ensure_workspace_access(&state, &claims, bot.as_ref().map(|b| &b.0), invocation.workspace_id).await?;
    Ok(ApiResponse::success(invocation))
}

pub async fn list_invocation_tool_calls(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(invocation_id): Path<Uuid>,
    Query(query): Query<ListInvocationToolCallsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let invocation = find_invocation(&state, invocation_id).await?;
    ensure_workspace_access(&state, &claims, bot.as_ref().map(|b| &b.0), invocation.workspace_id).await?;

    if let Some(status) = query.status.as_deref() {
        normalize_tool_call_status(status)?;
    }

    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(50).clamp(1, 200);
    let offset = (page - 1) * per_page;

    let mut where_parts = vec!["invocation_id = $1".to_string()];
    let mut values = vec![invocation_id.into()];
    let mut idx = 2;

    if let Some(status) = query.status {
        where_parts.push(format!("status = ${idx}"));
        values.push(normalize_tool_call_status(&status)?.into());
        idx += 1;
    }
    if let Some(tool_name) = query.tool_name {
        let tool_name = normalize_tool_name(&tool_name)?;
        where_parts.push(format!("tool_name = ${idx}"));
        values.push(tool_name.into());
        idx += 1;
    }

    let where_sql = format!("WHERE {}", where_parts.join(" AND "));
    let total = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("SELECT COUNT(*)::bigint AS count FROM agent_invocation_tool_calls {where_sql}"),
            values.clone(),
        ))
        .await?
        .ok_or_else(|| ApiError::Internal)?
        .try_get::<i64>("", "count")?;

    values.push(per_page.into());
    values.push(offset.into());
    let list_sql = format!(
        r"
            SELECT id, invocation_id, workspace_id, project_id, actor_id, tool_name, transport,
                   status, arguments, result_summary, error_message, duration_ms,
                   started_at, completed_at, created_at
            FROM agent_invocation_tool_calls
            {where_sql}
            ORDER BY started_at ASC, created_at ASC
            LIMIT ${idx} OFFSET ${}
        ",
        idx + 1
    );
    let items = InvocationToolCallResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &list_sql,
        values,
    ))
    .all(&state.db)
    .await?;
    let total_pages = if total == 0 {
        0
    } else {
        ((total as f64) / (per_page as f64)).ceil() as i64
    };

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages,
    }))
}

pub async fn list_invocation_inbox(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(invocation_id): Path<Uuid>,
    Query(query): Query<ListInvocationInboxQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let invocation = find_invocation(&state, invocation_id).await?;
    ensure_workspace_access(&state, &claims, bot.as_ref().map(|b| &b.0), invocation.workspace_id).await?;
    if let Some(status) = query.status.as_deref() {
        normalize_inbox_status(status)?;
    }

    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;
    let invocation_ref = invocation_id.to_string();
    let idempotency_pattern = format!("%{invocation_ref}%");

    let mut where_parts = vec![
        "workspace_id = $1".to_string(),
        "(payload->>'invocation_id' = $2 OR source_id = $2 OR idempotency_key LIKE $3)".to_string(),
    ];
    let mut values = vec![
        invocation.workspace_id.into(),
        invocation_ref.into(),
        idempotency_pattern.into(),
    ];
    let mut idx = 4;
    if let Some(status) = query.status {
        where_parts.push(format!("status = ${idx}"));
        values.push(normalize_inbox_status(&status)?.into());
        idx += 1;
    }
    let where_sql = format!("WHERE {}", where_parts.join(" AND "));
    let total = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("SELECT COUNT(*)::bigint AS count FROM event_inbox {where_sql}"),
            values.clone(),
        ))
        .await?
        .ok_or_else(|| ApiError::Internal)?
        .try_get::<i64>("", "count")?;

    values.push(per_page.into());
    values.push(offset.into());
    let list_sql = format!(
        r"
            SELECT id, workspace_id, project_id, source_kind, source_id, idempotency_key,
                   event_type, payload, status, attempts, last_error, received_at,
                   processed_at, updated_at
            FROM event_inbox
            {where_sql}
            ORDER BY received_at DESC, updated_at DESC
            LIMIT ${idx} OFFSET ${}
        ",
        idx + 1
    );
    let items =
        EventInboxResponse::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, &list_sql, values))
            .all(&state.db)
            .await?;
    let total_pages = if total == 0 {
        0
    } else {
        ((total as f64) / (per_page as f64)).ceil() as i64
    };

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages,
    }))
}

pub async fn replay_invocation_inbox(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((invocation_id, inbox_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let invocation = find_invocation(&state, invocation_id).await?;
    let (auth_actor_id, _, is_bot) =
        ensure_workspace_access(&state, &claims, bot.as_ref().map(|b| &b.0), invocation.workspace_id).await?;
    let invocation_ref = invocation_id.to_string();
    let idempotency_pattern = format!("%{invocation_ref}%");
    let tx = state.db.begin().await?;
    let row = EventInboxResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE event_inbox
            SET status = 'received',
                attempts = attempts + 1,
                last_error = NULL,
                processed_at = NULL,
                updated_at = now()
            WHERE id = $1
              AND workspace_id = $2
              AND (payload->>'invocation_id' = $3 OR source_id = $3 OR idempotency_key LIKE $4)
            RETURNING id, workspace_id, project_id, source_kind, source_id, idempotency_key,
                      event_type, payload, status, attempts, last_error, received_at,
                      processed_at, updated_at
        ",
        vec![
            inbox_id.into(),
            invocation.workspace_id.into(),
            invocation_ref.into(),
            idempotency_pattern.into(),
        ],
    ))
    .one(&tx)
    .await?
    .ok_or_else(|| ApiError::NotFound("event inbox row not found".to_string()))?;
    insert_business_event(
        &tx,
        BusinessEventInput {
            workspace_id: invocation.workspace_id,
            project_id: invocation.project_id,
            event_type: "event_inbox.replayed".to_string(),
            aggregate_type: "event_inbox".to_string(),
            aggregate_id: row.id.to_string(),
            actor_id: if is_bot { None } else { Some(auth_actor_id) },
            source: json!({
                "type": if is_bot { "bot" } else { "user" },
                "actor_id": auth_actor_id,
                "invocation_id": invocation_id
            }),
            payload: json!({
                "inbox_id": row.id,
                "invocation_id": invocation_id,
                "idempotency_key": row.idempotency_key,
                "event_type": row.event_type,
                "status": row.status,
                "attempts": row.attempts
            }),
            metadata: json!({
                "inbox_id": row.id,
                "invocation_id": invocation_id,
                "connector_id": invocation.connector_id,
                "connector_kind": invocation.connector_kind
            }),
            correlation_id: invocation.audit_chain_id,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    tx.commit().await?;

    Ok(ApiResponse::success(row))
}

pub async fn list_form_inbox(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(form_id): Path<Uuid>,
    Query(query): Query<ListFormInboxQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form_inbox_scope(&state, form_id).await?;
    ensure_workspace_access(&state, &claims, bot.as_ref().map(|b| &b.0), form.workspace_id).await?;
    if let Some(status) = query.status.as_deref() {
        normalize_inbox_status(status)?;
    }

    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;
    let form_ref = form_id.to_string();
    let idempotency_pattern = format!("%{form_ref}%");

    let mut where_parts = vec![
        "workspace_id = $1".to_string(),
        "project_id = $2".to_string(),
        form_inbox_scope_sql("$3", "$4", "$5"),
    ];
    let mut values = vec![
        form.workspace_id.into(),
        form.project_id.into(),
        form_ref.into(),
        form.form_key.into(),
        idempotency_pattern.into(),
    ];
    let mut idx = 6;
    if let Some(status) = query.status {
        where_parts.push(format!("status = ${idx}"));
        values.push(normalize_inbox_status(&status)?.into());
        idx += 1;
    }
    let where_sql = format!("WHERE {}", where_parts.join(" AND "));
    let total = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("SELECT COUNT(*)::bigint AS count FROM event_inbox {where_sql}"),
            values.clone(),
        ))
        .await?
        .ok_or_else(|| ApiError::Internal)?
        .try_get::<i64>("", "count")?;

    values.push(per_page.into());
    values.push(offset.into());
    let list_sql = format!(
        r"
            SELECT id, workspace_id, project_id, source_kind, source_id, idempotency_key,
                   event_type, payload, status, attempts, last_error, received_at,
                   processed_at, updated_at
            FROM event_inbox
            {where_sql}
            ORDER BY received_at DESC, updated_at DESC
            LIMIT ${idx} OFFSET ${}
        ",
        idx + 1
    );
    let items =
        EventInboxResponse::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, &list_sql, values))
            .all(&state.db)
            .await?;
    let total_pages = if total == 0 {
        0
    } else {
        ((total as f64) / (per_page as f64)).ceil() as i64
    };

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages,
    }))
}

pub async fn replay_form_inbox(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((form_id, inbox_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let form = find_form_inbox_scope(&state, form_id).await?;
    let (auth_actor_id, _, is_bot) =
        ensure_workspace_access(&state, &claims, bot.as_ref().map(|b| &b.0), form.workspace_id).await?;
    let form_ref = form_id.to_string();
    let idempotency_pattern = format!("%{form_ref}%");
    let tx = state.db.begin().await?;
    let row = EventInboxResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r"
                UPDATE event_inbox
                SET status = 'received',
                    attempts = attempts + 1,
                    last_error = NULL,
                    processed_at = NULL,
                    updated_at = now()
                WHERE id = $1
                  AND workspace_id = $2
                  AND project_id = $3
                  AND {}
                RETURNING id, workspace_id, project_id, source_kind, source_id, idempotency_key,
                          event_type, payload, status, attempts, last_error, received_at,
                          processed_at, updated_at
            ",
            form_inbox_scope_sql("$4", "$5", "$6")
        ),
        vec![
            inbox_id.into(),
            form.workspace_id.into(),
            form.project_id.into(),
            form_ref.into(),
            form.form_key.clone().into(),
            idempotency_pattern.into(),
        ],
    ))
    .one(&tx)
    .await?
    .ok_or_else(|| ApiError::NotFound("event inbox row not found".to_string()))?;
    insert_business_event(
        &tx,
        BusinessEventInput {
            workspace_id: form.workspace_id,
            project_id: Some(form.project_id),
            event_type: "event_inbox.replayed".to_string(),
            aggregate_type: "event_inbox".to_string(),
            aggregate_id: row.id.to_string(),
            actor_id: if is_bot { None } else { Some(auth_actor_id) },
            source: json!({
                "type": if is_bot { "bot" } else { "user" },
                "actor_id": auth_actor_id,
                "form_id": form_id,
                "form_key": form.form_key
            }),
            payload: json!({
                "inbox_id": row.id,
                "form_id": form_id,
                "form_key": form.form_key,
                "idempotency_key": row.idempotency_key,
                "event_type": row.event_type,
                "status": row.status,
                "attempts": row.attempts
            }),
            metadata: json!({
                "inbox_id": row.id,
                "form_id": form_id,
                "form_key": form.form_key
            }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    tx.commit().await?;

    Ok(ApiResponse::success(row))
}

pub async fn report_invocation_tool_call(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(invocation_id): Path<Uuid>,
    Json(req): Json<ReportInvocationToolCallRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let invocation = find_invocation(&state, invocation_id).await?;
    let (actor_id, _, _) =
        ensure_workspace_access(&state, &claims, bot.as_ref().map(|b| &b.0), invocation.workspace_id).await?;
    let tool_name = normalize_tool_name(&req.tool_name)?;
    let status = normalize_tool_call_status(&req.status)?;
    let transport = normalize_tool_transport(req.transport.as_deref().unwrap_or("mcp"))?;
    let arguments = ensure_json_object(req.arguments.unwrap_or_else(|| json!({})), "arguments")?;
    let duration_ms = req.duration_ms.unwrap_or(0).max(0);
    let result_summary = req.result_summary.map(|value| truncate_string(value, 2_000));
    let error_message = req.error_message.map(|value| truncate_string(value, 2_000));

    let row = InvocationToolCallResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO agent_invocation_tool_calls (
                id,
                invocation_id,
                workspace_id,
                project_id,
                actor_id,
                tool_name,
                transport,
                status,
                arguments,
                result_summary,
                error_message,
                duration_ms,
                started_at,
                completed_at,
                created_at
            )
            VALUES (
                gen_random_uuid(),
                $1,
                $2,
                $3,
                $4,
                $5,
                $6,
                $7,
                $8,
                $9,
                $10,
                $11,
                now() - make_interval(secs => ($11::double precision / 1000.0)),
                now(),
                now()
            )
            RETURNING id, invocation_id, workspace_id, project_id, actor_id, tool_name, transport,
                      status, arguments, result_summary, error_message, duration_ms,
                      started_at, completed_at, created_at
        ",
        vec![
            invocation_id.into(),
            invocation.workspace_id.into(),
            invocation.project_id.into(),
            actor_id.into(),
            tool_name.into(),
            transport.into(),
            status.into(),
            SeaValue::Json(Some(Box::new(arguments))),
            result_summary.into(),
            error_message.into(),
            duration_ms.into(),
        ],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::Internal)?;

    Ok(ApiResponse::success(row))
}

pub async fn cancel_invocation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(invocation_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let invocation = find_invocation(&state, invocation_id).await?;
    let (auth_actor_id, is_admin, is_bot) =
        ensure_workspace_access(&state, &claims, bot.as_ref().map(|b| &b.0), invocation.workspace_id).await?;
    if !is_admin {
        return Err(ApiError::Forbidden("workspace admin or owner required".to_string()));
    }

    let tx = state.db.begin().await?;
    let result = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE agent_invocations
                SET status = 'cancelled', updated_at = now()
                WHERE id = $1 AND status IN ('pending', 'dispatched', 'running')
            ",
            vec![invocation_id.into()],
        ))
        .await?;
    if result.rows_affected() > 0 {
        let updated_invocation = find_invocation_with_conn(&tx, invocation_id).await?;
        insert_invocation_event(
            &tx,
            InvocationEventInput {
                event_type: "invocation.cancelled",
                invocation: &updated_invocation,
                actor_id: if is_bot { None } else { Some(auth_actor_id) },
                source: json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": auth_actor_id }),
                previous_status: Some(invocation.status.as_str()),
            },
        )
        .await?;
    }
    if let Some(task_id) = invocation.source_task_id {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                    UPDATE ai_tasks
                    SET status = 'cancelled', updated_at = now()
                    WHERE id = $1 AND status IN ('pending', 'processing')
                ",
            vec![task_id.into()],
        ))
        .await?;
    }
    tx.commit().await?;
    get_invocation(State(state), Extension(claims), bot, Path(invocation_id)).await
}

pub async fn report_invocation_progress(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(invocation_id): Path<Uuid>,
    Json(req): Json<InvocationProgressRequest>,
) -> Result<impl IntoResponse, ApiError> {
    update_invocation_status(
        &state,
        &claims,
        bot.as_ref().map(|b| &b.0),
        invocation_id,
        "running",
        req.payload.map(|payload| json!({ "progress": payload })),
        None,
        false,
    )
    .await?;
    get_invocation(State(state), Extension(claims), bot, Path(invocation_id)).await
}

pub async fn complete_invocation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(invocation_id): Path<Uuid>,
    Json(req): Json<CompleteInvocationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    update_invocation_status(
        &state,
        &claims,
        bot.as_ref().map(|b| &b.0),
        invocation_id,
        "completed",
        req.result,
        None,
        true,
    )
    .await?;
    get_invocation(State(state), Extension(claims), bot, Path(invocation_id)).await
}

pub async fn fail_invocation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(invocation_id): Path<Uuid>,
    Json(req): Json<FailInvocationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    update_invocation_status(
        &state,
        &claims,
        bot.as_ref().map(|b| &b.0),
        invocation_id,
        "failed",
        req.result,
        req.error_message.or_else(|| Some("invocation failed".to_string())),
        true,
    )
    .await?;
    get_invocation(State(state), Extension(claims), bot, Path(invocation_id)).await
}

pub async fn report_connector_receipt(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(invocation_id): Path<Uuid>,
    Json(req): Json<ConnectorReceiptRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let invocation = find_invocation(&state, invocation_id).await?;
    ensure_workspace_access(&state, &claims, bot.as_ref().map(|b| &b.0), invocation.workspace_id).await?;
    let receipt_status = normalize_receipt_status(&req.status)?;
    let invocation_status = match receipt_status.as_str() {
        "completed" => "completed",
        "failed" => "failed",
        "received" => "running",
        _ => return Err(ApiError::BadRequest("unsupported receipt status".to_string())),
    };
    let payload = ensure_json_object(req.payload.unwrap_or_else(|| json!({})), "payload")?;
    validate_connector_receipt_target(&invocation, &payload)?;
    let idempotency_key = normalize_receipt_idempotency_key(req.idempotency_key, invocation_id, &receipt_status)?;
    let source_kind = invocation
        .connector_kind
        .as_deref()
        .map(|kind| format!("connector.{kind}"))
        .unwrap_or_else(|| "connector".to_string());
    let source_id = invocation
        .connector_id
        .map(|connector_id| connector_id.to_string())
        .unwrap_or_else(|| invocation_id.to_string());
    let event_type = if receipt_status == "failed" {
        "connector.delivery.failed"
    } else {
        "connector.delivery.received"
    };
    let receipt_payload = json!({
        "invocation_id": invocation_id,
        "connector_id": invocation.connector_id,
        "connector_kind": invocation.connector_kind,
        "source_kind": source_kind.clone(),
        "source_id": source_id.clone(),
        "event_type": event_type,
        "idempotency_key": idempotency_key.clone(),
        "inbox_status": "processed",
        "receipt_status": receipt_status,
        "payload": payload,
        "error_message": req.error_message
    });
    let result_payload = if receipt_status == "failed" {
        json!({ "receipt": receipt_payload })
    } else {
        receipt_payload.clone()
    };
    let error_message = if receipt_status == "failed" {
        req.error_message
            .or_else(|| Some("connector delivery failed".to_string()))
    } else {
        None
    };
    let tx = state.db.begin().await?;

    if let Some(existing) = find_receipt_idempotency_row(&tx, &source_kind, &idempotency_key).await? {
        validate_receipt_idempotency_replay(
            &existing,
            invocation.workspace_id,
            invocation.project_id,
            Some(source_id.as_str()),
            event_type,
            &receipt_payload,
        )?;
    }

    let receipt_result = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            INSERT INTO event_inbox (
                id, workspace_id, project_id, source_kind, source_id, idempotency_key,
                event_type, payload, status, received_at, processed_at, updated_at
            )
            VALUES (
                gen_random_uuid(), $1, $2, $3, $4, $5,
                $6, $7, 'processed', now(), now(), now()
            )
            ON CONFLICT (source_kind, idempotency_key) DO UPDATE
            SET updated_at = now()
            WHERE event_inbox.workspace_id = EXCLUDED.workspace_id
              AND event_inbox.project_id IS NOT DISTINCT FROM EXCLUDED.project_id
              AND event_inbox.source_id IS NOT DISTINCT FROM EXCLUDED.source_id
              AND event_inbox.event_type = EXCLUDED.event_type
              AND event_inbox.payload = EXCLUDED.payload
        ",
            vec![
                invocation.workspace_id.into(),
                invocation.project_id.into(),
                source_kind.into(),
                source_id.into(),
                idempotency_key.into(),
                event_type.to_string().into(),
                receipt_payload.into(),
            ],
        ))
        .await?;
    if receipt_result.rows_affected() == 0 {
        return Err(ApiError::Conflict(
            "receipt idempotency key already exists with different content".to_string(),
        ));
    }

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE agent_invocations
            SET status = CASE
                    WHEN status IN ('completed', 'failed', 'cancelled') THEN status
                    ELSE $2
                END,
                result = COALESCE($3, result),
                error_message = COALESCE($4, error_message),
                updated_at = now()
            WHERE id = $1
        ",
        vec![
            invocation_id.into(),
            invocation_status.to_string().into(),
            Some(result_payload).into(),
            error_message.into(),
        ],
    ))
    .await?;
    let updated_invocation = find_invocation_with_conn(&tx, invocation_id).await?;
    if invocation.status != updated_invocation.status {
        insert_invocation_event(
            &tx,
            InvocationEventInput {
                event_type: invocation_event_type_for_status(&updated_invocation.status),
                invocation: &updated_invocation,
                actor_id: None,
                source: json!({
                    "type": "connector_receipt",
                    "connector_id": invocation.connector_id,
                    "connector_kind": invocation.connector_kind
                }),
                previous_status: Some(invocation.status.as_str()),
            },
        )
        .await?;
    }
    tx.commit().await?;

    get_invocation(State(state), Extension(claims), bot, Path(invocation_id)).await
}

async fn update_invocation_status(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&BotAuthContext>,
    invocation_id: Uuid,
    status: &str,
    result: Option<Value>,
    error_message: Option<String>,
    terminal: bool,
) -> Result<(), ApiError> {
    let invocation = find_invocation(state, invocation_id).await?;
    ensure_workspace_access(state, claims, bot, invocation.workspace_id).await?;
    if terminal && matches!(invocation.status.as_str(), "completed" | "failed" | "cancelled") {
        return Err(ApiError::BadRequest("invocation is already terminal".to_string()));
    }

    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
                UPDATE agent_invocations
                SET status = $2,
                    result = COALESCE($3, result),
                    error_message = $4,
                    updated_at = now()
                WHERE id = $1
            ",
        vec![
            invocation_id.into(),
            status.to_string().into(),
            result.clone().into(),
            error_message.clone().into(),
        ],
    ))
    .await?;
    let updated_invocation = find_invocation_with_conn(&tx, invocation_id).await?;
    if invocation.status != updated_invocation.status {
        insert_invocation_event(
            &tx,
            InvocationEventInput {
                event_type: invocation_event_type_for_status(status),
                invocation: &updated_invocation,
                actor_id: invocation.actor_id,
                source: json!({ "type": if bot.is_some() { "bot" } else { "user" } }),
                previous_status: Some(invocation.status.as_str()),
            },
        )
        .await?;
    }

    if terminal && let Some(task_id) = invocation.source_task_id {
        sync_source_task_terminal_status(&tx, task_id, status, result, error_message).await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn sync_source_task_terminal_status<C>(
    db: &C,
    task_id: Uuid,
    invocation_status: &str,
    result: Option<Value>,
    error_message: Option<String>,
) -> Result<(), ApiError>
where
    C: ConnectionTrait,
{
    let task_status = match invocation_status {
        "completed" => "completed",
        "failed" => "failed",
        _ => return Ok(()),
    };

    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
                UPDATE ai_tasks
                SET status = $2,
                    result = COALESCE($3, result),
                    error_message = $4,
                    completed_at = now(),
                    updated_at = now()
                WHERE id = $1
                  AND status NOT IN ('completed', 'failed', 'cancelled')
            ",
        vec![
            task_id.into(),
            task_status.to_string().into(),
            result.into(),
            error_message.into(),
        ],
    ))
    .await?;
    Ok(())
}

async fn list_invocations_inner(
    state: &AppState,
    project_id: Option<Uuid>,
    query: ListInvocationsQuery,
) -> Result<Json<ApiResponse<PaginatedData<InvocationResponse>>>, ApiError> {
    if let Some(status) = query.status.as_deref() {
        normalize_invocation_status(status)?;
    }
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;

    let mut where_parts = Vec::new();
    let mut values = Vec::new();
    let mut idx = 1;
    if let Some(project_id) = project_id {
        where_parts.push(format!("project_id = ${idx}"));
        values.push(project_id.into());
        idx += 1;
    }
    if let Some(status) = query.status {
        where_parts.push(format!("status = ${idx}"));
        values.push(normalize_invocation_status(&status)?.into());
        idx += 1;
    }
    let where_sql = if where_parts.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_parts.join(" AND "))
    };

    let count_sql = format!("SELECT COUNT(*)::bigint AS count FROM agent_invocations {where_sql}");
    let total = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            &count_sql,
            values.clone(),
        ))
        .await?
        .ok_or_else(|| ApiError::Internal)?
        .try_get::<i64>("", "count")?;

    values.push(per_page.into());
    values.push(offset.into());
    let list_sql = format!(
        r"
            SELECT id, workspace_id, project_id, actor_id, target_agent_id, source_task_id,
                   trigger_kind, trigger_ref_type, trigger_ref_id, connector_id, connector_kind,
                   status, payload, result, error_message, audit_chain_id, created_at, updated_at
            FROM agent_invocations
            {where_sql}
            ORDER BY created_at DESC
            LIMIT ${idx} OFFSET ${}
        ",
        idx + 1
    );
    let items =
        InvocationResponse::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, &list_sql, values))
            .all(&state.db)
            .await?;
    let total_pages = if total == 0 {
        0
    } else {
        ((total as f64) / (per_page as f64)).ceil() as i64
    };

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages,
    }))
}

async fn find_connector(
    state: &AppState,
    workspace_id: Uuid,
    connector_id: Uuid,
) -> Result<ConnectorResponse, ApiError> {
    find_connector_with_conn(&state.db, workspace_id, connector_id).await
}

async fn find_connector_with_conn<C>(
    db: &C,
    workspace_id: Uuid,
    connector_id: Uuid,
) -> Result<ConnectorResponse, ApiError>
where
    C: ConnectionTrait,
{
    ConnectorResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, webhook_id, kind, name, description, endpoint,
                   auth_policy, capability_manifest, is_active, created_by, created_at, updated_at
            FROM connectors
            WHERE id = $1 AND workspace_id = $2
        ",
        vec![connector_id.into(), workspace_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("connector not found".to_string()))
}

async fn find_form_inbox_scope(state: &AppState, form_id: Uuid) -> Result<FormInboxScopeRow, ApiError> {
    FormInboxScopeRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT workspace_id, project_id, key AS form_key
            FROM project_forms
            WHERE id = $1 AND archived_at IS NULL
        ",
        vec![form_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("form not found".to_string()))
}

fn form_inbox_scope_sql(form_id_param: &str, form_key_param: &str, idempotency_param: &str) -> String {
    format!(
        r"(
            payload->>'form_id' = {form_id_param}
            OR payload#>>'{{payload,form_id}}' = {form_id_param}
            OR payload->>'form_key' = {form_key_param}
            OR payload#>>'{{payload,form_key}}' = {form_key_param}
            OR idempotency_key LIKE {idempotency_param}
        )"
    )
}

async fn insert_connector_event<C>(
    db: &C,
    connector: &ConnectorResponse,
    event_type: &str,
    actor_id: Option<Uuid>,
    source: Value,
    payload: Value,
) -> Result<(), ApiError>
where
    C: ConnectionTrait,
{
    insert_business_event(
        db,
        BusinessEventInput {
            workspace_id: connector.workspace_id,
            project_id: connector.project_id,
            event_type: event_type.to_string(),
            aggregate_type: "connector".to_string(),
            aggregate_id: connector.id.to_string(),
            actor_id,
            source,
            payload,
            metadata: json!({
                "connector_id": connector.id,
                "connector_kind": connector.kind,
                "project_id": connector.project_id
            }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    Ok(())
}

async fn insert_invocation_event<C>(db: &C, input: InvocationEventInput<'_>) -> Result<(), ApiError>
where
    C: ConnectionTrait,
{
    insert_business_event(
        db,
        BusinessEventInput {
            workspace_id: input.invocation.workspace_id,
            project_id: input.invocation.project_id,
            event_type: input.event_type.to_string(),
            aggregate_type: "invocation".to_string(),
            aggregate_id: input.invocation.id.to_string(),
            actor_id: input.actor_id,
            source: input.source,
            payload: json!({
                "invocation_id": input.invocation.id,
                "workspace_id": input.invocation.workspace_id,
                "project_id": input.invocation.project_id,
                "actor_id": input.invocation.actor_id,
                "target_agent_id": input.invocation.target_agent_id,
                "source_task_id": input.invocation.source_task_id,
                "trigger_kind": input.invocation.trigger_kind,
                "trigger_ref_type": input.invocation.trigger_ref_type,
                "trigger_ref_id": input.invocation.trigger_ref_id,
                "connector_id": input.invocation.connector_id,
                "connector_kind": input.invocation.connector_kind,
                "status": input.invocation.status,
                "previous_status": input.previous_status,
                "payload": input.invocation.payload,
                "result": input.invocation.result,
                "error_message": input.invocation.error_message,
                "audit_chain_id": input.invocation.audit_chain_id
            }),
            metadata: json!({
                "invocation_id": input.invocation.id,
                "trigger_kind": input.invocation.trigger_kind,
                "connector_id": input.invocation.connector_id,
                "connector_kind": input.invocation.connector_kind,
                "previous_status": input.previous_status,
                "status": input.invocation.status
            }),
            correlation_id: input.invocation.audit_chain_id,
            causation_id: input.invocation.source_task_id,
            idempotency_key: None,
        },
    )
    .await?;
    Ok(())
}

fn invocation_event_type_for_status(status: &str) -> &'static str {
    match status {
        "running" => "invocation.running",
        "completed" => "invocation.completed",
        "failed" => "invocation.failed",
        "cancelled" => "invocation.cancelled",
        _ => "invocation.status_changed",
    }
}

async fn find_invocation(state: &AppState, invocation_id: Uuid) -> Result<InvocationResponse, ApiError> {
    find_invocation_with_conn(&state.db, invocation_id).await
}

async fn find_invocation_with_conn<C>(db: &C, invocation_id: Uuid) -> Result<InvocationResponse, ApiError>
where
    C: ConnectionTrait,
{
    InvocationResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, actor_id, target_agent_id, source_task_id,
                   trigger_kind, trigger_ref_type, trigger_ref_id, connector_id, connector_kind,
                   status, payload, result, error_message, audit_chain_id, created_at, updated_at
            FROM agent_invocations
            WHERE id = $1
        ",
        vec![invocation_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("invocation not found".to_string()))
}

async fn find_receipt_idempotency_row<C>(
    db: &C,
    source_kind: &str,
    idempotency_key: &str,
) -> Result<Option<ReceiptIdempotencyRow>, ApiError>
where
    C: ConnectionTrait,
{
    ReceiptIdempotencyRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT workspace_id, project_id, source_id, event_type, payload
            FROM event_inbox
            WHERE source_kind = $1 AND idempotency_key = $2
        ",
        vec![source_kind.to_string().into(), idempotency_key.to_string().into()],
    ))
    .one(db)
    .await
    .map_err(ApiError::from)
}

async fn find_project_workspace(state: &AppState, project_id: Uuid) -> Result<ProjectWorkspaceRow, ApiError> {
    ProjectWorkspaceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))
}

async fn ensure_project_in_workspace(state: &AppState, workspace_id: Uuid, project_id: Uuid) -> Result<(), ApiError> {
    let project = find_project_workspace(state, project_id).await?;
    if project.workspace_id != workspace_id {
        return Err(ApiError::BadRequest("project does not belong to workspace".to_string()));
    }
    Ok(())
}

async fn ensure_workspace_access(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&BotAuthContext>,
    workspace_id: Uuid,
) -> Result<(Uuid, bool, bool), ApiError> {
    if let Some(bot) = bot {
        if bot.workspace_id != workspace_id {
            return Err(ApiError::Forbidden("bot not authorized for this workspace".to_string()));
        }
        let is_admin = bot.permissions.iter().any(|p| p == "admin");
        return Ok((bot.bot_id, is_admin, true));
    }

    let user_id = parse_user_id(claims)?;
    let role = ensure_workspace_member(state, workspace_id, user_id).await?;
    Ok((user_id, matches!(role.as_str(), "owner" | "admin"), false))
}

async fn ensure_workspace_member(state: &AppState, workspace_id: Uuid, user_id: Uuid) -> Result<String, ApiError> {
    let row = RoleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        vec![workspace_id.into(), user_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::Forbidden("workspace access denied".to_string()))?;
    Ok(row.role)
}

fn parse_user_id(claims: &JwtClaims) -> Result<Uuid, ApiError> {
    Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))
}

fn normalize_connector_kind(kind: &str) -> Result<String, ApiError> {
    let normalized = kind.trim().to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "webhook" | "mcp" | "rest" | "cli" | "openprx_tunnel" | "print" | "device"
    ) {
        Ok(normalized)
    } else {
        Err(ApiError::BadRequest("invalid connector kind".to_string()))
    }
}

fn normalize_invocation_status(status: &str) -> Result<String, ApiError> {
    let normalized = status.trim().to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "pending" | "dispatched" | "running" | "completed" | "failed" | "cancelled"
    ) {
        Ok(normalized)
    } else {
        Err(ApiError::BadRequest("invalid invocation status".to_string()))
    }
}

fn normalize_receipt_status(status: &str) -> Result<String, ApiError> {
    let normalized = status.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "received" | "accepted" | "running" => Ok("received".to_string()),
        "completed" | "success" | "succeeded" | "ok" => Ok("completed".to_string()),
        "failed" | "failure" | "error" => Ok("failed".to_string()),
        _ => Err(ApiError::BadRequest("invalid receipt status".to_string())),
    }
}

fn normalize_receipt_idempotency_key(
    idempotency_key: Option<String>,
    invocation_id: Uuid,
    receipt_status: &str,
) -> Result<String, ApiError> {
    let key = idempotency_key
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("connector-receipt:{invocation_id}:{receipt_status}"));
    if key.len() > 512 {
        return Err(ApiError::BadRequest("receipt idempotency key is too long".to_string()));
    }
    if key.chars().any(|c| c.is_control()) {
        return Err(ApiError::BadRequest(
            "receipt idempotency key cannot contain control characters".to_string(),
        ));
    }
    Ok(key)
}

fn validate_connector_receipt_target(invocation: &InvocationResponse, payload: &Value) -> Result<(), ApiError> {
    if invocation.connector_id.is_none() {
        return Err(ApiError::BadRequest(
            "receipt can only be reported for connector invocations".to_string(),
        ));
    }

    validate_uuid_payload_claim(payload, "invocation_id", Some(invocation.id))?;
    validate_uuid_payload_claim(payload, "workspace_id", Some(invocation.workspace_id))?;
    validate_uuid_payload_claim(payload, "project_id", invocation.project_id)?;
    validate_uuid_payload_claim(payload, "connector_id", invocation.connector_id)?;
    validate_uuid_payload_claim(payload, "trigger_ref_id", invocation.trigger_ref_id)?;

    if let Some(trigger_ref_type) = payload.get("trigger_ref_type").and_then(Value::as_str)
        && invocation.trigger_ref_type.as_deref() != Some(trigger_ref_type)
    {
        return Err(ApiError::BadRequest(
            "receipt trigger_ref_type does not match invocation".to_string(),
        ));
    }

    Ok(())
}

fn validate_receipt_idempotency_replay(
    existing: &ReceiptIdempotencyRow,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    source_id: Option<&str>,
    event_type: &str,
    payload: &Value,
) -> Result<(), ApiError> {
    if existing.workspace_id != workspace_id
        || existing.project_id != project_id
        || existing.source_id.as_deref() != source_id
        || existing.event_type != event_type
        || &existing.payload != payload
    {
        return Err(ApiError::Conflict(
            "receipt idempotency key already exists with different content".to_string(),
        ));
    }
    Ok(())
}

fn validate_uuid_payload_claim(payload: &Value, field: &str, expected: Option<Uuid>) -> Result<(), ApiError> {
    let Some(value) = payload.get(field) else {
        return Ok(());
    };
    if value.is_null() {
        if expected.is_some() {
            return Err(ApiError::BadRequest(format!(
                "receipt {field} does not match invocation"
            )));
        }
        return Ok(());
    }
    let actual = value
        .as_str()
        .ok_or_else(|| ApiError::BadRequest(format!("receipt {field} must be a UUID string")))?;
    let actual = Uuid::parse_str(actual).map_err(|_| ApiError::BadRequest(format!("receipt {field} is invalid")))?;
    if Some(actual) != expected {
        return Err(ApiError::BadRequest(format!(
            "receipt {field} does not match invocation"
        )));
    }
    Ok(())
}

fn normalize_inbox_status(status: &str) -> Result<String, ApiError> {
    let normalized = status.trim().to_ascii_lowercase();
    if matches!(normalized.as_str(), "received" | "processing" | "processed" | "failed") {
        Ok(normalized)
    } else {
        Err(ApiError::BadRequest("invalid inbox status".to_string()))
    }
}

fn normalize_tool_call_status(status: &str) -> Result<String, ApiError> {
    let normalized = status.trim().to_ascii_lowercase();
    if matches!(normalized.as_str(), "succeeded" | "failed") {
        Ok(normalized)
    } else {
        Err(ApiError::BadRequest("invalid tool call status".to_string()))
    }
}

fn normalize_tool_name(tool_name: &str) -> Result<String, ApiError> {
    let normalized = tool_name.trim();
    if normalized.is_empty() || normalized.len() > 160 {
        return Err(ApiError::BadRequest("invalid tool name".to_string()));
    }
    if normalized
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        Ok(normalized.to_string())
    } else {
        Err(ApiError::BadRequest("invalid tool name".to_string()))
    }
}

fn normalize_tool_transport(transport: &str) -> Result<String, ApiError> {
    let normalized = transport.trim().to_ascii_lowercase();
    if normalized.is_empty() || normalized.len() > 64 {
        return Err(ApiError::BadRequest("invalid tool transport".to_string()));
    }
    if normalized
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        Ok(normalized)
    } else {
        Err(ApiError::BadRequest("invalid tool transport".to_string()))
    }
}

fn truncate_string(value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value
    } else {
        value.chars().take(max_chars).collect()
    }
}

fn normalize_trigger_kind(kind: &str) -> Result<String, ApiError> {
    let normalized = kind.trim().to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "mention" | "assigned" | "workflow" | "proposal_vote" | "mcp" | "schedule" | "manual"
    ) {
        Ok(normalized)
    } else {
        Err(ApiError::BadRequest("invalid trigger kind".to_string()))
    }
}

fn ensure_json_object(value: Value, field: &str) -> Result<Value, ApiError> {
    if value.is_object() {
        Ok(value)
    } else {
        Err(ApiError::BadRequest(format!("{field} must be a JSON object")))
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    use super::{
        InvocationResponse, ReceiptIdempotencyRow, normalize_connector_kind, normalize_invocation_status,
        normalize_receipt_idempotency_key, normalize_receipt_status, normalize_tool_call_status, normalize_tool_name,
        normalize_tool_transport, normalize_trigger_kind, truncate_string, validate_connector_receipt_target,
        validate_receipt_idempotency_replay,
    };

    fn invocation_with_connector() -> InvocationResponse {
        let now = Utc::now();
        InvocationResponse {
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            project_id: Some(Uuid::new_v4()),
            actor_id: None,
            target_agent_id: None,
            source_task_id: None,
            trigger_kind: "manual".to_string(),
            trigger_ref_type: Some("form".to_string()),
            trigger_ref_id: Some(Uuid::new_v4()),
            connector_id: Some(Uuid::new_v4()),
            connector_kind: Some("print".to_string()),
            status: "dispatched".to_string(),
            payload: json!({}),
            result: None,
            error_message: None,
            audit_chain_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn connector_kind_accepts_first_class_peer_types() {
        for kind in ["webhook", "mcp", "rest", "cli", "openprx_tunnel"] {
            assert_eq!(normalize_connector_kind(kind).unwrap(), kind);
        }
    }

    #[test]
    fn invocation_status_accepts_ledger_lifecycle() {
        for status in ["pending", "dispatched", "running", "completed", "failed", "cancelled"] {
            assert_eq!(normalize_invocation_status(status).unwrap(), status);
        }
    }

    #[test]
    fn receipt_status_accepts_external_consumer_aliases() {
        assert_eq!(normalize_receipt_status(" accepted ").unwrap(), "received");
        assert_eq!(normalize_receipt_status("SUCCESS").unwrap(), "completed");
        assert_eq!(normalize_receipt_status("error").unwrap(), "failed");
        assert!(normalize_receipt_status("queued").is_err());
    }

    #[test]
    fn receipt_idempotency_key_is_trimmed_defaulted_and_bounded() {
        let invocation_id = Uuid::new_v4();
        assert_eq!(
            normalize_receipt_idempotency_key(Some(" receipt-1 ".to_string()), invocation_id, "completed").unwrap(),
            "receipt-1"
        );
        assert_eq!(
            normalize_receipt_idempotency_key(Some(" ".to_string()), invocation_id, "completed").unwrap(),
            format!("connector-receipt:{invocation_id}:completed")
        );
        assert!(normalize_receipt_idempotency_key(Some("bad\nkey".to_string()), invocation_id, "completed").is_err());
        assert!(normalize_receipt_idempotency_key(Some("x".repeat(513)), invocation_id, "completed").is_err());
    }

    #[test]
    fn receipt_validation_requires_connector_invocation() {
        let mut invocation = invocation_with_connector();
        invocation.connector_id = None;
        assert!(validate_connector_receipt_target(&invocation, &json!({})).is_err());
    }

    #[test]
    fn receipt_validation_accepts_matching_scope_claims() {
        let invocation = invocation_with_connector();
        let payload = json!({
            "invocation_id": invocation.id,
            "workspace_id": invocation.workspace_id,
            "project_id": invocation.project_id,
            "connector_id": invocation.connector_id,
            "trigger_ref_type": invocation.trigger_ref_type,
            "trigger_ref_id": invocation.trigger_ref_id,
            "printed": true
        });
        validate_connector_receipt_target(&invocation, &payload).unwrap();
    }

    #[test]
    fn receipt_validation_rejects_mismatched_scope_claims() {
        let invocation = invocation_with_connector();
        assert!(validate_connector_receipt_target(&invocation, &json!({ "invocation_id": Uuid::new_v4() })).is_err());
        assert!(validate_connector_receipt_target(&invocation, &json!({ "connector_id": Uuid::new_v4() })).is_err());
        assert!(validate_connector_receipt_target(&invocation, &json!({ "project_id": "not-a-uuid" })).is_err());
        assert!(validate_connector_receipt_target(&invocation, &json!({ "trigger_ref_type": "other" })).is_err());
    }

    #[test]
    fn receipt_idempotency_replay_rejects_changed_content() {
        let workspace_id = Uuid::new_v4();
        let project_id = Some(Uuid::new_v4());
        let source_id = Uuid::new_v4().to_string();
        let payload = json!({ "receipt_status": "completed", "printed": true });
        let existing = ReceiptIdempotencyRow {
            workspace_id,
            project_id,
            source_id: Some(source_id.clone()),
            event_type: "connector.delivery.received".to_string(),
            payload: payload.clone(),
        };

        validate_receipt_idempotency_replay(
            &existing,
            workspace_id,
            project_id,
            Some(source_id.as_str()),
            "connector.delivery.received",
            &payload,
        )
        .unwrap();
        assert!(
            validate_receipt_idempotency_replay(
                &existing,
                workspace_id,
                project_id,
                Some(source_id.as_str()),
                "connector.delivery.failed",
                &payload,
            )
            .is_err()
        );
        assert!(
            validate_receipt_idempotency_replay(
                &existing,
                workspace_id,
                project_id,
                Some(source_id.as_str()),
                "connector.delivery.received",
                &json!({ "receipt_status": "completed", "printed": false }),
            )
            .is_err()
        );
    }

    #[test]
    fn tool_call_status_accepts_audit_outcomes() {
        assert_eq!(normalize_tool_call_status("succeeded").unwrap(), "succeeded");
        assert_eq!(normalize_tool_call_status("FAILED").unwrap(), "failed");
        assert!(normalize_tool_call_status("running").is_err());
    }

    #[test]
    fn tool_call_names_and_transport_are_strict_identifiers() {
        assert_eq!(normalize_tool_name(" comments.create ").unwrap(), "comments.create");
        assert_eq!(normalize_tool_transport("MCP_STDIO").unwrap(), "mcp_stdio");
        assert!(normalize_tool_name("comments create").is_err());
        assert!(normalize_tool_transport("../stdio").is_err());
    }

    #[test]
    fn tool_call_summary_truncation_is_character_safe() {
        assert_eq!(truncate_string("abcdef".to_string(), 3), "abc");
        assert_eq!(truncate_string("好好好".to_string(), 2), "好好");
    }

    #[test]
    fn trigger_kind_accepts_phase_three_invocation_sources() {
        for kind in [
            "mention",
            "assigned",
            "workflow",
            "proposal_vote",
            "mcp",
            "schedule",
            "manual",
        ] {
            assert_eq!(normalize_trigger_kind(kind).unwrap(), kind);
        }
    }
}
