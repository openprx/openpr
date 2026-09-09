//! Sylvode Flow v0.4 — **delivery-path numeric budget measurement harness**.
//!
//! `contracts/limits-v1.md` "Delivery path budgets" leaves thirteen values at `status: unset`, each
//! carrying `set_by: v0.4 实现者` and its own `rule` (the derivation the value must follow), plus
//! the instruction **"冻结前不得填入自造数值"** — no invented number may be frozen. The gate
//! `dispatch_numeric_budgets_locked` refuses the release while they stay unset.
//!
//! This file does not freeze anything and does not read the contract. It **runs the real
//! dispatcher** (`api::events::dispatcher::run_tick`) against a real `PostgreSQL` and a real local
//! HTTP subscriber, and measures the quantities each `rule` names, so that a value can be proposed
//! with evidence behind it instead of a plausible-sounding constant.
//!
//! | Section | Measures | `rule` it feeds |
//! |---|---|---|
//! | `baseline` | fixed per-tick cost with nothing to do | subtrahend for every other timing below |
//! | `expansion_by_subscriber_count` | expansion transaction duration vs. active subscribers | `dispatch_lease_ttl_ms` ("展开事务 p99 时长的若干倍"), `subscribers_per_workspace_max` ("单次展开的插入量在单次 lease 预算内完成") |
//! | `same_document_backlog_drain` | ticks and wall time to drain N same-document work items | `dispatch_head_wait_backoff_ms` ("N 条积压的排空时间与 N 成线性而非与阶梯和成正比") |
//! | `expansion_failure_ladder` | the retry ladder the implementation actually writes, and where it terminates | `dispatch_backoff_ms` ("形态对齐 `delivery_backoff_ms`"), `dispatch_max_attempts` |
//! | `lease_reclaim_accounting` | reclaims charged per vanished worker, and the observed ceiling | `dispatch_max_lease_reclaims` ("显著大于一次发布窗口内的预期重启次数") |
//! | `webhook_round_trip` | real HTTP delivery attempt duration, and what a timeout costs | `webhook_request_timeout_ms`, `delivery_lease_ttl_ms` ("≥ webhook 请求超时 + 安全余量") |
//! | `coalescing_window` | source events merged into one delivery under the frozen 10 updates/s + 2,000 ms debounce | `coalesced_source_events_max` ("debounce 窗口内单文档合并事件数的 p99 上界") |
//! | `delivery_body_size` | delivery body bytes as a function of block-id and source-event counts | `changed_block_ids_per_delivery_max` ("投递体在 p99 合并窗口下显著小于订阅端常见请求体上限") |
//! | `expanded_row_footprint` | bytes per retained `expanded` row and head-of-queue query cost vs. table size | `dispatch_expanded_retention_days` ("使队首查询所在表规模有界") |
//!
//! # Measurement caliber
//!
//! Every number is taken from one of three authorities, never from the implementation's own
//! self-report:
//!
//! 1. **Wall clock around `run_tick`**, with a separately measured empty-tick baseline subtracted,
//!    so a per-tick fixed cost is not charged to the work item.
//! 2. **`PostgreSQL` rows** (`event_dispatch`, `event_deliveries`, `event_delivery_sources`) and
//!    `pg_total_relation_size`, read after the fact.
//! 3. **A real HTTP server in this process** that records the exact bytes the dispatcher sent and
//!    when — the subscriber's own view, not the sender's.
//!
//! Constants the implementation keeps private (the lease TTL, the backoff ladder, the reclaim
//! ceiling, the attempt ceiling) are **discovered empirically** here rather than imported, so this
//! file measures behaviour rather than restating source.
//!
//! # Fault injection
//!
//! The expansion-failure ladder needs expansion to actually fail after the work item is leased.
//! That is done with a `BEFORE INSERT` trigger on `event_deliveries` in the scratch database,
//! which raises. It is database-side, temporary, and touches no application source.
//!
//! # Honest failure
//!
//! Structural expectations (FIFO order, exhaustion terminal state, no duplicate delivery) are
//! asserted and land in `violations`. Timing distributions are reported, never gated: this file
//! exists precisely because no threshold is frozen yet.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::too_many_arguments
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use platform::{
    app::AppState,
    config::{
        AppConfig, AuditConfig, AuthConfig, DatabaseConfig, FlowConfig, LoggingConfig, McpConfig, MigrationsConfig,
        OpenPrConfig, OutboundConfig, Secret, ServerConfig, StorageBackend, StorageConfig,
    },
};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

use api::events::dispatcher::run_tick;

const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";
const OUT_ENV: &str = "OPENPR_FLOW_DISPATCH_BUDGET_OUT";

/// Subscriber counts the expansion cost is sampled at. The point is to find where expansion stops
/// fitting inside a lease, not to confirm that eight subscribers are cheap, so the ladder doubles
/// until it reaches the ceiling itself: `subscribers_per_workspace_max` was frozen at 100 on
/// 2026-08-31 and the dispatcher now refuses to expand a workspace past it, so a rung above 100
/// would measure a state the implementation no longer permits. The affine fit over these rungs is
/// what extrapolates the cost beyond the ceiling.
const SUBSCRIBER_LADDER: [usize; 8] = [1, 2, 4, 8, 16, 32, 64, 100];
/// Work items expanded per subscriber-count rung.
const EXPANSIONS_PER_RUNG: usize = 25;
/// Empty ticks used to establish the fixed per-tick cost.
const BASELINE_TICKS: usize = 40;
/// Backlog depths the same-document drain is measured at.
const BACKLOG_LADDER: [usize; 4] = [4, 8, 16, 32];
/// The frozen per-connection update rate (`limits-v1.md`), used to pace the coalescing fixture.
const UPDATES_PER_SECOND_CAP: usize = 10;
/// The frozen content debounce (`limits-v1.md`), in milliseconds.
const CONTENT_DEBOUNCE_MS: u64 = 2_000;
/// Debounce windows the coalescing fixture runs for.
const COALESCING_WINDOWS: usize = 8;
/// Block-id counts the delivery body size is sampled at.
const BLOCK_ID_LADDER: [usize; 5] = [2, 10, 50, 100, 200];
/// Retained-row counts the head-of-queue query is timed at.
const FOOTPRINT_LADDER: [usize; 3] = [10_000, 50_000, 200_000];

// ---------------------------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------------------------

fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = (quantile * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        f64::NAN
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn distribution(samples: &[f64]) -> Value {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    json!({
        "n": sorted.len(),
        "p50_ms": percentile(&sorted, 0.50),
        "p90_ms": percentile(&sorted, 0.90),
        "p95_ms": percentile(&sorted, 0.95),
        "p99_ms": percentile(&sorted, 0.99),
        "max_ms": sorted.last().copied().unwrap_or(f64::NAN),
        "mean_ms": mean(&sorted),
    })
}

fn p(samples: &[f64], quantile: f64) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    percentile(&sorted, quantile)
}

/// Least-squares fit of `y = intercept + slope * x`.
fn linear_fit(points: &[(f64, f64)]) -> (f64, f64) {
    let n = points.len() as f64;
    if n < 2.0 {
        return (f64::NAN, f64::NAN);
    }
    let mean_x = points.iter().map(|(x, _)| *x).sum::<f64>() / n;
    let mean_y = points.iter().map(|(_, y)| *y).sum::<f64>() / n;
    let mut numerator = 0.0;
    let mut denominator = 0.0;
    for (x, y) in points {
        numerator = (x - mean_x).mul_add(y - mean_y, numerator);
        denominator = (x - mean_x).mul_add(x - mean_x, denominator);
    }
    if denominator == 0.0 {
        return (f64::NAN, f64::NAN);
    }
    let slope = numerator / denominator;
    (mean_y - slope * mean_x, slope)
}

// ---------------------------------------------------------------------------------------------
// Local HTTP subscriber
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Received {
    delivery_id: Option<String>,
    body_bytes: usize,
    body: String,
    at: Instant,
}

/// A minimal HTTP/1.1 endpoint the dispatcher can really POST to. It records exactly what arrived,
/// which is the only view of the delivery body that is not the sender's own.
///
/// `/hook` answers 200 immediately. `/slow` sleeps past any plausible client timeout before
/// answering, which is how the request-timeout budget is observed rather than assumed.
struct Receiver {
    port: u16,
    received: Arc<Mutex<Vec<Received>>>,
}

