use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    response::{ApiResponse, PaginatedData},
    services::{
        governance_audit_service::{GovernanceAuditLogInput, write_governance_audit_log},
        trust_score_service::{is_project_admin_or_owner, is_project_member, is_system_admin},
    },
};

#[derive(Debug, Deserialize)]
pub struct ListProposalTemplatesQuery {
    pub project_id: Uuid,
    pub template_type: Option<String>,
    pub is_active: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct CreateProposalTemplateRequest {
    pub project_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub template_type: Option<String>,
    pub content: Value,
    pub is_default: Option<bool>,
    pub is_active: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProposalTemplateRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub template_type: Option<String>,
    pub content: Option<Value>,
    pub is_default: Option<bool>,
    pub is_active: Option<bool>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct ProposalTemplateRow {
    pub id: String,
    pub project_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub template_type: String,
    pub content: Value,
    pub is_default: bool,
    pub is_active: bool,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub async fn list_proposal_templates(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Query(query): Query<ListProposalTemplatesQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_claim_user_id(&claims)?;
    ensure_project_member_or_admin(&state, query.project_id, user_id).await?;

    let mut values: Vec<sea_orm::Value> = vec![query.project_id.into()];
    let mut where_parts = vec!["project_id = $1".to_string()];
    let mut idx = 2;

    if let Some(template_type) = query.template_type {
        if template_type.trim().is_empty() {
            return Err(ApiError::BadRequest("template_type cannot be empty".to_string()));
        }
        where_parts.push(format!("template_type = ${idx}"));
        values.push(template_type.trim().to_string().into());
        idx += 1;
    }

    if let Some(is_active) = query.is_active {
        where_parts.push(format!("is_active = ${idx}"));
        values.push(is_active.into());
    }

    let sql = format!(
        r#"
            SELECT id, project_id, name, description, template_type, content,
                   is_default, is_active, created_by, created_at, updated_at
            FROM proposal_templates
            WHERE {}
            ORDER BY is_default DESC, updated_at DESC
        "#,
        where_parts.join(" AND ")
    );

    let items = ProposalTemplateRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .all(&state.db)
    .await?;

    Ok(ApiResponse::success(PaginatedData::from_items(items)))
}

pub async fn get_proposal_template(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_claim_user_id(&claims)?;
    let template = find_template(&state, &id).await?;
    ensure_project_member_or_admin(&state, template.project_id, user_id).await?;
    Ok(ApiResponse::success(template))
}

pub async fn create_proposal_template(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Json(req): Json<CreateProposalTemplateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_claim_user_id(&claims)?;
    ensure_project_admin_or_owner_or_system_admin(&state, req.project_id, user_id).await?;

    let name = req.name.trim();
    if name.len() < 2 || name.len() > 120 {
        return Err(ApiError::BadRequest(
            "name must be between 2 and 120 characters".to_string(),
        ));
    }
    let template_type = req
        .template_type
        .unwrap_or_else(|| "governance".to_string())
        .trim()
        .to_string();
    if template_type.is_empty() || template_type.len() > 30 {
        return Err(ApiError::BadRequest("invalid template_type".to_string()));
    }
    if !req.content.is_object() {
        return Err(ApiError::BadRequest(
            "template content must be a JSON object".to_string(),
        ));
    }

    let template_id = format!("TPL-{}", Uuid::new_v4());
    let now = Utc::now();
    let is_default = req.is_default.unwrap_or(false);
    let is_active = req.is_active.unwrap_or(true);

    let tx = state.db.begin().await?;

    if is_default {
        tx
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE proposal_templates SET is_default = false, updated_at = $2 WHERE project_id = $1",
                vec![req.project_id.into(), now.into()],
            ))
            .await?;
    }

    let insert = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO proposal_templates (
                    id, project_id, name, description, template_type, content,
                    is_default, is_active, created_by, created_at, updated_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            "#,
            vec![
                template_id.clone().into(),
                req.project_id.into(),
                name.to_string().into(),
                req.description.map(|v| v.trim().to_string()).into(),
                template_type.into(),
                req.content.into(),
                is_default.into(),
                is_active.into(),
                Some(user_id).into(),
                now.into(),
                now.into(),
            ],
        ))
        .await;

    if let Err(err) = insert {
        let msg = err.to_string();
        if msg.contains("uq_proposal_templates_project_name") || msg.contains("duplicate key value") {
            return Err(ApiError::Conflict(
                "template name already exists in this project".to_string(),
            ));
        }
        return Err(ApiError::Database(err));
    }

    let created = find_template_by_conn(&tx, &template_id).await?;
    write_governance_audit_log(
        &tx,
        GovernanceAuditLogInput {
            project_id: req.project_id,
            actor_id: Some(user_id),
            action: "proposal_template.created".to_string(),
            resource_type: "proposal_template".to_string(),
            resource_id: Some(template_id),
            old_value: None,
            new_value: Some(serde_json::to_value(&created).map_err(|_| ApiError::Internal)?),
            metadata: Some(json!({
                "source": "api",
            })),
        },
    )
    .await?;
    tx.commit().await?;

    Ok(ApiResponse::success(created))
}

