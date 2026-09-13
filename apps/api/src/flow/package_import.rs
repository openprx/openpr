//! Isolated upload, frozen preview, and atomic promotion for Flow package v1 imports.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Cursor, Read};

use base64::Engine as _;
use chrono::{DateTime, Duration, Utc};
use collab_core::{CollabEngine, LoroCollabEngine};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::ApiError;
use crate::events::{BusinessEventInput, FlowDispatchSpec, insert_flow_event};

use super::package::{ExportPackageManifest, VerifiedPackage, verify_package};
use super::{import::BoundedImportStager, projection, repository};

const IMPORT_ARTIFACT_TTL_MINUTES: i64 = 30;

#[cfg(test)]
static FAIL_PROMOTION_AFTER_OBJECT: parking_lot::Mutex<Option<Uuid>> = parking_lot::Mutex::new(None);

#[derive(Debug, Clone)]
pub struct ImportPrincipal {
    pub id: Uuid,
    pub kind: String,
    pub role: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalReferencePolicy {
    Reject,
    Detach,
}

impl ExternalReferencePolicy {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Reject => "reject",
            Self::Detach => "detach",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    RejectExisting,
    ReuseImportLineage,
}

impl ConflictPolicy {
    const fn as_str(self) -> &'static str {
        match self {
            Self::RejectExisting => "reject_existing",
            Self::ReuseImportLineage => "reuse_import_lineage",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PreviewImportRequest {
    pub workspace_id: Uuid,
    pub artifact_id: Uuid,
    pub project_map: BTreeMap<Uuid, Option<Uuid>>,
    pub external_reference_policy: ExternalReferencePolicy,
    pub conflict_policy: ConflictPolicy,
    pub include_history: bool,
    pub idempotency_key: String,
    pub principal: ImportPrincipal,
}

#[derive(Debug, Clone)]
pub struct CommitImportRequest {
    pub workspace_id: Uuid,
    pub preview_id: Uuid,
    pub package_sha256: String,
    pub mapping_hash: String,
    pub conflict_policy: ConflictPolicy,
    pub confirm: bool,
    pub idempotency_key: String,
    pub principal: ImportPrincipal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrozenImportMapping {
    pub workspace_map: BTreeMap<Uuid, Uuid>,
    pub object_map: BTreeMap<Uuid, Uuid>,
    pub document_map: BTreeMap<Uuid, Uuid>,
    pub relation_map: BTreeMap<Uuid, Uuid>,
    pub project_map: BTreeMap<Uuid, Option<Uuid>>,
    pub external_reference_policy: ExternalReferencePolicy,
    pub conflict_policy: ConflictPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPreviewReceipt {
    pub preview_id: Uuid,
    pub package_id: Uuid,
    pub package_sha256: String,
    pub mapping_hash: String,
    pub mapping: FrozenImportMapping,
    pub conflicts: Vec<String>,
    pub warnings: Vec<String>,
    pub estimated_changes: BTreeMap<String, u64>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportReport {
    pub import_id: Uuid,
    pub package_id: Uuid,
    pub package_sha256: String,
    pub source_workspace_id: Uuid,
    pub target_workspace_id: Uuid,
    pub status: String,
    pub mapping_hash: String,
    pub conflict_policy: String,
    pub counts: BTreeMap<String, u64>,
    pub object_mapping: BTreeMap<Uuid, Uuid>,
    pub document_mapping: BTreeMap<Uuid, Uuid>,
    pub detached_references: Vec<Uuid>,
    pub warnings: Vec<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub actor: Uuid,
    pub audit_event_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct PackageObject {
    source_object_id: Uuid,
    source_workspace_id: Uuid,
    object_type: String,
    lifecycle_status: String,
    project_id: Option<Uuid>,
    parent_object_id: Option<Uuid>,
    governance_metadata: Value,
    source_document_id: Uuid,
    engine: String,
    format_version: String,
    accepted_seq: i64,
    accepted_frontier: String,
    semantic_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct RelationEndpoint {
    kind: String,
    source_object_id: Uuid,
}

#[derive(Debug, Serialize, Deserialize)]
struct PackageRelation {
    source_relation_id: Uuid,
    relation_type: String,
    source: RelationEndpoint,
    target: RelationEndpoint,
    position_key: String,
    properties: Value,
}

struct PackagePlan {
    verified: VerifiedPackage,
    objects: Vec<PackageObject>,
    relations: Vec<PackageRelation>,
}

#[derive(Debug, FromQueryResult)]
struct ArtifactRow {
    actor_kind: String,
    actor_id: Uuid,
    package_sha256: String,
    package_bytes: Vec<u8>,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, FromQueryResult)]
struct PreviewRow {
    artifact_id: Uuid,
    actor_kind: String,
    actor_id: Uuid,
    package_sha256: String,
    mapping_hash: String,
    mapping: Value,
    request: Value,
    expires_at: DateTime<Utc>,
    committed_import_id: Option<Uuid>,
}

#[derive(Debug, FromQueryResult)]
struct ExistingPreviewRow {
    id: Uuid,
    package_sha256: String,
    mapping_hash: String,
    mapping: Value,
    conflicts: Value,
    warnings: Value,
    estimated_changes: Value,
    request_hash: String,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, FromQueryResult)]
struct ExistingArtifactRow {
    id: Uuid,
    package_sha256: String,
    request_hash: Option<String>,
    expires_at: DateTime<Utc>,
}

pub async fn upload_package_artifact(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    principal: &ImportPrincipal,
    bytes: Vec<u8>,
    expected_package_sha256: Option<&str>,
    idempotency_key: &str,
) -> Result<(Uuid, String, DateTime<Utc>), ApiError> {
    enforce_admin(principal)?;
    validate_key(idempotency_key)?;
    let verified = verify_package(Cursor::new(&bytes), expected_package_sha256)?;
    let request_hash = verified.package_sha256.clone();
    if let Some(existing) = ExistingArtifactRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id,package_sha256,request_hash,expires_at FROM flow_package_artifacts \
         WHERE workspace_id=$1 AND actor_kind=$2 AND actor_id=$3 AND purpose='import' AND idempotency_key=$4",
        vec![
            workspace_id.into(),
            principal.kind.clone().into(),
            principal.id.into(),
            idempotency_key.into(),
        ],
    ))
    .one(db)
    .await?
    {
        if existing.request_hash.as_deref() != Some(request_hash.as_str()) {
            return Err(ApiError::Conflict("artifact idempotency key body drift".to_string()));
        }
        return Ok((existing.id, existing.package_sha256, existing.expires_at));
    }
    let id = Uuid::new_v4();
    let expires_at = Utc::now() + Duration::minutes(IMPORT_ARTIFACT_TTL_MINUTES);
    let size = i64::try_from(bytes.len()).map_err(|_| ApiError::Internal)?;
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_package_artifacts \
         (id,workspace_id,actor_kind,actor_id,purpose,package_sha256,idempotency_key,request_hash,size_bytes,package_bytes,expires_at) \
         VALUES ($1,$2,$3,$4,'import',$5,$6,$7,$8,$9,$10)",
        vec![
            id.into(),
            workspace_id.into(),
            principal.kind.clone().into(),
            principal.id.into(),
            verified.package_sha256.clone().into(),
            idempotency_key.into(),
            request_hash.into(),
            size.into(),
            bytes.into(),
            expires_at.into(),
        ],
    ))
    .await?;
    Ok((id, verified.package_sha256, expires_at))
}

pub async fn upload_inline_base64_artifact(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    principal: &ImportPrincipal,
    encoded: &[u8],
    expected_package_sha256: Option<&str>,
    idempotency_key: &str,
) -> Result<(Uuid, String, DateTime<Utc>), ApiError> {
    let mut stager = BoundedImportStager::new(Vec::new(), std::io::sink());
    stager.stage_inline_base64_archive(Cursor::new(encoded))?;
    stager.finish_archive()?;
    let (bytes, _) = stager.into_stages()?;
    upload_package_artifact(
        db,
        workspace_id,
        principal,
        bytes,
        expected_package_sha256,
        idempotency_key,
    )
    .await
}

pub async fn get_import_report(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    import_id: Uuid,
    principal: &ImportPrincipal,
) -> Result<ImportReport, ApiError> {
    enforce_admin(principal)?;
    let job = repository::fetch_import_job(db, workspace_id, import_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("import not found".to_string()))?;
    job.report
        .ok_or_else(|| ApiError::Conflict("import has not completed".to_string()))
        .and_then(|value| serde_json::from_value(value).map_err(|_| ApiError::Internal))
}

pub async fn preview_package_import(
    db: &DatabaseConnection,
    request: &PreviewImportRequest,
) -> Result<ImportPreviewReceipt, ApiError> {
    validate_key(&request.idempotency_key)?;
    enforce_admin(&request.principal)?;
    let artifact = load_artifact(db, request.workspace_id, request.artifact_id, &request.principal).await?;
    let plan = parse_plan(&artifact.package_bytes, &artifact.package_sha256)?;
    if request.include_history != plan.verified.manifest.history.included {
        return Err(ApiError::BadRequest(
            "include_history must match the uploaded package manifest".to_string(),
        ));
    }
    validate_projects(db, request.workspace_id, &request.project_map, &plan.objects).await?;
    let request_json = json!({
        "artifact_id": request.artifact_id,
        "project_map": request.project_map,
        "external_reference_policy": request.external_reference_policy,
        "conflict_policy": request.conflict_policy,
        "include_history": request.include_history,
    });
    let request_hash = sha256_json(&request_json)?;
    if let Some(existing) = existing_preview(db, request, &request_hash).await? {
        return preview_receipt(existing, &plan.verified.manifest);
    }
    let prior_targets = lineage_targets(db, request.workspace_id, &plan).await?;
    let conflicts: Vec<String> = prior_targets
        .keys()
        .filter(|(kind, _)| kind == "object")
        .map(|(_, source_id)| source_id.to_string())
        .collect();
    if !conflicts.is_empty() && request.conflict_policy == ConflictPolicy::RejectExisting {
        return Err(ApiError::Conflict(
            "package objects already have import lineage".to_string(),
        ));
    }
    let mapping = freeze_mapping(request, &plan, &prior_targets)?;
    let mapping_json = serde_json::to_value(&mapping).map_err(|_| ApiError::Internal)?;
    let mapping_hash = sha256_json(&mapping_json)?;
    let preview_id = Uuid::new_v4();
    let expires_at = Utc::now() + Duration::minutes(IMPORT_ARTIFACT_TTL_MINUTES);
    let package_id = parse_manifest_uuid(&plan.verified.manifest)?;
    let estimated_changes = estimated_changes(&plan);
    let tx = db.begin().await?;
    lock_authorization_epoch(&tx, request.workspace_id).await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_import_previews \
         (id,workspace_id,artifact_id,actor_kind,actor_id,package_sha256,mapping_hash,mapping,request,manifest_summary,conflicts,warnings,estimated_changes,idempotency_key,request_hash,expires_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'[]'::jsonb,$12,$13,$14,$15)",
        vec![
            preview_id.into(), request.workspace_id.into(), request.artifact_id.into(),
            request.principal.kind.clone().into(), request.principal.id.into(), artifact.package_sha256.clone().into(),
            mapping_hash.clone().into(), mapping_json.into(), request_json.clone().into(),
            manifest_summary(&plan.verified.manifest).into(), json!(conflicts).into(), json!(estimated_changes).into(),
            request.idempotency_key.clone().into(), request_hash.into(), expires_at.into(),
        ],
    )).await?;
    insert_flow_event(
        &tx,
        import_event(
            request.workspace_id,
            preview_id,
            &request.principal,
            "flow.import.previewed",
            None,
        ),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(ImportPreviewReceipt {
        preview_id,
        package_id,
        package_sha256: artifact.package_sha256,
        mapping_hash,
        mapping,
        conflicts,
        warnings: Vec::new(),
        estimated_changes,
        expires_at,
    })
}

pub async fn commit_package_import(
    db: &DatabaseConnection,
    request: &CommitImportRequest,
) -> Result<ImportReport, ApiError> {
    if !request.confirm {
        return Err(ApiError::BadRequest("confirm=true is required".to_string()));
    }
    validate_key(&request.idempotency_key)?;
    enforce_admin(&request.principal)?;
    if let Some(existing) = repository::find_import_job_by_idempotency_key(
        db,
        request.workspace_id,
        "flow_package",
        &request.idempotency_key,
    )
    .await?
    {
        let expected = commit_request_hash(request)?;
        if existing.request_body_hash != expected {
            return Err(ApiError::Conflict("import idempotency key body drift".to_string()));
        }
        return existing
            .report
            .ok_or_else(|| ApiError::Conflict("import job has not completed".to_string()))
            .and_then(|value| serde_json::from_value(value).map_err(|_| ApiError::Internal));
    }

    let tx = db.begin().await?;
    lock_authorization_epoch(&tx, request.workspace_id).await?;
    let preview = load_preview(&tx, request).await?;
    if let Some(import_id) = preview.committed_import_id {
        let job = repository::fetch_import_job(&tx, request.workspace_id, import_id)
            .await?
            .ok_or(ApiError::Internal)?;
        tx.rollback().await?;
        return job
            .report
            .ok_or_else(|| ApiError::Conflict("import job has not completed".to_string()))
            .and_then(|value| serde_json::from_value(value).map_err(|_| ApiError::Internal));
    }
    let mapping: FrozenImportMapping = serde_json::from_value(preview.mapping.clone())
        .map_err(|_| ApiError::checksum_mismatch("frozen import mapping cannot be decoded"))?;
    let artifact = load_artifact_tx(&tx, request.workspace_id, preview.artifact_id, &request.principal).await?;
    let plan = parse_plan(&artifact.package_bytes, &request.package_sha256)?;
    validate_frozen_mapping(&mapping, &plan, request.workspace_id, request.conflict_policy)?;
    validate_projects(&tx, request.workspace_id, &mapping.project_map, &plan.objects).await?;

    let import_id = Uuid::new_v4();
    let started_at = Utc::now();
    let request_json = json!({
        "preview_id": request.preview_id,
        "package_sha256": request.package_sha256,
        "mapping_hash": request.mapping_hash,
        "conflict_policy": request.conflict_policy,
        "confirm": true,
    });
    repository::insert_import_job(
        &tx,
        &repository::NewImportJob {
            id: import_id,
            workspace_id: request.workspace_id,
            kind: "flow_package",
            source_workspace_id: Some(parse_source_workspace(&plan.verified.manifest)?),
            mapping_hash: &request.mapping_hash,
            package_sha256: Some(&request.package_sha256),
            artifact_id: Some(preview.artifact_id),
            conflict_policy: Some(request.conflict_policy.as_str()),
            external_reference_policy: Some(mapping.external_reference_policy.as_str()),
            request: request_json,
            idempotency_key: &request.idempotency_key,
            request_body_hash: &commit_request_hash(request)?,
            actor_user_id: actor_user_id(&request.principal),
        },
    )
    .await?;
    repository::start_import_job(&tx, import_id).await?;

    let (created, reused) = promote_objects(&tx, &artifact.package_bytes, &plan, &mapping, request, import_id).await?;
    let detached = promote_relations(&tx, &plan, &mapping, request, import_id).await?;
    let event = insert_flow_event(
        &tx,
        import_event(
            request.workspace_id,
            import_id,
            &request.principal,
            "flow.import.completed",
            Some(json!({
                "import_id": import_id, "package_id": plan.verified.manifest.package_id,
                "package_sha256": request.package_sha256, "created": created, "reused": reused,
            })),
        ),
        Some(FlowDispatchSpec {
            max_attempts: crate::config::runtime().flow.dispatch_max_attempts,
            document_id: None,
            accepted_seq: None,
        }),
    )
    .await?;
    let finished_at = Utc::now();
    let report = ImportReport {
        import_id,
        package_id: parse_manifest_uuid(&plan.verified.manifest)?,
        package_sha256: request.package_sha256.clone(),
        source_workspace_id: parse_source_workspace(&plan.verified.manifest)?,
        target_workspace_id: request.workspace_id,
        status: "completed".to_string(),
        mapping_hash: request.mapping_hash.clone(),
        conflict_policy: request.conflict_policy.as_str().to_string(),
        counts: BTreeMap::from([
            (
                "planned".to_string(),
                u64::try_from(plan.objects.len()).unwrap_or(u64::MAX),
            ),
            ("created".to_string(), created),
            ("reused".to_string(), reused),
            (
                "detached".to_string(),
                u64::try_from(detached.len()).unwrap_or(u64::MAX),
            ),
            ("failed".to_string(), 0),
        ]),
        object_mapping: mapping.object_map.clone(),
        document_mapping: mapping.document_map.clone(),
        detached_references: detached,
        warnings: Vec::new(),
        started_at,
        finished_at,
        actor: request.principal.id,
        audit_event_id: event.event_id,
    };
    let report_json = serde_json::to_value(&report).map_err(|_| ApiError::Internal)?;
    repository::finish_import_job(
        &tx,
        import_id,
        "completed",
        Some(report_json),
        None,
        Some(event.event_id),
    )
    .await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_import_previews SET committed_import_id=$2 WHERE id=$1 AND committed_import_id IS NULL",
        vec![request.preview_id.into(), import_id.into()],
    ))
    .await?;
    tx.commit().await?;
    Ok(report)
}

