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

async fn post_package_bytes_structured(
    client: &OpenPrClient,
    path: &str,
    bytes: Vec<u8>,
    idempotency_key: &str,
) -> Result<Value, StructuredApiError> {
    let url = format!("{}{path}", client.base_url);
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name("mcp-flow-package.zip")
        .mime_str("application/vnd.sylvode.flow-package+zip;version=1")
        .map_err(|error| StructuredApiError::transport(format!("Failed to build package upload: {error}")))?;
    send_structured(
        client,
        client
            .client
            .post(&url)
            .header("Idempotency-Key", idempotency_key)
            .multipart(reqwest::multipart::Form::new().part("package", part)),
        path,
    )
    .await
}

async fn put_structured(client: &OpenPrClient, path: &str, body: &Value) -> Result<Value, StructuredApiError> {
    let url = format!("{}{path}", client.base_url);
    send_structured(client, client.client.put(&url).json(body), path).await
}

async fn delete_structured_with_key(
    client: &OpenPrClient,
    path: &str,
    idempotency_key: &str,
) -> Result<Value, StructuredApiError> {
    let url = format!("{}{path}", client.base_url);
    send_structured(
        client,
        client.client.delete(&url).header("Idempotency-Key", idempotency_key),
        path,
    )
    .await
}

fn parse_input<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, CallToolResult> {
    serde_json::from_value(args).map_err(|err| CallToolResult::error(format!("Invalid input: {err}")))
}

fn recoverable_business_error(code: &str) -> bool {
    matches!(
        code,
        "unauthenticated"
            | "stale_frontier"
            | "limit_exceeded"
            | "resync_required"
            | "authorization_churn"
            | "server_draining"
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
    respond(result.and_then(|envelope| {
        envelope
            .get("data")
            .cloned()
            .ok_or_else(|| StructuredApiError::transport("Malformed successful API envelope: missing data".to_string()))
    }))
}

/// Keeps the pre-existing public client helpers part of the compiled client surface while these
/// tools use the richer envelope reader. Downstream code may still call the String-returning
/// helpers directly.
fn retain_client_method<T>(_method: T) {}

pub fn reference_flow_object_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.reference".to_string(),
        description: "Create a permission-aware reference from a Flow object to a Form or Form record.".to_string(),
        input_schema: json!({"type":"object","properties":{
            "object_id":{"type":"string"},"target_type":{"type":"string","enum":["form","form_record"]},
            "target_id":{"type":"string"},"display":{"type":"object"},
            "idempotency_key":{"type":"string","minLength":1,"maxLength":128}},
            "required":["object_id","target_type","target_id","idempotency_key"],"additionalProperties":false}),
    }
}

pub fn unreference_flow_object_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.unreference".to_string(),
        description: "Remove a Flow-to-Forms reference without deleting its target.".to_string(),
        input_schema: json!({"type":"object","properties":{
            "object_id":{"type":"string"},"reference_id":{"type":"string"},
            "idempotency_key":{"type":"string","minLength":1,"maxLength":128}},
            "required":["object_id","reference_id","idempotency_key"],"additionalProperties":false}),
    }
}

pub fn convert_preview_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.convert_preview".to_string(),
        description: "Preview a frozen Flow-to-Forms conversion for 15 minutes.".to_string(),
        input_schema: json!({"type":"object","properties":{
            "source_object_id":{"type":"string"},"source_frontier":{"type":"string"},
            "target_type":{"type":"string","enum":["form","form_record"]},"mapping":{"type":"object"},
            "idempotency_key":{"type":"string","minLength":1,"maxLength":128}},
            "required":["source_object_id","source_frontier","target_type","mapping","idempotency_key"],
            "additionalProperties":false}),
    }
}

pub fn convert_commit_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.convert_commit".to_string(),
        description: "Commit a previously previewed conversion with frozen source and target versions.".to_string(),
        input_schema: json!({"type":"object","properties":{
            "preview_id":{"type":"string"},"source_frontier":{"type":"string"},
            "target_schema_version":{"type":"integer"},"confirm":{"const":true},
            "idempotency_key":{"type":"string","minLength":1,"maxLength":128}},
            "required":["preview_id","source_frontier","target_schema_version","confirm","idempotency_key"],
            "additionalProperties":false}),
    }
}

