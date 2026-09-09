//! Read paths: `GET .../flow/objects`, `GET .../flow/objects/{id}` and
//! `GET .../flow/objects/{id}/history`.
//!
//! Every read here comes from `flow_objects` + `collab_documents` + `flow_object_projections`
//! (or, for history, `collab_updates` + `business_events`); nothing decodes a CRDT snapshot on the
//! read path.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
use chrono::{DateTime, Utc};
use collab_core::{CollabEngine, Frontier, LoroCollabEngine};
use platform::app::AppState;
use serde_json::json;
use uuid::Uuid;

use crate::error::ApiError;

use super::collab::bootstrap;
use super::collab::frame::TailUpdate;
use super::collab::{limits, runtime};
use super::model::{
    Bootstrap, FlowFeatureView, FlowObjectListResponse, FlowObjectView, HistoryItem, HistoryResponse,
    ObjectDiffResponse, ProjectionLagItem, ProjectionLagResponse,
};
use super::policy::{self, AuthorizedFlowObject, FlowReadContext};
use super::projection;
use super::repository::{
    self, FlowSettingsRow, HistoryFilter, HistoryRow, ListFilter, ObjectViewRow, ProjectionLagFilter, ProjectionLagRow,
};

pub const DEFAULT_LIST_LIMIT: u64 = 50;
pub const MAX_LIST_LIMIT: u64 = 100;

/// Each authorized-scan batch is one `page_limit_max`-sized page of *candidate* rows —
/// `limits-v1.md`'s own reasoning for `authorized_scan_rows_max` ("最多 overfetch 10 个最大页")
/// is ten of these, not an independently chosen tuning constant.
const SCAN_BATCH_SIZE: u64 = MAX_LIST_LIMIT;

/// Advances the running count of *candidate* rows checked so far and returns
/// `limit_exceeded`/`scan_budget` the instant it would exceed `limits::AUTHORIZED_SCAN_ROWS_MAX` —
/// `limits-v1.md`: "最多 overfetch 10 个最大页；授权过滤后不足一页也不得无界扫描". `examined` is
/// counted *before* policy filtering runs on the row (`limits-v1.md`: "Scan budget 统计
/// policy-filter 前实际检查 rows"), so this must be called for every candidate a scan loop looks
/// at, whether or not that candidate turns out to be policy-visible.
///
/// The error's `observed` is always exactly `AUTHORIZED_SCAN_ROWS_MAX`, never the caller's true
/// candidate count beyond it. `limits-v1.md` also says "不向 caller 返回过滤前 count" — the
/// pre-filter row count is exactly the number that would tell a caller how many rows exist that
/// policy would have rejected them from seeing, which is the thing this ceiling exists to keep
/// from leaking. Echoing back the fixed ceiling instead of the real scan depth satisfies
/// `error-mapping-v1.md`'s convention of naming what was exceeded (every other `limit_exceeded`
/// case here does the same) without revealing anything data-dependent: it is the same public
/// number for every caller, on every request, win or lose — already published verbatim in
/// `Bootstrap.limits.authorized_scan_rows_max` — so it carries zero information beyond "the scan
/// hit its ceiling".
fn check_scan_budget(examined: u64) -> Result<(), ApiError> {
    if examined > limits::AUTHORIZED_SCAN_ROWS_MAX {
        return Err(ApiError::limit_exceeded(
            "authorized scan budget exceeded before enough policy-visible rows were found",
            "scan_budget",
            Some(json!(limits::AUTHORIZED_SCAN_ROWS_MAX)),
            Some(json!(limits::AUTHORIZED_SCAN_ROWS_MAX)),
            None,
        ));
    }
    Ok(())
}

