// CLI output functions necessarily use print macros and indexing — allow these for this module.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::unreachable,
    clippy::indexing_slicing
)]

use base64::Engine as _;
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::{Value, json};

use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolContent};
use crate::server::McpServer;

#[derive(Debug, Clone, ValueEnum, Default)]
pub enum OutputFormat {
    #[default]
    Json,
    Table,
}

/// `OpenPR` MCP server and CLI tool
#[derive(Debug, Parser)]
#[command(name = "mcp-server", about = "OpenPR MCP server and CLI tool")]
#[command(arg_required_else_help = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Output format
    #[arg(long, value_enum, global = true, default_value_t = OutputFormat::Json)]
    pub format: OutputFormat,

    /// API URL (overrides `OPENPR_API_URL`)
    #[arg(long, global = true)]
    pub api_url: Option<String>,

    /// Bot authentication token (overrides `OPENPR_BOT_TOKEN`)
    #[arg(long, global = true)]
    pub bot_token: Option<String>,

    /// Workspace ID (overrides `OPENPR_WORKSPACE_ID`)
    #[arg(long, global = true)]
    pub workspace_id: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Run the MCP server (default mode)
    Serve(ServeArgs),
    /// Manage projects
    Projects(ProjectsCmd),
    /// Manage work items
    #[command(name = "work-items")]
    WorkItems(WorkItemsCmd),
    /// Manage comments
    Comments(CommentsCmd),
    /// Manage labels
    Labels(LabelsCmd),
    /// Manage sprints
    Sprints(SprintsCmd),
    /// Global workspace search
    Search(SearchArgs),
    /// Upload files
    Files(FilesCmd),
}

// ---- Serve ----

#[derive(Debug, Clone, ValueEnum)]
pub enum Transport {
    Http,
    Sse,
    Stdio,
}

#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Transport protocol
    #[arg(long, value_enum, default_value_t = Transport::Stdio)]
    pub transport: Transport,
    /// Bind address for HTTP/SSE transports
    #[arg(long, default_value = "0.0.0.0:8090")]
    pub bind_addr: String,
}

// ---- Projects ----

#[derive(Debug, Args)]
pub struct ProjectsCmd {
    #[command(subcommand)]
    pub action: ProjectsAction,
}

#[derive(Debug, Subcommand)]
pub enum ProjectsAction {
    /// List all projects in the workspace
    List,
    /// Get a project by UUID
    Get {
        /// Project UUID
        id: String,
    },
    /// Create a new project
    Create {
        #[arg(long)]
        name: String,
        #[arg(long)]
        description: Option<String>,
    },
}

// ---- Work Items ----

#[derive(Debug, Args)]
pub struct WorkItemsCmd {
    #[command(subcommand)]
    pub action: WorkItemsAction,
}

#[derive(Debug, Subcommand)]
pub enum WorkItemsAction {
    /// List work items in a project
    List {
        #[arg(long)]
        project: String,
        /// Filter by state (`backlog|todo|in_progress|done`)
        #[arg(long)]
        state: Option<String>,
    },
    /// Get a work item by UUID or identifier (e.g. PRX-42)
    Get { id: String },
    /// Create a work item
    Create {
        #[arg(long)]
        project: String,
        #[arg(long)]
        title: String,
        /// Initial state (`backlog|todo|in_progress|done`)
        #[arg(long, default_value = "backlog")]
        state: String,
        /// Priority (`none|low|medium|high|urgent`)
        #[arg(long, default_value = "medium")]
        priority: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// Search work items by query
    Search {
        #[arg(long)]
        query: String,
    },
    /// Update a work item
    Update {
        /// Work item UUID
        id: String,
        /// New state (`backlog|todo|in_progress|done`)
        #[arg(long)]
        state: Option<String>,
        /// New priority (`none|low|medium|high|urgent`)
        #[arg(long)]
        priority: Option<String>,
        #[arg(long)]
        title: Option<String>,
    },
}

// ---- Comments ----

#[derive(Debug, Args)]
pub struct CommentsCmd {
    #[command(subcommand)]
    pub action: CommentsAction,
}

#[derive(Debug, Subcommand)]
pub enum CommentsAction {
    /// List comments on a work item
    List {
        #[arg(long)]
        work_item: String,
    },
    /// Create a comment on a work item
    Create {
        #[arg(long)]
        work_item: String,
        #[arg(long)]
        content: String,
    },
}

// ---- Labels ----

#[derive(Debug, Args)]
pub struct LabelsCmd {
    #[command(subcommand)]
    pub action: LabelsAction,
}

#[derive(Debug, Subcommand)]
pub enum LabelsAction {
    /// List labels (workspace-wide, or project-specific with --project)
    List {
        #[arg(long)]
        project: Option<String>,
    },
}

// ---- Sprints ----

#[derive(Debug, Args)]
pub struct SprintsCmd {
    #[command(subcommand)]
    pub action: SprintsAction,
}

#[derive(Debug, Subcommand)]
pub enum SprintsAction {
    /// List sprints for a project
    List {
        #[arg(long)]
        project: String,
    },
}

// ---- Search ----

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Search query
    pub query: String,
}

