use crate::middleware::bot_auth::{BotAuthContext, require_workspace_access};
use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::ApiError,
    response::{ApiResponse, PaginatedData},
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks},
};

#[derive(Debug, Serialize)]
pub struct LabelResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub color: String,
    pub description: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateLabelRequest {
    pub name: String,
    pub color: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateLabelRequest {
    pub name: Option<String>,
    pub color: Option<String>,
    pub description: Option<String>,
}

/// Collect the auth context into an `Extensions` bag for `require_workspace_access`,
/// which resolves bot-token and JWT callers through the same code path.
fn build_auth_extensions(claims: JwtClaims, bot: Option<Extension<BotAuthContext>>) -> axum::http::Extensions {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }
    extensions
}

/// Guard for mutations of an existing label.
///
/// Labels are workspace-scoped (`migrations/0003_labels.sql` has no `project_id`),
/// so renaming or recoloring one rewrites the taxonomy of every project in the
/// workspace. Only workspace owners/admins may do that.
///
/// The bot branch is explicit on purpose: `require_workspace_access` derives the
/// role of a bot token from its permission bits (`admin` -> admin, otherwise
/// member), so a `read`/`write` bot is rejected here by decision instead of by
/// the accident of a missing `workspace_members` row.
fn ensure_label_mutation_allowed(role: &str, is_bot: bool, action: &str) -> Result<(), ApiError> {
    if role == "owner" || role == "admin" {
        return Ok(());
    }
    if is_bot {
        return Err(ApiError::Forbidden(format!(
            "bot token requires the 'admin' permission to {action} labels"
        )));
    }
    Err(ApiError::Forbidden(format!(
        "only owners and admins can {action} labels"
    )))
}

/// POST /api/v1/workspaces/:workspace_id/labels - Create a label
pub async fn create_label(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CreateLabelRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);

    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name is required".to_string()));
    }

    // Check workspace membership (bot tokens are bound to their own workspace)
    require_workspace_access(&state, &extensions, workspace_id).await?;

    // Check name uniqueness
    let existing = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM labels WHERE workspace_id = $1 AND name = $2",
            vec![workspace_id.into(), req.name.clone().into()],
        ))
        .await?;

    if existing.is_some() {
        return Err(ApiError::Conflict(
            "label with this name already exists in workspace".to_string(),
        ));
    }

    let label_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    let color = req.color.unwrap_or_else(|| "#gray".to_string());
    let description = req.description.unwrap_or_default();

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO labels (id, workspace_id, name, color, description, created_at) VALUES ($1, $2, $3, $4, $5, $6)",
            vec![
                label_id.into(),
                workspace_id.into(),
                req.name.clone().into(),
                color.clone().into(),
                description.clone().into(),
                now.into(),
            ],
        ))
        .await?;

    Ok(ApiResponse::success(LabelResponse {
        id: label_id,
        workspace_id,
        name: req.name,
        color,
        description,
        created_at: now.to_rfc3339(),
    }))
}

/// GET /api/v1/workspaces/:workspace_id/labels - List labels in workspace
pub async fn list_labels(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }

    // Check workspace membership
    require_workspace_access(&state, &extensions, workspace_id).await?;

    #[derive(Debug, FromQueryResult)]
    struct LabelRow {
        id: Uuid,
        workspace_id: Uuid,
        name: String,
        color: String,
        description: String,
        created_at: chrono::DateTime<chrono::Utc>,
    }

    let labels = LabelRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, name, color, description, created_at FROM labels WHERE workspace_id = $1 ORDER BY name ASC",
        vec![workspace_id.into()],
    ))
    .all(&state.db)
    .await?;

    let response: Vec<LabelResponse> = labels
        .into_iter()
        .map(|l| LabelResponse {
            id: l.id,
            workspace_id: l.workspace_id,
            name: l.name,
            color: l.color,
            description: l.description,
            created_at: l.created_at.to_rfc3339(),
        })
        .collect();

    Ok(ApiResponse::success(PaginatedData::from_items(response)))
}

