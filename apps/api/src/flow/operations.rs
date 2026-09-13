//! Exact-scope v0.8 maintenance application services.

use std::collections::BTreeSet;

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::collab::{bootstrap, compaction, integrity, snapshot};
use super::event_origin::CommandOrigin;
use super::maintenance;
use crate::error::ApiError;
use crate::events::{BusinessEventInput, FlowDispatchSpec, insert_flow_event};

#[derive(Debug, Clone, FromQueryResult)]
pub struct DocumentScope {
    pub workspace_id: Uuid,
    pub object_id: Uuid,
    pub document_id: Uuid,
}

#[derive(Debug, Clone, Copy)]
pub struct Principal {
    pub id: Uuid,
    pub is_bot: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct OperationReceipt {
    pub operation_id: Uuid,
    pub operation: String,
    pub status: String,
    pub dry_run: bool,
    pub workspace_id: Uuid,
    pub object_id: Uuid,
    pub document_id: Uuid,
    pub expected_head_seq: i64,
    pub result: Value,
}

#[derive(Debug, Clone)]
pub enum RepairQuarantineScope {
    Workspace { workspace_id: Uuid },
    Document(DocumentScope),
}

impl RepairQuarantineScope {
    fn workspace_id(&self) -> Uuid {
        match self {
            Self::Workspace { workspace_id } => *workspace_id,
            Self::Document(scope) => scope.workspace_id,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Workspace { .. } => "workspace",
            Self::Document(_) => "document",
        }
    }

