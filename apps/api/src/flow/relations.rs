//! PostgreSQL-only relation commands and reads.
//!
//! `link`/`unlink` are governance commands because they change durable object metadata, but
//! `ADR-0013` deliberately assigns both `existing_document_cardinality = 0`: the transaction
//! touches `flow_relations`, the business event, and dispatch work only. It never locks or updates
//! `collab_documents` and never writes `collab_updates`.
//!
//! ## Authorization grade
//!
//! `rest-api-v1.md` requires both objects in a link to be authorized. `ADR-0012` §2 assigns
//! `edit` to content and ordinary reversible governance, reserving `full_access` for authorization
//! changes, inheritance changes, and cross-parent moves. A relation changes none of those three,
//! so both source and target require `edit`. The early check improves rejection latency; the same
//! two permissions are recomputed from `PostgreSQL` after the transaction has acquired the
//! workspace epoch `FOR SHARE` fence, closing the authorization TOCTOU window.
//!
//! ## Duplicate links and idempotency
//!
//! The relation identity is the schema's unique key `(workspace, source, target, relation_type)`.
//! A same-key replay of the same identity returns the original event id through the existing
//! `business_events` idempotency index. A different idempotency key attempting that identity is a
//! non-recoverable invalid request, not an idempotent success. `error-mapping-v1.md` freezes no
//! dedicated duplicate-link semantic (a contract gap), so both that case and reuse of one key for
//! another identity use the existing typed `invalid_update`; they never use legacy `Conflict`,
//! whose error kind is `Unclassified`.
//!
//! ## Opaque pagination and bounded scanning
//!
//! Stable keyset order remains `(created_at,id)`, but the pair is encrypted and authenticated with
//! ChaCha20-Poly1305 under a key derived from the deployment JWT secret. Thus even when a page ends
//! on an `Unavailable` placeholder, its cursor reveals neither the relation id nor creation time.
//! With `limit <= 100`, `needed <= 101`: fixed batches of 100 mean at most two batches and 200 rows
//! are examined today. The 1000-row authorization budget is retained as defense in depth for future
//! policy filtering. Its `observed` detail intentionally reports the fixed safe limit, not the real
//! examined count, matching WP-10's non-disclosure convention.

use std::fmt::Write as _;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
use chrono::{DateTime, Utc};
use platform::app::AppState;
use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::ApiError;
use crate::events::{BusinessEventInput, FlowDispatchSpec, FlowEventOutcome, insert_flow_event};

use super::collab::{authz, limits};
use super::command::{ExecuteCommandInput, accepted_change_from_row, actor_user_id};
use super::model::{AcceptedChange, RelatedObjectView, RelationListResponse, RelationView};
use super::move_object::GovernanceCommandType;
use super::{policy, repository};

const RELATION_SCAN_BATCH_SIZE: u64 = 100;
const CURSOR_VERSION: u8 = 1;
const CURSOR_AAD: &[u8] = b"openpr.flow.relations.cursor.v1";

fn check_relation_scan_budget(examined: u64) -> Result<(), ApiError> {
    if examined > limits::AUTHORIZED_SCAN_ROWS_MAX {
        return Err(ApiError::limit_exceeded(
            "relation authorization scan budget exceeded",
            "scan_budget",
            Some(json!(limits::AUTHORIZED_SCAN_ROWS_MAX)),
            Some(json!(limits::AUTHORIZED_SCAN_ROWS_MAX)),
            None,
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkPayload {
    target_object_id: Uuid,
    relation_type: String,
    #[serde(default = "empty_object")]
    properties: Value,
    #[serde(default)]
    position_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnlinkPayload {
    relation_id: Uuid,
}

fn empty_object() -> Value {
    json!({})
}

fn principal_kind(input: &ExecuteCommandInput) -> &'static str {
    if input.actor_is_bot() { "bot" } else { "user" }
}

fn valid_relation_type(value: &str) -> bool {
    let mut chars = value.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[derive(Debug, FromQueryResult)]
struct RelationIdentityRow {
    id: Uuid,
    workspace_id: Uuid,
    source_object_id: Uuid,
    target_object_id: Uuid,
    relation_type: String,
}

#[derive(Debug, FromQueryResult)]
struct InsertedRelationRow {
    id: Uuid,
}

async fn find_relation<C: ConnectionTrait>(
    conn: &C,
    relation_id: Uuid,
) -> Result<Option<RelationIdentityRow>, ApiError> {
    Ok(RelationIdentityRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, source_object_id, target_object_id, relation_type \
         FROM flow_relations WHERE id = $1",
        vec![relation_id.into()],
    ))
    .one(conn)
    .await?)
}

#[derive(Debug, FromQueryResult)]
struct RelationEventRow {
    id: Uuid,
    event_type: String,
    aggregate_id: String,
    payload: Value,
}

async fn find_replay<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    idempotency_key: &str,
) -> Result<Option<RelationEventRow>, ApiError> {
    Ok(RelationEventRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, event_type, aggregate_id, payload FROM business_events \
         WHERE workspace_id = $1 AND idempotency_key = $2",
        vec![workspace_id.into(), idempotency_key.into()],
    ))
    .one(conn)
    .await?)
}

fn payload_uuid(payload: &Value, key: &str) -> Option<Uuid> {
    payload.get(key)?.as_str()?.parse().ok()
}

async fn replay(
    state: &AppState,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    kind: GovernanceCommandType,
    expected_target: Option<Uuid>,
    expected_relation_type: Option<&str>,
    expected_relation_id: Option<Uuid>,
) -> Result<Option<AcceptedChange>, ApiError> {
    let Some(event) = find_replay(&state.db, workspace_id, &input.idempotency_key).await? else {
        return Ok(None);
    };
    let source_matches = payload_uuid(&event.payload, "source_object_id") == Some(input.object_id);
    let target_matches =
        expected_target.is_none_or(|target| payload_uuid(&event.payload, "target_object_id") == Some(target));
    let type_matches = expected_relation_type
        .is_none_or(|relation_type| event.payload.get("relation_type").and_then(Value::as_str) == Some(relation_type));
    let relation_matches = expected_relation_id.is_none_or(|relation_id| {
        payload_uuid(&event.payload, "relation_id") == Some(relation_id)
            && event.aggregate_id == relation_id.to_string()
    });
    if event.event_type != kind.event_type() || !source_matches || !target_matches || !type_matches || !relation_matches
    {
        return Err(ApiError::invalid_update(
            "idempotency_key was already used for a different operation",
        ));
    }
    let relation_id = payload_uuid(&event.payload, "relation_id").ok_or(ApiError::Internal)?;
    let current = repository::fetch_object_view(&state.db, input.object_id)
        .await?
        .ok_or(ApiError::Internal)?;
    let mut change = accepted_change_from_row(current, event.id);
    change.affected_object_ids = [
        payload_uuid(&event.payload, "source_object_id"),
        payload_uuid(&event.payload, "target_object_id"),
    ]
    .into_iter()
    .flatten()
    .collect();
    change.command_result = Some(match kind {
        GovernanceCommandType::Link => json!({
            "relation": {
                "relation_id": relation_id,
                "relation_type": event.payload.get("relation_type").ok_or(ApiError::Internal)?,
                "source_object_id": input.object_id,
                "target_object_id": payload_uuid(&event.payload, "target_object_id").ok_or(ApiError::Internal)?,
            },
            "existing_document_cardinality": 0,
        }),
        GovernanceCommandType::Unlink => json!({
            "removed": true,
            "event_id": event.id,
            "existing_document_cardinality": 0,
        }),
        GovernanceCommandType::MoveObject => return Err(ApiError::Internal),
    });
    Ok(Some(change))
}

