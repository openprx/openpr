//! `legacy_pages.*` — the four ADR-0003 conditional migration tools.
//!
//! ADR-0003 gates these on measured `legacy-pages-import-v1.md` inventory across
//! development/test/target: when the three-environment total is zero, the importer branch is
//! `not_required_zero_inventory` and neither `apps/api`'s admin migration REST endpoints nor a
//! native `sylvode legacy-pages` CLI are required to exist
//! (`cli-surface-v1.md`: "零行分支不要求命令存在"). That is this deployment's measured state
//! (see the workspace's ADR-0003 gate evidence), and none of `apps/api`'s admin legacy-pages
//! routes are wired up.
//!
//! What v0.4 *does* require even on the zero branch is that these four tools stay registered
//! and answer safely (`v0.4-flow-alpha.md`: "MCP schema 仍要注册并安全解释零分支"; task brief:
//! "零行分支不要求发布 native CLI 命令，但 MCP schema 仍要注册并安全解释零分支") — so an agent
//! that discovers them by name never gets a silent `unknown tool`, a fabricated job id, or a
//! network call to an endpoint that does not exist. Every handler below is a fixed,
//! non-networked answer: `inventory` reports the zero count as a normal read (it is the same
//! zero for every workspace on this deployment — a build-time fact, not workspace-scoped
//! secret data), and the three import commands, which cannot do anything meaningful over zero
//! rows, refuse with the stable `not_required_zero_inventory` reason. None of the four ever
//! reaches the network, and none ever returns legacy page body content.
//!
//! `mcp-surface-v1.md` scopes all four `WorkspaceWide(admin)` — see `server.rs`'s
//! `PolicyScope::WorkspaceWideAdmin` for what that enforces at this layer.

use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

fn parse_input<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, CallToolResult> {
    serde_json::from_value(args).map_err(|err| CallToolResult::error(format!("Invalid input: {err}")))
}

/// The stable reason every import command answers with on the zero-row branch.
const ZERO_INVENTORY_REASON: &str = "not_required_zero_inventory";

fn zero_inventory_refusal(action: &str, details: &Value) -> CallToolResult {
    CallToolResult::business_error(
        ZERO_INVENTORY_REASON,
        format!(
            "legacy Markdown Page inventory is zero across every environment on this deployment \
             (ADR-0003 gate), so there is nothing for '{action}' to act on"
        ),
        false,
        details,
    )
}

// ---- legacy_pages.inventory ----