    fn id(&self) -> Uuid {
        match self {
            Self::Workspace { workspace_id } => *workspace_id,
            Self::Document(scope) => scope.document_id,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RepairQuarantineReceipt {
    pub operation_id: Option<Uuid>,
    pub event_id: Option<Uuid>,
    pub operation: &'static str,
    pub status: &'static str,
    pub dry_run: bool,
    pub workspace_id: Uuid,
    pub scope_kind: &'static str,
    pub scope_id: Uuid,
    pub affected: Value,
}

#[derive(Debug, FromQueryResult)]
struct RepairCandidate {
    id: Uuid,
    object_id: Option<Uuid>,
}

#[derive(Debug, FromQueryResult)]
struct PriorRun {
    id: Uuid,
    request_hash: Option<String>,
    status: String,
    result_redacted: Value,
    dry_run: bool,
    expected_head_seq: Option<i64>,
}

#[derive(Debug, FromQueryResult)]
struct InsertedRun {
    id: Uuid,
}

pub async fn document_scope(db: &DatabaseConnection, document_id: Uuid) -> Result<DocumentScope, ApiError> {
    DocumentScope::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT fo.workspace_id,fo.id AS object_id,cd.id AS document_id \
           FROM collab_documents cd JOIN flow_objects fo ON fo.id=cd.object_id WHERE cd.id=$1",
        vec![document_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("collab document not found".to_string()))
}

pub async fn object_scope(db: &DatabaseConnection, object_id: Uuid) -> Result<DocumentScope, ApiError> {
    DocumentScope::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT fo.workspace_id,fo.id AS object_id,cd.id AS document_id \
           FROM flow_objects fo JOIN collab_documents cd ON cd.object_id=fo.id WHERE fo.id=$1",
        vec![object_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))
}

fn request_hash(
    operation: &str,
    scope: &DocumentScope,
    dry_run: bool,
    expected_head_seq: i64,
    extra: &Value,
) -> String {
    let body = json!({
        "operation": operation,
        "workspace_id": scope.workspace_id,
        "object_id": scope.object_id,
        "document_id": scope.document_id,
        "dry_run": dry_run,
        "expected_head_seq": expected_head_seq,
        "extra": extra,
    });
    format!("{:x}", Sha256::digest(serde_json::to_vec(&body).unwrap_or_default()))
}

async fn claim(
    db: &DatabaseConnection,
    operation: &str,
    scope: &DocumentScope,
    dry_run: bool,
    expected_head_seq: i64,
    principal: Principal,
    idempotency_key: &str,
    extra: &Value,
) -> Result<Result<Uuid, OperationReceipt>, ApiError> {
    if idempotency_key.trim().is_empty() {
        return Err(ApiError::BadRequest("idempotency_key is required".to_string()));
    }
    let principal_kind = if principal.is_bot { "bot" } else { "user" };
    let hash = request_hash(operation, scope, dry_run, expected_head_seq, extra);
    let inserted = InsertedRun::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_operation_runs \
           (workspace_id,operation,scope_kind,scope_id,dry_run,expected_head_seq,status, \
            principal_kind,principal_id,idempotency_key,request_hash) \
         VALUES ($1,$2,'document',$3,$4,$5,'running',$6,$7,$8,$9) \
         ON CONFLICT (workspace_id,operation,principal_kind,principal_id,idempotency_key) \
           WHERE idempotency_key IS NOT NULL DO NOTHING RETURNING id",
        vec![
            scope.workspace_id.into(),
            operation.into(),
            scope.document_id.into(),
            dry_run.into(),
            expected_head_seq.into(),
            principal_kind.into(),
            principal.id.into(),
            idempotency_key.into(),
            hash.clone().into(),
        ],
    ))
    .one(db)
    .await?;
    if let Some(inserted) = inserted {
        return Ok(Ok(inserted.id));
    }
    let prior = PriorRun::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id,request_hash,status,result_redacted,dry_run,expected_head_seq \
           FROM flow_operation_runs WHERE workspace_id=$1 AND operation=$2 AND principal_kind=$3 \
             AND principal_id=$4 AND idempotency_key=$5",
        vec![
            scope.workspace_id.into(),
            operation.into(),
            principal_kind.into(),
            principal.id.into(),
            idempotency_key.into(),
        ],
    ))
    .one(db)
    .await?
    .ok_or(ApiError::Internal)?;
    if prior.request_hash.as_deref() != Some(hash.as_str()) {
        return Err(ApiError::Conflict("idempotency key body drift".to_string()));
    }
    if prior.status != "completed" {
        return Err(ApiError::Conflict(
            "identical maintenance operation is still running".to_string(),
        ));
    }
    Ok(Err(OperationReceipt {
        operation_id: prior.id,
        operation: operation.to_string(),
        status: prior.status,
        dry_run: prior.dry_run,
        workspace_id: scope.workspace_id,
        object_id: scope.object_id,
        document_id: scope.document_id,
        expected_head_seq: prior.expected_head_seq.ok_or(ApiError::Internal)?,
        result: prior.result_redacted,
    }))
}

async fn finish(
    db: &DatabaseConnection,
    id: Uuid,
    operation: &str,
    scope: &DocumentScope,
    dry_run: bool,
    expected_head_seq: i64,
    result: Value,
) -> Result<OperationReceipt, ApiError> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_operation_runs SET status='completed',result_redacted=$2,finished_at=now() WHERE id=$1",
        vec![id.into(), result.clone().into()],
    ))
    .await?;
    Ok(OperationReceipt {
        operation_id: id,
        operation: operation.to_string(),
        status: "completed".to_string(),
        dry_run,
        workspace_id: scope.workspace_id,
        object_id: scope.object_id,
        document_id: scope.document_id,
        expected_head_seq,
        result,
    })
}

async fn abandon(db: &DatabaseConnection, id: Uuid) {
    let _ = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM flow_operation_runs WHERE id=$1 AND status='running'",
            vec![id.into()],
        ))
        .await;
}

fn repair_request_hash(scope: &RepairQuarantineScope) -> String {
    let body = json!({
        "operation": "repair_quarantine",
        "scope_kind": scope.kind(),
        "scope_id": scope.id(),
        "confirm_quarantine": true,
    });
    format!("{:x}", Sha256::digest(serde_json::to_vec(&body).unwrap_or_default()))
}