/// PUT /api/v1/labels/:id - Update label
pub async fn update_label(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(label_id): Path<Uuid>,
    Json(req): Json<UpdateLabelRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);

    // Get label's workspace and verify access
    #[derive(Debug, FromQueryResult)]
    struct LabelWorkspace {
        workspace_id: Uuid,
    }

    let label = LabelWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM labels WHERE id = $1",
        vec![label_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("label not found".to_string()))?;

    let (_, role, is_bot) = require_workspace_access(&state, &extensions, label.workspace_id).await?;
    ensure_label_mutation_allowed(&role, is_bot, "update")?;

    // Check name uniqueness if name is being updated
    if let Some(ref name) = req.name {
        if name.trim().is_empty() {
            return Err(ApiError::BadRequest("name cannot be empty".to_string()));
        }

        let existing = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM labels WHERE workspace_id = $1 AND name = $2 AND id != $3",
                vec![label.workspace_id.into(), name.clone().into(), label_id.into()],
            ))
            .await?;

        if existing.is_some() {
            return Err(ApiError::Conflict(
                "label with this name already exists in workspace".to_string(),
            ));
        }
    }

    // Build update query
    let mut updates = Vec::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut param_idx = 1;

    if let Some(name) = req.name {
        updates.push(format!("name = ${}", param_idx));
        values.push(name.into());
        param_idx += 1;
    }

    if let Some(color) = req.color {
        updates.push(format!("color = ${}", param_idx));
        values.push(color.into());
        param_idx += 1;
    }

    if let Some(description) = req.description {
        updates.push(format!("description = ${}", param_idx));
        values.push(description.into());
        param_idx += 1;
    }

    if updates.is_empty() {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }

    values.push(label_id.into());

    let query = format!("UPDATE labels SET {} WHERE id = ${}", updates.join(", "), param_idx);

    state
        .db
        .execute(Statement::from_sql_and_values(DbBackend::Postgres, &query, values))
        .await?;

    // Fetch updated label
    #[derive(Debug, FromQueryResult)]
    struct LabelRow {
        id: Uuid,
        workspace_id: Uuid,
        name: String,
        color: String,
        description: String,
        created_at: chrono::DateTime<chrono::Utc>,
    }

    let updated = LabelRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, name, color, description, created_at FROM labels WHERE id = $1",
        vec![label_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::Internal)?;

    Ok(ApiResponse::success(LabelResponse {
        id: updated.id,
        workspace_id: updated.workspace_id,
        name: updated.name,
        color: updated.color,
        description: updated.description,
        created_at: updated.created_at.to_rfc3339(),
    }))
}

/// DELETE /api/v1/labels/:id - Delete label
pub async fn delete_label(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(label_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);

    // Get label's workspace and check permission
    #[derive(Debug, FromQueryResult)]
    struct LabelWorkspace {
        workspace_id: Uuid,
    }

    let label = LabelWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM labels WHERE id = $1",
        vec![label_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("label not found".to_string()))?;

    let (_, role, is_bot) = require_workspace_access(&state, &extensions, label.workspace_id).await?;
    ensure_label_mutation_allowed(&role, is_bot, "delete")?;

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM labels WHERE id = $1",
            vec![label_id.into()],
        ))
        .await?;

    Ok(ApiResponse::ok())
}

// ============================================================================
// Work Item Label Association APIs
// ============================================================================

