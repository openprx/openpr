use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    events::{BusinessEventInput, insert_business_event},
    middleware::bot_auth::{BotAuthContext, require_workspace_access},
    response::{ApiResponse, PaginatedData},
};

#[derive(Debug, Serialize, FromQueryResult)]
pub struct ProjectTypeResponse {
    pub key: String,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub description: String,
    pub domain: String,
    pub default_workflow_id: Option<Uuid>,
    pub default_governance_policy_id: Option<Uuid>,
    pub enabled_capabilities: JsonValue,
    pub field_schema: JsonValue,
    pub artifact_schema: JsonValue,
    pub default_connectors: JsonValue,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct ProjectResourceResponse {
    pub id: Uuid,
    pub project_id: Uuid,
    pub kind: String,
    pub name: String,
    pub locator: JsonValue,
    pub permission_policy: JsonValue,
    pub sync_status: String,
    pub created_by: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateProjectTypeRequest {
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub domain: Option<String>,
    pub default_workflow_id: Option<Uuid>,
    pub default_governance_policy_id: Option<Uuid>,
    pub enabled_capabilities: Option<JsonValue>,
    pub field_schema: Option<JsonValue>,
    pub artifact_schema: Option<JsonValue>,
    pub default_connectors: Option<JsonValue>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProjectTypeRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub domain: Option<String>,
    pub default_workflow_id: Option<Uuid>,
    pub default_governance_policy_id: Option<Uuid>,
    pub enabled_capabilities: Option<JsonValue>,
    pub field_schema: Option<JsonValue>,
    pub artifact_schema: Option<JsonValue>,
    pub default_connectors: Option<JsonValue>,
}

#[derive(Debug, Deserialize)]
pub struct CreateProjectResourceRequest {
    pub kind: String,
    pub name: String,
    pub locator: Option<JsonValue>,
    pub permission_policy: Option<JsonValue>,
    pub sync_status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProjectResourceRequest {
    pub kind: Option<String>,
    pub name: Option<String>,
    pub locator: Option<JsonValue>,
    pub permission_policy: Option<JsonValue>,
    pub sync_status: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct ProjectWorkspaceRow {
    workspace_id: Uuid,
}

fn build_auth_extensions(claims: JwtClaims, bot: Option<Extension<BotAuthContext>>) -> axum::http::Extensions {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }
    extensions
}

fn normalize_key(raw: &str) -> Result<String, ApiError> {
    let key = raw.trim().to_ascii_lowercase();
    if key.is_empty() {
        return Err(ApiError::BadRequest("project type key is required".to_string()));
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(ApiError::BadRequest(
            "project type key must contain lowercase letters, digits, or underscores".to_string(),
        ));
    }
    Ok(key)
}

fn normalize_kind(raw: &str) -> Result<String, ApiError> {
    let kind = raw.trim().to_ascii_lowercase();
    if matches!(
        kind.as_str(),
        "repo" | "directory" | "document_library" | "crm_account" | "erp_order" | "equipment" | "site" | "custom"
    ) {
        Ok(kind)
    } else {
        Err(ApiError::BadRequest("invalid project resource kind".to_string()))
    }
}

fn ensure_json_object(value: JsonValue, field: &str) -> Result<JsonValue, ApiError> {
    if value.is_object() {
        Ok(value)
    } else {
        Err(ApiError::BadRequest(format!("{field} must be a JSON object")))
    }
}

fn ensure_json_array(value: JsonValue, field: &str) -> Result<JsonValue, ApiError> {
    if value.is_array() {
        Ok(value)
    } else {
        Err(ApiError::BadRequest(format!("{field} must be a JSON array")))
    }
}

async fn find_project_workspace(state: &AppState, project_id: Uuid) -> Result<Uuid, ApiError> {
    let row = ProjectWorkspaceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;
    Ok(row.workspace_id)
}

async fn ensure_workspace_admin(
    state: &AppState,
    extensions: &axum::http::Extensions,
    workspace_id: Uuid,
) -> Result<(), ApiError> {
    let (_, role, _) = require_workspace_access(state, extensions, workspace_id).await?;
    let role = role.trim().to_ascii_lowercase();
    if role == "owner" || role == "admin" {
        Ok(())
    } else {
        Err(ApiError::Forbidden("workspace admin or owner required".to_string()))
    }
}

pub async fn list_project_types(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let workspace_id = extensions.get::<BotAuthContext>().map(|ctx| ctx.workspace_id);

    let (sql, values) = if let Some(workspace_id) = workspace_id {
        (
            r"
                SELECT key, workspace_id, name, description, domain, default_workflow_id,
                       default_governance_policy_id, enabled_capabilities, field_schema,
                       artifact_schema, default_connectors, created_at, updated_at
                FROM project_types
                WHERE workspace_id IS NULL OR workspace_id = $1
                ORDER BY workspace_id NULLS FIRST, name
            ",
            vec![workspace_id.into()],
        )
    } else {
        (
            r"
                SELECT key, workspace_id, name, description, domain, default_workflow_id,
                       default_governance_policy_id, enabled_capabilities, field_schema,
                       artifact_schema, default_connectors, created_at, updated_at
                FROM project_types
                WHERE workspace_id IS NULL
                ORDER BY name
            ",
            Vec::new(),
        )
    };

    let items =
        ProjectTypeResponse::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .all(&state.db)
            .await?;

    Ok(ApiResponse::success(PaginatedData::from_items(items)))
}

pub async fn list_workspace_project_types(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    require_workspace_access(&state, &extensions, workspace_id).await?;

    let items = ProjectTypeResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT key, workspace_id, name, description, domain, default_workflow_id,
                   default_governance_policy_id, enabled_capabilities, field_schema,
                   artifact_schema, default_connectors, created_at, updated_at
            FROM project_types
            WHERE workspace_id IS NULL OR workspace_id = $1
            ORDER BY workspace_id NULLS FIRST, name
        ",
        vec![workspace_id.into()],
    ))
    .all(&state.db)
    .await?;

    Ok(ApiResponse::success(PaginatedData::from_items(items)))
}

pub async fn get_project_type(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(key): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let workspace_id = extensions.get::<BotAuthContext>().map(|ctx| ctx.workspace_id);
    let key = normalize_key(&key)?;

    let (sql, values) = if let Some(workspace_id) = workspace_id {
        (
            r"
                SELECT key, workspace_id, name, description, domain, default_workflow_id,
                       default_governance_policy_id, enabled_capabilities, field_schema,
                       artifact_schema, default_connectors, created_at, updated_at
                FROM project_types
                WHERE key = $1 AND (workspace_id IS NULL OR workspace_id = $2)
                ORDER BY workspace_id NULLS LAST
                LIMIT 1
            ",
            vec![key.into(), workspace_id.into()],
        )
    } else {
        (
            r"
                SELECT key, workspace_id, name, description, domain, default_workflow_id,
                       default_governance_policy_id, enabled_capabilities, field_schema,
                       artifact_schema, default_connectors, created_at, updated_at
                FROM project_types
                WHERE key = $1 AND workspace_id IS NULL
                LIMIT 1
            ",
            vec![key.into()],
        )
    };

    let item = ProjectTypeResponse::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .one(&state.db)
        .await?
        .ok_or_else(|| ApiError::NotFound("project type not found".to_string()))?;

    Ok(ApiResponse::success(item))
}

pub async fn create_project_type(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CreateProjectTypeRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    ensure_workspace_admin(&state, &extensions, workspace_id).await?;
    let actor_id = actor_id_from_extensions(&extensions)?;

    let key = normalize_key(&req.key)?;
    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("project type name is required".to_string()));
    }

    let now = chrono::Utc::now();
    let enabled_capabilities = req
        .enabled_capabilities
        .map(|value| ensure_json_array(value, "enabled_capabilities"))
        .transpose()?
        .unwrap_or_else(|| json!([]));
    let field_schema = req
        .field_schema
        .map(|value| ensure_json_object(value, "field_schema"))
        .transpose()?
        .unwrap_or_else(|| json!({}));
    let artifact_schema = req
        .artifact_schema
        .map(|value| ensure_json_object(value, "artifact_schema"))
        .transpose()?
        .unwrap_or_else(|| json!({}));
    let default_connectors = req
        .default_connectors
        .map(|value| ensure_json_array(value, "default_connectors"))
        .transpose()?
        .unwrap_or_else(|| json!([]));

    let tx = state.db.begin().await?;
    let insert = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO project_types (
                    key, workspace_id, name, description, domain, default_workflow_id,
                    default_governance_policy_id, enabled_capabilities, field_schema,
                    artifact_schema, default_connectors, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            ",
            vec![
                key.clone().into(),
                workspace_id.into(),
                req.name.trim().to_string().into(),
                req.description.unwrap_or_default().into(),
                req.domain.unwrap_or_else(|| "custom".to_string()).into(),
                req.default_workflow_id.into(),
                req.default_governance_policy_id.into(),
                enabled_capabilities.into(),
                field_schema.into(),
                artifact_schema.into(),
                default_connectors.into(),
                now.into(),
                now.into(),
            ],
        ))
        .await;

    if let Err(err) = insert {
        let message = err.to_string();
        if message.contains("duplicate key value") {
            return Err(ApiError::Conflict("project type key already exists".to_string()));
        }
        return Err(ApiError::Database(err));
    }
    let row = find_project_type_with_conn(&tx, &key).await?;
    insert_project_type_event(
        &tx,
        &row,
        "project_type.created",
        actor_id,
        json!({
            "project_type_key": row.key,
            "name": row.name,
            "domain": row.domain,
            "workspace_id": row.workspace_id
        }),
    )
    .await?;
    tx.commit().await?;

    get_project_type_by_key(&state, &key).await
}

