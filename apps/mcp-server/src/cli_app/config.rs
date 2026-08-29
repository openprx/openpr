//! Builds the `OpenPrClient` `sylvode` speaks every command through.
//!
//! Reuses the same configuration file and `[mcp]` section `mcp-server`'s existing CLI
//! subcommands use (`cli-surface-v1.md`: "CLI 先复用现有 apps/mcp-server client/auth/config；
//! 不另建第二套 HTTP client"). `sylvode` is a local process with no caller of its own, so —
//! exactly like `stdio` and the legacy `mcp-server` CLI subcommands — it always acts as the
//! configured `mcp.bot_token` and always needs one.

use super::error::CliError;
use crate::client::{ClientConfig, OpenPrClient, TRANSPORT_LABEL_CLI};
use platform::config::{OpenPrConfig, Secret};
use std::path::PathBuf;

/// The global flags every `sylvode` subcommand accepts, resolved from `clap` before dispatch.
pub struct GlobalArgs {
    pub config: Option<PathBuf>,
    pub api_url: Option<String>,
    pub bot_token: Option<String>,
}

/// Loads the configuration file and layers the `--api-url`/`--bot-token` overrides onto it.
///
/// Every failure here is local — no request has been sent — so it is always
/// [`CliError::usage`] (`error-mapping-v1.md`: "2 | 本地参数/JSON/文件错误，未发请求").
pub fn build_client(global: &GlobalArgs) -> Result<OpenPrClient, CliError> {
    let config = OpenPrConfig::load(global.config.as_deref())
        .map_err(|err| CliError::usage(format!("configuration error: {err}")))?;
    let mcp = config
        .mcp_runtime()
        .map_err(|err| CliError::usage(format!("configuration error: {err}")))?;

    let api_url = match global.api_url.as_deref() {
        Some(url) => checked_api_url(url)?,
        None => mcp.api_url,
    };
    let bot_token = match global.bot_token.as_deref() {
        Some(token) => checked_bot_token(token)?,
        None => mcp.bot_token.ok_or_else(|| {
            CliError::usage(
                "mcp.bot_token (or --bot-token) is required: sylvode is a local process with no caller \
                 identity of its own to act on behalf of",
            )
        })?,
    };

    OpenPrClient::new(ClientConfig {
        base_url: api_url,
        credential: Some(bot_token),
        workspace_id: mcp.workspace_id.to_string(),
        transport_label: TRANSPORT_LABEL_CLI,
    })
    .map_err(CliError::usage)
}

/// Mirrors the shape `mcp.api_url` and `mcp-server`'s own `--api-url` are held to
/// (`apps/mcp-server/src/main.rs`'s `checked_api_url`), reimplemented here rather than
/// exposed from that binary's private helpers so this module stays self-contained inside the
/// shared library the `mcp-server` and `sylvode` binaries both link.
fn checked_api_url(value: &str) -> Result<String, CliError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.contains("${") {
        return Err(CliError::usage("--api-url must be a concrete URL, not a placeholder"));
    }
    let parsed = reqwest::Url::parse(trimmed)
        .map_err(|error| CliError::usage(format!("--api-url is not a valid URL: {error}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(CliError::usage("--api-url must start with http:// or https://"));
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(CliError::usage("--api-url names no host"));
    }
    Ok(trimmed.to_string())
}

/// Mirrors `mcp-server`'s `checked_bot_token`. The token itself is never echoed.
fn checked_bot_token(value: &str) -> Result<Secret, CliError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.contains("${") || trimmed.contains("replace_with") {
        return Err(CliError::usage("--bot-token must be a concrete bot token"));
    }
    if !trimmed.starts_with("opr_") {
        return Err(CliError::usage("--bot-token must use the opr_ token prefix"));
    }
    Ok(Secret::new(trimmed))
}
