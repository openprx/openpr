//! v0.7 bridge MCP handlers must reach the frozen REST paths with the same payloads.

use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
};
use mcp_server::{
    client::{ClientConfig, OpenPrClient, TRANSPORT_LABEL_STDIO},
    tools::objects::{
        convert_commit, convert_preview, convert_retry, convert_status, reference_flow_object, unreference_flow_object,
    },
};
use platform::config::Secret;
use serde_json::{Value, json};
use std::{error::Error, sync::Arc};

type Calls = Arc<tokio::sync::Mutex<Vec<(String, String, Value, Option<String>)>>>;
const SOURCE: &str = "11111111-1111-4111-8111-111111111111";
const TARGET: &str = "22222222-2222-4222-8222-222222222222";
const REFERENCE: &str = "33333333-3333-4333-8333-333333333333";
const PREVIEW: &str = "44444444-4444-4444-8444-444444444444";
const JOB: &str = "55555555-5555-4555-8555-555555555555";

async fn capture(State(calls): State<Calls>, request: Request) -> Json<Value> {
    let method = request.method().to_string();
    let uri = request.uri().to_string();
    let idempotency_key = request
        .headers()
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = to_bytes(request.into_body(), 1_048_576)
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null);
    calls.lock().await.push((method, uri, body, idempotency_key));
    Json(json!({"code":0,"data":{"accepted":true}}))
}

#[tokio::test]
async fn all_six_bridge_tools_call_the_frozen_rest_contract() -> Result<(), Box<dyn Error>> {
    let calls: Calls = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let router = Router::new().fallback(capture).with_state(Arc::clone(&calls));
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let client = OpenPrClient::new(ClientConfig {
        base_url: format!("http://{address}"),
        credential: Some(Secret::new("opr_flow_bridge_e2e")),
        workspace_id: SOURCE.to_string(),
        transport_label: TRANSPORT_LABEL_STDIO,
    })?;

    let results = [
        reference_flow_object(&client, json!({"object_id":SOURCE,"target_type":"form","target_id":TARGET,"display":{},"idempotency_key":"ref-key"})).await,
        unreference_flow_object(&client, json!({"object_id":SOURCE,"reference_id":REFERENCE,"idempotency_key":"unref-key"})).await,
        convert_preview(&client, json!({"source_object_id":SOURCE,"source_frontier":"frontier","target_type":"form_record","mapping":{"target_form_id":TARGET},"idempotency_key":"preview-key"})).await,
        convert_commit(&client, json!({"preview_id":PREVIEW,"source_frontier":"frontier","target_schema_version":7,"confirm":true,"idempotency_key":"commit-key"})).await,
        convert_status(&client, json!({"job_id":JOB})).await,
        convert_retry(&client, json!({"job_id":JOB,"confirm":true,"idempotency_key":"retry-key"})).await,
    ];
    assert!(results.iter().all(|result| result.is_error.is_none()));

    let calls = calls.lock().await.clone();
    let [reference, unreference, preview, commit, status, retry] = calls.as_slice() else {
        return Err(format!("expected six calls, got {}", calls.len()).into());
    };
    assert_eq!(
        (&reference.0, &reference.1),
        (
            &"POST".to_string(),
            &format!("/api/v1/flow/objects/{SOURCE}/references")
        )
    );
    assert_eq!(
        (&unreference.0, &unreference.1, unreference.3.as_deref()),
        (
            &"DELETE".to_string(),
            &format!("/api/v1/flow/objects/{SOURCE}/references/{REFERENCE}"),
            Some("unref-key")
        )
    );
    assert_eq!(
        (&preview.0, &preview.1),
        (&"POST".to_string(), &"/api/v1/flow/conversions/preview".to_string())
    );
    assert_eq!(preview.2.pointer("/source_frontier"), Some(&json!("frontier")));
    assert_eq!(
        (&commit.0, &commit.1),
        (&"POST".to_string(), &"/api/v1/flow/conversions".to_string())
    );
    assert_eq!(commit.2.pointer("/confirm"), Some(&json!(true)));
    assert_eq!(
        (&status.0, &status.1),
        (&"GET".to_string(), &format!("/api/v1/flow/conversions/{JOB}"))
    );
    assert_eq!(
        (&retry.0, &retry.1),
        (&"POST".to_string(), &format!("/api/v1/flow/conversions/{JOB}/retry"))
    );
    Ok(())
}
