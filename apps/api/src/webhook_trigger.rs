use std::collections::HashSet;
use std::time::Instant;

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use platform::app::AppState;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::Sha256;
use uuid::Uuid;

#[derive(Debug, Clone, Copy)]
pub enum WebhookEvent {
    IssueCreated,
    IssueUpdated,
    IssueAssigned,
    IssueStateChanged,
    IssueDeleted,
    CommentCreated,
    CommentUpdated,
    CommentDeleted,
    LabelAdded,
    LabelRemoved,
    SprintStarted,
    SprintCompleted,
    ProposalCreated,
    ProposalUpdated,
    ProposalDeleted,
    ProposalSubmitted,
    ProposalVotingStarted,
    ProposalArchived,
    ProposalVoteCast,
    ProjectCreated,
    ProjectUpdated,
    ProjectDeleted,
    MemberAdded,
    MemberRemoved,
    VetoExercised,
    VetoWithdrawn,
    EscalationStarted,
    AppealCreated,
    GovernanceConfigUpdated,
    AiTaskCompleted,
    AiTaskFailed,
}

impl WebhookEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IssueCreated => "issue.created",
            Self::IssueUpdated => "issue.updated",
            Self::IssueAssigned => "issue.assigned",
            Self::IssueStateChanged => "issue.state_changed",
            Self::IssueDeleted => "issue.deleted",
            Self::CommentCreated => "comment.created",
            Self::CommentUpdated => "comment.updated",
            Self::CommentDeleted => "comment.deleted",
            Self::LabelAdded => "label.added",
            Self::LabelRemoved => "label.removed",
            Self::SprintStarted => "sprint.started",
            Self::SprintCompleted => "sprint.completed",
            Self::ProposalCreated => "proposal.created",
            Self::ProposalUpdated => "proposal.updated",
            Self::ProposalDeleted => "proposal.deleted",
            Self::ProposalSubmitted => "proposal.submitted",
            Self::ProposalVotingStarted => "proposal.voting_started",
            Self::ProposalArchived => "proposal.archived",
            Self::ProposalVoteCast => "proposal.vote_cast",
            Self::ProjectCreated => "project.created",
            Self::ProjectUpdated => "project.updated",
            Self::ProjectDeleted => "project.deleted",
            Self::MemberAdded => "member.added",
            Self::MemberRemoved => "member.removed",
            Self::VetoExercised => "veto.exercised",
            Self::VetoWithdrawn => "veto.withdrawn",
            Self::EscalationStarted => "escalation.started",
            Self::AppealCreated => "appeal.created",
            Self::GovernanceConfigUpdated => "governance_config.updated",
            Self::AiTaskCompleted => "ai.task_completed",
            Self::AiTaskFailed => "ai.task_failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TriggerContext {
    pub event: WebhookEvent,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub actor_id: Uuid,
    pub issue_id: Option<Uuid>,
    pub comment_id: Option<Uuid>,
    pub label_id: Option<Uuid>,
    pub sprint_id: Option<Uuid>,
    pub changes: Option<Value>,
    pub mentions: Vec<Uuid>,
    pub extra_data: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct WebhookPayload {
    pub id: String,
    pub event: String,
    pub timestamp: String,
    pub workspace: PayloadWorkspace,
    pub project: PayloadProject,
    pub actor: PayloadActor,
    pub data: Value,
    pub bot_context: Option<BotContext>,
}

