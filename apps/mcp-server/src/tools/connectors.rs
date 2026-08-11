use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use crate::tools::capabilities;
use serde::Deserialize;
use serde_json::{Value, json};

pub fn list_connectors_tool() -> ToolDefinition {
    ToolDefinition {
        name: "connectors.list".to_string(),
        description: "List workspace/project automation connectors such as webhook, MCP, REST, CLI, OpenPRX tunnel, print, or device"
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "Optional project UUID to filter project-scoped connectors"
                },
                "kind": {
                    "type": "string",
                    "enum": ["webhook", "mcp", "rest", "cli", "openprx_tunnel", "print", "device"],
                    "description": "Optional connector kind filter"
                }
            }
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ListConnectorsInput {
    project_id: Option<String>,
    kind: Option<String>,
}

pub async fn list_connectors(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ListConnectorsInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    match client
        .list_connectors(input.project_id.as_deref(), input.kind.as_deref())
        .await
    {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

pub fn get_connector_tool() -> ToolDefinition {
    ToolDefinition {
        name: "connectors.get".to_string(),
        description: "Get an automation connector by UUID".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "connector_id": {
                    "type": "string",
                    "description": "Connector UUID"
                }
            },
            "required": ["connector_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct GetConnectorInput {
    connector_id: String,
}

pub async fn get_connector(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: GetConnectorInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    match client.get_connector(&input.connector_id).await {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

pub fn list_invocations_tool() -> ToolDefinition {
    ToolDefinition {
        name: "invocations.list".to_string(),
        description: "List AI execution invocation ledger entries for a project, newest first. Paginated: \
per_page defaults to 20 (max 100), so pass page/per_page explicitly when auditing more than the latest entries."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "Project UUID"
                },
                "status": {
                    "type": "string",
                    "description": "Optional invocation status filter"
                },
                "page": { "type": "integer", "minimum": 1, "description": "1-based page number (default 1)" },
                "per_page": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Page size, default 20, clamped to 100" }
            },
            "required": ["project_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ListInvocationsInput {
    project_id: String,
    status: Option<String>,
    page: Option<u32>,
    per_page: Option<u32>,
}

pub async fn list_invocations(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ListInvocationsInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    match client
        .list_invocations(&input.project_id, input.status.as_deref(), input.page, input.per_page)
        .await
    {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

pub fn get_invocation_tool() -> ToolDefinition {
    ToolDefinition {
        name: "invocations.get".to_string(),
        description: "Get one AI execution invocation ledger entry by UUID".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "invocation_id": {
                    "type": "string",
                    "description": "Invocation UUID"
                }
            },
            "required": ["invocation_id"]
        }),
    }
}

pub fn create_invocation_tool() -> ToolDefinition {
    ToolDefinition {
        name: "invocations.create".to_string(),
        description: "Create an auditable MCP/AI invocation ledger entry for a project".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string" },
                "target_agent_id": { "type": "string" },
                "trigger_kind": {
                    "type": "string",
                    "description": "mention, assigned, workflow, proposal_vote, mcp, schedule, or manual"
                },
                "trigger_ref_type": { "type": "string" },
                "trigger_ref_id": { "type": "string" },
                "connector_id": { "type": "string" },
                "payload": { "type": "object" }
            },
            "required": ["project_id", "trigger_kind"]
        }),
    }
}

pub fn report_invocation_progress_tool() -> ToolDefinition {
    ToolDefinition {
        name: "invocations.report_progress".to_string(),
        description: "Report progress for an MCP/AI invocation and mark it running".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "invocation_id": { "type": "string" },
                "payload": { "type": "object" }
            },
            "required": ["invocation_id"]
        }),
    }
}

pub fn complete_invocation_tool() -> ToolDefinition {
    ToolDefinition {
        name: "invocations.complete".to_string(),
        description: "Complete an MCP/AI invocation with a result payload".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "invocation_id": { "type": "string" },
                "result": { "type": "object" }
            },
            "required": ["invocation_id"]
        }),
    }
}

pub fn fail_invocation_tool() -> ToolDefinition {
    ToolDefinition {
        name: "invocations.fail".to_string(),
        description: "Fail an MCP/AI invocation with an error message and optional result payload".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "invocation_id": { "type": "string" },
                "error_message": { "type": "string" },
                "result": { "type": "object" }
            },
            "required": ["invocation_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct GetInvocationInput {
    invocation_id: String,
}

#[derive(Debug, Deserialize)]
struct CreateInvocationInput {
    project_id: String,
    target_agent_id: Option<String>,
    trigger_kind: String,
    trigger_ref_type: Option<String>,
    trigger_ref_id: Option<String>,
    connector_id: Option<String>,
    payload: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct InvocationProgressInput {
    invocation_id: String,
    payload: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct CompleteInvocationInput {
    invocation_id: String,
    result: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct FailInvocationInput {
    invocation_id: String,
    error_message: Option<String>,
    result: Option<Value>,
}

pub async fn get_invocation(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: GetInvocationInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    match client.get_invocation(&input.invocation_id).await {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn create_invocation(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: CreateInvocationInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    let body = json!({
        "target_agent_id": input.target_agent_id,
        "trigger_kind": input.trigger_kind,
        "trigger_ref_type": input.trigger_ref_type,
        "trigger_ref_id": input.trigger_ref_id,
        "connector_id": input.connector_id,
        "payload": input.payload.unwrap_or_else(|| json!({}))
    });

    match client.create_invocation(&input.project_id, body).await {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn report_invocation_progress(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: InvocationProgressInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    match client
        .report_invocation_progress(
            &input.invocation_id,
            json!({ "payload": input.payload.unwrap_or_else(|| json!({})) }),
        )
        .await
    {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn complete_invocation(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: CompleteInvocationInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    match client
        .complete_invocation(
            &input.invocation_id,
            json!({ "result": input.result.unwrap_or_else(|| json!({})) }),
        )
        .await
    {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn fail_invocation(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: FailInvocationInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    match client
        .fail_invocation(
            &input.invocation_id,
            json!({
                "error_message": input.error_message,
                "result": input.result
            }),
        )
        .await
    {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn invoke_connector_tool(client: &OpenPrClient, tool_name: &str, args: Value) -> CallToolResult {
    let Some(project_id) = args.get("project_id").and_then(Value::as_str).map(str::trim) else {
        return CallToolResult::error("Invalid input: project_id is required");
    };
    if project_id.is_empty() {
        return CallToolResult::error("Invalid input: project_id is required");
    }

    let policy = match client.get_project_agent_policy(project_id).await {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Failed to load project agent policy: {err}")),
    };
    let Some(tool) = capabilities::find_connector_tool(&policy, tool_name) else {
        return CallToolResult::error(format!("Unknown connector tool for project: {tool_name}"));
    };
    let Some(connector_id) = tool.get("connector_id").and_then(Value::as_str) else {
        return CallToolResult::error(format!("Connector tool is missing connector_id: {tool_name}"));
    };

    let payload = args
        .get("payload")
        .cloned()
        .unwrap_or_else(|| payload_without_project_id(&args));
    let body = json!({
        "trigger_kind": "mcp",
        "trigger_ref_type": "connector_tool",
        "connector_id": connector_id,
        "payload": {
            "tool_name": tool_name,
            "arguments": payload,
            "connector_tool": tool
        }
    });

    match client.create_invocation(project_id, body).await {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

fn payload_without_project_id(args: &Value) -> Value {
    let Some(object) = args.as_object() else {
        return json!({});
    };
    let mut payload = object.clone();
    payload.remove("project_id");
    Value::Object(payload)
}
