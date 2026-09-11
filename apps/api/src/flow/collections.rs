//! Flow v0.6 Collection commands and their synchronous projections.
//!
//! Collection schema/view nodes live in the Collection document. Every Record is a separate
//! `flow_objects`/`collab_documents` aggregate; only rebuildable typed values live in projection
//! tables. No path in this module reads or writes Universal Forms tables.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
use collab_core::{CollabEngine, LoroCollabEngine, NodeId, NodeKind, Operation};
use platform::app::AppState;
use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::ApiError;
use crate::events::{BusinessEventInput, FlowDispatchSpec, insert_flow_event};

use super::collab::{authz, bootstrap, frame, runtime, write};
use super::command::{ExecuteCommandInput, ExistingDocumentCardinality, actor_user_id, map_write_rejection};
use super::model::AcceptedChange;
use super::repository::{self, NewCollabDocument, NewFlowObject, NewProjection};
use super::{policy, projection, query};

const DOCUMENT_FORMAT_VERSION: &str = "loro-1";
const LABEL_MAX_CHARS: usize = 500;
const MAX_REBASE_ATTEMPTS: u32 = 3;
const QUERY_CURSOR_VERSION: u8 = 1;
const QUERY_CURSOR_AAD: &[u8] = b"openpr.flow.collections.query.cursor.v1";
const QUERY_SCAN_ROWS_MAX: u32 = 1_000;

#[derive(FromQueryResult)]
struct FieldTypeRow {
    field_type: String,
}

#[derive(FromQueryResult)]
struct ProjectedFieldRow {
    field_type: String,
    archived: bool,
}

#[derive(FromQueryResult)]
struct CountRow {
    count: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionCommandType {
    FieldCreate,
    FieldUpdate,
    FieldArchive,
    FieldReorder,
    ViewCreate,
    ViewUpdate,
    ViewReorder,
    RecordCreate,
    RecordPatch,
    RecordArchive,
    RecordQuery,
    CreateCollectionEmbed,
}

impl CollectionCommandType {
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "field_create" => Some(Self::FieldCreate),
            "field_update" => Some(Self::FieldUpdate),
            "field_archive" => Some(Self::FieldArchive),
            "field_reorder" => Some(Self::FieldReorder),
            "view_create" => Some(Self::ViewCreate),
            "view_update" => Some(Self::ViewUpdate),
            "view_reorder" => Some(Self::ViewReorder),
            "record_create" => Some(Self::RecordCreate),
            "record_patch" => Some(Self::RecordPatch),
            "record_archive" => Some(Self::RecordArchive),
            "record_query" => Some(Self::RecordQuery),
            "create_collection_embed" => Some(Self::CreateCollectionEmbed),
            _ => None,
        }
    }

    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::FieldCreate => "field_create",
            Self::FieldUpdate => "field_update",
            Self::FieldArchive => "field_archive",
            Self::FieldReorder => "field_reorder",
            Self::ViewCreate => "view_create",
            Self::ViewUpdate => "view_update",
            Self::ViewReorder => "view_reorder",
            Self::RecordCreate => "record_create",
            Self::RecordPatch => "record_patch",
            Self::RecordArchive => "record_archive",
            Self::RecordQuery => "record_query",
            Self::CreateCollectionEmbed => "create_collection_embed",
        }
    }

    #[must_use]
    pub const fn existing_document_cardinality(self) -> ExistingDocumentCardinality {
        match self {
            Self::FieldCreate
            | Self::FieldUpdate
            | Self::FieldArchive
            | Self::FieldReorder
            | Self::ViewCreate
            | Self::ViewUpdate
            | Self::ViewReorder
            | Self::RecordPatch
            | Self::CreateCollectionEmbed => ExistingDocumentCardinality::One,
            Self::RecordCreate | Self::RecordArchive | Self::RecordQuery => ExistingDocumentCardinality::Zero,
        }
    }
}

#[must_use]
pub fn v0_6_command_cardinality_registry() -> Vec<(&'static str, ExistingDocumentCardinality)> {
    [
        CollectionCommandType::FieldCreate,
        CollectionCommandType::FieldUpdate,
        CollectionCommandType::FieldArchive,
        CollectionCommandType::FieldReorder,
        CollectionCommandType::ViewCreate,
        CollectionCommandType::ViewUpdate,
        CollectionCommandType::ViewReorder,
        CollectionCommandType::RecordCreate,
        CollectionCommandType::RecordPatch,
        CollectionCommandType::RecordArchive,
        CollectionCommandType::RecordQuery,
        CollectionCommandType::CreateCollectionEmbed,
    ]
    .into_iter()
    .map(|kind| (kind.wire_name(), kind.existing_document_cardinality()))
    .collect()
}

pub async fn insert_collection_projection<C: ConnectionTrait>(
    conn: &C,
    collection_id: Uuid,
    document_id: Uuid,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    display_settings: Value,
) -> Result<(), ApiError> {
    if !display_settings.is_object() {
        return Err(ApiError::invalid_update("display_settings must be an object"));
    }
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_collection_projections \
         (collection_id, document_id, workspace_id, project_id, schema_seq, display_settings) \
         VALUES ($1, $2, $3, $4, 0, $5)",
        vec![
            collection_id.into(),
            document_id.into(),
            workspace_id.into(),
            project_id.into(),
            display_settings.into(),
        ],
    ))
    .await?;
    Ok(())
}

fn parse_payload<T: for<'de> Deserialize<'de>>(name: &str, payload: &Value) -> Result<T, ApiError> {
    serde_json::from_value(payload.clone()).map_err(|_| ApiError::invalid_update(format!("invalid {name} payload")))
}

fn validate_label(label: &str, field: &str) -> Result<String, ApiError> {
    let normalized = label.trim();
    if normalized.is_empty() || normalized.chars().count() > LABEL_MAX_CHARS {
        return Err(ApiError::invalid_update(format!(
            "{field} must contain 1-{LABEL_MAX_CHARS} characters"
        )));
    }
    Ok(normalized.to_string())
}

fn validate_field_type(field_type: &str) -> Result<(), ApiError> {
    if matches!(
        field_type,
        "text" | "number" | "boolean" | "date" | "select" | "multi_select" | "relation"
    ) {
        Ok(())
    } else {
        Err(ApiError::invalid_update("unsupported collection field type"))
    }
}

fn validate_view_type(view_type: &str) -> Result<(), ApiError> {
    if matches!(view_type, "table" | "board") {
        Ok(())
    } else {
        Err(ApiError::invalid_update("unsupported collection view type"))
    }
}

fn validate_field_config(config: &Map<String, Value>) -> Result<bool, ApiError> {
    match config.get("restricted") {
        None => Ok(false),
        Some(Value::Bool(restricted)) => Ok(*restricted),
        Some(_) => Err(ApiError::invalid_update("field config restricted must be a boolean")),
    }
}

#[derive(Debug, Deserialize)]
struct FieldCreatePayload {
    field_id: Uuid,
    label: String,
    field_type: String,
    #[serde(default)]
    config: Map<String, Value>,
    #[serde(default)]
    index: u32,
}

#[derive(Debug, Deserialize)]
struct FieldUpdatePayload {
    field_id: Uuid,
    label: Option<String>,
    #[serde(default)]
    config: Option<Map<String, Value>>,
    field_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdPayload {
    field_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct FieldReorderPayload {
    field_id: Uuid,
    index: u32,
}

#[derive(Debug, Deserialize)]
struct ViewCreatePayload {
    view_id: Uuid,
    name: String,
    view_type: String,
    #[serde(default)]
    config: Map<String, Value>,
    #[serde(default)]
    index: u32,
}

#[derive(Debug, Deserialize)]
struct ViewUpdatePayload {
    view_id: Uuid,
    name: Option<String>,
    #[serde(default)]
    config: Option<Map<String, Value>>,
    view_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ViewReorderPayload {
    view_id: Uuid,
    index: u32,
}

#[derive(Debug, Deserialize)]
struct RecordCreatePayload {
    #[serde(default)]
    record_id: Option<Uuid>,
    #[serde(default)]
    properties: Map<String, Value>,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RecordPatchPayload {
    record_id: Uuid,
    #[serde(default)]
    properties: Map<String, Value>,
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RecordIdPayload {
    record_id: Uuid,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RecordFilter {
    pub field_id: Uuid,
    pub op: String,
    pub value: Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RecordSort {
    pub field_id: Uuid,
    #[serde(default = "default_sort_direction")]
    pub direction: String,
}

fn default_sort_direction() -> String {
    "asc".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RecordQueryPayload {
    #[serde(default)]
    pub filter: Option<RecordFilter>,
    #[serde(default)]
    pub sort: Option<RecordSort>,
    #[serde(default)]
    pub group: Option<Uuid>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default = "default_query_limit")]
    pub limit: u32,
    #[serde(default)]
    pub field_ids: Vec<Uuid>,
}

const fn default_query_limit() -> u32 {
    50
}

#[derive(Debug, Deserialize, serde::Serialize, PartialEq, Eq)]
struct EmbedIdempotencyBody {
    title: String,
    display_settings: Map<String, Value>,
    initial_schema: Value,
}

#[derive(Debug, Deserialize)]
struct CreateCollectionEmbedPayload {
    title: String,
    #[serde(default)]
    display_settings: Map<String, Value>,
    #[serde(default)]
    initial_fields: Vec<Value>,
    #[serde(default)]
    initial_view: Option<Value>,
}

pub fn apply_initial_collection_schema(engine: &mut LoroCollabEngine, schema: &Value) -> Result<(), ApiError> {
    let fields = schema
        .get("initial_fields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for raw in fields {
        let field: FieldCreatePayload = parse_payload("initial collection field", &raw)?;
        validate_field_type(&field.field_type)?;
        let _ = validate_field_config(&field.config)?;
        let label = validate_label(&field.label, "field label")?;
        let id = node_id(field.field_id);
        for operation in [
            Operation::CreateNode {
                id: id.clone(),
                parent: None,
                index: field.index,
                kind: NodeKind::CollectionField,
            },
            Operation::SetProperty {
                id: id.clone(),
                key: "field_type".to_string(),
                value: field.field_type,
            },
            Operation::SetProperty {
                id: id.clone(),
                key: "label".to_string(),
                value: label,
            },
            Operation::SetProperty {
                id,
                key: "config".to_string(),
                value: serde_json::to_string(&field.config).map_err(|_| ApiError::Internal)?,
            },
        ] {
            engine
                .apply_operation(&operation)
                .map_err(|error| ApiError::invalid_update(format!("initial Collection schema rejected: {error}")))?;
        }
    }
    if let Some(raw) = schema.get("initial_view").filter(|value| !value.is_null()) {
        let view: ViewCreatePayload = parse_payload("initial collection view", raw)?;
        validate_view_type(&view.view_type)?;
        let name = validate_label(&view.name, "view name")?;
        let id = node_id(view.view_id);
        for operation in [
            Operation::CreateNode {
                id: id.clone(),
                parent: None,
                index: view.index,
                kind: NodeKind::CollectionView,
            },
            Operation::SetProperty {
                id: id.clone(),
                key: "view_type".to_string(),
                value: view.view_type,
            },
            Operation::SetProperty {
                id: id.clone(),
                key: "name".to_string(),
                value: name,
            },
            Operation::SetProperty {
                id,
                key: "config".to_string(),
                value: serde_json::to_string(&view.config).map_err(|_| ApiError::Internal)?,
            },
        ] {
            engine
                .apply_operation(&operation)
                .map_err(|error| ApiError::invalid_update(format!("initial Collection schema rejected: {error}")))?;
        }
    }
    Ok(())
}

fn node_id(id: Uuid) -> NodeId {
    Arc::<str>::from(id.to_string())
}

fn engine_at_head(boot: &bootstrap::BootstrapResult) -> Result<LoroCollabEngine, ApiError> {
    let mut engine = LoroCollabEngine::load(&boot.snapshot).map_err(|_| ApiError::Internal)?;
    for update in &boot.tail_updates {
        engine.import_update(&update.bytes).map_err(|_| ApiError::Internal)?;
    }
    Ok(engine)
}

fn export_operations(boot: &bootstrap::BootstrapResult, operations: &[Operation]) -> Result<Vec<u8>, ApiError> {
    let mut engine = engine_at_head(boot)?;
    let base = engine.frontier();
    for operation in operations {
        engine
            .apply_operation(operation)
            .map_err(|err| ApiError::invalid_update(format!("collection semantic operation rejected: {err}")))?;
    }
    engine.export_from(&base).map_err(|_| ApiError::Internal)
}

async fn sync_collection_projection(
    tx: &sea_orm::DatabaseTransaction,
    collection_id: Uuid,
    document_seq: i64,
    candidate: &LoroCollabEngine,
) -> Result<(), ApiError> {
    let semantic = candidate.semantic_snapshot().map_err(|_| ApiError::Internal)?;
    let mut field_secrecy_enabled = false;
    let mut fields: Vec<_> = semantic
        .nodes
        .iter()
        .filter(|(_, node)| node.kind == NodeKind::CollectionField)
        .collect();
    fields.sort_by(|(id_a, a), (id_b, b)| a.order_key.cmp(&b.order_key).then(id_a.cmp(id_b)));
    for (position, (id, node)) in fields.into_iter().enumerate() {
        let field_id = Uuid::parse_str(id).map_err(|_| ApiError::invalid_update("field id is not a UUID"))?;
        let field_type = node
            .properties
            .get("field_type")
            .ok_or_else(|| ApiError::invalid_update("field type is missing"))?;
        if let Err(error) = validate_field_type(field_type) {
            if node.deleted {
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "DELETE FROM flow_field_projections WHERE collection_id = $1 AND field_id = $2",
                    vec![collection_id.into(), field_id.into()],
                ))
                .await?;
                continue;
            }
            return Err(error);
        }
        let label = node
            .properties
            .get("label")
            .ok_or_else(|| ApiError::invalid_update("field label is missing"))?;
        let label = validate_label(label, "field label")?;
        let config: Value = node
            .properties
            .get("config")
            .map_or_else(|| Ok(json!({})), |raw| serde_json::from_str(raw))
            .map_err(|_| ApiError::invalid_update("field config is invalid"))?;
        let config_object = config
            .as_object()
            .ok_or_else(|| ApiError::invalid_update("field config must be an object"))?;
        let restricted = validate_field_config(config_object)?;
        if !node.deleted {
            field_secrecy_enabled |= restricted;
        }
        let position = i64::try_from(position).map_err(|_| ApiError::Internal)?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO flow_field_projections
                    (collection_id, field_id, field_type, label, config, position, document_seq, archived_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, CASE WHEN $8 THEN now() ELSE NULL END)
                ON CONFLICT (collection_id, field_id) DO UPDATE SET
                    field_type = EXCLUDED.field_type, label = EXCLUDED.label,
                    config = EXCLUDED.config, position = EXCLUDED.position,
                    document_seq = EXCLUDED.document_seq,
                    archived_at = CASE WHEN $8 THEN COALESCE(flow_field_projections.archived_at, now()) ELSE NULL END,
                    updated_at = now()
            ",
            vec![
                collection_id.into(),
                field_id.into(),
                field_type.clone().into(),
                label.into(),
                config.into(),
                position.into(),
                document_seq.into(),
                node.deleted.into(),
            ],
        ))
        .await?;
        if node.deleted {
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM flow_record_value_projections WHERE collection_id = $1 AND field_id = $2",
                vec![collection_id.into(), field_id.into()],
            ))
            .await?;
        }
    }

    let mut views: Vec<_> = semantic
        .nodes
        .iter()
        .filter(|(_, node)| node.kind == NodeKind::CollectionView && !node.deleted)
        .collect();
    views.sort_by(|(id_a, a), (id_b, b)| a.order_key.cmp(&b.order_key).then(id_a.cmp(id_b)));
    for (position, (id, node)) in views.into_iter().enumerate() {
        let view_id = Uuid::parse_str(id).map_err(|_| ApiError::invalid_update("view id is not a UUID"))?;
        let view_type = node
            .properties
            .get("view_type")
            .ok_or_else(|| ApiError::invalid_update("view type is missing"))?;
        validate_view_type(view_type)?;
        let name = node
            .properties
            .get("name")
            .ok_or_else(|| ApiError::invalid_update("view name is missing"))?;
        let name = validate_label(name, "view name")?;
        let config: Value = node
            .properties
            .get("config")
            .map_or_else(|| Ok(json!({})), |raw| serde_json::from_str(raw))
            .map_err(|_| ApiError::invalid_update("view config is invalid"))?;
        let position = i64::try_from(position).map_err(|_| ApiError::Internal)?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO flow_view_projections
                    (collection_id, view_id, view_type, name, config, position, document_seq)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                ON CONFLICT (collection_id, view_id) DO UPDATE SET
                    view_type = EXCLUDED.view_type, name = EXCLUDED.name,
                    config = EXCLUDED.config, position = EXCLUDED.position,
                    document_seq = EXCLUDED.document_seq, updated_at = now()
            ",
            vec![
                collection_id.into(),
                view_id.into(),
                view_type.clone().into(),
                name.into(),
                config.into(),
                position.into(),
                document_seq.into(),
            ],
        ))
        .await?;
    }
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_collection_projections SET schema_seq = $2, field_secrecy_enabled = $3, \
         client_crdt_enabled = CASE WHEN $3 THEN false ELSE client_crdt_enabled END, updated_at = now() \
         WHERE collection_id = $1",
        vec![collection_id.into(), document_seq.into(), field_secrecy_enabled.into()],
    ))
    .await?;
    Ok(())
}

pub async fn sync_new_collection_projection(
    tx: &sea_orm::DatabaseTransaction,
    collection_id: Uuid,
    engine: &LoroCollabEngine,
) -> Result<(), ApiError> {
    sync_collection_projection(tx, collection_id, 0, engine).await
}

