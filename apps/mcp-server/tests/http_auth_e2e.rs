//! End-to-end proof that the MCP server's own HTTP surface authenticates its callers.
//!
//! The regression that started this file: `/mcp/rpc`, `/sse` and `/messages` had no auth
//! layer at all, so anyone who could open a TCP connection to the port held every permission
//! the configured workspace bot has. The first fix put a shared secret in front of the port.
//! That closed the hole but left a second one open by design: everyone who knew the secret
//! still acted as the *same* bot, so the API could not tell two callers apart and the audit
//! trail recorded one identity for all of them.
//!
//! What is tested here is the model that replaced both: on a networked transport there is no
//! server-side identity and no shared secret. Every request presents its own caller's
//! workspace bot token in `Authorization: Bearer`, the server forwards it to the API
//! unchanged, and a request without one is refused. `caller_identity_http_e2e.rs` proves the
//! forwarding; this file proves the refusal.
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
const CALLER_TOKEN: &str = "opr_caller_http_auth_e2e_token";

/// Reserves a free localhost port by binding and releasing it.
async fn free_port() -> Result<u16, Box<dyn Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

/// A live `mcp-server serve` child process on a networked transport.
///
/// The transport and the bind address come out of the configuration file, so the listener
/// under test is the one a deployment would get.
struct HttpServer {
    child: Child,
    base_url: String,
    /// Kept alive for as long as the child runs: dropping it deletes the file.
    _config: ConfigFile,
}

impl HttpServer {
    fn spawn(bind_addr: &str) -> Result<Self, Box<dyn Error>> {
        Self::spawn_with(bind_addr, bind_addr, None, "http", &[])
    }

