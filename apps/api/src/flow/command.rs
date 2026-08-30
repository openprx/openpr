//! The write paths this package ships: `POST .../flow/objects` (object creation) and
//! `POST .../flow/objects/{id}/commands` (`set_title|insert_block|update_block|delete_block|
//! move_block|semantic_patch|archive|restore`, `rest-api-v1.md` "v0.4 Flow Alpha").
//!
//! The six content types share the *exact* write path `flow::collab::write::accept_update` and
//! the WebSocket `update` frame use (hydrate/isolated-apply outside any lock, the per-document
//! coordinator, the commit-time `authz_epoch` fence, the document row lock) — this module only
//! turns a command payload into the same shape of CRDT update bytes a WebSocket client would have
//! produced locally, then calls the identical function. `archive`/`restore` never advance a
//! document head (`existing_document_cardinality = 0`, `rest-api-v1.md`'s `move_object`/`link`
//! commentary on the same rule), so they take no document coordinator and no document row lock —
//! a plain `flow_objects` row transaction, matching `create_object`'s own shape below. They *do*
//! still take the commit-time `authz_epoch` fence (`ADR-0012` §3.1, `authz::fence_epoch_for_share`)
//! inside that transaction: the fence's job is authorization freshness, not document-head
//! consistency, so "no document head to advance" does not imply "no epoch to re-verify" — see
//! `execute_lifecycle_command`'s own doc comment for the TOCTOU window this closes.

#![allow(clippy::too_long_first_doc_paragraph)]

use collab_core::{CollabEngine, CollabError, LoroCollabEngine, NodeId, NodeKind, Operation};
use platform::app::AppState;
use sea_orm::TransactionTrait;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::{ApiError, ApiErrorKind, ServerDrainingReason};
use crate::events::{BusinessEventInput, FlowDispatchSpec, insert_flow_event};

use super::collab::{authz, bootstrap, frame, limits as collab_limits, runtime, write};
use super::model::{AcceptedChange, FlowFeatureUpdateView, FlowObjectView};
use super::projection;
use super::query::feature_view_from_row;
use super::repository::{self, NewCollabDocument, NewFlowObject, NewProjection};

/// Registered `object_type` values (`domain-model-v1.md` "Object types": v0.4 ships `navigator`
/// and `page`; `collection`/`record` are v0.6). Not a `flow_objects_object_type_check` mirror by
/// accident — the two must never drift, and the CHECK constraint is the actual enforcement; this
/// list only lets the handler reject early with a typed `invalid_update` instead of a DB error.
const REGISTERED_OBJECT_TYPES: &[&str] = &["page", "navigator"];

/// Not frozen by `limits-v1.md` (only `message` at 500 chars and `idempotency_key` at 1-128 bytes
/// are). Chosen to match the one limit the contract *does* freeze for a similar caller-supplied
/// string, pending that gap being closed upstream.
const TITLE_MAX_CHARS: usize = 500;
const MESSAGE_MAX_CHARS: usize = 500;
const IDEMPOTENCY_KEY_MIN_BYTES: usize = 1;
const IDEMPOTENCY_KEY_MAX_BYTES: usize = 128;

/// Loro document format tag stored in `collab_documents.format_version`. Not frozen by any
/// contract file (`export-package-v1.md` only freezes an analogous `wire_format_version` for the
/// export/import package shape, not for `collab_documents` itself) — versioned independently of
/// the `loro` crate's own semver so a future encoding change can be detected without conflating it
/// with a dependency bump.
const DOCUMENT_FORMAT_VERSION: &str = "loro-1";

/// `ADR-0013` §"Gate 接线" (`command_contended_document_cardinality`): every write command must
/// machine-verifiably declare how many *already-existing* documents its execution contends a head
/// advance on (the "contended existing document set", not the full set of rows a command writes —
/// a brand-new document a command also creates in the same transaction never counts here).
///
/// - `Zero`: no existing document's head is advanced — pure `PostgreSQL` governance/metadata
///   (`ADR-0013` §1's first table row: archive/restore, feature flag, authz change,
///   relation link/unlink).
/// - `One`: exactly one existing document's head is advanced — v0.4's content commands, and
///   (from v0.5) a parented create that only touches its navigator's ordering.
/// - `BoundedMany(n)`: `n` existing documents contend a head advance in one command — `ADR-0013`
///   §2's multi-document lock-order path, first introduced by v0.5's cross-project `move_object`.
///   **No v0.4-registered command may declare this** (`ADR-0013` §1: "v0.4 的竞争文档集合恒 ≤ 1");
///   `v0_4_command_cardinality_registry`'s own test below is the machine gate for that bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistingDocumentCardinality {
    Zero,
    One,
    BoundedMany(u8),
}

impl ExistingDocumentCardinality {
    pub const fn count(self) -> u8 {
        match self {
            Self::Zero => 0,
            Self::One => 1,
            Self::BoundedMany(n) => n,
        }
    }
}

/// `create_object`'s declared `existing_document_cardinality`: `Zero`, matching what this
/// function actually does today — it only ever inserts a brand-new `flow_objects` row and a
/// brand-new `collab_documents` row (see `create_object`'s own "no existing row to lock" doc
/// comment above); a `parent_object_id` only sets `flow_objects.parent_id` (a plain `PostgreSQL` FK
/// column), it does not touch the parent navigator's own CRDT document. This is *not* yet the
/// "带 parent 的 create 只锁 navigator" cardinality-1 case `ADR-0013` §1's table describes for a
/// future navigator-ordering feature — that feature does not exist in this package, so declaring
/// `One` here would assert a lock this code never takes.
pub const CREATE_OBJECT_CARDINALITY: ExistingDocumentCardinality = ExistingDocumentCardinality::Zero;

/// `set_flow_feature`'s declared `existing_document_cardinality`: `Zero` — a
/// `flow_workspace_settings` row transition, no `collab_documents` row involved at all.
pub const SET_FLOW_FEATURE_CARDINALITY: ExistingDocumentCardinality = ExistingDocumentCardinality::Zero;

/// Every v0.4-registered write command, by its wire `command.type`/endpoint name, alongside its
/// declared [`ExistingDocumentCardinality`] — the machine-checkable registry
/// `command_contended_document_cardinality` asserts over (this module's own test below, and any
/// future `verify-flow-cardinality-v0.4.sh` gate script that wants the same facts from Rust rather
/// than re-deriving them from prose).
pub fn v0_4_command_cardinality_registry() -> Vec<(&'static str, ExistingDocumentCardinality)> {
    let mut registry = vec![
        ("create_object", CREATE_OBJECT_CARDINALITY),
        ("set_flow_feature", SET_FLOW_FEATURE_CARDINALITY),
    ];
    for content in [
        ContentCommandType::SetTitle,
        ContentCommandType::InsertBlock,
        ContentCommandType::UpdateBlock,
        ContentCommandType::DeleteBlock,
        ContentCommandType::MoveBlock,
        ContentCommandType::SemanticPatch,
    ] {
        registry.push((content.wire_name(), content.existing_document_cardinality()));
    }
    for lifecycle in [LifecycleCommandType::Archive, LifecycleCommandType::Restore] {
        registry.push((lifecycle.wire_name(), lifecycle.existing_document_cardinality()));
    }
    registry
}

pub struct CreateObjectInput {
    pub workspace_id: Uuid,
    pub actor_id: Uuid,
    pub object_type: String,
    pub project_id: Option<Uuid>,
    pub parent_object_id: Option<Uuid>,
    pub title: String,
    pub idempotency_key: String,
    pub message: Option<String>,
}

fn validate(input: &CreateObjectInput) -> Result<(), ApiError> {
    if !REGISTERED_OBJECT_TYPES.contains(&input.object_type.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "object_type must be one of {REGISTERED_OBJECT_TYPES:?}"
        )));
    }
    let title = input.title.trim();
    if title.is_empty() {
        return Err(ApiError::BadRequest("title must not be empty".to_string()));
    }
    if title.chars().count() > TITLE_MAX_CHARS {
        return Err(ApiError::BadRequest(format!(
            "title must be at most {TITLE_MAX_CHARS} characters"
        )));
    }
    let key_bytes = input.idempotency_key.len();
    if !(IDEMPOTENCY_KEY_MIN_BYTES..=IDEMPOTENCY_KEY_MAX_BYTES).contains(&key_bytes) {
        return Err(ApiError::BadRequest(format!(
            "idempotency_key must be {IDEMPOTENCY_KEY_MIN_BYTES}-{IDEMPOTENCY_KEY_MAX_BYTES} bytes"
        )));
    }
    if let Some(message) = &input.message
        && message.chars().count() > MESSAGE_MAX_CHARS
    {
        return Err(ApiError::BadRequest(format!(
            "message must be at most {MESSAGE_MAX_CHARS} characters"
        )));
    }
    Ok(())
}

