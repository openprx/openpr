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
    /// Flow Collections
    Collections(CollectionsCmd),
    /// Collection Records
    Records(RecordsCmd),
    /// Flow collaboration diagnostics
    Collab(CollabCmd),
    /// Flow delivery maintenance
    Deliveries(DeliveriesCmd),
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
    /// Create an object export job
    Export {
        id: String,
        #[arg(long = "render", value_parser = ["json", "markdown", "csv", "package"])]
        render: String,
        #[arg(long = "at-seq")]
        at_seq: Option<i64>,
        #[arg(long)]
        wait: bool,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
    },
    /// Create a page, navigator, or Collection Flow object
    Create {
        #[arg(long)]
        workspace: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long = "type", value_parser = ["page", "navigator", "collection"])]
        object_type: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        parent: Option<String>,
        #[arg(long = "embed-page")]
        embed_page: Option<String>,
        #[arg(long = "schema-file")]
        schema_file: Option<PathBuf>,
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
    /// Reference a Form or Form record without copying target data
    Reference {
        source: String,
        #[arg(long = "target-type", value_parser = ["form", "form-record"])]
        target_type: String,
        #[arg(long)]
        target: String,
        #[arg(long = "display-file")]
        display_file: Option<PathBuf>,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
    },
    /// Remove a reference without deleting its target
    Unreference {
        source: String,
        #[arg(long = "reference")]
        reference_id: String,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
    },
    /// Preview a Flow-to-Forms conversion
    ConvertPreview {
        source: String,
        #[arg(long = "frontier")]
        source_frontier: String,
        #[arg(long = "to", value_parser = ["form", "form-record"])]
        target_type: String,
        #[arg(long = "mapping")]
        mapping_file: PathBuf,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
    },
    /// Commit a frozen conversion preview
    ConvertCommit {
        #[arg(long = "preview")]
        preview_id: String,
        #[arg(long = "frontier")]
        source_frontier: String,
        #[arg(long = "schema-version")]
        target_schema_version: i32,
        #[arg(long)]
        confirm: bool,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
    },
    /// Read conversion status
    ConvertStatus { job: String },
    /// Retry a failed conversion
    ConvertRetry {
        job: String,
        #[arg(long)]
        confirm: bool,
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
pub struct CollectionsCmd {
    #[command(subcommand)]
    pub action: CollectionsAction,
}

#[derive(Debug, Subcommand)]
pub enum CollectionsAction {
    Describe {
        id: String,
    },
    Query {
        id: String,
        #[arg(long = "query-file")]
        query_file: PathBuf,
        #[arg(long)]
        cursor: Option<String>,
    },
}

#[derive(Debug, Args)]
pub struct RecordsCmd {
    #[command(subcommand)]
    pub action: RecordsAction,
}

#[derive(Debug, Subcommand)]
pub enum RecordsAction {
    Create {
        #[arg(long)]
        collection: String,
        #[arg(long = "values-file")]
        values_file: PathBuf,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
        #[arg(long)]
        body: Option<String>,
    },
    Patch {
        id: String,
        #[arg(long = "values-file")]
        values_file: PathBuf,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
        #[arg(long)]
        body: Option<String>,
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
    /// Read workspace health, lag, and integrity summaries
    Status {
        #[arg(long)]
        workspace: String,
    },
    /// Document metadata (engine/seq/frontier/byte size) for one Flow object; never raw bytes
    Inspect { id: String },
    /// Shallow (v0.4: `deep=false`) collaboration integrity check for one Flow object
    Verify {
        id: String,
        #[arg(long)]
        deep: bool,
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
    /// Dry-run or execute compaction of one exact document
    Compact {
        id: String,
        #[arg(long, conflicts_with = "execute")]
        dry_run: bool,
        #[arg(long, conflicts_with = "dry_run")]
        execute: bool,
        #[arg(long = "expected-head")]
        expected_head: i64,
        #[arg(long)]
        confirm: Option<String>,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
    },
    /// Dry-run or execute projection rebuild for one exact object
    RebuildProjection {
        id: String,
        #[arg(long, conflicts_with = "execute")]
        dry_run: bool,
        #[arg(long, conflicts_with = "dry_run")]
        execute: bool,
        #[arg(long = "expected-head")]
        expected_head: i64,
        #[arg(long)]
        confirm: Option<String>,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
    },
    /// Export one object as a history-bearing package
    Export {
        id: String,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
    },
    /// Export a complete workspace package
    ExportWorkspace {
        #[arg(long)]
        workspace: String,
        #[arg(long = "include-history")]
        include_history: bool,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
    },
    /// Stream a package to artifact staging and create an import preview
    ImportPreview {
        #[arg(long)]
        workspace: String,
        #[arg(long = "package")]
        package_file: PathBuf,
        #[arg(long = "mapping")]
        mapping_file: PathBuf,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
    },
    /// Commit a frozen package import
    ImportCommit {
        #[arg(long)]
        workspace: String,
        #[arg(long = "import")]
        import_id: String,
        #[arg(long = "package-hash")]
        package_hash: String,
        #[arg(long = "mapping-hash")]
        mapping_hash: String,
        #[arg(long = "conflict-policy", value_parser = ["new-ids", "reuse-import-lineage"])]
        conflict_policy: String,
        #[arg(long)]
        confirm: bool,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
    },
    /// Read an import report
    ImportStatus {
        #[arg(long)]
        workspace: String,
        #[arg(long = "import")]
        import_id: String,
        #[arg(long)]
        wait: bool,
    },
}

#[derive(Debug, Args)]
pub struct DeliveriesCmd {
    #[command(subcommand)]
    pub action: DeliveriesAction,
}

#[derive(Debug, Subcommand)]
pub enum DeliveriesAction {
    Replay {
        #[arg(long)]
        workspace: String,
        #[arg(long, value_parser = ["rebuild", "requeue-failed"])]
        mode: String,
        #[arg(long = "from")]
        from_time: String,
        #[arg(long = "to")]
        to_time: String,
        #[arg(long = "event-type")]
        event_type: Option<String>,
        #[arg(long)]
        subscriber: Option<String>,
        #[arg(long, conflicts_with = "execute")]
        dry_run: bool,
        #[arg(long, conflicts_with = "dry_run")]
        execute: bool,
        #[arg(long)]
        confirm: bool,
        #[arg(long = "idempotency-key")]
        idempotency_key: String,
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
    fn parses_v08_package_and_maintenance_commands() {
        let id = "11111111-1111-4111-8111-111111111111";
        for args in [
            vec![
                "sylvode",
                "objects",
                "export",
                id,
                "--render",
                "package",
                "--idempotency-key",
                "k",
            ],
            vec!["sylvode", "collab", "status", "--workspace", id],
            vec![
                "sylvode",
                "collab",
                "compact",
                id,
                "--dry-run",
                "--expected-head",
                "7",
                "--idempotency-key",
                "k",
            ],
            vec![
                "sylvode",
                "collab",
                "rebuild-projection",
                id,
                "--execute",
                "--expected-head",
                "7",
                "--confirm",
                id,
                "--idempotency-key",
                "k",
            ],
            vec![
                "sylvode",
                "collab",
                "import-preview",
                "--workspace",
                id,
                "--package",
                "package.zip",
                "--mapping",
                "mapping.json",
                "--idempotency-key",
                "k",
            ],
            vec![
                "sylvode",
                "deliveries",
                "replay",
                "--workspace",
                id,
                "--mode",
                "rebuild",
                "--from",
                "2026-01-01T00:00:00Z",
                "--to",
                "2026-01-02T00:00:00Z",
                "--dry-run",
                "--confirm",
                "--idempotency-key",
                "k",
            ],
        ] {
            Cli::try_parse_from(args).expect("v0.8 command must parse");
        }
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
    fn parses_every_v07_bridge_command_line() {
        const ID: &str = "11111111-1111-4111-8111-111111111111";
        const OTHER: &str = "22222222-2222-4222-8222-222222222222";
        let lines = [
            vec![
                "sylvode",
                "objects",
                "reference",
                ID,
                "--target-type",
                "form",
                "--target",
                OTHER,
                "--idempotency-key",
                "k",
            ],
            vec![
                "sylvode",
                "objects",
                "unreference",
                ID,
                "--reference",
                OTHER,
                "--idempotency-key",
                "k",
            ],
            vec![
                "sylvode",
                "objects",
                "convert-preview",
                ID,
                "--frontier",
                "F",
                "--to",
                "form-record",
                "--mapping",
                "mapping.json",
                "--idempotency-key",
                "k",
            ],
            vec![
                "sylvode",
                "objects",
                "convert-commit",
                "--preview",
                ID,
                "--frontier",
                "F",
                "--schema-version",
                "1",
                "--confirm",
                "--idempotency-key",
                "k",
            ],
            vec!["sylvode", "objects", "convert-status", ID],
            vec![
                "sylvode",
                "objects",
                "convert-retry",
                ID,
                "--confirm",
                "--idempotency-key",
                "k",
            ],
        ];
        for line in lines {
            Cli::try_parse_from(line).expect("frozen v0.7 bridge command must parse");
        }
    }

    #[test]
    fn requires_a_subcommand() {
        assert!(Cli::try_parse_from(["sylvode"]).is_err());
    }
}
