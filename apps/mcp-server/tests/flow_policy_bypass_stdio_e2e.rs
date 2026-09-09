//! Flow tool policy-bypass regression, per `mcp-surface-v1.md`'s "`PolicyScope` 实现" section:
//! "必须新增 `flow_policy_bypass_stdio_e2e.rs` ... 伪造 project、foreign object、projectless
//! object 塞 project claim ... HTTP/stdio/SSE 结果一致".
//!
//! `objects.get` is `PolicyScope::OwnedBy(OwnerLookup::FlowObject)`
//! (`apps/mcp-server/src/server.rs`), the one v0.4 Flow tool whose owner can legitimately be
//! absent. Four things must hold against the *shipped binary*, not against
//! `server.rs`'s in-process unit tests, which never drive JSON-RPC over a transport at all:
//!
//! 1. A caller may address an object that belongs to a project with no `project_id` claim at
//!    all — the owner is resolved from the API, not trusted from the caller.
//! 2. The same object with a *foreign* `project_id` claim is refused: naming a project that
//!    does not own the target is an attempted bypass, not an alternative routing.
//! 3. An object that belongs to no project at all is served workspace-wide, with no project
//!    policy consulted (`mcp-surface-v1.md` describes this as `project_id=None` falling back
//!    to `WorkspaceWide`).
//! 4. The same projectless object with *any* `project_id` claim is refused — there is no real
//!    owner to check the claim against, so it cannot be silently ignored
//!    (`mcp-surface-v1.md`: "如 payload 任意位置出现 project claim，必须拒绝而非忽略").
//!
//! stdio is covered for all four; HTTP and SSE are driven through the same case (2), the
//! security-load-bearing one (a caller that actively lies about ownership), to show the three
//! transports reach the identical policy gate rather than three copies of it.

mod support;

use axum::{Json, Router, extract::Path, routing::get, routing::post};
use serde_json::{Value, json};
use std::error::Error;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use support::{ConfigFile, McpSettings, write_config};
use tokio::process::{Child, Command};

type TestResult = Result<(), Box<dyn Error>>;

const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const BOT_TOKEN: &str = "opr_flow_policy_bypass_e2e_token";
const CALLER_TOKEN: &str = "opr_flow_policy_bypass_e2e_caller_token";

/// A Flow object owned by [`OWNING_PROJECT`].
const OWNED_OBJECT: &str = "22222222-2222-4222-8222-222222222222";
/// The real owner of [`OWNED_OBJECT`].
const OWNING_PROJECT: &str = "33333333-3333-4333-8333-333333333333";
/// A project [`OWNED_OBJECT`] does not belong to.
const FOREIGN_PROJECT: &str = "44444444-4444-4444-8444-444444444444";
/// A Flow object with no owning project at all.
const PROJECTLESS_OBJECT: &str = "55555555-5555-4555-8555-555555555555";

/// The stand-in Flow API: `GET /flow/objects/{id}` for the two fixed objects above, and
/// `GET /projects/{id}/agent-policy` answering "every tool enabled" for the one real project.
fn flow_router() -> Router {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/flow/objects",
            post(|| async {
                Json(json!({
                    "code": 0,
                    "message": "ok",
                    "data": {
                        "object": { "id": OWNED_OBJECT, "workspace_id": WORKSPACE, "project_id": null },
                        "event_id": "66666666-6666-4666-8666-666666666666"
                    }
                }))
            }),
        )
        .route(
            "/api/v1/flow/objects/{object_id}",
            get(|Path(object_id): Path<String>| async move {
                let (project_id, ok) = match object_id.as_str() {
                    OWNED_OBJECT => (Some(OWNING_PROJECT), true),
                    PROJECTLESS_OBJECT => (None, true),
                    _ => (None, false),
                };
                if !ok {
                    return Json(json!({ "code": 404, "message": "flow object not found", "data": null }));
                }
                Json(json!({
                    "code": 0,
                    "message": "ok",
                    "data": {
                        "id": object_id,
                        "workspace_id": WORKSPACE,
                        "project_id": project_id,
                        "object_type": "page",
                        "title": "irrelevant to policy",
                    }
                }))
            }),
        )
        .route(
            &format!("/api/v1/projects/{OWNING_PROJECT}/agent-policy"),
            get(|| async { Json(json!({ "code": 0, "data": { "mcp": {} } })) }),
        )
}

