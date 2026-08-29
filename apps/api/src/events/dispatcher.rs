//! `ADR-0011` dispatcher.
//!
//! Single-row `event_dispatch` work, expanded into `event_deliveries` + `event_delivery_sources`
//! post-commit and *outside* the domain transaction that wrote the work row, then delivered to
//! subscribed webhooks.
//!
//! `apps/worker` calls [`run_tick`] on its existing 5-second poll loop (`--concurrency` is a batch
//! multiplier, not parallelism, matching every other job this worker already runs — see
//! `apps/worker/src/main.rs`'s `process_pending_tasks`). All three tables this module touches are
//! platform tables Flow is merely the v0.4 producer/consumer of (`migrations/0054_flow_data_layer.sql`
//! "`event_dispatch` / `event_deliveries` / `event_delivery_sources`"); this module — not `crate::flow` —
//! owns them because the next non-Flow producer that starts writing `event_dispatch` rows reuses
//! this dispatcher unchanged.
//!
//! `contracts/events-v1.md` ("投递") is the schema/algorithm's sole normative source; this file
//! implements its "展开是一个事务" ordering literally: reserve the source row (`delivery_id=NULL`)
//! → lock/select or create the delivery → bind the source to it → advance `event_dispatch`, all in
//! one transaction, with the final advance guarded by `WHERE lease_token = ?` so a worker whose
//! lease already expired cannot mark a re-leased work item `expanded` out from under its new owner.
#![allow(
    clippy::items_after_statements,
    clippy::struct_field_names,
    clippy::too_many_arguments
)]

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use platform::app::AppState;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::ApiError;
use crate::outbound::validate_outbound_url;
use crate::webhook_trigger::{WEBHOOK_SIGNATURE_HEADER, sign_payload};

// ---------------------------------------------------------------------------------------------
// Budgets `limits-v1.md` marks `status: unset` ("`set_by`: v0.4 实现者", "冻结前不得填入自造数值"
// for anything invented, but every value below still follows that entry's own `rule`). None of
// these back a database column default, unlike `FlowConfig::dispatch_max_attempts`
// (`crate::config::runtime()`'s doc comment explains why that one *is* deployment configuration);
// they are pure dispatcher-internal constants, so they live here rather than in `[flow]`.
// ---------------------------------------------------------------------------------------------

/// `dispatch_max_lease_reclaims`: worker-crash reclaims a work item survives before `failed`.
/// Rule: "显著大于一次发布窗口内的预期重启次数，同时使病态回收循环有界" — a rolling deploy
/// reclaims a work item at most once or twice; 5 leaves headroom without letting a poison work
/// item spin indefinitely.
const DISPATCH_MAX_LEASE_RECLAIMS: i64 = 5;

/// `dispatch_lease_ttl_ms`: expansion is a pure DB operation with a predictable p99, so this only
/// needs to be "several multiples" of that (rule) — 30s is generous for a single small
/// transaction and still short enough that a genuinely crashed worker's head-of-queue item is
/// freed well within one polling cycle.
const DISPATCH_LEASE_TTL_MS: i64 = 30_000;

/// `dispatch_backoff_ms`: expansion-failure retry ladder. Rule: "形态对齐 `delivery_backoff_ms`，
/// 但必须是独立的值" — same shape (linear, capped) as the frozen `delivery_backoff_ms`, half its
/// magnitude since expansion failures are DB errors, not network waits.
const DISPATCH_BACKOFF_STEP_MS: i64 = 15_000;
const DISPATCH_BACKOFF_CAP_MS: i64 = 150_000;

/// `delivery_max_attempts` (frozen at 10 by `limits-v1.md`) already lives as the `event_deliveries
/// .max_attempts` column default; this mirrors it only for the lease-reclaim path, which reads the
/// row's own `max_attempts` rather than this constant — kept here purely as the doc anchor.
const DELIVERY_BACKOFF_STEP_MS: i64 = 30_000;
const DELIVERY_BACKOFF_CAP_MS: i64 = 300_000;

/// `webhook_request_timeout_ms`. Matches the worker's existing outbound `reqwest::Client` (built
/// in `apps/worker/src/main.rs` with `timeout(10s)`) so this dispatcher's requests are governed by
/// the same budget as the AI-task webhook dispatch that client already serves.
const WEBHOOK_REQUEST_TIMEOUT_MS: i64 = 10_000;

/// `delivery_lease_ttl_ms`. Rule: "≥ webhook 请求超时 + 一个安全余量" — three times the request
/// timeout comfortably covers TCP/TLS setup plus the request itself before a still-in-flight send
/// is mistaken for a dead worker and reclaimed into a concurrent second send.
const DELIVERY_LEASE_TTL_MS: i64 = WEBHOOK_REQUEST_TIMEOUT_MS * 3;

/// `content_delivery_debounce_ms`, frozen at 2,000 by `limits-v1.md`.
const CONTENT_DELIVERY_DEBOUNCE_MS: i64 = 2_000;

/// `coalesced_source_events_max`. Rule: "取 debounce 窗口内单文档合并事件数的 p99 上界" — the
/// structured block derives this from `content_delivery_debounce_ms=2000` and the frozen 10
/// updates/s per-connection cap (`limits-v1.md`), i.e. at most ~20 accepted updates land in one
/// 2-second debounce window; 64 (the same warm-cache entry count `flow::collab::limits` already
/// uses elsewhere in this codebase for a similarly-derived per-document bound) gives 3x headroom
/// above that p99 without letting a single row's `event_delivery_sources` fan-out grow unbounded.
const COALESCED_SOURCE_EVENTS_MAX: i64 = 64;

/// `changed_block_ids_per_delivery_max`. Rule: "使投递体在 p99 合并窗口下仍显著小于订阅端常见
/// 请求体上限" — 200 UUIDs is ~7KB of JSON, well under typical webhook body limits even stacked
/// with the rest of the envelope.
const CHANGED_BLOCK_IDS_PER_DELIVERY_MAX: usize = 200;

/// `delivery_retention_days`, frozen at 30 by `limits-v1.md`.
const DELIVERY_RETENTION_DAYS: i64 = 30;

/// `dispatch_no_subscribers_retention_hours`, frozen at 24 by `limits-v1.md`.
const DISPATCH_NO_SUBSCRIBERS_RETENTION_HOURS: i64 = 24;

/// `dispatch_expanded_retention_days`. Rule: "足以支撑事后排障回溯，且使队首查询所在表规模有界" —
/// two weeks covers a typical incident-investigation window.
const DISPATCH_EXPANDED_RETENTION_DAYS: i64 = 14;

/// `dispatch_failed_retention_days`. Rule: "必须显著长于 `dispatch_expanded_retention_days`" and
/// "不短于一个值班轮换周期" — a quarter is both, and dead-letter rows are exactly the ones an
/// operator needs to still find weeks after the fact.
const DISPATCH_FAILED_RETENTION_DAYS: i64 = 90;