impl Receiver {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("receiver binds");
        let port = listener.local_addr().expect("receiver has an address").port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&received);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let sink = Arc::clone(&sink);
                tokio::spawn(async move {
                    serve_one(stream, sink).await;
                });
            }
        });
        Self { port, received }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn take(&self) -> Vec<Received> {
        std::mem::take(&mut *self.received.lock())
    }

    fn len(&self) -> usize {
        self.received.lock().len()
    }
}

async fn serve_one(mut stream: TcpStream, sink: Arc<Mutex<Vec<Received>>>) {
    let mut buffer = Vec::with_capacity(8192);
    let mut chunk = [0u8; 4096];
    // Read until the headers are complete, then until Content-Length bytes of body have arrived.
    let header_end = loop {
        let Ok(read) = stream.read(&mut chunk).await else {
            return;
        };
        if read == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = find_header_end(&buffer) {
            break position;
        }
        if buffer.len() > 64 * 1024 * 1024 {
            return;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let path = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_string();
    let delivery_id = header_value(&head, "x-sylvode-delivery-id");
    let content_length = header_value(&head, "content-length")
        .and_then(|raw| raw.parse::<usize>().ok())
        .unwrap_or(0);
    while buffer.len() < header_end + content_length {
        let Ok(read) = stream.read(&mut chunk).await else {
            return;
        };
        if read == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = String::from_utf8_lossy(&buffer[header_end..header_end + content_length]).to_string();
    sink.lock().push(Received {
        delivery_id,
        body_bytes: content_length,
        body,
        at: Instant::now(),
    });

    if path.starts_with("/slow") {
        // Longer than any client timeout this harness builds, so the abort is the client's.
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
    let _ = stream
        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
        .await;
    let _ = stream.flush().await;
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|at| at + 4)
}

fn header_value(head: &str, name: &str) -> Option<String> {
    head.lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim().to_string())
}

// ---------------------------------------------------------------------------------------------
// Scratch database
// ---------------------------------------------------------------------------------------------

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
    let name = format!("openpr_flow_dispatch_budget_{label}");
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
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
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

// ---------------------------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------------------------

/// Publishes an outbound allowlist containing only the local receiver, so `validate_outbound_url`
/// admits it. `parse_outbound_url` rejects a literal loopback address otherwise, and a delivery
/// that never leaves the process would measure nothing.
///
/// Must run before anything else reads `api::config::runtime()`: that read seals the `OnceLock`.
fn install_config(receiver_port: u16) -> Result<(), String> {
    api::config::install(&OpenPrConfig {
        origin: PathBuf::from("dispatch-budget-evidence"),
        server: ServerConfig::default(),
        database: DatabaseConfig::default(),
        auth: AuthConfig::default(),
        logging: LoggingConfig::default(),
        storage: StorageConfig {
            backend: StorageBackend::Local,
            dir: PathBuf::from("."),
            s3: None,
        },
        audit: AuditConfig::default(),
        migrations: MigrationsConfig::default(),
        outbound: OutboundConfig {
            allowed_hosts: vec![format!("127.0.0.1:{receiver_port}")],
            allow_private: false,
        },
        mcp: McpConfig::default(),
        flow: FlowConfig::default(),
    })
}

fn state_for(db: DatabaseConnection) -> AppState {
    AppState {
        cfg: AppConfig {
            app_name: "dispatch-budget-evidence".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new("dispatch-budget-secret"),
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
        .unwrap_or_else(|err| panic!("statement failed: {err}\n{sql}"));
}

async fn raw(db: &DatabaseConnection, sql: &str) {
    db.execute_unprepared(sql)
        .await
        .unwrap_or_else(|err| panic!("statement failed: {err}\n{sql}"));
}

#[derive(FromQueryResult)]
struct CountRow {
    n: i64,
}

async fn count(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> i64 {
    CountRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .one(db)
        .await
        .expect("count query runs")
        .expect("count query returns a row")
        .n
}

async fn seed_workspace(db: &DatabaseConnection) -> (Uuid, Uuid) {
    let workspace_id = Uuid::new_v4();
    let owner_id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO users (id, email, password_hash, name, role, is_active) \
         VALUES ($1, $2, '!', 'dispatch budget', 'user', true)",
        vec![owner_id.into(), format!("{owner_id}@dispatch-budget.test").into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'dispatch budget', $3)",
        vec![
            workspace_id.into(),
            format!("ws-{workspace_id}").into(),
            owner_id.into(),
        ],
    )
    .await;
    exec(
        db,
        "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
        vec![workspace_id.into(), owner_id.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO flow_workspace_settings (workspace_id, flow_enabled) VALUES ($1, true)",
        vec![workspace_id.into()],
    )
    .await;
    (workspace_id, owner_id)
}

async fn seed_webhook(db: &DatabaseConnection, workspace_id: Uuid, owner_id: Uuid, url: &str, events: &[&str]) -> Uuid {
    let webhook_id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO webhooks (id, workspace_id, name, url, secret, events, active, created_by) \
         VALUES ($1, $2, 'budget hook', $3, 'shh', $4::jsonb, true, $5)",
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

/// One committed domain transition: a `business_events` row plus the single `event_dispatch` work
/// row, exactly as `flow::collab::write` writes them. The dispatcher cannot tell this apart from a
/// real write, and volume/shape can be controlled precisely.
async fn commit_work(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    event_type: &str,
    document_id: Option<Uuid>,
    accepted_seq: Option<i64>,
    payload: &Value,
    max_attempts: i32,
) -> (Uuid, Uuid) {
    let event_id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO business_events (id, workspace_id, event_type, aggregate_type, aggregate_id, source, payload, metadata) \
         VALUES ($1, $2, $3, 'flow_object', $4, '{\"surface\":\"harness\"}'::jsonb, $5::jsonb, '{}'::jsonb)",
        vec![
            event_id.into(),
            workspace_id.into(),
            event_type.into(),
            Uuid::new_v4().to_string().into(),
            payload.clone().into(),
        ],
    )
    .await;
    let dispatch_id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO event_dispatch (id, event_id, workspace_id, event_type, document_id, accepted_seq, max_attempts) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
        vec![
            dispatch_id.into(),
            event_id.into(),
            workspace_id.into(),
            event_type.into(),
            document_id.into(),
            accepted_seq.into(),
            max_attempts.into(),
        ],
    )
    .await;
    (dispatch_id, event_id)
}

/// Pushes every existing delivery row out of the send loop's reach, so a timed tick measures
/// expansion and nothing else.
async fn park_deliveries(db: &DatabaseConnection) {
    raw(
        db,
        "UPDATE event_deliveries SET next_attempt_at = now() + interval '1 hour' \
         WHERE status IN ('pending', 'sealed')",
    )
    .await;
}

/// Holds every newly expanded delivery out of the send loop, so expansion and sending can be timed
/// separately. Without it a plain delivery is created with `next_attempt_at = now()` and is sent by
/// the *same* tick that expanded it, and neither number means anything on its own.
///
/// Database-side and temporary; no application source is touched.
async fn defer_new_deliveries(db: &DatabaseConnection) {
    raw(
        db,
        "CREATE OR REPLACE FUNCTION dispatch_budget_defer() RETURNS trigger AS $$ \
         BEGIN NEW.next_attempt_at := now() + interval '1 hour'; RETURN NEW; END; \
         $$ LANGUAGE plpgsql",
    )
    .await;
    raw(
        db,
        "CREATE TRIGGER dispatch_budget_defer_trigger BEFORE INSERT ON event_deliveries \
         FOR EACH ROW EXECUTE FUNCTION dispatch_budget_defer()",
    )
    .await;
}

/// Removes the hold and makes every waiting delivery sendable now.
async fn release_deferred_deliveries(db: &DatabaseConnection) {
    raw(
        db,
        "DROP TRIGGER IF EXISTS dispatch_budget_defer_trigger ON event_deliveries",
    )
    .await;
    raw(db, "DROP FUNCTION IF EXISTS dispatch_budget_defer()").await;
    raw(
        db,
        "UPDATE event_deliveries SET next_attempt_at = now() WHERE status IN ('pending', 'sealed')",
    )
    .await;
}

fn fast_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client builds")
}

// ---------------------------------------------------------------------------------------------
// Section: baseline
// ---------------------------------------------------------------------------------------------

/// The fixed cost of a tick with an empty backlog: two lease reclaims, two claim probes, three
/// retention reapers and two age queries. Subtracting it is what turns a tick measurement into an
/// expansion measurement.
async fn section_baseline(state: &AppState, client: &reqwest::Client) -> (Value, f64) {
    let mut samples = Vec::with_capacity(BASELINE_TICKS);
    for _ in 0..BASELINE_TICKS {
        let started = Instant::now();
        let report = run_tick(state, client, 1).await;
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(report.expanded, 0, "baseline tick expanded something");
        assert_eq!(report.delivered, 0, "baseline tick delivered something");
    }
    let median = p(&samples, 0.50);
    (
        json!({
            "shape": "run_tick against an empty backlog: 2 lease reclaims + 2 claim probes + 3 \
                      retention reapers + 2 age queries, no work item and no delivery",
            "empty_tick": distribution(&samples),
        }),
        median,
    )
}

// ---------------------------------------------------------------------------------------------
// Section: expansion cost by subscriber count
// ---------------------------------------------------------------------------------------------

async fn section_expansion_by_subscriber_count(
    db: &DatabaseConnection,
    state: &AppState,
    client: &reqwest::Client,
    workspace_id: Uuid,
    owner_id: Uuid,
    receiver: &Receiver,
    baseline_ms: f64,
    violations: &mut Vec<String>,
) -> Value {
    let mut rungs = Vec::with_capacity(SUBSCRIBER_LADDER.len());
    let mut fit_points: Vec<(f64, f64)> = Vec::with_capacity(SUBSCRIBER_LADDER.len());
    let mut installed = 0usize;

    for subscribers in SUBSCRIBER_LADDER {
        while installed < subscribers {
            seed_webhook(
                db,
                workspace_id,
                owner_id,
                &receiver.url("/hook"),
                &["flow.content.accepted"],
            )
            .await;
            installed += 1;
        }

        // Distinct documents so the same-document FIFO barrier never applies and every work item
        // is immediately selectable: this rung measures fan-out width, not queue order.
        for index in 0..EXPANSIONS_PER_RUNG {
            commit_work(
                db,
                workspace_id,
                "flow.content.accepted",
                Some(Uuid::new_v4()),
                Some(index as i64 + 1),
                &json!({ "changed_block_ids": [Uuid::new_v4()] }),
                10,
            )
            .await;
        }

        let mut samples = Vec::with_capacity(EXPANSIONS_PER_RUNG);
        let mut expanded_total = 0u64;
        for _ in 0..EXPANSIONS_PER_RUNG {
            // Untimed: keep the send loop starved so the timed tick is expansion only.
            park_deliveries(db).await;
            let started = Instant::now();
            let report = run_tick(state, client, 1).await;
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            if report.expanded == 1 {
                samples.push(elapsed);
                expanded_total += 1;
            }
            if report.delivered > 0 {
                violations.push(format!(
                    "expansion rung {subscribers}: a timed tick also delivered {} row(s); the \
                     measurement is contaminated",
                    report.delivered
                ));
            }
        }
        park_deliveries(db).await;

        if expanded_total as usize != EXPANSIONS_PER_RUNG {
            violations.push(format!(
                "expansion rung {subscribers}: only {expanded_total} of {EXPANSIONS_PER_RUNG} work \
                 items expanded"
            ));
        }
        let deliveries_created = count(
            db,
            "SELECT count(*) AS n FROM event_deliveries WHERE workspace_id = $1",
            vec![workspace_id.into()],
        )
        .await;

        let net: Vec<f64> = samples.iter().map(|value| (value - baseline_ms).max(0.0)).collect();
        let net_p99 = p(&net, 0.99);
        fit_points.push((subscribers as f64, p(&net, 0.50)));

        rungs.push(json!({
            "active_subscribers": subscribers,
            "work_items": EXPANSIONS_PER_RUNG,
            "deliveries_in_table_after_rung": deliveries_created,
            "tick_including_fixed_cost": distribution(&samples),
            "expansion_component_baseline_subtracted": distribution(&net),
            "expansion_component_p99_ms": net_p99,
        }));

        // Clear the fan-out so the next rung's table scan cost does not accumulate.
        raw(db, "DELETE FROM event_delivery_sources").await;
        raw(db, "DELETE FROM event_deliveries").await;
        raw(db, "DELETE FROM event_dispatch").await;
        raw(db, "DELETE FROM business_events").await;
    }

    raw(db, "DELETE FROM webhooks").await;

    let (intercept, slope) = linear_fit(&fit_points);
    json!({
        "note": "expansion writes 2 rows per subscriber (an event_delivery_sources reservation and \
                 an event_deliveries upsert) inside one transaction, so its duration is expected to \
                 be affine in the active subscriber count. The fitted slope is the marginal cost of \
                 one more subscriber and is what bounds subscribers_per_workspace_max.",
        "rungs": rungs,
        "fit_p50_expansion_component": {
            "intercept_ms": intercept,
            "slope_ms_per_subscriber": slope,
        },
    })
}

// ---------------------------------------------------------------------------------------------
// Section: same-document backlog drain
// ---------------------------------------------------------------------------------------------

/// `dispatch_head_wait_backoff_ms`'s rule is about the *shape* of drain time, not a latency:
/// "使 N 条积压的排空时间与 N 成线性而非与阶梯和成正比". The implementation realizes the head wait
/// by simply not selecting a non-head row, so the wait a blocked row actually serves is one
/// dispatcher poll. Both arms are measured: `batch = 1` forces exactly one expansion per tick (the
/// pessimal shape, where every non-head row waits a full poll), and `batch = N` is what the
/// deployed worker does.
async fn section_same_document_backlog_drain(
    db: &DatabaseConnection,
    state: &AppState,
    client: &reqwest::Client,
    workspace_id: Uuid,
    owner_id: Uuid,
    receiver: &Receiver,
    violations: &mut Vec<String>,
) -> Value {
    seed_webhook(
        db,
        workspace_id,
        owner_id,
        &receiver.url("/hook"),
        &["flow.content.accepted"],
    )
    .await;

    let mut rows = Vec::with_capacity(BACKLOG_LADDER.len());
    let mut single_points: Vec<(f64, f64)> = Vec::new();

    for backlog in BACKLOG_LADDER {
        for (label, batch) in [
            ("batch_1_one_expansion_per_tick", 1usize),
            ("batch_n_worker_shape", backlog),
        ] {
            let document_id = Uuid::new_v4();
            for seq in 1..=backlog {
                let (_dispatch_id, _) = commit_work(
                    db,
                    workspace_id,
                    "flow.content.accepted",
                    Some(document_id),
                    Some(seq as i64),
                    &json!({ "changed_block_ids": [Uuid::new_v4()] }),
                    10,
                )
                .await;
            }

            let started = Instant::now();
            let mut ticks = 0usize;
            let mut expanded = 0u64;
            // Bounded so a wedged queue fails the run instead of hanging it.
            while expanded < backlog as u64 && ticks < backlog * 4 + 8 {
                park_deliveries(db).await;
                let report = run_tick(state, client, batch).await;
                ticks += 1;
                expanded += report.expanded;
                if report.expanded == 0 {
                    break;
                }
            }
            let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
            park_deliveries(db).await;

            if expanded as usize != backlog {
                violations.push(format!(
                    "backlog {backlog} [{label}]: {expanded} of {backlog} expanded after {ticks} ticks"
                ));
            }

            // FIFO: expansion order must follow accepted_seq, which is what makes the head wait a
            // correctness mechanism rather than a scheduling accident.
            let order = ExpandedOrderRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT accepted_seq FROM event_dispatch WHERE document_id = $1 AND status = 'expanded' \
                 ORDER BY expanded_at, accepted_seq",
                vec![document_id.into()],
            ))
            .all(db)
            .await
            .expect("expansion order query runs")
            .into_iter()
            .map(|row| row.accepted_seq.unwrap_or(-1))
            .collect::<Vec<_>>();
            let fifo = order.windows(2).all(|pair| pair[0] < pair[1]);
            if !fifo {
                violations.push(format!(
                    "backlog {backlog} [{label}]: expansion order was not FIFO: {order:?}"
                ));
            }

            if batch == 1 {
                single_points.push((backlog as f64, ticks as f64));
            }

            rows.push(json!({
                "backlog": backlog,
                "arm": label,
                "batch": batch,
                "ticks_to_drain": ticks,
                "wall_ms": wall_ms,
                "wall_ms_per_item": wall_ms / backlog as f64,
                "fifo_by_accepted_seq": fifo,
            }));

            raw(db, "DELETE FROM event_delivery_sources").await;
            raw(db, "DELETE FROM event_deliveries").await;
            raw(db, "DELETE FROM event_dispatch").await;
            raw(db, "DELETE FROM business_events").await;
        }
    }

    raw(db, "DELETE FROM webhooks").await;

    let (intercept, slope) = linear_fit(&single_points);
    json!({
        "note": "with batch = 1 a non-head row is passed over and reconsidered on the next tick; \
                 ticks_to_drain is therefore the number of head waits N backlogged items cost. A \
                 slope of 1 tick per item is the linear shape the rule demands. The counterfactual \
                 column is what the same backlog would cost if a blocked row instead consumed a \
                 dispatch_backoff_ms rung, which is the shape the rule forbids.",
        "measurements": rows,
        "batch_1_fit": { "intercept_ticks": intercept, "slope_ticks_per_item": slope },
    })
}

#[derive(FromQueryResult)]
struct ExpandedOrderRow {
    accepted_seq: Option<i64>,
}

// ---------------------------------------------------------------------------------------------
// Section: expansion failure ladder
// ---------------------------------------------------------------------------------------------

#[derive(FromQueryResult)]
struct DispatchStateRow {
    attempts: i32,
    max_attempts: i32,
    status: String,
    last_error_code: Option<String>,
    backoff_ms: Option<i64>,
}

async fn dispatch_state(db: &DatabaseConnection, dispatch_id: Uuid) -> DispatchStateRow {
    DispatchStateRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT attempts, max_attempts, status, last_error_code, \
         (EXTRACT(EPOCH FROM (next_attempt_at - now())) * 1000)::bigint AS backoff_ms \
         FROM event_dispatch WHERE id = $1",
        vec![dispatch_id.into()],
    ))
    .one(db)
    .await
    .expect("dispatch state query runs")
    .expect("dispatch row exists")
}

