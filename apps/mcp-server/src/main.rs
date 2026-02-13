use axum::{Json, Router, routing::post};
use clap::{Parser, ValueEnum};
use platform::logging;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, ValueEnum)]
enum Transport {
    Http,
    Stdio,
}

#[derive(Debug, Parser)]
struct McpArgs {
    #[arg(long, value_enum, default_value_t = Transport::Http)]
    transport: Transport,
    #[arg(long, default_value = "0.0.0.0:8090")]
    bind_addr: String,
}

#[derive(Debug, Deserialize)]
struct ToolCall {
    tool: String,
    input: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct ToolCallResult {
    ok: bool,
    tool: String,
    output: serde_json::Value,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = McpArgs::parse();
    logging::init("mcp-server");

    match args.transport {
        Transport::Http => run_http(&args.bind_addr).await,
        Transport::Stdio => run_stdio().await,
    }
}

async fn run_http(bind_addr: &str) -> anyhow::Result<()> {
    let app = Router::new().route("/mcp/tool.call", post(tool_call));
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(bind_addr = %bind_addr, "mcp http transport started");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn run_stdio() -> anyhow::Result<()> {
    tracing::info!("mcp stdio transport bootstrapped");
    Ok(())
}

async fn tool_call(Json(req): Json<ToolCall>) -> Json<ToolCallResult> {
    Json(ToolCallResult {
        ok: true,
        tool: req.tool,
        output: serde_json::json!({
            "echo": req.input,
            "status": "not_implemented"
        }),
    })
}