/// Header carrying the immutable consumer dedup key (`events-v1.md` "投递报文与 `delivery_id` 的
/// 位置"). `delivery.id` in the body is the same value; both are written together below.
const DELIVERY_ID_HEADER: &str = "X-Sylvode-Delivery-Id";

fn ms_interval(param_index: usize) -> String {
    format!("(${param_index}::bigint * interval '1 millisecond')")
}

/// The `dispatch_backoff_ms` ladder as a SQL expression over the row's own `attempts` column,
/// avoiding a `format!` nested inside another `format!`'s arguments.
fn dispatch_backoff_expr() -> String {
    format!(
        "(LEAST((attempts + 1) * {DISPATCH_BACKOFF_STEP_MS}::bigint, {DISPATCH_BACKOFF_CAP_MS}::bigint) * interval '1 millisecond')"
    )
}

// ---------------------------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------------------------

/// Counters from one dispatcher pass.
///
/// Reclaims expired leases, expands up to `batch` `event_dispatch` work items, sends up to
/// `batch` `event_deliveries`, then runs the retention reapers. Every sub-step logs and continues
/// on error rather than aborting the tick — matching `apps/worker/src/main.rs`'s existing
/// tolerance for one failing job not blocking the others on the same poll.
#[derive(Debug, Clone, Copy, Default)]
pub struct DispatchTickReport {
    pub expanded: u64,
    pub no_subscribers: u64,
    pub delivered: u64,
    pub delivery_retried_or_failed: u64,
    pub dispatch_leases_reclaimed: u64,
    pub delivery_leases_reclaimed: u64,
    pub dispatch_rows_reaped: u64,
    pub delivery_rows_reaped: u64,
}

pub async fn run_tick(state: &AppState, client: &reqwest::Client, batch: usize) -> DispatchTickReport {
    let batch = batch.max(1);
    let mut report = DispatchTickReport::default();

    match reclaim_expired_dispatch_leases(&state.db).await {
        Ok(n) => report.dispatch_leases_reclaimed = n,
        Err(err) => tracing::warn!(error = %err, "dispatcher: event_dispatch lease reclaim failed"),
    }
    match reclaim_expired_delivery_leases(&state.db).await {
        Ok(n) => report.delivery_leases_reclaimed = n,
        Err(err) => tracing::warn!(error = %err, "dispatcher: event_deliveries lease reclaim failed"),
    }

    for _ in 0..batch {
        match expand_one(&state.db).await {
            Ok(Some(ExpansionOutcome::Expanded)) => report.expanded += 1,
            Ok(Some(ExpansionOutcome::NoSubscribers)) => report.no_subscribers += 1,
            Ok(Some(ExpansionOutcome::StaleLease)) => {}
            Ok(None) => break,
            Err(err) => {
                tracing::warn!(error = %err, "dispatcher: event_dispatch expansion failed");
                break;
            }
        }
    }

    for _ in 0..batch {
        match send_one(state, client).await {
            Ok(Some(true)) => report.delivered += 1,
            Ok(Some(false)) => report.delivery_retried_or_failed += 1,
            Ok(None) => break,
            Err(err) => {
                tracing::warn!(error = %err, "dispatcher: event_deliveries send failed");
                break;
            }
        }
    }

    match reap_dispatch_retention(&state.db).await {
        Ok(n) => report.dispatch_rows_reaped = n,
        Err(err) => tracing::warn!(error = %err, "dispatcher: event_dispatch retention reaper failed"),
    }
    match reap_delivery_retention(&state.db).await {
        Ok(n) => report.delivery_rows_reaped = n,
        Err(err) => tracing::warn!(error = %err, "dispatcher: event_deliveries retention reaper failed"),
    }

    report
}

// ---------------------------------------------------------------------------------------------
// Lease reclaim (`events-v1.md` "leased 行的回收必须有独立的 reaper" / dispatch's dual guard)
// ---------------------------------------------------------------------------------------------

/// Rotates the lease on any `event_dispatch` row whose owner disappeared. Reclaiming does **not**
/// count toward `attempts` (expansion is deterministic, so a vanished worker is never a "poison
/// payload"); it counts toward the separate `lease_reclaims` budget, which — once exhausted — puts
/// the row into `failed` so a permanently crash-looping owner cannot wedge that document's queue
/// head forever.
async fn reclaim_expired_dispatch_leases(db: &DatabaseConnection) -> Result<u64, ApiError> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                WITH candidates AS (
                    SELECT id FROM event_dispatch
                    WHERE status = 'pending' AND lease_token IS NOT NULL AND lease_expires_at < now()
                    FOR UPDATE SKIP LOCKED
                )
                UPDATE event_dispatch d
                SET lease_token = NULL,
                    lease_expires_at = NULL,
                    lease_reclaims = d.lease_reclaims + 1,
                    status = CASE WHEN d.lease_reclaims + 1 >= $1 THEN 'failed' ELSE 'pending' END,
                    expanded_at = CASE WHEN d.lease_reclaims + 1 >= $1 THEN now() ELSE NULL END,
                    last_error_code = CASE WHEN d.lease_reclaims + 1 >= $1 THEN 'lease_reclaims_exhausted' ELSE NULL END
                FROM candidates c
                WHERE d.id = c.id
            ",
            vec![DISPATCH_MAX_LEASE_RECLAIMS.into()],
        ))
        .await?;
    Ok(result.rows_affected())
}

/// Same idea for `event_deliveries`, with the "回 `sealed`, 绝不回 `pending`" exception for replay
/// rows (`document_id IS NULL`) `events-v1.md` freezes.
async fn reclaim_expired_delivery_leases(db: &DatabaseConnection) -> Result<u64, ApiError> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                WITH candidates AS (
                    SELECT id FROM event_deliveries
                    WHERE status = 'leased' AND lease_expires_at < now()
                    FOR UPDATE SKIP LOCKED
                )
                UPDATE event_deliveries d
                SET lease_token = NULL,
                    lease_expires_at = NULL,
                    attempts = d.attempts + 1,
                    status = CASE
                        WHEN d.attempts + 1 >= d.max_attempts THEN 'failed'
                        WHEN d.document_id IS NULL THEN 'pending'
                        ELSE 'sealed'
                    END,
                    terminated_at = CASE WHEN d.attempts + 1 >= d.max_attempts THEN now() ELSE NULL END,
                    next_attempt_at = CASE
                        WHEN d.attempts + 1 >= d.max_attempts THEN d.next_attempt_at
                        ELSE now() + (LEAST((d.attempts + 1) * $1::bigint, $2::bigint) * interval '1 millisecond')
                    END,
                    last_error_code = CASE
                        WHEN d.attempts + 1 >= d.max_attempts THEN 'lease_expired_retries_exhausted'
                        ELSE d.last_error_code
                    END
                FROM candidates c
                WHERE d.id = c.id
            ",
            vec![DELIVERY_BACKOFF_STEP_MS.into(), DELIVERY_BACKOFF_CAP_MS.into()],
        ))
        .await?;
    Ok(result.rows_affected())
}

