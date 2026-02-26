use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    error::ApiError,
    response::{ApiResponse, PaginatedData},
    services::ai_task_service::{CreateAiTaskInput, create_ai_task},
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks},
};

#[derive(Debug, Serialize)]
pub struct IssueResponse {
    pub id: Uuid,
    pub project_id: Uuid,
    pub sprint_id: Option<Uuid>,
    pub title: String,
    pub description: String,
    pub state: String,
    pub priority: String,
    pub assignee_id: Option<Uuid>,
    pub due_at: Option<String>,
    pub created_by: Option<Uuid>,
    pub created_at: String,
    pub updated_at: String,
    pub proposal_id: Option<String>,
    pub governance_exempt: bool,
    pub governance_exempt_reason: Option<String>,
    pub labels: Option<Vec<IssueLabelSummary>>,
}

#[derive(Debug, Serialize, Clone)]
pub struct IssueLabelSummary {
    pub id: Uuid,
    pub name: String,
    pub color: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateIssueRequest {
    pub title: String,
    pub description: Option<String>,
    pub state: Option<String>,
    pub priority: Option<String>,
    pub assignee_id: Option<Uuid>,
    pub due_at: Option<String>,
    pub sprint_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateIssueRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub state: Option<String>,
    pub priority: Option<String>,
    pub assignee_id: Option<Uuid>,
    pub due_at: Option<String>,
    pub sprint_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct ListIssuesQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub state: Option<String>,
    pub assignee_id: Option<Uuid>,
    pub priority: Option<String>,
    pub search: Option<String>,
    pub label_ids: Option<String>,
    pub sort_by: Option<String>,
    pub sort_order: Option<String>,
}

/// Validate state field
fn validate_state(state: &str) -> Result<(), ApiError> {
    match state {
        "backlog" | "todo" | "in_progress" | "done" => Ok(()),
        _ => Err(ApiError::BadRequest(
            "state must be one of: backlog, todo, in_progress, done".to_string(),
        )),
    }
}

/// Validate priority field
fn validate_priority(priority: &str) -> Result<(), ApiError> {
    match priority {
        "low" | "medium" | "high" | "urgent" => Ok(()),
        _ => Err(ApiError::BadRequest(
            "priority must be one of: low, medium, high, urgent".to_string(),
        )),
    }
}

/// POST /api/v1/projects/:project_id/issues - Create a new issue
pub async fn create_issue(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<CreateIssueRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if req.title.trim().is_empty() {
        return Err(ApiError::BadRequest("title is required".to_string()));
    }

    // Validate state and priority
    let issue_state = req.state.unwrap_or_else(|| "todo".to_string());
    validate_state(&issue_state)?;

    let issue_priority = req.priority.unwrap_or_else(|| "medium".to_string());
    validate_priority(&issue_priority)?;

    // Check project exists and user has access (via workspace membership)
    #[derive(Debug, FromQueryResult)]
    struct ProjectWorkspace {
        workspace_id: Uuid,
    }

    let project = ProjectWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT p.workspace_id
            FROM projects p
            INNER JOIN workspace_members wm ON p.workspace_id = wm.workspace_id
            WHERE p.id = $1 AND wm.user_id = $2
        "#,
        vec![project_id.into(), user_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found or access denied".to_string()))?;

    // If assignee specified, verify they're a workspace member
    if let Some(assignee_id) = req.assignee_id {
        let member_exists = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
                vec![project.workspace_id.into(), assignee_id.into()],
            ))
            .await?;

        if member_exists.is_none() {
            return Err(ApiError::BadRequest(
                "assignee must be a workspace member".to_string(),
            ));
        }
    }

    let issue_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    let description = req.description.unwrap_or_default();

    // Parse due_at if provided
    let due_at: Option<chrono::DateTime<chrono::Utc>> = if let Some(due_str) = req.due_at {
        Some(
            chrono::DateTime::parse_from_rfc3339(&due_str)
                .map_err(|_| ApiError::BadRequest("invalid due_at format".to_string()))?
                .with_timezone(&chrono::Utc),
        )
    } else {
        None
    };

