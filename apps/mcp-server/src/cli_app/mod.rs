//! The `sylvode` binary's command model, auth/config resolver, typed error and JSON renderer.
//!
//! `cli-surface-v1.md`: "apps/mcp-server/src/cli_app/ 承载共享 command model、auth/config
//! resolver、typed error 和 JSON renderer". `mcp-server` keeps its own existing `cli.rs`
//! business subcommands unchanged; this module is `sylvode`'s alone.

pub mod api_client;
pub mod command;
pub mod config;
pub mod error;
pub mod render;

use crate::client::{OpenPrClient, encode_query_component};
use command::{Cli, CollabAction, Commands, FeaturesAction, FlowFeatureAction, ObjectsAction};
use error::CliError;
use serde_json::{Value, json};
use uuid::Uuid;

/// Parses `sylvode`'s arguments, runs the command, prints the result, and returns the
/// process exit code the caller's shell should see.
pub async fn run(cli: Cli) -> i32 {
    let request_id = Uuid::new_v4().to_string();
    let command_name = command_name(&cli.command);

    let global = config::GlobalArgs {
        config: cli.config.clone(),
        api_url: cli.api_url.clone(),
        bot_token: cli.bot_token.clone(),
    };

    let outcome = match config::build_client(&global) {
        Ok(client) => dispatch(&client, &cli.command).await,
        Err(error) => Err(error),
    };

    render::render(cli.format, &command_name, outcome, &request_id)
}

fn command_name(command: &Commands) -> String {
    match command {
        Commands::Features(cmd) => match &cmd.action {
            FeaturesAction::Flow(flow) => match &flow.action {
                FlowFeatureAction::Get { .. } => "features.flow.get".to_string(),
                FlowFeatureAction::Set { .. } => "features.flow.set".to_string(),
            },
        },
        Commands::Objects(cmd) => match &cmd.action {
            ObjectsAction::Get { .. } => "objects.get".to_string(),
            ObjectsAction::Query { .. } => "objects.query".to_string(),
            ObjectsAction::History { .. } => "objects.history".to_string(),
        },
        Commands::Collab(cmd) => match &cmd.action {
            CollabAction::Inspect { .. } => "collab.inspect".to_string(),
            CollabAction::Verify { .. } => "collab.verify".to_string(),
        },
    }
}

async fn dispatch(client: &OpenPrClient, command: &Commands) -> Result<Value, CliError> {
    match command {
        Commands::Features(cmd) => match &cmd.action {
            FeaturesAction::Flow(flow) => match &flow.action {
                FlowFeatureAction::Get { workspace } => {
                    let workspace = checked_uuid("--workspace", workspace)?;
                    // `workspace` is already a canonicalized UUID (`checked_uuid`), safe to
                    // interpolate.
                    let path = format!("/api/v1/workspaces/{workspace}/features/flow");
                    api_data(client.get_structured::<Value>(&path).await)
                }
                FlowFeatureAction::Set {
                    workspace,
                    enabled,
                    default_member_level,
                    idempotency_key,
                } => {
                    let workspace = checked_uuid("--workspace", workspace)?;
                    if enabled.is_none() && default_member_level.is_none() {
                        return Err(CliError::usage(
                            "at least one of --enabled or --default-member-level must be supplied",
                        ));
                    }
                    if idempotency_key.trim().is_empty() {
                        return Err(CliError::usage("--idempotency-key must not be empty"));
                    }
                    let mut body = json!({ "idempotency_key": idempotency_key });
                    if let Some(object) = body.as_object_mut() {
                        if let Some(enabled) = enabled {
                            object.insert("enabled".to_string(), json!(enabled));
                        }
                        if let Some(level) = default_member_level {
                            object.insert("default_member_level".to_string(), json!(level));
                        }
                    }
                    let path = format!("/api/v1/workspaces/{workspace}/features/flow");
                    api_data(client.put_structured::<Value, _>(&path, &body).await)
                }
            },
        },
        Commands::Objects(cmd) => match &cmd.action {
            ObjectsAction::Get { id, at_seq, render } => {
                let id = checked_uuid("object id", id)?;
                let mut query = Vec::new();
                if let Some(at_seq) = at_seq {
                    query.push(format!("at_seq={at_seq}"));
                }
                if let Some(render) = render {
                    // clap's `value_parser` already restricted this to semantic-json|markdown.
                    query.push(format!("render={}", render.replace('-', "_")));
                }
                let path = format!("/api/v1/flow/objects/{id}{}", query_suffix(&query));
                api_data(client.get_structured::<Value>(&path).await)
            }
            ObjectsAction::Query {
                workspace,
                project,
                unprojected,
                object_type,
                query: q,
                cursor,
                limit,
            } => {
                let workspace = checked_uuid("--workspace", workspace)?;
                if project.is_some() == *unprojected {
                    return Err(CliError::usage(
                        "exactly one of --project or --unprojected must be supplied",
                    ));
                }
                let mut query = Vec::new();
                if let Some(project) = project {
                    let project = checked_uuid("--project", project)?;
                    query.push(format!("project_id={project}"));
                }
                if *unprojected {
                    query.push("unprojected=true".to_string());
                }
                if let Some(object_type) = object_type {
                    query.push(format!("object_type={}", encode_query_component(object_type)));
                }
                if let Some(q) = q {
                    query.push(format!("q={}", encode_query_component(q)));
                }
                if let Some(cursor) = cursor {
                    query.push(format!("cursor={}", encode_query_component(cursor)));
                }
                if let Some(limit) = limit {
                    if !(1..=100).contains(limit) {
                        return Err(CliError::usage("--limit must be between 1 and 100"));
                    }
                    query.push(format!("limit={limit}"));
                }
                let path = format!("/api/v1/workspaces/{workspace}/flow/objects{}", query_suffix(&query));
                api_data(client.get_structured::<Value>(&path).await)
            }
            ObjectsAction::History { id, before_seq, limit } => {
                let id = checked_uuid("object id", id)?;
                let mut query = Vec::new();
                if let Some(before_seq) = before_seq {
                    query.push(format!("before_seq={before_seq}"));
                }
                if let Some(limit) = limit {
                    if !(1..=100).contains(limit) {
                        return Err(CliError::usage("--limit must be between 1 and 100"));
                    }
                    query.push(format!("limit={limit}"));
                }
                let path = format!("/api/v1/flow/objects/{id}/history{}", query_suffix(&query));
                api_data(client.get_structured::<Value>(&path).await)
            }
        },
        Commands::Collab(cmd) => match &cmd.action {
            CollabAction::Inspect { id } => {
                let id = checked_uuid("object id", id)?;
                // `include_sizes=true` is always sent: the endpoint reports `byte_size`, never
                // the update/snapshot bytes themselves (`rest-api-v1.md`: "不返回 bytes"),
                // matching `cli-surface-v1.md`'s "raw update 只允许 collab inspect 读取
                // metadata". Called through the generic client, not a named wrapper method —
                // see `client.rs`'s Flow section doc comment for why.
                // `id` is already a canonicalized UUID (`checked_uuid`), safe to interpolate.
                let path = format!("/api/v1/flow/objects/{id}/collab?include_sizes=true");
                api_data(client.get_structured::<Value>(&path).await)
            }
            CollabAction::Verify { id, expected_head } => {
                let id = checked_uuid("object id", id)?;
                let mut body = json!({ "deep": false, "idempotency_key": Uuid::new_v4().to_string() });
                if let (Some(expected_head), Some(object)) = (expected_head, body.as_object_mut()) {
                    object.insert("expected_head_seq".to_string(), json!(expected_head));
                }
                let path = format!("/api/v1/flow/objects/{id}/collab/verify");
                let envelope: Value = client
                    .post_structured(&path, &body)
                    .await
                    .map_err(CliError::from_structured)?;
                let data = envelope.get("data").cloned().unwrap_or(Value::Null);
                if verify_found_mismatch(&data) {
                    return Err(CliError::integrity_mismatch(data));
                }
                Ok(data)
            }
        },
    }
}