/// Makes expansion genuinely fail *after* the work item has been leased, by having the database
/// reject the `event_deliveries` insert. The ladder and the terminal state are then read from the
/// row the implementation itself wrote.
async fn section_expansion_failure_ladder(
    db: &DatabaseConnection,
    state: &AppState,
    client: &reqwest::Client,
    workspace_id: Uuid,
    owner_id: Uuid,
    receiver: &Receiver,
    violations: &mut Vec<String>,
) -> Value {
    seed_webhook(
        db,
        workspace_id,
        owner_id,
        &receiver.url("/hook"),
        &["flow.thing.happened"],
    )
    .await;
    let configured_max_attempts = 10i32;
    let (dispatch_id, _) = commit_work(
        db,
        workspace_id,
        "flow.thing.happened",
        None,
        None,
        &json!({}),
        configured_max_attempts,
    )
    .await;

    raw(
        db,
        "CREATE OR REPLACE FUNCTION dispatch_budget_fail() RETURNS trigger AS $$ \
         BEGIN RAISE EXCEPTION 'dispatch budget harness: injected expansion failure'; END; \
         $$ LANGUAGE plpgsql",
    )
    .await;
    raw(
        db,
        "CREATE TRIGGER dispatch_budget_fail_trigger BEFORE INSERT ON event_deliveries \
         FOR EACH ROW EXECUTE FUNCTION dispatch_budget_fail()",
    )
    .await;

    let mut ladder = Vec::new();
    let mut terminal = json!(null);
    for _ in 0..(configured_max_attempts + 4) {
        let before = dispatch_state(db, dispatch_id).await;
        if before.status != "pending" {
            terminal = json!({
                "status": before.status,
                "attempts": before.attempts,
                "max_attempts": before.max_attempts,
                "last_error_code": before.last_error_code,
            });
            break;
        }
        let report = run_tick(state, client, 1).await;
        let after = dispatch_state(db, dispatch_id).await;
        ladder.push(json!({
            "attempts_after": after.attempts,
            "status": after.status,
            "last_error_code": after.last_error_code,
            "next_attempt_in_ms": after.backoff_ms,
            "tick_reported_expanded": report.expanded,
        }));
        if after.attempts == before.attempts {
            violations.push("expansion ladder: a tick did not advance attempts; the fault did not fire".to_string());
            break;
        }
        if after.status == "pending" {
            // The row has parked itself on its own backoff; bring it forward so the next rung is
            // measured without waiting out the real ladder. The recorded value above is the one
            // the implementation wrote.
            raw(
                db,
                &format!("UPDATE event_dispatch SET next_attempt_at = now() WHERE id = '{dispatch_id}'"),
            )
            .await;
        } else {
            terminal = json!({
                "status": after.status,
                "attempts": after.attempts,
                "max_attempts": after.max_attempts,
                "last_error_code": after.last_error_code,
            });
            break;
        }
    }

    raw(
        db,
        "DROP TRIGGER IF EXISTS dispatch_budget_fail_trigger ON event_deliveries",
    )
    .await;
    raw(db, "DROP FUNCTION IF EXISTS dispatch_budget_fail()").await;

    if terminal.get("status").and_then(Value::as_str) != Some("failed") {
        violations.push(format!(
            "expansion ladder: the work item did not reach 'failed' after its attempt ceiling: {terminal}"
        ));
    }

    raw(db, "DELETE FROM event_delivery_sources").await;
    raw(db, "DELETE FROM event_deliveries").await;
    raw(db, "DELETE FROM event_dispatch").await;
    raw(db, "DELETE FROM business_events").await;
    raw(db, "DELETE FROM webhooks").await;

    json!({
        "fault": "BEFORE INSERT trigger on event_deliveries raising an exception, so expansion fails \
                  after the work item is leased; no application source is modified",
        "configured_max_attempts": configured_max_attempts,
        "observed_ladder": ladder,
        "terminal": terminal,
        "note": "next_attempt_in_ms is read straight off the row the implementation updated, so the \
                 ladder is measured rather than restated from a constant.",
    })
}

