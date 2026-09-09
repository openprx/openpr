//! `flow.feature_get` / `flow.feature_set` — the Flow workspace rollout flag
//! (`flow_workspace_settings`, `GET|PUT /workspaces/{workspace_id}/features/flow`).
//!
//! `flow.feature_get` is `PolicyScope::WorkspaceWide` (any workspace member/bot may read
//! whether Flow is enabled and what it defaults to). `flow.feature_set` is
//! `PolicyScope::WorkspaceWideAdmin`: it changes rollout state and, when it edits
//! `default_member_level`, is a workspace-wide permission change
//! (`v0.4-flow-alpha.md`/`mcp-surface-v1.md`), so the underlying REST endpoint requires a
//! workspace admin principal — the MCP layer does not (and, per `mcp-surface-v1.md`, must not)
//! relax that; see `server.rs`'s `PolicyScope::WorkspaceWideAdmin` doc comment for exactly what
//! is and is not enforced at this layer today.

use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

fn parse_input<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, CallToolResult> {
    serde_json::from_value(args).map_err(|err| CallToolResult::error(format!("Invalid input: {err}")))
}

fn respond_data(result: Result<Value, String>) -> CallToolResult {
    match result {
        Ok(value) => CallToolResult::success(
            serde_json::to_string_pretty(value.get("data").unwrap_or(&Value::Null)).unwrap_or_default(),
        ),
        Err(error) => CallToolResult::error(error),
    }
}

const MEMBER_LEVELS: [&str; 4] = ["full_access", "edit", "comment", "view"];

pub fn get_flow_feature_tool() -> ToolDefinition {
    ToolDefinition {
        name: "flow.feature_get".to_string(),
        description: "Read the Flow rollout flag for a workspace: flow_enabled, \
default_member_level, authz_epoch, and who last changed it."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "workspace_id": { "type": "string", "description": "Workspace UUID" }
            },
            "required": ["workspace_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct GetFlowFeatureInput {
    workspace_id: String,
}

pub async fn get_flow_feature(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: GetFlowFeatureInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    respond_data(client.get_flow_feature(&input.workspace_id).await)
}

pub fn set_flow_feature_tool() -> ToolDefinition {
    ToolDefinition {
        name: "flow.feature_set".to_string(),
        description: "Change the Flow rollout flag for a workspace: enable/disable Flow, or \
change default_member_level. Requires a workspace admin principal; at least one of enabled or \
default_member_level must be supplied. default_member_level only accepts its current default \
('edit') before the v0.5 authorization surface ships; the API is the authority on that rule."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "workspace_id": { "type": "string", "description": "Workspace UUID" },
                "enabled": { "type": "boolean" },
                "default_member_level": { "type": "string", "enum": ["full_access", "edit", "comment", "view"] },
                "idempotency_key": { "type": "string" }
            },
            "required": ["workspace_id", "idempotency_key"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct SetFlowFeatureInput {
    workspace_id: String,
    enabled: Option<bool>,
    default_member_level: Option<String>,
    idempotency_key: String,
}

pub async fn set_flow_feature(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: SetFlowFeatureInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };

    if input.enabled.is_none() && input.default_member_level.is_none() {
        return CallToolResult::error("at least one of enabled or default_member_level must be supplied".to_string());
    }
    if let Some(level) = input.default_member_level.as_deref()
        && !MEMBER_LEVELS.contains(&level)
    {
        return CallToolResult::error(format!(
            "default_member_level must be one of {}",
            MEMBER_LEVELS.join(", ")
        ));
    }
    if input.idempotency_key.trim().is_empty() {
        return CallToolResult::error("idempotency_key must not be empty".to_string());
    }

    let mut body = json!({ "idempotency_key": input.idempotency_key });
    if let Some(object) = body.as_object_mut() {
        if let Some(enabled) = input.enabled {
            object.insert("enabled".to_string(), json!(enabled));
        }
        if let Some(level) = input.default_member_level.as_deref() {
            object.insert("default_member_level".to_string(), json!(level));
        }
    }

    respond_data(client.set_flow_feature(&input.workspace_id, body).await)
}

#[cfg(test)]
mod tests {
    use super::{get_flow_feature, set_flow_feature};
    use crate::client::test_api;
    use axum::{Json, Router, routing::get};
    use serde_json::json;

    #[tokio::test]
    async fn feature_get_rejects_missing_workspace_id() -> Result<(), String> {
        let client = test_api::client("http://127.0.0.1:1".to_string())?;
        let result = get_flow_feature(&client, json!({})).await;
        assert_eq!(result.is_error, Some(true));
        Ok(())
    }

    #[tokio::test]
    async fn feature_set_rejects_when_both_fields_absent() -> Result<(), String> {
        let client = test_api::client("http://127.0.0.1:1".to_string())?;
        let result = set_flow_feature(
            &client,
            json!({
                "workspace_id": "11111111-1111-4111-8111-111111111111",
                "idempotency_key": "key-1"
            }),
        )
        .await;
        assert_eq!(result.is_error, Some(true));
        Ok(())
    }

    #[tokio::test]
    async fn feature_set_rejects_unknown_member_level() -> Result<(), String> {
        let client = test_api::client("http://127.0.0.1:1".to_string())?;
        let result = set_flow_feature(
            &client,
            json!({
                "workspace_id": "11111111-1111-4111-8111-111111111111",
                "default_member_level": "owner",
                "idempotency_key": "key-1"
            }),
        )
        .await;
        assert_eq!(result.is_error, Some(true));
        Ok(())
    }

    #[tokio::test]
    async fn feature_set_rejects_missing_idempotency_key() -> Result<(), String> {
        let client = test_api::client("http://127.0.0.1:1".to_string())?;
        let result = set_flow_feature(
            &client,
            json!({ "workspace_id": "11111111-1111-4111-8111-111111111111", "enabled": true }),
        )
        .await;
        assert_eq!(result.is_error, Some(true));
        Ok(())
    }

    #[tokio::test]
    async fn feature_tools_return_rest_data_without_the_envelope() -> Result<(), Box<dyn std::error::Error>> {
        let response = || async { Json(json!({"code": 0, "message": "ok", "data": {"shape": "semantic-data"}})) };
        let router = Router::new().route(
            "/api/v1/workspaces/{workspace_id}/features/flow",
            get(response).put(response),
        );
        let base_url = test_api::spawn(router).await?;
        let client = test_api::client(base_url)?;
        let calls = [
            get_flow_feature(&client, json!({"workspace_id": "workspace"})).await,
            set_flow_feature(
                &client,
                json!({"workspace_id": "workspace", "enabled": true, "idempotency_key": "key"}),
            )
            .await,
        ];
        for result in calls {
            assert_ne!(result.is_error, Some(true), "Flow feature call failed: {result:?}");
            let Some(crate::protocol::ToolContent::Text { text }) = result.content.first() else {
                return Err("missing MCP text content".into());
            };
            let output: serde_json::Value = serde_json::from_str(text)?;
            assert_eq!(output, json!({"shape": "semantic-data"}));
        }
        Ok(())
    }
}
