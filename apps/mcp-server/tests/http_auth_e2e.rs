//! End-to-end proof that the MCP server's own HTTP surface authenticates its callers,
//! and that it refuses to start in the one configuration that cannot be made safe.
//!
//! The regression under test: `/mcp/rpc`, `/sse` and `/messages` had no auth layer at
//! all. The bot token only authenticated this process *to* the API, so anyone who could
//! open a TCP connection to the port held every permission the workspace bot has. The
//! compose file binding the port to `127.0.0.1` was the only thing in the way, and a port
//! mapping is a deployment convenience, not a security boundary.
//!
//! Everything here drives the *shipped binary* over real TCP, configured the way a
//! deployment configures it: a TOML file handed over with `--config`, and no environment
//! variables at all.

mod support;

use serde_json::{Value, json};
use std::error::Error;
use std::process::Stdio;
use std::time::Duration;
use support::{ConfigFile, McpSettings, write_config};
use tokio::process::{Child, Command};

type TestResult = Result<(), Box<dyn Error>>;

const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const BOT_TOKEN: &str = "opr_http_auth_e2e_token";
const INBOUND_TOKEN: &str = "inbound-token-long-enough-to-pass";

/// Reserves a free localhost port by binding and releasing it.
async fn free_port() -> Result<u16, Box<dyn Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

/// A live `mcp-server serve` child process on a networked transport.
///
/// The transport, the bind address and the inbound token all come out of the
/// configuration file, so the listener under test is the one a deployment would get.
struct HttpServer {
    child: Child,
    base_url: String,
    /// Kept alive for as long as the child runs: dropping it deletes the file.
    _config: ConfigFile,
}

impl HttpServer {
    fn spawn(bind_addr: &str, inbound_token: Option<&str>) -> Result<Self, Box<dyn Error>> {
        Self::spawn_with(bind_addr, bind_addr, inbound_token, "http", &[])
    }

    /// `configured_bind` is what the file says, `reachable_at` is where the listener is
    /// expected to end up, and `extra_args` are appended after `--config` — so a test can
    /// hand the two different values and prove a flag overrides the file.
    fn spawn_with(
        configured_bind: &str,
        reachable_at: &str,
        inbound_token: Option<&str>,
        transport: &str,
        extra_args: &[&str],
    ) -> Result<Self, Box<dyn Error>> {
        let config = write_config(&McpSettings {
            // No API call is made by `initialize`, so the URL only has to be well formed.
            api_url: "http://127.0.0.1:1",
            bot_token: BOT_TOKEN,
            workspace_id: WORKSPACE,
            auth_token: inbound_token,
            transport: Some(transport),
            bind_addr: Some(configured_bind),
        })?;

        let mut command = Command::new(env!("CARGO_BIN_EXE_mcp-server"));
        command
            .args(["serve", "--config"])
            .arg(config.path())
            .args(extra_args)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let child = command.spawn()?;
        Ok(Self {
            child,
            base_url: format!("http://{reachable_at}"),
            _config: config,
        })
    }

    /// Waits until `/health` answers, which also proves `/health` needs no credentials.
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

    async fn shutdown(mut self) -> TestResult {
        self.child.start_kill()?;
        tokio::time::timeout(Duration::from_secs(10), self.child.wait()).await??;
        Ok(())
    }
}

fn initialize_request() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2024-11-05", "clientInfo": { "name": "e2e", "version": "0" } }
    })
}