// ---------------------------------------------------------------------------------------------
// Section: lease reclaim accounting
// ---------------------------------------------------------------------------------------------

#[derive(FromQueryResult)]
struct ReclaimRow {
    lease_reclaims: i32,
    status: String,
    last_error_code: Option<String>,
}

/// A worker that vanished leaves a `pending` row with an expired lease. Ticking the dispatcher then
/// charges exactly one reclaim. Repeating that discovers the ceiling empirically, which is the
/// quantity `dispatch_max_lease_reclaims` freezes.
async fn section_lease_reclaim_accounting(
    db: &DatabaseConnection,
    state: &AppState,
    client: &reqwest::Client,
    workspace_id: Uuid,
    violations: &mut Vec<String>,
) -> Value {
    let (dispatch_id, _) = commit_work(db, workspace_id, "flow.thing.happened", None, None, &json!({}), 10).await;
    // Park it out of the expansion claim window: this section measures reclaim, not expansion.
    raw(
        db,
        &format!("UPDATE event_dispatch SET next_attempt_at = now() + interval '1 hour' WHERE id = '{dispatch_id}'"),
    )
    .await;

    let mut steps = Vec::new();
    let mut ceiling: Option<i32> = None;
    for crash in 1..=12 {
        // Exactly what a killed worker leaves behind: a lease it will never release.
        raw(
            db,
            &format!(
                "UPDATE event_dispatch SET lease_token = 'dead-worker-{crash}', \
                 lease_expires_at = now() - interval '1 second' WHERE id = '{dispatch_id}' AND status = 'pending'"
            ),
        )
        .await;
        let report = run_tick(state, client, 1).await;
        let row = ReclaimRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT lease_reclaims, status, last_error_code FROM event_dispatch WHERE id = $1",
            vec![dispatch_id.into()],
        ))
        .one(db)
        .await
        .expect("reclaim query runs")
        .expect("dispatch row exists");
        steps.push(json!({
            "simulated_worker_crash": crash,
            "tick_reported_reclaims": report.dispatch_leases_reclaimed,
            "lease_reclaims": row.lease_reclaims,
            "status": row.status,
            "last_error_code": row.last_error_code,
        }));
        if row.lease_reclaims != crash {
            violations.push(format!(
                "lease reclaim: crash {crash} left lease_reclaims = {}; one vanished worker must \
                 cost exactly one reclaim",
                row.lease_reclaims
            ));
        }
        if row.status != "pending" {
            ceiling = Some(row.lease_reclaims);
            if row.last_error_code.as_deref() != Some("lease_reclaims_exhausted") {
                violations.push(format!(
                    "lease reclaim: terminal row carries last_error_code {:?}, not \
                     'lease_reclaims_exhausted'",
                    row.last_error_code
                ));
            }
            break;
        }
    }
    if ceiling.is_none() {
        violations.push("lease reclaim: no ceiling was reached within 12 simulated crashes".to_string());
    }

    raw(db, "DELETE FROM event_dispatch").await;
    raw(db, "DELETE FROM business_events").await;

    json!({
        "shape": "each iteration writes the exact residue a killed worker leaves (a pending row \
                  with an expired lease token) and then runs one real dispatcher tick",
        "steps": steps,
        "observed_ceiling_reclaims": ceiling,
        "reclaims_charged_per_vanished_worker": 1,
    })
}

