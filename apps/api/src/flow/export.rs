//! Authorization-fenced Flow package export application service.

use std::collections::{BTreeMap, HashSet};
use std::io::Cursor;

use base64::Engine as _;
use chrono::{DateTime, Duration, Utc};
use collab_core::{CollabEngine, LoroCollabEngine};
use sea_orm::{AccessMode, ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, IsolationLevel, Statement};
use sea_orm::{TransactionTrait, Value as DbValue};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::ApiError;

use super::collab::authz::{self, PermissionLevel};
use super::event_policy::{
    flow_event_payload_policy, redact_flow_event_metadata_for_delivery, redact_flow_event_payload_for_delivery,
};
use super::package::{
    ENGINE_CRATE_VERSION, ENGINE_NAME, ENGINE_WIRE_FORMAT_VERSION, ExportPackageManifest, ExportPolicy, PackageCounts,
    PackageEngine, PackageHistory, PackageMemberInput, PackageProducer, PackageSource, build_package, verify_package,
};

const EXPORT_ARTIFACT_TTL_MINUTES: i64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportScope {
    Object(Uuid),
    Workspace {
        workspace_id: Uuid,
        project_id: Option<Uuid>,
    },
}

impl ExportScope {
    const fn workspace_id_hint(self) -> Option<Uuid> {
        match self {
            Self::Object(_) => None,
            Self::Workspace { workspace_id, .. } => Some(workspace_id),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExportPrincipal {
    pub id: Uuid,
    pub kind: String,
    pub role: String,
    pub workspace_export_capability: bool,
}

#[derive(Debug, Clone)]
pub struct CreateExportRequest {
    pub scope: ExportScope,
    pub include_history: bool,
    pub idempotency_key: String,
    pub source_head: String,
    pub principal: ExportPrincipal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportJobReceipt {
    pub job_id: Uuid,
    pub status: String,
    pub format: String,
    pub workspace_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_id: Option<Uuid>,
    pub package_schema: String,
    pub checksum: String,
    pub size: u64,
    pub expires_at: String,
}

#[derive(Debug, FromQueryResult)]
struct ExportObjectRow {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    parent_id: Option<Uuid>,
    object_type: String,
    lifecycle_status: String,
    governance_metadata: Value,
    document_id: Uuid,
    engine: String,
    format_version: String,
    snapshot: Vec<u8>,
    snapshot_checksum: String,
    snapshot_frontier: Vec<u8>,
    snapshot_seq: i64,
    head_frontier: Vec<u8>,
    head_seq: i64,
    projection_seq: i64,
    projection_frontier: Vec<u8>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    archived_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromQueryResult)]
struct ExportUpdateRow {
    seq: i64,
    content_hash: String,
    before_frontier: Vec<u8>,
    after_frontier: Vec<u8>,
    bytes: Vec<u8>,
}

#[derive(Debug, FromQueryResult)]
struct ExportRelationRow {
    id: Uuid,
    relation_type: String,
    source_object_id: Uuid,
    target_object_id: Uuid,
    position_key: String,
    properties: Value,
    created_at: DateTime<Utc>,
}

#[derive(Debug, FromQueryResult)]
struct ExportLineageRow {
    source_kind: String,
    source_id: Uuid,
    source_content_hash: String,
    target_object_id: Option<Uuid>,
    target_document_id: Option<Uuid>,
    result: String,
    imported_at: DateTime<Utc>,
}

#[derive(Debug, FromQueryResult)]
struct ExportEventRow {
    id: Uuid,
    event_type: String,
    aggregate_type: String,
    aggregate_id: String,
    actor_id: Option<Uuid>,
    source: Value,
    payload: Value,
    metadata: Value,
    created_at: DateTime<Utc>,
}

#[derive(Debug, FromQueryResult)]
struct ExistingJob {
    id: Uuid,
    workspace_id: Uuid,
    object_id: Option<Uuid>,
    request_hash: String,
    status: String,
    package_sha256: Option<String>,
    size_bytes: Option<i64>,
    expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromQueryResult)]
struct ExportAccessRow {
    id: Uuid,
    workspace_id: Uuid,
    object_id: Option<Uuid>,
    actor_kind: String,
    actor_id: Uuid,
    status: String,
    package_sha256: Option<String>,
    size_bytes: Option<i64>,
    expires_at: Option<DateTime<Utc>>,
    package_bytes: Option<Vec<u8>>,
}

pub async fn create_package_export(
    db: &DatabaseConnection,
    request: &CreateExportRequest,
) -> Result<ExportJobReceipt, ApiError> {
    validate_request(request)?;
    let request_hash = request_hash(request)?;
    if let Some(workspace_id) = request.scope.workspace_id_hint()
        && let Some(existing) = find_existing_job(db, workspace_id, request, &request_hash).await?
    {
        return Ok(existing);
    }

    let tx = db
        .begin_with_config(Some(IsolationLevel::RepeatableRead), Some(AccessMode::ReadWrite))
        .await?;
    let objects = load_scope(&tx, request.scope).await?;
    let first = objects
        .first()
        .ok_or_else(|| ApiError::NotFound("Flow export scope is empty".to_string()))?;
    let workspace_id = first.workspace_id;
    if request
        .scope
        .workspace_id_hint()
        .is_some_and(|expected| expected != workspace_id)
        || objects.iter().any(|row| row.workspace_id != workspace_id)
    {
        return Err(ApiError::NotFound("Flow export scope is empty".to_string()));
    }

    lock_authorization_epoch(&tx, workspace_id).await?;
    enforce_scope_principal(request, workspace_id)?;
    let object_ids: Vec<Uuid> = objects.iter().map(|row| row.id).collect();
    authorize_all(&tx, workspace_id, &object_ids, &request.principal, "export").await?;
    if request.include_history {
        authorize_all(&tx, workspace_id, &object_ids, &request.principal, "history export").await?;
    }

    let selected: HashSet<Uuid> = object_ids.iter().copied().collect();
    let mut members = Vec::new();
    let mut through_seq_by_document = BTreeMap::new();
    let mut update_count = 0u64;
    for object in &objects {
        let updates = load_updates(&tx, object).await?;
        let (full_snapshot, semantic_hash) = reconstruct_and_verify(object, &updates)?;
        let exported_snapshot = if request.include_history {
            object.snapshot.clone()
        } else {
            full_snapshot
        };
        let object_json = json!({
            "source_object_id": object.id,
            "source_workspace_id": object.workspace_id,
            "object_type": object.object_type,
            "lifecycle_status": object.lifecycle_status,
            "project_id": object.project_id,
            "parent_object_id": object.parent_id.filter(|parent| selected.contains(parent)),
            "governance_metadata": object.governance_metadata,
            "source_document_id": object.document_id,
            "engine": object.engine,
            "format_version": object.format_version,
            "accepted_seq": object.head_seq,
            "accepted_frontier": base64::engine::general_purpose::STANDARD.encode(&object.head_frontier),
            "semantic_hash": semantic_hash,
            "snapshot_sha256": sha256_hex(&exported_snapshot),
            "projection_seq": object.projection_seq,
            "created_at": object.created_at.to_rfc3339(),
            "updated_at": object.updated_at.to_rfc3339(),
            "archived_at": object.archived_at.map(|value| value.to_rfc3339()),
        });
        members.push(PackageMemberInput {
            path: format!("objects/{}/object.json", object.id),
            kind: "object".to_string(),
            bytes: canonical_json(&object_json)?,
        });
        members.push(PackageMemberInput {
            path: format!("documents/{}/snapshot.bin", object.document_id),
            kind: "snapshot".to_string(),
            bytes: exported_snapshot,
        });
        if request.include_history {
            for update in updates {
                update_count = update_count.saturating_add(1);
                members.push(PackageMemberInput {
                    path: format!("documents/{}/updates/{}.bin", object.document_id, update.seq),
                    kind: "update".to_string(),
                    bytes: update.bytes,
                });
            }
        }
        through_seq_by_document.insert(object.document_id.to_string(), object.head_seq);
    }

    let relations = load_relations(&tx, workspace_id, &object_ids).await?;
    let external_ids: Vec<Uuid> = relations
        .iter()
        .flat_map(|row| [row.source_object_id, row.target_object_id])
        .filter(|id| !selected.contains(id))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    authorize_all(
        &tx,
        workspace_id,
        &external_ids,
        &request.principal,
        "relation endpoint export",
    )
    .await?;
    let relation_jsonl = canonical_jsonl(relations.iter().map(|row| {
        json!({
            "source_relation_id": row.id,
            "relation_type": row.relation_type,
            "source": {"kind": if selected.contains(&row.source_object_id) {"internal"} else {"external"}, "source_object_id": row.source_object_id},
            "target": {"kind": if selected.contains(&row.target_object_id) {"internal"} else {"external"}, "source_object_id": row.target_object_id},
            "position_key": row.position_key,
            "properties": row.properties,
            "created_at": row.created_at.to_rfc3339(),
        })
    }))?;
    members.push(PackageMemberInput {
        path: "relations/relations.jsonl".to_string(),
        kind: "relation".to_string(),
        bytes: relation_jsonl,
    });

    let lineage = load_lineage(&tx, &object_ids).await?;
    let lineage_jsonl = canonical_jsonl(lineage.iter().map(|row| {
        json!({
            "source_kind": row.source_kind,
            "source_id": row.source_id,
            "source_content_hash": row.source_content_hash,
            "target_object_id": row.target_object_id,
            "target_document_id": row.target_document_id,
            "result": row.result,
            "imported_at": row.imported_at.to_rfc3339(),
        })
    }))?;
    members.push(PackageMemberInput {
        path: "lineage/lineage.jsonl".to_string(),
        kind: "lineage".to_string(),
        bytes: lineage_jsonl,
    });

    let events = if request.include_history {
        load_history_events(&tx, workspace_id, &object_ids).await?
    } else {
        Vec::new()
    };
    if request.include_history {
        let event_jsonl = canonical_jsonl(events.iter().map(|row| {
            let surface = row.source.get("surface").and_then(Value::as_str);
            json!({
                "source_event_id": row.id,
                "event_type": row.event_type,
                "aggregate_type": row.aggregate_type,
                "aggregate_id": row.aggregate_id,
                "actor": row.actor_id,
                "origin": surface,
                "payload": redact_flow_event_payload_for_delivery(&row.event_type, &row.payload),
                "metadata": redact_flow_event_metadata_for_delivery(&row.metadata),
                "created_at": row.created_at.to_rfc3339(),
            })
        }))?;
        members.push(PackageMemberInput {
            path: "history/events.jsonl".to_string(),
            kind: "event".to_string(),
            bytes: event_jsonl,
        });
    }

    let roots = selected_roots(&objects, &selected);
    let now = Utc::now();
    let manifest = ExportPackageManifest {
        schema: super::package::PACKAGE_SCHEMA.to_string(),
        package_id: Uuid::new_v4().to_string(),
        created_at: now.to_rfc3339(),
        producer: PackageProducer {
            product: "sylvode".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            source_head: request.source_head.clone(),
        },
        source: PackageSource {
            workspace_id: workspace_id.to_string(),
            scope: match request.scope {
                ExportScope::Object(_) => "object",
                ExportScope::Workspace { .. } => "workspace",
            }
            .to_string(),
            root_object_ids: roots.iter().map(ToString::to_string).collect(),
        },
        flow_schema_version: super::package::FLOW_SCHEMA_VERSION,
        engine: PackageEngine {
            name: ENGINE_NAME.to_string(),
            crate_version: ENGINE_CRATE_VERSION.to_string(),
            wire_format_version: ENGINE_WIRE_FORMAT_VERSION,
        },
        history: PackageHistory {
            included: request.include_history,
            through_seq_by_document,
        },
        counts: PackageCounts {
            objects: u64::try_from(objects.len()).unwrap_or(u64::MAX),
            documents: u64::try_from(objects.len()).unwrap_or(u64::MAX),
            updates: update_count,
            relations: u64::try_from(relations.len()).unwrap_or(u64::MAX),
            lineage: u64::try_from(lineage.len()).unwrap_or(u64::MAX),
            events: u64::try_from(events.len()).unwrap_or(u64::MAX),
        },
        members: Vec::new(),
        export_policy: ExportPolicy {
            complete: true,
            permission_snapshot_at: now.to_rfc3339(),
        },
    };
    let package = build_package(manifest, members)?;
    verify_package(Cursor::new(&package.bytes), Some(&package.package_sha256))?;
    tx.commit().await?;

    persist_export(
        db,
        request,
        workspace_id,
        &request_hash,
        package.bytes,
        package.package_sha256,
    )
    .await
}

pub async fn get_package_export(
    db: &DatabaseConnection,
    job_id: Uuid,
    principal: &ExportPrincipal,
) -> Result<ExportJobReceipt, ApiError> {
    let row = export_access_row(db, job_id).await?;
    authorize_export_access(db, &row, principal).await?;
    access_receipt(&row)
}

pub async fn download_package_export(
    db: &DatabaseConnection,
    job_id: Uuid,
    principal: &ExportPrincipal,
) -> Result<(Vec<u8>, String), ApiError> {
    let row = export_access_row(db, job_id).await?;
    authorize_export_access(db, &row, principal).await?;
    if row.expires_at.is_none_or(|expires| expires <= Utc::now()) {
        return Err(ApiError::NotFound("export artifact expired".to_string()));
    }
    Ok((
        row.package_bytes
            .ok_or_else(|| ApiError::NotFound("export artifact not found".to_string()))?,
        row.package_sha256.ok_or(ApiError::Internal)?,
    ))
}

async fn export_access_row(db: &DatabaseConnection, job_id: Uuid) -> Result<ExportAccessRow, ApiError> {
    ExportAccessRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT j.id,j.workspace_id,j.object_id,j.actor_kind,j.actor_id,j.status,j.package_sha256,j.size_bytes,j.expires_at,a.package_bytes \
         FROM flow_export_jobs j LEFT JOIN flow_package_artifacts a ON a.id=j.artifact_id WHERE j.id=$1",
        vec![job_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("export job not found".to_string()))
}

async fn authorize_export_access(
    db: &DatabaseConnection,
    row: &ExportAccessRow,
    principal: &ExportPrincipal,
) -> Result<(), ApiError> {
    let initiating = row.actor_kind == principal.kind && row.actor_id == principal.id;
    if !(initiating || principal.kind == "user" && matches!(principal.role.as_str(), "owner" | "admin")) {
        return Err(ApiError::NotFound("export job not found".to_string()));
    }
    let tx = db.begin().await?;
    lock_authorization_epoch(&tx, row.workspace_id).await?;
    if let Some(object_id) = row.object_id {
        authorize_all(&tx, row.workspace_id, &[object_id], principal, "export download").await?;
    } else if !((principal.kind == "user" && matches!(principal.role.as_str(), "owner" | "admin"))
        || (principal.kind == "bot" && principal.workspace_export_capability))
    {
        return Err(ApiError::Forbidden(
            "workspace export permission is required".to_string(),
        ));
    }
    tx.commit().await?;
    Ok(())
}

fn access_receipt(row: &ExportAccessRow) -> Result<ExportJobReceipt, ApiError> {
    Ok(ExportJobReceipt {
        job_id: row.id,
        status: row.status.clone(),
        format: "package".to_string(),
        workspace_id: row.workspace_id,
        object_id: row.object_id,
        package_schema: "v1".to_string(),
        checksum: row.package_sha256.clone().ok_or(ApiError::Internal)?,
        size: u64::try_from(row.size_bytes.ok_or(ApiError::Internal)?).map_err(|_| ApiError::Internal)?,
        expires_at: row.expires_at.ok_or(ApiError::Internal)?.to_rfc3339(),
    })
}

fn validate_request(request: &CreateExportRequest) -> Result<(), ApiError> {
    if request.idempotency_key.is_empty() || request.idempotency_key.len() > 128 {
        return Err(ApiError::invalid_update("idempotency_key must contain 1..128 bytes"));
    }
    if !matches!(request.principal.kind.as_str(), "user" | "bot") {
        return Err(ApiError::unauthenticated("unsupported principal kind"));
    }
    Ok(())
}

fn enforce_scope_principal(request: &CreateExportRequest, _workspace_id: Uuid) -> Result<(), ApiError> {
    if matches!(request.scope, ExportScope::Workspace { .. }) {
        let permitted = if request.principal.kind == "user" {
            matches!(request.principal.role.as_str(), "owner" | "admin")
        } else {
            request.principal.workspace_export_capability
        };
        if !permitted {
            return Err(ApiError::Forbidden(
                "workspace package export permission denied".to_string(),
            ));
        }
    }
    Ok(())
}

async fn lock_authorization_epoch<C: ConnectionTrait>(conn: &C, workspace_id: Uuid) -> Result<(), ApiError> {
    let row = conn
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT authz_epoch FROM flow_workspace_settings WHERE workspace_id=$1 AND flow_enabled=true FOR SHARE",
            vec![workspace_id.into()],
        ))
        .await?;
    if row.is_none() {
        return Err(ApiError::feature_disabled("Flow is disabled for this workspace"));
    }
    Ok(())
}

