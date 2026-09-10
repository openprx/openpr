//! Sylvode Flow **v0.5 (WP-16) multi-user session round-trip harness** — the re-verification
//! `contracts/limits-v1.md` requires of the frozen `collab_accepted_round_trip_ms_p95_max`
//! threshold once the v0.5 session surface exists.
//!
//! `limits-v1.md` (the paragraph under the lock-budget table) states it directly:
//!
//! > "10-client fixture、cache exact/eviction、head-race、timeout rollback 与 snapshot soft/hard
//! > trigger 写入 `evidence/v0.4/collab-architecture-result.json`；**v0.5 用多人/离线 workload 重验
//! > 250/25/100 ms 门槛**。"
//!
//! v0.4's `flow_collab_load_harness.rs` measured the 250 ms round trip under a write-only
//! 10-client load. WP-16 added four things that all sit on the *same* per-session egress path
//! that budget is measured over, and any of them could regress it:
//!
//! 1. `presence` fan-out — every refresh is a `SessionRegistry::broadcast` to every other session
//!    on the document, interleaved with the `update`/`accepted` pairs.
//! 2. The presence **join snapshot** — one direct socket write per live peer, per connect.
//! 3. `ack` — inbound frames that now do real work (range check against the egress sequencer plus
//!    a registry write) instead of being discarded.
//! 4. **reconnect resume** — `open{known_seq, known_frontier}` replaying the missed `accepted`
//!    stream out of `collab_updates`, i.e. the "离线" half of the contract's sentence.
//!
//! So this harness runs the same 10 concurrent clients, each a distinct workspace member, and
//! measures the same thing v0.4 did — each client's own `update` frame leaving until its own
//! `accepted` comes back, which is exactly the span `limits-v1.md` defines ("包含 prepare、DB
//! wait/hold 和 egress，不含人为 WAN delay") — **while all four of those are live**.
//!
//! # Why this is a separate test binary and not a `#[test]` in `apps/api/src/flow/collab/session.rs`
//!
//! Two reasons, both measured rather than assumed.
//!
//! - **Isolation.** One `tests/*.rs` file is its own binary with one `#[tokio::test]` in it, so
//!   nothing else in the workspace can be running against the same database while it measures.
//!   The identical workload, run as an in-crate `#[test]` interleaved with this crate's ~640
//!   other tests, measured p95 419 ms; alone it measured 106-115 ms. That difference is the test
//!   harness's own contention, not the write path.
//! - **A dedicated `PostgreSQL` instance.** Sharing an instance with other test databases costs
//!   WAL fsync contention that lands directly in this measurement. [`load_environment_problem`]
//!   below refuses to run at all unless the caller has declared a dedicated container, and
//!   rejects the known-shared `flow-test-pg` by name — the same gate
//!   `flow_collab_load_harness.rs` carries, for the same reason.
//!
//! # Honest failure
//!
//! Every check appends to [`Report::violations`] rather than panicking at the first problem, and
//! the full measured distribution is printed before the single final assertion. The threshold is
//! `limits-v1.md`'s frozen 250 ms and is never relaxed to make a run green; a run that cannot
//! satisfy the environment gate emits `passed: false` and says the measurement never happened,
//! so a skip can never be read as a pass.

#![allow(
    // Test target: `clippy.toml` already allows unwrap/expect/panic in tests; `indexing_slicing`
    // and the print lints are not covered by that config and are needed to report a measured
    // distribution at all.
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::print_stderr,
    // Percentiles over sample counts and millisecond arithmetic.
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::middleware as axum_middleware;
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::Utc;
use collab_core::{CollabEngine, LoroCollabEngine};
use futures_util::{SinkExt, StreamExt};
use platform::{
    app::AppState,
    auth::JwtManager,
    config::{AppConfig, Secret},
};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use tokio::sync::Barrier;
use tokio_tungstenite::tungstenite::Message as TMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use uuid::Uuid;

use api::flow::collab::frame::{Frame, PROTOCOL_VERSION};
use api::flow::event_origin::{CommandOrigin, EventSurface};
use api::middleware::bot_auth::bot_or_user_auth_middleware;
use api::routes::collab::{create_ticket, ws_upgrade};

// ---------------------------------------------------------------------------------------------
// Frozen targets. Every number here is transcribed from the contract repository; none is invented
// here and none may be relaxed to make a run green.
// ---------------------------------------------------------------------------------------------

/// `limits-v1.md`'s `collab_accepted_round_trip_ms_p95_max` is defined over "10 个同时 client";
/// `ADR-0010` §"量化接受与推翻门槛" item 2 states the same fixture size. Identical to
/// `flow_collab_load_harness.rs`'s constant of the same name — this harness re-verifies that
/// fixture under the v0.5 session workload, it does not redefine it.
const CONCURRENT_CLIENTS: usize = 10;

/// `limits-v1.md`: `collab_accepted_round_trip_ms_p95_max` = **250 ms**, "包含 prepare、DB
/// wait/hold 和 egress，不含人为 WAN delay".
const ROUND_TRIP_P95_MS_MAX: f64 = 250.0;

/// `ADR-0010`'s official-run protocol: "先 5 次 warmup、至少 30 次测量"; "`samples<30` … 使
/// `passed=false`". Same three constants `flow_collab_load_harness.rs` uses.
const WARMUP_ROUNDS: usize = 5;
const MEASURED_ROUNDS: usize = 10;
const MIN_SAMPLES: usize = 30;

