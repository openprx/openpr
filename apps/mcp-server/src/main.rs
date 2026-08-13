mod cli;
mod client;
mod protocol;
mod server;
mod tools;

use axum::{
    Json, Router,
    extract::{Query, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use clap::Parser;
use cli::{Cli, Commands};
use client::OpenPrClient;
use protocol::{JsonRpcRequest, JsonRpcResponse};
use serde::Deserialize;
use serde_json::json;
use server::McpServer;
use std::{collections::HashMap, convert::Infallible, sync::Arc};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, mpsc};
use tokio_stream::{StreamExt, wrappers::UnboundedReceiverStream};
use uuid::Uuid;

const DEFAULT_OPENPR_API_URL: &str = "http://localhost:8081";

/// Environment variable carrying the bearer token inbound HTTP/SSE callers must present.
///
/// Read from the environment only, never from a CLI flag: a flag would put the secret in
/// `argv`, which any local process can read out of `/proc`.
const INBOUND_AUTH_TOKEN_ENV: &str = "OPENPR_MCP_AUTH_TOKEN";

/// Shortest accepted inbound token. A two character token is worse than none: it invites
/// the operator to believe the port is protected while it is trivially guessable.
const MIN_INBOUND_AUTH_TOKEN_LEN: usize = 16;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Initialize tracing — always write to stderr to keep stdout clean for JSON-RPC
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive(
                "mcp_server=info"
                    .parse()
                    .unwrap_or_else(|_| tracing_subscriber::filter::Directive::default()),
            ),
        )
        .init();

    // Resolve config: CLI flags override environment variables
    let base_url = cli
        .api_url
        .clone()
        .or_else(|| std::env::var("OPENPR_API_URL").ok())
        .unwrap_or_else(|| DEFAULT_OPENPR_API_URL.to_string());

    let bot_token_opt = cli.bot_token.clone().or_else(|| std::env::var("OPENPR_BOT_TOKEN").ok());

    let workspace_id_opt = cli
        .workspace_id
        .clone()
        .or_else(|| std::env::var("OPENPR_WORKSPACE_ID").ok());

    let inbound_auth_token = std::env::var(INBOUND_AUTH_TOKEN_ENV).ok();

    if let Commands::Serve(serve_args) = &cli.command {
        let bot_token =
            bot_token_opt.ok_or_else(|| anyhow::anyhow!("OPENPR_BOT_TOKEN environment variable is required"))?;
        let workspace_id =
            workspace_id_opt.ok_or_else(|| anyhow::anyhow!("OPENPR_WORKSPACE_ID environment variable is required"))?;

        validate_runtime_config(&base_url, &bot_token, &workspace_id)?;
        let client = OpenPrClient::new(base_url, bot_token, workspace_id).map_err(|e| anyhow::anyhow!(e))?;

        // stdio is a pipe pair owned by the parent process, so it carries no inbound
        // authentication question: whoever spawned the process is already the caller.
        match serve_args.transport {
            cli::Transport::Http => {
                let auth = resolve_inbound_auth(&serve_args.bind_addr, inbound_auth_token.as_deref())?;
                run_http(&serve_args.bind_addr, client, auth).await
            }
            cli::Transport::Sse => {
                let auth = resolve_inbound_auth(&serve_args.bind_addr, inbound_auth_token.as_deref())?;
                run_sse(&serve_args.bind_addr, client, auth).await
            }
            cli::Transport::Stdio => run_stdio(client).await,
        }
    } else {
        let bot_token = bot_token_opt
            .ok_or_else(|| anyhow::anyhow!("OPENPR_BOT_TOKEN is required (set env var or --bot-token)"))?;
        let workspace_id = workspace_id_opt
            .ok_or_else(|| anyhow::anyhow!("OPENPR_WORKSPACE_ID is required (set env var or --workspace-id)"))?;

        validate_runtime_config(&base_url, &bot_token, &workspace_id)?;
        let client = OpenPrClient::new(base_url, bot_token, workspace_id).map_err(|e| anyhow::anyhow!(e))?;
        cli::run_cli_command(&cli.command, &cli.format, client).await
    }
}

