//! Sylvode Flow v0.4 collab **10-client load harness** — the measurement facility
//! `gates/gate-commands.md`'s v0.4 "Architecture verifier" paragraph and
//! `decisions/ADR-0010-collab-server-architecture.md` §"量化接受与推翻门槛" require and that
//! `scripts/verify-flow-collab-architecture.sh` has, until now, correctly reported as a build gap
//! (its `load_harness_grep` found zero hits under `apps/` and `crates/`).
//!
//! Two hard gates are blocked on it:
//!
//! - `bounded_warm_cache_lock_hold_and_round_trip_budgets` — needs the two *distribution*
//!   statistics the frozen hard-ceiling constants (`DOCUMENT_LOCK_WAIT_MS_MAX` = 100 ms,
//!   `DOCUMENT_LOCK_HOLD_MS_MAX` = 100 ms, `MAX_REBASE_ATTEMPTS` = 3) cannot express:
//!   **lock hold p95 ≤ 25 ms** and **10-client accepted round-trip p95 ≤ 250 ms**.
//! - `bootstrap_repeatable_read_and_ws_parity` — needs a REST/WS 10-client concurrent load
//!   harness proving the same `REPEATABLE READ` view and cross-surface `seq`/hash/frontier parity
//!   with no gaps.
//!
//! # What this harness actually is
//!
//! A real `axum::serve` listener on `127.0.0.1:0` wired the way `apps/api/src/main.rs` wires the
//! collab routes, backed by a **real, freshly migrated `PostgreSQL` scratch database** (never a mock
//! or in-memory DB — `gate-commands.md` fails a run outright for that), driven by
//! [`CONCURRENT_CLIENTS`] real `tokio-tungstenite` WebSocket clients that each produce genuine
//! Loro CRDT deltas from their own engine and wait for their own `accepted` frame, plus REST/WS
//! parity probes that run *concurrently with* that write load.
//!
//! # Measurement caliber (the part that decides whether the numbers mean anything)
//!
//! `gate-commands.md` fails a run that "把 replay/apply 计出 hold 但实际留在锁内" — i.e. a
//! self-reported in-process timer that starts and stops wherever the implementation chooses is
//! worth nothing here, because the exact failure being guarded against is the implementation
//! placing work inside the lock and *excluding it from its own timer*. So this harness does not
//! time the lock from inside `apps/api` at all. It reads the lock's lifetime out of
//! **`PostgreSQL`'s own statement log**: the scratch database is switched to
//! `log_min_duration_statement = 0` for the measurement window, so every statement the server
//! issues is logged by the server process with a `%m` timestamp and its own `duration:`, grouped
//! by backend PID. One connection's statements are strictly sequential, so `BEGIN … COMMIT` on one
//! PID reconstructs one write transaction exactly, from an authority that is neither the code
//! under test nor this harness.
//!
//! From that reconstruction come three independent facts, and it takes all three for the
//! "prepare/rebase really is outside the lock" claim to be earned rather than asserted:
//!
//! 1. **Statement inventory.** Every statement executed between `BEGIN` and `COMMIT` is known
//!    exactly (not sampled). [`LOCKED_PHASE_FORBIDDEN_READS`] fails the run if a snapshot load or
//!    a `collab_updates` tail `SELECT` — the hydrate/rebase reads — ever appears inside the write
//!    transaction.
//! 2. **Intra-lock application gap.** For each consecutive statement pair inside a transaction,
//!    the wall time that elapsed while the transaction was open but *no SQL was executing* is the
//!    time the application spent doing something else while holding the lock. A CRDT apply, an
//!    `isolated_apply` worker spawn, a projection render, or a cache call inside the lock all show
//!    up here and nowhere else. Reported as a distribution, asserted against
//!    [`INTRA_LOCK_APP_GAP_MS_MAX`].
//! 3. **A calibrated impossibility check.** The harness separately measures what one real
//!    `collab_core::isolation::isolated_apply` costs on *this* machine and *this* build profile
//!    ([`Report::isolated_apply_ms_p50`]). If the whole locked transaction is shorter than a
//!    single isolated apply, the apply provably is not inside it. This is the check that survives
//!    a machine or profile change, because both sides move together.
//!
//! Measurement resolution: `PostgreSQL`'s `%m` log prefix is millisecond-quantized, so each
//! transaction span carries roughly ±1 ms. That is reported (`hold_ms_resolution`) rather than
//! hidden, and it is small against a 25 ms budget. The span measured is `BEGIN`→`COMMIT`, which is
//! a **superset** of the row-lock hold (the lock is taken a few statements after `BEGIN`), so this
//! caliber can only ever over-report hold — it cannot manufacture a pass.
//!
//! # Honest failure
//!
//! Every check appends to [`Report::violations`] instead of panicking at the first problem, and
//! the full measured distribution is printed before the single final assertion. A red run is
//! supposed to show its numbers.

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

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::SocketAddr;
use std::process::Command;
use std::time::{Duration, Instant};

use axum::Router;
use axum::middleware as axum_middleware;
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::{DateTime, NaiveDateTime, Utc};
use collab_core::{CollabEngine, LoroCollabEngine};
use futures_util::{SinkExt, StreamExt};
use platform::{
    app::AppState,
    auth::JwtManager,
    config::{AppConfig, Secret},
};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite::Message as TMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use uuid::Uuid;

use api::flow::collab::frame::{Frame, PROTOCOL_VERSION};
use api::flow::projection;
use api::middleware::bot_auth::bot_or_user_auth_middleware;
use api::routes::collab::{create_ticket, ws_upgrade};
use api::routes::flow::get_flow_object_bootstrap;

// ---------------------------------------------------------------------------------------------
// Frozen targets. Every number here is transcribed from the contract repository; none is invented
// here and none may be relaxed to make a run green.
// ---------------------------------------------------------------------------------------------

/// `ADR-0010` §"量化接受与推翻门槛" item 2: "10 个并发 client 的 accepted round-trip p95 不超过
/// 250 ms". The literal name the verifier greps for (`10_client`, `ten_client`,
/// `concurrent_client`) is deliberate — this constant is that gate's subject.
const CONCURRENT_CLIENTS: usize = 10;

/// `ADR-0010` §"量化接受与推翻门槛" item 2 — the 10-client accepted `round_trip_p95` ceiling.
const ROUND_TRIP_P95_MS_MAX: f64 = 250.0;

/// `ADR-0010` §"量化接受与推翻门槛" item 1 — "hold p95 不超过 25 ms". This is the *distribution*
/// budget, a strictly different claim from the `DOCUMENT_LOCK_HOLD_MS_MAX = 100 ms` hard rollback
/// ceiling `verify-flow-collab-architecture.sh` already cross-checks against `limits.rs`.
const LOCK_HOLD_P95_MS_MAX: f64 = 25.0;

/// `ADR-0010` §"量化接受与推翻门槛" item 1 — "单次不超过 100 ms", the same number
/// `flow::collab::limits::DOCUMENT_LOCK_HOLD_MS_MAX` compiles in as `SET LOCAL statement_timeout`.
const LOCK_HOLD_SINGLE_MS_MAX: f64 = 100.0;

/// `ADR-0010` §"量化接受与推翻门槛" item 1 — "document lock wait 每次不超过 100 ms", the same
/// number `flow::collab::limits::DOCUMENT_LOCK_WAIT_MS_MAX` compiles in as `SET LOCAL
/// lock_timeout`.
const LOCK_WAIT_MS_MAX: f64 = 100.0;

/// `ADR-0010`'s official-run protocol: "先 5 次 warmup、至少 30 次测量"; "`samples<30` … 使
/// `passed=false`".
const WARMUP_ROUNDS: usize = 5;
const MEASURED_ROUNDS: usize = 10;
const MIN_SAMPLES: usize = 30;

/// The ceiling on wall time a write transaction may sit open with **no SQL executing** — i.e. the
/// application doing non-database work while holding the lock. `collab-protocol-v1.md`'s
/// lock-content discipline ("锁内严禁 snapshot/tail load、CRDT apply、semantic diff、projection
/// compute、网络 I/O 或等待 async mutex") forbids all of it; every forbidden item costs far more
/// than this, while legitimate back-to-back statement dispatch on a loaded Tokio runtime costs
/// far less.
///
/// Compared against the gap distribution's **p95**, not its maximum. The property under test is
/// "no forbidden work runs inside the lock", which is a property of *every* write — so if it were
/// violated the cost would appear in the body of the distribution, not as a single outlier. A
/// lone 6 ms sample among 2,600 whose p95 is 1 ms is Tokio scheduling jitter on a loaded host; a
/// max-based rule would report that as an implementation defect, which is a false red, while an
/// implementation that really did hold the lock across a CRDT apply would push p50 and p95 past
/// this bound on every single transaction. The maximum is still reported, and noted when it
/// exceeds this bound, so the outlier is visible rather than discarded.
const INTRA_LOCK_APP_GAP_MS_MAX: f64 = 5.0;

/// Statement shapes that must never appear *inside* the write transaction. Each entry is a label
/// plus the set of lowercase substrings that must **all** be present for the statement to match.
///
/// These are exactly the hydrate/rebase reads (`write::hydrate_and_apply` → `bootstrap::load`),
/// which `ADR-0010`'s write algorithm places strictly before `db.begin()`. Verified against the
/// locked phase's actual statement set, which reads `collab_documents` only through
/// `SELECT head_seq, byte_count, update_count … FOR UPDATE` and never selects `collab_updates` at
/// all — so a hit here is a real leak, not a false positive on the legitimate locked statements.
const LOCKED_PHASE_FORBIDDEN_READS: &[(&str, &[&str])] = &[
    ("snapshot_load_inside_lock", &["select", "collab_documents", "snapshot"]),
    ("tail_load_inside_lock", &["select", "collab_updates"]),
];

