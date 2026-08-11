use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

fn parse_input<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, CallToolResult> {
    serde_json::from_value(args).map_err(|err| CallToolResult::error(format!("Invalid input: {err}")))
}

pub fn list_plugins_tool() -> ToolDefinition {
    ToolDefinition {
        name: "plugins.list".to_string(),
        description: "List WASM plugins installed in a project. Paginated: per_page defaults to 50 (max 200)."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string", "description": "Project UUID" },
                "page": { "type": "integer", "minimum": 1, "description": "1-based page number (default 1)" },
                "per_page": { "type": "integer", "minimum": 1, "maximum": 200, "description": "Page size, default 50, clamped to 200" }
            },
            "required": ["project_id"]
        }),
    }
}

pub fn get_plugin_tool() -> ToolDefinition {
    ToolDefinition {
        name: "plugins.get".to_string(),
        description: "Get one installed WASM plugin and its manifest".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "plugin_id": { "type": "string", "description": "Plugin UUID" }
            },
            "required": ["plugin_id"]
        }),
    }
}

pub fn install_plugin_tool() -> ToolDefinition {
    ToolDefinition {
        name: "plugins.install".to_string(),
        description: "Install a project-scoped WASM plugin. wasm_base64 is optional for manifest-only registration."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "project_id": { "type": "string" },
                "manifest": { "type": "object" },
                "wasm_base64": { "type": "string" },
                "status": { "type": "string", "enum": ["active", "disabled", "failed"] }
            },
            "required": ["project_id", "manifest"]
        }),
    }
}

pub fn invoke_plugin_tool() -> ToolDefinition {
    ToolDefinition {
        name: "plugins.invoke".to_string(),
        description: "Invoke a WASM plugin hook or plugin-provided tool declared in the manifest".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "plugin_id": { "type": "string" },
                "hook_kind": {
                    "type": "string",
                    "description": "Hook kind such as field_validator/formula/event_handler or a manifest tool name"
                },
                "input": { "type": "object" }
            },
            "required": ["plugin_id", "hook_kind"]
        }),
    }
}

pub fn list_plugin_invocations_tool() -> ToolDefinition {
    ToolDefinition {
        name: "plugin_invocations.list".to_string(),
        description: "List invocation logs for one WASM plugin, newest first. Paginated: per_page defaults to \
50 (max 200)."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "plugin_id": { "type": "string" },
                "page": { "type": "integer", "minimum": 1, "description": "1-based page number (default 1)" },
                "per_page": { "type": "integer", "minimum": 1, "maximum": 200, "description": "Page size, default 50, clamped to 200" }
            },
            "required": ["plugin_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ListPluginsInput {
    project_id: String,
    page: Option<u32>,
    per_page: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct PluginInput {
    plugin_id: String,
}

#[derive(Debug, Deserialize)]
struct ListPluginInvocationsInput {
    plugin_id: String,
    page: Option<u32>,
    per_page: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct InstallPluginInput {
    project_id: String,
    manifest: Value,
    wasm_base64: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InvokePluginInput {
    plugin_id: String,
    hook_kind: String,
    input: Option<Value>,
}

pub async fn list_plugins(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ListPluginsInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client.list_plugins(&input.project_id, input.page, input.per_page).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn get_plugin(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: PluginInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client.get_plugin(&input.plugin_id).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn install_plugin(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: InstallPluginInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    let body = json!({
        "manifest": input.manifest,
        "wasm_base64": input.wasm_base64,
        "status": input.status
    });
    match client.install_plugin(&input.project_id, body).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn invoke_plugin(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: InvokePluginInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    let body = json!({
        "hook_kind": input.hook_kind,
        "input": input.input.unwrap_or_else(|| json!({}))
    });
    match client.invoke_plugin(&input.plugin_id, body).await {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}

pub async fn list_plugin_invocations(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ListPluginInvocationsInput = match parse_input(args) {
        Ok(value) => value,
        Err(err) => return err,
    };
    match client
        .list_plugin_invocations(&input.plugin_id, input.page, input.per_page)
        .await
    {
        Ok(value) => CallToolResult::success(pretty(&value)),
        Err(err) => CallToolResult::error(err),
    }
}
