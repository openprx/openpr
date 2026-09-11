//! Flow v0.6 Collection coverage through the shipped `sylvode` binary and the live MCP handler.

mod support;

use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
};
use mcp_server::{
    client::{ClientConfig, OpenPrClient, TRANSPORT_LABEL_STDIO},
    protocol::ToolContent,
    tools::objects::create_flow_object,
};
use platform::config::Secret;
use serde_json::{Value, json};
use std::{error::Error, path::Path, process::Output, sync::Arc, time::Duration};
use support::{ConfigFile, McpSettings, write_config};
use tokio::process::Command;

type TestResult = Result<(), Box<dyn Error>>;
type Calls = Arc<tokio::sync::Mutex<Vec<CapturedCall>>>;

const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const COLLECTION: &str = "22222222-2222-4222-8222-222222222222";
const PAGE: &str = "33333333-3333-4333-8333-333333333333";
const RECORD: &str = "44444444-4444-4444-8444-444444444444";
const FIELD: &str = "55555555-5555-4555-8555-555555555555";
const TOKEN: &str = "opr_flow_v06_collection_e2e";

#[derive(Debug, Clone, PartialEq)]
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
    calls.lock().await.push(CapturedCall {
        method: method.clone(),
        uri: uri.clone(),
        body,
    });

    let data = if method == "GET" && uri == format!("/api/v1/flow/objects/{RECORD}") {
        json!({"id": RECORD, "parent_id": COLLECTION, "object_type": "record"})
    } else {
        json!({"accepted": true})
    };
    Json(json!({"code": 0, "data": data}))
}

