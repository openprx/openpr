use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use hmac::{Hmac, Mac};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait, Value as SeaValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
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
    // A connector created through this endpoint is never linked to a legacy webhook secret.
    validate_connector_auth_policy(&auth_policy, false)?;
    if let Some(endpoint) = req.endpoint.as_deref() {
        validate_connector_endpoint(endpoint).await?;
    }
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
    if let Some(auth_policy) = req.auth_policy.as_ref() {
        validate_connector_auth_policy(auth_policy, existing.webhook_id.is_some())?;
    }
    if let Some(endpoint) = req.endpoint.as_deref() {
        validate_connector_endpoint(endpoint).await?;
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
        format!(
            r"
            UPDATE agent_invocations
            SET status = CASE
                    WHEN status IN ('completed', 'failed', 'cancelled') THEN status
                    ELSE $2
                END,
                result = {RESULT_MERGE_KEEPING_DELIVERY},
                error_message = COALESCE($4, error_message),
                updated_at = now()
            WHERE id = $1
        "
        ),
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

/// SQL expression that merges a caller supplied `result` payload into the stored one.
///
/// The delivery bookkeeping the worker relies on lives under `result.connector_delivery`.
/// Replacing the whole column (what a progress report or a receipt used to do) erased the lease
/// and the retry schedule, which left the invocation invisible to the pickup query forever: it was
/// never retried, never dead lettered and never logged. The bookkeeping key is therefore always
/// carried over, whatever the caller sends.
const RESULT_MERGE_KEEPING_DELIVERY: &str = r"CASE
                    WHEN $3::jsonb IS NULL THEN result
                    WHEN jsonb_typeof($3::jsonb) = 'object'
                         AND jsonb_typeof(result -> 'connector_delivery') = 'object'
                        THEN $3::jsonb || jsonb_build_object('connector_delivery', result -> 'connector_delivery')
                    ELSE $3::jsonb
                END";

async fn update_invocation_status(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&BotAuthContext>,
    invocation_id: Uuid,
    status: &str,
    result: Option<Value>,
    error_message: Option<String>,
) -> Result<(), ApiError> {
    let invocation = find_invocation(state, invocation_id).await?;
    ensure_workspace_access(state, claims, bot, invocation.workspace_id).await?;
    // Terminal invocations are frozen for every caller, including progress reports: resurrecting a
    // completed delivery to `running` puts it back in the state the pickup query cannot resolve.
    if matches!(invocation.status.as_str(), "completed" | "failed" | "cancelled") {
        return Err(ApiError::BadRequest("invocation is already terminal".to_string()));
    }

    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r"
                UPDATE agent_invocations
                SET status = $2,
                    result = {RESULT_MERGE_KEEPING_DELIVERY},
                    error_message = $4,
                    updated_at = now()
                WHERE id = $1
                  AND status NOT IN ('completed', 'failed', 'cancelled')
            "
        ),
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

    // `sync_source_task_terminal_status` ignores non terminal statuses, so a progress report
    // still leaves the source task untouched.
    if let Some(task_id) = invocation.source_task_id {
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

/// Header carrying `sha256=<lowercase hex>` HMAC-SHA256 over the exact request body.
pub const DELIVERY_SIGNATURE_HEADER: &str = "X-Webhook-Signature";
/// Header carrying the business event type of the delivery.
pub const DELIVERY_EVENT_HEADER: &str = "X-Webhook-Event";
/// Header carrying the invocation id, stable across delivery retries.
pub const DELIVERY_ID_HEADER: &str = "X-Webhook-Delivery";
/// Comma separated hosts (`host` or `host:port`) exempt from the private address checks.
pub const OUTBOUND_ALLOWED_HOSTS_ENV: &str = "OPENPR_OUTBOUND_ALLOWED_HOSTS";
/// Set to `1`/`true` to disable outbound private address checks entirely.
pub const OUTBOUND_ALLOW_PRIVATE_ENV: &str = "OPENPR_OUTBOUND_ALLOW_PRIVATE";
/// Schema objects the delivery pipeline cannot run without.
///
/// Each entry is a label and a boolean expression. The lease token and the two pickup predicates
/// come from 0048, the duplicate tagging from 0049; both files are outside the migration adoption
/// cutoff, so an existing database re-executes them. The pre-ledger runner downgraded a failed
/// migration to a warning, which left the worker silently unable to complete a delivery and
/// nothing in the log to explain it.
const DELIVERY_SCHEMA_CHECKS: &[(&str, &str)] = &[
    (
        "event_outbox.lease_token column (0048)",
        "SELECT EXISTS (
            SELECT 1 FROM information_schema.columns
            WHERE table_schema = 'public' AND table_name = 'event_outbox' AND column_name = 'lease_token'
        ) AS present",
    ),
    (
        "agent_invocations.duplicate_of column (0049)",
        "SELECT EXISTS (
            SELECT 1 FROM information_schema.columns
            WHERE table_schema = 'public' AND table_name = 'agent_invocations' AND column_name = 'duplicate_of'
        ) AS present",
    ),
    (
        "idx_event_outbox_lease_recovery index (0048)",
        "SELECT to_regclass('public.idx_event_outbox_lease_recovery') IS NOT NULL AS present",
    ),
    (
        "idx_agent_invocations_connector_pickup index (0048)",
        "SELECT to_regclass('public.idx_agent_invocations_connector_pickup') IS NOT NULL AS present",
    ),
    (
        "uq_agent_invocations_audit_chain_connector unique index (0049)",
        "SELECT to_regclass('public.uq_agent_invocations_audit_chain_connector') IS NOT NULL AS present",
    ),
    (
        // The fan-out insert infers this exact predicate in its ON CONFLICT clause. An index left
        // over from an earlier build has a shorter predicate and would make every insert fail.
        "uq_agent_invocations_audit_chain_connector predicate covers duplicate_of (0049)",
        "SELECT EXISTS (
            SELECT 1 FROM pg_indexes
            WHERE schemaname = 'public'
              AND indexname = 'uq_agent_invocations_audit_chain_connector'
              AND indexdef LIKE '%duplicate_of IS NULL%'
        ) AS present",
    ),
];

