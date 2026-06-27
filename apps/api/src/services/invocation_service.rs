use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    events::{BusinessEventInput, insert_business_event},
    services::ai_task_service::AiTaskRow,
};

#[derive(Debug, FromQueryResult)]
struct ProjectWorkspaceRow {
    workspace_id: Uuid,
}

#[derive(Debug, FromQueryResult)]
struct InvocationEventRow {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    actor_id: Option<Uuid>,
    target_agent_id: Option<Uuid>,
    source_task_id: Option<Uuid>,
    trigger_kind: String,
    trigger_ref_type: Option<String>,
    trigger_ref_id: Option<Uuid>,
    connector_id: Option<Uuid>,
    connector_kind: Option<String>,
    status: String,
    payload: Value,
    result: Option<Value>,
    error_message: Option<String>,
    audit_chain_id: Option<Uuid>,
}

pub async fn create_invocation_for_ai_task<C: ConnectionTrait>(
    db: &C,
    task: &AiTaskRow,
    actor_id: Option<Uuid>,
) -> Result<(), ApiError> {
    let project = ProjectWorkspaceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![task.project_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;

    let created = InvocationEventRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO agent_invocations (
                id,
                workspace_id,
                project_id,
                actor_id,
                target_agent_id,
                source_task_id,
                trigger_kind,
                trigger_ref_type,
                trigger_ref_id,
                status,
                payload,
                audit_chain_id,
                created_at,
                updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'pending', $10, $11, now(), now())
            ON CONFLICT (source_task_id) DO NOTHING
            RETURNING id, workspace_id, project_id, actor_id, target_agent_id, source_task_id,
                      trigger_kind, trigger_ref_type, trigger_ref_id, connector_id, connector_kind,
                      status, payload, result, error_message, audit_chain_id
        ",
        vec![
            Uuid::new_v4().into(),
            project.workspace_id.into(),
            task.project_id.into(),
            actor_id.into(),
            task.ai_participant_id.into(),
            task.id.into(),
            trigger_kind_for_task(&task.task_type).into(),
            task.reference_type.clone().into(),
            task.reference_id.into(),
            task.payload.clone().into(),
            task.id.into(),
        ],
    ))
    .one(db)
    .await?;

    if let Some(invocation) = created {
        insert_ai_task_invocation_event(db, "invocation.created", &invocation, None, actor_id).await?;
    }

    Ok(())
}

pub async fn update_invocation_for_ai_task<C: ConnectionTrait>(
    db: &C,
    task_id: Uuid,
    status: &str,
    result: Option<Value>,
    error_message: Option<String>,
    actor_id: Option<Uuid>,
) -> Result<(), ApiError> {
    let previous = find_invocation_for_task(db, task_id).await?;

    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            UPDATE agent_invocations
            SET status = $2,
                result = COALESCE($3, result),
                error_message = $4,
                updated_at = now()
            WHERE source_task_id = $1
        ",
        vec![
            task_id.into(),
            status.to_string().into(),
            result.into(),
            error_message.into(),
        ],
    ))
    .await?;

    let updated = find_invocation_for_task(db, task_id).await?;
    if let (Some(previous), Some(updated)) = (previous, updated)
        && previous.status != updated.status
    {
        insert_ai_task_invocation_event(
            db,
            invocation_event_type_for_status(&updated.status),
            &updated,
            Some(previous.status.as_str()),
            actor_id,
        )
        .await?;
    }

    Ok(())
}

pub fn trigger_kind_for_task(task_type: &str) -> &'static str {
    match task_type {
        "issue_assigned" => "assigned",
        "vote_requested" => "proposal_vote",
        "review_requested" | "comment_requested" => "mention",
        _ => "manual",
    }
}

async fn find_invocation_for_task<C: ConnectionTrait>(
    db: &C,
    task_id: Uuid,
) -> Result<Option<InvocationEventRow>, ApiError> {
    InvocationEventRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, actor_id, target_agent_id, source_task_id,
                   trigger_kind, trigger_ref_type, trigger_ref_id, connector_id, connector_kind,
                   status, payload, result, error_message, audit_chain_id
            FROM agent_invocations
            WHERE source_task_id = $1
        ",
        vec![task_id.into()],
    ))
    .one(db)
    .await
    .map_err(ApiError::from)
}

async fn insert_ai_task_invocation_event<C: ConnectionTrait>(
    db: &C,
    event_type: &str,
    invocation: &InvocationEventRow,
    previous_status: Option<&str>,
    actor_id: Option<Uuid>,
) -> Result<(), ApiError> {
    insert_business_event(
        db,
        BusinessEventInput {
            workspace_id: invocation.workspace_id,
            project_id: invocation.project_id,
            event_type: event_type.to_string(),
            aggregate_type: "invocation".to_string(),
            aggregate_id: invocation.id.to_string(),
            actor_id,
            source: json!({
                "type": "ai_task",
                "task_id": invocation.source_task_id,
                "actor_id": actor_id
            }),
            payload: json!({
                "invocation_id": invocation.id,
                "workspace_id": invocation.workspace_id,
                "project_id": invocation.project_id,
                "actor_id": invocation.actor_id,
                "target_agent_id": invocation.target_agent_id,
                "source_task_id": invocation.source_task_id,
                "trigger_kind": invocation.trigger_kind,
                "trigger_ref_type": invocation.trigger_ref_type,
                "trigger_ref_id": invocation.trigger_ref_id,
                "connector_id": invocation.connector_id,
                "connector_kind": invocation.connector_kind,
                "status": invocation.status,
                "previous_status": previous_status,
                "payload": invocation.payload,
                "result": invocation.result,
                "error_message": invocation.error_message,
                "audit_chain_id": invocation.audit_chain_id
            }),
            metadata: json!({
                "ai_task_invocation": true,
                "invocation_id": invocation.id,
                "source_task_id": invocation.source_task_id,
                "trigger_kind": invocation.trigger_kind,
                "connector_id": invocation.connector_id,
                "connector_kind": invocation.connector_kind,
                "previous_status": previous_status,
                "status": invocation.status
            }),
            correlation_id: invocation.audit_chain_id,
            causation_id: invocation.source_task_id,
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

#[cfg(test)]
mod tests {
    use super::{invocation_event_type_for_status, trigger_kind_for_task};

    #[test]
    fn maps_ai_task_types_to_invocation_triggers() {
        assert_eq!(trigger_kind_for_task("issue_assigned"), "assigned");
        assert_eq!(trigger_kind_for_task("vote_requested"), "proposal_vote");
        assert_eq!(trigger_kind_for_task("review_requested"), "mention");
        assert_eq!(trigger_kind_for_task("comment_requested"), "mention");
        assert_eq!(trigger_kind_for_task("unknown"), "manual");
    }

    #[test]
    fn maps_invocation_statuses_to_business_event_types() {
        assert_eq!(invocation_event_type_for_status("running"), "invocation.running");
        assert_eq!(invocation_event_type_for_status("completed"), "invocation.completed");
        assert_eq!(invocation_event_type_for_status("failed"), "invocation.failed");
        assert_eq!(invocation_event_type_for_status("pending"), "invocation.status_changed");
    }
}
