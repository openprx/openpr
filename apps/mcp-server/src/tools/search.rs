use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::json;

pub fn search_all_tool() -> ToolDefinition {
    ToolDefinition {
        name: "search.all".to_string(),
        description: "Global search across projects, work items, and comments in a workspace"
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query"
                }
            },
            "required": ["query"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct SearchAllInput {
    query: String,
}

pub async fn search_all(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: SearchAllInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client.search_all(&input.query).await {
        Ok(results) => {
            let json = serde_json::to_string_pretty(&results).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}
