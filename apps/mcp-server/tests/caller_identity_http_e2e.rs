//! End-to-end proof that a networked MCP request is served as *its own caller*, against the
//! shipped binary and a counting stand-in for the `OpenPR` API.
//!
//! The regression under test is not a missing check but a missing distinction. The MCP
//! server used to hold one workspace bot token and present it on every outbound call, so
//! every caller of the HTTP port acted as that single bot: the API could not tell two of them
//! apart, the project agent policy applied to all of them was the bot's, and the tool-call
//! audit recorded one identity no matter who had called. The bearer token the port asked for
//! was a shared secret — proof of admission, not of identity.
//!
//! So the assertions here are about *which token the backend saw*, per request. A test that
//! only checked that calls succeed would pass just as happily against the old behaviour.
//!
//! The stand-in API answers `GET /api/v1/projects/{id}/agent-policy` differently depending on
//! the bearer token it was called with, which is what lets one test show two callers getting
//! two different policy verdicts through one server process.

mod support;

use axum::{
    Json, Router,
    extract::Path,
    http::HeaderMap,
    routing::{get, post},
};
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
const PROJECT: &str = "22222222-2222-4222-8222-222222222222";
const FORM: &str = "33333333-3333-4333-8333-333333333333";

/// The bot token of caller A, who is allowed to list work items.
const ALICE_TOKEN: &str = "opr_alice_workspace_bot_token";
/// The bot token of caller B, whose policy disables the same tool.
const BOB_TOKEN: &str = "opr_bob_workspace_bot_token";
/// A token no policy is filed under, standing in for a credential the API rejects.
const STRANGER_TOKEN: &str = "opr_stranger_unknown_bot_token";
/// The identity a `stdio` deployment is configured with.
const CONFIGURED_TOKEN: &str = "opr_configured_server_bot_token";

/// Something the stand-in API knows and no caller should ever be told.
const API_INTERNAL_DETAIL: &str = "pg-primary-7.internal:5432 auth_backend=ldap";

/// Every bearer token the stand-in API was called with, in order, with the path it was
/// presented on.
type SeenCredentials = Arc<tokio::sync::Mutex<Vec<(String, String)>>>;

struct ApiProbe {
    seen: SeenCredentials,
    policy_hits: Arc<AtomicUsize>,
    form_hits: Arc<AtomicUsize>,
    audit_bodies: Arc<tokio::sync::Mutex<Vec<Value>>>,
}

impl ApiProbe {
    async fn tokens_for(&self, path_fragment: &str) -> Vec<String> {
        self.seen
            .lock()
            .await
            .iter()
            .filter(|(path, _)| path.contains(path_fragment))
            .map(|(_, token)| token.clone())
            .collect()
    }
}

/// The bearer token off an inbound request, or `"<none>"` so a missing one is visible in an
/// assertion failure rather than silently absent.
fn presented_token(headers: &HeaderMap) -> String {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("<none>")
        .to_string()
}

/// The agent policy filed for one bot token.
///
/// Alice may list work items; Bob may not; anybody else is not a bot this API knows, and is
/// answered the way the real API answers an unusable credential — HTTP 200 carrying a
/// `{code: 401}` envelope (`apps/api/src/error.rs`).
fn policy_for(token: &str) -> Value {
    match token {
        ALICE_TOKEN => json!({
            "code": 0,
            "message": "ok",
            "data": { "mcp": { "tool_registry": { "enabled_tools": ["work_items.list", "forms.get"] } } }
        }),
        BOB_TOKEN => json!({
            "code": 0,
            "message": "ok",
            "data": { "mcp": { "tool_registry": { "enabled_tools": ["forms.get"] } } }
        }),
        _ => json!({
            "code": 401,
            "message": format!("invalid bot token (looked it up in {API_INTERNAL_DETAIL})"),
            "data": null
        }),
    }
}

