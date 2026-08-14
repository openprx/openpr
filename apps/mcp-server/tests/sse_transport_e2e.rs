//! End-to-end proof that the SSE transport completes a whole round trip, against the shipped
//! binary and a counting stand-in for the `OpenPR` API.
//!
//! SSE is the one transport where the request and its answer travel on two different
//! connections: a `POST /messages` only *accepts* the JSON-RPC call, and the result is pushed
//! back over the `GET /sse` stream the client opened earlier. `http_auth_e2e.rs` already pins
//! down that `/sse` and `/messages` refuse an anonymous caller, but a refusal test cannot see
//! any of that: it never opens a stream, so it would pass just as happily against a server
//! that authenticated correctly and then never delivered an answer at all.
//!
//! So what is asserted here is the delivery semantics, per request:
//!
//! * the stream announces the endpoint its messages must be posted to,
//! * a `POST` to that endpoint answers `202 {"status":"accepted"}` and carries *no* result,
//! * the result arrives on the stream as an `event: message`,
//! * and each message is served as the caller that *posted it*, never as whoever opened the
//!   stream — which is the invariant that keeps one caller's identity off another's stream.
//!
//! Everything drives the shipped binary over real TCP, configured the way a deployment
//! configures it: a TOML file handed over with `--config`, and no environment variables.
//!
//! The stream is read over a raw socket rather than through an HTTP client, because the client
//! this workspace already depends on is built without its streaming feature. The frames axum
//! writes are `event: <name>\ndata: <payload>\n\n`, and chunked-transfer size lines cannot
//! collide with that shape, so the frames are located in the byte stream directly.

mod support;

use axum::{Json, Router, extract::Path, http::HeaderMap, routing::get};
use serde_json::{Value, json};
use std::error::Error;
use std::fmt::Write as _;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use support::{ConfigFile, McpSettings, write_config};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};

type TestResult = Result<(), Box<dyn Error>>;

const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";

/// The bot token of caller A, who opens the stream in most of these tests.
const ALICE_TOKEN: &str = "opr_alice_sse_workspace_bot_token";
/// The bot token of caller B, who posts onto a stream A opened.
const BOB_TOKEN: &str = "opr_bob_sse_workspace_bot_token";

/// The project name the stand-in API reports to each caller, so a result read off the stream
/// names the caller it was fetched for. Two callers that were served as the same identity
/// would come back with the same name here.
fn project_name_for(token: &str) -> &'static str {
    match token {
        ALICE_TOKEN => "alice-only-project",
        BOB_TOKEN => "bob-only-project",
        _ => "nobody-project",
    }
}

/// Every bearer token the stand-in API was called with, in order, with the path it arrived on.
type SeenCredentials = Arc<tokio::sync::Mutex<Vec<(String, String)>>>;

/// The counting stand-in for the `OpenPR` API.
///
/// Assertions are made against this rather than against the JSON a tool returned, because a
/// result that merely looks right proves nothing about *whose* credential fetched it.
struct ApiProbe {
    seen: SeenCredentials,
}

impl ApiProbe {
    /// Every token presented on a path containing `fragment`, in call order.
    async fn tokens_for(&self, fragment: &str) -> Vec<String> {
        self.seen
            .lock()
            .await
            .iter()
            .filter(|(path, _)| path.contains(fragment))
            .map(|(_, token)| token.clone())
            .collect()
    }

    /// How many outbound calls the server made in total.
    async fn call_count(&self) -> usize {
        self.seen.lock().await.len()
    }
}

/// The bearer token off an inbound request, or `"<none>"` so a missing one is visible in a
/// failure message rather than silently absent.
fn presented_token(headers: &HeaderMap) -> String {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("<none>")
        .to_string()
}