    let mut values: Vec<sea_orm::Value> = vec![
        issue_id.into(),
        project_id.into(),
        req.title.clone().into(),
        description.clone().into(),
        issue_state.clone().into(),
        issue_priority.clone().into(),
    ];

    let assignee_param = if let Some(aid) = req.assignee_id {
        values.push(aid.into());
        "$7"
    } else {
        values.push(sea_orm::Value::from(None::<Uuid>));
        "$7"
    };

    let due_param = if let Some(dt) = due_at {
        values.push(dt.into());
        "$8"
    } else {
        values.push(sea_orm::Value::from(None::<chrono::DateTime<chrono::Utc>>));
        "$8"
    };

    values.push(user_id.into());
    values.push(now.into());
    values.push(now.into());
    values.push(req.sprint_id.map(sea_orm::Value::from).unwrap_or(sea_orm::Value::from(None::<Uuid>)));

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            &format!(
                "INSERT INTO work_items (id, project_id, title, description, state, priority, assignee_id, due_at, created_by, created_at, updated_at, sprint_id) VALUES ($1, $2, $3, $4, $5, $6, {}, {}, $9, $10, $11, $12)",
                assignee_param, due_param
            ),
            values,
        ))
        .await?;

    let created_assignee_ids = req.assignee_id.map(|v| vec![v]).unwrap_or_default();
    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::IssueCreated,
            workspace_id: project.workspace_id,
            project_id,
            actor_id: user_id,
            issue_id: Some(issue_id),
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: None,
        },
    );

    if let Some(assignee_id) = req.assignee_id {
        let is_bot_assignee = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT 1 FROM users WHERE id = $1 AND entity_type = 'bot'",
                vec![assignee_id.into()],
            ))
            .await?
            .is_some();

        if is_bot_assignee {
            let _ = create_ai_task(
                &state.db,
                CreateAiTaskInput {
                    project_id,
                    ai_participant_id: assignee_id,
                    task_type: "issue_assigned".to_string(),
                    reference_type: Some("work_item".to_string()),
                    reference_id: Some(issue_id),
                    priority: 10,
                    payload: json!({
                        "issue_id": issue_id.to_string(),
                        "project_id": project_id.to_string(),
                        "assignee_id": assignee_id.to_string(),
                        "trigger": "issue.create",
                    }),
                    idempotency_key: Some(format!("issue_assigned:{issue_id}:{assignee_id}")),
                    max_attempts: 3,
                },
            )
            .await?;

            trigger_webhooks(
                state.clone(),
                TriggerContext {
                    event: WebhookEvent::IssueAssigned,
                    workspace_id: project.workspace_id,
                    project_id,
                    actor_id: user_id,
                    issue_id: Some(issue_id),
                    comment_id: None,
                    label_id: None,
                    sprint_id: None,
                    changes: Some(json!({
                        "assignee_ids": {
                            "old": [],
                            "new": created_assignee_ids.iter().map(Uuid::to_string).collect::<Vec<_>>(),
                            "added": created_assignee_ids.iter().map(Uuid::to_string).collect::<Vec<_>>(),
                            "removed": []
                        }
                    })),
                    mentions: Vec::new(),
                    extra_data: None,
                },
            );
        }
    }

    Ok(ApiResponse::success(IssueResponse {
        id: issue_id,
        project_id,
        sprint_id: req.sprint_id,
        title: req.title,
        description,
        state: issue_state,
        priority: issue_priority,
        assignee_id: req.assignee_id,
        due_at: due_at.map(|d| d.to_rfc3339()),
        created_by: Some(user_id),
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        proposal_id: None,
        governance_exempt: false,
        governance_exempt_reason: None,
        labels: None,
    }))
}

