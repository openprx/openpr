use chrono::{DateTime, Duration, Utc};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    events::{BusinessEventInput, insert_business_event},
};

#[derive(Debug, Clone, FromQueryResult)]
pub struct AiTaskRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub ai_participant_id: Uuid,
    pub task_type: String,
    pub reference_type: Option<String>,
    pub reference_id: Option<Uuid>,
    pub status: String,
    pub priority: i32,
    pub payload: Value,
    pub result: Option<Value>,
    pub error_message: Option<String>,
    pub idempotency_key: Option<String>,
    pub attempts: i32,
    pub max_attempts: i32,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct CreateAiTaskInput {
    pub project_id: Uuid,
    pub ai_participant_id: Uuid,
    pub actor_id: Option<Uuid>,
    pub task_type: String,
    pub reference_type: Option<String>,
    pub reference_id: Option<Uuid>,
    pub priority: i32,
    pub payload: Value,
    pub idempotency_key: Option<String>,
    pub max_attempts: i32,
}

#[derive(Debug, FromQueryResult)]
struct ProjectWorkspaceRow {
    workspace_id: Uuid,
}

pub async fn create_ai_task<C: ConnectionTrait>(
    db: &C,
    input: CreateAiTaskInput,
) -> Result<Option<AiTaskRow>, ApiError> {
    let now = Utc::now();
    let max_attempts = input.max_attempts.max(1);

    let created = AiTaskRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO ai_tasks (
                id,
                project_id,
                ai_participant_id,
                task_type,
                reference_type,
                reference_id,
                status,
                priority,
                payload,
                idempotency_key,
                max_attempts,
                created_at,
                updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7, $8, $9, $10, $11, $12)
            ON CONFLICT (idempotency_key) DO NOTHING
            RETURNING
                id,
                project_id,
                ai_participant_id,
                task_type,
                reference_type,
                reference_id,
                status,
                priority,
                payload,
                result,
                error_message,
                idempotency_key,
                attempts,
                max_attempts,
                next_retry_at,
                started_at,
                completed_at,
                created_at,
                updated_at
        ",
        vec![
            Uuid::new_v4().into(),
            input.project_id.into(),
            input.ai_participant_id.into(),
            input.task_type.into(),
            input.reference_type.clone().into(),
            input.reference_id.into(),
            input.priority.into(),
            input.payload.clone().into(),
            input.idempotency_key.clone().into(),
            max_attempts.into(),
            now.into(),
            now.into(),
        ],
    ))
    .one(db)
    .await?;

    let Some(task) = created else {
        return Ok(None);
    };

    insert_ai_task_event(
        db,
        task.id,
        "created",
        json!({
            "task_type": task.task_type,
            "reference_type": task.reference_type,
            "reference_id": task.reference_id.map(|v| v.to_string()),
            "idempotency_key": task.idempotency_key,
        }),
    )
    .await?;
    insert_ai_task_business_event(
        db,
        &task,
        "ai_task.created",
        input.actor_id,
        json!({ "type": "ai_task_service", "actor_id": input.actor_id }),
        json!({
            "task_type": task.task_type,
            "reference_type": task.reference_type,
            "reference_id": task.reference_id.map(|v| v.to_string()),
            "idempotency_key": task.idempotency_key,
        }),
        None,
    )
    .await?;

    Ok(Some(task))
}

pub async fn insert_ai_task_event<C: ConnectionTrait>(
    db: &C,
    task_id: Uuid,
    event_type: &str,
    payload: Value,
) -> Result<(), ApiError> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO ai_task_events (id, task_id, event_type, payload, created_at)
            VALUES ($1, $2, $3, $4, $5)
        ",
        vec![
            Uuid::new_v4().into(),
            task_id.into(),
            event_type.to_string().into(),
            payload.into(),
            Utc::now().into(),
        ],
    ))
    .await?;
    Ok(())
}

pub async fn insert_ai_task_business_event<C: ConnectionTrait>(
    db: &C,
    task: &AiTaskRow,
    event_type: &str,
    actor_id: Option<Uuid>,
    source: Value,
    event_payload: Value,
    previous_status: Option<&str>,
) -> Result<(), ApiError> {
    let project = ProjectWorkspaceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![task.project_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;

    insert_business_event(
        db,
        BusinessEventInput {
            workspace_id: project.workspace_id,
            project_id: Some(task.project_id),
            event_type: event_type.to_string(),
            aggregate_type: "ai_task".to_string(),
            aggregate_id: task.id.to_string(),
            actor_id,
            source,
            payload: json!({
                "task_id": task.id,
                "project_id": task.project_id,
                "ai_participant_id": task.ai_participant_id,
                "task_type": task.task_type,
                "reference_type": task.reference_type,
                "reference_id": task.reference_id,
                "status": task.status,
                "previous_status": previous_status,
                "priority": task.priority,
                "payload": task.payload,
                "result": task.result,
                "error_message": task.error_message,
                "attempts": task.attempts,
                "max_attempts": task.max_attempts,
                "next_retry_at": task.next_retry_at.map(|v| v.to_rfc3339()),
                "event_payload": event_payload
            }),
            metadata: json!({
                "ai_task_id": task.id,
                "ai_participant_id": task.ai_participant_id,
                "task_type": task.task_type,
                "reference_type": task.reference_type,
                "reference_id": task.reference_id,
                "status": task.status,
                "previous_status": previous_status
            }),
            correlation_id: Some(task.id),
            causation_id: task.reference_id,
            idempotency_key: None,
        },
    )
    .await?;
    Ok(())
}

pub fn next_retry_time(attempts: i32) -> DateTime<Utc> {
    let safe_attempts = attempts.max(1);
    let backoff_seconds = (safe_attempts * 30).min(30 * 20);
    Utc::now() + Duration::seconds(i64::from(backoff_seconds))
}

pub fn valid_task_type(value: &str) -> bool {
    matches!(
        value,
        "issue_assigned" | "review_requested" | "comment_requested" | "vote_requested"
    )
}

pub fn valid_reference_type(value: &str) -> bool {
    matches!(value, "work_item" | "proposal" | "comment")
}

pub async fn queue_vote_requested_tasks_for_project<C: ConnectionTrait>(
    db: &C,
    project_id: Uuid,
    proposal_id: &str,
    payload: Value,
) -> Result<usize, ApiError> {
    #[derive(Debug, FromQueryResult)]
    struct BotRow {
        user_id: Uuid,
    }

    let bots = BotRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT u.id AS user_id
            FROM ai_participants ap
            INNER JOIN users u
                ON u.id::text = ap.id
            WHERE ap.project_id = $1
              AND ap.is_active = true
              AND u.entity_type = 'bot'
        ",
        vec![project_id.into()],
    ))
    .all(db)
    .await?;

    let proposal_uuid = Uuid::parse_str(proposal_id).ok();
    let mut created = 0usize;
    for bot in bots {
        let idempotency_key = Some(format!("vote_requested:{project_id}:{proposal_id}:{}", bot.user_id));
        let task = create_ai_task(
            db,
            CreateAiTaskInput {
                project_id,
                ai_participant_id: bot.user_id,
                actor_id: None,
                task_type: "vote_requested".to_string(),
                reference_type: Some("proposal".to_string()),
                reference_id: proposal_uuid,
                priority: 5,
                payload: payload.clone(),
                idempotency_key,
                max_attempts: 3,
            },
        )
        .await?;
        if task.is_some() {
            created += 1;
        }
    }

    Ok(created)
}
