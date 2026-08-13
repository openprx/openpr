//! End-to-end proof that the project agent policy gate cannot be skipped over the real
//! stdio transport.
//!
//! Everything here runs against the *shipped binary*: the test starts a counting stand-in
//! for the `OpenPR` API, spawns `mcp-server serve --transport stdio` as a child process and
//! speaks line delimited JSON-RPC to it. Unit tests call `McpServer::call_tool` directly,
//! so they cannot prove that the transport a real MCP client uses goes through the same
//! gate; this one can.
//!
//! The regression under test: `connectors.list` shipped as
//! `PolicyScope::DeclaredProject { required: false }`. Leaving `project_id` out made the
//! policy lookup return "no project", the gate returned `Ok(())` without reading any
//! policy, and the tool still called the workspace addressed endpoint — handing back every
//! connector in the workspace, endpoints and `auth_policy` blobs included. The backend hit
//! counter is therefore the real assertion: a refusal that still reaches the API is not a
//! refusal.

mod support;

use axum::{Json, Router, extract::RawQuery, routing::get};
use serde_json::{Value, json};
use std::error::Error;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use support::{ConfigFile, McpSettings, write_config};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

type TestResult = Result<(), Box<dyn Error>>;

const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const PROJECT: &str = "22222222-2222-4222-8222-222222222222";
const BOT_TOKEN: &str = "opr_stdio_e2e_token";

/// The counters the assertions are made against.
struct ApiProbe {
    connector_list_hits: Arc<AtomicUsize>,
    policy_hits: Arc<AtomicUsize>,
    queries: Arc<tokio::sync::Mutex<Vec<String>>>,
}

/// Starts the stand-in API and returns its base URL plus the probe.
async fn spawn_api() -> Result<(String, ApiProbe), Box<dyn Error>> {
    let connector_list_hits = Arc::new(AtomicUsize::new(0));
    let policy_hits = Arc::new(AtomicUsize::new(0));
    let queries = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    let connector_counter = Arc::clone(&connector_list_hits);
    let query_log = Arc::clone(&queries);
    let policy_counter = Arc::clone(&policy_hits);

    let router = Router::new()
        .route(
            &format!("/api/v1/workspaces/{WORKSPACE}/connectors"),
            get(move |RawQuery(query): RawQuery| {
                let counter = Arc::clone(&connector_counter);
                let query_log = Arc::clone(&query_log);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    query_log.lock().await.push(query.unwrap_or_default());
                    // Shaped like the real payload: `list_connectors` returns `auth_policy`
                    // and `endpoint` unredacted (apps/api/src/routes/connector.rs).
                    Json(json!({
                        "code": 0,
                        "message": "ok",
                        "data": [{
                            "id": "33333333-3333-4333-8333-333333333333",
                            "workspace_id": WORKSPACE,
                            "project_id": PROJECT,
                            "kind": "webhook",
                            "name": "payroll-webhook",
                            "endpoint": "https://internal.example/hooks/payroll",
                            "auth_policy": { "mode": "hmac", "secret_ref": "OPENPR_CONNECTOR_SECRET_W_X_DEFAULT" }
                        }]
                    }))
                }
            }),
        )
        .route(
            &format!("/api/v1/projects/{PROJECT}/agent-policy"),
            get(move || {
                let counter = Arc::clone(&policy_counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "code": 0,
                        "message": "ok",
                        "data": { "mcp": { "tool_registry": { "enabled_tools": ["connectors.list"] } } }
                    }))
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    Ok((
        format!("http://{addr}"),
        ApiProbe {
            connector_list_hits,
            policy_hits,
            queries,
        },
    ))
}

/// A live `mcp-server serve --transport stdio` child process.
struct StdioServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    /// Kept alive for as long as the child runs: dropping it deletes the file.
    _config: ConfigFile,
}

impl StdioServer {
    /// The child is configured exactly the way a deployment configures it — a TOML file
    /// passed with `--config`, no environment variables involved.
    fn spawn(api_url: &str) -> Result<Self, Box<dyn Error>> {
        let config = write_config(&McpSettings {
            api_url,
            bot_token: BOT_TOKEN,
            workspace_id: WORKSPACE,
            auth_token: None,
            transport: Some("stdio"),
            bind_addr: None,
        })?;

        let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-server"))
            .args(["serve", "--config"])
            .arg(config.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;

        let stdin = child.stdin.take().ok_or("child stdin was not piped")?;
        let stdout = child.stdout.take().ok_or("child stdout was not piped")?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            _config: config,
        })
    }

    /// Sends one line delimited JSON-RPC request and returns its response.
    async fn request(&mut self, id: u32, method: &str, params: Value) -> Result<Value, Box<dyn Error>> {
        let line = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }))?;
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;

        let mut response = String::new();
        let read =
            tokio::time::timeout(std::time::Duration::from_secs(20), self.stdout.read_line(&mut response)).await??;
        if read == 0 {
            return Err(format!("stdio server closed stdout while answering {method}").into());
        }
        Ok(serde_json::from_str(&response)?)
    }

    async fn shutdown(mut self) -> TestResult {
        drop(self.stdin);
        tokio::time::timeout(std::time::Duration::from_secs(10), self.child.wait()).await??;
        Ok(())
    }
}

