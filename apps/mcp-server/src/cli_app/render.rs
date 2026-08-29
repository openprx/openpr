//! The `sylvode.cli.v1` success/failure envelope (`cli-surface-v1.md` "命名与格式") and its
//! two renderings.
//!
//! `--format json` is the stable machine contract; `--format table` is a human display that
//! may evolve.

// This is a CLI output module: writing to stdout/stderr is its entire job, matching
// `apps/mcp-server/src/cli.rs`'s existing `#![allow(clippy::print_stdout, clippy::print_stderr)]`
// for the same reason.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use super::error::CliError;
use serde_json::{Value, json};

pub const SCHEMA_VERSION: &str = "sylvode.cli.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum OutputFormat {
    #[default]
    Json,
    Table,
}

/// Renders one command outcome to `stdout`/`stderr` and returns the process exit code.
///
/// `--format json` writes the whole envelope to stdout on both success and failure — "JSON
/// 成功/失败均只写 stdout" — so a scripted caller reads one place for either. `--format table`
/// is a human display: success renders `data` as a table on stdout, failure renders a short
/// human line on stderr and nothing on stdout, matching this binary's `mcp-server` sibling
/// CLI's existing convention for a failed call.
pub fn render(format: OutputFormat, command: &str, outcome: Result<Value, CliError>, request_id: &str) -> i32 {
    match (format, outcome) {
        (OutputFormat::Json, Ok(data)) => {
            println!("{}", success_envelope(command, &data, request_id));
            0
        }
        (OutputFormat::Json, Err(error)) => {
            println!("{}", failure_envelope(command, &error, request_id));
            error.exit
        }
        (OutputFormat::Table, Ok(data)) => {
            print_table(&data);
            0
        }
        (OutputFormat::Table, Err(error)) => {
            eprintln!("Error [{}]: {}", error.code, error.message);
            error.exit
        }
    }
}

fn success_envelope(command: &str, data: &Value, request_id: &str) -> String {
    json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "command": command,
        "data": data,
        "request_id": request_id,
    })
    .to_string()
}

fn failure_envelope(command: &str, error: &CliError, request_id: &str) -> String {
    json!({
        "schema_version": SCHEMA_VERSION,
        "ok": false,
        "command": command,
        "error": {
            "code": error.code,
            "message": error.message,
            "recoverable": error.recoverable,
            "details": error.details,
        },
        "request_id": request_id,
    })
    .to_string()
}

fn fmt_val(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

fn print_table(value: &Value) {
    match value {
        Value::Object(obj) if obj.contains_key("items") && obj.get("items").is_some_and(Value::is_array) => {
            print_table(obj.get("items").unwrap_or(&Value::Null));
        }
        Value::Array(items) if !items.is_empty() => {
            if let Some(Value::Object(first)) = items.first() {
                let keys: Vec<String> = first.keys().cloned().collect();
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        println!("---");
                    }
                    if let Value::Object(obj) = item {
                        let max_key = keys.iter().map(String::len).max().unwrap_or(0);
                        for key in &keys {
                            println!("{key:<max_key$}  {}", fmt_val(obj.get(key).unwrap_or(&Value::Null)));
                        }
                    }
                }
            } else {
                for item in items {
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