fn validate_runtime_config(base_url: &str, bot_token: &str, workspace_id: &str) -> anyhow::Result<()> {
    if base_url.trim().is_empty() || base_url.contains("${") {
        anyhow::bail!("OPENPR_API_URL must be a concrete API URL");
    }
    let parsed_url =
        reqwest::Url::parse(base_url).map_err(|e| anyhow::anyhow!("OPENPR_API_URL is not a valid URL: {e}"))?;
    if !matches!(parsed_url.scheme(), "http" | "https") {
        anyhow::bail!("OPENPR_API_URL must use http or https");
    }

    if bot_token.trim().is_empty() || bot_token.contains("${") || bot_token.contains("replace_with") {
        anyhow::bail!("OPENPR_BOT_TOKEN must be a concrete production bot token");
    }
    if !bot_token.starts_with("opr_") {
        anyhow::bail!("OPENPR_BOT_TOKEN must use the opr_ token prefix");
    }

    let workspace_uuid =
        Uuid::parse_str(workspace_id).map_err(|e| anyhow::anyhow!("OPENPR_WORKSPACE_ID must be a valid UUID: {e}"))?;
    if workspace_uuid.is_nil() {
        anyhow::bail!("OPENPR_WORKSPACE_ID must not be the nil UUID placeholder");
    }

    Ok(())
}

/// How inbound HTTP/SSE requests are authenticated.
///
/// The MCP server holds a workspace bot token and speaks to the API with it, so anyone
/// who can reach this port holds every permission that bot has. The bot token
/// authenticates this process *to* the API; it says nothing about who is calling *in*.
#[derive(Clone)]
enum InboundAuth {
    /// Every data carrying route requires this bearer token.
    BearerToken(Arc<str>),
    /// No inbound authentication, permitted only because the listener is bound to a
    /// loopback address where the kernel already restricts callers to this host.
    LoopbackOnly,
}

impl InboundAuth {
    /// A label safe to log. Never derives from the token itself.
    const fn describe(&self) -> &'static str {
        match self {
            Self::BearerToken(_) => "bearer-token",
            Self::LoopbackOnly => "none (loopback bind)",
        }
    }
}

/// Decides how a listener authenticates its callers, refusing the one combination that
/// cannot be made safe: reachable from the network *and* unauthenticated.
///
/// Loopback binds may stay unauthenticated so local development keeps working, but they
/// say so in the log rather than passing silently.
fn resolve_inbound_auth(bind_addr: &str, configured_token: Option<&str>) -> anyhow::Result<InboundAuth> {
    if let Some(token) = configured_token.map(str::trim).filter(|token| !token.is_empty()) {
        if token.len() < MIN_INBOUND_AUTH_TOKEN_LEN {
            // The token is never echoed, not even its prefix.
            anyhow::bail!(
                "{INBOUND_AUTH_TOKEN_ENV} must be at least {MIN_INBOUND_AUTH_TOKEN_LEN} characters long to be worth \
                 checking"
            );
        }
        return Ok(InboundAuth::BearerToken(Arc::from(token)));
    }

    if is_loopback_bind_addr(bind_addr) {
        tracing::warn!(
            bind_addr = %bind_addr,
            "{INBOUND_AUTH_TOKEN_ENV} is not set, so inbound MCP requests are NOT authenticated. This is allowed only \
             because the listener is bound to a loopback address and callers are limited to this host. Set \
             {INBOUND_AUTH_TOKEN_ENV} before publishing this port."
        );
        return Ok(InboundAuth::LoopbackOnly);
    }

    anyhow::bail!(
        "Refusing to serve MCP on non-loopback bind address '{bind_addr}' without inbound authentication: set \
         {INBOUND_AUTH_TOKEN_ENV}, or bind to 127.0.0.1 for local development. This process holds a workspace bot \
         token, so anyone able to reach the port would hold every permission that bot has."
    )
}

