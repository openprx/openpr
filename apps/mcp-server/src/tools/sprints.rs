use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::json;

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

pub async fn create_sprint(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: CreateSprintInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let body = json!({
        "name": input.name,
        "start_date": input.start_date,
        "end_date": input.end_date
    });

    match client.create_sprint(&input.project_id, body).await {
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

pub async fn update_sprint(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: UpdateSprintInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let mut body = serde_json::Map::new();
    if let Some(name) = input.name {
        body.insert("name".to_string(), json!(name));
    }
    if let Some(status) = input.status {
        body.insert("status".to_string(), json!(status));
    }
    if let Some(start) = input.start_date {
        body.insert("start_date".to_string(), json!(start));
    }
    if let Some(end) = input.end_date {
        body.insert("end_date".to_string(), json!(end));
    }

    match client
        .update_sprint(&input.sprint_id, serde_json::Value::Object(body))
        .await
    {
        Ok(sprint) => {
            let json = serde_json::to_string_pretty(&sprint).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}