fn create_object_request() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 91,
        "method": "tools/call",
        "params": {
            "name": "objects.create",
            "arguments": {
                "workspace_id": WORKSPACE,
                "type": "page",
                "title": "same over all transports",
                "idempotency_key": "transport-parity-key"
            }
        }
    })
}

fn get_object_request(id: i64, object_id: &str, project_id: Option<&str>) -> Value {
    let mut arguments = json!({ "object_id": object_id });
    if let (Some(project_id), Some(object)) = (project_id, arguments.as_object_mut()) {
        object.insert("project_id".to_string(), json!(project_id));
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": "objects.get", "arguments": arguments }
    })
}

/// Starts the stand-in API on a random loopback port and returns its base URL.
///
/// Not reused from `mcp_server::client::test_api::spawn`: that helper is `#[cfg(test)]` inside
/// the library crate, which an integration test file links as a regular (non-test) dependency,
/// so it is invisible here — the same reason `cli_mode_e2e.rs`'s own `spawn_api` exists instead
/// of reusing it.
async fn spawn_router(router: Router) -> Result<String, Box<dyn Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(format!("http://{addr}"))
}

fn is_error(response: &Value) -> bool {
    response
        .get("result")
        .and_then(|result| result.get("isError"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn json_at<'a>(value: &'a Value, pointer: &str) -> Result<&'a Value, Box<dyn Error>> {
    value
        .pointer(pointer)
        .ok_or_else(|| format!("missing JSON pointer {pointer} in {value}").into())
}

// ---- stdio: all four scenarios ----

struct StdioClient {
    child: Child,
    stdin: tokio::process::ChildStdin,
    reader: tokio::io::BufReader<tokio::process::ChildStdout>,
    _config: ConfigFile,
}

impl StdioClient {
    fn spawn(api_url: &str) -> Result<Self, Box<dyn Error>> {
        let config = write_config(&McpSettings {
            api_url,
            bot_token: Some(BOT_TOKEN),
            workspace_id: WORKSPACE,
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
            reader: tokio::io::BufReader::new(stdout),
            _config: config,
        })
    }

    async fn call(&mut self, request: &Value) -> Result<Value, Box<dyn Error>> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        self.stdin.write_all(format!("{request}\n").as_bytes()).await?;
        self.stdin.flush().await?;
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(20), self.reader.read_line(&mut line)).await??;
        Ok(serde_json::from_str(&line)?)
    }

    async fn shutdown(mut self) -> TestResult {
        drop(self.stdin);
        tokio::time::timeout(Duration::from_secs(10), self.child.wait()).await??;
        Ok(())
    }
}

#[tokio::test]
async fn stdio_owned_object_with_no_claim_is_served() -> TestResult {
    let api_url = spawn_router(flow_router()).await?;
    let mut client = StdioClient::spawn(&api_url)?;
    let response = client.call(&get_object_request(1, OWNED_OBJECT, None)).await?;
    assert!(
        !is_error(&response),
        "an owned object with no claim was refused: {response}"
    );
    client.shutdown().await
}

#[tokio::test]
async fn stdio_owned_object_with_a_foreign_project_claim_is_refused() -> TestResult {
    let api_url = spawn_router(flow_router()).await?;
    let mut client = StdioClient::spawn(&api_url)?;
    let response = client
        .call(&get_object_request(1, OWNED_OBJECT, Some(FOREIGN_PROJECT)))
        .await?;
    assert!(
        is_error(&response),
        "a foreign project_id claim on an owned object was accepted: {response}"
    );
    client.shutdown().await
}

