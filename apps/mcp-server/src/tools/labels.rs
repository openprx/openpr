use crate::db;
use crate::protocol::{CallToolResult, ToolDefinition};
use platform::app::AppState;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

pub fn create_label_tool() -> ToolDefinition {
    ToolDefinition {
        name: "labels.create".to_string(),
        description: "Create a label in workspace".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "workspace_id": {
                    "type": "string",
                    "description": "UUID of the workspace"
                },
                "name": {
                    "type": "string",
                    "description": "Label name"
                },
                "color": {
                    "type": "string",
                    "description": "Label color"
                }
            },
            "required": ["workspace_id", "name", "color"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct CreateLabelInput {
    workspace_id: String,
    name: String,
    color: String,
}

pub async fn create_label(state: &AppState, args: serde_json::Value) -> CallToolResult {
    let input: CreateLabelInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let workspace_id = match Uuid::parse_str(&input.workspace_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid workspace_id format"),
    };

    match db::labels::create_label(state, workspace_id, input.name, input.color).await {
        Ok(label) => {
            let json = serde_json::to_string_pretty(&label).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}
