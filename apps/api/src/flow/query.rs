//! Read paths: `GET .../flow/objects`, `GET .../flow/objects/{id}` and
//! `GET .../flow/objects/{id}/history`.
//!
//! Every read here comes from `flow_objects` + `collab_documents` + `flow_object_projections`
//! (or, for history, `collab_updates` + `business_events`); nothing decodes a CRDT snapshot on the
//! read path.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
use chrono::{DateTime, Utc};
use platform::app::AppState;
use serde_json::json;
use uuid::Uuid;

use crate::error::ApiError;

use super::model::{FlowFeatureView, FlowObjectListResponse, FlowObjectView, HistoryItem, HistoryResponse};
use super::projection;
use super::repository::{self, FlowSettingsRow, HistoryFilter, ListFilter, ObjectViewRow};

pub const DEFAULT_LIST_LIMIT: u64 = 50;
pub const MAX_LIST_LIMIT: u64 = 100;

/// Clamps a caller-supplied `limit` query parameter.
///
/// Rejects anything outside `1..=100` rather than silently clamping it — `limits-v1.md`'s
/// boundary-testing convention throughout this contract is exact-accepted / one-past-rejected,
/// not silent clamping.
pub fn validate_limit(limit: Option<u64>) -> Result<u64, ApiError> {
    match limit {
        None => Ok(DEFAULT_LIST_LIMIT),
        Some(0) => Err(ApiError::BadRequest("limit must be at least 1".to_string())),
        Some(value) if value > MAX_LIST_LIMIT => {
            Err(ApiError::BadRequest(format!("limit must be at most {MAX_LIST_LIMIT}")))
        }
        Some(value) => Ok(value),
    }
}

pub fn object_view_from_row(row: ObjectViewRow) -> FlowObjectView {
    let projection_lag = row.document_seq - row.projection_document_seq;
    FlowObjectView {
        id: row.id,
        workspace_id: row.workspace_id,
        project_id: row.project_id,
        parent_id: row.parent_id,
        object_type: row.object_type,
        lifecycle_status: row.lifecycle_status,
        governance_metadata: row.governance_metadata,
        title: row.projection_title,
        semantic_content: row.projection_state,
        document_id: row.document_id,
        document_seq: row.document_seq,
        frontier: base64::engine::general_purpose::STANDARD.encode(&row.document_frontier),
        projection_seq: row.projection_document_seq,
        projection_lag,
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
        archived_at: row.archived_at.map(|t| t.to_rfc3339()),
    }
}

/// `render` query parameter values (`rest-api-v1.md`: `render=semantic_json|markdown`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Render {
    SemanticJson,
    Markdown,
}

impl Render {
    pub fn parse(raw: Option<&str>) -> Result<Self, ApiError> {
        match raw {
            None | Some("semantic_json") => Ok(Self::SemanticJson),
            Some("markdown") => Ok(Self::Markdown),
            Some(other) => Err(ApiError::BadRequest(format!(
                "render must be semantic_json or markdown, got '{other}'"
            ))),
        }
    }
}