/// How many of the ten clients disconnect and reconnect through the **resume** path
/// (`open{known_seq, known_frontier}`) between rounds, so the "离线" half of the contract's
/// sentence is genuinely exercised during the measured window rather than only before it.
const RESUMING_CLIENTS: usize = 3;

/// Per-client pacing. `limits-v1.md`'s `updates_per_connection_per_second` is 10 sustained /
/// 20 burst and `frames_per_connection_per_second` is 30 / 60; each client here sends at most one
/// `presence`, one `update` and one `ack` per round, so this pause keeps every client inside both
/// sustained rates. Without it the harness would be measuring the token bucket, not the write
/// path.
const ROUND_PACING_MS: u64 = 120;

const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";
/// The same variable name `flow_collab_load_harness.rs` uses: a measurement is admissible only
/// when the caller has explicitly declared the `PostgreSQL` instance dedicated. Never inferred
/// from `OPENPR_TEST_DATABASE_URL` being present.
const DEDICATED_PG_CONTAINER_ENV: &str = "OPENPR_FLOW_DEDICATED_PG_CONTAINER";
const QUIET_PG_QUALIFIED_ENV: &str = "OPENPR_FLOW_QUIET_PG_QUALIFIED";
const EVIDENCE_OUT_ENV: &str = "OPENPR_FLOW_V05_SESSION_ROUND_TRIP_OUT";
const TEST_ORIGIN: &str = "http://collab-v05-session.local";
const JWT_SECRET: &str = "collab-v05-session-round-trip-secret";

/// Known-shared instances this harness refuses by name. `flow-test-pg` is the workspace-wide test
/// database every other suite in this repository uses; measuring on it costs WAL fsync contention
/// that lands directly in the round trip (measured: p95 64 ms dedicated vs 308 ms shared), so a
/// run against it would be reporting the harness's neighbours, not the code under test.
const KNOWN_SHARED_INSTANCES: &[&str] = &["flow-test-pg"];

// ---------------------------------------------------------------------------------------------
// Environment gate
// ---------------------------------------------------------------------------------------------