/// A caller-supplied `project_id`/`parent_object_id` that resolves to a *real* row, but one that
/// lives in a different workspace than the request's own `workspace_id`. `rest-api-v1.md`
/// ("RelationView"): "数据库约束外出现跨 workspace relation 时整次请求 fail closed 为
/// `invalid_update`、记录 integrity alert" — `v0.4-flow-alpha.md` names this exact check as the
/// v0.4-scoped instance of that rule (no `flow_relations` table exists before v0.5; a create's
/// `project_id`/`parent_object_id` are v0.4's only cross-object references). Records the drift in
/// `flow_integrity_records` (`ADR-0013` §4) before failing closed — the id existing at all but in
/// the wrong workspace is never a plain "not found" typo, so it gets a paper trail a genuine
/// missing-row `BadRequest` does not.
async fn record_cross_workspace_relation_and_fail_closed(
    state: &AppState,
    requesting_workspace_id: Uuid,
    subject_kind: &str,
    referenced_id: Uuid,
    referenced_workspace_id: Uuid,
) -> ApiError {
    if let Err(err) = repository::insert_integrity_record(
        &state.db,
        repository::IntegrityRecordInput {
            workspace_id: requesting_workspace_id,
            kind: "cross_workspace_relation",
            subject_kind,
            subject_id: &referenced_id.to_string(),
            detected_by: "flow.command.create_object",
            details_redacted: json!({
                "referenced_workspace_id": referenced_workspace_id,
                "requesting_workspace_id": requesting_workspace_id,
            }),
        },
    )
    .await
    {
        tracing::error!(error = %err, "failed to record integrity alert for a cross-workspace relation");
    }
    ApiError::BadRequest("invalid_update".to_string())
}

pub async fn create_object(state: &AppState, input: CreateObjectInput) -> Result<AcceptedChange, ApiError> {
    runtime::runtime().ensure_workspace_accepting(input.workspace_id)?;
    validate(&input)?;
    let title = input.title.trim().to_string();

    // Idempotent replay: a caller retrying the exact same `idempotency_key` gets back the
    // original result instead of a unique-violation `Conflict`
    // (`business_events_idempotency` is unique on `(workspace_id, idempotency_key)`).
    if let Some(existing) =
        repository::find_idempotent_event(&state.db, input.workspace_id, &input.idempotency_key).await?
    {
        if existing.event_type != "flow.object.created" {
            return Err(ApiError::Conflict(
                "idempotency_key was already used for a different operation".to_string(),
            ));
        }
        let object_id = Uuid::parse_str(&existing.aggregate_id).map_err(|_| ApiError::Internal)?;
        let view = repository::fetch_object_view(&state.db, object_id)
            .await?
            .ok_or(ApiError::Internal)?;
        if view.projection_title != title {
            return Err(ApiError::Conflict(
                "idempotency_key was already used to create an object with a different title".to_string(),
            ));
        }
        return Ok(accepted_change_from_row(view, existing.id));
    }

    if let Some(project_id) = input.project_id {
        let project_workspace = repository::fetch_project_workspace(&state.db, project_id)
            .await?
            .ok_or_else(|| ApiError::BadRequest("project not found".to_string()))?;
        if project_workspace != input.workspace_id {
            return Err(record_cross_workspace_relation_and_fail_closed(
                state,
                input.workspace_id,
                "project",
                project_id,
                project_workspace,
            )
            .await);
        }
    }

    if let Some(parent_id) = input.parent_object_id {
        let parent = repository::fetch_parent_object(&state.db, parent_id)
            .await?
            .ok_or_else(|| ApiError::BadRequest("parent_object_id not found".to_string()))?;
        if parent.workspace_id != input.workspace_id {
            return Err(record_cross_workspace_relation_and_fail_closed(
                state,
                input.workspace_id,
                "flow_object",
                parent_id,
                parent.workspace_id,
            )
            .await);
        }
        if parent.lifecycle_status == "archived" {
            return Err(ApiError::BadRequest("parent_object_id is archived".to_string()));
        }
    }

    // Build the document outside any lock, matching `v0.4-flow-alpha.md`'s "hydrate/apply
    // outside the row lock" rule (there is no existing row to lock for a brand-new document
    // anyway: `contended_existing_document_set` is 0 for this command, see
    // `domain-model-v1.md` "Command boundaries").
    let mut engine = LoroCollabEngine::new_empty(rand::random());
    engine.set_title(&title).map_err(|err| {
        tracing::error!(error = %err, "collab-core: set_title failed on a brand-new document");
        ApiError::Internal
    })?;
    let snapshot = engine.export_snapshot().map_err(|err| {
        tracing::error!(error = %err, "collab-core: export_snapshot failed on a brand-new document");
        ApiError::Internal
    })?;
    let frontier = engine.frontier();
    let semantic = engine.semantic_snapshot().map_err(|err| {
        tracing::error!(error = %err, "collab-core: semantic_snapshot failed on a brand-new document");
        ApiError::Internal
    })?;
    let state_json = projection::state_json(&semantic).map_err(|_| ApiError::Internal)?;
    let plain_text = projection::plain_text(&semantic);

    let object_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let frontier_bytes = frontier.as_bytes().to_vec();

    let tx = state.db.begin().await?;

    repository::insert_flow_object(
        &tx,
        &NewFlowObject {
            id: object_id,
            workspace_id: input.workspace_id,
            project_id: input.project_id,
            object_type: input.object_type.clone(),
            parent_id: input.parent_object_id,
            created_by: input.actor_id,
        },
    )
    .await?;

    repository::insert_collab_document(
        &tx,
        &NewCollabDocument {
            id: document_id,
            object_id,
            format_version: DOCUMENT_FORMAT_VERSION.to_string(),
            snapshot,
            frontier: frontier_bytes.clone(),
        },
    )
    .await?;

    repository::insert_projection(
        &tx,
        &NewProjection {
            object_id,
            document_seq: 0,
            document_frontier: frontier_bytes.clone(),
            title: title.clone(),
            state: state_json.clone(),
            plain_text,
        },
    )
    .await?;

    let dispatch_max_attempts = crate::config::runtime().flow.dispatch_max_attempts;
    let event_id = insert_flow_event(
        &tx,
        BusinessEventInput {
            workspace_id: input.workspace_id,
            project_id: input.project_id,
            event_type: "flow.object.created".to_string(),
            aggregate_type: "flow_object".to_string(),
            aggregate_id: object_id.to_string(),
            actor_id: Some(input.actor_id),
            source: json!({ "surface": "rest" }),
            payload: json!({
                "object_id": object_id,
                "object_type": input.object_type,
                "parent_object_id": input.parent_object_id,
            }),
            metadata: json!({ "message": input.message }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: Some(input.idempotency_key.clone()),
        },
        Some(FlowDispatchSpec {
            max_attempts: dispatch_max_attempts,
            document_id: None,
            accepted_seq: None,
        }),
    )
    .await?
    .event_id;

    tx.commit().await?;

    let object = FlowObjectView {
        id: object_id,
        workspace_id: input.workspace_id,
        project_id: input.project_id,
        parent_id: input.parent_object_id,
        object_type: input.object_type,
        lifecycle_status: "active".to_string(),
        governance_metadata: json!({}),
        title,
        semantic_content: state_json.clone(),
        document_id,
        document_seq: 0,
        frontier: projection::encode_frontier(&frontier),
        projection_seq: 0,
        projection_lag: 0,
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        archived_at: None,
    };

    Ok(AcceptedChange {
        accepted_seq: 0,
        head_frontier: object.frontier.clone(),
        projection_seq: 0,
        semantic_diff: state_json,
        affected_object_ids: vec![object_id],
        event_id,
        command_result: None,
        object,
    })
}

/// `flow_object_grants.level` / `flow_workspace_settings.default_member_level`'s four values
/// (`domain-model-v1.md`), reused here so `set_flow_feature` rejects an unknown level the same way
/// the database `CHECK` constraint would, instead of surfacing a `Database` 500.
const MEMBER_LEVELS: &[&str] = &["full_access", "edit", "comment", "view"];

pub struct SetFlowFeatureInput {
    pub workspace_id: Uuid,
    pub actor_id: Uuid,
    pub enabled: Option<bool>,
    pub default_member_level: Option<String>,
    pub idempotency_key: String,
}

fn validate_set_flow_feature(input: &SetFlowFeatureInput) -> Result<(), ApiError> {
    if input.enabled.is_none() && input.default_member_level.is_none() {
        return Err(ApiError::BadRequest(
            "at least one of enabled or default_member_level must be supplied".to_string(),
        ));
    }
    if let Some(level) = input.default_member_level.as_deref() {
        if !MEMBER_LEVELS.contains(&level) {
            return Err(ApiError::BadRequest(format!(
                "default_member_level must be one of {MEMBER_LEVELS:?}"
            )));
        }
        // `v0.4-flow-alpha.md` / `rest-api-v1.md`: "default_member_level 在 v0.5 授权面上线前只
        // 接受默认值 edit" — the column's own default is 'edit' and nothing in this version can
        // move it, so any other (otherwise-valid) level is rejected here rather than silently
        // ignored.
        if level != "edit" {
            return Err(ApiError::BadRequest(
                "default_member_level only accepts 'edit' before the v0.5 authorization surface ships".to_string(),
            ));
        }
    }
    let key_bytes = input.idempotency_key.len();
    if !(IDEMPOTENCY_KEY_MIN_BYTES..=IDEMPOTENCY_KEY_MAX_BYTES).contains(&key_bytes) {
        return Err(ApiError::BadRequest(format!(
            "idempotency_key must be {IDEMPOTENCY_KEY_MIN_BYTES}-{IDEMPOTENCY_KEY_MAX_BYTES} bytes"
        )));
    }
    Ok(())
}

/// `PUT /api/v1/workspaces/{workspace_id}/features/flow`.
///
/// Only `enabled` can actually change anything in v0.4 (`default_member_level` is validated above
/// to always already equal the column default); a change is `flow.feature.enabled`/
/// `flow.feature.disabled` (`events-v1.md`), written in the same transaction as the settings row
/// update and dispatched exactly like every other Flow business event
/// (`repository::insert_event_dispatch`). A request whose `enabled` already matches the current
/// value still succeeds and still stamps `updated_at`/`updated_by` (the caller's admin action is
/// real even when it changes nothing observable) but produces no event — see
/// [`FlowFeatureUpdateView`]'s doc comment.
pub async fn set_flow_feature(state: &AppState, input: SetFlowFeatureInput) -> Result<FlowFeatureUpdateView, ApiError> {
    validate_set_flow_feature(&input)?;

    if let Some(existing) =
        repository::find_idempotent_event(&state.db, input.workspace_id, &input.idempotency_key).await?
    {
        if existing.event_type != "flow.feature.enabled" && existing.event_type != "flow.feature.disabled" {
            return Err(ApiError::Conflict(
                "idempotency_key was already used for a different operation".to_string(),
            ));
        }
        let row = repository::fetch_flow_settings(&state.db, input.workspace_id).await?;
        return Ok(FlowFeatureUpdateView {
            feature: feature_view_from_row(row),
            event_id: Some(existing.id),
        });
    }

    let tx = state.db.begin().await?;

    repository::ensure_flow_settings_row(&tx, input.workspace_id).await?;
    let current = repository::fetch_flow_settings_for_update(&tx, input.workspace_id)
        .await?
        .ok_or(ApiError::Internal)?;

    let new_enabled = input.enabled.unwrap_or(current.flow_enabled);
    let transition = input.enabled.filter(|&enabled| enabled != current.flow_enabled);

    let event_id = if let Some(enabled) = transition {
        let event_type = if enabled {
            "flow.feature.enabled"
        } else {
            "flow.feature.disabled"
        };
        let dispatch_max_attempts = crate::config::runtime().flow.dispatch_max_attempts;
        let id = insert_flow_event(
            &tx,
            BusinessEventInput {
                workspace_id: input.workspace_id,
                project_id: None,
                event_type: event_type.to_string(),
                aggregate_type: "flow_feature".to_string(),
                aggregate_id: input.workspace_id.to_string(),
                actor_id: Some(input.actor_id),
                source: json!({ "surface": "rest" }),
                payload: json!({ "workspace_id": input.workspace_id }),
                metadata: json!({}),
                correlation_id: None,
                causation_id: None,
                idempotency_key: Some(input.idempotency_key.clone()),
            },
            Some(FlowDispatchSpec {
                max_attempts: dispatch_max_attempts,
                document_id: None,
                accepted_seq: None,
            }),
        )
        .await?
        .event_id;
        Some(id)
    } else {
        None
    };

    repository::update_flow_settings(&tx, input.workspace_id, new_enabled, input.actor_id).await?;
    tx.commit().await?;

    let updated = repository::fetch_flow_settings(&state.db, input.workspace_id).await?;
    Ok(FlowFeatureUpdateView {
        feature: feature_view_from_row(updated),
        event_id,
    })
}

/// Reconstructs the `AcceptedChange` an idempotent replay returns: the object's current view,
/// with `accepted_seq`/`projection_seq` read back from storage rather than re-derived, and
/// `event_id` pointing at the original event row rather than minting a new one — a replay must
/// report the same fact that already happened, not a second event.
fn accepted_change_from_row(row: repository::ObjectViewRow, event_id: Uuid) -> AcceptedChange {
    let view = super::query::object_view_from_row(row);
    AcceptedChange {
        accepted_seq: view.document_seq,
        head_frontier: view.frontier.clone(),
        projection_seq: view.projection_seq,
        semantic_diff: view.semantic_content.clone(),
        affected_object_ids: vec![view.id],
        event_id,
        command_result: None,
        object: view,
    }
}

// ---- `POST .../flow/objects/{object_id}/commands` ----

/// `NodeId`/`client_id`-style caller-supplied string bound (matches
/// `ticket::validate_client_id`'s convention for the same class of opaque caller-chosen string).
const NODE_ID_MAX_BYTES: usize = 256;

/// The five v0.4 content command types (`rest-api-v1.md`): each maps to one or more
/// [`collab_core::Operation`]s (or, for `set_title`, [`LoroCollabEngine::set_title`]) applied to an
/// isolated fork before being exported as CRDT update bytes and handed to the exact same
/// [`write::accept_update`] the WebSocket write path uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentCommandType {
    SetTitle,
    InsertBlock,
    UpdateBlock,
    DeleteBlock,
    MoveBlock,
    SemanticPatch,
}

