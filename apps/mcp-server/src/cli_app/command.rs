//! `sylvode`'s command tree — `cli-surface-v1.md` "命令树", the v0.4 rows only.
//!
//! `features flow get|set`, `objects get|query|history`, `collab inspect|verify`.
//! `legacy-pages` is omitted: this deployment's ADR-0003 inventory is zero across every
//! environment, and the zero branch does not require a native command to exist
//! (`cli-surface-v1.md`: "零行分支不要求命令存在").

use super::render::OutputFormat;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

/// `sylvode` — the Sylvode Flow CLI (online-only; no `serve` subcommand, see `cli-surface-v1.md`).
#[derive(Debug, Parser)]
#[command(name = "sylvode", about = "Sylvode Flow CLI")]
#[command(arg_required_else_help = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Path to the configuration file \[default: config/openpr.toml\]
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Output format: `json` is the stable machine contract, `table` is a human display
    #[arg(long, value_enum, global = true, default_value_t = OutputFormat::Json)]
    pub format: OutputFormat,

    /// API URL (overrides `mcp.api_url`)
    #[arg(long, global = true)]
    pub api_url: Option<String>,

    /// Bot authentication token (overrides `mcp.bot_token`)
    #[arg(long, global = true)]
    pub bot_token: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Flow workspace feature flag
    Features(FeaturesCmd),
    /// Flow objects
    Objects(ObjectsCmd),
    /// Flow collaboration diagnostics
    Collab(CollabCmd),
}

// ---- features flow get|set ----

#[derive(Debug, Args)]
pub struct FeaturesCmd {
    #[command(subcommand)]
    pub action: FeaturesAction,
}

#[derive(Debug, Subcommand)]
pub enum FeaturesAction {
    /// The Flow rollout flag
    Flow(FlowFeatureCmd),
}

#[derive(Debug, Args)]
pub struct FlowFeatureCmd {
    #[command(subcommand)]
    pub action: FlowFeatureAction,
}

#[derive(Debug, Subcommand)]
pub enum FlowFeatureAction {
    /// Read `flow_enabled`, `default_member_level`, `authz_epoch` for a workspace
    Get {
        #[arg(long)]
        workspace: String,
    },
    /// Change the Flow rollout flag for a workspace (workspace admin only)
    Set {
        #[arg(long)]
        workspace: String,
        #[arg(long)]
        enabled: Option<bool>,
        #[arg(long = "default-member-level")]
        default_member_level: Option<String>,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
    },
}

// ---- objects get|query|history ----

#[derive(Debug, Args)]
pub struct ObjectsCmd {
    #[command(subcommand)]
    pub action: ObjectsAction,
}

#[derive(Debug, Subcommand)]
pub enum ObjectsAction {
    /// Get one Flow object
    Get {
        id: String,
        #[arg(long = "at-seq")]
        at_seq: Option<i64>,
        #[arg(long, value_parser = ["semantic-json", "markdown"])]
        render: Option<String>,
    },
    /// List Flow objects in a workspace, scoped to one project or --unprojected
    Query {
        #[arg(long)]
        workspace: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        unprojected: bool,
        #[arg(long = "type")]
        object_type: Option<String>,
        #[arg(long)]
        query: Option<String>,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        limit: Option<u64>,
    },
    /// One Flow object's accepted-update history page
    History {
        id: String,
        #[arg(long = "before-seq")]
        before_seq: Option<i64>,
        #[arg(long)]
        limit: Option<u64>,
    },
}

// ---- collab inspect|verify ----

#[derive(Debug, Args)]
pub struct CollabCmd {
    #[command(subcommand)]
    pub action: CollabAction,
}

#[derive(Debug, Subcommand)]
pub enum CollabAction {
    /// Document metadata (engine/seq/frontier/byte size) for one Flow object; never raw bytes
    Inspect { id: String },
    /// Shallow (v0.4: `deep=false`) collaboration integrity check for one Flow object
    Verify {
        id: String,
        #[arg(long = "expected-head")]
        expected_head: Option<i64>,
    },
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::Parser;

    #[test]
    fn parses_features_flow_get() {
        let cli = Cli::try_parse_from([
            "sylvode",
            "features",
            "flow",
            "get",
            "--workspace",
            "11111111-1111-4111-8111-111111111111",
        ])
        .expect("valid arguments should parse");
        assert!(matches!(cli.command, super::Commands::Features(_)));
    }

    #[test]
    fn parses_objects_get_with_render() {
        Cli::try_parse_from([
            "sylvode",
            "objects",
            "get",
            "11111111-1111-4111-8111-111111111111",
            "--render",
            "markdown",
        ])
        .expect("valid arguments should parse");
    }

    #[test]
    fn rejects_unknown_render_value_locally() {
        let result = Cli::try_parse_from([
            "sylvode",
            "objects",
            "get",
            "11111111-1111-4111-8111-111111111111",
            "--render",
            "html",
        ]);
        assert!(result.is_err(), "an unknown --render value must fail to parse");
    }

    #[test]
    fn parses_collab_verify_with_expected_head() {
        Cli::try_parse_from([
            "sylvode",
            "collab",
            "verify",
            "11111111-1111-4111-8111-111111111111",
            "--expected-head",
            "3",
        ])
        .expect("valid arguments should parse");
    }

    #[test]
    fn requires_a_subcommand() {
        assert!(Cli::try_parse_from(["sylvode"]).is_err());
    }
}
