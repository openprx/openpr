use crate::protocol::ToolDefinition;
use serde_json::Value;
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

#[cfg(test)]
mod tests {
    use super::{extract_enabled_tool_names, filter_tool_definitions, unknown_tool_names};
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
}
