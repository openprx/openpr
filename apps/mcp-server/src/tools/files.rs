use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;

pub fn upload_file_tool() -> ToolDefinition {
    ToolDefinition {
        name: "files.upload".to_string(),
        description: "Upload a file and return its URL".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "filename": {
                    "type": "string",
                    "description": "File name, e.g. error.log or debug.zip"
                },
                "content_base64": {
                    "type": "string",
                    "description": "Base64 encoded file content"
                }
            },
            "required": ["filename", "content_base64"]
        }),
    }
}

#[derive(Debug, Deserialize)]
struct UploadFileInput {
    filename: String,
    content_base64: String,
}

pub async fn upload_file(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: UploadFileInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {}", e)),
    };

    let base64_data = strip_base64_prefix(&input.content_base64);
    let bytes = match base64::engine::general_purpose::STANDARD.decode(base64_data) {
        Ok(b) => b,
        Err(e) => return CallToolResult::error(format!("Invalid base64 content: {}", e)),
    };

    match client.upload_file(&input.filename, bytes).await {
        Ok(uploaded) => {
            let payload = json!({
                "url": uploaded.url,
                "filename": uploaded.filename
            });
            let text = serde_json::to_string_pretty(&payload).unwrap_or_default();
            CallToolResult::success(text)
        }
        Err(e) => CallToolResult::error(e),
    }
}

fn strip_base64_prefix(content: &str) -> &str {
    if let Some((prefix, data)) = content.split_once(',')
        && prefix.to_ascii_lowercase().contains("base64")
    {
        return data;
    }
    content
}
