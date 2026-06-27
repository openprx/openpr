use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

pub fn code_resources_list_tool() -> ToolDefinition {
    ToolDefinition {
        name: "code.resources.list".to_string(),
        description: "List code-oriented project resources such as repositories and directories".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string", "description": "Project UUID" }
            },
            "required": ["project_id"]
        }),
    }
}

pub fn code_directory_get_tool() -> ToolDefinition {
    ToolDefinition {
        name: "code.directory.get".to_string(),
        description: "Read a directory resource locator and access policy from project context".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string", "description": "Project UUID" },
                "resource_id": { "type": "string", "description": "Optional project resource UUID" },
                "name": { "type": "string", "description": "Optional directory resource name" }
            },
            "required": ["project_id"]
        }),
    }
}

pub fn code_task_context_get_tool() -> ToolDefinition {
    ToolDefinition {
        name: "code.task_context.get".to_string(),
        description: "Get project context plus optional work-item context for a code task".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string", "description": "Project UUID" },
                "work_item_id": { "type": "string", "description": "Optional work item UUID" }
            },
            "required": ["project_id"]
        }),
    }
}

pub fn code_change_proposal_create_tool() -> ToolDefinition {
    ToolDefinition {
        name: "code.change_proposal.create".to_string(),
        description: "Record a governed code change proposal as a check result instead of directly mutating code"
            .to_string(),
        input_schema: governed_result_schema(
            "Code change proposal title",
            "Code change summary, expected impact, and validation notes",
        ),
    }
}

pub fn documents_extract_summary_tool() -> ToolDefinition {
    ToolDefinition {
        name: "documents.extract_summary".to_string(),
        description: "Record a document summary as a governed AI check result".to_string(),
        input_schema: governed_result_schema("Document summary title", "Extracted document summary"),
    }
}

pub fn documents_review_risk_tool() -> ToolDefinition {
    ToolDefinition {
        name: "documents.review_risk".to_string(),
        description: "Record document risk review findings as a high-risk governed check result".to_string(),
        input_schema: governed_result_schema("Risk review title", "Risk review summary"),
    }
}

pub fn approval_request_tool() -> ToolDefinition {
    ToolDefinition {
        name: "approval.request".to_string(),
        description: "Create a governed approval request check result for human decision flow".to_string(),
        input_schema: governed_result_schema("Approval request title", "Approval request summary"),
    }
}

pub fn inspection_report_tool() -> ToolDefinition {
    ToolDefinition {
        name: "inspection.report".to_string(),
        description: "Record an inspection report as a governed check result".to_string(),
        input_schema: governed_result_schema("Inspection report title", "Inspection findings summary"),
    }
}

pub fn corrective_action_propose_tool() -> ToolDefinition {
    ToolDefinition {
        name: "corrective_action.propose".to_string(),
        description: "Record a corrective-action proposal as a governed check result".to_string(),
        input_schema: governed_result_schema("Corrective action title", "Corrective action summary"),
    }
}

#[derive(Debug, Deserialize)]
struct ProjectInput {
    project_id: String,
}

