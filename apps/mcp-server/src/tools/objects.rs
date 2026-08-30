//! Flow read tools: `objects.get`, `objects.query`, `objects.history`.
//!
//! These three mirror the three read endpoints `apps/api/src/routes/flow.rs` ships today
//! (`GET /flow/objects/{id}`, `GET /workspaces/{workspace_id}/flow/objects`,
//! `GET /flow/objects/{id}/history`) one for one — `mcp-surface-v1.md`: "所有输出是 REST `data`
//! 的 semantic JSON". Every response is the REST envelope's `data` verbatim: `FlowObjectView`
//! carries a base64 `frontier` (an opaque version-vector marker used for optimistic
//! concurrency), never a CRDT snapshot or update payload — the raw-bytes exclusion
//! (`TM-CRDT-BYTES-1`) governs the bootstrap endpoint, which is not exposed as a tool at all.
//!
//! `objects.get` and `objects.history` are `PolicyScope::OwnedBy(OwnerLookup::FlowObject)`
//! (`server.rs`): the owning project, if any, is read back from the API rather than trusted
//! from the caller. `objects.query` is `PolicyScope::DeclaredProject { required: false }`: a
//! caller may address a project's objects directly, or pass `unprojected=true` to list objects
//! that belong to no project, in which case the call runs workspace-wide with no project
//! policy to evaluate — `mcp-surface-v1.md` describes this as `project_id=None` falling back
//! to `WorkspaceWide`.

use crate::client::{OpenPrClient, encode_query_component, rejected_request_error};
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug)]
struct StructuredApiError {
    message: String,
    error_code: Option<String>,
    details: Option<Value>,
}

impl StructuredApiError {
    const fn transport(message: String) -> Self {
        Self {
            message,
            error_code: None,
            details: None,
        }
    }
}

const UNAUTHENTICATED_MESSAGE: &str = "the OpenPR API rejected the credential this call was made with; check that the bot token presented is correct, enabled and not expired";

/// Flow tools need the typed `{error_code,details}` fields the legacy String-returning client
/// helpers intentionally collapse. Kept local to this allowed file so unrelated MCP tools retain
/// their established plain-error behavior.
async fn get_structured(client: &OpenPrClient, path: &str) -> Result<Value, StructuredApiError> {
    let url = format!("{}{path}", client.base_url);
    let response = client
        .operation_headers(client.client.get(&url))
        .header(
            "Authorization",
            client.authorization().map_err(StructuredApiError::transport)?,
        )
        .send()
        .await
        .map_err(|err| StructuredApiError::transport(format!("Request failed: {err}")))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|err| StructuredApiError::transport(format!("Failed to read response body from {path}: {err}")))?;
    if !status.is_success() {
        return Err(StructuredApiError::transport(rejected_request_error(
            status, path, &body,
        )));
    }
    let payload: Value = serde_json::from_str(&body)
        .map_err(|err| StructuredApiError::transport(format!("Failed to deserialize response from {path}: {err}")))?;
    let Some(envelope) = payload.as_object() else {
        return Err(StructuredApiError::transport(format!(
            "Malformed response from {path}: expected an API envelope"
        )));
    };
    match envelope.get("code").and_then(Value::as_i64) {
        Some(0) => Ok(payload),
        Some(code) => Err(StructuredApiError {
            // Match the legacy client's information-disclosure boundary: never relay backend
            // operator prose to a caller whose credential was not accepted.
            message: if code == 401 {
                UNAUTHENTICATED_MESSAGE.to_string()
            } else {
                envelope
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown API error")
                    .to_string()
            },
            error_code: envelope.get("error_code").and_then(Value::as_str).map(str::to_string),
            details: envelope.get("details").cloned(),
        }),
        None => Err(StructuredApiError::transport(format!(
            "Malformed response from {path}: envelope carries no integer code"
        ))),
    }
}

fn parse_input<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, CallToolResult> {
    serde_json::from_value(args).map_err(|err| CallToolResult::error(format!("Invalid input: {err}")))
}

fn recoverable_business_error(code: &str) -> bool {
    matches!(
        code,
        "unauthenticated" | "stale_frontier" | "limit_exceeded" | "resync_required" | "server_draining"
    )
}

fn respond(result: Result<Value, StructuredApiError>) -> CallToolResult {
    match result {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(error) => {
            let Some(code) = error.error_code.as_deref() else {
                return CallToolResult::error(error.message);
            };
            CallToolResult::business_error(
                code,
                error.message,
                recoverable_business_error(code),
                error.details.as_ref().unwrap_or(&Value::Null),
            )
        }
    }
}

/// Keeps the pre-existing public client helpers part of the compiled client surface while these
/// tools use the richer envelope reader. Downstream code may still call the String-returning
/// helpers directly.
fn retain_client_method<T>(_method: T) {}

// ---- objects.get ----