fn enforce_admin(principal: &ImportPrincipal) -> Result<(), ApiError> {
    if !matches!(principal.kind.as_str(), "user" | "bot") || !matches!(principal.role.as_str(), "owner" | "admin") {
        return Err(ApiError::Forbidden(
            "package import requires workspace admin".to_string(),
        ));
    }
    Ok(())
}

fn actor_user_id(principal: &ImportPrincipal) -> Option<Uuid> {
    (principal.kind == "user").then_some(principal.id)
}

fn validate_key(key: &str) -> Result<(), ApiError> {
    if key.trim().is_empty() || key.len() > 255 {
        return Err(ApiError::BadRequest(
            "idempotency_key must contain 1-255 characters".to_string(),
        ));
    }
    Ok(())
}

async fn lock_authorization_epoch<C: ConnectionTrait>(conn: &C, workspace_id: Uuid) -> Result<(), ApiError> {
    let row = conn
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT authz_epoch FROM flow_workspace_settings WHERE workspace_id=$1 AND flow_enabled FOR SHARE",
            vec![workspace_id.into()],
        ))
        .await?;
    if row.is_none() {
        return Err(ApiError::feature_disabled("Flow is disabled for this workspace"));
    }
    Ok(())
}

async fn load_artifact(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    artifact_id: Uuid,
    principal: &ImportPrincipal,
) -> Result<ArtifactRow, ApiError> {
    load_artifact_tx(db, workspace_id, artifact_id, principal).await
}