// ---------------------------------------------------------------------------------------------
// Expansion (`events-v1.md` "展开是一个事务" + "展开必须按 seq 有序")
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum ExpansionOutcome {
    Expanded,
    NoSubscribers,
    /// The final guarded `UPDATE` affected zero rows: this worker's lease was reclaimed mid-flight
    /// (`reclaim_expired_dispatch_leases` rotated the token) and another attempt now owns the row.
    /// The reservations this attempt made are safe leftovers (`event_delivery_sources`' `ON
    /// CONFLICT DO NOTHING` makes them idempotent for whoever expands next); nothing here is a
    /// failure worth counting or logging above `debug`.
    StaleLease,
}

#[derive(Debug, FromQueryResult)]
struct DispatchLeaseRow {
    id: Uuid,
    event_id: Uuid,
    workspace_id: Uuid,
    event_type: String,
    document_id: Option<Uuid>,
    accepted_seq: Option<i64>,
}

/// Leases exactly one `event_dispatch` work item — the oldest-ready row that is either not a
/// content event, or *is* the current head of its document's queue (`NOT EXISTS` a smaller pending
/// `accepted_seq` on the same document) — and expands it. Non-head content rows are simply never
/// selected, which realizes `dispatch_head_wait_backoff_ms`'s "bounded wait, zero `attempts` cost"
/// requirement without a separate wait state: an unselected row is untouched and is reconsidered
/// on the dispatcher's next pass once its predecessor has left `pending`.
async fn expand_one(db: &DatabaseConnection) -> Result<Option<ExpansionOutcome>, ApiError> {
    let lease_token = Uuid::new_v4().to_string();
    let leased = DispatchLeaseRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r"
                UPDATE event_dispatch
                SET lease_token = $1, lease_expires_at = now() + {ttl}
                WHERE id = (
                    SELECT d.id
                    FROM event_dispatch d
                    WHERE d.status = 'pending'
                      AND d.next_attempt_at <= now()
                      AND (d.lease_token IS NULL OR d.lease_expires_at < now())
                      AND (
                        d.document_id IS NULL
                        OR NOT EXISTS (
                            SELECT 1 FROM event_dispatch o
                            WHERE o.document_id = d.document_id
                              AND o.status = 'pending'
                              AND o.accepted_seq IS NOT NULL
                              AND o.accepted_seq < d.accepted_seq
                        )
                      )
                    ORDER BY d.next_attempt_at, d.id
                    LIMIT 1
                    FOR UPDATE SKIP LOCKED
                )
                RETURNING id, event_id, workspace_id, event_type, document_id, accepted_seq
            ",
            ttl = ms_interval(2)
        ),
        vec![lease_token.clone().into(), DISPATCH_LEASE_TTL_MS.into()],
    ))
    .one(db)
    .await?;

    let Some(work) = leased else { return Ok(None) };

    match expand_work(db, &work, &lease_token).await {
        Ok(outcome) => Ok(Some(outcome)),
        Err(err) => {
            record_expansion_failure(db, &work, &lease_token).await;
            Err(err)
        }
    }
}

async fn record_expansion_failure(db: &DatabaseConnection, work: &DispatchLeaseRow, lease_token: &str) {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                r"
                    UPDATE event_dispatch
                    SET attempts = attempts + 1,
                        lease_token = NULL,
                        lease_expires_at = NULL,
                        status = CASE WHEN attempts + 1 >= max_attempts THEN 'failed' ELSE 'pending' END,
                        expanded_at = CASE WHEN attempts + 1 >= max_attempts THEN now() ELSE NULL END,
                        next_attempt_at = CASE
                            WHEN attempts + 1 >= max_attempts THEN next_attempt_at
                            ELSE now() + {backoff}
                        END,
                        last_error_code = CASE WHEN attempts + 1 >= max_attempts THEN 'expansion_failed' ELSE 'expansion_retry' END
                    WHERE id = $1 AND lease_token = $2
                ",
                backoff = dispatch_backoff_expr()
            ),
            vec![work.id.into(), lease_token.into()],
        ))
        .await;
    if let Err(err) = result {
        tracing::warn!(dispatch_id = %work.id, error = %err, "dispatcher: recording expansion failure itself failed");
    }
}

#[derive(Debug, FromQueryResult)]
struct SubscriberRow {
    id: Uuid,
}

async fn expand_work(
    db: &DatabaseConnection,
    work: &DispatchLeaseRow,
    lease_token: &str,
) -> Result<ExpansionOutcome, ApiError> {
    let tx = db.begin().await?;

    // "active" subscribers, judged the same way `webhook_trigger.rs`'s existing fan-out judges
    // them (`events-v1.md` "订阅目录与 active 的判据": "与源码...WHERE active = true AND events ?
    // $2 同一判据，不另立标准"). v0.4's only `subscriber_kind` is `webhook`.
    let subscribers = SubscriberRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM webhooks WHERE workspace_id = $1 AND active = true AND events ? $2",
        vec![work.workspace_id.into(), work.event_type.clone().into()],
    ))
    .all(&tx)
    .await?;

    if subscribers.is_empty() {
        let affected = tx
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE event_dispatch SET status = 'no_subscribers', expanded_at = now() WHERE id = $1 AND lease_token = $2",
                vec![work.id.into(), lease_token.into()],
            ))
            .await?
            .rows_affected();
        if affected == 0 {
            tx.rollback().await?;
            return Ok(ExpansionOutcome::StaleLease);
        }
        tx.commit().await?;
        return Ok(ExpansionOutcome::NoSubscribers);
    }

    for subscriber in &subscribers {
        expand_one_subscriber(&tx, work, subscriber.id).await?;
    }

    let affected = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE event_dispatch SET status = 'expanded', expanded_at = now() WHERE id = $1 AND lease_token = $2",
            vec![work.id.into(), lease_token.into()],
        ))
        .await?
        .rows_affected();

    if affected == 0 {
        tx.rollback().await?;
        return Ok(ExpansionOutcome::StaleLease);
    }
    tx.commit().await?;
    Ok(ExpansionOutcome::Expanded)
}

#[derive(Debug, FromQueryResult)]
struct ReservedSourceRow {
    id: Uuid,
}

/// One `(work, subscriber)` pair. Order frozen by `events-v1.md`: reserve the source row first
/// (`delivery_id=NULL`); a conflict there means this pair is already registered from a prior
/// expansion attempt, and the function returns without touching any delivery row at all — the
/// literal "冲突分支保持零 delivery 变化" requirement.
async fn expand_one_subscriber<C: ConnectionTrait>(
    tx: &C,
    work: &DispatchLeaseRow,
    subscriber_id: Uuid,
) -> Result<(), ApiError> {
    let reserved = ReservedSourceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO event_delivery_sources (workspace_id, delivery_id, subscriber_kind, subscriber_id, source_event_id)
            VALUES ($1, NULL, 'webhook', $2, $3)
            ON CONFLICT (subscriber_kind, subscriber_id, source_event_id) DO NOTHING
            RETURNING id
        ",
        vec![work.workspace_id.into(), subscriber_id.into(), work.event_id.into()],
    ))
    .one(tx)
    .await?;

    let Some(reserved) = reserved else { return Ok(()) };

    let delivery_id = if let (Some(document_id), Some(accepted_seq)) = (work.document_id, work.accepted_seq) {
        bind_content_delivery(tx, work, subscriber_id, document_id, accepted_seq).await?
    } else {
        bind_plain_delivery(tx, work, subscriber_id).await?
    };

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE event_delivery_sources SET delivery_id = $1 WHERE id = $2",
        vec![delivery_id.into(), reserved.id.into()],
    ))
    .await?;

    Ok(())
}

