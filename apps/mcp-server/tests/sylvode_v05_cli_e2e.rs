//! Shipped-binary coverage for the twelve v0.5 CLI rows in `cli-surface-v1.md`.

mod support;

use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
};
use serde_json::{Value, json};
use std::{error::Error, path::Path, process::Output, sync::Arc, time::Duration};
use support::{ConfigFile, McpSettings, write_config};
use tokio::process::Command;

type TestResult = Result<(), Box<dyn Error>>;
type Calls = Arc<tokio::sync::Mutex<Vec<CapturedCall>>>;

const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const OBJECT: &str = "22222222-2222-4222-8222-222222222222";
const OTHER: &str = "33333333-3333-4333-8333-333333333333";
const THIRD: &str = "44444444-4444-4444-8444-444444444444";
const TOKEN: &str = "opr_sylvode_v05_cli_e2e";

#[derive(Debug, Clone)]
struct CapturedCall {
    method: String,
    uri: String,
    body: Value,
}

async fn capture(State(calls): State<Calls>, request: Request) -> Json<Value> {
    let method = request.method().to_string();
    let uri = request.uri().to_string();
    let body = to_bytes(request.into_body(), 1_048_576)
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null);
    let dry_run = body.get("dry_run").and_then(Value::as_bool) == Some(true);
    calls.lock().await.push(CapturedCall { method, uri, body });
    Json(if dry_run {
        json!({ "code": 0, "data": { "applied": false, "permission_changes": { "affected": [] } } })
    } else {
        json!({ "code": 0, "data": { "accepted": true } })
    })
}