async fn load_artifact_tx<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    artifact_id: Uuid,
    principal: &ImportPrincipal,
) -> Result<ArtifactRow, ApiError> {
    let row = ArtifactRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT actor_kind,actor_id,package_sha256,package_bytes,expires_at \
         FROM flow_package_artifacts WHERE id=$1 AND workspace_id=$2 AND purpose='import'",
        vec![artifact_id.into(), workspace_id.into()],
    ))
    .one(conn)
    .await?
    .ok_or_else(|| ApiError::NotFound("import artifact not found".to_string()))?;
    if row.actor_kind != principal.kind || row.actor_id != principal.id || row.expires_at <= Utc::now() {
        return Err(ApiError::NotFound("import artifact not found".to_string()));
    }
    Ok(row)
}

async fn load_preview<C: ConnectionTrait>(conn: &C, request: &CommitImportRequest) -> Result<PreviewRow, ApiError> {
    let row = PreviewRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT artifact_id,actor_kind,actor_id,package_sha256,mapping_hash,mapping,request,expires_at,committed_import_id \
         FROM flow_import_previews WHERE id=$1 AND workspace_id=$2 FOR UPDATE",
        vec![request.preview_id.into(), request.workspace_id.into()],
    )).one(conn).await?.ok_or_else(|| ApiError::NotFound("import preview not found".to_string()))?;
    if row.actor_kind != request.principal.kind || row.actor_id != request.principal.id || row.expires_at <= Utc::now()
    {
        return Err(ApiError::NotFound("import preview not found".to_string()));
    }
    if row.package_sha256 != request.package_sha256 || row.mapping_hash != request.mapping_hash {
        return Err(ApiError::checksum_mismatch("commit does not match the frozen preview"));
    }
    if row.request.get("conflict_policy") != Some(&json!(request.conflict_policy)) {
        return Err(ApiError::Conflict(
            "commit conflict policy differs from preview".to_string(),
        ));
    }
    Ok(row)
}

async fn existing_preview(
    db: &DatabaseConnection,
    request: &PreviewImportRequest,
    request_hash: &str,
) -> Result<Option<ExistingPreviewRow>, ApiError> {
    let row = ExistingPreviewRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id,package_sha256,mapping_hash,mapping,conflicts,warnings,estimated_changes,request_hash,expires_at \
         FROM flow_import_previews WHERE workspace_id=$1 AND actor_kind=$2 AND actor_id=$3 AND idempotency_key=$4",
        vec![
            request.workspace_id.into(),
            request.principal.kind.clone().into(),
            request.principal.id.into(),
            request.idempotency_key.clone().into(),
        ],
    ))
    .one(db)
    .await?;
    if row
        .as_ref()
        .is_some_and(|existing| existing.request_hash != request_hash)
    {
        return Err(ApiError::Conflict("preview idempotency key body drift".to_string()));
    }
    Ok(row)
}