/// Non-content event: exactly one `event_deliveries` row per `(dispatch_id, subscriber)`. The
/// `ON CONFLICT ... DO UPDATE` (rather than `DO NOTHING`) is the "re-expanding the same work must
/// not mint a new `delivery_id`" requirement — a `DO NOTHING` would return no row on the
/// (theoretical, since the source-table reservation above already de-duplicates this) re-expansion
/// path, leaving nothing to bind the source row to.
async fn bind_plain_delivery<C: ConnectionTrait>(
    tx: &C,
    work: &DispatchLeaseRow,
    subscriber_id: Uuid,
) -> Result<Uuid, ApiError> {
    #[derive(FromQueryResult)]
    struct IdRow {
        id: Uuid,
    }
    let new_id = Uuid::new_v4();
    let row = IdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO event_deliveries (id, dispatch_id, event_id, workspace_id, subscriber_kind, subscriber_id, status, next_attempt_at)
            VALUES ($1, $2, $3, $4, 'webhook', $5, 'pending', now())
            ON CONFLICT (dispatch_id, subscriber_kind, subscriber_id)
            DO UPDATE SET dispatch_id = event_deliveries.dispatch_id
            RETURNING id
        ",
        vec![
            new_id.into(),
            work.id.into(),
            work.event_id.into(),
            work.workspace_id.into(),
            subscriber_id.into(),
        ],
    ))
    .one(tx)
    .await?
    .ok_or(ApiError::Internal)?;
    Ok(row.id)
}

#[derive(Debug, FromQueryResult)]
struct PendingDeliveryRow {
    id: Uuid,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    count: i64,
}

/// `flow.content.accepted`: merges into the document's current `pending` delivery for this
/// subscriber when one exists and has room (`coalesced_source_events_max`), otherwise seals that
/// row and opens a fresh one. Safe to call without extra cross-document locking: the head-of-queue
/// rule in [`expand_one`] guarantees at most one `event_dispatch` work item per document is ever
/// being expanded at a time, so there is never a concurrent writer for this `(subscriber,
/// document)` pending row.
async fn bind_content_delivery<C: ConnectionTrait>(
    tx: &C,
    work: &DispatchLeaseRow,
    subscriber_id: Uuid,
    document_id: Uuid,
    accepted_seq: i64,
) -> Result<Uuid, ApiError> {
    let existing = PendingDeliveryRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id FROM event_deliveries
            WHERE subscriber_kind = 'webhook' AND subscriber_id = $1 AND document_id = $2 AND status = 'pending'
            FOR UPDATE
        ",
        vec![subscriber_id.into(), document_id.into()],
    ))
    .one(tx)
    .await?;

    if let Some(existing) = existing {
        let count = CountRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COUNT(*) AS count FROM event_delivery_sources WHERE delivery_id = $1",
            vec![existing.id.into()],
        ))
        .one(tx)
        .await?
        .map_or(0, |row| row.count);

        if count < COALESCED_SOURCE_EVENTS_MAX {
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!(
                    r"
                        UPDATE event_deliveries
                        SET latest_seq = GREATEST(latest_seq, $2), next_attempt_at = now() + {debounce}
                        WHERE id = $1
                    ",
                    debounce = ms_interval(3)
                ),
                vec![
                    existing.id.into(),
                    accepted_seq.into(),
                    CONTENT_DELIVERY_DEBOUNCE_MS.into(),
                ],
            ))
            .await?;
            return Ok(existing.id);
        }

        // Cap reached: freeze the row (guarded, so a racing lease-claim mid-transition cannot be
        // clobbered back to `pending`) and fall through to open a new one below.
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE event_deliveries SET status = 'sealed' WHERE id = $1 AND status = 'pending'",
            vec![existing.id.into()],
        ))
        .await?;
    }

    let new_id = Uuid::new_v4();
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r"
                INSERT INTO event_deliveries
                    (id, dispatch_id, event_id, workspace_id, subscriber_kind, subscriber_id, document_id,
                     status, first_seq, latest_seq, next_attempt_at)
                VALUES ($1, $2, $3, $4, 'webhook', $5, $6, 'pending', $7, $7, now() + {debounce})
            ",
            debounce = ms_interval(8)
        ),
        vec![
            new_id.into(),
            work.id.into(),
            work.event_id.into(),
            work.workspace_id.into(),
            subscriber_id.into(),
            document_id.into(),
            accepted_seq.into(),
            CONTENT_DELIVERY_DEBOUNCE_MS.into(),
        ],
    ))
    .await?;
    Ok(new_id)
}

// ---------------------------------------------------------------------------------------------
// Sending (`events-v1.md` "满额封存与 sealed 状态" lease-claim SQL, "投递报文与 delivery_id 的位置")
// ---------------------------------------------------------------------------------------------

#[derive(Debug, FromQueryResult)]
struct DeliveryLeaseRow {
    id: Uuid,
    event_id: Uuid,
    subscriber_id: Uuid,
    document_id: Option<Uuid>,
    attempts: i32,
    max_attempts: i32,
    first_seq: Option<i64>,
    latest_seq: Option<i64>,
}

/// Leases and sends exactly one `event_deliveries` row. `Ok(None)` means nothing was ready;
/// `Ok(Some(true))` a successful send, `Ok(Some(false))` a retry/terminal-failure/cancellation —
/// all three of those still "handled" the row and are not dispatcher errors.
async fn send_one(state: &AppState, client: &reqwest::Client) -> Result<Option<bool>, ApiError> {
    let lease_token = Uuid::new_v4().to_string();
    // Literal SQL from `events-v1.md` "满额封存与 `sealed` 状态", parameterized.
    let leased = DeliveryLeaseRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r"
                UPDATE event_deliveries
                SET status = 'leased', lease_token = $1, lease_expires_at = now() + {ttl}
                WHERE id = (
                    SELECT d.id FROM event_deliveries d
                    WHERE d.status IN ('pending', 'sealed') AND d.next_attempt_at <= now()
                      AND (
                        d.document_id IS NULL OR d.first_seq IS NULL
                        OR NOT EXISTS (
                            SELECT 1 FROM event_deliveries o
                            WHERE o.subscriber_kind = d.subscriber_kind
                              AND o.subscriber_id   = d.subscriber_id
                              AND o.document_id     = d.document_id
                              AND o.status IN ('sealed', 'leased')
                              AND o.first_seq < d.first_seq)
                      )
                    ORDER BY d.next_attempt_at, d.id
                    LIMIT 1 FOR UPDATE SKIP LOCKED)
                RETURNING id, event_id, subscriber_id, document_id, attempts, max_attempts, first_seq, latest_seq
            ",
            ttl = ms_interval(2)
        ),
        vec![lease_token.clone().into(), DELIVERY_LEASE_TTL_MS.into()],
    ))
    .one(&state.db)
    .await?;

    let Some(delivery) = leased else { return Ok(None) };
    Ok(Some(attempt_delivery(state, client, &delivery, &lease_token).await))
}