async fn sync_record_projection(
    tx: &sea_orm::DatabaseTransaction,
    collection_id: Uuid,
    record_id: Uuid,
    document_seq: i64,
    candidate: &LoroCollabEngine,
) -> Result<(), ApiError> {
    let semantic = candidate.semantic_snapshot().map_err(|_| ApiError::Internal)?;
    let mut properties = Map::new();
    let mut body = None;
    for (id, node) in &semantic.nodes {
        if node.deleted {
            continue;
        }
        match node.kind {
            NodeKind::RecordProperty => {
                let value = node
                    .properties
                    .get("value")
                    .ok_or_else(|| ApiError::invalid_update("record property value is missing"))?;
                let value = serde_json::from_str(value)
                    .map_err(|_| ApiError::invalid_update("record property value is invalid"))?;
                properties.insert(id.to_string(), value);
            }
            NodeKind::Block if id.as_ref() == "record:body" => {
                body = node.properties.get("body").cloned();
            }
            _ => {}
        }
    }
    let properties_value = Value::Object(properties.clone());
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_record_projections SET properties = $2, has_body = $3, document_seq = $4, updated_at = now() \
         WHERE record_id = $1 AND collection_id = $5",
        vec![
            record_id.into(),
            properties_value.into(),
            body.is_some().into(),
            document_seq.into(),
            collection_id.into(),
        ],
    ))
    .await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM flow_record_value_projections WHERE record_id = $1",
        vec![record_id.into()],
    ))
    .await?;
    for (field_id, value) in properties {
        let field_id = Uuid::parse_str(&field_id)
            .map_err(|_| ApiError::invalid_update("record property key is not a field UUID"))?;
        let field = ProjectedFieldRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT field_type, archived_at IS NOT NULL AS archived FROM flow_field_projections \
             WHERE collection_id = $1 AND field_id = $2",
            vec![collection_id.into(), field_id.into()],
        ))
        .one(tx)
        .await?
        .ok_or_else(|| ApiError::invalid_update("record property references an unknown field"))?;
        if field.archived || value.is_null() {
            continue;
        }
        validate_typed_value(&field.field_type, &value)?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO flow_record_value_projections
                    (record_id, collection_id, field_id, field_type, text_value, number_value,
                     boolean_value, date_value, select_value, multi_select_value, relation_value,
                     document_seq)
                VALUES (
                    $1, $2, $3, $4,
                    CASE WHEN $4 = 'text' THEN $5 #>> '{}' END,
                    CASE WHEN $4 = 'number' THEN ($5 #>> '{}')::numeric END,
                    CASE WHEN $4 = 'boolean' THEN ($5 #>> '{}')::boolean END,
                    CASE WHEN $4 = 'date' THEN ($5 #>> '{}')::timestamptz END,
                    CASE WHEN $4 = 'select' THEN $5 #>> '{}' END,
                    CASE WHEN $4 = 'multi_select' THEN $5 END,
                    CASE WHEN $4 = 'relation' THEN $5 END,
                    $6
                )
            ",
            vec![
                record_id.into(),
                collection_id.into(),
                field_id.into(),
                field.field_type.into(),
                value.into(),
                document_seq.into(),
            ],
        ))
        .await?;
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct ProjectionRebuildResult {
    pub collection_id: Uuid,
    pub collection_seq: i64,
    pub record_count: usize,
    pub field_count: i64,
    pub value_count: i64,
}

#[derive(Debug, FromQueryResult)]
struct RebuildDocumentRow {
    record_id: Uuid,
    document_id: Uuid,
    document_seq: i64,
}

/// Rebuilds only the typed, disposable Collection projections from canonical collab documents.
///
/// All canonical snapshots/tails are loaded before the replacement transaction starts, then the
/// derived rows are replaced atomically so readers never observe a half-rebuilt schema/value set.
pub async fn rebuild_typed_projections(
    state: &AppState,
    collection_id: Uuid,
) -> Result<ProjectionRebuildResult, ApiError> {
    #[derive(FromQueryResult)]
    struct CollectionDocumentRow {
        document_id: Uuid,
        schema_seq: i64,
    }
    let collection = CollectionDocumentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT document_id, schema_seq FROM flow_collection_projections WHERE collection_id = $1",
        vec![collection_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("collection not found".to_string()))?;
    let collection_boot = bootstrap::load(&state.db, collection.document_id).await?;
    let collection_engine = engine_at_head(&collection_boot)?;
    let records = RebuildDocumentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT record_id, document_id, document_seq FROM flow_record_projections \
         WHERE collection_id = $1 ORDER BY record_id",
        vec![collection_id.into()],
    ))
    .all(&state.db)
    .await?;
    let mut record_engines = Vec::with_capacity(records.len());
    for record in &records {
        let boot = bootstrap::load(&state.db, record.document_id).await?;
        record_engines.push(engine_at_head(&boot)?);
    }

    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM flow_record_value_projections WHERE collection_id = $1",
        vec![collection_id.into()],
    ))
    .await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM flow_view_projections WHERE collection_id = $1",
        vec![collection_id.into()],
    ))
    .await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM flow_field_projections WHERE collection_id = $1",
        vec![collection_id.into()],
    ))
    .await?;
    sync_collection_projection(&tx, collection_id, collection.schema_seq, &collection_engine).await?;
    for (record, engine) in records.iter().zip(&record_engines) {
        sync_record_projection(&tx, collection_id, record.record_id, record.document_seq, engine).await?;
    }
    let field_count = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT count(*) AS count FROM flow_field_projections WHERE collection_id = $1",
        vec![collection_id.into()],
    ))
    .one(&tx)
    .await?
    .ok_or(ApiError::Internal)?
    .count;
    let value_count = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT count(*) AS count FROM flow_record_value_projections WHERE collection_id = $1",
        vec![collection_id.into()],
    ))
    .one(&tx)
    .await?
    .ok_or(ApiError::Internal)?
    .count;
    tx.commit().await?;
    Ok(ProjectionRebuildResult {
        collection_id,
        collection_seq: collection.schema_seq,
        record_count: records.len(),
        field_count,
        value_count,
    })
}

fn validate_typed_value(field_type: &str, value: &Value) -> Result<(), ApiError> {
    let valid = match field_type {
        "text" | "select" | "date" => value.is_string(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "multi_select" => value
            .as_array()
            .is_some_and(|values| values.iter().all(Value::is_string)),
        "relation" => value.as_array().is_some_and(|values| {
            values
                .iter()
                .all(|value| value.as_str().is_some_and(|raw| Uuid::parse_str(raw).is_ok()))
        }),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(ApiError::invalid_update(format!(
            "record value does not match field type {field_type}"
        )))
    }
}

#[derive(Clone, Copy)]
enum ProjectionSync {
    Collection { collection_id: Uuid },
    Record { collection_id: Uuid, record_id: Uuid },
}

struct SemanticEventRewrite {
    event_type: &'static str,
    aggregate_type: &'static str,
    aggregate_id: String,
    payload: Value,
}

async fn semantic_event_rewrite(
    tx: &sea_orm::DatabaseTransaction,
    input: &ExecuteCommandInput,
    sync: ProjectionSync,
    seq: i64,
) -> Result<SemanticEventRewrite, ApiError> {
    let collection_id = match sync {
        ProjectionSync::Collection { collection_id } | ProjectionSync::Record { collection_id, .. } => collection_id,
    };
    let required_uuid = |key: &str| {
        input
            .payload
            .get(key)
            .and_then(Value::as_str)
            .and_then(|raw| Uuid::parse_str(raw).ok())
            .ok_or_else(|| ApiError::invalid_update(format!("{key} must be a UUID")))
    };
    match input.command_type.as_str() {
        "field_create" | "field_update" | "field_archive" | "field_reorder" => {
            let field_id = required_uuid("field_id")?;
            let change_kind = input.command_type.trim_start_matches("field_");
            Ok(SemanticEventRewrite {
                event_type: "flow.schema.changed",
                aggregate_type: "flow_collection",
                aggregate_id: collection_id.to_string(),
                payload: json!({
                    "collection_id": collection_id,
                    "field_id": field_id,
                    "change_kind": change_kind,
                    "schema_seq": seq,
                }),
            })
        }
        "view_create" => {
            let view_id = required_uuid("view_id")?;
            let view_type = input
                .payload
                .get("view_type")
                .and_then(Value::as_str)
                .ok_or_else(|| ApiError::invalid_update("view_type is required"))?;
            Ok(SemanticEventRewrite {
                event_type: "flow.view.created",
                aggregate_type: "flow_view",
                aggregate_id: view_id.to_string(),
                payload: json!({
                    "collection_id": collection_id,
                    "view_id": view_id,
                    "view_type": view_type,
                    "schema_seq": seq,
                }),
            })
        }
        "view_update" => {
            let view_id = required_uuid("view_id")?;
            Ok(SemanticEventRewrite {
                event_type: "flow.view.updated",
                aggregate_type: "flow_view",
                aggregate_id: view_id.to_string(),
                payload: json!({
                    "collection_id": collection_id,
                    "view_id": view_id,
                    "change_kind": "updated",
                    "schema_seq": seq,
                }),
            })
        }
        "view_reorder" => {
            #[derive(FromQueryResult)]
            struct PositionRow {
                position: i64,
            }
            let view_id = required_uuid("view_id")?;
            let old = PositionRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT position FROM flow_view_projections WHERE collection_id = $1 AND view_id = $2",
                vec![collection_id.into(), view_id.into()],
            ))
            .one(tx)
            .await?
            .ok_or_else(|| ApiError::invalid_update("view does not exist"))?;
            let new_ordinal = input
                .payload
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| ApiError::invalid_update("index is required"))?;
            Ok(SemanticEventRewrite {
                event_type: "flow.view.reordered",
                aggregate_type: "flow_view",
                aggregate_id: view_id.to_string(),
                payload: json!({
                    "collection_id": collection_id,
                    "view_id": view_id,
                    "old_ordinal": old.position,
                    "new_ordinal": new_ordinal,
                    "schema_seq": seq,
                }),
            })
        }
        "record_patch" => {
            let ProjectionSync::Record { record_id, .. } = sync else {
                return Err(ApiError::Internal);
            };
            let mut changed_field_ids = input
                .payload
                .get("properties")
                .and_then(Value::as_object)
                .map(|properties| properties.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            changed_field_ids.sort();
            Ok(SemanticEventRewrite {
                event_type: "flow.record.updated",
                aggregate_type: "flow_record",
                aggregate_id: record_id.to_string(),
                payload: json!({
                    "collection_id": collection_id,
                    "record_id": record_id,
                    "changed_field_ids": changed_field_ids,
                    "body_changed": input.payload.get("body").is_some_and(|body| !body.is_null()),
                    "seq": seq,
                }),
            })
        }
        _ => Err(ApiError::Internal),
    }
}

async fn execute_document_update(
    state: &AppState,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    document_id: Uuid,
    checked_epoch: i64,
    operations: &[Operation],
    response_object_id: Uuid,
    sync: ProjectionSync,
) -> Result<AcceptedChange, ApiError> {
    let boot = bootstrap::load(&state.db, document_id).await?;
    let bytes = export_operations(&boot, operations)?;
    let expected_frontier = input
        .expected_frontier
        .as_deref()
        .map(frame::decode_bytes)
        .transpose()
        .map_err(|_| ApiError::invalid_update("expected_frontier is not valid base64"))?;
    let update_id = write::replay_stable_update_id(document_id, &input.idempotency_key);
    let collab = runtime::runtime();
    let _permit =
        collab.coordinator.acquire(document_id).await.map_err(|_| {
            ApiError::server_draining(crate::error::ServerDrainingReason::Contention, 0, "server_draining")
        })?;

    for attempt in 1..=MAX_REBASE_ATTEMPTS {
        let prepared = match write::hydrate_and_apply(
            &state.db,
            &collab.cache,
            document_id,
            update_id,
            &bytes,
            expected_frontier.as_deref(),
        )
        .await?
        {
            write::HydrateOutcome::Prepared(prepared) => prepared,
            write::HydrateOutcome::Rejected(write::AcceptOutcome::Rejected(rejected)) => {
                return Err(map_write_rejection(&rejected));
            }
            write::HydrateOutcome::Rejected(write::AcceptOutcome::Accepted(_)) => {
                return Err(ApiError::Internal);
            }
        };
        let tx = state.db.begin().await?;
        write::set_locked_phase_statement_budgets(&tx, 1).await?;
        match authz::fence_epoch_for_share(&tx, workspace_id, checked_epoch).await {
            Ok(()) => {}
            Err(ApiError::Conflict(_)) => {
                tx.rollback().await?;
                return Err(ApiError::policy_rejected("authorization changed before commit"));
            }
            Err(err) => return Err(err),
        }
        let request = write::UpdateRequest {
            document_id,
            update_id,
            bytes: bytes.clone(),
            idempotency_key: Some(input.idempotency_key.clone()),
            event_idempotency_key: Some(input.idempotency_key.clone()),
            origin_client_id: Some(input.origin_client_id.clone()),
            message: input.message.clone(),
            actor_id: input.actor_id,
            actor_is_bot: input.actor_is_bot(),
            workspace_id,
            checked_epoch,
            expected_frontier: expected_frontier.clone(),
            origin: input.origin.clone(),
        };
        let staged = write::stage_one_document(
            &tx,
            &request,
            &prepared,
            crate::config::runtime().flow.dispatch_max_attempts,
        )
        .await?;
        let write::StagedOutcome::Ready(staged) = staged else {
            tx.rollback().await?;
            if matches!(staged, write::StagedOutcome::EpochMismatch) {
                return Err(ApiError::policy_rejected("authorization changed before commit"));
            }
            if attempt == MAX_REBASE_ATTEMPTS {
                return Err(ApiError::server_draining(
                    crate::error::ServerDrainingReason::Contention,
                    0,
                    "server_draining",
                ));
            }
            continue;
        };
        let semantic_event = semantic_event_rewrite(&tx, input, sync, staged.new_head_seq).await?;
        match sync {
            ProjectionSync::Collection { collection_id } => {
                sync_collection_projection(&tx, collection_id, staged.new_head_seq, &prepared.candidate).await?;
            }
            ProjectionSync::Record {
                collection_id,
                record_id,
            } => {
                sync_record_projection(&tx, collection_id, record_id, staged.new_head_seq, &prepared.candidate).await?;
            }
        }
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE business_events SET event_type = $2, aggregate_type = $3, aggregate_id = $4, \
             payload = $5, metadata = metadata || $6::jsonb WHERE id = $1",
            vec![
                staged.event_id.into(),
                semantic_event.event_type.into(),
                semantic_event.aggregate_type.into(),
                semantic_event.aggregate_id.into(),
                semantic_event.payload.into(),
                json!({"semantic_summary": {"action": input.command_type}}).into(),
            ],
        ))
        .await?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE event_dispatch SET event_type = $2, document_id = NULL, accepted_seq = NULL WHERE event_id = $1",
            vec![staged.event_id.into(), semantic_event.event_type.into()],
        ))
        .await?;
        tx.commit().await?;

        let before_frontier = prepared.observed.head_frontier.clone();
        let accepted = write::finish_committed(
            &collab.cache,
            &collab.registry,
            None,
            &request,
            prepared,
            write::Accepted {
                update_id,
                head_seq: staged.new_head_seq,
                head_frontier: staged.after_frontier,
                projection_seq: staged.new_head_seq,
                event_id: staged.event_id,
                before_frontier,
                should_advance_snapshot: false,
            },
            false,
        );
        let row = repository::fetch_object_view(&state.db, response_object_id)
            .await?
            .ok_or(ApiError::Internal)?;
        let mut change = super::command::accepted_change_from_row(row, accepted.event_id);
        change.accepted_seq = accepted.head_seq;
        change.projection_seq = accepted.projection_seq;
        change.affected_object_ids = vec![response_object_id];
        return Ok(change);
    }
    Err(ApiError::Internal)
}

