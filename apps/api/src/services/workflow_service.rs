use crate::error::ApiError;
use platform::app::AppState;
use sea_orm::{DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowStateDef {
    pub key: String,
    pub display_name: String,
    pub category: String,
    pub position: i32,
    pub color: Option<String>,
    pub is_initial: bool,
    pub is_terminal: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct EffectiveWorkflow {
    pub workflow_id: Uuid,
    pub source: String,
    pub project_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub states: Vec<WorkflowStateDef>,
}

#[derive(Debug, FromQueryResult)]
struct WorkflowRow {
    workflow_id: Uuid,
    source: String,
    project_id: Option<Uuid>,
    workspace_id: Option<Uuid>,
    name: String,
}

#[derive(Debug, FromQueryResult)]
struct WorkflowStateRow {
    key: String,
    display_name: String,
    category: String,
    position: i32,
    color: Option<String>,
    is_initial: bool,
    is_terminal: bool,
}

pub async fn resolve_effective_workflow_for_project(
    state: &AppState,
    project_id: Uuid,
) -> Result<EffectiveWorkflow, ApiError> {
    let row = WorkflowRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
        WITH project_ctx AS (
          SELECT p.id AS project_id,
                 p.workspace_id,
                 p.workflow_id AS project_workflow_id,
                 ws.workflow_id AS workspace_workflow_id
          FROM projects p
          INNER JOIN workspaces ws ON ws.id = p.workspace_id
          WHERE p.id = $1
        ),
        system_workflow AS (
          SELECT w.id
          FROM workflows w
          WHERE w.is_system_default = TRUE
          ORDER BY w.created_at ASC
          LIMIT 1
        )
        SELECT
          COALESCE(pc.project_workflow_id, pc.workspace_workflow_id, sw.id) AS workflow_id,
          CASE
            WHEN pc.project_workflow_id IS NOT NULL THEN 'project'
            WHEN pc.workspace_workflow_id IS NOT NULL THEN 'workspace'
            ELSE 'system'
          END AS source,
          pc.project_id,
          pc.workspace_id,
          w.name
        FROM project_ctx pc
        CROSS JOIN system_workflow sw
        INNER JOIN workflows w ON w.id = COALESCE(pc.project_workflow_id, pc.workspace_workflow_id, sw.id)
        ",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;

    let states = load_workflow_states(state, row.workflow_id).await?;

    Ok(EffectiveWorkflow {
        workflow_id: row.workflow_id,
        source: row.source,
        project_id: row.project_id,
        workspace_id: row.workspace_id,
        name: row.name,
        states,
    })
}

pub async fn default_project_state(state: &AppState, project_id: Uuid) -> Result<String, ApiError> {
    let workflow = resolve_effective_workflow_for_project(state, project_id).await?;
    if let Some(initial) = workflow.states.iter().find(|s| s.is_initial) {
        return Ok(initial.key.clone());
    }
    Ok(workflow
        .states
        .first()
        .map(|s| s.key.clone())
        .unwrap_or_else(|| "todo".to_string()))
}

pub fn allowed_state_values(workflow: &EffectiveWorkflow) -> String {
    workflow
        .states
        .iter()
        .map(|s| s.key.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

async fn load_workflow_states(state: &AppState, workflow_id: Uuid) -> Result<Vec<WorkflowStateDef>, ApiError> {
    let rows = WorkflowStateRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
        SELECT key, display_name, category, position, color, is_initial, is_terminal
        FROM workflow_states
        WHERE workflow_id = $1
        ORDER BY position ASC
        ",
        vec![workflow_id.into()],
    ))
    .all(&state.db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| WorkflowStateDef {
            key: row.key,
            display_name: row.display_name,
            category: row.category,
            position: row.position,
            color: row.color,
            is_initial: row.is_initial,
            is_terminal: row.is_terminal,
        })
        .collect())
}