pub async fn update_proposal_template(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<String>,
    Json(req): Json<UpdateProposalTemplateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_claim_user_id(&claims)?;
    let existing = find_template(&state, &id).await?;
    ensure_project_admin_or_owner_or_system_admin(&state, existing.project_id, user_id).await?;

    if req.name.is_none()
        && req.description.is_none()
        && req.template_type.is_none()
        && req.content.is_none()
        && req.is_default.is_none()
        && req.is_active.is_none()
    {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }

    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut set_parts: Vec<String> = Vec::new();
    let mut idx = 1;

    if let Some(name) = req.name {
        let name = name.trim().to_string();
        if name.len() < 2 || name.len() > 120 {
            return Err(ApiError::BadRequest(
                "name must be between 2 and 120 characters".to_string(),
            ));
        }
        set_parts.push(format!("name = ${idx}"));
        values.push(name.into());
        idx += 1;
    }

    if let Some(description) = req.description {
        set_parts.push(format!("description = ${idx}"));
        values.push(Some(description.trim().to_string()).into());
        idx += 1;
    }

    if let Some(template_type) = req.template_type {
        let template_type = template_type.trim().to_string();
        if template_type.is_empty() || template_type.len() > 30 {
            return Err(ApiError::BadRequest("invalid template_type".to_string()));
        }
        set_parts.push(format!("template_type = ${idx}"));
        values.push(template_type.into());
        idx += 1;
    }

    if let Some(content) = req.content {
        if !content.is_object() {
            return Err(ApiError::BadRequest(
                "template content must be a JSON object".to_string(),
            ));
        }
        set_parts.push(format!("content = ${idx}"));
        values.push(content.into());
        idx += 1;
    }

    let tx = state.db.begin().await?;

    if let Some(is_default) = req.is_default {
        if is_default {
            tx
                .execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE proposal_templates SET is_default = false, updated_at = $2 WHERE project_id = $1",
                    vec![existing.project_id.into(), Utc::now().into()],
                ))
                .await?;
        }
        set_parts.push(format!("is_default = ${idx}"));
        values.push(is_default.into());
        idx += 1;
    }

    if let Some(is_active) = req.is_active {
        set_parts.push(format!("is_active = ${idx}"));
        values.push(is_active.into());
        idx += 1;
    }

    set_parts.push(format!("updated_at = ${idx}"));
    values.push(Utc::now().into());
    idx += 1;

    values.push(id.clone().into());
    let sql = format!(
        "UPDATE proposal_templates SET {} WHERE id = ${}",
        set_parts.join(", "),
        idx
    );

    let res = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await;
    if let Err(err) = res {
        let msg = err.to_string();
        if msg.contains("uq_proposal_templates_project_name") || msg.contains("duplicate key value") {
            return Err(ApiError::Conflict(
                "template name already exists in this project".to_string(),
            ));
        }
        return Err(ApiError::Database(err));
    }

    let updated = find_template_by_conn(&tx, &id).await?;
    write_governance_audit_log(
        &tx,
        GovernanceAuditLogInput {
            project_id: existing.project_id,
            actor_id: Some(user_id),
            action: "proposal_template.updated".to_string(),
            resource_type: "proposal_template".to_string(),
            resource_id: Some(id),
            old_value: Some(serde_json::to_value(&existing).map_err(|_| ApiError::Internal)?),
            new_value: Some(serde_json::to_value(&updated).map_err(|_| ApiError::Internal)?),
            metadata: Some(json!({
                "source": "api",
            })),
        },
    )
    .await?;
    tx.commit().await?;

    Ok(ApiResponse::success(updated))
}

pub async fn delete_proposal_template(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_claim_user_id(&claims)?;
    let existing = find_template(&state, &id).await?;
    ensure_project_admin_or_owner_or_system_admin(&state, existing.project_id, user_id).await?;

    let tx = state.db.begin().await?;

    tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM proposal_templates WHERE id = $1",
            vec![id.into()],
        ))
        .await?;
    write_governance_audit_log(
        &tx,
        GovernanceAuditLogInput {
            project_id: existing.project_id,
            actor_id: Some(user_id),
            action: "proposal_template.deleted".to_string(),
            resource_type: "proposal_template".to_string(),
            resource_id: Some(existing.id.clone()),
            old_value: Some(serde_json::to_value(&existing).map_err(|_| ApiError::Internal)?),
            new_value: None,
            metadata: Some(json!({
                "source": "api",
            })),
        },
    )
    .await?;
    tx.commit().await?;

    Ok(ApiResponse::ok())
}

async fn find_template(state: &AppState, id: &str) -> Result<ProposalTemplateRow, ApiError> {
    find_template_by_conn(&state.db, id).await
}

async fn find_template_by_conn<C: ConnectionTrait>(
    db: &C,
    id: &str,
) -> Result<ProposalTemplateRow, ApiError> {
    ProposalTemplateRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, project_id, name, description, template_type, content,
                   is_default, is_active, created_by, created_at, updated_at
            FROM proposal_templates
            WHERE id = $1
        "#,
        vec![id.to_string().into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("proposal template not found".to_string()))
}

fn parse_claim_user_id(claims: &JwtClaims) -> Result<Uuid, ApiError> {
    Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))
}

async fn ensure_project_member_or_admin(
    state: &AppState,
    project_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let allowed = is_project_member(&state.db, project_id, user_id).await?
        || is_system_admin(&state.db, user_id).await?;
    if !allowed {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }
    Ok(())
}

async fn ensure_project_admin_or_owner_or_system_admin(
    state: &AppState,
    project_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let allowed = is_project_admin_or_owner(&state.db, project_id, user_id).await?
        || is_system_admin(&state.db, user_id).await?;
    if !allowed {
        return Err(ApiError::Forbidden(
            "admin or owner required for template management".to_string(),
        ));
    }
    Ok(())
}
