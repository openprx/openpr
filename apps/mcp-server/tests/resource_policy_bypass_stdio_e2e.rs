//! End-to-end proof that `resources/read` is governed by the same project agent policy
//! as `tools/call`, over the real stdio transport.
//!
//! The regression under test: the project agent policy was enforced on `tools/call` only.
//! `resources/read` dispatched straight to the client, so every capability an
//! administrator disabled had a second, wide open door —
//! `openpr://projects/{id}/connectors` handed back the connector list, `endpoint` and
//! `auth_policy` blobs included, for a project whose policy refuses `connectors.list`.
//!
//! As in `policy_bypass_stdio_e2e.rs`, the backend hit counters are the real assertion: a
//! refusal that still reaches the API is not a refusal.

mod support;

use axum::{Json, Router, routing::get};
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
/// A project whose agent policy enables neither `connectors.list` nor `form_records.list`.
const DENIED_PROJECT: &str = "22222222-2222-4222-8222-222222222222";
/// A project whose agent policy enables both.
const ALLOWED_PROJECT: &str = "44444444-4444-4444-8444-444444444444";
/// A form owned by `DENIED_PROJECT`, used to prove ownership is resolved from the API
/// rather than read off the URI.
const DENIED_FORM: &str = "55555555-5555-4555-8555-555555555555";
const BOT_TOKEN: &str = "opr_resource_e2e_token";

/// The counters the assertions are made against.
struct ApiProbe {
    connector_list_hits: Arc<AtomicUsize>,
    form_records_hits: Arc<AtomicUsize>,
}

fn counting_route(counter: &Arc<AtomicUsize>, payload: Value) -> axum::routing::MethodRouter {
    let counter = Arc::clone(counter);
    get(move || {
        let counter = Arc::clone(&counter);
        let payload = payload.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Json(payload)
        }
    })
}

fn policy_route(enabled_tools: Value) -> axum::routing::MethodRouter {
    get(move || {
        let enabled_tools = enabled_tools.clone();
        async move {
            Json(json!({
                "code": 0,
                "message": "ok",
                "data": { "mcp": { "tool_registry": { "enabled_tools": enabled_tools } } }
            }))
        }
    })
}

async fn spawn_api() -> Result<(String, ApiProbe), Box<dyn Error>> {
    let connector_list_hits = Arc::new(AtomicUsize::new(0));
    let form_records_hits = Arc::new(AtomicUsize::new(0));

    let router = Router::new()
        .route(
            &format!("/api/v1/projects/{DENIED_PROJECT}/agent-policy"),
            // Deliberately non-empty: the refusal has to come from the policy contents,
            // not from an empty or missing registry.
            policy_route(json!(["work_items.list", "projects.get"])),
        )
        .route(
            &format!("/api/v1/projects/{ALLOWED_PROJECT}/agent-policy"),
            policy_route(json!(["connectors.list", "form_records.list"])),
        )
        .route(
            &format!("/api/v1/workspaces/{WORKSPACE}/connectors"),
            // Shaped like the real payload: `list_connectors` returns `auth_policy` and
            // `endpoint` unredacted (apps/api/src/routes/connector.rs).
            counting_route(
                &connector_list_hits,
                json!({
                    "code": 0,
                    "message": "ok",
                    "data": [{
                        "id": "33333333-3333-4333-8333-333333333333",
                        "workspace_id": WORKSPACE,
                        "project_id": DENIED_PROJECT,
                        "kind": "webhook",
                        "name": "payroll-webhook",
                        "endpoint": "https://internal.example/hooks/payroll",
                        "auth_policy": { "mode": "hmac", "secret_ref": "OPENPR_CONNECTOR_SECRET_W_X_DEFAULT" }
                    }]
                }),
            ),
        )
        .route(
            &format!("/api/v1/forms/{DENIED_FORM}"),
            get(|| async {
                Json(json!({
                    "code": 0,
                    "message": "ok",
                    "data": { "id": DENIED_FORM, "project_id": DENIED_PROJECT, "name": "payroll" }
                }))
            }),
        )
        .route(
            &format!("/api/v1/forms/{DENIED_FORM}/records"),
            counting_route(
                &form_records_hits,
                json!({
                    "code": 0,
                    "message": "ok",
                    "data": [{ "id": "66666666-6666-4666-8666-666666666666", "salary": "top-secret-salary" }]
                }),
            ),
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
            form_records_hits,
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
            stdout: BufReader::new(stdout),
            _config: config,
        })
    }

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

    async fn read_resource(&mut self, id: u32, uri: &str) -> Result<Value, Box<dyn Error>> {
        self.request(id, "resources/read", json!({ "uri": uri })).await
    }

    async fn initialize(&mut self) -> TestResult {
        let initialized = self
            .request(
                1,
                "initialize",
                json!({ "protocolVersion": "2024-11-05", "clientInfo": { "name": "e2e", "version": "0" } }),
            )
            .await?;
        assert!(initialized.get("result").is_some(), "initialize failed: {initialized}");
        Ok(())
    }

    async fn shutdown(mut self) -> TestResult {
        drop(self.stdin);
        tokio::time::timeout(std::time::Duration::from_secs(10), self.child.wait()).await??;
        Ok(())
    }
}