fn preview_receipt(
    row: ExistingPreviewRow,
    manifest: &ExportPackageManifest,
) -> Result<ImportPreviewReceipt, ApiError> {
    Ok(ImportPreviewReceipt {
        preview_id: row.id,
        package_id: parse_manifest_uuid(manifest)?,
        package_sha256: row.package_sha256,
        mapping_hash: row.mapping_hash,
        mapping: serde_json::from_value(row.mapping).map_err(|_| ApiError::Internal)?,
        conflicts: serde_json::from_value(row.conflicts).map_err(|_| ApiError::Internal)?,
        warnings: serde_json::from_value(row.warnings).map_err(|_| ApiError::Internal)?,
        estimated_changes: serde_json::from_value(row.estimated_changes).map_err(|_| ApiError::Internal)?,
        expires_at: row.expires_at,
    })
}

fn parse_plan(bytes: &[u8], expected_hash: &str) -> Result<PackagePlan, ApiError> {
    let verified = verify_package(Cursor::new(bytes), Some(expected_hash))?;
    let mut objects = Vec::new();
    for member in &verified.manifest.members {
        if member.kind == "object" {
            let object: PackageObject = serde_json::from_slice(&member_bytes(bytes, &member.path)?)
                .map_err(|_| ApiError::unsupported_format("object metadata is invalid"))?;
            let expected_path = format!("objects/{}/object.json", object.source_object_id);
            if member.path != expected_path
                || object.source_workspace_id.to_string() != verified.manifest.source.workspace_id
            {
                return Err(ApiError::unsupported_format(
                    "object metadata identity disagrees with its manifest path",
                ));
            }
            objects.push(object);
        }
    }
    let relations: Vec<PackageRelation> = parse_jsonl(&member_bytes(bytes, "relations/relations.jsonl")?)?;
    if objects.len() != usize::try_from(verified.manifest.counts.objects).unwrap_or(usize::MAX)
        || relations.len() != usize::try_from(verified.manifest.counts.relations).unwrap_or(usize::MAX)
    {
        return Err(ApiError::checksum_mismatch(
            "package semantic counts disagree with member content",
        ));
    }
    validate_plan_graph(&objects, &relations)?;
    Ok(PackagePlan {
        verified,
        objects,
        relations,
    })
}

fn parse_jsonl<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<Vec<T>, ApiError> {
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        return Err(ApiError::unsupported_format("JSONL member must end with LF"));
    }
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            let value: Value =
                serde_json::from_slice(line).map_err(|_| ApiError::unsupported_format("JSONL member is invalid"))?;
            if serde_jcs::to_vec(&value).map_err(|_| ApiError::unsupported_format("JSONL cannot be canonicalized"))?
                != line
            {
                return Err(ApiError::unsupported_format("JSONL line is not canonical JSON"));
            }
            serde_json::from_value(value).map_err(|_| ApiError::unsupported_format("JSONL row shape is invalid"))
        })
        .collect()
}

fn member_bytes(bytes: &[u8], path: &str) -> Result<Vec<u8>, ApiError> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| ApiError::unsupported_format("invalid ZIP archive"))?;
    let mut file = archive
        .by_name(path)
        .map_err(|_| ApiError::unsupported_format("required package member is missing"))?;
    let mut body = Vec::with_capacity(usize::try_from(file.size()).unwrap_or(0));
    file.read_to_end(&mut body)
        .map_err(|_| ApiError::unsupported_format("package member cannot be decoded"))?;
    Ok(body)
}

fn validate_plan_graph(objects: &[PackageObject], relations: &[PackageRelation]) -> Result<(), ApiError> {
    let object_ids: HashSet<_> = objects.iter().map(|object| object.source_object_id).collect();
    let document_ids: HashSet<_> = objects.iter().map(|object| object.source_document_id).collect();
    if object_ids.len() != objects.len() || document_ids.len() != objects.len() {
        return Err(ApiError::unsupported_format(
            "package object or document ids are duplicated",
        ));
    }
    for object in objects {
        if object
            .parent_object_id
            .is_some_and(|parent| !object_ids.contains(&parent))
        {
            return Err(ApiError::unsupported_format(
                "object parent is not present in the complete package",
            ));
        }
    }
    let mut relation_ids = HashSet::new();
    for relation in relations {
        if !relation_ids.insert(relation.source_relation_id)
            || !matches!(relation.source.kind.as_str(), "internal" | "external")
            || !matches!(relation.target.kind.as_str(), "internal" | "external")
            || (relation.source.kind == "internal" && !object_ids.contains(&relation.source.source_object_id))
            || (relation.target.kind == "internal" && !object_ids.contains(&relation.target.source_object_id))
        {
            return Err(ApiError::unsupported_format(
                "relation mapping is incomplete or inconsistent",
            ));
        }
    }
    Ok(())
}

fn freeze_mapping(
    request: &PreviewImportRequest,
    plan: &PackagePlan,
    prior_targets: &HashMap<(String, Uuid), Uuid>,
) -> Result<FrozenImportMapping, ApiError> {
    let source_workspace = parse_source_workspace(&plan.verified.manifest)?;
    Ok(FrozenImportMapping {
        workspace_map: BTreeMap::from([(source_workspace, request.workspace_id)]),
        object_map: plan
            .objects
            .iter()
            .map(|row| {
                let key = ("object".to_string(), row.source_object_id);
                (
                    row.source_object_id,
                    prior_targets.get(&key).copied().unwrap_or_else(Uuid::new_v4),
                )
            })
            .collect(),
        document_map: plan
            .objects
            .iter()
            .map(|row| {
                let key = ("document".to_string(), row.source_document_id);
                (
                    row.source_document_id,
                    prior_targets.get(&key).copied().unwrap_or_else(Uuid::new_v4),
                )
            })
            .collect(),
        relation_map: plan
            .relations
            .iter()
            .map(|row| {
                let key = ("relation".to_string(), row.source_relation_id);
                (
                    row.source_relation_id,
                    prior_targets.get(&key).copied().unwrap_or_else(Uuid::new_v4),
                )
            })
            .collect(),
        project_map: request.project_map.clone(),
        external_reference_policy: request.external_reference_policy,
        conflict_policy: request.conflict_policy,
    })
}

fn validate_frozen_mapping(
    mapping: &FrozenImportMapping,
    plan: &PackagePlan,
    workspace_id: Uuid,
    policy: ConflictPolicy,
) -> Result<(), ApiError> {
    let source_workspace = parse_source_workspace(&plan.verified.manifest)?;
    if mapping.workspace_map != BTreeMap::from([(source_workspace, workspace_id)])
        || mapping.conflict_policy != policy
        || mapping.object_map.len() != plan.objects.len()
        || mapping.document_map.len() != plan.objects.len()
        || mapping.relation_map.len() != plan.relations.len()
        || plan.objects.iter().any(|row| {
            !mapping.object_map.contains_key(&row.source_object_id)
                || !mapping.document_map.contains_key(&row.source_document_id)
        })
        || plan
            .relations
            .iter()
            .any(|row| !mapping.relation_map.contains_key(&row.source_relation_id))
    {
        return Err(ApiError::checksum_mismatch(
            "frozen import mapping is incomplete or changed",
        ));
    }
    let targets: HashSet<_> = mapping
        .object_map
        .values()
        .chain(mapping.document_map.values())
        .chain(mapping.relation_map.values())
        .collect();
    if targets.len() != mapping.object_map.len() + mapping.document_map.len() + mapping.relation_map.len() {
        return Err(ApiError::checksum_mismatch("frozen target ids are not globally unique"));
    }
    Ok(())
}