async fn repair_candidates<C: ConnectionTrait>(
    conn: &C,
    scope: &RepairQuarantineScope,
    lock: bool,
) -> Result<Vec<RepairCandidate>, ApiError> {
    let (document_id, object_id) = match scope {
        RepairQuarantineScope::Workspace { .. } => (String::new(), String::new()),
        RepairQuarantineScope::Document(scope) => (scope.document_id.to_string(), scope.object_id.to_string()),
    };
    let lock_clause = if lock { " FOR UPDATE OF ir" } else { "" };
    RepairCandidate::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT ir.id,COALESCE(cd.object_id,fo.id) AS object_id \
               FROM flow_integrity_records ir \
               LEFT JOIN collab_documents cd ON ir.subject_kind='collab_document' AND ir.subject_id=cd.id::text \
               LEFT JOIN flow_objects fo ON ir.subject_kind='flow_object' AND ir.subject_id=fo.id::text \
              WHERE ir.workspace_id=$1 AND ir.status='open' \
                AND ($2='workspace' OR ($2='document' AND \
                     ((ir.subject_kind='collab_document' AND ir.subject_id=$3) OR \
                      (ir.subject_kind='flow_object' AND ir.subject_id=$4)))) \
              ORDER BY ir.id{lock_clause}"
        ),
        vec![
            scope.workspace_id().into(),
            scope.kind().into(),
            document_id.into(),
            object_id.into(),
        ],
    ))
    .all(conn)
    .await
    .map_err(Into::into)
}

fn repair_affected(scope: &RepairQuarantineScope, candidates: &[RepairCandidate]) -> Value {
    let integrity_record_ids: Vec<Uuid> = candidates.iter().map(|candidate| candidate.id).collect();
    let object_ids: BTreeSet<Uuid> = candidates.iter().filter_map(|candidate| candidate.object_id).collect();
    json!({
        "scope_kind": scope.kind(),
        "scope_id": scope.id(),
        "integrity_record_count": integrity_record_ids.len(),
        "integrity_record_ids": integrity_record_ids,
        "object_count": object_ids.len(),
        "affected_object_ids": object_ids,
    })
}

