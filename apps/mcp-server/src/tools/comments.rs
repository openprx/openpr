use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::json;

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

pub async fn list_comments(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: ListCommentsInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client.list_comments(&input.work_item_id).await {
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
}

pub async fn create_comment(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: CreateCommentInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let body = json!({ "content": input.content });

    match client.create_comment(&input.work_item_id, body).await {
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

pub async fn handle_delete_comment(
    client: &OpenPrClient,
    args: serde_json::Value,
) -> CallToolResult {
    let input: DeleteCommentInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client.delete_comment(&input.comment_id).await {
        Ok(()) => CallToolResult::success("Comment deleted"),
        Err(e) => CallToolResult::error(e),
    }
}