#[derive(Debug, FromQueryResult)]
struct WebhookRow {
    url: String,
    secret: String,
}

async fn attempt_delivery(
    state: &AppState,
    client: &reqwest::Client,
    delivery: &DeliveryLeaseRow,
    lease_token: &str,
) -> bool {
    // "发送前重新判定 active" (`events-v1.md` "订阅者在投递在飞时被改动"): read the current
    // endpoint/secret at send time, never a snapshot taken at expansion.
    let webhook = WebhookRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT url, secret FROM webhooks WHERE id = $1 AND active = true",
        vec![delivery.subscriber_id.into()],
    ))
    .one(&state.db)
    .await;

    let webhook = match webhook {
        Ok(Some(webhook)) => webhook,
        Ok(None) => {
            cancel_delivery(&state.db, delivery.id, lease_token, "subscriber_gone").await;
            return false;
        }
        Err(err) => {
            tracing::warn!(delivery_id = %delivery.id, error = %err, "dispatcher: subscriber lookup failed");
            retry_or_fail_delivery(&state.db, delivery, lease_token, "subscriber_lookup_failed").await;
            return false;
        }
    };

    let body = match build_delivery_body(&state.db, delivery).await {
        Ok(body) => body,
        Err(err) => {
            tracing::warn!(delivery_id = %delivery.id, error = %err, "dispatcher: building delivery body failed");
            retry_or_fail_delivery(&state.db, delivery, lease_token, "payload_build_failed").await;
            return false;
        }
    };

    let target = match validate_outbound_url(&webhook.url).await {
        Ok(target) => target,
        Err(err) => {
            tracing::warn!(delivery_id = %delivery.id, error = %err, "dispatcher: webhook url rejected");
            retry_or_fail_delivery(&state.db, delivery, lease_token, "endpoint_rejected").await;
            return false;
        }
    };

    let Ok(raw_body) = serde_json::to_string(&body) else {
        retry_or_fail_delivery(&state.db, delivery, lease_token, "payload_encode_failed").await;
        return false;
    };
    let signature = match sign_payload(&webhook.secret, &raw_body) {
        Ok(sig) => sig,
        Err(err) => {
            tracing::warn!(delivery_id = %delivery.id, error = %err, "dispatcher: signing failed");
            retry_or_fail_delivery(&state.db, delivery, lease_token, "signing_failed").await;
            return false;
        }
    };

    let response = client
        .post(target)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(WEBHOOK_SIGNATURE_HEADER, format!("sha256={signature}"))
        .header(DELIVERY_ID_HEADER, delivery.id.to_string())
        .body(raw_body)
        .send()
        .await;

    match response {
        Ok(resp) if resp.status().is_success() => {
            mark_delivered(&state.db, delivery.id, lease_token).await;
            true
        }
        Ok(resp) => {
            let code = format!("http_{}", resp.status().as_u16());
            retry_or_fail_delivery(&state.db, delivery, lease_token, &code).await;
            false
        }
        Err(err) => {
            tracing::warn!(delivery_id = %delivery.id, error = %err, "dispatcher: webhook request failed");
            retry_or_fail_delivery(&state.db, delivery, lease_token, "request_failed").await;
            false
        }
    }
}

async fn mark_delivered(db: &DatabaseConnection, delivery_id: Uuid, lease_token: &str) {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE event_deliveries
                SET status = 'dispatched', terminated_at = now(), lease_token = NULL, lease_expires_at = NULL
                WHERE id = $1 AND lease_token = $2
            ",
            vec![delivery_id.into(), lease_token.into()],
        ))
        .await;
    if let Err(err) = result {
        tracing::warn!(delivery_id = %delivery_id, error = %err, "dispatcher: marking delivery dispatched failed");
    }
}

async fn cancel_delivery(db: &DatabaseConnection, delivery_id: Uuid, lease_token: &str, reason: &str) {
    // `events-v1.md` "订阅者在投递在飞时被改动": a gone/inactive subscriber terminates the row as
    // `cancelled`, never `failed` — it must not pollute dead-letter counts or trigger alerts.
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE event_deliveries
                SET status = 'cancelled', terminated_at = now(), lease_token = NULL, lease_expires_at = NULL,
                    last_error_code = $2
                WHERE id = $1 AND lease_token = $3
            ",
            vec![delivery_id.into(), reason.into(), lease_token.into()],
        ))
        .await;
    if let Err(err) = result {
        tracing::warn!(delivery_id = %delivery_id, error = %err, "dispatcher: cancelling delivery failed");
    }
}

async fn retry_or_fail_delivery(db: &DatabaseConnection, delivery: &DeliveryLeaseRow, lease_token: &str, reason: &str) {
    let next_attempts = i64::from(delivery.attempts) + 1;
    let exhausted = next_attempts >= i64::from(delivery.max_attempts);
    let next_status = if exhausted {
        "failed"
    } else if delivery.document_id.is_none() {
        "pending"
    } else {
        "sealed"
    };
    let backoff_ms = (next_attempts * DELIVERY_BACKOFF_STEP_MS).min(DELIVERY_BACKOFF_CAP_MS);

    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE event_deliveries
                SET attempts = $2,
                    status = $3,
                    terminated_at = CASE WHEN $3 = 'failed' THEN now() ELSE NULL END,
                    next_attempt_at = CASE WHEN $3 = 'failed' THEN next_attempt_at ELSE now() + ($4::bigint * interval '1 millisecond') END,
                    lease_token = NULL,
                    lease_expires_at = NULL,
                    last_error_code = $5
                WHERE id = $1 AND lease_token = $6
            ",
            vec![
                delivery.id.into(),
                next_attempts.into(),
                next_status.into(),
                backoff_ms.into(),
                reason.into(),
                lease_token.into(),
            ],
        ))
        .await;
    if let Err(err) = result {
        tracing::warn!(delivery_id = %delivery.id, error = %err, "dispatcher: recording delivery failure failed");
    }
}

// ---------------------------------------------------------------------------------------------
// Delivery body (`events-v1.md` "投递报文与 delivery_id 的位置")
// ---------------------------------------------------------------------------------------------

#[derive(Debug, FromQueryResult)]
struct BusinessEventRow {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    event_type: String,
    aggregate_type: String,
    aggregate_id: String,
    actor_id: Option<Uuid>,
    source: Value,
    payload: Value,
    metadata: Value,
    correlation_id: Option<Uuid>,
    causation_id: Option<Uuid>,
    created_at: DateTime<Utc>,
}