#[tokio::test]
async fn stdio_projectless_object_with_no_claim_is_served_workspace_wide() -> TestResult {
    let api_url = spawn_router(flow_router()).await?;
    let mut client = StdioClient::spawn(&api_url)?;
    let response = client.call(&get_object_request(1, PROJECTLESS_OBJECT, None)).await?;
    assert!(
        !is_error(&response),
        "a projectless object with no claim was refused: {response}"
    );
    client.shutdown().await
}

#[tokio::test]
async fn stdio_projectless_object_with_any_project_claim_is_refused() -> TestResult {
    let api_url = spawn_router(flow_router()).await?;
    let mut client = StdioClient::spawn(&api_url)?;
    // Any claim is foreign here, including the one real project this stand-in API knows about.
    let response = client
        .call(&get_object_request(1, PROJECTLESS_OBJECT, Some(OWNING_PROJECT)))
        .await?;
    assert!(
        is_error(&response),
        "a project_id claim on a projectless object was silently ignored: {response}"
    );
    client.shutdown().await
}

// ---- HTTP and SSE: the security-load-bearing case (foreign project claim) ----

async fn free_port() -> Result<u16, Box<dyn Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

struct NetworkedServer {
    child: Child,
    base_url: String,
    _config: ConfigFile,
}

impl NetworkedServer {
    async fn spawn(api_url: &str, transport: &str) -> Result<Self, Box<dyn Error>> {
        let bind_addr = format!("127.0.0.1:{}", free_port().await?);
        let config = write_config(&McpSettings {
            api_url,
            bot_token: None,
            workspace_id: WORKSPACE,
            transport: Some(transport),
            bind_addr: Some(&bind_addr),
        })?;
        let child = Command::new(env!("CARGO_BIN_EXE_mcp-server"))
            .args(["serve", "--config"])
            .arg(config.path())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let server = Self {
            child,
            base_url: format!("http://{bind_addr}"),
            _config: config,
        };
        server.wait_until_ready().await?;
        Ok(server)
    }

    async fn wait_until_ready(&self) -> TestResult {
        let client = reqwest::Client::new();
        for _ in 0..100 {
            if let Ok(response) = client.get(format!("{}/health", self.base_url)).send().await
                && response.status().is_success()
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err("MCP server never became ready".into())
    }

    async fn shutdown(mut self) -> TestResult {
        self.child.start_kill()?;
        tokio::time::timeout(Duration::from_secs(10), self.child.wait()).await??;
        Ok(())
    }
}

async fn http_call(server: &NetworkedServer, request: &Value) -> Result<Value, Box<dyn Error>> {
    Ok(reqwest::Client::new()
        .post(format!("{}/mcp/rpc", server.base_url))
        .bearer_auth(CALLER_TOKEN)
        .json(request)
        .send()
        .await?
        .json()
        .await?)
}

async fn sse_call(server: &NetworkedServer, request: &Value) -> Result<Value, Box<dyn Error>> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpStream;

    let addr = server
        .base_url
        .strip_prefix("http://")
        .ok_or("base_url has no http:// prefix")?;
    let stream = TcpStream::connect(addr).await?;
    let (read_half, mut write_half) = stream.into_split();
    write_half
        .write_all(
            format!(
                "GET /sse HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {CALLER_TOKEN}\r\nConnection: keep-alive\r\n\r\n"
            )
            .as_bytes(),
        )
        .await?;
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    loop {
        line.clear();
        tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut line)).await??;
        if line == "\r\n" {
            break;
        }
    }
    let endpoint_path = loop {
        line.clear();
        tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut line)).await??;
        if let Some(rest) = line.strip_prefix("data: ") {
            break rest.trim().to_string();
        }
    };
    let accepted = reqwest::Client::new()
        .post(format!("{}{endpoint_path}", server.base_url))
        .bearer_auth(CALLER_TOKEN)
        .json(request)
        .send()
        .await?;
    if accepted.status() != reqwest::StatusCode::ACCEPTED {
        return Err(format!("POST /messages returned {}", accepted.status()).into());
    }
    let mut buf = Vec::new();
    loop {
        let mut byte = [0_u8; 1];
        tokio::time::timeout(Duration::from_secs(10), reader.read_exact(&mut byte)).await??;
        buf.push(byte[0]);
        let text = String::from_utf8_lossy(&buf);
        if let Some(index) = text.find("data: ")
            && let Some(end) = text[index..].find('\n')
        {
            let candidate = text[index + "data: ".len()..index + end].trim();
            if let Ok(value) = serde_json::from_str(candidate) {
                return Ok(value);
            }
        }
    }
}

