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

use super::collab::bootstrap;
use super::collab::frame::TailUpdate;
use super::collab::limits;
use super::model::{Bootstrap, FlowFeatureView, FlowObjectListResponse, FlowObjectView, HistoryItem, HistoryResponse};
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

/// `GET /api/v1/flow/objects/{object_id}/bootstrap` (`rest-api-v1.md`: "**user only**；object
/// read/write；flag").
///
/// `known_seq`/`known_frontier` are accepted and shape-validated (a malformed `known_frontier` is
/// `invalid_update`) but do not change the response: this endpoint always returns the *current*
/// `snapshot` + its exact `(snapshot_seq,head_seq]` tail, never a delta computed from the
/// caller's `known_seq`. Two different things are true here and must not be conflated: v0.4
/// **does** advance the snapshot pointer on the write path (`flow::collab::snapshot`, gate 7
/// `minimal_snapshot_advancement_bounds_tail`) precisely to keep that tail bounded by
/// `limits-v1.md`'s soft/hard triggers — but it still keeps every accepted `collab_updates` row
/// forever (no `DELETE`/retention compaction before v0.8, `versions/v0.4-flow-alpha.md:29`), and
/// it still has no "resume from partial tail" computed from `known_seq` either way — exactly the
/// same no-op treatment the WebSocket `Open` frame's identical fields already get in
/// `flow::collab::session::run`. `known_seq` beyond the current
/// `head_seq` is not an error either: a caller racing a concurrent write may legitimately observe
/// a `known_seq` the server has not caught up to broadcasting yet, and the full bootstrap it gets
/// back is still a correct, current view.
pub async fn get_bootstrap(
    state: &AppState,
    object_id: Uuid,
    known_seq: Option<i64>,
    known_frontier: Option<String>,
) -> Result<Bootstrap, ApiError> {
    let _ = known_seq;
    if let Some(raw) = known_frontier.as_deref() {
        base64::engine::general_purpose::STANDARD
            .decode(raw)
            .map_err(|_| ApiError::BadRequest("invalid_update: known_frontier is not valid base64".to_string()))?;
    }

    let row = repository::fetch_object_view(&state.db, object_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    let document_id = row.document_id;

    // The exact loader the WebSocket `snapshot` frame uses (`flow::collab::session::run`) —
    // `ADR-0010`/`collab-protocol-v1.md` require REST and WS to share it so the two surfaces can
    // never observe divergent document state.
    let boot = bootstrap::load(&state.db, document_id).await?;

    let decoded_bytes = boot
        .snapshot
        .len()
        .saturating_add(boot.tail_updates.iter().map(|update| update.bytes.len()).sum::<usize>());
    if decoded_bytes as u64 > limits::BOOTSTRAP_DECODED_BYTES_MAX {
        // `limits-v1.md`: "超出保留边界返回 resync_required" — `flow::collab::snapshot` advances
        // the snapshot on the write path precisely to keep this bounded (a hard trigger forces a
        // checkpoint before the tail can grow past it), but that only covers documents that are
        // still being written to; a document nobody has written to since this bound shipped (or
        // whose advancement is mid-retry) can still land here. v0.4 has no delete/retention
        // compaction path before v0.8 to fall back on either way, so the only fail-closed response
        // available is the same one the loader itself already uses for a corrupted tail.
        return Err(ApiError::Conflict("resync_required".to_string()));
    }

    Ok(Bootstrap {
        object_id,
        document_id,
        engine: boot.engine,
        format_version: boot.format_version,
        snapshot_seq: boot.snapshot_seq,
        head_seq: boot.head_seq,
        snapshot_base64: base64::engine::general_purpose::STANDARD.encode(&boot.snapshot),
        tail_updates: boot
            .tail_updates
            .into_iter()
            .map(|update| TailUpdate {
                seq: update.seq,
                update_id: update.update_id,
                bytes: base64::engine::general_purpose::STANDARD.encode(&update.bytes),
                before_frontier: base64::engine::general_purpose::STANDARD.encode(&update.before_frontier),
                after_frontier: base64::engine::general_purpose::STANDARD.encode(&update.after_frontier),
            })
            .collect(),
        head_frontier: base64::engine::general_purpose::STANDARD.encode(&boot.head_frontier),
        limits: limits::effective_limits(),
        websocket_path: "/api/v1/collab/ws".to_string(),
    })
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