// ---------------------------------------------------------------------------------------------
// Section: webhook round trip
// ---------------------------------------------------------------------------------------------

async fn drain_expansions(state: &AppState, client: &reqwest::Client, db: &DatabaseConnection, limit: usize) {
    for _ in 0..limit {
        let report = run_tick(state, client, 64).await;
        if report.expanded == 0 && report.no_subscribers == 0 {
            break;
        }
    }
    let _ = db;
}

async fn section_webhook_round_trip(
    db: &DatabaseConnection,
    state: &AppState,
    client: &reqwest::Client,
    workspace_id: Uuid,
    owner_id: Uuid,
    receiver: &Receiver,
    baseline_ms: f64,
    violations: &mut Vec<String>,
) -> Value {
    // ---- Fast arm.
    seed_webhook(
        db,
        workspace_id,
        owner_id,
        &receiver.url("/hook"),
        &["flow.thing.happened"],
    )
    .await;
    let attempts = 40usize;
    for _ in 0..attempts {
        commit_work(db, workspace_id, "flow.thing.happened", None, None, &json!({}), 10).await;
    }
    defer_new_deliveries(db).await;
    drain_expansions(state, client, db, 8).await;
    release_deferred_deliveries(db).await;
    let _ = receiver.take();

    let mut send_samples = Vec::with_capacity(attempts);
    let mut delivered_total = 0u64;
    for _ in 0..attempts {
        let started = Instant::now();
        let report = run_tick(state, client, 1).await;
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        if report.delivered == 1 {
            send_samples.push(elapsed);
            delivered_total += 1;
        }
    }
    let received = receiver.take();
    let unique_delivery_ids: std::collections::HashSet<String> =
        received.iter().filter_map(|item| item.delivery_id.clone()).collect();
    if received.len() != unique_delivery_ids.len() {
        violations.push(format!(
            "webhook round trip: {} requests carried only {} distinct delivery ids",
            received.len(),
            unique_delivery_ids.len()
        ));
    }
    if delivered_total as usize != attempts {
        violations.push(format!(
            "webhook round trip: {delivered_total} of {attempts} deliveries reached 'dispatched'"
        ));
    }
    let net: Vec<f64> = send_samples
        .iter()
        .map(|value| (value - baseline_ms).max(0.0))
        .collect();
    let body_bytes: Vec<f64> = received.iter().map(|item| item.body_bytes as f64).collect();

    raw(db, "DELETE FROM event_delivery_sources").await;
    raw(db, "DELETE FROM event_deliveries").await;
    raw(db, "DELETE FROM event_dispatch").await;
    raw(db, "DELETE FROM business_events").await;
    raw(db, "DELETE FROM webhooks").await;

    // ---- Timeout arm: a subscriber that never answers within the client's budget.
    let timeout_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .connect_timeout(Duration::from_secs(2))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("timeout client builds");
    seed_webhook(
        db,
        workspace_id,
        owner_id,
        &receiver.url("/slow"),
        &["flow.thing.happened"],
    )
    .await;
    let (slow_dispatch, _) = commit_work(db, workspace_id, "flow.thing.happened", None, None, &json!({}), 10).await;
    let _ = slow_dispatch;
    defer_new_deliveries(db).await;
    drain_expansions(state, &timeout_client, db, 4).await;
    release_deferred_deliveries(db).await;
    let started = Instant::now();
    let report = run_tick(state, &timeout_client, 1).await;
    let timeout_wall_ms = started.elapsed().as_secs_f64() * 1000.0;
    let timeout_row = DeliveryStateRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT status, attempts, last_error_code, \
         (EXTRACT(EPOCH FROM (next_attempt_at - now())) * 1000)::bigint AS backoff_ms \
         FROM event_deliveries ORDER BY created_at DESC LIMIT 1",
        vec![],
    ))
    .one(db)
    .await
    .expect("delivery state query runs");
    if report.delivered != 0 {
        violations.push("webhook timeout arm: a subscriber that never answered was counted as delivered".to_string());
    }
    let timeout_observed = timeout_row.as_ref().map(|row| {
        json!({
            "status": row.status,
            "attempts": row.attempts,
            "last_error_code": row.last_error_code,
            "next_attempt_in_ms": row.backoff_ms,
        })
    });
    if timeout_row.as_ref().and_then(|row| row.last_error_code.as_deref()) != Some("request_failed") {
        violations.push(format!(
            "webhook timeout arm: expected last_error_code 'request_failed', got {timeout_observed:?}"
        ));
    }
    let _ = receiver.take();

    raw(db, "DELETE FROM event_delivery_sources").await;
    raw(db, "DELETE FROM event_deliveries").await;
    raw(db, "DELETE FROM event_dispatch").await;
    raw(db, "DELETE FROM business_events").await;
    raw(db, "DELETE FROM webhooks").await;

    json!({
        "fast_subscriber": {
            "shape": "a real HTTP/1.1 endpoint in this process answering 200 immediately; the tick \
                      timed here performs exactly one send",
            "attempts": attempts,
            "tick_including_fixed_cost": distribution(&send_samples),
            "send_component_baseline_subtracted": distribution(&net),
            "plain_delivery_body_bytes": distribution(&body_bytes),
            "requests_observed_at_subscriber": received.len(),
            "distinct_delivery_ids": unique_delivery_ids.len(),
        },
        "unresponsive_subscriber": {
            "shape": "the same endpoint on a path that never answers within the budget; the client \
                      is built with a 3 s request timeout so the abort is observable inside one run",
            "client_request_timeout_ms": 3_000,
            "observed_tick_wall_ms": timeout_wall_ms,
            "delivery_row_after": timeout_observed,
            "note": "the wall clock of the aborting tick is the real cost a single unresponsive \
                     subscriber imposes on one dispatcher pass; delivery_lease_ttl_ms must exceed it.",
        },
    })
}

#[derive(FromQueryResult)]
struct DeliveryStateRow {
    status: String,
    attempts: i32,
    last_error_code: Option<String>,
    backoff_ms: Option<i64>,
}

// ---------------------------------------------------------------------------------------------
// Section: coalescing window
// ---------------------------------------------------------------------------------------------

#[derive(FromQueryResult)]
struct SourceCountRow {
    delivery_id: Uuid,
    sources: i64,
    first_seq: Option<i64>,
    latest_seq: Option<i64>,
    status: String,
}

