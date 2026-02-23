use crate::db;
use crate::protocol::{CallToolResult, ToolDefinition};
use platform::app::AppState;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

pub fn list_projects_tool() -> ToolDefinition {
    ToolDefinition {
        name: "projects.list".to_string(),
        description: "List all projects in a workspace".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "workspace_id": {
                    "type": "string",
                    "description": "UUID of the workspace"
                }
            },
            "required": ["workspace_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ListProjectsInput {
    workspace_id: String,
}

pub async fn list_projects(state: &AppState, args: serde_json::Value) -> CallToolResult {
    let input: ListProjectsInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let workspace_id = match Uuid::parse_str(&input.workspace_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid workspace_id format"),
    };

    match db::projects::list_projects(state, workspace_id).await {
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
                    "description": "UUID of the project"
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

pub async fn get_project(state: &AppState, args: serde_json::Value) -> CallToolResult {
    let input: GetProjectInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let project_id = match Uuid::parse_str(&input.project_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid project_id format"),
    };

    match db::projects::get_project(state, project_id).await {
        Ok(Some(project)) => {
            let json = serde_json::to_string_pretty(&project).unwrap_or_default();
            CallToolResult::success(json)
        }
        Ok(None) => CallToolResult::error("Project not found"),
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
                "workspace_id": {
                    "type": "string",
                    "description": "UUID of the workspace"
                },
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
                },
                "created_by": {
                    "type": "string",
                    "description": "UUID of the creator (optional)"
                }
            },
            "required": ["workspace_id", "key", "name"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct CreateProjectInput {
    workspace_id: String,
    key: String,
    name: String,
    description: Option<String>,
    created_by: Option<String>,
}

pub async fn create_project(state: &AppState, args: serde_json::Value) -> CallToolResult {
    let input: CreateProjectInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let workspace_id = match Uuid::parse_str(&input.workspace_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid workspace_id format"),
    };

    let created_by = if let Some(cb) = input.created_by {
        match Uuid::parse_str(&cb) {
            Ok(id) => Some(id),
            Err(_) => return CallToolResult::error("Invalid created_by format"),
        }
    } else {
        None
    };

    match db::projects::create_project(
        state,
        workspace_id,
        input.key,
        input.name,
        input.description.unwrap_or_default(),
        created_by,
    )
    .await
    {
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
                    "description": "UUID of the project"
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

pub async fn update_project(state: &AppState, args: serde_json::Value) -> CallToolResult {
    let input: UpdateProjectInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let project_id = match Uuid::parse_str(&input.project_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid project_id format"),
    };

    match db::projects::update_project(state, project_id, input.name, input.description).await {
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
                    "description": "UUID of the project"
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

pub async fn handle_delete_project(state: &AppState, args: serde_json::Value) -> CallToolResult {
    let input: DeleteProjectInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let project_id = match Uuid::parse_str(&input.project_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid project_id format"),
    };

    match db::projects::delete_project(state, project_id).await {
        Ok(true) => CallToolResult::success("Project deleted"),
        Ok(false) => CallToolResult::error("Project not found"),
        Err(e) => CallToolResult::error(e),
    }
}