/// Whether a bind address restricts callers to this host.
///
/// Anything that is not a literal loopback address is treated as remotely reachable. A
/// hostname other than `localhost` is not resolved: this decides whether authentication
/// may be skipped, and guessing wrong in that direction publishes an open port.
fn is_loopback_bind_addr(bind_addr: &str) -> bool {
    let bind_addr = bind_addr.trim();
    if let Ok(socket_addr) = bind_addr.parse::<std::net::SocketAddr>() {
        return socket_addr.ip().is_loopback();
    }
    let host = bind_addr.rsplit_once(':').map_or(bind_addr, |(host, _port)| host);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The token out of an `Authorization: Bearer <token>` header, if it is well formed.
fn bearer_token(header_value: &str) -> Option<&str> {
    let (scheme, token) = header_value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() { None } else { Some(token) }
}

/// Compares two secrets without an early exit on the first differing byte, so the time
/// taken does not reveal how much of a guess was correct.
///
/// The length comparison is deliberately not constant time — that is standard for bearer
/// token checks and leaks only the token's length, never its bytes.
fn constant_time_eq(presented: &[u8], expected: &[u8]) -> bool {
    if presented.len() != expected.len() {
        return false;
    }
    let mut difference = 0u8;
    for (presented_byte, expected_byte) in presented.iter().zip(expected.iter()) {
        difference |= presented_byte ^ expected_byte;
    }
    std::hint::black_box(difference) == 0
}

/// Rejects inbound requests that do not present the configured bearer token.
async fn require_inbound_auth(State(auth): State<InboundAuth>, request: Request, next: Next) -> Response {
    let InboundAuth::BearerToken(expected) = &auth else {
        return next.run(request).await;
    };

    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(bearer_token);

    if presented.is_some_and(|token| constant_time_eq(token.as_bytes(), expected.as_bytes())) {
        return next.run(request).await;
    }

    // Neither the presented nor the expected token is logged, on either branch.
    tracing::warn!(
        path = %request.uri().path(),
        "Rejected an inbound MCP request with a missing or invalid bearer token"
    );
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
        Json(json!({ "error": "Missing or invalid Authorization: Bearer token" })),
    )
        .into_response()
}

/// Wraps the data carrying routes in the inbound auth layer.
///
/// `/health` stays outside it on purpose: it answers a constant `OK`, reads nothing and
/// discloses nothing a caller who already completed the TCP handshake does not know, and
/// orchestrator probes must not need the shared secret to run.
fn serve_router(state: SseState, auth: InboundAuth, protected: Router<SseState>) -> Router {
    Router::new()
        .merge(protected.route_layer(middleware::from_fn_with_state(auth, require_inbound_auth)))
        .route("/health", get(health_check))
        .with_state(state)
}

async fn run_http(bind_addr: &str, client: OpenPrClient, auth: InboundAuth) -> anyhow::Result<()> {
    let state = SseState {
        client,
        sessions: Arc::new(Mutex::new(HashMap::new())),
    };

    let protected = Router::new()
        .route("/mcp/rpc", post(handle_jsonrpc))
        .route("/sse", get(handle_sse_connect))
        .route("/messages", post(handle_sse_message));
    let auth_mode = auth.describe();
    let app = serve_router(state, auth, protected);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(
        bind_addr = %bind_addr,
        inbound_auth = %auth_mode,
        "MCP HTTP transport started (JSON-RPC + SSE)"
    );
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle_jsonrpc(State(state): State<SseState>, Json(req): Json<JsonRpcRequest>) -> impl IntoResponse {
    let server = McpServer::new(state.client.clone());
    server.handle_request(req).await.map_or_else(
        || (StatusCode::ACCEPTED, Json(json!({"status": "accepted"}))),
        |response| (StatusCode::OK, Json(json!(response))),
    )
}

#[derive(Clone)]
struct SseState {
    client: OpenPrClient,
    sessions: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<SseServerEvent>>>>,
}

#[derive(Debug)]
enum SseServerEvent {
    Endpoint(String),
    Message(String),
}

struct SessionGuard {
    session_id: String,
    sessions: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<SseServerEvent>>>>,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let session_id = self.session_id.clone();
        let sessions = self.sessions.clone();
        tokio::spawn(async move {
            sessions.lock().await.remove(&session_id);
        });
    }
}

#[derive(Debug, Deserialize)]
struct MessagesQuery {
    session_id: String,
}

