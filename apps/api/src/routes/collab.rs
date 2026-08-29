//! HTTP handlers for the four Collab endpoints (`rest-api-v1.md` "v0.4 Flow Alpha"):
//!
//! ```text
//! POST /api/v1/collab/tickets
//! GET  /api/v1/collab/ws
//! GET  /api/v1/flow/objects/{object_id}/collab
//! POST /api/v1/flow/objects/{object_id}/collab/verify
//! ```

#![allow(clippy::items_after_statements, clippy::too_long_first_doc_paragraph)]

use axum::extract::ws::WebSocketUpgrade;
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::HeaderMap,
    response::IntoResponse,
};
use chrono::Utc;
use platform::{app::AppState, auth::JwtClaims};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::ApiError;
use crate::flow::collab::{bootstrap, session, ticket};
use crate::middleware::bot_auth::BotAuthContext;
use crate::response::ApiResponse;

fn build_auth_extensions(claims: JwtClaims, bot: Option<Extension<BotAuthContext>>) -> axum::http::Extensions {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }
    extensions
}

/// Percent-encodes a query component (unreserved set only: `ALPHA / DIGIT / "-" / "." / "_" /
/// "~"`). No `url`/`percent-encoding` crate is a workspace dependency; this is the one query value
/// (`client_id`) this package ever needs to embed verbatim into a URL.
fn percent_encode(raw: &str) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

#[derive(Debug, Deserialize)]
pub struct CreateTicketRequest {
    pub workspace_id: Uuid,
    pub document_id: Uuid,
    pub client_id: String,
    pub origin: String,
}

#[derive(Debug, Serialize)]
pub struct CreateTicketResponse {
    pub ticket: String,
    pub expires_at: String,
    pub websocket_url: String,
}

/// `POST /api/v1/collab/tickets` (`ADR-0007`): user access token only, bot tokens rejected.
pub async fn create_ticket(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Json(req): Json<CreateTicketRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if bot.is_some() {
        return Err(ApiError::Forbidden(
            "bot tokens cannot open a direct collab session".to_string(),
        ));
    }
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let issued = ticket::issue(
        &state.db,
        ticket::IssueTicketInput {
            user_id,
            workspace_id: req.workspace_id,
            document_id: req.document_id,
            client_id: req.client_id.clone(),
            origin: req.origin,
        },
        &state.cfg.collab_allowed_origins,
    )
    .await?;

    let websocket_url = format!(
        "/api/v1/collab/ws?ticket={}&client_id={}",
        percent_encode(&issued.ticket),
        percent_encode(&req.client_id)
    );

    Ok(ApiResponse::success(CreateTicketResponse {
        ticket: issued.ticket,
        expires_at: issued.expires_at.to_rfc3339(),
        websocket_url,
    }))
}

#[derive(Debug, Deserialize)]
pub struct WsQuery {
    pub ticket: String,
    pub client_id: String,
}

/// `GET /api/v1/collab/ws?ticket=...&client_id=...`: WebSocket upgrade, ticket-only auth
/// (`ADR-0007`) — no `bot_or_user_auth_middleware` on this route, matching the contract's "no
/// long-lived JWT/bot token over WS" rule.
pub async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(params): Query<WsQuery>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();

    ticket::consume(&state.db, &params.ticket, &params.client_id, &origin)
        .await
        .map_or_else(
            |_| ApiError::Unauthorized("invalid ticket".to_string()).into_response(),
            |consumed| {
                ws.on_upgrade(move |socket| session::run(socket, state, consumed))
                    .into_response()
            },
        )
}

#[derive(Debug, Serialize)]
pub struct CollabDiagnostics {
    pub document_id: Uuid,
    pub engine: String,
    pub format_version: String,
    pub snapshot_seq: i64,
    pub head_seq: i64,
    pub frontier: String,
    pub update_count: i64,
    pub byte_size: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_compacted_at: Option<String>,
    pub projection_seq: i64,
    pub integrity_state: String,
}