#[derive(Debug, Deserialize)]
struct DirectoryInput {
    project_id: String,
    resource_id: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodeTaskContextInput {
    project_id: String,
    work_item_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GovernedResultInput {
    project_id: String,
    invocation_id: Option<String>,
    connector_id: Option<String>,
    title: String,
    summary: String,
    result: Option<Value>,
    risk_level: Option<String>,
}

pub async fn code_resources_list(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ProjectInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    let resources = match client.list_project_resources(&input.project_id).await {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(err),
    };

    CallToolResult::success(serde_json::to_string_pretty(&filter_code_resources(resources)).unwrap_or_default())
}

pub async fn code_directory_get(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: DirectoryInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    let resources = match client.list_project_resources(&input.project_id).await {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(err),
    };
    let Some(resource) = find_directory_resource(&resources, input.resource_id.as_deref(), input.name.as_deref())
    else {
        return CallToolResult::error("directory resource not found");
    };

    CallToolResult::success(serde_json::to_string_pretty(&resource).unwrap_or_default())
}

pub async fn code_task_context_get(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: CodeTaskContextInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    let project_context = match client.get_project_context(&input.project_id).await {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(err),
    };
    let work_item = match input.work_item_id {
        Some(work_item_id) => match client.get_work_item(&work_item_id).await {
            Ok(value) => Some(value),
            Err(err) => return CallToolResult::error(err),
        },
        None => None,
    };

    CallToolResult::success(
        serde_json::to_string_pretty(&json!({
            "project_context": project_context,
            "work_item": work_item
        }))
        .unwrap_or_default(),
    )
}

pub async fn code_change_proposal_create(client: &OpenPrClient, args: Value) -> CallToolResult {
    create_governed_result(client, args, "high_risk_mutation", "high", "code.change_proposal").await
}

pub async fn documents_extract_summary(client: &OpenPrClient, args: Value) -> CallToolResult {
    create_governed_result(client, args, "read_only", "low", "documents.extract_summary").await
}

pub async fn documents_review_risk(client: &OpenPrClient, args: Value) -> CallToolResult {
    create_governed_result(
        client,
        args,
        "financial_legal_compliance",
        "high",
        "documents.review_risk",
    )
    .await
}

pub async fn approval_request(client: &OpenPrClient, args: Value) -> CallToolResult {
    create_governed_result(client, args, "high_risk_mutation", "high", "approval.request").await
}

pub async fn inspection_report(client: &OpenPrClient, args: Value) -> CallToolResult {
    create_governed_result(client, args, "comment_result", "medium", "inspection.report").await
}

pub async fn corrective_action_propose(client: &OpenPrClient, args: Value) -> CallToolResult {
    create_governed_result(client, args, "high_risk_mutation", "high", "corrective_action.propose").await
}

async fn create_governed_result(
    client: &OpenPrClient,
    args: Value,
    default_action_class: &str,
    default_risk_level: &str,
    scenario_tool: &str,
) -> CallToolResult {
    let input: GovernedResultInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    let body = json!({
        "invocation_id": input.invocation_id,
        "connector_id": input.connector_id,
        "action_class": default_action_class,
        "risk_level": input.risk_level.unwrap_or_else(|| default_risk_level.to_string()),
        "title": input.title,
        "summary": input.summary,
        "result": {
            "scenario_tool": scenario_tool,
            "payload": input.result.unwrap_or_else(|| json!({}))
        }
    });

    match client.create_check_result(&input.project_id, body).await {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

fn governed_result_schema(title_description: &str, summary_description: &str) -> Value {
    json!({
        "type": "object",
        "properties": {
            "project_id": { "type": "string", "description": "Project UUID" },
            "invocation_id": { "type": "string", "description": "Optional invocation UUID" },
            "connector_id": { "type": "string", "description": "Optional connector UUID" },
            "title": { "type": "string", "description": title_description },
            "summary": { "type": "string", "description": summary_description },
            "risk_level": { "type": "string", "description": "Optional override: low, medium, high, or critical" },
            "result": { "type": "object", "description": "Structured AI result payload" }
        },
        "required": ["project_id", "title", "summary"]
    })
}

fn filter_code_resources(value: Value) -> Value {
    map_resources(value, |resource| {
        matches!(resource.get("kind").and_then(Value::as_str), Some("repo" | "directory"))
    })
}

fn find_directory_resource(value: &Value, resource_id: Option<&str>, name: Option<&str>) -> Option<Value> {
    resource_items(value)
        .into_iter()
        .find(|resource| {
            let is_directory = matches!(resource.get("kind").and_then(Value::as_str), Some("directory" | "repo"));
            let id_matches = resource_id.is_none_or(|id| resource.get("id").and_then(Value::as_str) == Some(id));
            let name_matches =
                name.is_none_or(|expected| resource.get("name").and_then(Value::as_str) == Some(expected));
            is_directory && id_matches && name_matches
        })
        .cloned()
}

fn map_resources(value: Value, predicate: impl Fn(&Value) -> bool) -> Value {
    if let Some(items) = value
        .get("data")
        .and_then(|data| data.get("items"))
        .and_then(Value::as_array)
    {
        let filtered = items.iter().filter(|item| predicate(item)).cloned().collect();
        let mut output = value;
        if let Some(data) = output.get_mut("data").and_then(Value::as_object_mut) {
            data.insert("items".to_string(), Value::Array(filtered));
        }
        return output;
    }

    if let Some(items) = value.get("data").and_then(Value::as_array) {
        let filtered = items.iter().filter(|item| predicate(item)).cloned().collect();
        let mut output = value;
        if let Some(data) = output.get_mut("data") {
            *data = Value::Array(filtered);
        }
        return output;
    }

    if let Some(items) = value.as_array() {
        return Value::Array(items.iter().filter(|item| predicate(item)).cloned().collect());
    }

    value
}

fn resource_items(value: &Value) -> Vec<&Value> {
    value
        .get("data")
        .and_then(|data| data.get("items"))
        .and_then(Value::as_array)
        .or_else(|| value.get("data").and_then(Value::as_array))
        .or_else(|| value.as_array())
        .map(|items| items.iter().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{filter_code_resources, find_directory_resource};
    use serde_json::json;

    #[test]
    fn code_resource_filter_keeps_repo_and_directory_only() {
        let filtered = filter_code_resources(json!({
            "data": {
                "items": [
                    { "id": "1", "kind": "repo" },
                    { "id": "2", "kind": "directory" },
                    { "id": "3", "kind": "equipment" }
                ]
            }
        }));

        assert_eq!(
            filtered
                .get("data")
                .and_then(|data| data.get("items"))
                .and_then(|items| items.as_array())
                .expect("items")
                .len(),
            2
        );
    }

    #[test]
    fn directory_lookup_accepts_name_or_id() {
        let resources = json!({
            "data": [
                { "id": "r1", "kind": "directory", "name": "src" },
                { "id": "r2", "kind": "equipment", "name": "src" }
            ]
        });

        assert_eq!(
            find_directory_resource(&resources, None, Some("src"))
                .and_then(|value| value.get("id").and_then(|id| id.as_str()).map(str::to_string))
                .as_deref(),
            Some("r1")
        );
        assert!(find_directory_resource(&resources, Some("r2"), None).is_none());
    }
}