/// Fetches up to `needed` policy-visible [`ObjectViewRow`]s, scanning in
/// [`SCAN_BATCH_SIZE`]-sized candidate batches (advancing `filter`'s keyset cursor after every
/// row) until either `needed` rows have been accepted, the table runs out of matching rows, or
/// [`check_scan_budget`] rejects the scan.
///
/// Each candidate batch is authorized in bulk before rows are appended. The internal scan cursor
/// advances over every candidate, while the caller cursor is derived only from the accepted
/// sequence in [`list_objects`]; therefore hidden rows affect neither returned cardinality nor a
/// caller-visible count/cursor.
async fn scan_objects_within_budget(
    state: &AppState,
    access: &FlowReadContext,
    mut filter: ListFilter,
    needed: usize,
) -> Result<Option<Vec<ObjectViewRow>>, ApiError> {
    let mut accepted: Vec<ObjectViewRow> = Vec::new();
    let mut examined: u64 = 0;
    loop {
        filter.limit = SCAN_BATCH_SIZE;
        let batch = repository::list_objects(&state.db, &filter).await?;
        let batch_len = batch.len();
        if batch_len == 0 {
            return Ok(Some(accepted));
        }
        let object_ids: Vec<Uuid> = batch.iter().map(|row| row.id).collect();
        let Some(visible) =
            policy::authorize_flow_objects(state, access, &object_ids, super::collab::authz::PermissionLevel::View)
                .await?
        else {
            return Ok(None);
        };
        for (row, is_visible) in batch.into_iter().zip(visible) {
            examined += 1;
            check_scan_budget(examined)?;
            filter.after = Some((row.created_at, row.id));
            if is_visible {
                accepted.push(row);
                if accepted.len() >= needed {
                    return Ok(Some(accepted));
                }
            }
        }
        if (batch_len as u64) < SCAN_BATCH_SIZE {
            return Ok(Some(accepted));
        }
    }
}

/// Projection-lag counterpart of [`scan_objects_within_budget`]. Every candidate batch is sent
/// through the shared batch evaluator before a row can reach either `items` or the aggregate
/// input. Hidden rows advance only the internal scan cursor and candidate budget.
async fn scan_projection_lag_within_budget(
    state: &AppState,
    access: &FlowReadContext,
    mut filter: ProjectionLagFilter,
    needed: usize,
) -> Result<Option<Vec<ProjectionLagRow>>, ApiError> {
    let mut accepted = Vec::new();
    let mut examined = 0_u64;
    loop {
        filter.limit = SCAN_BATCH_SIZE;
        let batch = repository::list_projection_lag_candidates(&state.db, &filter).await?;
        let batch_len = batch.len();
        if batch_len == 0 {
            return Ok(Some(accepted));
        }
        let object_ids: Vec<Uuid> = batch.iter().map(|row| row.object_id).collect();
        let Some(visible) =
            policy::authorize_flow_objects(state, access, &object_ids, super::collab::authz::PermissionLevel::View)
                .await?
        else {
            return Ok(None);
        };
        if visible.len() != batch_len {
            return Err(ApiError::Internal);
        }
        for (row, is_visible) in batch.into_iter().zip(visible) {
            examined = examined.saturating_add(1);
            check_scan_budget(examined)?;
            filter.after = Some((row.created_at, row.object_id));
            if is_visible {
                accepted.push(row);
                if accepted.len() >= needed {
                    return Ok(Some(accepted));
                }
            }
        }
        if (batch_len as u64) < SCAN_BATCH_SIZE {
            return Ok(Some(accepted));
        }
    }
}

/// The `get_history` analog of [`scan_objects_within_budget`] — same batch/cursor/budget shape,
/// walking `HistoryFilter::before_seq` backwards instead of `ListFilter::after` forwards.
/// History rows are events from one object that the caller has already authorized, so they do
/// not receive per-row object authorization; the scan exists only to enforce pagination and the
/// shared bounded-work ceiling.
async fn scan_history_within_budget(
    state: &AppState,
    mut filter: HistoryFilter,
    needed: usize,
) -> Result<Vec<HistoryRow>, ApiError> {
    let mut accepted: Vec<HistoryRow> = Vec::new();
    let mut examined: u64 = 0;
    loop {
        filter.limit = SCAN_BATCH_SIZE;
        let batch = repository::fetch_history(&state.db, &filter).await?;
        let batch_len = batch.len();
        if batch_len == 0 {
            return Ok(accepted);
        }
        for row in batch {
            examined += 1;
            check_scan_budget(examined)?;
            filter.before_seq = Some(row.seq);
            accepted.push(row);
            if accepted.len() >= needed {
                return Ok(accepted);
            }
        }
        if (batch_len as u64) < SCAN_BATCH_SIZE {
            return Ok(accepted);
        }
    }
}