/// POST /api/v1/issues/:issue_id/labels/:label_id - Add label to issue
pub async fn add_label_to_issue(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((issue_id, label_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }

    // Verify issue exists and user has access
    #[derive(Debug, FromQueryResult)]
    struct IssueWorkspace {
        project_id: Uuid,
        workspace_id: Uuid,
    }

    let issue_ws = IssueWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT wi.project_id, p.workspace_id
            FROM work_items wi
            INNER JOIN projects p ON wi.project_id = p.id
            WHERE wi.id = $1
        ",
        vec![issue_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("issue not found".to_string()))?;

    require_workspace_access(&state, &extensions, issue_ws.workspace_id).await?;

    // Verify label belongs to same workspace
    let label_exists = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM labels WHERE id = $1 AND workspace_id = $2",
            vec![label_id.into(), issue_ws.workspace_id.into()],
        ))
        .await?;

    if label_exists.is_none() {
        return Err(ApiError::NotFound("label not found in workspace".to_string()));
    }

    // Check if already associated
    let already_linked = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM work_item_labels WHERE work_item_id = $1 AND label_id = $2",
            vec![issue_id.into(), label_id.into()],
        ))
        .await?;

    if already_linked.is_some() {
        return Err(ApiError::Conflict("label already added to issue".to_string()));
    }

    let now = chrono::Utc::now();

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO work_item_labels (work_item_id, label_id, created_at) VALUES ($1, $2, $3)",
            vec![issue_id.into(), label_id.into(), now.into()],
        ))
        .await?;

    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::LabelAdded,
            workspace_id: issue_ws.workspace_id,
            project_id: issue_ws.project_id,
            actor_id: user_id,
            issue_id: Some(issue_id),
            comment_id: None,
            label_id: Some(label_id),
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: None,
        },
    );

    Ok(ApiResponse::ok())
}

/// GET /api/v1/issues/:issue_id/labels - Get issue's labels
pub async fn get_issue_labels(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(issue_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let _user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }

    #[derive(Debug, FromQueryResult)]
    struct IssueWorkspace {
        workspace_id: Uuid,
    }

    let issue = IssueWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT p.workspace_id
            FROM work_items wi
            INNER JOIN projects p ON wi.project_id = p.id
            WHERE wi.id = $1
        ",
        vec![issue_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("issue not found".to_string()))?;

    require_workspace_access(&state, &extensions, issue.workspace_id).await?;

    #[derive(Debug, FromQueryResult)]
    struct LabelRow {
        id: Uuid,
        workspace_id: Uuid,
        name: String,
        color: String,
        description: String,
        created_at: chrono::DateTime<chrono::Utc>,
    }

    let labels = LabelRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT l.id, l.workspace_id, l.name, l.color, l.description, l.created_at
            FROM labels l
            INNER JOIN work_item_labels wil ON l.id = wil.label_id
            WHERE wil.work_item_id = $1
            ORDER BY l.name ASC
        ",
        vec![issue_id.into()],
    ))
    .all(&state.db)
    .await?;

    let response: Vec<LabelResponse> = labels
        .into_iter()
        .map(|l| LabelResponse {
            id: l.id,
            workspace_id: l.workspace_id,
            name: l.name,
            color: l.color,
            description: l.description,
            created_at: l.created_at.to_rfc3339(),
        })
        .collect();

    Ok(ApiResponse::success(PaginatedData::from_items(response)))
}

/// DELETE /api/v1/issues/:issue_id/labels/:label_id - Remove label from issue
pub async fn remove_label_from_issue(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((issue_id, label_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }

    #[derive(Debug, FromQueryResult)]
    struct IssueContext {
        project_id: Uuid,
        workspace_id: Uuid,
    }

    // Verify issue exists and user has access
    let issue_context = IssueContext::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT wi.project_id, p.workspace_id
            FROM work_items wi
            INNER JOIN projects p ON wi.project_id = p.id
            WHERE wi.id = $1
        ",
        vec![issue_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("issue not found".to_string()))?;

    require_workspace_access(&state, &extensions, issue_context.workspace_id).await?;

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM work_item_labels WHERE work_item_id = $1 AND label_id = $2",
            vec![issue_id.into(), label_id.into()],
        ))
        .await?;

    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::LabelRemoved,
            workspace_id: issue_context.workspace_id,
            project_id: issue_context.project_id,
            actor_id: user_id,
            issue_id: Some(issue_id),
            comment_id: None,
            label_id: Some(label_id),
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: None,
        },
    );

    Ok(ApiResponse::ok())
}

