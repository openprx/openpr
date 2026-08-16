use crate::client::{ListProposalsQuery, OpenPrClient};
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

pub fn list_proposals_tool() -> ToolDefinition {
    ToolDefinition {
        name: "proposals.list".to_string(),
        description: "List governance proposals. The API has no project filter, so results are not scoped to \
project_id; project_id is only used to apply this project's agent policy. Paginated: per_page defaults to the \
API page size, so page through the result set explicitly."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "UUID of the project whose agent policy governs this call. It does NOT filter the returned proposals",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                },
                "status": {
                    "type": "string",
                    "enum": ["draft", "open", "voting", "approved", "rejected", "vetoed", "archived"],
                    "description": "Proposal status filter (optional)"
                },
                "proposal_type": {
                    "type": "string",
                    "enum": ["feature", "architecture", "priority", "resource", "governance", "bugfix"],
                    "description": "Proposal type filter (optional)"
                },
                "domain": {
                    "type": "string",
                    "description": "Decision domain filter (optional)"
                },
                "page": { "type": "integer", "minimum": 1, "description": "1-based page number (optional)" },
                "per_page": { "type": "integer", "minimum": 1, "description": "Page size (optional)" }
            },
            "required": ["project_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ListProposalsInput {
    #[allow(
        dead_code,
        reason = "Only used for project agent-policy scoping; the API list is workspace-wide"
    )]
    project_id: String,
    status: Option<String>,
    proposal_type: Option<String>,
    domain: Option<String>,
    page: Option<u32>,
    per_page: Option<u32>,
}

pub async fn list_proposals(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: ListProposalsInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    let query = ListProposalsQuery {
        status: input.status.as_deref(),
        proposal_type: input.proposal_type.as_deref(),
        domain: input.domain.as_deref(),
        page: input.page,
        per_page: input.per_page,
        sort: None,
    };

    match client.list_proposals(&query).await {
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
        description: "Create a governance proposal in draft state. The project is derived from the issues \
linked to the proposal, not from project_id."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "minLength": 10,
                    "maxLength": 200,
                    "description": "Proposal title, 10 to 200 characters"
                },
                "content": {
                    "type": "string",
                    "minLength": 50,
                    "description": "Proposal body, at least 50 characters"
                },
                "proposal_type": {
                    "type": "string",
                    "enum": ["feature", "architecture", "priority", "resource", "governance", "bugfix"],
                    "description": "Required unless template_id supplies it"
                },
                "domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1,
                    "description": "Decision domains; required unless template_id supplies them"
                },
                "voting_rule": {
                    "type": "string",
                    "enum": ["simple_majority", "absolute_majority", "consensus"],
                    "description": "Defaults to simple_majority"
                },
                "cycle_template": {
                    "type": "string",
                    "enum": ["rapid", "fast", "standard", "critical"],
                    "description": "Defaults to the cycle template of the proposal type"
                },
                "template_id": {
                    "type": "string",
                    "description": "Optional proposal template id supplying title, type, content and domains"
                },
                "project_id": {
                    "type": "string",
                    "description": "UUID of the project whose agent policy governs this call. It is NOT stored on the proposal",
                    "pattern": "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
                }
            },
            "required": ["title", "content", "project_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct CreateProposalInput {
    title: String,
    content: String,
    proposal_type: Option<String>,
    domains: Option<Vec<String>>,
    voting_rule: Option<String>,
    cycle_template: Option<String>,
    template_id: Option<String>,
    #[allow(
        dead_code,
        reason = "Only used for project agent-policy scoping; the API derives the project from linked issues"
    )]
    project_id: String,
}

pub async fn create_proposal(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: CreateProposalInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };

    let body = json!({
        "title": input.title,
        "content": input.content,
        "proposal_type": input.proposal_type,
        "domains": input.domains,
        "voting_rule": input.voting_rule,
        "cycle_template": input.cycle_template,
        "template_id": input.template_id
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
                "action_class": {
                    "type": "string",
                    "enum": ["read_only", "comment_result", "low_risk_mutation", "high_risk_mutation", "external_side_effect", "financial_legal_compliance"]
                },
                "risk_level": {
                    "type": "string",
                    "enum": ["low", "medium", "high", "critical"]
                },
                "title": { "type": "string", "minLength": 10, "maxLength": 200 },
                "summary": { "type": "string", "minLength": 10 },
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
                "title": { "type": "string", "minLength": 10, "maxLength": 200 },
                "proposal_type": {
                    "type": "string",
                    "enum": ["feature", "architecture", "priority", "resource", "governance", "bugfix"]
                },
                "content": { "type": "string", "minLength": 50, "description": "Proposal body, at least 50 characters" },
                "domains": { "type": "array", "items": { "type": "string" } },
                "voting_rule": {
                    "type": "string",
                    "enum": ["simple_majority", "absolute_majority", "consensus"]
                },
                "cycle_template": {
                    "type": "string",
                    "enum": ["rapid", "fast", "standard", "critical"]
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