/// Asserts the delivery schema before any polling starts.
///
/// Returns the labels of everything missing rather than the first failure, so one restart tells the
/// operator the whole story.
pub async fn verify_delivery_schema<C>(db: &C) -> Result<(), String>
where
    C: ConnectionTrait,
{
    let mut missing = Vec::new();
    for (label, sql) in DELIVERY_SCHEMA_CHECKS {
        let row = db
            .query_one(Statement::from_string(DbBackend::Postgres, (*sql).to_string()))
            .await
            .map_err(|err| format!("delivery schema check {label} could not run: {err}"))?;
        let present = match row {
            Some(row) => row
                .try_get::<bool>("", "present")
                .map_err(|err| format!("delivery schema check {label} returned no answer: {err}"))?,
            None => false,
        };
        if !present {
            missing.push(*label);
        }
    }

    if missing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "delivery schema is incomplete, missing: {}. The owning migrations did not apply; \
         inspect `SELECT name, status, error FROM schema_migrations WHERE status <> 'applied'` and \
         restart the API to retry them",
        missing.join(", ")
    ))
}

/// Namespace a connector credential environment variable must live in.
///
/// A connector endpoint is attacker controlled by design, so an unconstrained `env:NAME`
/// reference would turn every connector into a read primitive for the whole process
/// environment (`JWT_SECRET`, `DATABASE_URL`, object storage keys). Credentials meant for
/// connectors must be provisioned under this prefix and nowhere else.
pub const CONNECTOR_SECRET_ENV_PREFIX: &str = "OPENPR_CONNECTOR_SECRET_";