pub fn get_flow_object_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.get".to_string(),
        description: "Get one Flow object (title, semantic content, document seq/frontier). \
Returns the same projection the REST and CLI surfaces read; never raw CRDT bytes."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "object_id": { "type": "string", "description": "Flow object UUID" },
                "at_seq": { "type": "integer", "description": "Read at this document seq; only the current head is available in v0.4" },
                "render": { "type": "string", "enum": ["semantic_json", "markdown"], "description": "Projection to render, default semantic_json" }
            },
            "required": ["object_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct GetFlowObjectInput {
    object_id: String,
    at_seq: Option<i64>,
    render: Option<String>,
}

pub async fn get_flow_object(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: GetFlowObjectInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };

    let mut query = Vec::new();
    if let Some(at_seq) = input.at_seq {
        query.push(format!("at_seq={at_seq}"));
    }
    if let Some(render) = input.render.as_deref() {
        if !matches!(render, "semantic_json" | "markdown") {
            return CallToolResult::error("render must be semantic_json or markdown".to_string());
        }
        query.push(format!("render={render}"));
    }
    let suffix = query_suffix(&query);

    let path = format!(
        "/api/v1/flow/objects/{}{suffix}",
        encode_query_component(&input.object_id)
    );
    respond(get_structured(client, &path).await)
}

// ---- objects.query ----

pub fn query_flow_objects_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.query".to_string(),
        description: "List Flow objects in a workspace, scoped to one project or, with \
unprojected=true, to objects that belong to no project. project_id and unprojected are mutually \
exclusive."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "workspace_id": { "type": "string", "description": "Workspace UUID" },
                "project_id": { "type": "string", "description": "Project UUID; mutually exclusive with unprojected" },
                "unprojected": { "type": "boolean", "description": "List only objects with no owning project; mutually exclusive with project_id" },
                "type": { "type": "string", "description": "Filter by object_type" },
                "q": { "type": "string", "description": "Title prefix filter" },
                "cursor": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 50 }
            },
            "required": ["workspace_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct QueryFlowObjectsInput {
    workspace_id: String,
    project_id: Option<String>,
    #[serde(default)]
    unprojected: bool,
    #[serde(rename = "type")]
    object_type: Option<String>,
    q: Option<String>,
    cursor: Option<String>,
    limit: Option<u64>,
}

pub async fn query_flow_objects(client: &OpenPrClient, args: Value) -> CallToolResult {
    // Keep the public legacy String-returning helper live for downstream callers even though this
    // Flow tool must use the structured path below to preserve business-error details.
    retain_client_method(OpenPrClient::list_flow_objects);
    let input: QueryFlowObjectsInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };

    if input.project_id.is_some() && input.unprojected {
        return CallToolResult::error("project_id and unprojected are mutually exclusive".to_string());
    }

    let mut query = Vec::new();
    if let Some(project_id) = input.project_id.as_deref() {
        query.push(format!("project_id={}", encode_query_component(project_id)));
    }
    if input.unprojected {
        query.push("unprojected=true".to_string());
    }
    if let Some(object_type) = input.object_type.as_deref() {
        query.push(format!("object_type={}", encode_query_component(object_type)));
    }
    if let Some(q) = input.q.as_deref() {
        query.push(format!("q={}", encode_query_component(q)));
    }
    if let Some(cursor) = input.cursor.as_deref() {
        query.push(format!("cursor={}", encode_query_component(cursor)));
    }
    if let Some(limit) = input.limit {
        if !(1..=100).contains(&limit) {
            return CallToolResult::error("limit must be between 1 and 100".to_string());
        }
        query.push(format!("limit={limit}"));
    }
    let suffix = query_suffix(&query);

    let path = format!(
        "/api/v1/workspaces/{}/flow/objects{suffix}",
        encode_query_component(&input.workspace_id)
    );
    respond(get_structured(client, &path).await)
}

// ---- objects.history ----

pub fn get_flow_object_history_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.history".to_string(),
        description: "Get one Flow object's accepted-update history page (seq, actor, origin, \
message, semantic summary). No raw update bytes."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "object_id": { "type": "string", "description": "Flow object UUID" },
                "before_seq": { "type": "integer", "description": "Return items strictly before this seq" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 50 }
            },
            "required": ["object_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct GetFlowObjectHistoryInput {
    object_id: String,
    before_seq: Option<i64>,
    limit: Option<u64>,
}