#[derive(Debug, Serialize)]
pub struct PayloadWorkspace {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct PayloadProject {
    pub id: String,
    pub name: String,
    pub key: String,
}

#[derive(Debug, Serialize)]
pub struct PayloadActor {
    pub id: String,
    pub name: String,
    pub email: String,
    pub entity_type: String,
}

#[derive(Debug, Serialize)]
pub struct BotContext {
    pub is_bot_task: bool,
    pub bot_id: String,
    pub bot_name: String,
    pub bot_agent_type: String,
    pub trigger_reason: String,
    pub webhook_id: String,
}

#[derive(Debug, FromQueryResult)]
struct ActiveWebhookRow {
    id: Uuid,
    url: String,
    secret: String,
    bot_user_id: Option<Uuid>,
}

#[derive(Debug, FromQueryResult)]
struct WorkspaceRow {
    id: Uuid,
    name: String,
}

#[derive(Debug, FromQueryResult)]
struct ProjectRow {
    id: Uuid,
    name: String,
    key: String,
}

#[derive(Debug, FromQueryResult)]
struct ActorRow {
    id: Uuid,
    name: String,
    email: String,
    entity_type: String,
}

#[derive(Debug)]
struct BotIdentity {
    bot_id: Uuid,
    bot_name: String,
    bot_agent_type: String,
}

#[derive(Debug)]
struct DeliveryRecord {
    webhook_id: Uuid,
    event: String,
    payload: Value,
    request_headers: Value,
    response_status: Option<i32>,
    response_body: Option<String>,
    error: Option<String>,
    duration_ms: Option<i64>,
    success: bool,
    delivered_at: DateTime<Utc>,
}

pub fn trigger_webhooks(state: AppState, ctx: TriggerContext) {
    tokio::spawn(async move {
        if let Err(err) = trigger_webhooks_inner(state, ctx).await {
            tracing::warn!(error = %err, "webhook trigger failed");
        }
    });
}

async fn trigger_webhooks_inner(state: AppState, ctx: TriggerContext) -> Result<(), sea_orm::DbErr> {
    let webhooks = ActiveWebhookRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, url, secret, bot_user_id
            FROM webhooks
            WHERE workspace_id = $1
              AND active = true
              AND events ? $2
        "#,
        vec![ctx.workspace_id.into(), ctx.event.as_str().into()],
    ))
    .all(&state.db)
    .await?;

    if webhooks.is_empty() {
        return Ok(());
    }

    for webhook in webhooks {
        let delivery_id = Uuid::new_v4();
        let payload = match build_payload(&state, &ctx, delivery_id, &webhook).await {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(webhook_id = %webhook.id, event = ctx.event.as_str(), error = %err, "build payload failed");
                let fallback_payload = json!({
                    "id": delivery_id.to_string(),
                    "event": ctx.event.as_str(),
                    "timestamp": Utc::now().to_rfc3339(),
                });
                let _ = record_delivery(
                    &state,
                    DeliveryRecord {
                        webhook_id: webhook.id,
                        event: ctx.event.as_str().to_string(),
                        payload: fallback_payload,
                        request_headers: json!({}),
                        response_status: None,
                        response_body: None,
                        error: Some(format!("build payload failed: {err}")),
                        duration_ms: None,
                        success: false,
                        delivered_at: Utc::now(),
                    },
                )
                .await;
                continue;
            }
        };

        let _ = deliver_webhook(&state, &webhook, payload).await;
    }

    Ok(())
}

async fn build_payload(
    state: &AppState,
    ctx: &TriggerContext,
    delivery_id: Uuid,
    webhook: &ActiveWebhookRow,
) -> Result<WebhookPayload, sea_orm::DbErr> {
    let workspace = WorkspaceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, name FROM workspaces WHERE id = $1",
        vec![ctx.workspace_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| sea_orm::DbErr::Custom("workspace not found".to_string()))?;

    let project = if ctx.project_id.is_nil() {
        ProjectRow {
            id: Uuid::nil(),
            name: "workspace".to_string(),
            key: "WORKSPACE".to_string(),
        }
    } else {
        ProjectRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, name, key FROM projects WHERE id = $1",
            vec![ctx.project_id.into()],
        ))
        .one(&state.db)
        .await?
        .ok_or_else(|| sea_orm::DbErr::Custom("project not found".to_string()))?
    };

    let actor = ActorRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, name, email, entity_type FROM users WHERE id = $1",
        vec![ctx.actor_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| sea_orm::DbErr::Custom("actor not found".to_string()))?;

    let built_data = build_event_data(state, ctx, &project.key).await?;

    let mut bot_context = None;
    if let Some(identity) =
        check_bot_context(&state.db, &built_data.assignee_ids, webhook.bot_user_id).await
    {
        bot_context = Some(BotContext {
            is_bot_task: true,
            bot_id: identity.bot_id.to_string(),
            bot_name: identity.bot_name,
            bot_agent_type: identity.bot_agent_type,
            trigger_reason: default_trigger_reason(ctx.event).to_string(),
            webhook_id: webhook.id.to_string(),
        });
    } else if matches!(ctx.event, WebhookEvent::CommentCreated) {
        if let Some(identity) = check_bot_mention(&state.db, &ctx.mentions, webhook.bot_user_id).await {
            bot_context = Some(BotContext {
                is_bot_task: true,
                bot_id: identity.bot_id.to_string(),
                bot_name: identity.bot_name,
                bot_agent_type: identity.bot_agent_type,
                trigger_reason: "mentioned".to_string(),
                webhook_id: webhook.id.to_string(),
            });
        }
    }

    Ok(WebhookPayload {
        id: delivery_id.to_string(),
        event: ctx.event.as_str().to_string(),
        timestamp: Utc::now().to_rfc3339(),
        workspace: PayloadWorkspace {
            id: workspace.id.to_string(),
            name: workspace.name,
        },
        project: PayloadProject {
            id: project.id.to_string(),
            name: project.name,
            key: project.key,
        },
        actor: PayloadActor {
            id: actor.id.to_string(),
            name: actor.name,
            email: actor.email,
            entity_type: actor.entity_type,
        },
        data: built_data.data,
        bot_context,
    })
}