/// Environment variables that must never be reachable through a credential reference.
///
/// [`CONNECTOR_SECRET_ENV_PREFIX`] already excludes all of them; this list is a second,
/// independent gate that keeps holding if the namespace rule is ever widened.
const DENIED_CREDENTIAL_ENV_NAMES: &[&str] = &["JWT_SECRET", "DATABASE_URL", "OPENPR_BOT_TOKEN", "RUST_LOG"];

/// Environment variable prefixes that must never be reachable through a credential reference.
const DENIED_CREDENTIAL_ENV_PREFIXES: &[&str] = &["POSTGRES_", "PG", "AWS_", "OPENPR_OBJECT_STORAGE_"];

/// Authentication mode a connector declares in `auth_policy.mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorAuthMode {
    None,
    Hmac,
    Bearer,
}

impl ConnectorAuthMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Hmac => "hmac",
            Self::Bearer => "bearer",
        }
    }
}

/// Where the delivery credential is read from. Never holds the credential itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectorCredentialSource {
    WebhookSecret,
    Env(String),
}

/// Declared authentication behaviour resolved from `connectors.auth_policy`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorAuthPlan {
    pub mode: ConnectorAuthMode,
    pub source: Option<ConnectorCredentialSource>,
}

/// Resolves the effective delivery authentication from a connector `auth_policy`.
///
/// The declared mode and the runtime behaviour are kept in sync: a policy that declares
/// `hmac`/`bearer` without a usable credential source is rejected instead of silently
/// downgrading to an unsigned delivery.
pub fn connector_auth_plan(auth_policy: &Value, webhook_linked: bool) -> Result<ConnectorAuthPlan, String> {
    if ["secret", "token", "password"]
        .iter()
        .any(|key| auth_policy.get(*key).is_some())
    {
        return Err("auth_policy must not store raw secrets, use secret_ref = \"env:NAME\"".to_string());
    }

    let mode = auth_policy
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let reference = auth_policy
        .get("secret_ref")
        .or_else(|| auth_policy.get("token_ref"))
        .and_then(Value::as_str);
    let source = match reference {
        Some(raw) => Some(parse_credential_ref(raw)?),
        None if webhook_linked => Some(ConnectorCredentialSource::WebhookSecret),
        None => None,
    };

    match mode.as_str() {
        "" => Ok(source.map_or(
            ConnectorAuthPlan {
                mode: ConnectorAuthMode::None,
                source: None,
            },
            |source| ConnectorAuthPlan {
                mode: ConnectorAuthMode::Hmac,
                source: Some(source),
            },
        )),
        "none" | "unsigned" => Ok(ConnectorAuthPlan {
            mode: ConnectorAuthMode::None,
            source: None,
        }),
        "hmac" | "hmac_sha256" | "hmac-sha256" => Ok(ConnectorAuthPlan {
            mode: ConnectorAuthMode::Hmac,
            source: Some(source.ok_or_else(|| {
                "auth_policy mode hmac requires secret_ref = \"env:NAME\" or a linked webhook secret".to_string()
            })?),
        }),
        "bearer" | "token" => match source {
            Some(ConnectorCredentialSource::Env(name)) => Ok(ConnectorAuthPlan {
                mode: ConnectorAuthMode::Bearer,
                source: Some(ConnectorCredentialSource::Env(name)),
            }),
            _ => Err("auth_policy mode bearer requires token_ref = \"env:NAME\"".to_string()),
        },
        other => Err(format!("unsupported auth_policy mode {other}")),
    }
}

fn parse_credential_ref(raw: &str) -> Result<ConnectorCredentialSource, String> {
    let Some(name) = raw.trim().strip_prefix("env:") else {
        return Err("credential reference must use the env:NAME form".to_string());
    };
    let name = name.trim();
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit());
    if !valid {
        return Err("credential reference env name must match [A-Z_][A-Z0-9_]*".to_string());
    }
    if DENIED_CREDENTIAL_ENV_NAMES.contains(&name)
        || DENIED_CREDENTIAL_ENV_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
    {
        return Err(format!(
            "credential reference env name {name} is reserved by the platform"
        ));
    }
    if name.len() <= CONNECTOR_SECRET_ENV_PREFIX.len() || !name.starts_with(CONNECTOR_SECRET_ENV_PREFIX) {
        return Err(format!(
            "credential reference env name must start with {CONNECTOR_SECRET_ENV_PREFIX}"
        ));
    }
    Ok(ConnectorCredentialSource::Env(name.to_string()))
}

