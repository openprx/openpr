//! End-to-end proof that the binary's CLI mode works, against the shipped binary and a
//! counting stand-in for the `OpenPR` API.
//!
//! The same executable is a server and a command line client, and the CLI half has no MCP
//! client in front of it to make its mistakes obvious: a human types a command and reads what
//! comes back on stdout and stderr. What that half owes its user is therefore not only correct
//! results but correct *refusals* — a missing argument, an unusable credential and an absent
//! configuration file each have to say what went wrong and what to do about it, without
//! leaking anything about the backend.
//!
//! The unit tests in `cli.rs` already check that the argument grammar parses. They cannot check
//! any of the above, because they never run the binary: nothing there reaches a config loader,
//! an API call or a process exit status. So every test here spawns the real executable with a
//! real configuration file, in a working directory that deliberately holds no
//! `config/openpr.toml` — a command that failed to pick up its `--config` would fail with
//! "configuration file not found" rather than silently falling back onto some file that
//! happened to be there.
//!
//! Assertions are made against the counting stand-in wherever the question is *what the CLI
//! did*, not merely what it printed: whether a refusal happened before any call went out,
//! which credential a call carried, and how many calls one command is worth.

mod support;

use axum::{Json, Router, extract::Path, http::HeaderMap, routing::get};
use serde_json::{Value, json};
use std::error::Error;
use std::path::Path as FsPath;
use std::process::{Output, Stdio};
use std::sync::Arc;
use std::time::Duration;
use support::{ConfigFile, McpSettings, write_config};
use tokio::process::Command;

type TestResult = Result<(), Box<dyn Error>>;

const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";

/// The identity the CLI is configured with. A CLI subcommand is a local process with no caller
/// to act on behalf of, so this is the only identity it can have.
const CONFIGURED_TOKEN: &str = "opr_configured_cli_bot_token";
/// A token shaped like a real one that the stand-in API does not accept, standing in for a
/// bot that was revoked, disabled or mistyped.
const REJECTED_TOKEN: &str = "opr_revoked_cli_bot_token";

/// Something the stand-in API knows and no CLI user should ever be shown.
const API_INTERNAL_DETAIL: &str = "pg-primary-7.internal:5432 auth_backend=ldap";

