use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;

pub fn list_comments_tool() -> ToolDefinition {
    ToolDefinition {
        name: "comments.list".to_string(),
        description: "List all comments on a work item".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
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
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
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
        description: "Create a new comment on a work item. @mentions in content (e.g. @Codex CLI) are automatically resolved to user UUIDs.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": {
                    "type": "string",
                    "description": "UUID of the work item",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "content": {
                    "type": "string",
                    "description": "Comment content"
                },
                "attachments": {
                    "type": "array",
                    "description": "Uploaded file URLs (optional)",
                    "items": {
                        "type": "string"
                    }
                },
                "mentions": {
                    "type": "array",
                    "description": "Optional extra user UUIDs to @mention (in addition to auto-resolved @mentions from content)",
                    "items": {
                        "type": "string"
                    }
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
    attachments: Option<Vec<String>>,
    mentions: Option<Vec<String>>,
}

pub async fn create_comment(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: CreateCommentInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    let content = append_attachments_to_content(input.content, input.attachments);

    // Auto-resolve @mentions from content text, merging with any explicit UUIDs
    let auto_mention_ids = resolve_mentions(&content, client).await;
    let all_mentions = merge_mentions(input.mentions, auto_mention_ids);

    let mut body = json!({ "content": content });
    if !all_mentions.is_empty()
        && let Some(obj) = body.as_object_mut()
    {
        obj.insert("mentions".to_string(), json!(all_mentions));
    }

    match client.create_comment(&input.work_item_id, body).await {
        Ok(comment) => {
            let json = serde_json::to_string_pretty(&comment).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

fn append_attachments_to_content(content: String, attachments: Option<Vec<String>>) -> String {
    use std::fmt::Write as _;
    match attachments {
        Some(items) if !items.is_empty() => {
            let mut output = content;
            output.push_str("\n\n**附件：**\n");
            for url in items {
                let name = attachment_name_from_url(&url);
                let _ = writeln!(output, "- [{name}]({url})");
            }
            output.trim_end().to_string()
        }
        _ => content,
    }
}

fn attachment_name_from_url(url: &str) -> String {
    url.rsplit('/')
        .next()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(url)
        .to_string()
}

/// Extract `@mention` names from comment content.
///
/// Handles patterns like `@Codex CLI`, `@Claude Code`, `请 @admin 审查`.
/// A mention starts with `@` and captures all subsequent characters until a
/// delimiter is encountered: another `@`, a newline, or common punctuation
/// that is unlikely to be part of a username (`,，。.!！?？;；:`).
/// Trailing whitespace on each captured name is trimmed.
fn extract_mention_names(content: &str) -> Vec<String> {
    let mut names = Vec::new();
    let delimiters: &[char] = &['@', '\n', '\r', ',', '，', '。', '.', '!', '！', '?', '？', ';', '；', ':'];
    let chars: Vec<char> = content.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars.get(i).copied() == Some('@') {
            i += 1; // skip the '@'
            let start = i;
            // Consume until delimiter or end
            while i < len {
                let c = chars.get(i).copied().unwrap_or('\0');
                if delimiters.contains(&c) {
                    break;
                }
                i += 1;
            }
            if i > start {
                let name: String = chars.get(start..i).map_or_else(String::new, |s| s.iter().collect());
                let trimmed = name.trim();
                if !trimmed.is_empty() {
                    names.push(trimmed.to_string());
                }
            }
        } else {
            i += 1;
        }
    }

    names
}

/// Resolve `@name` references in content to user UUIDs by looking up workspace members.
///
/// On any error (network, parse), returns an empty vec so the comment is still created.
async fn resolve_mentions(content: &str, client: &OpenPrClient) -> Vec<String> {
    let mentioned_names = extract_mention_names(content);
    if mentioned_names.is_empty() {
        return Vec::new();
    }

    // Fetch workspace members
    let members_value: Value = match client.list_members().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to fetch members for mention resolution");
            return Vec::new();
        }
    };

    // The API may return { "data": { "items": [...] } } (paginated),
    // { "data": [...] } (array), or just [...] (root array).
    let members_array = members_value
        .get("data")
        .and_then(|d| d.get("items"))
        .and_then(Value::as_array)
        .or_else(|| members_value.get("data").and_then(Value::as_array))
        .or_else(|| members_value.as_array());

    let Some(members) = members_array else {
        tracing::warn!("Members response is not an array; skipping mention resolution");
        return Vec::new();
    };

    // Build a lookup: lowercase name -> user id
    let mut name_to_id: Vec<(String, String)> = Vec::new();
    for member in members {
        // Try nested user object first: { "user": { "id": "...", "name": "..." } }
        let user_obj = member.get("user").unwrap_or(member);
        let id = user_obj
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| user_obj.get("user_id").and_then(Value::as_str));
        let name = user_obj.get("name").and_then(Value::as_str);

        if let (Some(uid), Some(uname)) = (id, name) {
            name_to_id.push((uname.to_lowercase(), uid.to_string()));
        }
    }

    let mut resolved = Vec::new();
    for mentioned in &mentioned_names {
        let m_lower = mentioned.to_lowercase();
        for (member_name_lower, member_id) in &name_to_id {
            // Match if: exact match, or member name contains mention, or mention contains member name.
            // e.g. @Codex CLI matches member "codex" because "codex cli" contains "codex".
            if *member_name_lower == m_lower
                || member_name_lower.contains(m_lower.as_str())
                || m_lower.contains(member_name_lower.as_str())
            {
                resolved.push(member_id.clone());
                break;
            }
        }
    }

    resolved
}

/// Merge explicit mention UUIDs with auto-resolved ones, deduplicating.
fn merge_mentions(explicit: Option<Vec<String>>, auto_resolved: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();

    // Explicit mentions first
    if let Some(ids) = explicit {
        for id in ids {
            if seen.insert(id.clone()) {
                result.push(id);
            }
        }
    }

    // Then auto-resolved
    for id in auto_resolved {
        if seen.insert(id.clone()) {
            result.push(id);
        }
    }

    result
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
                    "description": "UUID of the comment",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
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

pub async fn handle_delete_comment(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: DeleteCommentInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    match client.delete_comment(&input.comment_id).await {
        Ok(()) => CallToolResult::success("Comment deleted"),
        Err(e) => CallToolResult::error(e),
    }
}