struct BuiltEventData {
    data: Value,
    assignee_ids: Vec<Uuid>,
}

async fn build_event_data(
    state: &AppState,
    ctx: &TriggerContext,
    project_key: &str,
) -> Result<BuiltEventData, sea_orm::DbErr> {
    match ctx.event {
        WebhookEvent::IssueCreated
        | WebhookEvent::IssueUpdated
        | WebhookEvent::IssueAssigned
        | WebhookEvent::IssueStateChanged => {
            let (issue, assignee_ids) = if let Some(extra) = ctx.extra_data.clone() {
                let ids = extract_assignee_ids(&extra);
                (extra, ids)
            } else {
                let issue_id = ctx
                    .issue_id
                    .ok_or_else(|| sea_orm::DbErr::Custom("missing issue_id".to_string()))?;
                fetch_issue_payload(state, issue_id, project_key).await?
            };

            let data = if matches!(ctx.event, WebhookEvent::IssueUpdated) {
                json!({
                    "issue": issue,
                    "changes": ctx.changes.clone().unwrap_or_else(|| json!({})),
                })
            } else if matches!(ctx.event, WebhookEvent::IssueAssigned) {
                json!({
                    "issue": issue,
                    "changes": ctx.changes.clone().unwrap_or_else(|| json!({
                        "assignee_ids": {"old": [], "new": []}
                    })),
                })
            } else if matches!(ctx.event, WebhookEvent::IssueStateChanged) {
                json!({
                    "issue": issue,
                    "changes": ctx.changes.clone().unwrap_or_else(|| json!({
                        "state": {"old": null, "new": null}
                    })),
                })
            } else {
                json!({ "issue": issue })
            };

            Ok(BuiltEventData { data, assignee_ids })
        }
        WebhookEvent::IssueDeleted => {
            let issue = ctx
                .extra_data
                .clone()
                .unwrap_or_else(|| json!({ "id": ctx.issue_id.map(|v| v.to_string()) }));
            let assignee_ids = extract_assignee_ids(&issue);
            Ok(BuiltEventData {
                data: json!({ "issue": issue }),
                assignee_ids,
            })
        }
        WebhookEvent::CommentCreated | WebhookEvent::CommentUpdated | WebhookEvent::CommentDeleted => {
            let (comment, issue_summary) = if let Some(extra) = ctx.extra_data.clone() {
                let issue = extra
                    .get("issue")
                    .cloned()
                    .unwrap_or_else(|| json!({ "id": ctx.issue_id.map(|v| v.to_string()) }));
                let comment = extra
                    .get("comment")
                    .cloned()
                    .unwrap_or_else(|| json!({ "id": ctx.comment_id.map(|v| v.to_string()) }));
                (comment, issue)
            } else {
                let comment_id = ctx
                    .comment_id
                    .ok_or_else(|| sea_orm::DbErr::Custom("missing comment_id".to_string()))?;
                fetch_comment_payload(state, comment_id, project_key).await?
            };

            let data = if matches!(ctx.event, WebhookEvent::CommentCreated) {
                json!({
                    "comment": comment,
                    "issue": issue_summary,
                    "mentions": ctx
                        .mentions
                        .iter()
                        .map(Uuid::to_string)
                        .collect::<Vec<_>>(),
                })
            } else {
                json!({
                    "comment": comment,
                    "issue": issue_summary,
                })
            };

            Ok(BuiltEventData {
                data,
                assignee_ids: Vec::new(),
            })
        }
        WebhookEvent::LabelAdded | WebhookEvent::LabelRemoved => {
            let issue_id = ctx
                .issue_id
                .ok_or_else(|| sea_orm::DbErr::Custom("missing issue_id".to_string()))?;
            let label_id = ctx
                .label_id
                .ok_or_else(|| sea_orm::DbErr::Custom("missing label_id".to_string()))?;

            #[derive(Debug, FromQueryResult)]
            struct IssueBrief {
                id: Uuid,
                title: String,
            }

            #[derive(Debug, FromQueryResult)]
            struct LabelBrief {
                id: Uuid,
                name: String,
                color: String,
            }

            let issue = IssueBrief::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id, title FROM work_items WHERE id = $1",
                vec![issue_id.into()],
            ))
            .one(&state.db)
            .await?
            .ok_or_else(|| sea_orm::DbErr::Custom("issue not found".to_string()))?;

            let label = LabelBrief::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id, name, color FROM labels WHERE id = $1",
                vec![label_id.into()],
            ))
            .one(&state.db)
            .await?
            .ok_or_else(|| sea_orm::DbErr::Custom("label not found".to_string()))?;

            Ok(BuiltEventData {
                data: json!({
                    "issue": {
                        "id": issue.id.to_string(),
                        "key": format_issue_key(project_key, issue.id),
                        "title": issue.title,
                    },
                    "label": {
                        "id": label.id.to_string(),
                        "name": label.name,
                        "color": label.color,
                    }
                }),
                assignee_ids: Vec::new(),
            })
        }
        WebhookEvent::SprintStarted | WebhookEvent::SprintCompleted => {
            let sprint_id = ctx
                .sprint_id
                .ok_or_else(|| sea_orm::DbErr::Custom("missing sprint_id".to_string()))?;

            #[derive(Debug, FromQueryResult)]
            struct SprintRow {
                id: Uuid,
                name: String,
                description: String,
                status: String,
                start_date: Option<chrono::NaiveDate>,
                end_date: Option<chrono::NaiveDate>,
                created_at: DateTime<Utc>,
                updated_at: DateTime<Utc>,
            }

            let sprint = SprintRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id, name, description, status, start_date, end_date, created_at, updated_at FROM sprints WHERE id = $1",
                vec![sprint_id.into()],
            ))
            .one(&state.db)
            .await?
            .ok_or_else(|| sea_orm::DbErr::Custom("sprint not found".to_string()))?;

            Ok(BuiltEventData {
                data: json!({
                    "sprint": {
                        "id": sprint.id.to_string(),
                        "name": sprint.name,
                        "description": sprint.description,
                        "status": sprint.status,
                        "start_date": sprint.start_date.map(|v| v.to_string()),
                        "end_date": sprint.end_date.map(|v| v.to_string()),
                        "created_at": sprint.created_at.to_rfc3339(),
                        "updated_at": sprint.updated_at.to_rfc3339(),
                    }
                }),
                assignee_ids: Vec::new(),
            })
        }
        WebhookEvent::ProposalCreated
        | WebhookEvent::ProposalUpdated
        | WebhookEvent::ProposalDeleted
        | WebhookEvent::ProposalSubmitted
        | WebhookEvent::ProposalVotingStarted
        | WebhookEvent::ProposalArchived
        | WebhookEvent::ProposalVoteCast
        | WebhookEvent::ProjectCreated
        | WebhookEvent::ProjectUpdated
        | WebhookEvent::ProjectDeleted
        | WebhookEvent::MemberAdded
        | WebhookEvent::MemberRemoved
        | WebhookEvent::VetoExercised
        | WebhookEvent::VetoWithdrawn
        | WebhookEvent::EscalationStarted
        | WebhookEvent::AppealCreated
        | WebhookEvent::GovernanceConfigUpdated
        | WebhookEvent::AiTaskCompleted
        | WebhookEvent::AiTaskFailed => Ok(BuiltEventData {
            data: ctx.extra_data.clone().unwrap_or_else(|| json!({})),
            assignee_ids: Vec::new(),
        }),
    }
}

