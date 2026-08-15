use axum::{
    Extension,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use crate::{error::ApiError, middleware::bot_auth::BotAuthContext, response::ApiResponse};

const RELEASE_READINESS_SCHEMA_VERSION: &str = "openpr.project.release_readiness.v1";
const RELEASE_READINESS_SCHEMA_PATH: &str = "docs/schemas/openpr-project-release-readiness.schema.json";

#[derive(Debug, FromQueryResult)]
struct ProjectWorkspaceRow {
    workspace_id: Uuid,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    count: i64,
}

#[derive(Debug, Serialize)]
struct ReadinessMetric {
    key: &'static str,
    value: i64,
}

#[derive(Debug, Serialize)]
struct ReadinessGate {
    key: &'static str,
    name: &'static str,
    status: &'static str,
    required: bool,
    observed: i64,
    details: String,
}

#[derive(Debug, Serialize)]
struct ReadinessNextAction {
    review_order: usize,
    key: String,
    source_gate: String,
    blocking: bool,
    actor: &'static str,
    action: String,
    recommended_tool: &'static str,
    details: String,
}

pub async fn get_project_release_readiness(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let project = find_project_workspace(&state, project_id).await?;
    ensure_project_access(&state, &claims, bot.as_ref().map(|b| &b.0), project.workspace_id).await?;

    let pending_ai_tasks = count_pending_ai_tasks(&state, project_id).await?;
    let active_invocations = count_invocations(&state, project_id, &["pending", "dispatched", "running"]).await?;
    let failed_invocations = count_invocations(&state, project_id, &["failed"]).await?;
    let pending_check_results = count_check_results(&state, project_id, &["requires_proposal", "proposed"]).await?;
    let failed_connector_deliveries =
        count_failed_connector_deliveries(&state, project.workspace_id, project_id).await?;
    let stale_resources = count_stale_resources(&state, project_id).await?;
    let audit_events = count_audit_events(&state, project_id).await?;
    let result_artifacts = count_result_artifacts(&state, project_id).await?;

    let gates = vec![
        gate(
            "no_pending_ai_tasks",
            "No outstanding AI tasks",
            pending_ai_tasks,
            "pending, processing, and retried AI tasks must be closed",
        ),
        gate(
            "no_active_invocations",
            "No active invocations",
            active_invocations,
            "pending, dispatched, and running invocations must be closed",
        ),
        gate(
            "no_failed_invocations",
            "No failed invocations",
            failed_invocations,
            "failed invocations require review before release",
        ),
        gate(
            "no_pending_governance_results",
            "No pending governed check results",
            pending_check_results,
            "requires_proposal and proposed check results must be resolved",
        ),
        gate(
            "no_failed_connector_deliveries",
            "No failed connector deliveries",
            failed_connector_deliveries,
            "connector/webhook delivery errors must be reviewed",
        ),
        gate(
            "no_stale_resources",
            "No stale or failed resources",
            stale_resources,
            "resources marked stale, failed, or error must be refreshed",
        ),
        positive_gate(
            "audit_trail_present",
            "Governance audit trail present",
            audit_events,
            "at least one governance audit event should exist for release review",
        ),
        positive_gate(
            "result_artifact_present",
            "AI/result artifact present",
            result_artifacts,
            "at least one comment, check result, or invocation result should exist",
        ),
    ];
    let blockers = gates
        .iter()
        .filter(|gate| gate.required && gate.status != "passed")
        .map(|gate| gate.key)
        .collect::<Vec<_>>();
    let status = if blockers.is_empty() { "ready" } else { "blocked" };
    let next_actions = next_actions_for_gates(&gates);

    Ok(ApiResponse::success(json!({
        "schema_version": RELEASE_READINESS_SCHEMA_VERSION,
        "schema_path": RELEASE_READINESS_SCHEMA_PATH,
        "project_id": project_id,
        "status": status,
        "blockers": blockers,
        "gates": gates,
        "next_actions": next_actions,
        "metrics": [
            ReadinessMetric { key: "pending_ai_tasks", value: pending_ai_tasks },
            ReadinessMetric { key: "active_invocations", value: active_invocations },
            ReadinessMetric { key: "failed_invocations", value: failed_invocations },
            ReadinessMetric { key: "pending_check_results", value: pending_check_results },
            ReadinessMetric { key: "failed_connector_deliveries", value: failed_connector_deliveries },
            ReadinessMetric { key: "stale_resources", value: stale_resources },
            ReadinessMetric { key: "governance_audit_events", value: audit_events },
            ReadinessMetric { key: "result_artifacts", value: result_artifacts },
        ],
        "generated_at": chrono::Utc::now(),
    })))
}

fn next_actions_for_gates(gates: &[ReadinessGate]) -> Vec<ReadinessNextAction> {
    let mut actions = gates
        .iter()
        .filter(|gate| gate.required && gate.status != "passed")
        .enumerate()
        .map(|(index, gate)| ReadinessNextAction {
            review_order: index + 1,
            key: format!("resolve_{}", gate.key),
            source_gate: gate.key.to_string(),
            blocking: true,
            actor: next_action_actor(gate.key),
            action: format!("Resolve release readiness gate: {}", gate.name),
            recommended_tool: next_action_tool(gate.key),
            details: gate.details.clone(),
        })
        .collect::<Vec<_>>();

    if actions.is_empty() {
        actions.push(ReadinessNextAction {
            review_order: 1,
            key: "review_release_evidence".to_string(),
            source_gate: "all_required_gates".to_string(),
            blocking: false,
            actor: "reviewer",
            action: "Review release evidence and proceed with the release decision".to_string(),
            recommended_tool: "release.readiness.get",
            details: "all required release readiness gates are passed".to_string(),
        });
    }

    actions
}

fn next_action_actor(gate_key: &str) -> &'static str {
    match gate_key {
        "no_pending_ai_tasks" => "ai_operator",
        "no_active_invocations" | "no_failed_invocations" | "no_failed_connector_deliveries" => "operator",
        "no_pending_governance_results" | "audit_trail_present" | "result_artifact_present" => "reviewer",
        "no_stale_resources" => "project_owner",
        _ => "operator",
    }
}

