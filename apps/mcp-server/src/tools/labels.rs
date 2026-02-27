use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::json;

pub fn create_label_tool() -> ToolDefinition {
    ToolDefinition {
        name: "labels.create".to_string(),
        description: "Create a label in workspace".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Label name"
                },
                "color": {
                    "type": "string",
                    "description": "Label color"
                }
            },
            "required": ["name", "color"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct CreateLabelInput {
    name: String,
    color: String,
}

pub async fn create_label(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: CreateLabelInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let body = json!({ "name": input.name, "color": input.color });

    match client.create_label(body).await {
        Ok(label) => {
            let json = serde_json::to_string_pretty(&label).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn list_labels_tool() -> ToolDefinition {
    ToolDefinition {
        name: "labels.list".to_string(),
        description: "List all labels in workspace".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {}
        }),
    }
}

pub async fn list_labels(client: &OpenPrClient, _args: serde_json::Value) -> CallToolResult {
    match client.list_labels().await {
        Ok(labels) => {
            let json = serde_json::to_string_pretty(&labels).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn list_project_labels_tool() -> ToolDefinition {
    ToolDefinition {
        name: "labels.list_by_project".to_string(),
        description: "List all labels available for a specific project".to_string(),
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
struct ListProjectLabelsInput {
    project_id: String,
}

pub async fn list_project_labels(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: ListProjectLabelsInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client.list_project_labels(&input.project_id).await {
        Ok(labels) => {
            let json = serde_json::to_string_pretty(&labels).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn update_label_tool() -> ToolDefinition {
    ToolDefinition {
        name: "labels.update".to_string(),
        description: "Update label fields".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "label_id": {
                    "type": "string",
                    "description": "UUID of the label",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "name": {
                    "type": "string",
                    "description": "Label name (optional)"
                },
                "color": {
                    "type": "string",
                    "description": "Label color (optional)"
                },
                "description": {
                    "type": "string",
                    "description": "Label description (optional)"
                }
            },
            "required": ["label_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct UpdateLabelInput {
    label_id: String,
    name: Option<String>,
    color: Option<String>,
    description: Option<String>,
}

pub async fn update_label(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: UpdateLabelInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let mut body = serde_json::Map::new();
    if let Some(name) = input.name {
        body.insert("name".to_string(), json!(name));
    }
    if let Some(color) = input.color {
        body.insert("color".to_string(), json!(color));
    }
    if let Some(description) = input.description {
        body.insert("description".to_string(), json!(description));
    }

    match client
        .update_label(&input.label_id, serde_json::Value::Object(body))
        .await
    {
        Ok(label) => {
            let json = serde_json::to_string_pretty(&label).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn delete_label_tool() -> ToolDefinition {
    ToolDefinition {
        name: "labels.delete".to_string(),
        description: "Delete a label".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "label_id": {
                    "type": "string",
                    "description": "UUID of the label",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                }
            },
            "required": ["label_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct DeleteLabelInput {
    label_id: String,
}

pub async fn handle_delete_label(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: DeleteLabelInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client.delete_label(&input.label_id).await {
        Ok(()) => CallToolResult::success("Label deleted"),
        Err(e) => CallToolResult::error(e),
    }
}