/// Starts the stand-in API on a random loopback port and returns its base URL plus the probe.
async fn spawn_api() -> Result<(String, ApiProbe), Box<dyn Error>> {
    let seen: SeenCredentials = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let projects_seen = Arc::clone(&seen);
    let labels_seen = Arc::clone(&seen);

    let router = Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/projects",
            get(move |Path(workspace_id): Path<String>, headers: HeaderMap| {
                let seen = Arc::clone(&projects_seen);
                async move {
                    let token = presented_token(&headers);
                    seen.lock()
                        .await
                        .push((format!("/workspaces/{workspace_id}/projects"), token.clone()));
                    Json(json!({
                        "code": 0,
                        "message": "ok",
                        "data": [{ "id": "project-1", "name": project_name_for(&token) }]
                    }))
                }
            }),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/labels",
            get(move |Path(workspace_id): Path<String>, headers: HeaderMap| {
                let seen = Arc::clone(&labels_seen);
                async move {
                    seen.lock()
                        .await
                        .push((format!("/workspaces/{workspace_id}/labels"), presented_token(&headers)));
                    Json(json!({ "code": 0, "message": "ok", "data": [] }))
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    Ok((format!("http://{addr}"), ApiProbe { seen }))
}

/// A live `mcp-server serve --transport sse` child process.
struct SseServer {
    child: Child,
    /// `host:port`, which the raw socket needs as well as the HTTP client.
    authority: String,
    _config: ConfigFile,
}

impl SseServer {
    /// Started with no `bot_token` at all: a networked deployment holds no identity, so a
    /// message that arrived without a caller credential has nothing to fall back onto.
    fn spawn(api_url: &str, bind_addr: &str) -> Result<Self, Box<dyn Error>> {
        let config = write_config(&McpSettings {
            api_url,
            bot_token: None,
            workspace_id: WORKSPACE,
            transport: Some("sse"),
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
            authority: bind_addr.to_string(),
            _config: config,
        })
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.authority)
    }

    async fn wait_until_ready(&self, client: &reqwest::Client) -> TestResult {
        for _ in 0..100 {
            if let Ok(response) = client.get(format!("{}/health", self.base_url())).send().await
                && response.status().is_success()
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err("MCP SSE server never became ready".into())
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

/// One open `GET /sse` connection, read straight off the socket.
struct SseStream {
    socket: TcpStream,
    /// Everything received so far, response head included.
    buffer: String,
    /// How far [`SseStream::next_event`] has already consumed, so successive calls return
    /// successive frames instead of re-reading the first one.
    cursor: usize,
}

impl SseStream {
    /// Opens the stream and reads far enough to know the HTTP status, which is returned
    /// alongside it so an anonymous attempt can be inspected without reading any frames.
    async fn open(authority: &str, token: Option<&str>) -> Result<(u16, Self), Box<dyn Error>> {
        let socket = TcpStream::connect(authority).await?;
        let mut stream = Self {
            socket,
            buffer: String::new(),
            cursor: 0,
        };

        let mut request = format!("GET /sse HTTP/1.1\r\nHost: {authority}\r\nAccept: text/event-stream\r\n");
        if let Some(token) = token {
            write!(request, "Authorization: Bearer {token}\r\n")?;
        }
        request.push_str("\r\n");
        stream.socket.write_all(request.as_bytes()).await?;
        stream.socket.flush().await?;

        let status = stream.read_status().await?;
        Ok((status, stream))
    }

    /// Appends the next batch of bytes, failing on EOF or on a stalled stream.
    async fn read_more(&mut self) -> TestResult {
        let mut chunk = [0_u8; 4096];
        let read = tokio::time::timeout(Duration::from_secs(20), self.socket.read(&mut chunk)).await??;
        if read == 0 {
            return Err("the SSE connection closed before the expected bytes arrived".into());
        }
        self.buffer
            .push_str(&String::from_utf8_lossy(chunk.get(..read).unwrap_or_default()));
        Ok(())
    }

    async fn read_status(&mut self) -> Result<u16, Box<dyn Error>> {
        while !self.buffer.contains("\r\n") {
            self.read_more().await?;
        }
        let line = self.buffer.split("\r\n").next().unwrap_or_default();
        let code = line
            .split(' ')
            .nth(1)
            .ok_or_else(|| format!("no status code in the response head: {line:?}"))?;
        Ok(code.parse::<u16>()?)
    }

    /// The `data:` payload of the next `event: <event_name>` frame on the stream.
    async fn next_event(&mut self, event_name: &str) -> Result<String, Box<dyn Error>> {
        let needle = format!("event: {event_name}\ndata: ");
        loop {
            if let Some(tail) = self.buffer.get(self.cursor..)
                && let Some(offset) = tail.find(&needle)
            {
                let payload_start = self.cursor + offset + needle.len();
                if let Some(rest) = self.buffer.get(payload_start..)
                    && let Some(end) = rest.find('\n')
                {
                    self.cursor = payload_start + end;
                    return Ok(self
                        .buffer
                        .get(payload_start..payload_start + end)
                        .unwrap_or_default()
                        .to_string());
                }
            }
            self.read_more().await?;
        }
    }

    /// The `/messages?session_id=...` endpoint the server announced, which is the only place a
    /// client is allowed to learn it from.
    async fn announced_endpoint(&mut self) -> Result<String, Box<dyn Error>> {
        self.next_event("endpoint").await
    }
}

fn initialize_request(id: i32) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": { "protocolVersion": "2024-11-05", "clientInfo": { "name": "sse-e2e", "version": "0" } }
    })
}

fn tools_call_request(id: i32, tool: &str, arguments: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": tool, "arguments": arguments }
    })
}