/// Starts the stand-in API and returns its base URL plus the probe.
async fn spawn_api() -> Result<(String, ApiProbe), Box<dyn Error>> {
    let seen: SeenCredentials = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let policy_hits = Arc::new(AtomicUsize::new(0));
    let form_hits = Arc::new(AtomicUsize::new(0));
    let audit_bodies = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    let policy_seen = Arc::clone(&seen);
    let policy_counter = Arc::clone(&policy_hits);
    let form_seen = Arc::clone(&seen);
    let form_counter = Arc::clone(&form_hits);
    let issues_seen = Arc::clone(&seen);
    let audit_seen = Arc::clone(&seen);
    let audit_log = Arc::clone(&audit_bodies);

    let router = Router::new()
        .route(
            "/api/v1/projects/{project_id}/agent-policy",
            get(move |Path(project_id): Path<String>, headers: HeaderMap| {
                let seen = Arc::clone(&policy_seen);
                let counter = Arc::clone(&policy_counter);
                async move {
                    let token = presented_token(&headers);
                    counter.fetch_add(1, Ordering::SeqCst);
                    seen.lock()
                        .await
                        .push((format!("/projects/{project_id}/agent-policy"), token.clone()));
                    Json(policy_for(&token))
                }
            }),
        )
        .route(
            "/api/v1/projects/{project_id}/issues",
            get(move |Path(project_id): Path<String>, headers: HeaderMap| {
                let seen = Arc::clone(&issues_seen);
                async move {
                    seen.lock()
                        .await
                        .push((format!("/projects/{project_id}/issues"), presented_token(&headers)));
                    Json(json!({ "code": 0, "message": "ok", "data": [{ "id": "issue-1", "title": "an issue" }] }))
                }
            }),
        )
        .route(
            "/api/v1/forms/{form_id}",
            get(move |Path(form_id): Path<String>, headers: HeaderMap| {
                let seen = Arc::clone(&form_seen);
                let counter = Arc::clone(&form_counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    seen.lock()
                        .await
                        .push((format!("/forms/{form_id}"), presented_token(&headers)));
                    Json(json!({
                        "code": 0,
                        "message": "ok",
                        "data": { "id": form_id, "project_id": PROJECT, "name": "a form" }
                    }))
                }
            }),
        )
        .route(
            "/api/v1/invocations/{invocation_id}/tool-calls",
            post(
                move |Path(invocation_id): Path<String>, headers: HeaderMap, Json(body): Json<Value>| {
                    let seen = Arc::clone(&audit_seen);
                    let log = Arc::clone(&audit_log);
                    async move {
                        seen.lock().await.push((
                            format!("/invocations/{invocation_id}/tool-calls"),
                            presented_token(&headers),
                        ));
                        log.lock().await.push(body);
                        Json(json!({ "code": 0, "message": "ok", "data": {} }))
                    }
                },
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
            seen,
            policy_hits,
            form_hits,
            audit_bodies,
        },
    ))
}

/// A live `mcp-server serve --transport http` child process.
struct HttpServer {
    child: Child,
    base_url: String,
    _config: ConfigFile,
}

impl HttpServer {
    /// `bot_token` is `None` for the deployments this file is about: a networked MCP server
    /// holds no identity, which is half of why a request cannot be served without one.
    fn spawn(api_url: &str, bind_addr: &str, bot_token: Option<&str>) -> Result<Self, Box<dyn Error>> {
        let config = write_config(&McpSettings {
            api_url,
            bot_token,
            workspace_id: WORKSPACE,
            transport: Some("http"),
            bind_addr: Some(bind_addr),
        })?;

        let child = Command::new(env!("CARGO_BIN_EXE_mcp-server"))
            .args(["serve", "--config"])
            .arg(config.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;

        Ok(Self {
            child,
            base_url: format!("http://{bind_addr}"),
            _config: config,
        })
    }

    async fn wait_until_ready(&self, client: &reqwest::Client) -> TestResult {
        for _ in 0..100 {
            if let Ok(response) = client.get(format!("{}/health", self.base_url)).send().await
                && response.status().is_success()
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err("MCP HTTP server never became ready".into())
    }

    /// One `tools/call`, made as `token`.
    async fn call_tool(
        &self,
        client: &reqwest::Client,
        token: &str,
        tool: &str,
        arguments: Value,
    ) -> Result<Value, Box<dyn Error>> {
        let response = client
            .post(format!("{}/mcp/rpc", self.base_url))
            .bearer_auth(token)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": tool, "arguments": arguments }
            }))
            .send()
            .await?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(format!("tools/call answered {}", response.status()).into());
        }
        Ok(response.json().await?)
    }

    async fn shutdown(mut self) -> TestResult {
        self.child.start_kill()?;
        tokio::time::timeout(Duration::from_secs(10), self.child.wait()).await??;
        Ok(())
    }
}