async fn collection_operations(
    state: &AppState,
    document_id: Uuid,
    kind: CollectionCommandType,
    payload: &Value,
) -> Result<Vec<Operation>, ApiError> {
    let boot = bootstrap::load(&state.db, document_id).await?;
    let engine = engine_at_head(&boot)?;
    let semantic = engine.semantic_snapshot().map_err(|_| ApiError::Internal)?;
    let operations = match kind {
        CollectionCommandType::FieldCreate => {
            let payload: FieldCreatePayload = parse_payload("field_create", payload)?;
            validate_field_type(&payload.field_type)?;
            let _ = validate_field_config(&payload.config)?;
            let label = validate_label(&payload.label, "field label")?;
            let id = node_id(payload.field_id);
            vec![
                Operation::CreateNode {
                    id: id.clone(),
                    parent: None,
                    index: payload.index,
                    kind: NodeKind::CollectionField,
                },
                Operation::SetProperty {
                    id: id.clone(),
                    key: "field_type".to_string(),
                    value: payload.field_type,
                },
                Operation::SetProperty {
                    id: id.clone(),
                    key: "label".to_string(),
                    value: label,
                },
                Operation::SetProperty {
                    id,
                    key: "config".to_string(),
                    value: serde_json::to_string(&payload.config).map_err(|_| ApiError::Internal)?,
                },
            ]
        }
        CollectionCommandType::FieldUpdate => {
            let payload: FieldUpdatePayload = parse_payload("field_update", payload)?;
            let id = node_id(payload.field_id);
            let existing = semantic
                .nodes
                .get(&id)
                .filter(|node| node.kind == NodeKind::CollectionField && !node.deleted)
                .ok_or_else(|| ApiError::invalid_update("field does not exist"))?;
            if let Some(field_type) = payload.field_type {
                validate_field_type(&field_type)?;
                if existing.properties.get("field_type") != Some(&field_type) {
                    return Err(ApiError::invalid_update("field type is immutable after creation"));
                }
            }
            let mut operations = Vec::new();
            if let Some(label) = payload.label {
                operations.push(Operation::SetProperty {
                    id: id.clone(),
                    key: "label".to_string(),
                    value: validate_label(&label, "field label")?,
                });
            }
            if let Some(config) = payload.config {
                let _ = validate_field_config(&config)?;
                operations.push(Operation::SetProperty {
                    id,
                    key: "config".to_string(),
                    value: serde_json::to_string(&config).map_err(|_| ApiError::Internal)?,
                });
            }
            if operations.is_empty() {
                return Err(ApiError::invalid_update("field_update changes nothing"));
            }
            operations
        }
        CollectionCommandType::FieldArchive => {
            let payload: IdPayload = parse_payload("field_archive", payload)?;
            vec![Operation::DeleteNode {
                id: node_id(payload.field_id),
            }]
        }
        CollectionCommandType::FieldReorder => {
            let payload: FieldReorderPayload = parse_payload("field_reorder", payload)?;
            vec![Operation::MoveNode {
                id: node_id(payload.field_id),
                new_parent: None,
                index: payload.index,
            }]
        }
        CollectionCommandType::ViewCreate => {
            let payload: ViewCreatePayload = parse_payload("view_create", payload)?;
            validate_view_type(&payload.view_type)?;
            let name = validate_label(&payload.name, "view name")?;
            let id = node_id(payload.view_id);
            vec![
                Operation::CreateNode {
                    id: id.clone(),
                    parent: None,
                    index: payload.index,
                    kind: NodeKind::CollectionView,
                },
                Operation::SetProperty {
                    id: id.clone(),
                    key: "view_type".to_string(),
                    value: payload.view_type,
                },
                Operation::SetProperty {
                    id: id.clone(),
                    key: "name".to_string(),
                    value: name,
                },
                Operation::SetProperty {
                    id,
                    key: "config".to_string(),
                    value: serde_json::to_string(&payload.config).map_err(|_| ApiError::Internal)?,
                },
            ]
        }
        CollectionCommandType::ViewUpdate => {
            let payload: ViewUpdatePayload = parse_payload("view_update", payload)?;
            let id = node_id(payload.view_id);
            let existing = semantic
                .nodes
                .get(&id)
                .filter(|node| node.kind == NodeKind::CollectionView && !node.deleted)
                .ok_or_else(|| ApiError::invalid_update("view does not exist"))?;
            if let Some(view_type) = payload.view_type {
                validate_view_type(&view_type)?;
                if existing.properties.get("view_type") != Some(&view_type) {
                    return Err(ApiError::invalid_update("view type is immutable after creation"));
                }
            }
            let mut operations = Vec::new();
            if let Some(name) = payload.name {
                operations.push(Operation::SetProperty {
                    id: id.clone(),
                    key: "name".to_string(),
                    value: validate_label(&name, "view name")?,
                });
            }
            if let Some(config) = payload.config {
                operations.push(Operation::SetProperty {
                    id,
                    key: "config".to_string(),
                    value: serde_json::to_string(&config).map_err(|_| ApiError::Internal)?,
                });
            }
            if operations.is_empty() {
                return Err(ApiError::invalid_update("view_update changes nothing"));
            }
            operations
        }
        CollectionCommandType::ViewReorder => {
            let payload: ViewReorderPayload = parse_payload("view_reorder", payload)?;
            vec![Operation::MoveNode {
                id: node_id(payload.view_id),
                new_parent: None,
                index: payload.index,
            }]
        }
        _ => return Err(ApiError::Internal),
    };
    Ok(operations)
}

async fn ensure_collection_target(
    state: &AppState,
    collection_id: Uuid,
) -> Result<repository::ObjectViewRow, ApiError> {
    let row = repository::fetch_object_view(&state.db, collection_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("collection not found".to_string()))?;
    if row.object_type != "collection" || row.lifecycle_status == "archived" {
        return Err(ApiError::invalid_update("command target must be an active collection"));
    }
    Ok(row)
}

async fn validate_properties_against_fields<C: ConnectionTrait>(
    conn: &C,
    collection_id: Uuid,
    properties: &Map<String, Value>,
) -> Result<(), ApiError> {
    for (field_id, value) in properties {
        let field_id = Uuid::parse_str(field_id)
            .map_err(|_| ApiError::invalid_update("record property key must be a field UUID"))?;
        let field = FieldTypeRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT field_type FROM flow_field_projections \
             WHERE collection_id = $1 AND field_id = $2 AND archived_at IS NULL",
            vec![collection_id.into(), field_id.into()],
        ))
        .one(conn)
        .await?
        .ok_or_else(|| ApiError::invalid_update("record property references an unknown or archived field"))?;
        if value.is_null() {
            continue;
        }
        validate_typed_value(&field.field_type, value)?;
        if field.field_type == "relation" {
            let ids = value
                .as_array()
                .ok_or_else(|| ApiError::invalid_update("relation value must be an array"))?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .ok_or_else(|| ApiError::invalid_update("relation value must contain UUID strings"))
                        .and_then(|raw| {
                            Uuid::parse_str(raw)
                                .map_err(|_| ApiError::invalid_update("relation value must contain UUID strings"))
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let count = CountRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS count FROM flow_objects \
                 WHERE workspace_id = (SELECT workspace_id FROM flow_collection_projections WHERE collection_id = $1) \
                   AND id = ANY($2)",
                vec![collection_id.into(), ids.clone().into()],
            ))
            .one(conn)
            .await?
            .ok_or(ApiError::Internal)?;
            let expected = i64::try_from(ids.len()).map_err(|_| ApiError::Internal)?;
            if count.count != expected {
                return Err(ApiError::invalid_update(
                    "relation value references an object outside the collection workspace",
                ));
            }
        }
    }
    Ok(())
}

fn build_record_engine(properties: &Map<String, Value>, body: Option<&str>) -> Result<LoroCollabEngine, ApiError> {
    let mut engine = LoroCollabEngine::new_empty(rand::random());
    for (index, (field_id, value)) in properties.iter().enumerate() {
        let field_id = Uuid::parse_str(field_id)
            .map_err(|_| ApiError::invalid_update("record property key must be a field UUID"))?;
        let id = node_id(field_id);
        let index = u32::try_from(index).map_err(|_| ApiError::Internal)?;
        engine
            .apply_operation(&Operation::CreateNode {
                id: id.clone(),
                parent: None,
                index,
                kind: NodeKind::RecordProperty,
            })
            .map_err(|err| ApiError::invalid_update(format!("record property create rejected: {err}")))?;
        engine
            .apply_operation(&Operation::SetProperty {
                id,
                key: "value".to_string(),
                value: serde_json::to_string(value).map_err(|_| ApiError::Internal)?,
            })
            .map_err(|err| ApiError::invalid_update(format!("record property value rejected: {err}")))?;
    }
    if let Some(body) = body {
        let id = Arc::<str>::from("record:body");
        engine
            .apply_operation(&Operation::CreateNode {
                id: id.clone(),
                parent: None,
                index: u32::MAX,
                kind: NodeKind::Block,
            })
            .map_err(|err| ApiError::invalid_update(format!("record body create rejected: {err}")))?;
        engine
            .apply_operation(&Operation::SetProperty {
                id,
                key: "body".to_string(),
                value: body.to_string(),
            })
            .map_err(|err| ApiError::invalid_update(format!("record body rejected: {err}")))?;
    }
    Ok(engine)
}

#[derive(Debug, Deserialize, serde::Serialize, PartialEq)]
struct RecordCreateIdempotencyBody {
    collection_id: Uuid,
    requested_record_id: Option<Uuid>,
    properties: Map<String, Value>,
    body: Option<String>,
}

fn record_create_idempotency_fingerprint(body: &RecordCreateIdempotencyBody) -> Result<String, ApiError> {
    let canonical = serde_json::to_vec(body).map_err(|_| ApiError::Internal)?;
    Ok(hex::encode(Sha256::digest(canonical)))
}

async fn replay_record_create(
    state: &AppState,
    workspace_id: Uuid,
    key: &str,
    body: &RecordCreateIdempotencyBody,
) -> Result<Option<AcceptedChange>, ApiError> {
    let Some(event) = repository::find_idempotent_event(&state.db, workspace_id, key).await? else {
        return Ok(None);
    };
    if event.event_type != "flow.record.created" {
        return Err(ApiError::Conflict(
            "idempotency_key was already used for a different operation".to_string(),
        ));
    }
    let expected = record_create_idempotency_fingerprint(body)?;
    let matches = if let Some(stored) = event.metadata.get("idempotency_fingerprint").and_then(Value::as_str) {
        stored == expected
    } else if let Some(legacy) = event.metadata.get("idempotency_body") {
        serde_json::from_value::<RecordCreateIdempotencyBody>(legacy.clone()).is_ok_and(|stored| stored == *body)
    } else {
        return Err(ApiError::Conflict(
            "record create replay identity is missing".to_string(),
        ));
    };
    if !matches {
        return Err(ApiError::Conflict(
            "idempotency_key was already used with a different record create request".to_string(),
        ));
    }
    let record_id = Uuid::parse_str(&event.aggregate_id).map_err(|_| ApiError::Internal)?;
    let row = repository::fetch_object_view(&state.db, record_id)
        .await?
        .ok_or(ApiError::Internal)?;
    let mut change = super::command::accepted_change_from_row(row, event.id);
    change.command_result = Some(json!({"record_id": record_id}));
    Ok(Some(change))
}

async fn create_record(
    state: &AppState,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    collection: &repository::ObjectViewRow,
    checked_epoch: i64,
) -> Result<AcceptedChange, ApiError> {
    let payload: RecordCreatePayload = parse_payload("record_create", &input.payload)?;
    validate_properties_against_fields(&state.db, input.object_id, &payload.properties).await?;
    let idempotency_body = RecordCreateIdempotencyBody {
        collection_id: input.object_id,
        requested_record_id: payload.record_id,
        properties: payload.properties.clone(),
        body: payload.body.clone(),
    };
    let idempotency_fingerprint = record_create_idempotency_fingerprint(&idempotency_body)?;
    if let Some(replay) = replay_record_create(state, workspace_id, &input.idempotency_key, &idempotency_body).await? {
        return Ok(replay);
    }
    let record_id = payload.record_id.unwrap_or_else(Uuid::new_v4);
    let document_id = Uuid::new_v4();
    authz::ensure_parent_can_adopt_child(&state.db, workspace_id, input.object_id).await?;
    let engine = build_record_engine(&payload.properties, payload.body.as_deref())?;
    let snapshot = engine.export_snapshot().map_err(|_| ApiError::Internal)?;
    let frontier = engine.frontier().as_bytes().to_vec();
    let semantic = engine.semantic_snapshot().map_err(|_| ApiError::Internal)?;
    let state_json = projection::state_json(&semantic).map_err(|_| ApiError::Internal)?;
    let tx = state.db.begin().await?;
    authz::fence_epoch_for_share(&tx, workspace_id, checked_epoch).await?;
    repository::insert_flow_object(
        &tx,
        &NewFlowObject {
            id: record_id,
            workspace_id,
            project_id: collection.project_id,
            object_type: "record".to_string(),
            parent_id: Some(input.object_id),
            created_by: actor_user_id(input.actor_id, input.actor_is_bot()),
            governance_metadata: json!({}),
        },
    )
    .await?;
    repository::insert_collab_document(
        &tx,
        &NewCollabDocument {
            id: document_id,
            object_id: record_id,
            format_version: DOCUMENT_FORMAT_VERSION.to_string(),
            snapshot,
            frontier: frontier.clone(),
        },
    )
    .await?;
    repository::insert_projection(
        &tx,
        &NewProjection {
            object_id: record_id,
            document_seq: 0,
            document_frontier: frontier,
            title: String::new(),
            state: state_json,
            plain_text: String::new(),
        },
    )
    .await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_record_projections \
         (record_id, collection_id, document_id, properties, has_body, document_seq) \
         VALUES ($1, $2, $3, '{}'::jsonb, false, 0)",
        vec![record_id.into(), input.object_id.into(), document_id.into()],
    ))
    .await?;
    sync_record_projection(&tx, input.object_id, record_id, 0, &engine).await?;
    let event = insert_flow_event(
        &tx,
        BusinessEventInput {
            workspace_id,
            project_id: collection.project_id,
            event_type: "flow.record.created".to_string(),
            aggregate_type: "flow_record".to_string(),
            aggregate_id: record_id.to_string(),
            actor_id: actor_user_id(input.actor_id, input.actor_is_bot()),
            source: input.origin.source_json(),
            payload: json!({"collection_id": input.object_id, "record_id": record_id, "seq": 0}),
            metadata: json!({"idempotency_fingerprint": idempotency_fingerprint}),
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
    .await?;
    if !event.was_new {
        tx.rollback().await?;
        return replay_record_create(state, workspace_id, &input.idempotency_key, &idempotency_body)
            .await?
            .ok_or(ApiError::Internal);
    }
    tx.commit().await?;
    let row = repository::fetch_object_view(&state.db, record_id)
        .await?
        .ok_or(ApiError::Internal)?;
    let mut change = super::command::accepted_change_from_row(row, event.event_id);
    change.command_result = Some(json!({"record_id": record_id}));
    Ok(change)
}

#[derive(FromQueryResult)]
struct RecordTargetRow {
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    collection_id: Uuid,
    document_id: Uuid,
    lifecycle_status: String,
}

async fn fetch_record_target<C: ConnectionTrait>(
    conn: &C,
    collection_id: Uuid,
    record_id: Uuid,
) -> Result<RecordTargetRow, ApiError> {
    RecordTargetRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT fo.workspace_id, fo.project_id, frp.collection_id, frp.document_id, fo.lifecycle_status \
         FROM flow_record_projections frp JOIN flow_objects fo ON fo.id = frp.record_id \
         WHERE frp.collection_id = $1 AND frp.record_id = $2",
        vec![collection_id.into(), record_id.into()],
    ))
    .one(conn)
    .await?
    .ok_or_else(|| ApiError::NotFound("record not found in collection".to_string()))
}

async fn record_patch(
    state: &AppState,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    checked_epoch: i64,
) -> Result<AcceptedChange, ApiError> {
    let payload: RecordPatchPayload = parse_payload("record_patch", &input.payload)?;
    if payload.properties.is_empty() && payload.body.is_none() {
        return Err(ApiError::invalid_update("record_patch changes nothing"));
    }
    validate_properties_against_fields(&state.db, input.object_id, &payload.properties).await?;
    let target = fetch_record_target(&state.db, input.object_id, payload.record_id).await?;
    if target.workspace_id != workspace_id || target.collection_id != input.object_id {
        return Err(ApiError::invalid_update("record scope mismatch"));
    }
    if target.lifecycle_status == "archived" {
        return Err(ApiError::invalid_update("record is archived"));
    }
    let level = authz::effective_permission(
        &state.db,
        workspace_id,
        payload.record_id,
        if input.actor_is_bot() { "bot" } else { "user" },
        input.actor_id,
        &input.role,
    )
    .await?;
    if level < authz::PermissionLevel::Edit {
        return Err(ApiError::policy_rejected("insufficient record permission"));
    }
    let boot = bootstrap::load(&state.db, target.document_id).await?;
    let engine = engine_at_head(&boot)?;
    let semantic = engine.semantic_snapshot().map_err(|_| ApiError::Internal)?;
    let mut operations = Vec::new();
    for (index, (field_id, value)) in payload.properties.iter().enumerate() {
        let field_id = Uuid::parse_str(field_id)
            .map_err(|_| ApiError::invalid_update("record property key must be a field UUID"))?;
        let id = node_id(field_id);
        if !semantic.nodes.contains_key(&id) {
            operations.push(Operation::CreateNode {
                id: id.clone(),
                parent: None,
                index: u32::try_from(index).map_err(|_| ApiError::Internal)?,
                kind: NodeKind::RecordProperty,
            });
        }
        operations.push(Operation::SetProperty {
            id,
            key: "value".to_string(),
            value: serde_json::to_string(value).map_err(|_| ApiError::Internal)?,
        });
    }
    if let Some(body) = payload.body {
        let id = Arc::<str>::from("record:body");
        if !semantic.nodes.contains_key(&id) {
            operations.push(Operation::CreateNode {
                id: id.clone(),
                parent: None,
                index: u32::MAX,
                kind: NodeKind::Block,
            });
        }
        operations.push(Operation::SetProperty {
            id,
            key: "body".to_string(),
            value: body,
        });
    }
    let mut change = execute_document_update(
        state,
        input,
        workspace_id,
        target.document_id,
        checked_epoch,
        &operations,
        payload.record_id,
        ProjectionSync::Record {
            collection_id: input.object_id,
            record_id: payload.record_id,
        },
    )
    .await?;
    change.command_result = Some(json!({"record_id": payload.record_id}));
    Ok(change)
}