/// The `session_id` query parameter out of an announced endpoint.
fn session_id_of(endpoint: &str) -> Result<&str, Box<dyn Error>> {
    endpoint
        .split_once("session_id=")
        .map(|(_, id)| id)
        .ok_or_else(|| format!("the announced endpoint carries no session_id: {endpoint}").into())
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

/// The stream is refused to a caller who presents nothing, and so is a message posted to a
/// *real, live* endpoint — the session id is a delivery address, never a credential.
///
/// The last assertion is the one a status-code check cannot make on its own: nothing the
/// refused caller did reached the backend, so no outbound call was made on an unidentified
/// caller's behalf before the refusal.
#[tokio::test]
async fn an_anonymous_stream_and_an_anonymous_message_are_both_refused() -> TestResult {
    let (api_url, probe) = spawn_api().await?;
    let bind_addr = free_bind_addr().await?;
    let server = SseServer::spawn(&api_url, &bind_addr)?;
    let client = reqwest::Client::new();
    server.wait_until_ready(&client).await?;

    // 1. An anonymous `GET /sse` never becomes a stream.
    let (status, _anonymous) = SseStream::open(&server.authority, None).await?;
    assert_eq!(status, 401, "an anonymous GET /sse was upgraded to a stream");

    // 2. A live session id does not let an anonymous caller post onto it.
    let (opened, mut stream) = SseStream::open(&server.authority, Some(ALICE_TOKEN)).await?;
    assert_eq!(opened, 200, "an identified caller could not open a stream");
    let endpoint = stream.announced_endpoint().await?;

    let anonymous_post = client
        .post(format!("{}{endpoint}", server.base_url()))
        .json(&tools_call_request(1, "projects.list", &json!({})))
        .send()
        .await?;
    assert_eq!(
        anonymous_post.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "an anonymous message was accepted onto a live session"
    );
    assert_eq!(
        anonymous_post
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer"),
        "a 401 has to say how to authenticate"
    );
    let body = anonymous_post.text().await?;
    assert!(
        body.contains("Authorization: Bearer"),
        "the refusal must say what to present: {body}"
    );
    assert!(
        !body.contains(ALICE_TOKEN) && !body.contains(BOB_TOKEN),
        "the refusal echoed a token: {body}"
    );

    // 3. Neither refusal cost the backend a call.
    assert_eq!(
        probe.call_count().await,
        0,
        "a refused caller still reached the backend: {:?}",
        probe.tokens_for("/").await
    );

    server.shutdown().await
}

/// The core SSE semantics: the `POST` only accepts, and the answer comes back on the stream.
///
/// The assertion that the accept response carries no `result` is the point of the test. A
/// server that answered the JSON-RPC call inline from `POST /messages` would satisfy every
/// naive "did I get my result" check while being unusable by a real MCP client, which is
/// waiting on the stream and would hang forever.
#[tokio::test]
async fn an_authenticated_stream_announces_its_endpoint_and_answers_only_over_the_stream() -> TestResult {
    let (api_url, _probe) = spawn_api().await?;
    let bind_addr = free_bind_addr().await?;
    let server = SseServer::spawn(&api_url, &bind_addr)?;
    let client = reqwest::Client::new();
    server.wait_until_ready(&client).await?;

    let (status, mut stream) = SseStream::open(&server.authority, Some(ALICE_TOKEN)).await?;
    assert_eq!(status, 200, "an identified caller could not open a stream");

    // The endpoint frame is what tells a client where to post; without it the transport is
    // unusable no matter how correct everything downstream is.
    let endpoint = stream.announced_endpoint().await?;
    assert!(
        endpoint.starts_with("/messages?session_id="),
        "the announced endpoint is not a /messages address: {endpoint}"
    );
    let session_id = session_id_of(&endpoint)?;
    assert!(
        uuid::Uuid::parse_str(session_id).is_ok(),
        "the session id is not a UUID, so it is guessable: {session_id}"
    );

    let accepted = client
        .post(format!("{}{endpoint}", server.base_url()))
        .bearer_auth(ALICE_TOKEN)
        .json(&initialize_request(7))
        .send()
        .await?;
    assert_eq!(
        accepted.status(),
        reqwest::StatusCode::ACCEPTED,
        "a posted message was not merely accepted"
    );
    let accept_body: Value = accepted.json().await?;
    assert_eq!(
        accept_body,
        json!({ "status": "accepted" }),
        "the POST answered with more than an acknowledgement: {accept_body}"
    );
    assert!(
        accept_body.get("result").is_none() && accept_body.get("jsonrpc").is_none(),
        "the JSON-RPC answer came back from the POST instead of the stream: {accept_body}"
    );

    // And here is the answer, on the stream, where an MCP client is waiting for it.
    let pushed = stream.next_event("message").await?;
    let response: Value = serde_json::from_str(&pushed)?;
    assert_eq!(
        response.get("id").and_then(Value::as_i64),
        Some(7),
        "the pushed message does not answer the posted request: {response}"
    );
    assert_eq!(response.get("jsonrpc").and_then(Value::as_str), Some("2.0"));
    assert!(
        response
            .get("result")
            .and_then(|result| result.get("serverInfo"))
            .is_some(),
        "the pushed message carries no initialize result: {response}"
    );

    server.shutdown().await
}

/// A whole tool call over SSE, proved end to end: the credential the backend saw is the one
/// the `POST` carried, and the data it answered with came back over the stream.
#[tokio::test]
async fn a_tool_call_over_sse_reaches_the_backend_as_the_caller_that_posted_it() -> TestResult {
    let (api_url, probe) = spawn_api().await?;
    let bind_addr = free_bind_addr().await?;
    let server = SseServer::spawn(&api_url, &bind_addr)?;
    let client = reqwest::Client::new();
    server.wait_until_ready(&client).await?;

    let (_status, mut stream) = SseStream::open(&server.authority, Some(ALICE_TOKEN)).await?;
    let endpoint = stream.announced_endpoint().await?;

    let accepted = client
        .post(format!("{}{endpoint}", server.base_url()))
        .bearer_auth(ALICE_TOKEN)
        .json(&tools_call_request(11, "projects.list", &json!({})))
        .send()
        .await?;
    assert_eq!(accepted.status(), reqwest::StatusCode::ACCEPTED);

    let pushed = stream.next_event("message").await?;
    let response: Value = serde_json::from_str(&pushed)?;
    assert_eq!(response.get("id").and_then(Value::as_i64), Some(11));

    // The stand-in API answers with a project named after the credential it was called with,
    // so the payload itself carries the identity the call was made as.
    let text = result_text(&response);
    assert!(
        text.contains(project_name_for(ALICE_TOKEN)),
        "the streamed result was not fetched as the posting caller: {text}"
    );

    assert_eq!(
        probe.tokens_for("/projects").await,
        vec![ALICE_TOKEN.to_string()],
        "the outbound call was not made with the token the message carried"
    );

    server.shutdown().await
}

/// One stream, two callers, two identities — the invariant the transport is built around.
///
/// The session is a delivery channel and nothing else: identity comes from each `POST`, so a
/// message posted as Bob onto a stream Alice opened is served as *Bob*. Binding identity to
/// the stream instead would let whoever opened it lend their permissions to every message that
/// arrives on it afterwards, which is the failure this pins down.
#[tokio::test]
async fn two_callers_sharing_one_stream_are_each_served_as_themselves() -> TestResult {
    let (api_url, probe) = spawn_api().await?;
    let bind_addr = free_bind_addr().await?;
    let server = SseServer::spawn(&api_url, &bind_addr)?;
    let client = reqwest::Client::new();
    server.wait_until_ready(&client).await?;

    // The stream belongs to Alice, who opened it.
    let (_status, mut stream) = SseStream::open(&server.authority, Some(ALICE_TOKEN)).await?;
    let endpoint = stream.announced_endpoint().await?;
    let url = format!("{}{endpoint}", server.base_url());

    // Alice posts first, and is answered on her own stream.
    let alice_post = client
        .post(&url)
        .bearer_auth(ALICE_TOKEN)
        .json(&tools_call_request(1, "projects.list", &json!({})))
        .send()
        .await?;
    assert_eq!(alice_post.status(), reqwest::StatusCode::ACCEPTED);
    let alice_response: Value = serde_json::from_str(&stream.next_event("message").await?)?;
    assert_eq!(alice_response.get("id").and_then(Value::as_i64), Some(1));
    assert!(
        result_text(&alice_response).contains(project_name_for(ALICE_TOKEN)),
        "alice was not served as herself: {}",
        result_text(&alice_response)
    );

    // Bob posts onto the same session, and must be served as Bob.
    let bob_post = client
        .post(&url)
        .bearer_auth(BOB_TOKEN)
        .json(&tools_call_request(2, "projects.list", &json!({})))
        .send()
        .await?;
    assert_eq!(bob_post.status(), reqwest::StatusCode::ACCEPTED);
    let bob_response: Value = serde_json::from_str(&stream.next_event("message").await?)?;
    assert_eq!(bob_response.get("id").and_then(Value::as_i64), Some(2));
    let bob_text = result_text(&bob_response);
    assert!(
        bob_text.contains(project_name_for(BOB_TOKEN)),
        "bob's message was served as the caller who owns the stream: {bob_text}"
    );
    assert!(
        !bob_text.contains(project_name_for(ALICE_TOKEN)),
        "bob was served alice's data: {bob_text}"
    );

    // The backend is the witness: two calls, each as its own poster, in order.
    assert_eq!(
        probe.tokens_for("/projects").await,
        vec![ALICE_TOKEN.to_string(), BOB_TOKEN.to_string()],
        "the messages did not each reach the backend as their own poster"
    );

    server.shutdown().await
}

/// A session id nobody opened is refused, and the refusal happens before any work is done.
///
/// The identity check passes here — the caller is real — so this is the delivery check on its
/// own: an address that names no stream has nowhere to send an answer, and a server that ran
/// the call anyway would perform the side effect and drop the result on the floor.
#[tokio::test]
async fn an_unknown_session_id_is_refused_and_never_reaches_the_backend() -> TestResult {
    let (api_url, probe) = spawn_api().await?;
    let bind_addr = free_bind_addr().await?;
    let server = SseServer::spawn(&api_url, &bind_addr)?;
    let client = reqwest::Client::new();
    server.wait_until_ready(&client).await?;

    let response = client
        .post(format!(
            "{}/messages?session_id={}",
            server.base_url(),
            uuid::Uuid::new_v4()
        ))
        .bearer_auth(ALICE_TOKEN)
        .json(&tools_call_request(1, "projects.list", &json!({})))
        .send()
        .await?;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::NOT_FOUND,
        "a message for a session that was never opened was accepted"
    );
    let body: Value = response.json().await?;
    assert_eq!(
        body.get("error").and_then(Value::as_str),
        Some("Unknown SSE session_id"),
        "the refusal does not say the session is unknown: {body}"
    );

    assert_eq!(
        probe.call_count().await,
        0,
        "a message for an unknown session still ran its tool"
    );

    server.shutdown().await
}