/// Plans or irreversibly quarantines all currently-open integrity findings in one explicit
/// workspace/document scope. A dry-run deliberately writes no operation claim: the same
/// idempotency key remains available to execute exactly the plan the caller just inspected.
pub async fn repair_quarantine(
    db: &DatabaseConnection,
    scope: RepairQuarantineScope,
    dry_run: bool,
    principal: Principal,
    idempotency_key: &str,
    origin: CommandOrigin,
) -> Result<RepairQuarantineReceipt, ApiError> {
    if idempotency_key.trim().is_empty() {
        return Err(ApiError::BadRequest("idempotency_key is required".to_string()));
    }
    if dry_run {
        let candidates = repair_candidates(db, &scope, false).await?;
        return Ok(RepairQuarantineReceipt {
            operation_id: None,
            event_id: None,
            operation: "repair_quarantine",
            status: "planned",
            dry_run: true,
            workspace_id: scope.workspace_id(),
            scope_kind: scope.kind(),
            scope_id: scope.id(),
            affected: repair_affected(&scope, &candidates),
        });
    }

    let principal_kind = if principal.is_bot { "bot" } else { "user" };
    let request_hash = repair_request_hash(&scope);
    let tx = db.begin().await?;
    let inserted = InsertedRun::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_operation_runs \
           (workspace_id,operation,scope_kind,scope_id,dry_run,status,actor_id,principal_kind,principal_id, \
            idempotency_key,request_hash) \
         VALUES ($1,'repair_quarantine',$2,$3,false,'running',$4,$5,$6,$7,$8) \
         ON CONFLICT (workspace_id,operation,principal_kind,principal_id,idempotency_key) \
           WHERE idempotency_key IS NOT NULL DO NOTHING RETURNING id",
        vec![
            scope.workspace_id().into(),
            scope.kind().into(),
            scope.id().into(),
            (if principal.is_bot { None } else { Some(principal.id) }).into(),
            principal_kind.into(),
            principal.id.into(),
            idempotency_key.into(),
            request_hash.clone().into(),
        ],
    ))
    .one(&tx)
    .await?;
    let Some(inserted) = inserted else {
        let prior = PriorRun::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id,request_hash,status,result_redacted,dry_run,expected_head_seq \
               FROM flow_operation_runs WHERE workspace_id=$1 AND operation='repair_quarantine' \
                 AND principal_kind=$2 AND principal_id=$3 AND idempotency_key=$4",
            vec![
                scope.workspace_id().into(),
                principal_kind.into(),
                principal.id.into(),
                idempotency_key.into(),
            ],
        ))
        .one(&tx)
        .await?
        .ok_or(ApiError::Internal)?;
        if prior.request_hash.as_deref() != Some(request_hash.as_str()) {
            return Err(ApiError::Conflict("idempotency key body drift".to_string()));
        }
        if prior.status != "completed" {
            return Err(ApiError::Conflict(
                "identical maintenance operation is still running".to_string(),
            ));
        }
        let event_id = prior
            .result_redacted
            .get("event_id")
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok());
        let affected = prior
            .result_redacted
            .get("affected")
            .cloned()
            .ok_or(ApiError::Internal)?;
        tx.commit().await?;
        return Ok(RepairQuarantineReceipt {
            operation_id: Some(prior.id),
            event_id,
            operation: "repair_quarantine",
            status: "completed",
            dry_run: false,
            workspace_id: scope.workspace_id(),
            scope_kind: scope.kind(),
            scope_id: scope.id(),
            affected,
        });
    };

    let candidates = repair_candidates(&tx, &scope, true).await?;
    let affected = repair_affected(&scope, &candidates);
    if !candidates.is_empty() {
        let ids = candidates
            .iter()
            .map(|candidate| candidate.id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_integrity_records SET status='ignored',resolved_at=now() \
             WHERE id=ANY(string_to_array($1,',')::uuid[]) AND status='open'",
            vec![ids.into()],
        ))
        .await?;
    }
    let affected_object_ids = affected["affected_object_ids"].clone();
    let event = insert_flow_event(
        &tx,
        BusinessEventInput {
            workspace_id: scope.workspace_id(),
            project_id: None,
            event_type: "flow.repair.completed".to_string(),
            aggregate_type: "flow_repair".to_string(),
            aggregate_id: inserted.id.to_string(),
            actor_id: if principal.is_bot { None } else { Some(principal.id) },
            source: origin.source_json(),
            payload: json!({
                "operation_id": inserted.id,
                "kind": "quarantine",
                "scope_kind": scope.kind(),
                "scope_id": scope.id(),
                "integrity_record_count": affected["integrity_record_count"],
            }),
            metadata: json!({"affected_object_ids": affected_object_ids}),
            correlation_id: Some(origin.correlation_id),
            causation_id: origin.causation_id,
            idempotency_key: Some(format!(
                "repair_quarantine:{principal_kind}:{}:{idempotency_key}",
                principal.id
            )),
        },
        Some(FlowDispatchSpec {
            max_attempts: crate::config::runtime().flow.dispatch_max_attempts,
            document_id: None,
            accepted_seq: None,
        }),
    )
    .await?;
    let stored = json!({"event_id": event.event_id, "affected": affected});
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_operation_runs SET status='completed',result_redacted=$2,finished_at=now() WHERE id=$1",
        vec![inserted.id.into(), stored.into()],
    ))
    .await?;
    tx.commit().await?;
    Ok(RepairQuarantineReceipt {
        operation_id: Some(inserted.id),
        event_id: Some(event.event_id),
        operation: "repair_quarantine",
        status: "completed",
        dry_run: false,
        workspace_id: scope.workspace_id(),
        scope_kind: scope.kind(),
        scope_id: scope.id(),
        affected,
    })
}