async fn archive_record(
    state: &AppState,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    checked_epoch: i64,
) -> Result<AcceptedChange, ApiError> {
    let payload: RecordIdPayload = parse_payload("record_archive", &input.payload)?;
    let target = fetch_record_target(&state.db, input.object_id, payload.record_id).await?;
    if target.lifecycle_status == "archived" {
        return Err(ApiError::Conflict("record is already archived".to_string()));
    }
    let level = authz::effective_permission(
        &state.db,
        workspace_id,
        payload.record_id,
        if input.actor_is_bot() { "bot" } else { "user" },
        input.actor_id,
        &input.role,
    )
    .await?;
    if level < authz::PermissionLevel::Edit {
        return Err(ApiError::policy_rejected("insufficient record permission"));
    }
    let tx = state.db.begin().await?;
    authz::fence_epoch_for_share(&tx, workspace_id, checked_epoch).await?;
    let updated = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_objects SET lifecycle_status = 'archived', archived_at = now(), updated_at = now(), \
             updated_by = $2 WHERE id = $1 AND lifecycle_status = 'active'",
            vec![
                payload.record_id.into(),
                actor_user_id(input.actor_id, input.actor_is_bot()).into(),
            ],
        ))
        .await?;
    if updated.rows_affected() != 1 {
        return Err(ApiError::Conflict("record archive lost a concurrent race".to_string()));
    }
    let event = insert_flow_event(
        &tx,
        BusinessEventInput {
            workspace_id,
            project_id: target.project_id,
            event_type: "flow.record.archived".to_string(),
            aggregate_type: "flow_record".to_string(),
            aggregate_id: payload.record_id.to_string(),
            actor_id: actor_user_id(input.actor_id, input.actor_is_bot()),
            source: input.origin.source_json(),
            payload: json!({"collection_id": input.object_id, "record_id": payload.record_id, "status": "archived"}),
            metadata: json!({"message": input.message}),
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
    .await?;
    tx.commit().await?;
    let row = repository::fetch_object_view(&state.db, payload.record_id)
        .await?
        .ok_or(ApiError::Internal)?;
    let mut change = super::command::accepted_change_from_row(row, event.event_id);
    change.command_result = Some(json!({"record_id": payload.record_id}));
    Ok(change)
}

#[derive(Debug, FromQueryResult)]
struct CollectionReadRow {
    schema_seq: i64,
    client_crdt_enabled: bool,
    field_secrecy_enabled: bool,
}

#[derive(Debug, FromQueryResult, Serialize)]
pub struct CollectionFieldView {
    pub field_id: Uuid,
    pub label: String,
    #[serde(rename = "type")]
    pub field_type: String,
    pub config: Value,
    pub position: i64,
    pub archived: bool,
    pub document_seq: i64,
}

#[derive(Debug, FromQueryResult, Serialize)]
pub struct CollectionViewView {
    pub view_id: Uuid,
    pub name: String,
    #[serde(rename = "type")]
    pub view_type: String,
    pub config: Value,
    pub position: i64,
    pub document_seq: i64,
}

#[derive(Debug, Serialize)]
pub struct CollectionDescription {
    pub object: super::model::FlowObjectView,
    pub fields: Vec<CollectionFieldView>,
    pub views: Vec<CollectionViewView>,
    pub record_count: i64,
    pub schema_seq: i64,
    pub projection_seq: i64,
    pub client_crdt_enabled: bool,
    pub field_secrecy_enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct RecordQueryItem {
    pub record: super::model::FlowObjectView,
    pub values_by_field_id: Map<String, Value>,
}

#[derive(Debug, Serialize)]
pub struct RecordQueryResponse {
    pub fields: Vec<CollectionFieldView>,
    pub items: Vec<RecordQueryItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub projection_seq: i64,
    pub schema_seq: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct RecordQueryCursor {
    fingerprint: String,
    offset: u32,
}

#[derive(Debug, FromQueryResult)]
struct RecordCandidateRow {
    record_id: Uuid,
    properties: Value,
    document_seq: i64,
}

fn query_cursor_key(secret: &str) -> Result<LessSafeKey, ApiError> {
    let mut digest = Sha256::new();
    digest.update(QUERY_CURSOR_AAD);
    digest.update([0]);
    digest.update(secret.as_bytes());
    UnboundKey::new(&aead::CHACHA20_POLY1305, &digest.finalize())
        .map(LessSafeKey::new)
        .map_err(|_| ApiError::Internal)
}

fn query_fingerprint(query: &RecordQueryPayload) -> Result<String, ApiError> {
    let mut canonical = query.clone();
    canonical.cursor = None;
    let bytes = serde_json::to_vec(&canonical).map_err(|_| ApiError::Internal)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn encode_query_cursor(secret: &str, cursor: &RecordQueryCursor) -> Result<String, ApiError> {
    let mut nonce_bytes = [0_u8; aead::NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce_bytes)
        .map_err(|_| ApiError::Internal)?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);
    let mut encrypted = serde_json::to_vec(cursor).map_err(|_| ApiError::Internal)?;
    query_cursor_key(secret)?
        .seal_in_place_append_tag(nonce, Aad::from(QUERY_CURSOR_AAD), &mut encrypted)
        .map_err(|_| ApiError::Internal)?;
    let mut token = Vec::with_capacity(1 + aead::NONCE_LEN + encrypted.len());
    token.push(QUERY_CURSOR_VERSION);
    token.extend_from_slice(&nonce_bytes);
    token.extend_from_slice(&encrypted);
    Ok(BASE64_URL.encode(token))
}

fn decode_query_cursor(secret: &str, raw: &str, fingerprint: &str) -> Result<u32, ApiError> {
    let token = BASE64_URL
        .decode(raw)
        .map_err(|_| ApiError::invalid_update("cursor is not valid"))?;
    let (version, payload) = token
        .split_first()
        .ok_or_else(|| ApiError::invalid_update("cursor is not valid"))?;
    if *version != QUERY_CURSOR_VERSION {
        return Err(ApiError::invalid_update("cursor is not valid"));
    }
    let (nonce_bytes, ciphertext) = payload
        .split_at_checked(aead::NONCE_LEN)
        .ok_or_else(|| ApiError::invalid_update("cursor is not valid"))?;
    let nonce =
        Nonce::try_assume_unique_for_key(nonce_bytes).map_err(|_| ApiError::invalid_update("cursor is not valid"))?;
    let mut in_out = ciphertext.to_vec();
    let plaintext = query_cursor_key(secret)?
        .open_in_place(nonce, Aad::from(QUERY_CURSOR_AAD), &mut in_out)
        .map_err(|_| ApiError::invalid_update("cursor is not valid"))?;
    let cursor: RecordQueryCursor =
        serde_json::from_slice(plaintext).map_err(|_| ApiError::invalid_update("cursor is not valid"))?;
    if cursor.fingerprint != fingerprint {
        return Err(ApiError::invalid_update(
            "cursor belongs to a different collection query",
        ));
    }
    Ok(cursor.offset)
}

async fn collection_read_row<C: ConnectionTrait>(conn: &C, collection_id: Uuid) -> Result<CollectionReadRow, ApiError> {
    CollectionReadRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT schema_seq, client_crdt_enabled, field_secrecy_enabled \
         FROM flow_collection_projections WHERE collection_id = $1",
        vec![collection_id.into()],
    ))
    .one(conn)
    .await?
    .ok_or_else(|| ApiError::NotFound("collection not found".to_string()))
}

async fn collection_fields<C: ConnectionTrait>(
    conn: &C,
    collection_id: Uuid,
    include_archived: bool,
) -> Result<Vec<CollectionFieldView>, ApiError> {
    let archived_clause = if include_archived {
        ""
    } else {
        " AND archived_at IS NULL"
    };
    let sql = format!(
        "SELECT field_id, label, field_type, config, position, archived_at IS NOT NULL AS archived, document_seq \
         FROM flow_field_projections WHERE collection_id = $1{archived_clause} ORDER BY position, field_id"
    );
    Ok(CollectionFieldView::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        vec![collection_id.into()],
    ))
    .all(conn)
    .await?)
}

async fn collection_views<C: ConnectionTrait>(
    conn: &C,
    collection_id: Uuid,
) -> Result<Vec<CollectionViewView>, ApiError> {
    Ok(CollectionViewView::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT view_id, name, view_type, config, position, document_seq \
         FROM flow_view_projections WHERE collection_id = $1 ORDER BY position, view_id",
        vec![collection_id.into()],
    ))
    .all(conn)
    .await?)
}

fn field_is_restricted(field: &CollectionFieldView) -> bool {
    match field.config.get("restricted") {
        None | Some(Value::Bool(false)) => false,
        Some(_) => true,
    }
}

pub async fn describe_collection(
    state: &AppState,
    access: &policy::AuthorizedFlowObject,
    at_seq: Option<i64>,
) -> Result<Option<CollectionDescription>, ApiError> {
    let object_row = repository::fetch_object_view(&state.db, access.object_id())
        .await?
        .ok_or_else(|| ApiError::NotFound("collection not found".to_string()))?;
    if object_row.object_type != "collection" || object_row.workspace_id != access.workspace_id() {
        return Err(ApiError::NotFound("collection not found".to_string()));
    }
    let collection = collection_read_row(&state.db, access.object_id()).await?;
    if let Some(at_seq) = at_seq
        && at_seq != collection.schema_seq
    {
        return Err(ApiError::invalid_update(
            "at_seq is not the current collection schema sequence",
        ));
    }
    let mut fields = collection_fields(&state.db, access.object_id(), true).await?;
    if collection.field_secrecy_enabled {
        fields.retain(|field| !field_is_restricted(field));
    }
    let views = collection_views(&state.db, access.object_id()).await?;
    let count = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT count(*) AS count FROM flow_record_projections rp \
         JOIN flow_objects fo ON fo.id = rp.record_id \
         WHERE rp.collection_id = $1 AND fo.lifecycle_status = 'active'",
        vec![access.object_id().into()],
    ))
    .one(&state.db)
    .await?
    .ok_or(ApiError::Internal)?;
    if !policy::ensure_epoch_current(state, access.context()).await? {
        return Ok(None);
    }
    Ok(Some(CollectionDescription {
        object: query::object_view_from_row(object_row),
        fields,
        views,
        record_count: count.count,
        schema_seq: collection.schema_seq,
        projection_seq: collection.schema_seq,
        client_crdt_enabled: collection.client_crdt_enabled,
        field_secrecy_enabled: collection.field_secrecy_enabled,
    }))
}

fn typed_column(field_type: &str) -> Result<&'static str, ApiError> {
    match field_type {
        "text" => Ok("text_value"),
        "number" => Ok("number_value"),
        "boolean" => Ok("boolean_value"),
        "date" => Ok("date_value"),
        "select" => Ok("select_value"),
        "multi_select" => Ok("multi_select_value"),
        "relation" => Ok("relation_value"),
        _ => Err(ApiError::invalid_update("unsupported collection field type")),
    }
}

fn scalar_sql_parameter(column: &str, parameter: usize) -> Result<String, ApiError> {
    match column {
        "text_value" | "select_value" => Ok(format!("(${parameter}::jsonb #>> '{{}}')")),
        "number_value" => Ok(format!("(${parameter}::jsonb #>> '{{}}')::numeric")),
        "boolean_value" => Ok(format!("(${parameter}::jsonb #>> '{{}}')::boolean")),
        "date_value" => Ok(format!("(${parameter}::jsonb #>> '{{}}')::timestamptz")),
        "multi_select_value" | "relation_value" => Ok(format!("${parameter}::jsonb")),
        _ => Err(ApiError::Internal),
    }
}

fn query_field(fields: &[CollectionFieldView], field_id: Uuid) -> Result<&CollectionFieldView, ApiError> {
    fields
        .iter()
        .find(|field| field.field_id == field_id && !field.archived && !field_is_restricted(field))
        .ok_or_else(|| ApiError::invalid_update("query references an unknown, archived, or restricted field"))
}

pub async fn query_collection_records(
    state: &AppState,
    access: &policy::AuthorizedFlowObject,
    request: &RecordQueryPayload,
) -> Result<Option<RecordQueryResponse>, ApiError> {
    query_collection_records_inner(state, access, request, false).await
}

#[cfg(test)]
async fn query_collection_records_requiring_typed_index(
    state: &AppState,
    access: &policy::AuthorizedFlowObject,
    request: &RecordQueryPayload,
) -> Result<Option<RecordQueryResponse>, ApiError> {
    query_collection_records_inner(state, access, request, true).await
}

async fn query_collection_records_inner(
    state: &AppState,
    access: &policy::AuthorizedFlowObject,
    request: &RecordQueryPayload,
    require_typed_index: bool,
) -> Result<Option<RecordQueryResponse>, ApiError> {
    if request.limit == 0 || request.limit > 100 {
        return Err(ApiError::invalid_update("record query limit must be 1-100"));
    }
    if let Some(sort) = &request.sort
        && !matches!(sort.direction.as_str(), "asc" | "desc")
    {
        return Err(ApiError::invalid_update(
            "record query sort direction must be asc or desc",
        ));
    }
    let collection = collection_read_row(&state.db, access.object_id()).await?;
    let all_fields = collection_fields(&state.db, access.object_id(), false).await?;
    let fingerprint = query_fingerprint(request)?;
    let offset = request
        .cursor
        .as_deref()
        .map(|raw| decode_query_cursor(state.cfg.jwt_secret.expose(), raw, &fingerprint))
        .transpose()?
        .unwrap_or(0);

    let mut sql = String::from(
        "SELECT rp.record_id, rp.properties, rp.document_seq FROM flow_record_projections rp \
         JOIN flow_objects fo ON fo.id = rp.record_id",
    );
    let mut values: Vec<sea_orm::Value> = vec![access.object_id().into()];
    if let Some(group_id) = request.group {
        let field = query_field(&all_fields, group_id)?;
        let column = typed_column(&field.field_type)?;
        values.push(group_id.into());
        let _ = write!(
            sql,
            " LEFT JOIN flow_record_value_projections groupv ON groupv.record_id = rp.record_id \
             AND groupv.collection_id = $1 AND groupv.field_id = ${}",
            values.len()
        );
        let _ = column;
    }
    if let Some(sort) = &request.sort {
        let field = query_field(&all_fields, sort.field_id)?;
        values.push(sort.field_id.into());
        let _ = write!(
            sql,
            " LEFT JOIN flow_record_value_projections sortv ON sortv.record_id = rp.record_id \
             AND sortv.collection_id = $1 AND sortv.field_id = ${}",
            values.len()
        );
        let _ = typed_column(&field.field_type)?;
    }
    sql.push_str(" WHERE rp.collection_id = $1 AND fo.lifecycle_status = 'active'");
    if let Some(filter) = &request.filter {
        let field = query_field(&all_fields, filter.field_id)?;
        validate_typed_value(&field.field_type, &filter.value)?;
        let column = typed_column(&field.field_type)?;
        let operator = match filter.op.as_str() {
            "eq" => "=",
            "ne" => "<>",
            "lt" if !matches!(column, "multi_select_value" | "relation_value") => "<",
            "lte" if !matches!(column, "multi_select_value" | "relation_value") => "<=",
            "gt" if !matches!(column, "multi_select_value" | "relation_value") => ">",
            "gte" if !matches!(column, "multi_select_value" | "relation_value") => ">=",
            "contains" if matches!(column, "multi_select_value" | "relation_value") => "@>",
            _ => {
                return Err(ApiError::invalid_update(
                    "record query filter operator is not valid for the field type",
                ));
            }
        };
        values.push(filter.field_id.into());
        let field_parameter = values.len();
        values.push(filter.value.clone().into());
        let value_parameter = values.len();
        let typed_parameter = scalar_sql_parameter(column, value_parameter)?;
        let _ = write!(
            sql,
            " AND EXISTS (SELECT 1 FROM flow_record_value_projections filterv \
             WHERE filterv.collection_id = $1 AND filterv.record_id = rp.record_id \
             AND filterv.field_id = ${field_parameter} AND filterv.{column} {operator} {typed_parameter})"
        );
    }
    let mut order = Vec::new();
    if let Some(group_id) = request.group {
        let field = query_field(&all_fields, group_id)?;
        order.push(format!("groupv.{} ASC NULLS LAST", typed_column(&field.field_type)?));
    }
    if let Some(sort) = &request.sort {
        let field = query_field(&all_fields, sort.field_id)?;
        order.push(format!(
            "sortv.{} {} NULLS LAST",
            typed_column(&field.field_type)?,
            sort.direction.to_ascii_uppercase()
        ));
    }
    order.push("rp.record_id ASC".to_string());
    let _ = write!(sql, " ORDER BY {}", order.join(", "));
    values.push(i64::from(offset).into());
    let offset_parameter = values.len();
    values.push(i64::from(QUERY_SCAN_ROWS_MAX).into());
    let limit_parameter = values.len();
    let _ = write!(sql, " OFFSET ${offset_parameter} LIMIT ${limit_parameter}");

    #[cfg(not(test))]
    let _ = require_typed_index;
    #[cfg(test)]
    if require_typed_index {
        let explain = state
            .db
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!("EXPLAIN (FORMAT TEXT) {sql}"),
                values.clone(),
            ))
            .await?;
        let plan = explain
            .iter()
            .map(|row| row.try_get::<String>("", "QUERY PLAN").map_err(|_| ApiError::Internal))
            .collect::<Result<Vec<_>, _>>()?
            .join("\n");
        if !plan.contains("idx_flow_record_values_") {
            return Err(ApiError::invalid_update(format!(
                "typed Collection query did not use a typed value index: {plan}"
            )));
        }
    }
    let candidates =
        RecordCandidateRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .all(&state.db)
            .await?;
    let candidate_ids = candidates.iter().map(|row| row.record_id).collect::<Vec<_>>();
    let Some(visible) =
        policy::authorize_flow_objects(state, access.context(), &candidate_ids, authz::PermissionLevel::View).await?
    else {
        return Ok(None);
    };
    if visible.len() != candidates.len() {
        return Err(ApiError::Internal);
    }

    let requested_fields = if request.field_ids.is_empty() {
        all_fields
            .iter()
            .filter(|field| !field_is_restricted(field))
            .map(|field| field.field_id)
            .collect::<BTreeSet<_>>()
    } else {
        request.field_ids.iter().copied().collect::<BTreeSet<_>>()
    };
    for field_id in &requested_fields {
        let _ = query_field(&all_fields, *field_id)?;
    }

    let mut items = Vec::new();
    let mut last_returned_offset = None;
    let mut has_more = false;
    let mut projection_seq = collection.schema_seq;
    for (index, (candidate, is_visible)) in candidates.iter().zip(visible).enumerate() {
        if !is_visible {
            continue;
        }
        if items.len() == usize::try_from(request.limit).map_err(|_| ApiError::Internal)? {
            has_more = true;
            break;
        }
        let object_row = repository::fetch_object_view(&state.db, candidate.record_id)
            .await?
            .ok_or(ApiError::Internal)?;
        let properties = candidate.properties.as_object().ok_or(ApiError::Internal)?;
        let values_by_field_id = properties
            .iter()
            .filter_map(|(field_id, value)| {
                Uuid::parse_str(field_id)
                    .ok()
                    .filter(|field_id| requested_fields.contains(field_id))
                    .map(|_| (field_id.clone(), value.clone()))
            })
            .collect();
        projection_seq = projection_seq.max(candidate.document_seq);
        let mut record = query::object_view_from_row(object_row);
        if collection.field_secrecy_enabled {
            record.semantic_content = json!({});
        }
        items.push(RecordQueryItem {
            record,
            values_by_field_id,
        });
        let consumed = u32::try_from(index + 1).map_err(|_| ApiError::Internal)?;
        last_returned_offset = Some(offset.saturating_add(consumed));
    }
    if !has_more && candidates.len() == usize::try_from(QUERY_SCAN_ROWS_MAX).map_err(|_| ApiError::Internal)? {
        if items.len() < usize::try_from(request.limit).map_err(|_| ApiError::Internal)? {
            return Err(ApiError::limit_exceeded(
                "record query authorization scan budget exhausted",
                "scan_budget",
                Some(json!(QUERY_SCAN_ROWS_MAX)),
                Some(json!(QUERY_SCAN_ROWS_MAX)),
                None,
            ));
        }
        has_more = true;
    }
    let next_cursor = if has_more {
        last_returned_offset
            .map(|offset| {
                encode_query_cursor(
                    state.cfg.jwt_secret.expose(),
                    &RecordQueryCursor { fingerprint, offset },
                )
            })
            .transpose()?
    } else {
        None
    };
    let response_fields = all_fields
        .into_iter()
        .filter(|field| requested_fields.contains(&field.field_id) && !field_is_restricted(field))
        .collect();
    if !policy::ensure_epoch_current(state, access.context()).await? {
        return Ok(None);
    }
    Ok(Some(RecordQueryResponse {
        fields: response_fields,
        items,
        next_cursor,
        projection_seq,
        schema_seq: collection.schema_seq,
    }))
}