/// A message posted after its stream is gone is refused rather than accepted.
///
/// Both refusals are correct and the server can legitimately answer either: dropping the
/// connection makes the receiver unreachable (`410`, "the stream is closed") *and* schedules
/// the session's removal from the registry (`404`, "no such stream"), and which one a message
/// observes depends on which lands first. What must never happen is `202` — an acknowledgement
/// for an answer that can no longer be delivered anywhere.
#[tokio::test]
async fn a_message_posted_after_its_stream_closed_is_refused_not_acknowledged() -> TestResult {
    let (api_url, _probe) = spawn_api().await?;
    let bind_addr = free_bind_addr().await?;
    let server = SseServer::spawn(&api_url, &bind_addr)?;
    let client = reqwest::Client::new();
    server.wait_until_ready(&client).await?;

    let (_status, mut stream) = SseStream::open(&server.authority, Some(ALICE_TOKEN)).await?;
    let endpoint = stream.announced_endpoint().await?;
    let url = format!("{}{endpoint}", server.base_url());

    // The session works while the stream is open, which is what makes the refusal below a
    // statement about the closure rather than about the address being wrong all along.
    let live = client
        .post(&url)
        .bearer_auth(ALICE_TOKEN)
        .json(&initialize_request(1))
        .send()
        .await?;
    assert_eq!(
        live.status(),
        reqwest::StatusCode::ACCEPTED,
        "the session did not work while its stream was open"
    );

    drop(stream);

    // The server notices the closed connection asynchronously, so the refusal is awaited.
    let mut last_status = reqwest::StatusCode::ACCEPTED;
    let mut last_body = Value::Null;
    for _ in 0..100 {
        let response = client
            .post(&url)
            .bearer_auth(ALICE_TOKEN)
            .json(&initialize_request(2))
            .send()
            .await?;
        last_status = response.status();
        last_body = response.json().await.unwrap_or(Value::Null);
        if last_status != reqwest::StatusCode::ACCEPTED {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert!(
        last_status == reqwest::StatusCode::GONE || last_status == reqwest::StatusCode::NOT_FOUND,
        "a message for a closed stream was answered {last_status}, not a refusal"
    );
    let error = last_body.get("error").and_then(Value::as_str).unwrap_or_default();
    assert!(
        error == "SSE session is closed" || error == "Unknown SSE session_id",
        "the refusal does not explain that the session is gone: {last_body}"
    );

    server.shutdown().await
}
