use crate::protocol::ToolDefinition;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashSet};

pub fn filter_tool_definitions(
    tools: Vec<ToolDefinition>,
    enabled_tool_names: &BTreeSet<String>,
) -> Vec<ToolDefinition> {
    tools
        .into_iter()
        .filter(|tool| enabled_tool_names.contains(&tool.name))
        .collect()
}

pub fn extract_enabled_tool_names(policy_response: &Value) -> Option<BTreeSet<String>> {
    let enabled_tools = policy_response
        .get("data")
        .and_then(|data| data.get("mcp"))
        .and_then(|mcp| mcp.get("tool_registry"))
        .and_then(|registry| registry.get("enabled_tools"))?
        .as_array()?;

    Some(
        enabled_tools
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
    )
}

pub fn unknown_tool_names(tools: &[ToolDefinition], enabled_tool_names: &BTreeSet<String>) -> Vec<String> {
    let known = tools.iter().map(|tool| tool.name.as_str()).collect::<HashSet<_>>();
    enabled_tool_names
        .iter()
        .filter(|name| !known.contains(name.as_str()))
        .cloned()
        .collect()
}

pub fn extract_connector_tool_definitions(policy_response: &Value) -> Vec<ToolDefinition> {
    connector_tools(policy_response)
        .iter()
        .filter_map(connector_tool_definition)
        .collect()
}

pub fn find_connector_tool(policy_response: &Value, tool_name: &str) -> Option<Value> {
    connector_tools(policy_response).into_iter().find(|tool| {
        tool.get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| name == tool_name)
    })
}

fn connector_tools(policy_response: &Value) -> Vec<Value> {
    policy_response
        .get("data")
        .and_then(|data| data.get("mcp"))
        .and_then(|mcp| mcp.get("tool_registry"))
        .and_then(|registry| registry.get("connector_tools"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn connector_tool_definition(tool: &Value) -> Option<ToolDefinition> {
    let name = tool.get("name")?.as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    let description = tool
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("Connector-provided MCP tool")
        .to_string();
    let payload_schema = tool
        .get("input_schema")
        .or_else(|| tool.get("inputSchema"))
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({ "type": "object" }));

    Some(ToolDefinition {
        name: name.to_string(),
        description,
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "Project UUID"
                },
                "payload": payload_schema
            },
            "required": ["project_id"]
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        extract_connector_tool_definitions, extract_enabled_tool_names, filter_tool_definitions, find_connector_tool,
        unknown_tool_names,
    };
    use crate::protocol::ToolDefinition;
    use serde_json::json;
    use std::collections::BTreeSet;

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            input_schema: json!({ "type": "object" }),
        }
    }

    #[test]
    fn extracts_enabled_tool_names_from_agent_policy_response() {
        let policy = json!({
            "code": 0,
            "data": {
                "mcp": {
                    "tool_registry": {
                        "enabled_tools": ["context.get_project", "work_items.list"]
                    }
                }
            }
        });

        let names = extract_enabled_tool_names(&policy).expect("registry should exist");
        assert!(names.contains("context.get_project"));
        assert!(names.contains("work_items.list"));
    }

    #[test]
    fn filters_tools_by_enabled_names() {
        let tools = vec![tool("context.get_project"), tool("sprints.create")];
        let enabled = BTreeSet::from(["context.get_project".to_string()]);

        let filtered = filter_tool_definitions(tools, &enabled);
        assert_eq!(filtered.len(), 1);
        assert_eq!(
            filtered.first().map(|tool| tool.name.as_str()),
            Some("context.get_project")
        );
    }

    #[test]
    fn reports_registry_tools_missing_from_static_server() {
        let tools = vec![tool("context.get_project")];
        let enabled = BTreeSet::from(["context.get_project".to_string(), "future.tool".to_string()]);

        assert_eq!(unknown_tool_names(&tools, &enabled), vec!["future.tool".to_string()]);
    }

    #[test]
    fn extracts_connector_tool_definitions_from_policy() {
        let policy = json!({
            "code": 0,
            "data": {
                "mcp": {
                    "tool_registry": {
                        "connector_tools": [{
                            "name": "documents.review_risk",
                            "description": "Review document risk",
                            "connector_id": "c1",
                            "input_schema": {
                                "type": "object",
                                "properties": {
                                    "document_id": { "type": "string" }
                                }
                            }
                        }]
                    }
                }
            }
        });

        let tools = extract_connector_tool_definitions(&policy);
        assert_eq!(tools.len(), 1);
        let tool = tools.first().expect("connector tool");
        assert_eq!(tool.name, "documents.review_risk");
        assert_eq!(
            tool.input_schema
                .get("required")
                .and_then(|required| required.as_array())
                .and_then(|required| required.first())
                .and_then(|value| value.as_str()),
            Some("project_id")
        );
        assert_eq!(
            find_connector_tool(&policy, "documents.review_risk").and_then(|tool| tool
                .get("connector_id")
                .and_then(|value| value.as_str())
                .map(str::to_string)),
            Some("c1".to_string())
        );
    }
}
