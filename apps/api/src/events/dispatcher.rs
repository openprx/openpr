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
use std::sync::atomic::{AtomicI64, Ordering};

use chrono::{DateTime, Utc};
use platform::app::AppState;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::ApiError;
use crate::flow::collab::limits::SUBSCRIBERS_PER_WORKSPACE_MAX;
use crate::outbound::validate_outbound_url;
use crate::webhook_trigger::{WEBHOOK_SIGNATURE_HEADER, sign_payload};

// ---------------------------------------------------------------------------------------------
// Test-only fault injection (`events-v1.md` "展开是一个事务": the only way to prove step (b)'s
// failure rolls back the *whole* transaction — including the step (a) reservation that already
// ran on the same `tx` — is to make step (b) actually fail after (a) has already succeeded.
// Gated entirely behind `cfg(test)`: the task-local key does not exist, and this call site is a
// zero-cost `false` literal, in any non-test build (including `apps/worker`, which links this
// crate as a dependency and therefore never sees this crate's own `cfg(test)`).
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
tokio::task_local! {
    static FAIL_EXPANSION_STEP_B: std::cell::Cell<bool>;
}

#[cfg(test)]
fn step_b_fault_armed() -> bool {
    FAIL_EXPANSION_STEP_B.try_with(std::cell::Cell::get).unwrap_or(false)
}

#[cfg(not(test))]
const fn step_b_fault_armed() -> bool {
    false
}

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

fn delivery_backoff_ms(attempts: i64) -> i64 {
    (attempts * DELIVERY_BACKOFF_STEP_MS).min(DELIVERY_BACKOFF_CAP_MS)
}

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

/// `coalesced_source_events_max`, frozen at 20 by `limits-v1.md`. Rule: "取 debounce 窗口内单文档
/// 合并事件数的 p99 上界" — the structured block derives it from two values that are themselves
/// frozen, `content_delivery_debounce_ms=2000` and the per-connection `updates_per_connection_
/// _per_second=10`: at most 10 x 2 = 20 accepted updates can land in one 2-second debounce window,
/// so 20 *is* that p99 upper bound.
///
/// This constant used to be 64 "for 3x headroom", where the 64 was borrowed from
/// `flow::collab::limits`'s warm-cache entry count — a number from another domain, which is
/// exactly the invented value `limits-v1.md` forbids. It is also not free: because the debounce is
/// *trailing*, at the frozen 10 updates/s the cap itself is what closes the window, so the cap is
/// paid directly in content-delivery latency (measured 8317 ms at 64, about 4.0 s at 20) and buys
/// only a larger full-window body (9216 B at 20 vs. 10932 B at 64).
const COALESCED_SOURCE_EVENTS_MAX: i64 = 20;

/// The `limit_kind` `limits-v1.md` freezes for [`SUBSCRIBERS_PER_WORKSPACE_MAX`]; every rejection
/// this module raises for that ceiling reports it verbatim.
pub const WORKSPACE_SUBSCRIBERS_LIMIT_KIND: &str = "workspace_subscribers";

/// Active subscribers in one workspace, judged by exactly the predicate [`expand_work`] and
/// `webhook_trigger.rs` already use for "active" (`events-v1.md` "订阅目录与 active 的判据":
/// "与源码...`WHERE active = true` 同一判据，不另立标准"). v0.4's only `subscriber_kind` is
/// `webhook`, so this is the workspace's whole subscriber directory.
///
/// Counted without the `events ? type` filter on purpose: `subscribers_per_workspace_max` bounds
/// the workspace's *subscriber total*, not the subset that happens to match one event type.
pub async fn workspace_subscriber_count<C: ConnectionTrait>(conn: &C, workspace_id: Uuid) -> Result<i64, ApiError> {
    workspace_subscriber_count_excluding(conn, workspace_id, None).await
}

/// [`workspace_subscriber_count`] with one subscriber left out of the tally — the row an update is
/// about to switch on, which must not be counted both as "already there" and as "the one being
/// added". `NULL` excludes nothing.
async fn workspace_subscriber_count_excluding<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    excluded: Option<Uuid>,
) -> Result<i64, ApiError> {
    Ok(CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COUNT(*) AS count FROM webhooks \
         WHERE workspace_id = $1 AND active = true AND ($2::uuid IS NULL OR id <> $2)",
        vec![workspace_id.into(), excluded.into()],
    ))
    .one(conn)
    .await?
    .map_or(0, |row| row.count))
}

/// The one place a `subscribers_per_workspace_max` rejection is built, so the registration path and
/// the expansion path cannot report the ceiling differently. Carries the frozen `limit_kind` plus
/// the numeric `limit`/`observed` that `error-mapping-v1.md` defines for `limit_exceeded`.
fn workspace_subscribers_limit_exceeded(observed: i64) -> ApiError {
    ApiError::limit_exceeded(
        format!(
            "limit_exceeded: {WORKSPACE_SUBSCRIBERS_LIMIT_KIND} (at most {SUBSCRIBERS_PER_WORKSPACE_MAX} active subscribers per workspace)"
        ),
        WORKSPACE_SUBSCRIBERS_LIMIT_KIND,
        Some(json!(SUBSCRIBERS_PER_WORKSPACE_MAX)),
        Some(json!(observed)),
        None,
    )
}

/// Registration ceiling for `subscribers_per_workspace_max`: called before a workspace gains one
/// more *active* subscriber (a created webhook, or an existing one being switched back on).
///
/// This is the "拒绝 ceiling" half of the contract row — the exact ceiling is accepted, the one
/// past it is refused with `limit_exceeded{limit_kind:"workspace_subscribers"}`, and nothing
/// already registered is silently dropped (the same shape `limits-v1.md` spells out for
/// `object_grants_max`: "达到上限即拒绝新增条目，不静默截断已有授予"). It is *not* the delivery-budget
/// shape of `coalesced_source_events_max`, which raises no `limit_exceeded` at all: this key sits
/// in the contract's authorization/subscription table, and `workspace_subscribers` enters the
/// `limit_kind` universe (and therefore `error_kind_coverage.expected[]`) the moment it is frozen.
/// `becoming_active` names an existing subscriber row that this write is switching on, so it is
/// excluded from the "already there" tally and counted once, as the slot being taken; pass `None`
/// when a brand-new row is being inserted. An update that leaves an already-active subscriber
/// active therefore stays accepted even in a workspace sitting exactly at the ceiling.
pub async fn ensure_workspace_subscriber_slot<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    becoming_active: Option<Uuid>,
) -> Result<(), ApiError> {
    let current = workspace_subscriber_count_excluding(conn, workspace_id, becoming_active).await?;
    if current + 1 > SUBSCRIBERS_PER_WORKSPACE_MAX {
        return Err(workspace_subscribers_limit_exceeded(current + 1));
    }
    Ok(())
}

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

/// `delivery_source_retention_days` (`limits-v1.md`, `status: unset`): how long an
/// `event_delivery_sources` tombstone (`delivery_id IS NULL`, left behind once
/// [`reap_delivery_retention`] deletes the delivery row it named) survives before this file's own
/// tombstone reaper deletes it too. Rule: "取 replay 窗口上限并留一个清理周期的余量", and "必须严格
/// 大于 `replay_max_window_days`" and "必须 ≥ `delivery_retention_days`" -- v0.4 has no replay
/// feature yet (that lands v0.8, per `events-v1.md` "Replay 与 `event_delivery_sources`"), so
/// there is no frozen `replay_max_window_days` to measure against; matching
/// `DISPATCH_FAILED_RETENTION_DAYS`'s own "one duty-rotation quarter" reasoning both satisfies the
/// documented `>= delivery_retention_days` floor with 3x headroom and leaves an operator a full
/// quarter to look up what a dead-lettered delivery covered before its dedup evidence disappears.
const DELIVERY_SOURCE_RETENTION_DAYS: i64 = 90;

/// `replay_max_window_days`, proposed for the v0.8 reviewed budget lock.
///
/// Sixty days keeps replay strictly inside the independently retained 90-day source tombstone
/// window and leaves one full `delivery_retention_days` cleanup cycle of safety margin.  The
/// contract remains the authority: this value must be copied into its reviewed budget artifact
/// before an official candidate can pass.
pub const REPLAY_MAX_WINDOW_DAYS: i64 = 60;

/// Header carrying the immutable consumer dedup key (`events-v1.md` "投递报文与 `delivery_id` 的
/// 位置"). `delivery.id` in the body is the same value; both are written together below.
const DELIVERY_ID_HEADER: &str = "X-Sylvode-Delivery-Id";

/// `dispatcher_liveness` readiness budget: how stale the last started tick may be before
/// [`dispatcher_is_live`] reports the process not-live.
///
/// `limits-v1.md` freezes no numeric value for this signal (it only names the mechanism), so this
/// follows the same "several multiples of the expected cadence" rule the rest of this file's
/// `status: unset` budgets use — six times the worker's 5-second poll interval
/// (`apps/worker/src/main.rs`) tolerates one slow tick without flapping, while still catching a
/// genuinely wedged poll loop within half a minute.
pub const DISPATCHER_LIVENESS_MAX_SILENCE_MS: i64 = 30_000;

/// `oldest_pending_age` alert threshold.
///
/// Several multiples of the slowest *legitimate* resolution path in this file (FIFO head-of-queue
/// wait plus the delivery backoff ladder's 300s cap), so this only fires once a backlog item has
/// genuinely stalled rather than while it is merely waiting its turn behind a healthy predecessor.
pub const OLDEST_PENDING_AGE_ALERT_MS: i64 = 15 * 60 * 1000;

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
    /// `event_delivery_sources` tombstone reaper: rows deleted by the `delivery_id IS NULL`
    /// predicate (`events-v1.md` "来源表的保留期与外键语义").
    pub delivery_source_tombstones_reaped: u64,
    /// `oldest_pending_age` (dispatch half): age in ms of the oldest still-`pending`
    /// `event_dispatch` row after this pass, `None` when that backlog is empty. Anchored on
    /// `created_at`, the only timestamp such a row has before it resolves.
    pub oldest_pending_dispatch_age_ms: Option<i64>,
    /// `oldest_pending_age` (delivery half): age in ms of the oldest still-undelivered
    /// `event_deliveries` row (`pending`/`sealed`/`leased`) after this pass, `None` when empty.
    pub oldest_pending_delivery_age_ms: Option<i64>,
}

/// `dispatcher_liveness` readiness probe: `true` iff this process has *started* a `run_tick` pass
/// within `max_silence_ms` of `now`.
///
/// `false` before the first tick this process has ever run, or once the gap since the last started
/// tick exceeds the budget — either one is exactly the signal `ADR-0011`'s "dispatcher 不在线即整体
/// 告警" needs a caller (e.g. a `/readyz` route) to alert on.
pub fn dispatcher_is_live(now: DateTime<Utc>, max_silence_ms: i64) -> bool {
    let last = LAST_TICK_STARTED_AT_UNIX_MS.load(Ordering::Relaxed);
    let last = if last == 0 {
        None
    } else {
        DateTime::from_timestamp_millis(last)
    };
    dispatcher_is_live_since(last, now, max_silence_ms)
}

/// The pure core of [`dispatcher_is_live`], taking "when did the last tick start" as an explicit
/// argument instead of reading the process-global clock.
///
/// Exists so tests can exercise arbitrary `(last_tick, now)` pairs — including "no tick has ever
/// happened" — by advancing a virtual clock through the argument, without racing every *other* test
/// in the same binary that calls [`run_tick`] and therefore mutates the one shared global this
/// function's public wrapper reads.
pub fn dispatcher_is_live_since(
    last_tick_started_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    max_silence_ms: i64,
) -> bool {
    last_tick_started_at.is_some_and(|last| (now - last).num_milliseconds() <= max_silence_ms)
}

/// `oldest_pending_age` threshold check over a completed tick's report: `true` iff either half of
/// the backlog (dispatch or delivery) is older than `threshold_ms`. A caller alerts on `true`.
pub fn backlog_alert(report: &DispatchTickReport, threshold_ms: i64) -> bool {
    report
        .oldest_pending_dispatch_age_ms
        .is_some_and(|age| age > threshold_ms)
        || report
            .oldest_pending_delivery_age_ms
            .is_some_and(|age| age > threshold_ms)
}

/// Unix milliseconds of the last time [`run_tick`] *started* a pass in this process; `0` before
/// the first ever tick. No persistence by design: a fresh process is correctly not-live until its
/// own first tick, and a process whose poll loop wedges is correctly reported stale without any
/// other process needing to notice.
static LAST_TICK_STARTED_AT_UNIX_MS: AtomicI64 = AtomicI64::new(0);

pub async fn run_tick(state: &AppState, client: &reqwest::Client, batch: usize) -> DispatchTickReport {
    let batch = batch.max(1);
    let now = Utc::now();
    LAST_TICK_STARTED_AT_UNIX_MS.store(now.timestamp_millis(), Ordering::Relaxed);
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

    match reap_dispatch_retention(&state.db, now).await {
        Ok(n) => report.dispatch_rows_reaped = n,
        Err(err) => tracing::warn!(error = %err, "dispatcher: event_dispatch retention reaper failed"),
    }
    match reap_delivery_retention(&state.db, now).await {
        Ok(n) => report.delivery_rows_reaped = n,
        Err(err) => tracing::warn!(error = %err, "dispatcher: event_deliveries retention reaper failed"),
    }
    match reap_delivery_source_tombstones(&state.db, now).await {
        Ok(n) => report.delivery_source_tombstones_reaped = n,
        Err(err) => tracing::warn!(error = %err, "dispatcher: event_delivery_sources tombstone reaper failed"),
    }

    match oldest_pending_dispatch_age_ms(&state.db, now).await {
        Ok(age) => report.oldest_pending_dispatch_age_ms = age,
        Err(err) => tracing::warn!(error = %err, "dispatcher: oldest_pending_dispatch_age query failed"),
    }
    match oldest_pending_delivery_age_ms(&state.db, now).await {
        Ok(age) => report.oldest_pending_delivery_age_ms = age,
        Err(err) => tracing::warn!(error = %err, "dispatcher: oldest_pending_delivery_age query failed"),
    }

    report
}

#[derive(Debug, FromQueryResult)]
struct OldestPendingRow {
    oldest_created_at: Option<DateTime<Utc>>,
}

/// `oldest_pending_age`, dispatch half: age of the oldest still-unresolved (`status = 'pending'`)
/// `event_dispatch` row. `None` when there is no such row.
async fn oldest_pending_dispatch_age_ms(db: &DatabaseConnection, now: DateTime<Utc>) -> Result<Option<i64>, ApiError> {
    let row = OldestPendingRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT MIN(created_at) AS oldest_created_at FROM event_dispatch WHERE status = 'pending'",
        vec![],
    ))
    .one(db)
    .await?;
    Ok(row
        .and_then(|r| r.oldest_created_at)
        .map(|oldest| (now - oldest).num_milliseconds().max(0)))
}

