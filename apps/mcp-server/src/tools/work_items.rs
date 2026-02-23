use crate::db;
use crate::protocol::{CallToolResult, ToolDefinition};
use platform::app::AppState;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

pub fn list_work_items_tool() -> ToolDefinition {
    ToolDefinition {
        name: "work_items.list".to_string(),
        description: "List all work items in a project".to_string(),
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
struct ListWorkItemsInput {
    project_id: String,
}

pub async fn list_work_items(state: &AppState, args: serde_json::Value) -> CallToolResult {
    let input: ListWorkItemsInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let project_id = match Uuid::parse_str(&input.project_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid project_id format"),
    };

    match db::work_items::list_work_items(state, project_id).await {
        Ok(items) => {
            let json = serde_json::to_string_pretty(&items).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn get_work_item_tool() -> ToolDefinition {
    ToolDefinition {
        name: "work_items.get".to_string(),
        description: "Get details of a specific work item".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item"
                }
            },
            "required": ["work_item_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct GetWorkItemInput {
    work_item_id: String,
}

pub async fn get_work_item(state: &AppState, args: serde_json::Value) -> CallToolResult {
    let input: GetWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let work_item_id = match Uuid::parse_str(&input.work_item_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid work_item_id format"),
    };

    match db::work_items::get_work_item(state, work_item_id).await {
        Ok(Some(item)) => {
            let json = serde_json::to_string_pretty(&item).unwrap_or_default();
            CallToolResult::success(json)
        }
        Ok(None) => CallToolResult::error("Work item not found"),
        Err(e) => CallToolResult::error(e),
    }
}

pub fn create_work_item_tool() -> ToolDefinition {
    ToolDefinition {
        name: "work_items.create".to_string(),
        description: "Create a new work item in a project".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "UUID of the project"
                },
                "title": {
                    "type": "string",
                    "description": "Work item title"
                },
                "description": {
                    "type": "string",
                    "description": "Work item description (optional)"
                },
                "state": {
                    "type": "string",
                    "description": "Work item state (e.g., 'open', 'in_progress', 'closed')",
                    "default": "open"
                },
                "priority": {
                    "type": "string",
                    "description": "Work item priority (e.g., 'low', 'medium', 'high', 'critical')",
                    "default": "medium"
                },
                "assignee_id": {
                    "type": "string",
                    "description": "UUID of the assignee (optional)"
                },
                "due_at": {
                    "type": "string",
                    "description": "Due date in RFC3339 format (optional)"
                },
                "created_by": {
                    "type": "string",
                    "description": "UUID of the creator (optional)"
                }
            },
            "required": ["project_id", "title"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct CreateWorkItemInput {
    project_id: String,
    title: String,
    description: Option<String>,
    state: Option<String>,
    priority: Option<String>,
    assignee_id: Option<String>,
    due_at: Option<String>,
    created_by: Option<String>,
}

pub async fn create_work_item(state: &AppState, args: serde_json::Value) -> CallToolResult {
    let input: CreateWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let project_id = match Uuid::parse_str(&input.project_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid project_id format"),
    };

    let assignee_id = if let Some(aid) = input.assignee_id {
        match Uuid::parse_str(&aid) {
            Ok(id) => Some(id),
            Err(_) => return CallToolResult::error("Invalid assignee_id format"),
        }
    } else {
        None
    };

    let created_by = if let Some(cb) = input.created_by {
        match Uuid::parse_str(&cb) {
            Ok(id) => Some(id),
            Err(_) => return CallToolResult::error("Invalid created_by format"),
        }
    } else {
        None
    };

    match db::work_items::create_work_item(
        state,
        project_id,
        input.title,
        input.description.unwrap_or_default(),
        input.state.unwrap_or_else(|| "open".to_string()),
        input.priority.unwrap_or_else(|| "medium".to_string()),
        assignee_id,
        input.due_at,
        created_by,
    )
    .await
    {
        Ok(item) => {
            let json = serde_json::to_string_pretty(&item).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn update_work_item_tool() -> ToolDefinition {
    ToolDefinition {
        name: "work_items.update".to_string(),
        description: "Update an existing work item".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item"
                },
                "title": {
                    "type": "string",
                    "description": "New title (optional)"
                },
                "description": {
                    "type": "string",
                    "description": "New description (optional)"
                },
                "state": {
                    "type": "string",
                    "description": "New state (optional)"
                },
                "priority": {
                    "type": "string",
                    "description": "New priority (optional)"
                },
                "assignee_id": {
                    "type": "string",
                    "description": "New assignee UUID or null to unassign (optional)"
                },
                "due_at": {
                    "type": "string",
                    "description": "New due date in RFC3339 format or null to clear (optional)"
                }
            },
            "required": ["work_item_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct UpdateWorkItemInput {
    work_item_id: String,
    title: Option<String>,
    description: Option<String>,
    state: Option<String>,
    priority: Option<String>,
    assignee_id: Option<String>,
    due_at: Option<String>,
}

pub async fn update_work_item(state: &AppState, args: serde_json::Value) -> CallToolResult {
    // Clone args for later checking
    let args_obj = args.as_object().cloned();

    let input: UpdateWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let work_item_id = match Uuid::parse_str(&input.work_item_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid work_item_id format"),
    };

    // Handle assignee_id specially - need to distinguish between Some(Some(uuid)), Some(None), and None
    let assignee_id = if args_obj
        .as_ref()
        .and_then(|o| o.get("assignee_id"))
        .is_some()
    {
        if let Some(aid) = input.assignee_id {
            if aid == "null" || aid.is_empty() {
                Some(None)
            } else {
                match Uuid::parse_str(&aid) {
                    Ok(id) => Some(Some(id)),
                    Err(_) => return CallToolResult::error("Invalid assignee_id format"),
                }
            }
        } else {
            Some(None)
        }
    } else {
        None
    };

    // Handle due_at similarly
    let due_at = if args_obj.as_ref().and_then(|o| o.get("due_at")).is_some() {
        if let Some(d) = input.due_at {
            if d == "null" || d.is_empty() {
                Some(None)
            } else {
                Some(Some(d))
            }
        } else {
            Some(None)
        }
    } else {
        None
    };

    match db::work_items::update_work_item(
        state,
        work_item_id,
        input.title,
        input.description,
        input.state,
        input.priority,
        assignee_id,
        due_at,
    )
    .await
    {
        Ok(item) => {
            let json = serde_json::to_string_pretty(&item).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn search_work_items_tool() -> ToolDefinition {
    ToolDefinition {
        name: "work_items.search".to_string(),
        description: "Search work items across all projects in a workspace".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "workspace_id": {
                    "type": "string",
                    "description": "UUID of the workspace"
                },
                "query": {
                    "type": "string",
                    "description": "Search query (matches title and description)"
                }
            },
            "required": ["workspace_id", "query"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct SearchWorkItemsInput {
    workspace_id: String,
    query: String,
}

pub async fn search_work_items(state: &AppState, args: serde_json::Value) -> CallToolResult {
    let input: SearchWorkItemsInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let workspace_id = match Uuid::parse_str(&input.workspace_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid workspace_id format"),
    };

    match db::work_items::search_work_items(state, workspace_id, &input.query).await {
        Ok(items) => {
            let count = items.len();
            let json = serde_json::to_string_pretty(&items).unwrap_or_default();
            CallToolResult::success(format!("Found {} work items:\n{}", count, json))
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn delete_work_item_tool() -> ToolDefinition {
    ToolDefinition {
        name: "work_items.delete".to_string(),
        description: "Delete a work item".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item"
                }
            },
            "required": ["work_item_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct DeleteWorkItemInput {
    work_item_id: String,
}

pub async fn handle_delete_work_item(
    state: &AppState,
    args: serde_json::Value,
) -> CallToolResult {
    let input: DeleteWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let work_item_id = match Uuid::parse_str(&input.work_item_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid work_item_id format"),
    };

    match db::work_items::delete_work_item(state, work_item_id).await {
        Ok(true) => CallToolResult::success("Work item deleted"),
        Ok(false) => CallToolResult::error("Work item not found"),
        Err(e) => CallToolResult::error(e),
    }
}
