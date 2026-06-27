use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

pub fn list_scenario_templates_tool() -> ToolDefinition {
    ToolDefinition {
        name: "scenario_templates.list".to_string(),
        description: "List scenario templates for project creation, including workflow, fields, resources, AI roles, governance, suggested connections, and runtime usage_guide fields for operator steps, primary MCP tools, connector kinds, plugin keys, and acceptance focus".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_type_key": {
                    "type": "string",
                    "description": "Optional project type key, for example contract_review"
                },
                "industry": {
                    "type": "string",
                    "description": "Optional industry key, for example legal, operations, quality, delivery, or software"
                }
            }
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ListScenarioTemplatesInput {
    project_type_key: Option<String>,
    industry: Option<String>,
}

pub async fn list_scenario_templates(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ListScenarioTemplatesInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    match client
        .list_scenario_templates(input.project_type_key.as_deref(), input.industry.as_deref())
        .await
    {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

pub fn get_scenario_template_tool() -> ToolDefinition {
    ToolDefinition {
        name: "scenario_templates.get".to_string(),
        description: "Get one scenario template by key for project setup or agent instructions, including runtime usage_guide fields for frontend/REST/MCP entrypoints, operator steps, primary MCP tools, connector kinds, plugin keys, and acceptance focus".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "Scenario template key, for example contract_review_default"
                }
            },
            "required": ["key"]
        }),
    }
}

pub fn install_scenario_template_tool() -> ToolDefinition {
    ToolDefinition {
        name: "scenario_templates.install".to_string(),
        description: "Install a complete multi-form scenario template into an existing project, reusing the project creation scenario bundle path for forms, views, connectors, plugins, governance, AI placeholders, and workflow backfill".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "Existing project id that should receive the scenario template bundle"
                },
                "template_key": {
                    "type": "string",
                    "description": "Scenario template key, for example restaurant_ordering_default"
                }
            },
            "required": ["project_id", "template_key"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct GetScenarioTemplateInput {
    key: String,
}

#[derive(Debug, Deserialize)]
struct InstallScenarioTemplateInput {
    project_id: String,
    template_key: String,
}

pub async fn get_scenario_template(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: GetScenarioTemplateInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    match client.get_scenario_template(&input.key).await {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn install_scenario_template(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: InstallScenarioTemplateInput = match serde_json::from_value(args) {
        Ok(value) => value,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };

    match client
        .install_scenario_template(&input.project_id, &input.template_key)
        .await
    {
        Ok(value) => CallToolResult::success(serde_json::to_string_pretty(&value).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}

#[cfg(test)]
mod tests {
    use super::{get_scenario_template_tool, install_scenario_template_tool, list_scenario_templates_tool};

    #[test]
    fn scenario_template_tools_have_expected_names() {
        assert_eq!(list_scenario_templates_tool().name, "scenario_templates.list");
        assert_eq!(get_scenario_template_tool().name, "scenario_templates.get");
        assert_eq!(install_scenario_template_tool().name, "scenario_templates.install");
    }

    #[test]
    fn scenario_template_tools_document_usage_guide_contract() {
        let list_tool = list_scenario_templates_tool();
        assert!(list_tool.description.contains("usage_guide"));
        assert!(list_tool.description.contains("connector kinds"));
        let get_tool = get_scenario_template_tool();
        assert!(get_tool.description.contains("frontend/REST/MCP entrypoints"));
        assert!(get_tool.description.contains("acceptance focus"));
        let install_tool = install_scenario_template_tool();
        assert!(install_tool.description.contains("multi-form"));
        assert!(install_tool.description.contains("existing project"));
    }
}
