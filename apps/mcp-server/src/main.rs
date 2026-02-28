mod client;
mod protocol;
mod server;
mod tools;

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use clap::{Parser, ValueEnum};
use client::OpenPrClient;
use protocol::{JsonRpcRequest, JsonRpcResponse};
use server::McpServer;
use serde::Deserialize;
use serde_json::json;
use std::{collections::HashMap, convert::Infallible, sync::Arc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, mpsc};
use tokio_stream::{StreamExt, wrappers::UnboundedReceiverStream};
use uuid::Uuid;

#[derive(Debug, Clone, ValueEnum)]
enum Transport {
    Http,
    Sse,
    Stdio,
}

#[derive(Debug, Parser)]
struct McpArgs {
    #[arg(long, value_enum, default_value_t = Transport::Stdio)]
    transport: Transport,
    #[arg(long, default_value = "0.0.0.0:8090")]
    bind_addr: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = McpArgs::parse();

    // Initialize tracing — always write to stderr to keep stdout clean for JSON-RPC
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("mcp_server=info".parse().unwrap()),
        )
        .init();

    // Read configuration from environment
    let base_url =
        std::env::var("OPENPR_API_URL").unwrap_or_else(|_| "http://localhost:3000".to_string());
    let bot_token = std::env::var("OPENPR_BOT_TOKEN")
        .map_err(|_| anyhow::anyhow!("OPENPR_BOT_TOKEN environment variable is required"))?;
    let workspace_id = std::env::var("OPENPR_WORKSPACE_ID")
        .map_err(|_| anyhow::anyhow!("OPENPR_WORKSPACE_ID environment variable is required"))?;

    let client = OpenPrClient::new(base_url, bot_token, workspace_id);

    match args.transport {
        Transport::Http => run_http(&args.bind_addr, client).await,
        Transport::Sse => run_sse(&args.bind_addr, client).await,
        Transport::Stdio => run_stdio(client).await,
    }
}

async fn run_http(bind_addr: &str, client: OpenPrClient) -> anyhow::Result<()> {
    let state = SseState {
        client,
        sessions: Arc::new(Mutex::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/mcp/rpc", post(handle_jsonrpc))
        .route("/sse", get(handle_sse_connect))
        .route("/messages", post(handle_sse_message))
        .route("/health", get(health_check))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(
        bind_addr = %bind_addr,
        "MCP HTTP transport started (JSON-RPC + SSE)"
    );
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle_jsonrpc(
    State(state): State<SseState>,
    Json(req): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let server = McpServer::new(state.client.clone());
    match server.handle_request(req).await {
        Some(response) => (StatusCode::OK, Json(json!(response))),
        None => (StatusCode::ACCEPTED, Json(json!({"status": "accepted"}))),
    }
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

async fn run_sse(bind_addr: &str, client: OpenPrClient) -> anyhow::Result<()> {
    let state = SseState {
        client,
        sessions: Arc::new(Mutex::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/sse", get(handle_sse_connect))
        .route("/messages", post(handle_sse_message))
        .route("/health", get(health_check))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(bind_addr = %bind_addr, "MCP SSE transport started");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle_sse_connect(
    State(state): State<SseState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let session_id = Uuid::new_v4().to_string();
    let endpoint = format!("/messages?session_id={session_id}");
    let (tx, rx) = mpsc::unbounded_channel::<SseServerEvent>();

    state
        .sessions
        .lock()
        .await
        .insert(session_id.clone(), tx.clone());

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
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Unknown SSE session_id"})),
        );
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
        return (
            StatusCode::GONE,
            Json(json!({"error": "SSE session is closed"})),
        );
    }

    (StatusCode::ACCEPTED, Json(json!({"status": "accepted"})))
}

async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, "OK")
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
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                tracing::debug!(request = %line, "Received request");

                let request: JsonRpcRequest = match serde_json::from_str(line) {
                    Ok(req) => req,
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to parse request");
                        let error_response = JsonRpcResponse::error(
                            None,
                            protocol::JsonRpcError::parse_error(format!("Invalid JSON: {}", e)),
                        );
                        if let Ok(response_json) = serde_json::to_string(&error_response) {
                            stdout.write_all(response_json.as_bytes()).await?;
                            stdout.write_all(b"\n").await?;
                            stdout.flush().await?;
                        }
                        continue;
                    }
                };

                let response = server.handle_request(request).await;
                let Some(response) = response else {
                    continue;
                };

                match serde_json::to_string(&response) {
                    Ok(response_json) => {
                        tracing::debug!(response = %response_json, "Sending response");
                        stdout.write_all(response_json.as_bytes()).await?;
                        stdout.write_all(b"\n").await?;
                        stdout.flush().await?;
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