/// `GET /api/v1/flow/objects/{object_id}/collab`: diagnostic info, never raw bytes.
pub async fn get_collab_diagnostics(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let workspace_id = crate::flow::repository::fetch_object_workspace(&state.db, object_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    crate::flow::policy::require_flow_workspace_access(&state, &extensions, workspace_id).await?;

    #[derive(sea_orm::FromQueryResult)]
    struct Row {
        id: Uuid,
        engine: String,
        format_version: String,
        snapshot_seq: i64,
        head_seq: i64,
        head_frontier: Vec<u8>,
        update_count: i64,
        byte_count: i64,
    }
    use base64::Engine as _;
    use sea_orm::{DbBackend, FromQueryResult, Statement};

    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, engine, format_version, snapshot_seq, head_seq, head_frontier, update_count, byte_count \
         FROM collab_documents WHERE object_id = $1",
        vec![object_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("collab document not found".to_string()))?;

    #[derive(sea_orm::FromQueryResult)]
    struct ProjectionRow {
        document_seq: i64,
    }
    let projection_seq = ProjectionRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT document_seq FROM flow_object_projections WHERE object_id = $1",
        vec![object_id.into()],
    ))
    .one(&state.db)
    .await?
    .map_or(0, |r| r.document_seq);

    #[derive(sea_orm::FromQueryResult)]
    struct IntegrityRow {
        open_count: i64,
    }
    let integrity_state = IntegrityRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT count(*) AS open_count FROM flow_integrity_records \
         WHERE workspace_id = $1 AND subject_kind = 'collab_document' AND subject_id = $2 AND status = 'open'",
        vec![workspace_id.into(), row.id.to_string().into()],
    ))
    .one(&state.db)
    .await?
    .map_or_else(
        || "ok".to_string(),
        |r| {
            if r.open_count > 0 {
                "open_issues".to_string()
            } else {
                "ok".to_string()
            }
        },
    );

    Ok(ApiResponse::success(CollabDiagnostics {
        document_id: row.id,
        engine: row.engine,
        format_version: row.format_version,
        snapshot_seq: row.snapshot_seq,
        head_seq: row.head_seq,
        frontier: base64::engine::general_purpose::STANDARD.encode(&row.head_frontier),
        update_count: row.update_count,
        byte_size: row.byte_count,
        last_compacted_at: None,
        projection_seq,
        integrity_state,
    }))
}

#[derive(Debug, Deserialize)]
pub struct VerifyRequest {
    pub expected_head_seq: Option<i64>,
    #[serde(default)]
    pub deep: bool,
    #[allow(dead_code)] // part of the frozen request shape; v0.4 has no idempotent-replay path to key on yet
    pub idempotency_key: String,
}

#[derive(Debug, Serialize)]
pub struct OperationReceipt {
    pub operation_id: Uuid,
    pub mode: String,
    pub status: String,
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_head_seq: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_head_seq: Option<i64>,
    pub changes: Vec<Value>,
    pub warnings: Vec<String>,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audit_event_id: Option<Uuid>,
}

/// `POST /api/v1/flow/objects/{object_id}/collab/verify`: v0.4 ships only the shallow,
/// read-only check (`deep=false`); `deep=true` is rejected as `invalid_update` rather than
/// silently downgraded, since a caller asking for a deep verify and quietly getting a shallow one
/// is a worse failure mode than an explicit rejection.
pub async fn verify_collab(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object_id): Path<Uuid>,
    Json(req): Json<VerifyRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let workspace_id = crate::flow::repository::fetch_object_workspace(&state.db, object_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("flow object not found".to_string()))?;
    crate::flow::policy::require_flow_workspace_access(&state, &extensions, workspace_id).await?;

    if req.deep {
        return Err(ApiError::BadRequest(
            "deep verification is not implemented in v0.4; pass deep=false".to_string(),
        ));
    }

    #[derive(sea_orm::FromQueryResult)]
    struct DocRow {
        id: Uuid,
    }
    use sea_orm::{DbBackend, FromQueryResult, Statement};
    let doc = DocRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM collab_documents WHERE object_id = $1",
        vec![object_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("collab document not found".to_string()))?;

    let started_at = Utc::now();
    let mut warnings = Vec::new();
    let (status, observed_head_seq) = if let Ok(boot) = bootstrap::load(&state.db, doc.id).await {
        if let Some(expected) = req.expected_head_seq
            && expected != boot.head_seq
        {
            warnings.push(format!(
                "expected_head_seq {expected} does not match observed head_seq {}",
                boot.head_seq
            ));
        }
        ("passed".to_string(), Some(boot.head_seq))
    } else {
        warnings.push("resync_required: snapshot/tail integrity check failed".to_string());
        ("failed".to_string(), None)
    };

    Ok(ApiResponse::success(OperationReceipt {
        operation_id: Uuid::new_v4(),
        mode: "dry_run".to_string(),
        status,
        scope: "collab_document".to_string(),
        expected_head_seq: req.expected_head_seq,
        observed_head_seq,
        changes: Vec::new(),
        warnings,
        started_at: started_at.to_rfc3339(),
        finished_at: Some(Utc::now().to_rfc3339()),
        audit_event_id: None,
    }))
}