async fn validate_projects<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    project_map: &BTreeMap<Uuid, Option<Uuid>>,
    objects: &[PackageObject],
) -> Result<(), ApiError> {
    let source_projects: HashSet<_> = objects.iter().filter_map(|row| row.project_id).collect();
    if source_projects.iter().any(|id| !project_map.contains_key(id)) {
        return Err(ApiError::BadRequest(
            "project_map must cover every source project".to_string(),
        ));
    }
    for target in project_map.values().flatten() {
        let found = conn
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM projects WHERE id=$1 AND workspace_id=$2",
                vec![(*target).into(), workspace_id.into()],
            ))
            .await?;
        if found.is_none() {
            return Err(ApiError::BadRequest(
                "project_map target does not belong to the target workspace".to_string(),
            ));
        }
    }
    Ok(())
}

async fn lineage_targets<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    plan: &PackagePlan,
) -> Result<HashMap<(String, Uuid), Uuid>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        source_id: Uuid,
        target_kind: String,
        target_id: Uuid,
    }
    let source_ids: Vec<_> = plan
        .objects
        .iter()
        .flat_map(|row| [row.source_object_id, row.source_document_id])
        .chain(plan.relations.iter().map(|row| row.source_relation_id))
        .collect();
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT source_id,target_kind,target_id FROM flow_import_lineage WHERE target_workspace_id=$1 AND package_sha256=$2 AND source_kind='flow_package' AND source_id=ANY($3)",
        vec![workspace_id.into(), plan.verified.package_sha256.clone().into(), source_ids.into()],
    )).all(conn).await?;
    Ok(rows
        .into_iter()
        .map(|row| ((row.target_kind, row.source_id), row.target_id))
        .collect())
}

async fn promote_objects<C: ConnectionTrait>(
    tx: &C,
    package: &[u8],
    plan: &PackagePlan,
    mapping: &FrozenImportMapping,
    request: &CommitImportRequest,
    import_id: Uuid,
) -> Result<(u64, u64), ApiError> {
    let mut pending: HashMap<Uuid, &PackageObject> =
        plan.objects.iter().map(|row| (row.source_object_id, row)).collect();
    let mut inserted = HashSet::new();
    let mut created = 0u64;
    let mut reused = 0u64;
    while !pending.is_empty() {
        let ready: Vec<_> = pending
            .iter()
            .filter(|(_, row)| row.parent_object_id.is_none_or(|parent| inserted.contains(&parent)))
            .map(|(id, _)| *id)
            .collect();
        if ready.is_empty() {
            return Err(ApiError::unsupported_format("object parent graph contains a cycle"));
        }
        for source_id in ready {
            let object = pending.remove(&source_id).ok_or(ApiError::Internal)?;
            let target_object = mapping.object_map.get(&source_id).copied().ok_or(ApiError::Internal)?;
            let target_document = mapping
                .document_map
                .get(&object.source_document_id)
                .copied()
                .ok_or(ApiError::Internal)?;
            let snapshot = reconstruct_head(package, &plan.verified.manifest, object)?;
            let engine = LoroCollabEngine::load(&snapshot)
                .map_err(|_| ApiError::checksum_mismatch("import snapshot cannot be decoded"))?;
            let semantic = engine
                .semantic_snapshot()
                .map_err(|_| ApiError::checksum_mismatch("import semantic state cannot be decoded"))?;
            if semantic.semantic_hash() != object.semantic_hash
                || engine.frontier().as_bytes() != decode_frontier(&object.accepted_frontier)?
            {
                return Err(ApiError::checksum_mismatch(
                    "imported document head fingerprint does not match object metadata",
                ));
            }
            if mapping.conflict_policy == ConflictPolicy::ReuseImportLineage {
                #[derive(FromQueryResult)]
                struct ExistingHead {
                    head_seq: i64,
                    head_frontier: Vec<u8>,
                    snapshot: Vec<u8>,
                }
                if let Some(existing) = ExistingHead::find_by_statement(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT cd.head_seq,cd.head_frontier,cd.snapshot FROM flow_objects fo \
                     JOIN collab_documents cd ON cd.object_id=fo.id \
                     WHERE fo.workspace_id=$1 AND fo.id=$2 AND cd.id=$3",
                    vec![
                        request.workspace_id.into(),
                        target_object.into(),
                        target_document.into(),
                    ],
                ))
                .one(tx)
                .await?
                {
                    let existing_engine = LoroCollabEngine::load(&existing.snapshot).map_err(|_| {
                        ApiError::checksum_mismatch("reused import lineage points at an invalid document")
                    })?;
                    if existing.head_seq != object.accepted_seq
                        || existing.head_frontier != decode_frontier(&object.accepted_frontier)?
                        || existing_engine
                            .semantic_snapshot()
                            .map_err(|_| ApiError::Internal)?
                            .semantic_hash()
                            != object.semantic_hash
                    {
                        return Err(ApiError::Conflict(
                            "reused import lineage target has drifted".to_string(),
                        ));
                    }
                    inserted.insert(source_id);
                    reused = reused.saturating_add(1);
                    continue;
                }
            }
            let project_id = object
                .project_id
                .and_then(|id| mapping.project_map.get(&id).copied().flatten());
            let parent_id = if let Some(source_parent) = object.parent_object_id {
                Some(
                    mapping
                        .object_map
                        .get(&source_parent)
                        .copied()
                        .ok_or(ApiError::Internal)?,
                )
            } else {
                Some(repository::ensure_navigator_root(tx, request.workspace_id, project_id).await?)
            };
            let lifecycle = object.lifecycle_status.as_str();
            if !matches!(lifecycle, "active" | "archived") || object.engine != "loro" {
                return Err(ApiError::unsupported_format(
                    "object lifecycle or engine is unsupported",
                ));
            }
            let archived_at = (lifecycle == "archived").then_some(Utc::now());
            let governance = sanitize_governance(&object.governance_metadata)?;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO flow_objects \
                 (id,workspace_id,project_id,object_type,parent_id,governance_metadata,lifecycle_status,created_by,updated_by,archived_at) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$8,$9)",
                vec![target_object.into(),request.workspace_id.into(),project_id.into(),object.object_type.clone().into(),parent_id.into(),
                    governance.into(),lifecycle.into(),actor_user_id(&request.principal).into(),archived_at.into()],
            )).await?;
            let byte_count = i64::try_from(snapshot.len()).map_err(|_| ApiError::Internal)?;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO collab_documents \
                 (id,object_id,engine,format_version,snapshot,snapshot_frontier,snapshot_seq,head_frontier,head_seq,byte_count,update_count) \
                 VALUES ($1,$2,'loro',$3,$4,$5,$6,$5,$6,$7,0)",
                vec![target_document.into(),target_object.into(),object.format_version.clone().into(),snapshot.into(),
                    engine.frontier().as_bytes().to_vec().into(),object.accepted_seq.into(),byte_count.into()],
            )).await?;
            repository::insert_projection(
                tx,
                &repository::NewProjection {
                    object_id: target_object,
                    document_seq: object.accepted_seq,
                    document_frontier: engine.frontier().as_bytes().to_vec(),
                    title: engine.title().map_err(|_| ApiError::Internal)?,
                    state: projection::state_json(&semantic).map_err(|_| ApiError::Internal)?,
                    plain_text: projection::plain_text(&semantic),
                },
            )
            .await?;
            insert_package_lineage(
                tx,
                import_id,
                request.workspace_id,
                &plan.verified.package_sha256,
                "object",
                source_id,
                &object.semantic_hash,
                target_object,
            )
            .await?;
            insert_package_lineage(
                tx,
                import_id,
                request.workspace_id,
                &plan.verified.package_sha256,
                "document",
                object.source_document_id,
                &object.semantic_hash,
                target_document,
            )
            .await?;
            inserted.insert(source_id);
            created = created.saturating_add(1);
            #[cfg(test)]
            if FAIL_PROMOTION_AFTER_OBJECT.lock().as_ref() == Some(&request.preview_id) {
                return Err(ApiError::Internal);
            }
        }
    }
    Ok((created, reused))
}