/// Reads the credential for a resolved source. The returned value must never be logged.
pub fn resolve_connector_credential(
    source: &ConnectorCredentialSource,
    webhook_secret: Option<&str>,
) -> Result<String, String> {
    match source {
        ConnectorCredentialSource::WebhookSecret => webhook_secret
            .map(str::trim)
            .filter(|secret| !secret.is_empty())
            .map(ToString::to_string)
            .ok_or_else(|| "linked webhook secret is not configured".to_string()),
        ConnectorCredentialSource::Env(name) => match std::env::var(name) {
            Ok(value) if !value.trim().is_empty() => Ok(value),
            Ok(_) => Err(format!("credential env var {name} is empty")),
            Err(_) => Err(format!("credential env var {name} is not set")),
        },
    }
}

/// HMAC-SHA256 of the raw delivery body, lowercase hex encoded.
pub fn sign_delivery_body(secret: &str, body: &[u8]) -> Result<String, String> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|err| format!("invalid signing secret: {err}"))?;
    mac.update(body);
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// Value of [`DELIVERY_SIGNATURE_HEADER`], identical in shape to the legacy webhook path.
pub fn delivery_signature_header_value(secret: &str, body: &[u8]) -> Result<String, String> {
    Ok(format!("sha256={}", sign_delivery_body(secret, body)?))
}

fn outbound_allowlist() -> String {
    std::env::var(OUTBOUND_ALLOWED_HOSTS_ENV).unwrap_or_default()
}

fn outbound_private_targets_allowed() -> bool {
    std::env::var(OUTBOUND_ALLOW_PRIVATE_ENV)
        .is_ok_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
}

/// Matches a host (optionally with port) against a comma separated allowlist.
pub fn host_is_allowlisted(host: &str, port: Option<u16>, allowlist: &str) -> bool {
    let host = normalize_host(host);
    allowlist
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .any(|entry| {
            let entry = entry.to_ascii_lowercase();
            match entry.rsplit_once(':') {
                Some((entry_host, entry_port))
                    if !entry_port.is_empty() && entry_port.chars().all(|c| c.is_ascii_digit()) =>
                {
                    normalize_host(entry_host) == host && port.is_some_and(|value| value.to_string() == entry_port)
                }
                _ => normalize_host(&entry) == host,
            }
        })
}

fn normalize_host(host: &str) -> String {
    host.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase()
}

/// Rejects addresses that must never be reachable from a user supplied endpoint.
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_ipv4(v4),
        IpAddr::V6(v6) => is_blocked_ipv6(v6),
    }
}

fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    let [first, second, ..] = ip.octets();
    first == 0
        || ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || (first == 100 && (64..128).contains(&second))
        || (first == 192 && second == 0)
        || (first == 198 && (18..20).contains(&second))
        || first >= 240
}

fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    // Checked before any embedded IPv4 extraction: `::1` and `::` also look like the deprecated
    // IPv4-compatible form and would otherwise be read as 0.0.0.1 / 0.0.0.0.
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
        return true;
    }
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_blocked_ipv4(mapped);
    }
    let segments = ip.segments();
    let [first, second, ..] = segments;
    // NAT64: the well known prefix carries the IPv4 target in its last 32 bits, every other
    // 64:ff9b::/32 form (the local-use /48 with a configurable suffix) is refused outright.
    if first == 0x0064 && second == 0xff9b {
        return if segments[2..6].iter().all(|segment| *segment == 0) {
            is_blocked_ipv4(embedded_ipv4(segments[6], segments[7]))
        } else {
            true
        };
    }
    // 6to4: 2002:V4ADDR::/16 reaches the embedded IPv4 target.
    if first == 0x2002 {
        return is_blocked_ipv4(embedded_ipv4(segments[1], segments[2]));
    }
    // Deprecated IPv4-compatible form `::a.b.c.d`.
    if segments[..6].iter().all(|segment| *segment == 0) {
        return is_blocked_ipv4(embedded_ipv4(segments[6], segments[7]));
    }
    (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80
        || (first & 0xffc0) == 0xfec0
        || first == 0x2001 && second == 0x0db8
}