async fn authorize_all<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    object_ids: &[Uuid],
    principal: &ExportPrincipal,
    action: &str,
) -> Result<(), ApiError> {
    let levels = authz::effective_permissions(
        conn,
        workspace_id,
        object_ids,
        &principal.kind,
        principal.id,
        &principal.role,
    )
    .await?;
    if levels.len() != object_ids.len() || levels.iter().any(|(_, level)| *level < PermissionLevel::View) {
        return Err(ApiError::Forbidden(format!("{action} permission denied")));
    }
    Ok(())
}

async fn load_scope<C: ConnectionTrait>(conn: &C, scope: ExportScope) -> Result<Vec<ExportObjectRow>, ApiError> {
    let base = "SELECT fo.id,fo.workspace_id,fo.project_id,fo.parent_id,fo.object_type,fo.lifecycle_status, \
        fo.governance_metadata,cd.id AS document_id,cd.engine,cd.format_version,cd.snapshot, \
        cd.snapshot_checksum,cd.snapshot_frontier,cd.snapshot_seq,cd.head_frontier,cd.head_seq, \
        fp.document_seq AS projection_seq,fp.document_frontier AS projection_frontier, \
        fo.created_at,fo.updated_at,fo.archived_at \
        FROM flow_objects fo JOIN collab_documents cd ON cd.object_id=fo.id \
        JOIN flow_object_projections fp ON fp.object_id=fo.id";
    let (sql, values): (String, Vec<DbValue>) = match scope {
        ExportScope::Object(object_id) => (
            format!(
                "{base} WHERE fo.id=$1 AND NOT flow_is_system_navigator_root(fo.object_type,fo.parent_id,fo.governance_metadata) ORDER BY fo.id"
            ),
            vec![object_id.into()],
        ),
        ExportScope::Workspace {
            workspace_id,
            project_id,
        } => (
            format!(
                "{base} WHERE fo.workspace_id=$1 AND ($2::uuid IS NULL OR fo.project_id=$2) \
                 AND NOT flow_is_system_navigator_root(fo.object_type,fo.parent_id,fo.governance_metadata) ORDER BY fo.id"
            ),
            vec![workspace_id.into(), project_id.into()],
        ),
    };
    ExportObjectRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .all(conn)
        .await
        .map_err(Into::into)
}