#[tokio::test]
async fn one_flow_write_has_identical_semantic_results_over_stdio_http_and_sse() -> TestResult {
    let api_url = spawn_router(flow_router()).await?;
    let request = create_object_request();

    let mut stdio = StdioClient::spawn(&api_url)?;
    let stdio_result = stdio.call(&request).await?;
    stdio.shutdown().await?;

    let http = NetworkedServer::spawn(&api_url, "http").await?;
    let http_result = http_call(&http, &request).await?;
    http.shutdown().await?;

    let sse = NetworkedServer::spawn(&api_url, "sse").await?;
    let sse_result = sse_call(&sse, &request).await?;
    sse.shutdown().await?;

    assert!(!is_error(&stdio_result), "stdio write failed: {stdio_result}");
    assert_eq!(stdio_result.get("result"), http_result.get("result"));
    assert_eq!(stdio_result.get("result"), sse_result.get("result"));
    Ok(())
}

#[tokio::test]
async fn stdio_bot_can_write_flow_but_has_no_ticket_or_direct_ws_surface() -> TestResult {
    let api_url = spawn_router(flow_router()).await?;
    let mut stdio = StdioClient::spawn(&api_url)?;

    let write = stdio.call(&create_object_request()).await?;
    assert!(!is_error(&write), "configured stdio bot could not write Flow: {write}");

    let listed = stdio
        .call(&json!({
            "jsonrpc": "2.0",
            "id": 93,
            "method": "tools/list",
            "params": {}
        }))
        .await?;
    let names = json_at(&listed, "/result/tools")?
        .as_array()
        .ok_or("tools/list returned no tools array")?
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert!(names.contains(&"objects.create"));
    for forbidden in ["collab.ticket", "collab.tickets", "collab.ws", "collab.direct_ws"] {
        assert!(
            !names.contains(&forbidden),
            "stdio exposed forbidden direct-collaboration surface {forbidden}"
        );
    }

    stdio.shutdown().await
}

#[tokio::test]
async fn source_policy_cannot_override_the_apis_target_side_denial() -> TestResult {
    let command_calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&command_calls);
    let router = Router::new()
        .route(
            "/api/v1/flow/objects/{object_id}",
            get(|| async {
                Json(json!({
                    "code": 0,
                    "data": { "id": OWNED_OBJECT, "workspace_id": WORKSPACE, "project_id": OWNING_PROJECT }
                }))
            }),
        )
        .route(
            &format!("/api/v1/projects/{OWNING_PROJECT}/agent-policy"),
            get(|| async {
                Json(json!({
                    "code": 0,
                    "data": { "mcp": { "tool_registry": { "enabled_tools": ["objects.link"] } } }
                }))
            }),
        )
        .route(
            "/api/v1/flow/objects/{object_id}/commands",
            post(move || {
                let counter = Arc::clone(&counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "code": 403,
                        "message": "target object is unavailable",
                        "data": null,
                        "error_code": "policy_rejected",
                        "details": { "action": "link" }
                    }))
                }
            }),
        );
    let api_url = spawn_router(router).await?;
    let mut stdio = StdioClient::spawn(&api_url)?;
    let response = stdio
        .call(&json!({
            "jsonrpc": "2.0",
            "id": 92,
            "method": "tools/call",
            "params": {
                "name": "objects.link",
                "arguments": {
                    "source_object_id": OWNED_OBJECT,
                    "target_object_id": PROJECTLESS_OBJECT,
                    "relation_type": "related_to",
                    "idempotency_key": "denied-target"
                }
            }
        }))
        .await?;
    stdio.shutdown().await?;

    assert!(
        is_error(&response),
        "target-side denial was turned into success: {response}"
    );
    assert_eq!(
        command_calls.load(Ordering::SeqCst),
        1,
        "the API must make the target-side decision"
    );
    let text = json_at(&response, "/result/content/0/text")?
        .as_str()
        .ok_or("business error has no text content")?;
    let error: Value = serde_json::from_str(text)?;
    assert_eq!(json_at(&error, "/error/code")?, "policy_rejected");
    assert_eq!(json_at(&error, "/error/details/action")?, "link");
    Ok(())
}