/// GET /api/v1/projects/:project_id/issues - List issues in project
pub async fn list_issues(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<ListIssuesQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Check project access
    let project_exists = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT 1
                FROM projects p
                INNER JOIN workspace_members wm ON p.workspace_id = wm.workspace_id
                WHERE p.id = $1 AND wm.user_id = $2
            "#,
            vec![project_id.into(), user_id.into()],
        ))
        .await?;

    if project_exists.is_none() {
        return Err(ApiError::NotFound(
            "project not found or access denied".to_string(),
        ));
    }

    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(15).clamp(1, 100);
    let offset = (page - 1) * per_page;

    // Build query with filters
    let mut where_clauses = vec!["project_id = $1".to_string()];
    let mut values: Vec<sea_orm::Value> = vec![project_id.into()];
    let mut param_idx = 2;

    if let Some(state_filter) = query.state {
        validate_state(&state_filter)?;
        where_clauses.push(format!("state = ${}", param_idx));
        values.push(state_filter.into());
        param_idx += 1;
    }

    if let Some(assignee) = query.assignee_id {
        where_clauses.push(format!("assignee_id = ${}", param_idx));
        values.push(assignee.into());
        param_idx += 1;
    }

    if let Some(priority) = query.priority {
        validate_priority(&priority)?;
        where_clauses.push(format!("priority = ${}", param_idx));
        values.push(priority.into());
        param_idx += 1;
    }

    if let Some(search) = query.search {
        let search_text = search.trim();
        if !search_text.is_empty() {
            where_clauses.push(format!(
                "(title ILIKE ${0} OR description ILIKE ${0} OR CAST(id AS TEXT) ILIKE ${0})",
                param_idx
            ));
            values.push(format!("%{}%", search_text).into());
            param_idx += 1;
        }
    }

    if let Some(label_ids_raw) = query.label_ids {
        let mut label_ids: Vec<Uuid> = Vec::new();
        for raw in label_ids_raw
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let label_id = Uuid::parse_str(raw)
                .map_err(|_| ApiError::BadRequest(format!("invalid label id: {}", raw)))?;
            if !label_ids.contains(&label_id) {
                label_ids.push(label_id);
            }
        }

        if !label_ids.is_empty() {
            let start_idx = param_idx;
            let placeholders = (start_idx..start_idx + label_ids.len())
                .map(|idx| format!("${idx}"))
                .collect::<Vec<_>>()
                .join(", ");
            where_clauses.push(format!(
                "id IN (
                    SELECT wil.work_item_id
                    FROM work_item_labels wil
                    WHERE wil.label_id IN ({})
                    GROUP BY wil.work_item_id
                    HAVING COUNT(DISTINCT wil.label_id) = {}
                )",
                placeholders,
                label_ids.len()
            ));
            for label_id in label_ids {
                values.push(label_id.into());
                param_idx += 1;
            }
        }
    }

    let sort_by = match query.sort_by.as_deref() {
        Some("created_at") => "created_at",
        Some("priority") => "priority",
        Some("title") => "title",
        _ => "updated_at",
    };
    let sort_order = match query.sort_order.as_deref() {
        Some("asc") => "ASC",
        _ => "DESC",
    };
    let order_by = if sort_by == "priority" {
        format!(
            "CASE priority WHEN 'low' THEN 1 WHEN 'medium' THEN 2 WHEN 'high' THEN 3 WHEN 'urgent' THEN 4 ELSE 5 END {}",
            sort_order
        )
    } else {
        format!("{} {}", sort_by, sort_order)
    };
    let where_sql = where_clauses.join(" AND ");

    // Total count
    let count_result = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            &format!("SELECT COUNT(*) AS count FROM work_items WHERE {}", where_sql),
            values.clone(),
        ))
        .await?
        .ok_or_else(|| ApiError::Internal)?;

    let total: i64 = count_result.try_get("", "count")?;

    let sql = format!(
        "SELECT id, project_id, sprint_id, title, description, state, priority, assignee_id, due_at, created_by, created_at, updated_at, proposal_id, governance_exempt, governance_exempt_reason FROM work_items WHERE {} ORDER BY {} LIMIT ${} OFFSET ${}",
        where_sql,
        order_by,
        param_idx,
        param_idx + 1
    );
    values.push(per_page.into());
    values.push(offset.into());

    #[derive(Debug, FromQueryResult)]
    struct IssueRow {
        id: Uuid,
        project_id: Uuid,
        sprint_id: Option<Uuid>,
        title: String,
        description: String,
        state: String,
        priority: String,
        assignee_id: Option<Uuid>,
        due_at: Option<chrono::DateTime<chrono::Utc>>,
        created_by: Option<Uuid>,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
        proposal_id: Option<String>,
        governance_exempt: bool,
        governance_exempt_reason: Option<String>,
    }

    let issues = IssueRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &sql,
        values,
    ))
    .all(&state.db)
    .await?;

    #[derive(Debug, FromQueryResult)]
    struct IssueLabelRow {
        work_item_id: Uuid,
        label_id: Uuid,
        label_name: String,
        label_color: String,
    }

    let issue_ids: Vec<Uuid> = issues.iter().map(|issue| issue.id).collect();
    let mut label_map: HashMap<Uuid, Vec<IssueLabelSummary>> = HashMap::new();

    if !issue_ids.is_empty() {
        let placeholders = (1..=issue_ids.len())
            .map(|idx| format!("${idx}"))
            .collect::<Vec<_>>()
            .join(", ");
        let label_sql = format!(
            r#"
                SELECT wil.work_item_id, l.id AS label_id, l.name AS label_name, l.color AS label_color
                FROM work_item_labels wil
                INNER JOIN labels l ON wil.label_id = l.id
                WHERE wil.work_item_id IN ({})
                ORDER BY l.name ASC
            "#,
            placeholders
        );
        let label_values: Vec<sea_orm::Value> = issue_ids.into_iter().map(Into::into).collect();

        let issue_labels = IssueLabelRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            &label_sql,
            label_values,
        ))
        .all(&state.db)
        .await?;

        for issue_label in issue_labels {
            label_map
                .entry(issue_label.work_item_id)
                .or_default()
                .push(IssueLabelSummary {
                    id: issue_label.label_id,
                    name: issue_label.label_name,
                    color: issue_label.label_color,
                });
        }
    }

    let response: Vec<IssueResponse> = issues
        .into_iter()
        .map(|i| IssueResponse {
            id: i.id,
            project_id: i.project_id,
            sprint_id: i.sprint_id,
            title: i.title,
            description: i.description,
            state: i.state,
            priority: i.priority,
            assignee_id: i.assignee_id,
            due_at: i.due_at.map(|d| d.to_rfc3339()),
            created_by: i.created_by,
            created_at: i.created_at.to_rfc3339(),
            updated_at: i.updated_at.to_rfc3339(),
            proposal_id: i.proposal_id,
            governance_exempt: i.governance_exempt,
            governance_exempt_reason: i.governance_exempt_reason,
            labels: label_map.remove(&i.id),
        })
        .collect();

    let total_pages = if total == 0 {
        0
    } else {
        ((total as f64) / (per_page as f64)).ceil() as i64
    };

    Ok(ApiResponse::success(PaginatedData {
        items: response,
        total,
        page,
        per_page,
        total_pages,
    }))
}

