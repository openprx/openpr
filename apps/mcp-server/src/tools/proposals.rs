use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::json;

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

pub async fn list_proposals(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: ListProposalsInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client
        .list_proposals(&input.project_id, input.status.as_deref())
        .await
    {
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

pub async fn get_proposal(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: GetProposalInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    match client.get_proposal(&input.proposal_id).await {
        Ok(proposal) => {
            let json = serde_json::to_string_pretty(&proposal).unwrap_or_default();
            CallToolResult::success(json)
        }
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

pub async fn create_proposal(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: CreateProposalInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let body = json!({
        "title": input.title,
        "description": input.description,
        "project_id": input.project_id
    });

    match client.create_proposal(body).await {
        Ok(proposal) => {
            let json = serde_json::to_string_pretty(&proposal).unwrap_or_default();
            CallToolResult::success(json)
        }
        Err(e) => CallToolResult::error(e),
    }
}