fn reconstruct_head(
    package: &[u8],
    manifest: &ExportPackageManifest,
    object: &PackageObject,
) -> Result<Vec<u8>, ApiError> {
    let snapshot_path = format!("documents/{}/snapshot.bin", object.source_document_id);
    let snapshot = member_bytes(package, &snapshot_path)?;
    let mut engine = LoroCollabEngine::load(&snapshot)
        .map_err(|_| ApiError::checksum_mismatch("package snapshot cannot be decoded"))?;
    let prefix = format!("documents/{}/updates/", object.source_document_id);
    let mut updates: Vec<_> = manifest
        .members
        .iter()
        .filter(|member| member.path.starts_with(&prefix))
        .map(|member| {
            let seq = member
                .path
                .strip_prefix(&prefix)
                .and_then(|tail| tail.strip_suffix(".bin"))
                .and_then(|value| value.parse::<i64>().ok())
                .ok_or_else(|| ApiError::unsupported_format("update sequence path is invalid"))?;
            Ok((seq, member.path.as_str()))
        })
        .collect::<Result<_, ApiError>>()?;
    updates.sort_unstable_by_key(|(seq, _)| *seq);
    let first_seq = object
        .accepted_seq
        .saturating_sub(i64::try_from(updates.len()).unwrap_or(i64::MAX))
        .saturating_add(1);
    for (index, (seq, path)) in updates.into_iter().enumerate() {
        if seq != first_seq.saturating_add(i64::try_from(index).unwrap_or(i64::MAX)) {
            return Err(ApiError::checksum_mismatch("package update tail has a sequence gap"));
        }
        engine
            .import_update(&member_bytes(package, path)?)
            .map_err(|_| ApiError::checksum_mismatch("package update cannot be applied"))?;
    }
    engine
        .export_snapshot()
        .map_err(|_| ApiError::checksum_mismatch("package head cannot be compacted"))
}

async fn promote_relations<C: ConnectionTrait>(
    tx: &C,
    plan: &PackagePlan,
    mapping: &FrozenImportMapping,
    request: &CommitImportRequest,
    import_id: Uuid,
) -> Result<Vec<Uuid>, ApiError> {
    let mut detached = Vec::new();
    for relation in &plan.relations {
        let source = mapping.object_map.get(&relation.source.source_object_id).copied();
        let target = mapping.object_map.get(&relation.target.source_object_id).copied();
        let (Some(source), Some(target)) = (source, target) else {
            if mapping.external_reference_policy == ExternalReferencePolicy::Reject {
                return Err(ApiError::BadRequest(
                    "external relation reference rejected by import policy".to_string(),
                ));
            }
            detached.push(relation.source_relation_id);
            continue;
        };
        let relation_id = mapping
            .relation_map
            .get(&relation.source_relation_id)
            .copied()
            .ok_or(ApiError::Internal)?;
        if mapping.conflict_policy == ConflictPolicy::ReuseImportLineage {
            let existing = tx
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT id FROM flow_relations WHERE id=$1 AND workspace_id=$2 \
                     AND source_object_id=$3 AND target_object_id=$4 AND relation_type=$5",
                    vec![
                        relation_id.into(),
                        request.workspace_id.into(),
                        source.into(),
                        target.into(),
                        relation.relation_type.clone().into(),
                    ],
                ))
                .await?;
            if existing.is_some() {
                continue;
            }
        }
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_relations \
             (id,workspace_id,relation_type,source_object_id,target_object_id,position_key,properties,created_by) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
            vec![
                relation_id.into(),
                request.workspace_id.into(),
                relation.relation_type.clone().into(),
                source.into(),
                target.into(),
                relation.position_key.clone().into(),
                relation.properties.clone().into(),
                actor_user_id(&request.principal).into(),
            ],
        ))
        .await?;
        insert_package_lineage(
            tx,
            import_id,
            request.workspace_id,
            &plan.verified.package_sha256,
            "relation",
            relation.source_relation_id,
            &sha256_json(&serde_json::to_value(relation).map_err(|_| ApiError::Internal)?)?,
            relation_id,
        )
        .await?;
    }
    Ok(detached)
}

async fn insert_package_lineage<C: ConnectionTrait>(
    tx: &C,
    import_id: Uuid,
    workspace_id: Uuid,
    package_sha256: &str,
    target_kind: &str,
    source_id: Uuid,
    source_hash: &str,
    target_id: Uuid,
) -> Result<(), ApiError> {
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_import_lineage \
         (import_id,source_kind,source_id,target_workspace_id,source_content_hash,result,package_sha256,target_kind,target_id) \
         VALUES ($1,'flow_package',$2,$3,$4,'created',$5,$6,$7)",
        vec![import_id.into(),source_id.into(),workspace_id.into(),source_hash.into(),package_sha256.into(),
            target_kind.into(),target_id.into()],
    )).await?;
    Ok(())
}

fn sanitize_governance(value: &Value) -> Result<Value, ApiError> {
    let mut object = value
        .as_object()
        .cloned()
        .ok_or_else(|| ApiError::unsupported_format("governance metadata must be an object"))?;
    for key in [
        "system_role",
        "workspace_members",
        "grants",
        "tokens",
        "tickets",
        "presence",
    ] {
        object.remove(key);
    }
    Ok(Value::Object(object))
}

fn decode_frontier(encoded: &str) -> Result<Vec<u8>, ApiError> {
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| ApiError::unsupported_format("accepted frontier is not valid base64"))
}

fn parse_manifest_uuid(manifest: &ExportPackageManifest) -> Result<Uuid, ApiError> {
    Uuid::parse_str(&manifest.package_id).map_err(|_| ApiError::unsupported_format("package_id is not a UUID"))
}

fn parse_source_workspace(manifest: &ExportPackageManifest) -> Result<Uuid, ApiError> {
    Uuid::parse_str(&manifest.source.workspace_id)
        .map_err(|_| ApiError::unsupported_format("source workspace id is not a UUID"))
}

fn estimated_changes(plan: &PackagePlan) -> BTreeMap<String, u64> {
    BTreeMap::from([
        (
            "objects".to_string(),
            u64::try_from(plan.objects.len()).unwrap_or(u64::MAX),
        ),
        (
            "documents".to_string(),
            u64::try_from(plan.objects.len()).unwrap_or(u64::MAX),
        ),
        (
            "relations".to_string(),
            u64::try_from(plan.relations.len()).unwrap_or(u64::MAX),
        ),
    ])
}

fn manifest_summary(manifest: &ExportPackageManifest) -> Value {
    json!({"schema":manifest.schema,"package_id":manifest.package_id,"source":manifest.source,"counts":manifest.counts,"history":manifest.history})
}