pub async fn compact_document(
    db: &DatabaseConnection,
    scope: &DocumentScope,
    dry_run: bool,
    expected_head_seq: i64,
    force_resync: bool,
    principal: Principal,
    idempotency_key: &str,
) -> Result<OperationReceipt, ApiError> {
    let extra = json!({"force_resync": force_resync});
    let id = match claim(
        db,
        "compact",
        scope,
        dry_run,
        expected_head_seq,
        principal,
        idempotency_key,
        &extra,
    )
    .await?
    {
        Ok(id) => id,
        Err(receipt) => return Ok(receipt),
    };
    let outcome = async {
        let boot = bootstrap::load(db, scope.document_id).await?;
        if boot.head_seq != expected_head_seq {
            return Err(ApiError::Conflict("stale_frontier".to_string()));
        }
        if dry_run {
            let stats = snapshot::read_tail_stats(db, scope.document_id)
                .await?
                .ok_or_else(|| ApiError::NotFound("collab document not found".to_string()))?;
            return Ok(json!({
                "head_seq": boot.head_seq,
                "head_frontier": base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &boot.head_frontier,
                ),
                "snapshot_seq": stats.snapshot_seq,
                "tail_updates": stats.tail_updates,
                "tail_bytes": stats.tail_bytes,
                "would_compact": stats.snapshot_seq < stats.head_seq,
            }));
        }
        let result = compaction::compact(db, scope.document_id, expected_head_seq, force_resync).await?;
        Ok(json!({
            "head_seq": result.head_seq,
            "head_frontier": base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                &result.head_frontier,
            ),
            "semantic_hash": result.semantic_hash,
            "snapshot_checksum": result.snapshot_checksum,
            "deleted_updates": result.deleted_updates,
            "lagging_clients": result.lagging_clients,
            "forced_resync_clients": result.forced_resync_clients,
        }))
    }
    .await;
    match outcome {
        Ok(result) => finish(db, id, "compact", scope, dry_run, expected_head_seq, result).await,
        Err(error) => {
            abandon(db, id).await;
            Err(error)
        }
    }
}

pub async fn rebuild_projection(
    db: &DatabaseConnection,
    scope: &DocumentScope,
    dry_run: bool,
    expected_head_seq: i64,
    principal: Principal,
    idempotency_key: &str,
) -> Result<OperationReceipt, ApiError> {
    let id = match claim(
        db,
        "rebuild_projection",
        scope,
        dry_run,
        expected_head_seq,
        principal,
        idempotency_key,
        &json!({}),
    )
    .await?
    {
        Ok(id) => id,
        Err(receipt) => return Ok(receipt),
    };
    match maintenance::rebuild_projection(db, scope.object_id, Some(expected_head_seq), !dry_run).await {
        Ok(result) => {
            finish(
                db,
                id,
                "rebuild_projection",
                scope,
                dry_run,
                expected_head_seq,
                serde_json::to_value(result).map_err(|_| ApiError::Internal)?,
            )
            .await
        }
        Err(error) => {
            abandon(db, id).await;
            Err(error)
        }
    }
}