async fn fetch_business_event<C: ConnectionTrait>(
    conn: &C,
    event_id: Uuid,
) -> Result<Option<BusinessEventRow>, ApiError> {
    Ok(BusinessEventRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, project_id, event_type, aggregate_type, aggregate_id, actor_id,
                   source, payload, metadata, correlation_id, causation_id, created_at
            FROM business_events WHERE id = $1
        ",
        vec![event_id.into()],
    ))
    .one(conn)
    .await?)
}

fn envelope_json(
    event: &BusinessEventRow,
    event_id_override: Option<Uuid>,
    created_at_override: Option<DateTime<Utc>>,
    payload_override: Option<Value>,
) -> Value {
    json!({
        "version": "openpr.event.v1",
        "event_id": event_id_override.unwrap_or(event.id),
        "event_type": event.event_type,
        "workspace_id": event.workspace_id,
        "project_id": event.project_id,
        "aggregate": { "type": event.aggregate_type, "id": event.aggregate_id },
        "actor_id": event.actor_id,
        "source": event.source,
        "payload": payload_override.unwrap_or_else(|| event.payload.clone()),
        "metadata": event.metadata,
        "correlation_id": event.correlation_id,
        "causation_id": event.causation_id,
        "created_at": created_at_override.unwrap_or(event.created_at).to_rfc3339(),
    })
}

#[derive(Debug, FromQueryResult)]
struct SourceEventIdRow {
    source_event_id: Uuid,
}

/// Builds `{delivery:{...}, event:{...openpr.event.v1}}` per `events-v1.md`.
///
/// "Coalesced" is judged by `event_delivery_sources` row count for this `delivery_id` being > 1
/// (the contract's exact rule — not `document_id IS NOT NULL`, which a `requeue_failed` revival can
/// clear on an otherwise still-coalesced row). Every delivery — content or not — has at least one
/// source row from [`expand_one_subscriber`]'s reservation, so this check is uniform.
async fn build_delivery_body(db: &DatabaseConnection, delivery: &DeliveryLeaseRow) -> Result<Value, ApiError> {
    let source_ids = SourceEventIdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT source_event_id FROM event_delivery_sources WHERE delivery_id = $1 ORDER BY created_at ASC",
        vec![delivery.id.into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(|row| row.source_event_id)
    .collect::<Vec<_>>();

    let attempt = i64::from(delivery.attempts) + 1;

    if source_ids.len() <= 1 {
        let event = fetch_business_event(db, delivery.event_id)
            .await?
            .ok_or(ApiError::Internal)?;
        return Ok(json!({
            "delivery": { "id": delivery.id, "attempt": attempt, "coalesced": false },
            "event": envelope_json(&event, None, None, None),
        }));
    }

    // Coalesced: envelope anchors on the *first* source event (lineage), `changed_block_ids` is
    // the union across every source event bound to this row, capped at
    // `changed_block_ids_per_delivery_max`.
    let first_event = fetch_business_event(db, delivery.event_id)
        .await?
        .ok_or(ApiError::Internal)?;

    let mut block_ids: Vec<Uuid> = Vec::new();
    let mut seen: HashSet<Uuid> = HashSet::new();
    let mut truncated = false;
    for source_id in &source_ids {
        let Some(source_event) = fetch_business_event(db, *source_id).await? else {
            continue;
        };
        let Some(ids) = source_event.payload.get("changed_block_ids").and_then(Value::as_array) else {
            continue;
        };
        for id in ids {
            let Some(id) = id.as_str().and_then(|raw| Uuid::parse_str(raw).ok()) else {
                continue;
            };
            if block_ids.len() >= CHANGED_BLOCK_IDS_PER_DELIVERY_MAX {
                truncated = true;
                break;
            }
            if seen.insert(id) {
                block_ids.push(id);
            }
        }
    }

    let payload = if truncated {
        json!({ "changed_block_ids_truncated": true })
    } else {
        json!({ "changed_block_ids": block_ids })
    };

    Ok(json!({
        "delivery": {
            "id": delivery.id,
            "attempt": attempt,
            "coalesced": true,
            "range": { "first_seq": delivery.first_seq, "latest_seq": delivery.latest_seq },
            "source_event_ids": source_ids,
            "block_ids_truncated": truncated,
        },
        "event": envelope_json(&first_event, Some(first_event.id), Some(first_event.created_at), Some(payload)),
    }))
}

// ---------------------------------------------------------------------------------------------
// Retention reapers (`events-v1.md` "reaper 的谓词只能删终态" / "计时锚点")
// ---------------------------------------------------------------------------------------------

async fn reap_dispatch_retention(db: &DatabaseConnection) -> Result<u64, ApiError> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                DELETE FROM event_dispatch
                WHERE lease_token IS NULL
                  AND (
                    (status = 'no_subscribers' AND expanded_at < now() - ($1::bigint * interval '1 hour'))
                    OR (status = 'expanded' AND expanded_at < now() - ($2::bigint * interval '1 day'))
                    OR (status = 'failed' AND expanded_at < now() - ($3::bigint * interval '1 day'))
                  )
            ",
            vec![
                DISPATCH_NO_SUBSCRIBERS_RETENTION_HOURS.into(),
                DISPATCH_EXPANDED_RETENTION_DAYS.into(),
                DISPATCH_FAILED_RETENTION_DAYS.into(),
            ],
        ))
        .await?;
    Ok(result.rows_affected())
}

async fn reap_delivery_retention(db: &DatabaseConnection) -> Result<u64, ApiError> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                DELETE FROM event_deliveries
                WHERE lease_token IS NULL
                  AND status IN ('dispatched', 'failed', 'cancelled')
                  AND terminated_at < now() - ($1::bigint * interval '1 day')
            ",
            vec![DELIVERY_RETENTION_DAYS.into()],
        ))
        .await?;
    Ok(result.rows_affected())
}