async fn load_updates<C: ConnectionTrait>(
    conn: &C,
    object: &ExportObjectRow,
) -> Result<Vec<ExportUpdateRow>, ApiError> {
    ExportUpdateRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT seq,content_hash,before_frontier,after_frontier,bytes \
         FROM collab_updates WHERE document_id=$1 AND seq>$2 AND seq<=$3 ORDER BY seq",
        vec![
            object.document_id.into(),
            object.snapshot_seq.into(),
            object.head_seq.into(),
        ],
    ))
    .all(conn)
    .await
    .map_err(Into::into)
}

fn reconstruct_and_verify(
    object: &ExportObjectRow,
    updates: &[ExportUpdateRow],
) -> Result<(Vec<u8>, String), ApiError> {
    if sha256_hex(&object.snapshot) != object.snapshot_checksum {
        return Err(ApiError::checksum_mismatch(
            "canonical snapshot checksum mismatch during export",
        ));
    }
    let mut engine = LoroCollabEngine::load(&object.snapshot)
        .map_err(|_| ApiError::checksum_mismatch("canonical snapshot cannot be decoded during export"))?;
    if engine.frontier().as_bytes() != object.snapshot_frontier {
        return Err(ApiError::checksum_mismatch(
            "canonical snapshot frontier mismatch during export",
        ));
    }
    let expected_count = object.head_seq.saturating_sub(object.snapshot_seq);
    if usize::try_from(expected_count).unwrap_or(usize::MAX) != updates.len() {
        return Err(ApiError::checksum_mismatch(
            "canonical update tail has a sequence gap during export",
        ));
    }
    let mut expected_seq = object.snapshot_seq.saturating_add(1);
    for update in updates {
        if update.seq != expected_seq
            || update.before_frontier != engine.frontier().as_bytes()
            || sha256_hex(&update.bytes) != update.content_hash
            || update.bytes.len() > 65_536
        {
            return Err(ApiError::checksum_mismatch(
                "canonical update tail is inconsistent during export",
            ));
        }
        engine
            .import_update(&update.bytes)
            .map_err(|_| ApiError::checksum_mismatch("canonical update cannot be decoded during export"))?;
        if update.after_frontier != engine.frontier().as_bytes() {
            return Err(ApiError::checksum_mismatch(
                "canonical update frontier mismatch during export",
            ));
        }
        expected_seq = expected_seq.saturating_add(1);
    }
    if engine.frontier().as_bytes() != object.head_frontier
        || object.projection_seq != object.head_seq
        || object.projection_frontier != object.head_frontier
    {
        return Err(ApiError::checksum_mismatch(
            "canonical head and projection frontier disagree during export",
        ));
    }
    let semantic_hash = engine
        .semantic_snapshot()
        .map_err(|_| ApiError::checksum_mismatch("canonical semantic snapshot cannot be decoded"))?
        .semantic_hash();
    let snapshot = engine
        .export_snapshot()
        .map_err(|_| ApiError::checksum_mismatch("canonical head snapshot cannot be exported"))?;
    Ok((snapshot, semantic_hash))
}