async fn free_bind_addr() -> Result<String, Box<dyn Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    drop(listener);
    Ok(addr.to_string())
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

/// The core claim: the token the backend sees is the token the caller sent.
///
/// Both the policy lookup and the tool's own call have to carry it. A policy read as the
/// server and a tool call as the caller would be the worst of both — the caller would be
/// authorized against somebody else's permissions.
#[tokio::test]
async fn the_callers_own_bot_token_is_what_reaches_the_backend() -> TestResult {
    let (api_url, probe) = spawn_api().await?;
    let bind_addr = free_bind_addr().await?;
    let server = HttpServer::spawn(&api_url, &bind_addr, None)?;
    let client = reqwest::Client::new();
    server.wait_until_ready(&client).await?;

    let response = server
        .call_tool(
            &client,
            ALICE_TOKEN,
            "work_items.list",
            json!({ "project_id": PROJECT }),
        )
        .await?;
    assert!(!is_error(&response), "alice's authorized call failed: {response}");

    let policy_tokens = probe.tokens_for("agent-policy").await;
    assert_eq!(
        policy_tokens,
        vec![ALICE_TOKEN.to_string()],
        "the project agent policy was not fetched with the caller's own token"
    );
    let issue_tokens = probe.tokens_for("/issues").await;
    assert_eq!(
        issue_tokens,
        vec![ALICE_TOKEN.to_string()],
        "the tool call itself was not made with the caller's own token"
    );

    // Nothing the server was configured with leaked into an outbound call, because on this
    // transport it was configured with no identity at all.
    let all_tokens: Vec<String> = probe.seen.lock().await.iter().map(|(_, token)| token.clone()).collect();
    assert!(
        !all_tokens
            .iter()
            .any(|token| token == CONFIGURED_TOKEN || token == "<none>"),
        "an outbound call was made as something other than the caller: {all_tokens:?}"
    );

    server.shutdown().await
}

/// Two callers, one server process, two different verdicts — which is the whole point.
///
/// Under the old model both callers presented the same shared secret and were served as the
/// same bot, so this test could not have been written at all: there was only ever one policy
/// to fetch.
#[tokio::test]
async fn two_callers_get_their_own_policy_verdicts_and_do_not_cross() -> TestResult {
    let (api_url, probe) = spawn_api().await?;
    let bind_addr = free_bind_addr().await?;
    let server = HttpServer::spawn(&api_url, &bind_addr, None)?;
    let client = reqwest::Client::new();
    server.wait_until_ready(&client).await?;

    let arguments = json!({ "project_id": PROJECT });

    // Alice's policy enables the tool.
    let alice = server
        .call_tool(&client, ALICE_TOKEN, "work_items.list", arguments.clone())
        .await?;
    assert!(!is_error(&alice), "alice was refused her own enabled tool: {alice}");

    // Bob's policy does not, on the same project, through the same process.
    let bob = server
        .call_tool(&client, BOB_TOKEN, "work_items.list", arguments.clone())
        .await?;
    assert!(is_error(&bob), "bob was served alice's permissions: {bob}");
    assert!(
        result_text(&bob).contains("disabled by project agent policy"),
        "bob's refusal is not a policy refusal: {}",
        result_text(&bob)
    );

    // And back to Alice, to catch a cache that would have pinned the first verdict.
    let alice_again = server
        .call_tool(&client, ALICE_TOKEN, "work_items.list", arguments)
        .await?;
    assert!(
        !is_error(&alice_again),
        "alice inherited bob's verdict on a second call: {alice_again}"
    );

    assert_eq!(
        probe.tokens_for("agent-policy").await,
        vec![ALICE_TOKEN.to_string(), BOB_TOKEN.to_string(), ALICE_TOKEN.to_string()],
        "each call must read the policy as its own caller"
    );
    // Bob's refusal must not have reached the data endpoint.
    assert_eq!(
        probe.tokens_for("/issues").await,
        vec![ALICE_TOKEN.to_string(), ALICE_TOKEN.to_string()],
        "a refused call still reached the work items endpoint"
    );

    server.shutdown().await
}