fn next_action_tool(gate_key: &str) -> &'static str {
    match gate_key {
        "no_pending_ai_tasks" => "context.get_governance",
        "no_active_invocations" | "no_failed_invocations" | "no_failed_connector_deliveries" => "invocations.list",
        "no_pending_governance_results" | "result_artifact_present" => "check_results.create",
        "no_stale_resources" => "project_resources.list",
        "audit_trail_present" => "context.get_project",
        _ => "release.readiness.get",
    }
}

fn gate(key: &'static str, name: &'static str, observed: i64, details: &'static str) -> ReadinessGate {
    ReadinessGate {
        key,
        name,
        status: if observed == 0 { "passed" } else { "blocked" },
        required: true,
        observed,
        details: details.to_string(),
    }
}

fn positive_gate(key: &'static str, name: &'static str, observed: i64, details: &'static str) -> ReadinessGate {
    ReadinessGate {
        key,
        name,
        status: if observed > 0 { "passed" } else { "blocked" },
        required: true,
        observed,
        details: details.to_string(),
    }
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

async fn ensure_project_access(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&BotAuthContext>,
    workspace_id: Uuid,
) -> Result<(), ApiError> {
    if let Some(bot) = bot {
        if bot.workspace_id != workspace_id {
            return Err(ApiError::Forbidden("bot not authorized for this project".to_string()));
        }
        return Ok(());
    }

    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let has_access = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
            vec![workspace_id.into(), user_id.into()],
        ))
        .await?
        .is_some();
    if !has_access {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }
    Ok(())
}

async fn count_pending_ai_tasks(state: &AppState, project_id: Uuid) -> Result<i64, ApiError> {
    count_query(
        state,
        r"
            SELECT COUNT(*)::bigint AS count
            FROM ai_tasks
            WHERE project_id = $1
              AND status IN ('pending', 'processing', 'retried')
        ",
        vec![project_id.into()],
    )
    .await
}

async fn count_invocations(state: &AppState, project_id: Uuid, statuses: &[&str]) -> Result<i64, ApiError> {
    let status_values = statuses
        .iter()
        .map(|status| format!("'{status}'"))
        .collect::<Vec<_>>()
        .join(", ");
    count_query(
        state,
        &format!(
            r"
                SELECT COUNT(*)::bigint AS count
                FROM agent_invocations
                WHERE project_id = $1
                  AND status IN ({status_values})
            "
        ),
        vec![project_id.into()],
    )
    .await
}

async fn count_check_results(state: &AppState, project_id: Uuid, statuses: &[&str]) -> Result<i64, ApiError> {
    let status_values = statuses
        .iter()
        .map(|status| format!("'{status}'"))
        .collect::<Vec<_>>()
        .join(", ");
    count_query(
        state,
        &format!(
            r"
                SELECT COUNT(*)::bigint AS count
                FROM check_results
                WHERE project_id = $1
                  AND status IN ({status_values})
            "
        ),
        vec![project_id.into()],
    )
    .await
}