/// Rebuilds the IPv4 address carried by a transition format from its two IPv6 segments.
fn embedded_ipv4(high: u16, low: u16) -> Ipv4Addr {
    Ipv4Addr::from((u32::from(high) << 16) | u32::from(low))
}

fn literal_host_ip(host: &str) -> Option<IpAddr> {
    normalize_host(host).parse::<IpAddr>().ok()
}

/// Scheme, credential and literal address checks. Does not perform DNS resolution.
pub fn parse_outbound_url(raw: &str) -> Result<reqwest::Url, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("endpoint must not be empty".to_string());
    }
    let url = reqwest::Url::parse(trimmed).map_err(|err| format!("endpoint is not a valid absolute URL: {err}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "endpoint scheme {} is not allowed, use http or https",
            url.scheme()
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("endpoint must not embed credentials".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "endpoint must contain a host".to_string())?
        .to_string();

    if outbound_private_targets_allowed()
        || host_is_allowlisted(&host, url.port_or_known_default(), &outbound_allowlist())
    {
        return Ok(url);
    }
    if literal_host_ip(&host).is_some_and(is_blocked_ip) {
        return Err(format!("endpoint host {host} points at a blocked address"));
    }
    Ok(url)
}

/// Full outbound target validation: scheme/credential checks plus DNS resolution of the host.
///
/// Hosts listed in [`OUTBOUND_ALLOWED_HOSTS_ENV`] (for example the in-cluster service names used
/// by the compose deployment) skip the address checks.
pub async fn validate_outbound_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = parse_outbound_url(raw)?;
    if outbound_private_targets_allowed() {
        return Ok(url);
    }
    let host = url.host_str().unwrap_or_default().to_string();
    let port = url.port_or_known_default();
    if host_is_allowlisted(&host, port, &outbound_allowlist()) || literal_host_ip(&host).is_some() {
        return Ok(url);
    }

    let addresses = tokio::net::lookup_host((host.as_str(), port.unwrap_or(443)))
        .await
        .map_err(|err| format!("endpoint host {host} could not be resolved: {err}"))?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(format!("endpoint host {host} could not be resolved"));
    }
    if addresses.iter().any(|address| is_blocked_ip(address.ip())) {
        return Err(format!("endpoint host {host} resolves to a blocked address"));
    }
    Ok(url)
}

async fn validate_connector_endpoint(endpoint: &str) -> Result<(), ApiError> {
    validate_outbound_url(endpoint)
        .await
        .map(|_| ())
        .map_err(ApiError::BadRequest)
}

