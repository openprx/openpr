mod client;
mod protocol;
mod server;
mod tools;

use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
use clap::{Parser, ValueEnum};
use client::OpenPrClient;
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

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("mcp_server=info".parse().unwrap()),
        )
        .init();

    // Read configuration from environment
    let base_url = std::env::var("OPENPR_API_URL")
        .unwrap_or_else(|_| "http://localhost:3000".to_string());
    let bot_token = std::env::var("OPENPR_BOT_TOKEN")
        .map_err(|_| anyhow::anyhow!("OPENPR_BOT_TOKEN environment variable is required"))?;
    let workspace_id = std::env::var("OPENPR_WORKSPACE_ID")
        .map_err(|_| anyhow::anyhow!("OPENPR_WORKSPACE_ID environment variable is required"))?;

    let client = OpenPrClient::new(base_url, bot_token, workspace_id);

    match args.transport {
        Transport::Http => run_http(&args.bind_addr, client).await,
        Transport::Stdio => run_stdio(client).await,
    }
}

async fn run_http(bind_addr: &str, client: OpenPrClient) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/mcp/rpc", post(handle_jsonrpc))
        .route("/health", axum::routing::get(health_check))
        .with_state(client);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(bind_addr = %bind_addr, "MCP HTTP transport started");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle_jsonrpc(
    axum::extract::State(client): axum::extract::State<OpenPrClient>,
    Json(req): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let server = McpServer::new(client);
    let response = server.handle_request(req).await;
    Json(response)
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
