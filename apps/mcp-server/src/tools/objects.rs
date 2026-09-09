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
use reqwest::RequestBuilder;
use serde::{Deserialize, Serialize};
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
async fn send_structured(
    client: &OpenPrClient,
    request: RequestBuilder,
    path: &str,
) -> Result<Value, StructuredApiError> {
    let response = client
        .operation_headers(request)
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

async fn get_structured(client: &OpenPrClient, path: &str) -> Result<Value, StructuredApiError> {
    let url = format!("{}{path}", client.base_url);
    send_structured(client, client.client.get(&url), path).await
}

async fn post_structured(client: &OpenPrClient, path: &str, body: &Value) -> Result<Value, StructuredApiError> {
    let url = format!("{}{path}", client.base_url);
    send_structured(client, client.client.post(&url).json(body), path).await
}

async fn put_structured(client: &OpenPrClient, path: &str, body: &Value) -> Result<Value, StructuredApiError> {
    let url = format!("{}{path}", client.base_url);
    send_structured(client, client.client.put(&url).json(body), path).await
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

fn respond_data(result: Result<Value, StructuredApiError>) -> CallToolResult {
    respond(result.map(|envelope| envelope.get("data").cloned().unwrap_or(Value::Null)))
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

// ---- Flow v0.5 write and derived read tools ----

fn required_write_key(key: &str) -> Result<(), CallToolResult> {
    if (1..=128).contains(&key.len()) {
        Ok(())
    } else {
        Err(CallToolResult::error(
            "idempotency_key must contain between 1 and 128 bytes".to_string(),
        ))
    }
}

fn command_body(command_type: &str, payload: &Value, idempotency_key: &str, message: Option<&str>) -> Value {
    let mut body = json!({
        "command": { "type": command_type, "payload": payload },
        "idempotency_key": idempotency_key,
    });
    if let (Some(message), Some(object)) = (message, body.as_object_mut()) {
        object.insert("message".to_string(), json!(message));
    }
    body
}

pub fn create_flow_object_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.create".to_string(),
        description: "Create a page or navigator Flow object; same key and body replay the original receipt, while body drift conflicts.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "workspace_id": { "type": "string" },
                "project_id": { "type": "string" },
                "type": { "type": "string", "enum": ["page", "navigator"] },
                "parent_id": { "type": "string" },
                "title": { "type": "string" },
                "idempotency_key": { "type": "string", "minLength": 1, "maxLength": 128 },
                "message": { "type": "string" }
            },
            "required": ["workspace_id", "type", "title", "idempotency_key"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateFlowObjectInput {
    workspace_id: String,
    project_id: Option<String>,
    #[serde(rename = "type")]
    object_type: String,
    parent_id: Option<String>,
    title: String,
    idempotency_key: String,
    message: Option<String>,
}

pub async fn create_flow_object(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: CreateFlowObjectInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    if let Err(result) = required_write_key(&input.idempotency_key) {
        return result;
    }
    if !matches!(input.object_type.as_str(), "page" | "navigator") {
        return CallToolResult::error("type must be page or navigator".to_string());
    }
    let mut body = json!({
        "object_type": input.object_type,
        "title": input.title,
        "idempotency_key": input.idempotency_key,
    });
    if let Some(object) = body.as_object_mut() {
        if let Some(project_id) = input.project_id {
            object.insert("project_id".to_string(), json!(project_id));
        }
        if let Some(parent_id) = input.parent_id {
            object.insert("parent_object_id".to_string(), json!(parent_id));
        }
        if let Some(message) = input.message {
            object.insert("message".to_string(), json!(message));
        }
    }
    let path = format!(
        "/api/v1/workspaces/{}/flow/objects",
        encode_query_component(&input.workspace_id)
    );
    respond_data(post_structured(client, &path, &body).await)
}

pub fn patch_flow_object_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.patch".to_string(),
        description: "Atomically apply one to 100 semantic operations to a Flow object.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "object_id": { "type": "string" },
                "operations": { "type": "array", "minItems": 1, "maxItems": 100, "items": { "type": "object" } },
                "expected_frontier": { "type": "string" },
                "idempotency_key": { "type": "string", "minLength": 1, "maxLength": 128 },
                "message": { "type": "string" }
            },
            "required": ["object_id", "operations", "idempotency_key"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PatchFlowObjectInput {
    object_id: String,
    operations: Vec<Value>,
    expected_frontier: Option<String>,
    idempotency_key: String,
    message: Option<String>,
}

pub async fn patch_flow_object(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: PatchFlowObjectInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    if !(1..=100).contains(&input.operations.len()) {
        return CallToolResult::error("operations must contain between 1 and 100 items".to_string());
    }
    if let Err(result) = required_write_key(&input.idempotency_key) {
        return result;
    }
    let mut body = command_body(
        "semantic_patch",
        &json!({ "operations": input.operations }),
        &input.idempotency_key,
        input.message.as_deref(),
    );
    if let (Some(frontier), Some(object)) = (input.expected_frontier, body.as_object_mut()) {
        object.insert("expected_frontier".to_string(), json!(frontier));
    }
    let path = format!(
        "/api/v1/flow/objects/{}/commands",
        encode_query_component(&input.object_id)
    );
    respond_data(post_structured(client, &path, &body).await)
}

pub fn move_flow_object_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.move".to_string(),
        description: "Move an object under target_object_id, the new parent; authorization is checked on both sides."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "object_id": { "type": "string" },
                "target_object_id": { "type": "string", "description": "New parent object UUID" },
                "after_id": { "type": "string" },
                "expected_target_frontier": { "type": "string" },
                "confirm_self_lockout": { "type": "boolean" },
                "idempotency_key": { "type": "string", "minLength": 1, "maxLength": 128 },
                "message": { "type": "string" }
            },
            "required": ["object_id", "target_object_id", "idempotency_key"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveFlowObjectInput {
    object_id: String,
    target_object_id: String,
    after_id: Option<String>,
    expected_target_frontier: Option<String>,
    #[serde(default)]
    confirm_self_lockout: bool,
    idempotency_key: String,
    message: Option<String>,
}

pub async fn move_flow_object(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: MoveFlowObjectInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    if let Err(result) = required_write_key(&input.idempotency_key) {
        return result;
    }
    let mut payload = json!({
        "target_object_id": input.target_object_id,
        "confirm_self_lockout": input.confirm_self_lockout,
    });
    if let Some(object) = payload.as_object_mut() {
        if let Some(after_id) = input.after_id {
            object.insert("after_id".to_string(), json!(after_id));
        }
        if let Some(frontier) = input.expected_target_frontier {
            object.insert("expected_target_frontier".to_string(), json!(frontier));
        }
    }
    let body = command_body(
        "move_object",
        &payload,
        &input.idempotency_key,
        input.message.as_deref(),
    );
    let path = format!(
        "/api/v1/flow/objects/{}/commands",
        encode_query_component(&input.object_id)
    );
    respond_data(post_structured(client, &path, &body).await)
}

pub fn link_flow_objects_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.link".to_string(),
        description: "Create a typed relation after the API reauthorizes both source and target objects.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "source_object_id": { "type": "string" },
                "target_object_id": { "type": "string" },
                "relation_type": { "type": "string" },
                "properties": { "type": "object" },
                "idempotency_key": { "type": "string", "minLength": 1, "maxLength": 128 },
                "message": { "type": "string" }
            },
            "required": ["source_object_id", "target_object_id", "relation_type", "idempotency_key"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkFlowObjectsInput {
    source_object_id: String,
    target_object_id: String,
    relation_type: String,
    properties: Option<Value>,
    idempotency_key: String,
    message: Option<String>,
}

pub async fn link_flow_objects(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: LinkFlowObjectsInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    if let Err(result) = required_write_key(&input.idempotency_key) {
        return result;
    }
    let payload = json!({
        "target_object_id": input.target_object_id,
        "relation_type": input.relation_type,
        "properties": input.properties.unwrap_or_else(|| json!({})),
    });
    let body = command_body("link", &payload, &input.idempotency_key, input.message.as_deref());
    let path = format!(
        "/api/v1/flow/objects/{}/commands",
        encode_query_component(&input.source_object_id)
    );
    respond_data(post_structured(client, &path, &body).await)
}

pub fn unlink_flow_objects_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.unlink".to_string(),
        description: "Remove one relation from a source Flow object.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "source_object_id": { "type": "string" },
                "relation_id": { "type": "string" },
                "idempotency_key": { "type": "string", "minLength": 1, "maxLength": 128 },
                "message": { "type": "string" }
            },
            "required": ["source_object_id", "relation_id", "idempotency_key"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnlinkFlowObjectsInput {
    source_object_id: String,
    relation_id: String,
    idempotency_key: String,
    message: Option<String>,
}

pub async fn unlink_flow_objects(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: UnlinkFlowObjectsInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    if let Err(result) = required_write_key(&input.idempotency_key) {
        return result;
    }
    let body = command_body(
        "unlink",
        &json!({ "relation_id": input.relation_id }),
        &input.idempotency_key,
        input.message.as_deref(),
    );
    let path = format!(
        "/api/v1/flow/objects/{}/commands",
        encode_query_component(&input.source_object_id)
    );
    respond_data(post_structured(client, &path, &body).await)
}

pub fn diff_flow_object_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.diff".to_string(),
        description: "Read a semantic history diff without CRDT bytes or peer identifiers.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "object_id": { "type": "string" },
                "from_seq": { "type": "integer" },
                "to_seq": { "type": "integer" },
                "render": { "type": "string", "enum": ["semantic_json", "markdown"] }
            },
            "required": ["object_id", "from_seq", "to_seq"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiffFlowObjectInput {
    object_id: String,
    from_seq: i64,
    to_seq: i64,
    render: Option<String>,
}

pub async fn diff_flow_object(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: DiffFlowObjectInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    if input.from_seq > input.to_seq {
        return CallToolResult::error("from_seq must not exceed to_seq".to_string());
    }
    let mut query = vec![
        format!("from_seq={}", input.from_seq),
        format!("to_seq={}", input.to_seq),
    ];
    if let Some(render) = input.render {
        if !matches!(render.as_str(), "semantic_json" | "markdown") {
            return CallToolResult::error("render must be semantic_json or markdown".to_string());
        }
        query.push(format!("render={render}"));
    }
    let path = format!(
        "/api/v1/flow/objects/{}/diff{}",
        encode_query_component(&input.object_id),
        query_suffix(&query)
    );
    respond_data(get_structured(client, &path).await)
}

pub fn get_flow_object_grants_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.grants_get".to_string(),
        description:
            "Read the caller's effective access; the API only includes the full grant roster for full_access callers."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": { "object_id": { "type": "string" } },
            "required": ["object_id"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObjectIdInput {
    object_id: String,
}

pub async fn get_flow_object_grants(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ObjectIdInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    let path = format!(
        "/api/v1/flow/objects/{}/grants",
        encode_query_component(&input.object_id)
    );
    respond_data(get_structured(client, &path).await)
}

fn grant_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "principal_kind": { "type": "string", "enum": ["user", "bot"] },
            "principal_id": { "type": "string" },
            "level": { "type": "string", "enum": ["full_access", "edit", "comment", "view"] }
        },
        "required": ["principal_kind", "principal_id", "level"],
        "additionalProperties": false
    })
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FlowGrantInput {
    principal_kind: String,
    principal_id: String,
    level: String,
}

fn validate_grants(grants: &[FlowGrantInput]) -> Result<(), CallToolResult> {
    for grant in grants {
        if !matches!(grant.principal_kind.as_str(), "user" | "bot") {
            return Err(CallToolResult::error("principal_kind must be user or bot".to_string()));
        }
        if !matches!(grant.level.as_str(), "full_access" | "edit" | "comment" | "view") {
            return Err(CallToolResult::error(
                "grant level must be full_access, edit, comment, or view".to_string(),
            ));
        }
        if uuid::Uuid::parse_str(&grant.principal_id).is_err() {
            return Err(CallToolResult::error("principal_id must be a UUID".to_string()));
        }
    }
    Ok(())
}

pub fn set_flow_object_grants_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.grants_set".to_string(),
        description: "Replace explicit grants, or preview the same full-access-authorized change with zero writes and zero events.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "object_id": { "type": "string" },
                "grants": { "type": "array", "minItems": 0, "maxItems": 100, "items": grant_schema() },
                "confirm_self_lockout": { "type": "boolean" },
                "dry_run": { "type": "boolean" },
                "idempotency_key": { "type": "string", "minLength": 1, "maxLength": 128 },
                "message": { "type": "string" }
            },
            "required": ["object_id", "grants", "idempotency_key"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetFlowObjectGrantsInput {
    object_id: String,
    grants: Vec<FlowGrantInput>,
    #[serde(default)]
    confirm_self_lockout: bool,
    #[serde(default)]
    dry_run: bool,
    idempotency_key: String,
    message: Option<String>,
}

pub async fn set_flow_object_grants(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: SetFlowObjectGrantsInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    if input.grants.len() > 100 {
        return CallToolResult::error("grants must contain at most 100 items".to_string());
    }
    if let Err(result) = validate_grants(&input.grants) {
        return result;
    }
    if let Err(result) = required_write_key(&input.idempotency_key) {
        return result;
    }
    let mut body = json!({
        "grants": input.grants,
        "confirm_self_lockout": input.confirm_self_lockout,
        "dry_run": input.dry_run,
        "idempotency_key": input.idempotency_key,
    });
    if let (Some(message), Some(object)) = (input.message, body.as_object_mut()) {
        object.insert("message".to_string(), json!(message));
    }
    let path = format!(
        "/api/v1/flow/objects/{}/grants",
        encode_query_component(&input.object_id)
    );
    respond_data(put_structured(client, &path, &body).await)
}

pub fn set_flow_object_inheritance_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.inheritance_set".to_string(),
        description: "Set inheritance, optionally replacing initial grants atomically; dry_run retains the full authorization gate and writes nothing.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "object_id": { "type": "string" },
                "inherit_from_parent": { "type": "boolean" },
                "confirm_self_lockout": { "type": "boolean" },
                "dry_run": { "type": "boolean" },
                "initial_grants": { "type": "array", "minItems": 0, "maxItems": 100, "items": grant_schema() },
                "idempotency_key": { "type": "string", "minLength": 1, "maxLength": 128 },
                "message": { "type": "string" }
            },
            "required": ["object_id", "inherit_from_parent", "idempotency_key"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetFlowObjectInheritanceInput {
    object_id: String,
    inherit_from_parent: bool,
    #[serde(default)]
    confirm_self_lockout: bool,
    #[serde(default)]
    dry_run: bool,
    initial_grants: Option<Vec<FlowGrantInput>>,
    idempotency_key: String,
    message: Option<String>,
}

pub async fn set_flow_object_inheritance(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: SetFlowObjectInheritanceInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    if input.initial_grants.as_ref().is_some_and(|grants| grants.len() > 100) {
        return CallToolResult::error("initial_grants must contain at most 100 items".to_string());
    }
    if let Some(grants) = input.initial_grants.as_deref()
        && let Err(result) = validate_grants(grants)
    {
        return result;
    }
    if let Err(result) = required_write_key(&input.idempotency_key) {
        return result;
    }
    let mut body = json!({
        "inherit_from_parent": input.inherit_from_parent,
        "confirm_self_lockout": input.confirm_self_lockout,
        "dry_run": input.dry_run,
        "idempotency_key": input.idempotency_key,
    });
    if let (Some(message), Some(object)) = (input.message, body.as_object_mut()) {
        object.insert("message".to_string(), json!(message));
    }
    if let (Some(grants), Some(object)) = (input.initial_grants, body.as_object_mut()) {
        object.insert("initial_grants".to_string(), json!(grants));
    }
    let path = format!(
        "/api/v1/flow/objects/{}/inheritance",
        encode_query_component(&input.object_id)
    );
    respond_data(put_structured(client, &path, &body).await)
}

pub fn list_flow_object_relations_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.relations".to_string(),
        description: "List policy-filtered relations; an invisible opposite endpoint is represented only as visibility=unavailable.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "object_id": { "type": "string" },
                "direction": { "type": "string", "enum": ["outgoing", "incoming", "both"] },
                "relation_type": { "type": "string" },
                "cursor": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
            },
            "required": ["object_id"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListFlowObjectRelationsInput {
    object_id: String,
    direction: Option<String>,
    relation_type: Option<String>,
    cursor: Option<String>,
    limit: Option<u64>,
}

pub async fn list_flow_object_relations(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ListFlowObjectRelationsInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    let mut query = Vec::new();
    if let Some(direction) = input.direction {
        if !matches!(direction.as_str(), "outgoing" | "incoming" | "both") {
            return CallToolResult::error("direction must be outgoing, incoming, or both".to_string());
        }
        query.push(format!("direction={direction}"));
    }
    if let Some(relation_type) = input.relation_type {
        query.push(format!("relation_type={}", encode_query_component(&relation_type)));
    }
    if let Some(cursor) = input.cursor {
        query.push(format!("cursor={}", encode_query_component(&cursor)));
    }
    if let Some(limit) = input.limit {
        if !(1..=100).contains(&limit) {
            return CallToolResult::error("limit must be between 1 and 100".to_string());
        }
        query.push(format!("limit={limit}"));
    }
    let path = format!(
        "/api/v1/flow/objects/{}/relations{}",
        encode_query_component(&input.object_id),
        query_suffix(&query)
    );
    respond_data(get_structured(client, &path).await)
}

pub fn search_flow_objects_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.search".to_string(),
        description: "Search one project or the projectless scope with per-result API reauthorization; bots have no all_visible escape hatch.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "workspace_id": { "type": "string" },
                "q": { "type": "string", "minLength": 1, "maxLength": 256 },
                "project_id": { "type": "string" },
                "unprojected": { "type": "boolean" },
                "type": { "type": "string" },
                "freshness": { "type": "string", "enum": ["allow_stale", "require_current"] },
                "cursor": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
            },
            "required": ["workspace_id", "q"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchFlowObjectsInput {
    workspace_id: String,
    q: String,
    project_id: Option<String>,
    #[serde(default)]
    unprojected: bool,
    #[serde(rename = "type")]
    object_type: Option<String>,
    freshness: Option<String>,
    cursor: Option<String>,
    limit: Option<u64>,
}

pub async fn search_flow_objects(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: SearchFlowObjectsInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    if input.project_id.is_some() == input.unprojected {
        return CallToolResult::error("exactly one of project_id or unprojected=true is required".to_string());
    }
    if input.q.is_empty() || input.q.chars().count() > 256 {
        return CallToolResult::error("q must contain between 1 and 256 characters".to_string());
    }
    let mut query = vec![format!("q={}", encode_query_component(&input.q))];
    if let Some(project_id) = input.project_id {
        query.push(format!("project_id={}", encode_query_component(&project_id)));
    }
    if input.unprojected {
        query.push("unprojected=true".to_string());
    }
    if let Some(object_type) = input.object_type {
        query.push(format!("object_type={}", encode_query_component(&object_type)));
    }
    if let Some(freshness) = input.freshness {
        if !matches!(freshness.as_str(), "allow_stale" | "require_current") {
            return CallToolResult::error("freshness must be allow_stale or require_current".to_string());
        }
        query.push(format!("freshness={freshness}"));
    }
    if let Some(cursor) = input.cursor {
        query.push(format!("cursor={}", encode_query_component(&cursor)));
    }
    if let Some(limit) = input.limit {
        if !(1..=100).contains(&limit) {
            return CallToolResult::error("limit must be between 1 and 100".to_string());
        }
        query.push(format!("limit={limit}"));
    }
    let path = format!(
        "/api/v1/workspaces/{}/flow/search{}",
        encode_query_component(&input.workspace_id),
        query_suffix(&query)
    );
    respond_data(get_structured(client, &path).await)
}

pub fn get_flow_projection_lag_tool() -> ToolDefinition {
    ToolDefinition {
        name: "collab.projection_lag".to_string(),
        description: "Read policy-filtered projection lag aggregates and items without content or byte payloads."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "workspace_id": { "type": "string" },
                "project_id": { "type": "string" },
                "cursor": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
            },
            "required": ["workspace_id"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GetFlowProjectionLagInput {
    workspace_id: String,
    project_id: Option<String>,
    cursor: Option<String>,
    limit: Option<u64>,
}

pub async fn get_flow_projection_lag(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: GetFlowProjectionLagInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    let mut query = Vec::new();
    if let Some(project_id) = input.project_id {
        query.push(format!("project_id={}", encode_query_component(&project_id)));
    }
    if let Some(cursor) = input.cursor {
        query.push(format!("cursor={}", encode_query_component(&cursor)));
    }
    if let Some(limit) = input.limit {
        if !(1..=100).contains(&limit) {
            return CallToolResult::error("limit must be between 1 and 100".to_string());
        }
        query.push(format!("limit={limit}"));
    }
    let path = format!(
        "/api/v1/workspaces/{}/flow/projection-lag{}",
        encode_query_component(&input.workspace_id),
        query_suffix(&query)
    );
    respond_data(get_structured(client, &path).await)
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
    use super::{
        get_flow_object, get_flow_object_history, move_flow_object, query_flow_objects, search_flow_objects,
        set_flow_object_grants,
    };
    use crate::client::test_api;
    use axum::{Json, Router, extract::State, routing::get, routing::post, routing::put};
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Mutex;

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
    async fn objects_search_has_no_all_visible_escape_hatch() -> Result<(), String> {
        let client = test_api::client("http://127.0.0.1:1".to_string())?;
        let result = search_flow_objects(
            &client,
            json!({
                "workspace_id": "11111111-1111-4111-8111-111111111111",
                "project_id": "22222222-2222-4222-8222-222222222222",
                "q": "needle",
                "all_visible": true
            }),
        )
        .await;
        assert_eq!(result.is_error, Some(true));
        Ok(())
    }

    #[tokio::test]
    async fn objects_move_maps_target_frontier_only_to_the_new_parent_payload() -> Result<(), Box<dyn std::error::Error>>
    {
        let captured = Arc::new(Mutex::new(None));
        let router = Router::new()
            .route(
                "/api/v1/flow/objects/{object_id}/commands",
                post(
                    |State(captured): State<Arc<Mutex<Option<serde_json::Value>>>>,
                     Json(body): Json<serde_json::Value>| async move {
                        *captured.lock().await = Some(body);
                        Json(json!({ "code": 0, "data": { "event_id": "ok" } }))
                    },
                ),
            )
            .with_state(Arc::clone(&captured));
        let base_url = test_api::spawn(router).await?;
        let client = test_api::client(base_url)?;
        let result = move_flow_object(
            &client,
            json!({
                "object_id": "11111111-1111-4111-8111-111111111111",
                "target_object_id": "22222222-2222-4222-8222-222222222222",
                "after_id": "33333333-3333-4333-8333-333333333333",
                "expected_target_frontier": "dGFyZ2V0",
                "idempotency_key": "move-key",
                "message": "move it"
            }),
        )
        .await;
        assert_ne!(
            result.is_error,
            Some(true),
            "successful API response was not propagated"
        );
        let body = captured
            .lock()
            .await
            .clone()
            .ok_or("the command endpoint was not called")?;
        assert_eq!(body["command"]["type"], "move_object");
        assert_eq!(
            body["command"]["payload"]["target_object_id"],
            "22222222-2222-4222-8222-222222222222"
        );
        assert_eq!(body["command"]["payload"]["expected_target_frontier"], "dGFyZ2V0");
        assert!(body.get("expected_frontier").is_none());
        assert_eq!(body["message"], "move it");
        Ok(())
    }

    #[tokio::test]
    async fn grants_dry_run_is_forwarded_on_the_same_tool_and_preserves_a_no_event_response()
    -> Result<(), Box<dyn std::error::Error>> {
        let captured = Arc::new(Mutex::new(None));
        let router = Router::new()
            .route(
                "/api/v1/flow/objects/{object_id}/grants",
                put(
                    |State(captured): State<Arc<Mutex<Option<serde_json::Value>>>>,
                     Json(body): Json<serde_json::Value>| async move {
                        *captured.lock().await = Some(body);
                        Json(json!({
                            "code": 0,
                            "data": { "applied": false, "permission_changes": { "affected": [] } }
                        }))
                    },
                ),
            )
            .with_state(Arc::clone(&captured));
        let base_url = test_api::spawn(router).await?;
        let client = test_api::client(base_url)?;
        let result = set_flow_object_grants(
            &client,
            json!({
                "object_id": "11111111-1111-4111-8111-111111111111",
                "grants": [],
                "dry_run": true,
                "idempotency_key": "preview-key"
            }),
        )
        .await;
        assert_ne!(result.is_error, Some(true));
        let body = captured
            .lock()
            .await
            .clone()
            .ok_or("the grants endpoint was not called")?;
        assert_eq!(body["dry_run"], true);
        let Some(crate::protocol::ToolContent::Text { text }) = result.content.first() else {
            return Err("missing MCP text content".into());
        };
        let output: serde_json::Value = serde_json::from_str(text)?;
        assert_eq!(output["applied"], false);
        assert!(output.get("event_id").is_none());
        assert!(
            output.get("data").is_none(),
            "MCP must expose REST data, not its envelope"
        );
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