pub fn legacy_pages_inventory_tool() -> ToolDefinition {
    ToolDefinition {
        name: "legacy_pages.inventory".to_string(),
        description: "Row count/max body bytes/schema hash of the legacy Markdown Page table \
this deployment would migrate from, never page content. Conditional per ADR-0003."
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
struct WorkspaceIdInput {
    workspace_id: String,
}

// `server::McpServer::execute_tool` dispatches every registered tool through a
// uniform `async fn(&OpenPrClient, Value) -> CallToolResult` and `.await`s it; this
// handler never awaits anything itself because it is answered entirely on the
// zero-inventory branch without a network call (see the module doc comment), but it
// keeps the same signature as every other tool handler rather than forcing dispatch
// to special-case it.
#[allow(clippy::unused_async)]
pub async fn legacy_pages_inventory(_client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: WorkspaceIdInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };

    CallToolResult::success(
        serde_json::to_string_pretty(&json!({
            "workspace_id": input.workspace_id,
            "row_count": 0,
            "max_body_md_bytes": 0,
            "source_schema_sha256": null,
            "collected_at": null,
            "note": "legacy Markdown Page inventory is zero across every environment on this \
        deployment (ADR-0003 not_required_zero_inventory gate); migration tooling is not required"
        }))
        .unwrap_or_default(),
    )
}

// ---- legacy_pages.import_preview ----

pub fn legacy_pages_import_preview_tool() -> ToolDefinition {
    ToolDefinition {
        name: "legacy_pages.import_preview".to_string(),
        description: "Dry-run preview of a legacy Markdown Page -> Flow Page import. On this \
deployment inventory is zero (ADR-0003), so this always answers not_required_zero_inventory and \
writes nothing."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "workspace_id": { "type": "string", "description": "Workspace UUID" },
                "source_page_ids": { "type": "array", "items": { "type": "string" }, "description": "Defaults to every legacy page when omitted" },
                "idempotency_key": { "type": "string" }
            },
            "required": ["workspace_id", "idempotency_key"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ImportPreviewInput {
    workspace_id: String,
    source_page_ids: Option<Vec<String>>,
    idempotency_key: String,
}

// `server::McpServer::execute_tool` dispatches every registered tool through a
// uniform `async fn(&OpenPrClient, Value) -> CallToolResult` and `.await`s it; this
// handler never awaits anything itself because it is answered entirely on the
// zero-inventory branch without a network call (see the module doc comment), but it
// keeps the same signature as every other tool handler rather than forcing dispatch
// to special-case it.
#[allow(clippy::unused_async)]
pub async fn legacy_pages_import_preview(_client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ImportPreviewInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    if input.idempotency_key.trim().is_empty() {
        return CallToolResult::error("idempotency_key must not be empty".to_string());
    }
    zero_inventory_refusal(
        "legacy_pages.import_preview",
        &json!({
            "workspace_id": input.workspace_id,
            "source_page_ids": input.source_page_ids,
        }),
    )
}

// ---- legacy_pages.import_commit ----

pub fn legacy_pages_import_commit_tool() -> ToolDefinition {
    ToolDefinition {
        name: "legacy_pages.import_commit".to_string(),
        description: "Commits a previously previewed legacy Markdown Page import. On this \
deployment inventory is zero (ADR-0003), so this always answers not_required_zero_inventory and \
writes nothing."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "workspace_id": { "type": "string", "description": "Workspace UUID" },
                "import_id": { "type": "string" },
                "source_set_hash": { "type": "string" },
                "confirm": { "type": "boolean", "const": true },
                "idempotency_key": { "type": "string" }
            },
            "required": ["workspace_id", "import_id", "source_set_hash", "confirm", "idempotency_key"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ImportCommitInput {
    workspace_id: String,
    import_id: String,
    source_set_hash: String,
    confirm: bool,
    idempotency_key: String,
}

// `server::McpServer::execute_tool` dispatches every registered tool through a
// uniform `async fn(&OpenPrClient, Value) -> CallToolResult` and `.await`s it; this
// handler never awaits anything itself because it is answered entirely on the
// zero-inventory branch without a network call (see the module doc comment), but it
// keeps the same signature as every other tool handler rather than forcing dispatch
// to special-case it.
#[allow(clippy::unused_async)]
pub async fn legacy_pages_import_commit(_client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ImportCommitInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    if !input.confirm {
        return CallToolResult::error("confirm must be true".to_string());
    }
    if input.idempotency_key.trim().is_empty() {
        return CallToolResult::error("idempotency_key must not be empty".to_string());
    }
    zero_inventory_refusal(
        "legacy_pages.import_commit",
        &json!({
            "workspace_id": input.workspace_id,
            "import_id": input.import_id,
            "source_set_hash": input.source_set_hash,
        }),
    )
}

// ---- legacy_pages.import_status ----

pub fn legacy_pages_import_status_tool() -> ToolDefinition {
    ToolDefinition {
        name: "legacy_pages.import_status".to_string(),
        description: "Status/lineage of a previously committed legacy Markdown Page import; \
never returns page body content. On this deployment inventory is zero (ADR-0003), so no import \
can exist and this always answers not_required_zero_inventory."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "workspace_id": { "type": "string", "description": "Workspace UUID" },
                "import_id": { "type": "string" }
            },
            "required": ["workspace_id", "import_id"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct ImportStatusInput {
    workspace_id: String,
    import_id: String,
}

// `server::McpServer::execute_tool` dispatches every registered tool through a
// uniform `async fn(&OpenPrClient, Value) -> CallToolResult` and `.await`s it; this
// handler never awaits anything itself because it is answered entirely on the
// zero-inventory branch without a network call (see the module doc comment), but it
// keeps the same signature as every other tool handler rather than forcing dispatch
// to special-case it.
#[allow(clippy::unused_async)]
pub async fn legacy_pages_import_status(_client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ImportStatusInput = match parse_input(args) {
        Ok(value) => value,
        Err(result) => return result,
    };
    zero_inventory_refusal(
        "legacy_pages.import_status",
        &json!({
            "workspace_id": input.workspace_id,
            "import_id": input.import_id,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        legacy_pages_import_commit, legacy_pages_import_preview, legacy_pages_import_status, legacy_pages_inventory,
    };
    use crate::client::test_api;
    use serde_json::json;

    #[tokio::test]
    async fn inventory_reports_zero_without_calling_the_network() -> Result<(), String> {
        // Base URL points at a port nothing listens on: a network call would fail the read.
        let client = test_api::client("http://127.0.0.1:1".to_string())?;
        let result = legacy_pages_inventory(
            &client,
            json!({ "workspace_id": "11111111-1111-4111-8111-111111111111" }),
        )
        .await;
        assert_eq!(result.is_error, None);
        Ok(())
    }

    #[tokio::test]
    async fn import_preview_safely_explains_the_zero_branch() -> Result<(), String> {
        let client = test_api::client("http://127.0.0.1:1".to_string())?;
        let result = legacy_pages_import_preview(
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
    async fn import_commit_requires_confirm_true() -> Result<(), String> {
        let client = test_api::client("http://127.0.0.1:1".to_string())?;
        let result = legacy_pages_import_commit(
            &client,
            json!({
                "workspace_id": "11111111-1111-4111-8111-111111111111",
                "import_id": "22222222-2222-4222-8222-222222222222",
                "source_set_hash": "deadbeef",
                "confirm": false,
                "idempotency_key": "key-1"
            }),
        )
        .await;
        assert_eq!(result.is_error, Some(true));
        Ok(())
    }

    #[tokio::test]
    async fn import_status_safely_explains_the_zero_branch() -> Result<(), String> {
        let client = test_api::client("http://127.0.0.1:1".to_string())?;
        let result = legacy_pages_import_status(
            &client,
            json!({
                "workspace_id": "11111111-1111-4111-8111-111111111111",
                "import_id": "22222222-2222-4222-8222-222222222222"
            }),
        )
        .await;
        assert_eq!(result.is_error, Some(true));
        Ok(())
    }
}