pub async fn update_project_type(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(key): Path<String>,
    Json(req): Json<UpdateProjectTypeRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let key = normalize_key(&key)?;
    let actor_id = actor_id_from_extensions(&extensions)?;

    #[derive(Debug, FromQueryResult)]
    struct TypeWorkspaceRow {
        workspace_id: Option<Uuid>,
    }

    let type_row = TypeWorkspaceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM project_types WHERE key = $1",
        vec![key.clone().into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project type not found".to_string()))?;

    let workspace_id = type_row
        .workspace_id
        .ok_or_else(|| ApiError::Forbidden("system project types are read-only".to_string()))?;
    ensure_workspace_admin(&state, &extensions, workspace_id).await?;

    let mut updates = Vec::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut param_idx = 1;

    if let Some(name) = req.name {
        if name.trim().is_empty() {
            return Err(ApiError::BadRequest("project type name cannot be empty".to_string()));
        }
        updates.push(format!("name = ${param_idx}"));
        values.push(name.trim().to_string().into());
        param_idx += 1;
    }
    if let Some(description) = req.description {
        updates.push(format!("description = ${param_idx}"));
        values.push(description.into());
        param_idx += 1;
    }
    if let Some(domain) = req.domain {
        updates.push(format!("domain = ${param_idx}"));
        values.push(domain.into());
        param_idx += 1;
    }
    if let Some(default_workflow_id) = req.default_workflow_id {
        updates.push(format!("default_workflow_id = ${param_idx}"));
        values.push(default_workflow_id.into());
        param_idx += 1;
    }
    if let Some(default_governance_policy_id) = req.default_governance_policy_id {
        updates.push(format!("default_governance_policy_id = ${param_idx}"));
        values.push(default_governance_policy_id.into());
        param_idx += 1;
    }
    if let Some(enabled_capabilities) = req.enabled_capabilities {
        updates.push(format!("enabled_capabilities = ${param_idx}"));
        values.push(ensure_json_array(enabled_capabilities, "enabled_capabilities")?.into());
        param_idx += 1;
    }
    if let Some(field_schema) = req.field_schema {
        updates.push(format!("field_schema = ${param_idx}"));
        values.push(ensure_json_object(field_schema, "field_schema")?.into());
        param_idx += 1;
    }
    if let Some(artifact_schema) = req.artifact_schema {
        updates.push(format!("artifact_schema = ${param_idx}"));
        values.push(ensure_json_object(artifact_schema, "artifact_schema")?.into());
        param_idx += 1;
    }
    if let Some(default_connectors) = req.default_connectors {
        updates.push(format!("default_connectors = ${param_idx}"));
        values.push(ensure_json_array(default_connectors, "default_connectors")?.into());
        param_idx += 1;
    }

    if updates.is_empty() {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }

    updates.push(format!("updated_at = ${param_idx}"));
    values.push(chrono::Utc::now().into());
    param_idx += 1;
    values.push(key.clone().into());

    let sql = format!(
        "UPDATE project_types SET {} WHERE key = ${param_idx}",
        updates.join(", ")
    );
    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await?;
    let row = find_project_type_with_conn(&tx, &key).await?;
    insert_project_type_event(
        &tx,
        &row,
        "project_type.updated",
        actor_id,
        json!({
            "project_type_key": row.key,
            "name": row.name,
            "domain": row.domain,
            "workspace_id": row.workspace_id
        }),
    )
    .await?;
    tx.commit().await?;

    get_project_type_by_key(&state, &key).await
}

pub async fn list_project_resources(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let workspace_id = find_project_workspace(&state, project_id).await?;
    require_workspace_access(&state, &extensions, workspace_id).await?;

    let items = ProjectResourceResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, project_id, kind, name, locator, permission_policy, sync_status,
                   created_by, created_at, updated_at
            FROM project_resources
            WHERE project_id = $1
            ORDER BY kind, name
        ",
        vec![project_id.into()],
    ))
    .all(&state.db)
    .await?;

    Ok(ApiResponse::success(PaginatedData::from_items(items)))
}

