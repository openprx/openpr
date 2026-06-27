use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

pub fn list_proposals_tool() -> ToolDefinition {
    ToolDefinition {
        name: "proposals.list".to_string(),
        description: "List proposals for a project, optionally filtered by status".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "UUID of the project",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
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
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    match client.list_proposals(&input.project_id, input.status.as_deref()).await {
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
                    "description": "Proposal ID",
                    "pattern": "^(PROP-)?[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"
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
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
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
                    "description": "UUID of the project",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
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
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
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

pub fn create_check_result_tool() -> ToolDefinition {
    ToolDefinition {
        name: "check_results.create".to_string(),
        description: "Record a governed MCP/AI check result for a high-risk action".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string", "description": "Project UUID" },
                "invocation_id": { "type": "string", "description": "Optional invocation UUID" },
                "connector_id": { "type": "string", "description": "Optional connector UUID" },
                "action_class": {
                    "type": "string",
                    "description": "read_only, comment_result, low_risk_mutation, high_risk_mutation, external_side_effect, or financial_legal_compliance"
                },
                "risk_level": {
                    "type": "string",
                    "description": "low, medium, high, or critical"
                },
                "title": { "type": "string" },
                "summary": { "type": "string" },
                "result": { "type": "object" }
            },
            "required": ["project_id", "title", "summary"]
        }),
    }
}

pub fn create_proposal_from_result_tool() -> ToolDefinition {
    ToolDefinition {
        name: "proposals.create_from_result".to_string(),
        description: "Create a governance proposal from a check result instead of directly applying a high-risk action"
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "check_result_id": { "type": "string", "description": "Check result UUID" },
                "title": { "type": "string" },
                "proposal_type": {
                    "type": "string",
                    "description": "feature, architecture, priority, resource, governance, or bugfix"
                },
                "content": { "type": "string" },
                "domains": { "type": "array", "items": { "type": "string" } },
                "voting_rule": {
                    "type": "string",
                    "description": "simple_majority, absolute_majority, or consensus"
                },
                "cycle_template": {
                    "type": "string",
                    "description": "rapid, fast, standard, or critical"
                },
                "submit": {
                    "type": "boolean",
                    "description": "If true, submit the proposal immediately as open; otherwise keep it draft"
                }
            },
            "required": ["check_result_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct CreateCheckResultInput {
    project_id: String,
    invocation_id: Option<String>,
    connector_id: Option<String>,
    action_class: Option<String>,
    risk_level: Option<String>,
    title: String,
    summary: String,
    result: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct CreateProposalFromResultInput {
    check_result_id: String,
    title: Option<String>,
    proposal_type: Option<String>,
    content: Option<String>,
    domains: Option<Vec<String>>,
    voting_rule: Option<String>,
    cycle_template: Option<String>,
    submit: Option<bool>,
}

pub async fn create_check_result(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: CreateCheckResultInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    let body = json!({
        "invocation_id": input.invocation_id,
        "connector_id": input.connector_id,
        "action_class": input.action_class.unwrap_or_else(|| "high_risk_mutation".to_string()),
        "risk_level": input.risk_level.unwrap_or_else(|| "high".to_string()),
        "title": input.title,
        "summary": input.summary,
        "result": input.result.unwrap_or_else(|| json!({}))
    });

    match client.create_check_result(&input.project_id, body).await {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(e) => CallToolResult::error(e),
    }
}

pub async fn create_proposal_from_result(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: CreateProposalFromResultInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    let body = json!({
        "title": input.title,
        "proposal_type": input.proposal_type,
        "content": input.content,
        "domains": input.domains,
        "voting_rule": input.voting_rule,
        "cycle_template": input.cycle_template,
        "submit": input.submit
    });

    match client.create_proposal_from_result(&input.check_result_id, body).await {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(e) => CallToolResult::error(e),
    }
}