/// GET /api/v1/issues/:id - Get issue details
pub async fn get_issue(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(issue_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    #[derive(Debug, FromQueryResult)]
    struct IssueRow {
        id: Uuid,
        project_id: Uuid,
        sprint_id: Option<Uuid>,
        title: String,
        description: String,
        state: String,
        priority: String,
        assignee_id: Option<Uuid>,
        due_at: Option<chrono::DateTime<chrono::Utc>>,
        created_by: Option<Uuid>,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
        proposal_id: Option<String>,
        governance_exempt: bool,
        governance_exempt_reason: Option<String>,
    }

    let issue = IssueRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT wi.id, wi.project_id, wi.sprint_id, wi.title, wi.description, wi.state, wi.priority,
                   wi.assignee_id, wi.due_at, wi.created_by, wi.created_at, wi.updated_at,
                   wi.proposal_id, wi.governance_exempt, wi.governance_exempt_reason
            FROM work_items wi
            INNER JOIN projects p ON wi.project_id = p.id
            INNER JOIN workspace_members wm ON p.workspace_id = wm.workspace_id
            WHERE wi.id = $1 AND wm.user_id = $2
        "#,
        vec![issue_id.into(), user_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("issue not found or access denied".to_string()))?;

    Ok(ApiResponse::success(IssueResponse {
        id: issue.id,
        project_id: issue.project_id,
        sprint_id: issue.sprint_id,
        title: issue.title,
        description: issue.description,
        state: issue.state,
        priority: issue.priority,
        assignee_id: issue.assignee_id,
        due_at: issue.due_at.map(|d| d.to_rfc3339()),
        created_by: issue.created_by,
        created_at: issue.created_at.to_rfc3339(),
        updated_at: issue.updated_at.to_rfc3339(),
        proposal_id: issue.proposal_id,
        governance_exempt: issue.governance_exempt,
        governance_exempt_reason: issue.governance_exempt_reason,
        labels: None,
    }))
}

/// PUT /api/v1/issues/:id - Update issue
pub async fn update_issue(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(issue_id): Path<Uuid>,
    Json(req): Json<UpdateIssueRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Get current issue values and verify access.
    #[derive(Debug, FromQueryResult)]
    struct CurrentIssue {
        project_id: Uuid,
        workspace_id: Uuid,
        title: String,
        description: String,
        state: String,
        priority: String,
        assignee_id: Option<Uuid>,
        sprint_id: Option<Uuid>,
    }

    let current_issue = CurrentIssue::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT wi.project_id, p.workspace_id, wi.title, wi.description, wi.state, wi.priority, wi.assignee_id, wi.sprint_id
            FROM work_items wi
            INNER JOIN projects p ON wi.project_id = p.id
            INNER JOIN workspace_members wm ON p.workspace_id = wm.workspace_id
            WHERE wi.id = $1 AND wm.user_id = $2
        "#,
        vec![issue_id.into(), user_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("issue not found or access denied".to_string()))?;

    // Validate state and priority if provided
    if let Some(ref state_val) = req.state {
        validate_state(state_val)?;
    }

    if let Some(ref priority) = req.priority {
        validate_priority(priority)?;
    }

    // Verify assignee is workspace member if provided
    if let Some(assignee_id) = req.assignee_id {
        let member_exists = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
                vec![current_issue.workspace_id.into(), assignee_id.into()],
            ))
            .await?;

        if member_exists.is_none() {
            return Err(ApiError::BadRequest(
                "assignee must be a workspace member".to_string(),
            ));
        }
    }

    // Build update query
    let requested_state = req.state.clone();
    let requested_priority = req.priority.clone();
    let requested_assignee = req.assignee_id;
    let requested_sprint = req.sprint_id;

    let mut updates = Vec::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut param_idx = 1;

    if let Some(title) = req.title.clone() {
        if title.trim().is_empty() {
            return Err(ApiError::BadRequest("title cannot be empty".to_string()));
        }
        updates.push(format!("title = ${}", param_idx));
        values.push(title.into());
        param_idx += 1;
    }

    if let Some(description) = req.description.clone() {
        updates.push(format!("description = ${}", param_idx));
        values.push(description.into());
        param_idx += 1;
    }

    if let Some(state_val) = req.state.clone() {
        updates.push(format!("state = ${}", param_idx));
        values.push(state_val.into());
        param_idx += 1;
    }

    if let Some(priority) = req.priority.clone() {
        updates.push(format!("priority = ${}", param_idx));
        values.push(priority.into());
        param_idx += 1;
    }

    if let Some(assignee) = req.assignee_id {
        updates.push(format!("assignee_id = ${}", param_idx));
        values.push(assignee.into());
        param_idx += 1;
    }

    if let Some(due_str) = req.due_at.clone() {
        let due_at = chrono::DateTime::parse_from_rfc3339(&due_str)
            .map_err(|_| ApiError::BadRequest("invalid due_at format".to_string()))?
            .with_timezone(&chrono::Utc);
        updates.push(format!("due_at = ${}", param_idx));
        values.push(due_at.into());
        param_idx += 1;
    }

    if let Some(sprint_id) = req.sprint_id {
        updates.push(format!("sprint_id = ${}", param_idx));
        values.push(sprint_id.into());
        param_idx += 1;
    }

    if updates.is_empty() {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }

    let now = chrono::Utc::now();
    updates.push(format!("updated_at = ${}", param_idx));
    values.push(now.clone().into());
    param_idx += 1;

    values.push(issue_id.into());

    let query = format!(
        "UPDATE work_items SET {} WHERE id = ${}",
        updates.join(", "),
        param_idx
    );

    let tx = state.db.begin().await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &query,
        values,
    ))
    .await?;

    // Fetch updated issue
    #[derive(Debug, FromQueryResult)]
    struct IssueRow {
        id: Uuid,
        project_id: Uuid,
        sprint_id: Option<Uuid>,
        title: String,
        description: String,
        state: String,
        priority: String,
        assignee_id: Option<Uuid>,
        due_at: Option<chrono::DateTime<chrono::Utc>>,
        created_by: Option<Uuid>,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
        proposal_id: Option<String>,
        governance_exempt: bool,
        governance_exempt_reason: Option<String>,
    }

    let updated = IssueRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, project_id, sprint_id, title, description, state, priority, assignee_id, due_at, created_by, created_at, updated_at, proposal_id, governance_exempt, governance_exempt_reason FROM work_items WHERE id = $1",
        vec![issue_id.into()],
    ))
    .one(&tx)
    .await?
    .ok_or_else(|| ApiError::Internal)?;

    let assignee_changed = requested_assignee
        .map(|new_assignee| Some(new_assignee) != current_issue.assignee_id)
        .unwrap_or(false);
    let state_changed = requested_state
        .as_ref()
        .map(|new_state| *new_state != current_issue.state)
        .unwrap_or(false);

    let mut updated_fields = Map::<String, Value>::new();
    if let Some(new_title) = req.title.as_ref() {
        if new_title != &current_issue.title {
            updated_fields.insert(
                "title".to_string(),
                json!({
                    "old": current_issue.title.clone(),
                    "new": updated.title.clone(),
                }),
            );
        }
    }
    if let Some(new_description) = req.description.as_ref() {
        if new_description != &current_issue.description {
            updated_fields.insert(
                "description".to_string(),
                json!({
                    "old": current_issue.description.clone(),
                    "new": updated.description.clone(),
                }),
            );
        }
    }
    if let Some(new_priority) = req.priority.as_ref() {
        if new_priority != &current_issue.priority {
            updated_fields.insert(
                "priority".to_string(),
                json!({
                    "old": current_issue.priority.clone(),
                    "new": updated.priority.clone(),
                }),
            );
        }
    }

    struct ActivityChange {
        action: &'static str,
        detail: serde_json::Value,
    }

    let mut changes: Vec<ActivityChange> = Vec::new();

    if let Some(new_assignee) = requested_assignee {
        if Some(new_assignee) != current_issue.assignee_id {
            changes.push(ActivityChange {
                action: "assigned",
                detail: serde_json::json!({
                    "from": current_issue.assignee_id.map(|id| id.to_string()),
                    "to": new_assignee.to_string()
                }),
            });
        }
    }

    if let Some(new_state) = requested_state {
        if new_state != current_issue.state {
            changes.push(ActivityChange {
                action: "status_changed",
                detail: serde_json::json!({
                    "from": current_issue.state.clone(),
                    "to": new_state
                }),
            });
        }
    }

    if let Some(new_priority) = requested_priority {
        if new_priority != current_issue.priority {
            changes.push(ActivityChange {
                action: "priority_changed",
                detail: serde_json::json!({
                    "from": current_issue.priority.clone(),
                    "to": new_priority
                }),
            });
        }
    }

    if let Some(new_sprint_id) = requested_sprint {
        if Some(new_sprint_id) != current_issue.sprint_id {
            changes.push(ActivityChange {
                action: "sprint_changed",
                detail: serde_json::json!({
                    "from": current_issue.sprint_id.map(|id| id.to_string()),
                    "to": new_sprint_id.to_string()
                }),
            });
        }
    }

    for change in &changes {
        let detail_text = change.detail.to_string();

        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO activities (
                    id, workspace_id, project_id, issue_id, user_id, action, detail, created_at,
                    resource_type, resource_id, event_type, actor_id, payload
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb, $8, $9, $10, $11, $12, $13::jsonb)
            "#,
            vec![
                Uuid::new_v4().into(),
                current_issue.workspace_id.into(),
                current_issue.project_id.into(),
                issue_id.into(),
                user_id.into(),
                change.action.into(),
                detail_text.clone().into(),
                now.clone().into(),
                "issue".into(),
                issue_id.into(),
                change.action.into(),
                user_id.into(),
                detail_text.into(),
            ],
        ))
        .await?;
    }

    if let Some(new_assignee) = requested_assignee {
        if Some(new_assignee) != current_issue.assignee_id {
            let link = format!(
                "/workspace/{}/projects/{}/issues/{}",
                current_issue.workspace_id, current_issue.project_id, issue_id
            );

            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                    INSERT INTO notifications (
                        id, user_id, workspace_id, type, kind, payload, title, content, link, related_issue_id, is_read, created_at
                    )
                    VALUES ($1, $2, $3, $4, $5, $6::jsonb, $7, $8, $9, $10, false, $11)
                "#,
                vec![
                    Uuid::new_v4().into(),
                    new_assignee.into(),
                    current_issue.workspace_id.into(),
                    "assignment".into(),
                    "assignment".into(),
                    json!({}).into(),
                    "notification.issueAssignedTitle".into(),
                    format!("issue_assigned:{}", updated.title).into(),
                    link.into(),
                    issue_id.into(),
                    now.clone().into(),
                ],
            ))
            .await?;
        }
    }

    tx.commit().await?;

    let to_single_or_empty = |value: Option<Uuid>| {
        value
            .map(|id| vec![id.to_string()])
            .unwrap_or_else(Vec::new)
    };

    if assignee_changed {
        if let Some(new_assignee) = updated.assignee_id {
            let is_bot_assignee = state
                .db
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT 1 FROM users WHERE id = $1 AND entity_type = 'bot'",
                    vec![new_assignee.into()],
                ))
                .await?
                .is_some();

            if is_bot_assignee {
                let _ = create_ai_task(
                    &state.db,
                    CreateAiTaskInput {
                        project_id: current_issue.project_id,
                        ai_participant_id: new_assignee,
                        task_type: "issue_assigned".to_string(),
                        reference_type: Some("work_item".to_string()),
                        reference_id: Some(issue_id),
                        priority: 10,
                        payload: json!({
                            "issue_id": issue_id.to_string(),
                            "project_id": current_issue.project_id.to_string(),
                            "assignee_id": new_assignee.to_string(),
                            "trigger": "issue.update_assignee",
                        }),
                        idempotency_key: Some(format!("issue_assigned:{issue_id}:{new_assignee}")),
                        max_attempts: 3,
                    },
                )
                .await?;
            }
        }

        trigger_webhooks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::IssueAssigned,
                workspace_id: current_issue.workspace_id,
                project_id: current_issue.project_id,
                actor_id: user_id,
                issue_id: Some(issue_id),
                comment_id: None,
                label_id: None,
                sprint_id: None,
                changes: Some(json!({
                    "assignee_ids": {
                        "old": to_single_or_empty(current_issue.assignee_id),
                        "new": to_single_or_empty(updated.assignee_id),
                        "added": to_single_or_empty(updated.assignee_id.filter(|id| Some(*id) != current_issue.assignee_id)),
                        "removed": to_single_or_empty(current_issue.assignee_id.filter(|id| Some(*id) != updated.assignee_id)),
                    }
                })),
                mentions: Vec::new(),
                extra_data: None,
            },
        );
    }

    if state_changed {
        trigger_webhooks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::IssueStateChanged,
                workspace_id: current_issue.workspace_id,
                project_id: current_issue.project_id,
                actor_id: user_id,
                issue_id: Some(issue_id),
                comment_id: None,
                label_id: None,
                sprint_id: None,
                changes: Some(json!({
                    "state": {
                        "old": current_issue.state.clone(),
                        "new": updated.state.clone(),
                    }
                })),
                mentions: Vec::new(),
                extra_data: None,
            },
        );
    }

    if !updated_fields.is_empty() {
        trigger_webhooks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::IssueUpdated,
                workspace_id: current_issue.workspace_id,
                project_id: current_issue.project_id,
                actor_id: user_id,
                issue_id: Some(issue_id),
                comment_id: None,
                label_id: None,
                sprint_id: None,
                changes: Some(json!(updated_fields)),
                mentions: Vec::new(),
                extra_data: None,
            },
        );
    }

    Ok(ApiResponse::success(IssueResponse {
        id: updated.id,
        project_id: updated.project_id,
        sprint_id: updated.sprint_id,
        title: updated.title,
        description: updated.description,
        state: updated.state,
        priority: updated.priority,
        assignee_id: updated.assignee_id,
        due_at: updated.due_at.map(|d| d.to_rfc3339()),
        created_by: updated.created_by,
        created_at: updated.created_at.to_rfc3339(),
        updated_at: updated.updated_at.to_rfc3339(),
        proposal_id: updated.proposal_id,
        governance_exempt: updated.governance_exempt,
        governance_exempt_reason: updated.governance_exempt_reason,
        labels: None,
    }))
}

