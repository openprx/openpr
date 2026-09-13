//! v0.8 admin MCP operations preserve REST reauthorization and never synthesize execute inputs.
#![allow(clippy::indexing_slicing)]

use std::{error::Error, sync::Arc};

use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mcp_server::{
    client::{ClientConfig, OpenPrClient, TRANSPORT_LABEL_STDIO},
    tools::objects::{
        compact_flow_document, compact_flow_document_tool, rebuild_flow_projection, rebuild_flow_projection_tool,
    },
};
use platform::config::Secret;
use serde_json::{Value, json};

const OBJECT: &str = "11111111-1111-4111-8111-111111111111";
const FOREIGN_OBJECT: &str = "22222222-2222-4222-8222-222222222222";
const DOCUMENT: &str = "33333333-3333-4333-8333-333333333333";
const FOREIGN_DOCUMENT: &str = "44444444-4444-4444-8444-444444444444";

#[derive(Clone, Debug)]
struct Call {
    method: String,
    uri: String,
    body: Value,
}

#[derive(Clone, Default)]
struct Fixture {
    calls: Arc<tokio::sync::Mutex<Vec<Call>>>,
    canonical_writes: Arc<std::sync::atomic::AtomicUsize>,
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"code":status.as_u16(),"message":message}))).into_response()
}