pub async fn get_flow_object_history(client: &OpenPrClient, args: Value) -> CallToolResult {
    // See `query_flow_objects`: this symbol remains part of the client API, while the tool itself
    // needs the structured envelope path.
    retain_client_method(OpenPrClient::get_flow_object_history);
    let input: GetFlowObjectHistoryInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };

    let mut query = Vec::new();
    if let Some(before_seq) = input.before_seq {
        query.push(format!("before_seq={before_seq}"));
    }
    if let Some(limit) = input.limit {
        if !(1..=100).contains(&limit) {
            return CallToolResult::error("limit must be between 1 and 100".to_string());
        }
        query.push(format!("limit={limit}"));
    }
    let suffix = query_suffix(&query);

    let path = format!(
        "/api/v1/flow/objects/{}/history{suffix}",
        encode_query_component(&input.object_id)
    );
    respond(get_structured(client, &path).await)
}

fn query_suffix(params: &[String]) -> String {
    if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::{get_flow_object, get_flow_object_history, query_flow_objects};
    use crate::client::test_api;
    use axum::{Json, Router, routing::get};
    use serde_json::json;

    #[tokio::test]
    async fn objects_get_rejects_missing_object_id() -> Result<(), String> {
        let client = test_api::client("http://127.0.0.1:1".to_string())?;
        let result = get_flow_object(&client, json!({})).await;
        assert_eq!(result.is_error, Some(true));
        Ok(())
    }

    #[tokio::test]
    async fn objects_get_rejects_unknown_render() -> Result<(), String> {
        let client = test_api::client("http://127.0.0.1:1".to_string())?;
        let result = get_flow_object(
            &client,
            json!({ "object_id": "11111111-1111-4111-8111-111111111111", "render": "html" }),
        )
        .await;
        assert_eq!(result.is_error, Some(true));
        Ok(())
    }

    #[tokio::test]
    async fn objects_query_rejects_project_id_with_unprojected() -> Result<(), String> {
        let client = test_api::client("http://127.0.0.1:1".to_string())?;
        let result = query_flow_objects(
            &client,
            json!({
                "workspace_id": "11111111-1111-4111-8111-111111111111",
                "project_id": "22222222-2222-4222-8222-222222222222",
                "unprojected": true
            }),
        )
        .await;
        assert_eq!(result.is_error, Some(true));
        Ok(())
    }

    #[tokio::test]
    async fn objects_query_rejects_out_of_range_limit() -> Result<(), String> {
        let client = test_api::client("http://127.0.0.1:1".to_string())?;
        let result = query_flow_objects(
            &client,
            json!({ "workspace_id": "11111111-1111-4111-8111-111111111111", "limit": 101 }),
        )
        .await;
        assert_eq!(result.is_error, Some(true));
        Ok(())
    }

    #[tokio::test]
    async fn objects_history_rejects_missing_object_id() -> Result<(), String> {
        let client = test_api::client("http://127.0.0.1:1".to_string())?;
        let result = get_flow_object_history(&client, json!({})).await;
        assert_eq!(result.is_error, Some(true));
        Ok(())
    }

    #[tokio::test]
    async fn flow_server_draining_is_a_structured_mcp_business_error() -> Result<(), Box<dyn std::error::Error>> {
        let router = Router::new().route(
            "/api/v1/flow/objects/{object_id}",
            get(|| async {
                Json(json!({
                    "code": 409,
                    "message": "server_draining",
                    "data": null,
                    "error_code": "server_draining",
                    "details": {"reason": "drain", "retry_after_ms": 1500}
                }))
            }),
        );
        let base_url = test_api::spawn(router).await?;
        let client = test_api::client(base_url)?;
        let result = get_flow_object(&client, json!({"object_id": "11111111-1111-4111-8111-111111111111"})).await;

        assert_eq!(result.is_error, Some(true));
        let Some(crate::protocol::ToolContent::Text { text }) = result.content.first() else {
            return Err("missing MCP text content".into());
        };
        let body: serde_json::Value = serde_json::from_str(text)?;
        assert_eq!(body["error"]["code"], "server_draining");
        assert_eq!(body["error"]["recoverable"], true);
        assert_eq!(body["error"]["details"]["reason"], "drain");
        assert_eq!(body["error"]["details"]["retry_after_ms"], 1500);
        Ok(())
    }

    #[tokio::test]
    async fn structured_flow_errors_do_not_relay_unauthenticated_backend_prose()
    -> Result<(), Box<dyn std::error::Error>> {
        let router = Router::new().route(
            "/api/v1/flow/objects/{object_id}",
            get(|| async {
                Json(json!({
                    "code": 401,
                    "message": "credential lookup failed at pg-primary.internal",
                    "data": null,
                    "error_code": "unauthenticated",
                    "details": null
                }))
            }),
        );
        let base_url = test_api::spawn(router).await?;
        let client = test_api::client(base_url)?;
        let result = get_flow_object(&client, json!({"object_id": "11111111-1111-4111-8111-111111111111"})).await;
        let Some(crate::protocol::ToolContent::Text { text }) = result.content.first() else {
            return Err("missing MCP text content".into());
        };
        assert!(text.contains("rejected the credential"));
        assert!(!text.contains("pg-primary.internal"));
        Ok(())
    }
}