async fn authorize_both<C: ConnectionTrait>(
    conn: &C,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    target_object_id: Uuid,
) -> Result<(), ApiError> {
    let levels = authz::effective_permissions(
        conn,
        workspace_id,
        &[input.object_id, target_object_id],
        principal_kind(input),
        input.actor_id,
        &input.role,
    )
    .await?;
    if levels.len() != 2 || levels.iter().any(|(_, level)| *level < authz::PermissionLevel::Edit) {
        return Err(ApiError::policy_rejected(
            "edit permission on both source and target is required for relation commands",
        ));
    }
    Ok(())
}

async fn lock_participants<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    source_object_id: Uuid,
    target_object_id: Uuid,
    require_active: bool,
) -> Result<(), ApiError> {
    #[derive(FromQueryResult)]
    struct IdRow {
        id: Uuid,
    }
    let active_predicate = if require_active {
        " AND lifecycle_status = 'active'"
    } else {
        ""
    };
    let rows = IdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT id FROM flow_objects WHERE workspace_id = $1 AND id = ANY($2){active_predicate} \
             ORDER BY id FOR SHARE"
        ),
        vec![workspace_id.into(), vec![source_object_id, target_object_id].into()],
    ))
    .all(conn)
    .await?;
    let expected = if source_object_id == target_object_id { 1 } else { 2 };
    if rows.len() != expected
        || rows
            .iter()
            .any(|row| row.id != source_object_id && row.id != target_object_id)
    {
        return Err(ApiError::invalid_update(
            "relation participants must be active objects in the same workspace",
        ));
    }
    Ok(())
}

/// Executes the `link` and `unlink` governance variants.
pub async fn execute_command(
    state: &AppState,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    checked_epoch: i64,
    kind: GovernanceCommandType,
) -> Result<AcceptedChange, ApiError> {
    if input.expected_frontier.is_some() {
        return Err(ApiError::invalid_update(
            "expected_frontier is not accepted for link/unlink because they advance no document head",
        ));
    }
    match kind {
        GovernanceCommandType::Link => execute_link(state, input, workspace_id, checked_epoch).await,
        GovernanceCommandType::Unlink => execute_unlink(state, input, workspace_id, checked_epoch).await,
        GovernanceCommandType::MoveObject => Err(ApiError::Internal),
    }
}

async fn execute_link(
    state: &AppState,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    checked_epoch: i64,
) -> Result<AcceptedChange, ApiError> {
    let payload: LinkPayload = serde_json::from_value(input.payload.clone())
        .map_err(|err| ApiError::invalid_update(format!("invalid link payload: {err}")))?;
    if !valid_relation_type(&payload.relation_type) {
        return Err(ApiError::invalid_update("relation_type must match ^[a-z][a-z0-9_]*$"));
    }
    if !payload.properties.is_object() {
        return Err(ApiError::invalid_update("properties must be a JSON object"));
    }
    if let Some(change) = replay(
        state,
        input,
        workspace_id,
        GovernanceCommandType::Link,
        Some(payload.target_object_id),
        Some(&payload.relation_type),
        None,
    )
    .await?
    {
        return Ok(change);
    }

    repository::fetch_object_workspace(&state.db, payload.target_object_id)
        .await?
        .filter(|target_workspace| *target_workspace == workspace_id)
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    authorize_both(&state.db, input, workspace_id, payload.target_object_id).await?;

    let tx = state.db.begin().await?;
    match authz::fence_epoch_for_share(&tx, workspace_id, checked_epoch).await {
        Ok(()) => {}
        Err(ApiError::Conflict(_)) => {
            let _ = tx.rollback().await;
            return Err(ApiError::policy_rejected(
                "authz_epoch advanced since relation permission was checked",
            ));
        }
        Err(err) => {
            let _ = tx.rollback().await;
            return Err(err);
        }
    }
    lock_participants(&tx, workspace_id, input.object_id, payload.target_object_id, true).await?;
    authorize_both(&tx, input, workspace_id, payload.target_object_id).await?;

    let relation_id = Uuid::new_v4();
    let inserted = InsertedRelationRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_relations \
         (id, workspace_id, relation_type, source_object_id, target_object_id, position_key, properties, created_by) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8) \
         ON CONFLICT (workspace_id, source_object_id, target_object_id, relation_type) DO NOTHING RETURNING id",
        vec![
            relation_id.into(),
            workspace_id.into(),
            payload.relation_type.clone().into(),
            input.object_id.into(),
            payload.target_object_id.into(),
            payload.position_key.clone().unwrap_or_default().into(),
            payload.properties.clone().into(),
            actor_user_id(input.actor_id, input.actor_is_bot()).into(),
        ],
    ))
    .one(&tx)
    .await?;
    if inserted.is_none() {
        let concurrent_replay = replay(
            state,
            input,
            workspace_id,
            GovernanceCommandType::Link,
            Some(payload.target_object_id),
            Some(&payload.relation_type),
            None,
        )
        .await?;
        let _ = tx.rollback().await;
        if let Some(change) = concurrent_replay {
            return Ok(change);
        }
        return Err(ApiError::invalid_update("relation already exists"));
    }
    debug_assert_eq!(inserted.map(|row| row.id), Some(relation_id));

    let event = write_relation_event(
        &tx,
        input,
        workspace_id,
        GovernanceCommandType::Link,
        relation_id,
        payload.target_object_id,
        &payload.relation_type,
    )
    .await?;
    if !event.was_new {
        let _ = tx.rollback().await;
        return replay(
            state,
            input,
            workspace_id,
            GovernanceCommandType::Link,
            Some(payload.target_object_id),
            Some(&payload.relation_type),
            None,
        )
        .await?
        .ok_or(ApiError::Internal);
    }
    tx.commit().await?;
    build_change(
        state,
        input,
        event.event_id,
        relation_id,
        payload.target_object_id,
        Some(&payload.relation_type),
        false,
    )
    .await
}

async fn execute_unlink(
    state: &AppState,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    checked_epoch: i64,
) -> Result<AcceptedChange, ApiError> {
    let payload: UnlinkPayload = serde_json::from_value(input.payload.clone())
        .map_err(|err| ApiError::invalid_update(format!("invalid unlink payload: {err}")))?;
    if let Some(change) = replay(
        state,
        input,
        workspace_id,
        GovernanceCommandType::Unlink,
        None,
        None,
        Some(payload.relation_id),
    )
    .await?
    {
        return Ok(change);
    }
    let relation = find_relation(&state.db, payload.relation_id)
        .await?
        .filter(|row| row.workspace_id == workspace_id && row.source_object_id == input.object_id)
        .ok_or_else(|| ApiError::NotFound("flow relation not found".to_string()))?;
    authorize_both(&state.db, input, workspace_id, relation.target_object_id).await?;

    let tx = state.db.begin().await?;
    match authz::fence_epoch_for_share(&tx, workspace_id, checked_epoch).await {
        Ok(()) => {}
        Err(ApiError::Conflict(_)) => {
            let _ = tx.rollback().await;
            return Err(ApiError::policy_rejected(
                "authz_epoch advanced since relation permission was checked",
            ));
        }
        Err(err) => {
            let _ = tx.rollback().await;
            return Err(err);
        }
    }
    // Unlink remains available for archived participants so a stale relation can be cleaned up;
    // unlike link, it creates no new edge to an archived object.
    lock_participants(&tx, workspace_id, input.object_id, relation.target_object_id, false).await?;
    authorize_both(&tx, input, workspace_id, relation.target_object_id).await?;
    let locked = RelationIdentityRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, source_object_id, target_object_id, relation_type \
         FROM flow_relations WHERE id = $1 AND workspace_id = $2 AND source_object_id = $3 FOR UPDATE",
        vec![payload.relation_id.into(), workspace_id.into(), input.object_id.into()],
    ))
    .one(&tx)
    .await?
    .ok_or_else(|| ApiError::NotFound("flow relation not found".to_string()))?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM flow_relations WHERE id = $1",
        vec![locked.id.into()],
    ))
    .await?;
    let event = write_relation_event(
        &tx,
        input,
        workspace_id,
        GovernanceCommandType::Unlink,
        locked.id,
        locked.target_object_id,
        &locked.relation_type,
    )
    .await?;
    if !event.was_new {
        let _ = tx.rollback().await;
        return replay(
            state,
            input,
            workspace_id,
            GovernanceCommandType::Unlink,
            None,
            None,
            Some(payload.relation_id),
        )
        .await?
        .ok_or(ApiError::Internal);
    }
    tx.commit().await?;
    build_change(
        state,
        input,
        event.event_id,
        locked.id,
        locked.target_object_id,
        None,
        true,
    )
    .await
}

