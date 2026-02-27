use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::json;

pub fn list_projects_tool() -> ToolDefinition {
    ToolDefinition {
        name: "projects.list".to_string(),
        description: "List all projects in a workspace".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "workspace_id": {
                    "type": "string",
                    "description": "UUID of the workspace (optional, uses bot token workspace)",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                }
            }
        }),
    }
}

pub async fn list_projects(client: &OpenPrClient, _args: serde_json::Value) -> CallToolResult {
    match client.list_projects().await {
        Ok(projects) => {
            let json = serde_json::to_string_pretty(&projects).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn get_project_tool() -> ToolDefinition {
    ToolDefinition {
        name: "projects.get".to_string(),
        description: "Get details of a specific project".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "UUID of the project",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                }
            },
            "required": ["project_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct GetProjectInput {
    project_id: String,
}

pub async fn get_project(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: GetProjectInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client.get_project(&input.project_id).await {
        Ok(project) => {
            let json = serde_json::to_string_pretty(&project).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn create_project_tool() -> ToolDefinition {
    ToolDefinition {
        name: "projects.create".to_string(),
        description: "Create a new project in a workspace".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "Project key (uppercase letters only, e.g., 'PROJ')"
                },
                "name": {
                    "type": "string",
                    "description": "Project name"
                },
                "description": {
                    "type": "string",
                    "description": "Project description (optional)"
                }
            },
            "required": ["key", "name"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct CreateProjectInput {
    key: String,
    name: String,
    description: Option<String>,
}

pub async fn create_project(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: CreateProjectInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let body = json!({
        "key": input.key,
        "name": input.name,
        "description": input.description
    });

    match client.create_project(body).await {
        Ok(project) => {
            let json = serde_json::to_string_pretty(&project).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn update_project_tool() -> ToolDefinition {
    ToolDefinition {
        name: "projects.update".to_string(),
        description: "Update an existing project".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "UUID of the project",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "name": {
                    "type": "string",
                    "description": "New project name (optional)"
                },
                "description": {
                    "type": "string",
                    "description": "New project description (optional)"
                }
            },
            "required": ["project_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct UpdateProjectInput {
    project_id: String,
    name: Option<String>,
    description: Option<String>,
}

pub async fn update_project(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: UpdateProjectInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let mut body = serde_json::Map::new();
    if let Some(name) = input.name {
        body.insert("name".to_string(), json!(name));
    }
    if let Some(desc) = input.description {
        body.insert("description".to_string(), json!(desc));
    }

    match client
        .update_project(&input.project_id, serde_json::Value::Object(body))
        .await
    {
        Ok(project) => {
            let json = serde_json::to_string_pretty(&project).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn delete_project_tool() -> ToolDefinition {
    ToolDefinition {
        name: "projects.delete".to_string(),
        description: "Delete an existing project".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "UUID of the project",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                }
            },
            "required": ["project_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct DeleteProjectInput {
    project_id: String,
}

pub async fn handle_delete_project(
    client: &OpenPrClient,
    args: serde_json::Value,
) -> CallToolResult {
    let input: DeleteProjectInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client.delete_project(&input.project_id).await {
        Ok(()) => CallToolResult::success("Project deleted"),
        Err(e) => CallToolResult::error(e),
    }
}
