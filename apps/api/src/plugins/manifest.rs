use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::forms::schema::normalize_key;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PluginManifest {
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
    pub key: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub capabilities: PluginCapabilities,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PluginCapabilities {
    #[serde(default)]
    pub hooks: Vec<PluginHook>,
    #[serde(default)]
    pub tools: Vec<PluginTool>,
    #[serde(default)]
    pub runtime: PluginRuntimePolicy,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PluginHook {
    pub kind: String,
    #[serde(default)]
    pub form_key: Option<String>,
    #[serde(default)]
    pub field_key: Option<String>,
    #[serde(default)]
    pub event_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PluginTool {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub input_schema: Value,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PluginRuntimePolicy {
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_fuel")]
    pub fuel: u64,
    #[serde(default = "default_memory_bytes")]
    pub memory_bytes: usize,
}

impl Default for PluginRuntimePolicy {
    fn default() -> Self {
        Self {
            timeout_ms: default_timeout_ms(),
            fuel: default_fuel(),
            memory_bytes: default_memory_bytes(),
        }
    }
}

const fn default_timeout_ms() -> u64 {
    1_000
}

const fn default_fuel() -> u64 {
    5_000_000
}

const fn default_memory_bytes() -> usize {
    16 * 1024 * 1024
}

fn default_schema_version() -> String {
    "openpr.plugin.v1".to_string()
}

pub fn parse_manifest(value: &Value) -> Result<PluginManifest, String> {
    let manifest: PluginManifest =
        serde_json::from_value(value.clone()).map_err(|err| format!("invalid plugin manifest: {err}"))?;
    validate_manifest(manifest)
}

fn validate_manifest(mut manifest: PluginManifest) -> Result<PluginManifest, String> {
    if manifest.schema_version != "openpr.plugin.v1" {
        return Err("unsupported plugin manifest schema_version".to_string());
    }
    manifest.key = normalize_key(&manifest.key)?;
    if manifest.name.trim().is_empty() {
        return Err("plugin name is required".to_string());
    }
    manifest.name = manifest.name.trim().to_string();
    if manifest.version.trim().is_empty() {
        return Err("plugin version is required".to_string());
    }
    manifest.version = manifest.version.trim().to_string();
    manifest.description = manifest.description.trim().to_string();

    for hook in &mut manifest.capabilities.hooks {
        validate_hook(hook)?;
    }
    for tool in &mut manifest.capabilities.tools {
        validate_tool(&manifest.key, tool)?;
    }
    if manifest.capabilities.runtime.timeout_ms == 0 || manifest.capabilities.runtime.timeout_ms > 30_000 {
        return Err("runtime.timeout_ms must be between 1 and 30000".to_string());
    }
    if manifest.capabilities.runtime.fuel == 0 || manifest.capabilities.runtime.fuel > 1_000_000_000 {
        return Err("runtime.fuel must be between 1 and 1000000000".to_string());
    }
    if manifest.capabilities.runtime.memory_bytes < 64 * 1024
        || manifest.capabilities.runtime.memory_bytes > 128 * 1024 * 1024
    {
        return Err("runtime.memory_bytes must be between 64KiB and 128MiB".to_string());
    }

    Ok(manifest)
}

fn validate_hook(hook: &mut PluginHook) -> Result<(), String> {
    match hook.kind.as_str() {
        "field_validator" | "formula" | "event_handler" => {}
        _ => return Err(format!("unsupported plugin hook kind: {}", hook.kind)),
    }
    if let Some(form_key) = hook.form_key.as_deref() {
        hook.form_key = Some(normalize_key(form_key)?);
    }
    if let Some(field_key) = hook.field_key.as_deref() {
        hook.field_key = Some(normalize_key(field_key)?);
    }
    if hook.kind == "event_handler"
        && hook
            .event_type
            .as_deref()
            .map(str::trim)
            .as_ref()
            .is_none_or(|value| value.is_empty())
    {
        return Err("event_handler hook requires event_type".to_string());
    }
    hook.event_type = hook
        .event_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    Ok(())
}

fn validate_tool(plugin_key: &str, tool: &mut PluginTool) -> Result<(), String> {
    let name = tool.name.trim();
    if name.is_empty() {
        return Err("plugin tool name is required".to_string());
    }
    if !name.starts_with(plugin_key) || !name.contains('.') {
        return Err("plugin tool name must be namespaced as <plugin_key>.<tool>".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
    {
        return Err("plugin tool name must contain lowercase letters, digits, underscores, or dots".to_string());
    }
    tool.name = name.to_string();
    tool.description = tool.description.trim().to_string();
    if !tool.input_schema.is_object() {
        tool.input_schema = serde_json::json!({"type": "object"});
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_manifest;
    use serde_json::json;

    #[test]
    fn parses_manifest_with_hooks_tools_and_runtime_policy() {
        let manifest = parse_manifest(&json!({
            "schema_version": "openpr.plugin.v1",
            "key": "restaurant_calc",
            "name": "Restaurant Calc",
            "version": "0.1.0",
            "capabilities": {
                "hooks": [
                    {"kind": "field_validator", "form_key": "order", "field_key": "total_amount"},
                    {"kind": "formula", "form_key": "order_line", "field_key": "line_total"},
                    {"kind": "event_handler", "event_type": "form.record.created"}
                ],
                "tools": [
                    {"name": "restaurant_calc.today_revenue", "description": "Revenue"}
                ],
                "runtime": {"timeout_ms": 500, "fuel": 100_000, "memory_bytes": 1_048_576}
            }
        }))
        .expect("manifest should parse");

        assert_eq!(manifest.key, "restaurant_calc");
        assert_eq!(manifest.capabilities.hooks.len(), 3);
        assert_eq!(manifest.capabilities.tools.len(), 1);
        assert_eq!(manifest.capabilities.runtime.timeout_ms, 500);
    }

    #[test]
    fn rejects_unscoped_tool_names() {
        let err = parse_manifest(&json!({
            "schema_version": "openpr.plugin.v1",
            "key": "risk",
            "name": "Risk",
            "version": "0.1.0",
            "capabilities": {
                "tools": [{"name": "score", "description": "Score"}]
            }
        }))
        .expect_err("tool should be rejected");

        assert!(err.contains("namespaced"));
    }
}