async fn write_relation_event<C: ConnectionTrait>(
    conn: &C,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    kind: GovernanceCommandType,
    relation_id: Uuid,
    target_object_id: Uuid,
    relation_type: &str,
) -> Result<FlowEventOutcome, ApiError> {
    insert_flow_event(
        conn,
        BusinessEventInput {
            workspace_id,
            project_id: None,
            event_type: kind.event_type().to_string(),
            aggregate_type: "flow_relation".to_string(),
            aggregate_id: relation_id.to_string(),
            actor_id: actor_user_id(input.actor_id, input.actor_is_bot()),
            source: input.origin.source_json(),
            payload: json!({
                "relation_id": relation_id,
                "source_object_id": input.object_id,
                "target_object_id": target_object_id,
                "relation_type": relation_type,
            }),
            metadata: json!({ "message": input.message }),
            correlation_id: Some(input.origin.correlation_id),
            causation_id: input.origin.causation_id,
            idempotency_key: Some(input.idempotency_key.clone()),
        },
        Some(FlowDispatchSpec {
            max_attempts: crate::config::runtime().flow.dispatch_max_attempts,
            document_id: None,
            accepted_seq: None,
        }),
    )
    .await
}

async fn build_change(
    state: &AppState,
    input: &ExecuteCommandInput,
    event_id: Uuid,
    relation_id: Uuid,
    target_object_id: Uuid,
    linked_relation_type: Option<&str>,
    removed: bool,
) -> Result<AcceptedChange, ApiError> {
    let current = repository::fetch_object_view(&state.db, input.object_id)
        .await?
        .ok_or(ApiError::Internal)?;
    let mut change = accepted_change_from_row(current, event_id);
    change.affected_object_ids = vec![input.object_id, target_object_id];
    change.command_result = Some(linked_relation_type.map_or_else(
        || {
            json!({
                "removed": removed,
                "event_id": event_id,
                "existing_document_cardinality": 0,
            })
        },
        |relation_type| {
            json!({
            "relation": {
                "relation_id": relation_id,
                "relation_type": relation_type,
                "source_object_id": input.object_id,
                "target_object_id": target_object_id,
            },
            "existing_document_cardinality": 0,
            })
        },
    ));
    Ok(change)
}

/// Parsed relation-list direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationDirection {
    Outgoing,
    Incoming,
    Both,
}

impl RelationDirection {
    pub fn parse(raw: Option<&str>) -> Result<Self, ApiError> {
        match raw {
            None | Some("both") => Ok(Self::Both),
            Some("outgoing") => Ok(Self::Outgoing),
            Some("incoming") => Ok(Self::Incoming),
            Some(_) => Err(ApiError::invalid_update(
                "direction must be outgoing, incoming, or both",
            )),
        }
    }
}

pub struct ListRelationsParams {
    pub direction: RelationDirection,
    pub relation_type: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u64>,
}

#[derive(Debug, FromQueryResult)]
struct RelationReadRow {
    id: Uuid,
    workspace_id: Uuid,
    relation_type: String,
    source_object_id: Uuid,
    position_key: String,
    properties: Value,
    created_at: DateTime<Utc>,
    source_workspace_id: Option<Uuid>,
    target_workspace_id: Option<Uuid>,
    other_id: Option<Uuid>,
    other_object_type: Option<String>,
    other_title: Option<String>,
    other_lifecycle_status: Option<String>,
    other_project_id: Option<Uuid>,
}

fn cursor_key(secret: &str) -> Result<LessSafeKey, ApiError> {
    let mut digest = Sha256::new();
    digest.update(CURSOR_AAD);
    digest.update([0]);
    digest.update(secret.as_bytes());
    UnboundKey::new(&aead::CHACHA20_POLY1305, &digest.finalize())
        .map(LessSafeKey::new)
        .map_err(|_| ApiError::Internal)
}

#[allow(clippy::print_stderr)]
fn encode_cursor(secret: &str, created_at: DateTime<Utc>, id: Uuid) -> Result<String, ApiError> {
    #[cfg(test)]
    if std::env::var_os("OPENPR_FLOW_TEST_MUTATION_RELATION_CURSOR_PLAINTEXT").is_some() {
        eprintln!("WP28_MUTATION_RELATION_CURSOR_PLAINTEXT_ACTIVE");
        return Ok(BASE64_URL.encode(format!("{}|{id}", created_at.to_rfc3339())));
    }
    let mut nonce_bytes = [0u8; aead::NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce_bytes)
        .map_err(|_| ApiError::Internal)?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);
    let mut encrypted = format!("{}|{id}", created_at.to_rfc3339()).into_bytes();
    cursor_key(secret)?
        .seal_in_place_append_tag(nonce, Aad::from(CURSOR_AAD), &mut encrypted)
        .map_err(|_| ApiError::Internal)?;
    let mut token = Vec::with_capacity(1 + aead::NONCE_LEN + encrypted.len());
    token.push(CURSOR_VERSION);
    token.extend_from_slice(&nonce_bytes);
    token.extend_from_slice(&encrypted);
    Ok(BASE64_URL.encode(token))
}

fn decode_cursor(secret: &str, raw: &str) -> Result<(DateTime<Utc>, Uuid), ApiError> {
    let token = BASE64_URL
        .decode(raw)
        .map_err(|_| ApiError::invalid_update("cursor is not valid"))?;
    let (version, payload) = token
        .split_first()
        .ok_or_else(|| ApiError::invalid_update("cursor is not valid"))?;
    if *version != CURSOR_VERSION {
        return Err(ApiError::invalid_update("cursor is not valid"));
    }
    let (nonce_bytes, ciphertext) = payload
        .split_at_checked(aead::NONCE_LEN)
        .ok_or_else(|| ApiError::invalid_update("cursor is not valid"))?;
    let nonce =
        Nonce::try_assume_unique_for_key(nonce_bytes).map_err(|_| ApiError::invalid_update("cursor is not valid"))?;
    let mut in_out = ciphertext.to_vec();
    let plaintext = cursor_key(secret)?
        .open_in_place(nonce, Aad::from(CURSOR_AAD), &mut in_out)
        .map_err(|_| ApiError::invalid_update("cursor is not valid"))?;
    let text = std::str::from_utf8(plaintext).map_err(|_| ApiError::invalid_update("cursor is not valid"))?;
    let (created_at, id) = text
        .split_once('|')
        .ok_or_else(|| ApiError::invalid_update("cursor is not valid"))?;
    Ok((
        DateTime::parse_from_rfc3339(created_at)
            .map_err(|_| ApiError::invalid_update("cursor is not valid"))?
            .with_timezone(&Utc),
        id.parse()
            .map_err(|_| ApiError::invalid_update("cursor is not valid"))?,
    ))
}