fn validate_connector_auth_policy(auth_policy: &Value, webhook_linked: bool) -> Result<(), ApiError> {
    connector_auth_plan(auth_policy, webhook_linked)
        .map(|_| ())
        .map_err(ApiError::BadRequest)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    use std::net::IpAddr;

    use super::{
        ConnectorAuthMode, ConnectorCredentialSource, InvocationResponse, ReceiptIdempotencyRow, connector_auth_plan,
        delivery_signature_header_value, host_is_allowlisted, is_blocked_ip, normalize_connector_kind,
        normalize_invocation_status, normalize_receipt_idempotency_key, normalize_receipt_status,
        normalize_tool_call_status, normalize_tool_name, normalize_tool_transport, normalize_trigger_kind,
        parse_outbound_url, resolve_connector_credential, sign_delivery_body, truncate_string,
        validate_connector_receipt_target, validate_receipt_idempotency_replay,
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
    fn delivery_signature_matches_hmac_sha256_reference_vector() {
        // RFC-style reference vector, also proves the `sha256=<hex>` header shape.
        let signature = sign_delivery_body("key", b"The quick brown fox jumps over the lazy dog").unwrap();
        assert_eq!(
            signature,
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
        assert_eq!(
            delivery_signature_header_value("key", b"The quick brown fox jumps over the lazy dog").unwrap(),
            format!("sha256={signature}")
        );
    }

    #[test]
    fn delivery_signature_covers_the_exact_body_bytes() {
        let body = br#"{"event":"form.record.created"}"#;
        let tampered = br#"{"event":"form.record.updated"}"#;
        assert_ne!(
            sign_delivery_body("s3cret", body).unwrap(),
            sign_delivery_body("s3cret", tampered).unwrap()
        );
        assert_ne!(
            sign_delivery_body("s3cret", body).unwrap(),
            sign_delivery_body("other", body).unwrap()
        );
    }

    #[test]
    fn auth_plan_declares_only_modes_it_can_perform() {
        let webhook_linked = connector_auth_plan(&json!({ "mode": "hmac", "legacy_webhook": true }), true).unwrap();
        assert_eq!(webhook_linked.mode, ConnectorAuthMode::Hmac);
        assert_eq!(webhook_linked.source, Some(ConnectorCredentialSource::WebhookSecret));

        // hmac without any credential source must be rejected instead of delivering unsigned.
        assert!(connector_auth_plan(&json!({ "mode": "hmac" }), false).is_err());
        assert!(connector_auth_plan(&json!({ "mode": "bearer" }), true).is_err());
        assert!(connector_auth_plan(&json!({ "mode": "mtls" }), false).is_err());
        assert!(connector_auth_plan(&json!({ "mode": "hmac", "secret": "inline" }), false).is_err());
        assert!(connector_auth_plan(&json!({ "mode": "hmac", "secret_ref": "vault:x" }), false).is_err());
        assert!(connector_auth_plan(&json!({ "mode": "hmac", "secret_ref": "env:bad-name" }), false).is_err());

        let env_backed = connector_auth_plan(
            &json!({ "mode": "hmac", "secret_ref": "env:OPENPR_CONNECTOR_SECRET_TEST" }),
            false,
        )
        .unwrap();
        assert_eq!(
            env_backed.source,
            Some(ConnectorCredentialSource::Env(
                "OPENPR_CONNECTOR_SECRET_TEST".to_string()
            ))
        );

        let empty = connector_auth_plan(&json!({}), false).unwrap();
        assert_eq!(empty.mode, ConnectorAuthMode::None);
        assert_eq!(
            connector_auth_plan(&json!({}), true).unwrap().mode,
            ConnectorAuthMode::Hmac
        );
        assert_eq!(
            connector_auth_plan(&json!({ "mode": "none" }), true).unwrap().mode,
            ConnectorAuthMode::None
        );
    }

    #[test]
    fn credential_refs_cannot_reach_outside_the_connector_secret_namespace() {
        // A connector endpoint is attacker controlled, so an unconstrained env reference would let
        // whoever can create a connector have the worker post the process environment to that
        // endpoint. Reading JWT_SECRET this way is a full authentication bypass.
        for reference in [
            "env:JWT_SECRET",
            "env:DATABASE_URL",
            "env:POSTGRES_PASSWORD",
            "env:PGPASSWORD",
            "env:AWS_SECRET_ACCESS_KEY",
            "env:OPENPR_OBJECT_STORAGE_S3_SECRET_ACCESS_KEY",
            "env:OPENPR_BOT_TOKEN",
            "env:HOME",
            "env:OPENPR_CONNECTOR_SECRET_",
            "env:MY_HOOK_SECRET",
        ] {
            let policy = json!({ "mode": "bearer", "token_ref": reference });
            assert!(
                connector_auth_plan(&policy, false).is_err(),
                "{reference} must not be referenceable"
            );
        }

        let allowed = connector_auth_plan(
            &json!({ "mode": "bearer", "token_ref": "env:OPENPR_CONNECTOR_SECRET_MYHOOK" }),
            false,
        )
        .unwrap();
        assert_eq!(allowed.mode, ConnectorAuthMode::Bearer);
        assert_eq!(
            allowed.source,
            Some(ConnectorCredentialSource::Env(
                "OPENPR_CONNECTOR_SECRET_MYHOOK".to_string()
            ))
        );
    }

    #[test]
    fn credential_resolution_requires_a_configured_secret() {
        assert_eq!(
            resolve_connector_credential(&ConnectorCredentialSource::WebhookSecret, Some(" top-secret ")).unwrap(),
            "top-secret"
        );
        assert!(resolve_connector_credential(&ConnectorCredentialSource::WebhookSecret, Some("  ")).is_err());
        assert!(resolve_connector_credential(&ConnectorCredentialSource::WebhookSecret, None).is_err());
        assert!(
            resolve_connector_credential(
                &ConnectorCredentialSource::Env("OPENPR_ABSENT_TEST_SECRET".to_string()),
                None
            )
            .is_err()
        );
    }

    #[test]
    fn outbound_urls_reject_private_and_non_http_targets() {
        assert!(parse_outbound_url("http://127.0.0.1/hook").is_err());
        assert!(parse_outbound_url("http://[::1]/hook").is_err());
        assert!(parse_outbound_url("http://169.254.169.254/latest/meta-data/").is_err());
        assert!(parse_outbound_url("http://10.89.0.1:8080/hook").is_err());
        assert!(parse_outbound_url("http://192.168.1.10/hook").is_err());
        assert!(parse_outbound_url("http://0.0.0.0/hook").is_err());
        assert!(parse_outbound_url("file:///etc/passwd").is_err());
        assert!(parse_outbound_url("gopher://example.com/x").is_err());
        assert!(parse_outbound_url("http://user:pass@example.com/hook").is_err());
        assert!(parse_outbound_url("not-a-url").is_err());
        assert!(parse_outbound_url("   ").is_err());
        assert!(parse_outbound_url("https://hooks.example.com/openpr").is_ok());
    }

    #[test]
    fn blocked_ip_ranges_cover_loopback_private_and_metadata_addresses() {
        for raw in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            // Deprecated and transition formats that still reach an internal IPv4 target.
            "::7f00:1",
            "64:ff9b::7f00:1",
            "2002:c0a8:0001::",
            "fec0::1",
            "0.1.2.3",
            // NAT64 forms whose embedded target cannot be read are refused outright.
            "64:ff9b:1::1",
            "2001:db8::1",
        ] {
            let ip: IpAddr = raw.parse().unwrap();
            assert!(is_blocked_ip(ip), "{raw} must be blocked");
        }
        for raw in [
            "1.1.1.1",
            "93.184.216.34",
            "2606:4700:4700::1111",
            // The same transition formats pointing at a public target stay reachable.
            "64:ff9b::0101:0101",
            "2002:0101:0101::",
        ] {
            let ip: IpAddr = raw.parse().unwrap();
            assert!(!is_blocked_ip(ip), "{raw} must be allowed");
        }
    }

    #[test]
    fn allowlist_matches_deployment_internal_service_names() {
        assert!(host_is_allowlisted("api", Some(8080), "api,webhook"));
        assert!(host_is_allowlisted("API", Some(8080), " api , webhook "));
        assert!(host_is_allowlisted("api", Some(8080), "api:8080"));
        assert!(!host_is_allowlisted("api", Some(9090), "api:8080"));
        assert!(!host_is_allowlisted("evil.example.com", Some(443), "api,webhook"));
        assert!(!host_is_allowlisted("api", Some(8080), ""));
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