/// `oldest_pending_age`, delivery half: age of the oldest still-undelivered `event_deliveries` row
/// (`pending`/`sealed`/`leased` — everything short of a terminal state counts as "未投递"). `None`
/// when there is no such row.
async fn oldest_pending_delivery_age_ms(db: &DatabaseConnection, now: DateTime<Utc>) -> Result<Option<i64>, ApiError> {
    let row = OldestPendingRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT MIN(created_at) AS oldest_created_at FROM event_deliveries WHERE status IN ('pending', 'sealed', 'leased')",
        vec![],
    ))
    .one(db)
    .await?;
    Ok(row
        .and_then(|r| r.oldest_created_at)
        .map(|oldest| (now - oldest).num_milliseconds().max(0)))
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

    // `subscribers_per_workspace_max` = 100 (`limits-v1.md`, `limit_kind: workspace_subscribers`)
    // on the expansion path. The contract's row for this key describes it as the **dispatcher
    // fan-out amplification bound**: one domain transaction writes exactly one `event_dispatch`
    // row, and expanding it may produce at most this many `event_deliveries` rows.
    //
    // A workspace over the ceiling makes the expansion refuse rather than expand a truncated
    // subset: the contract's rejection ceilings say "达到上限即拒绝新增条目，不静默截断已有" — a
    // silently dropped subscriber would be exactly that silent truncation, and it would be
    // invisible (the delivery it should have got simply never exists). Refusing raises
    // `limit_exceeded{limit_kind:"workspace_subscribers"}`, which [`expand_one`] turns into the
    // ordinary expansion-failure path (`attempts + 1`, backoff, dead-letter at `max_attempts`
    // with `last_error_code`) — loud, bounded and operator-visible.
    //
    // [`ensure_workspace_subscriber_slot`] on the registration path is what keeps this unreachable
    // in practice: a workspace can only cross the ceiling through rows written before this ceiling
    // existed, or written around the API. This is a fixed contract anchor, deliberately *not* a
    // lease-budget computation — `limits-v1.md` records that its own `rule` cannot bind (expansion
    // would need ~39,700 subscribers to saturate `dispatch_lease_ttl_ms`).
    let subscriber_total = workspace_subscriber_count(&tx, work.workspace_id).await?;
    if subscriber_total > SUBSCRIBERS_PER_WORKSPACE_MAX {
        tx.rollback().await?;
        return Err(workspace_subscribers_limit_exceeded(subscriber_total));
    }

    // "active" subscribers, judged the same way `webhook_trigger.rs`'s existing fan-out judges
    // them (`events-v1.md` "订阅目录与 active 的判据": "与源码...WHERE active = true AND events ?
    // $2 同一判据，不另立标准"). v0.4's only `subscriber_kind` is `webhook`.
    //
    // `LIMIT` restates the same ceiling structurally, so the "at most
    // `subscribers_per_workspace_max` delivery rows per work item" invariant holds for this query
    // even if a subscriber were inserted between the count above and this select. The guard above
    // is what makes that limit unreachable rather than a silent truncation.
    let subscribers = SubscriberRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM webhooks WHERE workspace_id = $1 AND active = true AND events ? $2 \
         ORDER BY id LIMIT $3",
        vec![
            work.workspace_id.into(),
            work.event_type.clone().into(),
            SUBSCRIBERS_PER_WORKSPACE_MAX.into(),
        ],
    ))
    .all(&tx)
    .await?;

    if subscribers.is_empty() {
        let affected = tx
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE event_dispatch SET status = 'no_subscribers', expanded_at = now(), \
                 lease_token = NULL, lease_expires_at = NULL WHERE id = $1 AND lease_token = $2",
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
            "UPDATE event_dispatch SET status = 'expanded', expanded_at = now(), \
             lease_token = NULL, lease_expires_at = NULL WHERE id = $1 AND lease_token = $2",
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

    if step_b_fault_armed() {
        // Test-only: step (a) above already ran on `tx`; returning `Err` here forces the whole
        // transaction to roll back without a `COMMIT`, exactly like a worker dying mid-expansion.
        return Err(ApiError::Conflict("test_injected_expansion_step_b_failure".to_string()));
    }

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

#[cfg(test)]
tokio::task_local! {
    static TEST_ALLOW_PRIVATE_DELIVERY_TARGET: bool;
    static TEST_REPLAY_CANDIDATE_BARRIER: std::sync::Arc<tokio::sync::Barrier>;
}

async fn validate_delivery_target(raw: &str) -> Result<reqwest::Url, String> {
    #[cfg(test)]
    if TEST_ALLOW_PRIVATE_DELIVERY_TARGET
        .try_with(|allow| *allow)
        .unwrap_or(false)
    {
        return reqwest::Url::parse(raw).map_err(|error| error.to_string());
    }
    validate_outbound_url(raw).await
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

    let target = match validate_delivery_target(&webhook.url).await {
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
    let backoff_ms = delivery_backoff_ms(next_attempts);

    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE event_deliveries
                SET attempts = $2,
                    status = $3,
                    terminated_at = CASE WHEN $3 = 'failed' THEN now() ELSE NULL END,
                    next_attempt_at = now() + ($4::bigint * interval '1 millisecond'),
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

/// `events-v1.md` "每个 Flow event type 必须在 payload policy registry 中显式声明；未声明的
/// producer 测试失败，未知 payload 在 delivery 时整体 withheld": every payload reaching a webhook
/// body goes through `flow::event_policy` here, not just events whose type happens to start with
/// `flow.` -- a non-Flow producer's event type simply has no registry entry and is withheld the
/// same fail-closed way, which is the intended behavior (this dispatcher is a shared platform
/// component; `flow::event_policy` documents its policies are Flow's v0.4 registry, not the only
/// one that will ever exist here).
fn envelope_json(
    event: &BusinessEventRow,
    event_id_override: Option<Uuid>,
    created_at_override: Option<DateTime<Utc>>,
    payload_override: Option<Value>,
) -> Value {
    let raw_payload = payload_override.unwrap_or_else(|| event.payload.clone());
    let payload = crate::flow::event_policy::redact_flow_event_payload_for_delivery(&event.event_type, &raw_payload);
    let metadata = crate::flow::event_policy::redact_flow_event_metadata_for_delivery(&event.metadata);
    json!({
        "version": "openpr.event.v1",
        "event_id": event_id_override.unwrap_or(event.id),
        "event_type": event.event_type,
        "workspace_id": event.workspace_id,
        "project_id": event.project_id,
        "aggregate": { "type": event.aggregate_type, "id": event.aggregate_id },
        "actor_id": event.actor_id,
        "source": event.source,
        "payload": payload,
        "metadata": metadata,
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

/// Takes `now` as an explicit argument (rather than letting the `DELETE` read Postgres's own
/// `now()`) so callers — in particular exact-boundary tests — can pin the *one* reference instant
/// used both to compute a backdated anchor column and to evaluate this predicate against it. Two
/// independent `now()` calls (one in the test's `UPDATE`, one in this `DELETE`) would always have
/// drifted apart by at least the round-trip between them, which makes "exactly at the boundary"
/// unobservable. `run_tick` passes real wall-clock time; nothing else changes for production use.
async fn reap_dispatch_retention(db: &DatabaseConnection, now: DateTime<Utc>) -> Result<u64, ApiError> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                DELETE FROM event_dispatch
                WHERE lease_token IS NULL
                  AND (
                    (status = 'no_subscribers' AND expanded_at < $4::timestamptz - ($1::bigint * interval '1 hour'))
                    OR (status = 'expanded' AND expanded_at < $4::timestamptz - ($2::bigint * interval '1 day'))
                    OR (status = 'failed' AND expanded_at < $4::timestamptz - ($3::bigint * interval '1 day'))
                  )
            ",
            vec![
                DISPATCH_NO_SUBSCRIBERS_RETENTION_HOURS.into(),
                DISPATCH_EXPANDED_RETENTION_DAYS.into(),
                DISPATCH_FAILED_RETENTION_DAYS.into(),
                now.into(),
            ],
        ))
        .await?;
    Ok(result.rows_affected())
}

/// See [`reap_dispatch_retention`]'s doc comment for why `now` is an explicit argument.
async fn reap_delivery_retention(db: &DatabaseConnection, now: DateTime<Utc>) -> Result<u64, ApiError> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                DELETE FROM event_deliveries
                WHERE lease_token IS NULL
                  AND status IN ('dispatched', 'failed', 'cancelled')
                  AND terminated_at < $2::timestamptz - ($1::bigint * interval '1 day')
            ",
            vec![DELIVERY_RETENTION_DAYS.into(), now.into()],
        ))
        .await?;
    Ok(result.rows_affected())
}

/// `event_delivery_sources` has no `status`/`lease_token` (`events-v1.md` "reaper 的谓词只能删
/// 终态": "`event_delivery_sources` 无 status，用它自己的 `delivery_id IS NULL` 谓词"), so its
/// reaper's predicate is not the `WHERE status IN (...) AND <anchor> < now() - retention` shape
/// [`reap_dispatch_retention`]/[`reap_delivery_retention`] share -- it is the tombstone-specific
/// one the contract writes out verbatim: `WHERE created_at < now() - delivery_source_retention_days
/// AND delivery_id IS NULL`. A row whose `delivery_id` is still non-NULL is not a tombstone yet --
/// the delivery it names has not been retention-reaped -- and deleting it here would destroy the
/// `(subscriber_kind, subscriber_id, source_event_id)` dedup evidence a still-live delivery
/// depends on, so `delivery_id IS NULL` is not an optimization, it is the entire safety property.
async fn reap_delivery_source_tombstones(db: &DatabaseConnection, now: DateTime<Utc>) -> Result<u64, ApiError> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                DELETE FROM event_delivery_sources
                WHERE delivery_id IS NULL
                  AND created_at < $2::timestamptz - ($1::bigint * interval '1 day')
            ",
            vec![DELIVERY_SOURCE_RETENTION_DAYS.into(), now.into()],
        ))
        .await?;
    Ok(result.rows_affected())
}

/// Revives dead-lettered (`status='failed'`) `event_deliveries` rows for a workspace back into the
/// live queue, `events-v1.md`/`gate-commands.md`'s `requeue_failed` mode.
///
/// Clears `document_id` on every revived row -- content or not -- per the schema's own comment on
/// `event_deliveries.document_id` ("NULL ... for replay/requeue_failed-revived rows (deliberately,
/// so the partial coalescing unique index never applies to them)") and `gate-commands.md`'s
/// negative fixture: reviving a content delivery *without* clearing `document_id` lets it re-enter
/// `idx_event_deliveries_coalesce_pending`'s `(subscriber_kind, subscriber_id, document_id) WHERE
/// status='pending'` partial unique index under its original `document_id`, where it can collide
/// with a live pending row for the same document, or -- if there is none yet -- squat that
/// document's FIFO coalescing slot with a resurrected retry ahead of genuinely new real-time
/// updates.
///
/// Never revives `cancelled` rows: that status is `subscriber_gone`'s deliberate terminal state
/// (the subscription itself was deleted), not a retryable failure, and `events-v1.md` requires it
/// stays that way -- reviving a cancelled row would resurrect deliveries for a subscriber that no
/// longer exists.
///
/// Clears `terminated_at` back to `NULL`: the schema's own comment on that column says "只有
/// `requeue_failed` 可以清空它（复活即离开终态），其余路径不得改写" -- required for
/// `event_deliveries_terminated_at_check` (`status IN ('dispatched','failed','cancelled') =
/// (terminated_at IS NOT NULL)`) to still hold once `status` becomes `pending`.
///
/// `pub`: the v0.4 scope this module ships is the dispatcher primitive itself, proven against a
/// real database below; wiring it to an operator-facing REST/CLI surface is `events-v1.md`'s
/// fuller v0.8 `mode=requeue_failed` admin replay feature (`"v0.8 的 admin replay ... 全程不经过
/// event_dispatch"`), out of this task's scope.
pub async fn requeue_failed(db: &DatabaseConnection, workspace_id: Uuid, now: DateTime<Utc>) -> Result<u64, ApiError> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE event_deliveries
                SET status = 'pending',
                    document_id = NULL,
                    terminated_at = NULL,
                    attempts = 0,
                    next_attempt_at = $2,
                    lease_token = NULL,
                    lease_expires_at = NULL,
                    last_error_code = NULL
                WHERE workspace_id = $1 AND status = 'failed'
            ",
            vec![workspace_id.into(), now.into()],
        ))
        .await?;
    Ok(result.rows_affected())
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReplayMode {
    Rebuild,
    RequeueFailed,
}