fn sha256_json(value: &Value) -> Result<String, ApiError> {
    let bytes = serde_jcs::to_vec(value).map_err(|_| ApiError::Internal)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn commit_request_hash(request: &CommitImportRequest) -> Result<String, ApiError> {
    sha256_json(&json!({
        "preview_id":request.preview_id,"package_sha256":request.package_sha256,"mapping_hash":request.mapping_hash,
        "conflict_policy":request.conflict_policy,"confirm":request.confirm,
    }))
}

fn import_event(
    workspace_id: Uuid,
    aggregate_id: Uuid,
    principal: &ImportPrincipal,
    event_type: &str,
    payload: Option<Value>,
) -> BusinessEventInput {
    BusinessEventInput {
        workspace_id,
        project_id: None,
        event_type: event_type.to_string(),
        aggregate_type: "flow_import".to_string(),
        aggregate_id: aggregate_id.to_string(),
        actor_id: actor_user_id(principal),
        source: json!({"surface":"system","actor_kind":principal.kind}),
        payload: payload.unwrap_or_else(|| json!({"preview_id":aggregate_id})),
        metadata: json!({}),
        correlation_id: None,
        causation_id: None,
        idempotency_key: None,
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::items_after_statements,
    clippy::print_stderr,
    clippy::struct_field_names
)]
mod tests {
    use super::*;
    use crate::flow::command::{
        CreateObjectInput, ExecuteCommandInput, SetFlowFeatureInput, create_object, execute_command, set_flow_feature,
    };
    use crate::flow::event_origin::{CommandOrigin, EventSurface};
    use crate::flow::export::{CreateExportRequest, ExportPrincipal, ExportScope, create_package_export};
    use crate::routes::context::tenant_fixture::{exec, scratch, seed_tenant};
    use platform::{
        app::AppState,
        config::{AppConfig, Secret},
    };

    fn state_for(db: DatabaseConnection) -> AppState {
        AppState {
            cfg: AppConfig {
                app_name: "flow-package-import-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("flow-package-import-test-secret"),
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

    async fn enabled_workspace(state: &AppState, label: &str) -> (Uuid, Uuid) {
        let tenant = seed_tenant(&state.db, label).await;
        exec(
            &state.db,
            "UPDATE workspace_members SET role='owner' WHERE workspace_id=$1 AND user_id=$2",
            vec![tenant.workspace_id.into(), tenant.member_id.into()],
        )
        .await;
        set_flow_feature(
            state,
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
        .unwrap();
        (tenant.workspace_id, tenant.member_id)
    }

    async fn exported_fixture(state: &AppState, source_workspace: Uuid, source_owner: Uuid, label: &str) -> Vec<u8> {
        #[derive(FromQueryResult)]
        struct Root {
            id: Uuid,
        }
        let root = Root::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM flow_objects WHERE workspace_id=$1 AND object_type='navigator' AND parent_id IS NULL",
            vec![source_workspace.into()],
        ))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
        let page = create_object(
            state,
            CreateObjectInput {
                workspace_id: source_workspace,
                actor_id: source_owner,
                actor_is_bot: false,
                object_type: "page".to_string(),
                project_id: None,
                parent_object_id: Some(root.id),
                title: "portable state".to_string(),
                idempotency_key: format!("create-{label}"),
                message: None,
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            },
        )
        .await
        .unwrap()
        .object
        .id;
        execute_command(
            state,
            ExecuteCommandInput {
                object_id: page,
                actor_id: source_owner,
                principal_kind: "user".to_string(),
                role: "owner".to_string(),
                command_type: "set_title".to_string(),
                payload: json!({"title":"portable accepted head"}),
                expected_frontier: None,
                idempotency_key: format!("update-{label}"),
                message: None,
                origin_client_id: "package-import-test".to_string(),
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            },
        )
        .await
        .unwrap();
        let receipt = create_package_export(
            &state.db,
            &CreateExportRequest {
                scope: ExportScope::Workspace {
                    workspace_id: source_workspace,
                    project_id: None,
                },
                format: "package".to_string(),
                at_seq: None,
                include_history: true,
                idempotency_key: format!("export-{label}"),
                source_head: "0123456789abcdef0123456789abcdef01234567".to_string(),
                principal: ExportPrincipal {
                    id: source_owner,
                    kind: "user".to_string(),
                    role: "owner".to_string(),
                    workspace_export_capability: false,
                },
            },
        )
        .await
        .unwrap();
        #[derive(FromQueryResult)]
        struct Bytes {
            package_bytes: Vec<u8>,
        }
        Bytes::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT a.package_bytes FROM flow_export_jobs j JOIN flow_package_artifacts a ON a.id=j.artifact_id WHERE j.id=$1",
            vec![receipt.job_id.into()],
        ))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap()
        .package_bytes
    }

    fn principal(id: Uuid) -> ImportPrincipal {
        ImportPrincipal {
            id,
            kind: "user".to_string(),
            role: "owner".to_string(),
        }
    }

    async fn canonical_count(db: &DatabaseConnection, workspace_id: Uuid) -> i64 {
        db.query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COUNT(*)::bigint AS count FROM flow_objects WHERE workspace_id=$1",
            vec![workspace_id.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "count")
        .unwrap()
    }

    async fn prepare_preview(
        db: &DatabaseConnection,
        workspace_id: Uuid,
        owner_id: Uuid,
        package: Vec<u8>,
        label: &str,
    ) -> (ImportPreviewReceipt, CommitImportRequest) {
        let principal = principal(owner_id);
        let replay_package = package.clone();
        let artifact_key = format!("artifact-{label}");
        let (artifact_id, package_sha256, _) =
            upload_package_artifact(db, workspace_id, &principal, package, None, &artifact_key)
                .await
                .unwrap();
        let replay = upload_package_artifact(
            db,
            workspace_id,
            &principal,
            replay_package,
            Some(&package_sha256),
            &artifact_key,
        )
        .await
        .unwrap();
        assert_eq!(replay.0, artifact_id);
        assert_eq!(replay.1, package_sha256);
        let preview = preview_package_import(
            db,
            &PreviewImportRequest {
                workspace_id,
                artifact_id,
                project_map: BTreeMap::new(),
                external_reference_policy: ExternalReferencePolicy::Detach,
                conflict_policy: ConflictPolicy::RejectExisting,
                include_history: true,
                idempotency_key: format!("preview-{label}"),
                principal: principal.clone(),
            },
        )
        .await
        .unwrap();
        let commit = CommitImportRequest {
            workspace_id,
            preview_id: preview.preview_id,
            package_sha256,
            mapping_hash: preview.mapping_hash.clone(),
            conflict_policy: ConflictPolicy::RejectExisting,
            confirm: true,
            idempotency_key: format!("commit-{label}"),
            principal,
        };
        (preview, commit)
    }

    #[tokio::test]
    async fn flow_package_import_preview_writes_no_canonical_state_and_commit_remaps_exact_document_heads() {
        let Some(scratch) = scratch("flow_package_import_roundtrip").await else {
            eprintln!("SKIPPED (no database): set OPENPR_TEST_DATABASE_URL to run this test");
            return;
        };
        let state = state_for(scratch.db.clone());
        let (source_workspace, source_owner) = enabled_workspace(&state, "package_source").await;
        let (target_workspace, target_owner) = enabled_workspace(&state, "package_target").await;
        let package = exported_fixture(&state, source_workspace, source_owner, "roundtrip").await;
        let reuse_package = package.clone();
        let before = canonical_count(&state.db, target_workspace).await;
        let (preview, commit) = prepare_preview(&state.db, target_workspace, target_owner, package, "roundtrip").await;
        assert_eq!(canonical_count(&state.db, target_workspace).await, before);
        assert_ne!(
            preview.mapping.object_map.keys().collect::<Vec<_>>(),
            preview.mapping.object_map.values().collect::<Vec<_>>()
        );
        let report = commit_package_import(&state.db, &commit).await.unwrap();
        assert_eq!(report.counts["created"], 1);
        assert_eq!(canonical_count(&state.db, target_workspace).await, before + 1);
        let dispatch = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT \
                   COUNT(*) FILTER (WHERE be.event_type='flow.import.completed')::bigint AS completed, \
                   COUNT(*) FILTER (WHERE be.event_type='flow.import.previewed')::bigint AS previewed \
                 FROM event_dispatch ed JOIN business_events be ON be.id=ed.event_id WHERE be.workspace_id=$1",
                vec![target_workspace.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(dispatch.try_get::<i64>("", "completed").unwrap(), 1);
        assert_eq!(dispatch.try_get::<i64>("", "previewed").unwrap(), 0);
        let replay = commit_package_import(&state.db, &commit).await.unwrap();
        assert_eq!(replay.import_id, report.import_id);
        assert_eq!(replay.audit_event_id, report.audit_event_id);

        let target_principal = principal(target_owner);
        let (reuse_artifact, reuse_sha, _) = upload_package_artifact(
            &state.db,
            target_workspace,
            &target_principal,
            reuse_package,
            None,
            "artifact-reuse",
        )
        .await
        .unwrap();
        let reuse_preview = preview_package_import(
            &state.db,
            &PreviewImportRequest {
                workspace_id: target_workspace,
                artifact_id: reuse_artifact,
                project_map: BTreeMap::new(),
                external_reference_policy: ExternalReferencePolicy::Detach,
                conflict_policy: ConflictPolicy::ReuseImportLineage,
                include_history: true,
                idempotency_key: "preview-reuse".to_string(),
                principal: target_principal.clone(),
            },
        )
        .await
        .unwrap();
        assert_eq!(reuse_preview.mapping.object_map, report.object_mapping);
        let reuse_report = commit_package_import(
            &state.db,
            &CommitImportRequest {
                workspace_id: target_workspace,
                preview_id: reuse_preview.preview_id,
                package_sha256: reuse_sha,
                mapping_hash: reuse_preview.mapping_hash,
                conflict_policy: ConflictPolicy::ReuseImportLineage,
                confirm: true,
                idempotency_key: "commit-reuse".to_string(),
                principal: target_principal,
            },
        )
        .await
        .unwrap();
        assert_eq!(reuse_report.counts["created"], 0);
        assert_eq!(reuse_report.counts["reused"], 1);
        assert_eq!(canonical_count(&state.db, target_workspace).await, before + 1);

        #[derive(FromQueryResult)]
        struct Head {
            document_id: Uuid,
            head_seq: i64,
            head_frontier: Vec<u8>,
            snapshot: Vec<u8>,
        }
        for (source_object, target_object) in &report.object_mapping {
            let source = Head::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT cd.id AS document_id,cd.head_seq,cd.head_frontier,cd.snapshot FROM collab_documents cd JOIN flow_objects fo ON fo.id=cd.object_id WHERE fo.id=$1",
                vec![(*source_object).into()],
            )).one(&state.db).await.unwrap().unwrap();
            let target = Head::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT cd.id AS document_id,cd.head_seq,cd.head_frontier,cd.snapshot FROM collab_documents cd JOIN flow_objects fo ON fo.id=cd.object_id WHERE fo.id=$1",
                vec![(*target_object).into()],
            )).one(&state.db).await.unwrap().unwrap();
            assert_eq!(target.head_seq, source.head_seq);
            assert_eq!(target.head_frontier, source.head_frontier);
            #[derive(FromQueryResult)]
            struct Update {
                bytes: Vec<u8>,
            }
            let updates = Update::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT bytes FROM collab_updates WHERE document_id=$1 ORDER BY seq",
                vec![source.document_id.into()],
            ))
            .all(&state.db)
            .await
            .unwrap();
            let mut source_engine = LoroCollabEngine::load(&source.snapshot).unwrap();
            for update in updates {
                source_engine.import_update(&update.bytes).unwrap();
            }
            let source_hash = source_engine.semantic_snapshot().unwrap().semantic_hash();
            let target_hash = LoroCollabEngine::load(&target.snapshot)
                .unwrap()
                .semantic_snapshot()
                .unwrap()
                .semantic_hash();
            assert_eq!(target_hash, source_hash);
        }
        let mut drift = commit.clone();
        drift.mapping_hash = "0".repeat(64);
        assert!(matches!(
            commit_package_import(&state.db, &drift).await,
            Err(ApiError::Conflict(_))
        ));
        assert!(matches!(
            upload_package_artifact(
                &state.db,
                target_workspace,
                &ImportPrincipal {
                    role: "member".to_string(),
                    ..principal(target_owner)
                },
                Vec::new(),
                None,
                "artifact-denied",
            )
            .await,
            Err(ApiError::Forbidden(_))
        ));
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_package_import_promotion_fault_rolls_back_every_canonical_row_and_completion_event() {
        let Some(scratch) = scratch("flow_package_import_rollback").await else {
            eprintln!("SKIPPED (no database): set OPENPR_TEST_DATABASE_URL to run this test");
            return;
        };
        let state = state_for(scratch.db.clone());
        let (source_workspace, source_owner) = enabled_workspace(&state, "rollback_source").await;
        let (target_workspace, target_owner) = enabled_workspace(&state, "rollback_target").await;
        let package = exported_fixture(&state, source_workspace, source_owner, "rollback").await;
        let before = canonical_count(&state.db, target_workspace).await;
        let (_, commit) = prepare_preview(&state.db, target_workspace, target_owner, package, "rollback").await;
        *FAIL_PROMOTION_AFTER_OBJECT.lock() = Some(commit.preview_id);
        let result = commit_package_import(&state.db, &commit).await;
        *FAIL_PROMOTION_AFTER_OBJECT.lock() = None;
        assert!(matches!(result, Err(ApiError::Internal)));
        assert_eq!(canonical_count(&state.db, target_workspace).await, before);
        let rows = state.db.query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT \
               (SELECT COUNT(*) FROM flow_import_jobs WHERE workspace_id=$1 AND kind='flow_package')::bigint AS jobs, \
               (SELECT COUNT(*) FROM flow_import_lineage WHERE target_workspace_id=$1 AND source_kind='flow_package')::bigint AS lineage, \
               (SELECT COUNT(*) FROM business_events WHERE workspace_id=$1 AND event_type='flow.import.completed')::bigint AS completed",
            vec![target_workspace.into()],
        )).await.unwrap().unwrap();
        assert_eq!(rows.try_get::<i64>("", "jobs").unwrap(), 0);
        assert_eq!(rows.try_get::<i64>("", "lineage").unwrap(), 0);
        assert_eq!(rows.try_get::<i64>("", "completed").unwrap(), 0);
        scratch.drop_self().await;
    }
}