async fn fetch_relation_batch<C: ConnectionTrait>(
    conn: &C,
    object_id: Uuid,
    direction: RelationDirection,
    relation_type: Option<&str>,
    after: Option<(DateTime<Utc>, Uuid)>,
    limit: u64,
) -> Result<Vec<RelationReadRow>, ApiError> {
    let mut values: Vec<sea_orm::Value> = vec![object_id.into()];
    let direction_predicate = match direction {
        RelationDirection::Outgoing => "r.source_object_id = $1",
        RelationDirection::Incoming => "r.target_object_id = $1",
        RelationDirection::Both => "(r.source_object_id = $1 OR r.target_object_id = $1)",
    };
    let mut sql = format!(
        "SELECT r.id, r.workspace_id, r.relation_type, r.source_object_id, r.target_object_id, \
         r.position_key, r.properties, r.created_at, source.workspace_id AS source_workspace_id, \
         target.workspace_id AS target_workspace_id, other.id AS other_id, \
         other.object_type AS other_object_type, projection.title AS other_title, \
         other.lifecycle_status AS other_lifecycle_status, other.project_id AS other_project_id \
         FROM flow_relations r \
         LEFT JOIN flow_objects source ON source.id = r.source_object_id \
         LEFT JOIN flow_objects target ON target.id = r.target_object_id \
         LEFT JOIN flow_objects other ON other.id = CASE WHEN r.source_object_id = $1 THEN r.target_object_id ELSE r.source_object_id END \
         LEFT JOIN flow_object_projections projection ON projection.object_id = other.id \
         WHERE {direction_predicate}"
    );
    if let Some(value) = relation_type {
        values.push(value.into());
        let _ = write!(sql, " AND r.relation_type = ${}", values.len());
    }
    if let Some((created_at, id)) = after {
        values.push(created_at.into());
        let created = values.len();
        values.push(id.into());
        let id_index = values.len();
        let _ = write!(sql, " AND (r.created_at, r.id) > (${created}, ${id_index})");
    }
    values.push(limit.into());
    let _ = write!(sql, " ORDER BY r.created_at ASC, r.id ASC LIMIT ${}", values.len());
    Ok(
        RelationReadRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .all(conn)
            .await?,
    )
}

async fn record_relation_integrity(state: &AppState, workspace_id: Uuid, relation_id: Uuid) -> ApiError {
    let result = repository::insert_integrity_record(
        &state.db,
        repository::IntegrityRecordInput {
            workspace_id,
            kind: "cross_workspace_relation",
            subject_kind: "flow_relation",
            subject_id: &relation_id.to_string(),
            detected_by: "flow.query.relations",
            details_redacted: json!({ "relation_id": relation_id }),
        },
    )
    .await;
    if let Err(err) = result {
        tracing::error!(%err, %relation_id, "failed to record relation integrity alert");
    }
    ApiError::invalid_update("cross-workspace relation integrity violation")
}