// ---- Files ----

#[derive(Debug, Args)]
pub struct FilesCmd {
    #[command(subcommand)]
    pub action: FilesAction,
}

#[derive(Debug, Subcommand)]
pub enum FilesAction {
    /// Upload a file from disk (reads file, encodes to base64, posts to API)
    Upload {
        /// Path to the file to upload
        #[arg(long)]
        file: String,
    },
}

// ---- Output formatting ----

pub fn print_result(format: &OutputFormat, result: &CallToolResult) {
    let text = result
        .content
        .iter()
        .find_map(|c| match c {
            ToolContent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap_or("");

    if result.is_error == Some(true) {
        eprintln!("{text}");
        std::process::exit(1);
    }

    match format {
        OutputFormat::Json => println!("{text}"),
        OutputFormat::Table => {
            if let Ok(value) = serde_json::from_str::<Value>(text) {
                print_table(&value);
            } else {
                println!("{text}");
            }
        }
    }
}

fn print_table(value: &Value) {
    match value {
        Value::Array(arr) if !arr.is_empty() => {
            if let Some(Value::Object(first)) = arr.first() {
                let keys: Vec<String> = first.keys().cloned().collect();
                let mut widths: Vec<usize> = keys.iter().map(String::len).collect();
                for item in arr {
                    if let Value::Object(obj) = item {
                        for (width, key) in widths.iter_mut().zip(keys.iter()) {
                            let s = fmt_val(obj.get(key).unwrap_or(&Value::Null));
                            *width = (*width).max(s.len().min(60));
                        }
                    }
                }
                // header
                for (key, width) in keys.iter().zip(widths.iter()) {
                    print!("{key:<width$}  ");
                }
                println!();
                // separator
                for w in &widths {
                    print!("{:-<w$}  ", "");
                }
                println!();
                // rows
                for item in arr {
                    if let Value::Object(obj) = item {
                        for (key, width) in keys.iter().zip(widths.iter()) {
                            let s = fmt_val(obj.get(key).unwrap_or(&Value::Null));
                            let truncated = truncate_display(s, 59);
                            print!("{truncated:<width$}  ");
                        }
                        println!();
                    }
                }
            } else {
                for item in arr {
                    println!("{}", fmt_val(item));
                }
            }
        }
        Value::Array(_) => println!("(empty)"),
        Value::Object(obj) => {
            let max_key = obj.keys().map(String::len).max().unwrap_or(0);
            for (key, val) in obj {
                println!("{key:<max_key$}  {}", fmt_val(val));
            }
        }
        _ => println!("{}", fmt_val(value)),
    }
}

/// Truncate a string to at most `max_bytes` bytes on a char boundary, appending `…` if truncated.
fn truncate_display(s: String, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s;
    }
    // Find char boundary just before max_bytes
    let end = s
        .char_indices()
        .take_while(|(i, _)| *i < max_bytes)
        .last()
        .map_or(0, |(i, c)| i + c.len_utf8());
    let prefix = s.get(..end).unwrap_or(&s);
    format!("{prefix}…")
}

fn fmt_val(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

// ---- Dispatch ----

pub async fn run_cli_command(command: &Commands, format: &OutputFormat, client: OpenPrClient) -> anyhow::Result<()> {
    let server = McpServer::new(client);

    // Files upload requires async disk I/O before calling execute_tool, handle it separately
    if let Commands::Files(files_cmd) = command {
        let result = run_file_upload(files_cmd, &server).await?;
        print_result(format, &result);
        return Ok(());
    }

    let (tool_name, args): (&'static str, Value) = match command {
        Commands::Serve(_) => unreachable!("serve is handled before run_cli_command"),

        Commands::Projects(cmd) => match &cmd.action {
            ProjectsAction::List => ("projects.list", json!({})),
            ProjectsAction::Get { id } => ("projects.get", json!({ "project_id": id })),
            ProjectsAction::Create { name, description } => {
                let mut body = json!({ "name": name });
                if let Some(desc) = description {
                    body["description"] = json!(desc);
                }
                ("projects.create", body)
            }
        },

        Commands::WorkItems(cmd) => match &cmd.action {
            WorkItemsAction::List { project, state } => {
                let mut args = json!({ "project_id": project });
                if let Some(s) = state {
                    args["state"] = json!(s);
                }
                ("work_items.list", args)
            }
            WorkItemsAction::Get { id } => {
                // Heuristic: 36-char hex with dashes is UUID, else treat as identifier
                if id.len() == 36 && id.chars().filter(|&c| c == '-').count() == 4 {
                    ("work_items.get", json!({ "work_item_id": id }))
                } else {
                    ("work_items.get_by_identifier", json!({ "identifier": id }))
                }
            }
            WorkItemsAction::Create {
                project,
                title,
                state,
                priority,
                description,
            } => {
                let mut args = json!({
                    "project_id": project,
                    "title": title,
                    "state": state,
                    "priority": priority,
                });
                if let Some(desc) = description {
                    args["description"] = json!(desc);
                }
                ("work_items.create", args)
            }
            WorkItemsAction::Search { query } => ("work_items.search", json!({ "query": query })),
            WorkItemsAction::Update {
                id,
                state,
                priority,
                title,
            } => {
                let mut args = json!({ "work_item_id": id });
                if let Some(s) = state {
                    args["state"] = json!(s);
                }
                if let Some(p) = priority {
                    args["priority"] = json!(p);
                }
                if let Some(t) = title {
                    args["title"] = json!(t);
                }
                ("work_items.update", args)
            }
        },

        Commands::Comments(cmd) => match &cmd.action {
            CommentsAction::List { work_item } => ("comments.list", json!({ "work_item_id": work_item })),
            CommentsAction::Create { work_item, content } => (
                "comments.create",
                json!({ "work_item_id": work_item, "content": content }),
            ),
        },

        Commands::Labels(cmd) => match &cmd.action {
            LabelsAction::List { project } => project.as_ref().map_or_else(
                || ("labels.list", json!({})),
                |pid| ("labels.list_by_project", json!({ "project_id": pid })),
            ),
        },

        Commands::Sprints(cmd) => match &cmd.action {
            SprintsAction::List { project } => ("sprints.list", json!({ "project_id": project })),
        },

        Commands::Search(search_args) => ("search.all", json!({ "query": search_args.query })),

        // Files handled above via run_file_upload
        Commands::Files(_) => unreachable!("Files handled before this match"),
    };

    let result = server.execute_tool(tool_name, args).await;
    print_result(format, &result);
    Ok(())
}

/// Handle file upload: read file from disk, base64-encode, then call files.upload tool.
async fn run_file_upload(cmd: &FilesCmd, server: &McpServer) -> anyhow::Result<CallToolResult> {
    match &cmd.action {
        FilesAction::Upload { file } => {
            let path = std::path::Path::new(file.as_str());
            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(file.as_str())
                .to_string();
            let content = tokio::fs::read(path)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to read file {file}: {e}"))?;
            let encoded = base64::engine::general_purpose::STANDARD.encode(&content);
            Ok(server
                .execute_tool(
                    "files.upload",
                    json!({ "filename": filename, "content_base64": encoded }),
                )
                .await)
        }
    }
}