    /// `configured_bind` is what the file says, `reachable_at` is where the listener is
    /// expected to end up, and `extra_args` are appended after `--config` — so a test can
    /// hand the two different values and prove a flag overrides the file.
    fn spawn_with(
        configured_bind: &str,
        reachable_at: &str,
        bot_token: Option<&str>,
        transport: &str,
        extra_args: &[&str],
    ) -> Result<Self, Box<dyn Error>> {
        let config = write_config(&McpSettings {
            // No API call is made by `initialize`, so the URL only has to be well formed.
            api_url: "http://127.0.0.1:1",
            bot_token,
            workspace_id: WORKSPACE,
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

/// Every data carrying route needs a caller bot token, and `/health` needs none.
///
/// The token is not checked here for anything but shape — that is the API's job — so the
/// assertion is about which requests are *served*, not about which token is correct.
#[tokio::test]
async fn inbound_requests_need_a_caller_bot_token() -> TestResult {
    let bind_addr = format!("127.0.0.1:{}", free_port().await?);
    let server = HttpServer::spawn(&bind_addr)?;
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
    assert!(
        body.contains("Authorization: Bearer"),
        "the refusal must say what to present: {body}"
    );
    assert!(
        !body.contains(BOT_TOKEN) && !body.contains(CALLER_TOKEN),
        "the rejection echoed a token: {body}"
    );

    // Shapes that are not a bearer token are not a caller identity either.
    for header_value in ["", "Bearer", "Bearer   ", &format!("Basic {CALLER_TOKEN}")] {
        let response = client
            .post(&rpc_url)
            .header(reqwest::header::AUTHORIZATION, header_value)
            .json(&initialize_request())
            .send()
            .await?;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "Authorization: {header_value:?} was accepted as an identity"
        );
    }

    // Any well formed bearer token gets in — the API, not this server, decides whether the
    // bot it names is real. `initialize` makes no API call, so it answers from here.
    let authorized = client
        .post(&rpc_url)
        .bearer_auth(CALLER_TOKEN)
        .json(&initialize_request())
        .send()
        .await?;
    assert_eq!(
        authorized.status(),
        reqwest::StatusCode::OK,
        "a caller that presented a bot token was refused"
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

    // `/health` is deliberately exempt: probes have no bot account to authenticate as.
    let health = client.get(format!("{}/health", server.base_url)).send().await?;
    assert_eq!(
        health.status(),
        reqwest::StatusCode::OK,
        "/health should not require auth"
    );
    assert_eq!(health.text().await?, "OK");

    server.shutdown().await
}

/// The invariant that replaced the old "non-loopback bind without a shared secret refuses to
/// start" gate. That gate existed because the process held a workspace bot and an anonymous
/// caller would have inherited it. Neither half is true any more: there is no anonymous
/// caller, and on a networked transport there is no bot to inherit. So a wide open bind now
/// *serves* — and is still not an open door, which is what this test pins down.
#[tokio::test]
async fn a_wide_open_bind_serves_but_still_refuses_anonymous_callers() -> TestResult {
    let port = free_port().await?;
    let server = HttpServer::spawn_with(
        &format!("0.0.0.0:{port}"),
        &format!("127.0.0.1:{port}"),
        None,
        "http",
        &[],
    )?;
    let client = reqwest::Client::new();
    server.wait_until_ready(&client).await?;

    let rpc_url = format!("{}/mcp/rpc", server.base_url);
    let anonymous = client.post(&rpc_url).json(&initialize_request()).send().await?;
    assert_eq!(
        anonymous.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "0.0.0.0 served an anonymous caller"
    );

    let identified = client
        .post(&rpc_url)
        .bearer_auth(CALLER_TOKEN)
        .json(&initialize_request())
        .send()
        .await?;
    assert_eq!(
        identified.status(),
        reqwest::StatusCode::OK,
        "0.0.0.0 refused an identified caller"
    );

    server.shutdown().await
}

/// A networked deployment holds no identity, so it must not be made to configure one. The
/// file this starts from carries a transport, a bind address and a workspace — nothing else.
#[tokio::test]
async fn a_networked_deployment_starts_without_a_configured_bot_token() -> TestResult {
    let bind_addr = format!("127.0.0.1:{}", free_port().await?);
    let server = HttpServer::spawn(&bind_addr)?;
    let client = reqwest::Client::new();
    server.wait_until_ready(&client).await?;

    let response = client
        .post(format!("{}/mcp/rpc", server.base_url))
        .bearer_auth(CALLER_TOKEN)
        .json(&initialize_request())
        .send()
        .await?;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "a file with no bot_token did not serve an identified caller"
    );

    server.shutdown().await
}

/// The SSE transport shares the routes and therefore the rule.
#[tokio::test]
async fn the_sse_transport_refuses_anonymous_callers_too() -> TestResult {
    let bind_addr = format!("127.0.0.1:{}", free_port().await?);
    let server = HttpServer::spawn_with(&bind_addr, &bind_addr, None, "sse", &[])?;
    let client = reqwest::Client::new();
    server.wait_until_ready(&client).await?;

    for path in ["/sse", "/messages"] {
        let response = client.get(format!("{}{path}", server.base_url)).send().await?;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "sse {path} was reachable without credentials"
        );
    }

    server.shutdown().await
}

/// `--bind-addr` beats `[mcp] bind_addr`, and the listener really moves to the flag's
/// address rather than the file's.
#[tokio::test]
async fn a_bind_address_flag_overrides_the_file() -> TestResult {
    let bind_addr = format!("127.0.0.1:{}", free_port().await?);
    let serving = HttpServer::spawn_with("127.0.0.1:8090", &bind_addr, None, "http", &["--bind-addr", &bind_addr])?;
    let client = reqwest::Client::new();
    serving.wait_until_ready(&client).await?;

    let authorized = client
        .post(format!("{}/mcp/rpc", serving.base_url))
        .bearer_auth(CALLER_TOKEN)
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

/// stdio is a pipe pair owned by the parent process, so the inbound rule must not leak into
/// it: an MCP client launching the binary has no port and no header to present. Its identity
/// is `mcp.bot_token`, and a stdio process with no bot token has no identity at all.
#[tokio::test]
async fn stdio_serves_from_its_configured_bot_token_with_no_inbound_credential() -> TestResult {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    // Bound wide open in the file, which stdio ignores entirely.
    let config = write_config(&McpSettings {
        api_url: "http://127.0.0.1:1",
        bot_token: Some(BOT_TOKEN),
        workspace_id: WORKSPACE,
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

/// stdio has nowhere else to get an identity from, so a file without one must stop the
/// process rather than let it run and fail every call at the API.
#[tokio::test]
async fn stdio_without_a_configured_bot_token_refuses_to_start() -> TestResult {
    let config = write_config(&McpSettings {
        api_url: "http://127.0.0.1:1",
        bot_token: None,
        workspace_id: WORKSPACE,
        transport: Some("stdio"),
        bind_addr: None,
    })?;

    let child = Command::new(env!("CARGO_BIN_EXE_mcp-server"))
        .args(["serve", "--config"])
        .arg(config.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output()).await??;
    assert!(
        !output.status.success(),
        "stdio with no identity started anyway (exit status {:?})",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("mcp.bot_token"),
        "the refusal must name the key that fixes it: {stderr}"
    );
    Ok(())
}

/// The shared inbound secret is gone. A file that still carries `mcp.auth_token` must be
/// stopped with an explanation: serving it silently would leave the operator believing the
/// port is gated by a secret that nothing reads.
#[tokio::test]
async fn a_configuration_still_carrying_the_retired_shared_secret_refuses_to_start() -> TestResult {
    let config = support::write_config_with_extra_mcp_keys(
        &McpSettings {
            api_url: "http://127.0.0.1:1",
            bot_token: Some(BOT_TOKEN),
            workspace_id: WORKSPACE,
            transport: Some("http"),
            bind_addr: Some("127.0.0.1:8090"),
        },
        "auth_token = \"a-retired-inbound-shared-secret\"\n",
    )?;

    let child = Command::new(env!("CARGO_BIN_EXE_mcp-server"))
        .args(["serve", "--config"])
        .arg(config.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output()).await??;
    assert!(
        !output.status.success(),
        "a file carrying the retired shared secret started anyway"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("mcp.auth_token has been removed"),
        "the refusal must explain what happened to the key: {stderr}"
    );
    assert!(
        !stderr.contains("a-retired-inbound-shared-secret"),
        "the refusal echoed the retired secret: {stderr}"
    );
    Ok(())
}

/// The environment is not a configuration source. Handed every variable the server used to
/// read and no file, it must refuse to start instead of quietly running on values that are
/// no longer part of the deployment's configuration.
#[tokio::test]
async fn the_retired_environment_variables_no_longer_configure_the_server() -> TestResult {
    let empty = support::write_config(&McpSettings {
        api_url: "http://127.0.0.1:1",
        bot_token: Some(BOT_TOKEN),
        workspace_id: WORKSPACE,
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
        !stderr.contains(BOT_TOKEN),
        "the failure echoed a token from the environment: {stderr}"
    );
    Ok(())
}
