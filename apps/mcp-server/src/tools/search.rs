use crate::db;
use crate::protocol::{CallToolResult, ToolDefinition};
use platform::app::AppState;
use sea_orm::{ConnectionTrait, DbErr, FromQueryResult};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

pub fn search_all_tool() -> ToolDefinition {
    ToolDefinition {
        name: "search.all".to_string(),
        description: "Global search across projects, work items, and comments in a workspace"
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "workspace_id": {
                    "type": "string",
                    "description": "UUID of the workspace"
                },
                "query": {
                    "type": "string",
                    "description": "Search query"
                }
            },
            "required": ["workspace_id", "query"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct SearchAllInput {
    workspace_id: String,
    query: String,
}

pub async fn search_all(state: &AppState, args: serde_json::Value) -> CallToolResult {
    let input: SearchAllInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let workspace_id = match Uuid::parse_str(&input.workspace_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid workspace_id format"),
    };

    let search_pattern = format!("%{}%", input.query);

    // Search projects
    let projects_result = state
        .db
        .query_all(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            r#"
                SELECT 
                    id::text,
                    workspace_id::text,
                    key,
                    name,
                    description,
                    created_at::text,
                    updated_at::text
                FROM projects 
                WHERE workspace_id = $1
                  AND (name ILIKE $2 OR description ILIKE $2 OR key ILIKE $2)
                ORDER BY created_at DESC
                LIMIT 20
            "#,
            vec![workspace_id.into(), search_pattern.clone().into()],
        ))
        .await;

    let projects = match projects_result {
        Ok(rows) => rows
            .into_iter()
            .map(|row| {
                db::projects::ProjectInfo::from_query_result(&row, "")
                    .map_err(|e: sea_orm::DbErr| e.to_string())
            })
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_default(),
        Err(_) => vec![],
    };

    // Search work items
    let work_items: Vec<db::work_items::WorkItemInfo> =
        db::work_items::search_work_items(state, workspace_id, &input.query)
            .await
            .unwrap_or_default();

    // Search comments
    let comments_result: Result<Vec<sea_orm::QueryResult>, DbErr> = state
        .db
        .query_all(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            r#"
                SELECT 
                    c.id::text,
                    c.work_item_id::text,
                    c.content,
                    c.author_id::text,
                    c.created_at::text,
                    c.updated_at::text
                FROM comments c
                JOIN work_items wi ON c.work_item_id = wi.id
                JOIN projects p ON wi.project_id = p.id
                WHERE p.workspace_id = $1
                  AND c.content ILIKE $2
                ORDER BY c.created_at DESC
                LIMIT 20
            "#,
            vec![workspace_id.into(), search_pattern.into()],
        ))
        .await;

    let comments = match comments_result {
        Ok(rows) => rows
            .into_iter()
            .map(|row| {
                db::comments::CommentInfo::from_query_result(&row, "")
                    .map_err(|e: sea_orm::DbErr| e.to_string())
            })
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_default(),
        Err(_) => vec![],
    };

    let result = json!({
        "query": input.query,
        "results": {
            "projects": projects,
            "work_items": work_items,
            "comments": comments
        },
        "counts": {
            "projects": projects.len(),
            "work_items": work_items.len(),
            "comments": comments.len(),
            "total": projects.len() + work_items.len() + comments.len()
        }
    });

    let json = serde_json::to_string_pretty(&result).unwrap_or_default();
    CallToolResult::success(json)
}
