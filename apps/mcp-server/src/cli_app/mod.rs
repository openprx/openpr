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
use command::{
    Cli, CollabAction, CollectionsAction, Commands, DeliveriesAction, FeaturesAction, FlowFeatureAction, GrantsAction,
    InheritanceAction, ObjectsAction, RecordsAction,
};
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
            ObjectsAction::Export { .. } => "objects.export".to_string(),
            ObjectsAction::Create { .. } => "objects.create".to_string(),
            ObjectsAction::Patch { .. } => "objects.patch".to_string(),
            ObjectsAction::Move { .. } => "objects.move".to_string(),
            ObjectsAction::Grants(grants) => match &grants.action {
                GrantsAction::Get { .. } => "objects.grants.get".to_string(),
                GrantsAction::Set { .. } => "objects.grants.set".to_string(),
            },
            ObjectsAction::Inheritance(inheritance) => match &inheritance.action {
                InheritanceAction::Set { .. } => "objects.inheritance.set".to_string(),
            },
            ObjectsAction::Link { .. } => "objects.link".to_string(),
            ObjectsAction::Unlink { .. } => "objects.unlink".to_string(),
            ObjectsAction::Reference { .. } => "objects.reference".to_string(),
            ObjectsAction::Unreference { .. } => "objects.unreference".to_string(),
            ObjectsAction::ConvertPreview { .. } => "objects.convert-preview".to_string(),
            ObjectsAction::ConvertCommit { .. } => "objects.convert-commit".to_string(),
            ObjectsAction::ConvertStatus { .. } => "objects.convert-status".to_string(),
            ObjectsAction::ConvertRetry { .. } => "objects.convert-retry".to_string(),
            ObjectsAction::Diff { .. } => "objects.diff".to_string(),
            ObjectsAction::Relations { .. } => "objects.relations".to_string(),
            ObjectsAction::Search { .. } => "objects.search".to_string(),
            ObjectsAction::Get { .. } => "objects.get".to_string(),
            ObjectsAction::Query { .. } => "objects.query".to_string(),
            ObjectsAction::History { .. } => "objects.history".to_string(),
        },
        Commands::Collections(cmd) => match &cmd.action {
            CollectionsAction::Describe { .. } => "collections.describe".to_string(),
            CollectionsAction::Query { .. } => "collections.query".to_string(),
        },
        Commands::Records(cmd) => match &cmd.action {
            RecordsAction::Create { .. } => "records.create".to_string(),
            RecordsAction::Patch { .. } => "records.patch".to_string(),
        },
        Commands::Collab(cmd) => match &cmd.action {
            CollabAction::Status { .. } => "collab.status".to_string(),
            CollabAction::Inspect { .. } => "collab.inspect".to_string(),
            CollabAction::Verify { .. } => "collab.verify".to_string(),
            CollabAction::ProjectionLag { .. } => "collab.projection-lag".to_string(),
            CollabAction::Compact { .. } => "collab.compact".to_string(),
            CollabAction::RebuildProjection { .. } => "collab.rebuild-projection".to_string(),
            CollabAction::Export { .. } => "collab.export".to_string(),
            CollabAction::ExportWorkspace { .. } => "collab.export-workspace".to_string(),
            CollabAction::ImportPreview { .. } => "collab.import-preview".to_string(),
            CollabAction::ImportCommit { .. } => "collab.import-commit".to_string(),
            CollabAction::ImportStatus { .. } => "collab.import-status".to_string(),
        },
        Commands::Deliveries(cmd) => match &cmd.action {
            DeliveriesAction::Replay { .. } => "deliveries.replay".to_string(),
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
            ObjectsAction::Export {
                id,
                render,
                at_seq,
                wait,
                idempotency_key,
            } => {
                let id = checked_uuid("object id", id)?;
                checked_idempotency_key(idempotency_key)?;
                let body =
                    json!({"format":render,"at_seq":at_seq,"include_history":false,"idempotency_key":idempotency_key});
                let created = api_data(
                    client
                        .post_structured::<Value, _>(&format!("/api/v1/flow/objects/{id}/exports"), &body)
                        .await,
                )?;
                if *wait {
                    let job_id = created
                        .get("job_id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| CliError::usage("export response has no job_id"))?;
                    api_data(
                        client
                            .get_structured::<Value>(&format!("/api/v1/flow/exports/{job_id}"))
                            .await,
                    )
                } else {
                    Ok(created)
                }
            }
            ObjectsAction::Create {
                workspace,
                project,
                object_type,
                title,
                parent,
                embed_page,
                schema_file,
                idempotency_key,
            } => {
                let workspace = checked_uuid("--workspace", workspace)?;
                checked_idempotency_key(idempotency_key)?;
                if title.trim().is_empty() {
                    return Err(CliError::usage("--title must not be empty"));
                }
                if parent.is_some() && embed_page.is_some() {
                    return Err(CliError::usage("--parent and --embed-page are mutually exclusive"));
                }
                let mut initial_schema = schema_file
                    .as_ref()
                    .map(|path| read_json_file(path))
                    .transpose()?
                    .unwrap_or_else(|| json!({}));
                {
                    let schema_object = initial_schema
                        .as_object_mut()
                        .ok_or_else(|| CliError::usage("--schema-file must contain a JSON object"))?;
                    schema_object.entry("initial_fields").or_insert_with(|| json!([]));
                    schema_object.entry("initial_view").or_insert(Value::Null);
                }
                let (path, mut body) = if let Some(page) = embed_page {
                    if object_type != "collection" {
                        return Err(CliError::usage("--embed-page requires --type collection"));
                    }
                    let page = checked_uuid("--embed-page", page)?;
                    if let Some(schema_object) = initial_schema.as_object_mut() {
                        schema_object.insert("title".to_string(), json!(title));
                    }
                    (
                        format!("/api/v1/flow/objects/{page}/commands"),
                        flow_command("create_collection_embed", &initial_schema, idempotency_key),
                    )
                } else {
                    if let Some(schema_object) = initial_schema.as_object_mut() {
                        schema_object.insert("object_type".to_string(), json!(object_type));
                        schema_object.insert("title".to_string(), json!(title));
                        schema_object.insert("idempotency_key".to_string(), json!(idempotency_key));
                    }
                    (format!("/api/v1/workspaces/{workspace}/flow/objects"), initial_schema)
                };
                if embed_page.is_none()
                    && let Some(object) = body.as_object_mut()
                {
                    if let Some(project) = project {
                        object.insert("project_id".to_string(), json!(checked_uuid("--project", project)?));
                    }
                    if let Some(parent) = parent {
                        object.insert("parent_object_id".to_string(), json!(checked_uuid("--parent", parent)?));
                    }
                }
                api_data(client.post_structured::<Value, _>(&path, &body).await)
            }
            ObjectsAction::Patch {
                id,
                patch_file,
                expected_frontier,
                idempotency_key,
            } => {
                let id = checked_uuid("object id", id)?;
                checked_idempotency_key(idempotency_key)?;
                let patch_document = read_json_file(patch_file)?;
                let operations = match patch_document {
                    Value::Array(operations) => operations,
                    Value::Object(mut object) => object
                        .remove("operations")
                        .and_then(|value| value.as_array().cloned())
                        .ok_or_else(|| {
                            CliError::usage(
                                "--patch-file must contain a JSON array or an object with an operations array",
                            )
                        })?,
                    _ => {
                        return Err(CliError::usage(
                            "--patch-file must contain a JSON array or an object with an operations array",
                        ));
                    }
                };
                if !(1..=100).contains(&operations.len()) {
                    return Err(CliError::usage(
                        "--patch-file must contain between 1 and 100 operations",
                    ));
                }
                let mut body = flow_command("semantic_patch", &json!({ "operations": operations }), idempotency_key);
                if let (Some(frontier), Some(object)) = (expected_frontier, body.as_object_mut()) {
                    object.insert("expected_frontier".to_string(), json!(frontier));
                }
                let path = format!("/api/v1/flow/objects/{id}/commands");
                api_data(client.post_structured::<Value, _>(&path, &body).await)
            }
            ObjectsAction::Move {
                id,
                parent,
                after,
                expected_target_frontier,
                confirm_self_lockout,
                idempotency_key,
            } => {
                let id = checked_uuid("object id", id)?;
                let parent = checked_uuid("--parent", parent)?;
                checked_idempotency_key(idempotency_key)?;
                let mut payload = json!({
                    "target_object_id": parent,
                    "confirm_self_lockout": confirm_self_lockout,
                });
                if let Some(object) = payload.as_object_mut() {
                    if let Some(after) = after {
                        object.insert("after_id".to_string(), json!(checked_uuid("--after", after)?));
                    }
                    if let Some(frontier) = expected_target_frontier {
                        object.insert("expected_target_frontier".to_string(), json!(frontier));
                    }
                }
                let body = flow_command("move_object", &payload, idempotency_key);
                let path = format!("/api/v1/flow/objects/{id}/commands");
                api_data(client.post_structured::<Value, _>(&path, &body).await)
            }
            ObjectsAction::Reference {
                source,
                target_type,
                target,
                display_file,
                idempotency_key,
            } => {
                let source = checked_uuid("source", source)?;
                let target = checked_uuid("--target", target)?;
                checked_idempotency_key(idempotency_key)?;
                let display = display_file
                    .as_ref()
                    .map(|path| read_json_file(path))
                    .transpose()?
                    .unwrap_or_else(|| json!({}));
                if !display.is_object() {
                    return Err(CliError::usage("--display-file must contain a JSON object"));
                }
                let path = format!("/api/v1/flow/objects/{source}/references");
                api_data(
                    client
                        .post_structured::<Value, _>(
                            &path,
                            &json!({
                    "target_type":target_type.replace('-', "_"),"target_id":target,"display":display,
                    "idempotency_key":idempotency_key}),
                        )
                        .await,
                )
            }
            ObjectsAction::Unreference {
                source,
                reference_id,
                idempotency_key,
            } => {
                let source = checked_uuid("source", source)?;
                let reference_id = checked_uuid("--reference", reference_id)?;
                checked_idempotency_key(idempotency_key)?;
                let path = format!("/api/v1/flow/objects/{source}/references/{reference_id}");
                api_data(
                    client
                        .delete_structured_with_idempotency::<Value>(&path, idempotency_key)
                        .await,
                )
            }
            ObjectsAction::ConvertPreview {
                source,
                source_frontier,
                target_type,
                mapping_file,
                idempotency_key,
            } => {
                let source = checked_uuid("source", source)?;
                checked_idempotency_key(idempotency_key)?;
                let mapping = read_json_file(mapping_file)?;
                if !mapping.is_object() {
                    return Err(CliError::usage("--mapping-file must contain a JSON object"));
                }
                api_data(
                    client
                        .post_structured::<Value, _>(
                            "/api/v1/flow/conversions/preview",
                            &json!({
                    "source_object_id":source,"source_frontier":source_frontier,
                    "target_type":target_type.replace('-', "_"),"mapping":mapping,"idempotency_key":idempotency_key}),
                        )
                        .await,
                )
            }
            ObjectsAction::ConvertCommit {
                preview_id,
                source_frontier,
                target_schema_version,
                confirm,
                idempotency_key,
            } => {
                let preview_id = checked_uuid("--preview", preview_id)?;
                checked_idempotency_key(idempotency_key)?;
                if !confirm {
                    return Err(CliError::usage("--confirm is required"));
                }
                api_data(client.post_structured::<Value, _>("/api/v1/flow/conversions", &json!({
                    "preview_id":preview_id,"source_frontier":source_frontier,"target_schema_version":target_schema_version,
                    "confirm":true,"idempotency_key":idempotency_key})).await)
            }
            ObjectsAction::ConvertStatus { job } => {
                let job = checked_uuid("job", job)?;
                api_data(
                    client
                        .get_structured::<Value>(&format!("/api/v1/flow/conversions/{job}"))
                        .await,
                )
            }
            ObjectsAction::ConvertRetry {
                job,
                confirm,
                idempotency_key,
            } => {
                let job = checked_uuid("job", job)?;
                checked_idempotency_key(idempotency_key)?;
                if !confirm {
                    return Err(CliError::usage("--confirm is required"));
                }
                api_data(
                    client
                        .post_structured::<Value, _>(
                            &format!("/api/v1/flow/conversions/{job}/retry"),
                            &json!({"confirm":true,"idempotency_key":idempotency_key}),
                        )
                        .await,
                )
            }
            ObjectsAction::Grants(grants) => match &grants.action {
                GrantsAction::Get { id } => {
                    let id = checked_uuid("object id", id)?;
                    let path = format!("/api/v1/flow/objects/{id}/grants");
                    api_data(client.get_structured::<Value>(&path).await)
                }
                GrantsAction::Set {
                    id,
                    grants,
                    confirm_self_lockout,
                    dry_run,
                    idempotency_key,
                } => {
                    let id = checked_uuid("object id", id)?;
                    checked_idempotency_key(idempotency_key)?;
                    if grants.len() > 100 {
                        return Err(CliError::usage("at most 100 --grant values may be supplied"));
                    }
                    let parsed = grants
                        .iter()
                        .map(|grant| parse_grant(grant))
                        .collect::<Result<Vec<_>, _>>()?;
                    let body = json!({
                        "grants": parsed,
                        "confirm_self_lockout": confirm_self_lockout,
                        "dry_run": dry_run,
                        "idempotency_key": idempotency_key,
                    });
                    let path = format!("/api/v1/flow/objects/{id}/grants");
                    api_data(client.put_structured::<Value, _>(&path, &body).await)
                }
            },
            ObjectsAction::Inheritance(inheritance) => match &inheritance.action {
                InheritanceAction::Set {
                    id,
                    inherit_from_parent,
                    confirm_self_lockout,
                    dry_run,
                    idempotency_key,
                } => {
                    let id = checked_uuid("object id", id)?;
                    checked_idempotency_key(idempotency_key)?;
                    let body = json!({
                        "inherit_from_parent": inherit_from_parent,
                        "confirm_self_lockout": confirm_self_lockout,
                        "dry_run": dry_run,
                        "idempotency_key": idempotency_key,
                    });
                    let path = format!("/api/v1/flow/objects/{id}/inheritance");
                    api_data(client.put_structured::<Value, _>(&path, &body).await)
                }
            },
            ObjectsAction::Link {
                source,
                target,
                relation_type,
                idempotency_key,
            } => {
                let source = checked_uuid("source object id", source)?;
                let target = checked_uuid("target object id", target)?;
                checked_idempotency_key(idempotency_key)?;
                let body = flow_command(
                    "link",
                    &json!({ "target_object_id": target, "relation_type": relation_type }),
                    idempotency_key,
                );
                let path = format!("/api/v1/flow/objects/{source}/commands");
                api_data(client.post_structured::<Value, _>(&path, &body).await)
            }
            ObjectsAction::Unlink {
                source,
                relation_id,
                idempotency_key,
            } => {
                let source = checked_uuid("source object id", source)?;
                let relation_id = checked_uuid("--relation", relation_id)?;
                checked_idempotency_key(idempotency_key)?;
                let body = flow_command("unlink", &json!({ "relation_id": relation_id }), idempotency_key);
                let path = format!("/api/v1/flow/objects/{source}/commands");
                api_data(client.post_structured::<Value, _>(&path, &body).await)
            }
            ObjectsAction::Diff {
                id,
                from_seq,
                to_seq,
                render,
            } => {
                let id = checked_uuid("object id", id)?;
                if from_seq > to_seq {
                    return Err(CliError::usage("--from must not exceed --to"));
                }
                let mut query = vec![format!("from_seq={from_seq}"), format!("to_seq={to_seq}")];
                if let Some(render) = render {
                    query.push(format!("render={}", render.replace('-', "_")));
                }
                let path = format!("/api/v1/flow/objects/{id}/diff{}", query_suffix(&query));
                api_data(client.get_structured::<Value>(&path).await)
            }
            ObjectsAction::Relations {
                id,
                direction,
                relation_type,
                cursor,
                limit,
            } => {
                let id = checked_uuid("object id", id)?;
                let mut query = Vec::new();
                if let Some(direction) = direction {
                    query.push(format!("direction={direction}"));
                }
                if let Some(relation_type) = relation_type {
                    query.push(format!("relation_type={}", encode_query_component(relation_type)));
                }
                if let Some(cursor) = cursor {
                    query.push(format!("cursor={}", encode_query_component(cursor)));
                }
                append_limit(&mut query, *limit)?;
                let path = format!("/api/v1/flow/objects/{id}/relations{}", query_suffix(&query));
                api_data(client.get_structured::<Value>(&path).await)
            }
            ObjectsAction::Search {
                workspace,
                project,
                unprojected,
                query: q,
                object_type,
                freshness,
                cursor,
                limit,
            } => {
                let workspace = checked_uuid("--workspace", workspace)?;
                if project.is_some() == *unprojected {
                    return Err(CliError::usage(
                        "exactly one of --project or --unprojected must be supplied",
                    ));
                }
                if q.is_empty() || q.chars().count() > 256 {
                    return Err(CliError::usage("--query must contain between 1 and 256 characters"));
                }
                let mut query = vec![format!("q={}", encode_query_component(q))];
                if let Some(project) = project {
                    query.push(format!("project_id={}", checked_uuid("--project", project)?));
                }
                if *unprojected {
                    query.push("unprojected=true".to_string());
                }
                if let Some(object_type) = object_type {
                    query.push(format!("object_type={}", encode_query_component(object_type)));
                }
                if let Some(freshness) = freshness {
                    query.push(format!("freshness={}", freshness.replace('-', "_")));
                }
                if let Some(cursor) = cursor {
                    query.push(format!("cursor={}", encode_query_component(cursor)));
                }
                append_limit(&mut query, *limit)?;
                let path = format!("/api/v1/workspaces/{workspace}/flow/search{}", query_suffix(&query));
                api_data(client.get_structured::<Value>(&path).await)
            }
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
        Commands::Collections(cmd) => match &cmd.action {
            CollectionsAction::Describe { id } => {
                let id = checked_uuid("collection id", id)?;
                api_data(
                    client
                        .get_structured::<Value>(&format!("/api/v1/flow/collections/{id}"))
                        .await,
                )
            }
            CollectionsAction::Query { id, query_file, cursor } => {
                let id = checked_uuid("collection id", id)?;
                let mut body = read_json_file(query_file)?;
                if !body.is_object() {
                    return Err(CliError::usage("--query-file must contain a JSON object"));
                }
                if let (Some(cursor), Some(object)) = (cursor, body.as_object_mut()) {
                    object.insert("cursor".to_string(), json!(cursor));
                }
                api_data(
                    client
                        .post_structured::<Value, _>(&format!("/api/v1/flow/collections/{id}/query"), &body)
                        .await,
                )
            }
        },
        Commands::Records(cmd) => {
            match &cmd.action {
                RecordsAction::Create {
                    collection,
                    values_file,
                    idempotency_key,
                    body,
                } => {
                    let collection = checked_uuid("--collection", collection)?;
                    checked_idempotency_key(idempotency_key)?;
                    let values = read_json_file(values_file)?;
                    if !values.is_object() {
                        return Err(CliError::usage(
                            "--values-file must contain a JSON object keyed by field UUID",
                        ));
                    }
                    api_data(client.post_structured::<Value, _>(&format!("/api/v1/flow/collections/{collection}/records"), &json!({"values_by_field_id": values, "body": body, "idempotency_key": idempotency_key})).await)
                }
                RecordsAction::Patch {
                    id,
                    values_file,
                    idempotency_key,
                    body,
                } => {
                    let id = checked_uuid("record id", id)?;
                    checked_idempotency_key(idempotency_key)?;
                    let values = read_json_file(values_file)?;
                    if !values.is_object() {
                        return Err(CliError::usage(
                            "--values-file must contain a JSON object keyed by field UUID",
                        ));
                    }
                    let record = api_data(
                        client
                            .get_structured::<Value>(&format!("/api/v1/flow/objects/{id}"))
                            .await,
                    )?;
                    let collection = record
                        .get("parent_id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| CliError::usage("record response has no owning Collection"))?;
                    let payload = json!({"record_id": id, "properties": values, "body": body});
                    api_data(
                        client
                            .post_structured::<Value, _>(
                                &format!("/api/v1/flow/objects/{collection}/commands"),
                                &flow_command("record_patch", &payload, idempotency_key),
                            )
                            .await,
                    )
                }
            }
        }
        Commands::Collab(cmd) => match &cmd.action {
            CollabAction::Status { workspace } => {
                let workspace = checked_uuid("--workspace", workspace)?;
                let health = api_data(
                    client
                        .get_structured::<Value>(&format!("/api/v1/admin/workspaces/{workspace}/flow/health"))
                        .await,
                )?;
                let lag = api_data(
                    client
                        .get_structured::<Value>(&format!("/api/v1/admin/workspaces/{workspace}/flow/lag"))
                        .await,
                )?;
                let integrity = api_data(
                    client
                        .get_structured::<Value>(&format!(
                            "/api/v1/admin/workspaces/{workspace}/flow/integrity?scope=summary"
                        ))
                        .await,
                )?;
                Ok(json!({"health":health,"lag":lag,"integrity":integrity}))
            }
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
            CollabAction::Verify {
                id,
                deep,
                expected_head,
            } => {
                let id = checked_uuid("object id", id)?;
                let mut body = json!({ "dry_run": true, "deep": deep, "idempotency_key": Uuid::new_v4().to_string() });
                if let (Some(expected_head), Some(object)) = (expected_head, body.as_object_mut()) {
                    object.insert("expected_head_seq".to_string(), json!(expected_head));
                }
                let path = if *deep {
                    format!("/api/v1/admin/flow/documents/{id}/verify")
                } else {
                    body.as_object_mut().map(|object| object.remove("dry_run"));
                    format!("/api/v1/flow/objects/{id}/collab/verify")
                };
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
            CollabAction::ProjectionLag {
                workspace,
                project,
                cursor,
                limit,
            } => {
                let workspace = checked_uuid("--workspace", workspace)?;
                let mut query = Vec::new();
                if let Some(project) = project {
                    query.push(format!("project_id={}", checked_uuid("--project", project)?));
                }
                if let Some(cursor) = cursor {
                    query.push(format!("cursor={}", encode_query_component(cursor)));
                }
                append_limit(&mut query, *limit)?;
                let path = format!(
                    "/api/v1/workspaces/{workspace}/flow/projection-lag{}",
                    query_suffix(&query)
                );
                api_data(client.get_structured::<Value>(&path).await)
            }
            CollabAction::Compact {
                id,
                dry_run,
                execute,
                expected_head,
                confirm,
                idempotency_key,
            } => {
                let id = checked_uuid("document id", id)?;
                let mode = checked_admin_mode(*dry_run, *execute, confirm.as_deref(), &id)?;
                checked_idempotency_key(idempotency_key)?;
                api_data(client.post_structured::<Value, _>(&format!("/api/v1/admin/flow/documents/{id}/compact"), &json!({"dry_run":mode,"expected_head_seq":expected_head,"confirm_document_id":confirm,"idempotency_key":idempotency_key})).await)
            }
            CollabAction::RebuildProjection {
                id,
                dry_run,
                execute,
                expected_head,
                confirm,
                idempotency_key,
            } => {
                let id = checked_uuid("object id", id)?;
                let mode = checked_admin_mode(*dry_run, *execute, confirm.as_deref(), &id)?;
                checked_idempotency_key(idempotency_key)?;
                api_data(client.post_structured::<Value, _>(&format!("/api/v1/admin/flow/objects/{id}/rebuild-projection"), &json!({"dry_run":mode,"expected_head_seq":expected_head,"confirm_object_id":confirm,"idempotency_key":idempotency_key})).await)
            }
            CollabAction::Export { id, idempotency_key } => {
                let id = checked_uuid("object id", id)?;
                checked_idempotency_key(idempotency_key)?;
                api_data(
                    client
                        .post_structured::<Value, _>(
                            &format!("/api/v1/flow/objects/{id}/exports"),
                            &json!({"format":"package","include_history":true,"idempotency_key":idempotency_key}),
                        )
                        .await,
                )
            }
            CollabAction::ExportWorkspace {
                workspace,
                include_history,
                idempotency_key,
            } => {
                let workspace = checked_uuid("--workspace", workspace)?;
                checked_idempotency_key(idempotency_key)?;
                api_data(client.post_structured::<Value, _>(&format!("/api/v1/workspaces/{workspace}/flow/exports"), &json!({"format":"package","include_history":include_history,"idempotency_key":idempotency_key})).await)
            }
            CollabAction::ImportPreview {
                workspace,
                package_file,
                mapping_file,
                idempotency_key,
            } => {
                let workspace = checked_uuid("--workspace", workspace)?;
                checked_idempotency_key(idempotency_key)?;
                let upload_path = format!("/api/v1/workspaces/{workspace}/flow/import-artifacts");
                let artifact = api_data(
                    client
                        .post_package_file_structured::<Value>(&upload_path, package_file, idempotency_key)
                        .await,
                )?;
                let artifact_id = artifact
                    .get("artifact_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CliError::usage("artifact response has no artifact_id"))?;
                let mapping = read_json_file(mapping_file)?;
                let project_mapping = mapping
                    .get("project_mapping")
                    .cloned()
                    .unwrap_or_else(|| mapping.clone());
                let external_policy = mapping
                    .get("external_reference_policy")
                    .and_then(Value::as_str)
                    .unwrap_or("reject");
                let conflict_policy = mapping
                    .get("conflict_policy")
                    .and_then(Value::as_str)
                    .unwrap_or("reject_existing");
                let include_history = mapping.get("include_history").and_then(Value::as_bool).unwrap_or(false);
                api_data(client.post_structured::<Value, _>(&format!("/api/v1/workspaces/{workspace}/flow/imports/preview"), &json!({"artifact_id":artifact_id,"project_mapping":project_mapping,"external_reference_policy":external_policy,"conflict_policy":conflict_policy,"include_history":include_history,"idempotency_key":idempotency_key})).await)
            }
            CollabAction::ImportCommit {
                workspace,
                import_id,
                package_hash,
                mapping_hash,
                conflict_policy,
                confirm,
                idempotency_key,
            } => {
                let workspace = checked_uuid("--workspace", workspace)?;
                let import_id = checked_uuid("--import", import_id)?;
                if !*confirm {
                    return Err(CliError::usage("--confirm is required"));
                }
                checked_idempotency_key(idempotency_key)?;
                api_data(client.post_structured::<Value, _>(&format!("/api/v1/workspaces/{workspace}/flow/imports/{import_id}/commit"), &json!({"package_sha256":package_hash,"mapping_hash":mapping_hash,"conflict_policy":conflict_policy.replace('-', "_"),"confirm":true,"idempotency_key":idempotency_key})).await)
            }
            CollabAction::ImportStatus {
                workspace,
                import_id,
                wait: _,
            } => {
                let workspace = checked_uuid("--workspace", workspace)?;
                let import_id = checked_uuid("--import", import_id)?;
                api_data(
                    client
                        .get_structured::<Value>(&format!("/api/v1/workspaces/{workspace}/flow/imports/{import_id}"))
                        .await,
                )
            }
        },
        Commands::Deliveries(cmd) => match &cmd.action {
            DeliveriesAction::Replay {
                workspace,
                mode,
                from_time,
                to_time,
                event_type,
                subscriber,
                dry_run,
                execute,
                confirm,
                idempotency_key,
            } => {
                let workspace = checked_uuid("--workspace", workspace)?;
                if *dry_run == *execute {
                    return Err(CliError::usage("exactly one of --dry-run or --execute is required"));
                }
                if !*confirm {
                    return Err(CliError::usage("--confirm is required"));
                }
                checked_idempotency_key(idempotency_key)?;
                let (subscriber_kind, subscriber_id) = subscriber
                    .as_deref()
                    .map(parse_subscriber)
                    .transpose()?
                    .unwrap_or((None, None));
                api_data(client.post_structured::<Value, _>(&format!("/api/v1/admin/workspaces/{workspace}/flow/deliveries/replay"), &json!({"mode":mode.replace('-', "_"),"event_type":event_type,"subscriber_kind":subscriber_kind,"subscriber_id":subscriber_id,"from":from_time,"to":to_time,"dry_run":dry_run,"confirm":true,"idempotency_key":idempotency_key})).await)
            }
        },
    }
}

fn checked_idempotency_key(value: &str) -> Result<(), CliError> {
    if (1..=128).contains(&value.len()) {
        Ok(())
    } else {
        Err(CliError::usage(
            "--idempotency-key must contain between 1 and 128 bytes",
        ))
    }
}

fn checked_admin_mode(dry_run: bool, execute: bool, confirm: Option<&str>, target: &str) -> Result<bool, CliError> {
    if dry_run == execute {
        return Err(CliError::usage("exactly one of --dry-run or --execute is required"));
    }
    if execute {
        let confirmed = confirm.ok_or_else(|| CliError::usage("--execute requires --confirm ID"))?;
        let confirmed = checked_uuid("--confirm", confirmed)?;
        if confirmed != target {
            return Err(CliError::usage("--confirm must exactly match the target ID"));
        }
    } else if confirm.is_some() {
        return Err(CliError::usage("--confirm is only valid with --execute"));
    }
    Ok(dry_run)
}

fn parse_subscriber(value: &str) -> Result<(Option<String>, Option<String>), CliError> {
    let (kind, id) = value
        .split_once(':')
        .ok_or_else(|| CliError::usage("--subscriber must use KIND:ID"))?;
    if kind.is_empty() {
        return Err(CliError::usage("--subscriber kind must not be empty"));
    }
    Ok((Some(kind.to_string()), Some(checked_uuid("--subscriber ID", id)?)))
}

fn flow_command(command_type: &str, payload: &Value, idempotency_key: &str) -> Value {
    json!({
        "command": { "type": command_type, "payload": payload },
        "idempotency_key": idempotency_key,
    })
}

fn read_json_file(path: &std::path::Path) -> Result<Value, CliError> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| CliError::usage(format!("failed to read {}: {error}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|error| CliError::usage(format!("{} is not valid JSON: {error}", path.display())))
}

fn parse_grant(value: &str) -> Result<Value, CliError> {
    let (principal, level) = value
        .split_once('=')
        .ok_or_else(|| CliError::usage("--grant must use KIND:ID=LEVEL"))?;
    let (kind, id) = principal
        .split_once(':')
        .ok_or_else(|| CliError::usage("--grant must use KIND:ID=LEVEL"))?;
    if !matches!(kind, "user" | "bot") {
        return Err(CliError::usage("--grant KIND must be user or bot"));
    }
    if !matches!(level, "full_access" | "edit" | "comment" | "view") {
        return Err(CliError::usage(
            "--grant LEVEL must be full_access, edit, comment, or view",
        ));
    }
    Ok(json!({
        "principal_kind": kind,
        "principal_id": checked_uuid("--grant principal id", id)?,
        "level": level,
    }))
}

fn append_limit(query: &mut Vec<String>, limit: Option<u64>) -> Result<(), CliError> {
    if let Some(limit) = limit {
        if !(1..=100).contains(&limit) {
            return Err(CliError::usage("--limit must be between 1 and 100"));
        }
        query.push(format!("limit={limit}"));
    }
    Ok(())
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
