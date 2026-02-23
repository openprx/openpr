use crate::db;
use crate::protocol::{CallToolResult, ToolDefinition};
use platform::app::AppState;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

pub fn list_comments_tool() -> ToolDefinition {
    ToolDefinition {
        name: "comments.list".to_string(),
        description: "List all comments on a work item".to_string(),
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
struct ListCommentsInput {
    work_item_id: String,
}

pub async fn list_comments(state: &AppState, args: serde_json::Value) -> CallToolResult {
    let input: ListCommentsInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let work_item_id = match Uuid::parse_str(&input.work_item_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid work_item_id format"),
    };

    match db::comments::list_comments(state, work_item_id).await {
        Ok(comments) => {
            let json = serde_json::to_string_pretty(&comments).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn create_comment_tool() -> ToolDefinition {
    ToolDefinition {
        name: "comments.create".to_string(),
        description: "Create a new comment on a work item".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item"
                },
                "content": {
                    "type": "string",
                    "description": "Comment content"
                },
                "author_id": {
                    "type": "string",
                    "description": "UUID of the comment author (optional)"
                }
            },
            "required": ["work_item_id", "content"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct CreateCommentInput {
    work_item_id: String,
    content: String,
    author_id: Option<String>,
}

pub async fn create_comment(state: &AppState, args: serde_json::Value) -> CallToolResult {
    let input: CreateCommentInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let work_item_id = match Uuid::parse_str(&input.work_item_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid work_item_id format"),
    };

    // Use author_id from input, or fall back to configured default, or None
    let author_id = if let Some(aid) = input.author_id {
        match Uuid::parse_str(&aid) {
            Ok(id) => Some(id),
            Err(_) => return CallToolResult::error("Invalid author_id format"),
        }
    } else {
        // Use configured default_author_id from app config (if set)
        state.cfg.default_author_id
    };

    match db::comments::create_comment(state, work_item_id, input.content, author_id).await {
        Ok(comment) => {
            let json = serde_json::to_string_pretty(&comment).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn delete_comment_tool() -> ToolDefinition {
    ToolDefinition {
        name: "comments.delete".to_string(),
        description: "Delete a comment".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "comment_id": {
                    "type": "string",
                    "description": "UUID of the comment"
                }
            },
            "required": ["comment_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct DeleteCommentInput {
    comment_id: String,
}

pub async fn handle_delete_comment(state: &AppState, args: serde_json::Value) -> CallToolResult {
    let input: DeleteCommentInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let comment_id = match Uuid::parse_str(&input.comment_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid comment_id format"),
    };

    match db::comments::delete_comment(state, comment_id).await {
        Ok(true) => CallToolResult::success("Comment deleted"),
        Ok(false) => CallToolResult::error("Comment not found"),
        Err(e) => CallToolResult::error(e),
    }
}
