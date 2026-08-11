use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

pub fn get_release_readiness_tool() -> ToolDefinition {
    ToolDefinition {
        name: "release.readiness.get".to_string(),
        description: "Get project release/readiness gates, blockers, next actions, and AI governance evidence"
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "Project UUID"
                }
            },
            "required": ["project_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ReleaseReadinessInput {
    project_id: String,
}

pub async fn get_release_readiness(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ReleaseReadinessInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    match client.get_project_release_readiness(&input.project_id).await {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

#[cfg(test)]
mod tests {
    use super::get_release_readiness_tool;

    #[test]
    fn release_readiness_tool_documents_next_actions() {
        let tool = get_release_readiness_tool();

        assert_eq!(tool.name, "release.readiness.get");
        assert!(tool.description.contains("next actions"));
        assert_eq!(
            tool.input_schema.get("required"),
            Some(&serde_json::json!(["project_id"]))
        );
    }
}