/// `coalesced_source_events_max`'s rule is "取 debounce 窗口内单文档合并事件数的 p99 上界". The two
/// frozen inputs are `content_delivery_debounce_ms = 2000` and the 10 updates/s per-connection cap.
/// This section produces content work at exactly that cap on one document while a dispatcher ticks
/// continuously, and counts how many source events each delivery row actually ends up covering.
async fn section_coalescing_window(
    db: &DatabaseConnection,
    state: &AppState,
    client: &reqwest::Client,
    workspace_id: Uuid,
    owner_id: Uuid,
    receiver: &Receiver,
    violations: &mut Vec<String>,
) -> Value {
    seed_webhook(
        db,
        workspace_id,
        owner_id,
        &receiver.url("/hook"),
        &["flow.content.accepted"],
    )
    .await;
    let document_id = Uuid::new_v4();
    let total_updates = UPDATES_PER_SECOND_CAP * COALESCING_WINDOWS * (CONTENT_DEBOUNCE_MS as usize) / 1000;
    let interval = Duration::from_millis(1000 / UPDATES_PER_SECOND_CAP as u64);

    let started = Instant::now();
    for seq in 1..=total_updates {
        let deadline = started + interval * (seq as u32);
        commit_work(
            db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(seq as i64),
            &json!({ "changed_block_ids": [Uuid::new_v4()] }),
            10,
        )
        .await;
        // One dispatcher pass per produced update, which is far more aggressive than the deployed
        // 5 s poll: coalescing that survives this survives any slower cadence.
        let _ = run_tick(state, client, 64).await;
        let now = Instant::now();
        if deadline > now {
            tokio::time::sleep(deadline - now).await;
        }
    }
    // Let the debounce elapse and flush whatever is still pending.
    tokio::time::sleep(Duration::from_millis(CONTENT_DEBOUNCE_MS + 500)).await;
    for _ in 0..20 {
        let report = run_tick(state, client, 64).await;
        if report.expanded == 0 && report.delivered == 0 {
            break;
        }
    }

    let rows = SourceCountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT d.id AS delivery_id, d.first_seq, d.latest_seq, d.status, \
         (SELECT count(*) FROM event_delivery_sources s WHERE s.delivery_id = d.id) AS sources \
         FROM event_deliveries d WHERE d.document_id = $1 ORDER BY d.created_at",
        vec![document_id.into()],
    ))
    .all(db)
    .await
    .expect("coalescing query runs");

    let counts: Vec<f64> = rows.iter().map(|row| row.sources as f64).collect();
    let total_sources: i64 = rows.iter().map(|row| row.sources).sum();
    let orphan_sources = count(
        db,
        "SELECT count(*) AS n FROM event_delivery_sources WHERE delivery_id IS NULL",
        vec![],
    )
    .await;
    if total_sources + orphan_sources < total_updates as i64 {
        violations.push(format!(
            "coalescing: {total_updates} source events produced but only {total_sources} bound plus \
             {orphan_sources} unbound were found; a source entry was dropped"
        ));
    }
    // Ranges must tile the seq space without gaps or overlap.
    let mut ranges: Vec<(i64, i64)> = rows
        .iter()
        .filter_map(|row| row.first_seq.zip(row.latest_seq))
        .collect();
    ranges.sort_unstable();
    let contiguous = ranges.windows(2).all(|pair| pair[0].1 < pair[1].0)
        && ranges.first().is_some_and(|first| first.0 == 1)
        && ranges.last().is_some_and(|last| last.1 == total_updates as i64);
    if !contiguous {
        violations.push(format!(
            "coalescing: delivery ranges do not tile 1..={total_updates}: {ranges:?}"
        ));
    }

    let per_row: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "delivery_id": row.delivery_id,
                "sources": row.sources,
                "first_seq": row.first_seq,
                "latest_seq": row.latest_seq,
                "status": row.status,
            })
        })
        .collect();

    // When each merged row actually reached the subscriber, relative to the first produced event.
    // This is the delivery latency the coalescing cap imposes at a sustained write rate, and it is
    // the second thing coalesced_source_events_max buys or spends.
    let arrivals: Vec<f64> = receiver
        .take()
        .iter()
        .map(|item| item.at.saturating_duration_since(started).as_secs_f64() * 1000.0)
        .collect();

    raw(db, "DELETE FROM event_delivery_sources").await;
    raw(db, "DELETE FROM event_deliveries").await;
    raw(db, "DELETE FROM event_dispatch").await;
    raw(db, "DELETE FROM business_events").await;
    raw(db, "DELETE FROM webhooks").await;

    json!({
        "delivery_arrival_ms_since_first_event": arrivals,
        "inputs": {
            "updates_per_second_cap": UPDATES_PER_SECOND_CAP,
            "content_delivery_debounce_ms": CONTENT_DEBOUNCE_MS,
            "debounce_windows_driven": COALESCING_WINDOWS,
            "total_content_events": total_updates,
            "dispatcher_cadence": "one full tick per produced update (far tighter than the deployed 5 s poll)",
        },
        "sources_per_delivery": {
            "rows": rows.len(),
            "distribution": distribution(&counts),
            "max": counts.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            "theoretical_ceiling_from_frozen_inputs": UPDATES_PER_SECOND_CAP * (CONTENT_DEBOUNCE_MS as usize) / 1000,
        },
        "ranges_tile_seq_space": contiguous,
        "total_sources_bound": total_sources,
        "per_delivery": per_row,
    })
}

// ---------------------------------------------------------------------------------------------
// Section: what closes a coalesced row — the cap or the debounce
// ---------------------------------------------------------------------------------------------

/// The sustained-rate arm above showed rows closing at the coalescing cap, not at the 2,000 ms
/// debounce. That is only meaningful next to the opposite case, so this arm produces content
/// **slower** than one event per debounce period and shows the debounce closing each row at one
/// source. Together the two arms establish which of the two budgets is actually load-bearing at
/// which rate, which is what `coalesced_source_events_max` has to be chosen against.
async fn section_debounce_versus_cap_closure(
    db: &DatabaseConnection,
    state: &AppState,
    client: &reqwest::Client,
    workspace_id: Uuid,
    owner_id: Uuid,
    receiver: &Receiver,
    violations: &mut Vec<String>,
) -> Value {
    seed_webhook(
        db,
        workspace_id,
        owner_id,
        &receiver.url("/hook"),
        &["flow.content.accepted"],
    )
    .await;
    let document_id = Uuid::new_v4();
    // One event per 1.5 debounce periods: strictly slower than the debounce can be refreshed.
    let interval = Duration::from_millis(CONTENT_DEBOUNCE_MS * 3 / 2);
    let events = 4usize;
    let _ = receiver.take();

    for seq in 1..=events {
        commit_work(
            db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(seq as i64),
            &json!({ "changed_block_ids": [Uuid::new_v4()] }),
            10,
        )
        .await;
        let until = Instant::now() + interval;
        while Instant::now() < until {
            let _ = run_tick(state, client, 16).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    for _ in 0..20 {
        let report = run_tick(state, client, 16).await;
        if report.expanded == 0 && report.delivered == 0 {
            break;
        }
    }

    let rows = SourceCountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT d.id AS delivery_id, d.first_seq, d.latest_seq, d.status, \
         (SELECT count(*) FROM event_delivery_sources s WHERE s.delivery_id = d.id) AS sources \
         FROM event_deliveries d WHERE d.document_id = $1 ORDER BY d.created_at",
        vec![document_id.into()],
    ))
    .all(db)
    .await
    .expect("slow-rate coalescing query runs");
    let counts: Vec<i64> = rows.iter().map(|row| row.sources).collect();
    if rows.len() != events {
        violations.push(format!(
            "debounce closure: {events} events produced at one per {} ms yielded {} delivery rows \
             carrying {counts:?}; below the debounce refresh rate each event must get its own row",
            interval.as_millis(),
            rows.len()
        ));
    }

    raw(db, "DELETE FROM event_delivery_sources").await;
    raw(db, "DELETE FROM event_deliveries").await;
    raw(db, "DELETE FROM event_dispatch").await;
    raw(db, "DELETE FROM business_events").await;
    raw(db, "DELETE FROM webhooks").await;
    let _ = receiver.take();

    json!({
        "shape": "content events produced one per 1.5 debounce periods, slower than the debounce can \
                  be refreshed",
        "production_interval_ms": interval.as_millis() as u64,
        "content_delivery_debounce_ms": CONTENT_DEBOUNCE_MS,
        "events": events,
        "delivery_rows": rows.len(),
        "sources_per_row": counts,
        "closed_by": if rows.len() == events { "debounce" } else { "inconclusive" },
        "note": "bind_content_delivery sets next_attempt_at = now() + debounce on every merge, so the \
                 debounce is trailing rather than a fixed window: above roughly one event per \
                 debounce period a row can only be closed by coalesced_source_events_max, and below \
                 it the debounce closes each row on its own.",
    })
}

// ---------------------------------------------------------------------------------------------
// Section: delivery body size
// ---------------------------------------------------------------------------------------------

