use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use platform::app::AppState;
use sea_orm::{DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};

use crate::{
    error::ApiError,
    response::{ApiResponse, PaginatedData},
};

#[derive(Debug, Serialize, FromQueryResult)]
pub struct DecisionRow {
    pub id: String,
    pub proposal_id: String,
    pub result: String,
    pub approval_rate: Option<f64>,
    pub total_votes: i32,
    pub yes_votes: i32,
    pub no_votes: i32,
    pub abstain_votes: i32,
    pub decided_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct ListDecisionsQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    count: i64,
}

pub async fn list_decisions(
    State(state): State<AppState>,
    Query(query): Query<ListDecisionsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;

    let total = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COUNT(*)::bigint AS count FROM decisions",
        vec![],
    ))
    .one(&state.db)
    .await?
    .map(|r| r.count)
    .unwrap_or(0);

    let rows = DecisionRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, proposal_id, result::text AS result, approval_rate,
                   total_votes, yes_votes, no_votes, abstain_votes, decided_at
            FROM decisions
            ORDER BY decided_at DESC
            LIMIT $1 OFFSET $2
        "#,
        vec![per_page.into(), offset.into()],
    ))
    .all(&state.db)
    .await?;

    let total_pages = if total == 0 {
        1
    } else {
        ((total as f64) / (per_page as f64)).ceil() as i64
    };

    Ok(ApiResponse::success(PaginatedData {
        items: rows,
        total,
        page,
        per_page,
        total_pages,
    }))
}

pub async fn get_decision(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let decision = DecisionRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, proposal_id, result::text AS result, approval_rate,
                   total_votes, yes_votes, no_votes, abstain_votes, decided_at
            FROM decisions
            WHERE id = $1
        "#,
        vec![id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("decision not found".to_string()))?;

    Ok(ApiResponse::success(decision))
}

pub async fn get_proposal_decision(
    State(state): State<AppState>,
    Path(proposal_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let decision = DecisionRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT id, proposal_id, result::text AS result, approval_rate,
                   total_votes, yes_votes, no_votes, abstain_votes, decided_at
            FROM decisions
            WHERE proposal_id = $1
        "#,
        vec![proposal_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("decision not found".to_string()))?;

    Ok(ApiResponse::success(decision))
}
