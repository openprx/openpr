use crate::db;
use crate::protocol::{CallToolResult, ToolDefinition};
use platform::app::AppState;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

pub fn list_members_tool() -> ToolDefinition {
    ToolDefinition {
        name: "members.list".to_string(),
        description: "List all members and roles in a workspace".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "workspace_id": {
                    "type": "string",
                    "description": "UUID of the workspace"
                }
            },
            "required": ["workspace_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ListMembersInput {
    workspace_id: String,
}

pub async fn list_members(state: &AppState, args: serde_json::Value) -> CallToolResult {
    let input: ListMembersInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let workspace_id = match Uuid::parse_str(&input.workspace_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid workspace_id format"),
    };

    match db::members::list_members(state, workspace_id).await {
        Ok(members) => {
            let json = serde_json::to_string_pretty(&members).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}
