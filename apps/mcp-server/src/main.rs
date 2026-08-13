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
use cli::{Cli, Commands, ServeArgs};
use client::{ClientConfig, OpenPrClient, transport_label};
use platform::config::{MIN_MCP_AUTH_TOKEN_LEN, McpConfig, McpRuntime, McpTransport, OpenPrConfig, Secret};
use protocol::{JsonRpcRequest, JsonRpcResponse};
use serde::Deserialize;
use serde_json::json;
use server::McpServer;
use std::{collections::HashMap, convert::Infallible, sync::Arc};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, mpsc};
use tokio_stream::{StreamExt, wrappers::UnboundedReceiverStream};
use uuid::Uuid;

/// Tracing target of this binary, and the scope of the default `[logging]` filter.
///
/// The module path `tracing` stamps on every event is `mcp_server`, so a filter written
/// against the binary's hyphenated name would silence the whole process.
const SERVICE_NAME: &str = "mcp_server";

/// Configuration key carrying the bearer token inbound HTTP/SSE callers must present.
///
/// Named in every diagnostic so a refusal says exactly what to edit. Deliberately not
/// exposed as a CLI flag: a flag would put the secret in `argv`, which any local process
/// can read out of `/proc`.
const INBOUND_AUTH_TOKEN_KEY: &str = "mcp.auth_token";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // The file is the only source of configuration; nothing below reads the environment.
    let mut config = OpenPrConfig::load(cli.config.as_deref())?;
    // stdio frames JSON-RPC on stdout, so the log stream is reserved to stderr no
    // matter what the file asks for.
    platform::logging::init_reserving_stdout(&config.logging, SERVICE_NAME)?;

    let serve_args = match &cli.command {
        Commands::Serve(args) => Some(args),
        _ => None,
    };
    apply_cli_overrides(&mut config.mcp, &cli, serve_args)?;

    // Lazy validation: `[mcp]` is optional for the other binaries, so the fields this one
    // cannot run without are reported here, all of them in one pass.
    let mcp = config.mcp_runtime()?;

    let client = OpenPrClient::new(ClientConfig {
        base_url: mcp.api_url.clone(),
        bot_token: mcp.bot_token.clone(),
        workspace_id: mcp.workspace_id.to_string(),
        invocation_id: mcp.invocation_id.clone(),
        transport_label: transport_label(mcp.transport),
    })
    .map_err(|e| anyhow::anyhow!(e))?;

    if serve_args.is_some() {
        serve(&mcp, client).await
    } else {
        cli::run_cli_command(&cli.command, &cli.format, client).await
    }
}

/// Layers the CLI overrides onto the file's `[mcp]` section.
///
/// A flag wins over the file: the file is the deployment's configuration, a flag is a
/// deliberate one-off. A value that arrives through a flag has not passed the file's
/// validation, so it is checked here — against the same rules, reported against the flag
/// that carried it. Values that came from the file are left alone: they are already
/// validated, and checking them twice is how one bad value grows two different messages.
fn apply_cli_overrides(mcp: &mut McpConfig, cli: &Cli, serve: Option<&ServeArgs>) -> anyhow::Result<()> {
    if let Some(api_url) = cli.api_url.as_deref() {
        mcp.api_url = Some(checked_api_url(api_url)?);
    }
    if let Some(bot_token) = cli.bot_token.as_deref() {
        mcp.bot_token = Some(checked_bot_token(bot_token)?);
    }
    if let Some(workspace_id) = cli.workspace_id.as_deref() {
        mcp.workspace_id = Some(checked_workspace_id(workspace_id)?);
    }
    if let Some(serve) = serve {
        if let Some(transport) = serve.transport {
            mcp.transport = transport.into();
        }
        if let Some(bind_addr) = serve.bind_addr.as_deref() {
            mcp.bind_addr = Some(checked_bind_addr(bind_addr)?);
        }
    }
    Ok(())
}