async fn api_fixture(State(fixture): State<Fixture>, request: Request) -> Response {
    let authorized = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        == Some("Bearer admin-token");
    if !authorized {
        return error(StatusCode::FORBIDDEN, "admin required");
    }
    let method = request.method().to_string();
    let uri = request.uri().path().to_string();
    let bytes = to_bytes(request.into_body(), 1_048_576).await.unwrap_or_default();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    fixture.calls.lock().await.push(Call {
        method: method.clone(),
        uri: uri.clone(),
        body: body.clone(),
    });

    if method == "GET" && uri.ends_with("/collab") {
        let document_id = if uri.contains(FOREIGN_OBJECT) {
            FOREIGN_DOCUMENT
        } else {
            DOCUMENT
        };
        return Json(json!({"code":0,"data":{"document_id":document_id}})).into_response();
    }
    if uri.contains(FOREIGN_OBJECT) {
        return error(StatusCode::FORBIDDEN, "object outside admin scope");
    }
    let Some(dry_run) = body.get("dry_run").and_then(Value::as_bool) else {
        return error(StatusCode::BAD_REQUEST, "dry_run is required");
    };
    if body.get("expected_head_seq").and_then(Value::as_i64).is_none() {
        return error(StatusCode::BAD_REQUEST, "expected_head_seq is required");
    }
    if !body
        .get("idempotency_key")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
    {
        return error(StatusCode::BAD_REQUEST, "idempotency_key is required");
    }
    if !dry_run {
        let confirm_matches = if uri.ends_with("/compact") {
            body.get("confirm_document_id").and_then(Value::as_str) == Some(DOCUMENT)
        } else {
            body.get("confirm_object_id").and_then(Value::as_str) == Some(OBJECT)
        };
        if !confirm_matches {
            return error(StatusCode::FORBIDDEN, "exact confirm required");
        }
        fixture
            .canonical_writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    Json(json!({"code":0,"data":{"dry_run":dry_run,"status":"completed"}})).into_response()
}

fn client(base_url: &str, credential: Option<&str>) -> Result<OpenPrClient, Box<dyn Error>> {
    Ok(OpenPrClient::new(ClientConfig {
        base_url: base_url.to_string(),
        credential: credential.map(|value| Secret::new(value.to_string())),
        workspace_id: "55555555-5555-4555-8555-555555555555".to_string(),
        transport_label: TRANSPORT_LABEL_STDIO,
    })?)
}

#[tokio::test]
async fn mcp_admin_operations_fail_closed_and_only_exact_execute_changes_canonical_state() -> Result<(), Box<dyn Error>>
{
    let fixture = Fixture::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let router = Router::new().fallback(api_fixture).with_state(fixture.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let base_url = format!("http://{address}");
    let admin = client(&base_url, Some("admin-token"))?;
    let wrong_admin = client(&base_url, Some("member-token"))?;
    let missing_admin = client(&base_url, None)?;

    let compact_dry = json!({
        "object_id":OBJECT,"document_id":DOCUMENT,"dry_run":true,"expected_head_seq":7,
        "idempotency_key":"compact-dry"
    });
    assert!(
        compact_flow_document(&admin, compact_dry.clone())
            .await
            .is_error
            .is_none()
    );
    assert_eq!(fixture.canonical_writes.load(std::sync::atomic::Ordering::SeqCst), 0);

    for (client, args) in [
        (&missing_admin, compact_dry.clone()),
        (&wrong_admin, compact_dry.clone()),
        (
            &admin,
            json!({"object_id":FOREIGN_OBJECT,"document_id":DOCUMENT,"dry_run":true,"expected_head_seq":7,"idempotency_key":"wrong-scope"}),
        ),
        (
            &admin,
            json!({"object_id":OBJECT,"document_id":DOCUMENT,"dry_run":true,"idempotency_key":"missing-head"}),
        ),
        (
            &admin,
            json!({"object_id":OBJECT,"document_id":DOCUMENT,"dry_run":true,"expected_head_seq":7,"idempotency_key":""}),
        ),
        (
            &admin,
            json!({"object_id":OBJECT,"document_id":DOCUMENT,"dry_run":false,"expected_head_seq":7,"confirm_document_id":FOREIGN_DOCUMENT,"idempotency_key":"wrong-confirm"}),
        ),
    ] {
        assert!(compact_flow_document(client, args).await.is_error == Some(true));
        assert_eq!(fixture.canonical_writes.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    assert!(
        compact_flow_document(
            &admin,
            json!({"object_id":OBJECT,"document_id":DOCUMENT,"dry_run":false,"expected_head_seq":7,"confirm_document_id":DOCUMENT,"idempotency_key":"compact-execute"}),
        )
        .await
        .is_error
        .is_none()
    );
    assert_eq!(fixture.canonical_writes.load(std::sync::atomic::Ordering::SeqCst), 1);

    let projection_dry =
        json!({"object_id":OBJECT,"dry_run":true,"expected_head_seq":7,"idempotency_key":"projection-dry"});
    assert!(rebuild_flow_projection(&admin, projection_dry).await.is_error.is_none());
    assert_eq!(fixture.canonical_writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    for args in [
        json!({"object_id":FOREIGN_OBJECT,"dry_run":true,"expected_head_seq":7,"idempotency_key":"wrong-scope"}),
        json!({"object_id":OBJECT,"dry_run":true,"idempotency_key":"missing-head"}),
        json!({"object_id":OBJECT,"dry_run":true,"expected_head_seq":7,"idempotency_key":""}),
        json!({"object_id":OBJECT,"dry_run":false,"expected_head_seq":7,"confirm_object_id":FOREIGN_OBJECT,"idempotency_key":"wrong-confirm"}),
    ] {
        assert!(rebuild_flow_projection(&admin, args).await.is_error == Some(true));
        assert_eq!(fixture.canonical_writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
    assert!(
        rebuild_flow_projection(
            &admin,
            json!({"object_id":OBJECT,"dry_run":false,"expected_head_seq":7,"confirm_object_id":OBJECT,"idempotency_key":"projection-execute"}),
        )
        .await
        .is_error
        .is_none()
    );
    assert_eq!(fixture.canonical_writes.load(std::sync::atomic::Ordering::SeqCst), 2);

    for schema in [
        compact_flow_document_tool().input_schema,
        rebuild_flow_projection_tool().input_schema,
    ] {
        assert_eq!(schema["additionalProperties"], false);
        let required = schema["required"].as_array().expect("required is an array");
        for key in ["dry_run", "expected_head_seq", "idempotency_key"] {
            assert!(required.contains(&json!(key)), "schema must require {key}");
        }
        assert!(
            !schema.to_string().contains("default"),
            "MCP must not invent execute defaults"
        );
    }

    let calls = fixture.calls.lock().await;
    assert!(
        calls
            .iter()
            .any(|call| call.method == "POST" && call.uri.ends_with("/compact"))
    );
    assert!(calls.iter().any(|call| {
        call.method == "POST" && call.uri.ends_with("/rebuild-projection") && call.body["confirm_object_id"] == OBJECT
    }));
    Ok(())
}
