use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

pub fn get_project_context_tool() -> ToolDefinition {
    ToolDefinition {
        name: "context.get_project".to_string(),
        description: "Get project-aware MCP context including type, resources, connectors, governance, workflow, and recent decisions".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string", "description": "Project UUID" }
            },
            "required": ["project_id"]
        }),
    }
}

pub fn get_governance_context_tool() -> ToolDefinition {
    ToolDefinition {
        name: "context.get_governance".to_string(),
        description: "Get governance and workflow context for a project".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string", "description": "Project UUID" }
            },
            "required": ["project_id"]
        }),
    }
}

pub fn get_agent_policy_tool() -> ToolDefinition {
    ToolDefinition {
        name: "context.get_agent_policy".to_string(),
        description: "Get the effective MCP/AI agent policy for a project".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string", "description": "Project UUID" }
            },
            "required": ["project_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ProjectContextInput {
    project_id: String,
}

pub async fn get_project_context(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input = match parse_project_input(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(err),
    };

    match client.get_project_context(&input.project_id).await {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn get_governance_context(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input = match parse_project_input(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(err),
    };

    match client.get_project_governance_context(&input.project_id).await {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn get_agent_policy(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input = match parse_project_input(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(err),
    };

    match client.get_project_agent_policy(&input.project_id).await {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

fn parse_project_input(args: Value) -> Result<ProjectContextInput, String> {
    serde_json::from_value(args).map_err(|err| format!("Invalid input: {err}"))
}
