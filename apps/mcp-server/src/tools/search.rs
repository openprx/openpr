use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::json;

pub fn search_all_tool() -> ToolDefinition {
    ToolDefinition {
        name: "search.all".to_string(),
        description: "Global search across projects, work items, and comments the caller can access".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query"
                },
                "type": {
                    "type": "string",
                    "enum": ["issue", "project", "comment"],
                    "description": "Optional result type filter; omit to search all types"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional maximum number of rows per result type"
                }
            },
            "required": ["query"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct SearchAllInput {
    query: String,
    #[serde(rename = "type")]
    search_type: Option<String>,
    limit: Option<u32>,
}

pub async fn search_all(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: SearchAllInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    match client
        .search(&input.query, input.search_type.as_deref(), input.limit)
        .await
    {
        Ok(results) => {
            let json = serde_json::to_string_pretty(&results).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}