async fn fetch_issue_payload(
    state: &AppState,
    issue_id: Uuid,
    project_key: &str,
) -> Result<(Value, Vec<Uuid>), sea_orm::DbErr> {
    #[derive(Debug, FromQueryResult)]
    struct IssueRow {
        id: Uuid,
        title: String,
        description: String,
        state: String,
        priority: String,
        assignee_id: Option<Uuid>,
        sprint_id: Option<Uuid>,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    }

    #[derive(Debug, FromQueryResult)]
    struct LabelIdRow {
        label_id: Uuid,
    }

    let issue = IssueRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, title, description, state, priority, assignee_id, sprint_id, created_at, updated_at
            FROM work_items
            WHERE id = $1
        "#,
        vec![issue_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| sea_orm::DbErr::Custom("issue not found".to_string()))?;

    let label_rows = LabelIdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT label_id FROM work_item_labels WHERE work_item_id = $1 ORDER BY label_id",
        vec![issue_id.into()],
    ))
    .all(&state.db)
    .await?;

    let label_ids = label_rows
        .into_iter()
        .map(|row| row.label_id.to_string())
        .collect::<Vec<_>>();

    let assignee_ids = issue
        .assignee_id
        .map(|v| vec![v])
        .unwrap_or_default();

    let assignee_id_values = assignee_ids
        .iter()
        .map(Uuid::to_string)
        .collect::<Vec<_>>();

    Ok((
        json!({
            "id": issue.id.to_string(),
            "key": format_issue_key(project_key, issue.id),
            "title": issue.title,
            "description": issue.description,
            "state": issue.state,
            "priority": issue.priority,
            "assignee_ids": assignee_id_values,
            "label_ids": label_ids,
            "sprint_id": issue.sprint_id.map(|v| v.to_string()),
            "created_at": issue.created_at.to_rfc3339(),
            "updated_at": issue.updated_at.to_rfc3339(),
        }),
        assignee_ids,
    ))
}