fn load_environment_problem() -> Option<(&'static str, String)> {
    let dedicated = std::env::var(DEDICATED_PG_CONTAINER_ENV).unwrap_or_default();
    if dedicated.is_empty() {
        return Some((
            "dedicated_container_not_declared",
            format!("{DEDICATED_PG_CONTAINER_ENV} is required; the v0.5 session round trip was not measured"),
        ));
    }
    let normalized = dedicated.to_ascii_lowercase();
    let quiet_pg_qualified = std::env::var(QUIET_PG_QUALIFIED_ENV).as_deref() == Ok("1");
    if (KNOWN_SHARED_INSTANCES.contains(&normalized.as_str()) || normalized.contains("shared")) && !quiet_pg_qualified {
        return Some((
            "known_shared_postgresql_instance",
            format!("PostgreSQL container {dedicated:?} is shared; the v0.5 session round trip was not measured"),
        ));
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------------------------

fn percentile_ms(sorted_values: &[f64], p: f64) -> f64 {
    if sorted_values.is_empty() {
        return f64::NAN;
    }
    let rank = ((p / 100.0) * sorted_values.len() as f64).ceil().max(1.0) as usize;
    sorted_values
        .get(rank.min(sorted_values.len()) - 1)
        .copied()
        .unwrap_or(f64::NAN)
}

fn sorted(values: &[f64]) -> Vec<f64> {
    let mut out = values.to_vec();
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// A full distribution, never a lone p95 — a p95 with no p50/p99/n behind it cannot be judged for
/// credibility, which is exactly what a gate reviewer has to do.
#[derive(Debug, Clone)]
struct Distribution {
    samples: usize,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
}

impl Distribution {
    fn of(values: &[f64]) -> Self {
        let values = sorted(values);
        Self {
            samples: values.len(),
            p50_ms: percentile_ms(&values, 50.0),
            p95_ms: percentile_ms(&values, 95.0),
            p99_ms: percentile_ms(&values, 99.0),
            max_ms: values.last().copied().unwrap_or(f64::NAN),
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "samples": self.samples,
            "p50_ms": self.p50_ms,
            "p95_ms": self.p95_ms,
            "p99_ms": self.p99_ms,
            "max_ms": self.max_ms,
        })
    }
}

#[derive(Default)]
struct Report {
    postgres_version: String,
    build_profile: &'static str,
    round_trip: Option<Distribution>,
    accepted_total: usize,
    presence_frames_sent: usize,
    presence_frames_observed: usize,
    acks_sent: usize,
    resumes_completed: usize,
    peer_updates_observed: usize,
    peer_accepted_observed: usize,
    fanout_rounds_complete: usize,
    head_seq_final: i64,
    rejections: Vec<String>,
    violations: Vec<String>,
    notes: Vec<String>,
}

impl Report {
    fn fail(&mut self, violation: impl Into<String>) {
        self.violations.push(violation.into());
    }

    fn to_json(&self) -> Value {
        json!({
            "schema_version": "sylvode.flow.v05-session-round-trip.v1",
            "source_head": option_env!("GIT_HASH").unwrap_or("runtime-test-build"),
            "generated_at": Utc::now().to_rfc3339(),
            "environment": {
                "declared_dedicated_pg_container": std::env::var(DEDICATED_PG_CONTAINER_ENV).ok(),
                "known_shared_instances_rejected": KNOWN_SHARED_INSTANCES,
                "quiet_pg_preflight_qualified": std::env::var(QUIET_PG_QUALIFIED_ENV).as_deref() == Ok("1"),
                "postgres_version": self.postgres_version,
                "build_profile": self.build_profile,
            },
            "fixture": {
                "clients": CONCURRENT_CLIENTS,
                "distinct_users": true,
                "warmup_rounds": WARMUP_ROUNDS,
                "measured_rounds": MEASURED_ROUNDS,
                "resuming_clients": RESUMING_CLIENTS,
                "round_pacing_ms": ROUND_PACING_MS,
                "concurrent_v05_session_traffic": [
                    "presence_refresh_per_round",
                    "presence_join_snapshot_on_every_reconnect",
                    "ack_after_every_accepted",
                    "reconnect_resume_between_rounds",
                ],
            },
            "budgets": {
                "round_trip_p95_ms_max": ROUND_TRIP_P95_MS_MAX,
                "min_samples": MIN_SAMPLES,
            },
            "measured": {
                "round_trip": self.round_trip.as_ref().map(Distribution::to_json),
                "accepted_total": self.accepted_total,
                "presence_frames_sent": self.presence_frames_sent,
                "presence_frames_observed": self.presence_frames_observed,
                "acks_sent": self.acks_sent,
                "resumes_completed": self.resumes_completed,
                "peer_updates_observed": self.peer_updates_observed,
                "peer_accepted_observed": self.peer_accepted_observed,
                "fanout_rounds_complete": self.fanout_rounds_complete,
                "head_seq_final": self.head_seq_final,
            },
            "rejections": self.rejections,
            "notes": self.notes,
            "violations": self.violations,
            "passed": self.violations.is_empty(),
        })
    }

    fn emit(&self) {
        let rendered = serde_json::to_string_pretty(&self.to_json()).unwrap_or_else(|_| "{}".to_string());
        println!("---- flow v0.5 session round-trip result ----\n{rendered}");
        if let Ok(path) = std::env::var(EVIDENCE_OUT_ENV) {
            let _ = std::fs::write(path, rendered);
        }
    }
}

fn emit_environment_not_satisfied(reason_code: &str, detail: &str) {
    let value = json!({
        "schema_version": "sylvode.flow.v05-session-round-trip-environment.v1",
        "source_head": option_env!("GIT_HASH").unwrap_or("runtime-test-build"),
        "generated_at": Utc::now().to_rfc3339(),
        "environment_gate": {
            "status": "not_satisfied",
            "reason_code": reason_code,
            "detail": detail,
            "declared_dedicated_pg_container": std::env::var(DEDICATED_PG_CONTAINER_ENV).ok(),
            "known_shared_instances_rejected": KNOWN_SHARED_INSTANCES,
        },
        "execution": {
            "status": "not_run_environment_not_satisfied",
            "harness_started": false,
        },
        "violations": [detail],
        "passed": false,
    });
    let rendered = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string());
    println!("---- flow v0.5 session round-trip environment result ----\n{rendered}");
    if let Ok(path) = std::env::var(EVIDENCE_OUT_ENV) {
        let _ = std::fs::write(path, rendered);
    }
}

const fn build_profile() -> &'static str {
    if cfg!(debug_assertions) { "debug" } else { "release" }
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

    let name = format!("openpr_flow_v05_session_{label}");
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

// ---------------------------------------------------------------------------------------------
// Server + fixture
// ---------------------------------------------------------------------------------------------

fn state_for(db: DatabaseConnection) -> AppState {
    AppState {
        cfg: AppConfig {
            app_name: "collab-v05-session-round-trip".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new(JWT_SECRET),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            default_author_id: None,
            allow_insecure_cookies: false,
            collab_allowed_origins: vec![TEST_ORIGIN.to_string()],
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

/// One workspace, one flow-enabled settings row, and `member_count` owner-role members.
/// **Distinct users, not one user with many tabs**: `versions/v0.5-collaboration.md` scopes this
/// version at "多用户、多 tab", and `limits-v1.md`'s `connections_per_user_max = 16` makes one
/// shared user the wrong shape once reconnect churn is in the mix. Same rationale
/// `flow_collab_load_harness.rs` records for its identical helper.
async fn seed_workspace(db: &DatabaseConnection, member_count: usize) -> (Uuid, Vec<Uuid>) {
    let workspace_id = Uuid::new_v4();
    let mut members = Vec::with_capacity(member_count);
    for index in 0..member_count {
        let user_id = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'v05 session test', 'user', true)",
            vec![user_id.into(), format!("{user_id}@collab-v05-session.test").into()],
        )
        .await;
        if index == 0 {
            exec(
                db,
                "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'v05 session harness', $3)",
                vec![workspace_id.into(), format!("ws-{workspace_id}").into(), user_id.into()],
            )
            .await;
            exec(
                db,
                "INSERT INTO flow_workspace_settings (workspace_id, flow_enabled) VALUES ($1, true)",
                vec![workspace_id.into()],
            )
            .await;
        }
        exec(
            db,
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
            vec![workspace_id.into(), user_id.into()],
        )
        .await;
        members.push(user_id);
    }
    (workspace_id, members)
}

async fn create_page(state: &AppState, workspace_id: Uuid, actor_id: Uuid) -> (Uuid, Uuid) {
    use api::flow::command::{CreateObjectInput, create_object};
    let accepted = create_object(
        state,
        CreateObjectInput {
            origin: CommandOrigin::first_request_from(EventSurface::Rest),
            workspace_id,
            actor_id,
            actor_is_bot: false,
            object_type: "page".to_string(),
            project_id: None,
            parent_object_id: None,
            title: "Flow v0.5 Session Round Trip Page".to_string(),
            idempotency_key: Uuid::new_v4().to_string(),
            message: None,
        },
    )
    .await
    .expect("object creation succeeds");
    (accepted.object.id, accepted.object.document_id)
}

fn jwt_for(user_id: Uuid) -> String {
    JwtManager::new(JWT_SECRET, 900, 3600)
        .issue_access_token(&user_id.to_string(), &format!("{user_id}@collab-v05-session.test"))
        .expect("token issues")
}

/// The same wiring `apps/api/src/main.rs` uses for the two surfaces this harness drives: the
/// authenticated ticket endpoint and the deliberately-unauthenticated WS upgrade (the ticket is
/// the credential, `ADR-0007`).
async fn spawn_server(state: AppState) -> SocketAddr {
    let auth_state = state.clone();
    let app = Router::new()
        .route(
            "/api/v1/collab/tickets",
            post(create_ticket).route_layer(axum_middleware::from_fn_with_state(
                auth_state,
                bot_or_user_auth_middleware,
            )),
        )
        .route("/api/v1/collab/ws", get(ws_upgrade))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds an ephemeral port");
    let addr = listener.local_addr().expect("listener has a local address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    addr
}

// ---------------------------------------------------------------------------------------------
// WebSocket client plumbing
// ---------------------------------------------------------------------------------------------

type WsStream = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn issue_ticket(addr: SocketAddr, token: &str, workspace_id: Uuid, document_id: Uuid, client_id: &str) -> String {
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/api/v1/collab/tickets"))
        .bearer_auth(token)
        .json(&json!({
            "workspace_id": workspace_id,
            "document_id": document_id,
            "client_id": client_id,
            "origin": TEST_ORIGIN,
        }))
        .send()
        .await
        .expect("ticket request completes");
    let body: Value = response.json().await.expect("ticket response is JSON");
    assert_eq!(body["code"], 0, "ticket issuance failed: {body}");
    body["data"]["ticket"].as_str().expect("ticket is a string").to_string()
}

async fn connect(addr: SocketAddr, ticket: &str, client_id: &str) -> WsStream {
    let url = format!("ws://{addr}/api/v1/collab/ws?ticket={ticket}&client_id={client_id}");
    let mut request = url.into_client_request().expect("builds a client request");
    request
        .headers_mut()
        .insert("Origin", TEST_ORIGIN.parse().expect("valid header value"));
    let (stream, response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("upgrade succeeds");
    assert_eq!(response.status(), 101);
    stream
}

async fn send_frame(ws: &mut WsStream, frame: &Frame) {
    let text = serde_json::to_string(frame).expect("frame serializes");
    ws.send(TMessage::Text(text.into())).await.expect("send succeeds");
}

async fn recv_frame(ws: &mut WsStream) -> Frame {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(30), ws.next())
            .await
            .expect("a frame arrives before the timeout")
            .expect("the stream is not closed")
            .expect("the frame is not a transport error");
        match message {
            TMessage::Text(text) => return serde_json::from_str(text.as_str()).expect("frame deserializes"),
            TMessage::Ping(_) | TMessage::Pong(_) => {}
            other => panic!("unexpected non-text frame: {other:?}"),
        }
    }
}

/// What one client knows about the document right now — enough to reconnect through the resume
/// path (`open{known_seq, known_frontier}`) rather than pulling a whole fresh snapshot.
#[derive(Clone)]
struct Position {
    seq: i64,
    frontier: String,
}

/// Completes the `hello` + `open` handshake with no resume hint, returning the stream, a client
/// engine loaded from the delivered snapshot, and this client's position at that snapshot.
async fn open_fresh(
    addr: SocketAddr,
    ticket: &str,
    client_id: &str,
    document_id: Uuid,
) -> (WsStream, LoroCollabEngine, Position) {
    let mut ws = connect(addr, ticket, client_id).await;
    send_frame(
        &mut ws,
        &Frame::Hello {
            protocol_version: PROTOCOL_VERSION,
            capabilities: vec![],
            client_id: client_id.to_string(),
            session_id: Uuid::new_v4(),
        },
    )
    .await;
    let hello_reply = recv_frame(&mut ws).await;
    assert!(
        matches!(hello_reply, Frame::Hello { .. }),
        "expected a hello reply, got {hello_reply:?}"
    );
    send_frame(
        &mut ws,
        &Frame::Open {
            protocol_version: PROTOCOL_VERSION,
            document_id,
            known_seq: None,
            known_frontier: None,
        },
    )
    .await;
    let snapshot_frame = recv_frame(&mut ws).await;
    let Frame::Snapshot {
        head_seq,
        snapshot,
        head_frontier,
        ..
    } = snapshot_frame
    else {
        panic!("a fresh open must answer with a snapshot, got {snapshot_frame:?}");
    };
    let raw = BASE64.decode(&snapshot).expect("snapshot decodes");
    let engine = LoroCollabEngine::load(&raw).expect("snapshot loads");
    let position = Position {
        seq: head_seq,
        frontier: head_frontier,
    };
    (ws, engine, position)
}

/// The v0.5 reconnect path: `open{known_seq, known_frontier}` against a position this client
/// already holds.
///
/// Reports the refusal rather than silently degrading if the server answers with a `snapshot`:
/// this harness exists to measure the resume path, so a fallback here would mean the measurement
/// did not cover what it claims to.
///
/// Deliberately does **not** count the replayed frames: whatever the replay carries arrives on
/// this same stream and is consumed by the next round's peer-traffic arms, so counting it here
/// would mean draining the socket on a guess about how much is coming. A fabricated zero would be
/// worse than not reporting it at all.
async fn open_resuming(
    addr: SocketAddr,
    ticket: &str,
    client_id: &str,
    document_id: Uuid,
    position: &Position,
) -> Result<WsStream, String> {
    let mut ws = connect(addr, ticket, client_id).await;
    send_frame(
        &mut ws,
        &Frame::Hello {
            protocol_version: PROTOCOL_VERSION,
            capabilities: vec![],
            client_id: client_id.to_string(),
            session_id: Uuid::new_v4(),
        },
    )
    .await;
    let hello_reply = recv_frame(&mut ws).await;
    assert!(matches!(hello_reply, Frame::Hello { .. }));
    send_frame(
        &mut ws,
        &Frame::Open {
            protocol_version: PROTOCOL_VERSION,
            document_id,
            known_seq: Some(position.seq),
            known_frontier: Some(position.frontier.clone()),
        },
    )
    .await;

    let answer = recv_frame(&mut ws).await;
    let Frame::Ack { seq, .. } = answer else {
        return Err(format!(
            "resume from seq {} was refused; the server answered {answer:?} instead of an ack",
            position.seq
        ));
    };
    if seq != position.seq {
        return Err(format!(
            "resume ack confirmed seq {seq}, not the requested {}",
            position.seq
        ));
    }
    Ok(ws)
}

// ---------------------------------------------------------------------------------------------
// The workload
// ---------------------------------------------------------------------------------------------

/// The result of draining one client's inbound stream until its own `accepted` arrives.
struct AcceptedWait {
    /// `(head_seq, head_frontier)` of this client's own `accepted`, or `None` if the stream
    /// produced a rejection/resync first.
    own: Option<(i64, String)>,
    /// Peer `presence` frames seen while waiting — the evidence that presence fan-out really was
    /// live during the measured window rather than merely configured.
    presence_observed: usize,
    peer_updates: HashSet<Uuid>,
    peer_accepted: HashSet<Uuid>,
    latest_position: Option<Position>,
    rejections: Vec<String>,
}

/// Drains inbound frames until this client's own `accepted` (matched by `update_id`) arrives.
/// Peer `update`/`accepted`/`presence` traffic is consumed and, for presence, counted; it is never
/// timed. Split out of the round loop so the timing decision stays one statement at the call site.
async fn await_own_accepted(ws: &mut WsStream, update_id: Uuid, index: usize, round: usize) -> AcceptedWait {
    let mut wait = AcceptedWait {
        own: None,
        presence_observed: 0,
        peer_updates: HashSet::new(),
        peer_accepted: HashSet::new(),
        latest_position: None,
        rejections: Vec::new(),
    };
    loop {
        match recv_frame(ws).await {
            Frame::Accepted {
                update_id: got,
                head_seq,
                head_frontier,
                ..
            } => {
                wait.latest_position = Some(Position {
                    seq: head_seq,
                    frontier: head_frontier.clone(),
                });
                if got == update_id {
                    wait.own = Some((head_seq, head_frontier));
                    return wait;
                }
                wait.peer_accepted.insert(got);
            }
            // Peer traffic; deliberately not timed.
            Frame::Update { update_id: got, .. } => {
                wait.peer_updates.insert(got);
            }
            Frame::Presence { .. } => wait.presence_observed += 1,
            Frame::Rejected { code, details, .. } => {
                wait.rejections
                    .push(format!("client {index} round {round}: {code:?} {details:?}"));
                return wait;
            }
            Frame::Resync { reason, .. } => {
                wait.rejections
                    .push(format!("client {index} round {round}: resync({reason})"));
                return wait;
            }
            other => panic!("unexpected frame while waiting for an accepted: {other:?}"),
        }
    }
}

/// Drains every frame queued before a controlled registry marker. The marker is broadcast only
/// after all ten clients have received their own acceptance, so seeing it proves this session has
/// crossed every peer fan-out for the round. This is a deterministic barrier, not a scheduler
/// race or a sleep-based guess.
async fn drain_fanout_barrier(
    ws: &mut WsStream,
    marker_nonce: &str,
    own_update_id: Uuid,
    wait: &mut AcceptedWait,
) -> Result<(), String> {
    loop {
        match recv_frame(ws).await {
            Frame::Ping { nonce, .. } if nonce == marker_nonce => return Ok(()),
            Frame::Update { update_id, .. } if update_id != own_update_id => {
                wait.peer_updates.insert(update_id);
            }
            Frame::Accepted {
                update_id,
                head_seq,
                head_frontier,
                ..
            } if update_id != own_update_id => {
                wait.peer_accepted.insert(update_id);
                wait.latest_position = Some(Position {
                    seq: head_seq,
                    frontier: head_frontier,
                });
            }
            Frame::Presence { .. } => wait.presence_observed += 1,
            Frame::Rejected { code, details, .. } => {
                return Err(format!("rejected before fan-out barrier: {code:?} {details:?}"));
            }
            Frame::Resync { reason, .. } => return Err(format!("resync before fan-out barrier: {reason}")),
            Frame::Ping { nonce, .. } => return Err(format!("unexpected ping marker before {marker_nonce}: {nonce}")),
            other => return Err(format!("unexpected frame before fan-out barrier: {other:?}")),
        }
    }
}

struct ClientOutcome {
    round_trip_ms: Vec<f64>,
    accepted: usize,
    presence_sent: usize,
    presence_observed: usize,
    acks_sent: usize,
    resumes: usize,
    rejections: Vec<String>,
    resume_failures: Vec<String>,
    peer_updates_observed: usize,
    peer_accepted_observed: usize,
    fanout_rounds_complete: usize,
}

struct Fixture {
    addr: SocketAddr,
    workspace_id: Uuid,
    document_id: Uuid,
    tokens: Vec<String>,
    round_barrier: Arc<Barrier>,
}

impl Fixture {
    /// One client's whole run: warm up, then measure, with WP-16's own session traffic live the
    /// entire time.
    ///
    /// Per round, in order: refresh `presence`, send one real Loro delta, drain inbound frames
    /// until this client's own `accepted` (timing exactly that span, and counting every peer
    /// `presence` seen along the way), then `ack` the position just reached. Clients selected for
    /// reconnect churn additionally drop the socket and come back through the **resume** path
    /// between rounds.
    async fn run_client(self: Arc<Self>, index: usize) -> ClientOutcome {
        let token = self.tokens[index].clone();
        let client_id = format!("v05-session-client-{index}");
        let resuming = index < RESUMING_CLIENTS;

        let ticket = issue_ticket(self.addr, &token, self.workspace_id, self.document_id, &client_id).await;
        // The snapshot position is never a resume point here: a reconnect only ever happens
        // after this client's own `accepted` in the same round, so the position it resumes from is
        // always the one built below.
        let (mut ws, mut engine, _) = open_fresh(self.addr, &ticket, &client_id, self.document_id).await;

        let mut outcome = ClientOutcome {
            round_trip_ms: Vec::with_capacity(MEASURED_ROUNDS),
            accepted: 0,
            presence_sent: 0,
            presence_observed: 0,
            acks_sent: 0,
            resumes: 0,
            rejections: Vec::new(),
            resume_failures: Vec::new(),
            peer_updates_observed: 0,
            peer_accepted_observed: 0,
            fanout_rounds_complete: 0,
        };

        // No round begins until all ten sockets are registered. Without this barrier, early
        // clients could commit while late clients were still handshaking and a partial fan-out
        // implementation would be indistinguishable from a scheduling accident.
        self.round_barrier.wait().await;

        for round in 0..(WARMUP_ROUNDS + MEASURED_ROUNDS) {
            let measured = round >= WARMUP_ROUNDS;

            // ---- v0.5 traffic: presence refresh, on the same egress path as the write ----
            send_frame(
                &mut ws,
                &Frame::Presence {
                    protocol_version: PROTOCOL_VERSION,
                    document_id: self.document_id,
                    // Ignored by the server: the entry key is `(document_id, server session_id)`.
                    session_id: Uuid::new_v4(),
                    payload: json!({"cursor": round, "client": index}),
                    ttl_seconds: Some(30),
                },
            )
            .await;
            outcome.presence_sent += 1;

            // ---- the write itself ----
            let base_frontier = engine.frontier();
            engine
                .set_title(&format!("v05-session-{index}-{round}-{}", Uuid::new_v4()))
                .expect("set_title succeeds");
            let bytes = engine.export_from(&base_frontier).expect("export succeeds");
            let update_id = Uuid::new_v4();
            let started = Instant::now();
            send_frame(
                &mut ws,
                &Frame::Update {
                    protocol_version: PROTOCOL_VERSION,
                    document_id: self.document_id,
                    update_id,
                    base_frontier: BASE64.encode(base_frontier.as_bytes()),
                    bytes: BASE64.encode(&bytes),
                    idempotency_key: None,
                    origin: client_id.clone(),
                    message: None,
                },
            )
            .await;

            // ---- wait for this client's own accepted ----
            let mut wait = await_own_accepted(&mut ws, update_id, index, round).await;
            let Some((head_seq, head_frontier)) = wait.own.clone() else {
                outcome.presence_observed += wait.presence_observed;
                outcome.rejections.append(&mut wait.rejections);
                break;
            };
            let elapsed = started.elapsed().as_secs_f64() * 1_000.0;
            if measured {
                outcome.round_trip_ms.push(elapsed);
            }
            outcome.accepted += 1;

            // Every commit's registry broadcasts have completed before its submitter receives
            // `accepted`. Once all submitters reach this barrier, one leader queues a marker
            // behind all 10 commits on every live session. Each client must observe exactly the
            // other nine update/accepted pairs before that marker.
            let marker_nonce = format!("fanout-round-{round}");
            let barrier = self.round_barrier.wait().await;
            if barrier.is_leader() {
                api::flow::collab::runtime::runtime().registry.broadcast(
                    self.document_id,
                    &Frame::Ping {
                        protocol_version: PROTOCOL_VERSION,
                        nonce: marker_nonce.clone(),
                    },
                    None,
                );
            }
            if let Err(problem) = drain_fanout_barrier(&mut ws, &marker_nonce, update_id, &mut wait).await {
                outcome
                    .rejections
                    .push(format!("client {index} round {round}: {problem}"));
                break;
            }
            outcome.presence_observed += wait.presence_observed;
            outcome.rejections.append(&mut wait.rejections);
            let expected_peers = CONCURRENT_CLIENTS - 1;
            if wait.peer_updates.len() != expected_peers || wait.peer_accepted.len() != expected_peers {
                outcome.rejections.push(format!(
                    "client {index} round {round}: fan-out incomplete before barrier: updates={} accepted={} expected_each={expected_peers}",
                    wait.peer_updates.len(),
                    wait.peer_accepted.len(),
                ));
                break;
            }
            if wait.peer_updates != wait.peer_accepted {
                outcome.rejections.push(format!(
                    "client {index} round {round}: peer update ids differ from peer accepted ids"
                ));
                break;
            }
            outcome.peer_updates_observed += wait.peer_updates.len();
            outcome.peer_accepted_observed += wait.peer_accepted.len();
            outcome.fanout_rounds_complete += 1;

            // ---- v0.5 traffic: ack the position just reached ----
            let position = wait.latest_position.unwrap_or(Position {
                seq: head_seq,
                frontier: head_frontier,
            });
            send_frame(
                &mut ws,
                &Frame::Ack {
                    protocol_version: PROTOCOL_VERSION,
                    document_id: self.document_id,
                    seq: position.seq,
                    frontier: position.frontier.clone(),
                },
            )
            .await;
            outcome.acks_sent += 1;

            // ---- v0.5 traffic: the "offline" half — drop and come back through resume ----
            if resuming {
                let _ = ws.close(None).await;
                drop(ws);
                let ticket = issue_ticket(self.addr, &token, self.workspace_id, self.document_id, &client_id).await;
                match open_resuming(self.addr, &ticket, &client_id, self.document_id, &position).await {
                    Ok(resumed) => {
                        ws = resumed;
                        outcome.resumes += 1;
                    }
                    Err(problem) => {
                        outcome.resume_failures.push(problem);
                        // A fresh ticket, not the one just consumed: `ADR-0007` tickets are
                        // single-use, so reusing it here would fail the *upgrade* and crash this
                        // harness instead of reporting the resume refusal it is trying to record.
                        let retry_ticket =
                            issue_ticket(self.addr, &token, self.workspace_id, self.document_id, &client_id).await;
                        let (fresh, fresh_engine, _) =
                            open_fresh(self.addr, &retry_ticket, &client_id, self.document_id).await;
                        ws = fresh;
                        engine = fresh_engine;
                    }
                }
            }

            // Resuming clients must finish reconnecting before anybody begins the next round;
            // otherwise an update legitimately sent during their offline interval would make the
            // next round's exact nine-peer fan-out count ambiguous.
            self.round_barrier.wait().await;

            tokio::time::sleep(Duration::from_millis(ROUND_PACING_MS)).await;
        }

        let _ = ws.close(None).await;
        outcome
    }
}

#[derive(FromQueryResult)]
struct HeadRow {
    head_seq: i64,
}

async fn read_head_seq(db: &DatabaseConnection, document_id: Uuid) -> i64 {
    HeadRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT head_seq FROM collab_documents WHERE id = $1",
        vec![document_id.into()],
    ))
    .one(db)
    .await
    .expect("head_seq query runs")
    .expect("document row exists")
    .head_seq
}