/// A credential the API rejects comes back as a refusal that says so, and says nothing else.
///
/// The MCP server never decides this: it forwards the token and relays the verdict. What it
/// must not relay is the API's own reasoning, which is written for an operator reading logs
/// and can name infrastructure the caller cannot reach.
#[tokio::test]
async fn a_credential_the_api_rejects_is_refused_without_leaking_the_backend() -> TestResult {
    let (api_url, probe) = spawn_api().await?;
    let bind_addr = free_bind_addr().await?;
    let server = HttpServer::spawn(&api_url, &bind_addr, None)?;
    let client = reqwest::Client::new();
    server.wait_until_ready(&client).await?;

    let response = server
        .call_tool(
            &client,
            STRANGER_TOKEN,
            "work_items.list",
            json!({ "project_id": PROJECT }),
        )
        .await?;
    assert!(is_error(&response), "an unknown bot token was served: {response}");

    let text = result_text(&response);
    assert!(
        text.contains("401"),
        "the refusal must report the API's verdict: {text}"
    );
    assert!(
        !text.contains(API_INTERNAL_DETAIL),
        "the refusal leaked the API's internals: {text}"
    );
    assert!(
        !text.contains(STRANGER_TOKEN),
        "the refusal echoed the caller's own token: {text}"
    );

    // It was refused at the policy gate, so it never reached the data endpoint.
    assert!(
        probe.tokens_for("/issues").await.is_empty(),
        "a call with a rejected credential still reached the work items endpoint"
    );

    server.shutdown().await
}

/// The ownership cache maps `form -> project`, which is a fact about the data and not about
/// the caller — so it is safe to share, but only if it never lets one caller skip a lookup
/// another caller's credential paid for. It does not, because the cache lives on the
/// `McpServer` built for one request.
///
/// The observable consequence is what is asserted: a second caller asking about the same form
/// causes a second backend lookup, made with *its own* token. A cache shared across callers
/// would show one lookup and would have turned "does this form exist" into a question
/// answerable without a valid credential.
#[tokio::test]
async fn the_ownership_cache_never_answers_for_a_different_caller() -> TestResult {
    let (api_url, probe) = spawn_api().await?;
    let bind_addr = free_bind_addr().await?;
    let server = HttpServer::spawn(&api_url, &bind_addr, None)?;
    let client = reqwest::Client::new();
    server.wait_until_ready(&client).await?;

    let arguments = json!({ "form_id": FORM });
    let alice = server
        .call_tool(&client, ALICE_TOKEN, "forms.get", arguments.clone())
        .await?;
    assert!(!is_error(&alice), "alice could not read the form: {alice}");

    let hits_after_alice = probe.form_hits.load(Ordering::SeqCst);
    assert!(hits_after_alice >= 1, "alice's call never resolved the form's owner");

    let bob = server.call_tool(&client, BOB_TOKEN, "forms.get", arguments).await?;
    assert!(!is_error(&bob), "bob could not read the form: {bob}");

    assert!(
        probe.form_hits.load(Ordering::SeqCst) > hits_after_alice,
        "bob's ownership lookup was answered from a cache alice's credential filled"
    );
    let form_tokens = probe.tokens_for("/forms/").await;
    assert!(
        form_tokens.contains(&ALICE_TOKEN.to_string()) && form_tokens.contains(&BOB_TOKEN.to_string()),
        "each caller's ownership lookup must be made as itself: {form_tokens:?}"
    );

    server.shutdown().await
}

