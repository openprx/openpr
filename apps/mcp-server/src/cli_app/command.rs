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
    /// Create a page or navigator Flow object
    Create {
        #[arg(long)]
        workspace: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long = "type", value_parser = ["page", "navigator"])]
        object_type: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        parent: Option<String>,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
    },
    /// Atomically apply semantic operations read from a JSON file
    Patch {
        id: String,
        #[arg(long = "patch-file")]
        patch_file: PathBuf,
        #[arg(long = "expected-frontier")]
        expected_frontier: Option<String>,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
    },
    /// Move an object under a new parent
    Move {
        id: String,
        #[arg(long)]
        parent: String,
        #[arg(long)]
        after: Option<String>,
        #[arg(long = "expected-target-frontier")]
        expected_target_frontier: Option<String>,
        #[arg(long = "confirm-self-lockout")]
        confirm_self_lockout: bool,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
    },
    /// Read or replace object grants
    Grants(GrantsCmd),
    /// Change object inheritance
    Inheritance(InheritanceCmd),
    /// Link two objects
    Link {
        source: String,
        target: String,
        #[arg(long = "kind")]
        relation_type: String,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
    },
    /// Remove an object relation
    Unlink {
        source: String,
        #[arg(long = "relation")]
        relation_id: String,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
    },
    /// Read a semantic diff
    Diff {
        id: String,
        #[arg(long = "from")]
        from_seq: i64,
        #[arg(long = "to")]
        to_seq: i64,
        #[arg(long, value_parser = ["semantic-json", "markdown"])]
        render: Option<String>,
    },
    /// List policy-filtered relations
    Relations {
        id: String,
        #[arg(long, value_parser = ["outgoing", "incoming", "both"])]
        direction: Option<String>,
        #[arg(long = "kind")]
        relation_type: Option<String>,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        limit: Option<u64>,
    },
    /// Search accepted Flow projections
    Search {
        #[arg(long)]
        workspace: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        unprojected: bool,
        #[arg(long = "query")]
        query: String,
        #[arg(long = "type")]
        object_type: Option<String>,
        #[arg(long, value_parser = ["allow-stale", "require-current"])]
        freshness: Option<String>,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        limit: Option<u64>,
    },
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

#[derive(Debug, Args)]
pub struct GrantsCmd {
    #[command(subcommand)]
    pub action: GrantsAction,
}

#[derive(Debug, Subcommand)]
pub enum GrantsAction {
    /// Read effective and, when authorized, complete object grants
    Get { id: String },
    /// Replace explicit grants; repeat --grant KIND:ID=LEVEL
    Set {
        id: String,
        #[arg(long = "grant", required = true)]
        grants: Vec<String>,
        #[arg(long = "confirm-self-lockout")]
        confirm_self_lockout: bool,
        #[arg(long = "dry-run")]
        dry_run: bool,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
    },
}

#[derive(Debug, Args)]
pub struct InheritanceCmd {
    #[command(subcommand)]
    pub action: InheritanceAction,
}

#[derive(Debug, Subcommand)]
pub enum InheritanceAction {
    /// Set whether the object inherits from its parent
    Set {
        id: String,
        #[arg(long = "inherit", action = clap::ArgAction::Set)]
        inherit_from_parent: bool,
        #[arg(long = "confirm-self-lockout")]
        confirm_self_lockout: bool,
        #[arg(long = "dry-run")]
        dry_run: bool,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
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
    /// Policy-filtered projection lag for a workspace or project
    ProjectionLag {
        #[arg(long)]
        workspace: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        limit: Option<u64>,
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
    fn parses_every_v05_contract_command_line() {
        const ID: &str = "11111111-1111-4111-8111-111111111111";
        const OTHER: &str = "22222222-2222-4222-8222-222222222222";
        let lines = [
            vec![
                "sylvode",
                "objects",
                "create",
                "--workspace",
                ID,
                "--type",
                "page",
                "--title",
                "T",
                "--idempotency-key",
                "k",
            ],
            vec![
                "sylvode",
                "objects",
                "patch",
                ID,
                "--patch-file",
                "patch.json",
                "--idempotency-key",
                "k",
            ],
            vec![
                "sylvode",
                "objects",
                "move",
                ID,
                "--parent",
                OTHER,
                "--expected-target-frontier",
                "F",
                "--idempotency-key",
                "k",
            ],
            vec!["sylvode", "objects", "grants", "get", ID],
            vec![
                "sylvode",
                "objects",
                "grants",
                "set",
                ID,
                "--grant",
                "user:22222222-2222-4222-8222-222222222222=view",
                "--dry-run",
                "--idempotency-key",
                "k",
            ],
            vec![
                "sylvode",
                "objects",
                "inheritance",
                "set",
                ID,
                "--inherit",
                "true",
                "--dry-run",
                "--idempotency-key",
                "k",
            ],
            vec![
                "sylvode",
                "objects",
                "link",
                ID,
                OTHER,
                "--kind",
                "related_to",
                "--idempotency-key",
                "k",
            ],
            vec![
                "sylvode",
                "objects",
                "unlink",
                ID,
                "--relation",
                OTHER,
                "--idempotency-key",
                "k",
            ],
            vec!["sylvode", "objects", "diff", ID, "--from", "1", "--to", "2"],
            vec!["sylvode", "objects", "relations", ID, "--direction", "both"],
            vec![
                "sylvode",
                "objects",
                "search",
                "--workspace",
                ID,
                "--unprojected",
                "--query",
                "needle",
            ],
            vec!["sylvode", "collab", "projection-lag", "--workspace", ID],
        ];
        for line in lines {
            Cli::try_parse_from(line).expect("the exact v0.5 contract line should parse");
        }
    }

    #[test]
    fn requires_a_subcommand() {
        assert!(Cli::try_parse_from(["sylvode"]).is_err());
    }
}