pub async fn create_project_resource(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<CreateProjectResourceRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let workspace_id = find_project_workspace(&state, project_id).await?;
    ensure_workspace_admin(&state, &extensions, workspace_id).await?;
    let actor_id = actor_id_from_extensions(&extensions)?;

    let kind = normalize_kind(&req.kind)?;
    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("project resource name is required".to_string()));
    }

    let resource_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
                INSERT INTO project_resources (
                    id, project_id, kind, name, locator, permission_policy,
                    sync_status, created_by, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ",
        vec![
            resource_id.into(),
            project_id.into(),
            kind.into(),
            req.name.trim().to_string().into(),
            req.locator.unwrap_or_else(|| json!({})).into(),
            req.permission_policy.unwrap_or_else(|| json!({})).into(),
            req.sync_status.unwrap_or_else(|| "manual".to_string()).into(),
            actor_id.into(),
            now.into(),
            now.into(),
        ],
    ))
    .await?;
    let row = find_project_resource_with_conn(&tx, resource_id).await?;
    insert_project_resource_event(
        &tx,
        workspace_id,
        &row,
        "project_resource.created",
        actor_id,
        json!({
            "resource_id": row.id,
            "project_id": row.project_id,
            "kind": row.kind,
            "name": row.name,
            "sync_status": row.sync_status
        }),
    )
    .await?;
    tx.commit().await?;

    get_project_resource_by_id(&state, resource_id).await
}