pub async fn verify_document(
    db: &DatabaseConnection,
    scope: &DocumentScope,
    expected_head_seq: i64,
    deep: bool,
    principal: Principal,
    idempotency_key: &str,
) -> Result<OperationReceipt, ApiError> {
    let id = match claim(
        db,
        "verify_document",
        scope,
        true,
        expected_head_seq,
        principal,
        idempotency_key,
        &json!({"deep": deep}),
    )
    .await?
    {
        Ok(id) => id,
        Err(receipt) => return Ok(receipt),
    };
    match integrity::document_fingerprint(db, scope.document_id).await {
        Ok(fingerprint) if fingerprint.head_seq == expected_head_seq => {
            finish(
                db,
                id,
                "verify_document",
                scope,
                true,
                expected_head_seq,
                json!({"deep": deep, "fingerprint": fingerprint}),
            )
            .await
        }
        Ok(_) => {
            abandon(db, id).await;
            Err(ApiError::Conflict("stale_frontier".to_string()))
        }
        Err(error) => {
            abandon(db, id).await;
            Err(error)
        }
    }
}

#[derive(Debug, FromQueryResult)]
struct HealthRow {
    accept_rate: f64,
    reject_rate: f64,
    queue_depth: i64,
    oldest_job_age_seconds: Option<f64>,
    storage_bytes: i64,
    dispatch_failed: i64,
    delivery_failed: i64,
    oldest_failed_age_seconds: Option<f64>,
    delivery_cancelled: i64,
}

/// Content-free workspace health summary. Connection count is explicitly this API instance;
/// queue/dead-letter/storage values are durable cluster-wide facts.
pub async fn workspace_health(db: &DatabaseConnection, workspace_id: Uuid) -> Result<Value, ApiError> {
    let row = HealthRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT
              (SELECT count(*)::double precision/60.0 FROM business_events
                WHERE workspace_id=$1 AND event_type='flow.content.accepted'
                  AND created_at >= now()-interval '60 seconds') AS accept_rate,
              (SELECT count(*)::double precision/60.0 FROM business_events
                WHERE workspace_id=$1 AND event_type='flow.command.rejected'
                  AND created_at >= now()-interval '60 seconds') AS reject_rate,
              (SELECT count(*) FROM flow_operation_runs WHERE workspace_id=$1 AND status IN ('planned','running')) AS queue_depth,
              (SELECT EXTRACT(EPOCH FROM now()-min(created_at)) FROM flow_operation_runs
                WHERE workspace_id=$1 AND status IN ('planned','running')) AS oldest_job_age_seconds,
              COALESCE((SELECT sum(octet_length(cd.snapshot)) +
                               COALESCE(sum((SELECT COALESCE(sum(octet_length(cu.bytes)),0)
                                              FROM collab_updates cu WHERE cu.document_id=cd.id)),0)
                          FROM collab_documents cd JOIN flow_objects fo ON fo.id=cd.object_id
                         WHERE fo.workspace_id=$1),0)::bigint AS storage_bytes,
              (SELECT count(*) FROM event_dispatch WHERE workspace_id=$1 AND status='failed') AS dispatch_failed,
              (SELECT count(*) FROM event_deliveries WHERE workspace_id=$1 AND status='failed') AS delivery_failed,
              (SELECT EXTRACT(EPOCH FROM now()-min(terminated_at)) FROM event_deliveries
                WHERE workspace_id=$1 AND status='failed') AS oldest_failed_age_seconds,
              (SELECT count(*) FROM event_deliveries WHERE workspace_id=$1 AND status='cancelled') AS delivery_cancelled
        ",
        vec![workspace_id.into()],
    ))
    .one(db)
    .await?
    .ok_or(ApiError::Internal)?;
    let connections = super::collab::runtime::runtime()
        .registry
        .workspace_connection_count(workspace_id);
    Ok(json!({
        "status": if row.dispatch_failed > 0 || row.delivery_failed > 0 { "degraded" } else { "healthy" },
        "connections": connections,
        "accept_rate": row.accept_rate,
        "reject_rate": row.reject_rate,
        "queue_depth": row.queue_depth,
        "oldest_job_age": row.oldest_job_age_seconds,
        "storage_bytes": row.storage_bytes,
        "dead_letter": {
            "dispatch_failed": row.dispatch_failed,
            "delivery_failed": row.delivery_failed,
            "oldest_failed_age": row.oldest_failed_age_seconds,
        },
        "delivery_cancelled": row.delivery_cancelled,
    }))
}