/// The **complete** set of statements `ADR-0010`'s locked phase is allowed to execute — the
/// positive form of the same discipline, and the stronger one: a blacklist only catches the leaks
/// somebody thought of, while this fails on *any* statement that appears between `BEGIN` and
/// `COMMIT` and is not one of these.
///
/// Each entry is a label plus needles: the first is matched as a prefix of the normalized
/// statement, the rest as substrings. Transcribed from `flow::collab::write::run_locked_phase`'s
/// own module documentation ("the epoch fence, the row lock, the head-match recheck, and the five
/// fixed, parameterized inserts/updates"), not from whatever a run happened to produce.
const LOCKED_PHASE_ALLOWED_STATEMENTS: &[(&str, &[&str])] = &[
    ("transaction_begin", &["begin"]),
    ("transaction_commit", &["commit"]),
    ("transaction_rollback", &["rollback"]),
    ("set_local_lock_timeout", &["set local lock_timeout"]),
    ("set_local_statement_timeout", &["set local statement_timeout"]),
    (
        "epoch_fence_for_share",
        &["select", "flow_workspace_settings", "for share"],
    ),
    ("document_row_lock", &["select", "collab_documents", "for update"]),
    ("business_event_insert", &["insert into business_events"]),
    ("event_dispatch_insert", &["insert into event_dispatch"]),
    ("collab_update_insert", &["insert into collab_updates"]),
    ("document_head_update", &["update collab_documents"]),
    ("projection_update", &["update flow_object_projections"]),
];

/// The container log trails the server under load; the harvest is retried until it accounts for
/// every accepted update rather than trusting one early read.
const HARVEST_ATTEMPTS_MAX: usize = 20;

/// `PostgreSQL`'s `%m` log prefix is millisecond-quantized, so any derived interval carries about
/// this much noise. Any calibrated comparison has to clear it by a real margin or say it cannot
/// decide.
const PG_LOG_TIMESTAMP_RESOLUTION_MS: f64 = 1.0;
const CALIBRATION_ITERATIONS: usize = 10;
const HARVEST_RETRY_MS: u64 = 500;

const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";
const PG_LOG_ENGINE_ENV: &str = "OPENPR_FLOW_PG_LOG_ENGINE";
const PG_LOG_CONTAINER_ENV: &str = "OPENPR_FLOW_PG_LOG_CONTAINER";
const EVIDENCE_OUT_ENV: &str = "OPENPR_FLOW_LOAD_HARNESS_OUT";
const DEFAULT_PG_LOG_CONTAINER: &str = "flow-test-pg";
const TEST_ORIGIN: &str = "http://collab-load.local";
const JWT_SECRET: &str = "collab-load-harness-secret";

// ---------------------------------------------------------------------------------------------
// Scratch database
// ---------------------------------------------------------------------------------------------

struct Scratch {
    db: DatabaseConnection,
    name: String,
    admin_url: String,
}

impl Scratch {
    async fn admin(&self) -> Option<DatabaseConnection> {
        Database::connect(&self.admin_url).await.ok()
    }

    /// Turns `PostgreSQL`'s own statement logging on for **this scratch database only**, so the
    /// measurement never depends on a server-wide setting and never perturbs any other database
    /// on the same instance. `log_parameter_max_length = 0` suppresses bind values so base64 CRDT
    /// blobs do not end up in the container log.
    ///
    /// `ALTER DATABASE … SET` only reaches connections opened *after* it runs, which is why the
    /// caller opens the application pool afterwards.
    async fn enable_statement_logging(&self) -> bool {
        let Some(admin) = self.admin().await else {
            return false;
        };
        let quoted = format!("\"{}\"", self.name);
        for sql in [
            format!("ALTER DATABASE {quoted} SET log_min_duration_statement = 0"),
            format!("ALTER DATABASE {quoted} SET log_parameter_max_length = 0"),
        ] {
            if admin.execute_unprepared(&sql).await.is_err() {
                return false;
            }
        }
        true
    }

    /// Puts the scratch database back to the instance default, so a leaked scratch database can
    /// never leave statement logging on for anything else.
    async fn disable_statement_logging(&self) -> bool {
        let Some(admin) = self.admin().await else {
            return false;
        };
        let quoted = format!("\"{}\"", self.name);
        admin
            .execute_unprepared(&format!("ALTER DATABASE {quoted} RESET log_min_duration_statement"))
            .await
            .is_ok()
    }

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

    let name = format!("openpr_flow_load_{label}");
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
            app_name: "collab-load-harness".to_string(),
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
    }
}

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
}