fn record_query_requires_read_endpoint(payload: &Value) -> Result<AcceptedChange, ApiError> {
    let payload: RecordQueryPayload = parse_payload("record_query", payload)?;
    if payload.limit == 0 || payload.limit > 100 {
        return Err(ApiError::invalid_update("record query limit must be 1-100"));
    }
    Err(ApiError::invalid_update(
        "record_query is read-only; use the collection records or query endpoint",
    ))
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmbedFaultPoint {
    Object,
    Document,
    Block,
    Relation,
    Dispatch,
}

#[cfg(test)]
fn inject_embed_fault(selected: Option<EmbedFaultPoint>, point: EmbedFaultPoint) -> Result<(), ApiError> {
    if selected == Some(point) {
        Err(ApiError::Internal)
    } else {
        Ok(())
    }
}

async fn replay_embed(
    state: &AppState,
    workspace_id: Uuid,
    page_id: Uuid,
    page_document_id: Uuid,
    key: &str,
    body: &EmbedIdempotencyBody,
) -> Result<Option<AcceptedChange>, ApiError> {
    let Some(event) = repository::find_idempotent_event(&state.db, workspace_id, key).await? else {
        return Ok(None);
    };
    if event.event_type != "flow.content.accepted" || event.aggregate_id != page_document_id.to_string() {
        return Err(ApiError::Conflict(
            "idempotency_key was already used for a different operation".to_string(),
        ));
    }
    let stored_body: EmbedIdempotencyBody = serde_json::from_value(
        event
            .metadata
            .get("embed_idempotency_body")
            .cloned()
            .ok_or_else(|| ApiError::Conflict("embed replay identity is missing".to_string()))?,
    )
    .map_err(|_| ApiError::Conflict("embed replay identity is invalid".to_string()))?;
    if stored_body != *body {
        return Err(ApiError::Conflict(
            "idempotency_key was already used with a different embed request".to_string(),
        ));
    }
    let result = event
        .metadata
        .get("command_result")
        .cloned()
        .ok_or_else(|| ApiError::Conflict("embed replay result is missing".to_string()))?;
    let row = repository::fetch_object_view(&state.db, page_id)
        .await?
        .ok_or(ApiError::Internal)?;
    let mut change = super::command::accepted_change_from_row(row, event.id);
    change.command_result = Some(result.clone());
    let collection_id = result
        .get("collection_id")
        .and_then(Value::as_str)
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .ok_or(ApiError::Internal)?;
    change.affected_object_ids = vec![page_id, collection_id];
    Ok(Some(change))
}

async fn create_collection_embed(
    state: &AppState,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    page: &repository::ObjectViewRow,
    checked_epoch: i64,
    #[cfg(test)] fault: Option<EmbedFaultPoint>,
) -> Result<AcceptedChange, ApiError> {
    if page.object_type != "page" || page.lifecycle_status == "archived" {
        return Err(ApiError::invalid_update(
            "create_collection_embed target must be an active page",
        ));
    }
    let payload: CreateCollectionEmbedPayload = parse_payload("create_collection_embed", &input.payload)?;
    let title = validate_label(&payload.title, "collection title")?;
    let idempotency_body = EmbedIdempotencyBody {
        title: title.clone(),
        display_settings: payload.display_settings.clone(),
        initial_schema: json!({
            "initial_fields": payload.initial_fields,
            "initial_view": payload.initial_view,
        }),
    };
    if let Some(replay) = replay_embed(
        state,
        workspace_id,
        input.object_id,
        page.document_id,
        &input.idempotency_key,
        &idempotency_body,
    )
    .await?
    {
        return Ok(replay);
    }
    authz::ensure_parent_can_adopt_child(&state.db, workspace_id, input.object_id).await?;
    let collection_id = Uuid::new_v4();
    let collection_document_id = Uuid::new_v4();
    let block_id = Uuid::new_v4();
    let block_node = node_id(block_id);
    let operations = vec![
        Operation::CreateNode {
            id: block_node.clone(),
            parent: None,
            index: u32::MAX,
            kind: NodeKind::Block,
        },
        Operation::SetProperty {
            id: block_node.clone(),
            key: "embed_type".to_string(),
            value: "collection".to_string(),
        },
        Operation::SetProperty {
            id: block_node,
            key: "embed_object_id".to_string(),
            value: collection_id.to_string(),
        },
    ];
    let boot = bootstrap::load(&state.db, page.document_id).await?;
    let bytes = export_operations(&boot, &operations)?;
    let expected_frontier = input
        .expected_frontier
        .as_deref()
        .map(frame::decode_bytes)
        .transpose()
        .map_err(|_| ApiError::invalid_update("expected_frontier is not valid base64"))?;
    let update_id = write::replay_stable_update_id(page.document_id, &input.idempotency_key);
    let collab = runtime::runtime();
    let _permits = collab
        .coordinator
        .acquire_many(&[page.document_id])
        .await
        .map_err(|_| ApiError::server_draining(crate::error::ServerDrainingReason::Contention, 0, "server_draining"))?;

    for attempt in 1..=MAX_REBASE_ATTEMPTS {
        let prepared = match write::hydrate_and_apply(
            &state.db,
            &collab.cache,
            page.document_id,
            update_id,
            &bytes,
            expected_frontier.as_deref(),
        )
        .await?
        {
            write::HydrateOutcome::Prepared(prepared) => prepared,
            write::HydrateOutcome::Rejected(write::AcceptOutcome::Rejected(rejected)) => {
                return Err(map_write_rejection(&rejected));
            }
            write::HydrateOutcome::Rejected(write::AcceptOutcome::Accepted(_)) => {
                return Err(ApiError::Internal);
            }
        };
        let mut collection_engine = LoroCollabEngine::new_empty(rand::random());
        collection_engine.set_title(&title).map_err(|_| ApiError::Internal)?;
        apply_initial_collection_schema(&mut collection_engine, &idempotency_body.initial_schema)?;
        let collection_snapshot = collection_engine.export_snapshot().map_err(|_| ApiError::Internal)?;
        let collection_frontier = collection_engine.frontier().as_bytes().to_vec();
        let collection_semantic = collection_engine.semantic_snapshot().map_err(|_| ApiError::Internal)?;
        let collection_state = projection::state_json(&collection_semantic).map_err(|_| ApiError::Internal)?;
        let tx = state.db.begin().await?;
        write::set_locked_phase_statement_budgets(&tx, 1).await?;
        authz::fence_epoch_for_share(&tx, workspace_id, checked_epoch).await?;
        let request = write::UpdateRequest {
            document_id: page.document_id,
            update_id,
            bytes: bytes.clone(),
            idempotency_key: Some(input.idempotency_key.clone()),
            event_idempotency_key: Some(input.idempotency_key.clone()),
            origin_client_id: Some(input.origin_client_id.clone()),
            message: input.message.clone(),
            actor_id: input.actor_id,
            actor_is_bot: input.actor_is_bot(),
            workspace_id,
            checked_epoch,
            expected_frontier: expected_frontier.clone(),
            origin: input.origin.clone(),
        };
        let staged = write::stage_one_document(
            &tx,
            &request,
            &prepared,
            crate::config::runtime().flow.dispatch_max_attempts,
        )
        .await?;
        let write::StagedOutcome::Ready(staged) = staged else {
            tx.rollback().await?;
            if attempt == MAX_REBASE_ATTEMPTS {
                return Err(ApiError::server_draining(
                    crate::error::ServerDrainingReason::Contention,
                    0,
                    "server_draining",
                ));
            }
            continue;
        };
        #[cfg(test)]
        inject_embed_fault(fault, EmbedFaultPoint::Block)?;
        repository::insert_flow_object(
            &tx,
            &NewFlowObject {
                id: collection_id,
                workspace_id,
                project_id: page.project_id,
                object_type: "collection".to_string(),
                parent_id: Some(input.object_id),
                created_by: actor_user_id(input.actor_id, input.actor_is_bot()),
                governance_metadata: json!({}),
            },
        )
        .await?;
        #[cfg(test)]
        inject_embed_fault(fault, EmbedFaultPoint::Object)?;
        repository::insert_collab_document(
            &tx,
            &NewCollabDocument {
                id: collection_document_id,
                object_id: collection_id,
                format_version: DOCUMENT_FORMAT_VERSION.to_string(),
                snapshot: collection_snapshot,
                frontier: collection_frontier.clone(),
            },
        )
        .await?;
        #[cfg(test)]
        inject_embed_fault(fault, EmbedFaultPoint::Document)?;
        repository::insert_projection(
            &tx,
            &NewProjection {
                object_id: collection_id,
                document_seq: 0,
                document_frontier: collection_frontier,
                title: title.clone(),
                state: collection_state,
                plain_text: String::new(),
            },
        )
        .await?;
        insert_collection_projection(
            &tx,
            collection_id,
            collection_document_id,
            workspace_id,
            page.project_id,
            Value::Object(payload.display_settings.clone()),
        )
        .await?;
        sync_collection_projection(&tx, collection_id, 0, &collection_engine).await?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_relations \
             (id, workspace_id, relation_type, source_object_id, target_object_id, position_key, properties, created_by) \
             VALUES ($1, $2, 'embeds', $3, $4, $5, $6, $7)",
            vec![
                Uuid::new_v4().into(),
                workspace_id.into(),
                input.object_id.into(),
                collection_id.into(),
                block_id.to_string().into(),
                json!({"block_id": block_id}).into(),
                actor_user_id(input.actor_id, input.actor_is_bot()).into(),
            ],
        ))
        .await?;
        #[cfg(test)]
        inject_embed_fault(fault, EmbedFaultPoint::Relation)?;
        let command_result = json!({
            "collection_id": collection_id,
            "collection_document_id": collection_document_id,
            "block_id": block_id,
        });
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE business_events SET metadata = metadata || $2::jsonb, payload = payload || $3::jsonb \
             WHERE id = $1",
            vec![
                staged.event_id.into(),
                json!({
                    "semantic_summary": {"action": "create_collection_embed"},
                    "embed_idempotency_body": idempotency_body,
                    "command_result": command_result,
                })
                .into(),
                json!({
                    "collection_id": collection_id,
                    "block_id": block_id,
                    "relation_type": "embeds",
                })
                .into(),
            ],
        ))
        .await?;
        #[cfg(test)]
        inject_embed_fault(fault, EmbedFaultPoint::Dispatch)?;
        tx.commit().await?;
        let before_frontier = prepared.observed.head_frontier.clone();
        let accepted = write::finish_committed(
            &collab.cache,
            &collab.registry,
            None,
            &request,
            prepared,
            write::Accepted {
                update_id,
                head_seq: staged.new_head_seq,
                head_frontier: staged.after_frontier,
                projection_seq: staged.new_head_seq,
                event_id: staged.event_id,
                before_frontier,
                should_advance_snapshot: false,
            },
            false,
        );
        let row = repository::fetch_object_view(&state.db, input.object_id)
            .await?
            .ok_or(ApiError::Internal)?;
        let mut change = super::command::accepted_change_from_row(row, accepted.event_id);
        change.command_result = Some(command_result);
        change.affected_object_ids = vec![input.object_id, collection_id];
        return Ok(change);
    }
    Err(ApiError::Internal)
}

async fn replay_existing_command(
    state: &AppState,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    kind: CollectionCommandType,
) -> Result<Option<AcceptedChange>, ApiError> {
    let Some(event) = repository::find_idempotent_event(&state.db, workspace_id, &input.idempotency_key).await? else {
        return Ok(None);
    };
    let (expected_type, expected_aggregate, response_id, result) = match kind {
        CollectionCommandType::FieldCreate
        | CollectionCommandType::FieldUpdate
        | CollectionCommandType::FieldArchive
        | CollectionCommandType::FieldReorder => (
            "flow.schema.changed",
            input.object_id.to_string(),
            input.object_id,
            None,
        ),
        CollectionCommandType::ViewCreate | CollectionCommandType::ViewUpdate | CollectionCommandType::ViewReorder => {
            let view_id = input
                .payload
                .get("view_id")
                .and_then(Value::as_str)
                .and_then(|raw| Uuid::parse_str(raw).ok())
                .ok_or_else(|| ApiError::invalid_update("view_id must be a UUID"))?;
            let event_type = match kind {
                CollectionCommandType::ViewCreate => "flow.view.created",
                CollectionCommandType::ViewUpdate => "flow.view.updated",
                CollectionCommandType::ViewReorder => "flow.view.reordered",
                _ => return Err(ApiError::Internal),
            };
            (event_type, view_id.to_string(), input.object_id, None)
        }
        CollectionCommandType::RecordPatch => {
            let payload: RecordPatchPayload = parse_payload("record_patch", &input.payload)?;
            fetch_record_target(&state.db, input.object_id, payload.record_id).await?;
            (
                "flow.record.updated",
                payload.record_id.to_string(),
                payload.record_id,
                Some(json!({"record_id": payload.record_id})),
            )
        }
        CollectionCommandType::RecordArchive => {
            let payload: RecordIdPayload = parse_payload("record_archive", &input.payload)?;
            (
                "flow.record.archived",
                payload.record_id.to_string(),
                payload.record_id,
                Some(json!({"record_id": payload.record_id})),
            )
        }
        CollectionCommandType::RecordCreate
        | CollectionCommandType::RecordQuery
        | CollectionCommandType::CreateCollectionEmbed => return Ok(None),
    };
    if event.event_type != expected_type || event.aggregate_id != expected_aggregate {
        return Err(ApiError::Conflict(
            "idempotency_key was already used for a different operation".to_string(),
        ));
    }
    let row = repository::fetch_object_view(&state.db, response_id)
        .await?
        .ok_or(ApiError::Internal)?;
    let mut change = super::command::accepted_change_from_row(row, event.id);
    change.command_result = result;
    Ok(Some(change))
}

pub async fn execute(
    state: &AppState,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    checked_epoch: i64,
    kind: CollectionCommandType,
    target: &repository::ObjectViewRow,
) -> Result<AcceptedChange, ApiError> {
    runtime::runtime().ensure_workspace_accepting(workspace_id)?;
    if kind == CollectionCommandType::CreateCollectionEmbed {
        #[cfg(test)]
        return create_collection_embed(state, input, workspace_id, target, checked_epoch, None).await;
        #[cfg(not(test))]
        return create_collection_embed(state, input, workspace_id, target, checked_epoch).await;
    }
    let collection = ensure_collection_target(state, input.object_id).await?;
    if let Some(replay) = replay_existing_command(state, input, workspace_id, kind).await? {
        return Ok(replay);
    }
    match kind {
        CollectionCommandType::FieldCreate
        | CollectionCommandType::FieldUpdate
        | CollectionCommandType::FieldArchive
        | CollectionCommandType::FieldReorder
        | CollectionCommandType::ViewCreate
        | CollectionCommandType::ViewUpdate
        | CollectionCommandType::ViewReorder => {
            let operations = collection_operations(state, collection.document_id, kind, &input.payload).await?;
            execute_document_update(
                state,
                input,
                workspace_id,
                collection.document_id,
                checked_epoch,
                &operations,
                input.object_id,
                ProjectionSync::Collection {
                    collection_id: input.object_id,
                },
            )
            .await
        }
        CollectionCommandType::RecordCreate => {
            create_record(state, input, workspace_id, &collection, checked_epoch).await
        }
        CollectionCommandType::RecordPatch => record_patch(state, input, workspace_id, checked_epoch).await,
        CollectionCommandType::RecordArchive => archive_record(state, input, workspace_id, checked_epoch).await,
        CollectionCommandType::RecordQuery => record_query_requires_read_endpoint(&input.payload),
        CollectionCommandType::CreateCollectionEmbed => Err(ApiError::Internal),
    }
}