async fn load_relations<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    object_ids: &[Uuid],
) -> Result<Vec<ExportRelationRow>, ApiError> {
    ExportRelationRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id,relation_type,source_object_id,target_object_id,position_key,properties,created_at \
         FROM flow_relations WHERE workspace_id=$1 AND (source_object_id=ANY($2) OR target_object_id=ANY($2)) \
         ORDER BY id",
        vec![workspace_id.into(), object_ids.to_vec().into()],
    ))
    .all(conn)
    .await
    .map_err(Into::into)
}

async fn load_lineage<C: ConnectionTrait>(conn: &C, object_ids: &[Uuid]) -> Result<Vec<ExportLineageRow>, ApiError> {
    ExportLineageRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT source_kind,source_id,source_content_hash,target_object_id,target_document_id,result,imported_at \
         FROM flow_import_lineage WHERE target_object_id=ANY($1) ORDER BY source_kind,source_id",
        vec![object_ids.to_vec().into()],
    ))
    .all(conn)
    .await
    .map_err(Into::into)
}

async fn load_history_events<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    object_ids: &[Uuid],
) -> Result<Vec<ExportEventRow>, ApiError> {
    let rows = ExportEventRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT DISTINCT be.id,be.event_type,be.aggregate_type,be.aggregate_id,be.actor_id,be.source,be.payload,be.metadata,be.created_at \
         FROM business_events be JOIN flow_objects fo ON fo.workspace_id=be.workspace_id \
           AND (be.aggregate_id=fo.id::text OR be.payload->>'object_id'=fo.id::text) \
         WHERE be.workspace_id=$1 AND fo.id=ANY($2) AND be.event_type<>'flow.command.rejected' ORDER BY be.created_at,be.id",
        vec![workspace_id.into(), object_ids.to_vec().into()],
    ))
    .all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|row| flow_event_payload_policy(&row.event_type).is_some())
        .collect())
}

