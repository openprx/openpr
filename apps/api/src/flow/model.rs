//! Wire-shape response types this package's four endpoints share, matching
//! `contracts/rest-api-v1.md` ("公共类型") in `/opt/working/sylvode-flow`.
//!
//! Request DTOs live next to the handlers that parse them (`routes::flow`); these are the
//! response shapes assembled by [`crate::flow::query`] and [`crate::flow::command`] and returned
//! through the existing `ApiResponse` envelope.

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::collab::frame::TailUpdate;
use super::collab::limits::FlowLimitsV1;

/// `FlowObjectView` from `rest-api-v1.md`.
///
/// `title` and `semantic_content` are read from `flow_object_projections` (the rebuildable
/// replica `ADR-0002` describes), not decoded from the CRDT snapshot on every read.
#[derive(Debug, Clone, Serialize)]
pub struct FlowObjectView {
    pub id: Uuid,
    pub workspace_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Uuid>,
    pub object_type: String,
    pub lifecycle_status: String,
    pub governance_metadata: Value,
    pub title: String,
    pub semantic_content: Value,
    pub document_id: Uuid,
    pub document_seq: i64,
    /// Base64 of the opaque Loro version-vector frontier (`collab_core::Frontier::as_bytes`).
    pub frontier: String,
    pub projection_seq: i64,
    /// `document_seq - projection_seq`. Always `0` in this package: creation writes the
    /// projection synchronously in the same transaction as the document, so nothing can lag.
    pub projection_lag: i64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<String>,
}

/// `AcceptedChange` from `rest-api-v1.md`. Returned by the create endpoint.
///
/// `event_id` is the same `business_events.id` a caller would see again via
/// `OperationReceipt.audit_event_id` on a future command endpoint (not part of this package).
#[derive(Debug, Clone, Serialize)]
pub struct AcceptedChange {
    pub object: FlowObjectView,
    pub accepted_seq: i64,
    pub head_frontier: String,
    pub projection_seq: i64,
    pub semantic_diff: Value,
    pub affected_object_ids: Vec<Uuid>,
    pub event_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_result: Option<Value>,
}

/// `Bootstrap` from `rest-api-v1.md` (`GET /flow/objects/{object_id}/bootstrap`).
///
/// Built from `flow::collab::bootstrap::load`'s `BootstrapResult` —
/// the exact same `REPEATABLE READ READ ONLY` loader the WebSocket `snapshot` frame uses
/// (`flow::collab::session::run`), so this response and that frame can never observe divergent
/// document state (`ADR-0010`, `collab-protocol-v1.md`: "WS `open` 与 REST endpoint 使用同一
/// loader/authorization policy").
#[derive(Debug, Clone, Serialize)]
pub struct Bootstrap {
    pub object_id: Uuid,
    pub document_id: Uuid,
    pub engine: String,
    pub format_version: String,
    pub snapshot_seq: i64,
    pub head_seq: i64,
    /// Base64 of the full document snapshot bytes — only present on this diagnostics/bootstrap
    /// surface, never in `FlowObjectView`/`AcceptedChange` (`rest-api-v1.md`: "`snapshot_base64`
    /// 和 tail bytes 只在 bootstrap/受控 diagnostics 出现").
    pub snapshot_base64: String,
    pub tail_updates: Vec<TailUpdate>,
    pub head_frontier: String,
    pub limits: FlowLimitsV1,
    pub websocket_path: String,
}

/// `{items:FlowObjectView[],next_cursor?}` from the list endpoint.
#[derive(Debug, Serialize)]
pub struct FlowObjectListResponse {
    pub items: Vec<FlowObjectView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// One row of `{items:[{seq,actor,origin,message,semantic_summary,created_at}],next_before_seq?}`
/// from the history endpoint.
#[derive(Debug, Serialize)]
pub struct HistoryItem {
    pub seq: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<Uuid>,
    pub origin: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub semantic_summary: Value,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct HistoryResponse {
    pub items: Vec<HistoryItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_before_seq: Option<i64>,
}

/// `{flow_enabled,default_member_level,authz_epoch,updated_at,updated_by}` from `rest-api-v1.md`
/// (`GET|PUT /workspaces/{workspace_id}/features/flow`).
///
/// `updated_at`/`updated_by` are `null` (key still present, matching every other field here) for a
/// workspace whose `flow_workspace_settings` row does not exist yet — `flow_workspace_settings` is
/// provisioned lazily by the first `PUT`, and a `GET` must not have the side effect of creating one
/// (see [`super::policy::require_flow_enabled`]'s "provisioned lazily" doc comment).
#[derive(Debug, Clone, Serialize)]
pub struct FlowFeatureView {
    pub flow_enabled: bool,
    pub default_member_level: String,
    pub authz_epoch: i64,
    pub updated_at: Option<String>,
    pub updated_by: Option<Uuid>,
}

/// The `PUT` response: the same fields as [`FlowFeatureView`] plus `event_id`.
///
/// `event_id` is `null` when the request changed nothing observable (e.g. `enabled` was supplied
/// but already matched the current value): `flow.feature.enabled`/`flow.feature.disabled` are
/// transition events (`events-v1.md`: "false→true"/"true→false"), so a call that does not cross
/// that transition legitimately produces no new `business_events` row to point at. An idempotent
/// replay of a prior transition, and a request that does cross it, both return the real id.
#[derive(Debug, Clone, Serialize)]
pub struct FlowFeatureUpdateView {
    #[serde(flatten)]
    pub feature: FlowFeatureView,
    pub event_id: Option<Uuid>,
}

/// A `flow_import_jobs` row (`migrations/0055_flow_import_jobs.sql`).
///
/// Shaped for the status endpoints both import surfaces expose: `GET /admin/.../legacy-pages/
/// imports/{import_id}` (`rest-api-v1.md`) and the v0.8 `GET /workspaces/{workspace_id}/flow/
/// imports/{import_id}` (`ImportReport`, `export-package-v1.md`).
///
/// `report` stays an opaque JSON blob here rather than being unpacked into `counts`/
/// `object_mapping`/`warnings`/... fields: assembling that shape is the importing command's job
/// (not shipped in this package — see `flow_import_jobs.report`'s column comment), and this view
/// exists only so a future command/query module has a typed row to read the ledger through.
#[derive(Debug, Clone, Serialize)]
pub struct ImportJobView {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_workspace_id: Option<Uuid>,
    pub mapping_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_sha256: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audit_event_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// One `flow_import_lineage` row.
///
/// Shaped for the legacy status endpoint's `items:[{source_id,source_content_hash,
/// target_object_id?,result}]` (`rest-api-v1.md`) and the v0.8 `ImportReport.object_mapping[]`
/// (`export-package-v1.md`).
///
/// `target_object_id`/`target_document_id` are never absent on a row read from the table (a
/// lineage row is only ever written once its job's single commit transaction has committed, so it
/// always names a real target — see `flow_import_lineage_result_check`'s column comment); the
/// response schema's `target_object_id?` accounts for problem items the command layer reports
/// from `flow_import_jobs.report` instead, which never become a row here.
#[derive(Debug, Clone, Serialize)]
pub struct ImportLineageView {
    pub source_id: Uuid,
    pub source_content_hash: String,
    pub target_object_id: Uuid,
    pub target_document_id: Uuid,
    pub result: String,
}