async fn api() -> Result<(String, Calls), Box<dyn Error>> {
    let calls: Calls = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let router = Router::new().fallback(capture).with_state(Arc::clone(&calls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok((format!("http://{address}"), calls))
}

fn config(api_url: &str) -> Result<ConfigFile, Box<dyn Error>> {
    write_config(&McpSettings {
        api_url,
        bot_token: Some(TOKEN),
        workspace_id: WORKSPACE,
        transport: Some("stdio"),
        bind_addr: None,
    })
}

async fn run(config: &ConfigFile, args: &[&str]) -> Result<Output, Box<dyn Error>> {
    let cwd = config.path().parent().ok_or("config path has no parent")?;
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        Command::new(env!("CARGO_BIN_EXE_sylvode"))
            .arg("--config")
            .arg(config.path())
            .args(args)
            .current_dir(cwd)
            .output(),
    )
    .await??;
    Ok(output)
}

fn assert_success(output: &Output, command: &str) -> TestResult {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{command} failed: {stderr}");
    let envelope: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(envelope["schema_version"], "sylvode.cli.v1");
    assert_eq!(envelope["ok"], true);
    assert_eq!(envelope["command"], command);
    Ok(())
}

fn write_patch_file(dir: &Path, contents: &str) -> Result<String, Box<dyn Error>> {
    let path = dir.join("patch.json");
    std::fs::write(&path, contents)?;
    Ok(path.to_string_lossy().to_string())
}

#[tokio::test]
async fn all_twelve_v05_lines_map_to_the_frozen_rest_shape() -> TestResult {
    let (api_url, calls) = api().await?;
    let config = config(&api_url)?;
    let cwd = config.path().parent().ok_or("config path has no parent")?;
    let patch = write_patch_file(cwd, r#"[{"type":"set_title","title":"new"}]"#)?;

    let invocations: Vec<(&str, Vec<&str>)> = vec![
        (
            "objects.create",
            vec![
                "objects",
                "create",
                "--workspace",
                WORKSPACE,
                "--type",
                "page",
                "--title",
                "Title",
                "--parent",
                OTHER,
                "--idempotency-key",
                "create-key",
            ],
        ),
        (
            "objects.patch",
            vec![
                "objects",
                "patch",
                OBJECT,
                "--patch-file",
                &patch,
                "--expected-frontier",
                "source-frontier",
                "--idempotency-key",
                "patch-key",
            ],
        ),
        (
            "objects.move",
            vec![
                "objects",
                "move",
                OBJECT,
                "--parent",
                OTHER,
                "--after",
                THIRD,
                "--expected-target-frontier",
                "target-frontier",
                "--idempotency-key",
                "move-key",
            ],
        ),
        ("objects.grants.get", vec!["objects", "grants", "get", OBJECT]),
        (
            "objects.grants.set",
            vec![
                "objects",
                "grants",
                "set",
                OBJECT,
                "--grant",
                "user:33333333-3333-4333-8333-333333333333=view",
                "--dry-run",
                "--idempotency-key",
                "grants-key",
            ],
        ),
        (
            "objects.inheritance.set",
            vec![
                "objects",
                "inheritance",
                "set",
                OBJECT,
                "--inherit",
                "false",
                "--dry-run",
                "--idempotency-key",
                "inheritance-key",
            ],
        ),
        (
            "objects.link",
            vec![
                "objects",
                "link",
                OBJECT,
                OTHER,
                "--kind",
                "related_to",
                "--idempotency-key",
                "link-key",
            ],
        ),
        (
            "objects.unlink",
            vec![
                "objects",
                "unlink",
                OBJECT,
                "--relation",
                THIRD,
                "--idempotency-key",
                "unlink-key",
            ],
        ),
        (
            "objects.diff",
            vec![
                "objects", "diff", OBJECT, "--from", "1", "--to", "2", "--render", "markdown",
            ],
        ),
        (
            "objects.relations",
            vec![
                "objects",
                "relations",
                OBJECT,
                "--direction",
                "both",
                "--kind",
                "related_to",
                "--limit",
                "25",
            ],
        ),
        (
            "objects.search",
            vec![
                "objects",
                "search",
                "--workspace",
                WORKSPACE,
                "--unprojected",
                "--query",
                "needle",
                "--freshness",
                "require-current",
            ],
        ),
        (
            "collab.projection-lag",
            vec![
                "collab",
                "projection-lag",
                "--workspace",
                WORKSPACE,
                "--project",
                OTHER,
                "--limit",
                "10",
            ],
        ),
    ];

    for (command, args) in invocations {
        assert_success(&run(&config, &args).await?, command)?;
    }

    let calls = calls.lock().await.clone();
    assert_eq!(calls.len(), 12);
    assert_eq!(calls[0].method, "POST");
    assert_eq!(calls[0].uri, format!("/api/v1/workspaces/{WORKSPACE}/flow/objects"));
    assert_eq!(calls[0].body["parent_object_id"], OTHER);
    assert_eq!(calls[1].body["command"]["type"], "semantic_patch");
    assert_eq!(calls[1].body["expected_frontier"], "source-frontier");
    assert_eq!(calls[2].body["command"]["type"], "move_object");
    assert_eq!(calls[2].body["command"]["payload"]["target_object_id"], OTHER);
    assert_eq!(
        calls[2].body["command"]["payload"]["expected_target_frontier"],
        "target-frontier"
    );
    assert!(calls[2].body.get("expected_frontier").is_none());
    assert_eq!(calls[3].method, "GET");
    assert_eq!(calls[4].body["dry_run"], true);
    assert_eq!(calls[5].body["inherit_from_parent"], false);
    assert_eq!(calls[6].body["command"]["payload"]["relation_type"], "related_to");
    assert_eq!(calls[7].body["command"]["payload"]["relation_id"], THIRD);
    assert!(calls[8].uri.contains("from_seq=1&to_seq=2&render=markdown"));
    assert!(
        calls[9]
            .uri
            .contains("direction=both&relation_type=related_to&limit=25")
    );
    assert!(calls[10].uri.contains("unprojected=true"));
    assert!(!calls[10].uri.contains("all_visible"));
    assert!(calls[11].uri.contains("project_id="));
    Ok(())
}

#[tokio::test]
async fn malformed_patch_file_exits_two_before_the_network() -> TestResult {
    let (api_url, calls) = api().await?;
    let config = config(&api_url)?;
    let cwd = config.path().parent().ok_or("config path has no parent")?;
    let patch = write_patch_file(cwd, "not json")?;
    let output = run(
        &config,
        &[
            "objects",
            "patch",
            OBJECT,
            "--patch-file",
            &patch,
            "--idempotency-key",
            "patch-key",
        ],
    )
    .await?;
    assert_eq!(output.status.code(), Some(2));
    let envelope: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["code"], "usage_error");
    assert!(calls.lock().await.is_empty(), "invalid local JSON reached the API");
    Ok(())
}