/// The whole response as text, so an assertion cannot miss a leak that landed in a field
/// the test did not think to look at.
fn body(response: &Value) -> String {
    response.to_string()
}

fn is_error(response: &Value) -> bool {
    response.get("error").is_some()
}

#[tokio::test]
async fn a_policy_disabled_capability_cannot_be_read_through_the_resource_door() -> TestResult {
    let (api_url, probe) = spawn_api().await?;
    let mut server = StdioServer::spawn(&api_url)?;
    server.initialize().await?;

    // Project addressed: the URI names the project directly.
    let denied = server
        .read_resource(10, &format!("openpr://projects/{DENIED_PROJECT}/connectors"))
        .await?;
    assert!(is_error(&denied), "the resource read was not refused: {denied}");
    let text = body(&denied);
    assert!(
        text.contains("disabled by project agent policy"),
        "refusal did not come from the policy: {text}"
    );
    assert!(
        !text.contains("payroll-webhook") && !text.contains("secret_ref") && !text.contains("internal.example"),
        "the refusal leaked connector data: {text}"
    );
    assert_eq!(
        probe.connector_list_hits.load(Ordering::SeqCst),
        0,
        "a refused resources/read still reached GET /api/v1/workspaces/{WORKSPACE}/connectors"
    );

    // Id addressed: the URI names a form, and the owning project is read back from the
    // API. A caller cannot pick the project this is judged against.
    let denied_form = server
        .read_resource(11, &format!("openpr://forms/{DENIED_FORM}/records"))
        .await?;
    assert!(is_error(&denied_form), "the form read was not refused: {denied_form}");
    let text = body(&denied_form);
    assert!(
        text.contains("disabled by project agent policy"),
        "refusal did not come from the policy: {text}"
    );
    assert!(
        !text.contains("top-secret-salary"),
        "the refusal leaked record data: {text}"
    );
    assert_eq!(
        probe.form_records_hits.load(Ordering::SeqCst),
        0,
        "a refused resources/read still reached GET /api/v1/forms/{{form_id}}/records"
    );

    server.shutdown().await
}

/// The gate is a policy check, not a blanket denial: the same door serves a project whose
/// policy enables the capability.
#[tokio::test]
async fn the_resource_door_still_serves_a_policy_enabled_project() -> TestResult {
    let (api_url, probe) = spawn_api().await?;
    let mut server = StdioServer::spawn(&api_url)?;
    server.initialize().await?;

    let allowed = server
        .read_resource(20, &format!("openpr://projects/{ALLOWED_PROJECT}/connectors"))
        .await?;
    assert!(!is_error(&allowed), "an authorized read was refused: {allowed}");
    assert!(
        body(&allowed).contains("payroll-webhook"),
        "authorized read returned no data: {}",
        body(&allowed)
    );
    assert_eq!(probe.connector_list_hits.load(Ordering::SeqCst), 1);

    server.shutdown().await
}

/// A project id that is not a canonical UUID is refused before it can be interpolated
/// into the API path, and the compile time guides stay readable without any API call.
#[tokio::test]
async fn resource_uris_are_validated_and_static_guides_stay_open() -> TestResult {
    let (api_url, probe) = spawn_api().await?;
    let mut server = StdioServer::spawn(&api_url)?;
    server.initialize().await?;

    let malformed = server
        .read_resource(30, "openpr://projects/not-a-uuid/connectors")
        .await?;
    assert!(is_error(&malformed), "a malformed project id was accepted: {malformed}");
    assert!(
        body(&malformed).contains("not a canonical UUID"),
        "{}",
        body(&malformed)
    );
    assert_eq!(
        probe.connector_list_hits.load(Ordering::SeqCst),
        0,
        "a malformed project id still reached the connector endpoint"
    );

    let guide = server.read_resource(31, "openpr://guides/agents").await?;
    assert!(!is_error(&guide), "the static agent guide was refused: {guide}");
    assert!(body(&guide).contains("OpenPR Agent Guide"), "{}", body(&guide));

    server.shutdown().await
}