/// Clamps a caller-supplied `limit` query parameter.
///
/// Rejects anything outside `1..=100` rather than silently clamping it — `limits-v1.md`'s
/// boundary-testing convention throughout this contract is exact-accepted / one-past-rejected,
/// not silent clamping.
pub fn validate_limit(limit: Option<u64>) -> Result<u64, ApiError> {
    match limit {
        None => Ok(DEFAULT_LIST_LIMIT),
        Some(0) => Err(ApiError::BadRequest("limit must be at least 1".to_string())),
        Some(value) if value > MAX_LIST_LIMIT => Err(ApiError::limit_exceeded(
            format!("limit must be at most {MAX_LIST_LIMIT}"),
            "page_size",
            Some(json!(MAX_LIST_LIMIT)),
            Some(json!(value)),
            None,
        )),
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
    access: &AuthorizedFlowObject,
    at_seq: Option<i64>,
    render: Render,
) -> Result<Option<FlowObjectView>, ApiError> {
    let object_id = access.object_id();
    let row = repository::fetch_object_view(&state.db, object_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    runtime::runtime().ensure_workspace_accepting(row.workspace_id)?;
    if row.workspace_id != access.workspace_id() {
        return Err(ApiError::NotFound("flow object not found".to_string()));
    }

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
    if !policy::ensure_epoch_current(state, access.context()).await? {
        return Ok(None);
    }
    Ok(Some(view))
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
    access: &AuthorizedFlowObject,
    known_seq: Option<i64>,
    known_frontier: Option<String>,
) -> Result<Option<Bootstrap>, ApiError> {
    let object_id = access.object_id();
    let _ = known_seq;
    if let Some(raw) = known_frontier.as_deref() {
        base64::engine::general_purpose::STANDARD
            .decode(raw)
            .map_err(|_| ApiError::BadRequest("invalid_update: known_frontier is not valid base64".to_string()))?;
    }

    let row = repository::fetch_object_view(&state.db, object_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    runtime::runtime().ensure_workspace_accepting(row.workspace_id)?;
    if row.workspace_id != access.workspace_id() {
        return Err(ApiError::NotFound("flow object not found".to_string()));
    }
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

    let response = Bootstrap {
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
    };
    if !policy::ensure_epoch_current(state, access.context()).await? {
        return Ok(None);
    }
    Ok(Some(response))
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

pub async fn list_objects(
    state: &AppState,
    access: &FlowReadContext,
    params: ListObjectsParams,
) -> Result<Option<FlowObjectListResponse>, ApiError> {
    if params.workspace_id != access.workspace_id() {
        return Err(ApiError::Internal);
    }
    runtime::runtime().ensure_workspace_accepting(params.workspace_id)?;
    if params.project_id.is_some() && params.unprojected {
        return Err(ApiError::BadRequest(
            "project_id and unprojected=true are mutually exclusive".to_string(),
        ));
    }
    let limit = validate_limit(params.limit)?;
    let after = params.cursor.as_deref().map(decode_cursor).transpose()?;
    let limit_usize = usize::try_from(limit).unwrap_or(usize::MAX);
    // Fetch one extra row to know whether a further page exists without a second query.
    let needed = limit_usize.saturating_add(1);

    let filter = ListFilter {
        workspace_id: params.workspace_id,
        project_id: params.project_id,
        unprojected: params.unprojected,
        object_type: params.object_type,
        parent_id: params.parent_id,
        title_prefix: params.q,
        include_archived: params.include_archived,
        after,
        // Overwritten per candidate batch by `scan_objects_within_budget`.
        limit: 0,
    };
    let Some(mut rows) = scan_objects_within_budget(state, access, filter, needed).await? else {
        return Ok(None);
    };
    if !policy::ensure_epoch_current(state, access).await? {
        return Ok(None);
    }

    let next_cursor = if rows.len() > limit_usize {
        rows.truncate(limit_usize);
        rows.last().map(|row| encode_cursor(row.created_at, row.id))
    } else {
        None
    };

    Ok(Some(FlowObjectListResponse {
        items: rows.into_iter().map(object_view_from_row).collect(),
        next_cursor,
    }))
}

pub async fn get_history(
    state: &AppState,
    access: &AuthorizedFlowObject,
    before_seq: Option<i64>,
    limit: Option<u64>,
) -> Result<Option<HistoryResponse>, ApiError> {
    let object_id = access.object_id();
    let row = repository::fetch_object_view(&state.db, object_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    runtime::runtime().ensure_workspace_accepting(row.workspace_id)?;
    if row.workspace_id != access.workspace_id() {
        return Err(ApiError::NotFound("flow object not found".to_string()));
    }
    let limit = validate_limit(limit)?;
    let limit_usize = usize::try_from(limit).unwrap_or(usize::MAX);
    // Fetch one extra row to know whether a further page exists without a second query.
    let needed = limit_usize.saturating_add(1);

    let filter = HistoryFilter {
        document_id: row.document_id,
        before_seq,
        // Overwritten per candidate batch by `scan_history_within_budget`.
        limit: 0,
    };
    let mut rows = scan_history_within_budget(state, filter, needed).await?;

    let next_before_seq = if rows.len() > limit_usize {
        rows.truncate(limit_usize);
        rows.last().map(|row| row.seq)
    } else {
        None
    };

    let response = HistoryResponse {
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
    };
    if !policy::ensure_epoch_current(state, access.context()).await? {
        return Ok(None);
    }
    Ok(Some(response))
}

/// Reconstructs and compares two accepted document sequences using `collab_core`'s canonical
/// semantic representation.
///
/// The history UI contract is the privacy precedent: semantic fields and application `NodeId`s
/// are readable, while CRDT update bytes and engine peer ids are not. Consequently only
/// [`SemanticSnapshot::diff`] reaches `semantic_diff`; the retained bytes are consumed locally
/// and discarded. Markdown uses [`projection::render_markdown`] rather than a second renderer.
pub async fn get_object_diff(
    state: &AppState,
    access: &AuthorizedFlowObject,
    from_seq: i64,
    to_seq: i64,
    render: Render,
) -> Result<Option<ObjectDiffResponse>, ApiError> {
    if from_seq < 0 || to_seq < 0 {
        return Err(ApiError::invalid_update("from_seq and to_seq must be non-negative"));
    }
    if from_seq > to_seq {
        return Err(ApiError::invalid_update(
            "from_seq must be less than or equal to to_seq",
        ));
    }

    let object_id = access.object_id();
    let rows = repository::fetch_diff_history(&state.db, object_id, to_seq)
        .await?
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    runtime::runtime().ensure_workspace_accepting(rows.workspace_id)?;
    if rows.workspace_id != access.workspace_id() {
        return Err(ApiError::NotFound("flow object not found".to_string()));
    }
    if to_seq > rows.head_seq {
        let head_frontier = base64::engine::general_purpose::STANDARD.encode(&rows.head_frontier);
        return Err(ApiError::stale_frontier(
            "to_seq is beyond the current document head",
            Some(rows.head_seq),
            Some(&head_frontier),
        ));
    }

    // Accepted history is retained in v0.5. A missing/corrupt row inside `1..=to_seq` is not an
    // alternate meaning of the requested sequence and must never be clamped to a nearby state.
    let document_id = rows.document_id;
    let history_invalid = || {
        tracing::error!(%object_id, %document_id, "diff history is missing or corrupt");
        ApiError::resync_required("requested history cannot be reconstructed", None)
    };

    let mut engine = LoroCollabEngine::load(&rows.snapshot).map_err(|_| history_invalid())?;
    if engine.frontier().as_bytes() != rows.snapshot_frontier {
        return Err(history_invalid());
    }

    let history_upper = if to_seq == 0 && rows.head_seq > 0 { 1 } else { to_seq };
    let mut expected_seq = 1_i64;
    let mut running_frontier: Option<Vec<u8>> = None;
    let mut from_frontier = None;
    let mut to_frontier = None;
    for row in rows.updates {
        if row.seq != expected_seq
            || running_frontier
                .as_ref()
                .is_some_and(|frontier| frontier != &row.before_frontier)
            || super::collab::bootstrap::content_hash(&row.bytes) != row.content_hash
        {
            return Err(history_invalid());
        }
        if row.seq == 1 {
            if from_seq == 0 {
                from_frontier = Some(Frontier::from_bytes(row.before_frontier.clone()));
            }
            if to_seq == 0 {
                to_frontier = Some(Frontier::from_bytes(row.before_frontier.clone()));
            }
        }
        if row.seq > rows.snapshot_seq && row.seq <= to_seq {
            if engine.frontier().as_bytes() != row.before_frontier {
                return Err(history_invalid());
            }
            engine.import_update(&row.bytes).map_err(|_| history_invalid())?;
            if engine.frontier().as_bytes() != row.after_frontier {
                return Err(history_invalid());
            }
        }
        if row.seq == from_seq {
            from_frontier = Some(Frontier::from_bytes(row.after_frontier.clone()));
        }
        if row.seq == to_seq {
            to_frontier = Some(Frontier::from_bytes(row.after_frontier.clone()));
        }
        running_frontier = Some(row.after_frontier);
        expected_seq = expected_seq.saturating_add(1);
    }
    if expected_seq != history_upper.saturating_add(1) {
        return Err(history_invalid());
    }
    if rows.head_seq == 0 {
        let initial = Frontier::from_bytes(rows.head_frontier);
        from_frontier = Some(initial.clone());
        to_frontier = Some(initial);
    }

    let Some(from_frontier) = from_frontier else {
        return Err(history_invalid());
    };
    let Some(to_frontier) = to_frontier else {
        return Err(history_invalid());
    };
    let from_engine = engine.fork_at_frontier(&from_frontier).map_err(|_| history_invalid())?;
    let to_engine = engine.fork_at_frontier(&to_frontier).map_err(|_| history_invalid())?;
    let from_snapshot = from_engine.semantic_snapshot().map_err(|_| history_invalid())?;
    let to_snapshot = to_engine.semantic_snapshot().map_err(|_| history_invalid())?;
    let from_title = from_engine.title().map_err(|_| history_invalid())?;
    let to_title = to_engine.title().map_err(|_| history_invalid())?;

    let rendered = (render == Render::Markdown).then(|| projection::render_markdown(&to_title));
    let title_diff = (from_title != to_title).then(|| json!({ "before": from_title, "after": to_title }));
    let response = ObjectDiffResponse {
        object_id,
        from_seq,
        to_seq,
        from_frontier: base64::engine::general_purpose::STANDARD.encode(from_frontier.as_bytes()),
        to_frontier: base64::engine::general_purpose::STANDARD.encode(to_frontier.as_bytes()),
        semantic_diff: json!({
            "title": title_diff,
            "nodes": from_snapshot.diff(&to_snapshot),
        }),
        rendered,
    };
    if !policy::ensure_epoch_current(state, access.context()).await? {
        return Ok(None);
    }
    Ok(Some(response))
}

pub struct ProjectionLagParams {
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub cursor: Option<String>,
    pub limit: Option<u64>,
}

/// Aggregates one already-policy-filtered response page.
///
/// `p95_lag` uses the nearest-rank definition: sort ascending and select rank
/// `ceil(0.95 * n)` (one-based). An empty page returns `(0, 0)`; for 1 through 19 samples that
/// rank is the final sample, so p95 equals the page maximum. Both values are computed only after
/// authorization and page truncation, making an inaccessible object's lag incapable of changing
/// even an aggregate field.
fn projection_lag_aggregates(items: &[ProjectionLagItem]) -> (i64, i64) {
    if items.is_empty() {
        return (0, 0);
    }
    let mut lags: Vec<i64> = items.iter().map(|item| item.lag).collect();
    lags.sort_unstable();
    let max_lag = lags.last().copied().unwrap_or(0);
    let rank = lags.len().saturating_mul(95).div_ceil(100);
    let p95_lag = lags.get(rank.saturating_sub(1)).copied().unwrap_or(0);
    (max_lag, p95_lag)
}

/// `GET /workspaces/{workspace_id}/flow/projection-lag` domain read.
pub async fn get_projection_lag(
    state: &AppState,
    access: &FlowReadContext,
    params: ProjectionLagParams,
) -> Result<Option<ProjectionLagResponse>, ApiError> {
    if params.workspace_id != access.workspace_id() {
        return Err(ApiError::Internal);
    }
    runtime::runtime().ensure_workspace_accepting(params.workspace_id)?;
    let limit = validate_limit(params.limit)?;
    let after = params.cursor.as_deref().map(decode_cursor).transpose()?;
    let limit_usize = usize::try_from(limit).unwrap_or(usize::MAX);
    let needed = limit_usize.saturating_add(1);
    let filter = ProjectionLagFilter {
        workspace_id: params.workspace_id,
        project_id: params.project_id,
        after,
        limit: 0,
    };
    let Some(mut rows) = scan_projection_lag_within_budget(state, access, filter, needed).await? else {
        return Ok(None);
    };
    if !policy::ensure_epoch_current(state, access).await? {
        return Ok(None);
    }

    let next_cursor = if rows.len() > limit_usize {
        rows.truncate(limit_usize);
        rows.last().map(|row| encode_cursor(row.created_at, row.object_id))
    } else {
        None
    };
    let items: Vec<ProjectionLagItem> = rows
        .into_iter()
        .map(|row| ProjectionLagItem {
            object_id: row.object_id,
            head_seq: row.head_seq,
            projection_seq: row.projection_seq,
            lag: row.head_seq - row.projection_seq,
        })
        .collect();
    let (max_lag, p95_lag) = projection_lag_aggregates(&items);
    Ok(Some(ProjectionLagResponse {
        max_lag,
        p95_lag,
        items,
        next_cursor,
    }))
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::error::ApiErrorKind;

    /// `limits-v1.md`'s `authorized_scan_rows_max` (1,000) exact boundary: a scan that examines
    /// exactly the ceiling's worth of candidate rows is still within budget — the ceiling is a
    /// "would need one more row" trigger, not an "at the ceiling" one.
    #[test]
    fn check_scan_budget_accepts_the_exact_authorized_scan_rows_max_boundary() {
        assert!(check_scan_budget(limits::AUTHORIZED_SCAN_ROWS_MAX).is_ok());
    }

    /// One candidate row past the ceiling is rejected as `limit_exceeded`/`scan_budget`, and both
    /// `limit` and `observed` in the response are the fixed ceiling itself — never the caller's
    /// true candidate count, which would tell them how many rows exist that they are not
    /// authorized to see (`limits-v1.md`: "不向 caller 返回过滤前 count"). This is asserted at
    /// `AUTHORIZED_SCAN_ROWS_MAX + 1` specifically (not some larger number) so the boundary itself
    /// — not just "eventually rejects" — is what is pinned.
    #[test]
    fn check_scan_budget_rejects_one_row_past_the_authorized_scan_rows_max_boundary() {
        let err =
            check_scan_budget(limits::AUTHORIZED_SCAN_ROWS_MAX + 1).expect_err("one row past the ceiling must reject");
        let ApiError::Typed { kind, details, .. } = err else {
            panic!("expected ApiError::Typed, got a differently-shaped ApiError");
        };
        assert_eq!(kind.stable_code(), ApiErrorKind::LimitExceeded.stable_code());
        let details = details.expect("limit_exceeded always carries structured details");
        assert_eq!(details["limit_kind"], "scan_budget");
        assert_eq!(details["limit"], limits::AUTHORIZED_SCAN_ROWS_MAX);
        assert_eq!(
            details["observed"],
            limits::AUTHORIZED_SCAN_ROWS_MAX,
            "observed must equal the fixed ceiling, not a real pre-filter row count -- that would \
             leak how many unauthorized rows exist beyond it"
        );
    }

    /// A scan budget one below the ceiling never trips, regardless of how many more candidates
    /// remain to be examined after it -- `examined` alone (not "examined so far this request minus
    /// something") is what the ceiling compares against.
    #[test]
    fn check_scan_budget_accepts_one_row_before_the_authorized_scan_rows_max_boundary() {
        assert!(check_scan_budget(limits::AUTHORIZED_SCAN_ROWS_MAX - 1).is_ok());
    }

    fn lag_item(lag: i64) -> ProjectionLagItem {
        ProjectionLagItem {
            object_id: Uuid::nil(),
            head_seq: lag,
            projection_seq: 0,
            lag,
        }
    }

    #[test]
    fn projection_lag_p95_is_nearest_rank_with_explicit_small_sample_behavior() {
        assert_eq!(projection_lag_aggregates(&[]), (0, 0));
        assert_eq!(projection_lag_aggregates(&[lag_item(7)]), (7, 7));
        assert_eq!(
            projection_lag_aggregates(&(0..19).map(lag_item).collect::<Vec<_>>()),
            (18, 18),
            "fewer than twenty samples use the maximum as nearest-rank p95"
        );

        let mut twenty: Vec<ProjectionLagItem> = (0..19).map(lag_item).collect();
        twenty.push(lag_item(10_000));
        assert_eq!(
            projection_lag_aggregates(&twenty),
            (10_000, 18),
            "at twenty samples the single largest outlier is above nearest-rank p95"
        );
    }
}
