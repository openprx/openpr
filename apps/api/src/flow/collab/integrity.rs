//! Canonical per-document fingerprints for verify, rebuild, and restore drills.

use base64::Engine as _;
use collab_core::{CollabEngine, LoroCollabEngine};
use sea_orm::{DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use uuid::Uuid;

use super::bootstrap;
use crate::error::ApiError;

/// State that must compare exactly before and after a storage/rebuild operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DocumentFingerprint {
    pub workspace_id: Uuid,
    pub object_id: Uuid,
    pub document_id: Uuid,
    pub head_seq: i64,
    pub head_frontier: String,
    pub semantic_hash: String,
    pub projection_seq: i64,
}

#[derive(FromQueryResult)]
struct IdentityAndProjection {
    workspace_id: Uuid,
    object_id: Uuid,
    projection_seq: i64,
}

/// Rebuilds one accepted document through the shared consistency loader and returns its exact
/// logical fingerprint.
///
/// A missing projection or any checksum/frontier/seq/decode mismatch fails closed; callers cannot
/// compare a partial row and call a restore healthy.
pub async fn document_fingerprint(db: &DatabaseConnection, document_id: Uuid) -> Result<DocumentFingerprint, ApiError> {
    let boot = bootstrap::load(db, document_id).await?;
    let mut engine = LoroCollabEngine::load(&boot.snapshot).map_err(|error| {
        tracing::error!(%error, %document_id, "document fingerprint snapshot decode failed");
        ApiError::Internal
    })?;
    for update in &boot.tail_updates {
        engine.import_update(&update.bytes).map_err(|error| {
            tracing::error!(%error, %document_id, seq = update.seq, "document fingerprint tail replay failed");
            ApiError::Internal
        })?;
    }
    if engine.frontier().as_bytes() != boot.head_frontier.as_slice() {
        tracing::error!(%document_id, "document fingerprint replay frontier mismatch");
        return Err(ApiError::Conflict("resync_required".to_string()));
    }
    let semantic_hash = engine
        .semantic_snapshot()
        .map_err(|error| {
            tracing::error!(%error, %document_id, "document fingerprint semantic snapshot failed");
            ApiError::Internal
        })?
        .semantic_hash();

    let identity = IdentityAndProjection::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT fo.workspace_id, fo.id AS object_id, fp.document_seq AS projection_seq \
         FROM collab_documents cd \
         JOIN flow_objects fo ON fo.id = cd.object_id \
         JOIN flow_object_projections fp ON fp.object_id = fo.id \
         WHERE cd.id = $1",
        vec![document_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::Conflict("document projection missing during integrity verification".to_string()))?;

    Ok(DocumentFingerprint {
        workspace_id: identity.workspace_id,
        object_id: identity.object_id,
        document_id,
        head_seq: boot.head_seq,
        head_frontier: base64::engine::general_purpose::STANDARD.encode(boot.head_frontier),
        semantic_hash,
        projection_seq: identity.projection_seq,
    })
}

/// Enumerates every canonical collaboration document in stable id order and requires every one
/// to yield a complete fingerprint.
pub async fn all_document_fingerprints(db: &DatabaseConnection) -> Result<Vec<DocumentFingerprint>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        id: Uuid,
    }
    let rows = Row::find_by_statement(Statement::from_string(
        DbBackend::Postgres,
        "SELECT id FROM collab_documents ORDER BY id".to_string(),
    ))
    .all(db)
    .await?;
    let mut fingerprints = Vec::with_capacity(rows.len());
    for row in rows {
        fingerprints.push(document_fingerprint(db, row.id).await?);
    }
    Ok(fingerprints)
}