/// GET /api/v1/projects/:project_id/labels (bot-auth)
/// Lists all workspace labels available for a project.
pub async fn list_project_labels(
    State(state): State<AppState>,
    Extension(bot): Extension<BotAuthContext>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    // Verify project belongs to bot's workspace
    #[derive(Debug, FromQueryResult)]
    struct ProjectRow {
        workspace_id: Uuid,
    }

    let project = ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;

    if project.workspace_id != bot.workspace_id {
        return Err(ApiError::Forbidden("project not in bot workspace".to_string()));
    }

    #[derive(Debug, FromQueryResult)]
    struct LabelRow {
        id: Uuid,
        workspace_id: Uuid,
        name: String,
        color: String,
        description: String,
        created_at: chrono::DateTime<chrono::Utc>,
    }

    let labels = LabelRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, name, color, description, created_at \
         FROM labels WHERE workspace_id = $1 ORDER BY name ASC",
        vec![project.workspace_id.into()],
    ))
    .all(&state.db)
    .await?;
    let response: Vec<LabelResponse> = labels
        .into_iter()
        .map(|l| LabelResponse {
            id: l.id,
            workspace_id: l.workspace_id,
            name: l.name,
            color: l.color,
            description: l.description,
            created_at: l.created_at.to_rfc3339(),
        })
        .collect();

    Ok(ApiResponse::success(PaginatedData::from_items(response)))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic,
    clippy::nursery
)]
mod tests {
    use super::{BotAuthContext, Extension, FromQueryResult, ensure_label_mutation_allowed};
    use crate::error::ApiError;
    use uuid::Uuid;

    fn forbidden_message(result: Result<(), ApiError>) -> String {
        match result {
            Err(ApiError::Forbidden(msg)) => msg,
            other => panic!("expected Forbidden, got {other:?}"),
        }
    }

    #[test]
    fn label_mutation_allows_workspace_owner_and_admin() {
        assert!(ensure_label_mutation_allowed("owner", false, "update").is_ok());
        assert!(ensure_label_mutation_allowed("admin", false, "update").is_ok());
        assert!(ensure_label_mutation_allowed("admin", true, "update").is_ok());
        assert!(ensure_label_mutation_allowed("owner", false, "delete").is_ok());
    }

    #[test]
    fn label_mutation_rejects_plain_member() {
        let msg = forbidden_message(ensure_label_mutation_allowed("member", false, "update"));
        assert_eq!(msg, "only owners and admins can update labels");

        let msg = forbidden_message(ensure_label_mutation_allowed("member", false, "delete"));
        assert_eq!(msg, "only owners and admins can delete labels");
    }

    #[test]
    fn label_mutation_rejects_bot_without_admin_permission() {
        let msg = forbidden_message(ensure_label_mutation_allowed("member", true, "update"));
        assert_eq!(msg, "bot token requires the 'admin' permission to update labels");
    }

    #[test]
    fn label_mutation_rejects_unknown_role() {
        assert!(ensure_label_mutation_allowed("guest", false, "update").is_err());
        assert!(ensure_label_mutation_allowed("", true, "delete").is_err());
    }

    // ---- Real-database tests (opt-in via OPENPR_TEST_DATABASE_URL) ----
    //
    // These drive the `update_label` / `delete_label` handlers themselves, so
    // they fail if the guard is removed from a handler and not only from
    // `ensure_label_mutation_allowed`.

