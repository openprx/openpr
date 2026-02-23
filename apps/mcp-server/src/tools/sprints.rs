use crate::db;
use crate::protocol::{CallToolResult, ToolDefinition};
use platform::app::AppState;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

pub fn create_sprint_tool() -> ToolDefinition {
    ToolDefinition {
        name: "sprints.create".to_string(),
        description: "Create a sprint in a project".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "UUID of the project"
                },
                "name": {
                    "type": "string",
                    "description": "Sprint name"
                },
                "start_date": {
                    "type": "string",
                    "description": "Start date (YYYY-MM-DD, optional)"
                },
                "end_date": {
                    "type": "string",
                    "description": "End date (YYYY-MM-DD, optional)"
                }
            },
            "required": ["project_id", "name"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct CreateSprintInput {
    project_id: String,
    name: String,
    start_date: Option<String>,
    end_date: Option<String>,
}

pub async fn create_sprint(state: &AppState, args: serde_json::Value) -> CallToolResult {
    let input: CreateSprintInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let project_id = match Uuid::parse_str(&input.project_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid project_id format"),
    };

    match db::sprints::create_sprint(
        state,
        project_id,
        input.name,
        input.start_date,
        input.end_date,
    )
    .await
    {
        Ok(sprint) => {
            let json = serde_json::to_string_pretty(&sprint).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn update_sprint_tool() -> ToolDefinition {
    ToolDefinition {
        name: "sprints.update".to_string(),
        description: "Update sprint fields".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "sprint_id": {
                    "type": "string",
                    "description": "UUID of the sprint"
                },
                "name": {
                    "type": "string",
                    "description": "Sprint name (optional)"
                },
                "status": {
                    "type": "string",
                    "description": "Sprint status (optional)"
                },
                "start_date": {
                    "type": "string",
                    "description": "Start date (YYYY-MM-DD, optional)"
                },
                "end_date": {
                    "type": "string",
                    "description": "End date (YYYY-MM-DD, optional)"
                }
            },
            "required": ["sprint_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct UpdateSprintInput {
    sprint_id: String,
    name: Option<String>,
    status: Option<String>,
    start_date: Option<String>,
    end_date: Option<String>,
}

pub async fn update_sprint(state: &AppState, args: serde_json::Value) -> CallToolResult {
    let input: UpdateSprintInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let sprint_id = match Uuid::parse_str(&input.sprint_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid sprint_id format"),
    };

    match db::sprints::update_sprint(
        state,
        sprint_id,
        input.name,
        input.status,
        input.start_date,
        input.end_date,
    )
    .await
    {
        Ok(sprint) => {
            let json = serde_json::to_string_pretty(&sprint).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}