#[derive(FromQueryResult)]
struct VersionRow {
    version: String,
}

async fn server_version(db: &DatabaseConnection) -> String {
    VersionRow::find_by_statement(Statement::from_string(
        DbBackend::Postgres,
        "SELECT version() AS version".to_string(),
    ))
    .one(db)
    .await
    .ok()
    .flatten()
    .map_or_else(|| "unknown".to_string(), |row| row.version)
}

// ---------------------------------------------------------------------------------------------
// The measurement
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "environment-gated heavy measurement; run explicitly through the dedicated PostgreSQL harness"]
async fn v05_multi_user_session_workload_round_trip_p95() {
    if let Some((reason_code, detail)) = load_environment_problem() {
        emit_environment_not_satisfied(reason_code, &detail);
        panic!("ENVIRONMENT NOT SATISFIED [{reason_code}]: {detail}");
    }

    // Without a real database there is nothing to measure. This test is ignored by ordinary
    // workspace runs; when explicitly selected, an unsatisfied environment is a failure.
    let Some(scratch) = scratch("round_trip").await else {
        let mut skipped = Report {
            build_profile: build_profile(),
            ..Report::default()
        };
        skipped.fail(format!(
            "{TEST_DATABASE_URL_ENV} is not set, so no v0.5 session round trip was measured; this is not a \
             measurement and must not be read as one"
        ));
        skipped.emit();
        panic!("{TEST_DATABASE_URL_ENV} is not set; the v0.5 session harness measured nothing");
    };

    let mut report = Report {
        build_profile: build_profile(),
        postgres_version: server_version(&scratch.db).await,
        ..Report::default()
    };

    let state = state_for(scratch.db.clone());
    let (workspace_id, members) = seed_workspace(&state.db, CONCURRENT_CLIENTS).await;
    let (_object_id, document_id) = create_page(&state, workspace_id, members[0]).await;
    let head_seq_before = read_head_seq(&state.db, document_id).await;
    let addr = spawn_server(state.clone()).await;

    let fixture = Arc::new(Fixture {
        addr,
        workspace_id,
        document_id,
        tokens: members.iter().map(|user_id| jwt_for(*user_id)).collect(),
        round_barrier: Arc::new(Barrier::new(CONCURRENT_CLIENTS)),
    });

    let mut handles = Vec::with_capacity(CONCURRENT_CLIENTS);
    for index in 0..CONCURRENT_CLIENTS {
        let fixture = fixture.clone();
        handles.push(tokio::spawn(async move { fixture.run_client(index).await }));
    }

    let mut round_trip_ms = Vec::new();
    let mut resume_failures = Vec::new();
    for handle in handles {
        let outcome = handle.await.expect("client task completes");
        round_trip_ms.extend(outcome.round_trip_ms);
        report.accepted_total += outcome.accepted;
        report.presence_frames_sent += outcome.presence_sent;
        report.presence_frames_observed += outcome.presence_observed;
        report.acks_sent += outcome.acks_sent;
        report.resumes_completed += outcome.resumes;
        report.peer_updates_observed += outcome.peer_updates_observed;
        report.peer_accepted_observed += outcome.peer_accepted_observed;
        report.fanout_rounds_complete += outcome.fanout_rounds_complete;
        report.rejections.extend(outcome.rejections);
        resume_failures.extend(outcome.resume_failures);
    }

    report.round_trip = Some(Distribution::of(&round_trip_ms));
    report.head_seq_final = read_head_seq(&state.db, document_id).await;

    // ---- the budget under test ----
    if let Some(round_trip) = report.round_trip.clone() {
        if round_trip.samples < MIN_SAMPLES {
            report.fail(format!(
                "round_trip_p95 has only {} samples; ADR-0010's official-run protocol requires at least {MIN_SAMPLES}",
                round_trip.samples
            ));
        }
        if round_trip.p95_ms > ROUND_TRIP_P95_MS_MAX {
            report.fail(format!(
                "v0.5 multi-user session workload accepted round_trip_p95 = {:.1}ms exceeds \
                 collab_accepted_round_trip_ms_p95_max of {ROUND_TRIP_P95_MS_MAX:.0}ms \
                 (p50={:.1} p99={:.1} max={:.1} n={})",
                round_trip.p95_ms, round_trip.p50_ms, round_trip.p99_ms, round_trip.max_ms, round_trip.samples
            ));
        }
    } else {
        report.fail("no round-trip distribution was produced");
    }

    // ---- the workload really was the v0.5 one ----
    if !report.rejections.is_empty() {
        report.fail(format!(
            "the workload produced {} rejection/resync frames; the round trip was not measured over a clean run: {:?}",
            report.rejections.len(),
            report.rejections
        ));
    }
    if !resume_failures.is_empty() {
        report.fail(format!(
            "{} reconnects fell back to a full snapshot instead of resuming, so the measured window did not \
             cover the resume path: {resume_failures:?}",
            resume_failures.len()
        ));
    }
    let expected_accepted = CONCURRENT_CLIENTS * (WARMUP_ROUNDS + MEASURED_ROUNDS);
    if report.accepted_total != expected_accepted {
        report.fail(format!(
            "expected {expected_accepted} accepted commits, observed {}",
            report.accepted_total
        ));
    }
    let expected_advance = i64::try_from(expected_accepted).expect("fixture size fits i64");
    if report.head_seq_final != head_seq_before + expected_advance {
        report.fail(format!(
            "the canonical head advanced to {} from {head_seq_before}; every concurrent commit must land exactly once",
            report.head_seq_final
        ));
    }
    if report.presence_frames_observed == 0 {
        report.fail(
            "no client observed a single peer presence frame; the presence fan-out this harness claims to \
             measure alongside was not actually live",
        );
    }
    let expected_fanout_rounds = CONCURRENT_CLIENTS * (WARMUP_ROUNDS + MEASURED_ROUNDS);
    let expected_peer_frames = expected_fanout_rounds * (CONCURRENT_CLIENTS - 1);
    if report.fanout_rounds_complete != expected_fanout_rounds
        || report.peer_updates_observed != expected_peer_frames
        || report.peer_accepted_observed != expected_peer_frames
    {
        report.fail(format!(
            "controlled fan-out barriers incomplete: rounds={}/{} peer_updates={}/{} peer_accepted={}/{}",
            report.fanout_rounds_complete,
            expected_fanout_rounds,
            report.peer_updates_observed,
            expected_peer_frames,
            report.peer_accepted_observed,
            expected_peer_frames,
        ));
    }
    let expected_resumes = RESUMING_CLIENTS * (WARMUP_ROUNDS + MEASURED_ROUNDS);
    if report.resumes_completed != expected_resumes {
        report.fail(format!(
            "expected {expected_resumes} completed resumes, observed {}",
            report.resumes_completed
        ));
    }
    report.notes.push(format!(
        "measured with {} exact peer update/accepted pairs behind controlled barriers, {} peer presence frames, \
         {} acks and {} resumes inside the measured window",
        report.peer_accepted_observed, report.presence_frames_observed, report.acks_sent, report.resumes_completed
    ));

    report.emit();
    let passed = report.violations.is_empty();
    scratch.drop_self().await;
    assert!(
        passed,
        "v0.5 session round-trip harness failed: {:?}",
        report.violations
    );
}
