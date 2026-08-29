//! `sylvode` — the Sylvode Flow CLI (`cli-surface-v1.md`).
//!
//! A new `[[bin]]` in the existing `mcp-server` package rather than a new crate or a rename of
//! the existing binary (`cli-surface-v1.md` "交付形态": "选择同 crate 新 `[[bin]]`，不 rename
//! 现有 binary、不建新 crate"). It shares `mcp_server::client::OpenPrClient` — the same
//! `reqwest` HTTP client, envelope handling and audit headers every `mcp-server` tool and CLI
//! subcommand already goes through — and its own `mcp_server::cli_app` command model, config
//! resolver, typed error and JSON renderer. It never starts a server: there is no `serve`
//! subcommand here.

use clap::Parser;
use mcp_server::cli_app::{self, command::Cli};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let exit_code = cli_app::run(cli).await;
    std::process::exit(exit_code);
}