/// `changed_block_ids_per_delivery_max` bounds the union of block ids in a **coalesced** body. The
/// body also carries one `source_event_ids` entry per merged event, so the size model needs both
/// slopes. Both are measured from the bytes the subscriber actually received.
/// Builds one delivery merged from `source_events` content events, each carrying
/// `block_ids_each` changed block ids, sends it, and returns the bytes the subscriber received.
///
/// This is the shape `changed_block_ids_per_delivery_max`'s rule is actually about ("投递体在 p99
/// 合并窗口下"): the two-source rungs above isolate the per-block-id cost, this measures the whole
/// worst-case body, `source_event_ids` array included.
async fn merged_body_probe(
    db: &DatabaseConnection,
    state: &AppState,
    client: &reqwest::Client,
    workspace_id: Uuid,
    owner_id: Uuid,
    receiver: &Receiver,
    source_events: usize,
    block_ids_each: usize,
) -> Value {
    seed_webhook(
        db,
        workspace_id,
        owner_id,
        &receiver.url("/hook"),
        &["flow.content.accepted"],
    )
    .await;
    let document_id = Uuid::new_v4();
    for seq in 1..=source_events {
        let ids: Vec<String> = (0..block_ids_each).map(|_| Uuid::new_v4().to_string()).collect();
        commit_work(
            db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(seq as i64),
            &json!({ "changed_block_ids": ids }),
            10,
        )
        .await;
    }
    for _ in 0..(source_events + 4) {
        let report = run_tick(state, client, 64).await;
        if report.expanded == 0 {
            break;
        }
    }
    raw(
        db,
        "UPDATE event_deliveries SET next_attempt_at = now() WHERE status IN ('pending', 'sealed')",
    )
    .await;
    let _ = receiver.take();
    for _ in 0..(source_events + 4) {
        let report = run_tick(state, client, 64).await;
        if report.delivered == 0 {
            break;
        }
    }
    let received = receiver.take();
    let biggest = received.iter().max_by_key(|item| item.body_bytes);
    let parsed: Option<Value> = biggest.and_then(|item| serde_json::from_str(&item.body).ok());
    let result = json!({
        "source_events_produced": source_events,
        "block_ids_each": block_ids_each,
        "block_ids_produced_total": source_events * block_ids_each,
        "deliveries_sent": received.len(),
        "largest_body_bytes": biggest.map(|item| item.body_bytes),
        "source_event_ids_in_body": parsed
            .as_ref()
            .and_then(|body| body.pointer("/delivery/source_event_ids"))
            .and_then(Value::as_array)
            .map(Vec::len),
        "block_ids_in_body": parsed
            .as_ref()
            .and_then(|body| body.pointer("/event/payload/changed_block_ids"))
            .and_then(Value::as_array)
            .map(Vec::len),
        "block_ids_truncated": parsed
            .as_ref()
            .and_then(|body| body.pointer("/delivery/block_ids_truncated"))
            .and_then(Value::as_bool),
    });

    raw(db, "DELETE FROM event_delivery_sources").await;
    raw(db, "DELETE FROM event_deliveries").await;
    raw(db, "DELETE FROM event_dispatch").await;
    raw(db, "DELETE FROM business_events").await;
    raw(db, "DELETE FROM webhooks").await;
    result
}

async fn section_delivery_body_size(
    db: &DatabaseConnection,
    state: &AppState,
    client: &reqwest::Client,
    workspace_id: Uuid,
    owner_id: Uuid,
    receiver: &Receiver,
    violations: &mut Vec<String>,
) -> Value {
    let mut rows = Vec::with_capacity(BLOCK_ID_LADDER.len());
    let mut block_points: Vec<(f64, f64)> = Vec::new();

    for block_ids in BLOCK_ID_LADDER {
        seed_webhook(
            db,
            workspace_id,
            owner_id,
            &receiver.url("/hook"),
            &["flow.content.accepted"],
        )
        .await;
        let document_id = Uuid::new_v4();
        // Two source events so the coalesced branch is taken, splitting the block ids between them.
        let half = block_ids.div_ceil(2);
        let first: Vec<String> = (0..half).map(|_| Uuid::new_v4().to_string()).collect();
        let second: Vec<String> = (0..block_ids - half).map(|_| Uuid::new_v4().to_string()).collect();
        commit_work(
            db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(1),
            &json!({ "changed_block_ids": first }),
            10,
        )
        .await;
        commit_work(
            db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(2),
            &json!({ "changed_block_ids": second }),
            10,
        )
        .await;
        for _ in 0..4 {
            let report = run_tick(state, client, 16).await;
            if report.expanded == 0 {
                break;
            }
        }
        // Release the debounce so the merged row is sendable now.
        raw(
            db,
            "UPDATE event_deliveries SET next_attempt_at = now() WHERE status IN ('pending', 'sealed')",
        )
        .await;
        let _ = receiver.take();
        for _ in 0..4 {
            let report = run_tick(state, client, 16).await;
            if report.delivered == 0 {
                break;
            }
        }
        let received = receiver.take();
        let observed = received.first();
        let parsed: Option<Value> = observed.and_then(|item| serde_json::from_str(&item.body).ok());
        let coalesced = parsed
            .as_ref()
            .and_then(|body| body.pointer("/delivery/coalesced"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let carried = parsed
            .as_ref()
            .and_then(|body| body.pointer("/event/payload/changed_block_ids"))
            .and_then(Value::as_array)
            .map(Vec::len);
        let truncated = parsed
            .as_ref()
            .and_then(|body| body.pointer("/delivery/block_ids_truncated"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let bytes = observed.map(|item| item.body_bytes);

        if !coalesced {
            violations.push(format!(
                "body size rung {block_ids}: the delivery was not coalesced, so the block-id cap \
                 path was never exercised"
            ));
        }
        if let (Some(bytes), Some(carried)) = (bytes, carried) {
            block_points.push((carried as f64, bytes as f64));
        }

        rows.push(json!({
            "block_ids_produced": block_ids,
            "block_ids_carried_in_body": carried,
            "block_ids_truncated": truncated,
            "coalesced": coalesced,
            "body_bytes": bytes,
            "source_events": 2,
        }));

        raw(db, "DELETE FROM event_delivery_sources").await;
        raw(db, "DELETE FROM event_deliveries").await;
        raw(db, "DELETE FROM event_dispatch").await;
        raw(db, "DELETE FROM business_events").await;
        raw(db, "DELETE FROM webhooks").await;
    }

    // The worst realistic body: a row merged all the way to the coalescing cap. Two variants, one
    // just under the block-id cap and one over it, so both the size and the truncation fallback are
    // measured rather than reasoned about.
    let merged_under_cap = merged_body_probe(db, state, client, workspace_id, owner_id, receiver, 64, 3).await;
    let merged_over_cap = merged_body_probe(db, state, client, workspace_id, owner_id, receiver, 64, 8).await;

    let (intercept, slope) = linear_fit(&block_points);
    json!({
        "note": "every value is the byte count the subscriber received, not the sender's estimate. \
                 The fit converts a candidate changed_block_ids_per_delivery_max into a body size.",
        "rungs": rows,
        "fit_body_bytes_vs_block_ids": {
            "intercept_bytes": intercept,
            "bytes_per_block_id": slope,
        },
        "fully_merged_worst_case": {
            "under_block_id_cap": merged_under_cap,
            "over_block_id_cap": merged_over_cap,
        },
    })
}

// ---------------------------------------------------------------------------------------------
// Section: expanded-row footprint
// ---------------------------------------------------------------------------------------------

#[derive(FromQueryResult)]
struct SizeRow {
    total_bytes: i64,
    rows: i64,
}

#[derive(FromQueryResult)]
struct HeadProbeRow {
    id: Option<Uuid>,
}

/// `dispatch_expanded_retention_days`' rule has two halves: keep enough history for an
/// investigation, and "使队首查询所在表规模有界". Only the second half is measurable, and it is
/// measured here: how much a retained `expanded` row costs on disk, and what the head-of-queue
/// selection predicate costs once the table holds that many of them.
async fn section_expanded_row_footprint(db: &DatabaseConnection, workspace_id: Uuid) -> Value {
    let mut rungs = Vec::with_capacity(FOOTPRINT_LADDER.len());
    let mut present = 0usize;

    // One live pending head-of-queue row that the probe must find among the retained rows.
    let live_document = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO business_events (id, workspace_id, event_type, aggregate_type, aggregate_id) \
         VALUES ($1, $2, 'flow.content.accepted', 'flow_object', $3)",
        vec![event_id.into(), workspace_id.into(), Uuid::new_v4().to_string().into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO event_dispatch (id, event_id, workspace_id, event_type, document_id, accepted_seq, max_attempts) \
         VALUES ($1, $2, $3, 'flow.content.accepted', $4, 1, 10)",
        vec![
            Uuid::new_v4().into(),
            event_id.into(),
            workspace_id.into(),
            live_document.into(),
        ],
    )
    .await;

    for target in FOOTPRINT_LADDER {
        let to_add = target - present;
        // Bulk generation in the database: the point is the resulting table, not the insert path.
        raw(
            db,
            &format!(
                "WITH minted AS ( \
                   INSERT INTO business_events (id, workspace_id, event_type, aggregate_type, aggregate_id) \
                   SELECT gen_random_uuid(), '{workspace_id}', 'flow.content.accepted', 'flow_object', \
                          gen_random_uuid()::text \
                   FROM generate_series(1, {to_add}) \
                   RETURNING id \
                 ) \
                 INSERT INTO event_dispatch (id, event_id, workspace_id, event_type, document_id, accepted_seq, \
                                             max_attempts, status, expanded_at) \
                 SELECT gen_random_uuid(), minted.id, '{workspace_id}', 'flow.content.accepted', \
                        gen_random_uuid(), 1, 10, 'expanded', now() \
                 FROM minted"
            ),
        )
        .await;
        present = target;
        raw(db, "ANALYZE event_dispatch").await;

        let size = SizeRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_total_relation_size('event_dispatch')::bigint AS total_bytes, \
             (SELECT count(*) FROM event_dispatch)::bigint AS rows",
            vec![],
        ))
        .one(db)
        .await
        .expect("size query runs")
        .expect("size query returns a row");

        // The exact head-of-queue predicate `expand_one` uses to pick its next work item.
        let mut probes = Vec::with_capacity(30);
        let mut probe_found = 0usize;
        for _ in 0..30 {
            let started = Instant::now();
            let found = HeadProbeRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT d.id FROM event_dispatch d \
                 WHERE d.status = 'pending' AND d.next_attempt_at <= now() \
                   AND (d.lease_token IS NULL OR d.lease_expires_at < now()) \
                   AND (d.document_id IS NULL OR NOT EXISTS ( \
                        SELECT 1 FROM event_dispatch o WHERE o.document_id = d.document_id \
                          AND o.status = 'pending' AND o.accepted_seq IS NOT NULL \
                          AND o.accepted_seq < d.accepted_seq)) \
                 ORDER BY d.next_attempt_at, d.id LIMIT 1",
                vec![],
            ))
            .one(db)
            .await
            .expect("head probe runs");
            probes.push(started.elapsed().as_secs_f64() * 1000.0);
            if found.and_then(|row| row.id).is_some() {
                probe_found += 1;
            }
        }

        rungs.push(json!({
            "retained_expanded_rows": size.rows,
            "event_dispatch_total_relation_bytes": size.total_bytes,
            "bytes_per_row": size.total_bytes as f64 / size.rows.max(1) as f64,
            "head_of_queue_probe": distribution(&probes),
            "probe_found_the_live_pending_head": probe_found,
        }));
    }

    raw(db, "DELETE FROM event_dispatch").await;
    raw(db, "DELETE FROM business_events").await;
    raw(db, "VACUUM FULL event_dispatch").await;

    json!({
        "note": "the probe is expand_one's own selection predicate, run against a table holding the \
                 stated number of retained 'expanded' rows plus one live pending head. Converting \
                 this into a retention period needs a deployment's own event rate, which this \
                 harness cannot measure.",
        "rungs": rungs,
    })
}