/// With a token configured, only the exact token gets in — and `/health` still does not.
#[tokio::test]
async fn inbound_requests_need_the_configured_bearer_token() -> TestResult {
    let bind_addr = format!("127.0.0.1:{}", free_port().await?);
    let server = HttpServer::spawn(&bind_addr, Some(INBOUND_TOKEN))?;
    let client = reqwest::Client::new();
    server.wait_until_ready(&client).await?;

    let rpc_url = format!("{}/mcp/rpc", server.base_url);

    // No credentials at all.
    let anonymous = client.post(&rpc_url).json(&initialize_request()).send().await?;
    assert_eq!(
        anonymous.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "an anonymous JSON-RPC call was accepted"
    );
    assert_eq!(
        anonymous
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok()),
        Some("Bearer"),
        "a 401 has to say how to authenticate"
    );
    let body = anonymous.text().await?;
    assert!(!body.contains(INBOUND_TOKEN), "the rejection echoed the token: {body}");

    // Wrong token, right shape.
    let wrong = client
        .post(&rpc_url)
        .bearer_auth("inbound-token-long-enough-to-FAIL")
        .json(&initialize_request())
        .send()
        .await?;
    assert_eq!(
        wrong.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "a wrong token was accepted"
    );

    // A correct prefix of the real token must not be enough.
    let prefix = client
        .post(&rpc_url)
        .bearer_auth(INBOUND_TOKEN.get(..10).ok_or("token shorter than its own prefix")?)
        .json(&initialize_request())
        .send()
        .await?;
    assert_eq!(
        prefix.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "a prefix of the token was accepted"
    );

    // The wrong scheme is not a bearer token.
    let basic = client
        .post(&rpc_url)
        .header(reqwest::header::AUTHORIZATION, format!("Basic {INBOUND_TOKEN}"))
        .json(&initialize_request())
        .send()
        .await?;
    assert_eq!(
        basic.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "Basic auth was accepted"
    );

    // The real token works and the server answers real JSON-RPC.
    let authorized = client
        .post(&rpc_url)
        .bearer_auth(INBOUND_TOKEN)
        .json(&initialize_request())
        .send()
        .await?;
    assert_eq!(
        authorized.status(),
        reqwest::StatusCode::OK,
        "the correct token was refused"
    );
    let payload: Value = authorized.json().await?;
    assert!(
        payload
            .get("result")
            .and_then(|result| result.get("serverInfo"))
            .is_some(),
        "authorized call did not return an initialize result: {payload}"
    );

    // The SSE routes are behind the same layer.
    for path in ["/sse", "/messages"] {
        let response = client.get(format!("{}{path}", server.base_url)).send().await?;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "{path} was reachable without credentials"
        );
    }

    // `/health` is deliberately exempt: probes must not need the shared secret.
    let health = client.get(format!("{}/health", server.base_url)).send().await?;
    assert_eq!(
        health.status(),
        reqwest::StatusCode::OK,
        "/health should not require auth"
    );
    assert_eq!(health.text().await?, "OK");

    server.shutdown().await
}

/// Loopback without a token keeps working, so the change does not break local runs.
#[tokio::test]
async fn a_loopback_bind_without_a_token_still_serves_local_callers() -> TestResult {
    let bind_addr = format!("127.0.0.1:{}", free_port().await?);
    let server = HttpServer::spawn(&bind_addr, None)?;
    let client = reqwest::Client::new();
    server.wait_until_ready(&client).await?;

    let response = client
        .post(format!("{}/mcp/rpc", server.base_url))
        .json(&initialize_request())
        .send()
        .await?;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "an unauthenticated loopback call should still be served"
    );

    server.shutdown().await
}

/// The fail-closed case: bound to every interface with no inbound authentication, the
/// process must refuse to start rather than serve a workspace bot to the network.
#[tokio::test]
async fn a_non_loopback_bind_without_a_token_refuses_to_start() -> TestResult {
    let port = free_port().await?;
    let bind_addr = format!("0.0.0.0:{port}");
    let server = HttpServer::spawn(&bind_addr, None)?;

    let output = tokio::time::timeout(Duration::from_secs(20), server.child.wait_with_output()).await??;
    assert!(
        !output.status.success(),
        "serving 0.0.0.0 without inbound auth must not start (exit status {:?})",
        output.status
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("mcp.auth_token"),
        "the refusal must name the configuration key that fixes it: {stderr}"
    );
    assert!(
        stderr.contains(&bind_addr),
        "the refusal must name the offending bind address: {stderr}"
    );

    // Nothing is listening on that port: the refusal happens before the bind, so there is
    // no window in which the open configuration is briefly reachable.
    assert!(
        reqwest::Client::new()
            .get(format!("http://127.0.0.1:{port}/health"))
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .is_err(),
        "the refused configuration still bound a listener"
    );

    Ok(())
}