impl ContentCommandType {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "set_title" => Some(Self::SetTitle),
            "insert_block" => Some(Self::InsertBlock),
            "update_block" => Some(Self::UpdateBlock),
            "delete_block" => Some(Self::DeleteBlock),
            "move_block" => Some(Self::MoveBlock),
            "semantic_patch" => Some(Self::SemanticPatch),
            _ => None,
        }
    }

    const fn wire_name(self) -> &'static str {
        match self {
            Self::SetTitle => "set_title",
            Self::InsertBlock => "insert_block",
            Self::UpdateBlock => "update_block",
            Self::DeleteBlock => "delete_block",
            Self::MoveBlock => "move_block",
            Self::SemanticPatch => "semantic_patch",
        }
    }

    /// `ADR-0013` §1's table: "所有内容编辑" → cardinality 1. Every content command loads and
    /// advances exactly the one existing `collab_documents` row `execute_content_command` reads
    /// via `bootstrap::load(&state.db, document_id)` — never zero (there is always a document to
    /// edit) and never more than one (v0.4 has no cross-document content command). Matched on
    /// `self` (rather than a bare constant) so a future content command variant with a different
    /// cardinality must edit this match, not silently inherit `One`.
    const fn existing_document_cardinality(self) -> ExistingDocumentCardinality {
        match self {
            Self::SetTitle
            | Self::InsertBlock
            | Self::UpdateBlock
            | Self::DeleteBlock
            | Self::MoveBlock
            | Self::SemanticPatch => ExistingDocumentCardinality::One,
        }
    }
}

/// The two v0.4 lifecycle command types. Neither advances a document head
/// (`existing_document_cardinality = 0`): a plain `flow_objects.lifecycle_status` transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LifecycleCommandType {
    Archive,
    Restore,
}