fn selected_roots(objects: &[ExportObjectRow], selected: &HashSet<Uuid>) -> Vec<Uuid> {
    let mut roots: Vec<Uuid> = objects
        .iter()
        .filter(|row| row.parent_id.is_none_or(|parent| !selected.contains(&parent)))
        .map(|row| row.id)
        .collect();
    roots.sort_unstable();
    roots
}

fn canonical_json(value: &Value) -> Result<Vec<u8>, ApiError> {
    serde_jcs::to_vec(value).map_err(|_| ApiError::Internal)
}

fn canonical_jsonl(values: impl Iterator<Item = Value>) -> Result<Vec<u8>, ApiError> {
    let mut output = Vec::new();
    for value in values {
        output.extend_from_slice(&canonical_json(&value)?);
        output.push(b'\n');
    }
    Ok(output)
}

fn request_hash(request: &CreateExportRequest) -> Result<String, ApiError> {
    let scope = match request.scope {
        ExportScope::Object(object_id) => json!({"kind":"object","object_id":object_id}),
        ExportScope::Workspace {
            workspace_id,
            project_id,
        } => json!({"kind":"workspace","workspace_id":workspace_id,"project_id":project_id}),
    };
    Ok(sha256_hex(&canonical_json(&json!({
        "scope": scope,
        "format": "package",
        "include_history": request.include_history,
    }))?))
}