#[tokio::test]
async fn http_owned_object_with_a_foreign_project_claim_is_refused() -> TestResult {
    let api_url = spawn_router(flow_router()).await?;
    let server = NetworkedServer::spawn(&api_url, "http").await?;
    let client = reqwest::Client::new();

    let response: Value = client
        .post(format!("{}/mcp/rpc", server.base_url))
        .bearer_auth(CALLER_TOKEN)
        .json(&get_object_request(1, OWNED_OBJECT, Some(FOREIGN_PROJECT)))
        .send()
        .await?
        .json()
        .await?;
    assert!(
        is_error(&response),
        "HTTP accepted a foreign project_id claim on an owned object: {response}"
    );

    server.shutdown().await
}

#[tokio::test]
async fn sse_owned_object_with_a_foreign_project_claim_is_refused() -> TestResult {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpStream;

    let api_url = spawn_router(flow_router()).await?;
    let server = NetworkedServer::spawn(&api_url, "sse").await?;
    let addr = server
        .base_url
        .strip_prefix("http://")
        .ok_or("base_url has no http:// prefix")?;

    let stream = TcpStream::connect(addr).await?;
    let (read_half, mut write_half) = stream.into_split();
    write_half
        .write_all(
            format!(
                "GET /sse HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {CALLER_TOKEN}\r\nConnection: keep-alive\r\n\r\n"
            )
            .as_bytes(),
        )
        .await?;
    let mut reader = BufReader::new(read_half);

    // Drain the HTTP status/header lines, then read the `event: endpoint` frame's `data:` line
    // to learn where `tools/call` must be posted.
    let mut line = String::new();
    loop {
        line.clear();
        tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut line)).await??;
        if line == "\r\n" {
            break;
        }
    }
    let endpoint_path = loop {
        line.clear();
        tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut line)).await??;
        if let Some(rest) = line.strip_prefix("data: ") {
            break rest.trim().to_string();
        }
    };

    let post_url = format!("{}{endpoint_path}", server.base_url);
    let client = reqwest::Client::new();
    let accepted = client
        .post(&post_url)
        .bearer_auth(CALLER_TOKEN)
        .json(&get_object_request(1, OWNED_OBJECT, Some(FOREIGN_PROJECT)))
        .send()
        .await?;
    assert_eq!(
        accepted.status(),
        reqwest::StatusCode::ACCEPTED,
        "POST /messages was not accepted"
    );

    // The JSON-RPC result itself arrives on the SSE stream as an `event: message` frame.
    let mut buf = Vec::new();
    let payload = loop {
        let mut byte = [0_u8; 1];
        tokio::time::timeout(Duration::from_secs(10), reader.read_exact(&mut byte)).await??;
        buf.push(byte[0]);
        let text = String::from_utf8_lossy(&buf);
        if let Some(index) = text.find("data: ")
            && let Some(end) = text[index..].find('\n')
        {
            let candidate = text[index + "data: ".len()..index + end].trim().to_string();
            if let Ok(value) = serde_json::from_str::<Value>(&candidate) {
                break value;
            }
        }
    };

    assert!(
        is_error(&payload),
        "SSE accepted a foreign project_id claim on an owned object: {payload}"
    );

    server.shutdown().await
}