/// Lists relations for an already-authorized root object.
///
/// Candidate rows are fetched in fixed batches and counted before authorization. The current
/// relation contract preserves denied targets as `Unavailable`, so today a row is never silently
/// discarded. Since `limit <= 100`, at most two 100-row batches (200 rows total) can be examined;
/// retaining the 1000-row guard makes that safety invariant explicit and prevents a future
/// policy-filtering rule from turning into unbounded overfetch or a silently short page. As in
/// WP-10, a violation reports the fixed limit as `observed`, never the actual scan count.
pub async fn list_relations(
    state: &AppState,
    access: &policy::AuthorizedFlowObject,
    params: ListRelationsParams,
) -> Result<Option<RelationListResponse>, ApiError> {
    let limit = super::query::validate_limit(params.limit)?;
    let limit_usize = usize::try_from(limit).unwrap_or(usize::MAX);
    let needed = limit_usize.saturating_add(1);
    let mut after = params
        .cursor
        .as_deref()
        .map(|cursor| decode_cursor(state.cfg.jwt_secret.expose(), cursor))
        .transpose()?;
    let mut examined = 0u64;
    let mut accepted: Vec<(RelationReadRow, bool)> = Vec::with_capacity(needed);
    while accepted.len() < needed {
        let batch = fetch_relation_batch(
            &state.db,
            access.object_id(),
            params.direction,
            params.relation_type.as_deref(),
            after,
            RELATION_SCAN_BATCH_SIZE,
        )
        .await?;
        let batch_len = batch.len();
        if batch_len == 0 {
            break;
        }
        for row in &batch {
            examined = examined.saturating_add(1);
            check_relation_scan_budget(examined)?;
            if row.workspace_id != access.workspace_id()
                || row.source_workspace_id != Some(access.workspace_id())
                || row.target_workspace_id != Some(access.workspace_id())
                || row.other_id.is_none()
                || row.other_title.is_none()
            {
                return Err(record_relation_integrity(state, access.workspace_id(), row.id).await);
            }
        }
        let other_ids: Vec<Uuid> = batch.iter().filter_map(|row| row.other_id).collect();
        let Some(visible) =
            policy::authorize_flow_objects(state, access.context(), &other_ids, authz::PermissionLevel::View).await?
        else {
            return Ok(None);
        };
        for (row, is_visible) in batch.into_iter().zip(visible) {
            after = Some((row.created_at, row.id));
            accepted.push((row, is_visible));
            if accepted.len() >= needed {
                break;
            }
        }
        if batch_len < usize::try_from(RELATION_SCAN_BATCH_SIZE).unwrap_or(usize::MAX) {
            break;
        }
    }
    if !policy::ensure_epoch_current(state, access.context()).await? {
        return Ok(None);
    }
    let next_cursor = if accepted.len() > limit_usize {
        accepted.truncate(limit_usize);
        accepted
            .last()
            .map(|(row, _)| encode_cursor(state.cfg.jwt_secret.expose(), row.created_at, row.id))
            .transpose()?
    } else {
        None
    };
    let items = accepted
        .into_iter()
        .map(|(row, is_visible)| -> Result<RelationView, ApiError> {
            if !is_visible {
                return Ok(RelationView::Unavailable);
            }
            Ok(RelationView::Visible {
                relation_id: row.id,
                relation_type: row.relation_type,
                direction: if row.source_object_id == access.object_id() {
                    "outgoing".to_string()
                } else {
                    "incoming".to_string()
                },
                position_key: (!row.position_key.is_empty()).then_some(row.position_key),
                properties: row.properties,
                created_at: row.created_at.to_rfc3339(),
                other_object: Box::new(RelatedObjectView {
                    id: row.other_id.ok_or(ApiError::Internal)?,
                    object_type: row.other_object_type.ok_or(ApiError::Internal)?,
                    title: row.other_title.ok_or(ApiError::Internal)?,
                    lifecycle_status: row.other_lifecycle_status.ok_or(ApiError::Internal)?,
                    project_id: row.other_project_id,
                }),
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    Ok(Some(RelationListResponse { items, next_cursor }))
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn opaque_cursor_round_trips_and_rejects_tampering_or_another_key() {
        let created_at = DateTime::parse_from_rfc3339("2026-09-09T12:34:56Z")
            .expect("fixture time parses")
            .with_timezone(&Utc);
        let relation_id = Uuid::new_v4();
        let cursor = encode_cursor("cursor-secret-a", created_at, relation_id).expect("cursor encrypts");
        assert_eq!(
            decode_cursor("cursor-secret-a", &cursor).expect("cursor decrypts"),
            (created_at, relation_id)
        );
        assert_eq!(
            decode_cursor("cursor-secret-b", &cursor)
                .expect_err("a cursor is bound to the deployment key")
                .kind(),
            crate::error::ApiErrorKind::InvalidUpdate
        );

        let mut tampered = BASE64_URL.decode(cursor).expect("cursor is base64url");
        let last = tampered.last_mut().expect("cursor has authenticated ciphertext");
        *last ^= 1;
        let tampered = BASE64_URL.encode(tampered);
        assert_eq!(
            decode_cursor("cursor-secret-a", &tampered)
                .expect_err("ciphertext mutation must fail authentication")
                .kind(),
            crate::error::ApiErrorKind::InvalidUpdate
        );
    }

    #[test]
    fn relation_type_validation_matches_the_database_constraint() {
        for valid in ["related", "blocks_2", "a"] {
            assert!(valid_relation_type(valid));
        }
        for invalid in ["", "2blocks", "HasCaps", "has-dash", "has space"] {
            assert!(!valid_relation_type(invalid));
        }
    }

    #[test]
    fn unavailable_serializes_to_exactly_one_field() {
        let value = serde_json::to_value(RelationView::Unavailable).expect("serializes");
        assert_eq!(value, json!({ "visibility": "unavailable" }));
        assert_eq!(value.as_object().map(serde_json::Map::len), Some(1));
    }

    #[test]
    fn relation_governance_commands_are_zero_document_cardinality() {
        for command in [GovernanceCommandType::Link, GovernanceCommandType::Unlink] {
            assert_eq!(
                command.existing_document_cardinality(),
                super::super::command::ExistingDocumentCardinality::Zero
            );
        }
    }

    #[test]
    fn relation_scan_budget_accepts_exact_boundary_and_rejects_one_more() {
        assert!(super::check_relation_scan_budget(limits::AUTHORIZED_SCAN_ROWS_MAX).is_ok());
        let err = super::check_relation_scan_budget(limits::AUTHORIZED_SCAN_ROWS_MAX + 1)
            .expect_err("one row beyond the fixed scan budget must reject, never return a short page");
        assert_eq!(err.kind(), crate::error::ApiErrorKind::LimitExceeded);
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::indexing_slicing,
    clippy::too_many_lines
)]
mod database_tests {
    use axum::{body::to_bytes, http::Extensions, response::IntoResponse};
    use base64::Engine as _;
    use platform::{
        app::AppState,
        auth::{JwtClaims, TokenType},
        config::{AppConfig, Secret},
    };
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::{BASE64_URL, CURSOR_VERSION, ListRelationsParams, RelationDirection, decode_cursor, list_relations};
    use crate::error::ApiErrorKind;
    use crate::flow::collab::authz::PermissionLevel;
    use crate::flow::command::{CreateObjectInput, ExecuteCommandInput, create_object, execute_command};
    use crate::flow::event_origin::{CommandOrigin, EventSurface};
    use crate::flow::policy;

    const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

    struct Scratch {
        db: DatabaseConnection,
        name: String,
        admin_url: String,
    }

    impl Scratch {
        async fn drop_self(self) {
            let Self { db, name, admin_url } = self;
            drop(db);
            let Ok(admin) = Database::connect(&admin_url).await else {
                return;
            };
            let _ = admin
                .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                .await;
        }
    }

    async fn scratch(label: &str) -> Option<Scratch> {
        let admin_url = std::env::var(TEST_DATABASE_URL_ENV).ok()?;
        let admin = Database::connect(&admin_url)
            .await
            .unwrap_or_else(|err| panic!("{TEST_DATABASE_URL_ENV} is set but unusable: {err}"));
        let name = format!("openpr_flow_relations_{label}");
        let quoted = format!("\"{name}\"");
        admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS {quoted} WITH (FORCE)"))
            .await
            .unwrap_or_else(|err| panic!("could not reset scratch database {name}: {err}"));
        admin
            .execute_unprepared(&format!("CREATE DATABASE {quoted}"))
            .await
            .unwrap_or_else(|err| panic!("could not create scratch database {name}: {err}"));
        let (prefix, _) = admin_url.rsplit_once('/')?;
        let url = format!("{prefix}/{name}");
        let db = Database::connect(&url)
            .await
            .unwrap_or_else(|err| panic!("could not connect to scratch database {name}: {err}"));
        migrate(&db).await;
        Some(Scratch { db, name, admin_url })
    }

    async fn migrate(db: &DatabaseConnection) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .expect("migrations directory is readable")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
            .collect();
        files.sort();
        for path in files {
            let sql = std::fs::read_to_string(&path).expect("migration is readable");
            db.execute_unprepared(&sql)
                .await
                .unwrap_or_else(|err| panic!("applying {} failed: {err}", path.display()));
        }
    }

    macro_rules! scratch_or_skip {
        ($label:expr) => {
            match scratch($label).await {
                Some(scratch) => scratch,
                None => {
                    eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
                    return;
                }
            }
        };
    }

    fn state_for(db: DatabaseConnection) -> AppState {
        AppState {
            cfg: AppConfig {
                app_name: "flow-relations-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("flow-relations-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db,
            flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
        }
    }

    async fn exec(state: &AppState, sql: &str, values: Vec<sea_orm::Value>) {
        state
            .db
            .execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .await
            .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
    }

    async fn seed_workspace(state: &AppState) -> (Uuid, Uuid, Uuid) {
        let workspace_id = Uuid::new_v4();
        let owner_id = Uuid::new_v4();
        let member_id = Uuid::new_v4();
        for user_id in [owner_id, member_id] {
            exec(
                state,
                "INSERT INTO users (id,email,password_hash,name,role,is_active) VALUES ($1,$2,'!','test','user',true)",
                vec![user_id.into(), format!("{user_id}@relations.test").into()],
            )
            .await;
        }
        exec(
            state,
            "INSERT INTO workspaces (id,slug,name,created_by) VALUES ($1,$2,'relations test',$3)",
            vec![
                workspace_id.into(),
                format!("ws-{workspace_id}").into(),
                owner_id.into(),
            ],
        )
        .await;
        for (user_id, role) in [(owner_id, "owner"), (member_id, "member")] {
            exec(
                state,
                "INSERT INTO workspace_members (workspace_id,user_id,role) VALUES ($1,$2,$3)",
                vec![workspace_id.into(), user_id.into(), role.into()],
            )
            .await;
        }
        exec(
            state,
            "INSERT INTO flow_workspace_settings (workspace_id,flow_enabled,default_member_level) VALUES ($1,true,'edit')",
            vec![workspace_id.into()],
        )
        .await;
        (workspace_id, owner_id, member_id)
    }

    async fn page(state: &AppState, workspace_id: Uuid, owner_id: Uuid, title: &str) -> Uuid {
        create_object(
            state,
            CreateObjectInput {
                workspace_id,
                actor_id: owner_id,
                actor_is_bot: false,
                object_type: "page".to_string(),
                project_id: None,
                parent_object_id: None,
                title: title.to_string(),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            },
        )
        .await
        .expect("page creation succeeds")
        .object
        .id
    }

    fn relation_command(
        source: Uuid,
        actor: Uuid,
        role: &str,
        command_type: &str,
        payload: Value,
        key: String,
    ) -> ExecuteCommandInput {
        ExecuteCommandInput {
            object_id: source,
            actor_id: actor,
            principal_kind: "user".to_string(),
            role: role.to_string(),
            command_type: command_type.to_string(),
            payload,
            expected_frontier: None,
            idempotency_key: key,
            message: Some("relation test".to_string()),
            origin_client_id: format!("test:{actor}"),
            origin: CommandOrigin::first_request_from(EventSurface::Rest),
        }
    }

    #[derive(FromQueryResult)]
    struct HeadState {
        head_seq: i64,
        update_count: i64,
    }

    async fn head_state(state: &AppState, object_id: Uuid) -> HeadState {
        HeadState::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT d.head_seq, (SELECT count(*) FROM collab_updates u WHERE u.document_id=d.id) AS update_count \
             FROM collab_documents d WHERE d.object_id=$1",
            vec![object_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("head query succeeds")
        .expect("head exists")
    }

    #[derive(FromQueryResult)]
    struct EventProbe {
        id: Uuid,
        aggregate_type: String,
        payload: Value,
        source: Value,
        correlation_id: Option<Uuid>,
        causation_id: Option<Uuid>,
    }

    #[derive(FromQueryResult)]
    struct RelationCursorProbe {
        created_at: chrono::DateTime<chrono::Utc>,
    }

    async fn event(state: &AppState, event_type: &str) -> EventProbe {
        EventProbe::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id,aggregate_type,payload,source,correlation_id,causation_id FROM business_events \
             WHERE event_type=$1 ORDER BY created_at DESC LIMIT 1",
            vec![event_type.into()],
        ))
        .one(&state.db)
        .await
        .expect("event query succeeds")
        .expect("event exists")
    }

    async fn error_wire(error: crate::error::ApiError) -> Vec<u8> {
        to_bytes(error.into_response().into_body(), usize::MAX)
            .await
            .expect("error body is readable")
            .to_vec()
    }

    #[tokio::test]
    async fn link_and_unlink_are_real_zero_document_writes_with_exact_events_and_replay() {
        let scratch = scratch_or_skip!("write_success");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id, _) = seed_workspace(&state).await;
        let source = page(&state, workspace_id, owner_id, "Source").await;
        let target = page(&state, workspace_id, owner_id, "Target").await;
        let before_source = head_state(&state, source).await;
        let before_target = head_state(&state, target).await;

        let link_key = Uuid::new_v4().to_string();
        let link_input = relation_command(
            source,
            owner_id,
            "owner",
            "link",
            json!({
                "target_object_id": target,
                "relation_type": "depends_on",
                "position_key": "a0",
                "properties": {"private_value": "must-not-enter-event"}
            }),
            link_key.clone(),
        );
        let linked = execute_command(&state, link_input).await.expect("link succeeds");
        let relation_id = linked.command_result.as_ref().expect("result")["relation"]["relation_id"]
            .as_str()
            .expect("relation id")
            .parse::<Uuid>()
            .expect("uuid");
        assert_eq!(
            linked.command_result.as_ref().unwrap()["existing_document_cardinality"],
            0
        );

        let replayed = execute_command(
            &state,
            relation_command(
                source,
                owner_id,
                "owner",
                "link",
                json!({
                    "target_object_id": target,
                    "relation_type": "depends_on",
                    "position_key": "a0",
                    "properties": {"private_value": "must-not-enter-event"}
                }),
                link_key,
            ),
        )
        .await
        .expect("same-key link replays");
        assert_eq!(replayed.event_id, linked.event_id);
        assert_eq!(replayed.command_result, linked.command_result);

        let link_event = event(&state, "flow.relation.linked").await;
        assert_eq!(link_event.id, linked.event_id);
        assert_eq!(link_event.aggregate_type, "flow_relation");
        assert_eq!(link_event.payload["relation_id"], relation_id.to_string());
        assert_eq!(link_event.payload["source_object_id"], source.to_string());
        assert_eq!(link_event.payload["target_object_id"], target.to_string());
        assert_eq!(link_event.payload["relation_type"], "depends_on");
        assert_eq!(link_event.payload.as_object().map(serde_json::Map::len), Some(4));
        assert!(link_event.payload.get("properties").is_none());
        assert_eq!(link_event.source["surface"], "rest");
        assert!(link_event.correlation_id.is_some());
        assert!(link_event.causation_id.is_none());

        let unlink_key = Uuid::new_v4().to_string();
        let unlinked = execute_command(
            &state,
            relation_command(
                source,
                owner_id,
                "owner",
                "unlink",
                json!({"relation_id": relation_id}),
                unlink_key.clone(),
            ),
        )
        .await
        .expect("unlink succeeds");
        assert_eq!(unlinked.command_result.as_ref().unwrap()["removed"], true);
        let replayed_unlink = execute_command(
            &state,
            relation_command(
                source,
                owner_id,
                "owner",
                "unlink",
                json!({"relation_id": relation_id}),
                unlink_key,
            ),
        )
        .await
        .expect("same-key unlink replays after the row is gone");
        assert_eq!(replayed_unlink.event_id, unlinked.event_id);
        let unlink_event = event(&state, "flow.relation.unlinked").await;
        assert_eq!(unlink_event.aggregate_type, "flow_relation");
        assert_eq!(unlink_event.payload.as_object().map(serde_json::Map::len), Some(4));
        assert!(unlink_event.payload.get("properties").is_none());

        for (before, after) in [
            (before_source, head_state(&state, source).await),
            (before_target, head_state(&state, target).await),
        ] {
            assert_eq!(
                after.head_seq, before.head_seq,
                "relation commands must not advance head_seq"
            );
            assert_eq!(
                after.update_count, before.update_count,
                "relation commands must not append collab_updates"
            );
        }
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn duplicate_link_is_typed_invalid_update_and_target_permission_is_a_real_second_gate() {
        let scratch = scratch_or_skip!("double_auth");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id, member_id) = seed_workspace(&state).await;
        let source = page(&state, workspace_id, owner_id, "Source").await;
        let target = page(&state, workspace_id, owner_id, "Restricted target").await;
        exec(
            &state,
            "UPDATE flow_objects SET inherit_from_parent=false WHERE id=$1",
            vec![target.into()],
        )
        .await;
        exec(
            &state,
            "UPDATE flow_workspace_settings SET authz_epoch=authz_epoch+1 WHERE workspace_id=$1",
            vec![workspace_id.into()],
        )
        .await;
        let denied = execute_command(
            &state,
            relation_command(
                source,
                member_id,
                "member",
                "link",
                json!({"target_object_id":target,"relation_type":"blocks"}),
                Uuid::new_v4().to_string(),
            ),
        )
        .await
        .expect_err("source edit must not bypass target denial");
        assert_eq!(denied.kind(), ApiErrorKind::PolicyRejected);

        let first = execute_command(
            &state,
            relation_command(
                source,
                owner_id,
                "owner",
                "link",
                json!({"target_object_id":target,"relation_type":"blocks"}),
                Uuid::new_v4().to_string(),
            ),
        )
        .await
        .expect("owner link succeeds");
        let duplicate = execute_command(
            &state,
            relation_command(
                source,
                owner_id,
                "owner",
                "link",
                json!({"target_object_id":target,"relation_type":"blocks"}),
                Uuid::new_v4().to_string(),
            ),
        )
        .await
        .expect_err("different-key duplicate relation is invalid");
        assert_eq!(duplicate.kind(), ApiErrorKind::InvalidUpdate);
        assert_ne!(first.event_id, Uuid::nil());

        let forbidden_frontier = ExecuteCommandInput {
            expected_frontier: Some("AA==".to_string()),
            ..relation_command(
                source,
                owner_id,
                "owner",
                "link",
                json!({"target_object_id":target,"relation_type":"other"}),
                Uuid::new_v4().to_string(),
            )
        };
        assert_eq!(
            execute_command(&state, forbidden_frontier)
                .await
                .expect_err("frontier is forbidden")
                .kind(),
            ApiErrorKind::InvalidUpdate
        );
        let target_frontier = execute_command(
            &state,
            relation_command(
                source,
                owner_id,
                "owner",
                "link",
                json!({
                    "target_object_id":target,
                    "relation_type":"other",
                    "expected_target_frontier":"AA=="
                }),
                Uuid::new_v4().to_string(),
            ),
        )
        .await
        .expect_err("payload target frontier is forbidden");
        assert_eq!(target_frontier.kind(), ApiErrorKind::InvalidUpdate);
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn caller_supplied_foreign_target_matches_missing_and_records_no_integrity_alert() {
        let scratch = scratch_or_skip!("foreign_input");
        let state = state_for(scratch.db.clone());
        let (workspace_a, owner_a, _) = seed_workspace(&state).await;
        let (workspace_b, owner_b, _) = seed_workspace(&state).await;
        let source = page(&state, workspace_a, owner_a, "Source").await;
        let foreign_target = page(&state, workspace_b, owner_b, "Foreign target").await;
        let missing_target = Uuid::new_v4();

        let foreign = execute_command(
            &state,
            relation_command(
                source,
                owner_a,
                "owner",
                "link",
                json!({"target_object_id":foreign_target,"relation_type":"blocks"}),
                Uuid::new_v4().to_string(),
            ),
        )
        .await
        .expect_err("a caller-supplied target from another workspace must be hidden");
        let missing = execute_command(
            &state,
            relation_command(
                source,
                owner_a,
                "owner",
                "link",
                json!({"target_object_id":missing_target,"relation_type":"blocks"}),
                Uuid::new_v4().to_string(),
            ),
        )
        .await
        .expect_err("a missing target must be hidden");
        assert_eq!(foreign.kind(), ApiErrorKind::NotFound);
        assert_eq!(missing.kind(), ApiErrorKind::NotFound);
        assert_eq!(
            error_wire(foreign).await,
            error_wire(missing).await,
            "foreign and missing targets must have byte-identical response bodies"
        );

        let alerts = CountRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS count FROM flow_integrity_records WHERE workspace_id=$1",
            vec![workspace_a.into()],
        ))
        .one(&state.db)
        .await
        .expect("integrity query runs")
        .expect("count row");
        assert_eq!(
            alerts.count, 0,
            "ordinary caller input must not create integrity records"
        );
        scratch.drop_self().await;
    }

    fn claims(user_id: Uuid) -> JwtClaims {
        JwtClaims {
            sub: user_id.to_string(),
            email: format!("{user_id}@relations.test"),
            token_type: TokenType::Access,
            iat: 0,
            exp: 0,
        }
    }

    async fn read_access(
        state: &AppState,
        workspace_id: Uuid,
        object_id: Uuid,
        user_id: Uuid,
    ) -> policy::AuthorizedFlowObject {
        let mut extensions = Extensions::new();
        extensions.insert(claims(user_id));
        policy::require_flow_object_access(state, &extensions, workspace_id, object_id, PermissionLevel::View)
            .await
            .expect("authorization runs")
            .expect("epoch is stable")
    }

    #[tokio::test]
    async fn relation_read_paginates_and_unavailable_is_the_exact_one_field_union() {
        let scratch = scratch_or_skip!("read_union");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id, member_id) = seed_workspace(&state).await;
        let source = page(&state, workspace_id, owner_id, "Source").await;
        let visible_target = page(&state, workspace_id, owner_id, "Visible").await;
        let hidden_target = page(&state, workspace_id, owner_id, "Hidden").await;
        let hidden_link = execute_command(
            &state,
            relation_command(
                source,
                owner_id,
                "owner",
                "link",
                json!({"target_object_id":hidden_target,"relation_type":"beta","properties":{"secret":hidden_target}}),
                Uuid::new_v4().to_string(),
            ),
        )
        .await
        .expect("hidden fixture relation links");
        let hidden_relation_id = hidden_link.command_result.as_ref().expect("link result")["relation"]["relation_id"]
            .as_str()
            .expect("relation id is text")
            .parse::<Uuid>()
            .expect("relation id is a UUID");
        let visible_link = execute_command(
            &state,
            relation_command(
                source,
                owner_id,
                "owner",
                "link",
                json!({"target_object_id":visible_target,"relation_type":"alpha","properties":{"secret":visible_target}}),
                Uuid::new_v4().to_string(),
            ),
        )
        .await
        .expect("visible fixture relation links");
        let visible_relation_id = visible_link.command_result.as_ref().expect("link result")["relation"]["relation_id"]
            .as_str()
            .expect("relation id is text")
            .parse::<Uuid>()
            .expect("relation id is a UUID");
        exec(
            &state,
            "UPDATE flow_relations SET created_at = CASE WHEN id=$1 THEN '2026-01-01T00:00:00Z'::timestamptz \
             WHEN id=$2 THEN '2026-01-01T00:00:01Z'::timestamptz ELSE created_at END WHERE id=ANY($3)",
            vec![
                hidden_relation_id.into(),
                visible_relation_id.into(),
                vec![hidden_relation_id, visible_relation_id].into(),
            ],
        )
        .await;
        let hidden_cursor = RelationCursorProbe::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT created_at FROM flow_relations WHERE id=$1",
            vec![hidden_relation_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("relation cursor query runs")
        .expect("hidden relation exists");
        exec(
            &state,
            "UPDATE flow_objects SET inherit_from_parent=false WHERE id=$1",
            vec![hidden_target.into()],
        )
        .await;
        exec(
            &state,
            "UPDATE flow_workspace_settings SET authz_epoch=authz_epoch+1 WHERE workspace_id=$1",
            vec![workspace_id.into()],
        )
        .await;
        let access = read_access(&state, workspace_id, source, member_id).await;
        let first = list_relations(
            &state,
            &access,
            ListRelationsParams {
                direction: RelationDirection::Both,
                relation_type: None,
                cursor: None,
                limit: Some(1),
            },
        )
        .await
        .expect("query succeeds")
        .expect("epoch stable");
        assert_eq!(first.items.len(), 1);
        let cursor = first.next_cursor.expect("another row exists");
        assert_eq!(
            serde_json::to_value(&first.items[0]).expect("unavailable item serializes"),
            json!({"visibility":"unavailable"}),
            "the page boundary must exercise an unavailable relation"
        );
        let decoded_cursor = decode_cursor(state.cfg.jwt_secret.expose(), &cursor)
            .expect("the server must authenticate and decrypt the opaque relation cursor");
        assert_eq!(
            decoded_cursor,
            (hidden_cursor.created_at, hidden_relation_id),
            "the opaque cursor must resume after the unavailable row without exposing it"
        );
        let raw_cursor = BASE64_URL.decode(&cursor).expect("cursor is base64url");
        assert_eq!(raw_cursor.first().copied(), Some(CURSOR_VERSION));
        let hidden_id_text = hidden_relation_id.to_string();
        let hidden_time_text = hidden_cursor.created_at.to_rfc3339();
        assert!(
            !raw_cursor
                .windows(hidden_id_text.len())
                .any(|window| window == hidden_id_text.as_bytes()),
            "opaque cursor must not contain the unavailable relation id"
        );
        assert!(
            !raw_cursor
                .windows(hidden_time_text.len())
                .any(|window| window == hidden_time_text.as_bytes()),
            "opaque cursor must not contain the unavailable relation creation time"
        );
        let access = read_access(&state, workspace_id, source, member_id).await;
        let second = list_relations(
            &state,
            &access,
            ListRelationsParams {
                direction: RelationDirection::Both,
                relation_type: None,
                cursor: Some(cursor),
                limit: Some(1),
            },
        )
        .await
        .expect("second page succeeds")
        .expect("epoch stable");
        assert_eq!(second.items.len(), 1);
        assert!(second.next_cursor.is_none());
        let values: Vec<Value> = first
            .items
            .into_iter()
            .chain(second.items)
            .map(|item| serde_json::to_value(item).expect("relation serializes"))
            .collect();
        assert!(values.iter().any(|item| item["visibility"] == "visible"));
        let unavailable = values
            .iter()
            .find(|item| item["visibility"] == "unavailable")
            .expect("hidden target retains an unavailable placeholder");
        assert_eq!(unavailable, &json!({"visibility":"unavailable"}));
        assert_eq!(unavailable.as_object().map(serde_json::Map::len), Some(1));
        let visible = values.iter().find(|item| item["visibility"] == "visible").unwrap();
        let visible_keys: std::collections::BTreeSet<&str> =
            visible.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            visible_keys,
            [
                "created_at",
                "direction",
                "other_object",
                "properties",
                "relation_id",
                "relation_type",
                "visibility",
            ]
            .into_iter()
            .collect(),
            "the visible union must not grow caller-invented fields"
        );
        assert_eq!(visible["direction"], "outgoing");
        assert_eq!(visible["other_object"]["id"], visible_target.to_string());
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn relation_read_covers_incoming_and_both_directions() {
        let scratch = scratch_or_skip!("read_directions");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id, _) = seed_workspace(&state).await;
        let root = page(&state, workspace_id, owner_id, "Root").await;
        let outgoing_target = page(&state, workspace_id, owner_id, "Outgoing target").await;
        let incoming_source = page(&state, workspace_id, owner_id, "Incoming source").await;
        for (source, target, relation_type) in [
            (root, outgoing_target, "outgoing_test"),
            (incoming_source, root, "incoming_test"),
        ] {
            execute_command(
                &state,
                relation_command(
                    source,
                    owner_id,
                    "owner",
                    "link",
                    json!({"target_object_id":target,"relation_type":relation_type}),
                    Uuid::new_v4().to_string(),
                ),
            )
            .await
            .expect("direction fixture relation links");
        }

        let incoming = list_relations(
            &state,
            &read_access(&state, workspace_id, root, owner_id).await,
            ListRelationsParams {
                direction: RelationDirection::Incoming,
                relation_type: None,
                cursor: None,
                limit: None,
            },
        )
        .await
        .expect("incoming query succeeds")
        .expect("epoch is stable");
        assert_eq!(incoming.items.len(), 1);
        let incoming_value = serde_json::to_value(&incoming.items[0]).expect("incoming item serializes");
        assert_eq!(incoming_value["direction"], "incoming");
        assert_eq!(incoming_value["other_object"]["id"], incoming_source.to_string());

        let both = list_relations(
            &state,
            &read_access(&state, workspace_id, root, owner_id).await,
            ListRelationsParams {
                direction: RelationDirection::Both,
                relation_type: None,
                cursor: None,
                limit: None,
            },
        )
        .await
        .expect("both-direction query succeeds")
        .expect("epoch is stable");
        assert_eq!(both.items.len(), 2);
        let directions: std::collections::BTreeSet<String> = both
            .items
            .iter()
            .map(|item| serde_json::to_value(item).expect("relation serializes")["direction"].to_string())
            .collect();
        assert_eq!(
            directions,
            ["\"incoming\"".to_string(), "\"outgoing\"".to_string()].into()
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn concurrent_same_key_different_links_commit_exactly_one_relation() {
        let scratch = scratch_or_skip!("idempotency_race");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id, _) = seed_workspace(&state).await;
        let source = page(&state, workspace_id, owner_id, "Source").await;
        let target_a = page(&state, workspace_id, owner_id, "Target A").await;
        let target_b = page(&state, workspace_id, owner_id, "Target B").await;
        let key = Uuid::new_v4().to_string();
        let left = execute_command(
            &state,
            relation_command(
                source,
                owner_id,
                "owner",
                "link",
                json!({"target_object_id":target_a,"relation_type":"race"}),
                key.clone(),
            ),
        );
        let right = execute_command(
            &state,
            relation_command(
                source,
                owner_id,
                "owner",
                "link",
                json!({"target_object_id":target_b,"relation_type":"race"}),
                key.clone(),
            ),
        );
        let (left, right) = tokio::join!(left, right);
        assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
        let loser = if let Err(err) = left {
            err
        } else {
            right.expect_err("exactly one command loses the idempotency race")
        };
        assert_eq!(loser.kind(), ApiErrorKind::InvalidUpdate);

        let relations = CountRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS count FROM flow_relations WHERE workspace_id=$1 AND source_object_id=$2",
            vec![workspace_id.into(), source.into()],
        ))
        .one(&state.db)
        .await
        .expect("relation count runs")
        .expect("count row");
        let events = CountRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS count FROM business_events WHERE workspace_id=$1 AND idempotency_key=$2",
            vec![workspace_id.into(), key.into()],
        ))
        .one(&state.db)
        .await
        .expect("event count runs")
        .expect("count row");
        assert_eq!(relations.count, 1, "the losing relation insert must roll back");
        assert_eq!(events.count, 1, "the database idempotency index owns the race");
        scratch.drop_self().await;
    }

    #[derive(FromQueryResult)]
    struct CountRow {
        count: i64,
    }

    #[tokio::test]
    async fn corrupted_cross_workspace_relation_fails_closed_and_records_integrity() {
        let scratch = scratch_or_skip!("integrity");
        let state = state_for(scratch.db.clone());
        let (workspace_a, owner_a, _) = seed_workspace(&state).await;
        let (workspace_b, owner_b, _) = seed_workspace(&state).await;
        let source = page(&state, workspace_a, owner_a, "Source").await;
        let target = page(&state, workspace_b, owner_b, "Foreign target").await;
        state
            .db
            .execute_unprepared("ALTER TABLE flow_relations DROP CONSTRAINT flow_relations_target_workspace_fk")
            .await
            .expect("fixture deliberately removes the target workspace guard");
        let relation_id = Uuid::new_v4();
        exec(
            &state,
            "INSERT INTO flow_relations (id,workspace_id,relation_type,source_object_id,target_object_id) \
             VALUES ($1,$2,'corrupt',$3,$4)",
            vec![relation_id.into(), workspace_a.into(), source.into(), target.into()],
        )
        .await;
        let access = read_access(&state, workspace_a, source, owner_a).await;
        let err = list_relations(
            &state,
            &access,
            ListRelationsParams {
                direction: RelationDirection::Both,
                relation_type: None,
                cursor: None,
                limit: None,
            },
        )
        .await
        .expect_err("corruption must fail the whole page");
        assert_eq!(err.kind(), ApiErrorKind::InvalidUpdate);
        let alerts = CountRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS count FROM flow_integrity_records WHERE workspace_id=$1 AND kind='cross_workspace_relation' \
             AND subject_kind='flow_relation' AND subject_id=$2",
            vec![workspace_a.into(), relation_id.to_string().into()],
        ))
        .one(&state.db)
        .await
        .expect("integrity query runs")
        .expect("count row");
        assert_eq!(alerts.count, 1);
        scratch.drop_self().await;
    }
}
