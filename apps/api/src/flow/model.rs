//! Wire-shape response types this package's four endpoints share, matching
//! `contracts/rest-api-v1.md` ("公共类型") in `/opt/working/sylvode-flow`.
//!
//! Request DTOs live next to the handlers that parse them (`routes::flow`); these are the
//! response shapes assembled by [`crate::flow::query`] and [`crate::flow::command`] and returned
//! through the existing `ApiResponse` envelope.

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

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