/// Unwraps a structured API call's `{code, message, data}` envelope down to its `data`, which
/// is `sylvode`'s own stable `data` field (`cli-surface-v1.md`'s per-command "稳定 `data`"
/// column) — distinct from the legacy `mcp-server` tool convention of rendering the whole
/// envelope as tool output. `get_structured`/`put_structured::<Value>` deserialize the whole
/// envelope on success, exactly like the String-returning `get`/`put` this replaces, so `data`
/// still has to be pulled out here.
fn api_data(result: Result<Value, api_client::StructuredApiError>) -> Result<Value, CliError> {
    let envelope = result.map_err(CliError::from_structured)?;
    Ok(envelope.get("data").cloned().unwrap_or(Value::Null))
}

/// Best-effort read of whether a `collab verify` `OperationReceipt` found an inconsistency.
///
/// The exact vocabulary `OperationReceipt.status` uses is not yet observable: `POST
/// /flow/objects/{object_id}/collab/verify` is not wired into `apps/api`'s router yet (see
/// `client.rs`'s Flow section doc comment), and `rest-api-v1.md` only says the receipt
/// "检查 snapshot+tail/hash/head/projection" without enumerating `status` values. This treats
/// any `status` other than an "everything matched" spelling, or any non-empty `warnings`, as a
/// mismatch; it is flagged as an open question in the delivery report rather than presented as
/// settled.
fn verify_found_mismatch(data: &Value) -> bool {
    let status_signals_mismatch = data.get("status").and_then(Value::as_str).is_some_and(|status| {
        !matches!(
            status.to_ascii_lowercase().as_str(),
            "ok" | "consistent" | "clean" | "succeeded" | "completed" | "success"
        )
    });
    let has_warnings = data
        .get("warnings")
        .and_then(Value::as_array)
        .is_some_and(|warnings| !warnings.is_empty());
    status_signals_mismatch || has_warnings
}

fn checked_uuid(label: &str, value: &str) -> Result<String, CliError> {
    Uuid::parse_str(value.trim())
        .map(|id| id.to_string())
        .map_err(|_| CliError::usage(format!("{label} '{value}' is not a canonical UUID")))
}

fn query_suffix(params: &[String]) -> String {
    if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    }
}

#[cfg(test)]
mod tests {
    use super::verify_found_mismatch;
    use serde_json::json;

    #[test]
    fn clean_status_and_no_warnings_is_not_a_mismatch() {
        assert!(!verify_found_mismatch(&json!({ "status": "ok", "warnings": [] })));
    }

    #[test]
    fn unexpected_status_is_a_mismatch() {
        assert!(verify_found_mismatch(&json!({ "status": "inconsistent" })));
    }

    #[test]
    fn non_empty_warnings_is_a_mismatch_even_with_a_clean_status() {
        assert!(verify_found_mismatch(
            &json!({ "status": "ok", "warnings": ["head_seq mismatch"] })
        ));
    }
}