/// Every bearer token the stand-in API was called with, in order, with the path it arrived on.
type SeenCredentials = Arc<tokio::sync::Mutex<Vec<(String, String)>>>;

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

    /// How many outbound calls every command so far is worth in total.
    async fn call_count(&self) -> usize {
        self.seen.lock().await.len()
    }

    /// Every call made so far, for a failure message that has to explain an unexpected count.
    async fn calls(&self) -> Vec<(String, String)> {
        self.seen.lock().await.clone()
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

/// The API's answer for one credential.
///
/// A rejected token is answered the way the real API answers one: HTTP 200 carrying a
/// `{code: 401}` envelope (`apps/api/src/error.rs`), whose message is operator prose that
/// names infrastructure the caller cannot reach.
fn envelope_for(token: &str, data: &Value) -> Value {
    if token == REJECTED_TOKEN {
        json!({
            "code": 401,
            "message": format!("bot token not found (looked it up in {API_INTERNAL_DETAIL})"),
            "data": null
        })
    } else {
        json!({ "code": 0, "message": "ok", "data": data.clone() })
    }
}

/// Starts the stand-in API on a random loopback port and returns its base URL plus the probe.
///
/// Only the two workspace-wide read endpoints the tested subcommands use are served. They are
/// workspace scoped, so no project agent policy governs them and one subcommand is worth
/// exactly one outbound call — which is itself asserted below.
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
                    Json(envelope_for(
                        &token,
                        &json!([
                            { "id": "project-1", "name": "apollo", "key": "APL" },
                            { "id": "project-2", "name": "borealis", "key": "BOR" }
                        ]),
                    ))
                }
            }),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/labels",
            get(move |Path(workspace_id): Path<String>, headers: HeaderMap| {
                let seen = Arc::clone(&labels_seen);
                async move {
                    let token = presented_token(&headers);
                    seen.lock()
                        .await
                        .push((format!("/workspaces/{workspace_id}/labels"), token.clone()));
                    Json(envelope_for(&token, &json!([{ "id": "label-1", "name": "bug" }])))
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

/// A configuration file carrying `bot_token`, plus the empty directory commands run from.
///
/// The directory is the file's own private one, which holds nothing but `openpr.toml` — no
/// `config/openpr.toml`, so the default configuration path cannot resolve there.
fn cli_config(api_url: &str, bot_token: &str) -> Result<ConfigFile, Box<dyn Error>> {
    write_config(&McpSettings {
        api_url,
        bot_token: Some(bot_token),
        workspace_id: WORKSPACE,
        // The CLI subcommands never start a listener; the transport only labels the audit.
        transport: Some("stdio"),
        bind_addr: None,
    })
}

/// Runs the shipped binary once and collects its exit status, stdout and stderr.
///
/// `cwd` is always a directory with no `config/openpr.toml` in it, so nothing succeeds by
/// accident of where the test happened to run.
async fn run_cli(cwd: &FsPath, args: &[&str], envs: &[(&str, &str)]) -> Result<Output, Box<dyn Error>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mcp-server"));
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (key, value) in envs {
        command.env(key, value);
    }

    Ok(tokio::time::timeout(Duration::from_secs(30), command.spawn()?.wait_with_output()).await??)
}

/// The directory a configuration file lives in, which is also the empty working directory
/// commands are run from.
fn dir_of(config: &ConfigFile) -> Result<&FsPath, Box<dyn Error>> {
    config
        .path()
        .parent()
        .ok_or_else(|| "the configuration file has no parent directory".into())
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

/// The read-only subcommands work, answer with the API envelope, and cost exactly one call.
///
/// The call count is the assertion a "did it print something" check cannot make: these tools
/// are workspace scoped and therefore outside the project policy gate, so a second outbound
/// call would mean the CLI had started evaluating a policy that does not apply to them.
#[tokio::test]
async fn read_only_subcommands_answer_with_the_api_envelope_as_the_configured_bot() -> TestResult {
    let (api_url, probe) = spawn_api().await?;
    let config = cli_config(&api_url, CONFIGURED_TOKEN)?;
    let cwd = dir_of(&config)?;
    let config_path = config.path().to_string_lossy().to_string();

    let projects = run_cli(cwd, &["projects", "list", "--config", &config_path], &[]).await?;
    assert!(
        projects.status.success(),
        "projects list failed: {}",
        stderr_of(&projects)
    );
    let payload: Value = serde_json::from_str(stdout_of(&projects).trim())?;
    assert_eq!(
        payload.get("code").and_then(Value::as_i64),
        Some(0),
        "projects list did not print the API envelope: {payload}"
    );
    assert_eq!(
        payload
            .get("data")
            .and_then(Value::as_array)
            .and_then(|rows| rows.first())
            .and_then(|row| row.get("name"))
            .and_then(Value::as_str),
        Some("apollo"),
        "projects list lost the payload it exists to print: {payload}"
    );

    let labels = run_cli(cwd, &["labels", "list", "--config", &config_path], &[]).await?;
    assert!(labels.status.success(), "labels list failed: {}", stderr_of(&labels));
    let payload: Value = serde_json::from_str(stdout_of(&labels).trim())?;
    assert_eq!(payload.get("code").and_then(Value::as_i64), Some(0));

    // Each subcommand reached its own endpoint, as the configured identity, once.
    assert_eq!(probe.tokens_for("/projects").await, vec![CONFIGURED_TOKEN.to_string()]);
    assert_eq!(probe.tokens_for("/labels").await, vec![CONFIGURED_TOKEN.to_string()]);
    assert_eq!(
        probe.call_count().await,
        2,
        "two subcommands cost more than two calls: {:?}",
        probe.calls().await
    );

    Ok(())
}

/// `tools call` reaches a tool by name, and refuses a payload that is not a JSON object
/// before anything goes out on the wire.
///
/// It is the escape hatch for every tool without a dedicated subcommand, which makes it the
/// one entry point whose breakage would not show up in any of the others.
#[tokio::test]
async fn the_generic_tool_entry_point_calls_a_tool_by_name_and_validates_its_payload() -> TestResult {
    let (api_url, probe) = spawn_api().await?;
    let config = cli_config(&api_url, CONFIGURED_TOKEN)?;
    let cwd = dir_of(&config)?;
    let config_path = config.path().to_string_lossy().to_string();

    let called = run_cli(
        cwd,
        &[
            "tools",
            "call",
            "--name",
            "projects.list",
            "--args-json",
            "{}",
            "--config",
            &config_path,
        ],
        &[],
    )
    .await?;
    assert!(called.status.success(), "tools call failed: {}", stderr_of(&called));
    let payload: Value = serde_json::from_str(stdout_of(&called).trim())?;
    assert_eq!(payload.get("code").and_then(Value::as_i64), Some(0));
    assert_eq!(
        probe.tokens_for("/projects").await,
        vec![CONFIGURED_TOKEN.to_string()],
        "the generic entry point did not call the API as the configured bot"
    );

    // A payload that is not a JSON object is refused locally, so it costs no call.
    let malformed = run_cli(
        cwd,
        &[
            "tools",
            "call",
            "--name",
            "projects.list",
            "--args-json",
            "[]",
            "--config",
            &config_path,
        ],
        &[],
    )
    .await?;
    assert!(
        !malformed.status.success(),
        "a non-object --args-json was accepted: {}",
        stdout_of(&malformed)
    );
    let stderr = stderr_of(&malformed);
    assert!(
        stderr.contains("--args-json must be a JSON object"),
        "the refusal does not say what --args-json has to be: {stderr}"
    );
    assert_eq!(
        probe.call_count().await,
        1,
        "a rejected payload still cost an API call: {:?}",
        probe.calls().await
    );

    Ok(())
}

/// A missing required argument is a parameter error, reported before any call goes out.
///
/// `work-items list` needs a project because the tool behind it is project scoped. The failure
/// that matters is not the wording but the timing: sending the call with an empty or absent
/// project id would turn a typo into a backend round trip and, worse, into whatever that
/// backend does with a blank id.
#[tokio::test]
async fn a_missing_required_argument_is_a_parameter_error_that_costs_no_call() -> TestResult {
    let (api_url, probe) = spawn_api().await?;
    let config = cli_config(&api_url, CONFIGURED_TOKEN)?;
    let cwd = dir_of(&config)?;
    let config_path = config.path().to_string_lossy().to_string();

    let output = run_cli(cwd, &["work-items", "list", "--config", &config_path], &[]).await?;
    assert!(
        !output.status.success(),
        "work-items list ran without a project: {}",
        stdout_of(&output)
    );
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("--project"),
        "the error does not name the missing argument: {stderr}"
    );
    assert!(
        stderr.contains("Usage") || stderr.contains("usage"),
        "the error does not show how to invoke the command: {stderr}"
    );
    assert_eq!(
        probe.call_count().await,
        0,
        "an unparseable command still called the API: {:?}",
        probe.calls().await
    );

    Ok(())
}

/// `--format table` is a different rendering of the same result, not a different result.
///
/// The API contract is an envelope object, so the table renderer lays it out as aligned
/// `key value` rows rather than as a header-and-rows grid. The assertions are therefore that
/// the two formats disagree in shape — one is machine readable JSON, the other is not — while
/// carrying the same payload.
#[tokio::test]
async fn the_table_format_renders_the_same_result_in_a_non_json_layout() -> TestResult {
    let (api_url, probe) = spawn_api().await?;
    let config = cli_config(&api_url, CONFIGURED_TOKEN)?;
    let cwd = dir_of(&config)?;
    let config_path = config.path().to_string_lossy().to_string();

    let json_run = run_cli(
        cwd,
        &["projects", "list", "--config", &config_path, "--format", "json"],
        &[],
    )
    .await?;
    assert!(
        json_run.status.success(),
        "json format failed: {}",
        stderr_of(&json_run)
    );
    let json_out = stdout_of(&json_run);
    assert!(
        serde_json::from_str::<Value>(json_out.trim()).is_ok(),
        "the json format is not machine readable: {json_out}"
    );

    let table_run = run_cli(
        cwd,
        &["projects", "list", "--config", &config_path, "--format", "table"],
        &[],
    )
    .await?;
    assert!(
        table_run.status.success(),
        "table format failed: {}",
        stderr_of(&table_run)
    );
    let table_out = stdout_of(&table_run);

    assert!(
        serde_json::from_str::<Value>(table_out.trim()).is_err(),
        "--format table printed JSON, so the flag did nothing: {table_out}"
    );
    // Every envelope key gets its own row, in a column wide enough for the longest of them.
    for key in ["code", "message", "data"] {
        assert!(
            table_out.lines().any(|line| line.starts_with(key)),
            "the table has no row for {key}: {table_out}"
        );
    }
    assert!(
        table_out.contains("apollo") && table_out.contains("borealis"),
        "the table dropped the payload the json format carried: {table_out}"
    );
    assert!(
        table_out
            .lines()
            .any(|line| line.starts_with("code") && line.contains("  ")),
        "the table rows are not column aligned: {table_out}"
    );

    assert_eq!(
        probe.call_count().await,
        2,
        "two renderings cost more than two calls: {:?}",
        probe.calls().await
    );

    Ok(())
}

/// A credential the API rejects produces an actionable message and reveals nothing else.
///
/// The CLI never decides this: it forwards the configured token and relays the verdict. What
/// it must relay is what the user can act on — that the token is the problem and which
/// properties of it to check — and what it must not relay is the API's own prose, which is
/// written for an operator reading logs and can name infrastructure the user cannot reach.
#[tokio::test]
async fn a_rejected_bot_token_is_reported_readably_without_leaking_the_backend() -> TestResult {
    let (api_url, probe) = spawn_api().await?;
    let config = cli_config(&api_url, REJECTED_TOKEN)?;
    let cwd = dir_of(&config)?;
    let config_path = config.path().to_string_lossy().to_string();

    let output = run_cli(cwd, &["projects", "list", "--config", &config_path], &[]).await?;
    assert!(
        !output.status.success(),
        "a rejected credential exited successfully: {}",
        stdout_of(&output)
    );

    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("401"),
        "the error does not report the verdict: {stderr}"
    );
    assert!(
        stderr.contains("correct, enabled and not expired"),
        "the error is not actionable — it does not say what to check about the token: {stderr}"
    );
    assert!(
        !stderr.contains(API_INTERNAL_DETAIL),
        "the error leaked the API's internals: {stderr}"
    );
    assert!(
        !stderr.contains(REJECTED_TOKEN),
        "the error echoed the token itself: {stderr}"
    );

    // The verdict really came from the API, reached with the configured token — the CLI did
    // not invent a refusal locally, which would have made the message a guess.
    assert_eq!(
        probe.tokens_for("/projects").await,
        vec![REJECTED_TOKEN.to_string()],
        "the CLI did not forward the configured token to the API"
    );

    Ok(())
}

/// With no configuration file, the CLI says so, says the environment is not consulted, and
/// gives the steps that fix it.
///
/// The environment is handed every variable the server used to read, precisely so a fallback
/// would be visible: a run that silently succeeded here, or that failed for some other reason,
/// would mean the environment is still a configuration source. The message has to close the
/// obvious next guess ("then I'll export the variables instead") in the same breath.
#[tokio::test]
async fn a_missing_configuration_file_is_explained_and_the_environment_is_not_a_fallback() -> TestResult {
    let (api_url, probe) = spawn_api().await?;
    // A real file, used only for the empty directory it lives in — the command below is run
    // there without `--config`, so the default `config/openpr.toml` cannot resolve.
    let config = cli_config(&api_url, CONFIGURED_TOKEN)?;
    let cwd = dir_of(&config)?;

    let output = run_cli(
        cwd,
        &["projects", "list"],
        &[
            ("OPENPR_API_URL", &api_url),
            ("OPENPR_BOT_TOKEN", CONFIGURED_TOKEN),
            ("OPENPR_WORKSPACE_ID", WORKSPACE),
            ("OPENPR_MCP_TRANSPORT", "stdio"),
            ("RUST_LOG", "error"),
        ],
    )
    .await?;

    assert!(
        !output.status.success(),
        "the environment alone ran a CLI subcommand: {}",
        stdout_of(&output)
    );

    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("configuration file not found"),
        "the failure is not the missing file: {stderr}"
    );
    assert!(
        stderr.contains("config/openpr.toml"),
        "the error does not name the file it looked for: {stderr}"
    );
    assert!(
        stderr.contains("reads no environment variables"),
        "the error does not close off the environment as an alternative: {stderr}"
    );
    assert!(
        stderr.contains("config/openpr.example.toml"),
        "the error does not point at the example to copy: {stderr}"
    );
    assert!(
        stderr.contains("--config"),
        "the error does not mention the flag that names another path: {stderr}"
    );
    assert!(
        !stderr.contains(CONFIGURED_TOKEN),
        "the error echoed a token out of the environment: {stderr}"
    );

    assert_eq!(
        probe.call_count().await,
        0,
        "a run with no configuration still called the API: {:?}",
        probe.calls().await
    );

    Ok(())
}

