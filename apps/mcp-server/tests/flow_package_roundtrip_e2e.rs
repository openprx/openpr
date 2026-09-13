//! v0.8 package MCP tools must preserve the frozen REST chain and package policy vocabulary.
#![allow(clippy::indexing_slicing)]

use std::{error::Error, sync::Arc};

use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
};
use mcp_server::{
    client::{ClientConfig, OpenPrClient, TRANSPORT_LABEL_STDIO},
    tools::objects::{
        export_flow_object, export_flow_workspace, import_flow_artifact, import_flow_artifact_tool, import_flow_commit,
        import_flow_commit_tool, import_flow_preview, import_flow_preview_tool, import_flow_status,
    },
};
use platform::config::Secret;
use serde_json::{Value, json};

#[derive(Clone, Debug)]
struct Call {
    method: String,
    uri: String,
    content_type: String,
    body: Value,
}

type Calls = Arc<tokio::sync::Mutex<Vec<Call>>>;
const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const OBJECT: &str = "22222222-2222-4222-8222-222222222222";
const ARTIFACT: &str = "33333333-3333-4333-8333-333333333333";
const IMPORT: &str = "44444444-4444-4444-8444-444444444444";
const PACKAGE_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MAPPING_HASH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

async fn capture(State(calls): State<Calls>, request: Request) -> Json<Value> {
    let method = request.method().to_string();
    let uri = request.uri().to_string();
    let content_type = request
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = to_bytes(request.into_body(), 1_048_576).await.unwrap_or_default();
    let body = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({"raw_bytes":bytes.len()}));
    calls.lock().await.push(Call {
        method,
        uri: uri.clone(),
        content_type,
        body,
    });
    let data = if uri.ends_with("/flow/import-artifacts") {
        json!({"artifact_id":ARTIFACT,"package_sha256":PACKAGE_HASH,"size":8})
    } else if uri.ends_with("/flow/imports/preview") {
        json!({"preview_id":IMPORT,"package_sha256":PACKAGE_HASH,"mapping_hash":MAPPING_HASH})
    } else if uri.ends_with("/commit") || uri.ends_with(IMPORT) {
        json!({"import_id":IMPORT,"status":"completed"})
    } else {
        json!({"accepted":true})
    };
    Json(json!({"code":0,"data":data}))
}

#[tokio::test]
async fn package_tools_follow_artifact_preview_commit_status_with_exact_contract_values() -> Result<(), Box<dyn Error>>
{
    let calls: Calls = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let router = Router::new().fallback(capture).with_state(Arc::clone(&calls));
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let client = OpenPrClient::new(ClientConfig {
        base_url: format!("http://{address}"),
        credential: Some(Secret::new("opr_flow_package_e2e")),
        workspace_id: WORKSPACE.to_string(),
        transport_label: TRANSPORT_LABEL_STDIO,
    })?;

    let results = [
        export_flow_object(
            &client,
            json!({"object_id":OBJECT,"format":"package","include_history":true,"idempotency_key":"object-export"}),
        )
        .await,
        export_flow_workspace(
            &client,
            json!({"workspace_id":WORKSPACE,"include_history":false,"idempotency_key":"workspace-export"}),
        )
        .await,
        import_flow_artifact(
            &client,
            json!({"workspace_id":WORKSPACE,"package_base64":"c25hcHNob3Q=","package_sha256":PACKAGE_HASH,"idempotency_key":"inline-artifact"}),
        )
        .await,
        import_flow_artifact(
            &client,
            json!({"workspace_id":WORKSPACE,"staged_object":{"object_key":format!("flow-package-staging/{WORKSPACE}/fixture.zip"),"package_sha256":PACKAGE_HASH,"size":8},"idempotency_key":"staged-artifact"}),
        )
        .await,
        import_flow_preview(
            &client,
            json!({"workspace_id":WORKSPACE,"artifact_id":ARTIFACT,"project_mapping":{},"external_reference_policy":"detach","conflict_policy":"reject_existing","include_history":false,"idempotency_key":"preview"}),
        )
        .await,
        import_flow_commit(
            &client,
            json!({"workspace_id":WORKSPACE,"import_id":IMPORT,"package_sha256":PACKAGE_HASH,"mapping_hash":MAPPING_HASH,"conflict_policy":"reject_existing","confirm":true,"idempotency_key":"commit"}),
        )
        .await,
        import_flow_status(&client, json!({"workspace_id":WORKSPACE,"import_id":IMPORT})).await,
    ];
    assert!(results.iter().all(|result| result.is_error.is_none()));

    let calls = calls.lock().await.clone();
    assert_eq!(calls.len(), 7);
    assert_eq!(
        (&calls[0].method, &calls[0].uri),
        (&"POST".to_string(), &format!("/api/v1/flow/objects/{OBJECT}/exports"))
    );
    assert_eq!(calls[0].body["format"], "package");
    assert_eq!(calls[1].uri, format!("/api/v1/workspaces/{WORKSPACE}/flow/exports"));
    assert_eq!(calls[1].body["format"], "package");
    assert!(calls[2].content_type.starts_with("multipart/form-data; boundary="));
    assert!(calls[2].body["raw_bytes"].as_u64().is_some_and(|size| size > 8));
    assert_eq!(calls[3].body["source"]["kind"], "staged_object");
    assert_eq!(calls[4].body["external_reference_policy"], "detach");
    assert_eq!(calls[4].body["conflict_policy"], "reject_existing");
    assert_eq!(calls[5].body["confirm"], true);
    assert_eq!(calls[5].body["conflict_policy"], "reject_existing");
    assert_eq!(calls[6].method, "GET");

    for tool in [
        import_flow_artifact_tool(),
        import_flow_preview_tool(),
        import_flow_commit_tool(),
    ] {
        assert_eq!(tool.input_schema["additionalProperties"], false);
    }
    assert_eq!(
        import_flow_preview_tool().input_schema["properties"]["external_reference_policy"]["enum"],
        json!(["reject", "detach"])
    );
    assert_eq!(
        import_flow_commit_tool().input_schema["properties"]["conflict_policy"]["enum"],
        json!(["reject_existing", "reuse_import_lineage"])
    );
    Ok(())
}