#[cfg(test)]
pub(crate) async fn execute_embed_with_fault(
    state: &AppState,
    input: &ExecuteCommandInput,
    workspace_id: Uuid,
    page: &repository::ObjectViewRow,
    checked_epoch: i64,
    fault: EmbedFaultPoint,
) -> Result<AcceptedChange, ApiError> {
    create_collection_embed(state, input, workspace_id, page, checked_epoch, Some(fault)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_collection_cardinality_registry_is_complete_and_exact() {
        let registry = v0_6_command_cardinality_registry();
        assert_eq!(registry.len(), 12);
        assert_eq!(
            registry,
            vec![
                ("field_create", ExistingDocumentCardinality::One),
                ("field_update", ExistingDocumentCardinality::One),
                ("field_archive", ExistingDocumentCardinality::One),
                ("field_reorder", ExistingDocumentCardinality::One),
                ("view_create", ExistingDocumentCardinality::One),
                ("view_update", ExistingDocumentCardinality::One),
                ("view_reorder", ExistingDocumentCardinality::One),
                ("record_create", ExistingDocumentCardinality::Zero),
                ("record_patch", ExistingDocumentCardinality::One),
                ("record_archive", ExistingDocumentCardinality::Zero),
                ("record_query", ExistingDocumentCardinality::Zero),
                ("create_collection_embed", ExistingDocumentCardinality::One),
            ]
        );
        assert!(
            registry
                .iter()
                .all(|(_, cardinality)| !matches!(cardinality, ExistingDocumentCardinality::BoundedMany(_))),
            "this package ships no cross-record batch command"
        );
    }

    fn normalized_non_comment_source(path: &std::path::Path) -> String {
        let source = std::fs::read_to_string(path).expect("source file reads");
        source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase()
    }

    fn rust_files_below(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut pending = vec![root.to_path_buf()];
        let mut files = Vec::new();
        while let Some(path) = pending.pop() {
            if path.is_dir() {
                for entry in std::fs::read_dir(path).expect("source directory reads") {
                    pending.push(entry.expect("source directory entry reads").path());
                }
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
        files
    }

    fn sql_identifier_matches_table(identifier: &str, table: &str) -> bool {
        identifier == table
            || identifier
                .strip_suffix(table)
                .is_some_and(|prefix| prefix.ends_with('.'))
    }

    fn source_mutates_table(source: &str, table: &str) -> bool {
        let tokens = source
            .split(|character: char| !(character.is_ascii_alphanumeric() || matches!(character, '_' | '.')))
            .filter(|token| !token.is_empty())
            .collect::<Vec<_>>();
        tokens.windows(2).any(|pair| {
            matches!(pair, [verb, identifier]
                if matches!(*verb, "copy" | "update" | "truncate")
                    && sql_identifier_matches_table(identifier, table))
        }) || tokens.windows(3).any(|triple| {
            matches!(triple, [verb, modifier, identifier]
                if matches!(
                    (*verb, *modifier),
                    ("insert" | "merge", "into")
                        | ("delete", "from")
                        | ("update", "only")
                        | ("truncate", "table" | "only")
                ) && sql_identifier_matches_table(identifier, table))
        }) || tokens.windows(4).any(|quad| {
            matches!(quad, [verb, preposition, modifier, identifier]
                if matches!(
                    (*verb, *preposition, *modifier),
                    ("delete", "from", "only") | ("truncate", "table", "only")
                ) && sql_identifier_matches_table(identifier, table))
        })
    }

    #[test]
    fn flow_collection_forms_tables_untouched_scans_executable_sql_paths() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace = manifest.join("../..");
        let forbidden_forms = ["project_forms", "form_records", "form_views", "form_record_field_index"];
        let allowed_forms_writers = [
            "apps/api/src/forms/projections.rs",
            "apps/api/src/routes/form.rs",
            "apps/api/src/routes/project.rs",
        ];
        let mut production_files = Vec::new();
        for root in ["apps/api/src", "apps/worker/src", "apps/mcp-server/src", "crates"] {
            production_files.extend(rust_files_below(&workspace.join(root)));
        }
        assert!(
            !production_files.is_empty(),
            "production SQL source scan must be non-empty"
        );
        let mut forms_sources = Vec::new();
        for path in production_files {
            let relative = path
                .strip_prefix(&workspace)
                .expect("production source stays below workspace")
                .to_string_lossy()
                .replace('\\', "/");
            let source = normalized_non_comment_source(&path);
            let normalized_sql = source.replace('"', "");
            let mutates_forms = forbidden_forms
                .iter()
                .any(|table| source_mutates_table(&normalized_sql, table));
            if mutates_forms {
                assert!(
                    allowed_forms_writers.contains(&relative.as_str()),
                    "production source outside the reviewed Forms owners mutates a Forms table: {relative}"
                );
            }
            if allowed_forms_writers.contains(&relative.as_str()) {
                forms_sources.push(source);
            }
        }
        assert!(
            !forms_sources.is_empty(),
            "reviewed Forms source scan must be non-empty"
        );
        let forms_sources = forms_sources.join(" ");
        for table in [
            "flow_objects",
            "collab_documents",
            "flow_collection_projections",
            "flow_field_projections",
            "flow_view_projections",
            "flow_record_projections",
            "flow_record_value_projections",
        ] {
            assert!(
                !forms_sources.contains(&format!(" from {table}"))
                    && !forms_sources.contains(&format!(" join {table}")),
                "Forms executable SQL must not read Flow canonical table {table}"
            );
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::indexing_slicing,
    clippy::items_after_statements,
    clippy::too_many_lines
)]
mod database_tests {
    use std::collections::BTreeSet;
    use std::time::Instant;

    use axum::body::to_bytes;
    use axum::{Extension, Router, routing::post};
    use collab_core::{CollabEngine, NodeKind, Operation};
    use platform::{
        app::AppState,
        auth::{JwtClaims, TokenType},
        config::{AppConfig, Secret},
    };
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::{
        CollectionCommandType, CountRow, EmbedFaultPoint, RecordFilter, RecordQueryPayload, RecordSort,
        collection_operations, describe_collection, engine_at_head, execute_embed_with_fault, node_id,
        query_collection_records, query_collection_records_requiring_typed_index, rebuild_typed_projections,
    };
    use crate::error::ApiError;
    use crate::flow::collab::bootstrap;
    use crate::flow::command::{
        CreateObjectInput, ExecuteCommandInput, create_object, create_object_with_collection_schema, execute_command,
    };
    use crate::flow::event_origin::{CommandOrigin, EventSurface};
    use crate::flow::repository;
    use crate::flow::{
        collab::authz::{self, PermissionLevel},
        policy,
    };
    use crate::routes::flow::post_flow_object_command;

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
        let name = format!("openpr_flow_collection_{label}");
        admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
            .await
            .expect("old scratch database drops");
        admin
            .execute_unprepared(&format!("CREATE DATABASE \"{name}\""))
            .await
            .expect("scratch database creates");
        let (prefix, _) = admin_url.rsplit_once('/')?;
        let db = Database::connect(format!("{prefix}/{name}"))
            .await
            .expect("scratch database connects");
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
        let mut files = std::fs::read_dir(dir)
            .expect("migrations directory reads")
            .map(|entry| entry.expect("migration entry reads").path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
            .collect::<Vec<_>>();
        files.sort();
        for path in files {
            let sql = std::fs::read_to_string(&path).expect("migration reads");
            db.execute_unprepared(&sql)
                .await
                .unwrap_or_else(|err| panic!("applying {} failed: {err}", path.display()));
        }
        Some(Scratch { db, name, admin_url })
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
                app_name: "flow-collection-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("flow-collection-test-secret"),
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

    async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
        db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .await
            .unwrap_or_else(|err| panic!("fixture SQL failed: {err}"));
    }

    async fn seed(state: &AppState) -> (Uuid, Uuid) {
        let workspace_id = Uuid::new_v4();
        let owner_id = Uuid::new_v4();
        exec(
            &state.db,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'owner', 'user', true)",
            vec![owner_id.into(), format!("{owner_id}@collection.test").into()],
        )
        .await;
        exec(
            &state.db,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'collections', $3)",
            vec![
                workspace_id.into(),
                format!("collection-{workspace_id}").into(),
                owner_id.into(),
            ],
        )
        .await;
        exec(
            &state.db,
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
            vec![workspace_id.into(), owner_id.into()],
        )
        .await;
        exec(
            &state.db,
            "INSERT INTO flow_workspace_settings (workspace_id, flow_enabled, default_member_level) \
             VALUES ($1, true, 'edit')",
            vec![workspace_id.into()],
        )
        .await;
        (workspace_id, owner_id)
    }

    async fn create_object_for(
        state: &AppState,
        workspace_id: Uuid,
        owner_id: Uuid,
        object_type: &str,
        title: &str,
    ) -> Uuid {
        create_object(
            state,
            CreateObjectInput {
                workspace_id,
                actor_id: owner_id,
                actor_is_bot: false,
                object_type: object_type.to_string(),
                project_id: None,
                parent_object_id: None,
                title: title.to_string(),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            },
        )
        .await
        .expect("object creates")
        .object
        .id
    }

    async fn command(
        state: &AppState,
        owner_id: Uuid,
        object_id: Uuid,
        kind: &str,
        payload: Value,
        key: String,
    ) -> Result<crate::flow::model::AcceptedChange, crate::error::ApiError> {
        execute_command(
            state,
            ExecuteCommandInput {
                object_id,
                actor_id: owner_id,
                principal_kind: "user".to_string(),
                role: "owner".to_string(),
                command_type: kind.to_string(),
                payload,
                expected_frontier: None,
                idempotency_key: key,
                message: None,
                origin_client_id: format!("test:{owner_id}"),
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            },
        )
        .await
    }

    async fn read_access(
        state: &AppState,
        workspace_id: Uuid,
        owner_id: Uuid,
        object_id: Uuid,
    ) -> policy::AuthorizedFlowObject {
        let claims = JwtClaims {
            sub: owner_id.to_string(),
            email: format!("{owner_id}@collection.test"),
            token_type: TokenType::Access,
            iat: 0,
            exp: usize::MAX,
        };
        let mut extensions = axum::http::Extensions::new();
        extensions.insert(claims);
        policy::require_flow_object_access(state, &extensions, workspace_id, object_id, PermissionLevel::View)
            .await
            .expect("collection authorization runs")
            .expect("authorization epoch is stable")
    }

    #[tokio::test]
    async fn flow_collection_commands_keep_stable_schema_ids_and_independent_record_documents() {
        let scratch = scratch_or_skip!("commands");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed(&state).await;
        let collection_id = create_object_for(&state, workspace_id, owner_id, "collection", "Issues").await;
        let root = repository::fetch_workspace_navigator_root(&state.db, workspace_id)
            .await
            .expect("root query runs")
            .expect("root exists");
        let collection = repository::fetch_object_view(&state.db, collection_id)
            .await
            .expect("collection query runs")
            .expect("collection exists");
        assert_eq!(collection.parent_id, Some(root));

        let field_types = [
            ("text", json!("hello")),
            ("number", json!(3.5)),
            ("boolean", json!(true)),
            ("date", json!("2026-09-11T12:00:00Z")),
            ("select", json!("todo")),
            ("multi_select", json!(["a", "b"])),
            ("relation", json!([collection_id.to_string()])),
        ];
        let mut properties = serde_json::Map::new();
        let mut field_ids = Vec::new();
        for (index, (field_type, value)) in field_types.into_iter().enumerate() {
            let field_id = Uuid::new_v4();
            field_ids.push(field_id);
            properties.insert(field_id.to_string(), value);
            command(
                &state,
                owner_id,
                collection_id,
                "field_create",
                json!({
                    "field_id": field_id,
                    "label": format!("Field {index}"),
                    "field_type": field_type,
                    "index": index,
                }),
                Uuid::new_v4().to_string(),
            )
            .await
            .expect("field creates");
        }
        command(
            &state,
            owner_id,
            collection_id,
            "field_update",
            json!({"field_id": field_ids[0], "label": "Renamed"}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("field label updates");
        #[derive(FromQueryResult)]
        struct LabelRow {
            field_id: Uuid,
            label: String,
        }
        let label = LabelRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT field_id, label FROM flow_field_projections WHERE collection_id = $1 AND field_id = $2",
            vec![collection_id.into(), field_ids[0].into()],
        ))
        .one(&state.db)
        .await
        .expect("field projection query runs")
        .expect("field projection exists");
        assert_eq!(label.field_id, field_ids[0]);
        assert_eq!(label.label, "Renamed");

        for view_type in ["table", "board"] {
            command(
                &state,
                owner_id,
                collection_id,
                "view_create",
                json!({"view_id": Uuid::new_v4(), "name": view_type, "view_type": view_type}),
                Uuid::new_v4().to_string(),
            )
            .await
            .expect("view creates");
        }
        let mut records = Vec::new();
        for _ in 0..2 {
            let created = command(
                &state,
                owner_id,
                collection_id,
                "record_create",
                json!({"properties": properties, "body": "record body"}),
                Uuid::new_v4().to_string(),
            )
            .await
            .expect("record creates");
            records.push(created.object);
        }
        assert_ne!(records[0].id, records[1].id);
        assert_ne!(records[0].document_id, records[1].document_id);
        assert_ne!(records[0].document_id, collection.document_id);
        assert_ne!(records[1].document_id, collection.document_id);

        command(
            &state,
            owner_id,
            collection_id,
            "record_patch",
            json!({"record_id": records[0].id, "properties": {field_ids[1].to_string(): 42}}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("record patch commits");
        #[derive(FromQueryResult)]
        struct TypedRow {
            number_value: String,
            document_seq: i64,
        }
        let typed = TypedRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT number_value::text AS number_value, document_seq FROM flow_record_value_projections \
             WHERE record_id = $1 AND field_id = $2",
            vec![records[0].id.into(), field_ids[1].into()],
        ))
        .one(&state.db)
        .await
        .expect("typed projection query runs")
        .expect("typed projection exists");
        assert_eq!(typed.number_value, "42");
        assert_eq!(typed.document_seq, 1);

        let (other_workspace_id, other_owner_id) = seed(&state).await;
        let outside_page =
            create_object_for(&state, other_workspace_id, other_owner_id, "page", "Outside workspace").await;
        let cross_workspace = command(
            &state,
            owner_id,
            collection_id,
            "record_patch",
            json!({
                "record_id": records[0].id,
                "properties": {field_ids[6].to_string(): [outside_page.to_string()]}
            }),
            Uuid::new_v4().to_string(),
        )
        .await;
        assert!(
            cross_workspace.is_err(),
            "typed relation values must reject objects outside the Collection workspace"
        );
        command(
            &state,
            owner_id,
            collection_id,
            "field_archive",
            json!({"field_id": field_ids[0]}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("field archives");
        #[derive(FromQueryResult)]
        struct Count {
            count: i64,
        }
        let archived_values = Count::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS count FROM flow_record_value_projections \
             WHERE collection_id = $1 AND field_id = $2",
            vec![collection_id.into(), field_ids[0].into()],
        ))
        .one(&state.db)
        .await
        .expect("archived typed values query runs")
        .expect("archived typed values count exists");
        assert_eq!(archived_values.count, 0);
        command(
            &state,
            owner_id,
            collection_id,
            "record_patch",
            json!({"record_id": records[0].id, "properties": {field_ids[1].to_string(): 43}}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("record remains patchable after a field is archived");

        let generic_record = create_object(
            &state,
            CreateObjectInput {
                workspace_id,
                actor_id: owner_id,
                actor_is_bot: false,
                object_type: "record".to_string(),
                project_id: None,
                parent_object_id: Some(collection_id),
                title: "forbidden".to_string(),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            },
        )
        .await;
        assert!(generic_record.is_err(), "generic object create must reject Record");
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn collection_and_record_writes_require_typed_server_paths_and_corrupt_field_can_be_archived() {
        let scratch = scratch_or_skip!("typed_only");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed(&state).await;
        let collection_id = create_object_for(&state, workspace_id, owner_id, "collection", "Guarded").await;
        let collection = repository::fetch_object_view(&state.db, collection_id)
            .await
            .expect("collection lookup runs")
            .expect("collection exists");
        let bad_field_id = Uuid::new_v4();
        let semantic_patch = json!({"operations": [
            {"op": "create_node", "id": bad_field_id, "parent": null, "index": 0, "kind": "collection_field"},
            {"op": "set_property", "id": bad_field_id, "key": "field_type", "value": "formula"},
            {"op": "set_property", "id": bad_field_id, "key": "label", "value": "Forbidden formula"}
        ]});

        let generic = command(
            &state,
            owner_id,
            collection_id,
            "semantic_patch",
            semantic_patch,
            Uuid::new_v4().to_string(),
        )
        .await;
        assert!(
            matches!(generic, Err(ApiError::Typed { ref message, .. }) if message.contains("typed collection commands")),
            "generic content path must reject Collection documents: {generic:?}"
        );
        let formula = command(
            &state,
            owner_id,
            collection_id,
            "field_create",
            json!({"field_id": bad_field_id, "label": "Formula", "field_type": "formula"}),
            Uuid::new_v4().to_string(),
        )
        .await;
        assert!(
            matches!(formula, Err(ApiError::Typed { ref message, .. }) if message.contains("unsupported collection field type")),
            "typed write must reject deferred field types: {formula:?}"
        );
        let formula_operations = collection_operations(
            &state,
            collection.document_id,
            CollectionCommandType::FieldCreate,
            &json!({"field_id": Uuid::new_v4(), "label": "Formula", "field_type": "formula"}),
        )
        .await;
        assert!(
            matches!(formula_operations, Err(ApiError::Typed { ref message, .. }) if message.contains("unsupported collection field type")),
            "write-time validator must reject before projection synchronization: {formula_operations:?}"
        );
        let ticket = crate::flow::collab::ticket::issue(
            &state.db,
            crate::flow::collab::ticket::IssueTicketInput {
                user_id: owner_id,
                workspace_id,
                document_id: collection.document_id,
                client_id: "collection-ticket-probe".to_string(),
                origin: "https://flow.test".to_string(),
            },
            &["https://flow.test".to_string()],
        )
        .await;
        assert!(matches!(ticket, Err(ApiError::Forbidden(message)) if message.contains("server-only typed commands")));

        let boot = bootstrap::load(&state.db, collection.document_id)
            .await
            .expect("collection bootstrap loads");
        let mut corrupt = engine_at_head(&boot).expect("collection engine loads");
        let bad_node = node_id(bad_field_id);
        for operation in [
            Operation::CreateNode {
                id: bad_node.clone(),
                parent: None,
                index: 0,
                kind: NodeKind::CollectionField,
            },
            Operation::SetProperty {
                id: bad_node.clone(),
                key: "field_type".to_string(),
                value: "formula".to_string(),
            },
            Operation::SetProperty {
                id: bad_node,
                key: "label".to_string(),
                value: "Legacy corrupt field".to_string(),
            },
        ] {
            corrupt
                .apply_operation(&operation)
                .expect("legacy corruption fixture applies");
        }
        let snapshot = corrupt.export_snapshot().expect("legacy corrupt snapshot exports");
        let frontier = corrupt.frontier().as_bytes().to_vec();
        exec(
            &state.db,
            "UPDATE collab_documents SET snapshot = $2, snapshot_frontier = $3, head_frontier = $3 WHERE id = $1",
            vec![collection.document_id.into(), snapshot.into(), frontier.into()],
        )
        .await;

        command(
            &state,
            owner_id,
            collection_id,
            "field_archive",
            json!({"field_id": bad_field_id}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("an unsupported legacy field remains archivable");
        let valid_field_id = Uuid::new_v4();
        command(
            &state,
            owner_id,
            collection_id,
            "field_create",
            json!({"field_id": valid_field_id, "label": "Recovered", "field_type": "text"}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("collection accepts typed writes after rescue");

        let record = command(
            &state,
            owner_id,
            collection_id,
            "record_create",
            json!({"properties": {valid_field_id.to_string(): "value"}}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("record creates");
        let record_generic = command(
            &state,
            owner_id,
            record.object.id,
            "semantic_patch",
            json!({"operations": []}),
            Uuid::new_v4().to_string(),
        )
        .await;
        assert!(
            matches!(record_generic, Err(ApiError::Typed { ref message, .. }) if message.contains("typed collection commands")),
            "generic content path must reject Record documents: {record_generic:?}"
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn collection_commands_emit_the_frozen_v0_6_semantic_event_family() {
        #[derive(FromQueryResult)]
        struct EventRow {
            event_type: String,
            aggregate_type: String,
            aggregate_id: String,
            payload: Value,
            dispatch_event_type: String,
            document_id: Option<Uuid>,
            accepted_seq: Option<i64>,
        }
        async fn event(db: &DatabaseConnection, id: Uuid) -> EventRow {
            EventRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT b.event_type, b.aggregate_type, b.aggregate_id, b.payload, \
                        d.event_type AS dispatch_event_type, d.document_id, d.accepted_seq \
                 FROM business_events b JOIN event_dispatch d ON d.event_id = b.id WHERE b.id = $1",
                vec![id.into()],
            ))
            .one(db)
            .await
            .expect("semantic event query runs")
            .expect("semantic event exists")
        }

        let scratch = scratch_or_skip!("semantic_events");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed(&state).await;
        let collection_id = create_object_for(&state, workspace_id, owner_id, "collection", "Events").await;
        let field_id = Uuid::new_v4();
        let field_create_key = Uuid::new_v4().to_string();
        let mut field_events = Vec::new();
        let mut field_event_ids = Vec::new();
        for (kind, payload, key) in [
            (
                "field_create",
                json!({"field_id": field_id, "label": "Name", "field_type": "text"}),
                field_create_key.clone(),
            ),
            (
                "field_update",
                json!({"field_id": field_id, "label": "Renamed"}),
                Uuid::new_v4().to_string(),
            ),
            (
                "field_reorder",
                json!({"field_id": field_id, "index": 0}),
                Uuid::new_v4().to_string(),
            ),
            (
                "field_archive",
                json!({"field_id": field_id}),
                Uuid::new_v4().to_string(),
            ),
        ] {
            let changed = command(&state, owner_id, collection_id, kind, payload, key)
                .await
                .expect("field command succeeds");
            field_event_ids.push(changed.event_id);
            field_events.push(event(&state.db, changed.event_id).await);
        }
        assert_eq!(field_events.len(), 4);
        for (row, change_kind) in field_events.iter().zip(["create", "update", "reorder", "archive"]) {
            assert_eq!(row.event_type, "flow.schema.changed");
            assert_eq!(row.aggregate_type, "flow_collection");
            assert_eq!(row.aggregate_id, collection_id.to_string());
            assert_eq!(row.payload["field_id"], json!(field_id));
            assert_eq!(row.payload["change_kind"], json!(change_kind));
            assert!(row.payload["schema_seq"].as_i64().is_some());
            assert_eq!(row.dispatch_event_type, row.event_type);
            assert_eq!(row.document_id, None);
            assert_eq!(row.accepted_seq, None);
        }
        let replay = command(
            &state,
            owner_id,
            collection_id,
            "field_create",
            json!({"field_id": field_id, "label": "Name", "field_type": "text"}),
            field_create_key,
        )
        .await
        .expect("semantic command replay succeeds");
        assert_eq!(replay.event_id, field_event_ids[0]);

        let view_id = Uuid::new_v4();
        let view_create = command(
            &state,
            owner_id,
            collection_id,
            "view_create",
            json!({"view_id": view_id, "name": "Main", "view_type": "table"}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("view creates");
        let second_view_id = Uuid::new_v4();
        let second_view = command(
            &state,
            owner_id,
            collection_id,
            "view_create",
            json!({"view_id": second_view_id, "name": "Second", "view_type": "board", "index": 1}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("second view creates");
        let view_update = command(
            &state,
            owner_id,
            collection_id,
            "view_update",
            json!({"view_id": view_id, "name": "Renamed"}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("view updates");
        let view_reorder = command(
            &state,
            owner_id,
            collection_id,
            "view_reorder",
            json!({"view_id": second_view_id, "index": 0}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("view reorders");
        for (id, expected_type) in [
            (view_create.event_id, "flow.view.created"),
            (second_view.event_id, "flow.view.created"),
            (view_update.event_id, "flow.view.updated"),
            (view_reorder.event_id, "flow.view.reordered"),
        ] {
            let row = event(&state.db, id).await;
            assert_eq!(row.event_type, expected_type);
            assert_eq!(row.aggregate_type, "flow_view");
            assert!(
                matches!(row.payload["view_id"].as_str(), Some(raw) if raw == view_id.to_string() || raw == second_view_id.to_string())
            );
            assert_eq!(row.payload["view_id"].as_str(), Some(row.aggregate_id.as_str()));
            assert!(row.payload["schema_seq"].as_i64().is_some());
            assert_eq!(row.dispatch_event_type, row.event_type);
            assert_eq!((row.document_id, row.accepted_seq), (None, None));
        }

        let active_field_id = Uuid::new_v4();
        command(
            &state,
            owner_id,
            collection_id,
            "field_create",
            json!({"field_id": active_field_id, "label": "Active", "field_type": "text"}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("active field creates");
        let created = command(
            &state,
            owner_id,
            collection_id,
            "record_create",
            json!({"properties": {active_field_id.to_string(): "secret"}}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("record creates");
        let patched = command(
            &state,
            owner_id,
            collection_id,
            "record_patch",
            json!({"record_id": created.object.id, "properties": {active_field_id.to_string(): "new secret"}}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("record patches");
        let archived = command(
            &state,
            owner_id,
            collection_id,
            "record_archive",
            json!({"record_id": created.object.id}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("record archives");
        for (id, expected_type) in [
            (created.event_id, "flow.record.created"),
            (patched.event_id, "flow.record.updated"),
            (archived.event_id, "flow.record.archived"),
        ] {
            let row = event(&state.db, id).await;
            assert_eq!(row.event_type, expected_type);
            assert_eq!(row.aggregate_type, "flow_record");
            assert_eq!(row.aggregate_id, created.object.id.to_string());
            assert_eq!(row.dispatch_event_type, row.event_type);
            assert_eq!((row.document_id, row.accepted_seq), (None, None));
            let encoded = serde_json::to_string(&row.payload).expect("event payload serializes");
            assert!(
                !encoded.contains("secret"),
                "semantic event payload leaked record values"
            );
        }
        scratch.drop_self().await;
    }

    #[derive(FromQueryResult, Debug, PartialEq, Eq)]
    struct OrphanCounts {
        collections: i64,
        page_blocks: i64,
        page_updates: i64,
        embeds: i64,
        dispatches: i64,
    }

    async fn orphan_counts(db: &DatabaseConnection, page_document_id: Uuid) -> OrphanCounts {
        OrphanCounts::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT \
               (SELECT count(*) FROM flow_objects WHERE object_type = 'collection') AS collections, \
               (SELECT count(*) FROM flow_object_projections p, \
                  LATERAL jsonb_each(p.state->'nodes') node \
                 WHERE p.object_id = (SELECT object_id FROM collab_documents WHERE id = $1) \
                   AND node.value->>'deleted' = 'false') AS page_blocks, \
               (SELECT count(*) FROM collab_updates WHERE document_id = $1) AS page_updates, \
               (SELECT count(*) FROM flow_relations WHERE relation_type = 'embeds') AS embeds, \
               (SELECT count(*) FROM event_dispatch) AS dispatches",
            vec![page_document_id.into()],
        ))
        .one(db)
        .await
        .expect("orphan counts query runs")
        .expect("orphan counts returns one row")
    }

    #[tokio::test]
    async fn flow_collection_standalone_create_with_schema_is_idempotent() {
        let scratch = scratch_or_skip!("standalone_replay");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed(&state).await;
        let key = Uuid::new_v4().to_string();
        let field_id = Uuid::new_v4();
        let view_id = Uuid::new_v4();
        let schema = json!({
            "initial_fields": [{
                "field_id": field_id,
                "label": "Priority",
                "field_type": "select",
                "config": {"options": ["high", "low"]},
                "index": 0
            }],
            "initial_view": {
                "view_id": view_id,
                "name": "All records",
                "view_type": "table",
                "index": 0
            }
        });
        let make_input = || CreateObjectInput {
            workspace_id,
            actor_id: owner_id,
            actor_is_bot: false,
            object_type: "collection".to_string(),
            project_id: None,
            parent_object_id: None,
            title: "Standalone schema".to_string(),
            idempotency_key: key.clone(),
            message: None,
            origin: CommandOrigin::first_request_from(EventSurface::Rest),
        };

        let first = create_object_with_collection_schema(&state, make_input(), schema.clone())
            .await
            .expect("first standalone Collection create succeeds");
        let replay = create_object_with_collection_schema(&state, make_input(), schema)
            .await
            .expect("identical standalone Collection create replays");
        assert_eq!(first.object.id, replay.object.id);
        assert_eq!(first.object.document_id, replay.object.document_id);
        assert_eq!(first.event_id, replay.event_id);
        assert_eq!(first.projection_seq, replay.projection_seq);
        let access = read_access(&state, workspace_id, owner_id, first.object.id).await;
        let described = describe_collection(&state, &access, None)
            .await
            .expect("Collection projection describes")
            .expect("authorization epoch remains stable");
        assert_eq!(described.fields.len(), 1);
        assert_eq!(described.fields[0].field_id, field_id);
        assert_eq!(described.views.len(), 1);
        assert_eq!(described.views[0].view_id, view_id);
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_collection_atomic_create_two_real_requests_same_key_return_same_ids() {
        use tower::ServiceExt as _;

        let scratch = scratch_or_skip!("http_replay");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed(&state).await;
        let project_id = Uuid::new_v4();
        exec(
            &state.db,
            "INSERT INTO projects (id, workspace_id, key, name, created_by) \
             VALUES ($1, $2, $3, 'Collection project', $4)",
            vec![
                project_id.into(),
                workspace_id.into(),
                format!("COLL-{}", &project_id.simple().to_string()[..6]).into(),
                owner_id.into(),
            ],
        )
        .await;
        let standalone = create_object(
            &state,
            CreateObjectInput {
                workspace_id,
                actor_id: owner_id,
                actor_is_bot: false,
                object_type: "collection".to_string(),
                project_id: Some(project_id),
                parent_object_id: None,
                title: "Standalone project collection".to_string(),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            },
        )
        .await
        .expect("standalone project collection creates")
        .object;
        let project_root = repository::fetch_navigator_root(&state.db, workspace_id, Some(project_id))
            .await
            .expect("project root query runs")
            .expect("project root exists");
        assert_eq!(standalone.parent_id, Some(project_root));
        assert_eq!(standalone.project_id, Some(project_id));
        let page_id = create_object(
            &state,
            CreateObjectInput {
                workspace_id,
                actor_id: owner_id,
                actor_is_bot: false,
                object_type: "page".to_string(),
                project_id: Some(project_id),
                parent_object_id: None,
                title: "HTTP embed host".to_string(),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            },
        )
        .await
        .expect("project page creates")
        .object
        .id;
        let before = OrphanCounts::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT \
               (SELECT count(*) FROM flow_objects WHERE object_type = 'collection') AS collections, \
               (SELECT count(*) FROM flow_object_projections p, \
                  LATERAL jsonb_each(p.state->'nodes') node \
                 WHERE p.object_id = $1 AND node.value->>'deleted' = 'false') AS page_blocks, \
               (SELECT count(*) FROM collab_updates) AS page_updates, \
               (SELECT count(*) FROM flow_relations WHERE relation_type = 'embeds') AS embeds, \
               (SELECT count(*) FROM event_dispatch) AS dispatches",
            vec![page_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("baseline counts query runs")
        .expect("baseline counts exists");
        let claims = JwtClaims {
            sub: owner_id.to_string(),
            email: format!("{owner_id}@collection.test"),
            token_type: TokenType::Access,
            iat: 0,
            exp: usize::MAX,
        };
        let app = Router::new()
            .route(
                "/api/v1/flow/objects/{object_id}/commands",
                post(post_flow_object_command),
            )
            .layer(Extension(claims))
            .with_state(state.clone());
        let key = Uuid::new_v4().to_string();
        let body = json!({
            "command": {"type": "create_collection_embed", "payload": {"title": "HTTP collection"}},
            "idempotency_key": key,
        })
        .to_string();
        let mut responses = Vec::new();
        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method(axum::http::Method::POST)
                        .uri(format!("/api/v1/flow/objects/{page_id}/commands"))
                        .header(axum::http::header::CONTENT_TYPE, "application/json")
                        .body(axum::body::Body::from(body.clone()))
                        .expect("request builds"),
                )
                .await
                .expect("router responds");
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            let bytes = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("response reads");
            let json: Value = serde_json::from_slice(&bytes).expect("response is JSON");
            assert_eq!(json["code"], 0, "request must succeed: {json}");
            responses.push(json);
        }
        assert_eq!(
            responses[0]["data"]["command_result"], responses[1]["data"]["command_result"],
            "same key must return identical server-generated collection/document/block ids"
        );
        let after = OrphanCounts::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT \
               (SELECT count(*) FROM flow_objects WHERE object_type = 'collection') AS collections, \
               (SELECT count(*) FROM flow_object_projections p, \
                  LATERAL jsonb_each(p.state->'nodes') node \
                 WHERE p.object_id = $1 AND node.value->>'deleted' = 'false') AS page_blocks, \
               (SELECT count(*) FROM collab_updates) AS page_updates, \
               (SELECT count(*) FROM flow_relations WHERE relation_type = 'embeds') AS embeds, \
               (SELECT count(*) FROM event_dispatch) AS dispatches",
            vec![page_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("final counts query runs")
        .expect("final counts exists");
        assert_eq!(after.collections - before.collections, 1);
        assert_eq!(after.page_blocks - before.page_blocks, 1);
        assert_eq!(after.page_updates - before.page_updates, 1);
        assert_eq!(after.embeds - before.embeds, 1);
        assert_eq!(after.dispatches - before.dispatches, 1);
        let result = &responses[0]["data"]["command_result"];
        let collection_id = Uuid::parse_str(result["collection_id"].as_str().expect("collection id string"))
            .expect("collection id UUID");
        let collection = repository::fetch_object_view(&state.db, collection_id)
            .await
            .expect("collection query runs")
            .expect("collection exists");
        assert_eq!(collection.parent_id, Some(page_id));
        assert_eq!(collection.project_id, Some(project_id));

        let member_id = Uuid::new_v4();
        exec(
            &state.db,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'member', 'user', true)",
            vec![member_id.into(), format!("{member_id}@collection.test").into()],
        )
        .await;
        exec(
            &state.db,
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
            vec![workspace_id.into(), member_id.into()],
        )
        .await;
        exec(
            &state.db,
            "UPDATE flow_objects SET inherit_from_parent = false WHERE id = $1",
            vec![page_id.into()],
        )
        .await;
        let page_permission =
            authz::effective_permission(&state.db, workspace_id, page_id, "user", member_id, "member")
                .await
                .expect("page permission evaluates");
        let collection_permission =
            authz::effective_permission(&state.db, workspace_id, collection_id, "user", member_id, "member")
                .await
                .expect("embedded Collection permission evaluates");
        assert_eq!(page_permission, PermissionLevel::Denied);
        assert_eq!(collection_permission, PermissionLevel::Denied);
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_collection_atomic_create_each_fault_point_rolls_back_every_orphan_class() {
        let scratch = scratch_or_skip!("faults");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed(&state).await;
        for fault in [
            EmbedFaultPoint::Object,
            EmbedFaultPoint::Document,
            EmbedFaultPoint::Block,
            EmbedFaultPoint::Relation,
            EmbedFaultPoint::Dispatch,
        ] {
            let page_id = create_object_for(&state, workspace_id, owner_id, "page", "Embed host").await;
            let page = repository::fetch_object_view(&state.db, page_id)
                .await
                .expect("page query runs")
                .expect("page exists");
            let before = orphan_counts(&state.db, page.document_id).await;
            let input = ExecuteCommandInput {
                object_id: page_id,
                actor_id: owner_id,
                principal_kind: "user".to_string(),
                role: "owner".to_string(),
                command_type: "create_collection_embed".to_string(),
                payload: json!({"title": "Embedded"}),
                expected_frontier: None,
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
                origin_client_id: format!("fault:{owner_id}"),
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            };
            let epoch = crate::flow::collab::authz::read_epoch(&state.db, workspace_id)
                .await
                .expect("epoch reads");
            let result = execute_embed_with_fault(&state, &input, workspace_id, &page, epoch, fault).await;
            assert!(result.is_err(), "fault {fault:?} must abort the command");
            let after = orphan_counts(&state.db, page.document_id).await;
            assert_eq!(after, before, "fault {fault:?} left an orphan row");
        }
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_collection_typed_projection_failure_rolls_back_the_record_accepted_update() {
        #[derive(FromQueryResult, Debug, PartialEq, Eq)]
        struct RecordState {
            head_seq: i64,
            properties: Value,
        }

        let scratch = scratch_or_skip!("typed_atomic");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed(&state).await;
        let collection_id = create_object_for(&state, workspace_id, owner_id, "collection", "Atomic typed").await;
        let field_id = Uuid::new_v4();
        command(
            &state,
            owner_id,
            collection_id,
            "field_create",
            json!({"field_id": field_id, "label": "Count", "field_type": "number"}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("field creates");
        let record = command(
            &state,
            owner_id,
            collection_id,
            "record_create",
            json!({"properties": {field_id.to_string(): 1}}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("record creates")
        .object;
        let before = RecordState::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT cd.head_seq, rp.properties FROM collab_documents cd \
             JOIN flow_record_projections rp ON rp.document_id = cd.id WHERE rp.record_id = $1",
            vec![record.id.into()],
        ))
        .one(&state.db)
        .await
        .expect("record state query runs")
        .expect("record state exists");
        state
            .db
            .execute_unprepared(
                "CREATE FUNCTION test_fail_typed_projection() RETURNS trigger LANGUAGE plpgsql AS $$ \
                 BEGIN RAISE EXCEPTION 'injected typed projection failure'; END $$; \
                 CREATE TRIGGER test_fail_typed_projection BEFORE INSERT ON flow_record_value_projections \
                 FOR EACH ROW EXECUTE FUNCTION test_fail_typed_projection()",
            )
            .await
            .expect("failure trigger installs");
        let rejected = command(
            &state,
            owner_id,
            collection_id,
            "record_patch",
            json!({"record_id": record.id, "properties": {field_id.to_string(): 2}}),
            Uuid::new_v4().to_string(),
        )
        .await;
        assert!(
            rejected.is_err(),
            "projection failure must reject the whole accepted update"
        );
        let after = RecordState::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT cd.head_seq, rp.properties FROM collab_documents cd \
             JOIN flow_record_projections rp ON rp.document_id = cd.id WHERE rp.record_id = $1",
            vec![record.id.into()],
        ))
        .one(&state.db)
        .await
        .expect("record state query runs")
        .expect("record state exists");
        assert_eq!(
            after, before,
            "record head and typed projection must roll back together"
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn ten_thousand_record_query_uses_index_without_decoding_documents() {
        let total_started = Instant::now();
        let scratch = scratch_or_skip!("query_10k");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed(&state).await;
        let collection_id = create_object_for(&state, workspace_id, owner_id, "collection", "Ten thousand").await;
        let field_id = Uuid::new_v4();
        command(
            &state,
            owner_id,
            collection_id,
            "field_create",
            json!({"field_id": field_id, "label": "Rank", "field_type": "number"}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("number field creates");
        let fixture_sql = format!(
            r"
            CREATE TEMP TABLE query_10k_fixture AS
              SELECT gen_random_uuid() AS record_id, gen_random_uuid() AS document_id, n
                FROM generate_series(1, 10000) n;
            INSERT INTO flow_objects (id, workspace_id, object_type, parent_id, created_by)
              SELECT record_id, '{workspace_id}', 'record', '{collection_id}', '{owner_id}'
                FROM query_10k_fixture;
            INSERT INTO collab_documents
              (id, object_id, format_version, snapshot, snapshot_frontier, head_frontier, byte_count)
              SELECT document_id, record_id, 'loro-1', decode('00', 'hex'), ''::bytea, ''::bytea, 1
                FROM query_10k_fixture;
            INSERT INTO flow_object_projections
              (object_id, document_seq, document_frontier, title, state, plain_text)
              SELECT record_id, 0, ''::bytea, '', '{{}}'::jsonb, '' FROM query_10k_fixture;
            INSERT INTO flow_record_projections
              (record_id, collection_id, document_id, properties, document_seq)
              SELECT record_id, '{collection_id}', document_id,
                     jsonb_build_object('{field_id}', n), 0
                FROM query_10k_fixture;
            INSERT INTO flow_record_value_projections
              (record_id, collection_id, field_id, field_type, number_value, document_seq)
              SELECT record_id, '{collection_id}', '{field_id}', 'number', n, 0
                FROM query_10k_fixture;
            ANALYZE flow_record_projections;
            ANALYZE flow_record_value_projections;
            "
        );
        let fixture_started = Instant::now();
        state
            .db
            .execute_unprepared(&fixture_sql)
            .await
            .expect("ten thousand typed records insert");
        let fixture_ms = fixture_started.elapsed().as_millis();
        let access = read_access(&state, workspace_id, owner_id, collection_id).await;
        let query_started = Instant::now();
        let response = query_collection_records_requiring_typed_index(
            &state,
            &access,
            &RecordQueryPayload {
                filter: Some(RecordFilter {
                    field_id,
                    op: "eq".to_string(),
                    value: json!(7777),
                }),
                sort: None,
                group: None,
                cursor: None,
                limit: 10,
                field_ids: vec![field_id],
            },
        )
        .await
        .expect("typed query runs despite invalid document bytes")
        .expect("authorization epoch is stable");
        let query_ms = query_started.elapsed().as_millis();
        assert_eq!(response.items.len(), 1);
        assert_eq!(response.fields.len(), 1);
        assert_eq!(response.fields[0].field_id, field_id);
        assert_eq!(response.fields[0].label, "Rank");
        assert_eq!(
            response.items[0].values_by_field_id.get(&field_id.to_string()),
            Some(&json!(7777))
        );
        eprintln!(
            "FLOW_COLLECTION_10K_METRICS {}",
            json!({
                "records": 10_000,
                "fixture_ms": fixture_ms,
                "query_ms": query_ms,
                "total_ms": total_started.elapsed().as_millis(),
                "matched_records": response.items.len(),
                "typed_index_required": true,
            })
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn typed_projection_rebuild_matches_canonical() {
        #[derive(Debug, FromQueryResult, PartialEq)]
        struct ProjectionRow {
            record_id: Uuid,
            properties: Value,
            values: Value,
        }

        async fn projection_rows(db: &DatabaseConnection, collection_id: Uuid) -> Vec<ProjectionRow> {
            ProjectionRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT rp.record_id, rp.properties, COALESCE(jsonb_object_agg(v.field_id::text, \
                 COALESCE(to_jsonb(v.number_value), to_jsonb(v.text_value))) \
                 FILTER (WHERE v.field_id IS NOT NULL), '{}'::jsonb) AS values \
                 FROM flow_record_projections rp LEFT JOIN flow_record_value_projections v \
                 ON v.record_id = rp.record_id WHERE rp.collection_id = $1 \
                 GROUP BY rp.record_id, rp.properties ORDER BY rp.record_id",
                vec![collection_id.into()],
            ))
            .all(db)
            .await
            .expect("projection rows query runs")
        }

        #[derive(FromQueryResult)]
        struct CanonicalDocumentRow {
            record_id: Uuid,
            document_id: Uuid,
        }

        async fn canonical_rows(state: &AppState, collection_id: Uuid) -> Vec<ProjectionRow> {
            let documents = CanonicalDocumentRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT record_id, document_id FROM flow_record_projections \
                 WHERE collection_id = $1 ORDER BY record_id",
                vec![collection_id.into()],
            ))
            .all(&state.db)
            .await
            .expect("canonical document registry query runs");
            let mut rows = Vec::with_capacity(documents.len());
            for document in documents {
                let boot = bootstrap::load(&state.db, document.document_id)
                    .await
                    .expect("canonical Record document loads");
                let semantic = engine_at_head(&boot)
                    .expect("canonical Record engine loads")
                    .semantic_snapshot()
                    .expect("canonical Record semantic snapshot reads");
                let properties = semantic
                    .nodes
                    .iter()
                    .filter(|(_, node)| node.kind == NodeKind::RecordProperty && !node.deleted)
                    .map(|(field_id, node)| {
                        let value = node.properties.get("value").expect("canonical property has a value");
                        (
                            field_id.to_string(),
                            serde_json::from_str(value).expect("canonical property value is JSON"),
                        )
                    })
                    .collect::<serde_json::Map<_, _>>();
                rows.push(ProjectionRow {
                    record_id: document.record_id,
                    properties: Value::Object(properties.clone()),
                    values: Value::Object(properties),
                });
            }
            rows
        }

        let scratch = scratch_or_skip!("projection_rebuild");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed(&state).await;
        let collection_id = create_object_for(&state, workspace_id, owner_id, "collection", "Rebuild").await;
        let number_id = Uuid::new_v4();
        let text_id = Uuid::new_v4();
        for (field_id, label, field_type) in [(number_id, "Count", "number"), (text_id, "Title", "text")] {
            command(
                &state,
                owner_id,
                collection_id,
                "field_create",
                json!({"field_id": field_id, "label": label, "field_type": field_type}),
                Uuid::new_v4().to_string(),
            )
            .await
            .expect("field creates");
        }
        for (count, title) in [(7, "seven"), (11, "eleven"), (19, "nineteen")] {
            command(
                &state,
                owner_id,
                collection_id,
                "record_create",
                json!({"properties": {number_id.to_string(): count, text_id.to_string(): title}}),
                Uuid::new_v4().to_string(),
            )
            .await
            .expect("record creates");
        }
        let expected = canonical_rows(&state, collection_id).await;
        exec(
            &state.db,
            "UPDATE flow_record_projections SET properties = '{}'::jsonb WHERE collection_id = $1",
            vec![collection_id.into()],
        )
        .await;
        exec(
            &state.db,
            "DELETE FROM flow_record_value_projections WHERE collection_id = $1",
            vec![collection_id.into()],
        )
        .await;
        let rebuilt = rebuild_typed_projections(&state, collection_id)
            .await
            .expect("canonical rebuild succeeds");
        assert_eq!(rebuilt.record_count, 3);
        assert_eq!(rebuilt.field_count, 2);
        assert_eq!(rebuilt.value_count, 6);
        assert_eq!(
            projection_rows(&state.db, collection_id).await,
            expected,
            "rebuilt typed rows must match values decoded independently from canonical Record documents"
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn field_secrecy_client_crdt_denied_and_server_query_redacts_restricted_fields() {
        let scratch = scratch_or_skip!("field_secrecy");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed(&state).await;
        let collection_id = create_object_for(&state, workspace_id, owner_id, "collection", "Restricted").await;
        let field_id = Uuid::new_v4();
        command(
            &state,
            owner_id,
            collection_id,
            "field_create",
            json!({
                "field_id": field_id,
                "label": "Private salary",
                "field_type": "number",
                "config": {"restricted": true}
            }),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("restricted field creates through the server-only schema path");
        let record = command(
            &state,
            owner_id,
            collection_id,
            "record_create",
            json!({"properties": {field_id.to_string(): 9001}, "body": "PRIVATE-BODY-9001"}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("restricted record creates through the semantic API")
        .object;
        let forbidden_fixture_update = ["SET client_crdt_enabled = false", "field_secrecy_enabled = true"].join(", ");
        assert!(
            !include_str!("collections.rs").contains(&forbidden_fixture_update),
            "field secrecy gate must not manufacture production state with raw SQL"
        );
        let description = describe_collection(
            &state,
            &read_access(&state, workspace_id, owner_id, collection_id).await,
            None,
        )
        .await
        .expect("collection description runs")
        .expect("authorization epoch is stable");
        assert!(
            description.field_secrecy_enabled,
            "the production field command must enable collection field secrecy"
        );
        assert!(description.fields.is_empty());
        let invalid_client_enable = state
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE flow_collection_projections SET client_crdt_enabled = true WHERE collection_id = $1",
                vec![collection_id.into()],
            ))
            .await;
        assert!(
            invalid_client_enable.is_err(),
            "database constraint must deny client CRDT with field secrecy"
        );

        let collection_access = read_access(&state, workspace_id, owner_id, collection_id).await;
        let response = query_collection_records(
            &state,
            &collection_access,
            &RecordQueryPayload {
                filter: None,
                sort: None,
                group: None,
                cursor: None,
                limit: 10,
                field_ids: Vec::new(),
            },
        )
        .await
        .expect("server-only query runs")
        .expect("authorization epoch is stable");
        assert!(response.fields.is_empty());
        assert_eq!(response.items.len(), 1);
        assert!(response.items[0].values_by_field_id.is_empty());
        assert_eq!(response.items[0].record.semantic_content, json!({}));
        let response_json = serde_json::to_string(&response).expect("response serializes");
        for secret in ["9001", "PRIVATE-BODY-9001"] {
            assert!(
                !response_json.contains(secret),
                "server query leaked restricted record content through one response channel: {secret}"
            );
        }

        let record_access = read_access(&state, workspace_id, owner_id, record.id).await;
        let bootstrap = crate::flow::query::get_bootstrap(&state, &record_access, None, None).await;
        assert!(
            matches!(bootstrap, Err(ApiError::Forbidden(_))),
            "full Record CRDT snapshot must be denied"
        );
        let leaked_events = CountRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS count FROM business_events \
             WHERE workspace_id = $1 AND (payload::text LIKE '%PRIVATE-BODY-9001%' \
                OR metadata::text LIKE '%PRIVATE-BODY-9001%' OR payload::text LIKE '%9001%')",
            vec![workspace_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("event leakage query runs")
        .expect("event leakage count exists");
        assert_eq!(
            leaked_events.count, 0,
            "history/event delivery material must not contain restricted values"
        );
        let leaked_search = CountRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS count FROM flow_search_index search \
             JOIN flow_objects object ON object.id = search.object_id \
             WHERE object.workspace_id = $1 AND (search.title LIKE '%9001%' OR search.plain_text LIKE '%9001%')",
            vec![workspace_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("search leakage query runs")
        .expect("search leakage count exists");
        assert_eq!(
            leaked_search.count, 0,
            "restricted values must not enter search projection"
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn collection_query_cursor_field_ids_sort_and_group_are_projection_backed() {
        let scratch = scratch_or_skip!("query_page");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed(&state).await;
        let collection_id = create_object_for(&state, workspace_id, owner_id, "collection", "Query page").await;
        let group_id = Uuid::new_v4();
        let rank_id = Uuid::new_v4();
        for (field_id, label, field_type) in [(group_id, "Status", "select"), (rank_id, "Rank", "number")] {
            command(
                &state,
                owner_id,
                collection_id,
                "field_create",
                json!({"field_id": field_id, "label": label, "field_type": field_type}),
                Uuid::new_v4().to_string(),
            )
            .await
            .expect("query field creates");
        }
        for (group, rank) in [("alpha", 1), ("beta", 8), ("alpha", 9)] {
            command(
                &state,
                owner_id,
                collection_id,
                "record_create",
                json!({"properties": {group_id.to_string(): group, rank_id.to_string(): rank}}),
                Uuid::new_v4().to_string(),
            )
            .await
            .expect("query record creates");
        }
        let access = read_access(&state, workspace_id, owner_id, collection_id).await;
        let mut request = RecordQueryPayload {
            filter: None,
            sort: Some(RecordSort {
                field_id: rank_id,
                direction: "desc".to_string(),
            }),
            group: Some(group_id),
            cursor: None,
            limit: 2,
            field_ids: vec![group_id, rank_id],
        };
        let first = query_collection_records(&state, &access, &request)
            .await
            .expect("first page query runs")
            .expect("first page epoch stable");
        assert_eq!(first.items.len(), 2);
        assert_eq!(
            first
                .fields
                .iter()
                .map(|field| (field.field_id, field.label.as_str()))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([(group_id, "Status"), (rank_id, "Rank")])
        );
        assert_eq!(
            first.items[0].values_by_field_id.get(&group_id.to_string()),
            Some(&json!("alpha"))
        );
        assert_eq!(
            first.items[0].values_by_field_id.get(&rank_id.to_string()),
            Some(&json!(9))
        );
        request.cursor = first.next_cursor;
        let second = query_collection_records(&state, &access, &request)
            .await
            .expect("second page query runs")
            .expect("second page epoch stable");
        assert_eq!(second.items.len(), 1);
        assert_eq!(
            second.items[0].values_by_field_id.get(&group_id.to_string()),
            Some(&json!("beta"))
        );
        assert!(second.next_cursor.is_none());
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn schema_field_and_view_convergence_keeps_stable_ids() {
        let scratch = scratch_or_skip!("schema_convergence");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed(&state).await;
        let collection_id = create_object_for(&state, workspace_id, owner_id, "collection", "Convergence").await;
        let field_a = Uuid::new_v4();
        let field_b = Uuid::new_v4();
        let view_a = Uuid::new_v4();
        let view_b = Uuid::new_v4();
        command(
            &state,
            owner_id,
            collection_id,
            "field_create",
            json!({"field_id": field_a, "label": "Alpha", "field_type": "text"}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("base field creates through the production command path");
        command(
            &state,
            owner_id,
            collection_id,
            "view_create",
            json!({"view_id": view_a, "name": "Table", "view_type": "table"}),
            Uuid::new_v4().to_string(),
        )
        .await
        .expect("base view creates through the production command path");

        let left = async {
            command(
                &state,
                owner_id,
                collection_id,
                "field_create",
                json!({"field_id": field_b, "label": "Beta", "field_type": "number", "index": 0}),
                Uuid::new_v4().to_string(),
            )
            .await?;
            command(
                &state,
                owner_id,
                collection_id,
                "field_update",
                json!({"field_id": field_a, "label": "Alpha renamed"}),
                Uuid::new_v4().to_string(),
            )
            .await?;
            command(
                &state,
                owner_id,
                collection_id,
                "view_reorder",
                json!({"view_id": view_a, "index": 1}),
                Uuid::new_v4().to_string(),
            )
            .await
        };
        let right = async {
            command(
                &state,
                owner_id,
                collection_id,
                "view_create",
                json!({"view_id": view_b, "name": "Board", "view_type": "board", "index": 0}),
                Uuid::new_v4().to_string(),
            )
            .await?;
            command(
                &state,
                owner_id,
                collection_id,
                "view_update",
                json!({"view_id": view_a, "name": "Grid renamed"}),
                Uuid::new_v4().to_string(),
            )
            .await?;
            command(
                &state,
                owner_id,
                collection_id,
                "field_reorder",
                json!({"field_id": field_a, "index": 1}),
                Uuid::new_v4().to_string(),
            )
            .await
        };
        let (left_result, right_result) = tokio::join!(left, right);
        left_result.expect("left concurrent production command sequence converges");
        right_result.expect("right concurrent production command sequence converges");

        let described = describe_collection(
            &state,
            &read_access(&state, workspace_id, owner_id, collection_id).await,
            None,
        )
        .await
        .expect("converged collection describes")
        .expect("authorization epoch is stable");
        assert_eq!(
            described
                .fields
                .iter()
                .map(|field| (field.field_id, field.label.as_str()))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([(field_a, "Alpha renamed"), (field_b, "Beta")])
        );
        assert_eq!(
            described
                .views
                .iter()
                .map(|view| (view.view_id, view.name.as_str()))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([(view_a, "Grid renamed"), (view_b, "Board")])
        );
        assert_eq!(described.schema_seq, 8);
        scratch.drop_self().await;
    }
}