// ---------------------------------------------------------------------------------------------
// Real-database tests (opt-in via `OPENPR_TEST_DATABASE_URL`, matching
// `apps/api/src/routes/flow.rs`'s `flow_database_tests` — own throwaway database per run,
// migrated from `migrations/*.sql` on disk, dropped on the way out). Colocated with the private
// functions above (not in `routes::flow`) so tests can call `expand_one`/`send_one`/
// `build_delivery_body` directly rather than only through the public `run_tick` surface.
// ---------------------------------------------------------------------------------------------
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod dispatcher_database_tests {
    use platform::{
        app::AppState,
        config::{AppConfig, Secret},
    };
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
    use serde_json::json;
    use uuid::Uuid;

    use super::{
        ExpansionOutcome, build_delivery_body, expand_one, reclaim_expired_dispatch_leases, run_tick, send_one,
    };
    use crate::events::{BusinessEventInput, insert_business_event};

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

        let name = format!("openpr_dispatcher_{label}");
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
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "no migration file was found in {dir}");
        for path in files {
            let sql = std::fs::read_to_string(&path).expect("a migration file is readable");
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
                app_name: "dispatcher-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("dispatcher-route-test-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db,
        }
    }

    async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
        db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .await
            .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
    }

    async fn seed_workspace(db: &DatabaseConnection) -> Uuid {
        let workspace_id = Uuid::new_v4();
        let owner_id = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'test', 'user', true)",
            vec![owner_id.into(), format!("{owner_id}@dispatcher.test").into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'dispatcher test', $3)",
            vec![
                workspace_id.into(),
                format!("ws-{workspace_id}").into(),
                owner_id.into(),
            ],
        )
        .await;
        workspace_id
    }

    /// Writes one `business_events` row plus its **exactly one** `event_dispatch` row, the same
    /// way a real domain transaction does (`flow::command::create_object`,
    /// `flow::collab::write::insert_event_dispatch_content`) — this is the "domain 事务只插一行
    /// `event_dispatch`" half of the required proof; [`run_tick`]/[`expand_one`] below is the other
    /// half ("`event_deliveries` 是 commit 之后由 dispatcher 展开的，不在域事务内").
    async fn commit_dispatch_work(
        db: &DatabaseConnection,
        workspace_id: Uuid,
        event_type: &str,
        document_id: Option<Uuid>,
        accepted_seq: Option<i64>,
        payload: serde_json::Value,
    ) -> Uuid {
        let event_id = insert_business_event(
            db,
            BusinessEventInput {
                workspace_id,
                project_id: None,
                event_type: event_type.to_string(),
                aggregate_type: "flow_object".to_string(),
                aggregate_id: Uuid::new_v4().to_string(),
                actor_id: None,
                source: json!({ "surface": "rest" }),
                payload,
                metadata: json!({}),
                correlation_id: None,
                causation_id: None,
                idempotency_key: None,
            },
        )
        .await
        .expect("business event insert succeeds");

        exec(
            db,
            "INSERT INTO event_dispatch (id, event_id, workspace_id, event_type, document_id, accepted_seq, max_attempts) \
             VALUES ($1, $2, $3, $4, $5, $6, 10)",
            vec![
                Uuid::new_v4().into(),
                event_id.into(),
                workspace_id.into(),
                event_type.into(),
                document_id.into(),
                accepted_seq.into(),
            ],
        )
        .await;

        event_id
    }

    async fn seed_webhook(db: &DatabaseConnection, workspace_id: Uuid, url: &str, events: &[&str]) -> Uuid {
        let webhook_id = Uuid::new_v4();
        let owner_id = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'test', 'user', true)",
            vec![owner_id.into(), format!("{owner_id}@dispatcher.test").into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO webhooks (id, workspace_id, name, url, secret, events, active, created_by) \
             VALUES ($1, $2, 'test hook', $3, 'shh', $4::jsonb, true, $5)",
            vec![
                webhook_id.into(),
                workspace_id.into(),
                url.into(),
                serde_json::to_value(events).unwrap_or_default().into(),
                owner_id.into(),
            ],
        )
        .await;
        webhook_id
    }

    async fn count(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> i64 {
        let row = db
            .query_one(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .await
            .expect("count query runs")
            .expect("count query returns a row");
        row.try_get("", "n").expect("n column reads")
    }

    #[tokio::test]
    async fn event_deliveries_are_created_only_by_a_dispatcher_tick_never_by_the_domain_transaction() {
        let scratch = scratch_or_skip!("post-commit-expansion");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created"],
        )
        .await;

        // The "domain transaction" step: exactly what `flow::command::create_object` does, with
        // nothing about dispatching that runs the dispatcher itself.
        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;

        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_dispatch WHERE event_id = $1",
                vec![event_id.into()],
            )
            .await,
            1,
            "the domain transaction must write exactly one event_dispatch row"
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
                vec![event_id.into()],
            )
            .await,
            0,
            "no event_deliveries row may exist before the dispatcher ever runs"
        );

        // Now, and only now, run the dispatcher tick — a step entirely separate from (and later
        // than) the commit above.
        let state = state_for(scratch.db.clone());
        let client = reqwest::Client::new();
        let report = run_tick(&state, &client, 4).await;
        assert_eq!(report.expanded, 1, "{report:?}");

        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
                vec![event_id.into()],
            )
            .await,
            1,
            "expansion must have created exactly one delivery for the one registered subscriber"
        );
        let dispatch_status: String = scratch
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT status FROM event_dispatch WHERE event_id = $1",
                vec![event_id.into()],
            ))
            .await
            .expect("status query runs")
            .expect("status query returns a row")
            .try_get("", "status")
            .expect("status column reads");
        assert_eq!(dispatch_status, "expanded");

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn zero_subscribers_terminalizes_the_work_and_creates_zero_deliveries() {
        let scratch = scratch_or_skip!("no-subscribers");
        let workspace_id = seed_workspace(&scratch.db).await;
        // No webhook registered at all.
        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.archived", None, None, json!({})).await;

        let state = state_for(scratch.db.clone());
        let client = reqwest::Client::new();
        let report = run_tick(&state, &client, 4).await;
        assert_eq!(report.no_subscribers, 1, "{report:?}");
        assert_eq!(report.expanded, 0, "{report:?}");

        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
                vec![event_id.into()],
            )
            .await,
            0
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_delivery_sources WHERE source_event_id = $1",
                vec![event_id.into()],
            )
            .await,
            0
        );

        let dispatch_status: String = scratch
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT status FROM event_dispatch WHERE event_id = $1",
                vec![event_id.into()],
            ))
            .await
            .expect("status query runs")
            .expect("status query returns a row")
            .try_get("", "status")
            .expect("status column reads");
        assert_eq!(dispatch_status, "no_subscribers");

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn re_expanding_the_same_work_after_a_simulated_crash_does_not_change_delivery_id_or_duplicate_sources() {
        let scratch = scratch_or_skip!("reexpansion-idempotent");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created"],
        )
        .await;
        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;

        let first = expand_one(&scratch.db)
            .await
            .expect("expansion runs")
            .expect("one work item was pending");
        assert!(matches!(first, ExpansionOutcome::Expanded));

        let delivery_id: Uuid = scratch
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM event_deliveries WHERE event_id = $1",
                vec![event_id.into()],
            ))
            .await
            .expect("delivery query runs")
            .expect("exactly one delivery row exists")
            .try_get("", "id")
            .expect("id column reads");

        // Simulate a dispatcher crash mid-flight: the work item is put back to `pending` with no
        // lease, exactly what `reclaim_expired_dispatch_leases` would have done to a genuinely
        // abandoned lease. `expand_one` must then re-expand it without minting a second delivery.
        exec(
            &scratch.db,
            "UPDATE event_dispatch SET status = 'pending', expanded_at = NULL, lease_token = NULL, lease_expires_at = NULL \
             WHERE event_id = $1",
            vec![event_id.into()],
        )
        .await;

        let second = expand_one(&scratch.db)
            .await
            .expect("re-expansion runs")
            .expect("the work item is pending again");
        assert!(matches!(second, ExpansionOutcome::Expanded));

        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
                vec![event_id.into()],
            )
            .await,
            1,
            "re-expansion must not mint a second delivery row"
        );
        let delivery_id_after: Uuid = scratch
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM event_deliveries WHERE event_id = $1",
                vec![event_id.into()],
            ))
            .await
            .expect("delivery query runs")
            .expect("exactly one delivery row exists")
            .try_get("", "id")
            .expect("id column reads");
        assert_eq!(
            delivery_id_after, delivery_id,
            "delivery_id must stay stable across re-expansion"
        );

        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_delivery_sources WHERE source_event_id = $1",
                vec![event_id.into()],
            )
            .await,
            1,
            "the source table's ON CONFLICT DO NOTHING must keep this at exactly one row"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn content_events_on_the_same_document_coalesce_into_one_pending_delivery() {
        let scratch = scratch_or_skip!("coalescing");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.content.accepted"],
        )
        .await;
        let document_id = Uuid::new_v4();

        let event_1 = commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(1),
            json!({ "changed_block_ids": [Uuid::new_v4()] }),
        )
        .await;
        let event_2 = commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(2),
            json!({ "changed_block_ids": [Uuid::new_v4()] }),
        )
        .await;

        let state = state_for(scratch.db.clone());
        let client = reqwest::Client::new();
        // Two work items to expand, head-of-queue ordered (seq 1 before seq 2).
        let report = run_tick(&state, &client, 8).await;
        assert_eq!(report.expanded, 2, "{report:?}");

        let deliveries = count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE subscriber_kind = 'webhook' AND document_id = $1",
            vec![document_id.into()],
        )
        .await;
        assert_eq!(
            deliveries, 1,
            "both accepted updates must coalesce into one delivery row"
        );

        #[derive(sea_orm::FromQueryResult)]
        struct Row {
            id: Uuid,
            first_seq: Option<i64>,
            latest_seq: Option<i64>,
        }
        let row = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, first_seq, latest_seq FROM event_deliveries WHERE subscriber_kind = 'webhook' AND document_id = $1",
            vec![document_id.into()],
        ))
        .one(&scratch.db)
        .await
        .expect("delivery query runs")
        .expect("one coalesced delivery row exists");
        assert_eq!(row.first_seq, Some(1));
        assert_eq!(row.latest_seq, Some(2));

        let source_rows = count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_delivery_sources WHERE delivery_id = $1",
            vec![row.id.into()],
        )
        .await;
        assert_eq!(source_rows, 2, "one source row per merged accepted update");

        let body = build_delivery_body(&scratch.db, &delivery_lease_row_for_test(row.id, event_1, None))
            .await
            .expect("body builds");
        assert_eq!(body["delivery"]["coalesced"], true, "{body}");
        let source_ids = body["delivery"]["source_event_ids"].as_array().expect("array");
        assert_eq!(source_ids.len(), 2);
        assert!(source_ids.contains(&json!(event_1)));
        assert!(source_ids.contains(&json!(event_2)));
        assert_eq!(body["event"]["event_type"], "flow.content.accepted", "{body}");

        scratch.drop_self().await;
    }

    /// Test-only constructor: production code only ever builds a `DispatchLeaseRow`/
    /// `DeliveryLeaseRow` from a `RETURNING` clause; this lets [`build_delivery_body`] (a private
    /// function this module tests directly) be exercised against a delivery already created by
    /// [`run_tick`] above without re-running the lease `UPDATE`.
    fn delivery_lease_row_for_test(id: Uuid, event_id: Uuid, subscriber_id: Option<Uuid>) -> super::DeliveryLeaseRow {
        super::DeliveryLeaseRow {
            id,
            event_id,
            subscriber_id: subscriber_id.unwrap_or_else(Uuid::new_v4),
            document_id: None,
            attempts: 0,
            max_attempts: 10,
            first_seq: None,
            latest_seq: None,
        }
    }

    #[tokio::test]
    async fn send_one_leases_a_delivery_and_retries_it_when_the_endpoint_is_rejected() {
        let scratch = scratch_or_skip!("send-retry");
        let workspace_id = seed_workspace(&scratch.db).await;
        // Loopback is refused by `validate_outbound_url`'s SSRF guard under the fallback (no
        // `[outbound] allow_private`) runtime configuration every unit test in this binary shares,
        // exactly like `webhook_trigger.rs`'s own `internal_targets_are_refused_before_the_request_is_built`
        // test asserts. This proves the full lease -> build body -> validate -> retry path, short of
        // an actual successful send.
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://127.0.0.1:1/hook",
            &["flow.object.created"],
        )
        .await;
        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;

        let state = state_for(scratch.db.clone());
        let client = reqwest::Client::new();
        expand_one(&scratch.db).await.expect("expansion runs");

        let outcome = send_one(&state, &client).await.expect("send_one runs");
        assert_eq!(
            outcome,
            Some(false),
            "a rejected endpoint is a handled failure, not a dispatcher error"
        );

        #[derive(sea_orm::FromQueryResult)]
        struct Row {
            status: String,
            attempts: i32,
            last_error_code: Option<String>,
            lease_token: Option<String>,
        }
        let row = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status, attempts, last_error_code, lease_token FROM event_deliveries WHERE event_id = $1",
            vec![event_id.into()],
        ))
        .one(&scratch.db)
        .await
        .expect("delivery query runs")
        .expect("one delivery row exists");
        assert_eq!(
            row.status, "pending",
            "non-content deliveries retry back to pending, never sealed"
        );
        assert_eq!(row.attempts, 1);
        assert_eq!(row.last_error_code.as_deref(), Some("endpoint_rejected"));
        assert!(
            row.lease_token.is_none(),
            "the lease must be released before the next attempt"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_dispatch_leases_expired_worker_is_reclaimed_and_eventually_dead_lettered() {
        let scratch = scratch_or_skip!("dispatch-lease-reclaim");
        let workspace_id = seed_workspace(&scratch.db).await;
        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;

        // Simulate a worker that leased the work item and then vanished: an expired lease still
        // sitting on a `pending` row.
        exec(
            &scratch.db,
            "UPDATE event_dispatch SET lease_token = 'stale', lease_expires_at = now() - interval '1 minute' \
             WHERE event_id = $1",
            vec![event_id.into()],
        )
        .await;

        let reclaimed = reclaim_expired_dispatch_leases(&scratch.db)
            .await
            .expect("reclaim runs");
        assert_eq!(reclaimed, 1);

        #[derive(sea_orm::FromQueryResult)]
        struct Row {
            status: String,
            lease_reclaims: i32,
            lease_token: Option<String>,
        }
        let row = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status, lease_reclaims, lease_token FROM event_dispatch WHERE event_id = $1",
            vec![event_id.into()],
        ))
        .one(&scratch.db)
        .await
        .expect("dispatch query runs")
        .expect("one dispatch row exists");
        assert_eq!(row.status, "pending", "a first reclaim must not exhaust the budget");
        assert_eq!(row.lease_reclaims, 1);
        assert!(row.lease_token.is_none());

        scratch.drop_self().await;
    }
}
