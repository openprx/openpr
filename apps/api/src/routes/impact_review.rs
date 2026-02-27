use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    entities::impact_review::{ReviewRating, ReviewStatus},
    error::ApiError,
    response::{ApiResponse, PaginatedData},
    services::{
        impact_review_service::{ImpactReviewService, ReviewSummary},
        trust_score_service::{is_project_member, is_system_admin},
    },
};

#[derive(Debug, Deserialize)]
pub struct CreateImpactReviewRequest {
    pub reviewer_id: Option<Uuid>,
    pub scheduled_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateImpactReviewRequest {
    pub status: Option<ReviewStatus>,
    pub rating: Option<ReviewRating>,
    pub metrics: Option<Value>,
    pub goal_achievements: Option<Value>,
    pub achievements: Option<String>,
    pub lessons: Option<String>,
    pub data_sources: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct ListImpactReviewsQuery {
    pub project_id: Option<Uuid>,
    pub status: Option<ReviewStatus>,
    pub rating: Option<ReviewRating>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct UpsertParticipantRequest {
    pub user_id: String,
    pub role: String,
    pub vote_choice: Option<String>,
    pub exercised_veto: Option<bool>,
    pub veto_overturned: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateParticipantRequest {
    pub role: Option<String>,
    pub vote_choice: Option<String>,
    pub feedback_submitted: Option<bool>,
    pub feedback_content: Option<String>,
    pub exercised_veto: Option<bool>,
    pub veto_overturned: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ImpactReviewDetailResponse {
    review: crate::entities::impact_review::Model,
    participants: Vec<crate::entities::review_participant::Model>,
    summary: ReviewSummary,
}

pub async fn create_impact_review(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(proposal_id): Path<String>,
    Json(req): Json<CreateImpactReviewRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let svc = ImpactReviewService::new(state.db.clone());
    let reviewer_id = parse_claim_user_id(&claims)?;
    let project_id = svc.get_project_id_for_proposal(&proposal_id).await?;
    ensure_project_member_or_admin(&state, project_id, reviewer_id).await?;

    let review = svc
        .create_review(
            &proposal_id,
            req.reviewer_id.or(Some(reviewer_id)),
            req.scheduled_at,
        )
        .await?;

    Ok(ApiResponse::success(review))
}

pub async fn get_impact_review_by_proposal(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(proposal_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let viewer_id = parse_claim_user_id(&claims)?;

    let svc = ImpactReviewService::new(state.db.clone());
    let review = svc
        .get_by_proposal(&proposal_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("impact review not found".to_string()))?;

    ensure_project_member_or_admin(&state, review.project_id, viewer_id).await?;

    let participants = svc.get_participants(&review.id).await?;
    let summary = svc.summarize(&review.id).await?;

    Ok(ApiResponse::success(ImpactReviewDetailResponse {
        review,
        participants,
        summary,
    }))
}

pub async fn update_impact_review_by_proposal(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(proposal_id): Path<String>,
    Json(req): Json<UpdateImpactReviewRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let reviewer_id = parse_claim_user_id(&claims)?;

    let svc = ImpactReviewService::new(state.db.clone());
    let project_id = svc.get_project_id_for_proposal(&proposal_id).await?;
    ensure_project_member_or_admin(&state, project_id, reviewer_id).await?;

    let updated = svc
        .complete_review(
            &proposal_id,
            reviewer_id,
            req.rating,
            req.goal_achievements,
            req.achievements,
            req.lessons,
            req.metrics,
            req.status,
            req.data_sources,
        )
        .await?;

    let participants = svc.get_participants(&updated.id).await?;
    let summary = svc.summarize(&updated.id).await?;

    Ok(ApiResponse::success(ImpactReviewDetailResponse {
        review: updated,
        participants,
        summary,
    }))
}

pub async fn delete_impact_review_by_proposal(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(proposal_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_claim_user_id(&claims)?;
    let svc = ImpactReviewService::new(state.db.clone());
    let project_id = svc.get_project_id_for_proposal(&proposal_id).await?;
    ensure_project_member_or_admin(&state, project_id, user_id).await?;
    svc.delete_review(&proposal_id).await?;
    Ok(ApiResponse::ok())
}

pub async fn list_impact_reviews(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Query(query): Query<ListImpactReviewsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let viewer_id = parse_claim_user_id(&claims)?;

    if let Some(project_id) = query.project_id {
        ensure_project_member_or_admin(&state, project_id, viewer_id).await?;
    } else if !is_system_admin(&state.db, viewer_id).await? {
        return Err(ApiError::Forbidden(
            "admin access required for global impact reviews".to_string(),
        ));
    }

    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);

    let svc = ImpactReviewService::new(state.db.clone());
    let (items, total) = svc
        .list_reviews(query.project_id, query.status, query.rating, page, per_page)
        .await?;

    let total_pages = if total == 0 {
        0
    } else {
        (total + per_page - 1) / per_page
    };

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages,
    }))
}

pub async fn list_review_participants(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(review_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let viewer_id = parse_claim_user_id(&claims)?;
    let svc = ImpactReviewService::new(state.db.clone());
    let project_id = svc.get_project_id_for_review(&review_id).await?;
    ensure_project_member_or_admin(&state, project_id, viewer_id).await?;
    let participants = svc.get_participants(&review_id).await?;
    let summary = svc.summarize(&review_id).await?;
    Ok(ApiResponse::success(json!({
        "participants": participants,
        "summary": summary,
    })))
}

pub async fn upsert_review_participant(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(review_id): Path<String>,
    Json(req): Json<UpsertParticipantRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_claim_user_id(&claims)?;
    let svc = ImpactReviewService::new(state.db.clone());
    let project_id = svc.get_project_id_for_review(&review_id).await?;
    ensure_project_member_or_admin(&state, project_id, user_id).await?;
    let participant = svc
        .upsert_participant(
            &review_id,
            req.user_id,
            &req.role,
            req.vote_choice,
            req.exercised_veto.unwrap_or(false),
            req.veto_overturned.unwrap_or(false),
        )
        .await?;

    Ok(ApiResponse::success(participant))
}

pub async fn update_review_participant(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((review_id, user_id)): Path<(String, String)>,
    Json(req): Json<UpdateParticipantRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let actor_id = parse_claim_user_id(&claims)?;
    let svc = ImpactReviewService::new(state.db.clone());
    let project_id = svc.get_project_id_for_review(&review_id).await?;
    ensure_project_member_or_admin(&state, project_id, actor_id).await?;
    let participant = svc
        .update_participant_feedback(
            &review_id,
            user_id,
            req.feedback_submitted,
            req.feedback_content,
            req.role,
            req.vote_choice,
            req.exercised_veto,
            req.veto_overturned,
        )
        .await?;

    Ok(ApiResponse::success(participant))
}

pub async fn delete_review_participant(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((review_id, user_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let actor_id = parse_claim_user_id(&claims)?;
    let svc = ImpactReviewService::new(state.db.clone());
    let project_id = svc.get_project_id_for_review(&review_id).await?;
    ensure_project_member_or_admin(&state, project_id, actor_id).await?;
    svc.remove_participant(&review_id, user_id).await?;
    Ok(ApiResponse::ok())
}

fn parse_claim_user_id(claims: &JwtClaims) -> Result<Uuid, ApiError> {
    Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))
}

async fn ensure_project_member_or_admin(
    state: &AppState,
    project_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let allowed = is_project_member(&state.db, project_id, user_id).await?
        || is_system_admin(&state.db, user_id).await?;
    if !allowed {
        return Err(ApiError::Forbidden("project access denied".to_string()));
    }
    Ok(())
}