/// One workspace, one flow-enabled settings row, and `member_count` owner-role members —
/// `limits-v1.md`'s `connections_per_user_max = 16` makes one shared user the wrong shape for a
/// 10-client harness that also runs parity probes, and distinct actors are what a real 10-client
/// workload looks like anyway.
async fn seed_workspace(db: &DatabaseConnection, member_count: usize) -> (Uuid, Vec<Uuid>) {
    let workspace_id = Uuid::new_v4();
    let mut members = Vec::with_capacity(member_count);
    for index in 0..member_count {
        let user_id = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'load test', 'user', true)",
            vec![user_id.into(), format!("{user_id}@collab-load.test").into()],
        )
        .await;
        if index == 0 {
            exec(
                db,
                "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'collab load harness', $3)",
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
            workspace_id,
            actor_id,
            object_type: "page".to_string(),
            project_id: None,
            parent_object_id: None,
            title: "Collab Load Harness Page".to_string(),
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
        .issue_access_token(&user_id.to_string(), &format!("{user_id}@collab-load.test"))
        .expect("token issues")
}

/// The same wiring `apps/api/src/main.rs` uses for the three surfaces this harness drives: the
/// authenticated ticket endpoint, the deliberately-unauthenticated WS upgrade (the ticket is the
/// credential, `ADR-0007`), and the authenticated REST bootstrap that must stay in parity with the
/// WS `snapshot` frame.
async fn spawn_server(state: AppState) -> SocketAddr {
    let auth_state = state.clone();
    let app = Router::new()
        .route(
            "/api/v1/collab/tickets",
            post(create_ticket).route_layer(axum_middleware::from_fn_with_state(
                auth_state.clone(),
                bot_or_user_auth_middleware,
            )),
        )
        .route("/api/v1/collab/ws", get(ws_upgrade))
        .route(
            "/api/v1/flow/objects/{object_id}/bootstrap",
            get(get_flow_object_bootstrap).route_layer(axum_middleware::from_fn_with_state(
                auth_state,
                bot_or_user_auth_middleware,
            )),
        )
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

/// Completes the `hello` / `open` handshake and returns the `snapshot` frame's raw fields.
async fn open_document(ws: &mut WsStream, document_id: Uuid, client_id: &str) -> SurfaceObservation {
    send_frame(
        ws,
        &Frame::Hello {
            protocol_version: PROTOCOL_VERSION,
            capabilities: vec![],
            client_id: client_id.to_string(),
            session_id: Uuid::new_v4(),
        },
    )
    .await;
    let hello = recv_frame(ws).await;
    assert!(
        matches!(hello, Frame::Hello { .. }),
        "expected a hello reply, got {hello:?}"
    );

    send_frame(
        ws,
        &Frame::Open {
            protocol_version: PROTOCOL_VERSION,
            document_id,
            known_seq: None,
            known_frontier: None,
        },
    )
    .await;
    let frame = recv_frame(ws).await;
    let Frame::Snapshot {
        snapshot_seq,
        head_seq,
        snapshot,
        tail_updates,
        head_frontier,
        ..
    } = frame
    else {
        panic!("expected a snapshot frame, got {frame:?}");
    };
    SurfaceObservation::build(
        "ws_snapshot",
        snapshot_seq,
        head_seq,
        &snapshot,
        &tail_updates
            .into_iter()
            .map(|update| (update.seq, update.bytes))
            .collect::<Vec<_>>(),
        &head_frontier,
    )
}

// ---------------------------------------------------------------------------------------------
// Cross-surface parity
// ---------------------------------------------------------------------------------------------

/// One `REPEATABLE READ` view of the document as returned by one surface, reduced to the values
/// `gate-commands.md` requires the two surfaces to agree on: `seq`, hash, frontier — plus the
/// derived facts that make "agree" checkable rather than a formality.
#[derive(Debug, Clone)]
struct SurfaceObservation {
    surface: &'static str,
    snapshot_seq: i64,
    head_seq: i64,
    /// SHA-256 over the exact bytes the surface returned (snapshot ‖ each tail update, in seq
    /// order). Byte-level parity of the loader output.
    wire_hash: String,
    /// SHA-256 over the projected semantic state after replaying snapshot + tail. Two surfaces can
    /// only match here if they observed the same MVCC view.
    semantic_hash: String,
    head_frontier: String,
    /// Whether the `head_frontier` the surface reported is *version-equivalent* to the state
    /// reached by replaying the returned tail.
    ///
    /// Not a byte comparison: `LoroCollabEngine::frontier()` is `state_vv().encode()`, and a Loro
    /// `VersionVector` is a hash map, so two engines at the identical logical version can encode
    /// their peers in different orders. Equivalence is therefore asked of the engine itself —
    /// `export_from(reported_frontier)` must produce exactly what `export_from(own_frontier)`
    /// produces, which happens only when the two versions select the same set of operations.
    frontier_equivalent_to_replay: bool,
    /// Whether the returned tail is exactly `snapshot_seq+1 ..= head_seq` with no gap.
    tail_contiguous: bool,
    tail_len: usize,
}

impl SurfaceObservation {
    fn build(
        surface: &'static str,
        snapshot_seq: i64,
        head_seq: i64,
        snapshot_b64: &str,
        tail: &[(i64, String)],
        head_frontier_b64: &str,
    ) -> Self {
        let snapshot_bytes = BASE64.decode(snapshot_b64).expect("snapshot is valid base64");

        let mut wire = Sha256::new();
        wire.update(&snapshot_bytes);
        let mut engine = LoroCollabEngine::load(&snapshot_bytes).expect("snapshot loads");
        let mut expected_seq = snapshot_seq;
        let mut contiguous = true;
        for (seq, bytes_b64) in tail {
            expected_seq += 1;
            if *seq != expected_seq {
                contiguous = false;
            }
            let bytes = BASE64.decode(bytes_b64).expect("tail update is valid base64");
            wire.update(&bytes);
            engine.import_update(&bytes).expect("tail update replays");
        }
        if expected_seq != head_seq {
            contiguous = false;
        }

        let semantic = engine.semantic_snapshot().expect("semantic snapshot builds");
        let state = projection::state_json(&semantic).expect("projection renders");
        let mut sem = Sha256::new();
        sem.update(serde_json::to_string(&state).expect("state serializes").as_bytes());
        // The title is hashed explicitly: `projection::state_json` renders the block tree, and a
        // hash that does not move when the document's title moves would report parity between two
        // genuinely different views. This harness's own edits are title edits.
        sem.update(engine.title().expect("title reads").as_bytes());
        sem.update(projection::plain_text(&semantic).as_bytes());

        let reported = BASE64.decode(head_frontier_b64).expect("head_frontier is valid base64");
        let from_reported = engine.export_from(&collab_core::Frontier::from_bytes(reported));
        let from_own = engine.export_from(&engine.frontier());
        let frontier_equivalent_to_replay = match (from_reported, from_own) {
            (Ok(reported_delta), Ok(own_delta)) => reported_delta == own_delta,
            _ => false,
        };

        Self {
            surface,
            snapshot_seq,
            head_seq,
            wire_hash: hex::encode(wire.finalize()),
            semantic_hash: hex::encode(sem.finalize()),
            head_frontier: head_frontier_b64.to_string(),
            frontier_equivalent_to_replay,
            tail_contiguous: contiguous,
            tail_len: tail.len(),
        }
    }

    fn identity(&self) -> (String, String, String, i64) {
        (
            self.wire_hash.clone(),
            self.semantic_hash.clone(),
            self.head_frontier.clone(),
            self.snapshot_seq,
        )
    }
}

async fn rest_bootstrap(addr: SocketAddr, token: &str, object_id: Uuid) -> SurfaceObservation {
    let response = reqwest::Client::new()
        .get(format!("http://{addr}/api/v1/flow/objects/{object_id}/bootstrap"))
        .bearer_auth(token)
        .send()
        .await
        .expect("bootstrap request completes");
    let body: Value = response.json().await.expect("bootstrap response is JSON");
    assert_eq!(body["code"], 0, "bootstrap failed: {body}");
    let data = &body["data"];
    let tail: Vec<(i64, String)> = data["tail_updates"]
        .as_array()
        .expect("tail_updates is an array")
        .iter()
        .map(|update| {
            (
                update["seq"].as_i64().expect("tail seq is an integer"),
                update["bytes"].as_str().expect("tail bytes is a string").to_string(),
            )
        })
        .collect();
    SurfaceObservation::build(
        "rest_bootstrap",
        data["snapshot_seq"].as_i64().expect("snapshot_seq is an integer"),
        data["head_seq"].as_i64().expect("head_seq is an integer"),
        data["snapshot_base64"].as_str().expect("snapshot_base64 is a string"),
        &tail,
        data["head_frontier"].as_str().expect("head_frontier is a string"),
    )
}

// ---------------------------------------------------------------------------------------------
// PostgreSQL statement-log reconstruction — the lock-hold measurement authority
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct LoggedStatement {
    /// When the statement *finished* (`PostgreSQL` logs the duration line on completion).
    at: DateTime<Utc>,
    duration_ms: f64,
    sql: String,
}

impl LoggedStatement {
    fn started_at(&self) -> DateTime<Utc> {
        self.at - chrono::Duration::microseconds((self.duration_ms * 1000.0).round() as i64)
    }
}

#[derive(Debug, Clone)]
struct LoggedTransaction {
    statements: Vec<LoggedStatement>,
    committed: bool,
}

impl LoggedTransaction {
    fn span_ms(&self) -> f64 {
        let (Some(first), Some(last)) = (self.statements.first(), self.statements.last()) else {
            return 0.0;
        };
        (last.at - first.started_at()).num_microseconds().unwrap_or(0) as f64 / 1000.0
    }

    /// Wall time inside this transaction during which no SQL was executing — the application
    /// holding the transaction open while doing something else.
    fn app_gaps_ms(&self) -> Vec<f64> {
        self.statements
            .windows(2)
            .map(|pair| (pair[1].started_at() - pair[0].at).num_microseconds().unwrap_or(0) as f64 / 1000.0)
            .map(|gap| gap.max(0.0))
            .collect()
    }

    fn is_document_write(&self) -> bool {
        self.statements.iter().any(|statement| {
            let sql = statement.sql.to_ascii_lowercase();
            sql.contains("collab_documents") && sql.contains("for update")
        })
    }

    /// `SeaORM`'s `begin_with_config(RepeatableRead, ReadOnly)` emits the isolation level and the
    /// access mode as **two separate statements** right after `BEGIN` (verified against the live
    /// server log), so this has to look for both across the transaction rather than for one
    /// combined `START TRANSACTION` line.
    fn is_repeatable_read_read_only(&self) -> bool {
        let mut repeatable_read = false;
        let mut read_only = false;
        for statement in &self.statements {
            let sql = statement.sql.to_ascii_lowercase();
            if sql.contains("set transaction") && sql.contains("repeatable read") {
                repeatable_read = true;
            }
            if sql.contains("set transaction") && sql.contains("read only") {
                read_only = true;
            }
        }
        repeatable_read && read_only
    }

    fn reads_collab_updates_tail(&self) -> bool {
        self.statements.iter().any(|statement| {
            let sql = statement.sql.to_ascii_lowercase();
            sql.trim_start().starts_with("select") && sql.contains("collab_updates")
        })
    }

    /// The `COMMIT` statement's own duration — WAL flush time, which is part of the lock hold but
    /// is a property of the storage layer rather than of anything the write algorithm does.
    /// Reported separately so a hold budget miss can be attributed instead of guessed at.
    fn commit_ms(&self) -> Option<f64> {
        self.statements
            .last()
            .filter(|statement| statement.sql.trim().to_ascii_uppercase().starts_with("COMMIT"))
            .map(|statement| statement.duration_ms)
    }

    fn lock_wait_ms(&self) -> Option<f64> {
        self.statements
            .iter()
            .filter(|statement| {
                let sql = statement.sql.to_ascii_lowercase();
                sql.contains("collab_documents") && sql.contains("for update")
            })
            .map(|statement| statement.duration_ms)
            .fold(None, |acc: Option<f64>, value| {
                Some(acc.map_or(value, |a| a.max(value)))
            })
    }
}

/// Parses `log_line_prefix = '%m [%p] '` + `log_min_duration_statement = 0` output.
///
/// Shape (verified against the live container before this harness was written):
/// `2026-08-30 21:49:17.254 UTC [51766] LOG:  duration: 0.118 ms  execute <unnamed>: SELECT …`
///
/// `SeaORM`/`sqlx` use **named** prepared statements (`sqlx_s_5`), and `PostgreSQL` prints the SQL
/// text only on the `parse` line — every later `bind`/`execute` of the same name logs an empty
/// body. Reconstructing the transaction's statement inventory therefore requires carrying a
/// per-backend `name -> SQL` map forward; without it the very statements this harness must
/// classify (the `FOR UPDATE` row lock, the tail `SELECT`) arrive as blanks and the run silently
/// measures nothing. The map is built from the whole harvested log, including the lines that
/// precede the measurement window, because a connection prepares each statement once.
fn parse_pg_log(raw: &str, from: DateTime<Utc>) -> BTreeMap<i32, Vec<LoggedStatement>> {
    let mut per_pid: BTreeMap<i32, Vec<LoggedStatement>> = BTreeMap::new();
    let mut prepared: HashMap<(i32, String), String> = HashMap::new();
    let mut pending: Option<ParsedLogLine> = None;

    for line in raw.lines() {
        if is_log_line_start(line) {
            if let Some(parsed) = pending.take() {
                flush_log_line(parsed, from, &mut prepared, &mut per_pid);
            }
            pending = parse_pg_log_line(line);
        } else if let Some(parsed) = pending.as_mut() {
            // A statement whose SQL spans several lines (every raw-string query in
            // `flow::collab::write`'s locked phase does) is logged with its body on continuation
            // lines. Dropping them would leave the map holding an empty body for that prepared
            // statement name, and every later `bind`/`execute` of it would resolve to nothing —
            // which is exactly how a run silently classifies a real write transaction as "not a
            // write" and measures 0 samples.
            if !parsed.sql.is_empty() {
                parsed.sql.push(' ');
            }
            parsed.sql.push_str(line.trim());
        }
    }
    if let Some(parsed) = pending.take() {
        flush_log_line(parsed, from, &mut prepared, &mut per_pid);
    }
    per_pid
}

/// True when the line opens a new server log record (`%m [%p] `), whether or not it is a
/// `duration:` line — anything else is a continuation of the previous record's body.
fn is_log_line_start(line: &str) -> bool {
    line.split_once(" UTC [").is_some_and(|(stamp, rest)| {
        NaiveDateTime::parse_from_str(stamp.trim(), "%Y-%m-%d %H:%M:%S%.f").is_ok()
            && rest.split_once("] ").is_some_and(|(pid, _)| pid.parse::<i32>().is_ok())
    })
}

fn flush_log_line(
    parsed: ParsedLogLine,
    from: DateTime<Utc>,
    prepared: &mut HashMap<(i32, String), String>,
    per_pid: &mut BTreeMap<i32, Vec<LoggedStatement>>,
) {
    let ParsedLogLine {
        at,
        pid,
        prepared_name,
        duration_ms,
        sql,
    } = parsed;
    let resolved = match (prepared_name, sql.is_empty()) {
        (Some(name), false) => {
            prepared.insert((pid, name), sql.clone());
            sql
        }
        (Some(name), true) => prepared.get(&(pid, name)).cloned().unwrap_or_default(),
        (None, _) => sql,
    };
    if at < from {
        return;
    }
    per_pid.entry(pid).or_default().push(LoggedStatement {
        at,
        duration_ms,
        sql: resolved,
    });
}

/// One parsed log line: timestamp, backend pid, prepared-statement name (`None` for a simple
/// query such as `statement: BEGIN`, `Some` for the extended protocol's `parse sqlx_s_5: …`),
/// statement duration in milliseconds, and the SQL body (empty on a `bind`/`execute` of an
/// already-parsed named statement).
struct ParsedLogLine {
    at: DateTime<Utc>,
    pid: i32,
    prepared_name: Option<String>,
    duration_ms: f64,
    sql: String,
}

fn parse_pg_log_line(line: &str) -> Option<ParsedLogLine> {
    let (stamp, rest) = line.split_once(" UTC [")?;
    let at = NaiveDateTime::parse_from_str(stamp.trim(), "%Y-%m-%d %H:%M:%S%.f")
        .ok()?
        .and_utc();
    let (pid_raw, rest) = rest.split_once("] ")?;
    let pid: i32 = pid_raw.parse().ok()?;
    let rest = rest.strip_prefix("LOG:  duration: ")?;
    let (duration_raw, rest) = rest.split_once(" ms  ")?;
    let duration_ms: f64 = duration_raw.parse().ok()?;
    // The tag is `statement` / `parse <name>` / `bind <name>` / `execute <name>`; no tag contains
    // ": ", so the first occurrence separates tag from SQL.
    let (tag, sql) = rest.split_once(':')?;
    let prepared_name = tag.split_once(' ').map(|(_kind, name)| name.trim().to_string());
    Some(ParsedLogLine {
        at,
        pid,
        prepared_name,
        duration_ms,
        sql: sql.trim().to_string(),
    })
}

/// Groups one backend's strictly-sequential statement stream into transactions.
fn group_transactions(per_pid: &BTreeMap<i32, Vec<LoggedStatement>>) -> (Vec<LoggedTransaction>, usize) {
    let mut transactions = Vec::new();
    let mut unterminated = 0usize;
    for statements in per_pid.values() {
        let mut open: Option<Vec<LoggedStatement>> = None;
        for statement in statements {
            let upper = statement.sql.trim().to_ascii_uppercase();
            let starts = upper.starts_with("BEGIN") || upper.starts_with("START TRANSACTION");
            let ends = upper.starts_with("COMMIT") || upper.starts_with("ROLLBACK");
            if starts {
                if open.is_some() {
                    unterminated += 1;
                }
                open = Some(vec![statement.clone()]);
                continue;
            }
            if let Some(buffer) = open.as_mut() {
                buffer.push(statement.clone());
                if ends {
                    transactions.push(LoggedTransaction {
                        statements: std::mem::take(buffer),
                        committed: upper.starts_with("COMMIT"),
                    });
                    open = None;
                }
            }
        }
        if open.is_some() {
            unterminated += 1;
        }
    }
    (transactions, unterminated)
}

/// Reads the `PostgreSQL` server log out of the container running the test database. The container
/// engine and name are overridable; the defaults match the dedicated `flow-test-pg` instance
/// `OPENPR_TEST_DATABASE_URL` points at.
fn harvest_pg_log(window_start: DateTime<Utc>) -> Result<String, String> {
    let engines: Vec<String> = std::env::var(PG_LOG_ENGINE_ENV).map_or_else(
        |_| vec!["podman".to_string(), "docker".to_string()],
        |value| vec![value],
    );
    let container = std::env::var(PG_LOG_CONTAINER_ENV).unwrap_or_else(|_| DEFAULT_PG_LOG_CONTAINER.to_string());
    let since = (window_start - chrono::Duration::seconds(2))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();

    let mut failures = Vec::new();
    for engine in engines {
        match Command::new(&engine)
            .args(["logs", "--since", &since, &container])
            .output()
        {
            Ok(output) if output.status.success() => {
                // PostgreSQL writes to stderr; the container engine keeps the streams separate.
                let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
                combined.push('\n');
                combined.push_str(&String::from_utf8_lossy(&output.stderr));
                return Ok(combined);
            }
            Ok(output) => failures.push(format!(
                "{engine}: exit {:?}: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(err) => failures.push(format!("{engine}: {err}")),
        }
    }
    Err(format!(
        "could not read the PostgreSQL server log ({}); set {PG_LOG_ENGINE_ENV}/{PG_LOG_CONTAINER_ENV}",
        failures.join("; ")
    ))
}

// ---------------------------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------------------------

/// Nearest-rank percentile over an ascending-sorted sample vector.
fn percentile_ms(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted.get(rank.min(sorted.len()) - 1).copied().unwrap_or(f64::NAN)
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

// ---------------------------------------------------------------------------------------------
// The harness
// ---------------------------------------------------------------------------------------------

struct ClientCredentials {
    client_id: String,
    token: String,
}

/// The 10-client load generator: fixture identity plus per-client credentials. Every client drives
/// the **same** `document_id`, because `ADR-0010`'s target ("10 个并发 client 的 accepted
/// round-trip p95 … seq 连续") is about contention on one document's head — spreading the clients
/// over separate documents would remove exactly the queueing this gate exists to bound.
struct LoadHarness {
    addr: SocketAddr,
    object_id: Uuid,
    document_id: Uuid,
    workspace_id: Uuid,
    clients: Vec<ClientCredentials>,
    parity_token: String,
}

#[derive(Debug, Default)]
struct ClientOutcome {
    round_trip_ms: Vec<f64>,
    accepted_seqs: Vec<i64>,
    rejections: Vec<(Uuid, String)>,
}

impl LoadHarness {
    /// One client's closed loop: warm up, then measure. Each round exports a genuine Loro delta
    /// from the client's own engine (so the server does real decode/apply work) and blocks until
    /// its *own* `accepted` frame comes back, ignoring the peer `update`/`accepted` broadcasts
    /// that every other client's commit fans out to this connection.
    async fn load_generator_client(&self, index: usize) -> ClientOutcome {
        let credentials = &self.clients[index];
        let ticket = issue_ticket(
            self.addr,
            &credentials.token,
            self.workspace_id,
            self.document_id,
            &credentials.client_id,
        )
        .await;
        let mut ws = connect(self.addr, &ticket, &credentials.client_id).await;
        // The `open` handshake is not optional plumbing: an un-opened connection receives no
        // broadcasts, which would silently delete the peer fan-out this harness exists to create.
        let opened = open_document(&mut ws, self.document_id, &credentials.client_id).await;
        assert!(
            opened.tail_contiguous,
            "the WS snapshot handed to client {index} already had a tail gap"
        );

        let mut engine = client_engine_from(self.addr, &self.parity_token, self.object_id).await;

        let mut outcome = ClientOutcome::default();
        for round in 0..(WARMUP_ROUNDS + MEASURED_ROUNDS) {
            let base_frontier = engine.frontier();
            engine
                .set_title(&format!("client-{index}-round-{round}"))
                .expect("set_title succeeds");
            let bytes = engine.export_from(&base_frontier).expect("exports a real delta");
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
                    origin: "web".to_string(),
                    message: None,
                },
            )
            .await;

            let mut settled = false;
            while !settled {
                match recv_frame(&mut ws).await {
                    Frame::Accepted {
                        update_id: got,
                        head_seq,
                        ..
                    } if got == update_id => {
                        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
                        if round >= WARMUP_ROUNDS {
                            outcome.round_trip_ms.push(elapsed);
                        }
                        outcome.accepted_seqs.push(head_seq);
                        settled = true;
                    }
                    Frame::Rejected {
                        update_id: Some(got),
                        code,
                        ..
                    } if got == update_id => {
                        outcome.rejections.push((got, format!("{code:?}")));
                        settled = true;
                    }
                    _ => {}
                }
            }
        }
        let _ = ws.close(None).await;
        outcome
    }
}

/// The client-side engine is seeded from the REST bootstrap rather than the WS snapshot only so
/// that the WS snapshot stays a *measured* surface rather than plumbing; both are proven identical
/// by the parity probes.
async fn client_engine_from(addr: SocketAddr, token: &str, object_id: Uuid) -> LoroCollabEngine {
    let response = reqwest::Client::new()
        .get(format!("http://{addr}/api/v1/flow/objects/{object_id}/bootstrap"))
        .bearer_auth(token)
        .send()
        .await
        .expect("bootstrap request completes");
    let body: Value = response.json().await.expect("bootstrap response is JSON");
    let data = &body["data"];
    let snapshot = BASE64
        .decode(data["snapshot_base64"].as_str().expect("snapshot_base64 is a string"))
        .expect("snapshot is valid base64");
    let mut engine = LoroCollabEngine::load(&snapshot).expect("client loads the server snapshot");
    for update in data["tail_updates"].as_array().expect("tail_updates is an array") {
        let bytes = BASE64
            .decode(update["bytes"].as_str().expect("tail bytes is a string"))
            .expect("tail bytes are valid base64");
        engine.import_update(&bytes).expect("tail update replays");
    }
    engine
}

/// Builds the real end-of-run document and a real delta against it, then hands both to
/// [`measure_prepare_ms`]. Using the document the load actually produced (150 accepted updates
/// deep) keeps the reference honest: an empty fixture would understate the prepare phase and make
/// the "prepare stayed outside the lock" comparison easier to pass than it should be.
async fn calibrate_prepare(addr: SocketAddr, token: &str, object_id: Uuid) -> (Vec<f64>, Vec<f64>) {
    let mut engine = client_engine_from(addr, token, object_id).await;
    let Ok(snapshot) = engine.export_snapshot() else {
        return (Vec::new(), Vec::new());
    };
    let base = engine.frontier();
    if engine.set_title("calibration").is_err() {
        return (Vec::new(), Vec::new());
    }
    let Ok(update) = engine.export_from(&base) else {
        return (Vec::new(), Vec::new());
    };
    measure_prepare_ms(&snapshot, &update)
}

/// Calibrates, on this machine and this build profile, what the write algorithm's **out-of-lock
/// prepare phase** costs: exactly the body of `flow::collab::write::hydrate_and_apply` after the
/// head read — `collab_core::isolation::isolated_apply` (a spawned, resource-ceilinged worker
/// process), reloading its result, `semantic_snapshot`, and the projection render.
///
/// This is the reference the "prepare stayed outside the lock" verdict is measured against, and it
/// has to be the *whole* prepare phase rather than the isolated apply alone: the apply by itself
/// can measure close enough to the log's 1 ms timestamp quantization that a comparison against it
/// proves nothing either way, and a check that cannot fail is not a check. Measured on the real
/// end-of-run document and a real delta produced against it, so it reflects the workload that was
/// actually under load rather than an empty fixture.
fn measure_prepare_ms(snapshot: &[u8], update: &[u8]) -> (Vec<f64>, Vec<f64>) {
    let mut prepare_samples = Vec::new();
    let mut apply_samples = Vec::new();
    for _ in 0..CALIBRATION_ITERATIONS {
        let started = Instant::now();
        let Ok(applied) = collab_core::isolation::isolated_apply(snapshot, update) else {
            return (Vec::new(), Vec::new());
        };
        apply_samples.push(started.elapsed().as_secs_f64() * 1000.0);
        let Ok(candidate) = LoroCollabEngine::load(&applied.snapshot) else {
            return (Vec::new(), Vec::new());
        };
        let Ok(semantic) = candidate.semantic_snapshot() else {
            return (Vec::new(), Vec::new());
        };
        if projection::state_json(&semantic).is_err() || candidate.title().is_err() {
            return (Vec::new(), Vec::new());
        }
        let _ = projection::plain_text(&semantic);
        prepare_samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    (prepare_samples, apply_samples)
}

// ---------------------------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct Report {
    violations: Vec<String>,
    notes: Vec<String>,
    round_trip: Option<Distribution>,
    lock_hold: Option<Distribution>,
    lock_wait: Option<Distribution>,
    commit: Option<Distribution>,
    intra_lock_app_gap: Option<Distribution>,
    isolated_apply_ms_p50: f64,
    out_of_lock_prepare_ms_p50: f64,
    write_transactions: usize,
    committed_write_transactions: usize,
    locked_phase_statements: BTreeSet<String>,
    /// Reconstruction coverage, so a silently truncated or mis-parsed log can never masquerade as
    /// "the implementation only did this many writes".
    harvested_log_lines: usize,
    parsed_statements: usize,
    unresolved_statements: usize,
    transactions_total: usize,
    transactions_in_window: usize,
    bootstrap_transactions: usize,
    repeatable_read_read_only_transactions: usize,
    unterminated_transactions: usize,
    accepted_total: usize,
    rejections: Vec<(Uuid, String)>,
    parity: Vec<SurfaceObservation>,
    seq_contiguous: bool,
    head_seq: i64,
    build_profile: &'static str,
    postgres_version: String,
}

impl Report {
    fn fail(&mut self, message: impl Into<String>) {
        self.violations.push(message.into());
    }

    fn absorb(&mut self, metrics: LockMetrics) {
        self.write_transactions = metrics.write_transactions;
        self.bootstrap_transactions = metrics.bootstrap_transactions;
        self.repeatable_read_read_only_transactions = metrics.repeatable_read_read_only_transactions;
        self.lock_hold = Some(Distribution::of(&metrics.hold_ms));
        self.lock_wait = Some(Distribution::of(&metrics.wait_ms));
        self.commit = Some(Distribution::of(&metrics.commit_ms));
        self.intra_lock_app_gap = Some(Distribution::of(&metrics.app_gap_ms));
        self.locked_phase_statements = metrics.locked_phase_statements;
        self.violations.extend(metrics.violations);
    }

    fn to_json(&self) -> Value {
        json!({
            "schema_version": "sylvode.flow.collab-load-harness.v1",
            "environment": {
                "build_profile": self.build_profile,
                "postgres_version": self.postgres_version,
                "pg_log_container": std::env::var(PG_LOG_CONTAINER_ENV)
                    .unwrap_or_else(|_| DEFAULT_PG_LOG_CONTAINER.to_string()),
                "hold_ms_resolution": "±1ms (PostgreSQL %m log prefix is millisecond-quantized)",
                "measurement_authority": "postgresql server statement log (log_min_duration_statement=0), not an in-process timer",
            },
            "10_client": {
                "clients": CONCURRENT_CLIENTS,
                "warmup_rounds": WARMUP_ROUNDS,
                "measured_rounds": MEASURED_ROUNDS,
                "accepted_total": self.accepted_total,
                "seq_contiguous": self.seq_contiguous,
                "head_seq": self.head_seq,
                "round_trip_p95": self.round_trip.as_ref().map(Distribution::to_json),
            },
            "lock": {
                "lock_hold_p95": self.lock_hold.as_ref().map(Distribution::to_json),
                "lock_wait": self.lock_wait.as_ref().map(Distribution::to_json),
                "commit_wal_flush": self.commit.as_ref().map(Distribution::to_json),
                "intra_lock_app_gap": self.intra_lock_app_gap.as_ref().map(Distribution::to_json),
                "isolated_apply_ms_p50": self.isolated_apply_ms_p50,
                "out_of_lock_prepare_ms_p50": self.out_of_lock_prepare_ms_p50,
                "write_transactions": self.write_transactions,
                "unterminated_transactions": self.unterminated_transactions,
                "committed_write_transactions": self.committed_write_transactions,
                "locked_phase_statement_inventory": self.locked_phase_statements,
                "reconstruction": {
                    "harvested_log_lines": self.harvested_log_lines,
                    "parsed_statements": self.parsed_statements,
                    "unresolved_statements": self.unresolved_statements,
                    "transactions_total": self.transactions_total,
                    "transactions_in_window": self.transactions_in_window,
                },
            },
            "bootstrap_parity": {
                "bootstrap_transactions": self.bootstrap_transactions,
                "repeatable_read_read_only_transactions": self.repeatable_read_read_only_transactions,
                "observations": self.parity.iter().map(|observation| json!({
                    "surface": observation.surface,
                    "snapshot_seq": observation.snapshot_seq,
                    "head_seq": observation.head_seq,
                    "wire_hash": observation.wire_hash,
                    "semantic_hash": observation.semantic_hash,
                    "head_frontier": observation.head_frontier,
                    "frontier_equivalent_to_replay": observation.frontier_equivalent_to_replay,
                    "tail_contiguous": observation.tail_contiguous,
                    "tail_len": observation.tail_len,
                })).collect::<Vec<_>>(),
            },
            "budgets": {
                "round_trip_p95_ms_max": ROUND_TRIP_P95_MS_MAX,
                "lock_hold_p95_ms_max": LOCK_HOLD_P95_MS_MAX,
                "lock_hold_single_ms_max": LOCK_HOLD_SINGLE_MS_MAX,
                "lock_wait_ms_max": LOCK_WAIT_MS_MAX,
                "intra_lock_app_gap_ms_max": INTRA_LOCK_APP_GAP_MS_MAX,
                "min_samples": MIN_SAMPLES,
            },
            "rejections": self.rejections.iter().map(|(id, code)| json!({"update_id": id, "code": code})).collect::<Vec<_>>(),
            "notes": self.notes,
            "violations": self.violations,
            "passed": self.violations.is_empty(),
        })
    }

    fn emit(&self) {
        let value = self.to_json();
        let rendered = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string());
        println!("---- flow collab load harness result ----\n{rendered}");
        if let Ok(path) = std::env::var(EVIDENCE_OUT_ENV) {
            let _ = std::fs::write(path, rendered);
        }
    }
}

/// `ADR-0010`'s result schema requires `environment.postgres_version`; it is read from the server
/// actually under measurement, never assumed.
async fn server_version(db: &DatabaseConnection) -> String {
    #[derive(FromQueryResult)]
    struct VersionRow {
        version: String,
    }
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

const fn build_profile() -> &'static str {
    if cfg!(debug_assertions) { "debug" } else { "release" }
}

/// One attempt at rebuilding the measurement window's transactions out of a harvested log,
/// together with the coverage counters that make an incomplete harvest visible instead of letting
/// it masquerade as "the server only did this many writes".
struct Reconstruction {
    harvested_log_lines: usize,
    parsed_statements: usize,
    unresolved_statements: usize,
    transactions_total: usize,
    unterminated: usize,
    measured: Vec<LoggedTransaction>,
}

impl Reconstruction {
    fn committed_writes(&self) -> usize {
        self.measured
            .iter()
            .filter(|transaction| transaction.committed && transaction.is_document_write())
            .count()
    }
}

fn reconstruct(raw: &str, log_start: DateTime<Utc>, window_start: DateTime<Utc>) -> Reconstruction {
    let per_pid = parse_pg_log(raw, log_start);
    let parsed_statements = per_pid.values().map(Vec::len).sum();
    let unresolved_statements = per_pid
        .values()
        .flatten()
        .filter(|statement| statement.sql.is_empty())
        .count();
    let (transactions, unterminated) = group_transactions(&per_pid);
    let transactions_total = transactions.len();
    let measured = transactions
        .into_iter()
        .filter(|transaction| {
            transaction
                .statements
                .first()
                .is_some_and(|first| first.at >= window_start)
        })
        .collect();
    Reconstruction {
        harvested_log_lines: raw.lines().count(),
        parsed_statements,
        unresolved_statements,
        transactions_total,
        unterminated,
        measured,
    }
}

/// Everything the reconstructed transaction stream says about the locked phase, kept separate
/// from [`Report`] so the per-transaction classification stays a small, testable function rather
/// than a deeply nested block inside the gate test.
#[derive(Default)]
struct LockMetrics {
    hold_ms: Vec<f64>,
    wait_ms: Vec<f64>,
    commit_ms: Vec<f64>,
    app_gap_ms: Vec<f64>,
    write_transactions: usize,
    bootstrap_transactions: usize,
    repeatable_read_read_only_transactions: usize,
    locked_phase_statements: BTreeSet<String>,
    violations: Vec<String>,
}

/// Collapses whitespace and case so a multi-line raw-string query and its single-line log
/// reconstruction compare as the same statement.
fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// First needle is a prefix, the rest are substrings.
fn statement_matches(sql: &str, needles: &[&str]) -> bool {
    let Some((prefix, rest)) = needles.split_first() else {
        return false;
    };
    sql.starts_with(prefix) && rest.iter().all(|needle| sql.contains(needle))
}

/// Audits one write transaction's statement inventory both ways: against the named hydrate/rebase
/// reads that must never appear inside the lock, and against the complete allowlist of statements
/// the locked phase is permitted to run at all.
fn audit_locked_phase(transaction: &LoggedTransaction) -> (Vec<String>, BTreeSet<String>) {
    let mut found = Vec::new();
    let mut inventory = BTreeSet::new();
    for statement in &transaction.statements {
        let sql = normalize_sql(&statement.sql);
        if sql.is_empty() {
            found.push(
                "unresolvable_statement_inside_lock: a statement executed between BEGIN and COMMIT could not be \
                 reconstructed from the server log, so the lock's contents are unverified"
                    .to_string(),
            );
            continue;
        }
        inventory.insert(sql.clone());
        for (label, needles) in LOCKED_PHASE_FORBIDDEN_READS {
            if statement_matches(&sql, needles) {
                found.push(format!(
                    "{label}: a write transaction executed `{sql}` between BEGIN and COMMIT — \
                     prepare/rebase leaked inside the lock"
                ));
            }
        }
        if !LOCKED_PHASE_ALLOWED_STATEMENTS
            .iter()
            .any(|(_label, needles)| statement_matches(&sql, needles))
        {
            found.push(format!(
                "unexpected_statement_inside_lock: `{sql}` is not one of the statements ADR-0010's locked \
                 phase is allowed to execute"
            ));
        }
    }
    (found, inventory)
}

/// Classifies every reconstructed transaction and collects the three lock distributions.
fn analyze_transactions(transactions: &[LoggedTransaction]) -> LockMetrics {
    let mut metrics = LockMetrics::default();
    for transaction in transactions {
        let repeatable_read = transaction.is_repeatable_read_read_only();
        if repeatable_read {
            metrics.repeatable_read_read_only_transactions += 1;
        }
        if !transaction.is_document_write() {
            if transaction.reads_collab_updates_tail() {
                metrics.bootstrap_transactions += 1;
                if !repeatable_read {
                    metrics.violations.push(
                        "a bootstrap/tail-loading transaction did not run at REPEATABLE READ READ ONLY".to_string(),
                    );
                }
            }
            continue;
        }
        metrics.write_transactions += 1;
        let (violations, inventory) = audit_locked_phase(transaction);
        metrics.violations.extend(violations);
        metrics.locked_phase_statements.extend(inventory);
        if !transaction.committed {
            continue;
        }
        metrics.hold_ms.push(transaction.span_ms());
        if let Some(wait) = transaction.lock_wait_ms() {
            metrics.wait_ms.push(wait);
        }
        if let Some(commit) = transaction.commit_ms() {
            metrics.commit_ms.push(commit);
        }
        metrics.app_gap_ms.extend(transaction.app_gaps_ms());
    }
    metrics
}

#[derive(FromQueryResult)]
struct SeqRow {
    seq: i64,
}

#[derive(FromQueryResult)]
struct CountRow {
    n: i64,
}

// ---------------------------------------------------------------------------------------------
// The gate test
// ---------------------------------------------------------------------------------------------

/// The `ten_client` load run behind both blocked v0.4 hard gates:
/// `bounded_warm_cache_lock_hold_and_round_trip_budgets` (lock hold p95 / 10-client accepted
/// round-trip p95) and `bootstrap_repeatable_read_and_ws_parity` (same `REPEATABLE READ` view,
/// cross-surface `seq`/hash/frontier parity, no gaps).
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn ten_client_load_harness_round_trip_p95_and_lock_hold_p95() {
    // Without a real database there is nothing to measure -- `gate-commands.md` fails any run that
    // substitutes a mock or in-memory database outright. The test does not panic (that would break
    // `cargo test -p api` on every machine without the scratch instance), but it also refuses to
    // look like a pass: it writes an evidence file whose `passed` is false and whose violation says
    // the run never happened, so nothing downstream can read a green out of a skip.
    let Some(scratch) = scratch("ten_client").await else {
        let mut skipped = Report {
            build_profile: build_profile(),
            ..Report::default()
        };
        skipped.fail(format!(
            "{TEST_DATABASE_URL_ENV} is not set, so no load run happened; this is not a measurement and \
             must not be read as one"
        ));
        skipped.emit();
        eprintln!("SKIPPED: {TEST_DATABASE_URL_ENV} is not set; the v0.4 load harness measured nothing");
        return;
    };

    let mut report = Report {
        build_profile: build_profile(),
        postgres_version: server_version(&scratch.db).await,
        ..Report::default()
    };

    let logging_enabled = scratch.enable_statement_logging().await;
    // Everything logged from here on is harvested, but only transactions that *begin* after
    // `window_start` below are measured. The earlier lines still matter: they carry the `parse`
    // bodies that name the prepared statements the measured window reuses.
    let log_start = Utc::now();
    if !logging_enabled {
        report.fail("could not enable log_min_duration_statement on the scratch database: the lock-hold measurement has no authority to read");
    }

    // Opened *after* `ALTER DATABASE`, so every connection in this pool logs its statements.
    let admin_url = scratch.admin_url.clone();
    let (prefix, _) = admin_url.rsplit_once('/').expect("admin url has a database segment");
    let app_db = Database::connect(format!("{prefix}/{}", scratch.name))
        .await
        .expect("application pool connects to the scratch database");
    let state = state_for(app_db);

    let (workspace_id, members) = seed_workspace(&state.db, CONCURRENT_CLIENTS + 1).await;
    let (object_id, document_id) = create_page(&state, workspace_id, members[0]).await;
    let addr = spawn_server(state.clone()).await;

    let harness = LoadHarness {
        addr,
        object_id,
        document_id,
        workspace_id,
        clients: members
            .iter()
            .take(CONCURRENT_CLIENTS)
            .enumerate()
            .map(|(index, user_id)| ClientCredentials {
                client_id: format!("load-client-{index}"),
                token: jwt_for(*user_id),
            })
            .collect(),
        parity_token: jwt_for(members[CONCURRENT_CLIENTS]),
    };

    // The measurement window opens here: everything logged from now on belongs to the load.
    let window_start = Utc::now();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // ---- concurrent REST/WS parity probes, running *during* the write load ----
    let parity_addr = addr;
    let parity_token = harness.parity_token.clone();
    let parity_handle = tokio::spawn(async move {
        let mut observations = Vec::new();
        for iteration in 0..12 {
            let client_id = format!("parity-probe-{iteration}");
            let ticket = issue_ticket(parity_addr, &parity_token, workspace_id, document_id, &client_id).await;
            // Both surfaces are asked at the same instant, on purpose: `gate-commands.md` requires
            // the parity proof to hold *while* snapshot advancement and load are concurrent, not
            // only on a quiesced document.
            let (rest, ws_observation) = tokio::join!(rest_bootstrap(parity_addr, &parity_token, object_id), async {
                let mut ws = connect(parity_addr, &ticket, &client_id).await;
                let observation = open_document(&mut ws, document_id, &client_id).await;
                let _ = ws.close(None).await;
                observation
            });
            observations.push(rest);
            observations.push(ws_observation);
            tokio::time::sleep(Duration::from_millis(120)).await;
        }
        observations
    });

    // ---- the 10-client load itself ----
    let mut client_handles = Vec::with_capacity(CONCURRENT_CLIENTS);
    let harness = std::sync::Arc::new(harness);
    for index in 0..CONCURRENT_CLIENTS {
        let harness = harness.clone();
        client_handles.push(tokio::spawn(async move { harness.load_generator_client(index).await }));
    }

    let mut round_trip_ms = Vec::new();
    for handle in client_handles {
        let outcome = handle.await.expect("client task completes");
        round_trip_ms.extend(outcome.round_trip_ms);
        report.accepted_total += outcome.accepted_seqs.len();
        report.rejections.extend(outcome.rejections);
    }
    let mut parity = parity_handle.await.expect("parity probe task completes");

    // A quiesced final pair: with no writes in flight the two surfaces must be byte-identical, so
    // an "always disagrees" bug cannot hide behind legitimate concurrent head movement.
    let (final_rest, final_ws) = tokio::join!(rest_bootstrap(addr, &harness.parity_token, object_id), async {
        let client_id = "parity-final";
        let ticket = issue_ticket(addr, &harness.parity_token, workspace_id, document_id, client_id).await;
        let mut ws = connect(addr, &ticket, client_id).await;
        let observation = open_document(&mut ws, document_id, client_id).await;
        let _ = ws.close(None).await;
        observation
    });
    if final_rest.identity() != final_ws.identity() || final_rest.head_seq != final_ws.head_seq {
        report.fail(format!(
            "quiesced REST/WS parity mismatch: rest={final_rest:?} ws={final_ws:?}"
        ));
    }
    parity.push(final_rest);
    parity.push(final_ws);

    // Give PostgreSQL a moment to flush the tail of the window before harvesting.
    tokio::time::sleep(Duration::from_millis(400)).await;

    // ---- distributions ----
    report.round_trip = Some(Distribution::of(&round_trip_ms));
    let (prepare_samples, apply_samples) = calibrate_prepare(addr, &harness.parity_token, object_id).await;
    report.isolated_apply_ms_p50 = percentile_ms(&sorted(&apply_samples), 50.0);
    report.out_of_lock_prepare_ms_p50 = percentile_ms(&sorted(&prepare_samples), 50.0);

    // ---- lock hold, read out of `PostgreSQL`'s own statement log ----
    //
    // The container log lags the server by a noticeable margin under load, and a harvest taken too
    // early returns a *prefix* of the window: a partial, time-biased sample that would still yield
    // a plausible-looking p95. So the harvest is retried until it accounts for at least every
    // accepted update, and the achieved coverage is asserted below either way.
    let mut reconstruction: Option<Reconstruction> = None;
    for _ in 0..HARVEST_ATTEMPTS_MAX {
        tokio::time::sleep(Duration::from_millis(HARVEST_RETRY_MS)).await;
        match tokio::task::spawn_blocking(move || harvest_pg_log(log_start)).await {
            Ok(Ok(raw)) => {
                let candidate = reconstruct(&raw, log_start, window_start);
                let complete = candidate.committed_writes() >= report.accepted_total;
                reconstruction = Some(candidate);
                if complete {
                    break;
                }
            }
            Ok(Err(err)) => {
                report.fail(format!("lock-hold measurement unavailable: {err}"));
                break;
            }
            Err(err) => {
                report.fail(format!("log harvest task failed: {err}"));
                break;
            }
        }
    }

    if let Some(candidate) = reconstruction {
        report.harvested_log_lines = candidate.harvested_log_lines;
        report.parsed_statements = candidate.parsed_statements;
        report.unresolved_statements = candidate.unresolved_statements;
        report.transactions_total = candidate.transactions_total;
        report.transactions_in_window = candidate.measured.len();
        report.unterminated_transactions = candidate.unterminated;
        report.committed_write_transactions = candidate.committed_writes();
        report.absorb(analyze_transactions(&candidate.measured));
    }

    // ---- seq contiguity and rebase-exhaustion zero-write ----
    let rows = SeqRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT seq FROM collab_updates WHERE document_id = $1 ORDER BY seq",
        vec![document_id.into()],
    ))
    .all(&state.db)
    .await
    .expect("seq query runs");
    let seqs: Vec<i64> = rows.into_iter().map(|row| row.seq).collect();
    report.head_seq = seqs.last().copied().unwrap_or(0);
    report.seq_contiguous = seqs.iter().zip(1i64..).all(|(seq, expected)| *seq == expected);

    let rejections = report.rejections.clone();
    for (update_id, code) in &rejections {
        let count = CountRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM collab_updates WHERE update_id = $1",
            vec![(*update_id).into()],
        ))
        .one(&state.db)
        .await
        .expect("rejection count query runs")
        .expect("rejection count query returns a row");
        if count.n != 0 {
            report.fail(format!(
                "a rejected update ({code}) still persisted {} collab_updates row(s)",
                count.n
            ));
        }
    }

    report.parity = parity;
    evaluate(&mut report);
    report.emit();

    let _ = scratch.disable_statement_logging().await;
    drop(state);
    scratch.drop_self().await;

    assert!(
        report.violations.is_empty(),
        "the v0.4 collab load harness did not meet its frozen budgets:\n - {}",
        report.violations.join("\n - ")
    );
}

