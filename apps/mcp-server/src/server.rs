use crate::client::OpenPrClient;
use crate::protocol::{
    CallToolParams, CallToolResult, JsonRpcError, JsonRpcRequest, JsonRpcResponse, ListToolsResult,
};
use crate::tools;
use serde_json::Value;

pub struct McpServer {
    client: OpenPrClient,
}

impl McpServer {
    pub fn new(client: OpenPrClient) -> Self {
        Self { client }
    }

    pub async fn handle_request(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        tracing::info!(method = %req.method, id = ?req.id, "Handling MCP request");

        match req.method.as_str() {
            "tools/list" => self.handle_list_tools(req.id),
            "tools/call" => self.handle_call_tool(req.id, req.params).await,
            "initialize" => self.handle_initialize(req.id),
            _ => JsonRpcResponse::error(
                req.id,
                JsonRpcError::method_not_found(format!("Unknown method: {}", req.method)),
            ),
        }
    }

    fn handle_list_tools(&self, id: Option<Value>) -> JsonRpcResponse {
        let tools = tools::get_all_tool_definitions();
        let result = ListToolsResult { tools };

        match serde_json::to_value(&result) {
            Ok(value) => JsonRpcResponse::success(id, value),
            Err(e) => JsonRpcResponse::error(
                id,
                JsonRpcError::internal_error(format!("Failed to serialize tools: {}", e)),
            ),
        }
    }

    async fn handle_call_tool(&self, id: Option<Value>, params: Option<Value>) -> JsonRpcResponse {
        let params = match params {
            Some(p) => p,
            None => {
                return JsonRpcResponse::error(id, JsonRpcError::invalid_params("Missing params"));
            }
        };

        let call_params: CallToolParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    id,
                    JsonRpcError::invalid_params(format!("Invalid params: {}", e)),
                );
            }
        };

        let args = call_params
            .arguments
            .unwrap_or(Value::Object(Default::default()));

        let result = self.execute_tool(&call_params.name, args).await;

        match serde_json::to_value(&result) {
            Ok(value) => JsonRpcResponse::success(id, value),
            Err(e) => JsonRpcResponse::error(
                id,
                JsonRpcError::internal_error(format!("Failed to serialize result: {}", e)),
            ),
        }
    }

    async fn execute_tool(&self, name: &str, args: Value) -> CallToolResult {
        match name {
            // Projects
            "projects.list" => tools::projects::list_projects(&self.client, args).await,
            "projects.get" => tools::projects::get_project(&self.client, args).await,
            "projects.create" => tools::projects::create_project(&self.client, args).await,
            "projects.update" => tools::projects::update_project(&self.client, args).await,
            "projects.delete" => {
                tools::projects::handle_delete_project(&self.client, args).await
            }

            // Work Items
            "work_items.list" => tools::work_items::list_work_items(&self.client, args).await,
            "work_items.get" => tools::work_items::get_work_item(&self.client, args).await,
            "work_items.create" => tools::work_items::create_work_item(&self.client, args).await,
            "work_items.update" => tools::work_items::update_work_item(&self.client, args).await,
            "work_items.delete" => {
                tools::work_items::handle_delete_work_item(&self.client, args).await
            }
            "work_items.search" => {
                tools::work_items::search_work_items(&self.client, args).await
            }

            // Comments
            "comments.list" => tools::comments::list_comments(&self.client, args).await,
            "comments.create" => tools::comments::create_comment(&self.client, args).await,
            "comments.delete" => {
                tools::comments::handle_delete_comment(&self.client, args).await
            }

            // Proposals
            "proposals.list" => tools::proposals::list_proposals(&self.client, args).await,
            "proposals.get" => tools::proposals::get_proposal(&self.client, args).await,
            "proposals.create" => tools::proposals::create_proposal(&self.client, args).await,

            // Members
            "members.list" => tools::members::list_members(&self.client, args).await,

            // Sprints
            "sprints.create" => tools::sprints::create_sprint(&self.client, args).await,
            "sprints.update" => tools::sprints::update_sprint(&self.client, args).await,

            // Labels
            "labels.create" => tools::labels::create_label(&self.client, args).await,

            // Search
            "search.all" => tools::search::search_all(&self.client, args).await,

            _ => CallToolResult::error(format!("Unknown tool: {}", name)),
        }
    }

    fn handle_initialize(&self, id: Option<Value>) -> JsonRpcResponse {
        let result = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "openpr-mcp-server",
                "version": "0.1.0"
            }
        });

        JsonRpcResponse::success(id, result)
    }
}
