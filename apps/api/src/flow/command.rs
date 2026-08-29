//! The one write path this package ships: `POST .../flow/objects` (object creation).
//!
//! Content commands (`insert_block`, `set_title`, `archive`, ...) are `POST
//! /flow/objects/{id}/commands` in the contract and are explicitly out of scope for this package
//! (see `versions/v0.4-flow-alpha.md` "Rust 与 API" — this package ships `apps/api/src/flow/` and
//! the four non-WebSocket endpoints only).

use collab_core::{CollabEngine, LoroCollabEngine};
use platform::app::AppState;
use sea_orm::TransactionTrait;
use serde_json::json;
use uuid::Uuid;

use crate::error::ApiError;
use crate::events::{BusinessEventInput, insert_business_event};

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

pub async fn create_object(state: &AppState, input: CreateObjectInput) -> Result<AcceptedChange, ApiError> {
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
            return Err(ApiError::BadRequest(
                "project does not belong to this workspace".to_string(),
            ));
        }
    }

    if let Some(parent_id) = input.parent_object_id {
        let parent = repository::fetch_parent_object(&state.db, parent_id)
            .await?
            .ok_or_else(|| ApiError::BadRequest("parent_object_id not found".to_string()))?;
        if parent.workspace_id != input.workspace_id {
            return Err(ApiError::BadRequest(
                "parent_object_id does not belong to this workspace".to_string(),
            ));
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

    let event_id = insert_business_event(
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
    )
    .await?;

    let dispatch_max_attempts = crate::config::runtime().flow.dispatch_max_attempts;
    repository::insert_event_dispatch(
        &tx,
        event_id,
        input.workspace_id,
        "flow.object.created",
        dispatch_max_attempts,
    )
    .await?;

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
        let id = insert_business_event(
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
        )
        .await?;

        let dispatch_max_attempts = crate::config::runtime().flow.dispatch_max_attempts;
        repository::insert_event_dispatch(&tx, id, input.workspace_id, event_type, dispatch_max_attempts).await?;
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
/// `event_id` pointing at the original `flow.object.created` row rather than minting a new one —
/// a replay must report the same fact that already happened, not a second event.
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