async fn fetch_comment_payload(
    state: &AppState,
    comment_id: Uuid,
    project_key: &str,
) -> Result<(Value, Value), sea_orm::DbErr> {
    #[derive(Debug, FromQueryResult)]
    struct CommentRow {
        id: Uuid,
        issue_id: Uuid,
        content: String,
        author_id: Option<Uuid>,
        author_name: Option<String>,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    }

    #[derive(Debug, FromQueryResult)]
    struct IssueRow {
        id: Uuid,
        title: String,
    }

    let comment = CommentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT c.id,
                   c.work_item_id AS issue_id,
                   c.body AS content,
                   c.author_id,
                   u.name AS author_name,
                   c.created_at,
                   c.updated_at
            FROM comments c
            LEFT JOIN users u ON c.author_id = u.id
            WHERE c.id = $1
        "#,
        vec![comment_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| sea_orm::DbErr::Custom("comment not found".to_string()))?;

    let issue = IssueRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, title FROM work_items WHERE id = $1",
        vec![comment.issue_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| sea_orm::DbErr::Custom("issue not found".to_string()))?;

    Ok((
        json!({
            "id": comment.id.to_string(),
            "issue_id": comment.issue_id.to_string(),
            "content": comment.content,
            "author_id": comment.author_id.map(|v| v.to_string()),
            "author_name": comment.author_name.unwrap_or_default(),
            "created_at": comment.created_at.to_rfc3339(),
            "updated_at": comment.updated_at.to_rfc3339(),
        }),
        json!({
            "id": issue.id.to_string(),
            "key": format_issue_key(project_key, issue.id),
            "title": issue.title,
        }),
    ))
}