/// Validates an `--api-url`, mirroring the rules `mcp.api_url` is held to.
fn checked_api_url(value: &str) -> anyhow::Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.contains("${") {
        anyhow::bail!("--api-url must be a concrete URL, not a placeholder");
    }
    let parsed =
        reqwest::Url::parse(trimmed).map_err(|error| anyhow::anyhow!("--api-url is not a valid URL: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        anyhow::bail!("--api-url must start with http:// or https://");
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        anyhow::bail!("--api-url names no host");
    }
    Ok(trimmed.to_string())
}

/// Validates a `--bot-token`, mirroring the rules `mcp.bot_token` is held to.
///
/// The token itself is never echoed, not even its prefix.
fn checked_bot_token(value: &str) -> anyhow::Result<Secret> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.contains("${") || trimmed.contains("replace_with") {
        anyhow::bail!("--bot-token must be a concrete bot token");
    }
    if !trimmed.starts_with("opr_") {
        anyhow::bail!("--bot-token must use the opr_ token prefix");
    }
    Ok(Secret::new(trimmed))
}

/// Validates a `--workspace-id`, mirroring the rules `mcp.workspace_id` is held to.
fn checked_workspace_id(value: &str) -> anyhow::Result<Uuid> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.contains("${") || trimmed.contains("replace_with") {
        anyhow::bail!("--workspace-id must be a concrete UUID, not a placeholder");
    }
    let parsed =
        Uuid::parse_str(trimmed).map_err(|error| anyhow::anyhow!("--workspace-id is not a valid UUID: {error}"))?;
    if parsed.is_nil() {
        anyhow::bail!("--workspace-id must not be the nil UUID placeholder");
    }
    Ok(parsed)
}

/// Validates a `--bind-addr`, mirroring the rules `mcp.bind_addr` is held to.
///
/// A host without a port is refused rather than completed with one: the bind address is
/// what `resolve_inbound_auth` decides against, and a silently invented port would move
/// the listener away from the address that decision was made for.
fn checked_bind_addr(value: &str) -> anyhow::Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.contains("${") {
        anyhow::bail!("--bind-addr must be a concrete host:port, not a placeholder");
    }
    if trimmed.chars().any(char::is_whitespace) {
        anyhow::bail!("--bind-addr must not contain whitespace");
    }
    let (host, port) = split_host_port(trimmed)
        .ok_or_else(|| anyhow::anyhow!("--bind-addr must be host:port, e.g. 127.0.0.1:8090"))?;
    if host.is_empty() {
        anyhow::bail!("--bind-addr names no host");
    }
    match port.parse::<u16>() {
        Ok(0) | Err(_) => anyhow::bail!("--bind-addr has an invalid port {port}, expected 1-65535"),
        Ok(_) => Ok(trimmed.to_string()),
    }
}

/// Splits an authority into host and port, tolerating a bracketed IPv6 literal.
fn split_host_port(authority: &str) -> Option<(&str, &str)> {
    if let Some(end) = authority.rfind(']') {
        let port = authority.get(end + 1..)?.strip_prefix(':')?;
        return Some((authority.get(..=end)?, port));
    }
    authority.rsplit_once(':')
}