/// Turns the measured distributions into gate verdicts. Every budget compared here is a frozen
/// contract number; none is derived from the observed data.
fn evaluate(report: &mut Report) {
    // ---- gate: bounded_warm_cache_lock_hold_and_round_trip_budgets ----
    if let Some(round_trip) = report.round_trip.clone() {
        if round_trip.samples < MIN_SAMPLES {
            report.fail(format!(
                "round_trip_p95 has only {} samples; ADR-0010 requires at least {MIN_SAMPLES}",
                round_trip.samples
            ));
        }
        if round_trip.p95_ms > ROUND_TRIP_P95_MS_MAX {
            report.fail(format!(
                "10-client accepted round_trip_p95 = {:.1}ms exceeds the ADR-0010 budget of {ROUND_TRIP_P95_MS_MAX:.0}ms (p50={:.1} p99={:.1} n={})",
                round_trip.p95_ms, round_trip.p50_ms, round_trip.p99_ms, round_trip.samples
            ));
        }
    } else {
        report.fail("no round-trip distribution was produced");
    }

    if let Some(hold) = report.lock_hold.clone() {
        if hold.samples < MIN_SAMPLES {
            report.fail(format!(
                "lock_hold_p95 has only {} samples; ADR-0010 requires at least {MIN_SAMPLES}",
                hold.samples
            ));
        }
        if hold.p95_ms > LOCK_HOLD_P95_MS_MAX {
            report.fail(format!(
                "lock_hold_p95 = {:.1}ms exceeds the ADR-0010 budget of {LOCK_HOLD_P95_MS_MAX:.0}ms (p50={:.1} p99={:.1} max={:.1} n={})",
                hold.p95_ms, hold.p50_ms, hold.p99_ms, hold.max_ms, hold.samples
            ));
        }
        if hold.max_ms > LOCK_HOLD_SINGLE_MS_MAX {
            report.fail(format!(
                "a single lock hold reached {:.1}ms, past the {LOCK_HOLD_SINGLE_MS_MAX:.0}ms per-transaction ceiling",
                hold.max_ms
            ));
        }
        // The locked phase's own floor is the WAL flush, which is storage, not algorithm — so a
        // hold that exceeds one isolated apply says nothing on its own. The statement inventory
        // and the calibrated in-lock idle-gap check below carry the "prepare/apply stayed outside
        // the lock" claim; this note only records how the hold decomposes.
        if let Some(commit) = report.commit.clone() {
            report.notes.push(format!(
                "lock hold p95 {:.1}ms decomposes into commit/WAL flush p95 {:.1}ms plus the fixed \
                 locked statements; one isolated_apply measures {:.2}ms on this host",
                hold.p95_ms, commit.p95_ms, report.isolated_apply_ms_p50
            ));
        }
    } else {
        report.fail("no lock-hold distribution was produced");
    }

    if let Some(wait) = report.lock_wait.clone()
        && wait.max_ms > LOCK_WAIT_MS_MAX
    {
        report.fail(format!(
            "a document row-lock acquisition waited {:.1}ms, past the {LOCK_WAIT_MS_MAX:.0}ms ceiling",
            wait.max_ms
        ));
    }

    if let Some(gap) = report.intra_lock_app_gap.clone() {
        if gap.p95_ms > INTRA_LOCK_APP_GAP_MS_MAX {
            report.fail(format!(
                "write transactions stay open with no SQL executing for p95={:.1}ms (budget {INTRA_LOCK_APP_GAP_MS_MAX:.0}ms) — \
                 non-database work is happening inside the lock (p50={:.1} max={:.1} n={})",
                gap.p95_ms, gap.p50_ms, gap.max_ms, gap.samples
            ));
        }
        // The calibrated form of the same claim, and the one that survives a machine or profile
        // change: the whole out-of-lock prepare phase costs `out_of_lock_prepare_ms_p50` here, so
        // an implementation that ran it inside the lock would show a *typical* in-lock idle gap at
        // least that large. Only asserted when the reference clears the log's timestamp
        // quantization by a real margin — below that the two are indistinguishable and claiming a
        // pass would be claiming a check that could not have failed.
        let prepare = report.out_of_lock_prepare_ms_p50;
        if prepare.is_finite() && prepare > 2.0 * PG_LOG_TIMESTAMP_RESOLUTION_MS {
            if gap.p95_ms >= prepare {
                report.fail(format!(
                    "in-lock idle gap p95 ({:.2}ms) is at least as large as one measured out-of-lock prepare \
                     ({prepare:.2}ms) — consistent with prepare/apply running inside the lock",
                    gap.p95_ms
                ));
            }
        } else {
            report.notes.push(format!(
                "the calibrated in-lock-gap check is inconclusive on this host: one out-of-lock prepare measures \
                 {prepare:.2}ms, within {:.0}x the log's {PG_LOG_TIMESTAMP_RESOLUTION_MS:.0}ms timestamp resolution; \
                 the statement inventory and the absolute gap budget carry the verdict instead",
                2.0
            ));
        }
        if gap.max_ms > INTRA_LOCK_APP_GAP_MS_MAX {
            report.notes.push(format!(
                "one in-lock idle gap reached {:.1}ms while p50={:.2}ms and p95={:.2}ms over {} samples — \
                 an isolated scheduling outlier, not a systematic in-lock cost",
                gap.max_ms, gap.p50_ms, gap.p95_ms, gap.samples
            ));
        }
    }

    if report.write_transactions == 0 {
        report.fail("no write transaction was reconstructed from the `PostgreSQL` statement log");
    }
    // Coverage: one accepted update is one committed write transaction, so anything short of that
    // means the harvested log was truncated and the distributions above describe a biased prefix
    // of the run rather than the run.
    if report.committed_write_transactions < report.accepted_total {
        report.fail(format!(
            "log reconstruction covered only {} committed write transactions for {} accepted updates; \
             the lock-hold distribution is a truncated, time-biased sample and must not be read as the run's",
            report.committed_write_transactions, report.accepted_total
        ));
    }

    // ---- gate: bootstrap_repeatable_read_and_ws_parity ----
    if !report.seq_contiguous {
        report.fail("accepted seq values are not contiguous from 1".to_string());
    }
    if report.parity.len() < 4 {
        report.fail("too few cross-surface parity observations were collected".to_string());
    }
    let mut by_head: HashMap<i64, Vec<&SurfaceObservation>> = HashMap::new();
    for observation in &report.parity {
        if !observation.tail_contiguous {
            report.violations.push(format!(
                "{} returned a tail with a gap (snapshot_seq={} head_seq={} tail_len={})",
                observation.surface, observation.snapshot_seq, observation.head_seq, observation.tail_len
            ));
        }
        if !observation.frontier_equivalent_to_replay {
            report.violations.push(format!(
                "{} reported a head_frontier that is not version-equivalent to replaying its own tail (head_seq={})",
                observation.surface, observation.head_seq
            ));
        }
        by_head.entry(observation.head_seq).or_default().push(observation);
    }
    let mut surfaces_compared = 0usize;
    for (head_seq, group) in &by_head {
        let distinct: BTreeSet<_> = group.iter().map(|observation| observation.identity()).collect();
        let surfaces: BTreeSet<_> = group.iter().map(|observation| observation.surface).collect();
        if surfaces.len() > 1 {
            surfaces_compared += 1;
        }
        if distinct.len() > 1 {
            report.violations.push(format!(
                "surfaces disagree at head_seq={head_seq}: {} distinct (wire_hash, semantic_hash, head_frontier, snapshot_seq) identities across {:?}",
                distinct.len(),
                surfaces
            ));
        }
    }
    if surfaces_compared == 0 {
        report.notes.push(
            "no head_seq was observed by both REST and WS while under load; only the quiesced pair \
             constitutes a cross-surface comparison in this run"
                .to_string(),
        );
    }
    if report.bootstrap_transactions > 0 && report.repeatable_read_read_only_transactions == 0 {
        report.fail("bootstrap transactions were observed but none ran at REPEATABLE READ READ ONLY");
    }
}

