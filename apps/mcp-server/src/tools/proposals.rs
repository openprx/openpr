use crate::db;
use crate::protocol::{CallToolResult, ToolDefinition};
use platform::app::AppState;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

pub fn list_proposals_tool() -> ToolDefinition {
    ToolDefinition {
        name: "proposals.list".to_string(),
        description: "List proposals for a project, optionally filtered by status".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "UUID of the project"
                },
                "status": {
                    "type": "string",
                    "description": "Proposal status filter (optional)"
                }
            },
            "required": ["project_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ListProposalsInput {
    project_id: String,
    status: Option<String>,
}

pub async fn list_proposals(state: &AppState, args: serde_json::Value) -> CallToolResult {
    let input: ListProposalsInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let project_id = match Uuid::parse_str(&input.project_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid project_id format"),
    };

    match db::proposals::list_proposals(state, project_id, input.status).await {
        Ok(proposals) => {
            let json = serde_json::to_string_pretty(&proposals).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}

pub fn get_proposal_tool() -> ToolDefinition {
    ToolDefinition {
        name: "proposals.get".to_string(),
        description: "Get details of a specific proposal".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "proposal_id": {
                    "type": "string",
                    "description": "Proposal ID"
                }
            },
            "required": ["proposal_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct GetProposalInput {
    proposal_id: String,
}

pub async fn get_proposal(state: &AppState, args: serde_json::Value) -> CallToolResult {
    let input: GetProposalInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match db::proposals::get_proposal(state, input.proposal_id).await {
        Ok(Some(proposal)) => {
            let json = serde_json::to_string_pretty(&proposal).unwrap_or_default();
            CallToolResult::success(json)
        }
        Ok(None) => CallToolResult::error("Proposal not found"),
        Err(e) => CallToolResult::error(e),
    }
}

pub fn create_proposal_tool() -> ToolDefinition {
    ToolDefinition {
        name: "proposals.create".to_string(),
        description: "Create a new proposal".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Proposal title"
                },
                "description": {
                    "type": "string",
                    "description": "Proposal description"
                },
                "project_id": {
                    "type": "string",
                    "description": "UUID of the project"
                }
            },
            "required": ["title", "description", "project_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct CreateProposalInput {
    title: String,
    description: String,
    project_id: String,
}

pub async fn create_proposal(state: &AppState, args: serde_json::Value) -> CallToolResult {
    let input: CreateProposalInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let project_id = match Uuid::parse_str(&input.project_id) {
        Ok(id) => id,
        Err(_) => return CallToolResult::error("Invalid project_id format"),
    };

    let author_id = state.cfg.default_author_id;

    match db::proposals::create_proposal(
        state,
        project_id,
        input.title,
        input.description,
        author_id,
    )
    .await
    {
        Ok(proposal) => {
            let json = serde_json::to_string_pretty(&proposal).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}