pub async fn update_project_resource(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((project_id, resource_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateProjectResourceRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let workspace_id = find_project_workspace(&state, project_id).await?;
    ensure_workspace_admin(&state, &extensions, workspace_id).await?;
    let actor_id = actor_id_from_extensions(&extensions)?;

    let mut updates = Vec::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut param_idx = 1;

    if let Some(kind) = req.kind {
        updates.push(format!("kind = ${param_idx}"));
        values.push(normalize_kind(&kind)?.into());
        param_idx += 1;
    }
    if let Some(name) = req.name {
        if name.trim().is_empty() {
            return Err(ApiError::BadRequest(
                "project resource name cannot be empty".to_string(),
            ));
        }
        updates.push(format!("name = ${param_idx}"));
        values.push(name.trim().to_string().into());
        param_idx += 1;
    }
    if let Some(locator) = req.locator {
        updates.push(format!("locator = ${param_idx}"));
        values.push(locator.into());
        param_idx += 1;
    }
    if let Some(permission_policy) = req.permission_policy {
        updates.push(format!("permission_policy = ${param_idx}"));
        values.push(permission_policy.into());
        param_idx += 1;
    }
    if let Some(sync_status) = req.sync_status {
        updates.push(format!("sync_status = ${param_idx}"));
        values.push(sync_status.into());
        param_idx += 1;
    }

    if updates.is_empty() {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }

    updates.push(format!("updated_at = ${param_idx}"));
    values.push(chrono::Utc::now().into());
    param_idx += 1;
    values.push(project_id.into());
    param_idx += 1;
    values.push(resource_id.into());

    let sql = format!(
        "UPDATE project_resources SET {} WHERE project_id = ${} AND id = ${}",
        updates.join(", "),
        param_idx - 1,
        param_idx
    );
    let tx = state.db.begin().await?;
    let result = tx
        .execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("project resource not found".to_string()));
    }
    let row = find_project_resource_with_conn(&tx, resource_id).await?;
    insert_project_resource_event(
        &tx,
        workspace_id,
        &row,
        "project_resource.updated",
        actor_id,
        json!({
            "resource_id": row.id,
            "project_id": row.project_id,
            "kind": row.kind,
            "name": row.name,
            "sync_status": row.sync_status
        }),
    )
    .await?;
    tx.commit().await?;

    get_project_resource_by_id(&state, resource_id).await
}