#[derive(Debug, FromQueryResult)]
struct LagRow {
    projection_max: i64,
    projection_p95: i64,
    search_max: i64,
    search_p95: i64,
    fanout_max: i64,
    fanout_p95: i64,
}

pub async fn workspace_lag(db: &DatabaseConnection, workspace_id: Uuid) -> Result<Value, ApiError> {
    let row = LagRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
          WITH lag AS (
            SELECT cd.id,
                   GREATEST(cd.head_seq-COALESCE(fp.document_seq,0),0) AS projection_lag,
                   GREATEST(cd.head_seq-COALESCE(si.indexed_seq,0),0) AS search_lag,
                   GREATEST(cd.head_seq-COALESCE((SELECT max(fn.document_seq) FROM flow_fanout_notices fn
                                                  WHERE fn.document_id=cd.id AND fn.notice_kind='document_update'),0),0) AS fanout_lag
              FROM collab_documents cd
              JOIN flow_objects fo ON fo.id=cd.object_id
              LEFT JOIN flow_object_projections fp ON fp.object_id=cd.object_id
              LEFT JOIN flow_search_index si ON si.object_id=cd.object_id
             WHERE fo.workspace_id=$1
          )
          SELECT COALESCE(max(projection_lag),0)::bigint AS projection_max,
                 COALESCE(percentile_disc(0.95) WITHIN GROUP (ORDER BY projection_lag),0)::bigint AS projection_p95,
                 COALESCE(max(search_lag),0)::bigint AS search_max,
                 COALESCE(percentile_disc(0.95) WITHIN GROUP (ORDER BY search_lag),0)::bigint AS search_p95,
                 COALESCE(max(fanout_lag),0)::bigint AS fanout_max,
                 COALESCE(percentile_disc(0.95) WITHIN GROUP (ORDER BY fanout_lag),0)::bigint AS fanout_p95
            FROM lag
        ",
        vec![workspace_id.into()],
    ))
    .one(db)
    .await?
    .ok_or(ApiError::Internal)?;
    Ok(json!({
        "projection": {"max": row.projection_max, "p95": row.projection_p95, "items": []},
        "search": {"max": row.search_max, "p95": row.search_p95, "items": []},
        "fanout": {"max": row.fanout_max, "p95": row.fanout_p95, "items": []},
        "next_cursor": null,
    }))
}

#[derive(Debug, FromQueryResult)]
struct DocumentIdRow {
    id: Uuid,
}

pub async fn workspace_integrity(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    include_documents: bool,
    limit: u64,
) -> Result<Value, ApiError> {
    if limit == 0 || limit > 100 {
        return Err(ApiError::BadRequest("limit must be in 1..=100".to_string()));
    }
    let rows = DocumentIdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT cd.id FROM collab_documents cd JOIN flow_objects fo ON fo.id=cd.object_id \
          WHERE fo.workspace_id=$1 ORDER BY cd.id LIMIT $2",
        vec![workspace_id.into(), i64::try_from(limit).unwrap_or(100).into()],
    ))
    .all(db)
    .await?;
    let mut documents = Vec::with_capacity(rows.len());
    let mut failed = 0_u64;
    for row in rows {
        match integrity::document_fingerprint(db, row.id).await {
            Ok(fingerprint) => documents.push(serde_json::to_value(fingerprint).map_err(|_| ApiError::Internal)?),
            Err(_) => failed += 1,
        }
    }
    let checked = documents.len() as u64 + failed;
    Ok(json!({
        "status": if failed == 0 { "healthy" } else { "integrity_error" },
        "checked_at": chrono::Utc::now(),
        "counts": {"checked": checked, "healthy": documents.len(), "failed": failed},
        "documents": if include_documents { Value::Array(documents) } else { Value::Null },
    }))
}
