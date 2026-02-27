use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde_json::json;

pub fn list_members_tool() -> ToolDefinition {
    ToolDefinition {
        name: "members.list".to_string(),
        description: "List all members and roles in a workspace".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {}
        }),
    }
}

pub async fn list_members(client: &OpenPrClient, _args: serde_json::Value) -> CallToolResult {
    match client.list_members().await {
        Ok(members) => {
            let json = serde_json::to_string_pretty(&members).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}