/// The same fail-closed rule covers the SSE transport, which shares the routes.
#[tokio::test]
async fn a_non_loopback_sse_bind_without_a_token_refuses_to_start() -> TestResult {
    let bind_addr = format!("0.0.0.0:{}", free_port().await?);
    let server = HttpServer::spawn_with(&bind_addr, &bind_addr, None, "sse", &[])?;

    let output = tokio::time::timeout(Duration::from_secs(20), server.child.wait_with_output()).await??;
    assert!(
        !output.status.success(),
        "SSE on 0.0.0.0 without inbound auth must not start"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("mcp.auth_token"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

/// `--bind-addr` beats `[mcp] bind_addr`, and the fail-closed gate is decided on the
/// address the listener actually gets. A flag that were checked against the file's address
/// would let `--bind-addr 0.0.0.0:...` walk straight past a loopback entry in the file.
#[tokio::test]
async fn a_bind_address_flag_overrides_the_file_and_still_faces_the_gate() -> TestResult {
    // The file says loopback, the flag says everywhere, and no inbound token is configured.
    let wide_open = format!("0.0.0.0:{}", free_port().await?);
    let server = HttpServer::spawn_with("127.0.0.1:8090", &wide_open, None, "http", &["--bind-addr", &wide_open])?;

    let output = tokio::time::timeout(Duration::from_secs(20), server.child.wait_with_output()).await??;
    assert!(
        !output.status.success(),
        "a flag that widens the bind address must face the same refusal"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&wide_open),
        "the refusal named the wrong address: {stderr}"
    );
    assert!(stderr.contains("mcp.auth_token"), "{stderr}");

    // And with the token supplied, the same flag serves — on the flag's address, not the
    // file's, which is what proves the override reached the listener.
    let bind_addr = format!("127.0.0.1:{}", free_port().await?);
    let serving = HttpServer::spawn_with(
        "127.0.0.1:8090",
        &bind_addr,
        Some(INBOUND_TOKEN),
        "http",
        &["--bind-addr", &bind_addr],
    )?;
    let client = reqwest::Client::new();
    serving.wait_until_ready(&client).await?;

    let authorized = client
        .post(format!("{}/mcp/rpc", serving.base_url))
        .bearer_auth(INBOUND_TOKEN)
        .json(&initialize_request())
        .send()
        .await?;
    assert_eq!(
        authorized.status(),
        reqwest::StatusCode::OK,
        "the overridden bind address did not serve"
    );

    serving.shutdown().await
}

/// stdio is a pipe pair owned by the parent process, so the inbound auth rule must not
/// leak into it: an MCP client launching the binary has no port and no token to present.
#[tokio::test]
async fn stdio_is_unaffected_by_the_inbound_auth_rule() -> TestResult {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    // Bound wide open in the file, and with no inbound token: on a networked transport
    // this exact configuration is refused, so serving it proves the rule stops at stdio
    // rather than leaking into a transport that has no port to protect.
    let config = write_config(&McpSettings {
        api_url: "http://127.0.0.1:1",
        bot_token: BOT_TOKEN,
        workspace_id: WORKSPACE,
        auth_token: None,
        transport: Some("stdio"),
        bind_addr: Some("0.0.0.0:8090"),
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

    stdin
        .write_all(format!("{}\n", initialize_request()).as_bytes())
        .await?;
    stdin.flush().await?;

    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(20), reader.read_line(&mut line)).await??;
    let response: Value = serde_json::from_str(&line)?;
    assert!(
        response.get("result").is_some(),
        "stdio initialize was refused: {response}"
    );

    drop(stdin);
    tokio::time::timeout(Duration::from_secs(10), child.wait()).await??;
    Ok(())
}

/// The environment is not a configuration source any more. Handed every variable the
/// server used to read and no file, it must refuse to start instead of quietly running on
/// values that are no longer part of the deployment's configuration.
#[tokio::test]
async fn the_retired_environment_variables_no_longer_configure_the_server() -> TestResult {
    let empty = support::write_config(&McpSettings {
        api_url: "http://127.0.0.1:1",
        bot_token: BOT_TOKEN,
        workspace_id: WORKSPACE,
        auth_token: None,
        transport: Some("stdio"),
        bind_addr: None,
    })?;
    // A directory that holds no `config/openpr.toml`, so the default path misses too.
    let empty_dir = empty.path().parent().ok_or("config file has no parent directory")?;

    let child = Command::new(env!("CARGO_BIN_EXE_mcp-server"))
        .args(["serve", "--transport", "stdio"])
        .current_dir(empty_dir)
        .env("OPENPR_API_URL", "http://127.0.0.1:1")
        .env("OPENPR_BOT_TOKEN", BOT_TOKEN)
        .env("OPENPR_WORKSPACE_ID", WORKSPACE)
        .env("OPENPR_MCP_AUTH_TOKEN", INBOUND_TOKEN)
        .env("OPENPR_MCP_TRANSPORT", "mcp_stdio")
        .env("OPENPR_INVOCATION_ID", "inv-0001")
        .env("RUST_LOG", "error")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output()).await??;
    assert!(
        !output.status.success(),
        "the environment alone started the server (exit status {:?})",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("configuration file not found"),
        "the failure must be the missing file, not something else: {stderr}"
    );
    assert!(
        !stderr.contains(INBOUND_TOKEN),
        "the failure echoed a token from the environment: {stderr}"
    );
    Ok(())
}