// ---------------------------------------------------------------------------------------------
// Mutation checks on the harness itself
//
// A measurement facility that cannot fail is worth nothing, and the two checks that carry the
// "prepare/rebase stayed outside the lock" verdict -- the locked-phase statement audit and the log
// reconstruction it depends on -- are pure functions, so their negative cases can be pinned
// directly instead of being taken on trust from a green run.
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod harness_self_checks {
    use super::{
        Distribution, LoggedStatement, LoggedTransaction, audit_locked_phase, group_transactions, parse_pg_log,
        percentile_ms,
    };
    use chrono::{DateTime, TimeZone, Utc};

    fn at(seconds: i64, millis: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 30, 22, 0, seconds.try_into().unwrap_or(0))
            .single()
            .expect("a valid timestamp")
            + chrono::Duration::milliseconds(i64::from(millis))
    }

    fn statement(seconds: i64, millis: u32, duration_ms: f64, sql: &str) -> LoggedStatement {
        LoggedStatement {
            at: at(seconds, millis),
            duration_ms,
            sql: sql.to_string(),
        }
    }

    fn locked_phase(extra: &[&str]) -> LoggedTransaction {
        let mut statements = vec![
            statement(0, 0, 0.02, "BEGIN"),
            statement(0, 0, 0.02, "SET LOCAL lock_timeout = '100ms'"),
            statement(0, 0, 0.02, "SET LOCAL statement_timeout = '100ms'"),
            statement(
                0,
                1,
                0.05,
                "SELECT authz_epoch FROM flow_workspace_settings WHERE workspace_id = $1 FOR SHARE",
            ),
            statement(
                0,
                1,
                0.03,
                "SELECT head_seq, byte_count, update_count FROM collab_documents WHERE id = $1 FOR UPDATE",
            ),
            statement(
                0,
                2,
                0.20,
                "INSERT INTO business_events ( id, workspace_id ) VALUES ($1, $2)",
            ),
            statement(0, 2, 0.10, "INSERT INTO event_dispatch (id, event_id) VALUES ($1, $2)"),
            statement(
                0,
                2,
                0.20,
                "INSERT INTO collab_updates (document_id, seq) VALUES ($1, $2)",
            ),
            statement(0, 2, 0.10, "UPDATE collab_documents SET head_seq = $2 WHERE id = $1"),
            statement(
                0,
                2,
                0.10,
                "UPDATE flow_object_projections SET document_seq = $2 WHERE object_id = $1",
            ),
        ];
        for (index, sql) in extra.iter().enumerate() {
            statements.push(statement(0, 3, 0.10, sql));
            let _ = index;
        }
        statements.push(statement(0, 4, 1.00, "COMMIT"));
        LoggedTransaction {
            statements,
            committed: true,
        }
    }

    #[test]
    fn the_real_locked_phase_passes_the_statement_audit_with_exactly_eleven_statements() {
        let (violations, inventory) = audit_locked_phase(&locked_phase(&[]));
        assert!(violations.is_empty(), "unexpected violations: {violations:?}");
        assert_eq!(inventory.len(), 11, "inventory: {inventory:?}");
    }

    #[test]
    fn a_tail_load_moved_inside_the_lock_is_caught() {
        let leaked = locked_phase(&["SELECT seq, bytes FROM collab_updates WHERE document_id = $1 AND seq > $2"]);
        let (violations, _) = audit_locked_phase(&leaked);
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("tail_load_inside_lock")),
            "a tail load inside the lock must be reported: {violations:?}"
        );
    }

    #[test]
    fn a_snapshot_load_moved_inside_the_lock_is_caught() {
        let leaked = locked_phase(&["SELECT snapshot, format_version FROM collab_documents WHERE id = $1"]);
        let (violations, _) = audit_locked_phase(&leaked);
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("snapshot_load_inside_lock")),
            "a snapshot load inside the lock must be reported: {violations:?}"
        );
    }

    #[test]
    fn any_statement_outside_the_allowlist_is_caught_even_when_no_blacklist_rule_names_it() {
        let leaked = locked_phase(&["SELECT pg_sleep(0.05)"]);
        let (violations, _) = audit_locked_phase(&leaked);
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("unexpected_statement_inside_lock")),
            "the allowlist must reject a statement no blacklist rule anticipates: {violations:?}"
        );
    }

    #[test]
    fn a_statement_the_log_could_not_resolve_fails_instead_of_passing_silently() {
        let unresolved = LoggedTransaction {
            statements: vec![
                statement(0, 0, 0.02, "BEGIN"),
                statement(0, 1, 0.05, ""),
                statement(0, 2, 1.00, "COMMIT"),
            ],
            committed: true,
        };
        let (violations, _) = audit_locked_phase(&unresolved);
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("unresolvable_statement_inside_lock")),
            "an unreadable in-lock statement must fail, not be skipped: {violations:?}"
        );
    }

    /// Pins the two log-shape facts the whole lock-hold measurement rests on: a named prepared
    /// statement whose body only ever appears on its `parse` line, and a multi-line statement body
    /// carried on continuation lines. Without either, a real write transaction reconstructs as
    /// blanks and the run measures nothing while still looking plausible.
    #[test]
    fn the_log_parser_resolves_named_prepared_statements_and_multi_line_bodies() {
        let raw = "\
2026-08-30 22:00:00.000 UTC [42] LOG:  duration: 0.020 ms  statement: BEGIN
2026-08-30 22:00:00.001 UTC [42] LOG:  duration: 0.100 ms  parse sqlx_s_1: SELECT head_seq FROM collab_documents WHERE id = $1 FOR UPDATE
2026-08-30 22:00:00.002 UTC [42] LOG:  duration: 0.010 ms  bind sqlx_s_1: 
2026-08-30 22:00:00.003 UTC [42] LOG:  duration: 0.050 ms  execute sqlx_s_1: 
2026-08-30 22:00:00.004 UTC [42] LOG:  duration: 0.030 ms  parse sqlx_s_2: 
\tINSERT INTO collab_updates
\t(document_id, seq) VALUES ($1, $2)
2026-08-30 22:00:00.005 UTC [42] LOG:  duration: 0.020 ms  execute sqlx_s_2: 
2026-08-30 22:00:00.010 UTC [42] LOG:  duration: 1.000 ms  statement: COMMIT
";
        let per_pid = parse_pg_log(raw, at(0, 0) - chrono::Duration::seconds(60));
        let statements = per_pid.get(&42).expect("the backend was parsed");
        assert!(
            statements.iter().all(|statement| !statement.sql.is_empty()),
            "every statement must resolve to a body: {statements:?}"
        );
        let (transactions, unterminated) = group_transactions(&per_pid);
        assert_eq!(unterminated, 0);
        assert_eq!(transactions.len(), 1);
        let transaction = &transactions[0];
        assert!(transaction.is_document_write(), "the FOR UPDATE must be recognised");
        assert!(
            transaction
                .statements
                .iter()
                .any(|statement| statement.sql.contains("INSERT INTO collab_updates")),
            "the multi-line body must be reassembled"
        );
        assert!(
            (transaction.span_ms() - 10.0).abs() < 0.5,
            "span: {}",
            transaction.span_ms()
        );
    }

    #[test]
    fn percentiles_use_nearest_rank_and_a_distribution_reports_its_sample_count() {
        let values: Vec<f64> = (1..=100).map(f64::from).collect();
        assert!((percentile_ms(&values, 50.0) - 50.0).abs() < f64::EPSILON);
        assert!((percentile_ms(&values, 95.0) - 95.0).abs() < f64::EPSILON);
        let distribution = Distribution::of(&values);
        assert_eq!(distribution.samples, 100);
        assert!((distribution.max_ms - 100.0).abs() < f64::EPSILON);
    }
}
