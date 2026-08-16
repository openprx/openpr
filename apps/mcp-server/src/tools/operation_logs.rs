use crate::client::{OpenPrClient, encode_query_component};
use crate::protocol::{CallToolResult, ToolDefinition};
use serde_json::{Value, json};

pub fn list_bot_operation_logs_tool() -> ToolDefinition {
    ToolDefinition {
        name: "bot_operation_logs.list".to_string(),
        description: "List metadata-only bot operation records for the configured workspace".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "bot_id": { "type": "string", "format": "uuid" },
                "tool_name": { "type": "string" },
                "outcome": { "type": "string", "enum": ["ok", "error"] },
                "cursor": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 50 }
            }
        }),
    }
}

pub async fn list_bot_operation_logs(client: &OpenPrClient, args: Value) -> CallToolResult {
    let mut query = Vec::new();
    for key in ["bot_id", "tool_name", "outcome", "cursor"] {
        if let Some(raw) = args.get(key) {
            let Some(value) = raw.as_str() else {
                return CallToolResult::error(format!("{key} must be a string"));
            };
            query.push(format!("{key}={}", encode_query_component(value)));
        }
    }
    if let Some(raw) = args.get("limit") {
        let Some(limit) = raw.as_u64() else {
            return CallToolResult::error("limit must be an integer".to_string());
        };
        if !(1..=100).contains(&limit) {
            return CallToolResult::error("limit must be between 1 and 100".to_string());
        }
        query.push(format!("limit={limit}"));
    }
    let suffix = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    match client.list_bot_operation_logs(&suffix).await {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(error) => CallToolResult::error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::list_bot_operation_logs;
    use crate::client::test_api;
    use serde_json::json;

    #[tokio::test]
    async fn rejects_wrongly_typed_filters_before_calling_the_api() -> Result<(), String> {
        let client = test_api::client("http://127.0.0.1:1".to_string())?;

        let invalid_filter = list_bot_operation_logs(&client, json!({ "tool_name": 7 })).await;
        let invalid_limit = list_bot_operation_logs(&client, json!({ "limit": 101 })).await;

        assert_eq!(invalid_filter.is_error, Some(true));
        assert_eq!(invalid_limit.is_error, Some(true));
        Ok(())
    }
}