/// `--config` is declared global, so it has to be accepted wherever it is written.
///
/// Declaring it on `serve` alone once made `mcp-server projects list --config <path>` fail to
/// parse, and that is the spelling every operator reaches for first. The `cli.rs` unit test
/// covers the grammar; what is added here is that the path is actually *loaded* from each
/// position — the commands run in a directory with no configuration file, so one that ignored
/// the flag would die on the missing file instead of reaching the API.
#[tokio::test]
async fn the_global_config_flag_is_honoured_before_and_after_the_subcommand() -> TestResult {
    let (api_url, probe) = spawn_api().await?;
    let config = cli_config(&api_url, CONFIGURED_TOKEN)?;
    let cwd = dir_of(&config)?;
    let config_path = config.path().to_string_lossy().to_string();

    let spellings: Vec<Vec<&str>> = vec![
        // After the subcommand, which is what a person types.
        vec!["projects", "list", "--config", &config_path],
        // Before it, which is what a script generates.
        vec!["--config", &config_path, "projects", "list"],
        // After a subcommand that takes flags of its own.
        vec!["tools", "call", "--name", "projects.list", "--config", &config_path],
        // Between a subcommand's own flags.
        vec![
            "tools",
            "call",
            "--name",
            "projects.list",
            "--config",
            &config_path,
            "--args-json",
            "{}",
        ],
    ];

    let expected = spellings.len();
    for args in spellings {
        let output = run_cli(cwd, &args, &[]).await?;
        assert!(
            output.status.success(),
            "{args:?} did not load its configuration: {}",
            stderr_of(&output)
        );
        let payload: Value = serde_json::from_str(stdout_of(&output).trim())?;
        assert_eq!(
            payload.get("code").and_then(Value::as_i64),
            Some(0),
            "{args:?} did not reach the API: {payload}"
        );
    }

    // Every spelling reached the backend as the file's identity, so each one really read the
    // file rather than failing over to something else.
    assert_eq!(
        probe.tokens_for("/projects").await,
        vec![CONFIGURED_TOKEN.to_string(); expected],
        "not every spelling of --config loaded the same configured identity"
    );

    Ok(())
}