async fn find_existing_job(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    request: &CreateExportRequest,
    request_hash: &str,
) -> Result<Option<ExportJobReceipt>, ApiError> {
    let row = ExistingJob::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id,workspace_id,object_id,request_hash,status,package_sha256,size_bytes,expires_at \
         FROM flow_export_jobs WHERE workspace_id=$1 AND actor_kind=$2 AND actor_id=$3 AND idempotency_key=$4",
        vec![
            workspace_id.into(),
            request.principal.kind.clone().into(),
            request.principal.id.into(),
            request.idempotency_key.clone().into(),
        ],
    ))
    .one(db)
    .await?;
    row.map(|row| existing_receipt(row, request_hash)).transpose()
}

fn existing_receipt(row: ExistingJob, request_hash: &str) -> Result<ExportJobReceipt, ApiError> {
    if row.request_hash != request_hash {
        return Err(ApiError::Conflict("export idempotency key body drift".to_string()));
    }
    Ok(ExportJobReceipt {
        job_id: row.id,
        status: row.status,
        format: "package".to_string(),
        workspace_id: row.workspace_id,
        object_id: row.object_id,
        package_schema: "v1".to_string(),
        checksum: row.package_sha256.ok_or(ApiError::Internal)?,
        size: u64::try_from(row.size_bytes.ok_or(ApiError::Internal)?).map_err(|_| ApiError::Internal)?,
        expires_at: row.expires_at.ok_or(ApiError::Internal)?.to_rfc3339(),
    })
}

