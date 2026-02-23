mod db;
mod protocol;
mod server;
mod tools;

use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
use clap::{Parser, ValueEnum};
use platform::{app, config::AppConfig, logging};
use protocol::{JsonRpcRequest, JsonRpcResponse};
use server::McpServer;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Debug, Clone, ValueEnum)]
enum Transport {
    Http,
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
    logging::init("mcp-server");

    let cfg = AppConfig::from_env("mcp-server", &args.bind_addr)?;
    let db = app::connect_db(&cfg.database_url).await?;
    let state = platform::app::AppState { cfg, db };

    match args.transport {
        Transport::Http => run_http(&args.bind_addr, state).await,
        Transport::Stdio => run_stdio(state).await,
    }
}

async fn run_http(bind_addr: &str, state: platform::app::AppState) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/mcp/rpc", post(handle_jsonrpc))
        .route("/health", axum::routing::get(health_check))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(bind_addr = %bind_addr, "MCP HTTP transport started");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle_jsonrpc(
    axum::extract::State(state): axum::extract::State<platform::app::AppState>,
    Json(req): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let server = McpServer::new(state);
    let response = server.handle_request(req).await;
    Json(response)
}

async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}

async fn run_stdio(state: platform::app::AppState) -> anyhow::Result<()> {
    tracing::info!("MCP stdio transport started");

    let server = McpServer::new(state);
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