async fn run_sse(bind_addr: &str, client: OpenPrClient, auth: InboundAuth) -> anyhow::Result<()> {
    let state = SseState {
        client,
        sessions: Arc::new(Mutex::new(HashMap::new())),
    };

    let protected = Router::new()
        .route("/sse", get(handle_sse_connect))
        .route("/messages", post(handle_sse_message));
    let auth_mode = auth.describe();
    let app = serve_router(state, auth, protected);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(
        bind_addr = %bind_addr,
        inbound_auth = %auth_mode,
        "MCP SSE transport started"
    );
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle_sse_connect(
    State(state): State<SseState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let session_id = Uuid::new_v4().to_string();
    let endpoint = format!("/messages?session_id={session_id}");
    let (tx, rx) = mpsc::unbounded_channel::<SseServerEvent>();

    state.sessions.lock().await.insert(session_id.clone(), tx.clone());

    let _ = tx.send(SseServerEvent::Endpoint(endpoint));

    let session_guard = SessionGuard {
        session_id,
        sessions: state.sessions.clone(),
    };

    let stream = UnboundedReceiverStream::new(rx).map(move |msg| {
        let _keep_guard_alive = &session_guard;
        let event = match msg {
            SseServerEvent::Endpoint(url) => Event::default().event("endpoint").data(url),
            SseServerEvent::Message(payload) => Event::default().event("message").data(payload),
        };
        Ok::<Event, Infallible>(event)
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn handle_sse_message(
    State(state): State<SseState>,
    Query(query): Query<MessagesQuery>,
    Json(req): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let sender = state.sessions.lock().await.get(&query.session_id).cloned();
    let Some(sender) = sender else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "Unknown SSE session_id"})));
    };

    let server = McpServer::new(state.client.clone());
    let response = server.handle_request(req).await;
    let Some(response) = response else {
        return (StatusCode::ACCEPTED, Json(json!({"status": "accepted"})));
    };

    let response_json = match serde_json::to_string(&response) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Failed to serialize response: {e}")})),
            );
        }
    };

    if sender.send(SseServerEvent::Message(response_json)).is_err() {
        state.sessions.lock().await.remove(&query.session_id);
        return (StatusCode::GONE, Json(json!({"error": "SSE session is closed"})));
    }

    (StatusCode::ACCEPTED, Json(json!({"status": "accepted"})))
}