async fn persist_export(
    db: &DatabaseConnection,
    request: &CreateExportRequest,
    workspace_id: Uuid,
    request_hash: &str,
    bytes: Vec<u8>,
    package_sha256: String,
) -> Result<ExportJobReceipt, ApiError> {
    let tx = db.begin().await?;
    let job_id = Uuid::new_v4();
    let artifact_id = Uuid::new_v4();
    let expires_at = Utc::now() + Duration::minutes(EXPORT_ARTIFACT_TTL_MINUTES);
    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_package_artifacts \
         (id,workspace_id,actor_kind,actor_id,purpose,package_sha256,size_bytes,package_bytes,expires_at) \
         VALUES ($1,$2,$3,$4,'export',$5,$6,$7,$8)",
        vec![
            artifact_id.into(),
            workspace_id.into(),
            request.principal.kind.clone().into(),
            request.principal.id.into(),
            package_sha256.clone().into(),
            i64::try_from(size).unwrap_or(i64::MAX).into(),
            bytes.into(),
            expires_at.into(),
        ],
    ))
    .await?;
    let object_id = match request.scope {
        ExportScope::Object(id) => Some(id),
        ExportScope::Workspace { .. } => None,
    };
    let project_id = match request.scope {
        ExportScope::Workspace { project_id, .. } => project_id,
        ExportScope::Object(_) => None,
    };
    let inserted = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_export_jobs \
             (id,workspace_id,object_id,project_id,scope_kind,format,include_history,actor_kind,actor_id, \
              idempotency_key,request_hash,status,artifact_id,package_sha256,size_bytes,expires_at) \
             VALUES ($1,$2,$3,$4,$5,'package',$6,$7,$8,$9,$10,'completed',$11,$12,$13,$14) \
             ON CONFLICT (workspace_id,actor_kind,actor_id,idempotency_key) DO NOTHING",
            vec![
                job_id.into(),
                workspace_id.into(),
                object_id.into(),
                project_id.into(),
                match request.scope {
                    ExportScope::Object(_) => "object",
                    ExportScope::Workspace { .. } => "workspace",
                }
                .into(),
                request.include_history.into(),
                request.principal.kind.clone().into(),
                request.principal.id.into(),
                request.idempotency_key.clone().into(),
                request_hash.to_string().into(),
                artifact_id.into(),
                package_sha256.clone().into(),
                i64::try_from(size).unwrap_or(i64::MAX).into(),
                expires_at.into(),
            ],
        ))
        .await?;
    if inserted.rows_affected() == 0 {
        tx.rollback().await?;
        return find_existing_job(db, workspace_id, request, request_hash)
            .await?
            .ok_or(ApiError::Internal);
    }
    tx.commit().await?;
    Ok(ExportJobReceipt {
        job_id,
        status: "completed".to_string(),
        format: "package".to_string(),
        workspace_id,
        object_id,
        package_schema: "v1".to_string(),
        checksum: package_sha256,
        size,
        expires_at: expires_at.to_rfc3339(),
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::flow::command::{
        CreateObjectInput, ExecuteCommandInput, SetFlowFeatureInput, create_object, execute_command, set_flow_feature,
    };
    use crate::flow::event_origin::{CommandOrigin, EventSurface};
    use crate::routes::context::tenant_fixture::{exec, scratch, seed_tenant};
    use platform::{
        app::AppState,
        config::{AppConfig, Secret},
    };
    use sea_orm::DatabaseConnection;
    use std::io::Read as _;

    fn state_for(db: DatabaseConnection) -> AppState {
        AppState {
            cfg: AppConfig {
                app_name: "flow-export-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("flow-export-test-secret"),
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

    fn principal(id: Uuid, role: &str) -> ExportPrincipal {
        ExportPrincipal {
            id,
            kind: "user".to_string(),
            role: role.to_string(),
            workspace_export_capability: false,
        }
    }

    async fn artifact_bytes(db: &DatabaseConnection, job_id: Uuid) -> Vec<u8> {
        #[derive(FromQueryResult)]
        struct Row {
            package_bytes: Vec<u8>,
        }
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT a.package_bytes FROM flow_export_jobs j \
             JOIN flow_package_artifacts a ON a.id=j.artifact_id WHERE j.id=$1",
            vec![job_id.into()],
        ))
        .one(db)
        .await
        .unwrap()
        .expect("export artifact exists")
        .package_bytes
    }

    fn package_entry(package: &[u8], path: &str) -> Vec<u8> {
        let mut archive = zip::ZipArchive::new(Cursor::new(package)).unwrap();
        let mut entry = archive.by_name(path).unwrap();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        bytes
    }

    async fn fixture(label: &str) -> Option<(crate::routes::context::tenant_fixture::Scratch, AppState, Uuid, Uuid)> {
        let scratch = scratch(label).await?;
        let tenant = seed_tenant(&scratch.db, label).await;
        exec(
            &scratch.db,
            "UPDATE workspace_members SET role='owner' WHERE workspace_id=$1 AND user_id=$2",
            vec![tenant.workspace_id.into(), tenant.member_id.into()],
        )
        .await;
        let state = state_for(scratch.db.clone());
        set_flow_feature(
            &state,
            SetFlowFeatureInput {
                workspace_id: tenant.workspace_id,
                actor_id: tenant.member_id,
                actor_is_bot: false,
                enabled: Some(true),
                default_member_level: Some("edit".to_string()),
                idempotency_key: format!("enable-{label}"),
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            },
        )
        .await
        .expect("Flow feature enables through production service");
        #[derive(FromQueryResult)]
        struct Root {
            id: Uuid,
        }
        let navigator = Root::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM flow_objects WHERE workspace_id=$1 AND object_type='navigator' AND parent_id IS NULL",
            vec![tenant.workspace_id.into()],
        ))
        .one(&scratch.db)
        .await
        .unwrap()
        .expect("workspace trigger creates navigator")
        .id;
        let page = create_object(
            &state,
            CreateObjectInput {
                workspace_id: tenant.workspace_id,
                actor_id: tenant.member_id,
                actor_is_bot: false,
                object_type: "page".to_string(),
                project_id: None,
                parent_object_id: Some(navigator),
                title: "export source".to_string(),
                idempotency_key: format!("create-{label}"),
                message: None,
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            },
        )
        .await
        .expect("page created through production service")
        .object
        .id;
        execute_command(
            &state,
            ExecuteCommandInput {
                object_id: page,
                actor_id: tenant.member_id,
                principal_kind: "user".to_string(),
                role: "owner".to_string(),
                command_type: "set_title".to_string(),
                payload: json!({"title":"accepted head title"}),
                expected_frontier: None,
                idempotency_key: format!("update-{label}"),
                message: Some("export history marker".to_string()),
                origin_client_id: "flow-export-test".to_string(),
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            },
        )
        .await
        .expect("nonzero accepted tail created through production command");
        Some((scratch, state, tenant.workspace_id, tenant.member_id))
    }

    #[tokio::test]
    async fn workspace_and_object_exports_reconstruct_exact_head_and_freeze_idempotency() {
        let Some((scratch, _state, workspace_id, owner_id)) = fixture("flow_export_roundtrip").await else {
            eprintln!("SKIPPED (no database): set OPENPR_TEST_DATABASE_URL to run this test");
            return;
        };
        #[derive(FromQueryResult)]
        struct Page {
            object_id: Uuid,
            document_id: Uuid,
            head_seq: i64,
            head_frontier: Vec<u8>,
        }
        let page = Page::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT fo.id AS object_id,cd.id AS document_id,cd.head_seq,cd.head_frontier \
             FROM flow_objects fo JOIN collab_documents cd ON cd.object_id=fo.id \
             WHERE fo.workspace_id=$1 AND fo.object_type='page'",
            vec![workspace_id.into()],
        ))
        .one(&scratch.db)
        .await
        .unwrap()
        .unwrap();
        assert_eq!(page.head_seq, 1, "fixture must cross the snapshot/tail branch");

        let request = CreateExportRequest {
            scope: ExportScope::Workspace {
                workspace_id,
                project_id: None,
            },
            include_history: false,
            idempotency_key: "workspace-package".to_string(),
            source_head: "0123456789abcdef0123456789abcdef01234567".to_string(),
            principal: principal(owner_id, "owner"),
        };
        let receipt = create_package_export(&scratch.db, &request).await.unwrap();
        let replay = create_package_export(&scratch.db, &request).await.unwrap();
        assert_eq!(receipt.job_id, replay.job_id);
        assert_eq!(receipt.checksum, replay.checksum);
        let package = artifact_bytes(&scratch.db, receipt.job_id).await;
        let verified = verify_package(Cursor::new(&package), Some(&receipt.checksum)).unwrap();
        assert_eq!(verified.manifest.counts.objects, 1);
        assert_eq!(verified.manifest.counts.documents, 1);
        assert_eq!(verified.manifest.counts.updates, 0);
        assert!(!verified.manifest.history.included);
        assert_eq!(
            verified.manifest.history.through_seq_by_document[&page.document_id.to_string()],
            page.head_seq
        );
        let exported_snapshot = package_entry(&package, &format!("documents/{}/snapshot.bin", page.document_id));
        let engine = LoroCollabEngine::load(&exported_snapshot).unwrap();
        assert_eq!(engine.frontier().as_bytes(), page.head_frontier);
        let object: Value = serde_json::from_slice(&package_entry(
            &package,
            &format!("objects/{}/object.json", page.object_id),
        ))
        .unwrap();
        assert_eq!(object["accepted_seq"], page.head_seq);
        assert_eq!(
            object["accepted_frontier"],
            base64::engine::general_purpose::STANDARD.encode(&page.head_frontier)
        );
        assert_eq!(
            object["semantic_hash"],
            engine.semantic_snapshot().unwrap().semantic_hash(),
            "exported metadata must fingerprint the reconstructed accepted head"
        );
        assert!(
            zip::ZipArchive::new(Cursor::new(&package))
                .unwrap()
                .file_names()
                .all(|name| !name.contains("/updates/")),
            "history=false exports a compact head snapshot without tail members"
        );

        let drift = CreateExportRequest {
            include_history: true,
            ..request.clone()
        };
        assert!(matches!(
            create_package_export(&scratch.db, &drift).await,
            Err(ApiError::Conflict(_))
        ));

        let object_request = CreateExportRequest {
            scope: ExportScope::Object(page.object_id),
            include_history: true,
            idempotency_key: "object-history-package".to_string(),
            source_head: request.source_head,
            principal: principal(owner_id, "owner"),
        };
        let object_receipt = create_package_export(&scratch.db, &object_request).await.unwrap();
        let history_package = artifact_bytes(&scratch.db, object_receipt.job_id).await;
        let history = verify_package(Cursor::new(&history_package), Some(&object_receipt.checksum)).unwrap();
        assert_eq!(history.manifest.counts.objects, 1);
        assert_eq!(history.manifest.counts.updates, 1);
        assert!(history.manifest.history.included);
        assert!(!package_entry(&history_package, "history/events.jsonl").is_empty());
        assert!(
            zip::ZipArchive::new(Cursor::new(&history_package))
                .unwrap()
                .by_name(&format!("documents/{}/updates/1.bin", page.document_id))
                .is_ok()
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn workspace_export_requires_admin_and_object_export_rechecks_effective_permission() {
        let Some((scratch, _state, workspace_id, owner_id)) = fixture("flow_export_auth").await else {
            eprintln!("SKIPPED (no database): set OPENPR_TEST_DATABASE_URL to run this test");
            return;
        };
        #[derive(FromQueryResult)]
        struct Page {
            id: Uuid,
        }
        let page = Page::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM flow_objects WHERE workspace_id=$1 AND object_type='page'",
            vec![workspace_id.into()],
        ))
        .one(&scratch.db)
        .await
        .unwrap()
        .unwrap();
        let workspace_request = CreateExportRequest {
            scope: ExportScope::Workspace {
                workspace_id,
                project_id: None,
            },
            include_history: false,
            idempotency_key: "denied-workspace".to_string(),
            source_head: "0123456789abcdef0123456789abcdef01234567".to_string(),
            principal: principal(owner_id, "member"),
        };
        assert!(matches!(
            create_package_export(&scratch.db, &workspace_request).await,
            Err(ApiError::Forbidden(_))
        ));

        let guest_request = CreateExportRequest {
            scope: ExportScope::Object(page.id),
            include_history: false,
            idempotency_key: "denied-object".to_string(),
            source_head: workspace_request.source_head,
            principal: principal(owner_id, "__flow_guest"),
        };
        assert!(matches!(
            create_package_export(&scratch.db, &guest_request).await,
            Err(ApiError::Forbidden(_))
        ));
        let count = scratch
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT COUNT(*)::bigint AS count FROM flow_export_jobs",
                vec![],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "count")
            .unwrap();
        assert_eq!(count, 0, "denied exports leave no job or artifact");

        scratch.drop_self().await;
    }
}