// ---------------------------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn delivery_path_numeric_budget_evidence() {
    let Some(scratch) = scratch("evidence").await else {
        eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
        return;
    };
    let started = Instant::now();
    let receiver = Receiver::start().await;
    install_config(receiver.port).expect("configuration installs before anything reads it");
    assert_eq!(receiver.len(), 0, "the receiver starts empty");

    let mut violations: Vec<String> = Vec::new();
    let db = scratch.db.clone();
    let state = state_for(db.clone());
    let client = fast_client();
    let (workspace_id, owner_id) = seed_workspace(&db).await;

    let (baseline, baseline_ms) = section_baseline(&state, &client).await;
    let expansion = section_expansion_by_subscriber_count(
        &db,
        &state,
        &client,
        workspace_id,
        owner_id,
        &receiver,
        baseline_ms,
        &mut violations,
    )
    .await;
    let drain =
        section_same_document_backlog_drain(&db, &state, &client, workspace_id, owner_id, &receiver, &mut violations)
            .await;
    let ladder =
        section_expansion_failure_ladder(&db, &state, &client, workspace_id, owner_id, &receiver, &mut violations)
            .await;
    let reclaim = section_lease_reclaim_accounting(&db, &state, &client, workspace_id, &mut violations).await;
    let round_trip = section_webhook_round_trip(
        &db,
        &state,
        &client,
        workspace_id,
        owner_id,
        &receiver,
        baseline_ms,
        &mut violations,
    )
    .await;
    let coalescing =
        section_coalescing_window(&db, &state, &client, workspace_id, owner_id, &receiver, &mut violations).await;
    let closure =
        section_debounce_versus_cap_closure(&db, &state, &client, workspace_id, owner_id, &receiver, &mut violations)
            .await;
    let body_size =
        section_delivery_body_size(&db, &state, &client, workspace_id, owner_id, &receiver, &mut violations).await;
    let footprint = section_expanded_row_footprint(&db, workspace_id).await;

    let artifact = json!({
        "schema_version": "sylvode.flow.dispatch-budget-evidence.v1",
        "source_head": source_head().0,
        "source_tree_dirty": source_head().1,
        "environment": {
            "build_profile": build_profile(),
            "postgres_version": postgres_version(&db).await,
            "scratch_database": scratch.name,
            "subscriber_endpoint": "in-process HTTP/1.1 server on 127.0.0.1",
            "wall_seconds": started.elapsed().as_secs_f64(),
        },
        "baseline": baseline,
        "expansion_by_subscriber_count": expansion,
        "same_document_backlog_drain": drain,
        "expansion_failure_ladder": ladder,
        "lease_reclaim_accounting": reclaim,
        "webhook_round_trip": round_trip,
        "coalescing_window": coalescing,
        "debounce_versus_cap_closure": closure,
        "delivery_body_size": body_size,
        "expanded_row_footprint": footprint,
        "budgets_this_harness_cannot_measure": {
            "dispatch_failed_retention_days": "the rule is '不短于一个值班轮换周期' — an on-call \
                rotation length is an organisational fact, not a property of the running system",
            "delivery_source_retention_days": "set_by is the v0.8 implementer and the rule binds it \
                strictly above replay_max_window_days, which does not exist in v0.4",
            "replay_max_window_days": "admin replay is a v0.8 feature; there is nothing to measure",
        },
        "violations": violations,
        "passed": violations.is_empty(),
    });

    let rendered = serde_json::to_string_pretty(&artifact).expect("artifact serializes");
    println!("{rendered}");
    if let Ok(path) = std::env::var(OUT_ENV) {
        let temp = format!("{path}.tmp");
        std::fs::write(&temp, format!("{rendered}\n")).expect("artifact temp file writes");
        std::fs::rename(&temp, &path).expect("artifact renames into place");
    }

    scratch.drop_self().await;
    assert!(
        violations.is_empty(),
        "dispatch budget harness recorded violations: {violations:#?}"
    );
}

fn source_head() -> (String, bool) {
    let head = std::process::Command::new("git")
        .args(["-C", env!("CARGO_MANIFEST_DIR"), "rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map_or_else(
            || "unknown".to_string(),
            |out| String::from_utf8_lossy(&out.stdout).trim().to_string(),
        );
    let dirty = std::process::Command::new("git")
        .args(["-C", env!("CARGO_MANIFEST_DIR"), "status", "--porcelain"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .is_some_and(|out| !out.stdout.is_empty());
    (head, dirty)
}

const fn build_profile() -> &'static str {
    if cfg!(debug_assertions) { "debug" } else { "release" }
}

#[derive(FromQueryResult)]
struct VersionRow {
    version: String,
}

async fn postgres_version(db: &DatabaseConnection) -> String {
    VersionRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT version() AS version",
        vec![],
    ))
    .one(db)
    .await
    .ok()
    .flatten()
    .map_or_else(|| "unknown".to_string(), |row| row.version)
}