pub async fn get_object(
    state: &AppState,
    object_id: Uuid,
    at_seq: Option<i64>,
    render: Render,
) -> Result<FlowObjectView, ApiError> {
    let row = repository::fetch_object_view(&state.db, object_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;

    // This package ships no content commands, so `document_seq` never advances past 0 for any
    // object it creates; `at_seq` can only ever be satisfied at the current head. A mismatch is
    // `invalid_update` rather than silently serving the current state under a different seq.
    if let Some(requested) = at_seq
        && requested != row.document_seq
    {
        return Err(ApiError::BadRequest(format!(
            "at_seq {requested} is out of range; this object's only available seq is {}",
            row.document_seq
        )));
    }

    let mut view = object_view_from_row(row);
    if render == Render::Markdown {
        view.semantic_content = json!({ "rendered": projection::render_markdown(&view.title) });
    }
    Ok(view)
}

/// Parameters for [`list_objects`], bundled into one struct so the handler-facing signature does
/// not carry ten positional arguments for what is one HTTP query string.
pub struct ListObjectsParams {
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub unprojected: bool,
    pub object_type: Option<String>,
    pub parent_id: Option<Uuid>,
    pub q: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u64>,
    pub include_archived: bool,
}

pub async fn list_objects(state: &AppState, params: ListObjectsParams) -> Result<FlowObjectListResponse, ApiError> {
    if params.project_id.is_some() && params.unprojected {
        return Err(ApiError::BadRequest(
            "project_id and unprojected=true are mutually exclusive".to_string(),
        ));
    }
    let limit = validate_limit(params.limit)?;
    let after = params.cursor.as_deref().map(decode_cursor).transpose()?;
    let limit_usize = usize::try_from(limit).unwrap_or(usize::MAX);

    let filter = ListFilter {
        workspace_id: params.workspace_id,
        project_id: params.project_id,
        unprojected: params.unprojected,
        object_type: params.object_type,
        parent_id: params.parent_id,
        title_prefix: params.q,
        include_archived: params.include_archived,
        after,
        // Fetch one extra row to know whether a further page exists without a second query.
        limit: limit + 1,
    };
    let mut rows = repository::list_objects(&state.db, &filter).await?;

    let next_cursor = if rows.len() > limit_usize {
        rows.truncate(limit_usize);
        rows.last().map(|row| encode_cursor(row.created_at, row.id))
    } else {
        None
    };

    Ok(FlowObjectListResponse {
        items: rows.into_iter().map(object_view_from_row).collect(),
        next_cursor,
    })
}

pub async fn get_history(
    state: &AppState,
    object_id: Uuid,
    before_seq: Option<i64>,
    limit: Option<u64>,
) -> Result<HistoryResponse, ApiError> {
    let row = repository::fetch_object_view(&state.db, object_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    let limit = validate_limit(limit)?;

    let filter = HistoryFilter {
        document_id: row.document_id,
        before_seq,
        limit: limit + 1,
    };
    let mut rows = repository::fetch_history(&state.db, &filter).await?;

    let limit_usize = usize::try_from(limit).unwrap_or(usize::MAX);
    let next_before_seq = if rows.len() > limit_usize {
        rows.truncate(limit_usize);
        rows.last().map(|row| row.seq)
    } else {
        None
    };

    Ok(HistoryResponse {
        items: rows
            .into_iter()
            .map(|row| HistoryItem {
                seq: row.seq,
                actor: row.actor_id,
                origin: row.origin_surface,
                message: row.message,
                semantic_summary: row.semantic_summary,
                created_at: row.created_at.to_rfc3339(),
            })
            .collect(),
        next_before_seq,
    })
}

/// Row-to-wire mapping shared by the `GET` handler and `command::set_flow_feature`'s response.
///
/// `None` (never-provisioned workspace) maps to the column defaults with `updated_at`/`updated_by`
/// left `null` (see [`FlowFeatureView`]'s doc comment).
pub fn feature_view_from_row(row: Option<FlowSettingsRow>) -> FlowFeatureView {
    row.map_or_else(
        || FlowFeatureView {
            flow_enabled: false,
            default_member_level: "edit".to_string(),
            authz_epoch: 0,
            updated_at: None,
            updated_by: None,
        },
        |row| FlowFeatureView {
            flow_enabled: row.flow_enabled,
            default_member_level: row.default_member_level,
            authz_epoch: row.authz_epoch,
            updated_at: Some(row.updated_at.to_rfc3339()),
            updated_by: row.updated_by,
        },
    )
}

/// `GET /api/v1/workspaces/{workspace_id}/features/flow`.
pub async fn get_flow_feature(state: &AppState, workspace_id: Uuid) -> Result<FlowFeatureView, ApiError> {
    let row = repository::fetch_flow_settings(&state.db, workspace_id).await?;
    Ok(feature_view_from_row(row))
}

fn encode_cursor(created_at: DateTime<Utc>, id: Uuid) -> String {
    BASE64_URL.encode(format!("{}|{id}", created_at.to_rfc3339()))
}

fn decode_cursor(raw: &str) -> Result<(DateTime<Utc>, Uuid), ApiError> {
    let bytes = BASE64_URL
        .decode(raw)
        .map_err(|_| ApiError::BadRequest("cursor is not valid".to_string()))?;
    let text = String::from_utf8(bytes).map_err(|_| ApiError::BadRequest("cursor is not valid".to_string()))?;
    let (created_at_raw, id_raw) = text
        .split_once('|')
        .ok_or_else(|| ApiError::BadRequest("cursor is not valid".to_string()))?;
    let created_at = DateTime::parse_from_rfc3339(created_at_raw)
        .map_err(|_| ApiError::BadRequest("cursor is not valid".to_string()))?
        .with_timezone(&Utc);
    let id = Uuid::parse_str(id_raw).map_err(|_| ApiError::BadRequest("cursor is not valid".to_string()))?;
    Ok((created_at, id))
}