async fn count_failed_connector_deliveries(
    state: &AppState,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<i64, ApiError> {
    count_query(
        state,
        r"
            SELECT COUNT(*)::bigint AS count
            FROM webhook_deliveries wd
            INNER JOIN webhooks w ON w.id = wd.webhook_id
            WHERE w.workspace_id = $1
              AND (wd.error IS NOT NULL OR COALESCE(wd.response_status, 200) >= 400)
              AND (
                wd.payload->>'project_id' = $2::text
                OR wd.payload->'project'->>'id' = $2::text
                OR wd.payload->'data'->>'project_id' = $2::text
              )
        ",
        vec![workspace_id.into(), project_id.into()],
    )
    .await
}

async fn count_stale_resources(state: &AppState, project_id: Uuid) -> Result<i64, ApiError> {
    count_query(
        state,
        r"
            SELECT COUNT(*)::bigint AS count
            FROM project_resources
            WHERE project_id = $1
              AND sync_status IN ('stale', 'failed', 'error')
        ",
        vec![project_id.into()],
    )
    .await
}

async fn count_audit_events(state: &AppState, project_id: Uuid) -> Result<i64, ApiError> {
    count_query(
        state,
        r"
            SELECT COUNT(*)::bigint AS count
            FROM governance_audit_logs
            WHERE project_id = $1
        ",
        vec![project_id.into()],
    )
    .await
}

async fn count_result_artifacts(state: &AppState, project_id: Uuid) -> Result<i64, ApiError> {
    count_query(
        state,
        r"
            SELECT
              (
                SELECT COUNT(*)::bigint
                FROM check_results
                WHERE project_id = $1
              )
              +
              (
                SELECT COUNT(*)::bigint
                FROM agent_invocations
                WHERE project_id = $1
                  AND result IS NOT NULL
              )
              +
              (
                SELECT COUNT(*)::bigint
                FROM comments c
                INNER JOIN work_items wi ON wi.id = c.work_item_id
                WHERE wi.project_id = $1
              ) AS count
        ",
        vec![project_id.into()],
    )
    .await
}

async fn count_query(state: &AppState, sql: &str, values: Vec<sea_orm::Value>) -> Result<i64, ApiError> {
    let row = CountRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .one(&state.db)
        .await?
        .ok_or_else(|| ApiError::Internal)?;
    Ok(row.count)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::{
        RELEASE_READINESS_SCHEMA_PATH, RELEASE_READINESS_SCHEMA_VERSION, gate, next_actions_for_gates, positive_gate,
    };

    #[test]
    fn zero_count_gate_passes_only_when_empty() {
        assert_eq!(gate("a", "A", 0, "details").status, "passed");
        assert_eq!(gate("a", "A", 1, "details").status, "blocked");
    }

    #[test]
    fn positive_gate_requires_evidence() {
        assert_eq!(positive_gate("a", "A", 1, "details").status, "passed");
        assert_eq!(positive_gate("a", "A", 0, "details").status, "blocked");
    }

    #[test]
    fn release_readiness_schema_constants_are_versioned() {
        assert_eq!(RELEASE_READINESS_SCHEMA_VERSION, "openpr.project.release_readiness.v1");
        assert_eq!(
            RELEASE_READINESS_SCHEMA_PATH,
            "docs/schemas/openpr-project-release-readiness.schema.json"
        );
    }

    #[test]
    fn ready_release_readiness_returns_review_action() {
        let gates = vec![
            gate(
                "no_pending_ai_tasks",
                "No outstanding AI tasks",
                0,
                "pending tasks must be closed",
            ),
            positive_gate(
                "audit_trail_present",
                "Governance audit trail present",
                1,
                "audit trail required",
            ),
        ];

        let actions = next_actions_for_gates(&gates);

        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].review_order, 1);
        assert_eq!(actions[0].key, "review_release_evidence");
        assert_eq!(actions[0].source_gate, "all_required_gates");
        assert!(!actions[0].blocking);
        assert_eq!(actions[0].recommended_tool, "release.readiness.get");
    }

    #[test]
    fn blocked_release_readiness_returns_gate_remediation_actions() {
        let gates = vec![
            gate(
                "no_pending_ai_tasks",
                "No outstanding AI tasks",
                2,
                "pending tasks must be closed",
            ),
            positive_gate(
                "audit_trail_present",
                "Governance audit trail present",
                0,
                "audit trail required",
            ),
        ];

        let actions = next_actions_for_gates(&gates);
        let keys = actions.iter().map(|action| action.key.as_str()).collect::<Vec<_>>();

        assert_eq!(
            keys,
            vec!["resolve_no_pending_ai_tasks", "resolve_audit_trail_present",]
        );
        assert_eq!(actions[0].review_order, 1);
        assert_eq!(actions[0].source_gate, "no_pending_ai_tasks");
        assert!(actions[0].blocking);
        assert_eq!(actions[0].actor, "ai_operator");
        assert_eq!(actions[0].recommended_tool, "context.get_governance");
        assert_eq!(actions[1].review_order, 2);
        assert_eq!(actions[1].source_gate, "audit_trail_present");
        assert!(actions[1].blocking);
        assert_eq!(actions[1].actor, "reviewer");
        assert_eq!(actions[1].recommended_tool, "context.get_project");
    }
}