/// DELETE /api/v1/issues/:id - Delete issue
pub async fn delete_issue(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(issue_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Get issue context and check permission
    #[derive(Debug, FromQueryResult)]
    struct IssueWorkspace {
        project_id: Uuid,
        workspace_id: Uuid,
        project_key: String,
        title: String,
        description: String,
        state: String,
        priority: String,
        assignee_id: Option<Uuid>,
        sprint_id: Option<Uuid>,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    let issue_ws = IssueWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT wi.project_id, p.workspace_id, p.key AS project_key,
                   wi.title, wi.description, wi.state, wi.priority, wi.assignee_id, wi.sprint_id,
                   wi.created_at, wi.updated_at
            FROM work_items wi
            INNER JOIN projects p ON wi.project_id = p.id
            WHERE wi.id = $1
        "#,
        vec![issue_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("issue not found".to_string()))?;

    // Check user role
    #[derive(Debug, FromQueryResult)]
    struct RoleRow {
        role: String,
    }

    let role = RoleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        vec![issue_ws.workspace_id.into(), user_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::Forbidden("access denied".to_string()))?;

    if role.role != "owner" && role.role != "admin" {
        return Err(ApiError::Forbidden(
            "only owners and admins can delete issues".to_string(),
        ));
    }

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM work_items WHERE id = $1",
            vec![issue_id.into()],
        ))
        .await?;

    let deleted_issue_payload = json!({
        "id": issue_id.to_string(),
        "key": format!(
            "{}-{}",
            issue_ws.project_key,
            issue_id.simple().to_string().to_uppercase()[..8].to_string()
        ),
        "title": issue_ws.title,
        "description": issue_ws.description,
        "state": issue_ws.state,
        "priority": issue_ws.priority,
        "assignee_ids": issue_ws
            .assignee_id
            .map(|id| vec![id.to_string()])
            .unwrap_or_else(Vec::new),
        "label_ids": Vec::<String>::new(),
        "sprint_id": issue_ws.sprint_id.map(|id| id.to_string()),
        "created_at": issue_ws.created_at.to_rfc3339(),
        "updated_at": issue_ws.updated_at.to_rfc3339(),
    });

    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::IssueDeleted,
            workspace_id: issue_ws.workspace_id,
            project_id: issue_ws.project_id,
            actor_id: user_id,
            issue_id: Some(issue_id),
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: Some(deleted_issue_payload),
        },
    );

    Ok(ApiResponse::ok())
}