async fn api() -> TestResultValue<(String, Calls)> {
    let calls: Calls = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let router = Router::new().fallback(capture).with_state(Arc::clone(&calls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok((format!("http://{address}"), calls))
}

type TestResultValue<T> = Result<T, Box<dyn Error>>;

fn config(api_url: &str) -> TestResultValue<ConfigFile> {
    write_config(&McpSettings {
        api_url,
        bot_token: Some(TOKEN),
        workspace_id: WORKSPACE,
        transport: Some("stdio"),
        bind_addr: None,
    })
}

async fn run(config: &ConfigFile, args: &[&str]) -> TestResultValue<Output> {
    let cwd = config.path().parent().ok_or("config path has no parent")?;
    Ok(tokio::time::timeout(
        Duration::from_secs(30),
        Command::new(env!("CARGO_BIN_EXE_sylvode"))
            .arg("--config")
            .arg(config.path())
            .args(args)
            .current_dir(cwd)
            .output(),
    )
    .await??)
}

fn assert_success(output: &Output, command: &str) -> TestResult {
    assert!(
        output.status.success(),
        "{command} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(envelope.pointer("/schema_version"), Some(&json!("sylvode.cli.v1")));
    assert_eq!(envelope.pointer("/ok"), Some(&json!(true)));
    assert_eq!(envelope.pointer("/command"), Some(&json!(command)));
    Ok(())
}

fn write_json(dir: &Path, name: &str, value: &Value) -> TestResultValue<String> {
    let path = dir.join(name);
    std::fs::write(&path, serde_json::to_vec(value)?)?;
    Ok(path.to_string_lossy().to_string())
}

#[tokio::test]
async fn shipped_cli_covers_collection_create_describe_query_and_record_write_paths() -> TestResult {
    let (api_url, calls) = api().await?;
    let config = config(&api_url)?;
    let cwd = config.path().parent().ok_or("config path has no parent")?;
    let schema = write_json(
        cwd,
        "schema.json",
        &json!({
            "initial_fields": [{"field_id": FIELD, "label": "Score", "field_type": "number"}],
            "initial_view": {"view_id": "66666666-6666-4666-8666-666666666666", "label": "All"}
        }),
    )?;
    let query = write_json(
        cwd,
        "query.json",
        &json!({"filter": {"field_id": FIELD, "op": "gte", "value": 10}, "limit": 20}),
    )?;
    let values = write_json(cwd, "values.json", &json!({(FIELD): 42}))?;

    let invocations: Vec<(&str, Vec<&str>)> = vec![
        (
            "objects.create",
            vec![
                "objects",
                "create",
                "--workspace",
                WORKSPACE,
                "--type",
                "collection",
                "--title",
                "Scores",
                "--schema-file",
                &schema,
                "--idempotency-key",
                "standalone-create",
            ],
        ),
        (
            "objects.create",
            vec![
                "objects",
                "create",
                "--workspace",
                WORKSPACE,
                "--type",
                "collection",
                "--title",
                "Embedded",
                "--embed-page",
                PAGE,
                "--schema-file",
                &schema,
                "--idempotency-key",
                "embedded-create",
            ],
        ),
        ("collections.describe", vec!["collections", "describe", COLLECTION]),
        (
            "collections.query",
            vec![
                "collections",
                "query",
                COLLECTION,
                "--query-file",
                &query,
                "--cursor",
                "opaque-cursor",
            ],
        ),
        (
            "records.create",
            vec![
                "records",
                "create",
                "--collection",
                COLLECTION,
                "--values-file",
                &values,
                "--body",
                "row",
                "--idempotency-key",
                "record-create",
            ],
        ),
        (
            "records.patch",
            vec![
                "records",
                "patch",
                RECORD,
                "--values-file",
                &values,
                "--body",
                "updated",
                "--idempotency-key",
                "record-patch",
            ],
        ),
    ];
    for (command, args) in invocations {
        assert_success(&run(&config, &args).await?, command)?;
    }

    let calls = calls.lock().await.clone();
    assert_eq!(
        calls.len(),
        7,
        "record patch performs one ownership read and one command write"
    );
    let [
        standalone,
        embedded,
        describe,
        query_call,
        create_record,
        get_record,
        patch_record,
    ] = calls.as_slice()
    else {
        return Err("expected the seven captured Collection calls".into());
    };
    assert_eq!(standalone.uri, format!("/api/v1/workspaces/{WORKSPACE}/flow/objects"));
    assert_eq!(
        standalone.body.pointer("/initial_fields/0/field_id"),
        Some(&json!(FIELD))
    );
    assert_eq!(embedded.uri, format!("/api/v1/flow/objects/{PAGE}/commands"));
    assert_eq!(
        embedded.body.pointer("/command/type"),
        Some(&json!("create_collection_embed"))
    );
    assert_eq!(describe.uri, format!("/api/v1/flow/collections/{COLLECTION}"));
    assert_eq!(query_call.uri, format!("/api/v1/flow/collections/{COLLECTION}/query"));
    assert_eq!(query_call.body.pointer("/cursor"), Some(&json!("opaque-cursor")));
    assert_eq!(
        create_record.uri,
        format!("/api/v1/flow/collections/{COLLECTION}/records")
    );
    assert_eq!(
        create_record.body.pointer(&format!("/values_by_field_id/{FIELD}")),
        Some(&json!(42))
    );
    assert_eq!(get_record.uri, format!("/api/v1/flow/objects/{RECORD}"));
    assert_eq!(patch_record.uri, format!("/api/v1/flow/objects/{COLLECTION}/commands"));
    assert_eq!(patch_record.body.pointer("/command/type"), Some(&json!("record_patch")));
    Ok(())
}

#[tokio::test]
async fn mcp_cli_create_equivalence() -> TestResult {
    let (api_url, calls) = api().await?;
    let config = config(&api_url)?;
    let cwd = config.path().parent().ok_or("config path has no parent")?;
    let schema = json!({
        "initial_fields": [{"field_id": FIELD, "label": "Score", "field_type": "number"}],
        "initial_view": {"view_id": "66666666-6666-4666-8666-666666666666", "label": "All"}
    });
    let schema_file = write_json(cwd, "equivalent-schema.json", &schema)?;
    let client = OpenPrClient::new(ClientConfig {
        base_url: api_url,
        credential: Some(Secret::new(TOKEN)),
        workspace_id: WORKSPACE.to_string(),
        transport_label: TRANSPORT_LABEL_STDIO,
    })?;

    let mcp_result = create_flow_object(
        &client,
        json!({
            "workspace_id": WORKSPACE,
            "type": "collection",
            "title": "Same Collection",
            "initial_fields": schema.get("initial_fields").ok_or("schema has no initial_fields")?,
            "initial_view": schema.get("initial_view").ok_or("schema has no initial_view")?,
            "idempotency_key": "same-create-key"
        }),
    )
    .await;
    assert_eq!(mcp_result.is_error, None);
    let Some(ToolContent::Text { text }) = mcp_result.content.first() else {
        return Err("MCP create did not return JSON text".into());
    };
    assert_eq!(serde_json::from_str::<Value>(text)?, json!({"accepted": true}));

    assert_success(
        &run(
            &config,
            &[
                "objects",
                "create",
                "--workspace",
                WORKSPACE,
                "--type",
                "collection",
                "--title",
                "Same Collection",
                "--schema-file",
                &schema_file,
                "--idempotency-key",
                "same-create-key",
            ],
        )
        .await?,
        "objects.create",
    )?;

    let calls = calls.lock().await.clone();
    assert_eq!(calls.len(), 2);
    let [mcp_call, cli_call] = calls.as_slice() else {
        return Err("expected one MCP and one CLI call".into());
    };
    assert_eq!(
        mcp_call, cli_call,
        "MCP and shipped CLI must make the same REST request"
    );
    Ok(())
}
