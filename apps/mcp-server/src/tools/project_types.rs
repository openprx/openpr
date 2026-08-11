use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

pub fn list_project_types_tool() -> ToolDefinition {
    ToolDefinition {
        name: "project_types.list".to_string(),
        description: "List project scenario types available to the bot workspace".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {}
        }),
    }
}

pub async fn list_project_types(client: &OpenPrClient, _args: Value) -> CallToolResult {
    match client.list_project_types().await {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

pub fn get_project_type_tool() -> ToolDefinition {
    ToolDefinition {
        name: "project_types.get".to_string(),
        description: "Get a project scenario type and its capability schema".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "Project type key, for example code_project or contract_review"
                }
            },
            "required": ["key"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct GetProjectTypeInput {
    key: String,
}

pub async fn get_project_type(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: GetProjectTypeInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    match client.get_project_type(&input.key).await {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

pub fn list_project_resources_tool() -> ToolDefinition {
    ToolDefinition {
        name: "project_resources.list".to_string(),
        description: "List project resources such as repositories, directories, document libraries, equipment, or business records".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "UUID of the project",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                }
            },
            "required": ["project_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ProjectResourceProjectInput {
    project_id: String,
}

pub async fn list_project_resources(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ProjectResourceProjectInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    match client.list_project_resources(&input.project_id).await {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

pub fn create_project_resource_tool() -> ToolDefinition {
    ToolDefinition {
        name: "project_resources.create".to_string(),
        description: "Create a typed project resource for AI/project context".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "UUID of the project"
                },
                "kind": {
                    "type": "string",
                    "description": "repo, directory, document_library, crm_account, erp_order, equipment, site, or custom"
                },
                "name": {
                    "type": "string",
                    "description": "Human-readable resource name"
                },
                "locator": {
                    "type": "object",
                    "description": "Structured locator, for example {\"url\":\"...\"} or {\"path\":\"...\"}"
                },
                "permission_policy": {
                    "type": "object",
                    "description": "Optional access policy metadata"
                },
                "sync_status": {
                    "type": "string",
                    "description": "manual, synced, stale, or custom status text"
                }
            },
            "required": ["project_id", "kind", "name"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct CreateProjectResourceInput {
    project_id: String,
    kind: String,
    name: String,
    locator: Option<Value>,
    permission_policy: Option<Value>,
    sync_status: Option<String>,
}

pub async fn create_project_resource(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: CreateProjectResourceInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    let body = json!({
        "kind": input.kind,
        "name": input.name,
        "locator": input.locator,
        "permission_policy": input.permission_policy,
        "sync_status": input.sync_status
    });

    match client.create_project_resource(&input.project_id, body).await {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

pub fn update_project_resource_tool() -> ToolDefinition {
    ToolDefinition {
        name: "project_resources.update".to_string(),
        description: "Update a project resource used as AI/project context".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string" },
                "resource_id": { "type": "string" },
                "kind": { "type": "string" },
                "name": { "type": "string" },
                "locator": { "type": "object" },
                "permission_policy": { "type": "object" },
                "sync_status": { "type": "string" }
            },
            "required": ["project_id", "resource_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct UpdateProjectResourceInput {
    project_id: String,
    resource_id: String,
    kind: Option<String>,
    name: Option<String>,
    locator: Option<Value>,
    permission_policy: Option<Value>,
    sync_status: Option<String>,
}

pub async fn update_project_resource(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: UpdateProjectResourceInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    let mut body = serde_json::Map::new();
    if let Some(kind) = input.kind {
        body.insert("kind".to_string(), json!(kind));
    }
    if let Some(name) = input.name {
        body.insert("name".to_string(), json!(name));
    }
    if let Some(locator) = input.locator {
        body.insert("locator".to_string(), locator);
    }
    if let Some(permission_policy) = input.permission_policy {
        body.insert("permission_policy".to_string(), permission_policy);
    }
    if let Some(sync_status) = input.sync_status {
        body.insert("sync_status".to_string(), json!(sync_status));
    }

    match client
        .update_project_resource(&input.project_id, &input.resource_id, Value::Object(body))
        .await
    {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

pub fn delete_project_resource_tool() -> ToolDefinition {
    ToolDefinition {
        name: "project_resources.delete".to_string(),
        description: "Delete a project resource".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string" },
                "resource_id": { "type": "string" }
            },
            "required": ["project_id", "resource_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct DeleteProjectResourceInput {
    project_id: String,
    resource_id: String,
}

pub async fn delete_project_resource(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: DeleteProjectResourceInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    match client
        .delete_project_resource(&input.project_id, &input.resource_id)
        .await
    {
        Ok(_) => CallToolResult::success("Project resource deleted"),
        Err(err) => CallToolResult::error(err),
    }
}
