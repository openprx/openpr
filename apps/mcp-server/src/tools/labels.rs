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