pub fn convert_status_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.convert_status".to_string(),
        description: "Read a conversion job after reauthorizing its Flow source.".to_string(),
        input_schema: json!({"type":"object","properties":{"job_id":{"type":"string"}},
            "required":["job_id"],"additionalProperties":false}),
    }
}

pub fn convert_retry_tool() -> ToolDefinition {
    ToolDefinition {
        name: "objects.convert_retry".to_string(),
        description: "Idempotently retry a failed conversion with all permissions rechecked.".to_string(),
        input_schema: json!({"type":"object","properties":{"job_id":{"type":"string"},"confirm":{"const":true},
            "idempotency_key":{"type":"string","minLength":1,"maxLength":128}},
            "required":["job_id","confirm","idempotency_key"],"additionalProperties":false}),
    }
}

fn v08_tool(name: &str, description: &str, properties: &Value, required: &[&str]) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: description.to_string(),
        input_schema: json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
    }
}

pub fn export_flow_object_tool() -> ToolDefinition {
    v08_tool(
        "objects.export",
        "Create an authorized Flow object export job.",
        &json!({
            "object_id":{"type":"string"},"format":{"type":"string","enum":["json","markdown","csv","package"]},
            "at_seq":{"type":"integer"},"include_history":{"type":"boolean","default":false},
            "idempotency_key":{"type":"string","minLength":1,"maxLength":128}
        }),
        &["object_id", "format", "idempotency_key"],
    )
}
pub fn export_flow_workspace_tool() -> ToolDefinition {
    v08_tool(
        "objects.export_workspace",
        "Create an all-or-nothing workspace package export.",
        &json!({
            "workspace_id":{"type":"string"},"include_history":{"type":"boolean","default":false},
            "project_id":{"type":"string"},"idempotency_key":{"type":"string","minLength":1,"maxLength":128}
        }),
        &["workspace_id", "idempotency_key"],
    )
}
pub fn import_flow_artifact_tool() -> ToolDefinition {
    let mut tool = v08_tool(
        "objects.import_artifact",
        "Stage one bounded package artifact; caller URLs and filesystem paths are forbidden.",
        &json!({
            "workspace_id":{"type":"string"},
            "staged_object":{"type":"object","properties":{"object_key":{"type":"string"},"package_sha256":{"type":"string"},"size":{"type":"integer","minimum":0}},"required":["object_key","package_sha256","size"],"additionalProperties":false},
            "package_base64":{"type":"string"},"package_sha256":{"type":"string"},
            "idempotency_key":{"type":"string","minLength":1,"maxLength":128}
        }),
        &["workspace_id", "idempotency_key"],
    );
    if let Some(schema) = tool.input_schema.as_object_mut() {
        schema.insert(
            "oneOf".to_string(),
            json!([
                {"required":["package_base64"],"not":{"required":["staged_object"]}},
                {"required":["staged_object"],"not":{"required":["package_base64"]}}
            ]),
        );
    }
    tool
}
pub fn import_flow_preview_tool() -> ToolDefinition {
    v08_tool(
        "objects.import_preview",
        "Preview a frozen package import without canonical writes.",
        &json!({
            "workspace_id":{"type":"string"},"artifact_id":{"type":"string"},"project_mapping":{"type":"object"},
            "external_reference_policy":{"type":"string","enum":["reject","detach"]},
            "conflict_policy":{"type":"string","enum":["reject_existing","reuse_import_lineage"]},
            "include_history":{"type":"boolean","default":false},"idempotency_key":{"type":"string","minLength":1,"maxLength":128}
        }),
        &[
            "workspace_id",
            "artifact_id",
            "external_reference_policy",
            "conflict_policy",
            "idempotency_key",
        ],
    )
}
pub fn import_flow_commit_tool() -> ToolDefinition {
    v08_tool(
        "objects.import_commit",
        "Atomically commit a frozen package import.",
        &json!({
            "workspace_id":{"type":"string"},"import_id":{"type":"string"},"package_sha256":{"type":"string"},
            "mapping_hash":{"type":"string"},"conflict_policy":{"type":"string","enum":["reject_existing","reuse_import_lineage"]},
            "confirm":{"const":true},"idempotency_key":{"type":"string","minLength":1,"maxLength":128}
        }),
        &[
            "workspace_id",
            "import_id",
            "package_sha256",
            "mapping_hash",
            "conflict_policy",
            "confirm",
            "idempotency_key",
        ],
    )
}
pub fn import_flow_status_tool() -> ToolDefinition {
    v08_tool(
        "objects.import_status",
        "Read a redacted import report.",
        &json!({
            "workspace_id":{"type":"string"},"import_id":{"type":"string"}
        }),
        &["workspace_id", "import_id"],
    )
}
pub fn flow_collab_status_tool() -> ToolDefinition {
    v08_tool(
        "collab.status",
        "Read workspace Flow health and lag without content bytes.",
        &json!({
            "workspace_id":{"type":"string"}
        }),
        &["workspace_id"],
    )
}
pub fn flow_integrity_tool() -> ToolDefinition {
    let mut tool = v08_tool(
        "objects.integrity",
        "Read workspace integrity or shallow-verify one object.",
        &json!({
            "workspace_id":{"type":"string"},"object_id":{"type":"string"},"deep":{"const":false},
            "cursor":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":100}
        }),
        &[],
    );
    if let Some(schema) = tool.input_schema.as_object_mut() {
        schema.insert(
            "oneOf".to_string(),
            json!([
                {"required":["workspace_id"],"not":{"required":["object_id"]}},
                {"required":["object_id"],"not":{"required":["workspace_id"]}}
            ]),
        );
    }
    tool
}
pub fn compact_flow_document_tool() -> ToolDefinition {
    v08_tool(
        "collab.compact",
        "Dry-run or execute exact-scope document compaction.",
        &json!({
            "object_id":{"type":"string"},"document_id":{"type":"string"},"dry_run":{"type":"boolean"},
            "expected_head_seq":{"type":"integer"},"retain_after_seq":{"type":"integer"},"confirm_document_id":{"type":"string"},
            "idempotency_key":{"type":"string","minLength":1,"maxLength":128}
        }),
        &[
            "object_id",
            "document_id",
            "dry_run",
            "expected_head_seq",
            "idempotency_key",
        ],
    )
}
pub fn replay_flow_deliveries_tool() -> ToolDefinition {
    v08_tool(
        "deliveries.replay",
        "Replay or requeue deliveries in one explicit retained window.",
        &json!({
            "workspace_id":{"type":"string"},"mode":{"type":"string","enum":["rebuild","requeue_failed"]},
            "event_type":{"type":"string"},"subscriber_kind":{"type":"string"},"subscriber_id":{"type":"string"},
            "from":{"type":"string"},"to":{"type":"string"},"dry_run":{"type":"boolean"},"confirm":{"const":true},
            "idempotency_key":{"type":"string","minLength":1,"maxLength":128}
        }),
        &[
            "workspace_id",
            "mode",
            "from",
            "to",
            "dry_run",
            "confirm",
            "idempotency_key",
        ],
    )
}
pub fn rebuild_flow_projection_tool() -> ToolDefinition {
    v08_tool(
        "collab.rebuild_projection",
        "Dry-run or execute an exact-object projection rebuild.",
        &json!({
            "object_id":{"type":"string"},"dry_run":{"type":"boolean"},"expected_head_seq":{"type":"integer"},
            "confirm_object_id":{"type":"string"},"idempotency_key":{"type":"string","minLength":1,"maxLength":128}
        }),
        &["object_id", "dry_run", "expected_head_seq", "idempotency_key"],
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReferenceInput {
    object_id: String,
    target_type: String,
    target_id: String,
    display: Option<Value>,
    idempotency_key: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnreferenceInput {
    object_id: String,
    reference_id: String,
    idempotency_key: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ConvertPreviewInput {
    source_object_id: String,
    source_frontier: String,
    target_type: String,
    mapping: Value,
    idempotency_key: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ConvertCommitInput {
    preview_id: String,
    source_frontier: String,
    target_schema_version: i32,
    confirm: bool,
    idempotency_key: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConvertStatusInput {
    job_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ConvertRetryInput {
    job_id: String,
    confirm: bool,
    idempotency_key: String,
}

pub async fn reference_flow_object(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ReferenceInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    if required_write_key(&input.idempotency_key).is_err() || uuid::Uuid::parse_str(&input.target_id).is_err() {
        return CallToolResult::error("target_id must be a UUID and idempotency_key must be non-empty".to_string());
    }
    let path = format!(
        "/api/v1/flow/objects/{}/references",
        encode_query_component(&input.object_id)
    );
    respond_data(
        post_structured(
            client,
            &path,
            &json!({"target_type":input.target_type,"target_id":input.target_id,
        "display":input.display.unwrap_or_else(|| json!({})),"idempotency_key":input.idempotency_key}),
        )
        .await,
    )
}

pub async fn unreference_flow_object(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: UnreferenceInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    if let Err(result) = required_write_key(&input.idempotency_key) {
        return result;
    }
    let path = format!(
        "/api/v1/flow/objects/{}/references/{}",
        encode_query_component(&input.object_id),
        encode_query_component(&input.reference_id)
    );
    respond_data(delete_structured_with_key(client, &path, &input.idempotency_key).await)
}

pub async fn convert_preview(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ConvertPreviewInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    if let Err(result) = required_write_key(&input.idempotency_key) {
        return result;
    }
    respond_data(
        post_structured(
            client,
            "/api/v1/flow/conversions/preview",
            &serde_json::to_value(input).unwrap_or_default(),
        )
        .await,
    )
}

pub async fn convert_commit(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ConvertCommitInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    if !input.confirm {
        return CallToolResult::error("confirm must be true".to_string());
    }
    if let Err(result) = required_write_key(&input.idempotency_key) {
        return result;
    }
    respond_data(
        post_structured(
            client,
            "/api/v1/flow/conversions",
            &serde_json::to_value(input).unwrap_or_default(),
        )
        .await,
    )
}

pub async fn convert_status(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ConvertStatusInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    respond_data(
        get_structured(
            client,
            &format!("/api/v1/flow/conversions/{}", encode_query_component(&input.job_id)),
        )
        .await,
    )
}

pub async fn convert_retry(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ConvertRetryInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    if !input.confirm {
        return CallToolResult::error("confirm must be true".to_string());
    }
    if let Err(result) = required_write_key(&input.idempotency_key) {
        return result;
    }
    let path = format!(
        "/api/v1/flow/conversions/{}/retry",
        encode_query_component(&input.job_id)
    );
    respond_data(
        post_structured(
            client,
            &path,
            &json!({"confirm":true,"idempotency_key":input.idempotency_key}),
        )
        .await,
    )
}

fn required_string<'a>(args: &'a Value, key: &str) -> Result<&'a str, CallToolResult> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CallToolResult::error(format!("{key} must be a non-empty string")))
}

fn without_fields(args: &Value, fields: &[&str]) -> Value {
    let mut body = args.clone();
    if let Some(object) = body.as_object_mut() {
        for field in fields {
            object.remove(*field);
        }
    }
    body
}

pub async fn export_flow_object(client: &OpenPrClient, args: Value) -> CallToolResult {
    let object_id = match required_string(&args, "object_id") {
        Ok(value) => value,
        Err(result) => return result,
    };
    let body = without_fields(&args, &["object_id"]);
    respond_data(
        post_structured(
            client,
            &format!("/api/v1/flow/objects/{}/exports", encode_query_component(object_id)),
            &body,
        )
        .await,
    )
}

pub async fn export_flow_workspace(client: &OpenPrClient, args: Value) -> CallToolResult {
    let workspace_id = match required_string(&args, "workspace_id") {
        Ok(value) => value,
        Err(result) => return result,
    };
    let mut body = without_fields(&args, &["workspace_id"]);
    body.as_object_mut()
        .map(|object| object.insert("format".to_string(), json!("package")));
    respond_data(
        post_structured(
            client,
            &format!(
                "/api/v1/workspaces/{}/flow/exports",
                encode_query_component(workspace_id)
            ),
            &body,
        )
        .await,
    )
}

pub async fn import_flow_artifact(client: &OpenPrClient, args: Value) -> CallToolResult {
    let workspace_id = match required_string(&args, "workspace_id") {
        Ok(value) => value,
        Err(result) => return result,
    };
    let inline = args.get("package_base64").and_then(Value::as_str);
    let staged_source = args.get("staged_object").filter(|value| !value.is_null());
    if inline.is_some() == staged_source.is_some() {
        return CallToolResult::error("exactly one of package_base64 or staged_object is required".to_string());
    }
    let path = format!(
        "/api/v1/workspaces/{}/flow/import-artifacts",
        encode_query_component(workspace_id)
    );
    if let Some(package_base64) = inline {
        let idempotency_key = match required_string(&args, "idempotency_key") {
            Ok(value) => value,
            Err(result) => return result,
        };
        let mut archive_stager = api::flow::import::BoundedImportStager::new(Vec::new(), std::io::sink());
        if let Err(error) = archive_stager.stage_inline_base64_archive(std::io::Cursor::new(package_base64.as_bytes()))
        {
            return CallToolResult::error(format!("invalid bounded package_base64: {error}"));
        }
        if let Err(error) = archive_stager.finish_archive() {
            return CallToolResult::error(format!("invalid bounded package_base64: {error}"));
        }
        let (bytes, _) = match archive_stager.into_stages() {
            Ok(value) => value,
            Err(error) => return CallToolResult::error(format!("invalid bounded package_base64: {error}")),
        };
        let result = post_package_bytes_structured(client, &path, bytes, idempotency_key).await;
        return respond_data(result.and_then(|envelope| {
            if let Some(expected) = args.get("package_sha256").and_then(Value::as_str)
                && envelope.pointer("/data/package_sha256").and_then(Value::as_str) != Some(expected)
            {
                return Err(StructuredApiError::transport(
                    "uploaded package hash does not match package_sha256".to_string(),
                ));
            }
            Ok(envelope)
        }));
    }
    let source = {
        let mut value = staged_source.cloned().unwrap_or(Value::Null);
        value
            .as_object_mut()
            .map(|object| object.insert("kind".to_string(), json!("staged_object")));
        value
    };
    let body = json!({"source":source,"idempotency_key":args.get("idempotency_key")});
    respond_data(post_structured(client, &path, &body).await)
}

pub async fn import_flow_preview(client: &OpenPrClient, args: Value) -> CallToolResult {
    let workspace_id = match required_string(&args, "workspace_id") {
        Ok(value) => value,
        Err(result) => return result,
    };
    let body = without_fields(&args, &["workspace_id"]);
    respond_data(
        post_structured(
            client,
            &format!(
                "/api/v1/workspaces/{}/flow/imports/preview",
                encode_query_component(workspace_id)
            ),
            &body,
        )
        .await,
    )
}

pub async fn import_flow_commit(client: &OpenPrClient, args: Value) -> CallToolResult {
    let workspace_id = match required_string(&args, "workspace_id") {
        Ok(value) => value,
        Err(result) => return result,
    };
    let import_id = match required_string(&args, "import_id") {
        Ok(value) => value,
        Err(result) => return result,
    };
    let body = without_fields(&args, &["workspace_id", "import_id"]);
    respond_data(
        post_structured(
            client,
            &format!(
                "/api/v1/workspaces/{}/flow/imports/{}/commit",
                encode_query_component(workspace_id),
                encode_query_component(import_id)
            ),
            &body,
        )
        .await,
    )
}

pub async fn import_flow_status(client: &OpenPrClient, args: Value) -> CallToolResult {
    let workspace_id = match required_string(&args, "workspace_id") {
        Ok(value) => value,
        Err(result) => return result,
    };
    let import_id = match required_string(&args, "import_id") {
        Ok(value) => value,
        Err(result) => return result,
    };
    respond_data(
        get_structured(
            client,
            &format!(
                "/api/v1/workspaces/{}/flow/imports/{}",
                encode_query_component(workspace_id),
                encode_query_component(import_id)
            ),
        )
        .await,
    )
}

pub async fn flow_collab_status(client: &OpenPrClient, args: Value) -> CallToolResult {
    let workspace_id = match required_string(&args, "workspace_id") {
        Ok(value) => value,
        Err(result) => return result,
    };
    let health = get_structured(
        client,
        &format!(
            "/api/v1/admin/workspaces/{}/flow/health",
            encode_query_component(workspace_id)
        ),
    )
    .await;
    let lag = get_structured(
        client,
        &format!(
            "/api/v1/admin/workspaces/{}/flow/lag",
            encode_query_component(workspace_id)
        ),
    )
    .await;
    respond_data(match (health, lag) {
        (Ok(health), Ok(lag)) => Ok(
            json!({"data":{"health":health.get("data").cloned().unwrap_or(Value::Null),"lag":lag.get("data").cloned().unwrap_or(Value::Null)}}),
        ),
        (Err(error), _) | (_, Err(error)) => Err(error),
    })
}

pub async fn flow_integrity(client: &OpenPrClient, args: Value) -> CallToolResult {
    match (
        args.get("workspace_id").and_then(Value::as_str),
        args.get("object_id").and_then(Value::as_str),
    ) {
        (Some(workspace_id), None) => {
            let mut query = vec!["scope=summary".to_string()];
            if let Some(cursor) = args.get("cursor").and_then(Value::as_str) {
                query.push(format!("cursor={}", encode_query_component(cursor)));
            }
            if let Some(limit) = args.get("limit").and_then(Value::as_u64) {
                query.push(format!("limit={limit}"));
            }
            respond_data(
                get_structured(
                    client,
                    &format!(
                        "/api/v1/admin/workspaces/{}/flow/integrity?{}",
                        encode_query_component(workspace_id),
                        query.join("&")
                    ),
                )
                .await,
            )
        }
        (None, Some(object_id)) => respond_data(
            post_structured(
                client,
                &format!(
                    "/api/v1/flow/objects/{}/collab/verify",
                    encode_query_component(object_id)
                ),
                &json!({"deep":false,"idempotency_key":uuid::Uuid::new_v4().to_string()}),
            )
            .await,
        ),
        _ => CallToolResult::error("exactly one of workspace_id or object_id is required".to_string()),
    }
}

pub async fn compact_flow_document(client: &OpenPrClient, args: Value) -> CallToolResult {
    let object_id = match required_string(&args, "object_id") {
        Ok(value) => value,
        Err(result) => return result,
    };
    let document_id = match required_string(&args, "document_id") {
        Ok(value) => value,
        Err(result) => return result,
    };
    let scope = get_structured(
        client,
        &format!("/api/v1/flow/objects/{}/collab", encode_query_component(object_id)),
    )
    .await;
    match scope {
        Ok(envelope) if envelope.pointer("/data/document_id").and_then(Value::as_str) == Some(document_id) => {}
        Ok(_) => return CallToolResult::error("object_id does not own document_id".to_string()),
        Err(error) => return respond(Err(error)),
    }
    let body = without_fields(&args, &["object_id", "document_id"]);
    respond_data(
        post_structured(
            client,
            &format!(
                "/api/v1/admin/flow/documents/{}/compact",
                encode_query_component(document_id)
            ),
            &body,
        )
        .await,
    )
}

pub async fn replay_flow_deliveries(client: &OpenPrClient, args: Value) -> CallToolResult {
    let workspace_id = match required_string(&args, "workspace_id") {
        Ok(value) => value,
        Err(result) => return result,
    };
    let body = without_fields(&args, &["workspace_id"]);
    respond_data(
        post_structured(
            client,
            &format!(
                "/api/v1/admin/workspaces/{}/flow/deliveries/replay",
                encode_query_component(workspace_id)
            ),
            &body,
        )
        .await,
    )
}

pub async fn rebuild_flow_projection(client: &OpenPrClient, args: Value) -> CallToolResult {
    let object_id = match required_string(&args, "object_id") {
        Ok(value) => value,
        Err(result) => return result,
    };
    let body = without_fields(&args, &["object_id"]);
    respond_data(
        post_structured(
            client,
            &format!(
                "/api/v1/admin/flow/objects/{}/rebuild-projection",
                encode_query_component(object_id)
            ),
            &body,
        )
        .await,
    )
}

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
    respond_data(get_structured(client, &path).await)
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
    respond_data(get_structured(client, &path).await)
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
    respond_data(get_structured(client, &path).await)
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
        description: "Create a page, navigator, or Collection Flow object. A Collection with embed_page_id uses the server's atomic Page embed command; Record creation is collection-scoped.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "workspace_id": { "type": "string" },
                "project_id": { "type": "string" },
                "type": { "type": "string", "enum": ["page", "navigator", "collection"] },
                "parent_id": { "type": "string" },
                "embed_page_id": { "type": "string", "description": "For type=collection, atomically create and embed in this Page" },
                "initial_fields": {"type": "array", "items": {"type": "object"}},
                "initial_view": {"type": "object"},
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
    embed_page_id: Option<String>,
    #[serde(default)]
    initial_fields: Vec<Value>,
    initial_view: Option<Value>,
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
    if !matches!(input.object_type.as_str(), "page" | "navigator" | "collection") {
        return CallToolResult::error(
            "type must be page, navigator, or collection; records use records.create".to_string(),
        );
    }
    if input.embed_page_id.is_some() && input.object_type != "collection" {
        return CallToolResult::error("embed_page_id is only valid for type=collection".to_string());
    }
    if input.embed_page_id.is_some() && input.parent_id.is_some() {
        return CallToolResult::error("embed_page_id and parent_id are mutually exclusive".to_string());
    }
    if let Some(page_id) = input.embed_page_id.as_deref() {
        let body = command_body(
            "create_collection_embed",
            &json!({
                "title": input.title,
                "initial_fields": input.initial_fields,
                "initial_view": input.initial_view,
            }),
            &input.idempotency_key,
            input.message.as_deref(),
        );
        let path = format!("/api/v1/flow/objects/{}/commands", encode_query_component(page_id));
        return respond_data(post_structured(client, &path, &body).await);
    }
    let mut body = json!({
        "object_type": input.object_type,
        "title": input.title,
        "idempotency_key": input.idempotency_key,
        "initial_fields": input.initial_fields,
        "initial_view": input.initial_view,
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

// ---- Flow v0.6 Collection tools ----

pub fn describe_collection_tool() -> ToolDefinition {
    ToolDefinition {
        name: "collections.describe".to_string(),
        description: "Describe a Collection with field IDs and labels, views, record count, schema seq, and projection seq. Reads typed projections only.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "collection_id": {"type": "string"},
                "at_seq": {"type": "integer"}
            },
            "required": ["collection_id"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DescribeCollectionInput {
    collection_id: String,
    at_seq: Option<i64>,
}

pub async fn describe_collection(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: DescribeCollectionInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    let suffix = input.at_seq.map_or_else(String::new, |seq| format!("?at_seq={seq}"));
    let path = format!(
        "/api/v1/flow/collections/{}{suffix}",
        encode_query_component(&input.collection_id)
    );
    respond_data(get_structured(client, &path).await)
}

pub fn query_collection_tool() -> ToolDefinition {
    ToolDefinition {
        name: "collections.query".to_string(),
        description: "Query typed Collection record projections with opaque cursor pagination, field-ID filter/sort/group, and field ID plus label metadata.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "collection_id": {"type": "string"},
                "filter": {
                    "type": "object",
                    "properties": {
                        "field_id": {"type": "string"},
                        "op": {"type": "string", "enum": ["eq", "ne", "lt", "lte", "gt", "gte", "contains"]},
                        "value": {}
                    },
                    "required": ["field_id", "op", "value"]
                },
                "sort": {
                    "type": "object",
                    "properties": {
                        "field_id": {"type": "string"},
                        "direction": {"type": "string", "enum": ["asc", "desc"]}
                    },
                    "required": ["field_id"]
                },
                "group": {"type": "string", "description": "Field UUID"},
                "field_ids": {"type": "array", "items": {"type": "string"}},
                "cursor": {"type": "string"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 50}
            },
            "required": ["collection_id"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct QueryCollectionInput {
    collection_id: String,
    filter: Option<Value>,
    sort: Option<Value>,
    group: Option<String>,
    field_ids: Option<Vec<String>>,
    cursor: Option<String>,
    limit: Option<u64>,
}

pub async fn query_collection(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: QueryCollectionInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    if input.limit.is_some_and(|limit| !(1..=100).contains(&limit)) {
        return CallToolResult::error("limit must be between 1 and 100".to_string());
    }
    let path = format!(
        "/api/v1/flow/collections/{}/query",
        encode_query_component(&input.collection_id)
    );
    let mut body = serde_json::to_value(&input).unwrap_or_else(|_| json!({}));
    if let Some(object) = body.as_object_mut() {
        object.remove("collection_id");
    }
    respond_data(post_structured(client, &path, &body).await)
}

pub fn create_collection_record_tool() -> ToolDefinition {
    ToolDefinition {
        name: "records.create".to_string(),
        description: "Create one Record inside a Collection using values keyed only by stable field UUID. The response and follow-up query include field IDs and labels.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "collection_id": {"type": "string"},
                "values_by_field_id": {"type": "object"},
                "body": {"type": "string"},
                "idempotency_key": {"type": "string", "minLength": 1, "maxLength": 128},
                "message": {"type": "string"}
            },
            "required": ["collection_id", "values_by_field_id", "idempotency_key"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateCollectionRecordInput {
    collection_id: String,
    values_by_field_id: Value,
    body: Option<String>,
    idempotency_key: String,
    message: Option<String>,
}

pub async fn create_collection_record(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: CreateCollectionRecordInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    if !input.values_by_field_id.is_object() {
        return CallToolResult::error("values_by_field_id must be an object keyed by field UUID".to_string());
    }
    if let Err(result) = required_write_key(&input.idempotency_key) {
        return result;
    }
    let path = format!(
        "/api/v1/flow/collections/{}/records",
        encode_query_component(&input.collection_id)
    );
    let body = json!({
        "values_by_field_id": input.values_by_field_id,
        "body": input.body,
        "idempotency_key": input.idempotency_key,
        "message": input.message,
    });
    respond_data(post_structured(client, &path, &body).await)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::{
        diff_flow_object, get_flow_object, get_flow_object_history, move_flow_object, query_flow_objects,
        search_flow_objects, set_flow_object_grants,
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
    async fn every_v04_object_read_returns_rest_data_like_the_v05_tools() -> Result<(), Box<dyn std::error::Error>> {
        let response = || async { Json(json!({"code": 0, "message": "ok", "data": {"shape": "semantic-data"}})) };
        let router = Router::new()
            .route("/api/v1/flow/objects/{object_id}", get(response))
            .route("/api/v1/flow/objects/{object_id}/history", get(response))
            .route("/api/v1/flow/objects/{object_id}/diff", get(response))
            .route("/api/v1/workspaces/{workspace_id}/flow/objects", get(response));
        let base_url = test_api::spawn(router).await?;
        let client = test_api::client(base_url)?;
        let calls = [
            get_flow_object(&client, json!({"object_id": "object"})).await,
            query_flow_objects(&client, json!({"workspace_id": "workspace"})).await,
            get_flow_object_history(&client, json!({"object_id": "object"})).await,
            diff_flow_object(&client, json!({"object_id": "object", "from_seq": 0, "to_seq": 1})).await,
        ];
        for result in calls {
            assert_ne!(result.is_error, Some(true), "Flow read failed: {result:?}");
            let Some(crate::protocol::ToolContent::Text { text }) = result.content.first() else {
                return Err("missing MCP text content".into());
            };
            let output: serde_json::Value = serde_json::from_str(text)?;
            assert_eq!(output, json!({"shape": "semantic-data"}));
            assert!(output.get("code").is_none(), "REST envelope leaked into MCP: {output}");
            assert!(
                output.get("data").is_none(),
                "nested REST data wrapper leaked into MCP: {output}"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn object_tools_fail_when_a_success_envelope_omits_data() -> Result<(), Box<dyn std::error::Error>> {
        let router = Router::new().route(
            "/api/v1/flow/objects/{object_id}",
            get(|| async { Json(json!({"code": 0, "message": "ok"})) }),
        );
        let base_url = test_api::spawn(router).await?;
        let client = test_api::client(base_url)?;
        let result = get_flow_object(&client, json!({"object_id": "object"})).await;
        assert_eq!(result.is_error, Some(true), "missing data was reported as success");
        let Some(crate::protocol::ToolContent::Text { text }) = result.content.first() else {
            return Err("missing MCP error text".into());
        };
        assert!(text.contains("missing data"), "{text}");
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
    async fn flow_authorization_churn_preserves_retry_hint_as_a_recoverable_mcp_error()
    -> Result<(), Box<dyn std::error::Error>> {
        let router = Router::new().route(
            "/api/v1/flow/objects/{object_id}",
            get(|| async {
                Json(json!({
                    "code": 409,
                    "message": "authorization changed repeatedly",
                    "data": null,
                    "error_code": "authorization_churn",
                    "details": {"retry_after_ms": 200}
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
        assert_eq!(body["error"]["code"], "authorization_churn");
        assert_eq!(body["error"]["recoverable"], true);
        assert_eq!(body["error"]["details"]["retry_after_ms"], 200);
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