// ---- Real-database, real-WebSocket tests (opt-in via `OPENPR_TEST_DATABASE_URL`) ----
//
// Unlike `apps/api/src/routes/flow.rs`'s database tests (which call handlers directly through
// their `axum::extract` signatures), a WebSocket upgrade cannot be exercised that way -- there is
// no upgrade without a real HTTP connection. This spins up a real `axum::serve` listener on
// `127.0.0.1:0` backed by the scratch database, and drives it with a real `tokio-tungstenite`
// client, exactly the way a browser would.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines
)]
mod collab_database_tests {
    use axum::Router;
    use axum::middleware as axum_middleware;
    use axum::routing::{get, post};
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use collab_core::{CollabEngine, LoroCollabEngine, NodeId, NodeKind, Operation};
    use futures_util::{SinkExt, StreamExt};
    use platform::{
        app::AppState,
        auth::JwtManager,
        config::{AppConfig, Secret},
    };
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
    use serde_json::Value;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message as TMessage;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use uuid::Uuid;

    use super::{create_ticket, get_collab_diagnostics, verify_collab, ws_upgrade};
    use crate::flow::collab::frame::{Frame, PROTOCOL_VERSION, RejectedCode};
    use crate::middleware::bot_auth::bot_or_user_auth_middleware;

    const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";
    const TEST_ORIGIN: &str = "http://collab-test.local";

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

        let name = format!("openpr_collab_ws_{label}");
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

    const JWT_SECRET: &str = "collab-ws-e2e-test-secret";