impl LifecycleCommandType {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "archive" => Some(Self::Archive),
            "restore" => Some(Self::Restore),
            _ => None,
        }
    }

    const fn wire_name(self) -> &'static str {
        match self {
            Self::Archive => "archive",
            Self::Restore => "restore",
        }
    }

    /// `ADR-0013` §1's table, first row: pure `flow_objects.lifecycle_status` governance, no
    /// `collab_documents` row touched at all.
    const fn existing_document_cardinality(self) -> ExistingDocumentCardinality {
        match self {
            Self::Archive | Self::Restore => ExistingDocumentCardinality::Zero,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandKind {
    Content(ContentCommandType),
    Lifecycle(LifecycleCommandType),
}

impl CommandKind {
    fn parse(raw: &str) -> Option<Self> {
        ContentCommandType::parse(raw)
            .map(Self::Content)
            .or_else(|| LifecycleCommandType::parse(raw).map(Self::Lifecycle))
    }

    /// The `events-v1.md` primary event type this command produces on success — also the
    /// expected `business_events.event_type` an idempotent replay of this `idempotency_key` must
    /// match (a caller reusing the same key for a different command type is `Conflict`, not a
    /// silent replay of the wrong operation).
    const fn event_type(self) -> &'static str {
        match self {
            Self::Content(_) => "flow.content.accepted",
            Self::Lifecycle(LifecycleCommandType::Archive) => "flow.object.archived",
            Self::Lifecycle(LifecycleCommandType::Restore) => "flow.object.restored",
        }
    }

    /// `rest-api-v1.md`'s archive-tier table (`ADR-0012` §2): a non-cascading, non-root
    /// archive/restore needs only `edit` — exactly the v0.4-frozen command set's first row. A
    /// `navigator` (root) object needs `full_access`; v0.4 ships no `flow_object_grants`, so in
    /// practice only a workspace admin (who always holds `full_access` via
    /// `authz::effective_permission`'s own admin override) can archive/restore one, matching
    /// "第二、三行的 `full_access` 由 workspace admin 承担".
    fn required_permission_level(self, object_type: &str) -> authz::PermissionLevel {
        match self {
            Self::Lifecycle(_) if object_type == "navigator" => authz::PermissionLevel::FullAccess,
            _ => authz::PermissionLevel::Edit,
        }
    }
}

pub struct ExecuteCommandInput {
    pub object_id: Uuid,
    pub actor_id: Uuid,
    /// `"user"` or `"bot"` (matches `flow_object_grants.principal_kind`/
    /// `authz::effective_permission`'s `principal_kind` parameter).
    pub principal_kind: String,
    /// The caller's `workspace_members.role` (or bot-synthesized role) — see
    /// `middleware::bot_auth::require_workspace_access`.
    pub role: String,
    pub command_type: String,
    pub payload: Value,
    /// Base64 `Frontier` bytes, when the caller wants strict optimistic-concurrency locking
    /// instead of the CRDT's default commutative merge (`write::UpdateRequest::expected_frontier`).
    /// Only valid for the six content command types; `archive`/`restore` reject a non-`None` value
    /// (they never advance a document head, so it could never be honored).
    pub expected_frontier: Option<String>,
    pub idempotency_key: String,
    pub message: Option<String>,
    /// A caller identity string stamped onto `collab_updates.origin_client_id` and onto the
    /// `update` frame relayed to any WebSocket sessions with this document open — this surface has
    /// no `client_id` handshake like the WebSocket ticket flow, so the REST layer synthesizes one
    /// (see `routes::flow::post_flow_object_command`).
    pub origin_client_id: String,
}

fn validate_execute_command_input(input: &ExecuteCommandInput) -> Result<(), ApiError> {
    let key_bytes = input.idempotency_key.len();
    if !(IDEMPOTENCY_KEY_MIN_BYTES..=IDEMPOTENCY_KEY_MAX_BYTES).contains(&key_bytes) {
        return Err(ApiError::BadRequest(format!(
            "idempotency_key must be {IDEMPOTENCY_KEY_MIN_BYTES}-{IDEMPOTENCY_KEY_MAX_BYTES} bytes"
        )));
    }
    if let Some(message) = &input.message
        && message.chars().count() > MESSAGE_MAX_CHARS
    {
        return Err(ApiError::BadRequest(format!(
            "message must be at most {MESSAGE_MAX_CHARS} characters"
        )));
    }
    Ok(())
}

/// Writes the audit-only `flow.command.rejected` event (`events-v1.md` "Event type registry":
/// `aggregate_type=flow_command`, "命令在创建 job 前被拒绝时只产生 audit-only `flow.command.
/// rejected`"). `insert_flow_event`'s `dispatch: None` is the `delivery_class=audit_only` half of
/// that helper: the row is written but never gets an `event_dispatch` row, so it never reaches a
/// webhook subscriber ("永不产生 dispatch work 与投递行、不触发 webhook").
///
/// Always called against `state.db` directly, never inside the rejected command's own
/// transaction: `events-v1.md` — "失败的 domain transaction 不得留下 success event 与 dispatch
/// work...；rejected/audit-only row 在失败 transaction 回滚后以安全摘要单独写入" — every caller
/// below has already rolled back (or never opened) that transaction by the time this runs.
///
/// Only called once `workspace_id` is known, i.e. after the target object has been resolved: a
/// rejection that happens before that point (malformed request shape, an unregistered
/// `command.type`, or the object simply not existing) has no workspace to scope an audit row
/// under, and the REST error response itself remains the caller's complete signal for those.
///
/// Never surfaces a failure to the caller — `events-v1.md` requires "写 audit 失败必须告警", not
/// that a command's own (already-decided) rejection be replaced by a second, unrelated database
/// error — so a failure here is only logged.
async fn record_command_rejected(
    state: &AppState,
    workspace_id: Uuid,
    actor_id: Uuid,
    action: &str,
    error: &ApiError,
    object_id: Uuid,
    document_id: Uuid,
) {
    let error_code = error.kind().stable_code();
    let result = insert_flow_event(
        &state.db,
        BusinessEventInput {
            workspace_id,
            project_id: None,
            event_type: "flow.command.rejected".to_string(),
            aggregate_type: "flow_command".to_string(),
            // `events-v1.md`: "aggregate id 是服务端 request_id 或 update_id" — this command never
            // minted one before being rejected, so a fresh id is synthesized for this audit row
            // (never the caller's `idempotency_key`, which stays reserved for a future successful
            // retry of the same request).
            aggregate_id: Uuid::new_v4().to_string(),
            actor_id: Some(actor_id),
            source: json!({ "surface": "rest" }),
            payload: json!({
                "action": action,
                "error_code": error_code,
                "object_id": object_id,
                "document_id": document_id,
            }),
            metadata: json!({}),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
        None,
    )
    .await;
    if let Err(err) = result {
        tracing::error!(
            error = %err,
            action,
            error_code,
            "failed to record flow.command.rejected audit event"
        );
    }
}

/// `POST /api/v1/flow/objects/{object_id}/commands`.
///
/// # Errors
/// `NotFound` if the object does not exist; `BadRequest` for an unregistered `command.type` or a
/// malformed payload (`invalid_update` on the wire); a typed `policy_rejected` for insufficient
/// permission or a commit-time `authz_epoch` mismatch; `Conflict` for an `idempotency_key` reused
/// with a different command or a redundant `archive`; a typed `stale_frontier`/`resync_required`/
/// `server_draining`/`limit_exceeded`/`invalid_update` rejection from the shared write path.
/// Propagates a database failure otherwise. Every rejection reached once `object_id` resolves to a
/// real object also records an audit-only `flow.command.rejected` event (see
/// [`record_command_rejected`]).
pub async fn execute_command(state: &AppState, input: ExecuteCommandInput) -> Result<AcceptedChange, ApiError> {
    validate_execute_command_input(&input)?;

    let kind = CommandKind::parse(&input.command_type).ok_or_else(|| {
        ApiError::invalid_update(format!(
            "command.type '{}' is not a registered v0.4 command",
            input.command_type
        ))
    })?;
    if matches!(kind, CommandKind::Content(ContentCommandType::SemanticPatch)) {
        check_semantic_patch_json_bytes(&input.payload)?;
    }

    let view_row = repository::fetch_object_view(&state.db, input.object_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    let workspace_id = view_row.workspace_id;
    let document_id = view_row.document_id;
    let object_type = view_row.object_type.clone();
    runtime::runtime().ensure_workspace_accepting(workspace_id)?;

    match execute_command_authorized(state, &input, kind, workspace_id, document_id, &object_type).await {
        Ok(change) => Ok(change),
        Err(err) => {
            record_command_rejected(
                state,
                workspace_id,
                input.actor_id,
                &input.command_type,
                &err,
                input.object_id,
                document_id,
            )
            .await;
            Err(err)
        }
    }
}

/// The idempotency-replay check, permission check, and command dispatch that make up
/// `execute_command`'s body once `workspace_id`/`document_id`/`object_type` are known — split out
/// so [`execute_command`] can wrap every `Err` this returns with [`record_command_rejected`]
/// without duplicating that wrapping at each individual early return.
async fn execute_command_authorized(
    state: &AppState,
    input: &ExecuteCommandInput,
    kind: CommandKind,
    workspace_id: Uuid,
    document_id: Uuid,
    object_type: &str,
) -> Result<AcceptedChange, ApiError> {
    if let Some(existing) = repository::find_idempotent_event(&state.db, workspace_id, &input.idempotency_key).await? {
        if existing.event_type != kind.event_type() {
            return Err(ApiError::Conflict(
                "idempotency_key was already used for a different operation".to_string(),
            ));
        }
        let current = repository::fetch_object_view(&state.db, input.object_id)
            .await?
            .ok_or(ApiError::Internal)?;
        return Ok(accepted_change_from_row(current, existing.id));
    }

    let principal_kind = if input.principal_kind == "bot" { "bot" } else { "user" };
    let level = authz::effective_permission(
        &state.db,
        workspace_id,
        input.object_id,
        principal_kind,
        input.actor_id,
        &input.role,
    )
    .await?;
    if level < kind.required_permission_level(object_type) {
        return Err(ApiError::policy_rejected("insufficient permission for this command"));
    }

    match kind {
        CommandKind::Content(content_kind) => {
            execute_content_command(state, input, workspace_id, document_id, content_kind).await
        }
        CommandKind::Lifecycle(lifecycle_kind) => {
            execute_lifecycle_command(state, input, workspace_id, lifecycle_kind).await
        }
    }
}

#[derive(Debug, Deserialize)]
struct SetTitlePayload {
    title: String,
}

#[derive(Debug, Deserialize)]
struct InsertBlockPayload {
    block_id: String,
    #[serde(default)]
    parent_block_id: Option<String>,
    #[serde(default)]
    index: u32,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateBlockPayload {
    block_id: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    properties: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct DeleteBlockPayload {
    block_id: String,
}

#[derive(Debug, Deserialize)]
struct MoveBlockPayload {
    block_id: String,
    #[serde(default)]
    parent_block_id: Option<String>,
    #[serde(default)]
    index: u32,
}

#[derive(Debug, Deserialize)]
struct SemanticPatchPayload {
    operations: Vec<Operation>,
}

/// Enforces the serialized semantic-patch payload ceiling before the target object is read and,
/// crucially, before any canonical/audit/event row can be written. The parsed JSON value is
/// re-serialized compactly because every REST/MCP/CLI/Web producer reaches this application
/// service after JSON decoding; insignificant transport whitespace is not semantic patch data.
fn check_semantic_patch_json_bytes(payload: &Value) -> Result<(), ApiError> {
    let bytes =
        serde_json::to_vec(payload).map_err(|_| ApiError::invalid_update("semantic_patch is not valid JSON"))?;
    let observed = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if observed > collab_limits::SEMANTIC_PATCH_JSON_BYTES_MAX {
        return Err(ApiError::limit_exceeded(
            "semantic patch JSON exceeds the fixed byte ceiling",
            "semantic_patch_bytes",
            Some(json!(collab_limits::SEMANTIC_PATCH_JSON_BYTES_MAX)),
            Some(json!(observed)),
            None,
        ));
    }
    Ok(())
}

fn parse_node_id(raw: &str, field: &str) -> Result<NodeId, ApiError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > NODE_ID_MAX_BYTES {
        return Err(ApiError::BadRequest(format!(
            "{field} must be 1-{NODE_ID_MAX_BYTES} non-whitespace-only characters"
        )));
    }
    Ok(NodeId::from(trimmed))
}

fn parse_payload<T: serde::de::DeserializeOwned>(command_type: &str, payload: &Value) -> Result<T, ApiError> {
    serde_json::from_value(payload.clone())
        .map_err(|err| ApiError::BadRequest(format!("invalid payload for {command_type}: {err}")))
}

/// Maps a [`CollabError`] to the typed `invalid_update`/`limit_exceeded` [`ApiErrorKind`]s
/// (`error-mapping-v1.md`: both REST `BadRequest`/400, but distinct stable codes — never folded
/// into one string-typed `BadRequest` the caller would have to substring-match, which is exactly
/// the anti-pattern this typed discriminant replaces). Never a raw `{:?}` dump of engine
/// internals — each arm produces a caller-safe message naming only the logical node id or limit
/// kind, and `limit_exceeded` carries its `limit_kind`/`limit`/`observed` as structured `details`
/// instead of interpolated into the message text.
fn map_collab_error(err: &CollabError) -> ApiError {
    match err {
        CollabError::UnknownNode { id } => ApiError::invalid_update(format!("unknown node id '{id}'")),
        CollabError::DuplicateNode { id } => ApiError::invalid_update(format!("duplicate node id '{id}'")),
        CollabError::CycleRejected { id } => {
            ApiError::invalid_update(format!("move of node '{id}' rejected: would create a cycle"))
        }
        // `CollabError::limit_kind()` special-cases this exact shape as the `update_bytes` limit
        // (an oversized snapshot/update byte slice); every other `InputTooLarge`/`EmptyInput`/
        // `DecodeFailed` shape falls through to the generic `invalid_update` arm below.
        CollabError::InputTooLarge {
            input: "update",
            actual_bytes,
            max_bytes,
        } => ApiError::limit_exceeded(
            "limit_exceeded: update_bytes",
            "update_bytes",
            Some(json!(max_bytes)),
            Some(json!(actual_bytes)),
            None,
        ),
        CollabError::EmptyInput { .. } | CollabError::InputTooLarge { .. } | CollabError::DecodeFailed { .. } => {
            ApiError::invalid_update("invalid_update")
        }
        CollabError::OperationFailed { reason } => ApiError::invalid_update(format!("invalid_update: {reason}")),
        CollabError::LimitExceeded {
            limit_kind,
            limit,
            observed,
        } => ApiError::limit_exceeded(
            format!("limit_exceeded: {limit_kind}"),
            limit_kind,
            Some(json!(limit)),
            Some(json!(observed)),
            None,
        ),
    }
}

/// Applies one content command's payload to `engine` (already forked, mutated in place). Every
/// operation this dispatches through is `apply_operation`'s shared vocabulary
/// (`crates/collab-core/src/engine.rs`) — no command type invents a second mutation path.
///
/// `set_title` aside (which never goes through the `Operation` vocabulary at all —
/// [`LoroCollabEngine::set_title`] is a document-level field, not a tree op, and `TITLE_MAX_CHARS`
/// already bounds it), every other command type first materializes the *complete, ordered* list
/// of [`Operation`]s the payload requires — `update_block`'s caller-supplied `properties` map is
/// the one case in this handler whose operation count is not fixed by the command shape itself,
/// so it is exactly the case `semantic_patch_operations_max` exists to bound. That full list is
/// checked with [`collab_core::limits::check_operation_batch_count`] *before* a single operation
/// is applied to `engine` — `limits-v1.md`'s "reject the whole patch atomically, never a partial
/// prefix" — then each operation is checked with [`collab_core::limits::check_operation`] against
/// the snapshot immediately before it (so a `create_node` that would make a second `create_node`
/// in the same batch exceed `container_count`, for example, is still caught) right before it is
/// applied. `engine` here is always a fresh, request-local fork (`hydrate_and_apply`'s caller in
/// `execute_content_command`) that is simply dropped on any `Err` return — a batch-count or
/// per-operation rejection therefore never reaches `engine.export_from`/`write::accept_update`, so
/// it can never advance a document head or produce a business event/`event_dispatch` row.
fn apply_content_command(
    engine: &mut LoroCollabEngine,
    kind: ContentCommandType,
    payload: &Value,
) -> Result<(), ApiError> {
    let ops: Vec<Operation> = match kind {
        ContentCommandType::SetTitle => {
            let payload: SetTitlePayload = parse_payload("set_title", payload)?;
            let title = payload.title.trim();
            if title.is_empty() {
                return Err(ApiError::BadRequest("title must not be empty".to_string()));
            }
            if title.chars().count() > TITLE_MAX_CHARS {
                return Err(ApiError::BadRequest(format!(
                    "title must be at most {TITLE_MAX_CHARS} characters"
                )));
            }
            return engine.set_title(title).map_err(|err| map_collab_error(&err));
        }
        ContentCommandType::InsertBlock => {
            let payload: InsertBlockPayload = parse_payload("insert_block", payload)?;
            let id = parse_node_id(&payload.block_id, "block_id")?;
            let parent = payload
                .parent_block_id
                .as_deref()
                .map(|raw| parse_node_id(raw, "parent_block_id"))
                .transpose()?;
            let mut ops = vec![Operation::CreateNode {
                id: id.clone(),
                parent,
                index: payload.index,
                kind: NodeKind::Block,
            }];
            if let Some(text) = payload.text.filter(|text| !text.is_empty()) {
                ops.push(Operation::InsertText { id, index: 0, text });
            }
            ops
        }
        ContentCommandType::UpdateBlock => {
            let payload: UpdateBlockPayload = parse_payload("update_block", payload)?;
            let id = parse_node_id(&payload.block_id, "block_id")?;
            if payload.text.is_none() && payload.properties.is_empty() {
                return Err(ApiError::BadRequest(
                    "update_block requires at least one of text or properties".to_string(),
                ));
            }
            let mut ops = Vec::new();
            if let Some(text) = payload.text {
                let existing = engine.semantic_snapshot().map_err(|err| map_collab_error(&err))?;
                let node = existing
                    .nodes
                    .get(&id)
                    .ok_or_else(|| ApiError::BadRequest(format!("unknown node id '{id}'")))?;
                let old_len = u32::try_from(node.text.len())
                    .map_err(|_| ApiError::BadRequest("block text too large to update".to_string()))?;
                if old_len > 0 {
                    ops.push(Operation::DeleteText {
                        id: id.clone(),
                        index: 0,
                        len: old_len,
                    });
                }
                if !text.is_empty() {
                    ops.push(Operation::InsertText {
                        id: id.clone(),
                        index: 0,
                        text,
                    });
                }
            }
            for (key, value) in payload.properties {
                ops.push(Operation::SetProperty {
                    id: id.clone(),
                    key,
                    value,
                });
            }
            ops
        }
        ContentCommandType::DeleteBlock => {
            let payload: DeleteBlockPayload = parse_payload("delete_block", payload)?;
            let id = parse_node_id(&payload.block_id, "block_id")?;
            vec![Operation::DeleteNode { id }]
        }
        ContentCommandType::MoveBlock => {
            let payload: MoveBlockPayload = parse_payload("move_block", payload)?;
            let id = parse_node_id(&payload.block_id, "block_id")?;
            let new_parent = payload
                .parent_block_id
                .as_deref()
                .map(|raw| parse_node_id(raw, "parent_block_id"))
                .transpose()?;
            vec![Operation::MoveNode {
                id,
                new_parent,
                index: payload.index,
            }]
        }
        ContentCommandType::SemanticPatch => {
            let payload: SemanticPatchPayload = parse_payload("semantic_patch", payload)?;
            if payload.operations.is_empty() {
                return Err(ApiError::invalid_update(
                    "semantic_patch requires at least one operation",
                ));
            }
            payload.operations
        }
    };

    let limits = collab_limits::document_limits();
    collab_core::limits::check_operation_batch_count(ops.len(), &limits)
        .map_err(|violation| map_collab_error(&CollabError::from(violation)))?;

    for op in ops {
        let snapshot = engine.semantic_snapshot().map_err(|err| map_collab_error(&err))?;
        collab_core::limits::check_operation(&snapshot, &op, &limits)
            .map_err(|violation| map_collab_error(&CollabError::from(violation)))?;
        engine.apply_operation(&op).map_err(|err| map_collab_error(&err))?;
    }
    Ok(())
}

/// `error-mapping-v1.md`'s stable-code → REST mapping, applied to a rejection from the shared
/// write path (`write::accept_update`) exactly the way the WebSocket layer would report it on the
/// wire, just carried through `ApiError` instead of a `Frame::Rejected`.
///
/// Every arm now carries an exact [`ApiErrorKind`] discriminant instead of a plain `BadRequest`/
/// `Conflict` string — the same class of gap the module's own doc comment on
/// [`ApiError::Typed`]-style constructors calls out: a caller previously had to substring-match
/// `"limit_exceeded: ..."`/`"server_draining"` out of the message text, which could not
/// distinguish `server_draining`'s `drain` from `contention` reason at all (both collapsed into
/// one `Conflict("server_draining")`). `rejected.details`' structured fields (`limit_kind`/
/// `limit`/`observed`/`retry_after_ms`/`minimum_snapshot_seq`) are now carried through as typed
/// `details` rather than interpolated into the message.
fn map_write_rejection(rejected: &write::Rejected) -> ApiError {
    use super::collab::frame::RejectedCode;

    // Hoisted once so every arm below reads the same `Option<&Value>` instead of each
    // re-deriving `rejected.details.as_ref()` independently -- this is also the only real
    // caller-facing REST site that ever inspects `Rejected::details` at all, so it is where
    // `error-mapping-v1.md`'s "REST 必须能读出 limit_kind/limit/observed" requirement is either
    // honored or silently dropped.
    let details = rejected.details.as_ref();

    match rejected.code {
        RejectedCode::Unauthenticated => ApiError::unauthenticated("unauthenticated"),
        RejectedCode::Forbidden => ApiError::typed(ApiErrorKind::Forbidden, "forbidden"),
        RejectedCode::FeatureDisabled => ApiError::feature_disabled("feature_disabled"),
        RejectedCode::NotFound => ApiError::NotFound("not_found".to_string()),
        RejectedCode::UnsupportedProtocol => ApiError::unsupported_protocol("unsupported_protocol"),
        RejectedCode::InvalidUpdate => ApiError::invalid_update("invalid_update"),
        RejectedCode::PolicyRejected => ApiError::policy_rejected("policy_rejected"),
        RejectedCode::StaleFrontier => {
            let current_frontier = rejected.current_frontier.as_deref().map(frame::encode_bytes);
            ApiError::stale_frontier("stale_frontier", rejected.current_seq, current_frontier.as_deref())
        }
        RejectedCode::ResyncRequired => {
            let minimum_snapshot_seq = details
                .and_then(|details| details.get("minimum_snapshot_seq"))
                .and_then(Value::as_i64);
            ApiError::resync_required("resync_required", minimum_snapshot_seq)
        }
        RejectedCode::LimitExceeded => {
            let (limit_kind, limit, observed, retry_after_ms) =
                details.map_or(("unknown", None, None, None), |details| {
                    (
                        details.get("limit_kind").and_then(Value::as_str).unwrap_or("unknown"),
                        details.get("limit").cloned(),
                        details.get("observed").cloned(),
                        details.get("retry_after_ms").and_then(Value::as_u64),
                    )
                });
            ApiError::limit_exceeded(
                format!("limit_exceeded: {limit_kind}"),
                limit_kind,
                limit,
                observed,
                retry_after_ms,
            )
        }
        RejectedCode::ServerDraining => {
            let reason = details
                .and_then(|details| details.get("reason"))
                .and_then(Value::as_str);
            let retry_after_ms = details
                .and_then(|details| details.get("retry_after_ms"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            match reason {
                Some("drain") => {
                    ApiError::server_draining(ServerDrainingReason::Drain, retry_after_ms, "server_draining")
                }
                Some("contention") => {
                    ApiError::server_draining(ServerDrainingReason::Contention, retry_after_ms, "server_draining")
                }
                // `error-mapping-v1.md`: "缺失/未知 reason 是 producer contract violation... 不得
                // 猜测为维护或竞争、不得用 message 补判" -- the write path never producing a
                // recognized reason here is a bug in that path, not something to paper over with a
                // guessed discriminant; the caller still gets a safe, generically-retryable
                // rejection instead of a fabricated `ServerDraining` reason.
                other => {
                    tracing::error!(
                        reason = ?other,
                        "write path rejected with server_draining but no recognized reason; this is a producer contract violation"
                    );
                    ApiError::Conflict("server_draining".to_string())
                }
            }
        }
    }
}

/// Builds the CRDT update bytes for one content command (isolated fork, outside any lock — matches
/// `write::hydrate_and_apply`'s own discipline for exactly this reason: neither path may hold a
/// lock across a CRDT apply), then submits them through [`write::accept_update`] — the identical
/// function `flow::collab::session::handle_client_frame` calls for a WebSocket `update` frame.
/// `accept_update` itself broadcasts the resulting `update`+`accepted` frame pair to this
/// document's WebSocket sessions (`collab-protocol-v1.md`: "复用 ADR-0010 的 ordered egress /
/// invalidation 通道，不另起一套" — this call and the WebSocket path share that one broadcast call
/// site inside `accept_update`, not a second one here).
async fn execute_content_command(
    state: &AppState,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    document_id: Uuid,
    kind: ContentCommandType,
) -> Result<AcceptedChange, ApiError> {
    let expected_frontier = input
        .expected_frontier
        .as_deref()
        .map(frame::decode_bytes)
        .transpose()
        .map_err(|_| ApiError::BadRequest("expected_frontier is not valid base64".to_string()))?;

    let boot = bootstrap::load(&state.db, document_id).await?;
    let mut engine = LoroCollabEngine::load(&boot.snapshot).map_err(|err| map_collab_error(&err))?;
    for tail in &boot.tail_updates {
        engine
            .import_update(&tail.bytes)
            .map_err(|err| map_collab_error(&err))?;
    }
    let base_frontier = engine.frontier();
    apply_content_command(&mut engine, kind, &input.payload)?;
    let update_bytes = engine
        .export_from(&base_frontier)
        .map_err(|err| map_collab_error(&err))?;

    let collab = runtime::runtime();
    let checked_epoch = authz::read_epoch(&state.db, workspace_id).await?;
    let update_id = Uuid::new_v4();

    let outcome = write::accept_update(
        &state.db,
        &collab.cache,
        &collab.coordinator,
        &collab.registry,
        &collab.snapshot,
        crate::config::runtime().flow.dispatch_max_attempts,
        None,
        write::UpdateRequest {
            document_id,
            update_id,
            bytes: update_bytes,
            idempotency_key: Some(input.idempotency_key.clone()),
            origin_client_id: Some(input.origin_client_id.clone()),
            message: input.message.clone(),
            actor_id: input.actor_id,
            workspace_id,
            checked_epoch,
            expected_frontier,
        },
    )
    .await?;

    let accepted = match outcome {
        write::AcceptOutcome::Accepted(accepted) => accepted,
        write::AcceptOutcome::Rejected(rejected) => return Err(map_write_rejection(&rejected)),
    };
    if accepted.should_advance_snapshot {
        crate::flow::collab::snapshot::spawn_background(&collab.snapshot, state.db.clone(), document_id);
    }
    // `write::accept_update` already broadcast the `update`+`accepted` frame pair to every
    // WebSocket session with `document_id` open (this REST caller has no session_id of its own to
    // exclude, so `None` above means every open session receives it) -- see that function's doc
    // comment for why this is its one broadcast call site, not a second one here.

    let view = repository::fetch_object_view(&state.db, input.object_id)
        .await?
        .ok_or(ApiError::Internal)?;
    let object = super::query::object_view_from_row(view);
    Ok(AcceptedChange {
        accepted_seq: accepted.head_seq,
        head_frontier: object.frontier.clone(),
        projection_seq: accepted.projection_seq,
        semantic_diff: object.semantic_content.clone(),
        affected_object_ids: vec![input.object_id],
        event_id: accepted.event_id,
        command_result: None,
        object,
    })
}

/// `archive`/`restore`: a plain `flow_objects.lifecycle_status` transition, no document
/// coordinator and no document row lock (this command's `existing_document_cardinality = 0` —
/// see this module's own doc comment) — but it *does* take the commit-time `authz_epoch` fence
/// (`ADR-0012` §3.1) below, on the same `flow_workspace_settings` row `execute_content_command`
/// fences against, held to this transaction's own commit. Without it, the TOCTOU window
/// `collab-protocol-v1.md` names for content writes applies here too: `execute_command`'s caller
/// computes `authz::effective_permission` once, outside any transaction; a concurrent
/// authorization change (a v0.5 grant revoke) that commits between that check and this
/// transaction's own commit must not let this now-stale permission still land an
/// archive/restore. `restore` is idempotent at the row level (setting an already-active object
/// active again is a harmless no-op write, not an error); `archive` on an already-archived object
/// is a real conflict — a caller retrying with a *new* `idempotency_key` after losing the
/// original response should not silently succeed a second time.
async fn execute_lifecycle_command(
    state: &AppState,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    kind: LifecycleCommandType,
) -> Result<AcceptedChange, ApiError> {
    if input.expected_frontier.is_some() {
        return Err(ApiError::BadRequest(
            "expected_frontier is not accepted for archive/restore: this command never advances a document head"
                .to_string(),
        ));
    }

    let (event_type, target_status) = match kind {
        LifecycleCommandType::Archive => ("flow.object.archived", "archived"),
        LifecycleCommandType::Restore => ("flow.object.restored", "active"),
    };

    // The `authz_epoch` `execute_command`'s own `authz::effective_permission` check (the caller
    // of this function) was computed against, re-read here immediately before opening the
    // transaction that will commit this lifecycle change -- matches
    // `execute_content_command`'s own `checked_epoch` read right before its write path.
    let checked_epoch = authz::read_epoch(&state.db, workspace_id).await?;

    let tx = state.db.begin().await?;

    // Commit-time fencing barrier (`ADR-0012` §3.1), held to commit -- previously entirely
    // missing for archive/restore (this package's own doc comment above used to justify that gap
    // by "archive/restore never advance a document head, so they take no `authz_epoch` fence", but
    // the fence's job is not document-head consistency, it is authorization freshness: without
    // it, a `full_access` grant revoked by a concurrent authorization change after this
    // function's caller already checked `effective_permission` could still land as a committed
    // `archived`/`restored` transition). Reuses `fence_epoch_for_share` exactly as
    // `write::run_locked_phase` does for content commands: only a genuine epoch mismatch
    // (`ApiError::Conflict`) means the caller's permission is stale; any other error (a
    // `lock_timeout` hit while waiting on the `FOR SHARE`, a dropped connection, ...) says
    // nothing about authorization and must propagate as a real `Err` rather than being folded
    // into a permanent, non-recoverable rejection -- the exact class of bug `12700c4` fixed for
    // the content write path, which this lifecycle path must not reintroduce.
    match authz::fence_epoch_for_share(&tx, workspace_id, checked_epoch).await {
        Ok(()) => {}
        Err(ApiError::Conflict(_)) => {
            let _ = tx.rollback().await;
            return Err(ApiError::policy_rejected(
                "authz_epoch advanced since permission was checked; command rejected",
            ));
        }
        Err(err) => {
            let _ = tx.rollback().await;
            return Err(err);
        }
    }

    let locked = repository::fetch_object_lifecycle_for_update(&tx, input.object_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;

    if kind == LifecycleCommandType::Archive && locked.lifecycle_status == "archived" {
        let _ = tx.rollback().await;
        return Err(ApiError::Conflict("object is already archived".to_string()));
    }

    let archived_at = if target_status == "archived" {
        Some(chrono::Utc::now())
    } else {
        None
    };
    repository::set_object_lifecycle(&tx, input.object_id, target_status, archived_at, input.actor_id).await?;

    let dispatch_max_attempts = crate::config::runtime().flow.dispatch_max_attempts;
    let event_id = insert_flow_event(
        &tx,
        BusinessEventInput {
            workspace_id,
            project_id: None,
            event_type: event_type.to_string(),
            aggregate_type: "flow_object".to_string(),
            aggregate_id: input.object_id.to_string(),
            actor_id: Some(input.actor_id),
            source: json!({ "surface": "rest" }),
            payload: json!({ "object_id": input.object_id, "status": target_status }),
            metadata: json!({ "message": input.message }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: Some(input.idempotency_key.clone()),
        },
        Some(FlowDispatchSpec {
            max_attempts: dispatch_max_attempts,
            document_id: None,
            accepted_seq: None,
        }),
    )
    .await?
    .event_id;

    tx.commit().await?;

    let view = repository::fetch_object_view(&state.db, input.object_id)
        .await?
        .ok_or(ApiError::Internal)?;
    Ok(accepted_change_from_row(view, event_id))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod cardinality_gate_tests {
    use super::{ExistingDocumentCardinality, v0_4_command_cardinality_registry};

    /// `command_contended_document_cardinality` (`ADR-0013` §1, v0.4): "v0.4 的竞争文档集合恒
    /// ≤ 1". Every command this package registers — content, lifecycle, and the two
    /// non-`CommandKind` write paths (`create_object`, `set_flow_feature`) — must declare a
    /// cardinality of at most 1; a future command that needs `BoundedMany` must fail this test
    /// until it also ships the `ADR-0013` §2 multi-document lock-order machinery, not slip in
    /// silently.
    #[test]
    fn v0_4_command_set_existing_document_cardinality_is_always_at_most_one() {
        let registry = v0_4_command_cardinality_registry();
        assert!(!registry.is_empty(), "the v0.4 command registry must not be empty");
        for (name, cardinality) in registry {
            assert!(
                cardinality.count() <= 1,
                "command '{name}' declares existing_document_cardinality={cardinality:?} \
                 (count={}), violating ADR-0013's v0.4 bound of <= 1",
                cardinality.count()
            );
        }
    }

    #[test]
    fn v0_4_registry_covers_every_registered_command_name() {
        let registry = v0_4_command_cardinality_registry();
        let names: Vec<&str> = registry.iter().map(|(name, _)| *name).collect();
        for expected in [
            "create_object",
            "set_flow_feature",
            "set_title",
            "insert_block",
            "update_block",
            "delete_block",
            "move_block",
            "semantic_patch",
            "archive",
            "restore",
        ] {
            assert!(
                names.contains(&expected),
                "command '{expected}' is missing from the cardinality registry"
            );
        }
        assert_eq!(names.len(), 10, "registry must not silently gain or lose commands");
    }

    #[test]
    fn cardinality_count_matches_each_variant() {
        assert_eq!(ExistingDocumentCardinality::Zero.count(), 0);
        assert_eq!(ExistingDocumentCardinality::One.count(), 1);
        assert_eq!(ExistingDocumentCardinality::BoundedMany(3).count(), 3);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod typed_error_mapping_tests {
    use collab_core::CollabError;
    use serde_json::{Value, json};

    use super::{check_semantic_patch_json_bytes, map_collab_error, map_write_rejection};
    use crate::error::{ApiError, ApiErrorKind, ServerDrainingReason};
    use crate::flow::collab::frame::RejectedCode;
    use crate::flow::collab::write::Rejected;

    fn rejected(code: RejectedCode, details: Option<Value>) -> Rejected {
        Rejected {
            update_id: None,
            code,
            recoverable: false,
            details,
            current_seq: None,
            current_frontier: None,
        }
    }

    fn semantic_patch_payload_with_serialized_bytes(target: u64) -> Value {
        let empty = json!({"operations": [], "padding": ""});
        let base = u64::try_from(serde_json::to_vec(&empty).expect("serializes").len()).expect("fits");
        let padding = usize::try_from(target - base).expect("target fits usize");
        json!({"operations": [], "padding": "x".repeat(padding)})
    }

    #[test]
    fn semantic_patch_bytes_exact_boundary_is_accepted_and_plus_one_is_rejected_before_writes() {
        let limit = crate::flow::collab::limits::SEMANTIC_PATCH_JSON_BYTES_MAX;
        let exact = semantic_patch_payload_with_serialized_bytes(limit);
        assert_eq!(
            u64::try_from(serde_json::to_vec(&exact).expect("serializes").len()).expect("fits"),
            limit
        );
        assert!(check_semantic_patch_json_bytes(&exact).is_ok());

        let plus_one = semantic_patch_payload_with_serialized_bytes(limit + 1);
        let err = check_semantic_patch_json_bytes(&plus_one).expect_err("one byte over must reject");
        assert_eq!(err.kind(), ApiErrorKind::LimitExceeded);
        let ApiError::Typed { details, .. } = err else {
            panic!("limit rejection must be typed");
        };
        let details = details.expect("limit rejection carries details");
        assert_eq!(details["limit_kind"], "semantic_patch_bytes");
        assert_eq!(details["limit"], limit);
        assert_eq!(details["observed"], limit + 1);
    }

    /// `error-mapping-v1.md`'s central invariant this package's typed discriminant exists to
    /// enforce: `server_draining`'s two reasons must never collapse into the same `ApiErrorKind`
    /// -- the exact regression `12700c4` fixed once for the epoch-fence path.
    #[test]
    fn server_draining_drain_and_contention_map_to_distinct_kinds() {
        let drain = map_write_rejection(&rejected(
            RejectedCode::ServerDraining,
            Some(json!({ "reason": "drain", "retry_after_ms": 500 })),
        ));
        let contention = map_write_rejection(&rejected(
            RejectedCode::ServerDraining,
            Some(json!({ "reason": "contention", "retry_after_ms": 200 })),
        ));

        assert_eq!(drain.kind(), ApiErrorKind::ServerDraining(ServerDrainingReason::Drain));
        assert_eq!(
            contention.kind(),
            ApiErrorKind::ServerDraining(ServerDrainingReason::Contention)
        );
        assert_ne!(drain.kind(), contention.kind());

        // The two reasons share one CLI exit code (contract: "两种 reason 不拆退出码") but must
        // use different WS close-code behavior: `drain` closes the connection (4410), `contention`
        // never does (a `rejected` control frame keeps the connection open).
        assert_eq!(drain.kind().cli_exit_code(), 9);
        assert_eq!(contention.kind().cli_exit_code(), 9);
        assert_eq!(drain.kind().ws_close_code(), Some(4410));
        assert_eq!(contention.kind().ws_close_code(), None);
        assert_ne!(drain.kind().ui_key(), contention.kind().ui_key());
    }

    /// `error-mapping-v1.md`: "缺失/未知 reason 是 producer contract violation... 不得猜测为维护或
    /// 竞争". A missing/unrecognized `reason` must never be silently coerced into `Drain` or
    /// `Contention` -- it must fall back to a generic, `Unclassified`-kind rejection instead of
    /// fabricating a discriminant the producer never actually sent.
    #[test]
    fn server_draining_with_missing_reason_never_fabricates_a_discriminant() {
        let missing_reason = map_write_rejection(&rejected(RejectedCode::ServerDraining, None));
        let unknown_reason = map_write_rejection(&rejected(
            RejectedCode::ServerDraining,
            Some(json!({ "reason": "maintenance_window" })),
        ));

        assert!(matches!(missing_reason, ApiError::Conflict(_)));
        assert!(matches!(unknown_reason, ApiError::Conflict(_)));
        assert_eq!(missing_reason.kind(), ApiErrorKind::Unclassified);
        assert_eq!(unknown_reason.kind(), ApiErrorKind::Unclassified);
    }

    #[test]
    fn limit_exceeded_rejection_carries_structured_details() {
        let err = map_write_rejection(&rejected(
            RejectedCode::LimitExceeded,
            Some(json!({ "limit_kind": "document_block_count", "limit": 5000, "observed": 5001 })),
        ));
        assert_eq!(err.kind(), ApiErrorKind::LimitExceeded);
        let ApiError::Typed { details, .. } = err else {
            panic!("expected a Typed limit_exceeded error");
        };
        let details = details.expect("limit_exceeded must carry details");
        assert_eq!(details.get("limit_kind"), Some(&json!("document_block_count")));
        assert_eq!(details.get("limit"), Some(&json!(5000)));
        assert_eq!(details.get("observed"), Some(&json!(5001)));
    }

    #[test]
    fn stale_frontier_rejection_carries_current_seq() {
        let mut with_seq = rejected(RejectedCode::StaleFrontier, None);
        with_seq.current_seq = Some(42);
        let err = map_write_rejection(&with_seq);
        assert_eq!(err.kind(), ApiErrorKind::StaleFrontier);
        let ApiError::Typed { details, .. } = err else {
            panic!("expected a Typed stale_frontier error");
        };
        assert_eq!(details.expect("details").get("current_seq"), Some(&json!(42)));
    }

    #[test]
    fn policy_rejected_from_write_path_is_distinguishable_from_forbidden() {
        let policy = map_write_rejection(&rejected(RejectedCode::PolicyRejected, None));
        let forbidden = map_write_rejection(&rejected(RejectedCode::Forbidden, None));
        assert_eq!(policy.kind(), ApiErrorKind::PolicyRejected);
        assert_eq!(forbidden.kind(), ApiErrorKind::Forbidden);
        assert_ne!(policy.kind(), forbidden.kind());
        // Both share the same HTTP/CLI bucket (403 / exit 4) but are still distinct stable codes.
        assert_eq!(policy.kind().http_status_code(), forbidden.kind().http_status_code());
        assert_ne!(policy.kind().stable_code(), forbidden.kind().stable_code());
    }

    #[test]
    fn collab_limit_exceeded_maps_to_typed_limit_exceeded_with_numeric_details() {
        let err = map_collab_error(&CollabError::LimitExceeded {
            limit_kind: "tree_depth",
            limit: 32,
            observed: 33,
        });
        assert_eq!(err.kind(), ApiErrorKind::LimitExceeded);
        let ApiError::Typed { details, .. } = err else {
            panic!("expected a Typed limit_exceeded error");
        };
        let details = details.expect("details");
        assert_eq!(details.get("limit_kind"), Some(&json!("tree_depth")));
        assert_eq!(details.get("limit"), Some(&json!(32)));
        assert_eq!(details.get("observed"), Some(&json!(33)));
    }

    #[test]
    fn collab_oversized_update_maps_to_update_bytes_limit_kind() {
        let err = map_collab_error(&CollabError::InputTooLarge {
            input: "update",
            actual_bytes: 999,
            max_bytes: 512,
        });
        assert_eq!(err.kind(), ApiErrorKind::LimitExceeded);
        let ApiError::Typed { details, .. } = err else {
            panic!("expected a Typed limit_exceeded error");
        };
        assert_eq!(
            details.expect("details").get("limit_kind"),
            Some(&json!("update_bytes"))
        );
    }

    #[test]
    fn collab_unknown_node_maps_to_invalid_update() {
        let err = map_collab_error(&CollabError::UnknownNode { id: "n1".to_string() });
        assert_eq!(err.kind(), ApiErrorKind::InvalidUpdate);
    }
}