/// Starts the transport `[mcp]` selected.
///
/// `stdio` is a pipe pair owned by the parent process, so it carries no inbound
/// authentication question: whoever spawned the process is already the caller. The two
/// networked transports share the same routes and therefore take the same decision, and
/// neither reaches its listener before that decision has been made.
async fn serve(mcp: &McpRuntime, client: OpenPrClient) -> anyhow::Result<()> {
    match mcp.transport {
        McpTransport::Stdio => run_stdio(client).await,
        McpTransport::Http => {
            let auth = resolve_inbound_auth(&mcp.bind_addr, mcp.auth_token.as_ref())?;
            run_http(&mcp.bind_addr, client, auth).await
        }
        McpTransport::Sse => {
            let auth = resolve_inbound_auth(&mcp.bind_addr, mcp.auth_token.as_ref())?;
            run_sse(&mcp.bind_addr, client, auth).await
        }
    }
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
/// The length rule is a second line of defence, not the first: the configuration layer
/// already refuses a short `mcp.auth_token` at load time. It is kept, and worded from the
/// same constant, so the function stays fail-closed for any caller — a decision this
/// consequential must not depend on where its input came from.
fn resolve_inbound_auth(bind_addr: &str, configured_token: Option<&Secret>) -> anyhow::Result<InboundAuth> {
    if let Some(token) = configured_token
        .map(Secret::expose)
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        if token.len() < MIN_MCP_AUTH_TOKEN_LEN {
            // The token is never echoed, not even its prefix.
            anyhow::bail!(
                "{INBOUND_AUTH_TOKEN_KEY} must be at least {MIN_MCP_AUTH_TOKEN_LEN} characters long to be worth \
                 checking"
            );
        }
        return Ok(InboundAuth::BearerToken(Arc::from(token)));
    }

    if is_loopback_bind_addr(bind_addr) {
        tracing::warn!(
            bind_addr = %bind_addr,
            "{INBOUND_AUTH_TOKEN_KEY} is not set, so inbound MCP requests are NOT authenticated. This is allowed only \
             because the listener is bound to a loopback address and callers are limited to this host. Set \
             {INBOUND_AUTH_TOKEN_KEY} before publishing this port."
        );
        return Ok(InboundAuth::LoopbackOnly);
    }

    anyhow::bail!(
        "Refusing to serve MCP on non-loopback bind address '{bind_addr}' without inbound authentication: set \
         {INBOUND_AUTH_TOKEN_KEY} in the configuration file, or bind to 127.0.0.1 for local development. This process \
         holds a workspace bot token, so anyone able to reach the port would hold every permission that bot has."
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
        InboundAuth, MIN_MCP_AUTH_TOKEN_LEN, bearer_token, checked_api_url, checked_bind_addr, checked_bot_token,
        checked_workspace_id, constant_time_eq, is_loopback_bind_addr, resolve_inbound_auth,
    };
    use platform::config::{DEFAULT_MCP_API_URL, Secret};

    const TOKEN: &str = "an-inbound-token-long-enough";

    #[test]
    fn accepts_concrete_cli_overrides() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(checked_api_url(" http://api:8080 ")?, "http://api:8080");
        assert_eq!(
            checked_bot_token("opr_forms_mcp_test_token")?.expose(),
            "opr_forms_mcp_test_token"
        );
        assert_eq!(
            checked_workspace_id("550e8400-e29b-41d4-a716-446655440000")?.to_string(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(checked_bind_addr("0.0.0.0:8090")?, "0.0.0.0:8090");
        assert_eq!(checked_bind_addr("[::1]:8090")?, "[::1]:8090");
        Ok(())
    }

    #[test]
    fn the_api_url_default_still_targets_the_compose_api_port() {
        assert_eq!(DEFAULT_MCP_API_URL, "http://localhost:8081");
    }

    /// The shell templates a compose file used to interpolate are values, not configuration:
    /// reaching the process unexpanded means the deployment is broken, so they are refused
    /// rather than used as a hostname or a token.
    #[test]
    fn rejects_unexpanded_shell_templates_on_the_command_line() {
        assert!(checked_api_url("${OPENPR_API_URL:-http://api:8080}").is_err());
        assert!(checked_bot_token("${OPENPR_BOT_TOKEN:?set OPENPR_BOT_TOKEN}").is_err());
        assert!(checked_workspace_id("${OPENPR_WORKSPACE_ID:?set OPENPR_WORKSPACE_ID}").is_err());
        assert!(checked_bind_addr("${OPENPR_MCP_BIND_ADDR}").is_err());
    }

    #[test]
    fn rejects_placeholder_token_and_nil_workspace_on_the_command_line() {
        assert!(checked_bot_token("opr_replace_with_workspace_bot_token").is_err());
        assert!(checked_bot_token("some_other_prefix_token").is_err());
        assert!(checked_bot_token("").is_err());
        assert!(checked_workspace_id("00000000-0000-0000-0000-000000000000").is_err());
        assert!(checked_workspace_id("not-a-uuid").is_err());
    }

    /// A `--bot-token` failure must not print the token it rejected.
    #[test]
    fn a_rejected_bot_token_is_never_echoed() {
        let Err(error) = checked_bot_token("nope_secret_material_here") else {
            panic!("a token without the opr_ prefix must be refused");
        };
        assert!(!error.to_string().contains("secret_material"), "{error}");
    }

    #[test]
    fn rejects_api_urls_that_are_not_absolute_http_urls() {
        assert!(checked_api_url("ftp://api:8080").is_err());
        assert!(checked_api_url("api:8080").is_err());
        assert!(checked_api_url("").is_err());
    }

    #[test]
    fn rejects_bind_addresses_without_a_usable_port() {
        assert!(checked_bind_addr("0.0.0.0").is_err());
        assert!(checked_bind_addr("0.0.0.0:0").is_err());
        assert!(checked_bind_addr("0.0.0.0:not-a-port").is_err());
        assert!(checked_bind_addr(":8090").is_err());
        assert!(checked_bind_addr("0.0.0.0 :8090").is_err());
    }

    /// The combination this gate exists to make impossible: reachable from the network
    /// and unauthenticated. Binding wide open used to be a plain config choice, with the
    /// compose port mapping as the only thing between a workspace bot and the internet.
    #[test]
    fn a_non_loopback_bind_without_a_token_refuses_to_start() {
        let blank = Secret::new("   ");
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
                resolve_inbound_auth(bind_addr, Some(&blank)).is_err(),
                "{bind_addr} treated a blank token as configured auth"
            );
        }
    }

    #[test]
    fn a_non_loopback_bind_serves_once_a_token_is_configured() -> Result<(), Box<dyn std::error::Error>> {
        let auth = resolve_inbound_auth("0.0.0.0:8090", Some(&Secret::new(TOKEN)))?;
        assert!(matches!(auth, InboundAuth::BearerToken(_)));
        assert_eq!(auth.describe(), "bearer-token");
        Ok(())
    }

    /// Loopback stays usable without a token so local development is unaffected, and the
    /// mode is reported rather than assumed.
    #[test]
    fn a_loopback_bind_may_stay_unauthenticated() -> Result<(), Box<dyn std::error::Error>> {
        // A `None` token is what an absent `mcp.auth_token` resolves to.
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
        let short = Secret::new("x".repeat(MIN_MCP_AUTH_TOKEN_LEN - 1));
        let long = Secret::new("x".repeat(MIN_MCP_AUTH_TOKEN_LEN));
        assert!(resolve_inbound_auth("0.0.0.0:8090", Some(&short)).is_err());
        assert!(resolve_inbound_auth("127.0.0.1:8090", Some(&short)).is_err());
        assert!(resolve_inbound_auth("0.0.0.0:8090", Some(&long)).is_ok());
    }

    /// The failure message has to name the fix without ever printing the secret.
    #[test]
    fn the_fail_closed_message_names_the_config_key_and_never_a_token() {
        let Err(error) = resolve_inbound_auth("0.0.0.0:8090", None) else {
            panic!("a wide open bind without a token must not be accepted");
        };
        let message = error.to_string();
        assert!(message.contains("mcp.auth_token"), "{message}");
        assert!(message.contains("127.0.0.1"), "{message}");

        let Err(error) = resolve_inbound_auth("0.0.0.0:8090", Some(&Secret::new("short"))) else {
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
