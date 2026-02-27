use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::json;

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

pub async fn list_work_items(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: ListWorkItemsInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client.list_work_items(&input.project_id).await {
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

pub async fn get_work_item(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: GetWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client.get_work_item(&input.work_item_id).await {
        Ok(item) => {
            let json = serde_json::to_string_pretty(&item).unwrap_or_default();
            CallToolResult::success(json)
        }
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
}

pub async fn create_work_item(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: CreateWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let body = json!({
        "title": input.title,
        "description": input.description,
        "state": input.state.unwrap_or_else(|| "open".to_string()),
        "priority": input.priority.unwrap_or_else(|| "medium".to_string()),
        "assignee_id": input.assignee_id,
        "due_at": input.due_at
    });

    match client.create_work_item(&input.project_id, body).await {
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

pub async fn update_work_item(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let args_obj = args.as_object().cloned();

    let input: UpdateWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let mut body = serde_json::Map::new();
    if let Some(title) = input.title {
        body.insert("title".to_string(), json!(title));
    }
    if let Some(desc) = input.description {
        body.insert("description".to_string(), json!(desc));
    }
    if let Some(state) = input.state {
        body.insert("state".to_string(), json!(state));
    }
    if let Some(priority) = input.priority {
        body.insert("priority".to_string(), json!(priority));
    }
    if args_obj.as_ref().and_then(|o| o.get("assignee_id")).is_some() {
        match input.assignee_id.as_deref() {
            Some("") | Some("null") | None => {
                body.insert("assignee_id".to_string(), serde_json::Value::Null);
            }
            Some(aid) => {
                body.insert("assignee_id".to_string(), json!(aid));
            }
        }
    }
    if args_obj.as_ref().and_then(|o| o.get("due_at")).is_some() {
        match input.due_at.as_deref() {
            Some("") | Some("null") | None => {
                body.insert("due_at".to_string(), serde_json::Value::Null);
            }
            Some(d) => {
                body.insert("due_at".to_string(), json!(d));
            }
        }
    }

    match client
        .update_work_item(&input.work_item_id, serde_json::Value::Object(body))
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
                "query": {
                    "type": "string",
                    "description": "Search query (matches title and description)"
                }
            },
            "required": ["query"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct SearchWorkItemsInput {
    query: String,
}

pub async fn search_work_items(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: SearchWorkItemsInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client.search_work_items(&input.query).await {
        Ok(items) => {
            let json = serde_json::to_string_pretty(&items).unwrap_or_default();
            CallToolResult::success(json)
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
    client: &OpenPrClient,
    args: serde_json::Value,
) -> CallToolResult {
    let input: DeleteWorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client.delete_work_item(&input.work_item_id).await {
        Ok(()) => CallToolResult::success("Work item deleted"),
        Err(e) => CallToolResult::error(e),
    }
}
