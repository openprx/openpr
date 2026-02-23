use platform::app::AppState;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, FromQueryResult)]
pub struct ProposalInfo {
    pub id: String,
    pub title: String,
    pub proposal_type: String,
    pub status: String,
    pub author_id: String,
    pub author_type: String,
    pub content: String,
    pub created_at: String,
}

pub async fn list_proposals(
    state: &AppState,
    project_id: uuid::Uuid,
    status_filter: Option<String>,
) -> Result<Vec<ProposalInfo>, String> {
    let rows = if let Some(status) = status_filter {
        state
            .db
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                    SELECT DISTINCT
                        p.id,
                        p.title,
                        p.proposal_type::text AS proposal_type,
                        p.status::text AS status,
                        p.author_id,
                        p.author_type::text AS author_type,
                        p.content,
                        p.created_at::text
                    FROM proposals p
                    LEFT JOIN proposal_issue_links pil ON pil.proposal_id = p.id
                    LEFT JOIN work_items wi ON wi.id = pil.issue_id
                    WHERE (wi.project_id = $1 OR p.author_id = $1::text)
                      AND p.status::text = $2
                    ORDER BY p.created_at DESC
                "#,
                vec![project_id.into(), status.into()],
            ))
            .await
    } else {
        state
            .db
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"
                    SELECT DISTINCT
                        p.id,
                        p.title,
                        p.proposal_type::text AS proposal_type,
                        p.status::text AS status,
                        p.author_id,
                        p.author_type::text AS author_type,
                        p.content,
                        p.created_at::text
                    FROM proposals p
                    LEFT JOIN proposal_issue_links pil ON pil.proposal_id = p.id
                    LEFT JOIN work_items wi ON wi.id = pil.issue_id
                    WHERE (wi.project_id = $1 OR p.author_id = $1::text)
                    ORDER BY p.created_at DESC
                "#,
                vec![project_id.into()],
            ))
            .await
    }
    .map_err(|e| format!("Database error: {}", e))?;

    let proposals = rows
        .into_iter()
        .map(|row| ProposalInfo::from_query_result(&row, "").map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(proposals)
}

pub async fn get_proposal(
    state: &AppState,
    proposal_id: String,
) -> Result<Option<ProposalInfo>, String> {
    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT
                    id,
                    title,
                    proposal_type::text AS proposal_type,
                    status::text AS status,
                    author_id,
                    author_type::text AS author_type,
                    content,
                    created_at::text
                FROM proposals
                WHERE id = $1
            "#,
            vec![proposal_id.into()],
        ))
        .await
        .map_err(|e| format!("Database error: {}", e))?;

    match row {
        Some(r) => Ok(Some(
            ProposalInfo::from_query_result(&r, "").map_err(|e| e.to_string())?,
        )),
        None => Ok(None),
    }
}

pub async fn create_proposal(
    state: &AppState,
    project_id: uuid::Uuid,
    title: String,
    description: String,
    author_id: Option<uuid::Uuid>,
) -> Result<ProposalInfo, String> {
    let proposal_id = format!("PROP-{}", uuid::Uuid::new_v4().simple());
    let now = chrono::Utc::now();
    let resolved_author_id = author_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| project_id.to_string());

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO proposals (
                    id,
                    title,
                    proposal_type,
                    status,
                    author_id,
                    author_type,
                    content,
                    domains,
                    voting_rule,
                    cycle_template,
                    created_at
                ) VALUES (
                    $1,
                    $2,
                    'feature'::proposal_type,
                    'draft'::proposal_status,
                    $3,
                    'human'::author_type,
                    $4,
                    '[]'::jsonb,
                    'simple_majority'::voting_rule,
                    'fast'::cycle_template,
                    $5
                )
            "#,
            vec![
                proposal_id.clone().into(),
                title.clone().into(),
                resolved_author_id.clone().into(),
                description.clone().into(),
                now.into(),
            ],
        ))
        .await
        .map_err(|e| format!("Failed to create proposal: {}", e))?;

    Ok(ProposalInfo {
        id: proposal_id,
        title,
        proposal_type: "feature".to_string(),
        status: "draft".to_string(),
        author_id: resolved_author_id,
        author_type: "human".to_string(),
        content: description,
        created_at: now.to_rfc3339(),
    })
}