async fn check_bot_context(
    db: &impl ConnectionTrait,
    assignee_ids: &[Uuid],
    webhook_bot_user_id: Option<Uuid>,
) -> Option<BotIdentity> {
    let webhook_bot_user_id = webhook_bot_user_id?;

    for assignee_id in assignee_ids {
        if *assignee_id != webhook_bot_user_id {
            continue;
        }

        let row = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT entity_type, agent_type, name FROM users WHERE id = $1",
                vec![(*assignee_id).into()],
            ))
            .await
            .ok()??;

        let entity_type: Option<String> = row.try_get("", "entity_type").ok();
        if entity_type.as_deref() != Some("bot") {
            continue;
        }

        let bot_name: String = row
            .try_get::<String>("", "name")
            .unwrap_or_else(|_| "Bot".to_string());
        let bot_agent_type = row
            .try_get::<Option<String>>("", "agent_type")
            .ok()
            .flatten()
            .unwrap_or_else(|| "custom".to_string());

        return Some(BotIdentity {
            bot_id: *assignee_id,
            bot_name,
            bot_agent_type,
        });
    }

    None
}

async fn check_bot_mention(
    db: &impl ConnectionTrait,
    mention_ids: &[Uuid],
    webhook_bot_user_id: Option<Uuid>,
) -> Option<BotIdentity> {
    let bot_id = webhook_bot_user_id?;
    if !mention_ids.contains(&bot_id) {
        return None;
    }

    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT entity_type, agent_type, name FROM users WHERE id = $1",
            vec![bot_id.into()],
        ))
        .await
        .ok()??;

    let entity_type: Option<String> = row.try_get("", "entity_type").ok();
    if entity_type.as_deref() != Some("bot") {
        return None;
    }

    let bot_name: String = row
        .try_get::<String>("", "name")
        .unwrap_or_else(|_| "Bot".to_string());
    let bot_agent_type = row
        .try_get::<Option<String>>("", "agent_type")
        .ok()
        .flatten()
        .unwrap_or_else(|| "custom".to_string());

    Some(BotIdentity {
        bot_id,
        bot_name,
        bot_agent_type,
    })
}

async fn deliver_webhook(
    state: &AppState,
    webhook: &ActiveWebhookRow,
    payload: WebhookPayload,
) -> Result<(), sea_orm::DbErr> {
    let body = serde_json::to_string(&payload)
        .map_err(|e| sea_orm::DbErr::Custom(format!("serialize payload failed: {e}")))?;

    let signature = sign_payload(&webhook.secret, &body)
        .map_err(|e| sea_orm::DbErr::Custom(format!("sign payload failed: {e}")))?;

    let delivery_id = payload.id.clone();
    let event = payload.event.clone();

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static("OpenPR-Webhook/1.0"),
    );
    headers.insert(
        "X-Webhook-Signature",
        HeaderValue::from_str(&format!("sha256={signature}")).unwrap_or(HeaderValue::from_static("sha256=")),
    );
    headers.insert(
        "X-Webhook-Event",
        HeaderValue::from_str(&event).unwrap_or(HeaderValue::from_static("unknown")),
    );
    headers.insert(
        "X-Webhook-Delivery",
        HeaderValue::from_str(&delivery_id).unwrap_or(HeaderValue::from_static("unknown")),
    );

    let request_headers = json!({
        "Content-Type": "application/json",
        "User-Agent": "OpenPR-Webhook/1.0",
        "X-Webhook-Signature": format!("sha256={signature}"),
        "X-Webhook-Event": event,
        "X-Webhook-Delivery": delivery_id,
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| sea_orm::DbErr::Custom(format!("build reqwest client failed: {e}")))?;

    let started = Instant::now();
    let sent = client
        .post(&webhook.url)
        .headers(headers)
        .body(body)
        .send()
        .await;

    let (response_status, response_body, error, success) = match sent {
        Ok(resp) => {
            let status = resp.status().as_u16() as i32;
            let success = resp.status().is_success();
            let text = resp.text().await.ok();
            (Some(status), text, None, success)
        }
        Err(err) => (None, None, Some(err.to_string()), false),
    };

    let duration_ms = started.elapsed().as_millis() as i64;
    let payload_json = serde_json::to_value(&payload)
        .map_err(|e| sea_orm::DbErr::Custom(format!("serialize payload json failed: {e}")))?;

    let _ = record_delivery(
        state,
        DeliveryRecord {
            webhook_id: webhook.id,
            event: payload.event.clone(),
            payload: payload_json,
            request_headers,
            response_status,
            response_body,
            error,
            duration_ms: Some(duration_ms),
            success,
            delivered_at: Utc::now(),
        },
    )
    .await;

    let _ = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webhooks SET last_triggered_at = $1 WHERE id = $2",
            vec![Utc::now().into(), webhook.id.into()],
        ))
        .await;

    Ok(())
}