    use super::{UpdateLabelRequest, delete_label, update_label};
    use axum::{
        Json,
        extract::{Path, State},
    };
    use platform::{
        app::AppState,
        auth::{JwtClaims, TokenType},
        config::AppConfig,
    };
    use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};

    struct Fixture {
        state: AppState,
        workspace_id: Uuid,
        label_id: Uuid,
        owner_id: Uuid,
        member_id: Uuid,
        read_bot_id: Uuid,
        admin_bot_id: Uuid,
    }

    /// Caller identity used to invoke a handler.
    #[derive(Clone, Copy)]
    enum Caller {
        User(Uuid),
        /// Bot token: (bot id, workspace of the token, has the `admin` permission).
        Bot(Uuid, Uuid, bool),
    }

    async fn state_from_env() -> Option<AppState> {
        let url = std::env::var("OPENPR_TEST_DATABASE_URL").ok()?;
        let db = Database::connect(&url)
            .await
            .unwrap_or_else(|err| panic!("cannot connect to OPENPR_TEST_DATABASE_URL: {err}"));
        let cfg = AppConfig {
            app_name: "api-test".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: url,
            jwt_secret: "test-secret".to_string(),
            jwt_access_ttl_seconds: 60,
            jwt_refresh_ttl_seconds: 60,
            default_author_id: None,
        };
        Some(AppState { cfg, db })
    }

    async fn exec(state: &AppState, sql: &str, values: Vec<sea_orm::Value>) {
        state
            .db
            .execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .await
            .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
    }

    /// Seeds one workspace with an owner, a plain member, two bot identities and
    /// one label.
    ///
    /// The bots are inserted the way `create_bot` does it, i.e. they also get a
    /// `workspace_members` row. That row is what made the pre-fix `update_label`
    /// reachable for a read-only bot token, so the guard cannot rely on bots
    /// being absent from `workspace_members`.
    async fn seed(state: AppState) -> Fixture {
        let workspace_id = Uuid::new_v4();
        let label_id = Uuid::new_v4();
        let owner_id = Uuid::new_v4();
        let member_id = Uuid::new_v4();
        let read_bot_id = Uuid::new_v4();
        let admin_bot_id = Uuid::new_v4();

        for (user_id, role) in [
            (owner_id, "owner"),
            (member_id, "member"),
            (read_bot_id, "member"),
            (admin_bot_id, "admin"),
        ] {
            exec(
                &state,
                "INSERT INTO users (id, email, password_hash, name, role, is_active) \
                 VALUES ($1, $2, '!', 'test', 'user', true)",
                vec![user_id.into(), format!("{user_id}@label.test").into()],
            )
            .await;
            if user_id == owner_id {
                exec(
                    &state,
                    "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'label test', $3)",
                    vec![
                        workspace_id.into(),
                        format!("ws-{workspace_id}").into(),
                        owner_id.into(),
                    ],
                )
                .await;
            }
            exec(
                &state,
                "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, $3)",
                vec![workspace_id.into(), user_id.into(), role.into()],
            )
            .await;
        }

        exec(
            &state,
            "INSERT INTO labels (id, workspace_id, name, color, description) VALUES ($1, $2, 'bug', '#f00', '')",
            vec![label_id.into(), workspace_id.into()],
        )
        .await;

        Fixture {
            state,
            workspace_id,
            label_id,
            owner_id,
            member_id,
            read_bot_id,
            admin_bot_id,
        }
    }

    async fn cleanup(fx: &Fixture) {
        exec(
            &fx.state,
            "DELETE FROM workspaces WHERE id = $1",
            vec![fx.workspace_id.into()],
        )
        .await;
        for user_id in [fx.owner_id, fx.member_id, fx.read_bot_id, fx.admin_bot_id] {
            exec(&fx.state, "DELETE FROM users WHERE id = $1", vec![user_id.into()]).await;
        }
    }

    fn auth_for(caller: Caller) -> (Extension<JwtClaims>, Option<Extension<BotAuthContext>>) {
        let (subject, bot) = match caller {
            Caller::User(user_id) => (user_id, None),
            Caller::Bot(bot_id, workspace_id, is_admin) => {
                let permissions = if is_admin {
                    vec!["read".to_string(), "write".to_string(), "admin".to_string()]
                } else {
                    vec!["read".to_string()]
                };
                (
                    bot_id,
                    Some(Extension(BotAuthContext {
                        bot_id,
                        workspace_id,
                        permissions,
                    })),
                )
            }
        };
        let claims = JwtClaims {
            sub: subject.to_string(),
            email: format!("{subject}@label.test"),
            token_type: TokenType::Access,
            iat: 0,
            exp: 0,
        };
        (Extension(claims), bot)
    }

    /// Calls `update_label` the way the router does, renaming the label.
    async fn rename(fx: &Fixture, caller: Caller, new_name: &str) -> Result<(), ApiError> {
        let (claims, bot) = auth_for(caller);
        update_label(
            State(fx.state.clone()),
            claims,
            bot,
            Path(fx.label_id),
            Json(UpdateLabelRequest {
                name: Some(new_name.to_string()),
                color: None,
                description: None,
            }),
        )
        .await
        .map(|_| ())
    }

    async fn label_name(fx: &Fixture) -> Option<String> {
        #[derive(Debug, FromQueryResult)]
        struct NameRow {
            name: String,
        }

        NameRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT name FROM labels WHERE id = $1",
            vec![fx.label_id.into()],
        ))
        .one(&fx.state.db)
        .await
        .unwrap_or_else(|err| panic!("label lookup failed: {err}"))
        .map(|row| row.name)
    }

    #[tokio::test]
    async fn db_update_label_requires_owner_or_admin() {
        let Some(state) = state_from_env().await else {
            return;
        };
        let fx = seed(state).await;

        // A plain workspace member must not rewrite the workspace-wide taxonomy.
        let denied = rename(&fx, Caller::User(fx.member_id), "member-rename").await;
        assert!(
            matches!(denied, Err(ApiError::Forbidden(_))),
            "member must be rejected, got {denied:?}"
        );
        assert_eq!(label_name(&fx).await.as_deref(), Some("bug"));

        // A read-only bot has a workspace_members row, so only the explicit
        // permission-bit check keeps it out.
        let denied = rename(&fx, Caller::Bot(fx.read_bot_id, fx.workspace_id, false), "bot-rename").await;
        assert!(
            matches!(denied, Err(ApiError::Forbidden(_))),
            "read-only bot must be rejected, got {denied:?}"
        );
        assert_eq!(label_name(&fx).await.as_deref(), Some("bug"));

        // A bot token issued for a different workspace is rejected as well.
        let denied = rename(
            &fx,
            Caller::Bot(fx.admin_bot_id, Uuid::new_v4(), true),
            "foreign-bot-rename",
        )
        .await;
        assert!(
            matches!(denied, Err(ApiError::Forbidden(_))),
            "cross-workspace bot must be rejected, got {denied:?}"
        );
        assert_eq!(label_name(&fx).await.as_deref(), Some("bug"));

        // The owner may rename.
        rename(&fx, Caller::User(fx.owner_id), "owner-rename")
            .await
            .unwrap_or_else(|err| panic!("owner rename failed: {err}"));
        assert_eq!(label_name(&fx).await.as_deref(), Some("owner-rename"));

        // A bot carrying the admin permission bit may rename.
        rename(
            &fx,
            Caller::Bot(fx.admin_bot_id, fx.workspace_id, true),
            "admin-bot-rename",
        )
        .await
        .unwrap_or_else(|err| panic!("admin bot rename failed: {err}"));
        assert_eq!(label_name(&fx).await.as_deref(), Some("admin-bot-rename"));

        cleanup(&fx).await;
    }

    #[tokio::test]
    async fn db_delete_label_requires_owner_or_admin() {
        let Some(state) = state_from_env().await else {
            return;
        };
        let fx = seed(state).await;

        let (claims, bot) = auth_for(Caller::User(fx.member_id));
        let denied = delete_label(State(fx.state.clone()), claims, bot, Path(fx.label_id))
            .await
            .map(|_| ());
        assert!(
            matches!(denied, Err(ApiError::Forbidden(_))),
            "member must not delete labels, got {denied:?}"
        );
        assert!(label_name(&fx).await.is_some());

        let (claims, bot) = auth_for(Caller::Bot(fx.read_bot_id, fx.workspace_id, false));
        let denied = delete_label(State(fx.state.clone()), claims, bot, Path(fx.label_id))
            .await
            .map(|_| ());
        assert!(
            matches!(denied, Err(ApiError::Forbidden(_))),
            "read-only bot must not delete labels, got {denied:?}"
        );
        assert!(label_name(&fx).await.is_some());

        let (claims, bot) = auth_for(Caller::User(fx.owner_id));
        delete_label(State(fx.state.clone()), claims, bot, Path(fx.label_id))
            .await
            .map(|_| ())
            .unwrap_or_else(|err| panic!("owner delete failed: {err}"));
        assert!(label_name(&fx).await.is_none());

        cleanup(&fx).await;
    }
}
