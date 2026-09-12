//! Cross-instance notification adapter for v0.8.
//!
//! Every notice is inserted in the same transaction as its already-durable document update.
//! `pg_notify` is only a wake-up hint; each API instance polls the durable cursor and reconstructs
//! frames from `collab_updates`. Duplicate, lost, or reordered notifications therefore cannot
//! allocate a seq or make a subscription skip one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use sea_orm::{ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, FromQueryResult, Statement};
use uuid::Uuid;

use super::bootstrap;
use super::frame::{Frame, PROTOCOL_VERSION};
use super::registry::SessionRegistry;
use crate::error::ApiError;

const POLL_INTERVAL: Duration = Duration::from_millis(50);
const POLL_BATCH: i64 = 256;
static LISTENER_STARTED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, FromQueryResult)]
struct NoticeId {
    id: i64,
}

#[derive(Debug, FromQueryResult)]
struct Notice {
    id: i64,
    document_id: Uuid,
    document_seq: Option<i64>,
}

/// Stages a pointer to one accepted update and emits a best-effort wakeup. Both statements are in
/// the caller's head-locked transaction; `PostgreSQL` delivers the notification only on commit.
pub async fn stage_document_update(
    tx: &DatabaseTransaction,
    workspace_id: Uuid,
    document_id: Uuid,
    document_seq: i64,
) -> Result<i64, ApiError> {
    let notice = NoticeId::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_fanout_notices \
             (workspace_id, document_id, notice_kind, document_seq) \
         VALUES ($1, $2, 'document_update', $3) RETURNING id",
        vec![workspace_id.into(), document_id.into(), document_seq.into()],
    ))
    .one(tx)
    .await?
    .ok_or(ApiError::Internal)?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_notify('openpr_flow_fanout', $1)",
        vec![notice.id.to_string().into()],
    ))
    .await?;
    Ok(notice.id)
}

async fn newest_notice_id(db: &DatabaseConnection) -> Result<i64, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        id: i64,
    }
    Ok(Row::find_by_statement(Statement::from_string(
        DbBackend::Postgres,
        "SELECT COALESCE(max(id), 0)::bigint AS id FROM flow_fanout_notices".to_string(),
    ))
    .one(db)
    .await?
    .map_or(0, |row| row.id))
}

async fn poll_after(db: &DatabaseConnection, cursor: i64) -> Result<Vec<Notice>, ApiError> {
    Ok(Notice::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, document_id, document_seq FROM flow_fanout_notices \
         WHERE id > $1 AND notice_kind = 'document_update' ORDER BY id ASC LIMIT $2",
        vec![cursor.into(), POLL_BATCH.into()],
    ))
    .all(db)
    .await?)
}

async fn relay(registry: &SessionRegistry, db: &DatabaseConnection, notice: &Notice) -> Result<(), ApiError> {
    let Some(seq) = notice.document_seq else {
        return Ok(());
    };
    // A transient reconstruction failure must leave the durable cursor behind this notice so the
    // next poll retries it. Advancing here would turn a recoverable database outage into a silent
    // permanent gap for every session connected to this instance.
    let rows = bootstrap::fetch_update_range(db, notice.document_id, seq, seq).await?;
    let Some(row) = rows.into_iter().next() else {
        registry.broadcast(
            notice.document_id,
            &Frame::Resync {
                protocol_version: PROTOCOL_VERSION,
                document_id: notice.document_id,
                reason: "fanout_history_unavailable".to_string(),
                minimum_snapshot_seq: Some(seq),
            },
            None,
        );
        return Ok(());
    };
    registry.broadcast(
        notice.document_id,
        &Frame::Update {
            protocol_version: PROTOCOL_VERSION,
            document_id: notice.document_id,
            update_id: row.update_id,
            base_frontier: BASE64.encode(row.before_frontier),
            bytes: BASE64.encode(row.bytes),
            idempotency_key: None,
            origin: row.origin_client_id.unwrap_or_default(),
            message: None,
        },
        None,
    );
    registry.broadcast(
        notice.document_id,
        &Frame::Accepted {
            protocol_version: PROTOCOL_VERSION,
            document_id: notice.document_id,
            update_id: row.update_id,
            head_seq: row.seq,
            head_frontier: BASE64.encode(row.after_frontier),
            projection_seq: row.projection_seq,
            event_id: row.event_id,
        },
        None,
    );
    Ok(())
}

async fn relay_batch(
    registry: &SessionRegistry,
    db: &DatabaseConnection,
    notices: Vec<Notice>,
    mut cursor: i64,
) -> i64 {
    for notice in notices {
        if let Err(error) = relay(registry, db, &notice).await {
            tracing::warn!(%error, notice_id = notice.id, "flow fanout reconstruction failed; retaining cursor for retry");
            break;
        }
        cursor = notice.id;
    }
    cursor
}

/// Starts the API-local durable cursor. Multiple calls in one process are a no-op; multiple API
/// processes each have their own cursor and relay into only their own session registry.
pub fn spawn_listener(db: DatabaseConnection, registry: &'static SessionRegistry) {
    if LISTENER_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    tokio::spawn(async move {
        let mut cursor = loop {
            match newest_notice_id(&db).await {
                Ok(id) => break id,
                Err(error) => tracing::warn!(%error, "flow fanout cursor initialization failed"),
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        };
        loop {
            match poll_after(&db, cursor).await {
                Ok(notices) => {
                    cursor = relay_batch(registry, &db, notices, cursor).await;
                }
                Err(error) => tracing::warn!(%error, cursor, "flow fanout durable poll failed"),
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use sea_orm::DatabaseConnection;
    use uuid::Uuid;

    use super::{Notice, relay_batch};
    use crate::flow::collab::registry::SessionRegistry;

    #[tokio::test]
    async fn transient_reconstruction_failure_retains_the_durable_cursor() {
        let cursor = relay_batch(
            &SessionRegistry::new(),
            &DatabaseConnection::Disconnected,
            vec![Notice {
                id: 42,
                document_id: Uuid::new_v4(),
                document_seq: Some(7),
            }],
            41,
        )
        .await;
        assert_eq!(cursor, 41, "a transient database failure must retry notice 42");
    }
}