    fn state_for(db: DatabaseConnection) -> AppState {
        AppState {
            cfg: AppConfig {
                app_name: "collab-ws-test".to_string(),
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

    async fn exec(state: &AppState, sql: &str, values: Vec<sea_orm::Value>) {
        state
            .db
            .execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .await
            .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
    }

    async fn seed_workspace(state: &AppState) -> (Uuid, Uuid) {
        let workspace_id = Uuid::new_v4();
        let owner_id = Uuid::new_v4();
        exec(
            state,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'test', 'user', true)",
            vec![owner_id.into(), format!("{owner_id}@collab.test").into()],
        )
        .await;
        exec(
            state,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'collab ws test', $3)",
            vec![
                workspace_id.into(),
                format!("ws-{workspace_id}").into(),
                owner_id.into(),
            ],
        )
        .await;
        exec(
            state,
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
            vec![workspace_id.into(), owner_id.into()],
        )
        .await;
        exec(
            state,
            "INSERT INTO flow_workspace_settings (workspace_id, flow_enabled) VALUES ($1, true)",
            vec![workspace_id.into()],
        )
        .await;
        (workspace_id, owner_id)
    }

    async fn create_page(state: &AppState, workspace_id: Uuid, actor_id: Uuid) -> (Uuid, Uuid) {
        use crate::flow::command::{CreateObjectInput, create_object};
        let accepted = create_object(
            state,
            CreateObjectInput {
                workspace_id,
                actor_id,
                object_type: "page".to_string(),
                project_id: None,
                parent_object_id: None,
                title: "Collab WS Test Page".to_string(),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
            },
        )
        .await
        .expect("object creation succeeds");
        (accepted.object.id, accepted.object.document_id)
    }

    fn jwt_for(user_id: Uuid) -> String {
        let manager = JwtManager::new(JWT_SECRET, 900, 3600);
        manager
            .issue_access_token(&user_id.to_string(), &format!("{user_id}@collab.test"))
            .expect("token issues")
    }

    /// Spins up a real listener serving exactly the four collab routes, the same way
    /// `apps/api/src/main.rs` wires them (ticket/diagnostics/verify behind
    /// `bot_or_user_auth_middleware`, the WS upgrade route deliberately unauthenticated).
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
                "/api/v1/flow/objects/{object_id}/collab",
                get(get_collab_diagnostics).route_layer(axum_middleware::from_fn_with_state(
                    auth_state.clone(),
                    bot_or_user_auth_middleware,
                )),
            )
            .route(
                "/api/v1/flow/objects/{object_id}/collab/verify",
                post(verify_collab).route_layer(axum_middleware::from_fn_with_state(
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
        // Give the accept loop a moment to actually start polling the listener.
        tokio::time::sleep(Duration::from_millis(20)).await;
        addr
    }

    async fn issue_ticket(
        addr: SocketAddr,
        token: &str,
        workspace_id: Uuid,
        document_id: Uuid,
        client_id: &str,
    ) -> String {
        let client = reqwest::Client::new();
        let response = client
            .post(format!("http://{addr}/api/v1/collab/tickets"))
            .bearer_auth(token)
            .json(&serde_json::json!({
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

    type WsStream = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

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
            let message = tokio::time::timeout(Duration::from_secs(5), ws.next())
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

    #[tokio::test]
    async fn full_session_hello_open_snapshot_update_accepted_and_two_rejections() {
        let scratch = scratch_or_skip!("full-session");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
        let token = jwt_for(owner_id);
        let addr = spawn_server(state.clone()).await;

        let client_id = "e2e-client-1";
        let ticket = issue_ticket(addr, &token, workspace_id, document_id, client_id).await;
        let mut ws = connect(addr, &ticket, client_id).await;

        // ---- hello ----
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
        let Frame::Hello { protocol_version, .. } = hello_reply else {
            panic!("expected a hello reply, got {hello_reply:?}");
        };
        assert_eq!(protocol_version, PROTOCOL_VERSION);

        // ---- open / snapshot ----
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
            snapshot: snapshot_b64,
            head_seq,
            ..
        } = snapshot_frame
        else {
            panic!("expected a snapshot frame, got {snapshot_frame:?}");
        };
        assert_eq!(head_seq, 0, "a brand-new document starts at head_seq 0");

        // ---- update -> accepted: a real client-side CRDT edit, merged by the server ----
        let snapshot_bytes = BASE64.decode(snapshot_b64).expect("snapshot is valid base64");
        let mut client_engine = LoroCollabEngine::load(&snapshot_bytes).expect("client loads the server snapshot");
        let base_frontier = client_engine.frontier();
        client_engine
            .set_title("Edited over the wire")
            .expect("set_title succeeds");
        let update_bytes = client_engine.export_from(&base_frontier).expect("exports a real delta");

        let update_id = Uuid::new_v4();
        send_frame(
            &mut ws,
            &Frame::Update {
                protocol_version: PROTOCOL_VERSION,
                document_id,
                update_id,
                base_frontier: BASE64.encode(base_frontier.as_bytes()),
                bytes: BASE64.encode(&update_bytes),
                idempotency_key: None,
                origin: "web".to_string(),
                message: Some("e2e edit".to_string()),
            },
        )
        .await;
        let accepted_frame = recv_frame(&mut ws).await;
        let Frame::Accepted {
            update_id: accepted_update_id,
            head_seq: new_head_seq,
            ..
        } = accepted_frame
        else {
            panic!("expected an accepted frame, got {accepted_frame:?}");
        };
        assert_eq!(accepted_update_id, update_id);
        assert_eq!(new_head_seq, 1, "the first accepted update advances head_seq to 1");

        // ---- rejection 1: a limit-exceeded update (over update_bytes_max = 65536) ----
        let oversized = vec![0u8; 70_000];
        send_frame(
            &mut ws,
            &Frame::Update {
                protocol_version: PROTOCOL_VERSION,
                document_id,
                update_id: Uuid::new_v4(),
                base_frontier: BASE64.encode(base_frontier.as_bytes()),
                bytes: BASE64.encode(&oversized),
                idempotency_key: None,
                origin: "web".to_string(),
                message: None,
            },
        )
        .await;
        let rejected_limit = recv_frame(&mut ws).await;
        let Frame::Rejected { code: limit_code, .. } = rejected_limit else {
            panic!("expected a rejected frame, got {rejected_limit:?}");
        };
        assert_eq!(limit_code, RejectedCode::LimitExceeded);

        // ---- rejection 2: a well-under-limit but structurally invalid update (decode failure) ----
        let garbage = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        send_frame(
            &mut ws,
            &Frame::Update {
                protocol_version: PROTOCOL_VERSION,
                document_id,
                update_id: Uuid::new_v4(),
                base_frontier: BASE64.encode(base_frontier.as_bytes()),
                bytes: BASE64.encode(&garbage),
                idempotency_key: None,
                origin: "web".to_string(),
                message: None,
            },
        )
        .await;
        let rejected_invalid = recv_frame(&mut ws).await;
        let Frame::Rejected { code: invalid_code, .. } = rejected_invalid else {
            panic!("expected a rejected frame, got {rejected_invalid:?}");
        };
        assert_eq!(invalid_code, RejectedCode::InvalidUpdate);

        // The two rejected updates must never have been persisted.
        #[derive(sea_orm::FromQueryResult)]
        struct CountRow {
            n: i64,
        }
        let count = CountRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM collab_updates WHERE document_id = $1",
            vec![document_id.into()],
        ))
        .one(&state.db)
        .await
        .expect("count query runs")
        .expect("count query returns a row");
        assert_eq!(count.n, 1, "only the one genuinely accepted update was persisted");

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_ticket_issued_from_a_disallowed_origin_is_rejected() {
        let scratch = scratch_or_skip!("bad-origin");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
        let token = jwt_for(owner_id);
        let addr = spawn_server(state.clone()).await;

        let client = reqwest::Client::new();
        let response = client
            .post(format!("http://{addr}/api/v1/collab/tickets"))
            .bearer_auth(&token)
            .json(&serde_json::json!({
                "workspace_id": workspace_id,
                "document_id": document_id,
                "client_id": "bad-origin-client",
                "origin": "https://evil.example",
            }))
            .send()
            .await
            .expect("request completes");
        let body: Value = response.json().await.expect("response is JSON");
        assert_eq!(body["code"], 403, "an unallowlisted origin must be rejected: {body}");

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_bot_token_cannot_issue_a_collab_ticket() {
        let scratch = scratch_or_skip!("bot-rejected");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
        let addr = spawn_server(state.clone()).await;

        let bot_id = Uuid::new_v4();
        let token_raw = format!("opr_{}", Uuid::new_v4().simple());
        let token_hash = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(token_raw.as_bytes());
            hex::encode(hasher.finalize())
        };
        exec(
            &state,
            "INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, is_active, created_by) \
             VALUES ($1, $2, 'collab-test-bot', $3, $5, '[\"admin\"]'::jsonb, true, $4)",
            vec![
                bot_id.into(),
                workspace_id.into(),
                token_hash.into(),
                owner_id.into(),
                token_raw.chars().take(8).collect::<String>().into(),
            ],
        )
        .await;

        let client = reqwest::Client::new();
        let response = client
            .post(format!("http://{addr}/api/v1/collab/tickets"))
            .bearer_auth(&token_raw)
            .json(&serde_json::json!({
                "workspace_id": workspace_id,
                "document_id": document_id,
                "client_id": "bot-client",
                "origin": TEST_ORIGIN,
            }))
            .send()
            .await
            .expect("request completes");
        let body: Value = response.json().await.expect("response is JSON");
        assert_eq!(
            body["code"], 403,
            "a bot token must never receive a collab ticket: {body}"
        );

        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn diagnostics_and_verify_report_the_real_document_state() {
        let scratch = scratch_or_skip!("diagnostics-verify");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
        let token = jwt_for(owner_id);
        let addr = spawn_server(state.clone()).await;

        let client = reqwest::Client::new();
        let diag_response = client
            .get(format!("http://{addr}/api/v1/flow/objects/{object_id}/collab"))
            .bearer_auth(&token)
            .send()
            .await
            .expect("request completes");
        let diag_body: Value = diag_response.json().await.expect("response is JSON");
        assert_eq!(diag_body["code"], 0, "{diag_body}");
        assert_eq!(diag_body["data"]["document_id"], document_id.to_string());
        assert_eq!(diag_body["data"]["head_seq"], 0);
        assert_eq!(diag_body["data"]["snapshot_seq"], 0);
        assert_eq!(diag_body["data"]["engine"], "loro");
        assert_eq!(diag_body["data"]["integrity_state"], "ok");
        assert!(
            diag_body["data"]["frontier"].is_string(),
            "frontier is base64, never absent"
        );

        let verify_response = client
            .post(format!("http://{addr}/api/v1/flow/objects/{object_id}/collab/verify"))
            .bearer_auth(&token)
            .json(&serde_json::json!({"expected_head_seq": 0, "deep": false, "idempotency_key": Uuid::new_v4().to_string()}))
            .send()
            .await
            .expect("request completes");
        let verify_body: Value = verify_response.json().await.expect("response is JSON");
        assert_eq!(verify_body["code"], 0, "{verify_body}");
        assert_eq!(verify_body["data"]["status"], "passed");
        assert_eq!(verify_body["data"]["observed_head_seq"], 0);
        assert_eq!(
            verify_body["data"]["warnings"]
                .as_array()
                .expect("warnings is an array")
                .len(),
            0
        );

        // deep=true is explicitly not implemented in v0.4.
        let deep_response = client
            .post(format!("http://{addr}/api/v1/flow/objects/{object_id}/collab/verify"))
            .bearer_auth(&token)
            .json(&serde_json::json!({"deep": true, "idempotency_key": Uuid::new_v4().to_string()}))
            .send()
            .await
            .expect("request completes");
        let deep_body: Value = deep_response.json().await.expect("response is JSON");
        assert_eq!(deep_body["code"], 400, "{deep_body}");

        scratch.drop_self().await;
    }

    /// ★ The multi-tab gap this module's `session::broadcast_content_update` closes: a second,
    /// already-`open` WebSocket session on the *same* document must receive a real `update` frame
    /// carrying bytes it can incrementally apply — not just an `accepted` ack it cannot act on, and
    /// not a forced reconnect/re-bootstrap. `B` never calls `bootstrap`/`snapshot` a second time
    /// anywhere in this test; it applies exactly one relayed `update` on top of the one snapshot it
    /// received at `open` time, and the resulting semantic state must hash identically to `A`'s own
    /// local state.
    #[tokio::test]
    #[allow(clippy::similar_names)] // `snapshot_a_b64`/`snapshot_b_b64` are deliberately parallel tab A/B names
    async fn a_second_open_session_receives_an_incremental_update_and_converges_on_the_same_semantic_hash() {
        let scratch = scratch_or_skip!("two-tabs");
        let state = state_for(scratch.db.clone());
        let (workspace_id, owner_id) = seed_workspace(&state).await;
        let (_object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
        let token = jwt_for(owner_id);
        let addr = spawn_server(state.clone()).await;

        async fn hello_open_snapshot(ws: &mut WsStream, client_id: &str, document_id: Uuid) -> String {
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
            let hello_reply = recv_frame(ws).await;
            assert!(matches!(hello_reply, Frame::Hello { .. }), "{hello_reply:?}");

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
            let snapshot_frame = recv_frame(ws).await;
            let Frame::Snapshot { snapshot, head_seq, .. } = snapshot_frame else {
                panic!("expected a snapshot frame, got {snapshot_frame:?}");
            };
            assert_eq!(head_seq, 0, "a brand-new document starts at head_seq 0");
            snapshot
        }

        // Tab A and tab B: two independent WebSocket connections open on the exact same document,
        // exactly like two browser tabs.
        let client_a = "two-tabs-client-a";
        let ticket_a = issue_ticket(addr, &token, workspace_id, document_id, client_a).await;
        let mut ws_a = connect(addr, &ticket_a, client_a).await;
        let snapshot_a_b64 = hello_open_snapshot(&mut ws_a, client_a, document_id).await;

        let client_b = "two-tabs-client-b";
        let ticket_b = issue_ticket(addr, &token, workspace_id, document_id, client_b).await;
        let mut ws_b = connect(addr, &ticket_b, client_b).await;
        let snapshot_b_b64 = hello_open_snapshot(&mut ws_b, client_b, document_id).await;
        assert_eq!(
            snapshot_a_b64, snapshot_b_b64,
            "both tabs bootstrap from the same document state"
        );

        // A performs a real local CRDT edit and submits it as an `update` frame.
        let snapshot_bytes = BASE64.decode(&snapshot_a_b64).expect("snapshot is valid base64");
        let mut engine_a = LoroCollabEngine::load(&snapshot_bytes).expect("A loads the shared snapshot");
        let base_frontier = engine_a.frontier();
        let node_id = NodeId::from("blk-two-tabs");
        engine_a
            .apply_operation(&Operation::CreateNode {
                id: node_id.clone(),
                parent: None,
                index: 0,
                kind: NodeKind::Block,
            })
            .expect("create succeeds");
        engine_a
            .apply_operation(&Operation::InsertText {
                id: node_id.clone(),
                index: 0,
                text: "hello from A".to_string(),
            })
            .expect("insert succeeds");
        let update_bytes = engine_a.export_from(&base_frontier).expect("exports a real delta");

        let update_id = Uuid::new_v4();
        send_frame(
            &mut ws_a,
            &Frame::Update {
                protocol_version: PROTOCOL_VERSION,
                document_id,
                update_id,
                base_frontier: BASE64.encode(base_frontier.as_bytes()),
                bytes: BASE64.encode(&update_bytes),
                idempotency_key: None,
                origin: "web".to_string(),
                message: None,
            },
        )
        .await;

        // A gets its own ack.
        let accepted_to_a = recv_frame(&mut ws_a).await;
        let Frame::Accepted {
            update_id: acked_id,
            head_seq: a_head_seq,
            ..
        } = accepted_to_a
        else {
            panic!("expected an accepted frame for A, got {accepted_to_a:?}");
        };
        assert_eq!(acked_id, update_id);
        assert_eq!(a_head_seq, 1);

        // B -- the *other* tab, never reconnecting and never re-bootstrapping -- must receive a
        // real `update` frame it can apply incrementally, immediately followed by the matching
        // `accepted` seq anchor. This is the assertion that fails against the pre-fix behavior
        // (broadcasting only a bytes-less `accepted`).
        let relay_to_b = recv_frame(&mut ws_b).await;
        let Frame::Update {
            update_id: relay_update_id,
            bytes: relay_bytes_b64,
            ..
        } = relay_to_b
        else {
            panic!("B must receive a real `update` frame carrying bytes, not just an ack; got {relay_to_b:?}");
        };
        assert_eq!(relay_update_id, update_id);

        let accepted_to_b = recv_frame(&mut ws_b).await;
        let Frame::Accepted {
            update_id: accepted_to_b_id,
            head_seq: b_head_seq,
            ..
        } = accepted_to_b
        else {
            panic!("expected an accepted frame for B, got {accepted_to_b:?}");
        };
        assert_eq!(accepted_to_b_id, update_id);
        assert_eq!(b_head_seq, 1);

        // B applies the relayed bytes to a fresh copy of the *same* snapshot it bootstrapped from
        // at `open` time -- proving genuine incremental application, not a disguised full resync.
        let mut engine_b = LoroCollabEngine::load(&snapshot_bytes).expect("B loads its own bootstrapped snapshot");
        let relay_bytes = BASE64.decode(&relay_bytes_b64).expect("relay bytes are valid base64");
        engine_b
            .import_update(&relay_bytes)
            .expect("B applies the incremental update");

        let semantic_a = engine_a.semantic_snapshot().expect("A's semantic snapshot reads");
        let semantic_b = engine_b.semantic_snapshot().expect("B's semantic snapshot reads");
        assert_eq!(
            semantic_a.semantic_hash(),
            semantic_b.semantic_hash(),
            "B's incrementally-applied state must be semantically identical to A's local state"
        );
        assert_eq!(
            semantic_b
                .nodes
                .get(&node_id)
                .expect("the node A created is present on B")
                .text,
            "hello from A"
        );

        scratch.drop_self().await;
    }
}
