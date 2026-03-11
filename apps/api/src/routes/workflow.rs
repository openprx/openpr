use axum::{
    Extension,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use uuid::Uuid;

use crate::{
    error::ApiError,
    middleware::bot_auth::{BotAuthContext, require_workspace_access},
    response::ApiResponse,
    services::workflow_service::resolve_effective_workflow_for_project,
};

/// GET /api/v1/projects/:project_id/workflow/effective
pub async fn get_effective_workflow_by_project(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let _user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }

    let workflow = resolve_effective_workflow_for_project(&state, project_id).await?;
    if let Some(workspace_id) = workflow.workspace_id {
        require_workspace_access(&state, &extensions, workspace_id).await?;
    }

    Ok(ApiResponse::success(workflow))
}