async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_OPENPR_API_URL, InboundAuth, MIN_INBOUND_AUTH_TOKEN_LEN, bearer_token, constant_time_eq,
        is_loopback_bind_addr, resolve_inbound_auth, validate_runtime_config,
    };

    const TOKEN: &str = "an-inbound-token-long-enough";

    #[test]
    fn accepts_concrete_runtime_config() {
        validate_runtime_config(
            "http://api:8080",
            "opr_forms_mcp_test_token",
            "550e8400-e29b-41d4-a716-446655440000",
        )
        .expect("valid runtime config should pass");
    }

    #[test]
    fn default_api_url_targets_compose_api_host_port() {
        assert_eq!(DEFAULT_OPENPR_API_URL, "http://localhost:8081");
    }

    #[test]
    fn rejects_compose_placeholder_literals() {
        assert!(
            validate_runtime_config(
                "${OPENPR_API_URL:-http://api:8080}",
                "${OPENPR_BOT_TOKEN:?set OPENPR_BOT_TOKEN}",
                "${OPENPR_WORKSPACE_ID:?set OPENPR_WORKSPACE_ID}",
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_demo_token_and_nil_workspace() {
        assert!(
            validate_runtime_config(
                "http://api:8080",
                "opr_replace_with_workspace_bot_token",
                "00000000-0000-0000-0000-000000000000",
            )
            .is_err()
        );
    }

    /// The combination this gate exists to make impossible: reachable from the network
    /// and unauthenticated. Binding wide open used to be a plain config choice, with the
    /// compose port mapping as the only thing between a workspace bot and the internet.
    #[test]
    fn a_non_loopback_bind_without_a_token_refuses_to_start() {
        for bind_addr in [
            "0.0.0.0:8090",
            "[::]:8090",
            "192.168.31.136:8090",
            "10.0.0.5:8090",
            // A hostname that is not `localhost` is not resolved, so it counts as remote.
            "mcp.internal:8090",
        ] {
            assert!(
                resolve_inbound_auth(bind_addr, None).is_err(),
                "{bind_addr} was allowed to serve unauthenticated"
            );
            assert!(
                resolve_inbound_auth(bind_addr, Some("   ")).is_err(),
                "{bind_addr} treated a blank token as configured auth"
            );
        }
    }

    #[test]
    fn a_non_loopback_bind_serves_once_a_token_is_configured() -> Result<(), Box<dyn std::error::Error>> {
        let auth = resolve_inbound_auth("0.0.0.0:8090", Some(TOKEN))?;
        assert!(matches!(auth, InboundAuth::BearerToken(_)));
        assert_eq!(auth.describe(), "bearer-token");
        Ok(())
    }

    /// Loopback stays usable without a token so local development is unaffected, and the
    /// mode is reported rather than assumed.
    #[test]
    fn a_loopback_bind_may_stay_unauthenticated() -> Result<(), Box<dyn std::error::Error>> {
        for bind_addr in ["127.0.0.1:8090", "[::1]:8090", "localhost:8090", "127.7.7.7:8090"] {
            let auth = resolve_inbound_auth(bind_addr, None)?;
            assert!(
                matches!(auth, InboundAuth::LoopbackOnly),
                "{bind_addr} should be recognised as loopback"
            );
            assert_eq!(auth.describe(), "none (loopback bind)");
        }
        Ok(())
    }

    /// A token too short to resist guessing is rejected outright rather than accepted as
    /// weak protection, on loopback as well as off it.
    #[test]
    fn a_too_short_token_is_refused_everywhere() {
        let short = "x".repeat(MIN_INBOUND_AUTH_TOKEN_LEN - 1);
        assert!(resolve_inbound_auth("0.0.0.0:8090", Some(&short)).is_err());
        assert!(resolve_inbound_auth("127.0.0.1:8090", Some(&short)).is_err());
        assert!(resolve_inbound_auth("0.0.0.0:8090", Some(&"x".repeat(MIN_INBOUND_AUTH_TOKEN_LEN))).is_ok());
    }

    /// The failure message has to name the fix without ever printing the secret.
    #[test]
    fn the_fail_closed_message_names_the_variable_and_never_a_token() {
        let Err(error) = resolve_inbound_auth("0.0.0.0:8090", None) else {
            panic!("a wide open bind without a token must not be accepted");
        };
        let message = error.to_string();
        assert!(message.contains("OPENPR_MCP_AUTH_TOKEN"), "{message}");
        assert!(message.contains("127.0.0.1"), "{message}");

        let Err(error) = resolve_inbound_auth("0.0.0.0:8090", Some("short")) else {
            panic!("a too short token must not be accepted");
        };
        assert!(!error.to_string().contains("short"), "the error echoed the token");
    }

    #[test]
    fn loopback_detection_does_not_guess_in_the_permissive_direction() {
        assert!(is_loopback_bind_addr("127.0.0.1:8090"));
        assert!(is_loopback_bind_addr(" localhost:8090 "));
        assert!(!is_loopback_bind_addr("0.0.0.0:8090"));
        assert!(!is_loopback_bind_addr("[::]:8090"));
        // Not a bindable address at all, so it must not be read as loopback.
        assert!(!is_loopback_bind_addr(""));
        assert!(!is_loopback_bind_addr("localhost.evil.example:8090"));
    }

    #[test]
    fn bearer_parsing_accepts_only_a_well_formed_bearer_header() {
        assert_eq!(bearer_token("Bearer abc"), Some("abc"));
        assert_eq!(bearer_token("bearer  abc  "), Some("abc"));
        assert_eq!(bearer_token("BEARER abc"), Some("abc"));
        assert_eq!(bearer_token("Basic abc"), None);
        assert_eq!(bearer_token("Bearer "), None);
        assert_eq!(bearer_token("abc"), None);
    }

    #[test]
    fn token_comparison_matches_only_the_exact_secret() {
        assert!(constant_time_eq(TOKEN.as_bytes(), TOKEN.as_bytes()));
        assert!(!constant_time_eq(b"", TOKEN.as_bytes()));
        // A correct prefix must not be accepted, which is what a `starts_with` check or a
        // truncated comparison would do.
        let prefix = TOKEN.as_bytes().get(..8).unwrap_or_default();
        assert!(!constant_time_eq(prefix, TOKEN.as_bytes()));
        assert!(!constant_time_eq(format!("{TOKEN}x").as_bytes(), TOKEN.as_bytes()));
        let mut wrong_last_byte = TOKEN.to_string();
        wrong_last_byte.pop();
        wrong_last_byte.push('!');
        assert!(!constant_time_eq(wrong_last_byte.as_bytes(), TOKEN.as_bytes()));
    }
}

/// Whether the line is a Content-Length or Content-Type header (case-insensitive).
fn is_stdio_header_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.starts_with("content-length:") || lower.starts_with("content-type:")
}

/// Parse content-length value from a header line.
fn parse_content_length(line: &str) -> Option<usize> {
    let lower = line.to_ascii_lowercase();
    if lower.starts_with("content-length:") {
        line[15..].trim().parse::<usize>().ok()
    } else {
        None
    }
}

/// Whether we used Content-Length framing or line-delimited.
#[derive(Copy, Clone)]
enum StdioFrame {
    LineDelimited,
    ContentLength,
}

async fn write_stdio_response(
    stdout: &mut tokio::io::Stdout,
    response_json: &str,
    frame: StdioFrame,
) -> anyhow::Result<()> {
    match frame {
        StdioFrame::LineDelimited => {
            stdout.write_all(response_json.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
        }
        StdioFrame::ContentLength => {
            let len = response_json.len();
            let header = format!("Content-Length: {len}\r\n\r\n");
            stdout.write_all(header.as_bytes()).await?;
            stdout.write_all(response_json.as_bytes()).await?;
        }
    }
    stdout.flush().await?;
    Ok(())
}

/// Read HTTP-style headers until empty line; return the Content-Length value (0 if absent).
async fn read_headers_get_content_length(reader: &mut BufReader<tokio::io::Stdin>, first_line: &str) -> usize {
    let mut content_length = parse_content_length(first_line);
    loop {
        let mut header_line = String::new();
        match reader.read_line(&mut header_line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let ht = header_line.trim();
                if ht.is_empty() {
                    break;
                }
                content_length = content_length.or_else(|| parse_content_length(ht));
            }
        }
    }
    content_length.unwrap_or(0)
}

async fn run_stdio(client: OpenPrClient) -> anyhow::Result<()> {
    tracing::info!("MCP stdio transport started");

    let server = McpServer::new(client);
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) => {
                tracing::info!("stdin closed, shutting down");
                break;
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                // Detect Content-Length framing (used by Codex, Claude Desktop)
                let (payload, frame) = if is_stdio_header_line(trimmed) {
                    let cl = read_headers_get_content_length(&mut reader, trimmed).await;
                    if cl == 0 {
                        continue;
                    }
                    let mut body = vec![0u8; cl];
                    if let Err(e) = reader.read_exact(&mut body).await {
                        tracing::error!(error = %e, "Failed to read Content-Length body");
                        continue;
                    }
                    (body, StdioFrame::ContentLength)
                } else {
                    // Line-delimited JSON
                    (trimmed.as_bytes().to_vec(), StdioFrame::LineDelimited)
                };

                let request: JsonRpcRequest = match serde_json::from_slice(&payload) {
                    Ok(req) => req,
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to parse request");
                        let error_response = JsonRpcResponse::error(
                            None,
                            protocol::JsonRpcError::parse_error(format!("Invalid JSON: {e}")),
                        );
                        if let Ok(rj) = serde_json::to_string(&error_response) {
                            let _ = write_stdio_response(&mut stdout, &rj, frame).await;
                        }
                        continue;
                    }
                };

                tracing::debug!(method = %request.method, "Received request");

                let response = server.handle_request(request).await;
                let Some(response) = response else {
                    continue;
                };

                match serde_json::to_string(&response) {
                    Ok(response_json) => {
                        if let Err(e) = write_stdio_response(&mut stdout, &response_json, frame).await {
                            tracing::error!(error = %e, "Failed to write response");
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to serialize response");
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to read from stdin");
                break;
            }
        }
    }

    Ok(())
}