/// The `content[0].text` of a `tools/call` result.
fn result_text(response: &Value) -> String {
    response
        .get("result")
        .and_then(|result| result.get("content"))
        .and_then(Value::as_array)
        .and_then(|content| content.first())
        .and_then(|item| item.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn is_error(response: &Value) -> bool {
    response
        .get("result")
        .and_then(|result| result.get("isError"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

#[tokio::test]
async fn connectors_list_without_a_project_id_is_refused_over_stdio() -> TestResult {
    let (api_url, probe) = spawn_api().await?;
    let mut server = StdioServer::spawn(&api_url)?;

    let initialized = server
        .request(
            1,
            "initialize",
            json!({ "protocolVersion": "2024-11-05", "clientInfo": { "name": "e2e", "version": "0" } }),
        )
        .await?;
    assert!(initialized.get("result").is_some(), "initialize failed: {initialized}");

    // Every shape of "just leave the project out".
    let bypasses = [
        json!({}),
        json!({ "kind": "webhook" }),
        json!({ "project_id": null }),
        // A project id nested where it is caller data, not a routing decision.
        json!({ "context": { "project_id": PROJECT } }),
    ];
    for (offset, arguments) in bypasses.iter().enumerate() {
        let id = u32::try_from(offset)? + 10;
        let response = server
            .request(
                id,
                "tools/call",
                json!({ "name": "connectors.list", "arguments": arguments }),
            )
            .await?;
        assert!(is_error(&response), "{arguments} was not refused: {response}");
        let text = result_text(&response);
        assert!(text.contains("requires a project_id"), "{arguments} -> {text}");
        assert!(
            !text.contains("payroll-webhook") && !text.contains("secret_ref"),
            "the refusal leaked connector data: {text}"
        );
    }

    assert_eq!(
        probe.connector_list_hits.load(Ordering::SeqCst),
        0,
        "a refused call still reached GET /api/v1/workspaces/{WORKSPACE}/connectors"
    );
    assert_eq!(
        probe.policy_hits.load(Ordering::SeqCst),
        0,
        "no project was named, so no project policy could have been consulted"
    );

    // The same tool works once the caller names the project it is authorized for, and the
    // backend query is scoped to exactly that project.
    let allowed = server
        .request(
            20,
            "tools/call",
            json!({ "name": "connectors.list", "arguments": { "project_id": PROJECT } }),
        )
        .await?;
    assert!(!is_error(&allowed), "authorized call was refused: {allowed}");
    assert!(
        result_text(&allowed).contains("payroll-webhook"),
        "{}",
        result_text(&allowed)
    );
    assert_eq!(probe.policy_hits.load(Ordering::SeqCst), 1);
    assert_eq!(probe.connector_list_hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        probe.queries.lock().await.as_slice(),
        [format!("project_id={PROJECT}")],
        "the project the policy authorized must be the project the API is asked about"
    );

    server.shutdown().await
}

/// The published schema has to tell an MCP client that `project_id` is mandatory. If it
/// stays optional the model is invited to make exactly the call the gate then refuses, and
/// a future round could "fix" the friction by relaxing the gate again.
#[tokio::test]
async fn tools_list_publishes_project_id_as_required_for_connectors_list() -> TestResult {
    let (api_url, _probe) = spawn_api().await?;
    let mut server = StdioServer::spawn(&api_url)?;

    server
        .request(
            1,
            "initialize",
            json!({ "protocolVersion": "2024-11-05", "clientInfo": { "name": "e2e", "version": "0" } }),
        )
        .await?;

    let listed = server.request(2, "tools/list", json!({})).await?;
    let tools = listed
        .get("result")
        .and_then(|result| result.get("tools"))
        .and_then(Value::as_array)
        .ok_or("tools/list returned no tools")?;
    let connectors_list = tools
        .iter()
        .find(|tool| tool.get("name").and_then(Value::as_str) == Some("connectors.list"))
        .ok_or("connectors.list is not served")?;

    let required = connectors_list
        .get("inputSchema")
        .and_then(|schema| schema.get("required"))
        .and_then(Value::as_array)
        .ok_or("connectors.list publishes no required arguments")?;
    assert!(
        required.iter().any(|value| value.as_str() == Some("project_id")),
        "connectors.list must publish project_id as required: {connectors_list}"
    );

    server.shutdown().await
}