#[derive(Debug, Clone)]
pub struct ReplayRequest {
    pub workspace_id: Uuid,
    pub mode: ReplayMode,
    pub event_type: Option<String>,
    pub subscriber_kind: Option<String>,
    pub subscriber_id: Option<Uuid>,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ReplayWindow {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ReplayResult {
    Rebuild {
        replayed: u64,
        skipped_already_delivered: u64,
        rebuilt_delivery_ids: Vec<Uuid>,
        window: ReplayWindow,
    },
    RequeueFailed {
        requeued: u64,
        skipped_not_failed: u64,
        requeued_delivery_ids: Vec<Uuid>,
        window: ReplayWindow,
    },
}

#[derive(Debug, FromQueryResult)]
struct ReplayCandidate {
    event_id: Uuid,
    subscriber_id: Uuid,
    already_delivered: bool,
}

#[derive(Debug, FromQueryResult)]
struct RequeueCandidate {
    id: Uuid,
    status: String,
}

fn validate_replay_window(request: &ReplayRequest, now: DateTime<Utc>) -> Result<(), ApiError> {
    if request.from >= request.to || request.to > now {
        return Err(ApiError::BadRequest(
            "replay window must be non-empty, half-open, and not extend into the future".to_string(),
        ));
    }
    let oldest = now - chrono::Duration::days(REPLAY_MAX_WINDOW_DAYS);
    if request.from <= oldest {
        return Err(ApiError::BadRequest(
            "replay window is outside replay_max_window_days".to_string(),
        ));
    }
    if request.subscriber_kind.as_deref().is_some_and(|kind| kind != "webhook") {
        return Err(ApiError::BadRequest("unsupported subscriber_kind".to_string()));
    }
    Ok(())
}

/// Rebuilds missing webhook deliveries or revives failed deliveries inside one exact window.
///
/// `rebuild` filters source events by `business_events.created_at`, reserves the existing
/// `(subscriber, source_event)` dedup key before it creates a delivery, and binds both rows in one
/// transaction. `requeue_failed` instead filters `event_deliveries.terminated_at` and preserves
/// each delivery id. Dry-run executes the same candidate queries but performs zero writes.
pub async fn replay_deliveries(
    db: &DatabaseConnection,
    request: &ReplayRequest,
    now: DateTime<Utc>,
) -> Result<ReplayResult, ApiError> {
    validate_replay_window(request, now)?;
    let window = ReplayWindow {
        from: request.from,
        to: request.to,
    };
    match request.mode {
        ReplayMode::Rebuild => {
            let candidates = ReplayCandidate::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                    SELECT be.id AS event_id, wh.id AS subscriber_id,
                           EXISTS (
                             SELECT 1 FROM event_delivery_sources src
                              WHERE src.subscriber_kind = 'webhook'
                                AND src.subscriber_id = wh.id
                                AND src.source_event_id = be.id
                           ) AS already_delivered
                      FROM business_events be
                      JOIN webhooks wh ON wh.workspace_id = be.workspace_id
                                      AND wh.active = true
                                      AND wh.events ? be.event_type
                     WHERE be.workspace_id = $1
                       AND be.created_at >= $2 AND be.created_at < $3
                       AND ($4::text IS NULL OR be.event_type = $4)
                       AND ($5::uuid IS NULL OR wh.id = $5)
                     ORDER BY be.created_at, be.id, wh.id
                ",
                vec![
                    request.workspace_id.into(),
                    request.from.into(),
                    request.to.into(),
                    request.event_type.clone().into(),
                    request.subscriber_id.into(),
                ],
            ))
            .all(db)
            .await?;
            #[cfg(test)]
            if let Ok(barrier) = TEST_REPLAY_CANDIDATE_BARRIER.try_with(Clone::clone) {
                barrier.wait().await;
            }
            let skipped = candidates.iter().filter(|row| row.already_delivered).count() as u64;
            let planned = candidates.len() as u64 - skipped;
            if request.dry_run {
                return Ok(ReplayResult::Rebuild {
                    replayed: planned,
                    skipped_already_delivered: skipped,
                    rebuilt_delivery_ids: Vec::new(),
                    window,
                });
            }

            let tx = db.begin().await?;
            let mut rebuilt = Vec::new();
            let mut concurrent_skips = 0_u64;
            for candidate in candidates.into_iter().filter(|row| !row.already_delivered) {
                let reserved = ReservedSourceRow::find_by_statement(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "INSERT INTO event_delivery_sources \
                       (workspace_id, delivery_id, subscriber_kind, subscriber_id, source_event_id) \
                     VALUES ($1, NULL, 'webhook', $2, $3) \
                     ON CONFLICT (subscriber_kind, subscriber_id, source_event_id) DO NOTHING \
                     RETURNING id",
                    vec![
                        request.workspace_id.into(),
                        candidate.subscriber_id.into(),
                        candidate.event_id.into(),
                    ],
                ))
                .one(&tx)
                .await?;
                let Some(reserved) = reserved else {
                    concurrent_skips += 1;
                    continue;
                };
                let delivery_id = Uuid::new_v4();
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "INSERT INTO event_deliveries \
                       (id, dispatch_id, event_id, workspace_id, subscriber_kind, subscriber_id, \
                        document_id, status, next_attempt_at) \
                     VALUES ($1, NULL, $2, $3, 'webhook', $4, NULL, 'pending', $5)",
                    vec![
                        delivery_id.into(),
                        candidate.event_id.into(),
                        request.workspace_id.into(),
                        candidate.subscriber_id.into(),
                        now.into(),
                    ],
                ))
                .await?;
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE event_delivery_sources SET delivery_id = $1 WHERE id = $2",
                    vec![delivery_id.into(), reserved.id.into()],
                ))
                .await?;
                rebuilt.push(delivery_id);
            }
            tx.commit().await?;
            Ok(ReplayResult::Rebuild {
                replayed: rebuilt.len() as u64,
                skipped_already_delivered: skipped + concurrent_skips,
                rebuilt_delivery_ids: rebuilt,
                window,
            })
        }
        ReplayMode::RequeueFailed => {
            let candidates = RequeueCandidate::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                    SELECT d.id, d.status
                      FROM event_deliveries d
                      JOIN business_events be ON be.id = d.event_id
                     WHERE d.workspace_id = $1
                       AND d.terminated_at >= $2 AND d.terminated_at < $3
                       AND ($4::text IS NULL OR be.event_type = $4)
                       AND ($5::uuid IS NULL OR d.subscriber_id = $5)
                     ORDER BY d.terminated_at, d.id
                ",
                vec![
                    request.workspace_id.into(),
                    request.from.into(),
                    request.to.into(),
                    request.event_type.clone().into(),
                    request.subscriber_id.into(),
                ],
            ))
            .all(db)
            .await?;
            let ids = candidates
                .iter()
                .filter(|row| row.status == "failed")
                .map(|row| row.id)
                .collect::<Vec<_>>();
            let skipped = candidates.len() as u64 - ids.len() as u64;
            if !request.dry_run && !ids.is_empty() {
                db.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE event_deliveries \
                        SET status='pending', document_id=NULL, terminated_at=NULL, attempts=0, \
                            next_attempt_at=$2, lease_token=NULL, lease_expires_at=NULL, last_error_code=NULL \
                      WHERE workspace_id=$1 AND status='failed' AND id = ANY($3)",
                    vec![request.workspace_id.into(), now.into(), ids.clone().into()],
                ))
                .await?;
            }
            Ok(ReplayResult::RequeueFailed {
                requeued: ids.len() as u64,
                skipped_not_failed: skipped,
                requeued_delivery_ids: if request.dry_run { Vec::new() } else { ids },
                window,
            })
        }
    }
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
    use chrono::{DateTime, Utc};
    use platform::{
        app::AppState,
        config::{AppConfig, Secret},
    };
    use sea_orm::{
        ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait,
    };
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use uuid::Uuid;

    use super::{
        BusinessEventRow, DISPATCHER_LIVENESS_MAX_SILENCE_MS, ExpansionOutcome, FAIL_EXPANSION_STEP_B,
        OLDEST_PENDING_AGE_ALERT_MS, REPLAY_MAX_WINDOW_DAYS, ReplayMode, ReplayRequest, ReplayResult,
        SUBSCRIBERS_PER_WORKSPACE_MAX, TEST_ALLOW_PRIVATE_DELIVERY_TARGET, TEST_REPLAY_CANDIDATE_BARRIER,
        backlog_alert, build_delivery_body, delivery_backoff_ms, dispatcher_is_live, dispatcher_is_live_since,
        ensure_workspace_subscriber_slot, envelope_json, expand_one, oldest_pending_delivery_age_ms,
        oldest_pending_dispatch_age_ms, reap_delivery_retention, reap_delivery_source_tombstones,
        reclaim_expired_delivery_leases, reclaim_expired_dispatch_leases, replay_deliveries, requeue_failed, run_tick,
        send_one, workspace_subscriber_count,
    };
    use crate::error::ApiError;
    use crate::events::{BusinessEventInput, insert_business_event};

    const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

    #[test]
    fn flow_delivery_retry_backoff_matches_every_frozen_attempt() {
        assert_eq!(
            (1..=10).map(delivery_backoff_ms).collect::<Vec<_>>(),
            vec![
                30_000, 60_000, 90_000, 120_000, 150_000, 180_000, 210_000, 240_000, 270_000, 300_000
            ]
        );
    }

    #[test]
    fn delivery_envelope_redacts_record_content_from_metadata() {
        let event = BusinessEventRow {
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            project_id: None,
            event_type: "flow.record.created".to_string(),
            aggregate_type: "flow_record".to_string(),
            aggregate_id: Uuid::new_v4().to_string(),
            actor_id: Some(Uuid::new_v4()),
            source: json!({"surface": "rest"}),
            payload: json!({"collection_id": "collection", "record_id": "record"}),
            metadata: json!({
                "idempotency_body": {
                    "properties": {"salary": "SECRET-SALARY-9001"},
                    "body": "SECRET-BODY-NOTE"
                },
                "idempotency_fingerprint": "internal",
                "message": "SECRET-MESSAGE"
            }),
            correlation_id: None,
            causation_id: None,
            created_at: Utc::now(),
        };

        let envelope = envelope_json(&event, None, None, None);
        let encoded = serde_json::to_string(&envelope).expect("envelope serializes");
        assert_eq!(envelope["metadata"], json!({}));
        for secret in ["SECRET-SALARY-9001", "SECRET-BODY-NOTE", "SECRET-MESSAGE"] {
            assert!(!encoded.contains(secret), "delivery envelope leaked {secret}");
        }
    }

    struct Scratch {
        db: DatabaseConnection,
        name: String,
        admin_url: String,
        url: String,
    }

    impl Scratch {
        async fn drop_self(self) {
            let Self {
                db,
                name,
                admin_url,
                url: _,
            } = self;
            drop(db);
            let Ok(admin) = Database::connect(&admin_url).await else {
                return;
            };
            let _ = admin
                .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                .await;
        }

        /// A second, independent connection to the same scratch database — needed to prove
        /// something about cross-connection concurrency (a held lock, a simultaneous lease race)
        /// that a single `DatabaseConnection`'s own pool cannot observe from the inside.
        async fn second_connection(&self) -> DatabaseConnection {
            Database::connect(&self.url)
                .await
                .unwrap_or_else(|err| panic!("could not open a second connection to {}: {err}", self.name))
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
        Some(Scratch {
            db,
            name,
            admin_url,
            url,
        })
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
            flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
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

    /// Reads a single named column off the one row a query returns. Generic sibling of [`count`]
    /// for everything that isn't a `count(*)`.
    async fn get_col<T: sea_orm::TryGetable>(
        db: &DatabaseConnection,
        sql: &str,
        values: Vec<sea_orm::Value>,
        col: &str,
    ) -> T {
        db.query_one(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .await
            .expect("query runs")
            .expect("row exists")
            .try_get("", col)
            .expect("column reads")
    }

    /// This suite's **clock-advancement primitive**: backdates one timestamp column on the row
    /// matched by `id_column = id` by `offset` (positive moves it further into the past). Every
    /// retention-boundary / backlog-age test below reaches its boundary by rewriting a row's own
    /// timestamp this way — never by sleeping real wall-clock time. `table`/`timestamp_column`/
    /// `id_column` are always `&'static str` literals fixed by the call site, never caller input.
    async fn backdate(
        db: &DatabaseConnection,
        table: &'static str,
        timestamp_column: &'static str,
        id_column: &'static str,
        id: Uuid,
        offset: chrono::Duration,
    ) {
        exec(
            db,
            &format!(
                "UPDATE {table} SET {timestamp_column} = now() - ($2::bigint * interval '1 millisecond') \
                 WHERE {id_column} = $1"
            ),
            vec![id.into(), offset.num_milliseconds().into()],
        )
        .await;
    }

    /// [`backdate`]'s sibling for *exact*-boundary fixtures: sets a timestamp column to a literal
    /// value computed on the Rust side, rather than relative to Postgres's own `now()`. Pair this
    /// with the retention reapers' explicit `now` argument so the test's write and the reaper's
    /// read share one frozen reference instant instead of two independent, always-slightly-drifted
    /// `now()` calls — required for asserting "exactly at the boundary survives, one ms past it
    /// does not" deterministically.
    async fn set_timestamp(
        db: &DatabaseConnection,
        table: &'static str,
        timestamp_column: &'static str,
        id_column: &'static str,
        id: Uuid,
        value: chrono::DateTime<Utc>,
    ) {
        exec(
            db,
            &format!("UPDATE {table} SET {timestamp_column} = $2 WHERE {id_column} = $1"),
            vec![id.into(), value.into()],
        )
        .await;
    }

    /// Deletes a webhook outright — the "subscriber deleted" half of the snapshot-semantics gate,
    /// distinguished from the `active` toggle case, which is exercised separately.
    async fn delete_webhook(db: &DatabaseConnection, webhook_id: Uuid) {
        exec(db, "DELETE FROM webhooks WHERE id = $1", vec![webhook_id.into()]).await;
    }

    /// Drains `expand_one` against `db` until nothing is left to lease, counting how many work
    /// items this task personally expanded. Used to run several simulated dispatcher instances
    /// concurrently against one shared backlog.
    async fn drain_expand_one(db: &DatabaseConnection) -> u32 {
        let mut expanded = 0u32;
        loop {
            match expand_one(db).await {
                Ok(Some(ExpansionOutcome::Expanded | ExpansionOutcome::NoSubscribers)) => expanded += 1,
                Ok(Some(ExpansionOutcome::StaleLease)) => {}
                Ok(None) | Err(_) => break,
            }
        }
        expanded
    }

    /// [`drain_expand_one`]'s sibling for the send side.
    async fn drain_send_one(state: AppState) -> u32 {
        let client = reqwest::Client::new();
        let mut sent = 0u32;
        while let Ok(Some(_)) = send_one(&state, &client).await {
            sent += 1;
        }
        sent
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

    /// The `subscribers_per_workspace_max` ceiling, both halves, on the path the contract's row
    /// describes ("dispatcher fan-out 的放大边界"): a workspace holding exactly
    /// `SUBSCRIBERS_PER_WORKSPACE_MAX` active subscribers expands normally and produces exactly
    /// that many `event_deliveries` rows, and one subscriber past it makes the expansion refuse
    /// with `limit_exceeded{limit_kind:"workspace_subscribers"}` instead of quietly expanding a
    /// truncated subset. The `+1` arm writes the extra subscriber straight to the table, because
    /// `ensure_workspace_subscriber_slot` refuses it through the registration path -- the state
    /// under test is one only pre-ceiling rows (or a write around the API) can produce.
    #[tokio::test]
    async fn workspace_subscribers_exact_boundary_expands_and_the_next_subscriber_is_rejected_with_the_frozen_limit_kind()
     {
        let scratch = scratch_or_skip!("workspace-subscribers-ceiling");
        let workspace_id = seed_workspace(&scratch.db).await;
        const CEILING: i64 = SUBSCRIBERS_PER_WORKSPACE_MAX;

        for _ in 0..CEILING {
            seed_webhook(
                &scratch.db,
                workspace_id,
                "http://example.invalid/hook",
                &["flow.object.created"],
            )
            .await;
        }
        assert_eq!(
            workspace_subscriber_count(&scratch.db, workspace_id)
                .await
                .expect("subscriber count query runs"),
            CEILING,
            "the exact ceiling must be reachable, not one short of it"
        );

        // Exact boundary: accepted, and the fan-out is exactly the ceiling.
        let at_ceiling =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
        let outcome = expand_one(&scratch.db)
            .await
            .expect("expansion at the exact ceiling runs")
            .expect("the committed work item is pending");
        assert!(matches!(outcome, ExpansionOutcome::Expanded), "{outcome:?}");
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
                vec![at_ceiling.into()],
            )
            .await,
            CEILING,
            "one work item at the ceiling must expand into exactly SUBSCRIBERS_PER_WORKSPACE_MAX deliveries"
        );

        // Boundary + 1.
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created"],
        )
        .await;
        let past_ceiling =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
        let err = expand_one(&scratch.db)
            .await
            .expect_err("expansion must refuse a workspace past subscribers_per_workspace_max");
        let ApiError::Typed { kind, details, .. } = &err else {
            panic!("expected a typed limit_exceeded rejection, got {err:?}");
        };
        assert_eq!(kind.stable_code(), "limit_exceeded");
        let details = details.as_ref().expect("limit_exceeded always carries details");
        assert_eq!(details["limit_kind"], json!("workspace_subscribers"));
        assert_eq!(details["limit"], json!(CEILING));
        assert_eq!(details["observed"], json!(CEILING + 1));

        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
                vec![past_ceiling.into()],
            )
            .await,
            0,
            "a refused expansion must not write a single delivery row"
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_delivery_sources WHERE source_event_id = $1",
                vec![past_ceiling.into()],
            )
            .await,
            0,
            "a refused expansion must not reserve a single source row either"
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM business_events WHERE id = $1",
                vec![past_ceiling.into()],
            )
            .await,
            1,
            "the canonical business_events row is untouched by a refused expansion"
        );
        let status: String = get_col(
            &scratch.db,
            "SELECT status FROM event_dispatch WHERE event_id = $1",
            vec![past_ceiling.into()],
            "status",
        )
        .await;
        assert_eq!(
            status, "pending",
            "a refused expansion goes back to pending, not expanded"
        );
        let attempts: i32 = get_col(
            &scratch.db,
            "SELECT attempts FROM event_dispatch WHERE event_id = $1",
            vec![past_ceiling.into()],
            "attempts",
        )
        .await;
        assert_eq!(
            attempts, 1,
            "the refusal is charged as one expansion attempt, so it dead-letters instead of spinning"
        );

        scratch.drop_self().await;
    }

    /// The registration half of the same ceiling: the subscriber that lands exactly on
    /// `SUBSCRIBERS_PER_WORKSPACE_MAX` is accepted, the next one is refused with the frozen
    /// `limit_kind`, an already-active subscriber can still be re-saved at the ceiling, and an
    /// inactive webhook does not occupy a slot (the dispatcher's directory query requires
    /// `active = true`, so neither may this).
    #[tokio::test]
    async fn workspace_subscribers_registration_ceiling_rejects_the_subscriber_past_the_frozen_maximum() {
        let scratch = scratch_or_skip!("workspace-subscribers-registration");
        let workspace_id = seed_workspace(&scratch.db).await;
        const CEILING: i64 = SUBSCRIBERS_PER_WORKSPACE_MAX;

        let mut last = Uuid::nil();
        for _ in 0..(CEILING - 1) {
            last = seed_webhook(
                &scratch.db,
                workspace_id,
                "http://example.invalid/hook",
                &["flow.object.created"],
            )
            .await;
        }
        ensure_workspace_subscriber_slot(&scratch.db, workspace_id, None)
            .await
            .expect("the subscriber landing exactly on the ceiling is accepted");

        let at_ceiling = seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created"],
        )
        .await;
        let err = ensure_workspace_subscriber_slot(&scratch.db, workspace_id, None)
            .await
            .expect_err("the subscriber past the ceiling is refused");
        let ApiError::Typed { kind, details, .. } = &err else {
            panic!("expected a typed limit_exceeded rejection, got {err:?}");
        };
        assert_eq!(kind.stable_code(), "limit_exceeded");
        let details = details.as_ref().expect("limit_exceeded always carries details");
        assert_eq!(details["limit_kind"], json!("workspace_subscribers"));
        assert_eq!(details["limit"], json!(CEILING));
        assert_eq!(details["observed"], json!(CEILING + 1));

        ensure_workspace_subscriber_slot(&scratch.db, workspace_id, Some(at_ceiling))
            .await
            .expect("re-saving an already-active subscriber at the ceiling is not a new slot");

        exec(
            &scratch.db,
            "UPDATE webhooks SET active = false WHERE id = $1",
            vec![last.into()],
        )
        .await;
        ensure_workspace_subscriber_slot(&scratch.db, workspace_id, None)
            .await
            .expect("an inactive webhook is not a subscriber and frees its slot");

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
    async fn delivery_attempts_one_through_ten_write_the_frozen_database_backoff_and_never_attempt_eleven() {
        async fn read_request(socket: &mut tokio::net::TcpStream) {
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0_u8; 4096];
                let read = socket.read(&mut chunk).await.expect("request is readable");
                assert!(read > 0, "request ended before its declared body");
                bytes.extend_from_slice(&chunk[..read]);
                let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().expect("content-length is numeric"))
                    })
                    .unwrap_or(0);
                if bytes.len() >= header_end + 4 + content_length {
                    return;
                }
            }
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("loopback listener binds");
        let address = listener.local_addr().expect("listener address is available");
        let receiver = tokio::spawn(async move {
            for _ in 1..=10 {
                let (mut socket, _) = listener.accept().await.expect("delivery connection arrives");
                read_request(&mut socket).await;
                socket
                    .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await
                    .expect("failure response writes");
            }
        });

        let scratch = scratch_or_skip!("delivery-backoff-database");
        let workspace_id = seed_workspace(&scratch.db).await;
        let webhook_id = seed_webhook(
            &scratch.db,
            workspace_id,
            &format!("http://{address}/hook"),
            &["flow.object.created"],
        )
        .await;
        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
        exec(
            &scratch.db,
            "DELETE FROM event_dispatch WHERE event_id=$1",
            vec![event_id.into()],
        )
        .await;
        let now = Utc::now();
        let replay = replay_deliveries(
            &scratch.db,
            &ReplayRequest {
                workspace_id,
                mode: ReplayMode::Rebuild,
                event_type: Some("flow.object.created".to_string()),
                subscriber_kind: Some("webhook".to_string()),
                subscriber_id: Some(webhook_id),
                from: now - chrono::Duration::minutes(1),
                to: now,
                dry_run: false,
            },
            now,
        )
        .await
        .expect("admin replay builds a delivery");
        let delivery_id = match replay {
            ReplayResult::Rebuild {
                replayed: 1,
                rebuilt_delivery_ids,
                ..
            } => rebuilt_delivery_ids[0],
            other => panic!("unexpected replay result: {other:?}"),
        };
        #[derive(FromQueryResult)]
        struct ReplayShape {
            dispatch_id: Option<Uuid>,
            document_id: Option<Uuid>,
            first_seq: Option<i64>,
            latest_seq: Option<i64>,
            status: String,
        }
        let replay_shape = ReplayShape::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT dispatch_id,document_id,first_seq,latest_seq,status FROM event_deliveries WHERE id=$1",
            vec![delivery_id.into()],
        ))
        .one(&scratch.db)
        .await
        .expect("replay shape query runs")
        .expect("replay delivery exists");
        assert_eq!(
            (
                replay_shape.dispatch_id,
                replay_shape.document_id,
                replay_shape.first_seq,
                replay_shape.latest_seq,
                replay_shape.status.as_str(),
            ),
            (None, None, None, None, "pending"),
            "replay rows bypass event_dispatch and cannot enter live document coalescing"
        );
        let state = state_for(scratch.db.clone());
        let client = reqwest::Client::new();

        #[derive(FromQueryResult)]
        struct RetryRow {
            status: String,
            attempts: i32,
            next_attempt_at: DateTime<Utc>,
        }
        for attempt in 1_i64..=10 {
            exec(
                &scratch.db,
                "UPDATE event_deliveries SET next_attempt_at=now() WHERE id=$1",
                vec![delivery_id.into()],
            )
            .await;
            let before: DateTime<Utc> = get_col(&scratch.db, "SELECT clock_timestamp() AS ts", vec![], "ts").await;
            let outcome = TEST_ALLOW_PRIVATE_DELIVERY_TARGET
                .scope(true, send_one(&state, &client))
                .await
                .expect("delivery attempt runs");
            assert_eq!(outcome, Some(false), "HTTP 503 is a handled failed attempt");
            let after: DateTime<Utc> = get_col(&scratch.db, "SELECT clock_timestamp() AS ts", vec![], "ts").await;
            let row = RetryRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT status,attempts,next_attempt_at FROM event_deliveries WHERE id=$1",
                vec![delivery_id.into()],
            ))
            .one(&scratch.db)
            .await
            .expect("retry row query runs")
            .expect("retry row remains present");
            let expected = chrono::Duration::milliseconds((attempt * 30_000).min(300_000));
            assert!(
                row.next_attempt_at >= before + expected && row.next_attempt_at <= after + expected,
                "attempt {attempt} wrote {} outside the database-observed [{}, {}] schedule",
                row.next_attempt_at,
                before + expected,
                after + expected
            );
            assert_eq!(i64::from(row.attempts), attempt);
            assert_eq!(row.status, if attempt == 10 { "failed" } else { "pending" });
        }
        receiver.await.expect("receiver handled exactly ten attempts");
        assert_eq!(
            TEST_ALLOW_PRIVATE_DELIVERY_TARGET
                .scope(true, send_one(&state, &client))
                .await
                .expect("post-exhaustion dispatcher scan runs"),
            None,
            "a failed row cannot produce attempt eleven"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_delivery_failure_then_fresh_dispatcher_delivers_once_with_the_same_delivery_id() {
        async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0_u8; 4096];
                let read = socket.read(&mut chunk).await.expect("request is readable");
                assert!(read > 0, "request ended before its declared body");
                bytes.extend_from_slice(&chunk[..read]);
                let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().expect("content-length is numeric"))
                    })
                    .unwrap_or(0);
                if bytes.len() >= header_end + 4 + content_length {
                    return String::from_utf8(bytes).expect("delivery request is UTF-8");
                }
            }
        }

        const CHILD_ROLE: &str = "OPENPR_DELIVERY_ATTEMPT_CHILD";
        const CHILD_DATABASE_URL: &str = "OPENPR_DELIVERY_ATTEMPT_DATABASE_URL";

        // The same libtest binary is the dispatcher executable for this fixture. The parent starts
        // it as a separate OS process and kills that whole process after the receiver has read the
        // request but before it responds. A fresh process then reclaims the durable lease and sends
        // the same row again. This is deliberately not a task abort or a fresh in-process client.
        if std::env::var_os(CHILD_ROLE).is_some() {
            let database_url = std::env::var(CHILD_DATABASE_URL).expect("child database URL is set");
            let db = Database::connect(database_url)
                .await
                .expect("child connects to scratch database");
            let result = TEST_ALLOW_PRIVATE_DELIVERY_TARGET
                .scope(true, send_one(&state_for(db.clone()), &reqwest::Client::new()))
                .await
                .expect("child dispatcher attempt runs");
            assert!(result.is_some(), "child must lease one delivery");
            db.close().await.expect("child closes its database pool");
            return;
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("loopback listener binds");
        let address = listener.local_addr().expect("listener address is available");
        let (first_request_started_tx, first_request_started_rx) = tokio::sync::oneshot::channel();
        let receiver = tokio::spawn(async move {
            let mut requests = Vec::new();
            let (mut first_socket, _) = listener.accept().await.expect("first delivery connection arrives");
            requests.push(read_request(&mut first_socket).await);
            first_request_started_tx
                .send(())
                .expect("parent still waits for the in-flight attempt");
            let mut eof = [0_u8; 1];
            assert_eq!(
                first_socket.read(&mut eof).await.expect("killed child closes socket"),
                0
            );

            let (mut second_socket, _) = listener.accept().await.expect("restarted delivery connection arrives");
            requests.push(read_request(&mut second_socket).await);
            second_socket
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .expect("success response writes");
            second_socket.flush().await.expect("success response flushes");
            requests
        });

        let scratch = scratch_or_skip!("failure-restart-success");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            &format!("http://{address}/hook"),
            &["flow.object.created"],
        )
        .await;
        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
        expand_one(&scratch.db).await.expect("expansion runs");
        let delivery_id: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_deliveries WHERE event_id=$1",
            vec![event_id.into()],
            "id",
        )
        .await;

        let test_name = "events::dispatcher::dispatcher_database_tests::flow_delivery_failure_then_fresh_dispatcher_delivers_once_with_the_same_delivery_id";
        let mut killed_dispatcher =
            tokio::process::Command::new(std::env::current_exe().expect("test executable exists"))
                .arg("--exact")
                .arg(test_name)
                .arg("--nocapture")
                .env(CHILD_ROLE, "in-flight")
                .env(CHILD_DATABASE_URL, &scratch.url)
                .kill_on_drop(true)
                .spawn()
                .expect("dispatcher child starts");
        first_request_started_rx
            .await
            .expect("receiver observed the in-flight attempt");
        killed_dispatcher
            .kill()
            .await
            .expect("whole dispatcher process is killed");
        let killed_status = killed_dispatcher.wait().await.expect("killed dispatcher is reaped");
        assert!(!killed_status.success(), "the first dispatcher really died mid-attempt");

        #[derive(FromQueryResult)]
        struct LeasedRow {
            id: Uuid,
            status: String,
            lease_token: Option<String>,
        }
        let interrupted = LeasedRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id,status,lease_token FROM event_deliveries WHERE id=$1",
            vec![delivery_id.into()],
        ))
        .one(&scratch.db)
        .await
        .expect("interrupted row query runs")
        .expect("interrupted delivery remains durable");
        assert_eq!(interrupted.id, delivery_id);
        assert_eq!(interrupted.status, "leased");
        assert!(
            interrupted.lease_token.is_some(),
            "the killed attempt left its durable lease"
        );
        exec(
            &scratch.db,
            "UPDATE event_deliveries SET lease_expires_at=now()-interval '1 millisecond' WHERE id=$1",
            vec![delivery_id.into()],
        )
        .await;
        assert_eq!(
            reclaim_expired_delivery_leases(&scratch.db)
                .await
                .expect("expired lease is reclaimed"),
            1
        );
        exec(
            &scratch.db,
            "UPDATE event_deliveries SET next_attempt_at=now() WHERE id=$1",
            vec![delivery_id.into()],
        )
        .await;

        let restarted_status = tokio::process::Command::new(std::env::current_exe().expect("test executable exists"))
            .arg("--exact")
            .arg(test_name)
            .arg("--nocapture")
            .env(CHILD_ROLE, "restarted")
            .env(CHILD_DATABASE_URL, &scratch.url)
            .status()
            .await
            .expect("restarted dispatcher child runs");
        assert!(
            restarted_status.success(),
            "fresh dispatcher process must deliver the reclaimed row"
        );

        #[derive(FromQueryResult)]
        struct Row {
            status: String,
            attempts: i32,
        }
        let row = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status,attempts FROM event_deliveries WHERE id=$1",
            vec![delivery_id.into()],
        ))
        .one(&scratch.db)
        .await
        .expect("delivery query runs")
        .expect("delivery remains visible");
        assert_eq!(row.status, "dispatched");
        assert_eq!(
            row.attempts, 1,
            "lease recovery consumes exactly one crash-attempt budget slot"
        );

        let requests = receiver.await.expect("receiver task completes");
        assert_eq!(requests.len(), 2);
        let expected_header = format!("x-sylvode-delivery-id: {delivery_id}");
        let mut consumer_seen_delivery_ids = std::collections::HashSet::new();
        for request in &requests {
            assert!(
                request.to_ascii_lowercase().contains(&expected_header),
                "every attempt carries the immutable consumer dedupe key"
            );
            let key = request
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("x-sylvode-delivery-id")
                        .then(|| value.trim().to_owned())
                })
                .expect("consumer reads delivery_id from the required header");
            consumer_seen_delivery_ids.insert(key);
        }
        assert_eq!(
            consumer_seen_delivery_ids.len(),
            1,
            "consumer delivery_id dedupe observes one delivery"
        );
        let first_body = requests[0].split("\r\n\r\n").nth(1).expect("first body exists");
        let second_body = requests[1].split("\r\n\r\n").nth(1).expect("second body exists");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(first_body).unwrap()["delivery"]["attempt"],
            1
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(second_body).unwrap()["delivery"]["attempt"],
            2
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_delivery_v08_reads_the_current_endpoint_and_rotated_secret_only_at_send_time() {
        async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0_u8; 4096];
                let read = socket.read(&mut chunk).await.expect("request is readable");
                assert!(read > 0, "request ended before its declared body");
                bytes.extend_from_slice(&chunk[..read]);
                let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().expect("content-length is numeric"))
                    })
                    .unwrap_or(0);
                if bytes.len() >= header_end + 4 + content_length {
                    return String::from_utf8(bytes).expect("request is UTF-8");
                }
            }
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("loopback listener binds");
        let address = listener.local_addr().expect("listener address is available");
        let receiver = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("delivery connection arrives");
            let request = read_request(&mut socket).await;
            socket
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .expect("response writes");
            request
        });

        let scratch = scratch_or_skip!("current-endpoint-secret");
        let workspace_id = seed_workspace(&scratch.db).await;
        let webhook_id = seed_webhook(
            &scratch.db,
            workspace_id,
            "http://old.example.invalid/hook",
            &["flow.object.created"],
        )
        .await;
        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
        expand_one(&scratch.db)
            .await
            .expect("expansion takes the subscriber snapshot");
        exec(
            &scratch.db,
            "UPDATE webhooks SET url=$2,secret='rotated-secret' WHERE id=$1",
            vec![webhook_id.into(), format!("http://{address}/new-hook").into()],
        )
        .await;
        let outcome = TEST_ALLOW_PRIVATE_DELIVERY_TARGET
            .scope(true, send_one(&state_for(scratch.db.clone()), &reqwest::Client::new()))
            .await
            .expect("send runs");
        assert_eq!(outcome, Some(true), "the old expansion-time endpoint was not retained");
        let request = receiver.await.expect("receiver completes");
        assert!(request.starts_with("POST /new-hook "));
        let (headers, body) = request.split_once("\r\n\r\n").expect("request has a body boundary");
        let signature = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case(super::WEBHOOK_SIGNATURE_HEADER)
                    .then(|| value.trim())
            })
            .expect("signature header exists");
        let expected = super::sign_payload("rotated-secret", body).expect("rotated payload signs");
        assert_eq!(signature, format!("sha256={expected}"));
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE event_id=$1 AND status='dispatched'",
                vec![event_id.into()],
            )
            .await,
            1
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

    // =============================================================================================
    // Gate: business_event_dispatch_same_transaction
    // =============================================================================================

    #[tokio::test]
    async fn business_event_dispatch_same_transaction_touches_zero_subscription_queries() {
        let scratch = scratch_or_skip!("zero-sub-query");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created"],
        )
        .await;

        // A second connection holds an ACCESS EXCLUSIVE lock on `webhooks` for the whole test body.
        // That lock blocks *every* statement against that table, not just writes -- including a
        // plain unlocked SELECT, which a lighter lock (e.g. row-level FOR UPDATE) would not. If the
        // domain transaction below queried the subscription directory even once, it would block
        // here until the lock is released, and the timeout turns that into a failed assertion
        // instead of a hang.
        let locker = scratch.second_connection().await;
        let lock_tx = locker.begin().await.expect("lock tx begins");
        lock_tx
            .execute_unprepared("LOCK TABLE webhooks IN ACCESS EXCLUSIVE MODE")
            .await
            .expect("lock acquired");

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})),
        )
        .await;
        assert!(
            result.is_ok(),
            "the domain transaction blocked on a table lock held on `webhooks`: it must issue zero \
             subscription-directory queries in the domain transaction (ADR-0011)"
        );

        lock_tx.rollback().await.expect("lock released");
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn business_event_dispatch_same_transaction_rollback_leaves_zero_residue() {
        let scratch = scratch_or_skip!("rollback-residue");
        let workspace_id = seed_workspace(&scratch.db).await;

        let tx = scratch.db.begin().await.expect("tx begins");
        let event_id = insert_business_event(
            &tx,
            BusinessEventInput {
                workspace_id,
                project_id: None,
                event_type: "flow.object.created".to_string(),
                aggregate_type: "flow_object".to_string(),
                aggregate_id: Uuid::new_v4().to_string(),
                actor_id: None,
                source: json!({ "surface": "rest" }),
                payload: json!({}),
                metadata: json!({}),
                correlation_id: None,
                causation_id: None,
                idempotency_key: None,
            },
        )
        .await
        .expect("business event insert succeeds inside the open transaction");
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO event_dispatch (id, event_id, workspace_id, event_type, max_attempts) \
             VALUES ($1, $2, $3, $4, 10)",
            vec![
                Uuid::new_v4().into(),
                event_id.into(),
                workspace_id.into(),
                "flow.object.created".into(),
            ],
        ))
        .await
        .expect("event_dispatch insert succeeds inside the open transaction");

        // Simulate the rest of the domain transaction failing after both rows above were written --
        // e.g. a later step in the same transaction hit a constraint violation -- by rolling back
        // explicitly. This has the identical server-visible effect as an error percolating up
        // through `?` before `tx.commit()` is ever reached.
        tx.rollback().await.expect("rollback succeeds");

        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM business_events WHERE id = $1",
                vec![event_id.into()],
            )
            .await,
            0,
            "a rolled-back domain transaction must leave zero business_events rows"
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_dispatch WHERE event_id = $1",
                vec![event_id.into()],
            )
            .await,
            0,
            "a rolled-back domain transaction must leave zero event_dispatch rows"
        );

        scratch.drop_self().await;
    }

    // =============================================================================================
    // Gate: dispatch_expansion_snapshot_semantics
    // =============================================================================================

    #[tokio::test]
    async fn dispatch_expansion_delivers_to_a_subscriber_created_after_the_event_committed_but_before_expansion() {
        let scratch = scratch_or_skip!("snapshot-late-subscriber");
        let workspace_id = seed_workspace(&scratch.db).await;

        // The event commits with zero subscribers registered yet.
        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;

        // A webhook is registered *after* that commit, but the dispatcher has not expanded the work
        // item yet (nothing has called `expand_one` in between).
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created"],
        )
        .await;

        let outcome = expand_one(&scratch.db)
            .await
            .expect("expansion runs")
            .expect("one work item was pending");
        assert!(
            matches!(outcome, ExpansionOutcome::Expanded),
            "events-v1.md: the receiver set is whoever is active *at expansion time*, not at commit \
             time -- a webhook registered after commit but before expansion must receive this event, \
             which is defined-in-spec behavior, not a bug"
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
                vec![event_id.into()],
            )
            .await,
            1
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn dispatch_expansion_skips_a_subscriber_deleted_after_commit_but_before_expansion() {
        let scratch = scratch_or_skip!("snapshot-deleted-subscriber");
        let workspace_id = seed_workspace(&scratch.db).await;
        let webhook_id = seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created"],
        )
        .await;
        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;

        // The subscriber is deleted before the dispatcher ever expands the work item.
        delete_webhook(&scratch.db, webhook_id).await;

        let outcome = expand_one(&scratch.db)
            .await
            .expect("expansion runs")
            .expect("one work item was pending");
        assert!(
            matches!(outcome, ExpansionOutcome::NoSubscribers),
            "a subscriber deleted before expansion must not receive the event, and with no other \
             subscriber left the work item must terminalize as no_subscribers"
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
                vec![event_id.into()],
            )
            .await,
            0
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn dispatch_expansion_skips_a_subscriber_disabled_after_commit_but_before_expansion() {
        let scratch = scratch_or_skip!("snapshot-disabled-subscriber");
        let workspace_id = seed_workspace(&scratch.db).await;
        let webhook_id = seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created"],
        )
        .await;
        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;

        // Disabled is judged the same as deleted (events-v1.md "订阅目录与 active 的判据").
        exec(
            &scratch.db,
            "UPDATE webhooks SET active = false WHERE id = $1",
            vec![webhook_id.into()],
        )
        .await;

        let outcome = expand_one(&scratch.db)
            .await
            .expect("expansion runs")
            .expect("one work item was pending");
        assert!(matches!(outcome, ExpansionOutcome::NoSubscribers));
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
                vec![event_id.into()],
            )
            .await,
            0
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn dispatch_expansion_delivers_to_exactly_the_subscribers_active_at_expansion_moment() {
        let scratch = scratch_or_skip!("snapshot-exact-set");
        let workspace_id = seed_workspace(&scratch.db).await;
        // Three subscribers: one for the right event type, one for a different event type (must
        // not receive), one disabled (must not receive).
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook-a",
            &["flow.object.created"],
        )
        .await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook-b",
            &["flow.object.archived"],
        )
        .await;
        let disabled = seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook-c",
            &["flow.object.created"],
        )
        .await;
        exec(
            &scratch.db,
            "UPDATE webhooks SET active = false WHERE id = $1",
            vec![disabled.into()],
        )
        .await;

        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
        let report = run_tick(&state_for(scratch.db.clone()), &reqwest::Client::new(), 4).await;
        assert_eq!(report.expanded, 1, "{report:?}");
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
                vec![event_id.into()],
            )
            .await,
            1,
            "exactly one of the three registered webhooks matches both the event type and active=true"
        );

        scratch.drop_self().await;
    }

    // =============================================================================================
    // Gate: no_subscribers_terminalized_and_reaped
    // =============================================================================================

    #[tokio::test]
    async fn no_subscribers_row_survives_exactly_at_the_retention_boundary_and_is_reaped_one_ms_past_it() {
        let scratch = scratch_or_skip!("no-sub-retention-boundary");
        let workspace_id = seed_workspace(&scratch.db).await;
        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.archived", None, None, json!({})).await;

        let dispatch_id: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_dispatch WHERE event_id = $1",
            vec![event_id.into()],
            "id",
        )
        .await;

        let outcome = expand_one(&scratch.db)
            .await
            .expect("expansion runs")
            .expect("one work item was pending");
        assert!(matches!(outcome, ExpansionOutcome::NoSubscribers));

        // `dispatch_no_subscribers_retention_hours` is frozen at 24h. Freeze one reference instant
        // in Rust and use it for both the row's `expanded_at` and the reaper's `now` argument, so
        // "exactly at the boundary" is not at the mercy of two independent Postgres `now()` calls
        // drifting apart between the `UPDATE` and the `DELETE`.
        let reference_now = Utc::now();
        set_timestamp(
            &scratch.db,
            "event_dispatch",
            "expanded_at",
            "id",
            dispatch_id,
            reference_now - chrono::Duration::hours(24),
        )
        .await;
        let reaped_at_boundary = super::reap_dispatch_retention(&scratch.db, reference_now)
            .await
            .expect("reaper runs");
        assert_eq!(
            reaped_at_boundary, 0,
            "a row exactly at the retention boundary must survive"
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_dispatch WHERE id = $1",
                vec![dispatch_id.into()],
            )
            .await,
            1
        );

        // One millisecond older crosses the boundary (same reference instant for the reaper call).
        set_timestamp(
            &scratch.db,
            "event_dispatch",
            "expanded_at",
            "id",
            dispatch_id,
            reference_now - chrono::Duration::hours(24) - chrono::Duration::milliseconds(1),
        )
        .await;
        let reaped_past_boundary = super::reap_dispatch_retention(&scratch.db, reference_now)
            .await
            .expect("reaper runs");
        assert_eq!(
            reaped_past_boundary, 1,
            "one millisecond past the retention boundary the row must be reaped"
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_dispatch WHERE id = $1",
                vec![dispatch_id.into()],
            )
            .await,
            0
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM business_events WHERE id = $1",
                vec![event_id.into()],
            )
            .await,
            1,
            "no-subscriber retention must never delete the permanent audit event"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn no_subscribers_work_does_not_block_a_later_work_item_in_the_same_batch() {
        let scratch = scratch_or_skip!("no-sub-non-blocking");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created"],
        )
        .await;

        // A no-subscriber event (archived, nobody subscribes) followed by a real one.
        commit_dispatch_work(&scratch.db, workspace_id, "flow.object.archived", None, None, json!({})).await;
        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;

        let report = run_tick(&state_for(scratch.db.clone()), &reqwest::Client::new(), 8).await;
        assert_eq!(report.no_subscribers, 1, "{report:?}");
        assert_eq!(report.expanded, 1, "{report:?}");
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
                vec![event_id.into()],
            )
            .await,
            1
        );

        scratch.drop_self().await;
    }

    // =============================================================================================
    // Gate: dispatcher_liveness_and_backlog
    // =============================================================================================

    #[test]
    fn dispatcher_liveness_pure_boundary_via_virtual_clock() {
        let last_tick = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("fixed timestamp parses")
            .with_timezone(&Utc);

        assert!(
            !dispatcher_is_live_since(None, last_tick, DISPATCHER_LIVENESS_MAX_SILENCE_MS),
            "before any tick has ever happened, liveness must be false regardless of `now`"
        );

        let exactly_at_budget = last_tick + chrono::Duration::milliseconds(DISPATCHER_LIVENESS_MAX_SILENCE_MS);
        assert!(
            dispatcher_is_live_since(Some(last_tick), exactly_at_budget, DISPATCHER_LIVENESS_MAX_SILENCE_MS),
            "exactly at the silence budget, the dispatcher must still be reported live"
        );

        let one_ms_past_budget = exactly_at_budget + chrono::Duration::milliseconds(1);
        assert!(
            !dispatcher_is_live_since(Some(last_tick), one_ms_past_budget, DISPATCHER_LIVENESS_MAX_SILENCE_MS),
            "one millisecond past the silence budget, the dispatcher must be reported stale"
        );
    }

    #[tokio::test]
    async fn dispatcher_reports_live_immediately_after_a_real_tick() {
        let scratch = scratch_or_skip!("liveness-integration");
        let state = state_for(scratch.db.clone());
        run_tick(&state, &reqwest::Client::new(), 1).await;
        assert!(
            dispatcher_is_live(Utc::now(), DISPATCHER_LIVENESS_MAX_SILENCE_MS),
            "the process-global wrapper must report live immediately after any run_tick call in \
             this process (this assertion only ever requires *some* recent tick, so it cannot be \
             made to fail by other tests concurrently ticking the same process)"
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn oldest_pending_dispatch_age_advances_with_a_backdated_created_at_not_a_sleep() {
        let scratch = scratch_or_skip!("oldest-pending-age");
        let workspace_id = seed_workspace(&scratch.db).await;
        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
        let dispatch_id: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_dispatch WHERE event_id = $1",
            vec![event_id.into()],
            "id",
        )
        .await;

        let fresh_age = oldest_pending_dispatch_age_ms(&scratch.db, Utc::now())
            .await
            .expect("query runs")
            .expect("one pending row exists");
        assert!(
            fresh_age < 5_000,
            "a freshly committed row must be reported as ~0ms old, got {fresh_age}ms"
        );

        // Advance the virtual clock by backdating the row's own `created_at`, not by sleeping.
        backdate(
            &scratch.db,
            "event_dispatch",
            "created_at",
            "id",
            dispatch_id,
            chrono::Duration::milliseconds(OLDEST_PENDING_AGE_ALERT_MS + 1),
        )
        .await;

        let aged = oldest_pending_dispatch_age_ms(&scratch.db, Utc::now())
            .await
            .expect("query runs")
            .expect("still one pending row");
        assert!(aged > OLDEST_PENDING_AGE_ALERT_MS, "aged={aged}ms");

        let report = super::DispatchTickReport {
            oldest_pending_dispatch_age_ms: Some(aged),
            ..Default::default()
        };
        assert!(
            backlog_alert(&report, OLDEST_PENDING_AGE_ALERT_MS),
            "a backlog item older than the alert threshold must trip backlog_alert"
        );
        assert!(
            !backlog_alert(&super::DispatchTickReport::default(), OLDEST_PENDING_AGE_ALERT_MS),
            "an empty report (no known backlog) must never alert"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn oldest_pending_delivery_age_counts_pending_sealed_and_leased_but_not_terminal_rows() {
        let scratch = scratch_or_skip!("oldest-pending-delivery-age");
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
        expand_one(&scratch.db).await.expect("expansion runs");
        let delivery_id: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_deliveries WHERE event_id = $1",
            vec![event_id.into()],
            "id",
        )
        .await;
        backdate(
            &scratch.db,
            "event_deliveries",
            "created_at",
            "id",
            delivery_id,
            chrono::Duration::milliseconds(OLDEST_PENDING_AGE_ALERT_MS + 1),
        )
        .await;

        let age = oldest_pending_delivery_age_ms(&scratch.db, Utc::now())
            .await
            .expect("query runs")
            .expect("one undelivered row exists");
        assert!(age > OLDEST_PENDING_AGE_ALERT_MS, "age={age}ms");

        // Once it reaches a terminal state, it must stop counting toward the backlog age at all.
        exec(
            &scratch.db,
            "UPDATE event_deliveries SET status = 'dispatched', terminated_at = now(), \
             lease_token = NULL, lease_expires_at = NULL WHERE id = $1",
            vec![delivery_id.into()],
        )
        .await;
        let age_after_terminal = oldest_pending_delivery_age_ms(&scratch.db, Utc::now())
            .await
            .expect("query runs");
        assert_eq!(
            age_after_terminal, None,
            "a dispatched (terminal) row must not count toward oldest_pending_delivery_age"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn killed_dispatcher_backlog_is_drained_after_restart_with_zero_duplicate_delivery() {
        let scratch = scratch_or_skip!("kill-restart-backlog");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created"],
        )
        .await;

        let mut event_ids = Vec::new();
        for _ in 0..5 {
            let event_id =
                commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
            event_ids.push(event_id);
        }

        // Simulate a worker that claimed every one of these leases and then vanished before
        // completing expansion: replicate exactly the lease-claiming UPDATE `expand_one` issues,
        // then never run the expansion transaction that would normally follow it in the same call.
        exec(
            &scratch.db,
            "UPDATE event_dispatch SET lease_token = 'dead-worker', \
             lease_expires_at = now() - interval '1 minute' WHERE workspace_id = $1 AND status = 'pending'",
            vec![workspace_id.into()],
        )
        .await;

        // "Restart": a fresh dispatcher process reclaims the abandoned leases, then works the
        // backlog exactly as `run_tick`'s normal loop does.
        let reclaimed = reclaim_expired_dispatch_leases(&scratch.db)
            .await
            .expect("reclaim runs");
        assert_eq!(reclaimed, 5, "every abandoned lease must be reclaimed on restart");

        let report = run_tick(&state_for(scratch.db.clone()), &reqwest::Client::new(), 16).await;
        assert_eq!(report.expanded, 5, "{report:?}");

        for event_id in &event_ids {
            assert_eq!(
                count(
                    &scratch.db,
                    "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
                    vec![(*event_id).into()],
                )
                .await,
                1,
                "event {event_id} must have exactly one delivery row after drain, zero duplicates"
            );
            assert_eq!(
                count(
                    &scratch.db,
                    "SELECT count(*) AS n FROM event_delivery_sources WHERE source_event_id = $1",
                    vec![(*event_id).into()],
                )
                .await,
                1,
                "event {event_id} must have exactly one source-table row after drain"
            );
        }
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_dispatch WHERE workspace_id = $1 AND status = 'pending'",
                vec![workspace_id.into()],
            )
            .await,
            0,
            "no work item may remain pending after the backlog is drained"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_true_mid_expansion_crash_via_injected_failure_recovers_cleanly_on_the_next_tick() {
        let scratch = scratch_or_skip!("kill-mid-expansion-injected");
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

        // "Kill" the worker exactly mid-expansion: step (a) (the source reservation) has already
        // run on the open transaction, then step (b) fails and the whole transaction rolls back --
        // the same server-visible effect as the process actually dying at that instant.
        let crashed = FAIL_EXPANSION_STEP_B
            .scope(std::cell::Cell::new(true), expand_one(&scratch.db))
            .await;
        assert!(
            crashed.is_err(),
            "the injected failure must propagate as an Err from expand_one"
        );

        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
                vec![event_id.into()],
            )
            .await,
            0,
            "the rolled-back transaction must leave zero delivery rows"
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_delivery_sources WHERE source_event_id = $1",
                vec![event_id.into()],
            )
            .await,
            0,
            "the rolled-back transaction must leave zero source-table rows -- step (a)'s reservation \
             rolled back along with everything else in the same transaction"
        );

        let status: String = get_col(
            &scratch.db,
            "SELECT status FROM event_dispatch WHERE event_id = $1",
            vec![event_id.into()],
            "status",
        )
        .await;
        assert_eq!(
            status, "pending",
            "a failed expansion must leave the work item back in pending"
        );

        // The failed attempt backed off `next_attempt_at` (`dispatch_backoff_ms`); advance the
        // virtual clock past it by backdating that column directly, not by sleeping through the
        // real backoff window.
        let dispatch_id: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_dispatch WHERE event_id = $1",
            vec![event_id.into()],
            "id",
        )
        .await;
        backdate(
            &scratch.db,
            "event_dispatch",
            "next_attempt_at",
            "id",
            dispatch_id,
            chrono::Duration::milliseconds(1),
        )
        .await;

        // "Restart": the next tick, with the fault no longer armed, must expand cleanly with no
        // leftover from the crashed attempt.
        let report = run_tick(&state_for(scratch.db.clone()), &reqwest::Client::new(), 4).await;
        assert_eq!(report.expanded, 1, "{report:?}");
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
                vec![event_id.into()],
            )
            .await,
            1,
            "exactly one delivery row after the crash-then-retry, no duplicate from the aborted attempt"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn concurrent_dispatcher_instances_racing_the_same_backlog_never_double_expand_or_double_send() {
        let scratch = scratch_or_skip!("concurrent-race");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://127.0.0.1:1/hook", // refused by the SSRF guard, so `send_one` reliably retries
            &["flow.object.created"],
        )
        .await;

        let mut event_ids = Vec::new();
        for _ in 0..10 {
            let event_id =
                commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
            event_ids.push(event_id);
        }

        // Five simulated dispatcher instances race `expand_one` against the same ten-item backlog
        // concurrently, sharing one `DatabaseConnection` (itself pool-backed, so this genuinely
        // exercises concurrent connections, not just concurrent async tasks on one connection).
        let handles: Vec<_> = (0..5)
            .map(|_| {
                let db = scratch.db.clone();
                tokio::spawn(async move { drain_expand_one(&db).await })
            })
            .collect();

        let mut total_expanded = 0u32;
        for handle in handles {
            total_expanded += handle.await.expect("worker task does not panic");
        }
        assert_eq!(
            total_expanded, 10,
            "every work item must be expanded exactly once across all racing workers"
        );

        for event_id in &event_ids {
            assert_eq!(
                count(
                    &scratch.db,
                    "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
                    vec![(*event_id).into()],
                )
                .await,
                1,
                "event {event_id} must have exactly one delivery row despite five racing expanders"
            );
        }

        // Now race `send_one` the same way; the coalescing/FIFO predicates that matter for content
        // events are exercised separately elsewhere, this proves the plain fan-out lease query
        // itself never double-leases a delivery row.
        let send_handles: Vec<_> = (0..5)
            .map(|_| {
                let db = scratch.db.clone();
                tokio::spawn(async move { drain_send_one(state_for(db)).await })
            })
            .collect();
        let mut total_sent = 0u32;
        for handle in send_handles {
            total_sent += handle.await.expect("send task does not panic");
        }
        assert_eq!(
            total_sent, 10,
            "every delivery must be leased and attempted exactly once per pass"
        );

        for event_id in &event_ids {
            let attempts: i32 = get_col(
                &scratch.db,
                "SELECT attempts FROM event_deliveries WHERE event_id = $1",
                vec![(*event_id).into()],
                "attempts",
            )
            .await;
            assert_eq!(
                attempts, 1,
                "each delivery must have been attempted exactly once, not raced twice"
            );
        }

        scratch.drop_self().await;
    }

    // =============================================================================================
    // Gate: flow_content_delivery_coalescing
    // =============================================================================================

    #[tokio::test]
    async fn coalescing_only_merges_into_a_pending_row_never_into_a_sealed_or_leased_one() {
        let scratch = scratch_or_skip!("coalesce-pending-only");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.content.accepted"],
        )
        .await;
        let document_id = Uuid::new_v4();

        commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(1),
            json!({}),
        )
        .await;
        let outcome = expand_one(&scratch.db)
            .await
            .expect("expansion runs")
            .expect("one work item was pending");
        assert!(matches!(outcome, ExpansionOutcome::Expanded));

        let first_delivery_id: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_deliveries WHERE subscriber_kind = 'webhook' AND document_id = $1",
            vec![document_id.into()],
            "id",
        )
        .await;

        // Seal it directly -- exactly the state a real row is in the instant `send_one` leases it
        // (pending/sealed -> leased), or once it has hit the coalescing cap.
        exec(
            &scratch.db,
            "UPDATE event_deliveries SET status = 'sealed' WHERE id = $1",
            vec![first_delivery_id.into()],
        )
        .await;

        commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(2),
            json!({}),
        )
        .await;
        let outcome2 = expand_one(&scratch.db)
            .await
            .expect("expansion runs")
            .expect("the seq-2 work item is now head");
        assert!(matches!(outcome2, ExpansionOutcome::Expanded));

        #[derive(sea_orm::FromQueryResult)]
        struct Row {
            id: Uuid,
            status: String,
            first_seq: Option<i64>,
        }
        let rows = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, status, first_seq FROM event_deliveries WHERE subscriber_kind = 'webhook' \
             AND document_id = $1 ORDER BY first_seq",
            vec![document_id.into()],
        ))
        .all(&scratch.db)
        .await
        .expect("query runs");

        assert_eq!(
            rows.len(),
            2,
            "a sealed row must not absorb the next event -- a new row opens"
        );
        assert_eq!(rows[0].id, first_delivery_id);
        assert_eq!(rows[0].status, "sealed");
        assert_eq!(rows[0].first_seq, Some(1));
        assert_eq!(rows[1].status, "pending");
        assert_eq!(rows[1].first_seq, Some(2));

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn coalescing_freezes_the_range_and_source_set_the_instant_the_row_is_leased() {
        let scratch = scratch_or_skip!("coalesce-freeze-on-lease");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://127.0.0.1:1/hook",
            &["flow.content.accepted"],
        )
        .await;
        let document_id = Uuid::new_v4();

        commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(1),
            json!({}),
        )
        .await;
        commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(2),
            json!({}),
        )
        .await;
        let state = state_for(scratch.db.clone());
        let client = reqwest::Client::new();
        let report = run_tick(&state, &client, 8).await;
        assert_eq!(report.expanded, 2, "{report:?}");

        let delivery_id: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_deliveries WHERE subscriber_kind = 'webhook' AND document_id = $1",
            vec![document_id.into()],
            "id",
        )
        .await;

        // Force the debounce to have elapsed and lease it, exactly as `send_one` would.
        exec(
            &scratch.db,
            "UPDATE event_deliveries SET next_attempt_at = now() WHERE id = $1",
            vec![delivery_id.into()],
        )
        .await;
        send_one(&state, &client).await.expect("send_one runs");

        #[derive(sea_orm::FromQueryResult)]
        struct RangeRow {
            status: String,
            first_seq: Option<i64>,
            latest_seq: Option<i64>,
        }
        let leased_before = RangeRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status, first_seq, latest_seq FROM event_deliveries WHERE id = $1",
            vec![delivery_id.into()],
        ))
        .one(&scratch.db)
        .await
        .expect("query runs")
        .expect("row exists");
        // A failed send against a refused endpoint retries a content delivery straight to
        // `sealed` (never back to `pending`) -- either way it must not still be `leased` once
        // `send_one` returns.
        assert_ne!(leased_before.status, "pending", "{:?}", leased_before.status);
        let source_count_before = count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_delivery_sources WHERE delivery_id = $1",
            vec![delivery_id.into()],
        )
        .await;
        assert_eq!(source_count_before, 2);

        // A third accepted update on the same document must open a *new* pending row -- it must
        // never be able to touch the row that was in flight, whose range and source set are frozen.
        commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(3),
            json!({}),
        )
        .await;
        let outcome3 = expand_one(&scratch.db)
            .await
            .expect("expansion runs")
            .expect("the seq-3 work item is head once seq 1/2's work items are already resolved");
        assert!(matches!(outcome3, ExpansionOutcome::Expanded));

        let rows = count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE subscriber_kind = 'webhook' AND document_id = $1",
            vec![document_id.into()],
        )
        .await;
        assert_eq!(rows, 2, "the frozen row plus one fresh pending row for seq 3");

        let after = RangeRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status, first_seq, latest_seq FROM event_deliveries WHERE id = $1",
            vec![delivery_id.into()],
        ))
        .one(&scratch.db)
        .await
        .expect("query runs")
        .expect("row still exists");
        assert_eq!(
            (after.first_seq, after.latest_seq),
            (leased_before.first_seq, leased_before.latest_seq),
            "the once-leased row's range must be unchanged by the later seq-3 event"
        );
        let source_count_after = count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_delivery_sources WHERE delivery_id = $1",
            vec![delivery_id.into()],
        )
        .await;
        assert_eq!(
            source_count_after, source_count_before,
            "the once-leased row's source-table entries must be unchanged by the later seq-3 event"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn build_delivery_body_reuses_the_same_delivery_id_across_retries_only_the_attempt_number_changes() {
        let scratch = scratch_or_skip!("dedupe-delivery-id");
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
        expand_one(&scratch.db).await.expect("expansion runs");

        let delivery_id: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_deliveries WHERE event_id = $1",
            vec![event_id.into()],
            "id",
        )
        .await;

        let row_attempt_0 = delivery_lease_row_for_test(delivery_id, event_id, None);
        let mut row_attempt_2 = delivery_lease_row_for_test(delivery_id, event_id, None);
        row_attempt_2.attempts = 2;

        let body_first = build_delivery_body(&scratch.db, &row_attempt_0)
            .await
            .expect("body builds");
        let body_retry = build_delivery_body(&scratch.db, &row_attempt_2)
            .await
            .expect("body builds");

        assert_eq!(
            body_first["delivery"]["id"], body_retry["delivery"]["id"],
            "delivery_id must be immutable across retries"
        );
        assert_eq!(body_first["delivery"]["id"], json!(delivery_id));
        assert_eq!(body_first["delivery"]["attempt"], 1);
        assert_eq!(body_retry["delivery"]["attempt"], 3, "attempt is attempts+1");
        assert_eq!(
            body_first["event"]["event_id"], body_retry["event"]["event_id"],
            "retries do not mint a new event_id"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn coalescing_cap_freezes_the_row_and_opens_a_new_one_with_exact_source_counts() {
        let scratch = scratch_or_skip!("coalesce-cap");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.content.accepted"],
        )
        .await;
        let document_id = Uuid::new_v4();

        // COALESCED_SOURCE_EVENTS_MAX is 20 (private to this module, read through `super::` below
        // rather than restated): committing and expanding CAP+1 sequential accepted updates on the
        // same document must fill exactly one row to the cap and open a second one for the overflow
        // event, never drop or summarize a source entry.
        const CAP: i64 = super::COALESCED_SOURCE_EVENTS_MAX;
        for seq in 1..=(CAP + 1) {
            commit_dispatch_work(
                &scratch.db,
                workspace_id,
                "flow.content.accepted",
                Some(document_id),
                Some(seq),
                json!({}),
            )
            .await;
            let outcome = expand_one(&scratch.db)
                .await
                .unwrap_or_else(|err| panic!("expansion of seq {seq} runs: {err}"))
                .unwrap_or_else(|| panic!("seq {seq}'s work item is head"));
            assert!(matches!(outcome, ExpansionOutcome::Expanded), "seq {seq}");
        }

        #[derive(sea_orm::FromQueryResult)]
        struct Row {
            id: Uuid,
            status: String,
            first_seq: Option<i64>,
            latest_seq: Option<i64>,
        }
        let rows = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, status, first_seq, latest_seq FROM event_deliveries WHERE subscriber_kind = 'webhook' \
             AND document_id = $1 ORDER BY first_seq",
            vec![document_id.into()],
        ))
        .all(&scratch.db)
        .await
        .expect("query runs");

        assert_eq!(
            rows.len(),
            2,
            "the cap must open a second row instead of dropping or summarizing the overflow event"
        );
        assert_eq!(
            rows[0].status, "sealed",
            "the first row must be sealed once it reaches the cap"
        );
        assert_eq!(rows[0].first_seq, Some(1));
        assert_eq!(rows[0].latest_seq, Some(CAP));
        assert_eq!(rows[1].status, "pending");
        assert_eq!(rows[1].first_seq, Some(CAP + 1));

        let sources_first = count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_delivery_sources WHERE delivery_id = $1",
            vec![rows[0].id.into()],
        )
        .await;
        assert_eq!(
            sources_first, CAP,
            "the sealed row's source count must equal the cap exactly, no summary"
        );
        let sources_second = count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_delivery_sources WHERE delivery_id = $1",
            vec![rows[1].id.into()],
        )
        .await;
        assert_eq!(sources_second, 1);

        scratch.drop_self().await;
    }

    // =============================================================================================
    // Gate: coalescing_seal_and_source_first_expansion
    // =============================================================================================

    #[tokio::test]
    async fn dispatch_fifo_barrier_blocks_a_newer_same_document_work_item_while_an_older_one_is_in_flight() {
        let scratch = scratch_or_skip!("fifo-barrier-blocks");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.content.accepted"],
        )
        .await;
        let document_id = Uuid::new_v4();
        commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(1),
            json!({}),
        )
        .await;
        commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(2),
            json!({}),
        )
        .await;
        let seq1_id: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_dispatch WHERE workspace_id = $1 AND accepted_seq = 1",
            vec![workspace_id.into()],
            "id",
        )
        .await;

        // A genuine second, still-open transaction on an independent connection takes the exact
        // `FOR UPDATE` row lock `expand_one`'s own candidate-selection subquery's `FOR UPDATE SKIP
        // LOCKED` would contend on -- not a fabricated `lease_token`/`lease_expires_at` stamp on
        // the same connection. `gate-commands.md`'s literal requirement: "把旧 sealed 行锁在另一个
        // 事务里再启动第二个 worker" -- a materially stronger proof than an UPDATE, because it
        // exercises the real Postgres row-lock contention `SKIP LOCKED` must respect, not just the
        // lease columns `expand_one`'s WHERE clause also happens to check.
        let locker = scratch.second_connection().await;
        let locker_tx = locker.begin().await.expect("locker transaction begins");
        locker_tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM event_dispatch WHERE id = $1 FOR UPDATE",
                vec![seq1_id.into()],
            ))
            .await
            .expect("lock query runs")
            .expect("seq 1's row exists to be locked");

        // Seq 2 must not be pickable while seq 1 -- still `status = 'pending'`, merely row-locked
        // by the still-open second transaction above -- is an older eligible predecessor on the
        // same document.
        let outcome = expand_one(&scratch.db).await.expect("query runs");
        assert!(
            outcome.is_none(),
            "seq 2 must not be selectable while seq 1 (its older same-document predecessor) is \
             still pending and genuinely row-locked by a real, held second transaction -- SKIP \
             LOCKED must not let a worker skip past a busy head to a newer sibling"
        );

        // Release the real lock and confirm the head is now genuinely leasable -- proves the
        // negative result above came from the held lock, not from some other reason expand_one
        // returned nothing.
        locker_tx
            .rollback()
            .await
            .expect("locker transaction releases its lock");
        let outcome = expand_one(&scratch.db)
            .await
            .expect("query runs")
            .expect("seq 1 becomes leasable the instant the real row lock is released");
        assert!(matches!(
            outcome,
            ExpansionOutcome::NoSubscribers | ExpansionOutcome::Expanded
        ));

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn dispatch_fifo_barrier_orders_by_accepted_seq_not_created_at_when_both_rows_share_a_timestamp() {
        // `gate-commands.md`'s literal requirement: "并补一组 created_at 相同、first_seq 不同的固定
        // fixture" -- an implementation that orders the head-of-queue candidate by `created_at`
        // (or insertion order) instead of `accepted_seq` would pass every other FIFO fixture in
        // this file (they all insert in seq order) but silently misorder this one.
        let scratch = scratch_or_skip!("fifo-barrier-same-created-at");
        let workspace_id = seed_workspace(&scratch.db).await;
        let document_id = Uuid::new_v4();

        let event_2 = commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(2),
            json!({}),
        )
        .await;
        let dispatch_2_id: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_dispatch WHERE event_id = $1",
            vec![event_2.into()],
            "id",
        )
        .await;
        let same_created_at: DateTime<Utc> = get_col(
            &scratch.db,
            "SELECT created_at FROM event_dispatch WHERE event_id = $1",
            vec![event_2.into()],
            "created_at",
        )
        .await;

        let event_1 = commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(1),
            json!({}),
        )
        .await;
        let dispatch_1_id: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_dispatch WHERE event_id = $1",
            vec![event_1.into()],
            "id",
        )
        .await;
        // Pin the seq-1 row's `created_at` to exactly the seq-2 row's own -- two independent
        // `commit_dispatch_work` calls would each stamp their own `now()` and could never
        // reproduce an exact tie deterministically otherwise.
        exec(
            &scratch.db,
            "UPDATE event_dispatch SET created_at = $2 WHERE id = $1",
            vec![dispatch_1_id.into(), same_created_at.into()],
        )
        .await;

        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.content.accepted"],
        )
        .await;

        // Real `expand_one` call (not a hand-duplicated copy of its candidate-selection SQL): the
        // seq-2 row must still be `pending` afterwards, and the seq-1 row must be the one that
        // actually got expanded, even though both share the exact same `created_at`.
        let outcome = expand_one(&scratch.db)
            .await
            .expect("expansion query runs")
            .expect("one of the two same-created_at rows is head");
        assert!(matches!(outcome, ExpansionOutcome::Expanded));

        let dispatch_1_status: String = get_col(
            &scratch.db,
            "SELECT status FROM event_dispatch WHERE id = $1",
            vec![dispatch_1_id.into()],
            "status",
        )
        .await;
        let dispatch_2_status: String = get_col(
            &scratch.db,
            "SELECT status FROM event_dispatch WHERE id = $1",
            vec![dispatch_2_id.into()],
            "status",
        )
        .await;
        assert_eq!(
            dispatch_1_status, "expanded",
            "accepted_seq=1 must be the row expand_one picked, even though created_at ties with seq=2"
        );
        assert_eq!(
            dispatch_2_status, "pending",
            "accepted_seq=2 must still be blocked behind its older same-document sibling"
        );

        scratch.drop_self().await;
    }

    /// `events-v1.md` "reaper 的谓词" / "来源表的保留期与外键语义": `event_delivery_sources` has no
    /// `status`, so its reaper cannot use the `WHERE status IN (...)` shape the other two reapers
    /// share -- its safety property is entirely the `delivery_id IS NULL` predicate. This proves
    /// the three-way partition literally: a tombstone (`delivery_id IS NULL`) past retention is
    /// reaped; a tombstone not yet past retention survives; and a row whose `delivery_id` is still
    /// non-NULL survives *regardless of age*, because deleting it would destroy the
    /// `(subscriber_kind, subscriber_id, source_event_id)` dedup evidence a still-referenced
    /// delivery depends on.
    #[tokio::test]
    async fn tombstone_reaper_deletes_only_delivery_id_null_rows_past_retention_and_never_a_still_referenced_row() {
        let scratch = scratch_or_skip!("tombstone-reaper");
        let workspace_id = seed_workspace(&scratch.db).await;

        async fn source_event(db: &DatabaseConnection, workspace_id: Uuid) -> Uuid {
            insert_business_event(
                db,
                BusinessEventInput {
                    workspace_id,
                    project_id: None,
                    event_type: "flow.content.accepted".to_string(),
                    aggregate_type: "flow_document".to_string(),
                    aggregate_id: Uuid::new_v4().to_string(),
                    actor_id: None,
                    source: json!({ "surface": "system" }),
                    payload: json!({}),
                    metadata: json!({}),
                    correlation_id: None,
                    causation_id: None,
                    idempotency_key: None,
                },
            )
            .await
            .expect("source business event inserts")
        }

        let live_delivery_id = Uuid::new_v4();
        exec(
            &scratch.db,
            "INSERT INTO event_deliveries (id, event_id, workspace_id, subscriber_kind, subscriber_id) \
             VALUES ($1, $2, $3, 'webhook', $4)",
            vec![
                live_delivery_id.into(),
                source_event(&scratch.db, workspace_id).await.into(),
                workspace_id.into(),
                Uuid::new_v4().into(),
            ],
        )
        .await;

        let now = Utc::now();
        let old_created_at = now - chrono::Duration::days(super::DELIVERY_SOURCE_RETENTION_DAYS + 1);
        let fresh_created_at = now - chrono::Duration::days(super::DELIVERY_SOURCE_RETENTION_DAYS - 1);

        async fn insert_source_row(
            db: &DatabaseConnection,
            workspace_id: Uuid,
            delivery_id: Option<Uuid>,
            source_event_id: Uuid,
            created_at: DateTime<Utc>,
        ) -> Uuid {
            let id = Uuid::new_v4();
            exec(
                db,
                "INSERT INTO event_delivery_sources \
                 (id, workspace_id, delivery_id, subscriber_kind, subscriber_id, source_event_id, created_at) \
                 VALUES ($1, $2, $3, 'webhook', $4, $5, $6)",
                vec![
                    id.into(),
                    workspace_id.into(),
                    delivery_id.into(),
                    Uuid::new_v4().into(),
                    source_event_id.into(),
                    created_at.into(),
                ],
            )
            .await;
            id
        }

        let old_tombstone = insert_source_row(
            &scratch.db,
            workspace_id,
            None,
            source_event(&scratch.db, workspace_id).await,
            old_created_at,
        )
        .await;
        let fresh_tombstone = insert_source_row(
            &scratch.db,
            workspace_id,
            None,
            source_event(&scratch.db, workspace_id).await,
            fresh_created_at,
        )
        .await;
        let old_but_still_referenced = insert_source_row(
            &scratch.db,
            workspace_id,
            Some(live_delivery_id),
            source_event(&scratch.db, workspace_id).await,
            old_created_at,
        )
        .await;

        let reaped = reap_delivery_source_tombstones(&scratch.db, now)
            .await
            .expect("tombstone reaper runs");
        assert_eq!(reaped, 1, "exactly one row -- the old tombstone -- must be reaped");

        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_delivery_sources WHERE id = $1",
                vec![old_tombstone.into()],
            )
            .await,
            0,
            "a delivery_id=NULL row past retention must be deleted"
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_delivery_sources WHERE id = $1",
                vec![fresh_tombstone.into()],
            )
            .await,
            1,
            "a delivery_id=NULL row not yet past retention must survive"
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_delivery_sources WHERE id = $1",
                vec![old_but_still_referenced.into()],
            )
            .await,
            1,
            "a row whose delivery_id is still non-NULL must survive regardless of age -- deleting \
             it would destroy dedup evidence a still-live delivery depends on"
        );

        scratch.drop_self().await;
    }

    /// `events-v1.md`/`gate-commands.md` `requeue_failed`: revives `event_deliveries` rows stuck
    /// at `status='failed'` back into the live queue, clearing `document_id` on every revived row
    /// (content or not) so a revived content delivery cannot collide with -- or jump the FIFO
    /// queue ahead of -- a live `pending` row for the same document via the partial coalescing
    /// unique index, and never revives `cancelled` rows (`subscriber_gone`'s deliberate terminal
    /// state, not a retryable failure).
    #[tokio::test]
    async fn requeue_failed_revives_failed_content_rows_clears_document_id_and_never_revives_cancelled() {
        let scratch = scratch_or_skip!("requeue-failed");
        let workspace_id = seed_workspace(&scratch.db).await;
        let other_workspace_id = seed_workspace(&scratch.db).await;
        let document_id = Uuid::new_v4();

        async fn source_event(db: &DatabaseConnection, workspace_id: Uuid) -> Uuid {
            insert_business_event(
                db,
                BusinessEventInput {
                    workspace_id,
                    project_id: None,
                    event_type: "flow.content.accepted".to_string(),
                    aggregate_type: "flow_document".to_string(),
                    aggregate_id: Uuid::new_v4().to_string(),
                    actor_id: None,
                    source: json!({ "surface": "system" }),
                    payload: json!({}),
                    metadata: json!({}),
                    correlation_id: None,
                    causation_id: None,
                    idempotency_key: None,
                },
            )
            .await
            .expect("source business event inserts")
        }

        #[allow(clippy::too_many_arguments)]
        async fn insert_delivery(
            db: &DatabaseConnection,
            workspace_id: Uuid,
            event_id: Uuid,
            document_id: Option<Uuid>,
            status: &str,
        ) -> Uuid {
            let id = Uuid::new_v4();
            exec(
                db,
                "INSERT INTO event_deliveries \
                 (id, event_id, workspace_id, subscriber_kind, subscriber_id, document_id, status, \
                  attempts, terminated_at, last_error_code) \
                 VALUES ($1, $2, $3, 'webhook', $4, $5, $6, 3, now(), 'webhook_5xx')",
                vec![
                    id.into(),
                    event_id.into(),
                    workspace_id.into(),
                    Uuid::new_v4().into(),
                    document_id.into(),
                    status.into(),
                ],
            )
            .await;
            id
        }

        let failed_content = insert_delivery(
            &scratch.db,
            workspace_id,
            source_event(&scratch.db, workspace_id).await,
            Some(document_id),
            "failed",
        )
        .await;
        let failed_non_content = insert_delivery(
            &scratch.db,
            workspace_id,
            source_event(&scratch.db, workspace_id).await,
            None,
            "failed",
        )
        .await;
        let cancelled = insert_delivery(
            &scratch.db,
            workspace_id,
            source_event(&scratch.db, workspace_id).await,
            Some(document_id),
            "cancelled",
        )
        .await;
        // A `failed` row in a *different* workspace must not be touched by this call.
        let other_workspace_failed = insert_delivery(
            &scratch.db,
            other_workspace_id,
            source_event(&scratch.db, other_workspace_id).await,
            None,
            "failed",
        )
        .await;

        let revived = requeue_failed(&scratch.db, workspace_id, Utc::now())
            .await
            .expect("requeue_failed runs");
        assert_eq!(
            revived, 2,
            "exactly the two failed rows in this workspace must be revived"
        );

        #[derive(sea_orm::FromQueryResult)]
        struct Row {
            status: String,
            document_id: Option<Uuid>,
            attempts: i32,
            terminated_at: Option<DateTime<Utc>>,
        }
        async fn fetch(db: &DatabaseConnection, id: Uuid) -> Row {
            Row::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT status, document_id, attempts, terminated_at FROM event_deliveries WHERE id = $1",
                vec![id.into()],
            ))
            .one(db)
            .await
            .expect("query runs")
            .expect("row exists")
        }

        let revived_content = fetch(&scratch.db, failed_content).await;
        assert_eq!(revived_content.status, "pending");
        assert_eq!(
            revived_content.document_id, None,
            "document_id must be cleared on revival so this row cannot collide with -- or jump \
             the FIFO queue ahead of -- a live pending row for the same document"
        );
        assert_eq!(revived_content.attempts, 0);
        assert_eq!(revived_content.terminated_at, None);

        let revived_non_content = fetch(&scratch.db, failed_non_content).await;
        assert_eq!(revived_non_content.status, "pending");
        assert_eq!(revived_non_content.document_id, None);

        let cancelled_row = fetch(&scratch.db, cancelled).await;
        assert_eq!(
            cancelled_row.status, "cancelled",
            "cancelled is subscriber_gone's deliberate terminal state and must never be revived"
        );

        let other_row = fetch(&scratch.db, other_workspace_failed).await;
        assert_eq!(
            other_row.status, "failed",
            "requeue_failed must be scoped to the requested workspace only"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn dispatch_head_definition_three_branches_release_the_queue_head() {
        let scratch = scratch_or_skip!("head-three-branches");
        let workspace_id = seed_workspace(&scratch.db).await;
        let document_id = Uuid::new_v4();

        // Branch 1: `no_subscribers` (a resolved-but-not-`expanded` terminal state) must release
        // the head for its successor, even though it "was never expanded" in the literal sense.
        commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(1),
            json!({}),
        )
        .await;
        // No webhook registered yet: seq 1 terminalizes as no_subscribers.
        let outcome1 = expand_one(&scratch.db)
            .await
            .expect("expansion runs")
            .expect("seq 1 is head");
        assert!(matches!(outcome1, ExpansionOutcome::NoSubscribers));

        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.content.accepted"],
        )
        .await;
        commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(2),
            json!({}),
        )
        .await;
        let outcome2 = expand_one(&scratch.db)
            .await
            .expect("expansion runs")
            .expect("seq 2 must be head now that seq 1 has left status=pending");
        assert!(
            matches!(outcome2, ExpansionOutcome::Expanded),
            "a no_subscribers predecessor must not permanently block its successor"
        );

        // Branch 2: `failed` (dispatch-side dead-letter, via exhausted lease reclaims) must also
        // release the head, leaving a seq gap for the consumer's own connectivity checking.
        commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(3),
            json!({}),
        )
        .await;
        let dispatch3_id: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_dispatch WHERE document_id = $1 AND accepted_seq = 3",
            vec![document_id.into()],
            "id",
        )
        .await;
        exec(
            &scratch.db,
            "UPDATE event_dispatch SET status = 'failed', expanded_at = now(), \
             last_error_code = 'expansion_failed' WHERE id = $1",
            vec![dispatch3_id.into()],
        )
        .await;

        commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(4),
            json!({}),
        )
        .await;
        let outcome4 = expand_one(&scratch.db)
            .await
            .expect("expansion runs")
            .expect("seq 4 must be head: failed predecessors release the head too");
        assert!(matches!(outcome4, ExpansionOutcome::Expanded));

        // Branch 3: a `pending` row that is merely *leased* (not yet resolved) does NOT release the
        // head -- already covered by `dispatch_fifo_barrier_blocks_...` above; asserted here as a
        // negative control against the same document to complete the three-way partition.
        commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(5),
            json!({}),
        )
        .await;
        commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(6),
            json!({}),
        )
        .await;
        exec(
            &scratch.db,
            "UPDATE event_dispatch SET lease_token = 'in-flight', lease_expires_at = now() + interval '1 minute' \
             WHERE document_id = $1 AND accepted_seq = 5",
            vec![document_id.into()],
        )
        .await;
        let outcome6 = expand_one(&scratch.db).await.expect("query runs");
        assert!(
            outcome6.is_none(),
            "an in-flight (still pending) seq 5 must keep blocking seq 6"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn non_content_work_is_not_starved_while_a_content_documents_queue_head_is_blocked() {
        let scratch = scratch_or_skip!("anti-starvation");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.content.accepted", "flow.object.created"],
        )
        .await;
        let document_id = Uuid::new_v4();

        // A content document whose head is permanently blocked (an in-flight lease on seq 1) sits
        // in the backlog first...
        commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(1),
            json!({}),
        )
        .await;
        exec(
            &scratch.db,
            "UPDATE event_dispatch SET lease_token = 'in-flight', lease_expires_at = now() + interval '5 minutes' \
             WHERE document_id = $1 AND accepted_seq = 1",
            vec![document_id.into()],
        )
        .await;
        commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(2),
            json!({}),
        )
        .await;

        // ...followed by an unrelated non-content event.
        let non_content_event =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;

        // The blocked document's seq 2 must never be selectable, but the non-content work item must
        // still be picked up promptly -- the head-of-queue predicate only ever applies to content
        // events (`events-v1.md`'s third "坑").
        let outcome = expand_one(&scratch.db)
            .await
            .expect("expansion runs")
            .expect("the unrelated non-content work item must be selectable");
        assert!(matches!(outcome, ExpansionOutcome::Expanded));
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
                vec![non_content_event.into()],
            )
            .await,
            1,
            "the non-content event must not be starved by the blocked content document"
        );

        let still_none = count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE document_id = $1",
            vec![document_id.into()],
        )
        .await;
        assert_eq!(still_none, 0, "seq 2 on the blocked document must remain unexpanded");

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn delivery_retention_exact_boundary_and_never_deletes_a_non_terminal_row_regardless_of_age() {
        let scratch = scratch_or_skip!("delivery-retention-boundary");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created"],
        )
        .await;

        // A dispatched (terminal) delivery, backdated on `terminated_at` -- the frozen anchor for
        // this retention window (never `created_at`/`updated_at`, both of which would let the
        // boundary be gamed, per events-v1.md "计时锚点").
        let dispatched_event =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
        expand_one(&scratch.db).await.expect("expansion runs");
        let dispatched_delivery: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_deliveries WHERE event_id = $1",
            vec![dispatched_event.into()],
            "id",
        )
        .await;
        exec(
            &scratch.db,
            "UPDATE event_deliveries SET status = 'dispatched', terminated_at = now(), \
             lease_token = NULL, lease_expires_at = NULL WHERE id = $1",
            vec![dispatched_delivery.into()],
        )
        .await;

        // A `pending` delivery, artificially aged far past the retention window -- the reaper must
        // never touch a non-terminal row on age alone, regardless of which column looks old.
        let stale_pending_event =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
        expand_one(&scratch.db).await.expect("expansion runs");
        let stale_pending_delivery: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_deliveries WHERE event_id = $1",
            vec![stale_pending_event.into()],
            "id",
        )
        .await;
        backdate(
            &scratch.db,
            "event_deliveries",
            "created_at",
            "id",
            stale_pending_delivery,
            chrono::Duration::days(3650),
        )
        .await;

        let reference_now = Utc::now();

        // Exactly at the 30-day boundary: must survive.
        set_timestamp(
            &scratch.db,
            "event_deliveries",
            "terminated_at",
            "id",
            dispatched_delivery,
            reference_now - chrono::Duration::days(30),
        )
        .await;
        let reaped_at_boundary = super::reap_delivery_retention(&scratch.db, reference_now)
            .await
            .expect("reaper runs");
        assert_eq!(reaped_at_boundary, 0);
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE id = $1",
                vec![dispatched_delivery.into()],
            )
            .await,
            1
        );

        // One millisecond past it: must be reaped.
        set_timestamp(
            &scratch.db,
            "event_deliveries",
            "terminated_at",
            "id",
            dispatched_delivery,
            reference_now - chrono::Duration::days(30) - chrono::Duration::milliseconds(1),
        )
        .await;
        let reaped_past_boundary = super::reap_delivery_retention(&scratch.db, reference_now)
            .await
            .expect("reaper runs");
        assert_eq!(reaped_past_boundary, 1);
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE id = $1",
                vec![dispatched_delivery.into()],
            )
            .await,
            0
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM business_events WHERE id = $1",
                vec![dispatched_event.into()],
            )
            .await,
            1,
            "delivery retention must never delete the permanent audit event"
        );

        // The stale pending row must have survived both reaper passes above untouched.
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE id = $1",
                vec![stale_pending_delivery.into()],
            )
            .await,
            1,
            "a non-terminal row must never be deleted by the retention reaper regardless of age"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn dispatch_expanded_and_failed_retention_use_expanded_at_and_differ_from_each_other() {
        let scratch = scratch_or_skip!("dispatch-expanded-failed-retention");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created"],
        )
        .await;

        let expanded_event =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
        expand_one(&scratch.db).await.expect("expansion runs");
        let expanded_dispatch: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_dispatch WHERE event_id = $1",
            vec![expanded_event.into()],
            "id",
        )
        .await;

        let failed_event =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.archived", None, None, json!({})).await;
        let failed_dispatch: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_dispatch WHERE event_id = $1",
            vec![failed_event.into()],
            "id",
        )
        .await;
        exec(
            &scratch.db,
            "UPDATE event_dispatch SET status = 'failed', expanded_at = now(), \
             last_error_code = 'expansion_failed' WHERE id = $1",
            vec![failed_dispatch.into()],
        )
        .await;

        let reference_now = Utc::now();

        // `dispatch_expanded_retention_days` = 14: 14 days old survives, 15 is reaped. The `failed`
        // row (still fresh at `reference_now`) must be unaffected by either pass.
        set_timestamp(
            &scratch.db,
            "event_dispatch",
            "expanded_at",
            "id",
            expanded_dispatch,
            reference_now - chrono::Duration::days(14),
        )
        .await;
        assert_eq!(
            super::reap_dispatch_retention(&scratch.db, reference_now)
                .await
                .expect("reaper runs"),
            0
        );
        set_timestamp(
            &scratch.db,
            "event_dispatch",
            "expanded_at",
            "id",
            expanded_dispatch,
            reference_now - chrono::Duration::days(15),
        )
        .await;
        assert_eq!(
            super::reap_dispatch_retention(&scratch.db, reference_now)
                .await
                .expect("reaper runs"),
            1
        );

        // `dispatch_failed_retention_days` = 90, deliberately far longer -- dead-letter rows must
        // stay visible long after an `expanded` row of the same age would already be gone.
        set_timestamp(
            &scratch.db,
            "event_dispatch",
            "expanded_at",
            "id",
            failed_dispatch,
            reference_now - chrono::Duration::days(15),
        )
        .await;
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_dispatch WHERE id = $1",
                vec![failed_dispatch.into()],
            )
            .await,
            1,
            "a failed (dead-letter) row must still be visible 15 days in, well past the expanded window"
        );
        set_timestamp(
            &scratch.db,
            "event_dispatch",
            "expanded_at",
            "id",
            failed_dispatch,
            reference_now - chrono::Duration::days(91),
        )
        .await;
        assert_eq!(
            super::reap_dispatch_retention(&scratch.db, reference_now)
                .await
                .expect("reaper runs"),
            1
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn subscriber_deleted_mid_flight_cancels_the_delivery_as_a_terminal_state_distinct_from_failed() {
        let scratch = scratch_or_skip!("cancelled-distinct-terminal");
        let workspace_id = seed_workspace(&scratch.db).await;
        let webhook_id = seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created"],
        )
        .await;
        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
        expand_one(&scratch.db).await.expect("expansion runs");

        // The subscriber is deleted after expansion but before the delivery is ever sent.
        delete_webhook(&scratch.db, webhook_id).await;

        let state = state_for(scratch.db.clone());
        let client = reqwest::Client::new();
        let outcome = send_one(&state, &client).await.expect("send_one runs");
        assert_eq!(outcome, Some(false));

        #[derive(sea_orm::FromQueryResult)]
        struct Row {
            status: String,
            last_error_code: Option<String>,
            attempts: i32,
        }
        let row = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status, last_error_code, attempts FROM event_deliveries WHERE event_id = $1",
            vec![event_id.into()],
        ))
        .one(&scratch.db)
        .await
        .expect("query runs")
        .expect("row exists");
        assert_eq!(
            row.status, "cancelled",
            "a gone subscriber must terminate the row as cancelled, not failed"
        );
        assert_eq!(row.last_error_code.as_deref(), Some("subscriber_gone"));
        assert_eq!(
            row.attempts, 0,
            "cancellation must not consume a retry attempt -- it is not a delivery failure"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_delivery_v08_deleting_a_subscriber_cancels_its_entire_backlog_without_dead_letter_or_audit_loss() {
        let scratch = scratch_or_skip!("subscriber-backlog-cancelled");
        let workspace_id = seed_workspace(&scratch.db).await;
        let webhook_id = seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created"],
        )
        .await;
        let mut event_ids = Vec::new();
        for _ in 0..3 {
            let event_id =
                commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
            event_ids.push(event_id);
            expand_one(&scratch.db).await.expect("expansion runs");
        }
        delete_webhook(&scratch.db, webhook_id).await;
        let state = state_for(scratch.db.clone());
        let client = reqwest::Client::new();
        for _ in 0..3 {
            assert_eq!(
                send_one(&state, &client).await.expect("cancellation attempt runs"),
                Some(false)
            );
        }
        assert_eq!(send_one(&state, &client).await.expect("queue scan runs"), None);
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries \
                  WHERE subscriber_id=$1 AND status='cancelled' AND last_error_code='subscriber_gone'",
                vec![webhook_id.into()],
            )
            .await,
            3
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE workspace_id=$1 AND status='failed'",
                vec![workspace_id.into()],
            )
            .await,
            0,
            "cancelled backlog cannot inflate dead-letter count or oldest_failed_age"
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM business_events WHERE id=ANY($1)",
                vec![event_ids.into()],
            )
            .await,
            3,
            "subscriber removal must not delete audit facts"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_delivery_v08_dispatch_and_delivery_lease_pairs_reject_both_unreachable_halves() {
        let scratch = scratch_or_skip!("lease-pair-checks");
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
        let dispatch_id: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_dispatch WHERE event_id=$1",
            vec![event_id.into()],
            "id",
        )
        .await;
        for sql in [
            "UPDATE event_dispatch SET lease_token='impossible' WHERE id=$1",
            "UPDATE event_dispatch SET lease_expires_at=now()+interval '1 minute' WHERE id=$1",
        ] {
            assert!(
                scratch
                    .db
                    .execute(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        sql,
                        vec![dispatch_id.into()],
                    ))
                    .await
                    .is_err(),
                "dispatch lease half-pair must violate its CHECK: {sql}"
            );
        }

        expand_one(&scratch.db).await.expect("expansion runs");
        let delivery_id: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_deliveries WHERE event_id=$1",
            vec![event_id.into()],
            "id",
        )
        .await;
        for sql in [
            "UPDATE event_deliveries SET lease_token='impossible' WHERE id=$1",
            "UPDATE event_deliveries SET lease_expires_at=now()+interval '1 minute' WHERE id=$1",
        ] {
            assert!(
                scratch
                    .db
                    .execute(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        sql,
                        vec![delivery_id.into()],
                    ))
                    .await
                    .is_err(),
                "delivery lease half-pair must violate its CHECK: {sql}"
            );
        }

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn delivery_dead_letter_row_is_visible_immediately_and_survives_until_its_own_retention_window() {
        let scratch = scratch_or_skip!("dead-letter-visibility");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://127.0.0.1:1/hook",
            &["flow.object.created"],
        )
        .await;
        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
        expand_one(&scratch.db).await.expect("expansion runs");

        let state = state_for(scratch.db.clone());
        let client = reqwest::Client::new();

        // Drive it through all 10 attempts (`delivery_max_attempts`) by repeatedly clearing the
        // backoff and sending again -- clock-advancement via backdating `next_attempt_at`, not
        // sleeping through the real backoff ladder.
        for attempt in 1..=10 {
            exec(
                &scratch.db,
                "UPDATE event_deliveries SET next_attempt_at = now() WHERE event_id = $1",
                vec![event_id.into()],
            )
            .await;
            let outcome = send_one(&state, &client)
                .await
                .expect("send_one runs")
                .unwrap_or_else(|| panic!("a delivery was ready to lease on attempt {attempt}"));
            assert!(!outcome, "the refused endpoint fails every attempt");
        }

        #[derive(sea_orm::FromQueryResult)]
        struct Row {
            status: String,
            attempts: i32,
            last_error_code: Option<String>,
        }
        let row = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status, attempts, last_error_code FROM event_deliveries WHERE event_id = $1",
            vec![event_id.into()],
        ))
        .one(&scratch.db)
        .await
        .expect("query runs")
        .expect("row exists");
        assert_eq!(
            row.status, "failed",
            "attempts exhausted must land in the failed dead-letter terminal state"
        );
        assert_eq!(row.attempts, 10);
        assert!(row.last_error_code.is_some());

        // Dead-letter rows must remain queryable immediately, and are not touched by the reaper
        // until their own (`delivery_retention_days`) window elapses.
        assert_eq!(
            super::reap_delivery_retention(&scratch.db, Utc::now())
                .await
                .expect("reaper runs"),
            0
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
                vec![event_id.into()],
            )
            .await,
            1
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn delivery_lease_expiry_is_reclaimed_by_its_own_reaper_content_to_sealed_plain_to_pending() {
        let scratch = scratch_or_skip!("delivery-leased-reaper");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created", "flow.content.accepted"],
        )
        .await;

        // A plain (non-content) delivery, stuck `leased` by a worker that vanished.
        let plain_event =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
        expand_one(&scratch.db).await.expect("expansion runs");
        let plain_delivery: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_deliveries WHERE event_id = $1",
            vec![plain_event.into()],
            "id",
        )
        .await;
        exec(
            &scratch.db,
            "UPDATE event_deliveries SET status = 'leased', lease_token = 'dead', \
             lease_expires_at = now() - interval '1 minute' WHERE id = $1",
            vec![plain_delivery.into()],
        )
        .await;

        // A content delivery, likewise stuck `leased`.
        let document_id = Uuid::new_v4();
        let content_event = commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(1),
            json!({}),
        )
        .await;
        expand_one(&scratch.db).await.expect("expansion runs");
        let content_delivery: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_deliveries WHERE event_id = $1",
            vec![content_event.into()],
            "id",
        )
        .await;
        exec(
            &scratch.db,
            "UPDATE event_deliveries SET status = 'leased', lease_token = 'dead', \
             lease_expires_at = now() - interval '1 minute' WHERE id = $1",
            vec![content_delivery.into()],
        )
        .await;

        // The plain lease-claim query (`status IN ('pending','sealed')`) cannot reach either row
        // while they sit at `leased` -- only the dedicated reclaim reaper can.
        let claimed_before_reclaim = send_one(&state_for(scratch.db.clone()), &reqwest::Client::new())
            .await
            .expect("send_one runs");
        assert_eq!(
            claimed_before_reclaim, None,
            "a stuck leased row must not be reachable by the ordinary lease-claim query"
        );

        let reclaimed = reclaim_expired_delivery_leases(&scratch.db)
            .await
            .expect("reclaim runs");
        assert_eq!(reclaimed, 2);

        #[derive(sea_orm::FromQueryResult)]
        struct Row {
            status: String,
            attempts: i32,
            lease_token: Option<String>,
        }
        let plain_row = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status, attempts, lease_token FROM event_deliveries WHERE id = $1",
            vec![plain_delivery.into()],
        ))
        .one(&scratch.db)
        .await
        .expect("query runs")
        .expect("row exists");
        assert_eq!(
            plain_row.status, "pending",
            "a non-content row reclaims to pending, never sealed"
        );
        assert_eq!(
            plain_row.attempts, 1,
            "lease expiry counts toward attempts, same as a delivery failure"
        );
        assert!(plain_row.lease_token.is_none());

        let content_row = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status, attempts, lease_token FROM event_deliveries WHERE id = $1",
            vec![content_delivery.into()],
        ))
        .one(&scratch.db)
        .await
        .expect("query runs")
        .expect("row exists");
        assert_eq!(
            content_row.status, "sealed",
            "a content row's reclaim goes to sealed, never back to pending, \
             per events-v1.md's 'sealed 是一个真状态'"
        );
        assert_eq!(content_row.attempts, 1);

        scratch.drop_self().await;
    }

    // =============================================================================================
    // Golden wire fixtures (`events-v1.md` "投递报文与 delivery_id 的位置"): plain / coalesced /
    // retry. This crate does not own `testing/fixtures/flow-delivery-v1/` (outside apps/api and
    // apps/worker), so the three frozen shapes are asserted inline here instead of as checked-in
    // fixture files.
    // =============================================================================================

    #[tokio::test]
    async fn golden_wire_fixture_plain_delivery_body_matches_the_frozen_shape() {
        let scratch = scratch_or_skip!("golden-plain");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created"],
        )
        .await;
        let event_id = commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.object.created",
            None,
            None,
            json!({ "object_id": "11111111-1111-1111-1111-111111111111" }),
        )
        .await;
        expand_one(&scratch.db).await.expect("expansion runs");
        let delivery_id: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_deliveries WHERE event_id = $1",
            vec![event_id.into()],
            "id",
        )
        .await;

        let body = build_delivery_body(&scratch.db, &delivery_lease_row_for_test(delivery_id, event_id, None))
            .await
            .expect("body builds");

        // A non-coalesced delivery's body is frozen as exactly
        // `{delivery:{id,attempt,coalesced:false}, event:{...openpr.event.v1}}` -- not just "some
        // fields present".
        assert_eq!(body["delivery"]["id"], json!(delivery_id));
        assert_eq!(body["delivery"]["attempt"], json!(1));
        assert_eq!(body["delivery"]["coalesced"], json!(false));
        assert_eq!(
            body["delivery"].as_object().expect("object").len(),
            3,
            "plain delivery must have exactly {{id,attempt,coalesced}}, no range/source_event_ids/block_ids_truncated"
        );
        assert_eq!(body["event"]["version"], json!("openpr.event.v1"));
        assert_eq!(body["event"]["event_id"], json!(event_id));
        assert_eq!(body["event"]["event_type"], json!("flow.object.created"));

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn golden_wire_fixture_coalesced_delivery_body_matches_the_frozen_shape() {
        let scratch = scratch_or_skip!("golden-coalesced");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.content.accepted"],
        )
        .await;
        let document_id = Uuid::new_v4();
        let block_a = Uuid::new_v4();
        let block_b = Uuid::new_v4();
        let event_1 = commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(1),
            json!({ "changed_block_ids": [block_a] }),
        )
        .await;
        commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(2),
            json!({ "changed_block_ids": [block_b] }),
        )
        .await;
        let state = state_for(scratch.db.clone());
        run_tick(&state, &reqwest::Client::new(), 8).await;

        #[derive(sea_orm::FromQueryResult)]
        struct Row {
            id: Uuid,
            first_seq: Option<i64>,
            latest_seq: Option<i64>,
        }
        let row = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, first_seq, latest_seq FROM event_deliveries WHERE subscriber_kind = 'webhook' \
             AND document_id = $1",
            vec![document_id.into()],
        ))
        .one(&scratch.db)
        .await
        .expect("query runs")
        .expect("coalesced delivery row exists");

        // Built directly from the real row's own `first_seq`/`latest_seq` (not the always-`None`
        // test constructor) so the golden `range` assertion below reflects DB truth.
        let lease_row = super::DeliveryLeaseRow {
            id: row.id,
            event_id: event_1,
            subscriber_id: Uuid::new_v4(),
            document_id: Some(document_id),
            attempts: 0,
            max_attempts: 10,
            first_seq: row.first_seq,
            latest_seq: row.latest_seq,
        };
        let body = build_delivery_body(&scratch.db, &lease_row).await.expect("body builds");

        assert_eq!(body["delivery"]["id"], json!(row.id));
        assert_eq!(body["delivery"]["coalesced"], json!(true));
        assert_eq!(body["delivery"]["range"]["first_seq"], json!(1));
        assert_eq!(body["delivery"]["range"]["latest_seq"], json!(2));
        assert_eq!(body["delivery"]["block_ids_truncated"], json!(false));
        let source_ids = body["delivery"]["source_event_ids"].as_array().expect("array");
        assert_eq!(source_ids.len(), 2);
        assert!(source_ids.contains(&json!(event_1)));
        let correct_consumer_keys = [body["delivery"]["id"].clone()]
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        let wrong_event_id_consumer_keys = source_ids.iter().cloned().collect::<std::collections::HashSet<_>>();
        assert_eq!(
            correct_consumer_keys.len(),
            1,
            "one delivery_id means one consumer delivery"
        );
        assert_eq!(
            wrong_event_id_consumer_keys.len(),
            2,
            "negative consumer fixture: treating each coalesced event_id as the dedupe key observably processes one delivery twice"
        );
        assert_eq!(body["event"]["event_type"], json!("flow.content.accepted"));
        assert_eq!(
            body["event"]["event_id"],
            json!(event_1),
            "the envelope anchors on the first source event as lineage"
        );
        let block_ids = body["event"]["payload"]["changed_block_ids"].as_array().expect("array");
        assert_eq!(block_ids.len(), 2);
        assert!(block_ids.contains(&json!(block_a)));
        assert!(block_ids.contains(&json!(block_b)));

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_delivery_v08_changed_block_union_exact_limit_and_plus_one_truncation() {
        let scratch = scratch_or_skip!("changed-block-union-boundary");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.content.accepted"],
        )
        .await;
        let document_id = Uuid::new_v4();
        let block_ids = (0..200).map(|_| Uuid::new_v4()).collect::<Vec<_>>();
        let event_1 = commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(1),
            json!({ "changed_block_ids": &block_ids[..100] }),
        )
        .await;
        commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(2),
            json!({ "changed_block_ids": &block_ids[100..] }),
        )
        .await;
        expand_one(&scratch.db).await.expect("first expansion runs");
        expand_one(&scratch.db).await.expect("second expansion runs");

        #[derive(FromQueryResult)]
        struct DeliveryRange {
            id: Uuid,
            first_seq: Option<i64>,
            latest_seq: Option<i64>,
        }
        let row = DeliveryRange::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id,first_seq,latest_seq FROM event_deliveries WHERE subscriber_kind='webhook' AND document_id=$1",
            vec![document_id.into()],
        ))
        .one(&scratch.db)
        .await
        .expect("delivery query runs")
        .expect("coalesced row exists");
        let lease = super::DeliveryLeaseRow {
            id: row.id,
            event_id: event_1,
            subscriber_id: Uuid::new_v4(),
            document_id: Some(document_id),
            attempts: 0,
            max_attempts: 10,
            first_seq: row.first_seq,
            latest_seq: row.latest_seq,
        };
        let exact = build_delivery_body(&scratch.db, &lease)
            .await
            .expect("exact body builds");
        assert_eq!(exact["delivery"]["block_ids_truncated"], json!(false));
        let exact_ids = exact["event"]["payload"]["changed_block_ids"]
            .as_array()
            .expect("exact union is present");
        assert_eq!(exact_ids.len(), 200);
        assert_eq!(
            exact_ids
                .iter()
                .cloned()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            block_ids.len()
        );

        let forbidden_body_id = Uuid::new_v4();
        commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(3),
            json!({ "changed_block_ids": [forbidden_body_id], "body": "MUST-NOT-LEAK" }),
        )
        .await;
        expand_one(&scratch.db).await.expect("plus-one expansion runs");
        let plus_one_range = DeliveryRange::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id,first_seq,latest_seq FROM event_deliveries WHERE id=$1",
            vec![row.id.into()],
        ))
        .one(&scratch.db)
        .await
        .expect("delivery query runs")
        .expect("same delivery remains");
        let plus_one = build_delivery_body(
            &scratch.db,
            &super::DeliveryLeaseRow {
                latest_seq: plus_one_range.latest_seq,
                ..lease
            },
        )
        .await
        .expect("plus-one body builds");
        assert_eq!(plus_one["delivery"]["range"], json!({"first_seq": 1, "latest_seq": 3}));
        assert_eq!(plus_one["delivery"]["block_ids_truncated"], json!(true));
        assert_eq!(
            plus_one["event"]["payload"],
            json!({"changed_block_ids_truncated": true})
        );
        let encoded = serde_json::to_string(&plus_one).expect("body serializes");
        assert!(!encoded.contains(&forbidden_body_id.to_string()));
        assert!(!encoded.contains("MUST-NOT-LEAK"));

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn golden_wire_fixture_retry_reuses_delivery_id_and_only_advances_attempt() {
        let scratch = scratch_or_skip!("golden-retry");
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
        expand_one(&scratch.db).await.expect("expansion runs");
        let delivery_id: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_deliveries WHERE event_id = $1",
            vec![event_id.into()],
            "id",
        )
        .await;

        let attempt_1 = delivery_lease_row_for_test(delivery_id, event_id, None);
        let mut attempt_4 = delivery_lease_row_for_test(delivery_id, event_id, None);
        attempt_4.attempts = 3;

        let body_1 = build_delivery_body(&scratch.db, &attempt_1).await.expect("body builds");
        let body_4 = build_delivery_body(&scratch.db, &attempt_4).await.expect("body builds");

        assert_eq!(
            body_1["delivery"]["id"], body_4["delivery"]["id"],
            "retries must carry the same delivery_id header/body value -- the consumer's dedup key"
        );
        assert_eq!(body_1["delivery"]["attempt"], json!(1));
        assert_eq!(body_4["delivery"]["attempt"], json!(4));
        assert_eq!(
            body_1["event"], body_4["event"],
            "the event envelope itself is identical across retries"
        );

        scratch.drop_self().await;
    }

    /// The merged-row counterpart of `re_expanding_the_same_work_after_a_simulated_crash_...`
    /// above: `ADR-0011` Â§2.1 requires the crash/re-expansion idempotency proof specifically for a
    /// *coalesced* delivery (multiple source events bound to one row), not just a plain one-event
    /// delivery -- the two have different code paths (`bind_content_delivery`'s merge branch vs.
    /// `bind_plain_delivery`) and a bug could exist in either independently.
    #[tokio::test]
    async fn re_expanding_a_coalesced_delivery_after_a_simulated_crash_does_not_duplicate_source_rows() {
        let scratch = scratch_or_skip!("reexpansion-idempotent-coalesced");
        let workspace_id = seed_workspace(&scratch.db).await;
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.content.accepted"],
        )
        .await;
        let document_id = Uuid::new_v4();
        for seq in 1..=3 {
            commit_dispatch_work(
                &scratch.db,
                workspace_id,
                "flow.content.accepted",
                Some(document_id),
                Some(seq),
                json!({}),
            )
            .await;
        }
        let report = run_tick(&state_for(scratch.db.clone()), &reqwest::Client::new(), 8).await;
        assert_eq!(report.expanded, 3, "{report:?}");

        let delivery_id: Uuid = get_col(
            &scratch.db,
            "SELECT id FROM event_deliveries WHERE subscriber_kind = 'webhook' AND document_id = $1",
            vec![document_id.into()],
            "id",
        )
        .await;
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_delivery_sources WHERE delivery_id = $1",
                vec![delivery_id.into()],
            )
            .await,
            3,
            "all 3 merged accepted updates must be registered as source rows before the simulated crash"
        );

        // Simulate the dispatcher crashing after all 3 work items were already expanded into the
        // one merged delivery row: put all 3 dispatch rows back to pending, exactly what
        // `reclaim_expired_dispatch_leases` does to abandoned leases.
        exec(
            &scratch.db,
            "UPDATE event_dispatch SET status = 'pending', expanded_at = NULL, lease_token = NULL, \
             lease_expires_at = NULL WHERE document_id = $1",
            vec![document_id.into()],
        )
        .await;
        let report2 = run_tick(&state_for(scratch.db.clone()), &reqwest::Client::new(), 8).await;
        assert_eq!(report2.expanded, 3, "{report2:?}");

        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_delivery_sources WHERE delivery_id = $1",
                vec![delivery_id.into()],
            )
            .await,
            3,
            "no source event may be counted twice across the crash + re-expansion (ADR-0011 Â§2.1)"
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE subscriber_kind = 'webhook' AND document_id = $1",
                vec![document_id.into()],
            )
            .await,
            1,
            "no second delivery row was minted by the re-expansion"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn replay_is_windowed_deduplicated_and_crosses_delivery_retention_without_duplication() {
        let scratch = scratch_or_skip!("v08-replay-retention");
        let workspace_id = seed_workspace(&scratch.db).await;
        let webhook_id = seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created"],
        )
        .await;
        let now = Utc::now();
        let event_id = commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.object.created",
            None,
            None,
            json!({"object_id": Uuid::new_v4()}),
        )
        .await;
        exec(
            &scratch.db,
            "DELETE FROM event_dispatch WHERE event_id=$1",
            vec![event_id.into()],
        )
        .await;
        exec(
            &scratch.db,
            "UPDATE business_events SET created_at=$2 WHERE id=$1",
            vec![event_id.into(), (now - chrono::Duration::days(40)).into()],
        )
        .await;
        let request = ReplayRequest {
            workspace_id,
            mode: ReplayMode::Rebuild,
            event_type: Some("flow.object.created".to_string()),
            subscriber_kind: Some("webhook".to_string()),
            subscriber_id: Some(webhook_id),
            from: now - chrono::Duration::days(41),
            to: now - chrono::Duration::days(39),
            dry_run: true,
        };

        let preview = replay_deliveries(&scratch.db, &request, now)
            .await
            .expect("preview succeeds");
        assert!(matches!(
            preview,
            ReplayResult::Rebuild {
                replayed: 1,
                skipped_already_delivered: 0,
                ref rebuilt_delivery_ids,
                ..
            } if rebuilt_delivery_ids.is_empty()
        ));
        assert_eq!(
            count(&scratch.db, "SELECT count(*) AS n FROM event_deliveries", vec![]).await,
            0,
            "dry-run must write no canonical delivery or source row"
        );

        let mut execute = request.clone();
        execute.dry_run = false;
        let first = replay_deliveries(&scratch.db, &execute, now)
            .await
            .expect("rebuild succeeds");
        let delivery_id = match first {
            ReplayResult::Rebuild {
                replayed: 1,
                skipped_already_delivered: 0,
                rebuilt_delivery_ids,
                ..
            } => rebuilt_delivery_ids[0],
            other => panic!("unexpected first replay result: {other:?}"),
        };
        exec(
            &scratch.db,
            "UPDATE event_delivery_sources SET created_at=$2 WHERE delivery_id=$1",
            vec![delivery_id.into(), (now - chrono::Duration::days(40)).into()],
        )
        .await;
        exec(
            &scratch.db,
            "UPDATE event_deliveries SET status='dispatched', terminated_at=$2 WHERE id=$1",
            vec![delivery_id.into(), (now - chrono::Duration::days(31)).into()],
        )
        .await;
        assert_eq!(reap_delivery_retention(&scratch.db, now).await.expect("reaper runs"), 1);
        assert_eq!(
            reap_delivery_source_tombstones(&scratch.db, now)
                .await
                .expect("source reaper runs"),
            0,
            "a 40-day source tombstone must survive the proposed 90-day retention"
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_delivery_sources \
                  WHERE source_event_id=$1 AND delivery_id IS NULL",
                vec![event_id.into()],
            )
            .await,
            1,
            "the source tombstone must outlive the 30-day delivery"
        );

        let second = replay_deliveries(&scratch.db, &execute, now)
            .await
            .expect("second replay succeeds");
        assert!(matches!(
            second,
            ReplayResult::Rebuild {
                replayed: 0,
                skipped_already_delivered: 1,
                ref rebuilt_delivery_ids,
                ..
            } if rebuilt_delivery_ids.is_empty()
        ));
        assert_eq!(
            count(&scratch.db, "SELECT count(*) AS n FROM event_deliveries", vec![]).await,
            0,
            "a retained source tombstone must prevent a replacement delivery"
        );

        let mut exact_boundary = request;
        exact_boundary.from = now - chrono::Duration::days(REPLAY_MAX_WINDOW_DAYS);
        assert!(replay_deliveries(&scratch.db, &exact_boundary, now).await.is_err());
        exact_boundary.from += chrono::Duration::milliseconds(1);
        assert!(replay_deliveries(&scratch.db, &exact_boundary, now).await.is_ok());

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_delivery_v08_concurrent_replay_reserves_before_building_and_never_merges() {
        let scratch = scratch_or_skip!("v08-concurrent-replay");
        let workspace_id = seed_workspace(&scratch.db).await;
        let webhook_id = seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created"],
        )
        .await;
        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
        exec(
            &scratch.db,
            "DELETE FROM event_dispatch WHERE event_id=$1",
            vec![event_id.into()],
        )
        .await;

        // Negative fixture for an implementation that copies a document id into replay rows: a
        // live content row already occupies the partial-unique coalescing key. Correct replay rows
        // keep document_id/dispatch_id/range NULL, so they insert independently.
        let live_content_delivery = Uuid::new_v4();
        exec(
            &scratch.db,
            "INSERT INTO event_deliveries \
               (id,event_id,workspace_id,subscriber_kind,subscriber_id,document_id,status,next_attempt_at) \
             VALUES ($1,$2,$3,'webhook',$4,$5,'pending',now()+interval '1 hour')",
            vec![
                live_content_delivery.into(),
                event_id.into(),
                workspace_id.into(),
                webhook_id.into(),
                Uuid::new_v4().into(),
            ],
        )
        .await;

        let now = Utc::now();
        let request = ReplayRequest {
            workspace_id,
            mode: ReplayMode::Rebuild,
            event_type: Some("flow.object.created".to_string()),
            subscriber_kind: Some("webhook".to_string()),
            subscriber_id: Some(webhook_id),
            from: now - chrono::Duration::minutes(1),
            to: now,
            dry_run: false,
        };
        let second = scratch.second_connection().await;
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let (left, right) = tokio::join!(
            TEST_REPLAY_CANDIDATE_BARRIER.scope(barrier.clone(), replay_deliveries(&scratch.db, &request, now),),
            TEST_REPLAY_CANDIDATE_BARRIER.scope(barrier, replay_deliveries(&second, &request, now)),
        );
        let results = [
            left.expect("first concurrent replay completes"),
            right.expect("second concurrent replay completes"),
        ];
        let replayed = results
            .iter()
            .map(|result| match result {
                ReplayResult::Rebuild { replayed, .. } => *replayed,
                other => panic!("unexpected concurrent replay result: {other:?}"),
            })
            .sum::<u64>();
        let skipped = results
            .iter()
            .map(|result| match result {
                ReplayResult::Rebuild {
                    skipped_already_delivered,
                    ..
                } => *skipped_already_delivered,
                other => panic!("unexpected concurrent replay result: {other:?}"),
            })
            .sum::<u64>();
        assert_eq!(
            (replayed, skipped),
            (1, 1),
            "the source unique key is a write-before-build reservation"
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_delivery_sources \
                  WHERE subscriber_id=$1 AND source_event_id=$2",
                vec![webhook_id.into(), event_id.into()],
            )
            .await,
            1
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries \
                  WHERE subscriber_id=$1 AND event_id=$2 AND dispatch_id IS NULL AND document_id IS NULL \
                    AND first_seq IS NULL AND latest_seq IS NULL AND status='pending'",
                vec![webhook_id.into(), event_id.into()],
            )
            .await,
            1,
            "exactly one standalone replay row is created beside the active content row"
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE id=$1 AND status='pending'",
                vec![live_content_delivery.into()],
            )
            .await,
            1,
            "replay must neither collide with nor merge into the active content delivery"
        );
        let third = replay_deliveries(&scratch.db, &request, now)
            .await
            .expect("sequential replay completes");
        assert!(matches!(
            third,
            ReplayResult::Rebuild {
                replayed: 0,
                skipped_already_delivered: 1,
                ref rebuilt_delivery_ids,
                ..
            } if rebuilt_delivery_ids.is_empty()
        ));

        second.close().await.expect("second connection closes");
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn requeue_failed_filters_terminated_time_and_preserves_delivery_id() {
        let scratch = scratch_or_skip!("v08-requeue-failed");
        let workspace_id = seed_workspace(&scratch.db).await;
        let webhook_id = seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created"],
        )
        .await;
        let now = Utc::now();
        let event_id = commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.object.created",
            None,
            None,
            json!({"object_id": Uuid::new_v4()}),
        )
        .await;
        exec(
            &scratch.db,
            "DELETE FROM event_dispatch WHERE event_id=$1",
            vec![event_id.into()],
        )
        .await;
        exec(
            &scratch.db,
            "UPDATE business_events SET created_at=$2 WHERE id=$1",
            vec![event_id.into(), (now - chrono::Duration::days(120)).into()],
        )
        .await;
        let delivery_id = Uuid::new_v4();
        exec(
            &scratch.db,
            "INSERT INTO event_deliveries \
               (id,event_id,workspace_id,subscriber_kind,subscriber_id,document_id,status,attempts,terminated_at) \
             VALUES ($1,$2,$3,'webhook',$4,$5,'failed',10,$6)",
            vec![
                delivery_id.into(),
                event_id.into(),
                workspace_id.into(),
                webhook_id.into(),
                Uuid::new_v4().into(),
                (now - chrono::Duration::days(1)).into(),
            ],
        )
        .await;
        exec(
            &scratch.db,
            "INSERT INTO event_delivery_sources \
               (workspace_id,delivery_id,subscriber_kind,subscriber_id,source_event_id) \
             VALUES ($1,$2,'webhook',$3,$4)",
            vec![
                workspace_id.into(),
                delivery_id.into(),
                webhook_id.into(),
                event_id.into(),
            ],
        )
        .await;
        let request = ReplayRequest {
            workspace_id,
            mode: ReplayMode::RequeueFailed,
            event_type: Some("flow.object.created".to_string()),
            subscriber_kind: Some("webhook".to_string()),
            subscriber_id: Some(webhook_id),
            from: now - chrono::Duration::days(2),
            to: now,
            dry_run: false,
        };
        let result = replay_deliveries(&scratch.db, &request, now)
            .await
            .expect("requeue succeeds");
        assert_eq!(
            result,
            ReplayResult::RequeueFailed {
                requeued: 1,
                skipped_not_failed: 0,
                requeued_delivery_ids: vec![delivery_id],
                window: super::ReplayWindow {
                    from: request.from,
                    to: request.to,
                },
            }
        );
        let status: String = get_col(
            &scratch.db,
            "SELECT status FROM event_deliveries WHERE id=$1",
            vec![delivery_id.into()],
            "status",
        )
        .await;
        let document_id: Option<Uuid> = get_col(
            &scratch.db,
            "SELECT document_id FROM event_deliveries WHERE id=$1",
            vec![delivery_id.into()],
            "document_id",
        )
        .await;
        assert_eq!(status, "pending");
        assert_eq!(document_id, None);
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_delivery_sources WHERE delivery_id=$1",
                vec![delivery_id.into()],
            )
            .await,
            1,
            "requeue must not replace or duplicate the source tombstone"
        );

        scratch.drop_self().await;
    }
}