pub async fn delete_project_resource(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((project_id, resource_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let workspace_id = find_project_workspace(&state, project_id).await?;
    ensure_workspace_admin(&state, &extensions, workspace_id).await?;
    let actor_id = actor_id_from_extensions(&extensions)?;

    let tx = state.db.begin().await?;
    let row = find_project_resource_with_conn(&tx, resource_id).await?;
    let result = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM project_resources WHERE project_id = $1 AND id = $2",
            vec![project_id.into(), resource_id.into()],
        ))
        .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("project resource not found".to_string()));
    }
    insert_project_resource_event(
        &tx,
        workspace_id,
        &row,
        "project_resource.deleted",
        actor_id,
        json!({
            "resource_id": row.id,
            "project_id": row.project_id,
            "kind": row.kind,
            "name": row.name,
            "sync_status": row.sync_status
        }),
    )
    .await?;
    tx.commit().await?;

    Ok(ApiResponse::ok())
}

async fn get_project_resource_by_id(
    state: &AppState,
    resource_id: Uuid,
) -> Result<Json<ApiResponse<ProjectResourceResponse>>, ApiError> {
    let item = find_project_resource_with_conn(&state.db, resource_id).await?;

    Ok(ApiResponse::success(item))
}

async fn find_project_resource_with_conn<C>(db: &C, resource_id: Uuid) -> Result<ProjectResourceResponse, ApiError>
where
    C: ConnectionTrait,
{
    ProjectResourceResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, project_id, kind, name, locator, permission_policy, sync_status,
                   created_by, created_at, updated_at
            FROM project_resources
            WHERE id = $1
        ",
        vec![resource_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project resource not found".to_string()))
}

async fn get_project_type_by_key(
    state: &AppState,
    key: &str,
) -> Result<Json<ApiResponse<ProjectTypeResponse>>, ApiError> {
    let item = find_project_type_with_conn(&state.db, key).await?;

    Ok(ApiResponse::success(item))
}

async fn find_project_type_with_conn<C>(db: &C, key: &str) -> Result<ProjectTypeResponse, ApiError>
where
    C: ConnectionTrait,
{
    ProjectTypeResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT key, workspace_id, name, description, domain, default_workflow_id,
                   default_governance_policy_id, enabled_capabilities, field_schema,
                   artifact_schema, default_connectors, created_at, updated_at
            FROM project_types
            WHERE key = $1
            LIMIT 1
        ",
        vec![key.to_string().into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project type not found".to_string()))
}

async fn insert_project_type_event<C>(
    db: &C,
    project_type: &ProjectTypeResponse,
    event_type: &str,
    actor_id: Option<Uuid>,
    payload: JsonValue,
) -> Result<(), ApiError>
where
    C: ConnectionTrait,
{
    insert_business_event(
        db,
        BusinessEventInput {
            workspace_id: project_type
                .workspace_id
                .ok_or_else(|| ApiError::Forbidden("system project types are read-only".to_string()))?,
            project_id: None,
            event_type: event_type.to_string(),
            aggregate_type: "project_type".to_string(),
            aggregate_id: project_type.key.clone(),
            actor_id,
            source: actor_source(actor_id),
            payload,
            metadata: json!({
                "project_type_key": project_type.key,
                "workspace_id": project_type.workspace_id
            }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    Ok(())
}

async fn insert_project_resource_event<C>(
    db: &C,
    workspace_id: Uuid,
    resource: &ProjectResourceResponse,
    event_type: &str,
    actor_id: Option<Uuid>,
    payload: JsonValue,
) -> Result<(), ApiError>
where
    C: ConnectionTrait,
{
    insert_business_event(
        db,
        BusinessEventInput {
            workspace_id,
            project_id: Some(resource.project_id),
            event_type: event_type.to_string(),
            aggregate_type: "project_resource".to_string(),
            aggregate_id: resource.id.to_string(),
            actor_id,
            source: actor_source(actor_id),
            payload,
            metadata: json!({
                "resource_id": resource.id,
                "resource_kind": resource.kind,
                "project_id": resource.project_id
            }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    Ok(())
}

fn actor_source(actor_id: Option<Uuid>) -> JsonValue {
    json!({ "type": if actor_id.is_some() { "user" } else { "bot" }, "actor_id": actor_id })
}

fn claims_from_extensions(extensions: &axum::http::Extensions) -> Result<JwtClaims, ApiError> {
    extensions
        .get::<JwtClaims>()
        .cloned()
        .ok_or_else(|| ApiError::Unauthorized("missing auth context".to_string()))
}

fn actor_id_from_extensions(extensions: &axum::http::Extensions) -> Result<Option<Uuid>, ApiError> {
    if extensions.get::<BotAuthContext>().is_some() {
        return Ok(None);
    }
    let claims = claims_from_extensions(extensions)?;
    Uuid::parse_str(&claims.sub)
        .map(Some)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))
}

#[cfg(test)]
mod tests {
    use super::{ensure_json_array, ensure_json_object, normalize_key, normalize_kind};
    use serde_json::json;

    #[test]
    fn normalize_key_accepts_scenario_type_keys() {
        assert_eq!(normalize_key("Code_Project").unwrap(), "code_project");
        assert_eq!(normalize_key("contract_review").unwrap(), "contract_review");
        assert_eq!(normalize_key("quality2").unwrap(), "quality2");
    }

    #[test]
    fn normalize_key_rejects_unsafe_values() {
        assert!(normalize_key("").is_err());
        assert!(normalize_key("code-project").is_err());
        assert!(normalize_key("code project").is_err());
        assert!(normalize_key("code/project").is_err());
    }

    #[test]
    fn normalize_kind_matches_phase_one_resource_kinds() {
        for kind in [
            "repo",
            "directory",
            "document_library",
            "crm_account",
            "erp_order",
            "equipment",
            "site",
            "custom",
        ] {
            assert_eq!(normalize_kind(kind).unwrap(), kind);
        }
        assert!(normalize_kind("mysql").is_err());
    }

    #[test]
    fn json_shape_guards_protect_project_type_schema_fields() {
        assert!(ensure_json_array(json!(["mcp"]), "enabled_capabilities").is_ok());
        assert!(ensure_json_array(json!({}), "enabled_capabilities").is_err());
        assert!(ensure_json_object(json!({"fields": []}), "field_schema").is_ok());
        assert!(ensure_json_object(json!([]), "field_schema").is_err());
    }
}