async fn record_delivery(state: &AppState, rec: DeliveryRecord) -> Result<(), sea_orm::DbErr> {
    let column_rows = state
        .db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT column_name
                FROM information_schema.columns
                WHERE table_schema = 'public'
                  AND table_name = 'webhook_deliveries'
            "#,
            vec![],
        ))
        .await?;

    let mut columns = HashSet::new();
    for row in column_rows {
        if let Ok(col) = row.try_get::<String>("", "column_name") {
            columns.insert(col);
        }
    }

    let mut insert_cols = Vec::<String>::new();
    let mut params = Vec::<sea_orm::Value>::new();

    let mut push = |name: &str, value: sea_orm::Value| {
        insert_cols.push(name.to_string());
        params.push(value);
    };

    if columns.contains("id") {
        push("id", Uuid::new_v4().into());
    }
    if columns.contains("webhook_id") {
        push("webhook_id", rec.webhook_id.into());
    }
    if columns.contains("event") {
        push("event", rec.event.clone().into());
    } else if columns.contains("event_type") {
        push("event_type", rec.event.into());
    }
    if columns.contains("payload") {
        push("payload", rec.payload.into());
    }
    if columns.contains("request_headers") {
        push("request_headers", rec.request_headers.into());
    }
    if columns.contains("response_status") {
        push("response_status", rec.response_status.into());
    }
    if columns.contains("response_body") {
        push("response_body", rec.response_body.into());
    }
    if columns.contains("error") {
        push("error", rec.error.into());
    }
    if columns.contains("duration_ms") {
        push("duration_ms", rec.duration_ms.into());
    }
    if columns.contains("success") {
        push("success", rec.success.into());
    }
    if columns.contains("delivered_at") {
        push("delivered_at", rec.delivered_at.into());
    }
    if columns.contains("created_at") {
        push("created_at", Utc::now().into());
    }
    if columns.contains("retry_count") {
        push("retry_count", 0_i32.into());
    }

    if insert_cols.is_empty() {
        return Ok(());
    }

    let placeholders = (1..=insert_cols.len())
        .map(|idx| format!("${idx}"))
        .collect::<Vec<_>>()
        .join(", ");

    let sql = format!(
        "INSERT INTO webhook_deliveries ({}) VALUES ({})",
        insert_cols.join(", "),
        placeholders
    );

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            &sql,
            params,
        ))
        .await?;

    Ok(())
}

fn sign_payload(secret: &str, raw_body: &str) -> Result<String, String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|e| format!("invalid hmac secret: {e}"))?;
    mac.update(raw_body.as_bytes());
    let bytes = mac.finalize().into_bytes();
    Ok(hex::encode(bytes))
}

fn format_issue_key(project_key: &str, issue_id: Uuid) -> String {
    let id = issue_id.simple().to_string().to_uppercase();
    let short = &id[..8];
    format!("{}-{}", project_key, short)
}

fn extract_assignee_ids(issue_payload: &Value) -> Vec<Uuid> {
    issue_payload
        .get("assignee_ids")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .filter_map(|raw| Uuid::parse_str(raw).ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn default_trigger_reason(event: WebhookEvent) -> &'static str {
    match event {
        WebhookEvent::IssueCreated => "created",
        WebhookEvent::IssueAssigned | WebhookEvent::IssueUpdated => "assigned",
        WebhookEvent::IssueStateChanged => "status_changed",
        WebhookEvent::CommentCreated => "mentioned",
        WebhookEvent::AiTaskCompleted => "completed",
        WebhookEvent::AiTaskFailed => "failed",
        _ => "assigned",
    }
}