/// The tool-call audit is reported as the caller, and reports what the caller did — never
/// what the caller presented. A credential in an audit payload would be a credential at rest
/// in the API's database.
#[tokio::test]
async fn the_tool_call_audit_carries_the_callers_identity_but_never_its_token() -> TestResult {
    let (api_url, probe) = spawn_api().await?;
    let bind_addr = free_bind_addr().await?;

    // `invocation_id` is what switches the audit report on.
    let config = support::write_config_with_extra_mcp_keys(
        &McpSettings {
            api_url: &api_url,
            bot_token: None,
            workspace_id: WORKSPACE,
            transport: Some("http"),
            bind_addr: Some(&bind_addr),
        },
        "invocation_id = \"inv-0001\"\n",
    )?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-server"))
        .args(["serve", "--config"])
        .arg(config.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;

    let client = reqwest::Client::new();
    let base_url = format!("http://{bind_addr}");
    for _ in 0..100 {
        if let Ok(response) = client.get(format!("{base_url}/health")).send().await
            && response.status().is_success()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let response = client
        .post(format!("{base_url}/mcp/rpc"))
        .bearer_auth(ALICE_TOKEN)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "work_items.list", "arguments": { "project_id": PROJECT } }
        }))
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let audit_tokens = probe.tokens_for("tool-calls").await;
    assert_eq!(
        audit_tokens,
        vec![ALICE_TOKEN.to_string()],
        "the audit report was not made as the caller"
    );

    let bodies = probe.audit_bodies.lock().await;
    assert_eq!(bodies.len(), 1, "expected exactly one audit report: {bodies:?}");
    for body in bodies.iter() {
        let rendered = serde_json::to_string(body)?;
        for token in [ALICE_TOKEN, BOB_TOKEN, STRANGER_TOKEN, CONFIGURED_TOKEN] {
            assert!(
                !rendered.contains(token),
                "the audit payload carried a token: {rendered}"
            );
        }
        assert!(
            !rendered.contains("Bearer") && !rendered.contains("uthorization"),
            "the audit payload carried an authorization header: {rendered}"
        );
        assert_eq!(
            body.get("tool_name").and_then(Value::as_str),
            Some("work_items.list"),
            "the audit payload lost the thing it exists to record: {rendered}"
        );
        assert_eq!(body.get("transport").and_then(Value::as_str), Some("mcp_http"));
    }
    drop(bodies);

    child.start_kill()?;
    tokio::time::timeout(Duration::from_secs(10), child.wait()).await??;
    Ok(())
}

/// stdio has no per-request header, so its identity is the configured one — and that is
/// already per-account, because one person runs one process from their own MCP client. The
/// assertion is that the configured token is what the backend sees, unchanged.
#[tokio::test]
async fn stdio_calls_the_backend_as_its_configured_bot_token() -> TestResult {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let (api_url, probe) = spawn_api().await?;
    let config = write_config(&McpSettings {
        api_url: &api_url,
        bot_token: Some(CONFIGURED_TOKEN),
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

    let mut stdin = child.stdin.take().ok_or("child stdin was not piped")?;
    let stdout = child.stdout.take().ok_or("child stdout was not piped")?;
    let mut reader = BufReader::new(stdout);

    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": "forms.get", "arguments": { "form_id": FORM } }
    });
    stdin.write_all(format!("{request}\n").as_bytes()).await?;
    stdin.flush().await?;

    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(20), reader.read_line(&mut line)).await??;
    let response: Value = serde_json::from_str(&line)?;

    // The configured bot is not one the stand-in API files a policy for, so the call is
    // refused — which is itself the proof that the configured token, and not something else,
    // is what was presented.
    assert!(is_error(&response), "an unknown configured bot was served: {response}");
    let form_tokens = probe.tokens_for("/forms/").await;
    assert_eq!(
        form_tokens,
        vec![CONFIGURED_TOKEN.to_string()],
        "stdio did not call the backend as its configured bot token"
    );
    assert!(
        probe.policy_hits.load(Ordering::SeqCst) >= 1,
        "the policy gate did not run on the stdio path"
    );

    drop(stdin);
    tokio::time::timeout(Duration::from_secs(10), child.wait()).await??;
    Ok(())
}
